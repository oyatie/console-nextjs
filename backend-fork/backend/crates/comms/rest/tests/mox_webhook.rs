#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS + auth gate for the mox delivery webhook (slice 1).
//!
//! Exercised as the genuine non-owner runtime role `console_rt` (NOBYPASSRLS, FORCE
//! RLS) — the only faithful exercise of the cross-tenant account lookup (a
//! SECURITY DEFINER function GRANTed to console_rt) plus the org-armed inbound
//! UPSERT. The default `#[sqlx::test]` pool is a BYPASSRLS superuser and would
//! green-light a broken arming path.
//!
//! Deny-path negatives (deny-by-omission):
//!
//!   * webhook secret UNSET               -> 503 (feature off, never open)
//!   * MISSING / WRONG Authorization      -> 401 (constant-time reject)
//!   * VALID secret, UNKNOWN recipient    -> 200 ingested=false (nothing to do)
//!
//! Happy + idempotency:
//!
//!   * VALID secret, KNOWN recipient      -> 200 ingested=true; a REDELIVERY of
//!     the same payload -> 200 ingested=false (idempotent on mox MsgID +
//!     Message-ID), and the message is queryable in the tenant's read model.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use console_comms_adapter_postgres::PgMailStore;
use console_comms_application::{
    AccountUpsert, EmailAccountId, MailStore, account_config_audit_event,
};
use console_comms_credential_cipher::{
    Aad, CredentialCipher, EnvelopeCredentialCipher, SealedCredential,
};
use console_comms_domain::MailSecurity;
use console_comms_rest::{CommsRestState, MAIL_MOX_WEBHOOK_PATH, router};
use console_kernel_core::{AuditAction, AuditEvent, OrgId, TraceContext, UserId};
use console_platform_db::{DbError, with_audit};
use console_platform_request_context::CURRENT_ORG;
use console_platform_test_support::runtime_role_pool;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use tower::ServiceExt;

const SECRET: &str = "test-mox-webhook-secret";
const RECIPIENT: &str = "persona-b@localhost";

fn test_cipher() -> EnvelopeCredentialCipher {
    let b64 = base64::engine::general_purpose::STANDARD.encode([21u8; 32]);
    EnvelopeCredentialCipher::from_base64_key(&b64).unwrap()
}

fn seal(
    cipher: &EnvelopeCredentialCipher,
    org: OrgId,
    account: EmailAccountId,
    field: &str,
) -> SealedCredential {
    let org_s = org.to_string();
    let acc_s = account.to_string();
    cipher
        .encrypt(
            b"pw",
            Aad {
                org_id: &org_s,
                account_id: &acc_s,
                field,
            },
        )
        .unwrap()
}

/// Seed the org row (RLS FK target) as the owner, row-security off. The SQL
/// write is still wrapped in `with_audit` because this `rest/` integration test
/// file is scanned by the backend audit-coverage gate.
async fn seed_org(owner_pool: &PgPool, org: OrgId) {
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_org").unwrap(),
        "organization",
        org.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    );
    with_audit::<_, (), DbError>(owner_pool, event, |tx| {
        Box::pin(async move {
            sqlx::query("SET LOCAL row_security = off")
                .execute(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;
            sqlx::query(
                "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
            )
            .bind(*org.as_uuid())
            .bind("org-knl")
            .bind("Org KNL")
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok(())
        })
    })
    .await
    .unwrap();
}

