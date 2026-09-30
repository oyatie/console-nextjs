#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Contract proof for `data_import_runs.pay_period_start/_end` (migration 0224).
//!
//! The pay period is what scopes a payroll roster. Before it existed, the only
//! scope was `source_filename LIKE '2026/5월/%'` in a one-off script — a literal
//! matching whatever an operator happened to name the upload. These tests prove
//! the three properties that make the replacement worth having, against a real
//! database rather than from the DDL text:
//!
//!   1. an import CANNOT be recorded without declaring its period;
//!   2. the period cannot be inside-out;
//!   3. the period cannot be changed after the fact.
//!
//! (3) is the one that is easy to miss. `console_rt` holds table-wide UPDATE on
//! this table (0070:83, never revoked) and 0166's writer guard only inspects
//! `entity_type` and INSERT-with-APPLIED, so without the trigger a period-only
//! UPDATE is permitted — and the scope of a payroll roster would be a mutable,
//! unaudited pointer sitting on top of append-only material.

use sqlx::PgPool;
use uuid::Uuid;

async fn seed_org(pool: &PgPool) -> Uuid {
    let org = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, 'Pay Period Contract')",
    )
    .bind(org)
    .bind(format!("payperiod-{}", &org.to_string()[..8]))
    .execute(pool)
    .await
    .unwrap();
    org
}

/// Insert an import run, optionally declaring a period.
async fn insert_run(
    pool: &PgPool,
    org: Uuid,
    period: Option<(&str, &str)>,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::new_v4();
    match period {
        Some((start, end)) => {
            sqlx::query(
                "INSERT INTO data_import_runs \
                 (id, org_id, entity_type, status, source_filename, source_format, source_sha256, \
                  pay_period_start, pay_period_end) \
                 VALUES ($1, $2, 'employee_hr', 'PREVIEWED', 'book.xlsx', 'xlsx', \
                         repeat('a', 64), $3::date, $4::date)",
            )
            .bind(id)
            .bind(org)
            .bind(start)
            .bind(end)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query(
                "INSERT INTO data_import_runs \
                 (id, org_id, entity_type, status, source_filename, source_format, source_sha256) \
                 VALUES ($1, $2, 'employee_hr', 'PREVIEWED', 'book.xlsx', 'xlsx', repeat('a', 64))",
            )
            .bind(id)
            .bind(org)
            .execute(pool)
            .await?;
        }
    }
    Ok(id)
}

/// The positive control. Without it, a schema that refused every import would
/// satisfy the two refusals below — and block every upload, which is worse than
/// the gap.
#[sqlx::test(migrations = "./migrations")]
async fn an_import_declaring_its_period_is_accepted(pool: PgPool) {
    let org = seed_org(&pool).await;
    let id = insert_run(&pool, org, Some(("2026-05-01", "2026-05-31")))
        .await
        .expect("a declared period must be accepted");
    let (start, end): (time::Date, time::Date) = sqlx::query_as(
        "SELECT pay_period_start, pay_period_end FROM data_import_runs WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(start.to_string(), "2026-05-01");
    assert_eq!(end.to_string(), "2026-05-31");
}

/// An import with no declared period cannot be recorded at all.
///
/// NOT NULL with no DEFAULT is deliberate: a default would let an upload acquire
/// a period nobody chose, which is the fabricated provenance the column exists to
/// remove.
#[sqlx::test(migrations = "./migrations")]
async fn an_import_without_a_period_is_refused(pool: PgPool) {
    let org = seed_org(&pool).await;
    let err = insert_run(&pool, org, None)
        .await
        .expect_err("an import with no declared pay period must be refused");
    let message = err.to_string();
    assert!(
        message.contains("pay_period_start") || message.contains("null value"),
        "the refusal must name the missing period, got: {message}"
    );
}

/// An inside-out period is refused.
#[sqlx::test(migrations = "./migrations")]
async fn a_period_ending_before_it_starts_is_refused(pool: PgPool) {
    let org = seed_org(&pool).await;
    let err = insert_run(&pool, org, Some(("2026-05-31", "2026-05-01")))
        .await
        .expect_err("pay_period_end before pay_period_start must be refused");
    assert!(
        err.to_string().contains("pay_period_order"),
        "expected the ordering CHECK to fire, got: {err}"
    );
}

/// The period cannot be changed after the import is recorded.
///
/// This is what keeps the roster's scope as trustworthy as the append-only rows
/// it selects. Without it the material would be immutable while the question
/// "which material?" stayed quietly editable.
#[sqlx::test(migrations = "./migrations")]
async fn the_period_is_immutable_after_insert(pool: PgPool) {
    let org = seed_org(&pool).await;
    let id = insert_run(&pool, org, Some(("2026-05-01", "2026-05-31")))
        .await
        .unwrap();
    let err = sqlx::query("UPDATE data_import_runs SET pay_period_start = $2::date WHERE id = $1")
        .bind(id)
        .bind("2026-06-01")
        .execute(&pool)
        .await
        .expect_err("the pay period must be immutable once recorded");
    assert!(
        err.to_string().contains("pay_period is immutable"),
        "expected the immutability trigger to fire, got: {err}"
    );

    // An UPDATE that leaves the period alone must still be allowed, or the
    // trigger would freeze the whole row and break the import lifecycle.
    sqlx::query("UPDATE data_import_runs SET status = 'DRY_RUN' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("an update that does not touch the period must still be allowed");
}
