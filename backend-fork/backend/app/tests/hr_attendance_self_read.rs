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
        &bearer(&pool, &keys, member, "MEMBER").await,
        json!({ "kind": "CLOCK_IN", "idempotency_key": "member-clock-in-1" }),
    )
    .await;
    assert_eq!(created.status, StatusCode::OK, "{:?}", created.json);
    let _ = member_employee;

    // ADMIN self-read: 200 with an empty page, NOT 403, and NOT the member's row.
    let admin_read = get(
        service,
        ME_PATH,
        &bearer(&pool, &keys, admin, "ADMIN").await,
    )
    .await;
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
            &bearer(&pool, &keys, user, "MEMBER").await,
            json!({ "kind": "CLOCK_IN", "idempotency_key": key }),
        )
        .await;
        assert_eq!(created.status, StatusCode::OK, "{:?}", created.json);
    }

    // Alice sees exactly one record — her own, never bob's.
    let alice_read = get(
        service,
        ME_PATH,
        &bearer(&pool, &keys, alice, "MEMBER").await,
    )
    .await;
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
    seed_exception(
        &pool,
        alice,
        alice_employee,
        "alice-console-exception",
        None,
    )
    .await;
    seed_exception(&pool, bob, bob_employee, "bob-console-exception", None).await;

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let alice_token = bearer(&pool, &keys, alice, "MEMBER").await;
    let unlinked_token = bearer(&pool, &keys, unlinked, "MEMBER").await;

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

