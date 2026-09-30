#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Mounted `GET /api/v1/me/workbench` integration coverage over PostgreSQL.
//!
//! The request runs through the public router on a real non-owner `console_rt`
//! connection pool. It proves the composition boundary uses each native source
//! under the authenticated caller's tenant and branch scope; it does not use
//! reader doubles or test-only production hooks.

use std::sync::atomic::{AtomicU16, Ordering};

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, SupportTicketId, UserId, WorkOrderId};
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
const PATH: &str = "/api/v1/me/workbench";
const RANGE_FROM: &str = "2026-07-01T00:00:00Z";
const RANGE_TO: &str = "2026-07-02T00:00:00Z";
static EQUIPMENT_SEQUENCE: AtomicU16 = AtomicU16::new(1);

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn mounted_workbench_bounds_native_sources_to_the_selected_branch_and_limit(pool: PgPool) {
    let keys = keys();
    let selected_branch = seed_branch(&pool, "Workbench Region", "Workbench Branch").await;
    let other_allowed_branch = seed_branch(&pool, "Other Region", "Other Branch").await;
    let user = UserId::new();
    seed_user(&pool, user, &[selected_branch, other_allowed_branch]).await;

    let base = OffsetDateTime::parse(
        "2026-07-01T12:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    // The public action inbox traverses support in immutable created_at order,
    // but Workbench must rank the complete bounded set by urgency/due/id before
    // applying action_limit. The older WAIT ticket must not hide the newer
    // overdue NOW ticket at action_limit=1. A ticket in the caller's other
    // allowed branch remains excluded by the explicit selected-branch scope.
    let older_wait_support = seed_support_ticket(&pool, selected_branch, user, base).await;
    let newer_overdue_support =
        seed_support_ticket(&pool, selected_branch, user, base + Duration::minutes(1)).await;
    sqlx::query("UPDATE support_tickets SET due_at = $1 WHERE id = $2")
        .bind(OffsetDateTime::now_utc() + Duration::days(7))
        .bind(*older_wait_support.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE support_tickets SET due_at = $1 WHERE id = $2")
        .bind(OffsetDateTime::now_utc() - Duration::hours(1))
        .bind(*newer_overdue_support.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let _other_allowed_support = seed_support_ticket(
        &pool,
        other_allowed_branch,
        user,
        base - Duration::minutes(1),
    )
    .await;

    // The todo owner orders open todos by created_at DESC then id, so the later
    // fixture is deterministically first after the workbench preserves source
    // order. Calendar is ordered by starts_at ASC, so its first fixture wins.
    let _older_todo = seed_todo(&pool, user, base).await;
    let first_todo = seed_todo(&pool, user, base + Duration::minutes(1)).await;
    let first_event = seed_calendar_event(&pool, user, base).await;
    let _second_event = seed_calendar_event(&pool, user, base + Duration::hours(1)).await;

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&keys, user, &[selected_branch, other_allowed_branch]);
    let response = get(
        service,
        &format!(
            "{PATH}?from={RANGE_FROM}&to={RANGE_TO}&branch_id={selected_branch}&action_limit=1&todo_limit=1&calendar_limit=1"
        ),
        &token,
    )
    .await;

    assert_eq!(response.status, StatusCode::OK, "{:?}", response.json);
    assert_eq!(response.json["timezone"], "Asia/Seoul");
    assert_eq!(response.json["range"]["from"], RANGE_FROM);
    assert_eq!(response.json["range"]["to"], RANGE_TO);
    assert_eq!(response.json["scope"]["kind"], "branches");
    assert_eq!(
        response.json["scope"]["branch_ids"],
        json!([selected_branch.to_string()])
    );
    assert_eq!(
        response.json["scope"]["selected_branch_id"],
        selected_branch.to_string()
    );
    assert_eq!(response.json["partial"], false, "{:?}", response.json);

    assert_source(
        &response.json["action_inbox"],
        format!("support:{newer_overdue_support}"),
        2,
        true,
    );
    assert_source(&response.json["todos"], first_todo.to_string(), 2, true);
    assert_source(&response.json["calendar"], first_event.to_string(), 2, true);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn mounted_workbench_dispatch_respects_selected_branch_scope(pool: PgPool) {
    let keys = keys();
    let selected_branch = seed_branch(&pool, "Dispatch Selected Region", "Selected Branch").await;
    let other_allowed_branch = seed_branch(&pool, "Dispatch Other Region", "Other Branch").await;
    let outside_scope_branch =
        seed_branch(&pool, "Dispatch Outside Region", "Outside Branch").await;
    let user = UserId::new();
    seed_user(&pool, user, &[selected_branch, other_allowed_branch]).await;

    let created_at = OffsetDateTime::parse(
        "2026-07-01T12:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let selected_equipment = seed_equipment(&pool, selected_branch, "selected-dispatch").await;
    let other_equipment = seed_equipment(&pool, other_allowed_branch, "other-dispatch").await;
    let outside_equipment = seed_equipment(&pool, outside_scope_branch, "outside-dispatch").await;
    let selected_work = seed_work_order(
        &pool,
        selected_branch,
        selected_equipment,
        user,
        "20260701-901",
    )
    .await;
    let other_work = seed_work_order(
        &pool,
        other_allowed_branch,
        other_equipment,
        user,
        "20260701-902",
    )
    .await;
    let outside_work = seed_work_order(
        &pool,
        outside_scope_branch,
        outside_equipment,
        user,
        "20260701-903",
    )
    .await;
    let selected_dispatch = seed_dispatch_offer(
        &pool,
        selected_branch,
        selected_work,
        user,
        user,
        created_at,
    )
    .await;
    let other_dispatch = seed_dispatch_offer(
        &pool,
        other_allowed_branch,
        other_work,
        user,
        user,
        created_at + Duration::minutes(1),
    )
    .await;
    let outside_dispatch = seed_dispatch_offer(
        &pool,
        outside_scope_branch,
        outside_work,
        user,
        user,
        created_at + Duration::minutes(2),
    )
    .await;

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let response = get(
        service,
        &format!(
            "{PATH}?from={RANGE_FROM}&to={RANGE_TO}&branch_id={selected_branch}&action_limit=10"
        ),
        &bearer(&keys, user, &[selected_branch, other_allowed_branch]),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK, "{:?}", response.json);
    assert_source(
        &response.json["action_inbox"],
        format!("dispatch:{selected_dispatch}"),
        1,
        false,
    );
    let items = response.json["action_inbox"]["items"].as_array().unwrap();
    assert!(
        items.iter().all(|item| {
            item["id"] != format!("dispatch:{other_dispatch}")
                && item["id"] != format!("dispatch:{outside_dispatch}")
        }),
        "dispatch COUNT and rows must share the selected-branch predicate: {:?}",
        response.json["action_inbox"]
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn mounted_workbench_marks_todos_unavailable_when_runtime_role_loses_select(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool, "Failure Region", "Failure Branch").await;
    let user = UserId::new();
    seed_user(&pool, user, &[branch]).await;
    let base = OffsetDateTime::parse(
        "2026-07-01T12:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let support = seed_support_ticket(&pool, branch, user, base).await;
    let event = seed_calendar_event(&pool, user, base).await;

    // This is a real source failure at the runtime boundary, not an injected
    // reader double: native todo storage receives SQLSTATE insufficient_privilege
    // while action and calendar retain their independently granted access.
    sqlx::query("REVOKE SELECT ON TABLE todos FROM console_rt")
        .execute(&pool)
        .await
        .unwrap();

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let response = get(
        service,
        &format!(
            "{PATH}?from={RANGE_FROM}&to={RANGE_TO}&branch_id={branch}&action_limit=1&todo_limit=1&calendar_limit=1"
        ),
        &bearer(&keys, user, &[branch]),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK, "{:?}", response.json);
    assert_eq!(response.json["partial"], true, "{:?}", response.json);
    assert_source(
        &response.json["action_inbox"],
        format!("support:{support}"),
        1,
        false,
    );
    assert_eq!(
        response.json["todos"],
        json!({"status": "unavailable", "code": "todo_unavailable"})
    );
    assert_source(&response.json["calendar"], event.to_string(), 1, false);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn mounted_workbench_rejects_an_explicit_branch_outside_token_scope(pool: PgPool) {
    let keys = keys();
    let allowed_branch = seed_branch(&pool, "Allowed Region", "Allowed Branch").await;
    let excluded_branch = seed_branch(&pool, "Excluded Region", "Excluded Branch").await;
    let user = UserId::new();
    seed_user(&pool, user, &[allowed_branch]).await;

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let response = get(
        service,
        &format!(
            "{PATH}?from={RANGE_FROM}&to={RANGE_TO}&branch_id={excluded_branch}&action_limit=1&todo_limit=1&calendar_limit=1"
        ),
        &bearer(&keys, user, &[allowed_branch]),
    )
    .await;

    assert_eq!(
        response.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        response.json
    );
    assert_eq!(response.json["error"]["code"], "branch_out_of_scope");
    assert_eq!(
        response.json["error"]["message"],
        "workbench access is not permitted"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn mounted_workbench_validation_error_uses_the_standard_error_body(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool, "Validation Region", "Validation Branch").await;
    let user = UserId::new();
    seed_user(&pool, user, &[branch]).await;

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let response = get(
        service,
        &format!("{PATH}?action_limit=0"),
        &bearer(&keys, user, &[branch]),
    )
    .await;

    assert_eq!(
        response.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        response.json
    );
    assert_eq!(response.json["error"]["code"], "invalid_action_limit");
    assert_eq!(
        response.json["error"]["message"],
        "workbench request is invalid"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn mounted_workbench_all_source_failures_use_the_standard_error_body(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool, "Unavailable Region", "Unavailable Branch").await;
    let user = UserId::new();
    seed_user(&pool, user, &[branch]).await;

    // These real runtime-role denials make every aggregate source unavailable:
    // the action inbox reaches support after its empty workflow and dispatch
    // pages, while todo and calendar read their respective durable tables.
    sqlx::query(
        "REVOKE SELECT ON TABLE support_tickets, todos, collaboration_calendar_events FROM console_rt",
    )
    .execute(&pool)
    .await
    .unwrap();

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let response = get(
        service,
        &format!("{PATH}?from={RANGE_FROM}&to={RANGE_TO}&branch_id={branch}"),
        &bearer(&keys, user, &[branch]),
    )
    .await;

    assert_eq!(
        response.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{:?}",
        response.json
    );
    assert_eq!(
        response.json["error"]["code"],
        "workbench_sources_unavailable"
    );
    assert_eq!(
        response.json["error"]["message"],
        "workbench is temporarily unavailable"
    );
}

fn assert_source(source: &Value, expected_id: String, total: u64, truncated: bool) {
    assert_eq!(source["status"], "ok", "{source:?}");
    assert_eq!(source["total"], total, "{source:?}");
    assert_eq!(source["truncated"], truncated, "{source:?}");
    let items = source["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{source:?}");
    assert_eq!(items[0]["id"], expected_id, "{source:?}");
}

async fn seed_branch(pool: &PgPool, region_name: &str, branch_name: &str) -> BranchId {
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(region_name)
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
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

async fn seed_user(pool: &PgPool, user: UserId, branches: &[BranchId]) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user.as_uuid())
        .bind(format!("workbench-{}", user.as_uuid()))
        .bind(vec!["MEMBER"])
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    for branch in branches {
        sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
            .bind(*user.as_uuid())
            .bind(*branch.as_uuid())
            .bind(*OrgId::knl().as_uuid())
            .execute(pool)
            .await
            .unwrap();
    }
}

async fn seed_support_ticket(
    pool: &PgPool,
    branch: BranchId,
    user: UserId,
    created_at: OffsetDateTime,
) -> SupportTicketId {
    let id = SupportTicketId::new();
    sqlx::query(
        "INSERT INTO support_tickets (id, branch_id, origin, category, priority, status, \
         title, body, requester_user_id, assignee_user_id, due_at, created_at, updated_at, org_id) \
         VALUES ($1, $2, 'INTERNAL', 'OPERATIONAL', 'MEDIUM', 'OPEN', $3, 'details', \
                 $4, $4, $5, $6, $6, $7)",
    )
    .bind(*id.as_uuid())
    .bind(*branch.as_uuid())
    .bind(format!("native-workbench-ticket-{id}"))
    .bind(*user.as_uuid())
    .bind(created_at + Duration::hours(1))
    .bind(created_at)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn seed_equipment(pool: &PgPool, branch: BranchId, tag: &str) -> Uuid {
    let sequence = EQUIPMENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    assert!(sequence <= 9_999, "equipment fixture sequence exhausted");
    let customer_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(format!("Workbench customer {tag}"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(customer_id)
    .bind(format!("Workbench site {tag}"))
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
                'A', 'B', 'C', '임대', '좌식', '2.5', 'GTS25DE', 'workbench-test', 1, $6)
        RETURNING id
        "#,
    )
    .bind(*branch.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    // `registry_equipment.equipment_no` CHECKs `^[A-Z]{3}[A-Z0-9]{2}-[0-9]{4}$`
    // (0007_create_registry.sql:35): three letters, two alphanumerics, a hyphen, four digits.
    // `WB{sequence:06}` produced "WB000001" — two leading letters and no hyphen — which has
    // never satisfied that column. The `assert!(sequence <= 9_999)` above already guarantees
    // the four-digit tail, and per-org UNIQUE (0029_enforce_org_id.sql:117) still holds.
    .bind(format!("WBA01-{sequence:04}"))
    .bind(format!("WB-MG-{sequence:04}"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_work_order(
    pool: &PgPool,
    branch: BranchId,
    equipment: Uuid,
    requester: UserId,
    request_no: &str,
) -> WorkOrderId {
    let id = WorkOrderId::new();
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, priority, symptom, org_id
        )
        SELECT $1, $2, $3, e.id, e.customer_id, e.site_id,
               $4, 'UNASSIGNED', 'P1', 'workbench dispatch fixture', $6
        FROM registry_equipment e
        WHERE e.id = $5
        "#,
    )
    .bind(*id.as_uuid())
    .bind(request_no)
    .bind(*branch.as_uuid())
    .bind(*requester.as_uuid())
    .bind(equipment)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn seed_dispatch_offer(
    pool: &PgPool,
    branch: BranchId,
    work_order: WorkOrderId,
    creator: UserId,
    target: UserId,
    created_at: OffsetDateTime,
) -> Uuid {
    let dispatch_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO p1_dispatches \
         (id, work_order_id, branch_id, status, include_region, accept_window_started_at, \
          accept_window_ends_at, created_by, created_at, updated_at, org_id) \
         VALUES ($1, $2, $3, 'BROADCASTING', FALSE, $4, $5, $6, $4, $4, $7)",
    )
    .bind(dispatch_id)
    .bind(*work_order.as_uuid())
    .bind(*branch.as_uuid())
    .bind(created_at)
    .bind(OffsetDateTime::now_utc() + Duration::days(2))
    .bind(*creator.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO p1_dispatch_targets \
         (dispatch_id, user_id, target_role, push_token_count, fanout_created_at, org_id) \
         VALUES ($1, $2, 'TECHNICIAN', 0, $3, $4)",
    )
    .bind(dispatch_id)
    .bind(*target.as_uuid())
    .bind(created_at)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    dispatch_id
}

async fn seed_todo(pool: &PgPool, user: UserId, created_at: OffsetDateTime) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO todos (id, org_id, owner_user_id, body, scopes, links, created_at, updated_at) \
         VALUES ($1, $2, $3, 'native workbench todo', '[]'::jsonb, '[]'::jsonb, $4, $4)",
    )
    .bind(id)
    .bind(*OrgId::knl().as_uuid())
    .bind(*user.as_uuid())
    .bind(created_at)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn seed_calendar_event(pool: &PgPool, user: UserId, starts_at: OffsetDateTime) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO collaboration_calendar_events (\
            id, org_id, scope_type, scope_ref, title, description, starts_at, ends_at, \
            all_day, status, created_by, updated_by, created_at, updated_at\
         ) VALUES (\
            $1, $2, 'PERSONAL', $3, 'native workbench event', '', $4, $5, \
            FALSE, 'ACTIVE', $6, $6, $7, $7\
         )",
    )
    .bind(id)
    .bind(*OrgId::knl().as_uuid())
    .bind(user.as_uuid().to_string())
    .bind(starts_at)
    .bind(starts_at + Duration::minutes(30))
    .bind(*user.as_uuid())
    .bind(starts_at)
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

fn bearer(keys: &Keys, user: UserId, branches: &[BranchId]) -> String {
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
            subject: user,
            org_id: OrgId::knl(),
            roles: vec!["MEMBER".to_owned()],
            branches: branches.to_vec(),
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

async fn get(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    let request = Request::builder()
        .uri(uri)
        .method("GET")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
}
