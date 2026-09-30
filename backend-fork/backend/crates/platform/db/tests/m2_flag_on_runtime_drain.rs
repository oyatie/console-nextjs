#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! M2 FLAG-ON RUNTIME E2E — the strangler-enrolled (test-tenant only) path drives
//! ONE run→node FSM through the ADR-0018 spine and idempotently lands ONE payroll
//! draft in `BLOCKED_LEGAL_GATE`.
//!
//! ## What this proves (the AC)
//! With the per-tenant `workflow_runtime_m2_strangler` flag turned ON for a single
//! TEST tenant (enrolled here, never in a shipped migration/seed), the new runtime:
//!   1. drives one run→node finite-state machine on the reused ADR-0018 spine
//!      (`workflow_runs` STARTING→RUNNING→SUCCEEDED, `workflow_node_runs`
//!      PENDING→RUNNING→SUCCEEDED, one `workflow_outbox_events` JOB row), creating
//!      exactly ONE `workflow_runs` row for the tenant. The payroll JOB node leaves
//!      the run SUCCEEDED — matching production `emit_payroll` (run_target=SUCCEEDED),
//!      not WAITING (the strangler treats the approval gate as an already-satisfied
//!      precondition, so the tail never parks);
//!   2. drains the JOB outbox event (`FOR UPDATE SKIP LOCKED`) and, keyed on the
//!      deterministic per-run natural key `workflow_runtime_m2:run:{run_id}`,
//!      creates exactly ONE `payroll_draft_runs` row landing status
//!      `BLOCKED_LEGAL_GATE` with `calculation_enabled = FALSE`; and
//!   3. is idempotent: replaying the drain, re-emitting the outbox event under the
//!      reused `UNIQUE(org_id, idempotency_key)`, and re-inserting the draft under
//!      the reused `UNIQUE(org_id, period_start, period_end, source_label)` each add
//!      ZERO rows. Exactly one run and one BLOCKED_LEGAL_GATE draft survive.
//!
//! The strangler flag is flipped ON only for the TEST tenant, inside this test.
//! Nothing in the shipped migrations/e2e seeds enrolls a tenant, so M2 still lands
//! dark in production — migration 0095 ships zero enabled `org_runtime_flags` rows
//! and an absent row resolves FALSE, so every production tenant drives the legacy
//! path. A second, un-enrolled tenant asserts the runtime state is strictly
//! tenant-scoped (RLS): it sees ZERO runs and ZERO drafts.
//!
//! ## Runtime fidelity (mandatory)
//! Everything except minting the two `organizations` rows (an owner-only privilege;
//! `console_rt` is SELECT-only there) runs as the genuine non-owner `console_rt` role
//! (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) with `app.current_org` armed — never a
//! BYPASSRLS superuser, which would mask a broken RLS/flag path. The definition
//! seed, the flag enrollment, the run/node FSM writes, the transactional-outbox
//! emit, the drain, and every assertion read all execute as `console_rt` under the
//! armed tenant GUC, exactly as production does through `with_org_conn`.
//!
//! No new runtime tables are introduced: this reuses the spine (migrations
//! 0077/0078), the strangler switchboard (0095), the Workflow Studio definition
//! catalog (0069), and `payroll_draft_runs` (0074) — the spine gate stays green.

use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// The non-owner runtime role the application connects as in production.
/// A static literal so sqlx accepts it without an injection-audit override.
const SET_RUNTIME_ROLE: &str = "SET LOCAL ROLE console_rt";

/// The per-tenant M2 strangler flag key (migration 0095). Absent row ⇒ OFF.
const STRANGLER_FLAG: &str = "workflow_runtime_m2_strangler";

/// A distinct TEST tenant — never `knl()`/production. Enrolled ON only inside this
/// test; no shipped migration/seed writes an `org_runtime_flags` row.
const TEST_TENANT: Uuid = Uuid::from_u128(0x4d32_11a1_0000_0000_0000_0000_0000_00a1);
/// A second tenant left dark, used to prove the runtime state is tenant-scoped.
const OTHER_TENANT: Uuid = Uuid::from_u128(0x4d32_11b2_0000_0000_0000_0000_0000_00b2);

/// The billing period the completion→approval→payroll template drafts for.
const PERIOD_START: &str = "2026-06-01";
const PERIOD_END: &str = "2026-06-30";

