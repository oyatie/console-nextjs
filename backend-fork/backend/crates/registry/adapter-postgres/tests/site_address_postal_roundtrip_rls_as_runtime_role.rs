#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS round-trip gate for a customer site's ADDRESS + POSTAL CODE
//! (issue #19.4 — the 고객 현장관리 form reported address/postal_code vanishing
//! after a save).
//!
//! Proves, as the genuine non-owner `console_rt` role (NOSUPERUSER, NOBYPASSRLS,
//! FORCE RLS) under org A's armed `app.current_org`, that address + postal_code:
//!   (a) PERSIST on create_site,
//!   (b) PERSIST on a subsequent update_site (PATCH) that changes them,
//!   (c) ROUND-TRIP on the by-location read (`equipment_by_location`) the
//!       SiteGeographyPanel seeds its form from.
//!
//! Why `console_rt`: the default `#[sqlx::test]` pool connects as a BYPASSRLS
//! superuser, which would green-light a read/write that is actually broken under
//! the production tenant policy. We SEED as the owner (raw inserts) and exercise
//! create/update/read as `console_rt`.

use console_kernel_core::{BranchId, BranchScope, CustomerId, OrgId, TraceContext, UserId};
use console_platform_request_context::scope_org;
use console_registry_adapter_postgres::PgRegistryStore;
use console_registry_application::{
    CreateSiteCommand, EquipmentByLocationQuery, UpdateSiteCommand, UpdateSiteFields,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

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
// create → update → by-location read: address, postal_code, and coordinates
// survive end-to-end, and coordinates can be cleared back to NULL.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn site_address_postal_code_and_coordinates_round_trip_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_uuid = *org_a.as_uuid();

    seed_org(&owner_pool, org_uuid, "A").await;
    let branch_id = seed_branch(&owner_pool, org_uuid).await;
    let admin = seed_user(&owner_pool, org_uuid, "ADMIN", branch_id).await;
    let customer = seed_customer(&owner_pool, org_uuid, branch_id, "한울로지스").await;

    let (group, cleared_group) = scope_org(org_a, async {
        let store = PgRegistryStore::new(rt_pool.clone());

        // (a) Create the site WITH an address + postal_code.
        let site = store
            .create_site(CreateSiteCommand {
                actor: admin,
                branch_scope: BranchScope::single(branch_id),
                customer_id: customer,
                name: "안산1공장".to_owned(),
                address: Some("경기도 안산시 단원구 1로 1".to_owned()),
                province: Some("경기도".to_owned()),
                city: Some("안산시".to_owned()),
                postal_code: Some("15433".to_owned()),
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
            .expect("create_site must succeed as console_rt under org-A's armed GUC");
        assert_eq!(
            site.address.as_deref(),
            Some("경기도 안산시 단원구 1로 1"),
            "create_site must return the supplied address"
        );
        assert_eq!(
            site.postal_code.as_deref(),
            Some("15433"),
            "create_site must return the supplied postal_code"
        );

        // (b) Update the site with a DIFFERENT address + postal_code + coordinate
        // pair (the operator re-saves the form). Fields are present as
        // `Some(Some(_))`, mirroring the SiteGeographyPanel PATCH body.
        store
            .update_site(UpdateSiteCommand {
                actor: admin,
                site_id: site.id,
                fields: UpdateSiteFields {
                    address: Some(Some("서울특별시 중구 세종대로 110".to_owned())),
                    postal_code: Some(Some("04524".to_owned())),
                    latitude: Some(Some(37.5665)),
                    longitude: Some(Some(126.9780)),
                    ..UpdateSiteFields::default()
                },
                branch_scope: BranchScope::All,
                trace: TraceContext::generate(),
                occurred_at: occurred_at(),
            })
            .await
            .expect("update_site must succeed as console_rt under org-A's armed GUC");

        // (c) Read the site back via the by-location aggregation the panel seeds
        // its form from, and confirm the UPDATED address + postal_code +
        // coordinates survive.
        let group = store
            .equipment_by_location(EquipmentByLocationQuery {
                branch_scope: BranchScope::All,
            })
            .await
            .expect("equipment_by_location must succeed as console_rt under org-A's armed GUC")
            .into_iter()
            .find(|g| g.site_id == site.id)
            .expect("the created site must be returned by the by-location read");

        // (d) Clear the coordinate pair with explicit null/null semantics while
        // leaving address/postal_code untouched.
        store
            .update_site(UpdateSiteCommand {
                actor: admin,
                site_id: site.id,
                fields: UpdateSiteFields {
                    latitude: Some(None),
                    longitude: Some(None),
                    ..UpdateSiteFields::default()
                },
                branch_scope: BranchScope::All,
                trace: TraceContext::generate(),
                occurred_at: occurred_at(),
            })
            .await
            .expect("coordinate clear must succeed as console_rt under org-A's armed GUC");

        let cleared_group = store
            .equipment_by_location(EquipmentByLocationQuery {
                branch_scope: BranchScope::All,
            })
            .await
            .expect("equipment_by_location must succeed after coordinate clear")
            .into_iter()
            .find(|g| g.site_id == site.id)
            .expect("the cleared site must still be returned by the by-location read");

        (group, cleared_group)
    })
    .await;

    assert_eq!(
        group.address.as_deref(),
        Some("서울특별시 중구 세종대로 110"),
        "the updated address must round-trip on the by-location read (issue #19.4)"
    );
    assert_eq!(
        group.postal_code.as_deref(),
        Some("04524"),
        "the updated postal_code must round-trip on the by-location read (issue #19.4)"
    );
    assert_eq!(
        group.latitude,
        Some(37.5665),
        "the updated latitude must round-trip on the by-location read"
    );
    assert_eq!(
        group.longitude,
        Some(126.9780),
        "the updated longitude must round-trip on the by-location read"
    );
    assert_eq!(
        cleared_group.latitude, None,
        "explicit null clears latitude"
    );
    assert_eq!(
        cleared_group.longitude, None,
        "explicit null clears longitude"
    );

    // And it is genuinely persisted in the row (owner pool sees all), not just
    // echoed by the read mapping.
    let (stored_address, stored_postal, stored_latitude, stored_longitude): (
        Option<String>,
        Option<String>,
        Option<f64>,
        Option<f64>,
    ) = sqlx::query_as(
        "SELECT address, postal_code, latitude, longitude FROM registry_sites WHERE id = $1",
    )
    .bind(*group.site_id.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        stored_address.as_deref(),
        Some("서울특별시 중구 세종대로 110")
    );
    assert_eq!(stored_postal.as_deref(), Some("04524"));
    assert_eq!(stored_latitude, None);
    assert_eq!(stored_longitude, None);
}
