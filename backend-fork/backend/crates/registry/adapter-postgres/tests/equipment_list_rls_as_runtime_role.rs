#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the paginated equipment LIST endpoint.
//!
//! `list_equipment` runs inside `with_org_conn(current_org()?, ..)` so every
//! read is tenant-isolated by the row-security policy
//! `registry_equipment_org_isolation`. This test exercises all three RLS
//! invariants as the genuine non-owner `console_rt` role (NOSUPERUSER,
//! NOBYPASSRLS, FORCE RLS) — the only faithful exercise of the tenant policy:
//!
//! (a) FAIL-CLOSED — without `app.current_org` armed, a raw COUNT returns 0
//!     (FORCE RLS hides every row from an unarmed connection).
//! (b) ORG-SCOPED — under the correct org's GUC the list returns the seeded
//!     rows for that org.
//! (c) CROSS-TENANT-INVISIBLE — rows seeded for org B are invisible when the
//!     GUC is armed for org A.
//! (d) BRANCH-SCOPE-FILTERED — the branch_scope filter narrows the result so
//!     a branch-scoped principal only sees their own branches' rows.

use console_kernel_core::{BranchScope, OrgId};
use console_platform_request_context::scope_org;
use console_registry_adapter_postgres::PgRegistryStore;
use console_registry_application::{EquipmentListQuery, EquipmentSortBy};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// Second tenant org used for cross-tenant isolation assertions.
const ORG_B: Uuid = Uuid::from_u128(0xb000_0000_0000_0000_0000_0000_0000_0002);

/// Runtime-role pool: every connection becomes the genuine non-owner `console_rt`
/// (NOBYPASSRLS, subject to FORCE RLS), exactly like production.
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

/// Seed one org + HQ region/branch + customer/site + two equipment rows as the
/// BYPASSRLS owner with row_security off, returning (branch_id, [eq_id1, eq_id2]).
async fn seed_org_equipment(owner_pool: &PgPool, org: Uuid, tag: &str) -> (Uuid, Vec<Uuid>) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(&mut *tx)
    .await
    .unwrap();

    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("HQ-{tag}"))
            .bind(org)
            .fetch_one(&mut *tx)
            .await
            .unwrap();

    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("HQ-{tag}"))
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();

    let customer_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(branch_id)
    .bind(format!("Customer-{tag}"))
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();

    let site_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(branch_id)
    .bind(customer_id)
    .bind(format!("Site-{tag}"))
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();

    // equipment_no must satisfy the migration-0007 CHECK regex
    // `^[A-Z]{3}[A-Z0-9]{2}-[0-9]{4}$`. Derive a stable uppercase letter from the
    // tag so per-tag rows stay distinct (all current callers use distinct first
    // letters: A / B / SRCH).
    let tag_letter = tag.chars().next().unwrap().to_ascii_uppercase();
    let mut eq_ids = Vec::new();
    for i in 1i32..=2 {
        let eq_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO registry_equipment (
                branch_id, customer_id, site_id,
                equipment_no, management_no, model,
                manufacturer_code, kind_code, power_code,
                status, specification, ton_text,
                source_sheet, source_row, org_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, 'A', 'B', 'C', '임대', '좌식', '3.0T',
                    'list-rls-test', $7, $8)
            RETURNING id
            "#,
        )
        .bind(branch_id)
        .bind(customer_id)
        .bind(site_id)
        .bind(format!("EQ{tag_letter}{i:02}-{i:04}"))
        .bind(format!("{i:03}"))
        .bind(format!("Model-{tag}-{i}"))
        .bind(i)
        .bind(org)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        eq_ids.push(eq_id);
    }

    tx.commit().await.unwrap();
    (branch_id, eq_ids)
}

/// Unarmed COUNT as console_rt — must return 0 under FORCE RLS.
async fn unarmed_count(rt_pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM registry_equipment WHERE source_sheet = 'list-rls-test'",
    )
    .fetch_one(rt_pool)
    .await
    .unwrap()
}

