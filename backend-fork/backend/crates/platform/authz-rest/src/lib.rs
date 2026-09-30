//! Cedar Policy Studio REST API (arch §5a/§5c).
//!
//! `router(state)` self-applies `with_request_context`, matching every other
//! domain rest crate; `build_router` (L-WIRE) merges it alongside the rest. The
//! whole surface is org-scoped PBAC admin — gated on org-wide `RoleManage`, the
//! existing PBAC-admin capability (mirrors `governance/rest`).
//!
//! Deny-by-omission is the default everywhere: authoring can only produce drafts
//! and `approved_for_promotion` (never a live/shadow row), and simulate/authorize
//! reuse one fail-closed evaluator ([`authoring::simulate`]).
//!
//! [`authoring::simulate`]: console_platform_authz::cedar_pbac::authoring::simulate
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod store;

pub use store::{
    AttachObjectPolicyCommand, CatalogEntry, CreateDraftCommand, DecisionLogEntry, DecisionLogRow,
    DraftRecord, PgCedarError, PgCedarPolicyStore, ReviewDraftCommand, UpdateDraftCommand,
};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_kernel_core::{ErrorKind, KernelError};
use console_platform_auth::JwtVerifier;
use console_platform_authz::cedar_pbac::authoring::{
    AuthoredPolicy, NoCodeBlocks, ReviewDecision, SimRequest, SimResource, SimSubject,
    SimulationOutcome, simulate as simulate_policies,
};
use console_platform_authz::{Action, Feature, Principal, authorize_org_wide};
use console_platform_db::DbError;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone)]
pub struct CedarPolicyRestState {
    store: PgCedarPolicyStore,
    jwt_verifier: Option<JwtVerifier>,
}

impl CedarPolicyRestState {
    #[must_use]
    pub fn new(store: PgCedarPolicyStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
        }
    }
}

pub const POLICY_CATALOG_PATH: &str = "/api/v1/policy/catalog";
pub const POLICY_DRAFTS_PATH: &str = "/api/v1/policy/drafts";
pub const POLICY_DRAFT_PATH_TEMPLATE: &str = "/api/v1/policy/drafts/{draft_id}";
pub const POLICY_DRAFT_VALIDATE_PATH_TEMPLATE: &str = "/api/v1/policy/drafts/{draft_id}/validate";
pub const POLICY_DRAFT_SUBMIT_PATH_TEMPLATE: &str = "/api/v1/policy/drafts/{draft_id}/submit";
pub const POLICY_DRAFT_REVIEW_PATH_TEMPLATE: &str = "/api/v1/policy/drafts/{draft_id}/review";
pub const POLICY_SIMULATE_PATH: &str = "/api/v1/policy/simulate";
pub const POLICY_AUTHORIZE_PATH: &str = "/api/v1/policy/authorize";
pub const POLICY_AUTHORIZE_BULK_PATH: &str = "/api/v1/policy/authorize/bulk";
pub const POLICY_DECISIONS_PATH: &str = "/api/v1/policy/decisions";

pub const CEDAR_POLICY_ROUTE_PATHS: &[&str] = &[
    POLICY_CATALOG_PATH,
    POLICY_DRAFTS_PATH,
    POLICY_DRAFT_PATH_TEMPLATE,
    POLICY_DRAFT_VALIDATE_PATH_TEMPLATE,
    POLICY_DRAFT_SUBMIT_PATH_TEMPLATE,
    POLICY_DRAFT_REVIEW_PATH_TEMPLATE,
    POLICY_SIMULATE_PATH,
    POLICY_AUTHORIZE_PATH,
    POLICY_AUTHORIZE_BULK_PATH,
    POLICY_DECISIONS_PATH,
];

/// Feed page cap — the Integrity console pages recent decisions, never the whole
/// (retention-bounded) ledger.
const DECISION_FEED_LIMIT: i64 = 200;

