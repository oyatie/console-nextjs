//! `GET /api/v1/payroll/employees/{employee_id}/payslip-draft` — one real
//! employee's 급여명세서 draft, computed from the contract wage in force, every
//! figure carrying its instrument.
//!
//! The period's real timesheet is read and **gates** the draft — it does not
//! yet drive a figure. No amount here is prorated by attendance; a 근태기록 that
//! is absent, SHORT OF the period's working days, or unbalanced raises
//! `ATTENDANCE_INCOMPLETE` instead, so the draft cannot look complete on data it
//! never read. 근로기준법 제56조's 연장·야간·휴일 premiums are what will make it
//! arithmetic.
//!
//! # Why this is not the run lifecycle
//!
//! `payroll_draft_lines` has zero production INSERT and the close preflight
//! blocks on attendance counters nothing increments, so the run path cannot
//! reach real data yet. This route reaches it sideways and READ-ONLY: no
//! writes, no roster, no lifecycle. It shares `console_payroll_domain`'s single
//! arithmetic path with `calculate_run_in_tx`, so the run path inherits the
//! same won the moment it gets a roster.
//!
//! # Why 200 and not 409 when blocked
//!
//! The response always carries `issuable` and a `blockers[]` array. A caller
//! needs to see WHICH instrument is missing and WHICH question is unresolved;
//! a 409 with an opaque message hides exactly that. `issuable` is false in this
//! slice for every employee, because withholding is not computed.
//!
//! # Audit
//!
//! Compensation-adjacent read of ANOTHER person's data, so it is org-wide
//! gated and itself an audited event, exactly like `/runs` and `/runs/{id}`.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get, routing::post};
use console_kernel_core::{AuditAction, AuditEvent, TraceContext};
use console_payroll_adapter_postgres::payslip_draft::{
    AttendanceSummary, ContractWage, NewContractWage, StatutoryCitation, attendance_summary_in_tx,
    contract_wage_in_force_in_tx, employee_display_name_in_tx, insert_contract_wage_in_tx,
    statutory_citations_in_tx,
};
use console_payroll_domain::{
    Instrument, StatutoryComponent, StatutoryInsuranceInput, build_statutory_insurance_draft,
};
use console_platform_authz::{Action, Feature, Principal, authorize_org_wide};
use console_platform_db::with_audits;
use serde::Deserialize;
use serde_json::{Value, json};
use time::{Date, Month};
use uuid::Uuid;

use crate::{PayrollRestState, RestError, principal_from_headers};

pub const PAYROLL_EMPLOYEE_PAYSLIP_DRAFT_PATH_TEMPLATE: &str =
    "/api/v1/payroll/employees/{employee_id}/payslip-draft";
pub const PAYROLL_EMPLOYEE_CONTRACT_WAGES_PATH_TEMPLATE: &str =
    "/api/v1/payroll/employees/{employee_id}/contract-wages";

pub(crate) fn routes() -> Router<PayrollRestState> {
    Router::new()
        .route(
            PAYROLL_EMPLOYEE_PAYSLIP_DRAFT_PATH_TEMPLATE,
            get(get_payslip_draft),
        )
        .route(
            PAYROLL_EMPLOYEE_CONTRACT_WAGES_PATH_TEMPLATE,
            post(create_contract_wage),
        )
}

#[derive(Debug, Deserialize)]
pub(crate) struct PayslipDraftParams {
    /// 급여계산기간, `YYYY-MM`.
    period: String,
    /// 지급일, `YYYY-MM-DD`. Defaults to the period end.
    ///
    /// It is a SEPARATE input from the period on purpose: the pay date selects
    /// the rate row AND the 기준소득월액 band, so a June period paid in July
    /// must not silently reuse the June band.
    pay_date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateContractWageRequest {
    effective_from: String,
    wage_kind: String,
    amount_won: i64,
    monthly_standard_hours: i32,
    #[serde(default)]
    source_note: String,
}

fn require_payroll_manage(principal: &Principal) -> Result<(), RestError> {
    authorize_org_wide(principal, Action::new(Feature::PayrollRunManage))
        .map_err(RestError::from_kernel)
}

fn parse_date(value: &str, field: &str) -> Result<Date, RestError> {
    Date::parse(
        value,
        &time::macros::format_description!("[year]-[month]-[day]"),
    )
    .map_err(|_| {
        RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            format!("{field} must be YYYY-MM-DD"),
        )
    })
}

