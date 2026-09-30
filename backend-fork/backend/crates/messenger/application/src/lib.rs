//! Messenger application contracts.
//!
//! Adapters implement these use-case shapes. Realtime is represented only as a
//! post-commit notification port carrying IDs, per ADR-0007.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::future::Future;
use std::pin::Pin;

use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, EvidenceId, KernelError, MessageId, ThreadId,
    Timestamp, TraceContext, UserId, WorkOrderId,
};
use console_messenger_domain::{PresenceStatus, ThreadKind, ThreadVisibility};
use serde::{Deserialize, Serialize};

pub type MessageNotifyFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessagePostedNotification {
    pub message_id: MessageId,
    pub thread_id: ThreadId,
    pub branch_id: BranchId,
}

/// Post-commit realtime signal that a message's ack set changed. Carries IDs
/// only (like [`MessagePostedNotification`]); the listener re-reads the live
/// count before fan-out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageAckNotification {
    pub message_id: MessageId,
    pub thread_id: ThreadId,
    pub branch_id: BranchId,
}

pub trait MessageNotifier: Send + Sync {
    fn message_posted(&self, notification: MessagePostedNotification) -> MessageNotifyFuture<'_>;

    /// Publish that a message's ack count changed so subscribed thread members
    /// see the count chip update live. Defaults to a no-op so notifier doubles
    /// (tests, non-realtime deployments) need not implement it.
    fn message_ack_toggled(&self, notification: MessageAckNotification) -> MessageNotifyFuture<'_> {
        let _ = notification;
        Box::pin(async {})
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateThreadCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub branch_id: BranchId,
    pub kind: ThreadKind,
    /// `None` = derive from `kind`/title ([`ThreadVisibility::default_for`]).
    pub visibility: Option<ThreadVisibility>,
    pub title: Option<String>,
    pub work_order_id: Option<WorkOrderId>,
    pub member_ids: Vec<UserId>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Join an existing `channel`-visibility thread the caller can see in scope.
/// A `direct` thread is not joinable (its member set is fixed at creation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinThreadCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub thread_id: ThreadId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Discover joinable channels within the caller's branch scope, whether or not
/// the caller is already a member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListChannelsQuery {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub limit: i64,
}

/// Toggle the caller's ack on a message (idempotent insert/delete).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToggleAckCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub message_id: MessageId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckSummary {
    pub message_id: MessageId,
    pub thread_id: ThreadId,
    /// Whether the caller's ack is now present (post-toggle state).
    pub acked: bool,
    pub ack_count: i64,
}

/// Direct-save personal per-thread mute (DESIGN §3.9.0 whitelist ①).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetThreadMuteCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub thread_id: ThreadId,
    pub muted: bool,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMuteSummary {
    pub thread_id: ThreadId,
    pub muted: bool,
}

