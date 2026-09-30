//! FSM-driven advance logic.
//!
//! Walks a run/node through the domain transition tables ([`validate_run_transition`],
//! [`validate_node_transition`]) and commits each step through the
//! [`WorkflowRuntimePort`]. Sync-in-request advance up to the first WAITING/terminal
//! node; the port arms `app.current_org` and writes audit rows in the same
//! transaction as each mutation.

use console_kernel_core::{
    AuditAction, AuditEvent, KernelError, OrgId, Timestamp, TraceContext, UserId,
};
use console_workflow_domain::{
    NewNodeRun, NewRun, NodeStatus, NodeStepCommit, RunStatus, RunTransition, TriggerType,
    WorkflowRuntimePort, validate_node_transition, validate_run_transition,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::idempotency::node_attempt_key;
use crate::interpreter::{NodeOutcome, NodeSpec, interpret_node};

/// Per-request audit metadata threaded onto every emitted [`AuditEvent`].
#[derive(Debug, Clone)]
pub struct AuditContext {
    /// `None` for a system-initiated advance (background worker).
    pub actor: Option<UserId>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Inputs to start (and immediately activate) a new run. `run_id` is pre-generated
/// so the run/node/outbox idempotency keys can be derived before the row exists.
#[derive(Debug, Clone)]
pub struct StartRunRequest {
    pub run_id: Uuid,
    pub org_id: OrgId,
    pub definition_id: Uuid,
    pub definition_version: i32,
    pub trigger_type: TriggerType,
    pub object_type: Option<String>,
    pub object_id: Option<Uuid>,
    pub idempotency_key: String,
    pub correlation_id: String,
    pub trace_id: Option<String>,
    pub input_payload: Value,
    pub context_payload: Value,
    pub initiated_by: Option<UserId>,
    /// The `workflow_schedules` row that fired this run (schedule-poller starts
    /// only); `None` for manual/API/event-triggered runs.
    pub schedule_id: Option<Uuid>,
}

/// Inputs to process one node atomically.
#[derive(Debug, Clone)]
pub struct ProcessNodeRequest {
    pub org_id: OrgId,
    pub run_id: Uuid,
    /// Pre-generated node id so emission keys can reference it.
    pub node_run_id: Uuid,
    /// The run's current status, used to validate the run transition below.
    pub current_run_status: RunStatus,
    /// Where the run should sit after this node (RUNNING to keep advancing, WAITING
    /// to park, or a terminal status). Equal to `current_run_status` ⇒ no run
    /// transition is written.
    pub run_target: RunStatus,
    pub spec: NodeSpec,
    pub attempt: i32,
    pub input_payload: Value,
    /// Cedar/PBAC observe-and-record shadow audit rows produced by the caller's
    /// guard(s) for this transition (design §D). They are folded into the node
    /// step's own `with_audits` transaction so the shadow decision commits (or
    /// rolls back) atomically with the node it guards. Empty when the node is a
    /// worker-driven system node that is audited but not per-request Cedar-guarded.
    pub guard_audits: Vec<AuditEvent>,
}

/// The statuses the run/node landed after a processed node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeStepOutcome {
    pub node_final_status: NodeStatus,
    pub run_status: RunStatus,
}

/// Start a run: INSERT `workflow_runs` STARTING, then advance STARTING→RUNNING.
/// Both writes are audited by the port. Returns the run id.
pub async fn start_run<P: WorkflowRuntimePort + ?Sized>(
    port: &P,
    request: StartRunRequest,
    audit: &AuditContext,
) -> Result<Uuid, KernelError> {
    // A run is born STARTING and immediately advances to RUNNING; validate the
    // edge up front so an illegal FSM table never reaches the DB.
    validate_run_transition(RunStatus::Starting, RunStatus::Running)?;

    let run_id = request.run_id;
    let org = request.org_id;
    let new_run = NewRun {
        id: run_id,
        org_id: org,
        definition_id: request.definition_id,
        definition_version: request.definition_version,
        trigger_type: request.trigger_type,
        object_type: request.object_type,
        object_id: request.object_id,
        idempotency_key: request.idempotency_key,
        correlation_id: request.correlation_id,
        trace_id: request.trace_id,
        input_payload: request.input_payload,
        context_payload: request.context_payload,
        initiated_by: request.initiated_by,
        schedule_id: request.schedule_id,
    };

    let insert_audit = run_audit_event(
        "workflow_run.start",
        audit,
        run_id,
        org,
        None,
        Some(json!({ "status": RunStatus::Starting.as_db_str() })),
    )?;
    port.insert_run(new_run, insert_audit).await?;

    let transition = RunTransition {
        run_id,
        from: RunStatus::Starting,
        to: RunStatus::Running,
        output_payload: None,
        error_payload: None,
    };
    let transition_audit = run_audit_event(
        "workflow_run.transition",
        audit,
        run_id,
        org,
        Some(json!({ "status": RunStatus::Starting.as_db_str() })),
        Some(json!({ "status": RunStatus::Running.as_db_str() })),
    )?;
    port.transition_run(org, transition, transition_audit)
        .await?;

    Ok(run_id)
}

/// Process one node atomically: interpret it, validate the node walk
/// (PENDING→RUNNING→final) and the run transition against the FSM tables, then
/// commit the whole step (node + emissions + optional waiting task + optional run
/// transition + audit) in a single port transaction.
pub async fn process_node<P: WorkflowRuntimePort + ?Sized>(
    port: &P,
    request: ProcessNodeRequest,
    audit: &AuditContext,
) -> Result<NodeStepOutcome, KernelError> {
    let outcome = interpret_node(
        &request.spec,
        request.run_id,
        request.node_run_id,
        &request.input_payload,
    );
    let node_final_status = outcome.node_status();

    // Node walk: PENDING → RUNNING → final. Reject an illegal landing state.
    validate_node_transition(NodeStatus::Pending, NodeStatus::Running)?;
    validate_node_transition(NodeStatus::Running, node_final_status)?;

    // Run transition (skip when the run stays put).
    let run_transition = if request.run_target == request.current_run_status {
        None
    } else {
        validate_run_transition(request.current_run_status, request.run_target)?;
        Some(RunTransition {
            run_id: request.run_id,
            from: request.current_run_status,
            to: request.run_target,
            output_payload: None,
            error_payload: None,
        })
    };

    let (node_output, node_error, emissions, waiting_task, selected_port, guardrail_audit_actions) =
        match outcome {
            NodeOutcome::Succeeded {
                output,
                emissions,
                selected_port,
                audit_actions,
            } => (
                Some(with_selected_port(output, selected_port.as_deref())),
                None,
                emissions,
                None,
                selected_port,
                audit_actions,
            ),
            NodeOutcome::Waiting {
                task,
                selected_port,
                audit_actions,
            } => (
                None,
                None,
                Vec::new(),
                Some(task),
                selected_port,
                audit_actions,
            ),
            NodeOutcome::Failed {
                error,
                selected_port,
                audit_actions,
            } => (
                None,
                Some(with_selected_port(error, selected_port.as_deref())),
                Vec::new(),
                None,
                selected_port,
                audit_actions,
            ),
        };

    let new_node = NewNodeRun {
        id: request.node_run_id,
        run_id: request.run_id,
        node_key: request.spec.node_key.clone(),
        node_type: request.spec.node_type.clone(),
        attempt: request.attempt,
        idempotency_key: node_attempt_key(request.run_id, &request.spec.node_key, request.attempt),
        input_payload: request.input_payload,
    };

    let node_audit = node_audit_event(
        "workflow_node.commit",
        audit,
        request.node_run_id,
        request.org_id,
        node_final_status,
    )?;

    // The node's own audit row plus guardrail-specific audit rows and any
    // Cedar/PBAC shadow audit rows the caller's guard produced for this transition
    // (design §D) — one atomic `with_audits` txn.
    let mut audit_events =
        Vec::with_capacity(1 + guardrail_audit_actions.len() + request.guard_audits.len());
    audit_events.push(node_audit);
    for action in guardrail_audit_actions {
        audit_events.push(guardrail_audit_event(
            &action,
            audit,
            request.node_run_id,
            request.org_id,
            node_final_status,
            selected_port.as_deref(),
        )?);
    }
    audit_events.extend(request.guard_audits);

    let commit = NodeStepCommit {
        new_node,
        node_final_status,
        node_output,
        node_error,
        emissions,
        waiting_task,
        run_transition,
        audit_events,
    };
    port.commit_node_step(request.org_id, commit).await?;

    Ok(NodeStepOutcome {
        node_final_status,
        run_status: request.run_target,
    })
}

pub(crate) fn run_audit_event(
    action: &str,
    audit: &AuditContext,
    run_id: Uuid,
    org: OrgId,
    before: Option<Value>,
    after: Option<Value>,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        audit.actor,
        AuditAction::new(action)?,
        "workflow_run",
        run_id.to_string(),
        audit.trace.clone(),
        audit.occurred_at,
    )
    .with_org(org)
    .with_snapshots(before, after))
}

