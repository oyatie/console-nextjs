#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Runtime proof for the branch-explicit purchase-request collection.
//!
//! The assertions use the assembled app router over the non-owner runtime role
//! so authorization, tenant RLS, status filtering, pagination, empty results,
//! and the public error envelope are exercised together.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::Value;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const PATH: &str = "/api/v1/financial/purchase-requests";

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn purchase_request_collection_is_branch_scoped_requester_safe_and_tenant_isolated(
    owner_pool: PgPool,
) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let org_a = OrgId::knl();
    let org_b = OrgId::new();
    // The migration set owns the canonical KNL tenant row. Re-inserting it
    // makes this otherwise isolated sqlx test fail on the primary key before
    // the collection contract is exercised.
    seed_org(&owner_pool, org_b, "other", "Other tenant").await;
    let branch_a = seed_branch(&owner_pool, org_a, "A").await;
    let branch_b = seed_branch(&owner_pool, org_a, "B").await;
    let tenant_b_branch = seed_branch(&owner_pool, org_b, "B1").await;

    let requester = seed_user(&owner_pool, org_a, branch_a, "MEMBER", "Requester").await;
    let other_requester = seed_user(&owner_pool, org_a, branch_a, "MEMBER", "Other").await;
    let reader = seed_user(&owner_pool, org_a, branch_a, "RECEPTIONIST", "Reader").await;
    let wrong_branch_reader =
        seed_user(&owner_pool, org_a, branch_b, "RECEPTIONIST", "Other branch").await;
    let tenant_b_reader = seed_user(
        &owner_pool,
        org_b,
        tenant_b_branch,
        "RECEPTIONIST",
        "Other tenant",
    )
    .await;
    let tenant_b_admin = seed_user(
        &owner_pool,
        org_b,
        tenant_b_branch,
        "SUPER_ADMIN",
        "Tenant B admin",
    )
    .await;

    let own_id = seed_purchase(
        &owner_pool,
        org_a,
        branch_a,
        requester,
        "STATEMENT_ATTACHED",
        1,
    )
    .await;
    let submitted_id = seed_purchase(
        &owner_pool,
        org_a,
        branch_a,
        other_requester,
        "REQUEST_SUBMITTED",
        2,
    )
    .await;
    let _tenant_b_id = seed_purchase(
        &owner_pool,
        org_b,
        tenant_b_branch,
        tenant_b_reader,
        "STATEMENT_ATTACHED",
        3,
    )
    .await;

    // Seed lines out of order and attachment states at distinct timestamps so
    // the assembled collection proves its batch hydration contracts.
    seed_purchase_line(&owner_pool, org_a, own_id, 2, "Own replacement filter", 12).await;
    seed_purchase_line(&owner_pool, org_a, own_id, 1, "Own pump assembly", 11).await;
    seed_purchase_line(
        &owner_pool,
        org_a,
        submitted_id,
        2,
        "Submitted safety valve",
        22,
    )
    .await;
    seed_purchase_line(
        &owner_pool,
        org_a,
        submitted_id,
        1,
        "Submitted control panel",
        21,
    )
    .await;

    seed_purchase_attachment(
        &owner_pool,
        org_a,
        branch_a,
        requester,
        own_id,
        "own-confirmed-old.pdf",
        "CONFIRMED",
        31,
    )
    .await;
    seed_purchase_attachment(
        &owner_pool,
        org_a,
        branch_a,
        requester,
        own_id,
        "own-confirmed-new.pdf",
        "CONFIRMED",
        32,
    )
    .await;
    seed_purchase_attachment(
        &owner_pool,
        org_a,
        branch_a,
        requester,
        own_id,
        "own-pending.pdf",
        "PENDING",
        33,
    )
    .await;
    seed_purchase_attachment(
        &owner_pool,
        org_a,
        branch_a,
        other_requester,
        submitted_id,
        "submitted-confirmed-old.pdf",
        "CONFIRMED",
        41,
    )
    .await;
    seed_purchase_attachment(
        &owner_pool,
        org_a,
        branch_a,
        other_requester,
        submitted_id,
        "submitted-confirmed-new.pdf",
        "CONFIRMED",
        42,
    )
    .await;
    seed_purchase_attachment(
        &owner_pool,
        org_a,
        branch_a,
        other_requester,
        submitted_id,
        "submitted-pending.pdf",
        "PENDING",
        43,
    )
    .await;

    let requester_token = issue_token(
        private_pem.as_bytes(),
        public_pem.as_bytes(),
        requester,
        org_a,
        vec!["MEMBER".to_owned()],
    );
    let reader_token = issue_token(
        private_pem.as_bytes(),
        public_pem.as_bytes(),
        reader,
        org_a,
        vec!["RECEPTIONIST".to_owned()],
    );
    let wrong_branch_token = issue_token(
        private_pem.as_bytes(),
        public_pem.as_bytes(),
        wrong_branch_reader,
        org_a,
        vec!["RECEPTIONIST".to_owned()],
    );
    let tenant_b_reader_token = issue_token(
        private_pem.as_bytes(),
        public_pem.as_bytes(),
        tenant_b_reader,
        org_b,
        vec!["RECEPTIONIST".to_owned()],
    );
    let tenant_b_admin_token = issue_token(
        private_pem.as_bytes(),
        public_pem.as_bytes(),
        tenant_b_admin,
        org_b,
        vec!["SUPER_ADMIN".to_owned()],
    );
    let service = build_router(app_state(console_rt_pool(&owner_pool).await, public_pem));

    let requester_page = get(
        service.clone(),
        &format!("{PATH}?branch_id={branch_a}&limit=10&offset=0"),
        &requester_token,
    )
    .await;
    assert_eq!(requester_page.status(), StatusCode::OK);
    let requester_page = body_json(requester_page).await;
    assert_eq!(requester_page["total"], 1);
    assert_eq!(requester_page["items"][0]["id"], own_id.to_string());
    assert_eq!(
        requester_page["items"][0]["requester"]["display_name"],
        "Requester"
    );
    assert_purchase_item_hydration(
        &requester_page["items"][0],
        own_id,
        &["Own pump assembly", "Own replacement filter"],
        &["own-confirmed-new.pdf", "own-confirmed-old.pdf"],
        "own-pending.pdf",
    );

    let status_filtered = get(
        service.clone(),
        &format!("{PATH}?branch_id={branch_a}&status=REQUEST_SUBMITTED&limit=1&offset=0"),
        &reader_token,
    )
    .await;
    assert_eq!(status_filtered.status(), StatusCode::OK);
    let status_filtered = body_json(status_filtered).await;
    assert_eq!(status_filtered["total"], 1);
    assert_eq!(status_filtered["items"][0]["id"], submitted_id.to_string());

    let repeated_statuses = get(
        service.clone(),
        &format!(
            "{PATH}?branch_id={branch_a}&status=STATEMENT_ATTACHED&status=REQUEST_SUBMITTED&limit=10&offset=0"
        ),
        &reader_token,
    )
    .await;
    assert_eq!(repeated_statuses.status(), StatusCode::OK);
    let repeated_statuses = body_json(repeated_statuses).await;
    assert_eq!(repeated_statuses["total"], 2);
    let repeated_items = repeated_statuses["items"].as_array().unwrap();
    assert_eq!(repeated_items.len(), 2);
    assert_purchase_item_hydration(
        purchase_item(&repeated_statuses, own_id),
        own_id,
        &["Own pump assembly", "Own replacement filter"],
        &["own-confirmed-new.pdf", "own-confirmed-old.pdf"],
        "own-pending.pdf",
    );
    assert_purchase_item_hydration(
        purchase_item(&repeated_statuses, submitted_id),
        submitted_id,
        &["Submitted control panel", "Submitted safety valve"],
        &["submitted-confirmed-new.pdf", "submitted-confirmed-old.pdf"],
        "submitted-pending.pdf",
    );

    let first = get(
        service.clone(),
        &format!("{PATH}?branch_id={branch_a}&limit=1&offset=0"),
        &reader_token,
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first = body_json(first).await;
    assert_eq!(first["total"], 2);
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    let second = get(
        service.clone(),
        &format!("{PATH}?branch_id={branch_a}&limit=1&offset=1"),
        &reader_token,
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);
    let second = body_json(second).await;
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_ne!(first["items"][0]["id"], second["items"][0]["id"]);

    let denied_branch = get(
        service.clone(),
        &format!("{PATH}?branch_id={branch_a}&limit=10&offset=0"),
        &wrong_branch_token,
    )
    .await;
    assert_error(denied_branch, StatusCode::FORBIDDEN, "forbidden").await;
    let denied_tenant_branch = get(
        service.clone(),
        &format!("{PATH}?branch_id={branch_a}&limit=10&offset=0"),
        &tenant_b_reader_token,
    )
    .await;
    assert_error(denied_tenant_branch, StatusCode::FORBIDDEN, "forbidden").await;

    // An org-wide role in another tenant may name the UUID, but FORCE RLS
    // still returns no org-A data.
    let cross_tenant = get(
        service.clone(),
        &format!("{PATH}?branch_id={branch_a}&limit=10&offset=0"),
        &tenant_b_admin_token,
    )
    .await;
    assert_eq!(cross_tenant.status(), StatusCode::OK);
    let cross_tenant = body_json(cross_tenant).await;
    assert_eq!(cross_tenant["total"], 0);
    assert_eq!(cross_tenant["items"], serde_json::json!([]));

    let empty = get(
        service.clone(),
        &format!("{PATH}?branch_id={branch_b}&limit=10&offset=0"),
        &wrong_branch_token,
    )
    .await;
    assert_eq!(empty.status(), StatusCode::OK);
    let empty = body_json(empty).await;
    assert_eq!(empty["total"], 0);
    assert_eq!(empty["items"], serde_json::json!([]));

    let invalid_status = get(
        service.clone(),
        &format!("{PATH}?branch_id={branch_a}&status=NOT_A_STATUS"),
        &reader_token,
    )
    .await;
    assert_error(
        invalid_status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation",
    )
    .await;
    let invalid_page = get(
        service.clone(),
        &format!("{PATH}?branch_id={branch_a}&limit=101"),
        &reader_token,
    )
    .await;
    assert_error(invalid_page, StatusCode::UNPROCESSABLE_ENTITY, "validation").await;
    for malformed in [
        format!("{PATH}?branch_id=not-a-uuid"),
        format!("{PATH}?branch_id={branch_a}&limit=one"),
        format!("{PATH}?branch_id={branch_a}&offset=one"),
        format!("{PATH}?branch_id={branch_a}&status[]=STATEMENT_ATTACHED"),
        format!("{PATH}?branch_id={branch_a}&unexpected=value"),
    ] {
        let response = get(service.clone(), &malformed, &reader_token).await;
        assert_error(response, StatusCode::UNPROCESSABLE_ENTITY, "validation").await;
    }
}

