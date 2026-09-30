//! Inspection REST API.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_inspection_adapter_postgres::{PgInspectionError, PgInspectionStore};
use console_inspection_application::{
    CompleteInspectionRoundCommand, CreateInspectionScheduleCommand, ListInspectionSchedulesQuery,
    ListMyInspectionSchedulesQuery,
};
use console_inspection_domain::{InspectionCycle, InspectionRoundOutcome};
use console_kernel_core::{
    BranchId, EquipmentId, ErrorKind, InspectionScheduleId, KernelError, TraceContext, UserId,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize, authorize_capability};
use console_platform_db::DbError;
use serde::{Deserialize, Serialize};
use time::macros::format_description;
use time::{Date, OffsetDateTime};

// The OpenAPI contract types `due_date` as a `string` (ISO `YYYY-MM-DD`), but a
// bare `time::Date` serde impl expects the array form and rejects the string the
// web client sends (422). Deserialize it from the ISO date string instead — the
// same fix the work-order daily-plan `plan_date` already uses.
time::serde::format_description!(iso_date, Date, "[year]-[month]-[day]");

pub const INSPECTION_SCHEDULES_PATH: &str = "/api/v1/inspections/schedules";
pub const MY_INSPECTION_SCHEDULES_PATH: &str = "/api/v1/inspections/my-schedules";
pub const INSPECTION_ROUNDS_PATH_TEMPLATE: &str =
    "/api/v1/inspections/schedules/{schedule_id}/rounds";
pub const INSPECTION_ROUTE_PATHS: &[&str] = &[
    INSPECTION_SCHEDULES_PATH,
    MY_INSPECTION_SCHEDULES_PATH,
    INSPECTION_ROUNDS_PATH_TEMPLATE,
];

#[derive(Debug, Clone)]
pub struct InspectionRestState {
    store: PgInspectionStore,
    jwt_verifier: Option<JwtVerifier>,
}

impl InspectionRestState {
    #[must_use]
    pub fn new(store: PgInspectionStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
        }
    }
}

