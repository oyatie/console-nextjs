//! Authenticated, tenant-scoped inventory REST routes.
//!
//! The Postgres adapter remains the transaction owner: it locks stock rows,
//! rejects negative balances, records idempotency fingerprints, and emits the
//! inventory audit event in the same transaction as a consumption event.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_inventory_adapter_postgres::{PgInventoryError, PgInventoryStore};
use console_inventory_application::{
    CancelCycleCountCommand, ConsumeInventoryCommand, ConsumeInventorySource, CycleCountDecision,
    CycleCountDetail, CycleCountPage, DecideCycleCountCommand, InventoryConsumptionEventView,
    InventoryConsumptionResult, InventoryItemPage, InventoryItemView, InventoryMovementView,
    InventoryReceiptResult, ListConsumptionEventsQuery, ListCycleCountsQuery,
    ListInventoryItemsQuery, ListMovementsQuery, MrpLineView, MrpQuery, OpenCycleCountCommand,
    RecordReceiptCommand, SubmitCycleCountCommand, UpsertCountLineCommand,
};
use console_inventory_domain::{CycleCountStatus, InventoryItemStatus, VarianceReason};
use console_kernel_core::{
    BranchId, BranchScope, ErrorKind, InventoryItemId, InventoryStockLocationId, KernelError,
    P1DispatchId, SiteId, TraceContext, WorkOrderId,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{
    Action, EffectiveFeatureGrant, Feature, PermissionLevel, Principal, authorize, permission_for,
};
use console_platform_request_context::RequestContextError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use time::OffsetDateTime;
use uuid::Uuid;

pub const INVENTORY_ITEMS_PATH: &str = "/api/v1/inventory/items";
pub const INVENTORY_ITEM_PATH_TEMPLATE: &str = "/api/v1/inventory/items/{item_id}";
pub const INVENTORY_ITEM_CONSUMPTIONS_PATH_TEMPLATE: &str =
    "/api/v1/inventory/items/{item_id}/consumptions";
pub const INVENTORY_ITEM_MOVEMENTS_PATH_TEMPLATE: &str =
    "/api/v1/inventory/items/{item_id}/movements";
pub const INVENTORY_ITEM_RECEIPTS_PATH_TEMPLATE: &str =
    "/api/v1/inventory/items/{item_id}/receipts";
pub const INVENTORY_MRP_PATH: &str = "/api/v1/inventory/mrp";
pub const INVENTORY_CYCLE_COUNTS_PATH: &str = "/api/v1/inventory/cycle-counts";
pub const INVENTORY_CYCLE_COUNT_PATH_TEMPLATE: &str = "/api/v1/inventory/cycle-counts/{count_id}";
pub const INVENTORY_CYCLE_COUNT_LINES_PATH_TEMPLATE: &str =
    "/api/v1/inventory/cycle-counts/{count_id}/lines";
pub const INVENTORY_CYCLE_COUNT_SUBMIT_PATH_TEMPLATE: &str =
    "/api/v1/inventory/cycle-counts/{count_id}/submit";
pub const INVENTORY_CYCLE_COUNT_DECIDE_PATH_TEMPLATE: &str =
    "/api/v1/inventory/cycle-counts/{count_id}/decision";
pub const INVENTORY_CYCLE_COUNT_CANCEL_PATH_TEMPLATE: &str =
    "/api/v1/inventory/cycle-counts/{count_id}/cancel";
pub const INVENTORY_ROUTE_PATHS: &[&str] = &[
    INVENTORY_ITEMS_PATH,
    INVENTORY_ITEM_PATH_TEMPLATE,
    INVENTORY_ITEM_CONSUMPTIONS_PATH_TEMPLATE,
    INVENTORY_ITEM_MOVEMENTS_PATH_TEMPLATE,
    INVENTORY_ITEM_RECEIPTS_PATH_TEMPLATE,
    INVENTORY_MRP_PATH,
    INVENTORY_CYCLE_COUNTS_PATH,
    INVENTORY_CYCLE_COUNT_PATH_TEMPLATE,
    INVENTORY_CYCLE_COUNT_LINES_PATH_TEMPLATE,
    INVENTORY_CYCLE_COUNT_SUBMIT_PATH_TEMPLATE,
    INVENTORY_CYCLE_COUNT_DECIDE_PATH_TEMPLATE,
    INVENTORY_CYCLE_COUNT_CANCEL_PATH_TEMPLATE,
];

#[derive(Clone)]
pub struct InventoryRestState {
    store: PgInventoryStore,
    jwt_verifier: Option<JwtVerifier>,
}

impl InventoryRestState {
    #[must_use]
    pub fn new(store: PgInventoryStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
        }
    }
}

