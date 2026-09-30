//! BE-LC REST surface: period locks (freeze windows) + the generic object
//! lifecycle engine.
//!
//! Period locks are the enforcement substrate for UI-M7 month-close and UI-M8
//! 마감: locking `(domain, period)` makes every date-stamping payroll/financial
//! write inside the window fail closed (the guard lives in
//! `console_platform_db::period_lock` and is called from the domain write paths).
//! Lock/unlock/list are authority-gated (`Feature::PeriodLockManage`,
//! org-wide) and audited, following how the payroll admin endpoints gate.
//!
//! The lifecycle endpoints drive `console_platform_db::lifecycle`: a validated
//! per-object-type FSM (seeded `document` chain), an append-only transition
//! log, and legal-hold/retention dispose gates.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use console_kernel_core::{AuditAction, AuditEvent, ErrorKind, KernelError, TraceContext};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize_org_wide};
use console_platform_db::{
    DbError, PeriodLockDomain, lifecycle as lifecycle_db, lock_period_lock_key, with_audit,
    with_org_conn,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgPool, Row};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

pub const PERIOD_LOCKS_PATH: &str = "/api/v1/period-locks";
pub const PERIOD_LOCK_UNLOCK_PATH_TEMPLATE: &str = "/api/v1/period-locks/{lockId}/unlock";
pub const LIFECYCLE_PATH_TEMPLATE: &str = "/api/v1/lifecycles/{objectType}/{objectId}";
pub const LIFECYCLE_TRANSITION_PATH_TEMPLATE: &str =
    "/api/v1/lifecycles/{objectType}/{objectId}/transition";
pub const LIFECYCLE_HOLD_PATH_TEMPLATE: &str = "/api/v1/lifecycles/{objectType}/{objectId}/hold";

pub const LIFECYCLE_ROUTE_PATHS: &[&str] = &[
    PERIOD_LOCKS_PATH,
    PERIOD_LOCK_UNLOCK_PATH_TEMPLATE,
    LIFECYCLE_PATH_TEMPLATE,
    LIFECYCLE_TRANSITION_PATH_TEMPLATE,
    LIFECYCLE_HOLD_PATH_TEMPLATE,
];

const MAX_LIST_LIMIT: i64 = 200;
const DEFAULT_LIST_LIMIT: i64 = 50;

#[derive(Debug, Clone)]
pub struct LifecycleState {
    pool: PgPool,
    jwt_verifier: Option<JwtVerifier>,
}

impl LifecycleState {
    #[must_use]
    pub fn new(pool: PgPool, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self { pool, jwt_verifier }
    }
}

pub fn router(state: LifecycleState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool.clone();
    let router = Router::new()
        .route(
            PERIOD_LOCKS_PATH,
            get(list_period_locks).post(create_period_lock),
        )
        .route(PERIOD_LOCK_UNLOCK_PATH_TEMPLATE, post(unlock_period_lock))
        .route(LIFECYCLE_PATH_TEMPLATE, get(get_lifecycle))
        .route(
            LIFECYCLE_TRANSITION_PATH_TEMPLATE,
            post(transition_lifecycle),
        )
        .route(LIFECYCLE_HOLD_PATH_TEMPLATE, post(set_lifecycle_hold))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

// ===========================================================================
// Period locks.
// ===========================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatePeriodLockRequest {
    domain: String,
    period_start: String,
    period_end: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnlockPeriodLockRequest {
    reason: String,
}

#[derive(Debug, Deserialize)]
struct PeriodLockListQuery {
    domain: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PeriodLockResponse {
    id: Uuid,
    domain: String,
    period_start: String,
    period_end: String,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    locked_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    locked_at: OffsetDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    unlocked_by: Option<Uuid>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    unlocked_at: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unlock_reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct PeriodLockListResponse {
    items: Vec<PeriodLockResponse>,
}

fn period_lock_from_row(row: &sqlx::postgres::PgRow) -> Result<PeriodLockResponse, LifecycleError> {
    let period_start: Date = row.try_get("period_start")?;
    let period_end: Date = row.try_get("period_end")?;
    Ok(PeriodLockResponse {
        id: row.try_get("id")?,
        domain: row.try_get("domain")?,
        period_start: period_start.to_string(),
        period_end: period_end.to_string(),
        reason: row.try_get("reason")?,
        locked_by: row.try_get("locked_by")?,
        locked_at: row.try_get("locked_at")?,
        unlocked_by: row.try_get("unlocked_by")?,
        unlocked_at: row.try_get("unlocked_at")?,
        unlock_reason: row.try_get("unlock_reason")?,
    })
}

fn parse_iso_date(raw: &str, field: &str) -> Result<Date, LifecycleError> {
    Date::parse(raw, &time::format_description::well_known::Iso8601::DATE).map_err(|_| {
        LifecycleError::validation(format!("{field} must be an ISO date (YYYY-MM-DD)"))
    })
}

async fn list_period_locks(
    State(state): State<LifecycleState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<PeriodLockListQuery>,
) -> Result<Json<PeriodLockListResponse>, LifecycleError> {
    authorize_period_lock_manage(&principal)?;
    let domain = query
        .domain
        .as_deref()
        .map(PeriodLockDomain::parse)
        .transpose()?
        .map(PeriodLockDomain::as_str);
    let limit = query
        .limit
        .unwrap_or(DEFAULT_LIST_LIMIT)
        .clamp(1, MAX_LIST_LIMIT);
    let org = principal.org_id;

    let items = with_org_conn::<_, _, LifecycleError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT id, domain, period_start, period_end, reason, locked_by, locked_at, \
                        unlocked_by, unlocked_at, unlock_reason \
                 FROM period_locks \
                 WHERE ($1::text IS NULL OR domain = $1) \
                 ORDER BY locked_at DESC \
                 LIMIT $2",
            )
            .bind(domain)
            .bind(limit)
            .fetch_all(tx.as_mut())
            .await?;
            rows.iter().map(period_lock_from_row).collect()
        })
    })
    .await?;
    Ok(Json(PeriodLockListResponse { items }))
}

