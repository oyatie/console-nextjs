//! Read-only REST API for the payroll draft-run staging tables
//! (`payroll_draft_runs`/`payroll_draft_lines`, migration 0074).
//!
//! # What this is — and is NOT
//!
//! `payroll_draft_runs`/`payroll_draft_lines` are *pre-calculation readiness*
//! rows: work-day/hour counts and `*_source_present` booleans, gated
//! `BLOCKED_LEGAL_GATE` by default. They store **no won amount** — the real
//! per-employee deduction math (`console_payroll_domain::build_employee_payroll_draft`)
//! is a pure in-memory function with no persistence anywhere in this schema.
//! `/payslips/me` (`/me/lines` below) is therefore a self-service view of
//! draft READINESS data, not an issued payslip: it must not return vault
//! document fields (`kind`, `legal_basis`, `confirmed_at`, inbox doc id) or
//! won amounts. Issued 명세서 live in the statutory-notice vault
//! (`GET /api/v1/me/inbox-docs?filter=kind:payslip`, `console-inbox-rest`).
//!
//! # Branch scoping
//!
//! `payroll_draft_runs`/`payroll_draft_lines` have an `org_id` column only —
//! no `branch_id`. Admin reads are therefore gated org-wide
//! (`authorize_org_wide`): built-in access is EXECUTIVE/SUPER_ADMIN only —
//! `authorize_org_wide`'s built-in path never grants ADMIN, all-branch-scoped
//! or not (same as `EmployeeDirectoryRead`/`OrgWideQueueTriage` today; an
//! ADMIN's only path in is a custom org-wide PBAC grant). A branch-scoped
//! caller is denied (403) rather than silently granted org-wide visibility
//! mislabeled as "their branch".
//!
//! # Audit
//!
//! `/runs` and `/runs/{id}` read another person's compensation-adjacent data,
//! so each read is itself an audited event (`with_audits`), mirroring the
//! `office.rs::issue_session_version` / `lib.rs::audit_read_event` pattern.
//! `/me/lines` is a self-scoped read of the caller's own data — never
//! audited, mirroring `GET /api/v1/hr/attendance-records/me`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

mod lifecycle;
mod payslip_draft;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_kernel_core::{AuditAction, AuditEvent, ErrorKind, KernelError, TraceContext};
use console_payroll_adapter_postgres::{
    MyPayrollLinePage, PayrollRunDetail, PayrollRunPage, PayrollRunSummary, PgPayrollError,
    PgPayrollStore, get_run_in_tx, list_runs_in_tx,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize_org_wide};
use console_platform_db::{DbError, with_audits};
use console_platform_request_context::RequestContextError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PAYROLL_RUNS_PATH: &str = "/api/v1/payroll/runs";
pub const PAYROLL_RUN_PATH_TEMPLATE: &str = "/api/v1/payroll/runs/{id}";
pub const PAYROLL_MY_PAYSLIPS_PATH: &str = "/api/v1/payroll/payslips/me";
pub const PAYROLL_RUN_CLOSE_PREFLIGHT_PATH_TEMPLATE: &str =
    "/api/v1/payroll/runs/{id}/close-preflight";
pub const PAYROLL_RUN_CLOSE_ATTENDANCE_PATH_TEMPLATE: &str =
    "/api/v1/payroll/runs/{id}/close-attendance";
pub const PAYROLL_RUN_CALCULATE_PATH_TEMPLATE: &str = "/api/v1/payroll/runs/{id}/calculate";
pub const PAYROLL_RUN_EXCEPTIONS_PATH_TEMPLATE: &str = "/api/v1/payroll/runs/{id}/exceptions";
pub const PAYROLL_RUN_EXCEPTION_RESOLVE_PATH_TEMPLATE: &str =
    "/api/v1/payroll/runs/{id}/exceptions/{exception_id}/resolve";
pub const PAYROLL_RUN_SUBMIT_PATH_TEMPLATE: &str = "/api/v1/payroll/runs/{id}/submit";
pub const PAYROLL_RUN_DECISION_PATH_TEMPLATE: &str = "/api/v1/payroll/runs/{id}/decision";
pub const PAYROLL_RUN_WITHDRAW_PATH_TEMPLATE: &str = "/api/v1/payroll/runs/{id}/withdraw";
pub const PAYROLL_RUN_SCHEDULE_DISBURSEMENT_PATH_TEMPLATE: &str =
    "/api/v1/payroll/runs/{id}/schedule-disbursement";
