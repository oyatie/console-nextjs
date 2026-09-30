//! Identity application layer.
//!
//! Maintenance owns identity locally through passkey-backed accounts. This
//! crate exposes org-setup commands, read models, and audit builders used by the
//! REST and persistence adapters; it does not define speculative external
//! identity-provider contracts.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod org;

pub use org::{
    AccountStatus, ActivateUserCommand, BranchSummary, CreateBranchCommand,
    CreatePolicyAssignmentPreviewReceiptCommand, CreatePolicyRoleCommand, CreateRegionCommand,
    CreateUserCommand, DeactivateBranchCommand, DeactivateRegionCommand, DeactivateUserCommand,
    DirectoryListQuery, DirectoryPage, DirectoryPerson, EmployeeLinkStatus,
    MAX_DIRECTORY_PAGE_LIMIT, PolicyAssignmentPreviewReceiptSummary, PolicyAuditEventSummary,
    PolicyRoleAssignmentSummary, PolicyRoleCondition, PolicyRolePermission, PolicyRoleSummary,
    PolicyVersionSummary, RegionSummary, ReplacePolicyRoleAssignmentsCommand, UpdateBranchCommand,
    UpdatePolicyRoleCommand, UpdatePolicyRoleStatusCommand, UpdateRegionCommand,
    UpdateSelfProfileCommand, UpdateUserCommand, UserListQuery, UserPage, UserSummary,
    account_status_for, branch_audit_event, policy_account_audit_event,
    policy_role_assignment_audit_event, policy_role_audit_event, region_audit_event,
    user_audit_event,
};
