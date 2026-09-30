//! Statutory-notice vault REST API (개인 수신함).
//!
//! Every route is person-scoped: the recipient is taken from the authenticated
//! principal (`/api/v1/me/...`), never from the request body or path, so one
//! user can neither list, read, nor confirm another user's documents.
//!
//! Receipt confirmation of a legal notice is the legal receipt evidence, so it
//! requires a FRESH passkey step-up (the exact `PasskeyService::verify_step_up`
//! mechanism workflow-studio publication uses). Reading a locked legal notice
//! returns metadata only and never auto-confirms.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_inbox_adapter_postgres::{PgInboxError, PgInboxStore};
use console_inbox_application::{
    ConfirmReceiptCommand, GetInboxDocQuery, InboxDocFilter, ListInboxDocsQuery,
};
use console_kernel_core::{ErrorKind, InboxDocId, KernelError, TraceContext};
use console_platform_auth::{JwtVerifier, PasskeyAuthenticationCredential, PasskeyService};
use console_platform_authz::Principal;
use console_platform_request_context::RequestContextError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const ME_INBOX_DOCS_PATH: &str = "/api/v1/me/inbox-docs";
pub const ME_INBOX_DOC_PATH_TEMPLATE: &str = "/api/v1/me/inbox-docs/{id}";
pub const ME_INBOX_DOC_CONFIRM_PATH_TEMPLATE: &str = "/api/v1/me/inbox-docs/{id}/confirm-receipt";

pub const INBOX_ROUTE_PATHS: &[&str] = &[
    ME_INBOX_DOCS_PATH,
    ME_INBOX_DOC_PATH_TEMPLATE,
    ME_INBOX_DOC_CONFIRM_PATH_TEMPLATE,
];

#[derive(Clone)]
pub struct InboxRestState {
    store: PgInboxStore,
    jwt_verifier: Option<JwtVerifier>,
    passkey_step_up: Option<PasskeyService>,
}

impl std::fmt::Debug for InboxRestState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxRestState")
            .field("has_jwt_verifier", &self.jwt_verifier.is_some())
            .field("has_passkey_step_up", &self.passkey_step_up.is_some())
            .finish()
    }
}

impl InboxRestState {
    #[must_use]
    pub fn new(store: PgInboxStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
            passkey_step_up: None,
        }
    }

    #[must_use]
    pub fn with_passkey_step_up(mut self, passkey_step_up: Option<PasskeyService>) -> Self {
        self.passkey_step_up = passkey_step_up;
        self
    }
}

pub fn router(state: InboxRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(ME_INBOX_DOCS_PATH, get(list_inbox_docs))
        .route(ME_INBOX_DOC_PATH_TEMPLATE, get(get_inbox_doc))
        .route(ME_INBOX_DOC_CONFIRM_PATH_TEMPLATE, post(confirm_receipt))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
struct ListParams {
    filter: Option<String>,
    before: Option<InboxDocId>,
    limit: Option<i64>,
}

/// Body for the confirm-receipt POST. The fresh passkey assertion is mandatory:
/// its absence yields 428 (precondition required), matching workflow-studio.
#[derive(Debug, Deserialize)]
struct ConfirmReceiptRequest {
    #[serde(default)]
    step_up: Option<StepUpAssertionRequest>,
}

#[derive(Debug, Deserialize)]
struct StepUpAssertionRequest {
    ceremony_id: Uuid,
    credential: PasskeyAuthenticationCredential,
}

async fn list_inbox_docs(
    State(state): State<InboxRestState>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let filter = InboxDocFilter::parse(params.filter.as_deref()).map_err(RestError::from_kernel)?;
    let page = state
        .store
        .list(ListInboxDocsQuery {
            recipient: principal.user_id,
            filter,
            before_id: params.before,
            limit: params.limit.unwrap_or(50),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page).into_response())
}

async fn get_inbox_doc(
    State(state): State<InboxRestState>,
    headers: HeaderMap,
    Path(id): Path<InboxDocId>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let detail = state
        .store
        .get(GetInboxDocQuery {
            recipient: principal.user_id,
            id,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(detail).into_response())
}

async fn confirm_receipt(
    State(state): State<InboxRestState>,
    headers: HeaderMap,
    Path(id): Path<InboxDocId>,
    Json(body): Json<ConfirmReceiptRequest>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    // The receipt IS the legal act, so require a fresh passkey step-up before
    // any state change — even for an idempotent re-confirm.
    verify_step_up(&state, &principal, body.step_up).await?;
    let summary = state
        .store
        .confirm_receipt(ConfirmReceiptCommand {
            recipient: principal.user_id,
            doc_id: id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary).into_response())
}

async fn verify_step_up(
    state: &InboxRestState,
    principal: &Principal,
    step_up: Option<StepUpAssertionRequest>,
) -> Result<(), RestError> {
    let step_up = step_up.ok_or_else(|| {
        RestError::new(
            StatusCode::PRECONDITION_REQUIRED,
            "passkey_step_up_required",
            "receipt confirmation requires a fresh passkey step-up",
        )
    })?;
    let verifier = state.passkey_step_up.as_ref().ok_or_else(|| {
        RestError::unavailable("passkey step-up is not configured for the inbox API")
    })?;
    verifier
        .verify_step_up_for_user(
            state.store.pool(),
            step_up.ceremony_id,
            step_up.credential,
            *principal.user_id.as_uuid(),
        )
        .await
        .map_err(|_| RestError::unauthorized("passkey step-up failed"))?;
    Ok(())
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

    fn from_store(err: PgInboxError) -> Self {
        match err {
            PgInboxError::Domain(err) => Self::from_kernel(err),
            PgInboxError::Dedup | PgInboxError::Db(_) => {
                // Never leak sqlx/schema internals (OWASP A05). Log server-side.
                tracing::error!(error = %err, "inbox store error");
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
                },
            }),
        )
            .into_response()
    }
}

async fn principal_from_headers(
    state: &InboxRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for the inbox API")
    })?;
    console_platform_request_context::resolve_principal(verifier, state.store.pool(), headers)
        .await
        .map_err(rest_error_from_request_context)
}

fn rest_error_from_request_context(err: RequestContextError) -> RestError {
    match err {
        RequestContextError::VerifierUnavailable => {
            RestError::unavailable("JWT verification is not configured for the inbox API")
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
