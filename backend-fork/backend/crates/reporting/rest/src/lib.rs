//! Reporting REST API.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_kernel_core::{BranchId, ErrorKind, KernelError, RegionId, TraceContext, UserId};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{
    Action, Feature, Principal, authorize, authorize_capability, authorize_org_wide,
};
use console_reporting_adapter_postgres::PgKpiRepository;
use console_reporting_application::{
    KpiExportQuery, KpiQuery, KpiQueryError, KpiQueryPort, KpiScope, OpsSummaryPort,
    OpsSummaryQuery, Period, ReportingExportError, ReportingExportPort, ReportingExportQuery,
    WorkDiaryBody, WorkDiaryConfirmCommand, WorkDiaryDraft, WorkDiaryDraftPort, WorkDiaryQuery,
    WorkDiaryUpdateCommand,
};
use serde::{Deserialize, Serialize};
use time::macros::format_description;
use time::{Date, Time};

pub const KPI_PATH: &str = "/api/v1/kpi";
pub const OPS_SUMMARY_PATH: &str = "/api/v1/ops/summary";
pub const DAILY_STATUS_EXPORT_PATH: &str = "/api/v1/exports/daily-status";
pub const WORK_DIARY_EXPORT_PATH: &str = "/api/v1/exports/work-diary";
pub const KPI_EXPORT_PATH: &str = "/api/v1/exports/kpi";
pub const WORK_DIARY_PATH: &str = "/api/v1/reporting/work-diary";
pub const WORK_DIARY_CONFIRM_PATH: &str = "/api/v1/reporting/work-diary/confirm";
pub const KPI_ROUTE_PATHS: &[&str] = &[
    KPI_PATH,
    OPS_SUMMARY_PATH,
    DAILY_STATUS_EXPORT_PATH,
    WORK_DIARY_EXPORT_PATH,
    KPI_EXPORT_PATH,
    WORK_DIARY_PATH,
    WORK_DIARY_CONFIRM_PATH,
];

/// Aging threshold (hours) past which an unresolved work order is "aging".
const OPS_AGING_HOURS: u32 = 24;
/// Lead time (minutes) before a P1 accept-window deadline to flag "at risk".
const OPS_AT_RISK_MINUTES: u32 = 5;
/// Cap on the mechanic-utilization list.
const OPS_TOP_MECHANICS: u32 = 10;

#[derive(Debug, Clone)]
pub struct KpiRestState {
    repository: PgKpiRepository,
    jwt_verifier: Option<JwtVerifier>,
}

impl KpiRestState {
    #[must_use]
    pub fn new(repository: PgKpiRepository, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            repository,
            jwt_verifier,
        }
    }
}

