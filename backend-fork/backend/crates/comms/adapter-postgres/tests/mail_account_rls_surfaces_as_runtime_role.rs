#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the webmail store (PgMailStore).
//!
//! Mirrors `region_branch_crud_rls_surfaces_as_runtime_role.rs`: we SEED as the
//! owner (raw inserts, row_security off) and MUTATE/READ as the genuine non-owner
//! runtime role `console_rt` (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) — the only
//! faithful exercise of the `org_isolation` policy. The default `#[sqlx::test]`
//! pool is a BYPASSRLS superuser and would green-light a broken/leaking path.
//!
//! Asserts, with two tenants A (KNL) and B:
//!   * upsert_account stores a mailbox under A's armed GUC and writes an
//!     `email.account.configure` audit row; only the CIPHERTEXT is persisted
//!     (no plaintext password column exists or holds the secret);
//!   * get_account under A returns the mailbox, and its write-only view carries
//!     NO password field (only `has_*_password` booleans);
//!   * cross-tenant isolation: under B's armed GUC, A's mailbox is INVISIBLE
//!     (get_account returns None) as `console_rt`;
//!   * FAIL-CLOSED: with no GUC armed, get_account returns None / errors, never
//!     A's row;
//!   * prepared outbound operations are immutable, idempotent, org-scoped and
//!     never surfaced as SENT before two-site confirmation and egress.

use console_comms_adapter_postgres::PgMailStore;
use console_comms_application::{
    AccountUpsert, EmailAccountId, MailServiceError, MailStore, OutboundPreparation,
    PreparedFormat, PreparedOutbound, account_config_audit_event, send_preparation_audit_event,
};
use console_comms_credential_cipher::{
    Aad, CredentialCipher, EnvelopeCredentialCipher, SealedCredential,
};
use console_comms_domain::MailSecurity;
use console_kernel_core::{OrgId, TraceContext, UserId};
use console_platform_request_context::CURRENT_ORG;
use sha2::Digest;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use uuid::Uuid;

const ORG_B: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);

/// A deterministic 32-byte test KEK (base64), distinct from any production key.
fn test_cipher() -> EnvelopeCredentialCipher {
    use base64::Engine as _;
    let key = [13u8; 32];
    let b64 = base64::engine::general_purpose::STANDARD.encode(key);
    EnvelopeCredentialCipher::from_base64_key(&b64).unwrap()
}

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

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

async fn seed_active_user(owner_pool: &PgPool, org: Uuid) -> UserId {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id, is_active) VALUES ($1, $2, $3, true) RETURNING id",
    )
    .bind(format!("User {}", Uuid::new_v4()))
    .bind(vec!["ADMIN".to_string()])
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    UserId::from_uuid(user_id)
}

async fn audit_count(owner_pool: &PgPool, action: &str, target_id: &str) -> i64 {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = $1 AND target_id = $2",
    )
    .bind(action)
    .bind(target_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    count
}

/// Read the raw ciphertext bytes of the stored SMTP secret as OWNER, so we can
/// assert the DB never holds the plaintext.
async fn raw_smtp_ct(owner_pool: &PgPool, account: Uuid) -> Vec<u8> {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let ct: Vec<u8> =
        sqlx::query_scalar("SELECT smtp_password_ct FROM email_accounts WHERE id = $1")
            .bind(account)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    ct
}

fn seal(
    cipher: &EnvelopeCredentialCipher,
    org: OrgId,
    account: EmailAccountId,
    field: &str,
    pw: &[u8],
) -> SealedCredential {
    let org_s = org.to_string();
    let acc_s = account.to_string();
    cipher
        .encrypt(
            pw,
            Aad {
                org_id: &org_s,
                account_id: &acc_s,
                field,
            },
        )
        .unwrap()
}

fn upsert_for(
    org: OrgId,
    account: EmailAccountId,
    actor: UserId,
    cipher: &EnvelopeCredentialCipher,
) -> AccountUpsert {
    AccountUpsert {
        id: account,
        actor,
        display_name: "KNL Mail".to_owned(),
        email_address: format!("ops-{}@knl.example", account),
        from_name: Some("KNL Ops".to_owned()),
        imap_host: "imap.knl.example".to_owned(),
        imap_port: 993,
        imap_security: MailSecurity::SslTls,
        imap_username: "ops".to_owned(),
        smtp_host: "smtp.knl.example".to_owned(),
        smtp_port: 587,
        smtp_security: MailSecurity::StartTls,
        smtp_username: "ops".to_owned(),
        smtp_password: Some(seal(cipher, org, account, "smtp_password", b"smtp-secret")),
        imap_password: Some(seal(cipher, org, account, "imap_password", b"imap-secret")),
    }
}

