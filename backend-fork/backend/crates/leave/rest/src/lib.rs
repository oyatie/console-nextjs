//! Leave-request REST API (연차 결재 + §61 촉진).
//!
//! Reads are gated on `employee_directory_read`, mutations on
//! `employee_directory_manage` — the branch HR/manager tier. The tenant (org)
//! is always bound from the authenticated principal; the approval queue and
//! decide path are additionally *branch*-scoped from the principal's resolved
//! [`BranchScope`], so an approver only sees and acts on their own branches.
//! The statutory push validates its target `branch_id` against the actor's
//! scope via [`authorize`] — a branch outside the actor's scope is rejected,
//! never trusted from input.
//!
//! Separation of duties: the decide path forbids a requester from deciding
//! their own request (enforced in the adapter + a DB CHECK), mirroring the
//! workflow-engine initiator guard (#205).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_kernel_core::{
    BranchScope, Date, ErrorKind, KernelError, LeaveRequestId, Timestamp, TraceContext, UserId,
};
use console_leave_adapter_postgres::{PgLeaveError, PgLeaveStore};
use console_leave_application::{
    CreateLeaveRequestCommand, DecideLeaveRequestCommand, LeaveRequestPage, LeaveRequestView,
    ListLeaveRequestsQuery, ListSelfLeaveRequestsQuery, ProposeAlternateDatesCommand,
    ResolveLeaveChargeCommand, SelfLeaveBalanceView, StatutoryPushCommand,
};
use console_leave_domain::{
    LeaveDateCharge, LeaveDecision, LeaveStatus, LeaveType, LeaveUnits, NewLeaveRequest,
    NonWorkBasis, PartialDayPeriod, PromotionKind, PromotionTrack, SourceRevisionRef,
    WorkObligation,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{
    Action, Feature, Principal, authorize, authorize_capability, authorize_org_wide,
};
use console_platform_request_context::RequestContextError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

time::serde::format_description!(iso_date, Date, "[year]-[month]-[day]");

pub const LEAVE_REQUESTS_PATH: &str = "/api/v1/leave/requests";
pub const LEAVE_DECIDE_PATH_TEMPLATE: &str = "/api/v1/leave/requests/{id}/decide";
pub const LEAVE_REQUESTS_V2_PATH: &str = "/api/v2/leave/requests";
pub const LEAVE_DECIDE_V2_PATH_TEMPLATE: &str = "/api/v2/leave/requests/{id}/decide";
pub const LEAVE_CHARGE_RESOLUTION_V2_PATH_TEMPLATE: &str =
    "/api/v2/leave/requests/{id}/charge-resolution";
pub const LEAVE_ALTERNATE_DATES_V2_PATH_TEMPLATE: &str =
    "/api/v2/leave/requests/{id}/alternate-dates";
pub const MY_LEAVE_V2_PATH: &str = "/api/v2/me/leave";
pub const LEAVE_BALANCES_PATH: &str = "/api/v1/leave/balances";
pub const LEAVE_PROMOTIONS_PATH: &str = "/api/v1/leave/promotions";
pub const LEAVE_REFUSAL_NOTICES_PATH: &str = "/api/v1/leave/refusal-notices";

pub const LEAVE_ROUTE_PATHS: &[&str] = &[
    LEAVE_REQUESTS_PATH,
    LEAVE_DECIDE_PATH_TEMPLATE,
    LEAVE_REQUESTS_V2_PATH,
    LEAVE_DECIDE_V2_PATH_TEMPLATE,
    LEAVE_CHARGE_RESOLUTION_V2_PATH_TEMPLATE,
    LEAVE_ALTERNATE_DATES_V2_PATH_TEMPLATE,
    MY_LEAVE_V2_PATH,
    LEAVE_BALANCES_PATH,
    LEAVE_PROMOTIONS_PATH,
    LEAVE_REFUSAL_NOTICES_PATH,
];

#[derive(Clone)]
pub struct LeaveRestState {
    store: PgLeaveStore,
    jwt_verifier: Option<JwtVerifier>,
}

impl std::fmt::Debug for LeaveRestState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeaveRestState")
            .field("has_jwt_verifier", &self.jwt_verifier.is_some())
            .finish()
    }
}

impl LeaveRestState {
    #[must_use]
    pub fn new(store: PgLeaveStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
        }
    }
}

