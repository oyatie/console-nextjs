//! Group-admin tenant-context mint: REAL end-to-end handler proof that the token
//! carries live subject freshness (Cedar/PBAC activation, ADR-0021).
//!
//! Unlike a construction-seam test (which supplies the freshness itself and so
//! would still pass if the handler stopped reading it), this drives the actual
//! `POST /api/v1/group-admin/tenant-context` handler through the built `router()`
//! under the real `console_rt` runtime role. It fails if the handler stops sourcing
//! freshness, stamps zeros, or transposes the fields — because it asserts the
//! MINTED token carries the target subsidiary's DB-current `policy_version` (and
//! the absent-0 baseline for a cross-org actor), and that a later bump flows.
//!
//! `console_rt` (NOSUPERUSER, NOBYPASSRLS) — never a BYPASSRLS superuser pool, which
//! would mask a broken read path (rls-verify-as-runtime-role).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use console_kernel_core::{OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_auth_rest::{
    AuthRestConfig, AuthRestState, GROUP_ADMIN_TENANT_CONTEXT_PATH, router,
};
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

struct Keys {
    private_pem: String,
    public_pem: String,
}

fn keypair() -> Keys {
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

fn settings() -> JwtSettings {
    JwtSettings {
        issuer: TEST_ISSUER.to_owned(),
        audience: TEST_AUDIENCE.to_owned(),
        access_token_ttl: Duration::minutes(15),
    }
}

/// Build the auth-rest state from an explicit keypair, so the test can mint an
/// actor token the built router verifies with the SAME key.
fn state_with_keys(pool: PgPool, keys: &Keys) -> AuthRestState {
    AuthRestState::new(
        pool,
        AuthRestConfig {
            rp_id: "example.com".to_owned(),
            rp_origin: "https://auth.example.com".to_owned(),
            rp_name: "Console".to_owned(),
            ceremony_ttl: Duration::minutes(5),
            jwt_issuer: TEST_ISSUER.to_owned(),
            jwt_audience: TEST_AUDIENCE.to_owned(),
            jwt_private_key_pem: keys.private_pem.clone(),
            jwt_public_key_pem: keys.public_pem.clone(),
            refresh_token_ttl: Duration::days(30),
            refresh_family_absolute_ttl: Duration::hours(24),
            cookie_secure: false,
        },
    )
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

// --- seeding (owner pool; group tables are owner-only, console_rt has no access) ---

async fn seed_org(owner_pool: &PgPool, slug: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO organizations (slug, name, status) VALUES ($1, $2, 'ACTIVE') RETURNING id",
    )
    .bind(slug)
    .bind(format!("Org {slug}"))
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

async fn seed_user(owner_pool: &PgPool, org: Uuid) -> UserId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id, is_active) VALUES ($1, $2, $3, true) RETURNING id",
    )
    .bind(format!("User {}", Uuid::new_v4()))
    .bind(vec!["ADMIN".to_owned()])
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    UserId::from_uuid(id)
}

async fn seed_group(owner_pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO groups (slug, name) VALUES ($1, $2) RETURNING id")
        .bind(format!("g-{}", &Uuid::new_v4().simple().to_string()[..12]))
        .bind("Test Group")
        .fetch_one(owner_pool)
        .await
        .unwrap()
}

async fn seed_group_membership(owner_pool: &PgPool, group: Uuid, org: Uuid) {
    sqlx::query(
        "INSERT INTO group_memberships (group_id, org_id) VALUES ($1, $2)
         ON CONFLICT (org_id) DO UPDATE SET group_id = EXCLUDED.group_id",
    )
    .bind(group)
    .bind(org)
    .execute(owner_pool)
    .await
    .unwrap();
    sqlx::query("UPDATE organizations SET group_id = $1 WHERE id = $2")
        .bind(group)
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
}