async fn create_period_lock(
    State(state): State<LifecycleState>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<CreatePeriodLockRequest>,
) -> Result<(StatusCode, Json<PeriodLockResponse>), LifecycleError> {
    authorize_period_lock_manage(&principal)?;
    let domain = PeriodLockDomain::parse(&body.domain)?;
    let period_start = parse_iso_date(&body.period_start, "periodStart")?;
    let period_end = parse_iso_date(&body.period_end, "periodEnd")?;
    if period_end < period_start {
        return Err(LifecycleError::validation(
            "periodEnd must not precede periodStart",
        ));
    }
    let reason = body.reason.trim().to_owned();
    if reason.is_empty() {
        return Err(LifecycleError::validation("reason must not be blank"));
    }

    let lock_id = Uuid::new_v4();
    let org = principal.org_id;
    let actor = principal.user_id;
    let audit_event = AuditEvent::new(
        Some(actor),
        AuditAction::new("period_lock.lock")?,
        "period_lock",
        lock_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(json!({
            "domain": domain.as_str(),
            "period_start": period_start.to_string(),
            "period_end": period_end.to_string(),
            "reason": reason,
        })),
    );

    let response = with_audit::<_, _, LifecycleError>(&state.pool, audit_event, move |tx| {
        Box::pin(async move {
            // Serialize the lock CREATE with the drain's gated staging write:
            // both take this per-org advisory lock, so a lock cannot commit
            // between the staging INSERT's snapshot and its commit.
            lock_period_lock_key(tx, domain, *org.as_uuid()).await?;
            // Refuse a duplicate active lock covering the same window: one
            // active lock per (domain, window) keeps unlock semantics obvious.
            let overlapping: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM period_locks \
                 WHERE domain = $1 AND unlocked_at IS NULL \
                   AND period_start <= $3 AND period_end >= $2",
            )
            .bind(domain.as_str())
            .bind(period_start)
            .bind(period_end)
            .fetch_one(tx.as_mut())
            .await?;
            if overlapping > 0 {
                return Err(LifecycleError::from_kernel(KernelError::conflict(format!(
                    "an active {domain} lock already overlaps {period_start}..{period_end}"
                ))));
            }

            let row = sqlx::query(
                "INSERT INTO period_locks \
                     (id, org_id, domain, period_start, period_end, reason, locked_by) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7) \
                 RETURNING id, domain, period_start, period_end, reason, locked_by, locked_at, \
                           unlocked_by, unlocked_at, unlock_reason",
            )
            .bind(lock_id)
            .bind(*org.as_uuid())
            .bind(domain.as_str())
            .bind(period_start)
            .bind(period_end)
            .bind(&reason)
            .bind(*actor.as_uuid())
            .fetch_one(tx.as_mut())
            .await?;
            period_lock_from_row(&row)
        })
    })
    .await?;
    Ok((StatusCode::CREATED, Json(response)))
}

