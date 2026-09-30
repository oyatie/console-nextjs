//! REST API for P1 emergency dispatch.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_dispatch_adapter_postgres::{PendingFcmPush, PgDispatchError, PgDispatchStore};
use console_dispatch_application::{
    DispatchCandidatePage, DispatchQueueCursor, DispatchQueuePage, DispatchQueueStatus,
    ForceAssignP1DispatchCommand, IncidentLocationInput, ListDispatchQueueQuery,
    MyDispatchOfferPage, P1DispatchResponsePage, P1DispatchSummary, RespondP1DispatchCommand,
    StartP1DispatchCommand,
};
use console_dispatch_domain::{DispatchResponseKind, DispatchTimerConfig};
use console_kernel_core::{
    BranchScope, ErrorKind, KernelError, P1DispatchAlertId, P1DispatchId, TraceContext, UserId,
    WorkOrderId,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{
    Action, EffectiveFeatureGrant, Feature, PermissionLevel, Principal, authorize, permission_for,
};
use console_platform_jobs::{JobQueue, JobQueueError, JobRequest};
use console_platform_push::{FcmPushMessage, PushError, PushNotifier};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Person-scoped pending-offer list for the signed-in mechanic (UI-M3
/// overview inbox). The owner comes from the principal, never the request.
pub const ME_DISPATCH_OFFERS_PATH: &str = "/api/v1/me/dispatch-offers";

#[derive(Clone)]
pub struct DispatchRestState {
    store: PgDispatchStore,
    jwt_verifier: Option<JwtVerifier>,
    timers: DispatchTimerConfig,
    job_queue: Option<Arc<dyn JobQueue>>,
    push_notifier: Option<Arc<dyn PushNotifier>>,
}

impl DispatchRestState {
    #[must_use]
    pub fn new(
        store: PgDispatchStore,
        jwt_verifier: Option<JwtVerifier>,
        timers: DispatchTimerConfig,
        job_queue: Option<Arc<dyn JobQueue>>,
        push_notifier: Option<Arc<dyn PushNotifier>>,
    ) -> Self {
        Self {
            store,
            jwt_verifier,
            timers,
            job_queue,
            push_notifier,
        }
    }
}

pub const START_DISPATCH_PATH_TEMPLATE: &str = "/api/v1/work-orders/{work_order_id}/p1-dispatch";
pub const P1_DISPATCH_PATH_TEMPLATE: &str = "/api/v1/p1-dispatches/{dispatch_id}";
pub const P1_DISPATCH_RESPONSES_PATH_TEMPLATE: &str =
    "/api/v1/p1-dispatches/{dispatch_id}/responses";
pub const P1_DISPATCH_FORCE_ASSIGN_PATH_TEMPLATE: &str =
    "/api/v1/p1-dispatches/{dispatch_id}/force-assign";
pub const CONSOLE_DISPATCH_QUEUE_PATH: &str = "/api/v1/console/dispatch/queue";
pub const P1_DISPATCH_CANDIDATES_PATH_TEMPLATE: &str =
    "/api/v1/p1-dispatches/{dispatch_id}/candidates";
pub const DISPATCH_ROUTE_PATHS: &[&str] = &[
    START_DISPATCH_PATH_TEMPLATE,
    P1_DISPATCH_PATH_TEMPLATE,
    P1_DISPATCH_RESPONSES_PATH_TEMPLATE,
    P1_DISPATCH_FORCE_ASSIGN_PATH_TEMPLATE,
    CONSOLE_DISPATCH_QUEUE_PATH,
    P1_DISPATCH_CANDIDATES_PATH_TEMPLATE,
    ME_DISPATCH_OFFERS_PATH,
];

pub fn router(state: DispatchRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(START_DISPATCH_PATH_TEMPLATE, post(start_dispatch))
        .route(P1_DISPATCH_PATH_TEMPLATE, get(get_dispatch))
        .route(
            P1_DISPATCH_RESPONSES_PATH_TEMPLATE,
            post(respond_dispatch).get(list_dispatch_responses),
        )
        .route(CONSOLE_DISPATCH_QUEUE_PATH, get(list_dispatch_queue))
        .route(
            P1_DISPATCH_CANDIDATES_PATH_TEMPLATE,
            get(list_dispatch_candidates),
        )
        .route(P1_DISPATCH_FORCE_ASSIGN_PATH_TEMPLATE, post(force_assign))
        .route(ME_DISPATCH_OFFERS_PATH, get(list_my_offers))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

struct DispatchQuery<T>(T);
impl<S, T> FromRequestParts<S> for DispatchQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = RestError;
    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(|_| RestError::malformed_request())
    }
}
struct DispatchPath<T>(T);
impl<S, T> FromRequestParts<S> for DispatchPath<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = RestError;
    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(|_| RestError::malformed_request())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchQueueRequest {
    status: Option<String>,
    limit: Option<i64>,
    after: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StartDispatchRequest {
    incident_location: Option<IncidentLocationInput>,
    include_region: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RespondDispatchRequest {
    response: DispatchResponseKind,
}

#[derive(Debug, Deserialize)]
struct ForceAssignRequest {
    mechanic_id: UserId,
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

async fn list_dispatch_queue(
    State(state): State<DispatchRestState>,
    headers: HeaderMap,
    DispatchQuery(query): DispatchQuery<DispatchQueueRequest>,
) -> Result<Json<DispatchQueuePage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let branch_scope = authorized_feature_scope(&principal, Feature::WorkOrderReadAll)?;
    let now = time::OffsetDateTime::now_utc();
    let after = query
        .after
        .as_deref()
        .map(|raw| DispatchQueueCursor::decode(raw, now))
        .transpose()
        .map_err(RestError::from_kernel)?;
    let statuses =
        DispatchQueueStatus::parse_csv(query.status.as_deref()).map_err(RestError::from_kernel)?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(RestError::from_kernel(KernelError::validation(
            "dispatch queue limit must be between 1 and 200",
        )));
    }
    let page = state
        .store
        .list_dispatch_queue(ListDispatchQueueQuery {
            branch_scope,
            statuses,
            limit,
            after,
            now,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page))
}

async fn list_dispatch_candidates(
    State(state): State<DispatchRestState>,
    headers: HeaderMap,
    DispatchPath(dispatch_id): DispatchPath<P1DispatchId>,
) -> Result<Json<DispatchCandidatePage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let dispatch = state
        .store
        .dispatch(dispatch_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::AssigneeManage),
        dispatch.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    Ok(Json(
        state
            .store
            .dispatch_candidates(dispatch_id, time::OffsetDateTime::now_utc(), state.timers)
            .await
            .map_err(RestError::from_store)?,
    ))
}

async fn list_dispatch_responses(
    State(state): State<DispatchRestState>,
    headers: HeaderMap,
    DispatchPath(dispatch_id): DispatchPath<P1DispatchId>,
) -> Result<Json<P1DispatchResponsePage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let dispatch = state
        .store
        .dispatch(dispatch_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::WorkOrderReadAll),
        dispatch.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    Ok(Json(
        state
            .store
            .dispatch_responses(dispatch_id)
            .await
            .map_err(RestError::from_store)?,
    ))
}