pub const PAYROLL_RUN_DISBURSEMENT_ATTEST_PATH_TEMPLATE: &str =
    "/api/v1/payroll/runs/{id}/disbursement/attest";
pub const PAYROLL_RUN_ISSUE_PAYSLIPS_PATH_TEMPLATE: &str =
    "/api/v1/payroll/runs/{id}/issue-payslips";
pub const PAYROLL_RUN_PAYSLIP_DELIVERY_PATH_TEMPLATE: &str =
    "/api/v1/payroll/runs/{id}/payslip-delivery";

pub use payslip_draft::{
    PAYROLL_EMPLOYEE_CONTRACT_WAGES_PATH_TEMPLATE, PAYROLL_EMPLOYEE_PAYSLIP_DRAFT_PATH_TEMPLATE,
};

pub const PAYROLL_ROUTE_PATHS: &[&str] = &[
    PAYROLL_RUNS_PATH,
    PAYROLL_RUN_PATH_TEMPLATE,
    PAYROLL_MY_PAYSLIPS_PATH,
    PAYROLL_RUN_CLOSE_PREFLIGHT_PATH_TEMPLATE,
    PAYROLL_RUN_CLOSE_ATTENDANCE_PATH_TEMPLATE,
    PAYROLL_RUN_CALCULATE_PATH_TEMPLATE,
    PAYROLL_RUN_EXCEPTIONS_PATH_TEMPLATE,
    PAYROLL_RUN_EXCEPTION_RESOLVE_PATH_TEMPLATE,
    PAYROLL_RUN_SUBMIT_PATH_TEMPLATE,
    PAYROLL_RUN_DECISION_PATH_TEMPLATE,
    PAYROLL_RUN_WITHDRAW_PATH_TEMPLATE,
    PAYROLL_RUN_SCHEDULE_DISBURSEMENT_PATH_TEMPLATE,
    PAYROLL_RUN_DISBURSEMENT_ATTEST_PATH_TEMPLATE,
    PAYROLL_RUN_ISSUE_PAYSLIPS_PATH_TEMPLATE,
    PAYROLL_RUN_PAYSLIP_DELIVERY_PATH_TEMPLATE,
    PAYROLL_EMPLOYEE_PAYSLIP_DRAFT_PATH_TEMPLATE,
    PAYROLL_EMPLOYEE_CONTRACT_WAGES_PATH_TEMPLATE,
];

#[derive(Clone)]
pub struct PayrollRestState {
    store: PgPayrollStore,
    jwt_verifier: Option<JwtVerifier>,
}

impl std::fmt::Debug for PayrollRestState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PayrollRestState")
            .field("has_jwt_verifier", &self.jwt_verifier.is_some())
            .finish()
    }
}

impl PayrollRestState {
    #[must_use]
    pub fn new(store: PgPayrollStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
        }
    }

    /// SSR composition helper: the same `PayrollRunRead` listing as GET `/runs`,
    /// or an empty vec (omit) when the caller is unauthenticated, unauthorized,
    /// or the listing fails. Never a 401/403 on the HTML shell.
    pub async fn visible_run_summaries(&self, headers: &HeaderMap) -> Vec<PayrollRunSummary> {
        match list_runs_page(
            self,
            headers,
            PageParams {
                limit: Some(100),
                offset: Some(0),
            },
        )
        .await
        {
            Ok(page) => page.items,
            Err(_) => Vec::new(),
        }
    }
}

