//! Pure per-node interpreter.
//!
//! Given a typed [`NodeSpec`] and the node's input payload, decide the node's
//! outcome — succeed (optionally emitting transactional-outbox side effects),
//! park on a waiting task, or fail. No I/O: the [`engine`](crate::engine) turns the
//! outcome into an atomic [`console_workflow_domain::NodeStepCommit`].
//!
//! The typed model mirrors the completion→approval→payroll template (design
//! §template): `object_gate`/`object_mutation` pass through, `human_task` parks on
//! a waiting task, and `job` enqueues one JOB outbox event to a connector (e.g.
//! `internal.jobs` for `payroll_draft`). Parsing the definition JSONB into these
//! specs is the template step; this interpreter operates on the already-typed spec.

use std::str::FromStr;

use console_kernel_core::{AuditAction, KernelError};
use console_platform_authz::Feature;
use console_workflow_domain::{NewWaitingTask, NodeStatus, OutboxChannel, OutboxEmission};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::idempotency::outbox_job_key;
use crate::predicate::Predicate;

pub const CHECKLIST_ATTESTATION_NODE_TYPE: &str = "guard.checklist_attestation";
pub const FOUR_EYES_PEER_REVIEW_NODE_TYPE: &str = "guard.four_eyes_peer_review";
pub const SEGREGATION_OF_DUTIES_NODE_TYPE: &str = "guard.segregation_of_duties";
pub const EGRESS_POLICY_NODE_TYPE: &str = "guard.egress_policy";

const CHECKLIST_OPENED_AUDIT: &str = "workflow_guardrail.checklist.opened";
const FOUR_EYES_OPENED_AUDIT: &str = "workflow_guardrail.four_eyes.opened";
const SOD_ALLOWED_AUDIT: &str = "workflow_guardrail.sod.allowed";
const SOD_DENIED_AUDIT: &str = "workflow_guardrail.sod.denied";
const SOD_EXEMPTION_AUDIT: &str = "workflow_guardrail.sod.exemption_recorded";
const EGRESS_ALLOWED_AUDIT: &str = "workflow_guardrail.egress.allowed";
const EGRESS_DENIED_AUDIT: &str = "workflow_guardrail.egress.denied";
const EGRESS_REVIEW_AUDIT: &str = "workflow_guardrail.egress.review_required";

/// The behavioral classification of a workflow node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// An object gate / event capture: passes through immediately (SUCCEEDED).
    ObjectGate,
    /// A domain object mutation step: records node success (the mutation itself is
    /// performed by the caller's own transaction — e.g. the strangler).
    ObjectMutation,
    /// A human/approval step: parks the run on a waiting task.
    HumanTask {
        title: String,
        required_policy: Option<String>,
        assignee_role_key: Option<String>,
    },
    /// Checklist attestation guardrail: parks the run for evidence/attestation.
    ChecklistAttestation(ChecklistAttestationSpec),
    /// Four-eyes guardrail: parks for an authorized peer review decision.
    FourEyesPeerReview(FourEyesPeerReviewSpec),
    /// Synchronous segregation-of-duties guardrail.
    SegregationOfDuties(SegregationOfDutiesSpec),
    /// Synchronous/manual-review outbound egress guardrail.
    EgressPolicy(EgressPolicySpec),
    /// A job step: succeeds and enqueues one JOB outbox event to `connector`.
    Job {
        connector: String,
        job: String,
        emits_status: Option<String>,
    },
    /// A branch/condition step: passes through immediately (SUCCEEDED, no side
    /// effects), but its `predicate` selects which outgoing (`when`) edge the
    /// [`crate::graph`] walk follows next. Evaluated against the run context.
    Condition { predicate: Predicate },
}

/// Business meaning for approval human tasks that affect closeout semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HumanTaskSemantic {
    /// Ordinary approval-line decision: approve/reject/return.
    Decision,
    /// Author or delegated closeout step. This is still pre-terminal WAITING.
    Finalization,
    /// Legal receipt confirmation step. This is still pre-terminal WAITING.
    ReceiptConfirmation,
}

impl NodeKind {
    /// Parse a guardrail node's typed config from executable definition JSON.
    /// Unknown fields and unknown Feature policy keys fail closed.
    pub fn from_guardrail_config(node_type: &str, config: Value) -> Result<Self, KernelError> {
        match node_type {
            CHECKLIST_ATTESTATION_NODE_TYPE => {
                let spec: ChecklistAttestationSpec = parse_guardrail_config(node_type, config)?;
                spec.validate()?;
                Ok(Self::ChecklistAttestation(spec))
            }
            FOUR_EYES_PEER_REVIEW_NODE_TYPE => {
                let spec: FourEyesPeerReviewSpec = parse_guardrail_config(node_type, config)?;
                spec.validate()?;
                Ok(Self::FourEyesPeerReview(spec))
            }
            SEGREGATION_OF_DUTIES_NODE_TYPE => {
                let spec: SegregationOfDutiesSpec = parse_guardrail_config(node_type, config)?;
                spec.validate()?;
                Ok(Self::SegregationOfDuties(spec))
            }
            EGRESS_POLICY_NODE_TYPE => {
                let spec: EgressPolicySpec = parse_guardrail_config(node_type, config)?;
                spec.validate()?;
                Ok(Self::EgressPolicy(spec))
            }
            other => Err(KernelError::validation(format!(
                "unsupported guardrail node_type {other}"
            ))),
        }
    }
}

