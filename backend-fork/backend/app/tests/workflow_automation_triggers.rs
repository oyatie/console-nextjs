#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-AUTO slice 1 E2E — event trigger bindings + cron schedule poller against
//! the REAL runtime engine on a genuine `PgWorkflowRuntimeStore`.
//!
//! ## What this proves
//! 1. an ENABLED `workflow_trigger_bindings` rule fires: dispatching the
//!    registered `work_order.completed` event starts EXACTLY one run of the
//!    bound `wf.exec.v1` definition (trigger_type OBJECT_EVENT, driven to
//!    SUCCEEDED through the graph walk), and RE-dispatching the same event
//!    occurrence adds ZERO runs (deterministic `trigger:{binding}:{object}`
//!    key against the run spine's `UNIQUE(org_id, idempotency_key)`);
//! 2. a DISABLED binding does NOT fire — zero runs;
//! 3. the schedule poller starts a due schedule's run EXACTLY once under
//!    CONCURRENT polling (two `poll_org` passes racing on the same due fire),
//!    stamps the run's `schedule_id` provenance + `SCHEDULE` trigger type, and
//!    advances `next_run_at` into the future exactly once (the guarded
//!    advance); a follow-up poll starts nothing.
//!
//! Cron next-fire correctness and garbage rejection are unit-tested in
//! `console_app::workflow_schedules`.
//!
//! ## Runtime fidelity (mandatory)
//! Every runtime write + read runs as the genuine non-owner `console_rt` role
//! (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) with `app.current_org` armed — never
//! a BYPASSRLS superuser. Only minting the `organizations`/`users` rows
//! (owner-only surfaces for `console_rt`) uses the owner pool; binding/schedule
//! seeds, the dispatch/poll, and every assertion read execute as `console_rt`
//! under the armed tenant GUC, exactly as production does.

use console_app::workflow_schedules::poll_org;
use console_kernel_core::OrgId;
use console_workflow_runtime_adapter_postgres::PgWorkflowRuntimeStore;
use console_workorder_rest::workflow_triggers::{
    WORK_ORDER_COMPLETED_EVENT, dispatch_event_bindings,
};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

/// Dedicated TEST tenants (fresh, empty sqlx::test DB — never production).
const BINDING_TENANT: Uuid = Uuid::from_u128(0x4d32_11c5_0000_0000_0000_0000_0000_00c5);
const DISABLED_TENANT: Uuid = Uuid::from_u128(0x4d32_11c6_0000_0000_0000_0000_0000_00c6);
const SCHEDULE_TENANT: Uuid = Uuid::from_u128(0x4d32_11c7_0000_0000_0000_0000_0000_00c7);

// ===========================================================================
// Runtime-role pool + seeds (pattern copied from m2_real_engine_drive.rs).
// ===========================================================================

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(8)
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

async fn arm_org(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, org: OrgId) {
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(&mut **tx)
        .await
        .unwrap();
}

/// Mint the org + one user via the OWNER pool (`console_rt` is SELECT-only there).
/// The user backs the bindings/schedules `created_by` FK.
async fn seed_org_and_author(owner_pool: &PgPool, org: OrgId, slug: &str) -> Uuid {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(*org.as_uuid())
    .bind(slug)
    .bind(format!("Org {slug}"))
    .execute(owner_pool)
    .await
    .unwrap();
    let author_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(author_id)
        .bind("automation author")
        .bind(vec!["ADMIN".to_owned()])
        .bind(*org.as_uuid())
        .execute(owner_pool)
        .await
        .unwrap();
    author_id
}

/// Seed an ACTIVE `wf.exec.v1` definition with a minimal single-node graph
/// (one `object_gate` → terminal, so a triggered run drives straight to
/// SUCCEEDED), as `console_rt` under the armed tenant GUC.
async fn seed_graph_definition(rt_pool: &PgPool, org: OrgId, key: &str) -> Uuid {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let definition_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_definitions \
             (org_id, workflow_key, display_name, object_type, status, \
              latest_version, active_version) \
         VALUES ($1, $2, 'Automation Target', 'work_order', 'ACTIVE', 1, 1) \
         RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(key)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_definition_versions \
             (org_id, definition_id, version, status, definition, \
              required_approval_line, required_payment_line) \
         VALUES ($1, $2, 1, 'PUBLISHED', $3, FALSE, FALSE)",
    )
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(serde_json::json!({
        "schema_version": "wf.exec.v1",
        "nodes": [
            {"node_key": "record_event", "node_type": "object_gate"}
        ],
        "edges": []
    }))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    definition_id
}