pub fn router(state: LeaveRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(LEAVE_REQUESTS_PATH, get(list_requests_v1))
        .route(LEAVE_DECIDE_PATH_TEMPLATE, post(decide_v1))
        .route(
            LEAVE_REQUESTS_V2_PATH,
            get(list_requests_v2).post(create_request_v2),
        )
        .route(LEAVE_DECIDE_V2_PATH_TEMPLATE, post(decide_v2))
        .route(
            LEAVE_CHARGE_RESOLUTION_V2_PATH_TEMPLATE,
            post(resolve_charge),
        )
        .route(
            LEAVE_ALTERNATE_DATES_V2_PATH_TEMPLATE,
            post(propose_alternate_dates_v2),
        )
        .route(MY_LEAVE_V2_PATH, get(get_my_leave_v2))
        .route(LEAVE_BALANCES_PATH, get(list_balances))
        .route(LEAVE_PROMOTIONS_PATH, post(push_promotion))
        .route(LEAVE_REFUSAL_NOTICES_PATH, post(push_refusal))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
struct ListParams {
    status: Option<String>,
    limit: Option<i64>,
    cursor: Option<LeaveRequestId>,
}

/// Frozen v1 response DTO. Deployed Kotlin clients reject unknown JSON keys,
/// so the exact pre-v2 field set must remain stable even though the internal
/// application view has grown exact-charge and CAS metadata. `reason` stays in
/// the key set for that reason only; requests filed after the 근로기준법 §60
/// guardrail have none and project as an empty string.
#[derive(Debug, Serialize)]
struct LegacyLeaveRequestView {
    id: LeaveRequestId,
    branch_id: Uuid,
    requester_user_id: UserId,
    subject_employee_id: Uuid,
    leave_type: LeaveType,
    days: f64,
    #[serde(with = "iso_date")]
    start_date: Date,
    #[serde(with = "iso_date")]
    end_date: Date,
    reason: String,
    status: LeaveStatus,
    decided_by: Option<UserId>,
    #[serde(with = "time::serde::rfc3339::option")]
    decided_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision_comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ap_run_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: Timestamp,
}

impl From<LeaveRequestView> for LegacyLeaveRequestView {
    fn from(value: LeaveRequestView) -> Self {
        Self {
            id: value.id,
            branch_id: value.branch_id,
            requester_user_id: value.requester_user_id,
            subject_employee_id: value.subject_employee_id,
            leave_type: value.leave_type,
            days: value.days,
            start_date: value.start_date,
            end_date: value.end_date,
            reason: value.reason.unwrap_or_default(),
            status: value.status,
            decided_by: value.decided_by,
            decided_at: value.decided_at,
            decision_comment: value.decision_comment,
            ap_run_id: value.ap_run_id,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
struct LegacyLeaveRequestPage {
    items: Vec<LegacyLeaveRequestView>,
}

impl From<LeaveRequestPage> for LegacyLeaveRequestPage {
    fn from(value: LeaveRequestPage) -> Self {
        Self {
            items: value.items.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct MyLeaveOverview {
    balance: SelfLeaveBalanceView,
    requests: LeaveRequestPage,
}

async fn get_my_leave_v2(
    State(state): State<LeaveRestState>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    // Deliberately no employee-directory feature gate. Both reads bind the
    // caller's user id to the linked employee on the server.
    let balance = state
        .store
        .get_self_balance(principal.user_id)
        .await
        .map_err(RestError::from_store)?;
    let requests = state
        .store
        .list_self_requests(ListSelfLeaveRequestsQuery {
            requester: principal.user_id,
            limit: params.limit.unwrap_or(100),
            cursor: params.cursor,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(MyLeaveOverview { balance, requests }).into_response())
}

async fn load_requests(
    state: &LeaveRestState,
    headers: &HeaderMap,
    params: ListParams,
) -> Result<LeaveRequestPage, RestError> {
    let principal = principal_from_headers(state, headers).await?;
    require_read(&principal)?;
    let status = match params.status.as_deref() {
        Some(s) => Some(LeaveStatus::parse(s).map_err(RestError::from_kernel)?),
        None => None,
    };
    state
        .store
        .list_requests(ListLeaveRequestsQuery {
            branch_scope: principal.branch_scope,
            status,
            limit: params.limit.unwrap_or(100),
            cursor: params.cursor,
        })
        .await
        .map_err(RestError::from_store)
}

async fn list_requests_v1(
    State(state): State<LeaveRestState>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
) -> Result<Response, RestError> {
    let page = load_requests(&state, &headers, params).await?;
    Ok(Json(LegacyLeaveRequestPage::from(page)).into_response())
}

async fn list_requests_v2(
    State(state): State<LeaveRestState>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
) -> Result<Response, RestError> {
    let page = load_requests(&state, &headers, params).await?;
    Ok(Json(page).into_response())
}

/// 근로기준법 §60 guardrail (charter §4-31): there is deliberately **no
/// `reason` field** — 연차 requires no 사유, and `deny_unknown_fields` turns a
/// client that still sends one into a 4xx instead of silently storing it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequestBody {
    /// Stable submission id generated once by modern clients and reused for a
    /// retry after an unknown/lost response. Optional for deployed v1 clients.
    idempotency_key: Uuid,
    /// `annual` (full-day intent) or `half_day` (policy-resolved partial-day intent).
    leave_type: String,
    /// Required exactly for `half_day`; absent for `annual`.
    #[serde(default)]
    partial_day_period: Option<String>,
    /// `YYYY-MM-DD`. Half-day intent must target a single date.
    start_date: String,
    end_date: String,
}

fn parse_iso_date(value: &str, field: &'static str) -> Result<Date, RestError> {
    Date::parse(value, &time::format_description::well_known::Iso8601::DATE).map_err(|_| {
        RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            format!("{field} must be a YYYY-MM-DD date"),
        )
    })
}

/// Self-service 연차/반차 신청 (POST /api/v2/leave/requests). The caller files a
/// request for THEMSELVES: `subject_employee_id` and the routing `branch_id` are
/// resolved server-side from the caller's own account (`users.employee_id` +
/// `employees.home_branch_id`), never from input, so a caller can only ever file for their
/// own employee record. No directory feature is required — filing your own leave
/// is a base employee capability; the gate is the employee link itself (an
/// unlinked or inactive account is 403, deny-by-omission). The created request is `pending`
/// and moves no ledger until a *separate* approver decides it (SoD).
async fn create_request_v2(
    State(state): State<LeaveRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateRequestBody>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let (subject_employee_id, _) = state
        .store
        .resolve_self_filing_context(principal.user_id)
        .await
        .map_err(RestError::from_store)?;

    let leave_type = LeaveType::parse(&body.leave_type).map_err(RestError::from_kernel)?;
    let start_date = parse_iso_date(&body.start_date, "start_date")?;
    let end_date = parse_iso_date(&body.end_date, "end_date")?;
    let partial_day_period = body
        .partial_day_period
        .as_deref()
        .map(PartialDayPeriod::parse)
        .transpose()
        .map_err(RestError::from_kernel)?;
    let request = NewLeaveRequest::new(leave_type, start_date, end_date, partial_day_period)
        .map_err(RestError::from_kernel)?;

    let view = state
        .store
        .create_request(CreateLeaveRequestCommand {
            requester_user_id: principal.user_id,
            subject_employee_id,
            idempotency_key: body.idempotency_key,
            request,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(view)).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecideRequestV1 {
    decision: String,
    #[serde(default)]
    comment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecideRequestV2 {
    /// Mutable request/workflow CAS token from the v2 view. This is not the
    /// independently monotonic immutable charge-evidence revision.
    expected_version: i64,
    decision: String,
    #[serde(default)]
    comment: Option<String>,
}

async fn perform_decide(
    state: &LeaveRestState,
    headers: &HeaderMap,
    id: LeaveRequestId,
    expected_version: Option<i64>,
    decision: &str,
    comment: Option<&str>,
) -> Result<LeaveRequestView, RestError> {
    let principal = principal_from_headers(state, headers).await?;
    // Role gate: the branch HR/manager tier. Which requests they can touch is
    // confined by branch_scope in the store; SoD is enforced there too.
    require_manage(&principal)?;
    let decision = LeaveDecision::parse(decision).map_err(RestError::from_kernel)?;
    let comment = console_leave_domain::validate_decision_comment(decision, comment)
        .map_err(RestError::from_kernel)?;
    let expected_version = expected_version
        .map(validate_expected_request_version)
        .transpose()?;
    state
        .store
        .decide(DecideLeaveRequestCommand {
            request_id: id,
            decider: principal.user_id,
            branch_scope: principal.branch_scope.clone(),
            expected_version,
            decision,
            comment,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)
}

async fn decide_v1(
    State(state): State<LeaveRestState>,
    headers: HeaderMap,
    Path(id): Path<LeaveRequestId>,
    Json(body): Json<DecideRequestV1>,
) -> Result<Response, RestError> {
    let view = perform_decide(
        &state,
        &headers,
        id,
        None,
        &body.decision,
        body.comment.as_deref(),
    )
    .await?;
    Ok(Json(LegacyLeaveRequestView::from(view)).into_response())
}

async fn decide_v2(
    State(state): State<LeaveRestState>,
    headers: HeaderMap,
    Path(id): Path<LeaveRequestId>,
    Json(body): Json<DecideRequestV2>,
) -> Result<Response, RestError> {
    let view = perform_decide(
        &state,
        &headers,
        id,
        Some(body.expected_version),
        &body.decision,
        body.comment.as_deref(),
    )
    .await?;
    Ok(Json(view).into_response())
}

/// Worker-chosen alternate 시기 on a §60⑤ `time_change_consult` (charter
/// §4-31). No manage capability — only the original requester may propose.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposeAlternateDatesBody {
    start_date: String,
    end_date: String,
}

async fn propose_alternate_dates_v2(
    State(state): State<LeaveRestState>,
    headers: HeaderMap,
    Path(id): Path<LeaveRequestId>,
    Json(body): Json<ProposeAlternateDatesBody>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let start_date = parse_iso_date(&body.start_date, "start_date")?;
    let end_date = parse_iso_date(&body.end_date, "end_date")?;
    let view = state
        .store
        .propose_alternate_dates(ProposeAlternateDatesCommand {
            request_id: id,
            proposer: principal.user_id,
            start_date,
            end_date,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(view).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveChargeRequest {
    /// Mutable request/workflow CAS token from `LeaveRequestView.request_version`.
    /// A successful resolution separately creates a new `charge_version`.
    expected_version: i64,
    date_charges: Vec<DateChargeRequest>,
    calendar_revision_ref: SourceRevisionRequest,
    policy_revision_ref: SourceRevisionRequest,
    #[serde(default)]
    supporting_source_refs: Vec<SourceRevisionRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DateChargeRequest {
    date: String,
    obligation: WorkObligationRequest,
    /// Exact fixed-scale day units. Totals and digests are never accepted.
    charge_units: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WorkObligationRequest {
    Scheduled { minutes: u32 },
    NotScheduled { basis: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceRevisionRequest {
    kind: String,
    reference: String,
    revision: String,
}

async fn resolve_charge(
    State(state): State<LeaveRestState>,
    headers: HeaderMap,
    Path(id): Path<LeaveRequestId>,
    Json(body): Json<ResolveChargeRequest>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_manage(&principal)?;
    let expected_version = validate_expected_request_version(body.expected_version)?;
    let date_charges = body
        .date_charges
        .into_iter()
        .map(|charge| {
            Ok(LeaveDateCharge {
                date: parse_iso_date(&charge.date, "date_charges[].date")?,
                obligation: match charge.obligation {
                    WorkObligationRequest::Scheduled { minutes } => {
                        WorkObligation::Scheduled { minutes }
                    }
                    WorkObligationRequest::NotScheduled { basis } => WorkObligation::NotScheduled {
                        basis: parse_non_work_basis(&basis)?,
                    },
                },
                units: parse_leave_units(&charge.charge_units)?,
            })
        })
        .collect::<Result<Vec<_>, RestError>>()?;
    let calendar_revision_ref = source_revision(body.calendar_revision_ref)?;
    let policy_revision_ref = source_revision(body.policy_revision_ref)?;
    let supporting_source_refs = body
        .supporting_source_refs
        .into_iter()
        .map(source_revision)
        .collect::<Result<Vec<_>, _>>()?;
    let view = state
        .store
        .resolve_charge(ResolveLeaveChargeCommand {
            request_id: id,
            resolver: principal.user_id,
            branch_scope: principal.branch_scope.clone(),
            expected_version,
            date_charges,
            calendar_revision_ref,
            policy_revision_ref,
            supporting_source_refs,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(view).into_response())
}

fn source_revision(body: SourceRevisionRequest) -> Result<SourceRevisionRef, RestError> {
    SourceRevisionRef::new(&body.kind, &body.reference, &body.revision)
        .map_err(RestError::from_kernel)
}

fn validate_expected_request_version(value: i64) -> Result<i64, RestError> {
    if value <= 0 {
        return Err(RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "expected_version must be a positive request_version",
        ));
    }
    Ok(value)
}

fn parse_non_work_basis(value: &str) -> Result<NonWorkBasis, RestError> {
    match value {
        "rest_day" => Ok(NonWorkBasis::RestDay),
        "public_holiday" => Ok(NonWorkBasis::PublicHoliday),
        "substitute_holiday" => Ok(NonWorkBasis::SubstituteHoliday),
        "contractual_day_off" => Ok(NonWorkBasis::ContractualDayOff),
        "other" => Ok(NonWorkBasis::Other),
        _ => Err(RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "non-work basis must be rest_day|public_holiday|substitute_holiday|contractual_day_off|other",
        )),
    }
}

fn parse_leave_units(value: &str) -> Result<LeaveUnits, RestError> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') || value.starts_with('+') {
        return Err(RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "charge_units must be a non-negative decimal string with at most six fractional digits",
        ));
    }
    let mut parts = value.split('.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > 6
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "charge_units must be a non-negative decimal string with at most six fractional digits",
        ));
    }
    let whole = whole.parse::<i64>().map_err(|_| {
        RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "charge_units is outside the supported range",
        )
    })?;
    let fraction = format!("{fraction:0<6}").parse::<i64>().map_err(|_| {
        RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "charge_units is invalid",
        )
    })?;
    let micros = whole
        .checked_mul(1_000_000)
        .and_then(|whole| whole.checked_add(fraction))
        .ok_or_else(|| {
            RestError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                "charge_units is outside the supported range",
            )
        })?;
    LeaveUnits::from_micros(micros).map_err(RestError::from_kernel)
}

async fn list_balances(
    State(state): State<LeaveRestState>,
    headers: HeaderMap,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_read(&principal)?;
    let page = state
        .store
        .list_balances(principal.branch_scope.clone())
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page).into_response())
}

/// A §61 push request.
///
/// `leave_period_end` and `track` are required: the statutory windows are all
/// counted back from the end of the 연차 사용기간, and §61① and §61② have
/// different periods, so neither can be defaulted. The unused-day count is
/// *not* accepted from the caller — §61①1 requires the employee's own figure,
/// which the store reads from the authoritative roster.
#[derive(Debug, Deserialize)]
struct PromotionRequest {
    /// The branch the push is served under — validated against the actor's
    /// scope, never trusted blindly.
    branch_id: Uuid,
    target_user_id: Uuid,
    target_employee_id: Uuid,
    target_name: String,
    round: i16,
    /// annual | first_year_early | first_year_late.
    track: String,
    /// 제60조제7항 본문에 따른 기간의 마지막 날 (ISO 8601).
    leave_period_end: String,
    /// The 사용 시기 a round-2 notice designates (ISO 8601). Round 1 and the
    /// refusal must not carry any.
    #[serde(default)]
    designated_dates: Vec<String>,
}

async fn push_promotion(
    State(state): State<LeaveRestState>,
    headers: HeaderMap,
    Json(body): Json<PromotionRequest>,
) -> Result<Response, RestError> {
    statutory_push(&state, &headers, PromotionKind::Promotion, body).await
}

#[derive(Debug, Deserialize)]
struct RefusalRequest {
    branch_id: Uuid,
    target_user_id: Uuid,
    target_employee_id: Uuid,
    target_name: String,
    track: String,
    leave_period_end: String,
}

async fn push_refusal(
    State(state): State<LeaveRestState>,
    headers: HeaderMap,
    Json(body): Json<RefusalRequest>,
) -> Result<Response, RestError> {
    let promotion = PromotionRequest {
        branch_id: body.branch_id,
        target_user_id: body.target_user_id,
        target_employee_id: body.target_employee_id,
        target_name: body.target_name,
        // A refusal follows a completed round 2; the domain normalizes this.
        round: 2,
        track: body.track,
        leave_period_end: body.leave_period_end,
        // The refused days are read back from the recorded 2차 통보, never
        // restated by the caller.
        designated_dates: Vec::new(),
    };
    statutory_push(&state, &headers, PromotionKind::Refusal, promotion).await
}

async fn statutory_push(
    state: &LeaveRestState,
    headers: &HeaderMap,
    kind: PromotionKind,
    body: PromotionRequest,
) -> Result<Response, RestError> {
    let track = PromotionTrack::parse(&body.track).map_err(RestError::from_kernel)?;
    let leave_period_end = parse_iso_date(&body.leave_period_end, "leave_period_end")?;
    let designated_dates = body
        .designated_dates
        .iter()
        .map(|value| parse_iso_date(value, "designated_dates"))
        .collect::<Result<Vec<_>, _>>()?;
    let principal = principal_from_headers(state, headers).await?;
    // Authorize the manage feature AGAINST the target branch: this both role-
    // gates the actor and confirms `branch_id` is within their scope, so a
    // branch outside the actor's scope is rejected rather than trusted.
    authorize(
        &principal,
        Action::new(Feature::EmployeeDirectoryManage),
        console_kernel_core::BranchId::from_uuid(body.branch_id),
    )
    .map_err(RestError::from_kernel)?;
    state
        .store
        .verify_statutory_push_target(
            body.branch_id,
            UserId::from_uuid(body.target_user_id),
            body.target_employee_id,
        )
        .await
        .map_err(RestError::from_store)?;
    let view = state
        .store
        .statutory_push(StatutoryPushCommand {
            actor: principal.user_id,
            branch_id: body.branch_id,
            target_user_id: UserId::from_uuid(body.target_user_id),
            target_employee_id: body.target_employee_id,
            target_name: body.target_name,
            kind,
            round: body.round,
            track,
            leave_period_end,
            designated_dates,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(view).into_response())
}

// ---------------------------------------------------------------------------
// Authorization helpers
// ---------------------------------------------------------------------------

/// Require the read tier (`employee_directory_read`). Role permission is
/// branch-independent, so an org-wide scope is checked org-wide and a
/// branch-scoped principal is checked against any one of its branches; the
/// branch confinement of returned rows happens in the store.
fn require_read(principal: &Principal) -> Result<(), RestError> {
    require_feature(principal, Feature::EmployeeDirectoryRead)
}

fn require_manage(principal: &Principal) -> Result<(), RestError> {
    require_feature(principal, Feature::EmployeeDirectoryManage)
}

/// A directory-wide capability decision, not a branch-less table.
///
/// `employees` DOES have a branch column: `home_branch_id`, added by migration
/// 0166 with an FK to `branches(id, org_id)` — the creating migration (0063) is
/// not the whole schema. But nothing on this surface reads it: the handlers this
/// gates list and mutate the directory across the caller's whole scope, and the
/// `Branches` arm it replaced authorized against `branches.iter().next()`, a
/// branch taken from the CALLER, so no resource branch was ever checked. The swap
/// is verdict-identical and makes that visible.
///
/// TODO(branch-scope): threading `employees.home_branch_id` through these
/// handlers would be a real tightening, and is a separate lane — it needs the
/// column backfilled (it is NULLABLE) before it can gate anything.
///
/// The `All` arm deliberately STAYS `authorize_org_wide`. Collapsing both arms
/// into `authorize_capability` would WIDEN: `EmployeeDirectoryRead` is
/// `[D,D,D,A,A,A]`, and the group-admin tenant-context path mints a
/// `{Admin} + BranchScope::All` principal
/// (`console_platform_request_context`), which `authorize_org_wide`'s
/// `SUPER_ADMIN|EXECUTIVE` built-in restriction denies today.
fn require_feature(principal: &Principal, feature: Feature) -> Result<(), RestError> {
    let action = Action::new(feature);
    let result = match &principal.branch_scope {
        BranchScope::All => authorize_org_wide(principal, action),
        BranchScope::Branches(_) => authorize_capability(principal, action),
    };
    result.map_err(RestError::from_kernel)
}

// ---------------------------------------------------------------------------
// Errors + principal resolution (mirrors the inbox REST surface)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    reasons: Vec<String>,
}

#[derive(Debug)]
struct RestError {
    status: StatusCode,
    code: &'static str,
    message: String,
    reasons: Vec<String>,
}

impl RestError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            reasons: Vec::new(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message)
    }

    fn from_kernel(err: KernelError) -> Self {
        match err.kind {
            ErrorKind::Validation => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "validation", err.message)
            }
            ErrorKind::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", err.message),
            ErrorKind::Forbidden => Self::new(StatusCode::FORBIDDEN, "forbidden", err.message),
            ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                Self::new(StatusCode::CONFLICT, "conflict", err.message)
            }
            ErrorKind::Internal => {
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", err.message)
            }
        }
    }

    fn from_store(err: PgLeaveError) -> Self {
        match err {
            PgLeaveError::Domain(err) => Self::from_kernel(err),
            PgLeaveError::MissingHomeBranch => Self::new(
                StatusCode::CONFLICT,
                "leave_home_branch_review_required",
                "an active linked employee needs an explicit home branch before filing leave",
            ),
            PgLeaveError::ChargeReviewRequired(reasons) => {
                let mut error = Self::new(
                    StatusCode::CONFLICT,
                    "leave_calendar_review_required",
                    "leave charge evidence must be resolved before approval",
                );
                error.reasons = reasons
                    .into_iter()
                    .map(|reason| reason.as_str().to_owned())
                    .collect();
                error
            }
            PgLeaveError::ConcurrentModification => Self::new(
                StatusCode::CONFLICT,
                "leave_concurrent_modification",
                "leave request_version changed since it was read; reload before retrying",
            ),
            PgLeaveError::CommandUnavailable => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "leave_command_unavailable",
                "leave command database is not configured or unavailable",
            ),
            PgLeaveError::Db(_) => {
                // Never leak sqlx/schema internals (OWASP A05). Log server-side.
                tracing::error!(error = %err, "leave store error");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal server error",
                )
            }
        }
    }
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: ErrorPayload {
                    code: self.code,
                    message: self.message,
                    reasons: self.reasons,
                },
            }),
        )
            .into_response()
    }
}

