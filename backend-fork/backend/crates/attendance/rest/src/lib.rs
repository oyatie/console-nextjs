//! Authenticated REST boundary for the Attendance console.  It validates wire
//! input, derives a tenant/branch scope from the signed principal, and delegates
//! all business decisions and database work to the private application layers.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::{
    Json, Router,
    extract::{FromRequestParts, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use console_attendance_adapter_postgres::{AttendanceStoreError, PgAttendanceStore};
use console_attendance_application::{
    AcknowledgeWeek52, AmendClose, AssignSubstitute, AttendanceEvidence, AttendanceExceptionRead,
    AttendanceSubstitutionRead, CallerScope, CancelSubstitution, CloseMonth, ClosePreflightRead,
    ListExceptions, ListSubstitutions, MonthCloseRead, RaiseException, ResolveException,
    SubstitutionCandidateQuery, SubstitutionCandidateRead, Week52Read, validate_week52_start,
    week52_tone,
};
use console_attendance_domain::{
    AttendanceDateRange, ExceptionKind, ResolutionAction, SubstitutionWindow,
};
use console_kernel_core::{BranchId, BranchScope, ErrorKind, KernelError};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize, authorize_org_wide};
use console_platform_request_context::{RequestContextError, resolve_principal};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use time::{Date, Duration};
use uuid::Uuid;

mod self_service;

pub const ATTENDANCE_EXCEPTIONS_PATH: &str = "/api/v1/attendance/exceptions";
pub const ATTENDANCE_EXCEPTION_DETAIL_PATH: &str = "/api/v1/attendance/exceptions/{exception_id}";
pub const ATTENDANCE_EXCEPTION_RESOLVE_PATH: &str =
    "/api/v1/attendance/exceptions/{exception_id}/resolve";
pub const ATTENDANCE_SUBSTITUTIONS_PATH: &str = "/api/v1/attendance/substitutions";
pub const ATTENDANCE_SUBSTITUTION_CANDIDATES_PATH: &str =
    "/api/v1/attendance/substitution-candidates";
pub const ATTENDANCE_SUBSTITUTION_CANCEL_PATH: &str =
    "/api/v1/attendance/substitutions/{substitution_id}/cancel";
pub const ATTENDANCE_CLOSES_PATH: &str = "/api/v1/attendance/closes";
pub const ATTENDANCE_CLOSE_PREFLIGHT_PATH: &str = "/api/v1/attendance/closes/preflight";
pub const ATTENDANCE_CLOSE_AMEND_PATH: &str = "/api/v1/attendance/closes/{close_id}/amendments";
pub const ATTENDANCE_WEEK52_PATH: &str = "/api/v1/attendance/week52";
pub const ATTENDANCE_WEEK52_ACK_PATH: &str = "/api/v1/attendance/week52/acks";
pub const ATTENDANCE_ME_EXCEPTIONS_PATH: &str = "/api/v1/attendance/me/exceptions";
pub const ATTENDANCE_ME_WEEK52_PATH: &str = "/api/v1/attendance/me/week52";
pub const ATTENDANCE_ROUTE_PATHS: &[&str] = &[
    ATTENDANCE_EXCEPTIONS_PATH,
    ATTENDANCE_EXCEPTION_DETAIL_PATH,
    ATTENDANCE_EXCEPTION_RESOLVE_PATH,
    ATTENDANCE_SUBSTITUTIONS_PATH,
    ATTENDANCE_SUBSTITUTION_CANDIDATES_PATH,
    ATTENDANCE_SUBSTITUTION_CANCEL_PATH,
    ATTENDANCE_CLOSES_PATH,
    ATTENDANCE_CLOSE_PREFLIGHT_PATH,
    ATTENDANCE_CLOSE_AMEND_PATH,
    ATTENDANCE_WEEK52_PATH,
    ATTENDANCE_WEEK52_ACK_PATH,
    ATTENDANCE_ME_EXCEPTIONS_PATH,
    ATTENDANCE_ME_WEEK52_PATH,
];
const READ: Feature = Feature::EmployeeDirectoryRead;
const EXCEPTION_MANAGE: Feature = Feature::AttendanceExceptionManage;
const SUBSTITUTION_MANAGE: Feature = Feature::AttendanceSubstitutionManage;
const CLOSE: Feature = Feature::PeriodLockManage;
const ATTENDANCE_REST_READS_TOTAL: &str = "attendance_rest_reads_total";
// Asserted by `attendance_read_surfaces_are_static_and_complete`; nothing on
// the serving path reads it, so it does not belong in a non-test build.
#[cfg(test)]
const ATTENDANCE_READ_SURFACES: [&str; 8] = [
    "substitutions",
    "substitution_candidates",
    "exceptions",
    "exception_detail",
    "closes",
    "week52",
    "me_exceptions",
    "me_week52",
];

struct AttendanceQuery<T>(T);

impl<S, T> FromRequestParts<S> for AttendanceQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = RestError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(query)| Self(query))
            .map_err(|_| RestError::malformed_query())
    }
}

fn record_read(surface: &'static str) {
    metrics::counter!(ATTENDANCE_REST_READS_TOTAL, "surface" => surface).increment(1);
}

#[derive(Clone)]
pub struct AttendanceRestState {
    store: PgAttendanceStore,
    jwt: Option<JwtVerifier>,
}
impl AttendanceRestState {
    #[must_use]
    pub fn new(store: PgAttendanceStore, jwt: Option<JwtVerifier>) -> Self {
        Self { store, jwt }
    }
}
pub fn router(state: AttendanceRestState) -> Router {
    let verifier = state.jwt.clone();
    let pool = state.store.pool().clone();
    let r = Router::new()
        .route(
            ATTENDANCE_EXCEPTIONS_PATH,
            get(list_exceptions).post(raise_exception),
        )
        .route(ATTENDANCE_EXCEPTION_DETAIL_PATH, get(exception_detail))
        .route(ATTENDANCE_EXCEPTION_RESOLVE_PATH, post(resolve_exception))
        .route(
            ATTENDANCE_SUBSTITUTIONS_PATH,
            get(list_substitutions).post(assign_substitute),
        )
        .route(
            ATTENDANCE_SUBSTITUTION_CANDIDATES_PATH,
            get(list_substitution_candidates),
        )
        .route(ATTENDANCE_CLOSE_PREFLIGHT_PATH, post(close_preflight))
        .route(
            ATTENDANCE_SUBSTITUTION_CANCEL_PATH,
            post(cancel_substitution),
        )
        .route(ATTENDANCE_CLOSES_PATH, get(list_closes).post(close_month))
        .route(ATTENDANCE_CLOSE_AMEND_PATH, post(amend_close))
        .route(ATTENDANCE_WEEK52_PATH, get(week52))
        .route(ATTENDANCE_WEEK52_ACK_PATH, post(acknowledge_week52))
        .route(
            ATTENDANCE_ME_EXCEPTIONS_PATH,
            get(self_service::list_own_exceptions),
        )
        .route(
            ATTENDANCE_ME_WEEK52_PATH,
            get(self_service::read_own_week52),
        )
        .with_state(state);
    console_platform_request_context::with_request_context(r, verifier, pool)
}