/// Seed an active user (the audit `actor_user_id` FK target) as the owner.
async fn seed_active_user(owner_pool: &PgPool, org: OrgId) -> UserId {
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_active_user").unwrap(),
        "user",
        org.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org);
    with_audit::<_, UserId, DbError>(owner_pool, event, |tx| {
        Box::pin(async move {
            sqlx::query("SET LOCAL row_security = off")
                .execute(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;
            let user_id: uuid::Uuid = sqlx::query_scalar(
                "INSERT INTO users (display_name, roles, org_id, is_active) VALUES ($1, $2, $3, true) RETURNING id",
            )
            .bind(format!("Persona B {}", uuid::Uuid::new_v4()))
            .bind(vec!["ADMIN".to_owned()])
            .bind(*org.as_uuid())
            .fetch_one(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok(UserId::from_uuid(user_id))
        })
    })
    .await
    .unwrap()
}

/// Seed an ACTIVE mailbox for KNL whose address is `RECIPIENT`.
async fn seed_account(
    store: &PgMailStore,
    cipher: &EnvelopeCredentialCipher,
    org: OrgId,
    actor: UserId,
) {
    let account = EmailAccountId::new();
    let upsert = AccountUpsert {
        id: account,
        actor,
        display_name: "Persona B".to_owned(),
        email_address: RECIPIENT.to_owned(),
        from_name: Some("Persona B".to_owned()),
        imap_host: "imap.example.test".to_owned(),
        imap_port: 993,
        imap_security: MailSecurity::SslTls,
        imap_username: "b".to_owned(),
        smtp_host: "smtp.example.test".to_owned(),
        smtp_port: 587,
        smtp_security: MailSecurity::StartTls,
        smtp_username: "b".to_owned(),
        smtp_password: Some(seal(cipher, org, account, "smtp_password")),
        imap_password: Some(seal(cipher, org, account, "imap_password")),
    };
    let audit = account_config_audit_event(
        actor,
        account,
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .with_org(org);
    CURRENT_ORG
        .scope(org, store.upsert_account(upsert, audit))
        .await
        .unwrap_or_else(|e| panic!("seed mailbox: {e:?}"));
}

fn incoming_body(msg_id: i64, rcpt: &str, subject: &str) -> Value {
    json!({
        "Version": 0,
        "From": [{ "Name": "Persona A", "Address": "persona-a@localhost" }],
        "To": [{ "Address": rcpt }],
        "Subject": subject,
        "MessageID": format!("<msg-{msg_id}@localhost>"),
        "References": [],
        "Text": "hello from persona A",
        "Meta": { "MsgID": msg_id, "RcptTo": rcpt, "MailboxName": "Inbox" }
    })
}

struct Resp {
    status: StatusCode,
    json: Value,
}

async fn post_webhook(service: axum::Router, auth: Option<&str>, body: Value) -> Resp {
    let mut builder = Request::builder()
        .method("POST")
        .uri(MAIL_MOX_WEBHOOK_PATH)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(auth) = auth {
        builder = builder.header(header::AUTHORIZATION, auth);
    }
    let response = service
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    Resp { status, json }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn secret_unset_returns_503(pool: PgPool) {
    let rt = runtime_role_pool(&pool).await;
    // No `.with_mox_webhook_secret(..)` → the webhook is disabled.
    let service = router(CommsRestState::new(PgMailStore::new(rt), None, None));
    let resp = post_webhook(
        service,
        Some(&format!("Bearer {SECRET}")),
        incoming_body(1, RECIPIENT, "hi"),
    )
    .await;
    assert_eq!(
        resp.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{:?}",
        resp.json
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn missing_or_wrong_secret_returns_401(pool: PgPool) {
    let rt = runtime_role_pool(&pool).await;
    let state = CommsRestState::new(PgMailStore::new(rt), None, None)
        .with_mox_webhook_secret(Some(SECRET.to_owned()));

    let missing = post_webhook(
        router(state.clone()),
        None,
        incoming_body(1, RECIPIENT, "hi"),
    )
    .await;
    assert_eq!(
        missing.status,
        StatusCode::UNAUTHORIZED,
        "{:?}",
        missing.json
    );

    let wrong = post_webhook(
        router(state),
        Some("Bearer not-the-secret"),
        incoming_body(1, RECIPIENT, "hi"),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED, "{:?}", wrong.json);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn unknown_recipient_is_acked_not_ingested(pool: PgPool) {
    let rt = runtime_role_pool(&pool).await;
    let state = CommsRestState::new(PgMailStore::new(rt), None, None)
        .with_mox_webhook_secret(Some(SECRET.to_owned()));
    let resp = post_webhook(
        router(state),
        Some(&format!("Bearer {SECRET}")),
        incoming_body(1, "nobody@localhost", "hi"),
    )
    .await;
    assert_eq!(resp.status, StatusCode::OK, "{:?}", resp.json);
    assert_eq!(resp.json["ingested"], json!(false));
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn known_recipient_ingests_and_is_idempotent(pool: PgPool) {
    let cipher = test_cipher();
    seed_org(&pool, OrgId::knl()).await;
    let actor = seed_active_user(&pool, OrgId::knl()).await;
    // Seed + serve + ingest all as the genuine runtime role (console_rt).
    let rt = runtime_role_pool(&pool).await;
    seed_account(&PgMailStore::new(rt.clone()), &cipher, OrgId::knl(), actor).await;
    let state = CommsRestState::new(PgMailStore::new(rt), None, None)
        .with_mox_webhook_secret(Some(SECRET.to_owned()));

    let first = post_webhook(
        router(state.clone()),
        Some(&format!("Bearer {SECRET}")),
        incoming_body(42, RECIPIENT, "Quarterly report"),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.json);
    assert_eq!(first.json["ingested"], json!(true), "new message ingested");

    // Redelivery of the SAME payload (same MsgID + Message-ID) is a no-op.
    let again = post_webhook(
        router(state),
        Some(&format!("Bearer {SECRET}")),
        incoming_body(42, RECIPIENT, "Quarterly report"),
    )
    .await;
    assert_eq!(again.status, StatusCode::OK, "{:?}", again.json);
    assert_eq!(
        again.json["ingested"],
        json!(false),
        "redelivery must not duplicate"
    );

    // The message is now queryable in the tenant's read model: exactly one
    // inbound message under KNL with the delivered subject.
    let count: i64 = CURRENT_ORG
        .scope(OrgId::knl(), async {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM email_messages WHERE direction = 'IN' AND subject = $1",
            )
            .bind("Quarterly report")
            .fetch_one(&pool)
            .await
        })
        .await
        .unwrap();
    assert_eq!(count, 1, "exactly one inbound row persisted");
}

/// `email_accounts` is unique only per `(org_id, email_address)` — the SAME
/// address can exist under TWO different orgs. That must never route
/// nondeterministically to a plan-order-picked winner: delivery is refused to
/// BOTH orgs, an anomaly audit row lands (platform-tier, `org_id IS NULL`),
/// and mox still gets a 200 (quarantined-but-acked, so it does not retry
/// forever).
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn dual_org_match_is_quarantined_and_audited(pool: PgPool) {
    let cipher = test_cipher();
    let org_a = OrgId::knl();
    let org_b = OrgId::new();
    seed_org(&pool, org_a).await;
    seed_org(&pool, org_b).await;
    let actor_a = seed_active_user(&pool, org_a).await;
    let actor_b = seed_active_user(&pool, org_b).await;
    // Seed + serve as the genuine runtime role (console_rt) — same as the
    // single-match happy path above.
    let rt = runtime_role_pool(&pool).await;
    seed_account(&PgMailStore::new(rt.clone()), &cipher, org_a, actor_a).await;
    seed_account(&PgMailStore::new(rt.clone()), &cipher, org_b, actor_b).await;
    let state = CommsRestState::new(PgMailStore::new(rt), None, None)
        .with_mox_webhook_secret(Some(SECRET.to_owned()));

    let resp = post_webhook(
        router(state),
        Some(&format!("Bearer {SECRET}")),
        incoming_body(99, RECIPIENT, "should not land anywhere"),
    )
    .await;
    assert_eq!(resp.status, StatusCode::OK, "{:?}", resp.json);
    assert_eq!(
        resp.json["ingested"],
        json!(false),
        "an ambiguous address must not be ingested"
    );

    // Neither org's read model received the message.
    for org in [org_a, org_b] {
        let count: i64 = CURRENT_ORG
            .scope(org, async {
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM email_messages WHERE direction = 'IN' AND subject = $1",
                )
                .bind("should not land anywhere")
                .fetch_one(&pool)
                .await
            })
            .await
            .unwrap();
        assert_eq!(count, 0, "ambiguous address must deliver to neither org");
    }

    // The anomaly is audited exactly once, as a platform-tier row (no single
    // tenant owns a cross-tenant ambiguity).
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events \
         WHERE action = 'email.webhook.address_ambiguous' \
           AND org_id IS NULL \
           AND target_id = $1",
    )
    .bind(RECIPIENT)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 1, "the ambiguity must be audited exactly once");
}