async fn principal_from_headers(
    state: &LeaveRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for the leave API")
    })?;
    console_platform_request_context::resolve_principal(verifier, state.store.pool(), headers)
        .await
        .map_err(rest_error_from_request_context)
}

fn rest_error_from_request_context(err: RequestContextError) -> RestError {
    match err {
        RequestContextError::VerifierUnavailable => {
            RestError::unavailable("JWT verification is not configured for the leave API")
        }
        RequestContextError::WrongTokenTier => RestError::from_kernel(KernelError::forbidden(
            "token tier is not valid for this route",
        )),
        RequestContextError::AccessScope(error) => RestError::from_kernel(error),
        RequestContextError::BranchScope(message)
        | RequestContextError::EffectivePolicy(message) => {
            RestError::from_kernel(KernelError::internal(message))
        }
        RequestContextError::MissingOrg => RestError::from_kernel(KernelError::internal(
            "no tenant context is bound to the current request",
        )),
        RequestContextError::MissingBearer => {
            RestError::unauthorized("missing or malformed bearer token")
        }
        RequestContextError::InvalidToken => RestError::unauthorized("invalid bearer token"),
        RequestContextError::InvalidClaim(message) => {
            RestError::unauthorized(format!("token claim is invalid: {message}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_kernel_core::{BranchId, OrgId};
    use console_platform_authz::Role;
    use std::collections::BTreeSet;

    fn principal(role: Role, branch: BranchId) -> Principal {
        Principal::new(
            UserId::new(),
            OrgId::knl(),
            BTreeSet::from([role]),
            BranchScope::Branches(BTreeSet::from([branch])),
        )
    }

    #[test]
    fn member_is_denied_read_and_manage() {
        let p = principal(Role::Member, BranchId::new());
        assert_eq!(
            require_read(&p).unwrap_err().status,
            StatusCode::FORBIDDEN,
            "a MEMBER cannot read the leave queue"
        );
        assert_eq!(
            require_manage(&p).unwrap_err().status,
            StatusCode::FORBIDDEN,
            "a MEMBER cannot decide leave"
        );
    }

    #[test]
    fn exact_leave_units_parser_never_uses_floating_point() {
        assert_eq!(parse_leave_units("0").unwrap().micros(), 0);
        assert_eq!(parse_leave_units("0.4").unwrap().micros(), 400_000);
        assert_eq!(parse_leave_units("0.500000").unwrap().micros(), 500_000);
        assert_eq!(parse_leave_units("1.000001").unwrap().micros(), 1_000_001);
        assert!(parse_leave_units("0.0000001").is_err());
        assert!(parse_leave_units("-0.5").is_err());
        assert!(parse_leave_units("NaN").is_err());
    }

    #[test]
    fn missing_leave_command_capability_maps_to_stable_503() {
        let error = RestError::from_store(PgLeaveError::CommandUnavailable);
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.code, "leave_command_unavailable");
    }

    #[test]
    fn expected_version_is_the_positive_request_cas_token() {
        for invalid in [i64::MIN, -1, 0] {
            let error = validate_expected_request_version(invalid).unwrap_err();
            assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(error.code, "validation");
            assert!(error.message.contains("positive request_version"));
        }
        assert_eq!(validate_expected_request_version(1).unwrap(), 1);
        assert_eq!(validate_expected_request_version(7).unwrap(), 7);
    }

    #[test]
    fn v2_decide_body_requires_request_cas_and_rejects_charge_version() {
        let body: DecideRequestV2 = serde_json::from_value(serde_json::json!({
            "expected_version": 7,
            "decision": "approve"
        }))
        .unwrap();
        assert_eq!(body.expected_version, 7);

        let error = serde_json::from_value::<DecideRequestV2>(serde_json::json!({
            "expected_version": 7,
            "charge_version": 3,
            "decision": "approve"
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field `charge_version`"));
    }

    #[test]
    fn v1_decide_body_accepts_deployed_shape_without_request_cas() {
        let body: DecideRequestV1 = serde_json::from_value(serde_json::json!({
            "decision": "time_change",
            "comment": "성수기 인력 공백"
        }))
        .unwrap();
        assert_eq!(body.decision, "time_change");
    }

    /// Charter §4-31 / 근로기준법 §60: refusal decisions must die at the
    /// domain parse — the REST layer maps that to a 422, never a decision.
    #[test]
    fn kr_charter_refusal_decisions_are_unprocessable() {
        for forbidden in ["reject", "return"] {
            let error = RestError::from_kernel(
                LeaveDecision::parse(forbidden).expect_err("refusal must not parse"),
            );
            assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
        }
    }

    #[test]
    fn v2_create_body_requires_stable_submission_key() {
        let key = Uuid::new_v4();
        let modern: CreateRequestBody = serde_json::from_value(serde_json::json!({
            "idempotency_key": key,
            "leave_type": "annual",
            "start_date": "2026-08-03",
            "end_date": "2026-08-03"
        }))
        .unwrap();
        assert_eq!(modern.idempotency_key, key);

        let missing_key = serde_json::from_value::<CreateRequestBody>(serde_json::json!({
            "leave_type": "annual",
            "start_date": "2026-08-03",
            "end_date": "2026-08-03"
        }));
        assert!(missing_key.is_err());
    }

    /// Charter §4-31 / 근로기준법 §60: "연차=사유란 없음". A client that still
    /// submits a 사유 is refused at the boundary instead of the field being
    /// stored (or silently dropped, which would keep soliciting it).
    #[test]
    fn kr_charter_create_body_has_no_reason_field() {
        let error = serde_json::from_value::<CreateRequestBody>(serde_json::json!({
            "idempotency_key": Uuid::new_v4(),
            "leave_type": "annual",
            "start_date": "2026-08-03",
            "end_date": "2026-08-03",
            "reason": "vacation"
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field `reason`"));
    }

    #[test]
    fn branch_admin_can_read_and_manage_its_branch() {
        let branch = BranchId::new();
        let p = principal(Role::Admin, branch);
        assert!(require_read(&p).is_ok());
        assert!(require_manage(&p).is_ok());
        // In-scope branch push authorization passes...
        assert!(authorize(&p, Action::new(Feature::EmployeeDirectoryManage), branch,).is_ok());
        // ...but a branch OUTSIDE the actor's scope is rejected, so a pushed
        // `branch_id` can never escape the actor's authority.
        assert!(
            authorize(
                &p,
                Action::new(Feature::EmployeeDirectoryManage),
                BranchId::new(),
            )
            .is_err(),
            "an out-of-scope branch_id must be rejected"
        );
    }
}
