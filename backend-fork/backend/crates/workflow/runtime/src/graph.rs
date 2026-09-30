//! `wf.exec.v1` graph walker + run driver.
//!
//! The strangler completion tail hard-codes its two nodes ([`crate::completion`]);
//! the generalized instance REST (`POST /api/v1/workflow-runs`, task `decide`
//! advance) instead walks the definition's `nodes`/`edges` graph. This module
//! parses a `wf.exec.v1` definition into typed [`NodeSpec`]s and drives a run
//! forward through [`process_node`] until it parks on the first human task or
//! reaches a terminal node — the exact synchronous-until-WAITING semantics the
//! spike's instance surface requires.
//!
//! Linear approval lines (submit → review → approve → finalize → receipt) follow a
//! single outgoing edge per node. BE-AUTO slice 2 adds `condition` nodes: a
//! condition passes through, then its predicate (evaluated against the run
//! context threaded into [`drive_from`]) selects which `when`-labeled outgoing
//! edge the walk follows. The untaken branch simply never executes — no node run
//! rows are written for it (a dead branch leaves no trace, which the FSM's
//! terminal-node = no-successor invariant already tolerates).
//!
//! ponytail: the untaken branch is left unmarked rather than swept as SKIPPED.
//! Upgrade path if audit completeness ever needs the untaken subtree recorded:
//! compute reachable-from-untaken-minus-reachable-from-taken and emit SKIPPED
//! node steps for that set.

use std::collections::HashSet;

use console_kernel_core::{AuditEvent, KernelError, OrgId};
use console_workflow_domain::{RunStatus, WorkflowRuntimePort};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::engine::{AuditContext, ProcessNodeRequest, process_node};
use crate::interpreter::{NodeKind, NodeSpec};

/// A directed edge. `when` labels a condition node's branch (`"true"`/`"false"`);
/// `None` for the single successor of an ordinary (non-branching) node.
#[derive(Debug, Clone)]
struct Edge {
    from: String,
    to: String,
    when: Option<String>,
}

/// A parsed `wf.exec.v1` node graph: the typed node specs plus their directed
/// edges. Built once per run drive from the published definition JSON.
#[derive(Debug, Clone)]
pub struct ExecGraph {
    nodes: Vec<NodeSpec>,
    edges: Vec<Edge>,
}

impl ExecGraph {
    /// Parse a `wf.exec.v1` definition value. Fails (validation) when the graph has
    /// no `nodes` array — e.g. the strangler completion template, which carries no
    /// node graph and is driven by its own hard-coded tail instead.
    pub fn parse(definition: &Value) -> Result<Self, KernelError> {
        let raw_nodes = definition
            .get("nodes")
            .and_then(Value::as_array)
            .filter(|nodes| !nodes.is_empty())
            .ok_or_else(|| {
                KernelError::validation("workflow definition has no executable node graph")
            })?;

        let mut nodes = Vec::with_capacity(raw_nodes.len());
        for node in raw_nodes {
            nodes.push(parse_node(node)?);
        }

        let edges = definition
            .get("edges")
            .and_then(Value::as_array)
            .map(|edges| {
                edges
                    .iter()
                    .map(|edge| {
                        let from = edge.get("from").and_then(Value::as_str).ok_or_else(|| {
                            KernelError::validation("workflow edge missing string 'from'")
                        })?;
                        let to = edge.get("to").and_then(Value::as_str).ok_or_else(|| {
                            KernelError::validation("workflow edge missing string 'to'")
                        })?;
                        let when = match edge.get("when") {
                            Some(Value::String(value))
                                if matches!(value.as_str(), "true" | "false") =>
                            {
                                Some(value.clone())
                            }
                            Some(_) => {
                                return Err(KernelError::validation(
                                    "workflow edge 'when' must be the string \"true\" or \"false\"",
                                ));
                            }
                            None => None,
                        };
                        Ok(Edge {
                            from: from.to_owned(),
                            to: to.to_owned(),
                            when,
                        })
                    })
                    .collect::<Result<Vec<_>, KernelError>>()
            })
            .transpose()?
            .unwrap_or_default();

        Ok(Self { nodes, edges })
    }

