//! Org-setup application layer: commands, query DTOs, read models, and audit
//! event builders for users, regions, and branches.
//!
//! Roles travel as canonical DB role strings (`SUPER_ADMIN`, `ADMIN`, …). The
//! REST boundary parses and authorizes them against the `console-platform-authz`
//! matrix; this layer stays free of that platform dependency to satisfy the
//! layer-boundary gate.

use console_identity_domain::Team;
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, KernelError, RegionId, Timestamp, TraceContext,
    UserId,
};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Create a user and (optionally) attach branch memberships in one transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateUserCommand {
    /// Acting administrator (audited).
    pub actor: UserId,
    pub display_name: String,
    /// Explicit HR employee-directory link. Never inferred by name.
    pub employee_id: Option<uuid::Uuid>,
    pub phone: Option<String>,
    pub team: Option<Team>,
    /// Canonical DB role strings, already validated at the REST boundary.
    pub roles: Vec<String>,
    /// Branch memberships to insert into `user_branches`.
    pub branch_ids: Vec<BranchId>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Partial update of a user's profile, roles, and/or branch memberships. A
/// `None` field is left unchanged; `Some` replaces it wholesale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateUserCommand {
    pub actor: UserId,
    pub user_id: UserId,
    pub display_name: Option<String>,
    /// `Some(None)` clears the employee link; `Some(Some(_))` sets it; `None` leaves it.
    pub employee_id: Option<Option<uuid::Uuid>>,
    /// `Some(None)` clears the phone; `Some(Some(_))` sets it; `None` leaves it.
    pub phone: Option<Option<String>>,
    /// `Some(None)` clears the team; `Some(Some(_))` sets it; `None` leaves it.
    pub team: Option<Option<Team>>,
    /// Replacement role set (canonical DB strings) when `Some`.
    pub roles: Option<Vec<String>>,
    /// Replacement branch-membership set when `Some`.
    pub branch_ids: Option<Vec<BranchId>>,
    /// Short-lived impact-preview receipt required for role/scope replacements.
    pub preview_receipt_id: Option<uuid::Uuid>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Self-service profile edit available to every authenticated user. Limited to
/// non-privileged fields (no role/branch escalation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSelfProfileCommand {
    pub user_id: UserId,
    pub display_name: Option<String>,
    /// `Some(None)` clears the phone; `Some(Some(_))` sets it; `None` leaves it.
    pub phone: Option<Option<String>>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Deactivate (soft-disable) a user. Sign-in is gated on `is_active`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeactivateUserCommand {
    pub actor: UserId,
    pub user_id: UserId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Reactivate a previously archived user. Credentials are not recreated here;
/// a reactivated account without passkeys returns to `PENDING_SETUP`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivateUserCommand {
    pub actor: UserId,
    pub user_id: UserId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Create one tenant-owned custom role definition. Definitions are persisted,
/// audited, versioned, and become runtime-effective only through ACTIVE
/// custom-role assignments resolved by the platform authz layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePolicyRoleCommand {
    pub actor: UserId,
    pub role_key: String,
    pub display_name: String,
    pub description: Option<String>,
    pub permissions: Vec<PolicyRolePermission>,
    pub conditions: Vec<PolicyRoleCondition>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Change a tenant-owned custom role definition lifecycle state. Publishing or
/// rolling back a role is a sensitive policy action: the REST layer must require
/// a fresh passkey step-up before constructing this command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdatePolicyRoleStatusCommand {
    pub actor: UserId,
    pub role_id: uuid::Uuid,
    pub status: String,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Update mutable metadata and policy surface for a tenant-owned custom role.
/// The role key is immutable; changing permissions/conditions is a sensitive
/// policy action and must be guarded by REST-layer passkey step-up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdatePolicyRoleCommand {
    pub actor: UserId,
    pub role_id: uuid::Uuid,
    pub display_name: String,
    pub description: Option<String>,
    pub permissions: Vec<PolicyRolePermission>,
    pub conditions: Vec<PolicyRoleCondition>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Replace a user's custom-role assignments. ACTIVE custom roles become
/// runtime-effective on the user's next resolved request principal; DRAFT and
/// RETIRED roles remain audit/planning data only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplacePolicyRoleAssignmentsCommand {
    pub actor: UserId,
    pub user_id: UserId,
    pub role_ids: Vec<uuid::Uuid>,
    pub preview_receipt_id: uuid::Uuid,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Persist a short-lived receipt for the exact assignment preview the actor saw.
/// The write path consumes this server-side receipt only if the mutable
/// authorization baseline still matches under the write transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePolicyAssignmentPreviewReceiptCommand {
    pub actor: UserId,
    pub user_id: UserId,
    pub current_branch_ids: Vec<uuid::Uuid>,
    pub current_system_roles: Vec<String>,
    pub current_role_ids: Vec<uuid::Uuid>,
    pub branch_ids: Vec<uuid::Uuid>,
    pub system_roles: Vec<String>,
    pub role_ids: Vec<uuid::Uuid>,
    pub policy_version: i64,
    pub expires_at: Timestamp,
}

/// Server-side receipt proving an actor reviewed a policy assignment preview
/// for one exact target user and normalized role set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyAssignmentPreviewReceiptSummary {
    pub id: uuid::Uuid,
    pub expires_at: Timestamp,
}

