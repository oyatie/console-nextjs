//! Authenticated evaluation-console routes (CAP-EVALUATION-CONSOLE).
//!
//! Evaluation is an ORG-LEVEL HR module (no branch column): feature checks
//! authorize against a representative branch exactly like the sales catalog.
//! Beyond the feature matrix the boundary enforces deny-by-omission — a
//! caller who is neither an `EvaluationRead` holder nor the subject's
//! assigned manager receives 404, never 403, for a concrete subject — and
//! field bounds return 422 before any DB CHECK can fire. All SQL, FSM
//! preflights, SoD, and the `with_audits` carve-outs live in the adapter.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use console_evaluation_adapter_postgres::{PgEvaluationError, PgEvaluationStore, SubjectGate};
use console_evaluation_application::{
    CreateCycleInput, CycleDetail, CyclePage, CycleQuery, EvidenceInput, GoalInput, LedgerPage,
    PreflightReport, ReviewDraftInput, ReviewView, SubjectDetail, TaskPage,
};
use console_evaluation_domain::{CycleStage, CycleTransition, Grade, ReviewKind};
use console_kernel_core::{ErrorKind, KernelError, UserId};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize_capability};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Route paths (exported for the openapi drift test)
// ---------------------------------------------------------------------------

pub const EVALUATION_CYCLES_PATH: &str = "/api/v1/evaluation/cycles";
pub const EVALUATION_CYCLE_PATH_TEMPLATE: &str = "/api/v1/evaluation/cycles/{cycle_id}";
pub const EVALUATION_CYCLE_PREFLIGHT_PATH_TEMPLATE: &str =
    "/api/v1/evaluation/cycles/{cycle_id}/preflight";
pub const EVALUATION_CYCLE_OPEN_PATH_TEMPLATE: &str = "/api/v1/evaluation/cycles/{cycle_id}/open";
pub const EVALUATION_CYCLE_START_CALIBRATION_PATH_TEMPLATE: &str =
    "/api/v1/evaluation/cycles/{cycle_id}/start-calibration";
pub const EVALUATION_CYCLE_FINALIZE_PATH_TEMPLATE: &str =
    "/api/v1/evaluation/cycles/{cycle_id}/finalize";
pub const EVALUATION_CYCLE_ARCHIVE_PATH_TEMPLATE: &str =
    "/api/v1/evaluation/cycles/{cycle_id}/archive";
pub const EVALUATION_SUBJECTS_PATH: &str = "/api/v1/evaluation/subjects";
pub const EVALUATION_SUBJECT_PATH_TEMPLATE: &str = "/api/v1/evaluation/subjects/{subject_id}";
pub const EVALUATION_SUBJECT_GOALS_PATH_TEMPLATE: &str =
    "/api/v1/evaluation/subjects/{subject_id}/goals";
pub const EVALUATION_SUBJECT_REVIEW_PATH_TEMPLATE: &str =
    "/api/v1/evaluation/subjects/{subject_id}/reviews/{kind}";
pub const EVALUATION_SUBJECT_REVIEW_SUBMIT_PATH_TEMPLATE: &str =
    "/api/v1/evaluation/subjects/{subject_id}/reviews/{kind}/submit";
pub const EVALUATION_SUBJECT_CALIBRATE_PATH_TEMPLATE: &str =
    "/api/v1/evaluation/subjects/{subject_id}/calibrate";
pub const EVALUATION_MY_TASKS_PATH: &str = "/api/v1/evaluation/my-tasks";
pub const EVALUATION_EMPLOYEE_REVIEWS_PATH_TEMPLATE: &str =
    "/api/v1/evaluation/employees/{employee_id}/reviews";
