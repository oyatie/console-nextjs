#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

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
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

/// Exercises the composed app router rather than a constant: authentication,
/// distinct compliance actions, org-scope denial, branch authorization, request
/// validation, server-derived actor, and the adapter's audit writer all execute.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn compliance_catalog_enforces_real_scope_and_audits(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch = seed_branch(&pool, "catalog primary").await;
    let other_branch = seed_branch(&pool, "catalog isolated").await;
    let admin = UserId::new();
    let super_admin = UserId::new();
    seed_user(&pool, admin, "ADMIN", Some(branch)).await;
    seed_user(&pool, super_admin, "SUPER_ADMIN", None).await;

    let admin_token = issue_token(
        private_key.as_bytes(),
        public_key.as_bytes(),
        admin,
        "ADMIN",
    );
    let super_token = issue_token(
        private_key.as_bytes(),
        public_key.as_bytes(),
        super_admin,
        "SUPER_ADMIN",
    );
    let service = build_router(app_state(pool.clone(), public_key.to_string()).unwrap());

    let unauthenticated = request(
        service.clone(),
        "GET",
        "/api/v1/compliance/regulations",
        None,
        None,
    )
    .await;
    assert_eq!(unauthenticated.status, StatusCode::UNAUTHORIZED);

    // An ADMIN has the branch-level compliance matrix cell, but may not turn it
    // into a tenant-wide regulation/framework/evidence view.
    let denied_org_read = request(
        service.clone(),
        "GET",
        "/api/v1/compliance/regulations",
        Some(&admin_token),
        None,
    )
    .await;
    assert_eq!(
        denied_org_read.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        denied_org_read.json
    );
    assert_eq!(denied_org_read.json["error"]["code"], "forbidden");

    let bad_page = request(
        service.clone(),
        "GET",
        "/api/v1/compliance/regulations?offset=-1",
        Some(&super_token),
        None,
    )
    .await;
    assert_eq!(
        bad_page.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        bad_page.json
    );

    let regulation = request(
        service.clone(),
        "POST",
        "/api/v1/compliance/regulations",
        Some(&super_token),
        Some(json!({
            "title": "Korea serious accident duties",
            "jurisdiction": "KR",
            "citation": "Serious Accidents Punishment Act",
            "impact_area": "safety",
            "impact_summary": "Assign accountable control owners.",
            "risk_level": "HIGH",
            "metadata": {"source": "integration-test"}
        })),
    )
    .await;
    assert_eq!(regulation.status, StatusCode::OK, "{:?}", regulation.json);
    assert_eq!(regulation.json["created_by"], super_admin.to_string());
    assert_eq!(regulation.json["risk_level"], "HIGH");
    assert_eq!(regulation.json["status"], "DRAFT");
    assert_eq!(
        audit_count(&pool, "compliance.regulation_impact.create").await,
        1
    );

    let invalid_id = request(
        service.clone(),
        "GET",
        "/api/v1/compliance/framework-controls?framework_id=not-a-uuid",
        Some(&super_token),
        None,
    )
    .await;
    assert_eq!(
        invalid_id.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        invalid_id.json
    );

    let branch_obligation = obligation_body(branch);
    let created = request(
        service.clone(),
        "POST",
        "/api/v1/compliance/obligations",
        Some(&admin_token),
        Some(branch_obligation),
    )
    .await;
    assert_eq!(created.status, StatusCode::OK, "{:?}", created.json);
    assert_eq!(created.json["created_by"], admin.to_string());
    assert_eq!(created.json["obligation_type"], "LEGAL");
    assert_eq!(created.json["scope"]["kind"], "BRANCH");
    assert_eq!(created.json["severity"], "HIGH");
    assert_eq!(created.json["status"], "DRAFT");
    assert_eq!(audit_count(&pool, "compliance.obligation.create").await, 1);

    let outside_scope = request(
        service,
        "POST",
        "/api/v1/compliance/obligations",
        Some(&admin_token),
        Some(obligation_body(other_branch)),
    )
    .await;
    assert_eq!(
        outside_scope.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        outside_scope.json
    );
    assert_eq!(outside_scope.json["error"]["code"], "forbidden");
}

fn obligation_body(branch_id: BranchId) -> Value {
    json!({
        "title": "Branch safety inspection",
        "description": "Document the monthly safety control review.",
        "obligation_type": "LEGAL",
        "scope": {
            "kind": "BRANCH",
            "scope_ref": branch_id.as_uuid().to_string(),
            "branch_id": branch_id.as_uuid().to_string(),
            "site_id": null
        },
        "severity": "HIGH",
        "metadata": {}
    })
}

struct Response {
    status: StatusCode,
    json: Value,
}

async fn request(
    service: axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let response = builder
        .body(Body::from(
            body.map_or_else(String::new, |value| value.to_string()),
        ))
        .unwrap();
    let response = service.oneshot(response).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    Response {
        status,
        json: serde_json::from_slice(&body).unwrap_or_else(|_| json!({})),
    }
}

fn issue_token(private: &[u8], public: &[u8], user_id: UserId, role: &str) -> String {
    JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private,
        public,
    )
    .unwrap()
    .issue_access_token(AccessTokenInput {
        subject: user_id,
        org_id: OrgId::knl(),
        roles: vec![role.to_owned()],
        branches: Vec::new(),
        platform: false,
        view_as: false,
        read_only: false,
        display_name: None,
        feature_grants: Vec::new(),
        authz_subject_version: 1,
        authz_policy_version: 1,
        session_generation: 1,
        issued_at: OffsetDateTime::now_utc(),
    })
    .unwrap()
}

fn app_state(pool: PgPool, public_key: String) -> Result<AppState, console_app::AppError> {
    AppState::new(
        AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
            ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
            ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
            ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
            ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key),
        ])?,
        DatabaseDependency::Postgres(pool),
    )
}

async fn seed_branch(pool: &PgPool, name: &str) -> BranchId {
    let region: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("{name} region"))
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    BranchId::from_uuid(
        sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(region)
        .bind(name)
        .bind(*OrgId::knl().as_uuid())
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}

async fn seed_user(pool: &PgPool, user_id: UserId, role: &str, branch: Option<BranchId>) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("catalog {role}"))
        .bind(vec![role])
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    if let Some(branch) = branch {
        sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
            .bind(*user_id.as_uuid())
            .bind(*branch.as_uuid())
            .bind(*OrgId::knl().as_uuid())
            .execute(pool)
            .await
            .unwrap();
    }
}

async fn audit_count(pool: &PgPool, action: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = $1")
        .bind(action)
        .fetch_one(pool)
        .await
        .unwrap()
}
