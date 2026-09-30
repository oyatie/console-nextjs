//! Pure support-ticket domain: status FSM and priority→SLA mapping.
//!
//! Kept strictly separate from the 16-state work-order FSM and the P1 dispatch
//! FSM. A support ticket is a help-desk request (internal staff or external
//! customer), not a 정비 job. No I/O lives here — only data and transition
//! rules, exercised by unit tests.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::{
    BranchId, EvidenceObjectId, KernelError, OrgId, SupportTicketId, Timestamp, Transition,
    TransitionError, UserId, WorkOrderId,
};
use serde::{Deserialize, Serialize};
use time::Duration;

/// Ticket lifecycle:
///
/// ```text
/// OPEN ──► IN_PROGRESS ──► RESOLVED ──► CLOSED
///             ▲   │            │
///             │   ▼            │ (reopen)
///             └ ON_HOLD        ▼
///                          IN_PROGRESS
/// ```
///
/// Valid edges:
///   * OPEN        → IN_PROGRESS
///   * IN_PROGRESS → ON_HOLD | RESOLVED
///   * ON_HOLD     → IN_PROGRESS
///   * RESOLVED    → CLOSED | IN_PROGRESS (reopen)
///   * CLOSED      → (terminal)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TicketStatus {
    Open,
    InProgress,
    OnHold,
    Resolved,
    Closed,
}

impl TicketStatus {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::InProgress => "IN_PROGRESS",
            Self::OnHold => "ON_HOLD",
            Self::Resolved => "RESOLVED",
            Self::Closed => "CLOSED",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "OPEN" => Ok(Self::Open),
            "IN_PROGRESS" => Ok(Self::InProgress),
            "ON_HOLD" => Ok(Self::OnHold),
            "RESOLVED" => Ok(Self::Resolved),
            "CLOSED" => Ok(Self::Closed),
            other => Err(KernelError::validation(format!(
                "unknown support ticket status {other:?}"
            ))),
        }
    }

    /// Whether a direct transition `self → to` is permitted by the FSM.
    #[must_use]
    pub const fn can_transition_to(self, to: Self) -> bool {
        matches!(
            (self, to),
            (Self::Open, Self::InProgress)
                | (Self::InProgress, Self::OnHold)
                | (Self::InProgress, Self::Resolved)
                | (Self::OnHold, Self::InProgress)
                | (Self::Resolved, Self::Closed)
                | (Self::Resolved, Self::InProgress)
        )
    }

    /// Apply a status transition, enforcing the FSM. Returns the captured
    /// `from → to` edge for auditing, or an illegal-transition error.
    pub fn transition_to(self, to: Self) -> Result<Transition<Self>, KernelError> {
        if self.can_transition_to(to) {
            Ok(Transition { from: self, to })
        } else {
            Err(TransitionError { from: self, to }.into())
        }
    }

    /// Terminal states accept no further transitions.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Closed)
    }
}

impl std::fmt::Display for TicketStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_db_str())
    }
}

/// Where the ticket entered the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TicketOrigin {
    /// Authenticated staff member; carries the requester's branch.
    Internal,
    /// Unauthenticated external customer; branch-less until triaged.
    Customer,
}

impl TicketOrigin {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Internal => "INTERNAL",
            Self::Customer => "CUSTOMER",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "INTERNAL" => Ok(Self::Internal),
            "CUSTOMER" => Ok(Self::Customer),
            other => Err(KernelError::validation(format!(
                "unknown support ticket origin {other:?}"
            ))),
        }
    }
}

/// Coarse classification used for routing and filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TicketCategory {
    SystemBug,
    AccessRequest,
    Operational,
    EquipmentInquiry,
    Complaint,
    Other,
}

impl TicketCategory {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::SystemBug => "SYSTEM_BUG",
            Self::AccessRequest => "ACCESS_REQUEST",
            Self::Operational => "OPERATIONAL",
            Self::EquipmentInquiry => "EQUIPMENT_INQUIRY",
            Self::Complaint => "COMPLAINT",
            Self::Other => "OTHER",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "SYSTEM_BUG" => Ok(Self::SystemBug),
            "ACCESS_REQUEST" => Ok(Self::AccessRequest),
            "OPERATIONAL" => Ok(Self::Operational),
            "EQUIPMENT_INQUIRY" => Ok(Self::EquipmentInquiry),
            "COMPLAINT" => Ok(Self::Complaint),
            "OTHER" => Ok(Self::Other),
            other => Err(KernelError::validation(format!(
                "unknown support ticket category {other:?}"
            ))),
        }
    }
}

