//! Inspection application layer.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_inspection_domain::{
    InspectionCycle, InspectionRoundOutcome, InspectionScheduleStatus,
};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, EquipmentId, InspectionRoundId,
    InspectionScheduleId, KernelError, Timestamp, TraceContext, UserId,
};
use serde::{Deserialize, Serialize};
use time::Date;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInspectionScheduleCommand {
    pub actor: UserId,
    pub branch_id: BranchId,
    pub equipment_id: EquipmentId,
    pub mechanic_id: UserId,
    pub cycle: InspectionCycle,
    pub interval_days: i32,
    pub due_date: Date,
    pub note: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompleteInspectionRoundCommand {
    pub actor: UserId,
    pub schedule_id: InspectionScheduleId,
    pub outcome: InspectionRoundOutcome,
    pub completed_at: Timestamp,
    pub findings: String,
    pub note: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListInspectionSchedulesQuery {
    pub branch_scope: BranchScope,
    pub due_start: Date,
    pub due_end: Date,
    /// Page size. The adapter clamps to a hard cap so an unbounded fetch over a
    /// wide date range is impossible even when the client sends no limit.
    pub limit: i64,
    /// Zero-based row offset into the date-range ordering for offset pagination.
    pub offset: i64,
}

/// A mechanic's own branch-scoped schedules. The caller identity is part of
/// the query rather than a client-controlled filter, so the adapter cannot
/// accidentally return a colleague's assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListMyInspectionSchedulesQuery {
    pub branch_scope: BranchScope,
    pub mechanic_id: UserId,
    pub due_start: Date,
    pub due_end: Date,
    pub limit: i64,
    pub offset: i64,
}

/// One page of inspection schedules plus the unpaged `total` for the matching
/// date range, so the console can show an honest count and page beyond the cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectionSchedulePage {
    pub items: Vec<InspectionScheduleSummary>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectionScheduleSummary {
    pub id: InspectionScheduleId,
    pub branch_id: BranchId,
    pub equipment_id: EquipmentId,
    pub mechanic_id: UserId,
    /// Assigned mechanic's display name, resolved via a same-org LEFT JOIN on
    /// `users`. `None` when the mechanic account no longer exists; the web
    /// renders it through `safeLabel` so a missing name never leaks the UUID.
    pub mechanic_display_name: Option<String>,
    pub cycle: InspectionCycle,
    pub interval_days: i32,
    pub due_date: Date,
    pub status: InspectionScheduleStatus,
    #[serde(with = "time::serde::rfc3339::option")]
    pub completed_at: Option<Timestamp>,
    pub note: Option<String>,
    pub site_name: String,
    pub management_no: Option<String>,
    pub model: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectionRoundSummary {
    pub id: InspectionRoundId,
    pub schedule_id: InspectionScheduleId,
    pub branch_id: BranchId,
    pub equipment_id: EquipmentId,
    pub mechanic_id: UserId,
    pub completed_by: UserId,
    pub outcome: InspectionRoundOutcome,
    pub findings: String,
    pub note: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub completed_at: Timestamp,
}

pub fn inspection_audit_event(
    action: &str,
    actor: UserId,
    branch_id: BranchId,
    target_type: &str,
    target_id: impl ToString,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new(action)?,
        target_type,
        target_id.to_string(),
        trace,
        occurred_at,
    )
    .with_branch(branch_id))
}
