//! Finance GL (전표) REST API.
//!
//! Authorization: voucher management is the 경리/accounting-close tier. This slice
//! reuses the existing `PeriodLockManage` capability (ADMIN + EXECUTIVE +
//! SUPER_ADMIN — the same accounting-close authority) rather than minting a
//! dedicated `VoucherManage` feature, to keep the lane hermetic and avoid
//! churning the shared `Feature` enum while a parallel wave lane is editing it. A
//! dedicated `VoucherManage` feature is a clean follow-up for the wire step.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_finance_gl_adapter_postgres::{PgVoucherError, PgVoucherStore};
use console_finance_gl_application::{
    CreateVoucherDraftCommand, ReverseVoucherCommand, VoucherLineInput, VoucherTransitionCommand,
};
use console_finance_gl_domain::{VoucherId, VoucherStatus};
use console_kernel_core::{BranchId, ErrorKind, KernelError, TraceContext};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize, authorize_org_wide};
use console_platform_db::DbError;
use serde::{Deserialize, Serialize};

/// The capability gating voucher management (see module docs).
const VOUCHER_FEATURE: Feature = Feature::PeriodLockManage;

#[derive(Clone)]
pub struct FinanceGlRestState {
    store: PgVoucherStore,
    jwt_verifier: Option<JwtVerifier>,
}

impl FinanceGlRestState {
    #[must_use]
    pub fn new(store: PgVoucherStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
        }
    }
}

pub const FINANCE_GL_VOUCHERS_PATH: &str = "/api/v1/finance-gl/vouchers";
pub const FINANCE_GL_VOUCHER_PATH_TEMPLATE: &str = "/api/v1/finance-gl/vouchers/{voucher_id}";
pub const FINANCE_GL_VOUCHER_SUBMIT_PATH_TEMPLATE: &str =
    "/api/v1/finance-gl/vouchers/{voucher_id}/submit";
pub const FINANCE_GL_VOUCHER_APPROVE_PATH_TEMPLATE: &str =
    "/api/v1/finance-gl/vouchers/{voucher_id}/approve";
pub const FINANCE_GL_VOUCHER_POST_PATH_TEMPLATE: &str =
    "/api/v1/finance-gl/vouchers/{voucher_id}/post";
pub const FINANCE_GL_VOUCHER_REVERSE_PATH_TEMPLATE: &str =
    "/api/v1/finance-gl/vouchers/{voucher_id}/reverse";
pub const FINANCE_GL_ACCOUNT_ENTRIES_PATH_TEMPLATE: &str =
    "/api/v1/finance-gl/accounts/{account_code}/entries";

pub const FINANCE_GL_ROUTE_PATHS: &[&str] = &[
    FINANCE_GL_VOUCHERS_PATH,
    FINANCE_GL_VOUCHER_PATH_TEMPLATE,
    FINANCE_GL_VOUCHER_SUBMIT_PATH_TEMPLATE,
    FINANCE_GL_VOUCHER_APPROVE_PATH_TEMPLATE,
    FINANCE_GL_VOUCHER_POST_PATH_TEMPLATE,
    FINANCE_GL_VOUCHER_REVERSE_PATH_TEMPLATE,
    FINANCE_GL_ACCOUNT_ENTRIES_PATH_TEMPLATE,
];

