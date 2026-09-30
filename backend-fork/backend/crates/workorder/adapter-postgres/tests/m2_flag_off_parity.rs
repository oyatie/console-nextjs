#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! M2 DARK-LANDING PARITY GATE — flag-OFF E2E on the work-order executive-approval
//! → FINAL_COMPLETED path.
//!
//! ## What this proves (the AC)
//! With the per-tenant `workflow_runtime_m2_strangler` flag OFF (its dark
//! default: no `org_runtime_flags` row ⇒ `org_runtime_flag_enabled()` resolves
//! FALSE), the workorder executive-approval → `FINAL_COMPLETED` path is
//! byte-identical to legacy:
//!   1. the exact legacy `audit_events` action sequence is written,
//!   2. `work_orders.status` lands on `FINAL_COMPLETED`, and
//!   3. ZERO `workflow_runs` and ZERO `payroll_draft_runs` rows are created.
//!
//! This is the regression gate for the M2 dark landing: once the strangler
//! wiring (step 7) is present, a tenant whose flag is OFF must still drive the
//! legacy FSM with no new runtime state. If the flag default is ever flipped, or
//! the runtime writes a run/draft under an OFF tenant, assertions (3) fail.
//!
//! ## Runtime fidelity (mandatory)
//! Everything that matters runs as the genuine non-owner `console_rt` role
//! (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) with `app.current_org` armed — never a
//! BYPASSRLS superuser, which would mask a broken RLS/flag path. Only the base
//! fixtures that `console_rt` provably cannot create (an `organizations` row; child
//! rows are seeded with org_id set) are inserted via the owner pool, exactly as
//! the sibling runtime-role gate (`rls_read_surfaces_as_runtime_role.rs`) does.
//! The whole lifecycle (create → assign → start → report → admin approve →
//! executive approve), the completion-evidence write, the strangler resolution,
//! and every assertion read execute as `console_rt`.

use console_kernel_core::{BranchId, OrgId, TraceContext, UserId, WorkOrderId};
use console_workorder_adapter_postgres::PgWorkOrderStore;
use console_workorder_application::{
    AssignmentInput, CreateWorkOrderCommand, SubmitReportCommand, WorkOrderApprovalCommand,
    WorkOrderAssignmentCommand, WorkOrderStartCommand,
};
use console_workorder_domain::{AssignmentRole, AttachmentStage, WorkOrderStatus, WorkResultType};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use std::sync::atomic::{AtomicU64, Ordering};
use time::OffsetDateTime;
use uuid::Uuid;

/// The per-tenant M2 strangler flag key (migration 0095). Absent row ⇒ OFF.
const STRANGLER_FLAG: &str = "workflow_runtime_m2_strangler";

// ===========================================================================
// Runtime-role pool: every connection assumes the genuine non-owner `console_rt`.
// Copied verbatim from the sibling runtime-role gate so RLS is ACTUALLY
// enforced (BYPASSRLS does not apply, FORCE RLS does) — exactly as production.
// ===========================================================================
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

/// Arm `app.current_org` transaction-locally, exactly as the org middleware /
/// `with_org_conn` do, so RLS scopes every statement in the transaction.
async fn arm_org(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, org: OrgId) {
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(&mut **tx)
        .await
        .unwrap();
}

// ===========================================================================
// Base fixtures (OWNER pool). `console_rt` is SELECT-only on `organizations` and a
// fresh org's id matches no armed GUC, so the org row must be minted by the
// owner; every child row carries an explicit org_id so it lands in the tenant.
// ===========================================================================

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(owner_pool)
    .await
    .unwrap();
}