pub fn router(state: InventoryRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(INVENTORY_ITEMS_PATH, get(list_items))
        .route(INVENTORY_ITEM_PATH_TEMPLATE, get(get_item))
        .route(
            INVENTORY_ITEM_CONSUMPTIONS_PATH_TEMPLATE,
            get(list_consumptions).post(consume_item),
        )
        .route(INVENTORY_ITEM_MOVEMENTS_PATH_TEMPLATE, get(list_movements))
        .route(INVENTORY_ITEM_RECEIPTS_PATH_TEMPLATE, post(record_receipt))
        .route(INVENTORY_MRP_PATH, get(mrp))
        .route(
            INVENTORY_CYCLE_COUNTS_PATH,
            get(list_cycle_counts).post(open_cycle_count),
        )
        .route(INVENTORY_CYCLE_COUNT_PATH_TEMPLATE, get(get_cycle_count))
        .route(
            INVENTORY_CYCLE_COUNT_LINES_PATH_TEMPLATE,
            post(upsert_cycle_count_line),
        )
        .route(
            INVENTORY_CYCLE_COUNT_SUBMIT_PATH_TEMPLATE,
            post(submit_cycle_count),
        )
        .route(
            INVENTORY_CYCLE_COUNT_DECIDE_PATH_TEMPLATE,
            post(decide_cycle_count),
        )
        .route(
            INVENTORY_CYCLE_COUNT_CANCEL_PATH_TEMPLATE,
            post(cancel_cycle_count),
        )
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListItemsParams {
    branch_id: Option<Uuid>,
    site_id: Option<Uuid>,
    stock_location_id: Option<Uuid>,
    status: Option<InventoryItemStatus>,
    low_stock: Option<bool>,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_items(
    State(state): State<InventoryRestState>,
    headers: HeaderMap,
    Query(params): Query<ListItemsParams>,
) -> Result<Json<InventoryItemPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let branch_scope = authorized_feature_scope(&principal, Feature::InventoryRead)?;
    if let Some(branch_id) = params.branch_id {
        ensure_scope_allows(&branch_scope, BranchId::from_uuid(branch_id))?;
    }
    let page = state
        .store
        .list_items(ListInventoryItemsQuery {
            branch_scope,
            branch_id: params.branch_id.map(BranchId::from_uuid),
            site_id: params.site_id.map(SiteId::from_uuid),
            stock_location_id: params
                .stock_location_id
                .map(InventoryStockLocationId::from_uuid),
            status: params.status,
            low_stock: params.low_stock,
            q: params.q,
            limit: params.limit,
            offset: params.offset,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page))
}

async fn get_item(
    State(state): State<InventoryRestState>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<Json<InventoryItemView>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let item = find_item(
        &state.store,
        InventoryItemId::from_uuid(item_id),
        &principal,
    )
    .await?;
    authorize(
        &principal,
        Action::new(Feature::InventoryRead),
        item.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    Ok(Json(item))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListConsumptionsParams {
    source_kind: Option<String>,
    work_order_id: Option<Uuid>,
    dispatch_id: Option<Uuid>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_consumptions(
    State(state): State<InventoryRestState>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(params): Query<ListConsumptionsParams>,
) -> Result<Json<Vec<InventoryConsumptionEventView>>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let item_id = InventoryItemId::from_uuid(item_id);
    let item = find_item(&state.store, item_id, &principal).await?;
    authorize(
        &principal,
        Action::new(Feature::InventoryRead),
        item.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let events = state
        .store
        .list_consumption_events(ListConsumptionEventsQuery {
            branch_scope: principal.branch_scope,
            item_id,
            source_kind: params.source_kind,
            work_order_id: params.work_order_id.map(WorkOrderId::from_uuid),
            dispatch_id: params.dispatch_id.map(P1DispatchId::from_uuid),
            limit: params.limit,
            offset: params.offset,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(events))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConsumeItemBody {
    source: ConsumptionSourceBody,
    quantity_consumed_milli: i64,
    occurred_at: Option<OffsetDateTime>,
    memo: Option<String>,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ConsumptionSourceBody {
    WorkOrder { work_order_id: Uuid },
    P1Dispatch { dispatch_id: Uuid },
}

impl From<ConsumptionSourceBody> for ConsumeInventorySource {
    fn from(value: ConsumptionSourceBody) -> Self {
        match value {
            ConsumptionSourceBody::WorkOrder { work_order_id } => Self::WorkOrder {
                work_order_id: WorkOrderId::from_uuid(work_order_id),
            },
            ConsumptionSourceBody::P1Dispatch { dispatch_id } => Self::P1Dispatch {
                dispatch_id: P1DispatchId::from_uuid(dispatch_id),
            },
        }
    }
}

async fn consume_item(
    State(state): State<InventoryRestState>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Json(body): Json<ConsumeItemBody>,
) -> Result<Json<InventoryConsumptionResult>, RestError> {
    validate_idempotency_key(&body.idempotency_key).map_err(RestError::from_kernel)?;
    let trace = trace_context_from_headers(&headers).map_err(RestError::from_kernel)?;
    let principal = principal_from_headers(&state, &headers).await?;
    let item_id = InventoryItemId::from_uuid(item_id);
    let item = find_item(&state.store, item_id, &principal).await?;
    authorize(
        &principal,
        Action::new(Feature::InventoryConsume),
        item.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    let now = OffsetDateTime::now_utc();
    let result = state
        .store
        .consume_item(ConsumeInventoryCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope,
            item_id,
            source: body.source.into(),
            quantity_consumed_milli: body.quantity_consumed_milli,
            occurred_at: body.occurred_at,
            memo: body.memo,
            idempotency_key: body.idempotency_key,
            trace,
            requested_at: now,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PageParams {
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_movements(
    State(state): State<InventoryRestState>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(page): Query<PageParams>,
) -> Result<Json<Vec<InventoryMovementView>>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let item = find_item(
        &state.store,
        InventoryItemId::from_uuid(item_id),
        &principal,
    )
    .await?;
    authorize(
        &principal,
        Action::new(Feature::InventoryRead),
        item.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    Ok(Json(
        state
            .store
            .list_movements(ListMovementsQuery {
                branch_scope: principal.branch_scope,
                item_id: InventoryItemId::from_uuid(item_id),
                limit: page.limit,
                offset: page.offset,
            })
            .await
            .map_err(RestError::from_store)?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptBody {
    quantity_received_milli: i64,
    source_ref: Option<String>,
    memo: Option<String>,
    idempotency_key: String,
}
async fn record_receipt(
    State(state): State<InventoryRestState>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Json(body): Json<ReceiptBody>,
) -> Result<Json<InventoryReceiptResult>, RestError> {
    validate_idempotency_key(&body.idempotency_key).map_err(RestError::from_kernel)?;
    let trace = trace_context_from_headers(&headers).map_err(RestError::from_kernel)?;
    let principal = principal_from_headers(&state, &headers).await?;
    let item = find_item(
        &state.store,
        InventoryItemId::from_uuid(item_id),
        &principal,
    )
    .await?;
    let branch_scope = authorized_feature_scope(&principal, Feature::InventoryConsume)?;
    ensure_scope_allows(&branch_scope, item.branch_id)?;
    Ok(Json(
        state
            .store
            .record_receipt(RecordReceiptCommand {
                actor: principal.user_id,
                branch_scope,
                item_id: InventoryItemId::from_uuid(item_id),
                quantity_received_milli: body.quantity_received_milli,
                source_ref: body.source_ref,
                memo: body.memo,
                idempotency_key: body.idempotency_key,
                trace,
                requested_at: OffsetDateTime::now_utc(),
            })
            .await
            .map_err(RestError::from_store)?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BranchParams {
    branch_id: Uuid,
}
async fn mrp(
    State(state): State<InventoryRestState>,
    headers: HeaderMap,
    Query(params): Query<BranchParams>,
) -> Result<Json<Vec<MrpLineView>>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let branch = BranchId::from_uuid(params.branch_id);
    authorize(&principal, Action::new(Feature::InventoryRead), branch)
        .map_err(RestError::from_kernel)?;
    Ok(Json(
        state
            .store
            .mrp(MrpQuery {
                branch_scope: principal.branch_scope,
                branch_id: branch,
            })
            .await
            .map_err(RestError::from_store)?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CycleListParams {
    branch_id: Uuid,
    status: Option<CycleCountStatus>,
    limit: Option<i64>,
    offset: Option<i64>,
}
async fn list_cycle_counts(
    State(state): State<InventoryRestState>,
    headers: HeaderMap,
    Query(params): Query<CycleListParams>,
) -> Result<Json<CycleCountPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let branch = BranchId::from_uuid(params.branch_id);
    authorize(&principal, Action::new(Feature::InventoryRead), branch)
        .map_err(RestError::from_kernel)?;
    Ok(Json(
        state
            .store
            .list_cycle_counts(ListCycleCountsQuery {
                branch_scope: principal.branch_scope,
                branch_id: branch,
                status: params.status,
                limit: params.limit,
                offset: params.offset,
            })
            .await
            .map_err(RestError::from_store)?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenCountBody {
    branch_id: Uuid,
    stock_location_id: Uuid,
}
async fn open_cycle_count(
    State(state): State<InventoryRestState>,
    headers: HeaderMap,
    Json(body): Json<OpenCountBody>,
) -> Result<Json<CycleCountDetail>, RestError> {
    let trace = trace_context_from_headers(&headers).map_err(RestError::from_kernel)?;
    let principal = principal_from_headers(&state, &headers).await?;
    let branch = BranchId::from_uuid(body.branch_id);
    let branch_scope = authorized_feature_scope(&principal, Feature::InventoryConsume)?;
    ensure_scope_allows(&branch_scope, branch)?;
    Ok(Json(
        state
            .store
            .open_cycle_count(OpenCycleCountCommand {
                actor: principal.user_id,
                branch_scope,
                branch_id: branch,
                stock_location_id: InventoryStockLocationId::from_uuid(body.stock_location_id),
                trace,
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .map_err(RestError::from_store)?,
    ))
}
async fn get_cycle_count(
    State(state): State<InventoryRestState>,
    headers: HeaderMap,
    Path(count_id): Path<Uuid>,
) -> Result<Json<CycleCountDetail>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let detail = state
        .store
        .get_cycle_count(count_id, principal.branch_scope.clone())
        .await
        .map_err(RestError::from_store)?
        .ok_or_else(|| {
            RestError::from_kernel(KernelError::not_found("cycle count was not found"))
        })?;
    authorize(
        &principal,
        Action::new(Feature::InventoryRead),
        detail.count.branch_id,
    )
    .map_err(RestError::from_kernel)?;
    Ok(Json(detail))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CountLineBody {
    expected_version: i32,
    item_id: Uuid,
    counted_quantity_milli: i64,
    reason: Option<VarianceReason>,
    note: Option<String>,
}
async fn upsert_cycle_count_line(
    State(state): State<InventoryRestState>,
    headers: HeaderMap,
    Path(count_id): Path<Uuid>,
    Json(body): Json<CountLineBody>,
) -> Result<Json<CycleCountDetail>, RestError> {
    let trace = trace_context_from_headers(&headers).map_err(RestError::from_kernel)?;
    let principal = principal_from_headers(&state, &headers).await?;
    let branch_scope = authorized_feature_scope(&principal, Feature::InventoryConsume)?;
    Ok(Json(
        state
            .store
            .upsert_cycle_count_line(UpsertCountLineCommand {
                actor: principal.user_id,
                branch_scope,
                count_id,
                expected_version: body.expected_version,
                item_id: InventoryItemId::from_uuid(body.item_id),
                counted_quantity_milli: body.counted_quantity_milli,
                reason: body.reason,
                note: body.note,
                trace,
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .map_err(RestError::from_store)?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VersionBody {
    expected_version: i32,
}
async fn submit_cycle_count(
    State(state): State<InventoryRestState>,
    headers: HeaderMap,
    Path(count_id): Path<Uuid>,
    Json(body): Json<VersionBody>,
) -> Result<Json<CycleCountDetail>, RestError> {
    let trace = trace_context_from_headers(&headers).map_err(RestError::from_kernel)?;
    let principal = principal_from_headers(&state, &headers).await?;
    let branch_scope = authorized_feature_scope(&principal, Feature::InventoryConsume)?;
    Ok(Json(
        state
            .store
            .submit_cycle_count(SubmitCycleCountCommand {
                actor: principal.user_id,
                branch_scope,
                count_id,
                expected_version: body.expected_version,
                trace,
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .map_err(RestError::from_store)?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DecideBody {
    expected_version: i32,
    decision: CycleCountDecision,
    memo: Option<String>,
    idempotency_key: Option<String>,
}
async fn decide_cycle_count(
    State(state): State<InventoryRestState>,
    headers: HeaderMap,
    Path(count_id): Path<Uuid>,
    Json(body): Json<DecideBody>,
) -> Result<Json<CycleCountDetail>, RestError> {
    let trace = trace_context_from_headers(&headers).map_err(RestError::from_kernel)?;
    let principal = principal_from_headers(&state, &headers).await?;
    let branch_scope = authorized_feature_scope(&principal, Feature::InventoryConsume)?;
    Ok(Json(
        state
            .store
            .decide_cycle_count(DecideCycleCountCommand {
                actor: principal.user_id,
                branch_scope,
                count_id,
                expected_version: body.expected_version,
                decision: body.decision,
                memo: body.memo,
                idempotency_key: body.idempotency_key,
                trace,
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .map_err(RestError::from_store)?,
    ))
}
async fn cancel_cycle_count(
    State(state): State<InventoryRestState>,
    headers: HeaderMap,
    Path(count_id): Path<Uuid>,
    Json(body): Json<VersionBody>,
) -> Result<Json<CycleCountDetail>, RestError> {
    let trace = trace_context_from_headers(&headers).map_err(RestError::from_kernel)?;
    let principal = principal_from_headers(&state, &headers).await?;
    let branch_scope = authorized_feature_scope(&principal, Feature::InventoryConsume)?;
    Ok(Json(
        state
            .store
            .cancel_cycle_count(CancelCycleCountCommand {
                actor: principal.user_id,
                branch_scope,
                count_id,
                expected_version: body.expected_version,
                trace,
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .map_err(RestError::from_store)?,
    ))
}

async fn find_item(
    store: &PgInventoryStore,
    item_id: InventoryItemId,
    principal: &Principal,
) -> Result<InventoryItemView, RestError> {
    store
        .get_item(item_id, principal.branch_scope.clone())
        .await
        .map_err(RestError::from_store)?
        .ok_or_else(|| {
            RestError::from_kernel(KernelError::not_found("inventory item was not found"))
        })
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
            BranchScope::Branches(granted) => branches.extend(granted),
        }
    }
    (!branches.is_empty()).then_some(BranchScope::Branches(branches))
}

fn ensure_scope_allows(scope: &BranchScope, branch: BranchId) -> Result<(), RestError> {
    if scope.allows(branch) {
        Ok(())
    } else {
        Err(RestError::from_kernel(KernelError::forbidden(
            "requested branch is outside principal feature scope",
        )))
    }
}

fn trace_context_from_headers(headers: &HeaderMap) -> Result<TraceContext, KernelError> {
    let Some(traceparent) = headers.get("traceparent") else {
        return Ok(TraceContext::generate());
    };
    let traceparent = traceparent
        .to_str()
        .map_err(|_| KernelError::validation("traceparent must be valid header text"))?;
    let mut fields = traceparent.split('-');
    let (Some(_version), Some(trace_id), Some(span_id), Some(_flags)) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err(KernelError::validation(
            "traceparent must have version-trace-id-parent-id-flags fields",
        ));
    };
    if fields.next().is_some() {
        return Err(KernelError::validation(
            "traceparent must have version-trace-id-parent-id-flags fields",
        ));
    }
    TraceContext::new(trace_id, span_id)
}

fn validate_idempotency_key(value: &str) -> Result<(), KernelError> {
    let trimmed = value.trim();
    if !(16..=200).contains(&trimmed.chars().count()) {
        return Err(KernelError::validation(
            "idempotency_key must be between 16 and 200 characters",
        ));
    }
    Ok(())
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

    fn from_kernel(error: KernelError) -> Self {
        match error.kind {
            ErrorKind::Validation => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                error.message,
            ),
            ErrorKind::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", error.message),
            ErrorKind::Forbidden => Self::new(StatusCode::FORBIDDEN, "forbidden", error.message),
            ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                Self::new(StatusCode::CONFLICT, "conflict", error.message)
            }
            ErrorKind::Internal => {
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", error.message)
            }
        }
    }

    fn from_store(error: PgInventoryError) -> Self {
        match error {
            PgInventoryError::Domain(error) => Self::from_kernel(error),
            error => match error.kind() {
                ErrorKind::NotFound => Self::new(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "inventory resource was not found",
                ),
                ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                    Self::new(StatusCode::CONFLICT, "conflict", "inventory state conflict")
                }
                ErrorKind::Validation | ErrorKind::Forbidden | ErrorKind::Internal => {
                    tracing::error!(error = %error, "inventory store error");
                    Self::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "internal",
                        "internal server error",
                    )
                }
            },
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

async fn principal_from_headers(
    state: &InventoryRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "JWT verification is not configured for the inventory API",
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
                RestError::from_kernel(KernelError::forbidden(
                    "token is not authorized for this inventory route",
                ))
            }
            RequestContextError::VerifierUnavailable => RestError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "JWT verification is not configured for the inventory API",
            ),
            RequestContextError::BranchScope(message)
            | RequestContextError::EffectivePolicy(message) => {
                RestError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
            }
            RequestContextError::MissingOrg => RestError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "no tenant context is bound to the current request",
            ),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_kernel_core::OrgId;
    use console_platform_authz::Role;

    fn principal(role: Role, scope: BranchScope) -> Principal {
        Principal::new(
            console_kernel_core::UserId::new(),
            OrgId::knl(),
            BTreeSet::from([role]),
            scope,
        )
    }

    #[test]
    fn consumption_idempotency_key_matches_the_authoritative_character_contract() {
        assert!(validate_idempotency_key(&"x".repeat(15)).is_err());
        assert!(validate_idempotency_key(&"x".repeat(16)).is_ok());
        assert!(validate_idempotency_key(&"x".repeat(200)).is_ok());
        assert!(validate_idempotency_key(&"x".repeat(201)).is_err());
        assert!(validate_idempotency_key(&format!("  {}  ", "x".repeat(16))).is_ok());
    }

    #[test]
    fn inventory_read_and_consume_default_deny_members() {
        let principal = principal(Role::Member, BranchScope::All);
        assert_eq!(
            authorized_feature_scope(&principal, Feature::InventoryRead)
                .unwrap_err()
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            authorized_feature_scope(&principal, Feature::InventoryConsume)
                .unwrap_err()
                .status,
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn custom_inventory_read_grant_is_intersected_with_membership_scope() {
        let member_branch = BranchId::new();
        let other_member_branch = BranchId::new();
        let grant_only_branch = BranchId::new();
        let principal = principal(
            Role::Member,
            BranchScope::Branches(BTreeSet::from([member_branch, other_member_branch])),
        )
        .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
            Feature::InventoryRead,
            PermissionLevel::Allow,
            BranchScope::Branches(BTreeSet::from([member_branch, grant_only_branch])),
        )]);

        assert_eq!(
            authorized_feature_scope(&principal, Feature::InventoryRead).unwrap(),
            BranchScope::single(member_branch)
        );
    }

    #[test]
    fn custom_grant_order_cannot_false_deny_an_authorized_branch() {
        let first_branch = BranchId::new();
        let second_branch = BranchId::new();
        let principal = principal(
            Role::Member,
            BranchScope::Branches(BTreeSet::from([first_branch, second_branch])),
        )
        .with_effective_feature_grants(vec![
            EffectiveFeatureGrant::new(
                Feature::InventoryRead,
                PermissionLevel::Allow,
                BranchScope::single(first_branch),
            ),
            EffectiveFeatureGrant::new(
                Feature::InventoryRead,
                PermissionLevel::Allow,
                BranchScope::single(second_branch),
            ),
        ]);

        let scope = authorized_feature_scope(&principal, Feature::InventoryRead).unwrap();
        assert!(scope.allows(first_branch));
        assert!(scope.allows(second_branch));
    }

    #[test]
    fn custom_consume_grant_is_narrowed_before_cycle_mutations_reach_the_store() {
        let allowed = BranchId::new();
        let denied = BranchId::new();
        let principal = principal(
            Role::Member,
            BranchScope::Branches(BTreeSet::from([allowed, denied])),
        )
        .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
            Feature::InventoryConsume,
            PermissionLevel::Allow,
            BranchScope::single(allowed),
        )]);
        let scope = authorized_feature_scope(&principal, Feature::InventoryConsume).unwrap();
        assert!(ensure_scope_allows(&scope, allowed).is_ok());
        assert_eq!(
            ensure_scope_allows(&scope, denied).unwrap_err().status,
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn cycle_mutation_bodies_are_strict_camel_case_and_versioned() {
        let body = serde_json::json!({
            "expectedVersion": 3,
            "itemId": "00000000-0000-0000-0000-000000000001",
            "countedQuantityMilli": 1000,
            "reason": "MISCOUNT"
        });
        assert!(serde_json::from_value::<CountLineBody>(body).is_ok());
        assert!(
            serde_json::from_value::<CountLineBody>(serde_json::json!({
                "expected_version": 3,
                "item_id": "00000000-0000-0000-0000-000000000001",
                "counted_quantity_milli": 1000
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<VersionBody>(serde_json::json!({"expectedVersion": 3}))
                .is_ok()
        );
    }

    #[test]
    fn requested_branch_outside_read_scope_is_forbidden() {
        let allowed = BranchId::new();
        let denied = BranchId::new();
        let scope = BranchScope::single(allowed);
        assert!(ensure_scope_allows(&scope, allowed).is_ok());
        assert_eq!(
            ensure_scope_allows(&scope, denied).unwrap_err().status,
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn consume_is_denied_without_the_consume_feature_even_on_a_member_branch() {
        let branch = BranchId::new();
        let principal = principal(Role::Member, BranchScope::single(branch));
        let error =
            authorize(&principal, Action::new(Feature::InventoryConsume), branch).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Forbidden);
    }

    #[test]
    fn store_forbidden_maps_to_typed_forbidden_response() {
        let error = RestError::from_store(PgInventoryError::Domain(KernelError::forbidden(
            "inventory branch is outside principal scope",
        )));
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(error.code, "forbidden");
    }

    #[test]
    fn mutation_reuses_valid_inbound_traceparent() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "traceparent",
            "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01"
                .parse()
                .unwrap(),
        );
        let trace = trace_context_from_headers(&headers).unwrap();
        assert_eq!(trace.trace_id(), "0123456789abcdef0123456789abcdef");
        assert_eq!(trace.span_id(), "0123456789abcdef");
    }

    #[test]
    fn malformed_inbound_traceparent_is_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", "not-a-traceparent".parse().unwrap());
        let error = trace_context_from_headers(&headers).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Validation);
    }

    #[test]
    fn consume_body_rejects_caller_tenant_or_branch_identity() {
        let body = serde_json::json!({
            "orgId": "00000000-0000-0000-0000-000000000001",
            "branchId": "00000000-0000-0000-0000-000000000002",
            "source": {"kind": "work_order", "workOrderId": "00000000-0000-0000-0000-000000000003"},
            "quantityConsumedMilli": 1000,
            "idempotencyKey": "consume-20260724-0001"
        });
        assert!(serde_json::from_value::<ConsumeItemBody>(body).is_err());
    }

    #[test]
    fn route_surface_exposes_reads_and_a_single_audited_consumption_write() {
        assert!(INVENTORY_ROUTE_PATHS.contains(&INVENTORY_ITEMS_PATH));
        assert!(INVENTORY_ROUTE_PATHS.contains(&INVENTORY_ITEM_PATH_TEMPLATE));
        assert!(INVENTORY_ROUTE_PATHS.contains(&INVENTORY_ITEM_CONSUMPTIONS_PATH_TEMPLATE));
        assert!(INVENTORY_ROUTE_PATHS.contains(&INVENTORY_ITEM_MOVEMENTS_PATH_TEMPLATE));
        assert!(INVENTORY_ROUTE_PATHS.contains(&INVENTORY_ITEM_RECEIPTS_PATH_TEMPLATE));
        assert!(INVENTORY_ROUTE_PATHS.contains(&INVENTORY_MRP_PATH));
        assert!(INVENTORY_ROUTE_PATHS.contains(&INVENTORY_CYCLE_COUNTS_PATH));
        assert!(INVENTORY_ROUTE_PATHS.contains(&INVENTORY_CYCLE_COUNT_DECIDE_PATH_TEMPLATE));
        assert_eq!(
            INVENTORY_ROUTE_PATHS
                .iter()
                .filter(|path| path.contains("consumption"))
                .count(),
            1
        );
    }
}
