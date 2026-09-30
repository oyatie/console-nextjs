#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the console equipment 호기 lookup (#18.5 + #18.6).
//!
//! `find_model_by_management_no` is the adapter twin of the workorder-rest
//! `lookup_equipment` / `autocomplete_equipment` handlers: both arm
//! `app.current_org` transaction-locally (`with_org_conn(current_org()?, ..)`)
//! and both normalize the typed 호기 (strip a leading '#' and a trailing '호기')
//! then match leading-zero-insensitively (`ltrim(management_no,'0') =
//! ltrim($1,'0')`) so the floor-typed '10' / '10호기' / '#10호기' all match the
//! stored zero-padded '010'.
//!
//! This runs as the genuine non-owner runtime role `console_rt` (NOSUPERUSER,
//! NOBYPASSRLS, FORCE RLS) — the only faithful exercise of the tenant policy and
//! the role production connects as. The default `#[sqlx::test]` pool connects as
//! a BYPASSRLS superuser, which sees every row regardless of `app.current_org`
//! and so MASKS the #18.6 bug (a bare-pool read that, unarmed, returns zero rows
//! under FORCE RLS). We SEED as the owner (row_security off) and READ as `console_rt`.
//!
//! Asserts, as `console_rt`, for a single equipment row stored with management_no
//! '010' (model 'DFO30-MODEL'):
//!   (a) FAIL-CLOSED — a raw `console_rt` lookup with NO `app.current_org` armed
//!       returns ZERO rows (FORCE RLS filters every row): the #18.6 prod bug;
//!   (b) ARMED — under the row's org GUC, `find_model_by_management_no` returns
//!       the model (the arming fix);
//!   (c) NORMALIZATION — '10호기', '#10호기', '#010호기' and '10' all return that
//!       row under the armed GUC (#18.5 호기-suffix + leading-zero match), while
//!       a different 호기 ('11') does NOT.

use console_kernel_core::OrgId;
use console_platform_request_context::scope_org;
use console_registry_adapter_postgres::PgRegistryStore;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// The stored, zero-padded management number for 호기 10 (live DB confirmed
/// '010' → equipment_no 'DFO30-0010'). The normalization must match it from the
/// bare '10' the caller types.
const STORED_MANAGEMENT_NO: &str = "010";
const STORED_MODEL: &str = "DFO30-MODEL";

/// Runtime-role pool: every connection becomes the genuine non-owner `console_rt`
/// (NOBYPASSRLS, subject to FORCE RLS), exactly like production. Mirrors the
/// other runtime-role RLS tests in this crate.
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

/// Seed org + HQ region/branch + customer/site + one equipment row
/// (management_no '010', model 'DFO30-MODEL') as the BYPASSRLS owner with
/// row_security off, so the row exists independent of any armed GUC.
async fn seed_equipment(owner_pool: &PgPool, org: Uuid) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, 'knl', 'KNL') \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .execute(&mut *tx)
    .await
    .unwrap();
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ('HQ', $1) RETURNING id")
            .bind(org)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, 'HQ', $2) RETURNING id",
    )
    .bind(region_id)
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let customer_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, 'KNL Customer', $2) RETURNING id",
    )
    .bind(branch_id)
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let site_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, 'KNL Site', $3) RETURNING id",
    )
    .bind(branch_id)
    .bind(customer_id)
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no, model,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, 'DFO30-0010', $4, $5, 'A', 'B', 'C', '임대',
                '좌식', '3.0T', 'hogi-lookup-rls-test', 1, $6)
        "#,
    )
    .bind(branch_id)
    .bind(customer_id)
    .bind(site_id)
    .bind(STORED_MANAGEMENT_NO)
    .bind(STORED_MODEL)
    .bind(org)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// Raw `console_rt` lookup mirroring the handler SQL, run on a tx with NO
/// `app.current_org` armed: under FORCE RLS this MUST return zero rows. This is
/// the exact #18.6 failure mode (a bare-pool read returns nothing in prod).
async fn unarmed_runtime_lookup_count(rt_pool: &PgPool, management_no: &str) -> i64 {
    let mut tx = rt_pool.begin().await.unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM registry_equipment \
         WHERE ltrim(management_no, '0') = ltrim($1, '0')",
    )
    .bind(management_no)
    .fetch_one(tx.as_mut())
    .await
    .unwrap();
    tx.commit().await.unwrap();
    count
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn equipment_hogi_lookup_normalizes_and_is_rls_armed_as_runtime_role(owner_pool: PgPool) {
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_equipment(&owner_pool, org_uuid).await;

    let rt_pool = runtime_role_pool(&owner_pool).await;

    // (a) FAIL-CLOSED: without app.current_org armed, even the stored '010'
    // returns ZERO rows under FORCE RLS as console_rt — the #18.6 prod bug.
    assert_eq!(
        unarmed_runtime_lookup_count(&rt_pool, STORED_MANAGEMENT_NO).await,
        0,
        "unarmed console_rt lookup must return zero rows (FORCE RLS), reproducing #18.6"
    );

    let store = PgRegistryStore::new(rt_pool.clone());

    // (b) ARMED: under the row's org GUC, the lookup returns the model.
    let armed = scope_org(org, store.find_model_by_management_no(STORED_MANAGEMENT_NO))
        .await
        .expect("armed lookup must succeed as console_rt");
    assert_eq!(
        armed.as_deref(),
        Some(STORED_MODEL),
        "with app.current_org armed the row must be visible to console_rt"
    );

    // (c) NORMALIZATION: the floor-typed 호기 forms all resolve to stored '010'.
    for typed in ["10호기", "#10호기", "#010호기", "10", " 10호기 "] {
        let found = scope_org(org, store.find_model_by_management_no(typed))
            .await
            .unwrap_or_else(|e| panic!("lookup of {typed:?} must not error: {e:?}"));
        assert_eq!(
            found.as_deref(),
            Some(STORED_MODEL),
            "typed 호기 {typed:?} must match stored '010' (호기-suffix + leading-zero)"
        );
    }

    // A different 호기 must NOT match (the normalization is exact, not a prefix).
    let other = scope_org(org, store.find_model_by_management_no("11호기"))
        .await
        .expect("lookup of a non-existent 호기 returns Ok(None), not an error");
    assert!(
        other.is_none(),
        "호기 11 must not match stored 호기 10 ('010')"
    );
}
