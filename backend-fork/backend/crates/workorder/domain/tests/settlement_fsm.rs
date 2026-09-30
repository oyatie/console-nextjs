#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Settlement FSM legal-edge coverage (design-contract §3.2).

use console_kernel_core::ErrorKind;
use console_workorder_domain::{
    SETTLEMENT_ELIGIBLE_WORK_ORDER_STATUSES, SETTLEMENT_TRANSITIONS, SettlementStatus,
    WorkOrderStatus, validate_settlement_transition,
};

#[test]
fn every_declared_edge_validates() {
    for (from, to) in SETTLEMENT_TRANSITIONS {
        let transition = validate_settlement_transition(*from, *to).unwrap();
        assert_eq!(transition.from, *from);
        assert_eq!(transition.to, *to);
    }
}

#[test]
fn approved_and_void_are_terminal() {
    for terminal in [SettlementStatus::Approved, SettlementStatus::Void] {
        for to in [
            SettlementStatus::Draft,
            SettlementStatus::Submitted,
            SettlementStatus::Approved,
            SettlementStatus::Void,
        ] {
            let err = validate_settlement_transition(terminal, to).unwrap_err();
            assert_eq!(err.kind, ErrorKind::InvalidTransition);
        }
    }
}

#[test]
fn draft_cannot_jump_to_approved() {
    let err = validate_settlement_transition(SettlementStatus::Draft, SettlementStatus::Approved)
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidTransition);
}

#[test]
fn returned_submission_goes_back_to_draft() {
    let transition =
        validate_settlement_transition(SettlementStatus::Submitted, SettlementStatus::Draft)
            .unwrap();
    assert_eq!(transition.to, SettlementStatus::Draft);
}

#[test]
fn settlement_eligibility_covers_report_review_and_final_only() {
    assert_eq!(
        SETTLEMENT_ELIGIBLE_WORK_ORDER_STATUSES,
        &[
            WorkOrderStatus::ReportSubmitted,
            WorkOrderStatus::AdminReview,
            WorkOrderStatus::FinalCompleted,
        ]
    );
}
