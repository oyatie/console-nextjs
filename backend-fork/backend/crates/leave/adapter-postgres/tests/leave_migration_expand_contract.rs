#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Populated 0165 -> 0166 expand/contract regression.
//!
//! A database migration can reach production before every old application
//! replica has drained, and rollback can deliberately put that old binary back.
//! The pre-0166 binary names `leave_requests.days` for reads and inserts, so
//! 0166 must preserve that surface while the new binary reads `legacy_days` and
//! writes exact charges only through `leave_api`.

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use console_governance_domain::{GateChainConfig, GateEvidence, evaluate_gate_chain};

const MIGRATION_0166: &str =
    include_str!("../../../platform/db/migrations/0166_leave_exact_charge_and_home_branch.sql");
const MIGRATOR_PASSWORD: &str = "leave-migration-owner-a166";
const MIGRATION_LOCK_TIMEOUT: &str = "5s";
const MIGRATION_STATEMENT_TIMEOUT: &str = "1min";

const ORG: Uuid = Uuid::from_u128(0xa166_a166_a166_a166_a166_a166_a166_a166);
const REGION: Uuid = Uuid::from_u128(0x1166_1166_1166_1166_1166_1166_1166_1166);
const BRANCH: Uuid = Uuid::from_u128(0xb166_b166_b166_b166_b166_b166_b166_b166);
const USER: Uuid = Uuid::from_u128(0xc166_c166_c166_c166_c166_c166_c166_c166);
const DECIDER: Uuid = Uuid::from_u128(0xc266_c266_c266_c266_c266_c266_c266_c266);
const EMPLOYEE: Uuid = Uuid::from_u128(0xd166_d166_d166_d166_d166_d166_d166_d166);
const PREEXISTING_REQUEST: Uuid = Uuid::from_u128(0xe166_e166_e166_e166_e166_e166_e166_e166);
const ROLLBACK_REQUEST: Uuid = Uuid::from_u128(0xf166_f166_f166_f166_f166_f166_f166_f166);
const STAGED_IMPORT_RUN: Uuid = Uuid::from_u128(0xa266_a266_a266_a266_a266_a266_a266_a266);
const F6FF_APPLY_AUDIT_SQL: &str = r#"
    INSERT INTO audit_events
        (actor,action,target_type,target_id,before_snap,after_snap,
         trace_id,span_id,occurred_at,org_id)
    VALUES ($1,'data_import.apply','data_import_run',$2,NULL,$3,
            'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','bbbbbbbbbbbbbbbb',now(),$4)
"#;
const F6FF_EMPLOYEE_UPSERT_SQL: &str = r#"
    INSERT INTO employees (
        org_id, company, name, source_filename, source_sheet, source_row,
        source_key, raw_row, source_metadata, employee_number, org_unit, job,
        position, worksite_name, worksite_address, hire_date, exit_date,
        employment_status, leave_accrued, leave_used, leave_remaining,
        identity_resolution_strategy, identity_resolution_confidence,
        identity_review_required, identity_name_only_merge
    )
    VALUES (
        $1, 'A166', $3, 'immediate.xlsx', 'Sheet1', 2,
        $2, '{}'::jsonb, '{}'::jsonb, 'E-166', NULL, NULL,
        NULL, NULL, NULL, DATE '2024-01-01', NULL,
        'ACTIVE', NULLIF($4::TEXT, '')::NUMERIC,
        NULLIF($5::TEXT, '')::NUMERIC, NULLIF($6::TEXT, '')::NUMERIC,
        'source_row_fingerprint', 'high', false, false
    )
    ON CONFLICT (org_id, source_key) DO UPDATE SET
        company = EXCLUDED.company,
        name = EXCLUDED.name,
        source_filename = EXCLUDED.source_filename,
        source_sheet = EXCLUDED.source_sheet,
        source_row = EXCLUDED.source_row,
        raw_row = EXCLUDED.raw_row,
        source_metadata = EXCLUDED.source_metadata,
        employee_number = EXCLUDED.employee_number,
        org_unit = EXCLUDED.org_unit,
        job = EXCLUDED.job,
        position = EXCLUDED.position,
        worksite_name = EXCLUDED.worksite_name,
        worksite_address = EXCLUDED.worksite_address,
        hire_date = EXCLUDED.hire_date,
        exit_date = EXCLUDED.exit_date,
        employment_status = EXCLUDED.employment_status,
        leave_accrued = EXCLUDED.leave_accrued,
        leave_used = EXCLUDED.leave_used,
        leave_remaining = EXCLUDED.leave_remaining,
        identity_resolution_strategy = EXCLUDED.identity_resolution_strategy,
        identity_resolution_confidence = EXCLUDED.identity_resolution_confidence,
        identity_review_required = EXCLUDED.identity_review_required,
        identity_name_only_merge = EXCLUDED.identity_name_only_merge,
        updated_at = now()
    RETURNING CASE WHEN xmax = 0 THEN 'inserted' ELSE 'updated' END
"#;

const F6FF_LEAVE_CREATE_SQL: &str = r#"
    INSERT INTO leave_requests
        (id, org_id, branch_id, requester_user_id, subject_employee_id, leave_type, days,
         start_date, end_date, reason, status)
    VALUES ($1, $2, $3, $4, $5, 'annual', 1, DATE '2026-12-01', DATE '2026-12-01',
            'legacy audit contract', 'pending')
"#;

const F6FF_LEAVE_CREATE_AUDIT_SQL: &str = r#"
    INSERT INTO audit_events
        (actor, action, target_type, target_id, branch_id, before_snap, after_snap,
         trace_id, span_id, occurred_at, org_id)
    VALUES ($1, 'leave_request.create', 'leave_request', $2, $3, NULL,
            '{"status":"pending"}'::jsonb,
            'abababababababababababababababab', 'cdcdcdcdcdcdcdcd', now(), $4)
"#;

const F6FF_LEAVE_DECIDE_SQL: &str = r#"
    UPDATE leave_requests
       SET status = 'returned', decided_by = $2, decided_at = now(),
           decision_comment = 'legacy audit contract'
     WHERE id = $1 AND status = 'pending'
"#;

const F6FF_LEAVE_DECIDE_AUDIT_SQL: &str = r#"
    INSERT INTO audit_events
        (actor, action, target_type, target_id, branch_id, before_snap, after_snap,
         trace_id, span_id, occurred_at, org_id)
    VALUES ($1, 'leave_request.decide', 'leave_request', $2, $3,
            '{"status":"pending"}'::jsonb, '{"status":"returned"}'::jsonb,
            'efefefefefefefefefefefefefefefef', '1212121212121212', now(), $4)
"#;

async fn restore_pre_0166_schema(pool: &PgPool) {
    sqlx::raw_sql(
        r#"
        DROP SCHEMA leave_api CASCADE;
        DROP TABLE leave_balance_import_receipts CASCADE;
        DROP TABLE leave_charge_resolutions CASCADE;
        DROP FUNCTION leave_charge_resolutions_immutable();
        DROP FUNCTION leave_requests_intent_routing_immutable() CASCADE;
        DROP FUNCTION leave_requests_charge_pointer_consistent() CASCADE;

        DROP INDEX employees_org_home_branch_idx;
        ALTER TABLE employees
            DROP CONSTRAINT employees_home_branch_same_org_fk,
            DROP CONSTRAINT employees_id_org_id_unique,
            DROP COLUMN home_branch_id,
            ALTER COLUMN leave_accrued TYPE NUMERIC(10,2),
            ALTER COLUMN leave_used TYPE NUMERIC(10,2),
            ALTER COLUMN leave_remaining TYPE NUMERIC(10,2);

        ALTER TABLE leave_requests
            DROP CONSTRAINT IF EXISTS leave_requests_current_charge_resolution_fk,
            DROP CONSTRAINT leave_requests_submission_pair,
            DROP CONSTRAINT leave_requests_status_charge_state,
            DROP CONSTRAINT leave_requests_charge_review_reason_values,
            DROP CONSTRAINT leave_requests_partial_day_shape,
            DROP CONSTRAINT leave_requests_charge_shape,
            DROP CONSTRAINT leave_requests_org_id_id_unique,
            DROP COLUMN current_charge_resolution_id,
            DROP COLUMN charge_version,
            DROP COLUMN request_version,
            DROP COLUMN charge_units,
            DROP COLUMN submission_initial_charge_version,
            DROP COLUMN submission_digest,
            DROP COLUMN submission_key,
            DROP COLUMN charge_review_reasons,
            DROP COLUMN charge_state,
            DROP COLUMN partial_day_period,
            DROP COLUMN legacy_days,
            ALTER COLUMN days TYPE NUMERIC(4,1),
            ALTER COLUMN days SET NOT NULL;

        ALTER ROLE console_app LOGIN INHERIT NOSUPERUSER BYPASSRLS NOCREATEDB NOCREATEROLE
            NOREPLICATION PASSWORD 'leave-migration-owner-a166';
        ALTER ROLE console_leave_definer NOLOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT
            NOCREATEDB NOCREATEROLE NOREPLICATION;
        ALTER ROLE console_leave_cmd LOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT
            NOCREATEDB NOCREATEROLE NOREPLICATION;
        REVOKE console_leave_definer FROM console_rt, console_leave_cmd;
        REVOKE console_rt, console_leave_cmd FROM console_app, console_leave_definer;
        GRANT console_leave_definer TO console_app
            WITH ADMIN FALSE, INHERIT TRUE, SET TRUE;

        DO $db_owner$
        BEGIN
            EXECUTE format('ALTER DATABASE %I OWNER TO console_app', current_database());
        END
        $db_owner$;

        ALTER TABLE organizations OWNER TO console_app;
        ALTER TABLE users OWNER TO console_app;
        ALTER TABLE user_branches OWNER TO console_app;
        ALTER TABLE regions OWNER TO console_app;
        ALTER TABLE branches OWNER TO console_app;
        ALTER TABLE employees OWNER TO console_app;
        ALTER TABLE leave_requests OWNER TO console_app;
        ALTER TABLE data_import_runs OWNER TO console_app;
        ALTER TABLE data_import_rows OWNER TO console_app;
        ALTER TABLE audit_events OWNER TO console_app;
        ALTER TABLE policy_roles OWNER TO console_app;
        ALTER TABLE policy_role_permissions OWNER TO console_app;
        ALTER TABLE policy_role_conditions OWNER TO console_app;
        ALTER TABLE user_role_assignments OWNER TO console_app;

        REVOKE ALL ON leave_requests FROM PUBLIC, console_rt, console_leave_cmd, console_leave_definer;
        GRANT SELECT, INSERT, UPDATE ON leave_requests TO console_rt;
        "#,
    )
    .execute(pool)
    .await
    .expect("the migrated fixture must be reducible to the populated pre-0166 shape");
}

