//! M2 workflow-runtime strangler seam (design §Strangler, step 7).
//!
//! Routes the work-order completion path (executive approval → `FINAL_COMPLETED`)
//! through the M2 workflow runtime for tenants whose per-tenant
//! `workflow_runtime_m2_strangler` flag is ON. The flag ships DARK (absent row ⇒
//! `org_runtime_flag_enabled()` ⇒ FALSE ⇒ every tenant OFF), so in production this
//! seam is inert and the legacy path runs byte-for-byte.
//!
//! ## Ordering (legacy-first, additive)
//! The legacy `approve_work_order` adapter call ALWAYS runs first and unchanged —
//! it performs the `work_orders` `FINAL_COMPLETED` mutation + audit in its own
//! transaction. Only *after* the completion has persisted, and only when the flag
//! resolves ON, does this seam additively drive the runtime to RECORD the
//! completion run and enqueue the payroll JOB outbox event. A runtime failure never
//! rolls back (or blocks) a completion that already committed; the worst case is a
//! missing async payroll draft, which the outbox drainer restages. The OFF path
//! executes exactly one read-only flag SELECT and returns — no `workflow_runs`,
//! node runs, waiting tasks, outbox events, or `payroll_draft_runs` rows are
//! written, so it is byte-identical to legacy.
//!
//! ## Scope (design mismatch, deliberately reported)
//! As-built, the runtime interpreter's `object_mutation` node does NOT itself write
//! `work_orders` (the caller's transaction does — `interpreter.rs:24-27`) and no
//! cross-context shared-transaction API exists, so the design's "runtime owns the
//! FinalCompleted mutation in its own txn" is not buildable within the slice's
//! constraints. This seam follows the as-built contract: legacy owns the mutation;
//! the runtime records the terminal tail `apply_completion` → `emit_payroll`. The
//! admin/executive human-task gates were already enforced by the legacy approval
//! line, so they are treated as satisfied preconditions rather than re-modelled as
//! runtime waiting tasks (which would require start/park wiring at earlier lifecycle
//! steps — a separate charter).

use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, ErrorKind, KernelError, OrgId, TraceContext, UserId,
    WorkOrderId,
};
use console_platform_authz::{AuthorizationAuditEvent, Feature, Principal};
use console_platform_db::{DbError, with_org_conn};
use console_platform_request_context::current_org;
use console_workflow_domain::{RunStatus, TriggerType};
use console_workflow_runtime::idempotency::run_completion_key;
use console_workflow_runtime::{
    AuditContext, NODE_TRANSITION_DOMAIN, NodeKind, NodeSpec, ProcessNodeRequest, StartRunRequest,
    build_guard_request, guard, process_node, start_run, workflow_coexistence_entry,
};
use console_workflow_runtime_adapter_postgres::{M2_STRANGLER_FLAG, PgWorkflowRuntimeStore};
use serde_json::json;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::RestError;

/// The single canonical `workflow_key` the work-order completion tail binds to.
/// The definition lookup is pinned to this key (plus `object_type = 'work_order'`,
/// `status = 'ACTIVE'`, an active `wf.exec.v1` version) and FAILS CLOSED on
/// zero-or-multiple candidates, so the runtime tail can never silently attach to
/// the wrong definition/version once a tenant is flag-on. `workflow_definitions`
/// enforces `UNIQUE(org_id, workflow_key)` (0069:23), so a resolved match is
/// unambiguous per tenant.
const WORK_ORDER_COMPLETION_WORKFLOW_KEY: &str = "work_order.completion";

