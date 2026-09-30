//! Governance REST API — override / four-eyes / lifecycle-config console
//! surface plus a §16 gate-chain preflight (status only, never commits).
//!
//! `router(state)` self-applies `with_request_context`, matching every other
//! domain rest crate; `build_router` merges it alongside the rest.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use console_governance_adapter_postgres::{
    LifecyclePreflightQuery, PgGovernanceError, PgGovernanceStore,
};
use console_governance_application::{
    ApprovalDecision, ConfigureTransitionCommand, CreateApprovalCommand,
    DecidePendingApprovalCommand, OpenOverrideCommand,
};
use console_governance_domain::{
    GateChainConfig, GateChainOutcome, LifecycleState, TransitionRequirements,
};
use console_kernel_core::{ErrorKind, KernelError, TraceContext};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize_org_wide};
use console_platform_db::DbError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone)]
pub struct GovernanceRestState {
    store: PgGovernanceStore,
    jwt_verifier: Option<JwtVerifier>,
}

impl GovernanceRestState {
    #[must_use]
    pub fn new(store: PgGovernanceStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
        }
    }
}

pub const GOVERNANCE_OVERRIDES_PATH: &str = "/api/v1/governance/overrides";
pub const GOVERNANCE_APPROVALS_PATH: &str = "/api/v1/governance/approvals";
pub const GOVERNANCE_APPROVALS_DECIDE_PATH: &str = "/api/v1/governance/approvals/decide";
pub const GOVERNANCE_LIFECYCLE_TRANSITIONS_PATH: &str = "/api/v1/governance/lifecycle/transitions";
pub const GOVERNANCE_LIFECYCLE_PREFLIGHT_PATH: &str = "/api/v1/governance/lifecycle/preflight";

pub const GOVERNANCE_ROUTE_PATHS: &[&str] = &[
    GOVERNANCE_OVERRIDES_PATH,
    GOVERNANCE_APPROVALS_PATH,
    GOVERNANCE_APPROVALS_DECIDE_PATH,
    GOVERNANCE_LIFECYCLE_TRANSITIONS_PATH,
    GOVERNANCE_LIFECYCLE_PREFLIGHT_PATH,
];

pub fn router(state: GovernanceRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(GOVERNANCE_OVERRIDES_PATH, post(open_override))
        .route(GOVERNANCE_APPROVALS_PATH, post(create_approval))
        .route(GOVERNANCE_APPROVALS_DECIDE_PATH, post(decide_approval))
        .route(
            GOVERNANCE_LIFECYCLE_TRANSITIONS_PATH,
            post(configure_transition),
        )
        .route(
            GOVERNANCE_LIFECYCLE_PREFLIGHT_PATH,
            post(lifecycle_preflight),
        )
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

// ---------------------------------------------------------------------------
// Request / response payloads
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OpenOverrideRequest {
    target_type: String,
    target_id: Uuid,
    reason: String,
    before_snapshot: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct CreateApprovalRequest {
    request_ref: Uuid,
    kind: String,
    /// The object this approval is FOR — a gate binds the approval to the action's
    /// target so it can never satisfy a gate for a different object. `None` for
    /// create-style actions with no pre-existing target.
    #[serde(default)]
    target_ref: Option<Uuid>,
    #[serde(default = "empty_object")]
    payload_summary: serde_json::Value,
}

fn empty_object() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Debug, Deserialize)]
struct DecideApprovalRequest {
    request_ref: Uuid,
    kind: String,
    /// Deprecated. Still accepted on the wire (the published OpenAPI contract
    /// carries it) but IGNORED as authority: the requester is read from the open
    /// pending request row inside the decision transaction, so a client cannot
    /// name the person the approver is supposedly distinct from.
    #[allow(dead_code)]
    requested_by: Uuid,
    decision: ApprovalDecision,
}

#[derive(Debug, Deserialize)]
struct ConfigureTransitionRequest {
    object_type_id: Uuid,
    from_state: LifecycleState,
    to_state: LifecycleState,
    #[serde(default)]
    requires_reason: bool,
    #[serde(default)]
    requires_four_eyes: bool,
    #[serde(default)]
    requires_checklist: bool,
}

#[derive(Debug, Deserialize)]
struct PreflightRequest {
    object_type_id: Uuid,
    from_state: LifecycleState,
    to_state: LifecycleState,
    /// Cedar authorize effect for the Authority gate (the console already has a
    /// decision; the writeback lane re-runs Cedar itself). Absent ⇒ fail-closed.
    #[serde(default)]
    authority_allow: Option<bool>,
    #[serde(default)]
    checklist_all_acknowledged: Option<bool>,
    /// Four-eyes request ref; its decision is read from the DB, not trusted from
    /// the client.
    #[serde(default)]
    four_eyes_request_ref: Option<Uuid>,
    #[serde(default)]
    egress_cleared: Option<bool>,
}

