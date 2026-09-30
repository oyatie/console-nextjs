//! Financial REST API.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_financial_adapter_postgres::{PgFinancialError, PgFinancialStore};
use console_financial_application::{
    AppendCostLedgerEntryCommand, ConfirmPurchaseAttachmentUploadCommand, CostLedgerSource,
    CreatePurchaseRequestCommand, CreateRentalQuoteCommand, ExecutePurchaseCommand,
    FinancialConfigSnapshot, ListPurchaseRequestsQuery, PrepareExpenditureCommand,
    PreparePurchaseAttachmentUploadCommand, PurchaseApprovalCommand, PurchaseRequestLineInput,
    PurchaseRestartCommand, PurchaseSubmitCommand, PurchaseType, RejectPurchaseCommand,
    financial_audit_event,
};
use console_financial_domain::{
    MoneyInput, PurchaseStatus, RentalQuoteInput, compute_rental_quote,
};
use console_kernel_core::{
    BranchId, EquipmentId, ErrorKind, EvidenceId, KernelError, PurchaseRequestId, QuoteId,
    TraceContext, WorkOrderId,
};
use console_platform_auth::{
    AuthError, JwtVerifier, PasskeyAuthenticationCredential, PasskeyService,
};
use console_platform_authz::{Action, Feature, Principal, authorize};
use console_platform_db::{DbError, with_audit};
use console_platform_storage::{
    PresignGetRequest, PresignPutRequest, PresignedUpload, S3ObjectStore, SeaweedS3Storage,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone)]
pub struct FinancialRestState {
    store: PgFinancialStore,
    jwt_verifier: Option<JwtVerifier>,
    passkey_step_up: Option<PasskeyService>,
    purchase_attachment_storage: Option<(SeaweedS3Storage, String)>,
}

impl FinancialRestState {
    #[must_use]
    pub fn new(store: PgFinancialStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
            passkey_step_up: None,
            purchase_attachment_storage: None,
        }
    }

    #[must_use]
    pub fn with_passkey_step_up(mut self, passkey_step_up: Option<PasskeyService>) -> Self {
        self.passkey_step_up = passkey_step_up;
        self
    }

    #[must_use]
    pub fn with_purchase_attachment_storage(
        mut self,
        storage: Option<(SeaweedS3Storage, String)>,
    ) -> Self {
        self.purchase_attachment_storage = storage;
        self
    }
}

pub const FINANCIAL_RENTAL_QUOTES_COMPUTE_PATH: &str = "/api/v1/financial/rental-quotes/compute";
pub const FINANCIAL_RENTAL_QUOTES_PATH: &str = "/api/v1/financial/rental-quotes";
pub const FINANCIAL_RENTAL_QUOTE_PATH_TEMPLATE: &str = "/api/v1/financial/rental-quotes/{quote_id}";
pub const FINANCIAL_EQUIPMENT_COST_LEDGER_PATH_TEMPLATE: &str =
    "/api/v1/financial/equipment/{equipment_id}/cost-ledger";
pub const FINANCIAL_EQUIPMENT_LIFECYCLE_COST_PATH_TEMPLATE: &str =
    "/api/v1/financial/equipment/{equipment_id}/lifecycle-cost";
pub const FINANCIAL_EQUIPMENT_COST_LEDGER_MANUAL_PATH_TEMPLATE: &str =
    "/api/v1/financial/equipment/{equipment_id}/cost-ledger/manual";
pub const FINANCIAL_PURCHASE_REQUESTS_PATH: &str = "/api/v1/financial/purchase-requests";
pub const FINANCIAL_PURCHASE_REQUEST_PREFERENCES_PATH: &str =
    "/api/v1/financial/purchase-requests/preferences";
pub const FINANCIAL_PURCHASE_ATTACHMENT_PRESIGN_PATH: &str =
    "/api/v1/financial/purchase-requests/attachments/presign";
pub const FINANCIAL_PURCHASE_ATTACHMENT_CONFIRM_PATH_TEMPLATE: &str =
    "/api/v1/financial/purchase-requests/attachments/{attachment_id}/confirm";
pub const FINANCIAL_PURCHASE_REQUEST_PATH_TEMPLATE: &str =
    "/api/v1/financial/purchase-requests/{purchase_request_id}";
pub const FINANCIAL_PURCHASE_ATTACHMENT_DOWNLOAD_PATH_TEMPLATE: &str = "/api/v1/financial/purchase-requests/{purchase_request_id}/attachments/{attachment_id}/download";
pub const FINANCIAL_PURCHASE_REQUEST_SUBMIT_PATH_TEMPLATE: &str =
    "/api/v1/financial/purchase-requests/{purchase_request_id}/submit";
pub const FINANCIAL_PURCHASE_REQUEST_APPROVE_ADMIN_PATH_TEMPLATE: &str =
    "/api/v1/financial/purchase-requests/{purchase_request_id}/approve-admin";
pub const FINANCIAL_PURCHASE_REQUEST_PREPARE_EXPENDITURE_PATH_TEMPLATE: &str =
    "/api/v1/financial/purchase-requests/{purchase_request_id}/prepare-expenditure";
pub const FINANCIAL_PURCHASE_REQUEST_APPROVE_EXECUTIVE_PATH_TEMPLATE: &str =
    "/api/v1/financial/purchase-requests/{purchase_request_id}/approve-executive";
pub const FINANCIAL_PURCHASE_REQUEST_REJECT_PATH_TEMPLATE: &str =
    "/api/v1/financial/purchase-requests/{purchase_request_id}/reject";
pub const FINANCIAL_PURCHASE_REQUEST_RESTART_PATH_TEMPLATE: &str =
    "/api/v1/financial/purchase-requests/{purchase_request_id}/restart";
pub const FINANCIAL_PURCHASE_REQUEST_EXECUTE_PATH_TEMPLATE: &str =
    "/api/v1/financial/purchase-requests/{purchase_request_id}/execute";
