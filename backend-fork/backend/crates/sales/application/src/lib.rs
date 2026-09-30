//! Sales-catalog application layer (#6): read models, commands, and audit
//! builders. Sales is an ORG-LEVEL catalog (no branch scoping), so audit events
//! carry no branch. The public inquiry submit has no authenticated actor.

use console_kernel_core::{
    AuditAction, AuditEvent, CustomerInquiryId, EquipmentId, KernelError, SalesListingId,
    Timestamp, TraceContext, UserId,
};
use console_sales_domain::{
    InquiryStatus, InquiryTopic, ListingCondition, ListingKind, ListingStatus, ListingType,
};
use serde::{Deserialize, Serialize};

// ─── Read models ─────────────────────────────────────────────────────────────

/// One photo attached to a listing. `url` is a stable, public storefront
/// serve path built by the REST layer from the listing + media `id`
/// (`/api/v1/storefront/listings/{listing_id}/media/{id}`); the object bytes
/// live in the object store keyed by an internal s3_key never exposed here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListingMediaView {
    pub id: String,
    pub url: String,
    pub content_type: String,
    pub alt_text: Option<String>,
    pub sort_order: i32,
}

/// A sales listing as read by the storefront or the admin console.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SalesListingView {
    pub id: SalesListingId,
    pub equipment_id: Option<EquipmentId>,
    pub kind: ListingKind,
    pub condition: ListingCondition,
    pub model_name: String,
    pub capacity_milli: Option<i64>,
    pub model_year: Option<i32>,
    pub usage_hours: Option<i32>,
    pub price_won: Option<i64>,
    pub badge: Option<String>,
    pub usage_label: Option<String>,
    pub condition_label: Option<String>,
    pub availability: Option<String>,
    pub location: Option<String>,
    pub description: Option<String>,
    pub listing_type: ListingType,
    pub status: ListingStatus,
    pub sort_weight: i32,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub media: Vec<ListingMediaView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SalesListingPage {
    pub items: Vec<SalesListingView>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

/// Catalog read filter. `include_non_public` is set only on the admin read; the
/// public storefront leaves it false so RLS + the status filter expose only
/// published/reserved listings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogQuery {
    pub kind: Option<ListingKind>,
    pub condition: Option<ListingCondition>,
    pub listing_type: Option<ListingType>,
    pub include_non_public: bool,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomerInquiryView {
    pub id: CustomerInquiryId,
    pub name: String,
    pub phone: String,
    pub topic: InquiryTopic,
    pub location: Option<String>,
    pub message: Option<String>,
    pub listing_id: Option<SalesListingId>,
    pub status: InquiryStatus,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomerInquiryPage {
    pub items: Vec<CustomerInquiryView>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InquiryInboxQuery {
    pub status: Option<InquiryStatus>,
    pub limit: i64,
    pub offset: i64,
}

// ─── Commands ────────────────────────────────────────────────────────────────

/// The full editable field set of a listing (used for create).
#[derive(Debug, Clone)]
pub struct ListingInput {
    pub kind: ListingKind,
    pub condition: ListingCondition,
    pub model_name: String,
    pub capacity_milli: Option<i64>,
    pub model_year: Option<i32>,
    pub usage_hours: Option<i32>,
    pub price_won: Option<i64>,
    pub badge: Option<String>,
    pub usage_label: Option<String>,
    pub condition_label: Option<String>,
    pub availability: Option<String>,
    pub location: Option<String>,
    pub description: Option<String>,
    pub listing_type: ListingType,
    pub status: ListingStatus,
    pub sort_weight: i32,
    pub equipment_id: Option<EquipmentId>,
}

#[derive(Debug, Clone)]
pub struct CreateListingCommand {
    pub actor: UserId,
    pub listing_id: SalesListingId,
    pub input: ListingInput,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// PATCH fields. An absent `Option` is unchanged; a nullable column uses
/// `Option<Option<T>>` (outer absent = unchanged, inner None = clear to NULL).
#[derive(Debug, Clone, Default)]
pub struct UpdateListingFields {
    pub kind: Option<ListingKind>,
    pub condition: Option<ListingCondition>,
    pub model_name: Option<String>,
    pub capacity_milli: Option<Option<i64>>,
    pub model_year: Option<Option<i32>>,
    pub usage_hours: Option<Option<i32>>,
    pub price_won: Option<Option<i64>>,
    pub badge: Option<Option<String>>,
    pub usage_label: Option<Option<String>>,
    pub condition_label: Option<Option<String>>,
    pub availability: Option<Option<String>>,
    pub location: Option<Option<String>>,
    pub description: Option<Option<String>>,
    pub listing_type: Option<ListingType>,
    pub status: Option<ListingStatus>,
    pub sort_weight: Option<i32>,
    pub equipment_id: Option<Option<EquipmentId>>,
}

impl UpdateListingFields {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.condition.is_none()
            && self.model_name.is_none()
            && self.capacity_milli.is_none()
            && self.model_year.is_none()
            && self.usage_hours.is_none()
            && self.price_won.is_none()
            && self.badge.is_none()
            && self.usage_label.is_none()
            && self.condition_label.is_none()
            && self.availability.is_none()
            && self.location.is_none()
            && self.description.is_none()
            && self.listing_type.is_none()
            && self.status.is_none()
            && self.sort_weight.is_none()
            && self.equipment_id.is_none()
    }
}

#[derive(Debug, Clone)]
pub struct UpdateListingCommand {
    pub actor: UserId,
    pub listing_id: SalesListingId,
    pub fields: UpdateListingFields,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone)]
pub struct DeleteListingCommand {
    pub actor: UserId,
    pub listing_id: SalesListingId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// A public inquiry submission. No authenticated actor; the storefront supplies
/// the lead fields and (optionally) the listing it was made from.
#[derive(Debug, Clone)]
pub struct SubmitInquiryCommand {
    pub inquiry_id: CustomerInquiryId,
    pub name: String,
    pub phone: String,
    pub topic: InquiryTopic,
    pub location: Option<String>,
    pub message: Option<String>,
    pub listing_id: Option<SalesListingId>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone)]
pub struct UpdateInquiryStatusCommand {
    pub actor: UserId,
    pub inquiry_id: CustomerInquiryId,
    pub status: InquiryStatus,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

// ─── Audit-event builders (org-level: no branch) ─────────────────────────────

/// # Errors
/// Propagates `AuditAction::new` validation failure.
pub fn listing_create_audit_event(
    actor: UserId,
    listing_id: SalesListingId,
    after: serde_json::Value,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("sales_listing.create")?,
        "sales_listings",
        listing_id.to_string(),
        trace,
        occurred_at,
    )
    .with_snapshots(None, Some(after)))
}

/// # Errors
/// Propagates `AuditAction::new` validation failure.
pub fn listing_update_audit_event(
    actor: UserId,
    listing_id: SalesListingId,
    before: serde_json::Value,
    after: serde_json::Value,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("sales_listing.update")?,
        "sales_listings",
        listing_id.to_string(),
        trace,
        occurred_at,
    )
    .with_snapshots(Some(before), Some(after)))
}

