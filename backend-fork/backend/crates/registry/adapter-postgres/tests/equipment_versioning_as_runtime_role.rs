#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-LC generic versioning, first adoption (registry_equipment), proven as
//! the genuine non-owner runtime role `console_rt`:
//!
//!   (a) every `update_equipment` captures non-destructive versions (the
//!       pre-update content backfills version 1 on first capture);
//!   (b) `rollback_equipment` restores a prior version's content by appending
//!       a NEW `ROLLBACK` version — history is never rewritten — and the live
//!       row actually returns to the old content;
//!   (c) the versions table is append-only (UPDATE/DELETE rejected);
//!   (d) versions are tenant-isolated under RLS.

use console_kernel_core::{BranchId, BranchScope, EquipmentId, OrgId, TraceContext, UserId};
use console_registry_adapter_postgres::PgRegistryStore;
use console_registry_application::{
    RollbackEquipmentCommand, UpdateEquipmentCommand, UpdateEquipmentFields,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

const ORG_B: Uuid = Uuid::from_u128(0x5555_5555_5555_5555_5555_5555_5555_5555);

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

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(owner_pool)
    .await
    .unwrap();
}

async fn seed_branch(owner_pool: &PgPool, org: Uuid) -> BranchId {
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Region {}", Uuid::new_v4()))
            .bind(org)
            .fetch_one(owner_pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("Branch {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_admin(owner_pool: &PgPool, org: Uuid, branch_id: BranchId) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("Admin {}", Uuid::new_v4()))
        .bind(vec!["ADMIN"])
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    user_id
}

fn unique_equipment_no() -> String {
    let n = Uuid::new_v4().as_u128() % 10_000;
    format!("ABC12-{n:04}")
}

async fn seed_equipment(owner_pool: &PgPool, org: Uuid, branch_id: BranchId) -> EquipmentId {
    let customer_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(format!("Customer {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let site_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(format!("Site {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, vehicle_value, residual_value,
            source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, $4, $5, 'A', 'B', 'C', '임대',
                '좌식', '2.5T', 30000000, 12000000,
                'versioning-test', 1, $6)
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(unique_equipment_no())
    .bind(format!("MGMT-{}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    EquipmentId::from_uuid(id)
}

fn update_fields(manager: &str) -> UpdateEquipmentFields {
    UpdateEquipmentFields {
        manager_name: Some(Some(manager.to_owned())),
        ..UpdateEquipmentFields::default()
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn equipment_updates_capture_versions_and_rollback_restores(owner_pool: PgPool) {
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid, "A").await;
    seed_org(&owner_pool, ORG_B, "B").await;
    let branch_id = seed_branch(&owner_pool, org_uuid).await;
    let admin = seed_admin(&owner_pool, org_uuid, branch_id).await;
    let equipment_id = seed_equipment(&owner_pool, org_uuid, branch_id).await;

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let store = PgRegistryStore::new(rt_pool.clone());

    // (a) Two updates → 3 versions (backfilled original + 2 captures).
    let versions = console_platform_request_context::scope_org(org, async {
        store
            .update_equipment(UpdateEquipmentCommand {
                actor: admin,
                branch_scope: BranchScope::All,
                equipment_id,
                fields: update_fields("김철수"),
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-07-01 09:00 UTC),
            })
            .await
            .expect("first update must succeed as console_rt");
        store
            .update_equipment(UpdateEquipmentCommand {
                actor: admin,
                branch_scope: BranchScope::All,
                equipment_id,
                fields: update_fields("이영희"),
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-07-02 09:00 UTC),
            })
            .await
            .expect("second update must succeed as console_rt");
        store
            .list_equipment_versions(equipment_id, &BranchScope::All)
            .await
            .expect("version list must be readable as console_rt")
    })
    .await;

    assert_eq!(versions.len(), 3, "original backfill + 2 captured updates");
    assert_eq!(versions[0].version, 3);
    assert_eq!(
        versions[0]
            .content
            .get("manager_name")
            .and_then(|v| v.as_str()),
        Some("이영희")
    );
    assert_eq!(
        versions[2].version, 1,
        "the pre-update original must be backfilled as version 1"
    );
    assert!(
        versions[2].content.get("manager_name").unwrap().is_null(),
        "version 1 preserves the original NULL manager"
    );
    assert!(versions.iter().all(|v| v.status == "CAPTURED"));

    // (b) Rollback to version 1 → NEW version 4, live row restored.
    let new_version = console_platform_request_context::scope_org(org, async {
        store
            .rollback_equipment(RollbackEquipmentCommand {
                actor: admin,
                branch_scope: BranchScope::All,
                equipment_id,
                version: 1,
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-07-03 09:00 UTC),
            })
            .await
            .expect("rollback must succeed as console_rt")
    })
    .await;
    assert_eq!(new_version, 4, "rollback appends, never rewrites");

    let manager: Option<String> =
        sqlx::query_scalar("SELECT manager_name FROM registry_equipment WHERE id = $1")
            .bind(*equipment_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(manager, None, "live row must return to version 1 content");

    let versions = console_platform_request_context::scope_org(org, async {
        store
            .list_equipment_versions(equipment_id, &BranchScope::All)
            .await
            .unwrap()
    })
    .await;
    assert_eq!(versions.len(), 4);
    assert_eq!(versions[0].status, "ROLLBACK");
    assert_eq!(versions[0].source_version, Some(1));

    // Rollback is audited.
    let rollback_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events \
         WHERE action = 'registry.equipment.rollback' AND target_id = $1",
    )
    .bind(equipment_id.as_uuid().to_string())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(rollback_audits, 1, "rollback must be audited");

    // (c) Append-only protection.
    let rewrite = sqlx::query("UPDATE registry_equipment_versions SET status = 'CAPTURED'")
        .execute(&owner_pool)
        .await;
    assert!(rewrite.is_err(), "version UPDATE must be rejected");
    let delete = sqlx::query("DELETE FROM registry_equipment_versions")
        .execute(&owner_pool)
        .await;
    assert!(delete.is_err(), "version DELETE must be rejected");

    // (d) Cross-org isolation: org B sees no versions for org A's asset.
    let foreign = console_platform_request_context::scope_org(OrgId::from_uuid(ORG_B), async {
        store
            .list_equipment_versions(equipment_id, &BranchScope::All)
            .await
    })
    .await;
    assert!(
        foreign.is_err(),
        "org B must not even resolve org A's equipment (not found under RLS)"
    );
}
