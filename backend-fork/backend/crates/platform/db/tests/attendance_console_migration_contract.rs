#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Contract proof for attendance console persistence.  It exercises the schema
//! as the non-owner runtime role, so tenant isolation and least privilege are
//! not inferred from DDL text alone.

use sqlx::{
    PgPool, Row,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::str::FromStr;
use tokio::{
    sync::oneshot,
    time::{Duration, Instant, sleep},
};
use uuid::Uuid;

const MIGRATION_0196: &str =
    include_str!("../migrations/0196_platform_force_command_and_fk_closure.sql");
const FORCE_MIGRATOR_PASSWORD: &str = "platform-force-migration-owner-a198";
const ORG_A: Uuid = Uuid::from_u128(0x1880_0000_0000_0000_0000_0000_0000_0001);
const ORG_B: Uuid = Uuid::from_u128(0x1880_0000_0000_0000_0000_0000_0000_0002);
const FINGERPRINT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct Seeded {
    branch: Uuid,
    employee: Uuid,
    user: Uuid,
}

fn database_error_code(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()?
        .code()
        .map(|code| code.into_owned())
}

async fn seed_org(pool: &PgPool, org: Uuid, tag: &str) -> Seeded {
    let region = Uuid::new_v4();
    let branch = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let user = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(org)
        .bind(format!("attendance-contract-{tag}"))
        .bind(format!("Attendance contract {tag}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO regions (id, name, org_id) VALUES ($1, $2, $3)")
        .bind(region)
        .bind(format!("Region {tag}"))
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO branches (id, region_id, name, org_id) VALUES ($1, $2, $3, $4)")
        .bind(branch)
        .bind(region)
        .bind(format!("Branch {tag}"))
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(user)
        .bind(format!("User {tag}"))
        .bind(vec!["MECHANIC".to_owned()])
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO employees (id, org_id, company, name, source_filename, source_sheet, source_row, source_key) \
         VALUES ($1, $2, 'Maintenance', $3, 'attendance-contract', 'employees', 1, $4)",
    )
    .bind(employee)
    .bind(org)
    .bind(format!("Employee {tag}"))
    .bind(format!("attendance-contract-{tag}"))
    .execute(pool)
    .await
    .unwrap();
    Seeded {
        branch,
        employee,
        user,
    }
}

async fn runtime_tx(pool: &PgPool, org: Uuid) -> sqlx::Transaction<'_, sqlx::Postgres> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE console_rt")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx
}

