#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Runtime proof for the Benefits vertical.
//!
//! The owner pool used by `sqlx::test` bypasses RLS, so every actual catalog
//! write below runs as production's non-owner `console_rt` role with a transaction
//! scoped org GUC. This covers create/update/tier replacement/condition
//! replacement, logical retirement of children, audit evidence, and cross-tenant
//! denial-by-omission.

use console_benefit_adapter_postgres::PgBenefitCatalogStore;
use console_benefit_application::{
    BenefitCatalogScopeDraft, BenefitConditionDraft, BenefitTierDraft,
    CreateBenefitCatalogItemCommand, ListBenefitCatalogItemsQuery, ReplaceBenefitConditionsCommand,
    ReplaceBenefitTiersCommand, UpdateBenefitCatalogItemCommand, UpdateBenefitCatalogItemFields,
};
use console_benefit_domain::{BenefitCategory, BenefitConditionKind, BenefitConditionOperator};
use console_kernel_core::{BranchScope, OrgId, TraceContext};
use console_platform_db::lifecycle;
use console_platform_request_context::scope_org;
use console_platform_test_support::{
    grant_console_rt, runtime_role_pool, seed_org_and_super_admin,
};
use serde_json::json;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

const OTHER_ORG: Uuid = Uuid::from_u128(0xb3e0_0000_0000_0000_0000_0000_0000_0002);

fn tier(label: &str) -> BenefitTierDraft {
    BenefitTierDraft {
        tier_basis: "employment_type".to_owned(),
        tier_key: "regular".to_owned(),
        value_label: label.to_owned(),
        amount_won: Some(120_000),
        limit_period: Some("MONTH".to_owned()),
        criteria: json!({"employment_type": "regular"}),
        display_order: 0,
    }
}