/// Priority drives the SLA `due_at` derived on create.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TicketPriority {
    Low,
    Medium,
    High,
    Urgent,
}

impl TicketPriority {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Low => "LOW",
            Self::Medium => "MEDIUM",
            Self::High => "HIGH",
            Self::Urgent => "URGENT",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "LOW" => Ok(Self::Low),
            "MEDIUM" => Ok(Self::Medium),
            "HIGH" => Ok(Self::High),
            "URGENT" => Ok(Self::Urgent),
            other => Err(KernelError::validation(format!(
                "unknown support ticket priority {other:?}"
            ))),
        }
    }

    /// SLA response window for this priority. Configurable via [`SlaPolicy`];
    /// these are the default targets.
    #[must_use]
    pub const fn default_sla(self) -> Duration {
        match self {
            Self::Urgent => Duration::hours(4),
            Self::High => Duration::days(1),
            Self::Medium => Duration::days(3),
            Self::Low => Duration::days(7),
        }
    }
}

/// Customer acceptance verdict for a resolved field ticket. Acceptance drives
/// the EXISTING ticket FSM edges (`RESOLVED → CLOSED` on accept,
/// `RESOLVED → IN_PROGRESS` reopen on decline) — it adds no new states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AcceptanceKind {
    CustomerAccepted,
    CustomerDeclined,
}

impl AcceptanceKind {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::CustomerAccepted => "CUSTOMER_ACCEPTED",
            Self::CustomerDeclined => "CUSTOMER_DECLINED",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "CUSTOMER_ACCEPTED" => Ok(Self::CustomerAccepted),
            "CUSTOMER_DECLINED" => Ok(Self::CustomerDeclined),
            other => Err(KernelError::validation(format!(
                "unknown support acceptance kind {other:?}"
            ))),
        }
    }

    /// The ticket status this acceptance verdict drives the FSM to, from
    /// RESOLVED: accepted closes the ticket, declined reopens it.
    #[must_use]
    pub const fn transition_target(self) -> TicketStatus {
        match self {
            Self::CustomerAccepted => TicketStatus::Closed,
            Self::CustomerDeclined => TicketStatus::InProgress,
        }
    }
}

/// How the customer acknowledgement was received (curated enum, §4-19; the
/// unauthenticated customer ack link is a future charter — acceptance is
/// staff-recorded today).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AcceptanceChannel {
    InPerson,
    Phone,
    Email,
    Messenger,
}

impl AcceptanceChannel {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::InPerson => "IN_PERSON",
            Self::Phone => "PHONE",
            Self::Email => "EMAIL",
            Self::Messenger => "MESSENGER",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "IN_PERSON" => Ok(Self::InPerson),
            "PHONE" => Ok(Self::Phone),
            "EMAIL" => Ok(Self::Email),
            "MESSENGER" => Ok(Self::Messenger),
            other => Err(KernelError::validation(format!(
                "unknown support acceptance channel {other:?}"
            ))),
        }
    }
}

/// Deterministic per-site SLA state over the site's OPEN/IN_PROGRESS/ON_HOLD
/// tickets (§4-28 no-AI; §4-26 SLA — contractual, site-scoped — never SLO):
/// BREACHED if any `due_at < now`; else AT_RISK if any `due_at < now + 24h`;
/// else OK. The SQL derivation in the adapter is the single evaluation site;
/// this enum is its typed value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FieldSlaState {
    Ok,
    AtRisk,
    Breached,
}

impl FieldSlaState {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::AtRisk => "AT_RISK",
            Self::Breached => "BREACHED",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "OK" => Ok(Self::Ok),
            "AT_RISK" => Ok(Self::AtRisk),
            "BREACHED" => Ok(Self::Breached),
            other => Err(KernelError::validation(format!(
                "unknown field SLA state {other:?}"
            ))),
        }
    }
}

/// Maps a priority to its SLA window. Defaults match [`TicketPriority::default_sla`];
/// kept as a struct so deployment-specific targets can override the constants
/// without touching call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlaPolicy {
    pub urgent: Duration,
    pub high: Duration,
    pub medium: Duration,
    pub low: Duration,
}

impl Default for SlaPolicy {
    fn default() -> Self {
        Self {
            urgent: TicketPriority::Urgent.default_sla(),
            high: TicketPriority::High.default_sla(),
            medium: TicketPriority::Medium.default_sla(),
            low: TicketPriority::Low.default_sla(),
        }
    }
}