pub const FINANCIAL_ROUTE_PATHS: &[&str] = &[
    FINANCIAL_RENTAL_QUOTES_COMPUTE_PATH,
    FINANCIAL_RENTAL_QUOTES_PATH,
    FINANCIAL_RENTAL_QUOTE_PATH_TEMPLATE,
    FINANCIAL_EQUIPMENT_COST_LEDGER_PATH_TEMPLATE,
    FINANCIAL_EQUIPMENT_LIFECYCLE_COST_PATH_TEMPLATE,
    FINANCIAL_EQUIPMENT_COST_LEDGER_MANUAL_PATH_TEMPLATE,
    FINANCIAL_PURCHASE_REQUESTS_PATH,
    FINANCIAL_PURCHASE_REQUEST_PREFERENCES_PATH,
    FINANCIAL_PURCHASE_ATTACHMENT_PRESIGN_PATH,
    FINANCIAL_PURCHASE_ATTACHMENT_CONFIRM_PATH_TEMPLATE,
    FINANCIAL_PURCHASE_REQUEST_PATH_TEMPLATE,
    FINANCIAL_PURCHASE_ATTACHMENT_DOWNLOAD_PATH_TEMPLATE,
    FINANCIAL_PURCHASE_REQUEST_SUBMIT_PATH_TEMPLATE,
    FINANCIAL_PURCHASE_REQUEST_APPROVE_ADMIN_PATH_TEMPLATE,
    FINANCIAL_PURCHASE_REQUEST_PREPARE_EXPENDITURE_PATH_TEMPLATE,
    FINANCIAL_PURCHASE_REQUEST_APPROVE_EXECUTIVE_PATH_TEMPLATE,
    FINANCIAL_PURCHASE_REQUEST_REJECT_PATH_TEMPLATE,
    FINANCIAL_PURCHASE_REQUEST_RESTART_PATH_TEMPLATE,
    FINANCIAL_PURCHASE_REQUEST_EXECUTE_PATH_TEMPLATE,
];

