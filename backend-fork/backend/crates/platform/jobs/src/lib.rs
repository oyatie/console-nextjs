//! `console-platform-jobs` - job queue port plus the apalis-postgres adapter.
//!
//! ADR-0011 keeps apalis isolated behind [`JobQueue`]. The adapter owns apalis'
//! timestamped migrations and `apalis.*` schema; numbered project migrations
//! remain in `console-platform-db`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::{future::Future, pin::Pin, sync::Arc, time::Duration as StdDuration};

use apalis::prelude::{BoxDynError, Data, TaskSink, WorkerBuilder, WorkerContext};
use apalis_postgres::{Config, PgPool as ApalisPgPool, PostgresStorage};
use apalis_sql::ext::TaskBuilderExt as _;
use apalis_sqlx::Row as _;
use console_kernel_core::{Clock, EvidenceId, OrgId, P1DispatchId, Timestamp};
use serde::{Deserialize, Serialize};
use sqlx::{Connection as _, Row as _};
use thiserror::Error;

pub mod soak;

/// Operational hygiene cap for per-pod Apalis worker identities.
///
/// Kubernetes pods generate unique worker names, so old pods can otherwise
/// leave inert `apalis.workers` rows forever. Seven days is intentionally much
/// longer than the Apalis heartbeat/orphan re-enqueue window, while still
/// bounding registry growth for rows that are no longer referenced by jobs.
pub const DEFAULT_APALIS_WORKER_RETENTION: StdDuration = StdDuration::from_secs(7 * 24 * 60 * 60);

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn new(value: impl Into<String>) -> Result<Self, JobQueueError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(JobQueueError::InvalidIdempotencyKey);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId(String);

impl JobId {
    /// Build a [`JobId`] from a key string. Used by alternate [`JobQueue`]
    /// implementations (e.g. test stubs) that don't go through apalis.
    #[must_use]
    pub fn from_key(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlatformJob {
    EscalationTimer(EscalationTimerJob),
    DispatchAcceptWindowExpired(DispatchTimerJob),
    DispatchAlimtalkNoAck(DispatchTimerJob),
    DispatchManualCallRequired(DispatchTimerJob),
    /// Transcode/optimize a staged evidence original into the final 1080p/
    /// recompressed deliverable. Carries the owning tenant so the worker arms
    /// `app.current_org` to the right org for its RLS-gated reads/writes.
    EvidenceTranscode(EvidenceTranscodeJob),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceTranscodeJob {
    /// The tenant that owns the evidence row. Carried on the payload so the
    /// background worker arms `app.current_org` to the RIGHT tenant for its
    /// RLS-gated staging read + status write — never a hardcoded tenant.
    pub org_id: OrgId,
    pub evidence_id: EvidenceId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EscalationTimerJob {
    pub scenario_id: String,
    pub timer_id: String,
    pub scheduled_for: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispatchTimerJob {
    pub dispatch_id: P1DispatchId,
    /// The tenant that owns the dispatch. Carried on the payload so the
    /// background worker can arm `app.current_org` to the RIGHT tenant for its
    /// RLS-gated reads/writes — never a hardcoded bootstrap tenant. Defaults to
    /// KNL on a legacy payload that predates this field, so jobs enqueued before
    /// the multi-tenant rollout still process under the original single tenant.
    #[serde(default = "OrgId::knl")]
    pub org_id: OrgId,
    pub scheduled_for: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobRequest {
    pub job: PlatformJob,
    pub idempotency_key: IdempotencyKey,
}

impl JobRequest {
    pub fn escalation_timer(
        scenario_id: impl Into<String>,
        timer_id: impl Into<String>,
        scheduled_for: Timestamp,
        idempotency_key: impl Into<String>,
    ) -> Result<Self, JobQueueError> {
        Ok(Self {
            job: PlatformJob::EscalationTimer(EscalationTimerJob {
                scenario_id: scenario_id.into(),
                timer_id: timer_id.into(),
                scheduled_for,
            }),
            idempotency_key: IdempotencyKey::new(idempotency_key)?,
        })
    }

    pub fn dispatch_accept_window_expired(
        dispatch_id: P1DispatchId,
        org_id: OrgId,
        scheduled_for: Timestamp,
    ) -> Result<Self, JobQueueError> {
        Ok(Self {
            job: PlatformJob::DispatchAcceptWindowExpired(DispatchTimerJob {
                dispatch_id,
                org_id,
                scheduled_for,
            }),
            idempotency_key: IdempotencyKey::new(format!(
                "p1-dispatch:{}:accept-window",
                dispatch_id
            ))?,
        })
    }

    pub fn dispatch_alimtalk_no_ack(
        dispatch_id: P1DispatchId,
        org_id: OrgId,
        scheduled_for: Timestamp,
    ) -> Result<Self, JobQueueError> {
        Ok(Self {
            job: PlatformJob::DispatchAlimtalkNoAck(DispatchTimerJob {
                dispatch_id,
                org_id,
                scheduled_for,
            }),
            idempotency_key: IdempotencyKey::new(format!(
                "p1-dispatch:{}:alimtalk-no-ack",
                dispatch_id
            ))?,
        })
    }

    pub fn dispatch_manual_call_required(
        dispatch_id: P1DispatchId,
        org_id: OrgId,
        scheduled_for: Timestamp,
    ) -> Result<Self, JobQueueError> {
        Ok(Self {
            job: PlatformJob::DispatchManualCallRequired(DispatchTimerJob {
                dispatch_id,
                org_id,
                scheduled_for,
            }),
            idempotency_key: IdempotencyKey::new(format!(
                "p1-dispatch:{}:manual-call-required",
                dispatch_id
            ))?,
        })
    }

    /// Enqueue a media transcode/optimize job for a staged evidence original.
    /// Idempotency keys on the evidence id, so a re-issued presign for the same
    /// row coalesces to a single transcode.
    pub fn evidence_transcode(
        org_id: OrgId,
        evidence_id: EvidenceId,
    ) -> Result<Self, JobQueueError> {
        Ok(Self {
            job: PlatformJob::EvidenceTranscode(EvidenceTranscodeJob {
                org_id,
                evidence_id,
            }),
            idempotency_key: IdempotencyKey::new(format!("evidence-transcode:{evidence_id}"))?,
        })
    }
}

pub trait JobQueue: Send + Sync {
    fn enqueue<'a>(&'a self, request: JobRequest) -> BoxFuture<'a, Result<JobId, JobQueueError>>;

    fn schedule_at<'a>(
        &'a self,
        request: JobRequest,
        scheduled_at: Timestamp,
    ) -> BoxFuture<'a, Result<JobId, JobQueueError>>;
}

pub trait PlatformJobHandler: Send + Sync + 'static {
    fn handle<'a>(&'a self, job: PlatformJob) -> BoxFuture<'a, Result<(), JobQueueError>>;
}

#[derive(Debug, Error)]
pub enum JobQueueError {
    #[error("idempotency key must not be empty")]
    InvalidIdempotencyKey,

    #[error("scheduled timestamp is before the Unix epoch")]
    ScheduledBeforeUnixEpoch,

    #[error("schedule delay is too large")]
    ScheduleDelayTooLarge,

    #[error("worker retention window is too large")]
    WorkerRetentionTooLarge,

    #[error("apalis-postgres error: {0}")]
    ApalisPostgres(String),

    #[error("apalis task sink error: {0}")]
    ApalisTaskSink(String),

    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("worker error: {0}")]
    Worker(String),

    #[error("soak failed: {0}")]
    Soak(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApalisWorkerRetentionSummary {
    pub stale_workers_pruned: i64,
    pub stale_workers_retained_with_jobs: i64,
    pub worker_rows_remaining: i64,
}

#[cfg(test)]
fn should_prune_apalis_worker(
    current_worker_name: &str,
    candidate_worker_name: &str,
    candidate_age: StdDuration,
    retention: StdDuration,
    referenced_by_jobs: bool,
) -> bool {
    candidate_worker_name != current_worker_name
        && candidate_age >= retention
        && !referenced_by_jobs
}

#[derive(Debug, Clone)]
pub struct ApalisPostgresJobQueue {
    pool: ApalisPgPool,
    config: Config,
}

impl ApalisPostgresJobQueue {
    pub async fn connect(database_url: &str, queue_name: &str) -> Result<Self, JobQueueError> {
        let pool = connect_apalis_runtime_pool(database_url).await?;
        Self::from_pool(pool, queue_name).await
    }

    pub(crate) async fn from_pool(
        pool: ApalisPgPool,
        queue_name: &str,
    ) -> Result<Self, JobQueueError> {
        validate_apalis_runtime(&pool).await?;
        Ok(Self {
            pool,
            config: Config::new(queue_name),
        })
    }

    pub(crate) async fn from_pool_with_config(
        pool: ApalisPgPool,
        config: Config,
    ) -> Result<Self, JobQueueError> {
        validate_apalis_runtime(&pool).await?;
        Ok(Self { pool, config })
    }
}

pub async fn connect_apalis_runtime_pool(
    database_url: &str,
) -> Result<ApalisPgPool, JobQueueError> {
    apalis_sqlx::postgres::PgPoolOptions::new()
        .after_connect(|conn, _meta| {
            Box::pin(async move { validate_apalis_runtime_connection(conn).await })
        })
        .after_release(|conn, _meta| {
            Box::pin(async move {
                apalis_sqlx::query("RESET SESSION AUTHORIZATION")
                    .execute(&mut *conn)
                    .await?;
                apalis_sqlx::query("RESET ROLE").execute(&mut *conn).await?;
                apalis_sqlx::query("RESET ALL").execute(&mut *conn).await?;
                Ok(validate_apalis_runtime_connection(conn).await.is_ok())
            })
        })
        .connect(database_url)
        .await
        .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))
}

async fn validate_apalis_runtime_connection(
    conn: &mut apalis_sqlx::PgConnection,
) -> Result<(), apalis_sqlx::Error> {
    let row = apalis_sqlx::query(
        r#"
        SELECT
            session_user = 'console_rt'
            AND current_user = 'console_rt'
            AND authenticated.rolcanlogin
            AND NOT authenticated.rolsuper
            AND NOT authenticated.rolbypassrls
            AND NOT authenticated.rolinherit
            AND NOT authenticated.rolcreatedb
            AND NOT authenticated.rolcreaterole
            AND NOT authenticated.rolreplication
            AND NOT EXISTS (
                SELECT 1
                FROM pg_catalog.pg_roles AS candidate
                WHERE candidate.rolname <> session_user
                  AND pg_catalog.pg_has_role(session_user, candidate.oid, 'MEMBER')
            )
            AND NOT EXISTS (
                SELECT 1
                FROM pg_catalog.pg_auth_members AS membership
                WHERE membership.roleid = authenticated.oid
            )
            AND current_setting('statement_timeout')::interval = interval '30 seconds'
            AND current_setting('idle_in_transaction_session_timeout')::interval = interval '30 seconds'
            AND current_setting('transaction_timeout')::interval = interval '45 seconds'
            AS valid
        FROM pg_catalog.pg_roles AS authenticated
        WHERE authenticated.rolname = session_user
        "#,
    )
    .fetch_one(&mut *conn)
    .await?;
    if row.try_get::<bool, _>("valid")? {
        Ok(())
    } else {
        Err(apalis_sqlx::Error::Protocol(
            "Apalis pool must authenticate directly as hardened console_rt with exact 30s/30s/45s timeouts and no membership edges".to_owned(),
        ))
    }
}

/// Apply the adapter-owned Apalis migrations and reconcile the runtime ACL.
///
/// The caller must pass the already validated migration-owner connection used
/// for the numbered application migrations. Keeping this API connection-based
/// prevents a second pool or a second physical checkout during startup.
pub async fn migrate_and_reconcile_apalis_postgres(
    conn: &mut sqlx::PgConnection,
) -> Result<(), JobQueueError> {
    const LOCK_ID: i64 = 901_011;

    let mut transaction = conn.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(LOCK_ID)
        .execute(&mut *transaction)
        .await?;
    run_apalis_owner_reconciliation(&mut transaction).await?;
    transaction.commit().await?;
    Ok(())
}

async fn run_apalis_owner_reconciliation(
    conn: &mut sqlx::PgConnection,
) -> Result<(), JobQueueError> {
    let owner_identity_is_exact: bool = sqlx::query_scalar(
        r#"
        SELECT session_user = 'console_app'
           AND current_user = 'console_app'
           AND pg_get_userbyid(database.datdba) = 'console_app'
        FROM pg_catalog.pg_database AS database
        WHERE database.datname = current_database()
        "#,
    )
    .fetch_one(&mut *conn)
    .await?;
    if !owner_identity_is_exact {
        return Err(JobQueueError::ApalisPostgres(
            "Apalis owner reconciliation requires the directly authenticated console_app database owner"
                .to_owned(),
        ));
    }

    let runtime_role_exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'console_rt')")
            .fetch_one(&mut *conn)
            .await?;
    if !runtime_role_exists {
        return Err(JobQueueError::ApalisPostgres(
            "required Apalis runtime role console_rt does not exist".to_owned(),
        ));
    }

