#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{HeaderMap, Request, StatusCode, header};
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

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn consent_routes_audit_transitions_and_report_status(pool: PgPool) {
    let fixture = TestFixture::new(&pool, "Consent API", "Consent Branch").await;
    let service = build_router(app_state(pool.clone(), fixture.public_key_pem.clone()).unwrap());
    let body = json!({ "branch_id": fixture.branch_id });

    let grant = post_json(
        service.clone(),
        "/api/v1/location-consent/grant",
        &fixture.mechanic_token,
        body.clone(),
    )
    .await;
    assert_eq!(grant.status, StatusCode::OK, "{:?}", grant.json);
    assert_eq!(grant.json["state"], "GRANTED");
    assert_eq!(grant.json["may_collect"], true);

    let suspend = post_json(
        service.clone(),
        "/api/v1/location-consent/suspend",
        &fixture.mechanic_token,
        body.clone(),
    )
    .await;
    assert_eq!(suspend.status, StatusCode::OK, "{:?}", suspend.json);
    assert_eq!(suspend.json["state"], "SUSPENDED");
    assert_eq!(suspend.json["may_collect"], false);

    let resume = post_json(
        service.clone(),
        "/api/v1/location-consent/resume",
        &fixture.mechanic_token,
        body.clone(),
    )
    .await;
    assert_eq!(resume.status, StatusCode::OK, "{:?}", resume.json);
    assert_eq!(resume.json["state"], "GRANTED");

    let withdraw = post_json(
        service.clone(),
        "/api/v1/location-consent/withdraw",
        &fixture.mechanic_token,
        body,
    )
    .await;
    assert_eq!(withdraw.status, StatusCode::OK, "{:?}", withdraw.json);
    assert_eq!(withdraw.json["state"], "WITHDRAWN");
    assert_eq!(withdraw.json["may_collect"], false);

    let status = get_json(
        service,
        &format!(
            "/api/v1/location-consent/status?branch_id={}",
            fixture.branch_id.as_uuid()
        ),
        &fixture.mechanic_token,
    )
    .await;
    assert_eq!(status.status, StatusCode::OK, "{:?}", status.json);
    assert_eq!(status.json["state"], "WITHDRAWN");

    for action in [
        "consent.grant",
        "consent.suspend",
        "consent.resume",
        "consent.withdraw",
    ] {
        assert_eq!(audit_count(&pool, action).await, 1, "{action}");
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn ping_ingestion_rejects_without_granted_consent(pool: PgPool) {
    let fixture = TestFixture::new(&pool, "Consent Ping", "No Consent Branch").await;
    let service = build_router(app_state(pool.clone(), fixture.public_key_pem.clone()).unwrap());

    let ping = post_json(
        service,
        "/api/v1/location-pings",
        &fixture.mechanic_token,
        json!({
            "branch_id": fixture.branch_id,
            "latitude": 37.5665,
            "longitude": 126.9780,
            "accuracy_m": 12.5,
            "recorded_at": OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
            "on_duty": true
        }),
    )
    .await;

    assert_eq!(ping.status, StatusCode::FORBIDDEN, "{:?}", ping.json);
    assert_eq!(ping.json["error"]["code"], "forbidden");
    assert_eq!(location_ping_count(&pool).await, 0);
    assert_eq!(location_collection_log_count(&pool).await, 0);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn withdrawal_route_destroys_location_pings_and_logs(pool: PgPool) {
    let fixture = TestFixture::new(&pool, "Consent Destroy", "Destroy Branch").await;
    let service = build_router(app_state(pool.clone(), fixture.public_key_pem.clone()).unwrap());
    let transition_body = json!({ "branch_id": fixture.branch_id });

    let grant = post_json(
        service.clone(),
        "/api/v1/location-consent/grant",
        &fixture.mechanic_token,
        transition_body.clone(),
    )
    .await;
    assert_eq!(grant.status, StatusCode::OK, "{:?}", grant.json);

    let ping = post_json(
        service.clone(),
        "/api/v1/location-pings",
        &fixture.mechanic_token,
        json!({
            "branch_id": fixture.branch_id,
            "latitude": 37.5665,
            "longitude": 126.9780,
            "accuracy_m": 12.5,
            "recorded_at": OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
            "on_duty": true
        }),
    )
    .await;
    assert_eq!(ping.status, StatusCode::NO_CONTENT, "{:?}", ping.json);
    assert_eq!(location_ping_count(&pool).await, 1);
    assert_eq!(location_collection_log_count(&pool).await, 1);

    let withdraw = post_json(
        service,
        "/api/v1/location-consent/withdraw",
        &fixture.mechanic_token,
        transition_body,
    )
    .await;
    assert_eq!(withdraw.status, StatusCode::OK, "{:?}", withdraw.json);
    assert_eq!(location_ping_count(&pool).await, 0);
    assert_eq!(location_collection_log_count(&pool).await, 0);
    assert_eq!(coordinate_audit_count(&pool).await, 0);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn admin_can_read_and_export_consent_ledger(pool: PgPool) {
    let fixture = TestFixture::new(&pool, "Consent Ledger", "Ledger Branch").await;
    let service = build_router(app_state(pool.clone(), fixture.public_key_pem.clone()).unwrap());

    let grant = post_json(
        service.clone(),
        "/api/v1/location-consent/grant",
        &fixture.mechanic_token,
        json!({ "branch_id": fixture.branch_id }),
    )
    .await;
    assert_eq!(grant.status, StatusCode::OK, "{:?}", grant.json);

    let ledger = get_json(
        service.clone(),
        &format!(
            "/api/v1/location-consents/ledger?branch_id={}",
            fixture.branch_id.as_uuid()
        ),
        &fixture.admin_token,
    )
    .await;
    assert_eq!(ledger.status, StatusCode::OK, "{:?}", ledger.json);
    assert_eq!(ledger.json["items"][0]["action"], "consent.grant");
    assert_eq!(ledger.json["total"], 1);

    let csv = get(
        service,
        &format!(
            "/api/v1/location-consents/ledger.csv?branch_id={}",
            fixture.branch_id.as_uuid()
        ),
        &fixture.admin_token,
    )
    .await;
    assert_eq!(csv.status, StatusCode::OK, "{}", csv.body);
    assert!(
        csv.headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/csv"))
    );
    assert!(csv.body.contains("consent.grant"));
    assert!(!csv.body.contains("37.5665"));
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn ceo_covert_audit_stream_requires_clearance_and_audits_successful_read(pool: PgPool) {
    let fixture = TestFixture::new(&pool, "CEO Audit", "CEO Audit Branch").await;
    let sensitive_event_id = seed_ceo_covert_audit_event(&pool, fixture.admin_id).await;
    grant_ceo_covert_clearance(&pool, fixture.admin_id).await;
    seed_subject_freshness(&pool, fixture.admin_id).await;
    let service = build_router(app_state(pool.clone(), fixture.public_key_pem.clone()).unwrap());

    let denied = get(
        service.clone(),
        "/api/v1/audit-streams/ceo-covert/events",
        &fixture.mechanic_token,
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{}", denied.body);
    assert_eq!(audit_count(&pool, "audit_stream.ceo_read").await, 0);

    let allowed = get_json(
        service,
        "/api/v1/audit-streams/ceo-covert/events?limit=10&offset=0",
        &fixture.admin_token,
    )
    .await;
    assert_eq!(allowed.status, StatusCode::OK, "{:?}", allowed.json);
    assert_eq!(allowed.json["stream_key"], "ceo_covert_audit");
    assert_eq!(allowed.json["read_kind"], "events");
    assert_eq!(allowed.json["total"], 1);
    assert_eq!(
        allowed.json["items"][0]["id"],
        sensitive_event_id.to_string()
    );
    assert_eq!(allowed.json["items"][0]["sensitivity"], "CEO_COVERT");
    assert_eq!(allowed.json["access_audit_id"].as_str().unwrap().len(), 36);
    assert_eq!(audit_count(&pool, "audit_stream.ceo_read").await, 1);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn ceo_covert_access_log_read_is_clearance_gated_and_audited(pool: PgPool) {
    let fixture = TestFixture::new(&pool, "CEO Access", "CEO Access Branch").await;
    seed_ceo_covert_audit_event(&pool, fixture.admin_id).await;
    grant_ceo_covert_clearance(&pool, fixture.admin_id).await;
    seed_subject_freshness(&pool, fixture.admin_id).await;
    let service = build_router(app_state(pool.clone(), fixture.public_key_pem.clone()).unwrap());

    let first_read = get_json(
        service.clone(),
        "/api/v1/audit-streams/ceo-covert/events",
        &fixture.admin_token,
    )
    .await;
    assert_eq!(first_read.status, StatusCode::OK, "{:?}", first_read.json);
    assert_eq!(audit_count(&pool, "audit_stream.ceo_read").await, 1);

    let denied = get(
        service.clone(),
        "/api/v1/audit-streams/ceo-covert/access-events",
        &fixture.mechanic_token,
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{}", denied.body);
    assert_eq!(audit_count(&pool, "audit_stream.access_log_read").await, 0);

    let access_log = get_json(
        service,
        "/api/v1/audit-streams/ceo-covert/access-events?limit=10&offset=0",
        &fixture.admin_token,
    )
    .await;
    assert_eq!(access_log.status, StatusCode::OK, "{:?}", access_log.json);
    assert_eq!(access_log.json["stream_key"], "ceo_covert_audit");
    assert_eq!(access_log.json["read_kind"], "access_events");
    assert_eq!(access_log.json["total"], 1);
    assert_eq!(
        access_log.json["items"][0]["action"],
        "audit_stream.ceo_read"
    );
    assert_eq!(access_log.json["items"][0]["target_id"], "ceo_covert_audit");
    assert_eq!(audit_count(&pool, "audit_stream.access_log_read").await, 1);
}

struct TestFixture {
    branch_id: BranchId,
    admin_id: UserId,
    public_key_pem: String,
    mechanic_token: String,
    admin_token: String,
}

impl TestFixture {
    async fn new(pool: &PgPool, region: &str, branch: &str) -> Self {
        let signing_key = SigningKey::random(&mut OsRng);
        let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_key_pem = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let branch_id = seed_branch(pool, region, branch).await;
        let mechanic = UserId::new();
        let admin = UserId::new();
        seed_user_with_branch(pool, mechanic, "MECHANIC", branch_id).await;
        seed_user_with_branch(pool, admin, "ADMIN", branch_id).await;
        let mechanic_token = issue_token(
            private_pem.as_bytes(),
            public_key_pem.as_bytes(),
            mechanic,
            vec!["MECHANIC".to_owned()],
            vec![branch_id],
        )
        .unwrap();
        let admin_token = issue_token(
            private_pem.as_bytes(),
            public_key_pem.as_bytes(),
            admin,
            vec!["ADMIN".to_owned()],
            vec![branch_id],
        )
        .unwrap();

        Self {
            branch_id,
            admin_id: admin,
            public_key_pem,
            mechanic_token,
            admin_token,
        }
    }
}

struct HttpResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
    json: Value,
}

async fn post_json(service: axum::Router, uri: &str, token: &str, body: Value) -> HttpResponse {
    let request = Request::builder()
        .uri(uri)
        .method("POST")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    send(service, request).await
}

async fn get_json(service: axum::Router, uri: &str, token: &str) -> HttpResponse {
    get(service, uri, token).await
}

async fn get(service: axum::Router, uri: &str, token: &str) -> HttpResponse {
    let request = Request::builder()
        .uri(uri)
        .method("GET")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    send(service, request).await
}

async fn send(service: axum::Router, request: Request<Body>) -> HttpResponse {
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    let json = serde_json::from_str(&body).unwrap_or_else(|_| json!({}));
    HttpResponse {
        status,
        headers,
        body,
        json,
    }
}

fn issue_token(
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

    Ok(issuer.issue_access_token(AccessTokenInput {
        subject: user_id,
        org_id: OrgId::knl(),
        roles,
        branches,
        platform: false,
        view_as: false,
        read_only: false,
        display_name: None,
        feature_grants: Vec::new(),
        // Snapshot the DB-current freshness baseline (policy/subject/session all
        // default to 1 on first insert). The CEO-covert audit stream is a
        // Cedar-only, server-loaded guard: a zero-stamped token trips the
        // boundary's `MissingSubjectFreshness` precondition before clearance is
        // ever consulted (mirrors cedar_parity_shadow / cedar_freshness_mint).
        authz_subject_version: 1,
        authz_policy_version: 1,
        session_generation: 1,
        issued_at: OffsetDateTime::now_utc(),
    })?)
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
        .bind(format!("Compliance API {role}"))
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

async fn seed_ceo_covert_audit_event(pool: &PgPool, actor: UserId) -> uuid::Uuid {
    let event_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(OrgId::knl().as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor, action, target_type, target_id, branch_id,
            before_snap, after_snap, trace_id, span_id, occurred_at, org_id
        ) VALUES (
            $1, $2, 'executive.case_review', 'executive_case', $3, NULL,
            NULL, $4, 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'bbbbbbbbbbbbbbbb', now(), $5
        )
        "#,
    )
    .bind(event_id)
    .bind(*actor.as_uuid())
    .bind(event_id.to_string())
    .bind(json!({ "classification": "CEO_COVERT", "decision": "approved" }))
    .bind(*OrgId::knl().as_uuid())
    .execute(tx.as_mut())
    .await
    .unwrap();

    sqlx::query(
        r#"
        INSERT INTO audit_stream_event_labels (org_id, audit_event_id, stream_key, sensitivity)
        VALUES ($1, $2, 'ceo_covert_audit', 'CEO_COVERT')
        "#,
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(event_id)
    .execute(tx.as_mut())
    .await
    .unwrap();
    tx.commit().await.unwrap();

    event_id
}

/// Establish the DB-current freshness material the Cedar-only audit-stream guard
/// requires: a per-org `policy_versions` row and a per-(org,user)
/// `subject_authz_versions` row (both default to version/session 1). Without
/// this the server-loaded requirement carries no material and the boundary
/// denies `MissingSubjectFreshness` before clearance is evaluated. Mirrors
/// `seed_freshness` in cedar_parity_shadow / cedar_freshness_mint.
async fn seed_subject_freshness(pool: &PgPool, user_id: UserId) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(OrgId::knl().as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    sqlx::query("INSERT INTO policy_versions (org_id) VALUES ($1) ON CONFLICT (org_id) DO NOTHING")
        .bind(*OrgId::knl().as_uuid())
        .execute(tx.as_mut())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO subject_authz_versions (org_id, user_id) VALUES ($1, $2) \
         ON CONFLICT (org_id, user_id) DO NOTHING",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(*user_id.as_uuid())
    .execute(tx.as_mut())
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

async fn grant_ceo_covert_clearance(pool: &PgPool, user_id: UserId) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(OrgId::knl().as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO clearance_assignments (
            org_id, user_id, clearance_key, stream_key, status, granted_by, grant_reason
        ) VALUES (
            $1, $2, 'audit.ceo_covert.read', 'ceo_covert_audit', 'ACTIVE', $2, 'integration test clearance'
        )
        "#,
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(*user_id.as_uuid())
    .execute(tx.as_mut())
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

async fn audit_count(pool: &PgPool, action: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = $1")
        .bind(action)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn location_ping_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM location_pings")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn location_collection_log_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM location_collection_logs")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn coordinate_audit_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM audit_events
        WHERE before_snap::text LIKE '%37.5665%'
           OR after_snap::text LIKE '%37.5665%'
           OR before_snap::text LIKE '%126.978%'
           OR after_snap::text LIKE '%126.978%'
        "#,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}