pub fn router(state: FinancialRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(FINANCIAL_RENTAL_QUOTES_COMPUTE_PATH, post(compute_quote))
        .route(FINANCIAL_RENTAL_QUOTES_PATH, post(create_rental_quote))
        .route(FINANCIAL_RENTAL_QUOTE_PATH_TEMPLATE, get(get_rental_quote))
        .route(
            FINANCIAL_EQUIPMENT_COST_LEDGER_PATH_TEMPLATE,
            get(list_cost_ledger),
        )
        .route(
            FINANCIAL_EQUIPMENT_LIFECYCLE_COST_PATH_TEMPLATE,
            get(get_lifecycle_cost),
        )
        .route(
            FINANCIAL_EQUIPMENT_COST_LEDGER_MANUAL_PATH_TEMPLATE,
            post(append_manual_cost_ledger),
        )
        .route(
            FINANCIAL_PURCHASE_REQUESTS_PATH,
            get(list_purchase_requests).post(create_purchase_request),
        )
        .route(
            FINANCIAL_PURCHASE_REQUEST_PREFERENCES_PATH,
            get(get_purchase_preferences).put(save_purchase_preferences),
        )
        .route(
            FINANCIAL_PURCHASE_ATTACHMENT_PRESIGN_PATH,
            post(presign_purchase_attachment),
        )
        .route(
            FINANCIAL_PURCHASE_ATTACHMENT_CONFIRM_PATH_TEMPLATE,
            post(confirm_purchase_attachment),
        )
        .route(
            FINANCIAL_PURCHASE_REQUEST_PATH_TEMPLATE,
            get(get_purchase_request),
        )
        .route(
            FINANCIAL_PURCHASE_ATTACHMENT_DOWNLOAD_PATH_TEMPLATE,
            get(download_purchase_attachment),
        )
        .route(
            FINANCIAL_PURCHASE_REQUEST_SUBMIT_PATH_TEMPLATE,
            post(submit_purchase_request),
        )
        .route(
            FINANCIAL_PURCHASE_REQUEST_APPROVE_ADMIN_PATH_TEMPLATE,
            post(approve_purchase_admin),
        )
        .route(
            FINANCIAL_PURCHASE_REQUEST_PREPARE_EXPENDITURE_PATH_TEMPLATE,
            post(prepare_expenditure),
        )
        .route(
            FINANCIAL_PURCHASE_REQUEST_APPROVE_EXECUTIVE_PATH_TEMPLATE,
            post(approve_purchase_executive),
        )
        .route(
            FINANCIAL_PURCHASE_REQUEST_REJECT_PATH_TEMPLATE,
            post(reject_purchase_request),
        )
        .route(
            FINANCIAL_PURCHASE_REQUEST_RESTART_PATH_TEMPLATE,
            post(restart_purchase_request),
        )
        .route(
            FINANCIAL_PURCHASE_REQUEST_EXECUTE_PATH_TEMPLATE,
            post(execute_purchase),
        )
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
struct ComputeQuoteRequest {
    branch_id: BranchId,
    acquisition_value_won: i64,
    current_residual_value_won: i64,
    cumulative_repair_cost_won: i64,
    config: FinancialConfigSnapshot,
}

#[derive(Debug, Deserialize)]
struct CreateRentalQuoteRequest {
    branch_id: BranchId,
    equipment_id: EquipmentId,
    config: FinancialConfigSnapshot,
}

#[derive(Debug, Deserialize)]
struct AppendManualCostLedgerRequest {
    branch_id: BranchId,
    work_order_id: Option<WorkOrderId>,
    amount_won: i64,
    memo: String,
    config: FinancialConfigSnapshot,
}

#[derive(Debug, Deserialize)]
struct CreatePurchaseRequest {
    branch_id: BranchId,
    equipment_id: Option<EquipmentId>,
    work_order_id: Option<WorkOrderId>,
    statement_evidence_id: Option<EvidenceId>,
    purchase_type: PurchaseType,
    vendor_name: String,
    amount_won: Option<i64>,
    lines: Vec<PurchaseRequestLineInput>,
    quote_attachment_ids: Vec<uuid::Uuid>,
    memo: String,
    config: FinancialConfigSnapshot,
}

struct PurchaseRequestListParams {
    /// Collection reads are deliberately branch-explicit: no unbounded tenant
    /// queue exists at this endpoint.
    branch_id: BranchId,
    statuses: Vec<PurchaseStatus>,
    limit: i64,
    offset: i64,
}

#[derive(Debug, Deserialize)]
struct PurchaseAttachmentPresignRequest {
    branch_id: BranchId,
    file_name: String,
    content_type: String,
    size_bytes: i64,
    checksum_sha256: Option<String>,
    role: Option<String>,
}

#[derive(Debug, Serialize)]
struct PurchaseAttachmentPresignResponse {
    attachment_id: uuid::Uuid,
    upload: PresignedUpload,
    file_name: String,
    content_type: String,
    size_bytes: i64,
    role: String,
    upload_state: String,
}

#[derive(Debug, Serialize)]
struct PurchaseAttachmentDownloadResponse {
    url: String,
}

#[derive(Debug, Deserialize)]
struct SavePurchasePreferencesRequest {
    schema_version: i32,
    preferences: serde_json::Value,
}

#[derive(Debug, Deserialize, Default)]
struct FinancialStepUpRequest {
    #[serde(default)]
    step_up: Option<PasskeyStepUpAssertionRequest>,
}

#[derive(Debug, Deserialize)]
struct PasskeyStepUpAssertionRequest {
    ceremony_id: uuid::Uuid,
    credential: PasskeyAuthenticationCredential,
}

#[derive(Debug, Deserialize, Default)]
struct PrepareExpenditureRequest {
    #[serde(default)]
    expenditure_no: String,
    #[serde(default)]
    step_up: Option<PasskeyStepUpAssertionRequest>,
}

#[derive(Debug, Deserialize, Default)]
struct RejectPurchaseRequest {
    #[serde(default)]
    memo: String,
    #[serde(default)]
    step_up: Option<PasskeyStepUpAssertionRequest>,
}

#[derive(Debug, Deserialize)]
struct RestartPurchaseRequest {
    statement_evidence_id: Option<EvidenceId>,
    amount_won: Option<i64>,
    lines: Vec<PurchaseRequestLineInput>,
    quote_attachment_ids: Vec<uuid::Uuid>,
    memo: String,
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

async fn compute_quote(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Json(body): Json<ComputeQuoteRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize(
        &principal,
        Action::new(Feature::RentalQuoteManage),
        body.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let quote = compute_rental_quote(RentalQuoteInput {
        acquisition_value: MoneyInput::won(body.acquisition_value_won),
        current_residual_value: MoneyInput::won(body.current_residual_value_won),
        cumulative_repair_cost: MoneyInput::won(body.cumulative_repair_cost_won),
        config: body.config.quote_config(),
    })
    .map_err(RestError::from_kernel)?;
    Ok(Json(quote))
}

async fn create_rental_quote(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateRentalQuoteRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize(
        &principal,
        Action::new(Feature::RentalQuoteManage),
        body.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let quote = state
        .store
        .create_rental_quote(CreateRentalQuoteCommand {
            actor: principal.user_id,
            branch_id: body.branch_id,
            equipment_id: body.equipment_id,
            config: body.config,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(quote)))
}

async fn get_rental_quote(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Path(quote_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let quote = state
        .store
        .rental_quote(QuoteId::from_uuid(quote_id))
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::RentalQuoteManage),
        quote.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    Ok(Json(quote))
}

async fn list_cost_ledger(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Path(equipment_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let equipment_id = EquipmentId::from_uuid(equipment_id);
    let principal = principal_from_headers(&state, &headers).await?;
    let branch_id = state
        .store
        .equipment_branch(equipment_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::EquipmentCostLedgerRead),
        branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let entries = state
        .store
        .cost_ledger_for_equipment(equipment_id)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(entries))
}

async fn get_lifecycle_cost(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Path(equipment_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let equipment_id = EquipmentId::from_uuid(equipment_id);
    let principal = principal_from_headers(&state, &headers).await?;
    let branch_id = state
        .store
        .equipment_branch(equipment_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::new(Feature::EquipmentCostLedgerRead),
        branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let summary = state
        .store
        .lifecycle_cost_for_equipment(equipment_id)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

async fn append_manual_cost_ledger(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Path(equipment_id): Path<uuid::Uuid>,
    Json(body): Json<AppendManualCostLedgerRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize(
        &principal,
        Action::new(Feature::EquipmentCostLedgerWrite),
        body.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let entry = state
        .store
        .append_cost_ledger_entry(AppendCostLedgerEntryCommand {
            actor: principal.user_id,
            branch_id: body.branch_id,
            equipment_id: EquipmentId::from_uuid(equipment_id),
            work_order_id: body.work_order_id,
            source: CostLedgerSource::ManualAdmin,
            amount_won: body.amount_won,
            memo: body.memo,
            config: body.config,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(entry)))
}

async fn create_purchase_request(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Json(body): Json<CreatePurchaseRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize(
        &principal,
        Action::request(Feature::PurchaseRequestCreate),
        body.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let purchase = state
        .store
        .create_purchase_request(CreatePurchaseRequestCommand {
            actor: principal.user_id,
            branch_id: body.branch_id,
            equipment_id: body.equipment_id,
            work_order_id: body.work_order_id,
            statement_evidence_id: body.statement_evidence_id,
            purchase_type: body.purchase_type,
            vendor_name: body.vendor_name,
            amount_won: body.amount_won,
            lines: body.lines,
            quote_attachment_ids: body.quote_attachment_ids,
            memo: body.memo,
            config: body.config,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(purchase)))
}

async fn list_purchase_requests(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let params = parse_purchase_request_list_query(raw_query.as_deref())?;
    if !principal.branch_scope.allows(params.branch_id) {
        return Err(RestError::from_kernel(KernelError::forbidden(
            "resource branch is outside principal scope",
        )));
    }

    // The individual GET endpoint permits a requester to read their own
    // request without the broader PurchaseRequestRead grant.  Preserve that
    // contract here without materializing other requesters' branch rows.
    let requester_id = authorize(
        &principal,
        Action::limited(Feature::PurchaseRequestRead),
        params.branch_id,
    )
    .is_err()
    .then_some(principal.user_id);
    let page = state
        .store
        .list_purchase_requests(ListPurchaseRequestsQuery {
            branch_id: params.branch_id,
            statuses: params.statuses,
            requester_id,
            limit: params.limit,
            offset: params.offset,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page))
}

/// Strict collection parser. Axum's `Query<Vec<_>>` does not provide a stable
/// repeated-key contract, so keep this boundary explicit and reject every
/// alternate/unknown spelling rather than silently changing queue semantics.
fn parse_purchase_request_list_query(
    raw_query: Option<&str>,
) -> Result<PurchaseRequestListParams, RestError> {
    let mut branch_id = None;
    let mut statuses = Vec::new();
    let mut limit = None;
    let mut offset = None;
    if let Some(raw_query) = raw_query {
        for pair in raw_query.split('&').filter(|pair| !pair.is_empty()) {
            let (raw_key, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
            let key = decode_purchase_query_component(raw_key)?;
            let value = decode_purchase_query_component(raw_value)?;
            match key.as_str() {
                "branch_id" => {
                    if branch_id.is_some() {
                        return Err(purchase_query_validation("branch_id may appear only once"));
                    }
                    let parsed = uuid::Uuid::parse_str(value.trim())
                        .map_err(|_| purchase_query_validation("branch_id must be a UUID"))?;
                    branch_id = Some(BranchId::from_uuid(parsed));
                }
                "status" => statuses.push(
                    PurchaseStatus::from_db_str(value.trim()).map_err(RestError::from_kernel)?,
                ),
                "limit" => {
                    if limit.is_some() {
                        return Err(purchase_query_validation("limit may appear only once"));
                    }
                    limit = Some(parse_purchase_query_i64("limit", &value)?);
                }
                "offset" => {
                    if offset.is_some() {
                        return Err(purchase_query_validation("offset may appear only once"));
                    }
                    offset = Some(parse_purchase_query_i64("offset", &value)?);
                }
                _ => {
                    return Err(purchase_query_validation(format!(
                        "unsupported purchase-request query parameter {key:?}"
                    )));
                }
            }
        }
    }
    let branch_id = branch_id.ok_or_else(|| purchase_query_validation("branch_id is required"))?;
    let limit = limit.unwrap_or(25);
    if !(1..=100).contains(&limit) {
        return Err(purchase_query_validation("limit must be between 1 and 100"));
    }
    let offset = offset.unwrap_or(0);
    if offset < 0 {
        return Err(purchase_query_validation("offset must not be negative"));
    }
    Ok(PurchaseRequestListParams {
        branch_id,
        statuses,
        limit,
        offset,
    })
}

fn parse_purchase_query_i64(name: &str, value: &str) -> Result<i64, RestError> {
    value
        .trim()
        .parse::<i64>()
        .map_err(|_| purchase_query_validation(format!("{name} must be an integer")))
}

fn purchase_query_validation(message: impl Into<String>) -> RestError {
    RestError::from_kernel(KernelError::validation(message))
}

fn decode_purchase_query_component(input: &str) -> Result<String, RestError> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' => {
                if index + 2 >= bytes.len() {
                    return Err(purchase_query_validation(
                        "query contains an incomplete percent-escape",
                    ));
                }
                let high = purchase_query_hex_value(bytes[index + 1]).ok_or_else(|| {
                    purchase_query_validation("query contains an invalid percent-escape")
                })?;
                let low = purchase_query_hex_value(bytes[index + 2]).ok_or_else(|| {
                    purchase_query_validation("query contains an invalid percent-escape")
                })?;
                decoded.push((high << 4) | low);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded)
        .map_err(|_| purchase_query_validation("query contains invalid UTF-8 data"))
}

const fn purchase_query_hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

async fn get_purchase_preferences(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let preferences = state
        .store
        .purchase_feature_preferences(principal.user_id, "purchase_requests")
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(preferences))
}

async fn save_purchase_preferences(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Json(body): Json<SavePurchasePreferencesRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let preferences = state
        .store
        .save_purchase_feature_preferences(
            principal.user_id,
            "purchase_requests",
            body.schema_version,
            body.preferences,
        )
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(preferences))
}

async fn presign_purchase_attachment(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Json(body): Json<PurchaseAttachmentPresignRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize(
        &principal,
        Action::request(Feature::PurchaseRequestCreate),
        body.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    validate_purchase_attachment_content_type(&body.content_type)
        .map_err(RestError::from_kernel)?;

    let (object_store, bucket) = state
        .purchase_attachment_storage
        .as_ref()
        .ok_or_else(|| RestError::unavailable("purchase attachment storage is not configured"))?;
    let org =
        console_platform_request_context::current_org().map_err(rest_error_from_request_context)?;
    let storage_key = purchase_attachment_s3_key(org, &body.file_name);
    let role = body
        .role
        .unwrap_or_else(|| "QUOTE".to_owned())
        .to_uppercase();
    let upload = object_store
        .presign_put(PresignPutRequest {
            bucket: bucket.clone(),
            key: storage_key.clone(),
            content_type: body.content_type.clone(),
            size_bytes: body.size_bytes,
            checksum_sha256: body.checksum_sha256.clone(),
            expires_in: Duration::from_secs(15 * 60),
        })
        .await
        .map_err(|err| {
            RestError::unavailable(format!("purchase attachment storage unavailable: {err}"))
        })?;

    let record = state
        .store
        .prepare_purchase_attachment_upload(PreparePurchaseAttachmentUploadCommand {
            actor: principal.user_id,
            branch_id: body.branch_id,
            file_name: body.file_name,
            content_type: body.content_type,
            size_bytes: body.size_bytes,
            checksum_sha256: body.checksum_sha256,
            role,
            s3_bucket: bucket.clone(),
            s3_key: storage_key,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;

    Ok((
        StatusCode::CREATED,
        Json(PurchaseAttachmentPresignResponse {
            attachment_id: record.id,
            upload,
            file_name: record.file_name,
            content_type: record.content_type,
            size_bytes: record.size_bytes,
            role: record.role,
            upload_state: record.upload_state,
        }),
    ))
}

async fn confirm_purchase_attachment(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Path(attachment_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let staged = state
        .store
        .purchase_attachment_upload_record(attachment_id)
        .await
        .map_err(RestError::from_store)?;
    authorize(
        &principal,
        Action::request(Feature::PurchaseRequestCreate),
        staged.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let record = state
        .store
        .confirm_purchase_attachment_upload(ConfirmPurchaseAttachmentUploadCommand {
            actor: principal.user_id,
            attachment_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;

    Ok(Json(record))
}

async fn download_purchase_attachment(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Path((purchase_request_id, attachment_id)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<impl IntoResponse, RestError> {
    let purchase_request_id = PurchaseRequestId::from_uuid(purchase_request_id);
    let _ = authorize_for_purchase_read(&state, &headers, purchase_request_id).await?;
    let attachment = state
        .store
        .purchase_attachment_download(purchase_request_id, attachment_id)
        .await
        .map_err(RestError::from_store)?;
    let (object_store, _) = state
        .purchase_attachment_storage
        .as_ref()
        .ok_or_else(|| RestError::unavailable("purchase attachment storage is not configured"))?;
    let url = object_store
        .presign_get(PresignGetRequest {
            bucket: attachment.s3_bucket,
            key: attachment.s3_key,
            expires_in: Duration::from_secs(10 * 60),
        })
        .await
        .map_err(|err| {
            RestError::unavailable(format!("purchase attachment storage unavailable: {err}"))
        })?;
    Ok(Json(PurchaseAttachmentDownloadResponse { url }))
}

async fn get_purchase_request(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Path(purchase_request_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let purchase_request_id = PurchaseRequestId::from_uuid(purchase_request_id);
    let (purchase, _) = authorize_for_purchase(
        &state,
        &headers,
        purchase_request_id,
        Action::limited(Feature::PurchaseRequestRead),
    )
    .await?;
    Ok(Json(purchase))
}

async fn submit_purchase_request(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Path(purchase_request_id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let purchase_request_id = PurchaseRequestId::from_uuid(purchase_request_id);
    let (_, principal) = authorize_for_purchase(
        &state,
        &headers,
        purchase_request_id,
        Action::new(Feature::PurchaseRequestCreate),
    )
    .await?;
    let purchase = state
        .store
        .submit_purchase_request(PurchaseSubmitCommand {
            actor: principal.user_id,
            purchase_request_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(purchase))
}

async fn approve_purchase_admin(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Path(purchase_request_id): Path<uuid::Uuid>,
    body: Bytes,
) -> Result<impl IntoResponse, RestError> {
    let body: FinancialStepUpRequest = decode_financial_request(body)?;
    let purchase_request_id = PurchaseRequestId::from_uuid(purchase_request_id);
    let (purchase, principal) = authorize_for_purchase(
        &state,
        &headers,
        purchase_request_id,
        Action::new(Feature::PurchaseRequestApprove),
    )
    .await?;
    verify_financial_step_up(
        &state,
        &principal,
        purchase.branch_id,
        purchase_request_id,
        FinancialStepUpAction::AdminApprove,
        body.step_up,
    )
    .await?;
    let purchase = state
        .store
        .approve_purchase_admin(PurchaseApprovalCommand {
            actor: principal.user_id,
            purchase_request_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(purchase))
}

async fn prepare_expenditure(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Path(purchase_request_id): Path<uuid::Uuid>,
    body: Bytes,
) -> Result<impl IntoResponse, RestError> {
    let body: PrepareExpenditureRequest = decode_financial_request(body)?;
    let purchase_request_id = PurchaseRequestId::from_uuid(purchase_request_id);
    let (purchase, principal) = authorize_for_purchase(
        &state,
        &headers,
        purchase_request_id,
        Action::new(Feature::PurchaseRequestCreate),
    )
    .await?;
    if body.step_up.is_some() && body.expenditure_no.trim().is_empty() {
        return Err(RestError::validation("expenditure number is required"));
    }
    verify_financial_step_up(
        &state,
        &principal,
        purchase.branch_id,
        purchase_request_id,
        FinancialStepUpAction::PrepareExpenditure,
        body.step_up,
    )
    .await?;
    let purchase = state
        .store
        .prepare_expenditure(PrepareExpenditureCommand {
            actor: principal.user_id,
            purchase_request_id,
            expenditure_no: body.expenditure_no,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(purchase))
}

async fn approve_purchase_executive(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Path(purchase_request_id): Path<uuid::Uuid>,
    body: Bytes,
) -> Result<impl IntoResponse, RestError> {
    let body: FinancialStepUpRequest = decode_financial_request(body)?;
    let purchase_request_id = PurchaseRequestId::from_uuid(purchase_request_id);
    let (purchase, principal) = authorize_for_purchase(
        &state,
        &headers,
        purchase_request_id,
        Action::new(Feature::PurchaseFinalApprove),
    )
    .await?;
    verify_financial_step_up(
        &state,
        &principal,
        purchase.branch_id,
        purchase_request_id,
        FinancialStepUpAction::ExecutiveApprove,
        body.step_up,
    )
    .await?;
    let purchase = state
        .store
        .approve_purchase_executive(PurchaseApprovalCommand {
            actor: principal.user_id,
            purchase_request_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(purchase))
}

async fn reject_purchase_request(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Path(purchase_request_id): Path<uuid::Uuid>,
    body: Bytes,
) -> Result<impl IntoResponse, RestError> {
    let body: RejectPurchaseRequest = decode_financial_request(body)?;
    let purchase_request_id = PurchaseRequestId::from_uuid(purchase_request_id);
    let (purchase, principal) =
        authorize_for_purchase_read(&state, &headers, purchase_request_id).await?;
    authorize_any(
        &principal,
        purchase.branch_id,
        &[
            Action::new(Feature::PurchaseRequestApprove),
            Action::new(Feature::PurchaseFinalApprove),
        ],
    )?;
    if body.step_up.is_some() && body.memo.trim().is_empty() {
        return Err(RestError::validation("reject memo is required"));
    }
    verify_financial_step_up(
        &state,
        &principal,
        purchase.branch_id,
        purchase_request_id,
        FinancialStepUpAction::Reject,
        body.step_up,
    )
    .await?;
    let purchase = state
        .store
        .reject_purchase_request(RejectPurchaseCommand {
            actor: principal.user_id,
            purchase_request_id,
            memo: body.memo,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(purchase))
}

async fn restart_purchase_request(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Path(purchase_request_id): Path<uuid::Uuid>,
    Json(body): Json<RestartPurchaseRequest>,
) -> Result<impl IntoResponse, RestError> {
    let purchase_request_id = PurchaseRequestId::from_uuid(purchase_request_id);
    let (_, principal) = authorize_for_purchase(
        &state,
        &headers,
        purchase_request_id,
        Action::new(Feature::PurchaseRequestCreate),
    )
    .await?;
    let purchase = state
        .store
        .restart_purchase_request(PurchaseRestartCommand {
            actor: principal.user_id,
            purchase_request_id,
            statement_evidence_id: body.statement_evidence_id,
            amount_won: body.amount_won,
            lines: body.lines,
            quote_attachment_ids: body.quote_attachment_ids,
            memo: body.memo,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(purchase))
}

async fn execute_purchase(
    State(state): State<FinancialRestState>,
    headers: HeaderMap,
    Path(purchase_request_id): Path<uuid::Uuid>,
    body: Bytes,
) -> Result<impl IntoResponse, RestError> {
    let body: FinancialStepUpRequest = decode_financial_request(body)?;
    let purchase_request_id = PurchaseRequestId::from_uuid(purchase_request_id);
    let (purchase, principal) = authorize_for_purchase(
        &state,
        &headers,
        purchase_request_id,
        Action::new(Feature::PurchaseExecute),
    )
    .await?;
    verify_financial_step_up(
        &state,
        &principal,
        purchase.branch_id,
        purchase_request_id,
        FinancialStepUpAction::Execute,
        body.step_up,
    )
    .await?;
    let purchase = state
        .store
        .execute_purchase(ExecutePurchaseCommand {
            actor: principal.user_id,
            purchase_request_id,
            trace: TraceContext::generate(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(purchase))
}

fn purchase_attachment_s3_key(org: console_kernel_core::OrgId, file_name: &str) -> String {
    let safe_name: String = file_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let safe_name = safe_name.trim_matches('_');
    let file_name = if safe_name.is_empty() {
        "quote"
    } else {
        safe_name
    };
    format!(
        "orgs/{}/purchase-requests/quotes/{}-{}",
        org.as_uuid(),
        uuid::Uuid::new_v4(),
        file_name
    )
}

fn validate_purchase_attachment_content_type(content_type: &str) -> Result<(), KernelError> {
    let content_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    const ALLOWED: &[&str] = &[
        "application/pdf",
        "image/jpeg",
        "image/png",
        "image/webp",
        "image/heic",
    ];
    if ALLOWED.iter().any(|allowed| *allowed == content_type) {
        Ok(())
    } else {
        Err(KernelError::validation(
            "purchase quote attachments must be PDF or image files",
        ))
    }
}

fn decode_financial_request<T>(body: Bytes) -> Result<T, RestError>
where
    T: DeserializeOwned + Default,
{
    if body.is_empty() || body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(&body)
        .map_err(|_| RestError::validation("request body must be valid JSON"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinancialStepUpAction {
    AdminApprove,
    PrepareExpenditure,
    ExecutiveApprove,
    Reject,
    Execute,
}

impl FinancialStepUpAction {
    const fn audit_action(self) -> &'static str {
        match self {
            Self::AdminApprove => "purchase.admin.approve",
            Self::PrepareExpenditure => "purchase.expenditure.prepare",
            Self::ExecutiveApprove => "purchase.executive.approve",
            Self::Reject => "purchase.reject",
            Self::Execute => "purchase.execute",
        }
    }

    const fn required_message(self) -> &'static str {
        match self {
            Self::AdminApprove => "admin purchase approval requires a fresh passkey step-up",
            Self::PrepareExpenditure => {
                "purchase expenditure preparation requires a fresh passkey step-up"
            }
            Self::ExecutiveApprove => {
                "executive purchase approval requires a fresh passkey step-up"
            }
            Self::Reject => "purchase rejection requires a fresh passkey step-up",
            Self::Execute => "purchase execution requires a fresh passkey step-up",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct FinancialStepUpFailure {
    code: &'static str,
    reason: &'static str,
}

async fn verify_financial_step_up(
    state: &FinancialRestState,
    principal: &Principal,
    branch_id: BranchId,
    purchase_request_id: PurchaseRequestId,
    action: FinancialStepUpAction,
    step_up: Option<PasskeyStepUpAssertionRequest>,
) -> Result<(), RestError> {
    let Some(step_up) = step_up else {
        let error =
            RestError::precondition_required("passkey_step_up_required", action.required_message());
        record_financial_step_up_failure(
            state,
            principal,
            branch_id,
            purchase_request_id,
            action,
            error.code(),
            "missing",
        )
        .await?;
        return Err(error);
    };

    let Some(verifier) = state.passkey_step_up.as_ref() else {
        let error = RestError::unavailable("passkey step-up is not configured for financial API");
        record_financial_step_up_failure(
            state,
            principal,
            branch_id,
            purchase_request_id,
            action,
            "passkey_step_up_unconfigured",
            "unconfigured",
        )
        .await?;
        return Err(error);
    };

    if let Err(err) = verifier
        .verify_step_up_for_user(
            state.store.pool(),
            step_up.ceremony_id,
            step_up.credential,
            *principal.user_id.as_uuid(),
        )
        .await
    {
        let Some(failure_reason) = financial_step_up_denial_reason(&err) else {
            return Err(rest_error_from_step_up_dependency(err));
        };
        tracing::warn!(
            action = action.audit_action(),
            purchase_request_id = %purchase_request_id,
            "financial passkey step-up rejected"
        );
        let error =
            RestError::passkey_step_up_failed("passkey_step_up_failed", "passkey step-up failed");
        record_financial_step_up_failure(
            state,
            principal,
            branch_id,
            purchase_request_id,
            action,
            error.code(),
            failure_reason,
        )
        .await?;
        return Err(error);
    }

    Ok(())
}

fn financial_step_up_denial_reason(error: &AuthError) -> Option<&'static str> {
    match error {
        AuthError::Webauthn(_) | AuthError::InvalidStoredData(_) => Some("invalid_or_expired"),
        AuthError::Sqlx(_)
        | AuthError::Db(_)
        | AuthError::Serde(_)
        | AuthError::Jwt(_)
        | AuthError::Kernel(_)
        | AuthError::Refresh(_) => None,
    }
}

fn rest_error_from_step_up_dependency(error: AuthError) -> RestError {
    match error {
        AuthError::Sqlx(err) => RestError::from_db(DbError::Sqlx(err)),
        AuthError::Db(err) => RestError::from_db(err),
        AuthError::Kernel(err) => RestError::from_kernel(err),
        AuthError::Serde(err) => {
            tracing::error!(error = %err, "passkey step-up verification state is invalid");
            RestError::internal("passkey step-up verification failed")
        }
        AuthError::Jwt(err) => {
            tracing::error!(error = %err, "unexpected JWT error during passkey step-up verification");
            RestError::internal("passkey step-up verification failed")
        }
        AuthError::Refresh(err) => {
            tracing::error!(error = %err, "unexpected refresh-token error during passkey step-up verification");
            RestError::internal("passkey step-up verification failed")
        }
        AuthError::Webauthn(_) | AuthError::InvalidStoredData(_) => {
            RestError::passkey_step_up_failed("passkey_step_up_failed", "passkey step-up failed")
        }
    }
}

async fn record_financial_step_up_failure(
    state: &FinancialRestState,
    principal: &Principal,
    branch_id: BranchId,
    purchase_request_id: PurchaseRequestId,
    action: FinancialStepUpAction,
    failure_code: &'static str,
    failure_reason: &'static str,
) -> Result<(), RestError> {
    let org =
        console_platform_request_context::current_org().map_err(rest_error_from_request_context)?;
    let event = financial_step_up_failure_event(
        principal.user_id,
        branch_id,
        purchase_request_id,
        action,
        FinancialStepUpFailure {
            code: failure_code,
            reason: failure_reason,
        },
        TraceContext::generate(),
        time::OffsetDateTime::now_utc(),
    )?
    .with_org(org);
    with_audit::<_, (), RestError>(state.store.pool(), event, |_tx| {
        Box::pin(async move { Ok(()) })
    })
    .await
}

fn financial_step_up_failure_event(
    actor: console_kernel_core::UserId,
    branch_id: BranchId,
    purchase_request_id: PurchaseRequestId,
    action: FinancialStepUpAction,
    failure: FinancialStepUpFailure,
    trace: TraceContext,
    occurred_at: time::OffsetDateTime,
) -> Result<console_kernel_core::AuditEvent, RestError> {
    Ok(financial_audit_event(
        "purchase.step_up.denied",
        actor,
        branch_id,
        "financial_purchase_request",
        purchase_request_id,
        trace,
        occurred_at,
    )?
    .with_snapshots(
        None,
        Some(serde_json::json!({
            "required_action": action.audit_action(),
            "failure_code": failure.code,
            "failure_reason": failure.reason,
            "step_up_verified": false,
        })),
    ))
}

async fn authorize_for_purchase(
    state: &FinancialRestState,
    headers: &HeaderMap,
    purchase_request_id: PurchaseRequestId,
    action: Action,
) -> Result<
    (
        console_financial_application::PurchaseRequestSummary,
        Principal,
    ),
    RestError,
> {
    let (purchase, principal) =
        authorize_for_purchase_read(state, headers, purchase_request_id).await?;
    authorize(&principal, action, purchase.branch_id).map_err(RestError::from_kernel)?;
    Ok((purchase, principal))
}

async fn authorize_for_purchase_read(
    state: &FinancialRestState,
    headers: &HeaderMap,
    purchase_request_id: PurchaseRequestId,
) -> Result<
    (
        console_financial_application::PurchaseRequestSummary,
        Principal,
    ),
    RestError,
> {
    let principal = principal_from_headers(state, headers).await?;
    let purchase = state
        .store
        .purchase_request(purchase_request_id)
        .await
        .map_err(RestError::from_store)?;
    if purchase.requester.user_id != principal.user_id {
        authorize(
            &principal,
            Action::limited(Feature::PurchaseRequestRead),
            purchase.branch_id,
        )
        .map_err(RestError::from_kernel)?;
    }
    Ok((purchase, principal))
}

fn authorize_any(
    principal: &Principal,
    branch_id: BranchId,
    actions: &[Action],
) -> Result<(), RestError> {
    if actions
        .iter()
        .any(|action| authorize(principal, *action, branch_id).is_ok())
    {
        Ok(())
    } else {
        Err(RestError::from_kernel(KernelError::forbidden(
            "role is not allowed to use feature",
        )))
    }
}

async fn principal_from_headers(
    state: &FinancialRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for financial API")
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
            RestError::unavailable("JWT verification is not configured for financial API")
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
            RestError::internal(message)
        }
        console_platform_request_context::RequestContextError::MissingOrg => {
            RestError::internal("no tenant context is bound to the current request")
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
    code_override: Option<&'static str>,
    message: String,
}

impl RestError {
    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            kind: ErrorKind::Forbidden,
            code_override: None,
            message: message.into(),
        }
    }

    fn validation(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            kind: ErrorKind::Validation,
            code_override: None,
            message: message.into(),
        }
    }

    fn precondition_required(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::PRECONDITION_REQUIRED,
            kind: ErrorKind::Validation,
            code_override: Some(code),
            message: message.into(),
        }
    }

    fn passkey_step_up_failed(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            kind: ErrorKind::Forbidden,
            code_override: Some(code),
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            kind: ErrorKind::Internal,
            code_override: None,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            kind: ErrorKind::Internal,
            code_override: None,
            message: message.into(),
        }
    }

    fn from_kernel(error: KernelError) -> Self {
        Self {
            status: status_for_error_kind(error.kind),
            kind: error.kind,
            code_override: None,
            message: error.message,
        }
    }

    fn from_store(error: PgFinancialError) -> Self {
        match error {
            PgFinancialError::Domain(error) => Self::from_kernel(error),
            PgFinancialError::Db(error) => Self::from_db(error),
        }
    }

    fn from_db(error: DbError) -> Self {
        match error {
            DbError::Sqlx(sqlx::Error::RowNotFound) => {
                Self::from_kernel(KernelError::not_found("row was not found"))
            }
            DbError::Sqlx(sqlx::Error::Database(err))
                if err.code().is_some_and(|code| code == "23505") =>
            {
                // Log the constraint name server-side; never leak it (schema
                // disclosure, OWASP A05). Clients get a stable generic message.
                tracing::error!(error = %err, "database unique-constraint violation");
                Self::from_kernel(KernelError::conflict("resource already exists"))
            }
            DbError::Sqlx(err) => {
                tracing::error!(error = %err, "database error");
                Self::internal("internal server error")
            }
            DbError::Serialize(err) => {
                tracing::error!(error = %err, "serialization error");
                Self::internal("internal server error")
            }
            DbError::CodeIssuance(err) => {
                tracing::error!(error = %err, "object-code issuance error");
                Self::internal("internal server error")
            }
        }
    }

    fn code(&self) -> &'static str {
        if let Some(code) = self.code_override {
            return code;
        }
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

impl From<DbError> for RestError {
    fn from(value: DbError) -> Self {
        Self::from_db(value)
    }
}

impl From<KernelError> for RestError {
    fn from(value: KernelError) -> Self {
        Self::from_kernel(value)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_sensitive_financial_body_is_decoded_as_missing_step_up() {
        let body: FinancialStepUpRequest = decode_financial_request(Bytes::new()).unwrap();
        assert!(body.step_up.is_none());
    }

    #[test]
    fn purchase_request_collection_parser_accepts_only_repeated_status_keys() {
        let branch_id = BranchId::new();
        let parsed = parse_purchase_request_list_query(Some(&format!(
            "branch_id={branch_id}&status=STATEMENT_ATTACHED&status=REQUEST_SUBMITTED&limit=10&offset=2"
        )))
        .unwrap();
        assert_eq!(parsed.branch_id, branch_id);
        assert_eq!(
            parsed.statuses,
            vec![
                PurchaseStatus::StatementAttached,
                PurchaseStatus::RequestSubmitted
            ]
        );
        assert_eq!(parsed.limit, 10);
        assert_eq!(parsed.offset, 2);
    }

    #[test]
    fn purchase_request_collection_parser_rejects_ambiguous_or_malformed_keys() {
        let branch_id = BranchId::new();
        for raw in [
            format!("branch_id={branch_id}&status[]=STATEMENT_ATTACHED"),
            format!("branch_id={branch_id}&limit=one"),
            format!("branch_id={branch_id}&offset=-1"),
            "branch_id=not-a-uuid".to_owned(),
            format!("branch_id={branch_id}&unknown=value"),
        ] {
            assert!(
                parse_purchase_request_list_query(Some(&raw)).is_err(),
                "{raw}"
            );
        }
    }

    #[test]
    fn missing_step_up_error_uses_precondition_required_code() {
        let err = RestError::precondition_required(
            "passkey_step_up_required",
            "financial action requires a fresh passkey step-up",
        );
        assert_eq!(err.status, StatusCode::PRECONDITION_REQUIRED);
        assert_eq!(err.code(), "passkey_step_up_required");
    }

    #[test]
    fn denied_step_up_audit_event_contains_no_assertion_material() {
        let actor = console_kernel_core::UserId::new();
        let branch = BranchId::new();
        let purchase_request_id = PurchaseRequestId::new();
        let event = financial_step_up_failure_event(
            actor,
            branch,
            purchase_request_id,
            FinancialStepUpAction::Execute,
            FinancialStepUpFailure {
                code: "passkey_step_up_failed",
                reason: "invalid_or_expired",
            },
            TraceContext::generate(),
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();

        assert_eq!(event.action.as_str(), "purchase.step_up.denied");
        assert_eq!(event.target_type, "financial_purchase_request");
        assert_eq!(event.target_id, purchase_request_id.to_string());
        let after = event.after.expect("denial audit snapshot");
        assert_eq!(after["required_action"], "purchase.execute");
        assert_eq!(after["failure_code"], "passkey_step_up_failed");
        assert_eq!(after["failure_reason"], "invalid_or_expired");
        assert!(after.get("credential").is_none());
        assert!(after.get("assertion").is_none());
        assert!(after.get("ceremony_id").is_none());
    }

    #[test]
    fn step_up_dependency_error_is_not_reported_as_credential_denial() {
        let err = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let rest_error = rest_error_from_step_up_dependency(AuthError::Serde(err));

        assert_eq!(rest_error.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(rest_error.code(), "internal");
        assert_eq!(
            financial_step_up_denial_reason(&AuthError::InvalidStoredData(
                "ceremony not found or already consumed".to_owned()
            )),
            Some("invalid_or_expired")
        );
    }
}