    let has_foreign_owned_apalis_objects: bool = sqlx::query_scalar(
        r#"
        SELECT
            EXISTS (
                SELECT 1 FROM pg_catalog.pg_namespace AS namespace
                WHERE namespace.nspname = 'apalis'
                  AND pg_get_userbyid(namespace.nspowner) <> 'console_app'
            )
            OR EXISTS (
                SELECT 1
                FROM pg_catalog.pg_class AS object
                JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
                WHERE namespace.nspname = 'apalis'
                  AND pg_get_userbyid(object.relowner) <> 'console_app'
            )
            OR EXISTS (
                SELECT 1
                FROM pg_catalog.pg_proc AS function
                JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = function.pronamespace
                WHERE namespace.nspname = 'apalis'
                  AND pg_get_userbyid(function.proowner) <> 'console_app'
            )
            OR EXISTS (
                SELECT 1
                FROM pg_catalog.pg_class AS ledger
                JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = ledger.relnamespace
                WHERE namespace.nspname = 'public'
                  AND ledger.relname = 'platform_jobs_apalis_migrations'
                  AND pg_get_userbyid(ledger.relowner) <> 'console_app'
            )
            OR EXISTS (
                SELECT 1
                FROM pg_catalog.pg_proc AS function
                JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = function.pronamespace
                WHERE namespace.nspname = 'public'
                  AND function.proname = 'generate_ulid'
                  AND function.pronargs = 0
                  AND pg_get_userbyid(function.proowner) <> 'console_app'
            )
        "#,
    )
    .fetch_one(&mut *conn)
    .await?;
    if has_foreign_owned_apalis_objects {
        return Err(JobQueueError::ApalisPostgres(
            "refusing to adopt or mutate Apalis objects not owned by console_app".to_owned(),
        ));
    }

    let unledgered_existing_schema: bool = sqlx::query_scalar(
        r#"
        SELECT
            (
                EXISTS (
                    SELECT 1
                    FROM pg_catalog.pg_class AS object
                    JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
                    WHERE namespace.nspname = 'apalis'
                )
                OR EXISTS (
                    SELECT 1
                    FROM pg_catalog.pg_proc AS function
                    JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = function.pronamespace
                    WHERE namespace.nspname = 'apalis'
                )
                OR to_regprocedure('public.generate_ulid()') IS NOT NULL
            )
            AND to_regclass('apalis.platform_jobs_apalis_migrations') IS NULL
            AND to_regclass('public.platform_jobs_apalis_migrations') IS NULL
        "#,
    )
    .fetch_one(&mut *conn)
    .await?;
    if unledgered_existing_schema {
        return Err(JobQueueError::ApalisPostgres(
            "refusing to auto-adopt an existing unledgered Apalis schema".to_owned(),
        ));
    }

