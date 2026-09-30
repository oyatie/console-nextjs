#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::time::Duration as StdDuration;

use console_kernel_core::{FixedClock, Timestamp};
use console_platform_jobs::{
    ApalisPostgresJobQueue, DEFAULT_APALIS_WORKER_RETENTION, JobQueue, JobRequest, SkewedClock,
    connect_apalis_runtime_pool, migrate_and_reconcile_apalis_postgres, prune_stale_apalis_workers,
    soak::{self, APALIS_POSTGRES_VERSION, APALIS_VERSION},
};
use sqlx::{Connection as _, Row};

#[test]
fn crate_versions_are_pinned_to_live_verified_rcs() {
    assert_eq!(APALIS_VERSION, "1.0.0-rc.9");
    assert_eq!(APALIS_POSTGRES_VERSION, "1.0.0-rc.8");
    let stable_available = soak::APALIS_STABLE_1_0_0_AVAILABLE;
    assert!(!stable_available);
}

#[test]
fn skewed_clock_drives_schedule_after() {
    let base = time::macros::datetime!(2026-06-12 09:00:00 UTC);
    let clock = FixedClock(base);
    let skewed = SkewedClock::new(&clock, time::Duration::milliseconds(-750));

    let scheduled = console_platform_jobs::schedule_after(&skewed, StdDuration::from_millis(1_750))
        .expect("skewed schedule should be valid");

    assert_eq!(scheduled, time::macros::datetime!(2026-06-12 09:00:01 UTC));
}

#[tokio::test]
async fn apalis_adapter_dedupes_repeated_idempotency_keys() {
    let owner_database_url = std::env::var("CONSOLE_APALIS_OWNER_DATABASE_URL")
        .expect("CONSOLE_APALIS_OWNER_DATABASE_URL is required for apalis adapter test");
    let runtime_database_url = std::env::var("CONSOLE_APALIS_RUNTIME_DATABASE_URL")
        .expect("CONSOLE_APALIS_RUNTIME_DATABASE_URL is required for apalis adapter test");
    reassert_serving_role_passwords(&owner_database_url, &runtime_database_url).await;
    let mut owner_connection = sqlx::PgConnection::connect(&owner_database_url)
        .await
        .expect("connect migration owner");
    migrate_and_reconcile_apalis_postgres(&mut owner_connection)
        .await
        .expect("migrate and reconcile Apalis as owner");
    let queue_name = format!("console.t110.adapter-test.{}", uuid::Uuid::new_v4());
    let queue = ApalisPostgresJobQueue::connect(&runtime_database_url, &queue_name)
        .await
        .expect("validate and connect Apalis queue as console_rt");
    let workspace_pool = sqlx::PgPool::connect(&owner_database_url)
        .await
        .expect("connect workspace sqlx pool");
    let scheduled_for: Timestamp = time::OffsetDateTime::now_utc() + time::Duration::seconds(60);
    let key = format!("adapter-test:{}", uuid::Uuid::new_v4());
    let request = JobRequest::escalation_timer("adapter-test", "timer-001", scheduled_for, &key)
        .expect("valid job request");

    queue
        .schedule_at(request.clone(), scheduled_for)
        .await
        .expect("first enqueue succeeds");
    queue
        .schedule_at(request, scheduled_for)
        .await
        .expect("second enqueue is idempotent");

    let row = sqlx::query(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM apalis.jobs
        WHERE job_type = $1 AND idempotency_key = $2
        "#,
    )
    .bind(&queue_name)
    .bind(&key)
    .fetch_one(&workspace_pool)
    .await
    .expect("count apalis jobs");
    let count: i64 = row.try_get(0).expect("count column");
    assert_eq!(count, 1);

    sqlx::query("DELETE FROM apalis.jobs WHERE job_type = $1")
        .bind(&queue_name)
        .execute(&workspace_pool)
        .await
        .expect("cleanup apalis jobs");
}