/// Create a region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRegionCommand {
    pub actor: UserId,
    pub name: String,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Create a branch inside a region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateBranchCommand {
    pub actor: UserId,
    pub region_id: RegionId,
    pub name: String,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Rename a region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRegionCommand {
    pub actor: UserId,
    pub region_id: RegionId,
    pub name: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Soft-delete (deactivate) a region. Refused while the region still has active
/// branches (referential guard) — the adapter returns a `Conflict`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeactivateRegionCommand {
    pub actor: UserId,
    pub region_id: RegionId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Rename a branch and/or move it to a different region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateBranchCommand {
    pub actor: UserId,
    /// Principal-resolved branch scope. Never caller-supplied. The mutated
    /// object here IS a branch, so the caller may only touch a branch its own
    /// scope allows; anything else is refused as `not_found`.
    pub branch_scope: BranchScope,
    pub branch_id: BranchId,
    pub region_id: Option<RegionId>,
    pub name: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Soft-delete (deactivate) a branch. Refused while the branch still has active
/// users or non-terminal equipment (referential guard) — the adapter returns a
/// `Conflict`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeactivateBranchCommand {
    pub actor: UserId,
    /// Principal-resolved branch scope. Never caller-supplied.
    pub branch_scope: BranchScope,
    pub branch_id: BranchId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// Branch-scoped user listing. The adapter resolves the caller's scope and only
/// returns users that share at least one in-scope branch (or all users for a
/// cross-branch caller).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserListQuery {
    pub include_inactive: bool,
    /// Page size; the adapter clamps to `1..=200` and defaults a missing value.
    pub limit: Option<i64>,
    /// Zero-based row offset into the scope-ordered roster for offset
    /// pagination. `None` starts at the first page.
    pub offset: Option<i64>,
}

/// Maximum directory page size accepted at the public REST boundary.
///
/// Persistence retains a defensive clamp for non-REST callers, but external
/// requests above this limit are rejected rather than silently rewritten.
pub const MAX_DIRECTORY_PAGE_LIMIT: i64 = 200;

/// Validated filters for a tenant people-directory query.
///
/// The persistence adapter must apply every filter to both its count and page
/// queries, and order by `(display_name, id)` — `display_name` under the
/// `und-x-icu` (ICU root) collation, so paging is identical on every server
/// without abandoning human name order — before applying pagination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryListQuery {
    /// Lowercase, trimmed search term; `None` means no name search filter.
    pub search: Option<String>,
    /// Optional exact team affiliation.
    pub team: Option<Team>,
    /// Optional branch filter, intersected with the caller's effective scope.
    pub branch_id: Option<BranchId>,
    pub include_inactive: bool,
    /// Page size; REST accepts `1..=MAX_DIRECTORY_PAGE_LIMIT` and defaults a
    /// missing value. The adapter retains a defensive bound for non-REST calls.
    pub limit: Option<i64>,
    /// Zero-based row offset into the fully filtered, `(display_name, id)`
    /// ordered roster. `None` starts at the first page.
    pub offset: Option<i64>,
}

/// One page of org users plus the unpaged `total` for the caller's branch scope,
/// so the console can show an honest count and page beyond the per-request cap.
/// Directory listing uses [`DirectoryPage`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPage {
    pub items: Vec<UserSummary>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

// ---------------------------------------------------------------------------
// Read models
// ---------------------------------------------------------------------------

/// Derived account-setup state for the console roster.
///
/// `is_active` alone is insufficient: a freshly-created user (admin issued an OTP
/// but the user has not yet enrolled a passkey) is `is_active = true` yet cannot
/// actually sign in. The console must show "활성" ONLY once the account is set up
/// with a credential, so this enum distinguishes pending setup from the
/// fully-active state and the archived/보관 lifecycle state. It is derived (never
/// stored): see `account_status_for`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AccountStatus {
    /// Active AND has at least one enrolled passkey — can sign in.
    Active,
    /// Active but has NO passkey yet — created / OTP-issued, awaiting enrollment.
    PendingSetup,
    /// Archived/보관 — sign-in is blocked regardless of credentials.
    Archived,
}

