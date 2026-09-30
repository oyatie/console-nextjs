//! dev-auth: mint a local role-switch session for an arbitrary role/org/branch
//! combo, proving (a) it works end-to-end — a real signed token backed by a
//! real `users`/`user_branches` row, so branch-scoped RLS resolves the same way
//! it would for a real employee — and (b) calling it again for the SAME
//! (org, role) reuses that one persona (idempotent upsert) instead of piling up
//! throwaway rows.
//!
//! Only compiled with `--features dev-auth` (this whole file is cfg'd out
//! otherwise — see `dev_auth_absence.rs` for the default-build proof). Runs
//! against the real, non-owner `console_rt` role (rls-verify-as-runtime-role): a
//! superuser test pool would let a broken FORCE-RLS insert pass silently.
#![cfg(feature = "dev-auth")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use console_kernel_core::OrgId;
use console_platform_auth::RefreshTokenStore;
use console_platform_auth_rest::{AuthRestConfig, AuthRestState, router};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

#[derive(Debug, Deserialize)]
struct SessionResponse {
    access_token: String,
    refresh_token: Option<String>,
    requires_passkey_setup: bool,
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

fn test_state(pool: PgPool) -> AuthRestState {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key
        .to_pkcs8_pem(LineEnding::LF)
        .unwrap()
        .to_string();
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    AuthRestState::new(
        pool,
        AuthRestConfig {
            rp_id: "example.com".to_owned(),
            rp_origin: "https://auth.example.com".to_owned(),
            rp_name: "Console".to_owned(),
            ceremony_ttl: Duration::minutes(5),
            jwt_issuer: TEST_ISSUER.to_owned(),
            jwt_audience: TEST_AUDIENCE.to_owned(),
            jwt_private_key_pem: private_pem,
            jwt_public_key_pem: public_pem,
            refresh_token_ttl: Duration::days(30),
            refresh_family_absolute_ttl: Duration::hours(24),
            cookie_secure: false,
        },
    )
    .unwrap()
}

/// Seed a fresh org + region + branch as the migration-owner pool (RLS-exempt
/// setup fixture, mirroring `backend/app/tests/auth_rest.rs`'s `seed_branch`).
async fn seed_org_and_branch(pool: &PgPool) -> (Uuid, Uuid) {
    let org_id = Uuid::new_v4();
    let slug = format!("dev-{}", &org_id.simple().to_string()[..8]);
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, status) VALUES ($1, $2, 'Dev Org', 'ACTIVE')",
    )
    .bind(org_id)
    .bind(&slug)
    .execute(pool)
    .await
    .unwrap();
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Region {slug}"))
            .bind(org_id)
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, 'Branch', $2) RETURNING id",
    )
    .bind(region_id)
    .bind(org_id)
    .fetch_one(pool)
    .await
    .unwrap();
    (org_id, branch_id)
}

async fn post(app: axum::Router, org_id: Uuid, body: serde_json::Value) -> http::Response<Body> {
    let _ = org_id;
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/v1/dev-auth/session")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn post_cookie_session(
    app: axum::Router,
    org_id: Uuid,
    body: serde_json::Value,
) -> http::Response<Body> {
    let _ = org_id;
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/v1/dev-auth/session")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-auth-transport", "cookie")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn post_cookie_refresh(app: axum::Router, cookie: &str) -> http::Response<Body> {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/v1/auth/token/refresh")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-auth-transport", "cookie")
            .header(header::COOKIE, cookie)
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await
    .unwrap()
}

