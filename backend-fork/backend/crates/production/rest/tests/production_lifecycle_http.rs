#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! Runtime-role HTTP coverage for the production planning lifecycle.
//!
//! Fixtures are seeded as the migration owner, while every HTTP request uses
//! the non-owner `console_rt` role. This ensures the real request-context/RLS path
//! is exercised rather than a BYPASSRLS pool or direct SQL substitute.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_test_support::{grant_console_rt, runtime_role_pool};
use console_production_rest::{
    PRODUCTION_CAPACITY_SLOTS_PATH, PRODUCTION_PLAN_PATH, PRODUCTION_PLANS_PATH,
    PRODUCTION_SOURCE_INGRESS_PATH, PRODUCTION_SOURCE_SYSTEM_DISABLE_PATH,
    PRODUCTION_SOURCE_SYSTEM_ROTATE_PATH, PRODUCTION_SOURCE_SYSTEMS_PATH, ProductionRestState,
    router,
};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const ISSUER: &str = "console-platform-auth";
const AUDIENCE: &str = "console-api";
const SERVICE_PRINCIPAL_HMAC_KEY: [u8; 32] = [42; 32];

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct Fixture {
    org: OrgId,
    branch: BranchId,
    planner: UserId,
    reviewer: UserId,
    operator: UserId,
    site: Uuid,
    demand: Uuid,
    capacity: Uuid,
    material: Uuid,
    ontology: Uuid,
    due_at: OffsetDateTime,
}

#[derive(Debug, PartialEq, sqlx::FromRow)]
struct LifecycleAuditRow {
    id: Uuid,
    org_id: Uuid,
    service_principal_id: Uuid,
    event_type: String,
    actor_id: Option<Uuid>,
    expected_generation: Option<i32>,
    resulting_generation: i32,
    occurred_at: OffsetDateTime,
}

fn keys() -> Keys {
    let key = SigningKey::random(&mut OsRng);
    Keys {
        private_pem: key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
        public_pem: key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap(),
    }
}

fn bearer(keys: &Keys, user: UserId, org: OrgId, role: &str, branch: BranchId) -> String {
    JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: ISSUER.to_owned(),
            audience: AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
    )
    .unwrap()
    .issue_access_token(AccessTokenInput {
        subject: user,
        org_id: org,
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

fn app(pool: PgPool, keys: &Keys) -> axum::Router {
    let verifier = JwtVerifier::from_es256_public_pem(
        JwtSettings {
            issuer: ISSUER.to_owned(),
            audience: AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        keys.public_pem.as_bytes(),
    )
    .unwrap();
    router(
        ProductionRestState::new(pool, Some(verifier))
            .with_service_principal_hmac_key(Some(SERVICE_PRINCIPAL_HMAC_KEY)),
    )
}

async fn post(service: axum::Router, uri: &str, token: &str, body: Value) -> (StatusCode, Value) {
    let response = service
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&body).unwrap_or_else(|_| json!({})),
    )
}

async fn post_source(
    service: axum::Router,
    uri: &str,
    authorization: String,
    body: Value,
) -> (StatusCode, Value) {
    let response = service
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::AUTHORIZATION, authorization)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&body).unwrap_or_else(|_| json!({})),
    )
}

fn source_authorization(id: &str, encoded_secret: &str) -> String {
    let secret = base64::engine::general_purpose::STANDARD
        .decode(encoded_secret)
        .expect("source credential is base64");
    assert_eq!(secret.len(), 32, "source credential has exactly 32 bytes");
    let mut wire = id.as_bytes().to_vec();
    wire.push(b':');
    wire.extend(secret);
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(wire)
    )
}

async fn get(service: axum::Router, uri: &str, token: &str) -> (StatusCode, Value) {
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
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&body).unwrap_or_else(|_| json!({})),
    )
}

