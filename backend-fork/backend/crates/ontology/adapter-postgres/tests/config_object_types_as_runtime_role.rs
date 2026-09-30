#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proofs that the governed console config objects ride the ONE ontology
//! engine — seeded through the engine (not raw SQL), and getting revision
//! staging (§3.9.0) + fixity for free. Exercised as the genuine non-owner
//! `console_rt` role (FORCE RLS), the only faithful RLS exercise.
//!
//! Proves:
//!   (a) `seed_governed_config_object_types` publishes support_slo_setting +
//!       console_view through the engine, each with a generic `create` action
//!       (instance_revision dispatch) — no bespoke store;
//!   (b) a support_slo_setting instance creates (v1) and stages a v+1 revision;
//!       as-of(t) returns the HISTORICAL v1 while current returns v2 — the
//!       §3.9.0 staging semantics come free from the engine;
//!   (c) a personal-scope console_view saves directly (an ordinary instance) —
//!       team-scope deploy is instead gated by a governance approval (L-GOV),
//!       proven in the governance lane.

use console_ontology_adapter_postgres::PgOntologyStore;
use console_ontology_adapter_postgres::instances::{
    CreateInstance, PgInstanceStore, StageRevision,
};
use console_ontology_adapter_postgres::seed::{
    CONSOLE_VIEW_KEY, SUPPORT_SLO_SETTING_KEY, seed_governed_config_object_types,
};
use console_ontology_domain::ObjectTypeId;

use console_kernel_core::{OrgId, TraceContext, UserId};
use console_platform_request_context::scope_org;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

const ORG_A: Uuid = Uuid::from_u128(0x5555_5555_5555_5555_5555_5555_5555_5555);
const AT_V1: time::OffsetDateTime = datetime!(2026-07-10 09:00 UTC);
const AT_V2: time::OffsetDateTime = datetime!(2026-07-10 15:00 UTC);

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
        .bind("Org A")
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

/// Seed both config types through the engine, returning (slo_type_id,
/// console_view_type_id) as published heads.
async fn seed(owner_pool: &PgPool, org: OrgId, actor: UserId) -> (ObjectTypeId, ObjectTypeId) {
    scope_org(org, async {
        let store = PgOntologyStore::new(owner_pool.clone())
            .with_command_pool(command_role_pool(owner_pool).await);
        let published = seed_governed_config_object_types(&store, actor, AT_V1)
            .await
            .expect("seed governed config object types");
        // Prove the generic `create` action rode the engine onto each type.
        for key in [SUPPORT_SLO_SETTING_KEY, CONSOLE_VIEW_KEY] {
            let detail = store.get_object_type(key, None).await.expect("get type");
            assert!(
                detail.actions.iter().any(|a| a.stable_key == "create"),
                "{key} must have the seeded `create` action"
            );
            assert_eq!(
                detail.object_type.lifecycle_state,
                console_ontology_domain::SchemaLifecycleState::Published
            );
        }
        let slo = published
            .iter()
            .find(|s| s.stable_key == SUPPORT_SLO_SETTING_KEY)
            .unwrap()
            .id;
        let view = published
            .iter()
            .find(|s| s.stable_key == CONSOLE_VIEW_KEY)
            .unwrap()
            .id;
        (slo, view)
    })
    .await
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn slo_setting_instance_creates_and_stages_v2(owner_pool: PgPool) {
    let org = OrgId::from_uuid(ORG_A);
    let actor = seed_org_and_user(&owner_pool, ORG_A).await;
    let (slo_type, _view_type) = seed(&owner_pool, org, actor).await;

    let rt = runtime_role_pool(&owner_pool).await;
    let instances = PgInstanceStore::new(rt.clone());

    // v1: SLA of 60 minutes for incidents in business hours.
    let created = scope_org(org, async {
        instances
            .create_instance(
                actor,
                CreateInstance {
                    object_type_id: slo_type,
                    title: "incident SLO".to_owned(),
                    attributes: serde_json::json!({
                        "ticket_type": "incident",
                        "threshold_minutes": 60,
                        "window": "business_hours",
                        "escalation_target": "role:duty_manager"
                    }),
                    valid_from: Some(AT_V1),
                    action_type_id: None,
                    reason: Some("initial SLO".to_owned()),
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

    // v2: tighten to 30 minutes, effective later.
    let staged = scope_org(org, async {
        instances
            .stage_revision(
                actor,
                instance_id,
                StageRevision {
                    attributes: serde_json::json!({
                        "ticket_type": "incident",
                        "threshold_minutes": 30,
                        "window": "business_hours",
                        "escalation_target": "role:duty_manager"
                    }),
                    valid_from: Some(AT_V2),
                    action_type_id: None,
                    reason: Some("tighten SLO".to_owned()),
                },
                TraceContext::generate(),
                AT_V2,
            )
            .await
    })
    .await
    .expect("stage v2");
    assert_eq!(staged.revision.version, 2);

    // §3.9.0 staging: as-of just after v1 returns the HISTORICAL v1 threshold…
    let as_of_v1 = scope_org(org, async {
        instances
            .get_as_of(instance_id, AT_V1 + time::Duration::hours(1))
            .await
    })
    .await
    .expect("as-of v1");
    assert_eq!(as_of_v1.revision.version, 1);
    assert_eq!(as_of_v1.revision.attributes["threshold_minutes"], 60);

    // …while current returns the staged v2.
    let current = scope_org(org, async { instances.get_current(instance_id).await })
        .await
        .expect("current");
    assert_eq!(current.revision.version, 2);
    assert_eq!(current.revision.attributes["threshold_minutes"], 30);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn console_view_personal_scope_saves_directly(owner_pool: PgPool) {
    let org = OrgId::from_uuid(ORG_A);
    let actor = seed_org_and_user(&owner_pool, ORG_A).await;
    let (_slo_type, view_type) = seed(&owner_pool, org, actor).await;

    let rt = runtime_role_pool(&owner_pool).await;
    let instances = PgInstanceStore::new(rt.clone());

    // A personal-scope console_view is an ordinary instance: it saves directly,
    // no approval linkage required (team scope is gated by L-GOV approvals).
    let created = scope_org(org, async {
        instances
            .create_instance(
                actor,
                CreateInstance {
                    object_type_id: view_type,
                    title: "my ops dashboard".to_owned(),
                    attributes: serde_json::json!({
                        "screen_key": "ops.dashboard",
                        "config": {"columns": ["id", "status"], "sort": "status"},
                        "scope": "personal"
                    }),
                    valid_from: Some(AT_V1),
                    action_type_id: None,
                    reason: None,
                },
                TraceContext::generate(),
                AT_V1,
            )
            .await
    })
    .await
    .expect("personal console_view saves directly");
    assert_eq!(created.revision.attributes["scope"], "personal");
    assert_eq!(created.revision.version, 1);
}
