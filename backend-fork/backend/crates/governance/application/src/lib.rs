//! Governance application layer — commands, summaries, audit-event builder.
//!
//! Persistence (sqlx) and HTTP live in the outer crates; this layer only shapes
//! the use-case inputs/outputs and stamps audit events.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_governance_domain::{LifecycleState, TransitionRequirements};
use console_kernel_core::{AuditAction, AuditEvent, KernelError, TraceContext, UserId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Four-eyes decision state (mirrors the DB `decision` CHECK).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Pending,
    Approved,
    Rejected,
}

impl ApprovalDecision {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            other => Err(KernelError::validation(format!(
                "unknown approval decision {other:?}"
            ))),
        }
    }
}

/// Open a post-draft edit override: a reason + before-value snapshot recorded
/// before a non-draft instance is edited (arch §3b). Opening an override does NOT
/// open a four-eyes request: to have one decided, the requester must first send a
/// [`CreateApprovalCommand`] keyed by the created override id, and a distinct
/// approver then sends a [`DecidePendingApprovalCommand`] for the same ref. A
/// decision without that open request is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenOverrideCommand {
    pub actor: UserId,
    pub target_type: String,
    pub target_id: Uuid,
    pub reason: String,
    pub before_snapshot: serde_json::Value,
    pub trace: TraceContext,
    pub occurred_at: OffsetDateTime,
}

/// Open a pending four-eyes request: a requester records what needs deciding
/// (arch §19 팀 배포 / override open). A distinct approver later decides it via
/// [`DecideApprovalCommand`] keyed by the same `request_ref`. `request_ref` is a
/// logical ref to whatever is gated (an override id, a console_view instance id
/// for a team deploy, an action-execute ref).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateApprovalCommand {
    pub requester: UserId,
    pub request_ref: Uuid,
    pub kind: String,
    /// The object this approval is FOR (a hold id, a workflow definition id, an
    /// ontology instance id, …). A gate binds the approval to the action's target,
    /// so an approval decided for one object can never satisfy a gate for another.
    /// `None` for create-style actions with no pre-existing target.
    pub target_ref: Option<Uuid>,
    /// Human/UI summary of the change awaiting approval (JSON object).
    pub payload_summary: serde_json::Value,
    pub trace: TraceContext,
    pub occurred_at: OffsetDateTime,
}

/// Record a four-eyes decision. The approver MUST differ from `requested_by`
/// (enforced in the domain, the store, and the DB CHECK — three layers). When a
/// pending [`CreateApprovalCommand`] request exists for `request_ref`, the store
/// treats that request's recorded requester as authoritative (never the
/// client-supplied `requested_by`), so the approver cannot spoof the requester
/// to dodge the self-approval bar.
///
/// For `company.*` / `hr.*` / `payroll.*` kinds the store also requires distinct
/// natural persons via `employee_person_bindings` (console-dgo.1). Generic kinds
/// do not — see [`console_governance_domain::requires_natural_person_four_eyes`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecideApprovalCommand {
    pub approver: UserId,
    pub request_ref: Uuid,
    pub kind: String,
    pub requested_by: UserId,
    /// Fallback binding target when no pending request row exists for this ref
    /// (e.g. a direct-seeded decision in a test). When a pending
    /// [`CreateApprovalCommand`] request IS open, that request's `target_ref` is
    /// authoritative — the approver can no more redirect the target than spoof the
    /// requester.
    pub target_ref: Option<Uuid>,
    pub decision: ApprovalDecision,
    pub trace: TraceContext,
    pub occurred_at: OffsetDateTime,
}

/// Record a four-eyes decision on a request that MUST already be open as a
/// pending [`CreateApprovalCommand`]. This is the hardened decision contract and
/// the one the REST surface uses.
///
/// There is deliberately **no `requested_by` field**: the requester (and the
/// binding target) are read from the open request row inside the decision
/// transaction. Without an open request there is no authenticated requester to be
/// distinct from, so an org admin could otherwise name any other user and
/// manufacture a "four-eyes" approval single-handedly.
///
/// [`DecideApprovalCommand`] remains as the deprecated compatibility contract for
/// in-process callers that seed a decision without opening a request first; it
/// does not require an open request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecidePendingApprovalCommand {
    pub approver: UserId,
    pub request_ref: Uuid,
    pub kind: String,
    pub decision: ApprovalDecision,
    pub trace: TraceContext,
    pub occurred_at: OffsetDateTime,
}

/// Upsert one per-object-type lifecycle FSM edge with its guardrail requirements
/// (arch §15). The edge must be legal in the base FSM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigureTransitionCommand {
    pub actor: UserId,
    pub object_type_id: Uuid,
    pub from_state: LifecycleState,
    pub to_state: LifecycleState,
    pub requirements: TransitionRequirements,
    pub trace: TraceContext,
    pub occurred_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverrideSummary {
    pub id: Uuid,
    pub target_type: String,
    pub target_id: Uuid,
    pub actor: UserId,
    pub reason: String,
    pub before_snapshot: serde_json::Value,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequestSummary {
    pub id: Uuid,
    pub request_ref: Uuid,
    pub kind: String,
    pub requested_by: UserId,
    pub payload_summary: serde_json::Value,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalSummary {
    pub id: Uuid,
    pub request_ref: Uuid,
    pub kind: String,
    pub requested_by: UserId,
    pub approver_id: UserId,
    pub decision: ApprovalDecision,
    #[serde(with = "time::serde::rfc3339")]
    pub decided_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleTransitionConfig {
    pub object_type_id: Uuid,
    pub from_state: LifecycleState,
    pub to_state: LifecycleState,
    pub requirements: TransitionRequirements,
}

/// Build a governance audit event. Governance objects are org-scoped (not
/// branch-scoped), so no branch is attached; the store adds `with_org`.
pub fn governance_audit_event(
    action: &str,
    actor: UserId,
    target_type: &str,
    target_id: impl ToString,
    trace: TraceContext,
    occurred_at: OffsetDateTime,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new(action)?,
        target_type,
        target_id.to_string(),
        trace,
        occurred_at,
    ))
}