async fn seed_fixture(pool: &PgPool) -> Fixture {
    // `resolve_required_sources` joins `customer_inquiries` to reject demand whose
    // inquiry is CLOSED. Production hands console_rt that table through the
    // default-privilege auto-grant, but `#[sqlx::test]` migrates as a different
    // superuser, so the runtime role reaches it here only via an explicit GRANT.
    grant_console_rt(pool, &["GRANT SELECT ON customer_inquiries TO console_rt"]).await;
    let org = OrgId::knl();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
        .bind(*org.as_uuid())
        .bind("production-http")
        .bind("Production HTTP")
        .execute(pool)
        .await
        .unwrap();

    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("production-http-{}", Uuid::new_v4()))
            .bind(*org.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch = BranchId::from_uuid(
        sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(region)
        .bind(format!("production-http-{}", Uuid::new_v4()))
        .bind(*org.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap(),
    );
    let planner = seed_user(pool, org, branch, "SUPER_ADMIN", "planner").await;
    let reviewer = seed_user(pool, org, branch, "SUPER_ADMIN", "reviewer").await;
    let operator = seed_user(pool, org, branch, "SUPER_ADMIN", "operator").await;
    let customer: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, org_id, name) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(*org.as_uuid())
    .bind("Production customer")
    .fetch_one(pool)
    .await
    .unwrap();
    let site: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, org_id, name) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(customer)
    .bind(*org.as_uuid())
    .bind("Production site")
    .fetch_one(pool)
    .await
    .unwrap();
    let inquiry = Uuid::new_v4();
    sqlx::query("INSERT INTO customer_inquiries (id, org_id, name, phone, topic, status) VALUES ($1, $2, $3, $4, 'OTHER', 'NEW')")
        .bind(inquiry).bind(*org.as_uuid()).bind("Production customer").bind("010-0000-0000")
        .execute(pool).await.unwrap();
    // `production_demand_contracts.due_at` is compared to the request body for exact
    // equality (lib.rs `demand_due_at != request.due_at`). Postgres stores timestamptz
    // at microsecond precision, so a nanosecond-precision `now_utc()` would come back
    // truncated and never match the value the wire carried.
    let due_at = (OffsetDateTime::now_utc() + Duration::days(7))
        .replace_nanosecond(0)
        .unwrap();
    let demand = Uuid::new_v4();
    sqlx::query("INSERT INTO production_demand_contracts (id, org_id, inquiry_id, product_code, quantity, due_at, source_system, source_id, source_version, evaluated_at) VALUES ($1,$2,$3,'IV-PROD-001',10,$4,'sales','inquiry-1','v1',now())")
        .bind(demand).bind(*org.as_uuid()).bind(inquiry).bind(due_at).execute(pool).await.unwrap();
    let location: Uuid = sqlx::query_scalar("INSERT INTO inventory_stock_locations (org_id,branch_id,site_id,label,status) VALUES ($1,$2,$3,'Production store','ACTIVE') RETURNING id")
        .bind(*org.as_uuid()).bind(*branch.as_uuid()).bind(site).fetch_one(pool).await.unwrap();
    let material: Uuid = sqlx::query_scalar("INSERT INTO inventory_items (org_id,branch_id,stock_location_id,site_id,iv_code,display_name,unit_code,quantity_on_hand_milli,safety_stock_milli,status,created_by) VALUES ($1,$2,$3,$4,'IV-PROD-001','Production material','EA',100,5,'ACTIVE',$5) RETURNING id")
        .bind(*org.as_uuid()).bind(*branch.as_uuid()).bind(location).bind(site).bind(*planner.as_uuid()).fetch_one(pool).await.unwrap();
    let capacity = Uuid::new_v4();
    sqlx::query("INSERT INTO production_capacity_slots (id,org_id,branch_id,site_id,capacity_date,available_quantity,reserved_quantity,source_system,source_id,source_version,evaluated_at) VALUES ($1,$2,$3,$4,$5,100,0,'erp','capacity-1','v1',now())")
        .bind(capacity).bind(*org.as_uuid()).bind(*branch.as_uuid()).bind(site).bind(due_at.date()).execute(pool).await.unwrap();
    // 0165 closed ont_object_types to unaudited writers: every row must be
    // accompanied by exactly one ontology.object_type.create audit row inserted by the
    // same transaction (trg_ont_object_types_current_audit), and only console_rt or
    // console_ontology_cmd may write that audit row (protected_audit_writer_guard).
    // console_rt is also the only invoker for which prepare_legacy_object_type_write()
    // auto-creates the ont_object_type_key_revisions sidecar the composite FK needs.
    // Seeding this row on the owner pool satisfied neither guard, so it is written here
    // through the same retained-binary shape the runtime uses. This test had never
    // executed, so both guards could land without anything reporting the fixture broke.
    let stable_key = format!("production.plan.test{}", Uuid::new_v4().simple());
    let ontology = Uuid::new_v4();
    let ontology_at = OffsetDateTime::now_utc();
    let mut ontology_tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE console_rt")
        .execute(&mut *ontology_tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(&mut *ontology_tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO ont_object_types (id,org_id,stable_key,title,backing_kind,schema_version,lifecycle_state,created_by,created_at,updated_at) VALUES ($1,$2,$3,'Production plan','instance',1,'published',$4,$5,$5)")
        .bind(ontology).bind(*org.as_uuid()).bind(&stable_key).bind(*planner.as_uuid()).bind(ontology_at)
        .execute(&mut *ontology_tx).await.unwrap();
    sqlx::query("INSERT INTO audit_events (actor,action,target_type,target_id,trace_id,span_id,occurred_at,org_id) VALUES ($1,'ontology.object_type.create','ont_object_types',$2,'0123456789abcdef0123456789abcdef','0123456789abcdef',$3,$4)")
        .bind(*planner.as_uuid()).bind(ontology.to_string()).bind(ontology_at).bind(*org.as_uuid())
        .execute(&mut *ontology_tx).await.unwrap();
    ontology_tx.commit().await.unwrap();
    Fixture {
        org,
        branch,
        planner,
        reviewer,
        operator,
        site,
        demand,
        capacity,
        material,
        ontology,
        due_at,
    }
}

async fn seed_user(pool: &PgPool, org: OrgId, branch: BranchId, role: &str, name: &str) -> UserId {
    let user = UserId::new();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, org_id, is_active) VALUES ($1,$2,$3,$4,true)",
    )
    .bind(*user.as_uuid())
    .bind(name)
    .bind(vec![role.to_owned()])
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1,$2,$3)")
        .bind(*user.as_uuid())
        .bind(*branch.as_uuid())
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    user
}

