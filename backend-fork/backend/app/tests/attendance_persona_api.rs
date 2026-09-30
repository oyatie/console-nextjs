#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Attendance REST persona contract for payroll-vertical handoff (PR-5 REST).
//!
//! CONFIRM is `AttendanceExceptionManage` `[D,D,D,A,D,A]` — MEMBER Deny,
//! EXECUTIVE Deny, ADMIN Allow. Production looks up the in-org exception first,
//! then denies the feature, so MEMBER CONFIRM is 403 (not 404-omit). A
//! branch-scoped ADMIN CONFIRM on another branch is 404 (`not_found`, same as
//! absent) and writes no resolution. Other-org CONFIRM is omit (404), not
//! 403-with-existence. Month close is `PeriodLockManage` `[D,D,D,A,A,A]` —
//! ADMIN and EXECUTIVE Allow, MEMBER Deny. Drives the assembled router on
//! `console_rt`.

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

const ISSUER: &str = "console-platform-auth";
const AUDIENCE: &str = "console-api";
const EXCEPTIONS: &str = "/api/v1/attendance/exceptions";
const CLOSES: &str = "/api/v1/attendance/closes";
const FINGERPRINT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct Response {
    status: StatusCode,
    json: Value,
}

struct Personas {
    branch: Uuid,
    admin: UserId,
    admin_token: String,
    executive_token: String,
    member_token: String,
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn executive_cannot_confirm_exception_admin_can(pool: PgPool) {
    let keys = keys();
    let personas = seed_personas(&pool, &keys).await;
    let exception_id = seed_open_late_exception(&pool, personas.branch, personas.admin).await;
    let other_branch = seed_named_branch(&pool, "attendance-persona-other-branch").await;
    let other_exception_id = seed_open_late_exception(&pool, other_branch, personas.admin).await;
    let other_org = OrgId::from_uuid(Uuid::new_v4());
    seed_org(&pool, other_org).await;
    let foreign_branch =
        seed_named_branch_in(&pool, other_org, "attendance-persona-foreign-branch").await;
    let foreign_actor = seed_user_in(&pool, other_org, "ADMIN", Some(foreign_branch)).await;
    let foreign_exception_id =
        seed_open_late_exception_in(&pool, other_org, foreign_branch, foreign_actor).await;
    let app =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let resolve = format!("{EXCEPTIONS}/{exception_id}/resolve");
    let other_resolve = format!("{EXCEPTIONS}/{other_exception_id}/resolve");
    let foreign_resolve = format!("{EXCEPTIONS}/{foreign_exception_id}/resolve");
    let confirm = json!({"action": "CONFIRM", "reason": "verified arrival"});

    let seen = get(
        app.clone(),
        &format!("{EXCEPTIONS}/{exception_id}"),
        &personas.executive_token,
    )
    .await;
    assert_eq!(seen.status, StatusCode::OK, "{:?}", seen.json);
    assert_eq!(seen.json["status"], "OPEN");

    let executive_denied = post(
        app.clone(),
        &resolve,
        &personas.executive_token,
        confirm.clone(),
    )
    .await;
    assert_forbidden(&executive_denied);
    assert_eq!(open_resolutions(&pool, exception_id).await, 0);
    let still_open = get(
        app.clone(),
        &format!("{EXCEPTIONS}/{exception_id}"),
        &personas.admin_token,
    )
    .await;
    assert_eq!(still_open.status, StatusCode::OK, "{:?}", still_open.json);
    assert_eq!(still_open.json["status"], "OPEN");
    assert!(still_open.json.get("resolution").is_none());

    let member_denied = post(
        app.clone(),
        &resolve,
        &personas.member_token,
        confirm.clone(),
    )
    .await;
    // In-org MEMBER CONFIRM is feature Deny after the row is visible: 403, not
    // 404-omit. Production 403 JSON must not carry employee/payroll figures.
    assert_forbidden(&member_denied);
    assert_eq!(open_resolutions(&pool, exception_id).await, 0);

    let other_denied = post(
        app.clone(),
        &other_resolve,
        &personas.admin_token,
        confirm.clone(),
    )
    .await;
    // Out-of-scope resource ids are 404, not 403, so existence is not leaked.
    assert_not_found(&other_denied);
    assert_eq!(open_resolutions(&pool, other_exception_id).await, 0);
    assert_eq!(open_resolutions(&pool, exception_id).await, 0);

    let foreign_admin = post(
        app.clone(),
        &foreign_resolve,
        &personas.admin_token,
        confirm.clone(),
    )
    .await;
    let foreign_member = post(
        app.clone(),
        &foreign_resolve,
        &personas.member_token,
        confirm.clone(),
    )
    .await;
    // Other-org CONFIRM is omit (404), including for MEMBER — not 403-with-existence.
    assert_not_found(&foreign_admin);
    assert_omits_exception(&foreign_admin, foreign_exception_id);
    assert_not_found(&foreign_member);
    assert_omits_exception(&foreign_member, foreign_exception_id);
    assert_eq!(open_resolutions(&pool, foreign_exception_id).await, 0);
    assert_eq!(open_resolutions(&pool, exception_id).await, 0);

    let admin_ok = post(app, &resolve, &personas.admin_token, confirm).await;
    assert_eq!(admin_ok.status, StatusCode::OK, "{:?}", admin_ok.json);
    assert_eq!(admin_ok.json["status"], "RESOLVED");
    assert_eq!(admin_ok.json["resolution"]["action"], "CONFIRM");
    assert_eq!(open_resolutions(&pool, exception_id).await, 1);
    assert_eq!(open_resolutions(&pool, other_exception_id).await, 0);
    assert_eq!(open_resolutions(&pool, foreign_exception_id).await, 0);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn period_lock_month_close_allows_admin_and_executive_forbids_member(pool: PgPool) {
    let keys = keys();
    let personas = seed_personas(&pool, &keys).await;
    let app =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let month = "2026-08";
    let admin_body = json!({
        "month": month,
        "branch_scope": personas.branch,
        "attest": true,
    });
    let member_denied = post(
        app.clone(),
        CLOSES,
        &personas.member_token,
        admin_body.clone(),
    )
    .await;
    assert_forbidden(&member_denied);
    assert_eq!(close_count(&pool, month).await, 0);

    let admin_ok = post(app.clone(), CLOSES, &personas.admin_token, admin_body).await;
    assert_eq!(admin_ok.status, StatusCode::CREATED, "{:?}", admin_ok.json);
    assert_eq!(admin_ok.json["month"], "2026-08-01");
    assert_eq!(
        admin_ok.json["branch_scope"],
        json!(personas.branch.to_string())
    );
    assert!(admin_ok.json.get("period_lock_id").is_none());

    let executive_ok = post(
        app,
        CLOSES,
        &personas.executive_token,
        json!({"month": month, "attest": true}),
    )
    .await;
    assert_eq!(
        executive_ok.status,
        StatusCode::CREATED,
        "{:?}",
        executive_ok.json
    );
    assert_eq!(executive_ok.json["month"], "2026-08-01");
    assert_eq!(executive_ok.json["branch_scope"], "org");
    assert!(
        executive_ok.json["period_lock_id"].as_str().is_some(),
        "org-wide close must attach the payroll period lock: {:?}",
        executive_ok.json
    );
    assert_eq!(close_count(&pool, month).await, 2);
}

fn assert_forbidden(response: &Response) {
    assert_eq!(
        response.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        response.json
    );
    assert_eq!(response.json["error"]["code"], "forbidden");
    assert_no_employee_or_payroll_figures(response);
}

fn assert_not_found(response: &Response) {
    assert_eq!(
        response.status,
        StatusCode::NOT_FOUND,
        "{:?}",
        response.json
    );
    assert_eq!(response.json["error"]["code"], "not_found");
    assert_no_employee_or_payroll_figures(response);
}

fn assert_omits_exception(response: &Response, exception_id: Uuid) {
    let body = response.json.to_string();
    assert!(
        !body.contains(&exception_id.to_string()),
        "omit body leaked exception id: {body}"
    );
    assert_no_employee_or_payroll_figures(response);
}

fn assert_no_employee_or_payroll_figures(response: &Response) {
    let body = response.json.to_string();
    let lower = body.to_ascii_lowercase();
    for needle in [
        "employee_id",
        "employeeid",
        "employee_name",
        "employeename",
        "employee_number",
        "persona-employee",
        "ot_hours",
        "othours",
        "base_pay",
        "basepay",
        "payroll",
        "period_lock",
        "worker_rate",
    ] {
        assert!(
            !lower.contains(needle),
            "error body leaked {needle}: {body}"
        );
    }
}

async fn seed_personas(pool: &PgPool, keys: &Keys) -> Personas {
    let branch = seed_branch(pool).await;
    let admin = seed_user(pool, "ADMIN", Some(branch)).await;
    let executive = seed_user(pool, "EXECUTIVE", None).await;
    let member = seed_user(pool, "MEMBER", Some(branch)).await;
    Personas {
        branch,
        admin,
        admin_token: bearer(keys, admin, "ADMIN"),
        executive_token: bearer(keys, executive, "EXECUTIVE"),
        member_token: bearer(keys, member, "MEMBER"),
    }
}

async fn seed_branch(pool: &PgPool) -> Uuid {
    seed_named_branch(pool, "attendance-persona-branch").await
}

async fn seed_org(pool: &PgPool, org: OrgId) {
    let id = *org.as_uuid();
    let slug = format!("att-xorg-{}", &id.to_string()[..8]);
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(slug)
        .bind("Attendance persona other org")
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_named_branch(pool: &PgPool, name: &str) -> Uuid {
    seed_named_branch_in(pool, OrgId::knl(), name).await
}

async fn seed_named_branch_in(pool: &PgPool, org: OrgId, name: &str) -> Uuid {
    let org = *org.as_uuid();
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("{name}-region"))
            .bind(org)
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region)
    .bind(name)
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_user(pool: &PgPool, role: &str, branch: Option<Uuid>) -> UserId {
    seed_user_in(pool, OrgId::knl(), role, branch).await
}

async fn seed_user_in(pool: &PgPool, org: OrgId, role: &str, branch: Option<Uuid>) -> UserId {
    let user = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user.as_uuid())
        .bind(format!("attendance-persona-{role}-{}", user.as_uuid()))
        .bind(vec![role])
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    if let Some(branch) = branch {
        sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
            .bind(*user.as_uuid())
            .bind(branch)
            .bind(*org.as_uuid())
            .execute(pool)
            .await
            .unwrap();
    }
    user
}

async fn seed_open_late_exception(pool: &PgPool, branch: Uuid, actor: UserId) -> Uuid {
    seed_open_late_exception_in(pool, OrgId::knl(), branch, actor).await
}

async fn seed_open_late_exception_in(
    pool: &PgPool,
    org: OrgId,
    branch: Uuid,
    actor: UserId,
) -> Uuid {
    let employee = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO employees (
            id, org_id, company, name, employee_number, source_filename,
            source_sheet, source_row, source_key, raw_row, source_metadata
        )
        VALUES ($1, $2, '테스트', 'persona-employee', NULL, 'employees.xlsx', '직원', 2, $3, '{}', '{}')
        "#,
    )
    .bind(employee)
    .bind(*org.as_uuid())
    .bind(format!("employee-row-{employee}"))
    .execute(pool)
    .await
    .unwrap();
    let exception_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO attendance_exceptions \
         (id, org_id, code, kind, employee_id, branch_id, work_date, detail, created_by, idempotency_key, request_fingerprint) \
         VALUES ($1, $2, $3, 'LATE', $4, $5, DATE '2026-07-20', 'late arrival', $6, $7, $8)",
    )
    .bind(exception_id)
    .bind(*org.as_uuid())
    .bind(format!("AT-{exception_id}"))
    .bind(employee)
    .bind(branch)
    .bind(*actor.as_uuid())
    .bind(format!("persona-exception-{exception_id}"))
    .bind(FINGERPRINT)
    .execute(pool)
    .await
    .unwrap();
    exception_id
}