#[tokio::test]
async fn apalis_worker_retention_prunes_only_stale_unreferenced_workers() {
    let owner_database_url = std::env::var("CONSOLE_APALIS_OWNER_DATABASE_URL")
        .expect("CONSOLE_APALIS_OWNER_DATABASE_URL is required for apalis adapter test");
    let runtime_database_url = std::env::var("CONSOLE_APALIS_RUNTIME_DATABASE_URL")
        .expect("CONSOLE_APALIS_RUNTIME_DATABASE_URL is required for apalis adapter test");
    let mut owner_connection = sqlx::PgConnection::connect(&owner_database_url)
        .await
        .expect("connect migration owner");
    migrate_and_reconcile_apalis_postgres(&mut owner_connection)
        .await
        .expect("migrate and reconcile Apalis as owner");
    let queue_name = format!("console.t110.retention-test.{}", uuid::Uuid::new_v4());
    let other_queue_name = format!("console.t110.retention-other.{}", uuid::Uuid::new_v4());
    let current_worker = format!("retention-current-{}", uuid::Uuid::new_v4());
    let stale_worker = format!("retention-stale-{}", uuid::Uuid::new_v4());
    let recent_worker = format!("retention-recent-{}", uuid::Uuid::new_v4());
    let referenced_worker = format!("retention-referenced-{}", uuid::Uuid::new_v4());
    let other_queue_worker = format!("retention-other-{}", uuid::Uuid::new_v4());
    let referenced_job_id = format!("retention-job-{}", uuid::Uuid::new_v4());

    let _queue = ApalisPostgresJobQueue::connect(&runtime_database_url, &queue_name)
        .await
        .expect("validate and connect Apalis queue as console_rt");
    let apalis_pool = connect_apalis_runtime_pool(&runtime_database_url)
        .await
        .expect("connect hardened apalis sqlx runtime pool");
    let workspace_pool = sqlx::PgPool::connect(&owner_database_url)
        .await
        .expect("connect workspace sqlx pool");

    cleanup_worker_retention_rows(
        &workspace_pool,
        &[
            current_worker.as_str(),
            stale_worker.as_str(),
            recent_worker.as_str(),
            referenced_worker.as_str(),
            other_queue_worker.as_str(),
        ],
        &[referenced_job_id.as_str()],
    )
    .await;

    for (worker_id, worker_type, age_sql) in [
        (&current_worker, &queue_name, "8 days"),
        (&stale_worker, &queue_name, "8 days"),
        (&referenced_worker, &queue_name, "8 days"),
        (&other_queue_worker, &other_queue_name, "8 days"),
        (&recent_worker, &queue_name, "1 hour"),
    ] {
        sqlx::query(
            r#"
            INSERT INTO apalis.workers (id, worker_type, storage_name, layers, last_seen)
            VALUES ($1, $2, 'PostgresStorage', '', NOW() - ($3::TEXT)::INTERVAL)
            "#,
        )
        .bind(worker_id)
        .bind(worker_type)
        .bind(age_sql)
        .execute(&workspace_pool)
        .await
        .expect("insert retention test worker");
    }
    sqlx::query(
        r#"
        INSERT INTO apalis.jobs (job, id, job_type, status, lock_by)
        VALUES (decode('00', 'hex'), $1, $2, 'Done', $3)
        "#,
    )
    .bind(&referenced_job_id)
    .bind(&queue_name)
    .bind(&referenced_worker)
    .execute(&workspace_pool)
    .await
    .expect("insert job referencing stale worker");

    let summary = prune_stale_apalis_workers(
        &apalis_pool,
        &queue_name,
        &current_worker,
        DEFAULT_APALIS_WORKER_RETENTION,
    )
    .await
    .expect("prune stale apalis workers");

    assert_eq!(summary.stale_workers_pruned, 1);
    assert_eq!(summary.stale_workers_retained_with_jobs, 1);
    assert_eq!(summary.worker_rows_remaining, 3);

    assert!(
        !worker_exists(&workspace_pool, &stale_worker).await,
        "stale unreferenced worker should be pruned"
    );
    for worker_id in [
        &current_worker,
        &recent_worker,
        &referenced_worker,
        &other_queue_worker,
    ] {
        assert!(
            worker_exists(&workspace_pool, worker_id).await,
            "{worker_id} should be retained"
        );
    }

    cleanup_worker_retention_rows(
        &workspace_pool,
        &[
            current_worker.as_str(),
            stale_worker.as_str(),
            recent_worker.as_str(),
            referenced_worker.as_str(),
            other_queue_worker.as_str(),
        ],
        &[referenced_job_id.as_str()],
    )
    .await;
}

