//! Financial application layer.
//!
//! Commands, summaries, and audit-event builders live here. Persistence and
//! HTTP concerns remain in outer crates.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_financial_domain::{
    AcquisitionBasis, DepreciationMethod, FinancialConfig, MoneyInput, PurchaseStatus, QuoteLine,
};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, EquipmentId, EvidenceId, KernelError, PurchaseRequestId,
    QuoteId, Timestamp, TraceContext, UserId, WorkOrderId,
};
use serde::{Deserialize, Serialize};
use time::Date;

/// Serialize/deserialize `Option<Date>` as an ISO-8601 `"YYYY-MM-DD"` string (or
/// `null`), so the wire contract matches the OpenAPI `format: date` and the
/// generated typed clients. `time`'s built-in `serde::iso8601` only covers
/// date-times, so calendar dates need this small helper.
mod iso_date_opt {
    use serde::{Deserialize, Deserializer, Serializer};
    use time::Date;
    use time::format_description::well_known::Iso8601;

    pub fn serialize<S: Serializer>(
        value: &Option<Date>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(date) => {
                let formatted = date
                    .format(&Iso8601::DATE)
                    .map_err(serde::ser::Error::custom)?;
                serializer.serialize_some(&formatted)
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Date>, D::Error> {
        let raw: Option<String> = Option::deserialize(deserializer)?;
        match raw {
            Some(value) => Date::parse(&value, &Iso8601::DATE)
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinancialConfigSnapshot {
    pub depreciation_method: DepreciationMethod,
    pub useful_life_months: u32,
    pub residual_rate_bps: i32,
    pub declining_balance_rate_bps: i32,
    pub management_fee_rate_bps: i32,
    pub profit_rate_bps: i32,
    pub floor_negative_quote_residual: bool,
    pub executive_approval_threshold_won: i64,
}

impl FinancialConfigSnapshot {
    #[must_use]
    pub fn quote_config(&self) -> FinancialConfig {
        FinancialConfig {
            depreciation_method: self.depreciation_method,
            useful_life_months: self.useful_life_months,
            residual_rate_bps: self.residual_rate_bps,
            declining_balance_rate_bps: self.declining_balance_rate_bps,
            management_fee_rate_bps: self.management_fee_rate_bps,
            profit_rate_bps: self.profit_rate_bps,
            floor_negative_quote_residual: self.floor_negative_quote_residual,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRentalQuoteCommand {
    pub actor: UserId,
    pub branch_id: BranchId,
    pub equipment_id: EquipmentId,
    pub config: FinancialConfigSnapshot,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CostLedgerSource {
    ManualAdmin,
    PurchaseExecution,
}

impl CostLedgerSource {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::ManualAdmin => "MANUAL_ADMIN",
            Self::PurchaseExecution => "PURCHASE_EXECUTION",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "MANUAL_ADMIN" => Ok(Self::ManualAdmin),
            "PURCHASE_EXECUTION" => Ok(Self::PurchaseExecution),
            other => Err(KernelError::validation(format!(
                "unknown cost ledger source {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppendCostLedgerEntryCommand {
    pub actor: UserId,
    pub branch_id: BranchId,
    pub equipment_id: EquipmentId,
    pub work_order_id: Option<WorkOrderId>,
    pub source: CostLedgerSource,
    pub amount_won: i64,
    pub memo: String,
    pub config: FinancialConfigSnapshot,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PurchaseType {
    Regular,
    OneOff,
    Other,
    LegacyManual,
}

impl PurchaseType {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Regular => "REGULAR",
            Self::OneOff => "ONE_OFF",
            Self::Other => "OTHER",
            Self::LegacyManual => "LEGACY_MANUAL",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, console_kernel_core::KernelError> {
        match value {
            "REGULAR" => Ok(Self::Regular),
            "ONE_OFF" => Ok(Self::OneOff),
            "OTHER" => Ok(Self::Other),
            "LEGACY_MANUAL" => Ok(Self::LegacyManual),
            other => Err(console_kernel_core::KernelError::validation(format!(
                "unknown purchase type {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseRequestLineInput {
    pub item: String,
    pub quantity: i32,
    pub unit_supply_price_won: i64,
    pub vat_won: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseRequestLineSummary {
    pub id: uuid::Uuid,
    pub line_no: i32,
    pub item: String,
    pub quantity: i32,
    pub unit_supply_price_won: i64,
    pub vat_won: i64,
    pub vat_overridden: bool,
    pub line_total_won: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseRequesterSummary {
    pub user_id: UserId,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseAttachmentSummary {
    pub id: uuid::Uuid,
    pub file_name: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub role: String,
    pub download_url: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparePurchaseAttachmentUploadCommand {
    pub actor: UserId,
    pub branch_id: BranchId,
    pub file_name: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub checksum_sha256: Option<String>,
    pub role: String,
    pub s3_bucket: String,
    pub s3_key: String,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseAttachmentUploadRecord {
    pub id: uuid::Uuid,
    pub branch_id: BranchId,
    pub file_name: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub role: String,
    pub upload_state: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmPurchaseAttachmentUploadCommand {
    pub actor: UserId,
    pub attachment_id: uuid::Uuid,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseAttachmentDownload {
    pub file_name: String,
    pub content_type: String,
    pub s3_bucket: String,
    pub s3_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseFeaturePreferences {
    pub feature_key: String,
    pub schema_version: i32,
    pub preferences: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchasePolicySummary {
    pub equipment_required: bool,
    pub statement_evidence_required: bool,
    pub price_anomaly: bool,
    pub quote_update_required: bool,
    pub submit_blocked: bool,
    pub messages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePurchaseRequestCommand {
    pub actor: UserId,
    pub branch_id: BranchId,
    pub equipment_id: Option<EquipmentId>,
    pub work_order_id: Option<WorkOrderId>,
    pub statement_evidence_id: Option<EvidenceId>,
    pub purchase_type: PurchaseType,
    pub vendor_name: String,
    pub amount_won: Option<i64>,
    pub lines: Vec<PurchaseRequestLineInput>,
    pub quote_attachment_ids: Vec<uuid::Uuid>,
    pub memo: String,
    pub config: FinancialConfigSnapshot,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseSubmitCommand {
    pub actor: UserId,
    pub purchase_request_id: PurchaseRequestId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseApprovalCommand {
    pub actor: UserId,
    pub purchase_request_id: PurchaseRequestId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareExpenditureCommand {
    pub actor: UserId,
    pub purchase_request_id: PurchaseRequestId,
    pub expenditure_no: String,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectPurchaseCommand {
    pub actor: UserId,
    pub purchase_request_id: PurchaseRequestId,
    pub memo: String,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseRestartCommand {
    pub actor: UserId,
    pub purchase_request_id: PurchaseRequestId,
    pub statement_evidence_id: Option<EvidenceId>,
    pub amount_won: Option<i64>,
    pub lines: Vec<PurchaseRequestLineInput>,
    pub quote_attachment_ids: Vec<uuid::Uuid>,
    pub memo: String,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutePurchaseCommand {
    pub actor: UserId,
    pub purchase_request_id: PurchaseRequestId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RentalQuoteSummary {
    pub id: QuoteId,
    pub branch_id: BranchId,
    pub equipment_id: EquipmentId,
    pub acquisition_value: MoneyInput,
    pub current_residual_value: MoneyInput,
    pub effective_residual_value: MoneyInput,
    pub residual_was_floored: bool,
    pub cumulative_repair_cost: MoneyInput,
    pub monthly_total: MoneyInput,
    pub lines: Vec<QuoteLine>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostLedgerEntrySummary {
    pub id: uuid::Uuid,
    pub branch_id: BranchId,
    pub equipment_id: EquipmentId,
    pub work_order_id: Option<WorkOrderId>,
    pub purchase_request_id: Option<PurchaseRequestId>,
    pub source: CostLedgerSource,
    pub amount_won: i64,
    pub memo: String,
    pub residual_before_won: i64,
    pub residual_after_won: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub entry_at: Timestamp,
}

/// Per-asset lifecycle / total-cost-of-ownership rollup.
///
/// Answers, for one asset: acquired for X, maintenance cost Y, sold for Z ->
/// TCO and gross margin, plus per-month and per-hour maintenance intensity.
///
/// `outsource_unlinked_won` is surfaced READ-ONLY for visibility and is
/// DELIBERATELY excluded from `tco_won` (double-count guard): outsource cost
/// lives on a separate column, not in the cost ledger, so summing it would
/// double-count work already captured as ledger entries. The acquisition leg of
/// `tco_won` is `acquisition_cost_won` when present, else `vehicle_value` (the
/// depreciation base) as a tagged fallback, added exactly once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetLifecycleCostSummary {
    pub equipment_id: EquipmentId,
    pub equipment_no: String,
    /// The Korean asset status code ('임대', '예비', '폐기', '대체', '매각').
    pub status: String,
    pub acquisition_cost_won: Option<i64>,
    #[serde(with = "iso_date_opt")]
    pub acquisition_date: Option<Date>,
    /// Where the acquisition leg of `tco_won` came from.
    pub acquisition_source: AcquisitionBasis,
    /// Σ of every cost-ledger entry on this asset (all sources).
    pub maintenance_total_won: i64,
    pub manual_total_won: i64,
    pub purchase_total_won: i64,
    /// Number of cost-ledger entries summed into `maintenance_total_won`.
    pub entry_count: i64,
    /// Read-only outsource cost (NOT part of `tco_won`).
    pub outsource_unlinked_won: Option<i64>,
    pub residual_value_won: i64,
    /// Latest realized sale price for a SOLD listing, if any.
    pub sale_price_won: Option<i64>,
    #[serde(with = "iso_date_opt")]
    pub sold_at: Option<Date>,
    /// `sale_price_won − tco_won`; `None` until sold (loss allowed, never floored).
    pub gross_margin_won: Option<i64>,
    pub tco_won: i64,
    pub cost_per_month_won: Option<i64>,
    pub cost_per_hour_won: Option<i64>,
    pub timeline: Vec<CostLedgerEntrySummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseRequestSummary {
    pub id: PurchaseRequestId,
    pub branch_id: BranchId,
    pub equipment_id: Option<EquipmentId>,
    pub work_order_id: Option<WorkOrderId>,
    pub statement_evidence_id: Option<EvidenceId>,
    pub purchase_type: PurchaseType,
    pub vendor_name: String,
    pub amount_won: i64,
    pub status: PurchaseStatus,
    pub requester: PurchaseRequesterSummary,
    pub lines: Vec<PurchaseRequestLineSummary>,
    pub quote_attachments: Vec<PurchaseAttachmentSummary>,
    pub policy: PurchasePolicySummary,
    pub expenditure_no: Option<String>,
    pub rejection_memo: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: Timestamp,
}

/// A branch-scoped, server-normalized purchase-request collection query.
///
/// `requester_id` is set by the REST authorization boundary when a caller may
/// only read requests that they submitted themselves.  It is deliberately an
/// application query rather than a client-side filter so disallowed rows never
/// leave Postgres.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPurchaseRequestsQuery {
    pub branch_id: BranchId,
    pub statuses: Vec<PurchaseStatus>,
    pub requester_id: Option<UserId>,
    pub limit: i64,
    pub offset: i64,
}

/// Stable offset page for the purchase-request queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseRequestPage {
    pub items: Vec<PurchaseRequestSummary>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

pub fn financial_audit_event(
    action: &str,
    actor: UserId,
    branch_id: BranchId,
    target_type: &str,
    target_id: impl ToString,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new(action)?,
        target_type,
        target_id.to_string(),
        trace,
        occurred_at,
    )
    .with_branch(branch_id))
}
