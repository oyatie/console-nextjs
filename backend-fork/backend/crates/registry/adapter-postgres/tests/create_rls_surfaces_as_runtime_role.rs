#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the direct customer/site CREATE path.
//!
//! `PgRegistryStore::create_customer` / `create_site` run their INSERTs (and the
//! in-transaction ownership/duplicate checks) inside a `with_audits(org, ..)`
//! closure that arms `app.current_org`. A *static* gate proves the wrapping is in
//! source; this test proves it WORKS AT RUNTIME when the write executes as the
//! genuine non-owner runtime role `console_rt` (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) —
//! the only faithful exercise of the tenant policy.
//!
//! Why `console_rt` and not the default `#[sqlx::test]` pool: that pool connects as a
//! BYPASSRLS superuser, which can INSERT any org_id regardless of
//! `app.current_org` and would green-light a totally broken (or cross-tenant
//! leaking) write. We SEED as the owner (raw inserts) and WRITE as `console_rt`.
//!
//! Asserts, with two tenants A (KNL) and B:
//!   (a) under org-A's armed GUC, create_customer + create_site succeed as
//!       `console_rt` and land under org A (FORCE RLS WITH CHECK passes);
//!   (b) CROSS-ORG ISOLATION: under org-A's armed GUC, creating a site under org
//!       B's customer is NOT FOUND — the customer is invisible to org A, so the
//!       site is never created in (or attached across) another tenant;
//!   (c) FAIL-CLOSED: with NO GUC armed the create fails closed (MissingOrg),
//!       never an unscoped write.

use console_kernel_core::{
    BranchId, BranchScope, CustomerId, ErrorKind, OrgId, TraceContext, UserId,
};
use console_registry_adapter_postgres::{PgRegistryError, PgRegistryStore};
use console_registry_application::{CreateCustomerCommand, CreateSiteCommand};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

/// A second, non-KNL tenant id, to prove cross-tenant isolation under `console_rt`.
const ORG_B: Uuid = Uuid::from_u128(0x3333_3333_3333_3333_3333_3333_3333_3333);

// ===========================================================================
// Runtime-role pool: every connection becomes the genuine non-owner `console_rt`.
// ===========================================================================
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

// ===========================================================================
// Seeding (OWNER pool). Raw inserts bypass RLS as superuser; org_id columns are
// set explicitly so each row lands in the intended tenant.
// ===========================================================================

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
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

async fn seed_user(owner_pool: &PgPool, org: Uuid, role: &str, branch_id: BranchId) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("User {}", Uuid::new_v4()))
        .bind(Vec::from([role]))
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

/// Seed a customer directly (owner pool, raw insert) in `org`/`branch_id`.
async fn seed_customer(
    owner_pool: &PgPool,
    org: Uuid,
    branch_id: BranchId,
    name: &str,
) -> CustomerId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(name)
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    CustomerId::from_uuid(id)
}

fn occurred_at() -> time::OffsetDateTime {
    datetime!(2026-06-23 09:00 UTC)
}