/// Checklist item authored for `guard.checklist_attestation`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChecklistItemSpec {
    pub key: String,
    pub label: String,
    pub kind: ChecklistItemKind,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)]
    pub min_count: Option<u16>,
    #[serde(default)]
    pub source_ref: Option<String>,
}

/// Allowed checklist value classes. No arbitrary browser-defined widgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecklistItemKind {
    Checkbox,
    Text,
    EvidenceRef,
    ObjectRef,
    PolicyAck,
}

/// Config for `guard.checklist_attestation`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChecklistAttestationSpec {
    pub label: String,
    #[serde(default)]
    pub required_policy: Option<String>,
    #[serde(default)]
    pub audit_event_key: Option<String>,
    #[serde(default)]
    pub assignee_role_key: Option<String>,
    pub items: Vec<ChecklistItemSpec>,
    #[serde(default = "default_true")]
    pub approve_requires_all_required: bool,
    #[serde(default = "default_true")]
    pub reject_requires_memo: bool,
    #[serde(default)]
    pub step_up_required: bool,
    #[serde(default)]
    pub passkey_purpose: Option<String>,
    #[serde(default)]
    pub due_after: Option<String>,
    #[serde(default)]
    pub redaction: Option<String>,
    #[serde(default)]
    pub on_missing_fact: Option<OnMissingFact>,
}

/// Config for `guard.four_eyes_peer_review`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FourEyesPeerReviewSpec {
    pub label: String,
    #[serde(default)]
    pub required_policy: Option<String>,
    #[serde(default)]
    pub audit_event_key: Option<String>,
    #[serde(default)]
    pub assignee_role_key: Option<String>,
    pub subject_actor_refs: Vec<String>,
    #[serde(default = "default_one")]
    pub min_reviewers: u8,
    #[serde(default = "default_true")]
    pub forbid_same_actor: bool,
    #[serde(default)]
    pub allow_org_lead_exemption: bool,
    #[serde(default)]
    pub allow_super_admin_exemption: bool,
    #[serde(default = "default_true")]
    pub exemption_requires_memo: bool,
    #[serde(default = "default_true")]
    pub step_up_required: bool,
    #[serde(default = "default_true")]
    pub reject_requires_memo: bool,
    #[serde(default)]
    pub passkey_purpose: Option<String>,
    #[serde(default)]
    pub due_after: Option<String>,
    #[serde(default)]
    pub redaction: Option<String>,
    #[serde(default)]
    pub on_missing_fact: Option<OnMissingFact>,
}

/// Config for `guard.segregation_of_duties`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SegregationOfDutiesSpec {
    pub label: String,
    pub policy_key: String,
    #[serde(default)]
    pub audit_event_key: Option<String>,
    #[serde(default = "default_actor_under_test_ref")]
    pub actor_under_test_ref: String,
    pub blocked_actor_refs: Vec<String>,
    #[serde(default)]
    pub blocked_role_refs: Vec<String>,
    #[serde(default = "default_sod_scope")]
    pub scope: String,
    #[serde(default)]
    pub mode: SodMode,
    #[serde(default)]
    pub exemptions: Vec<String>,
    #[serde(default = "default_true")]
    pub exemption_requires_memo: bool,
    #[serde(default)]
    pub on_missing_fact: Option<OnMissingFact>,
}

/// SoD handling mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SodMode {
    #[default]
    HardBlock,
    AllowWithGovernanceFinding,
}

/// Config for `guard.egress_policy`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EgressPolicySpec {
    pub label: String,
    pub egress_kind: EgressKind,
    pub channel: String,
    #[serde(default)]
    pub required_policy: Option<String>,
    #[serde(default)]
    pub audit_event_key: Option<String>,
    #[serde(default)]
    pub manual_review_role_key: Option<String>,
    #[serde(default = "default_egress_policy")]
    pub external_recipient_policy: EgressExternalRecipientPolicy,
    #[serde(default = "default_true")]
    pub step_up_required: bool,
    #[serde(default)]
    pub passkey_purpose: Option<String>,
    #[serde(default)]
    pub classification_ref: Option<String>,
    #[serde(default)]
    pub data_classes: Vec<String>,
    #[serde(default)]
    pub lifecycle_requirements: Vec<String>,
    #[serde(default)]
    pub on_missing_fact: Option<OnMissingFact>,
}

/// Outbound side-effect families guarded before delivery/enqueue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressKind {
    Mail,
    Export,
    Webhook,
    Job,
    DocumentDownload,
}

