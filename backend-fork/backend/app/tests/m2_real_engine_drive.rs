#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! M2 REAL-ENGINE completion-tail E2E — drives the ACTUAL runtime (not raw SQL).
//!
//! ## What this proves (the AC + review findings)
//! The sibling `m2_flag_on_runtime_drain.rs` (in `platform/db`) models the spine
//! writes with hand-written SQL. This test instead drives the REAL engine end to
//! end — `console_workorder_rest::m2_strangler::drive_completion_tail` on a genuine
//! `PgWorkflowRuntimeStore` (which routes every write through the domain
//! `WorkflowRuntimePort` + `console_workflow_runtime::{start_run, process_node}`) — so
//! the production code paths, not a test re-implementation, are exercised:
//!   1. one completion tail (`start_run` → `apply_completion` object_mutation →
//!      `emit_payroll` job) creates EXACTLY one `workflow_runs` row that lands
//!      SUCCEEDED (matching production `emit_payroll`, run_target=SUCCEEDED — NOT
//!      WAITING), two `workflow_node_runs`, and one JOB `workflow_outbox_events` row;
//!   2. the adapter drainer stages EXACTLY one `payroll_draft_runs` row in
//!      `BLOCKED_LEGAL_GATE` with `calculation_enabled = FALSE`;
//!   3. RE-DRIVING the same completion (the crash reconciler's re-drive; codex HIGH
//!      2) RESUMES rather than 409-aborts and adds ZERO rows — one run, one draft,
//!      success; and a re-drain adds ZERO drafts.
//!
//! A second test proves the NARROWER crash window (codex HIGH round 2): a partial
//! run — `start_run` committed the `workflow_runs` row, then the process died before
//! `emit_payroll` wrote the JOB outbox event — is now RECOVERED by the reconciler.
//! `reconcile_completion_tails` selects it (a non-`SUCCEEDED` completion run, not just
//! a missing one), re-drives it through the real engine so the resume path emits the
//! missing outbox event and drives the run to SUCCEEDED, and the drain stages exactly
//! one BLOCKED_LEGAL_GATE draft. A second reconciler + drain pass adds ZERO (the run
//! is now SUCCEEDED, so it is never re-selected).
//!
//! ## Runtime fidelity (mandatory)
//! Every runtime write + read runs as the genuine non-owner `console_rt` role
//! (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) with `app.current_org` armed — never a
//! BYPASSRLS superuser, which would mask a broken RLS path. Only minting the
//! `organizations` row (owner-only; `console_rt` is SELECT-only there) uses the owner
//! pool; the definition seed, the tail drive, the drain, and every assertion read
//! execute as `console_rt` under the armed tenant GUC, exactly as production does.
//!
//! No `work_orders` row is needed: the runtime tail records a run keyed on the work
//! order id but does not itself write `work_orders` (`workflow_runs.object_id` has
//! no FK to it), so a synthetic id exercises the tail without the full WO chain.

use console_kernel_core::{OrgId, WorkOrderId};
// `payroll_draft_runs` is `ObjectKey::PayRun`'s table, so the drain no longer
// writes it: the real owner is injected as the staging port. Using the REAL
// `PgPayRunPort` here rather than a stub is deliberate — a stub would prove the
// drain calls SOMETHING, not that the owner's statement still stages the row.
use console_payroll_adapter_postgres::pay_run::PgPayRunPort;
use console_workflow_runtime_adapter_postgres::{M2_STRANGLER_FLAG, PgWorkflowRuntimeStore};
use console_workorder_rest::m2_strangler::{drive_completion_tail, reconcile_completion_tails};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// A dedicated TEST tenant (never production). The sqlx::test DB is fresh + empty.
const TEST_TENANT: Uuid = Uuid::from_u128(0x4d32_11c3_0000_0000_0000_0000_0000_00c3);

/// A distinct TEST tenant for the partial-run recovery test (fresh DB per sqlx::test,
/// but kept distinct for clarity). Never production.
const PARTIAL_TEST_TENANT: Uuid = Uuid::from_u128(0x4d32_11c4_0000_0000_0000_0000_0000_00c4);

