//! Notifications application contracts.
//!
//! Adapters implement these use-case shapes. Two ports live here:
//!   * [`NotificationSink`] — the WRITE port other domains call to emit a
//!     notification. Producers depend on this trait, never on the Postgres
//!     adapter, so the dependency arrow points inward.
//!   * [`NotificationNotifier`] — the post-commit realtime port carrying only
//!     IDs, per ADR-0007. The Postgres/realtime layer implements it; the store
//!     calls it after the row is durably committed.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::future::Future;
use std::pin::Pin;

use console_kernel_core::{
    AuditAction, AuditEvent, KernelError, NotificationId, Timestamp, TraceContext, UserId,
};
use console_notifications_domain::{
    NotificationLink, NotificationPolicyId, NotificationPolicyScope,
};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Realtime notifier port (post-commit, IDs only)
// ---------------------------------------------------------------------------

pub type NotificationNotifyFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationCreatedNotification {
    pub notification_id: NotificationId,
    pub recipient_user_id: UserId,
}

pub trait NotificationNotifier: Send + Sync {
    fn notification_created(
        &self,
        notification: NotificationCreatedNotification,
    ) -> NotificationNotifyFuture<'_>;
}

// ---------------------------------------------------------------------------
// Write port other domains call to emit notifications
// ---------------------------------------------------------------------------

pub type EmitNotificationFuture<'a> =
    Pin<Box<dyn Future<Output = Result<NotificationSummary, KernelError>> + Send + 'a>>;