    let migrations = PostgresStorage::migrations();
    preflight_existing_apalis_ledgers(conn, migrations.iter()).await?;

    sqlx::raw_sql(
        r#"
            CREATE SCHEMA IF NOT EXISTS apalis;

            CREATE TABLE IF NOT EXISTS apalis.platform_jobs_apalis_migrations (
                version BIGINT PRIMARY KEY,
                description TEXT NOT NULL,
                installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
                success BOOLEAN NOT NULL,
                checksum BYTEA NOT NULL,
                execution_time BIGINT NOT NULL
            )
            "#,
    )
    .execute(&mut *conn)
    .await?;

    copy_legacy_apalis_migration_ledger(conn).await?;

    for migration in migrations.iter() {
        if migration.migration_type.is_down_migration() {
            continue;
        }

        let applied = sqlx::query(
            r#"
            SELECT description, checksum, success
            FROM apalis.platform_jobs_apalis_migrations
            WHERE version = $1
            "#,
        )
        .bind(migration.version)
        .fetch_optional(&mut *conn)
        .await?;

        if let Some(row) = applied {
            let description = row
                .try_get::<String, _>("description")
                .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?;
            let checksum = row
                .try_get::<Vec<u8>, _>("checksum")
                .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?;
            let success = row
                .try_get::<bool, _>("success")
                .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?;
            if !success {
                return Err(JobQueueError::ApalisPostgres(format!(
                    "apalis migration {} previously failed",
                    migration.version
                )));
            }
            if description != migration.description.as_ref() {
                return Err(JobQueueError::ApalisPostgres(format!(
                    "apalis migration {} description mismatch",
                    migration.version
                )));
            }
            if checksum.as_slice() != migration.checksum.as_ref() {
                return Err(JobQueueError::ApalisPostgres(format!(
                    "apalis migration {} checksum mismatch",
                    migration.version
                )));
            }
            continue;
        }

        let started = std::time::Instant::now();
        let sql = normalized_apalis_migration_sql(migration);
        // The only dynamic input is the compile-time vendor migration text
        // returned by `PostgresStorage::migrations`; no user/config data is
        // interpolated into this SQL.
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&mut *conn)
            .await
            .map_err(|err| {
                JobQueueError::ApalisPostgres(format!(
                    "failed apalis migration {}: {err}",
                    migration.version
                ))
            })?;
        sqlx::query(
            r#"
            INSERT INTO apalis.platform_jobs_apalis_migrations (
                version,
                description,
                success,
                checksum,
                execution_time
            )
            VALUES ($1, $2, TRUE, $3, $4)
            "#,
        )
        .bind(migration.version)
        .bind(migration.description.as_ref())
        .bind(migration.checksum.as_ref())
        .bind(started.elapsed().as_nanos() as i64)
        .execute(&mut *conn)
        .await
        .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?;
    }

    reconcile_apalis_runtime_acl(conn).await?;
    validate_apalis_owner_state(conn).await
}

#[derive(Debug)]
struct ExistingMigrationLedgerRow {
    version: i64,
    description: String,
    success: bool,
    checksum: Vec<u8>,
}