/// `YYYY-MM` → the exact calendar month. Half-open end is derived, never
/// guessed: February and the 31-day months must both be right.
fn parse_period(value: &str) -> Result<(Date, Date), RestError> {
    let invalid = || {
        RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "period must be YYYY-MM",
        )
    };
    let (year, month) = value.split_once('-').ok_or_else(invalid)?;
    // Shape first. `"26-07"` otherwise parses as year 26 AD and produces a
    // confusing downstream "missing payroll rate for 0026-07-31" instead of
    // naming the malformed input.
    if year.len() != 4
        || month.len() != 2
        || !year.bytes().all(|b| b.is_ascii_digit())
        || !month.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid());
    }
    let year: i32 = year.parse().map_err(|_| invalid())?;
    let month: u8 = month.parse().map_err(|_| invalid())?;
    let month = Month::try_from(month).map_err(|_| invalid())?;
    let start = Date::from_calendar_date(year, month, 1).map_err(|_| invalid())?;
    let end = start
        .replace_day(start.month().length(start.year()))
        .map_err(|_| invalid())?;
    Ok((start, end))
}

async fn create_contract_wage(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Path(employee_id): Path<Uuid>,
    Json(body): Json<CreateContractWageRequest>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_payroll_manage(&principal)?;

    if !matches!(body.wage_kind.as_str(), "MONTHLY" | "HOURLY") {
        return Err(RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "wage_kind must be MONTHLY or HOURLY",
        ));
    }
    if body.amount_won <= 0 {
        return Err(RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "amount_won must be positive",
        ));
    }
    if body.monthly_standard_hours <= 0 {
        return Err(RestError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "monthly_standard_hours must be positive",
        ));
    }
    let effective_from = parse_date(&body.effective_from, "effective_from")?;

    let org = principal.org_id;
    let actor = principal.user_id;
    let pool = state.store.pool().clone();
    let id = with_audits::<_, Uuid, RestError>(&pool, org, move |tx| {
        Box::pin(async move {
            let id = insert_contract_wage_in_tx(
                tx,
                &NewContractWage {
                    org_id: *org.as_uuid(),
                    employee_id,
                    created_by: *actor.as_uuid(),
                    effective_from,
                    wage_kind: body.wage_kind,
                    amount_won: body.amount_won,
                    monthly_standard_hours: body.monthly_standard_hours,
                    source_note: body.source_note,
                },
            )
            .await
            .map_err(RestError::from_store)?;
            let event = AuditEvent::new(
                Some(actor),
                AuditAction::new("payroll_contract_wage.create").map_err(RestError::from_kernel)?,
                "employee_contract_wage",
                id.to_string(),
                TraceContext::generate(),
                time::OffsetDateTime::now_utc(),
            )
            .with_org(org);
            Ok((id, vec![event]))
        })
    })
    .await?;

    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

