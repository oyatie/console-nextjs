#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, EquipmentId, OrgId, UserId, WorkOrderId};
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
async fn substitutes_endpoint_is_branch_scoped_and_super_admin_can_expand(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch = seed_branch(&pool, "REST Substitute Region", "REST Substitute Branch").await;
    let other_branch = seed_branch(&pool, "REST Other Region", "REST Other Branch").await;
    let admin = seed_user_with_branch(&pool, "ADMIN", branch).await;
    let super_admin = UserId::new();
    let down = seed_equipment(
        &pool,
        branch,
        EquipmentFixture::new("CFO25-0290", "290", "임대", "좌식", "2.5T"),
    )
    .await;
    seed_equipment(
        &pool,
        branch,
        EquipmentFixture::new("DFO25-0106", "106", "예비", "좌식", "2.5T")
            .placement_location("REST Reserve Exact"),
    )
    .await;
    seed_equipment(
        &pool,
        branch,
        EquipmentFixture::new("CFO35-0075", "075", "예비", "좌식", "3.5T"),
    )
    .await;
    seed_equipment(
        &pool,
        other_branch,
        EquipmentFixture::new("DFO25-9106", "9106", "예비", "좌식", "2.5T")
            .placement_location("REST Other Branch"),
    )
    .await;

    let admin_token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        admin,
        vec!["ADMIN".to_owned()],
        vec![branch],
    )
    .unwrap();
    let super_token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        super_admin,
        vec!["SUPER_ADMIN".to_owned()],
        vec![],
    )
    .unwrap();
    let service = build_router(app_state(pool, public_key_pem).unwrap());

    let scoped = get_json(
        service.clone(),
        &format!("/api/v1/equipment/{down}/substitutes"),
        &admin_token,
    )
    .await;
    assert_eq!(scoped.status, StatusCode::OK, "{:?}", scoped.json);
    assert_eq!(
        equipment_numbers(&scoped.json),
        vec!["DFO25-0106", "CFO35-0075"]
    );
    assert_eq!(scoped.json["items"][0]["status"], "spare");
    assert_eq!(
        scoped.json["items"][0]["placement_location"],
        "REST Reserve Exact"
    );

    let forbidden = get_json(
        service.clone(),
        &format!("/api/v1/equipment/{down}/substitutes?all_branches=true"),
        &admin_token,
    )
    .await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);

    let expanded = get_json(
        service,
        &format!("/api/v1/equipment/{down}/substitutes?all_branches=true"),
        &super_token,
    )
    .await;
    assert_eq!(expanded.status, StatusCode::OK, "{:?}", expanded.json);
    assert_eq!(
        equipment_numbers(&expanded.json),
        vec!["DFO25-0106", "DFO25-9106", "CFO35-0075"]
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn equipment_timeline_graph_is_branch_scoped_and_links_work_orders(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch = seed_branch(&pool, "Timeline Region", "Timeline Branch").await;
    let other_branch = seed_branch(&pool, "Timeline Other Region", "Timeline Other").await;
    let admin = seed_user_with_branch(&pool, "ADMIN", branch).await;
    let equipment = seed_equipment(
        &pool,
        branch,
        EquipmentFixture::new("TFO25-0290", "290", "임대", "좌식", "2.5T"),
    )
    .await;
    let hidden_equipment = seed_equipment(
        &pool,
        other_branch,
        EquipmentFixture::new("TFO25-0777", "777", "임대", "좌식", "2.5T"),
    )
    .await;
    let work_order = seed_work_order(&pool, branch, equipment, admin, "20260613-101").await;

    let admin_token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        admin,
        vec!["ADMIN".to_owned()],
        vec![branch],
    )
    .unwrap();
    let service = build_router(app_state(pool, public_key_pem).unwrap());

    let response = get_json(
        service.clone(),
        &format!("/api/v1/equipment/{equipment}/timeline-graph"),
        &admin_token,
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.json);
    assert_eq!(
        response.json["equipment"]["equipment_id"],
        equipment.to_string()
    );
    assert_eq!(response.json["work_order_count"], 1);
    assert!(
        lifecycle_kinds(&response.json).contains(&"rental_started"),
        "{:?}",
        response.json["lifecycle_events"]
    );
    assert!(
        lifecycle_kinds(&response.json).contains(&"work_order"),
        "{:?}",
        response.json["lifecycle_events"]
    );
    assert!(
        graph_node_ids(&response.json)
            .iter()
            .any(|id| id == &format!("work_order:{work_order}")),
        "{:?}",
        response.json["graph"]["nodes"]
    );
    assert!(
        graph_edge_kinds(&response.json).contains(&"has_work_order"),
        "{:?}",
        response.json["graph"]["edges"]
    );

    let hidden = get_json(
        service,
        &format!("/api/v1/equipment/{hidden_equipment}/timeline-graph"),
        &admin_token,
    )
    .await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND, "{:?}", hidden.json);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn object_action_catalog_and_executor_are_governed(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch = seed_branch(&pool, "Action Region", "Action Branch").await;
    let admin = seed_user_with_branch(&pool, "ADMIN", branch).await;
    let equipment = seed_equipment(
        &pool,
        branch,
        EquipmentFixture::new("AFO25-0290", "290", "임대", "좌식", "2.5T"),
    )
    .await;

    let admin_token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        admin,
        vec!["ADMIN".to_owned()],
        vec![branch],
    )
    .unwrap();
    let service = build_router(app_state(pool.clone(), public_key_pem).unwrap());

    let catalog = get_json(
        service.clone(),
        &format!("/api/v1/object-actions/catalog?object_type=equipment&object_id={equipment}"),
        &admin_token,
    )
    .await;
    assert_eq!(catalog.status, StatusCode::OK, "{:?}", catalog.json);
    assert_eq!(catalog.json["object_type"], "equipment");
    assert_eq!(
        catalog.json["actions"][0]["action_id"],
        "equipment.update_profile"
    );
    assert_eq!(catalog.json["actions"][0]["requires_passkey_step_up"], true);
    assert!(
        catalog.json["actions"][0]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["field_key"] == "status" && field["field_type"] == "select"),
        "{:?}",
        catalog.json["actions"][0]["fields"]
    );

    let rejected = post_json(
        service,
        "/api/v1/object-actions/execute",
        &admin_token,
        serde_json::json!({
            "action_id": "equipment.update_profile",
            "object_type": "equipment",
            "object_id": equipment,
            "input": {
                "status": "spare"
            }
        }),
    )
    .await;
    assert_eq!(
        rejected.status,
        StatusCode::PRECONDITION_REQUIRED,
        "{:?}",
        rejected.json
    );
    assert_eq!(rejected.json["error"]["code"], "passkey_step_up_required");

    let audit_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = 'equipment.update'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(audit_count, 0, "rejected action must not write audit");
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

