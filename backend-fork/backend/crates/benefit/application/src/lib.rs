//! Benefit-catalog application contracts.
//!
//! Commands deliberately omit org/tenant ids. The Postgres adapter derives org
//! scope from the authenticated request context (`current_org`) and arms RLS with
//! `with_org_conn`/`with_audits`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_benefit_domain::{
    BenefitCategory, BenefitConditionKind, BenefitConditionOperator, BenefitScopeKind,
};
use console_kernel_core::{
    AuditAction, AuditEvent, BenefitCatalogConditionId, BenefitCatalogItemId, BenefitCatalogTierId,
    BranchId, BranchScope, KernelError, SiteId, Timestamp, TraceContext, UserId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::Date;
use uuid::Uuid;

pub const BENEFIT_CATALOG_LIFECYCLE_OBJECT_TYPE: &str = "benefit_catalog_item";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BenefitCatalogScopeDraft {
    pub scope_type: BenefitScopeKind,
    pub scope_ref: Option<Uuid>,
    pub branch_id: Option<BranchId>,
    pub site_id: Option<SiteId>,
}

impl BenefitCatalogScopeDraft {
    #[must_use]
    pub const fn org() -> Self {
        Self {
            scope_type: BenefitScopeKind::Org,
            scope_ref: None,
            branch_id: None,
            site_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BenefitCatalogLifecycleBinding {
    pub object_type: String,
    pub object_id: BenefitCatalogItemId,
    pub current_state: Option<String>,
    pub legal_hold: Option<bool>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub retention_until: Option<Timestamp>,
}

impl BenefitCatalogLifecycleBinding {
    #[must_use]
    pub fn new(object_id: BenefitCatalogItemId) -> Self {
        Self {
            object_type: BENEFIT_CATALOG_LIFECYCLE_OBJECT_TYPE.to_owned(),
            object_id,
            current_state: None,
            legal_hold: None,
            retention_until: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenefitCatalogTierView {
    pub id: BenefitCatalogTierId,
    pub benefit_id: BenefitCatalogItemId,
    pub tier_basis: String,
    pub tier_key: String,
    pub value_label: String,
    pub amount_won: Option<i64>,
    pub limit_period: Option<String>,
    pub criteria: Value,
    pub display_order: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenefitCatalogConditionView {
    pub id: BenefitCatalogConditionId,
    pub benefit_id: BenefitCatalogItemId,
    pub condition_kind: BenefitConditionKind,
    pub operator: BenefitConditionOperator,
    pub condition_key: String,
    pub condition_value: Value,
    pub display_label: String,
    pub cedar_policy_ref: Option<String>,
    pub display_order: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenefitCatalogItemView {
    pub id: BenefitCatalogItemId,
    pub benefit_code: String,
    pub category: BenefitCategory,
    pub name: String,
    pub scope: BenefitCatalogScopeDraft,
    pub coverage_label: String,
    pub covered_count: Option<i32>,
    pub cost_label: String,
    pub estimated_annual_cost_won: Option<i64>,
    pub employer_rate_bps: Option<i32>,
    pub note: Option<String>,
    pub legal_basis: Option<String>,
    pub related_domain: Option<String>,
    pub related_object_id: Option<Uuid>,
    pub effective_on: Option<Date>,
    pub retires_on: Option<Date>,
    pub display_order: i32,
    pub metadata: Value,
    pub tiers: Vec<BenefitCatalogTierView>,
    pub conditions: Vec<BenefitCatalogConditionView>,
    pub lifecycle: BenefitCatalogLifecycleBinding,
    pub created_by: UserId,
    pub updated_by: UserId,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenefitCatalogItemPage {
    pub items: Vec<BenefitCatalogItemView>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListBenefitCatalogItemsQuery {
    pub branch_scope: BranchScope,
    pub category: Option<BenefitCategory>,
    pub branch_id: Option<BranchId>,
    pub site_id: Option<SiteId>,
    pub lifecycle_state: Option<String>,
    pub q: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetBenefitCatalogItemQuery {
    pub branch_scope: BranchScope,
    pub item_id: BenefitCatalogItemId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenefitTierDraft {
    pub tier_basis: String,
    pub tier_key: String,
    pub value_label: String,
    pub amount_won: Option<i64>,
    pub limit_period: Option<String>,
    pub criteria: Value,
    pub display_order: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenefitConditionDraft {
    pub condition_kind: BenefitConditionKind,
    pub operator: BenefitConditionOperator,
    pub condition_key: String,
    pub condition_value: Value,
    pub display_label: String,
    pub cedar_policy_ref: Option<String>,
    pub display_order: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateBenefitCatalogItemCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub scope: BenefitCatalogScopeDraft,
    pub category: BenefitCategory,
    pub name: String,
    pub coverage_label: String,
    pub covered_count: Option<i32>,
    pub cost_label: String,
    pub estimated_annual_cost_won: Option<i64>,
    pub employer_rate_bps: Option<i32>,
    pub note: Option<String>,
    pub legal_basis: Option<String>,
    pub related_domain: Option<String>,
    pub related_object_id: Option<Uuid>,
    pub effective_on: Option<Date>,
    pub retires_on: Option<Date>,
    pub display_order: i32,
    pub metadata: Value,
    pub tiers: Vec<BenefitTierDraft>,
    pub conditions: Vec<BenefitConditionDraft>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateBenefitCatalogItemFields {
    pub category: Option<BenefitCategory>,
    pub name: Option<String>,
    pub scope: Option<BenefitCatalogScopeDraft>,
    pub coverage_label: Option<String>,
    pub covered_count: Option<Option<i32>>,
    pub cost_label: Option<String>,
    pub estimated_annual_cost_won: Option<Option<i64>>,
    pub employer_rate_bps: Option<Option<i32>>,
    pub note: Option<Option<String>>,
    pub legal_basis: Option<Option<String>>,
    pub related_domain: Option<Option<String>>,
    pub related_object_id: Option<Option<Uuid>>,
    pub effective_on: Option<Option<Date>>,
    pub retires_on: Option<Option<Date>>,
    pub display_order: Option<i32>,
    pub metadata: Option<Value>,
}

impl UpdateBenefitCatalogItemFields {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.category.is_none()
            && self.name.is_none()
            && self.scope.is_none()
            && self.coverage_label.is_none()
            && self.covered_count.is_none()
            && self.cost_label.is_none()
            && self.estimated_annual_cost_won.is_none()
            && self.employer_rate_bps.is_none()
            && self.note.is_none()
            && self.legal_basis.is_none()
            && self.related_domain.is_none()
            && self.related_object_id.is_none()
            && self.effective_on.is_none()
            && self.retires_on.is_none()
            && self.display_order.is_none()
            && self.metadata.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateBenefitCatalogItemCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub item_id: BenefitCatalogItemId,
    pub fields: UpdateBenefitCatalogItemFields,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplaceBenefitTiersCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub item_id: BenefitCatalogItemId,
    pub tiers: Vec<BenefitTierDraft>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplaceBenefitConditionsCommand {
    pub actor: UserId,
    pub branch_scope: BranchScope,
    pub item_id: BenefitCatalogItemId,
    pub conditions: Vec<BenefitConditionDraft>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

pub fn benefit_catalog_audit_event(
    action: &str,
    actor: Option<UserId>,
    branch_id: Option<BranchId>,
    item_id: BenefitCatalogItemId,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    let mut event = AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        BENEFIT_CATALOG_LIFECYCLE_OBJECT_TYPE,
        item_id.to_string(),
        trace,
        occurred_at,
    );
    if let Some(branch_id) = branch_id {
        event = event.with_branch(branch_id);
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_fields_empty_checks_all_mutable_columns() {
        assert!(UpdateBenefitCatalogItemFields::default().is_empty());
        assert!(
            !UpdateBenefitCatalogItemFields {
                name: Some("건강검진".to_owned()),
                ..UpdateBenefitCatalogItemFields::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn lifecycle_binding_uses_canonical_object_type() {
        let id = BenefitCatalogItemId::new();
        let binding = BenefitCatalogLifecycleBinding::new(id);
        assert_eq!(binding.object_type, BENEFIT_CATALOG_LIFECYCLE_OBJECT_TYPE);
        assert_eq!(binding.object_id, id);
    }
}
