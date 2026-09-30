#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Runtime route proof for the tenant-scoped Benefits vertical.
//!
//! This uses the app router and a non-owner `console_rt` pool: JWT/PBAC denial and
//! authorization, catalog mutations, generic lifecycle transitions, and both
//! catalog and lifecycle audit events are proved through the deployed route
//! composition rather than store-direct calls.

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

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn benefit_catalog_routes_enforce_pbac_and_audit_catalog_and_lifecycle_writes(
    owner_pool: PgPool,
) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let org = OrgId::knl();
    let member_id = UserId::new();
    let admin_id = UserId::new();
    seed_tenant_user(&owner_pool, org, member_id, "MEMBER", "Benefits Member").await;
    seed_tenant_user(&owner_pool, org, admin_id, "SUPER_ADMIN", "Benefits Admin").await;

    let member_token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        member_id,
        org,
        vec!["MEMBER".to_owned()],
    );
    let admin_token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        admin_id,
        org,
        vec!["SUPER_ADMIN".to_owned()],
    );
    let service = build_router(app_state(
        console_rt_pool(&owner_pool).await,
        public_key_pem,
    ));
    let create_body = benefit_body();

    let denied = request(
        service.clone(),
        "POST",
        "/api/v1/benefit-catalog/items",
        &member_token,
        Some(create_body.clone()),
    )
    .await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        benefit_catalog_item_count(&owner_pool).await,
        0,
        "a denied principal must not reach the write path"
    );

    let created = request(
        service.clone(),
        "POST",
        "/api/v1/benefit-catalog/items",
        &admin_token,
        Some(create_body),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: Value = body_json(created).await;
    let item_id = created["id"].as_str().expect("created item id").to_owned();
    assert_eq!(created["lifecycle"]["current_state"], "draft");

    let updated = request(
        service.clone(),
        "PATCH",
        &format!("/api/v1/benefit-catalog/items/{item_id}"),
        &admin_token,
        Some(json!({"name": "국민연금 개정"})),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);

    let tiers = request(
        service.clone(),
        "PUT",
        &format!("/api/v1/benefit-catalog/items/{item_id}/tiers"),
        &admin_token,
        Some(json!({"tiers": [tier("관리자")]})),
    )
    .await;
    assert_eq!(tiers.status(), StatusCode::OK);

    let conditions = request(
        service.clone(),
        "PUT",
        &format!("/api/v1/benefit-catalog/items/{item_id}/conditions"),
        &admin_token,
        Some(json!({"conditions": [condition("정규직")]})),
    )
    .await;
    assert_eq!(conditions.status(), StatusCode::OK);

    let transitioned = request(
        service.clone(),
        "POST",
        &format!("/api/v1/lifecycles/benefit_catalog_item/{item_id}/transition"),
        &admin_token,
        Some(json!({"toState": "pending", "reason": "benefit catalog review"})),
    )
    .await;
    assert_eq!(transitioned.status(), StatusCode::OK);
    assert_eq!(body_json(transitioned).await["currentState"], "pending");

    let listed = request(
        service,
        "GET",
        "/api/v1/benefit-catalog/items?category=legal&limit=10&offset=0",
        &admin_token,
        None,
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = body_json(listed).await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["items"][0]["lifecycle"]["current_state"], "pending");

    let actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_events WHERE org_id = $1 ORDER BY occurred_at, created_at",
    )
    .bind(*org.as_uuid())
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    for action in [
        "benefit_catalog.item.create",
        "benefit_catalog.item.update",
        "benefit_catalog.tiers.replace",
        "benefit_catalog.conditions.replace",
        "lifecycle.transition",
    ] {
        assert!(
            actions.iter().any(|seen| seen == action),
            "missing audit {action}: {actions:?}"
        );
    }
}

async fn request(
    service: axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    service
        .oneshot(
            builder
                .body(match body {
                    Some(body) => Body::from(body.to_string()),
                    None => Body::empty(),
                })
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

async fn benefit_catalog_item_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM benefit_catalog_items")
        .fetch_one(pool)
        .await
        .unwrap()
}

fn benefit_body() -> Value {
    json!({
        "scope": {"scope_type": "ORG", "scope_ref": null, "branch_id": null, "site_id": null},
        "category": "legal",
        "name": "국민연금",
        "coverageLabel": "전 직원",
        "coveredCount": 12,
        "costLabel": "월 120,000원",
        "estimatedAnnualCostWon": 1440000,
        "employerRateBps": 450,
        "metadata": {},
        "tiers": [tier("정규직")],
        "conditions": [condition("재직자")]
    })
}

fn tier(label: &str) -> Value {
    json!({
        "tier_basis": "employment_type",
        "tier_key": "regular",
        "value_label": label,
        "amount_won": 120000,
        "limit_period": "MONTH",
        "criteria": {"employment_type": "regular"},
        "display_order": 0
    })
}

fn condition(label: &str) -> Value {
    json!({
        "condition_kind": "ORG",
        "operator": "exists",
        "condition_key": "employee",
        "condition_value": {"active": true},
        "display_label": label,
        "cedar_policy_ref": null,
        "display_order": 0
    })
}

async fn console_rt_pool(owner_pool: &PgPool) -> PgPool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for sqlx::test");
    let db_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(owner_pool)
        .await
        .unwrap();
    let base = url
        .rsplit_once('/')
        .map(|(prefix, _)| prefix.to_owned())
        .unwrap_or(url);
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|connection, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&format!("{base}/{db_name}"))
        .await
        .unwrap()
}

async fn seed_tenant_user(pool: &PgPool, org: OrgId, user: UserId, role: &str, name: &str) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, 'knl', 'KNL') \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, is_active, org_id) \
         VALUES ($1, $2, $3, true, $4)",
    )
    .bind(*user.as_uuid())
    .bind(name)
    .bind(vec![role.to_owned()])
    .bind(*org.as_uuid())
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

fn issue_token(
    private_pem: &[u8],
    public_pem: &[u8],
    user_id: UserId,
    org: OrgId,
    roles: Vec<String>,
) -> String {
    JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private_pem,
        public_pem,
    )
    .unwrap()
    .issue_access_token(AccessTokenInput {
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
    })
    .unwrap()
}
