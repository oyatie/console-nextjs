//! Messenger REST API.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use console_kernel_core::{
    BranchId, ErrorKind, EvidenceId, KernelError, MessageId, ThreadId, TraceContext, UserId,
    WorkOrderId,
};
use console_messenger_adapter_postgres::{PgMessengerError, PgMessengerStore};
use console_messenger_application::{
    CreateThreadCommand, JoinThreadCommand, ListChannelsQuery, ListMembersQuery, ListThreadsQuery,
    MarkThreadReadCommand, MemberProfileQuery, MessagePageQuery, SearchMessagesQuery,
    SendMessageCommand, SetThreadMuteCommand, ThreadPresenceQuery, ToggleAckCommand,
};
use console_messenger_domain::{ThreadKind, ThreadVisibility};
use console_platform_auth::JwtVerifier;
use console_platform_authz::Principal;
use serde::{Deserialize, Serialize};

pub const MESSENGER_ROUTE_PATHS: &[&str] = &[
    "/api/messenger/members",
    "/api/messenger/members/{userId}",
    "/api/messenger/channels",
    "/api/messenger/threads",
    "/api/messenger/threads/{threadId}/messages",
    "/api/messenger/threads/{threadId}/read-receipt",
    "/api/messenger/threads/{threadId}/join",
    "/api/messenger/threads/{threadId}/mute",
    "/api/messenger/threads/{threadId}/presence",
    "/api/messenger/messages/{messageId}/ack",
    "/api/messenger/search",
];

#[derive(Debug, Clone)]
pub struct MessengerRestState {
    store: PgMessengerStore,
    jwt_verifier: Option<JwtVerifier>,
}

impl MessengerRestState {
    #[must_use]
    pub fn new(store: PgMessengerStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
        }
    }
}