/// Presence of every member of a thread the caller belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadPresenceQuery {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub thread_id: ThreadId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberPresence {
    pub user_id: UserId,
    pub display_name: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_activity_at: Option<Timestamp>,
    pub status: PresenceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnsureWorkOrderThreadCommand {
    pub actor: UserId,
    pub branch_id: BranchId,
    pub work_order_id: WorkOrderId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessageCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub thread_id: ThreadId,
    pub body: String,
    pub attachment_evidence_ids: Vec<EvidenceId>,
    /// Optional reply-quote target. Must be a message in the SAME thread; a
    /// cross-thread quote is rejected in the send path.
    pub quoted_message_id: Option<MessageId>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkThreadReadCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub thread_id: ThreadId,
    pub last_read_message_id: MessageId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListThreadsQuery {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub limit: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListMembersQuery {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub branch_id: BranchId,
    pub limit: i64,
}

/// Fetch one branch-scoped member's summary for a person pin panel
/// (UI-M2a AC). Unlike the admin-gated `/api/v1/users/{id}`, this reads the
/// same non-admin branch directory as `list_members`, so any employee can open
/// a coworker's card. Viewing someone else records a `person.view` audit event
/// (DESIGN §4.7 "열람 — 기록 남음"); a self-view records none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberProfileQuery {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub branch_id: BranchId,
    pub user_id: UserId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageQuery {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub message_id: MessageId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessagePageQuery {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub thread_id: ThreadId,
    pub before_message_id: Option<MessageId>,
    pub limit: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchMessagesQuery {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub query: String,
    pub limit: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSummary {
    pub id: ThreadId,
    pub kind: ThreadKind,
    pub visibility: ThreadVisibility,
    pub branch_id: BranchId,
    pub title: Option<String>,
    pub work_order_id: Option<WorkOrderId>,
    /// Whether the caller has muted this thread. A muted thread is dropped from
    /// the client's unread badge total and suppresses this user's mention
    /// notifications.
    pub muted: bool,
    pub last_message_id: Option<MessageId>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_message_at: Option<Timestamp>,
    pub member_count: i64,
    pub unread_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberSummary {
    pub id: UserId,
    pub display_name: String,
    pub team: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSummary {
    pub id: MessageId,
    pub thread_id: ThreadId,
    pub branch_id: BranchId,
    pub sender_id: UserId,
    /// Display name of the sender, resolved via a LEFT JOIN on `users`. `None`
    /// when the sender row no longer exists (e.g. a hard-deleted account); the
    /// web falls back through `safeLabel` so a missing name never leaks a UUID.
    pub sender_name: Option<String>,
    pub body: String,
    pub attachment_evidence_ids: Vec<EvidenceId>,
    /// Number of non-sender thread members whose read receipt has reached this
    /// message. This is derived from thread-level receipts; no per-message rows.
    pub read_count: i64,
    /// Number of non-sender thread members who are expected to read this message.
    pub read_target_count: i64,
    /// Number of members who have acked ("확인") this message.
    pub ack_count: i64,
    /// Whether the reading actor has acked this message. Always `false` on a
    /// freshly-posted realtime event (no one has acked yet).
    pub acked_by_me: bool,
    /// Reply-quote target, when this message quotes an earlier one in the thread.
    pub quoted_message_id: Option<MessageId>,
    /// A short preview of the quoted message, resolved via a same-thread join.
    /// `None` when nothing is quoted (or the quoted message was deleted).
    pub quoted_body: Option<String>,
    pub quoted_sender_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub sent_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessagePage {
    pub items: Vec<MessageSummary>,
    pub next_cursor: Option<MessageId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadReceiptSummary {
    pub thread_id: ThreadId,
    pub user_id: UserId,
    pub last_read_message_id: MessageId,
    #[serde(with = "time::serde::rfc3339")]
    pub read_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: Timestamp,
}

pub fn messenger_audit_event(
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

#[cfg(test)]
mod tests {
    use super::ReadReceiptSummary;
    use console_kernel_core::{MessageId, ThreadId, UserId};

    #[test]
    fn read_receipt_serializes_fractional_timestamps_as_rfc3339_strings() {
        let timestamp = time::OffsetDateTime::from_unix_timestamp(0)
            .unwrap()
            .replace_nanosecond(123_456_789)
            .unwrap();
        let receipt = ReadReceiptSummary {
            thread_id: ThreadId::from_uuid(uuid::Uuid::nil()),
            user_id: UserId::from_uuid(uuid::Uuid::nil()),
            last_read_message_id: MessageId::from_uuid(uuid::Uuid::nil()),
            read_at: timestamp,
            updated_at: timestamp,
        };

        let json = serde_json::to_value(receipt).unwrap();
        for pointer in ["/read_at", "/updated_at"] {
            assert_eq!(
                json.pointer(pointer).and_then(serde_json::Value::as_str),
                Some("1970-01-01T00:00:00.123456789Z"),
                "{pointer} must honor the OpenAPI string/date-time contract"
            );
        }
    }
}
