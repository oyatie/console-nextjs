//! Finance GL application layer: voucher commands, summaries, and the audit-event
//! builder. Persistence and HTTP concerns live in the outer crates.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_finance_gl_domain::{DebitCredit, VoucherId, VoucherStatus};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, KernelError, Timestamp, TraceContext, UserId,
};
use serde::{Deserialize, Serialize};

/// One 차/대 line the caller wants on a draft voucher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoucherLineInput {
    pub account_code: String,
    pub side: DebitCredit,
    pub amount_won: i64,
    #[serde(default)]
    pub memo: String,
}

/// A logical reference to the source 기안/document a voucher was derived from
/// (승인 → 전표 파생 chain): an object type + id, resolvable in any domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoucherSourceRef {
    pub object_type: String,
    pub object_id: String,
}

/// Open a fresh draft voucher (기표). `source` is set when the draft was derived
/// from an approved expense-class 기안; `None` for a hand-keyed voucher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateVoucherDraftCommand {
    pub actor: UserId,
    pub branch_id: BranchId,
    pub memo: String,
    pub source: Option<VoucherSourceRef>,
    pub lines: Vec<VoucherLineInput>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Derive a draft voucher from an approved source 기안 (지출결의/전표/구매). The
/// approval chain calls this with the source object ref and the projected 차/대
/// totals from the structured 증빙 lines. Distinct from
/// [`CreateVoucherDraftCommand`] only in that the source is REQUIRED here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateVoucherDraftFromSourceCommand {
    pub actor: UserId,
    pub branch_id: BranchId,
    pub memo: String,
    pub source: VoucherSourceRef,
    pub projected_lines: Vec<VoucherLineInput>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// A pure FSM step (submit/approve/post) on an existing voucher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoucherTransitionCommand {
    pub actor: UserId,
    pub voucher_id: VoucherId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Reverse a posted voucher — creates a linked contra voucher; the memo explains
/// why (역분개 사유).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReverseVoucherCommand {
    pub actor: UserId,
    pub voucher_id: VoucherId,
    pub memo: String,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoucherLineSummary {
    pub id: VoucherLineIdWire,
    pub line_no: i32,
    pub account_code: String,
    pub side: DebitCredit,
    pub amount_won: i64,
    pub memo: String,
}

/// The wire form of a line id (kept as a plain uuid on summaries so REST clients
/// see a bare string).
pub type VoucherLineIdWire = uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoucherSummary {
    pub id: VoucherId,
    pub voucher_no: String,
    pub branch_id: BranchId,
    /// The branch's display name, resolved via a same-org correlated lookup on
    /// `branches` (RLS-scoped, mirrors support's `assignee_name` pattern).
    /// `None` only if the branch row itself is gone, which FK RESTRICT on
    /// `finance_gl_vouchers.branch_id` never allows in practice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_name: Option<String>,
    pub status: VoucherStatus,
    pub memo: String,
    pub source_object_type: Option<String>,
    pub source_object_id: Option<String>,
    pub reversal_of_voucher_id: Option<VoucherId>,
    pub reversed_by_voucher_id: Option<VoucherId>,
    pub debit_total_won: i64,
    pub credit_total_won: i64,
    pub lines: Vec<VoucherLineSummary>,
    pub created_by: UserId,
    /// `created_by`'s display name, resolved via a same-org correlated lookup
    /// on `users`. `None` only for a deleted user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by_name: Option<String>,
    /// The distinct principal who approved the voucher at 승인 (separation of
    /// duties: always `!= created_by`). `None` until approved.
    pub approved_by: Option<UserId>,
    /// `approved_by`'s display name (same resolution as `created_by_name`).
    /// `None` until approved or for a deleted approver.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub posted_at: Option<Timestamp>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: Timestamp,
}

/// One account-drill row: a single voucher line for an account, carrying its
/// voucher + source-object linkage so a reviewer can drill voucher → source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDrillEntry {
    pub voucher_id: VoucherId,
    pub voucher_no: String,
    pub status: VoucherStatus,
    pub line_id: VoucherLineIdWire,
    pub account_code: String,
    pub side: DebitCredit,
    pub amount_won: i64,
    pub source_object_type: Option<String>,
    pub source_object_id: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub entry_at: Timestamp,
}

/// Build a branch-scoped audit event for a voucher mutation.
pub fn voucher_audit_event(
    action: &str,
    actor: UserId,
    branch_id: BranchId,
    voucher_id: VoucherId,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new(action)?,
        "finance_gl_voucher",
        voucher_id.to_string(),
        trace,
        occurred_at,
    )
    .with_branch(branch_id))
}
