#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proofs that the 7 niche instance-backed config types (BE-niche-seeds:
//! sla_setting, handover_policy, shift_timetable, labor_refusal, regulation_param,
//! site_coverage, profitability_analytic) ride the ONE ontology engine, exactly
//! like `support_slo_setting`/`console_view`. Exercised as the genuine non-owner
//! `console_rt` role (FORCE RLS), the only faithful RLS exercise.
//!
//! Proves:
//!   (a) `seed_governed_config_object_types` publishes all 9 types (2 existing +
//!       7 new), each with the generic `create` action, per org, isolated from a
//!       second org's seed;
//!   (b) one representative round-trip (`regulation_param`): create (v1) → stage
//!       v+1 → as-of(t) returns the HISTORICAL v1 while current returns v2 — the
//!       §3.9.0 staging semantics come free from the engine for the new types too.

use console_ontology_adapter_postgres::PgOntologyStore;
use console_ontology_adapter_postgres::instances::{
    CreateInstance, PgInstanceStore, StageRevision,
};
use console_ontology_adapter_postgres::seed::{
    HANDOVER_POLICY_KEY, LABOR_REFUSAL_KEY, PROFITABILITY_ANALYTIC_KEY, REGULATION_PARAM_KEY,
    SHIFT_TIMETABLE_KEY, SITE_COVERAGE_KEY, SLA_SETTING_KEY, seed_governed_config_object_types,
};
use console_ontology_domain::ObjectTypeId;

use console_kernel_core::{OrgId, TraceContext, UserId};
use console_platform_request_context::scope_org;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

const ORG_A: Uuid = Uuid::from_u128(0x6666_6666_6666_6666_6666_6666_6666_6666);
const ORG_B: Uuid = Uuid::from_u128(0x7777_7777_7777_7777_7777_7777_7777_7777);
const AT_V1: time::OffsetDateTime = datetime!(2026-07-10 09:00 UTC);
const AT_V2: time::OffsetDateTime = datetime!(2026-07-10 15:00 UTC);

const NICHE_KEYS: [&str; 7] = [
    SLA_SETTING_KEY,
    HANDOVER_POLICY_KEY,
    SHIFT_TIMETABLE_KEY,
    LABOR_REFUSAL_KEY,
    REGULATION_PARAM_KEY,
    SITE_COVERAGE_KEY,
    PROFITABILITY_ANALYTIC_KEY,
];

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn command_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_ontology_cmd")
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn seed_org_and_user(owner_pool: &PgPool, org: Uuid) -> UserId {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
        .bind(org)
        .bind(format!("org-{}", &org.simple().to_string()[..12]))
        .bind("Org")
        .execute(owner_pool)
        .await
        .unwrap();
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind("Admin")
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    UserId::from_uuid(id)
}

/// All 9 governed config types (SLO/console_view + the 7 niche types), each
/// published with a `create` action — as `console_rt` would see them.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn all_seven_niche_types_seed_published_and_isolated_per_org(owner_pool: PgPool) {
    let org_a = OrgId::from_uuid(ORG_A);
    let org_b = OrgId::from_uuid(ORG_B);
    let actor_a = seed_org_and_user(&owner_pool, ORG_A).await;
    let actor_b = seed_org_and_user(&owner_pool, ORG_B).await;

    // Seed as `console_rt` (FORCE RLS) — the real role `tenant_config_seeder`
    // (app/src/lib.rs) provisions through in production; a superuser pool here
    // would bypass RLS and let org B's publish cross-supersede org A's row.
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let store = PgOntologyStore::new(rt.clone()).with_command_pool(cmd);

    scope_org(org_a, async {
        seed_governed_config_object_types(&store, actor_a, AT_V1)
            .await
            .expect("seed org A")
    })
    .await;

    // Org B seeds independently — a second call would conflict on org A's
    // one-published unique index if the registry weren't org-scoped.
    scope_org(org_b, async {
        seed_governed_config_object_types(&store, actor_b, AT_V1)
            .await
            .expect("seed org B")
    })
    .await;

    for key in NICHE_KEYS {
        // Visible + published under org A, as console_rt (RLS-armed).
        let detail_a = scope_org(org_a, async { store.get_object_type(key, None).await })
            .await
            .unwrap_or_else(|e| panic!("{key} must be visible under org A: {e}"));
        assert!(
            detail_a.actions.iter().any(|a| a.stable_key == "create"),
            "{key} must have the seeded `create` action"
        );
        assert_eq!(
            detail_a.object_type.lifecycle_state,
            console_ontology_domain::SchemaLifecycleState::Published
        );

        // Visible + independently published under org B too — each org's copy
        // is its own row, not shared.
        let detail_b = scope_org(org_b, async { store.get_object_type(key, None).await })
            .await
            .unwrap_or_else(|e| panic!("{key} must be visible under org B: {e}"));
        assert_ne!(
            detail_a.object_type.id, detail_b.object_type.id,
            "{key} must be a distinct row per org (RLS isolation), not shared"
        );
    }
    // Close before sqlx::test's teardown drops the ephemeral test database, so
    // an open connection never races the DROP DATABASE.
    rt.close().await;
}