    /// The graph's entry node key: the single node that is no edge's target. Fails
    /// closed when zero or more than one candidate exists (an ill-formed graph).
    pub fn entry_node_key(&self) -> Result<&str, KernelError> {
        let mut entries = self
            .nodes
            .iter()
            .map(|node| node.node_key.as_str())
            .filter(|key| !self.edges.iter().any(|edge| edge.to == *key));
        let first = entries
            .next()
            .ok_or_else(|| KernelError::validation("workflow graph has no entry node"))?;
        if entries.next().is_some() {
            return Err(KernelError::validation(
                "workflow graph has more than one entry node",
            ));
        }
        Ok(first)
    }

    /// The node spec for `key`, if present.
    #[must_use]
    pub fn node_spec(&self, key: &str) -> Option<&NodeSpec> {
        self.nodes.iter().find(|node| node.node_key == key)
    }

    /// The single node following `from` along its outgoing edge, if any. Used for
    /// ordinary (non-branching) nodes, whose one outgoing edge carries no `when`.
    #[must_use]
    pub fn next_node_key(&self, from: &str) -> Option<&str> {
        self.edges
            .iter()
            .find(|edge| edge.from == from)
            .map(|edge| edge.to.as_str())
    }

    /// The branch target following a condition node `from` for the evaluated
    /// `outcome`: the outgoing edge whose `when` is `"true"`/`"false"` to match.
    /// `None` when the graph declares no edge for that outcome (a mis-authored
    /// condition — the caller fails closed).
    #[must_use]
    pub fn next_branch(&self, from: &str, outcome: bool) -> Option<&str> {
        let want = if outcome { "true" } else { "false" };
        self.edges
            .iter()
            .find(|edge| edge.from == from && edge.when.as_deref() == Some(want))
            .map(|edge| edge.to.as_str())
    }

    /// The node key the walk advances to after `key`, given a run `context` used
    /// to evaluate a condition node's predicate. Fails closed for a condition
    /// node whose selected branch has no edge.
    fn successor(&self, key: &str, context: &Value) -> Result<Option<String>, KernelError> {
        match self.node_spec(key).map(|spec| &spec.kind) {
            Some(NodeKind::Condition { predicate }) => {
                let outcome = predicate.eval(context);
                match self.next_branch(key, outcome) {
                    Some(next) => Ok(Some(next.to_owned())),
                    None => Err(KernelError::validation(format!(
                        "condition node {key:?} has no {} branch edge",
                        if outcome { "true" } else { "false" }
                    ))),
                }
            }
            _ => Ok(self.next_node_key(key).map(ToOwned::to_owned)),
        }
    }
}

/// Purely walk the graph from the entry node, following condition branches by
/// evaluating their predicates against `context`, and return the ordered node
/// keys that WOULD execute — stopping at the first human task (the run would
/// park there) or a terminal node. No I/O: this is what the simulate endpoint
/// uses to exercise branches without starting a run.
pub fn simulate_path(graph: &ExecGraph, context: &Value) -> Result<Vec<String>, KernelError> {
    let mut key = graph.entry_node_key()?.to_owned();
    let mut path = Vec::new();
    loop {
        let spec = graph
            .node_spec(&key)
            .ok_or_else(|| KernelError::validation(format!("graph has no node {key:?}")))?;
        path.push(key.clone());
        if matches!(spec.kind, NodeKind::HumanTask { .. }) {
            return Ok(path);
        }
        match graph.successor(&key, context)? {
            Some(next) => {
                // A cycle would loop forever; the graph is a DAG in practice, but
                // guard so a mis-authored back-edge can't hang the simulator.
                if path.iter().any(|seen| seen == &next) {
                    return Err(KernelError::validation(
                        "workflow graph has a cycle; simulate cannot resolve a linear path",
                    ));
                }
                key = next;
            }
            None => return Ok(path),
        }
    }
}

