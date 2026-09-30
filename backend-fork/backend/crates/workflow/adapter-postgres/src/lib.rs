//! Postgres adapter for the workflow runtime spine.
//!
//! Implements [`WorkflowRuntimePort`] over migrations 0077/0078 using the platform
//! transactional helpers. Every method arms `app.current_org` before any statement
//! (via `with_audit` / `with_audits` from `event.org_id`/`org`, or `with_org_conn`
//! for reads) so RLS scopes it to the tenant exactly as production `console_rt`. This
//! is the ONLY workflow crate that touches the spine tables and `sqlx`. Runtime
//! `sqlx::query()` is used throughout (not the compile-time macros) because the
//! offline query cache cannot be regenerated in this environment.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::BTreeSet;

use console_kernel_core::{AuditAction, AuditEvent, KernelError, OrgId, TraceContext, UserId};
use console_notifications_application::{EmitNotificationCommand, NotificationSink};
use console_notifications_domain::NotificationLink;
use console_platform_db::{DbError, with_audit, with_audits, with_org_conn};
use console_workflow_domain::{
    FinalizeWaitingTaskCommand, FinalizeWaitingTaskContext, FinalizedWaitingTask, NewRun,
    NodeStepCommit, PayrollDraftStaging, PortFuture, PostFinalizationRejection,
    PostFinalizationRejectionCommand, RunRecord, RunStatus, RunTerminalTimestamp, RunTransition,
    StagePayrollDraft, WaitingTaskStatus, WorkflowRuntimePort,
};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// The per-tenant strangler flag (migration 0095) that routes a tenant through
/// the M2 workflow runtime. Resolved via `org_runtime_flag_enabled`; an absent
/// row resolves to `false` (dark default / fail-closed).
pub const M2_STRANGLER_FLAG: &str = "workflow_runtime_m2_strangler";

/// Audit action stamped when the drainer consumes a JOB payroll outbox event.
/// Matches the `audit_events.action` regex (≥2 dot-separated `[a-z0-9_]` segments).
const DRAIN_AUDIT_ACTION: &str = "workflow_runtime.outbox_drain";
/// A NOTIFICATION outbox event that keeps failing is dead-lettered at this many
/// attempts so a poison pill (e.g. a recipient with no users row) stops being
/// re-claimed every tick forever.
const NOTIFICATION_DEAD_LETTER_ATTEMPTS: i32 = 10;
/// Hard ceiling on approval-line recipients per event; over-cap is dead-lettered
/// rather than stalling the tick. ponytail: 512 is far above any real approval
/// line; raise only if a legitimate fan-out ever approaches it.
const MAX_NOTIFICATION_RECIPIENTS: usize = 512;
const POST_FINALIZATION_REJECTION: &str = "POST_FINALIZATION_REJECTION";
const FINALIZE_WAITING_KEY: &str = "finalize.author";
const RECEIPT_WAITING_KEY: &str = "receipt.target";

/// Adapter error. `Db` wraps the platform DB error (so `with_audit`'s
/// `E: From<DbError>` bound is satisfied); `Domain` carries a kernel error raised
/// inside a closure (e.g. an unknown status decoded from a row).
#[derive(Debug, thiserror::Error)]
enum PgWorkflowRuntimeError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),
}

#[derive(Debug, Clone)]
struct ReceiptNodeSpec {
    title: String,
    required_policy: Option<String>,
    assignee_role_key: Option<String>,
}

/// One enabled `workflow_trigger_bindings` row (0105) as the event dispatcher
/// consumes it.
#[derive(Debug, Clone)]
pub struct TriggerBindingRow {
    pub id: Uuid,
    pub definition_id: Uuid,
    pub trigger_type: console_workflow_domain::TriggerType,
    pub event_key: String,
}

/// One due `workflow_schedules` row (0106) as the schedule poller consumes it.
#[derive(Debug, Clone)]
pub struct DueScheduleRow {
    pub id: Uuid,
    pub label: String,
    pub cron_expr: String,
    pub timezone: String,
    pub definition_id: Uuid,
    /// The fire this row is due for — the poller's claim token: the run's
    /// idempotency key derives from it and the advance UPDATE is guarded on it.
    pub next_run_at: time::OffsetDateTime,
}

/// Filter for the waiting-task inbox listings (`GET /api/v1/workflow-tasks`).
#[derive(Debug, Clone)]
pub struct WaitingTaskListFilter {
    /// Group-inbox role filter (`?role_key=`), matched against `assignee_role_key`.
    pub role_key: Option<String>,
    /// Personal-inbox mode (`?assignee=me`): the user's CLAIMED tasks plus claimable OPEN ones.
    pub assignee_me: bool,
    /// The authority role keys the caller holds (security M3). In personal-inbox
    /// mode, an OPEN task the caller has NOT claimed is only surfaced when its
    /// `assignee_role_key` is one of these (or it is an ownership task bound to the
    /// caller as run initiator). Empty ⇒ no role-queued OPEN tasks are surfaced.
    pub authority_role_keys: Vec<String>,
    /// Statuses to include (defaults to `[OPEN]` at the REST layer).
    pub statuses: Vec<WaitingTaskStatus>,
}

/// One waiting-task inbox row (approval inbox / group inbox).
#[derive(Debug, Clone)]
pub struct WaitingTaskListItem {
    pub task_id: Uuid,
    pub run_id: Uuid,
    pub waiting_key: String,
    pub title: String,
    pub assignee_role_key: Option<String>,
    pub required_policy: Option<String>,
    pub object_type: Option<String>,
    pub object_id: Option<Uuid>,
    pub status: WaitingTaskStatus,
    pub claimed_by: Option<Uuid>,
    pub due_at: Option<time::OffsetDateTime>,
    pub created_at: time::OffsetDateTime,
    pub form_payload: serde_json::Value,
}

/// Filter for the submission box (`GET /api/v1/workflow-runs/mine`).
#[derive(Debug, Clone)]
pub struct RunListFilter {
    pub statuses: Vec<RunStatus>,
    pub object_type: Option<String>,
    /// Free-text (`?q=`) case-insensitive substring search over the run's
    /// human-readable content. `None` (or empty) returns all rows. The caller must
    /// pass the raw search term already trimmed; LIKE metacharacters are escaped here.
    pub q: Option<String>,
}

/// Escape LIKE/ILIKE metacharacters (`\`, `%`, `_`) in a raw search term and wrap it
/// as a `%term%` substring pattern, so user input is matched literally (a `q` of
/// `50%` matches the text `50%`, not `50<anything>`). Postgres ILIKE's default escape
/// character is backslash, so no explicit `ESCAPE` clause is needed.
fn ilike_contains_pattern(raw: &str) -> String {
    let mut escaped = String::with_capacity(raw.len() + 2);
    escaped.push('%');
    for ch in raw.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped.push('%');
    escaped
}

/// Filter for the org-wide admin run list (`GET /api/v1/workflow-runs`).
/// Keyset-paginated over `(updated_at DESC, id DESC)` like the notifications list.
#[derive(Debug, Clone)]
pub struct AdminRunListFilter {
    /// Statuses to include; empty = all (incl. `FAILED`/`DEAD_LETTERED`).
    pub statuses: Vec<RunStatus>,
    /// Keyset cursor: return only rows strictly older than this run's
    /// `(updated_at, id)`. `None` starts at the newest row.
    pub before: Option<Uuid>,
    /// Page size (the caller clamps to a sane bound).
    pub limit: i64,
}

/// One submission-box row (a run the principal initiated).
#[derive(Debug, Clone)]
pub struct RunListItem {
    pub run_id: Uuid,
    pub status: RunStatus,
    pub definition_id: Uuid,
    pub definition_version: i32,
    pub object_type: Option<String>,
    pub object_id: Option<Uuid>,
    pub initiated_by: Option<Uuid>,
    pub started_at: time::OffsetDateTime,
    pub updated_at: time::OffsetDateTime,
}

/// Audited command to claim an OPEN waiting task (`POST .../claim`).
#[derive(Debug, Clone)]
pub struct ClaimWaitingTaskCommand {
    pub task_id: Uuid,
    pub actor: console_kernel_core::UserId,
    pub transition_audits: Vec<AuditEvent>,
}

/// Result of a claim: the task now CLAIMED by the actor (or a same-user replay).
#[derive(Debug, Clone)]
pub struct ClaimedWaitingTask {
    pub task_id: Uuid,
    pub run_id: Uuid,
    pub status: WaitingTaskStatus,
    pub claimed_by: Option<Uuid>,
    pub claimed_at: Option<time::OffsetDateTime>,
}

/// The decision a `decide` request carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskDecision {
    Approve,
    Reject,
    Return,
}

impl TaskDecision {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
            Self::Return => "return",
        }
    }
}

/// Audited command to decide a non-finalize waiting task (`POST .../decide`).
#[derive(Debug, Clone)]
pub struct DecideWaitingTaskCommand {
    pub task_id: Uuid,
    pub actor: console_kernel_core::UserId,
    pub decision: TaskDecision,
    pub comment: Option<String>,
    pub idempotency_key: String,
    pub transition_audits: Vec<AuditEvent>,
}

/// Result of a decision: the task's terminal decision status, the run's resulting
/// status, and the next parked task (when an approval advanced the line).
#[derive(Debug, Clone)]
pub struct DecidedWaitingTask {
    pub task_id: Uuid,
    pub run_id: Uuid,
    pub status: WaitingTaskStatus,
    pub decision_payload: serde_json::Value,
    pub run_status: RunStatus,
    pub next_task: Option<WaitingTaskListItem>,
}

/// The node following `from_key` along its single outgoing edge, parsed from the
/// `wf.exec.v1` definition JSON. Generalizes [`receipt_node_after_finalize`] for
/// the decision-advance path.
#[derive(Debug, Clone)]
struct NextNode {
    node_key: String,
    node_type: String,
    title: String,
    required_policy: Option<String>,
    assignee_role_key: Option<String>,
}

fn next_node_after(definition: &serde_json::Value, from_key: &str) -> Option<NextNode> {
    let to = definition
        .get("edges")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .find(|edge| edge.get("from").and_then(serde_json::Value::as_str) == Some(from_key))
        .and_then(|edge| edge.get("to").and_then(serde_json::Value::as_str))?;

    let node = definition
        .get("nodes")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .find(|node| node.get("node_key").and_then(serde_json::Value::as_str) == Some(to))?;

    Some(NextNode {
        node_key: to.to_owned(),
        node_type: node
            .get("node_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("human_task")
            .to_owned(),
        title: node
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(to)
            .to_owned(),
        required_policy: node
            .get("required_policy")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        assignee_role_key: node
            .get("assignee_role_key")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
    })
}

/// Park a human-task node as an OPEN waiting task inside an existing txn: insert the
/// WAITING node run + the OPEN waiting task (both `ON CONFLICT DO NOTHING` so a
/// replay is a no-op), returning `(node_run_id, task_id)`. Mirrors the receipt
/// insert in `finalize_waiting_task`.
async fn park_waiting_node(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org: OrgId,
    run_id: Uuid,
    node: &NextNode,
) -> Result<(Uuid, Uuid), PgWorkflowRuntimeError> {
    sqlx::query(
        "INSERT INTO workflow_node_runs \
             (id, org_id, run_id, node_key, node_type, status, attempt, \
              idempotency_key, input_payload, started_at) \
         VALUES ($1, $2, $3, $4, 'human_task', 'WAITING', 1, $5, '{}'::jsonb, now()) \
         ON CONFLICT (org_id, run_id, node_key, attempt) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(*org.as_uuid())
    .bind(run_id)
    .bind(node.node_key.as_str())
    .bind(format!(
        "workflow_runtime:node:{run_id}:{}:1",
        node.node_key
    ))
    .execute(tx.as_mut())
    .await
    .map_err(PgWorkflowRuntimeError::from)?;

    let node_run_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow_node_runs \
         WHERE run_id = $1 AND node_key = $2 AND attempt = 1",
    )
    .bind(run_id)
    .bind(node.node_key.as_str())
    .fetch_one(tx.as_mut())
    .await
    .map_err(PgWorkflowRuntimeError::from)?;

    let task_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_waiting_tasks \
             (id, org_id, run_id, node_run_id, waiting_key, title, status, \
              assignee_role_key, required_policy, form_payload) \
         VALUES ($1, $2, $3, $4, $5, $6, 'OPEN', $7, $8, '{}'::jsonb) \
         ON CONFLICT (org_id, run_id, waiting_key) DO NOTHING",
    )
    .bind(task_id)
    .bind(*org.as_uuid())
    .bind(run_id)
    .bind(node_run_id)
    .bind(node.node_key.as_str())
    .bind(node.title.as_str())
    .bind(node.assignee_role_key.as_deref())
    .bind(node.required_policy.as_deref())
    .execute(tx.as_mut())
    .await
    .map_err(PgWorkflowRuntimeError::from)?;

    let persisted_task_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow_waiting_tasks \
         WHERE run_id = $1 AND waiting_key = $2",
    )
    .bind(run_id)
    .bind(node.node_key.as_str())
    .fetch_one(tx.as_mut())
    .await
    .map_err(PgWorkflowRuntimeError::from)?;

    Ok((node_run_id, persisted_task_id))
}

