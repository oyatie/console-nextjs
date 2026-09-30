//! Period locks (freeze windows) — the enforcement half of month-close/마감.
//!
//! A `period_locks` row with `unlocked_at IS NULL` freezes writes whose
//! business date falls inside `[period_start, period_end]` for one domain
//! (`payroll` or `accounting`). Every mutation that stamps a date must call
//! [`assert_period_open`] (single date) or [`assert_period_open_range`]
//! (period-shaped writes) inside its already-armed transaction; the check is
//! RLS-scoped, so it only ever sees the caller's own tenant locks.
//!
//! The guard fails closed with `KernelError::conflict` (HTTP 409 through every
//! domain error mapper), naming the domain and the locked window so the caller
//! can render an actionable "period closed" error.

use console_kernel_core::KernelError;
use sqlx::{Postgres, Row, Transaction};
use time::Date;
use uuid::Uuid;

use crate::error::DbError;

/// Business domains a period lock can freeze. Matches the `period_locks.domain`
/// CHECK constraint (migration 0107).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodLockDomain {
    Payroll,
    Accounting,
}

impl PeriodLockDomain {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Payroll => "payroll",
            Self::Accounting => "accounting",
        }
    }

    /// Parse a client-supplied domain string, fail-closed on anything unknown.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value {
            "payroll" => Ok(Self::Payroll),
            "accounting" => Ok(Self::Accounting),
            other => Err(KernelError::validation(format!(
                "unknown period lock domain '{other}' (expected payroll|accounting)"
            ))),
        }
    }
}

impl std::fmt::Display for PeriodLockDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Refuse the mutation when `date` falls inside an active lock for `domain`.
///
/// Runs inside the caller's armed transaction (`with_audit`/`with_audits`/
/// `with_org_conn`), so RLS confines the lookup to the current tenant and the
/// refusal rolls the whole mutation back.
pub async fn assert_period_open(
    tx: &mut Transaction<'_, Postgres>,
    domain: PeriodLockDomain,
    date: Date,
) -> Result<(), KernelError> {
    assert_period_open_range(tx, domain, date, date).await
}

/// Refuse the mutation when `[start, end]` overlaps an active lock for `domain`.
pub async fn assert_period_open_range(
    tx: &mut Transaction<'_, Postgres>,
    domain: PeriodLockDomain,
    start: Date,
    end: Date,
) -> Result<(), KernelError> {
    let lock = sqlx::query(
        "SELECT period_start, period_end FROM period_locks \
         WHERE domain = $1 AND unlocked_at IS NULL \
           AND period_start <= $3 AND period_end >= $2 \
         ORDER BY locked_at DESC LIMIT 1",
    )
    .bind(domain.as_str())
    .bind(start)
    .bind(end)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(|e| kernel_internal(DbError::Sqlx(e)))?;

    if let Some(row) = lock {
        let period_start: Date = row.try_get("period_start").map_err(sqlx_internal)?;
        let period_end: Date = row.try_get("period_end").map_err(sqlx_internal)?;
        return Err(KernelError::conflict(format!(
            "{domain} period {period_start}..{period_end} is locked; write dated {start}..{end} refused"
        )));
    }
    Ok(())
}

/// Serialize a period-lock CREATE with every gated write that re-checks the
/// freeze window, by taking one per-tenant, per-domain advisory lock on BOTH
/// sides of the race.
///
/// A gated write folds its freeze-window check into the write statement, which
/// closes the gap where a lock commits between a phase-1 read and the write's
/// snapshot. It does NOT close the smaller gap where a lock commits after the
/// write statement's snapshot but before the write commits: under READ
/// COMMITTED the two inserts touch unrelated rows and both can commit. Both the
/// lock creator and the gated writer must therefore take this same key first, so
/// the two are ordered strictly before-or-after each other and the writer either
/// sees the committed lock (refused) or commits before the lock exists (a draft
/// that predates the freeze). Transaction-scoped: released on commit/rollback.
///
/// Returns the raw driver error so both callers (`StageDraftError::Db`,
/// `AttendanceStoreError::Sql`) can convert it with their existing `From` impls.
pub async fn lock_period_lock_key(
    tx: &mut Transaction<'_, Postgres>,
    domain: PeriodLockDomain,
    org_id: Uuid,
) -> Result<(), sqlx::Error> {
    let material = format!("console.period-lock|{}|{}", domain.as_str(), org_id);
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(material)
        .execute(tx.as_mut())
        .await?;
    Ok(())
}

fn kernel_internal(err: DbError) -> KernelError {
    KernelError::internal(format!("period lock check failed: {err}"))
}

fn sqlx_internal(err: sqlx::Error) -> KernelError {
    kernel_internal(DbError::Sqlx(err))
}