pub fn router(state: KpiRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.repository.pool().clone();
    let router = Router::new()
        .route(KPI_PATH, get(get_kpis))
        .route(OPS_SUMMARY_PATH, get(get_ops_summary))
        .route(DAILY_STATUS_EXPORT_PATH, get(get_daily_status_export))
        .route(WORK_DIARY_EXPORT_PATH, get(get_work_diary_export))
        .route(KPI_EXPORT_PATH, get(get_kpi_export))
        .route(WORK_DIARY_PATH, get(get_work_diary).put(update_work_diary))
        .route(WORK_DIARY_CONFIRM_PATH, post(confirm_work_diary))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
struct KpiRequestQuery {
    period: String,
    scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DateRequestQuery {
    date: String,
}

#[derive(Debug, Deserialize)]
struct WorkDiaryUpdateRequest {
    body: WorkDiaryBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
}

#[derive(Debug)]
struct RestError {
    status: StatusCode,
    kind: ErrorKind,
    message: String,
}

impl RestError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            kind: ErrorKind::Validation,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            kind: ErrorKind::Forbidden,
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            kind: ErrorKind::Internal,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            kind: ErrorKind::Internal,
            message: message.into(),
        }
    }

    fn from_kernel(error: KernelError) -> Self {
        Self {
            status: status_for_error_kind(error.kind),
            kind: error.kind,
            message: error.message,
        }
    }

    fn from_query(error: KpiQueryError) -> Self {
        match error {
            KpiQueryError::Kernel(error) => Self::from_kernel(error),
            KpiQueryError::Database(message) => Self::internal(message),
        }
    }

    fn from_export(error: ReportingExportError) -> Self {
        match error {
            ReportingExportError::Kernel(error) => Self::from_kernel(error),
            ReportingExportError::Database(message) | ReportingExportError::Workbook(message) => {
                Self::internal(message)
            }
        }
    }

    fn code(&self) -> &'static str {
        match self.kind {
            ErrorKind::Validation => "validation",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Forbidden => "forbidden",
            ErrorKind::Conflict => "conflict",
            ErrorKind::InvalidTransition => "invalid_transition",
            ErrorKind::Internal => "internal",
        }
    }
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: ErrorPayload {
                    code: self.code(),
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

async fn get_kpis(
    State(state): State<KpiRestState>,
    headers: HeaderMap,
    Query(params): Query<KpiRequestQuery>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let period = parse_period(&params.period)?;
    let scope = parse_scope(params.scope.as_deref())?;
    authorize_kpi_scope(&principal, scope)?;

    let report = state
        .repository
        .query_kpis(KpiQuery {
            period,
            scope,
            branch_scope: principal.branch_scope,
        })
        .await
        .map_err(RestError::from_query)?;

    Ok(Json(report))
}

/// GET /api/v1/ops/summary — per-tenant operational rollup.
///
/// Org-scoped under RLS: every aggregate is computed inside
/// `with_org_conn(current_org())`, so a second tenant's rows are never counted.
/// The query carries no branch scope and therefore requires org-wide authority;
/// a branch-scoped ADMIN must not receive another branch's aggregate facts.
async fn get_ops_summary(
    State(state): State<KpiRestState>,
    headers: HeaderMap,
) -> Result<Json<console_reporting_application::OpsSummary>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_ops_summary(&principal)?;

    let summary = state
        .repository
        .ops_summary(OpsSummaryQuery {
            aging_hours: OPS_AGING_HOURS,
            at_risk_minutes: OPS_AT_RISK_MINUTES,
            top_mechanics: OPS_TOP_MECHANICS,
        })
        .await
        .map_err(RestError::from_query)?;

    Ok(Json(summary))
}

async fn get_daily_status_export(
    State(state): State<KpiRestState>,
    headers: HeaderMap,
    Query(params): Query<DateRequestQuery>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let date = parse_date(&params.date)?;
    authorize_reporting_feature(&principal, Feature::ExcelDownload)?;
    let workbook = state
        .repository
        .export_daily_status(export_query(&principal, date))
        .await
        .map_err(RestError::from_export)?;
    workbook_response(workbook.file_name, workbook.content_type, workbook.bytes)
}

async fn get_work_diary_export(
    State(state): State<KpiRestState>,
    headers: HeaderMap,
    Query(params): Query<DateRequestQuery>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let date = parse_date(&params.date)?;
    authorize_reporting_feature(&principal, Feature::ExcelDownload)?;
    let workbook = state
        .repository
        .export_work_diary(export_query(&principal, date))
        .await
        .map_err(RestError::from_export)?;
    workbook_response(workbook.file_name, workbook.content_type, workbook.bytes)
}