/// Drive the M2 completion runtime for a work order that just reached
/// `FINAL_COMPLETED` through the (unchanged) legacy path — but ONLY if the tenant's
/// strangler flag is ON. A no-op (single read-only flag SELECT) for every OFF
/// tenant, so the completion path stays byte-identical to legacy when dark.
pub(crate) async fn drive_completion_if_enabled(
    runtime: &PgWorkflowRuntimeStore,
    principal: &Principal,
    branch_id: BranchId,
    work_order_id: WorkOrderId,
) -> Result<(), RestError> {
    let org = current_org().map_err(|err| RestError::from_kernel(err.into()))?;

    // Dark default: absent row ⇒ FALSE ⇒ legacy path, zero runtime state written.
    if !runtime
        .strangler_flag_enabled(org, M2_STRANGLER_FLAG)
        .await
        .map_err(RestError::from_kernel)?
    {
        return Ok(());
    }

    // Resolve the tenant's active executable (`wf.exec.v1`) work-order completion
    // definition. Without one published there is nothing to drive (the completion
    // already persisted via legacy) — a real, benign skip, not a stub.
    let Some((definition_id, definition_version)) =
        resolve_completion_definition(runtime.pool(), org)
            .await
            .map_err(RestError::from_kernel)?
    else {
        tracing::info!(
            org = %org,
            work_order_id = %work_order_id,
            "m2 strangler: flag ON but no unambiguous active wf.exec.v1 work_order definition; skipping runtime record"
        );
        return Ok(());
    };

    record_completion_run(
        runtime,
        principal,
        org,
        branch_id,
        work_order_id,
        definition_id,
        definition_version,
    )
    .await
    .map_err(RestError::from_kernel)
}

/// Find the tenant's active executable work-order completion definition, pinned to
/// the canonical [`WORK_ORDER_COMPLETION_WORKFLOW_KEY`] (`status = ACTIVE`,
/// `object_type = 'work_order'`, an active `wf.exec.v1` version). Read as the
/// runtime role under the armed `app.current_org` (via `with_org_conn`), so RLS
/// scopes it to this tenant.
///
/// FAILS CLOSED (returns `None` ⇒ the caller SKIPS the runtime record) on
/// zero-or-multiple candidates rather than silently picking one by `updated_at`:
/// binding to the unique `workflow_key` means the runtime tail can never attach to
/// the wrong definition/version. `Some((id, version))` only when EXACTLY one active
/// `wf.exec.v1` definition matches.
async fn resolve_completion_definition(
    pool: &sqlx::PgPool,
    org: OrgId,
) -> Result<Option<(Uuid, i32)>, KernelError> {
    with_org_conn::<_, Option<(Uuid, i32)>, DbError>(pool, org, move |tx| {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT d.id, d.active_version \
                 FROM workflow_definitions d \
                 JOIN workflow_definition_versions v \
                   ON v.definition_id = d.id AND v.version = d.active_version \
                 WHERE d.workflow_key = $1 \
                   AND d.object_type = 'work_order' \
                   AND d.status = 'ACTIVE' \
                   AND d.active_version IS NOT NULL \
                   AND v.definition->>'schema_version' = 'wf.exec.v1'",
            )
            .bind(WORK_ORDER_COMPLETION_WORKFLOW_KEY)
            .fetch_all(tx.as_mut())
            .await?;
            // Exactly one candidate, or fail closed (skip). Zero ⇒ nothing published;
            // more than one ⇒ ambiguous (should be impossible under
            // UNIQUE(org_id, workflow_key)) — never guess a version.
            match rows.as_slice() {
                [row] => {
                    let id: Uuid = row.try_get("id")?;
                    let version: i32 = row.try_get("active_version")?;
                    Ok(Some((id, version)))
                }
                _ => Ok(None),
            }
        })
    })
    .await
    .map_err(|db| KernelError::internal(format!("workflow strangler definition lookup: {db}")))
}

/// The per-request (REST executive-approval) completion record. Builds the sole
/// per-request Cedar shadow for the `apply_completion` transition (design §D):
/// under `LegacyOnly` the legacy contract is the sole enforcer (fail-closed if it
/// denies), the inert Cedar verdict never blocks, and the shadow decision rides the
/// node's own `with_audits` txn. Delegates the actual run/node writes to the shared
/// [`drive_completion_tail`], so the REST path and the crash reconciler drive the
/// exact same idempotent tail.
async fn record_completion_run(
    runtime: &PgWorkflowRuntimeStore,
    principal: &Principal,
    org: OrgId,
    branch_id: BranchId,
    work_order_id: WorkOrderId,
    definition_id: Uuid,
    definition_version: i32,
) -> Result<(), KernelError> {
    let actor = principal.user_id;
    let now = OffsetDateTime::now_utc();

    // Sole per-request Cedar-guarded transition (apply_completion). Evaluate it
    // BEFORE any run row exists so a deny never orphans a run; under LegacyOnly the
    // legacy contract is the enforcer and the shadow verdict is recorded, never
    // enforced.
    let guard_request = build_guard_request(
        principal,
        "completion_review",
        org,
        branch_id,
        "work_order",
        &work_order_id.to_string(),
        NODE_TRANSITION_DOMAIN,
    )?;
    let entry = workflow_coexistence_entry(
        "workflow.node_transition.completion_review",
        NODE_TRANSITION_DOMAIN,
        Feature::CompletionReview,
        "work_order",
    );
    let guard_outcome = guard(&guard_request, &entry);
    if !guard_outcome.is_allowed() {
        return Err(KernelError::forbidden(
            "workflow runtime completion transition denied by the legacy authorization contract",
        ));
    }
    let shadow_audit = shadow_audit_event(&guard_outcome.audit, actor, org, work_order_id, now)?;

    drive_completion_tail(
        runtime,
        org,
        work_order_id,
        Some(actor),
        definition_id,
        definition_version,
        vec![shadow_audit],
    )
    .await
}