async fn get_payslip_draft(
    State(state): State<PayrollRestState>,
    headers: HeaderMap,
    Path(employee_id): Path<Uuid>,
    Query(params): Query<PayslipDraftParams>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_payroll_manage(&principal)?;

    let (period_start, period_end) = parse_period(&params.period)?;
    let pay_date = match params.pay_date.as_deref() {
        Some(value) => parse_date(value, "pay_date")?,
        None => period_end,
    };

    let org = principal.org_id;
    let actor = principal.user_id;
    let pool = state.store.pool().clone();

    type Loaded = (
        Option<String>,
        Option<ContractWage>,
        AttendanceSummary,
        Vec<StatutoryCitation>,
    );
    let (name, wage, attendance, citations) =
        with_audits::<_, Loaded, RestError>(&pool, org, move |tx| {
            Box::pin(async move {
                let name = employee_display_name_in_tx(tx, employee_id)
                    .await
                    .map_err(RestError::from_store)?;
                let wage = contract_wage_in_force_in_tx(tx, employee_id, pay_date)
                    .await
                    .map_err(RestError::from_store)?;
                let attendance =
                    attendance_summary_in_tx(tx, employee_id, period_start, period_end)
                        .await
                        .map_err(RestError::from_store)?;
                let citations = statutory_citations_in_tx(tx, pay_date)
                    .await
                    .map_err(RestError::from_store)?;
                // Audit only a real employee read — a miss carries no payload.
                let events = if name.is_some() {
                    vec![
                        AuditEvent::new(
                            Some(actor),
                            AuditAction::new("payroll_payslip_draft.read")
                                .map_err(RestError::from_kernel)?,
                            "employee",
                            employee_id.to_string(),
                            TraceContext::generate(),
                            time::OffsetDateTime::now_utc(),
                        )
                        .with_org(org),
                    ]
                } else {
                    Vec::new()
                };
                Ok(((name, wage, attendance, citations), events))
            })
        })
        .await?;

    let Some(name) = name else {
        return Err(RestError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "employee not found",
        ));
    };

    // No contract wage in force is a BLOCKER, not a zero payslip and not a 404:
    // the employee exists, the engine simply has no wage to compute from.
    let Some(wage) = wage else {
        return Ok(Json(json!({
            "employee_id": employee_id,
            "employee_name": name,
            "period": { "start": period_start.to_string(), "end": period_end.to_string() },
            "pay_date": pay_date.to_string(),
            "contract": Value::Null,
            "attendance": attendance,
            "issuable": false,
            "blockers": ["CONTRACT_WAGE_NOT_IN_FORCE"],
            "statutory_citations": citations,
        }))
        .into_response());
    };

    if wage.wage_kind != "MONTHLY" {
        // 시급제 needs hour-driven pay items, which this slice does not model.
        // Refusing is the honest answer; multiplying an hourly rate by the f64
        // hours cast would be the dishonest one.
        return Ok(Json(json!({
            "employee_id": employee_id,
            "employee_name": name,
            "period": { "start": period_start.to_string(), "end": period_end.to_string() },
            "pay_date": pay_date.to_string(),
            "contract": wage,
            "attendance": attendance,
            "issuable": false,
            "blockers": ["HOURLY_WAGE_NOT_SUPPORTED_IN_THIS_SLICE"],
            "statutory_citations": citations,
        }))
        .into_response());
    }

    let draft = build_statutory_insurance_draft(&StatutoryInsuranceInput {
        pay_date,
        monthly_remuneration_won: wage.amount_won,
        pension_standard_monthly_income_won: None,
        monthly_standard_hours: Some(wage.monthly_standard_hours),
    })
    .map_err(RestError::from_kernel)?;

    // The timesheet must be load-bearing or not claimed. It drives no FIGURE in
    // this slice — 근로기준법 제56조 premiums are the next increment — so it
    // carries the only weight it honestly can: the same refusal shape
    // withholding uses. An absent, PARTIAL or unbalanced 근태기록 blocks the
    // draft instead of silently yielding a full month.
    let mut blockers = draft.blockers.clone();
    let attendance_blocker = attendance_blocker(&attendance);
    let attendance_is_complete = attendance_blocker.is_none();
    blockers.extend(attendance_blocker);

    Ok(Json(json!({
        "employee_id": employee_id,
        "employee_name": name,
        "period": { "start": period_start.to_string(), "end": period_end.to_string() },
        "pay_date": pay_date.to_string(),
        "contract": wage,
        "attendance": attendance,
        "earnings": [{
            "code": "BASE_PAY",
            "label_ko": "기본급",
            "amount_won": draft.gross_won,
            "note_ko": "근로계약상 월 기본급, 완전출근 기준 (일할계산 없음)",
        }],
        "gross_won": draft.gross_won,
        "deductions": draft
            .components
            .iter()
            .map(component_json)
            .collect::<Vec<_>>(),
        "not_computed": draft
            .not_computed
            .iter()
            .map(|row| json!({
                "code": format!("{:?}", row.code),
                "label_ko": row.label_ko,
                "reason_ko": row.reason_ko,
                "instrument": instrument_json(&row.instrument),
            }))
            .collect::<Vec<_>>(),
        "minimum_wage_check": {
            "hourly_floor_won": draft.minimum_wage.hourly_floor_won,
            "monthly_209h_floor_won": draft.minimum_wage.monthly_209h_floor_won,
            "monthly_standard_hours": draft.minimum_wage.monthly_standard_hours,
            "effective_hourly_won": draft.minimum_wage.effective_hourly_won,
            "passes": draft.minimum_wage.passes,
            "instrument": instrument_json(&draft.minimum_wage.instrument),
        },
        "total_employee_insurance_won": draft.total_employee_insurance_won,
        "remainder_after_insurance_won": draft.remainder_after_insurance_won,
        "net_pay_won": Value::Null,
        "net_pay_unavailable_reason_ko":
            "원천징수(소득세·지방소득세)가 산정되지 않아 차인지급액을 산출할 수 없다",
        "issuable": draft.issuable && attendance_is_complete,
        "blockers": blockers,
        "statutory_citations": citations,
        "compliance_notice_ko":
            "이 초안은 법적 준거성을 주장하지 않는다. 각 수치는 근거 문서·조문·시행일자와 함께 제시되며, 자격 있는 노무사/세무사의 검토 전에는 지급에 사용할 수 없다.",
    }))
    .into_response())
}

