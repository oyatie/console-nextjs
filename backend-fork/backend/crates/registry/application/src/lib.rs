//! Registry application layer.
//!
//! Use-case DTOs and audit event builders live here. Concrete workbook and
//! database adapters remain in outer crates.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, CustomerId, EquipmentId,
    EquipmentSubstitutionId, KernelError, SiteId, Timestamp, TraceContext, UserId, WorkOrderId,
};
use console_registry_domain::{EquipmentNo, EquipmentStatus, MoneyWon, SubstituteMatchKind, Ton};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportSheet {
    Master,
    Reserve,
}

impl ImportSheet {
    #[must_use]
    pub const fn workbook_name(self) -> &'static str {
        match self {
            Self::Master => "K&L 지게차 Master list",
            Self::Reserve => "예비 및 여유차량",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MasterListEquipment {
    pub source_sheet: ImportSheet,
    pub source_row: u32,
    pub equipment_no: EquipmentNo,
    pub management_no: Option<String>,
    pub manufacturer_code: String,
    pub kind_code: String,
    pub power_code: String,
    pub power_label: Option<String>,
    pub customer_name: String,
    pub site_name: String,
    pub status: EquipmentStatus,
    pub manager_name: Option<String>,
    pub placement_location: Option<String>,
    pub placement_no: Option<String>,
    pub operation_shift: Option<String>,
    pub specification: String,
    pub ton: Ton,
    pub maker: Option<String>,
    pub model: Option<String>,
    pub vin: Option<String>,
    pub year: Option<Date>,
    pub hours: Option<i64>,
    pub vehicle_registration_no: Option<String>,
    pub insured: Option<bool>,
    pub insurer: Option<String>,
    pub policy_holder: Option<String>,
    pub insured_party: Option<String>,
    pub asset_owner: Option<String>,
    pub asset_registered_on: Option<Date>,
    pub rental_started_on: Option<Date>,
    pub rental_fee: Option<MoneyWon>,
    pub vehicle_value: Option<MoneyWon>,
    pub residual_value: Option<MoneyWon>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RegistryRowError {
    pub sheet: String,
    pub row: u32,
    pub message: String,
}

impl RegistryRowError {
    #[must_use]
    pub fn new(sheet: impl Into<String>, row: u32, message: impl Into<String>) -> Self {
        Self {
            sheet: sheet.into(),
            row,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ParsedMasterList {
    pub input_rows: usize,
    pub prefix_checked_rows: usize,
    pub equipment: Vec<MasterListEquipment>,
    pub errors: Vec<RegistryRowError>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RegistryImportReport {
    pub input_rows: usize,
    pub equipment_count: usize,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub orphaned: usize,
    pub errors: Vec<RegistryRowError>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SubstituteSearch {
    pub equipment_id: EquipmentId,
    pub branch_scope: BranchScope,
    pub include_all_branches: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SubstituteCandidate {
    pub equipment_id: EquipmentId,
    pub branch_id: BranchId,
    pub equipment_no: EquipmentNo,
    pub management_no: Option<String>,
    pub model: Option<String>,
    pub status: EquipmentStatus,
    pub specification: String,
    pub ton: Ton,
    pub power_code: String,
    pub power_label: Option<String>,
    pub customer_name: String,
    pub site_name: String,
    pub placement_location: Option<String>,
    pub match_kind: SubstituteMatchKind,
    pub ton_delta_milli: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SubstituteAssignmentCommand {
    pub actor: UserId,
    /// Principal-resolved branch scope. Never caller-supplied. The adapter
    /// refuses (as `not_found`) a source or substitute outside it.
    pub branch_scope: BranchScope,
    pub source_equipment_id: EquipmentId,
    pub substitute_equipment_id: EquipmentId,
    pub assigned_to: Option<UserId>,
    pub assignment_location: String,
    pub trace: TraceContext,
    pub assigned_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SubstituteReturnCommand {
    pub actor: UserId,
    /// Principal-resolved branch scope. Never caller-supplied.
    pub branch_scope: BranchScope,
    pub substitution_id: EquipmentSubstitutionId,
    pub trace: TraceContext,
    pub returned_at: Timestamp,
    pub return_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SubstituteAssignment {
    pub id: EquipmentSubstitutionId,
    pub branch_id: BranchId,
    pub source_equipment_id: EquipmentId,
    pub substitute_equipment_id: EquipmentId,
    pub assigned_by: UserId,
    pub assigned_to: Option<UserId>,
    pub assignment_location: String,
    pub assigned_at: Timestamp,
    pub returned_by: Option<UserId>,
    pub returned_at: Option<Timestamp>,
    pub return_note: Option<String>,
}

/// Ordered, auditable legal-owner transfer workflow for equipment assets.
/// The registry row's tenant/org discriminator remains immutable; only the
/// legal-owner fact (`asset_owner`) changes after every required signoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentOwnershipTransferStepKey {
    SendingOrgAdmin,
    ReceivingOrgAdmin,
    LegalSignoff,
    AccountingSignoff,
}

impl EquipmentOwnershipTransferStepKey {
    pub const ORDER: [Self; 4] = [
        Self::SendingOrgAdmin,
        Self::ReceivingOrgAdmin,
        Self::LegalSignoff,
        Self::AccountingSignoff,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SendingOrgAdmin => "sending_org_admin",
            Self::ReceivingOrgAdmin => "receiving_org_admin",
            Self::LegalSignoff => "legal_signoff",
            Self::AccountingSignoff => "accounting_signoff",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SendingOrgAdmin => "이전 법인 승인",
            Self::ReceivingOrgAdmin => "인수 법인 승인",
            Self::LegalSignoff => "법무 소유권 검토",
            Self::AccountingSignoff => "회계 자산대장 반영",
        }
    }
}

impl TryFrom<&str> for EquipmentOwnershipTransferStepKey {
    type Error = KernelError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "sending_org_admin" => Ok(Self::SendingOrgAdmin),
            "receiving_org_admin" => Ok(Self::ReceivingOrgAdmin),
            "legal_signoff" => Ok(Self::LegalSignoff),
            "accounting_signoff" => Ok(Self::AccountingSignoff),
            _ => Err(KernelError::validation(format!(
                "unknown ownership transfer step: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EquipmentOwnershipTransferStatus {
    Pending,
    Approved,
    Rejected,
}

impl EquipmentOwnershipTransferStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Approved => "APPROVED",
            Self::Rejected => "REJECTED",
        }
    }
}

impl TryFrom<&str> for EquipmentOwnershipTransferStatus {
    type Error = KernelError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "PENDING" => Ok(Self::Pending),
            "APPROVED" => Ok(Self::Approved),
            "REJECTED" => Ok(Self::Rejected),
            _ => Err(KernelError::validation(format!(
                "unknown ownership transfer status: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentOwnershipTransferDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentOwnershipTransferStep {
    pub step_key: String,
    pub label: String,
    pub status: String,
    pub decided_by: Option<UserId>,
    pub decided_at: Option<Timestamp>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentOwnershipTransferRequest {
    pub id: Uuid,
    pub equipment_id: EquipmentId,
    pub branch_id: BranchId,
    pub from_owner: String,
    pub to_owner: String,
    pub reason: String,
    pub status: EquipmentOwnershipTransferStatus,
    pub current_step: Option<EquipmentOwnershipTransferStepKey>,
    pub approval_line: Vec<EquipmentOwnershipTransferStep>,
    pub requested_by: Option<UserId>,
    pub requested_at: Timestamp,
    pub decided_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CreateEquipmentOwnershipTransferCommand {
    pub actor: UserId,
    /// Principal-resolved branch scope. Never caller-supplied.
    pub branch_scope: BranchScope,
    pub equipment_id: EquipmentId,
    pub to_owner: String,
    pub reason: String,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DecideEquipmentOwnershipTransferCommand {
    pub actor: UserId,
    /// Principal-resolved branch scope. Never caller-supplied.
    pub branch_scope: BranchScope,
    pub request_id: Uuid,
    pub decision: EquipmentOwnershipTransferDecision,
    pub comment: String,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Fields a caller may set when creating a single equipment master row outside
/// the bulk import path. Mirrors the importer's [`MasterListEquipment`] surface
/// but omits derived prefix codes (recomputed from `equipment_no`) and the
/// import-only `source_sheet`/`source_row` provenance.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CreateEquipmentCommand {
    pub actor: UserId,
    /// Branch to create the equipment under. Branch-scoped admins create on
    /// their own branch so the row is immediately visible in branch-scoped
    /// reads; org-wide principals pass `None` and the adapter falls back to the
    /// tenant default HQ branch.
    pub branch_id: Option<BranchId>,
    pub equipment_no: EquipmentNo,
    pub customer_name: String,
    pub site_name: String,
    pub status: EquipmentStatus,
    pub specification: String,
    pub ton: Ton,
    pub management_no: Option<String>,
    pub power_label: Option<String>,
    pub manager_name: Option<String>,
    pub placement_location: Option<String>,
    pub placement_no: Option<String>,
    pub operation_shift: Option<String>,
    pub maker: Option<String>,
    pub model: Option<String>,
    pub vin: Option<String>,
    pub year: Option<Date>,
    pub hours: Option<i64>,
    pub vehicle_registration_no: Option<String>,
    pub insured: Option<bool>,
    pub insurer: Option<String>,
    pub policy_holder: Option<String>,
    pub insured_party: Option<String>,
    pub asset_owner: Option<String>,
    pub asset_registered_on: Option<Date>,
    pub rental_started_on: Option<Date>,
    pub rental_fee: Option<MoneyWon>,
    pub vehicle_value: Option<MoneyWon>,
    pub residual_value: Option<MoneyWon>,
    pub note: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Create one customer (고객) directly, outside the bulk import path.
///
/// `branch_id` is the branch the new customer lands on: the REST handler passes
/// the caller's own branch for a branch-scoped admin (so the new row is
/// immediately visible to that admin's branch-scoped registry reads), and `None`
/// for an org-wide principal (SUPER_ADMIN/EXECUTIVE), which falls back to the
/// default HQ branch — the same branch the importer and equipment-create use.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CreateCustomerCommand {
    pub actor: UserId,
    pub branch_id: Option<BranchId>,
    pub name: String,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// One newly created customer, returned so the console can show it immediately.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CreatedCustomer {
    pub id: CustomerId,
    pub branch_id: BranchId,
    pub name: String,
}

/// Create one site (현장) under an existing customer, directly. The site lands on
/// the existing customer's branch under `customer_id`; the adapter validates that
/// the customer belongs to the caller's org and branch scope before writing.
/// Optional location/contact fields mirror the PATCH /sites surface so a site can
/// be onboarded with its address and representative contact in one step.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateSiteCommand {
    pub actor: UserId,
    /// Principal-resolved branch scope. Never caller-supplied. A customer in
    /// another branch of the same org is refused as `not_found`.
    pub branch_scope: BranchScope,
    pub customer_id: CustomerId,
    pub name: String,
    pub address: Option<String>,
    pub province: Option<String>,
    pub city: Option<String>,
    pub postal_code: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub geofence_radius_m: Option<f64>,
    pub contact_name: Option<String>,
    pub contact_phone: Option<String>,
    pub contact_email: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// One newly created site, returned so the console can drop it into the list/map
/// without a refetch.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreatedSite {
    pub id: SiteId,
    pub customer_id: CustomerId,
    pub branch_id: BranchId,
    pub name: String,
    pub address: Option<String>,
    pub province: Option<String>,
    pub city: Option<String>,
    pub postal_code: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub geofence_radius_m: Option<f64>,
    pub contact_name: Option<String>,
    pub contact_phone: Option<String>,
    pub contact_email: Option<String>,
}

/// A partial update to one equipment master row. `None` fields are left
/// untouched; `Some` fields are written. Customer/site and financial fields can
/// all be re-targeted here, including the 유효설비 `status` and 취득/잔존가액.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UpdateEquipmentFields {
    pub customer_name: Option<String>,
    pub site_name: Option<String>,
    pub status: Option<EquipmentStatus>,
    pub specification: Option<String>,
    pub ton: Option<Ton>,
    pub management_no: Option<Option<String>>,
    pub power_label: Option<Option<String>>,
    pub manager_name: Option<Option<String>>,
    pub placement_location: Option<Option<String>>,
    pub placement_no: Option<Option<String>>,
    pub operation_shift: Option<Option<String>>,
    pub maker: Option<Option<String>>,
    pub model: Option<Option<String>>,
    pub vin: Option<Option<String>>,
    pub year: Option<Option<Date>>,
    pub hours: Option<Option<i64>>,
    pub vehicle_registration_no: Option<Option<String>>,
    pub insured: Option<Option<bool>>,
    pub insurer: Option<Option<String>>,
    pub policy_holder: Option<Option<String>>,
    pub insured_party: Option<Option<String>>,
    pub asset_owner: Option<Option<String>>,
    pub asset_registered_on: Option<Option<Date>>,
    pub rental_started_on: Option<Option<Date>>,
    pub rental_fee: Option<Option<MoneyWon>>,
    pub vehicle_value: Option<Option<MoneyWon>>,
    pub residual_value: Option<Option<MoneyWon>>,
    /// Acquisition cost (취득원가): a distinct accounting fact, never the
    /// depreciation base. Independent of `vehicle_value`; never feeds the
    /// residual engine.
    pub acquisition_cost_won: Option<Option<MoneyWon>>,
    pub acquisition_date: Option<Option<Date>>,
    pub note: Option<Option<String>>,
}

impl UpdateEquipmentFields {
    /// True when no field would be written, so the adapter can reject empty
    /// patches with a validation error instead of opening a transaction.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UpdateEquipmentCommand {
    pub actor: UserId,
    /// Principal-resolved branch scope. Never caller-supplied. A row outside it
    /// is refused as `not_found`, exactly like [`UpdateSiteCommand`].
    pub branch_scope: BranchScope,
    pub equipment_id: EquipmentId,
    pub fields: UpdateEquipmentFields,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Roll an equipment row back to a prior captured version's content. The
/// rollback lands as a NEW version (`ROLLBACK`, `source_version = version`);
/// history is never rewritten.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RollbackEquipmentCommand {
    pub actor: UserId,
    /// Principal-resolved branch scope. Never caller-supplied.
    pub branch_scope: BranchScope,
    pub equipment_id: EquipmentId,
    pub version: i32,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeleteEquipmentCommand {
    pub actor: UserId,
    /// Principal-resolved branch scope. Never caller-supplied.
    pub branch_scope: BranchScope,
    pub equipment_id: EquipmentId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Sort column for the paginated equipment list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentSortBy {
    /// 호기 번호 ascending (default).
    #[default]
    EquipmentNo,
    /// Model name ascending.
    Model,
    /// Customer name ascending.
    Customer,
    /// Most-recently-updated first.
    UpdatedAt,
}

/// Paginated, filterable, searchable equipment list query. The branch scope is
/// always injected from the JWT principal, not from the caller. All other
/// filters are optional and combinatorial (AND).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentListQuery {
    /// Branch scope resolved from the JWT principal (SUPER_ADMIN = All, others =
    /// their assigned branches). Never caller-supplied.
    pub branch_scope: BranchScope,
    /// Free-text search across management_no (호기-normalized, leading-zero-
    /// insensitive), model, maker, equipment_no, customer name, site name, VIN.
    pub q: Option<String>,
    /// Filter to a single status.
    pub status: Option<EquipmentStatus>,
    /// Filter to a single branch (must be within the principal's branch_scope).
    pub branch_id: Option<BranchId>,
    /// Filter to a single customer.
    pub customer_id: Option<CustomerId>,
    /// Filter to a single site.
    pub site_id: Option<SiteId>,
    /// Filter by model name (exact, case-insensitive).
    pub model: Option<String>,
    /// Filter by maker name (exact, case-insensitive).
    pub maker: Option<String>,
    /// Sort column.
    pub sort: EquipmentSortBy,
    /// Max rows per page (1–200, default 50).
    pub limit: i64,
    /// Zero-based row offset.
    pub offset: i64,
}

/// One row in the paginated equipment list.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentListItem {
    pub equipment_id: EquipmentId,
    pub branch_id: BranchId,
    pub equipment_no: String,
    pub management_no: Option<String>,
    pub status: EquipmentStatus,
    pub model: Option<String>,
    pub maker: Option<String>,
    pub specification: String,
    pub ton_text: String,
    pub customer_name: String,
    pub site_name: String,
    /// Legal owner recorded on the asset master. This is distinct from the
    /// customer/site operator currently using or hosting the equipment.
    pub asset_owner: Option<String>,
    pub vin: Option<String>,
    pub updated_at: OffsetDateTime,
}

/// Paginated equipment list result.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentListPage {
    pub items: Vec<EquipmentListItem>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// Branch-scoped read of one equipment row by id.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentReadQuery {
    /// Principal-resolved branch scope. Never caller-supplied.
    pub branch_scope: BranchScope,
    /// Equipment id from the route path.
    pub equipment_id: EquipmentId,
}

/// Branch-scoped read of one equipment object's lifecycle ribbon and
/// customer→site→equipment→work-order relationship graph.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentTimelineGraphQuery {
    /// Principal-resolved branch scope. Never caller-supplied.
    pub branch_scope: BranchScope,
    /// Equipment id from the route path.
    pub equipment_id: EquipmentId,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentTimelineGraph {
    pub equipment: EquipmentTimelineEquipment,
    pub lifecycle_events: Vec<EquipmentLifecycleEvent>,
    pub graph: EquipmentRelationshipGraph,
    pub work_order_count: i64,
    pub cost_ledger_total_won: i64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentTimelineEquipment {
    pub equipment_id: EquipmentId,
    pub branch_id: BranchId,
    pub equipment_no: String,
    pub management_no: Option<String>,
    pub status: EquipmentStatus,
    pub model: Option<String>,
    pub maker: Option<String>,
    pub customer_id: CustomerId,
    pub customer_name: String,
    pub site_id: SiteId,
    pub site_name: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentLifecycleEvent {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub description: Option<String>,
    pub event_date: Option<Date>,
    pub occurred_at: Option<OffsetDateTime>,
    pub href: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentRelationshipGraph {
    pub nodes: Vec<EquipmentGraphNode>,
    pub edges: Vec<EquipmentGraphEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentGraphNode {
    pub id: String,
    pub node_type: String,
    pub label: String,
    pub subtitle: Option<String>,
    pub href: Option<String>,
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentGraphEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EquipmentTimelineBase {
    pub equipment_id: EquipmentId,
    pub branch_id: BranchId,
    pub equipment_no: String,
    pub management_no: Option<String>,
    pub status: EquipmentStatus,
    pub model: Option<String>,
    pub maker: Option<String>,
    pub customer_id: CustomerId,
    pub customer_name: String,
    pub site_id: SiteId,
    pub site_name: String,
    pub asset_registered_on: Option<Date>,
    pub rental_started_on: Option<Date>,
    pub acquisition_date: Option<Date>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EquipmentTimelineWorkOrder {
    pub id: WorkOrderId,
    pub request_no: String,
    pub status: String,
    pub priority: String,
    pub symptom: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub target_due_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EquipmentTimelineSubstitution {
    pub id: EquipmentSubstitutionId,
    pub source_equipment_id: EquipmentId,
    pub substitute_equipment_id: EquipmentId,
    pub assignment_location: String,
    pub assigned_at: OffsetDateTime,
    pub returned_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EquipmentCostLedgerSummary {
    pub entry_count: i64,
    pub total_won: i64,
    pub latest_entry_at: Option<OffsetDateTime>,
}

/// Branch-scoped read of the dispatch map's per-site equipment aggregation.
/// The scope is the principal's `branch_scope` so a non-SUPER_ADMIN sees only
/// their branches — the same filter `list_equipment_substitutes` applies.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EquipmentByLocationQuery {
    pub branch_scope: BranchScope,
}

/// One site row for the dispatch map. Sites with no entered coordinates come
/// back with `latitude`/`longitude` = `None`; the UI lists them as "ungeocoded"
/// instead of dropping a fabricated pin. Counts are computed in SQL from the
/// Korean status literals (`임대` rented, `예비` spare) and the active-substitution
/// table, so the map never shows a fake number either.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SiteLocationGroup {
    pub site_id: SiteId,
    pub site_name: String,
    pub customer_id: CustomerId,
    pub customer_name: String,
    pub branch_id: BranchId,
    pub address: Option<String>,
    pub postal_code: Option<String>,
    pub province: Option<String>,
    pub city: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub geofence_radius_m: Option<f64>,
    pub contact_name: Option<String>,
    pub contact_phone: Option<String>,
    pub contact_email: Option<String>,
    pub equipment_count: i64,
    pub rented_count: i64,
    pub spare_count: i64,
    pub substitution_active_count: i64,
}

/// Partial coordinate/address update for one site. Every field is optional so a
/// caller can set only what it has; absent fields are left unchanged. This is
/// the ONLY coordinate entry point — coordinates exist solely because an admin
/// (EquipmentManage) typed them, satisfying "no fake pins until data entered".
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpdateSiteFields {
    pub address: Option<Option<String>>,
    pub province: Option<Option<String>>,
    pub city: Option<Option<String>>,
    pub postal_code: Option<Option<String>>,
    pub latitude: Option<Option<f64>>,
    pub longitude: Option<Option<f64>>,
    pub geofence_radius_m: Option<Option<f64>>,
    pub contact_name: Option<Option<String>>,
    pub contact_phone: Option<Option<String>>,
    pub contact_email: Option<Option<String>>,
}

impl UpdateSiteFields {
    /// True when no field would be written, so the adapter can reject an empty
    /// patch with a validation error instead of opening a transaction.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpdateSiteCommand {
    pub actor: UserId,
    pub site_id: SiteId,
    pub fields: UpdateSiteFields,
    /// The actor's branch scope. Sites are branch-scoped (unlike org-global
    /// equipment), so the adapter rejects an edit to a site outside this scope —
    /// a branch admin cannot write another branch's site even within the org.
    pub branch_scope: BranchScope,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

pub fn site_update_audit_event(
    actor: UserId,
    branch_id: BranchId,
    site_id: SiteId,
    before: serde_json::Value,
    after: serde_json::Value,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("site.update")?,
        "registry_sites",
        site_id.to_string(),
        trace,
        occurred_at,
    )
    .with_branch(branch_id)
    .with_snapshots(Some(before), Some(after)))
}

pub fn equipment_create_audit_event(
    actor: UserId,
    branch_id: BranchId,
    equipment_id: EquipmentId,
    equipment_no: &EquipmentNo,
    status: EquipmentStatus,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    let after = serde_json::json!({
        "id": equipment_id,
        "equipment_no": equipment_no.as_str(),
        "status": status,
    });
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("equipment.create")?,
        "registry_equipment",
        equipment_id.to_string(),
        trace,
        occurred_at,
    )
    .with_branch(branch_id)
    .with_snapshots(None, Some(after)))
}

pub fn customer_create_audit_event(
    actor: UserId,
    branch_id: BranchId,
    customer: &CreatedCustomer,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    let after = serde_json::json!({
        "id": customer.id,
        "branch_id": customer.branch_id,
        "name": customer.name,
    });
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("customer.create")?,
        "registry_customers",
        customer.id.to_string(),
        trace,
        occurred_at,
    )
    .with_branch(branch_id)
    .with_snapshots(None, Some(after)))
}

pub fn site_create_audit_event(
    actor: UserId,
    branch_id: BranchId,
    site: &CreatedSite,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    let after = serde_json::json!({
        "id": site.id,
        "customer_id": site.customer_id,
        "branch_id": site.branch_id,
        "name": site.name,
        "address": site.address,
        "province": site.province,
        "city": site.city,
        "postal_code": site.postal_code,
        "latitude": site.latitude,
        "longitude": site.longitude,
        "geofence_radius_m": site.geofence_radius_m,
        "contact_name": site.contact_name,
        "contact_phone": site.contact_phone,
        "contact_email": site.contact_email,
    });
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("site.create")?,
        "registry_sites",
        site.id.to_string(),
        trace,
        occurred_at,
    )
    .with_branch(branch_id)
    .with_snapshots(None, Some(after)))
}

pub fn equipment_update_audit_event(
    actor: UserId,
    branch_id: BranchId,
    equipment_id: EquipmentId,
    before: serde_json::Value,
    after: serde_json::Value,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("equipment.update")?,
        "registry_equipment",
        equipment_id.to_string(),
        trace,
        occurred_at,
    )
    .with_branch(branch_id)
    .with_snapshots(Some(before), Some(after)))
}

pub fn equipment_delete_audit_event(
    actor: UserId,
    branch_id: BranchId,
    equipment_id: EquipmentId,
    equipment_no: &EquipmentNo,
    before_status: EquipmentStatus,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    let before = serde_json::json!({
        "id": equipment_id,
        "equipment_no": equipment_no.as_str(),
        "status": before_status,
    });
    let after = serde_json::json!({
        "id": equipment_id,
        "equipment_no": equipment_no.as_str(),
        "status": EquipmentStatus::Disposed,
    });
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("equipment.delete")?,
        "registry_equipment",
        equipment_id.to_string(),
        trace,
        occurred_at,
    )
    .with_branch(branch_id)
    .with_snapshots(Some(before), Some(after)))
}

pub fn registry_import_audit_event(
    actor: Option<UserId>,
    branch_id: BranchId,
    trace: TraceContext,
    occurred_at: Timestamp,
    source_name: &str,
    input_rows: usize,
    equipment_count: usize,
) -> Result<AuditEvent, KernelError> {
    let after = serde_json::json!({
        "source": source_name,
        "input_rows": input_rows,
        "equipment_count": equipment_count,
    });

    Ok(AuditEvent::new(
        actor,
        AuditAction::new("registry.import")?,
        "registry_import",
        source_name,
        trace,
        occurred_at,
    )
    .with_branch(branch_id)
    .with_snapshots(None, Some(after)))
}

pub fn substitute_assign_audit_event(
    command: &SubstituteAssignmentCommand,
    branch_id: BranchId,
    substitution_id: EquipmentSubstitutionId,
) -> Result<AuditEvent, KernelError> {
    let after = serde_json::json!({
        "id": substitution_id,
        "source_equipment_id": command.source_equipment_id,
        "substitute_equipment_id": command.substitute_equipment_id,
        "assigned_to": command.assigned_to,
        "assignment_location": command.assignment_location,
        "assigned_at": command.assigned_at,
    });

    Ok(AuditEvent::new(
        Some(command.actor),
        AuditAction::new("equipment.substitute.assign")?,
        "equipment_substitution",
        substitution_id.to_string(),
        command.trace.clone(),
        command.assigned_at,
    )
    .with_branch(branch_id)
    .with_snapshots(None, Some(after)))
}

pub fn substitute_return_audit_event(
    command: &SubstituteReturnCommand,
    before: &SubstituteAssignment,
) -> Result<AuditEvent, KernelError> {
    let before_snap = serde_json::json!({
        "id": before.id,
        "source_equipment_id": before.source_equipment_id,
        "substitute_equipment_id": before.substitute_equipment_id,
        "assigned_to": before.assigned_to,
        "assignment_location": before.assignment_location,
        "assigned_at": before.assigned_at,
        "returned_at": before.returned_at,
    });
    let after = serde_json::json!({
        "id": before.id,
        "returned_by": command.actor,
        "returned_at": command.returned_at,
        "return_note": command.return_note,
    });

    Ok(AuditEvent::new(
        Some(command.actor),
        AuditAction::new("equipment.substitute.return")?,
        "equipment_substitution",
        before.id.to_string(),
        command.trace.clone(),
        command.returned_at,
    )
    .with_branch(before.branch_id)
    .with_snapshots(Some(before_snap), Some(after)))
}

pub fn equipment_ownership_transfer_request_audit_event(
    command: &CreateEquipmentOwnershipTransferCommand,
    branch_id: BranchId,
    request: &EquipmentOwnershipTransferRequest,
) -> Result<AuditEvent, KernelError> {
    let after = serde_json::json!({
        "id": request.id,
        "equipment_id": request.equipment_id,
        "from_owner": request.from_owner,
        "to_owner": request.to_owner,
        "reason": request.reason,
        "status": request.status,
        "current_step": request.current_step.map(EquipmentOwnershipTransferStepKey::as_str),
        "approval_line": request.approval_line,
    });

    Ok(AuditEvent::new(
        Some(command.actor),
        AuditAction::new("equipment.ownership_transfer.request")?,
        "equipment_ownership_transfer_requests",
        request.id.to_string(),
        command.trace.clone(),
        command.occurred_at,
    )
    .with_branch(branch_id)
    .with_snapshots(None, Some(after)))
}

pub fn equipment_ownership_transfer_decision_audit_event(
    command: &DecideEquipmentOwnershipTransferCommand,
    before: &EquipmentOwnershipTransferRequest,
    after: &EquipmentOwnershipTransferRequest,
) -> Result<AuditEvent, KernelError> {
    let before_snap = serde_json::json!({
        "id": before.id,
        "equipment_id": before.equipment_id,
        "from_owner": before.from_owner,
        "to_owner": before.to_owner,
        "status": before.status,
        "current_step": before.current_step.map(EquipmentOwnershipTransferStepKey::as_str),
        "approval_line": before.approval_line,
    });
    let after_snap = serde_json::json!({
        "id": after.id,
        "equipment_id": after.equipment_id,
        "from_owner": after.from_owner,
        "to_owner": after.to_owner,
        "status": after.status,
        "current_step": after.current_step.map(EquipmentOwnershipTransferStepKey::as_str),
        "decision": command.decision,
        "comment": command.comment,
        "approval_line": after.approval_line,
        "completed_at": after.completed_at,
    });

    Ok(AuditEvent::new(
        Some(command.actor),
        AuditAction::new("equipment.ownership_transfer.decide")?,
        "equipment_ownership_transfer_requests",
        after.id.to_string(),
        command.trace.clone(),
        command.occurred_at,
    )
    .with_branch(after.branch_id)
    .with_snapshots(Some(before_snap), Some(after_snap)))
}
