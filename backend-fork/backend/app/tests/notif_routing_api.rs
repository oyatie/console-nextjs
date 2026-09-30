#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Notification ROUTING surfaces (by-object grouping, unread toggle, mute
//! policies) E2E over the REAL app router on a genuine non-owner `console_rt`
//! pool: full signature chain (ES256 JWT -> principal -> armed tenant GUC ->
//! RLS), denial WITHOUT existence leakage (anon 401; cross-user and
//! cross-tenant ids 404, indistinguishable from absent), and audit readback
//! for every mutation. Helpers copied from notifications_api.rs per the
//! no-shared-helper convention.

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
const OTHER_ORG: Uuid = Uuid::from_u128(0x7207_2472_0724_7207_2472_0724_7207_2472);

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn notification_routing_is_isolated_over_http_as_runtime_role(pool: PgPool) {
    let keys = keys();
    seed_other_org(&pool).await;
    let user_a = UserId::new();
    let user_b = UserId::new();
    let outsider = UserId::new();
    seed_user(&pool, user_a, *OrgId::knl().as_uuid()).await;
    seed_user(&pool, user_b, *OrgId::knl().as_uuid()).await;
    seed_user(&pool, outsider, OTHER_ORG).await;

    // A: two rows pointing at the same approval object + one screen row.
    let approval_link = r#"{"type":"object","kind":"approval","id":"ap-2026-042"}"#;
    seed_notification(&pool, user_a, "결재", approval_link).await;
    seed_notification(&pool, user_a, "멘션", approval_link).await;
    let toggle_id = seed_notification(
        &pool,
        user_a,
        "근태",
        r#"{"type":"screen","screen":"attendance"}"#,
    )
    .await;
    // B: one row on the SAME approval object (must never fold into A's group).
    seed_notification(&pool, user_b, "결재", approval_link).await;

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token_a = bearer(&keys, user_a, OrgId::knl());
    let token_b = bearer(&keys, user_b, OrgId::knl());
    let token_outsider = bearer(&keys, outsider, OrgId::from_uuid(OTHER_ORG));

    // --- signature chain: anon is 401 before any data path runs ---
    let anon = service
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/me/notifications/by-object")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);

    // --- by-object grouping over HTTP as console_rt (GUC armed by the chain) ---
    let a_groups = get(
        service.clone(),
        "/api/v1/me/notifications/by-object",
        &token_a,
    )
    .await;
    assert_eq!(a_groups.status, StatusCode::OK, "{:?}", a_groups.json);
    let a_items = a_groups.json["items"].as_array().unwrap();
    assert_eq!(a_items.len(), 2, "approval group + attendance screen group");
    let approval_group = a_items
        .iter()
        .find(|g| g["link"]["id"].as_str() == Some("ap-2026-042"))
        .expect("approval group");
    assert_eq!(
        approval_group["total"].as_i64(),
        Some(2),
        "B's row on the same object never counts for A"
    );

    // --- cross-tenant isolation: the outsider's tenant sees nothing ---
    let outsider_groups = get(
        service.clone(),
        "/api/v1/me/notifications/by-object",
        &token_outsider,
    )
    .await;
    assert_eq!(outsider_groups.status, StatusCode::OK);
    assert_eq!(
        outsider_groups.json["items"].as_array().unwrap().len(),
        0,
        "another tenant sees no groups"
    );
    let outsider_toggle = post_empty(
        service.clone(),
        &format!("/api/v1/me/notifications/{toggle_id}/unread"),
        &token_outsider,
    )
    .await;
    assert_eq!(
        outsider_toggle.status,
        StatusCode::NOT_FOUND,
        "cross-tenant id is 404, indistinguishable from absent"
    );

    // --- unread toggle round-trip + cross-user denial without leakage ---
    let read = post_empty(
        service.clone(),
        &format!("/api/v1/me/notifications/{toggle_id}/read"),
        &token_a,
    )
    .await;
    assert_eq!(read.status, StatusCode::OK, "{:?}", read.json);
    let toggled = post_empty(
        service.clone(),
        &format!("/api/v1/me/notifications/{toggle_id}/unread"),
        &token_a,
    )
    .await;
    assert_eq!(toggled.status, StatusCode::OK, "{:?}", toggled.json);
    assert_eq!(toggled.json["unread"].as_bool(), Some(true));
    assert!(
        toggled.json["read_at"].as_str().is_some(),
        "first-read forensic timestamp survives the toggle"
    );
    let cross_user = post_empty(
        service.clone(),
        &format!("/api/v1/me/notifications/{toggle_id}/unread"),
        &token_b,
    )
    .await;
    assert_eq!(cross_user.status, StatusCode::NOT_FOUND);
    let absent = post_empty(
        service.clone(),
        &format!("/api/v1/me/notifications/{}/unread", Uuid::new_v4()),
        &token_b,
    )
    .await;
    assert_eq!(
        (cross_user.status, cross_user.json["error"]["code"].as_str()),
        (absent.status, absent.json["error"]["code"].as_str()),
        "cross-user and truly-absent ids are indistinguishable"
    );

    // --- mute policy: direct-apply, badge-effective, recipient-owned ---
    let upserted = send_json(
        service.clone(),
        "PUT",
        "/api/v1/me/notification-policies",
        &token_a,
        json!({ "scope": "object", "link": { "type": "object", "kind": "approval", "id": "ap-2026-042" } }),
    )
    .await;
    assert_eq!(upserted.status, StatusCode::OK, "{:?}", upserted.json);
    let policy_id = upserted.json["id"].as_str().unwrap().to_owned();

    let count = get(
        service.clone(),
        "/api/v1/me/notifications/unread-count",
        &token_a,
    )
    .await;
    assert_eq!(
        count.json["unread"].as_i64(),
        Some(1),
        "both approval-object rows are muted out of the badge"
    );
    let muted_group = get(
        service.clone(),
        "/api/v1/me/notifications/by-object",
        &token_a,
    )
    .await;
    let muted_items = muted_group.json["items"].as_array().unwrap();
    assert_eq!(muted_items.len(), 2, "muting hides attention, not groups");
    assert_eq!(
        muted_items
            .iter()
            .find(|g| g["link"]["id"].as_str() == Some("ap-2026-042"))
            .unwrap()["muted"]
            .as_bool(),
        Some(true),
        "the group bell reflects the object policy"
    );

    // Cross-user policy delete is 404; own delete is 204 and restores counts.
    let cross_delete = send_empty(
        service.clone(),
        "DELETE",
        &format!("/api/v1/me/notification-policies/{policy_id}"),
        &token_b,
    )
    .await;
    assert_eq!(cross_delete.status, StatusCode::NOT_FOUND);
    let deleted = send_empty(
        service.clone(),
        "DELETE",
        &format!("/api/v1/me/notification-policies/{policy_id}"),
        &token_a,
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    let restored = get(
        service.clone(),
        "/api/v1/me/notifications/unread-count",
        &token_a,
    )
    .await;
    assert_eq!(restored.json["unread"].as_i64(), Some(3));

    // --- audit readback: every mutation above is on the trail ---
    for (action, expected) in [
        ("notification.read", 1i64),
        ("notification.unread", 1),
        ("notification.policy_set", 1),
        ("notification.policy_clear", 1),
    ] {
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = $1")
            .bind(action)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, expected, "audit readback for {action}");
    }
}