/// Derive the console account status from the row flag + credential presence.
#[must_use]
pub fn account_status_for(is_active: bool, has_passkey: bool) -> AccountStatus {
    match (is_active, has_passkey) {
        (false, _) => AccountStatus::Archived,
        (true, true) => AccountStatus::Active,
        (true, false) => AccountStatus::PendingSetup,
    }
}

/// Whether a platform account is explicitly linked to an HR employee record.
/// The absence of a link is a first-class state; the system must not silently
/// infer one from display name because Korean names often collide in real data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EmployeeLinkStatus {
    Linked,
    Unlinked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserSummary {
    pub id: UserId,
    pub display_name: String,
    pub employee_id: Option<uuid::Uuid>,
    pub employee_name: Option<String>,
    pub employee_number: Option<String>,
    pub employee_company: Option<String>,
    pub employee_org_unit: Option<String>,
    pub employee_position: Option<String>,
    pub employee_identity_review_required: Option<bool>,
    pub employee_identity_resolution_confidence: Option<String>,
    pub employee_link_status: EmployeeLinkStatus,
    pub phone: Option<String>,
    pub team: Option<Team>,
    pub roles: Vec<String>,
    pub branch_ids: Vec<BranchId>,
    pub is_active: bool,
    /// Whether the user has at least one enrolled passkey credential. A user can
    /// only actually sign in once this is true; until then they are pending setup.
    pub has_passkey: bool,
    /// Derived setup state (`is_active` + `has_passkey`) for the console badge.
    pub account_status: AccountStatus,
    pub created_at: Timestamp,
}

/// Directory list item: [`UserSummary`] fields except `phone`.
///
/// Org user get/list still use [`UserSummary`] (phone included). Mapping
/// `phone: None` onto `UserSummary` still serializes a `phone` key; this type
/// omits the field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryPerson {
    pub id: UserId,
    pub display_name: String,
    pub employee_id: Option<uuid::Uuid>,
    pub employee_name: Option<String>,
    pub employee_number: Option<String>,
    pub employee_company: Option<String>,
    pub employee_org_unit: Option<String>,
    pub employee_position: Option<String>,
    pub employee_identity_review_required: Option<bool>,
    pub employee_identity_resolution_confidence: Option<String>,
    pub employee_link_status: EmployeeLinkStatus,
    pub team: Option<Team>,
    pub roles: Vec<String>,
    pub branch_ids: Vec<BranchId>,
    pub is_active: bool,
    pub has_passkey: bool,
    pub account_status: AccountStatus,
    pub created_at: Timestamp,
}

impl From<UserSummary> for DirectoryPerson {
    fn from(user: UserSummary) -> Self {
        let UserSummary {
            id,
            display_name,
            employee_id,
            employee_name,
            employee_number,
            employee_company,
            employee_org_unit,
            employee_position,
            employee_identity_review_required,
            employee_identity_resolution_confidence,
            employee_link_status,
            phone: _,
            team,
            roles,
            branch_ids,
            is_active,
            has_passkey,
            account_status,
            created_at,
        } = user;
        Self {
            id,
            display_name,
            employee_id,
            employee_name,
            employee_number,
            employee_company,
            employee_org_unit,
            employee_position,
            employee_identity_review_required,
            employee_identity_resolution_confidence,
            employee_link_status,
            team,
            roles,
            branch_ids,
            is_active,
            has_passkey,
            account_status,
            created_at,
        }
    }
}

