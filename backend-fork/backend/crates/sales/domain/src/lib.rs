//! Pure sales-catalog domain (#6 지게차 매매/부가서비스).
//!
//! Holds the classification enums and small business rules for forklift sales
//! listings and customer inquiries. Depends only on the kernel (layer-boundary):
//! capacity is carried as plain milli-tons and price as plain KRW won — display
//! formatting (e.g. "2.5톤", "₩12,000,000") is a presentation concern.
#![cfg_attr(test, allow(clippy::unwrap_used))]

use console_kernel_core::KernelError;

/// Fuel / drive class of a listed forklift. Mirrors the `sales_listings.kind`
/// CHECK. The rental recommender additionally offers LPG, hence it is included
/// here even though the public used-filter only surfaces electric/diesel/reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ListingKind {
    Electric,
    Diesel,
    Lpg,
    Reach,
}

impl ListingKind {
    /// # Errors
    /// Returns `KernelError::validation` for an unknown value.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "ELECTRIC" => Ok(Self::Electric),
            "DIESEL" => Ok(Self::Diesel),
            "LPG" => Ok(Self::Lpg),
            "REACH" => Ok(Self::Reach),
            other => Err(KernelError::validation(format!(
                "unknown listing kind {other:?}"
            ))),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Electric => "ELECTRIC",
            Self::Diesel => "DIESEL",
            Self::Lpg => "LPG",
            Self::Reach => "REACH",
        }
    }
}

/// Whether a listed unit is used (중고) or brand-new (신차). Mirrors the
/// `sales_listings.condition` CHECK and drives the public storefront's
/// 중고/신차 sub-category filter. Orthogonal to the free-text `condition_label`
/// display copy ("검수 완료", "A급"), which describes a used unit's grade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ListingCondition {
    Used,
    New,
}

impl ListingCondition {
    /// # Errors
    /// Returns `KernelError::validation` for an unknown value.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "USED" => Ok(Self::Used),
            "NEW" => Ok(Self::New),
            other => Err(KernelError::validation(format!(
                "unknown listing condition {other:?}"
            ))),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Used => "USED",
            Self::New => "NEW",
        }
    }
}

/// Whether a listing is offered for sale, rental, or both. Mirrors the
/// `sales_listings.listing_type` CHECK.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ListingType {
    Sale,
    Rental,
    Both,
}

impl ListingType {
    /// # Errors
    /// Returns `KernelError::validation` for an unknown value.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "SALE" => Ok(Self::Sale),
            "RENTAL" => Ok(Self::Rental),
            "BOTH" => Ok(Self::Both),
            other => Err(KernelError::validation(format!(
                "unknown listing type {other:?}"
            ))),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Sale => "SALE",
            Self::Rental => "RENTAL",
            Self::Both => "BOTH",
        }
    }
}

/// Publication lifecycle of a listing. Mirrors the `sales_listings.status`
/// CHECK. Only `Published` (and `Reserved`, shown as "상담중") is visible to the
/// public catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ListingStatus {
    Draft,
    Published,
    Reserved,
    Sold,
    Withdrawn,
}

impl ListingStatus {
    /// # Errors
    /// Returns `KernelError::validation` for an unknown value.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "DRAFT" => Ok(Self::Draft),
            "PUBLISHED" => Ok(Self::Published),
            "RESERVED" => Ok(Self::Reserved),
            "SOLD" => Ok(Self::Sold),
            "WITHDRAWN" => Ok(Self::Withdrawn),
            other => Err(KernelError::validation(format!(
                "unknown listing status {other:?}"
            ))),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Published => "PUBLISHED",
            Self::Reserved => "RESERVED",
            Self::Sold => "SOLD",
            Self::Withdrawn => "WITHDRAWN",
        }
    }

    /// Whether a listing in this status appears in the public storefront catalog.
    /// Reserved units stay visible (marked 상담중) so buyers see the full range.
    #[must_use]
    pub const fn is_public(self) -> bool {
        matches!(self, Self::Published | Self::Reserved)
    }
}

/// Subject of a customer inquiry. Mirrors the `customer_inquiries.topic` CHECK
/// and drives which contact line the lead is routed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InquiryTopic {
    Rental,
    UsedSales,
    Maintenance,
    Other,
}