pub fn router(state: MessengerRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route("/api/messenger/members", get(list_members))
        .route("/api/messenger/members/{user_id}", get(get_member))
        .route("/api/messenger/channels", get(list_channels))
        .route(
            "/api/messenger/threads",
            get(list_threads).post(create_thread),
        )
        .route(
            "/api/messenger/threads/{thread_id}/messages",
            get(message_page).post(send_message),
        )
        .route(
            "/api/messenger/threads/{thread_id}/read-receipt",
            put(mark_thread_read),
        )
        .route("/api/messenger/threads/{thread_id}/join", post(join_thread))
        .route("/api/messenger/threads/{thread_id}/mute", put(set_mute))
        .route(
            "/api/messenger/threads/{thread_id}/presence",
            get(thread_presence),
        )
        .route("/api/messenger/messages/{message_id}/ack", post(toggle_ack))
        .route("/api/messenger/search", get(search_messages))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
struct CreateThreadRequest {
    branch_id: BranchId,
    kind: ThreadKind,
    #[serde(default)]
    visibility: Option<ThreadVisibility>,
    title: Option<String>,
    work_order_id: Option<WorkOrderId>,
    member_ids: Vec<UserId>,
}

#[derive(Debug, Deserialize)]
struct SendMessageRequest {
    body: String,
    #[serde(default)]
    attachment_evidence_ids: Vec<EvidenceId>,
    #[serde(default)]
    quoted_message_id: Option<MessageId>,
}

#[derive(Debug, Deserialize)]
struct SetMuteRequest {
    muted: bool,
}

#[derive(Debug, Deserialize)]
struct ReadReceiptRequest {
    last_read_message_id: MessageId,
}

#[derive(Debug, Deserialize)]
struct LimitQuery {
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MembersQuery {
    branch_id: BranchId,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MessagePageParams {
    before_message_id: Option<MessageId>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct SearchParams {
    q: String,
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct Items<T> {
    items: Vec<T>,
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

async fn create_thread(
    State(state): State<MessengerRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateThreadRequest>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    // Safer default: an API-created team thread is direct (fixed member set)
    // unless the caller EXPLICITLY passes visibility=channel. Left to the
    // domain default, a named `team` thread silently became a joinable
    // channel, exposing full history to anyone who joins even though the
    // caller supplied a curated `member_ids` list expecting a fixed set.
    let visibility = body.visibility.or(Some(ThreadVisibility::Direct));
    let summary = state
        .store
        .create_thread(CreateThreadCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            branch_id: body.branch_id,
            kind: body.kind,
            visibility,
            title: body.title,
            work_order_id: body.work_order_id,
            member_ids: body.member_ids,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(summary)).into_response())
}

async fn list_threads(
    State(state): State<MessengerRestState>,
    headers: HeaderMap,
    Query(query): Query<LimitQuery>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let items = state
        .store
        .list_threads(ListThreadsQuery {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            limit: query.limit.unwrap_or(50),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(Items { items }).into_response())
}

async fn list_members(
    State(state): State<MessengerRestState>,
    headers: HeaderMap,
    Query(query): Query<MembersQuery>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let items = state
        .store
        .list_members(ListMembersQuery {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            branch_id: query.branch_id,
            limit: query.limit.unwrap_or(100),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(Items { items }).into_response())
}

async fn get_member(
    State(state): State<MessengerRestState>,
    headers: HeaderMap,
    Path(user_id): Path<UserId>,
    Query(query): Query<MembersQuery>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let member = state
        .store
        .member_profile(MemberProfileQuery {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            branch_id: query.branch_id,
            user_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(member).into_response())
}

async fn send_message(
    State(state): State<MessengerRestState>,
    headers: HeaderMap,
    Path(thread_id): Path<ThreadId>,
    Json(body): Json<SendMessageRequest>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let message = state
        .store
        .send_message(SendMessageCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            thread_id,
            body: body.body,
            attachment_evidence_ids: body.attachment_evidence_ids,
            quoted_message_id: body.quoted_message_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(message)).into_response())
}

async fn message_page(
    State(state): State<MessengerRestState>,
    headers: HeaderMap,
    Path(thread_id): Path<ThreadId>,
    Query(query): Query<MessagePageParams>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let page = state
        .store
        .message_page(MessagePageQuery {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            thread_id,
            before_message_id: query.before_message_id,
            limit: query.limit.unwrap_or(50),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page).into_response())
}

async fn mark_thread_read(
    State(state): State<MessengerRestState>,
    headers: HeaderMap,
    Path(thread_id): Path<ThreadId>,
    Json(body): Json<ReadReceiptRequest>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let receipt = state
        .store
        .mark_thread_read(MarkThreadReadCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            thread_id,
            last_read_message_id: body.last_read_message_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(receipt).into_response())
}

async fn list_channels(
    State(state): State<MessengerRestState>,
    headers: HeaderMap,
    Query(query): Query<LimitQuery>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let items = state
        .store
        .list_channels(ListChannelsQuery {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            limit: query.limit.unwrap_or(50),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(Items { items }).into_response())
}

async fn join_thread(
    State(state): State<MessengerRestState>,
    headers: HeaderMap,
    Path(thread_id): Path<ThreadId>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let summary = state
        .store
        .join_thread(JoinThreadCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            thread_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary).into_response())
}

async fn set_mute(
    State(state): State<MessengerRestState>,
    headers: HeaderMap,
    Path(thread_id): Path<ThreadId>,
    Json(body): Json<SetMuteRequest>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let summary = state
        .store
        .set_thread_mute(SetThreadMuteCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            thread_id,
            muted: body.muted,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary).into_response())
}

async fn thread_presence(
    State(state): State<MessengerRestState>,
    headers: HeaderMap,
    Path(thread_id): Path<ThreadId>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let items = state
        .store
        .thread_presence(ThreadPresenceQuery {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            thread_id,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(Items { items }).into_response())
}

async fn toggle_ack(
    State(state): State<MessengerRestState>,
    headers: HeaderMap,
    Path(message_id): Path<MessageId>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let summary = state
        .store
        .toggle_ack(ToggleAckCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            message_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary).into_response())
}

async fn search_messages(
    State(state): State<MessengerRestState>,
    headers: HeaderMap,
    Query(query): Query<SearchParams>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let items = state
        .store
        .search_messages(SearchMessagesQuery {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            query: query.q,
            limit: query.limit.unwrap_or(50),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(Items { items }).into_response())
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

    fn from_store(err: PgMessengerError) -> Self {
        match err {
            PgMessengerError::Domain(err) => Self::from_kernel(err),
            PgMessengerError::Db(err) => {
                // Log the raw error server-side; never leak sqlx/schema internals
                // (schema disclosure, OWASP A05). Clients get a stable message.
                tracing::error!(error = %err, "database error");
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
    state: &MessengerRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for messenger API")
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
            RestError::unavailable("JWT verification is not configured for messenger API")
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
