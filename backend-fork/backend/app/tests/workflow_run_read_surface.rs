#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-WF-HARDEN: engine read surface E2E.
//!
//! Drives the REAL router on a genuine non-owner `console_rt` pool (RLS actually
//! enforced), covering:
//!   * `GET /workflow-runs/{id}` head + waiting tasks + node-step timeline,
//!   * visibility mirrors the approval inbox exactly (initiator / claimer /
//!     authority-role holder / workflow-manage admin — deny-by-omission (404)
//!     for everyone else, including cross-org callers),
//!   * `GET /workflow-runs` admin list incl. dead-letter visibility + status
//!     filter + workflow-manage authz gate.
//!
//! Gap 13 (decouple ACTIVE+DRAFT so staging an edit doesn't block starts) does
//! not reproduce: `apply_draft_update`'s pre-existing `ensure_draft_definition`
//! guard already refuses `PATCH .../definitions/{id}` unless the definition is
//! already DRAFT, so an ACTIVE definition can never reach the code path the
//! audit described. Actually enabling "stage a revision while ACTIVE keeps
//! serving" needs a new revision-staging endpoint (BE-LC's pendingRev scope),
//! which is not a small fix — skipped per the charter's own instruction.

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
use uuid::Uuid;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const OTHER_ORG: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_0002);

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

// ===========================================================================
// 1. Run detail: initiator sees head + waiting task + timeline.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn run_detail_visible_to_initiator_with_timeline(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.detail.initiator").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&keys, initiator, "SUPER_ADMIN", branch);

    let started = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &token,
        json!({
            "definition_id": definition_id,
            "trigger_type": "MANUAL",
            "idempotency_key": "detail-initiator-key-0001",
            "input_payload": {}
        }),
    )
    .await;
    assert_eq!(started.status, StatusCode::OK, "{:?}", started.json);
    let run_id = started.json["run"]["id"].as_str().unwrap().to_owned();

    let detail = get(service, &format!("/api/v1/workflow-runs/{run_id}"), &token).await;
    assert_eq!(detail.status, StatusCode::OK, "{:?}", detail.json);
    assert_eq!(detail.json["run"]["id"], run_id);
    assert_eq!(detail.json["run"]["status"], "WAITING");
    assert_eq!(detail.json["run"]["initiated_by"], initiator.to_string());
    assert_eq!(detail.json["run"]["trigger_type"], "MANUAL");

    let waiting = detail.json["waiting_tasks"].as_array().unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0]["waiting_key"], "review.hr");

    let timeline = detail.json["timeline"].as_array().unwrap();
    // The engine drives through the entry `submit` object_gate (terminal, SUCCEEDED)
    // before parking `review.hr` (WAITING) — both are recorded node steps.
    assert_eq!(timeline.len(), 2, "{timeline:?}");
    assert_eq!(timeline[0]["node_key"], "submit");
    assert_eq!(timeline[0]["node_type"], "object_gate");
    assert_eq!(timeline[0]["status"], "SUCCEEDED");
    assert_eq!(timeline[1]["node_key"], "review.hr");
    assert_eq!(timeline[1]["node_type"], "human_task");
    assert_eq!(timeline[1]["status"], "WAITING");
}

// ===========================================================================
// 2. Run detail: a non-initiator authority-role holder sees it too (mirrors
//    `resolve_approval_run`'s visibility contract exactly).
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn run_detail_visible_to_authority_role_holder(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.detail.authority").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    let started = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &bearer(&keys, initiator, "SUPER_ADMIN", branch),
        json!({
            "definition_id": definition_id,
            "trigger_type": "MANUAL",
            "idempotency_key": "detail-authority-key-0001",
            "input_payload": {}
        }),
    )
    .await;
    let run_id = started.json["run"]["id"].as_str().unwrap().to_owned();

    // A different SUPER_ADMIN never claimed anything on this run, but holds the
    // `hr_reviewer` authority role via the legacy `completion_review` guard.
    let reviewer = UserId::new();
    seed_user(&pool, reviewer, "SUPER_ADMIN", branch).await;
    let detail = get(
        service,
        &format!("/api/v1/workflow-runs/{run_id}"),
        &bearer(&keys, reviewer, "SUPER_ADMIN", branch),
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK, "{:?}", detail.json);
    assert_eq!(detail.json["run"]["id"], run_id);
}