impl SlaPolicy {
    #[must_use]
    pub const fn window_for(&self, priority: TicketPriority) -> Duration {
        match priority {
            TicketPriority::Urgent => self.urgent,
            TicketPriority::High => self.high,
            TicketPriority::Medium => self.medium,
            TicketPriority::Low => self.low,
        }
    }

    /// SLA `due_at` for a ticket created at `created_at` with `priority`.
    pub fn due_at(
        &self,
        priority: TicketPriority,
        created_at: Timestamp,
    ) -> Result<Timestamp, KernelError> {
        created_at
            .checked_add(self.window_for(priority))
            .ok_or_else(|| KernelError::validation("support SLA due_at overflows time"))
    }
}

/// Server-derived tenancy and branch ownership for the internal case aggregate.
/// A case is only created after a ticket has been triaged into a branch; this
/// deliberately leaves the existing branch-less customer-intake wire contract unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseScope {
    pub org_id: OrgId,
    pub branch_id: BranchId,
}

impl CaseScope {
    #[must_use]
    pub const fn new(org_id: OrgId, branch_id: BranchId) -> Self {
        Self { org_id, branch_id }
    }
}

/// Dispatch work attached to a support case. It is a reference, never a
/// work-order projection: ownership and status remain in the Work Order context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DispatchHandoffStatus {
    Requested,
    Accepted,
    Rejected,
    Cancelled,
}

impl DispatchHandoffStatus {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Accepted | Self::Rejected | Self::Cancelled)
    }

    #[must_use]
    pub const fn can_transition_to(self, to: Self) -> bool {
        matches!(
            (self, to),
            (Self::Requested, Self::Accepted)
                | (Self::Requested, Self::Rejected)
                | (Self::Requested, Self::Cancelled)
        )
    }
}

/// Idempotently linked work-order handoff. No URL is stored or inferred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchHandoff {
    pub work_order_id: WorkOrderId,
    pub status: DispatchHandoffStatus,
    pub requested_by: UserId,
    #[serde(with = "time::serde::rfc3339")]
    pub requested_at: Timestamp,
    pub resolved_by: Option<UserId>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub resolved_at: Option<Timestamp>,
}

/// A typed evidence reference. Evidence metadata and access control remain in
/// the Evidence context; Support persists only this exact object identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseEvidenceBinding {
    pub evidence_object_id: EvidenceObjectId,
    pub bound_by: UserId,
    #[serde(with = "time::serde::rfc3339")]
    pub bound_at: Timestamp,
}

/// Durable, append-only case history payload. The application transaction
/// persists it together with its matching outbox intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseHistoryEntry {
    pub version: u64,
    pub event: CaseEvent,
    pub actor: UserId,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CaseEvent {
    StatusTransition {
        from: TicketStatus,
        to: TicketStatus,
    },
    DispatchHandoffRequested {
        work_order_id: WorkOrderId,
    },
    DispatchHandoffResolved {
        work_order_id: WorkOrderId,
        status: DispatchHandoffStatus,
    },
    EvidenceBound {
        evidence_object_id: EvidenceObjectId,
    },
}

impl CaseEvent {
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::StatusTransition { .. } => "support.case.status_transition",
            Self::DispatchHandoffRequested { .. } => "support.case.dispatch_handoff_requested",
            Self::DispatchHandoffResolved { .. } => "support.case.dispatch_handoff_resolved",
            Self::EvidenceBound { .. } => "support.case.evidence_bound",
        }
    }
}

/// Internal case aggregate sharing the persisted ticket identifier and status
/// enum. It adds no migration-facing status values and therefore preserves the
/// current `/api/v1/support/tickets` contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SupportCase {
    id: SupportTicketId,
    scope: CaseScope,
    priority: TicketPriority,
    status: TicketStatus,
    due_at: Timestamp,
    version: u64,
    dispatch_handoff: Option<DispatchHandoff>,
    evidence_bindings: Vec<CaseEvidenceBinding>,
    history: Vec<CaseHistoryEntry>,
}

/// Persisted state used to reconstruct a [`SupportCase`] without a long,
/// position-sensitive argument list at adapter boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportCaseRehydration {
    pub id: SupportTicketId,
    pub scope: CaseScope,
    pub priority: TicketPriority,
    pub status: TicketStatus,
    pub due_at: Timestamp,
    pub version: u64,
    pub dispatch_handoff: Option<DispatchHandoff>,
    pub evidence_bindings: Vec<CaseEvidenceBinding>,
    pub history: Vec<CaseHistoryEntry>,
}

