//! Pure identity/org domain.
//!
//! No I/O lives here — only org-entity value objects (team affiliation) and the
//! field-validation rules shared by the application and adapter layers. Roles
//! themselves are governed by the branch-scoped authorization matrix in
//! `console-platform-authz` and are validated at the REST boundary; the domain layer
//! stays free of that platform dependency to satisfy the layer-boundary gate.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;
use serde::{Deserialize, Serialize};

/// Maximum length (Unicode scalar values) of a user's display name.
pub const MAX_DISPLAY_NAME_CHARS: usize = 200;
/// Maximum length (Unicode scalar values) of a phone string.
pub const MAX_PHONE_CHARS: usize = 40;
/// Maximum length (Unicode scalar values) of a region or branch name.
pub const MAX_ORG_NAME_CHARS: usize = 200;
/// Maximum length (Unicode scalar values) of a directory search term.
pub const MAX_DIRECTORY_SEARCH_CHARS: usize = 200;

/// Field-technician team affiliation (정비/예방), plus the back-office
/// affiliations (관리/접수). Mirrors the `users.team` CHECK constraint in
/// migration 0002. A `null` team (not modeled here) means "no affiliation".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Team {
    /// 정비 — maintenance/repair field team.
    Maintenance,
    /// 예방 — preventive-inspection field team.
    Prevention,
    /// 관리 — back-office management.
    Management,
    /// 접수 — reception desk.
    Reception,
}

impl Team {
    /// The exact string stored in `users.team` (the Korean label the CHECK
    /// constraint enforces).
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Maintenance => "정비",
            Self::Prevention => "예방",
            Self::Management => "관리",
            Self::Reception => "접수",
        }
    }

    /// Parse a `users.team` value back into the enum.
    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "정비" => Ok(Self::Maintenance),
            "예방" => Ok(Self::Prevention),
            "관리" => Ok(Self::Management),
            "접수" => Ok(Self::Reception),
            other => Err(KernelError::validation(format!(
                "unknown team affiliation {other:?}"
            ))),
        }
    }
}

impl std::fmt::Display for Team {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_db_str())
    }
}

/// Trim a required free-text field, rejecting an empty result.
pub fn require_non_empty(value: &str, message: &str) -> Result<String, KernelError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(KernelError::validation(message.to_owned()))
    } else {
        Ok(trimmed.to_owned())
    }
}

/// Enforce a maximum length in Unicode scalar values on an already-trimmed value.
pub fn require_max_chars(value: &str, max: usize, message: &str) -> Result<(), KernelError> {
    if value.chars().count() > max {
        Err(KernelError::validation(message.to_owned()))
    } else {
        Ok(())
    }
}

/// Normalize and validate an optional phone string. Empty/whitespace becomes
/// `None`; a present value is trimmed and length-bounded.
pub fn normalize_optional_phone(phone: Option<&str>) -> Result<Option<String>, KernelError> {
    match phone.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(None),
        Some(value) => {
            require_max_chars(value, MAX_PHONE_CHARS, "phone is too long")?;
            Ok(Some(value.to_owned()))
        }
    }
}

/// Validate a user display name: required, trimmed, length-bounded.
pub fn validate_display_name(display_name: &str) -> Result<String, KernelError> {
    let trimmed = require_non_empty(display_name, "display_name is required")?;
    require_max_chars(&trimmed, MAX_DISPLAY_NAME_CHARS, "display_name is too long")?;
    Ok(trimmed)
}

/// Validate a region/branch name: required, trimmed, length-bounded.
pub fn validate_org_name(name: &str) -> Result<String, KernelError> {
    let trimmed = require_non_empty(name, "name is required")?;
    require_max_chars(&trimmed, MAX_ORG_NAME_CHARS, "name is too long")?;
    Ok(trimmed)
}

/// Normalize an optional people-directory search term.
///
/// Blank input means no search filter. A present term is trimmed and converted
/// to Unicode lowercase so callers can consistently compare it with a
/// case-normalized database expression.
pub fn normalize_directory_search(search: Option<&str>) -> Result<Option<String>, KernelError> {
    match search.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(None),
        Some(value) => {
            require_max_chars(
                value,
                MAX_DIRECTORY_SEARCH_CHARS,
                "directory search is too long",
            )?;
            Ok(Some(value.to_lowercase()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_round_trips_through_db_str() {
        for team in [
            Team::Maintenance,
            Team::Prevention,
            Team::Management,
            Team::Reception,
        ] {
            assert_eq!(Team::from_db_str(team.as_db_str()).unwrap(), team);
        }
    }

    #[test]
    fn unknown_team_is_rejected() {
        assert!(Team::from_db_str("운전").is_err());
    }

    #[test]
    fn display_name_is_trimmed_and_required() {
        assert_eq!(validate_display_name("  Kim  ").unwrap(), "Kim");
        assert!(validate_display_name("   ").is_err());
    }

    #[test]
    fn display_name_length_is_bounded() {
        let too_long = "가".repeat(MAX_DISPLAY_NAME_CHARS + 1);
        assert!(validate_display_name(&too_long).is_err());
    }

    #[test]
    fn optional_phone_normalizes_blank_to_none() {
        assert_eq!(normalize_optional_phone(Some("   ")).unwrap(), None);
        assert_eq!(normalize_optional_phone(Some("")).unwrap(), None);
        assert_eq!(normalize_optional_phone(Some("\t")).unwrap(), None);
        assert_eq!(normalize_optional_phone(Some("\n")).unwrap(), None);
        assert_eq!(normalize_optional_phone(None).unwrap(), None);
        assert_eq!(
            normalize_optional_phone(Some(" 010-1234-5678 ")).unwrap(),
            Some("010-1234-5678".to_owned())
        );
        assert_eq!(
            normalize_optional_phone(Some("\t010-1234-5678\n")).unwrap(),
            Some("010-1234-5678".to_owned())
        );
    }

    #[test]
    fn optional_phone_length_is_bounded() {
        let too_long = "0".repeat(MAX_PHONE_CHARS + 1);
        let err = normalize_optional_phone(Some(&too_long)).unwrap_err();
        assert!(err.message.contains("phone"), "{}", err.message);
        let just_under = "0".repeat(MAX_PHONE_CHARS - 1);
        assert_eq!(
            normalize_optional_phone(Some(&just_under)).unwrap(),
            Some(just_under)
        );
    }

    #[test]
    fn directory_search_is_trimmed_casefolded_and_bounded() {
        assert_eq!(
            normalize_directory_search(Some("  ALIce  ")).unwrap(),
            Some("alice".to_owned())
        );
        assert_eq!(normalize_directory_search(Some("   ")).unwrap(), None);
        let too_long = "a".repeat(MAX_DIRECTORY_SEARCH_CHARS + 1);
        assert!(normalize_directory_search(Some(&too_long)).is_err());
    }
}
