#![allow(clippy::unwrap_used)]

use console_compliance_domain::{LocationPing, PingVolumeBound};
use console_kernel_core::{BranchId, LocationPingId, UserId};
use time::{Duration, macros::datetime};

#[test]
fn location_ping_validates_coordinate_ranges() {
    let ping = LocationPing::new(
        LocationPingId::new(),
        UserId::new(),
        BranchId::new(),
        37.5665,
        126.9780,
        Some(8.5),
        datetime!(2026-06-12 09:00:00 UTC),
        true,
    );
    assert!(ping.is_ok());

    let invalid_latitude = LocationPing::new(
        LocationPingId::new(),
        UserId::new(),
        BranchId::new(),
        91.0,
        126.9780,
        None,
        datetime!(2026-06-12 09:00:00 UTC),
        true,
    );
    assert!(invalid_latitude.is_err());

    let invalid_longitude = LocationPing::new(
        LocationPingId::new(),
        UserId::new(),
        BranchId::new(),
        37.5665,
        181.0,
        None,
        datetime!(2026-06-12 09:00:00 UTC),
        true,
    );
    assert!(invalid_longitude.is_err());
}

#[test]
fn ping_volume_bound_caps_rows_by_on_duty_window_ping_rate_and_users() {
    let bound = PingVolumeBound::new(300, Duration::hours(8), Duration::seconds(30)).unwrap();

    assert_eq!(bound.max_rows(), 288_000);
    assert!(bound.allows(288_000));
    assert!(!bound.allows(288_001));
}
