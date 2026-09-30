//! Personal-todos REST API.
//!
//! Every route is person-scoped: the owner is taken from the authenticated
//! principal (`/api/v1/me/...`), never from the request body or path, so one
//! user can neither list nor mutate another user's todos. Mirrors the
//! notification-center REST template.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;

pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, delete, get, post};
use axum::{Json, Router};
use console_kernel_core::{ErrorKind, KernelError, TodoId, TraceContext};
use console_platform_auth::JwtVerifier;
use console_platform_authz::Principal;
use console_platform_request_context::RequestContextError;
use console_todos_adapter_postgres::{PgTodoError, PgTodoStore};
use console_todos_application::{
    CreateTodoCommand, DeleteTodoCommand, ListTodosQuery, SetTodoDoneCommand,
};
use console_todos_domain::TodoRef;
use serde::{Deserialize, Serialize};

pub const ME_TODOS_PATH: &str = "/api/v1/me/todos";
pub const ME_TODO_PATH_TEMPLATE: &str = "/api/v1/me/todos/{todoId}";
pub const ME_TODO_DONE_PATH_TEMPLATE: &str = "/api/v1/me/todos/{todoId}/done";

/// Every route this face serves, as the single table [`router`] is built from.
///
/// [`route_paths`] reads the same table, so the contract-coverage test compares
/// the fragment against what the router is actually assembled from rather than
/// against a second hand-maintained list that can drift silently.
///
/// ponytail: the ceiling is that `router` must contain nothing but the fold
/// below — a `.route()` bolted on afterwards would still escape. Upgrade to
/// enumerating the built `Router` if axum ever exposes its route table.
fn routes() -> Vec<(&'static str, MethodRouter<TodoRestState>)> {
    vec![
        (ME_TODOS_PATH, get(list_todos).post(create_todo)),
        (ME_TODO_DONE_PATH_TEMPLATE, post(set_done)),
        (ME_TODO_PATH_TEMPLATE, delete(delete_todo)),
    ]
}

/// The path templates [`router`] serves, derived from the same table.
#[must_use]
pub fn route_paths() -> Vec<&'static str> {
    routes().into_iter().map(|(path, _)| path).collect()
}

#[derive(Debug, Clone)]
pub struct TodoRestState {
    store: PgTodoStore,
    jwt_verifier: Option<JwtVerifier>,
}

impl TodoRestState {
    #[must_use]
    pub fn new(store: PgTodoStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
        }
    }
}

pub fn router(state: TodoRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = routes()
        .into_iter()
        .fold(Router::new(), |router, (path, methods)| {
            router.route(path, methods)
        })
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
struct ListParams {
    include_done: Option<bool>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CreateTodoRequest {
    text: String,
    #[serde(default)]
    scopes: Vec<TodoRef>,
    #[serde(default)]
    links: Vec<TodoRef>,
}

#[derive(Debug, Deserialize)]
struct SetDoneRequest {
    done: bool,
}

async fn list_todos(
    State(state): State<TodoRestState>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let page = state
        .store
        .list(ListTodosQuery {
            owner: principal.user_id,
            include_done: params.include_done.unwrap_or(false),
            limit: params.limit.unwrap_or(100),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page).into_response())
}

async fn create_todo(
    State(state): State<TodoRestState>,
    headers: HeaderMap,
    Json(request): Json<CreateTodoRequest>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let summary = state
        .store
        .create(CreateTodoCommand {
            owner: principal.user_id,
            text: request.text,
            scopes: request.scopes,
            links: request.links,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(summary)).into_response())
}

async fn set_done(
    State(state): State<TodoRestState>,
    headers: HeaderMap,
    Path(todo_id): Path<TodoId>,
    Json(request): Json<SetDoneRequest>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let summary = state
        .store
        .set_done(SetTodoDoneCommand {
            owner: principal.user_id,
            todo_id,
            done: request.done,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary).into_response())
}

async fn delete_todo(
    State(state): State<TodoRestState>,
    headers: HeaderMap,
    Path(todo_id): Path<TodoId>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    state
        .store
        .delete(DeleteTodoCommand {
            owner: principal.user_id,
            todo_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(StatusCode::NO_CONTENT.into_response())
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

    fn from_store(err: PgTodoError) -> Self {
        match err {
            PgTodoError::Domain(err) => Self::from_kernel(err),
            PgTodoError::Db(_) => {
                // Never leak sqlx/schema internals (OWASP A05). Log server-side.
                tracing::error!(error = %err, "todo store error");
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
    state: &TodoRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for todos API")
    })?;
    console_platform_request_context::resolve_principal(verifier, state.store.pool(), headers)
        .await
        .map_err(rest_error_from_request_context)
}

fn rest_error_from_request_context(err: RequestContextError) -> RestError {
    match err {
        RequestContextError::VerifierUnavailable => {
            RestError::unavailable("JWT verification is not configured for todos API")
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