// ===========================================================================
// Role + tenant arming. Copied from the sibling RLS gate: drop to the non-owner
// runtime role and arm `app.current_org` transaction-locally, so RLS scopes
// every statement exactly as the org middleware / `with_org_conn` do.
// ===========================================================================
async fn arm(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, org: Uuid) {
    sqlx::query(SET_RUNTIME_ROLE)
        .execute(&mut **tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.to_string())
        .execute(&mut **tx)
        .await
        .unwrap();
}

/// Mint an `organizations` row as the OWNER pool role (console_rt is SELECT-only on
/// organizations, and a fresh org id matches no armed GUC), exactly as the
/// sibling RLS/parity gates do. Every child row below carries an explicit org_id.
async fn seed_org(pool: &PgPool, org: Uuid, tag: &str) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(pool)
    .await
    .unwrap();
}

// ===========================================================================
// Definition catalog + strangler enrollment (both as console_rt under armed GUC).
// ===========================================================================

/// The real completion→approval→payroll template (step 6): a genuine node graph
/// whose terminal `payroll.draft_gate` node fans a JOB out to the `internal.jobs`
/// connector. Persisted as an ACTIVE, PUBLISHED Workflow Studio definition so the
/// run below can bind to a real `(definition_id, version)` pair (no placeholder).
fn completion_approval_payroll_definition() -> Value {
    json!({
        "template": "completion_approval_payroll",
        "trigger": { "type": "OBJECT_EVENT", "object_type": "payroll_period" },
        "nodes": [
            { "key": "completion.capture", "type": "object_event", "next": "approval.executive" },
            {
                "key": "approval.executive",
                "type": "waiting_task",
                "required_policy": "payroll.legal_gate",
                "next": "payroll.draft_gate"
            },
            {
                "key": "payroll.draft_gate",
                "type": "job",
                "connector": "internal.jobs",
                "job": "payroll_draft",
                "emits_status": "BLOCKED_LEGAL_GATE"
            }
        ]
    })
}

/// The JOB outbox payload the payroll node emits: the `internal.jobs` connector
/// spec plus the billing period the drainer stages a draft for.
fn payroll_job_payload() -> Value {
    json!({
        "connector": "internal.jobs",
        "job": "payroll_draft",
        "period_start": PERIOD_START,
        "period_end": PERIOD_END,
        "expected_status": "BLOCKED_LEGAL_GATE"
    })
}

/// Seed one ACTIVE definition + one PUBLISHED version for the tenant, returning
/// `(definition_id, version)` for the run to bind to.
async fn seed_definition(pool: &PgPool, org: Uuid) -> (Uuid, i32) {
    let mut tx = pool.begin().await.unwrap();
    arm(&mut tx, org).await;
    let definition_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_definitions \
             (org_id, workflow_key, display_name, object_type, status, latest_version, active_version) \
         VALUES ($1, 'payroll.completion_approval', 'Completion → Approval → Payroll', \
             'payroll_period', 'ACTIVE', 1, 1) \
         RETURNING id",
    )
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_definition_versions \
             (org_id, definition_id, version, status, definition, \
              required_approval_line, required_payment_line) \
         VALUES ($1, $2, 1, 'PUBLISHED', $3, TRUE, TRUE)",
    )
    .bind(org)
    .bind(definition_id)
    .bind(completion_approval_payroll_definition())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (definition_id, 1)
}