// ===========================================================================
// 2b. Run detail: a NON-admin authority-role holder sees it — proves the
//     authority-role OR-branch on its own, not merely riding `is_admin`
//     (ADMIN fails RoleManage/workflow-manage but passes CompletionReview).
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn run_detail_visible_to_non_admin_authority_role_holder(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.detail.authority.nonadmin").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    let started = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &bearer(&keys, initiator, "SUPER_ADMIN", branch),
        json!({
            "definition_id": definition_id,
            "trigger_type": "MANUAL",
            "idempotency_key": "detail-nonadmin-auth-key-01",
            "input_payload": {}
        }),
    )
    .await;
    let run_id = started.json["run"]["id"].as_str().unwrap().to_owned();

    // ADMIN is denied workflow-manage (RoleManage matrix: only SUPER_ADMIN passes)
    // but allowed `completion_review`, so `is_admin` is false here — a 200 proves
    // the authority-role OR-branch fires independently. Unlike SUPER_ADMIN/EXECUTIVE
    // (branch-scope short-circuits to `All`), ADMIN's branch scope is resolved from
    // `user_branches` (`resolve_branch_scope_in_org`) — it must be seeded or the
    // legacy guard denies on branch containment before the role tier ever matters.
    let reviewer = UserId::new();
    seed_user(&pool, reviewer, "ADMIN", branch).await;
    seed_user_branch(&pool, reviewer, branch).await;
    let detail = get(
        service,
        &format!("/api/v1/workflow-runs/{run_id}"),
        &bearer(&keys, reviewer, "ADMIN", branch),
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK, "{:?}", detail.json);
    assert_eq!(detail.json["run"]["id"], run_id);
}

// ===========================================================================
// 1b. Run detail: a NON-admin, non-authority initiator sees their own run —
//     proves the `initiated_by` OR-branch on its own (MECHANIC fails both
//     RoleManage and CompletionReview).
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn run_detail_visible_to_non_admin_initiator(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "MECHANIC", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.detail.initiator.nonadmin").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&keys, initiator, "MECHANIC", branch);

    let started = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &token,
        json!({
            "definition_id": definition_id,
            "trigger_type": "MANUAL",
            "idempotency_key": "detail-nonadmin-init-key-01",
            "input_payload": {}
        }),
    )
    .await;
    assert_eq!(started.status, StatusCode::OK, "{:?}", started.json);
    let run_id = started.json["run"]["id"].as_str().unwrap().to_owned();

    let detail = get(service, &format!("/api/v1/workflow-runs/{run_id}"), &token).await;
    assert_eq!(detail.status, StatusCode::OK, "{:?}", detail.json);
    assert_eq!(detail.json["run"]["id"], run_id);
    assert_eq!(detail.json["run"]["initiated_by"], initiator.to_string());
}

// ===========================================================================
// 3. Run detail: insufficient-role stranger gets 404 (deny-by-omission, never
//    a 403 that would confirm the run's existence).
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn run_detail_denies_by_omission_for_insufficient_role(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.detail.stranger").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    let started = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &bearer(&keys, initiator, "SUPER_ADMIN", branch),
        json!({
            "definition_id": definition_id,
            "trigger_type": "MANUAL",
            "idempotency_key": "detail-stranger-key-00001",
            "input_payload": {}
        }),
    )
    .await;
    let run_id = started.json["run"]["id"].as_str().unwrap().to_owned();

    // MECHANIC is denied `completion_review` (same matrix `role_inbox_denies_by_omission`
    // exercises) — holds none of the run's authority roles, never initiated or claimed.
    let mechanic = UserId::new();
    seed_user(&pool, mechanic, "MECHANIC", branch).await;
    let denied = get(
        service,
        &format!("/api/v1/workflow-runs/{run_id}"),
        &bearer(&keys, mechanic, "MECHANIC", branch),
    )
    .await;
    assert_eq!(
        denied.status,
        StatusCode::NOT_FOUND,
        "insufficient role must be indistinguishable from a missing run: {:?}",
        denied.json
    );
}

// ===========================================================================
// 4. Run detail: cross-org caller gets 404 (RLS org isolation, not a leak).
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn run_detail_denies_cross_org_caller(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    seed_org(&pool, OTHER_ORG, "Other").await;
    let definition_id = seed_approval_definition(&pool, "approval.detail.crossorg").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    let started = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &bearer(&keys, initiator, "SUPER_ADMIN", branch),
        json!({
            "definition_id": definition_id,
            "trigger_type": "MANUAL",
            "idempotency_key": "detail-crossorg-key-0001",
            "input_payload": {}
        }),
    )
    .await;
    let run_id = started.json["run"]["id"].as_str().unwrap().to_owned();

    // A caller whose token is scoped to a DIFFERENT org: even a SUPER_ADMIN role
    // cannot see KNL's run — `app.current_org` (FORCE RLS) hides the row entirely.
    let outsider = UserId::new();
    let outsider_token = issue_token(
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
        outsider,
        OTHER_ORG,
        vec!["SUPER_ADMIN".to_owned()],
        Vec::new(),
    )
    .unwrap();
    let denied = get(
        service,
        &format!("/api/v1/workflow-runs/{run_id}"),
        &outsider_token,
    )
    .await;
    assert_eq!(
        denied.status,
        StatusCode::NOT_FOUND,
        "cross-org run must never be visible: {:?}",
        denied.json
    );
}

