#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the public sales/rental inquiry round-trip (#19.21).
//!
//! The public storefront submit and the staff inquiry inbox both run armed, but
//! the storefront pinned its org to a hardcoded `OrgId::knl()` literal while
//! staff read under their JWT `current_org`. For any tenant whose org is NOT the
//! `0x…a1` sentinel, the submit landed in a DIFFERENT org and FORCE RLS hid the
//! lead from staff — a customer-facing revenue defect. The fix resolves the
//! storefront tenant from configuration (the same org staff read under).
//!
//! This test proves, as the genuine non-owner runtime role `console_rt` (NOSUPERUSER,
//! NOBYPASSRLS, FORCE RLS) — NOT the default `#[sqlx::test]` BYPASSRLS superuser
//! pool, which would see every row and green-light a cross-org submit:
//!   (a) a public inquiry submitted under the RESOLVED storefront org is visible
//!       to a staff inbox read in that SAME org (round-trip), for BOTH a
//!       UsedSales and a RENTAL topic (rental leads must appear in 판매문의 관리);
//!   (b) cross-tenant: under a DIFFERENT org's GUC the lead is INVISIBLE.
//!
//! Crucially the storefront org here is a NON-`knl()` tenant, so the test would
//! reproduce the original bug if the submit still pinned `OrgId::knl()`.

use console_kernel_core::{CustomerInquiryId, ErrorKind, OrgId, SalesListingId, TraceContext};
use console_sales_adapter_postgres::{PgSalesError, PgSalesStore};
use console_sales_application::{InquiryInboxQuery, SubmitInquiryCommand};
use console_sales_domain::InquiryTopic;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

/// The RESOLVED storefront tenant — a console-minted, non-`knl()` org id. Using
/// a random-looking id (not `0x…a1`) is the whole point: it reproduces #19.21 if
/// the public submit were still hardcoded to `OrgId::knl()`.
const STOREFRONT_ORG: Uuid = Uuid::from_u128(0x5101_5101_5101_5101_5101_5101_5101_5101);
/// A second, different tenant to prove cross-tenant invisibility under `console_rt`.
const OTHER_ORG: Uuid = Uuid::from_u128(0x9999_9999_9999_9999_9999_9999_9999_9999);

/// A pool whose every connection runs `SET ROLE console_rt`, so statements execute as
/// the production runtime role under FORCE RLS (BYPASSRLS does not apply).
async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    // Static GRANT literals (no interpolation) to satisfy the dynamic-SQL audit
    // lint; the production default-privilege auto-grant gives these to console_rt,
    // but the #[sqlx::test] harness migrates as a different superuser.
    for grant in [
        "GRANT SELECT, INSERT ON customer_inquiries TO console_rt",
        "GRANT SELECT, INSERT, UPDATE ON sales_listings TO console_rt",
        "GRANT SELECT, INSERT ON audit_events TO console_rt",
    ] {
        sqlx::query(grant).execute(owner_pool).await.unwrap();
    }
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

fn inbox() -> InquiryInboxQuery {
    InquiryInboxQuery {
        status: None,
        limit: 50,
        offset: 0,
    }
}

/// Submit one inquiry of `topic` under `org`'s armed GUC as `console_rt`.
async fn submit(store: &PgSalesStore, org: OrgId, topic: InquiryTopic, name: &str) {
    console_platform_request_context::scope_org(org, async {
        store
            .submit_inquiry(SubmitInquiryCommand {
                inquiry_id: CustomerInquiryId::new(),
                name: name.to_owned(),
                phone: "010-1234-5678".to_owned(),
                topic,
                location: Some("창원".to_owned()),
                message: Some("재고 문의".to_owned()),
                listing_id: None,
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-06-21 11:00:00 UTC),
            })
            .await
    })
    .await
    .expect("public inquiry submit must succeed under the resolved storefront org as console_rt");
}

