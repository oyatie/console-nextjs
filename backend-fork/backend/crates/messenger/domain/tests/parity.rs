//! Parity taxonomy + presence derivation (pure domain logic).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_messenger_domain::{
    PRESENCE_AWAY_SECONDS, PRESENCE_ONLINE_SECONDS, PresenceStatus, ThreadKind, ThreadVisibility,
    presence_status_for_age,
};

#[test]
fn visibility_db_roundtrip() {
    for v in [ThreadVisibility::Channel, ThreadVisibility::Direct] {
        assert_eq!(ThreadVisibility::from_db_str(v.as_db_str()).unwrap(), v);
    }
    assert!(ThreadVisibility::from_db_str("nonsense").is_err());
}

#[test]
fn default_visibility_only_named_team_is_a_channel() {
    // A named team thread is the one that defaults to a channel.
    assert_eq!(
        ThreadVisibility::default_for(ThreadKind::Team, true),
        ThreadVisibility::Channel
    );
    // Untitled team, DMs, groups, and work-order threads default to direct.
    assert_eq!(
        ThreadVisibility::default_for(ThreadKind::Team, false),
        ThreadVisibility::Direct
    );
    for kind in [ThreadKind::Dm, ThreadKind::Group, ThreadKind::WorkOrder] {
        assert_eq!(
            ThreadVisibility::default_for(kind, true),
            ThreadVisibility::Direct
        );
    }
}

#[test]
fn presence_derives_from_activity_age() {
    assert_eq!(presence_status_for_age(None), PresenceStatus::Offline);
    assert_eq!(presence_status_for_age(Some(-3)), PresenceStatus::Online); // clock skew
    assert_eq!(presence_status_for_age(Some(0)), PresenceStatus::Online);
    assert_eq!(
        presence_status_for_age(Some(PRESENCE_ONLINE_SECONDS - 1)),
        PresenceStatus::Online
    );
    assert_eq!(
        presence_status_for_age(Some(PRESENCE_ONLINE_SECONDS)),
        PresenceStatus::Away
    );
    assert_eq!(
        presence_status_for_age(Some(PRESENCE_AWAY_SECONDS - 1)),
        PresenceStatus::Away
    );
    assert_eq!(
        presence_status_for_age(Some(PRESENCE_AWAY_SECONDS)),
        PresenceStatus::Offline
    );
}
