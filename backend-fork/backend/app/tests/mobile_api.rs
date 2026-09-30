#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, UserId, WorkOrderId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn mobile_sync_is_jwt_authorized_idempotent_and_reports_partial_failures(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Mobile Region", "Mobile Branch").await;
    let mechanic = UserId::new();
    let receptionist = UserId::new();
    seed_user_with_branch(&pool, mechanic, "MECHANIC", branch_id).await;
    seed_user_with_branch(&pool, receptionist, "RECEPTIONIST", branch_id).await;
    let equipment_id = seed_equipment(&pool, branch_id, "290").await;
    let work_order_id =
        seed_assigned_work_order(&pool, branch_id, equipment_id, receptionist, mechanic).await;
    let token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        mechanic,
        vec!["MECHANIC".to_owned()],
        vec![branch_id],
    )
    .unwrap();
    let service = build_router(app_state(pool.clone(), public_key_pem).unwrap());
    let missing_work_order_id = WorkOrderId::new();
    let body = json!({
        "sync_id": uuid::Uuid::new_v4(),
        "operations": [
            {
                "request_id": "missing-start-1",
                "operation": "WORK_ORDER_START",
                "created_at": OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                "payload": { "work_order_id": missing_work_order_id }
            },
            {
                "request_id": "start-1",
                "operation": "WORK_ORDER_START",
                "created_at": OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                "payload": { "work_order_id": work_order_id }
            }
        ]
    });

    let first = post_json(
        service.clone(),
        "/api/v1/sync",
        &token,
        Some(("x-device-id", "ios-device-01")),
        body.clone(),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.json);
    assert_eq!(first.json["results"][0]["status"], "FAILED");
    assert_eq!(first.json["results"][0]["http_status"], 404);
    assert_eq!(first.json["results"][1]["status"], "APPLIED");
    assert_eq!(first.json["results"][1]["result"]["status"], "IN_PROGRESS");
    assert_eq!(first.json["results"][1]["replayed"], false);

    let second = post_json(
        service,
        "/api/v1/sync",
        &token,
        Some(("x-device-id", "ios-device-01")),
        body,
    )
    .await;
    assert_eq!(second.status, StatusCode::OK);
    assert_eq!(second.json["results"][0]["status"], "FAILED");
    assert_eq!(second.json["results"][0]["replayed"], true);
    assert_eq!(second.json["results"][1]["status"], "APPLIED");
    assert_eq!(second.json["results"][1]["replayed"], true);

    let start_history_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_order_status_history WHERE work_order_id = $1 AND action = 'work_order.start'",
    )
    .bind(*work_order_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(start_history_count, 1);

    let sync_row_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM offline_sync_requests WHERE device_hash <> $1")
            .bind("ios-device-01")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(sync_row_count, 2);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn device_registration_upserts_by_user_and_hashed_device(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Device Region", "Device Branch").await;
    let mechanic = UserId::new();
    seed_user_with_branch(&pool, mechanic, "MECHANIC", branch_id).await;
    let token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        mechanic,
        vec!["MECHANIC".to_owned()],
        vec![branch_id],
    )
    .unwrap();
    let service = build_router(app_state(pool.clone(), public_key_pem).unwrap());
    let body = json!({
        "platform": "ios",
        "push_token": null,
        "app_version": "1.0.0"
    });

    let first = post_json(
        service.clone(),
        "/api/v1/devices",
        &token,
        Some(("x-device-id", "ios-device-01")),
        body.clone(),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(first.json["platform"], "ios");
    assert_ne!(first.json["device_hash"], "ios-device-01");

    let second = post_json(
        service,
        "/api/v1/devices",
        &token,
        Some(("x-device-id", "ios-device-01")),
        body,
    )
    .await;
    assert_eq!(second.status, StatusCode::OK);
    assert_eq!(second.json["id"], first.json["id"]);

    let device_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM registered_devices WHERE user_id = $1 AND device_hash <> $2",
    )
    .bind(*mechanic.as_uuid())
    .bind("ios-device-01")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(device_count, 1);

    let audit_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = 'device.register'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(audit_count, 2);
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

async fn post_json(
    service: axum::Router,
    uri: &str,
    token: &str,
    extra_header: Option<(&str, &str)>,
    body: Value,
) -> JsonResponse {
    let mut request = Request::builder()
        .uri(uri)
        .method("POST")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some((name, value)) = extra_header {
        request = request.header(name, value);
    }
    let response = service
        .oneshot(Body::from(body.to_string()).pipe(|body| request.body(body).unwrap()))
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}

fn issue_token(
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    roles: Vec<String>,
    branches: Vec<BranchId>,
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

async fn seed_branch(pool: &PgPool, region_name: &str, branch_name: &str) -> BranchId {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(region_name)
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(branch_name)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user_with_branch(pool: &PgPool, user_id: UserId, role: &str, branch_id: BranchId) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("Mobile API {role}"))
        .bind(Vec::from([role]))
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_equipment(pool: &PgPool, branch_id: BranchId, management_no: &str) -> uuid::Uuid {
    let equipment_suffix = format!("{:0>4}", management_no);
    let customer_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(format!("Mobile Customer {management_no}"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(format!("Mobile Site {management_no}"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, model, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, $4, $5,
                'A', 'B', 'C', '임대', '좌식', '2.5', 'GTS25DE', 'test', 1, $6)
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(format!("ABC12-{equipment_suffix}"))
    .bind(management_no)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_assigned_work_order(
    pool: &PgPool,
    branch_id: BranchId,
    equipment_id: uuid::Uuid,
    receptionist: UserId,
    mechanic: UserId,
) -> WorkOrderId {
    let work_order_id = WorkOrderId::new();
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, priority, symptom, org_id
        )
        SELECT $1, '20260612-801', $2, e.id, e.customer_id, e.site_id,
               $3, 'ASSIGNED', 'UNSET', 'Mobile sync fixture', $5
        FROM registry_equipment e
        WHERE e.id = $4
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*branch_id.as_uuid())
    .bind(*receptionist.as_uuid())
    .bind(equipment_id)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO work_order_assignments (work_order_id, mechanic_id, role, assigned_at, org_id)
        VALUES ($1, $2, 'PRIMARY', now(), $3)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*mechanic.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    work_order_id
}