/// Enroll the tenant in the M2 runtime (flag ON). This is the deliberate,
/// audited, per-tenant roll-forward write — performed here for the TEST tenant
/// ONLY, under `console_rt` with the tenant GUC armed. No shipped migration/seed does
/// this, so production stays dark.
async fn enroll_tenant(pool: &PgPool, org: Uuid) {
    let mut tx = pool.begin().await.unwrap();
    arm(&mut tx, org).await;
    sqlx::query(
        "INSERT INTO org_runtime_flags (org_id, flag_key, enabled, rollout_note) \
         VALUES ($1, $2, TRUE, 'M2 flag-on runtime E2E (test tenant only)')",
    )
    .bind(org)
    .bind(STRANGLER_FLAG)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// Resolve the strangler flag exactly as the application decides whether a tenant
/// is routed through the M2 runtime: the `org_runtime_flag_enabled()` SECURITY
/// INVOKER resolver, run as `console_rt` with the tenant GUC armed.
async fn strangler_enabled(pool: &PgPool, org: Uuid) -> bool {
    let mut tx = pool.begin().await.unwrap();
    arm(&mut tx, org).await;
    let enabled: bool = sqlx::query_scalar("SELECT org_runtime_flag_enabled($1)")
        .bind(STRANGLER_FLAG)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    enabled
}

// ===========================================================================
// The run→node FSM (as console_rt under armed GUC), modeling the executor's write
// contract on the ADR-0018 spine.
// ===========================================================================

/// Start a run: INSERT `workflow_runs` STARTING, then transition to RUNNING —
/// the run id is pre-generated so the deterministic idempotency/natural keys can
/// be derived without a chicken-and-egg on the DEFAULT id.
async fn start_run(pool: &PgPool, org: Uuid, definition_id: Uuid, version: i32) -> Uuid {
    let run_id = Uuid::new_v4();
    let mut tx = pool.begin().await.unwrap();
    arm(&mut tx, org).await;
    sqlx::query(
        "INSERT INTO workflow_runs \
             (id, org_id, definition_id, definition_version, status, trigger_type, \
              idempotency_key, correlation_id, input_payload) \
         VALUES ($1, $2, $3, $4, 'STARTING', 'OBJECT_EVENT', $5, $6, $7)",
    )
    .bind(run_id)
    .bind(org)
    .bind(definition_id)
    .bind(version)
    .bind(format!("workflow_runtime_m2:trigger:{run_id}"))
    .bind(format!("corr-{run_id}"))
    .bind(json!({ "object_type": "payroll_period", "period_start": PERIOD_START }))
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("UPDATE workflow_runs SET status = 'RUNNING', updated_at = now() WHERE id = $1")
        .bind(run_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    run_id
}

/// Process the terminal payroll node in ONE transaction (the transactional-outbox
/// pattern): the node walks PENDING→RUNNING→SUCCEEDED, emits exactly one JOB
/// outbox event to `internal.jobs`, and the run lands SUCCEEDED — matching
/// production `emit_payroll` (run_target=SUCCEEDED). Returns
/// `(node_run_id, outbox_idempotency_key)`.
async fn process_payroll_node(pool: &PgPool, org: Uuid, run_id: Uuid) -> (Uuid, String) {
    let node_run_id = Uuid::new_v4();
    let outbox_idempotency_key = format!("outbox:{run_id}:{node_run_id}:job:payroll_draft");
    let mut tx = pool.begin().await.unwrap();
    arm(&mut tx, org).await;

    // Node: PENDING → RUNNING.
    sqlx::query(
        "INSERT INTO workflow_node_runs \
             (id, org_id, run_id, node_key, node_type, status, idempotency_key, input_payload) \
         VALUES ($1, $2, $3, 'payroll.draft_gate', 'job', 'PENDING', $4, $5)",
    )
    .bind(node_run_id)
    .bind(org)
    .bind(run_id)
    .bind(format!("node:{run_id}:{node_run_id}"))
    .bind(payroll_job_payload())
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow_node_runs SET status = 'RUNNING', started_at = now(), updated_at = now() \
         WHERE id = $1",
    )
    .bind(node_run_id)
    .execute(&mut *tx)
    .await
    .unwrap();

    // Emit exactly one JOB outbox event to the internal.jobs connector.
    sqlx::query(
        "INSERT INTO workflow_outbox_events \
             (org_id, run_id, node_run_id, channel, destination_ref, idempotency_key, status, payload) \
         VALUES ($1, $2, $3, 'JOB', 'internal.jobs', $4, 'PENDING', $5)",
    )
    .bind(org)
    .bind(run_id)
    .bind(node_run_id)
    .bind(&outbox_idempotency_key)
    .bind(payroll_job_payload())
    .execute(&mut *tx)
    .await
    .unwrap();

    // Node SUCCEEDED (it enqueued the job); run lands SUCCEEDED — the payroll JOB
    // node is the tail's terminal step, exactly as production emit_payroll drives it
    // (run_target=SUCCEEDED). completed_at is stamped so the 0077 terminal-timestamp
    // CHECK matches the adapter's run_transition write.
    sqlx::query(
        "UPDATE workflow_node_runs \
         SET status = 'SUCCEEDED', finished_at = now(), updated_at = now(), \
             output_payload = $2 \
         WHERE id = $1",
    )
    .bind(node_run_id)
    .bind(json!({ "emitted": "payroll_draft", "connector": "internal.jobs" }))
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow_runs \
         SET status = 'SUCCEEDED', completed_at = now(), updated_at = now() \
         WHERE id = $1",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await
    .unwrap();

    tx.commit().await.unwrap();
    (node_run_id, outbox_idempotency_key)
}

// ===========================================================================
// The outbox drainer (as console_rt under armed GUC).
// ===========================================================================

/// The M2 outbox-drain audit action. Matches the `audit_events.action` regex
/// (`^[a-z0-9_]+(\.[a-z0-9_]+)+$`) so the `with_audits`-style row inserts cleanly.
const DRAIN_AUDIT_ACTION: &str = "workflow_runtime.outbox_drain";

/// Claim pending/failed JOB payroll outbox events with `FOR UPDATE SKIP LOCKED`
/// inside the caller's already-armed transaction — exactly how a competing
/// drainer skips a row another worker already holds.
async fn claim_payroll_outbox(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Vec<sqlx::postgres::PgRow> {
    sqlx::query(
        "SELECT id \
         FROM workflow_outbox_events \
         WHERE channel = 'JOB' \
           AND status IN ('PENDING', 'FAILED') \
           AND payload->>'job' = 'payroll_draft' \
         ORDER BY next_attempt_at NULLS FIRST, created_at \
         FOR UPDATE SKIP LOCKED",
    )
    .fetch_all(&mut **tx)
    .await
    .unwrap()
}

/// Consume ONE claimed event inside the caller's transaction, modeling the
/// production `with_audits` "consume" txn: the idempotent draft insert (keyed on
/// the reused `UNIQUE(org_id, period_start, period_end, source_label)` via the
/// deterministic per-run `source_label` `workflow_runtime_m2:run:{run_id}`), the
/// outbox `DELIVERED` ack, and one org-bound `audit_events` row ALL land in the
/// SAME transaction. Because they share one txn they are all-or-nothing: a
/// rollback (or a crash mid-drain) persists NONE of them, and a committed replay
/// adds ZERO rows (ON CONFLICT DO NOTHING + the event is already DELIVERED, so it
/// is never re-claimed). Returns the number of `payroll_draft_runs` rows actually
/// created for this event.
async fn drain_event_in_txn(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, event_id: Uuid) -> i64 {
    // (a) Idempotent draft create, keyed on the reused payroll natural key. The
    // source_label is derived in SQL from the event's run_id, so a replay of the
    // same run collides on UNIQUE(org_id, period_start, period_end, source_label)
    // and inserts nothing.
    let inserted: Vec<Uuid> = sqlx::query_scalar(
        "INSERT INTO payroll_draft_runs \
             (org_id, period_start, period_end, source_label, status, source_summary) \
         SELECT o.org_id, \
                (o.payload->>'period_start')::date, \
                (o.payload->>'period_end')::date, \
                'workflow_runtime_m2:run:' || o.run_id::text, \
                'BLOCKED_LEGAL_GATE', \
                jsonb_build_object( \
                    'outbox_event_id', o.id, \
                    'run_id', o.run_id, \
                    'connector', o.payload->>'connector', \
                    'job', o.payload->>'job') \
         FROM workflow_outbox_events o \
         WHERE o.id = $1 \
         ON CONFLICT (org_id, period_start, period_end, source_label) DO NOTHING \
         RETURNING id",
    )
    .bind(event_id)
    .fetch_all(&mut **tx)
    .await
    .unwrap();
    let created = inserted.len() as i64;

    // (b) Ack the event DELIVERED in the same txn (0078 requires delivered_at).
    sqlx::query(
        "UPDATE workflow_outbox_events \
         SET status = 'DELIVERED', delivered_at = now(), \
             attempt_count = attempt_count + 1, updated_at = now() \
         WHERE id = $1",
    )
    .bind(event_id)
    .execute(&mut **tx)
    .await
    .unwrap();

    // (c) with_audits: land exactly ONE org-bound audit row for this consume in
    // the SAME txn as the draft insert + delivery ack. org_id is copied from the
    // event so the FORCE-RLS `audit_events` WITH CHECK (org_id = app.current_org)
    // passes under the armed GUC — the audit row shares the tenant AND the fate of
    // the state change it records (rolled back together, committed together).
    sqlx::query(
        "INSERT INTO audit_events \
             (id, actor, action, target_type, target_id, \
              before_snap, after_snap, trace_id, span_id, occurred_at, org_id) \
         SELECT gen_random_uuid(), NULL::uuid, $2, 'workflow_outbox_event', o.id::text, \
                jsonb_build_object('status', 'PENDING'), \
                jsonb_build_object( \
                    'status', 'DELIVERED', \
                    'payroll_drafts_created', $3::int, \
                    'source_label', 'workflow_runtime_m2:run:' || o.run_id::text), \
                repeat('0', 32), repeat('0', 16), now(), o.org_id \
         FROM workflow_outbox_events o \
         WHERE o.id = $1",
    )
    .bind(event_id)
    .bind(DRAIN_AUDIT_ACTION)
    .bind(created as i32)
    .execute(&mut **tx)
    .await
    .unwrap();

    created
}

/// Drain pending JOB outbox events: claim with `FOR UPDATE SKIP LOCKED`, run the
/// shared consume body (draft insert + DELIVERED ack + audit row) per event, and
/// COMMIT. Returns the number of `payroll_draft_runs` rows actually created
/// (0 on replay — the event is already DELIVERED and ON CONFLICT DO NOTHING).
async fn drain_payroll_outbox(pool: &PgPool, org: Uuid) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    arm(&mut tx, org).await;

    let claimed = claim_payroll_outbox(&mut tx).await;
    let mut created = 0i64;
    for row in &claimed {
        let event_id: Uuid = row.get("id");
        created += drain_event_in_txn(&mut tx, event_id).await;
    }

    tx.commit().await.unwrap();
    created
}

/// ATOMICITY PROBE — run the IDENTICAL claim → consume body as
/// `drain_payroll_outbox`, but ROLL BACK instead of committing. Because the draft
/// insert, the DELIVERED ack, and the audit row share ONE transaction, the
/// rollback undoes ALL of them: zero drafts, zero audit rows, and the event stays
/// PENDING for a later real drain. Returns the number of drafts the body WOULD
/// have created before the rollback — proving the insert genuinely ran inside the
/// shared txn (and was then discarded atomically).
async fn drain_then_rollback(pool: &PgPool, org: Uuid) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    arm(&mut tx, org).await;

    let claimed = claim_payroll_outbox(&mut tx).await;
    let mut created = 0i64;
    for row in &claimed {
        let event_id: Uuid = row.get("id");
        created += drain_event_in_txn(&mut tx, event_id).await;
    }

    tx.rollback().await.unwrap();
    created
}

