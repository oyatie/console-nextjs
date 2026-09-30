//! Postgres adapter for the payroll draft-run staging tables (migration
//! 0074: `payroll_draft_runs`, `payroll_draft_lines`).
//!
//! Read-only. These tables are *pre-calculation* readiness/staging data —
//! `payroll_draft_lines` stores work-day/hour counts and `*_source_present`
//! booleans, never a computed won amount. The real per-employee deduction
//! math lives in `console_payroll_domain::build_employee_payroll_draft`, which is
//! a pure in-memory function with no persistence anywhere in this schema.
//! Callers must not present anything read here as an issued payslip.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod lifecycle;
/// `ObjectKey::PayRun`'s canonical write port, plus the one
/// `payroll_draft_runs` statement that used to live in
/// `console-workflow-runtime-adapter-postgres`.
pub mod pay_run;
pub mod payslip_draft;
pub mod roster;

use console_kernel_core::{ErrorKind, KernelError, UserId};
use console_platform_db::{DbError, with_org_conn};
use console_platform_request_context::current_org;
use serde::Serialize;
use sqlx::{PgPool, Postgres, Row, Transaction};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 500;

#[derive(Debug, thiserror::Error)]
pub enum PgPayrollError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl PgPayrollError {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Domain(err) => err.kind,
            Self::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => ErrorKind::NotFound,
            Self::Db(_) => ErrorKind::Internal,
        }
    }
}

impl From<sqlx::Error> for PgPayrollError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

impl From<pay_run::StageDraftError> for PgPayrollError {
    fn from(err: pay_run::StageDraftError) -> Self {
        match err {
            pay_run::StageDraftError::Db(db) => Self::Db(DbError::Sqlx(db)),
            pay_run::StageDraftError::ProvenanceMismatch => Self::Domain(KernelError::conflict(
                "a payroll draft for this run and period already exists with different provenance",
            )),
            pay_run::StageDraftError::PeriodLocked => Self::Domain(KernelError::conflict(
                "payroll period is locked; draft not staged",
            )),
        }
    }
}

