//! Notice-board (게시판 NT- 공지) domain.
//!
//! Pure value objects and validation only. Persistence, audit, code
//! issuance, cross-domain notification fan-out, and REST live in outer
//! layers. A notice is a draft -> published document; publishing snapshots
//! recipients and issues its canonical NT- code (outer layers own both).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::{BranchId, KernelError};
use serde::{Deserialize, Serialize};

const TITLE_MAX: usize = 300;
const BODY_MAX: usize = 20_000;

/// A notice's 유형 (typed category, matches the DB `category` CHECK).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeCategory {
    #[default]
    General,
    Legal,
    HrOrder,
    Training,
}

impl NoticeCategory {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Legal => "legal",
            Self::HrOrder => "hr_order",
            Self::Training => "training",
        }
    }

    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value {
            "general" => Ok(Self::General),
            "legal" => Ok(Self::Legal),
            "hr_order" => Ok(Self::HrOrder),
            "training" => Ok(Self::Training),
            other => Err(KernelError::validation(format!(
                "unknown notice category: {other}"
            ))),
        }
    }
}

/// Who a notice publishes to. Branch is the schema's maximum truthful
/// audience granularity (`user_branches` is the only org-membership
/// primitive); the audience is frozen once the notice is published.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NoticeAudience {
    #[default]
    Org,
    Branches(Vec<BranchId>),
}

impl NoticeAudience {
    /// Parse a raw scope + branch id list. `branch_ids` must be non-empty
    /// iff `scope` is `branches` — anything else is fail-closed validation.
    pub fn new(scope: &str, branch_ids: Vec<BranchId>) -> Result<Self, KernelError> {
        match scope {
            "org" => {
                if branch_ids.is_empty() {
                    Ok(Self::Org)
                } else {
                    Err(KernelError::validation(
                        "branch_ids must be empty when audience scope is org",
                    ))
                }
            }
            "branches" => {
                if branch_ids.is_empty() {
                    return Err(KernelError::validation(
                        "branch_ids must be non-empty when audience scope is branches",
                    ));
                }
                let mut ids = branch_ids;
                ids.sort_unstable();
                ids.dedup();
                Ok(Self::Branches(ids))
            }
            other => Err(KernelError::validation(format!(
                "unknown audience scope: {other}"
            ))),
        }
    }

    #[must_use]
    pub fn scope_str(&self) -> &'static str {
        match self {
            Self::Org => "org",
            Self::Branches(_) => "branches",
        }
    }

    #[must_use]
    pub fn branch_ids(&self) -> &[BranchId] {
        match self {
            Self::Org => &[],
            Self::Branches(ids) => ids,
        }
    }
}

/// A notice's title (matches the DB `title` CHECK).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NoticeTitle(String);

impl NoticeTitle {
    pub fn new(value: impl Into<String>) -> Result<Self, KernelError> {
        let trimmed = value.into().trim().to_owned();
        if trimmed.is_empty() {
            return Err(KernelError::validation("notice title is required"));
        }
        if trimmed.chars().count() > TITLE_MAX {
            return Err(KernelError::validation(format!(
                "notice title must be at most {TITLE_MAX} characters"
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

/// A notice's body (matches the DB `body` CHECK).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NoticeBody(String);

impl NoticeBody {
    pub fn new(value: impl Into<String>) -> Result<Self, KernelError> {
        let trimmed = value.into().trim().to_owned();
        if trimmed.is_empty() {
            return Err(KernelError::validation("notice body is required"));
        }
        if trimmed.chars().count() > BODY_MAX {
            return Err(KernelError::validation(format!(
                "notice body must be at most {BODY_MAX} characters"
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

/// A notice's lifecycle status. Publishing is a one-way transition: there is
/// no unpublish (a published notice's receipts are the legal/operational
/// record of what recipients were told, when).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeStatus {
    Draft,
    Published,
}

impl NoticeStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Published => "published",
        }
    }

    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value {
            "draft" => Ok(Self::Draft),
            "published" => Ok(Self::Published),
            other => Err(KernelError::validation(format!(
                "unknown notice status: {other}"
            ))),
        }
    }
}

/// A validated new draft notice, ready for the write port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNotice {
    pub title: NoticeTitle,
    pub body: NoticeBody,
    pub category: NoticeCategory,
    pub audience: NoticeAudience,
}

impl NewNotice {
    pub fn new(
        title: &str,
        body: &str,
        category: NoticeCategory,
        audience: NoticeAudience,
    ) -> Result<Self, KernelError> {
        Ok(Self {
            title: NoticeTitle::new(title)?,
            body: NoticeBody::new(body)?,
            category,
            audience,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_rejects_blank_and_overlong() {
        assert!(NoticeTitle::new("  ").is_err());
        assert!(NoticeTitle::new("x".repeat(TITLE_MAX + 1)).is_err());
        assert_eq!(
            NoticeTitle::new("  전사규정 개정 ").unwrap().as_str(),
            "전사규정 개정"
        );
    }

    #[test]
    fn body_rejects_blank_and_overlong() {
        assert!(NoticeBody::new("").is_err());
        assert!(NoticeBody::new("x".repeat(BODY_MAX + 1)).is_err());
    }

    #[test]
    fn status_roundtrips_and_rejects_unknown() {
        assert_eq!(NoticeStatus::parse("draft").unwrap(), NoticeStatus::Draft);
        assert_eq!(
            NoticeStatus::parse("published").unwrap(),
            NoticeStatus::Published
        );
        assert!(NoticeStatus::parse("archived").is_err());
    }

    #[test]
    fn new_notice_validates_both_fields() {
        let cat = NoticeCategory::General;
        assert!(NewNotice::new("공지", "내용", cat, NoticeAudience::Org).is_ok());
        assert!(NewNotice::new("", "내용", cat, NoticeAudience::Org).is_err());
        assert!(NewNotice::new("공지", "", cat, NoticeAudience::Org).is_err());
    }

    #[test]
    fn category_roundtrips_and_rejects_unknown() {
        for (raw, parsed) in [
            ("general", NoticeCategory::General),
            ("legal", NoticeCategory::Legal),
            ("hr_order", NoticeCategory::HrOrder),
            ("training", NoticeCategory::Training),
        ] {
            assert_eq!(NoticeCategory::parse(raw).unwrap(), parsed);
            assert_eq!(parsed.as_str(), raw);
        }
        assert!(NoticeCategory::parse("urgent").is_err());
    }

    #[test]
    fn audience_enforces_branch_ids_iff_branches_scope() {
        use console_kernel_core::BranchId;
        assert_eq!(
            NoticeAudience::new("org", vec![]).unwrap(),
            NoticeAudience::Org
        );
        assert!(NoticeAudience::new("org", vec![BranchId::new()]).is_err());
        assert!(NoticeAudience::new("branches", vec![]).is_err());
        assert!(NoticeAudience::new("sites", vec![]).is_err());

        let branch = BranchId::new();
        let audience = NoticeAudience::new("branches", vec![branch, branch]).unwrap();
        assert_eq!(audience.branch_ids(), &[branch], "duplicates collapse");
        assert_eq!(audience.scope_str(), "branches");
    }
}
