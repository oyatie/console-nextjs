//! Notification-center REST API.
//!
//! Every route is person-scoped: the recipient is taken from the authenticated
//! principal (`/api/v1/me/...`), never from the request body or path, so one
//! user can neither list nor read-mark another user's notifications. Mirrors
//! the `/api/v1/users/me` template.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use console_kernel_core::{ErrorKind, KernelError, NotificationId, TraceContext};
use console_notifications_adapter_postgres::{PgNotificationError, PgNotificationStore};
use console_notifications_application::{
    DeleteNotificationPolicyCommand, ListNotificationObjectGroupsQuery,
    ListNotificationPoliciesQuery, ListNotificationsQuery, MarkAllNotificationsReadCommand,
    MarkNotificationReadCommand, MarkNotificationUnreadCommand, NotificationCountsSummaryQuery,
    NotificationPolicyList, UnreadNotificationCountQuery, UpsertNotificationPolicyCommand,
};
use console_notifications_domain::{
    NotificationLink, NotificationPolicyId, NotificationPolicyScope,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::Principal;
use console_platform_request_context::RequestContextError;
use serde::{Deserialize, Serialize};

pub const ME_NOTIFICATIONS_PATH: &str = "/api/v1/me/notifications";
pub const ME_NOTIFICATIONS_BY_OBJECT_PATH: &str = "/api/v1/me/notifications/by-object";
pub const ME_NOTIFICATIONS_UNREAD_COUNT_PATH: &str = "/api/v1/me/notifications/unread-count";
pub const ME_NOTIFICATIONS_SUMMARY_PATH: &str = "/api/v1/me/notifications/summary";
pub const ME_NOTIFICATION_READ_PATH_TEMPLATE: &str = "/api/v1/me/notifications/{id}/read";
pub const ME_NOTIFICATION_UNREAD_PATH_TEMPLATE: &str = "/api/v1/me/notifications/{id}/unread";
pub const ME_NOTIFICATIONS_READ_ALL_PATH: &str = "/api/v1/me/notifications/read-all";
pub const ME_NOTIFICATION_POLICIES_PATH: &str = "/api/v1/me/notification-policies";
pub const ME_NOTIFICATION_POLICY_PATH_TEMPLATE: &str = "/api/v1/me/notification-policies/{id}";

pub const NOTIFICATIONS_ROUTE_PATHS: &[&str] = &[
    ME_NOTIFICATIONS_PATH,
    ME_NOTIFICATIONS_BY_OBJECT_PATH,
    ME_NOTIFICATIONS_UNREAD_COUNT_PATH,
    ME_NOTIFICATIONS_SUMMARY_PATH,
    ME_NOTIFICATION_READ_PATH_TEMPLATE,
    ME_NOTIFICATION_UNREAD_PATH_TEMPLATE,
    ME_NOTIFICATIONS_READ_ALL_PATH,
    ME_NOTIFICATION_POLICIES_PATH,
    ME_NOTIFICATION_POLICY_PATH_TEMPLATE,
];

#[derive(Debug, Clone)]
pub struct NotificationRestState {
    store: PgNotificationStore,
    jwt_verifier: Option<JwtVerifier>,
}

impl NotificationRestState {
    #[must_use]
    pub fn new(store: PgNotificationStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
        }
    }
}