fn purchase_item(page: &Value, id: uuid::Uuid) -> &Value {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id.to_string())
        .unwrap()
}

fn assert_purchase_item_hydration(
    item: &Value,
    purchase_id: uuid::Uuid,
    expected_line_items: &[&str],
    expected_attachment_names: &[&str],
    excluded_attachment_name: &str,
) {
    let lines = item["lines"].as_array().unwrap();
    assert_eq!(lines.len(), expected_line_items.len());
    assert_eq!(
        lines
            .iter()
            .map(|line| line["line_no"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        (1..=expected_line_items.len() as i64).collect::<Vec<_>>()
    );
    assert_eq!(
        lines
            .iter()
            .map(|line| line["item"].as_str().unwrap())
            .collect::<Vec<_>>(),
        expected_line_items
    );

    let attachments = item["quote_attachments"].as_array().unwrap();
    assert_eq!(attachments.len(), expected_attachment_names.len());
    assert_eq!(
        attachments
            .iter()
            .map(|attachment| attachment["file_name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        expected_attachment_names
    );
    assert!(attachments.iter().all(|attachment| {
        attachment["download_url"]
            .as_str()
            .unwrap()
            .contains(&format!("/purchase-requests/{purchase_id}/attachments/"))
    }));
    assert!(
        attachments
            .iter()
            .all(|attachment| attachment["file_name"] != excluded_attachment_name)
    );
}

async fn get(service: axum::Router, uri: &str, token: &str) -> axum::response::Response {
    service
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

async fn assert_error(response: axum::response::Response, status: StatusCode, code: &str) {
    assert_eq!(response.status(), status);
    assert_eq!(body_json(response).await["error"]["code"], code);
}

async fn seed_org(pool: &PgPool, org: OrgId, slug: &str, name: &str) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(*org.as_uuid())
        .bind(slug)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_branch(pool: &PgPool, org: OrgId, name: &str) -> BranchId {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Region {name} {org}"))
            .bind(*org.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    BranchId::from_uuid(
        sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(region_id)
        .bind(name)
        .bind(*org.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}

async fn seed_user(pool: &PgPool, org: OrgId, branch: BranchId, role: &str, name: &str) -> UserId {
    let user = UserId::new();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, is_active, org_id) VALUES ($1, $2, $3, true, $4)",
    )
    .bind(*user.as_uuid())
    .bind(name)
    .bind(vec![role.to_owned()])
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user.as_uuid())
        .bind(*branch.as_uuid())
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    user
}

async fn seed_purchase(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    requester: UserId,
    status: &str,
    sequence: i64,
) -> uuid::Uuid {
    sqlx::query_scalar(
        r#"
        INSERT INTO financial_purchase_requests (
            branch_id, equipment_id, statement_evidence_id, purchase_type, vendor_name,
            amount_won, memo, status, requested_by, depreciation_method, useful_life_months,
            residual_rate_bps, declining_balance_rate_bps, management_fee_rate_bps,
            profit_rate_bps, floor_negative_quote_residual, executive_threshold_won,
            created_at, updated_at, org_id
        ) VALUES (
            $1, NULL, NULL, 'ONE_OFF', $2, $3, 'collection test', $4, $5,
            'STRAIGHT_LINE', 60, 1000, 1000, 500, 500, false, 1000000,
            now() + ($6 * interval '1 second'), now() + ($6 * interval '1 second'), $7
        ) RETURNING id
        "#,
    )
    .bind(*branch.as_uuid())
    .bind(format!("Vendor {sequence}"))
    .bind(sequence * 1_000)
    .bind(status)
    .bind(*requester.as_uuid())
    .bind(sequence)
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_purchase_line(
    pool: &PgPool,
    org: OrgId,
    purchase_request_id: uuid::Uuid,
    line_no: i32,
    item: &str,
    sequence: i64,
) {
    sqlx::query(
        r#"
        INSERT INTO financial_purchase_request_lines (
            purchase_request_id, line_no, item, quantity, unit_supply_price_won,
            vat_won, vat_overridden, line_total_won, org_id
        ) VALUES ($1, $2, $3, 1, $4, 0, false, $4, $5)
        "#,
    )
    .bind(purchase_request_id)
    .bind(line_no)
    .bind(item)
    .bind(sequence * 1_000)
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn seed_purchase_attachment(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    uploaded_by: UserId,
    purchase_request_id: uuid::Uuid,
    file_name: &str,
    upload_state: &str,
    sequence: i64,
) {
    sqlx::query(
        r#"
        INSERT INTO financial_purchase_attachments (
            branch_id, purchase_request_id, uploaded_by, role, file_name,
            content_type, size_bytes, s3_bucket, s3_key, upload_state, created_at, org_id
        ) VALUES (
            $1, $2, $3, 'QUOTE', $4, 'application/pdf', 1024, 'collection-fixtures',
            $5, $6, now() + ($7 * interval '1 second'), $8
        )
        "#,
    )
    .bind(*branch.as_uuid())
    .bind(purchase_request_id)
    .bind(*uploaded_by.as_uuid())
    .bind(file_name)
    .bind(format!("collection/{purchase_request_id}/{file_name}"))
    .bind(upload_state)
    .bind(sequence)
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn console_rt_pool(owner_pool: &PgPool) -> PgPool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for sqlx::test");
    let db_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(owner_pool)
        .await
        .unwrap();
    let base = url
        .rsplit_once('/')
        .map(|(prefix, _)| prefix.to_owned())
        .unwrap_or(url);
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|connection, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&format!("{base}/{db_name}"))
        .await
        .unwrap()
}

fn app_state(pool: PgPool, public_key_pem: String) -> AppState {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key_pem),
    ])
    .unwrap();
    AppState::new(config, DatabaseDependency::Postgres(pool)).unwrap()
}

fn issue_token(
    private_pem: &[u8],
    public_pem: &[u8],
    user_id: UserId,
    org: OrgId,
    roles: Vec<String>,
) -> String {
    JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private_pem,
        public_pem,
    )
    .unwrap()
    .issue_access_token(AccessTokenInput {
        subject: user_id,
        org_id: org,
        roles,
        branches: Vec::<BranchId>::new(),
        platform: false,
        view_as: false,
        read_only: false,
        display_name: None,
        feature_grants: Vec::new(),
        authz_subject_version: 0,
        authz_policy_version: 0,
        session_generation: 0,
        issued_at: OffsetDateTime::now_utc(),
    })
    .unwrap()
}
