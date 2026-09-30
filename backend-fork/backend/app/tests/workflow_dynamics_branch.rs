#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-AUTO slice 2 E2E — condition/branch node runtime against the REAL engine
//! on a genuine `PgWorkflowRuntimeStore`, driven as the non-owner `console_rt` role
//! (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) with `app.current_org` armed.
//!
//! Proves the branch walk: a `condition` node's predicate over the run context
//! selects the outgoing `when` edge; each outcome drives a DIFFERENT path; and
//! the untaken (dead) branch NEVER executes — no node run row is written for it.
//! Cross-org isolation of the produced runs is asserted as `console_rt`.

use console_kernel_core::{OrgId, TraceContext};
use console_workflow_domain::TriggerType;
use console_workflow_runtime::{AuditContext, StartRunRequest, TriggeredStart, start_bound_run};
use console_workflow_runtime_adapter_postgres::PgWorkflowRuntimeStore;
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

const BRANCH_TENANT: Uuid = Uuid::from_u128(0x4d32_11d1_0000_0000_0000_0000_0000_00d1);
const OTHER_TENANT: Uuid = Uuid::from_u128(0x4d32_11d2_0000_0000_0000_0000_0000_00d2);

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(6)
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

async fn seed_org(owner_pool: &PgPool, org: OrgId, slug: &str) {
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
}

/// gate → decide(amount > 1000) → [true] escalate.exec (human) / [false] auto.approve (terminal)
fn branching_definition() -> Value {
    json!({
        "schema_version": "wf.exec.v1",
        "nodes": [
            {"node_key": "gate", "node_type": "object_gate"},
            {"node_key": "decide", "node_type": "condition",
             "predicate": {"field": "amount", "op": "gt", "value": 1000}},
            {"node_key": "escalate.exec", "node_type": "human_task",
             "assignee_role_key": "executive", "required_policy": "approval_decide"},
            {"node_key": "auto.approve", "node_type": "object_mutation"}
        ],
        "edges": [
            {"from": "gate", "to": "decide"},
            {"from": "decide", "to": "escalate.exec", "when": "true"},
            {"from": "decide", "to": "auto.approve", "when": "false"}
        ]
    })
}

async fn seed_branching_definition(rt_pool: &PgPool, org: OrgId) -> Uuid {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let definition_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_definitions \
             (org_id, workflow_key, display_name, object_type, status, latest_version, active_version) \
         VALUES ($1, 'automation.branching', 'Branching', 'work_order', 'ACTIVE', 1, 1) \
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
         VALUES ($1, $2, 1, 'PUBLISHED', $3, FALSE, FALSE)",
    )
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(branching_definition())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    definition_id
}

fn audit() -> AuditContext {
    // System-initiated fire (no per-request principal), like the trigger/schedule
    // producers — avoids an actor FK to a seeded user.
    AuditContext {
        actor: None,
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    }
}

fn request(org: OrgId, definition_id: Uuid, key: &str, context: Value) -> StartRunRequest {
    StartRunRequest {
        run_id: Uuid::new_v4(),
        org_id: org,
        definition_id,
        definition_version: 1,
        trigger_type: TriggerType::Manual,
        object_type: Some("work_order".to_owned()),
        object_id: Some(Uuid::new_v4()),
        idempotency_key: key.to_owned(),
        correlation_id: format!("branch-test:{key}"),
        trace_id: None,
        input_payload: json!({}),
        context_payload: context,
        initiated_by: None,
        schedule_id: None,
    }
}

async fn node_keys(rt_pool: &PgPool, org: OrgId, run_id: Uuid) -> Vec<String> {
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, org).await;
    let rows = sqlx::query(
        "SELECT node_key FROM workflow_node_runs WHERE run_id = $1 ORDER BY started_at",
    )
    .bind(run_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    rows.iter()
        .map(|r| r.get::<String, _>("node_key"))
        .collect()
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn condition_true_branch_parks_and_dead_branch_never_runs(owner_pool: PgPool) {
    let org = OrgId::from_uuid(BRANCH_TENANT);
    seed_org(&owner_pool, org, "org-branch").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let definition_id = seed_branching_definition(&rt_pool, org).await;
    let store = PgWorkflowRuntimeStore::new(rt_pool.clone());

    // amount > 1000 ⇒ TRUE branch ⇒ parks at the executive human task.
    let req = request(
        org,
        definition_id,
        "branch-true-0000000001",
        json!({ "amount": 5000 }),
    );
    let run_id = req.run_id;
    let outcome = start_bound_run(&store, req, &branching_definition(), &audit())
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        TriggeredStart::Started { run_status, .. } if run_status == console_workflow_domain::RunStatus::Waiting
    ));

    let keys = node_keys(&rt_pool, org, run_id).await;
    assert!(keys.contains(&"gate".to_owned()));
    assert!(keys.contains(&"decide".to_owned()));
    assert!(keys.contains(&"escalate.exec".to_owned()));
    assert!(
        !keys.contains(&"auto.approve".to_owned()),
        "the dead (false) branch must never execute; node runs = {keys:?}"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn condition_false_branch_runs_to_terminal_success(owner_pool: PgPool) {
    let org = OrgId::from_uuid(BRANCH_TENANT);
    seed_org(&owner_pool, org, "org-branch").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let definition_id = seed_branching_definition(&rt_pool, org).await;
    let store = PgWorkflowRuntimeStore::new(rt_pool.clone());

    // amount <= 1000 ⇒ FALSE branch ⇒ drives to the terminal object_mutation.
    let req = request(
        org,
        definition_id,
        "branch-false-000000001",
        json!({ "amount": 100 }),
    );
    let run_id = req.run_id;
    let outcome = start_bound_run(&store, req, &branching_definition(), &audit())
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        TriggeredStart::Started { run_status, .. } if run_status == console_workflow_domain::RunStatus::Succeeded
    ));

    let keys = node_keys(&rt_pool, org, run_id).await;
    assert!(keys.contains(&"gate".to_owned()));
    assert!(keys.contains(&"decide".to_owned()));
    assert!(keys.contains(&"auto.approve".to_owned()));
    assert!(
        !keys.contains(&"escalate.exec".to_owned()),
        "the dead (true) branch must never execute; node runs = {keys:?}"
    );

    // Cross-org isolation: another tenant sees none of these runs.
    let other = OrgId::from_uuid(OTHER_TENANT);
    seed_org(&owner_pool, other, "org-branch-other").await;
    let mut tx = rt_pool.begin().await.unwrap();
    arm_org(&mut tx, other).await;
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow_runs")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(visible, 0, "runs must be RLS-isolated to their own tenant");
}
