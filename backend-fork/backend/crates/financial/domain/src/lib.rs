//! Financial domain.
//!
//! Pure rental quote math, residual recomputation, purchase FSM rules, and
//! asset lifecycle / total-cost-of-ownership math.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;

pub mod tco;

pub use tco::{
    AcquisitionAnchor, AcquisitionBasis, cost_per_hour_won, cost_per_month_won, gross_margin_won,
    tco_won,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DepreciationMethod {
    StraightLine,
    DecliningBalance,
}

impl DepreciationMethod {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::StraightLine => "STRAIGHT_LINE",
            Self::DecliningBalance => "DECLINING_BALANCE",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "STRAIGHT_LINE" => Ok(Self::StraightLine),
            "DECLINING_BALANCE" => Ok(Self::DecliningBalance),
            other => Err(KernelError::validation(format!(
                "unknown depreciation method {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct MoneyInput(i64);

impl MoneyInput {
    #[must_use]
    pub const fn won(amount: i64) -> Self {
        Self(amount)
    }

    #[must_use]
    pub const fn amount(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FinancialConfig {
    pub depreciation_method: DepreciationMethod,
    pub useful_life_months: u32,
    pub residual_rate_bps: i32,
    pub declining_balance_rate_bps: i32,
    pub management_fee_rate_bps: i32,
    pub profit_rate_bps: i32,
    pub floor_negative_quote_residual: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RentalQuoteInput {
    pub acquisition_value: MoneyInput,
    pub current_residual_value: MoneyInput,
    pub cumulative_repair_cost: MoneyInput,
    pub config: FinancialConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QuoteLine {
    pub code: String,
    pub label: String,
    pub amount: MoneyInput,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ComputedRentalQuote {
    pub effective_residual_value: MoneyInput,
    pub residual_was_floored: bool,
    pub lines: Vec<QuoteLine>,
    pub monthly_total: MoneyInput,
}

impl ComputedRentalQuote {
    #[must_use]
    pub fn line(&self, code: &str) -> Option<&QuoteLine> {
        self.lines.iter().find(|line| line.code == code)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResidualRecomputeInput {
    pub acquisition_value: MoneyInput,
    pub months_elapsed: u32,
    pub cumulative_cost: MoneyInput,
    pub config: FinancialConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PurchaseStatus {
    StatementAttached,
    RequestSubmitted,
    AdminApproved,
    ExecutivePending,
    ReadyToExecute,
    Executed,
    Rejected,
}

impl PurchaseStatus {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::StatementAttached => "STATEMENT_ATTACHED",
            Self::RequestSubmitted => "REQUEST_SUBMITTED",
            Self::AdminApproved => "ADMIN_APPROVED",
            Self::ExecutivePending => "EXECUTIVE_PENDING",
            Self::ReadyToExecute => "READY_TO_EXECUTE",
            Self::Executed => "EXECUTED",
            Self::Rejected => "REJECTED",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "STATEMENT_ATTACHED" => Ok(Self::StatementAttached),
            "REQUEST_SUBMITTED" => Ok(Self::RequestSubmitted),
            "ADMIN_APPROVED" => Ok(Self::AdminApproved),
            "EXECUTIVE_PENDING" => Ok(Self::ExecutivePending),
            "READY_TO_EXECUTE" => Ok(Self::ReadyToExecute),
            "EXECUTED" => Ok(Self::Executed),
            "REJECTED" => Ok(Self::Rejected),
            other => Err(KernelError::validation(format!(
                "unknown purchase status {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurchaseActor {
    Mechanic,
    Receptionist,
    Admin,
    Executive,
    SuperAdmin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PurchaseTransition {
    pub from: PurchaseStatus,
    pub to: PurchaseStatus,
    pub actor: PurchaseActor,
    pub amount_won: i64,
    pub executive_threshold_won: i64,
}

pub fn compute_rental_quote(input: RentalQuoteInput) -> Result<ComputedRentalQuote, KernelError> {
    validate_financial_config(&input.config)?;
    if input.acquisition_value.amount() < 0 {
        return Err(KernelError::validation(
            "acquisition value must be non-negative",
        ));
    }
    if input.cumulative_repair_cost.amount() < 0 {
        return Err(KernelError::validation(
            "cumulative repair cost must be non-negative",
        ));
    }

    // The PERSISTED effective residual is always floored to >= 0 (the storage
    // column has a CHECK (>= 0)); `residual_was_floored` records that flooring
    // happened whenever the real residual is negative, regardless of the
    // `floor_negative_quote_residual` config flag. The real (possibly negative)
    // residual is preserved separately for audit in
    // `current_residual_value_won`.
    let residual_is_negative = input.current_residual_value.amount() < 0;
    let effective_residual = input.current_residual_value.amount().max(0);
    let residual_was_floored = residual_is_negative;
    // The config flag still governs the COMPUTED quote lines: when flooring is
    // enabled a negative residual is treated as 0 for depreciation; when
    // disabled the negative residual increases the depreciable base.
    let depreciable_residual = if input.config.floor_negative_quote_residual {
        effective_residual
    } else {
        input.current_residual_value.amount()
    };
    let depreciable = input
        .acquisition_value
        .amount()
        .saturating_sub(depreciable_residual)
        .max(0);
    let depreciation = div_ceil_i64(depreciable, i64::from(input.config.useful_life_months));
    let repair_reserve = div_ceil_i64(
        input.cumulative_repair_cost.amount(),
        i64::from(input.config.useful_life_months),
    );
    let subtotal = depreciation.saturating_add(repair_reserve);
    let management_fee = apply_bps_ceil(subtotal, input.config.management_fee_rate_bps)?;
    let profit_base = subtotal.saturating_add(management_fee);
    let profit = apply_bps_ceil(profit_base, input.config.profit_rate_bps)?;

    let lines = vec![
        quote_line("DEPRECIATION", "감가상각비", depreciation),
        quote_line("REPAIR_RESERVE", "수선충당", repair_reserve),
        quote_line("MANAGEMENT_FEE", "관리비", management_fee),
        quote_line("PROFIT", "이윤", profit),
    ];
    let monthly_total = lines.iter().fold(0_i64, |total, line| {
        total.saturating_add(line.amount.amount())
    });

    Ok(ComputedRentalQuote {
        effective_residual_value: MoneyInput::won(effective_residual),
        residual_was_floored,
        lines,
        monthly_total: MoneyInput::won(monthly_total),
    })
}

pub fn recompute_residual_value(input: ResidualRecomputeInput) -> Result<MoneyInput, KernelError> {
    validate_financial_config(&input.config)?;
    if input.acquisition_value.amount() < 0 {
        return Err(KernelError::validation(
            "acquisition value must be non-negative",
        ));
    }
    if input.cumulative_cost.amount() < 0 {
        return Err(KernelError::validation(
            "cumulative cost must be non-negative",
        ));
    }

    let acquisition = input.acquisition_value.amount();
    let residual_floor =
        acquisition.saturating_mul(i64::from(input.config.residual_rate_bps)) / 10_000;
    let book_value_before_cost = match input.config.depreciation_method {
        DepreciationMethod::StraightLine => {
            let depreciable = acquisition.saturating_sub(residual_floor).max(0);
            let depreciation = depreciable.saturating_mul(i64::from(input.months_elapsed))
                / i64::from(input.config.useful_life_months);
            acquisition.saturating_sub(depreciation).max(residual_floor)
        }
        DepreciationMethod::DecliningBalance => {
            let mut book_value = acquisition;
            let keep_bps = 10_000_i64 - i64::from(input.config.declining_balance_rate_bps);
            for _ in 0..input.months_elapsed {
                book_value = book_value.saturating_mul(keep_bps) / 10_000;
                if book_value <= residual_floor {
                    book_value = residual_floor;
                    break;
                }
            }
            book_value
        }
    };

    Ok(MoneyInput::won(
        book_value_before_cost.saturating_sub(input.cumulative_cost.amount()),
    ))
}

pub fn validate_purchase_transition(
    transition: PurchaseTransition,
) -> Result<PurchaseStatus, KernelError> {
    if transition.amount_won < 0 {
        return Err(KernelError::validation(
            "purchase amount must be non-negative",
        ));
    }
    if transition.executive_threshold_won < 0 {
        return Err(KernelError::validation(
            "executive approval threshold must be non-negative",
        ));
    }

    let allowed = match (transition.from, transition.to) {
        (PurchaseStatus::StatementAttached, PurchaseStatus::RequestSubmitted) => {
            matches!(
                transition.actor,
                PurchaseActor::Receptionist | PurchaseActor::Admin | PurchaseActor::SuperAdmin
            )
        }
        (PurchaseStatus::RequestSubmitted, PurchaseStatus::AdminApproved) => {
            matches!(
                transition.actor,
                PurchaseActor::Admin | PurchaseActor::SuperAdmin
            )
        }
        (PurchaseStatus::AdminApproved, PurchaseStatus::ReadyToExecute) => {
            transition.amount_won <= transition.executive_threshold_won
                && matches!(
                    transition.actor,
                    PurchaseActor::Receptionist | PurchaseActor::Admin | PurchaseActor::SuperAdmin
                )
        }
        (PurchaseStatus::AdminApproved, PurchaseStatus::ExecutivePending) => {
            transition.amount_won > transition.executive_threshold_won
                && matches!(
                    transition.actor,
                    PurchaseActor::Receptionist | PurchaseActor::Admin | PurchaseActor::SuperAdmin
                )
        }
        (PurchaseStatus::ExecutivePending, PurchaseStatus::ReadyToExecute) => {
            matches!(
                transition.actor,
                PurchaseActor::Executive | PurchaseActor::SuperAdmin
            )
        }
        (PurchaseStatus::ReadyToExecute, PurchaseStatus::Executed) => {
            matches!(
                transition.actor,
                PurchaseActor::Receptionist | PurchaseActor::Admin | PurchaseActor::SuperAdmin
            )
        }
        (
            PurchaseStatus::RequestSubmitted
            | PurchaseStatus::AdminApproved
            | PurchaseStatus::ExecutivePending,
            PurchaseStatus::Rejected,
        ) => matches!(
            transition.actor,
            PurchaseActor::Admin | PurchaseActor::Executive | PurchaseActor::SuperAdmin
        ),
        (PurchaseStatus::Rejected, PurchaseStatus::StatementAttached) => {
            matches!(
                transition.actor,
                PurchaseActor::Receptionist | PurchaseActor::Admin | PurchaseActor::SuperAdmin
            )
        }
        _ => false,
    };

    if allowed {
        Ok(transition.to)
    } else {
        Err(KernelError::conflict(format!(
            "illegal purchase transition {} -> {} for actor {:?}",
            transition.from.as_db_str(),
            transition.to.as_db_str(),
            transition.actor
        )))
    }
}

fn quote_line(code: &str, label: &str, amount: i64) -> QuoteLine {
    QuoteLine {
        code: code.to_owned(),
        label: label.to_owned(),
        amount: MoneyInput::won(amount.max(0)),
    }
}

fn validate_financial_config(config: &FinancialConfig) -> Result<(), KernelError> {
    if config.useful_life_months == 0 {
        return Err(KernelError::validation(
            "useful life months must be positive",
        ));
    }
    validate_bps("residual rate", config.residual_rate_bps)?;
    validate_bps("declining-balance rate", config.declining_balance_rate_bps)?;
    validate_bps("management fee rate", config.management_fee_rate_bps)?;
    validate_bps("profit rate", config.profit_rate_bps)?;
    Ok(())
}

fn validate_bps(field: &str, value: i32) -> Result<(), KernelError> {
    if (0..=10_000).contains(&value) {
        Ok(())
    } else {
        Err(KernelError::validation(format!(
            "{field} must be between 0 and 10000 basis points"
        )))
    }
}

fn apply_bps_ceil(amount: i64, bps: i32) -> Result<i64, KernelError> {
    if amount < 0 {
        return Err(KernelError::validation("basis-point amount is negative"));
    }
    Ok(div_ceil_i64(amount.saturating_mul(i64::from(bps)), 10_000))
}

fn div_ceil_i64(numerator: i64, denominator: i64) -> i64 {
    if numerator <= 0 {
        0
    } else {
        numerator.saturating_add(denominator - 1) / denominator
    }
}
