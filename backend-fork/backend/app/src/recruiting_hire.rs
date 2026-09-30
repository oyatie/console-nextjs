//! The recruiting hire handshake — the one recruiting route that lives in the
//! app crate, because acceptance → employee creation must go through the
//! HR-owned [`crate::hr::create_employee_core`] in the SAME transaction that
//! links the applicant and increments the posting fill count. Recruiting code
//! never writes `employees`/`employee_employment_profiles` directly and never
//! emits a second `employee.create` audit.
use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use console_kernel_core::{ErrorKind, KernelError};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize_org_wide};
use console_platform_db::{DbError, with_audits};
use console_recruiting_adapter_postgres::{
    PgRecruitingError, apply_hire, audit, hire_context, load_applicant, load_posting,
};
use console_recruiting_domain::AmountPeriod;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::hr;

#[derive(Clone)]
pub(crate) struct RecruitingHireState {
    pool: PgPool,
    jwt: Option<JwtVerifier>,
    hr: hr::HrState,
}

impl RecruitingHireState {
    pub(crate) fn new(pool: PgPool, jwt: Option<JwtVerifier>, hr: hr::HrState) -> Self {
        Self { pool, jwt, hr }
    }
}

pub const RECRUITING_HIRE_PATH: &str = "/api/v1/recruiting/applicants/{applicant_id}/hire";
pub const RECRUITING_HIRE_ROUTE_PATHS: &[&str] = &[RECRUITING_HIRE_PATH];

pub(crate) fn router(state: RecruitingHireState) -> Router {
    let verifier = state.jwt.clone();
    let pool = state.pool.clone();
    let router = Router::new()
        .route(RECRUITING_HIRE_PATH, post(hire))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HireRecruitApplicantRequest {
    employee_number: String,
    phone: String,
    org_unit: String,
    position: String,
    site: String,
    home_branch_id: Uuid,
    /// Defaults to the accepted offer amount for MONTHLY offers; required
    /// explicitly for DAILY offers (a daily rate is not a monthly base pay).
    base_pay: Option<String>,
}

async fn hire(
    State(state): State<RecruitingHireState>,
    Extension(principal): Extension<Principal>,
    Path(applicant_id): Path<Uuid>,
    Json(body): Json<HireRecruitApplicantRequest>,
) -> Result<(StatusCode, Json<Value>), HireError> {
    // Both gates, deny-by-default: the recruiting authority AND the owning HR
    // directory authority (the hire writes an employee row).
    authorize_org_wide(&principal, Action::new(Feature::RecruitingManage))
        .map_err(HireError::kernel)?;
    authorize_org_wide(&principal, Action::new(Feature::EmployeeDirectoryManage))
        .map_err(HireError::kernel)?;
    // Fail closed BEFORE any write: the home-branch routing authority is
    // command-only (0166) and must be assignable for the hired employee.
    let command_store = hr::require_home_branch_command_store(&state.hr).map_err(HireError::Hr)?;
    let org = principal.org_id;
    let org_uuid = *org.as_uuid();
    let actor = principal.user_id;
    let home_branch_id = body.home_branch_id;
    let now = OffsetDateTime::now_utc();

    let (employee_id, response, replayed) =
        with_audits::<_, _, HireError>(&state.pool, org, |tx| {
            Box::pin(async move {
                let context = hire_context(tx, applicant_id)
                    .await
                    .map_err(HireError::Recruiting)?;
                let Some(hr_employment_type) = context.employment_type.hr_employment_type() else {
                    // 재직 명부 비합산: a pool posting must never create an
                    // employee; the workforce-pool registry does not exist yet.
                    return Err(HireError::PoolRegistrationUnavailable);
                };
                let base_pay = match body.base_pay {
                    Some(base_pay) => base_pay,
                    None if context.accepted_period == AmountPeriod::Monthly => {
                        context.accepted_amount.clone()
                    }
                    None => {
                        return Err(HireError::kernel(KernelError::validation(
                            "base_pay is required when the accepted offer is a daily rate",
                        )));
                    }
                };
                let request = hr::normalize_create_employee_request(hr::CreateEmployeeRequest {
                    employee_number: body.employee_number,
                    name: context.candidate_name.clone(),
                    company: context.company.clone(),
                    employment_type: hr_employment_type.to_owned(),
                    phone: body.phone,
                    org_unit: body.org_unit,
                    position: body.position,
                    site: body.site,
                    home_branch_id: body.home_branch_id,
                    base_pay,
                    idempotency_key: format!("recruit-hire-{applicant_id}"),
                })
                .map_err(HireError::Hr)?;
                let request_hash = hr::sha256_hex(
                    serde_json::to_string(&request)
                        .map_err(|_| {
                            HireError::Hr(hr::HrError::validation(
                                "employee request could not be serialized",
                            ))
                        })?
                        .as_bytes(),
                );
                let candidate_employee_id = Uuid::new_v4();
                let (detail, replayed) = hr::create_employee_core(
                    tx,
                    org_uuid,
                    actor,
                    &request,
                    &request_hash,
                    candidate_employee_id,
                )
                .await
                .map_err(HireError::Hr)?;
                let employee_id = detail.employee_id();
                apply_hire(
                    tx,
                    org,
                    applicant_id,
                    context.posting_id,
                    employee_id,
                    actor,
                    now,
                )
                .await
                .map_err(HireError::Recruiting)?;
                let applicant = load_applicant(tx, applicant_id)
                    .await
                    .map_err(HireError::Recruiting)?;
                let posting = load_posting(tx, context.posting_id)
                    .await
                    .map_err(HireError::Recruiting)?;
                let mut audits = Vec::with_capacity(2);
                if !replayed {
                    audits.push(
                        hr::employee_create_audit(org, actor, &request, employee_id)
                            .map_err(HireError::Hr)?,
                    );
                }
                audits.push(
                    audit(
                        org,
                        actor,
                        "recruiting.applicant.hire",
                        "recruit_applicant",
                        applicant_id,
                        now,
                        Some(json!({
                            "employee_id": employee_id,
                            "posting_id": context.posting_id,
                            "posting_no": context.posting_no,
                            "applicant_no": context.applicant_no,
                        })),
                    )
                    .map_err(HireError::Recruiting)?,
                );
                Ok((
                    (
                        employee_id,
                        json!({
                            "employee_id": employee_id,
                            "applicant": applicant,
                            "posting": posting,
                        }),
                        replayed,
                    ),
                    audits,
                ))
            })
        })
        .await?;
    // Post-commit: establish the first home-branch routing authority through
    // the isolated leave command capability (its own CAS + audit). The
    // employee + recruiting linkage are already durable; a command-channel
    // failure here surfaces explicitly and is recoverable through the People &
    // Workforce home-branch assignment (hire replay reports 409 + employee_id).
    hr::assign_home_branch_if_unset(
        &state.pool,
        &command_store,
        org,
        employee_id,
        home_branch_id,
        actor,
        replayed,
    )
    .await
    .map_err(HireError::Hr)?;
    Ok((StatusCode::CREATED, Json(response)))
}

enum HireError {
    Hr(hr::HrError),
    Recruiting(PgRecruitingError),
    PoolRegistrationUnavailable,
}

impl HireError {
    fn kernel(error: KernelError) -> Self {
        Self::Hr(hr::HrError::from_kernel(error))
    }
}

impl From<DbError> for HireError {
    fn from(value: DbError) -> Self {
        Self::Recruiting(PgRecruitingError::Db(value))
    }
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({ "error": { "code": code, "message": message } })),
    )
        .into_response()
}

