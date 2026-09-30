#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the equipment master-list import.
//!
//! The reference-fixture import (`import_master_list_bytes`) is exercised here as
//! the genuine non-owner runtime role `console_rt` (NOSUPERUSER, NOBYPASSRLS, FORCE
//! RLS), the only faithful exercise of the tenant policy and the role production
//! actually connects as. The existing `master_list_import.rs` tests run on the
//! default `#[sqlx::test]` pool, which connects as a BYPASSRLS superuser and so
//! masks any unarmed-org write in the import path: every `org_isolation` policy
//! keys on the per-transaction GUC `app.current_org`, and an unset GUC fails
//! closed (INSERT WITH CHECK rejected, SELECT returns nothing).
//!
//! This is the documented rls-verify-as-runtime-role failure mode: the whole
//! import path — `ensure_default_hq_branch` (HQ region/branch creation) plus the
//! customer/site/equipment upserts and the audit row — must run inside an
//! org-armed transaction so the FORCE-RLS WITH CHECK passes as `console_rt`.
//!
//! Asserts that, under org-KNL's armed GUC, the full reference fixture imports
//! every valid row into `registry_equipment` for the armed org, with an audit
//! row, as `console_rt`. Before the arming fix this import 500s (every upsert
//! rejected by FORCE RLS); after it, it succeeds.

use std::path::PathBuf;

use console_kernel_core::{OrgId, UserId};
use console_registry_adapter_postgres::PgRegistryStore;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

fn master_list_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../docs/reference/master-list_251120.xlsx")
}

/// The reference fixture's valid-row count (444 main + 47 spare, folded by the
/// parser): every row must land in `registry_equipment` for the armed org. Kept
/// in sync with `master_list_import.rs`'s superuser assertion.
const EXPECTED_ROWS: i64 = 445;

/// Runtime-role pool: every connection becomes the genuine non-owner `console_rt`
/// (NOBYPASSRLS, subject to FORCE RLS), exactly like production. Mirrors the
/// financial/workorder runtime-role RLS tests.
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

/// Count rows as the armed tenant under FORCE RLS (a bare query would fail
/// closed). Arms `app.current_org` transaction-locally, exactly like the app.
async fn count_as_armed_tenant(pool: &PgPool, org: OrgId, sql: &'static str) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(sql)
        .fetch_one(tx.as_mut())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    count
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn import_master_list_as_runtime_role(owner_pool: PgPool) {
    // Seed the tenant org + the importing admin as the BYPASSRLS owner; the
    // import itself runs as console_rt. The audit row's `actor` FKs to `users`, so the
    // actor must be a real user in the org (exactly as the REST handler passes
    // `principal.user_id`).
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, 'knl', 'KNL') \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(*OrgId::knl().as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();
    let actor = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*actor.as_uuid())
        .bind("Import Admin")
        .bind(Vec::from(["ADMIN"]))
        .bind(*OrgId::knl().as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let bytes = std::fs::read(master_list_path()).unwrap();

    let report = console_platform_request_context::scope_org(OrgId::knl(), async {
        let store = PgRegistryStore::new(rt_pool.clone());
        store
            .import_master_list_bytes(actor, "master-list_251120.xlsx", &bytes)
            .await
    })
    .await
    .expect("import must succeed as console_rt under org-KNL's armed GUC (FORCE RLS)");

    assert_eq!(
        i64::try_from(report.added).unwrap(),
        EXPECTED_ROWS,
        "every fixture row must be inserted under the runtime role"
    );
    assert_eq!(report.updated, 0);
    assert_eq!(report.unchanged, 0);
    assert!(report.errors.is_empty(), "{:#?}", report.errors);

    // The rows must be visible to the armed tenant as console_rt (RLS allows own org).
    let count = count_as_armed_tenant(
        &rt_pool,
        OrgId::knl(),
        "SELECT COUNT(*) FROM registry_equipment",
    )
    .await;
    assert_eq!(
        count, EXPECTED_ROWS,
        "all imported rows visible to the armed tenant"
    );

    // The import must have written its audit row for the tenant, too.
    let audit_count = count_as_armed_tenant(
        &owner_pool,
        OrgId::knl(),
        "SELECT COUNT(*) FROM audit_events WHERE action = 'registry.import'",
    )
    .await;
    assert_eq!(audit_count, 1, "import must write exactly one audit row");
}
