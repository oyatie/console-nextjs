//! System-triggered run starts (BE-AUTO slice 1).
//!
//! One shared entry for every NON-human run producer — the domain-event
//! trigger-binding dispatcher and the cron schedule poller — so both start a
//! run exactly the way the REST `POST /api/v1/workflow-runs` path does
//! (`start_run` → synchronous [`drive_from`] until the first WAITING task or a
//! terminal node) without duplicating the walk. Pure over the domain port: no
//! sqlx here; callers resolve the published definition JSON themselves.
//!
//! ## Exactly-once
//! The caller supplies a DETERMINISTIC `idempotency_key` (e.g.
//! `trigger:{binding_id}:{object_id}` or `schedule:{schedule_id}:{fire_unix}`).
//! The run spine's `UNIQUE(org_id, idempotency_key)` turns a concurrent double
//! dispatch/poll into a Conflict; this module reloads the existing run and
//! either resumes an in-flight run or reports a benign already-handled skip.
//!
//! ## Authorization
//! A system start carries NO per-request principal: the authority is the
//! audited AUTHORING act (creating/enabling the binding or schedule required
//! the workflow-manage feature), matching how the m2 completion reconciler
//! re-drives with an empty guard set. Every write is still fully audited (actor
//! `None` ⇒ system) through the port's own `with_audit(s)` transactions.

use console_kernel_core::{ErrorKind, KernelError};
use console_workflow_domain::{RunStatus, RunTransition, WorkflowRuntimePort};
use serde_json::Value;

use crate::engine::{AuditContext, StartRunRequest, run_audit_event, start_run};
use crate::graph::{ExecGraph, drive_from};

/// How a system-triggered start landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggeredStart {
    /// A fresh run was created and driven to its first WAITING task or a
    /// terminal node.
    Started {
        run_id: uuid::Uuid,
        run_status: RunStatus,
    },
    /// A run under the same deterministic idempotency key already exists — the
    /// event/fire was already handled (possibly by a concurrent dispatcher).
    AlreadyStarted,
}

#[derive(Debug, Clone, Copy)]
struct RequestedDefinition {
    id: uuid::Uuid,
    version: i32,
}

/// Start a run for a trigger binding or schedule fire and drive it through its
/// `wf.exec.v1` node graph. Fails with a validation error when the definition
/// carries no executable node graph (e.g. the graph-less strangler completion
/// template, which is driven by its own hard-coded tail instead).
pub async fn start_bound_run<P: WorkflowRuntimePort + ?Sized>(
    port: &P,
    request: StartRunRequest,
    definition: &Value,
    audit: &AuditContext,
) -> Result<TriggeredStart, KernelError> {
    let graph = ExecGraph::parse(definition)?;
    let entry = graph.entry_node_key()?.to_owned();
    let org = request.org_id;
    let requested_definition = RequestedDefinition {
        id: request.definition_id,
        version: request.definition_version,
    };
    let idempotency_key = request.idempotency_key.clone();
    // The run context condition nodes evaluate against. A deterministic
    // re-dispatch/re-poll carries the same context, so a resumed run branches
    // identically.
    let context = request.context_payload.clone();

    match start_run(port, request, audit).await {
        Ok(run_id) => {
            let outcome = drive_from(
                port,
                org,
                run_id,
                RunStatus::Running,
                &graph,
                &entry,
                Vec::new(),
                &context,
                audit,
            )
            .await?;
            Ok(TriggeredStart::Started {
                run_id,
                run_status: outcome.run_status,
            })
        }
        // UNIQUE(org_id, idempotency_key): this fire/event already has a run.
        // Inspect it instead of blindly skipping so a crash after INSERT but
        // before/within graph drive can be resumed.
        Err(err) if err.kind == ErrorKind::Conflict => {
            resume_conflicted_run(
                port,
                org,
                idempotency_key,
                requested_definition,
                &graph,
                &entry,
                &context,
                audit,
            )
            .await
        }
        Err(err) => Err(err),
    }
}