pub fn router(state: InspectionRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(
            INSPECTION_SCHEDULES_PATH,
            get(list_schedules).post(create_schedule),
        )
        .route(MY_INSPECTION_SCHEDULES_PATH, get(list_my_schedules))
        .route(
            "/api/v1/inspections/schedules/{schedule_id}/rounds",
            post(complete_round),
        )
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
struct CreateScheduleRequest {
    branch_id: BranchId,
    equipment_id: EquipmentId,
    mechanic_id: UserId,
    cycle: InspectionCycle,
    interval_days: i32,
    #[serde(with = "iso_date")]
    due_date: Date,
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CompleteRoundRequest {
    outcome: InspectionRoundOutcome,
    completed_at: Option<OffsetDateTime>,
    findings: String,
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListSchedulesRequest {
    due_start: String,
    due_end: String,
    /// Page size. Optional; the adapter clamps to `1..=500` and a missing value
    /// defaults below, so older clients that omit it still get a bounded page.
    limit: Option<i64>,
    /// Zero-based row offset for offset pagination. Optional, defaults to 0.
    offset: Option<i64>,
}

/// Default schedule page size when the client omits `limit`. Generous enough
/// that a typical month's roster fits in one page, but still bounded.
const DEFAULT_SCHEDULE_LIMIT: i64 = 200;

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
}

async fn create_schedule(
    State(state): State<InspectionRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateScheduleRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize(
        &principal,
        Action::new(Feature::InspectionScheduleManage),
        body.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let schedule = state
        .store
        .create_schedule(CreateInspectionScheduleCommand {
            actor: principal.user_id,
            branch_id: body.branch_id,
            equipment_id: body.equipment_id,
            mechanic_id: body.mechanic_id,
            cycle: body.cycle,
            interval_days: body.interval_days,
            due_date: body.due_date,
            note: body.note,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(schedule)))
}

async fn list_schedules(
    State(state): State<InspectionRestState>,
    headers: HeaderMap,
    Query(query): Query<ListSchedulesRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    // LIST gate: the page spans the caller's whole `branch_scope`, which
    // `list_due_schedules` filters on below, so there is no single resource
    // branch. The row-level sites (`create_schedule` / `complete_round`) pass the
    // real `regular_inspection_schedules.branch_id` and are unchanged.
    authorize_capability(&principal, Action::new(Feature::InspectionScheduleManage))
        .map_err(RestError::from_kernel)?;
    let offset = query.offset.unwrap_or(0);
    if offset < 0 {
        return Err(RestError::from_kernel(KernelError::validation(
            "offset must be non-negative",
        )));
    }
    let page = state
        .store
        .list_due_schedules(ListInspectionSchedulesQuery {
            branch_scope: principal.branch_scope,
            due_start: parse_date(&query.due_start)?,
            due_end: parse_date(&query.due_end)?,
            limit: query.limit.unwrap_or(DEFAULT_SCHEDULE_LIMIT),
            offset,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page))
}

async fn list_my_schedules(
    State(state): State<InspectionRestState>,
    headers: HeaderMap,
    Query(query): Query<ListSchedulesRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_capability(&principal, Action::new(Feature::InspectionRoundComplete))
        .map_err(RestError::from_kernel)?;
    let offset = query.offset.unwrap_or(0);
    if offset < 0 {
        return Err(RestError::from_kernel(KernelError::validation(
            "offset must be non-negative",
        )));
    }
    let page = state
        .store
        .list_my_due_schedules(ListMyInspectionSchedulesQuery {
            branch_scope: principal.branch_scope,
            mechanic_id: principal.user_id,
            due_start: parse_date(&query.due_start)?,
            due_end: parse_date(&query.due_end)?,
            limit: query.limit.unwrap_or(DEFAULT_SCHEDULE_LIMIT),
            offset,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page))
}

async fn complete_round(
    State(state): State<InspectionRestState>,
    headers: HeaderMap,
    Path(schedule_id): Path<uuid::Uuid>,
    Json(body): Json<CompleteRoundRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let schedule_id = InspectionScheduleId::from_uuid(schedule_id);
    let branch_id = state
        .store
        .schedule_branch_in_scope(schedule_id, &principal.branch_scope)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::InspectionRoundComplete),
        branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let occurred_at = OffsetDateTime::now_utc();
    let round = state
        .store
        .complete_round(CompleteInspectionRoundCommand {
            actor: principal.user_id,
            schedule_id,
            outcome: body.outcome,
            completed_at: body.completed_at.unwrap_or(occurred_at),
            findings: body.findings,
            note: body.note,
            trace: TraceContext::generate(),
            occurred_at,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(round)))
}

fn parse_date(raw: &str) -> Result<Date, RestError> {
    let format = format_description!("[year]-[month]-[day]");
    Date::parse(raw, &format).map_err(|_| RestError::bad_request("date must use YYYY-MM-DD"))
}

async fn principal_from_headers(
    state: &InspectionRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for inspection API")
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
            RestError::unavailable("JWT verification is not configured for inspection API")
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

#[derive(Debug)]
struct RestError {
    status: StatusCode,
    kind: ErrorKind,
    message: String,
}

impl RestError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            kind: ErrorKind::Validation,
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

    fn from_store(error: PgInspectionError) -> Self {
        match error {
            PgInspectionError::Domain(error) => Self::from_kernel(error),
            PgInspectionError::Db(error) => Self::from_db(error),
        }
    }

    fn from_db(error: DbError) -> Self {
        match error {
            DbError::Sqlx(sqlx::Error::RowNotFound) => {
                Self::from_kernel(KernelError::not_found("row was not found"))
            }
            DbError::Sqlx(sqlx::Error::Database(error))
                if error.code().is_some_and(|code| code == "23505") =>
            {
                // Log the constraint name server-side; never leak it (schema
                // disclosure, OWASP A05). Clients get a stable generic message.
                tracing::error!(error = %error, "database unique-constraint violation");
                Self::from_kernel(KernelError::conflict("resource already exists"))
            }
            DbError::Sqlx(error) => {
                tracing::error!(error = %error, "database error");
                Self::internal("internal server error")
            }
            DbError::Serialize(error) => {
                tracing::error!(error = %error, "serialization error");
                Self::internal("internal server error")
            }
            DbError::CodeIssuance(error) => {
                tracing::error!(error = %error, "object-code issuance error");
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