async fn get_json(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
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
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
    JsonResponse { status, json }
}

async fn post_json(service: axum::Router, uri: &str, token: &str, body: Value) -> JsonResponse {
    let response = service
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
    JsonResponse { status, json }
}

fn equipment_numbers(json: &Value) -> Vec<&str> {
    json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["equipment_no"].as_str().unwrap())
        .collect()
}

fn lifecycle_kinds(json: &Value) -> Vec<&str> {
    json["lifecycle_events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["kind"].as_str().unwrap())
        .collect()
}

fn graph_node_ids(json: &Value) -> Vec<&str> {
    json["graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["id"].as_str().unwrap())
        .collect()
}

fn graph_edge_kinds(json: &Value) -> Vec<&str> {
    json["graph"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge["kind"].as_str().unwrap())
        .collect()
}

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

#[derive(Debug, Clone)]
struct EquipmentFixture {
    equipment_no: &'static str,
    management_no: &'static str,
    status: &'static str,
    specification: &'static str,
    ton: &'static str,
    placement_location: Option<&'static str>,
}

impl EquipmentFixture {
    fn new(
        equipment_no: &'static str,
        management_no: &'static str,
        status: &'static str,
        specification: &'static str,
        ton: &'static str,
    ) -> Self {
        Self {
            equipment_no,
            management_no,
            status,
            specification,
            ton,
            placement_location: None,
        }
    }

    fn placement_location(mut self, placement_location: &'static str) -> Self {
        self.placement_location = Some(placement_location);
        self
    }
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

async fn seed_user_with_branch(pool: &PgPool, role: &str, branch_id: BranchId) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("Registry REST {role}"))
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
    user_id
}

async fn seed_equipment(
    pool: &PgPool,
    branch_id: BranchId,
    fixture: EquipmentFixture,
) -> EquipmentId {
    let customer_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_customers (branch_id, name, org_id)
        VALUES ($1, 'K&L', $2)
        ON CONFLICT (branch_id, name) DO UPDATE SET name = EXCLUDED.name
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_sites (branch_id, customer_id, name, org_id)
        VALUES ($1, $2, '케이앤엘', $3)
        ON CONFLICT (branch_id, customer_id, name) DO UPDATE SET name = EXCLUDED.name
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let ton_milli = ton_milli(fixture.ton);
    let equipment_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, power_label, status,
            placement_location, specification, ton_text, ton_milli,
            model, asset_registered_on, rental_started_on, acquisition_date,
            source_sheet, source_row, org_id
        )
        VALUES (
            $1, $2, $3, $4, $5,
            $6, $7, $8, $9, $10,
            $11, $12, $13, $14,
            $15, '2024-01-10', '2024-02-01', '2024-01-15',
            'test fixture', 1, $16
        )
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(fixture.equipment_no)
    .bind(fixture.management_no)
    .bind(&fixture.equipment_no[0..1])
    .bind(&fixture.equipment_no[1..2])
    .bind(&fixture.equipment_no[2..3])
    .bind(power_label(&fixture.equipment_no[2..3]))
    .bind(fixture.status)
    .bind(fixture.placement_location)
    .bind(fixture.specification)
    .bind(fixture.ton)
    .bind(ton_milli)
    .bind(format!("Model {}", fixture.management_no))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    EquipmentId::from_uuid(equipment_id)
}

async fn seed_work_order(
    pool: &PgPool,
    branch_id: BranchId,
    equipment_id: EquipmentId,
    requested_by: UserId,
    request_no: &str,
) -> WorkOrderId {
    let work_order_id = WorkOrderId::new();
    let (customer_id, site_id): (uuid::Uuid, uuid::Uuid) =
        sqlx::query_as("SELECT customer_id, site_id FROM registry_equipment WHERE id = $1")
            .bind(*equipment_id.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();

    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, priority, symptom, target_due_at, org_id
        )
        VALUES (
            $1, $2, $3, $4, $5, $6,
            $7, 'ASSIGNED', 'P1', 'Timeline graph fixture', '2026-06-13T10:00:00Z', $8
        )
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(request_no)
    .bind(*branch_id.as_uuid())
    .bind(*equipment_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(*requested_by.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    work_order_id
}

fn ton_milli(ton: &str) -> Option<i32> {
    ton.strip_suffix('T')
        .and_then(|raw| raw.parse::<f64>().ok())
        .map(|tons| (tons * 1000.0).round() as i32)
}

fn power_label(power_code: &str) -> &'static str {
    match power_code {
        "B" => "전동",
        "O" => "디젤",
        _ => "기타",
    }
}
