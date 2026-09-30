//! Finance GL (총계정원장 전표) domain.
//!
//! Pure accounting-voucher rules: the 기표→차대검증→승인→전기→역분개 FSM, the
//! 차/대(debit/credit) balance math behind the balance gate, and the contra-line
//! derivation for a reversal. No persistence or HTTP concerns live here.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;

/// Defines a UUID-backed ID newtype (mirrors `console_kernel_core::typed_id!`, kept
/// local so this new domain does not have to edit the shared kernel id table).
macro_rules! typed_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
            serde::Serialize, serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(uuid::Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4())
            }

            #[must_use]
            pub const fn from_uuid(value: uuid::Uuid) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn as_uuid(&self) -> &uuid::Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(uuid::Uuid::parse_str(s)?))
            }
        }

        impl From<$name> for uuid::Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

typed_id!(
    /// A general-ledger voucher (전표, VC-…).
    VoucherId
);
typed_id!(
    /// One 차/대 line of a voucher.
    VoucherLineId
);

/// Debit (차변) or credit (대변) side of a voucher line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DebitCredit {
    Debit,
    Credit,
}

impl DebitCredit {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Debit => "DEBIT",
            Self::Credit => "CREDIT",
        }
    }

    /// The Korean bookkeeping label (차변/대변).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Debit => "차변",
            Self::Credit => "대변",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "DEBIT" => Ok(Self::Debit),
            "CREDIT" => Ok(Self::Credit),
            other => Err(KernelError::validation(format!(
                "unknown debit/credit side {other:?}"
            ))),
        }
    }

    /// The opposite side — used to build a reversal's contra lines.
    #[must_use]
    pub const fn reversed(self) -> Self {
        match self {
            Self::Debit => Self::Credit,
            Self::Credit => Self::Debit,
        }
    }
}

/// Voucher FSM state. 기표(draft) → 차대검증(balance-checked) → 승인(approved) →
/// 전기(posted) → 역분개(reversed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VoucherStatus {
    Draft,
    BalanceChecked,
    Approved,
    Posted,
    Reversed,
}

impl VoucherStatus {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::BalanceChecked => "BALANCE_CHECKED",
            Self::Approved => "APPROVED",
            Self::Posted => "POSTED",
            Self::Reversed => "REVERSED",
        }
    }

    /// The Korean accounting label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Draft => "기표",
            Self::BalanceChecked => "차대검증",
            Self::Approved => "승인",
            Self::Posted => "전기",
            Self::Reversed => "역분개",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "DRAFT" => Ok(Self::Draft),
            "BALANCE_CHECKED" => Ok(Self::BalanceChecked),
            "APPROVED" => Ok(Self::Approved),
            "POSTED" => Ok(Self::Posted),
            "REVERSED" => Ok(Self::Reversed),
            other => Err(KernelError::validation(format!(
                "unknown voucher status {other:?}"
            ))),
        }
    }

    /// True once the voucher is 전기(posted): its lines are immutable and it may
    /// only be superseded by a reversal.
    #[must_use]
    pub const fn is_terminal_posted(self) -> bool {
        matches!(self, Self::Posted | Self::Reversed)
    }
}

/// Validate one forward FSM edge. Returns the target state on success, a
/// `conflict` error otherwise. The balance gate for the
/// `Draft → BalanceChecked` edge is enforced separately over the voucher's lines
/// (see [`ensure_balanced`]); this function only guards the legal transitions.
pub fn validate_voucher_transition(
    from: VoucherStatus,
    to: VoucherStatus,
) -> Result<VoucherStatus, KernelError> {
    let allowed = matches!(
        (from, to),
        (VoucherStatus::Draft, VoucherStatus::BalanceChecked)
            | (VoucherStatus::BalanceChecked, VoucherStatus::Approved)
            | (VoucherStatus::Approved, VoucherStatus::Posted)
            | (VoucherStatus::Posted, VoucherStatus::Reversed)
    );
    if allowed {
        Ok(to)
    } else {
        Err(KernelError::conflict(format!(
            "illegal voucher transition {} -> {}",
            from.as_db_str(),
            to.as_db_str()
        )))
    }
}

