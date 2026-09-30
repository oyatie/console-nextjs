#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RLS-as-`console_rt` gate for the cross-device passkey-enrollment self-handoff
//! (`BootstrapCredentialStore::issue_self_enroll_handoff`).
//!
//! The self-handoff lets a user on a DESKTOP mint a fresh single-use, short-TTL
//! one-time code FOR THEMSELVES, render it as a QR, and finish passkey enrollment
//! on their PHONE — no Bluetooth / caBLE hybrid tunnel. Because it is a
//! credential-handoff path it must hold the same RLS + single-use + expiry
//! invariants as the rest of the bootstrap machinery, and it must NEVER mint a
//! code for another user.
//!
//! Every statement here runs as the genuine non-owner `console_rt` role (FORCE RLS
//! applies, BYPASSRLS does not), exactly like production, so the test fails closed
//! if the issuance forgets to arm `app.current_org`.
//!
//! Proven, as `console_rt`:
//!   * SELF-ONLY: the minted handoff is owned by exactly the issuing user, stamped
//!     with the issuer's own org, and is invisible under any other tenant's GUC.
//!   * SINGLE-USE: a handoff redeems → the user enrolls a passkey → the code is
//!     consumed and a second redeem of the SAME code fails.
//!   * EXPIRY: an expired handoff does not redeem.
//!   * HANDOFF → ENROLL: the happy path (issue → redeem → register passkey)
//!     works end to end as the runtime role.
//!   * SUPERSEDE: minting a handoff while the user already holds an OPEN code
//!     revokes the stale code (the one-open-per-user invariant) and the new one
//!     redeems while the old one no longer does.

use console_kernel_core::OrgId;
use console_platform_auth::{
    PasskeyRegistrationStart, PasskeyService, RefreshTokenStore, WebauthnSettings,
};
use console_platform_provisioning::{BootstrapCredentialStore, ProvisioningError};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use url::Url;
use uuid::Uuid;
use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;

/// A second, non-KNL tenant id, used to prove cross-tenant isolation of a handoff.
const ORG_T2: Uuid = Uuid::from_u128(0x3333_3333_3333_3333_3333_3333_3333_3333);

/// Short handoff TTL the REST layer uses (5 min); mirrored here.
const HANDOFF_TTL: Duration = Duration::minutes(5);

fn passkey_service() -> PasskeyService {
    PasskeyService::new(WebauthnSettings {
        rp_id: "example.com".to_owned(),
        rp_origin: Url::parse("https://auth.example.com").unwrap(),
        rp_name: "Console".to_owned(),
        extra_allowed_origins: vec![],
        ceremony_ttl: Duration::minutes(5),
    })
    .unwrap()
}

/// Build a SECOND pool whose every connection runs `SET ROLE console_rt` on checkout,
/// so statements execute as the genuine non-owner RUNTIME role — FORCE RLS
/// applies and BYPASSRLS does not — exactly as production connects.
async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

/// Seed an `organizations` row + one user in it, as the OWNER (superuser) pool
/// with `row_security` off so the rows go in regardless of any GUC. Returns the
/// new user's id.
async fn seed_org_and_user(owner_pool: &PgPool, org: Uuid, tag: &str) -> Uuid {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
        .bind(org)
        .bind(format!("org-{}", tag.to_lowercase()))
        .bind(format!("Org {tag}"))
        .execute(&mut *tx)
        .await
        .unwrap();
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("User {tag}"))
    .bind(vec!["MECHANIC".to_string()])
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    user_id
}

/// Register a discoverable passkey for `user_id` in `org` as `console_rt`, returning
/// the stored credential id. The registration-finish INSERT is org-stamped.
async fn register_passkey_as_runtime(
    service: &PasskeyService,
    rt_pool: &PgPool,
    org: OrgId,
    user_id: Uuid,
) -> String {
    let registration = service
        .start_registration(
            rt_pool,
            org,
            PasskeyRegistrationStart {
                user_id,
                username: "handoff.user".to_owned(),
                display_name: "Handoff User".to_owned(),
            },
        )
        .await
        .expect("start_registration must succeed as console_rt");

    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = authenticator
        .do_registration(
            Url::parse("https://auth.example.com").unwrap(),
            registration.challenge,
        )
        .unwrap();

    let stored = service
        .finish_registration(rt_pool, org, registration.ceremony_id, credential)
        .await
        .expect("finish_registration must INSERT the passkey as console_rt");
    stored.credential_id
}

/// Consume the user's open code atomically (the single point of single-use
/// enforcement; in production this runs in the same tx as the passkey insert).
async fn consume_open_code_as_runtime(rt_pool: &PgPool, org: OrgId, user_id: Uuid) {
    let mut tx = rt_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    BootstrapCredentialStore
        .consume_open_credentials_tx(&mut tx, org, user_id, OffsetDateTime::now_utc())
        .await
        .expect("consume must succeed as console_rt");
    tx.commit().await.unwrap();
}