/// Drive (or RESUME) the terminal completion tail through the runtime engine +
/// Postgres adapter: `start_run` (STARTING→RUNNING) → `apply_completion`
/// (object_mutation, run stays RUNNING) → `emit_payroll` (job, run→SUCCEEDED).
/// Every write is armed + audited by the adapter's own `with_audits`/`with_audit`
/// transactions.
///
/// ## Crash-safe & idempotent (design §Strangler + recovery reconciler)
/// The three engine steps commit in separate transactions, so a crash can leave a
/// partial run. Driving the SAME completion again is therefore safe:
/// * `start_run` is keyed on the deterministic `run_completion_key`. On the
///   `UNIQUE(org_id, idempotency_key)` conflict we LOAD the existing run instead of
///   aborting: if it already reached a terminal status the tail is complete (a
///   no-op), otherwise we RESUME from the run's current status.
/// * the node/outbox inserts are `ON CONFLICT DO NOTHING`, and every status UPDATE
///   is guarded on the expected `from` status, so re-running an already-recorded
///   node is a no-op and never duplicates the payroll JOB outbox event.
///
/// Net: driving the same completion twice yields exactly one run, one payroll draft,
/// and success — never a 409 conflict. `actor` is `None` for a system re-drive (the
/// crash reconciler); `apply_completion_guard_audits` carries the REST path's Cedar
/// shadow and is empty for the reconciler (the legacy approval already enforced
/// authz).
pub async fn drive_completion_tail(
    runtime: &PgWorkflowRuntimeStore,
    org: OrgId,
    work_order_id: WorkOrderId,
    actor: Option<UserId>,
    definition_id: Uuid,
    definition_version: i32,
    apply_completion_guard_audits: Vec<AuditEvent>,
) -> Result<(), KernelError> {
    let now = OffsetDateTime::now_utc();
    let audit = AuditContext {
        actor,
        trace: TraceContext::generate(),
        occurred_at: now,
    };
    let work_order_uuid = *work_order_id.as_uuid();
    let idempotency_key = run_completion_key(work_order_id);

    // 1. Start the run (STARTING → RUNNING), or resume an existing run on the
    //    deterministic-key conflict rather than aborting. `run_status` is where the
    //    run actually sits before the tail's nodes run.
    let (run_id, run_status) = match start_run(
        runtime,
        StartRunRequest {
            run_id: Uuid::new_v4(),
            org_id: org,
            definition_id,
            definition_version,
            trigger_type: TriggerType::ObjectEvent,
            object_type: Some("work_order".to_owned()),
            object_id: Some(work_order_uuid),
            idempotency_key: idempotency_key.clone(),
            correlation_id: format!("work_order_completion:{work_order_id}"),
            trace_id: None,
            input_payload: json!({ "work_order_id": work_order_uuid }),
            context_payload: json!({}),
            initiated_by: actor,
            schedule_id: None,
        },
        &audit,
    )
    .await
    {
        Ok(run_id) => (run_id, RunStatus::Running),
        Err(err) if err.kind == ErrorKind::Conflict => {
            // A prior (partial or complete) run exists under this completion key.
            let Some(existing) = runtime
                .load_run_by_idempotency_key(org, idempotency_key)
                .await?
            else {
                return Err(err);
            };
            if existing.status.is_terminal() {
                // The tail already completed on an earlier drive — idempotent no-op.
                return Ok(());
            }
            (existing.id, existing.status)
        }
        Err(err) => return Err(err),
    };

    // 2. apply_completion (object_mutation). The `work_orders` FINAL_COMPLETED write
    //    was already performed by the legacy path; this node records the terminal
    //    business transition and advances the run to RUNNING if it was resumed from
    //    STARTING/WAITING (a fresh run is already RUNNING, so no transition).
    process_node(
        runtime,
        ProcessNodeRequest {
            org_id: org,
            run_id,
            node_run_id: Uuid::new_v4(),
            current_run_status: run_status,
            run_target: RunStatus::Running,
            spec: NodeSpec {
                node_key: "apply_completion".to_owned(),
                node_type: "object_mutation".to_owned(),
                kind: NodeKind::ObjectMutation,
            },
            attempt: 1,
            input_payload: json!({
                "work_order_id": work_order_uuid,
                "target_status": "FINAL_COMPLETED",
            }),
            guard_audits: apply_completion_guard_audits,
        },
        &audit,
    )
    .await?;

    // 3. emit_payroll (job → run SUCCEEDED). A worker-driven system node: audited but
    //    NOT per-request Cedar-guarded (design §D). Enqueues exactly one JOB outbox
    //    event to internal.jobs; the drainer later stages a BLOCKED_LEGAL_GATE draft.
    let period = now.date();
    process_node(
        runtime,
        ProcessNodeRequest {
            org_id: org,
            run_id,
            node_run_id: Uuid::new_v4(),
            current_run_status: RunStatus::Running,
            run_target: RunStatus::Succeeded,
            spec: NodeSpec {
                node_key: "emit_payroll".to_owned(),
                node_type: "job".to_owned(),
                kind: NodeKind::Job {
                    connector: "internal.jobs".to_owned(),
                    job: "payroll_draft".to_owned(),
                    emits_status: Some("BLOCKED_LEGAL_GATE".to_owned()),
                },
            },
            attempt: 1,
            input_payload: json!({
                "work_order_id": work_order_uuid,
                "period_start": period.to_string(),
                "period_end": period.to_string(),
            }),
            guard_audits: Vec::new(),
        },
        &audit,
    )
    .await?;

    Ok(())
}