async fn start_dispatch(
    State(state): State<DispatchRestState>,
    headers: HeaderMap,
    Path(work_order_id): Path<WorkOrderId>,
    Json(body): Json<StartDispatchRequest>,
) -> Result<Json<P1DispatchSummary>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let branch_id = state
        .store
        .work_order_branch(work_order_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(&principal, Action::new(Feature::WorkOrderCreate), branch_id)
        .map_err(RestError::from_kernel)?;
    let summary = state
        .store
        .start_dispatch(
            StartP1DispatchCommand {
                actor: principal.user_id,
                work_order_id,
                incident_location: body.incident_location,
                include_region: body.include_region.unwrap_or(false),
                trace: current_trace_context(),
                occurred_at: time::OffsetDateTime::now_utc(),
            },
            state.timers,
        )
        .await
        .map_err(RestError::from_store)?;
    schedule_dispatch_jobs(&state, &summary).await?;
    deliver_fcm_pushes(&state, summary.id).await?;
    Ok(Json(summary))
}

async fn respond_dispatch(
    State(state): State<DispatchRestState>,
    headers: HeaderMap,
    Path(dispatch_id): Path<P1DispatchId>,
    Json(body): Json<RespondDispatchRequest>,
) -> Result<Json<P1DispatchSummary>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let current = state
        .store
        .dispatch(dispatch_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::WorkOrderStart),
        current.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let summary = state
        .store
        .record_response(
            RespondP1DispatchCommand {
                actor: principal.user_id,
                dispatch_id,
                response: body.response,
                trace: current_trace_context(),
                occurred_at: time::OffsetDateTime::now_utc(),
            },
            state.timers,
        )
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

async fn list_my_offers(
    State(state): State<DispatchRestState>,
    headers: HeaderMap,
) -> Result<Json<MyDispatchOfferPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    // No branch authorize: the query is person-scoped by construction (only
    // dispatches that fanned out to the caller), mirroring /api/v1/me/*.
    let items = state
        .store
        .list_my_pending_offers(principal.user_id, time::OffsetDateTime::now_utc())
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(MyDispatchOfferPage { items }))
}

async fn get_dispatch(
    State(state): State<DispatchRestState>,
    headers: HeaderMap,
    Path(dispatch_id): Path<P1DispatchId>,
) -> Result<Json<P1DispatchSummary>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let summary = state
        .store
        .dispatch(dispatch_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::WorkOrderReadAll),
        summary.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    Ok(Json(summary))
}

