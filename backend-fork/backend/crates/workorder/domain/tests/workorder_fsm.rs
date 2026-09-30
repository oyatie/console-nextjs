#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use console_kernel_core::ErrorKind;
use console_workorder_domain::{
    ALL_WORK_ORDER_STATUSES, TransitionActor, TransitionGuardContext, WORK_ORDER_TRANSITIONS,
    WorkOrderStatus, validate_status_transition,
};

const EXPECTED_LEGAL_EDGES: &[(WorkOrderStatus, WorkOrderStatus)] = &[
    (WorkOrderStatus::Received, WorkOrderStatus::Assigned),
    (WorkOrderStatus::Unassigned, WorkOrderStatus::Assigned),
    (WorkOrderStatus::Received, WorkOrderStatus::InProgress),
    (WorkOrderStatus::Unassigned, WorkOrderStatus::InProgress),
    (WorkOrderStatus::Assigned, WorkOrderStatus::InProgress),
    (
        WorkOrderStatus::InProgress,
        WorkOrderStatus::ReportSubmitted,
    ),
    (
        WorkOrderStatus::ReportSubmitted,
        WorkOrderStatus::AdminReview,
    ),
    (
        WorkOrderStatus::AdminReview,
        WorkOrderStatus::FinalCompleted,
    ),
    (
        WorkOrderStatus::AdminReview,
        WorkOrderStatus::TemporaryAction,
    ),
    (WorkOrderStatus::Assigned, WorkOrderStatus::Delayed),
    (WorkOrderStatus::InProgress, WorkOrderStatus::Delayed),
    (WorkOrderStatus::OnHold, WorkOrderStatus::Delayed),
    (WorkOrderStatus::PartWaiting, WorkOrderStatus::Delayed),
    (WorkOrderStatus::EquipmentInUse, WorkOrderStatus::Delayed),
    (WorkOrderStatus::RevisitRequired, WorkOrderStatus::Delayed),
    (WorkOrderStatus::Assigned, WorkOrderStatus::OnHold),
    (WorkOrderStatus::InProgress, WorkOrderStatus::OnHold),
    (WorkOrderStatus::TemporaryAction, WorkOrderStatus::OnHold),
    (WorkOrderStatus::OnHold, WorkOrderStatus::Assigned),
    (WorkOrderStatus::OnHold, WorkOrderStatus::InProgress),
    (WorkOrderStatus::InProgress, WorkOrderStatus::PartWaiting),
    (WorkOrderStatus::Assigned, WorkOrderStatus::PartWaiting),
    (WorkOrderStatus::PartWaiting, WorkOrderStatus::InProgress),
    (WorkOrderStatus::InProgress, WorkOrderStatus::EquipmentInUse),
    (WorkOrderStatus::Assigned, WorkOrderStatus::EquipmentInUse),
    (WorkOrderStatus::EquipmentInUse, WorkOrderStatus::InProgress),
    (
        WorkOrderStatus::TemporaryAction,
        WorkOrderStatus::RevisitRequired,
    ),
    (
        WorkOrderStatus::InProgress,
        WorkOrderStatus::RevisitRequired,
    ),
    (WorkOrderStatus::RevisitRequired, WorkOrderStatus::Assigned),
    (
        WorkOrderStatus::RevisitRequired,
        WorkOrderStatus::InProgress,
    ),
    (WorkOrderStatus::Delayed, WorkOrderStatus::Assigned),
    (WorkOrderStatus::Delayed, WorkOrderStatus::InProgress),
    (
        WorkOrderStatus::TemporaryAction,
        WorkOrderStatus::InProgress,
    ),
    (WorkOrderStatus::Received, WorkOrderStatus::Rejected),
    (WorkOrderStatus::Unassigned, WorkOrderStatus::Rejected),
    (WorkOrderStatus::Assigned, WorkOrderStatus::Rejected),
    (WorkOrderStatus::InProgress, WorkOrderStatus::Rejected),
    (WorkOrderStatus::ReportSubmitted, WorkOrderStatus::Rejected),
    (WorkOrderStatus::AdminReview, WorkOrderStatus::Rejected),
    (WorkOrderStatus::OnHold, WorkOrderStatus::Rejected),
    (WorkOrderStatus::Delayed, WorkOrderStatus::Rejected),
    (WorkOrderStatus::TemporaryAction, WorkOrderStatus::Rejected),
    (WorkOrderStatus::PartWaiting, WorkOrderStatus::Rejected),
    (WorkOrderStatus::EquipmentInUse, WorkOrderStatus::Rejected),
    (WorkOrderStatus::RevisitRequired, WorkOrderStatus::Rejected),
    (WorkOrderStatus::Received, WorkOrderStatus::Cancelled),
    (WorkOrderStatus::Unassigned, WorkOrderStatus::Cancelled),
    (WorkOrderStatus::Assigned, WorkOrderStatus::Cancelled),
    (WorkOrderStatus::InProgress, WorkOrderStatus::Cancelled),
    (WorkOrderStatus::ReportSubmitted, WorkOrderStatus::Cancelled),
    (WorkOrderStatus::AdminReview, WorkOrderStatus::Cancelled),
    (WorkOrderStatus::OnHold, WorkOrderStatus::Cancelled),
    (WorkOrderStatus::Delayed, WorkOrderStatus::Cancelled),
    (WorkOrderStatus::TemporaryAction, WorkOrderStatus::Cancelled),
    (WorkOrderStatus::PartWaiting, WorkOrderStatus::Cancelled),
    (WorkOrderStatus::EquipmentInUse, WorkOrderStatus::Cancelled),
    (WorkOrderStatus::RevisitRequired, WorkOrderStatus::Cancelled),
    (WorkOrderStatus::Received, WorkOrderStatus::Archived),
    (WorkOrderStatus::Unassigned, WorkOrderStatus::Archived),
    (WorkOrderStatus::Assigned, WorkOrderStatus::Archived),
    (WorkOrderStatus::InProgress, WorkOrderStatus::Archived),
    (WorkOrderStatus::ReportSubmitted, WorkOrderStatus::Archived),
    (WorkOrderStatus::AdminReview, WorkOrderStatus::Archived),
    (WorkOrderStatus::FinalCompleted, WorkOrderStatus::Archived),
    (WorkOrderStatus::Rejected, WorkOrderStatus::Archived),
    (WorkOrderStatus::OnHold, WorkOrderStatus::Archived),
    (WorkOrderStatus::Delayed, WorkOrderStatus::Archived),
    (WorkOrderStatus::TemporaryAction, WorkOrderStatus::Archived),
    (WorkOrderStatus::PartWaiting, WorkOrderStatus::Archived),
    (WorkOrderStatus::EquipmentInUse, WorkOrderStatus::Archived),
    (WorkOrderStatus::RevisitRequired, WorkOrderStatus::Archived),
    (WorkOrderStatus::Cancelled, WorkOrderStatus::Archived),
];