// ===========================================================================
// (a) Under org-A's armed GUC, create_customer + create_site succeed as console_rt.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_customer_and_site_succeed_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_uuid = *org_a.as_uuid();

    seed_org(&owner_pool, org_uuid, "A").await;
    // The admin is seeded on an arbitrary branch; create_customer lands the new
    // customer on the org's default HQ branch (resolved inside the create), so we
    // assert against HQ below, not this seeded branch.
    let seeded_branch = seed_branch(&owner_pool, org_uuid).await;
    let admin = seed_user(&owner_pool, org_uuid, "ADMIN", seeded_branch).await;

    let (customer, site) = console_platform_request_context::scope_org(org_a, async {
        let store = PgRegistryStore::new(rt_pool.clone());
        let customer = store
            .create_customer(CreateCustomerCommand {
                actor: admin,
                branch_id: None,
                name: "한울로지스".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: occurred_at(),
            })
            .await
            .expect("create_customer must succeed as console_rt under org-A's armed GUC");
        let site = store
            .create_site(CreateSiteCommand {
                actor: admin,
                branch_scope: BranchScope::single(customer.branch_id),
                customer_id: customer.id,
                name: "안산1공장".to_owned(),
                address: Some("경기도 안산시 1로 1".to_owned()),
                province: Some("경기도".to_owned()),
                city: Some("안산시".to_owned()),
                postal_code: Some("15433".to_owned()),
                latitude: Some(37.3219),
                longitude: Some(126.8309),
                geofence_radius_m: Some(200.0),
                contact_name: Some("김현장".to_owned()),
                contact_phone: Some("010-2625-0987".to_owned()),
                contact_email: Some("site@example.com".to_owned()),
                trace: TraceContext::generate(),
                occurred_at: occurred_at(),
            })
            .await
            .expect("create_site must succeed as console_rt under org-A's armed GUC");
        (customer, site)
    })
    .await;

    assert_eq!(customer.name, "한울로지스");
    // The customer lands on the org's default HQ branch (named 'HQ'), not the
    // arbitrary seeded branch; the site inherits the customer's branch.
    let hq_branch: Uuid =
        sqlx::query_scalar("SELECT id FROM branches WHERE org_id = $1 AND name = 'HQ'")
            .bind(org_uuid)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(
        customer.branch_id,
        BranchId::from_uuid(hq_branch),
        "customer must be created on the default HQ branch"
    );
    assert_eq!(
        site.branch_id, customer.branch_id,
        "site inherits the customer's branch"
    );
    assert_eq!(site.customer_id, customer.id);
    assert_eq!(site.name, "안산1공장");
    assert_eq!(site.latitude, Some(37.3219));
    assert_eq!(site.contact_name.as_deref(), Some("김현장"));

    // The rows landed under org A (verified with the owner pool, which sees all).
    let cust_org: Uuid = sqlx::query_scalar("SELECT org_id FROM registry_customers WHERE id = $1")
        .bind(*customer.id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert_eq!(cust_org, org_uuid, "customer must be created in org A");
    let site_org: Uuid = sqlx::query_scalar("SELECT org_id FROM registry_sites WHERE id = $1")
        .bind(*site.id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert_eq!(site_org, org_uuid, "site must be created in org A");

    // Each create emitted exactly one audit row with the expected action.
    let customer_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'customer.create' AND target_id = $1",
    )
    .bind(customer.id.to_string())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(customer_audits, 1, "create_customer must be audited once");
    let site_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'site.create' AND target_id = $1",
    )
    .bind(site.id.to_string())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(site_audits, 1, "create_site must be audited once");
}

// ===========================================================================
// (a2) A duplicate customer name on the same branch is a conflict (not a merge).
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn duplicate_customer_name_conflicts_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_uuid = *org_a.as_uuid();

    seed_org(&owner_pool, org_uuid, "A").await;
    let branch_id = seed_branch(&owner_pool, org_uuid).await;
    let admin = seed_user(&owner_pool, org_uuid, "ADMIN", branch_id).await;

    let err = console_platform_request_context::scope_org(org_a, async {
        let store = PgRegistryStore::new(rt_pool.clone());
        store
            .create_customer(CreateCustomerCommand {
                actor: admin,
                branch_id: None,
                name: "중복고객".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: occurred_at(),
            })
            .await
            .expect("first create_customer must succeed");
        // Second create with the same name (HQ branch) must conflict.
        store
            .create_customer(CreateCustomerCommand {
                actor: admin,
                branch_id: None,
                name: "중복고객".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: occurred_at(),
            })
            .await
    })
    .await;

    assert!(
        err.is_err(),
        "a same-name customer on the HQ branch must conflict, never silently merge"
    );
}

