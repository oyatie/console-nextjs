//! Authenticated equipment 3R pilot routes.  Every mutation has a distinct
//! capability grant; there is no inherited registry or work-order permission.
//! Transition routes carry no `branchId`: the branch is read from the locked
//! row and authorized in-transaction.
mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use console_equipment_adapter_postgres::{
    BranchAuthorization, PgEquipment3rError, PgEquipment3rStore,
};
use console_equipment_application::{
    AssessReturn, CompleteDisposition, DecideApproval, DispatchCase, HandoverCase, InspectCase,
    QuoteCase, RegisterUnit,
};
use console_kernel_core::{BranchId, BranchScope, ErrorKind, KernelError};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, authorize, authorize_org_wide};
use console_platform_request_context::RequestContextError;
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

pub const EQUIPMENT_3R_UNITS_PATH: &str = "/api/v1/equipment-3r/units";
pub const EQUIPMENT_3R_UNIT_PATH: &str = "/api/v1/equipment-3r/units/{unit_id}";
pub const EQUIPMENT_3R_UNIT_HISTORY_PATH: &str = "/api/v1/equipment-3r/units/{unit_id}/history";
pub const EQUIPMENT_3R_CASES_PATH: &str = "/api/v1/equipment-3r/rental-cases";
pub const EQUIPMENT_3R_CASE_PATH: &str = "/api/v1/equipment-3r/rental-cases/{case_id}";
pub const EQUIPMENT_3R_CASE_APPROVAL_PATH: &str =
    "/api/v1/equipment-3r/rental-cases/{case_id}/approval";
pub const EQUIPMENT_3R_CASE_DISPATCH_PATH: &str =
    "/api/v1/equipment-3r/rental-cases/{case_id}/dispatch";
pub const EQUIPMENT_3R_CASE_HANDOVER_PATH: &str =
    "/api/v1/equipment-3r/rental-cases/{case_id}/handover";
pub const EQUIPMENT_3R_CASE_INSPECTIONS_PATH: &str =
    "/api/v1/equipment-3r/rental-cases/{case_id}/inspections";
pub const EQUIPMENT_3R_CASE_RETURN_PATH: &str =
    "/api/v1/equipment-3r/rental-cases/{case_id}/return";
pub const EQUIPMENT_3R_CASE_ASSESSMENT_PATH: &str =
    "/api/v1/equipment-3r/rental-cases/{case_id}/assessment";
pub const EQUIPMENT_3R_DISPOSITION_COMPLETION_PATH: &str =
    "/api/v1/equipment-3r/dispositions/{disposition_id}/completion";

pub const EQUIPMENT_3R_ROUTE_PATHS: &[&str] = &[
    EQUIPMENT_3R_UNITS_PATH,
    EQUIPMENT_3R_UNIT_PATH,
    EQUIPMENT_3R_UNIT_HISTORY_PATH,
    EQUIPMENT_3R_CASES_PATH,
    EQUIPMENT_3R_CASE_PATH,
    EQUIPMENT_3R_CASE_APPROVAL_PATH,
    EQUIPMENT_3R_CASE_DISPATCH_PATH,
    EQUIPMENT_3R_CASE_HANDOVER_PATH,
    EQUIPMENT_3R_CASE_INSPECTIONS_PATH,
    EQUIPMENT_3R_CASE_RETURN_PATH,
    EQUIPMENT_3R_CASE_ASSESSMENT_PATH,
    EQUIPMENT_3R_DISPOSITION_COMPLETION_PATH,
];

#[derive(Clone)]
pub struct EquipmentRestState {
    store: PgEquipment3rStore,
    jwt: Option<JwtVerifier>,
}
impl EquipmentRestState {
    #[must_use]
    pub fn new(store: PgEquipment3rStore, jwt: Option<JwtVerifier>) -> Self {
        Self { store, jwt }
    }
}