/// Seed a trigger binding as `console_rt` under the armed tenant GUC.
async fn seed_binding(
    rt_pool: &PgPool,
    org: OrgId,
    definition_id: Uuid,
    author: Uuid,
    enabled: bool,
) -> Uuid {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_trigger_bindings \
             (org_id, definition_id, trigger_type, event_key, enabled, \
              created_by, updated_by) \
         VALUES ($1, $2, 'OBJECT_EVENT', $3, $4, $5, $5) \
         RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(WORK_ORDER_COMPLETED_EVENT)
    .bind(enabled)
    .bind(author)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

/// Seed a schedule due at `next_run_at`, as `console_rt` under the armed GUC.
async fn seed_schedule(
    rt_pool: &PgPool,
    org: OrgId,
    definition_id: Uuid,
    author: Uuid,
    next_run_at: OffsetDateTime,
) -> Uuid {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_schedules \
             (org_id, label, cron_expr, timezone, definition_id, enabled, \
              next_run_at, created_by, updated_by) \
         VALUES ($1, '매일 아침 자동 상신', '0 9 * * *', 'Asia/Seoul', $2, TRUE, $3, $4, $4) \
         RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(next_run_at)
    .bind(author)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

/// Tenant-scoped scalar read as `console_rt` under the armed GUC.
async fn count(rt_pool: &PgPool, org: OrgId, query: &'static str) -> i64 {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let n: i64 = sqlx::query_scalar(query).fetch_one(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    n
}

// ===========================================================================
// 1. Enabled binding fires exactly one run per event occurrence.
// ===========================================================================

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn enabled_binding_fires_exactly_one_run_per_event(owner_pool: PgPool) {
    let org = OrgId::from_uuid(BINDING_TENANT);
    let author = seed_org_and_author(&owner_pool, org, "org-auto-binding").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let definition_id = seed_graph_definition(&rt_pool, org, "automation.on_completion").await;
    let binding_id = seed_binding(&rt_pool, org, definition_id, author, true).await;
    let store = PgWorkflowRuntimeStore::new(rt_pool.clone());
    let work_order = Uuid::new_v4();

    let started = dispatch_event_bindings(
        &store,
        org,
        None,
        WORK_ORDER_COMPLETED_EVENT,
        "work_order",
        work_order,
    )
    .await
    .unwrap();
    assert_eq!(started, 1, "the enabled binding must start exactly one run");

    // The run is real engine output: OBJECT_EVENT provenance, driven through
    // the single-node graph to SUCCEEDED, one node run recorded.
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let run = sqlx::query(
        "SELECT status, trigger_type, definition_id, object_type, object_id, \
                idempotency_key, schedule_id \
         FROM workflow_runs",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(run.get::<String, _>("status"), "SUCCEEDED");
    assert_eq!(run.get::<String, _>("trigger_type"), "OBJECT_EVENT");
    assert_eq!(run.get::<Uuid, _>("definition_id"), definition_id);
    assert_eq!(
        run.get::<Option<String>, _>("object_type").as_deref(),
        Some("work_order")
    );
    assert_eq!(run.get::<Option<Uuid>, _>("object_id"), Some(work_order));
    assert_eq!(
        run.get::<String, _>("idempotency_key"),
        format!("trigger:{binding_id}:{work_order}")
    );
    assert_eq!(run.get::<Option<Uuid>, _>("schedule_id"), None);
    tx.commit().await.unwrap();
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_node_runs").await,
        1
    );

    // Re-publishing the SAME event occurrence (crash-replay / double commit
    // point) is a no-op: zero new runs.
    let started_again = dispatch_event_bindings(
        &store,
        org,
        None,
        WORK_ORDER_COMPLETED_EVENT,
        "work_order",
        work_order,
    )
    .await
    .unwrap();
    assert_eq!(
        started_again, 0,
        "same event occurrence must not double-fire"
    );
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_runs").await,
        1
    );

    // A DIFFERENT occurrence (another work order completing) fires again.
    let other = Uuid::new_v4();
    let started_other = dispatch_event_bindings(
        &store,
        org,
        None,
        WORK_ORDER_COMPLETED_EVENT,
        "work_order",
        other,
    )
    .await
    .unwrap();
    assert_eq!(started_other, 1);
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_runs").await,
        2
    );
}

