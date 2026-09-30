//! Logistics-pilot finite-state vocabulary.  The pilot has one warehouse leg;
//! its state machine intentionally has no routing, valuation, or finance edge.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FulfillmentState {
    Released,
    Picked,
    ShortPick,
    Packed,
    Dispatched,
    Delivered,
    Settled,
}
impl FulfillmentState {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Released => "RELEASED",
            Self::Picked => "PICKED",
            Self::ShortPick => "SHORT_PICK",
            Self::Packed => "PACKED",
            Self::Dispatched => "DISPATCHED",
            Self::Delivered => "DELIVERED",
            Self::Settled => "SETTLED",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "RELEASED" => Ok(Self::Released),
            "PICKED" => Ok(Self::Picked),
            "SHORT_PICK" => Ok(Self::ShortPick),
            "PACKED" => Ok(Self::Packed),
            "DISPATCHED" => Ok(Self::Dispatched),
            "DELIVERED" => Ok(Self::Delivered),
            "SETTLED" => Ok(Self::Settled),
            _ => Err(KernelError::conflict("unknown logistics fulfillment state")),
        }
    }
    pub fn can_transition_to(self, next: Self) -> Result<(), KernelError> {
        let allowed = matches!(
            (self, next),
            (Self::Released, Self::Picked | Self::ShortPick)
                | (Self::Picked | Self::ShortPick, Self::Packed)
                | (Self::Packed, Self::Dispatched)
                | (Self::Dispatched, Self::Delivered)
                | (Self::Delivered, Self::Settled)
        );
        if allowed {
            Ok(())
        } else {
            Err(KernelError::conflict("illegal logistics state transition"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FulfillmentState;

    #[test]
    fn allows_each_legal_fulfillment_transition() {
        for (from, to) in [
            (FulfillmentState::Released, FulfillmentState::Picked),
            (FulfillmentState::Released, FulfillmentState::ShortPick),
            (FulfillmentState::Picked, FulfillmentState::Packed),
            (FulfillmentState::ShortPick, FulfillmentState::Packed),
            (FulfillmentState::Packed, FulfillmentState::Dispatched),
            (FulfillmentState::Dispatched, FulfillmentState::Delivered),
            (FulfillmentState::Delivered, FulfillmentState::Settled),
        ] {
            from.can_transition_to(to)
                .expect("listed transition is legal");
        }
    }

    #[test]
    fn database_state_round_trips_to_the_same_transition_vocabulary() {
        for state in [
            FulfillmentState::Released,
            FulfillmentState::Picked,
            FulfillmentState::ShortPick,
            FulfillmentState::Packed,
            FulfillmentState::Dispatched,
            FulfillmentState::Delivered,
            FulfillmentState::Settled,
        ] {
            assert_eq!(FulfillmentState::from_db(state.as_db()).unwrap(), state);
        }
        assert!(FulfillmentState::from_db("UNKNOWN").is_err());
    }

    #[test]
    fn rejects_illegal_and_terminal_fulfillment_transitions() {
        for (from, to) in [
            (FulfillmentState::Released, FulfillmentState::Packed),
            (FulfillmentState::Settled, FulfillmentState::Delivered),
            (FulfillmentState::Delivered, FulfillmentState::Packed),
        ] {
            assert!(from.can_transition_to(to).is_err());
        }
    }

    #[test]
    fn fulfillment_state_wire_and_illegal_matrix() {
        // Full wire set + extra illegal edges beyond the three-spot smoke.
        for state in [
            FulfillmentState::Released,
            FulfillmentState::Picked,
            FulfillmentState::ShortPick,
            FulfillmentState::Packed,
            FulfillmentState::Dispatched,
            FulfillmentState::Delivered,
            FulfillmentState::Settled,
        ] {
            assert_eq!(FulfillmentState::from_db(state.as_db()).unwrap(), state);
        }
        assert!(FulfillmentState::from_db("released").is_err());
        assert!(FulfillmentState::from_db("PICKED ").is_err());

        // Skip steps and reverse edges stay fail-closed.
        for (from, to) in [
            (FulfillmentState::Released, FulfillmentState::Dispatched),
            (FulfillmentState::Released, FulfillmentState::Settled),
            (FulfillmentState::Picked, FulfillmentState::Released),
            (FulfillmentState::Packed, FulfillmentState::Picked),
            (FulfillmentState::Settled, FulfillmentState::Released),
            (FulfillmentState::Dispatched, FulfillmentState::Released),
        ] {
            assert!(
                from.can_transition_to(to).is_err(),
                "{from:?} -> {to:?} must be illegal"
            );
        }
    }
}