/// RECOVERY RECONCILER (design §Strangler crash-safety). One per-tenant pass, run by
/// the outbox drain worker under `scope_org` as `console_rt`.
///
/// The legacy path commits `FINAL_COMPLETED` and only THEN does the REST handler run
/// the runtime tail across several separate transactions. A crash in between leaves a
/// `FINAL_COMPLETED` work order whose completion tail never reached `SUCCEEDED` — in
/// either of two windows:
/// * the tail never started, so there is NO completion run at all; or
/// * `start_run` committed the run row but the process died before `emit_payroll`
///   wrote the JOB outbox event — a partial run stuck non-terminal, with nothing for
///   the drainer to consume, so the payroll draft is never staged.
///
/// Both leave the drainer (which only claims existing outbox rows) unable to stage the
/// payroll draft. This pass closes both windows: [`find_completions_needing_tail`]
/// selects every `FINAL_COMPLETED` work order without a `SUCCEEDED` completion run
/// (keyed on the deterministic `run_completion_key`) and re-drives the tail via the
/// shared [`drive_completion_tail`], which is idempotent and RESUMES a partial run —
/// emitting the missing outbox event and driving it to `SUCCEEDED` — rather than
/// aborting or duplicating. Returns the number of tails restaged.
///
/// Dark-safe: does nothing when the tenant's strangler flag is OFF, and nothing when
/// no unambiguous `wf.exec.v1` completion definition is published — so it is inert in
/// production exactly like the rest of M2.
pub async fn reconcile_completion_tails(
    runtime: &PgWorkflowRuntimeStore,
    org: OrgId,
) -> Result<u64, KernelError> {
    // Dark default: never touch a tenant that is not enrolled (flag OFF ⇒ legacy).
    if !runtime
        .strangler_flag_enabled(org, M2_STRANGLER_FLAG)
        .await?
    {
        return Ok(0);
    }
    // Without exactly one active wf.exec.v1 definition there is nothing to bind a
    // run to (fail-closed, same as the REST path).
    let Some((definition_id, definition_version)) =
        resolve_completion_definition(runtime.pool(), org).await?
    else {
        return Ok(0);
    };

    let needing_tail = find_completions_needing_tail(runtime.pool(), org).await?;
    let mut restaged: u64 = 0;
    for work_order_id in needing_tail {
        // System re-drive: no Principal (the legacy approval already enforced authz),
        // so no per-request Cedar shadow — an empty guard-audit set.
        match drive_completion_tail(
            runtime,
            org,
            work_order_id,
            None,
            definition_id,
            definition_version,
            Vec::new(),
        )
        .await
        {
            Ok(()) => restaged += 1,
            Err(err) => tracing::warn!(
                org = %org,
                work_order_id = %work_order_id,
                error = %err.message,
                "m2 reconciler: completion tail re-drive failed; will retry next tick"
            ),
        }
    }
    if restaged > 0 {
        tracing::info!(
            org = %org,
            restaged,
            "m2 reconciler: restaged crash-orphaned FINAL_COMPLETED completion tails"
        );
    }
    Ok(restaged)
}