pub fn router(state: CedarPolicyRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(POLICY_CATALOG_PATH, get(list_catalog))
        .route(POLICY_DRAFTS_PATH, get(list_drafts).post(create_draft))
        .route(POLICY_DRAFT_PATH_TEMPLATE, get(get_draft).put(update_draft))
        .route(POLICY_DRAFT_VALIDATE_PATH_TEMPLATE, post(validate_draft))
        .route(POLICY_DRAFT_SUBMIT_PATH_TEMPLATE, post(submit_draft))
        .route(POLICY_DRAFT_REVIEW_PATH_TEMPLATE, post(review_draft))
        .route(POLICY_SIMULATE_PATH, post(simulate))
        .route(POLICY_AUTHORIZE_PATH, post(authorize))
        .route(POLICY_AUTHORIZE_BULK_PATH, post(authorize_bulk))
        .route(POLICY_DECISIONS_PATH, get(list_decisions))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

// ---------------------------------------------------------------------------
// Payloads
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CatalogQuery {
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateDraftBody {
    draft_key: String,
    title: String,
    #[serde(default)]
    author_note: Option<String>,
    blocks: NoCodeBlocks,
}

#[derive(Debug, Deserialize)]
struct UpdateDraftBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    author_note: Option<String>,
    blocks: NoCodeBlocks,
}

#[derive(Debug, Deserialize)]
struct ReviewBody {
    decision: ReviewDecision,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SimulateBody {
    request: SimRequest,
    /// Optional what-if: include this draft's generated policy text on top of the
    /// enforced set (the "what if I promote this draft" preview).
    #[serde(default)]
    include_draft_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct AuthorizeBody {
    request: SimRequest,
    /// Scope the live decision to the policies attached to one object type (row
    /// policy) or property (field policy). Absent ⇒ the org's enforced set.
    #[serde(default)]
    object_type_id: Option<Uuid>,
    #[serde(default)]
    property_def_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct DecisionResponse {
    outcome: SimulationOutcome,
}

/// Batch point-decision request: one subject, N `(action, resource)` checks — the
/// `PolicyGate.can` seam needs many decisions for the current principal in one
/// round-trip. Evaluated over the org's enforced set through the SAME fail-closed
/// evaluator as `/authorize`, so every check is deny-by-omission.
#[derive(Debug, Deserialize)]
struct BulkAuthorizeBody {
    subject: SimSubject,
    checks: Vec<BulkCheck>,
}

#[derive(Debug, Deserialize)]
struct BulkCheck {
    action: String,
    resource: SimResource,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    field: Option<String>,
}

/// Per-check decisions, aligned by index with the request `checks`.
#[derive(Debug, Serialize)]
struct BulkDecisionResponse {
    decisions: Vec<SimulationOutcome>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_catalog(
    State(state): State<CedarPolicyRestState>,
    headers: HeaderMap,
    Query(query): Query<CatalogQuery>,
) -> Result<impl IntoResponse, RestError> {
    authorize_admin(&state, &headers).await?;
    let entries = state
        .store
        .list_catalog(query.status)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(entries))
}

async fn list_drafts(
    State(state): State<CedarPolicyRestState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, RestError> {
    authorize_admin(&state, &headers).await?;
    let drafts = state
        .store
        .list_drafts()
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(drafts))
}

async fn create_draft(
    State(state): State<CedarPolicyRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateDraftBody>,
) -> Result<impl IntoResponse, RestError> {
    let principal = authorize_admin(&state, &headers).await?;
    let draft = state
        .store
        .create_draft(CreateDraftCommand {
            actor: principal.user_id,
            draft_key: body.draft_key,
            title: body.title,
            author_note: body.author_note,
            blocks: body.blocks,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(draft)))
}

async fn get_draft(
    State(state): State<CedarPolicyRestState>,
    headers: HeaderMap,
    Path(draft_id): Path<Uuid>,
) -> Result<impl IntoResponse, RestError> {
    authorize_admin(&state, &headers).await?;
    let draft = state
        .store
        .get_draft(draft_id)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(draft))
}

async fn update_draft(
    State(state): State<CedarPolicyRestState>,
    headers: HeaderMap,
    Path(draft_id): Path<Uuid>,
    Json(body): Json<UpdateDraftBody>,
) -> Result<impl IntoResponse, RestError> {
    let principal = authorize_admin(&state, &headers).await?;
    let draft = state
        .store
        .update_draft(UpdateDraftCommand {
            actor: principal.user_id,
            draft_id,
            title: body.title,
            author_note: body.author_note,
            blocks: body.blocks,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(draft))
}

async fn validate_draft(
    State(state): State<CedarPolicyRestState>,
    headers: HeaderMap,
    Path(draft_id): Path<Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let principal = authorize_admin(&state, &headers).await?;
    let draft = state
        .store
        .validate_draft(principal.user_id, draft_id)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(draft))
}

async fn submit_draft(
    State(state): State<CedarPolicyRestState>,
    headers: HeaderMap,
    Path(draft_id): Path<Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let principal = authorize_admin(&state, &headers).await?;
    let draft = state
        .store
        .submit_draft(principal.user_id, draft_id)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(draft))
}

async fn review_draft(
    State(state): State<CedarPolicyRestState>,
    headers: HeaderMap,
    Path(draft_id): Path<Uuid>,
    Json(body): Json<ReviewBody>,
) -> Result<impl IntoResponse, RestError> {
    let principal = authorize_admin(&state, &headers).await?;
    let draft = state
        .store
        .review_draft(ReviewDraftCommand {
            reviewer: principal.user_id,
            draft_id,
            decision: body.decision,
            note: body.note,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(draft))
}

async fn simulate(
    State(state): State<CedarPolicyRestState>,
    headers: HeaderMap,
    Json(body): Json<SimulateBody>,
) -> Result<impl IntoResponse, RestError> {
    authorize_admin(&state, &headers).await?;
    let mut policies = state
        .store
        .load_enforced_policies()
        .await
        .map_err(RestError::from_store)?;
    if let Some(draft_id) = body.include_draft_id {
        let draft = state
            .store
            .get_draft(draft_id)
            .await
            .map_err(RestError::from_store)?;
        policies.push(AuthoredPolicy::new(
            format!("draft:{draft_id}"),
            draft.generated_policy_text,
        ));
    }
    let outcome = simulate_policies(&policies, &body.request);
    Ok(Json(DecisionResponse { outcome }))
}

async fn authorize(
    State(state): State<CedarPolicyRestState>,
    headers: HeaderMap,
    Json(body): Json<AuthorizeBody>,
) -> Result<impl IntoResponse, RestError> {
    let principal = authorize_admin(&state, &headers).await?;
    let outcome = if let Some(object_type_id) = body.object_type_id {
        state
            .store
            .authorize_object_row(object_type_id, &body.request)
            .await
            .map_err(RestError::from_store)?
    } else if let Some(property_def_id) = body.property_def_id {
        state
            .store
            .authorize_property_field(property_def_id, &body.request)
            .await
            .map_err(RestError::from_store)?
    } else {
        let policies = state
            .store
            .load_enforced_policies()
            .await
            .map_err(RestError::from_store)?;
        simulate_policies(&policies, &body.request)
    };
    // Persist the decision to the append-only Integrity feed.
    state
        .store
        .record_decisions(
            *principal.user_id.as_uuid(),
            vec![decision_entry(&body.request, &outcome)],
        )
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(DecisionResponse { outcome }))
}

async fn authorize_bulk(
    State(state): State<CedarPolicyRestState>,
    headers: HeaderMap,
    Json(body): Json<BulkAuthorizeBody>,
) -> Result<impl IntoResponse, RestError> {
    let principal = authorize_admin(&state, &headers).await?;
    // Load the enforced set ONCE, then evaluate every check against it.
    let policies = state
        .store
        .load_enforced_policies()
        .await
        .map_err(RestError::from_store)?;
    let BulkAuthorizeBody { subject, checks } = body;
    let mut decisions = Vec::with_capacity(checks.len());
    let mut entries = Vec::with_capacity(checks.len());
    for check in checks {
        let request = SimRequest {
            subject: subject.clone(),
            action: check.action,
            resource: check.resource,
            purpose: check.purpose,
            field: check.field,
        };
        let outcome = simulate_policies(&policies, &request);
        entries.push(decision_entry(&request, &outcome));
        decisions.push(outcome);
    }
    state
        .store
        .record_decisions(*principal.user_id.as_uuid(), entries)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(BulkDecisionResponse { decisions }))
}

#[derive(Debug, Deserialize)]
struct DecisionsQuery {
    /// RFC3339 cursor: only decisions strictly after this instant. Absent ⇒ the
    /// most-recent page.
    #[serde(default, with = "time::serde::rfc3339::option")]
    since: Option<OffsetDateTime>,
}

async fn list_decisions(
    State(state): State<CedarPolicyRestState>,
    headers: HeaderMap,
    Query(query): Query<DecisionsQuery>,
) -> Result<impl IntoResponse, RestError> {
    authorize_admin(&state, &headers).await?;
    let decisions = state
        .store
        .recent_decisions(query.since, DECISION_FEED_LIMIT)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(decisions))
}

