#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Region-active guard for the default HQ branch resolver (console-lx6 review).
//!
//! `ensure_default_hq_branch` (master-list import) and `ensure_hq_branch_in_tx`
//! (org-wide customer/site create) create the `HQ` branch on first use. Before
//! the guard, a DEACTIVATED `HQ` region still accepted the branch INSERT, so the
//! import stranded equipment under a region hidden from every org-tree read. The
//! resolver must now return a `Conflict` instead.

use std::path::PathBuf;

use console_kernel_core::{ErrorKind, OrgId, UserId};
use console_registry_adapter_postgres::{PgRegistryError, PgRegistryStore};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

fn master_list_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../docs/reference/master-list_251120.xlsx")
}

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

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn import_master_list_against_deactivated_hq_region_is_conflict(owner_pool: PgPool) {
    let knl = OrgId::knl();
    // Seed the tenant org + importing admin as the BYPASSRLS owner.
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, 'knl', 'KNL') \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(*knl.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    let actor = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*actor.as_uuid())
        .bind("Import Admin")
        .bind(Vec::from(["ADMIN"]))
        .bind(*knl.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
    // The HQ region already exists but is DEACTIVATED, so the default-HQ branch
    // resolution would strand imported rows under a hidden region.
    sqlx::query("INSERT INTO regions (name, org_id, deactivated_at) VALUES ('HQ', $1, now())")
        .bind(*knl.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let bytes = std::fs::read(master_list_path()).unwrap();

    let result = console_platform_request_context::scope_org(knl, async {
        let store = PgRegistryStore::new(rt_pool.clone());
        store
            .import_master_list_bytes(actor, "master-list_251120.xlsx", &bytes)
            .await
    })
    .await;

    match result {
        Err(PgRegistryError::Domain(err)) => assert_eq!(
            err.kind,
            ErrorKind::Conflict,
            "importing under a deactivated HQ region must be refused (409)"
        ),
        other => panic!("expected region-active conflict, got {other:?}"),
    }
}