/// The write port. A producer (e.g. the workflow compensation drain) holds an
/// `Arc<dyn NotificationSink>` and calls [`NotificationSink::emit`] to create a
/// recipient-scoped notification row. `emit` is idempotent-friendly: producers
/// that need at-most-once delivery pass a stable `dedup_key`.
pub trait NotificationSink: Send + Sync {
    fn emit(&self, command: EmitNotificationCommand) -> EmitNotificationFuture<'_>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitNotificationCommand {
    /// The user whose action caused the notification, recorded on the audit
    /// event. `None` for system-emitted notifications (e.g. a scheduled job).
    pub actor: Option<UserId>,
    pub recipient: UserId,
    pub category: String,
    /// Behavioral kind (see `NotificationKind`), e.g. `"info"` or
    /// `"slo_violation"`. A resolvable kind is auto-resolved by a later
    /// [`ResolveNotificationsByLinkCommand`] matching this notification's
    /// `link`.
    pub kind: String,
    pub text: String,
    pub link: NotificationLink,
    /// Optional stable key for at-most-once emission. When set, a second emit
    /// with the same `(recipient, dedup_key)` is a no-op returning the existing
    /// row, so an at-least-once outbox drain never doubles a notification.
    pub dedup_key: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

// ---------------------------------------------------------------------------
// Recipient-scoped read/mutation shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationQuery {
    /// Bound from the authenticated principal, never from request input.
    pub recipient: UserId,
    pub notification_id: NotificationId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListNotificationsQuery {
    /// Bound from the authenticated principal, never from request input.
    pub recipient: UserId,
    pub unread_only: bool,
    /// Keyset cursor: return rows strictly older than this notification.
    pub before_id: Option<NotificationId>,
    pub limit: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnreadNotificationCountQuery {
    /// Bound from the authenticated principal, never from request input.
    pub recipient: UserId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationCountsSummaryQuery {
    /// Bound from the authenticated principal, never from request input.
    pub recipient: UserId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkNotificationReadCommand {
    pub recipient: UserId,
    pub notification_id: NotificationId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Swipe/secondary-action read TOGGLE, the reverse arc of
/// [`MarkNotificationReadCommand`]. Never clears `read_at` — the FIRST read
/// timestamp stays as forensic truth; only the attention flag flips back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkNotificationUnreadCommand {
    pub recipient: UserId,
    pub notification_id: NotificationId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkAllNotificationsReadCommand {
    pub recipient: UserId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationSummary {
    pub id: NotificationId,
    pub recipient_user_id: UserId,
    pub category: String,
    pub kind: String,
    /// Recipient-facing text (the `notifs.text` field in the logic inventory;
    /// stored in the `body` column).
    pub text: String,
    pub link: NotificationLink,
    pub unread: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
    #[serde(with = "time::serde::rfc3339::option")]
    pub read_at: Option<Timestamp>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub resolved_at: Option<Timestamp>,
    /// Whether the CALLER's mute policies suppress this row's attention
    /// (badge counts, realtime). Computed per-row at read time — never stored,
    /// never filters the list. Additive: serde-defaults false for existing
    /// payloads and producers.
    #[serde(default)]
    pub muted: bool,
}

/// Per-category unread breakdown for the comms-rail badge (`kind`/`category`
/// double as the "surface" grouping — a new producer category needs no schema
/// change to show up here).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationCategoryCount {
    pub category: String,
    pub unread: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationCountsSummary {
    /// Unread rows that WANT attention: mute-suppressed rows are excluded here
    /// and in `by_category` (badge truth = attention truth).
    pub total_unread: i64,
    pub by_category: Vec<NotificationCategoryCount>,
    /// Unread rows suppressed by the caller's mute policies — surfaced so the
    /// UI can show a truthful "숨김 N" instead of silently losing data.
    #[serde(default)]
    pub muted_unread: i64,
}

// ---------------------------------------------------------------------------
// Aggregate-by-object read path (개체별 view)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListNotificationObjectGroupsQuery {
    /// Bound from the authenticated principal, never from request input.
    pub recipient: UserId,
    /// Only groups that still hold at least one unread row.
    pub unread_only: bool,
    /// Opaque keyset cursor from a previous page's `next_cursor`; the caller
    /// never interprets it. An undecodable or foreign cursor yields an empty
    /// page (fail-closed), matching the flat list's cursor semantics.
    pub before: Option<String>,
    pub limit: i64,
}

/// One source object with every notification pointing at it rolled up:
/// totals, per-category unread breakdown, the latest row for preview, and the
/// caller's object-level mute state (the bell toggle).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationObjectGroup {
    pub link: NotificationLink,
    pub total: i64,
    /// Unread rows in the group. Like the flat list this is NOT mute-filtered —
    /// grouping annotates, only badges exclude.
    pub unread: i64,
    /// Per-category unread breakdown within this group (categories with zero
    /// unread are omitted).
    pub categories: Vec<NotificationCategoryCount>,
    pub latest: NotificationSummary,
    /// Whether THIS OBJECT is muted for the caller: an `all`-scope or a
    /// matching `object`-scope policy. Deliberately excludes `category`-scope
    /// policies — the group bell toggles an object policy, and a category
    /// policy folded in here could never be un-toggled from the bell.
    /// Category muting still shows on the individual rows' `muted` flag.
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationObjectGroupPage {
    pub items: Vec<NotificationObjectGroup>,
    /// Opaque cursor for the next page; `None` at the end.
    pub next_cursor: Option<String>,
}

// ---------------------------------------------------------------------------
// Per-user routing policies (mute; `action` extensible to watch later)
// ---------------------------------------------------------------------------

/// PUT upsert of a mute policy. Direct-apply personal setting (§3.9.0-①) —
/// no approval lifecycle — but every set is audited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertNotificationPolicyCommand {
    pub recipient: UserId,
    pub scope: NotificationPolicyScope,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// DELETE = unmute. Cross-user ids are NotFound, indistinguishable from
/// absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteNotificationPolicyCommand {
    pub recipient: UserId,
    pub policy_id: NotificationPolicyId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListNotificationPoliciesQuery {
    /// Bound from the authenticated principal, never from request input.
    pub recipient: UserId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPolicySummary {
    pub id: NotificationPolicyId,
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<NotificationLink>,
    pub action: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPolicyList {
    pub items: Vec<NotificationPolicySummary>,
}

// ---------------------------------------------------------------------------
// Generic detect -> assign -> resolve chain
// ---------------------------------------------------------------------------

/// Mark every still-open notification pointing at `link` as resolved, in one
/// audited sweep. A producer calls this with the SAME [`NotificationLink`]
/// shape it originally emitted (e.g. `Object { kind: "attendance_gap", id }`)
/// when the resolving domain event fires (e.g. 대근 편성 covers a 미편성 결원
/// breach) — generic to any producer, never hardcoded to one domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveNotificationsByLinkCommand {
    pub link: NotificationLink,
    /// The actor/event that resolved it. `None` for a system-driven auto-close.
    pub resolved_by: Option<UserId>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

pub type ResolveNotificationsFuture<'a> =
    Pin<Box<dyn Future<Output = Result<u64, KernelError>> + Send + 'a>>;

pub trait NotificationResolver: Send + Sync {
    /// Returns the number of notifications resolved.
    fn resolve_by_link(
        &self,
        command: ResolveNotificationsByLinkCommand,
    ) -> ResolveNotificationsFuture<'_>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPage {
    pub items: Vec<NotificationSummary>,
    /// Cursor for the next page (oldest id on this page); `None` at the end.
    pub next_cursor: Option<NotificationId>,
}

/// Build the audit event for a recipient self-action or a producer emission.
/// `target_id` is the notification id (or the recipient id for the batch
/// read-all action, which has no single target).
pub fn notification_audit_event(
    action: &str,
    actor: Option<UserId>,
    target_id: impl ToString,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        "notification",
        target_id.to_string(),
        trace,
        occurred_at,
    ))
}