async fn submit_for_listing(
    store: &PgSalesStore,
    org: OrgId,
    listing_id: SalesListingId,
) -> Result<(), PgSalesError> {
    console_platform_request_context::scope_org(org, async {
        store
            .submit_inquiry(SubmitInquiryCommand {
                inquiry_id: CustomerInquiryId::new(),
                name: "Buyer".into(),
                phone: "010-1234-5678".into(),
                topic: InquiryTopic::UsedSales,
                location: None,
                message: None,
                listing_id: Some(listing_id),
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-06-21 11:00:00 UTC),
            })
            .await
    })
    .await
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn listing_inquiry_checks_public_status_and_tenant_as_runtime_role(owner_pool: PgPool) {
    seed_org(&owner_pool, STOREFRONT_ORG, "Storefront").await;
    seed_org(&owner_pool, OTHER_ORG, "Other").await;
    let listing_id = SalesListingId::new();
    let foreign_id = SalesListingId::new();
    for (id, org) in [(listing_id, STOREFRONT_ORG), (foreign_id, OTHER_ORG)] {
        sqlx::query(
            "INSERT INTO sales_listings (id, org_id, kind, model_name, status) \
             VALUES ($1, $2, 'ELECTRIC', 'Runtime listing', 'PUBLISHED')",
        )
        .bind(*id.as_uuid())
        .bind(org)
        .execute(&owner_pool)
        .await
        .unwrap();
    }
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let store = PgSalesStore::new(rt_pool);
    let storefront = OrgId::from_uuid(STOREFRONT_ORG);
    submit_for_listing(&store, storefront, listing_id)
        .await
        .expect("published same-tenant listing must be accepted as console_rt");
    let foreign = submit_for_listing(&store, storefront, foreign_id).await;
    assert!(
        matches!(foreign, Err(PgSalesError::Domain(ref err)) if err.kind == ErrorKind::Conflict)
    );
    let counted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM customer_inquiries")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert_eq!(counted, 1, "foreign listing must not create an inquiry");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn public_inquiry_is_visible_to_staff_in_same_org_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let storefront = OrgId::from_uuid(STOREFRONT_ORG);
    let other = OrgId::from_uuid(OTHER_ORG);
    seed_org(&owner_pool, STOREFRONT_ORG, "Storefront").await;
    seed_org(&owner_pool, OTHER_ORG, "Other").await;

    let store = PgSalesStore::new(rt_pool.clone());

    // (a) A 판매(중고) lead AND a 렌탈 lead, both submitted under the resolved
    //     storefront org — exactly as the public router does after the fix.
    submit(&store, storefront, InquiryTopic::UsedSales, "홍길동").await;
    submit(&store, storefront, InquiryTopic::Rental, "임꺽정").await;

    // Staff read under the SAME org see BOTH leads (round-trip), including the
    // rental one — rental inquiries land in the same customer_inquiries surface
    // the 판매문의 관리 inbox reads.
    let staff_view = console_platform_request_context::scope_org(storefront, async {
        store.list_inquiries(inbox()).await
    })
    .await
    .expect("staff inbox read must surface the storefront-org leads as console_rt");
    assert_eq!(
        staff_view.total, 2,
        "both the sale and rental leads are visible to staff in the storefront org"
    );
    let topics: Vec<InquiryTopic> = staff_view.items.iter().map(|i| i.topic).collect();
    assert!(
        topics.contains(&InquiryTopic::Rental),
        "the RENTAL inquiry is visible in the inquiry-management surface"
    );
    assert!(
        topics.contains(&InquiryTopic::UsedSales),
        "the 판매(중고) inquiry is visible in the inquiry-management surface"
    );

    // (b) Cross-tenant: under a DIFFERENT org's GUC the leads are INVISIBLE.
    let cross = console_platform_request_context::scope_org(other, async {
        store.list_inquiries(inbox()).await
    })
    .await
    .expect("the cross-org inbox read itself succeeds (just returns nothing)");
    assert_eq!(
        cross.total, 0,
        "storefront-org leads must be INVISIBLE under another org's GUC (RLS isolates tenants)"
    );
}
