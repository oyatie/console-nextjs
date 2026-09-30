//! HTTP-independent command contracts for the equipment 3R pilot.  `org_id`
//! is absent by design: the adapter derives it from the authenticated request
//! context.  The adapter validates every field against the design-contract
//! bounds before any row is written.
use console_kernel_core::BranchId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Register one serialized rental unit into the pilot registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterUnit {
    pub branch_id: BranchId,
    pub serial_no: String,
    pub model_name: String,
    pub capacity_class: String,
    pub acquisition_cost_minor: i64,
}

/// Open an idempotent rental-case quote against an unsold unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuoteCase {
    pub branch_id: BranchId,
    pub unit_id: Uuid,
    pub customer_name: String,
    pub site_reference: String,
    pub monthly_rate_minor: i64,
    pub duration_months: i32,
    pub currency_code: String,
}

/// Four-eyes approval decision on a quoted case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecideApproval {
    pub decision: String,
    pub reason: Option<String>,
}

/// Physical delivery leg of an approved case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchCase {
    pub carrier_name: String,
    pub vehicle_reference: String,
}

/// Customer handover with immutable evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoverCase {
    pub recipient_name: String,
    /// Docs/Evidence object UUID; its original verified WORM copy and custody are validated transactionally.
    pub evidence_object_id: Uuid,
    pub handed_over_at: OffsetDateTime,
}

/// On-rent inspection or maintenance record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InspectCase {
    pub outcome: String,
    pub findings: String,
    pub maintenance_note: Option<String>,
}

/// Return assessment binding the unit to a disposition branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssessReturn {
    pub condition_grade: String,
    pub findings: String,
    pub disposition: String,
}

/// Kind-dependent disposition completion payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteDisposition {
    pub cost_minor: Option<i64>,
    pub sale_amount_minor: Option<i64>,
    pub buyer_name: Option<String>,
}