pub fn router(state: PayrollRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(PAYROLL_RUNS_PATH, get(list_runs))
        .route(PAYROLL_RUN_PATH_TEMPLATE, get(get_run))
        .route(PAYROLL_MY_PAYSLIPS_PATH, get(list_my_lines))
        .route(
            PAYROLL_RUN_CLOSE_PREFLIGHT_PATH_TEMPLATE,
            get(lifecycle::get_close_preflight),
        )
        .route(
            PAYROLL_RUN_CLOSE_ATTENDANCE_PATH_TEMPLATE,
            post(lifecycle::close_attendance),
        )
        .route(
            PAYROLL_RUN_CALCULATE_PATH_TEMPLATE,
            post(lifecycle::calculate_run),
        )
        .route(
            PAYROLL_RUN_EXCEPTIONS_PATH_TEMPLATE,
            get(lifecycle::list_exceptions),
        )
        .route(
            PAYROLL_RUN_EXCEPTION_RESOLVE_PATH_TEMPLATE,
            post(lifecycle::resolve_exception),
        )
        .route(
            PAYROLL_RUN_SUBMIT_PATH_TEMPLATE,
            post(lifecycle::submit_run),
        )
        .route(
            PAYROLL_RUN_DECISION_PATH_TEMPLATE,
            post(lifecycle::decide_run),
        )
        .route(
            PAYROLL_RUN_WITHDRAW_PATH_TEMPLATE,
            post(lifecycle::withdraw_run),
        )
        .route(
            PAYROLL_RUN_SCHEDULE_DISBURSEMENT_PATH_TEMPLATE,
            post(lifecycle::schedule_disbursement),
        )
        .route(
            PAYROLL_RUN_DISBURSEMENT_ATTEST_PATH_TEMPLATE,
            post(lifecycle::attest_disbursement),
        )
        .route(
            PAYROLL_RUN_ISSUE_PAYSLIPS_PATH_TEMPLATE,
            post(lifecycle::issue_payslips),
        )
        .route(
            PAYROLL_RUN_PAYSLIP_DELIVERY_PATH_TEMPLATE,
            get(lifecycle::get_payslip_delivery),
        )
        .merge(payslip_draft::routes())
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
pub(crate) struct PageParams {
    pub(crate) limit: Option<i64>,
    pub(crate) offset: Option<i64>,
}

async fn list_runs_page(
    state: &PayrollRestState,
    headers: &HeaderMap,
    params: PageParams,
) -> Result<PayrollRunPage, RestError> {
    let principal = principal_from_headers(state, headers).await?;
    require_run_read(&principal)?;

    let org = principal.org_id;
    let actor = principal.user_id;
    let pool = state.store.pool().clone();
    with_audits::<_, PayrollRunPage, RestError>(&pool, org, move |tx| {
        Box::pin(async move {
            let page = list_runs_in_tx(tx, params.limit, params.offset)
                .await
                .map_err(RestError::from_store)?;
            let event = AuditEvent::new(
                Some(actor),
                AuditAction::new("payroll_run.list_read").map_err(RestError::from_kernel)?,
                "payroll_draft_run",
                "query",
                TraceContext::generate(),
                time::OffsetDateTime::now_utc(),
            )
            .with_org(org);
            Ok((page, vec![event]))
        })
    })
    .await
}

async fn list_runs(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Query(params): Query<PageParams>,
) -> Result<Response, RestError> {
    let page = list_runs_page(&state, &headers, params).await?;
    Ok(Json(page).into_response())
}

async fn get_run(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
    Query(params): Query<PageParams>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_run_read(&principal)?;

    let org = principal.org_id;
    let actor = principal.user_id;
    let pool = state.store.pool().clone();
    let detail = with_audits::<_, Option<PayrollRunDetail>, RestError>(&pool, org, move |tx| {
        Box::pin(async move {
            let detail = get_run_in_tx(tx, run_id, params.limit, params.offset)
                .await
                .map_err(RestError::from_store)?;
            // Audit only on an actual read of a real run — a miss carries
            // no sensitive payload and would otherwise pollute the trail
            // with probe noise.
            let events = if detail.is_some() {
                vec![
                    AuditEvent::new(
                        Some(actor),
                        AuditAction::new("payroll_run.read").map_err(RestError::from_kernel)?,
                        "payroll_draft_run",
                        run_id.to_string(),
                        TraceContext::generate(),
                        time::OffsetDateTime::now_utc(),
                    )
                    .with_org(org),
                ]
            } else {
                Vec::new()
            };
            Ok((detail, events))
        })
    })
    .await?;

    let detail = detail
        .ok_or_else(|| RestError::new(StatusCode::NOT_FOUND, "not_found", "run not found"))?;
    Ok(Json(detail).into_response())
}

async fn list_my_lines(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Query(params): Query<PageParams>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    // Self-scoped readiness, not issued 명세서. Do not attach
    // `require_run_read`: MEMBER has no PayrollRunRead and must still read
    // own draft lines. Mirrors `GET /api/v1/hr/attendance-records/me`: an
    // account with no linked employee (ADMIN/system) reads an empty page,
    // never a 403, and this read is never audited (own-data self-service).
    let page = match state
        .store
        .linked_employee_id(principal.user_id)
        .await
        .map_err(RestError::from_store)?
    {
        Some(employee_id) => state
            .store
            .list_my_lines(employee_id, params.limit, params.offset)
            .await
            .map_err(RestError::from_store)?,
        None => MyPayrollLinePage {
            items: Vec::new(),
            total: 0,
            limit: params.limit.unwrap_or(100),
            offset: params.offset.unwrap_or(0),
        },
    };
    Ok(Json(page).into_response())
}

/// Admin-tier run/lines read. Org-wide only (`authorize_org_wide`): the
/// underlying tables have no `branch_id`, so a branch-scoped ADMIN is denied
/// rather than silently widened to the whole org (see module docs).
pub(crate) fn require_run_read(principal: &Principal) -> Result<(), RestError> {
    authorize_org_wide(principal, Action::new(Feature::PayrollRunRead))
        .map_err(RestError::from_kernel)
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<serde_json::Value>,
}

#[derive(Debug)]
pub(crate) struct RestError {
    pub(crate) status: StatusCode,
    code: &'static str,
    message: String,
    details: Option<serde_json::Value>,
}

impl RestError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: None,
        }
    }

    pub(crate) fn with_details(mut self, details: Option<serde_json::Value>) -> Self {
        self.details = details;
        self
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message)
    }

    pub(crate) fn from_kernel(err: KernelError) -> Self {
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

    pub(crate) fn from_store(err: PgPayrollError) -> Self {
        match err {
            PgPayrollError::Domain(err) => Self::from_kernel(err),
            PgPayrollError::Db(_) => {
                // Never leak sqlx/schema internals (OWASP A05). Log server-side.
                tracing::error!(error = %err, "payroll store error");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal server error",
                )
            }
        }
    }
}

