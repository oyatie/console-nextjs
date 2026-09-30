//! Branch scoping. Authorization default-denies across branches; a scope is
//! the set of branches a principal may touch.

use std::collections::BTreeSet;

use crate::ids::BranchId;

/// The set of branches a principal is allowed to act within.
///
/// `All` is reserved for `SUPER_ADMIN`/`EXECUTIVE` rollup access; everyone
/// else carries an explicit branch set (from `UserBranch` memberships).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "branches", rename_all = "snake_case")]
pub enum BranchScope {
    All,
    Branches(BTreeSet<BranchId>),
}

impl BranchScope {
    #[must_use]
    pub fn single(branch: BranchId) -> Self {
        Self::Branches(BTreeSet::from([branch]))
    }

    #[must_use]
    pub fn allows(&self, branch: BranchId) -> bool {
        match self {
            Self::All => true,
            Self::Branches(set) => set.contains(&branch),
        }
    }

    /// Return the scope both inputs allow.
    ///
    /// Used when a live DB membership scope is further narrowed by a
    /// claim-level hierarchy [`AccessScope`](crate::AccessScope). This function
    /// never widens: `All` behaves as the identity, and two explicit scopes keep
    /// only their shared branch ids.
    #[must_use]
    pub fn intersect(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::All, Self::All) => Self::All,
            (Self::All, Self::Branches(branches)) | (Self::Branches(branches), Self::All) => {
                Self::Branches(branches.clone())
            }
            (Self::Branches(left), Self::Branches(right)) => {
                Self::Branches(left.intersection(right).copied().collect())
            }
        }
    }

    /// An empty explicit scope: allows nothing. The safe default for a
    /// principal with no memberships yet.
    #[must_use]
    pub fn none() -> Self {
        Self::Branches(BTreeSet::new())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::All => false,
            Self::Branches(set) => set.is_empty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_scope_allows_any_branch() {
        assert!(BranchScope::All.allows(BranchId::new()));
    }

    #[test]
    fn explicit_scope_allows_only_members() {
        let mine = BranchId::new();
        let other = BranchId::new();
        let scope = BranchScope::single(mine);
        assert!(scope.allows(mine));
        assert!(!scope.allows(other));
    }

    #[test]
    fn empty_scope_denies_everything() {
        let scope = BranchScope::none();
        assert!(scope.is_empty());
        assert!(!scope.allows(BranchId::new()));
    }

    #[test]
    fn intersection_never_widens_scope() {
        let branch = BranchId::new();
        let live = BranchScope::All;
        let narrowed = BranchScope::single(branch);

        assert_eq!(live.intersect(&narrowed), BranchScope::single(branch));
        assert_eq!(narrowed.intersect(&live), BranchScope::single(branch));
    }

    #[test]
    fn disjoint_intersection_is_empty_explicit_scope() {
        let left = BranchScope::single(BranchId::new());
        let right = BranchScope::single(BranchId::new());

        assert_eq!(left.intersect(&right), BranchScope::none());
    }

    #[test]
    fn scope_serde_roundtrip() {
        let scope = BranchScope::single(BranchId::new());
        let json = serde_json::to_string(&scope).unwrap();
        let back: BranchScope = serde_json::from_str(&json).unwrap();
        assert_eq!(scope, back);
    }
}
