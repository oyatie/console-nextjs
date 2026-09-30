//! Work-order domain.
//!
//! Pure state machines and value objects only. Persistence, audit writes,
//! runtime concerns, and evidence storage adapters live in outer crates.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::BTreeSet;

use console_kernel_core::{
    ApprovalId, AssignmentId, BranchId, KernelError, Timestamp, Transition, TransitionError,
    UserId, WorkOrderId,
};

macro_rules! domain_enum {
    (
        $(#[$enum_meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident => $wire:literal
            ),+ $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            serde::Serialize,
            serde::Deserialize,
        )]
        #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
        pub enum $name {
            $(
                $(#[$variant_meta])*
                $variant,
            )+
        }

        impl $name {
            #[must_use]
            pub const fn as_db_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }

            pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
                match value {
                    $($wire => Ok(Self::$variant),)+
                    other => Err(KernelError::validation(format!(
                        "unknown {} value {other:?}",
                        stringify!($name)
                    ))),
                }
            }
        }
    };
}

domain_enum! {
    /// Work-order lifecycle states inherited from the prior project.
    pub enum WorkOrderStatus {
        Received => "RECEIVED",
        Unassigned => "UNASSIGNED",
        Assigned => "ASSIGNED",
        InProgress => "IN_PROGRESS",
        ReportSubmitted => "REPORT_SUBMITTED",
        AdminReview => "ADMIN_REVIEW",
        FinalCompleted => "FINAL_COMPLETED",
        Rejected => "REJECTED",
        OnHold => "ON_HOLD",
        Delayed => "DELAYED",
        TemporaryAction => "TEMPORARY_ACTION",
        PartWaiting => "PART_WAITING",
        EquipmentInUse => "EQUIPMENT_IN_USE",
        RevisitRequired => "REVISIT_REQUIRED",
        Archived => "ARCHIVED",
        Cancelled => "CANCELLED",
    }
}

domain_enum! {
    /// Operational priority from the prior project.
    pub enum PriorityLevel {
        P1 => "P1",
        P2 => "P2",
        P3 => "P3",
        Outsource => "OUTSOURCE",
        Unset => "UNSET",
    }
}

domain_enum! {
    /// Maintenance classification: what kind of work the order is (유형).
    pub enum MaintenanceType {
        Emergency => "EMERGENCY",
        Corrective => "CORRECTIVE",
        Preventive => "PREVENTIVE",
        Inspection => "INSPECTION",
    }
}

domain_enum! {
    /// Maintenance classification: why the order exists (원인).
    pub enum MaintenanceCause {
        Breakdown => "BREAKDOWN",
        ReturnPrep => "RETURN_PREP",
        Scheduled => "SCHEDULED",
        InspectionFinding => "INSPECTION_FINDING",
        Other => "OTHER",
    }
}

domain_enum! {
    /// Cost-settlement lifecycle (정산 → 전표).
    pub enum SettlementStatus {
        Draft => "DRAFT",
        Submitted => "SUBMITTED",
        Approved => "APPROVED",
        Void => "VOID",
    }
}

domain_enum! {
    /// Cost-settlement line classification.
    pub enum SettlementLineKind {
        Labor => "LABOR",
        Part => "PART",
        Outsource => "OUTSOURCE",
        Other => "OTHER",
    }
}

/// Work-order statuses from which a cost settlement may be opened: the report
/// exists (REPORT_SUBMITTED / ADMIN_REVIEW) or the order is already closed
/// (FINAL_COMPLETED) and its cost is being booked.
pub const SETTLEMENT_ELIGIBLE_WORK_ORDER_STATUSES: &[WorkOrderStatus; 3] = &[
    WorkOrderStatus::ReportSubmitted,
    WorkOrderStatus::AdminReview,
    WorkOrderStatus::FinalCompleted,
];

/// Settlement FSM edges (design-contract §3.2). Four-eyes (approver ≠
/// submitter) and comment/reason requirements are enforced by the callers that
/// know the actors; this table is the pure legal-edge set.
pub const SETTLEMENT_TRANSITIONS: &[(SettlementStatus, SettlementStatus)] = &[
    (SettlementStatus::Draft, SettlementStatus::Submitted),
    (SettlementStatus::Submitted, SettlementStatus::Approved),
    (SettlementStatus::Submitted, SettlementStatus::Draft),
    (SettlementStatus::Draft, SettlementStatus::Void),
    (SettlementStatus::Submitted, SettlementStatus::Void),
];

pub fn validate_settlement_transition(
    from: SettlementStatus,
    to: SettlementStatus,
) -> Result<Transition<SettlementStatus>, KernelError> {
    SETTLEMENT_TRANSITIONS
        .iter()
        .find(|(rule_from, rule_to)| *rule_from == from && *rule_to == to)
        .map(|_| Transition { from, to })
        .ok_or_else(|| TransitionError { from, to }.into())
}

domain_enum! {
    /// Reason a work order is delayed or blocked.
    pub enum DelayReason {
        PartWaiting => "PART_WAITING",
        CustomerAbsent => "CUSTOMER_ABSENT",
        EquipmentInUse => "EQUIPMENT_IN_USE",
        MechanicOverloaded => "MECHANIC_OVERLOADED",
        OutsourceDelay => "OUTSOURCE_DELAY",
        AdditionalFaultFound => "ADDITIONAL_FAULT_FOUND",
        SafetyIssue => "SAFETY_ISSUE",
        Other => "OTHER",
    }
}

