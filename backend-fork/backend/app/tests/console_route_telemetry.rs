#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use console_platform_db::{DbError, with_org_conn};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn route_telemetry_records_tenant_events_and_surfaces_adoption_by_org(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    let branch_id = seed_branch(&pool).await.unwrap();
    let tenant_user = UserId::new();
    seed_user_with_branch(&pool, tenant_user, "MEMBER", branch_id)
        .await
        .unwrap();
    let platform_user = seed_platform_admin(&pool).await.unwrap();

    let tenant_token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        tenant_user,
        OrgId::knl(),
        vec!["MEMBER".to_owned()],
        vec![branch_id],
        false,
    )
    .unwrap();
    let platform_token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        platform_user,
        OrgId::platform(),
        vec!["SUPER_ADMIN".to_owned()],
        vec![],
        true,
    )
    .unwrap();
    let service = build_router(app_state(pool.clone(), public_key_pem).unwrap());

    post_json(
        service.clone(),
        "/api/v1/console/telemetry/route",
        &tenant_token,
        json!({
            "event_kind": "route_selection",
            "route_surface": "legacy",
            "route_path": "/work-orders/:id",
            "release_cycle": "2026.07.1",
            "duration_ms": 180
        }),
        StatusCode::ACCEPTED,
    )
    .await;
    post_json(
        service.clone(),
        "/api/v1/console/telemetry/route",
        &tenant_token,
        json!({
            "event_kind": "route_selection",
            "route_surface": "console",
            "route_path": "/console/identity",
            "release_cycle": "2026.07.1",
            "duration_ms": 95
        }),
        StatusCode::ACCEPTED,
    )
    .await;
    post_json(
        service.clone(),
        "/api/v1/console/telemetry/route",
        &tenant_token,
        json!({
            "event_kind": "rum_error",
            "route_surface": "console",
            "route_path": "/console/identity",
            "release_cycle": "2026.07.1",
            "error_name": "RouteBoundaryCrash"
        }),
        StatusCode::ACCEPTED,
    )
    .await;
    post_json(
        service.clone(),
        "/api/v1/console/telemetry/route",
        &tenant_token,
        json!({
            "event_kind": "route_selection",
            "route_surface": "console",
            "route_path": "/console/identity",
            "release_cycle": "2026.07.2",
            "duration_ms": 88
        }),
        StatusCode::ACCEPTED,
    )
    .await;

    let stored: (i64, i64, i64) = with_org_conn(&pool, OrgId::knl(), |tx| {
        Box::pin(async move {
            Ok::<_, DbError>(
                sqlx::query_as(
                    r#"
                    SELECT
                        COUNT(*) FILTER (WHERE route_surface = 'legacy')::BIGINT,
                        COUNT(*) FILTER (WHERE route_surface = 'console')::BIGINT,
                        COUNT(*) FILTER (WHERE event_kind = 'rum_error')::BIGINT
                    FROM console_route_telemetry
                    WHERE org_id = $1 AND user_id = $2
                    "#,
                )
                .bind(*OrgId::knl().as_uuid())
                .bind(*tenant_user.as_uuid())
                .fetch_one(tx.as_mut())
                .await?,
            )
        })
    })
    .await
    .unwrap();
    assert_eq!(stored, (1, 3, 1));

    let ops = get_json(
        service,
        "/api/platform/ops",
        &platform_token,
        StatusCode::OK,
    )
    .await;
    let tenant = ops["tenants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tenant| tenant["id"] == OrgId::knl().as_uuid().to_string())
        .expect("Knl tenant should be present in platform ops");
    assert_eq!(tenant["zero_legacy_release_cycles"], 1);

    let adoption = tenant["route_adoption"].as_array().unwrap();
    let first_cycle = adoption
        .iter()
        .find(|metric| metric["release_cycle"] == "2026.07.1")
        .expect("first release cycle should be aggregated");
    assert_eq!(first_cycle["legacy_route_events"], 1);
    assert_eq!(first_cycle["console_route_events"], 1);
    assert_eq!(first_cycle["rum_error_events"], 1);
    assert_eq!(first_cycle["rum_perf_p95_ms"], 180);

    let zero_legacy_cycle = adoption
        .iter()
        .find(|metric| metric["release_cycle"] == "2026.07.2")
        .expect("zero-legacy release cycle should be aggregated");
    assert_eq!(zero_legacy_cycle["legacy_route_events"], 0);
    assert_eq!(zero_legacy_cycle["console_route_events"], 1);
}

async fn get_json(service: axum::Router, uri: &str, token: &str, expected: StatusCode) -> Value {
    let response = service
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), expected);
    body_json(response).await
}

async fn post_json(
    service: axum::Router,
    uri: &str,
    token: &str,
    body: Value,
    expected: StatusCode,
) -> Value {
    let response = service
        .oneshot(
            Request::builder()
                .uri(uri)
                .method("POST")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), expected);
    body_json(response).await
}

async fn body_json(response: http::Response<Body>) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    }
}

fn issue_token(
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    org_id: OrgId,
    roles: Vec<String>,
    branches: Vec<BranchId>,
    platform: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private_key_pem,
        public_key_pem,
    )?;

    Ok(issuer.issue_access_token(AccessTokenInput {
        subject: user_id,
        org_id,
        roles,
        branches,
        platform,
        view_as: false,
        read_only: false,
        display_name: None,
        feature_grants: Vec::new(),
        authz_subject_version: 0,
        authz_policy_version: 0,
        session_generation: 0,
        issued_at: OffsetDateTime::now_utc(),
    })?)
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

async fn seed_branch(pool: &PgPool) -> Result<BranchId, sqlx::Error> {
    let region_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO regions (name, org_id) VALUES ('Route Telemetry Region', $1) RETURNING id",
    )
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await?;
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, 'Route Telemetry Branch', $2) RETURNING id",
    )
    .bind(region_id)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await?;
    Ok(BranchId::from_uuid(branch_id))
}

async fn seed_user_with_branch(
    pool: &PgPool,
    user_id: UserId,
    role: &str,
    branch_id: BranchId,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("Route Telemetry User {role}"))
        .bind(Vec::from([role]))
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await?;
    Ok(())
}

async fn seed_platform_admin(pool: &PgPool) -> Result<UserId, sqlx::Error> {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, status) VALUES ($1, 'platform', 'Platform', 'ARCHIVED') ON CONFLICT (id) DO NOTHING",
    )
    .bind(*OrgId::platform().as_uuid())
    .execute(pool)
    .await?;
    let id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*id.as_uuid())
        .bind("Platform Route Telemetry Admin")
        .bind(Vec::from(["SUPER_ADMIN"]))
        .bind(*OrgId::platform().as_uuid())
        .execute(pool)
        .await?;
    Ok(id)
}
