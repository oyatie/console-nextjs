//! Todos domain.
//!
//! Pure value objects only. A todo is an owner-scoped action item with scope
//! chips (person/team/site/entity refs) and object links (kind+id pairs).
//! Both ref lists share one wire shape, [`TodoRef`]: `kind` is a validated
//! free-form string, not an enum, so new object kinds (from the frontend
//! object registry) need no code change or migration.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;
use serde::{Deserialize, Serialize};

const BODY_MAX: usize = 500;
const REF_KIND_MAX: usize = 64;
const REF_ID_MAX: usize = 128;
const REF_LABEL_MAX: usize = 200;
/// Per-list cap (scopes and links each) so a request cannot bloat a row.
pub const REFS_MAX: usize = 20;

/// Owner-facing todo text (matches the DB `body` CHECK).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TodoText(String);

impl TodoText {
    pub fn new(value: impl Into<String>) -> Result<Self, KernelError> {
        let trimmed = value.into().trim().to_owned();
        if trimmed.is_empty() {
            return Err(KernelError::validation("todo text is required"));
        }
        if trimmed.chars().count() > BODY_MAX {
            return Err(KernelError::validation(format!(
                "todo text must be at most {BODY_MAX} characters"
            )));
        }
        Ok(Self(trimmed))
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

/// One scope chip or object link: a reference to a domain object by kind + id,
/// with an optional display label snapshot. Serializes to a JSONB array
/// element and back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoRef {
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl TodoRef {
    /// Validates the non-empty/length invariants the JSONB column cannot
    /// express.
    pub fn validated(self) -> Result<Self, KernelError> {
        let kind = self.kind.trim();
        let id = self.id.trim();
        if kind.is_empty() || kind.chars().count() > REF_KIND_MAX {
            return Err(KernelError::validation(format!(
                "todo ref kind must be 1..={REF_KIND_MAX} characters"
            )));
        }
        if id.is_empty() || id.chars().count() > REF_ID_MAX {
            return Err(KernelError::validation(format!(
                "todo ref id must be 1..={REF_ID_MAX} characters"
            )));
        }
        let label = match self.label {
            Some(label) => {
                let label = label.trim();
                if label.is_empty() {
                    None
                } else if label.chars().count() > REF_LABEL_MAX {
                    return Err(KernelError::validation(format!(
                        "todo ref label must be at most {REF_LABEL_MAX} characters"
                    )));
                } else {
                    Some(label.to_owned())
                }
            }
            None => None,
        };
        Ok(Self {
            kind: kind.to_owned(),
            id: id.to_owned(),
            label,
        })
    }
}

/// Validates a whole ref list (scope chips or object links).
pub fn validated_refs(refs: Vec<TodoRef>, list_name: &str) -> Result<Vec<TodoRef>, KernelError> {
    if refs.len() > REFS_MAX {
        return Err(KernelError::validation(format!(
            "todo {list_name} must contain at most {REFS_MAX} entries"
        )));
    }
    refs.into_iter().map(TodoRef::validated).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_rejects_blank_and_overlong() {
        assert!(TodoText::new("  ").is_err());
        assert!(TodoText::new("x".repeat(BODY_MAX + 1)).is_err());
        assert_eq!(
            TodoText::new(" 지게차 점검 ").unwrap().as_str(),
            "지게차 점검"
        );
    }

    #[test]
    fn ref_validates_and_roundtrips() {
        let valid = TodoRef {
            kind: " workOrder ".into(),
            id: "WO-1024".into(),
            label: Some("  ".into()),
        }
        .validated()
        .unwrap();
        assert_eq!(valid.kind, "workOrder");
        assert_eq!(valid.label, None, "blank label normalizes to None");

        let json = serde_json::to_string(&valid).unwrap();
        assert!(!json.contains("label"), "absent label is omitted");
        let back: TodoRef = serde_json::from_str(&json).unwrap();
        assert_eq!(valid, back);

        assert!(
            TodoRef {
                kind: " ".into(),
                id: "x".into(),
                label: None
            }
            .validated()
            .is_err()
        );
        assert!(
            TodoRef {
                kind: "site".into(),
                id: "".into(),
                label: None
            }
            .validated()
            .is_err()
        );
    }

    #[test]
    fn ref_list_caps_length() {
        let too_many = (0..=REFS_MAX)
            .map(|index| TodoRef {
                kind: "person".into(),
                id: format!("user-{index}"),
                label: None,
            })
            .collect::<Vec<_>>();
        assert!(validated_refs(too_many, "scopes").is_err());
        assert!(validated_refs(Vec::new(), "scopes").unwrap().is_empty());
    }
}
