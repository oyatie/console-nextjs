#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn dispatch_queue_is_authenticated_authorized_and_scope_closed(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch = seed_branch(&pool).await;
    let administrator = UserId::new();
    let member = UserId::new();
    seed_user(&pool, administrator, "ADMIN", branch).await;
    seed_user(&pool, member, "MEMBER", branch).await;
    let service = build_router(app_state(pool, public_pem.clone()).unwrap());
    let admin_token = issue_token(
        private_pem.as_bytes(),
        public_pem.as_bytes(),
        administrator,
        vec!["ADMIN".to_owned()],
        vec![branch],
    );
    let member_token = issue_token(
        private_pem.as_bytes(),
        public_pem.as_bytes(),
        member,
        vec!["MEMBER".to_owned()],
        vec![branch],
    );

    let allowed = get_json(
        service.clone(),
        "/api/v1/console/dispatch/queue?status=RECEIVED&limit=1",
        &admin_token,
    )
    .await;
    assert_eq!(allowed.0, StatusCode::OK, "{:?}", allowed.1);
    assert!(allowed.1.get("items").is_some());

    let forbidden = get_json(
        service.clone(),
        "/api/v1/console/dispatch/queue?status=RECEIVED&limit=1",
        &member_token,
    )
    .await;
    assert_eq!(forbidden.0, StatusCode::FORBIDDEN, "{:?}", forbidden.1);

    let validation = get_json(
        service.clone(),
        "/api/v1/console/dispatch/queue?status=NOT_A_STATUS",
        &admin_token,
    )
    .await;
    assert_eq!(
        validation.0,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        validation.1
    );
    assert!(
        validation.1["error"]["code"].is_string(),
        "{:?}",
        validation.1
    );

    for uri in [
        "/api/v1/console/dispatch/queue?branch_id=all",
        "/api/v1/p1-dispatches/not-a-uuid/candidates",
        "/api/v1/p1-dispatches/not-a-uuid/responses",
    ] {
        let malformed = get_json(service.clone(), uri, &admin_token).await;
        assert_eq!(
            malformed.0,
            StatusCode::BAD_REQUEST,
            "{uri}: {:?}",
            malformed.1
        );
        assert_eq!(
            malformed.1["error"]["code"], "bad_request",
            "{uri}: {:?}",
            malformed.1
        );
    }

    let scope_escalation = get_json(
        service,
        "/api/v1/console/dispatch/queue?status=RECEIVED&branch_id=all",
        &admin_token,
    )
    .await;
    assert_eq!(
        scope_escalation.0,
        StatusCode::BAD_REQUEST,
        "{:?}",
        scope_escalation.1
    );
}

async fn get_json(service: axum::Router, uri: &str, token: &str) -> (StatusCode, Value) {
    let response = service
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    assert!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json")),
        "{uri} must return application/json"
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

fn issue_token(
    private_pem: &[u8],
    public_pem: &[u8],
    user: UserId,
    roles: Vec<String>,
    branches: Vec<BranchId>,
) -> String {
    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private_pem,
        public_pem,
    )
    .unwrap();
    issuer
        .issue_access_token(AccessTokenInput {
            subject: user,
            org_id: OrgId::knl(),
            roles,
            branches,
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

fn app_state(pool: PgPool, public_pem: String) -> Result<AppState, console_app::AppError> {
    AppState::new(
        AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
            ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
            ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
            ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
            ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_pem),
        ])?,
        DatabaseDependency::Postgres(pool),
    )
}

async fn seed_branch(pool: &PgPool) -> BranchId {
    let region: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO regions (name, org_id) VALUES ('Dispatch API Region', $1) RETURNING id",
    )
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(
        sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, 'Dispatch API Branch', $2) RETURNING id",
        )
        .bind(region)
        .bind(*OrgId::knl().as_uuid())
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}

async fn seed_user(pool: &PgPool, user: UserId, role: &str, branch: BranchId) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user.as_uuid())
        .bind(format!("Dispatch API {role}"))
        .bind(Vec::from([role]))
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user.as_uuid())
        .bind(*branch.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}
