//! Equipment 3R pilot finite-state vocabulary.  The pilot has one bounded
//! rental-return-redeploy loop; its state machines intentionally have no
//! legacy-registry, work-order, or finance edge.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
use console_kernel_core::KernelError;
use serde::{Deserialize, Serialize};

/// Unit availability lifecycle.  `SOLD` is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Availability {
    Available,
    Reserved,
    OnRent,
    InAssessment,
    InRepair,
    InRefurbishment,
    ForSale,
    Sold,
}
impl Availability {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Available => "AVAILABLE",
            Self::Reserved => "RESERVED",
            Self::OnRent => "ON_RENT",
            Self::InAssessment => "IN_ASSESSMENT",
            Self::InRepair => "IN_REPAIR",
            Self::InRefurbishment => "IN_REFURBISHMENT",
            Self::ForSale => "FOR_SALE",
            Self::Sold => "SOLD",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "AVAILABLE" => Ok(Self::Available),
            "RESERVED" => Ok(Self::Reserved),
            "ON_RENT" => Ok(Self::OnRent),
            "IN_ASSESSMENT" => Ok(Self::InAssessment),
            "IN_REPAIR" => Ok(Self::InRepair),
            "IN_REFURBISHMENT" => Ok(Self::InRefurbishment),
            "FOR_SALE" => Ok(Self::ForSale),
            "SOLD" => Ok(Self::Sold),
            _ => Err(KernelError::conflict(
                "unknown equipment availability state",
            )),
        }
    }

    pub fn can_transition_to(self, next: Self) -> Result<(), KernelError> {
        let allowed = matches!(
            (self, next),
            (Self::Available, Self::Reserved)
                | (Self::Reserved, Self::OnRent)
                | (Self::OnRent, Self::InAssessment)
                | (
                    Self::InAssessment,
                    Self::InRepair | Self::InRefurbishment | Self::ForSale | Self::Available
                )
                | (Self::InRepair | Self::InRefurbishment, Self::Available)
                | (Self::ForSale, Self::Sold)
        );
        if allowed {
            Ok(())
        } else {
            Err(KernelError::conflict(
                "illegal equipment availability transition",
            ))
        }
    }
}

/// Rental-case lifecycle.  `DECLINED` and `CLOSED` are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CaseState {
    Quoted,
    Approved,
    Declined,
    Dispatched,
    HandedOver,
    Returned,
    Closed,
}
impl CaseState {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Quoted => "QUOTED",
            Self::Approved => "APPROVED",
            Self::Declined => "DECLINED",
            Self::Dispatched => "DISPATCHED",
            Self::HandedOver => "HANDED_OVER",
            Self::Returned => "RETURNED",
            Self::Closed => "CLOSED",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "QUOTED" => Ok(Self::Quoted),
            "APPROVED" => Ok(Self::Approved),
            "DECLINED" => Ok(Self::Declined),
            "DISPATCHED" => Ok(Self::Dispatched),
            "HANDED_OVER" => Ok(Self::HandedOver),
            "RETURNED" => Ok(Self::Returned),
            "CLOSED" => Ok(Self::Closed),
            _ => Err(KernelError::conflict("unknown rental case state")),
        }
    }

    pub fn can_transition_to(self, next: Self) -> Result<(), KernelError> {
        let allowed = matches!(
            (self, next),
            (Self::Quoted, Self::Approved | Self::Declined)
                | (Self::Approved, Self::Dispatched)
                | (Self::Dispatched, Self::HandedOver)
                | (Self::HandedOver, Self::Returned)
                | (Self::Returned, Self::Closed)
        );
        if allowed {
            Ok(())
        } else {
            Err(KernelError::conflict("illegal rental case transition"))
        }
    }
}

/// Disposition execution lifecycle.  `COMPLETED` is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DispositionState {
    Open,
    Completed,
}
impl DispositionState {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::Completed => "COMPLETED",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "OPEN" => Ok(Self::Open),
            "COMPLETED" => Ok(Self::Completed),
            _ => Err(KernelError::conflict("unknown disposition state")),
        }
    }

    pub fn can_transition_to(self, next: Self) -> Result<(), KernelError> {
        if matches!((self, next), (Self::Open, Self::Completed)) {
            Ok(())
        } else {
            Err(KernelError::conflict("illegal disposition transition"))
        }
    }
}