// These are real HTTP owner reads; no frontend/browser acceptance is implied.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn former_member_retains_exact_own_history_after_exit_unknown_and_site_closure(pool: PgPool) {
    let fixture = history_fixture(&pool).await;
    let keys = keys();
    let token = bearer(&pool, &keys, fixture.worker, "MEMBER").await;
    let service = build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem).unwrap());
    let before = read_history(service.clone(), &token).await;
    assert_eq!(before[0]["total"], 1);
    assert_eq!(
        before[0]["items"][0]["id"],
        fixture.exception_id.to_string()
    );
    assert_eq!(before[1]["projection"]["current_hours"], 8.0);
    assert_eq!(before[2]["total"], 2);
    assert_eq!(before[2]["items"].as_array().unwrap().len(), 2);
    for record in before[2]["items"].as_array().unwrap() {
        assert_eq!(record["employee_id"], fixture.employee.to_string());
    }
    for status in ["EXITED", "UNKNOWN"] {
        sqlx::query("UPDATE employees SET employment_status=$1 WHERE id=$2")
            .bind(status)
            .bind(fixture.employee)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            read_history(service.clone(), &token).await,
            before,
            "{status} must preserve the same owned retained history"
        );
    }
    sqlx::query("UPDATE branches SET deactivated_at=now() WHERE id=$1")
        .bind(fixture.branch)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(read_history(service.clone(), &token).await, before);
    for path in [MY_EXCEPTIONS_PATH, MY_WEEK52_PATH] {
        let date = if path == MY_EXCEPTIONS_PATH {
            "work_date"
        } else {
            "week_start"
        };
        for selector in ["employee_id", "branch_id"] {
            let response = get(
                service.clone(),
                &format!("{path}?{date}=2026-07-20&{selector}={}", fixture.employee),
                &token,
            )
            .await;
            assert_eq!(
                response.status,
                StatusCode::BAD_REQUEST,
                "{:?}",
                response.json
            );
            assert_eq!(response.json["error"]["code"], "invalid_query");
        }
        let response = send(
            service.clone(),
            "POST",
            &format!("{path}?{date}=2026-07-20"),
            &token,
            None,
        )
        .await;
        assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    }
    let past_page = get(
        service.clone(),
        &format!("{MY_EXCEPTIONS_PATH}?work_date=2026-07-20&limit=1&offset=1"),
        &token,
    )
    .await;
    assert_eq!(past_page.status, StatusCode::OK);
    assert_eq!(past_page.json["total"], 1);
    assert_eq!(past_page.json["items"], json!([]));
    let filtered = get(
        service,
        &format!("{MY_EXCEPTIONS_PATH}?work_date=2026-07-20&status=RESOLVED"),
        &token,
    )
    .await;
    assert_eq!(filtered.status, StatusCode::OK);
    assert_eq!(filtered.json["total"], 0);
    assert_eq!(filtered.json["items"], json!([]));
    for object in [&before[0]["items"][0], &before[1]["projection"]] {
        for field in [
            "employee_id",
            "employee_name",
            "name",
            "team",
            "branch_id",
            "links",
        ] {
            assert!(
                object.get(field).is_none(),
                "self projection exposed {field}"
            );
        }
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn retained_self_reads_respect_unlink_account_and_session_family_revocation(pool: PgPool) {
    let fixture = history_fixture(&pool).await;
    let keys = keys();
    let token = bearer(&pool, &keys, fixture.worker, "MEMBER").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let before = read_history(service.clone(), &token).await;
    sqlx::query("UPDATE users SET employee_id=NULL WHERE id=$1")
        .bind(*fixture.worker.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let empty = read_history(service.clone(), &token).await;
    assert_eq!(empty[0]["total"], 0);
    assert_eq!(empty[0]["items"], json!([]));
    assert_eq!(empty[1], json!({"status":"not_available"}));
    assert_eq!(empty[2]["total"], 0);
    assert_eq!(empty[2]["items"], json!([]));
    let records: i64 =
        sqlx::query_scalar("SELECT count(*) FROM employee_attendance_records WHERE employee_id=$1")
            .bind(fixture.employee)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(records, 2);
    let exceptions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM attendance_exceptions WHERE employee_id=$1")
            .bind(fixture.employee)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(exceptions, 1);
    sqlx::query("UPDATE users SET employee_id=$1 WHERE id=$2")
        .bind(fixture.employee)
        .bind(*fixture.worker.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(read_history(service.clone(), &token).await, before);
    let revoked = sqlx::query("UPDATE auth_refresh_token_families SET revoked_at=now() WHERE user_id=$1 AND revoked_at IS NULL")
        .bind(*fixture.worker.as_uuid()).execute(&pool).await.unwrap();
    assert_eq!(revoked.rows_affected(), 1);
    assert_history_unauthorized(service.clone(), &token).await;
    let fresh = bearer(&pool, &keys, fixture.worker, "MEMBER").await;
    assert_eq!(read_history(service.clone(), &fresh).await, before);
    // Current Account revocation also rejects a newly minted, unrevoked family.
    sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
        .bind(*fixture.worker.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_history_unauthorized(service, &fresh).await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn history_reads_preserve_existing_ack_subject_and_member_resolution_controls(pool: PgPool) {
    let fixture = history_fixture(&pool).await;
    let keys = keys();
    let member = bearer(&pool, &keys, fixture.worker, "MEMBER").await;
    let manager = bearer(&pool, &keys, fixture.manager, "SUPER_ADMIN").await;
    let service = build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem).unwrap());
    let ack_path = "/api/v1/attendance/week52/acks";
    let ack_body = json!({"employee_id":fixture.employee,"week_start":"2026-07-20"});
    let resolve_path = format!(
        "/api/v1/attendance/exceptions/{}/resolve",
        fixture.exception_id
    );
    let resolve_body = json!({"action":"CONFIRM","reason":"worker cannot manage an exception"});
    let member_active = post(
        service.clone(),
        &resolve_path,
        &member,
        resolve_body.clone(),
    )
    .await;
    assert_eq!(
        member_active.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        member_active.json
    );
    let active_ack = post(service.clone(), ack_path, &manager, ack_body.clone()).await;
    assert_eq!(active_ack.status, StatusCode::OK, "{:?}", active_ack.json);
    assert_eq!(active_ack.json["acked"], true);
    assert_eq!(active_ack.json["current_hours"], 8.0);
    let before = read_history(service.clone(), &member).await;
    assert!(before[1]["projection"]["acknowledged_at"].is_string());
    let ack_before = acknowledgment_and_audit_counts(&pool).await;
    assert_eq!(ack_before.0, 1);
    assert_eq!(ack_before.1, 1);
    let resolution_before: (String, Option<String>, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT e.status,r.action,r.resolved_at FROM attendance_exceptions e LEFT JOIN attendance_exception_resolutions r ON r.org_id=e.org_id AND r.exception_id=e.id WHERE e.id=$1",
    )
    .bind(fixture.exception_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let audits_before: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE employees SET employment_status='EXITED' WHERE id=$1")
        .bind(fixture.employee)
        .execute(&pool)
        .await
        .unwrap();
    // ACK refusal is subject eligibility, including for an authorized manager.
    for token in [&member, &manager] {
        let denied = post(service.clone(), ack_path, token, ack_body.clone()).await;
        assert_eq!(denied.status, StatusCode::NOT_FOUND, "{:?}", denied.json);
    }
    let member_exited = post(service.clone(), &resolve_path, &member, resolve_body).await;
    assert_eq!(
        member_exited.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        member_exited.json
    );
    assert_eq!(acknowledgment_and_audit_counts(&pool).await, ack_before);
    let resolution_after: (String, Option<String>, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT e.status,r.action,r.resolved_at FROM attendance_exceptions e LEFT JOIN attendance_exception_resolutions r ON r.org_id=e.org_id AND r.exception_id=e.id WHERE e.id=$1",
    )
    .bind(fixture.exception_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(resolution_after, resolution_before);
    let resolutions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM attendance_exception_resolutions WHERE exception_id=$1",
    )
    .bind(fixture.exception_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(resolutions, 0);
    let audits_after: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(audits_after, audits_before);
    // Capture after the valid ACK, so its retained evidence is also preserved.
    assert_eq!(read_history(service, &member).await, before);
}

struct HistoryFixture {
    worker: UserId,
    employee: Uuid,
    manager: UserId,
    branch: Uuid,
    exception_id: Uuid,
}

async fn history_fixture(pool: &PgPool) -> HistoryFixture {
    let branch = *console_platform_test_support::seed_branch(pool, "history-http", "operations")
        .await
        .as_uuid();
    let manager = UserId::new();
    seed_user(pool, manager, "SUPER_ADMIN").await;
    let worker = UserId::new();
    let employee = seed_linked_employee(pool, worker, "MEMBER", "history-worker").await;
    let other = UserId::new();
    let other_employee = seed_linked_employee(pool, other, "MEMBER", "other-history-worker").await;
    sqlx::query("INSERT INTO user_branches (user_id,branch_id,org_id) VALUES ($1,$2,$3)")
        .bind(*worker.as_uuid())
        .bind(branch)
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    let updated: OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM employees WHERE id=$1")
            .bind(employee)
            .fetch_one(pool)
            .await
            .unwrap();
    let mut command = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE console_leave_cmd")
        .execute(&mut *command)
        .await
        .unwrap();
    sqlx::query("SELECT * FROM leave_api.set_employee_home_branch($1,$2,$3,$4,$5,$6,$7)")
        .bind(*OrgId::knl().as_uuid())
        .bind(employee)
        .bind(branch)
        .bind(updated)
        .bind(*manager.as_uuid())
        .bind("0123456789abcdef0123456789abcdef")
        .bind("0123456789abcdef")
        .fetch_one(&mut *command)
        .await
        .unwrap();
    command.commit().await.unwrap();
    for (actor, employee_id, code) in [
        (worker, employee, "history-own"),
        (other, other_employee, "history-other"),
    ] {
        for (kind, at, state) in [
            ("CLOCK_IN", "2026-07-20 09:00:00+09", "CLOCKED_IN"),
            ("CLOCK_OUT", "2026-07-20 17:00:00+09", "OFF_DUTY"),
        ] {
            let record: Uuid = sqlx::query_scalar("INSERT INTO employee_attendance_records (org_id,employee_id,actor_user_id,kind,occurred_at,work_date,state_after,idempotency_key) VALUES ($1,$2,$3,$4,$5::text::timestamptz,DATE '2026-07-20',$6,$7) RETURNING id")
                .bind(*OrgId::knl().as_uuid()).bind(employee_id).bind(*actor.as_uuid()).bind(kind)
                .bind(at).bind(state).bind(Uuid::new_v4().to_string()).fetch_one(pool).await.unwrap();
            // The real HR projection joins this per-event material reference.
            sqlx::query("INSERT INTO payroll_attendance_material_refs (org_id,attendance_record_id,employee_id,work_date,source_digest) VALUES ($1,$2,$3,DATE '2026-07-20',$4)")
                .bind(*OrgId::knl().as_uuid()).bind(record).bind(employee_id).bind("a".repeat(64))
                .execute(pool).await.unwrap();
        }
        seed_exception(pool, actor, employee_id, code, Some(branch)).await;
    }
    let exception_id: Uuid =
        sqlx::query_scalar("SELECT id FROM attendance_exceptions WHERE employee_id=$1")
            .bind(employee)
            .fetch_one(pool)
            .await
            .unwrap();
    HistoryFixture {
        worker,
        employee,
        manager,
        branch,
        exception_id,
    }
}

fn history_paths() -> [String; 3] {
    [
        format!("{MY_EXCEPTIONS_PATH}?work_date=2026-07-20"),
        format!("{MY_WEEK52_PATH}?week_start=2026-07-20"),
        ME_PATH.to_owned(),
    ]
}

async fn read_history(service: axum::Router, token: &str) -> [Value; 3] {
    let mut values = Vec::new();
    for path in history_paths() {
        let response = get(service.clone(), &path, token).await;
        assert_eq!(
            response.status,
            StatusCode::OK,
            "{path}: {:?}",
            response.json
        );
        values.push(response.json);
    }
    values.try_into().unwrap()
}

async fn assert_history_unauthorized(service: axum::Router, token: &str) {
    for path in history_paths() {
        let response = get(service.clone(), &path, token).await;
        assert_eq!(
            response.status,
            StatusCode::UNAUTHORIZED,
            "{path}: {:?}",
            response.json
        );
    }
}

async fn acknowledgment_and_audit_counts(pool: &PgPool) -> (i64, i64) {
    let acknowledgments =
        sqlx::query_scalar("SELECT count(*) FROM attendance_week52_acknowledgements")
            .fetch_one(pool)
            .await
            .unwrap();
    let audits = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action='attendance.week52.acknowledge'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    (acknowledgments, audits)
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

async fn seed_exception(
    pool: &PgPool,
    actor: UserId,
    employee_id: Uuid,
    code: &str,
    branch: Option<Uuid>,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO attendance_exceptions (org_id, code, kind, employee_id, work_date, occurred_at, detail, idempotency_key, request_fingerprint, created_by, branch_id) VALUES ($1, $2, 'LATE', $3, DATE '2026-07-20', TIMESTAMPTZ '2026-07-20 09:00:00+00', 'late', $4, $5, $6, $7) RETURNING id",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(code)
    .bind(employee_id)
    .bind(format!("{code}-idempotency-key"))
    .bind("a".repeat(64))
    .bind(*actor.as_uuid())
    .bind(branch)
    .fetch_one(pool)
    .await
    .unwrap()
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

async fn bearer(pool: &PgPool, keys: &Keys, user_id: UserId, role: &str) -> String {
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
    let versions: (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(v.version,0),COALESCE(v.session_generation,0) FROM users u LEFT JOIN subject_authz_versions v ON v.user_id=u.id AND v.org_id=u.org_id WHERE u.id=$1",
    ).bind(*user_id.as_uuid()).fetch_one(pool).await.unwrap();
    console_platform_test_support::issue_session_token(
        pool,
        &issuer,
        AccessTokenInput {
            subject: user_id,
            org_id: OrgId::knl(),
            roles: vec![role.to_owned()],
            branches: Vec::new(),
            platform: false,
            view_as: false,
            read_only: false,
            display_name: None,
            feature_grants: Vec::new(),
            authz_subject_version: versions.0.try_into().unwrap(),
            authz_policy_version: 0,
            session_generation: versions.1.try_into().unwrap(),
            issued_at: OffsetDateTime::now_utc(),
        },
        None,
        Vec::new(),
    )
    .await
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