/// # Errors
/// Propagates `AuditAction::new` validation failure.
pub fn listing_delete_audit_event(
    actor: UserId,
    listing_id: SalesListingId,
    before: serde_json::Value,
    after: serde_json::Value,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("sales_listing.delete")?,
        "sales_listings",
        listing_id.to_string(),
        trace,
        occurred_at,
    )
    .with_snapshots(Some(before), Some(after)))
}

/// Public inquiry submit: no actor. The `after` snapshot is built PII-LIGHT by
/// the adapter (topic/listing/status only — never the name/phone/message, which
/// live in `customer_inquiries`), so the audit trail records the event without
/// duplicating personal data.
///
/// # Errors
/// Propagates `AuditAction::new` validation failure.
pub fn inquiry_submit_audit_event(
    inquiry_id: CustomerInquiryId,
    after: serde_json::Value,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        None,
        AuditAction::new("sales_inquiry.submit")?,
        "customer_inquiries",
        inquiry_id.to_string(),
        trace,
        occurred_at,
    )
    .with_snapshots(None, Some(after)))
}

/// # Errors
/// Propagates `AuditAction::new` validation failure.
pub fn inquiry_status_audit_event(
    actor: UserId,
    inquiry_id: CustomerInquiryId,
    before: serde_json::Value,
    after: serde_json::Value,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new("sales_inquiry.status")?,
        "customer_inquiries",
        inquiry_id.to_string(),
        trace,
        occurred_at,
    )
    .with_snapshots(Some(before), Some(after)))
}
