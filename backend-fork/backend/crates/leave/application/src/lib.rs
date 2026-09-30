//! Leave-request application contracts.
//!
//! Command/query shapes for the branch-scoped approval queue, the decide
//! mutation (with the leave-ledger write-back on approve), the balance roster,
//! and the §61 statutory push. Every scoping field that identifies the actor or
//! the tenant is bound by the REST layer from the authenticated principal, never
//! from request input. Branch scope is passed through from the resolved
//! [`BranchScope`](BranchScope) so the queue only ever
//! narrows to the caller's branches.
//!
//! The statutory push is a producer of two side effects it does not own: it
//! delivers a receipt-gated document into the target's 개인 수신함 (via the inbox
//! crate's `InboxDocSink`) and — once the 연차촉진 submittable definition exists —
//! starts an engine AP- run. Those integrations live in the REST/adapter layer;
//! this crate only defines the command shapes and views.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::{future::Future, pin::Pin};

use console_kernel_core::{
    AuditAction, AuditEvent, BranchScope, Date, KernelError, LeavePromotionId, LeaveRequestId,
    OrgId, Timestamp, TraceContext, UserId,
};
use console_leave_domain::{
    LeaveBalanceAmount, LeaveChargeAssessment, LeaveChargeResolutionOrigin,
    LeaveChargeReviewReason, LeaveChargeState, LeaveDateCharge, LeaveDecision, LeaveStatus,
    LeaveType, LeaveUnits, NewLeaveRequest, PartialDayPeriod, PromotionKind, PromotionTrack,
    SourceRevisionRef, TimeChangeGroundsCode,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Create (crate-level write port — no public REST route)
// ---------------------------------------------------------------------------

/// Create a pending leave request. Not a public REST endpoint: requests are
/// produced by the 기안/engine compose flow (submittable-templates, gap #1) or a
/// roster import, which call this port with the mapping they own. Routing is
/// resolved from the active employee's explicit home branch; the
/// `subject_employee_id` is still checked against the caller's linked employee.
#[derive(Debug, Clone)]
pub struct CreateLeaveRequestCommand {
    pub requester_user_id: UserId,
    pub subject_employee_id: Uuid,
    /// Stable client submission id. The database binds it to a canonical
    /// request-intent digest: same key + same payload replays the original;
    /// same key + different payload is a conflict.
    pub idempotency_key: Uuid,
    pub request: NewLeaveRequest,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Apply an exact imported/initial balance snapshot through the isolated leave
/// command capability. `expected_updated_at` is the employee-row CAS token;
/// `idempotency_key` is source-scoped and payload-bound by the database.
#[derive(Debug, Clone)]
pub struct ImportEmployeeLeaveBalanceCommand {
    pub employee_id: Uuid,
    pub expected_updated_at: Timestamp,
    pub accrued: Option<LeaveBalanceAmount>,
    pub used: Option<LeaveBalanceAmount>,
    pub remaining: Option<LeaveBalanceAmount>,
    pub source_kind: String,
    pub source_ref: String,
    pub idempotency_key: String,
    pub actor: UserId,
    pub trace: TraceContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportEmployeeLeaveBalanceResult {
    pub employee_id: Uuid,
    pub updated_at: Timestamp,
    pub changed: bool,
    pub replayed: bool,
}

/// Self-service history. `requester` is always bound from the authenticated
/// principal; the adapter also requires the same linked employee identity.
#[derive(Debug, Clone)]
pub struct ListSelfLeaveRequestsQuery {
    pub requester: UserId,
    pub limit: i64,
    /// Last request returned by the previous page. The adapter resolves its
    /// stable `(created_at, id)` coordinates inside the caller's self scope.
    pub cursor: Option<LeaveRequestId>,
}

/// Trusted input to the work-calendar/policy seam. The organization, branch,
/// and employee are resolved server-side and never accepted from request JSON.
#[derive(Debug, Clone)]
pub struct ResolveLeaveChargeQuery {
    pub org_id: OrgId,
    pub branch_id: Uuid,
    pub subject_employee_id: Uuid,
    pub leave_type: LeaveType,
    pub start_date: Date,
    pub end_date: Date,
    pub partial_day_period: Option<PartialDayPeriod>,
    pub as_of: Timestamp,
}

pub type LeaveChargeFuture<'a> =
    Pin<Box<dyn Future<Output = Result<LeaveChargeAssessment, KernelError>> + Send + 'a>>;

/// Portable calendar/policy port. Self-hosted and cloud-specific adapters must
/// return evidence or an explicit review reason; guessing is not permitted.
pub trait WorkCalendarPort: Send + Sync {
    fn resolve_charge(&self, query: ResolveLeaveChargeQuery) -> LeaveChargeFuture<'_>;
}

// ---------------------------------------------------------------------------
// Queue read
// ---------------------------------------------------------------------------

/// List the branch-scoped approval queue. Pending-first, newest-first.
#[derive(Debug, Clone)]
pub struct ListLeaveRequestsQuery {
    /// Resolved from the principal; passed opaquely to the data layer which
    /// narrows the query to these branches (deny-by-omission on empty scope).
    pub branch_scope: BranchScope,
    /// When set, only requests in this status; otherwise every status,
    /// including the read-only historical `returned`/`rejected` states.
    pub status: Option<LeaveStatus>,
    pub limit: i64,
    /// Last request returned by the previous page. The adapter resolves the
    /// complete queue sort key before fetching rows strictly after it.
    pub cursor: Option<LeaveRequestId>,
}

// ---------------------------------------------------------------------------
// Decide
// ---------------------------------------------------------------------------

/// Decide a pending leave request. The `decider` is bound from the principal;
/// the adapter enforces SoD (the decider must not be the request's requester)
/// and that only a `pending` request can be decided. On `approve`, the leave
/// ledger write-back (used += exact units, remaining -= exact units)
/// happens in the SAME audited transaction as the status change.
#[derive(Debug, Clone)]
pub struct DecideLeaveRequestCommand {
    pub request_id: LeaveRequestId,
    pub decider: UserId,
    pub branch_scope: BranchScope,
    /// Optimistic precondition for the mutable request workflow row. This is
    /// compared with `LeaveRequestView::request_version`, never with the
    /// immutable charge-evidence revision. `None` preserves the deployed v1
    /// first-writer-wins contract; version-aware callers must send `Some`.
    pub expected_version: Option<i64>,
    pub decision: LeaveDecision,
    pub comment: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Worker-only alternate 시기 proposal on a `time_change_consult` row (§60⑤ /
/// charter §4-31: 대체 일자는 근로자 선택). The adapter binds the actor as the
/// original requester and refuses employer/manager writes.
#[derive(Debug, Clone)]
pub struct ProposeAlternateDatesCommand {
    pub request_id: LeaveRequestId,
    pub proposer: UserId,
    pub start_date: Date,
    pub end_date: Date,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Record a server-validated manual/reference resolution. The caller supplies
/// per-date evidence and source identity, never a total or digest; the adapter
/// canonicalizes, totals, hashes, and persists the immutable snapshot.
#[derive(Debug, Clone)]
pub struct ResolveLeaveChargeCommand {
    pub request_id: LeaveRequestId,
    pub resolver: UserId,
    pub branch_scope: BranchScope,
    /// Optimistic precondition for the mutable request workflow row. Resolving
    /// evidence advances `request_version` and creates a new, independently
    /// monotonic `charge_version` snapshot.
    pub expected_version: i64,
    pub date_charges: Vec<LeaveDateCharge>,
    pub calendar_revision_ref: SourceRevisionRef,
    pub policy_revision_ref: SourceRevisionRef,
    pub supporting_source_refs: Vec<SourceRevisionRef>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

// ---------------------------------------------------------------------------
// Statutory push (촉진 / 노무수령거부)
// ---------------------------------------------------------------------------

/// Serve a §61 statutory push to a target employee. The adapter records the
/// push, delivers the receipt-gated notice into the target's 개인 수신함, and
/// (when a submittable definition exists) starts the engine AP- run. `round`
/// applies to a promotion (1|2); a refusal follows round 2. `target_user_id` is
/// the inbox recipient (the target employee's linked account).
///
/// The unused-day count is deliberately **absent**: §61①1 requires the notice
/// to state the employee's own 미사용 휴가 일수, so the adapter reads it from
/// the authoritative roster and refuses the push when the roster has not
/// established it. A caller-supplied figure would be unverifiable data on a
/// legal notice.
#[derive(Debug, Clone)]
pub struct StatutoryPushCommand {
    pub actor: UserId,
    pub branch_id: Uuid,
    pub target_user_id: UserId,
    pub target_employee_id: Uuid,
    pub target_name: String,
    pub kind: PromotionKind,
    pub round: i16,
    /// Which §61 window set applies — the paragraphs have different periods.
    pub track: PromotionTrack,
    /// 제60조제7항 본문에 따른 기간의 마지막 날 (§61② tracks: 최초 1년의 근로기간
    /// 종료일). The statutory windows are all counted back from this date, so
    /// it is required evidence, never inferred.
    pub leave_period_end: Date,
    /// The 사용 시기 a round-2 notice designates (§61①2 / §61②2). Required and
    /// non-empty for round 2; must be empty for round 1 and for a refusal,
    /// which reads the recorded round-2 designation instead of restating it.
    pub designated_dates: Vec<Date>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

/// One row of the approval queue (결재함 leave variant).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaveRequestView {
    pub id: LeaveRequestId,
    pub branch_id: Uuid,
    pub requester_user_id: UserId,
    pub subject_employee_id: Uuid,
    pub leave_type: LeaveType,
    /// Non-null v1 compatibility projection only. Authoritative writes use
    /// `charge_units`; legacy clients may continue decoding this field.
    pub days: f64,
    pub charge_units: Option<LeaveUnits>,
    pub charge_state: LeaveChargeState,
    pub charge_review_reasons: Vec<LeaveChargeReviewReason>,
    /// Mutable request/workflow CAS token. Clients submit this value as
    /// `expected_version` on resolve and decide commands.
    pub request_version: i64,
    /// Monotonic immutable charge-evidence revision counter. It advances only
    /// when a new evidence snapshot is recorded, remains unchanged by request
    /// decisions, and is never accepted as a request mutation precondition.
    pub charge_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charge_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charge_resolved_by: Option<UserId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charge_resolution_origin: Option<LeaveChargeResolutionOrigin>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_day_period: Option<PartialDayPeriod>,
    #[serde(with = "date_fmt")]
    pub start_date: Date,
    #[serde(with = "date_fmt")]
    pub end_date: Date,
    /// 근로기준법 §60 guardrail (charter §4-31): 연차 has no 사유란. Always
    /// `None` for requests filed after the guardrail; `Some` only on
    /// historical rows that were filed while the field was (unlawfully)
    /// mandatory. Serialized only when present so new rows expose no reason
    /// key at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub status: LeaveStatus,
    pub decided_by: Option<UserId>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_comment: Option<String>,
    /// System-judged §60⑤ grounds code when status is `time_change_consult`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_change_grounds: Option<TimeChangeGroundsCode>,
    /// Coverage evidence snapshot that justified the grounds (JSON object).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_change_evidence: Option<serde_json::Value>,
    /// Worker-chosen alternate 시기; absent until the requester proposes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "date_fmt_option"
    )]
    pub alternate_start_date: Option<Date>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "date_fmt_option"
    )]
    pub alternate_end_date: Option<Date>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alternate_partial_day_period: Option<PartialDayPeriod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub alternate_proposed_at: Option<Timestamp>,
    /// The engine AP- run started for a statutory push, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ap_run_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaveChargeResolutionView {
    pub request_id: LeaveRequestId,
    /// New mutable request/workflow CAS token after recording this resolution.
    pub request_version: i64,
    pub charge_units: LeaveUnits,
    pub charge_state: LeaveChargeState,
    /// Immutable revision assigned to this charge-evidence snapshot.
    pub charge_version: i64,
    pub server_digest: String,
    pub resolution_origin: LeaveChargeResolutionOrigin,
    pub resolved_by: Option<UserId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaveRequestPage {
    pub items: Vec<LeaveRequestView>,
    /// Opaque id cursor for the next stable keyset page, or `None` when the
    /// current page exhausted the matching result set.
    pub next_cursor: Option<LeaveRequestId>,
}

/// Closed set for the balance roster's urgency/promotion bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaveBalanceTone {
    /// Normal balance.
    Ok,
    /// Mostly unused balance that should be promoted for statutory use.
    Promote,
    /// Nearly exhausted balance.
    Low,
}

/// One employee's balance row (직원별 연차 현황). `left` is derived here so the
/// client never recomputes it. `tone` mirrors the prototype's ok/promote/low
/// bucketing so the bar color and the 촉진 flag come from the backend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaveBalanceView {
    pub employee_id: Uuid,
    pub name: String,
    pub team: Option<String>,
    pub grant: f64,
    pub used: f64,
    pub left: f64,
    /// `low` (mostly used), `promote` (mostly unused → 촉진 대상), or `ok`.
    pub tone: LeaveBalanceTone,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaveBalancePage {
    pub items: Vec<LeaveBalanceView>,
}

/// Exact self-service balance projection. `None` means the source roster has
/// not established that figure; zero is an explicit, materially different value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfLeaveFilingState {
    Ready,
    HomeBranchRequired,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelfLeaveBalanceView {
    pub employee_id: Uuid,
    pub name: String,
    pub accrued_units: Option<LeaveBalanceAmount>,
    pub used_units: Option<LeaveBalanceAmount>,
    pub remaining_units: Option<LeaveBalanceAmount>,
    /// Filing is ready only when the trusted employee identity has an active
    /// home branch. History and balances remain readable in either state.
    pub filing_state: SelfLeaveFilingState,
    pub home_branch_id: Option<Uuid>,
}

/// Closed set for the engine-submission state attached to a statutory push.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApSubmission {
    /// The engine AP- run was started.
    Submitted,
    /// The push is recorded but waiting for the engine submittable definition.
    PendingEngineDefinition,
}

/// The result of a statutory push. `inbox_doc_id` is the receipt-gated document
/// delivered to the target (always present — the concrete legal delivery).
/// `ap_run_id` is the engine submission; `None` until the 연차촉진 submittable
/// definition exists (gap #1), in which case `ap_submission` explains the state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatutoryPushView {
    pub id: LeavePromotionId,
    pub kind: PromotionKind,
    pub round: i16,
    pub target_user_id: UserId,
    pub inbox_doc_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ap_run_id: Option<Uuid>,
    /// `submitted` when an engine run was started, else `pending_engine_definition`.
    pub ap_submission: ApSubmission,
}

// ---------------------------------------------------------------------------
// Audit
// ---------------------------------------------------------------------------

/// Build a leave-request audit event (`leave_request.*`). The adapter attaches
/// org + before/after snapshots.
pub fn leave_request_audit_event(
    action: &str,
    actor: Option<UserId>,
    target_id: LeaveRequestId,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        "leave_request",
        target_id.to_string(),
        trace,
        occurred_at,
    ))
}

