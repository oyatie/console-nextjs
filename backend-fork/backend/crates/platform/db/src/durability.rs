//! Direct PostgreSQL physical-prefix confirmation after a caller's COMMIT.
//!
//! This proves two observed database copies have flushed/replayed a WAL prefix.
//! It does not mint or validate an external writer lease, positively fence an
//! old sender, or authorize egress by itself. Callers must obtain the expected
//! epoch from that separate authority and fail closed if it is unavailable.

use std::net::IpAddr;
use std::time::Duration;

use sqlx::{Acquire, PgPool, Postgres, Row, pool::PoolConnection};
use thiserror::Error;
use tokio::time::Instant;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedWriter {
    pub writer_epoch: i64,
    pub system_identifier: String,
    pub timeline_id: u32,
    pub primary_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalTopology {
    pub primary_address: IpAddr,
    pub primary_port: u16,
    pub standby_address: IpAddr,
    pub standby_port: u16,
    pub standby_application_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedWalPrefix {
    upper_bound_lsn: String,
    system_identifier: String,
    timeline_id: u32,
    writer_epoch: i64,
}

impl ConfirmedWalPrefix {
    #[must_use]
    pub fn upper_bound_lsn(&self) -> &str {
        &self.upper_bound_lsn
    }

    #[must_use]
    pub fn system_identifier(&self) -> &str {
        &self.system_identifier
    }

    #[must_use]
    pub const fn timeline_id(&self) -> u32 {
        self.timeline_id
    }

    #[must_use]
    pub const fn writer_epoch(&self) -> i64 {
        self.writer_epoch
    }
}

#[derive(Debug, Error)]
pub enum ConfirmationError {
    #[error("invalid confirmation input: {0}")]
    InvalidInput(&'static str),
    #[error("physical topology does not match: {0}")]
    Topology(&'static str),
    #[error("writer epoch does not match: {0}")]
    Epoch(&'static str),
    #[error("two-site confirmation deadline: {0}")]
    Deadline(&'static str),
    #[error("database observation failed: {0}")]
    Database(#[from] sqlx::Error),
}

/// Exact committed audit identity; Company context is not caller authorization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRowRef {
    pub id: Uuid,
    pub org_id: Uuid,
    pub action: String,
    pub target_type: String,
    pub target_id: String,
}

/// UNKNOWN custody with one immutable post-read INSERT bound. Only the observer
/// can construct this token. It has no serialization or reconstruction API:
/// losing it (including process loss) leaves continuation unsupported/UNKNOWN.
#[derive(Debug)]
pub struct PendingAuditObservation {
    observation: Box<AuditObservation>,
}

#[derive(Debug)]
struct AuditObservation {
    audit: AuditRowRef,
    topology: PhysicalTopology,
    expected: ExpectedWriter,
    upper_bound_lsn: String,
    wal_file: String,
}

impl PendingAuditObservation {
    #[must_use]
    pub fn upper_bound_lsn(&self) -> &str {
        &self.observation.upper_bound_lsn
    }

    #[must_use]
    pub fn wal_file(&self) -> &str {
        &self.observation.wal_file
    }
}

/// Observed custody of this exact audit row on two physical database copies.
/// This is neither a generic `ConfirmedWalPrefix` nor business-receipt,
/// authorization, fencing, serving or external-effect authority.
#[derive(Debug)]
pub struct AuditedRowCustody {
    observation: Box<AuditObservation>,
}

impl AuditedRowCustody {
    #[must_use]
    pub fn audit_id(&self) -> Uuid {
        self.observation.audit.id
    }

    #[must_use]
    pub fn org_id(&self) -> Uuid {
        self.observation.audit.org_id
    }

    #[must_use]
    pub fn action(&self) -> &str {
        &self.observation.audit.action
    }

    #[must_use]
    pub fn target_type(&self) -> &str {
        &self.observation.audit.target_type
    }

    #[must_use]
    pub fn target_id(&self) -> &str {
        &self.observation.audit.target_id
    }

    #[must_use]
    pub fn upper_bound_lsn(&self) -> &str {
        &self.observation.upper_bound_lsn
    }

    #[must_use]
    pub fn wal_file(&self) -> &str {
        &self.observation.wal_file
    }

    #[must_use]
    pub fn system_identifier(&self) -> &str {
        &self.observation.expected.system_identifier
    }

    #[must_use]
    pub fn timeline_id(&self) -> u32 {
        self.observation.expected.timeline_id
    }

    #[must_use]
    pub fn writer_epoch(&self) -> i64 {
        self.observation.expected.writer_epoch
    }
}

#[derive(Debug, Error)]
pub enum AuditCustodyError {
    #[error("audit custody is UNKNOWN; resume the retained in-process observation")]
    Pending(PendingAuditObservation),
    #[error("audit custody is UNKNOWN after an observation failure: {source}")]
    ObservationFailed {
        pending: PendingAuditObservation,
        #[source]
        source: Box<AuditCustodyError>,
    },
    #[error("exact Company audit row is absent, ambiguous or changed")]
    RowMismatch,
    #[error("audit custody is UNKNOWN; deadline expired before capturing an INSERT bound")]
    DeadlineBeforeBound,
    #[error(transparent)]
    Confirmation(#[from] ConfirmationError),
    #[error("audit row observation failed: {0}")]
    Database(#[from] sqlx::Error),
}

/// Observe the exact row in a fresh Company-scoped READ COMMITTED transaction
/// on the identified primary, then capture one INSERT bound after that read.
/// A deadline after capture returns the only token supported for continuation.
/// Pools require infrastructure observation privileges and SELECT under RLS;
/// choosing a Company still requires separate caller authorization.
pub async fn observe_audited_row_post_commit(
    primary: &PgPool,
    standby: &PgPool,
    topology: &PhysicalTopology,
    expected: &ExpectedWriter,
    audit: &AuditRowRef,
    deadline: Duration,
) -> Result<AuditedRowCustody, AuditCustodyError> {
    if deadline.is_zero() {
        return Err(AuditCustodyError::DeadlineBeforeBound);
    }
    validate_confirmation_input(topology, expected, deadline)?;
    let until = Instant::now() + deadline;
    let pending = tokio::time::timeout_at(until, async {
        let mut primary = primary.acquire().await?;
        let mut standby = standby.acquire().await?;
        check_topology(&mut primary, &mut standby, topology, expected).await?;
        check_epoch(&mut primary, expected).await?;
        check_audit_row(&mut primary, audit).await?;

        // Materialization evaluates INSERT-LSN exactly once, so the filename
        // (and timeline) belongs to that same bound, even at a segment boundary.
        let row = sqlx::query(
            "WITH bound AS MATERIALIZED (SELECT pg_current_wal_insert_lsn() AS lsn) \
             SELECT lsn::text AS lsn, pg_walfile_name(lsn) AS wal_file FROM bound",
        )
        .fetch_one(&mut *primary)
        .await?;
        let upper_bound_lsn = row.try_get("lsn")?;
        let wal_file: String = row.try_get("wal_file")?;
        if wal_timeline(&wal_file)? != expected.timeline_id {
            return Err(ConfirmationError::Epoch("primary WAL timeline changed").into());
        }
        // No await after capture: a timeout cannot discard an issued token.
        Ok::<_, AuditCustodyError>(PendingAuditObservation {
            observation: Box::new(AuditObservation {
                audit: audit.clone(),
                topology: topology.clone(),
                expected: expected.clone(),
                upper_bound_lsn,
                wal_file,
            }),
        })
    })
    .await
    .map_err(|_| AuditCustodyError::DeadlineBeforeBound)??;
    finish_audit_observation(primary, standby, pending, until).await
}

/// Continue only the original in-process observation; never recapture its LSN.
/// Caller-supplied topology must still equal the topology sealed in the token.
pub async fn resume_audited_row_observation(
    primary: &PgPool,
    standby: &PgPool,
    topology: &PhysicalTopology,
    pending: PendingAuditObservation,
    deadline: Duration,
) -> Result<AuditedRowCustody, AuditCustodyError> {
    if topology != &pending.observation.topology {
        return Err(ConfirmationError::Topology("pending observation topology differs").into());
    }
    if deadline.is_zero() {
        return Err(AuditCustodyError::Pending(pending));
    }
    finish_audit_observation(primary, standby, pending, Instant::now() + deadline).await
}

async fn finish_audit_observation(
    primary: &PgPool,
    standby: &PgPool,
    pending: PendingAuditObservation,
    until: Instant,
) -> Result<AuditedRowCustody, AuditCustodyError> {
    match tokio::time::timeout_at(until, wait_for_audit_custody(primary, standby, &pending)).await {
        Ok(Ok(())) => Ok(AuditedRowCustody {
            observation: pending.observation,
        }),
        Ok(Err(error)) => {
            if matches!(
                &error,
                AuditCustodyError::RowMismatch
                    | AuditCustodyError::Confirmation(
                        ConfirmationError::InvalidInput(_)
                            | ConfirmationError::Topology(_)
                            | ConfirmationError::Epoch(_)
                    )
            ) {
                Err(error)
            } else {
                Err(AuditCustodyError::ObservationFailed {
                    pending,
                    source: Box::new(error),
                })
            }
        }
        Err(_) => Err(AuditCustodyError::Pending(pending)),
    }
}

async fn wait_for_audit_custody(
    primary: &PgPool,
    standby: &PgPool,
    pending: &PendingAuditObservation,
) -> Result<(), AuditCustodyError> {
    let mut primary = primary.acquire().await?;
    let mut standby = standby.acquire().await?;
    let observation = &pending.observation;
    let topology = &observation.topology;
    let expected = &observation.expected;
    let bound = pending.upper_bound_lsn();
    check_topology(&mut primary, &mut standby, topology, expected).await?;
    check_epoch(&mut primary, expected).await?;
    check_audit_row(&mut primary, &observation.audit).await?;

    // A quiet primary's hint-bit WAL need not wake the WAL writer. If needed,
    // flush an empty nontransactional WAL message after the fixed INSERT bound.
    // It carries no row data, changes no table, and its LSN is never used as
    // evidence. This local flush neither waits for nor proves standby replay.
    sqlx::query(
        "SELECT pg_logical_emit_message(false, 'console.audit-custody', ''::text, true) \
         WHERE pg_current_wal_flush_lsn() < $1::pg_lsn",
    )
    .bind(bound)
    .execute(&mut *primary)
    .await?;

    loop {
        let primary_ready = primary_progress(&mut primary, bound, topology, expected).await?;
        let standby_ready = standby_progress(&mut standby, bound, topology, expected).await?;
        if primary_ready && standby_ready {
            // Both reads use new snapshots, including on continuation. A row
            // changed/deleted since capture cannot be certified by its old WAL.
            check_audit_row(&mut standby, &observation.audit).await?;
            check_audit_row(&mut primary, &observation.audit).await?;
            check_topology(&mut primary, &mut standby, topology, expected).await?;
            if primary_progress(&mut primary, bound, topology, expected).await?
                && standby_progress(&mut standby, bound, topology, expected).await?
            {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn check_audit_row(
    conn: &mut PoolConnection<Postgres>,
    audit: &AuditRowRef,
) -> Result<(), AuditCustodyError> {
    let mut tx = conn.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED, READ ONLY")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(audit.org_id.to_string())
        .execute(&mut *tx)
        .await?;
    let rows = sqlx::query(
        "SELECT id, org_id, action, target_type, target_id FROM audit_events \
         WHERE id = $1 AND org_id = $2 LIMIT 2",
    )
    .bind(audit.id)
    .bind(audit.org_id)
    .fetch_all(&mut *tx)
    .await?;
    // Compare strings exactly in Rust, independent of database collation.
    let matches = if let [row] = rows.as_slice() {
        row.try_get::<Uuid, _>("id")? == audit.id
            && row.try_get::<Uuid, _>("org_id")? == audit.org_id
            && row.try_get::<String, _>("action")? == audit.action
            && row.try_get::<String, _>("target_type")? == audit.target_type
            && row.try_get::<String, _>("target_id")? == audit.target_id
    } else {
        false
    };
    tx.rollback().await?;
    if !matches {
        return Err(AuditCustodyError::RowMismatch);
    }
    Ok(())
}

/// Capture a post-commit INSERT-LSN upper bound on the identified primary,
/// then wait for its own flush and the independently connected physical
/// standby's flush and replay. No result is returned on missing or ambiguous
/// evidence. The pools must use directly authenticated PostgreSQL connections;
/// the caller must separately verify the physical sites and current fenced
/// writer/egress lease represented by `expected`.
pub async fn confirm_post_commit(
    primary: &PgPool,
    standby: &PgPool,
    topology: &PhysicalTopology,
    expected: &ExpectedWriter,
    deadline: Duration,
) -> Result<ConfirmedWalPrefix, ConfirmationError> {
    validate_confirmation_input(topology, expected, deadline)?;
    tokio::time::timeout(
        deadline,
        confirm_with_connections(primary, standby, topology, expected),
    )
    .await
    .map_err(|_| ConfirmationError::Deadline("observer did not complete within deadline"))?
}

fn validate_confirmation_input(
    topology: &PhysicalTopology,
    expected: &ExpectedWriter,
    deadline: Duration,
) -> Result<(), ConfirmationError> {
    if deadline.is_zero() || expected.writer_epoch <= 0 || expected.timeline_id == 0 {
        return Err(ConfirmationError::InvalidInput(
            "epoch, timeline and deadline must be positive",
        ));
    }
    if expected.system_identifier.is_empty()
        || !expected
            .system_identifier
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        || expected.primary_id.is_empty()
        || topology.standby_application_name.is_empty()
        || topology.primary_port == 0
        || topology.standby_port == 0
        || (topology.primary_address, topology.primary_port)
            == (topology.standby_address, topology.standby_port)
    {
        return Err(ConfirmationError::InvalidInput(
            "missing or aliased writer/replica identity",
        ));
    }
    Ok(())
}

async fn confirm_with_connections(
    primary: &PgPool,
    standby: &PgPool,
    topology: &PhysicalTopology,
    expected: &ExpectedWriter,
) -> Result<ConfirmedWalPrefix, ConfirmationError> {
    let mut primary = primary.acquire().await?;
    let mut standby = standby.acquire().await?;
    check_topology(&mut primary, &mut standby, topology, expected).await?;

    // This SELECT happens after the caller's COMMIT and is an upper bound even
    // if unrelated transactions insert WAL between COMMIT and observation.
    let row = sqlx::query(
        "SELECT pg_current_wal_insert_lsn()::text AS lsn, \
         pg_walfile_name(pg_current_wal_insert_lsn()) AS wal_file",
    )
    .fetch_one(&mut *primary)
    .await?;
    let upper_bound_lsn: String = row.try_get("lsn")?;
    let wal_file: String = row.try_get("wal_file")?;
    let timeline_id = wal_timeline(&wal_file)?;
    if timeline_id != expected.timeline_id {
        return Err(ConfirmationError::Epoch("primary WAL timeline changed"));
    }
    check_epoch(&mut primary, expected).await?;

    loop {
        let primary_ready =
            primary_progress(&mut primary, &upper_bound_lsn, topology, expected).await?;
        let standby_ready =
            standby_progress(&mut standby, &upper_bound_lsn, topology, expected).await?;
        if primary_ready && standby_ready {
            // Epoch, timeline and both WAL positions are checked a second time
            // after observing the other site, closing ordinary check-order races.
            if primary_progress(&mut primary, &upper_bound_lsn, topology, expected).await?
                && standby_progress(&mut standby, &upper_bound_lsn, topology, expected).await?
            {
                return Ok(ConfirmedWalPrefix {
                    upper_bound_lsn,
                    system_identifier: expected.system_identifier.clone(),
                    timeline_id,
                    writer_epoch: expected.writer_epoch,
                });
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn check_topology(
    primary: &mut PoolConnection<Postgres>,
    standby: &mut PoolConnection<Postgres>,
    topology: &PhysicalTopology,
    expected: &ExpectedWriter,
) -> Result<(), ConfirmationError> {
    let primary_head = server_head(primary).await?;
    let standby_head = server_head(standby).await?;
    if primary_head.version / 10_000 != 18 || standby_head.version / 10_000 != 18 {
        return Err(ConfirmationError::Topology(
            "both servers must run PostgreSQL 18",
        ));
    }
    if primary_head.recovery || !standby_head.recovery {
        return Err(ConfirmationError::Topology(
            "expected one primary and one physical standby",
        ));
    }
    if (primary_head.address, primary_head.port)
        != (topology.primary_address, topology.primary_port)
        || (standby_head.address, standby_head.port)
            != (topology.standby_address, topology.standby_port)
    {
        return Err(ConfirmationError::Topology(
            "server endpoint differs from pinned topology",
        ));
    }
    if primary_head.system_identifier != expected.system_identifier
        || standby_head.system_identifier != expected.system_identifier
    {
        return Err(ConfirmationError::Topology(
            "physical system identifier differs",
        ));
    }

    Ok(())
}

struct ServerHead {
    recovery: bool,
    version: i32,
    system_identifier: String,
    address: IpAddr,
    port: u16,
}

async fn server_head(conn: &mut PoolConnection<Postgres>) -> Result<ServerHead, ConfirmationError> {
    let row = sqlx::query(
        "SELECT pg_is_in_recovery() AS recovery, \
         current_setting('server_version_num')::integer AS version, \
         (SELECT system_identifier::text FROM pg_control_system()) AS system_identifier, \
         host(inet_server_addr()) AS address, inet_server_port() AS port",
    )
    .fetch_one(&mut **conn)
    .await?;
    let address: String = row.try_get("address")?;
    let port: i32 = row.try_get("port")?;
    Ok(ServerHead {
        recovery: row.try_get("recovery")?,
        version: row.try_get("version")?,
        system_identifier: row.try_get("system_identifier")?,
        address: address
            .parse()
            .map_err(|_| ConfirmationError::Topology("server has no pinned TCP address"))?,
        port: u16::try_from(port)
            .map_err(|_| ConfirmationError::Topology("server has invalid TCP port"))?,
    })
}

fn wal_timeline(wal_file: &str) -> Result<u32, ConfirmationError> {
    if wal_file.len() != 24 || !wal_file.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ConfirmationError::Topology("invalid primary WAL filename"));
    }
    u32::from_str_radix(&wal_file[..8], 16)
        .map_err(|_| ConfirmationError::Topology("invalid primary WAL timeline"))
}

async fn check_epoch(
    conn: &mut PoolConnection<Postgres>,
    expected: &ExpectedWriter,
) -> Result<(), ConfirmationError> {
    let row = sqlx::query(
        "SELECT writer_epoch, system_identifier, timeline_id, primary_id \
         FROM platform_durability_epoch WHERE singleton",
    )
    .fetch_optional(&mut **conn)
    .await?
    .ok_or(ConfirmationError::Epoch("writer epoch marker is absent"))?;
    if row.try_get::<i64, _>("writer_epoch")? != expected.writer_epoch
        || row.try_get::<String, _>("system_identifier")? != expected.system_identifier
        || row.try_get::<i64, _>("timeline_id")? != i64::from(expected.timeline_id)
        || row.try_get::<String, _>("primary_id")? != expected.primary_id
    {
        return Err(ConfirmationError::Epoch("writer epoch marker differs"));
    }
    Ok(())
}

async fn primary_progress(
    conn: &mut PoolConnection<Postgres>,
    upper_bound_lsn: &str,
    topology: &PhysicalTopology,
    expected: &ExpectedWriter,
) -> Result<bool, ConfirmationError> {
    let row = sqlx::query(
        "SELECT pg_is_in_recovery() AS recovery, \
         pg_current_wal_flush_lsn() >= $1::pg_lsn AS flushed, \
         pg_walfile_name(pg_current_wal_insert_lsn()) AS wal_file",
    )
    .bind(upper_bound_lsn)
    .fetch_one(&mut **conn)
    .await?;
    if row.try_get::<bool, _>("recovery")?
        || wal_timeline(&row.try_get::<String, _>("wal_file")?)? != expected.timeline_id
    {
        return Err(ConfirmationError::Topology(
            "primary changed role or timeline",
        ));
    }
    check_epoch(conn, expected).await?;
    let replication = sqlx::query(
        "SELECT state, flush_lsn >= $1::pg_lsn AS flushed, \
         replay_lsn >= $1::pg_lsn AS replayed \
         FROM pg_stat_replication WHERE application_name = $2",
    )
    .bind(upper_bound_lsn)
    .bind(&topology.standby_application_name)
    .fetch_all(&mut **conn)
    .await?;
    if replication.len() > 1 {
        return Err(ConfirmationError::Topology(
            "ambiguous standby application name",
        ));
    }
    let Some(replication) = replication.first() else {
        return Ok(false);
    };
    Ok(row.try_get::<bool, _>("flushed")?
        && replication.try_get::<String, _>("state")? == "streaming"
        && replication.try_get::<Option<bool>, _>("flushed")? == Some(true)
        && replication.try_get::<Option<bool>, _>("replayed")? == Some(true))
}

async fn standby_progress(
    conn: &mut PoolConnection<Postgres>,
    upper_bound_lsn: &str,
    topology: &PhysicalTopology,
    expected: &ExpectedWriter,
) -> Result<bool, ConfirmationError> {
    let row = sqlx::query(
        "SELECT pg_is_in_recovery() AS recovery, \
         pg_is_wal_replay_paused() AS paused, \
         pg_last_wal_replay_lsn() >= $1::pg_lsn AS replayed",
    )
    .bind(upper_bound_lsn)
    .fetch_one(&mut **conn)
    .await?;
    if !row.try_get::<bool, _>("recovery")? {
        return Err(ConfirmationError::Topology("standby was promoted"));
    }
    if row.try_get::<Option<bool>, _>("replayed")? != Some(true)
        || row.try_get::<bool, _>("paused")?
    {
        return Ok(false);
    }
    check_epoch(conn, expected).await?;
    let receivers = sqlx::query(
        "SELECT status, received_tli, flushed_lsn >= $1::pg_lsn AS flushed, \
         sender_host, sender_port FROM pg_stat_wal_receiver",
    )
    .bind(upper_bound_lsn)
    .fetch_all(&mut **conn)
    .await?;
    if receivers.len() != 1 {
        return Ok(false);
    }
    let receiver = &receivers[0];
    Ok(receiver.try_get::<String, _>("status")? == "streaming"
        && receiver.try_get::<i32, _>("received_tli")?
            == i32::try_from(expected.timeline_id)
                .map_err(|_| ConfirmationError::Topology("timeline exceeds PostgreSQL integer"))?
        && receiver.try_get::<Option<bool>, _>("flushed")? == Some(true)
        && receiver.try_get::<String, _>("sender_host")? == topology.primary_address.to_string()
        && receiver.try_get::<i32, _>("sender_port")? == i32::from(topology.primary_port))
}