async fn principal(
    state: &AttendanceRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt.as_ref().ok_or_else(|| {
        RestError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "JWT verification is not configured",
        )
    })?;
    resolve_principal(verifier, state.store.pool(), headers)
        .await
        .map_err(|e| match e {
            RequestContextError::MissingBearer
            | RequestContextError::InvalidToken
            | RequestContextError::InvalidClaim(_) => RestError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing or invalid bearer token",
            ),
            RequestContextError::WrongTokenTier | RequestContextError::AccessScope(_) => {
                RestError::kernel(KernelError::forbidden(
                    "token is not authorized for attendance",
                ))
            }
            RequestContextError::VerifierUnavailable => RestError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "JWT verification is not configured",
            ),
            _ => RestError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "request principal could not be resolved",
            ),
        })
}
fn require_for_branch(
    principal: &Principal,
    feature: Feature,
    branch_id: Option<Uuid>,
) -> Result<(), RestError> {
    if !branch_request_is_well_scoped(&principal.branch_scope, branch_id) {
        return Err(RestError::kernel(KernelError::forbidden(
            "branchId is required for branch-limited attendance access",
        )));
    }
    match branch_id {
        Some(id) => authorize(principal, Action::new(feature), BranchId::from_uuid(id))
            .map_err(RestError::kernel),
        None if matches!(&principal.branch_scope, BranchScope::All) => {
            authorize_org_wide(principal, Action::new(feature)).map_err(RestError::kernel)
        }
        // `branch_request_is_well_scoped` rejects this case first. Keeping this
        // branch makes the authorization boundary fail closed if it changes.
        None => Err(RestError::kernel(KernelError::forbidden(
            "branchId is required",
        ))),
    }
}

/// An omitted branch is an org-wide query, never an implicit "my branches"
/// query. Concrete branch IDs are authorized by `authorize` below.
fn branch_request_is_well_scoped(scope: &BranchScope, branch_id: Option<Uuid>) -> bool {
    branch_id.is_some() || matches!(scope, BranchScope::All)
}
fn require_resource_branch(
    principal: &Principal,
    feature: Feature,
    branch_id: Option<Uuid>,
) -> Result<(), RestError> {
    let visible = match branch_id {
        Some(id) => principal.branch_scope.allows(BranchId::from_uuid(id)),
        None => matches!(&principal.branch_scope, BranchScope::All),
    };
    if !visible {
        return Err(RestError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "resource was not found",
        ));
    }
    // A resource inside the caller's scope remains visible; missing feature permission is a truthful 403.
    require_for_branch(principal, feature, branch_id)
}

fn scope(principal: &Principal) -> CallerScope {
    match &principal.branch_scope {
        BranchScope::All => CallerScope {
            org_id: *principal.org_id.as_uuid(),
            user_id: *principal.user_id.as_uuid(),
            branch_ids: vec![],
            org_wide: true,
        },
        BranchScope::Branches(branches) => CallerScope {
            org_id: *principal.org_id.as_uuid(),
            user_id: *principal.user_id.as_uuid(),
            branch_ids: branches.iter().map(|id| *id.as_uuid()).collect(),
            org_wide: false,
        },
    }
}
fn parse_date(raw: &str, name: &str) -> Result<Date, RestError> {
    Date::parse(
        raw,
        time::macros::format_description!("[year]-[month]-[day]"),
    )
    .map_err(|_| {
        RestError::kernel(KernelError::validation(format!(
            "{name} must be YYYY-MM-DD"
        )))
    })
}
fn parse_month_range(month: &str) -> Result<AttendanceDateRange, RestError> {
    AttendanceDateRange::selected_month_with_buffer(month).map_err(|e| {
        RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            e.to_string(),
        )
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    month: Option<String>,
    from_date: Option<String>,
    to_date: Option<String>,
    work_date: Option<String>,
    status: Option<String>,
    employee_id: Option<Uuid>,
    branch_id: Option<Uuid>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubstitutionCandidateListQuery {
    branch_id: Uuid,
    covered_employee_id: Uuid,
    cover_date: String,
    from_minutes: i32,
    to_minutes: i32,
    search: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}
fn list_range(q: &ListQuery) -> Result<AttendanceDateRange, RestError> {
    let selectors = usize::from(q.month.is_some())
        + usize::from(q.work_date.is_some())
        + usize::from(q.from_date.is_some() || q.to_date.is_some());
    if selectors != 1 {
        return Err(RestError::kernel(KernelError::validation(
            "supply exactly one date selector",
        )));
    }
    match (&q.month, &q.work_date, &q.from_date, &q.to_date) {
        (Some(month), None, None, None) => parse_month_range(month),
        (None, Some(day), None, None) => {
            let date = parse_date(day, "work_date")?;
            AttendanceDateRange::new(date, date + Duration::days(1)).map_err(|e| {
                RestError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation",
                    e.to_string(),
                )
            })
        }
        (None, None, Some(from), Some(to)) => {
            AttendanceDateRange::new(parse_date(from, "from_date")?, parse_date(to, "to_date")?)
                .map_err(|e| {
                    RestError::new(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "validation",
                        e.to_string(),
                    )
                })
        }
        _ => Err(RestError::kernel(KernelError::validation(
            "invalid date selector",
        ))),
    }
}
#[derive(Serialize)]
struct SubstitutionDto {
    id: String,
    site: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch_id: Option<String>,
    role: String,
    cover_date: String,
    from_minutes: i32,
    to_minutes: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    covered_employee_id: Option<String>,
    covered_name: String,
    reason_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_employee_id: Option<String>,
    worker_name: String,
    worker_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_rate: Option<String>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    exception_id: Option<String>,
    created_by: String,
    created_at: String,
}
impl From<AttendanceSubstitutionRead> for SubstitutionDto {
    fn from(v: AttendanceSubstitutionRead) -> Self {
        Self {
            id: v.id.to_string(),
            site: v.site,
            branch_id: v.branch_id.map(|id| id.to_string()),
            role: v.role,
            cover_date: v.cover_date.to_string(),
            from_minutes: v.from_minutes,
            to_minutes: v.to_minutes,
            covered_employee_id: Some(v.covered_employee_id.to_string()),
            covered_name: v.covered_name,
            reason_kind: v.reason_kind,
            reason_detail: v.reason_detail,
            worker_employee_id: v.worker_employee_id.map(|id| id.to_string()),
            worker_name: v.worker_name,
            worker_type: v.worker_type,
            worker_rate: v.worker_rate,
            status: v.status,
            exception_id: v.exception_id.map(|id| id.to_string()),
            created_by: v.created_by.to_string(),
            created_at: v
                .created_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| v.created_at.to_string()),
        }
    }
}
#[derive(Serialize)]
struct SubstitutionPageDto {
    items: Vec<SubstitutionDto>,
    total: i64,
    limit: i64,
    offset: i64,
}

