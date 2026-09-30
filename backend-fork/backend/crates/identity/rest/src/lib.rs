//! Identity / org-setup REST API.
//!
//! Authenticated, authz-gated endpoints for the org-setup flow:
//!   * Users  — `/api/v1/users` (create/list/get/update/deactivate) and the
//!     self-profile pair `/api/v1/users/me`.
//!   * Regions — read-only applied-state listing at `/api/v1/regions`.
//!   * Branches — read-only applied-state list/detail; the list also backs
//!     support-ticket triage.
//!
//! The orgchange REST owner mounts the legacy region/branch mutation methods at
//! composition time. Those methods create governed proposals; identity itself
//! has no live org-structure write route.
//!
//! Authorization mirrors the IDOR-hardening in `issue_admin_otp`: creating or
//! newly promoting a user into EXECUTIVE/SUPER_ADMIN is restricted to
//! SUPER_ADMIN callers, and a sub-admin may only create or re-scope
//! non-privileged users in branches it controls (additions and removals).
//! Self-profile edits are open to every authenticated user.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use std::collections::BTreeSet;
use std::str::FromStr;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use console_identity_adapter_postgres::{PgOrgError, PgOrgStore};
use console_identity_application::{
    AccountStatus, ActivateUserCommand, CreatePolicyAssignmentPreviewReceiptCommand,
    CreatePolicyRoleCommand, CreateUserCommand, DeactivateUserCommand, DirectoryListQuery,
    EmployeeLinkStatus, MAX_DIRECTORY_PAGE_LIMIT, PolicyAuditEventSummary,
    PolicyRoleAssignmentSummary, PolicyRoleCondition, PolicyRolePermission, PolicyRoleSummary,
    PolicyVersionSummary, ReplacePolicyRoleAssignmentsCommand, UpdatePolicyRoleCommand,
    UpdatePolicyRoleStatusCommand, UpdateSelfProfileCommand, UpdateUserCommand, UserListQuery,
    UserPage, UserSummary,
};
use console_identity_domain::{Team, normalize_directory_search};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, ErrorKind, KernelError, OrgId, Timestamp,
    TraceContext, UserId,
};
use console_platform_auth::{JwtVerifier, PasskeyAuthenticationCredential, PasskeyService};
use console_platform_authz::cedar_pbac::{engine, map::canonical_coexistence_map};
use console_platform_authz::{
    Action, AuthorizationAuditEvent, AuthorizationRequest, AuthorizationResource,
    CoexistenceMapEntry, DecisionEffect, DualEngineMode, Feature, PermissionLevel, Principal,
    RlsScopeProof, Role, SubjectFreshnessRequirement, authorize, authorize_capability,
    custom_role_runtime_feature_allowed, evaluate_cedar_pbac_boundary, observe_cedar_pbac_decision,
    permission_for,
};
use console_platform_db::{DbError, with_audit, with_audits, with_org_conn};
use console_platform_request_context::{RequestContextError, current_org};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Row, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Route paths (exported for the openapi_drift test)
// ---------------------------------------------------------------------------

pub const USERS_PATH: &str = "/api/v1/users";
/// Tenant people directory. Its router registration is intentionally private to
/// the identity service until app-router and OpenAPI consolidation lands.
pub const DIRECTORY_PEOPLE_PATH: &str = "/api/v1/directory/people";
pub const USERS_ME_PATH: &str = "/api/v1/users/me";
pub const CONSOLE_ROLLOUT_PATH: &str = "/api/v1/console/rollout";
pub const CONSOLE_ROLLOUT_OPT_IN_PATH: &str = "/api/v1/console/rollout/opt-in";
pub const CONSOLE_ROLLOUT_ORG_FLAG_PATH: &str = "/api/v1/console/rollout/org-flag";
pub const CONSOLE_LEGACY_KILL_SWITCH_PATH: &str = "/api/v1/console/kill-switch";
/// Per-(org,user) Oyatie Console workspace layout (UI-M1b). GET returns the
/// caller's saved layout (empty `{}` default), PUT upserts it. Opaque jsonb.
pub const ME_WORKSPACE_PATH: &str = "/api/v1/me/workspace";
/// Caller's authorization projection (Identity Console UI-M13 / charter G-a):
/// org, branch scope, roles-as-principal-attributes, and the legacy-matrix
/// capability grants the console needs for deny-by-omission rendering. NON-
/// AUTHORITATIVE (`authority = "advisory_ui_only"`) — the server stays the sole
/// enforcer; this is a rendering hint that converges with the Cedar promotion
/// (same shape, `source` flips from `legacy_matrix` to `cedar`).
pub const ME_AUTHZ_PATH: &str = "/api/v1/me/authz";
pub const USER_PATH_TEMPLATE: &str = "/api/v1/users/{id}";
pub const USER_DEACTIVATE_PATH_TEMPLATE: &str = "/api/v1/users/{id}/deactivate";
pub const USER_ACTIVATE_PATH_TEMPLATE: &str = "/api/v1/users/{id}/activate";
pub const REGIONS_PATH: &str = "/api/v1/regions";
pub const REGION_PATH_TEMPLATE: &str = "/api/v1/regions/{id}";
pub const BRANCHES_PATH: &str = "/api/v1/branches";
pub const BRANCH_PATH_TEMPLATE: &str = "/api/v1/branches/{id}";
pub const PASSKEYS_PATH: &str = "/api/v1/passkeys";
pub const PASSKEY_PATH_TEMPLATE: &str = "/api/v1/passkeys/{id}";
pub const POLICY_FEATURES_PATH: &str = "/api/v1/policy/features";
pub const POLICY_ROLES_PATH: &str = "/api/v1/policy/roles";
pub const POLICY_ROLE_PATH_TEMPLATE: &str = "/api/v1/policy/roles/{id}";
pub const POLICY_ROLE_STATUS_PATH_TEMPLATE: &str = "/api/v1/policy/roles/{id}/status";
pub const POLICY_ROLE_STATUS_PREVIEW_PATH_TEMPLATE: &str =
    "/api/v1/policy/roles/{id}/status-preview";
pub const POLICY_ROLE_TEMPLATES_PATH: &str = "/api/v1/policy/role-templates";
pub const POLICY_ASSIGNMENTS_PATH: &str = "/api/v1/policy/assignments";
pub const POLICY_USER_ASSIGNMENTS_PATH_TEMPLATE: &str = "/api/v1/policy/users/{id}/assignments";
pub const POLICY_USER_ASSIGNMENT_PREVIEW_PATH_TEMPLATE: &str =
    "/api/v1/policy/users/{id}/assignment-preview";
pub const POLICY_AUDIT_EVENTS_PATH: &str = "/api/v1/policy/audit-events";
const POLICY_STUDIO_OPERATION_TOTAL: &str = "policy_studio_operation_total";
const POLICY_ASSIGNMENT_PREVIEW_RECEIPT_TTL: Duration = Duration::minutes(10);
const CONSOLE_ROLLOUT_FLAG_KEY: &str = "console_carbon_copy";
const CONSOLE_LEGACY_KILL_SWITCH_FLAG_KEY: &str = "console_legacy_kill_switch";
const CONSOLE_ROLLOUT_USER_FEATURE_KEY: &str = "console_rollout";

fn record_policy_studio_operation(operation: &'static str, outcome: &'static str) {
    metrics::counter!(
        POLICY_STUDIO_OPERATION_TOTAL,
        "operation" => operation,
        "outcome" => outcome,
    )
    .increment(1);
}

fn policy_branch_scope_label(scope: &BranchScope) -> &'static str {
    match scope {
        BranchScope::All => "all",
        BranchScope::Branches(branches) if branches.is_empty() => "none",
        BranchScope::Branches(_) => "branches",
    }
}

fn record_policy_studio_rejection(
    operation: &'static str,
    principal: &Principal,
    error: &RestError,
) {
    let outcome = if error.status == StatusCode::FORBIDDEN {
        "denied"
    } else {
        "invalid"
    };
    record_policy_studio_operation(operation, outcome);
    tracing::warn!(
        event = "policy_studio_operation_rejected",
        operation,
        outcome,
        error_code = error.code,
        actor_user_id = %principal.user_id,
        branch_scope = policy_branch_scope_label(&principal.branch_scope),
        "policy studio operation rejected"
    );
}

pub const IDENTITY_ROUTE_PATHS: &[&str] = &[
    USERS_PATH,
    DIRECTORY_PEOPLE_PATH,
    USERS_ME_PATH,
    CONSOLE_ROLLOUT_PATH,
    CONSOLE_ROLLOUT_OPT_IN_PATH,
    CONSOLE_ROLLOUT_ORG_FLAG_PATH,
    CONSOLE_LEGACY_KILL_SWITCH_PATH,
    ME_WORKSPACE_PATH,
    ME_AUTHZ_PATH,
    USER_PATH_TEMPLATE,
    USER_DEACTIVATE_PATH_TEMPLATE,
    USER_ACTIVATE_PATH_TEMPLATE,
    REGIONS_PATH,
    REGION_PATH_TEMPLATE,
    BRANCHES_PATH,
    BRANCH_PATH_TEMPLATE,
    PASSKEYS_PATH,
    PASSKEY_PATH_TEMPLATE,
    POLICY_FEATURES_PATH,
    POLICY_ROLES_PATH,
    POLICY_ROLE_PATH_TEMPLATE,
    POLICY_ROLE_STATUS_PATH_TEMPLATE,
    POLICY_ROLE_STATUS_PREVIEW_PATH_TEMPLATE,
    POLICY_ROLE_TEMPLATES_PATH,
    POLICY_ASSIGNMENTS_PATH,
    POLICY_USER_ASSIGNMENTS_PATH_TEMPLATE,
    POLICY_USER_ASSIGNMENT_PREVIEW_PATH_TEMPLATE,
    POLICY_AUDIT_EVENTS_PATH,
];

#[derive(Clone)]
pub struct IdentityRestState {
    store: PgOrgStore,
    jwt_verifier: Option<JwtVerifier>,
    passkey_step_up: Option<PasskeyService>,
}

impl IdentityRestState {
    #[must_use]
    pub fn new(store: PgOrgStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
            passkey_step_up: None,
        }
    }

    #[must_use]
    pub fn with_passkey_step_up(mut self, passkey_step_up: Option<PasskeyService>) -> Self {
        self.passkey_step_up = passkey_step_up;
        self
    }

    fn pool(&self) -> &PgPool {
        self.store.pool()
    }

    /// SSR composition helper: the same `EmployeeDirectoryRead` listing as
    /// GET `/directory/people`, or an empty vec (omit) when the caller is
    /// unauthenticated, unauthorized, or the listing fails.
    pub async fn visible_directory_people(
        &self,
        headers: &HeaderMap,
    ) -> Vec<VisibleDirectoryPerson> {
        list_visible_directory_people(self, headers)
            .await
            .unwrap_or_default()
    }
}

/// Directory person already authorized for SSR. Required `DirectoryPerson`
/// keys only; never phone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibleDirectoryPerson {
    pub id: String,
    pub display_name: String,
    pub employee_id: String,
    pub employee_name: String,
    pub employee_number: String,
    pub employee_company: String,
    pub employee_org_unit: String,
    pub employee_position: String,
    pub employee_identity_review_required: String,
    pub employee_identity_resolution_confidence: String,
    pub employee_link_status: String,
    pub team: String,
    pub roles: String,
    pub branch_ids: String,
    pub is_active: String,
    pub has_passkey: String,
    pub account_status: String,
    pub created_at: String,
}

pub fn router(state: IdentityRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool().clone();
    let router = Router::new()
        // `/users/me` MUST be registered before `/users/{id}` so the literal
        // segment wins over the path capture.
        .route(USERS_ME_PATH, get(get_me).patch(update_me))
        .route(CONSOLE_ROLLOUT_PATH, get(get_console_rollout))
        .route(
            CONSOLE_ROLLOUT_OPT_IN_PATH,
            put(update_console_rollout_opt_in),
        )
        .route(
            CONSOLE_ROLLOUT_ORG_FLAG_PATH,
            put(update_console_rollout_org_flag),
        )
        .route(
            CONSOLE_LEGACY_KILL_SWITCH_PATH,
            post(update_console_legacy_kill_switch),
        )
        .route(ME_WORKSPACE_PATH, get(get_workspace).put(put_workspace))
        .route(ME_AUTHZ_PATH, get(get_me_authz))
        .route(USERS_PATH, get(list_users).post(create_user))
        .route(DIRECTORY_PEOPLE_PATH, get(list_directory_people))
        .route(USER_PATH_TEMPLATE, get(get_user).patch(update_user))
        .route(USER_DEACTIVATE_PATH_TEMPLATE, post(deactivate_user))
        .route(USER_ACTIVATE_PATH_TEMPLATE, post(activate_user))
        .route(REGIONS_PATH, get(list_regions))
        .route(BRANCHES_PATH, get(list_branches))
        .route(BRANCH_PATH_TEMPLATE, get(get_branch))
        .route(PASSKEYS_PATH, get(list_passkeys))
        .route(PASSKEY_PATH_TEMPLATE, delete(delete_passkey))
        .route(POLICY_FEATURES_PATH, get(list_policy_features))
        .route(
            POLICY_ROLES_PATH,
            get(list_policy_roles).post(create_policy_role),
        )
        .route(POLICY_ROLE_PATH_TEMPLATE, patch(update_policy_role))
        .route(
            POLICY_ROLE_STATUS_PATH_TEMPLATE,
            patch(update_policy_role_status),
        )
        .route(
            POLICY_ROLE_STATUS_PREVIEW_PATH_TEMPLATE,
            post(preview_policy_role_status),
        )
        .route(POLICY_ROLE_TEMPLATES_PATH, get(list_policy_role_templates))
        .route(POLICY_AUDIT_EVENTS_PATH, get(list_policy_audit_events))
        .route(POLICY_ASSIGNMENTS_PATH, get(list_policy_assignments))
        .route(
            POLICY_USER_ASSIGNMENTS_PATH_TEMPLATE,
            put(replace_policy_assignments),
        )
        .route(
            POLICY_USER_ASSIGNMENT_PREVIEW_PATH_TEMPLATE,
            post(preview_policy_assignments),
        )
        .with_state(state);
    // Per-request tenant context: resolves the Principal and arms `CURRENT_ORG`
    // for every authenticated route on this router, so adapter reads/writes run
    // scoped to the request's tenant.
    console_platform_request_context::with_request_context(router, verifier, pool)
}

// ---------------------------------------------------------------------------
// Request / response DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateUserRequest {
    display_name: String,
    #[serde(default)]
    employee_id: Option<uuid::Uuid>,
    #[serde(default)]
    phone: Option<String>,
    #[serde(default)]
    team: Option<Team>,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    branch_ids: Vec<BranchId>,
}

