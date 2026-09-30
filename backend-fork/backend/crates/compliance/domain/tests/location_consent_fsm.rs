#![allow(clippy::unwrap_used)]

use console_compliance_domain::{LocationConsent, LocationConsentState};
use console_kernel_core::{BranchId, ErrorKind, UserId};
use time::macros::datetime;

#[test]
fn consent_lifecycle_accepts_legal_edges_and_records_timestamps() {
    let user_id = UserId::new();
    let branch_id = BranchId::new();
    let mut consent = LocationConsent::unrecorded(user_id, branch_id);

    let granted_at = datetime!(2026-06-12 09:00:00 UTC);
    let grant = consent.grant(granted_at).unwrap();
    assert_eq!(grant.from, LocationConsentState::NoRecord);
    assert_eq!(grant.to, LocationConsentState::Granted);
    assert_eq!(consent.state(), LocationConsentState::Granted);
    assert_eq!(consent.granted_at(), Some(granted_at));

    let suspended_at = datetime!(2026-06-12 10:00:00 UTC);
    let suspend = consent.suspend(suspended_at).unwrap();
    assert_eq!(suspend.from, LocationConsentState::Granted);
    assert_eq!(suspend.to, LocationConsentState::Suspended);
    assert_eq!(consent.suspended_at(), Some(suspended_at));

    let resumed_at = datetime!(2026-06-12 11:00:00 UTC);
    let resume = consent.resume(resumed_at).unwrap();
    assert_eq!(resume.from, LocationConsentState::Suspended);
    assert_eq!(resume.to, LocationConsentState::Granted);
    assert_eq!(consent.suspended_at(), None);
    assert_eq!(consent.resumed_at(), Some(resumed_at));

    let withdrawn_at = datetime!(2026-06-12 12:00:00 UTC);
    let withdraw = consent.withdraw(withdrawn_at).unwrap();
    assert_eq!(withdraw.from, LocationConsentState::Granted);
    assert_eq!(withdraw.to, LocationConsentState::Withdrawn);
    assert_eq!(consent.withdrawn_at(), Some(withdrawn_at));
}

#[test]
fn consent_lifecycle_rejects_illegal_edges() {
    let mut consent = LocationConsent::unrecorded(UserId::new(), BranchId::new());

    let err = consent
        .suspend(datetime!(2026-06-12 09:00:00 UTC))
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidTransition);

    consent.grant(datetime!(2026-06-12 10:00:00 UTC)).unwrap();
    let err = consent
        .grant(datetime!(2026-06-12 10:01:00 UTC))
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidTransition);

    consent
        .withdraw(datetime!(2026-06-12 10:02:00 UTC))
        .unwrap();
    let err = consent
        .resume(datetime!(2026-06-12 10:03:00 UTC))
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidTransition);

    let regrant = consent.grant(datetime!(2026-06-13 09:00:00 UTC)).unwrap();
    assert_eq!(regrant.from, LocationConsentState::Withdrawn);
    assert_eq!(regrant.to, LocationConsentState::Granted);
}