#[test]
fn transition_matrix_asserts_all_256_cells() {
    assert_eq!(ALL_WORK_ORDER_STATUSES.len(), 16);

    let expected: BTreeSet<_> = EXPECTED_LEGAL_EDGES.iter().copied().collect();
    let guard_context = TransitionGuardContext {
        actor: TransitionActor::Admin,
        approval_line_complete: true,
        completion_evidence_verified: true,
    };

    let mut checked = 0;
    for from in ALL_WORK_ORDER_STATUSES {
        for to in ALL_WORK_ORDER_STATUSES {
            checked += 1;
            let result = validate_status_transition(*from, *to, guard_context);
            assert_eq!(
                result.is_ok(),
                expected.contains(&(*from, *to)),
                "{from:?} -> {to:?}"
            );
        }
    }

    assert_eq!(checked, 256);
}

#[test]
fn explicit_transition_table_matches_expected_edges_and_documents_rationale() {
    let expected: BTreeSet<_> = EXPECTED_LEGAL_EDGES.iter().copied().collect();
    let table: BTreeSet<_> = WORK_ORDER_TRANSITIONS
        .iter()
        .map(|rule| (rule.from, rule.to))
        .collect();

    assert_eq!(table, expected);
    assert_eq!(WORK_ORDER_TRANSITIONS.len(), expected.len());
    for rule in WORK_ORDER_TRANSITIONS {
        assert!(
            !rule.rationale.trim().is_empty(),
            "{:?} -> {:?} must document why the edge exists",
            rule.from,
            rule.to
        );
    }
}

#[test]
fn guarded_transitions_reject_missing_authority_or_invariants() {
    let non_admin = TransitionGuardContext {
        actor: TransitionActor::Mechanic,
        approval_line_complete: true,
        completion_evidence_verified: true,
    };
    let err = validate_status_transition(
        WorkOrderStatus::FinalCompleted,
        WorkOrderStatus::Archived,
        non_admin,
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Forbidden);

    let incomplete_approval = TransitionGuardContext {
        actor: TransitionActor::Admin,
        approval_line_complete: false,
        completion_evidence_verified: true,
    };
    let err = validate_status_transition(
        WorkOrderStatus::AdminReview,
        WorkOrderStatus::TemporaryAction,
        incomplete_approval,
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);

    let unverified_evidence = TransitionGuardContext {
        actor: TransitionActor::Admin,
        approval_line_complete: true,
        completion_evidence_verified: false,
    };
    let err = validate_status_transition(
        WorkOrderStatus::AdminReview,
        WorkOrderStatus::FinalCompleted,
        unverified_evidence,
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
}