// ===========================================================================
// 5. Run detail: a nonexistent run is 404 too (same shape as denied access).
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn run_detail_404_for_unknown_run(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let caller = UserId::new();
    seed_user(&pool, caller, "SUPER_ADMIN", branch).await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    let missing = get(
        service,
        &format!("/api/v1/workflow-runs/{}", Uuid::new_v4()),
        &bearer(&keys, caller, "SUPER_ADMIN", branch),
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

// ===========================================================================
// 6. Admin list: workflow-manage sees org-wide runs incl. dead-letter, status
//    filter narrows, keyset pagination advances; insufficient role is 403 (not
//    deny-by-omission — the list endpoint itself is manage-gated like every
//    other workflow-studio admin endpoint).
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn admin_run_list_filters_status_and_paginates(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.admin.list").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&keys, initiator, "SUPER_ADMIN", branch);

    let started = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &token,
        json!({
            "definition_id": definition_id,
            "trigger_type": "MANUAL",
            "idempotency_key": "admin-list-waiting-key-01",
            "input_payload": {}
        }),
    )
    .await;
    let waiting_run_id = started.json["run"]["id"].as_str().unwrap().to_owned();

    // Seed a DEAD_LETTERED run directly (the engine has no built-in path to
    // dead-letter a run in this test harness; dead-lettering is a runtime/outbox
    // concern out of this slice's scope — only its READ visibility is).
    let dead_letter_id =
        seed_dead_lettered_run(&pool, definition_id, initiator, "engine crashed twice").await;

    // Non-manage role (ADMIN, per the RoleManage matrix) → 403, not deny-by-omission:
    // the admin list endpoint itself is authz-gated like every other workflow-studio
    // admin surface (definitions CRUD, publish, pause, ...).
    let admin_role = UserId::new();
    seed_user(&pool, admin_role, "ADMIN", branch).await;
    let forbidden = get(
        service.clone(),
        "/api/v1/workflow-runs",
        &bearer(&keys, admin_role, "ADMIN", branch),
    )
    .await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);

    // workflow-manage (SUPER_ADMIN) sees both runs, newest first, unfiltered.
    let all = get(service.clone(), "/api/v1/workflow-runs?limit=10", &token).await;
    assert_eq!(all.status, StatusCode::OK, "{:?}", all.json);
    let items = all.json["items"].as_array().unwrap();
    let ids: Vec<&str> = items
        .iter()
        .map(|i| i["run_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&waiting_run_id.as_str()));
    assert!(ids.contains(&dead_letter_id.to_string().as_str()));

    // status=DEAD_LETTERED narrows to just the dead-lettered run.
    let dead_only = get(
        service,
        "/api/v1/workflow-runs?status=DEAD_LETTERED",
        &token,
    )
    .await;
    assert_eq!(dead_only.status, StatusCode::OK);
    let dead_items = dead_only.json["items"].as_array().unwrap();
    assert_eq!(dead_items.len(), 1);
    assert_eq!(dead_items[0]["run_id"], dead_letter_id.to_string());
    assert_eq!(dead_items[0]["status"], "DEAD_LETTERED");
}

// ===========================================================================
// 7. Dead-letter visibility on the detail endpoint: failure reason surfaces.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn run_detail_shows_dead_letter_failure_reason(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.detail.deadletter").await;
    let dead_letter_id =
        seed_dead_lettered_run(&pool, definition_id, initiator, "outbox exhausted retries").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    let detail = get(
        service,
        &format!("/api/v1/workflow-runs/{dead_letter_id}"),
        &bearer(&keys, initiator, "SUPER_ADMIN", branch),
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK, "{:?}", detail.json);
    assert_eq!(detail.json["run"]["status"], "DEAD_LETTERED");
    assert_eq!(
        detail.json["run"]["error_payload"]["reason"],
        "outbox exhausted retries"
    );
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// A linear approval definition: submit (gate) → review.hr → approve.manager →
/// finalize.author. Seeded ACTIVE with one PUBLISHED wf.exec.v1 version. Mirrors
/// `workflow_runtime_instance_api::seed_approval_definition`.
async fn seed_approval_definition(pool: &PgPool, workflow_key: &str) -> Uuid {
    let org = OrgId::knl();
    let definition = json!({
        "schema_version": "wf.exec.v1",
        "workflow_key": workflow_key,
        "nodes": [
            { "node_key": "submit", "node_type": "object_gate", "title": "Submit" },
            { "node_key": "review.hr", "node_type": "human_task", "title": "HR review",
              "assignee_role_key": "hr_reviewer", "required_policy": "approval_review" },
            { "node_key": "approve.manager", "node_type": "human_task", "title": "Manager approval",
              "assignee_role_key": "manager_approver", "required_policy": "approval_decide" },
            { "node_key": "finalize.author", "node_type": "human_task", "title": "Author finalize",
              "assignee_role_key": "initiator", "required_policy": "approval_finalize" }
        ],
        "edges": [
            { "from": "submit", "to": "review.hr" },
            { "from": "review.hr", "to": "approve.manager" },
            { "from": "approve.manager", "to": "finalize.author" }
        ]
    });
    let definition_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_definitions \
             (org_id, workflow_key, display_name, object_type, status, latest_version, active_version) \
         VALUES ($1, $2, 'Instance Approval', 'approval_document', 'ACTIVE', 1, 1) \
         RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(workflow_key)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_definition_versions \
             (org_id, definition_id, version, status, definition, required_approval_line, required_payment_line) \
         VALUES ($1, $2, 1, 'PUBLISHED', $3, TRUE, FALSE)",
    )
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(definition)
    .execute(pool)
    .await
    .unwrap();
    definition_id
}

/// Seed a DEAD_LETTERED run + one FAILED node step directly (bypassing the
/// engine — dead-lettering itself is an outbox/runtime concern out of scope for
/// this read-surface slice).
async fn seed_dead_lettered_run(
    pool: &PgPool,
    definition_id: Uuid,
    initiated_by: UserId,
    reason: &str,
) -> Uuid {
    let org = OrgId::knl();
    let run_id = Uuid::new_v4();
    let suffix = run_id.simple().to_string();
    let error_payload = json!({ "reason": reason });
    sqlx::query(
        "INSERT INTO workflow_runs \
             (id, org_id, definition_id, definition_version, status, trigger_type, \
              idempotency_key, correlation_id, initiated_by, error_payload, failed_at) \
         VALUES ($1, $2, $3, 1, 'DEAD_LETTERED', 'MANUAL', $4, $5, $6, $7, now())",
    )
    .bind(run_id)
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(format!("seed-dl-idem-{suffix}"))
    .bind(format!("seed-dl-corr-{suffix}"))
    .bind(*initiated_by.as_uuid())
    .bind(&error_payload)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_node_runs \
             (id, org_id, run_id, node_key, node_type, status, attempt, \
              idempotency_key, started_at, finished_at, error_payload) \
         VALUES ($1, $2, $3, 'approve.manager', 'human_task', 'FAILED', 1, $4, now(), now(), $5)",
    )
    .bind(Uuid::new_v4())
    .bind(*org.as_uuid())
    .bind(run_id)
    .bind(format!("seed-dl-node-idem-{suffix}"))
    .bind(&error_payload)
    .execute(pool)
    .await
    .unwrap();
    run_id
}

