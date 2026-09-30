#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use console_kernel_core::{OrgId, SalesListingId, TraceContext, UserId};
use console_sales_adapter_postgres::PgSalesStore;
use console_sales_application::{CreateListingCommand, DeleteListingCommand, ListingInput};
use console_sales_domain::{ListingCondition, ListingKind, ListingStatus, ListingType};
use console_sales_rest::{SalesRestState, router};
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn withdrawal_and_inquiry_serialize_on_the_selected_listing(pool: PgPool) {
    let listing_id = SalesListingId::new();
    sqlx::query(
        "INSERT INTO sales_listings (id, org_id, kind, model_name, status) \
         VALUES ($1, $2, 'ELECTRIC', 'Concurrent listing', 'PUBLISHED')",
    )
    .bind(*listing_id.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let mut holder = pool.begin().await.unwrap();
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(holder.as_mut())
        .await
        .unwrap();
    // A status UPDATE takes the same row-lock strength as a real withdrawal.
    // FOR UPDATE here would also block a too-weak FOR KEY SHARE inquiry.
    let updated = sqlx::query("UPDATE sales_listings SET status = 'WITHDRAWN' WHERE id = $1")
        .bind(*listing_id.as_uuid())
        .execute(holder.as_mut())
        .await
        .unwrap();
    assert_eq!(updated.rows_affected(), 1);

    let service = router(SalesRestState::new(PgSalesStore::new(pool.clone()), None));
    let submitted = tokio::spawn(async move {
        service
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/storefront/inquiries")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "name": "Buyer", "phone": "010-1234-5678", "topic": "OTHER",
                            "listing_id": listing_id,
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    });

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity \
                 WHERE state = 'active' AND $1 = ANY(pg_blocking_pids(pid)))",
            )
            .bind(holder_pid)
            .fetch_one(&pool)
            .await
            .unwrap();
            if blocked {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("inquiry must wait on the listing row before the withdrawal commits");

    holder.commit().await.unwrap();
    let response = submitted.await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let inquiries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM customer_inquiries")
        .fetch_one(&pool)
        .await
        .unwrap();
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'sales_inquiry.submit'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((inquiries, audited), (0, 0));
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn listing_inquiry_rejects_unavailable_object_without_silent_relink(pool: PgPool) {
    let actor_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id) VALUES ('Sales Admin', ARRAY['ADMIN'], $1) RETURNING id",
    )
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let actor = UserId::from_uuid(actor_id);
    let listing_id = SalesListingId::new();
    let store = PgSalesStore::new(pool.clone());
    console_platform_request_context::scope_org(OrgId::knl(), async {
        store
            .create_listing(CreateListingCommand {
                actor,
                listing_id,
                input: ListingInput {
                    equipment_id: None,
                    kind: ListingKind::Electric,
                    condition: ListingCondition::Used,
                    model_name: "Inquiry target".into(),
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
                },
                trace: TraceContext::generate(),
                occurred_at: time::OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
    })
    .await;

    let service = router(SalesRestState::new(store.clone(), None));
    let detail = service
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/storefront/listings/{listing_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);

    let body = json!({
        "name": "Buyer", "phone": "010-1234-5678", "topic": "OTHER",
        "listing_id": listing_id,
    });
    let submit = |body: serde_json::Value| {
        let service = service.clone();
        async move {
            service
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/storefront/inquiries")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap()
        }
    };
    assert_eq!(submit(body.clone()).await.status(), StatusCode::ACCEPTED);

    console_platform_request_context::scope_org(OrgId::knl(), async {
        store
            .delete_listing(DeleteListingCommand {
                actor,
                listing_id,
                trace: TraceContext::generate(),
                occurred_at: time::OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
    })
    .await;

    let foreign_org = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, 'Foreign')")
        .bind(foreign_org)
        .bind("foreign-test")
        .execute(&pool)
        .await
        .unwrap();
    let draft_id = SalesListingId::new();
    let sold_id = SalesListingId::new();
    let foreign_id = SalesListingId::new();
    let reserved_id = SalesListingId::new();
    for (id, org, status) in [
        (draft_id, *OrgId::knl().as_uuid(), "DRAFT"),
        (sold_id, *OrgId::knl().as_uuid(), "SOLD"),
        (foreign_id, foreign_org, "PUBLISHED"),
        (reserved_id, *OrgId::knl().as_uuid(), "RESERVED"),
    ] {
        sqlx::query(
            "INSERT INTO sales_listings (id, org_id, kind, model_name, status) \
             VALUES ($1, $2, 'ELECTRIC', 'Inquiry fixture', $3)",
        )
        .bind(*id.as_uuid())
        .bind(org)
        .bind(status)
        .execute(&pool)
        .await
        .unwrap();
    }

    for unavailable_id in [
        listing_id,
        SalesListingId::new(),
        draft_id,
        sold_id,
        foreign_id,
    ] {
        let mut unavailable = body.clone();
        unavailable["listing_id"] = json!(unavailable_id);
        let response = submit(unavailable).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            json!({"error": {"code": "conflict", "message": "listing is no longer available for this inquiry"}})
        );
    }

    let mut general = body;
    general["listing_id"] = json!(reserved_id);
    assert_eq!(submit(general.clone()).await.status(), StatusCode::ACCEPTED);
    general.as_object_mut().unwrap().remove("listing_id");
    assert_eq!(submit(general).await.status(), StatusCode::ACCEPTED);

    let inquiries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM customer_inquiries")
        .fetch_one(&pool)
        .await
        .unwrap();
    let linked: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM customer_inquiries WHERE listing_id = $1")
            .bind(*listing_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'sales_inquiry.submit'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((inquiries, linked, audited), (3, 1, 3));
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn public_inquiry_raw_text_bounds_are_validation_not_500(pool: PgPool) {
    let service = router(SalesRestState::new(PgSalesStore::new(pool.clone()), None));
    let valid_body = json!({
        "name": format!("{} ", "A".repeat(99)),
        "phone": "1".repeat(40),
        "topic": "OTHER",
        "location": "L".repeat(120),
        "message": "M".repeat(2000),
    });
    let valid = service
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/storefront/inquiries")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(valid_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(valid.status(), StatusCode::ACCEPTED, "exact bound is valid");

    for (field, over_bound) in [
        ("name", format!("{} ", "A".repeat(100))),
        ("phone", format!("{} ", "1".repeat(40))),
        ("location", format!("{} ", "L".repeat(120))),
        ("message", format!("{} ", "M".repeat(2000))),
    ] {
        let mut body = valid_body.clone();
        body[field] = json!(over_bound);
        let response = service
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/storefront/inquiries")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{field}");
    }

    let inquiries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM customer_inquiries")
        .fetch_one(&pool)
        .await
        .unwrap();
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'sales_inquiry.submit'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(inquiries, 1, "invalid input must not add an inquiry");
    assert_eq!(
        audited, 1,
        "invalid input must not claim a submitted inquiry"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn public_inquiry_rejects_nul_before_postgres_write(pool: PgPool) {
    let service = router(SalesRestState::new(PgSalesStore::new(pool.clone()), None));
    let valid_body = json!({
        "name": "A", "phone": "1", "topic": "OTHER", "location": "L", "message": "M"
    });
    let valid = service
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/storefront/inquiries")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(valid_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(valid.status(), StatusCode::ACCEPTED);

    for field in ["name", "phone", "location", "message"] {
        let mut body = valid_body.clone();
        body[field] = json!("A\0B");
        let response = service
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/storefront/inquiries")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{field}");
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            json!({"error": {"code": "bad_request", "message": "request failed validation"}}),
            "{field} must not leak inquiry text or field details"
        );
    }

    let inquiries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM customer_inquiries")
        .fetch_one(&pool)
        .await
        .unwrap();
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'sales_inquiry.submit'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(inquiries, 1, "invalid input must not add an inquiry");
    assert_eq!(audited, 1, "invalid input must not claim an inquiry");
}