/// Representative round-trip for a new niche type: create v1 → stage v2 →
/// as-of(t) returns the historical v1, current returns v2.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn regulation_param_instance_creates_and_stages_v2(owner_pool: PgPool) {
    let org = OrgId::from_uuid(ORG_A);
    let actor = seed_org_and_user(&owner_pool, ORG_A).await;

    let type_id: ObjectTypeId = scope_org(org, async {
        let store = PgOntologyStore::new(owner_pool.clone())
            .with_command_pool(command_role_pool(&owner_pool).await);
        let published = seed_governed_config_object_types(&store, actor, AT_V1)
            .await
            .expect("seed governed config object types");
        published
            .iter()
            .find(|s| s.stable_key == REGULATION_PARAM_KEY)
            .expect("regulation_param published")
            .id
    })
    .await;

    let rt = runtime_role_pool(&owner_pool).await;
    let instances = PgInstanceStore::new(rt.clone());

    // v1: 2026 minimum wage.
    let created = scope_org(org, async {
        instances
            .create_instance(
                actor,
                CreateInstance {
                    object_type_id: type_id,
                    title: "2026 최저임금".to_owned(),
                    attributes: serde_json::json!({
                        "param_key": "min_wage",
                        "value": 10030.0,
                        "effective_date": "2026-01-01",
                        "impact_scope": "전사",
                        "impact_note": "시급 인상"
                    }),
                    valid_from: Some(AT_V1),
                    action_type_id: None,
                    reason: Some("고시".to_owned()),
                },
                TraceContext::generate(),
                AT_V1,
            )
            .await
    })
    .await
    .expect("create v1");
    assert_eq!(created.revision.version, 1);
    let instance_id = created.instance.id;

    // v2: 2027 rate, staged ahead of its effective date.
    let staged = scope_org(org, async {
        instances
            .stage_revision(
                actor,
                instance_id,
                StageRevision {
                    attributes: serde_json::json!({
                        "param_key": "min_wage",
                        "value": 10500.0,
                        "effective_date": "2027-01-01",
                        "impact_scope": "전사",
                        "impact_note": "시급 인상"
                    }),
                    valid_from: Some(AT_V2),
                    action_type_id: None,
                    reason: Some("2027 고시".to_owned()),
                },
                TraceContext::generate(),
                AT_V2,
            )
            .await
    })
    .await
    .expect("stage v2");
    assert_eq!(staged.revision.version, 2);

    // as-of just after v1 returns the HISTORICAL v1 value…
    let as_of_v1 = scope_org(org, async {
        instances
            .get_as_of(instance_id, AT_V1 + time::Duration::hours(1))
            .await
    })
    .await
    .expect("as-of v1");
    assert_eq!(as_of_v1.revision.version, 1);
    assert_eq!(as_of_v1.revision.attributes["value"], 10030.0);

    // …while current returns the staged v2.
    let current = scope_org(org, async { instances.get_current(instance_id).await })
        .await
        .expect("current");
    assert_eq!(current.revision.version, 2);
    assert_eq!(current.revision.attributes["value"], 10500.0);
    rt.close().await;
}
