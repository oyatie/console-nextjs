#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! MEMBER Login-only ESS HTTP contract on the assembled router.
//!
//! A MEMBER JWT with empty `feature_grants` (no `PayrollRunRead`, no
//! `AttendanceExceptionManage`) must still read self-service surfaces, and
//! must be forbidden on org-wide payroll run list:
//!   * `GET /api/v1/me/inbox-docs?filter=payslip` → 200 (empty or own docs),
//!     never 403 — ESS vault, not payroll REST `/payslips/me`;
//!   * `GET /api/v1/payroll/payslips/me` → 200, never 403 — self-scoped
//!     readiness (hours / `*_source_present`), never a won amount, never
//!     `PayrollRunRead`; unlinked MEMBER is an empty page;
//!   * `GET /api/v1/attendance/me/exceptions` → 200, never 403 — self-scoped,
//!     not manager `AttendanceExceptionManage`;
//!   * `GET /api/v1/attendance/me/week52` → 200, never 403 — self-scoped;
//!     unlinked MEMBER is `status=not_available` with `projection` omitted;
//!   * `GET /api/v1/payroll/runs` → 403 — `authorize_org_wide(PayrollRunRead)`.
//!
//! Status lock only: does not emit a payslip (see inbox-rest filter=payslip
//! coverage) and does not seed exceptions. Drives `console_rt` (RLS on).

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_authz::Feature;
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const INBOX_PAYSLIP_PATH: &str = "/api/v1/me/inbox-docs?filter=payslip";
const MY_PAYSLIPS_PATH: &str = "/api/v1/payroll/payslips/me";
// Date selector is list validation (422 without it), not an authz grant.
const MY_EXCEPTIONS_PATH: &str = "/api/v1/attendance/me/exceptions?work_date=2026-07-20";
// Monday — week52 start validation, not an authz grant.
const MY_WEEK52_PATH: &str = "/api/v1/attendance/me/week52?week_start=2026-07-20";
const PAYROLL_RUNS_PATH: &str = "/api/v1/payroll/runs";

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn member_login_only_ess_is_200_and_payroll_runs_are_403(pool: PgPool) {
    let keys = keys();
    let member = UserId::new();
    seed_user(&pool, member).await;

    let token = bearer(&keys, member);
    let claims = JwtVerifier::from_es256_public_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        keys.public_pem.as_bytes(),
    )
    .unwrap()
    .verify_access_token(&token)
    .expect("MEMBER JWT");
    assert_eq!(claims.roles, ["MEMBER"]);
    assert!(
        claims.feature_grants.is_empty(),
        "Login-only MEMBER JWT must carry empty feature_grants (no {}, no {}): {:?}",
        Feature::PayrollRunRead.as_str(),
        Feature::AttendanceExceptionManage.as_str(),
        claims.feature_grants
    );
    for forbidden in [
        Feature::PayrollRunRead.as_str(),
        Feature::AttendanceExceptionManage.as_str(),
    ] {
        assert!(
            !claims.feature_grants.iter().any(|grant| grant == forbidden),
            "MEMBER ESS JWT must not carry {forbidden}: {:?}",
            claims.feature_grants
        );
    }

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    let inbox = get(service.clone(), INBOX_PAYSLIP_PATH, &token).await;
    assert_eq!(
        inbox.status,
        StatusCode::OK,
        "MEMBER Login-only inbox payslip filter must be 200, not 403: {:?}",
        inbox.json
    );
    let inbox_items = inbox.json["items"].as_array().expect("inbox items");
    assert!(
        inbox_items.iter().all(|item| item["kind"] == "payslip"),
        "filter=payslip must be empty or own payslips: {:?}",
        inbox.json
    );

    let exceptions = get(service.clone(), MY_EXCEPTIONS_PATH, &token).await;
    assert_eq!(
        exceptions.status,
        StatusCode::OK,
        "MEMBER Login-only own exceptions must be 200, not 403: {:?}",
        exceptions.json
    );

    let payslips = get(service.clone(), MY_PAYSLIPS_PATH, &token).await;
    assert_eq!(
        payslips.status,
        StatusCode::OK,
        "MEMBER Login-only own payslip readiness must be 200, not 403: {:?}",
        payslips.json
    );
    let payslip_items = payslips.json["items"]
        .as_array()
        .expect("payslips/me items");
    for item in payslip_items {
        let won = keys_containing_won(item);
        assert!(
            won.is_empty(),
            "payslips/me items are readiness (hours / *_source_present), never won: {won:?} in {item}"
        );
    }

    let week52 = get(service.clone(), MY_WEEK52_PATH, &token).await;
    assert_eq!(
        week52.status,
        StatusCode::OK,
        "MEMBER Login-only own week52 must be 200, not 403: {:?}",
        week52.json
    );
    assert_eq!(
        week52.json["status"], "not_available",
        "unlinked MEMBER week52 must be not_available: {:?}",
        week52.json
    );
    assert!(
        week52.json.get("projection").is_none(),
        "unlinked MEMBER week52 must omit projection (deny-by-omission): {:?}",
        week52.json
    );

    let runs = get(service, PAYROLL_RUNS_PATH, &token).await;
    assert_eq!(
        runs.status,
        StatusCode::FORBIDDEN,
        "MEMBER Login-only must not list payroll runs: {:?}",
        runs.json
    );
}

async fn seed_user(pool: &PgPool, user_id: UserId) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("persona-ess-member-{}", user_id.as_uuid()))
        .bind(vec!["MEMBER"])
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

async fn get(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    let request = Request::builder()
        .uri(uri)
        .method("GET")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
}

fn keys_containing_won(value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    collect_keys_containing_won(value, &mut found);
    found
}

fn collect_keys_containing_won(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                if key.to_ascii_lowercase().contains("won") {
                    found.push(key.clone());
                }
                collect_keys_containing_won(nested, found);
            }
        }
        Value::Array(items) => {
            for nested in items {
                collect_keys_containing_won(nested, found);
            }
        }
        _ => {}
    }
}

fn keys() -> Keys {
    let signing_key = SigningKey::random(&mut OsRng);
    Keys {
        private_pem: signing_key
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap()
            .to_string(),
        public_pem: signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap(),
    }
}

fn bearer(keys: &Keys, user_id: UserId) -> String {
    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
    )
    .unwrap();
    issuer
        .issue_access_token(AccessTokenInput {
            subject: user_id,
            org_id: OrgId::knl(),
            roles: vec!["MEMBER".to_owned()],
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