domain_enum! {
    /// Mechanic report outcome.
    pub enum WorkResultType {
        Completed => "COMPLETED",
        TemporaryAction => "TEMPORARY_ACTION",
        Incomplete => "INCOMPLETE",
        RevisitRequired => "REVISIT_REQUIRED",
        Unknown => "UNKNOWN",
    }
}

domain_enum! {
    /// Attachment stage for request evidence, work evidence, reports, and outsource results.
    pub enum AttachmentStage {
        Request => "REQUEST",
        Before => "BEFORE",
        During => "DURING",
        After => "AFTER",
        Report => "REPORT",
        OutsourceResult => "OUTSOURCE_RESULT",
    }
}

domain_enum! {
    /// Ordered approval-line roles.
    pub enum ApprovalRole {
        Mechanic => "MECHANIC",
        Admin => "ADMIN",
        Executive => "EXECUTIVE",
    }
}

domain_enum! {
    /// Approval-line step state.
    pub enum ApprovalStatus {
        NotStarted => "NOT_STARTED",
        Pending => "PENDING",
        Approved => "APPROVED",
        Rejected => "REJECTED",
    }
}

domain_enum! {
    /// Assignment role for 2-person work: 주/부.
    pub enum AssignmentRole {
        Primary => "PRIMARY",
        Secondary => "SECONDARY",
    }
}

domain_enum! {
    /// Coarse actor authority for transition-table guards.
    pub enum TransitionActor {
        Mechanic => "MECHANIC",
        Admin => "ADMIN",
        System => "SYSTEM",
    }
}

pub const ALL_WORK_ORDER_STATUSES: &[WorkOrderStatus; 16] = &[
    WorkOrderStatus::Received,
    WorkOrderStatus::Unassigned,
    WorkOrderStatus::Assigned,
    WorkOrderStatus::InProgress,
    WorkOrderStatus::ReportSubmitted,
    WorkOrderStatus::AdminReview,
    WorkOrderStatus::FinalCompleted,
    WorkOrderStatus::Rejected,
    WorkOrderStatus::OnHold,
    WorkOrderStatus::Delayed,
    WorkOrderStatus::TemporaryAction,
    WorkOrderStatus::PartWaiting,
    WorkOrderStatus::EquipmentInUse,
    WorkOrderStatus::RevisitRequired,
    WorkOrderStatus::Archived,
    WorkOrderStatus::Cancelled,
];

/// Runtime facts needed to satisfy guarded transition-table edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionGuardContext {
    pub actor: TransitionActor,
    pub approval_line_complete: bool,
    pub completion_evidence_verified: bool,
}

impl TransitionGuardContext {
    #[must_use]
    pub const fn mechanic() -> Self {
        Self {
            actor: TransitionActor::Mechanic,
            approval_line_complete: false,
            completion_evidence_verified: false,
        }
    }

    #[must_use]
    pub const fn admin() -> Self {
        Self {
            actor: TransitionActor::Admin,
            approval_line_complete: false,
            completion_evidence_verified: false,
        }
    }
}

/// Guard attached to a transition-table edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TransitionGuard {
    Open,
    AdminOnly,
    ApprovalLineComplete,
    FinalCompletionInterlock,
}

impl TransitionGuard {
    fn ensure(self, context: TransitionGuardContext) -> Result<(), KernelError> {
        match self {
            Self::Open => Ok(()),
            Self::AdminOnly if context.actor == TransitionActor::Admin => Ok(()),
            Self::AdminOnly => Err(KernelError::forbidden(
                "work-order transition requires admin authority",
            )),
            Self::ApprovalLineComplete if context.approval_line_complete => Ok(()),
            Self::ApprovalLineComplete => Err(KernelError::conflict(
                "approval line must be complete before this transition",
            )),
            Self::FinalCompletionInterlock
                if context.approval_line_complete && context.completion_evidence_verified =>
            {
                Ok(())
            }
            Self::FinalCompletionInterlock if !context.approval_line_complete => Err(
                KernelError::conflict("approval line must be complete before final completion"),
            ),
            Self::FinalCompletionInterlock => Err(KernelError::conflict(
                "required completion evidence is not WORM-verified",
            )),
        }
    }
}

/// One explicit legal work-order FSM edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkOrderTransitionRule {
    pub from: WorkOrderStatus,
    pub to: WorkOrderStatus,
    pub guard: TransitionGuard,
    pub rationale: &'static str,
}

const fn edge(
    from: WorkOrderStatus,
    to: WorkOrderStatus,
    guard: TransitionGuard,
    rationale: &'static str,
) -> WorkOrderTransitionRule {
    WorkOrderTransitionRule {
        from,
        to,
        guard,
        rationale,
    }
}