impl From<sqlx::Error> for PgWorkflowRuntimeError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

impl From<PgWorkflowRuntimeError> for KernelError {
    fn from(err: PgWorkflowRuntimeError) -> Self {
        match err {
            PgWorkflowRuntimeError::Domain(kernel) => kernel,
            PgWorkflowRuntimeError::Db(db) => db_error_to_kernel(db),
        }
    }
}

fn db_error_to_kernel(db: DbError) -> KernelError {
    match &db {
        DbError::Sqlx(sqlx::Error::RowNotFound) => {
            return KernelError::not_found("workflow runtime row not found");
        }
        DbError::Sqlx(sqlx::Error::Database(database))
            if database.code().is_some_and(|code| code == "23505") =>
        {
            return KernelError::conflict("workflow runtime idempotency/unique conflict");
        }
        _ => {}
    }
    KernelError::internal(format!("workflow runtime db error: {db}"))
}

fn receipt_node_after_finalize(definition: &serde_json::Value) -> Option<ReceiptNodeSpec> {
    let has_finalize_to_receipt_edge = definition
        .get("edges")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|edges| {
            edges.iter().any(|edge| {
                edge.get("from").and_then(serde_json::Value::as_str) == Some(FINALIZE_WAITING_KEY)
                    && edge.get("to").and_then(serde_json::Value::as_str)
                        == Some(RECEIPT_WAITING_KEY)
            })
        });
    if !has_finalize_to_receipt_edge {
        return None;
    }

    let receipt_node = definition
        .get("nodes")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .find(|node| {
            node.get("node_key").and_then(serde_json::Value::as_str) == Some(RECEIPT_WAITING_KEY)
        })?;

    Some(ReceiptNodeSpec {
        title: receipt_node
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Receipt confirmation")
            .to_owned(),
        required_policy: receipt_node
            .get("required_policy")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        assignee_role_key: receipt_node
            .get("assignee_role_key")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
    })
}

/// Postgres-backed workflow runtime store.
#[derive(Clone)]
pub struct PgWorkflowRuntimeStore {
    pool: PgPool,
}

impl std::fmt::Debug for PgWorkflowRuntimeStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgWorkflowRuntimeStore")
            .field("pool", &self.pool)
            .finish()
    }
}

impl PgWorkflowRuntimeStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// STEP 4 — resolve the per-tenant strangler flag by calling the SQL resolver
    /// `org_runtime_flag_enabled(flag_key)` (migration 0095) under an armed
    /// `console_rt` connection (`with_org_conn` sets `app.current_org` so the
    /// SECURITY INVOKER resolver reads only this tenant's row). An absent flag row
    /// resolves to `false` — the dark default that keeps un-enrolled tenants on
    /// the legacy path.
    pub async fn strangler_flag_enabled(
        &self,
        org: OrgId,
        flag_key: &str,
    ) -> Result<bool, KernelError> {
        let flag_key = flag_key.to_owned();
        with_org_conn::<_, bool, PgWorkflowRuntimeError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let enabled: bool = sqlx::query_scalar("SELECT org_runtime_flag_enabled($1)")
                    .bind(flag_key)
                    .fetch_one(tx.as_mut())
                    .await
                    .map_err(PgWorkflowRuntimeError::from)?;
                Ok(enabled)
            })
        })
        .await
        .map_err(KernelError::from)
    }

    /// Load a run by its tenant-scoped natural `idempotency_key`
    /// (`UNIQUE(org_id, idempotency_key)`, 0077:34), read as `console_rt` under the
    /// armed `app.current_org` (via `with_org_conn`). Used by the completion-tail
    /// strangler to RESUME an existing (partial) run on the deterministic
    /// completion-key conflict instead of aborting: the caller derives the same
    /// `run_completion_key`, and this returns the run already recorded under it so
    /// the tail continues idempotently. `None` when no run exists for that key.
    pub async fn load_run_by_idempotency_key(
        &self,
        org: OrgId,
        idempotency_key: String,
    ) -> Result<Option<RunRecord>, KernelError> {
        with_org_conn::<_, Option<RunRecord>, PgWorkflowRuntimeError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT id, org_id, status, definition_id, definition_version, \
                            object_type, object_id \
                     FROM workflow_runs WHERE idempotency_key = $1",
                )
                .bind(idempotency_key)
                .fetch_optional(tx.as_mut())
                .await
                .map_err(PgWorkflowRuntimeError::from)?;

                let Some(row) = row else {
                    return Ok(None);
                };
                let status_str: String = row.try_get("status")?;
                let id: Uuid = row.try_get("id")?;
                let org_uuid: Uuid = row.try_get("org_id")?;
                let definition_id: Uuid = row.try_get("definition_id")?;
                let definition_version: i32 = row.try_get("definition_version")?;
                let object_type: Option<String> = row.try_get("object_type")?;
                let object_id: Option<Uuid> = row.try_get("object_id")?;
                Ok(Some(RunRecord {
                    id,
                    org_id: OrgId::from_uuid(org_uuid),
                    status: RunStatus::from_db_str(&status_str)?,
                    definition_id,
                    definition_version,
                    object_type,
                    object_id,
                }))
            })
        })
        .await
        .map_err(KernelError::from)
    }

    /// Enabled trigger bindings for one registered domain event key (0105),
    /// read as `console_rt` under the armed `app.current_org`. The dispatcher calls
    /// this at an audited-mutation commit point; ordering is stable (oldest
    /// binding first) so evaluation order is deterministic.
    pub async fn list_enabled_trigger_bindings(
        &self,
        org: OrgId,
        event_key: &str,
    ) -> Result<Vec<TriggerBindingRow>, KernelError> {
        let event_key = event_key.to_owned();
        with_org_conn::<_, Vec<TriggerBindingRow>, PgWorkflowRuntimeError>(
            &self.pool,
            org,
            move |tx| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "SELECT id, definition_id, trigger_type, event_key \
                         FROM workflow_trigger_bindings \
                         WHERE event_key = $1 AND enabled \
                         ORDER BY created_at ASC",
                    )
                    .bind(event_key)
                    .fetch_all(tx.as_mut())
                    .await
                    .map_err(PgWorkflowRuntimeError::from)?;
                    rows.into_iter()
                        .map(|row| {
                            let trigger_type: String = row.try_get("trigger_type")?;
                            Ok(TriggerBindingRow {
                                id: row.try_get("id")?,
                                definition_id: row.try_get("definition_id")?,
                                trigger_type: console_workflow_domain::TriggerType::from_db_str(
                                    &trigger_type,
                                )
                                .map_err(PgWorkflowRuntimeError::Domain)?,
                                event_key: row.try_get("event_key")?,
                            })
                        })
                        .collect()
                })
            },
        )
        .await
        .map_err(KernelError::from)
    }

    /// Resolve a definition's ACTIVE executable (`wf.exec.v1`) version:
    /// `(active_version, definition JSON)`. `None` when the definition does not
    /// exist, is not ACTIVE, has no active version, or its active version is not
    /// `wf.exec.v1` — the trigger/schedule caller SKIPS (fail-safe) rather than
    /// guessing a version. Read as `console_rt` under the armed `app.current_org`.
    pub async fn resolve_active_exec_definition(
        &self,
        org: OrgId,
        definition_id: Uuid,
    ) -> Result<Option<(i32, serde_json::Value)>, KernelError> {
        with_org_conn::<_, Option<(i32, serde_json::Value)>, PgWorkflowRuntimeError>(
            &self.pool,
            org,
            move |tx| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT d.active_version, v.definition \
                         FROM workflow_definitions d \
                         JOIN workflow_definition_versions v \
                           ON v.definition_id = d.id AND v.org_id = d.org_id \
                          AND v.version = d.active_version \
                         WHERE d.id = $1 \
                           AND d.status = 'ACTIVE' \
                           AND d.active_version IS NOT NULL \
                           AND v.definition->>'schema_version' = 'wf.exec.v1'",
                    )
                    .bind(definition_id)
                    .fetch_optional(tx.as_mut())
                    .await
                    .map_err(PgWorkflowRuntimeError::from)?;
                    let Some(row) = row else {
                        return Ok(None);
                    };
                    Ok(Some((
                        row.try_get("active_version")?,
                        row.try_get("definition")?,
                    )))
                })
            },
        )
        .await
        .map_err(KernelError::from)
    }

    /// Due schedules for the poller: `enabled AND next_run_at <= now`, oldest
    /// due first, bounded by `limit` (one tick's work). Read as `console_rt` under
    /// the armed `app.current_org` (matches `idx_workflow_schedules_due`).
    pub async fn list_due_schedules(
        &self,
        org: OrgId,
        now: time::OffsetDateTime,
        limit: i64,
    ) -> Result<Vec<DueScheduleRow>, KernelError> {
        with_org_conn::<_, Vec<DueScheduleRow>, PgWorkflowRuntimeError>(
            &self.pool,
            org,
            move |tx| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "SELECT id, label, cron_expr, timezone, definition_id, next_run_at \
                         FROM workflow_schedules \
                         WHERE enabled AND next_run_at IS NOT NULL AND next_run_at <= $1 \
                         ORDER BY next_run_at ASC \
                         LIMIT $2",
                    )
                    .bind(now)
                    .bind(limit)
                    .fetch_all(tx.as_mut())
                    .await
                    .map_err(PgWorkflowRuntimeError::from)?;
                    rows.into_iter()
                        .map(|row| {
                            Ok(DueScheduleRow {
                                id: row.try_get("id")?,
                                label: row.try_get("label")?,
                                cron_expr: row.try_get("cron_expr")?,
                                timezone: row.try_get("timezone")?,
                                definition_id: row.try_get("definition_id")?,
                                next_run_at: row.try_get("next_run_at")?,
                            })
                        })
                        .collect()
                })
            },
        )
        .await
        .map_err(KernelError::from)
    }

    /// Advance a schedule past the fire it just handled: stamp
    /// `last_run_at`/`last_status` and move `next_run_at` forward — GUARDED on
    /// `next_run_at` still being the claimed fire, so a concurrent poller that
    /// handled the same fire (winner or loser of the run-start idempotency
    /// race; both then advance) applies exactly one advance and the slot is
    /// never skipped or double-advanced. The audited system mutation (actor
    /// `None`) commits in its own `with_audits` txn; a lost guard race writes
    /// NO audit row (nothing changed). Returns whether this call advanced.
    // console-gate: state-changing-handler
    pub async fn advance_schedule(
        &self,
        org: OrgId,
        schedule_id: Uuid,
        fired_at: time::OffsetDateTime,
        next_run_at: Option<time::OffsetDateTime>,
        last_status: &'static str,
    ) -> Result<bool, KernelError> {
        with_audits::<_, bool, PgWorkflowRuntimeError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let result = sqlx::query(
                    "UPDATE workflow_schedules \
                     SET next_run_at = $3, last_run_at = $2, last_status = $4, \
                         updated_at = now() \
                     WHERE id = $1 AND next_run_at = $2",
                )
                .bind(schedule_id)
                .bind(fired_at)
                .bind(next_run_at)
                .bind(last_status)
                .execute(tx.as_mut())
                .await
                .map_err(PgWorkflowRuntimeError::from)?;
                if result.rows_affected() == 0 {
                    // A concurrent poller already advanced this fire — no
                    // mutation, no audit row.
                    return Ok((false, Vec::new()));
                }
                let audit = AuditEvent::new(
                    None,
                    AuditAction::new("workflow_schedule.advance")
                        .map_err(PgWorkflowRuntimeError::Domain)?,
                    "workflow_schedule",
                    schedule_id.to_string(),
                    TraceContext::generate(),
                    time::OffsetDateTime::now_utc(),
                )
                .with_org(org)
                .with_snapshots(
                    Some(serde_json::json!({ "next_run_at": fired_at.to_string() })),
                    Some(serde_json::json!({
                        "next_run_at": next_run_at.map(|at| at.to_string()),
                        "last_run_at": fired_at.to_string(),
                        "last_status": last_status,
                    })),
                );
                Ok((true, vec![audit]))
            })
        })
        .await
        .map_err(KernelError::from)
    }

    /// STEP 5 — drain up to `limit` PENDING/FAILED JOB payroll outbox events for
    /// `org` (design §F). For each claimed event this idempotently stages the
    /// `payroll_draft_runs` row THROUGH THE OWNER — keyed on the deterministic
    /// per-run natural key `workflow_runtime_m2:run:{run_id}` (a same-provenance
    /// conflict replays `Ok(false)`; a changed-provenance conflict is refused as
    /// `StageDraftError::ProvenanceMismatch`), landing `BLOCKED_LEGAL_GATE` with
    /// `calculation_enabled = FALSE` (the column default) so nothing calculates
    /// without the legal gate — then marks the event `DELIVERED` (0078 requires
    /// `delivered_at`) and lands one `workflow_runtime.outbox_drain` audit row.
    /// Returns the number of payroll drafts actually created.
    ///
    /// # This crate no longer writes `payroll_draft_runs`
    ///
    /// It is `ObjectKey::PayRun`'s table and
    /// `console-payroll-adapter-postgres` is the contract's named owner, so the
    /// `INSERT` that used to be here now lives there, in
    /// `pay_run::stage_draft_run_in_tx`, and this drain reaches it through
    /// [`PayrollDraftStaging`] — a port declared in `console-workflow-domain`
    /// because the layer-boundary gate forbids one adapter crate depending on
    /// another. `console-app` wires the implementer in, exactly as it wires the
    /// [`NotificationSink`] the compensation bridge below uses.
    ///
    /// # Three phases, and why the ORDER is the safety argument
    ///
    /// The staging write belongs to another transaction now, so the drain is
    /// staged-then-acked rather than all-in-one:
    ///
    /// 1. claim the due events and apply the freeze-window gate, in ONE read
    ///    transaction that is CLOSED before any owner call — a cross-crate write
    ///    performed while this connection still held a transaction open would
    ///    make the drain a two-connection operation and is how a bounded pool
    ///    deadlocks;
    /// 2. stage each draft through the owner. Idempotent on the natural key, and
    ///    the owner re-runs the freeze-window gate ATOMICALLY inside its own
    ///    staging INSERT (`WHERE NOT EXISTS` over `period_locks`) — a period
    ///    lock acquired after step 1's read but before this write is refused in
    ///    the same statement (fail-closed, event stays PENDING);
    /// 3. ack + audit the events that staged, re-claiming with
    ///    `FOR UPDATE SKIP LOCKED` and acking only rows still `PENDING`/`FAILED`,
    ///    so a concurrent drainer that raced through phase 1 on the same event
    ///    lands neither a second ack nor a second audit row.
    ///
    /// A crash between 2 and 3 leaves the event `PENDING`: the next tick
    /// re-claims it, the restage collides on the natural key and returns
    /// `false`, and the ack lands. No draft is doubled and none is lost. The
    /// reverse order would be the unsafe one — acking first can lose a draft
    /// forever — which is why staging comes first. What is genuinely given up is
    /// "the draft and its ack roll back together"; the natural key was always
    /// the exactly-once mechanism, and this is the identical trade
    /// [`Self::drain_notification_outbox`] documents below.
    // console-gate: state-changing-handler
    pub async fn drain_payroll_job_outbox(
        &self,
        org: OrgId,
        limit: i64,
        staging: &dyn PayrollDraftStaging,
    ) -> Result<u64, KernelError> {
        // --- Phase 1: claim + freeze-window gate, in one read transaction ---
        let drafts: Vec<StagePayrollDraft> =
            with_org_conn::<_, _, PgWorkflowRuntimeError>(&self.pool, org, move |tx| {
                Box::pin(async move {
                    // The lease is the row lock itself (FOR UPDATE SKIP LOCKED);
                    // a competing drainer skips a held row.
                    let claimed = sqlx::query(
                        "SELECT id, run_id, payload \
                         FROM workflow_outbox_events \
                         WHERE channel = 'JOB' \
                           AND payload->>'job' = 'payroll_draft' \
                           AND status IN ('PENDING', 'FAILED') \
                           AND coalesce(next_attempt_at, created_at) <= now() \
                         ORDER BY created_at \
                         FOR UPDATE SKIP LOCKED \
                         LIMIT $1",
                    )
                    .bind(limit)
                    .fetch_all(tx.as_mut())
                    .await
                    .map_err(PgWorkflowRuntimeError::from)?;

                    let mut drafts = Vec::with_capacity(claimed.len());
                    for row in &claimed {
                        let event_id: Uuid = row.try_get("id")?;
                        let run_id: Uuid = row.try_get("run_id")?;
                        let payload: serde_json::Value = row.try_get("payload")?;

                        // The period is re-read with the SAME `::date` casts the
                        // staging statement used to carry, so a payload holding a
                        // malformed date still fails here rather than reaching the
                        // owner, and an absent one is still `None`.
                        let period: Option<(time::Date, time::Date)> = sqlx::query(
                            "SELECT (payload->>'period_start')::date AS period_start, \
                                    (payload->>'period_end')::date AS period_end \
                             FROM workflow_outbox_events WHERE id = $1",
                        )
                        .bind(event_id)
                        .fetch_optional(tx.as_mut())
                        .await
                        .map_err(PgWorkflowRuntimeError::from)?
                        .and_then(|r| {
                            let start: Option<time::Date> = r.try_get("period_start").ok()?;
                            let end: Option<time::Date> = r.try_get("period_end").ok()?;
                            Some((start?, end?))
                        });

                        // Freeze-window gate: a payroll draft whose period
                        // overlaps an active payroll period lock must NOT be
                        // created. The event is left un-acked (PENDING) so it
                        // retries after the period is unlocked — fail closed,
                        // never fail forgotten.
                        if let Some((period_start, period_end)) = period
                            && console_platform_db::assert_period_open_range(
                                tx,
                                console_platform_db::PeriodLockDomain::Payroll,
                                period_start,
                                period_end,
                            )
                            .await
                            .is_err()
                        {
                            tracing::warn!(
                                run_id = %run_id,
                                "payroll draft skipped: period is locked; event stays pending"
                            );
                            continue;
                        }

                        drafts.push(StagePayrollDraft {
                            org,
                            outbox_event_id: event_id,
                            run_id,
                            period_start: period.map(|(start, _)| start),
                            period_end: period.map(|(_, end)| end),
                            connector: payload
                                .get("connector")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned),
                            job: payload
                                .get("job")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned),
                        });
                    }
                    Ok(drafts)
                })
            })
            .await
            .map_err(KernelError::from)?;

        // --- Phase 2: stage each draft through the PayRun owner ---
        let mut created: u64 = 0;
        let mut staged: Vec<(Uuid, Uuid, u64)> = Vec::with_capacity(drafts.len());
        for draft in drafts {
            let event_id = draft.outbox_event_id;
            let run_id = draft.run_id;
            let source_label = draft.source_label();
            match staging.stage(draft).await {
                Ok(fresh) => {
                    let drafts_created = u64::from(fresh);
                    created += drafts_created;
                    staged.push((event_id, run_id, drafts_created));
                }
                Err(err) => {
                    // Leave the event PENDING. The next tick re-claims it and the
                    // restage is a no-op if a partial write did land.
                    tracing::warn!(
                        %event_id,
                        %source_label,
                        error = %err,
                        "payroll draft staging failed; event stays pending"
                    );
                }
            }
        }

        if staged.is_empty() {
            return Ok(created);
        }

        // --- Phase 3: ack + audit, one transaction ---
        with_audits::<_, (), PgWorkflowRuntimeError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let mut audit_events: Vec<AuditEvent> = Vec::with_capacity(staged.len());
                for (event_id, run_id, drafts_created) in staged {
                    // Re-claim: a concurrent drainer that raced through phase 1
                    // on this event may already have acked it, and acking twice
                    // would double the audit row.
                    let acked = sqlx::query(
                        "UPDATE workflow_outbox_events \
                         SET status = 'DELIVERED', delivered_at = now(), \
                             attempt_count = attempt_count + 1, updated_at = now() \
                         WHERE id = $1 AND status IN ('PENDING', 'FAILED')",
                    )
                    .bind(event_id)
                    .execute(tx.as_mut())
                    .await
                    .map_err(PgWorkflowRuntimeError::from)?
                    .rows_affected();
                    if acked == 0 {
                        continue;
                    }

                    let action = AuditAction::new(DRAIN_AUDIT_ACTION)
                        .map_err(PgWorkflowRuntimeError::from)?;
                    let event = AuditEvent::new(
                        None,
                        action,
                        "workflow_outbox_event",
                        event_id.to_string(),
                        TraceContext::generate(),
                        time::OffsetDateTime::now_utc(),
                    )
                    .with_org(org)
                    .with_snapshots(
                        Some(serde_json::json!({ "status": "PENDING" })),
                        Some(serde_json::json!({
                            "status": "DELIVERED",
                            "payroll_drafts_created": drafts_created,
                            "source_label": format!("workflow_runtime_m2:run:{run_id}"),
                        })),
                    );
                    audit_events.push(event);
                }
                Ok(((), audit_events))
            })
        })
        .await
        .map_err(KernelError::from)?;

        Ok(created)
    }

    /// Compensation bridge — drain PENDING/FAILED `NOTIFICATION` outbox events
    /// (the approval-line notify the post-finalization-rejection flow enqueues)
    /// into real notification-center rows via the [`NotificationSink`] write
    /// port. Two-phase and at-least-once: read the due events, emit one
    /// notification per approval-line recipient (idempotent on the outbox event
    /// id + recipient, so a re-drain never doubles a notification), then mark the
    /// event `DELIVERED` with an audit row. A failed emit leaves the event
    /// PENDING for the next tick. Returns the number of notifications emitted.
    pub async fn drain_notification_outbox(
        &self,
        org: OrgId,
        limit: i64,
        sink: &dyn NotificationSink,
    ) -> Result<u64, KernelError> {
        // Phase 1: read due NOTIFICATION events (the lock is NOT held across the
        // emits below; emit + mark are each idempotent, so a re-claim is safe).
        let events: Vec<(Uuid, Uuid, serde_json::Value)> =
            with_org_conn::<_, _, PgWorkflowRuntimeError>(&self.pool, org, move |tx| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "SELECT id, run_id, payload FROM workflow_outbox_events \
                         WHERE channel = 'NOTIFICATION' \
                           AND status IN ('PENDING', 'FAILED') \
                           AND coalesce(next_attempt_at, created_at) <= now() \
                         ORDER BY created_at \
                         LIMIT $1",
                    )
                    .bind(limit)
                    .fetch_all(tx.as_mut())
                    .await?;
                    rows.iter()
                        .map(|row| {
                            Ok((
                                row.try_get("id")?,
                                row.try_get("run_id")?,
                                row.try_get("payload")?,
                            ))
                        })
                        .collect::<Result<Vec<_>, PgWorkflowRuntimeError>>()
                })
            })
            .await
            .map_err(KernelError::from)?;

        let mut emitted: u64 = 0;
        for (event_id, run_id, payload) in events {
            let recipients = payload
                .get("recipients")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            let reason = payload
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();

            // L-1: cap fan-out. An over-cap payload can never fully deliver in a
            // bounded tick, so dead-letter it rather than stall.
            if recipients.len() > MAX_NOTIFICATION_RECIPIENTS {
                tracing::warn!(
                    %event_id,
                    recipient_count = recipients.len(),
                    cap = MAX_NOTIFICATION_RECIPIENTS,
                    "notification bridge: recipient count over cap; dead-lettering"
                );
                self.mark_notification_outbox_failed(
                    org,
                    event_id,
                    true,
                    format!(
                        "recipient count {} exceeds cap {MAX_NOTIFICATION_RECIPIENTS}",
                        recipients.len()
                    ),
                )
                .await?;
                continue;
            }

            let mut all_ok = true;
            let mut skipped: i64 = 0;
            for value in &recipients {
                let Some(recipient) = value.as_str().and_then(|s| Uuid::parse_str(s).ok()) else {
                    // L-2: a malformed recipient entry is a data defect, not a
                    // delivery — count it so the loss shows up in the audit.
                    skipped += 1;
                    tracing::warn!(
                        %event_id,
                        dropped = %value,
                        "notification bridge: skipping non-UUID recipient entry"
                    );
                    continue;
                };
                let text = if reason.is_empty() {
                    "결재가 반려되었습니다".to_owned()
                } else {
                    format!("결재가 반려되었습니다: {reason}")
                };
                let command = EmitNotificationCommand {
                    actor: None,
                    recipient: UserId::from_uuid(recipient),
                    category: "결재".to_owned(),
                    // ponytail: unrelated-lane unblock (BE-ingest-checklist-gates
                    // needed a green `cargo check -p console-app`) — `kind` landed on
                    // `EmitNotificationCommand` without updating this call site;
                    // "info" mirrors the notices crate's generic default. The
                    // notifications-kind lane should replace with a precise kind.
                    kind: "info".to_owned(),
                    text,
                    link: NotificationLink::Object {
                        kind: "workflow_run".to_owned(),
                        id: run_id.to_string(),
                    },
                    dedup_key: Some(format!("wf-outbox:{event_id}:{recipient}")),
                    trace: TraceContext::generate(),
                    occurred_at: time::OffsetDateTime::now_utc(),
                };
                match sink.emit(command).await {
                    Ok(_) => emitted += 1,
                    Err(err) => {
                        tracing::warn!(
                            %event_id,
                            error = %err,
                            "notification bridge: emit failed; backing off (dead-letter at ceiling)"
                        );
                        // M-1: back off + eventually dead-letter so a poison pill
                        // stops being re-claimed every tick. Already-emitted
                        // recipients dedup on retry, so partial progress is safe.
                        self.mark_notification_outbox_failed(
                            org,
                            event_id,
                            false,
                            format!("emit failed: {err}"),
                        )
                        .await?;
                        all_ok = false;
                        break;
                    }
                }
            }

            if all_ok {
                let parseable = i64::try_from(recipients.len()).unwrap_or(i64::MAX) - skipped;
                self.mark_notification_outbox_delivered(org, event_id, parseable, skipped)
                    .await?;
            }
        }
        Ok(emitted)
    }

    /// Back off a failed NOTIFICATION event, or dead-letter it once it has burned
    /// through its attempts (or `force_dead_letter` for an unrecoverable payload).
    /// `error` is stored in `error_payload` (0078 requires a non-empty object for
    /// DEAD_LETTERED) and audited.
    async fn mark_notification_outbox_failed(
        &self,
        org: OrgId,
        event_id: Uuid,
        force_dead_letter: bool,
        error: String,
    ) -> Result<(), KernelError> {
        with_audits::<_, (), PgWorkflowRuntimeError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let error_payload = serde_json::json!({ "error": error });
                // Backoff = least(attempt_count+1, 8) * 30s; dead-letter (terminal,
                // excluded from the claim predicate) at the attempt ceiling.
                let row = sqlx::query(
                    "UPDATE workflow_outbox_events \
                     SET attempt_count = attempt_count + 1, \
                         updated_at = now(), \
                         error_payload = $2, \
                         status = CASE WHEN $3 OR attempt_count + 1 >= $4 \
                                       THEN 'DEAD_LETTERED' ELSE 'FAILED' END, \
                         dead_lettered_at = CASE WHEN $3 OR attempt_count + 1 >= $4 \
                                                 THEN now() ELSE dead_lettered_at END, \
                         next_attempt_at = CASE WHEN $3 OR attempt_count + 1 >= $4 \
                                                THEN next_attempt_at \
                                                ELSE now() + (least(attempt_count + 1, 8) \
                                                              * interval '30 seconds') END \
                     WHERE id = $1 AND channel = 'NOTIFICATION' \
                     RETURNING status, attempt_count",
                )
                .bind(event_id)
                .bind(&error_payload)
                .bind(force_dead_letter)
                .bind(NOTIFICATION_DEAD_LETTER_ATTEMPTS)
                .fetch_one(tx.as_mut())
                .await
                .map_err(PgWorkflowRuntimeError::from)?;
                let status: String = row.try_get("status")?;
                let attempt_count: i32 = row.try_get("attempt_count")?;

                let audit = AuditEvent::new(
                    None,
                    AuditAction::new(DRAIN_AUDIT_ACTION).map_err(PgWorkflowRuntimeError::from)?,
                    "workflow_outbox_event",
                    event_id.to_string(),
                    TraceContext::generate(),
                    time::OffsetDateTime::now_utc(),
                )
                .with_org(org)
                .with_snapshots(
                    Some(serde_json::json!({ "channel": "NOTIFICATION" })),
                    Some(serde_json::json!({
                        "status": status,
                        "attempt_count": attempt_count,
                    })),
                );
                Ok(((), vec![audit]))
            })
        })
        .await
        .map_err(KernelError::from)
    }

    async fn mark_notification_outbox_delivered(
        &self,
        org: OrgId,
        event_id: Uuid,
        recipient_count: i64,
        skipped_count: i64,
    ) -> Result<(), KernelError> {
        with_audits::<_, (), PgWorkflowRuntimeError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                sqlx::query(
                    "UPDATE workflow_outbox_events \
                     SET status = 'DELIVERED', delivered_at = now(), \
                         attempt_count = attempt_count + 1, updated_at = now() \
                     WHERE id = $1 AND channel = 'NOTIFICATION'",
                )
                .bind(event_id)
                .execute(tx.as_mut())
                .await
                .map_err(PgWorkflowRuntimeError::from)?;

                let audit = AuditEvent::new(
                    None,
                    AuditAction::new(DRAIN_AUDIT_ACTION).map_err(PgWorkflowRuntimeError::from)?,
                    "workflow_outbox_event",
                    event_id.to_string(),
                    TraceContext::generate(),
                    time::OffsetDateTime::now_utc(),
                )
                .with_org(org)
                .with_snapshots(
                    Some(serde_json::json!({ "status": "PENDING" })),
                    Some(serde_json::json!({
                        "status": "DELIVERED",
                        "channel": "NOTIFICATION",
                        "notifications_recipient_count": recipient_count,
                        "skipped_recipient_count": skipped_count,
                    })),
                );
                Ok(((), vec![audit]))
            })
        })
        .await
        .map_err(KernelError::from)
    }

    /// List waiting-task inbox rows for the group (`role_key`) or personal
    /// (`assignee=me`) inbox. A plain tenant-scoped read: the REST layer applies the
    /// per-row policy guard and OMITS forbidden rows (deny-by-omission), so this
    /// returns every candidate matching the query filter.
    pub async fn list_waiting_tasks(
        &self,
        org: OrgId,
        me: console_kernel_core::UserId,
        filter: WaitingTaskListFilter,
    ) -> Result<Vec<WaitingTaskListItem>, KernelError> {
        let statuses: Vec<String> = filter
            .statuses
            .iter()
            .map(|status| status.as_db_str().to_owned())
            .collect();
        with_org_conn::<_, Vec<WaitingTaskListItem>, PgWorkflowRuntimeError>(
            &self.pool,
            org,
            move |tx| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "SELECT t.id AS task_id, t.run_id, t.waiting_key, t.title, \
                                t.assignee_role_key, t.required_policy, t.status, \
                                t.claimed_by, t.due_at, t.created_at, t.form_payload, \
                                r.object_type, r.object_id \
                         FROM workflow_waiting_tasks t \
                         JOIN workflow_runs r ON r.id = t.run_id AND r.org_id = t.org_id \
                         WHERE t.status = ANY($1) \
                           AND ($2::text IS NULL OR t.assignee_role_key = $2) \
                           AND (NOT $3 OR t.claimed_by = $4 \
                                OR (t.status = 'OPEN' \
                                    AND (t.assignee_role_key = ANY($5) \
                                         OR (t.assignee_role_key = 'initiator' \
                                             AND r.initiated_by = $4)))) \
                         ORDER BY t.created_at DESC, t.id DESC \
                         LIMIT 200",
                    )
                    .bind(&statuses)
                    .bind(filter.role_key.as_deref())
                    .bind(filter.assignee_me)
                    .bind(*me.as_uuid())
                    .bind(&filter.authority_role_keys)
                    .fetch_all(tx.as_mut())
                    .await?;
                    rows.iter()
                        .map(|row| {
                            let status: String = row.try_get("status")?;
                            Ok(WaitingTaskListItem {
                                task_id: row.try_get("task_id")?,
                                run_id: row.try_get("run_id")?,
                                waiting_key: row.try_get("waiting_key")?,
                                title: row.try_get("title")?,
                                assignee_role_key: row.try_get("assignee_role_key")?,
                                required_policy: row.try_get("required_policy")?,
                                object_type: row.try_get("object_type")?,
                                object_id: row.try_get("object_id")?,
                                status: WaitingTaskStatus::from_db_str(&status)?,
                                claimed_by: row.try_get("claimed_by")?,
                                due_at: row.try_get("due_at")?,
                                created_at: row.try_get("created_at")?,
                                form_payload: row.try_get("form_payload")?,
                            })
                        })
                        .collect()
                })
            },
        )
        .await
        .map_err(KernelError::from)
    }

    /// Bounded global-order candidate page for the unified action inbox. This
    /// retains the canonical personal-inbox SQL predicate; the composition root
    /// still applies its policy guard to every returned row.
    pub async fn list_waiting_tasks_action_page(
        &self,
        org: OrgId,
        me: console_kernel_core::UserId,
        filter: WaitingTaskListFilter,
        as_of: time::OffsetDateTime,
        after: Option<(time::OffsetDateTime, String)>,
        limit: i64,
    ) -> Result<(Vec<WaitingTaskListItem>, bool), KernelError> {
        let limit = limit.clamp(1, 200);
        let statuses = filter
            .statuses
            .iter()
            .map(|status| status.as_db_str().to_owned())
            .collect::<Vec<_>>();
        let (after_created_at, after_id) = after.map_or((None, None), |(created_at, id)| {
            (Some(created_at), Some(id))
        });
        with_org_conn::<_, (Vec<WaitingTaskListItem>, bool), PgWorkflowRuntimeError>(
            &self.pool,
            org,
            move |tx| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "SELECT t.id AS task_id, t.run_id, t.waiting_key, t.title, \
                            t.assignee_role_key, t.required_policy, t.status, \
                            t.claimed_by, t.due_at, t.created_at, t.form_payload, \
                            r.object_type, r.object_id \
                     FROM workflow_waiting_tasks t \
                     JOIN workflow_runs r ON r.id = t.run_id AND r.org_id = t.org_id \
                     WHERE t.status = ANY($1) \
                       AND ($2::text IS NULL OR t.assignee_role_key = $2) \
                       AND (NOT $3 OR t.claimed_by = $4 \
                            OR (t.status = 'OPEN' \
                                AND (t.assignee_role_key = ANY($5) \
                                     OR (t.assignee_role_key = 'initiator' \
                                         AND r.initiated_by = $4)))) \
                       AND t.created_at <= $6 \
                       AND ($8::text IS NULL OR t.created_at > $7 \
                            OR (t.created_at = $7 AND ('approval:' || t.id::text) > $8)) \
                     ORDER BY t.created_at ASC, ('approval:' || t.id::text) ASC \
                     LIMIT $9",
                    )
                    .bind(&statuses)
                    .bind(filter.role_key.as_deref())
                    .bind(filter.assignee_me)
                    .bind(*me.as_uuid())
                    .bind(&filter.authority_role_keys)
                    .bind(as_of)
                    .bind(after_created_at)
                    .bind(after_id.as_deref())
                    .bind(limit + 1)
                    .fetch_all(tx.as_mut())
                    .await?;
                    let has_more = i64::try_from(rows.len()).unwrap_or(0) > limit;
                    let mut items = rows
                        .iter()
                        .take(usize::try_from(limit).unwrap_or(rows.len()))
                        .map(|row| {
                            let status: String = row.try_get("status")?;
                            Ok(WaitingTaskListItem {
                                task_id: row.try_get("task_id")?,
                                run_id: row.try_get("run_id")?,
                                waiting_key: row.try_get("waiting_key")?,
                                title: row.try_get("title")?,
                                assignee_role_key: row.try_get("assignee_role_key")?,
                                required_policy: row.try_get("required_policy")?,
                                object_type: row.try_get("object_type")?,
                                object_id: row.try_get("object_id")?,
                                status: WaitingTaskStatus::from_db_str(&status)?,
                                claimed_by: row.try_get("claimed_by")?,
                                due_at: row.try_get("due_at")?,
                                created_at: row.try_get("created_at")?,
                                form_payload: row.try_get("form_payload")?,
                            })
                        })
                        .collect::<Result<Vec<_>, PgWorkflowRuntimeError>>()?;
                    items.shrink_to_fit();
                    Ok((items, has_more))
                })
            },
        )
        .await
        .map_err(KernelError::from)
    }

    /// List the submission-box runs a principal initiated (`initiated_by = me`).
    /// Final-approved-but-not-finalized runs are still WAITING (non-terminal) and so
    /// are naturally included — no terminal filter is applied.
    pub async fn list_runs_for_initiator(
        &self,
        org: OrgId,
        me: console_kernel_core::UserId,
        filter: RunListFilter,
    ) -> Result<Vec<RunListItem>, KernelError> {
        let statuses: Vec<String> = filter
            .statuses
            .iter()
            .map(|status| status.as_db_str().to_owned())
            .collect();
        // Free-text `?q=` filter (Engine-Gen follow-up): case-insensitive substring
        // over the row's human-readable content — the `object_type` slug and the
        // caller-supplied `input_payload` (reason/subject/etc., a JSONB object with
        // no dedicated title column). Added as an extra AND inside the existing
        // org-scoped + initiator-scoped query, so it only narrows the caller's own
        // rows and never widens visibility. No text index yet: per-user + LIMIT 200
        // keeps this a scan over a handful of rows; a pg_trgm index would only pay
        // off at large per-user submission volume (deferred).
        let q_pattern = filter.q.as_deref().map(ilike_contains_pattern);
        with_org_conn::<_, Vec<RunListItem>, PgWorkflowRuntimeError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    "SELECT r.id AS run_id, r.status, r.definition_id, r.definition_version, \
                            r.object_type, r.object_id, r.initiated_by, r.started_at, r.updated_at \
                     FROM workflow_runs r \
                     WHERE r.initiated_by = $1 \
                       AND (cardinality($2::text[]) = 0 OR r.status = ANY($2)) \
                       AND ($3::text IS NULL OR r.object_type = $3) \
                       AND ($4::text IS NULL \
                            OR r.object_type ILIKE $4 \
                            OR r.input_payload::text ILIKE $4) \
                     ORDER BY r.updated_at DESC \
                     LIMIT 200",
                )
                .bind(*me.as_uuid())
                .bind(&statuses)
                .bind(filter.object_type.as_deref())
                .bind(q_pattern.as_deref())
                .fetch_all(tx.as_mut())
                .await
                .map_err(PgWorkflowRuntimeError::from)?;

                let mut items = Vec::with_capacity(rows.len());
                for row in &rows {
                    let status: String = row.try_get("status")?;
                    items.push(RunListItem {
                        run_id: row.try_get("run_id")?,
                        status: RunStatus::from_db_str(&status)?,
                        definition_id: row.try_get("definition_id")?,
                        definition_version: row.try_get("definition_version")?,
                        object_type: row.try_get("object_type")?,
                        object_id: row.try_get("object_id")?,
                        initiated_by: row.try_get("initiated_by")?,
                        started_at: row.try_get("started_at")?,
                        updated_at: row.try_get("updated_at")?,
                    });
                }
                Ok(items)
            })
        })
        .await
        .map_err(KernelError::from)
    }

    /// List runs org-wide for an admin (workflow-manage), newest first, filterable
    /// by status (incl. `FAILED`/`DEAD_LETTERED` for dead-letter visibility) and
    /// keyset-paginated on `(updated_at, id)`. Org isolation is enforced by RLS on
    /// the armed connection; the caller is responsible for the manage-authz gate.
    pub async fn list_runs_admin(
        &self,
        org: OrgId,
        filter: AdminRunListFilter,
    ) -> Result<Vec<RunListItem>, KernelError> {
        let statuses: Vec<String> = filter
            .statuses
            .iter()
            .map(|status| status.as_db_str().to_owned())
            .collect();
        let before = filter.before;
        let limit = filter.limit.clamp(1, 200);
        with_org_conn::<_, Vec<RunListItem>, PgWorkflowRuntimeError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    "SELECT r.id AS run_id, r.status, r.definition_id, r.definition_version, \
                            r.object_type, r.object_id, r.initiated_by, r.started_at, r.updated_at \
                     FROM workflow_runs r \
                     WHERE (cardinality($1::text[]) = 0 OR r.status = ANY($1)) \
                       AND ($2::uuid IS NULL \
                            OR (r.updated_at, r.id) < \
                               (SELECT updated_at, id FROM workflow_runs WHERE id = $2)) \
                     ORDER BY r.updated_at DESC, r.id DESC \
                     LIMIT $3",
                )
                .bind(&statuses)
                .bind(before)
                .bind(limit)
                .fetch_all(tx.as_mut())
                .await
                .map_err(PgWorkflowRuntimeError::from)?;

                let mut items = Vec::with_capacity(rows.len());
                for row in &rows {
                    let status: String = row.try_get("status")?;
                    items.push(RunListItem {
                        run_id: row.try_get("run_id")?,
                        status: RunStatus::from_db_str(&status)?,
                        definition_id: row.try_get("definition_id")?,
                        definition_version: row.try_get("definition_version")?,
                        object_type: row.try_get("object_type")?,
                        object_id: row.try_get("object_id")?,
                        initiated_by: row.try_get("initiated_by")?,
                        started_at: row.try_get("started_at")?,
                        updated_at: row.try_get("updated_at")?,
                    });
                }
                Ok(items)
            })
        })
        .await
        .map_err(KernelError::from)
    }

    /// Claim an OPEN waiting task (OPEN → CLAIMED). A same-user replay on an
    /// already-CLAIMED task is a 200 no-op; a task claimed by another user, or in any
    /// terminal/cancelled/expired state, is a 409. Audits `workflow_task.claim`.
    // console-gate: state-changing-handler
    pub async fn claim_waiting_task(
        &self,
        org: OrgId,
        command: ClaimWaitingTaskCommand,
    ) -> Result<ClaimedWaitingTask, KernelError> {
        with_audits::<_, ClaimedWaitingTask, PgWorkflowRuntimeError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT id AS task_id, run_id, status, claimed_by, claimed_at \
                     FROM workflow_waiting_tasks WHERE id = $1 FOR UPDATE",
                )
                .bind(command.task_id)
                .fetch_optional(tx.as_mut())
                .await
                .map_err(PgWorkflowRuntimeError::from)?;

                let Some(row) = row else {
                    return Err(PgWorkflowRuntimeError::from(KernelError::not_found(
                        "workflow task not found",
                    )));
                };
                let run_id: Uuid = row.try_get("run_id")?;
                let status_str: String = row.try_get("status")?;
                let status = WaitingTaskStatus::from_db_str(&status_str)?;
                let claimed_by: Option<Uuid> = row.try_get("claimed_by")?;
                let claimed_at: Option<time::OffsetDateTime> = row.try_get("claimed_at")?;

                // Same-user replay: already CLAIMED by this actor is an idempotent 200.
                if status == WaitingTaskStatus::Claimed
                    && claimed_by == Some(*command.actor.as_uuid())
                {
                    return Ok((
                        ClaimedWaitingTask {
                            task_id: command.task_id,
                            run_id,
                            status,
                            claimed_by,
                            claimed_at,
                        },
                        Vec::new(),
                    ));
                }
                if status != WaitingTaskStatus::Open {
                    return Err(PgWorkflowRuntimeError::from(KernelError::conflict(
                        "workflow task is not open to claim",
                    )));
                }

                sqlx::query(
                    "UPDATE workflow_waiting_tasks \
                     SET status = 'CLAIMED', claimed_by = $2, claimed_at = now(), updated_at = now() \
                     WHERE id = $1 AND status = 'OPEN'",
                )
                .bind(command.task_id)
                .bind(*command.actor.as_uuid())
                .execute(tx.as_mut())
                .await
                .map_err(PgWorkflowRuntimeError::from)?;

                let new_claimed_at: Option<time::OffsetDateTime> = sqlx::query_scalar(
                    "SELECT claimed_at FROM workflow_waiting_tasks WHERE id = $1",
                )
                .bind(command.task_id)
                .fetch_one(tx.as_mut())
                .await
                .map_err(PgWorkflowRuntimeError::from)?;

                let mut audits = command.transition_audits;
                audits.push(
                    AuditEvent::new(
                        Some(command.actor),
                        AuditAction::new("workflow_task.claim")
                            .map_err(PgWorkflowRuntimeError::from)?,
                        "workflow_waiting_task",
                        command.task_id.to_string(),
                        TraceContext::generate(),
                        time::OffsetDateTime::now_utc(),
                    )
                    .with_org(org)
                    .with_snapshots(
                        Some(serde_json::json!({ "status": "OPEN" })),
                        Some(serde_json::json!({ "status": "CLAIMED" })),
                    ),
                );

                Ok((
                    ClaimedWaitingTask {
                        task_id: command.task_id,
                        run_id,
                        status: WaitingTaskStatus::Claimed,
                        claimed_by: Some(*command.actor.as_uuid()),
                        claimed_at: new_claimed_at,
                    },
                    audits,
                ))
            })
        })
        .await
        .map_err(KernelError::from)
    }

    /// Decide a non-finalize waiting task: `approve` advances the run to the next
    /// node (a human task parks WAITING; no successor closes the run SUCCEEDED);
    /// `reject`/`return` land the task REJECTED and cancel the run (no terminal
    /// reopen — a resubmission is a new run). Idempotent by `idempotency_key`. Audits
    /// `workflow_task.decide` plus the node/run transitions. Finalize/receipt tasks
    /// are 422 here (they go through the finalize endpoint).
    // console-gate: state-changing-handler
    pub async fn decide_waiting_task(
        &self,
        org: OrgId,
        command: DecideWaitingTaskCommand,
    ) -> Result<DecidedWaitingTask, KernelError> {
        with_audits::<_, DecidedWaitingTask, PgWorkflowRuntimeError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT t.id AS task_id, t.run_id, t.node_run_id, t.waiting_key, \
                            t.status AS task_status, t.claimed_by, t.completed_by, \
                            t.decision_payload, r.status AS run_status, r.initiated_by \
                     FROM workflow_waiting_tasks t \
                     JOIN workflow_runs r ON r.id = t.run_id AND r.org_id = t.org_id \
                     WHERE t.id = $1 FOR UPDATE OF t, r",
                )
                .bind(command.task_id)
                .fetch_optional(tx.as_mut())
                .await
                .map_err(PgWorkflowRuntimeError::from)?;

                let Some(row) = row else {
                    return Err(PgWorkflowRuntimeError::from(KernelError::not_found(
                        "workflow task not found",
                    )));
                };
                let run_id: Uuid = row.try_get("run_id")?;
                let node_run_id: Option<Uuid> = row.try_get("node_run_id")?;
                let waiting_key: String = row.try_get("waiting_key")?;
                let task_status_str: String = row.try_get("task_status")?;
                let run_status_str: String = row.try_get("run_status")?;
                let task_status = WaitingTaskStatus::from_db_str(&task_status_str)?;
                let run_status = RunStatus::from_db_str(&run_status_str)?;
                let claimed_by: Option<Uuid> = row.try_get("claimed_by")?;
                let completed_by: Option<Uuid> = row.try_get("completed_by")?;
                let initiated_by: Option<Uuid> = row.try_get("initiated_by")?;
                let existing_decision: Option<serde_json::Value> =
                    row.try_get("decision_payload")?;

                if waiting_key == FINALIZE_WAITING_KEY || waiting_key == RECEIPT_WAITING_KEY {
                    return Err(PgWorkflowRuntimeError::from(KernelError::validation(
                        "workflow task is a finalization/receipt task, not a decision task",
                    )));
                }

                // Idempotent replay: a completed decision with the same key returns
                // its recorded result (no next_task re-derivation needed).
                if matches!(
                    task_status,
                    WaitingTaskStatus::Approved | WaitingTaskStatus::Rejected
                ) && existing_decision
                    .as_ref()
                    .and_then(|value| value.get("idempotency_key"))
                    .and_then(serde_json::Value::as_str)
                    == Some(command.idempotency_key.as_str())
                {
                    let _ = completed_by;
                    return Ok((
                        DecidedWaitingTask {
                            task_id: command.task_id,
                            run_id,
                            status: task_status,
                            decision_payload: existing_decision
                                .unwrap_or_else(|| serde_json::json!({})),
                            run_status,
                            next_task: None,
                        },
                        Vec::new(),
                    ));
                }

                if !matches!(
                    task_status,
                    WaitingTaskStatus::Open | WaitingTaskStatus::Claimed
                ) {
                    return Err(PgWorkflowRuntimeError::from(KernelError::conflict(
                        "workflow task is not open for a decision",
                    )));
                }
                if let Some(claimed_by) = claimed_by
                    && claimed_by != *command.actor.as_uuid()
                {
                    return Err(PgWorkflowRuntimeError::from(KernelError::conflict(
                        "workflow task is claimed by another user",
                    )));
                }
                if run_status != RunStatus::Waiting {
                    return Err(PgWorkflowRuntimeError::from(KernelError::conflict(
                        "workflow run is not waiting for a decision",
                    )));
                }

                // Approval requires a known, different initiator. Rejection/return,
                // finalization/receipt and historical receipt replay keep their own paths.
                if command.decision == TaskDecision::Approve {
                    let maker = initiated_by.ok_or_else(|| {
                        KernelError::forbidden("기안자를 확인할 수 없어 승인할 수 없습니다")
                    })?;
                    if maker == *command.actor.as_uuid() {
                        return Err(KernelError::forbidden(
                            "본인이 기안한 건은 승인할 수 없습니다",
                        )
                        .into());
                    }
                }

                let approved = command.decision == TaskDecision::Approve;
                let new_task_status = if approved {
                    WaitingTaskStatus::Approved
                } else {
                    WaitingTaskStatus::Rejected
                };
                let decision_payload = serde_json::json!({
                    "decision": command.decision.as_str(),
                    "comment": command.comment,
                    "idempotency_key": command.idempotency_key,
                });

                sqlx::query(
                    "UPDATE workflow_waiting_tasks \
                     SET status = $2, completed_by = $3, completed_at = now(), \
                         decision_payload = $4, updated_at = now() \
                     WHERE id = $1 AND status IN ('OPEN', 'CLAIMED')",
                )
                .bind(command.task_id)
                .bind(new_task_status.as_db_str())
                .bind(*command.actor.as_uuid())
                .bind(decision_payload.clone())
                .execute(tx.as_mut())
                .await
                .map_err(PgWorkflowRuntimeError::from)?;

                if let Some(node_run_id) = node_run_id {
                    sqlx::query(
                        "UPDATE workflow_node_runs \
                         SET status = 'SUCCEEDED', finished_at = now(), updated_at = now(), \
                             output_payload = $2 \
                         WHERE id = $1 AND status = 'WAITING'",
                    )
                    .bind(node_run_id)
                    .bind(serde_json::json!({ "decision": command.decision.as_str() }))
                    .execute(tx.as_mut())
                    .await
                    .map_err(PgWorkflowRuntimeError::from)?;
                }

                let mut audit_events = command.transition_audits;
                audit_events.push(
                    AuditEvent::new(
                        Some(command.actor),
                        AuditAction::new("workflow_task.decide")
                            .map_err(PgWorkflowRuntimeError::from)?,
                        "workflow_waiting_task",
                        command.task_id.to_string(),
                        TraceContext::generate(),
                        time::OffsetDateTime::now_utc(),
                    )
                    .with_org(org)
                    .with_snapshots(
                        Some(serde_json::json!({ "status": task_status.as_db_str() })),
                        Some(serde_json::json!({
                            "status": new_task_status.as_db_str(),
                            "decision": command.decision.as_str(),
                        })),
                    ),
                );
                if let Some(node_run_id) = node_run_id {
                    audit_events.push(
                        AuditEvent::new(
                            Some(command.actor),
                            AuditAction::new("workflow_node.commit")
                                .map_err(PgWorkflowRuntimeError::from)?,
                            "workflow_node_run",
                            node_run_id.to_string(),
                            TraceContext::generate(),
                            time::OffsetDateTime::now_utc(),
                        )
                        .with_org(org)
                        .with_snapshots(
                            Some(serde_json::json!({ "status": "WAITING" })),
                            Some(serde_json::json!({ "status": "SUCCEEDED" })),
                        ),
                    );
                }

                // reject/return: cancel the run (terminal; no reopen). approve:
                // advance to the next node.
                if !approved {
                    sqlx::query(run_transition_sql(RunStatus::Cancelled))
                        .bind(run_id)
                        .bind(RunStatus::Cancelled.as_db_str())
                        .bind(RunStatus::Waiting.as_db_str())
                        .bind(serde_json::json!({ "decision": command.decision.as_str() }))
                        .bind(Option::<serde_json::Value>::None)
                        .execute(tx.as_mut())
                        .await
                        .map_err(PgWorkflowRuntimeError::from)?;
                    audit_events.push(run_transition_audit(
                        command.actor,
                        org,
                        run_id,
                        "WAITING",
                        RunStatus::Cancelled.as_db_str(),
                    )?);
                    return Ok((
                        DecidedWaitingTask {
                            task_id: command.task_id,
                            run_id,
                            status: new_task_status,
                            decision_payload,
                            run_status: RunStatus::Cancelled,
                            next_task: None,
                        },
                        audit_events,
                    ));
                }

                // approve: resolve the next node from the published graph.
                let definition: serde_json::Value = sqlx::query_scalar(
                    "SELECT v.definition \
                     FROM workflow_runs r \
                     JOIN workflow_definition_versions v \
                       ON v.definition_id = r.definition_id \
                      AND v.version = r.definition_version \
                      AND v.org_id = r.org_id \
                     WHERE r.id = $1",
                )
                .bind(run_id)
                .fetch_one(tx.as_mut())
                .await
                .map_err(PgWorkflowRuntimeError::from)?;

                let (run_final, next_task) = match next_node_after(&definition, &waiting_key) {
                    None => {
                        // Terminal: no successor closes the run SUCCEEDED.
                        sqlx::query(run_transition_sql(RunStatus::Succeeded))
                            .bind(run_id)
                            .bind(RunStatus::Succeeded.as_db_str())
                            .bind(RunStatus::Waiting.as_db_str())
                            .bind(serde_json::json!({ "decision": "approve" }))
                            .bind(Option::<serde_json::Value>::None)
                            .execute(tx.as_mut())
                            .await
                            .map_err(PgWorkflowRuntimeError::from)?;
                        audit_events.push(run_transition_audit(
                            command.actor,
                            org,
                            run_id,
                            "WAITING",
                            RunStatus::Succeeded.as_db_str(),
                        )?);
                        (RunStatus::Succeeded, None)
                    }
                    Some(next) if is_human_node(&next.node_type) => {
                        // Park the next approval step; the run stays WAITING.
                        let (_next_node_run, next_task_id) =
                            park_waiting_node(tx, org, run_id, &next).await?;
                        audit_events.push(
                            AuditEvent::new(
                                Some(command.actor),
                                AuditAction::new("workflow_node.commit")
                                    .map_err(PgWorkflowRuntimeError::from)?,
                                "workflow_node_run",
                                next.node_key.clone(),
                                TraceContext::generate(),
                                time::OffsetDateTime::now_utc(),
                            )
                            .with_org(org)
                            .with_snapshots(
                                Some(serde_json::json!({ "status": "PENDING" })),
                                Some(serde_json::json!({ "status": "WAITING" })),
                            ),
                        );
                        (
                            RunStatus::Waiting,
                            Some(WaitingTaskListItem {
                                task_id: next_task_id,
                                run_id,
                                waiting_key: next.node_key.clone(),
                                title: next.title.clone(),
                                assignee_role_key: next.assignee_role_key.clone(),
                                required_policy: next.required_policy.clone(),
                                object_type: None,
                                object_id: None,
                                status: WaitingTaskStatus::Open,
                                claimed_by: None,
                                due_at: None,
                                created_at: time::OffsetDateTime::now_utc(),
                                form_payload: serde_json::json!({}),
                            }),
                        )
                    }
                    Some(_) => {
                        // ponytail: approval lines are human chains end-to-end in the
                        // builder's templates; a non-human successor after a decision
                        // node is not emitted today. Fail closed rather than silently
                        // stranding the run — add gate/job advance here if a template
                        // ever needs it.
                        return Err(PgWorkflowRuntimeError::from(KernelError::validation(
                            "decision advance to a non-human node is unsupported",
                        )));
                    }
                };

                Ok((
                    DecidedWaitingTask {
                        task_id: command.task_id,
                        run_id,
                        status: new_task_status,
                        decision_payload,
                        run_status: run_final,
                        next_task,
                    },
                    audit_events,
                ))
            })
        })
        .await
        .map_err(KernelError::from)
    }
}