/// Weekdays in the period — the working days a full month could have.
///
/// 공휴일 are NOT modelled and neither is the contract's own 소정근로일, so this
/// OVER-counts in any month carrying a public holiday. That is the only
/// direction it is allowed to err: over-counting makes the blocker fire on a
/// timesheet that was actually complete, while under-counting would let a short
/// month pass as full. The draft is allowed to refuse too often; it is not
/// allowed to look finished on data it never read.
// ponytail: weekday count, not a 근로일 calendar. Replace with the 공휴일 table
// and the contract's 소정근로일 when this stops gating and starts prorating.
fn expected_working_days(period_start: Date, period_end: Date) -> i64 {
    core::iter::successors(Some(period_start), |day| day.next_day())
        .take_while(|day| *day <= period_end)
        .filter(|day| day.weekday().number_days_from_monday() < 5)
        .count() as i64
}

/// `ATTENDANCE_INCOMPLETE` when the period's timesheet cannot support a payslip.
///
/// Two conditions, both readable straight off the stored records:
///  * **fewer worked days than the period has working days** — including the
///    zero-record extreme. A month with 12 of 23 days recorded is not "absent",
///    but it is not a full month either, and the earnings line asserts 완전출근;
///  * **unbalanced punches** — a CLOCK_IN without its CLOCK_OUT is an open or
///    missing shift, and 근로기준법 제56조's 연장·야간·휴일 premiums (the next
///    increment) are derived from exactly those pairs.
///
/// Deliberately NOT a figure. This slice does not 일할계산 and does not invent
/// a proration from `worked_days`; it refuses, and names why. Silently
/// computing a full month on a partial timesheet is the failure this prevents.
fn attendance_blocker(attendance: &AttendanceSummary) -> Option<String> {
    let AttendanceSummary {
        period_start,
        period_end,
        worked_days,
        clock_in_events,
        clock_out_events,
    } = *attendance;
    let expected = expected_working_days(period_start, period_end);
    if worked_days < expected {
        return Some(format!(
            "ATTENDANCE_INCOMPLETE: 기록된 근무일이 기간의 소정근로일에 미달한다 \
             (worked_days={worked_days}, expected_working_days={expected}, \
             clock_in={clock_in_events}, clock_out={clock_out_events}) \
             — 완전출근을 가정하지 않고 일할계산도 하지 않는다"
        ));
    }
    if clock_in_events != clock_out_events {
        return Some(format!(
            "ATTENDANCE_INCOMPLETE: 출근·퇴근 기록 수가 일치하지 않는다 \
             (worked_days={worked_days}, clock_in={clock_in_events}, \
             clock_out={clock_out_events})"
        ));
    }
    None
}

fn component_json(component: &StatutoryComponent) -> Value {
    json!({
        "code": format!("{:?}", component.code),
        "label_ko": component.label_ko,
        "basis_kind": format!("{:?}", component.basis),
        "basis_won": component.basis_won,
        "rate_num": component.rate_num,
        "rate_den": component.rate_den,
        "total_won": component.total_won,
        "employee_won": component.employee_won,
        "employer_only": component.employer_only,
        "blocked_by": component.blocked_by,
        "total_rounding": component.total_rounding_unit,
        "employee_rounding": component.employee_rounding_unit,
        "instrument": instrument_json(&component.instrument),
        "share_instrument": component.share_instrument.as_ref().map(instrument_json),
        "provenance_ko": component.provenance,
    })
}

