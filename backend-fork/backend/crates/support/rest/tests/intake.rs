//! REST-level tests for the unauthenticated customer intake endpoint:
//! rate-limit 429 past cap, and a successful 202 acknowledgement.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use console_kernel_core::{AuditAction, AuditEvent, OrgId, TraceContext, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_db::{DbError, with_audit};
use console_platform_test_support::runtime_role_pool;
use console_support_adapter_postgres::{MAX_BODY_CHARS, PgSupportStore};
use console_support_rest::{SupportRestState, router};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::Value;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

async fn build(owner_pool: &PgPool) -> Router {
    build_with_public_intake_org(owner_pool, OrgId::knl()).await
}

async fn build_with_public_intake_org(owner_pool: &PgPool, public_intake_org: OrgId) -> Router {
    // No JWT verifier and no push notifier: the intake endpoint is
    // unauthenticated, and notifications degrade gracefully. Exercise the app
    // with the production-like runtime role so FORCE RLS (not a BYPASSRLS test
    // owner) governs tenant writes/reads at the REST surface.
    console_platform_request_context::with_trusted_client_ip(
        router(
            SupportRestState::new(
                PgSupportStore::new(runtime_role_pool(owner_pool).await),
                None,
                None,
            )
            .with_storefront_org(public_intake_org),
        ),
        1,
        vec!["10.0.0.0/8".parse().unwrap()],
    )
}