async fn unlock_period_lock(
    State(state): State<LifecycleState>,
    Extension(principal): Extension<Principal>,
    Path(lock_id): Path<Uuid>,
    Json(body): Json<UnlockPeriodLockRequest>,
) -> Result<Json<PeriodLockResponse>, LifecycleError> {
    authorize_period_lock_manage(&principal)?;
    let reason = body.reason.trim().to_owned();
    if reason.is_empty() {
        return Err(LifecycleError::validation("reason must not be blank"));
    }

    let org = principal.org_id;
    let actor = principal.user_id;
    let audit_event = AuditEvent::new(
        Some(actor),
        AuditAction::new("period_lock.unlock")?,
        "period_lock",
        lock_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(None, Some(json!({ "unlock_reason": reason })));

    let response = with_audit::<_, _, LifecycleError>(&state.pool, audit_event, move |tx| {
        Box::pin(async move {
            let existing =
                sqlx::query("SELECT unlocked_at FROM period_locks WHERE id = $1 FOR UPDATE")
                    .bind(lock_id)
                    .fetch_optional(tx.as_mut())
                    .await?;
            let Some(existing) = existing else {
                return Err(LifecycleError::from_kernel(KernelError::not_found(
                    "period lock was not found",
                )));
            };
            let already_unlocked: Option<OffsetDateTime> = existing.try_get("unlocked_at")?;
            if already_unlocked.is_some() {
                return Err(LifecycleError::from_kernel(KernelError::conflict(
                    "period lock is already unlocked",
                )));
            }

            let row = sqlx::query(
                "UPDATE period_locks \
                 SET unlocked_at = now(), unlocked_by = $2, unlock_reason = $3 \
                 WHERE id = $1 \
                 RETURNING id, domain, period_start, period_end, reason, locked_by, locked_at, \
                           unlocked_by, unlocked_at, unlock_reason",
            )
            .bind(lock_id)
            .bind(*actor.as_uuid())
            .bind(&reason)
            .fetch_one(tx.as_mut())
            .await?;
            period_lock_from_row(&row)
        })
    })
    .await?;
    Ok(Json(response))
}

// ===========================================================================
// Object lifecycles.
// ===========================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransitionLifecycleRequest {
    to_state: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetLifecycleHoldRequest {
    legal_hold: bool,
    #[serde(default)]
    retention_until: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LifecycleTransitionResponse {
    from_state: String,
    to_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    actor: Option<Uuid>,
    reason: String,
    #[serde(with = "time::serde::rfc3339")]
    occurred_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LifecycleResponse {
    object_type: String,
    object_id: Uuid,
    current_state: String,
    legal_hold: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    retention_until: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    transitions: Vec<LifecycleTransitionResponse>,
}

fn lifecycle_response(
    record: lifecycle_db::LifecycleRecord,
    transitions: Vec<lifecycle_db::LifecycleTransitionRecord>,
) -> LifecycleResponse {
    LifecycleResponse {
        object_type: record.object_type,
        object_id: record.object_id,
        current_state: record.current_state,
        legal_hold: record.legal_hold,
        retention_until: record.retention_until.map(|d| d.to_string()),
        created_at: record.created_at,
        updated_at: record.updated_at,
        transitions: transitions
            .into_iter()
            .map(|t| LifecycleTransitionResponse {
                from_state: t.from_state,
                to_state: t.to_state,
                actor: t.actor,
                reason: t.reason,
                occurred_at: t.occurred_at,
            })
            .collect(),
    }
}

fn validate_object_type(raw: &str) -> Result<(), LifecycleError> {
    let valid = (2..=64).contains(&raw.len())
        && raw.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && raw
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(LifecycleError::validation(
            "objectType must be a lowercase snake_case slug",
        ))
    }
}

async fn get_lifecycle(
    State(state): State<LifecycleState>,
    Extension(principal): Extension<Principal>,
    Path((object_type, object_id)): Path<(String, Uuid)>,
) -> Result<Json<LifecycleResponse>, LifecycleError> {
    authorize_lifecycle_manage(&principal)?;
    validate_object_type(&object_type)?;
    let org = principal.org_id;

    let response = with_org_conn::<_, _, LifecycleError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let record = lifecycle_db::get_lifecycle(tx, &object_type, object_id)
                .await?
                .ok_or_else(|| {
                    LifecycleError::from_kernel(KernelError::not_found(
                        "object has no lifecycle record",
                    ))
                })?;
            let transitions = lifecycle_db::list_transitions(tx, record.id).await?;
            Ok(lifecycle_response(record, transitions))
        })
    })
    .await?;
    Ok(Json(response))
}