async fn seed_isolated_tenant(pool: &PgPool) -> (OrgId, BranchId, UserId) {
    let org = OrgId::from_uuid(Uuid::new_v4());
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(*org.as_uuid())
        // `organizations.slug` CHECKs `^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$` — a 40-character
        // ceiling (0026_create_organizations.sql:17-18). A full simple UUID is 32 hex chars,
        // so `production-isolated-` + 32 was 52 and could never have inserted. Truncating the
        // UUID keeps the per-test uniqueness this needs at 21 characters.
        .bind(format!(
            "prod-iso-{}",
            &Uuid::new_v4().simple().to_string()[..12]
        ))
        .bind("Isolated production tenant")
        .execute(pool)
        .await
        .unwrap();
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("production-isolated-region-{}", Uuid::new_v4()))
            .bind(*org.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch = BranchId::from_uuid(
        sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(region)
        .bind(format!("production-isolated-branch-{}", Uuid::new_v4()))
        .bind(*org.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap(),
    );
    let user = seed_user(pool, org, branch, "SUPER_ADMIN", "isolated planner").await;
    (org, branch, user)
}

/// `CreatePlan::due_at` is `#[serde(with = "time::serde::rfc3339")]`, while the
/// `OffsetDateTime` default `Serialize` emits the crate's own format. Sending the
/// bare value was rejected by the extractor as a 422 before any handler ran.
fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

fn create_body(fixture: &Fixture, key: &str) -> Value {
    json!({
        "branch_id": fixture.branch,
        "customer_demand_id": fixture.demand,
        "capacity_slot_id": fixture.capacity,
        "material_item_id": fixture.material,
        "quantity": 10,
        "due_at": rfc3339(fixture.due_at),
        "idempotency_key": key,
        "ontology_type_id": fixture.ontology,
    })
}

async fn release_body(pool: &PgPool, fixture: &Fixture, plan: &Value, key: &str) -> Value {
    let approval = Uuid::new_v4();
    let plan_id = plan["id"].as_str().unwrap();
    let digest = plan["plan_digest"].as_str().unwrap();
    sqlx::query("INSERT INTO gov_approvals (id,org_id,request_ref,kind,target_ref,requested_by,approver_id,decision) VALUES ($1,$2,$3,$4,$5,$6,$7,'approved')")
        .bind(approval).bind(*fixture.org.as_uuid()).bind(Uuid::new_v4()).bind(format!("release:v1:{digest}")).bind(Uuid::parse_str(plan_id).unwrap()).bind(*fixture.planner.as_uuid()).bind(*fixture.reviewer.as_uuid()).execute(pool).await.unwrap();
    json!({"expected_version": 1, "approval_ref": approval, "idempotency_key": key})
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn planner_reviewer_operator_complete_a_durable_production_lifecycle(pool: PgPool) {
    let keys = keys();
    let fixture = seed_fixture(&pool).await;
    let service = app(runtime_role_pool(&pool).await, &keys);
    let planner_token = bearer(
        &keys,
        fixture.planner,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let reviewer_token = bearer(
        &keys,
        fixture.reviewer,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let operator_token = bearer(
        &keys,
        fixture.operator,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );

    let body = create_body(&fixture, "production-create-1");
    let (status, plan) = post(
        service.clone(),
        PRODUCTION_PLANS_PATH,
        &planner_token,
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{plan:?}");
    let plan_id = plan["id"].as_str().unwrap();
    let operation_id = plan["first_operation_id"].as_str().unwrap();
    let reserved: i64 =
        sqlx::query_scalar("SELECT reserved_quantity FROM production_capacity_slots WHERE id = $1")
            .bind(fixture.capacity)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(reserved, 10, "the real create handler reserves capacity");
    let material_after_reservation: i64 =
        sqlx::query_scalar("SELECT quantity_on_hand_milli FROM inventory_items WHERE id = $1")
            .bind(fixture.material)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        material_after_reservation, 90,
        "the real create handler reserves the requested material quantity"
    );

    let (status, replay) = post(service.clone(), PRODUCTION_PLANS_PATH, &planner_token, body).await;
    assert_eq!(status, StatusCode::OK, "{replay:?}");
    assert_eq!(replay["id"], plan["id"]);

    let (status, released) = post(
        service.clone(),
        &format!("/api/v1/production/plans/{plan_id}/release"),
        &reviewer_token,
        release_body(&pool, &fixture, &plan, "production-release-1").await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{released:?}");
    assert_eq!(released["status"], "RELEASED");

    let (status, operation) = post(
        service,
        &format!("/api/v1/production/plans/{plan_id}/operations/{operation_id}/records"),
        &operator_token,
        json!({"expected_version": 2, "idempotency_key": "production-record-1", "output_quantity": 10, "scrap_quantity": 0, "downtime_minutes": 0, "quality_evidence_ref": "inspection:passed", "quality_passed": true, "note": "completed"}),
    ).await;
    assert_eq!(status, StatusCode::OK, "{operation:?}");
    assert_eq!(operation["status"], "RECORDED");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn release_requires_the_bound_reviewer_and_consumes_the_approval_once(pool: PgPool) {
    let keys = keys();
    let fixture = seed_fixture(&pool).await;
    let service = app(runtime_role_pool(&pool).await, &keys);
    let planner = bearer(
        &keys,
        fixture.planner,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let reviewer = bearer(
        &keys,
        fixture.reviewer,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let operator = bearer(
        &keys,
        fixture.operator,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let (status, plan) = post(
        service.clone(),
        PRODUCTION_PLANS_PATH,
        &planner,
        create_body(&fixture, "bound-release-create"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{plan:?}");
    let plan_id = plan["id"].as_str().unwrap();
    let body = release_body(&pool, &fixture, &plan, "bound-release").await;
    let uri = format!("/api/v1/production/plans/{plan_id}/release");
    let (status, self_release) = post(service.clone(), &uri, &planner, body.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{self_release:?}");
    let (status, foreign_release) = post(service.clone(), &uri, &operator, body.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{foreign_release:?}");
    let (status, released) = post(service.clone(), &uri, &reviewer, body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{released:?}");
    let (status, replay) = post(service.clone(), &uri, &reviewer, body).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "idempotent replay returns the release: {replay:?}"
    );
    let consumed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM gov_approval_consumptions WHERE consumed_by=$1")
            .bind(*fixture.reviewer.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        consumed, 1,
        "the release approval is single-use and auditable"
    );
    let released_events: i64 = sqlx::query_scalar("SELECT count(*) FROM production_plan_events WHERE plan_id=$1 AND event_type='PLAN_RELEASED'")
        .bind(Uuid::parse_str(plan_id).unwrap()).fetch_one(&pool).await.unwrap();
    assert_eq!(
        released_events, 1,
        "a terminal plan has one release audit event"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_rejects_a_reused_idempotency_key_with_a_different_request(pool: PgPool) {
    let keys = keys();
    let fixture = seed_fixture(&pool).await;
    let service = app(runtime_role_pool(&pool).await, &keys);
    let token = bearer(
        &keys,
        fixture.planner,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let (status, created) = post(
        service.clone(),
        PRODUCTION_PLANS_PATH,
        &token,
        create_body(&fixture, "production-idempotency-conflict"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");

    let mut changed = create_body(&fixture, "production-idempotency-conflict");
    changed["quantity"] = json!(9);
    let (status, conflict) = post(service, PRODUCTION_PLANS_PATH, &token, changed).await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict:?}");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_rejects_a_demand_quantity_or_due_date_that_does_not_match_the_ingested_contract(
    pool: PgPool,
) {
    let keys = keys();
    let fixture = seed_fixture(&pool).await;
    let service = app(runtime_role_pool(&pool).await, &keys);
    let token = bearer(
        &keys,
        fixture.planner,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let mut body = create_body(&fixture, "production-demand-due-mismatch");
    body["due_at"] = json!(rfc3339(fixture.due_at + Duration::days(1)));
    let (status, unavailable) = post(service, PRODUCTION_PLANS_PATH, &token, body).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{unavailable:?}");
    let created: i64 = sqlx::query_scalar("SELECT count(*) FROM production_plans")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        created, 0,
        "a rejected demand contract cannot create a plan"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn drafts_do_not_consume_release_approval_before_review(pool: PgPool) {
    let keys = keys();
    let fixture = seed_fixture(&pool).await;
    let service = app(runtime_role_pool(&pool).await, &keys);
    let token = bearer(
        &keys,
        fixture.planner,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let (status, created) = post(
        service.clone(),
        PRODUCTION_PLANS_PATH,
        &token,
        create_body(&fixture, "production-approval-consume-1"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");

    let (status, second) = post(
        service,
        PRODUCTION_PLANS_PATH,
        &token,
        create_body(&fixture, "production-approval-consume-2"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "same demand and capacity reservation remains unavailable: {second:?}"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn concurrent_tenant_authorized_reservations_do_not_oversubscribe_material_or_capacity(
    pool: PgPool,
) {
    let keys = keys();
    let fixture = seed_fixture(&pool).await;
    let token = bearer(
        &keys,
        fixture.planner,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let second_demand = Uuid::new_v4();
    sqlx::query("INSERT INTO production_demand_contracts (id, org_id, inquiry_id, product_code, quantity, due_at, source_system, source_id, source_version, evaluated_at) SELECT $1, org_id, inquiry_id, product_code, quantity, due_at, source_system, $2, source_version, now() FROM production_demand_contracts WHERE id=$3")
        .bind(second_demand)
        .bind(format!("concurrent-demand-{}", Uuid::new_v4().simple()))
        .bind(fixture.demand)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("UPDATE production_capacity_slots SET available_quantity=10, reserved_quantity=0 WHERE id=$1")
        .bind(fixture.capacity)
        .execute(&pool)
        .await
        .unwrap();
    let first = create_body(&fixture, "production-concurrent-reservation-1");
    let mut second = create_body(&fixture, "production-concurrent-reservation-2");
    second["customer_demand_id"] = json!(second_demand);

    let mut capacity_lock = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM production_capacity_slots WHERE id=$1 FOR UPDATE")
        .bind(fixture.capacity)
        .execute(&mut *capacity_lock)
        .await
        .unwrap();
    let service = app(runtime_role_pool(&pool).await, &keys);
    let first_service = service.clone();
    let first_token = token.clone();
    let first_request = tokio::spawn(async move {
        post(first_service, PRODUCTION_PLANS_PATH, &first_token, first).await
    });
    let second_request =
        tokio::spawn(async move { post(service, PRODUCTION_PLANS_PATH, &token, second).await });

    let mut both_reservations_blocked = false;
    for _ in 0..1_000 {
        let blocked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock' AND (query LIKE '%production_capacity_slots%' OR query LIKE '%inventory_items%')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if blocked >= 2 {
            both_reservations_blocked = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    capacity_lock.commit().await.unwrap();
    assert!(
        both_reservations_blocked,
        "both tenant-authorized reservations must reach the database lock boundary"
    );

    let (first_status, first_body) = first_request.await.unwrap();
    let (second_status, second_body) = second_request.await.unwrap();
    let mut statuses = [first_status, second_status];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::CREATED, StatusCode::SERVICE_UNAVAILABLE],
        "only one reservation may succeed without a generic server failure: {first_body:?}, {second_body:?}"
    );
    let reserved: i64 =
        sqlx::query_scalar("SELECT reserved_quantity FROM production_capacity_slots WHERE id=$1")
            .bind(fixture.capacity)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(reserved, 10, "capacity cannot be oversubscribed");
    let on_hand: i64 =
        sqlx::query_scalar("SELECT quantity_on_hand_milli FROM inventory_items WHERE id=$1")
            .bind(fixture.material)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(on_hand, 90, "material cannot be oversubscribed");
    let hashes: Vec<String> = sqlx::query_scalar("SELECT request_hash FROM production_idempotency_claims WHERE org_id=$1 AND operation='CREATE_PLAN' AND idempotency_key = ANY($2)")
        .bind(*fixture.org.as_uuid())
        .bind(vec!["production-concurrent-reservation-1", "production-concurrent-reservation-2"])
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        hashes.len(),
        1,
        "only the committed reservation retains its idempotency claim"
    );
    assert!(
        hashes[0].len() == 64
            && hashes[0]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "the committed tenant-authorized reservation must retain its exact SHA-256 request hash"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn operation_record_rejects_a_terminal_operation_write(pool: PgPool) {
    let keys = keys();
    let fixture = seed_fixture(&pool).await;
    let service = app(runtime_role_pool(&pool).await, &keys);
    let planner_token = bearer(
        &keys,
        fixture.planner,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let reviewer_token = bearer(
        &keys,
        fixture.reviewer,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let operator_token = bearer(
        &keys,
        fixture.operator,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let (status, plan) = post(
        service.clone(),
        PRODUCTION_PLANS_PATH,
        &planner_token,
        create_body(&fixture, "production-terminal-create"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{plan:?}");
    let plan_id = plan["id"].as_str().unwrap();
    let operation_id = plan["first_operation_id"].as_str().unwrap();
    let (status, release) = post(
        service.clone(),
        &format!("/api/v1/production/plans/{plan_id}/release"),
        &reviewer_token,
        release_body(&pool, &fixture, &plan, "production-terminal-release").await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{release:?}");
    let request = json!({"expected_version": 2, "idempotency_key": "production-terminal-record-1", "output_quantity": 10, "scrap_quantity": 0, "downtime_minutes": 0, "quality_evidence_ref": "inspection:passed", "quality_passed": true, "note": "completed"});
    let (status, recorded) = post(
        service.clone(),
        &format!("/api/v1/production/plans/{plan_id}/operations/{operation_id}/records"),
        &operator_token,
        request,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{recorded:?}");
    let (status, conflict) = post(
        service,
        &format!("/api/v1/production/plans/{plan_id}/operations/{operation_id}/records"),
        &operator_token,
        json!({"expected_version": 3, "idempotency_key": "production-terminal-record-2", "output_quantity": 1, "scrap_quantity": 0, "downtime_minutes": 0, "quality_evidence_ref": "inspection:passed", "quality_passed": true, "note": "illegal second write"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict:?}");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn tenant_cannot_read_another_tenants_production_plan_under_force_rls(pool: PgPool) {
    let keys = keys();
    let fixture = seed_fixture(&pool).await;
    let service = app(runtime_role_pool(&pool).await, &keys);
    let planner_token = bearer(
        &keys,
        fixture.planner,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let (status, created) = post(
        service.clone(),
        PRODUCTION_PLANS_PATH,
        &planner_token,
        create_body(&fixture, "production-cross-tenant-create"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let (foreign_org, foreign_branch, foreign_user) = seed_isolated_tenant(&pool).await;
    let foreign_token = bearer(
        &keys,
        foreign_user,
        foreign_org,
        "SUPER_ADMIN",
        foreign_branch,
    );
    let (status, body) = get(
        service,
        &PRODUCTION_PLAN_PATH.replace("{plan_id}", created["id"].as_str().unwrap()),
        &foreign_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:?}");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn production_reads_deny_roles_without_daily_plan_request_or_review(pool: PgPool) {
    let keys = keys();
    let fixture = seed_fixture(&pool).await;
    let mechanic = seed_user(
        &pool,
        fixture.org,
        fixture.branch,
        "RECEPTIONIST",
        "production-read-denied",
    )
    .await;
    let service = app(runtime_role_pool(&pool).await, &keys);
    let token = bearer(&keys, mechanic, fixture.org, "RECEPTIONIST", fixture.branch);
    let (status, body) = get(
        service.clone(),
        &format!("{PRODUCTION_PLANS_PATH}?branch_id={}", fixture.branch),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:?}");
    let (status, body) = get(
        service,
        &format!(
            "{PRODUCTION_CAPACITY_SLOTS_PATH}?branch_id={}&capacity_date={}",
            fixture.branch,
            fixture.due_at.date()
        ),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body:?}");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn runtime_routes_reject_caller_authored_demand_and_capacity_ingress(pool: PgPool) {
    let keys = keys();
    let fixture = seed_fixture(&pool).await;
    let service = app(runtime_role_pool(&pool).await, &keys);
    let token = bearer(
        &keys,
        fixture.planner,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let (status, capacity) = post(
        service.clone(),
        PRODUCTION_CAPACITY_SLOTS_PATH,
        &token,
        json!({"branch_id": fixture.branch, "available_quantity": 999999}),
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{capacity:?}");
    let (status, demand) = post(
        service,
        "/api/v1/production/demand-contracts",
        &token,
        json!({"product_code": "IV-PROD-001", "quantity": 999999}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{demand:?}");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn human_reviewer_cannot_assert_production_source_truth(pool: PgPool) {
    let keys = keys();
    let fixture = seed_fixture(&pool).await;
    let service = app(runtime_role_pool(&pool).await, &keys);
    let token = bearer(
        &keys,
        fixture.reviewer,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let body = json!({
        "kind": "material", "branch_id": fixture.branch, "material_item_id": fixture.material,
        "quantity_on_hand_milli": 120, "safety_stock_milli": 5,
        "source_system": "erp", "source_id": "material-1", "source_version": "v2"
    });
    let (status, denied) = post(
        service.clone(),
        PRODUCTION_SOURCE_INGRESS_PATH,
        &token,
        body.clone(),
    )
    .await;
    // The ingress route is machine-Basic-only and deliberately sits outside the JWT
    // middleware (lib.rs `router`), so a human bearer token is not a credential it can
    // even parse. `parse_basic_credentials` collapses every non-Basic and malformed
    // form to one indistinguishable 401 on purpose — telling a caller that its
    // credential *form* was recognised but rejected is the probe that comment forbids.
    // 403 was never reachable here; the denial this test exists to prove is the zero
    // audit rows below.
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{denied:?}");
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM production_source_ingress_claims WHERE org_id=$1 AND kind='MATERIAL'",
    )
    .bind(*fixture.org.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        audit_count, 0,
        "a human reviewer creates no source ingress audit row"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn source_system_lifecycle_uses_typed_credentials_and_generation_cas(pool: PgPool) {
    let keys = keys();
    let fixture = seed_fixture(&pool).await;
    let service = app(runtime_role_pool(&pool).await, &keys);
    let token = bearer(
        &keys,
        fixture.planner,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );

    let (status, registered) = post(
        service.clone(),
        PRODUCTION_SOURCE_SYSTEMS_PATH,
        &token,
        json!({"branch_id": fixture.branch, "source_system": "production-erp"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{registered:?}");
    assert_eq!(registered["source_system"], "production-erp");
    assert_eq!(registered["enabled"], true);
    assert_eq!(registered["credential_generation"], 1);
    assert!(registered.get("verifier").is_none());
    assert!(registered.get("mac").is_none());
    let registered_secret = registered["secret"].as_str().expect("credential secret");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(registered_secret)
            .unwrap()
            .len(),
        32
    );
    let principal_id = registered["id"].as_str().expect("principal id");

    let rotate_uri =
        PRODUCTION_SOURCE_SYSTEM_ROTATE_PATH.replace("{source_system_id}", principal_id);
    let first_service = service.clone();
    let first_token = token.clone();
    let first_rotate_uri = rotate_uri.clone();
    let first = tokio::spawn(async move {
        post(
            first_service,
            &first_rotate_uri,
            &first_token,
            json!({"expected_generation": 1}),
        )
        .await
    });
    let second_service = service.clone();
    let second_token = token.clone();
    let second = tokio::spawn(async move {
        post(
            second_service,
            &rotate_uri,
            &second_token,
            json!({"expected_generation": 1}),
        )
        .await
    });
    let (first_status, first_body) = first.await.unwrap();
    let (second_status, second_body) = second.await.unwrap();
    let mut statuses = [first_status, second_status];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::OK, StatusCode::CONFLICT],
        "{first_body:?} {second_body:?}"
    );
    let rotated = if first_status == StatusCode::OK {
        first_body
    } else {
        second_body
    };
    let rotated_secret = rotated["secret"]
        .as_str()
        .expect("rotated credential secret");
    assert_ne!(
        rotated_secret, registered_secret,
        "rotation must disclose a new one-time credential"
    );
    assert_eq!(rotated["credential_generation"], 2);
    assert!(rotated.get("verifier").is_none());
    assert!(rotated.get("mac").is_none());

    let disable_uri =
        PRODUCTION_SOURCE_SYSTEM_DISABLE_PATH.replace("{source_system_id}", principal_id);
    let first_service = service.clone();
    let first_token = token.clone();
    let first_disable_uri = disable_uri.clone();
    let first = tokio::spawn(async move {
        post(
            first_service,
            &first_disable_uri,
            &first_token,
            json!({"expected_generation": 2}),
        )
        .await
    });
    let second_service = service.clone();
    let second_token = token.clone();
    let second = tokio::spawn(async move {
        post(
            second_service,
            &disable_uri,
            &second_token,
            json!({"expected_generation": 2}),
        )
        .await
    });
    let (first_status, first_body) = first.await.unwrap();
    let (second_status, second_body) = second.await.unwrap();
    let mut statuses = [first_status, second_status];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::OK, StatusCode::CONFLICT],
        "{first_body:?} {second_body:?}"
    );
    let disabled = if first_status == StatusCode::OK {
        first_body
    } else {
        second_body
    };
    assert_eq!(disabled["enabled"], false);
    assert_eq!(disabled["credential_generation"], 2);
    assert!(disabled.get("secret").is_none());
    assert!(disabled.get("verifier").is_none());
    assert!(disabled.get("mac").is_none());

    let id = Uuid::parse_str(principal_id).unwrap();
    let state: String = sqlx::query_scalar("SELECT state FROM service_principals WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "DISABLED");
    let events: Vec<(String, Option<i32>, i32)> = sqlx::query_as(
        "SELECT event_type,expected_generation,resulting_generation FROM service_principal_audit_events WHERE service_principal_id=$1 ORDER BY occurred_at,id",
    )
    .bind(id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        events,
        vec![
            ("REGISTERED".to_owned(), None, 1),
            ("ROTATED".to_owned(), Some(1), 2),
            ("DISABLED".to_owned(), Some(2), 2)
        ]
    );
    let generic_actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_events WHERE org_id=$1 AND target_type='production_source_system' AND target_id=$2 ORDER BY occurred_at,id",
    )
    .bind(*fixture.org.as_uuid())
    .bind(principal_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        generic_actions,
        vec![
            "production.source_system_register".to_owned(),
            "production.source_system_rotate".to_owned(),
            "production.source_system_disable".to_owned(),
        ],
        "credential lifecycle mutations must commit their platform audit rows atomically",
    );
    let persisted_secret_or_verifier: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM service_principal_audit_events WHERE service_principal_id=$1 AND (row_to_json(service_principal_audit_events)::text LIKE '%' || $2 || '%' OR row_to_json(service_principal_audit_events)::text LIKE '%verifier%' OR row_to_json(service_principal_audit_events)::text LIKE '%mac%')",
    )
    .bind(id)
    .bind(rotated_secret)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        persisted_secret_or_verifier, 0,
        "credential material cannot enter the audit stream"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn source_system_lifecycle_rejects_same_branch_caller_without_role_manage(pool: PgPool) {
    let keys = keys();
    let fixture = seed_fixture(&pool).await;
    let service = app(runtime_role_pool(&pool).await, &keys);
    let manager_token = bearer(
        &keys,
        fixture.planner,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let no_role_manage = seed_user(
        &pool,
        fixture.org,
        fixture.branch,
        "ADMIN",
        "lifecycle non-manager",
    )
    .await;
    let denied_token = bearer(&keys, no_role_manage, fixture.org, "ADMIN", fixture.branch);
    let (registered_status, registered) = post(
        service.clone(),
        PRODUCTION_SOURCE_SYSTEMS_PATH,
        &manager_token,
        json!({"branch_id": fixture.branch, "source_system": "restricted-erp"}),
    )
    .await;
    assert_eq!(registered_status, StatusCode::CREATED, "{registered:?}");
    let id = Uuid::parse_str(registered["id"].as_str().expect("principal id")).unwrap();
    let before: (Vec<u8>, i32, String) =
        sqlx::query_as("SELECT verifier,generation,state FROM service_principals WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let before_audit_rows: Vec<LifecycleAuditRow> = sqlx::query_as(
        "SELECT id,org_id,service_principal_id,event_type,actor_id,expected_generation,resulting_generation,occurred_at FROM service_principal_audit_events WHERE service_principal_id=$1 ORDER BY occurred_at,id",
    )
    .bind(id)
    .fetch_all(&pool)
    .await
    .unwrap();

    let rotate_uri =
        PRODUCTION_SOURCE_SYSTEM_ROTATE_PATH.replace("{source_system_id}", &id.to_string());
    let (rotate_status, rotate_body) = post(
        service.clone(),
        &rotate_uri,
        &denied_token,
        json!({"expected_generation": 1}),
    )
    .await;
    assert_eq!(rotate_status, StatusCode::FORBIDDEN, "{rotate_body:?}");

    let disable_uri =
        PRODUCTION_SOURCE_SYSTEM_DISABLE_PATH.replace("{source_system_id}", &id.to_string());
    let (disable_status, disable_body) = post(
        service,
        &disable_uri,
        &denied_token,
        json!({"expected_generation": 1}),
    )
    .await;
    assert_eq!(disable_status, StatusCode::FORBIDDEN, "{disable_body:?}");

    let after: (Vec<u8>, i32, String) =
        sqlx::query_as("SELECT verifier,generation,state FROM service_principals WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let after_audit_rows: Vec<LifecycleAuditRow> = sqlx::query_as(
        "SELECT id,org_id,service_principal_id,event_type,actor_id,expected_generation,resulting_generation,occurred_at FROM service_principal_audit_events WHERE service_principal_id=$1 ORDER BY occurred_at,id",
    )
    .bind(id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(after, before);
    assert_eq!(after_audit_rows, before_audit_rows);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn source_system_lifecycle_cannot_mutate_a_same_branch_other_feature_principal(pool: PgPool) {
    let keys = keys();
    let fixture = seed_fixture(&pool).await;
    let service = app(runtime_role_pool(&pool).await, &keys);
    let token = bearer(
        &keys,
        fixture.planner,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );

    // The current migration has only one machine feature. Remove that schema
    // restriction in this disposable database so this regression continues to
    // prove lifecycle endpoints remain scoped when future features are added.
    sqlx::query("ALTER TABLE service_principals DROP CONSTRAINT service_principals_feature_check")
        .execute(&pool)
        .await
        .unwrap();
    let other_principal = Uuid::new_v4();
    let other_feature_verifier = [9_u8; 32];
    sqlx::query("INSERT INTO service_principals (id,org_id,branch_id,feature,display_name,verifier,created_by) VALUES ($1,$2,$3,$4,$5,$6,$7)")
        .bind(other_principal)
        .bind(*fixture.org.as_uuid())
        .bind(*fixture.branch.as_uuid())
        .bind("other_machine_feature")
        .bind("Other machine principal")
        .bind(other_feature_verifier.as_slice())
        .bind(*fixture.planner.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    let rotate_uri = PRODUCTION_SOURCE_SYSTEM_ROTATE_PATH
        .replace("{source_system_id}", &other_principal.to_string());
    let (rotate_status, rotate_body) = post(
        service.clone(),
        &rotate_uri,
        &token,
        json!({"expected_generation": 1}),
    )
    .await;
    assert_eq!(rotate_status, StatusCode::NOT_FOUND, "{rotate_body:?}");

    let disable_uri = PRODUCTION_SOURCE_SYSTEM_DISABLE_PATH
        .replace("{source_system_id}", &other_principal.to_string());
    let (disable_status, disable_body) = post(
        service,
        &disable_uri,
        &token,
        json!({"expected_generation": 1}),
    )
    .await;
    assert_eq!(disable_status, StatusCode::NOT_FOUND, "{disable_body:?}");

    let (feature, state, generation): (String, String, i32) =
        sqlx::query_as("SELECT feature,state,generation FROM service_principals WHERE id=$1")
            .bind(other_principal)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(feature, "other_machine_feature");
    assert_eq!(state, "ACTIVE");
    assert_eq!(generation, 1);
    let audit_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM service_principal_audit_events WHERE service_principal_id=$1",
    )
    .bind(other_principal)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        audit_events, 0,
        "other feature must receive no lifecycle audit"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn capacity_ingress_receipt_uses_the_persisted_natural_key_row(pool: PgPool) {
    let keys = keys();
    let fixture = seed_fixture(&pool).await;
    let service = app(runtime_role_pool(&pool).await, &keys);
    let token = bearer(
        &keys,
        fixture.planner,
        fixture.org,
        "SUPER_ADMIN",
        fixture.branch,
    );
    let (register_status, registered) = post(
        service.clone(),
        PRODUCTION_SOURCE_SYSTEMS_PATH,
        &token,
        json!({"branch_id": fixture.branch, "source_system": "capacity-sync"}),
    )
    .await;
    assert_eq!(register_status, StatusCode::CREATED, "{registered:?}");
    let principal_id = registered["id"].as_str().expect("principal id");
    let authorization = source_authorization(
        principal_id,
        registered["secret"].as_str().expect("source secret"),
    );

    let first_input_id = Uuid::new_v4();
    let (first_status, first_receipt) = post_source(
        service.clone(),
        PRODUCTION_SOURCE_INGRESS_PATH,
        authorization.clone(),
        json!({
            "kind": "capacity",
            "id": first_input_id,
            "site_id": fixture.site,
            "capacity_date": fixture.due_at.date(),
            "available_quantity": 120,
            "source_id": "capacity-sync-1",
            "source_version": "v1"
        }),
    )
    .await;
    assert_eq!(first_status, StatusCode::OK, "{first_receipt:?}");
    assert_eq!(first_receipt["id"], fixture.capacity.to_string());

    let second_input_id = Uuid::new_v4();
    let (second_status, second_receipt) = post_source(
        service,
        PRODUCTION_SOURCE_INGRESS_PATH,
        authorization,
        json!({
            "kind": "capacity",
            "id": second_input_id,
            "site_id": fixture.site,
            "capacity_date": fixture.due_at.date(),
            "available_quantity": 140,
            "source_id": "capacity-sync-2",
            "source_version": "v2"
        }),
    )
    .await;
    assert_eq!(second_status, StatusCode::OK, "{second_receipt:?}");
    assert_eq!(second_receipt["id"], fixture.capacity.to_string());
    assert_ne!(first_input_id, fixture.capacity);
    assert_ne!(second_input_id, fixture.capacity);

    let (persisted_id, available_quantity): (Uuid, i64) = sqlx::query_as(
        "SELECT id,available_quantity FROM production_capacity_slots WHERE org_id=$1 AND branch_id=$2 AND site_id=$3 AND capacity_date=$4",
    )
    .bind(*fixture.org.as_uuid())
    .bind(*fixture.branch.as_uuid())
    .bind(fixture.site)
    .bind(fixture.due_at.date())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(persisted_id, fixture.capacity);
    assert_eq!(available_quantity, 140);
}