async fn login_role_pool(owner_pool: &PgPool, role: &str, password: &str) -> PgPool {
    let options = owner_pool
        .connect_options()
        .as_ref()
        .clone()
        .username(role)
        .password(password);
    PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap()
}

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _metadata| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn command_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _metadata| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_leave_cmd")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn definer_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _metadata| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_leave_definer")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn seed_pre_0166_data(pool: &PgPool) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, 'leave-a166', 'Leave A166')",
    )
    .bind(ORG)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO regions (id, name, org_id) VALUES ($1, 'Region A166', $2)")
        .bind(REGION)
        .bind(ORG)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO branches (id, region_id, name, org_id) VALUES ($1, $2, 'Branch A166', $3)",
    )
    .bind(BRANCH)
    .bind(REGION)
    .bind(ORG)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, org_id, is_active) \
         VALUES ($1, 'User A166', ARRAY['ADMIN']::text[], $2, true)",
    )
    .bind(USER)
    .bind(ORG)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, org_id, is_active) \
         VALUES ($1, 'Decider A166', ARRAY['ADMIN']::text[], $2, true)",
    )
    .bind(DECIDER)
    .bind(ORG)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(DECIDER)
        .bind(BRANCH)
        .bind(ORG)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO employees \
         (id, org_id, company, name, source_filename, source_sheet, source_row, source_key, \
          hire_date, employment_status, leave_accrued, leave_used, leave_remaining) \
         VALUES ($1, $2, 'A166', 'Employee A166', 'a166.xlsx', 'Sheet1', 1, 'a166-employee', \
                 DATE '2020-01-01', 'ACTIVE', 15, 2, 13)",
    )
    .bind(EMPLOYEE)
    .bind(ORG)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE users SET employee_id = $2 WHERE id = $1 AND org_id = $3")
        .bind(USER)
        .bind(EMPLOYEE)
        .bind(ORG)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO leave_requests \
         (id, org_id, branch_id, requester_user_id, subject_employee_id, leave_type, days, \
          start_date, end_date, reason, status) \
         VALUES ($1, $2, $3, $4, $5, 'annual', 2.5, DATE '2026-08-03', \
                 DATE '2026-08-05', 'pre-0166 populated request', 'pending')",
    )
    .bind(PREEXISTING_REQUEST)
    .bind(ORG)
    .bind(BRANCH)
    .bind(USER)
    .bind(EMPLOYEE)
    .execute(pool)
    .await
    .unwrap();
}

async fn migrate_populated_pre_0166(owner_pool: &PgPool) {
    restore_pre_0166_schema(owner_pool).await;
    seed_pre_0166_data(owner_pool).await;

    apply_0166_with_rehearsal_budgets(owner_pool).await;
}

async fn apply_0166_with_rehearsal_budgets(owner_pool: &PgPool) -> (String, String) {
    let migrator = login_role_pool(owner_pool, "console_app", MIGRATOR_PASSWORD).await;
    let mut migration = migrator.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '5s'")
        .execute(&mut *migration)
        .await
        .unwrap();
    sqlx::query("SET LOCAL statement_timeout = '60s'")
        .execute(&mut *migration)
        .await
        .unwrap();
    let budgets: (String, String) = sqlx::query_as(
        "SELECT current_setting('lock_timeout'), current_setting('statement_timeout')",
    )
    .fetch_one(&mut *migration)
    .await
    .unwrap();
    sqlx::raw_sql(MIGRATION_0166)
        .execute(&mut *migration)
        .await
        .expect("the exact shipped 0166 migration must run within the rehearsal budgets");
    migration.commit().await.unwrap();
    budgets
}

fn database_error_code(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()?
        .code()
        .map(|code| code.into_owned())
}

fn f6ff_apply_after_snap(run_id: Uuid) -> serde_json::Value {
    let gate_outcome = serde_json::to_value(evaluate_gate_chain(
        GateChainConfig {
            self_checklist: true,
            ..GateChainConfig::default()
        },
        &GateEvidence {
            checklist_all_acknowledged: Some(true),
            ..GateEvidence::default()
        },
    ))
    .unwrap();
    assert_eq!(
        gate_outcome,
        serde_json::json!({
            "gates": [
                {"gate": "authority", "status": {"status": "not_required"}},
                {"gate": "self_checklist", "status": {"status": "satisfied"}},
                {"gate": "four_eyes", "status": {"status": "not_required"}},
                {"gate": "egress_dlp", "status": {"status": "not_required"}}
            ],
            "allow": true
        }),
        "the legacy audit envelope must track the actual GateOutcome serde shape"
    );
    serde_json::json!({
        "run_id": run_id,
        "entity_type": "employee_hr",
        "gate_outcome": gate_outcome
    })
}

async fn migrate_staged_f6ff_employee_import(owner_pool: &PgPool) {
    restore_pre_0166_schema(owner_pool).await;
    seed_pre_0166_data(owner_pool).await;
    sqlx::query(
        "INSERT INTO data_import_runs \
         (id,org_id,entity_type,status,source_filename,source_format,source_sha256, \
          input_rows,candidate_rows,preserved_rows,created_by,dry_run_summary, pay_period_start, pay_period_end) \
         VALUES ($1,$2,'employee_hr','DRY_RUN','staged.xlsx','xlsx',repeat('a',64), \
                 1,1,0,$3,'{\"ready_rows\":1}'::jsonb, DATE '2026-06-01', DATE '2026-06-30')",
    )
    .bind(STAGED_IMPORT_RUN)
    .bind(ORG)
    .bind(USER)
    .execute(owner_pool)
    .await
    .unwrap();

    let migrator = login_role_pool(owner_pool, "console_app", MIGRATOR_PASSWORD).await;
    sqlx::raw_sql(MIGRATION_0166)
        .execute(&migrator)
        .await
        .expect("0166 must migrate while an f6ff employee import is staged for apply");
}

async fn assert_staged_import_rolled_back(owner_pool: &PgPool) {
    let state: (
        String,
        String,
        String,
        String,
        serde_json::Value,
        Option<Uuid>,
        bool,
        i64,
    ) = sqlx::query_as(
        "SELECT e.leave_accrued::text,e.leave_used::text,e.leave_remaining::text, \
                    r.status,r.apply_summary,r.applied_by,r.applied_at IS NULL, \
                    (SELECT count(*) FROM audit_events a WHERE a.org_id=$1 \
                      AND a.action='data_import.apply' AND a.target_id=$3::text) \
             FROM employees e JOIN data_import_runs r ON r.org_id=e.org_id \
             WHERE e.org_id=$1 AND e.id=$2 AND r.id=$3",
    )
    .bind(ORG)
    .bind(EMPLOYEE)
    .bind(STAGED_IMPORT_RUN)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    assert_eq!(state.0, "15.000000");
    assert_eq!(state.1, "2.000000");
    assert_eq!(state.2, "13.000000");
    assert_eq!(state.3, "DRY_RUN");
    assert_eq!(state.4, serde_json::json!({}));
    assert_eq!(state.5, None);
    assert!(state.6);
    assert_eq!(state.7, 0);
}

