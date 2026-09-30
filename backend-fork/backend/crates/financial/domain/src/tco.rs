//! Total-cost-of-ownership (TCO) and lifecycle-margin math.
//!
//! Pure, no-DB, integer-KRW-only arithmetic for a single asset's lifecycle:
//! what it was acquired for, how much maintenance has been spent on it, what it
//! sold for, and the per-month / per-hour cost intensity derived from those.
//!
//! All money is whole KRW (`i64`); there are no floats anywhere — KRW has no
//! sub-unit and rounding drift would corrupt accounting totals. Every quotient
//! that can divide by zero (no acquisition date, zero/NULL operating hours)
//! returns `None` rather than panicking or producing infinity.
//!
//! The acquisition basis is resolved with a SOURCE TAG ([`AcquisitionBasis`]):
//! an explicit `acquisition_cost_won` wins; otherwise we fall back to
//! `vehicle_value` (the depreciation base) so legacy bulk-imported assets still
//! roll up a TCO; otherwise there is no basis at all. `vehicle_value` is only
//! ever *read* here as a fallback — it is never written and never recomputed,
//! keeping the depreciation engine and this accounting view strictly separate.

use serde::{Deserialize, Serialize};

/// Where the acquisition figure that anchors TCO came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AcquisitionBasis {
    /// An explicit `acquisition_cost_won` was recorded on the master.
    Explicit,
    /// No explicit acquisition; `vehicle_value` (the depreciation base) is used.
    VehicleValueFallback,
    /// Neither acquisition cost nor vehicle value is known.
    None,
}

/// The resolved acquisition anchor: the won amount (if any) plus the tag saying
/// where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcquisitionAnchor {
    /// The won amount used as the acquisition leg of TCO, or `None` when neither
    /// an explicit cost nor a vehicle-value fallback exists.
    pub amount_won: Option<i64>,
    /// Where `amount_won` came from.
    pub basis: AcquisitionBasis,
}

impl AcquisitionAnchor {
    /// Resolve the acquisition anchor: prefer the explicit `acquisition_cost_won`,
    /// then fall back to `vehicle_value`, then nothing.
    ///
    /// `vehicle_value` is read ONLY as a fallback and is never mutated; this keeps
    /// the accounting view independent of the residual/depreciation engine.
    #[must_use]
    pub const fn resolve(
        acquisition_cost_won: Option<i64>,
        vehicle_value_won: Option<i64>,
    ) -> Self {
        match (acquisition_cost_won, vehicle_value_won) {
            (Some(amount), _) => Self {
                amount_won: Some(amount),
                basis: AcquisitionBasis::Explicit,
            },
            (None, Some(amount)) => Self {
                amount_won: Some(amount),
                basis: AcquisitionBasis::VehicleValueFallback,
            },
            (None, None) => Self {
                amount_won: None,
                basis: AcquisitionBasis::None,
            },
        }
    }
}

/// Total cost of ownership: the acquisition anchor plus all maintenance spent on
/// the asset. The acquisition leg is added EXACTLY ONCE and never double-counts a
/// ledger row; when no acquisition basis exists it contributes zero.
///
/// Outsource cost is deliberately NOT a parameter here — it is surfaced
/// read-only elsewhere and must never be summed into TCO (double-count guard).
#[must_use]
pub fn tco_won(anchor: AcquisitionAnchor, maintenance_total_won: i64) -> i64 {
    anchor
        .amount_won
        .unwrap_or(0)
        .saturating_add(maintenance_total_won)
}

/// Gross margin on a realized sale: `sale_price − tco`. `None` until the asset is
/// actually sold (no sale price). A loss (negative margin) is a valid result and
/// is returned as-is — it is never floored.
#[must_use]
pub fn gross_margin_won(sale_price_won: Option<i64>, tco_won: i64) -> Option<i64> {
    sale_price_won.map(|sale| sale.saturating_sub(tco_won))
}

/// Average maintenance cost per month of ownership:
/// `maintenance_total / months_since_acquisition`.
///
/// `None` when the elapsed month count is unknown (`None`) or zero — never a
/// divide-by-zero panic and never infinity. Integer (floored) KRW.
#[must_use]
pub fn cost_per_month_won(
    maintenance_total_won: i64,
    months_since_acquisition: Option<i64>,
) -> Option<i64> {
    match months_since_acquisition {
        Some(months) if months > 0 => Some(maintenance_total_won / months),
        _ => None,
    }
}

