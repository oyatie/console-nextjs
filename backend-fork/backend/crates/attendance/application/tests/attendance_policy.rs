#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_attendance_application::{
    CallerScope, CloseChecks, IdempotencyDecision, SubstitutionCandidateFacts,
    SubstitutionCandidateQuery, ensure_scope, idempotency_decision, require_worker_employee_id,
};
use console_attendance_domain::SubstitutionWindow;
use time::{Date, Month};
use uuid::Uuid;

#[test]
fn branch_authority_cannot_be_widened_by_requested_branch() {
    let allowed = Uuid::new_v4();
    let caller = CallerScope {
        org_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        branch_ids: vec![allowed],
        org_wide: false,
    };
    assert!(ensure_scope(&caller, Some(allowed)).is_ok());
    assert!(ensure_scope(&caller, Some(Uuid::new_v4())).is_err());
}

#[test]
fn duplicate_assignment_is_only_a_replay_when_the_fingerprint_matches() {
    assert_eq!(
        idempotency_decision(Some("same"), "same"),
        IdempotencyDecision::Replay
    );
    assert_eq!(
        idempotency_decision(Some("same"), "changed"),
        IdempotencyDecision::Conflict
    );
}

#[test]
fn close_gate_rejects_open_exceptions() {
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
            pending_leave: 1,
            already_closed: false
        }
        .ready()
    );
}

#[test]
fn branch_limited_caller_cannot_omit_branch_or_read_another_branch() {
    let allowed = Uuid::new_v4();
    let other = Uuid::new_v4();
    let caller = CallerScope {
        org_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        branch_ids: vec![allowed],
        org_wide: false,
    };
    assert!(ensure_scope(&caller, None).is_err());
    assert!(ensure_scope(&caller, Some(allowed)).is_ok());
    assert!(ensure_scope(&caller, Some(other)).is_err());
}

#[test]
fn org_wide_caller_may_query_all_branches() {
    let caller = CallerScope {
        org_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        branch_ids: vec![],
        org_wide: true,
    };
    assert!(ensure_scope(&caller, None).is_ok());
    assert!(ensure_scope(&caller, Some(Uuid::new_v4())).is_ok());
}

#[test]
fn substitution_candidate_query_normalizes_search_and_pagination() {
    let query = SubstitutionCandidateQuery::new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        SubstitutionWindow::new(
            Date::from_calendar_date(2026, Month::July, 2).unwrap(),
            480,
            960,
        )
        .unwrap(),
        Some("  Kim  ".into()),
        Some(999),
        Some(-1),
    )
    .unwrap();
    assert_eq!(query.search.as_deref(), Some("Kim"));
    assert_eq!(query.limit, 200);
    assert_eq!(query.offset, 0);
}

#[test]
fn substitution_candidate_eligibility_rejects_every_disqualifying_fact() {
    let branch = Uuid::new_v4();
    let worker = Uuid::new_v4();
    let covered = Uuid::new_v4();
    let eligible = SubstitutionCandidateFacts {
        employee_id: worker,
        employment_active: true,
        home_branch_id: Some(branch),
        conflicts_with_assigned_substitution: false,
        approved_leave_covers_window: false,
        has_open_no_show: false,
    };
    assert!(eligible.is_eligible_for(branch, covered));
    for ineligible in [
        SubstitutionCandidateFacts {
            employment_active: false,
            ..eligible
        },
        SubstitutionCandidateFacts {
            home_branch_id: None,
            ..eligible
        },
        SubstitutionCandidateFacts {
            conflicts_with_assigned_substitution: true,
            ..eligible
        },
        SubstitutionCandidateFacts {
            approved_leave_covers_window: true,
            ..eligible
        },
        SubstitutionCandidateFacts {
            has_open_no_show: true,
            ..eligible
        },
        SubstitutionCandidateFacts {
            employee_id: covered,
            ..eligible
        },
    ] {
        assert!(!ineligible.is_eligible_for(branch, covered));
    }
}

#[test]
fn substitution_assignment_requires_an_employee_worker() {
    assert!(require_worker_employee_id(None).is_err());
    assert!(require_worker_employee_id(Some(Uuid::new_v4())).is_ok());
}