/// External-recipient handling policy for egress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressExternalRecipientPolicy {
    Block,
    AllowIfApproved,
    ManualReview,
}

/// Missing server fact behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnMissingFact {
    HardBlock,
    RouteBlocked,
}

impl ChecklistAttestationSpec {
    fn validate(&self) -> Result<(), KernelError> {
        validate_guard_label(&self.label)?;
        validate_optional_feature(self.required_policy.as_deref())?;
        validate_optional_audit_action(self.audit_event_key.as_deref())?;
        if self.items.is_empty() || self.items.len() > 50 {
            return Err(KernelError::validation(
                "checklist guardrail requires 1..=50 items",
            ));
        }
        for item in &self.items {
            validate_guard_token("checklist item key", &item.key)?;
            validate_guard_label(&item.label)?;
            if item.min_count == Some(0) {
                return Err(KernelError::validation(
                    "checklist item min_count must be greater than zero",
                ));
            }
        }
        Ok(())
    }
}

impl FourEyesPeerReviewSpec {
    fn validate(&self) -> Result<(), KernelError> {
        validate_guard_label(&self.label)?;
        validate_optional_feature(self.required_policy.as_deref())?;
        validate_optional_audit_action(self.audit_event_key.as_deref())?;
        if self.subject_actor_refs.is_empty() {
            return Err(KernelError::validation(
                "four-eyes guardrail requires subject_actor_refs",
            ));
        }
        validate_ref_list("subject_actor_refs", &self.subject_actor_refs)?;
        if !(1..=4).contains(&self.min_reviewers) {
            return Err(KernelError::validation(
                "four-eyes min_reviewers must be between 1 and 4",
            ));
        }
        Ok(())
    }
}

impl SegregationOfDutiesSpec {
    fn validate(&self) -> Result<(), KernelError> {
        validate_guard_label(&self.label)?;
        validate_guard_token("policy_key", &self.policy_key)?;
        validate_optional_audit_action(self.audit_event_key.as_deref())?;
        validate_guard_token("actor_under_test_ref", &self.actor_under_test_ref)?;
        if self.blocked_actor_refs.is_empty() {
            return Err(KernelError::validation(
                "SoD guardrail requires blocked_actor_refs",
            ));
        }
        validate_ref_list("blocked_actor_refs", &self.blocked_actor_refs)?;
        validate_ref_list("blocked_role_refs", &self.blocked_role_refs)?;
        Ok(())
    }
}

impl EgressPolicySpec {
    fn validate(&self) -> Result<(), KernelError> {
        validate_guard_label(&self.label)?;
        validate_guard_token("channel", &self.channel)?;
        validate_optional_feature(self.required_policy.as_deref())?;
        validate_optional_audit_action(self.audit_event_key.as_deref())?;
        if matches!(
            self.egress_kind,
            EgressKind::Mail | EgressKind::Export | EgressKind::DocumentDownload
        ) && self.classification_ref.as_deref().is_none_or(str::is_empty)
        {
            return Err(KernelError::validation(
                "mail/export/document_download egress guards require classification_ref",
            ));
        }
        Ok(())
    }
}

/// A typed workflow node: its stable `node_key`, its DB `node_type` spelling, and
/// its behavioral [`NodeKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeSpec {
    pub node_key: String,
    pub node_type: String,
    pub kind: NodeKind,
}

impl NodeSpec {
    /// Parse a `wf.exec.v1` node object into the runtime's typed node spec.
    /// This is fail-closed: unknown node types, unknown guard config fields, and
    /// unknown Feature policy keys return validation errors before execution.
    pub fn from_execution_node(node: &Value) -> Result<Self, KernelError> {
        let object = node
            .as_object()
            .ok_or_else(|| KernelError::validation("execution nodes must be JSON objects"))?;
        let node_key = required_string(object, "node_key")?.to_owned();
        let node_type = required_string(object, "node_type")?.to_owned();
        let kind = match node_type.as_str() {
            "object_gate" => NodeKind::ObjectGate,
            "object_mutation" => NodeKind::ObjectMutation,
            "human_task" => NodeKind::HumanTask {
                title: optional_string(object, "title")
                    .unwrap_or("Workflow task")
                    .to_owned(),
                required_policy: optional_string(object, "required_policy").map(ToOwned::to_owned),
                assignee_role_key: Some(required_string(object, "assignee_role_key")?.to_owned()),
            },
            "job" => NodeKind::Job {
                connector: required_string(object, "connector_key")?.to_owned(),
                job: optional_string(object, "job")
                    .or_else(|| optional_string(object, "action_key"))
                    .ok_or_else(|| KernelError::validation("execution node job is required"))?
                    .to_owned(),
                emits_status: optional_string(object, "emits_status").map(ToOwned::to_owned),
            },
            CHECKLIST_ATTESTATION_NODE_TYPE
            | FOUR_EYES_PEER_REVIEW_NODE_TYPE
            | SEGREGATION_OF_DUTIES_NODE_TYPE
            | EGRESS_POLICY_NODE_TYPE => NodeKind::from_guardrail_config(
                &node_type,
                guardrail_config_from_execution_node(object)?,
            )?,
            other => {
                return Err(KernelError::validation(format!(
                    "unsupported execution node_type {other}"
                )));
            }
        };
        Ok(Self {
            node_key,
            node_type,
            kind,
        })
    }

