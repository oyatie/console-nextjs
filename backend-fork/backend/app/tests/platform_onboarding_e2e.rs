#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! End-to-end proof of the PLATFORM tier + tenant onboarding, running as the
//! genuine NON-OWNER runtime role `console_rt` (so RLS is actually enforced, exactly
//! as in production).
//!
//! Assertions (definition of done):
//!   1. A PLATFORM token can POST /api/platform/orgs to onboard tenant "acme",
//!      creating the org + its first SUPER_ADMIN + a one-time OTP.
//!   2. TENANT ISOLATION: a JWT scoped to acme reading /api/v1/users sees ONLY
//!      acme's users (never KNL's), and a KNL-scoped JWT sees only KNL's — under
//!      `console_rt`, so RLS is the gate.
//!   3. A TENANT token is REJECTED on /api/platform/* (403).
//!   4. A PLATFORM token is REJECTED on a tenant /api/v1/* route (403).

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

struct Keys {
    private_pem: String,
    public_pem: String,
}

fn gen_keys() -> Keys {
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

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn platform_onboards_tenant_and_rls_isolates(super_pool: PgPool) {
    let keys = gen_keys();

    // Seed KNL (tenant #1) + a KNL admin as the OWNER (bypasses RLS for setup).
    let knl = OrgId::knl();
    let knl_admin = UserId::new();
    seed_tenant(&super_pool, knl, "knl", knl_admin).await;

    // The app pool connects as the non-owner runtime role, so RLS is enforced.
    let runtime_pool = console_rt_pool(&super_pool).await;
    let service = build_router(app_state(runtime_pool.clone(), keys.public_pem.clone()));

    // --- (1) PLATFORM token onboards a NEW tenant "acme" -----------------------
    // Seed the platform admin user (homed in the sentinel org) so the onboarding
    // audit's actor FK resolves. Migration 0036 creates the sentinel org row.
    let platform_admin = UserId::new();
    seed_platform_admin(&super_pool, platform_admin).await;
    let platform_token = issue_token(
        &runtime_pool,
        &keys,
        platform_admin,
        OrgId::platform(),
        vec!["SUPER_ADMIN".to_owned()],
        true,
    )
    .await;

    let response = service
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/platform/orgs")
                .header(header::AUTHORIZATION, format!("Bearer {platform_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"slug":"acme","name":"Acme Inc"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        status,
        StatusCode::CREATED,
        "platform admin should onboard a tenant; body={}",
        String::from_utf8_lossy(&body)
    );
    let onboarding: Value = serde_json::from_slice(&body).unwrap();
    let acme_id = onboarding["org"]["id"].as_str().unwrap().to_owned();
    let acme_admin_id = onboarding["admin_user_id"].as_str().unwrap().to_owned();
    assert!(
        onboarding["otp"].as_str().is_some_and(|s| !s.is_empty()),
        "onboarding must return a one-time OTP: {onboarding}"
    );
    let acme_org = OrgId::from_uuid(acme_id.parse().unwrap());

    // --- (2) TENANT ISOLATION under console_rt -------------------------------------
    // The acme admin token reads /api/v1/users → sees ONLY acme's admin, never KNL.
    let acme_admin = UserId::from_uuid(acme_admin_id.parse().unwrap());
    let acme_token = issue_token(
        &runtime_pool,
        &keys,
        acme_admin,
        acme_org,
        vec!["SUPER_ADMIN".to_owned()],
        false,
    )
    .await;
    let acme_users = read_users(&service, &acme_token).await;
    assert!(
        acme_users
            .iter()
            .any(|u| u["id"].as_str() == Some(acme_admin_id.as_str())),
        "acme must see its own admin: {acme_users:?}"
    );
    assert!(
        !acme_users
            .iter()
            .any(|u| u["id"].as_str() == Some(&knl_admin.to_string())),
        "acme must NOT see KNL's admin (cross-tenant leak): {acme_users:?}"
    );

    // The KNL admin token sees ONLY KNL's admin, never acme's.
    let knl_token = issue_token(
        &runtime_pool,
        &keys,
        knl_admin,
        knl,
        vec!["SUPER_ADMIN".to_owned()],
        false,
    )
    .await;
    let knl_users = read_users(&service, &knl_token).await;
    assert!(
        knl_users
            .iter()
            .any(|u| u["id"].as_str() == Some(&knl_admin.to_string())),
        "KNL must see its own admin: {knl_users:?}"
    );
    assert!(
        !knl_users
            .iter()
            .any(|u| u["id"].as_str() == Some(acme_admin_id.as_str())),
        "KNL must NOT see acme's admin (cross-tenant leak): {knl_users:?}"
    );

    // --- (3) TENANT token REJECTED on /api/platform/* --------------------------
    let response = service
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/platform/orgs")
                .header(header::AUTHORIZATION, format!("Bearer {knl_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "a tenant token must be rejected on /api/platform/* (tier crossing)"
    );

    // --- (4) PLATFORM token REJECTED on a tenant /api/v1/* route ---------------
    let response = service
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/users")
                .header(header::AUTHORIZATION, format!("Bearer {platform_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "a platform token must be rejected on tenant /api/* routes (tier crossing)"
    );

    // --- (5) GET /api/platform/orgs lists acme + knl (cross-tenant read) -------
    let response = service
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/platform/orgs")
                .header(header::AUTHORIZATION, format!("Bearer {platform_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let listing: Value = serde_json::from_slice(&body).unwrap();
    let slugs: Vec<&str> = listing
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|o| o["slug"].as_str())
        .collect();
    assert!(slugs.contains(&"acme"), "list must include acme: {slugs:?}");
    assert!(slugs.contains(&"knl"), "list must include knl: {slugs:?}");
    assert!(
        !slugs.contains(&"platform"),
        "list must NOT include the platform sentinel: {slugs:?}"
    );

    // --- (6) PATCH /api/platform/orgs/{id} suspends acme -----------------------
    let response = service
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/platform/orgs/{acme_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {platform_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"status":"SUSPENDED"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "PATCH should suspend; body={}",
        String::from_utf8_lossy(&body)
    );
    let patched: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(patched["status"].as_str(), Some("SUSPENDED"));
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn read_users(service: &axum::Router, token: &str) -> Vec<Value> {
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
        "authenticated tenant read should succeed under console_rt"
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    json.get("users")
        .or_else(|| json.get("items"))
        .and_then(Value::as_array)
        .or_else(|| json.as_array())
        .cloned()
        .expect("response should contain a user array")
}

async fn console_rt_pool(super_pool: &PgPool) -> PgPool {
    // Preserve the SQLx per-test database, Unix socket and TLS options together.
    let options = super_pool.connect_options().as_ref().clone();
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
        .expect("failed to build console_rt runtime pool")
}

async fn seed_tenant(pool: &PgPool, org: OrgId, slug: &str, admin_id: UserId) {
    let org_uuid = *org.as_uuid();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(org_uuid)
    .bind(slug)
    .bind(slug.to_uppercase())
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO users (id, display_name, roles, is_active, org_id) \
         VALUES ($1, $2, $3, true, $4)",
    )
    .bind(*admin_id.as_uuid())
    .bind(format!("{slug} Admin"))
    .bind(vec!["SUPER_ADMIN".to_string()])
    .bind(org_uuid)
    .execute(pool)
    .await
    .unwrap();
}

/// Seed a platform admin user in the platform sentinel org (created by 0036).
async fn seed_platform_admin(pool: &PgPool, admin_id: UserId) {
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, is_active, org_id) \
         VALUES ($1, 'Platform Admin', $2, true, \
                 '00000000-0000-0000-0000-00000000face'::uuid)",
    )
    .bind(*admin_id.as_uuid())
    .bind(vec!["SUPER_ADMIN".to_string()])
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
    keys: &Keys,
    user_id: UserId,
    org: OrgId,
    roles: Vec<String>,
    platform: bool,
) -> String {
    let settings = JwtSettings {
        issuer: TEST_ISSUER.to_owned(),
        audience: TEST_AUDIENCE.to_owned(),
        access_token_ttl: Duration::minutes(15),
    };
    let issuer = JwtIssuer::from_es256_pem(
        settings,
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
    )
    .unwrap();
    console_platform_test_support::issue_session_token(
        pool,
        &issuer,
        AccessTokenInput {
            subject: user_id,
            org_id: org,
            roles,
            branches: Vec::<BranchId>::new(),
            platform,
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