async fn get_kpi_export(
    State(state): State<KpiRestState>,
    headers: HeaderMap,
    Query(params): Query<KpiRequestQuery>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let period = parse_period(&params.period)?;
    let scope = parse_scope(params.scope.as_deref())?;
    // KPI export exposes the same KpiRead-gated data as GET /api/v1/kpi, so it
    // requires the KPI read contract (feature + scope) in addition to the
    // Excel-download capability the sibling exports need.
    authorize_kpi_scope(&principal, scope)?;
    authorize_reporting_feature(&principal, Feature::ExcelDownload)?;
    let workbook = state
        .repository
        .export_kpi(KpiExportQuery {
            actor: principal.user_id,
            period,
            scope,
            branch_scope: principal.branch_scope.clone(),
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_export)?;
    workbook_response(workbook.file_name, workbook.content_type, workbook.bytes)
}

async fn get_work_diary(
    State(state): State<KpiRestState>,
    headers: HeaderMap,
    Query(params): Query<DateRequestQuery>,
) -> Result<Json<WorkDiaryDraft>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let date = parse_date(&params.date)?;
    authorize_reporting_feature(&principal, Feature::DailyPlanReview)?;
    let draft = state
        .repository
        .get_or_generate_work_diary(WorkDiaryQuery {
            actor: principal.user_id,
            date,
            branch_scope: principal.branch_scope,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_export)?;
    Ok(Json(draft))
}

async fn update_work_diary(
    State(state): State<KpiRestState>,
    headers: HeaderMap,
    Query(params): Query<DateRequestQuery>,
    Json(body): Json<WorkDiaryUpdateRequest>,
) -> Result<Json<WorkDiaryDraft>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let date = parse_date(&params.date)?;
    authorize_reporting_feature(&principal, Feature::DailyPlanReview)?;
    let draft = state
        .repository
        .update_work_diary(WorkDiaryUpdateCommand {
            actor: principal.user_id,
            date,
            branch_scope: principal.branch_scope,
            body: body.body,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_export)?;
    Ok(Json(draft))
}

async fn confirm_work_diary(
    State(state): State<KpiRestState>,
    headers: HeaderMap,
    Query(params): Query<DateRequestQuery>,
) -> Result<Json<WorkDiaryDraft>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let date = parse_date(&params.date)?;
    authorize_reporting_feature(&principal, Feature::DailyPlanReview)?;
    let draft = state
        .repository
        .confirm_work_diary(WorkDiaryConfirmCommand {
            actor: principal.user_id,
            date,
            branch_scope: principal.branch_scope,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_export)?;
    Ok(Json(draft))
}

fn export_query(principal: &Principal, date: Date) -> ReportingExportQuery {
    ReportingExportQuery {
        actor: principal.user_id,
        date,
        branch_scope: principal.branch_scope.clone(),
        trace: TraceContext::generate(),
        occurred_at: time::OffsetDateTime::now_utc(),
    }
}

fn workbook_response(
    file_name: String,
    content_type: &'static str,
    bytes: Vec<u8>,
) -> Result<Response, RestError> {
    let mut response = bytes.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    let disposition = format!("attachment; filename=\"{file_name}\"");
    let value = HeaderValue::from_str(&disposition)
        .map_err(|_| RestError::internal("export filename could not be encoded as a header"))?;
    response
        .headers_mut()
        .insert(header::CONTENT_DISPOSITION, value);
    Ok(response)
}

fn authorize_reporting_feature(principal: &Principal, feature: Feature) -> Result<(), RestError> {
    // Export/work-diary repositories receive the principal's complete branch
    // scope and filter their reads/writes with it. This is a capability
    // preflight, not authorization against one invented representative branch.
    authorize_capability(principal, Action::new(feature)).map_err(RestError::from_kernel)
}

fn authorize_ops_summary(principal: &Principal) -> Result<(), RestError> {
    authorize_org_wide(principal, Action::new(Feature::OpsDashboardRead))
        .map_err(RestError::from_kernel)
}

fn parse_period(raw: &str) -> Result<Period, RestError> {
    let (start_raw, end_raw) = raw
        .split_once("..")
        .ok_or_else(|| RestError::bad_request("period must use YYYY-MM-DD..YYYY-MM-DD"))?;
    let format = format_description!("[year]-[month]-[day]");
    let start_date = Date::parse(start_raw, &format)
        .map_err(|_| RestError::bad_request("period start must use YYYY-MM-DD"))?;
    let end_date = Date::parse(end_raw, &format)
        .map_err(|_| RestError::bad_request("period end must use YYYY-MM-DD"))?;
    let period = Period {
        start: start_date.with_time(Time::MIDNIGHT).assume_utc(),
        end: end_date.with_time(Time::MIDNIGHT).assume_utc(),
    };
    if period.start >= period.end {
        return Err(RestError::bad_request(
            "period start must be before period end",
        ));
    }
    Ok(period)
}

fn parse_date(raw: &str) -> Result<Date, RestError> {
    let format = format_description!("[year]-[month]-[day]");
    Date::parse(raw, &format).map_err(|_| RestError::bad_request("date must use YYYY-MM-DD"))
}

fn parse_scope(raw: Option<&str>) -> Result<KpiScope, RestError> {
    let Some(raw) = raw.filter(|value| !value.trim().is_empty()) else {
        return Ok(KpiScope::Company);
    };
    if raw == "company" {
        return Ok(KpiScope::Company);
    }
    let (kind, id) = raw.split_once(':').ok_or_else(|| {
        RestError::bad_request(
            "scope must be company, region:<id>, branch:<id>, or technician:<id>",
        )
    })?;
    let id =
        uuid::Uuid::parse_str(id).map_err(|_| RestError::bad_request("scope id must be a UUID"))?;
    match kind {
        "region" => Ok(KpiScope::Region(RegionId::from_uuid(id))),
        "branch" => Ok(KpiScope::Branch(BranchId::from_uuid(id))),
        "technician" => Ok(KpiScope::Technician(UserId::from_uuid(id))),
        _ => Err(RestError::bad_request(
            "scope must be company, region:<id>, branch:<id>, or technician:<id>",
        )),
    }
}

fn authorize_kpi_scope(principal: &Principal, scope: KpiScope) -> Result<(), RestError> {
    let action = Action::new(Feature::KpiRead);
    match scope {
        // A branch named by the request is a concrete resource branch.
        KpiScope::Branch(branch_id) => authorize(principal, action, branch_id),
        // These reports are further confined by the principal's complete scope
        // in the repository. Picking one member of that scope here would make
        // the branch check vacuous and custom-grant behavior order-dependent.
        KpiScope::Company | KpiScope::Region(_) | KpiScope::Technician(_) => {
            authorize_capability(principal, action)
        }
    }
    .map_err(RestError::from_kernel)
}

async fn principal_from_headers(
    state: &KpiRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state
        .jwt_verifier
        .as_ref()
        .ok_or_else(|| RestError::unavailable("JWT verification is not configured for KPI API"))?;
    console_platform_request_context::resolve_principal(verifier, state.repository.pool(), headers)
        .await
        .map_err(rest_error_from_request_context)
}

fn rest_error_from_request_context(
    err: console_platform_request_context::RequestContextError,
) -> RestError {
    match err {
        console_platform_request_context::RequestContextError::VerifierUnavailable => {
            RestError::unavailable("JWT verification is not configured for KPI API")
        }
        console_platform_request_context::RequestContextError::WrongTokenTier => {
            RestError::from_kernel(KernelError::forbidden(
                "token tier is not valid for this route",
            ))
        }
        console_platform_request_context::RequestContextError::AccessScope(error) => {
            RestError::from_kernel(error)
        }
        console_platform_request_context::RequestContextError::BranchScope(message)
        | console_platform_request_context::RequestContextError::EffectivePolicy(message) => {
            RestError::internal(message)
        }
        console_platform_request_context::RequestContextError::MissingOrg => {
            RestError::internal("no tenant context is bound to the current request")
        }
        console_platform_request_context::RequestContextError::MissingBearer => {
            RestError::unauthorized("missing or malformed bearer token")
        }
        console_platform_request_context::RequestContextError::InvalidToken => {
            RestError::unauthorized("invalid bearer token")
        }
        console_platform_request_context::RequestContextError::InvalidClaim(message) => {
            RestError::unauthorized(format!("token claim is invalid: {message}"))
        }
    }
}

fn status_for_error_kind(kind: ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Forbidden => StatusCode::FORBIDDEN,
        ErrorKind::Conflict => StatusCode::CONFLICT,
        ErrorKind::InvalidTransition => StatusCode::CONFLICT,
        ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::{authorize_kpi_scope, authorize_ops_summary, authorize_reporting_feature};
    use console_kernel_core::{BranchId, BranchScope, OrgId, UserId};
    use console_platform_authz::{
        EffectiveFeatureGrant, Feature, PermissionLevel, Principal, Role,
    };
    use console_reporting_application::KpiScope;
    use std::collections::BTreeSet;

    fn principal(roles: BTreeSet<Role>, scope: BranchScope) -> Principal {
        Principal::new(UserId::new(), OrgId::new(), roles, scope)
    }

    #[test]
    fn kpi_scope_uses_the_requested_branch_or_the_complete_principal_scope() {
        let branch_a = BranchId::new();
        let branch_b = BranchId::new();
        let admin = principal(BTreeSet::from([Role::Admin]), BranchScope::single(branch_a));

        assert!(authorize_kpi_scope(&admin, KpiScope::Branch(branch_a)).is_ok());
        assert!(authorize_kpi_scope(&admin, KpiScope::Branch(branch_b)).is_err());
        assert!(authorize_kpi_scope(&admin, KpiScope::Company).is_ok());

        let all_admin = principal(BTreeSet::from([Role::Admin]), BranchScope::All);
        assert!(authorize_kpi_scope(&all_admin, KpiScope::Company).is_ok());

        let empty = principal(BTreeSet::from([Role::Admin]), BranchScope::none());
        assert!(authorize_kpi_scope(&empty, KpiScope::Company).is_err());

        let two_branches = BranchScope::Branches(BTreeSet::from([branch_a, branch_b]));
        let partial = principal(BTreeSet::new(), two_branches.clone())
            .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
                Feature::KpiRead,
                PermissionLevel::Allow,
                BranchScope::single(branch_a),
            )]);
        assert!(
            authorize_kpi_scope(&partial, KpiScope::Company).is_err(),
            "a partial grant must not authorize a report over the complete visible scope"
        );

        let covering =
            principal(BTreeSet::new(), two_branches.clone()).with_effective_feature_grants(vec![
                EffectiveFeatureGrant::new(Feature::KpiRead, PermissionLevel::Allow, two_branches),
            ]);
        assert!(authorize_kpi_scope(&covering, KpiScope::Company).is_ok());
    }

    #[test]
    fn scoped_reporting_features_do_not_become_org_wide_only() {
        let branch = BranchId::new();
        let branch_admin = principal(BTreeSet::from([Role::Admin]), BranchScope::single(branch));
        assert!(authorize_reporting_feature(&branch_admin, Feature::DailyPlanReview).is_ok());
        assert!(authorize_reporting_feature(&branch_admin, Feature::ExcelDownload).is_ok());

        let all_admin = principal(BTreeSet::from([Role::Admin]), BranchScope::All);
        assert!(authorize_reporting_feature(&all_admin, Feature::DailyPlanReview).is_ok());

        let empty = principal(BTreeSet::from([Role::Admin]), BranchScope::none());
        assert!(authorize_reporting_feature(&empty, Feature::ExcelDownload).is_err());
    }

    #[test]
    fn tenant_wide_ops_summary_requires_org_wide_authority() {
        let branch = BranchId::new();
        let branch_admin = principal(BTreeSet::from([Role::Admin]), BranchScope::single(branch));
        assert!(authorize_ops_summary(&branch_admin).is_err());

        let all_admin = principal(BTreeSet::from([Role::Admin]), BranchScope::All);
        assert!(authorize_ops_summary(&all_admin).is_err());

        let super_admin = principal(BTreeSet::from([Role::SuperAdmin]), BranchScope::All);
        assert!(authorize_ops_summary(&super_admin).is_ok());

        let custom_all = principal(BTreeSet::new(), BranchScope::All)
            .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
                Feature::OpsDashboardRead,
                PermissionLevel::Allow,
                BranchScope::All,
            )]);
        assert!(authorize_ops_summary(&custom_all).is_ok());

        let partial =
            principal(BTreeSet::new(), BranchScope::All).with_effective_feature_grants(vec![
                EffectiveFeatureGrant::new(
                    Feature::OpsDashboardRead,
                    PermissionLevel::Allow,
                    BranchScope::single(branch),
                ),
            ]);
        assert!(authorize_ops_summary(&partial).is_err());

        let empty = principal(BTreeSet::from([Role::SuperAdmin]), BranchScope::none());
        assert!(authorize_ops_summary(&empty).is_err());
    }
}
