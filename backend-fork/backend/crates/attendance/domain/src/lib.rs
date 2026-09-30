//! Attendance value objects and invariants.  This crate is deliberately pure:
//! it knows neither HTTP, authentication, nor SQL.
#![cfg_attr(test, allow(clippy::unwrap_used))]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use time::{Date, Duration, Month, OffsetDateTime};
use uuid::Uuid;

pub const MAX_SUBSTITUTION_RANGE_DAYS: i64 = 38; // selected month plus D+7

/// An attendance event relevant to strict clock-pair duration derivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrictDurationEvent {
    pub employee_id: Uuid,
    pub occurred_at: OffsetDateTime,
    pub id: Uuid,
    pub kind: StrictDurationEventKind,
}

/// Only clock events affect the strict duration state machine. Other attendance
/// events remain in the timeline but do not open or close a clock pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrictDurationEventKind {
    ClockIn,
    ClockOut,
    Other,
}

/// A half-open time period used when clipping complete clock pairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrictDurationWindow {
    pub start: OffsetDateTime,
    pub end: OffsetDateTime,
}

impl StrictDurationWindow {
    /// # Errors
    ///
    /// Returns [`AttendanceDomainError::InvalidStrictDurationWindow`] when the
    /// end is not after the start.
    pub fn new(start: OffsetDateTime, end: OffsetDateTime) -> Result<Self, AttendanceDomainError> {
        if end <= start {
            return Err(AttendanceDomainError::InvalidStrictDurationWindow);
        }
        Ok(Self { start, end })
    }
}

/// Derives per-employee seconds from complete, strict CLOCK_IN/CLOCK_OUT pairs.
///
/// Events are ordered by employee ID, occurrence time, then event ID. Every
/// CLOCK_IN must be closed by exactly one CLOCK_OUT; any malformed employee
/// timeline fails the entire window. Complete pairs are clipped to `window`.
///
/// # Errors
///
/// Returns an error for a malformed pair sequence, negative duration, or an
/// accumulated duration that cannot fit in `i64` seconds.
pub fn strict_pair_seconds(
    events: &[StrictDurationEvent],
    window: StrictDurationWindow,
) -> Result<BTreeMap<Uuid, i64>, AttendanceDomainError> {
    let mut open = BTreeMap::<Uuid, OffsetDateTime>::new();
    let mut seconds = BTreeMap::<Uuid, i64>::new();
    let mut ordered_events = events.to_vec();
    ordered_events.sort_by_key(|event| (event.employee_id, event.occurred_at, event.id));

    for event in ordered_events {
        match event.kind {
            StrictDurationEventKind::ClockIn => {
                if open.insert(event.employee_id, event.occurred_at).is_some() {
                    return Err(AttendanceDomainError::RepeatedClockIn);
                }
            }
            StrictDurationEventKind::ClockOut => {
                let start = open
                    .remove(&event.employee_id)
                    .ok_or(AttendanceDomainError::UnmatchedClockOut)?;
                let elapsed = (event.occurred_at - start).whole_seconds();
                if elapsed < 0 {
                    return Err(AttendanceDomainError::NegativeStrictDuration);
                }
                let clipped_start = start.max(window.start);
                let clipped_end = event.occurred_at.min(window.end);
                let clipped_seconds = strict_clipped_seconds(clipped_start, clipped_end)?;
                if clipped_seconds > 0 {
                    let entry = seconds.entry(event.employee_id).or_default();
                    *entry = strict_add_seconds(*entry, clipped_seconds)?;
                }
            }
            StrictDurationEventKind::Other => {}
        }
    }

    if open.is_empty() {
        Ok(seconds)
    } else {
        Err(AttendanceDomainError::OpenClockIn)
    }
}

fn strict_add_seconds(current: i64, additional: i64) -> Result<i64, AttendanceDomainError> {
    current
        .checked_add(additional)
        .ok_or(AttendanceDomainError::StrictDurationOverflow)
}