/// Max JOB payroll events the adapter drainer claims per call.
const DRAIN_LIMIT: i64 = 100;

// ===========================================================================
// Runtime-role pool: every connection assumes the genuine non-owner `console_rt`, so
// RLS is ACTUALLY enforced (BYPASSRLS does not apply, FORCE RLS does). Copied from
// the sibling flag-off parity gate.
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

/// Arm `app.current_org` transaction-locally (the pool is already `console_rt` via
/// `after_connect`), exactly as the org middleware / `with_org_conn` do.
async fn arm_org(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, org: OrgId) {
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(&mut **tx)
        .await
        .unwrap();
}

/// Mint the `organizations` row via the OWNER pool (`console_rt` is SELECT-only there).
async fn seed_org(owner_pool: &PgPool, org: OrgId) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(*org.as_uuid())
    .bind("org-m2-real-engine")
    .bind("Org M2 Real Engine")
    .execute(owner_pool)
    .await
    .unwrap();
}

/// Seed the canonical work-order completion definition (`work_order.completion`,
/// object_type `work_order`, ACTIVE, one PUBLISHED `wf.exec.v1` version) as `console_rt`
/// under the armed tenant GUC, returning `(definition_id, version)` for the run to
/// bind to. Matches what `resolve_completion_definition` pins to.
async fn seed_definition(rt_pool: &PgPool, org: OrgId) -> (Uuid, i32) {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let definition_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_definitions \
             (org_id, workflow_key, display_name, object_type, status, \
              latest_version, active_version) \
         VALUES ($1, 'work_order.completion', 'Work Order Completion', 'work_order', \
                 'ACTIVE', 1, 1) \
         RETURNING id",
    )
    .bind(*org.as_uuid())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_definition_versions \
             (org_id, definition_id, version, status, definition, \
              required_approval_line, required_payment_line) \
         VALUES ($1, $2, 1, 'PUBLISHED', $3, TRUE, TRUE)",
    )
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(serde_json::json!({
        "schema_version": "wf.exec.v1",
        "template": "work_order_completion",
    }))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (definition_id, 1)
}

/// Tenant-scoped `SELECT count(*)` as `console_rt` under the armed GUC.
async fn count(rt_pool: &PgPool, org: OrgId, count_query: &'static str) -> i64 {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let n: i64 = sqlx::query_scalar(count_query)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    n
}

/// The single run's status, read as `console_rt` (exactly one run exists for the tenant).
async fn single_run_status(rt_pool: &PgPool, org: OrgId) -> String {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let status: String = sqlx::query_scalar("SELECT status FROM workflow_runs")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    status
}

