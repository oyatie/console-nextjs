#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, UserId, WorkOrderId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

#[tokio::test]
async fn openapi_yaml_is_served() -> Result<(), Box<dyn std::error::Error>> {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
    ])?;
    let state = AppState::new(config, DatabaseDependency::NotConfigured)?;

    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri("/openapi/openapi.yaml")
                .body(Body::empty())?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await?;
    let text = String::from_utf8(body.to_vec())?;
    assert!(text.contains("/api/work-orders"));
    assert!(text.contains("bearerAuth"));
    Ok(())
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn workorder_create_is_jwt_authorized_and_branch_scoped(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let admin_id = UserId::new();
    let branch_id = seed_branch(&pool, "WO Region", "WO Branch").await;
    let other_branch_id = seed_branch(&pool, "Other WO Region", "Other WO Branch").await;
    seed_user_with_branch(&pool, admin_id, "ADMIN", branch_id).await;
    seed_equipment(&pool, branch_id, "290").await;
    seed_equipment(&pool, other_branch_id, "777").await;
    let token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        admin_id,
        vec!["ADMIN".to_owned()],
        vec![branch_id],
    )
    .await
    .unwrap();
    let service = build_router(
        app_state(
            console_platform_test_support::runtime_role_pool(&pool).await,
            public_key_pem,
        )
        .unwrap(),
    );

    let forbidden = service
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/work-orders")
                .method("POST")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "branch_id": other_branch_id,
                        "management_no": "777",
                        "symptom": "Cross branch create"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let response = service
        .oneshot(
            Request::builder()
                .uri("/api/work-orders")
                .method("POST")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "branch_id": branch_id,
                        "management_no": "#290",
                        "symptom": "Hydraulic oil leak"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["branch_id"], branch_id.to_string());
    assert_eq!(json["status"], "RECEIVED");
    assert!(json["request_no"].as_str().unwrap().ends_with("-001"));

    let create_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = 'work_order.create'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(create_count, 1);

    let messenger_thread_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM messenger_threads t
        JOIN messenger_thread_members tm ON tm.thread_id = t.id
        WHERE t.work_order_id = $1
          AND t.kind = 'work_order'
          AND tm.user_id = $2
        "#,
    )
    .bind(uuid::Uuid::parse_str(json["id"].as_str().unwrap()).unwrap())
    .bind(*admin_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(messenger_thread_count, 1);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn workorder_read_surface_is_branch_scoped_filterable_and_detailed(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Read Region", "Read Branch").await;
    let other_branch_id = seed_branch(&pool, "Read Other Region", "Read Other Branch").await;
    let second_visible_branch_id = seed_branch(
        &pool,
        "Read Second Visible Region",
        "Read Second Visible Branch",
    )
    .await;
    let mechanic = UserId::new();
    let receptionist = UserId::new();
    let admin = UserId::new();
    seed_user_with_branch(&pool, mechanic, "MECHANIC", branch_id).await;
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*mechanic.as_uuid())
        .bind(*second_visible_branch_id.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    seed_user_with_branch(&pool, receptionist, "RECEPTIONIST", branch_id).await;
    seed_user_with_branch(&pool, admin, "ADMIN", branch_id).await;
    let equipment_290 = seed_equipment_record(&pool, branch_id, "290", "GTS25DE").await;
    let equipment_291 = seed_equipment_record(&pool, branch_id, "291", "GTS30DE").await;
    let other_equipment = seed_equipment_record(&pool, other_branch_id, "777", "OTHER").await;
    let second_visible_equipment =
        seed_equipment_record(&pool, second_visible_branch_id, "292", "VISIBLE").await;
    let due_anchor = OffsetDateTime::now_utc();

    let p1 = seed_read_work_order(
        &pool,
        ReadWorkOrderFixture {
            branch_id,
            equipment: equipment_290,
            receptionist,
            mechanic,
            request_no: "20260612-901",
            priority: "P1",
            target_due_at: due_anchor + Duration::hours(6),
        },
    )
    .await;
    let p2 = seed_read_work_order(
        &pool,
        ReadWorkOrderFixture {
            branch_id,
            equipment: equipment_291,
            receptionist,
            mechanic,
            request_no: "20260612-902",
            priority: "P2",
            target_due_at: due_anchor + Duration::hours(2),
        },
    )
    .await;
    let hidden = seed_read_work_order(
        &pool,
        ReadWorkOrderFixture {
            branch_id: other_branch_id,
            equipment: other_equipment,
            receptionist,
            mechanic,
            request_no: "20260612-903",
            priority: "P1",
            target_due_at: due_anchor + Duration::hours(1),
        },
    )
    .await;
    let second_visible = seed_read_work_order(
        &pool,
        ReadWorkOrderFixture {
            branch_id: second_visible_branch_id,
            equipment: second_visible_equipment,
            receptionist,
            mechanic,
            request_no: "20260612-904",
            priority: "P1",
            target_due_at: due_anchor + Duration::hours(1),
        },
    )
    .await;
    assert_ne!(hidden, p1);

    let token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        mechanic,
        vec!["MECHANIC".to_owned()],
        vec![branch_id, second_visible_branch_id],
    )
    .await
    .unwrap();
    let service = build_router(
        app_state(
            console_platform_test_support::runtime_role_pool(&pool).await,
            public_key_pem,
        )
        .unwrap(),
    );

    let first_page = get_json(
        service.clone(),
        "/api/v1/work-orders?status=ASSIGNED&assigned_to=me&limit=1&offset=0",
        &token,
    )
    .await;
    assert_eq!(first_page.status, StatusCode::OK, "{:?}", first_page.json);
    assert_eq!(first_page.json["total"], 3);
    assert_eq!(first_page.json["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        first_page.json["items"][0]["id"],
        second_visible.to_string()
    );
    assert_eq!(first_page.json["items"][0]["priority"], "P1");
    assert_eq!(first_page.json["lens"]["object_type"], "work_order");
    assert_eq!(first_page.json["lens"]["aggregates"]["total_count"], 3);
    assert_eq!(first_page.json["lens"]["aggregates"]["p1_count"], 2);
    assert_eq!(first_page.json["lens"]["aggregates"]["unassigned_count"], 0);
    assert!(
        first_page.json["lens"]["facets"]["status"]
            .as_array()
            .unwrap()
            .iter()
            .any(|bucket| bucket["value"] == "ASSIGNED"
                && bucket["count"] == 3
                && bucket["filters"]["status"] == "ASSIGNED"),
        "status facet should describe the full branch-scoped object set: {:?}",
        first_page.json["lens"]
    );
    assert!(
        first_page.json["lens"]["facets"]["priority"]
            .as_array()
            .unwrap()
            .iter()
            .any(|bucket| bucket["value"] == "P1"
                && bucket["count"] == 2
                && bucket["filters"]["priority"] == "P1"),
        "priority facet should expose drill filters: {:?}",
        first_page.json["lens"]
    );
    let due_histogram_total: i64 = first_page.json["lens"]["histograms"]["target_due_date"]
        .as_array()
        .unwrap()
        .iter()
        .map(|bucket| bucket["count"].as_i64().unwrap())
        .sum();
    assert_eq!(due_histogram_total, 3);
    assert!(
        first_page.json["lens"]["listograms"]["customers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|bucket| bucket["name"].as_str().unwrap().starts_with("Customer 29")),
        "customer listogram must stay branch-scoped: {:?}",
        first_page.json["lens"]["listograms"]["customers"]
    );

    let around_page = get_json(
        service.clone(),
        &format!("/api/v1/work-orders?around_work_order_id={p1}&status=ASSIGNED"),
        &token,
    )
    .await;
    assert_eq!(around_page.status, StatusCode::OK, "{:?}", around_page.json);
    assert_eq!(around_page.json["total"], 1);
    assert_eq!(around_page.json["items"][0]["id"], p1.to_string());
    assert_eq!(around_page.json["lens"]["aggregates"]["total_count"], 1);

    let second_page = get_json(
        service.clone(),
        &format!(
            "/api/v1/work-orders?branch_id={branch_id}&status=ASSIGNED&assigned_to=me&limit=1&offset=1"
        ),
        &token,
    )
    .await;
    assert_eq!(second_page.status, StatusCode::OK, "{:?}", second_page.json);
    assert_eq!(second_page.json["items"][0]["id"], p2.to_string());

    let requested_branch = get_json(
        service.clone(),
        &format!(
            "/api/v1/work-orders?branch_id={branch_id}&status=ASSIGNED&assigned_to=me&limit=10&offset=0"
        ),
        &token,
    )
    .await;
    assert_eq!(
        requested_branch.status,
        StatusCode::OK,
        "{:?}",
        requested_branch.json
    );
    assert_eq!(requested_branch.json["total"], 2);
    assert_eq!(requested_branch.json["items"].as_array().unwrap().len(), 2);
    assert!(
        requested_branch.json["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["branch_id"] == branch_id.to_string())
    );
    assert!(
        requested_branch.json["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["id"] != second_visible.to_string())
    );

    let foreign_branch = get_json(
        service.clone(),
        &format!("/api/v1/work-orders?branch_id={other_branch_id}&limit=10&offset=0"),
        &token,
    )
    .await;
    assert_eq!(
        foreign_branch.status,
        StatusCode::OK,
        "{:?}",
        foreign_branch.json
    );
    assert_eq!(foreign_branch.json["total"], 0);

    let filtered = get_json(
        service.clone(),
        "/api/v1/work-orders?status=ASSIGNED&priority=P2&assigned_to=me&limit=10&offset=0",
        &token,
    )
    .await;
    assert_eq!(filtered.status, StatusCode::OK, "{:?}", filtered.json);
    assert_eq!(filtered.json["total"], 1);
    assert_eq!(filtered.json["items"][0]["id"], p2.to_string());

    let customer_site_filtered = get_json(
        service.clone(),
        &format!(
            "/api/v1/work-orders?customer_id={}&site_id={}&limit=10&offset=0",
            equipment_290.customer_id, equipment_290.site_id
        ),
        &token,
    )
    .await;
    assert_eq!(
        customer_site_filtered.status,
        StatusCode::OK,
        "{:?}",
        customer_site_filtered.json
    );
    assert_eq!(customer_site_filtered.json["total"], 1);
    assert_eq!(
        customer_site_filtered.json["items"][0]["id"],
        p1.to_string()
    );

    let target_due_from = (due_anchor + Duration::hours(5)).format(&Rfc3339).unwrap();
    let target_due_to = (due_anchor + Duration::hours(7)).format(&Rfc3339).unwrap();
    let target_due_filtered = get_json(
        service.clone(),
        &format!(
            "/api/v1/work-orders?target_due_from={target_due_from}&target_due_to={target_due_to}&limit=10&offset=0"
        ),
        &token,
    )
    .await;
    assert_eq!(
        target_due_filtered.status,
        StatusCode::OK,
        "{:?}",
        target_due_filtered.json
    );
    assert_eq!(target_due_filtered.json["total"], 1);
    assert_eq!(target_due_filtered.json["items"][0]["id"], p1.to_string());

    let detail = get_json(
        service.clone(),
        &format!("/api/v1/work-orders/{p1}"),
        &token,
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK, "{:?}", detail.json);
    assert_eq!(detail.json["id"], p1.to_string());
    assert_eq!(detail.json["equipment"]["management_no"], "290");
    assert_eq!(detail.json["equipment"]["model"], "GTS25DE");
    assert_eq!(detail.json["assignments"].as_array().unwrap().len(), 1);
    assert_eq!(detail.json["approval_line"].as_array().unwrap().len(), 3);
    assert!(!detail.json["status_history"].as_array().unwrap().is_empty());
    assert_eq!(detail.json["evidence"].as_array().unwrap().len(), 1);

    let cross_branch_detail =
        get_json(service, &format!("/api/v1/work-orders/{hidden}"), &token).await;
    assert_eq!(cross_branch_detail.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn approval_items_are_server_federated_and_branch_scoped(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Approval Region", "Approval Branch").await;
    let other_branch_id =
        seed_branch(&pool, "Approval Other Region", "Approval Other Branch").await;
    let receptionist = UserId::new();
    let mechanic = UserId::new();
    let admin = UserId::new();
    seed_user_with_branch(&pool, receptionist, "RECEPTIONIST", branch_id).await;
    seed_user_with_branch(&pool, mechanic, "MECHANIC", branch_id).await;
    seed_user_with_branch(&pool, admin, "ADMIN", branch_id).await;
    let equipment = seed_equipment_record(&pool, branch_id, "391", "GTS35DE").await;
    let other_equipment = seed_equipment_record(&pool, other_branch_id, "392", "HIDDEN").await;

    let work_order_id = seed_report_submitted_work_order(
        &pool,
        branch_id,
        equipment,
        receptionist,
        mechanic,
        admin,
        "20260612-931",
    )
    .await;
    let daily_plan_id = seed_requested_daily_plan(&pool, branch_id, mechanic, "2026-06-29").await;
    let target_change_id = seed_requested_target_change(&pool, work_order_id, admin).await;
    let hidden_work_order_id = seed_report_submitted_work_order(
        &pool,
        other_branch_id,
        other_equipment,
        receptionist,
        mechanic,
        admin,
        "20260612-932",
    )
    .await;
    seed_requested_target_change(&pool, hidden_work_order_id, admin).await;

    let admin_token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        admin,
        vec!["ADMIN".to_owned()],
        vec![branch_id],
    )
    .await
    .unwrap();
    let mechanic_token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        mechanic,
        vec!["MECHANIC".to_owned()],
        vec![branch_id],
    )
    .await
    .unwrap();
    let service = build_router(
        app_state(
            console_platform_test_support::runtime_role_pool(&pool).await,
            public_key_pem,
        )
        .unwrap(),
    );

    let denied = get_json(
        service.clone(),
        "/api/approval-items?limit=50&offset=0",
        &mechanic_token,
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{:?}", denied.json);

    let page = get_json(
        service,
        "/api/approval-items?limit=50&offset=0",
        &admin_token,
    )
    .await;
    assert_eq!(page.status, StatusCode::OK, "{:?}", page.json);
    assert_eq!(page.json["total"], 3);
    assert_eq!(page.json["limit"], 50);
    assert_eq!(page.json["offset"], 0);

    let items = page.json["items"].as_array().unwrap();
    let sources = items
        .iter()
        .map(|item| item["source"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(sources.contains(&"WORK_ORDER"));
    assert!(sources.contains(&"DAILY_PLAN"));
    assert!(sources.contains(&"TARGET_CHANGE"));
    assert!(
        !items
            .iter()
            .any(|item| item["source_id"] == hidden_work_order_id.to_string())
    );

    let work_item = items
        .iter()
        .find(|item| item["source"] == "WORK_ORDER")
        .unwrap();
    assert_eq!(work_item["source_id"], work_order_id.to_string());
    assert_eq!(work_item["work_order"]["request_no"], "20260612-931");
    assert_eq!(work_item["ontology"]["object_type"], "WORK_ORDER");
    assert_eq!(
        work_item["ontology"]["object_id"],
        work_order_id.to_string()
    );
    assert_eq!(work_item["ontology"]["branch_id"], branch_id.to_string());
    assert_eq!(
        work_item["workflow"]["workflow_key"],
        "work_order.report_completion_review"
    );
    assert_eq!(work_item["policy"]["enforcement"], "server");
    assert_eq!(
        work_item["policy"]["required_features"][0],
        "completion_review"
    );
    assert!(work_item.get("daily_plan").is_none());
    assert!(work_item.get("target_change").is_none());

    let daily_item = items
        .iter()
        .find(|item| item["source"] == "DAILY_PLAN")
        .unwrap();
    assert_eq!(daily_item["source_id"], daily_plan_id.to_string());
    assert_eq!(daily_item["daily_plan"]["status"], "REQUESTED");

    let target_item = items
        .iter()
        .find(|item| item["source"] == "TARGET_CHANGE")
        .unwrap();
    assert_eq!(target_item["source_id"], target_change_id.to_string());
    assert_eq!(
        target_item["target_change"]["work_order_id"],
        work_order_id.to_string()
    );

    let source_counts = page.json["sources"].as_array().unwrap();
    assert_eq!(
        source_counts
            .iter()
            .find(|source| source["key"] == "workOrders")
            .unwrap()["count"],
        1,
    );
    assert_eq!(
        source_counts
            .iter()
            .find(|source| source["key"] == "dailyPlans")
            .unwrap()["count"],
        1,
    );
    assert_eq!(
        source_counts
            .iter()
            .find(|source| source["key"] == "targetChanges")
            .unwrap()["count"],
        1,
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn kpi_endpoint_is_jwt_authorized_and_branch_scoped(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "KPI Region", "KPI Branch").await;
    let other_branch_id = seed_branch(&pool, "KPI Other Region", "KPI Other Branch").await;
    let admin = UserId::new();
    let mechanic = UserId::new();
    seed_user_with_branch(&pool, admin, "ADMIN", branch_id).await;
    seed_user_with_branch(&pool, mechanic, "MECHANIC", branch_id).await;
    let equipment = seed_equipment_record(&pool, branch_id, "290", "GTS25DE").await;
    let other_equipment = seed_equipment_record(&pool, other_branch_id, "777", "OTHER").await;
    let created_at = OffsetDateTime::parse("2026-06-12T08:00:00Z", &Rfc3339).unwrap();
    seed_kpi_completed_work_order(
        &pool,
        KpiWorkOrderFixture {
            branch_id,
            equipment,
            actor: admin,
            mechanic,
            request_no: "20260612-950",
            priority: "P1",
            created_at,
            approved_at: created_at + Duration::hours(8),
        },
    )
    .await;
    seed_kpi_completed_work_order(
        &pool,
        KpiWorkOrderFixture {
            branch_id: other_branch_id,
            equipment: other_equipment,
            actor: admin,
            mechanic,
            request_no: "20260612-951",
            priority: "P2",
            created_at,
            approved_at: created_at + Duration::hours(9),
        },
    )
    .await;

    let admin_token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        admin,
        vec!["ADMIN".to_owned()],
        vec![branch_id],
    )
    .await
    .unwrap();
    let mechanic_token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        mechanic,
        vec!["MECHANIC".to_owned()],
        vec![branch_id],
    )
    .await
    .unwrap();
    let service = build_router(
        app_state(
            console_platform_test_support::runtime_role_pool(&pool).await,
            public_key_pem,
        )
        .unwrap(),
    );

    let denied = get_json(
        service.clone(),
        "/api/v1/kpi?period=2026-06-01..2026-07-01&scope=company",
        &mechanic_token,
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{:?}", denied.json);

    // The KPI Excel export exposes the same KpiRead-gated data, so a role without
    // KpiRead (mechanic) must be denied there too, not only on the JSON endpoint.
    let export_denied = get_json(
        service.clone(),
        "/api/v1/exports/kpi?period=2026-06-01..2026-07-01&scope=company",
        &mechanic_token,
    )
    .await;
    assert_eq!(
        export_denied.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        export_denied.json
    );

    let report = get_json(
        service,
        "/api/v1/kpi?period=2026-06-01..2026-07-01&scope=company",
        &admin_token,
    )
    .await;
    assert_eq!(report.status, StatusCode::OK, "{:?}", report.json);
    let company = report.json["rollups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rollup| rollup["scope"]["kind"] == "company")
        .unwrap();
    assert_eq!(company["completed_count"], 1);
    assert_eq!(company["weighted_completed_points"], 3);
    assert_eq!(
        report.json["rollups"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|rollup| rollup["scope"]["kind"] == "branch")
            .count(),
        1
    );
    assert_eq!(company["inspection_schedule_due_count"], 0);
    assert_eq!(company["inspection_schedule_completed_count"], 0);
    assert_eq!(
        company["inspection_plan_completion_bps"],
        serde_json::Value::Null
    );
    assert!(
        !report.json["unavailable_metrics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|metric| metric["metric"] == "inspection_plan_completion_rate")
    );
    // P1 acceptance is now computed from the p1_dispatch* source tables (present
    // in this migration set), so it is no longer reported as unavailable; with no
    // dispatches seeded the rate is null and the counts are zero.
    assert_eq!(company["p1_dispatch_count"], 0);
    assert_eq!(company["p1_accepted_count"], 0);
    assert_eq!(company["p1_acceptance_bps"], serde_json::Value::Null);
    assert!(
        !report.json["unavailable_metrics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|metric| metric["metric"] == "p1_acceptance_rate")
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn equipment_lookup_and_autocomplete_are_branch_scoped(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Equipment Region", "Equipment Branch").await;
    let other_branch_id = seed_branch(&pool, "Equipment Other Region", "Equipment Other").await;
    let receptionist = UserId::new();
    seed_user_with_branch(&pool, receptionist, "RECEPTIONIST", branch_id).await;
    seed_equipment_record(&pool, branch_id, "290", "GTS25DE").await;
    seed_equipment_record(&pool, branch_id, "291", "GTS30DE").await;
    seed_equipment_record(&pool, other_branch_id, "290", "SHOULD_NOT_LEAK").await;
    let token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        receptionist,
        vec!["RECEPTIONIST".to_owned()],
        vec![branch_id],
    )
    .await
    .unwrap();
    let service = build_router(
        app_state(
            console_platform_test_support::runtime_role_pool(&pool).await,
            public_key_pem,
        )
        .unwrap(),
    );

    let lookup = get_json(
        service.clone(),
        "/api/v1/equipment/lookup?management_no=%23290",
        &token,
    )
    .await;
    assert_eq!(lookup.status, StatusCode::OK, "{:?}", lookup.json);
    assert_eq!(lookup.json["management_no"], "290");
    assert_eq!(lookup.json["model"], "GTS25DE");
    assert_eq!(lookup.json["customer"]["name"], "Customer 290");
    assert_eq!(lookup.json["site"]["name"], "Site 290");

    let autocomplete = get_json(service, "/api/v1/equipment?q=29&limit=10", &token).await;
    assert_eq!(
        autocomplete.status,
        StatusCode::OK,
        "{:?}",
        autocomplete.json
    );
    let models = autocomplete.json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["model"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(models, vec!["GTS25DE", "GTS30DE"]);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn reject_with_memo_is_admin_only_and_audited(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Reject Region", "Reject Branch").await;
    let mechanic = UserId::new();
    let receptionist = UserId::new();
    let admin = UserId::new();
    seed_user_with_branch(&pool, mechanic, "MECHANIC", branch_id).await;
    seed_user_with_branch(&pool, receptionist, "RECEPTIONIST", branch_id).await;
    seed_user_with_branch(&pool, admin, "ADMIN", branch_id).await;
    let equipment = seed_equipment_record(&pool, branch_id, "290", "GTS25DE").await;
    let work_order_id =
        seed_received_work_order(&pool, branch_id, equipment, receptionist, "20260612-904").await;
    let mechanic_token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        mechanic,
        vec!["MECHANIC".to_owned()],
        vec![branch_id],
    )
    .await
    .unwrap();
    let admin_token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        admin,
        vec!["ADMIN".to_owned()],
        vec![branch_id],
    )
    .await
    .unwrap();
    let service = build_router(
        app_state(
            console_platform_test_support::runtime_role_pool(&pool).await,
            public_key_pem,
        )
        .unwrap(),
    );

    let denied = post_json(
        service.clone(),
        &format!("/api/v1/work-orders/{work_order_id}/reject"),
        &mechanic_token,
        json!({ "memo": "not allowed" }),
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{:?}", denied.json);

    let missing_memo = post_json(
        service.clone(),
        &format!("/api/v1/work-orders/{work_order_id}/reject"),
        &admin_token,
        json!({ "memo": "   " }),
    )
    .await;
    assert_eq!(
        missing_memo.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        missing_memo.json
    );
    let status_after_missing_memo: String =
        sqlx::query_scalar("SELECT status FROM work_orders WHERE id = $1")
            .bind(*work_order_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status_after_missing_memo, "RECEIVED");
    let reject_audit_count_before_success: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'work_order.reject' AND target_id = $1",
    )
    .bind(work_order_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(reject_audit_count_before_success, 0);

    let rejected = post_json(
        service,
        &format!("/api/v1/work-orders/{work_order_id}/reject"),
        &admin_token,
        json!({ "memo": "Duplicate request from customer" }),
    )
    .await;
    assert_eq!(rejected.status, StatusCode::OK, "{:?}", rejected.json);
    assert_eq!(rejected.json["status"], "REJECTED");

    let memo: String = sqlx::query_scalar(
        r#"
        SELECT after_snap->>'memo'
        FROM audit_events
        WHERE action = 'work_order.reject' AND target_id = $1
        "#,
    )
    .bind(work_order_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(memo, "Duplicate request from customer");
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

async fn get_json(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    let response = service
        .oneshot(
            Request::builder()
                .uri(uri)
                .method("GET")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    response_json(response).await
}

async fn post_json(service: axum::Router, uri: &str, token: &str, body: Value) -> JsonResponse {
    let response = service
        .oneshot(
            Request::builder()
                .uri(uri)
                .method("POST")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    response_json(response).await
}

async fn response_json(response: http::Response<Body>) -> JsonResponse {
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
}

async fn issue_token(
    pool: &PgPool,
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    roles: Vec<String>,
    branches: Vec<BranchId>,
) -> Result<String, Box<dyn std::error::Error>> {
    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private_key_pem,
        public_key_pem,
    )?;

    Ok(console_platform_test_support::issue_session_token(
        pool,
        &issuer,
        AccessTokenInput {
            subject: user_id,
            org_id: OrgId::knl(),
            roles,
            branches,
            platform: false,
            view_as: false,
            read_only: false,
            display_name: None,
            feature_grants: Vec::new(),
            authz_subject_version: 0,
            authz_policy_version: 0,
            session_generation: 0,
            issued_at: OffsetDateTime::now_utc(),
        },
        None,
        vec![],
    )
    .await)
}

fn app_state(pool: PgPool, public_key_pem: String) -> Result<AppState, console_app::AppError> {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key_pem),
    ])?;

    AppState::new(config, DatabaseDependency::Postgres(pool))
}

async fn seed_branch(pool: &PgPool, region_name: &str, branch_name: &str) -> BranchId {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(region_name)
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(branch_name)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user_with_branch(pool: &PgPool, user_id: UserId, role: &str, branch_id: BranchId) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("Workorder API {role}"))
        .bind(Vec::from([role]))
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_equipment(pool: &PgPool, branch_id: BranchId, management_no: &str) {
    seed_equipment_record(pool, branch_id, management_no, "GTS25DE").await;
}

#[derive(Clone, Copy)]
struct SeededEquipment {
    id: uuid::Uuid,
    customer_id: uuid::Uuid,
    site_id: uuid::Uuid,
}

struct ReadWorkOrderFixture {
    branch_id: BranchId,
    equipment: SeededEquipment,
    receptionist: UserId,
    mechanic: UserId,
    request_no: &'static str,
    priority: &'static str,
    target_due_at: OffsetDateTime,
}

struct KpiWorkOrderFixture {
    branch_id: BranchId,
    equipment: SeededEquipment,
    actor: UserId,
    mechanic: UserId,
    request_no: &'static str,
    priority: &'static str,
    created_at: OffsetDateTime,
    approved_at: OffsetDateTime,
}

async fn seed_equipment_record(
    pool: &PgPool,
    branch_id: BranchId,
    management_no: &str,
    model: &str,
) -> SeededEquipment {
    let equipment_suffix = format!("{:0>4}", management_no);
    let equipment_prefix = format!(
        "{}12",
        model
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .take(3)
            .collect::<String>()
            .to_ascii_uppercase()
    );
    let customer_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(format!("Customer {management_no}"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(format!("Site {management_no}"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let equipment_id = sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, model, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, $4, $5,
                'A', 'B', 'C', '임대', '좌식', '2.5', $6, 'test', 1, $7)
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(format!("{equipment_prefix}-{equipment_suffix}"))
    .bind(management_no)
    .bind(model)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();

    SeededEquipment {
        id: equipment_id,
        customer_id,
        site_id,
    }
}

async fn seed_kpi_completed_work_order(pool: &PgPool, fixture: KpiWorkOrderFixture) -> WorkOrderId {
    let work_order_id = WorkOrderId::new();
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, priority, symptom, result_type,
            report_submitted_by, report_submitted_at, created_at, updated_at, org_id
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, 'FINAL_COMPLETED', $8, 'KPI fixture',
            'COMPLETED', $9, $10, $11, $10, $12
        )
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(fixture.request_no)
    .bind(*fixture.branch_id.as_uuid())
    .bind(fixture.equipment.id)
    .bind(fixture.equipment.customer_id)
    .bind(fixture.equipment.site_id)
    .bind(*fixture.actor.as_uuid())
    .bind(fixture.priority)
    .bind(*fixture.mechanic.as_uuid())
    .bind(fixture.approved_at)
    .bind(fixture.created_at)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO work_order_assignments (work_order_id, mechanic_id, role, assigned_at, org_id)
        VALUES ($1, $2, 'PRIMARY', $3, $4)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*fixture.mechanic.as_uuid())
    .bind(fixture.created_at + Duration::minutes(30))
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO work_order_approval_steps (
            work_order_id, step_order, role, approver_id, status,
            requested_at, approved_at, approved_by_id, org_id
        )
        VALUES ($1, 2, 'ADMIN', $2, 'APPROVED', $3, $3, $2, $4)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*fixture.actor.as_uuid())
    .bind(fixture.approved_at)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    for (action, from_status, to_status, occurred_at) in [
        ("work_order.create", None, "RECEIVED", fixture.created_at),
        (
            "work_order.start",
            Some("ASSIGNED"),
            "IN_PROGRESS",
            fixture.created_at + Duration::hours(1),
        ),
        (
            "work_order.approve",
            Some("ADMIN_REVIEW"),
            "FINAL_COMPLETED",
            fixture.approved_at,
        ),
    ] {
        sqlx::query(
            r#"
            INSERT INTO work_order_status_history (
                work_order_id, actor, action, from_status, to_status, occurred_at, org_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(*work_order_id.as_uuid())
        .bind(*fixture.actor.as_uuid())
        .bind(action)
        .bind(from_status)
        .bind(to_status)
        .bind(occurred_at)
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    }
    work_order_id
}

async fn seed_received_work_order(
    pool: &PgPool,
    branch_id: BranchId,
    equipment: SeededEquipment,
    receptionist: UserId,
    request_no: &str,
) -> WorkOrderId {
    let work_order_id = WorkOrderId::new();
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, priority, symptom, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'RECEIVED', 'UNSET', 'Read fixture', $8)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(request_no)
    .bind(*branch_id.as_uuid())
    .bind(equipment.id)
    .bind(equipment.customer_id)
    .bind(equipment.site_id)
    .bind(*receptionist.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    work_order_id
}

async fn seed_report_submitted_work_order(
    pool: &PgPool,
    branch_id: BranchId,
    equipment: SeededEquipment,
    receptionist: UserId,
    mechanic: UserId,
    admin: UserId,
    request_no: &str,
) -> WorkOrderId {
    let work_order_id = WorkOrderId::new();
    let submitted_at = OffsetDateTime::parse("2026-06-28T01:00:00Z", &Rfc3339).unwrap();
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, priority, symptom, result_type, diagnosis,
            action_taken, target_due_at, report_submitted_by, report_submitted_at,
            created_at, updated_at, org_id
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, 'REPORT_SUBMITTED', 'P1',
            'Approval fixture', 'COMPLETED', 'diagnosis', 'action taken',
            $8, $9, $10, $10, $10, $11
        )
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(request_no)
    .bind(*branch_id.as_uuid())
    .bind(equipment.id)
    .bind(equipment.customer_id)
    .bind(equipment.site_id)
    .bind(*receptionist.as_uuid())
    .bind(submitted_at + Duration::days(1))
    .bind(*mechanic.as_uuid())
    .bind(submitted_at)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO work_order_assignments (work_order_id, mechanic_id, role, assigned_at, org_id)
        VALUES ($1, $2, 'PRIMARY', $3, $4)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*mechanic.as_uuid())
    .bind(submitted_at - Duration::hours(1))
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    for (step_order, role, approver_id, status, requested_at) in [
        (
            1_i16,
            "MECHANIC",
            Some(mechanic),
            "APPROVED",
            Some(submitted_at),
        ),
        (2_i16, "ADMIN", Some(admin), "PENDING", Some(submitted_at)),
        (3_i16, "EXECUTIVE", None, "NOT_STARTED", None),
    ] {
        sqlx::query(
            r#"
            INSERT INTO work_order_approval_steps (
                work_order_id, step_order, role, approver_id, status, requested_at, org_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(*work_order_id.as_uuid())
        .bind(step_order)
        .bind(role)
        .bind(approver_id.map(|id| *id.as_uuid()))
        .bind(status)
        .bind(requested_at)
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    }
    work_order_id
}

async fn seed_requested_daily_plan(
    pool: &PgPool,
    branch_id: BranchId,
    mechanic: UserId,
    plan_date: &str,
) -> uuid::Uuid {
    let plan_date = time::Date::parse(
        plan_date,
        time::macros::format_description!("[year]-[month]-[day]"),
    )
    .unwrap();
    sqlx::query_scalar(
        r#"
        INSERT INTO daily_work_plans (
            branch_id, mechanic_id, plan_date, status, requested_at, org_id
        )
        VALUES ($1, $2, $3, 'REQUESTED', $4, $5)
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(*mechanic.as_uuid())
    .bind(plan_date)
    .bind(OffsetDateTime::parse("2026-06-28T02:00:00Z", &Rfc3339).unwrap())
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_requested_target_change(
    pool: &PgPool,
    work_order_id: WorkOrderId,
    requested_by: UserId,
) -> uuid::Uuid {
    sqlx::query_scalar(
        r#"
        INSERT INTO target_change_requests (
            work_order_id, requested_by, requested_target_due_at, reason, status, created_at, org_id
        )
        VALUES ($1, $2, $3, 'Approval federation fixture', 'REQUESTED', $4, $5)
        RETURNING id
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*requested_by.as_uuid())
    .bind(OffsetDateTime::parse("2026-07-05T00:00:00Z", &Rfc3339).unwrap())
    .bind(OffsetDateTime::parse("2026-06-28T03:00:00Z", &Rfc3339).unwrap())
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_read_work_order(pool: &PgPool, fixture: ReadWorkOrderFixture) -> WorkOrderId {
    let work_order_id = WorkOrderId::new();
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, priority, symptom, target_due_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'ASSIGNED', $8, 'Read fixture', $9, $10)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(fixture.request_no)
    .bind(*fixture.branch_id.as_uuid())
    .bind(fixture.equipment.id)
    .bind(fixture.equipment.customer_id)
    .bind(fixture.equipment.site_id)
    .bind(*fixture.receptionist.as_uuid())
    .bind(fixture.priority)
    .bind(fixture.target_due_at)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO work_order_assignments (work_order_id, mechanic_id, role, assigned_at, org_id)
        VALUES ($1, $2, 'PRIMARY', now(), $3)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*fixture.mechanic.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    for (step_order, role, status) in [
        (1_i16, "MECHANIC", "PENDING"),
        (2_i16, "ADMIN", "NOT_STARTED"),
        (3_i16, "EXECUTIVE", "NOT_STARTED"),
    ] {
        sqlx::query(
            r#"
            INSERT INTO work_order_approval_steps (work_order_id, step_order, role, status, org_id)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(*work_order_id.as_uuid())
        .bind(step_order)
        .bind(role)
        .bind(status)
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query(
        r#"
        INSERT INTO work_order_status_history (
            work_order_id, actor, action, from_status, to_status, occurred_at, org_id
        )
        VALUES ($1, $2, 'work_order.assign', 'RECEIVED', 'ASSIGNED', now(), $3)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*fixture.receptionist.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO evidence_media (
            work_order_id, stage, s3_key, content_type, size_bytes,
            uploaded_by, worm_replica_status, org_id
        )
        VALUES ($1, 'BEFORE', $2, 'image/jpeg', 128, $3, 'VERIFIED', $4)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(format!("work-orders/{work_order_id}/before.jpg"))
    .bind(*fixture.mechanic.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    work_order_id
}