#[allow(clippy::too_many_arguments)]
async fn resume_conflicted_run<P: WorkflowRuntimePort + ?Sized>(
    port: &P,
    org: console_kernel_core::OrgId,
    idempotency_key: String,
    requested_definition: RequestedDefinition,
    graph: &ExecGraph,
    entry: &str,
    context: &Value,
    audit: &AuditContext,
) -> Result<TriggeredStart, KernelError> {
    let Some(existing) = port
        .load_run_by_idempotency_key(org, idempotency_key)
        .await?
    else {
        return Err(KernelError::conflict(
            "workflow run idempotency conflict but existing run was not found",
        ));
    };

    if existing.definition_id != requested_definition.id
        || existing.definition_version != requested_definition.version
    {
        return Err(KernelError::conflict(
            "workflow run idempotency conflict belongs to a different definition version",
        ));
    }

    match existing.status {
        RunStatus::Starting => {
            let transition = RunTransition {
                run_id: existing.id,
                from: RunStatus::Starting,
                to: RunStatus::Running,
                output_payload: None,
                error_payload: None,
            };
            let transition_audit = run_audit_event(
                "workflow_run.transition",
                audit,
                existing.id,
                org,
                Some(serde_json::json!({ "status": RunStatus::Starting.as_db_str() })),
                Some(serde_json::json!({ "status": RunStatus::Running.as_db_str() })),
            )?;
            if let Err(err) = port.transition_run(org, transition, transition_audit).await {
                return if err.kind == ErrorKind::Conflict {
                    Ok(TriggeredStart::AlreadyStarted)
                } else {
                    Err(err)
                };
            }
            let _ = drive_existing_running(port, org, existing.id, graph, entry, context, audit)
                .await?;
            Ok(TriggeredStart::AlreadyStarted)
        }
        RunStatus::Running => {
            let _ = drive_existing_running(port, org, existing.id, graph, entry, context, audit)
                .await?;
            Ok(TriggeredStart::AlreadyStarted)
        }
        RunStatus::Waiting => Ok(TriggeredStart::AlreadyStarted),
        RunStatus::Succeeded
        | RunStatus::Failed
        | RunStatus::Cancelled
        | RunStatus::DeadLettered => Ok(TriggeredStart::AlreadyStarted),
    }
}