async fn preflight_existing_apalis_ledgers<'a>(
    conn: &mut sqlx::PgConnection,
    migrations: impl Iterator<Item = &'a apalis_sqlx::migrate::Migration> + Clone,
) -> Result<(), JobQueueError> {
    let apalis_ledger_exists: bool = sqlx::query_scalar(
        "SELECT to_regclass('apalis.platform_jobs_apalis_migrations') IS NOT NULL",
    )
    .fetch_one(&mut *conn)
    .await?;
    let apalis_rows = if apalis_ledger_exists {
        let rows = sqlx::query(
            "SELECT version, description, success, checksum FROM apalis.platform_jobs_apalis_migrations ORDER BY version",
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(existing_migration_ledger_row)
        .collect::<Result<Vec<_>, _>>()?;
        validate_existing_migration_ledger("apalis", &rows, migrations.clone())?;
        Some(rows)
    } else {
        None
    };

    let legacy_ledger_exists: bool = sqlx::query_scalar(
        "SELECT to_regclass('public.platform_jobs_apalis_migrations') IS NOT NULL",
    )
    .fetch_one(&mut *conn)
    .await?;
    let legacy_rows = if legacy_ledger_exists {
        let rows = sqlx::query(
            "SELECT version, description, success, checksum FROM public.platform_jobs_apalis_migrations ORDER BY version",
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(existing_migration_ledger_row)
        .collect::<Result<Vec<_>, _>>()?;
        validate_existing_migration_ledger("legacy public", &rows, migrations)?;
        Some(rows)
    } else {
        None
    };

    if let (Some(apalis_rows), Some(legacy_rows)) = (&apalis_rows, &legacy_rows) {
        for apalis_row in apalis_rows {
            let ledgers_disagree = legacy_rows
                .iter()
                .find(|legacy_row| legacy_row.version == apalis_row.version)
                .is_some_and(|legacy_row| {
                    apalis_row.description != legacy_row.description
                        || apalis_row.success != legacy_row.success
                        || apalis_row.checksum != legacy_row.checksum
                });
            if ledgers_disagree {
                return Err(JobQueueError::ApalisPostgres(format!(
                    "canonical and legacy Apalis migration ledgers disagree at version {}",
                    apalis_row.version
                )));
            }
        }
    }
    Ok(())
}

fn existing_migration_ledger_row(
    row: sqlx::postgres::PgRow,
) -> Result<ExistingMigrationLedgerRow, JobQueueError> {
    Ok(ExistingMigrationLedgerRow {
        version: row.try_get("version")?,
        description: row.try_get("description")?,
        success: row.try_get("success")?,
        checksum: row.try_get("checksum")?,
    })
}

fn validate_existing_migration_ledger<'a>(
    ledger_name: &str,
    rows: &[ExistingMigrationLedgerRow],
    migrations: impl Iterator<Item = &'a apalis_sqlx::migrate::Migration>,
) -> Result<(), JobQueueError> {
    let known: Vec<_> = migrations
        .filter(|migration| !migration.migration_type.is_down_migration())
        .collect();
    let max_known = known
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or_default();
    let has_future = rows.iter().any(|row| row.version > max_known);
    let mut present = vec![false; known.len()];
    let mut previous_version = None;

    for row in rows {
        if previous_version == Some(row.version) {
            return Err(JobQueueError::ApalisPostgres(format!(
                "{ledger_name} Apalis migration ledger contains duplicate version {}",
                row.version
            )));
        }
        previous_version = Some(row.version);
        if !row.success {
            return Err(JobQueueError::ApalisPostgres(format!(
                "{ledger_name} Apalis migration ledger contains unsuccessful version {}",
                row.version
            )));
        }
        if row.version > max_known {
            continue;
        }
        let Some((index, migration)) = known
            .iter()
            .enumerate()
            .find(|(_, migration)| migration.version == row.version)
        else {
            return Err(JobQueueError::ApalisPostgres(format!(
                "{ledger_name} Apalis migration ledger contains unknown historical version {}",
                row.version
            )));
        };
        validate_migration_values(migration, &row.description, &row.checksum, row.success)?;
        present[index] = true;
    }

    if has_future && present.iter().any(|is_present| !is_present) {
        return Err(JobQueueError::ApalisPostgres(format!(
            "{ledger_name} Apalis migration ledger has later rows before all known migrations"
        )));
    }
    if !has_future {
        let present_count = present.iter().take_while(|is_present| **is_present).count();
        if present[present_count..]
            .iter()
            .any(|is_present| *is_present)
        {
            return Err(JobQueueError::ApalisPostgres(format!(
                "{ledger_name} Apalis migration ledger known rows do not form a prefix"
            )));
        }
    }
    Ok(())
}

async fn copy_legacy_apalis_migration_ledger(
    conn: &mut sqlx::PgConnection,
) -> Result<(), JobQueueError> {
    sqlx::raw_sql(
        r#"
            DO $$
            BEGIN
                IF to_regclass('public.platform_jobs_apalis_migrations') IS NOT NULL THEN
                    EXECUTE '
                        INSERT INTO apalis.platform_jobs_apalis_migrations (
                            version,
                            description,
                            installed_on,
                            success,
                            checksum,
                            execution_time
                        )
                        SELECT
                            version,
                            description,
                            installed_on,
                            success,
                            checksum,
                            execution_time
                        FROM public.platform_jobs_apalis_migrations
                        ON CONFLICT (version) DO NOTHING
                    ';
                END IF;
            END
            $$;
            "#,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn existing_apalis_schema_is_current(
    conn: &mut sqlx::PgConnection,
) -> Result<bool, JobQueueError> {
    Ok(sqlx::query_scalar(APALIS_STRUCTURE_QUERY)
        .bind(max_known_apalis_migration_version())
        .bind(EXPECTED_GET_JOBS_PROSRC)
        .fetch_one(&mut *conn)
        .await?)
}

async fn reconcile_apalis_runtime_acl(conn: &mut sqlx::PgConnection) -> Result<(), JobQueueError> {
    sqlx::raw_sql(
        r#"
        DO $acl$
        BEGIN
            EXECUTE format(
                'REVOKE CREATE ON DATABASE %I FROM console_rt',
                current_database()
            );
        END
        $acl$;

        REVOKE ALL PRIVILEGES ON SCHEMA apalis FROM PUBLIC;
        REVOKE ALL PRIVILEGES ON SCHEMA apalis FROM console_rt;
        GRANT USAGE ON SCHEMA apalis TO console_rt;

        REVOKE ALL PRIVILEGES ON TABLE apalis.jobs FROM PUBLIC, console_rt;
        REVOKE ALL PRIVILEGES ON TABLE apalis.workers FROM PUBLIC, console_rt;
        REVOKE ALL PRIVILEGES ON TABLE apalis.platform_jobs_apalis_migrations FROM PUBLIC, console_rt;
        GRANT SELECT, INSERT, UPDATE ON TABLE apalis.jobs TO console_rt;
        GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE apalis.workers TO console_rt;
        GRANT SELECT ON TABLE apalis.platform_jobs_apalis_migrations TO console_rt;

        REVOKE ALL PRIVILEGES ON FUNCTION apalis.get_jobs(TEXT, TEXT, INTEGER) FROM PUBLIC, console_rt;
        REVOKE ALL PRIVILEGES ON FUNCTION apalis.notify_new_jobs() FROM PUBLIC, console_rt;
        REVOKE ALL PRIVILEGES ON FUNCTION apalis.push_job(TEXT, JSON, TEXT, TIMESTAMPTZ, INTEGER, INTEGER) FROM PUBLIC, console_rt;
        REVOKE ALL PRIVILEGES ON FUNCTION public.generate_ulid() FROM PUBLIC, console_rt;
        GRANT EXECUTE ON FUNCTION apalis.get_jobs(TEXT, TEXT, INTEGER) TO console_rt;
        "#,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn validate_apalis_owner_state(conn: &mut sqlx::PgConnection) -> Result<(), JobQueueError> {
    if !existing_apalis_schema_is_current(conn).await? {
        return Err(JobQueueError::ApalisPostgres(
            "Apalis schema does not match the adapter-owned structure".to_owned(),
        ));
    }
    let all_objects_owned_by_console_app: bool = sqlx::query_scalar(
        r#"
        SELECT
            (SELECT pg_get_userbyid(nspowner) = 'console_app'
             FROM pg_catalog.pg_namespace WHERE nspname = 'apalis')
            AND NOT EXISTS (
                SELECT 1
                FROM pg_catalog.pg_class AS object
                JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
                WHERE namespace.nspname = 'apalis'
                  AND pg_get_userbyid(object.relowner) <> 'console_app'
            )
            AND NOT EXISTS (
                SELECT 1
                FROM pg_catalog.pg_proc AS function
                JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = function.pronamespace
                WHERE namespace.nspname = 'apalis'
                  AND pg_get_userbyid(function.proowner) <> 'console_app'
            )
            AND (
                SELECT pg_get_userbyid(function.proowner) = 'console_app'
                FROM pg_catalog.pg_proc AS function
                JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = function.pronamespace
                WHERE namespace.nspname = 'public'
                  AND function.proname = 'generate_ulid'
                  AND function.pronargs = 0
            )
        "#,
    )
    .fetch_one(&mut *conn)
    .await?;
    if !all_objects_owned_by_console_app {
        return Err(JobQueueError::ApalisPostgres(
            "Apalis schema, relations, and functions must all be owned by console_app".to_owned(),
        ));
    }
    validate_owner_migration_ledger(conn).await?;

    let acl_is_exact: bool = sqlx::query_scalar(APALIS_EXPLICIT_ROLE_ACL_QUERY)
        .bind("console_rt")
        .fetch_one(&mut *conn)
        .await?;
    if !acl_is_exact {
        return Err(JobQueueError::ApalisPostgres(
            "Apalis console_rt privileges do not match the least-privilege runtime contract"
                .to_owned(),
        ));
    }
    let public_helper_acl_is_exact: bool = sqlx::query_scalar(
        r#"
        SELECT NOT has_function_privilege('console_rt', 'public.generate_ulid()', 'EXECUTE')
           AND NOT EXISTS (
               SELECT 1
               FROM pg_catalog.pg_proc AS function,
                    LATERAL aclexplode(COALESCE(function.proacl, acldefault('f', function.proowner))) AS acl
               WHERE function.oid = to_regprocedure('public.generate_ulid()')
                 AND acl.grantee = 0
                 AND acl.privilege_type = 'EXECUTE'
           )
        "#,
    )
    .fetch_one(&mut *conn)
    .await?;
    if !public_helper_acl_is_exact {
        return Err(JobQueueError::ApalisPostgres(
            "Apalis public.generate_ulid privileges do not match the least-privilege contract"
                .to_owned(),
        ));
    }
    Ok(())
}

async fn validate_owner_migration_ledger(
    conn: &mut sqlx::PgConnection,
) -> Result<(), JobQueueError> {
    let migrations = PostgresStorage::migrations();
    validate_owner_forward_compatible_ledger(conn, migrations.iter()).await?;
    for migration in migrations
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
    {
        let row = sqlx::query(
            "SELECT description, checksum, success FROM apalis.platform_jobs_apalis_migrations WHERE version = $1",
        )
        .bind(migration.version)
        .fetch_optional(&mut *conn)
        .await?;
        validate_migration_row(
            migration,
            row.map(|row| {
                (
                    row.try_get::<String, _>("description"),
                    row.try_get::<Vec<u8>, _>("checksum"),
                    row.try_get::<bool, _>("success"),
                )
            }),
        )?;
    }
    Ok(())
}

async fn validate_apalis_runtime(pool: &ApalisPgPool) -> Result<(), JobQueueError> {
    let mut conn = pool
        .acquire()
        .await
        .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?;
    validate_runtime_migration_ledger(&mut conn).await?;
    if !existing_apalis_schema_is_current_runtime(&mut conn).await? {
        return Err(JobQueueError::ApalisPostgres(
            "Apalis runtime schema does not match the adapter-owned structure".to_owned(),
        ));
    }
    let acl_is_exact = apalis_sqlx::query(APALIS_CURRENT_ROLE_ACL_QUERY)
        .fetch_one(&mut *conn)
        .await
        .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?
        .try_get::<bool, _>("acl_is_exact")
        .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?;
    if !acl_is_exact {
        return Err(JobQueueError::ApalisPostgres(
            "current role does not match the least-privilege Apalis runtime contract".to_owned(),
        ));
    }
    Ok(())
}

async fn validate_runtime_migration_ledger(
    conn: &mut apalis_sqlx::pool::PoolConnection<apalis_sqlx::Postgres>,
) -> Result<(), JobQueueError> {
    let migrations = PostgresStorage::migrations();
    validate_runtime_forward_compatible_ledger(conn, migrations.iter()).await?;
    for migration in migrations
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
    {
        let row = apalis_sqlx::query(
            "SELECT description, checksum, success FROM apalis.platform_jobs_apalis_migrations WHERE version = $1",
        )
        .bind(migration.version)
        .fetch_optional(&mut **conn)
        .await
        .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?;
        validate_migration_row(
            migration,
            row.map(|row| {
                (
                    row.try_get::<String, _>("description"),
                    row.try_get::<Vec<u8>, _>("checksum"),
                    row.try_get::<bool, _>("success"),
                )
            }),
        )?;
    }
    Ok(())
}

type MigrationLedgerRead<E> = (Result<String, E>, Result<Vec<u8>, E>, Result<bool, E>);

fn validate_migration_row<E: std::fmt::Display>(
    migration: &apalis_sqlx::migrate::Migration,
    row: Option<MigrationLedgerRead<E>>,
) -> Result<(), JobQueueError> {
    let Some((description, checksum, success)) = row else {
        return Err(JobQueueError::ApalisPostgres(format!(
            "required Apalis migration {} is missing",
            migration.version
        )));
    };
    let description = description.map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?;
    let checksum = checksum.map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?;
    let success = success.map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?;
    validate_migration_values(migration, &description, &checksum, success)
}

fn validate_migration_values(
    migration: &apalis_sqlx::migrate::Migration,
    description: &str,
    checksum: &[u8],
    success: bool,
) -> Result<(), JobQueueError> {
    if !success {
        return Err(JobQueueError::ApalisPostgres(format!(
            "Apalis migration {} is not successful",
            migration.version
        )));
    }
    if description != migration.description.as_ref() {
        return Err(JobQueueError::ApalisPostgres(format!(
            "Apalis migration {} description mismatch",
            migration.version
        )));
    }
    if checksum != migration.checksum.as_ref() {
        return Err(JobQueueError::ApalisPostgres(format!(
            "Apalis migration {} checksum mismatch",
            migration.version
        )));
    }
    Ok(())
}

async fn validate_owner_forward_compatible_ledger<'a>(
    conn: &mut sqlx::PgConnection,
    migrations: impl Iterator<Item = &'a apalis_sqlx::migrate::Migration>,
) -> Result<(), JobQueueError> {
    let versions: Vec<i64> = migrations
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| migration.version)
        .collect();
    let max_known = versions.iter().copied().max().unwrap_or_default();
    let known_count = i64::try_from(versions.len()).map_err(|_| {
        JobQueueError::ApalisPostgres("Apalis migration count exceeds i64".to_owned())
    })?;
    let row = sqlx::query(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE version <= $1)::BIGINT AS at_or_before_known,
            COUNT(*) FILTER (WHERE version > $1 AND success)::BIGINT AS later_successful,
            COUNT(*)::BIGINT AS total,
            COUNT(*) FILTER (WHERE NOT success)::BIGINT AS unsuccessful
        FROM apalis.platform_jobs_apalis_migrations
        "#,
    )
    .bind(max_known)
    .fetch_one(&mut *conn)
    .await?;
    ensure_forward_compatible_ledger_counts(
        known_count,
        row.try_get("at_or_before_known")?,
        row.try_get("later_successful")?,
        row.try_get("total")?,
        row.try_get("unsuccessful")?,
    )
}