// ===========================================================================
// upsert + get under the armed GUC; ciphertext-only; write-only view.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn upsert_and_get_account_as_runtime_role_persist_ciphertext_only(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid, "A").await;
    let actor = seed_active_user(&owner_pool, org_uuid).await;
    let cipher = test_cipher();

    let account_id = EmailAccountId::new();
    let upsert = upsert_for(org, account_id, actor, &cipher);
    let audit = account_config_audit_event(
        actor,
        account_id,
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .with_org(org);

    let store = PgMailStore::new(rt_pool.clone());
    let stored = CURRENT_ORG
        .scope(org, store.upsert_account(upsert, audit))
        .await
        .expect("upsert_account must succeed as console_rt under the armed GUC");
    assert_eq!(stored.id, account_id);

    // Audited.
    assert_eq!(
        audit_count(
            &owner_pool,
            "email.account.configure",
            &account_id.to_string()
        )
        .await,
        1
    );

    // The DB holds CIPHERTEXT, never the plaintext.
    let ct = raw_smtp_ct(&owner_pool, *account_id.as_uuid()).await;
    assert!(!ct.is_empty());
    assert_ne!(
        ct,
        b"smtp-secret".to_vec(),
        "the plaintext must never be stored"
    );

    // get_account returns the mailbox; the write-only VIEW has no password field.
    let view = CURRENT_ORG
        .scope(org, store.get_account())
        .await
        .expect("get_account as console_rt")
        .expect("the configured mailbox is visible under its own org");
    let json = serde_json::to_string(&view.to_view()).unwrap();
    assert!(!json.contains("smtp-secret"));
    assert!(!json.to_lowercase().contains("password_ct"));
    assert!(json.contains("has_smtp_password"));

    // The sealed secret round-trips back to the plaintext via the cipher (the
    // store persisted faithful ciphertext, not garbage).
    let org_s = org.to_string();
    let acc_s = account_id.to_string();
    let recovered = cipher
        .decrypt(
            &view.smtp_password,
            Aad {
                org_id: &org_s,
                account_id: &acc_s,
                field: "smtp_password",
            },
        )
        .unwrap();
    use secrecy::ExposeSecret;
    assert_eq!(recovered.expose_secret().as_slice(), b"smtp-secret");
}

// ===========================================================================
// Cross-tenant isolation: B's GUC cannot see A's mailbox as console_rt.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn account_is_invisible_to_another_org_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);
    seed_org(&owner_pool, *org_a.as_uuid(), "A").await;
    seed_org(&owner_pool, *org_b.as_uuid(), "B").await;
    let actor_a = seed_active_user(&owner_pool, *org_a.as_uuid()).await;
    let cipher = test_cipher();

    let account_id = EmailAccountId::new();
    let upsert = upsert_for(org_a, account_id, actor_a, &cipher);
    let audit = account_config_audit_event(
        actor_a,
        account_id,
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .with_org(org_a);

    let store = PgMailStore::new(rt_pool.clone());
    CURRENT_ORG
        .scope(org_a, store.upsert_account(upsert, audit))
        .await
        .expect("seed A's mailbox");

    // Under B's GUC, A's mailbox is INVISIBLE.
    let seen_by_b = CURRENT_ORG
        .scope(org_b, store.get_account())
        .await
        .expect("get_account as console_rt under B");
    assert!(seen_by_b.is_none(), "B must never see A's mailbox");

    // FAIL-CLOSED: with no GUC armed at all, the read returns nothing/errors.
    let unarmed = store.get_account().await;
    match unarmed {
        Ok(None) => {}
        Err(_) => {}
        Ok(Some(_)) => panic!("an unarmed read must NEVER surface a tenant's mailbox"),
    }
}