async fn open_resolutions(pool: &PgPool, exception_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM attendance_exception_resolutions WHERE exception_id = $1",
    )
    .bind(exception_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn close_count(pool: &PgPool, month: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM attendance_month_closes WHERE month = ($1 || '-01')::date",
    )
    .bind(month)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn get(app: axum::Router, uri: &str, token: &str) -> Response {
    send(app, "GET", uri, token, None).await
}

async fn post(app: axum::Router, uri: &str, token: &str, body: Value) -> Response {
    send(app, "POST", uri, token, Some(body)).await
}

async fn send(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> Response {
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
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let json = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
        .unwrap_or_else(|_| json!({}));
    Response { status, json }
}

fn keys() -> Keys {
    let key = SigningKey::random(&mut OsRng);
    Keys {
        private_pem: key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
        public_pem: key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap(),
    }
}

fn bearer(keys: &Keys, user: UserId, role: &str) -> String {
    JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: ISSUER.into(),
            audience: AUDIENCE.into(),
            access_token_ttl: Duration::minutes(15),
        },
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
    )
    .unwrap()
    .issue_access_token(AccessTokenInput {
        subject: user,
        org_id: OrgId::knl(),
        roles: vec![role.into()],
        branches: vec![],
        platform: false,
        view_as: false,
        read_only: false,
        display_name: None,
        feature_grants: vec![],
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
        .after_connect(|conn, _| {
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
        ("CONSOLE_JWT_ISSUER", ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key_pem),
    ])?;
    AppState::new(config, DatabaseDependency::Postgres(pool))
}
