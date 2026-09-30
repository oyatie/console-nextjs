#![allow(clippy::unwrap_used)]

use console_kernel_core::{BranchId, ErrorKind, UserId, WorkOrderId};
use console_workorder_domain::{
    ApprovalLine, ApprovalRole, ApprovalStatus, AssignmentRole, CompletionEvidence,
    CompletionEvidenceInterlock, WorkOrder, WorkOrderAssignment, WorkOrderAssignments,
    WorkOrderStatus, WorkResultType,
};
use time::macros::datetime;

#[test]
fn approval_line_unlocks_in_order_and_completes_after_executive() {
    let mechanic_id = UserId::new();
    let admin_id = UserId::new();
    let executive_id = UserId::new();
    let mut line = ApprovalLine::new(
        mechanic_id,
        Some(admin_id),
        Some(executive_id),
        datetime!(2026-06-12 09:00:00 UTC),
    );

    assert_eq!(
        line.step(ApprovalRole::Mechanic).status(),
        ApprovalStatus::Pending
    );
    assert_eq!(
        line.step(ApprovalRole::Admin).status(),
        ApprovalStatus::NotStarted
    );
    assert_eq!(
        line.step(ApprovalRole::Executive).status(),
        ApprovalStatus::NotStarted
    );

    line.auto_approve_mechanic(datetime!(2026-06-12 10:00:00 UTC));
    assert_eq!(
        line.step(ApprovalRole::Mechanic).status(),
        ApprovalStatus::Approved
    );
    assert_eq!(
        line.step(ApprovalRole::Admin).status(),
        ApprovalStatus::Pending
    );
    assert_eq!(
        line.step(ApprovalRole::Executive).status(),
        ApprovalStatus::NotStarted
    );

    line.approve(
        ApprovalRole::Admin,
        admin_id,
        datetime!(2026-06-12 10:30:00 UTC),
    )
    .unwrap();
    assert_eq!(
        line.step(ApprovalRole::Admin).status(),
        ApprovalStatus::Approved
    );
    assert_eq!(
        line.step(ApprovalRole::Executive).status(),
        ApprovalStatus::Pending
    );
    assert!(!line.is_complete());

    line.approve(
        ApprovalRole::Executive,
        executive_id,
        datetime!(2026-06-12 11:00:00 UTC),
    )
    .unwrap();
    assert!(line.is_complete());
}

#[test]
fn approval_rejects_out_of_order_or_wrong_actor_steps() {
    let admin_id = UserId::new();
    let executive_id = UserId::new();
    let mut line = ApprovalLine::new(
        UserId::new(),
        Some(admin_id),
        Some(executive_id),
        datetime!(2026-06-12 09:00:00 UTC),
    );

    let err = line
        .approve(
            ApprovalRole::Executive,
            executive_id,
            datetime!(2026-06-12 09:30:00 UTC),
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);

    line.auto_approve_mechanic(datetime!(2026-06-12 10:00:00 UTC));
    let err = line
        .approve(
            ApprovalRole::Admin,
            UserId::new(),
            datetime!(2026-06-12 10:30:00 UTC),
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Forbidden);
}

#[test]
fn workorder_completion_requires_completed_approval_line_and_verified_evidence() {
    let mechanic_id = UserId::new();
    let admin_id = UserId::new();
    let executive_id = UserId::new();
    let assignments = WorkOrderAssignments::new(vec![WorkOrderAssignment::new(
        mechanic_id,
        AssignmentRole::Primary,
    )])
    .unwrap();
    let mut work_order = WorkOrder::new(
        WorkOrderId::new(),
        BranchId::new(),
        assignments,
        Some(admin_id),
        Some(executive_id),
        datetime!(2026-06-12 09:00:00 UTC),
    );

    work_order
        .start(datetime!(2026-06-12 09:10:00 UTC))
        .unwrap();
    work_order
        .submit_report(
            mechanic_id,
            WorkResultType::Completed,
            datetime!(2026-06-12 10:00:00 UTC),
        )
        .unwrap();
    work_order
        .approve_next(
            admin_id,
            datetime!(2026-06-12 10:10:00 UTC),
            &CompletionEvidence::Verified,
        )
        .unwrap();

    let err = work_order
        .transition_to(
            WorkOrderStatus::FinalCompleted,
            &CompletionEvidence::Verified,
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);

    let err = work_order
        .approve_next(
            executive_id,
            datetime!(2026-06-12 10:20:00 UTC),
            &CompletionEvidence::Unverified,
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);

    let transition = work_order
        .approve_next(
            executive_id,
            datetime!(2026-06-12 10:30:00 UTC),
            &CompletionEvidence::Verified,
        )
        .unwrap();
    assert_eq!(transition.from, WorkOrderStatus::AdminReview);
    assert_eq!(transition.to, WorkOrderStatus::FinalCompleted);
    assert_eq!(work_order.status(), WorkOrderStatus::FinalCompleted);
}

#[test]
fn multi_assignment_requires_at_least_one_assignee_exactly_one_primary_and_unique_users() {
    let empty = WorkOrderAssignments::new(vec![]).unwrap_err();
    assert_eq!(empty.kind, ErrorKind::Validation);

    let no_primary = WorkOrderAssignments::new(vec![WorkOrderAssignment::new(
        UserId::new(),
        AssignmentRole::Secondary,
    )])
    .unwrap_err();
    assert_eq!(no_primary.kind, ErrorKind::Validation);

    let mechanic_id = UserId::new();
    let duplicate = WorkOrderAssignments::new(vec![
        WorkOrderAssignment::new(mechanic_id, AssignmentRole::Primary),
        WorkOrderAssignment::new(mechanic_id, AssignmentRole::Secondary),
    ])
    .unwrap_err();
    assert_eq!(duplicate.kind, ErrorKind::Validation);

    let two_person = WorkOrderAssignments::new(vec![
        WorkOrderAssignment::new(mechanic_id, AssignmentRole::Primary),
        WorkOrderAssignment::new(UserId::new(), AssignmentRole::Secondary),
    ])
    .unwrap();
    assert_eq!(two_person.len(), 2);
    assert_eq!(two_person.primary().mechanic_id(), mechanic_id);
}

#[test]
fn completion_evidence_interlock_reports_verified_state() {
    assert!(
        CompletionEvidence::Verified
            .final_completion_evidence_verified(WorkOrderId::new())
            .unwrap()
    );
    assert!(
        !CompletionEvidence::Unverified
            .final_completion_evidence_verified(WorkOrderId::new())
            .unwrap()
    );
}