    /// Classify approval closeout nodes without changing the FSM. Finalization and
    /// receipt are human waiting tasks; only the later completion step may advance
    /// the run to SUCCEEDED.
    #[must_use]
    pub fn human_task_semantic(&self) -> Option<HumanTaskSemantic> {
        let NodeKind::HumanTask {
            required_policy, ..
        } = &self.kind
        else {
            return None;
        };

        match (self.node_key.as_str(), required_policy.as_deref()) {
            ("finalize.author", _) | (_, Some("approval_finalize")) => {
                Some(HumanTaskSemantic::Finalization)
            }
            ("receipt.target", _) | (_, Some("approval_receipt")) => {
                Some(HumanTaskSemantic::ReceiptConfirmation)
            }
            _ => Some(HumanTaskSemantic::Decision),
        }
    }
}

/// The interpreter's decision for one node.
#[derive(Debug, Clone)]
pub enum NodeOutcome {
    /// Node completed; may carry transactional-outbox emissions.
    Succeeded {
        output: Value,
        emissions: Vec<OutboxEmission>,
        selected_port: Option<String>,
        audit_actions: Vec<String>,
    },
    /// Node parks on a human/gate waiting task.
    Waiting {
        task: NewWaitingTask,
        selected_port: Option<String>,
        audit_actions: Vec<String>,
    },
    /// Node failed with a structured error payload.
    Failed {
        error: Value,
        selected_port: Option<String>,
        audit_actions: Vec<String>,
    },
}

impl NodeOutcome {
    /// The terminal/parked node status this outcome lands.
    #[must_use]
    pub const fn node_status(&self) -> NodeStatus {
        match self {
            Self::Succeeded { .. } => NodeStatus::Succeeded,
            Self::Waiting { .. } => NodeStatus::Waiting,
            Self::Failed { .. } => NodeStatus::Failed,
        }
    }
}

/// Interpret one node. `run_id`/`node_run_id` are pre-generated by the engine so
/// emission idempotency keys can be derived before the rows are inserted.
#[must_use]
pub fn interpret_node(
    spec: &NodeSpec,
    run_id: Uuid,
    node_run_id: Uuid,
    input: &Value,
) -> NodeOutcome {
    match &spec.kind {
        NodeKind::ObjectGate | NodeKind::ObjectMutation => NodeOutcome::Succeeded {
            output: json!({ "node_key": spec.node_key.as_str(), "selected_port": "done" }),
            emissions: Vec::new(),
            selected_port: Some("done".to_owned()),
            audit_actions: Vec::new(),
        },
        // A condition node is a pass-through: it never mutates or parks. The
        // branch decision (which outgoing edge to follow) is made by the graph
        // walk from the predicate + run context, not here.
        NodeKind::Condition { .. } => NodeOutcome::Succeeded {
            output: json!({ "node_key": spec.node_key.as_str(), "kind": "condition" }),
            emissions: Vec::new(),
            selected_port: None,
            audit_actions: Vec::new(),
        },
        NodeKind::HumanTask {
            title,
            required_policy,
            assignee_role_key,
        } => NodeOutcome::Waiting {
            task: NewWaitingTask {
                run_id,
                node_run_id: Some(node_run_id),
                waiting_key: spec.node_key.clone(),
                title: title.clone(),
                assignee_role_key: assignee_role_key.clone(),
                required_policy: required_policy.clone(),
                form_payload: input.clone(),
                due_at: None,
            },
            selected_port: None,
            audit_actions: Vec::new(),
        },
        NodeKind::ChecklistAttestation(config) => NodeOutcome::Waiting {
            task: guardrail_waiting_task(
                run_id,
                node_run_id,
                &spec.node_key,
                CHECKLIST_ATTESTATION_NODE_TYPE,
                &config.label,
                config.required_policy.clone(),
                config.assignee_role_key.clone(),
                json!({
                    "guardrail_kind": CHECKLIST_ATTESTATION_NODE_TYPE,
                    "items": &config.items,
                    "input": input,
                    "approve_requires_all_required": config.approve_requires_all_required,
                    "reject_requires_memo": config.reject_requires_memo,
                    "step_up_required": config.step_up_required,
                }),
            ),
            selected_port: None,
            audit_actions: vec![
                config
                    .audit_event_key
                    .clone()
                    .unwrap_or_else(|| CHECKLIST_OPENED_AUDIT.to_owned()),
            ],
        },
        NodeKind::FourEyesPeerReview(config) => NodeOutcome::Waiting {
            task: guardrail_waiting_task(
                run_id,
                node_run_id,
                &spec.node_key,
                FOUR_EYES_PEER_REVIEW_NODE_TYPE,
                &config.label,
                config.required_policy.clone(),
                config.assignee_role_key.clone(),
                json!({
                    "guardrail_kind": FOUR_EYES_PEER_REVIEW_NODE_TYPE,
                    "subject_actor_refs": &config.subject_actor_refs,
                    "min_reviewers": config.min_reviewers,
                    "forbid_same_actor": config.forbid_same_actor,
                    "step_up_required": config.step_up_required,
                    "input": input,
                }),
            ),
            selected_port: None,
            audit_actions: vec![
                config
                    .audit_event_key
                    .clone()
                    .unwrap_or_else(|| FOUR_EYES_OPENED_AUDIT.to_owned()),
            ],
        },
        NodeKind::SegregationOfDuties(config) => interpret_sod_guard(config, input),
        NodeKind::EgressPolicy(config) => {
            interpret_egress_guard(config, run_id, node_run_id, &spec.node_key, input)
        }
        NodeKind::Job {
            connector,
            job,
            emits_status,
        } => {
            // The emission payload carries the caller's job input plus the
            // connector/job identity the drainer keys on (payload->>'job').
            let mut payload = input.clone();
            if let Value::Object(map) = &mut payload {
                map.insert("connector".to_owned(), json!(connector.as_str()));
                map.insert("job".to_owned(), json!(job.as_str()));
                if let Some(status) = emits_status {
                    map.entry("expected_status")
                        .or_insert_with(|| json!(status.as_str()));
                }
            }
            let emission = OutboxEmission {
                node_run_id: Some(node_run_id),
                channel: OutboxChannel::Job,
                destination_ref: Some(connector.clone()),
                idempotency_key: outbox_job_key(run_id, node_run_id, job),
                payload,
            };
            NodeOutcome::Succeeded {
                output: json!({
                    "emitted": job.as_str(),
                    "connector": connector.as_str(),
                    "selected_port": "done",
                }),
                emissions: vec![emission],
                selected_port: Some("done".to_owned()),
                audit_actions: Vec::new(),
            }
        }
    }
}

