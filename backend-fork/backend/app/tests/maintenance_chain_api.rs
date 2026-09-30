#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! CAP-MAINTENANCE-CONSOLE runtime proof: the maintenance story chain
//! (request → assign → execute → report → cost settlement → close) driven
//! through the deployed router on a genuine non-owner `console_rt` pool, so FORCE
//! RLS, PBAC denial without leakage, cross-tenant isolation, idempotent
//! settlement creation, four-eyes review, and audit readback are all proved
//! against the real enforcement path.

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
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const IDEMPOTENCY_KEY: &str = "maintenance-settlement-0001";

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn settlement_chain_closes_order_into_cost_with_audit_readback(owner_pool: PgPool) {
    let keys = Keys::generate();
    let branch_id = seed_branch(&owner_pool, "정비 Region", "정비 Branch").await;
    let admin_id = UserId::new();
    let mechanic_id = UserId::new();
    seed_user(&owner_pool, OrgId::knl(), admin_id, "ADMIN", branch_id).await;
    seed_user(
        &owner_pool,
        OrgId::knl(),
        mechanic_id,
        "MECHANIC",
        branch_id,
    )
    .await;
    seed_equipment(&owner_pool, branch_id, "2643").await;
    let admin = keys.token(admin_id, OrgId::knl(), "ADMIN", vec![branch_id]);
    let mechanic = keys.token(mechanic_id, OrgId::knl(), "MECHANIC", vec![branch_id]);
    let service = build_router(app_state(
        console_rt_pool(&owner_pool).await,
        keys.public_pem.clone(),
    ));

    let work_order_id = drive_to_report(
        &service,
        &admin,
        &mechanic,
        mechanic_id,
        branch_id,
        "2643",
        "EMERGENCY",
        "BREAKDOWN",
    )
    .await;

    // A settlement draft without the Idempotency-Key header is fail-closed.
    let missing_key = send(
        &service,
        "POST",
        &format!("/api/v1/work-orders/{work_order_id}/settlement"),
        &mechanic,
        Some(settlement_body()),
        None,
    )
    .await;
    assert_eq!(missing_key.status, StatusCode::UNPROCESSABLE_ENTITY);

    // Mechanic (assigned to this order) drafts the settlement.
    let created = send(
        &service,
        "POST",
        &format!("/api/v1/work-orders/{work_order_id}/settlement"),
        &mechanic,
        Some(settlement_body()),
        Some(IDEMPOTENCY_KEY),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.json["status"], "DRAFT");
    assert_eq!(created.json["total_amount_krw"], 200_000);
    assert_eq!(created.json["lines"].as_array().unwrap().len(), 2);
    assert_eq!(created.json["lines"][1]["source_ref"], "PO-121");
    let settlement_id = created.json["id"].as_str().unwrap().to_owned();

    // Replaying the identical request under the same key returns the same
    // settlement without a second audit event.
    let replay = send(
        &service,
        "POST",
        &format!("/api/v1/work-orders/{work_order_id}/settlement"),
        &mechanic,
        Some(settlement_body()),
        Some(IDEMPOTENCY_KEY),
    )
    .await;
    assert_eq!(replay.status, StatusCode::CREATED);
    assert_eq!(replay.json["id"], settlement_id.as_str());
    assert_eq!(
        audit_count(&owner_pool, "work_order_settlement.create").await,
        1,
        "an idempotent replay must not append a second create audit event"
    );

    // The same key with a different request body conflicts.
    let mut different = settlement_body();
    different["note"] = json!("changed");
    let conflict = send(
        &service,
        "POST",
        &format!("/api/v1/work-orders/{work_order_id}/settlement"),
        &mechanic,
        Some(different),
        Some(IDEMPOTENCY_KEY),
    )
    .await;
    assert_eq!(conflict.status, StatusCode::CONFLICT);

    // Creator submits; a reviewer-featureless mechanic cannot approve.
    let submitted = send(
        &service,
        "POST",
        &format!("/api/v1/settlements/{settlement_id}/submit"),
        &mechanic,
        Some(json!({})),
        None,
    )
    .await;
    assert_eq!(submitted.status, StatusCode::OK);
    assert_eq!(submitted.json["status"], "SUBMITTED");
    let denied_review = send(
        &service,
        "POST",
        &format!("/api/v1/settlements/{settlement_id}/review"),
        &mechanic,
        Some(json!({ "decision": "APPROVED" })),
        None,
    )
    .await;
    assert_eq!(denied_review.status, StatusCode::FORBIDDEN);

    // RETURNED without a comment is fail-closed; APPROVED closes into cost.
    let return_without_comment = send(
        &service,
        "POST",
        &format!("/api/v1/settlements/{settlement_id}/review"),
        &admin,
        Some(json!({ "decision": "RETURNED", "comment": "  " })),
        None,
    )
    .await;
    assert_eq!(
        return_without_comment.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let approved = send(
        &service,
        "POST",
        &format!("/api/v1/settlements/{settlement_id}/review"),
        &admin,
        Some(json!({ "decision": "APPROVED", "comment": "정산 승인" })),
        None,
    )
    .await;
    assert_eq!(approved.status, StatusCode::OK);
    assert_eq!(approved.json["status"], "APPROVED");
    assert_eq!(approved.json["approved_by"], admin_id.to_string());

    // The order detail carries its classification and live settlement.
    let detail = send(
        &service,
        "GET",
        &format!("/api/v1/work-orders/{work_order_id}"),
        &admin,
        None,
        None,
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK);
    assert_eq!(detail.json["maintenance_type"], "EMERGENCY");
    assert_eq!(detail.json["maintenance_cause"], "BREAKDOWN");
    assert_eq!(detail.json["settlement"]["id"], settlement_id.as_str());
    assert_eq!(detail.json["settlement"]["status"], "APPROVED");

    // Audit readback: every settlement lifecycle step left exactly one event.
    for (action, expected) in [
        ("work_order_settlement.create", 1_i64),
        ("work_order_settlement.submit", 1),
        ("work_order_settlement.review", 1),
    ] {
        assert_eq!(
            audit_count(&owner_pool, action).await,
            expected,
            "audit action {action}"
        );
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn settlement_enforces_eligibility_four_eyes_and_void_discipline(owner_pool: PgPool) {
    let keys = Keys::generate();
    let branch_id = seed_branch(&owner_pool, "정산 Region", "정산 Branch").await;
    let admin_a = UserId::new();
    let admin_b = UserId::new();
    let mechanic_id = UserId::new();
    seed_user(&owner_pool, OrgId::knl(), admin_a, "ADMIN", branch_id).await;
    seed_user(&owner_pool, OrgId::knl(), admin_b, "ADMIN", branch_id).await;
    seed_user(
        &owner_pool,
        OrgId::knl(),
        mechanic_id,
        "MECHANIC",
        branch_id,
    )
    .await;
    seed_equipment(&owner_pool, branch_id, "2641").await;
    let token_a = keys.token(admin_a, OrgId::knl(), "ADMIN", vec![branch_id]);
    let token_b = keys.token(admin_b, OrgId::knl(), "ADMIN", vec![branch_id]);
    let mechanic = keys.token(mechanic_id, OrgId::knl(), "MECHANIC", vec![branch_id]);
    let service = build_router(app_state(
        console_rt_pool(&owner_pool).await,
        keys.public_pem.clone(),
    ));

    // A settlement cannot be opened before the report exists.
    let received = send(
        &service,
        "POST",
        "/api/work-orders",
        &token_a,
        Some(json!({
            "branch_id": branch_id,
            "management_no": "#2641",
            "symptom": "교체 정비 요청",
            "maintenance_type": "CORRECTIVE",
            "maintenance_cause": "RETURN_PREP"
        })),
        None,
    )
    .await;
    assert_eq!(received.status, StatusCode::CREATED);
    let premature_id = received.json["id"].as_str().unwrap().to_owned();
    let premature = send(
        &service,
        "POST",
        &format!("/api/v1/work-orders/{premature_id}/settlement"),
        &token_a,
        Some(settlement_body()),
        Some("premature-settlement-001"),
    )
    .await;
    assert_eq!(premature.status, StatusCode::CONFLICT);

    let work_order_id = drive_to_report(
        &service,
        &token_a,
        &mechanic,
        mechanic_id,
        branch_id,
        "2641",
        "CORRECTIVE",
        "RETURN_PREP",
    )
    .await;
    let created = send(
        &service,
        "POST",
        &format!("/api/v1/work-orders/{work_order_id}/settlement"),
        &token_a,
        Some(settlement_body()),
        Some("four-eyes-settlement-01"),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let settlement_id = created.json["id"].as_str().unwrap().to_owned();
    let submitted = send(
        &service,
        "POST",
        &format!("/api/v1/settlements/{settlement_id}/submit"),
        &token_a,
        Some(json!({})),
        None,
    )
    .await;
    assert_eq!(submitted.status, StatusCode::OK);

    // Four-eyes: the submitter cannot approve their own settlement.
    let self_review = send(
        &service,
        "POST",
        &format!("/api/v1/settlements/{settlement_id}/review"),
        &token_a,
        Some(json!({ "decision": "APPROVED" })),
        None,
    )
    .await;
    assert_eq!(self_review.status, StatusCode::FORBIDDEN);

    // A second reviewer returns it (comment required) back to DRAFT.
    let returned = send(
        &service,
        "POST",
        &format!("/api/v1/settlements/{settlement_id}/review"),
        &token_b,
        Some(json!({ "decision": "RETURNED", "comment": "외주비 근거 누락" })),
        None,
    )
    .await;
    assert_eq!(returned.status, StatusCode::OK);
    assert_eq!(returned.json["status"], "DRAFT");
    assert_eq!(returned.json["submitted_by"], Value::Null);

    // Void: mechanics are denied, an empty reason is fail-closed, an admin
    // with a reason voids — freeing the one-live-settlement slot.
    let mechanic_void = send(
        &service,
        "POST",
        &format!("/api/v1/settlements/{settlement_id}/void"),
        &mechanic,
        Some(json!({ "reason": "should not work" })),
        None,
    )
    .await;
    assert_eq!(mechanic_void.status, StatusCode::FORBIDDEN);
    let empty_reason = send(
        &service,
        "POST",
        &format!("/api/v1/settlements/{settlement_id}/void"),
        &token_b,
        Some(json!({ "reason": "   " })),
        None,
    )
    .await;
    assert_eq!(empty_reason.status, StatusCode::UNPROCESSABLE_ENTITY);
    let voided = send(
        &service,
        "POST",
        &format!("/api/v1/settlements/{settlement_id}/void"),
        &token_b,
        Some(json!({ "reason": "이중 기안 정리" })),
        None,
    )
    .await;
    assert_eq!(voided.status, StatusCode::OK);
    assert_eq!(voided.json["status"], "VOID");
    let recreated = send(
        &service,
        "POST",
        &format!("/api/v1/work-orders/{work_order_id}/settlement"),
        &token_a,
        Some(settlement_body()),
        Some("four-eyes-settlement-02"),
    )
    .await;
    assert_eq!(recreated.status, StatusCode::CREATED);
    assert_eq!(
        audit_count(&owner_pool, "work_order_settlement.void").await,
        1
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn pbac_denies_and_cross_tenant_reads_are_isolated_without_leakage(owner_pool: PgPool) {
    let keys = Keys::generate();
    let branch_id = seed_branch(&owner_pool, "격리 Region", "격리 Branch").await;
    let admin_id = UserId::new();
    let mechanic_id = UserId::new();
    let member_id = UserId::new();
    seed_user(&owner_pool, OrgId::knl(), admin_id, "ADMIN", branch_id).await;
    seed_user(
        &owner_pool,
        OrgId::knl(),
        mechanic_id,
        "MECHANIC",
        branch_id,
    )
    .await;
    seed_user(&owner_pool, OrgId::knl(), member_id, "MEMBER", branch_id).await;
    seed_equipment(&owner_pool, branch_id, "2638").await;
    let other_org = seed_other_org(&owner_pool).await;
    let outsider_id = UserId::new();
    let other_branch = seed_other_org_branch(&owner_pool, other_org).await;
    seed_user(&owner_pool, other_org, outsider_id, "ADMIN", other_branch).await;

    let admin = keys.token(admin_id, OrgId::knl(), "ADMIN", vec![branch_id]);
    let mechanic = keys.token(mechanic_id, OrgId::knl(), "MECHANIC", vec![branch_id]);
    let member = keys.token(member_id, OrgId::knl(), "MEMBER", vec![branch_id]);
    let outsider = keys.token(outsider_id, other_org, "ADMIN", vec![other_branch]);
    let service = build_router(app_state(
        console_rt_pool(&owner_pool).await,
        keys.public_pem.clone(),
    ));

    let work_order_id = drive_to_report(
        &service,
        &admin,
        &mechanic,
        mechanic_id,
        branch_id,
        "2638",
        "PREVENTIVE",
        "SCHEDULED",
    )
    .await;
    let created = send(
        &service,
        "POST",
        &format!("/api/v1/work-orders/{work_order_id}/settlement"),
        &admin,
        Some(settlement_body()),
        Some("isolation-settlement-01"),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);

    // One live settlement per order: a second draft under a DIFFERENT key hits
    // the partial unique index and surfaces as 409, not a 500.
    let duplicate_live = send(
        &service,
        "POST",
        &format!("/api/v1/work-orders/{work_order_id}/settlement"),
        &admin,
        Some(settlement_body()),
        Some("isolation-settlement-02"),
    )
    .await;
    assert_eq!(duplicate_live.status, StatusCode::CONFLICT);

    // Deny-by-default: a MEMBER principal gets 403 with the canonical error
    // envelope and no object data.
    for (method, path) in [
        ("GET", "/api/v1/work-orders".to_owned()),
        (
            "GET",
            format!("/api/v1/work-orders/{work_order_id}/settlement"),
        ),
    ] {
        let denied = send(&service, method, &path, &member, None, None).await;
        assert_eq!(denied.status, StatusCode::FORBIDDEN, "{method} {path}");
        assert!(denied.json["error"]["message"].is_string());
        assert!(
            denied.json.get("id").is_none() && denied.json.get("lines").is_none(),
            "a denial must not carry object fields"
        );
    }
    let member_create = send(
        &service,
        "POST",
        &format!("/api/v1/work-orders/{work_order_id}/settlement"),
        &member,
        Some(settlement_body()),
        Some("member-denied-settlement"),
    )
    .await;
    assert_eq!(member_create.status, StatusCode::FORBIDDEN);

    // Cross-tenant: to another org's principal the knl order does not exist —
    // 404, never 403, so tenancy reveals nothing about foreign objects.
    for path in [
        format!("/api/v1/work-orders/{work_order_id}"),
        format!("/api/v1/work-orders/{work_order_id}/settlement"),
    ] {
        let hidden = send(&service, "GET", &path, &outsider, None, None).await;
        assert_eq!(hidden.status, StatusCode::NOT_FOUND, "GET {path}");
    }
    let cross_create = send(
        &service,
        "POST",
        &format!("/api/v1/work-orders/{work_order_id}/settlement"),
        &outsider,
        Some(settlement_body()),
        Some("cross-tenant-settlement"),
    )
    .await;
    assert_eq!(cross_create.status, StatusCode::NOT_FOUND);
    let outsider_list = send(
        &service,
        "GET",
        "/api/v1/work-orders",
        &outsider,
        None,
        None,
    )
    .await;
    assert_eq!(outsider_list.status, StatusCode::OK);
    assert_eq!(outsider_list.json["total"], 0);
    assert_eq!(outsider_list.json["items"].as_array().unwrap().len(), 0);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn lens_and_filters_expose_maintenance_classification_truthfully(owner_pool: PgPool) {
    let keys = Keys::generate();
    let branch_id = seed_branch(&owner_pool, "렌즈 Region", "렌즈 Branch").await;
    let admin_id = UserId::new();
    let mechanic_id = UserId::new();
    seed_user(&owner_pool, OrgId::knl(), admin_id, "ADMIN", branch_id).await;
    seed_user(
        &owner_pool,
        OrgId::knl(),
        mechanic_id,
        "MECHANIC",
        branch_id,
    )
    .await;
    let emergency_equipment = seed_equipment(&owner_pool, branch_id, "3101").await;
    let preventive_equipment = seed_equipment(&owner_pool, branch_id, "3102").await;
    let admin = keys.token(admin_id, OrgId::knl(), "ADMIN", vec![branch_id]);
    let mechanic = keys.token(mechanic_id, OrgId::knl(), "MECHANIC", vec![branch_id]);
    let service = build_router(app_state(
        console_rt_pool(&owner_pool).await,
        keys.public_pem.clone(),
    ));

    let create = send(
        &service,
        "POST",
        "/api/work-orders",
        &admin,
        Some(json!({
            "branch_id": branch_id,
            "management_no": "#3102",
            "symptom": "정기 점검",
            "maintenance_type": "PREVENTIVE",
            "maintenance_cause": "SCHEDULED"
        })),
        None,
    )
    .await;
    assert_eq!(create.status, StatusCode::CREATED);
    assert_eq!(create.json["maintenance_type"], "PREVENTIVE");
    assert_eq!(create.json["maintenance_cause"], "SCHEDULED");

    // Before any execution: classification facets are truthful and the derived
    // aggregates are null, never fabricated zeros.
    let initial = send(&service, "GET", "/api/v1/work-orders", &admin, None, None).await;
    assert_eq!(initial.status, StatusCode::OK);
    assert_eq!(
        initial.json["lens"]["aggregates"]["preventive_on_time_rate"],
        Value::Null
    );
    assert_eq!(
        initial.json["lens"]["aggregates"]["mttr_minutes"],
        Value::Null
    );

    let emergency_id = drive_to_report(
        &service,
        &admin,
        &mechanic,
        mechanic_id,
        branch_id,
        "3101",
        "EMERGENCY",
        "BREAKDOWN",
    )
    .await;

    // Precise asset history: equipment_id narrows to that machine's orders.
    let by_equipment = send(
        &service,
        "GET",
        &format!("/api/v1/work-orders?equipment_id={}", emergency_equipment),
        &admin,
        None,
        None,
    )
    .await;
    assert_eq!(by_equipment.status, StatusCode::OK);
    assert_eq!(by_equipment.json["total"], 1);
    assert_eq!(by_equipment.json["items"][0]["id"], emergency_id.as_str());
    assert_eq!(
        by_equipment.json["items"][0]["maintenance_type"],
        "EMERGENCY"
    );
    let by_type = send(
        &service,
        "GET",
        "/api/v1/work-orders?maintenance_type=EMERGENCY",
        &admin,
        None,
        None,
    )
    .await;
    assert_eq!(by_type.json["total"], 1);
    let unused_equipment = preventive_equipment;
    let none_for_unused = send(
        &service,
        "GET",
        &format!(
            "/api/v1/work-orders?equipment_id={}&maintenance_type=EMERGENCY",
            unused_equipment
        ),
        &admin,
        None,
        None,
    )
    .await;
    assert_eq!(none_for_unused.json["total"], 0);

    // MTTR is derived from real status history once a report span exists; the
    // preventive rate becomes 1.0 once an on-time preventive close exists.
    seed_completed_preventive_order(&owner_pool, branch_id, admin_id, "3102").await;
    let closed = send(&service, "GET", "/api/v1/work-orders", &admin, None, None).await;
    assert_eq!(closed.status, StatusCode::OK);
    let aggregates = &closed.json["lens"]["aggregates"];
    assert!(aggregates["mttr_minutes"].as_f64().unwrap() >= 0.0);
    assert_eq!(aggregates["preventive_on_time_rate"], json!(1.0));
    let type_facets = closed.json["lens"]["facets"]["maintenance_type"]
        .as_array()
        .unwrap()
        .iter()
        .map(|bucket| {
            (
                bucket["value"].as_str().unwrap().to_owned(),
                bucket["count"].as_i64().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert!(type_facets.contains(&("EMERGENCY".to_owned(), 1)));
    assert!(type_facets.contains(&("PREVENTIVE".to_owned(), 2)));
}

// --- chain driver ---

#[allow(clippy::too_many_arguments)]
async fn drive_to_report(
    service: &axum::Router,
    admin_token: &str,
    mechanic_token: &str,
    mechanic_id: UserId,
    branch_id: BranchId,
    management_no: &str,
    maintenance_type: &str,
    maintenance_cause: &str,
) -> String {
    let created = send(
        service,
        "POST",
        "/api/work-orders",
        admin_token,
        Some(json!({
            "branch_id": branch_id,
            "management_no": format!("#{management_no}"),
            "symptom": "유압 경고",
            "maintenance_type": maintenance_type,
            "maintenance_cause": maintenance_cause
        })),
        None,
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.json["maintenance_type"], maintenance_type);
    let work_order_id = created.json["id"].as_str().unwrap().to_owned();

    let assigned = send(
        service,
        "PUT",
        &format!("/api/work-orders/{work_order_id}/assignments"),
        admin_token,
        Some(json!({
            "assignments": [{ "mechanic_id": mechanic_id, "role": "PRIMARY" }]
        })),
        None,
    )
    .await;
    assert_eq!(assigned.status, StatusCode::OK);
    let started = send(
        service,
        "POST",
        &format!("/api/work-orders/{work_order_id}/start"),
        mechanic_token,
        None,
        None,
    )
    .await;
    assert_eq!(started.status, StatusCode::OK);
    let reported = send(
        service,
        "POST",
        &format!("/api/work-orders/{work_order_id}/report"),
        mechanic_token,
        Some(json!({
            "result_type": "COMPLETED",
            "diagnosis": "호스 파열",
            "action_taken": "호스 교체"
        })),
        None,
    )
    .await;
    assert_eq!(reported.status, StatusCode::OK);
    assert_eq!(reported.json["status"], "REPORT_SUBMITTED");
    work_order_id
}

fn settlement_body() -> Value {
    json!({
        "lines": [
            { "kind": "LABOR", "label": "출동 공임", "amount_krw": 120_000 },
            { "kind": "PART", "label": "유압 호스", "amount_krw": 80_000, "source_ref": "PO-121" }
        ],
        "note": "긴급 출동 정산"
    })
}

// --- transport ---

struct Reply {
    status: StatusCode,
    json: Value,
}

async fn send(
    service: &axum::Router,
    method: &str,
    path: &str,
    token: &str,
    body: Option<Value>,
    idempotency_key: Option<&str>,
) -> Reply {
    let mut builder = Request::builder()
        .uri(path)
        .method(method)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if let Some(key) = idempotency_key {
        builder = builder.header("Idempotency-Key", key);
    }
    let request = match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = service.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    Reply { status, json }
}

async fn audit_count(pool: &PgPool, action: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = $1")
        .bind(action)
        .fetch_one(pool)
        .await
        .unwrap()
}

// --- fixtures (seeded via the owner pool; the router runs as console_rt) ---

struct Keys {
    private_pem: String,
    public_pem: String,
}

impl Keys {
    fn generate() -> Self {
        let signing_key = SigningKey::random(&mut OsRng);
        Self {
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

    fn token(&self, user_id: UserId, org: OrgId, role: &str, branches: Vec<BranchId>) -> String {
        let issuer = JwtIssuer::from_es256_pem(
            JwtSettings {
                issuer: TEST_ISSUER.to_owned(),
                audience: TEST_AUDIENCE.to_owned(),
                access_token_ttl: Duration::minutes(15),
            },
            self.private_pem.as_bytes(),
            self.public_pem.as_bytes(),
        )
        .unwrap();
        issuer
            .issue_access_token(AccessTokenInput {
                subject: user_id,
                org_id: org,
                roles: vec![role.to_owned()],
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

async fn seed_branch(pool: &PgPool, region_name: &str, branch_name: &str) -> BranchId {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, 'knl', 'KNL') \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
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

async fn seed_other_org(pool: &PgPool) -> OrgId {
    let org_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO organizations (slug, name) VALUES ('other-tenant', 'Other Tenant') \
         RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    OrgId::from_uuid(org_id)
}

async fn seed_other_org_branch(pool: &PgPool, org: OrgId) -> BranchId {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind("Other Region")
            .bind(*org.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind("Other Branch")
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(pool: &PgPool, org: OrgId, user_id: UserId, role: &str, branch_id: BranchId) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("Maintenance {role}"))
        .bind(Vec::from([role]))
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_equipment(pool: &PgPool, branch_id: BranchId, management_no: &str) -> uuid::Uuid {
    let customer_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(format!("Customer {management_no}"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(format!("Site {management_no}"))
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
        VALUES ($1, $2, $3, $4, $5, 'A', 'B', 'C', '임대', '좌식', '2.5', 'GTS25DE',
                'test', 1, $6)
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(format!("GTS12-{:0>4}", management_no))
    .bind(management_no)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A preventive order closed on time, seeded directly: FINAL_COMPLETED before
/// its target due date with the matching status-history row, so the
/// `preventive_on_time_rate` aggregate has a real, non-simulated basis.
async fn seed_completed_preventive_order(
    pool: &PgPool,
    branch_id: BranchId,
    actor: UserId,
    management_no: &str,
) {
    let equipment: (uuid::Uuid, uuid::Uuid, uuid::Uuid) = sqlx::query_as(
        "SELECT id, customer_id, site_id FROM registry_equipment WHERE management_no = $1",
    )
    .bind(management_no)
    .fetch_one(pool)
    .await
    .unwrap();
    let work_order_id = WorkOrderId::new();
    let completed_at = OffsetDateTime::now_utc() - Duration::days(2);
    let target_due_at = OffsetDateTime::now_utc() - Duration::days(1);
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, priority, symptom, result_type,
            maintenance_type, maintenance_cause, target_due_at,
            created_at, updated_at, org_id
        )
        VALUES ($1, '20260701-901', $2, $3, $4, $5, $6, 'FINAL_COMPLETED', 'P3',
                '예방 정비 완료 fixture', 'COMPLETED', 'PREVENTIVE', 'SCHEDULED', $7,
                $8, $8, $9)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*branch_id.as_uuid())
    .bind(equipment.0)
    .bind(equipment.1)
    .bind(equipment.2)
    .bind(*actor.as_uuid())
    .bind(target_due_at)
    .bind(completed_at - Duration::days(3))
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO work_order_status_history (
            work_order_id, actor, action, from_status, to_status, occurred_at, org_id
        )
        VALUES ($1, $2, 'work_order.approve', 'ADMIN_REVIEW', 'FINAL_COMPLETED', $3, $4)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*actor.as_uuid())
    .bind(completed_at)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
}
