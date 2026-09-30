#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Self-service attendance reads, including the Console self-service projections.
//!
//! Drives the REAL router on a genuine non-owner `console_rt` pool (RLS actually
//! enforced, never a BYPASSRLS superuser). Locks the contract that this endpoint
//! is *self-scoped, not role-gated*:
//!   * an authenticated user with no linked employee — an ADMIN/system account —
//!     reads an EMPTY page (200), never a 403. This is the ConsoleShell
//!     regression (#196 co-mounts /attendance on every console load).
//!   * a user with a linked employee reads ONLY their own records, and self-read
//!     never widens to another employee's records.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{OrgId, UserId};
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

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const ME_PATH: &str = "/api/v1/hr/attendance-records/me";
const MY_EXCEPTIONS_PATH: &str = "/api/v1/attendance/me/exceptions";
const MY_WEEK52_PATH: &str = "/api/v1/attendance/me/week52";

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

// ===========================================================================
// An ADMIN with no linked employee reads an empty page, not a 403 — even when
// OTHER employees' attendance records exist in the same org (no leak).
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn admin_without_employee_link_reads_empty_self_attendance(pool: PgPool) {
    let keys = keys();

    // A linked MEMBER punches in, so the org has a real attendance record.
    let member = UserId::new();
    let member_employee = seed_linked_employee(&pool, member, "MEMBER", "member-emp").await;
    let admin = UserId::new();
    seed_user(&pool, admin, "ADMIN").await; // no employee link

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    // Seed the member's record through the real write path (it also writes the
    // payroll material ref the read view joins on).
    let created = post(
        service.clone(),
        ME_PATH,
        &bearer(&keys, member, "MEMBER"),
        json!({ "kind": "CLOCK_IN", "idempotency_key": "member-clock-in-1" }),
    )
    .await;
    assert_eq!(created.status, StatusCode::OK, "{:?}", created.json);
    let _ = member_employee;

    // ADMIN self-read: 200 with an empty page, NOT 403, and NOT the member's row.
    let admin_read = get(service, ME_PATH, &bearer(&keys, admin, "ADMIN")).await;
    assert_eq!(
        admin_read.status,
        StatusCode::OK,
        "ADMIN self-attendance read must not be forbidden: {:?}",
        admin_read.json
    );
    assert_eq!(admin_read.json["total"], 0);
    assert_eq!(
        admin_read.json["items"].as_array().map(Vec::len),
        Some(0),
        "ADMIN self-read must not leak another employee's records: {:?}",
        admin_read.json
    );
}

// ===========================================================================
// A linked non-admin reads ONLY their own record (self-scoped, never widened).
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn linked_member_reads_only_own_attendance(pool: PgPool) {
    let keys = keys();

    let alice = UserId::new();
    seed_linked_employee(&pool, alice, "MEMBER", "alice").await;
    let bob = UserId::new();
    seed_linked_employee(&pool, bob, "MEMBER", "bob").await;

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    // Both punch in.
    for (user, key) in [(alice, "alice-in"), (bob, "bob-in")] {
        let created = post(
            service.clone(),
            ME_PATH,
            &bearer(&keys, user, "MEMBER"),
            json!({ "kind": "CLOCK_IN", "idempotency_key": key }),
        )
        .await;
        assert_eq!(created.status, StatusCode::OK, "{:?}", created.json);
    }

    // Alice sees exactly one record — her own, never bob's.
    let alice_read = get(service, ME_PATH, &bearer(&keys, alice, "MEMBER")).await;
    assert_eq!(alice_read.status, StatusCode::OK, "{:?}", alice_read.json);
    assert_eq!(alice_read.json["total"], 1);
    let items = alice_read.json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["employee_display_name"], "alice");
}