async fn force_assign(
    State(state): State<DispatchRestState>,
    headers: HeaderMap,
    Path(dispatch_id): Path<P1DispatchId>,
    Json(body): Json<ForceAssignRequest>,
) -> Result<Json<P1DispatchSummary>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let current = state
        .store
        .dispatch(dispatch_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::AssigneeManage),
        current.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let summary = state
        .store
        .force_assign(ForceAssignP1DispatchCommand {
            actor: principal.user_id,
            dispatch_id,
            mechanic_id: body.mechanic_id,
            trace: current_trace_context(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

async fn schedule_dispatch_jobs(
    state: &DispatchRestState,
    summary: &P1DispatchSummary,
) -> Result<(), RestError> {
    let Some(queue) = state.job_queue.as_ref() else {
        return Ok(());
    };
    // Carry the dispatch's tenant onto every scheduled job so the background
    // worker arms the correct `app.current_org`. This handler runs inside the
    // request's tenant scope, so the org is the in-flight tenant.
    let org = console_platform_request_context::current_org()
        .map_err(|err| RestError::internal(err.to_string()))?;
    let accept =
        JobRequest::dispatch_accept_window_expired(summary.id, org, summary.accept_window_ends_at)
            .map_err(RestError::from_jobs)?;
    queue
        .schedule_at(accept, summary.accept_window_ends_at)
        .await
        .map_err(RestError::from_jobs)?;
    let no_ack_at = summary
        .accept_window_started_at
        .checked_add(state.timers.alimtalk_no_ack_after)
        .ok_or_else(|| RestError::internal("dispatch Alimtalk timer overflows time"))?;
    let no_ack = JobRequest::dispatch_alimtalk_no_ack(summary.id, org, no_ack_at)
        .map_err(RestError::from_jobs)?;
    queue
        .schedule_at(no_ack, no_ack_at)
        .await
        .map_err(RestError::from_jobs)?;
    let manual_call_at = summary
        .accept_window_started_at
        .checked_add(state.timers.force_assign_alert_after)
        .ok_or_else(|| RestError::internal("dispatch manual-call timer overflows time"))?;
    let manual_call = JobRequest::dispatch_manual_call_required(summary.id, org, manual_call_at)
        .map_err(RestError::from_jobs)?;
    queue
        .schedule_at(manual_call, manual_call_at)
        .await
        .map_err(RestError::from_jobs)?;
    Ok(())
}

async fn deliver_fcm_pushes(
    state: &DispatchRestState,
    dispatch_id: P1DispatchId,
) -> Result<(), RestError> {
    let Some(notifier) = state.push_notifier.as_ref() else {
        return Ok(());
    };
    let pushes = state
        .store
        .claim_fcm_pushes(dispatch_id, "FCM_PUSH", time::OffsetDateTime::now_utc())
        .await
        .map_err(RestError::from_store)?;
    for push in pushes {
        send_one_fcm(state, notifier.as_ref(), push).await?;
    }
    Ok(())
}

async fn send_one_fcm(
    state: &DispatchRestState,
    notifier: &dyn PushNotifier,
    push: PendingFcmPush,
) -> Result<(), RestError> {
    let data = BTreeMap::from([
        ("type".to_owned(), "p1_dispatch".to_owned()),
        ("dispatch_id".to_owned(), push.dispatch_id.to_string()),
        ("work_order_id".to_owned(), push.work_order_id.to_string()),
    ]);
    let lease_token = push.lease_token;
    let alert_id = push.alert_id;
    let message = FcmPushMessage {
        token: push.push_token,
        title: "P1 emergency dispatch".to_owned(),
        body: "Immediate response requested".to_owned(),
        data,
        idempotency_key: push.idempotency_key,
    };
    match notifier.send_fcm(message).await {
        Ok(provider_id) => {
            let lease_held = state
                .store
                .mark_alert_sent(
                    alert_id,
                    lease_token,
                    if provider_id.0.is_empty() {
                        None
                    } else {
                        Some(provider_id.0)
                    },
                    TraceContext::generate(),
                    time::OffsetDateTime::now_utc(),
                )
                .await
                .map_err(RestError::from_store)?;
            warn_if_lease_lost(lease_held, alert_id);
        }
        Err(err) => {
            let lease_held = state
                .store
                .mark_alert_failed(
                    alert_id,
                    lease_token,
                    provider_failure_reason(err),
                    TraceContext::generate(),
                    time::OffsetDateTime::now_utc(),
                )
                .await
                .map_err(RestError::from_store)?;
            warn_if_lease_lost(lease_held, alert_id);
        }
    }
    Ok(())
}

/// Consume the lost-lease signal from a `mark_alert_*` transition. `false` means
/// the lease was reclaimed elsewhere (e.g. after a crash) and the transition was
/// a no-op; surface it so the designed double-handling guard is observable.
fn warn_if_lease_lost(lease_held: bool, alert_id: P1DispatchAlertId) {
    if !lease_held {
        tracing::warn!(
            %alert_id,
            "alert lease lost before status mark; transition was a no-op (reclaimed elsewhere)"
        );
    }
}

fn provider_failure_reason(err: PushError) -> String {
    let message = err.to_string();
    if message.len() > 512 {
        message.chars().take(512).collect()
    } else {
        message
    }
}

async fn principal_from_headers(
    state: &DispatchRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for dispatch API")
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
            RestError::unavailable("JWT verification is not configured for dispatch API")
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

fn authorized_feature_scope(
    principal: &Principal,
    feature: Feature,
) -> Result<BranchScope, RestError> {
    let builtin_allows = principal
        .roles
        .iter()
        .any(|role| permission_for(*role, feature) == PermissionLevel::Allow);
    let custom_scope = custom_feature_scope(&principal.effective_feature_grants, feature);
    let scope = match (builtin_allows, custom_scope) {
        (true, _) => principal.branch_scope.clone(),
        (false, Some(scope)) => principal.branch_scope.intersect(&scope),
        (false, None) => BranchScope::none(),
    };
    if scope.is_empty() {
        return Err(RestError::from_kernel(KernelError::forbidden(
            "principal has no authorized branch scope for feature",
        )));
    }
    Ok(scope)
}

fn custom_feature_scope(grants: &[EffectiveFeatureGrant], feature: Feature) -> Option<BranchScope> {
    let mut branches = BTreeSet::new();
    for grant in grants {
        if grant.feature != feature || grant.permission != PermissionLevel::Allow {
            continue;
        }
        match &grant.branch_scope {
            BranchScope::All => return Some(BranchScope::All),
            BranchScope::Branches(values) => branches.extend(values),
        }
    }
    (!branches.is_empty()).then_some(BranchScope::Branches(branches))
}

fn current_trace_context() -> TraceContext {
    TraceContext::generate()
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

    fn malformed_request() -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", "malformed request")
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            message,
        )
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }

    fn from_kernel(error: KernelError) -> Self {
        match error.kind {
            ErrorKind::Validation => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                error.message,
            ),
            ErrorKind::Forbidden => Self::new(StatusCode::FORBIDDEN, "forbidden", error.message),
            ErrorKind::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", error.message),
            ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                Self::new(StatusCode::CONFLICT, "conflict", error.message)
            }
            ErrorKind::Internal => Self::internal(error.message),
        }
    }

    fn from_store(error: PgDispatchError) -> Self {
        match error {
            // Domain errors carry safe, caller-facing messages.
            PgDispatchError::Domain(kernel) => Self::from_kernel(kernel),
            // Db errors must never surface raw sqlx strings / 23505 constraint
            // names to the client (schema disclosure, OWASP A05). Classify by
            // kind, log the raw error server-side, return stable generic messages.
            db_error => {
                let kind = db_error.kind();
                tracing::error!(error = %db_error, "dispatch database error");
                match kind {
                    ErrorKind::NotFound => {
                        Self::new(StatusCode::NOT_FOUND, "not_found", "resource not found")
                    }
                    ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                        Self::new(StatusCode::CONFLICT, "conflict", "resource already exists")
                    }
                    _ => Self::internal("internal server error"),
                }
            }
        }
    }

    fn from_jobs(error: JobQueueError) -> Self {
        // Job-queue failures are internal; log the detail, return a stable message.
        tracing::error!(error = %error, "job queue error");
        Self::internal("internal server error")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_queue_request_rejects_unknown_query_contract() {
        let parsed: DispatchQueueRequest =
            serde_json::from_str(r#"{"status":"RECEIVED,DELAYED","limit":50,"after":"opaque"}"#)
                .expect("documented queue query is accepted");
        assert_eq!(parsed.limit, Some(50));
        assert!(
            serde_json::from_str::<DispatchQueueRequest>(
                r#"{"status":"RECEIVED","branch_id":"scope-escalation"}"#,
            )
            .is_err()
        );
    }

    #[test]
    fn dispatch_route_surface_includes_bounded_console_reads() {
        assert!(DISPATCH_ROUTE_PATHS.contains(&CONSOLE_DISPATCH_QUEUE_PATH));
        assert!(DISPATCH_ROUTE_PATHS.contains(&P1_DISPATCH_CANDIDATES_PATH_TEMPLATE));
        assert!(DISPATCH_ROUTE_PATHS.contains(&P1_DISPATCH_RESPONSES_PATH_TEMPLATE));
    }
}
