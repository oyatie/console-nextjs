//! Reads behind `GET /api/v1/payroll/employees/{id}/payslip-draft`.
//!
//! Three inputs, all real production data:
//!  * the **contract wage in force** on the pay date (`employee_contract_wages`,
//!    migration 0210) — append-only, effective-dated, never
//!    `employee_employment_profiles.base_pay`;
//!  * the period's **actual timesheet** (`employee_attendance_records`,
//!    migration 0091 — the one attendance store with a production writer,
//!    `POST /api/v1/hr/attendance-records/me`). It GATES the draft rather than
//!    driving a figure: no amount is prorated by attendance, and a timesheet
//!    that is absent, short of the period's working days, or unbalanced raises
//!    `ATTENDANCE_INCOMPLETE`;
//!  * the **statutory citations** (`payroll_statutory_rates`, migration 0210).
//!
//! Deliberately does NOT touch `payroll_draft_lines` or
//! `data_import_rows.canonical_row->'payroll'`: both have zero production
//! writers, so a run has an empty roster and calculates nothing. Building the
//! roster is the next step; it is not a prerequisite for computing one real
//! employee's 4대보험 from data that already exists.

use console_platform_db::DbError;
use serde::Serialize;
use sqlx::{Postgres, Row, Transaction};
use time::Date;
use uuid::Uuid;

use crate::PgPayrollError;

/// `time::Date`'s default serde shape is `[year, ordinal]` — unusable to an API
/// client. Same convention as `console_evaluation_application::date_fmt`.
mod iso_date {
    use serde::Serializer;
    use time::Date;
    use time::format_description::well_known::Iso8601;

    pub fn serialize<S: Serializer>(date: &Date, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(
            &date
                .format(&Iso8601::DATE)
                .map_err(serde::ser::Error::custom)?,
        )
    }
}

mod iso_date_opt {
    use serde::Serializer;
    use time::Date;

    pub fn serialize<S: Serializer>(date: &Option<Date>, ser: S) -> Result<S::Ok, S::Error> {
        match date {
            Some(date) => super::iso_date::serialize(date, ser),
            None => ser.serialize_none(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ContractWage {
    pub id: Uuid,
    #[serde(serialize_with = "iso_date::serialize")]
    pub effective_from: Date,
    pub wage_kind: String,
    pub amount_won: i64,
    pub monthly_standard_hours: i32,
    pub source_note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttendanceSummary {
    #[serde(serialize_with = "iso_date::serialize")]
    pub period_start: Date,
    #[serde(serialize_with = "iso_date::serialize")]
    pub period_end: Date,
    /// Distinct `work_date`s with at least one CLOCK_IN — the timesheet's job
    /// in this slice is to evidence attendance, not to drive arithmetic.
    pub worked_days: i64,
    pub clock_in_events: i64,
    pub clock_out_events: i64,
}

/// One row of the citation register, verbatim.
#[derive(Debug, Clone, Serialize)]
pub struct StatutoryCitation {
    pub code: String,
    #[serde(serialize_with = "iso_date::serialize")]
    pub effective_from: Date,
    #[serde(serialize_with = "iso_date_opt::serialize")]
    pub effective_to_exclusive: Option<Date>,
    pub rate_num: Option<i64>,
    pub rate_den: Option<i64>,
    pub floor_won: Option<i64>,
    pub cap_won: Option<i64>,
    pub basis: String,
    pub bearer: String,
    pub instrument_ko: String,
    pub article_ko: String,
    pub promulgation_ko: String,
    #[serde(serialize_with = "iso_date::serialize")]
    pub enforced_on: Date,
    pub source_url: String,
    #[serde(serialize_with = "iso_date::serialize")]
    pub retrieved_on: Date,
    pub provenance_ko: String,
}

/// The wage in force on `on`: the greatest `effective_from <= on`. A future-
/// dated raise is invisible until its own date, which is the whole point of
/// storing history rather than overwriting it.
pub async fn contract_wage_in_force_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    employee_id: Uuid,
    on: Date,
) -> Result<Option<ContractWage>, PgPayrollError> {
    let row = sqlx::query(
        "SELECT id, effective_from, wage_kind, amount_won, monthly_standard_hours, source_note \
         FROM employee_contract_wages \
         WHERE employee_id = $1 AND effective_from <= $2 \
         ORDER BY effective_from DESC LIMIT 1",
    )
    .bind(employee_id)
    .bind(on)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(|err| PgPayrollError::Db(DbError::Sqlx(err)))?;

    row.map(|row| {
        Ok(ContractWage {
            id: row.try_get("id")?,
            effective_from: row.try_get("effective_from")?,
            wage_kind: row.try_get("wage_kind")?,
            amount_won: row.try_get("amount_won")?,
            monthly_standard_hours: row.try_get("monthly_standard_hours")?,
            source_note: row.try_get("source_note")?,
        })
    })
    .transpose()
}

/// A new effective-dated contract wage row. One struct rather than nine
/// positional arguments, so a caller cannot transpose `amount_won` and
/// `monthly_standard_hours` and still compile.
#[derive(Debug, Clone)]
pub struct NewContractWage {
    pub org_id: Uuid,
    pub employee_id: Uuid,
    pub created_by: Uuid,
    pub effective_from: Date,
    pub wage_kind: String,
    pub amount_won: i64,
    pub monthly_standard_hours: i32,
    pub source_note: String,
}

pub async fn insert_contract_wage_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    wage: &NewContractWage,
) -> Result<Uuid, PgPayrollError> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO employee_contract_wages \
         (org_id, employee_id, effective_from, wage_kind, amount_won, monthly_standard_hours, \
          source_note, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(wage.org_id)
    .bind(wage.employee_id)
    .bind(wage.effective_from)
    .bind(&wage.wage_kind)
    .bind(wage.amount_won)
    .bind(wage.monthly_standard_hours)
    .bind(&wage.source_note)
    .bind(wage.created_by)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|err| PgPayrollError::Db(DbError::Sqlx(err)))?;
    Ok(id)
}

pub async fn attendance_summary_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    employee_id: Uuid,
    period_start: Date,
    period_end: Date,
) -> Result<AttendanceSummary, PgPayrollError> {
    let row = sqlx::query(
        "SELECT COUNT(DISTINCT work_date) FILTER (WHERE kind = 'CLOCK_IN')  AS worked_days, \
                COUNT(*)                   FILTER (WHERE kind = 'CLOCK_IN')  AS clock_in_events, \
                COUNT(*)                   FILTER (WHERE kind = 'CLOCK_OUT') AS clock_out_events \
         FROM employee_attendance_records \
         WHERE employee_id = $1 AND work_date >= $2 AND work_date <= $3",
    )
    .bind(employee_id)
    .bind(period_start)
    .bind(period_end)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|err| PgPayrollError::Db(DbError::Sqlx(err)))?;