/// Find `FINAL_COMPLETED` work orders whose completion tail has NOT yet reached the
/// terminal `SUCCEEDED` state — keyed on the deterministic `run_completion_key`
/// (`run:work_order:{id}:completion:v1`). This deliberately selects BOTH crash
/// windows, not just the wider one:
/// * NO completion run row at all (crash before `start_run` committed), and
/// * a completion run that EXISTS but is NON-`SUCCEEDED` (a partial run: `start_run`
///   committed the row, then the process died before `emit_payroll` wrote the JOB
///   outbox event, so the run is stuck STARTING/RUNNING/WAITING with no outbox for
///   the drainer to consume).
///
/// Both are re-driven by the same idempotent [`drive_completion_tail`], whose resume
/// path completes the missing steps. A run already `SUCCEEDED` is excluded, so it is
/// never re-selected (idempotent — re-driving one would be a clean no-op anyway).
///
/// Read as `console_rt` under the armed `app.current_org` (via `with_org_conn`), so RLS
/// scopes both `work_orders` and the `workflow_runs` existence check to this tenant.
//
// ponytail: bounded per-tick scan (LIMIT). A dark/enrolled tenant has ~zero of these
// (a crash mid-tail is rare); the next tick picks up any remainder. Swap for a keyset
// cursor only if a real backlog ever makes one pass hurt.
async fn find_completions_needing_tail(
    pool: &sqlx::PgPool,
    org: OrgId,
) -> Result<Vec<WorkOrderId>, KernelError> {
    with_org_conn::<_, Vec<WorkOrderId>, DbError>(pool, org, move |tx| {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT wo.id \
                 FROM work_orders wo \
                 WHERE wo.status = 'FINAL_COMPLETED' \
                   AND NOT EXISTS ( \
                       SELECT 1 FROM workflow_runs wr \
                       WHERE wr.idempotency_key = \
                             'run:work_order:' || wo.id::text || ':completion:v1' \
                         AND wr.status = 'SUCCEEDED' \
                   ) \
                 LIMIT 500",
            )
            .fetch_all(tx.as_mut())
            .await?;
            let mut ids = Vec::with_capacity(rows.len());
            for row in &rows {
                let id: Uuid = row.try_get("id")?;
                ids.push(WorkOrderId::from_uuid(id));
            }
            Ok(ids)
        })
    })
    .await
    .map_err(|db| KernelError::internal(format!("workflow reconciler completion scan: {db}")))
}

/// Bridge a Cedar/PBAC observe-and-record shadow event into a kernel `AuditEvent`
/// so it can land in the guarded node's own `with_audits` transaction. The full
/// server-derived shadow record (enforced decision + inert Cedar verdict) is
/// preserved in the audit `after` snapshot for the forensic trail.
fn shadow_audit_event(
    shadow: &AuthorizationAuditEvent,
    actor: UserId,
    org: OrgId,
    work_order_id: WorkOrderId,
    occurred_at: OffsetDateTime,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("workflow_runtime.cedar_shadow")?,
        "work_order",
        work_order_id.to_string(),
        TraceContext::generate(),
        occurred_at,
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(serde_json::to_value(shadow).map_err(|err| KernelError::internal(err.to_string()))?),
    ))
}