pub fn router(state: NotificationRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(ME_NOTIFICATIONS_PATH, get(list_notifications))
        .route(ME_NOTIFICATIONS_BY_OBJECT_PATH, get(list_object_groups))
        .route(ME_NOTIFICATIONS_UNREAD_COUNT_PATH, get(unread_count))
        .route(ME_NOTIFICATIONS_SUMMARY_PATH, get(notifications_summary))
        // `read-all` is registered before `{id}/read`; the paths differ in
        // segment count so there is no capture collision, but keep the literal
        // first for clarity.
        .route(ME_NOTIFICATIONS_READ_ALL_PATH, post(mark_all_read))
        .route(ME_NOTIFICATION_READ_PATH_TEMPLATE, post(mark_read))
        .route(ME_NOTIFICATION_UNREAD_PATH_TEMPLATE, post(mark_unread))
        .route(
            ME_NOTIFICATION_POLICIES_PATH,
            get(list_policies).put(upsert_policy),
        )
        .route(ME_NOTIFICATION_POLICY_PATH_TEMPLATE, delete(delete_policy))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
struct ListParams {
    unread: Option<bool>,
    before: Option<NotificationId>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct GroupListParams {
    unread: Option<bool>,
    /// Opaque cursor from a previous page's `next_cursor`.
    before: Option<String>,
    limit: Option<i64>,
}

/// Flat wire shape `{scope, category?, link?}`; validated into
/// [`NotificationPolicyScope`] at the trust boundary so an invalid
/// combination is a 422, never a DB CHECK violation.
#[derive(Debug, Deserialize)]
struct UpsertNotificationPolicyRequest {
    scope: String,
    category: Option<String>,
    link: Option<NotificationLink>,
}

#[derive(Debug, Serialize)]
struct ReadAllResponse {
    marked: u64,
}

#[derive(Debug, Serialize)]
struct UnreadCountResponse {
    unread: i64,
}

async fn list_notifications(
    State(state): State<NotificationRestState>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let page = state
        .store
        .list(ListNotificationsQuery {
            recipient: principal.user_id,
            unread_only: params.unread.unwrap_or(false),
            before_id: params.before,
            limit: params.limit.unwrap_or(50),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page).into_response())
}

async fn unread_count(
    State(state): State<NotificationRestState>,
    headers: HeaderMap,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let unread = state
        .store
        .unread_count(UnreadNotificationCountQuery {
            recipient: principal.user_id,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(UnreadCountResponse { unread }).into_response())
}

async fn notifications_summary(
    State(state): State<NotificationRestState>,
    headers: HeaderMap,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let summary = state
        .store
        .summary(NotificationCountsSummaryQuery {
            recipient: principal.user_id,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary).into_response())
}

async fn mark_read(
    State(state): State<NotificationRestState>,
    headers: HeaderMap,
    Path(notification_id): Path<NotificationId>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let summary = state
        .store
        .mark_read(MarkNotificationReadCommand {
            recipient: principal.user_id,
            notification_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary).into_response())
}

async fn mark_all_read(
    State(state): State<NotificationRestState>,
    headers: HeaderMap,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let marked = state
        .store
        .mark_all_read(MarkAllNotificationsReadCommand {
            recipient: principal.user_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(ReadAllResponse { marked }).into_response())
}

async fn list_object_groups(
    State(state): State<NotificationRestState>,
    headers: HeaderMap,
    Query(params): Query<GroupListParams>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let page = state
        .store
        .list_object_groups(ListNotificationObjectGroupsQuery {
            recipient: principal.user_id,
            unread_only: params.unread.unwrap_or(false),
            before: params.before,
            limit: params.limit.unwrap_or(50),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page).into_response())
}

async fn mark_unread(
    State(state): State<NotificationRestState>,
    headers: HeaderMap,
    Path(notification_id): Path<NotificationId>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let summary = state
        .store
        .mark_unread(MarkNotificationUnreadCommand {
            recipient: principal.user_id,
            notification_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary).into_response())
}

async fn list_policies(
    State(state): State<NotificationRestState>,
    headers: HeaderMap,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let items = state
        .store
        .list_policies(ListNotificationPoliciesQuery {
            recipient: principal.user_id,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(NotificationPolicyList { items }).into_response())
}

async fn upsert_policy(
    State(state): State<NotificationRestState>,
    headers: HeaderMap,
    Json(body): Json<UpsertNotificationPolicyRequest>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let scope = NotificationPolicyScope::from_parts(&body.scope, body.category, body.link)
        .map_err(RestError::from_kernel)?;
    let summary = state
        .store
        .upsert_policy(UpsertNotificationPolicyCommand {
            recipient: principal.user_id,
            scope,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary).into_response())
}

async fn delete_policy(
    State(state): State<NotificationRestState>,
    headers: HeaderMap,
    Path(policy_id): Path<NotificationPolicyId>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    state
        .store
        .delete_policy(DeleteNotificationPolicyCommand {
            recipient: principal.user_id,
            policy_id,
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

    fn from_store(err: PgNotificationError) -> Self {
        match err {
            PgNotificationError::Domain(err) => Self::from_kernel(err),
            PgNotificationError::Dedup | PgNotificationError::Db(_) => {
                // Never leak sqlx/schema internals (OWASP A05). Log server-side.
                tracing::error!(error = %err, "notification store error");
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
    state: &NotificationRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for notifications API")
    })?;
    console_platform_request_context::resolve_principal(verifier, state.store.pool(), headers)
        .await
        .map_err(rest_error_from_request_context)
}

fn rest_error_from_request_context(err: RequestContextError) -> RestError {
    match err {
        RequestContextError::VerifierUnavailable => {
            RestError::unavailable("JWT verification is not configured for notifications API")
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