pub fn router(state: FinanceGlRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(
            FINANCE_GL_VOUCHERS_PATH,
            get(list_vouchers).post(create_draft),
        )
        .route(FINANCE_GL_VOUCHER_PATH_TEMPLATE, get(get_voucher))
        .route(
            FINANCE_GL_VOUCHER_SUBMIT_PATH_TEMPLATE,
            post(submit_voucher),
        )
        .route(
            FINANCE_GL_VOUCHER_APPROVE_PATH_TEMPLATE,
            post(approve_voucher),
        )
        .route(FINANCE_GL_VOUCHER_POST_PATH_TEMPLATE, post(post_voucher))
        .route(
            FINANCE_GL_VOUCHER_REVERSE_PATH_TEMPLATE,
            post(reverse_voucher),
        )
        .route(
            FINANCE_GL_ACCOUNT_ENTRIES_PATH_TEMPLATE,
            get(account_entries),
        )
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
struct ListVouchersQuery {
    branch_id: Option<BranchId>,
    status: Option<VoucherStatus>,
}

#[derive(Debug, Deserialize)]
struct CreateVoucherRequest {
    branch_id: BranchId,
    #[serde(default)]
    memo: String,
    lines: Vec<VoucherLineInput>,
}

#[derive(Debug, Deserialize)]
struct ReverseVoucherRequest {
    #[serde(default)]
    memo: String,
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

async fn list_vouchers(
    State(state): State<FinanceGlRestState>,
    headers: HeaderMap,
    Query(query): Query<ListVouchersQuery>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    // A branch-scoped listing authorizes against that branch; an org-wide listing
    // requires org-wide authority (RLS still confines rows to the tenant).
    match query.branch_id {
        Some(branch_id) => authorize(&principal, Action::new(VOUCHER_FEATURE), branch_id)
            .map_err(RestError::from_kernel)?,
        None => authorize_org_wide(&principal, Action::new(VOUCHER_FEATURE))
            .map_err(RestError::from_kernel)?,
    }
    let vouchers = state
        .store
        .list(query.branch_id, query.status)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(vouchers))
}

async fn create_draft(
    State(state): State<FinanceGlRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateVoucherRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize(&principal, Action::new(VOUCHER_FEATURE), body.branch_id)
        .map_err(RestError::from_kernel)?;
    let voucher = state
        .store
        .create_draft(CreateVoucherDraftCommand {
            actor: principal.user_id,
            branch_id: body.branch_id,
            memo: body.memo,
            // Hand-keyed vouchers carry no source linkage — provenance is set only
            // by the trusted approval-derived path (S7).
            source: None,
            lines: body.lines,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(voucher)))
}

async fn get_voucher(
    State(state): State<FinanceGlRestState>,
    headers: HeaderMap,
    Path(voucher_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let voucher = state
        .store
        .get(VoucherId::from_uuid(voucher_id))
        .await
        .map_err(RestError::from_store)?;
    authorize(&principal, Action::new(VOUCHER_FEATURE), voucher.branch_id)
        .map_err(RestError::from_kernel)?;
    Ok(Json(voucher))
}

async fn submit_voucher(
    State(state): State<FinanceGlRestState>,
    headers: HeaderMap,
    Path(voucher_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    run_transition(state, headers, voucher_id, Transition::Submit).await
}

async fn approve_voucher(
    State(state): State<FinanceGlRestState>,
    headers: HeaderMap,
    Path(voucher_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    run_transition(state, headers, voucher_id, Transition::Approve).await
}

async fn post_voucher(
    State(state): State<FinanceGlRestState>,
    headers: HeaderMap,
    Path(voucher_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    run_transition(state, headers, voucher_id, Transition::Post).await
}

#[derive(Clone, Copy)]
enum Transition {
    Submit,
    Approve,
    Post,
}

async fn run_transition(
    state: FinanceGlRestState,
    headers: HeaderMap,
    voucher_id: uuid::Uuid,
    transition: Transition,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let voucher_id = VoucherId::from_uuid(voucher_id);
    // Authorize against the voucher's (immutable) branch before mutating.
    let current = state
        .store
        .get(voucher_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(&principal, Action::new(VOUCHER_FEATURE), current.branch_id)
        .map_err(RestError::from_kernel)?;

    let command = VoucherTransitionCommand {
        actor: principal.user_id,
        voucher_id,
        trace: TraceContext::generate(),
        occurred_at: time::OffsetDateTime::now_utc(),
    };
    let voucher = match transition {
        Transition::Submit => state.store.submit(command).await,
        Transition::Approve => state.store.approve(command).await,
        Transition::Post => state.store.post(command).await,
    }
    .map_err(RestError::from_store)?;
    Ok(Json(voucher).into_response())
}

async fn reverse_voucher(
    State(state): State<FinanceGlRestState>,
    headers: HeaderMap,
    Path(voucher_id): Path<uuid::Uuid>,
    Json(body): Json<ReverseVoucherRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let voucher_id = VoucherId::from_uuid(voucher_id);
    let current = state
        .store
        .get(voucher_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(&principal, Action::new(VOUCHER_FEATURE), current.branch_id)
        .map_err(RestError::from_kernel)?;
    let contra = state
        .store
        .reverse(ReverseVoucherCommand {
            actor: principal.user_id,
            voucher_id,
            memo: body.memo,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(contra)))
}

async fn account_entries(
    State(state): State<FinanceGlRestState>,
    headers: HeaderMap,
    Path(account_code): Path<String>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    // Account drill spans branches — org-wide read authority.
    authorize_org_wide(&principal, Action::new(VOUCHER_FEATURE)).map_err(RestError::from_kernel)?;
    let entries = state
        .store
        .account_drill(&account_code)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(entries))
}

async fn principal_from_headers(
    state: &FinanceGlRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for finance-GL API")
    })?;
    console_platform_request_context::resolve_principal(verifier, state.store.pool(), headers)
        .await
        .map_err(rest_error_from_request_context)
}

fn rest_error_from_request_context(
    err: console_platform_request_context::RequestContextError,
) -> RestError {
    use console_platform_request_context::RequestContextError as E;
    match err {
        E::VerifierUnavailable => {
            RestError::unavailable("JWT verification is not configured for finance-GL API")
        }
        E::WrongTokenTier => RestError::from_kernel(KernelError::forbidden(
            "token tier is not valid for this route",
        )),
        E::AccessScope(error) => RestError::from_kernel(error),
        E::BranchScope(message) | E::EffectivePolicy(message) => RestError::internal(message),
        E::MissingOrg => RestError::internal("no tenant context is bound to the current request"),
        E::MissingBearer => RestError::unauthorized("missing or malformed bearer token"),
        E::InvalidToken => RestError::unauthorized("invalid bearer token"),
        E::InvalidClaim(message) => {
            RestError::unauthorized(format!("token claim is invalid: {message}"))
        }
    }
}

#[derive(Debug)]
struct RestError {
    status: StatusCode,
    kind: ErrorKind,
    message: String,
}

impl RestError {
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

    fn from_store(error: PgVoucherError) -> Self {
        match error {
            PgVoucherError::Domain(error) => Self::from_kernel(error),
            PgVoucherError::Db(error) => Self::from_db(error),
        }
    }

    fn from_db(error: DbError) -> Self {
        match error {
            DbError::Sqlx(sqlx::Error::RowNotFound) => {
                Self::from_kernel(KernelError::not_found("row was not found"))
            }
            DbError::Sqlx(sqlx::Error::Database(err))
                if err.code().is_some_and(|code| code == "23505") =>
            {
                tracing::error!(error = %err, "database unique-constraint violation");
                Self::from_kernel(KernelError::conflict("resource already exists"))
            }
            DbError::Sqlx(err) => {
                tracing::error!(error = %err, "database error");
                Self::internal("internal server error")
            }
            other => {
                tracing::error!(error = %other, "database error");
                Self::internal("internal server error")
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

fn status_for_error_kind(kind: ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Forbidden => StatusCode::FORBIDDEN,
        ErrorKind::Conflict | ErrorKind::InvalidTransition => StatusCode::CONFLICT,
        ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