impl IntoResponse for HireError {
    fn into_response(self) -> Response {
        match self {
            Self::Hr(error) => error.into_response(),
            Self::PoolRegistrationUnavailable => error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "POOL_REGISTRATION_UNAVAILABLE",
                "pool-daily hires register into the workforce pool, which is not available yet",
            ),
            Self::Recruiting(error) => match error {
                PgRecruitingError::Domain(kernel) => {
                    let (status, code) = match kernel.kind {
                        ErrorKind::Validation => (StatusCode::UNPROCESSABLE_ENTITY, "validation"),
                        ErrorKind::NotFound => (StatusCode::NOT_FOUND, "not_found"),
                        ErrorKind::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
                        ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                            (StatusCode::CONFLICT, "conflict")
                        }
                        ErrorKind::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
                    };
                    error_response(status, code, &kernel.message)
                }
                PgRecruitingError::AlreadyHired { employee_id } => (
                    StatusCode::CONFLICT,
                    Json(json!({
                        "error": {
                            "code": "conflict",
                            "message": "applicant is already hired"
                        },
                        "employee_id": employee_id,
                    })),
                )
                    .into_response(),
                PgRecruitingError::AssessmentRequired => error_response(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "ASSESSMENT_REQUIRED",
                    "assessment must be recorded before an offer",
                ),
                PgRecruitingError::PreflightFailed(_) => error_response(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "PREFLIGHT_FAILED",
                    "publish preflight failed",
                ),
                PgRecruitingError::Db(_) => error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal server error",
                ),
            },
        }
    }
}