pub const EVALUATION_ROUTE_PATHS: &[&str] = &[
    EVALUATION_CYCLES_PATH,
    EVALUATION_CYCLE_PATH_TEMPLATE,
    EVALUATION_CYCLE_PREFLIGHT_PATH_TEMPLATE,
    EVALUATION_CYCLE_OPEN_PATH_TEMPLATE,
    EVALUATION_CYCLE_START_CALIBRATION_PATH_TEMPLATE,
    EVALUATION_CYCLE_FINALIZE_PATH_TEMPLATE,
    EVALUATION_CYCLE_ARCHIVE_PATH_TEMPLATE,
    EVALUATION_SUBJECTS_PATH,
    EVALUATION_SUBJECT_PATH_TEMPLATE,
    EVALUATION_SUBJECT_GOALS_PATH_TEMPLATE,
    EVALUATION_SUBJECT_REVIEW_PATH_TEMPLATE,
    EVALUATION_SUBJECT_REVIEW_SUBMIT_PATH_TEMPLATE,
    EVALUATION_SUBJECT_CALIBRATE_PATH_TEMPLATE,
    EVALUATION_MY_TASKS_PATH,
    EVALUATION_EMPLOYEE_REVIEWS_PATH_TEMPLATE,
];

// Field bounds (migration 0190 CHECK constraints, rejected as 422 here first).
const NAME_MAX_CHARS: usize = 120;
const PERIOD_LABEL_MAX_CHARS: usize = 60;
const GOAL_TEXT_MAX_CHARS: usize = 200;
const GOALS_MAX: usize = 20;
const NOTE_MAX_CHARS: usize = 2000;
const EVIDENCE_MAX: usize = 10;
const OBJECT_REF_MAX_CHARS: usize = 120;
const EVIDENCE_LABEL_MAX_CHARS: usize = 200;
const CALIBRATION_REASON_MAX_CHARS: usize = 500;
const DEFAULT_CYCLE_LIMIT: i64 = 50;
const MAX_CYCLE_LIMIT: i64 = 100;

#[derive(Clone)]
pub struct EvaluationRestState {
    store: PgEvaluationStore,
    jwt_verifier: Option<JwtVerifier>,
}

impl EvaluationRestState {
    #[must_use]
    pub fn new(store: PgEvaluationStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
        }
    }
}

pub fn router(state: EvaluationRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let routes = Router::new()
        .route(EVALUATION_CYCLES_PATH, get(list_cycles).post(create_cycle))
        .route(EVALUATION_CYCLE_PATH_TEMPLATE, get(get_cycle))
        .route(EVALUATION_CYCLE_PREFLIGHT_PATH_TEMPLATE, get(preflight))
        .route(EVALUATION_CYCLE_OPEN_PATH_TEMPLATE, post(open_cycle))
        .route(
            EVALUATION_CYCLE_START_CALIBRATION_PATH_TEMPLATE,
            post(start_calibration),
        )
        .route(
            EVALUATION_CYCLE_FINALIZE_PATH_TEMPLATE,
            post(finalize_cycle),
        )
        .route(EVALUATION_CYCLE_ARCHIVE_PATH_TEMPLATE, post(archive_cycle))
        .route(EVALUATION_SUBJECTS_PATH, post(add_subject))
        .route(EVALUATION_SUBJECT_PATH_TEMPLATE, get(get_subject))
        .route(EVALUATION_SUBJECT_GOALS_PATH_TEMPLATE, put(replace_goals))
        .route(EVALUATION_SUBJECT_REVIEW_PATH_TEMPLATE, put(save_review))
        .route(
            EVALUATION_SUBJECT_REVIEW_SUBMIT_PATH_TEMPLATE,
            post(submit_review),
        )
        .route(EVALUATION_SUBJECT_CALIBRATE_PATH_TEMPLATE, post(calibrate))
        .route(EVALUATION_MY_TASKS_PATH, get(my_tasks))
        .route(
            EVALUATION_EMPLOYEE_REVIEWS_PATH_TEMPLATE,
            get(employee_reviews),
        )
        .with_state(state);
    console_platform_request_context::with_request_context(routes, verifier, pool)
}

