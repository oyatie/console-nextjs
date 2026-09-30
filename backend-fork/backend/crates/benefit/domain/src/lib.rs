//! Pure benefit-catalog domain invariants.
//!
//! This crate owns value-object parsing and validation only. It deliberately has
//! no SQLx, REST, authz, or platform request-context dependency so the benefit
//! storage layer stays clean-architecture compliant.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenefitCategory {
    Legal,
    Extra,
}

impl BenefitCategory {
    /// # Errors
    /// Returns `KernelError::validation` for unknown wire/database values.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "LEGAL" | "legal" => Ok(Self::Legal),
            "EXTRA" | "extra" => Ok(Self::Extra),
            other => Err(KernelError::validation(format!(
                "unknown benefit category {other:?}"
            ))),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Legal => "LEGAL",
            Self::Extra => "EXTRA",
        }
    }

    #[must_use]
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::Legal => "legal",
            Self::Extra => "extra",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BenefitScopeKind {
    Org,
    Branch,
    Site,
    Team,
    Role,
    EmployeeSegment,
}

impl BenefitScopeKind {
    /// # Errors
    /// Returns `KernelError::validation` for unknown scope values.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "ORG" => Ok(Self::Org),
            "BRANCH" => Ok(Self::Branch),
            "SITE" => Ok(Self::Site),
            "TEAM" => Ok(Self::Team),
            "ROLE" => Ok(Self::Role),
            "EMPLOYEE_SEGMENT" => Ok(Self::EmployeeSegment),
            other => Err(KernelError::validation(format!(
                "unknown benefit scope type {other:?}"
            ))),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Org => "ORG",
            Self::Branch => "BRANCH",
            Self::Site => "SITE",
            Self::Team => "TEAM",
            Self::Role => "ROLE",
            Self::EmployeeSegment => "EMPLOYEE_SEGMENT",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BenefitConditionKind {
    Org,
    Branch,
    Site,
    Team,
    Role,
    Position,
    Tenure,
    Age,
    Gender,
    EmploymentType,
    Contract,
    CostCenter,
    Custom,
}

impl BenefitConditionKind {
    /// # Errors
    /// Returns `KernelError::validation` for unknown condition kinds.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "ORG" => Ok(Self::Org),
            "BRANCH" => Ok(Self::Branch),
            "SITE" => Ok(Self::Site),
            "TEAM" => Ok(Self::Team),
            "ROLE" => Ok(Self::Role),
            "POSITION" => Ok(Self::Position),
            "TENURE" => Ok(Self::Tenure),
            "AGE" => Ok(Self::Age),
            "GENDER" => Ok(Self::Gender),
            "EMPLOYMENT_TYPE" => Ok(Self::EmploymentType),
            "CONTRACT" => Ok(Self::Contract),
            "COST_CENTER" => Ok(Self::CostCenter),
            "CUSTOM" => Ok(Self::Custom),
            other => Err(KernelError::validation(format!(
                "unknown benefit condition kind {other:?}"
            ))),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Org => "ORG",
            Self::Branch => "BRANCH",
            Self::Site => "SITE",
            Self::Team => "TEAM",
            Self::Role => "ROLE",
            Self::Position => "POSITION",
            Self::Tenure => "TENURE",
            Self::Age => "AGE",
            Self::Gender => "GENDER",
            Self::EmploymentType => "EMPLOYMENT_TYPE",
            Self::Contract => "CONTRACT",
            Self::CostCenter => "COST_CENTER",
            Self::Custom => "CUSTOM",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenefitConditionOperator {
    Eq,
    In,
    NotIn,
    Gte,
    Lte,
    Range,
    Exists,
    CustomPolicy,
}

impl BenefitConditionOperator {
    /// # Errors
    /// Returns `KernelError::validation` for unknown operators.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "eq" => Ok(Self::Eq),
            "in" => Ok(Self::In),
            "not_in" => Ok(Self::NotIn),
            "gte" => Ok(Self::Gte),
            "lte" => Ok(Self::Lte),
            "range" => Ok(Self::Range),
            "exists" => Ok(Self::Exists),
            "custom_policy" => Ok(Self::CustomPolicy),
            other => Err(KernelError::validation(format!(
                "unknown benefit condition operator {other:?}"
            ))),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Eq => "eq",
            Self::In => "in",
            Self::NotIn => "not_in",
            Self::Gte => "gte",
            Self::Lte => "lte",
            Self::Range => "range",
            Self::Exists => "exists",
            Self::CustomPolicy => "custom_policy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct BenefitCode(String);

impl BenefitCode {
    /// # Errors
    /// Returns `KernelError::validation` unless the code is canonical `BF-0001` style.
    pub fn new(raw: impl Into<String>) -> Result<Self, KernelError> {
        let value = raw.into().trim().to_ascii_uppercase();
        let Some(suffix) = value.strip_prefix("BF-") else {
            return Err(KernelError::validation("benefit code must start with BF-"));
        };
        let valid = suffix.len() >= 4 && suffix.chars().all(|ch| ch.is_ascii_digit());
        if valid {
            Ok(Self(value))
        } else {
            Err(KernelError::validation(
                "benefit code must match ^BF-[0-9]{4,}$",
            ))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for BenefitCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct MoneyWon(i64);

impl MoneyWon {
    /// # Errors
    /// Returns `KernelError::validation` when the amount is negative.
    pub fn new(value: i64) -> Result<Self, KernelError> {
        if value < 0 {
            Err(KernelError::validation("money_won must be non-negative"))
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn value(self) -> i64 {
        self.0
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct RateBasisPoints(i32);

impl RateBasisPoints {
    /// # Errors
    /// Returns `KernelError::validation` unless the value is 0..=10000.
    pub fn new(value: i32) -> Result<Self, KernelError> {
        if (0..=10_000).contains(&value) {
            Ok(Self(value))
        } else {
            Err(KernelError::validation(
                "rate basis points must be between 0 and 10000",
            ))
        }
    }

    #[must_use]
    pub const fn value(self) -> i32 {
        self.0
    }
}

/// # Errors
/// Returns `KernelError::validation` when `value` is blank or exceeds `max_chars`.
pub fn normalize_required_text(
    value: &str,
    max_chars: usize,
    field: &str,
) -> Result<String, KernelError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(KernelError::validation(format!("{field} is required")));
    }
    if trimmed.chars().count() > max_chars {
        return Err(KernelError::validation(format!("{field} is too long")));
    }
    Ok(trimmed.to_owned())
}

/// # Errors
/// Returns `KernelError::validation` when a nonblank value exceeds `max_chars`.
pub fn normalize_optional_text(
    value: Option<String>,
    max_chars: usize,
    field: &str,
) -> Result<Option<String>, KernelError> {
    value
        .map(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else if trimmed.chars().count() > max_chars {
                Err(KernelError::validation(format!("{field} is too long")))
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        })
        .transpose()
        .map(Option::flatten)
}

/// # Errors
/// Returns `KernelError::validation` for non-canonical related domain keys.
pub fn normalize_related_domain(value: Option<String>) -> Result<Option<String>, KernelError> {
    let Some(value) = normalize_optional_text(value, 64, "related_domain")? else {
        return Ok(None);
    };
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Ok(None);
    };
    let valid = first.is_ascii_lowercase()
        && value.len() >= 2
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_');
    if valid {
        Ok(Some(value))
    } else {
        Err(KernelError::validation(
            "related_domain must match ^[a-z][a-z0-9_]{1,63}$",
        ))
    }
}

/// # Errors
/// Returns `KernelError::validation` when the value is not a JSON object.
pub fn validate_metadata_object(value: &Value) -> Result<(), KernelError> {
    if value.is_object() {
        Ok(())
    } else {
        Err(KernelError::validation("metadata must be a JSON object"))
    }
}

/// # Errors
/// Returns `KernelError::validation` unless the condition value is a JSON object.
///
/// The OpenAPI and generated Kotlin, Swift, and TypeScript clients all expose
/// `condition_value` as an object. Keeping the server at the same boundary
/// prevents a write accepted by REST from becoming unreadable by a typed client.
pub fn validate_condition_value(value: &Value) -> Result<(), KernelError> {
    if value.is_object() {
        Ok(())
    } else {
        Err(KernelError::validation(
            "condition_value must be a JSON object",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_and_code_parse_canonical_values() {
        assert_eq!(
            BenefitCategory::parse("legal").unwrap(),
            BenefitCategory::Legal
        );
        assert_eq!(BenefitCategory::Extra.as_db_str(), "EXTRA");
        assert_eq!(BenefitCode::new("bf-0007").unwrap().as_str(), "BF-0007");
        assert!(BenefitCode::new("BF-7").is_err());
    }

    #[test]
    fn rate_money_and_text_validators_reject_invalid_values() {
        assert!(MoneyWon::new(-1).is_err());
        assert!(RateBasisPoints::new(10_001).is_err());
        assert!(normalize_required_text("   ", 10, "name").is_err());
        assert!(normalize_related_domain(Some("Bad Key".to_owned())).is_err());
    }

    #[test]
    fn condition_value_matches_the_typed_openapi_contract() {
        assert!(validate_condition_value(&serde_json::json!(null)).is_err());
        assert!(validate_condition_value(&serde_json::json!("site-a")).is_err());
        assert!(validate_condition_value(&serde_json::json!(["site-a"])).is_err());
        assert!(validate_condition_value(&serde_json::json!({"site": "A"})).is_ok());
    }

    #[test]
    fn benefit_vocab_roundtrips_parse_and_db_str() {
        for cat in [BenefitCategory::Legal, BenefitCategory::Extra] {
            assert_eq!(BenefitCategory::parse(cat.as_db_str()).unwrap(), cat);
            assert_eq!(BenefitCategory::parse(cat.as_wire_str()).unwrap(), cat);
        }
        assert!(BenefitCategory::parse("LEGACY").is_err());

        for scope in [
            BenefitScopeKind::Org,
            BenefitScopeKind::Branch,
            BenefitScopeKind::Site,
            BenefitScopeKind::Team,
            BenefitScopeKind::Role,
            BenefitScopeKind::EmployeeSegment,
        ] {
            assert_eq!(BenefitScopeKind::parse(scope.as_db_str()).unwrap(), scope);
        }
        assert!(BenefitScopeKind::parse("org").is_err());

        for kind in [
            BenefitConditionKind::Org,
            BenefitConditionKind::Tenure,
            BenefitConditionKind::EmploymentType,
            BenefitConditionKind::Custom,
        ] {
            assert_eq!(BenefitConditionKind::parse(kind.as_db_str()).unwrap(), kind);
        }
        assert!(BenefitConditionKind::parse("TENURE_YEARS").is_err());

        for op in [
            BenefitConditionOperator::Eq,
            BenefitConditionOperator::In,
            BenefitConditionOperator::NotIn,
            BenefitConditionOperator::Gte,
            BenefitConditionOperator::Range,
            BenefitConditionOperator::CustomPolicy,
        ] {
            assert_eq!(BenefitConditionOperator::parse(op.as_db_str()).unwrap(), op);
        }
        assert!(BenefitConditionOperator::parse("==").is_err());
    }
}
