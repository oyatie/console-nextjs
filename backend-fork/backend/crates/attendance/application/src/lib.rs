//! Attendance use-case contracts.  The adapter supplies persistence; this layer
//! fixes the business decisions so all transports share the same gates.
#![cfg_attr(test, allow(clippy::unwrap_used))]

use console_attendance_domain::{
    AttendanceDateRange, AttendanceDomainError, ExceptionKind, ResolutionAction, SubstitutionWindow,
};
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

pub mod self_service;
pub use self_service::{
    ListOwnExceptions, OwnAttendanceExceptionPage, OwnAttendanceExceptionRead,
    OwnExceptionResolutionRead, OwnWeek52Read, ReadOwnWeek52, SelfAttendanceScope,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallerScope {
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub branch_ids: Vec<Uuid>,
    pub org_wide: bool,
}
impl CallerScope {
    pub fn permits_branch(&self, branch_id: Option<Uuid>) -> bool {
        self.org_wide || branch_id.is_some_and(|id| self.branch_ids.contains(&id))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSubstitutions {
    pub range: AttendanceDateRange,
    pub branch_id: Option<Uuid>,
    pub limit: i64,
    pub offset: i64,
}
impl ListSubstitutions {
    pub fn new(
        range: AttendanceDateRange,
        branch_id: Option<Uuid>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Self {
        Self {
            range,
            branch_id,
            limit: limit.unwrap_or(50).clamp(1, 200),
            offset: offset.unwrap_or(0).clamp(0, 1_000_000),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListExceptions {
    pub range: AttendanceDateRange,
    pub branch_id: Option<Uuid>,
    pub status: Option<String>,
    pub employee_id: Option<Uuid>,
    pub limit: i64,
    pub offset: i64,
}
impl ListExceptions {
    pub fn new(
        range: AttendanceDateRange,
        branch_id: Option<Uuid>,
        status: Option<String>,
        employee_id: Option<Uuid>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Self, AttendanceApplicationError> {
        let status = status.map(|value| value.trim().to_owned());
        if status
            .as_deref()
            .is_some_and(|value| value != "OPEN" && value != "RESOLVED")
        {
            return Err(AttendanceApplicationError::InvalidText("status"));
        }
        Ok(Self {
            range,
            branch_id,
            status,
            employee_id,
            limit: limit.unwrap_or(50).clamp(1, 200),
            offset: offset.unwrap_or(0).clamp(0, 1_000_000),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaiseException {
    pub kind: ExceptionKind,
    pub employee_id: Uuid,
    pub branch_id: Option<Uuid>,
    pub work_date: Date,
    pub detail: String,
    pub evidence: Vec<AttendanceEvidence>,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveException {
    pub exception_id: Uuid,
    pub action: ResolutionAction,
    pub reason: String,
    pub linked_work_ref: Option<String>,
    pub overtime_minutes: Option<i32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignSubstitute {
    pub window: SubstitutionWindow,
    pub branch_id: Option<Uuid>,
    pub site: String,
    pub role: String,
    pub covered_employee_id: Uuid,
    pub reason_kind: String,
    pub reason_detail: Option<String>,
    pub worker_employee_id: Option<Uuid>,
    pub worker_name: String,
    pub worker_type: String,
    pub worker_rate: Option<String>,
    pub exception_id: Option<Uuid>,
    pub idempotency_key: String,
}

/// The explicit eligibility lookup required before assigning a substitute.
/// The adapter obtains the facts from canonical HR and attendance records;
/// this layer owns the decision over those facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubstitutionCandidateQuery {
    pub branch_id: Uuid,
    pub covered_employee_id: Uuid,
    pub cover_date: Date,
    pub from_minutes: i32,
    pub to_minutes: i32,
    pub search: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl SubstitutionCandidateQuery {
    pub fn new(
        branch_id: Uuid,
        covered_employee_id: Uuid,
        window: SubstitutionWindow,
        search: Option<String>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Self, AttendanceApplicationError> {
        let window =
            SubstitutionWindow::new(window.cover_date, window.from_minutes, window.to_minutes)?;
        Ok(Self {
            branch_id,
            covered_employee_id,
            cover_date: window.cover_date,
            from_minutes: window.from_minutes,
            to_minutes: window.to_minutes,
            search: normalize_optional_text(search, "search", 120)?,
            limit: limit.unwrap_or(50).clamp(1, 200),
            offset: offset.unwrap_or(0).clamp(0, 1_000_000),
        })
    }

    pub fn window(&self) -> Result<SubstitutionWindow, AttendanceApplicationError> {
        SubstitutionWindow::new(self.cover_date, self.from_minutes, self.to_minutes)
            .map_err(AttendanceApplicationError::from)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubstitutionCandidateFacts {
    pub employee_id: Uuid,
    pub employment_active: bool,
    pub home_branch_id: Option<Uuid>,
    pub conflicts_with_assigned_substitution: bool,
    pub approved_leave_covers_window: bool,
    pub has_open_no_show: bool,
}

impl SubstitutionCandidateFacts {
    #[must_use]
    pub fn is_eligible_for(&self, branch_id: Uuid, covered_employee_id: Uuid) -> bool {
        self.employment_active
            && self.home_branch_id == Some(branch_id)
            && self.employee_id != covered_employee_id
            && !self.conflicts_with_assigned_substitution
            && !self.approved_leave_covers_window
            && !self.has_open_no_show
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubstitutionCandidateRead {
    pub employee_id: Uuid,
    pub employee_name: String,
    pub branch_id: Uuid,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseMonth {
    pub month: String,
    pub branch_scope: Option<Uuid>,
    pub attest: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelSubstitution {
    pub substitution_id: Uuid,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmendClose {
    pub close_id: Uuid,
    pub reason: String,
    pub detail: String,
    pub reference: Option<String>,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcknowledgeWeek52 {
    pub employee_id: Uuid,
    pub week_start: Date,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseChecks {
    pub open_exceptions: i64,
    pub pending_leave: i64,
    pub already_closed: bool,
}
impl CloseChecks {
    #[must_use]
    pub const fn ready(&self) -> bool {
        self.open_exceptions == 0 && !self.already_closed
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Week52Input {
    pub employee_id: Uuid,
    pub week_start: Date,
    pub current_hours: f64,
    pub projected_hours: f64,
    pub acknowledged_at: Option<OffsetDateTime>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Week52Read {
    pub employee_id: Uuid,
    pub name: String,
    pub team: Option<String>,
    pub week_start: Date,
    pub current_hours: f64,
    pub projected_hours: f64,
    pub acknowledged_at: Option<OffsetDateTime>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseCheckRead {
    pub key: String,
    pub ok: bool,
    pub warn: Option<bool>,
    pub note: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseAmendmentRead {
    pub id: Uuid,
    pub reason: String,
    pub actor: Uuid,
    pub created_at: OffsetDateTime,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonthCloseRead {
    pub id: Uuid,
    pub month: Date,
    pub branch_id: Option<Uuid>,
    pub checks: Vec<CloseCheckRead>,
    pub attested_by: Uuid,
    pub attested_at: OffsetDateTime,
    pub period_lock_id: Option<Uuid>,
    pub closed_at: OffsetDateTime,
    pub amendments: Vec<CloseAmendmentRead>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClosePreflightRead {
    pub month: Date,
    pub branch_id: Option<Uuid>,
    pub checks: Vec<CloseCheckRead>,
    pub can_close: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Week52AcknowledgementRead {
    pub employee_id: Uuid,
    pub week_start: Date,
    pub acknowledged_at: OffsetDateTime,
}

/// Transport-neutral attendance read models. REST owns the snake_case wire mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttendanceEvidence {
    pub name: String,
    pub size: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttendanceObjectLink {
    pub kind: String,
    pub label: String,
    pub reference: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExceptionResolutionRead {
    pub action: ResolutionAction,
    pub reason: String,
    pub linked_work_ref: Option<String>,
    pub ot_hours: Option<String>,
    pub actor: Uuid,
    pub resolved_at: OffsetDateTime,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttendanceExceptionRead {
    pub id: Uuid,
    pub code: String,
    pub kind: ExceptionKind,
    pub status: String,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub team: Option<String>,
    pub branch_id: Option<Uuid>,
    pub work_date: Date,
    pub occurred_at: OffsetDateTime,
    pub detail: String,
    pub evidence: Vec<AttendanceEvidence>,
    pub links: Vec<AttendanceObjectLink>,
    pub resolution: Option<ExceptionResolutionRead>,
    pub created_at: OffsetDateTime,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttendanceSubstitutionRead {
    pub id: Uuid,
    pub site: String,
    pub branch_id: Option<Uuid>,
    pub role: String,
    pub cover_date: Date,
    pub from_minutes: i32,
    pub to_minutes: i32,
    pub covered_employee_id: Uuid,
    pub covered_name: String,
    pub reason_kind: String,
    pub reason_detail: Option<String>,
    pub worker_employee_id: Option<Uuid>,
    pub worker_name: String,
    pub worker_type: String,
    pub worker_rate: Option<String>,
    pub status: String,
    pub exception_id: Option<Uuid>,
    pub created_by: Uuid,
    pub created_at: OffsetDateTime,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttendancePage<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Week52Tone {
    Ok,
    Warn,
    Danger,
}
pub fn week52_tone(input: &Week52Input) -> Week52Tone {
    if input.current_hours >= 52.0 || input.projected_hours >= 52.0 {
        Week52Tone::Danger
    } else if input.projected_hours >= 48.0 {
        Week52Tone::Warn
    } else {
        Week52Tone::Ok
    }
}

pub fn validate_week52_start(week_start: Date) -> Result<Date, AttendanceApplicationError> {
    if week_start.weekday() == time::Weekday::Monday {
        Ok(week_start)
    } else {
        Err(AttendanceApplicationError::InvalidWeekStart)
    }
}

pub fn validate_idempotency_key(key: &str) -> Result<String, AttendanceApplicationError> {
    let key = key.trim();
    if !(16..=200).contains(&key.len()) {
        return Err(AttendanceApplicationError::InvalidIdempotencyKey);
    }
    Ok(key.to_owned())
}
pub fn validate_text(
    value: &str,
    name: &'static str,
    max: usize,
) -> Result<String, AttendanceApplicationError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max {
        return Err(AttendanceApplicationError::InvalidText(name));
    }
    Ok(value.to_owned())
}
pub fn normalize_optional_text(
    value: Option<String>,
    name: &'static str,
    max: usize,
) -> Result<Option<String>, AttendanceApplicationError> {
    match value {
        None => Ok(None),
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() || trimmed.chars().count() > max {
                Err(AttendanceApplicationError::InvalidText(name))
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
    }
}

pub fn ensure_scope(
    scope: &CallerScope,
    branch_id: Option<Uuid>,
) -> Result<(), AttendanceApplicationError> {
    if scope.permits_branch(branch_id) {
        Ok(())
    } else {
        Err(AttendanceApplicationError::ForbiddenBranch)
    }
}

pub fn require_worker_employee_id(
    worker_employee_id: Option<Uuid>,
) -> Result<Uuid, AttendanceApplicationError> {
    worker_employee_id.ok_or(AttendanceApplicationError::WorkerEmployeeRequired)
}

#[derive(Debug, thiserror::Error)]
pub enum AttendanceApplicationError {
    #[error(transparent)]
    Domain(#[from] AttendanceDomainError),
    #[error("Idempotency-Key must be 16..200 characters")]
    InvalidIdempotencyKey,
    #[error("{0} is required and too long")]
    InvalidText(&'static str),
    #[error("worker_employee_id is required")]
    WorkerEmployeeRequired,
    #[error("the requested branch is outside the caller scope")]
    ForbiddenBranch,
    #[error("monthly close requires explicit attestation")]
    MissingAttestation,
    #[error("open exceptions block this close")]
    CloseBlocked,
    #[error("weekStart must be an ISO Monday")]
    InvalidWeekStart,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Date, Month};
    #[test]
    fn branch_scope_never_expands() {
        let allowed = Uuid::new_v4();
        let caller = CallerScope {
            org_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            branch_ids: vec![allowed],
            org_wide: false,
        };
        assert!(ensure_scope(&caller, Some(allowed)).is_ok());
        assert!(ensure_scope(&caller, Some(Uuid::new_v4())).is_err());
        assert!(ensure_scope(&caller, None).is_err());
    }
    #[test]
    fn org_wide_scope_is_the_only_scope_that_can_omit_a_branch() {
        let caller = CallerScope {
            org_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            branch_ids: vec![],
            org_wide: true,
        };
        assert!(ensure_scope(&caller, None).is_ok());
    }
    #[test]
    fn close_requires_no_open_exception() {
        assert!(
            !CloseChecks {
                open_exceptions: 1,
                pending_leave: 0,
                already_closed: false
            }
            .ready()
        );
        assert!(
            CloseChecks {
                open_exceptions: 0,
                pending_leave: 2,
                already_closed: false
            }
            .ready()
        );
    }
    #[test]
    fn monitor_thresholds_are_server_policy() {
        let i = Week52Input {
            employee_id: Uuid::new_v4(),
            week_start: Date::from_calendar_date(2026, Month::July, 6).unwrap(),
            current_hours: 47.0,
            projected_hours: 49.0,
            acknowledged_at: None,
        };
        assert_eq!(week52_tone(&i), Week52Tone::Warn);
    }
    #[test]
    fn week52_start_must_be_an_iso_monday() {
        let monday = Date::from_calendar_date(2026, Month::July, 20).unwrap();
        assert_eq!(validate_week52_start(monday).unwrap(), monday);
        assert!(validate_week52_start(monday + time::Duration::days(1)).is_err());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdempotencyDecision {
    Create,
    Replay,
    Conflict,
}
#[must_use]
pub fn idempotency_decision(
    existing_fingerprint: Option<&str>,
    request_fingerprint: &str,
) -> IdempotencyDecision {
    match existing_fingerprint {
        None => IdempotencyDecision::Create,
        Some(existing) if existing == request_fingerprint => IdempotencyDecision::Replay,
        Some(_) => IdempotencyDecision::Conflict,
    }
}

#[cfg(test)]
mod idempotency_tests {
    use super::*;
    #[test]
    fn duplicate_assignment_is_replay_only_for_same_request() {
        assert_eq!(idempotency_decision(None, "a"), IdempotencyDecision::Create);
        assert_eq!(
            idempotency_decision(Some("a"), "a"),
            IdempotencyDecision::Replay
        );
        assert_eq!(
            idempotency_decision(Some("a"), "b"),
            IdempotencyDecision::Conflict
        );
    }
    #[test]
    fn exception_list_normalizes_status_and_pagination_bounds() {
        let range = AttendanceDateRange::new(
            time::Date::from_calendar_date(2026, time::Month::July, 1).unwrap(),
            time::Date::from_calendar_date(2026, time::Month::July, 2).unwrap(),
        )
        .unwrap();
        let defaulted =
            ListExceptions::new(range.clone(), None, Some(" OPEN ".into()), None, None, None)
                .unwrap();
        assert_eq!(defaulted.status.as_deref(), Some("OPEN"));
        assert_eq!((defaulted.limit, defaulted.offset), (50, 0));
        let clamped = ListExceptions::new(
            range.clone(),
            None,
            Some("RESOLVED".into()),
            None,
            Some(-2),
            Some(-1),
        )
        .unwrap();
        assert_eq!((clamped.limit, clamped.offset), (1, 0));
        let maximum =
            ListExceptions::new(range.clone(), None, None, None, Some(999), Some(9_999_999))
                .unwrap();
        assert_eq!((maximum.limit, maximum.offset), (200, 1_000_000));
        assert!(ListExceptions::new(range, None, Some("open".into()), None, None, None).is_err());
    }
}
