#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! §16 ingest-commit gate (85 판정, BE-ingest-checklist-gates) —
//! `POST /api/v1/hr/attendance-import/{run_id}/apply` ("적재"/commit) over the
//! REAL router on a genuine non-owner `console_rt` pool (RLS enforced), JWT-authed.
//!
//! Proves:
//!   * apply WITHOUT `checklist_all_acknowledged` (or with `false`) denies
//!     (403) BEFORE the run row is even read — fail-closed, zero rows written,
//!     the run stays `DRY_RUN`;
//!   * apply WITH `checklist_all_acknowledged: true` passes the gate and the
//!     real mutation runs (run flips to `APPLIED`), and the audit trail carries
//!     the gate outcome.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const SOURCE_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

// ===========================================================================
// Deny path: no checklist evidence ⇒ 403, before the run row is even touched.
// Nothing is written — the run (seeded DRY_RUN) is untouched, zero events.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn apply_without_checklist_denies_and_writes_nothing(owner_pool: PgPool) {
    let keys = keys();
    let admin = UserId::new();
    seed_admin(&owner_pool, admin).await;
    let run_id = seed_dry_run(&owner_pool, "checklist-deny").await;

    let service = build_router(
        app_state(
            runtime_role_pool(&owner_pool).await,
            keys.public_pem.clone(),
        )
        .unwrap(),
    );
    let token = bearer(&keys, admin);

    // No body at all (no Content-Type ⇒ `checklist_all_acknowledged: None`).
    let denied = send(
        service.clone(),
        &format!("/api/v1/hr/attendance-import/{run_id}/apply"),
        &token,
        None,
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{:?}", denied.json);
    assert!(
        denied.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("checklist"),
        "must deny for the checklist gate specifically, not some other 403: {:?}",
        denied.json
    );

    // Explicit `false` must deny identically (not just "missing").
    let denied_explicit = send(
        service.clone(),
        &format!("/api/v1/hr/attendance-import/{run_id}/apply"),
        &token,
        Some(json!({ "checklist_all_acknowledged": false })),
    )
    .await;
    assert_eq!(
        denied_explicit.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        denied_explicit.json
    );

    let status: String = sqlx::query_scalar("SELECT status FROM data_import_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert_eq!(status, "DRY_RUN", "a denied gate must not flip the run");
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM attendance_direct_import_events WHERE run_id = $1",
    )
    .bind(run_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(events, 0, "a denied gate must write zero rows");
}

// ===========================================================================
// Allow path: checklist acknowledged ⇒ the real mutation runs (run flips to
// APPLIED) and the audit event carries the gate outcome.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn apply_with_checklist_admits_and_applies(owner_pool: PgPool) {
    let keys = keys();
    let admin = UserId::new();
    seed_admin(&owner_pool, admin).await;
    let run_id = seed_dry_run(&owner_pool, "checklist-allow").await;

    let service = build_router(
        app_state(
            runtime_role_pool(&owner_pool).await,
            keys.public_pem.clone(),
        )
        .unwrap(),
    );
    let token = bearer(&keys, admin);

    let applied = send(
        service,
        &format!("/api/v1/hr/attendance-import/{run_id}/apply"),
        &token,
        Some(json!({ "checklist_all_acknowledged": true })),
    )
    .await;
    assert_eq!(applied.status, StatusCode::OK, "{:?}", applied.json);

    let status: String = sqlx::query_scalar("SELECT status FROM data_import_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert_eq!(status, "APPLIED", "an admitted gate must apply the run");

    let gate_outcome: Value = sqlx::query_scalar(
        "SELECT after_snap -> 'gate_outcome' FROM audit_events \
         WHERE target_type = 'data_import_run' AND target_id = $1 \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(run_id.to_string())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        gate_outcome["allow"],
        Value::Bool(true),
        "the audit row must carry the passing gate outcome: {gate_outcome:?}"
    );
}

// ===========================================================================
// Helpers.
// ===========================================================================

/// SUPER_ADMIN (not ADMIN): `authorize_hr_org_wide` requires `BranchScope::All`,
/// which only SUPER_ADMIN/EXECUTIVE resolve to without a `user_branches` row
/// (`resolve_branch_scope_in_org`); ADMIN would 403 upstream of the §16 gate
/// this test targets.
async fn seed_admin(pool: &PgPool, user_id: UserId) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("ingest-gate-admin-{}", user_id.as_uuid()))
        .bind(vec!["SUPER_ADMIN"])
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

/// Seed a `data_import_runs` row already at `DRY_RUN` with zero candidate rows,
/// so `apply` (once past the checklist gate) resolves an empty, error-free
/// summary and applies cleanly — exercising the real mutation path without
/// reconstructing the upload/preview/dry-run pipeline.
async fn seed_dry_run(pool: &PgPool, tag: &str) -> Uuid {
    let run_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO data_import_runs (
            id, org_id, entity_type, status, source_filename, source_format, source_sha256, pay_period_start, pay_period_end)
         VALUES ($1, $2, 'attendance_direct', 'DRY_RUN', $3, 'csv', $4, DATE '2026-06-01', DATE '2026-06-30')
        "#,
    )
    .bind(run_id)
    .bind(*OrgId::knl().as_uuid())
    .bind(format!("{tag}.csv"))
    .bind(SOURCE_SHA256)
    .execute(pool)
    .await
    .unwrap();
    run_id
}

async fn send(service: axum::Router, uri: &str, token: &str, body: Option<Value>) -> JsonResponse {
    let mut builder = Request::builder()
        .uri(uri)
        .method("POST")
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let request = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        builder.body(Body::from(body.to_string())).unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
}

fn keys() -> Keys {
    let signing_key = SigningKey::random(&mut OsRng);
    Keys {
        private_pem: signing_key
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap()
            .to_string(),
        public_pem: signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap(),
    }
}

fn bearer(keys: &Keys, user_id: UserId) -> String {
    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
    )
    .unwrap();
    issuer
        .issue_access_token(AccessTokenInput {
            subject: user_id,
            org_id: OrgId::knl(),
            roles: vec!["SUPER_ADMIN".to_owned()],
            branches: Vec::new(),
            platform: false,
            view_as: false,
            read_only: false,
            display_name: None,
            feature_grants: Vec::new(),
            authz_subject_version: 0,
            authz_policy_version: 0,
            session_generation: 0,
            issued_at: OffsetDateTime::now_utc(),
        })
        .unwrap()
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

fn app_state(pool: PgPool, public_key_pem: String) -> Result<AppState, console_app::AppError> {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key_pem),
    ])?;
    AppState::new(config, DatabaseDependency::Postgres(pool))
}