async fn list_substitutions(
    State(state): State<AttendanceRestState>,
    headers: HeaderMap,
    AttendanceQuery(q): AttendanceQuery<ListQuery>,
) -> Result<Json<SubstitutionPageDto>, RestError> {
    let p = principal(&state, &headers).await?;
    require_for_branch(&p, READ, q.branch_id)?;
    record_read("substitutions");
    let query = ListSubstitutions::new(list_range(&q)?, q.branch_id, q.limit, q.offset);
    let page = state
        .store
        .list_substitutions(&scope(&p), query)
        .await
        .map_err(RestError::store)?;
    Ok(Json(SubstitutionPageDto {
        items: page.items.into_iter().map(SubstitutionDto::from).collect(),
        total: page.total,
        limit: page.limit,
        offset: page.offset,
    }))
}

#[derive(Serialize)]
struct SubstitutionCandidateDto {
    employee_id: String,
    employee_name: String,
    branch_id: String,
}

impl From<SubstitutionCandidateRead> for SubstitutionCandidateDto {
    fn from(value: SubstitutionCandidateRead) -> Self {
        Self {
            employee_id: value.employee_id.to_string(),
            employee_name: value.employee_name,
            branch_id: value.branch_id.to_string(),
        }
    }
}

#[derive(Serialize)]
struct SubstitutionCandidatePageDto {
    items: Vec<SubstitutionCandidateDto>,
    total: i64,
    limit: i64,
    offset: i64,
}

async fn list_substitution_candidates(
    State(state): State<AttendanceRestState>,
    headers: HeaderMap,
    AttendanceQuery(q): AttendanceQuery<SubstitutionCandidateListQuery>,
) -> Result<Json<SubstitutionCandidatePageDto>, RestError> {
    let p = principal(&state, &headers).await?;
    require_for_branch(&p, SUBSTITUTION_MANAGE, Some(q.branch_id))?;
    let query = SubstitutionCandidateQuery::new(
        q.branch_id,
        q.covered_employee_id,
        SubstitutionWindow::new(
            parse_date(&q.cover_date, "cover_date")?,
            q.from_minutes,
            q.to_minutes,
        )
        .map_err(|error| {
            RestError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                error.to_string(),
            )
        })?,
        q.search,
        q.limit,
        q.offset,
    )
    .map_err(|error| {
        RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            error.to_string(),
        )
    })?;
    record_read("substitution_candidates");
    let page = state
        .store
        .list_substitution_candidates(&scope(&p), query)
        .await
        .map_err(RestError::store)?;
    Ok(Json(SubstitutionCandidatePageDto {
        items: page
            .items
            .into_iter()
            .map(SubstitutionCandidateDto::from)
            .collect(),
        total: page.total,
        limit: page.limit,
        offset: page.offset,
    }))
}
#[derive(Serialize)]
struct ExceptionEvidenceDto {
    name: String,
    size: Option<String>,
}
#[derive(Serialize)]
struct ExceptionLinkDto {
    kind: String,
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    r#ref: Option<String>,
}
#[derive(Serialize)]
struct ExceptionResolutionDto {
    action: String,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    linked_work_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ot_hours: Option<f64>,
    actor: String,
    resolved_at: String,
}
#[derive(Serialize)]
struct ExceptionDto {
    id: String,
    code: String,
    kind: String,
    status: String,
    employee_id: String,
    employee_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    team: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch_id: Option<String>,
    work_date: String,
    occurred_at: String,
    detail: String,
    evidence: Vec<ExceptionEvidenceDto>,
    links: Vec<ExceptionLinkDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolution: Option<ExceptionResolutionDto>,
    created_at: String,
}
impl From<AttendanceExceptionRead> for ExceptionDto {
    fn from(value: AttendanceExceptionRead) -> Self {
        Self {
            id: value.id.to_string(),
            code: value.code,
            kind: value.kind.as_db().to_owned(),
            status: value.status,
            employee_id: value.employee_id.to_string(),
            employee_name: value.employee_name,
            team: value.team,
            branch_id: value.branch_id.map(|id| id.to_string()),
            work_date: value.work_date.to_string(),
            occurred_at: value
                .occurred_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| value.occurred_at.to_string()),
            detail: value.detail,
            evidence: value
                .evidence
                .into_iter()
                .map(|v| ExceptionEvidenceDto {
                    name: v.name,
                    size: v.size,
                })
                .collect(),
            links: value
                .links
                .into_iter()
                .map(|v| ExceptionLinkDto {
                    kind: v.kind,
                    label: v.label,
                    r#ref: v.reference,
                })
                .collect(),
            resolution: value.resolution.map(|v| ExceptionResolutionDto {
                action: v.action.as_db().to_owned(),
                reason: v.reason,
                linked_work_ref: v.linked_work_ref,
                ot_hours: v.ot_hours.and_then(|hours| hours.parse().ok()),
                actor: v.actor.to_string(),
                resolved_at: v
                    .resolved_at
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| v.resolved_at.to_string()),
            }),
            created_at: value
                .created_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| value.created_at.to_string()),
        }
    }
}
#[derive(Serialize)]
struct ExceptionPageDto {
    items: Vec<ExceptionDto>,
    total: i64,
    limit: i64,
    offset: i64,
}

