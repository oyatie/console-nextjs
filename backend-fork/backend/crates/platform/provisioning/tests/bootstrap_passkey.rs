//! OTP first-sign-in (bootstrap credential) redemption tests.
//!
//! The OTP belongs to a pre-provisioned user (or cold-start admin). Redemption
//! Legacy version-0 redemption verifies without consumption; successful passkey
//! registration consumes it. The dormant version-1 owner path consumes proof
//! in its caller's family/admission transaction. There is no per-OTP attempt
//! cap, which would enable targeted lockout; expiry and REST rate limits bound
//! guessing.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_kernel_core::OrgId;
use console_platform_provisioning::{BootstrapCredentialStore, ProvisioningError};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn seed_user(pool: &PgPool) -> uuid::Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (display_name, phone, roles, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind("Cold Start User")
    .bind("010-3000-0001")
    .bind(Vec::<String>::from(["MECHANIC".to_owned()]))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The later session owner must be able to keep verification, source custody,
/// family issuance and audit inside one transaction. Rolling its transaction
/// back must leave neither accepted audit nor changed OTP state behind.
#[sqlx::test(migrations = "../db/migrations")]
async fn redemption_can_be_rolled_back_with_its_source_lock(pool: PgPool) {
    let user_id = seed_user(&pool).await;
    let store = BootstrapCredentialStore;
    let now = OffsetDateTime::now_utc();
    let issued = store
        .issue_for_zero_credential_user(&pool, user_id, OrgId::knl(), now, Duration::hours(1))
        .await
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    let redeemed = store
        .redeem_otp_in_tx(&mut tx, issued.token.as_str(), now)
        .await
        .unwrap();
    assert_eq!(redeemed.user_id, user_id);
    assert_eq!(redeemed.source.credential_id, issued.credential_id);
    assert_eq!(redeemed.source.issuance_version, 0);
    assert_eq!(redeemed.source.issued_generation, None);
    assert_eq!(redeemed.source.issuance_purpose, None);
    assert_eq!(redeemed.source.source_operation_id, None);

    let pending_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE target_id=$1 AND action='auth.otp.redeem'",
    )
    .bind(issued.credential_id.to_string())
    .fetch_one(tx.as_mut())
    .await
    .unwrap();
    assert_eq!(pending_audits, 1);

    // Another owner cannot replace/revoke the exact source while the caller
    // still holds the verification transaction's row lock.
    let mut contender = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='100ms'")
        .execute(contender.as_mut())
        .await
        .unwrap();
    let blocked = sqlx::query(
        "UPDATE auth_bootstrap_credentials SET revoked_at=clock_timestamp() WHERE id=$1",
    )
    .bind(issued.credential_id)
    .execute(contender.as_mut())
    .await
    .unwrap_err();
    assert_eq!(
        blocked
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("55P03")
    );
    contender.rollback().await.unwrap();
    tx.rollback().await.unwrap();

    let persisted: (Option<OffsetDateTime>, Option<OffsetDateTime>, i64) = sqlx::query_as(
        "SELECT b.consumed_at, b.revoked_at, \
         (SELECT count(*) FROM audit_events a WHERE a.target_id=b.id::text AND a.action='auth.otp.redeem') \
         FROM auth_bootstrap_credentials b WHERE b.id=$1",
    )
    .bind(issued.credential_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(persisted, (None, None, 0));
    assert!(
        store
            .redeem_otp(&pool, issued.token.as_str(), now)
            .await
            .is_ok()
    );
}

/// A version-1 first-enrollment source is authority only for the exact current
/// never-enrolled Account generation. A later generation or prior-enrollment
/// state must reject the same still-open OTP without recording accepted proof.
#[sqlx::test(migrations = "../db/migrations")]
async fn versioned_first_enrollment_redeem_rejects_stale_or_prior_enrollment(pool: PgPool) {
    let store = BootstrapCredentialStore;
    let now = OffsetDateTime::now_utc();
    let user_id = seed_user(&pool).await;
    let source_id = Uuid::new_v4();
    let source_operation_id = Uuid::new_v4();
    let otp = format!("first-enrollment-{}", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO auth_bootstrap_credentials \
         (id,user_id,token_hash,issued_at,expires_at,org_id,issuance_version,issued_generation,issuance_purpose,source_operation_id) \
         VALUES ($1,$2,$3,$4,$5,$6,1,1,'first_enrollment',$7)",
    )
    .bind(source_id)
    .bind(user_id)
    .bind(Sha256::digest(otp.as_bytes()).to_vec())
    .bind(now)
    .bind(now + Duration::hours(1))
    .bind(*OrgId::knl().as_uuid())
    .bind(source_operation_id)
    .execute(&pool)
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();
    let valid = store.redeem_otp_in_tx(&mut tx, &otp, now).await.unwrap();
    assert_eq!(valid.source.credential_id, source_id);
    assert_eq!(valid.source.issued_generation, Some(1));
    assert_eq!(valid.source.source_operation_id, Some(source_operation_id));
    let consumed: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT consumed_at FROM auth_bootstrap_credentials WHERE id=$1 AND user_id=$2",
    )
    .bind(source_id)
    .bind(user_id)
    .fetch_one(tx.as_mut())
    .await
    .unwrap();
    assert!(
        consumed.is_some(),
        "version-1 proof must be consumed before its owner commits a family"
    );
    let replay = store.redeem_otp_in_tx(&mut tx, &otp, now).await;
    assert!(
        matches!(replay, Err(ProvisioningError::InvalidBootstrapCredential)),
        "one source cannot be redeemed twice in one owner transaction"
    );
    tx.rollback().await.unwrap();

    // The public verify-and-commit wrapper has no family/admission owner. It
    // must not consume a version-1 proof on its own and strand the Account.
    let unowned = store.redeem_otp(&pool, &otp, now).await;
    assert!(
        matches!(unowned, Err(ProvisioningError::InvalidBootstrapCredential)),
        "a version-1 OTP requires its caller's family/admission transaction"
    );
    let unowned_consumed: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT consumed_at FROM auth_bootstrap_credentials WHERE id=$1 AND user_id=$2",
    )
    .bind(source_id)
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        unowned_consumed, None,
        "the public wrapper must roll back version-1 consumption"
    );

    sqlx::query(
        "UPDATE auth_security.account_state SET auth_generation=2, revision=revision+1 \
         WHERE account_id=$1 AND home_org_id=$2",
    )
    .bind(user_id)
    .bind(*OrgId::knl().as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let mut stale_tx = pool.begin().await.unwrap();
    let stale = store.redeem_otp_in_tx(&mut stale_tx, &otp, now).await;
    assert!(
        matches!(stale, Err(ProvisioningError::InvalidBootstrapCredential)),
        "a source from the prior authentication generation must be denied"
    );
    stale_tx.rollback().await.unwrap();

    let prior_user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, phone, roles, org_id) \
         VALUES ('Prior Enrollment User','010-3000-0002',ARRAY['MECHANIC'],$1) RETURNING id",
    )
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let prior_source = Uuid::new_v4();
    let prior_otp = format!("prior-enrollment-{}", Uuid::new_v4());
    sqlx::query(
        "UPDATE auth_security.account_state SET ever_enrolled=true, status='recovery_required', \
         revision=revision+1 WHERE account_id=$1 AND home_org_id=$2",
    )
    .bind(prior_user)
    .bind(*OrgId::knl().as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO auth_bootstrap_credentials \
         (id,user_id,token_hash,issued_at,expires_at,org_id,issuance_version,issued_generation,issuance_purpose,source_operation_id) \
         VALUES ($1,$2,$3,$4,$5,$6,1,1,'first_enrollment',$7)",
    )
    .bind(prior_source)
    .bind(prior_user)
    .bind(Sha256::digest(prior_otp.as_bytes()).to_vec())
    .bind(now)
    .bind(now + Duration::hours(1))
    .bind(*OrgId::knl().as_uuid())
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    let mut prior_tx = pool.begin().await.unwrap();
    let prior = store.redeem_otp_in_tx(&mut prior_tx, &prior_otp, now).await;
    assert!(
        matches!(prior, Err(ProvisioningError::InvalidBootstrapCredential)),
        "a previously enrolled zero-key Account needs recovery, not first enrollment"
    );
    prior_tx.rollback().await.unwrap();
    let accepted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action='auth.otp.redeem' \
         AND target_id IN ($1,$2)",
    )
    .bind(source_id.to_string())
    .bind(prior_source.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(accepted, 0, "denied proofs must not leave accepted audits");
}

