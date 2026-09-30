//! Fixtures for the roster-materialisation integration crate.
//!
//! Kept as a module of `roster_materialisation.rs`, not a second `tests/*.rs`
//! crate: Cargo would otherwise auto-discover a new test binary.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_payroll_adapter_postgres::roster::materialise_roster_in_tx;
use sqlx::PgPool;
use time::macros::date;
use uuid::Uuid;

pub(crate) const PERIOD_START: time::Date = date!(2026 - 06 - 01);
pub(crate) const PERIOD_END: time::Date = date!(2026 - 06 - 30);

pub(crate) struct Fixture {
    pub org: Uuid,
    pub run: Uuid,
}

pub(crate) async fn seed_org_and_run(pool: &PgPool) -> Fixture {
    let org = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, 'Roster Org')")
        .bind(org)
        .bind(format!("roster-{}", &org.to_string()[..8]))
        .execute(pool)
        .await
        .unwrap();
    let run: Uuid = sqlx::query_scalar(
        "INSERT INTO payroll_draft_runs (org_id, period_start, period_end, source_label) \
         VALUES ($1, $2, $3, 'roster-test') RETURNING id",
    )
    .bind(org)
    .bind(PERIOD_START)
    .bind(PERIOD_END)
    .fetch_one(pool)
    .await
    .unwrap();
    Fixture { org, run }
}

pub(crate) async fn seed_employee(pool: &PgPool, org: Uuid, source_key: &str, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO employees (org_id, company, name, source_filename, source_sheet, source_row, source_key) \
         VALUES ($1, 'KNL', $2, 'book.xlsx', 's', 1, $3) RETURNING id",
    )
    .bind(org)
    .bind(name)
    .bind(source_key)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// One import run for the given period/status, and one row for `employee_key`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn seed_import(
    pool: &PgPool,
    org: Uuid,
    status: &str,
    row_status: &str,
    period: (time::Date, time::Date),
    employee_key: &str,
    raw_row: serde_json::Value,
) {
    let run_id = Uuid::new_v4();
    // An employee_hr run cannot be INSERTed already APPLIED: 0166's writer guard
    // raises 42501 (`employee_import_run.command_required`). It CAN be updated
    // into APPLIED by `console_leave_definer`, which is the role the guard names
    // and the only one 0166 grants UPDATE on this table. So the fixture inserts
    // DRY_RUN and transitions — exercising the guard rather than routing round it.
    sqlx::query(
        "INSERT INTO data_import_runs \
         (id, org_id, entity_type, status, source_filename, source_format, source_sha256, \
          pay_period_start, pay_period_end) \
         VALUES ($1, $2, 'employee_hr', $3, 'book.xlsx', 'xlsx', repeat('a', 64), $4, $5)",
    )
    .bind(run_id)
    .bind(org)
    .bind(if status == "APPLIED" {
        "DRY_RUN"
    } else {
        status
    })
    .bind(period.0)
    .bind(period.1)
    .execute(pool)
    .await
    .unwrap();
    if status == "APPLIED" {
        // The DRY_RUN -> APPLIED transition is governed by 0166's writer guard,
        // which requires ALL of: the `console_leave_definer` role, an armed
        // `app.current_org` (the table is under org-isolation RLS, and without it
        // the UPDATE matches zero rows and succeeds SILENTLY), and exactly one
        // same-transaction `data_import.apply` audit row whose actor is an active
        // user in the org and equals `applied_by`. The fixture satisfies the
        // guard rather than routing around it, so these tests exercise the real
        // apply path.
        let actor = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, org_id, display_name) VALUES ($1, $2, 'Importer')")
            .bind(actor)
            .bind(org)
            .execute(pool)
            .await
            .unwrap();
        let mut conn = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL ROLE console_leave_definer")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org.to_string())
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE data_import_runs SET status = 'APPLIED', applied_by = $3, \
             applied_at = now(), updated_at = now() WHERE org_id = $1 AND id = $2",
        )
        .bind(org)
        .bind(run_id)
        .bind(actor)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO audit_events \
             (actor, action, target_type, target_id, before_snap, after_snap, trace_id, span_id, occurred_at, org_id) \
             VALUES ($1, 'data_import.apply', 'data_import_run', $2, NULL, '{}'::jsonb, \
                     '0123456789abcdef0123456789abcdef', '0123456789abcdef', now(), $3)",
        )
        .bind(actor)
        .bind(run_id.to_string())
        .bind(org)
        .execute(&mut *conn)
        .await
        .unwrap();
        conn.commit().await.unwrap();
        let applied: bool =
            sqlx::query_scalar("SELECT status = 'APPLIED' FROM data_import_runs WHERE id = $1")
                .bind(run_id)
                .fetch_one(pool)
                .await
                .unwrap();
        assert!(
            applied,
            "the fixture must actually reach APPLIED, not silently no-op"
        );
    }
    sqlx::query(
        "INSERT INTO data_import_rows \
         (org_id, run_id, source_sheet, source_row, source_key, row_status, raw_row, canonical_row) \
         VALUES ($1, $2, 's', 1, $3, $4, $5, jsonb_build_object('source_key', $6::text))",
    )
    .bind(org)
    .bind(run_id)
    .bind(format!("filename:book.xlsx|sheet:s|row:{}", Uuid::new_v4()))
    .bind(row_status)
    .bind(&raw_row)
    .bind(employee_key)
    .execute(pool)
    .await
    .unwrap();
}

pub(crate) fn attendance_row() -> serde_json::Value {
    serde_json::json!({ "출근": "09:00", "근무시간": "8", "근무일수": "1" })
}

pub(crate) async fn roster(pool: &PgPool, f: &Fixture) -> Vec<(String, i32, i32)> {
    sqlx::query_as(
        "SELECT employee_source_key, payroll_source_row_count, attendance_source_row_count \
         FROM payroll_draft_lines WHERE run_id = $1 ORDER BY employee_source_key",
    )
    .bind(f.run)
    .fetch_all(pool)
    .await
    .unwrap()
}

pub(crate) async fn materialise(pool: &PgPool, f: &Fixture) -> u64 {
    let mut tx = pool.begin().await.unwrap();
    let n = materialise_roster_in_tx(&mut tx, f.org, f.run, PERIOD_START, PERIOD_END)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    n
}
