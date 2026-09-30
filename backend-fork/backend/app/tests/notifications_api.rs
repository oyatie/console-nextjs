#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Notification-center REST E2E over the REAL router on a genuine non-owner
//! `console_rt` pool (RLS actually enforced, never a BYPASSRLS superuser).
//!
//! This is the shape that catches the failure a superuser-pool test masks: a
//! handler that never armed `app.current_org` returns nothing to real console_rt
//! traffic even though the row exists. Here user A must SEE its own notification
//! over HTTP as console_rt (proving the read path arms the tenant GUC), and B's id
//! must 404 for A (recipient scoping). Helpers copied from
//! workflow_runtime_instance_api.rs per the no-shared-helper convention.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, UserId};
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

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn notifications_are_recipient_scoped_over_http_as_runtime_role(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let user_a = UserId::new();
    let user_b = UserId::new();
    seed_user(&pool, user_a, "ADMIN", branch).await;
    seed_user(&pool, user_b, "ADMIN", branch).await;
    let notif_a = seed_notification(&pool, user_a).await;
    let notif_b = seed_notification(&pool, user_b).await;

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token_a = bearer(&keys, user_a, "ADMIN", branch);

    // A lists over HTTP as console_rt: must SEE its own row. If the handler failed
    // to arm app.current_org, RLS would hide it and this length check fails.
    let listed = get(service.clone(), "/api/v1/me/notifications", &token_a).await;
    assert_eq!(listed.status, StatusCode::OK, "{:?}", listed.json);
    let items = listed.json["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "A sees exactly its own notification as console_rt"
    );
    assert_eq!(items[0]["id"].as_str().unwrap(), notif_a.to_string());

    // A marking B's notification read is a 404 (recipient scoping), and A can
    // mark its own.
    let cross = post_empty(
        service.clone(),
        &format!("/api/v1/me/notifications/{notif_b}/read"),
        &token_a,
    )
    .await;
    assert_eq!(cross.status, StatusCode::NOT_FOUND, "{:?}", cross.json);

    let own = post_empty(
        service.clone(),
        &format!("/api/v1/me/notifications/{notif_a}/read"),
        &token_a,
    )
    .await;
    assert_eq!(own.status, StatusCode::OK, "{:?}", own.json);
    assert_eq!(own.json["unread"].as_bool(), Some(false));
}

async fn seed_notification(pool: &PgPool, recipient: UserId) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO notifications (id, org_id, recipient_user_id, category, body, link) \
         VALUES ($1, $2, $3, '결재', '결재 문서가 도착했습니다', \
                 '{\"type\":\"screen\",\"screen\":\"approvals\"}'::jsonb)",
    )
    .bind(id)
    .bind(OrgId::knl().as_uuid())
    .bind(recipient.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    id
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

fn bearer(keys: &Keys, user_id: UserId, role: &str, branch: BranchId) -> String {
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
            roles: vec![role.to_owned()],
            branches: vec![branch],
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

async fn seed_branch(pool: &PgPool) -> BranchId {
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind("Notif Region")
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind("Notif Branch")
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(pool: &PgPool, user_id: UserId, role: &str, _branch_id: BranchId) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("notif-{role}-{}", user_id.as_uuid()))
        .bind(vec![role])
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

async fn get(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    send(service, "GET", uri, token).await
}

async fn post_empty(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    send(service, "POST", uri, token).await
}

async fn send(service: axum::Router, method: &str, uri: &str, token: &str) -> JsonResponse {
    let request = Request::builder()
        .uri(uri)
        .method(method)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
}