fn node_audit_event(
    action: &str,
    audit: &AuditContext,
    node_run_id: Uuid,
    org: OrgId,
    final_status: NodeStatus,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        audit.actor,
        AuditAction::new(action)?,
        "workflow_node_run",
        node_run_id.to_string(),
        audit.trace.clone(),
        audit.occurred_at,
    )
    .with_org(org)
    .with_snapshots(
        Some(json!({ "status": NodeStatus::Pending.as_db_str() })),
        Some(json!({ "status": final_status.as_db_str() })),
    ))
}

fn guardrail_audit_event(
    action: &str,
    audit: &AuditContext,
    node_run_id: Uuid,
    org: OrgId,
    final_status: NodeStatus,
    selected_port: Option<&str>,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        audit.actor,
        AuditAction::new(action)?,
        "workflow_guardrail",
        node_run_id.to_string(),
        audit.trace.clone(),
        audit.occurred_at,
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(json!({
            "node_run_id": node_run_id,
            "status": final_status.as_db_str(),
            "selected_port": selected_port,
        })),
    ))
}

fn with_selected_port(payload: Value, selected_port: Option<&str>) -> Value {
    let Some(selected_port) = selected_port else {
        return payload;
    };
    match payload {
        Value::Object(mut map) => {
            map.entry("selected_port".to_owned())
                .or_insert_with(|| json!(selected_port));
            Value::Object(map)
        }
        other => json!({
            "selected_port": selected_port,
            "payload": other,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::task::{Context, Poll, Waker};

    use console_kernel_core::ErrorKind;
    use console_workflow_domain::{
        FinalizeWaitingTaskCommand, FinalizeWaitingTaskContext, FinalizedWaitingTask, NewRun,
        NodeStepCommit, PortFuture, PostFinalizationRejection, PostFinalizationRejectionCommand,
        RunRecord,
    };

    use super::*;
    use crate::interpreter::NodeKind;

    #[derive(Default)]
    struct RecordingPort {
        commits: Mutex<Vec<NodeStepCommit>>,
    }

    impl WorkflowRuntimePort for RecordingPort {
        fn insert_run<'a>(&'a self, _run: NewRun, _audit: AuditEvent) -> PortFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }

        fn load_run<'a>(&'a self, _org: OrgId, _run_id: Uuid) -> PortFuture<'a, Option<RunRecord>> {
            Box::pin(async { Ok(None) })
        }

        fn load_run_by_idempotency_key<'a>(
            &'a self,
            _org: OrgId,
            _idempotency_key: String,
        ) -> PortFuture<'a, Option<RunRecord>> {
            Box::pin(async { Ok(None) })
        }

        fn transition_run<'a>(
            &'a self,
            _org: OrgId,
            _transition: RunTransition,
            _audit: AuditEvent,
        ) -> PortFuture<'a, ()> {
            Box::pin(async { Ok(()) })
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

    fn audit_context() -> AuditContext {
        AuditContext {
            actor: Some(UserId::new()),
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
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
    fn author_finalize_node_parks_run_waiting_before_terminal_success() {
        let port = RecordingPort::default();
        let org = OrgId::knl();
        let run_id = Uuid::new_v4();
        let node_run_id = Uuid::new_v4();

        let outcome = block_on_ready(process_node(
            &port,
            ProcessNodeRequest {
                org_id: org,
                run_id,
                node_run_id,
                current_run_status: RunStatus::Running,
                run_target: RunStatus::Waiting,
                spec: NodeSpec {
                    node_key: "finalize.author".to_owned(),
                    node_type: "human_task".to_owned(),
                    kind: NodeKind::HumanTask {
                        title: "Author finalize".to_owned(),
                        required_policy: Some("approval_finalize".to_owned()),
                        assignee_role_key: Some("initiator".to_owned()),
                    },
                },
                attempt: 1,
                input_payload: json!({}),
                guard_audits: Vec::new(),
            },
            &audit_context(),
        ))
        .unwrap();

        assert_eq!(outcome.node_final_status, NodeStatus::Waiting);
        assert_eq!(outcome.run_status, RunStatus::Waiting);
        let commits = port.commits.lock().unwrap();
        let commit = commits.first().expect("node commit recorded");
        let transition = commit
            .run_transition
            .as_ref()
            .expect("finalize parks the run");
        assert_eq!(transition.from, RunStatus::Running);
        assert_eq!(transition.to, RunStatus::Waiting);
        let task = commit.waiting_task.as_ref().expect("waiting task recorded");
        assert_eq!(task.waiting_key, "finalize.author");
        assert_eq!(task.required_policy.as_deref(), Some("approval_finalize"));
    }

    #[test]
    fn terminal_run_cannot_be_reopened_for_late_finalization() {
        let port = RecordingPort::default();
        let err = block_on_ready(process_node(
            &port,
            ProcessNodeRequest {
                org_id: OrgId::knl(),
                run_id: Uuid::new_v4(),
                node_run_id: Uuid::new_v4(),
                current_run_status: RunStatus::Succeeded,
                run_target: RunStatus::Waiting,
                spec: NodeSpec {
                    node_key: "finalize.author".to_owned(),
                    node_type: "human_task".to_owned(),
                    kind: NodeKind::HumanTask {
                        title: "Author finalize".to_owned(),
                        required_policy: Some("approval_finalize".to_owned()),
                        assignee_role_key: Some("initiator".to_owned()),
                    },
                },
                attempt: 1,
                input_payload: json!({}),
                guard_audits: Vec::new(),
            },
            &audit_context(),
        ))
        .expect_err("terminal run must not reopen for late finalization");

        assert_eq!(err.kind, ErrorKind::InvalidTransition);
        assert!(port.commits.lock().unwrap().is_empty());
    }
}