#[derive(Debug, Deserialize)]
struct UpdateUserRequest {
    #[serde(default)]
    display_name: Option<String>,
    /// Present key (even `null`) updates the employee link; absent leaves it unchanged.
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    employee_id: Option<Option<uuid::Uuid>>,
    /// Present key (even `null`) updates the phone; absent leaves it unchanged.
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    phone: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    team: Option<Option<Team>>,
    #[serde(default)]
    roles: Option<Vec<String>>,
    #[serde(default)]
    branch_ids: Option<Vec<BranchId>>,
    /// Required when `roles` or `branch_ids` is present.
    #[serde(default)]
    preview_acknowledged: bool,
    /// Server-issued impact-preview receipt for the exact role/scope edit.
    #[serde(default)]
    preview_receipt_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct UpdateSelfRequest {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    phone: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
struct UpdateConsoleRolloutOptInRequest {
    opt_in: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateConsoleRolloutOrgFlagRequest {
    enabled: bool,
    #[serde(default)]
    rollout_note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateConsoleLegacyKillSwitchRequest {
    enabled: bool,
    #[serde(default, alias = "rollout_note")]
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct ConsoleRolloutResponse {
    flag_key: &'static str,
    org_enabled: bool,
    org_rollout_enabled: bool,
    user_opted_in: bool,
    legacy_kill_switch_enabled: bool,
    kill_switch_active: bool,
    effective_new_console: bool,
    effective_route: &'static str,
    effective_route_for_opted_in_user: &'static str,
    effective_route_for_opted_out_user: &'static str,
    overrides_individual_toggles: bool,
}

#[derive(Debug, Deserialize)]
struct ListUsersRequest {
    #[serde(default)]
    include_inactive: bool,
    limit: Option<i64>,
    /// Zero-based row offset for offset pagination. Optional, defaults to 0.
    offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DirectoryListRequest {
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    team: Option<String>,
    #[serde(default)]
    branch_id: Option<String>,
    #[serde(default)]
    include_inactive: bool,
    limit: Option<i64>,
    offset: Option<i64>,
}

impl DirectoryListRequest {
    fn into_query(self) -> Result<DirectoryListQuery, RestError> {
        let team = self.team.as_deref().map(parse_directory_team).transpose()?;
        let branch_id = self
            .branch_id
            .as_deref()
            .map(|value| {
                Uuid::parse_str(value)
                    .map(BranchId::from_uuid)
                    .map_err(|_| RestError::validation("branch_id must be a UUID"))
            })
            .transpose()?;
        if self
            .limit
            .is_some_and(|limit| !(1..=MAX_DIRECTORY_PAGE_LIMIT).contains(&limit))
        {
            return Err(RestError::validation("limit must be between 1 and 200"));
        }
        if self.offset.is_some_and(|offset| offset < 0) {
            return Err(RestError::validation("offset must be non-negative"));
        }
        Ok(DirectoryListQuery {
            search: normalize_directory_search(self.search.as_deref())
                .map_err(RestError::from_kernel)?,
            team,
            branch_id,
            include_inactive: self.include_inactive,
            limit: self.limit,
            offset: self.offset,
        })
    }
}

/// Directory-only person row: `UserSummary` fields except `phone`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct DirectoryPerson {
    id: UserId,
    display_name: String,
    employee_id: Option<uuid::Uuid>,
    employee_name: Option<String>,
    employee_number: Option<String>,
    employee_company: Option<String>,
    employee_org_unit: Option<String>,
    employee_position: Option<String>,
    employee_identity_review_required: Option<bool>,
    employee_identity_resolution_confidence: Option<String>,
    employee_link_status: EmployeeLinkStatus,
    team: Option<Team>,
    roles: Vec<String>,
    branch_ids: Vec<BranchId>,
    is_active: bool,
    has_passkey: bool,
    account_status: AccountStatus,
    created_at: Timestamp,
}

impl From<UserSummary> for VisibleDirectoryPerson {
    fn from(user: UserSummary) -> Self {
        use time::format_description::well_known::Rfc3339;
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
            id: id.to_string(),
            display_name,
            employee_id: employee_id
                .map(|value| value.to_string())
                .unwrap_or_default(),
            employee_name: employee_name.unwrap_or_default(),
            employee_number: employee_number.unwrap_or_default(),
            employee_company: employee_company.unwrap_or_default(),
            employee_org_unit: employee_org_unit.unwrap_or_default(),
            employee_position: employee_position.unwrap_or_default(),
            employee_identity_review_required: employee_identity_review_required
                .map(|value| value.to_string())
                .unwrap_or_default(),
            employee_identity_resolution_confidence: employee_identity_resolution_confidence
                .unwrap_or_default(),
            employee_link_status: match employee_link_status {
                EmployeeLinkStatus::Linked => "LINKED".to_owned(),
                EmployeeLinkStatus::Unlinked => "UNLINKED".to_owned(),
            },
            team: team.map(|value| value.to_string()).unwrap_or_default(),
            roles: roles.join(","),
            branch_ids: branch_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
            is_active: is_active.to_string(),
            has_passkey: has_passkey.to_string(),
            account_status: match account_status {
                AccountStatus::Active => "ACTIVE".to_owned(),
                AccountStatus::PendingSetup => "PENDING_SETUP".to_owned(),
                AccountStatus::Archived => "ARCHIVED".to_owned(),
            },
            created_at: created_at.format(&Rfc3339).unwrap_or_default(),
        }
    }
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

/// Directory-only page. Items never include a `phone` key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct DirectoryPage {
    items: Vec<DirectoryPerson>,
    limit: i64,
    offset: i64,
    total: i64,
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

fn parse_directory_team(value: &str) -> Result<Team, RestError> {
    match value {
        "MAINTENANCE" => Ok(Team::Maintenance),
        "PREVENTION" => Ok(Team::Prevention),
        "MANAGEMENT" => Ok(Team::Management),
        "RECEPTION" => Ok(Team::Reception),
        _ => Err(RestError::validation("team is invalid")),
    }
}

/// A passkey credential summary for the self-service management surface.
///
/// Deliberately carries NO secret material: never the `passkey_json` blob, the
/// public key, or the raw `credential_id`. Only the opaque row id (for the delete
/// route) and the registration / last-use timestamps are exposed.
#[derive(Debug, Serialize)]

struct PasskeySummary {
    id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    last_used_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
struct PolicyFeatureResponse {
    feature_key: String,
    elevated: bool,
    default_permissions: Vec<PolicyDefaultPermissionResponse>,
}

#[derive(Debug, Serialize)]
struct PolicyDefaultPermissionResponse {
    role_key: String,
    permission_level: String,
}

#[derive(Debug, Serialize)]
struct PolicyRoleCatalogResponse {
    policy_version: PolicyVersionResponse,
    system_roles: Vec<SystemPolicyRoleResponse>,
    custom_roles: Vec<PolicyRoleResponse>,
}

#[derive(Debug, Serialize)]
struct PolicyVersionResponse {
    version: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    updated_at: Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
struct PolicyAuditEventsQuery {
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct PolicyAuditEventResponse {
    id: Uuid,
    actor: Option<Uuid>,
    action: String,
    target_type: String,
    target_id: String,
    before_snapshot: Option<serde_json::Value>,
    after_snapshot: Option<serde_json::Value>,
    trace_id: String,
    span_id: String,
    #[serde(with = "time::serde::rfc3339")]
    occurred_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct SystemPolicyRoleResponse {
    role_key: String,
    display_name: String,
    status: String,
    is_system: bool,
    permissions: Vec<PolicyPermissionResponse>,
}

#[derive(Debug, Serialize)]
struct PolicyRoleResponse {
    id: Uuid,
    role_key: String,
    display_name: String,
    description: Option<String>,
    status: String,
    is_system: bool,
    permissions: Vec<PolicyPermissionResponse>,
    conditions: Vec<PolicyConditionResponse>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct PolicyRoleStatusPreviewResponse {
    role_id: Uuid,
    role_key: String,
    display_name: String,
    current_status: String,
    requested_status: String,
    permission_count: i64,
    condition_count: i64,
    planned_assignment_count: i64,
    requires_passkey_step_up: bool,
    effective_runtime_change: bool,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PolicyPermissionResponse {
    feature_key: String,
    permission_level: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PolicyConditionResponse {
    condition_key: String,
    attribute: String,
    operator: String,
    values: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CreatePolicyRoleRequest {
    role_key: String,
    display_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    permissions: Vec<PolicyPermissionResponse>,
    #[serde(default)]
    conditions: Vec<PolicyConditionResponse>,
}

#[derive(Debug, Deserialize)]
struct UpdatePolicyRoleRequest {
    display_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    permissions: Vec<PolicyPermissionResponse>,
    #[serde(default)]
    conditions: Vec<PolicyConditionResponse>,
    #[serde(default)]
    step_up: Option<PolicyStepUpAssertionRequest>,
}

#[derive(Debug, Deserialize)]
struct PolicyStepUpAssertionRequest {
    ceremony_id: Uuid,
    credential: PasskeyAuthenticationCredential,
}

#[derive(Debug, Deserialize)]
struct UpdatePolicyRoleStatusRequest {
    status: String,
    #[serde(default)]
    step_up: Option<PolicyStepUpAssertionRequest>,
}

#[derive(Debug, Deserialize)]
struct PolicyRoleStatusPreviewRequest {
    status: String,
}

#[derive(Debug, Serialize)]
struct PolicyRoleTemplateResponse {
    template_key: String,
    role_key: String,
    display_name: String,
    category: String,
    description: String,
    permissions: Vec<PolicyPermissionResponse>,
}

#[derive(Debug, Deserialize)]
struct ListPolicyAssignmentsRequest {
    user_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct PolicyRoleAssignmentResponse {
    user_id: Uuid,
    role_id: Uuid,
    role_key: String,
    display_name: String,
    status: String,
    assigned_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct ReplacePolicyRoleAssignmentsRequest {
    /// Custom-role replacement set. For account/person role-or-scope previews,
    /// omitting this field preserves the target's current custom-role set;
    /// sending an explicit empty array previews replacing it with no custom roles.
    #[serde(default)]
    role_ids: Option<Vec<Uuid>>,
    /// Optional replacement system-role set used by account/person mutation
    /// previews. The custom-role save endpoint ignores this and preserves the
    /// target's current system roles.
    #[serde(default)]
    system_roles: Option<Vec<String>>,
    /// Optional replacement branch-scope set used by account/person mutation
    /// previews. The custom-role save endpoint ignores this and preserves the
    /// target's current branch memberships.
    #[serde(default)]
    branch_ids: Option<Vec<BranchId>>,
    #[serde(default)]
    preview_acknowledged: bool,
    #[serde(default)]
    preview_receipt_id: Option<Uuid>,
    #[serde(default)]
    step_up: Option<PolicyStepUpAssertionRequest>,
}

#[derive(Debug, Serialize)]
struct PolicyRoleAssignmentDeltaResponse {
    added_role_ids: Vec<Uuid>,
    removed_role_ids: Vec<Uuid>,
    unchanged_role_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
struct PolicyRoleImpactResponse {
    role_id: Uuid,
    role_key: String,
    display_name: String,
    status: String,
    runtime_effective: bool,
    runtime_warnings: Vec<String>,
    conditions: Vec<PolicyConditionResponse>,
}

#[derive(Debug, Serialize)]
struct PolicyFeatureGrantPreviewResponse {
    feature_key: String,
    permission_level: String,
    source_type: String,
    source_key: String,
    source_label: String,
}

#[derive(Debug, Serialize)]
struct PolicyAssignmentPreviewResponse {
    user_id: Uuid,
    preview_receipt_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    preview_receipt_expires_at: OffsetDateTime,
    effective: bool,
    system_roles: Vec<String>,
    current_system_roles: Vec<String>,
    requested_system_roles: Vec<String>,
    current_branch_ids: Vec<Uuid>,
    requested_branch_ids: Vec<Uuid>,
    current_role_ids: Vec<Uuid>,
    requested_role_ids: Vec<Uuid>,
    delta: PolicyRoleAssignmentDeltaResponse,
    custom_roles: Vec<PolicyRoleImpactResponse>,
    feature_grants: Vec<PolicyFeatureGrantPreviewResponse>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
}

/// Deserialize a field so that a present-but-null JSON value maps to
/// `Some(None)` (clear), an absent field to `None` (leave unchanged), and a
/// present value to `Some(Some(value))` (set).
fn deserialize_optional_field<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::<T>::deserialize(deserializer)?))
}

// ---------------------------------------------------------------------------
// Policy Studio handlers (G016-P0)
// ---------------------------------------------------------------------------

async fn list_policy_features(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage_observed(&state, &principal, Feature::RoleManage).await?;
    let catalog = policy_feature_catalog();
    record_policy_studio_operation("list_features", "success");
    tracing::info!(
        event = "policy_studio_features_listed",
        operation = "list_features",
        outcome = "success",
        actor_user_id = %principal.user_id,
        feature_count = catalog.len(),
        branch_scope = policy_branch_scope_label(&principal.branch_scope),
        "policy studio features listed"
    );
    Ok(Json(catalog))
}

async fn list_policy_roles(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage_observed(&state, &principal, Feature::RoleManage).await?;
    let custom_roles = state
        .store
        .list_policy_roles()
        .await
        .map_err(RestError::from_store)?
        .into_iter()
        .filter(|role| policy_role_is_inside_delegated_authority(&principal, role))
        .map(PolicyRoleResponse::from)
        .collect::<Vec<_>>();
    let policy_version = state
        .store
        .get_policy_version()
        .await
        .map_err(RestError::from_store)?;
    let system_roles = system_policy_roles();
    record_policy_studio_operation("list_roles", "success");
    tracing::info!(
        event = "policy_studio_roles_listed",
        operation = "list_roles",
        outcome = "success",
        actor_user_id = %principal.user_id,
        custom_role_count = custom_roles.len(),
        system_role_count = system_roles.len(),
        policy_version = policy_version.version,
        branch_scope = policy_branch_scope_label(&principal.branch_scope),
        "policy studio roles listed"
    );
    Ok(Json(PolicyRoleCatalogResponse {
        policy_version: policy_version.into(),
        system_roles,
        custom_roles,
    }))
}

async fn list_policy_role_templates(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage_observed(&state, &principal, Feature::RoleManage).await?;
    let templates = policy_role_templates();
    record_policy_studio_operation("list_templates", "success");
    tracing::info!(
        event = "policy_studio_templates_listed",
        operation = "list_templates",
        outcome = "success",
        actor_user_id = %principal.user_id,
        template_count = templates.len(),
        branch_scope = policy_branch_scope_label(&principal.branch_scope),
        "policy studio role templates listed"
    );
    Ok(Json(templates))
}

async fn list_policy_audit_events(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Query(query): Query<PolicyAuditEventsQuery>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage_observed(&state, &principal, Feature::RoleManage).await?;
    let limit = normalize_policy_audit_limit(query.limit)?;
    let events = state
        .store
        .list_policy_audit_events(limit)
        .await
        .map_err(RestError::from_store)?
        .into_iter()
        .map(PolicyAuditEventResponse::from)
        .collect::<Vec<_>>();
    record_policy_studio_operation("list_audit_events", "success");
    tracing::info!(
        event = "policy_studio_audit_events_listed",
        operation = "list_audit_events",
        outcome = "success",
        actor_user_id = %principal.user_id,
        event_count = events.len(),
        branch_scope = policy_branch_scope_label(&principal.branch_scope),
        "policy studio audit events listed"
    );
    Ok(Json(events))
}

async fn create_policy_role(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Json(body): Json<CreatePolicyRoleRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage_observed(&state, &principal, Feature::RoleManage).await?;
    let role_key = normalize_policy_role_key(&body.role_key)?;
    let display_name = normalize_policy_display_name(&body.display_name)?;
    let description = normalize_policy_description(body.description.as_deref())?;
    let permissions = validate_policy_permissions(&body.permissions, false)?;
    let conditions = validate_policy_conditions(&body.conditions)?;
    ensure_policy_conditions_inside_delegated_authority_for_operation(
        "create_role",
        &principal,
        &conditions,
    )?;

    let trace = TraceContext::generate();
    let role = state
        .store
        .create_policy_role(CreatePolicyRoleCommand {
            actor: principal.user_id,
            role_key,
            display_name,
            description,
            permissions,
            conditions,
            trace: trace.clone(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    record_policy_studio_operation("create_role", "success");
    tracing::info!(
        event = "policy_studio_role_created",
        operation = "create_role",
        outcome = "success",
        actor_user_id = %principal.user_id,
        role_id = %role.id,
        permission_count = role.permissions.len(),
        condition_count = role.conditions.len(),
        audit_trace_id = trace.trace_id(),
        audit_span_id = trace.span_id(),
        branch_scope = policy_branch_scope_label(&principal.branch_scope),
        "policy studio role created"
    );
    Ok((StatusCode::CREATED, Json(PolicyRoleResponse::from(role))))
}

async fn update_policy_role(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdatePolicyRoleRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage_observed(&state, &principal, Feature::RoleManage).await?;
    let display_name = normalize_policy_display_name(&body.display_name)?;
    let description = normalize_policy_description(body.description.as_deref())?;
    let conditions = validate_policy_conditions(&body.conditions)?;
    let role = state
        .store
        .list_policy_roles()
        .await
        .map_err(RestError::from_store)?
        .into_iter()
        .find(|role| role.id == id)
        .ok_or_else(|| RestError::from_kernel(KernelError::not_found("policy role not found")))?;
    if role.is_system {
        return Err(RestError::validation(
            "system policy roles cannot be changed",
        ));
    }
    let legacy_ai_only = role
        .permissions
        .iter()
        .any(|permission| permission.feature_key == Feature::AiAssist.as_str())
        && role
            .permissions
            .iter()
            .all(|permission| permission.feature_key == Feature::AiAssist.as_str());
    let permissions = validate_policy_permissions(&body.permissions, legacy_ai_only)?;
    let requested_role = PolicyRoleSummary {
        id: role.id,
        role_key: role.role_key.clone(),
        display_name: display_name.clone(),
        description: description.clone(),
        status: role.status.clone(),
        is_system: role.is_system,
        permissions: permissions.clone(),
        conditions: conditions.clone(),
        created_at: role.created_at,
        updated_at: role.updated_at,
    };
    ensure_policy_roles_inside_delegated_authority_for_operation(
        "update_role",
        &principal,
        &[role.clone(), requested_role],
    )?;
    verify_policy_step_up(&state, &principal, body.step_up).await?;

    let trace = TraceContext::generate();
    let role = state
        .store
        .update_policy_role(UpdatePolicyRoleCommand {
            actor: principal.user_id,
            role_id: id,
            display_name,
            description,
            permissions,
            conditions,
            trace: trace.clone(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    record_policy_studio_operation("update_role", "success");
    tracing::info!(
        event = "policy_studio_role_updated",
        operation = "update_role",
        outcome = "success",
        actor_user_id = %principal.user_id,
        role_id = %role.id,
        permission_count = role.permissions.len(),
        condition_count = role.conditions.len(),
        audit_trace_id = trace.trace_id(),
        audit_span_id = trace.span_id(),
        branch_scope = policy_branch_scope_label(&principal.branch_scope),
        "policy studio role updated"
    );
    Ok(Json(PolicyRoleResponse::from(role)))
}

async fn preview_policy_role_status(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<PolicyRoleStatusPreviewRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage_observed(&state, &principal, Feature::RoleManage).await?;
    let requested_status = normalize_policy_role_status(&body.status)?;
    let role = state
        .store
        .list_policy_roles()
        .await
        .map_err(RestError::from_store)?
        .into_iter()
        .find(|role| role.id == id)
        .ok_or_else(|| RestError::from_kernel(KernelError::not_found("policy role not found")))?;
    ensure_policy_roles_inside_delegated_authority_for_operation(
        "preview_role_status",
        &principal,
        std::slice::from_ref(&role),
    )?;
    validate_policy_role_status_transition(&role.status, &requested_status)?;
    let planned_assignment_count = state
        .store
        .count_policy_role_assignments(id)
        .await
        .map_err(RestError::from_store)?;
    record_policy_studio_operation("preview_role_status", "success");
    tracing::info!(
        event = "policy_studio_role_status_previewed",
        operation = "preview_role_status",
        outcome = "success",
        actor_user_id = %principal.user_id,
        role_id = %role.id,
        current_status = %role.status,
        requested_status = %requested_status,
        planned_assignment_count,
        branch_scope = policy_branch_scope_label(&principal.branch_scope),
        "policy studio role status previewed"
    );
    Ok(Json(build_policy_role_status_preview(
        &role,
        requested_status,
        planned_assignment_count,
    )))
}

async fn update_policy_role_status(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdatePolicyRoleStatusRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage_observed(&state, &principal, Feature::RoleManage).await?;
    let status = normalize_policy_role_status(&body.status)?;
    let role = state
        .store
        .list_policy_roles()
        .await
        .map_err(RestError::from_store)?
        .into_iter()
        .find(|role| role.id == id)
        .ok_or_else(|| RestError::from_kernel(KernelError::not_found("policy role not found")))?;
    ensure_policy_roles_inside_delegated_authority_for_operation(
        "update_role_status",
        &principal,
        std::slice::from_ref(&role),
    )?;
    validate_policy_role_status_transition(&role.status, &status)?;
    verify_policy_step_up(&state, &principal, body.step_up).await?;

    let trace = TraceContext::generate();
    let role = state
        .store
        .update_policy_role_status(UpdatePolicyRoleStatusCommand {
            actor: principal.user_id,
            role_id: id,
            status,
            trace: trace.clone(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    record_policy_studio_operation("update_role_status", "success");
    tracing::info!(
        event = "policy_studio_role_status_updated",
        operation = "update_role_status",
        outcome = "success",
        actor_user_id = %principal.user_id,
        role_id = %role.id,
        status = %role.status,
        audit_trace_id = trace.trace_id(),
        audit_span_id = trace.span_id(),
        branch_scope = policy_branch_scope_label(&principal.branch_scope),
        "policy studio role status updated"
    );
    Ok(Json(PolicyRoleResponse::from(role)))
}

async fn list_policy_assignments(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Query(query): Query<ListPolicyAssignmentsRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage_observed(&state, &principal, Feature::RoleManage).await?;
    let Some(user_id) = query.user_id.map(UserId::from_uuid) else {
        return Err(RestError::validation("user_id query parameter is required"));
    };
    // Custom-role assignments are user governance data and ACTIVE roles are
    // runtime-effective, so branch-scoped RoleManage holders may only inspect
    // targets visible in their live branch scope.
    state
        .store
        .get_user(user_id, &principal.branch_scope)
        .await
        .map_err(RestError::from_store)?;
    let assignments = state
        .store
        .list_policy_role_assignments(user_id)
        .await
        .map_err(RestError::from_store)?
        .into_iter()
        .map(PolicyRoleAssignmentResponse::from)
        .collect::<Vec<_>>();
    record_policy_studio_operation("list_assignments", "success");
    tracing::info!(
        event = "policy_studio_assignments_listed",
        operation = "list_assignments",
        outcome = "success",
        actor_user_id = %principal.user_id,
        target_user_id = %user_id,
        assignment_count = assignments.len(),
        branch_scope = policy_branch_scope_label(&principal.branch_scope),
        "policy studio assignments listed"
    );
    Ok(Json(assignments))
}

async fn replace_policy_assignments(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<ReplacePolicyRoleAssignmentsRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage_observed(&state, &principal, Feature::RoleManage).await?;
    let user_id = UserId::from_uuid(id);
    state
        .store
        .get_user(user_id, &principal.branch_scope)
        .await
        .map_err(RestError::from_store)?;
    let current_assignments = state
        .store
        .list_policy_role_assignments(user_id)
        .await
        .map_err(RestError::from_store)?;
    let custom_roles = state
        .store
        .list_policy_roles()
        .await
        .map_err(RestError::from_store)?;
    let requested_role_ids = body.role_ids.clone().unwrap_or_default();
    let requested_roles = validate_requested_policy_roles(&custom_roles, &requested_role_ids)?;
    reject_new_ai_role_assignments(&requested_roles, &current_assignments)?;
    let authorized_roles = policy_roles_touched_by_assignment_replace(
        &custom_roles,
        &requested_roles,
        &current_assignments,
    )?;
    ensure_policy_roles_inside_delegated_authority_for_operation(
        "replace_assignments",
        &principal,
        &authorized_roles,
    )?;
    ensure_policy_roles_inside_actor_permission_ceiling_for_operation(
        "replace_assignments",
        &principal,
        &authorized_roles,
    )?;
    ensure_assignment_preview_acknowledged(&principal, body.preview_acknowledged)?;
    let preview_receipt_id =
        require_assignment_preview_receipt(&principal, body.preview_receipt_id)?;
    verify_policy_step_up(&state, &principal, body.step_up).await?;
    let trace = TraceContext::generate();
    let assignments = state
        .store
        .replace_policy_role_assignments(ReplacePolicyRoleAssignmentsCommand {
            actor: principal.user_id,
            user_id,
            role_ids: requested_role_ids,
            preview_receipt_id,
            trace: trace.clone(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?
        .into_iter()
        .map(PolicyRoleAssignmentResponse::from)
        .collect::<Vec<_>>();
    record_policy_studio_operation("replace_assignments", "success");
    tracing::info!(
        event = "policy_studio_assignments_replaced",
        operation = "replace_assignments",
        outcome = "success",
        actor_user_id = %principal.user_id,
        target_user_id = %user_id,
        assignment_count = assignments.len(),
        audit_trace_id = trace.trace_id(),
        audit_span_id = trace.span_id(),
        branch_scope = policy_branch_scope_label(&principal.branch_scope),
        "policy studio assignments replaced"
    );
    Ok(Json(assignments))
}

async fn preview_policy_assignments(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<ReplacePolicyRoleAssignmentsRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let user_id = UserId::from_uuid(id);
    // Fine-grained, deny-by-omission authorization: an account-scope change
    // (system roles / branch memberships) is the UserManage tier (ADMIN), while
    // a custom policy-role change is the stricter RoleManage tier (SUPER_ADMIN).
    // No unconditional top-level RoleManage gate — that would 403 a legitimate
    // ADMIN doing an account-scope-only preview.
    let account_scope_preview = body.system_roles.is_some() || body.branch_ids.is_some();
    let explicit_custom_role_preview = body.role_ids.is_some();
    if account_scope_preview {
        authorize_org_manage(&principal, Feature::UserManage)?;
    }
    if !account_scope_preview || explicit_custom_role_preview {
        authorize_org_manage_observed(&state, &principal, Feature::RoleManage).await?;
    }
    let user = state
        .store
        .get_user(user_id, &principal.branch_scope)
        .await
        .map_err(RestError::from_store)?;
    let current_assignments = state
        .store
        .list_policy_role_assignments(user_id)
        .await
        .map_err(RestError::from_store)?;
    let policy_version = state
        .store
        .get_policy_version()
        .await
        .map_err(RestError::from_store)?;
    let custom_roles = state
        .store
        .list_policy_roles()
        .await
        .map_err(RestError::from_store)?;
    let current_ids = current_assignments
        .iter()
        .map(|assignment| assignment.role_id)
        .collect::<BTreeSet<_>>();
    let requested_role_id_input = match (&body.role_ids, account_scope_preview) {
        (Some(role_ids), _) => role_ids.clone(),
        (None, true) => current_ids.iter().copied().collect(),
        (None, false) => Vec::new(),
    };
    let requested_roles = validate_requested_policy_roles(&custom_roles, &requested_role_id_input)?;
    reject_new_ai_role_assignments(&requested_roles, &current_assignments)?;
    let current_system_roles = role_db_strings(&parse_roles(&user.roles)?);
    let requested_system_role_set = match &body.system_roles {
        Some(raw) => Some(parse_roles(raw)?),
        None => None,
    };
    let requested_system_roles = requested_system_role_set
        .as_ref()
        .map(role_db_strings)
        .unwrap_or_else(|| current_system_roles.clone());
    let current_branch_ids = user
        .branch_ids
        .iter()
        .map(|branch_id| *branch_id.as_uuid())
        .collect::<Vec<_>>();
    let requested_branch_ids = body
        .branch_ids
        .as_ref()
        .map(|branch_ids| {
            branch_ids
                .iter()
                .map(|branch_id| *branch_id.as_uuid())
                .collect()
        })
        .unwrap_or_else(|| current_branch_ids.clone());
    if account_scope_preview {
        let elevated_role_changes =
            elevated_role_membership_changes(&user.roles, requested_system_role_set.as_ref())?;
        let target_branches = body
            .branch_ids
            .as_deref()
            .unwrap_or(user.branch_ids.as_slice());
        authorize_user_write(
            &principal,
            &elevated_role_changes,
            user.branch_ids.as_slice(),
            target_branches,
        )?;
    }
    let requested_ids = requested_roles
        .iter()
        .map(|role| role.id)
        .collect::<BTreeSet<_>>();
    let authorized_roles = policy_roles_touched_by_assignment_replace(
        &custom_roles,
        &requested_roles,
        &current_assignments,
    )?;
    ensure_policy_roles_inside_delegated_authority_for_operation(
        "preview_assignments",
        &principal,
        &authorized_roles,
    )?;
    ensure_policy_roles_inside_actor_permission_ceiling_for_operation(
        "preview_assignments",
        &principal,
        &authorized_roles,
    )?;

    let requested_role_ids = requested_ids.iter().copied().collect::<Vec<_>>();
    let delta = PolicyRoleAssignmentDeltaResponse {
        added_role_ids: requested_ids.difference(&current_ids).copied().collect(),
        removed_role_ids: current_ids.difference(&requested_ids).copied().collect(),
        unchanged_role_ids: requested_ids.intersection(&current_ids).copied().collect(),
    };

    let mut feature_grants = Vec::new();
    for role_code in &requested_system_roles {
        let role = Role::from_str(role_code)
            .map_err(|_| RestError::validation("user has an unknown system role"))?;
        for feature in Feature::ALL {
            let permission = permission_for(role, feature);
            if matches!(permission, PermissionLevel::Deny) {
                continue;
            }
            feature_grants.push(PolicyFeatureGrantPreviewResponse {
                feature_key: feature.as_str().to_owned(),
                permission_level: permission.as_str().to_owned(),
                source_type: "system_role".to_owned(),
                source_key: role.as_str().to_owned(),
                source_label: role.as_str().to_owned(),
            });
        }
    }
    let mut runtime_warning_codes = BTreeSet::new();
    let mut custom_role_impacts = Vec::with_capacity(requested_roles.len());
    for role in &requested_roles {
        let runtime_decision = policy_role_runtime_decision_for_user(role, &user);
        for warning in &runtime_decision.warnings {
            runtime_warning_codes.insert(warning.clone());
        }
        if runtime_decision.effective {
            for permission in runtime_allowed_policy_permissions(role) {
                feature_grants.push(PolicyFeatureGrantPreviewResponse {
                    feature_key: permission.feature_key.clone(),
                    permission_level: permission.permission_level.clone(),
                    source_type: "custom_role".to_owned(),
                    source_key: role.role_key.clone(),
                    source_label: role.display_name.clone(),
                });
            }
        }
        custom_role_impacts.push(PolicyRoleImpactResponse {
            role_id: role.id,
            role_key: role.role_key.clone(),
            display_name: role.display_name.clone(),
            status: role.status.clone(),
            runtime_effective: runtime_decision.effective,
            runtime_warnings: runtime_decision.warnings,
            conditions: role
                .conditions
                .iter()
                .cloned()
                .map(policy_condition_response)
                .collect(),
        });
    }
    feature_grants.sort_by(|left, right| {
        (
            &left.feature_key,
            &left.source_type,
            &left.source_key,
            &left.permission_level,
        )
            .cmp(&(
                &right.feature_key,
                &right.source_type,
                &right.source_key,
                &right.permission_level,
            ))
    });

    let assignment_runtime_effective = custom_role_impacts
        .iter()
        .any(|role| role.runtime_effective);
    let mut warnings = vec!["preview_only_pending_save".to_owned()];
    if assignment_runtime_effective {
        warnings.push("active_assignments_become_runtime_effective_after_save".to_owned());
    }
    warnings.extend(runtime_warning_codes);

    let requested_role_count = requested_roles.len();
    let custom_roles = custom_role_impacts;
    let current_role_ids = current_ids.iter().copied().collect::<Vec<_>>();
    let preview_receipt = state
        .store
        .create_policy_assignment_preview_receipt(CreatePolicyAssignmentPreviewReceiptCommand {
            actor: principal.user_id,
            user_id,
            current_branch_ids: current_branch_ids.clone(),
            current_system_roles: current_system_roles.clone(),
            current_role_ids: current_role_ids.clone(),
            branch_ids: requested_branch_ids.clone(),
            system_roles: requested_system_roles.clone(),
            role_ids: requested_role_ids.clone(),
            policy_version: policy_version.version,
            expires_at: OffsetDateTime::now_utc() + POLICY_ASSIGNMENT_PREVIEW_RECEIPT_TTL,
        })
        .await
        .map_err(RestError::from_store)?;

    record_policy_studio_operation("preview_assignments", "success");
    tracing::info!(
        event = "policy_studio_assignment_previewed",
        operation = "preview_assignments",
        outcome = "success",
        actor_user_id = %principal.user_id,
        target_user_id = %user_id,
        requested_role_count,
        feature_grant_count = feature_grants.len(),
        branch_scope = policy_branch_scope_label(&principal.branch_scope),
        "policy studio assignment previewed"
    );

    Ok(Json(PolicyAssignmentPreviewResponse {
        user_id: *user_id.as_uuid(),
        preview_receipt_id: preview_receipt.id,
        preview_receipt_expires_at: preview_receipt.expires_at,
        effective: assignment_runtime_effective,
        system_roles: requested_system_roles.clone(),
        current_system_roles,
        requested_system_roles,
        current_branch_ids,
        requested_branch_ids,
        current_role_ids,
        requested_role_ids,
        delta,
        custom_roles,
        feature_grants,
        warnings,
    }))
}

fn policy_feature_catalog() -> Vec<PolicyFeatureResponse> {
    Feature::ALL
        .into_iter()
        .filter(|feature| policy_studio_feature_visible(*feature))
        .map(|feature| PolicyFeatureResponse {
            feature_key: feature.as_str().to_owned(),
            elevated: is_elevated_policy_feature(feature),
            default_permissions: Role::ALL
                .into_iter()
                .map(|role| PolicyDefaultPermissionResponse {
                    role_key: role.as_str().to_owned(),
                    permission_level: permission_for(role, feature).as_str().to_owned(),
                })
                .collect(),
        })
        .collect()
}

fn system_policy_roles() -> Vec<SystemPolicyRoleResponse> {
    Role::ALL
        .into_iter()
        .map(|role| SystemPolicyRoleResponse {
            role_key: role.as_str().to_owned(),
            display_name: role.as_str().to_owned(),
            status: "ACTIVE".to_owned(),
            is_system: true,
            permissions: Feature::ALL
                .into_iter()
                .filter(|feature| policy_studio_feature_visible(*feature))
                .map(|feature| PolicyPermissionResponse {
                    feature_key: feature.as_str().to_owned(),
                    permission_level: permission_for(role, feature).as_str().to_owned(),
                })
                .collect(),
        })
        .collect()
}

fn policy_role_templates() -> Vec<PolicyRoleTemplateResponse> {
    use Feature::{
        AssigneeManage, CompletionReview, DailyPlanRequest, DailyPlanReview,
        EmployeeDirectoryManage, EmployeeDirectoryRead, EquipmentCostLedgerRead, EquipmentManage,
        EvidenceAttach, ExcelDownload, KpiRead, MailUse, OpsDashboardRead, PurchaseRequestApprove,
        PurchaseRequestCreate, PurchaseRequestRead, RentalQuoteManage, SalesManage, TargetManage,
        WorkOrderCreate, WorkOrderEditIntake, WorkOrderReadAll, WorkOrderStart, WorkReportSubmit,
    };
    use PermissionLevel::{Allow, Limited, RequestOnly};

    vec![
        role_template(
            "branch_operations_manager",
            "branch_operations_manager",
            "지점 운영 관리자",
            "operations",
            "지점 단위 작업 흐름, 일일 계획 검토, 배정 조율을 담당합니다.",
            &[
                (WorkOrderReadAll, Allow),
                (DailyPlanReview, Allow),
                (CompletionReview, Allow),
                (AssigneeManage, Limited),
                (TargetManage, RequestOnly),
                (OpsDashboardRead, Limited),
            ],
        ),
        role_template(
            "dispatch_reception",
            "dispatch_reception",
            "접수·배차 코디네이터",
            "operations",
            "접수, 작업 생성, 고객/현장 연락, 기본 배차 보조를 담당합니다.",
            &[
                (WorkOrderCreate, Allow),
                (WorkOrderEditIntake, Allow),
                (WorkOrderReadAll, Allow),
                (TargetManage, RequestOnly),
                (MailUse, Allow),
            ],
        ),
        role_template(
            "site_operations",
            "site_operations",
            "현장 운영 담당자",
            "field_operations",
            "현장 작업 진행, 작업 보고, 증빙 첨부, 일일 계획 요청을 담당합니다.",
            &[
                (WorkOrderReadAll, Allow),
                (WorkOrderStart, Allow),
                (WorkReportSubmit, Allow),
                (EvidenceAttach, Allow),
                (DailyPlanRequest, RequestOnly),
            ],
        ),
        role_template(
            "security_guard",
            "security_guard",
            "경비 담당자",
            "security_operations",
            "현장 출입·안전 이슈를 접수하고 제한된 작업 현황과 증빙을 기록합니다.",
            &[
                (WorkOrderReadAll, Limited),
                (WorkOrderCreate, RequestOnly),
                (WorkReportSubmit, Limited),
                (EvidenceAttach, Limited),
            ],
        ),
        role_template(
            "cleaning_staff",
            "cleaning_staff",
            "미화 담당자",
            "cleaning_operations",
            "미화 작업 배정을 확인하고 완료 보고와 현장 증빙을 남깁니다.",
            &[
                (WorkOrderReadAll, Limited),
                (WorkOrderStart, Limited),
                (WorkReportSubmit, Allow),
                (EvidenceAttach, Limited),
                (DailyPlanRequest, RequestOnly),
            ],
        ),
        role_template(
            "dispatch_office_staff",
            "dispatch_office_staff",
            "파견사무 담당자",
            "dispatch_office",
            "파견사무 접수, 작업 생성·수정, 현장 연락과 기본 대상 변경 요청을 담당합니다.",
            &[
                (WorkOrderCreate, Allow),
                (WorkOrderEditIntake, Allow),
                (WorkOrderReadAll, Allow),
                (TargetManage, RequestOnly),
                (MailUse, Allow),
            ],
        ),
        role_template(
            "asset_cost_analyst",
            "asset_cost_analyst",
            "자산 비용 분석가",
            "finance",
            "장비 원가, KPI, 구매 조회를 분석하되 승인 권한은 별도로 요청합니다.",
            &[
                (EquipmentCostLedgerRead, Allow),
                (KpiRead, Allow),
                (PurchaseRequestRead, Allow),
                (ExcelDownload, Limited),
            ],
        ),
        role_template(
            "purchasing_requester",
            "purchasing_requester",
            "구매 요청 담당자",
            "finance",
            "구매 요청을 작성하고 진행 상태를 추적합니다.",
            &[(PurchaseRequestCreate, Allow), (PurchaseRequestRead, Allow)],
        ),
        role_template(
            "purchase_reviewer",
            "purchase_reviewer",
            "구매 검토자",
            "finance",
            "구매 요청을 조회하고 제한된 승인 검토를 수행합니다.",
            &[
                (PurchaseRequestRead, Allow),
                (PurchaseRequestApprove, Limited),
            ],
        ),
        role_template(
            "people_ops_manager",
            "people_ops_manager",
            "HR 운영 관리자",
            "people",
            "직원 디렉터리와 조직 기본 정보를 관리합니다. 로그인 사용자 권한 관리는 포함하지 않습니다.",
            &[
                (EmployeeDirectoryRead, Allow),
                (EmployeeDirectoryManage, Allow),
                (ExcelDownload, Limited),
            ],
        ),
        role_template(
            "inspection_coordinator",
            "inspection_coordinator",
            "검사 일정 코디네이터",
            "assets",
            "검사 일정과 라운드 완료를 조율하고 장비 정보 변경은 제한적으로 요청합니다.",
            &[
                (EquipmentManage, Limited),
                (Feature::InspectionScheduleManage, Allow),
                (Feature::InspectionRoundComplete, Limited),
                (WorkOrderReadAll, Allow),
            ],
        ),
        role_template(
            "sales_service_coordinator",
            "sales_service_coordinator",
            "영업·서비스 코디네이터",
            "customer",
            "렌탈 견적, 판매 문의, 회사 메일 기반 고객 응대를 담당합니다.",
            &[
                (RentalQuoteManage, Limited),
                (SalesManage, Limited),
                (MailUse, Allow),
                (WorkOrderReadAll, Limited),
            ],
        ),
    ]
}

fn role_template(
    template_key: &str,
    role_key: &str,
    display_name: &str,
    category: &str,
    description: &str,
    permissions: &[(Feature, PermissionLevel)],
) -> PolicyRoleTemplateResponse {
    PolicyRoleTemplateResponse {
        template_key: template_key.to_owned(),
        role_key: role_key.to_owned(),
        display_name: display_name.to_owned(),
        category: category.to_owned(),
        description: description.to_owned(),
        permissions: permissions
            .iter()
            .map(|(feature, level)| PolicyPermissionResponse {
                feature_key: feature.as_str().to_owned(),
                permission_level: level.as_str().to_owned(),
            })
            .collect(),
    }
}

impl From<PolicyVersionSummary> for PolicyVersionResponse {
    fn from(value: PolicyVersionSummary) -> Self {
        Self {
            version: value.version,
            updated_at: value.updated_at,
        }
    }
}

impl From<PolicyAuditEventSummary> for PolicyAuditEventResponse {
    fn from(value: PolicyAuditEventSummary) -> Self {
        Self {
            id: value.id,
            actor: value.actor.map(|user_id| *user_id.as_uuid()),
            action: value.action,
            target_type: value.target_type,
            target_id: value.target_id,
            before_snapshot: value.before_snapshot,
            after_snapshot: value.after_snapshot,
            trace_id: value.trace_id,
            span_id: value.span_id,
            occurred_at: value.occurred_at,
        }
    }
}

fn build_policy_role_status_preview(
    role: &PolicyRoleSummary,
    requested_status: String,
    planned_assignment_count: i64,
) -> PolicyRoleStatusPreviewResponse {
    let runtime_permission_count = runtime_allowed_policy_permissions(role).len();
    let effective_runtime_change = runtime_permission_count > 0
        && planned_assignment_count > 0
        && role.status != requested_status
        && (role.status == "ACTIVE" || requested_status == "ACTIVE");
    let mut warnings = vec!["passkey_step_up_required".to_owned()];
    if role.status == requested_status {
        warnings.push("no_status_change".to_owned());
    }
    if planned_assignment_count > 0 && runtime_permission_count > 0 {
        warnings.push("assigned_users_may_gain_or_lose_runtime_permissions".to_owned());
    }
    if requested_status == "DRAFT" && role.status == "ACTIVE" && effective_runtime_change {
        warnings.push("rollback_disables_assigned_custom_role_runtime_grants".to_owned());
    }
    if requested_status == "RETIRED" && role.status == "ACTIVE" && effective_runtime_change {
        warnings.push("retire_disables_assigned_custom_role_runtime_grants".to_owned());
    }
    if requested_status == "ACTIVE" && role.status != requested_status && effective_runtime_change {
        warnings.push("publish_enables_assigned_custom_role_runtime_grants".to_owned());
    }

    PolicyRoleStatusPreviewResponse {
        role_id: role.id,
        role_key: role.role_key.clone(),
        display_name: role.display_name.clone(),
        current_status: role.status.clone(),
        requested_status,
        permission_count: runtime_permission_count as i64,
        condition_count: role.conditions.len() as i64,
        planned_assignment_count,
        requires_passkey_step_up: true,
        effective_runtime_change,
        warnings,
    }
}

impl From<PolicyRoleSummary> for PolicyRoleResponse {
    fn from(value: PolicyRoleSummary) -> Self {
        Self {
            id: value.id,
            role_key: value.role_key,
            display_name: value.display_name,
            description: value.description,
            status: value.status,
            is_system: value.is_system,
            permissions: value
                .permissions
                .into_iter()
                .filter(|permission| policy_feature_key_visible(&permission.feature_key))
                .map(|permission| PolicyPermissionResponse {
                    feature_key: permission.feature_key,
                    permission_level: permission.permission_level,
                })
                .collect(),
            conditions: value
                .conditions
                .into_iter()
                .map(policy_condition_response)
                .collect(),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

fn policy_condition_response(condition: PolicyRoleCondition) -> PolicyConditionResponse {
    PolicyConditionResponse {
        condition_key: condition.condition_key,
        attribute: condition.attribute,
        operator: condition.operator,
        values: condition.values,
    }
}

impl From<PolicyRoleAssignmentSummary> for PolicyRoleAssignmentResponse {
    fn from(value: PolicyRoleAssignmentSummary) -> Self {
        Self {
            user_id: *value.user_id.as_uuid(),
            role_id: value.role_id,
            role_key: value.role_key,
            display_name: value.display_name,
            status: value.status,
            assigned_by: value.assigned_by.map(|user_id| *user_id.as_uuid()),
            created_at: value.created_at,
        }
    }
}

fn normalize_policy_role_key(raw: &str) -> Result<String, RestError> {
    let value = raw.trim();
    if value.len() < 2 || value.len() > 64 {
        return Err(RestError::validation(
            "role key must be between 2 and 64 characters",
        ));
    }
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(RestError::validation("role key is required"));
    };
    if !first.is_ascii_lowercase() {
        return Err(RestError::validation(
            "role key must start with a lowercase ascii letter",
        ));
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        return Err(RestError::validation(
            "role key may contain lowercase ascii letters, digits, and underscores only",
        ));
    }
    if Role::ALL
        .into_iter()
        .any(|role| value.eq_ignore_ascii_case(role.as_str()))
    {
        return Err(RestError::validation(
            "custom role key must not shadow a built-in role",
        ));
    }
    Ok(value.to_owned())
}

fn normalize_policy_display_name(raw: &str) -> Result<String, RestError> {
    let value = raw.trim();
    if value.is_empty() || value.chars().count() > 80 {
        return Err(RestError::validation(
            "display name must be between 1 and 80 characters",
        ));
    }
    Ok(value.to_owned())
}

fn normalize_policy_description(raw: Option<&str>) -> Result<Option<String>, RestError> {
    let Some(value) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > 512 {
        return Err(RestError::validation(
            "description must be 512 characters or fewer",
        ));
    }
    Ok(Some(value.to_owned()))
}

fn validate_policy_permissions(
    raw: &[PolicyPermissionResponse],
    allow_empty: bool,
) -> Result<Vec<PolicyRolePermission>, RestError> {
    let mut seen = BTreeSet::new();
    let mut permissions = Vec::new();
    for permission in raw {
        let feature = Feature::from_str(&permission.feature_key)
            .map_err(|_| RestError::validation("unknown feature key"))?;
        if !custom_role_runtime_feature_allowed(feature) {
            return Err(RestError::forbidden(
                "custom roles cannot grant elevated or scope-widening policy features yet",
            ));
        }
        let level = PermissionLevel::from_str(&permission.permission_level)
            .map_err(|_| RestError::validation("unknown permission level"))?;
        if !seen.insert(feature) {
            return Err(RestError::validation("duplicate feature permission"));
        }
        if matches!(level, PermissionLevel::Deny) {
            continue;
        }
        permissions.push(PolicyRolePermission {
            feature_key: feature.as_str().to_owned(),
            permission_level: level.as_str().to_owned(),
        });
    }
    if permissions.is_empty() && !allow_empty {
        return Err(RestError::validation(
            "custom role must grant at least one non-deny feature",
        ));
    }
    Ok(permissions)
}

fn validate_policy_conditions(
    raw: &[PolicyConditionResponse],
) -> Result<Vec<PolicyRoleCondition>, RestError> {
    if raw.len() > 20 {
        return Err(RestError::validation(
            "custom role may define at most 20 policy conditions",
        ));
    }

    let mut seen_keys = BTreeSet::new();
    let mut conditions = Vec::with_capacity(raw.len());
    for condition in raw {
        let condition_key = normalize_policy_condition_key(&condition.condition_key)?;
        if !seen_keys.insert(condition_key.clone()) {
            return Err(RestError::validation("duplicate policy condition key"));
        }
        let attribute = normalize_policy_condition_attribute(&condition.attribute)?;
        let operator = normalize_policy_condition_operator(&condition.operator)?;
        let values = normalize_policy_condition_values(&condition.values)?;
        conditions.push(PolicyRoleCondition {
            condition_key,
            attribute,
            operator,
            values,
        });
    }
    Ok(conditions)
}

fn normalize_policy_condition_key(raw: &str) -> Result<String, RestError> {
    let value = raw.trim();
    if value.len() < 2 || value.len() > 64 {
        return Err(RestError::validation(
            "condition key must be between 2 and 64 characters",
        ));
    }
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(RestError::validation("condition key is required"));
    };
    if !first.is_ascii_lowercase()
        || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(RestError::validation(
            "condition key may contain lowercase ascii letters, digits, and underscores only",
        ));
    }
    Ok(value.to_owned())
}

fn normalize_policy_condition_attribute(raw: &str) -> Result<String, RestError> {
    let value = raw.trim().to_ascii_lowercase();
    match value.as_str() {
        "group" | "tenant" | "organization" | "org" | "department" | "team" | "position"
        | "employment_status" | "assignment" | "location" | "site" | "branch"
        | "device_posture" | "purpose" | "action" | "resource" | "sensitive_action" => Ok(value),
        _ => Err(RestError::validation("unknown policy condition attribute")),
    }
}

fn normalize_policy_condition_operator(raw: &str) -> Result<String, RestError> {
    let value = raw.trim().to_ascii_lowercase();
    match value.as_str() {
        "equals" | "not_equals" | "in" => Ok(value),
        _ => Err(RestError::validation("unknown policy condition operator")),
    }
}

fn normalize_policy_condition_values(raw: &[String]) -> Result<Vec<String>, RestError> {
    if raw.is_empty() || raw.len() > 20 {
        return Err(RestError::validation(
            "policy condition values must contain 1 to 20 entries",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut values = Vec::with_capacity(raw.len());
    for value in raw {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.chars().count() > 120 {
            return Err(RestError::validation(
                "policy condition values must be non-empty and 120 characters or fewer",
            ));
        }
        if trimmed.chars().any(char::is_control) {
            return Err(RestError::validation(
                "policy condition values must not contain control characters",
            ));
        }
        if !seen.insert(trimmed.to_owned()) {
            return Err(RestError::validation("duplicate policy condition value"));
        }
        values.push(trimmed.to_owned());
    }
    Ok(values)
}

fn validate_requested_policy_roles(
    custom_roles: &[PolicyRoleSummary],
    raw_role_ids: &[Uuid],
) -> Result<Vec<PolicyRoleSummary>, RestError> {
    let requested_ids = raw_role_ids.iter().copied().collect::<BTreeSet<_>>();
    let mut roles = Vec::with_capacity(requested_ids.len());
    for requested_id in requested_ids {
        let Some(role) = custom_roles
            .iter()
            .find(|role| role.id == requested_id && !role.is_system && role.status != "RETIRED")
        else {
            return Err(RestError::validation(
                "preview references an unknown or retired custom role",
            ));
        };
        roles.push(role.clone());
    }
    Ok(roles)
}

fn policy_roles_touched_by_assignment_replace(
    custom_roles: &[PolicyRoleSummary],
    requested_roles: &[PolicyRoleSummary],
    current_assignments: &[PolicyRoleAssignmentSummary],
) -> Result<Vec<PolicyRoleSummary>, RestError> {
    let requested_ids = requested_roles
        .iter()
        .map(|role| role.id)
        .collect::<BTreeSet<_>>();
    let mut roles = requested_roles.to_vec();
    for removed_id in current_assignments
        .iter()
        .map(|assignment| assignment.role_id)
        .collect::<BTreeSet<_>>()
        .difference(&requested_ids)
        .copied()
    {
        let Some(role) = custom_roles
            .iter()
            .find(|role| role.id == removed_id && !role.is_system)
        else {
            return Err(RestError::validation(
                "assignment references an unknown custom role",
            ));
        };
        roles.push(role.clone());
    }
    Ok(roles)
}

fn reject_new_ai_role_assignments(
    requested_roles: &[PolicyRoleSummary],
    current_assignments: &[PolicyRoleAssignmentSummary],
) -> Result<(), RestError> {
    let current_ids = current_assignments
        .iter()
        .map(|assignment| assignment.role_id)
        .collect::<BTreeSet<_>>();
    if requested_roles.iter().any(|role| {
        !current_ids.contains(&role.id)
            && role
                .permissions
                .iter()
                .any(|permission| permission.feature_key == Feature::AiAssist.as_str())
    }) {
        return Err(RestError::forbidden(
            "retired AI roles cannot be newly assigned",
        ));
    }
    Ok(())
}

fn ensure_policy_roles_inside_delegated_authority_for_operation(
    operation: &'static str,
    principal: &Principal,
    roles: &[PolicyRoleSummary],
) -> Result<(), RestError> {
    for role in roles {
        if let Err(error) =
            ensure_policy_conditions_inside_delegated_authority(principal, &role.conditions)
        {
            record_policy_studio_rejection(operation, principal, &error);
            return Err(error);
        }
    }
    Ok(())
}

fn ensure_policy_roles_inside_actor_permission_ceiling_for_operation(
    operation: &'static str,
    principal: &Principal,
    roles: &[PolicyRoleSummary],
) -> Result<(), RestError> {
    if let Err(error) = ensure_policy_roles_inside_actor_permission_ceiling(principal, roles) {
        record_policy_studio_rejection(operation, principal, &error);
        return Err(error);
    }
    Ok(())
}

fn ensure_policy_roles_inside_actor_permission_ceiling(
    principal: &Principal,
    roles: &[PolicyRoleSummary],
) -> Result<(), RestError> {
    for role in roles {
        let role_scope = policy_role_assignment_branch_scope(principal, role)?;
        for permission in &role.permissions {
            let feature = Feature::from_str(&permission.feature_key)
                .map_err(|_| RestError::validation("unknown feature key"))?;
            if feature == Feature::AiAssist {
                continue;
            }
            let requested = PermissionLevel::from_str(&permission.permission_level)
                .map_err(|_| RestError::validation("unknown permission level"))?;
            if matches!(requested, PermissionLevel::Deny) {
                continue;
            }
            if !custom_role_runtime_feature_allowed(feature) {
                return Err(RestError::forbidden(
                    "custom roles cannot grant elevated or scope-widening policy features yet",
                ));
            }
            if policy_feature_assignment_requires_elevated_grant(feature)
                && !principal_holds_policy_permission(
                    principal,
                    Feature::ElevatedRoleGrant,
                    PermissionLevel::Allow,
                    &role_scope,
                )
            {
                return Err(RestError::forbidden(
                    "elevated policy role assignments require elevated role grant",
                ));
            }
            if !principal_holds_policy_permission(principal, feature, requested, &role_scope) {
                return Err(RestError::forbidden(
                    "policy role permission exceeds delegated authority",
                ));
            }
        }
    }
    Ok(())
}

fn policy_role_assignment_branch_scope(
    principal: &Principal,
    role: &PolicyRoleSummary,
) -> Result<BranchScope, RestError> {
    let mut scope = principal.branch_scope.clone();
    for condition in role
        .conditions
        .iter()
        .filter(|condition| condition.attribute == "branch")
    {
        if !matches!(condition.operator.as_str(), "equals" | "in") {
            // Unsupported branch operators are not a safe narrowing proof. Keep
            // the current scope rather than assuming the role is branch-limited.
            continue;
        }
        let mut branches = BTreeSet::new();
        for value in &condition.values {
            let branch_uuid = Uuid::parse_str(value).map_err(|_| {
                RestError::validation(
                    "branch condition values must be branch UUIDs for delegated policy management",
                )
            })?;
            branches.insert(BranchId::from_uuid(branch_uuid));
        }
        scope = scope.intersect(&BranchScope::Branches(branches));
    }
    Ok(scope)
}

fn principal_holds_policy_permission(
    principal: &Principal,
    feature: Feature,
    requested: PermissionLevel,
    scope: &BranchScope,
) -> bool {
    principal.roles.iter().any(|role| {
        policy_permission_satisfies(permission_for(*role, feature), requested)
            && branch_scope_contains(&principal.branch_scope, scope)
    }) || principal.effective_feature_grants.iter().any(|grant| {
        grant.feature == feature
            && policy_permission_satisfies(grant.permission, requested)
            && branch_scope_contains(&grant.branch_scope, scope)
    })
}

fn policy_permission_satisfies(held: PermissionLevel, requested: PermissionLevel) -> bool {
    match requested {
        PermissionLevel::Deny => true,
        PermissionLevel::Allow => matches!(held, PermissionLevel::Allow),
        PermissionLevel::Limited => {
            matches!(held, PermissionLevel::Allow | PermissionLevel::Limited)
        }
        PermissionLevel::RequestOnly => {
            matches!(held, PermissionLevel::Allow | PermissionLevel::RequestOnly)
        }
    }
}

fn branch_scope_contains(container: &BranchScope, contained: &BranchScope) -> bool {
    match (container, contained) {
        (BranchScope::All, _) => true,
        (BranchScope::Branches(_), BranchScope::All) => false,
        (BranchScope::Branches(container), BranchScope::Branches(contained)) => {
            contained.is_subset(container)
        }
    }
}

fn policy_feature_assignment_requires_elevated_grant(feature: Feature) -> bool {
    if matches!(feature, Feature::UserManage) {
        return true;
    }
    let super_admin_allows = permission_for(Role::SuperAdmin, feature) == PermissionLevel::Allow;
    let non_super_admin_allows = Role::ALL
        .into_iter()
        .filter(|role| *role != Role::SuperAdmin)
        .any(|role| permission_for(role, feature) == PermissionLevel::Allow);
    super_admin_allows && !non_super_admin_allows
}

fn ensure_policy_conditions_inside_delegated_authority_for_operation(
    operation: &'static str,
    principal: &Principal,
    conditions: &[PolicyRoleCondition],
) -> Result<(), RestError> {
    if let Err(error) = ensure_policy_conditions_inside_delegated_authority(principal, conditions) {
        record_policy_studio_rejection(operation, principal, &error);
        return Err(error);
    }
    Ok(())
}

fn policy_role_is_inside_delegated_authority(
    principal: &Principal,
    role: &PolicyRoleSummary,
) -> bool {
    ensure_policy_conditions_inside_delegated_authority(principal, &role.conditions).is_ok()
}

fn ensure_policy_conditions_inside_delegated_authority(
    principal: &Principal,
    conditions: &[PolicyRoleCondition],
) -> Result<(), RestError> {
    let BranchScope::Branches(allowed_branches) = &principal.branch_scope else {
        return Ok(());
    };
    if allowed_branches.is_empty() {
        return Err(RestError::forbidden(
            "delegated policy managers must have at least one branch in scope",
        ));
    }

    let branch_conditions = conditions
        .iter()
        .filter(|condition| condition.attribute == "branch")
        .collect::<Vec<_>>();
    if branch_conditions.is_empty() {
        return Err(RestError::forbidden(
            "branch-scoped policy managers must include a branch condition",
        ));
    }

    for condition in branch_conditions {
        if condition.operator == "not_equals" {
            return Err(RestError::forbidden(
                "branch-scoped policy managers cannot use negative branch conditions",
            ));
        }
        for value in &condition.values {
            let branch_uuid = Uuid::parse_str(value).map_err(|_| {
                RestError::validation(
                    "branch condition values must be branch UUIDs for delegated policy management",
                )
            })?;
            let branch_id = BranchId::from_uuid(branch_uuid);
            if !allowed_branches.contains(&branch_id) {
                return Err(RestError::forbidden(
                    "policy role branch condition is outside delegated scope",
                ));
            }
        }
    }

    Ok(())
}

fn ensure_assignment_preview_acknowledged(
    principal: &Principal,
    preview_acknowledged: bool,
) -> Result<(), RestError> {
    if preview_acknowledged {
        return Ok(());
    }
    let error = RestError::validation("assignment preview must be acknowledged before saving");
    record_policy_studio_rejection("replace_assignments", principal, &error);
    Err(error)
}

fn require_assignment_preview_receipt(
    principal: &Principal,
    preview_receipt_id: Option<Uuid>,
) -> Result<Uuid, RestError> {
    let Some(preview_receipt_id) = preview_receipt_id else {
        let error = RestError::validation("assignment preview receipt is required before saving");
        record_policy_studio_rejection("replace_assignments", principal, &error);
        return Err(error);
    };
    Ok(preview_receipt_id)
}

async fn verify_policy_step_up(
    state: &IdentityRestState,
    principal: &Principal,
    step_up: Option<PolicyStepUpAssertionRequest>,
) -> Result<(), RestError> {
    let step_up = step_up.ok_or_else(|| {
        RestError::new(
            StatusCode::PRECONDITION_REQUIRED,
            "passkey_step_up_required",
            "policy changes require a fresh passkey step-up",
        )
    })?;
    let verifier = state.passkey_step_up.as_ref().ok_or_else(|| {
        RestError::unavailable("passkey step-up is not configured for identity API")
    })?;
    verifier
        .verify_step_up_for_user(
            state.pool(),
            step_up.ceremony_id,
            step_up.credential,
            *principal.user_id.as_uuid(),
        )
        .await
        .map_err(|_| RestError::unauthorized("passkey step-up failed"))?;
    Ok(())
}

fn normalize_policy_role_status(raw: &str) -> Result<String, RestError> {
    let status = raw.trim().to_ascii_uppercase();
    match status.as_str() {
        "DRAFT" | "ACTIVE" | "RETIRED" => Ok(status),
        _ => Err(RestError::validation(
            "policy role status must be DRAFT, ACTIVE, or RETIRED",
        )),
    }
}

fn validate_policy_role_status_transition(
    current_status: &str,
    requested_status: &str,
) -> Result<(), RestError> {
    if current_status == requested_status {
        return Ok(());
    }
    match (current_status, requested_status) {
        ("DRAFT", "ACTIVE") | ("ACTIVE", "DRAFT") | ("ACTIVE", "RETIRED") => Ok(()),
        _ => Err(RestError::validation(
            "policy role status transition is not allowed",
        )),
    }
}

fn normalize_policy_audit_limit(raw: Option<i64>) -> Result<i64, RestError> {
    let limit = raw.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return Err(RestError::validation(
            "policy audit limit must be between 1 and 100",
        ));
    }
    Ok(limit)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PolicyRoleRuntimeDecision {
    effective: bool,
    warnings: Vec<String>,
}

fn policy_role_runtime_decision_for_user(
    role: &PolicyRoleSummary,
    user: &UserSummary,
) -> PolicyRoleRuntimeDecision {
    let mut warnings = Vec::new();

    if role.status != "ACTIVE" {
        warnings.push("custom_role_status_not_active".to_owned());
    }

    let live_scope = policy_preview_branch_scope_for_user(user);
    match effective_scope_for_policy_preview_conditions(user, &live_scope, &role.conditions) {
        Ok(role_scope) if role_scope.is_empty() => {
            warnings.push("custom_role_condition_outside_target_branch_scope".to_owned());
        }
        Ok(_) => {}
        Err(reason) => warnings.push(reason.to_owned()),
    }

    if runtime_allowed_policy_permissions(role).is_empty() {
        warnings.push("custom_role_no_runtime_allowed_permissions".to_owned());
    }

    PolicyRoleRuntimeDecision {
        effective: warnings.is_empty(),
        warnings,
    }
}

fn runtime_allowed_policy_permissions(role: &PolicyRoleSummary) -> Vec<&PolicyRolePermission> {
    role.permissions
        .iter()
        .filter(|permission| {
            let Ok(feature) = Feature::from_str(&permission.feature_key) else {
                return false;
            };
            let Ok(level) = PermissionLevel::from_str(&permission.permission_level) else {
                return false;
            };
            level != PermissionLevel::Deny && custom_role_runtime_feature_allowed(feature)
        })
        .collect()
}

fn policy_preview_branch_scope_for_user(user: &UserSummary) -> BranchScope {
    let has_org_wide_system_role = user
        .roles
        .iter()
        .filter_map(|role| Role::from_str(role).ok())
        .any(|role| matches!(role, Role::Executive | Role::SuperAdmin));
    if has_org_wide_system_role {
        return BranchScope::All;
    }

    BranchScope::Branches(user.branch_ids.iter().copied().collect())
}

fn effective_scope_for_policy_preview_conditions(
    user: &UserSummary,
    live_scope: &BranchScope,
    conditions: &[PolicyRoleCondition],
) -> Result<BranchScope, &'static str> {
    let mut scope = live_scope.clone();
    for condition in conditions {
        if !matches!(condition.operator.as_str(), "equals" | "in") {
            return Err("custom_role_condition_unsupported_by_runtime_evaluator");
        }

        match condition.attribute.as_str() {
            "branch" => {
                let mut branches = BTreeSet::new();
                for value in &condition.values {
                    let branch = BranchId::from_str(value)
                        .map_err(|_| "custom_role_condition_invalid_branch_value")?;
                    branches.insert(branch);
                }
                scope = scope.intersect(&BranchScope::Branches(branches));
            }
            "team" => {
                if !team_condition_matches(user.team, &condition.values) {
                    return Err("custom_role_condition_outside_target_attributes");
                }
            }
            _ => return Err("custom_role_condition_unsupported_by_runtime_evaluator"),
        }
    }
    Ok(scope)
}

fn team_condition_matches(team: Option<Team>, values: &[String]) -> bool {
    let Some(team) = team else {
        return false;
    };
    let accepted = team_policy_values(team);
    values.iter().any(|value| {
        let value = value.trim();
        accepted
            .iter()
            .any(|accepted| value == *accepted || value.eq_ignore_ascii_case(accepted))
    })
}

fn team_policy_values(team: Team) -> [&'static str; 2] {
    match team {
        Team::Maintenance => ["MAINTENANCE", Team::Maintenance.as_db_str()],
        Team::Prevention => ["PREVENTION", Team::Prevention.as_db_str()],
        Team::Management => ["MANAGEMENT", Team::Management.as_db_str()],
        Team::Reception => ["RECEPTION", Team::Reception.as_db_str()],
    }
}

/// Policy Studio UI catalog / GET-response visibility — narrower than the
/// runtime safety predicate [`custom_role_runtime_feature_allowed`].
///
/// V1 retains the historical AI wire key for persisted policy evidence, but
/// never exposes it as a current permission or catalog affordance. Runtime
/// grant folding and write validation use the authz safety predicate.
fn policy_studio_feature_visible(feature: Feature) -> bool {
    !matches!(
        feature,
        Feature::AiAssist | Feature::AuditStreamRead | Feature::AuditStreamAccessLogRead
    )
}

fn policy_feature_key_visible(feature_key: &str) -> bool {
    Feature::from_str(feature_key)
        .map(policy_studio_feature_visible)
        .unwrap_or(true)
}

fn is_elevated_policy_feature(feature: Feature) -> bool {
    !custom_role_runtime_feature_allowed(feature)
}

#[cfg(test)]
mod policy_role_template_tests {
    use super::*;

    fn policy_role_for_test(
        role_key: &str,
        permissions: &[(&str, &str)],
        conditions: Vec<PolicyRoleCondition>,
    ) -> PolicyRoleSummary {
        PolicyRoleSummary {
            id: Uuid::new_v4(),
            role_key: role_key.to_owned(),
            display_name: role_key.to_owned(),
            description: None,
            status: "ACTIVE".to_owned(),
            is_system: false,
            permissions: permissions
                .iter()
                .map(|(feature_key, permission_level)| PolicyRolePermission {
                    feature_key: (*feature_key).to_owned(),
                    permission_level: (*permission_level).to_owned(),
                })
                .collect(),
            conditions,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        }
    }

    fn branch_condition(branch: BranchId) -> PolicyRoleCondition {
        PolicyRoleCondition {
            condition_key: "branch_scope".to_owned(),
            attribute: "branch".to_owned(),
            operator: "equals".to_owned(),
            values: vec![branch.to_string()],
        }
    }

    #[test]
    fn assignment_ceiling_rejects_capability_actor_does_not_hold() {
        let branch = BranchId::new();
        let principal = Principal::new(
            UserId::new(),
            console_kernel_core::OrgId::knl(),
            BTreeSet::from([Role::Admin]),
            BranchScope::single(branch),
        );
        let final_approval_role = policy_role_for_test(
            "final_approval_delegate",
            &[("purchase_final_approve", "allow")],
            vec![branch_condition(branch)],
        );

        let error =
            ensure_policy_roles_inside_actor_permission_ceiling(&principal, &[final_approval_role])
                .unwrap_err();

        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(error.code, "forbidden");
        assert_eq!(
            error.message,
            "policy role permission exceeds delegated authority"
        );
    }

    #[test]
    fn assignment_ceiling_requires_custom_grant_scope_to_cover_role_scope() {
        let allowed_branch = BranchId::new();
        let blocked_branch = BranchId::new();
        let principal = Principal::new(
            UserId::new(),
            console_kernel_core::OrgId::knl(),
            BTreeSet::from([Role::Member]),
            BranchScope::Branches(BTreeSet::from([allowed_branch, blocked_branch])),
        )
        .with_effective_feature_grants(vec![
            console_platform_authz::EffectiveFeatureGrant::new(
                Feature::WorkOrderCreate,
                PermissionLevel::Allow,
                BranchScope::single(allowed_branch),
            ),
        ]);
        let allowed_role = policy_role_for_test(
            "allowed_branch_creator",
            &[("work_order_create", "allow")],
            vec![branch_condition(allowed_branch)],
        );
        let blocked_role = policy_role_for_test(
            "blocked_branch_creator",
            &[("work_order_create", "allow")],
            vec![branch_condition(blocked_branch)],
        );

        ensure_policy_roles_inside_actor_permission_ceiling(&principal, &[allowed_role]).unwrap();
        let error =
            ensure_policy_roles_inside_actor_permission_ceiling(&principal, &[blocked_role])
                .unwrap_err();

        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(
            error.message,
            "policy role permission exceeds delegated authority"
        );
    }

    #[test]
    fn assignment_ceiling_requires_elevated_grant_for_user_management_roles() {
        let branch = BranchId::new();
        let admin = Principal::new(
            UserId::new(),
            console_kernel_core::OrgId::knl(),
            BTreeSet::from([Role::Admin]),
            BranchScope::single(branch),
        );
        let super_admin = Principal::new(
            UserId::new(),
            console_kernel_core::OrgId::knl(),
            BTreeSet::from([Role::SuperAdmin]),
            BranchScope::single(branch),
        );
        let user_manager = policy_role_for_test(
            "user_manager_delegate",
            &[("user_manage", "allow")],
            vec![branch_condition(branch)],
        );

        let error = ensure_policy_roles_inside_actor_permission_ceiling(
            &admin,
            std::slice::from_ref(&user_manager),
        )
        .unwrap_err();
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(
            error.message,
            "elevated policy role assignments require elevated role grant"
        );
        ensure_policy_roles_inside_actor_permission_ceiling(&super_admin, &[user_manager]).unwrap();
    }

    #[test]
    fn role_templates_are_unique_non_empty_and_never_grant_elevated_policy_features() {
        let templates = policy_role_templates();
        assert!(
            templates.len() >= 12,
            "expected enterprise starter coverage"
        );

        let mut template_keys = BTreeSet::new();
        let mut role_keys = BTreeSet::new();
        for template in templates {
            assert!(template_keys.insert(template.template_key.clone()));
            assert!(role_keys.insert(template.role_key.clone()));
            assert!(!template.display_name.trim().is_empty());
            assert!(!template.category.trim().is_empty());
            assert!(!template.permissions.is_empty());
            for permission in template.permissions {
                let feature = Feature::from_str(&permission.feature_key).unwrap();
                assert!(
                    !is_elevated_policy_feature(feature),
                    "template {} grants elevated feature {}",
                    template.template_key,
                    permission.feature_key
                );
                assert!(
                    !matches!(feature, Feature::UserManage),
                    "template {} grants admin user-management feature {}",
                    template.template_key,
                    permission.feature_key
                );
                let level = PermissionLevel::from_str(&permission.permission_level).unwrap();
                assert!(!matches!(level, PermissionLevel::Deny));
            }
        }
    }

    #[test]
    fn operational_persona_templates_cover_approved_role_set() {
        let templates = policy_role_templates();

        let find_template = |key: &str| {
            templates
                .iter()
                .find(|template| template.template_key == key)
                .unwrap_or_else(|| panic!("missing role template {key}"))
        };
        let assert_permissions = |template: &PolicyRoleTemplateResponse,
                                  expected: &[(&str, &str)]| {
            let actual: BTreeSet<(&str, &str)> = template
                .permissions
                .iter()
                .map(|permission| {
                    (
                        permission.feature_key.as_str(),
                        permission.permission_level.as_str(),
                    )
                })
                .collect();
            let expected: BTreeSet<(&str, &str)> = expected.iter().copied().collect();
            assert_eq!(actual, expected, "template {}", template.template_key);
        };

        let site = find_template("site_operations");
        assert_eq!(site.role_key, "site_operations");
        assert_eq!(site.display_name, "현장 운영 담당자");
        assert_eq!(site.category, "field_operations");
        assert_permissions(
            site,
            &[
                ("work_order_read_all", "allow"),
                ("work_order_start", "allow"),
                ("work_report_submit", "allow"),
                ("evidence_attach", "allow"),
                ("daily_plan_request", "request_only"),
            ],
        );

        let security = find_template("security_guard");
        assert_eq!(security.role_key, "security_guard");
        assert_eq!(security.display_name, "경비 담당자");
        assert_eq!(security.category, "security_operations");
        assert_permissions(
            security,
            &[
                ("work_order_read_all", "limited"),
                ("work_order_create", "request_only"),
                ("work_report_submit", "limited"),
                ("evidence_attach", "limited"),
            ],
        );

        let cleaning = find_template("cleaning_staff");
        assert_eq!(cleaning.role_key, "cleaning_staff");
        assert_eq!(cleaning.display_name, "미화 담당자");
        assert_eq!(cleaning.category, "cleaning_operations");
        assert_permissions(
            cleaning,
            &[
                ("work_order_read_all", "limited"),
                ("work_order_start", "limited"),
                ("work_report_submit", "allow"),
                ("evidence_attach", "limited"),
                ("daily_plan_request", "request_only"),
            ],
        );

        let dispatch_office = find_template("dispatch_office_staff");
        assert_eq!(dispatch_office.role_key, "dispatch_office_staff");
        assert_eq!(dispatch_office.display_name, "파견사무 담당자");
        assert_eq!(dispatch_office.category, "dispatch_office");
        assert_permissions(
            dispatch_office,
            &[
                ("work_order_create", "allow"),
                ("work_order_edit_intake", "allow"),
                ("work_order_read_all", "allow"),
                ("target_manage", "request_only"),
                ("mail_use", "allow"),
            ],
        );
    }

    #[test]
    fn policy_studio_quarantines_deferred_ai_assist_until_adapter_exists() {
        let feature_catalog = policy_feature_catalog();
        assert!(
            feature_catalog
                .iter()
                .all(|feature| feature.feature_key != "ai_assist"),
            "deferred AI assistant permission must not appear in the Policy Studio feature catalog"
        );

        for role in system_policy_roles() {
            assert!(
                role.permissions
                    .iter()
                    .all(|permission| permission.feature_key != "ai_assist"),
                "deferred AI assistant permission must not appear in system role metadata for {}",
                role.role_key
            );
        }

        let legacy_role = PolicyRoleResponse::from(policy_role_for_test(
            "legacy_ai_assist",
            &[("ai_assist", "allow"), ("work_order_create", "allow")],
            vec![],
        ));
        assert!(
            legacy_role
                .permissions
                .iter()
                .all(|permission| permission.feature_key != "ai_assist"),
            "deferred AI assistant permission must be hidden from existing custom role responses"
        );
        assert!(
            legacy_role
                .permissions
                .iter()
                .any(|permission| permission.feature_key == "work_order_create"),
            "visible custom-role permissions should remain intact"
        );

        assert!(
            !custom_role_runtime_feature_allowed(Feature::AiAssist),
            "retired AI permission must not become a runtime custom-role grant"
        );
        assert!(
            validate_policy_permissions(
                &[PolicyPermissionResponse {
                    feature_key: "ai_assist".to_owned(),
                    permission_level: "allow".to_owned(),
                }],
                false
            )
            .is_err()
        );

        let mut ai_only = policy_role_for_test("legacy_ai_only", &[("ai_assist", "allow")], vec![]);
        ai_only.status = "DRAFT".to_owned();
        let preview = build_policy_role_status_preview(&ai_only, "ACTIVE".to_owned(), 1);
        assert_eq!(preview.permission_count, 0);
        assert!(!preview.effective_runtime_change);
        assert!(
            !preview
                .warnings
                .iter()
                .any(|warning| warning == "publish_enables_assigned_custom_role_runtime_grants")
        );
        assert!(
            !preview
                .warnings
                .iter()
                .any(|warning| warning == "assigned_users_may_gain_or_lose_runtime_permissions")
        );
    }

    #[test]
    fn policy_condition_validation_accepts_scoped_abac_pbac_metadata_only() {
        let conditions = validate_policy_conditions(&[
            PolicyConditionResponse {
                condition_key: "dept_scope".to_owned(),
                attribute: "department".to_owned(),
                operator: "in".to_owned(),
                values: vec!["정비팀".to_owned(), "야간조".to_owned()],
            },
            PolicyConditionResponse {
                condition_key: "purpose_scope".to_owned(),
                attribute: "purpose".to_owned(),
                operator: "equals".to_owned(),
                values: vec!["work_order_approval".to_owned()],
            },
        ])
        .unwrap();
        assert_eq!(conditions.len(), 2);
        assert_eq!(conditions[0].attribute, "department");
        assert_eq!(conditions[0].values, vec!["정비팀", "야간조"]);

        assert!(
            validate_policy_conditions(&[PolicyConditionResponse {
                condition_key: "dept_scope".to_owned(),
                attribute: "machinery".to_owned(),
                operator: "equals".to_owned(),
                values: vec!["굴삭기".to_owned()],
            },])
            .is_err()
        );

        assert!(
            validate_policy_conditions(&[
                PolicyConditionResponse {
                    condition_key: "dept_scope".to_owned(),
                    attribute: "department".to_owned(),
                    operator: "equals".to_owned(),
                    values: vec!["정비팀".to_owned()],
                },
                PolicyConditionResponse {
                    condition_key: "dept_scope".to_owned(),
                    attribute: "team".to_owned(),
                    operator: "equals".to_owned(),
                    values: vec!["A".to_owned()],
                },
            ])
            .is_err()
        );
    }
}

// ---------------------------------------------------------------------------
// User handlers
// ---------------------------------------------------------------------------

async fn create_user(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateUserRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let roles = parse_roles(&body.roles)?;
    let elevated_role_changes = elevated_roles_in(&roles);
    authorize_user_write(&principal, &elevated_role_changes, &[], &body.branch_ids)?;

    let summary = state
        .store
        .create_user(CreateUserCommand {
            actor: principal.user_id,
            display_name: body.display_name,
            employee_id: body.employee_id,
            phone: body.phone,
            team: body.team,
            roles: role_db_strings(&roles),
            branch_ids: body.branch_ids,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(summary)))
}

async fn list_users(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Query(query): Query<ListUsersRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage(&principal, Feature::UserManage)?;
    if query.offset.is_some_and(|offset| offset < 0) {
        return Err(RestError::from_kernel(KernelError::validation(
            "offset must be non-negative",
        )));
    }
    let page = state
        .store
        .list_users(
            &principal.branch_scope,
            UserListQuery {
                include_inactive: query.include_inactive,
                limit: query.limit,
                offset: query.offset,
            },
        )
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page))
}

async fn list_directory_people(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Query(request): Query<DirectoryListRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let scope = directory_read_scope(&principal)?;
    let page = state
        .store
        .list_directory_people(&scope, request.into_query()?)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(DirectoryPage::from(page)))
}

async fn list_visible_directory_people(
    state: &IdentityRestState,
    headers: &HeaderMap,
) -> Result<Vec<VisibleDirectoryPerson>, RestError> {
    let principal = principal_from_headers(state, headers).await?;
    let scope = directory_read_scope(&principal)?;
    let org = principal.org_id;
    let page = console_platform_request_context::scope_org(org, async {
        state
            .store
            .list_directory_people(
                &scope,
                DirectoryListQuery {
                    search: None,
                    team: None,
                    branch_id: None,
                    include_inactive: false,
                    limit: Some(100),
                    offset: Some(0),
                },
            )
            .await
    })
    .await
    .map_err(RestError::from_store)?;
    Ok(page
        .items
        .into_iter()
        .map(VisibleDirectoryPerson::from)
        .collect())
}

async fn get_user(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage(&principal, Feature::UserManage)?;
    let summary = state
        .store
        .get_user(UserId::from_uuid(id), &principal.branch_scope)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

async fn update_user(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<UpdateUserRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let target_id = UserId::from_uuid(id);

    // Baseline: caller may manage users in their scope, and may only see/touch a
    // target within that scope (prevents cross-branch enumeration / IDOR).
    authorize_org_manage(&principal, Feature::UserManage)?;
    let target = state
        .store
        .get_user(target_id, &principal.branch_scope)
        .await
        .map_err(RestError::from_store)?;

    // Role/branch escalation guard: the *new* role set and the union of current
    // + requested branch sets must both be within the caller's authority. Run
    // whenever EITHER is being changed — a request that sets only `branch_ids`
    // (roles absent) must still prove branch authority over every branch the
    // user is being added to OR removed from, or a branch-scoped admin could
    // move a visible user into (or out of) branches they do not control
    // (console-0lj).
    let roles = match &body.roles {
        Some(raw) => Some(parse_roles(raw)?),
        None => None,
    };
    let mut preview_receipt_id = None;
    if roles.is_some() || body.branch_ids.is_some() {
        let elevated_role_changes =
            elevated_role_membership_changes(&target.roles, roles.as_ref())?;
        let target_branches = body
            .branch_ids
            .as_deref()
            .unwrap_or(target.branch_ids.as_slice());
        authorize_user_write(
            &principal,
            &elevated_role_changes,
            target.branch_ids.as_slice(),
            target_branches,
        )?;

        // Only an actual assignment change needs an impact-preview receipt. A
        // profile-only edit (e.g. phone) that re-sends the user's existing roles
        // and branches is a governance no-op and must not demand a preview; any
        // real role/branch change still requires it. (The escalation guard above
        // always runs.)
        let roles_changed = roles
            .as_ref()
            .is_some_and(|r| system_roles_changed(r, &target.roles));
        let branches_changed = body.branch_ids.as_ref().is_some_and(|requested| {
            let normalize = |branches: &[BranchId]| {
                let mut out = branches.to_vec();
                out.sort();
                out.dedup();
                out
            };
            normalize(requested) != normalize(&target.branch_ids)
        });
        if roles_changed || branches_changed {
            ensure_assignment_preview_acknowledged(&principal, body.preview_acknowledged)?;
            preview_receipt_id = Some(require_assignment_preview_receipt(
                &principal,
                body.preview_receipt_id,
            )?);
        }
    }

    let summary = state
        .store
        .update_user(UpdateUserCommand {
            actor: principal.user_id,
            user_id: target_id,
            display_name: body.display_name,
            employee_id: body.employee_id,
            phone: body.phone,
            team: body.team,
            roles: roles.as_ref().map(role_db_strings),
            branch_ids: body.branch_ids,
            preview_receipt_id,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

async fn deactivate_user(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let target_id = UserId::from_uuid(id);
    if target_id == principal.user_id {
        return Err(RestError::bad_request("cannot deactivate your own account"));
    }
    authorize_org_manage(&principal, Feature::UserManage)?;
    // Target must be within the caller's scope (IDOR).
    state
        .store
        .get_user(target_id, &principal.branch_scope)
        .await
        .map_err(RestError::from_store)?;
    let summary = state
        .store
        .deactivate_user(DeactivateUserCommand {
            actor: principal.user_id,
            user_id: target_id,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

async fn activate_user(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let target_id = UserId::from_uuid(id);
    if target_id == principal.user_id {
        return Err(RestError::bad_request("cannot activate your own account"));
    }
    authorize_org_manage(&principal, Feature::UserManage)?;
    // Target must be within the caller's scope (IDOR), including archived users.
    state
        .store
        .get_user(target_id, &principal.branch_scope)
        .await
        .map_err(RestError::from_store)?;
    let summary = state
        .store
        .activate_user(ActivateUserCommand {
            actor: principal.user_id,
            user_id: target_id,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

// ---------------------------------------------------------------------------
// New-console rollout handlers (per-org flag + per-user opt-in)
// ---------------------------------------------------------------------------

async fn get_console_rollout(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage(&principal, Feature::Login)?;
    let org = current_org()?;
    let user_id = principal.user_id;
    let status =
        with_org_conn::<_, ConsoleRolloutResponse, RestError>(state.pool(), org, move |tx| {
            Box::pin(async move { fetch_console_rollout_status_tx(tx, user_id).await })
        })
        .await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(status)))
}

async fn update_console_rollout_opt_in(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Json(body): Json<UpdateConsoleRolloutOptInRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage(&principal, Feature::Login)?;
    let org = current_org()?;
    let actor = principal.user_id;
    let opt_in = body.opt_in;
    let now = OffsetDateTime::now_utc();

    let status =
        with_audits::<_, ConsoleRolloutResponse, RestError>(state.pool(), org, move |tx| {
            Box::pin(async move {
                let before_preferences: Option<serde_json::Value> = sqlx::query_scalar(
                    r#"
                    SELECT preferences_json
                    FROM user_feature_preferences
                    WHERE user_id = $1 AND feature_key = $2
                    FOR UPDATE
                    "#,
                )
                .bind(*actor.as_uuid())
                .bind(CONSOLE_ROLLOUT_USER_FEATURE_KEY)
                .fetch_optional(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;
                let before_opt_in = before_preferences
                    .as_ref()
                    .and_then(|value| value.get("opt_in"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);

                let after_preferences = serde_json::json!({ "opt_in": opt_in });
                sqlx::query(
                    r#"
                    INSERT INTO user_feature_preferences (
                        org_id, user_id, feature_key, preferences_json, schema_version
                    ) VALUES ($1, $2, $3, $4, 1)
                    ON CONFLICT (org_id, user_id, feature_key) DO UPDATE SET
                        preferences_json = EXCLUDED.preferences_json,
                        schema_version = EXCLUDED.schema_version,
                        updated_at = now()
                    "#,
                )
                .bind(*org.as_uuid())
                .bind(*actor.as_uuid())
                .bind(CONSOLE_ROLLOUT_USER_FEATURE_KEY)
                .bind(after_preferences)
                .execute(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;

                let status = fetch_console_rollout_status_tx(tx, actor).await?;
                let action =
                    AuditAction::new("console.opt_in_update").map_err(RestError::from_kernel)?;
                let event = AuditEvent::new(
                    Some(actor),
                    action,
                    "user_feature_preference",
                    format!("{}:{CONSOLE_ROLLOUT_USER_FEATURE_KEY}", actor),
                    TraceContext::generate(),
                    now,
                )
                .with_org(org)
                .with_snapshots(
                    Some(serde_json::json!({
                        "feature_key": CONSOLE_ROLLOUT_USER_FEATURE_KEY,
                        "opt_in": before_opt_in,
                        "org_enabled": status.org_enabled,
                        "legacy_kill_switch_enabled": status.legacy_kill_switch_enabled,
                        "effective_new_console": status.org_enabled && before_opt_in && !status.legacy_kill_switch_enabled,
                    })),
                    Some(serde_json::json!({
                        "feature_key": CONSOLE_ROLLOUT_USER_FEATURE_KEY,
                        "opt_in": status.user_opted_in,
                        "org_enabled": status.org_enabled,
                        "effective_new_console": status.effective_new_console,
                    })),
                );
                Ok((status, vec![event]))
            })
        })
        .await?;

    Ok(Json(status))
}

async fn update_console_rollout_org_flag(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Json(body): Json<UpdateConsoleRolloutOrgFlagRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage(&principal, Feature::RoleManage)?;
    let org = current_org()?;
    let actor = principal.user_id;
    let enabled = body.enabled;
    let rollout_note = normalize_console_rollout_note(body.rollout_note)?;
    let now = OffsetDateTime::now_utc();

    let status =
        with_audits::<_, ConsoleRolloutResponse, RestError>(state.pool(), org, move |tx| {
            Box::pin(async move {
                let before_row = sqlx::query(
                    r#"
                    SELECT enabled, rollout_note
                    FROM org_runtime_flags
                    WHERE flag_key = $1
                    FOR UPDATE
                    "#,
                )
                .bind(CONSOLE_ROLLOUT_FLAG_KEY)
                .fetch_optional(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;
                let before_snapshot = match before_row {
                    Some(row) => Some(serde_json::json!({
                        "flag_key": CONSOLE_ROLLOUT_FLAG_KEY,
                        "enabled": row.try_get::<bool, _>("enabled").map_err(DbError::Sqlx)?,
                        "rollout_note": row
                            .try_get::<Option<String>, _>("rollout_note")
                            .map_err(DbError::Sqlx)?,
                    })),
                    None => None,
                };

                sqlx::query(
                    r#"
                    INSERT INTO org_runtime_flags (org_id, flag_key, enabled, rollout_note, set_by)
                    VALUES ($1, $2, $3, $4, $5)
                    ON CONFLICT (org_id, flag_key) DO UPDATE SET
                        enabled = EXCLUDED.enabled,
                        rollout_note = EXCLUDED.rollout_note,
                        set_by = EXCLUDED.set_by
                    "#,
                )
                .bind(*org.as_uuid())
                .bind(CONSOLE_ROLLOUT_FLAG_KEY)
                .bind(enabled)
                .bind(rollout_note.as_deref())
                .bind(*actor.as_uuid())
                .execute(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;

                let status = fetch_console_rollout_status_tx(tx, actor).await?;
                let action =
                    AuditAction::new("console.org_flag_update").map_err(RestError::from_kernel)?;
                let event = AuditEvent::new(
                    Some(actor),
                    action,
                    "org_runtime_flag",
                    CONSOLE_ROLLOUT_FLAG_KEY.to_owned(),
                    TraceContext::generate(),
                    now,
                )
                .with_org(org)
                .with_snapshots(
                    before_snapshot,
                    Some(serde_json::json!({
                        "flag_key": CONSOLE_ROLLOUT_FLAG_KEY,
                        "enabled": enabled,
                        "rollout_note": rollout_note,
                    })),
                );
                Ok((status, vec![event]))
            })
        })
        .await?;

    Ok(Json(status))
}

async fn update_console_legacy_kill_switch(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Json(body): Json<UpdateConsoleLegacyKillSwitchRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage(&principal, Feature::RoleManage)?;
    let org = current_org()?;
    let actor = principal.user_id;
    let enabled = body.enabled;
    let reason = normalize_console_rollout_note(body.reason)?;
    let now = OffsetDateTime::now_utc();

    let status = with_audits::<_, ConsoleRolloutResponse, RestError>(
        state.pool(),
        org,
        move |tx| {
            Box::pin(async move {
                let before_row = sqlx::query(
                    r#"
                    SELECT enabled, rollout_note
                    FROM org_runtime_flags
                    WHERE flag_key = $1
                    FOR UPDATE
                    "#,
                )
                .bind(CONSOLE_LEGACY_KILL_SWITCH_FLAG_KEY)
                .fetch_optional(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;
                let before_snapshot = match before_row {
                    Some(row) => Some(serde_json::json!({
                        "flag_key": CONSOLE_LEGACY_KILL_SWITCH_FLAG_KEY,
                        "enabled": row.try_get::<bool, _>("enabled").map_err(DbError::Sqlx)?,
                        "reason": row
                            .try_get::<Option<String>, _>("rollout_note")
                            .map_err(DbError::Sqlx)?,
                    })),
                    None => None,
                };

                sqlx::query(
                    r#"
                    INSERT INTO org_runtime_flags (org_id, flag_key, enabled, rollout_note, set_by)
                    VALUES ($1, $2, $3, $4, $5)
                    ON CONFLICT (org_id, flag_key) DO UPDATE SET
                        enabled = EXCLUDED.enabled,
                        rollout_note = EXCLUDED.rollout_note,
                        set_by = EXCLUDED.set_by
                    "#,
                )
                .bind(*org.as_uuid())
                .bind(CONSOLE_LEGACY_KILL_SWITCH_FLAG_KEY)
                .bind(enabled)
                .bind(reason.as_deref())
                .bind(*actor.as_uuid())
                .execute(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;

                let status = fetch_console_rollout_status_tx(tx, actor).await?;
                let action = AuditAction::new("console.kill_switch")
                    .map_err(RestError::from_kernel)?;
                let event = AuditEvent::new(
                    Some(actor),
                    action,
                    "org_runtime_flag",
                    CONSOLE_LEGACY_KILL_SWITCH_FLAG_KEY,
                    TraceContext::generate(),
                    now,
                )
                .with_org(org)
                .with_snapshots(
                    before_snapshot,
                    Some(serde_json::json!({
                        "flag_key": CONSOLE_LEGACY_KILL_SWITCH_FLAG_KEY,
                        "enabled": enabled,
                        "reason": reason,
                        "org_rollout_enabled": status.org_rollout_enabled,
                        "user_opted_in": status.user_opted_in,
                        "effective_route": status.effective_route,
                        "effective_route_for_opted_in_user": status.effective_route_for_opted_in_user,
                        "effective_route_for_opted_out_user": status.effective_route_for_opted_out_user,
                        "overrides_individual_toggles": status.overrides_individual_toggles,
                    })),
                );
                Ok((status, vec![event]))
            })
        },
    )
    .await?;

    if enabled {
        tracing::warn!(
            event = "console_legacy_kill_switch_updated",
            actor_user_id = %actor,
            org_id = %org,
            enabled,
            "console legacy kill switch enabled"
        );
    } else {
        tracing::info!(
            event = "console_legacy_kill_switch_updated",
            actor_user_id = %actor,
            org_id = %org,
            enabled,
            "console legacy kill switch disabled"
        );
    }

    Ok(Json(status))
}

async fn fetch_console_rollout_status_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: UserId,
) -> Result<ConsoleRolloutResponse, RestError> {
    let org_enabled: bool = sqlx::query_scalar("SELECT org_runtime_flag_enabled($1)")
        .bind(CONSOLE_ROLLOUT_FLAG_KEY)
        .fetch_one(tx.as_mut())
        .await
        .map_err(DbError::Sqlx)?;
    let legacy_kill_switch_enabled: bool =
        sqlx::query_scalar("SELECT org_runtime_flag_enabled($1)")
            .bind(CONSOLE_LEGACY_KILL_SWITCH_FLAG_KEY)
            .fetch_one(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
    let preferences: Option<serde_json::Value> = sqlx::query_scalar(
        r#"
        SELECT preferences_json
        FROM user_feature_preferences
        WHERE user_id = $1 AND feature_key = $2
        "#,
    )
    .bind(*user_id.as_uuid())
    .bind(CONSOLE_ROLLOUT_USER_FEATURE_KEY)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(DbError::Sqlx)?;
    let user_opted_in = preferences
        .as_ref()
        .and_then(|value| value.get("opt_in"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let effective_new_console = org_enabled && user_opted_in && !legacy_kill_switch_enabled;
    let effective_route_for_opted_in_user = if org_enabled && !legacy_kill_switch_enabled {
        "new_console"
    } else {
        "legacy"
    };
    Ok(ConsoleRolloutResponse {
        flag_key: CONSOLE_ROLLOUT_FLAG_KEY,
        org_enabled,
        org_rollout_enabled: org_enabled,
        user_opted_in,
        legacy_kill_switch_enabled,
        kill_switch_active: legacy_kill_switch_enabled,
        effective_new_console,
        effective_route: if effective_new_console {
            "new_console"
        } else {
            "legacy"
        },
        effective_route_for_opted_in_user,
        effective_route_for_opted_out_user: "legacy",
        overrides_individual_toggles: legacy_kill_switch_enabled,
    })
}

fn normalize_console_rollout_note(raw: Option<String>) -> Result<Option<String>, RestError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let note = raw.trim().to_owned();
    if note.is_empty() || note.chars().count() > 500 {
        return Err(RestError::validation(
            "rollout_note must be between 1 and 500 characters when provided",
        ));
    }
    Ok(Some(note))
}

// ---------------------------------------------------------------------------
// Self-profile handlers (any authenticated user)
// ---------------------------------------------------------------------------

async fn get_me(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    // A user can always read their own record regardless of branch scope.
    let summary = state
        .store
        .get_user(principal.user_id, &BranchScope::All)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

async fn update_me(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Json(body): Json<UpdateSelfRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let summary = state
        .store
        .update_self_profile(UpdateSelfProfileCommand {
            user_id: principal.user_id,
            display_name: body.display_name,
            phone: body.phone,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(summary))
}

// ---------------------------------------------------------------------------
// Authorization projection: GET /api/v1/me/authz (charter G-a)
// ---------------------------------------------------------------------------

/// The caller's authorization projection. NON-AUTHORITATIVE: `authority` is
/// always `advisory_ui_only` (mirroring `web/src/auth/policyProjection.ts`) —
/// the backend matrix (`authorize`) remains the sole enforcer; this only lets
/// the console render by grant instead of by hardcoded role lists (the class of
/// drift behind the ADMIN /overview 403 bug). `source = legacy_matrix` today;
/// the Cedar enforce-flip later flips it to `cedar` with this shape unchanged.
#[derive(Debug, Serialize)]
struct MeAuthzResponse {
    authority: &'static str,
    source: &'static str,
    user_id: Uuid,
    org_id: Uuid,
    /// Roles as principal attributes (canonical role keys).
    roles: Vec<String>,
    branch_scope: BranchScope,
    /// Capability grants the caller holds — deny-by-omission: a feature the
    /// caller cannot use at all is OMITTED, so `capabilities` is exactly the
    /// grant set the frontend checks against.
    capabilities: Vec<MeAuthzCapability>,
}

#[derive(Debug, Serialize)]
struct MeAuthzCapability {
    /// `Feature::as_str` snake_case key (matches `/api/v1/policy/features`).
    feature: String,
    /// `deny` is never emitted (omitted instead); one of `request_only`,
    /// `limited`, `allow`.
    permission: String,
    /// The branch subset this `permission` level actually holds over — NOT
    /// necessarily the caller's full `branch_scope`. A branch-narrowed custom
    /// grant (`EffectiveFeatureGrant::branch_scope`, already intersected with
    /// the caller's live scope by `resolve_effective_feature_grants_in_org`)
    /// only elevates the capability within its own branches; collapsing to a
    /// single scalar permission without this would over-promise the grant to
    /// every branch the caller can act in, an affordance the real `authorize`
    /// call would then 403 outside this scope — the exact class of drift this
    /// endpoint exists to fix. The UI must intersect this with its target
    /// branch before offering the affordance.
    branch_scope: BranchScope,
}

fn permission_rank(level: PermissionLevel) -> u8 {
    match level {
        PermissionLevel::Deny => 0,
        PermissionLevel::RequestOnly => 1,
        PermissionLevel::Limited => 2,
        PermissionLevel::Allow => 3,
    }
}

fn branch_scope_union(a: &BranchScope, b: &BranchScope) -> BranchScope {
    match (a, b) {
        (BranchScope::All, _) | (_, BranchScope::All) => BranchScope::All,
        (BranchScope::Branches(x), BranchScope::Branches(y)) => {
            BranchScope::Branches(x.union(y).copied().collect())
        }
    }
}

/// The caller's effective permission for one feature, paired with the branch
/// subset it holds over — exactly the `OR` the enforcer (`authorize`)
/// evaluates, projected to a single (level, scope) pair. Built-in roles hold
/// uniformly across the caller's full `branch_scope` (`authorize` never
/// branch-narrows a role permission beyond the principal's own scope). A
/// custom grant only elevates the capability within `grant.branch_scope`
/// (already the live-scope-intersected effective set) — a lower-ranked grant
/// never widens the current best scope, and an equal-ranked grant unions in
/// (there can be more than one branch-narrowed grant for the same feature).
fn feature_capability(
    principal: &Principal,
    feature: Feature,
) -> Option<(PermissionLevel, BranchScope)> {
    let role_level = principal
        .roles
        .iter()
        .map(|role| permission_for(*role, feature))
        .max_by_key(|level| permission_rank(*level))
        .unwrap_or(PermissionLevel::Deny);
    let mut best = role_level;
    let mut best_scope = principal.branch_scope.clone();

    for grant in principal
        .effective_feature_grants
        .iter()
        .filter(|grant| grant.feature == feature)
    {
        match permission_rank(grant.permission).cmp(&permission_rank(best)) {
            std::cmp::Ordering::Greater => {
                best = grant.permission;
                best_scope = grant.branch_scope.clone();
            }
            std::cmp::Ordering::Equal => {
                best_scope = branch_scope_union(&best_scope, &grant.branch_scope);
            }
            std::cmp::Ordering::Less => {}
        }
    }

    (best != PermissionLevel::Deny).then_some((best, best_scope))
}

fn me_authz_projection(principal: &Principal) -> MeAuthzResponse {
    let capabilities = Feature::ALL
        .into_iter()
        .filter_map(|feature| {
            let (level, scope) = feature_capability(principal, feature)?;
            Some(MeAuthzCapability {
                feature: feature.as_str().to_owned(),
                permission: level.as_str().to_owned(),
                branch_scope: scope,
            })
        })
        .collect();
    MeAuthzResponse {
        authority: "advisory_ui_only",
        source: "legacy_matrix",
        user_id: *principal.user_id.as_uuid(),
        org_id: *principal.org_id.as_uuid(),
        roles: principal
            .roles
            .iter()
            .map(|role| role.as_str().to_owned())
            .collect(),
        branch_scope: principal.branch_scope.clone(),
        capabilities,
    }
}

async fn get_me_authz(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, RestError> {
    // The resolved Principal already carries the live branch scope + runtime
    // custom-role grants (armed under this request's org), so the projection is
    // pure — no DB read beyond principal resolution.
    let principal = principal_from_headers(&state, &headers).await?;
    Ok(Json(me_authz_projection(&principal)))
}

// ---------------------------------------------------------------------------
// Console workspace layout handlers (any authenticated user; principal's row only)
// ---------------------------------------------------------------------------

/// Response for `GET /api/v1/me/workspace`. `layout` is an opaque, frontend-owned
/// JSON object (the console window/panel arrangement); the empty default is `{}`.
#[derive(Debug, Serialize)]
struct WorkspaceResponse {
    layout: serde_json::Value,
}

/// Body for `PUT /api/v1/me/workspace`. The `layout` is stored verbatim; the DB
/// enforces it is a JSON object within the size cap.
#[derive(Debug, Deserialize)]
struct WorkspaceUpsertRequest {
    layout: serde_json::Value,
}

async fn get_workspace(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let layout = state
        .store
        .get_workspace_layout(principal.user_id)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(WorkspaceResponse { layout }))
}

async fn put_workspace(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Json(body): Json<WorkspaceUpsertRequest>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    // Reject non-object payloads at the boundary with a clear 422 rather than
    // letting the DB CHECK surface as a generic 500. (The DB CHECK remains the
    // final backstop for size and shape.)
    if !body.layout.is_object() {
        return Err(RestError::validation("layout must be a JSON object"));
    }
    // Size cap at the boundary is the REAL enforcement: the DB CHECK uses
    // pg_column_size(), which measures the TOAST-COMPRESSED jsonb size and so
    // rejects far fewer payloads than a raw byte count. Guarding the serialized
    // length here returns a clean 422 (never a DB-CHECK 500); the CHECK
    // (pg_column_size <= 64KiB, migration 0098) is only a defense-in-depth backstop.
    if serde_json::to_vec(&body.layout).map_or(usize::MAX, |bytes| bytes.len()) > 64 * 1024 {
        return Err(RestError::validation("layout exceeds the maximum size"));
    }
    let layout = state
        .store
        .put_workspace_layout(
            principal.user_id,
            body.layout,
            TraceContext::generate(),
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(WorkspaceResponse { layout }))
}

// ---------------------------------------------------------------------------
// Region handlers
// ---------------------------------------------------------------------------

async fn list_regions(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    // Any authenticated user may read the org tree (needed to populate pickers).
    authorize_org_manage(&principal, Feature::Login)?;
    let regions = state
        .store
        .list_regions()
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(regions))
}

// ---------------------------------------------------------------------------
// Branch handlers
// ---------------------------------------------------------------------------

async fn list_branches(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    // Any authenticated user may read the branch list (org pickers + support
    // triage). Branch-scoped data on the branches themselves is not sensitive.
    authorize_org_manage(&principal, Feature::Login)?;
    let branches = state
        .store
        .list_branches()
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(branches))
}

// Get-one for an org-unit (branch) pin panel (UI-M2a). Same non-sensitive read
// gate as the list; org-RLS scopes it to the caller's org. No audit — this is
// org-structure metadata, not PII. It filters the branch list rather than
// adding a get-one store method because branch counts are small.
async fn get_branch(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage(&principal, Feature::Login)?;
    let branch = state
        .store
        .list_branches()
        .await
        .map_err(RestError::from_store)?
        .into_iter()
        .find(|b| b.id.as_uuid() == &id)
        .ok_or_else(|| RestError::from_kernel(KernelError::not_found("branch was not found")))?;
    Ok(Json(branch))
}

// ---------------------------------------------------------------------------
// Passkey self-management handlers (any authenticated user, OWN credentials)
// ---------------------------------------------------------------------------

/// List the AUTHENTICATED user's OWN passkey credentials.
///
/// Scoped to BOTH the caller (`principal.user_id`) AND the request's tenant: the
/// read runs inside `with_org_conn(.., current_org()?, ..)`, which arms the
/// `app.current_org` GUC so the FORCE-RLS `auth_webauthn_credentials` rows for
/// this org become visible to the non-owner `console_rt` role. The `WHERE user_id`
/// filter then narrows to the caller's own credentials. No secret material
/// (passkey_json / public key / credential_id) ever leaves this handler.
async fn list_passkeys(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    // Self-service surface: every authenticated user may manage its own passkeys.
    authorize_org_manage(&principal, Feature::Login)?;
    let org = current_org()?;
    let user_id = *principal.user_id.as_uuid();

    let summaries =
        with_org_conn::<_, Vec<PasskeySummary>, RestError>(state.pool(), org, move |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    r#"
                SELECT id, created_at, last_used_at
                FROM auth_webauthn_credentials
                WHERE user_id = $1
                ORDER BY created_at
                "#,
                )
                .bind(user_id)
                .fetch_all(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;

                rows.into_iter()
                    .map(|row| {
                        Ok(PasskeySummary {
                            id: row.try_get("id").map_err(DbError::Sqlx)?,
                            created_at: row.try_get("created_at").map_err(DbError::Sqlx)?,
                            last_used_at: row.try_get("last_used_at").map_err(DbError::Sqlx)?,
                        })
                    })
                    .collect::<Result<Vec<_>, RestError>>()
            })
        })
        .await?;

    Ok(Json(summaries))
}

/// Revoke ONE of the authenticated user's OWN passkey credentials.
///
/// IDOR guard: the DELETE is constrained to `id = $1 AND user_id = $2`, so a user
/// can never delete another user's credential even within the same org; a
/// credential that is not the caller's own matches zero rows and yields 404.
///
/// Lockout guard: refuses to delete the caller's LAST remaining passkey. A user
/// whose only login method is a single passkey would otherwise lock themselves
/// out; deleting it returns 409 with a clear message. (A fresh sign-in OTP can
/// only be minted by an admin, so the floor is enforced here rather than relying
/// on a self-service recovery path.)
///
/// The whole operation runs in ONE tenant-armed transaction via `with_audits`:
/// the count check, the ownership-scoped DELETE, and the audit row commit (or roll
/// back) atomically together.
async fn delete_passkey(
    State(state): State<IdentityRestState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_org_manage(&principal, Feature::Login)?;
    let org = current_org()?;
    let actor = principal.user_id;
    let user_id = *actor.as_uuid();
    let now = OffsetDateTime::now_utc();

    with_audits::<_, (), RestError>(state.pool(), org, move |tx| {
        Box::pin(async move {
            let event = console_platform_auth::delete_self_passkey_tx(tx, org, user_id, id, now)
                .await
                .map_err(|error| match error {
                    console_platform_auth::AuthError::Kernel(error) => {
                        RestError::from_kernel(error)
                    }
                    console_platform_auth::AuthError::InvalidStoredData(_) => {
                        RestError::unauthorized("inactive or missing Account")
                    }
                    _ => RestError::internal("passkey deletion failed"),
                })?;

            Ok(((), vec![event]))
        })
    })
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Authz helpers
// ---------------------------------------------------------------------------

/// Authorize an org-management feature as a CAPABILITY: may this caller manage
/// org structure at all.
///
/// This is not a claim that the managed rows have no branch identity. A branch
/// obviously has one — it IS one — and `update_branch`/`deactivate_branch` take a
/// branch id straight off the path. What this function replaced could not check
/// that id either: it authorized against
/// `representative_branch(&principal.branch_scope)`, a branch taken from the
/// CALLER, so `authorize`'s `branch_scope.allows(..)` was true by construction on
/// both arms and only the feature matrix ever decided. The swap to
/// [`authorize_capability`] is therefore verdict-identical — it makes the missing
/// branch dimension visible instead of faking it, and neither opens nor closes a
/// cross-branch hole.
///
/// Confining a branch mutation to the caller's own scope belongs where the TARGET
/// id is known — `PgOrgStore::update_branch` / `deactivate_branch`, comparing the
/// caller's scope against `command.branch_id`. As of this commit
/// `UpdateBranchCommand`/`DeactivateBranchCommand` carry no scope field and that
/// comparison does not exist yet; closing it is the identity lane's open work, and
/// it is the only thing that can stop a cross-branch edit. A representative-branch
/// call here never could, which is why removing it costs nothing. Reads already
/// carry their own scope resolution (`directory_read_scope`), as do elevated-role
/// writes (`authorize_user_write`).
///
/// NOT `authorize_org_wide`: `UserManage`/`BranchManage`/`RegionManage` are ALLOW
/// for built-in ADMIN, and `Login` is ALLOW for every role; org-wide routing would
/// deny every branch-scoped principal here.
fn authorize_org_manage(principal: &Principal, feature: Feature) -> Result<(), RestError> {
    authorize_capability(principal, Action::new(feature)).map_err(RestError::from_kernel)
}

/// Resolve the exact effective scope for a people-directory read. Built-in
/// grants hold across the caller's live membership scope; custom PBAC grants
/// contribute only their already-resolved scope, re-intersected here with live
/// membership as a defense-in-depth boundary. No allow produces no results.
fn directory_read_scope(principal: &Principal) -> Result<BranchScope, RestError> {
    let mut effective = BranchScope::Branches(BTreeSet::new());
    let role_allows = principal.roles.iter().any(|role| {
        permission_for(*role, Feature::EmployeeDirectoryRead) == PermissionLevel::Allow
    });
    if role_allows {
        effective = branch_scope_union(&effective, &principal.branch_scope);
    }
    for grant in principal.effective_feature_grants.iter().filter(|grant| {
        grant.feature == Feature::EmployeeDirectoryRead
            && grant.permission == PermissionLevel::Allow
    }) {
        let grant_scope = principal.branch_scope.intersect(&grant.branch_scope);
        effective = branch_scope_union(&effective, &grant_scope);
    }
    match &effective {
        BranchScope::All => Ok(effective),
        BranchScope::Branches(branches) if !branches.is_empty() => Ok(effective),
        BranchScope::Branches(_) => Err(RestError::forbidden("people directory access is denied")),
    }
}

#[cfg(test)]
mod directory_query_tests {
    use super::*;

    #[test]
    fn directory_scope_intersects_custom_grants_with_live_membership() {
        let member_branch = BranchId::new();
        let granted_branch = BranchId::new();
        let outside_branch = BranchId::new();
        let principal = Principal::new(
            UserId::new(),
            OrgId::knl(),
            BTreeSet::new(),
            BranchScope::Branches(BTreeSet::from([member_branch, granted_branch])),
        )
        .with_effective_feature_grants(vec![
            console_platform_authz::EffectiveFeatureGrant::new(
                Feature::EmployeeDirectoryRead,
                PermissionLevel::Allow,
                BranchScope::Branches(BTreeSet::from([granted_branch, outside_branch])),
            ),
        ]);

        assert_eq!(
            directory_read_scope(&principal).unwrap(),
            BranchScope::Branches(BTreeSet::from([granted_branch]))
        );
    }

    #[test]
    fn directory_scope_denies_principal_without_role_or_runtime_grant() {
        let principal = Principal::new(
            UserId::new(),
            OrgId::knl(),
            BTreeSet::from([Role::Member]),
            BranchScope::single(BranchId::new()),
        );

        let error = directory_read_scope(&principal).expect_err("no grant must fail closed");
        assert_eq!(error.status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn directory_scope_unions_only_live_membership_intersections() {
        let member_a = BranchId::new();
        let member_b = BranchId::new();
        let outside = BranchId::new();
        let principal = Principal::new(
            UserId::new(),
            OrgId::knl(),
            BTreeSet::from([Role::Member]),
            BranchScope::Branches(BTreeSet::from([member_a, member_b])),
        )
        .with_effective_feature_grants(vec![
            console_platform_authz::EffectiveFeatureGrant::new(
                Feature::EmployeeDirectoryRead,
                PermissionLevel::Allow,
                BranchScope::Branches(BTreeSet::from([member_a, outside])),
            ),
            console_platform_authz::EffectiveFeatureGrant::new(
                Feature::EmployeeDirectoryRead,
                PermissionLevel::Allow,
                BranchScope::Branches(BTreeSet::from([member_b, outside])),
            ),
        ]);

        assert_eq!(
            directory_read_scope(&principal).unwrap(),
            BranchScope::Branches(BTreeSet::from([member_a, member_b]))
        );
    }

    #[test]
    fn directory_request_rejects_malformed_filters() {
        let invalid_team = DirectoryListRequest {
            search: None,
            team: Some("unknown".to_owned()),
            branch_id: None,
            include_inactive: false,
            limit: None,
            offset: None,
        };
        assert_eq!(
            invalid_team.into_query().unwrap_err().status,
            StatusCode::UNPROCESSABLE_ENTITY
        );

        let invalid_offset = DirectoryListRequest {
            search: None,
            team: None,
            branch_id: None,
            include_inactive: false,
            limit: Some(10),
            offset: Some(-1),
        };
        assert_eq!(
            invalid_offset.into_query().unwrap_err().status,
            StatusCode::UNPROCESSABLE_ENTITY
        );

        let oversized_limit = DirectoryListRequest {
            search: None,
            team: None,
            branch_id: None,
            include_inactive: false,
            limit: Some(MAX_DIRECTORY_PAGE_LIMIT + 1),
            offset: None,
        };
        assert_eq!(
            oversized_limit.into_query().unwrap_err().status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[test]
    fn directory_person_json_omits_phone_while_user_summary_keeps_it() {
        let stored_phone = "010-1234-5678";
        let user = UserSummary {
            id: UserId::new(),
            display_name: "directory subject".to_owned(),
            employee_id: None,
            employee_name: None,
            employee_number: None,
            employee_company: None,
            employee_org_unit: None,
            employee_position: None,
            employee_identity_review_required: None,
            employee_identity_resolution_confidence: None,
            employee_link_status: EmployeeLinkStatus::Unlinked,
            phone: Some(stored_phone.to_owned()),
            team: Some(Team::Maintenance),
            roles: vec!["MECHANIC".to_owned()],
            branch_ids: vec![BranchId::new()],
            is_active: true,
            has_passkey: true,
            account_status: AccountStatus::Active,
            created_at: OffsetDateTime::now_utc(),
        };
        let user_json = serde_json::to_value(&user).expect("user summary json");
        assert_eq!(user_json["phone"], stored_phone);

        let directory_json = serde_json::to_value(DirectoryPage::from(UserPage {
            items: vec![user],
            limit: 50,
            offset: 0,
            total: 1,
        }))
        .expect("directory page json");
        let item = &directory_json["items"][0];
        let keys = item
            .as_object()
            .expect("directory item must be an object")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let allowed = [
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
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        assert_eq!(
            keys, allowed,
            "directory item JSON keys must match the DirectoryPerson allowlist"
        );
        for forbidden in ["phone", "salary", "bank_account", "rrn", "won"] {
            assert!(
                item.get(forbidden).is_none(),
                "directory item must omit {forbidden}; keys={keys:?}"
            );
        }
        assert_eq!(item["display_name"], "directory subject");
    }

    #[test]
    fn identity_face_composes_directory_schemas() {
        let composed = console_contracts::compose(&[&OPENAPI_FRAGMENT]).expect("identity fragment");
        assert!(
            composed.contains("    DirectoryPerson:\n"),
            "composed fragment missing DirectoryPerson"
        );
        assert!(
            composed.contains("    DirectoryPage:\n"),
            "composed fragment missing DirectoryPage"
        );

        let directory_person = include_str!("../openapi/schemas/DirectoryPerson.yaml");
        let property_names = yaml_map_keys(directory_person, "properties:");
        assert!(
            !property_names.contains(&"phone"),
            "DirectoryPerson OpenAPI must not include phone; properties={property_names:?}"
        );
    }

    fn yaml_map_keys<'a>(schema: &'a str, header: &str) -> Vec<&'a str> {
        let Some((_, body)) = schema.split_once(header) else {
            return Vec::new();
        };
        body.lines()
            .filter_map(|line| {
                let name = line.strip_prefix("  ")?;
                if name.starts_with(' ') || name.starts_with('#') {
                    return None;
                }
                name.strip_suffix(':')
            })
            .collect()
    }
}

/// Per-tenant DARK switch for the Cedar/PBAC role_manage shadow lane
/// (`org_runtime_flags`, migration 0095). Ships with ZERO enabled rows: an absent
/// row resolves FALSE via `org_runtime_flag_enabled()`, so the shadow lane never
/// runs and production authorization is byte-for-byte unchanged.
/// Exposed (doc-hidden) only so the DB-backed shadow-wiring integration tests can
/// live under `backend/app/tests/` (out of the `rest/` path the audit-coverage gate
/// scans). Not part of the crate's supported API.
#[doc(hidden)]
pub const CEDAR_PBAC_SHADOW_ROLE_MANAGE_FLAG: &str = "cedar_pbac_shadow_role_manage";
const CEDAR_PBAC_SHADOW_TOTAL: &str = "cedar_pbac_shadow_total";
const CEDAR_PBAC_SHADOW_DISAGREEMENT_TOTAL: &str = "cedar_pbac_shadow_disagreement_total";
/// Append-only audit action for a shadow observation (kernel `AuditAction` shape:
/// ≥2 dot-separated `[a-z0-9_]` segments).
///
/// Exposed (doc-hidden) only for the moved integration tests (see above).
#[doc(hidden)]
pub const CEDAR_PBAC_SHADOW_AUDIT_ACTION: &str = "authz.cedar_pbac_shadow";

/// Authorize an org-management feature and — for [`Feature::RoleManage`] ONLY,
/// and only when the per-tenant dark flag is enabled — run an AUDIT-ONLY Cedar/PBAC
/// shadow observation alongside it.
///
/// SAFETY (ADR-0021, HIGH finding): the legacy [`authorize_org_manage`] `Result`
/// is the SOLE enforcer and is what this returns, ALWAYS. The Cedar shadow lane is
/// a best-effort, side-effect-only observation whose error/deny/bug is swallowed
/// and can NEVER change the returned decision. In particular the coexistence
/// boundary's `CedarShadowLegacyEnforce` arm short-circuits to Cedar's deny BEFORE
/// consulting legacy, so its returned effect is deliberately NOT used to gate the
/// request — it is recorded for audit/metrics only.
#[doc(hidden)]
pub async fn authorize_org_manage_observed(
    state: &IdentityRestState,
    principal: &Principal,
    feature: Feature,
) -> Result<(), RestError> {
    // The sole enforcer. Computed first; its value is returned unchanged below no
    // matter what the shadow lane observes.
    let legacy = authorize_org_manage(principal, feature);

    if feature == Feature::RoleManage {
        let legacy_effect = if legacy.is_ok() {
            DecisionEffect::Allow
        } else {
            DecisionEffect::Deny
        };
        // Audit-only. Never propagates an error or mutates `legacy`.
        // ponytail: the shadow observation is wrapped in `catch_unwind`, so any panic
        // ANYWHERE in the lane becomes an `Err` and can NEVER unwind into this live
        // request. It runs on the same task (task-local `CURRENT_ORG` preserved) and is
        // awaited inline so the deterministic DB tests observe it synchronously.
        run_role_manage_cedar_shadow(state, principal, legacy_effect).await;
    }

    legacy
}

/// Best-effort Cedar/PBAC shadow observation for a role_manage request. Swallows
/// (and logs) EVERY error: building/evaluating/persisting the shadow can never
/// fail the live request — the legacy decision already stands.
async fn run_role_manage_cedar_shadow(
    state: &IdentityRestState,
    principal: &Principal,
    legacy_effect: DecisionEffect,
) {
    use futures::FutureExt;

    let actor = principal.user_id; // UserId is Copy; used for logging in both arms
    // Panic-isolate the ENTIRE shadow future: `catch_unwind` turns any panic (Cedar
    // SDK, metrics recorder, serialize — anywhere in the lane) into an `Err` instead
    // of letting it unwind into the live request. It runs on the SAME task, so the
    // request's armed `CURRENT_ORG` task-local is preserved and the lane resolves the
    // right tenant. `AssertUnwindSafe` is sound here: the lane holds no locks and its
    // only mutation is an audit-txn write that rolls back on a panic, so a caught
    // panic leaves no observable broken state. The enforced decision (legacy) is
    // already computed and returned by the caller regardless of anything here.
    let outcome = std::panic::AssertUnwindSafe(try_run_role_manage_cedar_shadow(
        state,
        principal,
        legacy_effect,
    ))
    .catch_unwind()
    .await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(err)) => tracing::warn!(
            event = "cedar_pbac_shadow_error",
            error = %err.message,
            actor_user_id = %actor,
            "cedar/pbac role_manage shadow lane failed (audit-only; live decision unaffected)"
        ),
        Err(_panic) => tracing::warn!(
            event = "cedar_pbac_shadow_error",
            actor_user_id = %actor,
            "cedar/pbac role_manage shadow lane panicked (audit-only; live decision unaffected)"
        ),
    }
}

async fn try_run_role_manage_cedar_shadow(
    state: &IdentityRestState,
    principal: &Principal,
    legacy_effect: DecisionEffect,
) -> Result<(), RestError> {
    // DARK switch: absent/false flag ⇒ do nothing at all (no reads beyond this
    // one, no bundle, no metrics, no audit). Production stays byte-identical.
    if !state
        .store
        .org_runtime_flag_enabled(CEDAR_PBAC_SHADOW_ROLE_MANAGE_FLAG)
        .await
        .map_err(RestError::from_store)?
    {
        return Ok(());
    }

    // Coexistence-map identity for role_manage. The committed map stays
    // legacy_only (slice 3); the shadow lane derives a shadow-mode entry from it,
    // bound to a real per-org compiled bundle, so it exercises the
    // `CedarShadowLegacyEnforce` boundary arm WITHOUT flipping the committed map.
    let load = canonical_coexistence_map().map_err(RestError::from_kernel)?;
    let Some(base) = load
        .entries
        .iter()
        .find(|entry| entry.feature == Feature::RoleManage)
    else {
        return Ok(()); // role_manage not enrolled ⇒ nothing to observe.
    };

    // The tenant this request's DB context (the `app.current_org` GUC) is armed to,
    // so the compiled bundle, resource scope, RLS proof, and persisted audit all
    // reflect the REAL armed scope rather than a claim off the principal. In the
    // normal request flow `current_org()` equals `principal.org_id`, so this is not a
    // behavior change — it removes a latent inconsistency and makes the audit
    // evidence trustworthy. (`current_org()?` maps a missing GUC to a 500 via the
    // existing `From<RequestContextError> for RestError`; `current_org` returns a
    // `RequestContextError`, not a `KernelError`.)
    let org = current_org()?;

    // DB-current freshness (guard-time) the token snapshot must be at least as
    // fresh as. Read under the armed `console_rt` GUC via the store.
    let policy_version = state
        .store
        .get_policy_version()
        .await
        .map_err(RestError::from_store)?
        .version;
    let (subject_version, session_generation) = state
        .store
        .get_subject_authz_versions(principal.user_id)
        .await
        .map_err(RestError::from_store)?;

    // Per-org, strict-validated compiled bundle keyed on this policy_version.
    // ponytail: recompiled per shadow eval; add the in-process bundle cache
    // (ADR-0021 §4) when the pilot widens past one org.
    let bundle = engine::compile_bundle(org, u64::try_from(policy_version).unwrap_or(0))
        .map_err(RestError::from_kernel)?;

    let shadow_entry = CoexistenceMapEntry::new(
        base.id.clone(),
        base.domain.clone(),
        base.feature,
        base.resource_type.clone(),
        DualEngineMode::CedarShadowLegacyEnforce,
        Some(bundle.key.clone()),
    );

    let request = AuthorizationRequest::new(
        principal.clone(),
        Action::new(Feature::RoleManage),
        AuthorizationResource::org_wide(org, base.resource_type.clone()),
    )
    .with_policy_domain(base.domain.clone())
    .with_subject_freshness(principal.authz_freshness)
    .requiring_freshness(SubjectFreshnessRequirement {
        min_policy_version: u64::try_from(policy_version).unwrap_or(0),
        min_subject_version: u64::try_from(subject_version).unwrap_or(0),
        min_session_generation: u64::try_from(session_generation).unwrap_or(0),
        required_step_up_generation: None,
    })
    .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(org));

    // Real Cedar evaluation (Result + catch_unwind guarded — cannot throw).
    let cedar = engine::evaluate(&request, &bundle);
    // AUDIT-ONLY boundary observation. Its effect is NOT used to gate the request.
    let observed = evaluate_cedar_pbac_boundary(&request, Some(&shadow_entry), cedar.clone());
    let audit = observe_cedar_pbac_decision(&request, Some(&shadow_entry), Some(&cedar), observed);

    emit_cedar_shadow_metrics(&audit, legacy_effect);
    persist_cedar_shadow_audit(state.pool(), org, principal.user_id, &audit).await;

    Ok(())
}

/// Render a serde-serialized authorization enum (snake_case) as a metric label
/// value, e.g. `deny`, `cedar_error`. Only invoked when the shadow flag is ON.
fn metric_label(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Emit the shadow decision metric plus a disagreement counter (shadow effect vs
/// the enforced legacy effect). Labels stay low-cardinality (effect/engine/reason/
/// mode/domain); version + digest material lives on the audit event, not here.
fn emit_cedar_shadow_metrics(audit: &AuthorizationAuditEvent, legacy_effect: DecisionEffect) {
    let labels = audit.metric_labels();
    let mode = labels
        .mode
        .map_or_else(|| "none".to_owned(), |mode| metric_label(&mode));
    let domain = labels.domain.clone().unwrap_or_else(|| "none".to_owned());

    metrics::counter!(
        CEDAR_PBAC_SHADOW_TOTAL,
        "effect" => metric_label(&labels.effect),
        "engine" => metric_label(&labels.engine),
        "reason" => metric_label(&labels.reason),
        "mode" => mode,
        "domain" => domain.clone(),
    )
    .increment(1);

    if labels.effect != legacy_effect {
        metrics::counter!(
            CEDAR_PBAC_SHADOW_DISAGREEMENT_TOTAL,
            "domain" => domain,
            "shadow_effect" => metric_label(&labels.effect),
            "legacy_effect" => metric_label(&legacy_effect),
        )
        .increment(1);
    }
}

/// Persist the forensic shadow observation, best-effort, under armed RLS. An audit
/// write failure is logged and swallowed — it must NOT fail the live request.
async fn persist_cedar_shadow_audit(
    pool: &PgPool,
    org: OrgId,
    actor: UserId,
    audit: &AuthorizationAuditEvent,
) {
    let payload = match serde_json::to_value(audit) {
        Ok(payload) => payload,
        Err(err) => {
            tracing::warn!(event = "cedar_pbac_shadow_error", error = %err, "cedar/pbac shadow audit serialize failed");
            return;
        }
    };
    let action = match AuditAction::new(CEDAR_PBAC_SHADOW_AUDIT_ACTION) {
        Ok(action) => action,
        Err(err) => {
            tracing::warn!(event = "cedar_pbac_shadow_error", error = %err.message, "cedar/pbac shadow audit action invalid");
            return;
        }
    };
    let event = AuditEvent::new(
        Some(actor),
        action,
        "policy_role",
        actor.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(org)
    .with_snapshots(None, Some(payload));

    if let Err(err) =
        with_audit::<_, (), DbError>(pool, event, |_tx| Box::pin(async move { Ok(()) })).await
    {
        tracing::warn!(
            event = "cedar_pbac_shadow_error",
            error = %err,
            "cedar/pbac shadow audit persist failed (live decision unaffected)"
        );
    }
}

/// Authorize a user create/update for elevated-role membership changes and
/// branch membership mutations, mirroring the `issue_admin_otp` IDOR hardening:
///   * Adding OR removing EXECUTIVE/SUPER_ADMIN requires `ElevatedRoleGrant`
///     (SUPER_ADMIN). Preserving an existing elevated role while changing other
///     fields must not block a branch admin from granting ordinary branch
///     permissions such as ADMIN to an existing executive.
///   * Otherwise the caller needs `SubordinateUserCreate` (limited) in EVERY
///     branch in the union of `current_branches` and `target_branches`, so a
///     branch-scoped admin cannot mint users into — or strip users from —
///     branches outside their scope (console-0lj). Pass `&[]` for create.
fn authorize_user_write(
    principal: &Principal,
    elevated_role_changes: &BTreeSet<Role>,
    current_branches: &[BranchId],
    target_branches: &[BranchId],
) -> Result<(), RestError> {
    // Baseline user-management authority.
    authorize_org_manage(principal, Feature::UserManage)?;

    if !elevated_role_changes.is_empty() {
        // Only SUPER_ADMIN holds ElevatedRoleGrant; checked org-globally.
        return authorize_capability(principal, Action::new(Feature::ElevatedRoleGrant))
            .map_err(|_| RestError::forbidden("not allowed to change elevated roles"));
    }

    // Non-privileged user: cross-branch principals are already covered by the
    // UserManage check above; branch-scoped principals must additionally hold
    // SubordinateUserCreate in every branch the membership touches (add OR
    // remove). Union is deliberate: authorizing only the post-state set left
    // removals ungated (console-0lj).
    if matches!(principal.branch_scope, BranchScope::All) {
        return Ok(());
    }
    let mut touched = BTreeSet::new();
    for branch_id in current_branches.iter().chain(target_branches.iter()) {
        touched.insert(*branch_id);
    }
    for branch_id in touched {
        authorize(
            principal,
            Action::limited(Feature::SubordinateUserCreate),
            branch_id,
        )
        .map_err(|_| RestError::forbidden("not allowed to manage users in that branch"))?;
    }
    Ok(())
}

#[cfg(test)]
mod authorize_user_write_tests {
    use super::*;

    fn admin_in(branches: impl IntoIterator<Item = BranchId>) -> Principal {
        Principal::new(
            UserId::new(),
            OrgId::knl(),
            BTreeSet::from([Role::Admin]),
            BranchScope::Branches(branches.into_iter().collect()),
        )
    }

    /// console-0lj: a Branches({A}) admin must not drop U from B by PATCHing
    /// branch_ids to [A] alone. The guard authorizes the union of current and
    /// requested sets; post-state-only authorization is the live defect.
    #[test]
    fn refuses_branch_removal_outside_caller_scope() {
        let branch_a = BranchId::new();
        let branch_b = BranchId::new();
        let principal = admin_in([branch_a]);
        let err = authorize_user_write(
            &principal,
            &BTreeSet::new(),
            &[branch_a, branch_b],
            &[branch_a],
        )
        .expect_err("removing B without SubordinateUserCreate in B must fail");
        assert_eq!(err.status, StatusCode::FORBIDDEN);
        assert_eq!(err.code, "forbidden");
        assert_eq!(err.message, "not allowed to manage users in that branch");
    }

    #[test]
    fn allows_in_scope_membership_rewrite() {
        let branch_a = BranchId::new();
        let principal = admin_in([branch_a]);
        authorize_user_write(&principal, &BTreeSet::new(), &[branch_a], &[branch_a])
            .expect("in-scope rewrite must pass");
    }

    #[test]
    fn still_refuses_branch_addition_outside_caller_scope() {
        let branch_a = BranchId::new();
        let branch_b = BranchId::new();
        let principal = admin_in([branch_a]);
        let err = authorize_user_write(
            &principal,
            &BTreeSet::new(),
            &[branch_a],
            &[branch_a, branch_b],
        )
        .expect_err("adding B without SubordinateUserCreate in B must fail");
        assert_eq!(err.status, StatusCode::FORBIDDEN);
    }
}

fn elevated_roles_in(roles: &BTreeSet<Role>) -> BTreeSet<Role> {
    roles
        .iter()
        .copied()
        .filter(|role| matches!(role, Role::Executive | Role::SuperAdmin))
        .collect()
}

fn elevated_role_membership_changes(
    current_roles: &[String],
    requested_roles: Option<&BTreeSet<Role>>,
) -> Result<BTreeSet<Role>, RestError> {
    let Some(requested_roles) = requested_roles else {
        return Ok(BTreeSet::new());
    };
    let current_roles = parse_roles(current_roles)?;
    Ok(elevated_roles_in(&current_roles)
        .symmetric_difference(&elevated_roles_in(requested_roles))
        .copied()
        .collect())
}

/// Parse and validate role strings against the authz matrix. Empty role sets are
/// permitted (a user with no roles still exists and can sign in via Login only
/// once roles are added).
fn parse_roles(raw: &[String]) -> Result<BTreeSet<Role>, RestError> {
    raw.iter()
        .map(|role| {
            Role::from_str(role)
                .map_err(|_| RestError::validation(format!("unknown role {role:?}")))
        })
        .collect()
}

fn role_db_strings(roles: &BTreeSet<Role>) -> Vec<String> {
    roles.iter().map(|role| role.as_str().to_owned()).collect()
}

/// Whether the requested role SET differs from the stored `users.roles` array
/// (console-9sb).
///
/// `users.roles` is stored in [`Role`]'s derived (DECLARATION) order, while a
/// client echoes the array in any order it was given. Comparing the two as
/// SEQUENCES therefore misreports an unchanged role set in a different order as
/// a change (and a lexicographic `sort()` of the stored strings disagrees with
/// `Role`'s declaration order even further). Both sides are normalised to a set
/// so only a real membership change counts, matching the adapter's
/// `normalize_system_roles` semantics.
fn system_roles_changed(requested: &BTreeSet<Role>, current: &[String]) -> bool {
    let requested: BTreeSet<String> = role_db_strings(requested).into_iter().collect();
    let current: BTreeSet<String> = current.iter().cloned().collect();
    requested != current
}

#[cfg(test)]
mod system_roles_changed_tests {
    use super::*;

    /// console-9sb: `users.roles` is stored in `Role`'s DECLARATION order
    /// (`SUPER_ADMIN`, `ADMIN`, ...), but a lexicographic sort yields
    /// (`ADMIN`, `SUPER_ADMIN`). Comparing the two as sequences misreports an
    /// unchanged set as changed. The comparison must be order-insensitive.
    #[test]
    fn unchanged_role_set_in_a_different_order_is_not_a_change() {
        let requested: BTreeSet<Role> = [Role::SuperAdmin, Role::Admin].into_iter().collect();
        // The stored array echoed back in lexicographic order — the reverse of
        // `Role`'s declaration order.
        let current = vec!["ADMIN".to_owned(), "SUPER_ADMIN".to_owned()];
        assert!(
            !system_roles_changed(&requested, &current),
            "the same role set in a different order must not be a change"
        );
    }

    /// An actual membership change is still a change.
    #[test]
    fn role_set_membership_difference_is_a_change() {
        let requested: BTreeSet<Role> = [Role::SuperAdmin, Role::Admin].into_iter().collect();
        let current = vec!["ADMIN".to_owned()];
        assert!(system_roles_changed(&requested, &current));
    }
}

// ---------------------------------------------------------------------------
// Principal extraction
// ---------------------------------------------------------------------------

async fn principal_from_headers(
    state: &IdentityRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for identity API")
    })?;
    console_platform_request_context::resolve_principal(verifier, state.pool(), headers)
        .await
        .map_err(rest_error_from_request_context)
}

fn rest_error_from_request_context(
    err: console_platform_request_context::RequestContextError,
) -> RestError {
    match err {
        console_platform_request_context::RequestContextError::VerifierUnavailable => {
            RestError::unavailable("JWT verification is not configured for identity API")
        }
        console_platform_request_context::RequestContextError::WrongTokenTier => {
            RestError::from_kernel(KernelError::forbidden(
                "token tier is not valid for this route",
            ))
        }
        console_platform_request_context::RequestContextError::AccessScope(error) => {
            RestError::from_kernel(error)
        }
        console_platform_request_context::RequestContextError::BranchScope(message)
        | console_platform_request_context::RequestContextError::EffectivePolicy(message) => {
            RestError::from_kernel(KernelError::internal(message))
        }
        console_platform_request_context::RequestContextError::MissingOrg => {
            RestError::from_kernel(KernelError::internal(
                "no tenant context is bound to the current request",
            ))
        }
        console_platform_request_context::RequestContextError::MissingBearer => {
            RestError::unauthorized("missing or malformed bearer token")
        }
        console_platform_request_context::RequestContextError::InvalidToken => {
            RestError::unauthorized("invalid bearer token")
        }
        console_platform_request_context::RequestContextError::InvalidClaim(message) => {
            RestError::unauthorized(format!("token claim is invalid: {message}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

/// Public (doc-hidden) only because [`authorize_org_manage_observed`] is exposed for
/// the moved integration tests and names this in its return type; it stays an opaque
/// error (all fields private) and is not part of the crate's supported API.
#[doc(hidden)]
#[derive(Debug)]
pub struct RestError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl RestError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    fn validation(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "validation", message)
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            message,
        )
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }

    fn from_kernel(error: KernelError) -> Self {
        match error.kind {
            ErrorKind::Validation => Self::validation(error.message),
            ErrorKind::Forbidden => Self::forbidden(error.message),
            ErrorKind::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", error.message),
            ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                Self::new(StatusCode::CONFLICT, "conflict", error.message)
            }
            ErrorKind::Internal => Self::internal(error.message),
        }
    }

    fn from_store(error: PgOrgError) -> Self {
        match error {
            // Domain errors carry safe, caller-facing messages.
            PgOrgError::Domain(kernel) => Self::from_kernel(kernel),
            // Db errors must never leak raw sqlx strings / constraint names
            // (schema disclosure, OWASP A05). Log server-side; return generic.
            db_error => {
                let kind = db_error.kind();
                tracing::error!(error = %db_error, "identity database error");
                match kind {
                    ErrorKind::NotFound => {
                        Self::new(StatusCode::NOT_FOUND, "not_found", "resource not found")
                    }
                    ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                        Self::new(StatusCode::CONFLICT, "conflict", "resource already exists")
                    }
                    ErrorKind::Validation => {
                        Self::validation("request references an unknown region or branch")
                    }
                    _ => Self::internal("internal server error"),
                }
            }
        }
    }
}

/// A bare `DbError` surfaced from a `with_org_conn`/`with_audits` closure (the
/// passkey self-management reads/writes) is an internal failure: it must never
/// leak a raw sqlx string / constraint name to the caller (schema disclosure,
/// OWASP A05). Log server-side and return a generic 500.
impl From<DbError> for RestError {
    fn from(error: DbError) -> Self {
        tracing::error!(error = %error, "identity passkey database error");
        Self::internal("internal server error")
    }
}

/// A missing/invalid request context at a tenant-scoped data-access site is an
/// internal invariant violation (the request reached a tenant read without an
/// armed org). It never produces tenant data, so it maps to a generic 500.
impl From<RequestContextError> for RestError {
    fn from(error: RequestContextError) -> Self {
        tracing::error!(error = %error, "identity request context error");
        Self::internal("internal server error")
    }
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: ErrorPayload {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

// ---------------------------------------------------------------------------
// Cedar/PBAC role_manage shadow-wiring tests (activation slice 4).
//
// The load-bearing property is the ADR-0021 HIGH finding: the legacy
// authorization result is the SOLE enforcer, and the Cedar shadow lane can NEVER
// change a live outcome. These tests prove that both at the pure
// decision-combination level (forced Cedar Error/Deny) and end-to-end through the
// real `authorize_org_manage_observed` wrapper (dark default + flag-on + console_rt
// RLS).
// ---------------------------------------------------------------------------
#[cfg(test)]
mod cedar_shadow_wiring_tests {
    use console_platform_authz::{CedarEvaluation, DecisionReason, SubjectFreshness};

    use super::*;

    const RESOURCE_TYPE: &str = "identity.policy_role";
    const DOMAIN: &str = "identity.policy";

    fn principal_with_role(org: OrgId, role: Role) -> Principal {
        Principal::new(UserId::new(), org, BTreeSet::from([role]), BranchScope::All)
    }

    /// A shadow request built with VALID freshness + RLS proof so it reaches the
    /// `CedarShadowLegacyEnforce` boundary arm (rather than short-circuiting on a
    /// precondition), letting the test control the Cedar result directly.
    fn shadow_request(principal: &Principal, org: OrgId) -> AuthorizationRequest {
        AuthorizationRequest::new(
            principal.clone(),
            Action::new(Feature::RoleManage),
            AuthorizationResource::org_wide(org, RESOURCE_TYPE),
        )
        .with_policy_domain(DOMAIN)
        .with_subject_freshness(SubjectFreshness {
            policy_version: 1,
            subject_version: 1,
            session_generation: 1,
            step_up_generation: None,
        })
        .requiring_freshness(SubjectFreshnessRequirement {
            min_policy_version: 1,
            min_subject_version: 1,
            min_session_generation: 1,
            required_step_up_generation: None,
        })
        .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(org))
    }

    fn shadow_entry(
        bundle_key: console_platform_authz::CompiledBundleCacheKey,
    ) -> CoexistenceMapEntry {
        CoexistenceMapEntry::new(
            format!("{DOMAIN}.role_manage"),
            DOMAIN,
            Feature::RoleManage,
            RESOURCE_TYPE,
            DualEngineMode::CedarShadowLegacyEnforce,
            Some(bundle_key),
        )
    }

    /// THE safety test (ADR-0021 HIGH finding). With Cedar FORCED to `Error` and,
    /// separately, `Deny`, the coexistence boundary returns Deny (its shadow arm
    /// short-circuits to Cedar's deny) — yet the ENFORCED decision is the legacy
    /// `authorize_org_manage` Result, unchanged: SUPER_ADMIN stays ALLOW, everyone
    /// else stays DENY. Cedar cannot change the outcome.
    #[test]
    fn cedar_error_or_deny_never_changes_role_manage_enforcement() {
        let org = OrgId::knl();
        let bundle = engine::compile_bundle(org, 1).expect("pilot bundle must compile");
        let entry = shadow_entry(bundle.key.clone());

        let super_admin = principal_with_role(org, Role::SuperAdmin);
        let member = principal_with_role(org, Role::Member);

        // Legacy is the sole enforcer, computed independently of any Cedar result.
        assert!(
            authorize_org_manage(&super_admin, Feature::RoleManage).is_ok(),
            "legacy must ALLOW SUPER_ADMIN role_manage"
        );
        assert!(
            authorize_org_manage(&member, Feature::RoleManage).is_err(),
            "legacy must DENY non-SUPER_ADMIN role_manage"
        );

        let forced_results = [
            (
                CedarEvaluation::Error {
                    reason: "forced cedar error".to_owned(),
                },
                DecisionReason::CedarError,
            ),
            (
                CedarEvaluation::Deny {
                    bundle_key: bundle.key.clone(),
                    reason: "forced cedar deny".to_owned(),
                },
                DecisionReason::CedarDenied,
            ),
        ];

        for (forced, expected_reason) in forced_results {
            // The boundary observation WOULD deny (short-circuit to Cedar's deny)…
            let observed = evaluate_cedar_pbac_boundary(
                &shadow_request(&super_admin, org),
                Some(&entry),
                forced.clone(),
            );
            assert_eq!(
                observed.effect,
                DecisionEffect::Deny,
                "boundary must surface Cedar {forced:?} as a Deny observation"
            );
            assert_eq!(observed.reason, expected_reason);

            // …but the enforced decision is STILL the legacy result. This is the
            // exact contract `authorize_org_manage_observed` implements: return the
            // legacy `Result`, never the boundary effect.
            assert!(
                authorize_org_manage(&super_admin, Feature::RoleManage).is_ok(),
                "SUPER_ADMIN allow must stand despite forced Cedar {forced:?}"
            );
            assert!(
                authorize_org_manage(&member, Feature::RoleManage).is_err(),
                "MEMBER deny must stand despite forced Cedar {forced:?}"
            );
        }
    }
}
