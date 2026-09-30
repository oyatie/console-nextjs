//! 근로기준법 §60⑤ 시기변경 협의 — automatic eligibility, worker alternate dates,
//! and repeat-exercise audit rollup (charter §4-31).
//!
//! cm3 made refusal unrepresentable and introduced the terminal
//! `time_change_consult` status. This module supplies the **consult mechanics**
//! that charter §4-31 names alongside the proviso:
//!
//! 1. **요건 자동 판정** — the system judges whether granting the requested
//!    시기 would leave the branch below a coverage floor. A manager comment
//!    alone cannot open a consult.
//! 2. **대체 일자는 근로자 선택** — only the requester may propose alternate
//!    dates, and they must differ from the original 시기.
//! 3. **반복 행사 = 감사 집계** — each exercise in the same leave-year is
//!    counted; at the threshold an audit rollup flag is raised.
//!
//! Pure functions only. Persistence and the fail-closed SQL gate live in the
//! leave adapter / migration 0217.

use console_kernel_core::KernelError;
use serde::{Deserialize, Serialize};

use crate::{LeaveType, NewLeaveRequest, PartialDayPeriod};

/// Closed set of system-judged §60⑤ grounds. Free-text manager narrative is
/// never a grounds code — it may only accompany a system verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeChangeGroundsCode {
    /// Granting the request would leave fewer than [`MINIMUM_ON_DUTY`] home-branch
    /// employees available across the requested span (counting already-approved
    /// overlapping leave).
    BranchCoverageShortfall,
}

impl TimeChangeGroundsCode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BranchCoverageShortfall => "branch_coverage_shortfall",
        }
    }

    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value {
            "branch_coverage_shortfall" => Ok(Self::BranchCoverageShortfall),
            other => Err(KernelError::validation(format!(
                "unknown time-change grounds: {other}"
            ))),
        }
    }
}

/// Smallest non-vacuous on-duty floor. Catalog overrides are a later lease; the
/// domain default keeps the judgment total and automatic.
pub const MINIMUM_ON_DUTY: u32 = 1;

/// Second exercise of §60⑤ against the same subject in one leave-year raises
/// the repeat-audit rollup (charter: 반복 행사=감사 집계).
pub const TIME_CHANGE_REPEAT_AUDIT_THRESHOLD: u32 = 2;

/// Evidence the SQL gate (and tests) collect before calling
/// [`judge_time_change_eligibility`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeChangeCoverageEvidence {
    /// Count of `employees` with `home_branch_id` on the request's branch and
    /// `employment_status = 'ACTIVE'`. EXITED (or UNKNOWN) rows that still
    /// carry a home branch must not inflate this figure — the SQL gate mirrors
    /// this contract fail-closed.
    pub headcount: u32,
    /// Distinct subjects (excluding the requester's subject) with approved
    /// leave overlapping the requested span on that branch.
    pub already_out: u32,
    pub minimum_on_duty: u32,
}

impl TimeChangeCoverageEvidence {
    /// Headcount left on duty if this request were approved.
    #[must_use]
    pub fn projected_available(self) -> i64 {
        i64::from(self.headcount) - i64::from(self.already_out) - 1
    }
}

/// Automatic §60⑤ eligibility verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeChangeEligibility {
    Eligible {
        grounds: TimeChangeGroundsCode,
        evidence: TimeChangeCoverageEvidence,
    },
    Ineligible {
        evidence: TimeChangeCoverageEvidence,
    },
}

impl TimeChangeEligibility {
    #[must_use]
    pub fn is_eligible(&self) -> bool {
        matches!(self, Self::Eligible { .. })
    }
}

/// Judge whether the §60⑤ proviso applies. Manager discretion is not an input.
#[must_use]
pub fn judge_time_change_eligibility(
    evidence: TimeChangeCoverageEvidence,
) -> TimeChangeEligibility {
    if evidence.headcount == 0 {
        // No roster evidence → cannot invoke the proviso.
        return TimeChangeEligibility::Ineligible { evidence };
    }
    if evidence.projected_available() < i64::from(evidence.minimum_on_duty) {
        TimeChangeEligibility::Eligible {
            grounds: TimeChangeGroundsCode::BranchCoverageShortfall,
            evidence,
        }
    } else {
        TimeChangeEligibility::Ineligible { evidence }
    }
}

/// Validate a worker-chosen alternate 시기 against the original request.
///
/// The alternate must be a well-formed [`NewLeaveRequest`] of the same leave
/// type (and partial-day period rules), and must not restates the original
/// dates — otherwise the consult would be a no-op refusal in disguise.
pub fn validate_alternate_dates(
    original: &NewLeaveRequest,
    proposed: NewLeaveRequest,
) -> Result<NewLeaveRequest, KernelError> {
    if proposed.leave_type != original.leave_type {
        return Err(KernelError::validation(
            "alternate leave type must match the original request",
        ));
    }
    if proposed.partial_day_period != original.partial_day_period {
        return Err(KernelError::validation(
            "alternate partial-day period must match the original request",
        ));
    }
    if proposed.start_date == original.start_date && proposed.end_date == original.end_date {
        return Err(KernelError::validation(
            "alternate dates must differ from the original 시기",
        ));
    }
    Ok(proposed)
}

/// Build a [`NewLeaveRequest`] for an alternate proposal (shared validation).
pub fn alternate_leave_request(
    leave_type: LeaveType,
    start_date: console_kernel_core::Date,
    end_date: console_kernel_core::Date,
    partial_day_period: Option<PartialDayPeriod>,
) -> Result<NewLeaveRequest, KernelError> {
    NewLeaveRequest::new(leave_type, start_date, end_date, partial_day_period)
}