/// The single staged draft's `(status, calculation_enabled)`, read as `console_rt`.
async fn single_draft(rt_pool: &PgPool, org: OrgId) -> (String, bool) {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let row = sqlx::query("SELECT status, calculation_enabled FROM payroll_draft_runs")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let out = (
        row.get::<String, _>("status"),
        row.get::<bool, _>("calculation_enabled"),
    );
    tx.commit().await.unwrap();
    out
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn real_engine_completion_tail_drives_one_succeeded_run_and_one_blocked_draft(
    owner_pool: PgPool,
) {
    let org = OrgId::from_uuid(TEST_TENANT);
    seed_org(&owner_pool, org).await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let (definition_id, version) = seed_definition(&rt_pool, org).await;
    let store = PgWorkflowRuntimeStore::new(rt_pool.clone());
    let payroll_staging = PgPayRunPort::new(rt_pool.clone(), tokio::runtime::Handle::current());
    let work_order_id = WorkOrderId::new();

    // --- Drive the tail through the REAL engine (system drive: no actor/shadow). --
    drive_completion_tail(
        &store,
        org,
        work_order_id,
        None,
        definition_id,
        version,
        Vec::new(),
    )
    .await
    .expect("completion tail must drive to success through the real engine");

    // Exactly one run (SUCCEEDED), two node runs, one JOB outbox event.
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_runs").await,
        1,
        "the tail must create exactly ONE workflow_runs row"
    );
    assert_eq!(
        single_run_status(&rt_pool, org).await,
        "SUCCEEDED",
        "the run must land SUCCEEDED (production emit_payroll sets run_target=SUCCEEDED, not WAITING)"
    );
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_node_runs").await,
        2,
        "the tail must record exactly TWO node runs (apply_completion + emit_payroll)"
    );
    assert_eq!(
        count(
            &rt_pool,
            org,
            "SELECT count(*) FROM workflow_outbox_events WHERE channel = 'JOB'"
        )
        .await,
        1,
        "emit_payroll must enqueue exactly ONE JOB outbox event"
    );
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM payroll_draft_runs").await,
        0,
        "no payroll draft exists before the outbox is drained"
    );

    // --- Drain: exactly ONE BLOCKED_LEGAL_GATE draft, calculation disabled. -------
    let created = store
        .drain_payroll_job_outbox(org, DRAIN_LIMIT, &payroll_staging)
        .await
        .unwrap();
    assert_eq!(
        created, 1,
        "the first drain must stage exactly ONE payroll draft"
    );
    let (status, calculation_enabled) = single_draft(&rt_pool, org).await;
    assert_eq!(
        status, "BLOCKED_LEGAL_GATE",
        "the drained draft must land status BLOCKED_LEGAL_GATE"
    );
    assert!(
        !calculation_enabled,
        "a BLOCKED_LEGAL_GATE draft must keep calculation disabled (fails closed)"
    );

    // --- RE-DRIVE (crash reconciler; codex HIGH 2): resume, add ZERO rows. --------
    // Driving the SAME completion again must LOAD the terminal run and no-op rather
    // than 409-abort on the deterministic run_completion_key.
    drive_completion_tail(
        &store,
        org,
        work_order_id,
        None,
        definition_id,
        version,
        Vec::new(),
    )
    .await
    .expect("re-driving a completed tail must resume to success, not conflict");
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_runs").await,
        1,
        "re-driving the same completion must NOT create a second run"
    );
    assert_eq!(
        single_run_status(&rt_pool, org).await,
        "SUCCEEDED",
        "the run stays SUCCEEDED after the idempotent re-drive"
    );
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_node_runs").await,
        2,
        "the re-drive must add ZERO node runs"
    );
    assert_eq!(
        count(
            &rt_pool,
            org,
            "SELECT count(*) FROM workflow_outbox_events WHERE channel = 'JOB'"
        )
        .await,
        1,
        "the re-drive must NOT duplicate the JOB outbox event"
    );

    // --- RE-DRAIN: the event is DELIVERED, so ZERO additional drafts. -------------
    assert_eq!(
        store
            .drain_payroll_job_outbox(org, DRAIN_LIMIT, &payroll_staging)
            .await
            .unwrap(),
        0,
        "re-draining must create ZERO additional payroll drafts (event already DELIVERED)"
    );
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM payroll_draft_runs").await,
        1,
        "exactly ONE payroll_draft_runs row survives the re-drive + re-drain"
    );
}

// ===========================================================================
// Partial-run recovery fixtures (codex HIGH round 2).
// ===========================================================================