impl SupportCase {
    pub fn open(
        id: SupportTicketId,
        scope: CaseScope,
        priority: TicketPriority,
        opened_at: Timestamp,
        sla_policy: SlaPolicy,
    ) -> Result<Self, KernelError> {
        Ok(Self {
            id,
            scope,
            priority,
            status: TicketStatus::Open,
            due_at: sla_policy.due_at(priority, opened_at)?,
            version: 0,
            dispatch_handoff: None,
            evidence_bindings: Vec::new(),
            history: Vec::new(),
        })
    }

    pub fn rehydrate(snapshot: SupportCaseRehydration) -> Result<Self, KernelError> {
        let SupportCaseRehydration {
            id,
            scope,
            priority,
            status,
            due_at,
            version,
            dispatch_handoff,
            evidence_bindings,
            history,
        } = snapshot;
        if history.len()
            != usize::try_from(version)
                .map_err(|_| KernelError::validation("support case version is too large"))?
        {
            return Err(KernelError::validation(
                "support case history must contain every version",
            ));
        }
        for (index, entry) in history.iter().enumerate() {
            let expected_version = u64::try_from(index + 1)
                .map_err(|_| KernelError::validation("support case history is too large"))?;
            if entry.version != expected_version {
                return Err(KernelError::validation(
                    "support case history versions must be contiguous",
                ));
            }
        }
        let mut reconstructed_status = TicketStatus::Open;
        let mut reconstructed_handoff = None;
        let mut reconstructed_evidence = Vec::new();
        for entry in &history {
            if reconstructed_status.is_terminal() {
                return Err(KernelError::validation(
                    "support case history changes a terminal case",
                ));
            }
            match entry.event {
                CaseEvent::StatusTransition { from, to } => {
                    if from != reconstructed_status || from.transition_to(to).is_err() {
                        return Err(KernelError::validation(
                            "support case history contains an invalid status transition",
                        ));
                    }
                    reconstructed_status = to;
                }
                CaseEvent::DispatchHandoffRequested { work_order_id } => {
                    if reconstructed_handoff.is_some() {
                        return Err(KernelError::validation(
                            "support case history contains multiple dispatch handoff requests",
                        ));
                    }
                    reconstructed_handoff = Some(DispatchHandoff {
                        work_order_id,
                        status: DispatchHandoffStatus::Requested,
                        requested_by: entry.actor,
                        requested_at: entry.occurred_at,
                        resolved_by: None,
                        resolved_at: None,
                    });
                }
                CaseEvent::DispatchHandoffResolved {
                    work_order_id,
                    status: handoff_status,
                } => {
                    let Some(handoff) = reconstructed_handoff.as_mut() else {
                        return Err(KernelError::validation(
                            "support case history resolves a missing dispatch handoff",
                        ));
                    };
                    if handoff.work_order_id != work_order_id
                        || !handoff.status.can_transition_to(handoff_status)
                    {
                        return Err(KernelError::validation(
                            "support case history contains an invalid dispatch handoff transition",
                        ));
                    }
                    handoff.status = handoff_status;
                    handoff.resolved_by = Some(entry.actor);
                    handoff.resolved_at = Some(entry.occurred_at);
                }
                CaseEvent::EvidenceBound { evidence_object_id } => {
                    if reconstructed_evidence
                        .iter()
                        .any(|binding: &CaseEvidenceBinding| {
                            binding.evidence_object_id == evidence_object_id
                        })
                    {
                        return Err(KernelError::validation(
                            "support case history binds evidence more than once",
                        ));
                    }
                    reconstructed_evidence.push(CaseEvidenceBinding {
                        evidence_object_id,
                        bound_by: entry.actor,
                        bound_at: entry.occurred_at,
                    });
                }
            }
        }
        if status != reconstructed_status
            || dispatch_handoff != reconstructed_handoff
            || evidence_bindings != reconstructed_evidence
        {
            return Err(KernelError::validation(
                "support case projection does not match durable history",
            ));
        }
        Ok(Self {
            id,
            scope,
            priority,
            status,
            due_at,
            version,
            dispatch_handoff,
            evidence_bindings,
            history,
        })
    }

    #[must_use]
    pub const fn id(&self) -> SupportTicketId {
        self.id
    }