async fn worker_exists(pool: &sqlx::PgPool, worker_id: &str) -> bool {
    let row = sqlx::query("SELECT COUNT(*)::BIGINT FROM apalis.workers WHERE id = $1")
        .bind(worker_id)
        .fetch_one(pool)
        .await
        .expect("count apalis worker");
    let count: i64 = row.try_get(0).expect("count column");
    count == 1
}

async fn cleanup_worker_retention_rows(pool: &sqlx::PgPool, worker_ids: &[&str], job_ids: &[&str]) {
    for job_id in job_ids {
        sqlx::query("DELETE FROM apalis.jobs WHERE id = $1")
            .bind(*job_id)
            .execute(pool)
            .await
            .expect("cleanup retention test job");
    }
    for worker_id in worker_ids {
        sqlx::query("DELETE FROM apalis.workers WHERE id = $1")
            .bind(*worker_id)
            .execute(pool)
            .await
            .expect("cleanup retention test worker");
    }
}

/// Re-assert the two serving-role passwords the harness minted, before connecting.
///
/// `ALTER ROLE ... PASSWORD` is CLUSTER-GLOBAL: it outlives the per-test database that
/// `#[sqlx::test]` drops. Four sibling test files in this same serialized single-container
/// run rewrite `console_app`'s password to a hardcoded literal so they can log in as it —
/// `platform/db/tests/attendance_console_migration_contract.rs:981,1178`,
/// `ontology/adapter-postgres/tests/key_revision_migration_upgrade.rs:37`,
/// `leave/adapter-postgres/tests/leave_migration_expand_contract.rs:158` — and none restores
/// it. Whichever of them runs first, every later target authenticating as `console_app` or
/// `console_rt` from the harness URL gets `28P01 password authentication failed`.
///
/// Restoring in those siblings would be order-dependent and a panic would skip it. Re-asserting
/// here is neither: this test states the credential it needs, from the admin URL the harness
/// also supplies, and is correct regardless of what any sibling did or how it exited.
async fn reassert_serving_role_passwords(owner_url: &str, runtime_url: &str) {
    fn password_of(url: &str) -> String {
        let after_scheme = url.split("://").nth(1).expect("database URL has a scheme");
        let credentials = after_scheme
            .split('@')
            .next()
            .expect("database URL has credentials");
        credentials
            .split_once(':')
            .expect("database URL carries a password")
            .1
            .to_owned()
    }
    let admin_url = std::env::var("CONSOLE_APALIS_ADMIN_DATABASE_URL")
        .expect("CONSOLE_APALIS_ADMIN_DATABASE_URL is required to re-assert role passwords");
    let mut admin = sqlx::PgConnection::connect(&admin_url)
        .await
        .expect("connect as harness admin");
    for (role, password) in [
        ("console_app", password_of(owner_url)),
        ("console_rt", password_of(runtime_url)),
    ] {
        // format(%I,%L) builds the statement server-side: ALTER ROLE takes no bind
        // parameters, and the password is a harness-minted literal.
        let statement: String =
            sqlx::query_scalar("SELECT format('ALTER ROLE %I PASSWORD %L', $1::text, $2::text)")
                .bind(role)
                .bind(&password)
                .fetch_one(&mut admin)
                .await
                .expect("build ALTER ROLE statement");
        // Sound: `statement` is built by PostgreSQL's own format(%I,%L) from a role name
        // this function fixes and a harness-minted hex password — never request-derived.
        sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
            .execute(&mut admin)
            .await
            .expect("re-assert serving role password");
    }
}