// ---------------------------------------------------------------------------
// Request / query DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateCycleRequest {
    name: String,
    kind: console_evaluation_domain::CycleKind,
    period_label: String,
    #[serde(with = "console_evaluation_application::date_fmt")]
    due_date: time::Date,
}

#[derive(Debug, Deserialize)]
struct CycleListFilter {
    stage: Option<CycleStage>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AddSubjectRequest {
    cycle_id: Uuid,
    employee_id: Uuid,
    manager_user_id: Uuid,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoalRequest {
    title: String,
    metric_kind: console_evaluation_domain::MetricKind,
    target_label: String,
    weight_pct: i16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplaceGoalsRequest {
    goals: Vec<GoalRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceLinkRequest {
    object_kind: console_evaluation_domain::EvidenceKind,
    object_ref: String,
    label: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpsertReviewRequest {
    #[serde(default)]
    grade: Option<Grade>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    evidence_links: Vec<EvidenceLinkRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CalibrateRequest {
    final_grade: Grade,
    #[serde(default)]
    reason: Option<String>,
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

// ---------------------------------------------------------------------------
// Cycle handlers
// ---------------------------------------------------------------------------

async fn create_cycle(
    State(state): State<EvaluationRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateCycleRequest>,
) -> Result<(StatusCode, Json<CycleDetail>), RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_feature(&principal, Feature::EvaluationManage)?;
    let name = bounded_required(&body.name, 1, NAME_MAX_CHARS, "name")?;
    let period_label = bounded_required(
        &body.period_label,
        1,
        PERIOD_LABEL_MAX_CHARS,
        "period_label",
    )?;
    let detail = state
        .store
        .create_cycle(
            principal.user_id,
            CreateCycleInput {
                name,
                kind: body.kind,
                period_label,
                due_date: body.due_date,
            },
        )
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(detail)))
}

async fn list_cycles(
    State(state): State<EvaluationRestState>,
    headers: HeaderMap,
    Query(filter): Query<CycleListFilter>,
) -> Result<Json<CyclePage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_feature(&principal, Feature::EvaluationRead)?;
    let page = state
        .store
        .list_cycles(CycleQuery {
            stage: filter.stage,
            limit: filter
                .limit
                .unwrap_or(DEFAULT_CYCLE_LIMIT)
                .clamp(1, MAX_CYCLE_LIMIT),
            offset: filter.offset.unwrap_or(0).max(0),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page))
}

async fn get_cycle(
    State(state): State<EvaluationRestState>,
    headers: HeaderMap,
    Path(cycle_id): Path<Uuid>,
) -> Result<Json<CycleDetail>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_feature(&principal, Feature::EvaluationRead)?;
    let detail = state
        .store
        .get_cycle(cycle_id)
        .await
        .map_err(RestError::from_store)?
        .ok_or_else(not_found_cycle)?;
    Ok(Json(detail))
}

async fn preflight(
    State(state): State<EvaluationRestState>,
    headers: HeaderMap,
    Path(cycle_id): Path<Uuid>,
) -> Result<Json<PreflightReport>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_feature(&principal, Feature::EvaluationRead)?;
    let report = state
        .store
        .preflight(cycle_id)
        .await
        .map_err(RestError::from_store)?
        .ok_or_else(not_found_cycle)?;
    Ok(Json(report))
}

async fn open_cycle(
    state: State<EvaluationRestState>,
    headers: HeaderMap,
    path: Path<Uuid>,
) -> Result<Json<CycleDetail>, RestError> {
    transition(state, headers, path, CycleTransition::Open).await
}

async fn start_calibration(
    state: State<EvaluationRestState>,
    headers: HeaderMap,
    path: Path<Uuid>,
) -> Result<Json<CycleDetail>, RestError> {
    transition(state, headers, path, CycleTransition::StartCalibration).await
}

async fn finalize_cycle(
    state: State<EvaluationRestState>,
    headers: HeaderMap,
    path: Path<Uuid>,
) -> Result<Json<CycleDetail>, RestError> {
    transition(state, headers, path, CycleTransition::Finalize).await
}

async fn archive_cycle(
    state: State<EvaluationRestState>,
    headers: HeaderMap,
    path: Path<Uuid>,
) -> Result<Json<CycleDetail>, RestError> {
    transition(state, headers, path, CycleTransition::Archive).await
}

async fn transition(
    State(state): State<EvaluationRestState>,
    headers: HeaderMap,
    Path(cycle_id): Path<Uuid>,
    transition: CycleTransition,
) -> Result<Json<CycleDetail>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_feature(&principal, Feature::EvaluationManage)?;
    let detail = state
        .store
        .transition_cycle(principal.user_id, cycle_id, transition)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(detail))
}

