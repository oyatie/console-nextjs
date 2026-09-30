//! Authenticated recruiting-pipeline routes (screen key `recruit`).
//!
//! Recruiting is org-wide HR-owned data: every gate goes through
//! `authorize_org_wide` (the tables carry no branch column to narrow by) and
//! org-scoped lookups return 404 for other-org ids — no existence leakage.
//! The hire handshake route is intentionally NOT here: it lives in the app
//! crate so the HR-owned `create_employee_core` and the recruiting linkage
//! share one transaction.
mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use console_kernel_core::{ErrorKind, KernelError};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize_org_wide};
use console_platform_request_context::RequestContextError;
use console_recruiting_adapter_postgres::{PgRecruitingError, PgRecruitingStore};
use console_recruiting_application::{
    ApplicantIntake, OfferTerms, PostingDraft, Rejection, canonical_amount, parse_assessment,
    parse_date, required_text,
};
use console_recruiting_domain::{OfferStatus, PostingScope, PostingStatus};
use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

pub const RECRUITING_ROUTE_PATHS: &[&str] = &[
    "/api/v1/recruiting/postings",
    "/api/v1/recruiting/postings/{posting_id}",
    "/api/v1/recruiting/postings/{posting_id}/preflight",
    "/api/v1/recruiting/postings/{posting_id}/publish",
    "/api/v1/recruiting/postings/{posting_id}/close",
    "/api/v1/recruiting/postings/{posting_id}/applicants",
    "/api/v1/recruiting/applicants/{applicant_id}",
    "/api/v1/recruiting/applicants/{applicant_id}/advance",
    "/api/v1/recruiting/applicants/{applicant_id}/assess",
    "/api/v1/recruiting/applicants/{applicant_id}/hold",
    "/api/v1/recruiting/applicants/{applicant_id}/request-documents",
    "/api/v1/recruiting/applicants/{applicant_id}/reject",
    "/api/v1/recruiting/applicants/{applicant_id}/reinstate",
    "/api/v1/recruiting/applicants/{applicant_id}/offer",
    "/api/v1/recruiting/offers/{offer_id}/adjust",
    "/api/v1/recruiting/offers/{offer_id}/withdraw",
    "/api/v1/recruiting/offers/{offer_id}/record-reply",
    "/api/v1/recruiting/talent-pool",
];

#[derive(Clone)]
pub struct RecruitingRestState {
    store: PgRecruitingStore,
    jwt: Option<JwtVerifier>,
}

impl RecruitingRestState {
    #[must_use]
    pub fn new(store: PgRecruitingStore, jwt: Option<JwtVerifier>) -> Self {
        Self { store, jwt }
    }
}