async fn validate_runtime_forward_compatible_ledger<'a>(
    conn: &mut apalis_sqlx::pool::PoolConnection<apalis_sqlx::Postgres>,
    migrations: impl Iterator<Item = &'a apalis_sqlx::migrate::Migration>,
) -> Result<(), JobQueueError> {
    let versions: Vec<i64> = migrations
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| migration.version)
        .collect();
    let max_known = versions.iter().copied().max().unwrap_or_default();
    let known_count = i64::try_from(versions.len()).map_err(|_| {
        JobQueueError::ApalisPostgres("Apalis migration count exceeds i64".to_owned())
    })?;
    let row = apalis_sqlx::query(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE version <= $1)::BIGINT AS at_or_before_known,
            COUNT(*) FILTER (WHERE version > $1 AND success)::BIGINT AS later_successful,
            COUNT(*)::BIGINT AS total,
            COUNT(*) FILTER (WHERE NOT success)::BIGINT AS unsuccessful
        FROM apalis.platform_jobs_apalis_migrations
        "#,
    )
    .bind(max_known)
    .fetch_one(&mut **conn)
    .await
    .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?;
    ensure_forward_compatible_ledger_counts(
        known_count,
        row.try_get("at_or_before_known")
            .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?,
        row.try_get("later_successful")
            .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?,
        row.try_get("total")
            .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?,
        row.try_get("unsuccessful")
            .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?,
    )
}

fn ensure_forward_compatible_ledger_counts(
    known_count: i64,
    at_or_before_known: i64,
    later_successful: i64,
    total: i64,
    unsuccessful: i64,
) -> Result<(), JobQueueError> {
    if unsuccessful != 0
        || at_or_before_known != known_count
        || total != known_count + later_successful
    {
        return Err(JobQueueError::ApalisPostgres(
            "Apalis migration ledger contains a failed, unknown historical, or malformed row"
                .to_owned(),
        ));
    }
    Ok(())
}

async fn existing_apalis_schema_is_current_runtime(
    conn: &mut apalis_sqlx::pool::PoolConnection<apalis_sqlx::Postgres>,
) -> Result<bool, JobQueueError> {
    let row = apalis_sqlx::query(APALIS_STRUCTURE_QUERY)
        .bind(max_known_apalis_migration_version())
        .bind(EXPECTED_GET_JOBS_PROSRC)
        .fetch_one(&mut **conn)
        .await
        .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?;
    row.try_get::<bool, _>("structure_is_current")
        .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))
}

fn max_known_apalis_migration_version() -> i64 {
    PostgresStorage::migrations()
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| migration.version)
        .max()
        .unwrap_or_default()
}