fn strict_clipped_seconds(
    start: OffsetDateTime,
    end: OffsetDateTime,
) -> Result<i64, AttendanceDomainError> {
    let seconds = (end - start).whole_seconds();
    if seconds < 0 {
        Err(AttendanceDomainError::NegativeStrictDuration)
    } else {
        Ok(seconds)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExceptionKind {
    Late,
    NoShow,
    UnapprovedOvertime,
    EarlyLeave,
}

impl ExceptionKind {
    pub fn parse(value: &str) -> Result<Self, AttendanceDomainError> {
        match value {
            "LATE" => Ok(Self::Late),
            "NO_SHOW" => Ok(Self::NoShow),
            "UNAPPROVED_OVERTIME" => Ok(Self::UnapprovedOvertime),
            "EARLY_LEAVE" => Ok(Self::EarlyLeave),
            _ => Err(AttendanceDomainError::InvalidExceptionKind),
        }
    }
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Late => "LATE",
            Self::NoShow => "NO_SHOW",
            Self::UnapprovedOvertime => "UNAPPROVED_OVERTIME",
            Self::EarlyLeave => "EARLY_LEAVE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttendanceDateRange {
    pub from: Date,
    pub to_exclusive: Date,
}

impl AttendanceDateRange {
    pub fn new(from: Date, to_exclusive: Date) -> Result<Self, AttendanceDomainError> {
        if to_exclusive <= from || (to_exclusive - from).whole_days() > MAX_SUBSTITUTION_RANGE_DAYS
        {
            return Err(AttendanceDomainError::RangeOutOfBounds);
        }
        Ok(Self { from, to_exclusive })
    }
    pub fn selected_month_with_buffer(month: &str) -> Result<Self, AttendanceDomainError> {
        let (year, raw_month) = month
            .split_once('-')
            .ok_or(AttendanceDomainError::InvalidMonth)?;
        let year = year
            .parse::<i32>()
            .map_err(|_| AttendanceDomainError::InvalidMonth)?;
        let month = raw_month
            .parse::<u8>()
            .ok()
            .and_then(|m| Month::try_from(m).ok())
            .ok_or(AttendanceDomainError::InvalidMonth)?;
        let from = Date::from_calendar_date(year, month, 1)
            .map_err(|_| AttendanceDomainError::InvalidMonth)?;
        let next = if month == Month::December {
            Date::from_calendar_date(year + 1, Month::January, 1)
        } else {
            Date::from_calendar_date(year, month.next(), 1)
        }
        .map_err(|_| AttendanceDomainError::InvalidMonth)?;
        Self::new(from, next + Duration::days(7))
    }
    #[must_use]
    pub fn includes(&self, date: Date) -> bool {
        date >= self.from && date < self.to_exclusive
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubstitutionWindow {
    pub cover_date: Date,
    pub from_minutes: i32,
    pub to_minutes: i32,
}
impl SubstitutionWindow {
    pub fn new(
        cover_date: Date,
        from_minutes: i32,
        to_minutes: i32,
    ) -> Result<Self, AttendanceDomainError> {
        if !(0..=1440).contains(&from_minutes)
            || !(1..=1440).contains(&to_minutes)
            || to_minutes <= from_minutes
        {
            return Err(AttendanceDomainError::InvalidCoverageWindow);
        }
        Ok(Self {
            cover_date,
            from_minutes,
            to_minutes,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoricalAbsence {
    pub employee_id: Uuid,
    pub work_date: Date,
    pub from_minutes: i32,
    pub to_minutes: i32,
}
impl HistoricalAbsence {
    pub fn new(
        employee_id: Uuid,
        work_date: Date,
        from_minutes: i32,
        to_minutes: i32,
    ) -> Result<Self, AttendanceDomainError> {
        if !(0..=1440).contains(&from_minutes)
            || !(1..=1440).contains(&to_minutes)
            || to_minutes <= from_minutes
        {
            return Err(AttendanceDomainError::InvalidAbsenceInterval);
        }
        Ok(Self {
            employee_id,
            work_date,
            from_minutes,
            to_minutes,
        })
    }
    #[must_use]
    pub fn fully_covers(&self, employee_id: Uuid, window: &SubstitutionWindow) -> bool {
        self.employee_id == employee_id
            && self.work_date == window.cover_date
            && self.from_minutes <= window.from_minutes
            && self.to_minutes >= window.to_minutes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResolutionAction {
    Confirm,
    ApproveOvertime,
}
impl ResolutionAction {
    pub fn parse(value: &str) -> Result<Self, AttendanceDomainError> {
        match value {
            "CONFIRM" => Ok(Self::Confirm),
            "APPROVE_OVERTIME" => Ok(Self::ApproveOvertime),
            _ => Err(AttendanceDomainError::InvalidResolutionAction),
        }
    }
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Confirm => "CONFIRM",
            Self::ApproveOvertime => "APPROVE_OVERTIME",
        }
    }
    pub fn validate_for(
        self,
        kind: ExceptionKind,
        linked_work_ref: Option<&str>,
        overtime_minutes: Option<i32>,
    ) -> Result<(), AttendanceDomainError> {
        match (kind, self) {
            (ExceptionKind::UnapprovedOvertime, Self::ApproveOvertime)
                if linked_work_ref.is_some_and(|v| !v.trim().is_empty())
                    && overtime_minutes.is_some_and(|v| v > 0) =>
            {
                Ok(())
            }
            (ExceptionKind::UnapprovedOvertime, _) => {
                Err(AttendanceDomainError::InvalidResolutionTransition)
            }
            (_, Self::Confirm) if linked_work_ref.is_none() && overtime_minutes.is_none() => Ok(()),
            _ => Err(AttendanceDomainError::InvalidResolutionTransition),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AttendanceDomainError {
    #[error("strict duration window must be positive")]
    InvalidStrictDurationWindow,
    #[error("clock-in was repeated before the previous clock-out")]
    RepeatedClockIn,
    #[error("clock-out has no matching clock-in")]
    UnmatchedClockOut,
    #[error("clock-in has no matching clock-out")]
    OpenClockIn,
    #[error("clock pair has a negative duration")]
    NegativeStrictDuration,
    #[error("strict duration exceeds supported seconds")]
    StrictDurationOverflow,
    #[error("month must be YYYY-MM")]
    InvalidMonth,
    #[error("range must be positive and no longer than selected month plus D+7")]
    RangeOutOfBounds,
    #[error("coverage window must be within a day and non-empty")]
    InvalidCoverageWindow,
    #[error("exception kind is not supported")]
    InvalidExceptionKind,
    #[error("absence interval must be within a day and non-empty")]
    InvalidAbsenceInterval,
    #[error("resolution action is not supported")]
    InvalidResolutionAction,
    #[error("resolution action is invalid for this exception kind")]
    InvalidResolutionTransition,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strict_event(
        employee_id: Uuid,
        id: u128,
        kind: StrictDurationEventKind,
        hours: i64,
    ) -> StrictDurationEvent {
        StrictDurationEvent {
            employee_id,
            occurred_at: OffsetDateTime::UNIX_EPOCH + Duration::hours(hours),
            id: Uuid::from_u128(id),
            kind,
        }
    }

    fn strict_window() -> StrictDurationWindow {
        StrictDurationWindow::new(
            OffsetDateTime::UNIX_EPOCH,
            OffsetDateTime::UNIX_EPOCH + Duration::days(7),
        )
        .unwrap()
    }

    #[test]
    fn strict_pairs_sort_reversed_input_and_ignore_intermediate_events() {
        let employee = Uuid::from_u128(1);
        let seconds = strict_pair_seconds(
            &[
                strict_event(employee, 3, StrictDurationEventKind::ClockOut, 9),
                strict_event(employee, 2, StrictDurationEventKind::Other, 1),
                strict_event(employee, 1, StrictDurationEventKind::ClockIn, 0),
            ],
            strict_window(),
        )
        .unwrap();
        assert_eq!(seconds.get(&employee), Some(&(9 * 60 * 60)));
    }

    #[test]
    fn strict_pairs_order_same_time_events_by_id() {
        let employee = Uuid::from_u128(1);
        let seconds = strict_pair_seconds(
            &[
                strict_event(employee, 2, StrictDurationEventKind::ClockOut, 0),
                strict_event(employee, 1, StrictDurationEventKind::ClockIn, 0),
            ],
            strict_window(),
        )
        .unwrap();
        assert!(!seconds.contains_key(&employee));
    }

    #[test]
    fn strict_pairs_clip_boundary_crossings_and_ignore_zero_length_pairs() {
        let employee = Uuid::from_u128(1);
        let window = StrictDurationWindow::new(
            OffsetDateTime::UNIX_EPOCH + Duration::days(7),
            OffsetDateTime::UNIX_EPOCH + Duration::days(14),
        )
        .unwrap();
        let seconds = strict_pair_seconds(
            &[
                strict_event(employee, 1, StrictDurationEventKind::ClockIn, 7 * 24 - 2),
                strict_event(employee, 2, StrictDurationEventKind::ClockOut, 7 * 24 + 3),
                strict_event(employee, 3, StrictDurationEventKind::ClockIn, 10 * 24),
                strict_event(employee, 4, StrictDurationEventKind::ClockOut, 10 * 24),
                strict_event(employee, 5, StrictDurationEventKind::ClockIn, 14 * 24 - 3),
                strict_event(employee, 6, StrictDurationEventKind::ClockOut, 14 * 24 + 2),
            ],
            window,
        )
        .unwrap();
        assert_eq!(seconds.get(&employee), Some(&(6 * 60 * 60)));
    }

    #[test]
    fn strict_pairs_fail_the_entire_window_for_malformed_sequences() {
        let employee = Uuid::from_u128(1);
        assert_eq!(
            strict_pair_seconds(
                &[strict_event(
                    employee,
                    1,
                    StrictDurationEventKind::ClockOut,
                    1
                )],
                strict_window(),
            ),
            Err(AttendanceDomainError::UnmatchedClockOut)
        );
        assert_eq!(
            strict_pair_seconds(
                &[
                    strict_event(employee, 1, StrictDurationEventKind::ClockIn, 0),
                    strict_event(employee, 2, StrictDurationEventKind::ClockIn, 1),
                ],
                strict_window(),
            ),
            Err(AttendanceDomainError::RepeatedClockIn)
        );
        assert_eq!(
            strict_pair_seconds(
                &[strict_event(
                    employee,
                    1,
                    StrictDurationEventKind::ClockIn,
                    0
                )],
                strict_window(),
            ),
            Err(AttendanceDomainError::OpenClockIn)
        );
    }

    #[test]
    fn strict_pairs_isolate_employees_but_fail_if_any_timeline_is_invalid() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        let seconds = strict_pair_seconds(
            &[
                strict_event(second, 4, StrictDurationEventKind::ClockOut, 4),
                strict_event(first, 2, StrictDurationEventKind::ClockOut, 3),
                strict_event(second, 3, StrictDurationEventKind::ClockIn, 2),
                strict_event(first, 1, StrictDurationEventKind::ClockIn, 1),
            ],
            strict_window(),
        )
        .unwrap();
        assert_eq!(seconds.get(&first), Some(&(2 * 60 * 60)));
        assert_eq!(seconds.get(&second), Some(&(2 * 60 * 60)));
        assert_eq!(
            strict_pair_seconds(
                &[
                    strict_event(first, 1, StrictDurationEventKind::ClockIn, 1),
                    strict_event(first, 2, StrictDurationEventKind::ClockOut, 2),
                    strict_event(second, 3, StrictDurationEventKind::ClockOut, 3),
                ],
                strict_window(),
            ),
            Err(AttendanceDomainError::UnmatchedClockOut)
        );
    }

    #[test]
    fn strict_duration_window_rejects_invalid_bounds() {
        let at = OffsetDateTime::UNIX_EPOCH;
        assert_eq!(
            StrictDurationWindow::new(at, at),
            Err(AttendanceDomainError::InvalidStrictDurationWindow)
        );
    }

    #[test]
    fn strict_duration_arithmetic_rejects_negative_and_overflow() {
        let at = OffsetDateTime::UNIX_EPOCH;
        assert_eq!(
            strict_clipped_seconds(at, at - Duration::seconds(1)),
            Err(AttendanceDomainError::NegativeStrictDuration)
        );
        assert_eq!(
            strict_add_seconds(i64::MAX, 1),
            Err(AttendanceDomainError::StrictDurationOverflow)
        );
    }
    #[test]
    fn selected_month_is_explicit_and_bounded() {
        let r = AttendanceDateRange::selected_month_with_buffer("2026-07").unwrap();
        assert_eq!(r.from.to_string(), "2026-07-01");
        assert_eq!(r.to_exclusive.to_string(), "2026-08-08");
        assert!(AttendanceDateRange::new(r.from, r.to_exclusive + Duration::days(1)).is_err());
    }
    #[test]
    fn historical_coverage_requires_full_same_day_interval() {
        let employee = Uuid::new_v4();
        let date = Date::from_calendar_date(2026, Month::July, 2).unwrap();
        let window = SubstitutionWindow::new(date, 540, 1020).unwrap();
        assert!(
            HistoricalAbsence::new(employee, date, 480, 1080)
                .unwrap()
                .fully_covers(employee, &window)
        );
        assert!(
            !HistoricalAbsence::new(employee, date, 541, 1020)
                .unwrap()
                .fully_covers(employee, &window)
        );
    }
    #[test]
    fn overtime_resolution_has_a_kind_action_matrix() {
        assert!(
            ResolutionAction::ApproveOvertime
                .validate_for(ExceptionKind::UnapprovedOvertime, Some("WO-1"), Some(60))
                .is_ok()
        );
        assert!(
            ResolutionAction::Confirm
                .validate_for(ExceptionKind::UnapprovedOvertime, None, None)
                .is_err()
        );
        assert!(
            ResolutionAction::ApproveOvertime
                .validate_for(ExceptionKind::Late, Some("WO-1"), Some(60))
                .is_err()
        );
    }
}