fn is_human_node(node_type: &str) -> bool {
    matches!(node_type, "human_task" | "waiting_task")
}

fn run_transition_audit(
    actor: console_kernel_core::UserId,
    org: OrgId,
    run_id: Uuid,
    from: &str,
    to: &str,
) -> Result<AuditEvent, PgWorkflowRuntimeError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("workflow_run.transition").map_err(PgWorkflowRuntimeError::from)?,
        "workflow_run",
        run_id.to_string(),
        TraceContext::generate(),
        time::OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(
        Some(serde_json::json!({ "status": from })),
        Some(serde_json::json!({ "status": to })),
    ))
}

/// The UPDATE for a run transition, selected by which terminal timestamp column
/// the target status must stamp (per the 0077 CHECKs). Each arm is a static
/// literal; only bound parameters carry data. Bind order is uniform:
/// `$1` run_id, `$2` new status, `$3` expected current status, `$4` output, `$5` error.
fn run_transition_sql(to: RunStatus) -> &'static str {
    match to.terminal_timestamp() {
        None => {
            "UPDATE workflow_runs \
             SET status = $2, updated_at = now(), \
                 output_payload = COALESCE($4, output_payload), \
                 error_payload = COALESCE($5, error_payload) \
             WHERE id = $1 AND status = $3"
        }
        Some(RunTerminalTimestamp::CompletedAt) => {
            "UPDATE workflow_runs \
             SET status = $2, updated_at = now(), completed_at = now(), \
                 output_payload = COALESCE($4, output_payload), \
                 error_payload = COALESCE($5, error_payload) \
             WHERE id = $1 AND status = $3"
        }
        Some(RunTerminalTimestamp::FailedAt) => {
            "UPDATE workflow_runs \
             SET status = $2, updated_at = now(), failed_at = now(), \
                 output_payload = COALESCE($4, output_payload), \
                 error_payload = COALESCE($5, error_payload) \
             WHERE id = $1 AND status = $3"
        }
    }
}