impl From<PgPayrollError> for KernelError {
    fn from(value: PgPayrollError) -> Self {
        match value {
            PgPayrollError::Domain(err) => err,
            PgPayrollError::Db(err) => KernelError::internal(err.to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PayrollRunSummary {
    pub id: Uuid,
    pub period_start: Date,
    pub period_end: Date,
    pub source_label: String,
    pub status: String,
    pub calculation_enabled: bool,
    pub created_by: Option<Uuid>,
    pub approved_by: Option<Uuid>,
    pub approved_at: Option<OffsetDateTime>,
    pub close_receipt: Option<serde_json::Value>,
    pub submitted_by: Option<Uuid>,
    pub submitted_at: Option<OffsetDateTime>,
    pub decided_by: Option<Uuid>,
    pub decided_at: Option<OffsetDateTime>,
    pub decision_reason: Option<String>,
    pub approval_ref: Option<Uuid>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize)]
pub struct PayrollRunPage {
    pub items: Vec<PayrollRunSummary>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PayrollRunDetail {
    pub run: PayrollRunSummary,
    pub legal_basis: serde_json::Value,
    pub source_summary: serde_json::Value,
    pub lines: Vec<PayrollLineSummary>,
    pub lines_total: i64,
    pub lines_limit: i64,
    pub lines_offset: i64,
    pub exceptions_open: i64,
    pub exceptions_total: i64,
    pub calculation: Option<lifecycle::RunCalcSummary>,
    pub disbursement: Option<lifecycle::Disbursement>,
    pub payslip_delivery: Option<lifecycle::PayslipDeliverySummary>,
}

/// One employee's readiness/staging row for a run. Deliberately has no pay
/// amount fields — the underlying table doesn't store any (see module docs).
#[derive(Debug, Clone, Serialize)]
pub struct PayrollLineSummary {
    pub id: Uuid,
    pub employee_id: Option<Uuid>,
    pub employee_display_name: String,
    pub employee_company: String,
    pub work_days: Option<f64>,
    pub regular_hours: Option<f64>,
    pub overtime_hours: Option<f64>,
    pub night_hours: Option<f64>,
    pub holiday_hours: Option<f64>,
    pub leave_used: Option<f64>,
    pub leave_remaining: Option<f64>,
    pub gross_pay_source_present: bool,
    pub net_pay_source_present: bool,
    pub nts_tax_row_status: String,
    pub calculation_status: String,
    pub blockers: serde_json::Value,
}

/// A caller's own draft-line rows across runs (self-service). Readiness only:
/// hours and `*_source_present`. Never a `*_won` amount. Trimmed of the
/// admin-only source-import bookkeeping columns.
#[derive(Debug, Clone, Serialize)]
pub struct MyPayrollLine {
    pub run_id: Uuid,
    pub period_start: Date,
    pub period_end: Date,
    pub run_status: String,
    pub calculation_status: String,
    pub work_days: Option<f64>,
    pub regular_hours: Option<f64>,
    pub overtime_hours: Option<f64>,
    pub night_hours: Option<f64>,
    pub holiday_hours: Option<f64>,
    pub leave_used: Option<f64>,
    pub leave_remaining: Option<f64>,
    pub gross_pay_source_present: bool,
    pub net_pay_source_present: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MyPayrollLinePage {
    pub items: Vec<MyPayrollLine>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// One SUBMITTED run visible to a distinct org-wide manager (inbox list only).
#[derive(Debug, Clone)]
pub struct PayrollActionInboxItem {
    pub id: Uuid,
    pub source_label: String,
    pub submitted_at: OffsetDateTime,
}

#[derive(Clone)]
pub struct PgPayrollStore {
    pool: PgPool,
}

impl std::fmt::Debug for PgPayrollStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgPayrollStore").finish_non_exhaustive()
    }
}

impl PgPayrollStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Org-scoped page of payroll draft runs, newest period first.
    ///
    /// This opens its own transaction. A caller that must combine the read
    /// with an audited write in one atomic transaction (the REST layer's
    /// "others' reads are audited" requirement) should use
    /// [`list_runs_in_tx`] against an already-armed `tx` instead — e.g.
    /// inside `console_platform_db::with_audits`.
    pub async fn list_runs(
        &self,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<PayrollRunPage, PgPayrollError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgPayrollError>(&self.pool, org, move |tx| {
            Box::pin(async move { list_runs_in_tx(tx, limit, offset).await })
        })
        .await
    }

    /// One run plus a page of its per-employee draft lines. `None` if the run
    /// doesn't exist (or belongs to another org — RLS makes the two
    /// indistinguishable, deny-by-omission).
    ///
    /// Opens its own transaction; see [`Self::list_runs`] on using
    /// [`get_run_in_tx`] instead for an atomic audited read.
    pub async fn get_run(
        &self,
        run_id: Uuid,
        lines_limit: Option<i64>,
        lines_offset: Option<i64>,
    ) -> Result<Option<PayrollRunDetail>, PgPayrollError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgPayrollError>(&self.pool, org, move |tx| {
            Box::pin(async move { get_run_in_tx(tx, run_id, lines_limit, lines_offset).await })
        })
        .await
    }

    /// The employee row linked to `user_id`, or `None` if the account has no
    /// link (mirrors `hr.rs::load_optional_linked_employee_id`).
    pub async fn linked_employee_id(
        &self,
        user_id: UserId,
    ) -> Result<Option<Uuid>, PgPayrollError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgPayrollError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let employee_id: Option<Uuid> =
                    sqlx::query_scalar("SELECT employee_id FROM users WHERE id = $1")
                        .bind(*user_id.as_uuid())
                        .fetch_optional(tx.as_mut())
                        .await?
                        .flatten();
                Ok(employee_id)
            })
        })
        .await
    }

    /// Self-scoped page of the caller's own draft-line rows across runs,
    /// newest run period first. `employee_id` must already be resolved from
    /// the authenticated principal (never accepted from client input).
    pub async fn list_my_lines(
        &self,
        employee_id: Uuid,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<MyPayrollLinePage, PgPayrollError> {
        let org = current_org().map_err(KernelError::from)?;
        let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let offset = offset.unwrap_or(0).max(0);

        let (items, total) = with_org_conn::<_, _, PgPayrollError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let total: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM payroll_draft_lines WHERE employee_id = $1",
                )
                .bind(employee_id)
                .fetch_one(tx.as_mut())
                .await?;

                // Readiness projection: hours + *_source_present. Never SELECT *_won.
                let rows = sqlx::query(
                    r#"
                    SELECT r.id AS run_id, r.period_start, r.period_end, r.status AS run_status,
                           l.calculation_status,
                           l.work_days::float8 AS work_days,
                           l.regular_hours::float8 AS regular_hours,
                           l.overtime_hours::float8 AS overtime_hours,
                           l.night_hours::float8 AS night_hours,
                           l.holiday_hours::float8 AS holiday_hours,
                           l.leave_used::float8 AS leave_used,
                           l.leave_remaining::float8 AS leave_remaining,
                           l.gross_pay_source_present, l.net_pay_source_present
                    FROM payroll_draft_lines l
                    JOIN payroll_draft_runs r ON r.id = l.run_id
                    WHERE l.employee_id = $1
                    ORDER BY r.period_start DESC, r.period_end DESC, l.id DESC
                    LIMIT $2 OFFSET $3
                    "#,
                )
                .bind(employee_id)
                .bind(limit)
                .bind(offset)
                .fetch_all(tx.as_mut())
                .await?;
                let items = rows
                    .iter()
                    .map(my_line_from_row)
                    .collect::<Result<Vec<_>, PgPayrollError>>()?;
                Ok((items, total))
            })
        })
        .await?;

        Ok(MyPayrollLinePage {
            items,
            total,
            limit,
            offset,
        })
    }

    /// Keyset page of SUBMITTED runs the caller did not submit.
    ///
    /// Capability (`PayrollRunManage` org-wide) is enforced by the inbox
    /// composition root; this query only applies the SoD and as-of predicates.
    pub async fn list_submitted_action_inbox_page(
        &self,
        caller: UserId,
        as_of: OffsetDateTime,
        after: Option<(OffsetDateTime, String)>,
        limit: i64,
    ) -> Result<(Vec<PayrollActionInboxItem>, i64, bool), PgPayrollError> {
        let limit = limit.clamp(1, 200);
        let org = current_org().map_err(KernelError::from)?;
        let caller_id = *caller.as_uuid();
        let (after_created_at, after_id) = after.map_or((None, None), |(created_at, id)| {
            (Some(created_at), Some(id))
        });
        with_org_conn::<_, _, PgPayrollError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let total: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM payroll_draft_runs \
                     WHERE status = 'SUBMITTED' \
                       AND submitted_by IS NOT NULL \
                       AND submitted_by <> $1 \
                       AND submitted_at IS NOT NULL \
                       AND submitted_at <= $2",
                )
                .bind(caller_id)
                .bind(as_of)
                .fetch_one(tx.as_mut())
                .await?;
                let rows = sqlx::query(
                    "SELECT id, source_label, submitted_at \
                     FROM payroll_draft_runs \
                     WHERE status = 'SUBMITTED' \
                       AND submitted_by IS NOT NULL \
                       AND submitted_by <> $1 \
                       AND submitted_at IS NOT NULL \
                       AND submitted_at <= $2 \
                       AND ($4::text IS NULL OR submitted_at > $3 \
                         OR (submitted_at = $3 AND ('payroll:' || id::text) > $4)) \
                     ORDER BY submitted_at ASC, ('payroll:' || id::text) ASC \
                     LIMIT $5",
                )
                .bind(caller_id)
                .bind(as_of)
                .bind(after_created_at)
                .bind(after_id.as_deref())
                .bind(limit + 1)
                .fetch_all(tx.as_mut())
                .await?;
                let has_more = i64::try_from(rows.len()).unwrap_or(0) > limit;
                let items = rows
                    .iter()
                    .take(usize::try_from(limit).unwrap_or(rows.len()))
                    .map(|row| {
                        Ok(PayrollActionInboxItem {
                            id: row.try_get("id")?,
                            source_label: row.try_get("source_label")?,
                            submitted_at: row.try_get("submitted_at")?,
                        })
                    })
                    .collect::<Result<Vec<_>, PgPayrollError>>()?;
                Ok((items, total, has_more))
            })
        })
        .await
    }

    /// Verify that an approver identity is visible in the current tenant.
    ///
    /// This is a read-only prerequisite for the future audited approval command.
    /// It intentionally performs no status update until the shared step-up,
    /// audit, immutable release-evidence, and issuance-artifact seams exist.
    pub async fn assert_approver_belongs_to_current_org(
        &self,
        approver: UserId,
    ) -> Result<(), PgPayrollError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgPayrollError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let exists: bool =
                    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)")
                        .bind(*approver.as_uuid())
                        .fetch_one(tx.as_mut())
                        .await?;
                if !exists {
                    return Err(KernelError::forbidden(
                        "payroll approver must belong to the current organization",
                    )
                    .into());
                }
                Ok(())
            })
        })
        .await
    }
}