/// Const transition table. Every edge carries the operational reason it exists.
pub const WORK_ORDER_TRANSITIONS: &[WorkOrderTransitionRule] = &[
    edge(
        WorkOrderStatus::Received,
        WorkOrderStatus::Assigned,
        TransitionGuard::Open,
        "initial assignment turns a newly received request into assigned work",
    ),
    edge(
        WorkOrderStatus::Unassigned,
        WorkOrderStatus::Assigned,
        TransitionGuard::Open,
        "manual assignment moves an unassigned request onto a mechanic queue",
    ),
    edge(
        WorkOrderStatus::Received,
        WorkOrderStatus::InProgress,
        TransitionGuard::Open,
        "urgent work may be started directly from intake",
    ),
    edge(
        WorkOrderStatus::Unassigned,
        WorkOrderStatus::InProgress,
        TransitionGuard::Open,
        "urgent work may be claimed and started before formal assignment",
    ),
    edge(
        WorkOrderStatus::Assigned,
        WorkOrderStatus::InProgress,
        TransitionGuard::Open,
        "assigned mechanic starts work",
    ),
    edge(
        WorkOrderStatus::InProgress,
        WorkOrderStatus::ReportSubmitted,
        TransitionGuard::Open,
        "mechanic submits the completion report",
    ),
    edge(
        WorkOrderStatus::ReportSubmitted,
        WorkOrderStatus::AdminReview,
        TransitionGuard::Open,
        "admin approval advances the report to executive review",
    ),
    edge(
        WorkOrderStatus::AdminReview,
        WorkOrderStatus::FinalCompleted,
        TransitionGuard::FinalCompletionInterlock,
        "all approvals are complete and required report evidence is WORM-verified",
    ),
    edge(
        WorkOrderStatus::AdminReview,
        WorkOrderStatus::TemporaryAction,
        TransitionGuard::ApprovalLineComplete,
        "all approvals are complete but the report outcome needs follow-up",
    ),
    edge(
        WorkOrderStatus::Assigned,
        WorkOrderStatus::Delayed,
        TransitionGuard::Open,
        "assigned work missed or is expected to miss its target date",
    ),
    edge(
        WorkOrderStatus::InProgress,
        WorkOrderStatus::Delayed,
        TransitionGuard::Open,
        "in-progress work missed or is expected to miss its target date",
    ),
    edge(
        WorkOrderStatus::OnHold,
        WorkOrderStatus::Delayed,
        TransitionGuard::Open,
        "held work becomes overdue while blocked",
    ),
    edge(
        WorkOrderStatus::PartWaiting,
        WorkOrderStatus::Delayed,
        TransitionGuard::Open,
        "part-waiting work becomes overdue",
    ),
    edge(
        WorkOrderStatus::EquipmentInUse,
        WorkOrderStatus::Delayed,
        TransitionGuard::Open,
        "customer equipment remained unavailable past target",
    ),
    edge(
        WorkOrderStatus::RevisitRequired,
        WorkOrderStatus::Delayed,
        TransitionGuard::Open,
        "required revisit is not completed by target",
    ),
    edge(
        WorkOrderStatus::Assigned,
        WorkOrderStatus::OnHold,
        TransitionGuard::Open,
        "assigned work is intentionally paused before starting",
    ),
    edge(
        WorkOrderStatus::InProgress,
        WorkOrderStatus::OnHold,
        TransitionGuard::Open,
        "active work is paused for an operational block",
    ),
    edge(
        WorkOrderStatus::TemporaryAction,
        WorkOrderStatus::OnHold,
        TransitionGuard::Open,
        "follow-up work is paused after temporary action",
    ),
    edge(
        WorkOrderStatus::OnHold,
        WorkOrderStatus::Assigned,
        TransitionGuard::Open,
        "hold is cleared before work resumes",
    ),
    edge(
        WorkOrderStatus::OnHold,
        WorkOrderStatus::InProgress,
        TransitionGuard::Open,
        "hold is cleared and the mechanic resumes work",
    ),
    edge(
        WorkOrderStatus::InProgress,
        WorkOrderStatus::PartWaiting,
        TransitionGuard::Open,
        "work is blocked until a required part arrives",
    ),
    edge(
        WorkOrderStatus::Assigned,
        WorkOrderStatus::PartWaiting,
        TransitionGuard::Open,
        "work cannot start until a required part arrives",
    ),
    edge(
        WorkOrderStatus::PartWaiting,
        WorkOrderStatus::InProgress,
        TransitionGuard::Open,
        "required part arrived and work resumes",
    ),
    edge(
        WorkOrderStatus::InProgress,
        WorkOrderStatus::EquipmentInUse,
        TransitionGuard::Open,
        "customer equipment is unavailable during the visit",
    ),
    edge(
        WorkOrderStatus::Assigned,
        WorkOrderStatus::EquipmentInUse,
        TransitionGuard::Open,
        "customer equipment is unavailable before the visit starts",
    ),
    edge(
        WorkOrderStatus::EquipmentInUse,
        WorkOrderStatus::InProgress,
        TransitionGuard::Open,
        "customer releases equipment and work resumes",
    ),
    edge(
        WorkOrderStatus::TemporaryAction,
        WorkOrderStatus::RevisitRequired,
        TransitionGuard::Open,
        "temporary action follow-up is due",
    ),
    edge(
        WorkOrderStatus::InProgress,
        WorkOrderStatus::RevisitRequired,
        TransitionGuard::Open,
        "mechanic finds the issue requires a revisit",
    ),
    edge(
        WorkOrderStatus::RevisitRequired,
        WorkOrderStatus::Assigned,
        TransitionGuard::Open,
        "revisit is scheduled onto a mechanic queue",
    ),
    edge(
        WorkOrderStatus::RevisitRequired,
        WorkOrderStatus::InProgress,
        TransitionGuard::Open,
        "mechanic starts the required revisit",
    ),
    edge(
        WorkOrderStatus::Delayed,
        WorkOrderStatus::Assigned,
        TransitionGuard::Open,
        "delayed work is replanned before resuming",
    ),
    edge(
        WorkOrderStatus::Delayed,
        WorkOrderStatus::InProgress,
        TransitionGuard::Open,
        "delayed work resumes immediately",
    ),
    edge(
        WorkOrderStatus::TemporaryAction,
        WorkOrderStatus::InProgress,
        TransitionGuard::Open,
        "follow-up work starts after a temporary action",
    ),
    edge(
        WorkOrderStatus::Received,
        WorkOrderStatus::Rejected,
        TransitionGuard::AdminOnly,
        "admin rejects an active request",
    ),
    edge(
        WorkOrderStatus::Unassigned,
        WorkOrderStatus::Rejected,
        TransitionGuard::AdminOnly,
        "admin rejects an active unassigned request",
    ),
    edge(
        WorkOrderStatus::Assigned,
        WorkOrderStatus::Rejected,
        TransitionGuard::AdminOnly,
        "admin rejects active assigned work",
    ),
    edge(
        WorkOrderStatus::InProgress,
        WorkOrderStatus::Rejected,
        TransitionGuard::AdminOnly,
        "admin rejects active in-progress work",
    ),
    edge(
        WorkOrderStatus::ReportSubmitted,
        WorkOrderStatus::Rejected,
        TransitionGuard::AdminOnly,
        "admin rejects a submitted report",
    ),
    edge(
        WorkOrderStatus::AdminReview,
        WorkOrderStatus::Rejected,
        TransitionGuard::AdminOnly,
        "admin rejects work during approval review",
    ),
    edge(
        WorkOrderStatus::OnHold,
        WorkOrderStatus::Rejected,
        TransitionGuard::AdminOnly,
        "admin rejects active held work",
    ),
    edge(
        WorkOrderStatus::Delayed,
        WorkOrderStatus::Rejected,
        TransitionGuard::AdminOnly,
        "admin rejects active delayed work",
    ),
    edge(
        WorkOrderStatus::TemporaryAction,
        WorkOrderStatus::Rejected,
        TransitionGuard::AdminOnly,
        "admin rejects active temporary-action follow-up",
    ),
    edge(
        WorkOrderStatus::PartWaiting,
        WorkOrderStatus::Rejected,
        TransitionGuard::AdminOnly,
        "admin rejects active part-waiting work",
    ),
    edge(
        WorkOrderStatus::EquipmentInUse,
        WorkOrderStatus::Rejected,
        TransitionGuard::AdminOnly,
        "admin rejects active equipment-unavailable work",
    ),
    edge(
        WorkOrderStatus::RevisitRequired,
        WorkOrderStatus::Rejected,
        TransitionGuard::AdminOnly,
        "admin rejects active revisit work",
    ),
    edge(
        WorkOrderStatus::Received,
        WorkOrderStatus::Cancelled,
        TransitionGuard::AdminOnly,
        "admin cancels an active request that should not proceed",
    ),
    edge(
        WorkOrderStatus::Unassigned,
        WorkOrderStatus::Cancelled,
        TransitionGuard::AdminOnly,
        "admin cancels an unassigned active request",
    ),
    edge(
        WorkOrderStatus::Assigned,
        WorkOrderStatus::Cancelled,
        TransitionGuard::AdminOnly,
        "admin cancels assigned active work",
    ),
    edge(
        WorkOrderStatus::InProgress,
        WorkOrderStatus::Cancelled,
        TransitionGuard::AdminOnly,
        "admin cancels in-progress active work",
    ),
    edge(
        WorkOrderStatus::ReportSubmitted,
        WorkOrderStatus::Cancelled,
        TransitionGuard::AdminOnly,
        "admin cancels a submitted report before final approval",
    ),
    edge(
        WorkOrderStatus::AdminReview,
        WorkOrderStatus::Cancelled,
        TransitionGuard::AdminOnly,
        "admin cancels work still under review",
    ),
    edge(
        WorkOrderStatus::OnHold,
        WorkOrderStatus::Cancelled,
        TransitionGuard::AdminOnly,
        "admin cancels held active work",
    ),
    edge(
        WorkOrderStatus::Delayed,
        WorkOrderStatus::Cancelled,
        TransitionGuard::AdminOnly,
        "admin cancels delayed active work",
    ),
    edge(
        WorkOrderStatus::TemporaryAction,
        WorkOrderStatus::Cancelled,
        TransitionGuard::AdminOnly,
        "admin cancels temporary-action follow-up",
    ),
    edge(
        WorkOrderStatus::PartWaiting,
        WorkOrderStatus::Cancelled,
        TransitionGuard::AdminOnly,
        "admin cancels part-waiting active work",
    ),
    edge(
        WorkOrderStatus::EquipmentInUse,
        WorkOrderStatus::Cancelled,
        TransitionGuard::AdminOnly,
        "admin cancels equipment-unavailable active work",
    ),
    edge(
        WorkOrderStatus::RevisitRequired,
        WorkOrderStatus::Cancelled,
        TransitionGuard::AdminOnly,
        "admin cancels revisit-required active work",
    ),
    edge(
        WorkOrderStatus::Received,
        WorkOrderStatus::Archived,
        TransitionGuard::AdminOnly,
        "admin archives a received record",
    ),
    edge(
        WorkOrderStatus::Unassigned,
        WorkOrderStatus::Archived,
        TransitionGuard::AdminOnly,
        "admin archives an unassigned record",
    ),
    edge(
        WorkOrderStatus::Assigned,
        WorkOrderStatus::Archived,
        TransitionGuard::AdminOnly,
        "admin archives an assigned record",
    ),
    edge(
        WorkOrderStatus::InProgress,
        WorkOrderStatus::Archived,
        TransitionGuard::AdminOnly,
        "admin archives an in-progress record",
    ),
    edge(
        WorkOrderStatus::ReportSubmitted,
        WorkOrderStatus::Archived,
        TransitionGuard::AdminOnly,
        "admin archives a submitted-report record",
    ),
    edge(
        WorkOrderStatus::AdminReview,
        WorkOrderStatus::Archived,
        TransitionGuard::AdminOnly,
        "admin archives a review record",
    ),
    edge(
        WorkOrderStatus::FinalCompleted,
        WorkOrderStatus::Archived,
        TransitionGuard::AdminOnly,
        "admin archives a final-completed record",
    ),
    edge(
        WorkOrderStatus::Rejected,
        WorkOrderStatus::Archived,
        TransitionGuard::AdminOnly,
        "admin archives a rejected record",
    ),
    edge(
        WorkOrderStatus::OnHold,
        WorkOrderStatus::Archived,
        TransitionGuard::AdminOnly,
        "admin archives a held record",
    ),
    edge(
        WorkOrderStatus::Delayed,
        WorkOrderStatus::Archived,
        TransitionGuard::AdminOnly,
        "admin archives a delayed record",
    ),
    edge(
        WorkOrderStatus::TemporaryAction,
        WorkOrderStatus::Archived,
        TransitionGuard::AdminOnly,
        "admin archives a temporary-action record",
    ),
    edge(
        WorkOrderStatus::PartWaiting,
        WorkOrderStatus::Archived,
        TransitionGuard::AdminOnly,
        "admin archives a part-waiting record",
    ),
    edge(
        WorkOrderStatus::EquipmentInUse,
        WorkOrderStatus::Archived,
        TransitionGuard::AdminOnly,
        "admin archives an equipment-unavailable record",
    ),
    edge(
        WorkOrderStatus::RevisitRequired,
        WorkOrderStatus::Archived,
        TransitionGuard::AdminOnly,
        "admin archives a revisit-required record",
    ),
    edge(
        WorkOrderStatus::Cancelled,
        WorkOrderStatus::Archived,
        TransitionGuard::AdminOnly,
        "admin archives a cancelled record",
    ),
];