fn intake_request(ip: &str) -> Request<Body> {
    let body = serde_json::json!({
        "category": "COMPLAINT",
        "priority": "MEDIUM",
        "title": "Late delivery",
        "body": "The forklift never arrived",
        "requester_name": "Customer",
        "requester_contact": "010-0000-0000"
    })
    .to_string();
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/v1/support/intake")
        .header("content-type", "application/json")
        .header("x-forwarded-for", ip)
        .body(Body::from(body))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "10.0.0.3:443".parse::<std::net::SocketAddr>().unwrap(),
    ));
    request
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn intake_writes_under_configured_public_org(pool: PgPool) {
    let public_org = OrgId::new();
    seed_org(&pool, public_org).await;
    let app = build_with_public_intake_org(&pool, public_org).await;

    let response = app
        .clone()
        .oneshot(intake_request("203.0.113.50"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let persisted_org: uuid::Uuid = sqlx::query_scalar(
        "SELECT org_id FROM support_tickets WHERE origin = 'CUSTOMER' AND title = $1",
    )
    .bind("Late delivery")
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(persisted_org, *public_org.as_uuid());
    assert_ne!(persisted_org, *OrgId::knl().as_uuid());
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn intake_written_to_configured_org_is_visible_only_to_same_org_staff(pool: PgPool) {
    let public_org = OrgId::new();
    let other_org = OrgId::new();
    seed_org(&pool, public_org).await;
    seed_org(&pool, other_org).await;
    let same_org_staff = seed_staff_user(&pool, public_org, "Configured support staff").await;
    let other_org_staff = seed_staff_user(&pool, other_org, "Other org support staff").await;

    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let verifier =
        JwtVerifier::from_es256_public_pem(jwt_settings(), public_key_pem.as_bytes()).unwrap();
    let same_org_token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        same_org_staff,
        public_org,
    );
    let other_org_token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        other_org_staff,
        other_org,
    );

    // Exercise the app with the production-like runtime role so FORCE RLS, not a
    // BYPASSRLS test owner, proves tenant visibility at the REST surface.
    let rt_pool = runtime_role_pool(&pool).await;
    let app = router(
        SupportRestState::new(PgSupportStore::new(rt_pool), Some(verifier), None)
            .with_storefront_org(public_org),
    );

    let response = app
        .clone()
        .oneshot(intake_request("203.0.113.51"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let same_org = get_json(
        app.clone(),
        "/api/v1/support/tickets?include_untriaged=true",
        &same_org_token,
    )
    .await;
    assert_eq!(same_org.0, StatusCode::OK, "{:?}", same_org.1);
    assert_eq!(same_org.1["total"].as_i64(), Some(1), "{:?}", same_org.1);
    let same_org_items = same_org.1["items"]
        .as_array()
        .expect("paginated items array");
    assert_eq!(same_org_items.len(), 1, "{same_org_items:?}");
    assert_eq!(
        same_org_items[0]["title"],
        Value::String("Late delivery".to_owned())
    );
    let ticket_id = same_org_items[0]["id"]
        .as_str()
        .expect("ticket id from same-org list");

    let same_org_detail = get_json(
        app.clone(),
        &format!("/api/v1/support/tickets/{ticket_id}"),
        &same_org_token,
    )
    .await;
    assert_eq!(same_org_detail.0, StatusCode::OK, "{:?}", same_org_detail.1);
    assert_eq!(
        same_org_detail.1["ticket"]["title"],
        Value::String("Late delivery".to_owned())
    );

    let other_org_list = get_json(
        app.clone(),
        "/api/v1/support/tickets?include_untriaged=true",
        &other_org_token,
    )
    .await;
    assert_eq!(other_org_list.0, StatusCode::OK, "{:?}", other_org_list.1);
    assert_eq!(
        other_org_list.1["total"].as_i64(),
        Some(0),
        "{:?}",
        other_org_list.1
    );
    let other_org_items = other_org_list.1["items"]
        .as_array()
        .expect("paginated items array");
    assert!(
        other_org_items.is_empty(),
        "other-org staff must not see configured-org intake: {other_org_items:?}"
    );

    let other_org_detail = get_json(
        app,
        &format!("/api/v1/support/tickets/{ticket_id}"),
        &other_org_token,
    )
    .await;
    assert_eq!(
        other_org_detail.0,
        StatusCode::NOT_FOUND,
        "{:?}",
        other_org_detail.1
    );
}

/// Real-clock smoke for the intake rate-limit wiring: a handful of under-cap
/// requests each succeed (202) and a different IP keeps an independent bucket.
///
/// The cap/reset boundary is asserted deterministically with a synthetic clock
/// in `console_support_rest`'s `rate_limit_trips_at_cap_and_resets_after_window`
/// unit test. Driving real HTTP round-trips past the cap here raced the wall
/// clock's minute boundary — a burst that straddles a minute lands the last
/// request in a fresh fixed window and resets the bucket before the cap trips —
/// which was the CI flake. This mirrors auth-rest's
/// `otp_redeem_rate_limit_wires_up_on_real_clock_path` split.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn intake_succeeds_and_rate_limit_wires_up_on_real_clock_path(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let app = build(&pool).await;
        let ip = "203.0.113.42";

        // Well under the per-IP cap (5/min): every request is a normal 202,
        // proving `OffsetDateTime::now_utc()` wires into `rate_limit` end to end.
        for i in 0..3 {
            let response = app.clone().oneshot(intake_request(ip)).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::ACCEPTED,
                "request {i} should be accepted"
            );
        }

        // A different IP is independent and still accepted.
        let other = app
            .clone()
            .oneshot(intake_request("198.51.100.1"))
            .await
            .unwrap();
        assert_eq!(other.status(), StatusCode::ACCEPTED);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn intake_rejects_missing_fields_generically(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let app = build(&pool).await;
        let body = serde_json::json!({
            "category": "OTHER",
            "priority": "LOW",
            "title": "   ",
            "body": "x",
            "requester_name": "Cust",
            "requester_contact": "c@example.com"
        })
        .to_string();
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/support/intake")
            .header("content-type", "application/json")
            .header("x-forwarded-for", "203.0.113.99")
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn intake_rejects_over_length_fields_generically(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let app = build(&pool).await;
        // One scalar past the store-side body bound: must be rejected at the edge
        // with a generic 400, before any persistence.
        let body = serde_json::json!({
            "category": "OTHER",
            "priority": "LOW",
            "title": "Late delivery",
            "body": "x".repeat(MAX_BODY_CHARS + 1),
            "requester_name": "Cust",
            "requester_contact": "c@example.com"
        })
        .to_string();
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/support/intake")
            .header("content-type", "application/json")
            .header("x-forwarded-for", "203.0.113.77")
            .body(Body::from(body))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    })
    .await;
}

async fn seed_org(pool: &PgPool, org: OrgId) {
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_org").unwrap(),
        "organization",
        org.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org);
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
            )
            .bind(*org.as_uuid())
            .bind(format!("support-{}", &org.to_string()[..8]))
            .bind("Configured Support Intake Org")
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
}

async fn seed_staff_user(pool: &PgPool, org: OrgId, display_name: &str) -> UserId {
    let user = UserId::new();
    let display_name = display_name.to_owned();
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_user").unwrap(),
        "user",
        user.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org);
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO users (id, display_name, roles, org_id, is_active) VALUES ($1, $2, $3, $4, true)",
            )
            .bind(*user.as_uuid())
            .bind(display_name)
            .bind(Vec::from(["SUPER_ADMIN".to_owned()]))
            .bind(*org.as_uuid())
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
    user
}

fn jwt_settings() -> JwtSettings {
    JwtSettings {
        issuer: TEST_ISSUER.to_owned(),
        audience: TEST_AUDIENCE.to_owned(),
        access_token_ttl: Duration::minutes(15),
    }
}

fn issue_token(
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    org_id: OrgId,
) -> String {
    let issuer =
        JwtIssuer::from_es256_pem(jwt_settings(), private_key_pem, public_key_pem).unwrap();
    issuer
        .issue_access_token(AccessTokenInput {
            subject: user_id,
            org_id,
            roles: vec!["SUPER_ADMIN".to_owned()],
            branches: Vec::new(),
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

async fn get_json(service: Router, uri: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    (status, json)
}
