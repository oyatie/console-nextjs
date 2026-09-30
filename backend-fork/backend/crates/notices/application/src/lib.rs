//! Notice-board application contracts.
//!
//! Adapters implement these use-case shapes. A notice moves draft ->
//! published; publishing is the only mutation an author's draft-write port
//! doesn't cover — it is gated separately (publish-tier authz) at the REST
//! layer and fans out one notification per snapshotted recipient.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, KernelError, NoticeId, Timestamp, TraceContext, UserId,
};
use serde::{Deserialize, Serialize};

/// Raw audience input as it arrives from REST; the domain
/// (`NoticeAudience::new`) validates scope/branch_ids coherence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeAudienceInput {
    pub scope: String,
    #[serde(default)]
    pub branch_ids: Vec<BranchId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDraftNoticeCommand {
    pub author: UserId,
    pub title: String,
    pub body: String,
    /// `None` = `general` (DB default).
    pub category: Option<String>,
    /// `None` = org-wide (DB default).
    pub audience: Option<NoticeAudienceInput>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Draft-only edit (§3.9.0-③): every field optional, audience replaced
/// whole. The adapter rejects the edit with Conflict once published —
/// published notices are frozen (their receipts are the record).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateDraftNoticeCommand {
    pub notice_id: NoticeId,
    pub editor: UserId,
    pub title: Option<String>,
    pub body: Option<String>,
    pub category: Option<String>,
    pub audience: Option<NoticeAudienceInput>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishNoticeCommand {
    pub notice_id: NoticeId,
    pub publisher: UserId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GetNoticeQuery {
    pub notice_id: NoticeId,
    /// Whose `my_receipt` to hydrate — always the authenticated principal.
    pub viewer: UserId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListNoticesQuery {
    /// Drafts are visible only to publish-tier callers; REST resolves this
    /// from the principal's feature grant before the query reaches the store.
    /// The same grant gates per-row `progress` hydration.
    pub include_drafts: bool,
    pub limit: i64,
    /// Whose `my_receipt` to hydrate — always the authenticated principal.
    pub viewer: UserId,
}

/// 수령확인 (receipt acknowledgment): a recipient confirms they have seen a
/// published notice. Owner-scoped like notifications — the recipient is
/// always bound from the authenticated principal, never from request input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcknowledgeNoticeCommand {
    pub notice_id: NoticeId,
    pub recipient: UserId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoticeProgressQuery {
    pub notice_id: NoticeId,
}

/// Manager-only receipts drill: who is (still) outstanding on a notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListNoticeReceiptsQuery {
    pub notice_id: NoticeId,
    /// `Some(false)` = the outstanding chase list; `None` = everyone.
    pub acknowledged: Option<bool>,
    pub limit: i64,
    pub offset: i64,
}

/// One audience branch, hydrated with its display name (대상 column).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeAudienceBranch {
    pub id: BranchId,
    pub name: String,
}

/// The caller's own receipt state on a notice; `None` on the summary means
/// the caller is not a snapshotted recipient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeReceiptState {
    #[serde(with = "time::serde::rfc3339::option")]
    pub acknowledged_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeSummary {
    pub id: NoticeId,
    pub code: Option<String>,
    pub author_user_id: UserId,
    pub title: String,
    pub body: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub published_at: Option<Timestamp>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
    pub category: String,
    pub audience_scope: String,
    /// Empty for org-wide notices.
    pub audience_branches: Vec<NoticeAudienceBranch>,
    /// `None` = the caller is not a recipient of this notice.
    pub my_receipt: Option<NoticeReceiptState>,
    /// Hydrated only for NoticeManage callers; `None` otherwise.
    pub progress: Option<NoticeProgress>,
}

/// 수령확인 progress (done/total), matching the console board's generic
/// progress-bar contract (see `docs/design/oyatie-console` TODO 2026-07-08).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeProgress {
    pub total: i64,
    pub acknowledged: i64,
}

/// One recipient row in the manager receipts drill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeReceipt {
    pub recipient_user_id: UserId,
    pub display_name: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub acknowledged_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeReceiptPage {
    pub items: Vec<NoticeReceipt>,
    /// Total matching rows (before limit/offset), for pager truth.
    pub total: i64,
}

/// Build the audit event for a notice mutation.
pub fn notice_audit_event(
    action: &str,
    actor: Option<UserId>,
    target_id: impl ToString,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        "notice",
        target_id.to_string(),
        trace,
        occurred_at,
    ))
}