/// Average maintenance cost per operating hour:
/// `maintenance_total / hours`.
///
/// `None` when operating hours are unknown (`None`) or zero — never a
/// divide-by-zero panic and never infinity. Integer (floored) KRW.
#[must_use]
pub fn cost_per_hour_won(maintenance_total_won: i64, hours: Option<i64>) -> Option<i64> {
    match hours {
        Some(hours) if hours > 0 => Some(maintenance_total_won / hours),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquisition_prefers_explicit_over_vehicle_value() {
        let anchor = AcquisitionAnchor::resolve(Some(30_000_000), Some(25_000_000));
        assert_eq!(anchor.amount_won, Some(30_000_000));
        assert_eq!(anchor.basis, AcquisitionBasis::Explicit);
    }

    #[test]
    fn acquisition_falls_back_to_vehicle_value() {
        let anchor = AcquisitionAnchor::resolve(None, Some(25_000_000));
        assert_eq!(anchor.amount_won, Some(25_000_000));
        assert_eq!(anchor.basis, AcquisitionBasis::VehicleValueFallback);
    }

    #[test]
    fn acquisition_none_when_both_absent() {
        let anchor = AcquisitionAnchor::resolve(None, None);
        assert_eq!(anchor.amount_won, None);
        assert_eq!(anchor.basis, AcquisitionBasis::None);
    }

    #[test]
    fn tco_is_acquisition_plus_maintenance() {
        let anchor = AcquisitionAnchor::resolve(Some(30_000_000), None);
        assert_eq!(tco_won(anchor, 4_500_000), 34_500_000);
    }

    #[test]
    fn tco_uses_vehicle_value_fallback_exactly_once() {
        let anchor = AcquisitionAnchor::resolve(None, Some(25_000_000));
        assert_eq!(tco_won(anchor, 1_000_000), 26_000_000);
    }

    #[test]
    fn tco_without_any_acquisition_is_just_maintenance() {
        let anchor = AcquisitionAnchor::resolve(None, None);
        assert_eq!(tco_won(anchor, 2_000_000), 2_000_000);
    }

    #[test]
    fn gross_margin_is_none_until_sold() {
        assert_eq!(gross_margin_won(None, 30_000_000), None);
    }

    #[test]
    fn gross_margin_profit() {
        // sold for 35M, TCO 30M -> +5M
        assert_eq!(
            gross_margin_won(Some(35_000_000), 30_000_000),
            Some(5_000_000)
        );
    }

    #[test]
    fn gross_margin_allows_loss() {
        // sold for 20M, TCO 30M -> -10M (loss is returned, never floored)
        assert_eq!(
            gross_margin_won(Some(20_000_000), 30_000_000),
            Some(-10_000_000)
        );
    }

    #[test]
    fn cost_per_month_divides_when_months_positive() {
        // 12,000,000 over 24 months -> 500,000/month
        assert_eq!(cost_per_month_won(12_000_000, Some(24)), Some(500_000));
    }

    #[test]
    fn cost_per_month_floors_to_integer_krw() {
        // 1,000,000 over 3 months -> 333,333 (floored, no float)
        assert_eq!(cost_per_month_won(1_000_000, Some(3)), Some(333_333));
    }

    #[test]
    fn cost_per_month_none_on_zero_months() {
        assert_eq!(cost_per_month_won(12_000_000, Some(0)), None);
    }

    #[test]
    fn cost_per_month_none_on_unknown_date() {
        assert_eq!(cost_per_month_won(12_000_000, None), None);
    }

    #[test]
    fn cost_per_month_none_on_negative_months() {
        // a date in the future yields a negative span; treat as unknown, not a quotient
        assert_eq!(cost_per_month_won(12_000_000, Some(-3)), None);
    }

    #[test]
    fn cost_per_hour_divides_when_hours_positive() {
        // 6,000,000 over 1,200 hours -> 5,000/hour
        assert_eq!(cost_per_hour_won(6_000_000, Some(1_200)), Some(5_000));
    }

    #[test]
    fn cost_per_hour_none_on_zero_hours() {
        assert_eq!(cost_per_hour_won(6_000_000, Some(0)), None);
    }

    #[test]
    fn cost_per_hour_none_on_null_hours() {
        assert_eq!(cost_per_hour_won(6_000_000, None), None);
    }
}
