#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_attendance_domain::{AttendanceDateRange, HistoricalAbsence, SubstitutionWindow};
use time::{Date, Duration, Month};
use uuid::Uuid;

#[test]
fn selected_month_plus_d7_is_the_only_default_listing_window() {
    let range = AttendanceDateRange::selected_month_with_buffer("2026-07").unwrap();
    assert_eq!(range.from.to_string(), "2026-07-01");
    assert_eq!(range.to_exclusive.to_string(), "2026-08-08");
    assert!(AttendanceDateRange::new(range.from, range.to_exclusive + Duration::days(1)).is_err());
}

#[test]
fn historical_substitution_requires_a_full_planned_absence_interval() {
    let employee = Uuid::new_v4();
    let date = Date::from_calendar_date(2026, Month::July, 2).unwrap();
    let window = SubstitutionWindow::new(date, 540, 1020).unwrap();
    assert!(
        HistoricalAbsence::new(employee, date, 480, 1080)
            .unwrap()
            .fully_covers(employee, &window)
    );
    assert!(
        !HistoricalAbsence::new(employee, date, 600, 1020)
            .unwrap()
            .fully_covers(employee, &window)
    );
}