const APALIS_STRUCTURE_QUERY: &str = r#"
SELECT
    to_regclass('apalis.jobs') IS NOT NULL
    AND to_regclass('apalis.workers') IS NOT NULL
    AND to_regclass('apalis.platform_jobs_apalis_migrations') IS NOT NULL
    AND EXISTS (
        SELECT 1
        FROM pg_catalog.pg_index AS job_index
        WHERE job_index.indexrelid = to_regclass('apalis.idx_jobs_idempotency_key')
          AND job_index.indrelid = to_regclass('apalis.jobs')
          AND job_index.indisunique
          AND job_index.indisvalid
          AND job_index.indisready
          AND job_index.indnkeyatts = 2
          AND job_index.indnatts = 2
          AND job_index.indexprs IS NULL
          AND job_index.indpred IS NULL
          AND ARRAY(
              SELECT attribute.attname
              FROM unnest(job_index.indkey::SMALLINT[]) WITH ORDINALITY AS key(attnum, position)
              JOIN pg_catalog.pg_attribute AS attribute
                ON attribute.attrelid = job_index.indrelid AND attribute.attnum = key.attnum
              ORDER BY key.position
          ) = ARRAY['job_type', 'idempotency_key']::NAME[]
    )
    AND EXISTS (
        SELECT 1
        FROM pg_catalog.pg_constraint AS primary_key
        JOIN pg_catalog.pg_attribute AS attribute
          ON attribute.attrelid = primary_key.conrelid
         AND attribute.attnum = primary_key.conkey[1]
        WHERE primary_key.conrelid = to_regclass('apalis.jobs')
          AND primary_key.contype = 'p'
          AND cardinality(primary_key.conkey) = 1
          AND attribute.attname = 'id'
    )
    AND EXISTS (
        SELECT 1
        FROM pg_catalog.pg_constraint AS foreign_key
        JOIN pg_catalog.pg_attribute AS local_attribute
          ON local_attribute.attrelid = foreign_key.conrelid
         AND local_attribute.attnum = foreign_key.conkey[1]
        JOIN pg_catalog.pg_attribute AS referenced_attribute
          ON referenced_attribute.attrelid = foreign_key.confrelid
         AND referenced_attribute.attnum = foreign_key.confkey[1]
        WHERE foreign_key.conrelid = to_regclass('apalis.jobs')
          AND foreign_key.confrelid = to_regclass('apalis.workers')
          AND foreign_key.contype = 'f'
          AND cardinality(foreign_key.conkey) = 1
          AND cardinality(foreign_key.confkey) = 1
          AND local_attribute.attname = 'lock_by'
          AND referenced_attribute.attname = 'id'
          AND foreign_key.confupdtype = 'a'
          AND foreign_key.confdeltype = 'a'
          AND foreign_key.confmatchtype = 's'
          AND foreign_key.convalidated
          AND NOT foreign_key.condeferrable
          AND NOT foreign_key.condeferred
    )
    AND EXISTS (
        SELECT 1
        FROM pg_catalog.pg_constraint AS primary_key
        JOIN pg_catalog.pg_attribute AS attribute
          ON attribute.attrelid = primary_key.conrelid
         AND attribute.attnum = primary_key.conkey[1]
        WHERE primary_key.conrelid = to_regclass('apalis.workers')
          AND primary_key.contype = 'p'
          AND cardinality(primary_key.conkey) = 1
          AND attribute.attname = 'id'
    )
    AND EXISTS (
        SELECT 1 FROM pg_trigger
        WHERE tgrelid = 'apalis.jobs'::regclass
          AND tgname = 'notify_workers'
          AND NOT tgisinternal
          AND tgenabled <> 'D'
          AND tgfoid = to_regprocedure('apalis.notify_new_jobs()')
          AND (tgtype & 1) = 1
          AND (tgtype & 2) = 0
          AND (tgtype & 4) = 4
          AND (tgtype & (8 | 16 | 32)) = 0
    )
    AND EXISTS (
        SELECT 1
        FROM pg_catalog.pg_proc AS function
        JOIN pg_catalog.pg_language AS language ON language.oid = function.prolang
        WHERE function.oid = to_regprocedure('apalis.get_jobs(text,text,integer)')
          AND function.proretset
          AND function.prorettype = to_regtype('apalis.jobs')
          AND function.provolatile = 'v'
          AND function.prokind = 'f'
          AND function.pronargdefaults = 1
          AND language.lanname = 'plpgsql'
          AND (
              EXISTS (
                  SELECT 1
                  FROM apalis.platform_jobs_apalis_migrations
                  WHERE version > $1 AND success
              )
              OR btrim(function.prosrc) = btrim($2)
          )
    )
    AND NOT EXISTS (
        SELECT 1
        FROM (VALUES
            ('attempts', 'integer'), ('done_at', 'timestamp with time zone'),
            ('id', 'text'), ('idempotency_key', 'text'), ('job', 'bytea'),
            ('job_type', 'text'), ('last_result', 'jsonb'),
            ('lock_at', 'timestamp with time zone'), ('lock_by', 'text'),
            ('max_attempts', 'integer'), ('metadata', 'jsonb'),
            ('priority', 'integer'), ('run_at', 'timestamp with time zone'),
            ('status', 'text')
        ) AS required(column_name, data_type)
        WHERE NOT EXISTS (
            SELECT 1 FROM information_schema.columns actual
            WHERE actual.table_schema = 'apalis' AND actual.table_name = 'jobs'
              AND actual.column_name = required.column_name
              AND actual.data_type = required.data_type
        )
    )
    AND NOT EXISTS (
        SELECT 1
        FROM (VALUES
            ('job', NULL::TEXT), ('id', NULL), ('job_type', NULL),
            ('status', '''Pending''::text'), ('attempts', '0'),
            ('max_attempts', '25'), ('run_at', 'now()')
        ) AS required(column_name, column_default)
        WHERE NOT EXISTS (
            SELECT 1 FROM information_schema.columns actual
            WHERE actual.table_schema = 'apalis' AND actual.table_name = 'jobs'
              AND actual.column_name = required.column_name
              AND actual.is_nullable = 'NO'
              AND (required.column_default IS NULL OR actual.column_default = required.column_default)
        )
    )
    AND NOT EXISTS (
        SELECT 1
        FROM (VALUES
            ('id', 'text'), ('last_seen', 'timestamp with time zone'),
            ('layers', 'text'), ('started_at', 'timestamp with time zone'),
            ('storage_name', 'text'), ('worker_type', 'text')
        ) AS required(column_name, data_type)
        WHERE NOT EXISTS (
            SELECT 1 FROM information_schema.columns actual
            WHERE actual.table_schema = 'apalis' AND actual.table_name = 'workers'
              AND actual.column_name = required.column_name
              AND actual.data_type = required.data_type
        )
    )
    AND NOT EXISTS (
        SELECT 1
        FROM (VALUES
            ('id', NULL::TEXT), ('worker_type', NULL), ('storage_name', NULL),
            ('layers', '''''::text'), ('last_seen', 'now()')
        ) AS required(column_name, column_default)
        WHERE NOT EXISTS (
            SELECT 1 FROM information_schema.columns actual
            WHERE actual.table_schema = 'apalis' AND actual.table_name = 'workers'
              AND actual.column_name = required.column_name
              AND actual.is_nullable = 'NO'
              AND (required.column_default IS NULL OR actual.column_default = required.column_default)
        )
    )
    AS structure_is_current
"#;

const EXPECTED_GET_JOBS_PROSRC: &str = r#" BEGIN RETURN QUERY
UPDATE apalis.jobs
SET status = 'Queued',
    lock_by = worker_id,
    lock_at = now()
WHERE id IN (
        SELECT id
        FROM apalis.jobs
        WHERE (status='Pending' OR (status = 'Failed' AND attempts < max_attempts))
            AND run_at < now()
            AND job_type = v_job_type
        ORDER BY priority DESC, run_at ASC
        LIMIT v_job_count FOR
        UPDATE SKIP LOCKED
    )
returning *;
END;
"#;

const APALIS_EXPLICIT_ROLE_ACL_QUERY: &str = r#"
SELECT
    has_schema_privilege($1, 'apalis', 'USAGE')
    AND NOT has_schema_privilege($1, 'apalis', 'CREATE')
    AND NOT has_database_privilege($1, current_database(), 'CREATE')
    AND has_table_privilege($1, 'apalis.jobs', 'SELECT')
    AND has_table_privilege($1, 'apalis.jobs', 'INSERT')
    AND has_table_privilege($1, 'apalis.jobs', 'UPDATE')
    AND NOT has_table_privilege($1, 'apalis.jobs', 'DELETE')
    AND NOT has_table_privilege($1, 'apalis.jobs', 'TRUNCATE')
    AND NOT has_table_privilege($1, 'apalis.jobs', 'REFERENCES')
    AND NOT has_table_privilege($1, 'apalis.jobs', 'TRIGGER')
    AND has_table_privilege($1, 'apalis.workers', 'SELECT')
    AND has_table_privilege($1, 'apalis.workers', 'INSERT')
    AND has_table_privilege($1, 'apalis.workers', 'UPDATE')
    AND has_table_privilege($1, 'apalis.workers', 'DELETE')
    AND NOT has_table_privilege($1, 'apalis.workers', 'TRUNCATE')
    AND NOT has_table_privilege($1, 'apalis.workers', 'REFERENCES')
    AND NOT has_table_privilege($1, 'apalis.workers', 'TRIGGER')
    AND has_table_privilege($1, 'apalis.platform_jobs_apalis_migrations', 'SELECT')
    AND NOT has_table_privilege($1, 'apalis.platform_jobs_apalis_migrations', 'INSERT')
    AND NOT has_table_privilege($1, 'apalis.platform_jobs_apalis_migrations', 'UPDATE')
    AND NOT has_table_privilege($1, 'apalis.platform_jobs_apalis_migrations', 'DELETE')
    AND NOT has_table_privilege($1, 'apalis.platform_jobs_apalis_migrations', 'TRUNCATE')
    AND NOT has_table_privilege($1, 'apalis.platform_jobs_apalis_migrations', 'REFERENCES')
    AND NOT has_table_privilege($1, 'apalis.platform_jobs_apalis_migrations', 'TRIGGER')
    AND has_function_privilege($1, 'apalis.get_jobs(text,text,integer)', 'EXECUTE')
    AND NOT has_function_privilege($1, 'apalis.notify_new_jobs()', 'EXECUTE')
    AND NOT has_function_privilege($1, 'apalis.push_job(text,json,text,timestamp with time zone,integer,integer)', 'EXECUTE')
    AND NOT has_function_privilege($1, 'public.generate_ulid()', 'EXECUTE')
    AS acl_is_exact
"#;

const APALIS_CURRENT_ROLE_ACL_QUERY: &str = r#"
SELECT
    has_schema_privilege(current_user, 'apalis', 'USAGE')
    AND NOT has_schema_privilege(current_user, 'apalis', 'CREATE')
    AND NOT has_database_privilege(current_user, current_database(), 'CREATE')
    AND has_table_privilege(current_user, 'apalis.jobs', 'SELECT')
    AND has_table_privilege(current_user, 'apalis.jobs', 'INSERT')
    AND has_table_privilege(current_user, 'apalis.jobs', 'UPDATE')
    AND NOT has_table_privilege(current_user, 'apalis.jobs', 'DELETE')
    AND NOT has_table_privilege(current_user, 'apalis.jobs', 'TRUNCATE')
    AND NOT has_table_privilege(current_user, 'apalis.jobs', 'REFERENCES')
    AND NOT has_table_privilege(current_user, 'apalis.jobs', 'TRIGGER')
    AND has_table_privilege(current_user, 'apalis.workers', 'SELECT')
    AND has_table_privilege(current_user, 'apalis.workers', 'INSERT')
    AND has_table_privilege(current_user, 'apalis.workers', 'UPDATE')
    AND has_table_privilege(current_user, 'apalis.workers', 'DELETE')
    AND NOT has_table_privilege(current_user, 'apalis.workers', 'TRUNCATE')
    AND NOT has_table_privilege(current_user, 'apalis.workers', 'REFERENCES')
    AND NOT has_table_privilege(current_user, 'apalis.workers', 'TRIGGER')
    AND has_table_privilege(current_user, 'apalis.platform_jobs_apalis_migrations', 'SELECT')
    AND NOT has_table_privilege(current_user, 'apalis.platform_jobs_apalis_migrations', 'INSERT')
    AND NOT has_table_privilege(current_user, 'apalis.platform_jobs_apalis_migrations', 'UPDATE')
    AND NOT has_table_privilege(current_user, 'apalis.platform_jobs_apalis_migrations', 'DELETE')
    AND NOT has_table_privilege(current_user, 'apalis.platform_jobs_apalis_migrations', 'TRUNCATE')
    AND NOT has_table_privilege(current_user, 'apalis.platform_jobs_apalis_migrations', 'REFERENCES')
    AND NOT has_table_privilege(current_user, 'apalis.platform_jobs_apalis_migrations', 'TRIGGER')
    AND has_function_privilege(current_user, 'apalis.get_jobs(text,text,integer)', 'EXECUTE')
    AND NOT has_function_privilege(current_user, 'apalis.notify_new_jobs()', 'EXECUTE')
    AND NOT has_function_privilege(current_user, 'apalis.push_job(text,json,text,timestamp with time zone,integer,integer)', 'EXECUTE')
    AND NOT has_function_privilege(
        current_user,
        (
            SELECT function.oid
            FROM pg_catalog.pg_proc AS function
            JOIN pg_catalog.pg_namespace AS namespace
              ON namespace.oid = function.pronamespace
            WHERE namespace.nspname = 'public'
              AND function.proname = 'generate_ulid'
              AND function.pronargs = 0
        ),
        'EXECUTE'
    )
    AS acl_is_exact
"#;

fn normalized_apalis_migration_sql(migration: &apalis_sqlx::migrate::Migration) -> String {
    let mut sql = migration.sql.replace(
        "CREATE SCHEMA apalis;",
        "CREATE SCHEMA IF NOT EXISTS apalis;",
    );
    if migration.version == 20_220_530_084_123 {
        sql = sql.replace(
            "CREATE FUNCTION apalis.notify_new_jobs() returns trigger",
            "CREATE OR REPLACE FUNCTION apalis.notify_new_jobs() returns trigger",
        );
        sql = sql.replace(
            "CREATE TRIGGER notify_workers after insert on apalis.jobs",
            "DROP TRIGGER IF EXISTS notify_workers ON apalis.jobs;\n\n        CREATE TRIGGER notify_workers after insert on apalis.jobs",
        );
    }
    sql
}

impl JobQueue for ApalisPostgresJobQueue {
    fn enqueue<'a>(&'a self, request: JobRequest) -> BoxFuture<'a, Result<JobId, JobQueueError>> {
        Box::pin(async move {
            self.schedule_at(request, console_kernel_core::SystemClock.now())
                .await
        })
    }

    fn schedule_at<'a>(
        &'a self,
        request: JobRequest,
        scheduled_at: Timestamp,
    ) -> BoxFuture<'a, Result<JobId, JobQueueError>> {
        Box::pin(async move {
            let run_at = unix_timestamp_seconds(scheduled_at)?;
            let idempotency_key = request.idempotency_key.as_str().to_owned();
            let task = apalis_postgres::PgTask::<PlatformJob>::builder(request.job)
                .run_at_timestamp(run_at)
                .with_idempotency_key(&idempotency_key)
                .max_attempts(5)
                .build();

            let mut backend =
                PostgresStorage::<PlatformJob>::new_with_config(&self.pool, &self.config);
            match backend.push_task(task).await {
                Ok(()) => Ok(JobId(idempotency_key)),
                Err(err) if is_unique_idempotency_conflict(&err.to_string()) => {
                    Ok(JobId(idempotency_key))
                }
                Err(err) => Err(JobQueueError::ApalisTaskSink(err.to_string())),
            }
        })
    }
}