fn condition(label: &str) -> BenefitConditionDraft {
    BenefitConditionDraft {
        condition_kind: BenefitConditionKind::Org,
        operator: BenefitConditionOperator::Exists,
        condition_key: "employee".to_owned(),
        condition_value: json!({"active": true}),
        display_label: label.to_owned(),
        cedar_policy_ref: None,
        display_order: 0,
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn benefit_catalog_writes_are_rls_scoped_audited_and_retire_children_as_runtime_role(
    owner_pool: PgPool,
) {
    // The production migration grants these already; the sqlx harness creates
    // tables as a different owner, so grant them explicitly for the console_rt proof.
    grant_console_rt(
        &owner_pool,
        &[
            "GRANT SELECT, INSERT, UPDATE ON benefit_code_counters TO console_rt",
            "GRANT SELECT, INSERT, UPDATE ON benefit_catalog_items TO console_rt",
            "GRANT SELECT, INSERT, UPDATE ON benefit_catalog_tiers TO console_rt",
            "GRANT SELECT, INSERT, UPDATE ON benefit_catalog_conditions TO console_rt",
            "GRANT SELECT, INSERT, UPDATE ON object_lifecycles TO console_rt",
            "GRANT SELECT, INSERT ON object_lifecycle_transitions TO console_rt",
            "GRANT SELECT, INSERT ON audit_events TO console_rt",
        ],
    )
    .await;

    let org_a = OrgId::knl();
    let actor_a = seed_org_and_super_admin(&owner_pool, *org_a.as_uuid(), "Benefits A").await;
    let _actor_b = seed_org_and_super_admin(&owner_pool, OTHER_ORG, "Benefits B").await;
    let store = PgBenefitCatalogStore::new(runtime_role_pool(&owner_pool).await);
    let now = OffsetDateTime::now_utc();

    let created = scope_org(org_a, async {
        store
            .create_item(CreateBenefitCatalogItemCommand {
                actor: actor_a,
                branch_scope: BranchScope::All,
                scope: BenefitCatalogScopeDraft::org(),
                category: BenefitCategory::Legal,
                name: "국민연금".to_owned(),
                coverage_label: "전 직원".to_owned(),
                covered_count: Some(12),
                cost_label: "월 120,000원".to_owned(),
                estimated_annual_cost_won: Some(1_440_000),
                employer_rate_bps: Some(450),
                note: None,
                legal_basis: Some("국민연금법".to_owned()),
                related_domain: Some("payroll".to_owned()),
                related_object_id: None,
                effective_on: None,
                retires_on: None,
                display_order: 0,
                metadata: json!({}),
                tiers: vec![tier("정규직 기준")],
                conditions: vec![condition("재직자")],
                trace: TraceContext::generate(),
                occurred_at: now,
            })
            .await
    })
    .await
    .expect("create must succeed as console_rt with the org GUC armed");
    assert_eq!(created.lifecycle.current_state.as_deref(), Some("draft"));
    assert_eq!(created.tiers.len(), 1);
    assert_eq!(created.conditions.len(), 1);
    assert_eq!(
        created.conditions[0].condition_value,
        json!({"active": true})
    );

    let item_id = created.id;
    let refreshed = scope_org(org_a, async {
        store
            .list_items(ListBenefitCatalogItemsQuery {
                branch_scope: BranchScope::All,
                category: Some(BenefitCategory::Legal),
                branch_id: None,
                site_id: None,
                lifecycle_state: None,
                q: None,
                limit: Some(10),
                offset: Some(0),
            })
            .await
    })
    .await
    .expect("list must hydrate the materialized generic lifecycle")
    .items;
    assert_eq!(refreshed.len(), 1);
    assert_eq!(
        refreshed[0].lifecycle.current_state.as_deref(),
        Some("draft")
    );

    // Exercise the same generic lifecycle engine used by the REST transition
    // endpoint as the non-owner runtime role, then prove a fresh catalog read
    // hydrates the committed state.
    let mut lifecycle_tx = store.pool().begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org_a.to_string())
        .execute(&mut *lifecycle_tx)
        .await
        .unwrap();
    let transitioned = lifecycle::transition_lifecycle(
        &mut lifecycle_tx,
        *org_a.as_uuid(),
        "benefit_catalog_item",
        *item_id.as_uuid(),
        "pending",
        Some(*actor_a.as_uuid()),
        "benefit catalog review",
        now.date(),
    )
    .await
    .unwrap();
    lifecycle_tx.commit().await.unwrap();
    assert_eq!(transitioned.current_state, "pending");

    let lifecycle_refreshed = scope_org(org_a, async {
        store
            .list_items(ListBenefitCatalogItemsQuery {
                branch_scope: BranchScope::All,
                category: Some(BenefitCategory::Legal),
                branch_id: None,
                site_id: None,
                lifecycle_state: None,
                q: None,
                limit: Some(10),
                offset: Some(0),
            })
            .await
    })
    .await
    .expect("list must reflect the generic lifecycle transition")
    .items;
    assert_eq!(
        lifecycle_refreshed[0].lifecycle.current_state.as_deref(),
        Some("pending")
    );

    let updated = scope_org(org_a, async {
        store
            .update_item(UpdateBenefitCatalogItemCommand {
                actor: actor_a,
                branch_scope: BranchScope::All,
                item_id,
                fields: UpdateBenefitCatalogItemFields {
                    name: Some("국민연금 개정".to_owned()),
                    ..UpdateBenefitCatalogItemFields::default()
                },
                trace: TraceContext::generate(),
                occurred_at: now + time::Duration::seconds(1),
            })
            .await
    })
    .await
    .expect("update must succeed as console_rt");
    assert_eq!(updated.name, "국민연금 개정");

    let replaced_tiers = scope_org(org_a, async {
        store
            .replace_tiers(ReplaceBenefitTiersCommand {
                actor: actor_a,
                branch_scope: BranchScope::All,
                item_id,
                tiers: vec![tier("개정 등급")],
                trace: TraceContext::generate(),
                occurred_at: now + time::Duration::seconds(2),
            })
            .await
    })
    .await
    .expect("tier replacement must succeed as console_rt");
    assert_eq!(replaced_tiers.tiers[0].value_label, "개정 등급");

    let replaced_conditions = scope_org(org_a, async {
        store
            .replace_conditions(ReplaceBenefitConditionsCommand {
                actor: actor_a,
                branch_scope: BranchScope::All,
                item_id,
                conditions: vec![condition("개정 재직자")],
                trace: TraceContext::generate(),
                occurred_at: now + time::Duration::seconds(3),
            })
            .await
    })
    .await
    .expect("condition replacement must succeed as console_rt");
    assert_eq!(
        replaced_conditions.conditions[0].display_label,
        "개정 재직자"
    );
    assert_eq!(
        replaced_conditions.conditions[0].condition_value,
        json!({"active": true})
    );

    let retired_children: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM benefit_catalog_tiers WHERE benefit_id = $1 AND status = 'RETIRED'",
    )
    .bind(*item_id.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        retired_children, 1,
        "replacement must not hard-delete tiers"
    );
    let retired_conditions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM benefit_catalog_conditions WHERE benefit_id = $1 AND status = 'RETIRED'",
    )
    .bind(*item_id.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        retired_conditions, 1,
        "replacement must not hard-delete conditions"
    );

    let audit_actions: Vec<String> = sqlx::query(
        "SELECT action FROM audit_events WHERE target_type = 'benefit_catalog_item' AND target_id = $1 ORDER BY occurred_at",
    )
    .bind(item_id.to_string())
    .fetch_all(&owner_pool)
    .await
    .unwrap()
    .iter()
    .map(|row| row.get("action"))
    .collect();
    assert_eq!(audit_actions.len(), 4, "every catalog write is audited");

    let foreign = scope_org(OrgId::from_uuid(OTHER_ORG), async {
        store
            .list_items(ListBenefitCatalogItemsQuery {
                branch_scope: BranchScope::All,
                category: None,
                branch_id: None,
                site_id: None,
                lifecycle_state: None,
                q: None,
                limit: Some(10),
                offset: Some(0),
            })
            .await
    })
    .await
    .expect("foreign-tenant list itself must remain safe");
    assert!(
        foreign.items.is_empty(),
        "RLS must hide tenant A's catalog from tenant B"
    );
}
