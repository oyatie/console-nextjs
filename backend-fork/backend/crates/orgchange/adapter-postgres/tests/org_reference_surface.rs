#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! L5-ORG reference surface: Company/OrgUnit canonical ids + resolution status
//! on preserved `/api/v1/org-entities` (via `PgOrgChangeStore::org_entities`).
//!
//! RED control: a port-shaped OrgUnit binding (region UUID →
//! `org_unit_source_bindings`) must read back as `RESOLVED` with the canonical
//! `orgUnitId`; unbound regions stay `UNBOUND` with a null id. Ambiguous free
//! text never reaches the binding writer (typed UUID seam).
//!
//! Oracle integrity: `org_entities_readback_…` seeds as the migration owner then
//! reads through a genuine `console_rt` pool (NOBYPASSRLS + FORCE RLS). An
//! owner-pool-only green would mask an unarmed `app.current_org` read.

use console_kernel_core::{OrgId, UserId};
use console_orgchange_adapter_postgres::PgOrgChangeStore;
use console_orgchange_adapter_postgres::org_unit_binding::{
    SOURCE_KIND_BRANCH, SOURCE_KIND_REGION, SourceBindingResolution, count_org_units_named,
    ensure_unambiguous_legacy_binding_in_tx, unambiguous_legacy_source_id,
};
use console_orgchange_domain::CanonicalResolutionStatus;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// Inlined from employment_port_as_runtime_role: SET ROLE console_rt on connect.
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
        .expect("connect console_rt-role test pool")
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn org_entities_readback_company_and_bound_org_units(owner_pool: PgPool) {
    let org = seed_org(&owner_pool, "7sx-ref").await;
    let actor = seed_user(&owner_pool, org, "EXECUTIVE").await;
    let group = seed_group_with_member(&owner_pool, org, actor).await;

    // CompanyPort-shaped revise: one company_revisions row ⇒ RESOLVED.
    sqlx::query(
        "INSERT INTO company_revisions \
         (org_id, version, command_id, actor_id, payload_digest, attributes, receipt) \
         VALUES ($1, 1, $2, $3, $4, '{}'::jsonb, '{}'::jsonb)",
    )
    .bind(*org.as_uuid())
    .bind(Uuid::new_v4())
    .bind(*actor.as_uuid())
    .bind(vec![0u8; 32])
    .execute(&owner_pool)
    .await
    .unwrap();

    let region = seed_region(&owner_pool, org, "리전-참조").await;
    let unbound_region = seed_region(&owner_pool, org, "리전-미결").await;
    let branch = seed_branch(&owner_pool, org, region, "지점-참조").await;

    // Port-created unit shape: org_units + revision + source binding.
    let unit = seed_bound_org_unit(
        &owner_pool,
        org,
        actor,
        SOURCE_KIND_REGION,
        region,
        "리전-참조",
    )
    .await;
    let _branch_unit = seed_bound_org_unit(
        &owner_pool,
        org,
        actor,
        SOURCE_KIND_BRANCH,
        branch,
        "지점-참조",
    )
    .await;

    let _ = group; // membership is what org_entities walks
    let runtime_pool = runtime_role_pool(&owner_pool).await;
    console_platform_request_context::scope_org(org, async move {
        let store = PgOrgChangeStore::new(runtime_pool);
        let entities = store.org_entities(actor).await.unwrap();
        let entity = entities
            .iter()
            .find(|e| e.org_id == *org.as_uuid())
            .expect("group member org must appear");

        assert_eq!(entity.company_id, *org.as_uuid());
        assert_eq!(
            entity.company_resolution_status,
            CanonicalResolutionStatus::Resolved,
            "console_rt must see company_revisions when app.current_org is armed"
        );

        let bound_region = entity
            .org_units
            .iter()
            .find(|u| u.source_kind == SOURCE_KIND_REGION && u.source_id == region.to_string())
            .expect("bound region projection");
        assert_eq!(bound_region.org_unit_id, Some(unit));
        assert_eq!(
            bound_region.resolution_status,
            CanonicalResolutionStatus::Resolved
        );

        let gap = entity
            .org_units
            .iter()
            .find(|u| {
                u.source_kind == SOURCE_KIND_REGION && u.source_id == unbound_region.to_string()
            })
            .expect("unbound region stays visible");
        assert!(gap.org_unit_id.is_none());
        assert_eq!(gap.resolution_status, CanonicalResolutionStatus::Unbound);

        let bound_branch = entity
            .org_units
            .iter()
            .find(|u| u.source_kind == SOURCE_KIND_BRANCH && u.source_id == branch.to_string())
            .expect("bound branch projection");
        assert_eq!(
            bound_branch.resolution_status,
            CanonicalResolutionStatus::Resolved
        );
    })
    .await;
}

