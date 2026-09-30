#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! End-to-end proof of the per-request tenant-context wiring (multi-tenant
//! phase 1) running as the genuine NON-OWNER runtime role `console_rt`.
//!
//! Unlike the other `app/tests/*` suites (which connect as the sqlx superuser and
//! therefore BYPASS RLS), this test builds a pool whose every connection drops to
//! `console_rt` — the production runtime role: NOSUPERUSER, NOBYPASSRLS, owns nothing.
//! So RLS is actually enforced here, exactly as in production.
//!
//! It proves the full request path:
//!   request → org middleware (`require_request_context`) resolves the Principal,
//!   reads the `org` claim, enters the `CURRENT_ORG` task-local scope →
//!   the domain handler's adapter read calls `with_org_conn(pool, current_org()?, ..)`
//!   which arms `app.current_org` → RLS narrows rows to that tenant.
//!
//! Assertions (definition of done):
//!   1. A request carrying a valid JWT (org = the seeded tenant) to a READ
//!      endpoint returns the tenant's seeded rows (NOT empty) under `console_rt`.
//!   2. A request with NO bearer token is rejected (fail-closed) — the handler
//!      never runs, so no tenant-scoped query executes without a bound org.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::Value;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

/// The seeded tenant. Reuses the KNL bootstrap id so it matches `OrgId::knl()`,
/// the single tenant the deployment ships with today.
fn tenant_org() -> OrgId {
    OrgId::knl()
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn list_users_round_trips_under_console_rt_with_org_from_jwt(super_pool: PgPool) {
    // --- keys + token (org = the seeded tenant) -------------------------------
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    let org = tenant_org();
    let admin_id = UserId::new();

    // --- seed the tenant as the OWNER (bypasses RLS for setup) ----------------
    seed_tenant(&super_pool, org, admin_id).await;

    // --- build the app pool that connects as the non-owner runtime role -------
    // Production sets DATABASE_URL to the `console_rt` user. Here the sqlx pool URL is
    // the superuser, so we drop every checked-out connection to `console_rt` via
    // `after_connect`. The org middleware + `with_org_conn` then arm
    // `app.current_org` per request transaction on top of that role, so RLS is
    // enforced exactly as in production.
    let runtime_pool = console_rt_pool(&super_pool).await;

    let token = issue_token(
        &runtime_pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        admin_id,
        org,
        vec!["SUPER_ADMIN".to_owned()],
    )
    .await;

    let service = build_router(app_state(runtime_pool.clone(), public_key_pem.clone()));

    // (1) Authenticated read returns the tenant's seeded user (NOT empty). -----
    let response = service
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/users")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "authenticated SUPER_ADMIN read should succeed under console_rt"
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let users = json
        .get("users")
        .or_else(|| json.get("items"))
        .and_then(Value::as_array)
        .or_else(|| json.as_array())
        .expect("response should contain a user array");
    assert!(
        !users.is_empty(),
        "RLS must return the seeded tenant's user rows (got empty) — the GUC was \
         not armed by the middleware. Body: {json}"
    );
    let seen = users.iter().any(|u| {
        u.get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id == admin_id.to_string())
    });
    assert!(seen, "the seeded SUPER_ADMIN user must be visible: {json}");
    // The paginated envelope's COUNT(*) total must also run under console_rt and be
    // RLS-scoped — at least the rows we can see, never fewer.
    let total = json
        .get("total")
        .and_then(Value::as_i64)
        .expect("UserPage must report a total");
    assert!(
        total >= users.len() as i64 && total >= 1,
        "total ({total}) must cover the visible rows ({}) under RLS",
        users.len()
    );

    // (2) No bearer token → fail closed (handler never runs). ------------------
    let response = service
        .oneshot(
            Request::builder()
                .uri("/api/v1/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "a request with no tenant context must be rejected before any query runs"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a pool that runs every connection as the non-owner `console_rt` role.
async fn console_rt_pool(super_pool: &PgPool) -> PgPool {
    // Preserve the SQLx per-test database, Unix socket and TLS options together.
    let options = super_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                // Drop to the least-privilege runtime role for the session so RLS
                // is enforced. `with_org_conn` arms the transaction-local GUC on
                // top of this.
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .expect("failed to build console_rt runtime pool")
}

/// Seed one organization plus a SUPER_ADMIN user as the OWNER (bypasses RLS).
async fn seed_tenant(pool: &PgPool, org: OrgId, admin_id: UserId) {
    let org_uuid = *org.as_uuid();
    // The KNL org may already be seeded by the migrations' backfill; upsert so the
    // test is idempotent regardless.
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, 'knl', 'KNL') \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(org_uuid)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO users (id, display_name, roles, is_active, org_id) \
         VALUES ($1, 'E2E Admin', $2, true, $3)",
    )
    .bind(*admin_id.as_uuid())
    .bind(vec!["SUPER_ADMIN".to_string()])
    .bind(org_uuid)
    .execute(pool)
    .await
    .unwrap();
}

fn app_state(pool: PgPool, public_key_pem: String) -> AppState {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key_pem),
    ])
    .unwrap();
    AppState::new(config, DatabaseDependency::Postgres(pool)).unwrap()
}

async fn issue_token(
    pool: &PgPool,
    private_pem: &[u8],
    public_pem: &[u8],
    user_id: UserId,
    org: OrgId,
    roles: Vec<String>,
) -> String {
    let settings = JwtSettings {
        issuer: TEST_ISSUER.to_owned(),
        audience: TEST_AUDIENCE.to_owned(),
        access_token_ttl: Duration::minutes(15),
    };
    let issuer = JwtIssuer::from_es256_pem(settings, private_pem, public_pem).unwrap();
    console_platform_test_support::issue_session_token(
        pool,
        &issuer,
        AccessTokenInput {
            subject: user_id,
            org_id: org,
            roles,
            branches: Vec::<BranchId>::new(),
            platform: false,
            view_as: false,
            read_only: false,
            display_name: None,
            feature_grants: Vec::new(),
            authz_subject_version: 0,
            authz_policy_version: 0,
            session_generation: 0,
            issued_at: OffsetDateTime::now_utc(),
        },
        None,
        vec![],
    )
    .await
}