async fn list_exceptions(
    State(state): State<AttendanceRestState>,
    headers: HeaderMap,
    AttendanceQuery(q): AttendanceQuery<ListQuery>,
) -> Result<Json<ExceptionPageDto>, RestError> {
    let p = principal(&state, &headers).await?;
    require_for_branch(&p, READ, q.branch_id)?;
    record_read("exceptions");
    let page = state
        .store
        .list_exceptions(
            &scope(&p),
            ListExceptions::new(
                list_range(&q)?,
                q.branch_id,
                q.status.clone(),
                q.employee_id,
                q.limit,
                q.offset,
            )
            .map_err(|e| {
                RestError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation",
                    e.to_string(),
                )
            })?,
        )
        .await
        .map_err(RestError::store)?;
    Ok(Json(ExceptionPageDto {
        items: page.items.into_iter().map(ExceptionDto::from).collect(),
        total: page.total,
        limit: page.limit,
        offset: page.offset,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RaiseBody {
    kind: String,
    employee_id: Uuid,
    branch_id: Option<Uuid>,
    work_date: String,
    detail: String,
    #[serde(default)]
    evidence: Vec<AttendanceEvidence>,
}
async fn raise_exception(
    State(state): State<AttendanceRestState>,
    headers: HeaderMap,
    Json(body): Json<RaiseBody>,
) -> Result<(StatusCode, Json<ExceptionDto>), RestError> {
    let p = principal(&state, &headers).await?;
    require_for_branch(&p, EXCEPTION_MANAGE, body.branch_id)?;
    let key = idempotency(&headers)?;
    let command = RaiseException {
        kind: ExceptionKind::parse(&body.kind).map_err(|e| {
            RestError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                e.to_string(),
            )
        })?,
        employee_id: body.employee_id,
        branch_id: body.branch_id,
        work_date: parse_date(&body.work_date, "workDate")?,
        detail: body.detail,
        evidence: body.evidence,
        idempotency_key: key,
    };
    let v = state
        .store
        .raise_exception(&scope(&p), command)
        .await
        .map_err(RestError::store)?;
    Ok((StatusCode::CREATED, Json(ExceptionDto::from(v))))
}
async fn exception_detail(
    State(state): State<AttendanceRestState>,
    headers: HeaderMap,
    axum::extract::Path(exception_id): axum::extract::Path<Uuid>,
) -> Result<Json<ExceptionDto>, RestError> {
    let p = principal(&state, &headers).await?;
    let branch = state
        .store
        .exception_branch(*p.org_id.as_uuid(), exception_id)
        .await
        .map_err(RestError::store)?
        .ok_or_else(|| {
            RestError::new(StatusCode::NOT_FOUND, "not_found", "resource was not found")
        })?;
    require_resource_branch(&p, READ, branch)?;
    record_read("exception_detail");
    state
        .store
        .exception_detail(&scope(&p), exception_id)
        .await
        .map(ExceptionDto::from)
        .map(Json)
        .map_err(RestError::store)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveBody {
    action: String,
    reason: String,
    linked_work_ref: Option<String>,
    ot_hours: Option<f64>,
}

fn parse_resolve_action(action: &str) -> Result<ResolutionAction, RestError> {
    ResolutionAction::parse(action).map_err(|e| {
        RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            e.to_string(),
        )
    })
}

async fn resolve_exception(
    State(state): State<AttendanceRestState>,
    headers: HeaderMap,
    axum::extract::Path(exception_id): axum::extract::Path<Uuid>,
    Json(body): Json<ResolveBody>,
) -> Result<Json<ExceptionDto>, RestError> {
    let p = principal(&state, &headers).await?;
    let resource_branch = state
        .store
        .exception_branch(*p.org_id.as_uuid(), exception_id)
        .await
        .map_err(RestError::store)?
        .ok_or_else(|| {
            RestError::new(StatusCode::NOT_FOUND, "not_found", "resource was not found")
        })?;
    require_resource_branch(&p, EXCEPTION_MANAGE, resource_branch)?;
    let v = state
        .store
        .resolve_exception(
            &scope(&p),
            ResolveException {
                exception_id,
                action: parse_resolve_action(&body.action)?,
                reason: body.reason,
                linked_work_ref: body.linked_work_ref,
                overtime_minutes: body.ot_hours.map(|hours| (hours * 60.0).round() as i32),
            },
        )
        .await
        .map_err(RestError::store)?;
    Ok(Json(ExceptionDto::from(v)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssignBody {
    site: String,
    branch_id: Option<Uuid>,
    role: String,
    cover_date: String,
    from_minutes: i32,
    to_minutes: i32,
    covered_employee_id: Uuid,
    reason_kind: String,
    reason_detail: Option<String>,
    worker_employee_id: Uuid,
    exception_id: Option<Uuid>,
}
async fn assign_substitute(
    State(state): State<AttendanceRestState>,
    headers: HeaderMap,
    Json(body): Json<AssignBody>,
) -> Result<(StatusCode, Json<SubstitutionDto>), RestError> {
    let p = principal(&state, &headers).await?;
    require_for_branch(&p, SUBSTITUTION_MANAGE, body.branch_id)?;
    let command = AssignSubstitute {
        window: SubstitutionWindow::new(
            parse_date(&body.cover_date, "coverDate")?,
            body.from_minutes,
            body.to_minutes,
        )
        .map_err(|e| {
            RestError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                e.to_string(),
            )
        })?,
        branch_id: body.branch_id,
        site: body.site,
        role: body.role,
        covered_employee_id: body.covered_employee_id,
        reason_kind: body.reason_kind,
        reason_detail: body.reason_detail,
        worker_employee_id: Some(body.worker_employee_id),
        // The store replaces these implementation-only fields from canonical
        // HR data before persistence; the public request cannot supply them.
        worker_name: String::new(),
        worker_type: String::new(),
        worker_rate: None,
        exception_id: body.exception_id,
        idempotency_key: idempotency(&headers)?,
    };
    let v = state
        .store
        .assign_substitute(&scope(&p), command)
        .await
        .map_err(RestError::store)?;
    Ok((StatusCode::CREATED, Json(SubstitutionDto::from(v))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelBody {
    reason: String,
}
async fn cancel_substitution(
    State(state): State<AttendanceRestState>,
    headers: HeaderMap,
    axum::extract::Path(substitution_id): axum::extract::Path<Uuid>,
    Json(body): Json<CancelBody>,
) -> Result<Json<SubstitutionDto>, RestError> {
    let p = principal(&state, &headers).await?;
    let branch = state
        .store
        .substitution_branch(*p.org_id.as_uuid(), substitution_id)
        .await
        .map_err(RestError::store)?
        .ok_or_else(|| {
            RestError::new(StatusCode::NOT_FOUND, "not_found", "resource was not found")
        })?;
    require_resource_branch(&p, SUBSTITUTION_MANAGE, branch)?;
    state
        .store
        .cancel_substitution(
            &scope(&p),
            CancelSubstitution {
                substitution_id,
                reason: body.reason,
            },
        )
        .await
        .map(SubstitutionDto::from)
        .map(Json)
        .map_err(RestError::store)
}

#[derive(Serialize)]
struct CloseCheckDto {
    key: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    warn: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}
#[derive(Serialize)]
struct CloseAmendmentDto {
    id: String,
    reason: String,
    actor: String,
    created_at: String,
}
#[derive(Serialize)]
struct MonthCloseDto {
    id: String,
    month: String,
    branch_scope: String,
    checks: Vec<CloseCheckDto>,
    attested_by: String,
    attested_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    period_lock_id: Option<String>,
    closed_at: String,
    amendments: Vec<CloseAmendmentDto>,
}
impl From<MonthCloseRead> for MonthCloseDto {
    fn from(v: MonthCloseRead) -> Self {
        Self {
            id: v.id.to_string(),
            month: v.month.to_string(),
            branch_scope: v
                .branch_id
                .map(|i| i.to_string())
                .unwrap_or_else(|| "org".into()),
            checks: v
                .checks
                .into_iter()
                .map(|c| CloseCheckDto {
                    key: c.key,
                    ok: c.ok,
                    warn: c.warn,
                    note: c.note,
                })
                .collect(),
            attested_by: v.attested_by.to_string(),
            attested_at: v.attested_at.to_string(),
            period_lock_id: v.period_lock_id.map(|i| i.to_string()),
            closed_at: v.closed_at.to_string(),
            amendments: v
                .amendments
                .into_iter()
                .map(|a| CloseAmendmentDto {
                    id: a.id.to_string(),
                    reason: a.reason,
                    actor: a.actor.to_string(),
                    created_at: a.created_at.to_string(),
                })
                .collect(),
        }
    }
}
#[derive(Serialize)]
struct ClosePreflightDto {
    month: String,
    branch_scope: String,
    checks: Vec<CloseCheckDto>,
    can_close: bool,
}
impl From<ClosePreflightRead> for ClosePreflightDto {
    fn from(v: ClosePreflightRead) -> Self {
        Self {
            month: v.month.to_string(),
            branch_scope: v
                .branch_id
                .map(|i| i.to_string())
                .unwrap_or_else(|| "org".into()),
            checks: v
                .checks
                .into_iter()
                .map(|c| CloseCheckDto {
                    key: c.key,
                    ok: c.ok,
                    warn: c.warn,
                    note: c.note,
                })
                .collect(),
            can_close: v.can_close,
        }
    }
}
#[derive(Serialize)]
struct MonthCloseItemDto {
    branch_scope: String,
    closed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    close: Option<MonthCloseDto>,
    open_exceptions: i64,
    pending_leave: i64,
}
#[derive(Serialize)]
struct MonthCloseBoardDto {
    month: String,
    items: Vec<MonthCloseItemDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CloseBody {
    month: String,
    branch_scope: Option<Uuid>,
    attest: Option<bool>,
}
async fn close_preflight(
    State(state): State<AttendanceRestState>,
    headers: HeaderMap,
    Json(body): Json<CloseBody>,
) -> Result<Json<ClosePreflightDto>, RestError> {
    let p = principal(&state, &headers).await?;
    require_for_branch(&p, CLOSE, body.branch_scope)?;
    let checks = state
        .store
        .close_checks(
            &scope(&p),
            &CloseMonth {
                month: body.month,
                branch_scope: body.branch_scope,
                attest: false,
            },
        )
        .await
        .map_err(RestError::store)?;
    Ok(Json(ClosePreflightDto::from(checks)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CloseListQuery {
    month: String,
    branch_id: Option<Uuid>,
}
async fn list_closes(
    State(state): State<AttendanceRestState>,
    headers: HeaderMap,
    AttendanceQuery(q): AttendanceQuery<CloseListQuery>,
) -> Result<Json<MonthCloseBoardDto>, RestError> {
    let p = principal(&state, &headers).await?;
    require_for_branch(&p, READ, q.branch_id)?;
    record_read("closes");
    let month = AttendanceDateRange::selected_month_with_buffer(&q.month)
        .map_err(|error| {
            RestError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                error.to_string(),
            )
        })?
        .from;
    let preflight = state
        .store
        .close_checks(
            &scope(&p),
            &CloseMonth {
                month: q.month.clone(),
                branch_scope: q.branch_id,
                attest: false,
            },
        )
        .await
        .map_err(RestError::store)?;
    let closes = state
        .store
        .list_closes(&scope(&p), q.branch_id, month)
        .await
        .map_err(RestError::store)?;
    let open = |checks: &[console_attendance_application::CloseCheckRead],
                key: &str|
     -> Result<i64, RestError> {
        checks
            .iter()
            .find(|check| check.key == key)
            .and_then(|check| check.note.as_deref())
            .and_then(|note| note.parse().ok())
            .ok_or_else(|| {
                RestError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "close checks are invalid",
                )
            })
    };
    let items = if closes.is_empty() {
        vec![MonthCloseItemDto {
            branch_scope: q
                .branch_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "org".into()),
            closed: false,
            close: None,
            open_exceptions: open(&preflight.checks, "open_exceptions")?,
            pending_leave: open(&preflight.checks, "pending_leave")?,
        }]
    } else {
        closes
            .into_iter()
            .map(|close| {
                let open_exceptions = open(&close.checks, "open_exceptions")?;
                let pending_leave = open(&close.checks, "pending_leave")?;
                Ok(MonthCloseItemDto {
                    branch_scope: close
                        .branch_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "org".into()),
                    closed: true,
                    close: Some(MonthCloseDto::from(close)),
                    open_exceptions,
                    pending_leave,
                })
            })
            .collect::<Result<Vec<_>, RestError>>()?
    };
    Ok(Json(MonthCloseBoardDto {
        month: q.month,
        items,
    }))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AmendCloseBody {
    reason: String,
    detail: String,
    #[serde(default)]
    r#ref: Option<String>,
}
async fn amend_close(
    State(state): State<AttendanceRestState>,
    headers: HeaderMap,
    axum::extract::Path(close_id): axum::extract::Path<Uuid>,
    Json(body): Json<AmendCloseBody>,
) -> Result<Json<CloseAmendmentDto>, RestError> {
    let p = principal(&state, &headers).await?;
    let branch = state
        .store
        .close_branch(*p.org_id.as_uuid(), close_id)
        .await
        .map_err(RestError::store)?
        .ok_or_else(|| {
            RestError::new(StatusCode::NOT_FOUND, "not_found", "resource was not found")
        })?;
    require_resource_branch(&p, CLOSE, branch)?;
    state
        .store
        .amend_close(
            &scope(&p),
            AmendClose {
                close_id,
                reason: body.reason,
                detail: body.detail,
                reference: body.r#ref,
                idempotency_key: idempotency(&headers)?,
            },
        )
        .await
        .map(|v| CloseAmendmentDto {
            id: v.id.to_string(),
            reason: v.reason,
            actor: v.actor.to_string(),
            created_at: v.created_at.to_string(),
        })
        .map(Json)
        .map_err(RestError::store)
}

async fn close_month(
    State(state): State<AttendanceRestState>,
    headers: HeaderMap,
    Json(body): Json<CloseBody>,
) -> Result<(StatusCode, Json<MonthCloseDto>), RestError> {
    let p = principal(&state, &headers).await?;
    require_for_branch(&p, CLOSE, body.branch_scope)?;
    let v = state
        .store
        .close_month(
            &scope(&p),
            CloseMonth {
                month: body.month,
                branch_scope: body.branch_scope,
                attest: body.attest.unwrap_or(false),
            },
        )
        .await
        .map_err(RestError::store)?;
    Ok((StatusCode::CREATED, Json(MonthCloseDto::from(v))))
}

#[derive(Serialize)]
struct Week52RowDto {
    employee_id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    team: Option<String>,
    week_start: String,
    current_hours: f64,
    projected_hours: f64,
    tone: String,
    acked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    acked_at: Option<String>,
}
impl From<Week52Read> for Week52RowDto {
    fn from(v: Week52Read) -> Self {
        let tone = week52_tone(&console_attendance_application::Week52Input {
            employee_id: v.employee_id,
            week_start: v.week_start,
            current_hours: v.current_hours,
            projected_hours: v.projected_hours,
            acknowledged_at: v.acknowledged_at,
        });
        Self {
            employee_id: v.employee_id.to_string(),
            name: v.name,
            team: v.team,
            week_start: v.week_start.to_string(),
            current_hours: v.current_hours,
            projected_hours: v.projected_hours,
            tone: match tone {
                console_attendance_application::Week52Tone::Ok => "OK",
                console_attendance_application::Week52Tone::Warn => "WARN",
                console_attendance_application::Week52Tone::Danger => "DANGER",
            }
            .into(),
            acked: v.acknowledged_at.is_some(),
            acked_at: v.acknowledged_at.map(|d| {
                d.format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| d.to_string())
            }),
        }
    }
}
#[derive(Serialize)]
struct Week52BoardDto {
    week_start: String,
    items: Vec<Week52RowDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Week52Query {
    week_start: String,
    branch_id: Option<Uuid>,
}
async fn week52(
    State(state): State<AttendanceRestState>,
    headers: HeaderMap,
    AttendanceQuery(q): AttendanceQuery<Week52Query>,
) -> Result<Json<Week52BoardDto>, RestError> {
    let p = principal(&state, &headers).await?;
    require_for_branch(&p, READ, q.branch_id)?;
    record_read("week52");
    let week_start =
        validate_week52_start(parse_date(&q.week_start, "weekStart")?).map_err(|e| {
            RestError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                e.to_string(),
            )
        })?;
    let items = state
        .store
        .week52_inputs(&scope(&p), week_start, q.branch_id)
        .await
        .map_err(RestError::store)?
        .into_iter()
        .map(Week52RowDto::from)
        .collect();
    Ok(Json(Week52BoardDto {
        week_start: week_start.to_string(),
        items,
    }))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Week52AckBody {
    employee_id: Uuid,
    week_start: String,
}
async fn acknowledge_week52(
    State(state): State<AttendanceRestState>,
    headers: HeaderMap,
    Json(body): Json<Week52AckBody>,
) -> Result<Json<Week52RowDto>, RestError> {
    let p = principal(&state, &headers).await?;
    let branch = state
        .store
        .active_employee_home_branch(*p.org_id.as_uuid(), body.employee_id)
        .await
        .map_err(RestError::store)?;
    require_resource_branch(&p, EXCEPTION_MANAGE, branch)?;
    let command = AcknowledgeWeek52 {
        employee_id: body.employee_id,
        week_start: validate_week52_start(parse_date(&body.week_start, "weekStart")?).map_err(
            |e| {
                RestError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation",
                    e.to_string(),
                )
            },
        )?,
    };
    state
        .store
        .acknowledge_week52(&scope(&p), command.clone())
        .await
        .map_err(RestError::store)?;
    let row = state
        .store
        .week52_inputs(&scope(&p), command.week_start, branch)
        .await
        .map_err(RestError::store)?
        .into_iter()
        .find(|row| row.employee_id == command.employee_id)
        .ok_or_else(|| {
            RestError::new(StatusCode::NOT_FOUND, "not_found", "resource was not found")
        })?;
    Ok(Json(Week52RowDto::from(row)))
}

fn idempotency(headers: &HeaderMap) -> Result<String, RestError> {
    headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| {
            RestError::kernel(KernelError::validation(
                "Idempotency-Key header is required",
            ))
        })
}

#[derive(Debug)]
struct RestError {
    status: StatusCode,
    code: &'static str,
    message: String,
}
impl RestError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    fn malformed_query() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "query parameters are invalid",
        )
    }
    fn kernel(error: KernelError) -> Self {
        match error.kind {
            ErrorKind::Validation => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                error.message,
            ),
            ErrorKind::Forbidden => Self::new(StatusCode::FORBIDDEN, "forbidden", error.message),
            ErrorKind::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", error.message),
            ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                Self::new(StatusCode::CONFLICT, "conflict", error.message)
            }
            ErrorKind::Internal => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal server error",
            ),
        }
    }
    fn store(error: AttendanceStoreError) -> Self {
        match error {
            AttendanceStoreError::Application(e) => match e {
                console_attendance_application::AttendanceApplicationError::ForbiddenBranch => {
                    Self::new(StatusCode::FORBIDDEN, "forbidden", e.to_string())
                }
                _ => Self::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation",
                    e.to_string(),
                ),
            },
            AttendanceStoreError::NotFound => {
                Self::new(StatusCode::NOT_FOUND, "not_found", "resource was not found")
            }
            AttendanceStoreError::Conflict => Self::new(
                StatusCode::CONFLICT,
                "conflict",
                "conflicting attendance state",
            ),
            AttendanceStoreError::CloseBlocked => Self::new(
                StatusCode::CONFLICT,
                "close_blocked",
                "open exceptions or a prior close block this close",
            ),
            AttendanceStoreError::InvalidCloseMonth => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                "close month is outside the supported date range",
            ),
            AttendanceStoreError::Database(_) | AttendanceStoreError::Sql(_) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal server error",
            ),
        }
    }
}
impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error":{"code":self.code,"message":self.message}})),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_kernel_core::{OrgId, UserId};
    use console_platform_authz::Role;
    use std::{
        collections::BTreeSet,
        future::Future,
        task::{Context, Poll, Waker},
    };

    fn run_ready<F: Future>(future: F) -> F::Output {
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        let mut future = std::pin::pin!(future);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("query extraction must complete without waiting"),
        }
    }

    fn assert_invalid_query<T>(uri: &str)
    where
        T: DeserializeOwned + Send,
    {
        let (mut parts, _) = axum::http::Request::builder()
            .uri(uri)
            .body(())
            .unwrap()
            .into_parts();
        let error = match run_ready(AttendanceQuery::<T>::from_request_parts(&mut parts, &())) {
            Ok(_) => panic!("unknown query parameter must be rejected"),
            Err(error) => error,
        };
        assert_eq!(error.status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(error.code, "invalid_query", "{uri}");
    }

    const BRANCH_BOUND_ENDPOINT_FAMILIES: &[&str] = &[
        "substitutions list",
        "exceptions list",
        "exceptions raise",
        "substitutions assign",
        "substitution candidates",
        "close preflight",
        "close confirm",
        "week52",
        "exception detail resource lookup",
        "exception resolve resource lookup",
        "substitution cancel resource lookup",
        "close list",
        "close amendment resource lookup",
        "week52 acknowledgement",
    ];
    #[test]
    fn explicit_range_needs_both_bounds() {
        let q = ListQuery {
            month: Some("2026-07".to_owned()),
            from_date: Some("2026-07-01".to_owned()),
            to_date: None,
            work_date: None,
            status: None,
            employee_id: None,
            branch_id: None,
            limit: None,
            offset: None,
        };
        assert!(list_range(&q).is_err());
    }
    #[test]
    fn no_implicit_unbounded_listing() {
        let q = ListQuery {
            month: Some("2026-07".to_owned()),
            from_date: None,
            to_date: None,
            work_date: None,
            status: None,
            employee_id: None,
            branch_id: None,
            limit: None,
            offset: None,
        };
        let r = list_range(&q).unwrap();
        assert_eq!(r.to_exclusive.to_string(), "2026-08-08");
    }

    #[test]
    fn week52_ack_rejects_client_supplied_branch() {
        assert!(serde_json::from_value::<Week52AckBody>(json!({"employeeId":Uuid::new_v4(),"weekStart":"2026-07-06","branchId":Uuid::new_v4()})).is_err());
    }

    #[test]
    fn malformed_query_uses_a_stable_json_error_contract() {
        let error = RestError::malformed_query();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.code, "invalid_query");
        assert_eq!(error.message, "query parameters are invalid");
    }

    #[test]
    fn invalid_close_month_is_a_stable_validation_error() {
        let error = RestError::store(AttendanceStoreError::InvalidCloseMonth);
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error.code, "validation");
        assert_eq!(
            error.message,
            "close month is outside the supported date range"
        );
    }

    #[test]
    fn every_query_endpoint_maps_unknown_parameters_to_the_stable_error() {
        assert_invalid_query::<ListQuery>("/api/v1/attendance/substitutions?unexpected=true");
        assert_invalid_query::<SubstitutionCandidateListQuery>(
            "/api/v1/attendance/substitution-candidates?unexpected=true",
        );
        assert_invalid_query::<ListQuery>("/api/v1/attendance/exceptions?unexpected=true");
        assert_invalid_query::<CloseListQuery>("/api/v1/attendance/closes?unexpected=true");
        assert_invalid_query::<Week52Query>("/api/v1/attendance/week52?unexpected=true");
        assert_invalid_query::<self_service::OwnExceptionsQuery>(
            "/api/v1/attendance/me/exceptions?employee_id=00000000-0000-0000-0000-000000000001",
        );
        assert_invalid_query::<self_service::OwnExceptionsQuery>(
            "/api/v1/attendance/me/exceptions?branch_id=00000000-0000-0000-0000-000000000001",
        );
        assert_invalid_query::<self_service::OwnWeek52Query>(
            "/api/v1/attendance/me/week52?employee_id=00000000-0000-0000-0000-000000000001&week_start=2026-07-20",
        );
        assert_invalid_query::<self_service::OwnWeek52Query>(
            "/api/v1/attendance/me/week52?branch_id=00000000-0000-0000-0000-000000000001&week_start=2026-07-20",
        );
    }

    #[test]
    fn attendance_read_surfaces_are_static_and_complete() {
        assert_eq!(
            ATTENDANCE_READ_SURFACES,
            [
                "substitutions",
                "substitution_candidates",
                "exceptions",
                "exception_detail",
                "closes",
                "week52",
                "me_exceptions",
                "me_week52",
            ]
        );
    }

    #[test]
    fn week52_read_and_ack_reject_non_monday_week_starts() {
        let sunday = parse_date("2026-07-19", "weekStart").unwrap();
        assert!(validate_week52_start(sunday).is_err());
        let monday = parse_date("2026-07-20", "weekStart").unwrap();
        assert!(validate_week52_start(monday).is_ok());
    }

    #[test]
    fn self_service_paths_extend_but_do_not_change_manager_route_inventory() {
        assert_eq!(
            ATTENDANCE_ROUTE_PATHS.len(),
            13,
            "some paths host two method-specific operations"
        );
        assert!(ATTENDANCE_ROUTE_PATHS.contains(&ATTENDANCE_EXCEPTION_DETAIL_PATH));
        assert!(ATTENDANCE_ROUTE_PATHS.contains(&ATTENDANCE_SUBSTITUTION_CANDIDATES_PATH));
        assert!(ATTENDANCE_ROUTE_PATHS.contains(&ATTENDANCE_SUBSTITUTION_CANCEL_PATH));
        assert!(ATTENDANCE_ROUTE_PATHS.contains(&ATTENDANCE_CLOSE_AMEND_PATH));
        assert!(ATTENDANCE_ROUTE_PATHS.contains(&ATTENDANCE_WEEK52_ACK_PATH));
        assert_eq!(
            ATTENDANCE_CLOSE_AMEND_PATH,
            "/api/v1/attendance/closes/{close_id}/amendments"
        );
        assert_eq!(ATTENDANCE_WEEK52_ACK_PATH, "/api/v1/attendance/week52/acks");
        assert_eq!(
            ATTENDANCE_ME_EXCEPTIONS_PATH,
            "/api/v1/attendance/me/exceptions"
        );
        assert_eq!(ATTENDANCE_ME_WEEK52_PATH, "/api/v1/attendance/me/week52");
        assert!(
            !ATTENDANCE_ROUTE_PATHS.contains(&"/api/v1/attendance/closes/{close_id}/amend"),
            "the legacy singular amendment route must not remain mounted"
        );
        assert!(
            !ATTENDANCE_ROUTE_PATHS.contains(&"/api/v1/attendance/week52/ack"),
            "the legacy singular acknowledgement route must not remain mounted"
        );
        assert_eq!(BRANCH_BOUND_ENDPOINT_FAMILIES.len(), 14);
        assert!(
            BRANCH_BOUND_ENDPOINT_FAMILIES.contains(&"exception resolve resource lookup"),
            "resolve is branch-bound; extra query coverage is not a separate family"
        );
        assert!(ATTENDANCE_ROUTE_PATHS.contains(&ATTENDANCE_EXCEPTION_RESOLVE_PATH));
        let confirm = serde_json::from_value::<ResolveBody>(json!({
            "action": "CONFIRM",
            "reason": "verified arrival"
        }))
        .expect("CONFIRM resolve body is valid wire input");
        assert_eq!(
            ResolutionAction::parse(&confirm.action),
            Ok(ResolutionAction::Confirm)
        );
        assert_eq!(
            parse_resolve_action(&confirm.action).expect("CONFIRM is a domain action"),
            ResolutionAction::Confirm
        );
        let overtime = serde_json::from_value::<ResolveBody>(json!({
            "action": "APPROVE_OVERTIME",
            "reason": "approved"
        }))
        .expect("APPROVE_OVERTIME resolve body is valid wire input");
        assert_eq!(
            ResolutionAction::parse(&overtime.action),
            Ok(ResolutionAction::ApproveOvertime)
        );
        assert_eq!(
            parse_resolve_action(&overtime.action).expect("APPROVE_OVERTIME is a domain action"),
            ResolutionAction::ApproveOvertime
        );
        assert_eq!(
            ResolutionAction::parse("CONFIRM"),
            Ok(ResolutionAction::Confirm)
        );
        assert!(
            ResolutionAction::parse("confirm").is_err(),
            "resolution actions are exact SCREAMING_SNAKE_CASE tokens"
        );
        assert!(
            ResolutionAction::parse("HOLD").is_err(),
            "HOLD is not an attendance ResolutionAction"
        );
        let hold = serde_json::from_value::<ResolveBody>(json!({
            "action": "HOLD",
            "reason": "park until next review"
        }))
        .expect("HOLD is valid wire shape; action parse is separate");
        let hold_err =
            parse_resolve_action(&hold.action).expect_err("handler maps HOLD through parse to 422");
        assert_eq!(hold_err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(hold_err.code, "validation");
        let empty_hold = serde_json::from_value::<ResolveBody>(json!({
            "action": "HOLD",
            "reason": ""
        }))
        .expect("empty reason still deserializes at ResolveBody");
        let empty_hold_err = parse_resolve_action(&empty_hold.action)
            .expect_err("empty reason + HOLD is rejected at the handler parse path");
        assert_eq!(empty_hold_err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(empty_hold_err.code, "validation");
        assert!(
            serde_json::from_value::<ResolveBody>(json!({ "action": "HOLD" })).is_err(),
            "ResolveBody requires a reason field for every action"
        );
        for token in ["approve", "YES", "confirm"] {
            assert!(
                ResolutionAction::parse(token).is_err(),
                "{token} is not an exact SCREAMING_SNAKE_CASE resolution token"
            );
            let err = parse_resolve_action(token).expect_err(token);
            assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY, "{token}");
            assert_eq!(err.code, "validation", "{token}");
        }
    }

    #[test]
    fn candidate_query_requires_every_strict_selector() {
        let branch = Uuid::new_v4();
        let covered = Uuid::new_v4();
        let query = format!(
            "/api/v1/attendance/substitution-candidates?branch_id={branch}&covered_employee_id={covered}&cover_date=2026-07-24&from_minutes=480&to_minutes=960"
        );
        let (mut parts, _) = axum::http::Request::builder()
            .uri(&query)
            .body(())
            .unwrap()
            .into_parts();
        let parsed = run_ready(
            AttendanceQuery::<SubstitutionCandidateListQuery>::from_request_parts(&mut parts, &()),
        )
        .expect("complete candidate query is valid")
        .0;
        assert_eq!(parsed.branch_id, branch);
        assert_eq!(parsed.covered_employee_id, covered);

        assert_invalid_query::<SubstitutionCandidateListQuery>(
            "/api/v1/attendance/substitution-candidates?covered_employee_id=00000000-0000-0000-0000-000000000001&cover_date=2026-07-24&from_minutes=480&to_minutes=960",
        );
    }

    #[test]
    fn candidate_lookup_uses_substitution_manage_at_the_explicit_branch() {
        let allowed = BranchId::new();
        let principal = Principal::new(
            UserId::new(),
            OrgId::knl(),
            BTreeSet::from([Role::Admin]),
            BranchScope::single(allowed),
        );
        assert!(
            require_for_branch(&principal, SUBSTITUTION_MANAGE, Some(*allowed.as_uuid())).is_ok()
        );
        assert_eq!(
            require_for_branch(&principal, SUBSTITUTION_MANAGE, Some(Uuid::new_v4()),)
                .unwrap_err()
                .status,
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn candidate_dto_serializes_the_stable_public_fields() {
        let candidate = SubstitutionCandidateDto::from(SubstitutionCandidateRead {
            employee_id: Uuid::nil(),
            employee_name: "Ada Kim".to_owned(),
            branch_id: Uuid::from_u128(1),
        });
        assert_eq!(
            serde_json::to_value(candidate).unwrap(),
            json!({
                "employee_id": Uuid::nil().to_string(),
                "employee_name": "Ada Kim",
                "branch_id": Uuid::from_u128(1).to_string(),
            })
        );
    }

    #[test]
    fn assignment_body_rejects_client_owned_worker_snapshots() {
        let body = json!({
            "site": "Main",
            "role": "Guard",
            "cover_date": "2026-07-24",
            "from_minutes": 480,
            "to_minutes": 960,
            "covered_employee_id": Uuid::new_v4(),
            "reason_kind": "LEAVE",
            "worker_employee_id": Uuid::new_v4(),
            "worker_name": "client cannot choose this"
        });
        assert!(serde_json::from_value::<AssignBody>(body).is_err());
    }

    #[test]
    fn http_boundary_allows_an_explicit_allowed_branch_for_every_endpoint_family() {
        let branch = BranchId::new();
        let scope = BranchScope::Branches(BTreeSet::from([branch]));
        for family in BRANCH_BOUND_ENDPOINT_FAMILIES {
            assert!(
                branch_request_is_well_scoped(&scope, Some(*branch.as_uuid())),
                "{family}"
            );
            assert!(scope.allows(branch), "{family}");
        }
    }

    #[test]
    fn http_boundary_rejects_a_foreign_branch_for_every_endpoint_family() {
        let allowed = BranchId::new();
        let foreign = BranchId::new();
        let scope = BranchScope::Branches(BTreeSet::from([allowed]));
        for family in BRANCH_BOUND_ENDPOINT_FAMILIES {
            assert!(
                branch_request_is_well_scoped(&scope, Some(*foreign.as_uuid())),
                "{family}"
            );
            assert!(!scope.allows(foreign), "{family}");
        }
    }

    #[test]
    fn http_boundary_rejects_omitted_branch_for_limited_scope_and_allows_org_scope() {
        let limited = BranchScope::single(BranchId::new());
        for family in BRANCH_BOUND_ENDPOINT_FAMILIES {
            assert!(!branch_request_is_well_scoped(&limited, None), "{family}");
            assert!(
                branch_request_is_well_scoped(&BranchScope::All, None),
                "{family}"
            );
        }
    }
    #[test]
    fn list_query_modes_are_mutually_exclusive() {
        let q = |month: Option<&str>, from: Option<&str>, to: Option<&str>| ListQuery {
            month: month.map(str::to_owned),
            from_date: from.map(str::to_owned),
            to_date: to.map(str::to_owned),
            work_date: None,
            status: None,
            employee_id: None,
            branch_id: None,
            limit: None,
            offset: None,
        };
        assert!(list_range(&q(Some("2026-07"), None, None)).is_ok());
        assert!(list_range(&q(None, Some("2026-07-01"), Some("2026-07-08"))).is_ok());
        assert!(list_range(&q(None, None, None)).is_err());
        assert!(list_range(&q(Some("2026-07"), Some("2026-07-01"), Some("2026-07-08"))).is_err());
        assert!(list_range(&q(None, Some("2026-07-01"), None)).is_err());
    }
    #[test]
    fn work_date_selector_is_exclusive() {
        let q = |month: Option<&str>, work_date: Option<&str>| ListQuery {
            month: month.map(str::to_owned),
            from_date: None,
            to_date: None,
            work_date: work_date.map(str::to_owned),
            status: None,
            employee_id: None,
            branch_id: None,
            limit: None,
            offset: None,
        };
        let range = list_range(&q(None, Some("2026-07-09"))).unwrap();
        assert_eq!(range.from.to_string(), "2026-07-09");
        assert_eq!(range.to_exclusive.to_string(), "2026-07-10");
        assert!(list_range(&q(Some("2026-07"), Some("2026-07-09"))).is_err());
    }
}
