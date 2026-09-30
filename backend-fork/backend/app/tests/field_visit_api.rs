#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Authenticated, runtime-role (`console_rt`) story for the field console
//! (CAP-FIELD-CONSOLE): customer site intake → link/triage → visit history →
//! resolution → customer acceptance → SLA rollup. It crosses the assembled
//! HTTP router (JWT → principal → authz → store → RLS) rather than calling
//! stores, so branch-scope confinement and Postgres RLS are both on the path.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const ISSUER: &str = "console-platform-auth";
const AUDIENCE: &str = "console-api";
const TICKETS: &str = "/api/v1/support/tickets";
const FIELD_SITES: &str = "/api/v1/field/sites";

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn authenticated_runtime_role_completes_field_visit_story(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let org = OrgId::knl();
    let branch = seed_branch(&pool, org, "field-main").await;
    let admin = seed_user(&pool, org, "ADMIN", branch).await;
    let mechanic = seed_user(&pool, org, "MECHANIC", branch).await;
    let token = keys.token(admin, org, "ADMIN", vec![branch]);

    let customer = seed_customer(&pool, org, branch, "화성정밀").await;
    let site = seed_site(&pool, org, branch, customer, "화성정밀 1공장").await;
    let equipment = seed_equipment(&pool, org, branch, customer, site).await;
    let work_order = seed_work_order(
        &pool,
        org,
        branch,
        customer,
        site,
        equipment,
        admin,
        "20260724-001",
    )
    .await;
    seed_attendance(&pool, org, branch, mechanic, work_order, site).await;

    // Intake: an internal ticket lands OPEN with an SLA due date but no site.
    let (status, ticket) = send(
        &rt,
        &keys,
        "POST",
        TICKETS,
        &token,
        Some(json!({
            "branch_id": branch, "category": "EQUIPMENT_INQUIRY", "priority": "MEDIUM",
            "title": "지게차 리프트 이상", "body": "리프트 체인 소음 발생"
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create ticket: {ticket}");
    assert!(
        ticket["site_id"].is_null(),
        "intake starts unlinked: {ticket}"
    );
    let ticket_id = ticket["id"].as_str().unwrap().to_owned();

    // Link the site: customer_id denormalizes from the site, names resolve.
    let (status, linked) = send(
        &rt,
        &keys,
        "POST",
        &format!("{TICKETS}/{ticket_id}/link"),
        &token,
        Some(json!({"site_id": site})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "link site: {linked}");
    assert_eq!(linked["site_id"].as_str().unwrap(), site.to_string());
    assert_eq!(linked["site_name"], "화성정밀 1공장");
    assert_eq!(
        linked["customer_id"].as_str().unwrap(),
        customer.to_string()
    );
    assert_eq!(linked["customer_name"], "화성정밀");

    // An empty link body is a validation error, not a silent no-op.
    let (status, empty) = send(
        &rt,
        &keys,
        "POST",
        &format!("{TICKETS}/{ticket_id}/link"),
        &token,
        Some(json!({})),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "empty link: {empty}"
    );

    // Link the dispatched work order (site matches — the guardrail passes).
    let (status, linked) = send(
        &rt,
        &keys,
        "POST",
        &format!("{TICKETS}/{ticket_id}/link"),
        &token,
        Some(json!({"work_order_id": work_order})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "link work order: {linked}");
    assert_eq!(
        linked["work_order_id"].as_str().unwrap(),
        work_order.to_string()
    );

    // Overview: the row aggregates the open ticket + active visit, SLA OK
    // (MEDIUM ⇒ 3-day window), and the derived-SLA filter matches the rows.
    let (status, page) = send(&rt, &keys, "GET", FIELD_SITES, &token, None, None).await;
    assert_eq!(status, StatusCode::OK, "field overview: {page}");
    assert_eq!(page["total"], 1);
    let row = &page["items"][0];
    assert_eq!(row["site_id"].as_str().unwrap(), site.to_string());
    assert_eq!(row["open_ticket_count"], 1);
    assert_eq!(row["breached_ticket_count"], 0);
    assert_eq!(row["active_work_order_count"], 1);
    assert_eq!(row["sla"], "OK");
    assert!(row["last_arrival_at"].is_string(), "arrival rollup: {row}");
    let (status, breached) = send(
        &rt,
        &keys,
        "GET",
        &format!("{FIELD_SITES}?sla=BREACHED"),
        &token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(breached["total"], 0, "no site is breached: {breached}");

    // Acceptance before RESOLVED is a conflict (closure evidence discipline).
    let early =
        json!({"kind": "CUSTOMER_ACCEPTED", "channel": "IN_PERSON", "accepted_by": "김담당"});
    let (status, body) = send(
        &rt,
        &keys,
        "POST",
        &format!("{TICKETS}/{ticket_id}/acceptance"),
        &token,
        Some(early.clone()),
        Some("field-acceptance-key-0001"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "acceptance requires RESOLVED: {body}"
    );

    for to in ["IN_PROGRESS", "RESOLVED"] {
        let (status, body) = send(
            &rt,
            &keys,
            "POST",
            &format!("{TICKETS}/{ticket_id}/transition"),
            &token,
            Some(json!({"to_status": to})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "transition to {to}: {body}");
    }

    // Accept: append-only evidence + the existing RESOLVED→CLOSED edge.
    let (status, acceptance) = send(
        &rt,
        &keys,
        "POST",
        &format!("{TICKETS}/{ticket_id}/acceptance"),
        &token,
        Some(early.clone()),
        Some("field-acceptance-key-0001"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "record acceptance: {acceptance}"
    );
    assert_eq!(acceptance["kind"], "CUSTOMER_ACCEPTED");
    assert_eq!(acceptance["accepted_by"], "김담당");
    let acceptance_id = acceptance["id"].as_str().unwrap().to_owned();
    let (status, closed) = send(
        &rt,
        &keys,
        "GET",
        &format!("{TICKETS}/{ticket_id}"),
        &token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        closed["ticket"]["status"], "CLOSED",
        "accept closes: {closed}"
    );

    // Idempotency: same key + same request replays the stored acceptance;
    // same key + different request is a conflict.
    let (status, replay) = send(
        &rt,
        &keys,
        "POST",
        &format!("{TICKETS}/{ticket_id}/acceptance"),
        &token,
        Some(early),
        Some("field-acceptance-key-0001"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "idempotent replay: {replay}");
    assert_eq!(replay["id"].as_str().unwrap(), acceptance_id);
    let (status, changed) = send(
        &rt,
        &keys,
        "POST",
        &format!("{TICKETS}/{ticket_id}/acceptance"),
        &token,
        Some(json!({"kind": "CUSTOMER_ACCEPTED", "channel": "PHONE", "accepted_by": "김담당"})),
        Some("field-acceptance-key-0001"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "changed replay must conflict: {changed}"
    );

    // Decline path on a second RESOLVED ticket: note required, ticket reopens,
    // and the reason lands as a customer-visible comment.
    let (_, ticket2) = send(
        &rt,
        &keys,
        "POST",
        TICKETS,
        &token,
        Some(json!({
            "branch_id": branch, "category": "COMPLAINT", "priority": "MEDIUM",
            "title": "재방문 요청", "body": "동일 증상 재발"
        })),
        None,
    )
    .await;
    let ticket2_id = ticket2["id"].as_str().unwrap().to_owned();
    send(
        &rt,
        &keys,
        "POST",
        &format!("{TICKETS}/{ticket2_id}/link"),
        &token,
        Some(json!({"site_id": site})),
        None,
    )
    .await;
    for to in ["IN_PROGRESS", "RESOLVED"] {
        send(
            &rt,
            &keys,
            "POST",
            &format!("{TICKETS}/{ticket2_id}/transition"),
            &token,
            Some(json!({"to_status": to})),
            None,
        )
        .await;
    }
    let (status, noteless) = send(
        &rt,
        &keys,
        "POST",
        &format!("{TICKETS}/{ticket2_id}/acceptance"),
        &token,
        Some(json!({"kind": "CUSTOMER_DECLINED", "channel": "PHONE", "accepted_by": "김담당"})),
        Some("field-acceptance-key-0002"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "decline requires a note: {noteless}"
    );
    let (status, declined) = send(&rt, &keys, "POST", &format!("{TICKETS}/{ticket2_id}/acceptance"), &token,
        Some(json!({"kind": "CUSTOMER_DECLINED", "channel": "PHONE", "accepted_by": "김담당", "note": "소음이 여전함"})),
        Some("field-acceptance-key-0003")).await;
    assert_eq!(status, StatusCode::CREATED, "record decline: {declined}");
    let (_, reopened) = send(
        &rt,
        &keys,
        "GET",
        &format!("{TICKETS}/{ticket2_id}"),
        &token,
        None,
        None,
    )
    .await;
    assert_eq!(
        reopened["ticket"]["status"], "IN_PROGRESS",
        "decline reopens: {reopened}"
    );
    let comments = reopened["comments"].as_array().unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c["body"] == "소음이 여전함" && c["is_internal_note"] == false),
        "decline note becomes a customer-visible comment: {reopened}"
    );

    // Detail: object + history layers all present and traversable.
    let (status, detail) = send(
        &rt,
        &keys,
        "GET",
        &format!("{FIELD_SITES}/{site}"),
        &token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "site detail: {detail}");
    assert_eq!(detail["site"]["name"], "화성정밀 1공장");
    assert_eq!(detail["site"]["customer_name"], "화성정밀");
    assert_eq!(
        detail["sla"]["open"], 1,
        "reopened decline ticket stays open: {detail}"
    );
    assert_eq!(detail["tickets"].as_array().unwrap().len(), 2);
    assert_eq!(detail["work_orders"].as_array().unwrap().len(), 1);
    assert_eq!(
        detail["work_orders"][0]["id"].as_str().unwrap(),
        work_order.to_string()
    );
    assert_eq!(detail["attendance"].as_array().unwrap().len(), 1);
    assert_eq!(detail["attendance"][0]["kind"], "ARRIVAL");
    assert_eq!(detail["acceptances"].as_array().unwrap().len(), 2);

    // Ticket list supports the per-site queue filter.
    let (status, queue) = send(
        &rt,
        &keys,
        "GET",
        &format!("{TICKETS}?site_id={site}"),
        &token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(queue["total"], 2, "site queue: {queue}");

    // Audit readback: link + acceptance events landed, and the acceptance
    // snapshot never carries the customer-side name (business fact, not log).
    let links: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = 'support.ticket.linked' AND target_id = $1")
        .bind(&ticket_id).fetch_one(&pool).await.unwrap();
    assert_eq!(links, 2, "site link and work-order link are each audited");
    let acceptance_snaps: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT after_snap::text FROM audit_events WHERE action = 'support.ticket.acceptance'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        acceptance_snaps.len(),
        2,
        "accept and decline are each audited once (replay is not re-audited)"
    );
    for snap in acceptance_snaps.into_iter().flatten() {
        assert!(
            !snap.contains("김담당"),
            "accepted_by must never enter audit snapshots: {snap}"
        );
    }
    let acceptance_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM support_ticket_acceptances")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        acceptance_rows, 2,
        "acceptance evidence is append-only, one row per verdict"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn field_console_denies_without_leakage(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let org = OrgId::knl();
    let branch_a = seed_branch(&pool, org, "field-a").await;
    let branch_b = seed_branch(&pool, org, "field-b").await;
    let admin_a = seed_user(&pool, org, "ADMIN", branch_a).await;
    let member_a = seed_user(&pool, org, "MEMBER", branch_a).await;
    let token_a = keys.token(admin_a, org, "ADMIN", vec![branch_a]);
    let member_token = keys.token(member_a, org, "MEMBER", vec![branch_a]);

    let customer_b = seed_customer(&pool, org, branch_b, "다른지점 거래처").await;
    let site_b = seed_site(&pool, org, branch_b, customer_b, "다른지점 현장").await;

    // Deny-by-omission: the out-of-branch site is absent from rows AND totals,
    // and its detail is a 404 (never a 403) so existence does not leak.
    let (status, page) = send(&rt, &keys, "GET", FIELD_SITES, &token_a, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        page["total"], 0,
        "out-of-scope site must not be counted: {page}"
    );
    assert_eq!(page["items"].as_array().unwrap().len(), 0);
    let (status, hidden) = send(
        &rt,
        &keys,
        "GET",
        &format!("{FIELD_SITES}/{site_b}"),
        &token_a,
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "out-of-scope detail is 404: {hidden}"
    );
    assert_eq!(hidden["error"]["code"], "not_found");
    assert!(
        !hidden.to_string().contains("다른지점"),
        "denial must not leak the site: {hidden}"
    );

    // Linking an out-of-scope site from an in-scope ticket is also a 404.
    let (_, ticket) = send(
        &rt,
        &keys,
        "POST",
        TICKETS,
        &token_a,
        Some(json!({
            "branch_id": branch_a, "category": "OPERATIONAL", "priority": "LOW",
            "title": "권한 검증", "body": "범위 밖 현장 연결 시도"
        })),
        None,
    )
    .await;
    let ticket_id = ticket["id"].as_str().unwrap();
    let (status, denied) = send(
        &rt,
        &keys,
        "POST",
        &format!("{TICKETS}/{ticket_id}/link"),
        &token_a,
        Some(json!({"site_id": site_b})),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "out-of-scope site link is 404: {denied}"
    );

    // Linking an out-of-scope work order is equally a 404 — its existence in
    // another branch must not surface as a wrong-site 409.
    let equipment_b = seed_equipment(&pool, org, branch_b, customer_b, site_b).await;
    let wo_b = seed_work_order(
        &pool,
        org,
        branch_b,
        customer_b,
        site_b,
        equipment_b,
        admin_a,
        "20260724-002",
    )
    .await;
    let (status, denied) = send(
        &rt,
        &keys,
        "POST",
        &format!("{TICKETS}/{ticket_id}/link"),
        &token_a,
        Some(json!({"work_order_id": wo_b})),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "out-of-scope work-order link is 404, not a 409 existence leak: {denied}"
    );

    // PBAC: the open-signup MEMBER tier is denied the field read gate
    // (work_order_read_all, mirroring the shell nav gate) and every mutation.
    let customer_a = seed_customer(&pool, org, branch_a, "본지점 거래처").await;
    let site_a = seed_site(&pool, org, branch_a, customer_a, "본지점 현장").await;
    let (status, denied) = send(&rt, &keys, "GET", FIELD_SITES, &member_token, None, None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "field overview is work_order_read_all-gated: {denied}"
    );
    assert_eq!(denied["error"]["code"], "forbidden");
    let (status, denied) = send(
        &rt,
        &keys,
        "GET",
        &format!("{FIELD_SITES}/{site_a}"),
        &member_token,
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "in-scope detail is 403 for MEMBER (out-of-scope stays 404): {denied}"
    );
    let (status, denied) = send(
        &rt,
        &keys,
        "POST",
        &format!("{TICKETS}/{ticket_id}/link"),
        &member_token,
        Some(json!({"site_id": site_a})),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "link is AssigneeManage-gated: {denied}"
    );
    assert_eq!(denied["error"]["code"], "forbidden");
    let (status, denied) = send(
        &rt,
        &keys,
        "POST",
        &format!("{TICKETS}/{ticket_id}/acceptance"),
        &member_token,
        Some(json!({"kind": "CUSTOMER_ACCEPTED", "channel": "IN_PERSON", "accepted_by": "김담당"})),
        Some("field-acceptance-key-0009"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "acceptance is AssigneeManage-gated: {denied}"
    );

    // A missing Idempotency-Key on acceptance is a validation error.
    let (status, missing) = send_without_idempotency(
        &rt,
        &keys,
        &format!("{TICKETS}/{ticket_id}/acceptance"),
        &token_a,
        json!({"kind": "CUSTOMER_ACCEPTED", "channel": "IN_PERSON", "accepted_by": "김담당"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "acceptance requires Idempotency-Key: {missing}"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn field_console_isolates_tenants_as_runtime_role(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let knl = OrgId::knl();
    let branch_knl = seed_branch(&pool, knl, "field-knl").await;
    let customer_knl = seed_customer(&pool, knl, branch_knl, "KNL 거래처").await;
    let site_knl = seed_site(&pool, knl, branch_knl, customer_knl, "KNL 현장").await;

    let other = OrgId::from_uuid(Uuid::new_v4());
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, 'other-tenant', 'Other Tenant')",
    )
    .bind(*other.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let branch_other = seed_branch(&pool, other, "field-other").await;
    let admin_other = seed_user(&pool, other, "ADMIN", branch_other).await;
    let token_other = keys.token(admin_other, other, "ADMIN", vec![branch_other]);

    // RLS as console_rt: the sibling tenant's site is invisible in list and detail
    // even though the JWT branch scope alone would not exclude it.
    let (status, page) = send(&rt, &keys, "GET", FIELD_SITES, &token_other, None, None).await;
    assert_eq!(status, StatusCode::OK, "cross-tenant list: {page}");
    assert_eq!(
        page["total"], 0,
        "sibling tenant rows must not be counted: {page}"
    );
    let (status, hidden) = send(
        &rt,
        &keys,
        "GET",
        &format!("{FIELD_SITES}/{site_knl}"),
        &token_other,
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "sibling tenant detail is 404: {hidden}"
    );
    assert!(
        !hidden.to_string().contains("KNL 현장"),
        "denial must not leak the site: {hidden}"
    );
}

struct Keys {
    private_pem: String,
    public_pem: String,
}
impl Keys {
    fn generate() -> Self {
        let key = SigningKey::random(&mut OsRng);
        Self {
            private_pem: key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
            public_pem: key
                .verifying_key()
                .to_public_key_pem(LineEnding::LF)
                .unwrap(),
        }
    }
    fn token(&self, user: UserId, org: OrgId, role: &str, branches: Vec<BranchId>) -> String {
        JwtIssuer::from_es256_pem(
            JwtSettings {
                issuer: ISSUER.into(),
                audience: AUDIENCE.into(),
                access_token_ttl: Duration::minutes(15),
            },
            self.private_pem.as_bytes(),
            self.public_pem.as_bytes(),
        )
        .unwrap()
        .issue_access_token(AccessTokenInput {
            subject: user,
            org_id: org,
            roles: vec![role.into()],
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
        })
        .unwrap()
    }
}

async fn runtime_role_pool(owner: &PgPool) -> PgPool {
    PgPoolOptions::new()
        .max_connections(8)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(owner.connect_options().as_ref().clone())
        .await
        .unwrap()
}

async fn send(
    pool: &PgPool,
    keys: &Keys,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
    idempotency_key: Option<&str>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            "Idempotency-Key",
            idempotency_key.unwrap_or("not-used-by-this-request"),
        )
        .body(
            body.map(|v| Body::from(serde_json::to_vec(&v).unwrap()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap();
    dispatch(pool, keys, request).await
}

/// Same as [`send`] but with the `Idempotency-Key` header genuinely absent,
/// for the header-required contract.
async fn send_without_idempotency(
    pool: &PgPool,
    keys: &Keys,
    uri: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    dispatch(pool, keys, request).await
}

async fn dispatch(pool: &PgPool, keys: &Keys, request: Request<Body>) -> (StatusCode, Value) {
    let response = build_router(app_state(pool.clone(), keys.public_pem.clone()).unwrap())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}

fn app_state(pool: PgPool, public_key: String) -> Result<AppState, console_app::AppError> {
    AppState::new(
        AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
            ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".into()),
            ("CONSOLE_JWT_ISSUER", ISSUER.into()),
            ("CONSOLE_JWT_AUDIENCE", AUDIENCE.into()),
            ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key),
        ])?,
        DatabaseDependency::Postgres(pool),
    )
}

async fn seed_branch(pool: &PgPool, org: OrgId, name: &str) -> BranchId {
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("region-{name}"))
            .bind(*org.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    BranchId::from_uuid(
        sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(region)
        .bind(name)
        .bind(*org.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}

async fn seed_user(pool: &PgPool, org: OrgId, role: &str, branch: BranchId) -> UserId {
    let user = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, is_active, org_id) VALUES ($1, $2, $3, true, $4)").bind(*user.as_uuid()).bind(format!("field-{role}-{user}")).bind(vec![role]).bind(*org.as_uuid()).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user.as_uuid())
        .bind(*branch.as_uuid())
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    user
}

async fn seed_customer(pool: &PgPool, org: OrgId, branch: BranchId, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(name)
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_site(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    customer: Uuid,
    name: &str,
) -> Uuid {
    sqlx::query_scalar("INSERT INTO registry_sites (branch_id, customer_id, name, org_id, address, contact_name, contact_phone) VALUES ($1, $2, $3, $4, '경기도 화성시 1번지', '김담당', '010-0000-0000') RETURNING id").bind(*branch.as_uuid()).bind(customer).bind(name).bind(*org.as_uuid()).fetch_one(pool).await.unwrap()
}

async fn seed_equipment(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    customer: Uuid,
    site: Uuid,
) -> Uuid {
    sqlx::query_scalar("INSERT INTO registry_equipment (branch_id, customer_id, site_id, equipment_no, manufacturer_code, kind_code, power_code, status, specification, ton_text, source_sheet, source_row, org_id) VALUES ($1, $2, $3, $4, 'MF', 'FL', 'EL', '임대', '2.0t 전동', '2.0', 'field-visit-test', 1, $5) RETURNING id")
        .bind(*branch.as_uuid()).bind(customer).bind(site).bind(format!("FLT{}-{:04}", "AB", 1)).bind(*org.as_uuid()).fetch_one(pool).await.unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn seed_work_order(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    customer: Uuid,
    site: Uuid,
    equipment: Uuid,
    requested_by: UserId,
    request_no: &str,
) -> Uuid {
    sqlx::query_scalar("INSERT INTO work_orders (request_no, branch_id, equipment_id, customer_id, site_id, requested_by, status, symptom, org_id) VALUES ($7, $1, $2, $3, $4, $5, 'IN_PROGRESS', '리프트 체인 소음', $6) RETURNING id")
        .bind(*branch.as_uuid()).bind(equipment).bind(customer).bind(site).bind(*requested_by.as_uuid()).bind(*org.as_uuid()).bind(request_no).fetch_one(pool).await.unwrap()
}

async fn seed_attendance(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    user: UserId,
    work_order: Uuid,
    site: Uuid,
) {
    sqlx::query("INSERT INTO site_attendance_events (org_id, user_id, branch_id, work_order_id, site_id, kind, occurred_at) VALUES ($1, $2, $3, $4, $5, 'ARRIVAL', now())")
        .bind(*org.as_uuid()).bind(*user.as_uuid()).bind(*branch.as_uuid()).bind(work_order).bind(site).execute(pool).await.unwrap();
}