// ===========================================================================
// Prepared outbound admission is immutable and never creates a SENT message.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn prepared_outbound_is_idempotent_and_runtime_cannot_read_or_mutate_mime(
    owner_pool: PgPool,
) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid, "A").await;
    let actor = seed_active_user(&owner_pool, org_uuid).await;
    let cipher = test_cipher();
    let account_id = EmailAccountId::new();
    let store = PgMailStore::new(rt_pool.clone());
    let upsert = upsert_for(org, account_id, actor, &cipher);
    let account_audit = account_config_audit_event(
        actor,
        account_id,
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .with_org(org);
    CURRENT_ORG
        .scope(org, store.upsert_account(upsert, account_audit))
        .await
        .expect("seed mailbox");

    let operation_id = Uuid::new_v4();
    let preparation = |hash: [u8; 32], mime: &[u8], message_id: &str| OutboundPreparation {
        operation_id,
        account_id,
        actor,
        request_codec_version: 1,
        request_hash: hash,
        payload_hash: sha2::Sha256::digest(mime).into(),
        from_address: "ops@knl.example".to_owned(),
        envelope_recipients: vec![
            "customer@example.com".to_owned(),
            "private@example.com".to_owned(),
        ],
        payload: PreparedOutbound {
            format: PreparedFormat::SmtpMime,
            bytes: mime.to_vec(),
            rfc_message_id: Some(message_id.to_owned()),
        },
    };
    let audit = || {
        send_preparation_audit_event(
            actor,
            operation_id,
            TraceContext::generate(),
            OffsetDateTime::now_utc(),
        )
        .unwrap()
        .with_org(org)
    };
    let original_mime = b"From: ops@knl.example\r\nTo: customer@example.com\r\n\r\nbody";
    CURRENT_ORG
        .scope(
            org,
            store.prepare_outbound(
                preparation([1; 32], original_mime, "<first@knl.example>"),
                audit(),
            ),
        )
        .await
        .expect("first preparation");

    // A runtime writer must not commit a PREPARED row without its matching
    // audit event, even when it bypasses the adapter's with_audits path.
    let direct_operation = Uuid::new_v4();
    let mut direct_tx = rt_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org_uuid.to_string())
        .execute(&mut *direct_tx)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO email_outbound_operations (
            org_id, operation_id, account_id, actor_user_id,
            request_codec_version, request_hash, payload_hash,
            payload_format, payload_bytes, envelope_from,
            envelope_recipients, rfc_message_id
        ) VALUES ($1, $2, $3, $4, 1, $5, $6,
                  'SMTP_MIME', $7, 'ops@knl.example',
                  ARRAY['customer@example.com'], '<direct@knl.example>')"#,
    )
    .bind(org_uuid)
    .bind(direct_operation)
    .bind(*account_id.as_uuid())
    .bind(*actor.as_uuid())
    .bind(vec![1_u8; 32])
    .bind(vec![2_u8; 32])
    .bind(b"body".to_vec())
    .execute(&mut *direct_tx)
    .await
    .unwrap();
    let direct_error = direct_tx
        .commit()
        .await
        .expect_err("unaudited direct preparation must roll back");
    assert_eq!(
        direct_error
            .as_database_error()
            .and_then(|db| db.code())
            .as_deref(),
        Some("23514")
    );

    // Retry with freshly generated MIME/Message-ID: the original exact bytes
    // and identity must win when semantic request and codec are unchanged.
    CURRENT_ORG
        .scope(
            org,
            store.prepare_outbound(
                preparation([1; 32], b"different bytes", "<retry@knl.example>"),
                audit(),
            ),
        )
        .await
        .expect("identical request retry");
    let conflict = CURRENT_ORG
        .scope(
            org,
            store.prepare_outbound(
                preparation([9; 32], b"changed", "<other@knl.example>"),
                audit(),
            ),
        )
        .await;
    assert!(matches!(conflict, Err(MailServiceError::OperationConflict)));

    let mut mismatched_actor = preparation([1; 32], original_mime, "<first@knl.example>");
    mismatched_actor.actor = UserId::new();
    let mismatch = CURRENT_ORG
        .scope(org, store.prepare_outbound(mismatched_actor, audit()))
        .await;
    assert!(matches!(mismatch, Err(MailServiceError::Domain(_))));

    seed_org(&owner_pool, ORG_B, "B").await;
    let foreign_actor = seed_active_user(&owner_pool, ORG_B).await;
    let foreign_operation = Uuid::new_v4();
    let mut foreign = preparation([3; 32], original_mime, "<foreign@knl.example>");
    foreign.operation_id = foreign_operation;
    foreign.actor = foreign_actor;
    let foreign_audit = send_preparation_audit_event(
        foreign_actor,
        foreign_operation,
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .with_org(org);
    let foreign_result = CURRENT_ORG
        .scope(org, store.prepare_outbound(foreign, foreign_audit))
        .await;
    assert!(matches!(foreign_result, Err(MailServiceError::Store)));

    let mut owner_tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *owner_tx)
        .await
        .unwrap();
    let row = sqlx::query(
        "SELECT state, payload_bytes, envelope_recipients, rfc_message_id FROM email_outbound_operations WHERE org_id = $1 AND operation_id = $2",
    )
    .bind(org_uuid)
    .bind(operation_id)
    .fetch_one(&mut *owner_tx)
    .await
    .unwrap();
    let state: String = sqlx::Row::try_get(&row, "state").unwrap();
    let bytes: Vec<u8> = sqlx::Row::try_get(&row, "payload_bytes").unwrap();
    let recipients: Vec<String> = sqlx::Row::try_get(&row, "envelope_recipients").unwrap();
    let rfc_message_id: String = sqlx::Row::try_get(&row, "rfc_message_id").unwrap();
    assert_eq!(state, "PREPARED");
    assert_eq!(bytes, original_mime);
    assert_eq!(rfc_message_id, "<first@knl.example>");
    assert_eq!(recipients[1], "private@example.com");
    let sent_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM email_messages WHERE org_id = $1 AND direction = 'OUT'",
    )
    .bind(org_uuid)
    .fetch_one(&mut *owner_tx)
    .await
    .unwrap();
    assert_eq!(sent_count, 0);
    owner_tx.commit().await.unwrap();
    assert_eq!(
        audit_count(&owner_pool, "email.send.prepare", &operation_id.to_string()).await,
        1
    );

    for statement in [
        "SELECT payload_bytes FROM email_outbound_operations WHERE operation_id = $1",
        "SELECT envelope_recipients FROM email_outbound_operations WHERE operation_id = $1",
        "UPDATE email_outbound_operations SET state = 'PREPARED' WHERE operation_id = $1",
        "DELETE FROM email_outbound_operations WHERE operation_id = $1",
    ] {
        let mut tx = rt_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org_uuid.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        let error = sqlx::query(statement)
            .bind(operation_id)
            .execute(&mut *tx)
            .await
            .unwrap_err();
        assert_eq!(
            error
                .as_database_error()
                .and_then(|db| db.code())
                .as_deref(),
            Some("42501")
        );
        tx.rollback().await.unwrap();
    }
}