/// The 차/대 totals of a voucher's lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BalanceOutcome {
    pub debit_total_won: i64,
    pub credit_total_won: i64,
}

impl BalanceOutcome {
    /// A voucher is balanced when 차변 = 대변 and the total is strictly positive
    /// (a zero-amount voucher never clears 차대검증).
    #[must_use]
    pub const fn is_balanced(self) -> bool {
        self.debit_total_won == self.credit_total_won && self.debit_total_won > 0
    }
}

/// Sum the 차/대 sides. Every line amount must be strictly positive; sums use
/// checked arithmetic so an overflow fails closed rather than wrapping (money
/// path).
pub fn compute_balance<I>(lines: I) -> Result<BalanceOutcome, KernelError>
where
    I: IntoIterator<Item = (DebitCredit, i64)>,
{
    let mut debit_total_won: i64 = 0;
    let mut credit_total_won: i64 = 0;
    let mut line_count = 0_usize;
    for (side, amount_won) in lines {
        line_count += 1;
        if amount_won <= 0 {
            return Err(KernelError::validation(
                "voucher line amount must be strictly positive",
            ));
        }
        match side {
            DebitCredit::Debit => {
                debit_total_won = debit_total_won
                    .checked_add(amount_won)
                    .ok_or_else(|| KernelError::validation("voucher debit total overflowed i64"))?;
            }
            DebitCredit::Credit => {
                credit_total_won = credit_total_won.checked_add(amount_won).ok_or_else(|| {
                    KernelError::validation("voucher credit total overflowed i64")
                })?;
            }
        }
    }
    if line_count == 0 {
        return Err(KernelError::validation(
            "voucher must have at least one line",
        ));
    }
    Ok(BalanceOutcome {
        debit_total_won,
        credit_total_won,
    })
}