impl WorkflowRuntimePort for PgWorkflowRuntimeStore {
    // console-gate: state-changing-handler
    fn insert_run<'a>(&'a self, run: NewRun, audit: AuditEvent) -> PortFuture<'a, ()> {
        Box::pin(async move {
            with_audit::<_, (), PgWorkflowRuntimeError>(&self.pool, audit, move |tx| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO workflow_runs \
                             (id, org_id, definition_id, definition_version, status, \
                              trigger_type, object_type, object_id, idempotency_key, \
                              correlation_id, trace_id, input_payload, context_payload, \
                              initiated_by, schedule_id) \
                         VALUES ($1, $2, $3, $4, 'STARTING', $5, $6, $7, $8, $9, $10, \
                                 $11, $12, $13, $14)",
                    )
                    .bind(run.id)
                    .bind(*run.org_id.as_uuid())
                    .bind(run.definition_id)
                    .bind(run.definition_version)
                    .bind(run.trigger_type.as_db_str())
                    .bind(run.object_type)
                    .bind(run.object_id)
                    .bind(run.idempotency_key)
                    .bind(run.correlation_id)
                    .bind(run.trace_id)
                    .bind(run.input_payload)
                    .bind(run.context_payload)
                    .bind(run.initiated_by.map(|user| *user.as_uuid()))
                    .bind(run.schedule_id)
                    .execute(tx.as_mut())
                    .await
                    .map_err(PgWorkflowRuntimeError::from)?;
                    Ok(())
                })
            })
            .await
            .map_err(KernelError::from)
        })
    }

    fn load_run<'a>(&'a self, org: OrgId, run_id: Uuid) -> PortFuture<'a, Option<RunRecord>> {
        Box::pin(async move {
            with_org_conn::<_, Option<RunRecord>, PgWorkflowRuntimeError>(
                &self.pool,
                org,
                move |tx| {
                    Box::pin(async move {
                        let row = sqlx::query(
                            "SELECT id, org_id, status, definition_id, definition_version, \
                                    object_type, object_id \
                             FROM workflow_runs WHERE id = $1",
                        )
                        .bind(run_id)
                        .fetch_optional(tx.as_mut())
                        .await
                        .map_err(PgWorkflowRuntimeError::from)?;

                        let Some(row) = row else {
                            return Ok(None);
                        };
                        let status_str: String = row.try_get("status")?;
                        let id: Uuid = row.try_get("id")?;
                        let org_uuid: Uuid = row.try_get("org_id")?;
                        let definition_id: Uuid = row.try_get("definition_id")?;
                        let definition_version: i32 = row.try_get("definition_version")?;
                        let object_type: Option<String> = row.try_get("object_type")?;
                        let object_id: Option<Uuid> = row.try_get("object_id")?;
                        Ok(Some(RunRecord {
                            id,
                            org_id: OrgId::from_uuid(org_uuid),
                            status: RunStatus::from_db_str(&status_str)?,
                            definition_id,
                            definition_version,
                            object_type,
                            object_id,
                        }))
                    })
                },
            )
            .await
            .map_err(KernelError::from)
        })
    }

    fn load_run_by_idempotency_key<'a>(
        &'a self,
        org: OrgId,
        idempotency_key: String,
    ) -> PortFuture<'a, Option<RunRecord>> {
        Box::pin(async move {
            PgWorkflowRuntimeStore::load_run_by_idempotency_key(self, org, idempotency_key).await
        })
    }

    // console-gate: state-changing-handler
    fn transition_run<'a>(
        &'a self,
        org: OrgId,
        transition: RunTransition,
        audit: AuditEvent,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move {
            with_audit::<_, (), PgWorkflowRuntimeError>(&self.pool, audit, move |tx| {
                Box::pin(async move {
                    let _ = org; // org is armed by with_audit from the event; kept for symmetry.
                    let result = sqlx::query(run_transition_sql(transition.to))
                        .bind(transition.run_id)
                        .bind(transition.to.as_db_str())
                        .bind(transition.from.as_db_str())
                        .bind(transition.output_payload)
                        .bind(transition.error_payload)
                        .execute(tx.as_mut())
                        .await
                        .map_err(PgWorkflowRuntimeError::from)?;
                    if result.rows_affected() == 0 {
                        return Err(PgWorkflowRuntimeError::from(KernelError::conflict(
                            "workflow run transition lost status race",
                        )));
                    }
                    Ok(())
                })
            })
            .await
            .map_err(KernelError::from)
        })
    }

    // console-gate: state-changing-handler
    fn commit_node_step<'a>(&'a self, org: OrgId, commit: NodeStepCommit) -> PortFuture<'a, ()> {
        Box::pin(async move {
            with_audits::<_, (), PgWorkflowRuntimeError>(&self.pool, org, move |tx| {
                Box::pin(async move {
                    let NodeStepCommit {
                        new_node,
                        node_final_status,
                        node_output,
                        node_error,
                        emissions,
                        waiting_task,
                        run_transition,
                        audit_events,
                    } = commit;
                    let org_uuid = *org.as_uuid();

                    // 1. Insert the node run PENDING. ON CONFLICT DO NOTHING on the
                    //    reused UNIQUE(org_id, idempotency_key) (0077:69) so a RESUMED
                    //    completion tail (a reconciler re-drive after a crash) does not
                    //    23505-abort on a node it already recorded — the node key is
                    //    deterministic (node:{run_id}:{node_key}:{attempt}), so a re-run
                    //    of the same node is a no-op and the subsequent status UPDATEs
                    //    (guarded on the fresh node id) simply match zero rows.
                    sqlx::query(
                        "INSERT INTO workflow_node_runs \
                             (id, org_id, run_id, node_key, node_type, status, attempt, \
                              idempotency_key, input_payload) \
                         VALUES ($1, $2, $3, $4, $5, 'PENDING', $6, $7, $8) \
                         ON CONFLICT (org_id, idempotency_key) DO NOTHING",
                    )
                    .bind(new_node.id)
                    .bind(org_uuid)
                    .bind(new_node.run_id)
                    .bind(new_node.node_key)
                    .bind(new_node.node_type)
                    .bind(new_node.attempt)
                    .bind(new_node.idempotency_key)
                    .bind(new_node.input_payload)
                    .execute(tx.as_mut())
                    .await
                    .map_err(PgWorkflowRuntimeError::from)?;

                    // 2. Node PENDING -> RUNNING.
                    sqlx::query(
                        "UPDATE workflow_node_runs \
                         SET status = 'RUNNING', started_at = now(), updated_at = now() \
                         WHERE id = $1 AND status = 'PENDING'",
                    )
                    .bind(new_node.id)
                    .execute(tx.as_mut())
                    .await
                    .map_err(PgWorkflowRuntimeError::from)?;

                    // 3. Transactional-outbox emissions. ON CONFLICT DO NOTHING on the
                    //    reused UNIQUE(org_id, idempotency_key) (0077:149) so re-running
                    //    a node whose emission was already enqueued (a resumed tail)
                    //    never duplicates the outbox row nor 23505-aborts.
                    for emission in emissions {
                        sqlx::query(
                            "INSERT INTO workflow_outbox_events \
                                 (org_id, run_id, node_run_id, channel, destination_ref, \
                                  idempotency_key, status, payload) \
                             VALUES ($1, $2, $3, $4, $5, $6, 'PENDING', $7) \
                             ON CONFLICT (org_id, idempotency_key) DO NOTHING",
                        )
                        .bind(org_uuid)
                        .bind(new_node.run_id)
                        .bind(emission.node_run_id)
                        .bind(emission.channel.as_db_str())
                        .bind(emission.destination_ref)
                        .bind(emission.idempotency_key)
                        .bind(emission.payload)
                        .execute(tx.as_mut())
                        .await
                        .map_err(PgWorkflowRuntimeError::from)?;
                    }

                    // 4. Optional waiting task (run parks on it).
                    if let Some(task) = waiting_task {
                        sqlx::query(
                            "INSERT INTO workflow_waiting_tasks \
                                 (org_id, run_id, node_run_id, waiting_key, title, status, \
                                  assignee_role_key, required_policy, form_payload, due_at) \
                             VALUES ($1, $2, $3, $4, $5, 'OPEN', $6, $7, $8, $9)",
                        )
                        .bind(org_uuid)
                        .bind(task.run_id)
                        .bind(task.node_run_id)
                        .bind(task.waiting_key)
                        .bind(task.title)
                        .bind(task.assignee_role_key)
                        .bind(task.required_policy)
                        .bind(task.form_payload)
                        .bind(task.due_at)
                        .execute(tx.as_mut())
                        .await
                        .map_err(PgWorkflowRuntimeError::from)?;
                    }

                    // 5. Node RUNNING -> final. Terminal statuses stamp finished_at.
                    let node_sql = if node_final_status.sets_finished_at() {
                        "UPDATE workflow_node_runs \
                         SET status = $2, updated_at = now(), finished_at = now(), \
                             output_payload = $3, error_payload = $4 \
                         WHERE id = $1 AND status = 'RUNNING'"
                    } else {
                        "UPDATE workflow_node_runs \
                         SET status = $2, updated_at = now(), \
                             output_payload = $3, error_payload = $4 \
                         WHERE id = $1 AND status = 'RUNNING'"
                    };
                    sqlx::query(node_sql)
                        .bind(new_node.id)
                        .bind(node_final_status.as_db_str())
                        .bind(node_output)
                        .bind(node_error)
                        .execute(tx.as_mut())
                        .await
                        .map_err(PgWorkflowRuntimeError::from)?;

                    // 6. Optional run transition (e.g. RUNNING -> WAITING on a gate).
                    if let Some(transition) = run_transition {
                        sqlx::query(run_transition_sql(transition.to))
                            .bind(transition.run_id)
                            .bind(transition.to.as_db_str())
                            .bind(transition.from.as_db_str())
                            .bind(transition.output_payload)
                            .bind(transition.error_payload)
                            .execute(tx.as_mut())
                            .await
                            .map_err(PgWorkflowRuntimeError::from)?;
                    }

                    Ok(((), audit_events))
                })
            })
            .await
            .map_err(KernelError::from)
        })
    }

    fn load_finalize_waiting_task<'a>(
        &'a self,
        org: OrgId,
        task_id: Uuid,
    ) -> PortFuture<'a, Option<FinalizeWaitingTaskContext>> {
        Box::pin(async move {
            with_org_conn::<_, Option<FinalizeWaitingTaskContext>, PgWorkflowRuntimeError>(
                &self.pool,
                org,
                move |tx| {
                    Box::pin(async move {
                        let row = sqlx::query(
                            "SELECT \
                                t.id AS task_id, t.run_id, t.node_run_id, t.waiting_key, \
                                t.status AS task_status, t.required_policy, \
                                r.status AS run_status, r.object_type, r.object_id, r.initiated_by \
                             FROM workflow_waiting_tasks t \
                             JOIN workflow_runs r ON r.id = t.run_id AND r.org_id = t.org_id \
                             WHERE t.id = $1",
                        )
                        .bind(task_id)
                        .fetch_optional(tx.as_mut())
                        .await
                        .map_err(PgWorkflowRuntimeError::from)?;

                        let Some(row) = row else {
                            return Ok(None);
                        };
                        let task_status: String = row.try_get("task_status")?;
                        let run_status: String = row.try_get("run_status")?;
                        let initiated_by: Option<Uuid> = row.try_get("initiated_by")?;
                        let initiated_by = initiated_by.ok_or_else(|| {
                            KernelError::validation("finalize task run has no initiator")
                        })?;

                        Ok(Some(FinalizeWaitingTaskContext {
                            task_id: row.try_get("task_id")?,
                            run_id: row.try_get("run_id")?,
                            node_run_id: row.try_get("node_run_id")?,
                            waiting_key: row.try_get("waiting_key")?,
                            task_status: WaitingTaskStatus::from_db_str(&task_status)?,
                            run_status: RunStatus::from_db_str(&run_status)?,
                            required_policy: row.try_get("required_policy")?,
                            object_type: row.try_get("object_type")?,
                            object_id: row.try_get("object_id")?,
                            initiated_by: console_kernel_core::UserId::from_uuid(initiated_by),
                        }))
                    })
                },
            )
            .await
            .map_err(KernelError::from)
        })
    }

    // console-gate: state-changing-handler
    fn finalize_waiting_task<'a>(
        &'a self,
        org: OrgId,
        command: FinalizeWaitingTaskCommand,
    ) -> PortFuture<'a, FinalizedWaitingTask> {
        Box::pin(async move {
            with_audits::<_, FinalizedWaitingTask, PgWorkflowRuntimeError>(
                &self.pool,
                org,
                move |tx| {
                    Box::pin(async move {
                        let row = sqlx::query(
                            "SELECT \
                                t.id AS task_id, t.run_id, t.node_run_id, t.waiting_key, \
                                t.status AS task_status, t.claimed_by, t.completed_by, t.decision_payload, \
                                r.status AS run_status \
                             FROM workflow_waiting_tasks t \
                             JOIN workflow_runs r ON r.id = t.run_id AND r.org_id = t.org_id \
                             WHERE t.id = $1 \
                             FOR UPDATE OF t, r",
                        )
                        .bind(command.task_id)
                        .fetch_optional(tx.as_mut())
                        .await
                        .map_err(PgWorkflowRuntimeError::from)?;

                        let Some(row) = row else {
                            return Err(PgWorkflowRuntimeError::from(KernelError::not_found(
                                "workflow task not found",
                            )));
                        };
                        let task_status_str: String = row.try_get("task_status")?;
                        let run_status_str: String = row.try_get("run_status")?;
                        let task_status = WaitingTaskStatus::from_db_str(&task_status_str)?;
                        let run_status = RunStatus::from_db_str(&run_status_str)?;
                        let waiting_key: String = row.try_get("waiting_key")?;
                        let node_run_id: Option<Uuid> = row.try_get("node_run_id")?;
                        let claimed_by: Option<Uuid> = row.try_get("claimed_by")?;
                        let completed_by: Option<Uuid> = row.try_get("completed_by")?;
                        let existing_decision: Option<serde_json::Value> =
                            row.try_get("decision_payload")?;

                        if waiting_key != FINALIZE_WAITING_KEY {
                            return Err(PgWorkflowRuntimeError::from(KernelError::validation(
                                "workflow task is not a finalization task",
                            )));
                        }

                        if task_status == WaitingTaskStatus::Approved
                            && existing_decision
                                .as_ref()
                                .and_then(|value| value.get("idempotency_key"))
                                .and_then(serde_json::Value::as_str)
                                == Some(command.idempotency_key.as_str())
                        {
                            return Ok((
                                FinalizedWaitingTask {
                                    task_id: command.task_id,
                                    run_id: row.try_get("run_id")?,
                                    status: WaitingTaskStatus::Approved,
                                    completed_by: completed_by.map(console_kernel_core::UserId::from_uuid),
                                    decision_payload: existing_decision
                                        .unwrap_or_else(|| serde_json::json!({})),
                                    run_status,
                                },
                                Vec::new(),
                            ));
                        }

                        if !matches!(
                            task_status,
                            WaitingTaskStatus::Open | WaitingTaskStatus::Claimed
                        ) {
                            return Err(PgWorkflowRuntimeError::from(KernelError::conflict(
                                "workflow task is not open for finalization",
                            )));
                        }
                        if let Some(claimed_by) = claimed_by
                            && claimed_by != *command.actor.as_uuid()
                        {
                            return Err(PgWorkflowRuntimeError::from(KernelError::conflict(
                                "workflow task is claimed by another user",
                            )));
                        }
                        if run_status != RunStatus::Waiting {
                            return Err(PgWorkflowRuntimeError::from(KernelError::conflict(
                                "workflow run is not waiting for finalization",
                            )));
                        }

                        let run_id: Uuid = row.try_get("run_id")?;
                        let receipt_spec = {
                            let definition: serde_json::Value = sqlx::query_scalar(
                                "SELECT v.definition \
                                 FROM workflow_runs r \
                                 JOIN workflow_definition_versions v \
                                   ON v.definition_id = r.definition_id \
                                  AND v.version = r.definition_version \
                                  AND v.org_id = r.org_id \
                                 WHERE r.id = $1",
                            )
                            .bind(run_id)
                            .fetch_one(tx.as_mut())
                            .await
                            .map_err(PgWorkflowRuntimeError::from)?;
                            receipt_node_after_finalize(&definition)
                        };
                        let decision_payload = serde_json::json!({
                            "mode": command.mode,
                            "delegated_reason": command.delegated_reason,
                            "idempotency_key": command.idempotency_key,
                            "awaiting_receipt": receipt_spec.is_some(),
                        });

                        sqlx::query(
                            "UPDATE workflow_waiting_tasks \
                             SET status = 'APPROVED', completed_by = $2, completed_at = now(), \
                                 decision_payload = $3, updated_at = now() \
                             WHERE id = $1 AND status IN ('OPEN', 'CLAIMED')",
                        )
                        .bind(command.task_id)
                        .bind(*command.actor.as_uuid())
                        .bind(decision_payload.clone())
                        .execute(tx.as_mut())
                        .await
                        .map_err(PgWorkflowRuntimeError::from)?;

                        if let Some(node_run_id) = node_run_id {
                            sqlx::query(
                                "UPDATE workflow_node_runs \
                                 SET status = 'SUCCEEDED', finished_at = now(), updated_at = now(), \
                                     output_payload = $2 \
                                 WHERE id = $1 AND status = 'WAITING'",
                            )
                            .bind(node_run_id)
                            .bind(serde_json::json!({
                                "finalized": true,
                                "awaiting_receipt": receipt_spec.is_some()
                            }))
                            .execute(tx.as_mut())
                            .await
                            .map_err(PgWorkflowRuntimeError::from)?;
                        }

                        let final_run_status = if let Some(receipt) = &receipt_spec {
                            let receipt_node_run_id = Uuid::new_v4();
                            sqlx::query(
                                "INSERT INTO workflow_node_runs \
                                     (id, org_id, run_id, node_key, node_type, status, attempt, \
                                      idempotency_key, input_payload, started_at) \
                                 VALUES ($1, $2, $3, $4, 'human_task', 'WAITING', 1, $5, \
                                         '{}'::jsonb, now()) \
                                 ON CONFLICT (org_id, run_id, node_key, attempt) DO NOTHING",
                            )
                            .bind(receipt_node_run_id)
                            .bind(*org.as_uuid())
                            .bind(run_id)
                            .bind(RECEIPT_WAITING_KEY)
                            .bind(format!(
                                "workflow_runtime:node:{run_id}:{RECEIPT_WAITING_KEY}:1"
                            ))
                            .execute(tx.as_mut())
                            .await
                            .map_err(PgWorkflowRuntimeError::from)?;

                            let persisted_receipt_node_run_id: Uuid = sqlx::query_scalar(
                                "SELECT id FROM workflow_node_runs \
                                 WHERE run_id = $1 AND node_key = $2 AND attempt = 1",
                            )
                            .bind(run_id)
                            .bind(RECEIPT_WAITING_KEY)
                            .fetch_one(tx.as_mut())
                            .await
                            .map_err(PgWorkflowRuntimeError::from)?;

                            sqlx::query(
                                "INSERT INTO workflow_waiting_tasks \
                                     (id, org_id, run_id, node_run_id, waiting_key, title, status, \
                                      assignee_role_key, required_policy, form_payload) \
                                 VALUES ($1, $2, $3, $4, $5, $6, 'OPEN', $7, $8, '{}'::jsonb) \
                                 ON CONFLICT (org_id, run_id, waiting_key) DO NOTHING",
                            )
                            .bind(Uuid::new_v4())
                            .bind(*org.as_uuid())
                            .bind(run_id)
                            .bind(persisted_receipt_node_run_id)
                            .bind(RECEIPT_WAITING_KEY)
                            .bind(receipt.title.as_str())
                            .bind(receipt.assignee_role_key.as_deref())
                            .bind(receipt.required_policy.as_deref())
                            .execute(tx.as_mut())
                            .await
                            .map_err(PgWorkflowRuntimeError::from)?;

                            RunStatus::Waiting
                        } else {
                            sqlx::query(run_transition_sql(RunStatus::Succeeded))
                                .bind(run_id)
                                .bind(RunStatus::Succeeded.as_db_str())
                                .bind(RunStatus::Waiting.as_db_str())
                                .bind(serde_json::json!({ "finalized": true }))
                                .bind(Option::<serde_json::Value>::None)
                                .execute(tx.as_mut())
                                .await
                                .map_err(PgWorkflowRuntimeError::from)?;
                            RunStatus::Succeeded
                        };

                        let mut audit_events = command.transition_audits;
                        audit_events.push(
                            AuditEvent::new(
                                Some(command.actor),
                                AuditAction::new("workflow_task.finalize")
                                    .map_err(PgWorkflowRuntimeError::from)?,
                                "workflow_waiting_task",
                                command.task_id.to_string(),
                                TraceContext::generate(),
                                time::OffsetDateTime::now_utc(),
                            )
                            .with_org(org)
                            .with_snapshots(
                                Some(serde_json::json!({ "status": task_status.as_db_str() })),
                                Some(serde_json::json!({
                                    "status": WaitingTaskStatus::Approved.as_db_str(),
                                    "mode": decision_payload["mode"],
                                    "delegated_reason": decision_payload["delegated_reason"],
                                })),
                            ),
                        );
                        if let Some(node_run_id) = node_run_id {
                            audit_events.push(
                                AuditEvent::new(
                                    Some(command.actor),
                                    AuditAction::new("workflow_node.commit")
                                        .map_err(PgWorkflowRuntimeError::from)?,
                                    "workflow_node_run",
                                    node_run_id.to_string(),
                                    TraceContext::generate(),
                                    time::OffsetDateTime::now_utc(),
                                )
                                .with_org(org)
                                .with_snapshots(
                                    Some(serde_json::json!({ "status": "WAITING" })),
                                    Some(serde_json::json!({ "status": "SUCCEEDED" })),
                                ),
                            );
                        }
                        if receipt_spec.is_some() {
                            audit_events.push(
                                AuditEvent::new(
                                    Some(command.actor),
                                    AuditAction::new("workflow_node.commit")
                                        .map_err(PgWorkflowRuntimeError::from)?,
                                    "workflow_node_run",
                                    RECEIPT_WAITING_KEY.to_owned(),
                                    TraceContext::generate(),
                                    time::OffsetDateTime::now_utc(),
                                )
                                .with_org(org)
                                .with_snapshots(
                                    Some(serde_json::json!({ "status": "PENDING" })),
                                    Some(serde_json::json!({ "status": "WAITING" })),
                                ),
                            );
                        } else {
                            audit_events.push(
                                AuditEvent::new(
                                    Some(command.actor),
                                    AuditAction::new("workflow_run.transition")
                                        .map_err(PgWorkflowRuntimeError::from)?,
                                    "workflow_run",
                                    run_id.to_string(),
                                    TraceContext::generate(),
                                    time::OffsetDateTime::now_utc(),
                                )
                                .with_org(org)
                                .with_snapshots(
                                    Some(serde_json::json!({ "status": "WAITING" })),
                                    Some(serde_json::json!({ "status": "SUCCEEDED" })),
                                ),
                            );
                        }

                        Ok((
                            FinalizedWaitingTask {
                                task_id: command.task_id,
                                run_id,
                                status: WaitingTaskStatus::Approved,
                                completed_by: Some(command.actor),
                                decision_payload,
                                run_status: final_run_status,
                            },
                            audit_events,
                        ))
                    })
                },
            )
            .await
            .map_err(KernelError::from)
        })
    }

    // console-gate: state-changing-handler
    fn create_post_finalization_rejection<'a>(
        &'a self,
        org: OrgId,
        command: PostFinalizationRejectionCommand,
    ) -> PortFuture<'a, PostFinalizationRejection> {
        Box::pin(async move {
            with_audits::<_, PostFinalizationRejection, PgWorkflowRuntimeError>(
                &self.pool,
                org,
                move |tx| {
                    Box::pin(async move {
                        let existing = sqlx::query(
                            "SELECT c.id, c.original_run_id, c.reason, c.created_by, r.status AS run_status \
                             FROM workflow_compensating_documents c \
                             JOIN workflow_runs r ON r.id = c.original_run_id AND r.org_id = c.org_id \
                             WHERE c.idempotency_key = $1",
                        )
                        .bind(command.idempotency_key.as_str())
                        .fetch_optional(tx.as_mut())
                        .await
                        .map_err(PgWorkflowRuntimeError::from)?;
                        if let Some(row) = existing {
                            // Idempotency-key cross-run reuse (security L6): the key is
                            // UNIQUE per (org, idempotency_key), so a stored row whose
                            // original_run_id differs from the request is the SAME key
                            // aimed at a DIFFERENT run — a 409, never a silent replay of
                            // the wrong compensation (mirrors start_workflow_run's
                            // mismatch handling).
                            let stored_run_id: Uuid = row.try_get("original_run_id")?;
                            if stored_run_id != command.original_run_id {
                                return Err(PgWorkflowRuntimeError::from(KernelError::conflict(
                                    "idempotency_key already used for a different run",
                                )));
                            }
                            let run_status: String = row.try_get("run_status")?;
                            return Ok((
                                PostFinalizationRejection {
                                    id: row.try_get("id")?,
                                    original_run_id: stored_run_id,
                                    reason: row.try_get("reason")?,
                                    created_by: console_kernel_core::UserId::from_uuid(
                                        row.try_get("created_by")?,
                                    ),
                                    run_status: RunStatus::from_db_str(&run_status)?,
                                },
                                Vec::new(),
                            ));
                        }

                        let run = sqlx::query(
                            "SELECT status, definition_id, definition_version, object_type, object_id, initiated_by \
                             FROM workflow_runs \
                             WHERE id = $1 \
                             FOR UPDATE",
                        )
                        .bind(command.original_run_id)
                        .fetch_optional(tx.as_mut())
                        .await
                        .map_err(PgWorkflowRuntimeError::from)?;
                        let Some(run) = run else {
                            return Err(PgWorkflowRuntimeError::from(KernelError::not_found(
                                "workflow run not found",
                            )));
                        };
                        let run_status: String = run.try_get("status")?;
                        let run_status = RunStatus::from_db_str(&run_status)?;
                        if run_status != RunStatus::Succeeded {
                            return Err(PgWorkflowRuntimeError::from(KernelError::conflict(
                                "workflow run is not finalized",
                            )));
                        }

                        let mut recipient_ids = BTreeSet::<Uuid>::new();
                        if let Some(initiated_by) = run.try_get::<Option<Uuid>, _>("initiated_by")?
                        {
                            recipient_ids.insert(initiated_by);
                        }
                        let recipient_rows = sqlx::query(
                            "SELECT DISTINCT COALESCE(completed_by, claimed_by, assignee_user_id) AS user_id \
                             FROM workflow_waiting_tasks \
                             WHERE run_id = $1 \
                               AND COALESCE(completed_by, claimed_by, assignee_user_id) IS NOT NULL",
                        )
                        .bind(command.original_run_id)
                        .fetch_all(tx.as_mut())
                        .await
                        .map_err(PgWorkflowRuntimeError::from)?;
                        for row in recipient_rows {
                            recipient_ids.insert(row.try_get("user_id")?);
                        }
                        let recipients = recipient_ids
                            .into_iter()
                            .map(|id| id.to_string())
                            .collect::<Vec<_>>();
                        let recipient_count = recipients.len();

                        let compensation_id = Uuid::new_v4();
                        let payload = serde_json::json!({
                            "original_run_id": command.original_run_id,
                            "definition_id": run.try_get::<Uuid, _>("definition_id")?,
                            "definition_version": run.try_get::<i32, _>("definition_version")?,
                            "object_type": run.try_get::<Option<String>, _>("object_type")?,
                            "object_id": run.try_get::<Option<Uuid>, _>("object_id")?,
                        });
                        sqlx::query(
                            "INSERT INTO workflow_compensating_documents \
                                 (id, org_id, original_run_id, compensation_type, reason, \
                                  idempotency_key, payload, created_by) \
                             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                        )
                        .bind(compensation_id)
                        .bind(*org.as_uuid())
                        .bind(command.original_run_id)
                        .bind(POST_FINALIZATION_REJECTION)
                        .bind(command.reason.as_str())
                        .bind(command.idempotency_key.as_str())
                        .bind(payload)
                        .bind(*command.actor.as_uuid())
                        .execute(tx.as_mut())
                        .await
                        .map_err(PgWorkflowRuntimeError::from)?;

                        sqlx::query(
                            "INSERT INTO workflow_outbox_events \
                                 (org_id, run_id, channel, destination_ref, idempotency_key, \
                                  status, payload) \
                             VALUES ($1, $2, 'NOTIFICATION', 'approval_line', $3, 'PENDING', $4) \
                             ON CONFLICT (org_id, idempotency_key) DO NOTHING",
                        )
                        .bind(*org.as_uuid())
                        .bind(command.original_run_id)
                        .bind(format!(
                            "workflow_compensation:post_finalization_rejection:{compensation_id}:notify_line"
                        ))
                        .bind(serde_json::json!({
                            "event": "post_finalization_rejection",
                            "compensation_id": compensation_id,
                            "original_run_id": command.original_run_id,
                            "reason": command.reason,
                            "recipients": recipients,
                            "recipient_count": recipient_count,
                        }))
                        .execute(tx.as_mut())
                        .await
                        .map_err(PgWorkflowRuntimeError::from)?;

                        let mut audit_events = command.transition_audits;
                        audit_events.push(
                            AuditEvent::new(
                                Some(command.actor),
                                AuditAction::new(
                                    "workflow_compensation.create_post_finalization_rejection",
                                )
                                .map_err(PgWorkflowRuntimeError::from)?,
                                "workflow_compensating_document",
                                compensation_id.to_string(),
                                TraceContext::generate(),
                                time::OffsetDateTime::now_utc(),
                            )
                            .with_org(org)
                            .with_snapshots(
                                None,
                                Some(serde_json::json!({
                                    "status": "CREATED",
                                    "compensation_id": compensation_id,
                                    "compensation_type": POST_FINALIZATION_REJECTION,
                                    "original_run_id": command.original_run_id,
                                })),
                            ),
                        );

                        Ok((
                            PostFinalizationRejection {
                                id: compensation_id,
                                original_run_id: command.original_run_id,
                                reason: command.reason,
                                created_by: command.actor,
                                run_status,
                            },
                            audit_events,
                        ))
                    })
                },
            )
            .await
            .map_err(KernelError::from)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ilike_contains_pattern;

    #[test]
    fn ilike_pattern_escapes_metacharacters() {
        // Plain term is wrapped as a substring pattern.
        assert_eq!(ilike_contains_pattern("annual"), "%annual%");
        // LIKE metacharacters (\ % _) are escaped so they match literally, not as
        // wildcards — a search for "50%" must not match "50<anything>".
        assert_eq!(ilike_contains_pattern("50%"), "%50\\%%");
        assert_eq!(ilike_contains_pattern("a_b"), "%a\\_b%");
        assert_eq!(ilike_contains_pattern("c\\d"), "%c\\\\d%");
    }
}