#[derive(Debug, Serialize)]
struct PreflightResponse {
    /// `false` when the edge is not configured for this object type — an
    /// unconfigured edge is denied (fail-closed), even if the base FSM allows it.
    configured: bool,
    config: GateChainConfig,
    outcome: GateChainOutcome,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn open_override(
    State(state): State<GovernanceRestState>,
    headers: HeaderMap,
    Json(body): Json<OpenOverrideRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = authorize_governance(&state, &headers).await?;
    let summary = state
        .store
        .open_override(OpenOverrideCommand {
            actor: principal.user_id,
            target_type: body.target_type,
            target_id: body.target_id,
            reason: body.reason,
            before_snapshot: body.before_snapshot,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(summary)))
}

async fn create_approval(
    State(state): State<GovernanceRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateApprovalRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = authorize_governance(&state, &headers).await?;
    let summary = state
        .store
        .create_approval(CreateApprovalCommand {
            requester: principal.user_id,
            request_ref: body.request_ref,
            kind: body.kind,
            target_ref: body.target_ref,
            payload_summary: body.payload_summary,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(summary)))
}

async fn decide_approval(
    State(state): State<GovernanceRestState>,
    headers: HeaderMap,
    Json(body): Json<DecideApprovalRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = authorize_governance(&state, &headers).await?;
    let summary = state
        .store
        .decide_pending_approval(DecidePendingApprovalCommand {
            approver: principal.user_id,
            request_ref: body.request_ref,
            kind: body.kind,
            decision: body.decision,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(summary)))
}

async fn configure_transition(
    State(state): State<GovernanceRestState>,
    headers: HeaderMap,
    Json(body): Json<ConfigureTransitionRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = authorize_governance(&state, &headers).await?;
    let config = state
        .store
        .configure_transition(ConfigureTransitionCommand {
            actor: principal.user_id,
            object_type_id: body.object_type_id,
            from_state: body.from_state,
            to_state: body.to_state,
            requirements: TransitionRequirements {
                requires_reason: body.requires_reason,
                requires_four_eyes: body.requires_four_eyes,
                requires_checklist: body.requires_checklist,
            },
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(config)))
}

async fn lifecycle_preflight(
    State(state): State<GovernanceRestState>,
    headers: HeaderMap,
    Json(body): Json<PreflightRequest>,
) -> Result<impl IntoResponse, RestError> {
    let _ = authorize_governance(&state, &headers).await?;
    // The whole preflight — base-FSM check, configured requirements, four-eyes
    // peek, gate chain — lives in the store, so the zero-row-delta non-mutation
    // test exercises this exact code rather than a copy of it.
    let preflight = state
        .store
        .lifecycle_preflight(LifecyclePreflightQuery {
            object_type_id: body.object_type_id,
            from_state: body.from_state,
            to_state: body.to_state,
            authority_allow: body.authority_allow,
            checklist_all_acknowledged: body.checklist_all_acknowledged,
            four_eyes_request_ref: body.four_eyes_request_ref,
            egress_cleared: body.egress_cleared,
        })
        .await
        .map_err(RestError::from_store)?;

    Ok(Json(PreflightResponse {
        configured: preflight.configured,
        config: preflight.config,
        outcome: preflight.outcome,
    }))
}

// ---------------------------------------------------------------------------
// Auth + errors
// ---------------------------------------------------------------------------

/// Governance is an org-scoped admin surface (config / override / four-eyes), so
/// it requires org-wide authority (`SUPER_ADMIN`/`EXECUTIVE` or an org-wide
/// custom grant). `RoleManage` is the existing PBAC-admin capability; L-WIRE may
/// introduce dedicated governance features later.
async fn authorize_governance(
    state: &GovernanceRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let principal = principal_from_headers(state, headers).await?;
    authorize_org_wide(&principal, Action::new(Feature::RoleManage))
        .map_err(RestError::from_kernel)?;
    Ok(principal)
}

async fn principal_from_headers(
    state: &GovernanceRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for governance API")
    })?;
    console_platform_request_context::resolve_principal(verifier, state.store.pool(), headers)
        .await
        .map_err(rest_error_from_request_context)
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

    fn from_store(error: PgGovernanceError) -> Self {
        match error {
            PgGovernanceError::Domain(error) => Self::from_kernel(error),
            PgGovernanceError::Db(error) => Self::from_db(error),
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
                // Never leak the constraint name (OWASP A05); log server-side.
                tracing::error!(error = %err, "governance unique-constraint violation");
                Self::from_kernel(KernelError::conflict("resource already exists"))
            }
            DbError::Sqlx(sqlx::Error::Database(err))
                if err.code().is_some_and(|code| code == "23514") =>
            {
                // A CHECK violation (e.g. self-approval) — stable generic message.
                tracing::error!(error = %err, "governance check-constraint violation");
                Self::from_kernel(KernelError::forbidden(
                    "operation violates a governance rule",
                ))
            }
            DbError::Sqlx(err) => {
                tracing::error!(error = %err, "database error");
                Self::internal("internal server error")
            }
            DbError::Serialize(err) => {
                tracing::error!(error = %err, "serialization error");
                Self::internal("internal server error")
            }
            DbError::CodeIssuance(err) => {
                tracing::error!(error = %err, "object-code issuance error");
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

fn rest_error_from_request_context(
    err: console_platform_request_context::RequestContextError,
) -> RestError {
    use console_platform_request_context::RequestContextError as E;
    match err {
        E::VerifierUnavailable => {
            RestError::unavailable("JWT verification is not configured for governance API")
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

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        let code = self.code();
        (
            self.status,
            Json(ErrorBody {
                error: ErrorPayload {
                    code,
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
