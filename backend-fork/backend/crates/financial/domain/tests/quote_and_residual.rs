#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_financial_domain::{
    DepreciationMethod, FinancialConfig, MoneyInput, PurchaseActor, PurchaseStatus,
    PurchaseTransition, RentalQuoteInput, ResidualRecomputeInput, compute_rental_quote,
    recompute_residual_value, validate_purchase_transition,
};

#[test]
fn rental_quote_floors_negative_residual_for_nonnegative_line_items() {
    let quote = compute_rental_quote(RentalQuoteInput {
        acquisition_value: MoneyInput::won(20_000_000),
        current_residual_value: MoneyInput::won(-1_250_000),
        cumulative_repair_cost: MoneyInput::won(2_400_000),
        config: FinancialConfig {
            depreciation_method: DepreciationMethod::StraightLine,
            useful_life_months: 60,
            residual_rate_bps: 1_000,
            declining_balance_rate_bps: 250,
            management_fee_rate_bps: 1_000,
            profit_rate_bps: 500,
            floor_negative_quote_residual: true,
        },
    })
    .unwrap();

    assert!(quote.residual_was_floored);
    assert_eq!(quote.effective_residual_value.amount(), 0);
    assert_eq!(quote.line("DEPRECIATION").unwrap().amount.amount(), 333_334);
    assert_eq!(
        quote.line("REPAIR_RESERVE").unwrap().amount.amount(),
        40_000
    );
    assert_eq!(
        quote.line("MANAGEMENT_FEE").unwrap().amount.amount(),
        37_334
    );
    assert_eq!(quote.line("PROFIT").unwrap().amount.amount(), 20_534);
    assert_eq!(quote.monthly_total.amount(), 431_202);
    assert!(quote.lines.iter().all(|line| line.amount.amount() >= 0));
}

#[test]
fn rental_quote_persists_floored_residual_even_when_flooring_flag_disabled() {
    // FIX 5: with flooring disabled, the negative residual still drives a larger
    // depreciable base for the COMPUTED quote lines, but the PERSISTED effective
    // residual is floored to 0 and flagged so it satisfies the DB CHECK (>= 0).
    let quote = compute_rental_quote(RentalQuoteInput {
        acquisition_value: MoneyInput::won(20_000_000),
        current_residual_value: MoneyInput::won(-1_250_000),
        cumulative_repair_cost: MoneyInput::won(2_400_000),
        config: FinancialConfig {
            depreciation_method: DepreciationMethod::StraightLine,
            useful_life_months: 60,
            residual_rate_bps: 1_000,
            declining_balance_rate_bps: 250,
            management_fee_rate_bps: 1_000,
            profit_rate_bps: 500,
            floor_negative_quote_residual: false,
        },
    })
    .unwrap();

    // Persisted residual is floored to 0 and flagged regardless of the flag.
    assert!(quote.residual_was_floored);
    assert_eq!(quote.effective_residual_value.amount(), 0);
    // Depreciation reflects the larger depreciable base (acq - negative residual
    // = 20_000_000 + 1_250_000 = 21_250_000, over 60 months, div-ceil).
    assert_eq!(quote.line("DEPRECIATION").unwrap().amount.amount(), 354_167);
    assert!(quote.lines.iter().all(|line| line.amount.amount() >= 0));
}

#[test]
fn residual_recompute_supports_straight_line_and_declining_balance_without_flooring_negative_data()
{
    let straight = recompute_residual_value(ResidualRecomputeInput {
        acquisition_value: MoneyInput::won(12_000_000),
        months_elapsed: 30,
        cumulative_cost: MoneyInput::won(7_000_000),
        config: FinancialConfig {
            depreciation_method: DepreciationMethod::StraightLine,
            useful_life_months: 60,
            residual_rate_bps: 1_000,
            declining_balance_rate_bps: 2_000,
            management_fee_rate_bps: 0,
            profit_rate_bps: 0,
            floor_negative_quote_residual: true,
        },
    })
    .unwrap();
    assert_eq!(straight.amount(), -400_000);

    let declining = recompute_residual_value(ResidualRecomputeInput {
        acquisition_value: MoneyInput::won(10_000_000),
        months_elapsed: 2,
        cumulative_cost: MoneyInput::won(500_000),
        config: FinancialConfig {
            depreciation_method: DepreciationMethod::DecliningBalance,
            useful_life_months: 60,
            residual_rate_bps: 1_000,
            declining_balance_rate_bps: 2_000,
            management_fee_rate_bps: 0,
            profit_rate_bps: 0,
            floor_negative_quote_residual: true,
        },
    })
    .unwrap();
    assert_eq!(declining.amount(), 5_900_000);
}

#[test]
fn purchase_fsm_requires_admin_then_thresholded_executive_approval_before_execution() {
    assert_eq!(
        validate_purchase_transition(PurchaseTransition {
            from: PurchaseStatus::StatementAttached,
            to: PurchaseStatus::RequestSubmitted,
            actor: PurchaseActor::Receptionist,
            amount_won: 1_500_000,
            executive_threshold_won: 2_000_000,
        })
        .unwrap(),
        PurchaseStatus::RequestSubmitted
    );

    assert!(
        validate_purchase_transition(PurchaseTransition {
            from: PurchaseStatus::RequestSubmitted,
            to: PurchaseStatus::AdminApproved,
            actor: PurchaseActor::Mechanic,
            amount_won: 1_500_000,
            executive_threshold_won: 2_000_000,
        })
        .is_err()
    );

    assert_eq!(
        validate_purchase_transition(PurchaseTransition {
            from: PurchaseStatus::AdminApproved,
            to: PurchaseStatus::ReadyToExecute,
            actor: PurchaseActor::Receptionist,
            amount_won: 1_500_000,
            executive_threshold_won: 2_000_000,
        })
        .unwrap(),
        PurchaseStatus::ReadyToExecute
    );

    assert_eq!(
        validate_purchase_transition(PurchaseTransition {
            from: PurchaseStatus::AdminApproved,
            to: PurchaseStatus::ExecutivePending,
            actor: PurchaseActor::Receptionist,
            amount_won: 3_000_000,
            executive_threshold_won: 2_000_000,
        })
        .unwrap(),
        PurchaseStatus::ExecutivePending
    );
}