/// Build a statutory-push audit event (`leave_promotion.*`).
pub fn leave_promotion_audit_event(
    action: &str,
    actor: Option<UserId>,
    target_id: LeavePromotionId,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        "leave_promotion",
        target_id.to_string(),
        trace,
        occurred_at,
    ))
}

/// `time::Date` wire format (`YYYY-MM-DD`), shared by the request views.
mod date_fmt {
    use console_kernel_core::Date;
    use serde::{self, Deserialize, Deserializer, Serializer};
    use time::format_description::well_known::Iso8601;

    pub fn serialize<S: Serializer>(date: &Date, ser: S) -> Result<S::Ok, S::Error> {
        let s = date
            .format(&Iso8601::DATE)
            .map_err(serde::ser::Error::custom)?;
        ser.serialize_str(&s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Date, D::Error> {
        let s = String::deserialize(de)?;
        Date::parse(&s, &Iso8601::DATE).map_err(serde::de::Error::custom)
    }
}

mod date_fmt_option {
    use console_kernel_core::Date;
    use serde::{self, Deserialize, Deserializer, Serializer};
    use time::format_description::well_known::Iso8601;

    pub fn serialize<S: Serializer>(date: &Option<Date>, ser: S) -> Result<S::Ok, S::Error> {
        match date {
            Some(date) => {
                let s = date
                    .format(&Iso8601::DATE)
                    .map_err(serde::ser::Error::custom)?;
                ser.serialize_some(&s)
            }
            None => ser.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Option<Date>, D::Error> {
        let Some(s) = Option::<String>::deserialize(de)? else {
            return Ok(None);
        };
        Date::parse(&s, &Iso8601::DATE)
            .map(Some)
            .map_err(serde::de::Error::custom)
    }
}