/// One page of directory people plus the unpaged `total` for the caller's
/// filtered branch scope. Items never include a `phone` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryPage {
    pub items: Vec<DirectoryPerson>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

impl From<UserPage> for DirectoryPage {
    fn from(page: UserPage) -> Self {
        let UserPage {
            items,
            limit,
            offset,
            total,
        } = page;
        Self {
            items: items.into_iter().map(DirectoryPerson::from).collect(),
            limit,
            offset,
            total,
        }
    }
}

/// One permission cell in a custom role definition. Keys are canonical snake-case
/// `Feature` and `PermissionLevel` strings validated at the REST boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRolePermission {
    pub feature_key: String,
    pub permission_level: String,
}

/// One ABAC/PBAC condition attached to a custom role definition. Runtime
/// authorization currently consumes branch equals/in as scope narrowers and team
/// equals/in as live user-attribute matches; unsupported conditions fail closed
/// at authorization time while remaining visible for preview/audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRoleCondition {
    pub condition_key: String,
    pub attribute: String,
    pub operator: String,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRoleSummary {
    pub id: uuid::Uuid,
    pub role_key: String,
    pub display_name: String,
    pub description: Option<String>,
    pub status: String,
    pub is_system: bool,
    pub permissions: Vec<PolicyRolePermission>,
    pub conditions: Vec<PolicyRoleCondition>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRoleAssignmentSummary {
    pub user_id: UserId,
    pub role_id: uuid::Uuid,
    pub role_key: String,
    pub display_name: String,
    pub status: String,
    pub assigned_by: Option<UserId>,
    pub created_at: Timestamp,
}

/// Per-tenant monotonic policy revision used by the future effective-policy
/// resolver cache. A missing DB row means no custom policy write has occurred,
/// so read APIs surface version 0 without mutating on read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyVersionSummary {
    pub version: i64,
    pub updated_at: Option<Timestamp>,
}

/// Append-only policy audit evidence visible from Policy Studio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyAuditEventSummary {
    pub id: uuid::Uuid,
    pub actor: Option<UserId>,
    pub action: String,
    pub target_type: String,
    pub target_id: String,
    pub before_snapshot: Option<serde_json::Value>,
    pub after_snapshot: Option<serde_json::Value>,
    pub trace_id: String,
    pub span_id: String,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionSummary {
    pub id: RegionId,
    pub name: String,
    /// `Some` when the region has been soft-deleted (deactivated); `None` for an
    /// active region. Active-only listings filter these out.
    pub deactivated_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchSummary {
    pub id: BranchId,
    pub region_id: RegionId,
    pub name: String,
    /// `Some` when the branch has been soft-deleted (deactivated); `None` for an
    /// active branch. Active-only listings filter these out.
    pub deactivated_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

// ---------------------------------------------------------------------------
// Audit builders
// ---------------------------------------------------------------------------

/// Build a custom-role policy audit event. Role policy changes are org-global;
/// the permission diff is stored in snapshots by the adapter.
pub fn policy_role_audit_event(
    action: &str,
    actor: Option<UserId>,
    role_id: uuid::Uuid,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        "policy_role",
        role_id.to_string(),
        trace,
        occurred_at,
    ))
}

/// Build a custom-role assignment audit event. The target is the user whose
/// custom-role assignment set changed.
pub fn policy_role_assignment_audit_event(
    action: &str,
    actor: Option<UserId>,
    user_id: UserId,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        "policy_role_assignment",
        user_id.to_string(),
        trace,
        occurred_at,
    ))
}

/// Build a policy-audit row for account/person lifecycle and authorization-scope
/// mutations. These `policy.*` rows are the evidence stream consumed by Policy
/// Studio audit chips; general `user.*` rows remain the operational audit trail.
pub fn policy_account_audit_event(
    action: &str,
    actor: Option<UserId>,
    user_id: UserId,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        "user",
        user_id.to_string(),
        trace,
        occurred_at,
    ))
}