/// Rollup after an exercise. `exercises_in_leave_year` is the count **including**
/// the exercise just recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeChangeRepeatRollup {
    pub exercises_in_leave_year: u32,
    /// True when the charter audit aggregation should fire.
    pub audit_flag: bool,
}

#[must_use]
pub fn rollup_time_change_exercises(exercises_in_leave_year: u32) -> TimeChangeRepeatRollup {
    TimeChangeRepeatRollup {
        exercises_in_leave_year,
        audit_flag: exercises_in_leave_year >= TIME_CHANGE_REPEAT_AUDIT_THRESHOLD,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_kernel_core::Date;
    use time::Month;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), day).unwrap()
    }

    #[test]
    fn coverage_shortfall_is_eligible_when_solo_employee_would_leave_branch_empty() {
        let evidence = TimeChangeCoverageEvidence {
            headcount: 1,
            already_out: 0,
            minimum_on_duty: MINIMUM_ON_DUTY,
        };
        let verdict = judge_time_change_eligibility(evidence);
        assert!(verdict.is_eligible());
        match verdict {
            TimeChangeEligibility::Eligible { grounds, evidence } => {
                assert_eq!(grounds, TimeChangeGroundsCode::BranchCoverageShortfall);
                assert_eq!(evidence.projected_available(), 0);
            }
            TimeChangeEligibility::Ineligible { .. } => panic!("expected eligible"),
        }
    }

    #[test]
    fn ample_coverage_makes_time_change_ineligible() {
        // Charter: 요건 자동 판정 — a manager cannot open a consult when the
        // branch would still meet the on-duty floor after granting leave.
        let evidence = TimeChangeCoverageEvidence {
            headcount: 5,
            already_out: 0,
            minimum_on_duty: MINIMUM_ON_DUTY,
        };
        let verdict = judge_time_change_eligibility(evidence);
        assert!(
            !verdict.is_eligible(),
            "time_change must fail closed without coverage shortfall"
        );
    }

    #[test]
    fn overlapping_approved_leave_can_create_shortfall() {
        let evidence = TimeChangeCoverageEvidence {
            headcount: 3,
            already_out: 2,
            minimum_on_duty: MINIMUM_ON_DUTY,
        };
        // 3 - 2 - 1 = 0 < 1
        assert!(judge_time_change_eligibility(evidence).is_eligible());
    }

    #[test]
    fn zero_headcount_cannot_invoke_the_proviso() {
        let evidence = TimeChangeCoverageEvidence {
            headcount: 0,
            already_out: 0,
            minimum_on_duty: MINIMUM_ON_DUTY,
        };
        assert!(!judge_time_change_eligibility(evidence).is_eligible());
    }

    /// EXITED peers must not be counted in headcount. Domain arithmetic: one
    /// ACTIVE subject + two EXITED home-branch stamps ⇒ headcount=1 ⇒ shortfall.
    #[test]
    fn exited_peers_are_excluded_from_active_headcount_arithmetic() {
        let active_only = TimeChangeCoverageEvidence {
            headcount: 1, // ACTIVE subject only; EXITED peers omitted
            already_out: 0,
            minimum_on_duty: MINIMUM_ON_DUTY,
        };
        assert!(
            judge_time_change_eligibility(active_only).is_eligible(),
            "ACTIVE-only headcount=1 must open §60⑤ when granting would empty the branch"
        );
        let inflated_with_exited = TimeChangeCoverageEvidence {
            headcount: 3, // bug: counting EXITED peers
            already_out: 0,
            minimum_on_duty: MINIMUM_ON_DUTY,
        };
        assert!(
            !judge_time_change_eligibility(inflated_with_exited).is_eligible(),
            "control: inflated headcount refuses — SQL must not produce this for EXITED peers"
        );
    }

    #[test]
    fn worker_alternate_dates_must_differ_from_original() {
        let original =
            NewLeaveRequest::new(LeaveType::Annual, d(2026, 7, 6), d(2026, 7, 8), None).unwrap();
        let same =
            NewLeaveRequest::new(LeaveType::Annual, d(2026, 7, 6), d(2026, 7, 8), None).unwrap();
        assert!(validate_alternate_dates(&original, same).is_err());
        let moved =
            NewLeaveRequest::new(LeaveType::Annual, d(2026, 8, 3), d(2026, 8, 5), None).unwrap();
        assert!(validate_alternate_dates(&original, moved).is_ok());
    }

    #[test]
    fn worker_alternate_cannot_change_leave_type() {
        let original =
            NewLeaveRequest::new(LeaveType::Annual, d(2026, 7, 6), d(2026, 7, 6), None).unwrap();
        let half = NewLeaveRequest::new(
            LeaveType::HalfDay,
            d(2026, 8, 3),
            d(2026, 8, 3),
            Some(PartialDayPeriod::Am),
        )
        .unwrap();
        assert!(validate_alternate_dates(&original, half).is_err());
    }

    #[test]
    fn second_exercise_in_leave_year_raises_repeat_audit_flag() {
        assert!(!rollup_time_change_exercises(1).audit_flag);
        let second = rollup_time_change_exercises(2);
        assert!(second.audit_flag);
        assert_eq!(second.exercises_in_leave_year, 2);
        assert!(rollup_time_change_exercises(3).audit_flag);
    }

    #[test]
    fn grounds_code_roundtrip() {
        assert_eq!(
            TimeChangeGroundsCode::parse("branch_coverage_shortfall").unwrap(),
            TimeChangeGroundsCode::BranchCoverageShortfall
        );
        assert!(TimeChangeGroundsCode::parse("manager_says_so").is_err());
    }
}