#[allow(clippy::too_many_arguments)]
async fn drive_existing_running<P: WorkflowRuntimePort + ?Sized>(
    port: &P,
    org: console_kernel_core::OrgId,
    run_id: uuid::Uuid,
    graph: &ExecGraph,
    entry: &str,
    context: &Value,
    audit: &AuditContext,
) -> Result<TriggeredStart, KernelError> {
    let outcome = drive_from(
        port,
        org,
        run_id,
        RunStatus::Running,
        graph,
        entry,
        Vec::new(),
        context,
        audit,
    )
    .await?;
    Ok(TriggeredStart::Started {
        run_id,
        run_status: outcome.run_status,
    })
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::task::{Context, Poll, Waker};

    use console_kernel_core::{ErrorKind, OrgId, TraceContext, UserId};
    use console_workflow_domain::{
        FinalizeWaitingTaskCommand, FinalizeWaitingTaskContext, FinalizedWaitingTask, NewRun,
        NodeStepCommit, PortFuture, PostFinalizationRejection, PostFinalizationRejectionCommand,
        RunRecord, TriggerType,
    };
    use serde_json::json;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    struct ConflictPort {
        org: OrgId,
        idempotency_key: String,
        existing: RunRecord,
        transitions: Mutex<Vec<RunTransition>>,
        commits: Mutex<Vec<NodeStepCommit>>,
    }

    impl WorkflowRuntimePort for ConflictPort {
        fn insert_run<'a>(
            &'a self,
            _run: NewRun,
            _audit: console_kernel_core::AuditEvent,
        ) -> PortFuture<'a, ()> {
            Box::pin(async { Err(KernelError::conflict("duplicate idempotency key")) })
        }

        fn load_run<'a>(&'a self, _org: OrgId, run_id: Uuid) -> PortFuture<'a, Option<RunRecord>> {
            Box::pin(async move { Ok((run_id == self.existing.id).then(|| self.existing.clone())) })
        }

        fn load_run_by_idempotency_key<'a>(
            &'a self,
            org: OrgId,
            idempotency_key: String,
        ) -> PortFuture<'a, Option<RunRecord>> {
            Box::pin(async move {
                Ok((org == self.org && idempotency_key == self.idempotency_key)
                    .then(|| self.existing.clone()))
            })
        }

        fn transition_run<'a>(
            &'a self,
            _org: OrgId,
            transition: RunTransition,
            _audit: console_kernel_core::AuditEvent,
        ) -> PortFuture<'a, ()> {
            Box::pin(async move {
                self.transitions.lock().unwrap().push(transition);
                Ok(())
            })
        }

        fn commit_node_step<'a>(
            &'a self,
            _org: OrgId,
            commit: NodeStepCommit,
        ) -> PortFuture<'a, ()> {
            Box::pin(async move {
                self.commits.lock().unwrap().push(commit);
                Ok(())
            })
        }

        fn load_finalize_waiting_task<'a>(
            &'a self,
            _org: OrgId,
            _task_id: Uuid,
        ) -> PortFuture<'a, Option<FinalizeWaitingTaskContext>> {
            Box::pin(async { Ok(None) })
        }

        fn finalize_waiting_task<'a>(
            &'a self,
            _org: OrgId,
            _command: FinalizeWaitingTaskCommand,
        ) -> PortFuture<'a, FinalizedWaitingTask> {
            Box::pin(async { Err(KernelError::internal("not implemented in test port")) })
        }

        fn create_post_finalization_rejection<'a>(
            &'a self,
            _org: OrgId,
            _command: PostFinalizationRejectionCommand,
        ) -> PortFuture<'a, PostFinalizationRejection> {
            Box::pin(async { Err(KernelError::internal("not implemented in test port")) })
        }
    }

    fn conflict_port(status: RunStatus) -> ConflictPort {
        let org = OrgId::knl();
        let run_id = Uuid::new_v4();
        ConflictPort {
            org,
            idempotency_key: "trigger:test-binding:test-object".to_owned(),
            existing: RunRecord {
                id: run_id,
                org_id: org,
                status,
                definition_id: Uuid::new_v4(),
                definition_version: 1,
                object_type: Some("work_order".to_owned()),
                object_id: Some(Uuid::new_v4()),
            },
            transitions: Mutex::new(Vec::new()),
            commits: Mutex::new(Vec::new()),
        }
    }

    fn request(port: &ConflictPort) -> StartRunRequest {
        StartRunRequest {
            run_id: Uuid::new_v4(),
            org_id: port.org,
            definition_id: port.existing.definition_id,
            definition_version: port.existing.definition_version,
            trigger_type: TriggerType::ObjectEvent,
            object_type: port.existing.object_type.clone(),
            object_id: port.existing.object_id,
            idempotency_key: port.idempotency_key.clone(),
            correlation_id: "trigger:test-object".to_owned(),
            trace_id: None,
            input_payload: json!({}),
            context_payload: json!({}),
            initiated_by: None,
            schedule_id: None,
        }
    }

    fn definition() -> serde_json::Value {
        json!({
            "nodes": [
                { "node_key": "gate", "node_type": "object_gate" }
            ],
            "edges": []
        })
    }

    fn audit_context() -> AuditContext {
        AuditContext {
            actor: Some(UserId::new()),
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        }
    }

    fn block_on_ready<T>(future: impl Future<Output = T>) -> T {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(future);
        match Pin::new(&mut future).poll(&mut context) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("test future unexpectedly pending"),
        }
    }

    #[test]
    fn conflicted_waiting_run_is_already_started_without_redrive() {
        let port = conflict_port(RunStatus::Waiting);

        let result = block_on_ready(start_bound_run(
            &port,
            request(&port),
            &definition(),
            &audit_context(),
        ))
        .unwrap();

        assert_eq!(result, TriggeredStart::AlreadyStarted);
        assert!(port.transitions.lock().unwrap().is_empty());
        assert!(port.commits.lock().unwrap().is_empty());
    }

    #[test]
    fn conflicted_starting_run_transitions_and_resumes_drive() {
        let port = conflict_port(RunStatus::Starting);

        let result = block_on_ready(start_bound_run(
            &port,
            request(&port),
            &definition(),
            &audit_context(),
        ))
        .unwrap();

        assert_eq!(result, TriggeredStart::AlreadyStarted);
        let transitions = port.transitions.lock().unwrap();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].from, RunStatus::Starting);
        assert_eq!(transitions[0].to, RunStatus::Running);
        let commits = port.commits.lock().unwrap();
        let run_transition = commits[0]
            .run_transition
            .as_ref()
            .expect("resumed drive closes the one-node graph");
        assert_eq!(run_transition.from, RunStatus::Running);
        assert_eq!(run_transition.to, RunStatus::Succeeded);
    }

    #[test]
    fn conflicted_run_rejects_definition_drift() {
        let port = conflict_port(RunStatus::Running);
        let mut request = request(&port);
        request.definition_version += 1;

        let err = block_on_ready(start_bound_run(
            &port,
            request,
            &definition(),
            &audit_context(),
        ))
        .expect_err("existing run must not be resumed with a different definition version");

        assert_eq!(err.kind, ErrorKind::Conflict);
        assert!(port.transitions.lock().unwrap().is_empty());
        assert!(port.commits.lock().unwrap().is_empty());
    }
}