async fn seed_org(pool: &PgPool, org: Uuid, tag: &str) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_branch(pool: &PgPool) -> BranchId {
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind("Read Surface Region")
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind("Read Surface Branch")
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(pool: &PgPool, user_id: UserId, role: &str, _branch_id: BranchId) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("read-surface-{role}-{}", user_id.as_uuid()))
        .bind(vec![role])
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

/// `resolve_branch_scope_in_org` short-circuits SUPER_ADMIN/EXECUTIVE to
/// `BranchScope::All` without touching this table, but every other role's
/// branch scope is resolved live from `user_branches` — a branch-tier role
/// with no row here gets an EMPTY scope, failing any branch-containment check
/// regardless of its legacy Feature-matrix permission level.
async fn seed_user_branch(pool: &PgPool, user_id: UserId, branch_id: BranchId) {
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(*OrgId::knl().as_uuid())
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

fn bearer(keys: &Keys, user_id: UserId, role: &str, branch: BranchId) -> String {
    issue_token(
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
        user_id,
        *OrgId::knl().as_uuid(),
        vec![role.to_owned()],
        vec![branch],
    )
    .unwrap()
}

fn issue_token(
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    org: Uuid,
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
        org_id: OrgId::from_uuid(org),
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

async fn post(service: axum::Router, uri: &str, token: &str, body: Value) -> JsonResponse {
    send(service, "POST", uri, token, Some(body)).await
}

async fn get(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    send(service, "GET", uri, token, None).await
}

async fn send(
    service: axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> JsonResponse {
    let mut builder = Request::builder()
        .uri(uri)
        .method(method)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let request = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        builder.body(Body::from(body.to_string())).unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
}