/// Read a bootstrap credential's owning user + org by its OTP, as `console_rt` with
/// the GUC armed to `org`.
async fn handoff_owner_as_runtime(rt_pool: &PgPool, org: OrgId, otp: &str) -> Option<(Uuid, Uuid)> {
    use sha2::{Digest, Sha256};
    let token_hash = Sha256::digest(otp.as_bytes()).to_vec();
    let mut tx = rt_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT user_id, org_id FROM auth_bootstrap_credentials WHERE token_hash = $1",
    )
    .bind(&token_hash)
    .fetch_optional(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    row
}

// ===========================================================================
// (1) SELF-ONLY: the minted handoff belongs to exactly the issuing user, is
// stamped with the issuer's own org, and is invisible under another tenant's GUC.
// ===========================================================================
#[sqlx::test(migrations = "../db/migrations")]
async fn handoff_is_scoped_to_the_issuing_user_only(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let user_id = seed_org_and_user(&owner_pool, *knl.as_uuid(), "KNL").await;
    // A second tenant exists so we can prove cross-tenant invisibility.
    let _other = seed_org_and_user(&owner_pool, ORG_T2, "T2").await;

    let issue = BootstrapCredentialStore
        .issue_self_enroll_handoff(
            &rt_pool,
            user_id,
            knl,
            OffsetDateTime::now_utc(),
            HANDOFF_TTL,
        )
        .await
        .expect("self-handoff issuance must succeed as console_rt");

    // Owned by exactly the issuing user, stamped with the issuer's own org.
    let owner = handoff_owner_as_runtime(&rt_pool, knl, issue.token.as_str()).await;
    assert_eq!(
        owner,
        Some((user_id, *knl.as_uuid())),
        "handoff must be owned by the issuing user and stamped with their own org"
    );

    // Invisible under another tenant's GUC (cross-tenant isolation holds).
    let other = OrgId::from_uuid(ORG_T2);
    assert_eq!(
        handoff_owner_as_runtime(&rt_pool, other, issue.token.as_str()).await,
        None,
        "the handoff must be invisible under a different tenant's GUC"
    );
}

// ===========================================================================
// (2) HANDOFF → ENROLL → SINGLE-USE: a handoff redeems, the user enrolls a
// passkey (which consumes the code), and a SECOND redeem of the same code fails.
// ===========================================================================
#[sqlx::test(migrations = "../db/migrations")]
async fn handoff_redeems_then_enroll_consumes_it_single_use(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let user_id = seed_org_and_user(&owner_pool, *knl.as_uuid(), "KNL").await;

    let issue = BootstrapCredentialStore
        .issue_self_enroll_handoff(
            &rt_pool,
            user_id,
            knl,
            OffsetDateTime::now_utc(),
            HANDOFF_TTL,
        )
        .await
        .expect("self-handoff issuance must succeed as console_rt");
    let otp = issue.token.as_str().to_owned();

    // The phone redeems the handoff (first sign-in path) and gets a session.
    let redemption = BootstrapCredentialStore
        .redeem_otp(&rt_pool, &otp, OffsetDateTime::now_utc())
        .await
        .expect("handoff redeem must find the credential as console_rt");
    assert_eq!(redemption.user_id, user_id);
    assert_eq!(redemption.org_id, knl);
    assert!(
        redemption.requires_passkey_setup,
        "a zero-passkey user redeeming a handoff must be flagged for enrollment"
    );

    RefreshTokenStore
        .issue_family(
            &rt_pool,
            user_id,
            knl,
            OffsetDateTime::now_utc(),
            Duration::days(30),
        )
        .await
        .expect("session mint must pass RLS as console_rt");

    // The phone enrolls a passkey; enrollment consumes the open handoff code
    // atomically (production runs this in the register-finish transaction).
    let service = passkey_service();
    register_passkey_as_runtime(&service, &rt_pool, knl, user_id).await;
    consume_open_code_as_runtime(&rt_pool, knl, user_id).await;

    // SINGLE-USE: the same handoff code can never mint another session.
    let replay = BootstrapCredentialStore
        .redeem_otp(&rt_pool, &otp, OffsetDateTime::now_utc())
        .await;
    assert!(
        matches!(replay, Err(ProvisioningError::InvalidBootstrapCredential)),
        "a consumed handoff must be rejected on replay, got {replay:?}"
    );
}