fn parse_node(node: &Value) -> Result<NodeSpec, KernelError> {
    let node_key = node
        .get("node_key")
        .and_then(Value::as_str)
        .ok_or_else(|| KernelError::validation("workflow node missing node_key"))?
        .to_owned();
    let node_type = node
        .get("node_type")
        .and_then(Value::as_str)
        .ok_or_else(|| KernelError::validation("workflow node missing node_type"))?
        .to_owned();

    let kind = match node_type.as_str() {
        "object_gate" => NodeKind::ObjectGate,
        "object_mutation" => NodeKind::ObjectMutation,
        "human_task" | "waiting_task" => NodeKind::HumanTask {
            title: node
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(node_key.as_str())
                .to_owned(),
            required_policy: node
                .get("required_policy")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            assignee_role_key: node
                .get("assignee_role_key")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        },
        "job" => NodeKind::Job {
            connector: node
                .get("connector")
                .and_then(Value::as_str)
                .ok_or_else(|| KernelError::validation("workflow job node missing connector"))?
                .to_owned(),
            job: node
                .get("job")
                .and_then(Value::as_str)
                .ok_or_else(|| KernelError::validation("workflow job node missing job"))?
                .to_owned(),
            emits_status: node
                .get("emits_status")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        },
        "condition" => NodeKind::Condition {
            predicate: crate::predicate::Predicate::parse(node.get("predicate").ok_or_else(
                || KernelError::validation("workflow condition node missing predicate"),
            )?)?,
        },
        other => {
            return Err(KernelError::validation(format!(
                "unsupported workflow node_type {other:?}"
            )));
        }
    };

    Ok(NodeSpec {
        node_key,
        node_type,
        kind,
    })
}

/// Where a [`drive_from`] walk stopped.
#[derive(Debug, Clone)]
pub struct DriveOutcome {
    /// The run status after the walk (WAITING when parked on a human task,
    /// SUCCEEDED when it reached a terminal node).
    pub run_status: RunStatus,
    /// The node/waiting key the run parked on, when it parked on a human task.
    pub parked_task_key: Option<String>,
}