/// Fail closed unless the voucher is balanced — the balance gate that blocks a
/// voucher from advancing past 차대검증.
pub fn ensure_balanced(outcome: BalanceOutcome) -> Result<(), KernelError> {
    if outcome.is_balanced() {
        Ok(())
    } else {
        Err(KernelError::validation(format!(
            "unbalanced voucher cannot advance past 차대검증: 차변 {} != 대변 {}",
            outcome.debit_total_won, outcome.credit_total_won
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fsm_allows_the_forward_chain_only() {
        assert!(
            validate_voucher_transition(VoucherStatus::Draft, VoucherStatus::BalanceChecked)
                .is_ok()
        );
        assert!(
            validate_voucher_transition(VoucherStatus::BalanceChecked, VoucherStatus::Approved)
                .is_ok()
        );
        assert!(
            validate_voucher_transition(VoucherStatus::Approved, VoucherStatus::Posted).is_ok()
        );
        assert!(
            validate_voucher_transition(VoucherStatus::Posted, VoucherStatus::Reversed).is_ok()
        );
        // Skips and backward edges are rejected.
        assert!(validate_voucher_transition(VoucherStatus::Draft, VoucherStatus::Posted).is_err());
        assert!(
            validate_voucher_transition(VoucherStatus::Approved, VoucherStatus::BalanceChecked)
                .is_err()
        );
        assert!(
            validate_voucher_transition(VoucherStatus::Reversed, VoucherStatus::Draft).is_err()
        );
    }

    #[test]
    fn balance_gate_requires_equal_positive_sides() {
        let balanced = compute_balance([
            (DebitCredit::Debit, 10_000),
            (DebitCredit::Credit, 4_000),
            (DebitCredit::Credit, 6_000),
        ])
        .unwrap();
        assert!(balanced.is_balanced());
        ensure_balanced(balanced).unwrap();

        let unbalanced =
            compute_balance([(DebitCredit::Debit, 10_000), (DebitCredit::Credit, 9_999)]).unwrap();
        assert!(!unbalanced.is_balanced());
        assert!(ensure_balanced(unbalanced).is_err());

        // Zero-total (no lines) and non-positive amounts fail closed.
        assert!(compute_balance(std::iter::empty()).is_err());
        assert!(compute_balance([(DebitCredit::Debit, 0)]).is_err());
        assert!(compute_balance([(DebitCredit::Debit, -1)]).is_err());
    }

    #[test]
    fn reversal_of_a_balanced_voucher_nets_to_zero() {
        let lines = [
            (DebitCredit::Debit, 7_000_i64),
            (DebitCredit::Credit, 7_000),
        ];
        let original = compute_balance(lines).unwrap();
        // Contra lines swap sides.
        let contra = compute_balance(lines.map(|(side, amt)| (side.reversed(), amt))).unwrap();
        // Combined: net debit == net credit and, per side, they cancel.
        assert_eq!(
            original.debit_total_won + contra.debit_total_won,
            original.credit_total_won + contra.credit_total_won
        );
        assert_eq!(original.debit_total_won, contra.credit_total_won);
        assert_eq!(original.credit_total_won, contra.debit_total_won);
    }

    #[test]
    fn debit_credit_wire_roundtrips_and_rejects_aliases() {
        for side in [DebitCredit::Debit, DebitCredit::Credit] {
            assert_eq!(DebitCredit::from_db_str(side.as_db_str()).unwrap(), side);
            assert_eq!(side.reversed().reversed(), side);
        }
        assert_eq!(DebitCredit::Debit.as_db_str(), "DEBIT");
        assert_eq!(DebitCredit::Credit.as_db_str(), "CREDIT");
        assert_eq!(DebitCredit::Debit.label(), "차변");
        assert_eq!(DebitCredit::Credit.label(), "대변");
        // Plausible wire aliases and empty/unknown tags fail closed.
        for bad in ["debit", "credit", "DR", "CR", "차변", "대변", "", "UNKNOWN"] {
            assert!(
                DebitCredit::from_db_str(bad).is_err(),
                "expected reject for {bad:?}"
            );
        }
    }

    #[test]
    fn voucher_status_wire_roundtrips_terminal_matrix_and_rejects_unknown() {
        for st in [
            VoucherStatus::Draft,
            VoucherStatus::BalanceChecked,
            VoucherStatus::Approved,
            VoucherStatus::Posted,
            VoucherStatus::Reversed,
        ] {
            assert_eq!(VoucherStatus::from_db_str(st.as_db_str()).unwrap(), st);
        }
        assert_eq!(VoucherStatus::Draft.as_db_str(), "DRAFT");
        assert_eq!(VoucherStatus::BalanceChecked.as_db_str(), "BALANCE_CHECKED");
        assert_eq!(VoucherStatus::Draft.label(), "기표");
        assert_eq!(VoucherStatus::BalanceChecked.label(), "차대검증");
        assert_eq!(VoucherStatus::Approved.label(), "승인");
        assert_eq!(VoucherStatus::Posted.label(), "전기");
        assert_eq!(VoucherStatus::Reversed.label(), "역분개");
        assert!(!VoucherStatus::Draft.is_terminal_posted());
        assert!(!VoucherStatus::BalanceChecked.is_terminal_posted());
        assert!(!VoucherStatus::Approved.is_terminal_posted());
        assert!(VoucherStatus::Posted.is_terminal_posted());
        assert!(VoucherStatus::Reversed.is_terminal_posted());
        for bad in [
            "draft",
            "posted",
            "BALANCECHECKED",
            "balance_checked",
            "POST",
            "",
            "UNKNOWN",
        ] {
            assert!(
                VoucherStatus::from_db_str(bad).is_err(),
                "expected reject for {bad:?}"
            );
        }
    }
}