/// Query logic behind [`PgPayrollStore::list_runs`], factored out so a
/// caller that must combine this read with an audited write in one
/// transaction (e.g. `console_platform_db::with_audits`) can run it against an
/// already-armed `tx` instead of opening a second, non-atomic transaction.
pub async fn list_runs_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<PayrollRunPage, PgPayrollError> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = offset.unwrap_or(0).max(0);

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM payroll_draft_runs")
        .fetch_one(tx.as_mut())
        .await?;
    let rows = sqlx::query(
        r#"
        SELECT id, period_start, period_end, source_label, status,
               calculation_enabled, created_by, approved_by, approved_at,
               close_receipt, submitted_by, submitted_at, decided_by, decided_at,
               decision_reason, approval_ref,
               created_at, updated_at
        FROM payroll_draft_runs
        ORDER BY period_start DESC, period_end DESC, id DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(tx.as_mut())
    .await?;
    let items = rows
        .iter()
        .map(run_summary_from_row)
        .collect::<Result<Vec<_>, PgPayrollError>>()?;

    Ok(PayrollRunPage {
        items,
        total,
        limit,
        offset,
    })
}

/// Query logic behind [`PgPayrollStore::get_run`]; see [`list_runs_in_tx`]
/// on why this takes an already-armed `tx` rather than owning its own
/// transaction.
pub async fn get_run_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    lines_limit: Option<i64>,
    lines_offset: Option<i64>,
) -> Result<Option<PayrollRunDetail>, PgPayrollError> {
    let lines_limit = lines_limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let lines_offset = lines_offset.unwrap_or(0).max(0);

    let Some(run_row) = fetch_run_row(tx, run_id).await? else {
        return Ok(None);
    };
    let run = run_summary_from_row(&run_row)?;
    let legal_basis: serde_json::Value = run_row.try_get("legal_basis")?;
    let source_summary: serde_json::Value = run_row.try_get("source_summary")?;

    let lines_total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM payroll_draft_lines WHERE run_id = $1")
            .bind(run_id)
            .fetch_one(tx.as_mut())
            .await?;

    let line_rows = sqlx::query(
        r#"
        SELECT id, employee_id, employee_display_name, employee_company,
               work_days::float8 AS work_days,
               regular_hours::float8 AS regular_hours,
               overtime_hours::float8 AS overtime_hours,
               night_hours::float8 AS night_hours,
               holiday_hours::float8 AS holiday_hours,
               leave_used::float8 AS leave_used,
               leave_remaining::float8 AS leave_remaining,
               gross_pay_source_present, net_pay_source_present,
               nts_tax_row_status, calculation_status, blockers
        FROM payroll_draft_lines
        WHERE run_id = $1
        ORDER BY employee_company, employee_display_name, id
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(run_id)
    .bind(lines_limit)
    .bind(lines_offset)
    .fetch_all(tx.as_mut())
    .await?;
    let lines = line_rows
        .iter()
        .map(line_summary_from_row)
        .collect::<Result<Vec<_>, PgPayrollError>>()?;

    let (exceptions_open, exceptions_total) = lifecycle::exception_counts_in_tx(tx, run_id).await?;
    let calculation = lifecycle::latest_calc_summary_in_tx(tx, run_id, lines_total).await?;
    let disbursement = lifecycle::get_disbursement_in_tx(tx, run_id).await?;
    let payslip_delivery = lifecycle::payslip_delivery_in_tx(tx, run_id, None, None)
        .await?
        .filter(|summary| summary.issued > 0);

    Ok(Some(PayrollRunDetail {
        run,
        legal_basis,
        source_summary,
        lines,
        lines_total,
        lines_limit,
        lines_offset,
        exceptions_open,
        exceptions_total,
        calculation,
        disbursement,
        payslip_delivery,
    }))
}

async fn fetch_run_row(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<Option<sqlx::postgres::PgRow>, PgPayrollError> {
    Ok(sqlx::query(
        r#"
        SELECT id, period_start, period_end, source_label, status,
               calculation_enabled, created_by, approved_by, approved_at,
               close_receipt, submitted_by, submitted_at, decided_by, decided_at,
               decision_reason, approval_ref,
               created_at, updated_at, legal_basis, source_summary
        FROM payroll_draft_runs
        WHERE id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(tx.as_mut())
    .await?)
}

fn run_summary_from_row(row: &sqlx::postgres::PgRow) -> Result<PayrollRunSummary, PgPayrollError> {
    Ok(PayrollRunSummary {
        id: row.try_get("id")?,
        period_start: row.try_get("period_start")?,
        period_end: row.try_get("period_end")?,
        source_label: row.try_get("source_label")?,
        status: row.try_get("status")?,
        calculation_enabled: row.try_get("calculation_enabled")?,
        created_by: row.try_get("created_by")?,
        approved_by: row.try_get("approved_by")?,
        approved_at: row.try_get("approved_at")?,
        close_receipt: row.try_get("close_receipt")?,
        submitted_by: row.try_get("submitted_by")?,
        submitted_at: row.try_get("submitted_at")?,
        decided_by: row.try_get("decided_by")?,
        decided_at: row.try_get("decided_at")?,
        decision_reason: row.try_get("decision_reason")?,
        approval_ref: row.try_get("approval_ref")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn line_summary_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<PayrollLineSummary, PgPayrollError> {
    Ok(PayrollLineSummary {
        id: row.try_get("id")?,
        employee_id: row.try_get("employee_id")?,
        employee_display_name: row.try_get("employee_display_name")?,
        employee_company: row.try_get("employee_company")?,
        work_days: row.try_get("work_days")?,
        regular_hours: row.try_get("regular_hours")?,
        overtime_hours: row.try_get("overtime_hours")?,
        night_hours: row.try_get("night_hours")?,
        holiday_hours: row.try_get("holiday_hours")?,
        leave_used: row.try_get("leave_used")?,
        leave_remaining: row.try_get("leave_remaining")?,
        gross_pay_source_present: row.try_get("gross_pay_source_present")?,
        net_pay_source_present: row.try_get("net_pay_source_present")?,
        nts_tax_row_status: row.try_get("nts_tax_row_status")?,
        calculation_status: row.try_get("calculation_status")?,
        blockers: row.try_get("blockers")?,
    })
}

fn my_line_from_row(row: &sqlx::postgres::PgRow) -> Result<MyPayrollLine, PgPayrollError> {
    Ok(MyPayrollLine {
        run_id: row.try_get("run_id")?,
        period_start: row.try_get("period_start")?,
        period_end: row.try_get("period_end")?,
        run_status: row.try_get("run_status")?,
        calculation_status: row.try_get("calculation_status")?,
        work_days: row.try_get("work_days")?,
        regular_hours: row.try_get("regular_hours")?,
        overtime_hours: row.try_get("overtime_hours")?,
        night_hours: row.try_get("night_hours")?,
        holiday_hours: row.try_get("holiday_hours")?,
        leave_used: row.try_get("leave_used")?,
        leave_remaining: row.try_get("leave_remaining")?,
        gross_pay_source_present: row.try_get("gross_pay_source_present")?,
        net_pay_source_present: row.try_get("net_pay_source_present")?,
    })
}