// ---------------------------------------------------------------------------
// Subject handlers
// ---------------------------------------------------------------------------

async fn add_subject(
    State(state): State<EvaluationRestState>,
    headers: HeaderMap,
    Json(body): Json<AddSubjectRequest>,
) -> Result<(StatusCode, Json<SubjectDetail>), RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_feature(&principal, Feature::EvaluationManage)?;
    let detail = state
        .store
        .add_subject(
            principal.user_id,
            body.cycle_id,
            body.employee_id,
            UserId::from_uuid(body.manager_user_id),
        )
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(detail)))
}

async fn get_subject(
    State(state): State<EvaluationRestState>,
    headers: HeaderMap,
    Path(subject_id): Path<Uuid>,
) -> Result<Json<SubjectDetail>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let can_read = holds_feature(&principal, Feature::EvaluationRead);
    let can_submit = holds_feature(&principal, Feature::EvaluationSubmit);
    if !can_read && !can_submit {
        return Err(RestError::from_kernel(KernelError::forbidden(
            "evaluation access requires a granted capability",
        )));
    }
    let detail = if can_read {
        state
            .store
            .get_subject(subject_id)
            .await
            .map_err(RestError::from_store)?
            .ok_or_else(not_found_subject)?
    } else {
        // The adapter locks the canonical users.employee_id row, evaluates the
        // SELF/MANAGER relationship, and loads this body in one transaction.
        state
            .store
            .get_subject_for_review_actor(principal.user_id, subject_id)
            .await
            .map_err(RestError::from_store)?
            .ok_or_else(not_found_subject)?
    };
    Ok(Json(detail))
}

async fn replace_goals(
    State(state): State<EvaluationRestState>,
    headers: HeaderMap,
    Path(subject_id): Path<Uuid>,
    Json(body): Json<ReplaceGoalsRequest>,
) -> Result<Json<SubjectDetail>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let can_manage = holds_feature(&principal, Feature::EvaluationManage);
    let can_submit = holds_feature(&principal, Feature::EvaluationSubmit);
    if !can_manage && !can_submit {
        return Err(RestError::from_kernel(KernelError::forbidden(
            "goal management requires a granted capability",
        )));
    }
    if !can_manage {
        let gate = subject_gate(&state, subject_id).await?;
        if gate.manager_user_id != principal.user_id {
            return Err(not_found_subject());
        }
    }
    if body.goals.len() > GOALS_MAX {
        return Err(validation(format!(
            "goals must contain at most {GOALS_MAX} entries"
        )));
    }
    let mut goals = Vec::with_capacity(body.goals.len());
    for goal in &body.goals {
        goals.push(GoalInput {
            title: bounded_required(&goal.title, 1, GOAL_TEXT_MAX_CHARS, "title")?,
            metric_kind: goal.metric_kind,
            target_label: bounded_required(
                &goal.target_label,
                1,
                GOAL_TEXT_MAX_CHARS,
                "target_label",
            )?,
            weight_pct: validated_weight(goal.weight_pct)?,
        });
    }
    let detail = state
        .store
        .replace_goals(principal.user_id, subject_id, goals)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(detail))
}