fn interpret_sod_guard(config: &SegregationOfDutiesSpec, input: &Value) -> NodeOutcome {
    let actor = input.get("actor_under_test").and_then(Value::as_str);
    let blocked_actors = input.get("blocked_actors").and_then(Value::as_array);
    let Some(actor) = actor else {
        return guardrail_failed(
            "blocked",
            "missing_actor_under_test",
            "SoD guardrail could not resolve actor_under_test from server facts",
            SOD_DENIED_AUDIT,
        );
    };
    let Some(blocked_actors) = blocked_actors else {
        return guardrail_failed(
            "blocked",
            "missing_blocked_actors",
            "SoD guardrail could not resolve blocked actors from server facts",
            SOD_DENIED_AUDIT,
        );
    };
    let blocked = blocked_actors
        .iter()
        .filter_map(Value::as_str)
        .any(|blocked| blocked == actor);
    let exemption_granted = input
        .get("exemption_granted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if blocked && !(config.mode == SodMode::AllowWithGovernanceFinding && exemption_granted) {
        return guardrail_failed(
            "blocked",
            "segregation_of_duties_denied",
            "actor_under_test matches a blocked actor",
            config
                .audit_event_key
                .as_deref()
                .unwrap_or(SOD_DENIED_AUDIT),
        );
    }

    let mut audit_actions = Vec::new();
    if blocked && exemption_granted {
        audit_actions.push(SOD_EXEMPTION_AUDIT.to_owned());
    }
    audit_actions.push(
        config
            .audit_event_key
            .clone()
            .unwrap_or_else(|| SOD_ALLOWED_AUDIT.to_owned()),
    );
    NodeOutcome::Succeeded {
        output: json!({
            "guardrail_kind": SEGREGATION_OF_DUTIES_NODE_TYPE,
            "policy_key": config.policy_key.as_str(),
            "status": "allowed",
            "selected_port": "allowed",
        }),
        emissions: Vec::new(),
        selected_port: Some("allowed".to_owned()),
        audit_actions,
    }
}