impl InquiryTopic {
    /// # Errors
    /// Returns `KernelError::validation` for an unknown value.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "RENTAL" => Ok(Self::Rental),
            "USED_SALES" => Ok(Self::UsedSales),
            "MAINTENANCE" => Ok(Self::Maintenance),
            "OTHER" => Ok(Self::Other),
            other => Err(KernelError::validation(format!(
                "unknown inquiry topic {other:?}"
            ))),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Rental => "RENTAL",
            Self::UsedSales => "USED_SALES",
            Self::Maintenance => "MAINTENANCE",
            Self::Other => "OTHER",
        }
    }

    /// A maintenance inquiry routes to the repair line; everything else to the
    /// business line. The concrete phone numbers live in the REST/config layer
    /// (KNL contact facts), not the domain.
    #[must_use]
    pub const fn is_repair(self) -> bool {
        matches!(self, Self::Maintenance)
    }
}

/// Triage state of an inbound inquiry in the internal inbox. Mirrors the
/// `customer_inquiries.status` CHECK.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InquiryStatus {
    New,
    Contacted,
    Closed,
}

impl InquiryStatus {
    /// # Errors
    /// Returns `KernelError::validation` for an unknown value.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "NEW" => Ok(Self::New),
            "CONTACTED" => Ok(Self::Contacted),
            "CLOSED" => Ok(Self::Closed),
            other => Err(KernelError::validation(format!(
                "unknown inquiry status {other:?}"
            ))),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::New => "NEW",
            Self::Contacted => "CONTACTED",
            Self::Closed => "CLOSED",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_round_trip_through_db_strings() {
        for s in [
            ListingKind::Electric,
            ListingKind::Diesel,
            ListingKind::Lpg,
            ListingKind::Reach,
        ] {
            assert_eq!(ListingKind::parse(s.as_db_str()).unwrap(), s);
        }
        for s in [ListingCondition::Used, ListingCondition::New] {
            assert_eq!(ListingCondition::parse(s.as_db_str()).unwrap(), s);
        }
        for s in [ListingType::Sale, ListingType::Rental, ListingType::Both] {
            assert_eq!(ListingType::parse(s.as_db_str()).unwrap(), s);
        }
        for s in [
            ListingStatus::Draft,
            ListingStatus::Published,
            ListingStatus::Reserved,
            ListingStatus::Sold,
            ListingStatus::Withdrawn,
        ] {
            assert_eq!(ListingStatus::parse(s.as_db_str()).unwrap(), s);
        }
        for s in [
            InquiryTopic::Rental,
            InquiryTopic::UsedSales,
            InquiryTopic::Maintenance,
            InquiryTopic::Other,
        ] {
            assert_eq!(InquiryTopic::parse(s.as_db_str()).unwrap(), s);
        }
        for s in [
            InquiryStatus::New,
            InquiryStatus::Contacted,
            InquiryStatus::Closed,
        ] {
            assert_eq!(InquiryStatus::parse(s.as_db_str()).unwrap(), s);
        }
    }

    #[test]
    fn only_published_and_reserved_are_public() {
        assert!(ListingStatus::Published.is_public());
        assert!(ListingStatus::Reserved.is_public());
        assert!(!ListingStatus::Draft.is_public());
        assert!(!ListingStatus::Sold.is_public());
        assert!(!ListingStatus::Withdrawn.is_public());
    }

    #[test]
    fn only_maintenance_routes_to_repair() {
        assert!(InquiryTopic::Maintenance.is_repair());
        assert!(!InquiryTopic::Rental.is_repair());
        assert!(!InquiryTopic::UsedSales.is_repair());
        assert!(!InquiryTopic::Other.is_repair());
    }

    #[test]
    fn parse_rejects_unknown() {
        assert!(ListingKind::parse("HYDROGEN").is_err());
        assert!(ListingCondition::parse("REFURBISHED").is_err());
        assert!(InquiryTopic::parse("spam").is_err());
    }

    #[test]
    fn serde_uses_screaming_snake_case() {
        assert_eq!(
            serde_json::to_string(&InquiryTopic::UsedSales).unwrap(),
            "\"USED_SALES\""
        );
    }
}
