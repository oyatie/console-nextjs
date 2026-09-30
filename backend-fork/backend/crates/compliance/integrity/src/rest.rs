//! REST API for the integrity / anomaly dashboard.
//!
//! Routes:
//!   GET  /api/v1/integrity/findings         — list findings (requires IntegrityFindingsRead)
//!   POST /api/v1/integrity/findings/{id}/triage — triage (requires IntegrityFindingTriage)
//!
//! Both routes require EXECUTIVE or SUPER_ADMIN. An ADMIN must NOT see findings
//! about themselves — enforcement is at the feature-matrix level (ADMIN = Deny).

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use console_kernel_core::{ErrorKind, KernelError, TraceContext};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize_capability};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::{TriageFindingCommand, TriageTarget, validate_triage_memo};
use crate::store::{PgIntegrityError, PgIntegrityStore};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct IntegrityRestState {
    store: PgIntegrityStore,
    jwt_verifier: Option<JwtVerifier>,
}

impl IntegrityRestState {
    #[must_use]
    pub fn new(store: PgIntegrityStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
        }
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub const INTEGRITY_FINDINGS_PATH: &str = "/api/v1/integrity/findings";
pub const INTEGRITY_FINDING_TRIAGE_PATH_TEMPLATE: &str = "/api/v1/integrity/findings/{id}/triage";
pub const INTEGRITY_ROUTE_PATHS: &[&str] = &[
    INTEGRITY_FINDINGS_PATH,
    INTEGRITY_FINDING_TRIAGE_PATH_TEMPLATE,
];

pub fn router(state: IntegrityRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(INTEGRITY_FINDINGS_PATH, get(list_findings))
        .route(INTEGRITY_FINDING_TRIAGE_PATH_TEMPLATE, post(triage_finding))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Query parameters for `GET /api/v1/integrity/findings`.
#[derive(Debug, Deserialize)]
struct FindingsQuery {
    /// Optional status filter: OPEN | REVIEWED | DISMISSED | ESCALATED.
    /// Omit to return all statuses.
    status: Option<String>,
}

async fn list_findings(
    State(state): State<IntegrityRestState>,
    headers: HeaderMap,
    Query(query): Query<FindingsQuery>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_integrity_read(&principal)?;

    let findings = state
        .store
        .list_findings(query.status.as_deref())
        .await
        .map_err(RestError::from_store)?;

    Ok(Json(findings))
}

/// Request body for `POST /api/v1/integrity/findings/{id}/triage`.
#[derive(Debug, Deserialize)]
struct TriageRequest {
    status: String,
    memo: Option<String>,
}

async fn triage_finding(
    State(state): State<IntegrityRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<TriageRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_integrity_triage(&principal)?;

    let new_status = parse_triage_target(&body.status)?;
    validate_triage_memo(new_status, &body.memo).map_err(RestError::from_kernel)?;

    let command = TriageFindingCommand {
        finding_id: id,
        reviewer: principal.user_id,
        new_status,
        memo: body.memo,
        occurred_at: time::OffsetDateTime::now_utc(),
        trace: TraceContext::generate(),
    };

    let finding = state
        .store
        .triage_finding(command)
        .await
        .map_err(RestError::from_store)?;

    Ok(Json(finding))
}

fn parse_triage_target(raw: &str) -> Result<TriageTarget, RestError> {
    match raw {
        "REVIEWED" => Ok(TriageTarget::Reviewed),
        "DISMISSED" => Ok(TriageTarget::Dismissed),
        "ESCALATED" => Ok(TriageTarget::Escalated),
        other => Err(RestError::from_kernel(KernelError::validation(format!(
            "invalid triage status {other:?}; expected REVIEWED, DISMISSED, or ESCALATED"
        )))),
    }
}

// ---------------------------------------------------------------------------
// Authorization helpers
// ---------------------------------------------------------------------------

/// `governance_findings` (migration 0050) has no `branch_id` column, so these are
/// capability decisions. NOT `authorize_org_wide`: both features are
/// `[D,D,D,D,A,A]` (EXECUTIVE + SUPER_ADMIN), and a branch-scoped EXECUTIVE holds
/// them today — org-wide routing would newly deny that principal.
fn authorize_integrity_read(principal: &Principal) -> Result<(), RestError> {
    authorize_capability(principal, Action::new(Feature::IntegrityFindingsRead))
        .map_err(RestError::from_kernel)
}

fn authorize_integrity_triage(principal: &Principal) -> Result<(), RestError> {
    authorize_capability(principal, Action::new(Feature::IntegrityFindingTriage))
        .map_err(RestError::from_kernel)
}

// ---------------------------------------------------------------------------
// Principal extraction (same pattern as compliance/financial REST)
// ---------------------------------------------------------------------------

async fn principal_from_headers(
    state: &IntegrityRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for integrity API")
    })?;
    console_platform_request_context::resolve_principal(verifier, state.store.pool(), headers)
        .await
        .map_err(rest_error_from_request_context)
}

fn rest_error_from_request_context(
    err: console_platform_request_context::RequestContextError,
) -> RestError {
    match err {
        console_platform_request_context::RequestContextError::VerifierUnavailable => {
            RestError::unavailable("JWT verification is not configured for integrity API")
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
            RestError::from_kernel(KernelError::internal(message))
        }
        console_platform_request_context::RequestContextError::MissingOrg => {
            RestError::from_kernel(KernelError::internal(
                "no tenant context is bound to the current request",
            ))
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

// ---------------------------------------------------------------------------
// Error type (same shape as other REST crates)
// ---------------------------------------------------------------------------

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

    fn from_kernel(error: KernelError) -> Self {
        Self {
            status: status_for_kind(error.kind),
            kind: error.kind,
            message: error.message,
        }
    }

    fn from_store(error: PgIntegrityError) -> Self {
        match error {
            PgIntegrityError::Domain(e) => Self::from_kernel(e),
            PgIntegrityError::Db(e) => Self::from_db(e),
        }
    }

    fn from_db(error: console_platform_db::DbError) -> Self {
        use console_platform_db::DbError;
        match error {
            DbError::Sqlx(sqlx::Error::RowNotFound) => {
                Self::from_kernel(KernelError::not_found("row was not found"))
            }
            DbError::Sqlx(err) => {
                tracing::error!(error = %err, "database error in integrity handler");
                Self {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    kind: ErrorKind::Internal,
                    message: "internal server error".into(),
                }
            }
            DbError::Serialize(err) => {
                tracing::error!(error = %err, "serialization error in integrity handler");
                Self {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    kind: ErrorKind::Internal,
                    message: "internal server error".into(),
                }
            }
            DbError::CodeIssuance(err) => {
                tracing::error!(error = %err, "object-code issuance error in integrity handler");
                Self {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    kind: ErrorKind::Internal,
                    message: "internal server error".into(),
                }
            }
        }
    }

    fn code(&self) -> &'static str {
        match self.kind {
            ErrorKind::Validation => "validation",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Forbidden => "forbidden",
            ErrorKind::Conflict | ErrorKind::InvalidTransition => "conflict",
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

fn status_for_kind(kind: ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Forbidden => StatusCode::FORBIDDEN,
        ErrorKind::Conflict | ErrorKind::InvalidTransition => StatusCode::CONFLICT,
        ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