pub fn router(state: RecruitingRestState) -> Router {
    let verifier = state.jwt.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(
            "/api/v1/recruiting/postings",
            get(list_postings).post(create_posting),
        )
        .route(
            "/api/v1/recruiting/postings/{posting_id}",
            get(get_posting).put(update_posting),
        )
        .route(
            "/api/v1/recruiting/postings/{posting_id}/preflight",
            post(preflight_posting),
        )
        .route(
            "/api/v1/recruiting/postings/{posting_id}/publish",
            post(publish_posting),
        )
        .route(
            "/api/v1/recruiting/postings/{posting_id}/close",
            post(close_posting),
        )
        .route(
            "/api/v1/recruiting/postings/{posting_id}/applicants",
            post(create_applicant),
        )
        .route(
            "/api/v1/recruiting/applicants/{applicant_id}",
            get(get_applicant),
        )
        .route(
            "/api/v1/recruiting/applicants/{applicant_id}/advance",
            post(advance_applicant),
        )
        .route(
            "/api/v1/recruiting/applicants/{applicant_id}/assess",
            post(assess_applicant),
        )
        .route(
            "/api/v1/recruiting/applicants/{applicant_id}/hold",
            post(hold_applicant),
        )
        .route(
            "/api/v1/recruiting/applicants/{applicant_id}/request-documents",
            post(request_documents),
        )
        .route(
            "/api/v1/recruiting/applicants/{applicant_id}/reject",
            post(reject_applicant),
        )
        .route(
            "/api/v1/recruiting/applicants/{applicant_id}/reinstate",
            post(reinstate_applicant),
        )
        .route(
            "/api/v1/recruiting/applicants/{applicant_id}/offer",
            post(extend_offer),
        )
        .route(
            "/api/v1/recruiting/offers/{offer_id}/adjust",
            post(adjust_offer),
        )
        .route(
            "/api/v1/recruiting/offers/{offer_id}/withdraw",
            post(withdraw_offer),
        )
        .route(
            "/api/v1/recruiting/offers/{offer_id}/record-reply",
            post(record_offer_reply),
        )
        .route("/api/v1/recruiting/talent-pool", get(talent_pool))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListPostingsQuery {
    status: Option<String>,
    scope: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRecruitPostingRequest {
    role_title: String,
    company: String,
    worksite: String,
    employment_type: String,
    scope: String,
    headcount: i32,
    deadline: Option<String>,
    #[serde(default)]
    requirements: Vec<String>,
    position_ref: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateRecruitPostingRequest {
    role_title: String,
    company: String,
    worksite: String,
    employment_type: String,
    scope: String,
    headcount: i32,
    deadline: Option<String>,
    #[serde(default)]
    requirements: Vec<String>,
    position_ref: Option<String>,
    expected_updated_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishRecruitPostingRequest {
    attest_exposure_scope: bool,
    expected_updated_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CloseRecruitPostingRequest {
    expected_updated_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRecruitApplicantRequest {
    name: String,
    #[serde(default)]
    profile_lines: Vec<String>,
    source_document: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdvanceRecruitApplicantRequest {
    expected_updated_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssessRecruitApplicantRequest {
    score: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HoldRecruitApplicantRequest {
    hold: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RejectRecruitApplicantRequest {
    reason: String,
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtendRecruitOfferRequest {
    amount: String,
    amount_period: String,
    reply_deadline: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdjustRecruitOfferRequest {
    amount: String,
    reply_deadline: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WithdrawRecruitOfferRequest {
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordOfferReplyRequest {
    decision: String,
}

async fn create_posting(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateRecruitPostingRequest>,
) -> Result<(StatusCode, Json<Value>), RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    let draft = posting_draft(
        body.role_title,
        body.company,
        body.worksite,
        &body.employment_type,
        &body.scope,
        body.headcount,
        body.deadline.as_deref(),
        body.requirements,
        body.position_ref,
    )?;
    let posting = state
        .store
        .create_posting(principal.user_id, draft)
        .await
        .map_err(RestError::store)?;
    Ok((StatusCode::CREATED, Json(posting)))
}

async fn list_postings(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Query(query): Query<ListPostingsQuery>,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_read(&principal)?;
    let status = query
        .status
        .as_deref()
        .map(PostingStatus::from_input)
        .transpose()
        .map_err(RestError::kernel)?;
    let scope = query
        .scope
        .as_deref()
        .map(PostingScope::from_input)
        .transpose()
        .map_err(RestError::kernel)?;
    Ok(Json(
        state
            .store
            .list_postings(status, scope)
            .await
            .map_err(RestError::store)?,
    ))
}

async fn get_posting(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(posting_id): Path<Uuid>,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_read(&principal)?;
    Ok(Json(
        state
            .store
            .get_posting(posting_id)
            .await
            .map_err(RestError::store)?,
    ))
}

async fn update_posting(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(posting_id): Path<Uuid>,
    Json(body): Json<UpdateRecruitPostingRequest>,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    let expected = parse_rfc3339(&body.expected_updated_at)?;
    let draft = posting_draft(
        body.role_title,
        body.company,
        body.worksite,
        &body.employment_type,
        &body.scope,
        body.headcount,
        body.deadline.as_deref(),
        body.requirements,
        body.position_ref,
    )?;
    Ok(Json(
        state
            .store
            .update_posting(principal.user_id, posting_id, draft, expected)
            .await
            .map_err(RestError::store)?,
    ))
}

async fn preflight_posting(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(posting_id): Path<Uuid>,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    Ok(Json(
        state
            .store
            .preflight(posting_id)
            .await
            .map_err(RestError::store)?,
    ))
}

async fn publish_posting(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(posting_id): Path<Uuid>,
    Json(body): Json<PublishRecruitPostingRequest>,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    let expected = parse_rfc3339(&body.expected_updated_at)?;
    Ok(Json(
        state
            .store
            .publish_posting(
                principal.user_id,
                posting_id,
                body.attest_exposure_scope,
                expected,
            )
            .await
            .map_err(RestError::store)?,
    ))
}

async fn close_posting(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(posting_id): Path<Uuid>,
    Json(body): Json<CloseRecruitPostingRequest>,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    let expected = parse_rfc3339(&body.expected_updated_at)?;
    Ok(Json(
        state
            .store
            .close_posting(principal.user_id, posting_id, expected)
            .await
            .map_err(RestError::store)?,
    ))
}

async fn create_applicant(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(posting_id): Path<Uuid>,
    Json(body): Json<CreateRecruitApplicantRequest>,
) -> Result<(StatusCode, Json<Value>), RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    let intake = ApplicantIntake::new(body.name, body.profile_lines, body.source_document)
        .map_err(RestError::kernel)?;
    let applicant = state
        .store
        .create_applicant(principal.user_id, posting_id, intake)
        .await
        .map_err(RestError::store)?;
    Ok((StatusCode::CREATED, Json(applicant)))
}

async fn get_applicant(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(applicant_id): Path<Uuid>,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_read(&principal)?;
    Ok(Json(
        state
            .store
            .applicant_detail(principal.user_id, applicant_id)
            .await
            .map_err(RestError::store)?,
    ))
}

async fn advance_applicant(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(applicant_id): Path<Uuid>,
    Json(body): Json<AdvanceRecruitApplicantRequest>,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    let expected = parse_rfc3339(&body.expected_updated_at)?;
    Ok(Json(
        state
            .store
            .advance_applicant(principal.user_id, applicant_id, expected)
            .await
            .map_err(RestError::store)?,
    ))
}

async fn assess_applicant(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(applicant_id): Path<Uuid>,
    Json(body): Json<AssessRecruitApplicantRequest>,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    let score = parse_assessment(&body.score).map_err(RestError::kernel)?;
    Ok(Json(
        state
            .store
            .assess_applicant(principal.user_id, applicant_id, score)
            .await
            .map_err(RestError::store)?,
    ))
}

async fn hold_applicant(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(applicant_id): Path<Uuid>,
    Json(body): Json<HoldRecruitApplicantRequest>,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    Ok(Json(
        state
            .store
            .set_hold(principal.user_id, applicant_id, body.hold)
            .await
            .map_err(RestError::store)?,
    ))
}

async fn request_documents(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(applicant_id): Path<Uuid>,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    Ok(Json(
        state
            .store
            .request_documents(principal.user_id, applicant_id)
            .await
            .map_err(RestError::store)?,
    ))
}

async fn reject_applicant(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(applicant_id): Path<Uuid>,
    Json(body): Json<RejectRecruitApplicantRequest>,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    let rejection = Rejection::new(&body.reason, body.note).map_err(RestError::kernel)?;
    Ok(Json(
        state
            .store
            .reject_applicant(principal.user_id, applicant_id, rejection)
            .await
            .map_err(RestError::store)?,
    ))
}

async fn reinstate_applicant(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(applicant_id): Path<Uuid>,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    Ok(Json(
        state
            .store
            .reinstate_applicant(principal.user_id, applicant_id)
            .await
            .map_err(RestError::store)?,
    ))
}

async fn extend_offer(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(applicant_id): Path<Uuid>,
    Json(body): Json<ExtendRecruitOfferRequest>,
) -> Result<(StatusCode, Json<Value>), RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    let terms = OfferTerms::new(&body.amount, &body.amount_period, &body.reply_deadline)
        .map_err(RestError::kernel)?;
    let offer = state
        .store
        .extend_offer(principal.user_id, applicant_id, terms)
        .await
        .map_err(RestError::store)?;
    Ok((StatusCode::CREATED, Json(offer)))
}

async fn adjust_offer(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(offer_id): Path<Uuid>,
    Json(body): Json<AdjustRecruitOfferRequest>,
) -> Result<(StatusCode, Json<Value>), RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    let amount = canonical_amount(&body.amount).map_err(RestError::kernel)?;
    let reply_deadline = body
        .reply_deadline
        .as_deref()
        .map(parse_date)
        .transpose()
        .map_err(RestError::kernel)?;
    let offer = state
        .store
        .adjust_offer(principal.user_id, offer_id, amount, reply_deadline)
        .await
        .map_err(RestError::store)?;
    Ok((StatusCode::CREATED, Json(offer)))
}

async fn withdraw_offer(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(offer_id): Path<Uuid>,
    Json(body): Json<WithdrawRecruitOfferRequest>,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    let reason = required_text(body.reason, "reason", 300).map_err(RestError::kernel)?;
    Ok(Json(
        state
            .store
            .withdraw_offer(principal.user_id, offer_id, reason)
            .await
            .map_err(RestError::store)?,
    ))
}

async fn record_offer_reply(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
    Path(offer_id): Path<Uuid>,
    Json(body): Json<RecordOfferReplyRequest>,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_manage(&principal)?;
    let decision = match body.decision.as_str() {
        "ACCEPTED" => OfferStatus::Accepted,
        "DECLINED" => OfferStatus::Declined,
        _ => {
            return Err(RestError::kernel(KernelError::validation(
                "decision must be ACCEPTED or DECLINED",
            )));
        }
    };
    Ok(Json(
        state
            .store
            .record_offer_reply(principal.user_id, offer_id, decision)
            .await
            .map_err(RestError::store)?,
    ))
}

async fn talent_pool(
    State(state): State<RecruitingRestState>,
    headers: HeaderMap,
) -> Result<Json<Value>, RestError> {
    let principal = principal(&state, &headers).await?;
    allow_read(&principal)?;
    Ok(Json(
        state.store.talent_pool().await.map_err(RestError::store)?,
    ))
}

#[allow(clippy::too_many_arguments)]
fn posting_draft(
    role_title: String,
    company: String,
    worksite: String,
    employment_type: &str,
    scope: &str,
    headcount: i32,
    deadline: Option<&str>,
    requirements: Vec<String>,
    position_ref: Option<String>,
) -> Result<PostingDraft, RestError> {
    PostingDraft::new(
        role_title,
        company,
        worksite,
        employment_type,
        scope,
        headcount,
        deadline,
        requirements,
        position_ref,
    )
    .map_err(RestError::kernel)
}

fn parse_rfc3339(value: &str) -> Result<OffsetDateTime, RestError> {
    OffsetDateTime::parse(value.trim(), &Rfc3339).map_err(|_| {
        RestError::kernel(KernelError::validation(
            "expected_updated_at must be an RFC 3339 timestamp",
        ))
    })
}

async fn principal(
    state: &RecruitingRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt.as_ref().ok_or_else(|| {
        RestError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "JWT verification is not configured",
        )
    })?;
    console_platform_request_context::resolve_principal(verifier, state.store.pool(), headers)
        .await
        .map_err(|error| match error {
            RequestContextError::MissingBearer
            | RequestContextError::InvalidToken
            | RequestContextError::InvalidClaim(_) => RestError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing, malformed, or invalid bearer token",
            ),
            RequestContextError::WrongTokenTier | RequestContextError::AccessScope(_) => {
                RestError::kernel(KernelError::forbidden(
                    "token is not authorized for recruiting",
                ))
            }
            RequestContextError::VerifierUnavailable => RestError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "JWT verification is not configured",
            ),
            RequestContextError::BranchScope(message)
            | RequestContextError::EffectivePolicy(message) => {
                RestError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
            }
            RequestContextError::MissingOrg => RestError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "no tenant context is bound",
            ),
        })
}

/// Read access: `recruiting_read` or the wider `recruiting_manage` grant.
fn allow_read(principal: &Principal) -> Result<(), RestError> {
    authorize_org_wide(principal, Action::new(Feature::RecruitingRead))
        .or_else(|_| authorize_org_wide(principal, Action::new(Feature::RecruitingManage)))
        .map_err(RestError::kernel)
}

fn allow_manage(principal: &Principal) -> Result<(), RestError> {
    authorize_org_wide(principal, Action::new(Feature::RecruitingManage)).map_err(RestError::kernel)
}

struct RestError {
    status: StatusCode,
    body: Value,
}

impl RestError {
    fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            body: json!({ "error": { "code": code, "message": message.into() } }),
        }
    }

    fn kernel(error: KernelError) -> Self {
        let (status, code) = match error.kind {
            ErrorKind::Validation => (StatusCode::UNPROCESSABLE_ENTITY, "validation"),
            ErrorKind::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            ErrorKind::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                (StatusCode::CONFLICT, "conflict")
            }
            ErrorKind::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        Self::new(status, code, error.message)
    }

    fn store(error: PgRecruitingError) -> Self {
        match error {
            PgRecruitingError::Domain(kernel) => Self::kernel(kernel),
            PgRecruitingError::Db(_) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal server error",
            ),
            PgRecruitingError::PreflightFailed(checks) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                body: json!({
                    "error": {
                        "code": "PREFLIGHT_FAILED",
                        "message": "publish preflight failed"
                    },
                    "checks": checks,
                    "publishable": false,
                }),
            },
            PgRecruitingError::AssessmentRequired => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "ASSESSMENT_REQUIRED",
                "assessment must be recorded before an offer",
            ),
            PgRecruitingError::AlreadyHired { employee_id } => Self {
                status: StatusCode::CONFLICT,
                body: json!({
                    "error": {
                        "code": "conflict",
                        "message": "applicant is already hired"
                    },
                    "employee_id": employee_id,
                }),
            },
        }
    }
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}