async fn stage_f6ff_employee_import_apply(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: Uuid,
) {
    sqlx::query_scalar::<_, String>(F6FF_EMPLOYEE_UPSERT_SQL)
        .bind(ORG)
        .bind("a166-employee")
        .bind("Employee A166 staged")
        .bind("25")
        .bind("4")
        .bind("21")
        .fetch_one(&mut **tx)
        .await
        .expect("the legacy employee balance mutation must be staged");
    sqlx::query(
        "UPDATE data_import_runs \
         SET status='APPLIED',apply_summary=$3,applied_by=$4,applied_at=now(),updated_at=now() \
         WHERE org_id=$1 AND id=$2",
    )
    .bind(ORG)
    .bind(STAGED_IMPORT_RUN)
    .bind(serde_json::json!({"input_rows": 1, "inserted": 0, "updated": 1}))
    .bind(actor)
    .execute(&mut **tx)
    .await
    .expect("the legacy APPLIED metadata mutation must be staged");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn populated_upgrade_preserves_pre_0166_read_and_insert_contract(owner_pool: PgPool) {
    migrate_populated_pre_0166(&owner_pool).await;

    let preexisting = sqlx::query(
        "SELECT days::text AS days, legacy_days::text AS legacy_days, charge_state \
         FROM leave_requests WHERE id = $1",
    )
    .bind(PREEXISTING_REQUEST)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(preexisting.get::<String, _>("days"), "2.500000");
    assert_eq!(preexisting.get::<String, _>("legacy_days"), "2.500000");
    assert_eq!(
        preexisting.get::<String, _>("charge_state"),
        "review_required"
    );

    let runtime = runtime_role_pool(&owner_pool).await;
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(ORG.to_string())
        .execute(&runtime)
        .await
        .unwrap();

    let protected_home_branch_insert = sqlx::query(
        "INSERT INTO employees \
         (org_id, company, name, source_filename, source_sheet, source_row, source_key, \
          hire_date, employment_status, leave_accrued, leave_used, leave_remaining,home_branch_id) \
         VALUES ($1, 'forged', 'forged', 'forged.xlsx', 'Sheet1', 9, 'forged-a166', \
                 DATE '2024-01-01', 'ACTIVE', 15, 0, 15,$2)",
    )
    .bind(ORG)
    .bind(BRANCH)
    .execute(&runtime)
    .await
    .expect_err("the additive home-branch authority must remain command-only");
    assert_eq!(
        database_error_code(&protected_home_branch_insert).as_deref(),
        Some("42501")
    );

    // This is the SQL shape issued by the pre-0166 binary after an application
    // rollback: it knows only `days`, not any exact-charge columns.
    let mut legacy_create = runtime.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO leave_requests \
         (id, org_id, branch_id, requester_user_id, subject_employee_id, leave_type, days, \
          start_date, end_date, reason, status) \
         VALUES ($1, $2, $3, $4, $5, 'half_day', 0.5, DATE '2026-09-01', \
                 DATE '2026-09-01', 'old binary rollback request', 'pending')",
    )
    .bind(ROLLBACK_REQUEST)
    .bind(ORG)
    .bind(BRANCH)
    .bind(USER)
    .bind(EMPLOYEE)
    .execute(&mut *legacy_create)
    .await
    .expect("the pre-0166 INSERT contract must remain usable after 0166");
    sqlx::query(F6FF_LEAVE_CREATE_AUDIT_SQL)
        .bind(USER)
        .bind(ROLLBACK_REQUEST.to_string())
        .bind(BRANCH)
        .bind(ORG)
        .execute(&mut *legacy_create)
        .await
        .expect("the base adapter's atomic create audit must survive the expand migration");
    legacy_create.commit().await.unwrap();

    let rollback_row = sqlx::query(
        "SELECT days::text AS days, legacy_days::text AS legacy_days, charge_state, \
                charge_review_reasons, charge_units::text AS charge_units \
         FROM leave_requests WHERE id = $1",
    )
    .bind(ROLLBACK_REQUEST)
    .fetch_one(&runtime)
    .await
    .unwrap();
    assert_eq!(rollback_row.get::<String, _>("days"), "0.500000");
    assert_eq!(rollback_row.get::<String, _>("legacy_days"), "0.500000");
    assert_eq!(
        rollback_row.get::<String, _>("charge_state"),
        "review_required"
    );
    assert_eq!(
        rollback_row.get::<Vec<String>, _>("charge_review_reasons"),
        vec!["missing_calendar"]
    );
    assert!(
        rollback_row
            .get::<Option<String>, _>("charge_units")
            .is_none()
    );

    // Exact base-adapter decision SQL: the old binary has no request-version
    // parameter, and writes the audit receipt in this same transaction.
    let mut legacy_decide = runtime.begin().await.unwrap();
    let old_shape = sqlx::query(
        "UPDATE leave_requests \
         SET status = 'returned', decided_by = $2, decided_at = now(), \
             decision_comment = 'needs correction' \
         WHERE id = $1 AND status = 'pending' \
         RETURNING days::float8 AS days, status, request_version",
    )
    .bind(ROLLBACK_REQUEST)
    .bind(DECIDER)
    .fetch_one(&mut *legacy_decide)
    .await
    .expect("the base adapter's decision update must survive the expand migration");
    assert_eq!(old_shape.get::<f64, _>("days"), 0.5);
    assert_eq!(old_shape.get::<String, _>("status"), "returned");
    assert_eq!(old_shape.get::<i64, _>("request_version"), 2);
    sqlx::query(
        "INSERT INTO audit_events \
         (actor, action, target_type, target_id, branch_id, before_snap, after_snap, \
          trace_id, span_id, occurred_at, org_id) \
         VALUES ($1, 'leave_request.decide', 'leave_request', $2, $3, \
                 '{\"status\":\"pending\"}'::jsonb, '{\"status\":\"returned\"}'::jsonb, \
                 '11111111111111111111111111111111', '2222222222222222', now(), $4)",
    )
    .bind(DECIDER)
    .bind(ROLLBACK_REQUEST.to_string())
    .bind(BRANCH)
    .bind(ORG)
    .execute(&mut *legacy_decide)
    .await
    .expect("the base adapter's atomic audit receipt must survive the expand migration");
    legacy_decide.commit().await.unwrap();

    let second_decision_error = sqlx::query(
        "UPDATE leave_requests SET status = 'rejected', decided_by = $2, decided_at = now(), \
         decision_comment = 'second writer' WHERE id = $1",
    )
    .bind(ROLLBACK_REQUEST)
    .bind(DECIDER)
    .execute(&runtime)
    .await
    .expect_err("a legacy second writer must not re-decide a terminal request");
    assert_eq!(
        database_error_code(&second_decision_error).as_deref(),
        Some("42501")
    );

    let legacy_approval_request = Uuid::new_v4();
    let mut legacy_approval_create = runtime.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO leave_requests \
         (id,org_id,branch_id,requester_user_id,subject_employee_id,leave_type,days, \
          start_date,end_date,reason,status) \
         VALUES ($1,$2,$3,$4,$5,'annual',1,DATE '2026-09-03',DATE '2026-09-03', \
                 'old binary approval','pending')",
    )
    .bind(legacy_approval_request)
    .bind(ORG)
    .bind(BRANCH)
    .bind(USER)
    .bind(EMPLOYEE)
    .execute(&mut *legacy_approval_create)
    .await
    .unwrap();
    sqlx::query(F6FF_LEAVE_CREATE_AUDIT_SQL)
        .bind(USER)
        .bind(legacy_approval_request.to_string())
        .bind(BRANCH)
        .bind(ORG)
        .execute(&mut *legacy_approval_create)
        .await
        .unwrap();
    legacy_approval_create.commit().await.unwrap();
    let mut legacy_approval = runtime.begin().await.unwrap();
    sqlx::query(
        "UPDATE leave_requests SET status='approved',decided_by=$2,decided_at=now() \
         WHERE id=$1 AND status='pending'",
    )
    .bind(legacy_approval_request)
    .bind(DECIDER)
    .execute(&mut *legacy_approval)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE employees SET leave_used=COALESCE(leave_used,0)+1, \
         leave_remaining=COALESCE(leave_remaining,0)-1,updated_at=now() \
         WHERE id=$1 AND COALESCE(leave_remaining,0)>=1",
    )
    .bind(EMPLOYEE)
    .execute(&mut *legacy_approval)
    .await
    .expect("the rollback binary's exact same-transaction approval ledger write must survive");
    sqlx::query(
        "INSERT INTO audit_events \
         (actor,action,target_type,target_id,branch_id,before_snap,after_snap, \
          trace_id,span_id,occurred_at,org_id) \
         VALUES ($1,'leave_request.decide','leave_request',$2,$3, \
                 '{\"status\":\"pending\"}'::jsonb,'{\"status\":\"approved\"}'::jsonb, \
                 '12121212121212121212121212121212','3434343434343434',now(),$4)",
    )
    .bind(DECIDER)
    .bind(legacy_approval_request.to_string())
    .bind(BRANCH)
    .bind(ORG)
    .execute(&mut *legacy_approval)
    .await
    .unwrap();
    legacy_approval.commit().await.unwrap();
    let legacy_ledger: (String, String) =
        sqlx::query_as("SELECT leave_used::text,leave_remaining::text FROM employees WHERE id=$1")
            .bind(EMPLOYEE)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(
        legacy_ledger,
        ("3.000000".to_owned(), "12.000000".to_owned())
    );

    let forged_home_branch = sqlx::query("UPDATE employees SET home_branch_id=$2 WHERE id=$1")
        .bind(EMPLOYEE)
        .bind(BRANCH)
        .execute(&runtime)
        .await
        .expect_err("legacy import compatibility must not expose home-branch authority");
    assert_eq!(
        database_error_code(&forged_home_branch).as_deref(),
        Some("42501")
    );

    let forged_charge_error = sqlx::query(
        "INSERT INTO leave_requests \
         (org_id, branch_id, requester_user_id, subject_employee_id, leave_type, days, \
          start_date, end_date, reason, status, charge_state, charge_review_reasons, charge_units) \
         VALUES ($1, $2, $3, $4, 'annual', 1, DATE '2026-09-02', DATE '2026-09-02', \
                 'forged exact charge', 'pending', 'resolved', ARRAY[]::text[], 1)",
    )
    .bind(ORG)
    .bind(BRANCH)
    .bind(USER)
    .bind(EMPLOYEE)
    .execute(&runtime)
    .await
    .expect_err("legacy INSERT compatibility must not expose exact-charge authority");
    assert_eq!(
        database_error_code(&forged_charge_error).as_deref(),
        Some("42501")
    );

    // Versionless v1 command requests are first-writer-wins under the row lock;
    // modern callers still receive an exact stale-version conflict.
    // NOTE: this test surgically restores pre-0166 + reapplies 0166 only, so
    // leave_api.decide_request is the 0166 signature (approve|return|reject) —
    // not the post-0216/0217 §60 surface. Keep reject here; §60 mechanics are
    // covered by leave_rls / personas on the full migrator tip.
    let v1_request = Uuid::new_v4();
    let modern_request = Uuid::new_v4();
    let mut legacy_command_fixtures = runtime.begin().await.unwrap();
    for (id, reason) in [
        (v1_request, "v1 versionless decision"),
        (modern_request, "modern stale decision"),
    ] {
        sqlx::query(
            "INSERT INTO leave_requests \
             (id, org_id, branch_id, requester_user_id, subject_employee_id, leave_type, days, \
              start_date, end_date, reason, status) \
             VALUES ($1, $2, $3, $4, $5, 'annual', 1, DATE '2026-10-01', \
                     DATE '2026-10-01', $6, 'pending')",
        )
        .bind(id)
        .bind(ORG)
        .bind(BRANCH)
        .bind(USER)
        .bind(EMPLOYEE)
        .bind(reason)
        .execute(&mut *legacy_command_fixtures)
        .await
        .unwrap();
        sqlx::query(F6FF_LEAVE_CREATE_AUDIT_SQL)
            .bind(USER)
            .bind(id.to_string())
            .bind(BRANCH)
            .bind(ORG)
            .execute(&mut *legacy_command_fixtures)
            .await
            .unwrap();
    }
    legacy_command_fixtures.commit().await.unwrap();

    let command = command_role_pool(&owner_pool).await;
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(ORG.to_string())
        .execute(&command)
        .await
        .unwrap();
    let v1_outcome: String = sqlx::query_scalar(
        "SELECT outcome FROM leave_api.decide_request( \
             $1, $2, $3, NULL, 'reject', 'v1 reject', \
             '33333333333333333333333333333333', '4444444444444444')",
    )
    .bind(ORG)
    .bind(v1_request)
    .bind(DECIDER)
    .fetch_one(&command)
    .await
    .expect("a missing v1 expected_version must use locked first-writer-wins semantics");
    assert_eq!(v1_outcome, "decided");

    let repeated_v1 = sqlx::query(
        "SELECT * FROM leave_api.decide_request( \
             $1, $2, $3, NULL, 'reject', 'repeated', \
             '55555555555555555555555555555555', '6666666666666666')",
    )
    .bind(ORG)
    .bind(v1_request)
    .bind(DECIDER)
    .fetch_one(&command)
    .await
    .expect_err("a second versionless writer must lose after the first terminal transition");
    assert_eq!(database_error_code(&repeated_v1).as_deref(), Some("P0001"));

    let stale_modern = sqlx::query(
        "SELECT * FROM leave_api.decide_request( \
             $1, $2, $3, 99, 'reject', 'stale', \
             '77777777777777777777777777777777', '8888888888888888')",
    )
    .bind(ORG)
    .bind(modern_request)
    .bind(DECIDER)
    .fetch_one(&command)
    .await
    .expect_err("a modern stale expected_version must still fail exact CAS");
    assert_eq!(database_error_code(&stale_modern).as_deref(), Some("40001"));

    // A resolved create uses an intermediate valid review-required row, then
    // installs its immutable evidence pointer in the same transaction. Approval
    // increments request CAS only; the pointed charge evidence version remains 1.
    let definer = definer_role_pool(&owner_pool).await;
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(ORG.to_string())
        .execute(&definer)
        .await
        .unwrap();
    sqlx::query("UPDATE employees SET home_branch_id = $2 WHERE id = $1 AND org_id = $3")
        .bind(EMPLOYEE)
        .bind(BRANCH)
        .bind(ORG)
        .execute(&definer)
        .await
        .unwrap();

    let resolved_request = Uuid::new_v4();
    let submission_key = Uuid::new_v4();
    let created = sqlx::query(
        r#"
        SELECT request_id, request_version, charge_version, charge_units::text AS charge_units
        FROM leave_api.create_request(
            $1, $2, $3, 'annual', DATE '2026-11-03', DATE '2026-11-03',
            'resolved create', NULL, ARRAY[]::text[], $4,
            '[{"date":"2026-11-03","obligation":{"kind":"scheduled","minutes":480},"units":"1"}]'::jsonb,
            '{"kind":"calendar","reference":"calendar-a166","revision":"1"}'::jsonb,
            '{"kind":"policy","reference":"policy-a166","revision":"1"}'::jsonb,
            '[]'::jsonb,
            $5, '99999999999999999999999999999999', 'aaaaaaaaaaaaaaaa'
        )
        "#,
    )
    .bind(ORG)
    .bind(resolved_request)
    .bind(USER)
    .bind(BRANCH)
    .bind(submission_key)
    .fetch_one(&command)
    .await
    .expect("resolved create must satisfy every immediate row CHECK and install its pointer");
    assert_eq!(created.get::<Uuid, _>("request_id"), resolved_request);
    assert_eq!(created.get::<i64, _>("request_version"), 1);
    assert_eq!(created.get::<i64, _>("charge_version"), 1);
    assert_eq!(created.get::<String, _>("charge_units"), "1.000000");

    // Simulate a committed response that the client never received. Retrying
    // the same canonical payload under the same stable key returns the first
    // request even after mutable routing/evidence context changes, and appends
    // neither a second request nor a second audit event.
    sqlx::query("UPDATE employees SET home_branch_id=NULL WHERE org_id=$1 AND id=$2")
        .bind(ORG)
        .bind(EMPLOYEE)
        .execute(&definer)
        .await
        .unwrap();
    let retry_request = Uuid::new_v4();
    let changed_evidence_branch = Uuid::new_v4();
    let retried_request_id: Uuid = sqlx::query_scalar(
        r#"
        SELECT request_id FROM leave_api.create_request(
            $1, $2, $3, 'annual', DATE '2026-11-03', DATE '2026-11-03',
            'resolved create', NULL, ARRAY[]::text[], $4,
            '{"calendar_state":"changed_after_commit"}'::jsonb,
            '{"revision":"replaced"}'::jsonb,
            '{"revision":"retired"}'::jsonb,
            '[{"source":"changed"}]'::jsonb,
            $5, 'dddddddddddddddddddddddddddddddd', 'eeeeeeeeeeeeeeee'
        )
        "#,
    )
    .bind(ORG)
    .bind(retry_request)
    .bind(USER)
    .bind(changed_evidence_branch)
    .bind(submission_key)
    .fetch_one(&command)
    .await
    .expect("a response-loss retry must return the originally committed request");
    assert_eq!(retried_request_id, resolved_request);

    let idempotency_evidence: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM leave_requests \
           WHERE org_id=$1 AND requester_user_id=$2 AND submission_key=$3), \
          (SELECT count(*) FROM audit_events \
           WHERE org_id=$1 AND action='leave_request.create' AND target_id=$4)",
    )
    .bind(ORG)
    .bind(USER)
    .bind(submission_key)
    .bind(resolved_request.to_string())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(idempotency_evidence, (1, 1));

    let idempotency_conflict = sqlx::query(
        r#"
        SELECT * FROM leave_api.create_request(
            $1, $2, $3, 'annual', DATE '2026-11-03', DATE '2026-11-03',
            'changed payload', NULL, ARRAY[]::text[], $4,
            '[{"date":"2026-11-03","obligation":{"kind":"scheduled","minutes":480},"units":"1"}]'::jsonb,
            '{"kind":"calendar","reference":"calendar-a166","revision":"1"}'::jsonb,
            '{"kind":"policy","reference":"policy-a166","revision":"1"}'::jsonb,
            '[]'::jsonb,
            $5, 'ffffffffffffffffffffffffffffffff', '1111111111111111'
        )
        "#,
    )
    .bind(ORG)
    .bind(Uuid::new_v4())
    .bind(USER)
    .bind(BRANCH)
    .bind(submission_key)
    .fetch_one(&command)
    .await
    .expect_err("the same submission key must reject a different canonical payload");
    assert_eq!(
        database_error_code(&idempotency_conflict).as_deref(),
        Some("22023")
    );

    let pointer = sqlx::query(
        "SELECT lr.days::text AS days, lr.charge_state, lr.request_version, lr.charge_version, \
                r.charge_version AS resolution_version, \
                (lr.current_charge_resolution_id = r.id) AS exact_pointer \
         FROM leave_requests lr \
         JOIN leave_charge_resolutions r ON r.org_id = lr.org_id \
          AND r.request_id = lr.id AND r.id = lr.current_charge_resolution_id \
         WHERE lr.org_id = $1 AND lr.id = $2",
    )
    .bind(ORG)
    .bind(resolved_request)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(pointer.get::<String, _>("days"), "1.000000");
    assert_eq!(pointer.get::<String, _>("charge_state"), "resolved");
    assert_eq!(pointer.get::<i64, _>("request_version"), 1);
    assert_eq!(pointer.get::<i64, _>("charge_version"), 1);
    assert_eq!(pointer.get::<i64, _>("resolution_version"), 1);
    assert!(pointer.get::<bool, _>("exact_pointer"));

    let approval = sqlx::query(
        "SELECT request_version, charge_version, outcome \
         FROM leave_api.decide_request( \
             $1, $2, $3, 1, 'approve', NULL, \
             'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'cccccccccccccccc')",
    )
    .bind(ORG)
    .bind(resolved_request)
    .bind(DECIDER)
    .fetch_one(&command)
    .await
    .expect("normal approval must preserve the immutable charge evidence pointer/version");
    assert_eq!(approval.get::<i64, _>("request_version"), 2);
    assert_eq!(approval.get::<i64, _>("charge_version"), 1);
    assert_eq!(approval.get::<String, _>("outcome"), "decided");

    let approved_pointer = sqlx::query(
        "SELECT lr.request_version, lr.charge_version, r.charge_version AS resolution_version \
         FROM leave_requests lr JOIN leave_charge_resolutions r \
          ON r.org_id = lr.org_id AND r.id = lr.current_charge_resolution_id \
         WHERE lr.org_id = $1 AND lr.id = $2",
    )
    .bind(ORG)
    .bind(resolved_request)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(approved_pointer.get::<i64, _>("request_version"), 2);
    assert_eq!(approved_pointer.get::<i64, _>("charge_version"), 1);
    assert_eq!(approved_pointer.get::<i64, _>("resolution_version"), 1);

    // The SECURITY DEFINER import boundary independently re-evaluates the
    // effective org-wide PBAC grant. An EXECUTIVE custom allow is accepted
    // only while its role is active and every condition preserves All scope.
    let custom_actor = Uuid::new_v4();
    let custom_role = Uuid::new_v4();
    let custom_condition = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id,display_name,roles,team,is_active,org_id) \
         VALUES ($1,'Custom importer',ARRAY['EXECUTIVE']::text[],'정비',true,$2)",
    )
    .bind(custom_actor)
    .bind(ORG)
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO policy_roles (id,org_id,role_key,display_name,status,is_system) \
         VALUES ($1,$2,'leave_importer_a166','Leave importer A166','ACTIVE',false)",
    )
    .bind(custom_role)
    .bind(ORG)
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO policy_role_permissions \
         (org_id,role_id,feature_key,permission_level) \
         VALUES ($1,$2,'employee_directory_manage','allow')",
    )
    .bind(ORG)
    .bind(custom_role)
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_role_assignments (org_id,user_id,role_id) VALUES ($1,$2,$3)")
        .bind(ORG)
        .bind(custom_actor)
        .bind(custom_role)
        .execute(&owner_pool)
        .await
        .unwrap();
    sqlx::query("SELECT leave_api.assert_employee_importer($1,$2)")
        .bind(ORG)
        .bind(custom_actor)
        .execute(&definer)
        .await
        .expect("an active condition-free org-wide custom allow must be effective");

    sqlx::query(
        "INSERT INTO policy_role_conditions \
         (id,org_id,role_id,condition_key,attribute,operator,condition_values) \
         VALUES ($1,$2,$3,'team_match','team','equals',ARRAY['MAINTENANCE']::text[])",
    )
    .bind(custom_condition)
    .bind(ORG)
    .bind(custom_role)
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query("SELECT leave_api.assert_employee_importer($1,$2)")
        .bind(ORG)
        .bind(custom_actor)
        .execute(&definer)
        .await
        .expect("a matching team condition preserves the executive's All scope");

    sqlx::query(
        "UPDATE policy_role_conditions SET condition_values=ARRAY['PREVENTION']::text[] \
         WHERE id=$1",
    )
    .bind(custom_condition)
    .execute(&owner_pool)
    .await
    .unwrap();
    let mismatched_team = sqlx::query("SELECT leave_api.assert_employee_importer($1,$2)")
        .bind(ORG)
        .bind(custom_actor)
        .execute(&definer)
        .await
        .expect_err("a mismatched team condition must fail closed");
    assert_eq!(
        database_error_code(&mismatched_team).as_deref(),
        Some("42501")
    );

    sqlx::query(
        "UPDATE policy_role_conditions \
         SET attribute='branch',operator='equals',condition_values=ARRAY[$2::text] \
         WHERE id=$1",
    )
    .bind(custom_condition)
    .bind(BRANCH)
    .execute(&owner_pool)
    .await
    .unwrap();
    let narrowed_scope = sqlx::query("SELECT leave_api.assert_employee_importer($1,$2)")
        .bind(ORG)
        .bind(custom_actor)
        .execute(&definer)
        .await
        .expect_err("a branch condition narrows All and cannot authorize org-wide import");
    assert_eq!(
        database_error_code(&narrowed_scope).as_deref(),
        Some("42501")
    );

    sqlx::query("DELETE FROM policy_role_conditions WHERE id=$1")
        .bind(custom_condition)
        .execute(&owner_pool)
        .await
        .unwrap();
    sqlx::query("UPDATE policy_roles SET status='DRAFT' WHERE id=$1")
        .bind(custom_role)
        .execute(&owner_pool)
        .await
        .unwrap();
    let draft_role = sqlx::query("SELECT leave_api.assert_employee_importer($1,$2)")
        .bind(ORG)
        .bind(custom_actor)
        .execute(&definer)
        .await
        .expect_err("a DRAFT custom role must not grant import authority");
    assert_eq!(database_error_code(&draft_role).as_deref(), Some("42501"));
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn migration_0166_rehearses_populated_expand_with_bounded_lock_and_statement_timeouts(
    owner_pool: PgPool,
) {
    restore_pre_0166_schema(&owner_pool).await;
    seed_pre_0166_data(&owner_pool).await;

    let budgets = apply_0166_with_rehearsal_budgets(&owner_pool).await;
    assert_eq!(
        budgets,
        (
            MIGRATION_LOCK_TIMEOUT.into(),
            MIGRATION_STATEMENT_TIMEOUT.into()
        )
    );

    let migrated: (i64, String, String) = sqlx::query_as(
        "SELECT count(*), min(charge_state), min(legacy_days::text) \
         FROM leave_requests WHERE org_id=$1",
    )
    .bind(ORG)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(migrated, (1, "review_required".into(), "2.500000".into()));
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn exact_charge_create_accepts_resolved_and_review_required_shapes(owner_pool: PgPool) {
    migrate_populated_pre_0166(&owner_pool).await;
    let definer = definer_role_pool(&owner_pool).await;
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(ORG.to_string())
        .execute(&definer)
        .await
        .unwrap();
    sqlx::query("UPDATE employees SET home_branch_id=$2 WHERE org_id=$1 AND id=$3")
        .bind(ORG)
        .bind(BRANCH)
        .bind(EMPLOYEE)
        .execute(&definer)
        .await
        .unwrap();
    let command = command_role_pool(&owner_pool).await;
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(ORG.to_string())
        .execute(&command)
        .await
        .unwrap();

    let resolved_id = Uuid::new_v4();
    let resolved: (i64, i64, String) = sqlx::query_as(
        r#"SELECT request_version, charge_version, charge_units::text
           FROM leave_api.create_request(
             $1,$2,$3,'annual',DATE '2026-12-10',DATE '2026-12-10','resolved shape',
             NULL,ARRAY[]::text[],$4,
             '[{"date":"2026-12-10","obligation":{"kind":"scheduled","minutes":480},"units":"1"}]'::jsonb,
             '{"kind":"calendar","reference":"calendar-a166","revision":"1"}'::jsonb,
             '{"kind":"policy","reference":"policy-a166","revision":"1"}'::jsonb,
             '[]'::jsonb,$5,'10101010101010101010101010101010','1010101010101010')"#,
    )
    .bind(ORG)
    .bind(resolved_id)
    .bind(USER)
    .bind(BRANCH)
    .bind(Uuid::new_v4())
    .fetch_one(&command)
    .await
    .expect("complete evidence with no review reasons must create a resolved request");
    assert_eq!(resolved, (1, 1, "1.000000".into()));

    let review_id = Uuid::new_v4();
    let review: (i64, i64, Option<String>) = sqlx::query_as(
        r#"SELECT request_version, charge_version, charge_units::text
           FROM leave_api.create_request(
             $1,$2,$3,'annual',DATE '2026-12-11',DATE '2026-12-11','review shape',
             NULL,ARRAY['missing_calendar']::text[],NULL,NULL,NULL,NULL,NULL,
             $4,'20202020202020202020202020202020','2020202020202020')"#,
    )
    .bind(ORG)
    .bind(review_id)
    .bind(USER)
    .bind(Uuid::new_v4())
    .fetch_one(&command)
    .await
    .expect("review reasons with no evidence must create a review-required request");
    assert_eq!(review, (1, 0, None));

    let persisted: Vec<(Uuid, String, Vec<String>, i64)> = sqlx::query_as(
        "SELECT id,charge_state,charge_review_reasons,charge_version FROM leave_requests \
         WHERE id IN ($1,$2) ORDER BY id",
    )
    .bind(resolved_id)
    .bind(review_id)
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert!(
        persisted
            .iter()
            .any(|row| row == &(resolved_id, "resolved".into(), vec![], 1))
    );
    assert!(persisted.iter().any(|row| {
        row == &(
            review_id,
            "review_required".into(),
            vec!["missing_calendar".into()],
            0,
        )
    }));
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn exact_charge_create_atomically_rejects_mismatched_reason_and_evidence_shapes(
    owner_pool: PgPool,
) {
    migrate_populated_pre_0166(&owner_pool).await;
    let definer = definer_role_pool(&owner_pool).await;
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(ORG.to_string())
        .execute(&definer)
        .await
        .unwrap();
    sqlx::query("UPDATE employees SET home_branch_id=$2 WHERE org_id=$1 AND id=$3")
        .bind(ORG)
        .bind(BRANCH)
        .bind(EMPLOYEE)
        .execute(&definer)
        .await
        .unwrap();
    let command = command_role_pool(&owner_pool).await;
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(ORG.to_string())
        .execute(&command)
        .await
        .unwrap();

    let resolved_with_review_reason = Uuid::new_v4();
    let error = sqlx::query(
        r#"SELECT * FROM leave_api.create_request(
             $1,$2,$3,'annual',DATE '2026-12-12',DATE '2026-12-12','mismatched resolved shape',
             NULL,ARRAY['missing_calendar']::text[],$4,
             '[{"date":"2026-12-12","obligation":{"kind":"scheduled","minutes":480},"units":"1"}]'::jsonb,
             '{"kind":"calendar","reference":"calendar-a166","revision":"1"}'::jsonb,
             '{"kind":"policy","reference":"policy-a166","revision":"1"}'::jsonb,
             '[]'::jsonb,$5,'30303030303030303030303030303030','3030303030303030')"#,
    )
    .bind(ORG)
    .bind(resolved_with_review_reason)
    .bind(USER)
    .bind(BRANCH)
    .bind(Uuid::new_v4())
    .fetch_one(&command)
    .await
    .expect_err("resolved evidence must reject non-empty review reasons");
    assert_eq!(database_error_code(&error).as_deref(), Some("22023"));
    assert!(
        error
            .as_database_error()
            .is_some_and(|db| db.message() == "leave_create.invalid_charge_choice")
    );

    let review_with_evidence = Uuid::new_v4();
    let error = sqlx::query(
        r#"SELECT * FROM leave_api.create_request(
             $1,$2,$3,'annual',DATE '2026-12-13',DATE '2026-12-13','mismatched review shape',
             NULL,ARRAY['missing_calendar']::text[],$4,NULL,NULL,NULL,NULL,
             $5,'40404040404040404040404040404040','4040404040404040')"#,
    )
    .bind(ORG)
    .bind(review_with_evidence)
    .bind(USER)
    .bind(BRANCH)
    .bind(Uuid::new_v4())
    .fetch_one(&command)
    .await
    .expect_err("review-required requests must reject a partial evidence envelope");
    assert_eq!(database_error_code(&error).as_deref(), Some("22023"));
    assert!(
        error
            .as_database_error()
            .is_some_and(|db| db.message() == "leave_create.invalid_charge_choice")
    );

    let residue: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM leave_requests WHERE id IN ($1,$2)), \
                (SELECT count(*) FROM leave_charge_resolutions WHERE request_id IN ($1,$2)), \
                (SELECT count(*) FROM audit_events WHERE target_id IN ($3,$4))",
    )
    .bind(resolved_with_review_reason)
    .bind(review_with_evidence)
    .bind(resolved_with_review_reason.to_string())
    .bind(review_with_evidence.to_string())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        residue,
        (0, 0, 0),
        "rejected create statements must leave no row, charge evidence, or audit residue"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn immediate_f6ff_employee_import_remains_usable_after_0166(owner_pool: PgPool) {
    migrate_populated_pre_0166(&owner_pool).await;

    let runtime = runtime_role_pool(&owner_pool).await;
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(ORG.to_string())
        .execute(&runtime)
        .await
        .unwrap();

    // Exact SQL shape from f6ff236 backend/app/src/hr.rs::apply_employee_rows_tx.
    // The immediate endpoint has no data_import_run or audit envelope, so the
    // expand migration must preserve the protected balance write itself.
    let inserted: String = sqlx::query_scalar(F6FF_EMPLOYEE_UPSERT_SQL)
        .bind(ORG)
        .bind("a166-immediate")
        .bind("Immediate import")
        .bind("20")
        .bind("3")
        .bind("17")
        .fetch_one(&runtime)
        .await
        .expect("the f6ff immediate employee import must survive the expand migration");
    assert_eq!(inserted, "inserted");

    let updated: String = sqlx::query_scalar(F6FF_EMPLOYEE_UPSERT_SQL)
        .bind(ORG)
        .bind("a166-immediate")
        .bind("Immediate import updated")
        .bind("21")
        .bind("5")
        .bind("16")
        .fetch_one(&runtime)
        .await
        .expect("the f6ff immediate employee upsert update must survive the expand migration");
    assert_eq!(updated, "updated");

    let balances: (String, String, String) = sqlx::query_as(
        "SELECT leave_accrued::text, leave_used::text, leave_remaining::text \
         FROM employees WHERE org_id=$1 AND source_key='a166-immediate'",
    )
    .bind(ORG)
    .fetch_one(&runtime)
    .await
    .unwrap();
    assert_eq!(
        balances,
        (
            "21.000000".to_owned(),
            "5.000000".to_owned(),
            "16.000000".to_owned()
        )
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn staged_f6ff_employee_import_apply_remains_atomic_after_0166(owner_pool: PgPool) {
    migrate_staged_f6ff_employee_import(&owner_pool).await;

    let runtime = runtime_role_pool(&owner_pool).await;
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(ORG.to_string())
        .execute(&runtime)
        .await
        .unwrap();

    // Exact f6ff apply ordering: protected employee upserts, APPLIED metadata,
    // then the with_audit data_import.apply event, all in one transaction.
    let mut apply = runtime.begin().await.unwrap();
    let staged_outcome: String = sqlx::query_scalar(F6FF_EMPLOYEE_UPSERT_SQL)
        .bind(ORG)
        .bind("a166-employee")
        .bind("Employee A166 staged")
        .bind("25")
        .bind("4")
        .bind("21")
        .fetch_one(&mut *apply)
        .await
        .expect("the f6ff staged apply must retain its protected employee update");
    assert_eq!(staged_outcome, "updated");
    sqlx::query(
        "UPDATE data_import_runs \
         SET status='APPLIED',apply_summary=$3,applied_by=$4,applied_at=now(),updated_at=now() \
         WHERE org_id=$1 AND id=$2",
    )
    .bind(ORG)
    .bind(STAGED_IMPORT_RUN)
    .bind(serde_json::json!({"input_rows": 1, "inserted": 0, "updated": 1}))
    .bind(USER)
    .execute(&mut *apply)
    .await
    .expect("the f6ff staged apply must retain its APPLIED metadata transition");
    sqlx::query(F6FF_APPLY_AUDIT_SQL)
        .bind(USER)
        .bind(STAGED_IMPORT_RUN.to_string())
        .bind(f6ff_apply_after_snap(STAGED_IMPORT_RUN))
        .bind(ORG)
        .execute(&mut *apply)
        .await
        .expect("the f6ff with_audit data_import.apply event must survive the expand migration");
    apply.commit().await.unwrap();

    let evidence: (String, String, String, String, i64) = sqlx::query_as(
        "SELECT e.leave_accrued::text,e.leave_used::text,e.leave_remaining::text,r.status, \
                (SELECT count(*) FROM audit_events a WHERE a.org_id=$1 \
                  AND a.action='data_import.apply' AND a.target_id=$3::text) \
         FROM employees e JOIN data_import_runs r ON r.org_id=e.org_id \
         WHERE e.org_id=$1 AND e.id=$2 AND r.id=$3",
    )
    .bind(ORG)
    .bind(EMPLOYEE)
    .bind(STAGED_IMPORT_RUN)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        evidence,
        (
            "25.000000".to_owned(),
            "4.000000".to_owned(),
            "21.000000".to_owned(),
            "APPLIED".to_owned(),
            1
        )
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn staged_f6ff_apply_rejects_missing_duplicate_or_forged_current_tx_audit(
    owner_pool: PgPool,
) {
    migrate_staged_f6ff_employee_import(&owner_pool).await;

    let runtime = runtime_role_pool(&owner_pool).await;
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(ORG.to_string())
        .execute(&runtime)
        .await
        .unwrap();

    // A run transition without its same-transaction audit cannot commit, and
    // the deferred failure rolls the employee and run mutations back together.
    let mut missing = runtime.begin().await.unwrap();
    stage_f6ff_employee_import_apply(&mut missing, USER).await;
    let missing_error = missing
        .commit()
        .await
        .expect_err("an APPLIED run without an audit must fail at commit");
    assert_eq!(
        database_error_code(&missing_error).as_deref(),
        Some("23514")
    );
    assert_eq!(
        missing_error.as_database_error().unwrap().message(),
        "employee_import_run.current_transaction_audit_required"
    );
    assert_staged_import_rolled_back(&owner_pool).await;

    // Both audit inserts satisfy the immediate legacy envelope guard, but the
    // deferred exact-one invariant rejects duplicate current-transaction proof.
    let mut duplicate = runtime.begin().await.unwrap();
    stage_f6ff_employee_import_apply(&mut duplicate, USER).await;
    for _ in 0..2 {
        sqlx::query(F6FF_APPLY_AUDIT_SQL)
            .bind(USER)
            .bind(STAGED_IMPORT_RUN.to_string())
            .bind(f6ff_apply_after_snap(STAGED_IMPORT_RUN))
            .bind(ORG)
            .execute(&mut *duplicate)
            .await
            .expect("an exact legacy audit envelope reaches the deferred count check");
    }
    let duplicate_error = duplicate
        .commit()
        .await
        .expect_err("two matching audits must fail the exact-one invariant");
    assert_eq!(
        database_error_code(&duplicate_error).as_deref(),
        Some("23514")
    );
    assert_eq!(
        duplicate_error.as_database_error().unwrap().message(),
        "employee_import_run.current_transaction_audit_required"
    );
    assert_staged_import_rolled_back(&owner_pool).await;

    let mut extra = f6ff_apply_after_snap(STAGED_IMPORT_RUN);
    extra["unexpected"] = serde_json::json!(true);
    let mut forged = f6ff_apply_after_snap(STAGED_IMPORT_RUN);
    forged["gate_outcome"]["allow"] = serde_json::json!(false);
    let mut missing_gate = f6ff_apply_after_snap(STAGED_IMPORT_RUN);
    missing_gate["gate_outcome"]["gates"]
        .as_array_mut()
        .unwrap()
        .remove(1);
    let invalid_snapshots = [
        ("missing", None),
        ("extra", Some(extra)),
        ("forged", Some(forged)),
        ("missing gate", Some(missing_gate)),
    ];
    for (label, after_snap) in invalid_snapshots {
        let mut invalid = runtime.begin().await.unwrap();
        stage_f6ff_employee_import_apply(&mut invalid, USER).await;
        let error = sqlx::query(F6FF_APPLY_AUDIT_SQL)
            .bind(USER)
            .bind(STAGED_IMPORT_RUN.to_string())
            .bind(after_snap)
            .bind(ORG)
            .execute(&mut *invalid)
            .await
            .expect_err(&format!("a {label} f6ff snapshot must be rejected"));
        assert_eq!(database_error_code(&error).as_deref(), Some("42501"));
        invalid.rollback().await.unwrap();
        assert_staged_import_rolled_back(&owner_pool).await;
    }

    let mut classified = runtime.begin().await.unwrap();
    stage_f6ff_employee_import_apply(&mut classified, USER).await;
    let classified_error = sqlx::query(
        "INSERT INTO audit_events \
         (actor,action,target_type,target_id,before_snap,after_snap,classification_badges, \
          trace_id,span_id,occurred_at,org_id) \
         VALUES ($1,'data_import.apply','data_import_run',$2,NULL,$3,ARRAY['forged'], \
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','bbbbbbbbbbbbbbbb',now(),$4)",
    )
    .bind(USER)
    .bind(STAGED_IMPORT_RUN.to_string())
    .bind(f6ff_apply_after_snap(STAGED_IMPORT_RUN))
    .bind(ORG)
    .execute(&mut *classified)
    .await
    .expect_err("legacy apply cannot add classification context absent from f6ff");
    assert_eq!(
        database_error_code(&classified_error).as_deref(),
        Some("42501")
    );
    classified.rollback().await.unwrap();
    assert_staged_import_rolled_back(&owner_pool).await;

    let inactive_actor = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id,display_name,roles,is_active,org_id) \
         VALUES ($1,'Inactive actor',ARRAY['ADMIN']::text[],false,$2)",
    )
    .bind(inactive_actor)
    .bind(ORG)
    .execute(&owner_pool)
    .await
    .unwrap();
    let mut inactive = runtime.begin().await.unwrap();
    stage_f6ff_employee_import_apply(&mut inactive, inactive_actor).await;
    let inactive_error = sqlx::query(F6FF_APPLY_AUDIT_SQL)
        .bind(inactive_actor)
        .bind(STAGED_IMPORT_RUN.to_string())
        .bind(f6ff_apply_after_snap(STAGED_IMPORT_RUN))
        .bind(ORG)
        .execute(&mut *inactive)
        .await
        .expect_err("an inactive same-org actor cannot use the legacy audit exception");
    assert_eq!(
        database_error_code(&inactive_error).as_deref(),
        Some("42501")
    );
    inactive.rollback().await.unwrap();
    assert_staged_import_rolled_back(&owner_pool).await;

    // data_import_runs.applied_by historically references users(id), not the
    // composite tenant key. Even a definer-authored, exact same-transaction
    // audit cannot satisfy the deferred invariant for a cross-org actor.
    let foreign_org = Uuid::new_v4();
    let foreign_actor = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id,slug,name) VALUES ($1,$2,'Foreign org')")
        .bind(foreign_org)
        .bind(format!("foreign-{}", &foreign_org.to_string()[..8]))
        .execute(&owner_pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id,display_name,roles,is_active,org_id) \
         VALUES ($1,'Foreign actor',ARRAY['ADMIN']::text[],true,$2)",
    )
    .bind(foreign_actor)
    .bind(foreign_org)
    .execute(&owner_pool)
    .await
    .unwrap();

    let mut cross_org = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE console_leave_definer")
        .execute(&mut *cross_org)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(ORG.to_string())
        .execute(&mut *cross_org)
        .await
        .unwrap();
    stage_f6ff_employee_import_apply(&mut cross_org, foreign_actor).await;
    sqlx::query(F6FF_APPLY_AUDIT_SQL)
        .bind(foreign_actor)
        .bind(STAGED_IMPORT_RUN.to_string())
        .bind(f6ff_apply_after_snap(STAGED_IMPORT_RUN))
        .bind(ORG)
        .execute(&mut *cross_org)
        .await
        .expect("the definer path must reach the deferred tenant-correlation check");
    let cross_org_error = cross_org
        .commit()
        .await
        .expect_err("a cross-org applied_by must not satisfy the deferred audit invariant");
    assert_eq!(
        database_error_code(&cross_org_error).as_deref(),
        Some("23514")
    );
    assert_eq!(
        cross_org_error.as_database_error().unwrap().message(),
        "employee_import_run.current_transaction_audit_required"
    );
    assert_staged_import_rolled_back(&owner_pool).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn legacy_leave_mutations_require_exactly_one_same_transaction_audit(owner_pool: PgPool) {
    migrate_populated_pre_0166(&owner_pool).await;

    let runtime = runtime_role_pool(&owner_pool).await;
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(ORG.to_string())
        .execute(&runtime)
        .await
        .unwrap();

    let missing_create_request = Uuid::new_v4();
    let mut missing_create = runtime.begin().await.unwrap();
    sqlx::query(F6FF_LEAVE_CREATE_SQL)
        .bind(missing_create_request)
        .bind(ORG)
        .bind(BRANCH)
        .bind(USER)
        .bind(EMPLOYEE)
        .execute(&mut *missing_create)
        .await
        .expect("the guarded legacy create reaches the deferred audit invariant");
    let missing_create_error = missing_create
        .commit()
        .await
        .expect_err("a legacy create without an audit must fail at commit");
    assert_eq!(
        database_error_code(&missing_create_error).as_deref(),
        Some("23514")
    );
    assert_eq!(
        missing_create_error.as_database_error().unwrap().message(),
        "leave_request.current_transaction_audit_required"
    );
    let missing_create_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM leave_requests WHERE id=$1")
            .bind(missing_create_request)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(
        missing_create_count, 0,
        "the unaudited create must roll back"
    );

    let valid_request = Uuid::new_v4();
    let mut valid_create = runtime.begin().await.unwrap();
    sqlx::query(F6FF_LEAVE_CREATE_SQL)
        .bind(valid_request)
        .bind(ORG)
        .bind(BRANCH)
        .bind(USER)
        .bind(EMPLOYEE)
        .execute(&mut *valid_create)
        .await
        .unwrap();
    sqlx::query(F6FF_LEAVE_CREATE_AUDIT_SQL)
        .bind(USER)
        .bind(valid_request.to_string())
        .bind(BRANCH)
        .bind(ORG)
        .execute(&mut *valid_create)
        .await
        .expect("the exact f6ff create plus audit must remain compatible");
    valid_create.commit().await.unwrap();

    let delayed_audit = sqlx::query(F6FF_LEAVE_CREATE_AUDIT_SQL)
        .bind(USER)
        .bind(valid_request.to_string())
        .bind(BRANCH)
        .bind(ORG)
        .execute(&runtime)
        .await
        .expect_err("an audit added after the leave mutation committed must be rejected");
    assert_eq!(
        database_error_code(&delayed_audit).as_deref(),
        Some("42501")
    );

    let duplicate_request = Uuid::new_v4();
    let mut duplicate_create = runtime.begin().await.unwrap();
    sqlx::query(F6FF_LEAVE_CREATE_SQL)
        .bind(duplicate_request)
        .bind(ORG)
        .bind(BRANCH)
        .bind(USER)
        .bind(EMPLOYEE)
        .execute(&mut *duplicate_create)
        .await
        .unwrap();
    for _ in 0..2 {
        sqlx::query(F6FF_LEAVE_CREATE_AUDIT_SQL)
            .bind(USER)
            .bind(duplicate_request.to_string())
            .bind(BRANCH)
            .bind(ORG)
            .execute(&mut *duplicate_create)
            .await
            .expect("both matching audits reach the deferred exact-one invariant");
    }
    let duplicate_error = duplicate_create
        .commit()
        .await
        .expect_err("duplicate same-transaction audits must roll back the create");
    assert_eq!(
        database_error_code(&duplicate_error).as_deref(),
        Some("23514")
    );
    let duplicate_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM leave_requests WHERE id=$1")
            .bind(duplicate_request)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(duplicate_count, 0);

    let mut missing_decide = runtime.begin().await.unwrap();
    sqlx::query(F6FF_LEAVE_DECIDE_SQL)
        .bind(valid_request)
        .bind(DECIDER)
        .execute(&mut *missing_decide)
        .await
        .expect("the guarded legacy decision reaches the deferred audit invariant");
    let missing_decide_error = missing_decide
        .commit()
        .await
        .expect_err("a legacy decision without an audit must fail at commit");
    assert_eq!(
        database_error_code(&missing_decide_error).as_deref(),
        Some("23514")
    );
    let status_after_missing_decide: String =
        sqlx::query_scalar("SELECT status FROM leave_requests WHERE id=$1")
            .bind(valid_request)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(status_after_missing_decide, "pending");

    let mut valid_decide = runtime.begin().await.unwrap();
    sqlx::query(F6FF_LEAVE_DECIDE_SQL)
        .bind(valid_request)
        .bind(DECIDER)
        .execute(&mut *valid_decide)
        .await
        .unwrap();
    sqlx::query(F6FF_LEAVE_DECIDE_AUDIT_SQL)
        .bind(DECIDER)
        .bind(valid_request.to_string())
        .bind(BRANCH)
        .bind(ORG)
        .execute(&mut *valid_decide)
        .await
        .expect("the exact f6ff decision plus audit must remain compatible");
    valid_decide.commit().await.unwrap();

    let final_evidence: (String, i64, i64) = sqlx::query_as(
        "SELECT status,\
         (SELECT count(*) FROM audit_events WHERE org_id=$2 AND action='leave_request.create'\
          AND target_id=$1::text),\
         (SELECT count(*) FROM audit_events WHERE org_id=$2 AND action='leave_request.decide'\
          AND target_id=$1::text)\
         FROM leave_requests WHERE id=$1",
    )
    .bind(valid_request)
    .bind(ORG)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(final_evidence, ("returned".to_owned(), 1, 1));
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn staged_employee_import_rejects_payload_not_equal_to_immutable_ledger(owner_pool: PgPool) {
    migrate_populated_pre_0166(&owner_pool).await;
    sqlx::query("UPDATE users SET roles=ARRAY['SUPER_ADMIN']::text[] WHERE org_id=$1 AND id=$2")
        .bind(ORG)
        .bind(USER)
        .execute(&owner_pool)
        .await
        .unwrap();

    let run_id = Uuid::new_v4();
    let source_key = format!("bound-row-{run_id}");
    let raw_row = serde_json::json!({"company":"A166","name":"Ledger Employee"});
    let canonical_row = serde_json::json!({
        "company": "A166",
        "name": "Ledger Employee",
        "source_filename": "bound.xlsx",
        "source_sheet": "Sheet1",
        "source_row": 2,
        "source_key": source_key,
        "source_metadata": {
            "identity_resolution": {
                "strategy": "employee_number",
                "manual_review_required": false
            }
        },
        "canonical": {
            "employee_number": "BOUND-166",
            "employment_status": "ACTIVE",
            "leave_accrued": "15.000000",
            "leave_used": "2.000000",
            "leave_remaining": "13.000000"
        }
    });
    sqlx::query(
        "INSERT INTO data_import_runs \
         (id,org_id,entity_type,status,source_filename,source_format,source_sha256, \
          input_rows,candidate_rows,preserved_rows,created_by,dry_run_summary, pay_period_start, pay_period_end) \
         VALUES ($1,$2,'employee_hr','DRY_RUN','bound.xlsx','xlsx',repeat('b',64), \
                 1,1,0,$3,'{\"ready_rows\":1}'::jsonb, DATE '2026-06-01', DATE '2026-06-30')",
    )
    .bind(run_id)
    .bind(ORG)
    .bind(USER)
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO data_import_rows \
         (org_id,run_id,source_sheet,source_row,source_key,row_status,raw_row,canonical_row,validation) \
         VALUES ($1,$2,'Sheet1',2,$3,'CANDIDATE',$4,$5,'{\"status\":\"ok\"}'::jsonb)",
    )
    .bind(ORG)
    .bind(run_id)
    .bind(&source_key)
    .bind(&raw_row)
    .bind(&canonical_row)
    .execute(&owner_pool)
    .await
    .unwrap();

    let mut forged_row = canonical_row.clone();
    forged_row["raw_row"] = serde_json::json!({"company":"FORGED","name":"Forged Employee"});
    forged_row["name"] = serde_json::json!("Forged Employee");
    forged_row["canonical"]["leave_remaining"] = serde_json::json!("999.000000");
    forged_row["identity"] = serde_json::json!({
        "strategy": "source_row_fingerprint",
        "confidence": "low",
        "review_required": true
    });
    let forged_rows = serde_json::json!([forged_row]);

    let command = command_role_pool(&owner_pool).await;
    sqlx::query("SELECT set_config('app.current_org', $1, false)")
        .bind(ORG.to_string())
        .execute(&command)
        .await
        .unwrap();
    let error = sqlx::query(
        "SELECT * FROM leave_api.apply_employee_import_batch(\
             $1,$2,$3,$4,$5,'{}'::jsonb,\
             '34343434343434343434343434343434','5656565656565656')",
    )
    .bind(ORG)
    .bind(run_id)
    .bind(format!("run:{run_id}"))
    .bind(forged_rows)
    .bind(USER)
    .fetch_one(&command)
    .await
    .expect_err("staged apply must reject same-key rows whose payload differs from the ledger");
    assert_eq!(database_error_code(&error).as_deref(), Some("22023"));
    assert_eq!(
        error.as_database_error().unwrap().message(),
        "employee_import_batch.run_payload_mismatch"
    );

    let state: (String, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT r.status,
          (SELECT count(*) FROM employees e
            WHERE e.org_id=$1 AND e.source_key=$3),
          (SELECT count(*) FROM leave_balance_import_receipts x
            WHERE x.org_id=$1 AND x.source_ref=$4),
          (SELECT count(*) FROM audit_events a
            WHERE a.org_id=$1 AND a.action='data_import.apply' AND a.target_id=$2::text)
        FROM data_import_runs r
        WHERE r.org_id=$1 AND r.id=$2
        "#,
    )
    .bind(ORG)
    .bind(run_id)
    .bind(&source_key)
    .bind(format!("run:{run_id}"))
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(state, ("DRY_RUN".to_owned(), 0, 0, 0));
}