// ---------------------------------------------------------------------------
// Review handlers
// ---------------------------------------------------------------------------

async fn save_review(
    State(state): State<EvaluationRestState>,
    headers: HeaderMap,
    Path((subject_id, kind)): Path<(Uuid, String)>,
    Json(body): Json<UpsertReviewRequest>,
) -> Result<Json<ReviewView>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let kind = ReviewKind::from_path(&kind).map_err(RestError::from_kernel)?;
    require_review_access(&state, &principal, subject_id).await?;
    if body.evidence_links.len() > EVIDENCE_MAX {
        return Err(validation(format!(
            "evidence_links must contain at most {EVIDENCE_MAX} entries"
        )));
    }
    let note = match body.note {
        Some(note) => Some(bounded_optional(&note, NOTE_MAX_CHARS, "note")?),
        None => None,
    }
    .flatten();
    let mut evidence_links = Vec::with_capacity(body.evidence_links.len());
    for link in &body.evidence_links {
        evidence_links.push(EvidenceInput {
            object_kind: link.object_kind,
            object_ref: bounded_required(&link.object_ref, 1, OBJECT_REF_MAX_CHARS, "object_ref")?,
            label: bounded_required(&link.label, 1, EVIDENCE_LABEL_MAX_CHARS, "label")?,
        });
    }
    let view = state
        .store
        .save_review(
            principal.user_id,
            subject_id,
            kind,
            ReviewDraftInput {
                grade: body.grade,
                note,
                evidence_links,
            },
        )
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(view))
}

async fn submit_review(
    State(state): State<EvaluationRestState>,
    headers: HeaderMap,
    Path((subject_id, kind)): Path<(Uuid, String)>,
) -> Result<Json<ReviewView>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let kind = ReviewKind::from_path(&kind).map_err(RestError::from_kernel)?;
    require_review_access(&state, &principal, subject_id).await?;
    let view = state
        .store
        .submit_review(principal.user_id, subject_id, kind)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(view))
}

async fn calibrate(
    State(state): State<EvaluationRestState>,
    headers: HeaderMap,
    Path(subject_id): Path<Uuid>,
    Json(body): Json<CalibrateRequest>,
) -> Result<Json<SubjectDetail>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_feature(&principal, Feature::EvaluationManage)?;
    let reason = match body.reason {
        Some(reason) => Some(bounded_optional(
            &reason,
            CALIBRATION_REASON_MAX_CHARS,
            "reason",
        )?),
        None => None,
    }
    .flatten();
    let detail = state
        .store
        .calibrate(principal.user_id, subject_id, body.final_grade, reason)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(detail))
}

// ---------------------------------------------------------------------------
// Task list and person ledger
// ---------------------------------------------------------------------------

async fn my_tasks(
    State(state): State<EvaluationRestState>,
    headers: HeaderMap,
) -> Result<Json<TaskPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_feature(&principal, Feature::EvaluationSubmit)?;
    let page = state
        .store
        .my_tasks(principal.user_id)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page))
}

async fn employee_reviews(
    State(state): State<EvaluationRestState>,
    headers: HeaderMap,
    Path(employee_id): Path<Uuid>,
) -> Result<Json<LedgerPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_feature(&principal, Feature::EvaluationRead)?;
    let page = state
        .store
        .employee_ledger(principal.user_id, employee_id)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page))
}

// ---------------------------------------------------------------------------
// Authorization helpers
// ---------------------------------------------------------------------------

/// Authorize an org-level evaluation feature. `evaluation_cycles`,
/// `evaluation_subjects`, `evaluation_goals`, `evaluation_reviews` and
/// `evaluation_evidence_links` (migration 0190) all lack a `branch_id` column, so
/// there is no resource branch to check — a capability decision.
fn require_feature(principal: &Principal, feature: Feature) -> Result<(), RestError> {
    authorize_capability(principal, Action::new(feature)).map_err(RestError::from_kernel)
}

