//! Domain-event → workflow trigger dispatcher (BE-AUTO slice 1, closes
//! adequacy-audit gap 8).
//!
//! The small event registry the audited-mutation commit points publish into.
//! For a registered event key (`console_workflow_domain::REGISTERED_EVENT_KEYS`)
//! the dispatcher evaluates an ORDERED list of bindings:
//!
//!   1. the BUILT-IN work-order-completion binding — the previously hardcoded
//!      inline start ([`crate::m2_strangler::drive_completion_if_enabled`]),
//!      now the first binding this mechanism evaluates. Behavior is identical:
//!      strangler-flag-gated, pinned to the graph-less
//!      `work_order.completion` template, driven by its own completion tail;
//!   2. every ENABLED `workflow_trigger_bindings` row (0105) for the event,
//!      oldest first — each starts one idempotent run through the shared
//!      [`console_workflow_runtime::start_bound_run`] path (`start_run` →
//!      synchronous graph drive), exactly like `POST /api/v1/workflow-runs`.
//!
//! ## Failure isolation
//! The publish happens AFTER the legacy mutation committed, so a binding
//! failure must never fail the request the tenant already saw succeed — each
//! binding is evaluated independently and a failure is logged (no PII: ids
//! only) and skipped. Idempotency (`trigger:{binding_id}:{object_id}` against
//! the run spine's `UNIQUE(org_id, idempotency_key)`) makes a re-publish of
//! the same event occurrence a no-op, never a duplicate run.
//!
//! ponytail: the dispatch loop lives beside its only producer (work-order
//! completion). When a second domain publishes events, hoist
//! `dispatch_event_bindings` into a shared workflow crate rather than copying
//! it.

use console_kernel_core::{BranchId, KernelError, OrgId, TraceContext, UserId, WorkOrderId};
use console_platform_authz::Principal;
use console_platform_request_context::current_org;
use console_workflow_runtime::{AuditContext, StartRunRequest, TriggeredStart, start_bound_run};
use console_workflow_runtime_adapter_postgres::PgWorkflowRuntimeStore;
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::RestError;
use crate::m2_strangler;

/// The event published when the legacy work-order completion (executive
/// approval → `FINAL_COMPLETED`) has committed. Must stay in
/// `console_workflow_domain::REGISTERED_EVENT_KEYS`.
pub const WORK_ORDER_COMPLETED_EVENT: &str = "work_order.completed";

/// Publish `work_order.completed` for a work order that just reached
/// `FINAL_COMPLETED` through the (unchanged) legacy path. Evaluates the
/// built-in completion binding first, then the tenant's enabled DB bindings.
/// Returns the number of NEW runs the DB bindings started.
pub(crate) async fn publish_work_order_completed(
    runtime: &PgWorkflowRuntimeStore,
    principal: &Principal,
    branch_id: BranchId,
    work_order_id: WorkOrderId,
) -> Result<u32, RestError> {
    // Binding 1 (built-in): the M2 completion-tail strangler, byte-identical to
    // the pre-registry inline start (dark by default; single flag SELECT when
    // OFF). Its failure is isolated exactly as before — logged, so the DB
    // bindings still evaluate.
    if let Err(err) =
        m2_strangler::drive_completion_if_enabled(runtime, principal, branch_id, work_order_id)
            .await
    {
        tracing::warn!(
            error = %err.message,
            work_order_id = %work_order_id,
            "workflow triggers: built-in completion binding failed (completion already persisted)"
        );
    }

    // Bindings 2..n: enabled `workflow_trigger_bindings` rows for the event.
    let org = current_org().map_err(|err| RestError::from_kernel(err.into()))?;
    dispatch_event_bindings(
        runtime,
        org,
        Some(principal.user_id),
        WORK_ORDER_COMPLETED_EVENT,
        "work_order",
        *work_order_id.as_uuid(),
    )
    .await
    .map_err(RestError::from_kernel)
}

/// Evaluate every enabled DB binding for `event_key` and start one idempotent
/// run per binding. Per-binding failures (unresolvable/graph-less definition,
/// engine error) are logged and skipped — one bad rule never blocks the rest.
/// `actor` is the user whose audited mutation raised the event (`None` for a
/// system producer); it rides the run + audit rows as automation provenance.
pub async fn dispatch_event_bindings(
    runtime: &PgWorkflowRuntimeStore,
    org: OrgId,
    actor: Option<UserId>,
    event_key: &str,
    object_type: &str,
    object_id: Uuid,
) -> Result<u32, KernelError> {
    let bindings = runtime
        .list_enabled_trigger_bindings(org, event_key)
        .await?;
    if bindings.is_empty() {
        return Ok(0);
    }

    let mut started = 0u32;
    for binding in bindings {
        // Resolve the binding's ACTIVE wf.exec.v1 definition; a binding whose
        // definition was paused/retired since authoring SKIPS (fail-safe).
        let resolved = match runtime
            .resolve_active_exec_definition(org, binding.definition_id)
            .await
        {
            Ok(Some(resolved)) => resolved,
            Ok(None) => {
                tracing::warn!(
                    binding_id = %binding.id,
                    definition_id = %binding.definition_id,
                    event_key,
                    "workflow triggers: binding definition is not an ACTIVE wf.exec.v1 definition; skipping"
                );
                continue;
            }
            Err(err) => {
                tracing::warn!(
                    binding_id = %binding.id,
                    error = %err.message,
                    "workflow triggers: binding definition resolve failed; skipping"
                );
                continue;
            }
        };
        let (version, definition) = resolved;

        let run_id = Uuid::new_v4();
        let request = StartRunRequest {
            run_id,
            org_id: org,
            definition_id: binding.definition_id,
            definition_version: version,
            trigger_type: binding.trigger_type,
            object_type: Some(object_type.to_owned()),
            object_id: Some(object_id),
            // Deterministic per (binding, event occurrence): a re-publish or a
            // concurrent dispatch of the same completion starts exactly one run.
            idempotency_key: format!("trigger:{}:{}", binding.id, object_id),
            correlation_id: format!("trigger:{event_key}:{object_id}"),
            trace_id: None,
            input_payload: json!({
                "event_key": event_key,
                "object_type": object_type,
                "object_id": object_id,
            }),
            context_payload: json!({}),
            initiated_by: actor,
            schedule_id: None,
        };
        let audit = AuditContext {
            actor,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        };

        match start_bound_run(runtime, request, &definition, &audit).await {
            Ok(TriggeredStart::Started { run_id, .. }) => {
                started += 1;
                tracing::info!(
                    binding_id = %binding.id,
                    run_id = %run_id,
                    event_key,
                    "workflow triggers: binding started run"
                );
            }
            Ok(TriggeredStart::AlreadyStarted) => {
                tracing::debug!(
                    binding_id = %binding.id,
                    event_key,
                    "workflow triggers: event occurrence already handled; skipping"
                );
            }
            Err(err) => {
                tracing::warn!(
                    binding_id = %binding.id,
                    error = %err.message,
                    event_key,
                    "workflow triggers: binding run start failed; skipping"
                );
            }
        }
    }
    Ok(started)
}