pub async fn run_apalis_worker_until_shutdown<H, F>(
    database_url: &str,
    queue_name: &str,
    worker_name: impl Into<String>,
    handler: H,
    shutdown: F,
) -> Result<(), JobQueueError>
where
    H: PlatformJobHandler,
    F: Future<Output = ()> + Send,
{
    let pool = connect_apalis_runtime_pool(database_url).await?;
    validate_apalis_runtime(&pool).await?;
    let worker_name = worker_name.into();
    let retention_summary = prune_stale_apalis_workers(
        &pool,
        queue_name,
        &worker_name,
        DEFAULT_APALIS_WORKER_RETENTION,
    )
    .await?;
    tracing::info!(
        queue_name = %queue_name,
        worker_name = %worker_name,
        retention_seconds = DEFAULT_APALIS_WORKER_RETENTION.as_secs(),
        stale_workers_pruned = retention_summary.stale_workers_pruned,
        stale_workers_retained_with_jobs = retention_summary.stale_workers_retained_with_jobs,
        worker_rows_remaining = retention_summary.worker_rows_remaining,
        "apalis worker registry retention applied"
    );
    let backend = PostgresStorage::<PlatformJob>::new_with_config(&pool, &Config::new(queue_name));
    let worker = WorkerBuilder::new(worker_name)
        .backend(backend)
        .data(Arc::new(handler))
        .build(handle_queued_platform_job::<H>);

    tokio::select! {
        result = worker.run() => result.map_err(|err| JobQueueError::Worker(err.to_string())),
        () = shutdown => Ok(()),
    }
}