fn holds_feature(principal: &Principal, feature: Feature) -> bool {
    require_feature(principal, feature).is_ok()
}

/// Review commands require the server-issued submit capability. The adapter
/// then resolves the actor's canonical employee identity and enforces the
/// kind-specific relationship in the same transaction as the write.
async fn require_review_access(
    _state: &EvaluationRestState,
    principal: &Principal,
    _subject_id: Uuid,
) -> Result<(), RestError> {
    require_feature(principal, Feature::EvaluationSubmit)
}

async fn subject_gate(
    state: &EvaluationRestState,
    subject_id: Uuid,
) -> Result<SubjectGate, RestError> {
    state
        .store
        .subject_gate(subject_id)
        .await
        .map_err(RestError::from_store)?
        .ok_or_else(not_found_subject)
}

async fn principal_from_headers(
    state: &EvaluationRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            "JWT verification is not configured for the evaluation API",
        )
    })?;
    console_platform_request_context::resolve_principal(verifier, state.store.pool(), headers)
        .await
        .map_err(|error| match error {
            console_platform_request_context::RequestContextError::MissingBearer => {
                RestError::unauthorized("missing or malformed bearer token")
            }
            console_platform_request_context::RequestContextError::InvalidToken => {
                RestError::unauthorized("invalid bearer token")
            }
            console_platform_request_context::RequestContextError::InvalidClaim(message) => {
                RestError::unauthorized(format!("token claim is invalid: {message}"))
            }
            console_platform_request_context::RequestContextError::WrongTokenTier => {
                RestError::from_kernel(KernelError::forbidden(
                    "token tier is not valid for this route",
                ))
            }
            console_platform_request_context::RequestContextError::AccessScope(error) => {
                RestError::from_kernel(error)
            }
            console_platform_request_context::RequestContextError::VerifierUnavailable => {
                RestError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "service_unavailable",
                    "JWT verification is not configured for the evaluation API",
                )
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
        })
}

// ---------------------------------------------------------------------------
// Validation helpers (422 before any DB CHECK)
// ---------------------------------------------------------------------------

fn bounded_required(value: &str, min: usize, max: usize, field: &str) -> Result<String, RestError> {
    let trimmed = value.trim();
    let chars = trimmed.chars().count();
    if chars < min || chars > max {
        return Err(validation(format!(
            "{field} must be between {min} and {max} characters"
        )));
    }
    Ok(trimmed.to_owned())
}

/// Bound an optional text field; a present-but-blank value normalizes to None.
fn bounded_optional(value: &str, max: usize, field: &str) -> Result<Option<String>, RestError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > max {
        return Err(validation(format!(
            "{field} must be at most {max} characters"
        )));
    }
    Ok(Some(trimmed.to_owned()))
}

fn validated_weight(weight_pct: i16) -> Result<i16, RestError> {
    if (0..=100).contains(&weight_pct) {
        Ok(weight_pct)
    } else {
        Err(validation("weight_pct must be between 0 and 100"))
    }
}

fn validation(message: impl Into<String>) -> RestError {
    RestError::from_kernel(KernelError::validation(message))
}

fn not_found_cycle() -> RestError {
    RestError::from_kernel(KernelError::not_found("evaluation cycle was not found"))
}

fn not_found_subject() -> RestError {
    RestError::from_kernel(KernelError::not_found("evaluation subject was not found"))
}

// ---------------------------------------------------------------------------
// Error mapping (canonical envelope)
// ---------------------------------------------------------------------------

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
            ErrorKind::Internal => {
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", error.message)
            }
        }
    }

    fn from_store(error: PgEvaluationError) -> Self {
        match error {
            // Domain errors carry safe, caller-facing messages.
            PgEvaluationError::Domain(kernel) => Self::from_kernel(kernel),
            // DB errors must never leak raw sqlx strings or constraint names.
            PgEvaluationError::Db(_) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "evaluation request failed",
            ),
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