fn interpret_egress_guard(
    config: &EgressPolicySpec,
    run_id: Uuid,
    node_run_id: Uuid,
    node_key: &str,
    input: &Value,
) -> NodeOutcome {
    let decision = input
        .get("egress_decision")
        .or_else(|| input.pointer("/egress/decision"))
        .and_then(Value::as_str);
    match decision {
        Some("allowed") => NodeOutcome::Succeeded {
            output: json!({
                "guardrail_kind": EGRESS_POLICY_NODE_TYPE,
                "egress_kind": config.egress_kind,
                "status": "allowed",
                "selected_port": "allowed",
            }),
            emissions: Vec::new(),
            selected_port: Some("allowed".to_owned()),
            audit_actions: vec![
                config
                    .audit_event_key
                    .clone()
                    .unwrap_or_else(|| EGRESS_ALLOWED_AUDIT.to_owned()),
            ],
        },
        Some("review_required") | None
            if config.external_recipient_policy == EgressExternalRecipientPolicy::ManualReview =>
        {
            NodeOutcome::Waiting {
                task: guardrail_waiting_task(
                    run_id,
                    node_run_id,
                    node_key,
                    EGRESS_POLICY_NODE_TYPE,
                    &config.label,
                    config.required_policy.clone(),
                    config.manual_review_role_key.clone(),
                    json!({
                        "guardrail_kind": EGRESS_POLICY_NODE_TYPE,
                        "egress_kind": config.egress_kind,
                        "channel": config.channel.as_str(),
                        "input": input,
                    }),
                ),
                selected_port: Some("review_required".to_owned()),
                audit_actions: vec![
                    config
                        .audit_event_key
                        .clone()
                        .unwrap_or_else(|| EGRESS_REVIEW_AUDIT.to_owned()),
                ],
            }
        }
        Some("denied" | "blocked") | None => guardrail_failed(
            "blocked",
            "egress_denied",
            "egress guardrail did not have an allowed decision",
            config
                .audit_event_key
                .as_deref()
                .unwrap_or(EGRESS_DENIED_AUDIT),
        ),
        Some(other) => guardrail_failed(
            "blocked",
            "invalid_egress_decision",
            &format!("unsupported egress decision {other}"),
            EGRESS_DENIED_AUDIT,
        ),
    }
}

// ponytail: builder-shaped signature, struct-ify deferred
#[allow(clippy::too_many_arguments)]
fn guardrail_waiting_task(
    run_id: Uuid,
    node_run_id: Uuid,
    node_key: &str,
    guardrail_kind: &str,
    title: &str,
    required_policy: Option<String>,
    assignee_role_key: Option<String>,
    mut form_payload: Value,
) -> NewWaitingTask {
    if let Value::Object(map) = &mut form_payload {
        map.entry("guardrail_kind".to_owned())
            .or_insert_with(|| json!(guardrail_kind));
    }
    NewWaitingTask {
        run_id,
        node_run_id: Some(node_run_id),
        waiting_key: node_key.to_owned(),
        title: title.to_owned(),
        assignee_role_key,
        required_policy,
        form_payload,
        due_at: None,
    }
}

fn guardrail_failed(
    selected_port: &str,
    code: &str,
    message: &str,
    audit_action: &str,
) -> NodeOutcome {
    NodeOutcome::Failed {
        error: json!({
            "code": code,
            "message": message,
            "selected_port": selected_port,
        }),
        selected_port: Some(selected_port.to_owned()),
        audit_actions: vec![audit_action.to_owned()],
    }
}

fn parse_guardrail_config<T: for<'de> Deserialize<'de>>(
    node_type: &str,
    config: Value,
) -> Result<T, KernelError> {
    serde_json::from_value(config)
        .map_err(|err| KernelError::validation(format!("invalid config for {node_type}: {err}")))
}

fn guardrail_config_from_execution_node(object: &Map<String, Value>) -> Result<Value, KernelError> {
    if let Some(config) = object.get("config") {
        for key in object.keys() {
            if !matches!(key.as_str(), "node_key" | "node_type" | "config") {
                return Err(KernelError::validation(
                    "guardrail execution node contains an unsupported top-level field",
                ));
            }
        }
        if !config.is_object() {
            return Err(KernelError::validation(
                "guardrail execution node config must be an object",
            ));
        }
        return Ok(config.clone());
    }
    let mut config = object.clone();
    config.remove("node_key");
    config.remove("node_type");
    Ok(Value::Object(config))
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &'static str,
) -> Result<&'a str, KernelError> {
    optional_string(object, key)
        .ok_or_else(|| KernelError::validation(format!("execution node {key} is required")))
}

fn optional_string<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn validate_guard_label(label: &str) -> Result<(), KernelError> {
    let len = label.trim().chars().count();
    if (1..=160).contains(&len) {
        Ok(())
    } else {
        Err(KernelError::validation(
            "guardrail label must be 1 to 160 characters",
        ))
    }
}

fn validate_guard_token(kind: &str, value: &str) -> Result<(), KernelError> {
    let valid = !value.trim().is_empty()
        && value.len() <= 160
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'));
    if valid {
        Ok(())
    } else {
        Err(KernelError::validation(format!(
            "{kind} must be a non-empty server-owned token"
        )))
    }
}

fn validate_ref_list(kind: &str, refs: &[String]) -> Result<(), KernelError> {
    for value in refs {
        validate_guard_token(kind, value)?;
    }
    Ok(())
}

fn validate_optional_feature(policy: Option<&str>) -> Result<(), KernelError> {
    if let Some(policy) = policy {
        Feature::from_str(policy)?;
    }
    Ok(())
}

