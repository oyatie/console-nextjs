//! Notifications domain.
//!
//! Pure value objects and the deep-link wire contract only. Persistence, audit,
//! REST, and realtime delivery live in outer layers. `category` is deliberately
//! a validated free-form string, not an enum: new producers (결재/멘션/문서/공지/
//! 근태/급여 and beyond) add categories without a code change or migration.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;
use serde::{Deserialize, Serialize};

const CATEGORY_MAX: usize = 64;
const BODY_MAX: usize = 2000;
const KIND_MAX: usize = 64;

/// Extensible notification category (matches the DB `category` CHECK).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NotificationCategory(String);

impl NotificationCategory {
    pub fn new(value: impl Into<String>) -> Result<Self, KernelError> {
        let trimmed = value.into().trim().to_owned();
        if trimmed.is_empty() {
            return Err(KernelError::validation("notification category is required"));
        }
        if trimmed.chars().count() > CATEGORY_MAX {
            return Err(KernelError::validation(format!(
                "notification category must be at most {CATEGORY_MAX} characters"
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

/// Extensible notification behavioral kind (matches the DB `kind` CHECK).
/// Distinct from [`NotificationCategory`] (display grouping, e.g. 결재/멘션):
/// `kind` drives the detect -> assign -> resolve chain — a resolvable kind
/// (e.g. `slo_violation`) is auto-resolved when the matching domain event
/// fires, generically, by matching on the notification's `link`. Defaults to
/// `info` (never resolvable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NotificationKind(String);

impl NotificationKind {
    pub fn new(value: impl Into<String>) -> Result<Self, KernelError> {
        let trimmed = value.into().trim().to_owned();
        if trimmed.is_empty() {
            return Err(KernelError::validation("notification kind is required"));
        }
        if trimmed.chars().count() > KIND_MAX {
            return Err(KernelError::validation(format!(
                "notification kind must be at most {KIND_MAX} characters"
            )));
        }
        Ok(Self(trimmed))
    }

    /// The default, never-auto-resolved kind for a plain informational
    /// notification (결재/멘션/문서/공지/근태/급여 and beyond).
    #[must_use]
    pub fn info() -> Self {
        Self("info".to_owned())
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

/// Recipient-facing notification text (matches the DB `body` CHECK).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NotificationBody(String);

impl NotificationBody {
    pub fn new(value: impl Into<String>) -> Result<Self, KernelError> {
        let trimmed = value.into().trim().to_owned();
        if trimmed.is_empty() {
            return Err(KernelError::validation("notification text is required"));
        }
        if trimmed.chars().count() > BODY_MAX {
            return Err(KernelError::validation(format!(
                "notification text must be at most {BODY_MAX} characters"
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

/// Deep-link target carried by a notification: either a reference to a domain
/// object (`kind` + `id`, e.g. a work order or approval) or a bare app screen.
/// Serializes to the JSONB `link` column and back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NotificationLink {
    Object { kind: String, id: String },
    Screen { screen: String },
}

impl NotificationLink {
    /// Validates the non-empty invariants the JSONB column cannot express.
    pub fn validated(self) -> Result<Self, KernelError> {
        let ok = match &self {
            Self::Object { kind, id } => !kind.trim().is_empty() && !id.trim().is_empty(),
            Self::Screen { screen } => !screen.trim().is_empty(),
        };
        if ok {
            Ok(self)
        } else {
            Err(KernelError::validation(
                "notification link fields must not be empty",
            ))
        }
    }
}

/// One row in `notification_policies` — a recipient-owned routing policy.
/// Lives here (not in kernel ids) because the id never crosses out of the
/// notifications domain: policies are personal `/me/` objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NotificationPolicyId(uuid::Uuid);

impl NotificationPolicyId {
    /// Generates a fresh random ID.
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

impl Default for NotificationPolicyId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for NotificationPolicyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for NotificationPolicyId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(uuid::Uuid::parse_str(s)?))
    }
}

/// What a notification policy targets. Correct-by-construction: a `category`
/// scope always carries a validated category and never a link, an `object`
/// scope always carries a validated link and never a category — mirroring the
/// DB CHECK scope-shape exclusivity so an invalid combination cannot reach SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationPolicyScope {
    All,
    Category(NotificationCategory),
    Object(NotificationLink),
}

impl NotificationPolicyScope {
    /// Builds the scope from the flat wire shape `{scope, category?, link?}`,
    /// rejecting every combination the DB CHECK would reject — at the trust
    /// boundary, so the caller gets a 422 instead of a 500.
    pub fn from_parts(
        scope: &str,
        category: Option<String>,
        link: Option<NotificationLink>,
    ) -> Result<Self, KernelError> {
        match (scope, category, link) {
            ("all", None, None) => Ok(Self::All),
            ("category", Some(category), None) => {
                Ok(Self::Category(NotificationCategory::new(category)?))
            }
            ("object", None, Some(link)) => Ok(Self::Object(link.validated()?)),
            _ => Err(KernelError::validation(
                "policy scope must be 'all' (no target), 'category' (with category, no link), \
                 or 'object' (with link, no category)",
            )),
        }
    }

    #[must_use]
    pub const fn as_scope_str(&self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Category(_) => "category",
            Self::Object(_) => "object",
        }
    }

    #[must_use]
    pub fn category(&self) -> Option<&str> {
        match self {
            Self::Category(category) => Some(category.as_str()),
            Self::All | Self::Object(_) => None,
        }
    }

    #[must_use]
    pub const fn link(&self) -> Option<&NotificationLink> {
        match self {
            Self::Object(link) => Some(link),
            Self::All | Self::Category(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_defaults_to_info_and_rejects_blank_and_overlong() {
        assert_eq!(NotificationKind::info().as_str(), "info");
        assert!(NotificationKind::new("  ").is_err());
        assert!(NotificationKind::new("x".repeat(KIND_MAX + 1)).is_err());
        assert_eq!(
            NotificationKind::new(" slo_violation ").unwrap().as_str(),
            "slo_violation"
        );
    }

    #[test]
    fn category_rejects_blank_and_overlong() {
        assert!(NotificationCategory::new("  ").is_err());
        assert!(NotificationCategory::new("x".repeat(CATEGORY_MAX + 1)).is_err());
        assert_eq!(
            NotificationCategory::new("  결재 ").unwrap().as_str(),
            "결재"
        );
    }

    #[test]
    fn body_rejects_blank_and_overlong() {
        assert!(NotificationBody::new("").is_err());
        assert!(NotificationBody::new("x".repeat(BODY_MAX + 1)).is_err());
    }

    #[test]
    fn policy_scope_enforces_shape_exclusivity() {
        assert_eq!(
            NotificationPolicyScope::from_parts("all", None, None).unwrap(),
            NotificationPolicyScope::All
        );
        let by_category =
            NotificationPolicyScope::from_parts("category", Some("멘션".to_owned()), None).unwrap();
        assert_eq!(by_category.category(), Some("멘션"));
        assert_eq!(by_category.as_scope_str(), "category");
        let link = NotificationLink::Object {
            kind: "approval".into(),
            id: "ap-1".into(),
        };
        let by_object =
            NotificationPolicyScope::from_parts("object", None, Some(link.clone())).unwrap();
        assert_eq!(by_object.link(), Some(&link));

        // Every combination the DB CHECK rejects is rejected here first.
        assert!(NotificationPolicyScope::from_parts("all", Some("멘션".into()), None).is_err());
        assert!(NotificationPolicyScope::from_parts("category", None, None).is_err());
        assert!(
            NotificationPolicyScope::from_parts(
                "category",
                Some("멘션".into()),
                Some(link.clone())
            )
            .is_err()
        );
        assert!(NotificationPolicyScope::from_parts("object", None, None).is_err());
        assert!(NotificationPolicyScope::from_parts("mute-everything", None, None).is_err());
        assert!(
            NotificationPolicyScope::from_parts(
                "object",
                None,
                Some(NotificationLink::Screen { screen: " ".into() })
            )
            .is_err()
        );
    }

    #[test]
    fn link_roundtrips_and_validates() {
        let object = NotificationLink::Object {
            kind: "work_order".into(),
            id: "wo-1".into(),
        };
        let json = serde_json::to_string(&object).unwrap();
        assert!(json.contains("\"type\":\"object\""));
        let back: NotificationLink = serde_json::from_str(&json).unwrap();
        assert_eq!(object, back);

        assert!(
            NotificationLink::Object {
                kind: " ".into(),
                id: "x".into()
            }
            .validated()
            .is_err()
        );
        assert!(
            NotificationLink::Screen {
                screen: "payroll".into()
            }
            .validated()
            .is_ok()
        );
    }
}