// ===========================================================================
// L3: the smtp_port DB CHECK rejects 25 (legacy MTA relay) and accepts 587/465.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn smtp_port_check_rejects_25_accepts_submission_ports(owner_pool: PgPool) {
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid, "A").await;
    let actor = seed_active_user(&owner_pool, org_uuid).await;

    // A minimal raw insert helper (owner, row_security off) parameterized on the
    // smtp_port, so we exercise the CHECK constraint directly — independent of the
    // application-layer ALLOWED_SMTP_PORTS validation.
    async fn try_insert_with_port(
        owner_pool: &PgPool,
        org: Uuid,
        actor: Uuid,
        smtp_port: i32,
    ) -> Result<(), sqlx::Error> {
        let mut tx = owner_pool.begin().await.unwrap();
        sqlx::query("SET LOCAL row_security = off")
            .execute(&mut *tx)
            .await
            .unwrap();
        let dummy: &[u8] = b"ct";
        let result = sqlx::query(
            r#"
            INSERT INTO email_accounts (
                org_id, display_name, email_address,
                imap_host, imap_port, imap_security, imap_username,
                smtp_host, smtp_port, smtp_security, smtp_username,
                smtp_password_ct, smtp_password_nonce, dek_wrapped, dek_nonce,
                imap_password_ct, imap_password_nonce, imap_dek_wrapped, imap_dek_nonce,
                key_version, created_by
            ) VALUES (
                $1, 'KNL', $2,
                'imap.knl.example', 993, 'TLS', 'ops',
                'smtp.knl.example', $3, 'STARTTLS', 'ops',
                $4, $4, $4, $4,
                $4, $4, $4, $4,
                1, $5
            )
            "#,
        )
        .bind(org)
        .bind(format!("ops-{}@knl.example", smtp_port))
        .bind(smtp_port)
        .bind(dummy)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map(|_| ());
        // Roll back regardless so each probe is independent.
        let _ = tx.rollback().await;
        result
    }

    // Port 25 is now rejected by the CHECK.
    let port_25 = try_insert_with_port(&owner_pool, org_uuid, *actor.as_uuid(), 25).await;
    assert!(
        port_25.is_err(),
        "the smtp_port CHECK must reject 25 (the unauthenticated MTA relay port)"
    );
    if let Err(sqlx::Error::Database(db)) = &port_25 {
        // 23514 = check_violation.
        assert_eq!(db.code().as_deref(), Some("23514"));
    } else {
        panic!("expected a check-constraint violation, got {port_25:?}");
    }

    // The authenticated submission ports still pass.
    assert!(
        try_insert_with_port(&owner_pool, org_uuid, *actor.as_uuid(), 587)
            .await
            .is_ok(),
        "587 (STARTTLS submission) must still be accepted"
    );
    assert!(
        try_insert_with_port(&owner_pool, org_uuid, *actor.as_uuid(), 465)
            .await
            .is_ok(),
        "465 (implicit TLS submission) must still be accepted"
    );
}