async fn seed_group_admin_grant(owner_pool: &PgPool, group: Uuid, user: UserId) {
    sqlx::query(
        "INSERT INTO group_role_grants (group_id, user_id, group_role) VALUES ($1, $2, 'GROUP_ADMIN')",
    )
    .bind(group)
    .bind(*user.as_uuid())
    .execute(owner_pool)
    .await
    .unwrap();
}

async fn set_policy_version(owner_pool: &PgPool, org: Uuid, version: i64) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO policy_versions (org_id, version, updated_at)
        VALUES ($1, $2, now())
        ON CONFLICT (org_id) DO UPDATE SET version = EXCLUDED.version, updated_at = now()
        "#,
    )
    .bind(org)
    .bind(version)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// Mint the ACTOR's own session token: a normal (tenant_context = None) token
/// carrying the GROUP_ADMIN group role, exactly what a group admin logs in with.
async fn mint_actor_token(
    pool: &PgPool,
    issuer: &JwtIssuer,
    actor: UserId,
    home_org: OrgId,
) -> String {
    console_platform_test_support::issue_session_token(
        pool,
        issuer,
        AccessTokenInput {
            subject: actor,
            org_id: home_org,
            roles: vec!["ADMIN".to_owned()],
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
        },
        None,
        vec!["GROUP_ADMIN".to_owned()],
    )
    .await
}

async fn post_tenant_context(app: axum::Router, token: &str, org_id: Uuid) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(GROUP_ADMIN_TENANT_CONTEXT_PATH)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "org_id": org_id }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

#[sqlx::test(migrations = "../db/migrations")]
async fn handler_mints_token_with_real_subject_freshness(owner_pool: PgPool) {
    // A group with the target subsidiary as member and a CROSS-ORG actor (home in
    // a different org, so no `users` row in the target → absent-0 subject/session).
    let target = seed_org(&owner_pool, "acme").await;
    let parent = seed_org(&owner_pool, "parent").await;
    let actor = seed_user(&owner_pool, parent).await;
    let group = seed_group(&owner_pool).await;
    seed_group_membership(&owner_pool, group, target).await;
    seed_group_admin_grant(&owner_pool, group, actor).await;
    // Target subsidiary has a real custom-policy revision.
    set_policy_version(&owner_pool, target, 7).await;

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let keys = keypair();
    let issuer = JwtIssuer::from_es256_pem(
        settings(),
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
    )
    .unwrap();
    let verifier =
        JwtVerifier::from_es256_public_pem(settings(), keys.public_pem.as_bytes()).unwrap();
    let app = router(state_with_keys(rt_pool.clone(), &keys));
    let actor_token = mint_actor_token(&rt_pool, &issuer, actor, OrgId::from_uuid(parent)).await;

    // Drive the REAL handler.
    let (status, body) = post_tenant_context(app.clone(), &actor_token, target).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    let claims = verifier
        .verify_access_token(body["access_token"].as_str().unwrap())
        .unwrap();
    // The minted token is the group-admin tenant-context token pinned to the target.
    assert_eq!(claims.org, target.to_string());
    assert_eq!(claims.actor_home_org, Some(parent.to_string()));
    assert_ne!(claims.actor_home_org.as_deref(), Some(claims.org.as_str()));
    // The load-bearing assertion: the handler sourced the target org's REAL
    // policy_version — a regression to a 0 stamp, or a field transposition, fails.
    assert_eq!(
        claims.authz_policy_version, 7,
        "handler must stamp the target subsidiary's real policy_version, not 0"
    );
    assert_eq!(
        claims.authz_subject_version, 0,
        "a cross-org actor has no subject row in the target → absent 0 baseline"
    );
    assert_eq!(claims.session_generation, 0);

    // Bump the target's policy revision and re-POST: the new value must flow,
    // proving the handler reads freshness LIVE on each mint (not a cached/constant).
    set_policy_version(&owner_pool, target, 8).await;
    let (status, body) = post_tenant_context(app, &actor_token, target).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let claims = verifier
        .verify_access_token(body["access_token"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        claims.authz_policy_version, 8,
        "handler must read the bumped policy_version live on the next mint"
    );
}
