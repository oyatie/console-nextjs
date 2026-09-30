//! REST API for the deterministic analytics projection service.
//!
//! One route, `POST /api/v1/analytics/projection`. The service is stateless and
//! read-only: it computes over the caller-supplied series, touching no tenant
//! table, so there is no RLS surface here. Authentication is enforced by the
//! shared request-context middleware (which also binds the tenant org), and the
//! handler additionally gates on the `KpiRead` analytics-read capability.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use console_analytics_quant_service::{ProjectionRequest, ProjectionResult, SeriesKind, project};
use console_kernel_core::{ErrorKind, KernelError};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize_capability};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

/// Projection endpoint path.
pub const PROJECTION_PATH: &str = "/api/v1/analytics/projection";

/// Route inventory for this surface (consumed by the app route-inventory gate).
pub const ANALYTICS_QUANT_ROUTE_PATHS: &[&str] = &[PROJECTION_PATH];

/// State needed to authenticate requests. The projection compute itself is
/// stateless; the pool + verifier exist only for the auth middleware.
#[derive(Debug, Clone)]
pub struct AnalyticsQuantState {
    pool: PgPool,
    jwt_verifier: Option<JwtVerifier>,
}

impl AnalyticsQuantState {
    #[must_use]
    pub fn new(pool: PgPool, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self { pool, jwt_verifier }
    }
}

/// Build the analytics-quant router, auth-wrapped.
pub fn router(state: AnalyticsQuantState) -> Router {
    let router = Router::new().route(PROJECTION_PATH, post(post_projection));
    console_platform_request_context::with_request_context(router, state.jwt_verifier, state.pool)
}

/// Request body for the projection endpoint.
#[derive(Debug, Deserialize)]
struct ProjectionBody {
    series: Vec<f64>,
    horizon: u32,
    kind: SeriesKind,
}

impl ProjectionBody {
    fn into_domain(self) -> ProjectionRequest {
        ProjectionRequest {
            series: self.series,
            horizon: self.horizon,
            kind: self.kind,
        }
    }
}

async fn post_projection(
    Extension(principal): Extension<Principal>,
    Json(body): Json<ProjectionBody>,
) -> Result<Json<ProjectionResult>, RestError> {
    authorize_capability(&principal, Action::new(Feature::KpiRead))
        .map_err(RestError::from_kernel)?;

    let result =
        project(&body.into_domain()).map_err(|err| RestError::bad_request(err.to_string()))?;
    Ok(Json(result))
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

    fn from_kernel(error: KernelError) -> Self {
        let status = match error.kind {
            ErrorKind::Validation | ErrorKind::InvalidTransition => StatusCode::BAD_REQUEST,
            ErrorKind::NotFound => StatusCode::NOT_FOUND,
            ErrorKind::Forbidden => StatusCode::FORBIDDEN,
            ErrorKind::Conflict => StatusCode::CONFLICT,
            ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            kind: error.kind,
            message: error.message,
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
        let body = ErrorBody {
            error: ErrorPayload {
                code: self.code(),
                message: self.message,
            },
        };
        (self.status, Json(body)).into_response()
    }
}