fn validate_optional_audit_action(action: Option<&str>) -> Result<(), KernelError> {
    if let Some(action) = action {
        AuditAction::new(action)?;
        if !action.starts_with("workflow_guardrail.") {
            return Err(KernelError::validation(
                "guardrail audit_event_key must use the workflow_guardrail namespace",
            ));
        }
    }
    Ok(())
}

fn default_true() -> bool {
    true
}

fn default_one() -> u8 {
    1
}

fn default_actor_under_test_ref() -> String {
    "run.current_actor".to_owned()
}

fn default_sod_scope() -> String {
    "workflow_run".to_owned()
}

fn default_egress_policy() -> EgressExternalRecipientPolicy {
    EgressExternalRecipientPolicy::Block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_finalize_is_a_pre_terminal_waiting_task() {
        let spec = NodeSpec {
            node_key: "finalize.author".to_owned(),
            node_type: "human_task".to_owned(),
            kind: NodeKind::HumanTask {
                title: "Author finalize".to_owned(),
                required_policy: Some("approval_finalize".to_owned()),
                assignee_role_key: Some("initiator".to_owned()),
            },
        };

        assert_eq!(
            spec.human_task_semantic(),
            Some(HumanTaskSemantic::Finalization)
        );
        let outcome = interpret_node(&spec, Uuid::from_u128(1), Uuid::from_u128(2), &json!({}));
        assert_eq!(outcome.node_status(), NodeStatus::Waiting);
        match outcome {
            NodeOutcome::Waiting { task, .. } => {
                assert_eq!(task.waiting_key, "finalize.author");
                assert_eq!(task.required_policy.as_deref(), Some("approval_finalize"));
                assert_eq!(task.assignee_role_key.as_deref(), Some("initiator"));
            }
            other => panic!("expected Waiting, got {other:?}"),
        }
    }

    #[test]
    fn receipt_confirm_is_a_pre_terminal_waiting_task() {
        let spec = NodeSpec {
            node_key: "receipt.target".to_owned(),
            node_type: "human_task".to_owned(),
            kind: NodeKind::HumanTask {
                title: "Receipt confirm".to_owned(),
                required_policy: Some("approval_receipt".to_owned()),
                assignee_role_key: Some("receipt_subject".to_owned()),
            },
        };

        assert_eq!(
            spec.human_task_semantic(),
            Some(HumanTaskSemantic::ReceiptConfirmation)
        );
        let outcome = interpret_node(&spec, Uuid::from_u128(1), Uuid::from_u128(2), &json!({}));
        assert_eq!(outcome.node_status(), NodeStatus::Waiting);
    }

    #[test]
    fn job_node_emits_one_keyed_job_event() {
        let run = Uuid::from_u128(0x11);
        let node = Uuid::from_u128(0x22);
        let spec = NodeSpec {
            node_key: "payroll.draft_gate".to_owned(),
            node_type: "job".to_owned(),
            kind: NodeKind::Job {
                connector: "internal.jobs".to_owned(),
                job: "payroll_draft".to_owned(),
                emits_status: Some("BLOCKED_LEGAL_GATE".to_owned()),
            },
        };
        let outcome = interpret_node(&spec, run, node, &json!({ "period_start": "2026-06-01" }));
        match outcome {
            NodeOutcome::Succeeded { emissions, .. } => {
                assert_eq!(emissions.len(), 1);
                assert_eq!(emissions[0].channel, OutboxChannel::Job);
                assert_eq!(
                    emissions[0].idempotency_key,
                    outbox_job_key(run, node, "payroll_draft")
                );
                assert_eq!(emissions[0].payload["job"], json!("payroll_draft"));
            }
            other => panic!("expected Succeeded, got {other:?}"),
        }
    }

    #[test]
    fn human_task_parks_waiting() {
        let spec = NodeSpec {
            node_key: "approval.executive".to_owned(),
            node_type: "waiting_task".to_owned(),
            kind: NodeKind::HumanTask {
                title: "Executive approval".to_owned(),
                required_policy: Some("completion_review".to_owned()),
                assignee_role_key: Some("executive".to_owned()),
            },
        };
        let outcome = interpret_node(&spec, Uuid::from_u128(1), Uuid::from_u128(2), &json!({}));
        assert_eq!(outcome.node_status(), NodeStatus::Waiting);
    }

    #[test]
    fn guardrail_checklist_opens_audited_waiting_task() {
        let spec = NodeSpec::from_execution_node(&json!({
            "node_key": "guard.checklist.ops_attestation",
            "node_type": "guard.checklist_attestation",
            "label": "Operations attestation",
            "required_policy": "completion_review",
            "assignee_role_key": "operations.manager",
            "items": [{
                "key": "evidence_uploaded",
                "label": "Evidence uploaded",
                "kind": "evidence_ref",
                "required": true,
                "min_count": 1
            }]
        }))
        .expect("guardrail node spec parses");

        let outcome = interpret_node(
            &spec,
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            &json!({ "work_order_id": "wo-1" }),
        );

        match outcome {
            NodeOutcome::Waiting {
                task,
                audit_actions,
                ..
            } => {
                assert_eq!(task.waiting_key, "guard.checklist.ops_attestation");
                assert_eq!(task.title, "Operations attestation");
                assert_eq!(task.required_policy.as_deref(), Some("completion_review"));
                assert_eq!(
                    task.assignee_role_key.as_deref(),
                    Some("operations.manager")
                );
                assert_eq!(
                    task.form_payload["guardrail_kind"],
                    json!("guard.checklist_attestation")
                );
                assert_eq!(
                    task.form_payload["items"][0]["key"],
                    json!("evidence_uploaded")
                );
                assert!(
                    audit_actions
                        .iter()
                        .any(|action| action == "workflow_guardrail.checklist.opened")
                );
            }
            other => panic!("expected checklist guardrail to wait, got {other:?}"),
        }
    }

    #[test]
    fn guardrail_config_rejects_unknown_fields_and_policies() {
        let err = NodeSpec::from_execution_node(&json!({
            "node_key": "guard.checklist.invalid",
            "node_type": "guard.checklist_attestation",
            "label": "Invalid guard",
            "required_policy": "unknown_policy_key",
            "assignee_role_key": "operations.manager",
            "items": [{ "key": "ack", "label": "Ack", "kind": "checkbox" }],
            "browser_supplied_org_id": "must-not-be-accepted"
        }))
        .expect_err("unknown fields and policies must fail closed");

        assert!(
            err.message.contains("unknown field") || err.message.contains("unknown feature"),
            "unexpected error: {}",
            err.message
        );
    }

    #[test]
    fn sod_guardrail_selects_allowed_or_blocked_port_from_server_facts() {
        let spec = NodeSpec::from_execution_node(&json!({
            "node_key": "guard.sod.purchase_approval",
            "node_type": "guard.segregation_of_duties",
            "label": "Purchase approver must differ",
            "policy_key": "purchase.self_approval",
            "actor_under_test_ref": "run.current_actor",
            "blocked_actor_refs": ["object.requested_by"],
            "mode": "hard_block"
        }))
        .expect("sod guard spec parses");

        let allowed = interpret_node(
            &spec,
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            &json!({
                "actor_under_test": "user:approver",
                "blocked_actors": ["user:requester"]
            }),
        );
        match allowed {
            NodeOutcome::Succeeded {
                selected_port,
                audit_actions,
                ..
            } => {
                assert_eq!(selected_port.as_deref(), Some("allowed"));
                assert!(
                    audit_actions
                        .iter()
                        .any(|action| action == "workflow_guardrail.sod.allowed")
                );
            }
            other => panic!("expected SoD guard to allow, got {other:?}"),
        }

        let blocked = interpret_node(
            &spec,
            Uuid::from_u128(1),
            Uuid::from_u128(3),
            &json!({
                "actor_under_test": "user:requester",
                "blocked_actors": ["user:requester"]
            }),
        );
        match blocked {
            NodeOutcome::Failed {
                selected_port,
                audit_actions,
                ..
            } => {
                assert_eq!(selected_port.as_deref(), Some("blocked"));
                assert!(
                    audit_actions
                        .iter()
                        .any(|action| action == "workflow_guardrail.sod.denied")
                );
            }
            other => panic!("expected SoD guard to fail closed, got {other:?}"),
        }
    }

    #[test]
    fn egress_guardrail_fails_closed_without_allowed_decision() {
        let spec = NodeSpec::from_execution_node(&json!({
            "node_key": "guard.egress.mail",
            "node_type": "guard.egress_policy",
            "label": "Mail egress gate",
            "egress_kind": "mail",
            "channel": "internal.mail",
            "required_policy": "mail_use",
            "classification_ref": "mail.thread.classification",
            "external_recipient_policy": "block"
        }))
        .expect("egress guard spec parses");

        let missing = interpret_node(&spec, Uuid::from_u128(1), Uuid::from_u128(2), &json!({}));
        assert_eq!(missing.node_status(), NodeStatus::Failed);
        match missing {
            NodeOutcome::Failed {
                selected_port,
                audit_actions,
                ..
            } => {
                assert_eq!(selected_port.as_deref(), Some("blocked"));
                assert!(
                    audit_actions
                        .iter()
                        .any(|action| action == "workflow_guardrail.egress.denied")
                );
            }
            other => panic!("expected missing egress decision to fail closed, got {other:?}"),
        }

        let allowed = interpret_node(
            &spec,
            Uuid::from_u128(1),
            Uuid::from_u128(3),
            &json!({ "egress_decision": "allowed" }),
        );
        match allowed {
            NodeOutcome::Succeeded {
                selected_port,
                audit_actions,
                ..
            } => {
                assert_eq!(selected_port.as_deref(), Some("allowed"));
                assert!(
                    audit_actions
                        .iter()
                        .any(|action| action == "workflow_guardrail.egress.allowed")
                );
            }
            other => panic!("expected allowed egress decision to pass, got {other:?}"),
        }
    }
}
