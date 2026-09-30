//! Attendance contracts that are intentionally limited to the caller's linked
//! employee record.  These projections must not grow manager-facing identity,
//! branch, team, actor, or object-link fields.

use console_attendance_domain::{AttendanceDateRange, ExceptionKind, ResolutionAction};
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::{AttendanceApplicationError, AttendanceEvidence, AttendancePage, Week52Tone};

/// The authenticated principal for attendance self-service reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfAttendanceScope {
    pub org_id: Uuid,
    pub user_id: Uuid,
}

/// A self-service exception listing. The caller cannot choose an employee,
/// branch, team, manager, or object-link scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListOwnExceptions {
    pub range: AttendanceDateRange,
    pub status: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl ListOwnExceptions {
    /// Builds a date/status/page constrained query.
    pub fn new(
        range: AttendanceDateRange,
        status: Option<String>,
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
            status,
            limit: limit.unwrap_or(50).clamp(1, 200),
            offset: offset.unwrap_or(0).clamp(0, 1_000_000),
        })
    }
}

/// A self-service Week52 read. The employee is derived from the scope, never
/// accepted from a transport or a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadOwnWeek52 {
    week_start: Date,
}

impl ReadOwnWeek52 {
    /// Validates the ISO-week boundary before persistence is consulted.
    pub fn new(week_start: Date) -> Result<Self, AttendanceApplicationError> {
        crate::validate_week52_start(week_start)?;
        Ok(Self { week_start })
    }

    /// Returns the ISO Monday boundary, rejecting values constructed through
    /// deserialization or other internal bypasses of [`Self::new`].
    pub fn week_start(&self) -> Result<Date, AttendanceApplicationError> {
        crate::validate_week52_start(self.week_start)
    }
}

/// Resolution details that are safe to expose to the employee who owns the
/// exception. Deliberately excludes resolver identity and linked objects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnExceptionResolutionRead {
    pub action: ResolutionAction,
    pub reason: String,
    pub ot_hours: Option<String>,
    pub resolved_at: OffsetDateTime,
}

/// Reduced exception projection for the linked employee.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnAttendanceExceptionRead {
    pub id: Uuid,
    pub code: String,
    pub kind: ExceptionKind,
    pub status: String,
    pub work_date: Date,
    pub occurred_at: OffsetDateTime,
    pub detail: String,
    pub evidence: Vec<AttendanceEvidence>,
    pub resolution: Option<OwnExceptionResolutionRead>,
    pub created_at: OffsetDateTime,
}

/// Reduced Week52 projection for the linked employee.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OwnWeek52Read {
    pub week_start: Date,
    pub current_hours: f64,
    pub projected_hours: f64,
    pub tone: Week52Tone,
    pub acknowledged_at: Option<OffsetDateTime>,
}

pub type OwnAttendanceExceptionPage = AttendancePage<OwnAttendanceExceptionRead>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use time::{Date, Month, OffsetDateTime};

    fn json_keys(value: &serde_json::Value) -> BTreeSet<&str> {
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn own_exception_query_accepts_only_date_status_and_pagination() {
        let range = AttendanceDateRange::new(
            Date::from_calendar_date(2026, Month::July, 1).unwrap(),
            Date::from_calendar_date(2026, Month::July, 8).unwrap(),
        )
        .unwrap();
        let query =
            ListOwnExceptions::new(range, Some(" OPEN ".into()), Some(999), Some(-1)).unwrap();
        assert_eq!(query.status.as_deref(), Some("OPEN"));
        assert_eq!(query.limit, 200);
        assert_eq!(query.offset, 0);

        let at = OffsetDateTime::from_unix_timestamp(0).unwrap();
        let resolution = OwnExceptionResolutionRead {
            action: ResolutionAction::Confirm,
            reason: "confirmed".to_owned(),
            ot_hours: Some("1.50".to_owned()),
            resolved_at: at,
        };
        let own = OwnAttendanceExceptionRead {
            id: Uuid::nil(),
            code: "EX-1".to_owned(),
            kind: ExceptionKind::Late,
            status: "RESOLVED".to_owned(),
            work_date: Date::from_calendar_date(2026, Month::July, 1).unwrap(),
            occurred_at: at,
            detail: "late clock-in".to_owned(),
            evidence: Vec::new(),
            resolution: Some(resolution),
            created_at: at,
        };
        let own_json = serde_json::to_value(&own).unwrap();
        let own_keys = json_keys(&own_json);
        assert_eq!(
            own_keys,
            BTreeSet::from([
                "code",
                "created_at",
                "detail",
                "evidence",
                "id",
                "kind",
                "occurred_at",
                "resolution",
                "status",
                "work_date",
            ])
        );
        for leaked in [
            "employee_id",
            "employee_name",
            "team",
            "branch_id",
            "links",
            "resolver_id",
            "phone",
        ] {
            assert!(!own_keys.contains(leaked), "{leaked}");
        }

        let resolution_json = serde_json::to_value(own.resolution.as_ref().unwrap()).unwrap();
        let resolution_keys = json_keys(&resolution_json);
        assert_eq!(
            resolution_keys,
            BTreeSet::from(["action", "ot_hours", "reason", "resolved_at"])
        );
        for leaked in ["actor", "resolver_id", "resolver_name", "employee_id"] {
            assert!(!resolution_keys.contains(leaked), "{leaked}");
        }
    }

    #[test]
    fn own_week52_requires_an_iso_monday() {
        let monday = Date::from_calendar_date(2026, Month::July, 20).unwrap();
        assert!(ReadOwnWeek52::new(monday).is_ok());
        assert!(ReadOwnWeek52::new(monday + time::Duration::days(1)).is_err());
    }

    #[test]
    fn own_week52_accessor_rejects_a_bypassed_non_monday_input() {
        let tuesday = Date::from_calendar_date(2026, Month::July, 21).unwrap();
        let bypassed = ReadOwnWeek52 {
            week_start: tuesday,
        };
        assert!(bypassed.week_start().is_err());
    }
}