pub fn validate_status_transition(
    from: WorkOrderStatus,
    to: WorkOrderStatus,
    context: TransitionGuardContext,
) -> Result<Transition<WorkOrderStatus>, KernelError> {
    let rule = WORK_ORDER_TRANSITIONS
        .iter()
        .find(|rule| rule.from == from && rule.to == to)
        .ok_or_else(|| illegal_transition(from, to))?;
    rule.guard.ensure(context)?;
    Ok(Transition { from, to })
}

fn illegal_transition(from: WorkOrderStatus, to: WorkOrderStatus) -> KernelError {
    TransitionError { from, to }.into()
}

/// Domain port for the T1.4 evidence pipeline: final completion may only pass
/// when required AFTER/REPORT evidence is WORM-verified.
pub trait CompletionEvidenceInterlock {
    fn final_completion_evidence_verified(
        &self,
        work_order_id: WorkOrderId,
    ) -> Result<bool, KernelError>;
}

domain_enum! {
    /// Already-computed evidence decision supplied by the future T1.4 adapter.
    pub enum CompletionEvidence {
        Verified => "VERIFIED",
        Unverified => "UNVERIFIED",
    }
}

impl CompletionEvidenceInterlock for CompletionEvidence {
    fn final_completion_evidence_verified(
        &self,
        _work_order_id: WorkOrderId,
    ) -> Result<bool, KernelError> {
        Ok(*self == Self::Verified)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ApprovalStep {
    id: ApprovalId,
    role: ApprovalRole,
    approver_id: Option<UserId>,
    status: ApprovalStatus,
    requested_at: Option<Timestamp>,
    approved_at: Option<Timestamp>,
    approved_by_id: Option<UserId>,
}

impl ApprovalStep {
    fn new(
        role: ApprovalRole,
        approver_id: Option<UserId>,
        status: ApprovalStatus,
        requested_at: Option<Timestamp>,
    ) -> Self {
        Self {
            id: ApprovalId::new(),
            role,
            approver_id,
            status,
            requested_at,
            approved_at: None,
            approved_by_id: None,
        }
    }

    #[must_use]
    pub const fn id(&self) -> ApprovalId {
        self.id
    }

    #[must_use]
    pub const fn role(&self) -> ApprovalRole {
        self.role
    }

    #[must_use]
    pub const fn approver_id(&self) -> Option<UserId> {
        self.approver_id
    }

    #[must_use]
    pub const fn status(&self) -> ApprovalStatus {
        self.status
    }

    #[must_use]
    pub const fn requested_at(&self) -> Option<Timestamp> {
        self.requested_at
    }

    #[must_use]
    pub const fn approved_at(&self) -> Option<Timestamp> {
        self.approved_at
    }

    #[must_use]
    pub const fn approved_by_id(&self) -> Option<UserId> {
        self.approved_by_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ApprovalLine {
    mechanic: ApprovalStep,
    admin: ApprovalStep,
    executive: ApprovalStep,
}

impl ApprovalLine {
    #[must_use]
    pub fn new(
        mechanic_id: UserId,
        admin_id: Option<UserId>,
        executive_id: Option<UserId>,
        requested_at: Timestamp,
    ) -> Self {
        Self {
            mechanic: ApprovalStep::new(
                ApprovalRole::Mechanic,
                Some(mechanic_id),
                ApprovalStatus::Pending,
                Some(requested_at),
            ),
            admin: ApprovalStep::new(
                ApprovalRole::Admin,
                admin_id,
                ApprovalStatus::NotStarted,
                None,
            ),
            executive: ApprovalStep::new(
                ApprovalRole::Executive,
                executive_id,
                ApprovalStatus::NotStarted,
                None,
            ),
        }
    }

    #[must_use]
    pub const fn step(&self, role: ApprovalRole) -> &ApprovalStep {
        match role {
            ApprovalRole::Mechanic => &self.mechanic,
            ApprovalRole::Admin => &self.admin,
            ApprovalRole::Executive => &self.executive,
        }
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.admin.status == ApprovalStatus::Approved
            && self.executive.status == ApprovalStatus::Approved
    }

    #[must_use]
    pub fn next_pending_non_mechanic_role(&self) -> Option<ApprovalRole> {
        if self.admin.status == ApprovalStatus::Pending {
            Some(ApprovalRole::Admin)
        } else if self.executive.status == ApprovalStatus::Pending {
            Some(ApprovalRole::Executive)
        } else {
            None
        }
    }

    pub fn auto_approve_mechanic(&mut self, at: Timestamp) {
        self.mechanic.status = ApprovalStatus::Approved;
        self.mechanic.approved_at = Some(at);
        self.mechanic.approved_by_id = self.mechanic.approver_id;
        self.unlock();
    }

    pub fn approve(
        &mut self,
        role: ApprovalRole,
        actor_id: UserId,
        at: Timestamp,
    ) -> Result<(), KernelError> {
        if role == ApprovalRole::Mechanic {
            return Err(KernelError::conflict(
                "mechanic approval is automatic when the report is submitted",
            ));
        }

        let step = self.step_mut(role);
        if step.status != ApprovalStatus::Pending {
            return Err(KernelError::conflict(format!(
                "{role:?} approval step is not pending"
            )));
        }
        if let Some(approver_id) = step.approver_id
            && approver_id != actor_id
        {
            return Err(KernelError::forbidden(
                "actor is not assigned to the pending approval step",
            ));
        }
        step.status = ApprovalStatus::Approved;
        step.approved_at = Some(at);
        step.approved_by_id = Some(actor_id);
        self.unlock();
        Ok(())
    }

    fn step_mut(&mut self, role: ApprovalRole) -> &mut ApprovalStep {
        match role {
            ApprovalRole::Mechanic => &mut self.mechanic,
            ApprovalRole::Admin => &mut self.admin,
            ApprovalRole::Executive => &mut self.executive,
        }
    }

    fn unlock(&mut self) {
        if self.mechanic.status == ApprovalStatus::Approved
            && self.admin.status == ApprovalStatus::NotStarted
        {
            self.admin.status = ApprovalStatus::Pending;
            self.admin.requested_at = self.mechanic.approved_at;
        }
        if self.admin.status == ApprovalStatus::Approved
            && self.executive.status == ApprovalStatus::NotStarted
        {
            self.executive.status = ApprovalStatus::Pending;
            self.executive.requested_at = self.admin.approved_at;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkOrderAssignment {
    id: AssignmentId,
    mechanic_id: UserId,
    role: AssignmentRole,
}

impl WorkOrderAssignment {
    #[must_use]
    pub fn new(mechanic_id: UserId, role: AssignmentRole) -> Self {
        Self {
            id: AssignmentId::new(),
            mechanic_id,
            role,
        }
    }

    #[must_use]
    pub const fn id(&self) -> AssignmentId {
        self.id
    }

    #[must_use]
    pub const fn mechanic_id(&self) -> UserId {
        self.mechanic_id
    }

    #[must_use]
    pub const fn role(&self) -> AssignmentRole {
        self.role
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WorkOrderAssignments {
    assignments: Vec<WorkOrderAssignment>,
}

impl WorkOrderAssignments {
    pub fn new(assignments: Vec<WorkOrderAssignment>) -> Result<Self, KernelError> {
        if assignments.is_empty() {
            return Err(KernelError::validation(
                "work order must have at least one assignee",
            ));
        }

        let primary_count = assignments
            .iter()
            .filter(|assignment| assignment.role == AssignmentRole::Primary)
            .count();
        if primary_count != 1 {
            return Err(KernelError::validation(
                "work order assignments must include exactly one primary assignee",
            ));
        }

        let mut seen = BTreeSet::new();
        for assignment in &assignments {
            if !seen.insert(assignment.mechanic_id) {
                return Err(KernelError::validation(
                    "work order assignments cannot contain duplicate mechanics",
                ));
            }
        }

        Ok(Self { assignments })
    }

    #[must_use]
    pub fn as_slice(&self) -> &[WorkOrderAssignment] {
        &self.assignments
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.assignments.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.assignments.is_empty()
    }

    #[must_use]
    pub fn primary(&self) -> &WorkOrderAssignment {
        self.assignments
            .iter()
            .find(|assignment| assignment.role == AssignmentRole::Primary)
            .unwrap_or(&self.assignments[0])
    }

    #[must_use]
    pub fn contains_mechanic(&self, mechanic_id: UserId) -> bool {
        self.assignments
            .iter()
            .any(|assignment| assignment.mechanic_id == mechanic_id)
    }
}

/// Work-order aggregate head for lifecycle operations.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WorkOrder {
    id: WorkOrderId,
    branch_id: BranchId,
    status: WorkOrderStatus,
    result_type: WorkResultType,
    assignments: WorkOrderAssignments,
    approval_line: ApprovalLine,
    requested_at: Timestamp,
    updated_at: Timestamp,
}

impl WorkOrder {
    #[must_use]
    pub fn new(
        id: WorkOrderId,
        branch_id: BranchId,
        assignments: WorkOrderAssignments,
        admin_approver_id: Option<UserId>,
        executive_approver_id: Option<UserId>,
        requested_at: Timestamp,
    ) -> Self {
        let primary_mechanic = assignments.primary().mechanic_id;
        Self {
            id,
            branch_id,
            status: WorkOrderStatus::Assigned,
            result_type: WorkResultType::Unknown,
            assignments,
            approval_line: ApprovalLine::new(
                primary_mechanic,
                admin_approver_id,
                executive_approver_id,
                requested_at,
            ),
            requested_at,
            updated_at: requested_at,
        }
    }

    #[must_use]
    pub const fn id(&self) -> WorkOrderId {
        self.id
    }

    #[must_use]
    pub const fn branch_id(&self) -> BranchId {
        self.branch_id
    }

    #[must_use]
    pub const fn status(&self) -> WorkOrderStatus {
        self.status
    }

    #[must_use]
    pub const fn result_type(&self) -> WorkResultType {
        self.result_type
    }

    #[must_use]
    pub const fn approval_line(&self) -> &ApprovalLine {
        &self.approval_line
    }

    #[must_use]
    pub const fn assignments(&self) -> &WorkOrderAssignments {
        &self.assignments
    }

    #[must_use]
    pub const fn requested_at(&self) -> Timestamp {
        self.requested_at
    }

    #[must_use]
    pub const fn updated_at(&self) -> Timestamp {
        self.updated_at
    }

    pub fn start(&mut self, at: Timestamp) -> Result<Transition<WorkOrderStatus>, KernelError> {
        self.apply_transition(
            WorkOrderStatus::InProgress,
            at,
            TransitionGuardContext::mechanic(),
        )
    }

    pub fn submit_report(
        &mut self,
        mechanic_id: UserId,
        result_type: WorkResultType,
        at: Timestamp,
    ) -> Result<Transition<WorkOrderStatus>, KernelError> {
        if !self.assignments.contains_mechanic(mechanic_id) {
            return Err(KernelError::forbidden(
                "only an assigned mechanic may submit a work report",
            ));
        }
        let transition = self.apply_transition(
            WorkOrderStatus::ReportSubmitted,
            at,
            TransitionGuardContext::mechanic(),
        )?;
        self.result_type = result_type;
        self.approval_line.auto_approve_mechanic(at);
        Ok(transition)
    }

    pub fn approve_next(
        &mut self,
        actor_id: UserId,
        at: Timestamp,
        evidence: &impl CompletionEvidenceInterlock,
    ) -> Result<Transition<WorkOrderStatus>, KernelError> {
        let role = self
            .approval_line
            .next_pending_non_mechanic_role()
            .ok_or_else(|| KernelError::conflict("no pending non-mechanic approval step"))?;

        let mut next_line = self.approval_line.clone();
        next_line.approve(role, actor_id, at)?;

        let to = match role {
            ApprovalRole::Admin => WorkOrderStatus::AdminReview,
            ApprovalRole::Executive if self.result_type == WorkResultType::Completed => {
                WorkOrderStatus::FinalCompleted
            }
            ApprovalRole::Executive => WorkOrderStatus::TemporaryAction,
            ApprovalRole::Mechanic => WorkOrderStatus::ReportSubmitted,
        };

        let completion_evidence_verified = if to == WorkOrderStatus::FinalCompleted {
            evidence.final_completion_evidence_verified(self.id)?
        } else {
            true
        };
        let context = TransitionGuardContext {
            actor: TransitionActor::Admin,
            approval_line_complete: next_line.is_complete(),
            completion_evidence_verified,
        };
        let transition = self.apply_transition(to, at, context)?;
        self.approval_line = next_line;
        Ok(transition)
    }

    pub fn transition_to(
        &mut self,
        to: WorkOrderStatus,
        evidence: &impl CompletionEvidenceInterlock,
    ) -> Result<Transition<WorkOrderStatus>, KernelError> {
        let completion_evidence_verified = if to == WorkOrderStatus::FinalCompleted {
            evidence.final_completion_evidence_verified(self.id)?
        } else {
            true
        };
        let context = TransitionGuardContext {
            actor: TransitionActor::Admin,
            approval_line_complete: self.approval_line.is_complete(),
            completion_evidence_verified,
        };
        self.apply_transition(to, self.updated_at, context)
    }

    fn apply_transition(
        &mut self,
        to: WorkOrderStatus,
        at: Timestamp,
        context: TransitionGuardContext,
    ) -> Result<Transition<WorkOrderStatus>, KernelError> {
        let transition = validate_status_transition(self.status, to, context)?;
        self.status = to;
        self.updated_at = at;
        Ok(transition)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_order_status_wire_roundtrips_and_rejects_unknown() {
        for status in ALL_WORK_ORDER_STATUSES {
            assert_eq!(
                WorkOrderStatus::from_db_str(status.as_db_str()).unwrap(),
                *status
            );
        }
        assert_eq!(WorkOrderStatus::InProgress.as_db_str(), "IN_PROGRESS");
        assert_eq!(
            WorkOrderStatus::from_db_str("REPORT_SUBMITTED").unwrap(),
            WorkOrderStatus::ReportSubmitted
        );
        // Fail-closed: lowercase / wrong tags must not parse.
        assert!(WorkOrderStatus::from_db_str("received").is_err());
        assert!(WorkOrderStatus::from_db_str("in_progress").is_err());
        assert!(WorkOrderStatus::from_db_str("BROADCASTING").is_err());
        assert!(WorkOrderStatus::from_db_str("").is_err());
    }

    #[test]
    fn priority_level_wire_roundtrips_and_rejects_unknown() {
        for priority in [
            PriorityLevel::P1,
            PriorityLevel::P2,
            PriorityLevel::P3,
            PriorityLevel::Outsource,
            PriorityLevel::Unset,
        ] {
            assert_eq!(
                PriorityLevel::from_db_str(priority.as_db_str()).unwrap(),
                priority
            );
        }
        assert_eq!(PriorityLevel::Outsource.as_db_str(), "OUTSOURCE");
        assert!(PriorityLevel::from_db_str("p1").is_err());
        assert!(PriorityLevel::from_db_str("outsource").is_err());
        assert!(PriorityLevel::from_db_str("P4").is_err());
        assert!(PriorityLevel::from_db_str("").is_err());
    }

    #[test]
    fn maintenance_type_wire_roundtrips_and_rejects_unknown() {
        for kind in [
            MaintenanceType::Emergency,
            MaintenanceType::Corrective,
            MaintenanceType::Preventive,
            MaintenanceType::Inspection,
        ] {
            assert_eq!(
                MaintenanceType::from_db_str(kind.as_db_str()).unwrap(),
                kind
            );
        }
        assert_eq!(MaintenanceType::Emergency.as_db_str(), "EMERGENCY");
        assert!(MaintenanceType::from_db_str("emergency").is_err());
        assert!(MaintenanceType::from_db_str("CORRECTIVE_MAINTENANCE").is_err());
        assert!(MaintenanceType::from_db_str("").is_err());
    }

    #[test]
    fn maintenance_cause_wire_roundtrips_and_rejects_unknown() {
        for cause in [
            MaintenanceCause::Breakdown,
            MaintenanceCause::ReturnPrep,
            MaintenanceCause::Scheduled,
            MaintenanceCause::InspectionFinding,
            MaintenanceCause::Other,
        ] {
            assert_eq!(
                MaintenanceCause::from_db_str(cause.as_db_str()).unwrap(),
                cause
            );
        }
        assert_eq!(MaintenanceCause::ReturnPrep.as_db_str(), "RETURN_PREP");
        assert!(MaintenanceCause::from_db_str("breakdown").is_err());
        assert!(MaintenanceCause::from_db_str("return_prep").is_err());
        assert!(MaintenanceCause::from_db_str("UNKNOWN_CAUSE").is_err());
        assert!(MaintenanceCause::from_db_str("").is_err());
    }

    #[test]
    fn settlement_status_wire_roundtrips_and_rejects_unknown() {
        for status in [
            SettlementStatus::Draft,
            SettlementStatus::Submitted,
            SettlementStatus::Approved,
            SettlementStatus::Void,
        ] {
            assert_eq!(
                SettlementStatus::from_db_str(status.as_db_str()).unwrap(),
                status
            );
        }
        assert_eq!(SettlementStatus::Submitted.as_db_str(), "SUBMITTED");
        assert!(SettlementStatus::from_db_str("draft").is_err());
        assert!(SettlementStatus::from_db_str("submitted").is_err());
        assert!(SettlementStatus::from_db_str("PAID").is_err());
        assert!(SettlementStatus::from_db_str("").is_err());
    }

    #[test]
    fn settlement_line_kind_wire_roundtrips_and_rejects_unknown() {
        for kind in [
            SettlementLineKind::Labor,
            SettlementLineKind::Part,
            SettlementLineKind::Outsource,
            SettlementLineKind::Other,
        ] {
            assert_eq!(
                SettlementLineKind::from_db_str(kind.as_db_str()).unwrap(),
                kind
            );
        }
        assert_eq!(SettlementLineKind::Labor.as_db_str(), "LABOR");
        assert!(SettlementLineKind::from_db_str("labor").is_err());
        assert!(SettlementLineKind::from_db_str("part").is_err());
        assert!(SettlementLineKind::from_db_str("MATERIAL").is_err());
        assert!(SettlementLineKind::from_db_str("").is_err());
    }

    #[test]
    fn settlement_eligible_work_order_statuses_are_report_review_and_final() {
        assert_eq!(SETTLEMENT_ELIGIBLE_WORK_ORDER_STATUSES.len(), 3);
        assert_eq!(
            SETTLEMENT_ELIGIBLE_WORK_ORDER_STATUSES,
            &[
                WorkOrderStatus::ReportSubmitted,
                WorkOrderStatus::AdminReview,
                WorkOrderStatus::FinalCompleted,
            ]
        );
        assert!(!SETTLEMENT_ELIGIBLE_WORK_ORDER_STATUSES.contains(&WorkOrderStatus::InProgress));
        assert!(!SETTLEMENT_ELIGIBLE_WORK_ORDER_STATUSES.contains(&WorkOrderStatus::Assigned));
    }

    #[test]
    fn validate_status_transition_rejects_illegal_edges_fail_closed() {
        // Open path with no special fixtures: pure table lookup fail-closed.
        let open = TransitionGuardContext::mechanic();
        assert!(
            validate_status_transition(
                WorkOrderStatus::Received,
                WorkOrderStatus::FinalCompleted,
                open
            )
            .is_err()
        );
        assert!(
            validate_status_transition(
                WorkOrderStatus::Archived,
                WorkOrderStatus::InProgress,
                open
            )
            .is_err()
        );
        // Legal open edge still accepts under mechanic context.
        assert!(
            validate_status_transition(WorkOrderStatus::Received, WorkOrderStatus::Assigned, open)
                .is_ok()
        );
    }
}