async fn transition_lifecycle(
    State(state): State<LifecycleState>,
    Extension(principal): Extension<Principal>,
    Path((object_type, object_id)): Path<(String, Uuid)>,
    Json(body): Json<TransitionLifecycleRequest>,
) -> Result<Json<LifecycleResponse>, LifecycleError> {
    authorize_lifecycle_manage(&principal)?;
    validate_object_type(&object_type)?;
    let to_state = body.to_state.trim().to_owned();
    let reason = body.reason.trim().to_owned();
    let org = principal.org_id;
    let actor = principal.user_id;
    let now = OffsetDateTime::now_utc();

    let audit_event = AuditEvent::new(
        Some(actor),
        AuditAction::new("lifecycle.transition")?,
        "object_lifecycle",
        format!("{object_type}:{object_id}"),
        TraceContext::generate(),
        now,
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(json!({
            "object_type": object_type,
            "object_id": object_id,
            "to_state": to_state,
            "reason": reason,
        })),
    );

    let response = with_audit::<_, _, LifecycleError>(&state.pool, audit_event, move |tx| {
        Box::pin(async move {
            let record = lifecycle_db::transition_lifecycle(
                tx,
                *org.as_uuid(),
                &object_type,
                object_id,
                &to_state,
                Some(*actor.as_uuid()),
                &reason,
                now.date(),
            )
            .await
            .map_err(LifecycleError::from_kernel)?;
            let transitions = lifecycle_db::list_transitions(tx, record.id).await?;
            Ok(lifecycle_response(record, transitions))
        })
    })
    .await?;
    Ok(Json(response))
}

async fn set_lifecycle_hold(
    State(state): State<LifecycleState>,
    Extension(principal): Extension<Principal>,
    Path((object_type, object_id)): Path<(String, Uuid)>,
    Json(body): Json<SetLifecycleHoldRequest>,
) -> Result<Json<LifecycleResponse>, LifecycleError> {
    authorize_lifecycle_manage(&principal)?;
    validate_object_type(&object_type)?;
    let retention_until = body
        .retention_until
        .as_deref()
        .map(|raw| parse_iso_date(raw, "retentionUntil"))
        .transpose()?;
    let legal_hold = body.legal_hold;
    let org = principal.org_id;
    let actor = principal.user_id;

    let audit_event = AuditEvent::new(
        Some(actor),
        AuditAction::new("lifecycle.hold_set")?,
        "object_lifecycle",
        format!("{object_type}:{object_id}"),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(
        None,
        Some(json!({
            "object_type": object_type,
            "object_id": object_id,
            "legal_hold": legal_hold,
            "retention_until": retention_until.map(|d| d.to_string()),
        })),
    );

    let response = with_audit::<_, _, LifecycleError>(&state.pool, audit_event, move |tx| {
        Box::pin(async move {
            let record = lifecycle_db::set_lifecycle_hold(
                tx,
                *org.as_uuid(),
                &object_type,
                object_id,
                legal_hold,
                retention_until,
            )
            .await
            .map_err(LifecycleError::from_kernel)?;
            let transitions = lifecycle_db::list_transitions(tx, record.id).await?;
            Ok(lifecycle_response(record, transitions))
        })
    })
    .await?;
    Ok(Json(response))
}

// ===========================================================================
// Authorization + error surface.
// ===========================================================================

fn authorize_period_lock_manage(principal: &Principal) -> Result<(), LifecycleError> {
    authorize_org_wide(principal, Action::new(Feature::PeriodLockManage))
        .map_err(LifecycleError::from_kernel)
}

fn authorize_lifecycle_manage(principal: &Principal) -> Result<(), LifecycleError> {
    authorize_org_wide(principal, Action::new(Feature::LifecycleManage))
        .map_err(LifecycleError::from_kernel)
}

#[derive(Debug)]
pub struct LifecycleError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl LifecycleError {
    fn from_kernel(error: KernelError) -> Self {
        let status = match error.kind {
            ErrorKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorKind::NotFound => StatusCode::NOT_FOUND,
            ErrorKind::Forbidden => StatusCode::FORBIDDEN,
            ErrorKind::Conflict | ErrorKind::InvalidTransition => StatusCode::CONFLICT,
            ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let code = match error.kind {
            ErrorKind::Validation => "validation",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Forbidden => "forbidden",
            ErrorKind::Conflict => "conflict",
            ErrorKind::InvalidTransition => "invalid_transition",
            ErrorKind::Internal => "internal",
        };
        Self {
            status,
            code,
            message: error.message,
        }
    }

    fn validation(message: impl Into<String>) -> Self {
        Self::from_kernel(KernelError::validation(message.into()))
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: message.into(),
        }
    }
}

impl From<KernelError> for LifecycleError {
    fn from(error: KernelError) -> Self {
        Self::from_kernel(error)
    }
}

impl From<DbError> for LifecycleError {
    fn from(value: DbError) -> Self {
        tracing::error!(error = %value, "lifecycle database operation failed");
        Self::internal("lifecycle request failed")
    }
}

impl From<sqlx::Error> for LifecycleError {
    fn from(value: sqlx::Error) -> Self {
        Self::from(DbError::Sqlx(value))
    }
}

impl IntoResponse for LifecycleError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response()
    }
}