async fn seed_branch(owner_pool: &PgPool, org: Uuid) -> BranchId {
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Region {}", Uuid::new_v4()))
            .bind(org)
            .fetch_one(owner_pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("Branch {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(owner_pool: &PgPool, org: Uuid, role: &str, branch_id: BranchId) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("{role} {}", Uuid::new_v4()))
        .bind(Vec::from([role]))
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    user_id
}

/// Deterministic, check-constraint-valid `equipment_no`
/// (`^[A-Z]{3}[A-Z0-9]{2}-[0-9]{4}$`). Random suffixes made the org-scoped
/// UNIQUE flaky under CI, so keep it monotonic.
static EQUIPMENT_NO_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_equipment_no() -> String {
    const BASE36: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let seq = EQUIPMENT_NO_COUNTER.fetch_add(1, Ordering::Relaxed);
    let suffix = seq % 10_000;
    let prefix = (seq / 10_000) % (36 * 36);
    let c1 = BASE36[(prefix / 36) as usize] as char;
    let c2 = BASE36[(prefix % 36) as usize] as char;
    format!("TST{c1}{c2}-{suffix:04}")
}

async fn seed_equipment(owner_pool: &PgPool, org: Uuid, branch_id: BranchId, management_no: &str) {
    let customer_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(format!("Customer {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let site_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(format!("Site {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, model, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, $4, $5,
                'A', 'B', 'C', '임대', '좌식', '2.5', 'GTS25DE', 'test', 1, $6)
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(unique_equipment_no())
    .bind(management_no)
    .bind(org)
    .execute(owner_pool)
    .await
    .unwrap();
}

// ===========================================================================
// console_rt-armed helpers (reads + completion-evidence write).
// ===========================================================================

/// Resolve the strangler flag exactly as the application resolves whether a
/// tenant is routed through the M2 runtime: the `org_runtime_flag_enabled()`
/// SECURITY INVOKER resolver, run as `console_rt` with the tenant GUC armed.
async fn strangler_enabled(rt_pool: &PgPool, org: OrgId) -> bool {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let enabled: bool = sqlx::query_scalar("SELECT org_runtime_flag_enabled($1)")
        .bind(STRANGLER_FLAG)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    enabled
}

/// Insert one WORM-VERIFIED `REPORT` evidence row as `console_rt`, satisfying the
/// completion-evidence interlock (`lock_work_order` computes `evidence_verified`
/// as: a VERIFIED AFTER/REPORT row exists AND no non-VERIFIED AFTER/REPORT row
/// exists). Runs under the armed GUC so RLS WITH CHECK accepts the tenant row.
async fn insert_verified_report_evidence(
    rt_pool: &PgPool,
    org: OrgId,
    work_order_id: WorkOrderId,
    uploaded_by: UserId,
) {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    sqlx::query(
        r#"
        INSERT INTO evidence_media (
            work_order_id, stage, s3_key, content_type, size_bytes,
            uploaded_by, worm_replica_status, retry_count, org_id
        )
        VALUES ($1, $2, $3, 'image/jpeg', 1024, $4, 'VERIFIED', 0, $5)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(AttachmentStage::Report.as_db_str())
    .bind(format!(
        "work-orders/{}/report/{}.jpg",
        work_order_id,
        Uuid::new_v4()
    ))
    .bind(*uploaded_by.as_uuid())
    .bind(*org.as_uuid())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// The ordered `audit_events.action` sequence for a work order, read as `console_rt`
/// under the armed GUC — the byte-identical legacy audit trail is the contract.
async fn audit_actions(rt_pool: &PgPool, org: OrgId, work_order_id: WorkOrderId) -> Vec<String> {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let actions: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT action
        FROM audit_events
        WHERE target_id = $1
        ORDER BY occurred_at, created_at
        "#,
    )
    .bind(work_order_id.to_string())
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    actions
}

/// A tenant-scoped `SELECT count(*)` run as `console_rt` under the armed GUC. The
/// query is a static literal so no injection-audit override is needed.
async fn count_as_runtime(rt_pool: &PgPool, org: OrgId, count_query: &'static str) -> i64 {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let count: i64 = sqlx::query_scalar(count_query)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    count
}

/// The persisted `work_orders.status` for a work order, read as `console_rt`.
async fn work_order_status(rt_pool: &PgPool, org: OrgId, work_order_id: WorkOrderId) -> String {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let row = sqlx::query("SELECT status FROM work_orders WHERE id = $1")
        .bind(*work_order_id.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let status: String = row.try_get("status").unwrap();
    tx.commit().await.unwrap();
    status
}

// ===========================================================================
// THE PARITY GATE.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn flag_off_executive_approval_to_final_completed_is_byte_identical_to_legacy(
    owner_pool: PgPool,
) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let knl_uuid = *knl.as_uuid();

    // --- Base fixtures (owner pool; org_id set on every child row). ----------
    seed_org(&owner_pool, knl_uuid, "knl").await;
    let branch = seed_branch(&owner_pool, knl_uuid).await;
    let receptionist = seed_user(&owner_pool, knl_uuid, "RECEPTIONIST", branch).await;
    let mechanic = seed_user(&owner_pool, knl_uuid, "MECHANIC", branch).await;
    let admin = seed_user(&owner_pool, knl_uuid, "ADMIN", branch).await;
    let executive = seed_user(&owner_pool, knl_uuid, "EXECUTIVE", branch).await;
    seed_equipment(&owner_pool, knl_uuid, branch, "290").await;

    // --- Precondition: the strangler flag is DARK for this tenant. -----------
    // No org_runtime_flags row was seeded, so the resolver must return FALSE:
    // this tenant drives the legacy path, and any runtime write below is a bug.
    assert!(
        !strangler_enabled(&rt_pool, knl).await,
        "workflow_runtime_m2_strangler must resolve FALSE (dark default: absent row ⇒ OFF)"
    );

    // --- Drive the FULL executive-approval → FINAL_COMPLETED lifecycle as the
    // --- genuine non-owner console_rt role (the store arms the GUC per mutation).
    let store = PgWorkOrderStore::new(rt_pool.clone());
    let work_order = console_platform_request_context::scope_org(knl, async {
        let created = store
            .create_work_order(CreateWorkOrderCommand {
                maintenance_type: None,
                maintenance_cause: None,
                actor: receptionist,
                branch_id: branch,
                management_no: "290".to_owned(),
                symptom: "유압 누유".to_owned(),
                customer_request: Some("오후 교대 전 점검".to_owned()),
                target_due_at: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .expect("create_work_order must succeed as console_rt under armed GUC");
        assert_eq!(created.status, WorkOrderStatus::Received);

        let assigned = store
            .assign_work_order(WorkOrderAssignmentCommand {
                actor: admin,
                work_order_id: created.id,
                assignments: vec![AssignmentInput {
                    mechanic_id: mechanic,
                    role: AssignmentRole::Primary,
                }],
                admin_approver_id: Some(admin),
                executive_approver_id: Some(executive),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .expect("assign_work_order must succeed");
        assert_eq!(assigned.status, WorkOrderStatus::Assigned);

        let started = store
            .start_work(WorkOrderStartCommand {
                actor: mechanic,
                work_order_id: created.id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .expect("start_work must succeed");
        assert_eq!(started.status, WorkOrderStatus::InProgress);

        let reported = store
            .submit_report(SubmitReportCommand {
                actor: mechanic,
                work_order_id: created.id,
                result_type: WorkResultType::Completed,
                diagnosis: "인입 실 마모".to_owned(),
                action_taken: "실 교체 후 압력 시험 완료".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .expect("submit_report must succeed");
        assert_eq!(reported.status, WorkOrderStatus::ReportSubmitted);

        let admin_review = store
            .approve_work_order(WorkOrderApprovalCommand {
                actor: admin,
                work_order_id: created.id,
                comment: "검토 의견".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .expect("admin approval must succeed");
        assert_eq!(admin_review.status, WorkOrderStatus::AdminReview);

        created
    })
    .await;

    // Completion-evidence interlock: one WORM-VERIFIED REPORT row, written as
    // console_rt, unblocks the FINAL_COMPLETED transition (legacy behavior).
    insert_verified_report_evidence(&rt_pool, knl, work_order.id, mechanic).await;

    let completed = console_platform_request_context::scope_org(knl, async {
        store
            .approve_work_order(WorkOrderApprovalCommand {
                actor: executive,
                work_order_id: work_order.id,
                comment: "최종 승인".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .expect("executive approval must reach FINAL_COMPLETED")
    })
    .await;

    // --- (2) Terminal status is FINAL_COMPLETED (in-memory + persisted). -----
    assert_eq!(completed.status, WorkOrderStatus::FinalCompleted);
    assert_eq!(
        work_order_status(&rt_pool, knl, work_order.id).await,
        "FINAL_COMPLETED",
        "the persisted work_orders.status must be FINAL_COMPLETED"
    );

    // --- (1) Byte-identical legacy audit trail. ------------------------------
    let actions = audit_actions(&rt_pool, knl, work_order.id).await;
    assert_eq!(
        actions,
        vec![
            "work_order.create",
            "work_order.assign",
            "work_order.start",
            "work_order.report",
            "work_order.approve", // admin → AdminReview
            "work_order.approve", // executive → FinalCompleted
        ],
        "the flag-OFF audit trail must match the legacy executive-approval sequence exactly"
    );

    // --- (3) ZERO M2 runtime rows created under the dark default. -------------
    // The ADR-0018 spine run table and the payroll draft table must both be
    // empty for this tenant: LegacyOnly + strangler OFF creates no runtime state.
    assert_eq!(
        count_as_runtime(&rt_pool, knl, "SELECT count(*) FROM workflow_runs").await,
        0,
        "flag-OFF path must create ZERO workflow_runs rows (M2 lands dark)"
    );
    assert_eq!(
        count_as_runtime(&rt_pool, knl, "SELECT count(*) FROM payroll_draft_runs").await,
        0,
        "flag-OFF path must create ZERO payroll_draft_runs rows (drainer never runs dark)"
    );
    // Defense in depth: no node runs / outbox events / waiting tasks either.
    assert_eq!(
        count_as_runtime(&rt_pool, knl, "SELECT count(*) FROM workflow_node_runs").await,
        0,
        "flag-OFF path must create ZERO workflow_node_runs rows"
    );
    assert_eq!(
        count_as_runtime(&rt_pool, knl, "SELECT count(*) FROM workflow_outbox_events").await,
        0,
        "flag-OFF path must create ZERO workflow_outbox_events rows"
    );
}