/// Build a user-management audit event. User management is org-global (a user
/// can span branches), so no `branch_id` is attached; the role/branch changes
/// live in the snapshots.
pub fn user_audit_event(
    action: &str,
    actor: Option<UserId>,
    user_id: UserId,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        "user",
        user_id.to_string(),
        trace,
        occurred_at,
    ))
}

/// Build a region-management audit event (org-global).
pub fn region_audit_event(
    action: &str,
    actor: Option<UserId>,
    region_id: RegionId,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        "region",
        region_id.to_string(),
        trace,
        occurred_at,
    ))
}

/// Build a branch-management audit event, scoped to the branch.
pub fn branch_audit_event(
    action: &str,
    actor: Option<UserId>,
    branch_id: BranchId,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        "branch",
        branch_id.to_string(),
        trace,
        occurred_at,
    )
    .with_branch(branch_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Closed DirectoryPerson JSON keys: directory fields only, never phone or
    /// payroll-ish scrape keys.
    const DIRECTORY_PERSON_JSON_KEYS: &[&str] = &[
        "id",
        "display_name",
        "employee_id",
        "employee_name",
        "employee_number",
        "employee_company",
        "employee_org_unit",
        "employee_position",
        "employee_identity_review_required",
        "employee_identity_resolution_confidence",
        "employee_link_status",
        "team",
        "roles",
        "branch_ids",
        "is_active",
        "has_passkey",
        "account_status",
        "created_at",
    ];

    const DIRECTORY_PERSON_FORBIDDEN_JSON_KEYS: &[&str] = &[
        "phone",
        "salary",
        "bank_account",
        "rrn",
        "resident_registration_number",
        "payroll",
        "won",
    ];

    fn directory_person_json_keys(value: &serde_json::Value) -> BTreeSet<&str> {
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect()
    }

    fn assert_directory_person_closed_json_keys(person_json: &serde_json::Value) {
        let keys = directory_person_json_keys(person_json);
        let allowed: BTreeSet<&str> = DIRECTORY_PERSON_JSON_KEYS.iter().copied().collect();
        assert_eq!(
            keys, allowed,
            "DirectoryPerson JSON keys must be the closed directory allowlist, got {person_json}"
        );
        for &forbidden in DIRECTORY_PERSON_FORBIDDEN_JSON_KEYS {
            assert!(
                !keys.contains(forbidden),
                "DirectoryPerson must omit {forbidden}, got {person_json}"
            );
        }
    }

    #[test]
    fn user_audit_event_is_org_global() {
        let event = user_audit_event(
            "user.create",
            Some(UserId::new()),
            UserId::new(),
            TraceContext::generate(),
            Timestamp::now_utc(),
        )
        .unwrap();
        assert!(event.branch_id.is_none());
        assert_eq!(event.target_type, "user");

        let user = UserSummary {
            id: UserId::new(),
            display_name: "홍길동".to_owned(),
            employee_id: None,
            employee_name: None,
            employee_number: None,
            employee_company: None,
            employee_org_unit: None,
            employee_position: None,
            employee_identity_review_required: None,
            employee_identity_resolution_confidence: None,
            employee_link_status: EmployeeLinkStatus::Unlinked,
            phone: Some("010-1234-5678".to_owned()),
            team: None,
            roles: Vec::new(),
            branch_ids: Vec::new(),
            is_active: true,
            has_passkey: true,
            account_status: AccountStatus::Active,
            created_at: Timestamp::now_utc(),
        };
        assert_eq!(
            serde_json::to_value(&user).unwrap()["phone"],
            "010-1234-5678"
        );
        let person_json = serde_json::to_value(DirectoryPerson::from(user.clone())).unwrap();
        assert_directory_person_closed_json_keys(&person_json);
        let page_json = serde_json::to_value(DirectoryPage::from(UserPage {
            items: vec![user],
            limit: 1,
            offset: 0,
            total: 1,
        }))
        .unwrap();
        assert_directory_person_closed_json_keys(&page_json["items"][0]);
    }

    #[test]
    fn branch_audit_event_carries_branch_scope() {
        let branch = BranchId::new();
        let event = branch_audit_event(
            "branch.create",
            Some(UserId::new()),
            branch,
            TraceContext::generate(),
            Timestamp::now_utc(),
        )
        .unwrap();
        assert_eq!(event.branch_id, Some(branch));
    }
}