fn refresh_cookie(response: &http::Response<Body>) -> String {
    response
        .headers()
        .get(header::SET_COOKIE)
        .expect("cookie transport must set a refresh cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn mints_a_real_session_and_backs_it_with_a_real_user(pool: PgPool) {
    let (org_id, branch_id) = seed_org_and_branch(&pool).await;
    let rt_pool = runtime_role_pool(&pool).await;
    let app = router(test_state(rt_pool));

    let response = post(
        app.clone(),
        org_id,
        json!({
            "org_id": org_id,
            "role": "MECHANIC",
            "branch_ids": [branch_id],
            "feature_grants": ["some.feature"],
            "display_name": "Dev Mechanic",
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let session: SessionResponse = serde_json::from_slice(&bytes).unwrap();
    assert!(!session.access_token.is_empty());
    assert!(!session.requires_passkey_setup);
    assert!(
        session.refresh_token.is_some(),
        "body-transport (mobile) request must return a refresh token"
    );

    // A real, branch-scoped user row backs the session — this is what lets
    // `resolve_branch_scope_in_org` (re-run on every subsequent request) see
    // this branch for a non-admin role.
    let user_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM users WHERE org_id = $1 AND display_name = 'Dev Mechanic'",
    )
    .bind(org_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let branch_row: Uuid =
        sqlx::query_scalar("SELECT branch_id FROM user_branches WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(branch_row, branch_id);

    // Calling it again for the SAME (org, role) reuses the SAME persona.
    let second = post(
        app,
        org_id,
        json!({
            "org_id": org_id,
            "role": "MECHANIC",
            "branch_ids": [branch_id],
            "display_name": "Dev Mechanic (renamed)",
        }),
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE org_id = $1")
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 1,
        "the same (org, role) dev persona must be reused, not duplicated"
    );
    let renamed: String = sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(renamed, "Dev Mechanic (renamed)");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn cookie_refresh_keeps_synthetic_dev_persona_out_of_passkey_onboarding(pool: PgPool) {
    let (org_id, branch_id) = seed_org_and_branch(&pool).await;
    let rt_pool = runtime_role_pool(&pool).await;
    let app = router(test_state(rt_pool));

    let minted = post_cookie_session(
        app.clone(),
        org_id,
        json!({
            "org_id": org_id,
            "role": "MECHANIC",
            "branch_ids": [branch_id],
            "display_name": "Refreshable Dev Mechanic",
        }),
    )
    .await;
    assert_eq!(minted.status(), StatusCode::OK);
    let minted_cookie = refresh_cookie(&minted);
    let minted_body: SessionResponse =
        serde_json::from_slice(&to_bytes(minted.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert!(!minted_body.requires_passkey_setup);
    assert!(
        minted_body.refresh_token.is_none(),
        "cookie transport must not expose the refresh token in JSON"
    );

    let refreshed = post_cookie_refresh(app, &minted_cookie).await;
    assert_eq!(refreshed.status(), StatusCode::OK);
    let refreshed_body: SessionResponse =
        serde_json::from_slice(&to_bytes(refreshed.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert!(
        !refreshed_body.requires_passkey_setup,
        "an authenticated synthetic dev persona must remain outside production passkey onboarding"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn cookie_refresh_still_requires_passkey_for_ordinary_zero_passkey_user(pool: PgPool) {
    let (org_id, branch_id) = seed_org_and_branch(&pool).await;
    let user_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO users (display_name, phone, roles, is_active, org_id)
        VALUES ('Ordinary User', '010-9000-0000', ARRAY['MECHANIC'], true, $1)
        RETURNING id
        "#,
    )
    .bind(org_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(branch_id)
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();

    let rt_pool = runtime_role_pool(&pool).await;
    let issued = RefreshTokenStore
        .issue_family(
            &rt_pool,
            user_id,
            OrgId::from_uuid(org_id),
            time::OffsetDateTime::now_utc(),
            Duration::days(30),
        )
        .await
        .unwrap();
    let app = router(test_state(rt_pool));
    let cookie = format!("console_refresh={}", issued.token.as_str());

    let refreshed = post_cookie_refresh(app, &cookie).await;
    assert_eq!(refreshed.status(), StatusCode::OK);
    let refreshed_body: SessionResponse =
        serde_json::from_slice(&to_bytes(refreshed.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert!(
        refreshed_body.requires_passkey_setup,
        "dev-auth builds must preserve passkey onboarding for ordinary zero-passkey users"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn rejects_an_unknown_role(pool: PgPool) {
    let (org_id, _branch_id) = seed_org_and_branch(&pool).await;
    let rt_pool = runtime_role_pool(&pool).await;
    let app = router(test_state(rt_pool));

    let response = post(app, org_id, json!({"org_id": org_id, "role": "NOT_A_ROLE"})).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn rejects_the_platform_sentinel_org(pool: PgPool) {
    let rt_pool = runtime_role_pool(&pool).await;
    let app = router(test_state(rt_pool));

    let response = post(
        app,
        *OrgId::platform().as_uuid(),
        json!({"org_id": OrgId::platform().as_uuid(), "role": "ADMIN"}),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn rejects_an_org_that_does_not_exist(pool: PgPool) {
    let rt_pool = runtime_role_pool(&pool).await;
    let app = router(test_state(rt_pool));
    let unknown_org = Uuid::new_v4();

    let response = post(
        app,
        unknown_org,
        json!({"org_id": unknown_org, "role": "ADMIN"}),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn rejects_a_branch_that_does_not_belong_to_the_org(pool: PgPool) {
    let (org_id, _branch_id) = seed_org_and_branch(&pool).await;
    let (_other_org, other_branch_id) = seed_org_and_branch(&pool).await;
    let rt_pool = runtime_role_pool(&pool).await;
    let app = router(test_state(rt_pool));

    let response = post(
        app,
        org_id,
        json!({"org_id": org_id, "role": "MECHANIC", "branch_ids": [other_branch_id]}),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