// ===========================================================================
// (2b) CONSUME IS TENANT-VISIBLE: the `auth.otp.consume` audit row must carry
// the caller's org so a tenant-scoped `/api/audit` read sees it.
//
// Regression for the task #26 deferred site: the consume event was built without
// `.with_org`, landing the row with NULL org_id. The FORCE-RLS WITH CHECK permits
// NULL so the write succeeded, but RLS `USING (org_id = app.current_org)` then
// hid it from every tenant read. RED (pre-fix): the KNL-armed read returns 0.
// ===========================================================================
#[sqlx::test(migrations = "../db/migrations")]
async fn consume_audit_row_is_tenant_scoped_and_visible_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let user_id = seed_org_and_user(&owner_pool, *knl.as_uuid(), "KNL").await;

    // Issue an open code, then consume it (the single-use burn) as `console_rt`.
    BootstrapCredentialStore
        .issue_self_enroll_handoff(
            &rt_pool,
            user_id,
            knl,
            OffsetDateTime::now_utc(),
            HANDOFF_TTL,
        )
        .await
        .expect("self-handoff issuance must succeed as console_rt");
    consume_open_code_as_runtime(&rt_pool, knl, user_id).await;

    // As `console_rt`, armed to KNL: the consume event must be visible AND stamped
    // with KNL. A NULL-org row would be invisible here (RLS USING excludes it).
    let mut tx = rt_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(knl.as_uuid().to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let org_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT org_id FROM audit_events WHERE action = 'auth.otp.consume' AND actor = $1",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await
    .expect("KNL-armed read must see the consume audit row");
    tx.commit().await.unwrap();
    assert_eq!(
        org_id,
        Some(*knl.as_uuid()),
        "the consume audit row must carry the caller's org, not NULL"
    );
}

// ===========================================================================
// (3) EXPIRY: a handoff that has lapsed does not redeem.
// ===========================================================================
#[sqlx::test(migrations = "../db/migrations")]
async fn expired_handoff_does_not_redeem(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let user_id = seed_org_and_user(&owner_pool, *knl.as_uuid(), "KNL").await;

    // Issuance uses the post-lock database clock, so a stale request timestamp
    // cannot shorten or extend the handoff. Test expiry at its actual deadline.
    let issued_at = OffsetDateTime::now_utc();
    let issue = BootstrapCredentialStore
        .issue_self_enroll_handoff(&rt_pool, user_id, knl, issued_at, HANDOFF_TTL)
        .await
        .expect("self-handoff issuance must succeed as console_rt");

    let result = BootstrapCredentialStore
        .redeem_otp(
            &rt_pool,
            issue.token.as_str(),
            issue.expires_at + Duration::seconds(1),
        )
        .await;
    assert!(
        matches!(result, Err(ProvisioningError::InvalidBootstrapCredential)),
        "an expired handoff must not redeem, got {result:?}"
    );
}

// ===========================================================================
// (4) SUPERSEDE: minting a handoff while the user already holds an OPEN code (the
// one they redeemed but have not yet enrolled against) revokes the stale code and
// the FRESH one redeems while the OLD one no longer does. The one-open-per-user
// partial-unique index forbids two live codes; this proves the supersede path.
// ===========================================================================
#[sqlx::test(migrations = "../db/migrations")]
async fn handoff_supersedes_a_users_existing_open_code(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let user_id = seed_org_and_user(&owner_pool, *knl.as_uuid(), "KNL").await;

    // First handoff: the user's initial open code.
    let first = BootstrapCredentialStore
        .issue_self_enroll_handoff(
            &rt_pool,
            user_id,
            knl,
            OffsetDateTime::now_utc(),
            HANDOFF_TTL,
        )
        .await
        .expect("first handoff issuance must succeed");

    // Second handoff (e.g. the desktop re-requests a QR): must revoke the first
    // and mint a fresh one — without erroring on the one-open-per-user index.
    let second = BootstrapCredentialStore
        .issue_self_enroll_handoff(
            &rt_pool,
            user_id,
            knl,
            OffsetDateTime::now_utc(),
            HANDOFF_TTL,
        )
        .await
        .expect("a second handoff must supersede the first, not conflict");
    assert_ne!(
        first.token.as_str(),
        second.token.as_str(),
        "the superseding handoff must be a fresh code"
    );

    // The OLD code is now dead; the FRESH code redeems.
    let stale = BootstrapCredentialStore
        .redeem_otp(&rt_pool, first.token.as_str(), OffsetDateTime::now_utc())
        .await;
    assert!(
        matches!(stale, Err(ProvisioningError::InvalidBootstrapCredential)),
        "the superseded handoff must no longer redeem, got {stale:?}"
    );

    let fresh = BootstrapCredentialStore
        .redeem_otp(&rt_pool, second.token.as_str(), OffsetDateTime::now_utc())
        .await
        .expect("the fresh handoff must redeem as console_rt");
    assert_eq!(fresh.user_id, user_id);
    assert_eq!(fresh.org_id, knl);
}