pub fn router(state: EquipmentRestState) -> Router {
    let verifier = state.jwt.clone();
    let pool = state.store.pool().clone();
    let r = Router::new()
        .route(EQUIPMENT_3R_UNITS_PATH, get(list_units).post(register_unit))
        .route(EQUIPMENT_3R_UNIT_PATH, get(unit_detail))
        .route(EQUIPMENT_3R_UNIT_HISTORY_PATH, get(unit_history))
        .route(EQUIPMENT_3R_CASES_PATH, get(list_cases).post(quote_case))
        .route(EQUIPMENT_3R_CASE_PATH, get(case_detail))
        .route(EQUIPMENT_3R_CASE_APPROVAL_PATH, post(decide_approval))
        .route(EQUIPMENT_3R_CASE_DISPATCH_PATH, post(dispatch_case))
        .route(EQUIPMENT_3R_CASE_HANDOVER_PATH, post(handover_case))
        .route(EQUIPMENT_3R_CASE_INSPECTIONS_PATH, post(inspect_case))
        .route(EQUIPMENT_3R_CASE_RETURN_PATH, post(return_case))
        .route(EQUIPMENT_3R_CASE_ASSESSMENT_PATH, post(assess_return))
        .route(
            EQUIPMENT_3R_DISPOSITION_COMPLETION_PATH,
            post(complete_disposition),
        )
        .with_state(state);
    console_platform_request_context::with_request_context(r, verifier, pool)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisterUnitBody {
    branch_id: Uuid,
    serial_no: String,
    model_name: String,
    capacity_class: String,
    acquisition_cost_minor: i64,
}
async fn register_unit(
    State(s): State<EquipmentRestState>,
    h: HeaderMap,
    Json(b): Json<RegisterUnitBody>,
) -> Result<(StatusCode, Json<Value>), RestError> {
    let p = principal(&s, &h).await?;
    let branch = BranchId::from_uuid(b.branch_id);
    allow(&p, Feature::Equipment3rRegistry, branch)?;
    let view = s
        .store
        .register_unit(
            p.user_id,
            RegisterUnit {
                branch_id: branch,
                serial_no: b.serial_no,
                model_name: b.model_name,
                capacity_class: b.capacity_class,
                acquisition_cost_minor: b.acquisition_cost_minor,
            },
        )
        .await
        .map_err(RestError::store)?;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn list_units(
    State(s): State<EquipmentRestState>,
    h: HeaderMap,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    authorize_org_wide(&p, Action::new(Feature::Equipment3rObserve)).map_err(RestError::kernel)?;
    Ok(Json(s.store.list_units().await.map_err(RestError::store)?))
}

async fn unit_detail(
    State(s): State<EquipmentRestState>,
    h: HeaderMap,
    Path(unit_id): Path<Uuid>,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    let (view, branch) = s
        .store
        .unit_detail(unit_id)
        .await
        .map_err(RestError::store)?;
    allow(&p, Feature::Equipment3rObserve, branch)?;
    Ok(Json(view))
}

async fn unit_history(
    State(s): State<EquipmentRestState>,
    h: HeaderMap,
    Path(unit_id): Path<Uuid>,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    let (view, branch) = s
        .store
        .unit_history(unit_id)
        .await
        .map_err(RestError::store)?;
    allow(&p, Feature::Equipment3rObserve, branch)?;
    Ok(Json(view))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QuoteBody {
    branch_id: Uuid,
    unit_id: Uuid,
    customer_name: String,
    site_reference: String,
    monthly_rate_minor: i64,
    duration_months: i32,
    currency_code: String,
}
async fn quote_case(
    State(s): State<EquipmentRestState>,
    h: HeaderMap,
    Json(b): Json<QuoteBody>,
) -> Result<(StatusCode, Json<Value>), RestError> {
    let p = principal(&s, &h).await?;
    let branch = BranchId::from_uuid(b.branch_id);
    allow(&p, Feature::Equipment3rQuote, branch)?;
    let key = idem_header(&h)?;
    let fingerprint = json!({
        "branchId": b.branch_id,
        "unitId": b.unit_id,
        "customerName": b.customer_name,
        "siteReference": b.site_reference,
        "monthlyRateMinor": b.monthly_rate_minor,
        "durationMonths": b.duration_months,
        "currencyCode": b.currency_code,
    });
    let (replayed, view) = s
        .store
        .quote_case(
            p.user_id,
            QuoteCase {
                branch_id: branch,
                unit_id: b.unit_id,
                customer_name: b.customer_name,
                site_reference: b.site_reference,
                monthly_rate_minor: b.monthly_rate_minor,
                duration_months: b.duration_months,
                currency_code: b.currency_code,
            },
            key,
            &fingerprint,
        )
        .await
        .map_err(RestError::store)?;
    let status = if replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(view)))
}

async fn list_cases(
    State(s): State<EquipmentRestState>,
    h: HeaderMap,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    authorize_org_wide(&p, Action::new(Feature::Equipment3rObserve)).map_err(RestError::kernel)?;
    Ok(Json(s.store.list_cases().await.map_err(RestError::store)?))
}

async fn case_detail(
    State(s): State<EquipmentRestState>,
    h: HeaderMap,
    Path(case_id): Path<Uuid>,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    let (view, branch) = s
        .store
        .case_detail(case_id)
        .await
        .map_err(RestError::store)?;
    allow(&p, Feature::Equipment3rObserve, branch)?;
    Ok(Json(view))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApprovalBody {
    decision: String,
    reason: Option<String>,
}
async fn decide_approval(
    State(s): State<EquipmentRestState>,
    h: HeaderMap,
    Path(case_id): Path<Uuid>,
    Json(b): Json<ApprovalBody>,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    let view = s
        .store
        .decide_approval(
            p.user_id,
            case_id,
            DecideApproval {
                decision: b.decision,
                reason: b.reason,
            },
            branch_authorization(&p, Feature::Equipment3rApprove),
        )
        .await
        .map_err(RestError::store)?;
    Ok(Json(view))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DispatchBody {
    carrier_name: String,
    vehicle_reference: String,
}
async fn dispatch_case(
    State(s): State<EquipmentRestState>,
    h: HeaderMap,
    Path(case_id): Path<Uuid>,
    Json(b): Json<DispatchBody>,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    let view = s
        .store
        .dispatch_case(
            p.user_id,
            case_id,
            DispatchCase {
                carrier_name: b.carrier_name,
                vehicle_reference: b.vehicle_reference,
            },
            branch_authorization(&p, Feature::Equipment3rDispatch),
        )
        .await
        .map_err(RestError::store)?;
    Ok(Json(view))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HandoverBody {
    recipient_name: String,
    evidence_object_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    handed_over_at: OffsetDateTime,
}
async fn handover_case(
    State(s): State<EquipmentRestState>,
    h: HeaderMap,
    Path(case_id): Path<Uuid>,
    Json(b): Json<HandoverBody>,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    let view = s
        .store
        .handover_case(
            p.user_id,
            case_id,
            HandoverCase {
                recipient_name: b.recipient_name,
                evidence_object_id: b.evidence_object_id,
                handed_over_at: b.handed_over_at,
            },
            branch_authorization(&p, Feature::Equipment3rDispatch),
        )
        .await
        .map_err(RestError::store)?;
    Ok(Json(view))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InspectionBody {
    outcome: String,
    findings: String,
    maintenance_note: Option<String>,
}
async fn inspect_case(
    State(s): State<EquipmentRestState>,
    h: HeaderMap,
    Path(case_id): Path<Uuid>,
    Json(b): Json<InspectionBody>,
) -> Result<(StatusCode, Json<Value>), RestError> {
    let p = principal(&s, &h).await?;
    let view = s
        .store
        .inspect_case(
            p.user_id,
            case_id,
            InspectCase {
                outcome: b.outcome,
                findings: b.findings,
                maintenance_note: b.maintenance_note,
            },
            branch_authorization(&p, Feature::Equipment3rInspect),
        )
        .await
        .map_err(RestError::store)?;
    Ok((StatusCode::CREATED, Json(view)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReturnBody {
    #[serde(with = "time::serde::rfc3339")]
    returned_at: OffsetDateTime,
}
async fn return_case(
    State(s): State<EquipmentRestState>,
    h: HeaderMap,
    Path(case_id): Path<Uuid>,
    Json(b): Json<ReturnBody>,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    let view = s
        .store
        .return_case(
            p.user_id,
            case_id,
            b.returned_at,
            branch_authorization(&p, Feature::Equipment3rAssess),
        )
        .await
        .map_err(RestError::store)?;
    Ok(Json(view))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssessmentBody {
    condition_grade: String,
    findings: String,
    disposition: String,
}
async fn assess_return(
    State(s): State<EquipmentRestState>,
    h: HeaderMap,
    Path(case_id): Path<Uuid>,
    Json(b): Json<AssessmentBody>,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    let view = s
        .store
        .assess_return(
            p.user_id,
            case_id,
            AssessReturn {
                condition_grade: b.condition_grade,
                findings: b.findings,
                disposition: b.disposition,
            },
            branch_authorization(&p, Feature::Equipment3rAssess),
        )
        .await
        .map_err(RestError::store)?;
    Ok(Json(view))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompletionBody {
    cost_minor: Option<i64>,
    sale_amount_minor: Option<i64>,
    buyer_name: Option<String>,
}
async fn complete_disposition(
    State(s): State<EquipmentRestState>,
    h: HeaderMap,
    Path(disposition_id): Path<Uuid>,
    Json(b): Json<CompletionBody>,
) -> Result<Json<Value>, RestError> {
    let p = principal(&s, &h).await?;
    let view = s
        .store
        .complete_disposition(
            p.user_id,
            disposition_id,
            CompleteDisposition {
                cost_minor: b.cost_minor,
                sale_amount_minor: b.sale_amount_minor,
                buyer_name: b.buyer_name,
            },
            branch_authorization(&p, Feature::Equipment3rDisposition),
        )
        .await
        .map_err(RestError::store)?;
    Ok(Json(view))
}

async fn principal(s: &EquipmentRestState, h: &HeaderMap) -> Result<Principal, RestError> {
    let verifier = s.jwt.as_ref().ok_or_else(|| {
        RestError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "JWT verification is not configured",
        )
    })?;
    console_platform_request_context::resolve_principal(verifier, s.store.pool(), h)
        .await
        .map_err(|e| match e {
            RequestContextError::MissingBearer
            | RequestContextError::InvalidToken
            | RequestContextError::InvalidClaim(_) => RestError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing, malformed, or invalid bearer token",
            ),
            RequestContextError::WrongTokenTier | RequestContextError::AccessScope(_) => {
                RestError::kernel(KernelError::forbidden(
                    "token is not authorized for equipment operations",
                ))
            }
            RequestContextError::VerifierUnavailable => RestError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "JWT verification is not configured",
            ),
            RequestContextError::BranchScope(m) | RequestContextError::EffectivePolicy(m) => {
                RestError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", m)
            }
            RequestContextError::MissingOrg => RestError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "no tenant context is bound",
            ),
        })
}

fn allow(p: &Principal, f: Feature, b: BranchId) -> Result<(), RestError> {
    let a = Action::new(f);
    match p.branch_scope {
        BranchScope::All => authorize_org_wide(p, a),
        _ => authorize(p, a, b),
    }
    .map_err(RestError::kernel)
}

/// In-transaction branch authorization for transition routes: the adapter
/// calls this with the branch read from the locked row.
fn branch_authorization(p: &Principal, f: Feature) -> BranchAuthorization {
    let p = p.clone();
    Box::new(move |b| {
        let a = Action::new(f);
        match p.branch_scope {
            BranchScope::All => authorize_org_wide(&p, a),
            _ => authorize(&p, a, b),
        }
    })
}

fn idem_header(h: &HeaderMap) -> Result<String, RestError> {
    h.get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| {
            RestError::kernel(KernelError::validation(
                "Idempotency-Key header is required",
            ))
        })
}

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
    fn kernel(e: KernelError) -> Self {
        match e.kind {
            ErrorKind::Validation => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "validation", e.message)
            }
            ErrorKind::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", e.message),
            ErrorKind::Forbidden => Self::new(StatusCode::FORBIDDEN, "forbidden", e.message),
            ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                Self::new(StatusCode::CONFLICT, "conflict", e.message)
            }
            ErrorKind::Internal => {
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.message)
            }
        }
    }
    fn store(e: PgEquipment3rError) -> Self {
        match e {
            PgEquipment3rError::Domain(k) => Self::kernel(k),
            db => match db.kind() {
                ErrorKind::Conflict => Self::new(
                    StatusCode::CONFLICT,
                    "conflict",
                    "conflicting concurrent write or duplicate value",
                ),
                ErrorKind::NotFound => {
                    Self::new(StatusCode::NOT_FOUND, "not_found", "resource was not found")
                }
                _ => Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal server error",
                ),
            },
        }
    }
}
impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error":{"code":self.code,"message":self.message}})),
        )
            .into_response()
    }
}