/// Issued OTP format: exactly 8 characters over the documented alphanumeric +
/// special alphabet.
#[sqlx::test(migrations = "../db/migrations")]
async fn issued_otp_is_eight_char_alphanumeric_special(pool: PgPool) {
    let user_id = seed_user(&pool).await;
    let now = OffsetDateTime::now_utc();
    let issue = BootstrapCredentialStore
        .issue_for_zero_credential_user(&pool, user_id, OrgId::knl(), now, Duration::hours(24))
        .await
        .unwrap();

    let token = issue.token.as_str();
    assert_eq!(token.chars().count(), 8, "OTP must be exactly 8 characters");
    const ALLOWED: &str =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#$%^&*-_";
    assert!(
        token.chars().all(|c| ALLOWED.contains(c)),
        "OTP must use only the documented alphabet, got {token:?}"
    );

    // The hash is stored, never the plaintext.
    let token_hash: Vec<u8> =
        sqlx::query("SELECT token_hash FROM auth_bootstrap_credentials WHERE id = $1")
            .bind(issue.credential_id)
            .fetch_one(&pool)
            .await
            .unwrap()
            .try_get("token_hash")
            .unwrap();
    assert_ne!(token_hash, token.as_bytes());
}

/// A caller can wait on the Account lock longer than the requested TTL. The
/// source owner must start the full validity window after that wait, not at the
/// time the request first entered Rust.
#[sqlx::test(migrations = "../db/migrations")]
async fn issued_otp_uses_post_lock_database_time(pool: PgPool) {
    let user_id = seed_user(&pool).await;
    let ttl = Duration::hours(1);
    let db_before: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&pool)
        .await
        .unwrap();
    let stale_request_time = db_before - Duration::hours(2);

    let issue = BootstrapCredentialStore
        .issue_for_zero_credential_user(&pool, user_id, OrgId::knl(), stale_request_time, ttl)
        .await
        .unwrap();
    let db_after: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&pool)
        .await
        .unwrap();
    let stored: (OffsetDateTime, OffsetDateTime) =
        sqlx::query_as("SELECT issued_at, expires_at FROM auth_bootstrap_credentials WHERE id=$1")
            .bind(issue.credential_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(stored.0 >= db_before && stored.0 <= db_after);
    assert_eq!(stored.1, stored.0 + ttl);
    assert_eq!(issue.expires_at, stored.1);
    let audit = sqlx::query(
        "SELECT occurred_at, after_snap FROM audit_events \
         WHERE action='auth.bootstrap.issue' AND target_id=$1",
    )
    .bind(user_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (
            audit.get::<OffsetDateTime, _>("occurred_at"),
            audit.get::<serde_json::Value, _>("after_snap")["expires_at"].clone()
        ),
        (stored.0, serde_json::json!(stored.1))
    );
    assert!(
        BootstrapCredentialStore
            .redeem_otp(&pool, issue.token.as_str(), db_after)
            .await
            .is_ok()
    );
}