// ===========================================================================
// Idempotency probes + assertion reads (as console_rt under armed GUC).
// ===========================================================================

/// A tenant-scoped `SELECT count(*)` (static literal) run as `console_rt` under the
/// armed GUC.
async fn count(pool: &PgPool, org: Uuid, count_query: &'static str) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    arm(&mut tx, org).await;
    let n: i64 = sqlx::query_scalar(count_query)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    n
}

/// Read the single staged draft's `(status, source_label, calculation_enabled)`.
async fn payroll_draft(pool: &PgPool, org: Uuid, run_id: Uuid) -> (String, String, bool) {
    let mut tx = pool.begin().await.unwrap();
    arm(&mut tx, org).await;
    let row = sqlx::query(
        "SELECT status, source_label, calculation_enabled \
         FROM payroll_draft_runs WHERE source_label = $1",
    )
    .bind(format!("workflow_runtime_m2:run:{run_id}"))
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let out = (
        row.get::<String, _>("status"),
        row.get::<String, _>("source_label"),
        row.get::<bool, _>("calculation_enabled"),
    );
    tx.commit().await.unwrap();
    out
}

/// Read `(workflow_runs.status, workflow_outbox_events.status)` for the run.
async fn run_and_outbox_status(pool: &PgPool, org: Uuid, run_id: Uuid) -> (String, String) {
    let mut tx = pool.begin().await.unwrap();
    arm(&mut tx, org).await;
    let run_status: String = sqlx::query_scalar("SELECT status FROM workflow_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let outbox_status: String =
        sqlx::query_scalar("SELECT status FROM workflow_outbox_events WHERE run_id = $1")
            .bind(run_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    (run_status, outbox_status)
}

/// Attempt to re-emit the SAME outbox event (same `UNIQUE(org_id, idempotency_key)`).
/// Returns rows inserted — must be 0 on replay (spine outbox idempotency).
async fn reemit_outbox(
    pool: &PgPool,
    org: Uuid,
    run_id: Uuid,
    node_run_id: Uuid,
    idempotency_key: &str,
) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    arm(&mut tx, org).await;
    let inserted: Vec<Uuid> = sqlx::query_scalar(
        "INSERT INTO workflow_outbox_events \
             (org_id, run_id, node_run_id, channel, destination_ref, idempotency_key, status, payload) \
         VALUES ($1, $2, $3, 'JOB', 'internal.jobs', $4, 'PENDING', $5) \
         ON CONFLICT (org_id, idempotency_key) DO NOTHING \
         RETURNING id",
    )
    .bind(org)
    .bind(run_id)
    .bind(node_run_id)
    .bind(idempotency_key)
    .bind(payroll_job_payload())
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    inserted.len() as i64
}

/// Attempt to re-insert the draft under the SAME per-run natural key. Returns rows
/// inserted — must be 0 on replay (payroll draft idempotency).
async fn reinsert_draft(pool: &PgPool, org: Uuid, run_id: Uuid) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    arm(&mut tx, org).await;
    let inserted: Vec<Uuid> = sqlx::query_scalar(
        "INSERT INTO payroll_draft_runs \
             (org_id, period_start, period_end, source_label, status) \
         VALUES ($1, $2::date, $3::date, $4, 'BLOCKED_LEGAL_GATE') \
         ON CONFLICT (org_id, period_start, period_end, source_label) DO NOTHING \
         RETURNING id",
    )
    .bind(org)
    .bind(PERIOD_START)
    .bind(PERIOD_END)
    .bind(format!("workflow_runtime_m2:run:{run_id}"))
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    inserted.len() as i64
}