/// Drive a RUNNING/WAITING run forward from `start_key`: process each node through
/// the FSM engine, following the single outgoing edge, until it parks on a human
/// task (run → WAITING) or reaches a node with no successor (run → SUCCEEDED).
///
/// `first_node_guard_audits` carries the per-request Cedar/PBAC shadow the caller
/// produced for the *entry* transition (design §D); it rides only the first node's
/// atomic `with_audits` commit. Pass-through gate/mutation/job nodes advance the run
/// without an extra guard here (jobs are worker-driven system nodes).
#[allow(clippy::too_many_arguments)]
pub async fn drive_from<P: WorkflowRuntimePort + ?Sized>(
    port: &P,
    org: OrgId,
    run_id: Uuid,
    mut current_status: RunStatus,
    graph: &ExecGraph,
    start_key: &str,
    first_node_guard_audits: Vec<AuditEvent>,
    context: &Value,
    audit: &AuditContext,
) -> Result<DriveOutcome, KernelError> {
    let mut node_key = start_key.to_owned();
    let mut guard_audits = first_node_guard_audits;
    let mut visited = HashSet::new();

    loop {
        if !visited.insert(node_key.clone()) {
            return Err(KernelError::validation(
                "workflow graph has a cycle; drive cannot resolve a linear path",
            ));
        }

        let spec = graph
            .node_spec(&node_key)
            .ok_or_else(|| {
                KernelError::validation(format!("workflow graph has no node {node_key:?}"))
            })?
            .clone();
        let is_human = matches!(spec.kind, NodeKind::HumanTask { .. });
        // Condition nodes select their successor by predicate over the run
        // context; ordinary nodes follow their single outgoing edge. A condition
        // whose chosen branch has no edge fails closed here.
        let next = graph.successor(&node_key, context)?;

        let run_target = if is_human {
            RunStatus::Waiting
        } else if next.is_some() {
            RunStatus::Running
        } else {
            RunStatus::Succeeded
        };

        process_node(
            port,
            ProcessNodeRequest {
                org_id: org,
                run_id,
                node_run_id: Uuid::new_v4(),
                current_run_status: current_status,
                run_target,
                spec,
                attempt: 1,
                input_payload: json!({}),
                guard_audits: std::mem::take(&mut guard_audits),
            },
            audit,
        )
        .await?;
        current_status = run_target;

        if is_human {
            return Ok(DriveOutcome {
                run_status: RunStatus::Waiting,
                parked_task_key: Some(node_key),
            });
        }

        match next {
            Some(next_key) => node_key = next_key,
            None => {
                return Ok(DriveOutcome {
                    run_status: RunStatus::Succeeded,
                    parked_task_key: None,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approval_graph() -> Value {
        json!({
            "schema_version": "wf.exec.v1",
            "nodes": [
                {"node_key": "submit", "node_type": "object_gate"},
                {"node_key": "review.hr", "node_type": "human_task",
                 "assignee_role_key": "hr_reviewer", "required_policy": "approval_review"},
                {"node_key": "approve.manager", "node_type": "human_task",
                 "assignee_role_key": "manager_approver", "required_policy": "approval_decide"}
            ],
            "edges": [
                {"from": "submit", "to": "review.hr"},
                {"from": "review.hr", "to": "approve.manager"}
            ]
        })
    }

    #[test]
    fn entry_node_is_the_untargeted_node() {
        let graph = ExecGraph::parse(&approval_graph()).unwrap();
        assert_eq!(graph.entry_node_key().unwrap(), "submit");
    }

    #[test]
    fn next_node_follows_the_single_edge() {
        let graph = ExecGraph::parse(&approval_graph()).unwrap();
        assert_eq!(graph.next_node_key("submit"), Some("review.hr"));
        assert_eq!(graph.next_node_key("review.hr"), Some("approve.manager"));
        assert_eq!(graph.next_node_key("approve.manager"), None);
    }

    #[test]
    fn human_node_parses_role_and_policy() {
        let graph = ExecGraph::parse(&approval_graph()).unwrap();
        let spec = graph.node_spec("review.hr").unwrap();
        match &spec.kind {
            NodeKind::HumanTask {
                assignee_role_key,
                required_policy,
                ..
            } => {
                assert_eq!(assignee_role_key.as_deref(), Some("hr_reviewer"));
                assert_eq!(required_policy.as_deref(), Some("approval_review"));
            }
            other => panic!("expected human task, got {other:?}"),
        }
    }

    #[test]
    fn no_node_graph_fails_closed() {
        let completion =
            json!({"schema_version": "wf.exec.v1", "template": "work_order_completion"});
        assert!(ExecGraph::parse(&completion).is_err());
    }

    fn branching_graph() -> Value {
        // gate → decide(condition on amount) → [true] escalate.exec / [false] auto.approve
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

    #[test]
    fn condition_node_parses_with_predicate() {
        let graph = ExecGraph::parse(&branching_graph()).unwrap();
        match &graph.node_spec("decide").unwrap().kind {
            NodeKind::Condition { .. } => {}
            other => panic!("expected condition, got {other:?}"),
        }
        assert_eq!(graph.next_branch("decide", true), Some("escalate.exec"));
        assert_eq!(graph.next_branch("decide", false), Some("auto.approve"));
    }

    #[test]
    fn condition_edge_with_invalid_when_label_fails_at_parse() {
        let mut def = branching_graph();
        def["edges"][1]["when"] = json!("True");

        assert!(ExecGraph::parse(&def).is_err());
    }

    #[test]
    fn edge_missing_endpoint_fails_at_parse() {
        let mut def = approval_graph();
        def["edges"][0] = json!({"from": "submit"});

        assert!(ExecGraph::parse(&def).is_err());
    }

    #[test]
    fn simulate_takes_true_branch_and_dead_branch_is_absent() {
        let graph = ExecGraph::parse(&branching_graph()).unwrap();
        // amount > 1000 ⇒ true branch, parks at the human task.
        let path = simulate_path(&graph, &json!({ "amount": 5000 })).unwrap();
        assert_eq!(path, vec!["gate", "decide", "escalate.exec"]);
        assert!(
            !path.iter().any(|n| n == "auto.approve"),
            "the dead (false) branch must never appear in the taken path"
        );
    }

    #[test]
    fn simulate_takes_false_branch_to_terminal() {
        let graph = ExecGraph::parse(&branching_graph()).unwrap();
        // amount <= 1000 (and missing ⇒ false) ⇒ false branch, runs to terminal.
        let path = simulate_path(&graph, &json!({ "amount": 100 })).unwrap();
        assert_eq!(path, vec!["gate", "decide", "auto.approve"]);
        let empty = simulate_path(&graph, &json!({})).unwrap();
        assert_eq!(empty, vec!["gate", "decide", "auto.approve"]);
    }

    #[test]
    fn condition_missing_a_branch_edge_fails_closed_at_walk() {
        let mut def = branching_graph();
        // Drop the false branch edge.
        def["edges"] = json!([
            {"from": "gate", "to": "decide"},
            {"from": "decide", "to": "escalate.exec", "when": "true"}
        ]);
        let graph = ExecGraph::parse(&def).unwrap();
        assert!(simulate_path(&graph, &json!({ "amount": 1 })).is_err());
    }
}