// ===========================================================================
// (b) CROSS-ORG ISOLATION: under org-A's GUC, a site under org-B's customer is
//     NOT FOUND — RLS hides the foreign customer, so the write never crosses.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn cannot_create_site_under_another_orgs_customer(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);

    seed_org(&owner_pool, *org_a.as_uuid(), "A").await;
    let _branch_a = seed_branch(&owner_pool, *org_a.as_uuid()).await;

    seed_org(&owner_pool, *org_b.as_uuid(), "B").await;
    let branch_b = seed_branch(&owner_pool, *org_b.as_uuid()).await;
    let admin_a = seed_user(&owner_pool, *org_a.as_uuid(), "ADMIN", _branch_a).await;
    // Org B owns a customer; org A must not be able to attach a site to it.
    let foreign_customer = seed_customer(&owner_pool, *org_b.as_uuid(), branch_b, "B고객").await;

    let result = console_platform_request_context::scope_org(org_a, async {
        let store = PgRegistryStore::new(rt_pool.clone());
        store
            .create_site(CreateSiteCommand {
                actor: admin_a,
                branch_scope: BranchScope::All,
                customer_id: foreign_customer,
                name: "탈취시도현장".to_owned(),
                address: None,
                province: None,
                city: None,
                postal_code: None,
                latitude: None,
                longitude: None,
                geofence_radius_m: None,
                contact_name: None,
                contact_phone: None,
                contact_email: None,
                trace: TraceContext::generate(),
                occurred_at: occurred_at(),
            })
            .await
    })
    .await;

    assert!(
        result.is_err(),
        "org A must NOT create a site under org B's customer (RLS isolates tenants)"
    );

    // And no site row leaked into either tenant for that name.
    let leaked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM registry_sites WHERE name = $1")
        .bind("탈취시도현장")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert_eq!(leaked, 0, "no cross-tenant site row may be created");
}

// ===========================================================================
// (b2) SAME-ORG BRANCH ISOLATION: a branch-scoped actor cannot attach a site to
//      another branch's customer, and the 404-shaped error reveals no row.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn cannot_create_site_under_another_branchs_customer(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();

    seed_org(&owner_pool, org_uuid, "A").await;
    let branch_a = seed_branch(&owner_pool, org_uuid).await;
    let branch_b = seed_branch(&owner_pool, org_uuid).await;
    let admin_a = seed_user(&owner_pool, org_uuid, "ADMIN", branch_a).await;
    let customer_b = seed_customer(&owner_pool, org_uuid, branch_b, "B지점고객").await;

    let result = console_platform_request_context::scope_org(org, async {
        PgRegistryStore::new(rt_pool.clone())
            .create_site(CreateSiteCommand {
                actor: admin_a,
                branch_scope: BranchScope::single(branch_a),
                customer_id: customer_b,
                name: "타지점침범현장".to_owned(),
                address: None,
                province: None,
                city: None,
                postal_code: None,
                latitude: None,
                longitude: None,
                geofence_radius_m: None,
                contact_name: None,
                contact_phone: None,
                contact_email: None,
                trace: TraceContext::generate(),
                occurred_at: occurred_at(),
            })
            .await
    })
    .await;

    assert!(
        matches!(result, Err(PgRegistryError::Domain(ref error)) if error.kind == ErrorKind::NotFound),
        "a foreign-branch customer must be indistinguishable from a missing one: {result:?}"
    );
    let sites: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM registry_sites WHERE name = $1")
        .bind("타지점침범현장")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'site.create' AND after_snap->>'name' = $1",
    )
    .bind("타지점침범현장")
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(sites, 0);
    assert_eq!(audits, 0);
}

// ===========================================================================
// (c) FAIL-CLOSED: with NO GUC armed the create fails closed, never writes.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_customer_fails_closed_without_org_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    seed_org(&owner_pool, *org_a.as_uuid(), "A").await;
    let branch_id = seed_branch(&owner_pool, *org_a.as_uuid()).await;
    let admin = seed_user(&owner_pool, *org_a.as_uuid(), "ADMIN", branch_id).await;

    // No scope_org wrapper: current_org() is unset, so the create must fail closed
    // (MissingOrg), never opening an unscoped write.
    let store = PgRegistryStore::new(rt_pool.clone());
    let result = store
        .create_customer(CreateCustomerCommand {
            actor: admin,
            branch_id: None,
            name: "무org고객".to_owned(),
            trace: TraceContext::generate(),
            occurred_at: occurred_at(),
        })
        .await;
    assert!(
        result.is_err(),
        "with no org armed the create must fail closed, never write an unscoped row"
    );
}
