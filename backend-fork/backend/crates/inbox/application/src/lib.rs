//! InboxDoc application contracts.
//!
//! Adapters implement these use-case shapes. The one port that lives here is
//! [`InboxDocSink`] — the WRITE port other domains call to deliver a document
//! into a recipient's vault (leave-promotion lane D will consume it; a payslip
//! backfill hook is the deferred producer). Producers depend on this trait,
//! never on the Postgres adapter, so the dependency arrow points inward.
//!
//! Passkey step-up verification for receipt confirmation is a platform concern
//! and is enforced in the REST layer (mirroring workflow-studio), not here.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::future::Future;
use std::pin::Pin;

use console_inbox_domain::{InboxDocKind, NewInboxDoc};
use console_kernel_core::{
    AuditAction, AuditEvent, InboxDocId, KernelError, Timestamp, TraceContext, UserId,
};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Write port other domains call to deliver documents
// ---------------------------------------------------------------------------

pub type EmitInboxDocFuture<'a> =
    Pin<Box<dyn Future<Output = Result<InboxDocSummary, KernelError>> + Send + 'a>>;

/// The delivery port. A producer (leave-promotion, payroll backfill) holds an
/// `Arc<dyn InboxDocSink>` and calls [`InboxDocSink::emit`] to place a
/// recipient-scoped document in the vault. `emit` is idempotent-friendly:
/// producers that need at-most-once delivery pass a stable `dedup_key`.
pub trait InboxDocSink: Send + Sync {
    fn emit(&self, command: EmitInboxDocCommand) -> EmitInboxDocFuture<'_>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitInboxDocCommand {
    /// Who caused the delivery (recorded on the audit event). `None` for
    /// system-emitted documents (e.g. a scheduled payroll backfill).
    pub actor: Option<UserId>,
    /// Bound by the producer from the target's identity, never from end-user
    /// request input.
    pub recipient: UserId,
    pub doc: NewInboxDoc,
    /// Optional stable key for at-most-once emission. A second emit with the
    /// same `(recipient, dedup_key)` is a no-op returning the existing row.
    pub dedup_key: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

// ---------------------------------------------------------------------------
// Recipient-scoped read/mutation shapes
// ---------------------------------------------------------------------------

/// List filters, mirroring the inbox screen pill row
/// (확인 필요 / 급여명세 / 완료 / 전체).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxDocFilter {
    /// 확인 필요 — legal notices awaiting receipt confirmation.
    ActionRequired,
    /// 급여명세 — payslips.
    Payslip,
    /// 완료 — confirmed documents.
    Done,
    /// 전체 — everything.
    All,
}

impl InboxDocFilter {
    pub fn parse(value: Option<&str>) -> Result<Self, KernelError> {
        match value.unwrap_or("all") {
            "action" | "action_required" => Ok(Self::ActionRequired),
            "pay" | "payslip" => Ok(Self::Payslip),
            "done" => Ok(Self::Done),
            "all" => Ok(Self::All),
            other => Err(KernelError::validation(format!(
                "unknown inbox filter: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListInboxDocsQuery {
    /// Bound from the authenticated principal, never from request input.
    pub recipient: UserId,
    pub filter: InboxDocFilter,
    /// Keyset cursor: return documents strictly older than this one.
    pub before_id: Option<InboxDocId>,
    pub limit: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GetInboxDocQuery {
    /// Bound from the authenticated principal, never from request input.
    pub recipient: UserId,
    pub id: InboxDocId,
}

/// Confirm receipt of a legal notice. This mutation is the legal receipt
/// evidence. The REST layer verifies a fresh passkey step-up before calling it;
/// the `recipient` is always bound from the authenticated principal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmReceiptCommand {
    pub recipient: UserId,
    pub doc_id: InboxDocId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

/// List-row view. Never carries `payload` — the list is metadata only, so a
/// locked legal notice's body never reaches the wire before receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxDocSummary {
    pub id: InboxDocId,
    pub recipient_user_id: UserId,
    pub kind: InboxDocKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice_type: Option<String>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legal_basis: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// `true` while a legal notice awaits receipt confirmation — its body is
    /// withheld until unlocked. Always `false` for payslips.
    pub locked: bool,
    pub confirmed_by: Option<UserId>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub confirmed_at: Option<Timestamp>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
}

/// Single-document view. `payload` is present only when the document is
/// readable (a payslip, or an already-confirmed legal notice); it is `None`
/// while a legal notice is locked, so reading a locked doc never discloses its
/// body and never auto-confirms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxDocDetail {
    #[serde(flatten)]
    pub summary: InboxDocSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxDocPage {
    pub items: Vec<InboxDocSummary>,
    /// Cursor for the next page (oldest id on this page); `None` at the end.
    pub next_cursor: Option<InboxDocId>,
}

/// Build the audit event for an emission or a receipt confirmation.
/// `target_id` is the inbox document id. The `receipt/self` semantics of the
/// prototype are carried by the action name (`inbox_doc.confirm_receipt`) plus
/// the before→after snapshots the adapter attaches.
pub fn inbox_doc_audit_event(
    action: &str,
    actor: Option<UserId>,
    target_id: InboxDocId,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        "inbox_doc",
        target_id.to_string(),
        trace,
        occurred_at,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_parses_aliases_and_defaults_to_all() {
        assert_eq!(InboxDocFilter::parse(None).unwrap(), InboxDocFilter::All);
        assert_eq!(
            InboxDocFilter::parse(Some("action")).unwrap(),
            InboxDocFilter::ActionRequired
        );
        assert_eq!(
            InboxDocFilter::parse(Some("pay")).unwrap(),
            InboxDocFilter::Payslip
        );
        assert_eq!(
            InboxDocFilter::parse(Some("done")).unwrap(),
            InboxDocFilter::Done
        );
        assert!(InboxDocFilter::parse(Some("bogus")).is_err());
    }
}
