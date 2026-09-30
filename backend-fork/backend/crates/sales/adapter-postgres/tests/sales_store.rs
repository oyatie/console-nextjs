#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use console_kernel_core::{CustomerInquiryId, OrgId, SalesListingId, TraceContext, UserId};
use console_sales_adapter_postgres::PgSalesStore;
use console_sales_application::{
    CatalogQuery, CreateListingCommand, DeleteListingCommand, InquiryInboxQuery, ListingInput,
    SubmitInquiryCommand, UpdateInquiryStatusCommand, UpdateListingCommand, UpdateListingFields,
};
use console_sales_domain::{
    InquiryStatus, InquiryTopic, ListingCondition, ListingKind, ListingStatus, ListingType,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use time::macros::datetime;
use tokio::sync::oneshot;

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn public_catalog_count_and_items_share_a_snapshot_during_withdrawal(pool: PgPool) {
    // SQLx migrations run as a superuser, without production's default grants.
    sqlx::query("GRANT SELECT ON sales_listings, sales_listing_media TO console_rt")
        .execute(&pool)
        .await
        .unwrap();
    let actor = seed_user(&pool).await;
    let listing_id = SalesListingId::new();
    let writer = PgSalesStore::new(pool.clone());
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        writer
            .create_listing(CreateListingCommand {
                actor,
                listing_id,
                input: ListingInput {
                    kind: ListingKind::Electric,
                    condition: ListingCondition::Used,
                    model_name: "Snapshot listing".into(),
                    capacity_milli: None,
                    model_year: None,
                    usage_hours: None,
                    price_won: None,
                    badge: None,
                    usage_label: None,
                    condition_label: None,
                    availability: None,
                    location: None,
                    description: None,
                    listing_type: ListingType::Sale,
                    status: ListingStatus::Published,
                    sort_weight: 0,
                    equipment_id: None,
                },
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-09-27 00:00:00 UTC),
            })
            .await
            .unwrap();

        // One reader connection makes SQLx's release hook a precise gate:
        // the current implementation releases it after COUNT, before rows.
        let (count_done_tx, count_done_rx) = oneshot::channel();
        let (withdraw_done_tx, withdraw_done_rx) = oneshot::channel();
        let gate = Arc::new(Mutex::new(Some((count_done_tx, withdraw_done_rx))));
        let reader_pool = PgPoolOptions::new()
            .max_connections(1)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET ROLE console_rt").execute(conn).await?;
                    Ok(())
                })
            })
            .after_release(move |_, _| {
                let gate = gate.lock().unwrap().take();
                Box::pin(async move {
                    if let Some((count_done, withdraw_done)) = gate {
                        count_done.send(()).map_err(|_| {
                            sqlx::Error::Protocol("catalog writer did not wait for count".into())
                        })?;
                        withdraw_done.await.map_err(|_| {
                            sqlx::Error::Protocol("catalog withdrawal did not finish".into())
                        })?;
                    }
                    Ok(true)
                })
            })
            .connect_lazy_with(pool.connect_options().as_ref().clone());
        let reader = PgSalesStore::new(reader_pool);
        let withdraw = tokio::spawn(async move {
            count_done_rx.await.unwrap();
            console_platform_request_context::scope_org(OrgId::knl(), async move {
                writer
                    .delete_listing(DeleteListingCommand {
                        actor,
                        listing_id,
                        trace: TraceContext::generate(),
                        occurred_at: datetime!(2026-09-27 00:01:00 UTC),
                    })
                    .await
                    .unwrap();
            })
            .await;
            withdraw_done_tx.send(()).unwrap();
        });

        let page = reader.list_listings(catalog(false)).await.unwrap();
        withdraw.await.unwrap();
        assert_eq!(page.total, 1, "count must describe the returned snapshot");
        assert_eq!(
            page.items.len(),
            1,
            "withdrawal cannot split count and rows"
        );
        assert_eq!(page.items[0].id, listing_id);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn listing_and_inquiry_lifecycle_is_tenant_scoped_and_audited(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let actor = seed_user(&pool).await;
        let store = PgSalesStore::new(pool.clone());

        // A DRAFT listing.
        let listing_id = SalesListingId::new();
        store
            .create_listing(CreateListingCommand {
                actor,
                listing_id,
                input: ListingInput {
                    kind: ListingKind::Electric,
                    condition: ListingCondition::Used,
                    model_name: "전동 지게차 2.5톤".into(),
                    capacity_milli: Some(2500),
                    model_year: Some(2021),
                    usage_hours: Some(1200),
                    price_won: Some(12_000_000),
                    badge: Some("실내 창고 추천".into()),
                    usage_label: Some("물류창고".into()),
                    condition_label: Some("검수 완료".into()),
                    availability: Some("렌탈·구매 가능".into()),
                    location: Some("창원".into()),
                    description: None,
                    listing_type: ListingType::Both,
                    status: ListingStatus::Draft,
                    sort_weight: 10,
                    equipment_id: None,
                },
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-06-21 09:00:00 UTC),
            })
            .await
            .unwrap();

        // Admin sees the draft; the public catalog does not.
        assert_eq!(store.list_listings(catalog(true)).await.unwrap().total, 1);
        assert_eq!(
            store.list_listings(catalog(false)).await.unwrap().total,
            0,
            "draft is hidden from the public catalog"
        );

        // Publish + reprice.
        let fields = UpdateListingFields {
            status: Some(ListingStatus::Published),
            price_won: Some(Some(11_500_000)),
            ..UpdateListingFields::default()
        };
        store
            .update_listing(UpdateListingCommand {
                actor,
                listing_id,
                fields,
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-06-21 10:00:00 UTC),
            })
            .await
            .unwrap();
        let media_id = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO sales_listing_media \
             (id, org_id, listing_id, s3_key, content_type, sort_order) \
             VALUES ($1, $2, $3, 'snapshot-test-media', 'image/png', 1)",
        )
        .bind(media_id)
        .bind(*OrgId::knl().as_uuid())
        .bind(*listing_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
        let public = store.list_listings(catalog(false)).await.unwrap();
        assert_eq!(public.total, 1, "published listing is public");
        assert_eq!(public.items[0].media[0].id, media_id.to_string());
        let detail = store.get_listing(listing_id, false).await.unwrap().unwrap();
        assert_eq!(detail.media[0].id, media_id.to_string());
        assert_eq!(public.items[0].price_won, Some(11_500_000));
        assert_eq!(public.items[0].kind, ListingKind::Electric);
        assert_eq!(public.items[0].condition, ListingCondition::Used);

        // A second, brand-new (신차) listing — published straight away so the
        // storefront's 중고/신차 sub-category filter is exercised end-to-end
        // through the RLS-armed store.
        let new_listing_id = SalesListingId::new();
        store
            .create_listing(CreateListingCommand {
                actor,
                listing_id: new_listing_id,
                input: ListingInput {
                    kind: ListingKind::Diesel,
                    condition: ListingCondition::New,
                    model_name: "신차 디젤 지게차 3.0톤".into(),
                    capacity_milli: Some(3000),
                    model_year: Some(2026),
                    usage_hours: Some(0),
                    price_won: Some(38_000_000),
                    badge: None,
                    usage_label: None,
                    condition_label: None,
                    availability: None,
                    location: None,
                    description: None,
                    listing_type: ListingType::Sale,
                    status: ListingStatus::Published,
                    sort_weight: 5,
                    equipment_id: None,
                },
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-06-21 10:30:00 UTC),
            })
            .await
            .unwrap();

        // The public catalog now holds one 중고 and one 신차 listing; each
        // condition filter returns exactly its own.
        assert_eq!(store.list_listings(catalog(false)).await.unwrap().total, 2);
        let used = store
            .list_listings(catalog_with_condition(ListingCondition::Used))
            .await
            .unwrap();
        assert_eq!(used.total, 1, "USED filter returns only the used listing");
        assert_eq!(used.items[0].condition, ListingCondition::Used);
        assert_eq!(used.items[0].id, listing_id);
        let new = store
            .list_listings(catalog_with_condition(ListingCondition::New))
            .await
            .unwrap();
        assert_eq!(new.total, 1, "NEW filter returns only the 신차 listing");
        assert_eq!(new.items[0].condition, ListingCondition::New);
        assert_eq!(new.items[0].id, new_listing_id);
        assert_eq!(new.items[0].usage_hours, Some(0));

        // A public inquiry against the listing.
        let inquiry_id = CustomerInquiryId::new();
        store
            .submit_inquiry(SubmitInquiryCommand {
                inquiry_id,
                name: "홍길동".into(),
                phone: "010-1234-5678".into(),
                topic: InquiryTopic::UsedSales,
                location: Some("창원".into()),
                message: Some("2.5톤 전동 재고 문의".into()),
                listing_id: Some(listing_id),
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-06-21 11:00:00 UTC),
            })
            .await
            .unwrap();
        let inbox = store
            .list_inquiries(InquiryInboxQuery {
                status: None,
                limit: 50,
                offset: 0,
            })
            .await
            .unwrap();
        assert_eq!(inbox.total, 1);
        assert_eq!(inbox.items[0].name, "홍길동");
        assert_eq!(inbox.items[0].status, InquiryStatus::New);

        // The inquiry audit snapshot is PII-LIGHT: no name/phone leaked into it.
        let pii_in_audit: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events \
             WHERE action = 'sales_inquiry.submit' \
               AND COALESCE(after_snap::text, '') ~ '(홍길동|010-1234)'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pii_in_audit, 0, "no PII in the inquiry audit snapshot");

        // Triage the inquiry.
        store
            .update_inquiry_status(UpdateInquiryStatusCommand {
                actor,
                inquiry_id,
                status: InquiryStatus::Contacted,
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-06-21 12:00:00 UTC),
            })
            .await
            .unwrap();
        let inbox = store
            .list_inquiries(InquiryInboxQuery {
                status: Some(InquiryStatus::Contacted),
                limit: 50,
                offset: 0,
            })
            .await
            .unwrap();
        assert_eq!(inbox.total, 1);
        assert_eq!(inbox.items[0].status, InquiryStatus::Contacted);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn public_listing_pages_use_id_to_break_equal_sort_ties(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let actor = seed_user(&pool).await;
        let store = PgSalesStore::new(pool.clone());
        let mut expected = Vec::new();

        for index in 1..=8 {
            let listing_id = SalesListingId::from_uuid(
                uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{index:012x}")).unwrap(),
            );
            store
                .create_listing(CreateListingCommand {
                    actor,
                    listing_id,
                    input: ListingInput {
                        kind: ListingKind::Electric,
                        condition: ListingCondition::Used,
                        model_name: format!("동순위 장비 {index}"),
                        capacity_milli: None,
                        model_year: None,
                        usage_hours: None,
                        price_won: None,
                        badge: None,
                        usage_label: None,
                        condition_label: None,
                        availability: None,
                        location: None,
                        description: None,
                        listing_type: ListingType::Sale,
                        status: ListingStatus::Published,
                        sort_weight: 10,
                        equipment_id: None,
                    },
                    trace: TraceContext::generate(),
                    occurred_at: datetime!(2026-09-27 00:00:00 UTC),
                })
                .await
                .unwrap();
            expected.push(listing_id);
        }

        // The public sort keys can tie. Equal timestamps are legal even for
        // independently created listings; no page boundary may depend on heap
        // or planner order when the user follows the catalog's offset links.
        sqlx::query("UPDATE sales_listings SET created_at = $1 WHERE org_id = $2")
            .bind(datetime!(2026-09-27 00:00:00 UTC))
            .bind(*OrgId::knl().as_uuid())
            .execute(&pool)
            .await
            .unwrap();
        expected.sort_by(|a, b| b.as_uuid().cmp(a.as_uuid()));

        let mut seen = Vec::new();
        for offset in [0, 3, 6] {
            let page = store
                .list_listings(CatalogQuery {
                    limit: 3,
                    offset,
                    ..catalog(false)
                })
                .await
                .unwrap();
            assert_eq!(page.total, 8);
            seen.extend(page.items.into_iter().map(|item| item.id));
        }
        assert_eq!(
            seen, expected,
            "equal-ranked listings need an ID tie-breaker across public pages"
        );
    })
    .await;
}