async fn seed_notification(pool: &PgPool, recipient: UserId, category: &str, link: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO notifications (id, org_id, recipient_user_id, category, body, link) \
         VALUES ($1, $2, $3, $4, '결재 문서가 도착했습니다', $5::jsonb)",
    )
    .bind(id)
    .bind(OrgId::knl().as_uuid())
    .bind(recipient.as_uuid())
    .bind(category)
    .bind(link)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn seed_other_org(pool: &PgPool) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, 'org-notif-other', 'Notif Other') \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(OTHER_ORG)
    .execute(pool)
    .await
    .unwrap();
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

fn bearer(keys: &Keys, user_id: UserId, org: OrgId) -> String {
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
            org_id: org,
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
        })
        .unwrap()
}

async fn seed_user(pool: &PgPool, user_id: UserId, org: Uuid) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("notif-routing-{}", user_id.as_uuid()))
        .bind(vec!["ADMIN"])
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
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

async fn get(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    send_empty(service, "GET", uri, token).await
}

async fn post_empty(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    send_empty(service, "POST", uri, token).await
}

async fn send_empty(service: axum::Router, method: &str, uri: &str, token: &str) -> JsonResponse {
    let request = Request::builder()
        .uri(uri)
        .method(method)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    dispatch(service, request).await
}

async fn send_json(
    service: axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Value,
) -> JsonResponse {
    let request = Request::builder()
        .uri(uri)
        .method(method)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    dispatch(service, request).await
}

async fn dispatch(service: axum::Router, request: Request<Body>) -> JsonResponse {
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
}