/// A redeem VERIFIES the code and resolves the user but does NOT consume it, so a
/// failed enrollment can't lock the user out — the code stays usable until a passkey
/// is actually registered. consume_open_credentials_tx (driven by passkey
/// registration) is the single point of consumption; after it the code is dead.
#[sqlx::test(migrations = "../db/migrations")]
async fn redeem_verifies_without_consuming_then_registration_consumes(pool: PgPool) {
    let user_id = seed_user(&pool).await;
    let store = BootstrapCredentialStore;
    let now = OffsetDateTime::now_utc();

    let issue = store
        .issue_for_zero_credential_user(&pool, user_id, OrgId::knl(), now, Duration::hours(24))
        .await
        .unwrap();

    let redemption = store
        .redeem_otp(&pool, issue.token.as_str(), now)
        .await
        .unwrap();
    assert_eq!(redemption.user_id, user_id);
    assert!(
        redemption.requires_passkey_setup,
        "a zero-passkey user must be flagged for passkey setup"
    );

    let consumed_at: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_bootstrap_credentials WHERE id = $1")
            .bind(issue.credential_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(consumed_at.is_none(), "a redeem must NOT consume the code");

    // A second redeem before registration STILL succeeds (no lockout).
    assert!(
        store
            .redeem_otp(&pool, issue.token.as_str(), now)
            .await
            .is_ok(),
        "the code stays redeemable until a passkey is registered"
    );

    // Registration consumes it atomically (here exercised directly).
    let mut tx = pool.begin().await.unwrap();
    store
        .consume_open_credentials_tx(&mut tx, OrgId::knl(), user_id, now)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let consumed_at: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_bootstrap_credentials WHERE id = $1")
            .bind(issue.credential_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(consumed_at.is_some(), "registration consumes the code");

    // Once consumed, a redeem is rejected.
    assert!(
        store
            .redeem_otp(&pool, issue.token.as_str(), now)
            .await
            .is_err(),
        "a consumed code must not redeem again"
    );
}

/// A WRONG guess must NOT consume or invalidate a legitimate user's OTP;
/// the current passkey-registration path is the only consumer.
#[sqlx::test(migrations = "../db/migrations")]
async fn wrong_guess_does_not_consume_the_otp(pool: PgPool) {
    let user_id = seed_user(&pool).await;
    let store = BootstrapCredentialStore;
    let now = OffsetDateTime::now_utc();

    let issue = store
        .issue_for_zero_credential_user(&pool, user_id, OrgId::knl(), now, Duration::hours(24))
        .await
        .unwrap();

    // Several wrong guesses.
    for guess in ["wrongone", "????????", "00000000"] {
        let result = store.redeem_otp(&pool, guess, now).await;
        assert!(result.is_err(), "a wrong guess must be rejected");
    }

    // The credential is still unconsumed and the real OTP still works.
    let consumed_at: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_bootstrap_credentials WHERE id = $1")
            .bind(issue.credential_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        consumed_at.is_none(),
        "wrong guesses must not consume the OTP"
    );

    let redemption = store
        .redeem_otp(&pool, issue.token.as_str(), now)
        .await
        .unwrap();
    assert_eq!(redemption.user_id, user_id);
}

/// The default-24h OTP works inside its window and is rejected after expiry.
#[sqlx::test(migrations = "../db/migrations")]
async fn otp_expiry_is_enforced_on_redeem(pool: PgPool) {
    let user_id = seed_user(&pool).await;
    let store = BootstrapCredentialStore;
    let now = OffsetDateTime::now_utc();

    let issue = store
        .issue_for_zero_credential_user(&pool, user_id, OrgId::knl(), now, Duration::hours(24))
        .await
        .unwrap();
    let issued_at: OffsetDateTime =
        sqlx::query_scalar("SELECT issued_at FROM auth_bootstrap_credentials WHERE id=$1")
            .bind(issue.credential_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(issued_at >= now);
    assert_eq!(issue.expires_at, issued_at + Duration::hours(24));

    // Within the window (just before expiry): redeem succeeds.
    let within = issue.expires_at - Duration::minutes(1);
    let redemption = store
        .redeem_otp(&pool, issue.token.as_str(), within)
        .await
        .unwrap();
    assert_eq!(redemption.user_id, user_id);

    // A fresh OTP, redeemed after its expiry, is rejected.
    let user2 = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO users (display_name, phone, roles, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind("Expired OTP User")
    .bind("010-3000-0002")
    .bind(Vec::<String>::from(["MECHANIC".to_owned()]))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let issue2 = store
        .issue_for_zero_credential_user(&pool, user2, OrgId::knl(), now, Duration::hours(1))
        .await
        .unwrap();
    let after_expiry = issue2.expires_at + Duration::seconds(1);
    let expired = store
        .redeem_otp(&pool, issue2.token.as_str(), after_expiry)
        .await;
    assert!(expired.is_err(), "an expired OTP must be rejected");

    let consumed_at: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_bootstrap_credentials WHERE id = $1")
            .bind(issue2.credential_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        consumed_at.is_none(),
        "an expired OTP must not be consumed by a redeem attempt"
    );
}

/// The cold-start OTP is no longer a committed constant: migration 0023 revoked
/// the fixed "coss0000" seed, so right after migrations there is NO open
/// cold-start credential. The OTP is now seeded at app boot via
/// `seed_cold_start_credential`. Once seeded it signs the cold admin in; a redeem
/// does not consume it (so a failed first-boot enrollment can't brick cold start);
/// it is consumed — and dead — once the admin registers a passkey.
#[sqlx::test(migrations = "../db/migrations")]
async fn cold_start_otp_seeded_at_boot_signs_in_then_dies_on_passkey_registration(pool: PgPool) {
    let store = BootstrapCredentialStore;
    let now = OffsetDateTime::now_utc();

    // The migration keeps the Cold Start Admin (SUPER_ADMIN) user row...
    let admin_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM users WHERE display_name = 'Cold Start Admin' AND roles @> ARRAY['SUPER_ADMIN']::text[]",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    // ...but the fixed seed is revoked: coss0000 must NOT redeem until re-seeded.
    assert!(
        store.redeem_otp(&pool, "coss0000", now).await.is_err(),
        "the committed coss0000 seed must be revoked by migration 0023"
    );

    // Boot-time seeding with the deploy-time secret.
    let seeded = store
        .seed_cold_start_credential(&pool, "coss0000", Duration::hours(1), now)
        .await
        .unwrap();
    assert!(
        seeded,
        "the cold admin has no passkey/open credential -> seeded"
    );

    let redemption = store.redeem_otp(&pool, "coss0000", now).await.unwrap();
    assert_eq!(redemption.user_id, admin_id);
    assert!(redemption.requires_passkey_setup);

    // Redeem does NOT consume — coss0000 stays usable until the admin enrolls a passkey.
    assert!(
        store.redeem_otp(&pool, "coss0000", now).await.is_ok(),
        "coss0000 stays redeemable until the admin registers a passkey"
    );

    // Passkey registration consumes it; afterwards it is dead.
    let mut tx = pool.begin().await.unwrap();
    store
        .consume_open_credentials_tx(&mut tx, OrgId::platform(), admin_id, now)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(
        store.redeem_otp(&pool, "coss0000", now).await.is_err(),
        "coss0000 is dead once the admin has a passkey"
    );
}

/// `seed_cold_start_credential` is idempotent and gated: it seeds only when the
/// cold admin has neither a passkey nor an open credential, returns the seeded OTP
/// as redeemable, and skips (returns false) once a credential is already open or a
/// passkey exists.
#[sqlx::test(migrations = "../db/migrations")]
async fn seed_cold_start_credential_is_gated_and_idempotent(pool: PgPool) {
    let store = BootstrapCredentialStore;
    let now = OffsetDateTime::now_utc();

    let admin_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM users WHERE display_name = 'Cold Start Admin' AND roles @> ARRAY['SUPER_ADMIN']::text[]",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    // First seed succeeds (no passkey, no open credential after 0023's revoke).
    let first = store
        .seed_cold_start_credential(&pool, "secret-otp", Duration::hours(1), now)
        .await
        .unwrap();
    assert!(first, "first seed must insert a credential");

    // The seeded token redeems via redeem_otp for the cold admin.
    let redemption = store.redeem_otp(&pool, "secret-otp", now).await.unwrap();
    assert_eq!(redemption.user_id, admin_id);

    // A second seed is a no-op: an open credential already exists.
    let second = store
        .seed_cold_start_credential(&pool, "another-otp", Duration::hours(1), now)
        .await
        .unwrap();
    assert!(!second, "a second seed must skip when a credential is open");

    // The audit trail records exactly one coldstart seed and never the OTP value.
    let seed_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'auth.coldstart.seed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(seed_audits, 1, "exactly one coldstart seed must be audited");
    let leaked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events \
         WHERE action = 'auth.coldstart.seed' AND after_snap::text LIKE '%secret-otp%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        leaked, 0,
        "the OTP value must never appear in the audit snapshot"
    );

    // Consume the open credential (simulating passkey registration), then a seed
    // still skips because the admin now has a passkey.
    let mut tx = pool.begin().await.unwrap();
    store
        .consume_open_credentials_tx(&mut tx, OrgId::platform(), admin_id, now)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    // Give the admin a passkey so the no-passkey gate is exercised.
    sqlx::query(
        "INSERT INTO auth_webauthn_credentials \
         (user_id, credential_id, passkey_json, org_id) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(admin_id)
    .bind("cred-id")
    .bind(serde_json::json!({}))
    // The Cold Start Admin is the PLATFORM admin, re-homed to the platform
    // sentinel org by migration 0036; its passkey must carry that same org to
    // satisfy the (user_id, org_id) composite FK to `users`.
    .bind(*OrgId::platform().as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let third = store
        .seed_cold_start_credential(&pool, "third-otp", Duration::hours(1), now)
        .await
        .unwrap();
    assert!(!third, "a seed must skip once the admin has a passkey");
}

/// Cold-start seeding also waits on the Account lock and must begin its TTL
/// after that wait. A stale request time cannot create an already-expired code.
#[sqlx::test(migrations = "../db/migrations")]
async fn cold_start_seed_uses_post_lock_database_time(pool: PgPool) {
    let admin_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM users WHERE display_name='Cold Start Admin' AND roles @> ARRAY['SUPER_ADMIN']::text[]",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let ttl = Duration::hours(1);
    let db_before: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&pool)
        .await
        .unwrap();
    let stale_request_time = db_before - Duration::hours(2);

    assert!(
        BootstrapCredentialStore
            .seed_cold_start_credential(&pool, "clock-bound-coldstart", ttl, stale_request_time)
            .await
            .unwrap()
    );
    let db_after: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&pool)
        .await
        .unwrap();
    let stored: (uuid::Uuid, OffsetDateTime, OffsetDateTime) = sqlx::query_as(
        "SELECT id, issued_at, expires_at FROM auth_bootstrap_credentials \
         WHERE user_id=$1 AND consumed_at IS NULL AND revoked_at IS NULL",
    )
    .bind(admin_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(stored.1 >= db_before && stored.1 <= db_after);
    assert_eq!(stored.2, stored.1 + ttl);
    let audit_time: OffsetDateTime = sqlx::query_scalar(
        "SELECT occurred_at FROM audit_events \
         WHERE action='auth.coldstart.seed' AND target_id=$1",
    )
    .bind(stored.0.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_time, stored.1);
    assert!(
        BootstrapCredentialStore
            .redeem_otp(&pool, "clock-bound-coldstart", db_after)
            .await
            .is_ok()
    );
}

/// An EXPIRED open cold-start credential must not wedge cold-start: a later boot
/// re-seeds (revives the expired row) so the operator gets a fresh redeemable
/// window. Regression for the seeder's expiry-blind "open credential" gate.
#[sqlx::test(migrations = "../db/migrations")]
async fn seed_cold_start_credential_reseeds_after_expiry(pool: PgPool) {
    let store = BootstrapCredentialStore;
    let now = OffsetDateTime::now_utc();

    // Seed a short-lived credential.
    let first = store
        .seed_cold_start_credential(&pool, "expiring-otp", Duration::hours(1), now)
        .await
        .unwrap();
    assert!(first, "first seed must insert");
    let admin_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM users WHERE display_name = 'Cold Start Admin' AND roles @> ARRAY['SUPER_ADMIN']::text[]",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let source_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM auth_bootstrap_credentials WHERE user_id=$1 AND token_hash=digest('expiring-otp','sha256')",
    )
    .bind(admin_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let old_family = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO auth_refresh_token_families(id,user_id,created_at,org_id) VALUES($1,$2,$3,$4)",
    )
    .bind(old_family)
    .bind(admin_id)
    .bind(now)
    .bind(*OrgId::platform().as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO auth_legacy_otp_family_sources(family_id,source_id,user_id,org_id) VALUES($1,$2,$3,$4)",
    )
    .bind(old_family)
    .bind(source_id)
    .bind(admin_id)
    .bind(*OrgId::platform().as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    // Past its TTL the same OTP no longer redeems.
    let later = now + Duration::hours(2);
    assert!(
        store
            .redeem_otp(&pool, "expiring-otp", later)
            .await
            .is_err(),
        "the credential must be expired at `later`"
    );

    // A boot at `later` must RE-SEED (revive the expired row), not skip.
    let reseeded = store
        .seed_cold_start_credential(&pool, "expiring-otp", Duration::hours(1), later)
        .await
        .unwrap();
    assert!(
        reseeded,
        "an expired open credential must not block re-seeding"
    );
    let old_revoked: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT revoked_at FROM auth_refresh_token_families WHERE id=$1")
            .bind(old_family)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        old_revoked.is_some(),
        "reusing a cold-start source must fence its old family"
    );

    // ...and the refreshed credential redeems again at `later`.
    let redemption = store
        .redeem_otp(&pool, "expiring-otp", later)
        .await
        .unwrap();
    assert_eq!(redemption.user_id, admin_id);
}
