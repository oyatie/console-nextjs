#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Manager attendance branch-scope contract. This file needs its generated
//! Buck target before it can run independently.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_inbox_adapter_postgres::PgInboxStore;
use console_kernel_core::{OrgId, TraceContext, UserId};
use console_leave_adapter_postgres::PgLeaveStore;
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const ISSUER: &str = "console-platform-auth";
const AUDIENCE: &str = "console-api";
const SUMMARY: &str = "/api/v1/hr/attendance-summary";
const RECORDS: &str = "/api/v1/hr/attendance-records";

struct Keys {
    private_pem: String,
    public_pem: String,
}
struct Response {
    status: StatusCode,
    json: Value,
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn manager_attendance_branch_scope_is_explicit_and_nonleaking(pool: PgPool) {
    let keys = keys();
    let branch_a = seed_branch(&pool, "a").await;
    let branch_b = seed_branch(&pool, "b").await;
    let admin_a = seed_user(&pool, "ADMIN", Some(branch_a)).await;
    let custom_a = seed_user(&pool, "MEMBER", Some(branch_a)).await;
    let executive = seed_user(&pool, "EXECUTIVE", None).await;
    let super_admin = seed_user(&pool, "SUPER_ADMIN", None).await;
    seed_custom_directory_reader(&pool, custom_a).await;
    let employee_b = seed_employee_attendance(&pool, super_admin, branch_b).await;
    seed_site_attendance(&pool, admin_a, branch_a, "901").await;
    seed_site_attendance(&pool, executive, branch_b, "902").await;
    let app =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let admin = bearer(&keys, admin_a, "ADMIN");
    let custom = bearer(&keys, custom_a, "MEMBER");
    let executive = bearer(&keys, executive, "EXECUTIVE");
    let super_admin = bearer(&keys, super_admin, "SUPER_ADMIN");
    let a = format!("?branch_id={branch_a}");
    let b = format!("?branch_id={branch_b}");

    // 1-2: branch ADMIN is concrete-only, for both manager surfaces.
    assert_forbidden(get(app.clone(), &format!("{SUMMARY}{b}"), &admin).await);
    assert_forbidden(get(app.clone(), SUMMARY, &admin).await);
    assert_forbidden(get(app.clone(), &format!("{RECORDS}{b}"), &admin).await);
    assert_forbidden(get(app.clone(), RECORDS, &admin).await);

    // 3-4: a concrete branch returns only its own durable site-attendance
    // event; the branch-B event is not merely hidden by an empty fixture.
    let branch_a_summary = get(app.clone(), &format!("{SUMMARY}{a}"), &admin).await;
    assert_ok(&branch_a_summary);
    assert_eq!(branch_a_summary.json["total"], 1);
    assert_eq!(
        branch_a_summary.json["items"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(
        branch_a_summary.json["items"][0]["user_id"],
        json!(*admin_a.as_uuid())
    );
    assert_ok(&get(app.clone(), &format!("{RECORDS}{a}"), &admin).await);

    // 5-6: built-in organization-wide personas may deliberately omit branch.
    let organization_summary = get(app.clone(), SUMMARY, &executive).await;
    assert_ok(&organization_summary);
    assert_eq!(organization_summary.json["total"], 2);
    assert_eq!(
        organization_summary.json["items"].as_array().map(Vec::len),
        Some(2)
    );
    assert_ok(&get(app.clone(), RECORDS, &super_admin).await);

    // 7-8: a custom grant follows the same concrete-only boundary on both
    // manager surfaces; it does not become organization-wide by omission.
    assert_ok(&get(app.clone(), &format!("{SUMMARY}{a}"), &custom).await);
    assert_forbidden(get(app.clone(), SUMMARY, &custom).await);
    assert_ok(&get(app.clone(), &format!("{RECORDS}{a}"), &custom).await);
    assert_forbidden(get(app, RECORDS, &custom).await);

    // The scoped employee lookup must not disclose whether an out-of-branch
    // employee exists: it is byte-for-byte the same empty page as an absent id.
    let app = build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem).unwrap());
    let out_of_branch = get(
        app.clone(),
        &format!("{RECORDS}{a}&employee_id={employee_b}"),
        &admin,
    )
    .await;
    let absent = get(
        app,
        &format!("{RECORDS}{a}&employee_id={}", Uuid::new_v4()),
        &admin,
    )
    .await;
    assert_eq!(
        out_of_branch.status,
        StatusCode::OK,
        "{:?}",
        out_of_branch.json
    );
    assert_eq!(absent.status, StatusCode::OK, "{:?}", absent.json);
    assert_eq!(out_of_branch.json, absent.json);
}

fn assert_ok(response: &Response) {
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.json);
}
fn assert_forbidden(response: Response) {
    assert_eq!(
        response.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        response.json
    );
}

async fn seed_branch(pool: &PgPool, name: &str) -> Uuid {
    let org = *OrgId::knl().as_uuid();
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("attendance-scope-{name}"))
            .bind(org)
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region)
    .bind(format!("attendance-scope-{name}"))
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_user(pool: &PgPool, role: &str, branch: Option<Uuid>) -> UserId {
    let user = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user.as_uuid())
        .bind(format!("attendance-{role}-{}", user.as_uuid()))
        .bind(vec![role])
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    if let Some(branch) = branch {
        sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
            .bind(*user.as_uuid())
            .bind(branch)
            .bind(*OrgId::knl().as_uuid())
            .execute(pool)
            .await
            .unwrap();
    }
    user
}