#[test]
fn ambiguous_team_text_never_becomes_source_id() {
    let err = unambiguous_legacy_source_id(SOURCE_KIND_BRANCH, "정비1팀").unwrap_err();
    assert_eq!(err.text, "정비1팀");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn ensure_binds_region_uuid_and_replays(pool: PgPool) {
    let org = seed_org(&pool, "7sx-bind").await;
    let actor = seed_user(&pool, org, "SUPER_ADMIN").await;
    let region = seed_region(&pool, org, "리전-바인딩").await;

    console_platform_request_context::scope_org(org, async move {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org.as_uuid().to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        let first = ensure_unambiguous_legacy_binding_in_tx(
            &mut tx,
            org,
            actor,
            SOURCE_KIND_REGION,
            region,
            serde_json::json!({"name": "리전-바인딩"}),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        let second = ensure_unambiguous_legacy_binding_in_tx(
            &mut tx,
            org,
            actor,
            SOURCE_KIND_REGION,
            region,
            serde_json::json!({"name": "리전-바인딩"}),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        assert_eq!(first, second);
        tx.commit().await.unwrap();

        let units: i64 =
            sqlx::query_scalar("SELECT count(*)::bigint FROM org_units WHERE org_id = $1")
                .bind(*org.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(units, 1);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn named_org_unit_probe_is_ambiguous_when_two_match(pool: PgPool) {
    let org = seed_org(&pool, "7sx-amb").await;
    let actor = seed_user(&pool, org, "SUPER_ADMIN").await;
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(org.as_uuid().to_string())
        .execute(&pool)
        .await
        .unwrap();
    for _ in 0..2 {
        let _ = seed_bound_org_unit(
            &pool,
            org,
            actor,
            SOURCE_KIND_BRANCH,
            Uuid::new_v4(),
            "동명팀",
        )
        .await;
    }
    let resolution = count_org_units_named(&pool, org, "동명팀").await.unwrap();
    assert_eq!(
        resolution,
        SourceBindingResolution::Ambiguous { match_count: 2 }
    );
    assert!(unambiguous_legacy_source_id(SOURCE_KIND_BRANCH, "동명팀").is_err());
}

async fn seed_org(pool: &PgPool, tag: &str) -> OrgId {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("{tag}-{}", &id.simple().to_string()[..8]))
        .bind(format!("Org {tag}"))
        .execute(pool)
        .await
        .unwrap();
    OrgId::from_uuid(id)
}

async fn seed_user(pool: &PgPool, org: OrgId, role: &str) -> UserId {
    let user = UserId::new();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, is_active, org_id) \
         VALUES ($1, $2, $3, true, $4)",
    )
    .bind(*user.as_uuid())
    .bind(format!("7sx-{user}"))
    .bind(vec![role])
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    user
}

async fn seed_group_with_member(pool: &PgPool, org: OrgId, actor: UserId) -> Uuid {
    let group = Uuid::new_v4();
    sqlx::query("INSERT INTO groups (id, slug, name, status) VALUES ($1, $2, $3, 'ACTIVE')")
        .bind(group)
        .bind(format!("g-{}", &group.simple().to_string()[..10]))
        .bind("7sx group")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO group_memberships (group_id, org_id) VALUES ($1, $2)
         ON CONFLICT (org_id) DO UPDATE SET group_id = EXCLUDED.group_id",
    )
    .bind(group)
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE organizations SET group_id = $1 WHERE id = $2")
        .bind(group)
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO group_role_grants (group_id, user_id, group_role, granted_by) \
         VALUES ($1, $2, 'GROUP_ADMIN', NULL)",
    )
    .bind(group)
    .bind(*actor.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    group
}

async fn seed_region(pool: &PgPool, org: OrgId, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(*org.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn seed_branch(pool: &PgPool, org: OrgId, region: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region)
    .bind(name)
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_bound_org_unit(
    pool: &PgPool,
    org: OrgId,
    actor: UserId,
    source_kind: &str,
    legacy_id: Uuid,
    name: &str,
) -> Uuid {
    let unit: Uuid = sqlx::query_scalar("INSERT INTO org_units (org_id) VALUES ($1) RETURNING id")
        .bind(*org.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO org_unit_revisions \
         (org_id, org_unit_id, version, command_id, actor_id, payload_digest, attributes, receipt) \
         VALUES ($1, $2, 1, $3, $4, $5, $6, '{}'::jsonb)",
    )
    .bind(*org.as_uuid())
    .bind(unit)
    .bind(Uuid::new_v4())
    .bind(*actor.as_uuid())
    .bind(vec![0u8; 32])
    .bind(serde_json::json!({ "name": name }))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO org_unit_source_bindings \
         (org_id, source_kind, source_id, org_unit_id, actor_id, payload_digest) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(*org.as_uuid())
    .bind(source_kind)
    .bind(legacy_id.to_string())
    .bind(unit)
    .bind(*actor.as_uuid())
    .bind(vec![0u8; 32])
    .execute(pool)
    .await
    .unwrap();
    unit
}
