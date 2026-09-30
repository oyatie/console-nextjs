//! HTTP-independent contracts for the logistics pilot.  `org_id` is absent by
//! design: the adapter derives it from the authenticated request context.
use console_kernel_core::{BranchId, UserId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAsn {
    pub actor: UserId,
    pub branch_id: BranchId,
    pub warehouse_code: String,
    pub external_reference: String,
    pub sku: String,
    pub expected_quantity: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub actor: UserId,
    pub asn_id: Uuid,
    pub quantity: i64,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub actor: UserId,
    pub branch_id: BranchId,
    pub warehouse_code: String,
    pub sku: String,
    pub quantity: i64,
    pub due_at: time::OffsetDateTime,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PilotView {
    pub id: Uuid,
    pub status: String,
    pub branch_id: BranchId,
}

/// Commands against an already-persisted logistics aggregate intentionally do
/// not carry a branch id. The adapter derives aggregate ownership under a row
/// lock; callers can never redirect a transition by supplying JSON metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Putaway {
    pub actor: UserId,
    pub asn_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pick {
    pub actor: UserId,
    pub fulfillment_id: Uuid,
    pub picked_quantity: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pack {
    pub actor: UserId,
    pub fulfillment_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dispatch {
    pub actor: UserId,
    pub fulfillment_id: Uuid,
    pub carrier_name: String,
    pub vehicle_reference: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmPod {
    pub actor: UserId,
    pub shipment_id: Uuid,
    pub recipient_name: String,
    pub evidence_reference: String,
    pub confirmed_at: time::OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettleOperationalCost {
    pub actor: UserId,
    pub shipment_id: Uuid,
    pub currency_code: String,
    pub amount_minor: i64,
    pub settled_at: time::OffsetDateTime,
}
