//! Cross-entity access scope. This is the inter-org anchor; existing
//! [`BranchScope`](crate::BranchScope) remains the per-org projection consumed
//! by current authz/repository code.

use crate::{BranchId, BranchScope, OrgId};
use std::collections::BTreeSet;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct ScopeNodeId(uuid::Uuid);

impl ScopeNodeId {
    #[must_use]
    pub const fn from_uuid(value: uuid::Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_uuid(&self) -> &uuid::Uuid {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessScopeLevel {
    Group,
    Org,
    Region,
    Branch,
    Worksite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccessScope {
    pub level: AccessScopeLevel,
    pub node_id: ScopeNodeId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchProjection {
    pub node_id: ScopeNodeId,
    pub org_id: OrgId,
    pub branches: BTreeSet<BranchId>,
}

impl BranchProjection {
    #[must_use]
    pub fn single(node_id: ScopeNodeId, org_id: OrgId, branch_id: BranchId) -> Self {
        Self {
            node_id,
            org_id,
            branches: BTreeSet::from([branch_id]),
        }
    }
}

impl AccessScope {
    #[must_use]
    pub const fn legacy_org(org_id: OrgId) -> Self {
        Self {
            level: AccessScopeLevel::Org,
            node_id: ScopeNodeId::from_uuid(*org_id.as_uuid()),
        }
    }

    #[must_use]
    pub const fn new(level: AccessScopeLevel, node_id: ScopeNodeId) -> Self {
        Self { level, node_id }
    }

    /// Project this inter-org scope to the existing per-org branch filter.
    ///
    /// `Group` returns `All` because callers may only invoke this while fanning
    /// out over resolver-authorized member orgs. Sub-org levels require a
    /// caller-provided projection from the current hierarchy snapshot; missing
    /// or mismatched projections fail closed.
    #[must_use]
    pub fn branch_scope_for_org(
        &self,
        org_id: OrgId,
        projection: Option<&BranchProjection>,
    ) -> BranchScope {
        match self.level {
            AccessScopeLevel::Group => BranchScope::All,
            AccessScopeLevel::Org if self.node_id.as_uuid() == org_id.as_uuid() => BranchScope::All,
            AccessScopeLevel::Org => BranchScope::none(),
            AccessScopeLevel::Region | AccessScopeLevel::Branch | AccessScopeLevel::Worksite => {
                match projection {
                    Some(p) if p.node_id == self.node_id && p.org_id == org_id => {
                        BranchScope::Branches(p.branches.clone())
                    }
                    _ => BranchScope::none(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_org_scope_projects_to_today_behavior() {
        let org = OrgId::new();
        let scope = AccessScope::legacy_org(org);

        assert_eq!(scope.branch_scope_for_org(org, None), BranchScope::All);
        assert_eq!(
            scope.branch_scope_for_org(OrgId::new(), None),
            BranchScope::none()
        );
    }

    #[test]
    fn branch_scope_uses_matching_projection_only() {
        let org = OrgId::new();
        let branch = BranchId::new();
        let node = ScopeNodeId::from_uuid(*branch.as_uuid());
        let scope = AccessScope::new(AccessScopeLevel::Branch, node);
        let projection = BranchProjection::single(node, org, branch);

        assert_eq!(
            scope.branch_scope_for_org(org, Some(&projection)),
            BranchScope::single(branch)
        );
        assert_eq!(
            scope.branch_scope_for_org(OrgId::new(), Some(&projection)),
            BranchScope::none()
        );
        assert_eq!(scope.branch_scope_for_org(org, None), BranchScope::none());
    }

    #[test]
    fn group_scope_projects_to_member_read_scope() {
        let scope = AccessScope::new(
            AccessScopeLevel::Group,
            ScopeNodeId::from_uuid(uuid::Uuid::new_v4()),
        );

        assert_eq!(
            scope.branch_scope_for_org(OrgId::new(), None),
            BranchScope::All
        );
    }
}