impl From<DbError> for RestError {
    fn from(err: DbError) -> Self {
        tracing::error!(error = %err, "payroll db error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal server error",
        )
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
                    details: self.details,
                },
            }),
        )
            .into_response()
    }
}

pub(crate) async fn principal_from_headers(
    state: &PayrollRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for the payroll API")
    })?;
    console_platform_request_context::resolve_principal(verifier, state.store.pool(), headers)
        .await
        .map_err(rest_error_from_request_context)
}

fn rest_error_from_request_context(err: RequestContextError) -> RestError {
    match err {
        RequestContextError::VerifierUnavailable => {
            RestError::unavailable("JWT verification is not configured for the payroll API")
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
    use console_platform_authz::Role;
    use std::collections::BTreeSet;

    fn principal(role: Role, scope: console_kernel_core::BranchScope) -> Principal {
        Principal::new(
            console_kernel_core::UserId::new(),
            console_kernel_core::OrgId::knl(),
            BTreeSet::from([role]),
            scope,
        )
    }

    #[test]
    fn member_is_denied_run_read() {
        let p = principal(Role::Member, console_kernel_core::BranchScope::All);
        assert_eq!(
            require_run_read(&p).unwrap_err().status,
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn built_in_admin_is_denied_run_read_even_with_all_branch_scope() {
        // `authorize_org_wide`'s built-in path only ever considers
        // SuperAdmin/Executive (`console_platform_authz::authorize_org_wide`) — an
        // ADMIN role never satisfies it, All-branch-scope or not. This matches
        // the existing `EmployeeDirectoryRead`/`OrgWideQueueTriage` behavior
        // exactly, so it is not a payroll-specific gap: an ADMIN's only path
        // to these endpoints is a custom org-wide PBAC grant.
        let p = principal(Role::Admin, console_kernel_core::BranchScope::All);
        assert_eq!(
            require_run_read(&p).unwrap_err().status,
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn branch_scoped_admin_is_denied_run_read() {
        // No branch_id on payroll_draft_runs/lines: a branch-scoped ADMIN must
        // be denied rather than silently widened to org-wide visibility.
        let p = principal(
            Role::Admin,
            console_kernel_core::BranchScope::Branches(BTreeSet::from([
                console_kernel_core::BranchId::new(),
            ])),
        );
        assert_eq!(
            require_run_read(&p).unwrap_err().status,
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn executive_and_super_admin_can_read_runs() {
        for role in [Role::Executive, Role::SuperAdmin] {
            let p = principal(role, console_kernel_core::BranchScope::All);
            assert!(
                require_run_read(&p).is_ok(),
                "{role:?} must be able to read payroll runs"
            );
        }
    }
}