/// Enroll the tenant in the M2 runtime (flag ON) as `console_rt` under the armed GUC —
/// the reconciler is dark-safe and does nothing for an un-enrolled tenant, so it must
/// be flipped ON to exercise the recovery. No shipped migration/seed writes this row.
async fn enroll_tenant(rt_pool: &PgPool, org: OrgId) {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    sqlx::query(
        "INSERT INTO org_runtime_flags (org_id, flag_key, enabled, rollout_note) \
         VALUES ($1, $2, TRUE, 'M2 partial-run recovery E2E (test tenant only)')",
    )
    .bind(*org.as_uuid())
    .bind(M2_STRANGLER_FLAG)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// Seed the full FK chain (region → branch → customer → site → equipment → user) for a
/// single `FINAL_COMPLETED` work order with an EXPLICIT id, so its deterministic
/// completion key `run:work_order:{id}:completion:v1` is known to the test. Minted via
/// the OWNER pool (the registry/work-order chain is console-owned in production and
/// `console_rt` cannot write it; the owner mint is the established fixture pattern, same as
/// `seed_org`). Every row carries `org_id = org` so RLS + the composite same-org FKs
/// are satisfied and the reconciler (as `console_rt` under the armed GUC) can see it.
async fn seed_final_completed_work_order(owner_pool: &PgPool, org: OrgId, work_order_uuid: Uuid) {
    let org_uuid = *org.as_uuid();
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind("recon-region")
            .bind(org_uuid)
            .fetch_one(owner_pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind("recon-branch")
    .bind(org_uuid)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let customer_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(branch_id)
    .bind("recon-customer")
    .bind(org_uuid)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let site_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(branch_id)
    .bind(customer_id)
    .bind("recon-site")
    .bind(org_uuid)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let equipment_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_equipment ( \
             branch_id, customer_id, site_id, equipment_no, manufacturer_code, kind_code, \
             power_code, status, specification, ton_text, source_sheet, source_row, org_id \
         ) \
         VALUES ($1, $2, $3, 'MNTRC-0001', 'FBR', 'FBR', 'BATTERY', '임대', '입식', '1.5톤', \
                 'recon', 1, $4) \
         RETURNING id",
    )
    .bind(branch_id)
    .bind(customer_id)
    .bind(site_id)
    .bind(org_uuid)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let requested_by = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(requested_by)
        .bind("recon-user")
        .bind(vec!["ADMIN"])
        .bind(org_uuid)
        .execute(owner_pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO work_orders ( \
             id, request_no, branch_id, equipment_id, customer_id, site_id, requested_by, \
             status, symptom, org_id \
         ) \
         VALUES ($1, '20260704-001', $2, $3, $4, $5, $6, 'FINAL_COMPLETED', 'recon symptom', $7)",
    )
    .bind(work_order_uuid)
    .bind(branch_id)
    .bind(equipment_id)
    .bind(customer_id)
    .bind(site_id)
    .bind(requested_by)
    .bind(org_uuid)
    .execute(owner_pool)
    .await
    .unwrap();
}

/// Simulate the crash the reconciler must recover: `start_run` committed the
/// `workflow_runs` row (STARTING → RUNNING) under the deterministic completion key,
/// then the process died before `emit_payroll` — so the run sits RUNNING with NO node
/// runs and NO outbox event. Written as `console_rt` under the armed GUC (mirrors the
/// sibling flag-on gate's `start_run` DB-state model), keyed on `completion_key` so the
/// reconciler's re-drive collides on `UNIQUE(org_id, idempotency_key)` and RESUMES it.
async fn seed_partial_run(
    rt_pool: &PgPool,
    org: OrgId,
    definition_id: Uuid,
    version: i32,
    completion_key: &str,
) {
    let run_id = Uuid::new_v4();
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    sqlx::query(
        "INSERT INTO workflow_runs \
             (id, org_id, definition_id, definition_version, status, trigger_type, \
              idempotency_key, correlation_id, input_payload) \
         VALUES ($1, $2, $3, $4, 'STARTING', 'OBJECT_EVENT', $5, $6, $7)",
    )
    .bind(run_id)
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(version)
    .bind(completion_key)
    .bind(format!("corr-{run_id}"))
    .bind(serde_json::json!({ "work_order": completion_key }))
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("UPDATE workflow_runs SET status = 'RUNNING', updated_at = now() WHERE id = $1")
        .bind(run_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn reconciler_recovers_a_partial_crashed_run_and_is_idempotent(owner_pool: PgPool) {
    let org = OrgId::from_uuid(PARTIAL_TEST_TENANT);
    seed_org(&owner_pool, org).await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let (definition_id, version) = seed_definition(&rt_pool, org).await;
    enroll_tenant(&rt_pool, org).await;

    // A FINAL_COMPLETED work order whose completion tail crashed mid-flight.
    let work_order_uuid = Uuid::new_v4();
    seed_final_completed_work_order(&owner_pool, org, work_order_uuid).await;

    // The crash: start_run committed the run (RUNNING) under the deterministic
    // completion key, but the process died before emit_payroll wrote the JOB outbox —
    // so there is NO node run, NO outbox event, and NO payroll draft. This is the
    // NARROWER window the old reconciler missed (a completion run EXISTS, so the
    // missing-run scan excluded it), leaving the payroll draft never staged.
    let completion_key = format!("run:work_order:{work_order_uuid}:completion:v1");
    seed_partial_run(&rt_pool, org, definition_id, version, &completion_key).await;

    let store = PgWorkflowRuntimeStore::new(rt_pool.clone());
    let payroll_staging = PgPayRunPort::new(rt_pool.clone(), tokio::runtime::Handle::current());

    // --- Precondition: exactly one RUNNING run, no nodes/outbox/draft. -------------
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_runs").await,
        1,
        "the crash left exactly ONE partial run"
    );
    assert_eq!(
        single_run_status(&rt_pool, org).await,
        "RUNNING",
        "the partial run sits RUNNING (start_run committed, emit_payroll never ran)"
    );
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_node_runs").await,
        0,
        "the crashed tail recorded ZERO node runs"
    );
    assert_eq!(
        count(
            &rt_pool,
            org,
            "SELECT count(*) FROM workflow_outbox_events WHERE channel = 'JOB'"
        )
        .await,
        0,
        "the crashed tail emitted ZERO JOB outbox events (the payroll draft is stranded)"
    );
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM payroll_draft_runs").await,
        0,
        "no payroll draft exists for the stranded partial run"
    );

    // --- FIRST reconciler pass: SELECT the non-SUCCEEDED run, RESUME it to SUCCEEDED,
    //     and emit the missing JOB outbox event through the real engine. ------------
    let restaged = reconcile_completion_tails(&store, org)
        .await
        .expect("reconciler pass must succeed");
    assert_eq!(
        restaged, 1,
        "the reconciler must restage exactly ONE crash-orphaned partial tail"
    );
    assert_eq!(
        single_run_status(&rt_pool, org).await,
        "SUCCEEDED",
        "the resumed partial run must be driven to SUCCEEDED"
    );
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_runs").await,
        1,
        "resuming the partial run must NOT create a second run"
    );
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_node_runs").await,
        2,
        "the resume completes the two missing nodes (apply_completion + emit_payroll)"
    );
    assert_eq!(
        count(
            &rt_pool,
            org,
            "SELECT count(*) FROM workflow_outbox_events WHERE channel = 'JOB'"
        )
        .await,
        1,
        "the resume must emit exactly ONE JOB outbox event (idempotently)"
    );

    // --- Drain the restaged tail (same tick as the drainer): ONE BLOCKED draft. ----
    let created = store
        .drain_payroll_job_outbox(org, DRAIN_LIMIT, &payroll_staging)
        .await
        .unwrap();
    assert_eq!(
        created, 1,
        "draining the restaged tail must stage exactly ONE payroll draft"
    );
    let (status, calculation_enabled) = single_draft(&rt_pool, org).await;
    assert_eq!(
        status, "BLOCKED_LEGAL_GATE",
        "the recovered draft must land status BLOCKED_LEGAL_GATE"
    );
    assert!(
        !calculation_enabled,
        "a BLOCKED_LEGAL_GATE draft must keep calculation disabled (fails closed)"
    );

    // --- SECOND pass: idempotent. The run is SUCCEEDED, so it is NOT re-selected. ---
    assert_eq!(
        reconcile_completion_tails(&store, org).await.unwrap(),
        0,
        "a fully-SUCCEEDED run must NOT be re-selected by the reconciler"
    );
    assert_eq!(
        store
            .drain_payroll_job_outbox(org, DRAIN_LIMIT, &payroll_staging)
            .await
            .unwrap(),
        0,
        "re-draining must stage ZERO additional payroll drafts (event already DELIVERED)"
    );
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_runs").await,
        1,
        "still exactly ONE run after the idempotent second pass"
    );
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM payroll_draft_runs").await,
        1,
        "still exactly ONE payroll draft after the idempotent second pass"
    );
    assert_eq!(
        single_run_status(&rt_pool, org).await,
        "SUCCEEDED",
        "the run stays SUCCEEDED after the idempotent second pass"
    );
}