    #[must_use]
    pub const fn scope(&self) -> CaseScope {
        self.scope
    }

    #[must_use]
    pub const fn priority(&self) -> TicketPriority {
        self.priority
    }

    #[must_use]
    pub const fn status(&self) -> TicketStatus {
        self.status
    }

    #[must_use]
    pub const fn due_at(&self) -> Timestamp {
        self.due_at
    }

    #[must_use]
    pub fn dispatch_handoff(&self) -> Option<&DispatchHandoff> {
        self.dispatch_handoff.as_ref()
    }

    #[must_use]
    pub fn evidence_bindings(&self) -> &[CaseEvidenceBinding] {
        &self.evidence_bindings
    }

    #[must_use]
    pub fn history(&self) -> &[CaseHistoryEntry] {
        &self.history
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    pub fn transition(
        &mut self,
        expected_version: u64,
        actor: UserId,
        to: TicketStatus,
        at: Timestamp,
    ) -> Result<(), KernelError> {
        self.require_mutable(expected_version)?;
        let edge = self.status.transition_to(to)?;
        self.status = edge.to;
        self.record(
            actor,
            at,
            CaseEvent::StatusTransition {
                from: edge.from,
                to: edge.to,
            },
        );
        Ok(())
    }

    /// Returns `false` for the same already-linked work order, allowing an
    /// adapter to replay a duplicate delivery without generating new history.
    pub fn request_dispatch_handoff(
        &mut self,
        expected_version: u64,
        actor: UserId,
        work_order_id: WorkOrderId,
        at: Timestamp,
    ) -> Result<bool, KernelError> {
        self.require_mutable(expected_version)?;
        if let Some(existing) = &self.dispatch_handoff {
            if existing.work_order_id == work_order_id {
                return Ok(false);
            }
            return Err(KernelError::conflict(
                "support case already links a different work order",
            ));
        }
        self.dispatch_handoff = Some(DispatchHandoff {
            work_order_id,
            status: DispatchHandoffStatus::Requested,
            requested_by: actor,
            requested_at: at,
            resolved_by: None,
            resolved_at: None,
        });
        self.record(
            actor,
            at,
            CaseEvent::DispatchHandoffRequested { work_order_id },
        );
        Ok(true)
    }

    pub fn resolve_dispatch_handoff(
        &mut self,
        expected_version: u64,
        actor: UserId,
        status: DispatchHandoffStatus,
        at: Timestamp,
    ) -> Result<(), KernelError> {
        self.require_mutable(expected_version)?;
        let handoff = self
            .dispatch_handoff
            .as_mut()
            .ok_or_else(|| KernelError::validation("support case has no dispatch handoff"))?;
        if !handoff.status.can_transition_to(status) {
            return Err(KernelError::conflict(
                "dispatch handoff transition is not permitted",
            ));
        }
        handoff.status = status;
        handoff.resolved_by = Some(actor);
        handoff.resolved_at = Some(at);
        let work_order_id = handoff.work_order_id;
        self.record(
            actor,
            at,
            CaseEvent::DispatchHandoffResolved {
                work_order_id,
                status,
            },
        );
        Ok(())
    }

    /// Returns `false` when the exact evidence object was already bound.
    pub fn bind_evidence(
        &mut self,
        expected_version: u64,
        actor: UserId,
        evidence_object_id: EvidenceObjectId,
        at: Timestamp,
    ) -> Result<bool, KernelError> {
        self.require_mutable(expected_version)?;
        if self
            .evidence_bindings
            .iter()
            .any(|binding| binding.evidence_object_id == evidence_object_id)
        {
            return Ok(false);
        }
        self.evidence_bindings.push(CaseEvidenceBinding {
            evidence_object_id,
            bound_by: actor,
            bound_at: at,
        });
        self.record(actor, at, CaseEvent::EvidenceBound { evidence_object_id });
        Ok(true)
    }

    fn require_mutable(&self, expected_version: u64) -> Result<(), KernelError> {
        if self.version != expected_version {
            return Err(KernelError::conflict("stale support case version"));
        }
        if self.status.is_terminal() {
            return Err(KernelError::conflict(
                "terminal support case cannot be changed",
            ));
        }
        Ok(())
    }

    fn record(&mut self, actor: UserId, occurred_at: Timestamp, event: CaseEvent) {
        self.version += 1;
        self.history.push(CaseHistoryEntry {
            version: self.version,
            event,
            actor,
            occurred_at,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    const ALL_STATUSES: [TicketStatus; 5] = [
        TicketStatus::Open,
        TicketStatus::InProgress,
        TicketStatus::OnHold,
        TicketStatus::Resolved,
        TicketStatus::Closed,
    ];

    #[test]
    fn valid_transitions_are_accepted() {
        let valid = [
            (TicketStatus::Open, TicketStatus::InProgress),
            (TicketStatus::InProgress, TicketStatus::OnHold),
            (TicketStatus::InProgress, TicketStatus::Resolved),
            (TicketStatus::OnHold, TicketStatus::InProgress),
            (TicketStatus::Resolved, TicketStatus::Closed),
            (TicketStatus::Resolved, TicketStatus::InProgress),
        ];
        for (from, to) in valid {
            let transition = from
                .transition_to(to)
                .unwrap_or_else(|_| panic!("{from} -> {to} must be valid"));
            assert_eq!(transition.from, from);
            assert_eq!(transition.to, to);
        }
    }

    #[test]
    fn invalid_transitions_are_rejected_for_full_matrix() {
        let valid = |from: TicketStatus, to: TicketStatus| {
            matches!(
                (from, to),
                (TicketStatus::Open, TicketStatus::InProgress)
                    | (TicketStatus::InProgress, TicketStatus::OnHold)
                    | (TicketStatus::InProgress, TicketStatus::Resolved)
                    | (TicketStatus::OnHold, TicketStatus::InProgress)
                    | (TicketStatus::Resolved, TicketStatus::Closed)
                    | (TicketStatus::Resolved, TicketStatus::InProgress)
            )
        };
        for from in ALL_STATUSES {
            for to in ALL_STATUSES {
                let allowed = from.can_transition_to(to);
                assert_eq!(
                    allowed,
                    valid(from, to),
                    "matrix mismatch for {from} -> {to}"
                );
                if !allowed {
                    assert!(
                        from.transition_to(to).is_err(),
                        "{from} -> {to} must be rejected"
                    );
                }
            }
        }
    }

    #[test]
    fn no_self_transition_is_allowed() {
        for status in ALL_STATUSES {
            assert!(
                status.transition_to(status).is_err(),
                "{status} -> {status} self-loop must be rejected"
            );
        }
    }

    #[test]
    fn closed_is_terminal() {
        assert!(TicketStatus::Closed.is_terminal());
        for to in ALL_STATUSES {
            assert!(TicketStatus::Closed.transition_to(to).is_err());
        }
    }

    #[test]
    fn resolved_can_reopen_to_in_progress() {
        let transition = TicketStatus::Resolved
            .transition_to(TicketStatus::InProgress)
            .unwrap();
        assert_eq!(transition.from, TicketStatus::Resolved);
        assert_eq!(transition.to, TicketStatus::InProgress);
    }

    #[test]
    fn sla_due_at_is_derived_from_priority() {
        let created = datetime!(2026-06-13 09:00 UTC);
        let policy = SlaPolicy::default();
        assert_eq!(
            policy.due_at(TicketPriority::Urgent, created).unwrap(),
            created + Duration::hours(4)
        );
        assert_eq!(
            policy.due_at(TicketPriority::High, created).unwrap(),
            created + Duration::days(1)
        );
        assert_eq!(
            policy.due_at(TicketPriority::Medium, created).unwrap(),
            created + Duration::days(3)
        );
        assert_eq!(
            policy.due_at(TicketPriority::Low, created).unwrap(),
            created + Duration::days(7)
        );
    }

    #[test]
    fn acceptance_kind_drives_only_existing_fsm_edges_from_resolved() {
        // ACCEPTED closes; DECLINED reopens — both must be legal RESOLVED edges.
        for kind in [
            AcceptanceKind::CustomerAccepted,
            AcceptanceKind::CustomerDeclined,
        ] {
            let target = kind.transition_target();
            let transition = TicketStatus::Resolved
                .transition_to(target)
                .unwrap_or_else(|_| panic!("RESOLVED -> {target} must be a legal FSM edge"));
            assert_eq!(transition.from, TicketStatus::Resolved);
        }
        assert_eq!(
            AcceptanceKind::CustomerAccepted.transition_target(),
            TicketStatus::Closed
        );
        assert_eq!(
            AcceptanceKind::CustomerDeclined.transition_target(),
            TicketStatus::InProgress
        );
    }

    #[test]
    fn field_enums_roundtrip_db_strings() {
        for kind in [
            AcceptanceKind::CustomerAccepted,
            AcceptanceKind::CustomerDeclined,
        ] {
            assert_eq!(AcceptanceKind::from_db_str(kind.as_db_str()).unwrap(), kind);
        }
        for channel in [
            AcceptanceChannel::InPerson,
            AcceptanceChannel::Phone,
            AcceptanceChannel::Email,
            AcceptanceChannel::Messenger,
        ] {
            assert_eq!(
                AcceptanceChannel::from_db_str(channel.as_db_str()).unwrap(),
                channel
            );
        }
        for state in [
            FieldSlaState::Ok,
            FieldSlaState::AtRisk,
            FieldSlaState::Breached,
        ] {
            assert_eq!(
                FieldSlaState::from_db_str(state.as_db_str()).unwrap(),
                state
            );
        }
        assert!(AcceptanceKind::from_db_str("BOGUS").is_err());
        assert!(AcceptanceChannel::from_db_str("FAX").is_err());
        assert!(FieldSlaState::from_db_str("SLO").is_err());
    }

    #[test]
    fn db_str_roundtrips_for_all_enums() {
        for status in ALL_STATUSES {
            assert_eq!(
                TicketStatus::from_db_str(status.as_db_str()).unwrap(),
                status
            );
        }
        for origin in [TicketOrigin::Internal, TicketOrigin::Customer] {
            assert_eq!(
                TicketOrigin::from_db_str(origin.as_db_str()).unwrap(),
                origin
            );
        }
        for category in [
            TicketCategory::SystemBug,
            TicketCategory::AccessRequest,
            TicketCategory::Operational,
            TicketCategory::EquipmentInquiry,
            TicketCategory::Complaint,
            TicketCategory::Other,
        ] {
            assert_eq!(
                TicketCategory::from_db_str(category.as_db_str()).unwrap(),
                category
            );
        }
        for priority in [
            TicketPriority::Low,
            TicketPriority::Medium,
            TicketPriority::High,
            TicketPriority::Urgent,
        ] {
            assert_eq!(
                TicketPriority::from_db_str(priority.as_db_str()).unwrap(),
                priority
            );
        }
    }

    #[test]
    fn case_handoff_and_evidence_binding_enforce_terminal_and_idempotent_rules() {
        let scope = CaseScope::new(OrgId::new(), BranchId::new());
        let now = datetime!(2026-07-24 09:00 UTC);
        let mut case = SupportCase::open(
            SupportTicketId::new(),
            scope,
            TicketPriority::High,
            now,
            SlaPolicy::default(),
        )
        .unwrap();
        let work_order = WorkOrderId::new();
        let evidence = EvidenceObjectId::new();
        let actor = UserId::new();

        assert!(
            case.request_dispatch_handoff(case.version(), actor, work_order, now)
                .unwrap()
        );
        assert!(
            !case
                .request_dispatch_handoff(case.version(), actor, work_order, now)
                .unwrap()
        );
        assert!(
            case.bind_evidence(case.version(), actor, evidence, now)
                .unwrap()
        );
        assert!(
            !case
                .bind_evidence(case.version(), actor, evidence, now)
                .unwrap()
        );

        case.transition(case.version(), actor, TicketStatus::InProgress, now)
            .unwrap();
        case.transition(case.version(), actor, TicketStatus::Resolved, now)
            .unwrap();
        case.transition(case.version(), actor, TicketStatus::Closed, now)
            .unwrap();
        assert!(
            case.bind_evidence(case.version(), actor, EvidenceObjectId::new(), now)
                .is_err()
        );
    }

    #[test]
    fn case_requires_current_version_for_mutation() {
        let now = datetime!(2026-07-24 09:00 UTC);
        let mut case = SupportCase::open(
            SupportTicketId::new(),
            CaseScope::new(OrgId::new(), BranchId::new()),
            TicketPriority::Medium,
            now,
            SlaPolicy::default(),
        )
        .unwrap();
        let actor = UserId::new();
        case.transition(0, actor, TicketStatus::InProgress, now)
            .unwrap();
        assert!(
            case.transition(0, actor, TicketStatus::Resolved, now)
                .is_err()
        );
    }

    #[test]
    fn rehydration_rejects_non_contiguous_history() {
        let now = datetime!(2026-07-24 09:00 UTC);
        let result = SupportCase::rehydrate(SupportCaseRehydration {
            id: SupportTicketId::new(),
            scope: CaseScope::new(OrgId::new(), BranchId::new()),
            priority: TicketPriority::Medium,
            status: TicketStatus::Open,
            due_at: now,
            version: 1,
            dispatch_handoff: None,
            evidence_bindings: Vec::new(),
            history: Vec::new(),
        });
        assert!(result.is_err());
    }

    #[test]
    fn rehydration_preserves_valid_case_through_read_only_accessors() {
        let now = datetime!(2026-07-24 09:00 UTC);
        let mut opened = SupportCase::open(
            SupportTicketId::new(),
            CaseScope::new(OrgId::new(), BranchId::new()),
            TicketPriority::Medium,
            now,
            SlaPolicy::default(),
        )
        .unwrap();
        let actor = UserId::new();
        let work_order_id = WorkOrderId::new();
        let evidence_object_id = EvidenceObjectId::new();
        opened
            .request_dispatch_handoff(0, actor, work_order_id, now)
            .unwrap();
        opened
            .resolve_dispatch_handoff(1, actor, DispatchHandoffStatus::Accepted, now)
            .unwrap();
        opened
            .bind_evidence(2, actor, evidence_object_id, now)
            .unwrap();
        opened
            .transition(3, actor, TicketStatus::InProgress, now)
            .unwrap();
        let rehydrated = SupportCase::rehydrate(SupportCaseRehydration {
            id: opened.id(),
            scope: opened.scope(),
            priority: opened.priority(),
            status: opened.status(),
            due_at: opened.due_at(),
            version: opened.version(),
            dispatch_handoff: opened.dispatch_handoff().cloned(),
            evidence_bindings: opened.evidence_bindings().to_vec(),
            history: opened.history().to_vec(),
        })
        .unwrap();
        assert_eq!(rehydrated.id(), opened.id());
        assert_eq!(rehydrated.status(), TicketStatus::InProgress);
        assert_eq!(
            rehydrated.evidence_bindings()[0].evidence_object_id,
            evidence_object_id
        );
        assert_eq!(
            rehydrated.dispatch_handoff().unwrap().work_order_id,
            work_order_id
        );
    }

    #[test]
    fn rehydration_rejects_status_projection_that_disagrees_with_history() {
        let now = datetime!(2026-07-24 09:00 UTC);
        let result = SupportCase::rehydrate(SupportCaseRehydration {
            id: SupportTicketId::new(),
            scope: CaseScope::new(OrgId::new(), BranchId::new()),
            priority: TicketPriority::Medium,
            status: TicketStatus::Open,
            due_at: now,
            version: 1,
            dispatch_handoff: None,
            evidence_bindings: Vec::new(),
            history: vec![CaseHistoryEntry {
                version: 1,
                event: CaseEvent::StatusTransition {
                    from: TicketStatus::Open,
                    to: TicketStatus::InProgress,
                },
                actor: UserId::new(),
                occurred_at: now,
            }],
        });
        assert!(result.is_err());
    }

    #[test]
    fn rehydration_rejects_incoherent_handoff_and_evidence_history() {
        let now = datetime!(2026-07-24 09:00 UTC);
        let scope = CaseScope::new(OrgId::new(), BranchId::new());
        let actor = UserId::new();
        let missing_handoff = SupportCase::rehydrate(SupportCaseRehydration {
            id: SupportTicketId::new(),
            scope,
            priority: TicketPriority::Medium,
            status: TicketStatus::Open,
            due_at: now,
            version: 1,
            dispatch_handoff: None,
            evidence_bindings: Vec::new(),
            history: vec![CaseHistoryEntry {
                version: 1,
                event: CaseEvent::DispatchHandoffResolved {
                    work_order_id: WorkOrderId::new(),
                    status: DispatchHandoffStatus::Accepted,
                },
                actor,
                occurred_at: now,
            }],
        });
        assert!(missing_handoff.is_err());

        let evidence = EvidenceObjectId::new();
        let missing_evidence_projection = SupportCase::rehydrate(SupportCaseRehydration {
            id: SupportTicketId::new(),
            scope,
            priority: TicketPriority::Medium,
            status: TicketStatus::Open,
            due_at: now,
            version: 1,
            dispatch_handoff: None,
            evidence_bindings: Vec::new(),
            history: vec![CaseHistoryEntry {
                version: 1,
                event: CaseEvent::EvidenceBound {
                    evidence_object_id: evidence,
                },
                actor,
                occurred_at: now,
            }],
        });
        assert!(missing_evidence_projection.is_err());
    }
}
