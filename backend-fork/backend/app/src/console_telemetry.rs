//! Tenant console adoption/RUM telemetry ingestion.
//!
//! The browser sends only cardinality-safe route labels and bounded event labels;
//! org/user identity is derived from the authenticated tenant token. Reads for
//! ramp decisions happen through the platform ops SECURITY DEFINER rollup, not by
//! exposing row-level telemetry back to the client.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use console_kernel_core::OrgId;
use console_platform_auth::JwtVerifier;
use console_platform_authz::Principal;
use console_platform_db::{DbError, with_org_conn};
use console_platform_request_context::{current_org, with_request_context};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

pub const CONSOLE_ROUTE_TELEMETRY_PATH: &str = "/api/v1/console/telemetry/route";
const MAX_ROUTE_PATH_LEN: usize = 120;
const MAX_RELEASE_CYCLE_LEN: usize = 80;
const MAX_ERROR_NAME_LEN: usize = 80;
const MAX_DURATION_MS: i32 = 600_000;

#[derive(Clone)]
pub struct ConsoleTelemetryState {
    pool: PgPool,
    jwt_verifier: Option<JwtVerifier>,
}

impl ConsoleTelemetryState {
    #[must_use]
    pub fn new(pool: PgPool, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self { pool, jwt_verifier }
    }
}

pub fn router(state: ConsoleTelemetryState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool.clone();
    let router = Router::new()
        .route(CONSOLE_ROUTE_TELEMETRY_PATH, post(record_route_telemetry))
        .with_state(state);
    with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
struct RouteTelemetryRequest {
    event_kind: RouteTelemetryEventKind,
    route_surface: RouteSurface,
    route_path: String,
    release_cycle: String,
    #[serde(default)]
    duration_ms: Option<i32>,
    #[serde(default)]
    error_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RouteTelemetryEventKind {
    RouteSelection,
    RumError,
    RumPerf,
}

impl RouteTelemetryEventKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::RouteSelection => "route_selection",
            Self::RumError => "rum_error",
            Self::RumPerf => "rum_perf",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RouteSurface {
    Console,
    Legacy,
}

impl RouteSurface {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Console => "console",
            Self::Legacy => "legacy",
        }
    }
}

#[derive(Debug)]
struct NormalizedRouteTelemetry {
    event_kind: RouteTelemetryEventKind,
    route_surface: RouteSurface,
    route_path: String,
    release_cycle: String,
    duration_ms: Option<i32>,
    error_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct AcceptedBody {
    accepted: bool,
}

async fn record_route_telemetry(
    State(state): State<ConsoleTelemetryState>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<RouteTelemetryRequest>,
) -> Result<impl IntoResponse, TelemetryError> {
    let org = current_org()?;
    let normalized = normalize(body)?;
    insert_route_telemetry(&state.pool, org, &principal, normalized).await?;
    Ok((StatusCode::ACCEPTED, Json(AcceptedBody { accepted: true })))
}

fn normalize(body: RouteTelemetryRequest) -> Result<NormalizedRouteTelemetry, TelemetryError> {
    let route_path = normalize_route_path(body.route_path)?;
    let release_cycle = normalize_bounded_label(
        body.release_cycle,
        MAX_RELEASE_CYCLE_LEN,
        "release_cycle is required and must be a bounded safe label",
    )?;
    let duration_ms = match body.duration_ms {
        Some(value) if !(0..=MAX_DURATION_MS).contains(&value) => {
            return Err(TelemetryError::validation(format!(
                "duration_ms must be between 0 and {MAX_DURATION_MS}"
            )));
        }
        value => value,
    };
    let error_name = body
        .error_name
        .map(|value| {
            normalize_bounded_label(
                value,
                MAX_ERROR_NAME_LEN,
                "error_name must be a bounded safe label",
            )
        })
        .transpose()?;

    Ok(NormalizedRouteTelemetry {
        event_kind: body.event_kind,
        route_surface: body.route_surface,
        route_path,
        release_cycle,
        duration_ms,
        error_name,
    })
}

fn normalize_route_path(value: String) -> Result<String, TelemetryError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_ROUTE_PATH_LEN
        || !value.starts_with('/')
        || value.contains('?')
        || value.contains('#')
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | ':' | '-' | '_' | '.'))
    {
        return Err(TelemetryError::validation(
            "route_path must be a bounded cardinality-safe path template",
        ));
    }
    Ok(value.to_owned())
}

fn normalize_bounded_label(
    value: String,
    max_len: usize,
    message: &'static str,
) -> Result<String, TelemetryError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > max_len
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':'))
    {
        return Err(TelemetryError::validation(message));
    }
    Ok(value.to_owned())
}

// console-gate: state-changing-handler
// console-gate: audit-exempt console_route_telemetry_ingestion
async fn insert_route_telemetry(
    pool: &PgPool,
    org: OrgId,
    principal: &Principal,
    telemetry: NormalizedRouteTelemetry,
) -> Result<(), TelemetryError> {
    let org_uuid = *org.as_uuid();
    let user_uuid = *principal.user_id.as_uuid();
    let event_kind = telemetry.event_kind.as_str();
    let route_surface = telemetry.route_surface.as_str();
    with_org_conn::<_, _, TelemetryError>(pool, org, |tx| {
        Box::pin(async move {
            sqlx::query(
                r#"
                INSERT INTO console_route_telemetry (
                    org_id, user_id, event_kind, route_surface, route_path,
                    release_cycle, duration_ms, error_name
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                "#,
            )
            .bind(org_uuid)
            .bind(user_uuid)
            .bind(event_kind)
            .bind(route_surface)
            .bind(&telemetry.route_path)
            .bind(&telemetry.release_cycle)
            .bind(telemetry.duration_ms)
            .bind(&telemetry.error_name)
            .execute(tx.as_mut())
            .await?;
            Ok(())
        })
    })
    .await?;

    metrics::counter!(
        "console_route_telemetry_events_total",
        "event_kind" => event_kind,
        "route_surface" => route_surface,
    )
    .increment(1);
    Ok(())
}

#[derive(Debug)]
struct TelemetryError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl TelemetryError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn validation(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "validation", message)
    }
}

impl From<DbError> for TelemetryError {
    fn from(err: DbError) -> Self {
        tracing::error!(error = %err, "console route telemetry database error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal server error",
        )
    }
}

impl From<sqlx::Error> for TelemetryError {
    fn from(err: sqlx::Error) -> Self {
        Self::from(DbError::Sqlx(err))
    }
}

impl From<console_platform_request_context::RequestContextError> for TelemetryError {
    fn from(err: console_platform_request_context::RequestContextError) -> Self {
        tracing::error!(error = %err, "console route telemetry request context error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal server error",
        )
    }
}

impl IntoResponse for TelemetryError {
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

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
}