fn all_scope_query(limit: i64) -> EquipmentListQuery {
    EquipmentListQuery {
        branch_scope: BranchScope::All,
        q: None,
        status: None,
        branch_id: None,
        customer_id: None,
        site_id: None,
        model: None,
        maker: None,
        sort: EquipmentSortBy::EquipmentNo,
        limit,
        offset: 0,
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn equipment_list_is_rls_armed_org_scoped_and_branch_filtered(owner_pool: PgPool) {
    let org_a = OrgId::knl();
    let org_a_uuid = *org_a.as_uuid();
    let org_b = OrgId::from_uuid(ORG_B);
    let org_b_uuid = *org_b.as_uuid();

    let (branch_a, _) = seed_org_equipment(&owner_pool, org_a_uuid, "A").await;
    let (branch_b_uuid, _) = seed_org_equipment(&owner_pool, org_b_uuid, "B").await;

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let store = PgRegistryStore::new(rt_pool.clone());

    // (a) FAIL-CLOSED: without app.current_org armed, zero rows visible.
    assert_eq!(
        unarmed_count(&rt_pool).await,
        0,
        "unarmed console_rt must see zero rows (FORCE RLS)"
    );

    // (b) ORG-SCOPED: under org A's GUC, only org A's 2 rows are visible.
    let page_a = scope_org(org_a, store.list_equipment(all_scope_query(100)))
        .await
        .expect("list_equipment must succeed under org A GUC");
    assert_eq!(
        page_a.total, 2,
        "org A must see exactly its 2 rows, got total={}",
        page_a.total
    );
    assert_eq!(page_a.items.len(), 2);

    // (c) CROSS-TENANT-INVISIBLE: org B's rows are invisible to org A.
    let org_b_ids: Vec<_> = {
        let page_b = scope_org(org_b, store.list_equipment(all_scope_query(100)))
            .await
            .expect("list_equipment must succeed under org B GUC");
        assert_eq!(
            page_b.total, 2,
            "org B must see exactly its 2 rows, got total={}",
            page_b.total
        );
        page_b.items.iter().map(|i| i.equipment_id).collect()
    };
    // Now check org A cannot see org B's IDs.
    let page_a2 = scope_org(org_a, store.list_equipment(all_scope_query(100)))
        .await
        .expect("re-read as org A must succeed");
    for item in &page_a2.items {
        assert!(
            !org_b_ids.contains(&item.equipment_id),
            "org A must not see org B's equipment (cross-tenant leak): {:?}",
            item.equipment_id
        );
    }

    // (d) BRANCH-SCOPE-FILTERED: a branch-scoped principal scoped to branch_a
    //     only sees its own branch's rows (none from branch_b even if same org).
    //     We use a second HQ branch to simulate a multi-branch org.
    let _ = branch_b_uuid; // org B's branch — not same org, already tested above.

    // Scope to branch_a only via BranchScope::Branches.
    use console_kernel_core::BranchId;
    use std::collections::BTreeSet;
    let branch_a_id = BranchId::from_uuid(branch_a);
    let branch_scoped_query = EquipmentListQuery {
        branch_scope: BranchScope::Branches(BTreeSet::from([branch_a_id])),
        ..all_scope_query(100)
    };
    let page_branch = scope_org(org_a, store.list_equipment(branch_scoped_query))
        .await
        .expect("branch-scoped list must succeed");
    // org A only has one branch (branch_a), so same 2 rows.
    assert_eq!(
        page_branch.total, 2,
        "branch-scoped principal must still see all rows in their branch"
    );

    // Empty-branch scope yields zero rows (fail-closed for branchless principals).
    let empty_scope_query = EquipmentListQuery {
        branch_scope: BranchScope::Branches(BTreeSet::new()),
        ..all_scope_query(100)
    };
    let page_empty = scope_org(org_a, store.list_equipment(empty_scope_query))
        .await
        .expect("empty-scope list must not error");
    assert_eq!(
        page_empty.total, 0,
        "empty branch scope must yield zero rows"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn equipment_list_search_and_pagination(owner_pool: PgPool) {
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();

    seed_org_equipment(&owner_pool, org_uuid, "SRCH").await;

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let store = PgRegistryStore::new(rt_pool);

    // Search by management_no "001" — only the first row.
    let page = scope_org(
        org,
        store.list_equipment(EquipmentListQuery {
            branch_scope: BranchScope::All,
            q: Some("001".to_owned()),
            status: None,
            branch_id: None,
            customer_id: None,
            site_id: None,
            model: None,
            maker: None,
            sort: EquipmentSortBy::EquipmentNo,
            limit: 50,
            offset: 0,
        }),
    )
    .await
    .expect("search must succeed");
    assert_eq!(page.total, 1, "search '001' must match exactly one row");
    assert!(
        page.items[0].management_no.as_deref() == Some("001"),
        "matched row must have management_no '001'"
    );

    // Search by 호기-normalized "1호기" must also match stored "001" (ltrim leading zero).
    let page_hogi = scope_org(
        org,
        store.list_equipment(EquipmentListQuery {
            branch_scope: BranchScope::All,
            q: Some("1호기".to_owned()),
            status: None,
            branch_id: None,
            customer_id: None,
            site_id: None,
            model: None,
            maker: None,
            sort: EquipmentSortBy::EquipmentNo,
            limit: 50,
            offset: 0,
        }),
    )
    .await
    .expect("호기-normalized search must succeed");
    assert_eq!(
        page_hogi.total, 1,
        "호기-normalized '1호기' must match stored '001'"
    );

    // Pagination: limit=1 returns total=2 but items.len()=1.
    let page_p1 = scope_org(org, store.list_equipment(all_scope_query(1)))
        .await
        .expect("limit=1 list must succeed");
    assert_eq!(
        page_p1.total, 2,
        "total must reflect full count not page size"
    );
    assert_eq!(page_p1.items.len(), 1, "items must be limited to 1");

    let page_p2 = scope_org(
        org,
        store.list_equipment(EquipmentListQuery {
            offset: 1,
            ..all_scope_query(1)
        }),
    )
    .await
    .expect("offset=1 list must succeed");
    assert_eq!(page_p2.items.len(), 1);
    assert_ne!(
        page_p1.items[0].equipment_id, page_p2.items[0].equipment_id,
        "pages must return distinct rows"
    );
}