fn instrument_json(instrument: &Instrument) -> Value {
    json!({
        "name_ko": instrument.name_ko,
        "article_ko": instrument.article_ko,
        "promulgation_ko": instrument.promulgation_ko,
        "enforced_on": instrument.enforced_on.to_string(),
        "url": instrument.url,
        "retrieved_on": instrument.retrieved_on.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn period_end_is_derived_not_assumed_to_be_the_31st() {
        // Getting this wrong silently shortens or lengthens the timesheet window
        // the payslip evidences attendance from.
        assert_eq!(
            parse_period("2026-07").unwrap(),
            (date!(2026 - 07 - 01), date!(2026 - 07 - 31))
        );
        assert_eq!(
            parse_period("2026-02").unwrap(),
            (date!(2026 - 02 - 01), date!(2026 - 02 - 28))
        );
        // Leap year.
        assert_eq!(
            parse_period("2028-02").unwrap(),
            (date!(2028 - 02 - 01), date!(2028 - 02 - 29))
        );
        assert_eq!(
            parse_period("2026-04").unwrap(),
            (date!(2026 - 04 - 01), date!(2026 - 04 - 30))
        );
    }

    #[test]
    fn a_malformed_period_is_rejected_rather_than_coerced() {
        for bad in ["2026", "2026-13", "2026-00", "26-07", "2026-7x", ""] {
            assert_eq!(
                parse_period(bad).unwrap_err().status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{bad} must be refused"
            );
        }
    }

    fn summary(worked_days: i64, clock_in: i64, clock_out: i64) -> AttendanceSummary {
        AttendanceSummary {
            period_start: date!(2026 - 07 - 01),
            period_end: date!(2026 - 07 - 31),
            worked_days,
            clock_in_events: clock_in,
            clock_out_events: clock_out,
        }
    }

    #[test]
    fn an_absent_partial_or_unbalanced_timesheet_blocks_and_a_complete_one_does_not() {
        // Zero records must NOT read as a full month.
        let blocker = attendance_blocker(&summary(0, 0, 0)).expect("zero attendance must block");
        assert!(blocker.starts_with("ATTENDANCE_INCOMPLETE"), "{blocker}");
        assert!(blocker.contains("worked_days=0"), "{blocker}");
        assert!(blocker.contains("expected_working_days=23"), "{blocker}");

        // PARTIAL BUT BALANCED — the case that used to sail through. Every
        // punch is paired, so the unbalanced check passes; only the shortfall
        // catches it, and the earnings line asserts 완전출근.
        let blocker =
            attendance_blocker(&summary(12, 12, 12)).expect("a half-recorded month must block");
        assert!(blocker.contains("worked_days=12"), "{blocker}");
        assert!(blocker.contains("expected_working_days=23"), "{blocker}");

        // One day short is still short.
        assert!(attendance_blocker(&summary(22, 22, 22)).is_some());

        // An unmatched punch is an open shift, not a worked day.
        let blocker = attendance_blocker(&summary(23, 23, 22)).expect("unbalanced must block");
        assert!(blocker.contains("clock_in=23"), "{blocker}");
        assert!(blocker.contains("clock_out=22"), "{blocker}");

        // GC-2026-07-KR-MONTHLY-A's timesheet: 23 weekdays, paired punches.
        assert_eq!(attendance_blocker(&summary(23, 23, 23)), None);
    }

    #[test]
    fn expected_working_days_counts_weekdays_and_not_calendar_days() {
        // July 2026: 23 weekdays out of 31 — the golden case's own month.
        assert_eq!(
            expected_working_days(date!(2026 - 07 - 01), date!(2026 - 07 - 31)),
            23
        );
        // February 2026 (28 days, starts Sunday) — 20 weekdays.
        assert_eq!(
            expected_working_days(date!(2026 - 02 - 01), date!(2026 - 02 - 28)),
            20
        );
        // A single Saturday has no working day at all, and the loop terminates.
        assert_eq!(
            expected_working_days(date!(2026 - 07 - 04), date!(2026 - 07 - 04)),
            0
        );
    }

    #[test]
    fn a_malformed_pay_date_is_rejected() {
        assert_eq!(
            parse_date("2026-08-10", "pay_date").unwrap(),
            date!(2026 - 08 - 10)
        );
        for bad in ["2026-08", "10/08/2026", "2026-13-01", ""] {
            assert_eq!(
                parse_date(bad, "pay_date").unwrap_err().status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{bad} must be refused"
            );
        }
    }
}