/// Project a computed decision into a durable feed entry.
fn decision_entry(request: &SimRequest, outcome: &SimulationOutcome) -> DecisionLogEntry {
    DecisionLogEntry {
        subject_ref: request.subject.user_id.clone(),
        action: request.action.clone(),
        resource_type: request.resource.resource_type.clone(),
        resource_id: request.resource.resource_id.clone(),
        effect: if outcome.effect.is_allow() {
            "allow"
        } else {
            "deny"
        }
        .to_owned(),
        determining_policies: outcome.determining_policies.clone(),
        reason: outcome.reason.clone(),
    }
}

// ---------------------------------------------------------------------------
// Auth + errors (mirrors governance/rest)
// ---------------------------------------------------------------------------

async fn authorize_admin(
    state: &CedarPolicyRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for policy API")
    })?;
    let principal =
        console_platform_request_context::resolve_principal(verifier, state.store.pool(), headers)
            .await
            .map_err(rest_error_from_request_context)?;
    authorize_org_wide(&principal, Action::new(Feature::RoleManage))
        .map_err(RestError::from_kernel)?;
    Ok(principal)
}

#[derive(Debug)]
struct RestError {
    status: StatusCode,
    kind: ErrorKind,
    message: String,
}

impl RestError {
    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            kind: ErrorKind::Internal,
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

    fn from_store(error: PgCedarError) -> Self {
        match error {
            PgCedarError::Domain(error) => Self::from_kernel(error),
            PgCedarError::Db(error) => Self::from_db(error),
            // A missing command pool is a deployment fault, not a client fault.
            // 500 would be indistinguishable from a real database error.
            PgCedarError::CommandUnavailable => {
                Self::unavailable("cedar policy command database is not configured or unavailable")
            }
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
                tracing::error!(error = %err, "cedar policy unique-constraint violation");
                Self::from_kernel(KernelError::conflict("resource already exists"))
            }
            DbError::Sqlx(sqlx::Error::Database(err))
                if err.code().is_some_and(|code| code == "23514") =>
            {
                tracing::error!(error = %err, "cedar policy check-constraint violation");
                Self::from_kernel(KernelError::forbidden("operation violates a policy rule"))
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
            RestError::unavailable("JWT verification is not configured for policy API")
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