// The attendance-console self-service surface is a separate, signed-principal
// read boundary. It deliberately does not require a manager feature grant.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn linked_member_reads_only_own_attendance_console_data(pool: PgPool) {
    let keys = keys();
    let alice = UserId::new();
    let alice_employee = seed_linked_employee(&pool, alice, "MEMBER", "alice-console").await;
    let bob = UserId::new();
    let bob_employee = seed_linked_employee(&pool, bob, "MEMBER", "bob-console").await;
    let unlinked = UserId::new();
    seed_user(&pool, unlinked, "MEMBER").await;
    seed_exception(&pool, alice, alice_employee, "alice-console-exception").await;
    seed_exception(&pool, bob, bob_employee, "bob-console-exception").await;

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let alice_token = bearer(&keys, alice, "MEMBER");
    let unlinked_token = bearer(&keys, unlinked, "MEMBER");

    let own = get(
        service.clone(),
        &format!("{MY_EXCEPTIONS_PATH}?work_date=2026-07-20"),
        &alice_token,
    )
    .await;
    assert_eq!(own.status, StatusCode::OK, "{:?}", own.json);
    assert_eq!(own.json["total"], 1);
    let item = &own.json["items"][0];
    assert_eq!(item["code"], "alice-console-exception");
    for forbidden in ["employee_id", "employee_name", "team", "branch_id", "links"] {
        assert!(item.get(forbidden).is_none(), "self DTO leaked {forbidden}");
    }

    let empty = get(
        service.clone(),
        &format!("{MY_EXCEPTIONS_PATH}?work_date=2026-07-20"),
        &unlinked_token,
    )
    .await;
    assert_eq!(empty.status, StatusCode::OK, "{:?}", empty.json);
    assert_eq!(empty.json["total"], 0);
    assert_eq!(empty.json["items"], json!([]));

    let empty_week = get(
        service.clone(),
        &format!("{MY_WEEK52_PATH}?week_start=2026-07-20"),
        &unlinked_token,
    )
    .await;
    assert_eq!(empty_week.status, StatusCode::OK, "{:?}", empty_week.json);
    assert_eq!(empty_week.json, json!({ "status": "not_available" }));
    assert!(
        empty_week.json.get("projection").is_none(),
        "unlinked principals must not receive a projection field"
    );

    let own_week = get(
        service.clone(),
        &format!("{MY_WEEK52_PATH}?week_start=2026-07-20"),
        &alice_token,
    )
    .await;
    assert_eq!(own_week.status, StatusCode::OK, "{:?}", own_week.json);
    assert_eq!(
        own_week.json,
        json!({
            "status": "available",
            "projection": {
                "week_start": "2026-07-20",
                "current_hours": 0.0,
                "projected_hours": 0.0,
                "tone": "OK"
            }
        })
    );

    for selector in ["employee_id", "branch_id"] {
        let rejected = get(
            service.clone(),
            &format!("{MY_EXCEPTIONS_PATH}?work_date=2026-07-20&{selector}={alice_employee}"),
            &alice_token,
        )
        .await;
        assert_eq!(
            rejected.status,
            StatusCode::BAD_REQUEST,
            "{:?}",
            rejected.json
        );
        assert_eq!(rejected.json["error"]["code"], "invalid_query");
    }

    let non_monday = get(
        service.clone(),
        &format!("{MY_WEEK52_PATH}?week_start=2026-07-21"),
        &alice_token,
    )
    .await;
    assert_eq!(non_monday.status, StatusCode::UNPROCESSABLE_ENTITY);
    let method_not_allowed = send(
        service,
        "POST",
        &format!("{MY_WEEK52_PATH}?week_start=2026-07-20"),
        &alice_token,
        None,
    )
    .await;
    assert_eq!(method_not_allowed.status, StatusCode::METHOD_NOT_ALLOWED);
}

// ===========================================================================
// Helpers (mirror workflow_runtime_instance_api.rs).
// ===========================================================================

async fn seed_user(pool: &PgPool, user_id: UserId, role: &str) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("self-read-{role}-{}", user_id.as_uuid()))
        .bind(vec![role])
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

/// Insert an employee and link `user_id` to it, so `GET /me` resolves a record.
async fn seed_linked_employee(pool: &PgPool, user_id: UserId, role: &str, name: &str) -> Uuid {
    let employee_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO employees (
            id, org_id, company, name, employee_number, source_filename,
            source_sheet, source_row, source_key, raw_row, source_metadata
        )
        VALUES ($1, $2, '테스트', $3, NULL, 'employees.xlsx', '직원', 2, $4, '{}', '{}')
        "#,
    )
    .bind(employee_id)
    .bind(*OrgId::knl().as_uuid())
    .bind(name)
    .bind(format!("employee-row-{name}"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, org_id, employee_id) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(*user_id.as_uuid())
    .bind(name)
    .bind(vec![role])
    .bind(*OrgId::knl().as_uuid())
    .bind(employee_id)
    .execute(pool)
    .await
    .unwrap();
    employee_id
}

async fn seed_exception(pool: &PgPool, actor: UserId, employee_id: Uuid, code: &str) {
    sqlx::query(
        "INSERT INTO attendance_exceptions (org_id, code, kind, employee_id, work_date, occurred_at, detail, idempotency_key, request_fingerprint, created_by) VALUES ($1, $2, 'LATE', $3, DATE '2026-07-20', TIMESTAMPTZ '2026-07-20 09:00:00+00', 'late', $4, $5, $6)",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(code)
    .bind(employee_id)
    .bind(format!("{code}-idempotency-key"))
    .bind("a".repeat(64))
    .bind(*actor.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn post(service: axum::Router, uri: &str, token: &str, body: Value) -> JsonResponse {
    send(service, "POST", uri, token, Some(body)).await
}

async fn get(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    send(service, "GET", uri, token, None).await
}

async fn send(
    service: axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> JsonResponse {
    let mut builder = Request::builder()
        .uri(uri)
        .method(method)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let request = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        builder.body(Body::from(body.to_string())).unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
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

fn bearer(keys: &Keys, user_id: UserId, role: &str) -> String {
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
            roles: vec![role.to_owned()],
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