    Ok(AttendanceSummary {
        period_start,
        period_end,
        worked_days: row.try_get("worked_days")?,
        clock_in_events: row.try_get("clock_in_events")?,
        clock_out_events: row.try_get("clock_out_events")?,
    })
}

/// Every citation in force on `pay_date`, in code order.
pub async fn statutory_citations_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    pay_date: Date,
) -> Result<Vec<StatutoryCitation>, PgPayrollError> {
    let rows = sqlx::query(
        "SELECT code, effective_from, effective_to_exclusive, rate_num, rate_den, floor_won, \
                cap_won, basis, bearer, instrument_ko, article_ko, promulgation_ko, enforced_on, \
                source_url, retrieved_on, provenance_ko \
         FROM payroll_statutory_rates \
         WHERE effective_from <= $1 \
           AND (effective_to_exclusive IS NULL OR effective_to_exclusive > $1) \
         ORDER BY code, effective_from",
    )
    .bind(pay_date)
    .fetch_all(tx.as_mut())
    .await
    .map_err(|err| PgPayrollError::Db(DbError::Sqlx(err)))?;

    rows.into_iter()
        .map(|row| {
            Ok(StatutoryCitation {
                code: row.try_get("code")?,
                effective_from: row.try_get("effective_from")?,
                effective_to_exclusive: row.try_get("effective_to_exclusive")?,
                rate_num: row.try_get("rate_num")?,
                rate_den: row.try_get("rate_den")?,
                floor_won: row.try_get("floor_won")?,
                cap_won: row.try_get("cap_won")?,
                basis: row.try_get("basis")?,
                bearer: row.try_get("bearer")?,
                instrument_ko: row.try_get("instrument_ko")?,
                article_ko: row.try_get("article_ko")?,
                promulgation_ko: row.try_get("promulgation_ko")?,
                enforced_on: row.try_get("enforced_on")?,
                source_url: row.try_get("source_url")?,
                retrieved_on: row.try_get("retrieved_on")?,
                provenance_ko: row.try_get("provenance_ko")?,
            })
        })
        .collect()
}

/// `employees.name` for the payslip header, org-scoped by RLS.
pub async fn employee_display_name_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    employee_id: Uuid,
) -> Result<Option<String>, PgPayrollError> {
    sqlx::query_scalar("SELECT name FROM employees WHERE id = $1")
        .bind(employee_id)
        .fetch_optional(tx.as_mut())
        .await
        .map_err(|err| PgPayrollError::Db(DbError::Sqlx(err)))
}