/// The branch a return assessment binds the unit to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DispositionKind {
    Repair,
    Refurbish,
    Resale,
    Redeploy,
}
impl DispositionKind {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Repair => "REPAIR",
            Self::Refurbish => "REFURBISH",
            Self::Resale => "RESALE",
            Self::Redeploy => "REDEPLOY",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "REPAIR" => Ok(Self::Repair),
            "REFURBISH" => Ok(Self::Refurbish),
            "RESALE" => Ok(Self::Resale),
            "REDEPLOY" => Ok(Self::Redeploy),
            _ => Err(KernelError::conflict("unknown disposition kind")),
        }
    }

    /// Availability the unit moves to when the assessment posts.
    #[must_use]
    pub const fn assessment_target(self) -> Availability {
        match self {
            Self::Repair => Availability::InRepair,
            Self::Refurbish => Availability::InRefurbishment,
            Self::Resale => Availability::ForSale,
            Self::Redeploy => Availability::Available,
        }
    }

    /// Availability the unit moves to when the disposition completes.
    /// `REDEPLOY` dispositions are inserted already completed, so they have
    /// no completion edge.
    #[must_use]
    pub const fn completion_target(self) -> Option<Availability> {
        match self {
            Self::Repair | Self::Refurbish => Some(Availability::Available),
            Self::Resale => Some(Availability::Sold),
            Self::Redeploy => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Availability, CaseState, DispositionKind, DispositionState};

    #[test]
    fn allows_each_legal_availability_transition() {
        for (from, to) in [
            (Availability::Available, Availability::Reserved),
            (Availability::Reserved, Availability::OnRent),
            (Availability::OnRent, Availability::InAssessment),
            (Availability::InAssessment, Availability::InRepair),
            (Availability::InAssessment, Availability::InRefurbishment),
            (Availability::InAssessment, Availability::ForSale),
            (Availability::InAssessment, Availability::Available),
            (Availability::InRepair, Availability::Available),
            (Availability::InRefurbishment, Availability::Available),
            (Availability::ForSale, Availability::Sold),
        ] {
            from.can_transition_to(to)
                .expect("listed transition is legal");
        }
    }

    #[test]
    fn rejects_illegal_and_terminal_availability_transitions() {
        for (from, to) in [
            (Availability::Available, Availability::OnRent),
            (Availability::Sold, Availability::Available),
            (Availability::Reserved, Availability::Available),
            (Availability::ForSale, Availability::Available),
        ] {
            assert!(from.can_transition_to(to).is_err());
        }
    }

    #[test]
    fn allows_each_legal_case_transition() {
        for (from, to) in [
            (CaseState::Quoted, CaseState::Approved),
            (CaseState::Quoted, CaseState::Declined),
            (CaseState::Approved, CaseState::Dispatched),
            (CaseState::Dispatched, CaseState::HandedOver),
            (CaseState::HandedOver, CaseState::Returned),
            (CaseState::Returned, CaseState::Closed),
        ] {
            from.can_transition_to(to)
                .expect("listed transition is legal");
        }
    }

    #[test]
    fn rejects_illegal_and_terminal_case_transitions() {
        for (from, to) in [
            (CaseState::Quoted, CaseState::Dispatched),
            (CaseState::Declined, CaseState::Approved),
            (CaseState::Closed, CaseState::Quoted),
            (CaseState::Approved, CaseState::HandedOver),
        ] {
            assert!(from.can_transition_to(to).is_err());
        }
    }

    #[test]
    fn disposition_state_machine_is_open_to_completed_only() {
        DispositionState::Open
            .can_transition_to(DispositionState::Completed)
            .expect("open completes");
        assert!(
            DispositionState::Completed
                .can_transition_to(DispositionState::Open)
                .is_err()
        );
    }

    #[test]
    fn disposition_kinds_map_to_contract_availability_targets() {
        assert_eq!(
            DispositionKind::Repair.assessment_target(),
            Availability::InRepair
        );
        assert_eq!(
            DispositionKind::Refurbish.assessment_target(),
            Availability::InRefurbishment
        );
        assert_eq!(
            DispositionKind::Resale.assessment_target(),
            Availability::ForSale
        );
        assert_eq!(
            DispositionKind::Redeploy.assessment_target(),
            Availability::Available
        );
        assert_eq!(
            DispositionKind::Repair.completion_target(),
            Some(Availability::Available)
        );
        assert_eq!(
            DispositionKind::Refurbish.completion_target(),
            Some(Availability::Available)
        );
        assert_eq!(
            DispositionKind::Resale.completion_target(),
            Some(Availability::Sold)
        );
        assert_eq!(DispositionKind::Redeploy.completion_target(), None);
    }

    #[test]
    fn db_round_trip_covers_every_state() {
        for value in [
            "AVAILABLE",
            "RESERVED",
            "ON_RENT",
            "IN_ASSESSMENT",
            "IN_REPAIR",
            "IN_REFURBISHMENT",
            "FOR_SALE",
            "SOLD",
        ] {
            assert_eq!(Availability::from_db(value).unwrap().as_db(), value);
        }
        for value in [
            "QUOTED",
            "APPROVED",
            "DECLINED",
            "DISPATCHED",
            "HANDED_OVER",
            "RETURNED",
            "CLOSED",
        ] {
            assert_eq!(CaseState::from_db(value).unwrap().as_db(), value);
        }
        for value in ["OPEN", "COMPLETED"] {
            assert_eq!(DispositionState::from_db(value).unwrap().as_db(), value);
        }
        for value in ["REPAIR", "REFURBISH", "RESALE", "REDEPLOY"] {
            assert_eq!(DispositionKind::from_db(value).unwrap().as_db(), value);
        }
        assert!(Availability::from_db("BROKEN").is_err());
        assert!(CaseState::from_db("BROKEN").is_err());
        assert!(DispositionState::from_db("BROKEN").is_err());
        assert!(DispositionKind::from_db("BROKEN").is_err());
    }

    /// Fail-closed wire matrix: empty, case-mismatched, whitespace-padded, and
    /// near-miss tokens must not parse as legal SCREAMING_SNAKE states.
    #[test]
    fn from_db_fails_closed_on_empty_case_and_near_miss_wires() {
        for bad in [
            "",
            "available",
            "AVAILABLE ",
            "ON-RENT",
            "sold",
            " IN_REPAIR",
        ] {
            assert!(
                Availability::from_db(bad).is_err(),
                "availability wire {bad:?}"
            );
        }
        for bad in ["", "quoted", "QUOTED ", "HANDED OVER", "closed"] {
            assert!(CaseState::from_db(bad).is_err(), "case wire {bad:?}");
        }
        for bad in ["", "open", "OPEN ", "complete", "COMPLETED\n"] {
            assert!(
                DispositionState::from_db(bad).is_err(),
                "disposition state wire {bad:?}"
            );
        }
        for bad in ["", "repair", "REPAIR ", "REDEPLOYED", "re-deploy"] {
            assert!(
                DispositionKind::from_db(bad).is_err(),
                "disposition kind wire {bad:?}"
            );
        }
    }
}