#[sqlx::test(migrations = "./migrations")]
async fn attendance_console_contract_is_tenant_scoped_immutable_and_idempotent(pool: PgPool) {
    let a = seed_org(&pool, ORG_A, "a").await;
    let b = seed_org(&pool, ORG_B, "b").await;

    for table in [
        "attendance_exceptions",
        "attendance_exception_resolutions",
        "attendance_substitutions",
        "attendance_month_closes",
        "attendance_close_amendments",
        "attendance_week52_acknowledgements",
    ] {
        let row = sqlx::query(
            "SELECT c.relrowsecurity, c.relforcerowsecurity, has_table_privilege('console_rt', c.oid, 'SELECT,INSERT') AS can_read_write \
             FROM pg_class c WHERE c.oid = $1::regclass",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            row.get::<bool, _>("relrowsecurity"),
            "{table} must enable RLS"
        );
        assert!(
            row.get::<bool, _>("relforcerowsecurity"),
            "{table} must force RLS"
        );
        assert!(
            row.get::<bool, _>("can_read_write"),
            "{table} needs runtime SELECT/INSERT"
        );
    }

    let mut tx = runtime_tx(&pool, ORG_A).await;
    let exception_id: Uuid = sqlx::query_scalar(
        "INSERT INTO attendance_exceptions \
         (org_id, code, kind, employee_id, branch_id, work_date, detail, created_by, idempotency_key, request_fingerprint) \
         VALUES ($1, 'AT-1', 'LATE', $2, $3, DATE '2026-07-01', 'late arrival', $4, 'exception-create-0001', $5) RETURNING id",
    )
    .bind(ORG_A).bind(a.employee).bind(a.branch).bind(a.user).bind(FINGERPRINT)
    .fetch_one(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = runtime_tx(&pool, ORG_A).await;
    let duplicate = sqlx::query(
        "INSERT INTO attendance_exceptions \
         (org_id, code, kind, employee_id, branch_id, work_date, detail, created_by, idempotency_key, request_fingerprint) \
         VALUES ($1, 'AT-2', 'LATE', $2, $3, DATE '2026-07-01', 'duplicate', $4, 'exception-create-0001', $5)",
    )
    .bind(ORG_A).bind(a.employee).bind(a.branch).bind(a.user).bind(FINGERPRINT)
    .execute(&mut *tx).await;
    assert!(
        duplicate.is_err(),
        "exception create key must be unique per org"
    );
    tx.rollback().await.unwrap();

    let mut tx = runtime_tx(&pool, ORG_B).await;
    let invisible: i64 =
        sqlx::query_scalar("SELECT count(*) FROM attendance_exceptions WHERE id = $1")
            .bind(exception_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(invisible, 0, "FORCE RLS must hide another org's exception");
    let cross_org = sqlx::query(
        "INSERT INTO attendance_exceptions \
         (org_id, code, kind, employee_id, branch_id, work_date, detail, created_by, idempotency_key, request_fingerprint) \
         VALUES ($1, 'AT-cross', 'LATE', $2, $3, DATE '2026-07-01', 'cross-org', $4, 'exception-create-cross', $5)",
    )
    .bind(ORG_A).bind(b.employee).bind(b.branch).bind(b.user).bind(FINGERPRINT)
    .execute(&mut *tx).await;
    assert!(
        cross_org.is_err(),
        "RLS WITH CHECK must reject a cross-org write"
    );
    tx.rollback().await.unwrap();

    let immutable =
        sqlx::query("UPDATE attendance_exceptions SET org_id = $1 WHERE id = $2 AND org_id = $3")
            .bind(ORG_B)
            .bind(exception_id)
            .bind(ORG_A)
            .execute(&pool)
            .await;
    assert!(immutable.is_err(), "exception org_id must be immutable");

    let mut tx = runtime_tx(&pool, ORG_A).await;
    let resolution = sqlx::query(
        "INSERT INTO attendance_exception_resolutions (org_id, exception_id, action, reason, actor_user_id) \
         VALUES ($1, $2, 'CONFIRM', 'reviewed', $3)",
    )
    .bind(ORG_A)
    .bind(exception_id)
    .bind(a.user)
    .execute(&mut *tx)
    .await;
    assert!(resolution.is_ok(), "a resolution is an append-only fact");
    sqlx::query("UPDATE attendance_exceptions SET status = 'RESOLVED' WHERE id = $1")
        .bind(exception_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit()
        .await
        .expect("matching resolution and RESOLVED state commit atomically");
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let resolution_rewrite = sqlx::query(
        "UPDATE attendance_exception_resolutions SET reason = 'rewritten' WHERE exception_id = $1",
    )
    .bind(exception_id)
    .execute(&mut *tx)
    .await;
    assert!(
        resolution_rewrite.is_err(),
        "resolution must be append-only"
    );
    tx.rollback().await.unwrap();

    let mut tx = runtime_tx(&pool, ORG_A).await;
    let substitution = sqlx::query(
        "INSERT INTO attendance_substitutions \
         (org_id, site, branch_id, role, cover_date, from_minutes, to_minutes, covered_employee_id, reason_kind, worker_name, worker_type, created_by, idempotency_key, request_fingerprint) \
         VALUES ($1, 'A site', $2, 'mechanic', DATE '2026-07-01', 540, 1020, $3, 'NO_SHOW', 'Cover worker', 'part_time', $4, 'substitution-create-1', $5)",
    )
    .bind(ORG_A).bind(a.branch).bind(a.employee).bind(a.user).bind(FINGERPRINT)
    .execute(&mut *tx).await;
    assert!(
        substitution.is_ok(),
        "runtime role may create a substitution"
    );
    tx.commit().await.unwrap();
    let mut tx = runtime_tx(&pool, ORG_A).await;
    let substitution_duplicate = sqlx::query(
        "INSERT INTO attendance_substitutions \
         (org_id, site, branch_id, role, cover_date, from_minutes, to_minutes, covered_employee_id, reason_kind, worker_name, worker_type, created_by, idempotency_key, request_fingerprint) \
         VALUES ($1, 'A site', $2, 'mechanic', DATE '2026-07-01', 540, 1020, $3, 'NO_SHOW', 'Cover worker', 'part_time', $4, 'substitution-create-1', $5)",
    )
    .bind(ORG_A).bind(a.branch).bind(a.employee).bind(a.user).bind(FINGERPRINT)
    .execute(&mut *tx).await;
    assert!(
        substitution_duplicate.is_err(),
        "substitution create key must be unique per org"
    );
    tx.rollback().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let lock_id: Uuid = sqlx::query_scalar(
        "INSERT INTO period_locks (org_id, domain, period_start, period_end, reason) \
         VALUES ($1, 'payroll', DATE '2026-07-01', DATE '2026-07-31', 'attendance close') RETURNING id",
    )
    .bind(ORG_A).fetch_one(&mut *tx).await.unwrap();
    let close_id: Uuid = sqlx::query_scalar(
        "INSERT INTO attendance_month_closes (org_id, month, checks, attested_by, period_lock_id) \
         VALUES ($1, DATE '2026-07-01', '{}'::jsonb, $2, $3) RETURNING id",
    )
    .bind(ORG_A)
    .bind(a.user)
    .bind(lock_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let amendment = sqlx::query(
        "INSERT INTO attendance_close_amendments \
         (org_id, close_id, reason, detail, actor_user_id, idempotency_key, request_fingerprint) \
         VALUES ($1, $2, 'correction', 'retroactive correction', $3, 'amendment-create-1', $4)",
    )
    .bind(ORG_A)
    .bind(close_id)
    .bind(a.user)
    .bind(FINGERPRINT)
    .execute(&mut *tx)
    .await;
    assert!(
        amendment.is_ok(),
        "a close amendment is an append-only create"
    );
    tx.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let amendment_duplicate = sqlx::query(
        "INSERT INTO attendance_close_amendments \
         (org_id, close_id, reason, detail, actor_user_id, idempotency_key, request_fingerprint) \
         VALUES ($1, $2, 'correction', 'same retry', $3, 'amendment-create-1', $4)",
    )
    .bind(ORG_A)
    .bind(close_id)
    .bind(a.user)
    .bind(FINGERPRINT)
    .execute(&mut *tx)
    .await;
    assert!(
        amendment_duplicate.is_err(),
        "amendment create key must be unique per org"
    );
    tx.rollback().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let invalid_org_close = sqlx::query(
        "INSERT INTO attendance_month_closes (org_id, month, checks, attested_by) \
         VALUES ($1, DATE '2026-07-01', '{}'::jsonb, $2)",
    )
    .bind(ORG_A)
    .bind(a.user)
    .execute(&mut *tx)
    .await;
    assert!(
        invalid_org_close.is_err(),
        "organization close must require its period lock"
    );
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn attendance_close_lock_and_exception_resolution_invariants_are_deferred(pool: PgPool) {
    let a = seed_org(&pool, ORG_A, "deferred").await;
    let exception_id = Uuid::new_v4();
    let mut tx = runtime_tx(&pool, ORG_A).await;
    sqlx::query(
        "INSERT INTO attendance_exceptions \
         (id, org_id, code, kind, employee_id, branch_id, work_date, detail, created_by, idempotency_key, request_fingerprint) \
         VALUES ($1, $2, 'AT-deferred', 'LATE', $3, $4, DATE '2026-07-01', 'late arrival', $5, 'exception-deferred-1', $6)",
    )
    .bind(exception_id)
    .bind(ORG_A)
    .bind(a.employee)
    .bind(a.branch)
    .bind(a.user)
    .bind(FINGERPRINT)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // A lone resolution and a lone RESOLVED state both remain legal interim
    // statements, but are rejected by the deferred pair invariant at commit.
    let mut tx = runtime_tx(&pool, ORG_A).await;
    sqlx::query(
        "INSERT INTO attendance_exception_resolutions (org_id, exception_id, action, reason, actor_user_id) \
         VALUES ($1, $2, 'CONFIRM', 'premature', $3)",
    )
    .bind(ORG_A)
    .bind(exception_id)
    .bind(a.user)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert!(
        tx.commit().await.is_err(),
        "OPEN exception cannot retain a resolution"
    );

    let mut tx = runtime_tx(&pool, ORG_A).await;
    sqlx::query("UPDATE attendance_exceptions SET status = 'RESOLVED' WHERE id = $1")
        .bind(exception_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(
        tx.commit().await.is_err(),
        "RESOLVED exception requires one resolution"
    );

    let mut tx = runtime_tx(&pool, ORG_A).await;
    sqlx::query(
        "INSERT INTO attendance_exception_resolutions (org_id, exception_id, action, reason, actor_user_id) \
         VALUES ($1, $2, 'CONFIRM', 'resolved atomically', $3)",
    )
    .bind(ORG_A)
    .bind(exception_id)
    .bind(a.user)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("UPDATE attendance_exceptions SET status = 'RESOLVED' WHERE id = $1")
        .bind(exception_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit()
        .await
        .expect("matching resolution and terminal state commit together");

    for (month, domain, period_start, period_end, unlocked, key) in [
        (
            "2026-08-01",
            "accounting",
            "2026-08-01",
            "2026-08-31",
            false,
            "wrong-domain",
        ),
        (
            "2026-09-01",
            "payroll",
            "2026-09-01",
            "2026-09-29",
            false,
            "wrong-period",
        ),
        (
            "2026-10-01",
            "payroll",
            "2026-10-01",
            "2026-10-31",
            true,
            "inactive",
        ),
    ] {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(ORG_A.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        let lock_id: Uuid = sqlx::query_scalar(
            "INSERT INTO period_locks (org_id, domain, period_start, period_end, reason, unlocked_at, unlock_reason) \
             VALUES ($1, $2, $3::date, $4::date, 'attendance close test', \
                     CASE WHEN $5 THEN now() ELSE NULL END, CASE WHEN $5 THEN 'inactive' ELSE NULL END) RETURNING id",
        )
        .bind(ORG_A)
        .bind(domain)
        .bind(period_start)
        .bind(period_end)
        .bind(unlocked)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO attendance_month_closes (org_id, month, checks, attested_by, period_lock_id) \
             VALUES ($1, $2::date, '{}'::jsonb, $3, $4)",
        )
        .bind(ORG_A)
        .bind(month)
        .bind(a.user)
        .bind(lock_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        assert!(
            tx.commit().await.is_err(),
            "{key} lock must not support an org close"
        );
    }

    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let lock_id: Uuid = sqlx::query_scalar(
        "INSERT INTO period_locks (org_id, domain, period_start, period_end, reason) \
         VALUES ($1, 'payroll', DATE '2026-11-01', DATE '2026-11-30', 'attendance close') RETURNING id",
    )
    .bind(ORG_A)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO attendance_month_closes (org_id, month, checks, attested_by, period_lock_id) \
         VALUES ($1, DATE '2026-11-01', '{}'::jsonb, $2, $3)",
    )
    .bind(ORG_A)
    .bind(a.user)
    .bind(lock_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit()
        .await
        .expect("exact active payroll lock supports org close");
}

#[sqlx::test(migrations = "./migrations")]
async fn employee_day_eligibility_coordination_is_catalogued_and_enforced(pool: PgPool) {
    let a = seed_org(&pool, ORG_A, "eligibility").await;

    let lock = sqlx::query(
        "SELECT p.provolatile::text AS provolatile, p.proparallel::text AS proparallel, p.prosecdef, p.proconfig, \
                has_function_privilege('console_rt', p.oid, 'EXECUTE') AS runtime_execute, \
                has_function_privilege('console_leave_definer', p.oid, 'EXECUTE') AS leave_execute, \
                EXISTS (SELECT 1 FROM aclexplode(coalesce(p.proacl, acldefault('f', p.proowner))) privilege \
                        WHERE privilege.grantee = 0 AND privilege.privilege_type = 'EXECUTE') AS public_execute, \
                pg_get_functiondef(p.oid) AS definition \
         FROM pg_proc p WHERE p.oid = 'public.console_employee_day_eligibility_lock(uuid,uuid,date)'::regprocedure",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(lock.get::<String, _>("provolatile"), "v");
    assert_eq!(lock.get::<String, _>("proparallel"), "u");
    assert!(
        !lock.get::<bool, _>("prosecdef"),
        "lock helper must use invoker rights"
    );
    assert!(
        lock.get::<Option<Vec<String>>, _>("proconfig")
            .unwrap_or_default()
            .iter()
            .any(|setting| setting == "search_path=pg_catalog"),
        "lock helper must pin pg_catalog"
    );
    assert!(lock.get::<bool, _>("runtime_execute"));
    assert!(lock.get::<bool, _>("leave_execute"));
    assert!(!lock.get::<bool, _>("public_execute"));
    assert!(
        lock.get::<String, _>("definition")
            .contains("attendance-substitution-eligibility-v1|"),
        "the lock material is a cross-domain compatibility contract"
    );
    assert!(
        lock.get::<String, _>("definition")
            .contains("to_char(p_work_date, 'YYYY-MM-DD')")
            && lock.get::<String, _>("definition").contains(" 0\n"),
        "the lock material must preserve the legacy seed-zero YYYY-MM-DD bytes"
    );

    let lock_material = format!(
        "attendance-substitution-eligibility-v1|{ORG_A}|{}|2026-07-02",
        a.employee
    );
    let mut lock_tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL DateStyle TO 'SQL, DMY'")
        .execute(&mut *lock_tx)
        .await
        .unwrap();
    sqlx::query("SELECT public.console_employee_day_eligibility_lock($1, $2, DATE '2026-07-02')")
        .bind(ORG_A)
        .bind(a.employee)
        .execute(&mut *lock_tx)
        .await
        .unwrap();
    let legacy_key_available: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(pg_catalog.hashtextextended($1, 0))")
            .bind(lock_material)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        !legacy_key_available,
        "alternate DateStyle must still lock the exact legacy Rust key"
    );
    lock_tx.rollback().await.unwrap();

    let triggers = sqlx::query(
        "SELECT c.relname AS table_name, t.tgname, t.tgtype::integer AS trigger_type, \
                t.tgenabled::text AS enabled, p.proname AS function_name \
         FROM pg_trigger t \
         JOIN pg_class c ON c.oid = t.tgrelid \
         JOIN pg_proc p ON p.oid = t.tgfoid \
         WHERE NOT t.tgisinternal AND t.tgname IN ( \
             'trg_attendance_exceptions_eligibility_lock', \
             'trg_attendance_substitutions_eligibility_guard', \
             'trg_leave_requests_eligibility_lock' \
         ) ORDER BY t.tgname",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let trigger_shape: Vec<(String, String, i32, String, String)> = triggers
        .iter()
        .map(|row| {
            (
                row.get("table_name"),
                row.get("tgname"),
                row.get("trigger_type"),
                row.get("enabled"),
                row.get("function_name"),
            )
        })
        .collect();
    assert_eq!(
        trigger_shape,
        vec![
            (
                "attendance_exceptions".into(),
                "trg_attendance_exceptions_eligibility_lock".into(),
                23,
                "O".into(),
                "console_attendance_exception_eligibility_lock".into()
            ),
            (
                "attendance_substitutions".into(),
                "trg_attendance_substitutions_eligibility_guard".into(),
                23,
                "O".into(),
                "console_attendance_substitution_eligibility_guard".into()
            ),
            (
                "leave_requests".into(),
                "trg_leave_requests_eligibility_lock".into(),
                19,
                "O".into(),
                "console_leave_request_eligibility_lock".into()
            ),
        ],
        "all transition triggers must retain their exact table, event, timing, function, and enabled metadata"
    );

    let decide_definition: String = sqlx::query_scalar(
        "SELECT pg_get_functiondef('leave_api.decide_request(uuid,uuid,uuid,bigint,text,text,text,text)'::regprocedure)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let lock_at = decide_definition
        .find("console_employee_day_eligibility_lock")
        .expect("leave approval takes sorted employee/day locks");
    let employee_lock_at = decide_definition
        .find("FROM public.employees e")
        .expect("leave decision retains its employee row lock");
    assert!(
        lock_at < employee_lock_at && decide_definition.contains("ORDER BY work_date"),
        "employee/day locks must precede the employee lock in deterministic date order"
    );

    let mut tx = runtime_tx(&pool, ORG_A).await;
    sqlx::query(
        "INSERT INTO attendance_exceptions \
         (org_id, code, kind, employee_id, branch_id, work_date, detail, created_by, idempotency_key, request_fingerprint) \
         VALUES ($1, 'AT-eligibility', 'NO_SHOW', $2, $3, DATE '2026-07-02', 'unavailable', $4, 'exception-eligibility-1', $5)",
    )
    .bind(ORG_A)
    .bind(a.employee)
    .bind(a.branch)
    .bind(a.user)
    .bind(FINGERPRINT)
    .execute(&mut *tx)
    .await
    .unwrap();
    let legacy = sqlx::query(
        "INSERT INTO attendance_substitutions \
         (org_id, site, branch_id, role, cover_date, from_minutes, to_minutes, covered_employee_id, reason_kind, worker_name, worker_type, created_by, idempotency_key, request_fingerprint) \
         VALUES ($1, 'A site', $2, 'mechanic', DATE '2026-07-02', 540, 1020, $3, 'NO_SHOW', 'Legacy cover', 'part_time', $4, 'substitution-legacy-null-1', $5)",
    )
    .bind(ORG_A)
    .bind(a.branch)
    .bind(a.employee)
    .bind(a.user)
    .bind(FINGERPRINT)
    .execute(&mut *tx)
    .await;
    assert!(
        legacy.is_ok(),
        "legacy NULL worker assignments remain compatible"
    );
    let guarded = sqlx::query(
        "INSERT INTO attendance_substitutions \
         (org_id, site, branch_id, role, cover_date, from_minutes, to_minutes, covered_employee_id, reason_kind, worker_employee_id, worker_name, worker_type, created_by, idempotency_key, request_fingerprint) \
         VALUES ($1, 'A site', $2, 'mechanic', DATE '2026-07-02', 540, 1020, $3, 'NO_SHOW', $3, 'Known cover', 'part_time', $4, 'substitution-guarded-worker-1', $5)",
    )
    .bind(ORG_A)
    .bind(a.branch)
    .bind(a.employee)
    .bind(a.user)
    .bind(FINGERPRINT)
    .execute(&mut *tx)
    .await
    .expect_err("known worker must be rejected when an open NO_SHOW exists");
    assert_eq!(
        guarded.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    assert_eq!(
        guarded.as_database_error().unwrap().message(),
        "attendance_substitutions_worker_eligibility_guard"
    );
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn employee_day_locks_serialize_leave_approval_and_release_terminal_transitions(
    pool: PgPool,
) {
    let a = seed_org(&pool, ORG_A, "coordination").await;
    let decider = Uuid::new_v4();
    let leave_request = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, 'Leave decider', $2, $3)",
    )
    .bind(decider)
    .bind(vec!["MECHANIC".to_owned()])
    .bind(ORG_A)
    .execute(&pool)
    .await
    .unwrap();

    let mut create_leave = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE console_leave_definer")
        .execute(&mut *create_leave)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(&mut *create_leave)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO leave_requests \
         (id, org_id, branch_id, requester_user_id, subject_employee_id, leave_type, days, start_date, end_date) \
         VALUES ($1, $2, $3, $4, $5, 'annual', 1, DATE '2026-07-03', DATE '2026-07-03')",
    )
    .bind(leave_request)
    .bind(ORG_A)
    .bind(a.branch)
    .bind(a.user)
    .bind(a.employee)
    .execute(&mut *create_leave)
    .await
    .unwrap();
    create_leave.commit().await.unwrap();

    let mut approve_leave = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE console_leave_definer")
        .execute(&mut *approve_leave)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(&mut *approve_leave)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE leave_requests SET status = 'approved', charge_state = 'legacy_unverified', charge_review_reasons = ARRAY[]::text[], charge_units = 1, decided_by = $1, decided_at = now() \
         WHERE id = $2 AND org_id = $3",
    )
    .bind(decider)
    .bind(leave_request)
    .bind(ORG_A)
    .execute(&mut *approve_leave)
    .await
    .unwrap();

    let connection_options = pool.connect_options().as_ref().clone();
    let assignment_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(connection_options.clone())
        .await
        .unwrap();
    let observer_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(connection_options)
        .await
        .unwrap();
    let (pid_sender, pid_receiver) = oneshot::channel();
    let assignment = tokio::spawn(async move {
        let mut tx = runtime_tx(&assignment_pool, ORG_A).await;
        let backend_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        pid_sender.send(backend_pid).unwrap();
        let result = sqlx::query(
            "INSERT INTO attendance_substitutions \
             (org_id, site, branch_id, role, cover_date, from_minutes, to_minutes, covered_employee_id, reason_kind, worker_employee_id, worker_name, worker_type, created_by, idempotency_key, request_fingerprint) \
             VALUES ($1, 'A site', $2, 'mechanic', DATE '2026-07-03', 540, 1020, $3, 'APPROVED_LEAVE', $3, 'Known cover', 'part_time', $4, 'substitution-concurrent-leave-1', $5)",
        )
        .bind(ORG_A)
        .bind(a.branch)
        .bind(a.employee)
        .bind(a.user)
        .bind(FINGERPRINT)
        .execute(&mut *tx)
        .await;
        tx.rollback().await.unwrap();
        result
    });
    let assignment_pid = pid_receiver.await.unwrap();
    let wait_deadline = Instant::now() + Duration::from_secs(2);
    let mut advisory_wait_observed = false;
    while Instant::now() < wait_deadline {
        advisory_wait_observed = sqlx::query_scalar(
            "SELECT COALESCE(a.wait_event_type = 'Lock' AND a.wait_event = 'advisory' \
                    AND EXISTS (SELECT 1 FROM pg_locks l \
                                WHERE l.pid = a.pid AND l.locktype = 'advisory' AND NOT l.granted), false) \
             FROM pg_stat_activity a WHERE a.pid = $1",
        )
        .bind(assignment_pid)
        .fetch_optional(&observer_pool)
        .await
        .unwrap()
        .unwrap_or(false);
        if advisory_wait_observed {
            break;
        }
        sleep(Duration::from_millis(10)).await;
    }
    assert!(
        advisory_wait_observed,
        "the exact assignment backend must wait on the advisory lock before the leave transaction commits"
    );
    approve_leave.commit().await.unwrap();
    let blocked = assignment
        .await
        .unwrap()
        .expect_err("leave approval wins after its lock commits");
    assert_eq!(
        blocked.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );

    let mut tx = runtime_tx(&pool, ORG_A).await;
    let rejected = sqlx::query(
        "INSERT INTO attendance_substitutions \
         (org_id, site, branch_id, role, cover_date, from_minutes, to_minutes, covered_employee_id, reason_kind, worker_employee_id, worker_name, worker_type, created_by, idempotency_key, request_fingerprint) \
         VALUES ($1, 'A site', $2, 'mechanic', DATE '2026-07-03', 540, 1020, $3, 'APPROVED_LEAVE', $3, 'Known cover', 'part_time', $4, 'substitution-after-leave-1', $5)",
    )
    .bind(ORG_A)
    .bind(a.branch)
    .bind(a.employee)
    .bind(a.user)
    .bind(FINGERPRINT)
    .execute(&mut *tx)
    .await
    .expect_err("a completed leave approval must reject the queued assignment");
    assert_eq!(
        rejected.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    tx.rollback().await.unwrap();

    let mut tx = runtime_tx(&pool, ORG_A).await;
    sqlx::query(
        "INSERT INTO attendance_substitutions \
         (org_id, site, branch_id, role, cover_date, from_minutes, to_minutes, covered_employee_id, reason_kind, worker_employee_id, worker_name, worker_type, created_by, idempotency_key, request_fingerprint) \
         VALUES ($1, 'A site', $2, 'mechanic', DATE '2026-07-04', 540, 1020, $3, 'OTHER', $3, 'First cover', 'part_time', $4, 'substitution-cancelled-1', $5)",
    )
    .bind(ORG_A)
    .bind(a.branch)
    .bind(a.employee)
    .bind(a.user)
    .bind(FINGERPRINT)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("UPDATE attendance_substitutions SET status = 'CANCELLED', cancel_reason = 'released' WHERE idempotency_key = 'substitution-cancelled-1'")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = runtime_tx(&pool, ORG_A).await;
    let replacement = sqlx::query(
        "INSERT INTO attendance_substitutions \
         (org_id, site, branch_id, role, cover_date, from_minutes, to_minutes, covered_employee_id, reason_kind, worker_employee_id, worker_name, worker_type, created_by, idempotency_key, request_fingerprint) \
         VALUES ($1, 'A site', $2, 'mechanic', DATE '2026-07-04', 540, 1020, $3, 'OTHER', $3, 'Replacement cover', 'part_time', $4, 'substitution-replacement-1', $5)",
    )
    .bind(ORG_A)
    .bind(a.branch)
    .bind(a.employee)
    .bind(a.user)
    .bind(FINGERPRINT)
    .execute(&mut *tx)
    .await;
    assert!(
        replacement.is_ok(),
        "cancelling an assignment releases that eligibility"
    );
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn platform_force_removal_closes_direct_org_restrict_fks_and_uses_dedicated_capability(
    pool: PgPool,
) {
    let owner_alignment: bool = sqlx::query_scalar(
        "SELECT bool_and(functions.proowner = organizations.relowner) \
         FROM unnest(ARRAY[ \
             'platform_force_remove_direct_org_children(uuid)'::regprocedure, \
             'platform_force_remove_organization(uuid)'::regprocedure, \
             'platform_force_remove_organization_command(uuid,uuid,character,character,timestamp with time zone)'::regprocedure \
         ]) AS procedures(oid) \
         JOIN pg_proc AS functions ON functions.oid = procedures.oid \
         CROSS JOIN pg_class AS organizations \
         WHERE organizations.oid = 'organizations'::regclass",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        owner_alignment,
        "force-remove SECURITY DEFINER functions must share the tenant-table owner so they retain \
         the same FORCE-RLS and direct-child privileges in production and isolated SQLx databases"
    );

    let function_body: String = sqlx::query_scalar(
        "SELECT pg_get_functiondef('platform_force_remove_organization_command(uuid,uuid,character,character,timestamp with time zone)'::regprocedure)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        function_body.contains("platform_force_remove_organization(p_id)"),
        "atomic command must delegate to the canonical direct-org FK closure"
    );

    let unsupported_direct_org_fk_count: i64 = sqlx::query_scalar(
        "SELECT count(*) \
         FROM pg_constraint fk \
         JOIN pg_class child ON child.oid = fk.conrelid \
         JOIN pg_namespace child_ns ON child_ns.oid = child.relnamespace \
         JOIN pg_class parent ON parent.oid = fk.confrelid \
         JOIN pg_namespace parent_ns ON parent_ns.oid = parent.relnamespace \
         LEFT JOIN pg_attribute child_attr \
           ON child_attr.attrelid = child.oid AND child_attr.attnum = fk.conkey[1] \
          AND NOT child_attr.attisdropped \
         WHERE fk.contype = 'f' AND fk.confdeltype IN ('a', 'r') \
           AND parent_ns.nspname = 'public' AND parent.relname = 'organizations' \
           AND child_ns.nspname = 'public' \
           AND (cardinality(fk.conkey) <> 1 OR child_attr.attname IS DISTINCT FROM 'org_id')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        unsupported_direct_org_fk_count, 0,
        "every direct restrictive organization FK must be handled by the closure or explicitly reviewed"
    );

    let permissions = sqlx::query(
        "SELECT has_function_privilege('console_rt', 'platform_force_remove_organization_command(uuid,uuid,character,character,timestamp with time zone)', 'EXECUTE') AS runtime_can_execute, \
                has_function_privilege('console_platform_force_cmd', 'platform_force_remove_organization_command(uuid,uuid,character,character,timestamp with time zone)', 'EXECUTE') AS command_can_execute, \
                EXISTS ( \
                    SELECT 1 \
                    FROM pg_class AS relation \
                    JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace \
                    CROSS JOIN unnest(ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE', 'TRUNCATE', 'REFERENCES', 'TRIGGER']) \
                        AS requested(privilege_name) \
                    WHERE namespace.nspname = 'public' \
                      AND relation.relkind IN ('r', 'p') \
                      AND has_table_privilege( \
                          'console_platform_force_cmd', relation.oid, requested.privilege_name \
                      ) \
                ) AS command_has_table_privilege",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !permissions.get::<bool, _>("runtime_can_execute"),
        "general runtime credential must not execute force removal"
    );
    assert!(
        permissions.get::<bool, _>("command_can_execute"),
        "dedicated platform command credential must execute force removal"
    );
    assert!(
        !permissions.get::<bool, _>("command_has_table_privilege"),
        "dedicated platform command credential must not receive direct access to any public table"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn platform_force_migration_rejects_superuser_on_console_app_owned_database(pool: PgPool) {
    sqlx::raw_sql(
        r#"
        ALTER ROLE console_app LOGIN INHERIT NOSUPERUSER BYPASSRLS NOCREATEDB NOCREATEROLE
            NOREPLICATION PASSWORD 'platform-force-migration-owner-a198';
        DO $database_owner$
        BEGIN
            EXECUTE format('ALTER DATABASE %I OWNER TO console_app', current_database());
        END
        $database_owner$;
        DO $ownership$
        DECLARE
            target RECORD;
        BEGIN
            FOR target IN
                SELECT namespace.nspname AS schema_name, relation.relname AS relation_name
                FROM pg_class AS relation
                JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace
                WHERE namespace.nspname = 'public'
                  AND relation.relkind IN ('r', 'p')
                  AND (
                      relation.relname IN ('organizations', 'auth_webauthn_ceremonies')
                      OR EXISTS (
                          SELECT 1
                          FROM pg_attribute AS attribute
                          WHERE attribute.attrelid = relation.oid
                            AND attribute.attname = 'org_id'
                            AND NOT attribute.attisdropped
                      )
                  )
            LOOP
                EXECUTE format(
                    'ALTER TABLE %I.%I OWNER TO console_app',
                    target.schema_name,
                    target.relation_name
                );
            END LOOP;
        END
        $ownership$;
        ALTER FUNCTION platform_force_remove_direct_org_children(UUID) OWNER TO console_app;
        ALTER FUNCTION platform_force_remove_organization(UUID) OWNER TO console_app;
        ALTER FUNCTION platform_force_remove_organization_command(
            UUID, UUID, CHAR(32), CHAR(16), TIMESTAMPTZ
        ) OWNER TO console_app;
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let mut dba_replay = pool.begin().await.unwrap();
    let rejected = sqlx::raw_sql(MIGRATION_0196)
        .execute(&mut *dba_replay)
        .await
        .expect_err(
            "a superuser must not create force-remove definers in an console_app-owned database",
        );
    assert_eq!(database_error_code(&rejected).as_deref(), Some("42501"));
    assert!(
        rejected
            .as_database_error()
            .is_some_and(|error| error.message()
                == "platform_force_role_topology.superuser_test_bootstrap_required")
    );
    dba_replay.rollback().await.unwrap();

    let migrator_options = pool
        .connect_options()
        .as_ref()
        .clone()
        .username("console_app")
        .password(FORCE_MIGRATOR_PASSWORD);
    let migrator = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(migrator_options)
        .await
        .unwrap();
    let identity = sqlx::query("SELECT current_user, session_user")
        .fetch_one(&migrator)
        .await
        .unwrap();
    assert_eq!(identity.get::<String, _>("current_user"), "console_app");
    assert_eq!(identity.get::<String, _>("session_user"), "console_app");
    sqlx::raw_sql(MIGRATION_0196)
        .execute(&migrator)
        .await
        .expect("the exact 0196 migration must replay through the direct console_app login");

    let owners: Vec<String> = sqlx::query_scalar(
        "SELECT owner.rolname \
         FROM unnest(ARRAY[ \
             'platform_force_remove_direct_org_children(uuid)'::regprocedure, \
             'platform_force_remove_organization(uuid)'::regprocedure, \
             'platform_force_remove_organization_command(uuid,uuid,character,character,timestamp with time zone)'::regprocedure \
         ]) AS procedures(oid) \
         JOIN pg_proc AS functions ON functions.oid = procedures.oid \
         JOIN pg_roles AS owner ON owner.oid = functions.proowner \
         ORDER BY procedures.oid",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(owners, vec!["console_app", "console_app", "console_app"]);
}

#[sqlx::test(migrations = "./migrations")]
async fn platform_force_migration_rejects_dba_owned_production_shaped_database(pool: PgPool) {
    let sqlx_identity = sqlx::query(
        "SELECT current_database() AS database_name, \
                current_user, \
                current_setting('console.sqlx_test_bootstrap', true) AS bootstrap_marker",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        sqlx_identity
            .get::<String, _>("database_name")
            .starts_with("_sqlx_test_")
    );
    assert_eq!(
        sqlx_identity.get::<String, _>("current_user"),
        "console_buck_admin"
    );
    assert_eq!(
        sqlx_identity.get::<String, _>("bootstrap_marker"),
        "buck-sqlx-superuser-v1"
    );

    let mut unmarked = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL console.sqlx_test_bootstrap = 'disabled'")
        .execute(&mut *unmarked)
        .await
        .unwrap();
    let unmarked_replay = sqlx::raw_sql(MIGRATION_0196).execute(&mut *unmarked).await;
    unmarked.rollback().await.unwrap();
    let unmarked_error =
        unmarked_replay.expect_err("a superuser migration without the exact test marker must fail");
    assert_eq!(
        database_error_code(&unmarked_error).as_deref(),
        Some("42501")
    );
    assert!(
        unmarked_error
            .as_database_error()
            .is_some_and(|error| error.message()
                == "platform_force_role_topology.superuser_test_bootstrap_required")
    );

    let master_options =
        PgConnectOptions::from_str(&std::env::var("DATABASE_URL").unwrap()).unwrap();
    let master = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(master_options.clone())
        .await
        .unwrap();
    let production_database = format!("platform_force_production_{}", Uuid::new_v4().simple());
    // The identifier is generated only from a UUID's lowercase hexadecimal
    // representation and a fixed prefix; it cannot contain SQL metacharacters.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE DATABASE \"{production_database}\""
    )))
    .execute(&master)
    .await
    .unwrap();
    let production = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(master_options.database(&production_database))
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE organizations (id UUID PRIMARY KEY)")
        .execute(&production)
        .await
        .unwrap();
    let production_replay = sqlx::raw_sql(MIGRATION_0196).execute(&production).await;
    production.close().await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE \"{production_database}\""
    )))
    .execute(&master)
    .await
    .unwrap();
    let production_error = production_replay
        .expect_err("a DBA-owned database outside the SQLx namespace must reject the migration");
    assert_eq!(
        database_error_code(&production_error).as_deref(),
        Some("42501")
    );
    assert!(
        production_error
            .as_database_error()
            .is_some_and(|error| error.message()
                == "platform_force_role_topology.superuser_test_bootstrap_required")
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn platform_force_migration_rejects_partially_drifted_force_table_owner(pool: PgPool) {
    sqlx::raw_sql(
        r#"
        ALTER ROLE console_app LOGIN INHERIT NOSUPERUSER BYPASSRLS NOCREATEDB NOCREATEROLE
            NOREPLICATION PASSWORD 'platform-force-migration-owner-a198';
        DO $ownership$
        DECLARE
            target RECORD;
        BEGIN
            EXECUTE format('ALTER DATABASE %I OWNER TO console_app', current_database());
            FOR target IN
                SELECT namespace.nspname AS schema_name, relation.relname AS relation_name
                FROM pg_class AS relation
                JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace
                WHERE namespace.nspname = 'public'
                  AND relation.relkind IN ('r', 'p')
                  AND (
                      relation.relname IN ('organizations', 'auth_webauthn_ceremonies')
                      OR EXISTS (
                          SELECT 1
                          FROM pg_attribute AS attribute
                          WHERE attribute.attrelid = relation.oid
                            AND attribute.attname = 'org_id'
                            AND NOT attribute.attisdropped
                      )
                  )
            LOOP
                EXECUTE format(
                    'ALTER TABLE %I.%I OWNER TO console_app',
                    target.schema_name,
                    target.relation_name
                );
            END LOOP;
        END
        $ownership$;
        ALTER FUNCTION platform_force_remove_direct_org_children(UUID) OWNER TO console_app;
        ALTER FUNCTION platform_force_remove_organization(UUID) OWNER TO console_app;
        ALTER FUNCTION platform_force_remove_organization_command(
            UUID, UUID, CHAR(32), CHAR(16), TIMESTAMPTZ
        ) OWNER TO console_app;
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql("ALTER TABLE registry_customers OWNER TO console_buck_admin")
        .execute(&pool)
        .await
        .unwrap();

    let migrator_options = pool
        .connect_options()
        .as_ref()
        .clone()
        .username("console_app")
        .password(FORCE_MIGRATOR_PASSWORD);
    let migrator = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(migrator_options)
        .await
        .unwrap();
    let drifted_replay = sqlx::raw_sql(MIGRATION_0196).execute(&migrator).await;
    let drifted_error =
        drifted_replay.expect_err("one drifted force-removal table owner must fail closed");
    assert_eq!(
        database_error_code(&drifted_error).as_deref(),
        Some("42501")
    );
    assert!(drifted_error.as_database_error().is_some_and(
        |error| error.message() == "platform_force_role_topology.force_table_owner_drift"
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn platform_force_removal_runtime_is_denied_and_command_role_can_remove_archived_org(
    pool: PgPool,
) {
    let org = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, status) VALUES ($1, $2, 'Force command contract', 'ARCHIVED')",
    )
    .bind(org)
    .bind(format!("force-command-{}", &org.to_string()[..8]))
    .execute(&pool)
    .await
    .unwrap();

    let mut runtime = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE console_rt")
        .execute(&mut *runtime)
        .await
        .unwrap();
    let denied = sqlx::query_scalar::<_, String>("SELECT platform_force_remove_organization_command($1, NULL::uuid, '00000000000000000000000000000000', '0000000000000000', now())")
        .bind(org)
        .fetch_one(&mut *runtime)
        .await
        .unwrap_err();
    let denied_code = denied
        .as_database_error()
        .and_then(|error| error.code().map(|code| code.into_owned()));
    assert_eq!(
        denied_code.as_deref(),
        Some("42501"),
        "console_rt must receive PostgreSQL insufficient_privilege, not a handler-only denial"
    );
    runtime.rollback().await.unwrap();

    let mut command = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE console_platform_force_cmd")
        .execute(&mut *command)
        .await
        .unwrap();
    let removed: String = sqlx::query_scalar("SELECT platform_force_remove_organization_command($1, NULL::uuid, '00000000000000000000000000000000', '0000000000000000', now())")
        .bind(org)
        .fetch_one(&mut *command)
        .await
        .unwrap();
    assert_eq!(removed, "removed");
    command.commit().await.unwrap();

    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM organizations WHERE id = $1")
        .bind(org)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        remaining, 0,
        "authorized command path must delete archived tenant"
    );
    let receipt: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'platform.tenant.force_remove' AND target_id = $1",
    )
    .bind(org.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        receipt, 1,
        "command must append exactly one immutable receipt"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn platform_force_removal_deletes_post_0188_attendance_rows_before_employee_roots(
    pool: PgPool,
) {
    let org = Uuid::new_v4();
    let seeded = seed_org(&pool, org, "force-attendance").await;
    sqlx::query("UPDATE organizations SET status = 'ARCHIVED' WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO attendance_exceptions \
         (org_id, code, kind, employee_id, branch_id, work_date, detail, created_by, idempotency_key, request_fingerprint) \
         VALUES ($1, 'AT-FORCE-1', 'LATE', $2, $3, DATE '2026-07-01', 'force removal contract', $4, 'force-remove-attendance-0001', $5)",
    )
    .bind(org)
    .bind(seeded.employee)
    .bind(seeded.branch)
    .bind(seeded.user)
    .bind(FINGERPRINT)
    .execute(&pool)
    .await
    .unwrap();

    let mut command = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE console_platform_force_cmd")
        .execute(&mut *command)
        .await
        .unwrap();
    let removed: String = sqlx::query_scalar("SELECT platform_force_remove_organization_command($1, NULL::uuid, '00000000000000000000000000000000', '0000000000000000', now())")
        .bind(org)
        .fetch_one(&mut *command)
        .await
        .unwrap();
    assert_eq!(removed, "removed");
    command.commit().await.unwrap();

    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM attendance_exceptions WHERE org_id = $1")
            .bind(org)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rows, 0);
}