// BE-OBJ slice 2, item 3 / audit gap 23: "deleting" a listing must soft-
// archive it (status -> WITHDRAWN), never hard-DELETE — the row and its media
// survive so the object graph never dangles and history stays reconstructable.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn deleting_a_listing_archives_it_instead_of_hard_deleting(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let actor = seed_user(&pool).await;
        let store = PgSalesStore::new(pool.clone());

        let listing_id = SalesListingId::new();
        store
            .create_listing(CreateListingCommand {
                actor,
                listing_id,
                input: ListingInput {
                    kind: ListingKind::Electric,
                    condition: ListingCondition::Used,
                    model_name: "전동 지게차 1.5톤".into(),
                    capacity_milli: Some(1500),
                    model_year: Some(2020),
                    usage_hours: Some(800),
                    price_won: Some(9_000_000),
                    badge: None,
                    usage_label: None,
                    condition_label: None,
                    availability: None,
                    location: None,
                    description: None,
                    listing_type: ListingType::Sale,
                    status: ListingStatus::Published,
                    sort_weight: 1,
                    equipment_id: None,
                },
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-07-09 09:00:00 UTC),
            })
            .await
            .unwrap();
        assert_eq!(
            store.list_listings(catalog(false)).await.unwrap().total,
            1,
            "published listing starts out public"
        );

        store
            .delete_listing(DeleteListingCommand {
                actor,
                listing_id,
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-07-09 10:00:00 UTC),
            })
            .await
            .unwrap();

        // No longer in the public catalog...
        assert_eq!(
            store.list_listings(catalog(false)).await.unwrap().total,
            0,
            "archived listing leaves the public catalog"
        );
        // ...but the row itself survives (soft archive, not a hard delete): the
        // admin view (include_non_public) still finds it, status = WITHDRAWN.
        let archived = store.get_listing(listing_id, true).await.unwrap();
        assert!(
            archived.is_some(),
            "listing must survive delete_listing as a row, not vanish"
        );
        assert_eq!(archived.unwrap().status, ListingStatus::Withdrawn);

        // The audit event captures the archival, not a bare removal: before
        // was PUBLISHED, after is WITHDRAWN (not absent).
        let (before, after): (serde_json::Value, serde_json::Value) = sqlx::query_as(
            "SELECT before_snap, after_snap FROM audit_events \
             WHERE action = 'sales_listing.delete' AND target_id = $1",
        )
        .bind(listing_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(before["status"], "PUBLISHED");
        assert_eq!(after["status"], "WITHDRAWN");

        // Repeat delete on an already-WITHDRAWN listing is a no-op: no second
        // audit event (a retried/duplicate delete call must not spam audit).
        store
            .delete_listing(DeleteListingCommand {
                actor,
                listing_id,
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-07-09 11:00:00 UTC),
            })
            .await
            .unwrap();
        let delete_audit_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events \
             WHERE action = 'sales_listing.delete' AND target_id = $1",
        )
        .bind(listing_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            delete_audit_count, 1,
            "deleting an already-archived listing must not write a second audit event"
        );
    })
    .await;
}

fn catalog(include_non_public: bool) -> CatalogQuery {
    CatalogQuery {
        kind: None,
        condition: None,
        listing_type: None,
        include_non_public,
        limit: 50,
        offset: 0,
    }
}

fn catalog_with_condition(condition: ListingCondition) -> CatalogQuery {
    CatalogQuery {
        kind: None,
        condition: Some(condition),
        listing_type: None,
        include_non_public: false,
        limit: 50,
        offset: 0,
    }
}

async fn seed_user(pool: &PgPool) -> UserId {
    let id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind("Sales Admin")
    .bind(vec!["ADMIN".to_string()])
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    UserId::from_uuid(id)
}
