//! Pure inventory domain for IV items, stock thresholds, and consumption.
//!
//! This crate owns only value objects and invariants. It deliberately has no
//! SQLx, REST, authz, workorder, dispatch, or ERP dependency so the layer-boundary
//! gate can prove inventory's operational stock model is independent.
#![cfg_attr(test, allow(clippy::unwrap_used))]

use console_kernel_core::{
    BranchId, InventoryItemId, InventoryStockLocationId, KernelError, P1DispatchId, WorkOrderId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InventoryItemStatus {
    Active,
    Archived,
}

impl InventoryItemStatus {
    /// # Errors
    /// Returns `KernelError::validation` for an unknown database value.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "ACTIVE" => Ok(Self::Active),
            "ARCHIVED" => Ok(Self::Archived),
            other => Err(KernelError::validation(format!(
                "unknown inventory item status {other:?}"
            ))),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Archived => "ARCHIVED",
        }
    }

    #[must_use]
    pub const fn can_consume(self) -> bool {
        matches!(self, Self::Active)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CycleCountStatus {
    Draft,
    Submitted,
    Approved,
    Rejected,
    Cancelled,
}
impl CycleCountStatus {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Submitted => "SUBMITTED",
            Self::Approved => "APPROVED",
            Self::Rejected => "REJECTED",
            Self::Cancelled => "CANCELLED",
        }
    }
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "DRAFT" => Ok(Self::Draft),
            "SUBMITTED" => Ok(Self::Submitted),
            "APPROVED" => Ok(Self::Approved),
            "REJECTED" => Ok(Self::Rejected),
            "CANCELLED" => Ok(Self::Cancelled),
            other => Err(KernelError::validation(format!(
                "unknown cycle count status {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MovementKind {
    /// Legacy consumption ledger projection; never persisted in inventory_movements.
    Issue,
    Receipt,
    Adjustment,
}
impl MovementKind {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Issue => "ISSUE",
            Self::Receipt => "RECEIPT",
            Self::Adjustment => "ADJUSTMENT",
        }
    }
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "ISSUE" => Ok(Self::Issue),
            "RECEIPT" => Ok(Self::Receipt),
            "ADJUSTMENT" => Ok(Self::Adjustment),
            other => Err(KernelError::validation(format!(
                "unknown inventory movement kind {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VarianceReason {
    Damage,
    Loss,
    Miscount,
    Found,
    Other,
}
impl VarianceReason {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Damage => "DAMAGE",
            Self::Loss => "LOSS",
            Self::Miscount => "MISCOUNT",
            Self::Found => "FOUND",
            Self::Other => "OTHER",
        }
    }
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "DAMAGE" => Ok(Self::Damage),
            "LOSS" => Ok(Self::Loss),
            "MISCOUNT" => Ok(Self::Miscount),
            "FOUND" => Ok(Self::Found),
            "OTHER" => Ok(Self::Other),
            other => Err(KernelError::validation(format!(
                "unknown inventory variance reason {other:?}"
            ))),
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct QuantityMilli(i64);

impl QuantityMilli {
    /// # Errors
    /// Returns `KernelError::validation` when the quantity is negative.
    pub fn new(value: i64) -> Result<Self, KernelError> {
        if value < 0 {
            Err(KernelError::validation(
                "inventory quantity_milli must be non-negative",
            ))
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn value(self) -> i64 {
        self.0
    }

    #[must_use]
    pub const fn is_low_stock(self, safety_stock: SafetyStockMilli) -> bool {
        self.0 <= safety_stock.value()
    }
}

impl TryFrom<i64> for QuantityMilli {
    type Error = KernelError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct SafetyStockMilli(i64);

impl SafetyStockMilli {
    /// # Errors
    /// Returns `KernelError::validation` when the threshold is negative.
    pub fn new(value: i64) -> Result<Self, KernelError> {
        if value < 0 {
            Err(KernelError::validation(
                "inventory safety_stock_milli must be non-negative",
            ))
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn value(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for SafetyStockMilli {
    type Error = KernelError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct PositiveQuantityMilli(i64);

impl PositiveQuantityMilli {
    /// # Errors
    /// Returns `KernelError::validation` when the quantity is zero or negative.
    pub fn new(value: i64) -> Result<Self, KernelError> {
        if value <= 0 {
            Err(KernelError::validation(
                "inventory consumed quantity_milli must be positive",
            ))
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn value(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for PositiveQuantityMilli {
    type Error = KernelError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct MoneyWon(i64);

impl MoneyWon {
    /// # Errors
    /// Returns `KernelError::validation` when the amount is negative.
    pub fn new(value: i64) -> Result<Self, KernelError> {
        if value < 0 {
            Err(KernelError::validation("money_won must be non-negative"))
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn value(self) -> i64 {
        self.0
    }

    #[must_use]
    pub fn cost_for_quantity(self, quantity: PositiveQuantityMilli) -> Option<Self> {
        let milli_cost = self.0.checked_mul(quantity.value())?;
        Self::new(milli_cost / 1_000).ok()
    }
}

impl TryFrom<i64> for MoneyWon {
    type Error = KernelError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct UnitCode(String);

impl UnitCode {
    /// # Errors
    /// Returns `KernelError::validation` when the unit code is not a compact
    /// uppercase code like `EA`, `BOX`, `M`, or `L`.
    pub fn new(raw: impl Into<String>) -> Result<Self, KernelError> {
        let value = raw.into().trim().to_ascii_uppercase();
        let mut chars = value.chars();
        let Some(first) = chars.next() else {
            return Err(KernelError::validation("unit_code is required"));
        };
        let valid = first.is_ascii_uppercase()
            && value.len() <= 16
            && chars.all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_');
        if valid {
            Ok(Self(value))
        } else {
            Err(KernelError::validation(
                "unit_code must match ^[A-Z][A-Z0-9_]{0,15}$",
            ))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct InventoryCode(String);

impl InventoryCode {
    /// # Errors
    /// Returns `KernelError::validation` unless the code is canonical `IV-...`.
    pub fn new(raw: impl Into<String>) -> Result<Self, KernelError> {
        let value = raw.into().trim().to_ascii_uppercase();
        let Some(suffix) = value.strip_prefix("IV-") else {
            return Err(KernelError::validation(
                "inventory code must start with IV-",
            ));
        };
        let valid_len = (3..=40).contains(&suffix.len());
        let valid_chars = suffix
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '-');
        if valid_len && valid_chars {
            Ok(Self(value))
        } else {
            Err(KernelError::validation(
                "inventory code must match ^IV-[A-Z0-9-]{3,40}$",
            ))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for InventoryCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InventoryConsumptionSource {
    WorkOrder {
        work_order_id: WorkOrderId,
    },
    P1Dispatch {
        dispatch_id: P1DispatchId,
        work_order_id: WorkOrderId,
    },
}

impl InventoryConsumptionSource {
    #[must_use]
    pub const fn kind_db_str(self) -> &'static str {
        match self {
            Self::WorkOrder { .. } => "WORK_ORDER",
            Self::P1Dispatch { .. } => "P1_DISPATCH",
        }
    }

    #[must_use]
    pub const fn work_order_id(self) -> WorkOrderId {
        match self {
            Self::WorkOrder { work_order_id } | Self::P1Dispatch { work_order_id, .. } => {
                work_order_id
            }
        }
    }

    #[must_use]
    pub const fn dispatch_id(self) -> Option<P1DispatchId> {
        match self {
            Self::WorkOrder { .. } => None,
            Self::P1Dispatch { dispatch_id, .. } => Some(dispatch_id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryItemState {
    pub item_id: InventoryItemId,
    pub branch_id: BranchId,
    pub stock_location_id: InventoryStockLocationId,
    pub status: InventoryItemStatus,
    pub quantity_on_hand_milli: QuantityMilli,
    pub safety_stock_milli: SafetyStockMilli,
    pub unit_cost_won: Option<MoneyWon>,
}

impl InventoryItemState {
    #[must_use]
    pub const fn new(
        item_id: InventoryItemId,
        branch_id: BranchId,
        stock_location_id: InventoryStockLocationId,
        status: InventoryItemStatus,
        quantity_on_hand_milli: QuantityMilli,
        safety_stock_milli: SafetyStockMilli,
        unit_cost_won: Option<MoneyWon>,
    ) -> Self {
        Self {
            item_id,
            branch_id,
            stock_location_id,
            status,
            quantity_on_hand_milli,
            safety_stock_milli,
            unit_cost_won,
        }
    }

    /// # Errors
    /// Rejects archived items and any consumption that would drive stock below 0.
    pub fn consume(
        self,
        quantity: PositiveQuantityMilli,
    ) -> Result<InventoryConsumptionOutcome, KernelError> {
        if !self.status.can_consume() {
            return Err(KernelError::conflict(
                "archived inventory items cannot be consumed",
            ));
        }
        let after = self
            .quantity_on_hand_milli
            .value()
            .checked_sub(quantity.value())
            .ok_or_else(|| {
                KernelError::conflict("inventory consumption would make stock negative")
            })?;
        Ok(InventoryConsumptionOutcome {
            quantity_before_milli: self.quantity_on_hand_milli,
            quantity_consumed_milli: quantity,
            quantity_after_milli: QuantityMilli::new(after)?,
            low_stock_after: QuantityMilli::new(after)?.is_low_stock(self.safety_stock_milli),
            cost_won: self
                .unit_cost_won
                .and_then(|unit_cost| unit_cost.cost_for_quantity(quantity)),
        })
    }

    #[must_use]
    pub const fn low_stock(self) -> bool {
        self.quantity_on_hand_milli
            .is_low_stock(self.safety_stock_milli)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryConsumptionOutcome {
    pub quantity_before_milli: QuantityMilli,
    pub quantity_consumed_milli: PositiveQuantityMilli,
    pub quantity_after_milli: QuantityMilli,
    pub low_stock_after: bool,
    pub cost_won: Option<MoneyWon>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_item(quantity: i64) -> InventoryItemState {
        InventoryItemState::new(
            InventoryItemId::new(),
            BranchId::new(),
            InventoryStockLocationId::new(),
            InventoryItemStatus::Active,
            QuantityMilli::new(quantity).unwrap(),
            SafetyStockMilli::new(5_000).unwrap(),
            Some(MoneyWon::new(1_200).unwrap()),
        )
    }

    #[test]
    fn cycle_count_movement_and_variance_wires_match_0191_constraints() {
        for (status, wire) in [
            (CycleCountStatus::Draft, "DRAFT"),
            (CycleCountStatus::Submitted, "SUBMITTED"),
            (CycleCountStatus::Approved, "APPROVED"),
            (CycleCountStatus::Rejected, "REJECTED"),
            (CycleCountStatus::Cancelled, "CANCELLED"),
        ] {
            assert_eq!(status.as_db_str(), wire);
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{wire}\"")
            );
            assert_eq!(CycleCountStatus::parse(wire).unwrap(), status);
        }
        for (kind, wire) in [
            (MovementKind::Issue, "ISSUE"),
            (MovementKind::Receipt, "RECEIPT"),
            (MovementKind::Adjustment, "ADJUSTMENT"),
        ] {
            assert_eq!(kind.as_db_str(), wire);
            assert_eq!(serde_json::to_string(&kind).unwrap(), format!("\"{wire}\""));
            assert_eq!(MovementKind::parse(wire).unwrap(), kind);
        }
        for (reason, wire) in [
            (VarianceReason::Damage, "DAMAGE"),
            (VarianceReason::Loss, "LOSS"),
            (VarianceReason::Miscount, "MISCOUNT"),
            (VarianceReason::Found, "FOUND"),
            (VarianceReason::Other, "OTHER"),
        ] {
            assert_eq!(reason.as_db_str(), wire);
            assert_eq!(
                serde_json::to_string(&reason).unwrap(),
                format!("\"{wire}\"")
            );
            assert_eq!(VarianceReason::parse(wire).unwrap(), reason);
        }
        assert!(CycleCountStatus::parse("VOID").is_err());
        assert!(MovementKind::parse("TRANSFER").is_err());
        assert!(VarianceReason::parse("THEFT").is_err());
        assert!(serde_json::from_str::<CycleCountStatus>("\"VOID\"").is_err());
        assert!(serde_json::from_str::<MovementKind>("\"TRANSFER\"").is_err());
        assert!(serde_json::from_str::<VarianceReason>("\"THEFT\"").is_err());
    }

    #[test]
    fn quantity_and_safety_stock_reject_negative_values() {
        assert!(QuantityMilli::new(-1).is_err());
        assert!(SafetyStockMilli::new(-1).is_err());
        assert!(MoneyWon::new(-1).is_err());
    }

    #[test]
    fn positive_consumption_rejects_zero_or_negative() {
        assert!(PositiveQuantityMilli::new(0).is_err());
        assert!(PositiveQuantityMilli::new(-10).is_err());
        assert_eq!(PositiveQuantityMilli::new(1).unwrap().value(), 1);
    }

    #[test]
    fn consumption_rejects_archived_item() {
        let item = InventoryItemState {
            status: InventoryItemStatus::Archived,
            ..active_item(10_000)
        };

        assert!(
            item.consume(PositiveQuantityMilli::new(1_000).unwrap())
                .is_err()
        );
    }

    #[test]
    fn consumption_rejects_stock_going_negative() {
        let item = active_item(1_000);

        assert!(
            item.consume(PositiveQuantityMilli::new(2_000).unwrap())
                .is_err()
        );
    }

    #[test]
    fn consumption_tracks_before_after_low_stock_and_cost() {
        let item = active_item(12_000);
        let outcome = item
            .consume(PositiveQuantityMilli::new(8_000).unwrap())
            .unwrap();

        assert_eq!(outcome.quantity_before_milli.value(), 12_000);
        assert_eq!(outcome.quantity_after_milli.value(), 4_000);
        assert!(outcome.low_stock_after);
        assert_eq!(outcome.cost_won.unwrap().value(), 9_600);
    }

    #[test]
    fn dispatch_source_carries_dispatch_and_derived_work_order() {
        let dispatch_id = P1DispatchId::new();
        let work_order_id = WorkOrderId::new();
        let source = InventoryConsumptionSource::P1Dispatch {
            dispatch_id,
            work_order_id,
        };

        assert_eq!(source.dispatch_id(), Some(dispatch_id));
        assert_eq!(source.work_order_id(), work_order_id);
        assert_eq!(source.kind_db_str(), "P1_DISPATCH");
    }

    #[test]
    fn code_and_unit_are_canonicalized() {
        assert_eq!(
            InventoryCode::new("iv-abc-123").unwrap().as_str(),
            "IV-ABC-123"
        );
        assert_eq!(UnitCode::new("ea").unwrap().as_str(), "EA");
        assert!(InventoryCode::new("WO-1").is_err());
        assert!(UnitCode::new("box size").is_err());
    }
}