// ===========================================================================
// 2. Disabled binding does not fire.
// ===========================================================================

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn disabled_binding_does_not_fire(owner_pool: PgPool) {
    let org = OrgId::from_uuid(DISABLED_TENANT);
    let author = seed_org_and_author(&owner_pool, org, "org-auto-disabled").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let definition_id = seed_graph_definition(&rt_pool, org, "automation.on_completion").await;
    seed_binding(&rt_pool, org, definition_id, author, false).await;
    let store = PgWorkflowRuntimeStore::new(rt_pool.clone());

    let started = dispatch_event_bindings(
        &store,
        org,
        None,
        WORK_ORDER_COMPLETED_EVENT,
        "work_order",
        Uuid::new_v4(),
    )
    .await
    .unwrap();
    assert_eq!(started, 0, "a disabled binding must not fire");
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_runs").await,
        0
    );
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_node_runs").await,
        0
    );
}

// ===========================================================================
// 3. Schedule poller: exactly once under concurrent poll, guarded advance.
// ===========================================================================

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn schedule_poller_starts_due_run_exactly_once_under_concurrent_poll(owner_pool: PgPool) {
    let org = OrgId::from_uuid(SCHEDULE_TENANT);
    let author = seed_org_and_author(&owner_pool, org, "org-auto-schedule").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let definition_id = seed_graph_definition(&rt_pool, org, "automation.daily").await;
    let now = OffsetDateTime::now_utc();
    // Due one minute ago, at whole-second precision so the value round-trips
    // Postgres' microsecond timestamptz exactly.
    let due_fire = OffsetDateTime::from_unix_timestamp(now.unix_timestamp() - 60).unwrap();
    let schedule_id = seed_schedule(&rt_pool, org, definition_id, author, due_fire).await;
    let store = PgWorkflowRuntimeStore::new(rt_pool.clone());

    // Two pollers race on the same due fire (the concurrent-poll idempotency
    // claim: deterministic run key + guarded advance).
    let (a, b) = tokio::join!(poll_org(&store, org, now), poll_org(&store, org, now));
    let started = a.unwrap() + b.unwrap();
    assert_eq!(started, 1, "concurrent polls must start exactly one run");
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_runs").await,
        1
    );

    // The run carries schedule provenance and the reserved SCHEDULE trigger.
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let run = sqlx::query(
        "SELECT status, trigger_type, schedule_id, idempotency_key, initiated_by \
         FROM workflow_runs",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(run.get::<String, _>("status"), "SUCCEEDED");
    assert_eq!(run.get::<String, _>("trigger_type"), "SCHEDULE");
    assert_eq!(run.get::<Option<Uuid>, _>("schedule_id"), Some(schedule_id));
    assert_eq!(
        run.get::<String, _>("idempotency_key"),
        format!("schedule:{schedule_id}:{}", due_fire.unix_timestamp())
    );
    assert_eq!(
        run.get::<Option<Uuid>, _>("initiated_by"),
        None,
        "system fire has no actor"
    );

    // The schedule advanced exactly once: next_run_at strictly in the future
    // (next 09:00 KST after now), last_run_at = the claimed fire.
    let schedule = sqlx::query(
        "SELECT next_run_at, last_run_at, last_status FROM workflow_schedules WHERE id = $1",
    )
    .bind(schedule_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let next: Option<OffsetDateTime> = schedule.get("next_run_at");
    assert!(
        next.unwrap() > now,
        "next_run_at must advance into the future"
    );
    assert_eq!(
        schedule.get::<Option<OffsetDateTime>, _>("last_run_at"),
        Some(due_fire)
    );
    let last_status: Option<String> = schedule.get("last_status");
    assert!(
        matches!(last_status.as_deref(), Some("STARTED") | Some("SKIPPED")),
        "last_status records the racing pollers' outcome, got {last_status:?}"
    );
    tx.commit().await.unwrap();

    // Nothing is due any more: a follow-up poll starts nothing and adds no rows.
    let again = poll_org(&store, org, OffsetDateTime::now_utc())
        .await
        .unwrap();
    assert_eq!(again, 0);
    assert_eq!(
        count(&rt_pool, org, "SELECT count(*) FROM workflow_runs").await,
        1
    );
}