async fn seed_custom_directory_reader(pool: &PgPool, user: UserId) {
    let org = *OrgId::knl().as_uuid();
    let role: Uuid = sqlx::query_scalar("INSERT INTO policy_roles (org_id, role_key, display_name, status) VALUES ($1, 'attendance_reader', 'Attendance reader', 'ACTIVE') RETURNING id")
        .bind(org).fetch_one(pool).await.unwrap();
    sqlx::query("INSERT INTO policy_role_permissions (org_id, role_id, feature_key, permission_level) VALUES ($1, $2, 'employee_directory_read', 'allow')")
        .bind(org).bind(role).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO user_role_assignments (org_id, user_id, role_id) VALUES ($1, $2, $3)")
        .bind(org)
        .bind(*user.as_uuid())
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_employee_attendance(pool: &PgPool, actor: UserId, branch: Uuid) -> Uuid {
    let org = OrgId::knl();
    let employee = Uuid::new_v4();
    let expected_updated_at: OffsetDateTime = sqlx::query_scalar(
        "INSERT INTO employees (id, org_id, company, name, source_filename, source_sheet, source_row, source_key, raw_row, source_metadata) VALUES ($1, $2, 'test', 'out-of-branch', 'test.xlsx', 'employees', 1, $3, '{}', '{}') RETURNING updated_at",
    )
    .bind(employee)
    .bind(*org.as_uuid())
    .bind(format!("attendance-scope-{employee}"))
    .fetch_one(pool)
    .await
    .unwrap();

    let runtime_pool = runtime_role_pool(pool).await;
    let leave_store = PgLeaveStore::new(
        runtime_pool.clone(),
        Arc::new(PgInboxStore::new(runtime_pool)),
    )
    .with_leave_command_pool(leave_command_role_pool(pool).await);
    let update = console_platform_request_context::scope_org(org, async move {
        leave_store
            .set_employee_home_branch(
                employee,
                branch,
                expected_updated_at,
                actor,
                TraceContext::generate(),
            )
            .await
    })
    .await
    .unwrap();
    assert_eq!(update.employee_id, employee);
    assert_eq!(update.home_branch_id, branch);

    let record = Uuid::new_v4();
    sqlx::query("INSERT INTO employee_attendance_records (id, org_id, employee_id, actor_user_id, kind, state_after, idempotency_key) VALUES ($1, $2, $3, $4, 'CLOCK_IN', 'CLOCKED_IN', $5)")
        .bind(record)
        .bind(*org.as_uuid())
        .bind(employee).bind(*actor.as_uuid()).bind(format!("attendance-scope-{record}")).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO payroll_attendance_material_refs (org_id, attendance_record_id, employee_id, work_date, source_digest) VALUES ($1, $2, $3, CURRENT_DATE, $4)")
        .bind(*org.as_uuid())
        .bind(record)
        .bind(employee).bind("a".repeat(64)).execute(pool).await.unwrap();
    employee
}

async fn seed_site_attendance(pool: &PgPool, user: UserId, branch: Uuid, request_suffix: &str) {
    let org = *OrgId::knl().as_uuid();
    let customer: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(branch)
    .bind(format!("attendance-summary-customer-{request_suffix}"))
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap();
    let site: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(branch)
    .bind(customer)
    .bind(format!("attendance-summary-site-{request_suffix}"))
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap();
    let equipment: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_equipment (branch_id, customer_id, site_id, equipment_no, management_no, manufacturer_code, kind_code, power_code, status, specification, ton_text, model, source_sheet, source_row, org_id) VALUES ($1, $2, $3, $4, $5, 'A', 'B', 'C', '임대', 'test', '2.5', 'test', 'test', 1, $6) RETURNING id",
    )
    .bind(branch)
    .bind(customer)
    .bind(site)
    .bind(format!("ATS12-0{request_suffix}"))
    .bind(format!("attendance-summary-{request_suffix}"))
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap();
    let work_order = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO work_orders (id, request_no, branch_id, equipment_id, customer_id, site_id, requested_by, status, priority, symptom, org_id) VALUES ($1, $2, $3, $4, $5, $6, $7, 'RECEIVED', 'UNSET', 'attendance summary fixture', $8)",
    )
    .bind(work_order)
    .bind(format!("20260724-{request_suffix}"))
    .bind(branch)
    .bind(equipment)
    .bind(customer)
    .bind(site)
    .bind(*user.as_uuid())
    .bind(org)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO site_attendance_events (org_id, user_id, branch_id, work_order_id, site_id, kind, occurred_at) VALUES ($1, $2, $3, $4, $5, 'ARRIVAL', now())",
    )
    .bind(org)
    .bind(*user.as_uuid())
    .bind(branch)
    .bind(work_order)
    .bind(site)
    .execute(pool)
    .await
    .unwrap();
}

async fn get(app: axum::Router, uri: &str, token: &str) -> Response {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
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
    scoped_role_pool(owner_pool, "console_rt").await
}

async fn leave_command_role_pool(owner_pool: &PgPool) -> PgPool {
    scoped_role_pool(owner_pool, "console_leave_cmd").await
}

async fn scoped_role_pool(owner_pool: &PgPool, role: &'static str) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _| {
            Box::pin(async move {
                match role {
                    "console_rt" => sqlx::query("SET ROLE console_rt").execute(conn).await?,
                    "console_leave_cmd" => {
                        sqlx::query("SET ROLE console_leave_cmd")
                            .execute(conn)
                            .await?
                    }
                    _ => unreachable!("test role is fixed by its helper"),
                };
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