pub async fn prune_stale_apalis_workers(
    pool: &ApalisPgPool,
    queue_name: &str,
    current_worker_name: &str,
    retention: StdDuration,
) -> Result<ApalisWorkerRetentionSummary, JobQueueError> {
    let retention_seconds =
        i64::try_from(retention.as_secs()).map_err(|_| JobQueueError::WorkerRetentionTooLarge)?;
    let row = apalis_sqlx::query(
        r#"
        WITH stale AS (
            SELECT workers.id
            FROM apalis.workers workers
            WHERE workers.worker_type = $1
                AND workers.id <> $2
                AND workers.last_seen < NOW() - ($3::DOUBLE PRECISION * INTERVAL '1 second')
        ),
        referenced_stale AS (
            SELECT COUNT(*)::BIGINT AS count
            FROM stale
            WHERE EXISTS (
                SELECT 1
                FROM apalis.jobs jobs
                WHERE jobs.lock_by = stale.id
            )
        ),
        deleted AS (
            DELETE FROM apalis.workers workers
            WHERE workers.worker_type = $1
                AND workers.id <> $2
                AND workers.last_seen < NOW() - ($3::DOUBLE PRECISION * INTERVAL '1 second')
                AND NOT EXISTS (
                    SELECT 1
                    FROM apalis.jobs jobs
                    WHERE jobs.lock_by = workers.id
                )
            RETURNING workers.id
        )
        SELECT
            (SELECT COUNT(*)::BIGINT FROM deleted) AS stale_workers_pruned,
            (SELECT count FROM referenced_stale) AS stale_workers_retained_with_jobs
        "#,
    )
    .bind(queue_name)
    .bind(current_worker_name)
    .bind(retention_seconds)
    // rls-arming: ok apalis.workers/apalis.jobs are global scheduler tables (no org_id, no RLS); pruning is queue-scoped and never tenant-data scoped
    .fetch_one(pool)
    .await
    .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?;
    let worker_rows_remaining = apalis_sqlx::query(
        r#"
        SELECT COUNT(*)::BIGINT AS count
        FROM apalis.workers workers
        WHERE workers.worker_type = $1
        "#,
    )
    .bind(queue_name)
    // rls-arming: ok apalis.workers is a global scheduler registry table (no org_id, no RLS); this readback is queue-scoped
    .fetch_one(pool)
    .await
    .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?
    .try_get::<i64, _>("count")
    .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?;

    Ok(ApalisWorkerRetentionSummary {
        stale_workers_pruned: row
            .try_get::<i64, _>("stale_workers_pruned")
            .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?,
        stale_workers_retained_with_jobs: row
            .try_get::<i64, _>("stale_workers_retained_with_jobs")
            .map_err(|err| JobQueueError::ApalisPostgres(err.to_string()))?,
        worker_rows_remaining,
    })
}

async fn handle_queued_platform_job<H>(
    job: PlatformJob,
    handler: Data<Arc<H>>,
    _worker: WorkerContext,
) -> Result<(), BoxDynError>
where
    H: PlatformJobHandler,
{
    handler
        .handle(job)
        .await
        .map_err(|err| Box::new(err) as BoxDynError)
}

pub fn schedule_after(clock: &dyn Clock, delay: StdDuration) -> Result<Timestamp, JobQueueError> {
    let delay =
        time::Duration::try_from(delay).map_err(|_| JobQueueError::ScheduleDelayTooLarge)?;
    clock
        .now()
        .checked_add(delay)
        .ok_or(JobQueueError::ScheduleDelayTooLarge)
}

#[derive(Clone, Copy)]
pub struct SkewedClock<'a> {
    inner: &'a dyn Clock,
    offset: time::Duration,
}

impl<'a> SkewedClock<'a> {
    #[must_use]
    pub fn new(inner: &'a dyn Clock, offset: time::Duration) -> Self {
        Self { inner, offset }
    }
}

impl Clock for SkewedClock<'_> {
    fn now(&self) -> Timestamp {
        self.inner.now() + self.offset
    }
}

fn unix_timestamp_seconds(value: Timestamp) -> Result<u64, JobQueueError> {
    let timestamp = value.unix_timestamp();
    u64::try_from(timestamp).map_err(|_| JobQueueError::ScheduledBeforeUnixEpoch)
}

fn is_unique_idempotency_conflict(message: &str) -> bool {
    message.contains("idx_jobs_idempotency_key")
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_kernel_core::FixedClock;

    #[test]
    fn schedule_after_uses_injected_clock() {
        let base = time::macros::datetime!(2026-06-12 09:00:00 UTC);
        let fixed = FixedClock(base);
        let skewed = SkewedClock::new(&fixed, time::Duration::seconds(7));

        let scheduled = schedule_after(&skewed, StdDuration::from_secs(3)).unwrap();

        assert_eq!(scheduled, time::macros::datetime!(2026-06-12 09:00:10 UTC));
    }

    #[test]
    fn rejects_empty_idempotency_key() {
        let result = JobRequest::escalation_timer(
            "scenario",
            "timer-1",
            time::macros::datetime!(2026-06-12 09:00:00 UTC),
            "   ",
        );

        assert!(matches!(result, Err(JobQueueError::InvalidIdempotencyKey)));
    }

    #[test]
    fn unique_conflict_detection_only_accepts_idempotency_constraint() {
        assert!(is_unique_idempotency_conflict(
            "duplicate key value violates unique constraint \"idx_jobs_idempotency_key\""
        ));
        assert!(!is_unique_idempotency_conflict(
            "duplicate key value violates unique constraint \"unique_job_id\""
        ));
        assert!(!is_unique_idempotency_conflict(
            "duplicate key value violates unique constraint"
        ));
    }

    #[test]
    fn worker_retention_prunes_only_stale_unreferenced_non_current_workers() {
        let retention = StdDuration::from_secs(60);

        assert!(should_prune_apalis_worker(
            "current-worker",
            "old-unreferenced-worker",
            StdDuration::from_secs(61),
            retention,
            false
        ));
        assert!(!should_prune_apalis_worker(
            "current-worker",
            "current-worker",
            StdDuration::from_secs(3_600),
            retention,
            false
        ));
        assert!(!should_prune_apalis_worker(
            "current-worker",
            "recent-worker",
            StdDuration::from_secs(59),
            retention,
            false
        ));
        assert!(!should_prune_apalis_worker(
            "current-worker",
            "old-referenced-worker",
            StdDuration::from_secs(61),
            retention,
            true
        ));
    }

    #[test]
    fn migration_ledger_accepts_only_successful_forward_rows() {
        assert!(ensure_forward_compatible_ledger_counts(4, 4, 0, 4, 0).is_ok());
        assert!(ensure_forward_compatible_ledger_counts(4, 4, 2, 6, 0).is_ok());

        assert!(ensure_forward_compatible_ledger_counts(4, 5, 0, 5, 0).is_err());
        assert!(ensure_forward_compatible_ledger_counts(4, 4, 1, 6, 0).is_err());
        assert!(ensure_forward_compatible_ledger_counts(4, 4, 1, 5, 1).is_err());
    }

    #[test]
    fn manual_call_required_job_uses_dispatch_scoped_idempotency_key() {
        let dispatch_id = P1DispatchId::new();
        let scheduled_for = time::macros::datetime!(2026-06-12 09:10:00 UTC);

        let org_id = OrgId::knl();
        let request = JobRequest::dispatch_manual_call_required(dispatch_id, org_id, scheduled_for)
            .expect("manual-call dispatch job request should be valid");

        assert_eq!(
            request.idempotency_key.as_str(),
            format!("p1-dispatch:{dispatch_id}:manual-call-required")
        );
        assert_eq!(
            request.job,
            PlatformJob::DispatchManualCallRequired(DispatchTimerJob {
                dispatch_id,
                org_id,
                scheduled_for,
            })
        );
    }
}