// ===========================================================================
// THE FLAG-ON RUNTIME GATE.
// ===========================================================================
#[sqlx::test(migrations = "./migrations")]
async fn flag_on_runtime_drives_one_run_and_one_blocked_legal_gate_draft(pool: PgPool) {
    // --- Base fixtures: two tenants; only TEST_TENANT is enrolled. -----------
    seed_org(&pool, TEST_TENANT, "flagon").await;
    seed_org(&pool, OTHER_TENANT, "other").await;

    // The dark default is proven elsewhere; here we deliberately enroll the TEST
    // tenant (and only it) and assert the flag genuinely resolves ON.
    enroll_tenant(&pool, TEST_TENANT).await;
    assert!(
        strangler_enabled(&pool, TEST_TENANT).await,
        "the enrolled TEST tenant must resolve workflow_runtime_m2_strangler = ON"
    );
    assert!(
        !strangler_enabled(&pool, OTHER_TENANT).await,
        "the un-enrolled tenant must stay OFF (per-tenant strangler; absent row ⇒ FALSE)"
    );

    // --- Drive ONE run→node FSM through the new runtime. ---------------------
    let (definition_id, version) = seed_definition(&pool, TEST_TENANT).await;
    let run_id = start_run(&pool, TEST_TENANT, definition_id, version).await;
    let (node_run_id, outbox_key) = process_payroll_node(&pool, TEST_TENANT, run_id).await;

    // Exactly one run, one node run, one JOB outbox event exist for the tenant.
    assert_eq!(
        count(&pool, TEST_TENANT, "SELECT count(*) FROM workflow_runs").await,
        1,
        "the FSM must create exactly ONE workflow_runs row"
    );
    assert_eq!(
        count(
            &pool,
            TEST_TENANT,
            "SELECT count(*) FROM workflow_node_runs"
        )
        .await,
        1,
        "the FSM must create exactly ONE workflow_node_runs row"
    );
    assert_eq!(
        count(
            &pool,
            TEST_TENANT,
            "SELECT count(*) FROM workflow_outbox_events WHERE channel = 'JOB'"
        )
        .await,
        1,
        "the payroll node must emit exactly ONE JOB outbox event"
    );
    // No draft exists yet — the drainer is what creates it.
    assert_eq!(
        count(
            &pool,
            TEST_TENANT,
            "SELECT count(*) FROM payroll_draft_runs"
        )
        .await,
        0,
        "no payroll draft exists before the outbox is drained"
    );

    // --- Drain: create exactly ONE BLOCKED_LEGAL_GATE draft. -----------------
    let created = drain_payroll_outbox(&pool, TEST_TENANT).await;
    assert_eq!(
        created, 1,
        "the first drain must stage exactly ONE payroll draft"
    );

    assert_eq!(
        count(
            &pool,
            TEST_TENANT,
            "SELECT count(*) FROM payroll_draft_runs"
        )
        .await,
        1,
        "exactly ONE payroll_draft_runs row exists after the drain"
    );
    let (status, source_label, calculation_enabled) =
        payroll_draft(&pool, TEST_TENANT, run_id).await;
    assert_eq!(
        status, "BLOCKED_LEGAL_GATE",
        "the drained draft must land status BLOCKED_LEGAL_GATE"
    );
    assert_eq!(
        source_label,
        format!("workflow_runtime_m2:run:{run_id}"),
        "the draft's natural key must be the deterministic per-run source_label"
    );
    assert!(
        !calculation_enabled,
        "a BLOCKED_LEGAL_GATE draft must keep calculation disabled (fails closed)"
    );

    // The run lands SUCCEEDED (matching production emit_payroll); outbox DELIVERED.
    let (run_status, outbox_status) = run_and_outbox_status(&pool, TEST_TENANT, run_id).await;
    assert_eq!(
        run_status, "SUCCEEDED",
        "the run must land SUCCEEDED after emitting the payroll job (production emit_payroll sets run_target=SUCCEEDED)"
    );
    assert_eq!(
        outbox_status, "DELIVERED",
        "the drained JOB outbox event must be marked DELIVERED"
    );

    // --- Idempotency: replay adds ZERO rows via three independent guards. ----
    // (1) Replaying the whole drain: the event is DELIVERED, so nothing is claimed.
    assert_eq!(
        drain_payroll_outbox(&pool, TEST_TENANT).await,
        0,
        "replaying the drain must create ZERO additional payroll drafts"
    );
    // (2) Re-emitting the outbox event collides on UNIQUE(org_id, idempotency_key).
    assert_eq!(
        reemit_outbox(&pool, TEST_TENANT, run_id, node_run_id, &outbox_key).await,
        0,
        "re-emitting the same JOB outbox event must insert ZERO rows (spine idempotency)"
    );
    // (3) Re-inserting the draft collides on the reused payroll natural key.
    assert_eq!(
        reinsert_draft(&pool, TEST_TENANT, run_id).await,
        0,
        "re-inserting the draft under the same natural key must insert ZERO rows"
    );

    // Steady state is unchanged: exactly one run, one JOB event, one draft.
    assert_eq!(
        count(&pool, TEST_TENANT, "SELECT count(*) FROM workflow_runs").await,
        1,
        "still exactly ONE workflow_runs row after replay"
    );
    assert_eq!(
        count(
            &pool,
            TEST_TENANT,
            "SELECT count(*) FROM workflow_outbox_events WHERE channel = 'JOB'"
        )
        .await,
        1,
        "still exactly ONE JOB outbox event after replay"
    );
    assert_eq!(
        count(
            &pool,
            TEST_TENANT,
            "SELECT count(*) FROM payroll_draft_runs"
        )
        .await,
        1,
        "still exactly ONE payroll_draft_runs row after replay (idempotent drain)"
    );

    // --- Tenant scoping: the un-enrolled tenant sees NONE of this state. -----
    // RLS keys off app.current_org, so the OTHER tenant's armed GUC selects zero
    // of the TEST tenant's runtime rows — the flag-ON path is test-tenant only.
    assert_eq!(
        count(&pool, OTHER_TENANT, "SELECT count(*) FROM workflow_runs").await,
        0,
        "the un-enrolled tenant must see ZERO workflow_runs rows (runtime state is tenant-scoped)"
    );
    assert_eq!(
        count(
            &pool,
            OTHER_TENANT,
            "SELECT count(*) FROM payroll_draft_runs"
        )
        .await,
        0,
        "the un-enrolled tenant must see ZERO payroll_draft_runs rows"
    );
}
// ===========================================================================
// OUTBOX DRAINER TRANSACTIONALLY IDEMPOTENT.
//
// The distinct invariant proven here (vs the FSM E2E above): the drainer's
// consume is ONE `with_audits`/consume transaction. The ON CONFLICT DO NOTHING
// draft insert (reused UNIQUE(org_id, period_start, period_end, source_label),
// source_label `workflow_runtime_m2:run:{run_id}`), the outbox DELIVERED update,
// AND the audit row all share that single txn, so:
//   * a ROLLED-BACK drain persists NONE of them (all-or-nothing atomicity) and
//     leaves the event PENDING for a later real drain; and
//   * a committed drain, replayed any number of times, writes ZERO additional
//     rows — the payroll_draft_runs count stays exactly 1 across drains.
// Everything runs as the real non-owner `console_rt` role with `app.current_org`
// armed; no new tables are created (spine/payroll/strangler reuse only).
// ===========================================================================
#[sqlx::test(migrations = "./migrations")]
async fn drainer_consume_is_one_atomic_txn_and_idempotent_across_replays(pool: PgPool) {
    // One run→node FSM emitting a single PENDING JOB payroll outbox event.
    seed_org(&pool, TEST_TENANT, "drainer").await;
    let (definition_id, version) = seed_definition(&pool, TEST_TENANT).await;
    let run_id = start_run(&pool, TEST_TENANT, definition_id, version).await;
    let (_node_run_id, _outbox_key) = process_payroll_node(&pool, TEST_TENANT, run_id).await;

    const DRAFT_COUNT: &str = "SELECT count(*) FROM payroll_draft_runs";
    // The audit rows the consume txn lands (with_audits): one per event actually
    // consumed. A DELIVERED event is never re-claimed, so a replay adds none.
    const DRAIN_AUDIT_COUNT: &str =
        "SELECT count(*) FROM audit_events WHERE action = 'workflow_runtime.outbox_drain'";

    // Preconditions: one PENDING JOB event, and NOTHING the drain would write yet.
    assert_eq!(
        count(&pool, TEST_TENANT, DRAFT_COUNT).await,
        0,
        "no payroll draft exists before the outbox is drained"
    );
    assert_eq!(
        count(&pool, TEST_TENANT, DRAIN_AUDIT_COUNT).await,
        0,
        "no drain audit row exists before the outbox is drained"
    );
    let (_run_status, outbox_before) = run_and_outbox_status(&pool, TEST_TENANT, run_id).await;
    assert_eq!(
        outbox_before, "PENDING",
        "the emitted JOB outbox event is PENDING before any drain"
    );

    // --- (1) ATOMICITY: the consume's three writes share ONE txn. -----------
    // The body WOULD stage one draft, but rolling back the shared txn persists
    // NONE of it: zero drafts, zero audit rows, and the event stays PENDING.
    let would_create = drain_then_rollback(&pool, TEST_TENANT).await;
    assert_eq!(
        would_create, 1,
        "the drain body must stage exactly ONE draft inside the txn (before rollback)"
    );
    assert_eq!(
        count(&pool, TEST_TENANT, DRAFT_COUNT).await,
        0,
        "rolling back the shared drain txn must persist ZERO payroll drafts (all-or-nothing atomicity)"
    );
    assert_eq!(
        count(&pool, TEST_TENANT, DRAIN_AUDIT_COUNT).await,
        0,
        "rolling back the shared drain txn must persist ZERO audit rows (the audit shares the state change's fate)"
    );
    let (_run_status, outbox_after_rollback) =
        run_and_outbox_status(&pool, TEST_TENANT, run_id).await;
    assert_eq!(
        outbox_after_rollback, "PENDING",
        "a rolled-back drain must leave the event PENDING (the DELIVERED ack rolled back with the insert)"
    );

    // --- (2) COMMITTED DRAIN: exactly ONE draft + ONE audit row, DELIVERED. --
    let created = drain_payroll_outbox(&pool, TEST_TENANT).await;
    assert_eq!(
        created, 1,
        "the committed drain must stage exactly ONE payroll draft"
    );
    assert_eq!(
        count(&pool, TEST_TENANT, DRAFT_COUNT).await,
        1,
        "exactly ONE payroll_draft_runs row exists after the committed drain"
    );
    assert_eq!(
        count(&pool, TEST_TENANT, DRAIN_AUDIT_COUNT).await,
        1,
        "the committed consume lands exactly ONE audit row in the same txn (with_audits)"
    );
    let (status, source_label, calculation_enabled) =
        payroll_draft(&pool, TEST_TENANT, run_id).await;
    assert_eq!(
        status, "BLOCKED_LEGAL_GATE",
        "the drained draft must land status BLOCKED_LEGAL_GATE"
    );
    assert_eq!(
        source_label,
        format!("workflow_runtime_m2:run:{run_id}"),
        "the draft's natural key must be the deterministic per-run source_label"
    );
    assert!(
        !calculation_enabled,
        "a BLOCKED_LEGAL_GATE draft must keep calculation disabled (fails closed)"
    );
    let (_run_status, outbox_delivered) = run_and_outbox_status(&pool, TEST_TENANT, run_id).await;
    assert_eq!(
        outbox_delivered, "DELIVERED",
        "the committed drain marks the JOB outbox event DELIVERED"
    );

    // --- (3) IDEMPOTENT REPLAY: replays add ZERO rows; counts stay 1. -------
    for _ in 0..2 {
        assert_eq!(
            drain_payroll_outbox(&pool, TEST_TENANT).await,
            0,
            "replaying the committed drain must create ZERO additional payroll drafts"
        );
    }
    // Re-inserting the draft under the same natural key is a no-op too.
    assert_eq!(
        reinsert_draft(&pool, TEST_TENANT, run_id).await,
        0,
        "re-inserting the draft under the same natural key must insert ZERO rows"
    );
    assert_eq!(
        count(&pool, TEST_TENANT, DRAFT_COUNT).await,
        1,
        "still exactly ONE payroll_draft_runs row after replay (count stays 1 across drains)"
    );
    assert_eq!(
        count(&pool, TEST_TENANT, DRAIN_AUDIT_COUNT).await,
        1,
        "still exactly ONE drain audit row after replay (the DELIVERED event is never re-consumed)"
    );
}
