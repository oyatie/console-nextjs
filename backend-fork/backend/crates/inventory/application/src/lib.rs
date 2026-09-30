//! Inventory application contracts: use-case DTOs, read models, source refs, and
//! audit event builders. Org scope is intentionally absent from commands; the
//! adapter derives it from the authenticated request context/current principal.

use console_inventory_domain::{
    CycleCountStatus, InventoryConsumptionSource, InventoryItemStatus, MovementKind, VarianceReason,
};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, InventoryConsumptionEventId, InventoryItemId,
    InventoryStockLocationId, KernelError, P1DispatchId, SiteId, Timestamp, TraceContext, UserId,
    WorkOrderId,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryStockLocationView {
    pub id: InventoryStockLocationId,
    pub branch_id: BranchId,
    pub site_id: Option<SiteId>,
    pub location_code: Option<String>,
    pub label: String,
    pub status: InventoryItemStatus,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryStockLocationSummary {
    pub id: InventoryStockLocationId,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryItemView {
    pub id: InventoryItemId,
    pub branch_id: BranchId,
    pub site_id: Option<SiteId>,
    pub stock_location: InventoryStockLocationSummary,
    pub iv_code: String,
    pub sku: Option<String>,
    pub display_name: String,
    pub description: Option<String>,
    pub unit_code: String,
    pub quantity_on_hand_milli: i64,
    pub safety_stock_milli: i64,
    pub unit_cost_won: Option<i64>,
    pub low_stock: bool,
    pub status: InventoryItemStatus,
    pub href: String,
    pub created_by: UserId,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryItemPage {
    pub items: Vec<InventoryItemView>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryConsumptionEventView {
    pub id: InventoryConsumptionEventId,
    pub item_id: InventoryItemId,
    pub iv_code: String,
    pub branch_id: BranchId,
    pub stock_location_id: InventoryStockLocationId,
    pub source: InventoryConsumptionSource,
    pub quantity_before_milli: i64,
    pub quantity_consumed_milli: i64,
    pub quantity_after_milli: i64,
    pub unit_cost_won: Option<i64>,
    pub cost_won: Option<i64>,
    pub consumed_by: UserId,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: Timestamp,
    pub memo: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryConsumptionResult {
    pub event: InventoryConsumptionEventView,
    pub item: InventoryItemView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListInventoryItemsQuery {
    pub branch_scope: BranchScope,
    pub branch_id: Option<BranchId>,
    pub site_id: Option<SiteId>,
    pub stock_location_id: Option<InventoryStockLocationId>,
    pub status: Option<InventoryItemStatus>,
    pub low_stock: Option<bool>,
    pub q: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListConsumptionEventsQuery {
    pub branch_scope: BranchScope,
    pub item_id: InventoryItemId,
    pub source_kind: Option<String>,
    pub work_order_id: Option<WorkOrderId>,
    pub dispatch_id: Option<P1DispatchId>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateStockLocationCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub branch_id: BranchId,
    pub site_id: Option<SiteId>,
    pub location_code: Option<String>,
    pub label: String,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInventoryItemCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub branch_id: BranchId,
    pub stock_location_id: InventoryStockLocationId,
    pub sku: Option<String>,
    pub display_name: String,
    pub description: Option<String>,
    pub unit_code: String,
    pub quantity_on_hand_milli: i64,
    pub safety_stock_milli: i64,
    pub unit_cost_won: Option<i64>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInventoryItemFields {
    pub sku: Option<Option<String>>,
    pub display_name: Option<String>,
    pub description: Option<Option<String>>,
    pub safety_stock_milli: Option<i64>,
    pub status: Option<InventoryItemStatus>,
}

impl UpdateInventoryItemFields {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sku.is_none()
            && self.display_name.is_none()
            && self.description.is_none()
            && self.safety_stock_milli.is_none()
            && self.status.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInventoryItemCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub item_id: InventoryItemId,
    pub fields: UpdateInventoryItemFields,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConsumeInventorySource {
    WorkOrder { work_order_id: WorkOrderId },
    P1Dispatch { dispatch_id: P1DispatchId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsumeInventoryCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub item_id: InventoryItemId,
    pub source: ConsumeInventorySource,
    pub quantity_consumed_milli: i64,
    pub occurred_at: Option<Timestamp>,
    pub memo: Option<String>,
    pub idempotency_key: String,
    pub trace: TraceContext,
    pub requested_at: Timestamp,
}

/// Source document behind one unified-ledger movement row, tagged for the
/// console: WO/dispatch drills navigate, cycle counts drill to the count, and
/// external refs (e.g. `PO-118`) render as validated non-navigable codes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum MovementSourceView {
    WorkOrder {
        work_order_id: WorkOrderId,
    },
    P1Dispatch {
        dispatch_id: P1DispatchId,
        work_order_id: WorkOrderId,
    },
    CycleCount {
        cycle_count_id: Uuid,
        cc_code: String,
    },
    ExternalRef {
        source_ref: Option<String>,
    },
}

/// One row of the unified movement ledger
/// (`ISSUE` ∪ `RECEIPT` ∪ `ADJUSTMENT`, newest first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryMovementView {
    pub id: Uuid,
    pub item_id: InventoryItemId,
    pub iv_code: String,
    pub kind: MovementKind,
    /// Signed: `ISSUE` negative, `RECEIPT` positive, `ADJUSTMENT` either.
    pub quantity_delta_milli: i64,
    pub quantity_before_milli: i64,
    pub quantity_after_milli: i64,
    pub source: MovementSourceView,
    pub actor: UserId,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: Timestamp,
    pub memo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryReceiptResult {
    pub movement: InventoryMovementView,
    pub item: InventoryItemView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListMovementsQuery {
    pub branch_scope: BranchScope,
    pub item_id: InventoryItemId,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordReceiptCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub item_id: InventoryItemId,
    pub quantity_received_milli: i64,
    /// Validated external document code (`^[A-Z]{1,4}-[A-Z0-9-]{1,40}$`),
    /// e.g. `PO-118`. Stored as text — no purchase module exists yet.
    pub source_ref: Option<String>,
    pub memo: Option<String>,
    pub idempotency_key: String,
    pub trace: TraceContext,
    pub requested_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CycleCountView {
    pub id: Uuid,
    pub cc_code: String,
    pub branch_id: BranchId,
    pub stock_location: InventoryStockLocationSummary,
    pub status: CycleCountStatus,
    pub version: i32,
    pub opened_by: UserId,
    pub submitted_by: Option<UserId>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub submitted_at: Option<Timestamp>,
    pub decided_by: Option<UserId>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<Timestamp>,
    pub decision_memo: Option<String>,
    pub line_count: i64,
    pub variance_line_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CycleCountLineView {
    pub id: Uuid,
    pub item_id: InventoryItemId,
    pub iv_code: String,
    pub display_name: String,
    pub unit_code: String,
    /// On-hand snapshot taken when the line was recorded.
    pub system_quantity_milli: i64,
    pub counted_quantity_milli: i64,
    /// `counted − system` (DB-generated).
    pub variance_milli: i64,
    pub reason: Option<VarianceReason>,
    pub note: Option<String>,
    pub recorded_by: UserId,
    #[serde(with = "time::serde::rfc3339")]
    pub recorded_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CycleCountDetail {
    pub count: CycleCountView,
    pub lines: Vec<CycleCountLineView>,
    /// Adjustment movements created by approval — audit lineage.
    pub applied_movement_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CycleCountPage {
    pub items: Vec<CycleCountView>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListCycleCountsQuery {
    pub branch_scope: BranchScope,
    pub branch_id: BranchId,
    pub status: Option<CycleCountStatus>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCycleCountCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub branch_id: BranchId,
    pub stock_location_id: InventoryStockLocationId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpsertCountLineCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub count_id: Uuid,
    /// Aggregate cycle-count version observed by the editor.
    pub expected_version: i32,
    pub item_id: InventoryItemId,
    pub counted_quantity_milli: i64,
    /// Required iff counted differs from the system snapshot.
    pub reason: Option<VarianceReason>,
    pub note: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitCycleCountCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub count_id: Uuid,
    pub expected_version: i32,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CycleCountDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecideCycleCountCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub count_id: Uuid,
    pub expected_version: i32,
    pub decision: CycleCountDecision,
    /// Required on `Reject`.
    pub memo: Option<String>,
    /// Required on `Approve` (adjustments are idempotent by this key).
    pub idempotency_key: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelCycleCountCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub count_id: Uuid,
    pub expected_version: i32,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Deterministic coverage projection, computed on read from the movement
/// ledger. `inbound_expected_milli`/`reserved_outbound_milli` are honestly 0
/// until purchase/reservation modules exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MrpLineView {
    pub item_id: InventoryItemId,
    pub iv_code: String,
    pub display_name: String,
    pub unit_code: String,
    pub quantity_on_hand_milli: i64,
    pub safety_stock_milli: i64,
    pub inbound_expected_milli: i64,
    pub reserved_outbound_milli: i64,
    /// Trailing-90-day `ISSUE` sum ÷ 3 (floor).
    pub monthly_usage_milli: i64,
    /// `(on_hand + inbound − reserved) × 100 ÷ monthly`; `None` when monthly
    /// usage is 0 (no consumption in the window — coverage is undefined).
    pub cover_months_centi: Option<i64>,
    /// `on_hand < safety_stock`.
    pub short: bool,
    /// `max(0, safety + monthly − (on_hand + inbound − reserved))` when short.
    pub proposed_order_milli: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MrpQuery {
    pub branch_scope: BranchScope,
    pub branch_id: BranchId,
}

pub fn inventory_audit_event(
    action: &str,
    actor: Option<UserId>,
    branch_id: Option<BranchId>,
    target_type: &str,
    target_id: impl ToString,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    let mut event = AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        target_type,
        target_id.to_string(),
        trace,
        occurred_at,
    );
    if let Some(branch_id) = branch_id {
        event = event.with_branch(branch_id);
    }
    Ok(event)
}
