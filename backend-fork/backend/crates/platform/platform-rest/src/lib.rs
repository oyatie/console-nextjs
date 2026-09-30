//! PLATFORM tier REST API â tenant onboarding and lifecycle.
//!
//! These routes live under `/api/platform/*` and are mounted behind the PLATFORM
//! extractor ([`console_platform_request_context::with_platform_context`]), NOT the
//! tenant org middleware. A TENANT token is rejected here (403) and a PLATFORM
//! token is rejected on the tenant `/api/v1/*` routes â the two tiers are strictly
//! separated, so a tenant admin can never reach a platform endpoint.
//!
//! They sit under the `/api` prefix so the ingress `/api`→backend rule reaches
//! them (the SPA owns the bare browser routes `/platform/*`); the `/api/platform`
//! namespace keeps the vendor data API collision-free with those client routes.
//!
//! Every write is cross-tenant and audited to the TARGET org (so the action
//! shows in both the platform and the tenant's audit trail).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod view_as;

pub use view_as::{
    PLATFORM_TENANT_CONTEXT_EXIT_PATH, PLATFORM_TENANT_CONTEXT_START_PATH,
    PLATFORM_VIEW_AS_EXIT_PATH, PLATFORM_VIEW_AS_START_PATH, TENANT_CONTEXT_TOKEN_TTL,
    VIEW_AS_READ_ONLY_CODE, VIEW_AS_TOKEN_TTL, with_view_as_read_only_gate,
};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, put};
use axum::{Extension, Json, Router};
use console_kernel_core::{OrgId, UserId};
use console_platform_auth::{JwtIssuer, JwtVerifier};
use console_platform_authz::{PlatformFeature, PlatformPrincipal};
use console_platform_provisioning::{
    GroupAccountOnboarding, GroupAccountSummary, GroupMemberSummary, GroupSummary,
    OrganizationSummary, PlatformProvisioner, ProvisioningError, RouteAdoptionMetric, TenantHealth,
    TenantOnboarding, TenantRemovalOutcome,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

pub const PLATFORM_ORGS_PATH: &str = "/api/platform/orgs";
pub const PLATFORM_ORG_PATH_TEMPLATE: &str = "/api/platform/orgs/{id}";
pub const PLATFORM_GROUPS_PATH: &str = "/api/platform/groups";
pub const PLATFORM_GROUP_PATH_TEMPLATE: &str = "/api/platform/groups/{id}";
pub const PLATFORM_GROUP_ACCOUNTS_PATH_TEMPLATE: &str = "/api/platform/groups/{id}/accounts";
pub const PLATFORM_GROUP_ACCOUNT_ROLE_PATH_TEMPLATE: &str =
    "/api/platform/groups/{id}/accounts/{user_id}/roles/{group_role}";
pub const PLATFORM_GROUP_ORG_PATH_TEMPLATE: &str =
    "/api/platform/groups/{id}/organizations/{org_id}";
pub const PLATFORM_OPS_PATH: &str = "/api/platform/ops";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlatformRouteOperation {
    pub method: &'static str,
    pub path: &'static str,
}

pub const PLATFORM_ROUTE_PATHS: &[&str] = &[
    PLATFORM_ORGS_PATH,
    PLATFORM_ORG_PATH_TEMPLATE,
    PLATFORM_GROUPS_PATH,
    PLATFORM_GROUP_PATH_TEMPLATE,
    PLATFORM_GROUP_ACCOUNTS_PATH_TEMPLATE,
    PLATFORM_GROUP_ACCOUNT_ROLE_PATH_TEMPLATE,
    PLATFORM_GROUP_ORG_PATH_TEMPLATE,
    PLATFORM_OPS_PATH,
    PLATFORM_VIEW_AS_START_PATH,
    PLATFORM_VIEW_AS_EXIT_PATH,
    PLATFORM_TENANT_CONTEXT_START_PATH,
    PLATFORM_TENANT_CONTEXT_EXIT_PATH,
];

/// Path+method inventory for the platform contract drift gate.
///
/// `PLATFORM_ROUTE_PATHS` keeps the legacy mounted-path coverage check green, but
/// it cannot detect a new method on an already-documented path. Keep this list in
/// lockstep with `router` and `view_as::routes`; `backend/app/tests/openapi_drift.rs`
/// fails CI when any `/api/platform/*` operation here is missing from OpenAPI.
pub const PLATFORM_ROUTE_OPERATIONS: &[PlatformRouteOperation] = &[
    PlatformRouteOperation {
        method: "GET",
        path: PLATFORM_ORGS_PATH,
    },
    PlatformRouteOperation {
        method: "POST",
        path: PLATFORM_ORGS_PATH,
    },
    PlatformRouteOperation {
        method: "PATCH",
        path: PLATFORM_ORG_PATH_TEMPLATE,
    },
    PlatformRouteOperation {
        method: "DELETE",
        path: PLATFORM_ORG_PATH_TEMPLATE,
    },
    PlatformRouteOperation {
        method: "GET",
        path: PLATFORM_GROUPS_PATH,
    },
    PlatformRouteOperation {
        method: "POST",
        path: PLATFORM_GROUPS_PATH,
    },
    PlatformRouteOperation {
        method: "PATCH",
        path: PLATFORM_GROUP_PATH_TEMPLATE,
    },
    PlatformRouteOperation {
        method: "GET",
        path: PLATFORM_GROUP_ACCOUNTS_PATH_TEMPLATE,
    },
    PlatformRouteOperation {
        method: "POST",
        path: PLATFORM_GROUP_ACCOUNTS_PATH_TEMPLATE,
    },
    PlatformRouteOperation {
        method: "DELETE",
        path: PLATFORM_GROUP_ACCOUNT_ROLE_PATH_TEMPLATE,
    },
    PlatformRouteOperation {
        method: "PUT",
        path: PLATFORM_GROUP_ORG_PATH_TEMPLATE,
    },
    PlatformRouteOperation {
        method: "DELETE",
        path: PLATFORM_GROUP_ORG_PATH_TEMPLATE,
    },
    PlatformRouteOperation {
        method: "GET",
        path: PLATFORM_OPS_PATH,
    },
    PlatformRouteOperation {
        method: "POST",
        path: PLATFORM_VIEW_AS_START_PATH,
    },
    PlatformRouteOperation {
        method: "POST",
        path: PLATFORM_VIEW_AS_EXIT_PATH,
    },
    PlatformRouteOperation {
        method: "POST",
        path: PLATFORM_TENANT_CONTEXT_START_PATH,
    },
    PlatformRouteOperation {
        method: "POST",
        path: PLATFORM_TENANT_CONTEXT_EXIT_PATH,
    },
];

/// Injected port that provisions a NEW tenant's governed-config object catalog
/// (§4-26 SLO settings, §19 console views) THROUGH the ontology engine, scoped to
/// the new org. Modeled as a boxed async callback so the platform tier does NOT
/// depend on the ontology ADAPTER (the layer boundary forbids `platform → adapter`);
/// the App composition root (`console-app`) supplies the concrete engine-backed impl,
/// and the wiring test supplies its own. `None` skips seeding (e.g. tests that only
/// exercise onboarding shell behavior).
pub type TenantConfigSeeder = std::sync::Arc<
    dyn Fn(
            OrgId,
            UserId,
            OffsetDateTime,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;

#[derive(Clone)]
pub struct PlatformRestState {
    pool: PgPool,
    /// Destructive tenant removal is executed only through this isolated
    /// capability pool; `pool` remains the general runtime/read path.
    force_remove_command_pool: Option<PgPool>,
    jwt_verifier: Option<JwtVerifier>,
    /// Issuer used only by platform START paths that mint short-lived tenant
    /// context tokens (read-only view-as and writable tenant management).
    /// `None` disables those START endpoints (503), so token issuance is opt-in.
    view_as_issuer: Option<JwtIssuer>,
    provisioner: PlatformProvisioner,
    /// Engine-backed catalog seeder run once per new tenant on onboarding. `None`
    /// leaves onboarding to create only the shell (org + admin + OTP).
    tenant_config_seeder: Option<TenantConfigSeeder>,
}

impl PlatformRestState {
    #[must_use]
    pub fn new(
        pool: PgPool,
        jwt_verifier: Option<JwtVerifier>,
        provisioner: PlatformProvisioner,
    ) -> Self {
        Self {
            pool,
            force_remove_command_pool: None,
            jwt_verifier,
            view_as_issuer: None,
            provisioner,
            tenant_config_seeder: None,
        }
    }

    /// Install the dedicated database identity allowed to execute the destructive
    /// platform force-removal function. `None` makes that endpoint fail closed.
    #[must_use]
    pub fn with_force_remove_command_pool(mut self, pool: Option<PgPool>) -> Self {
        self.force_remove_command_pool = pool;
        self
    }

    /// Install the engine-backed governed-config catalog seeder run for each newly
    /// onboarded tenant. Supplied by the App tier so the platform tier stays free
    /// of an ontology-adapter dependency.
    #[must_use]
    pub fn with_tenant_config_seeder(mut self, seeder: Option<TenantConfigSeeder>) -> Self {
        self.tenant_config_seeder = seeder;
        self
    }

    /// Install the JWT issuer used by platform START paths that mint tenant
    /// context tokens. Without it START endpoints return 503; EXIT and the
    /// read-only gate do not need an issuer.
    #[must_use]
    pub fn with_view_as_issuer(mut self, issuer: Option<JwtIssuer>) -> Self {
        self.view_as_issuer = issuer;
        self
    }
}

/// Build the `/api/platform/*` router behind the PLATFORM extractor.
pub fn router(state: PlatformRestState) -> Router {
    let pool = state.pool.clone();
    let verifier = state.jwt_verifier.clone();
    let router = Router::new()
        .route(PLATFORM_ORGS_PATH, get(list_orgs).post(create_org))
        .route(
            PLATFORM_ORG_PATH_TEMPLATE,
            patch(update_org).delete(delete_org),
        )
        .route(PLATFORM_GROUPS_PATH, get(list_groups).post(create_group))
        .route(PLATFORM_GROUP_PATH_TEMPLATE, patch(update_group))
        .route(
            PLATFORM_GROUP_ACCOUNTS_PATH_TEMPLATE,
            get(list_group_accounts).post(create_group_account),
        )
        .route(
            PLATFORM_GROUP_ACCOUNT_ROLE_PATH_TEMPLATE,
            axum::routing::delete(revoke_group_role),
        )
        .route(
            PLATFORM_GROUP_ORG_PATH_TEMPLATE,
            put(assign_org_to_group).delete(remove_org_from_group),
        )
        .route(PLATFORM_OPS_PATH, get(ops_dashboard));
    // View-as START + EXIT (read-only impersonation). Both are PLATFORM-tier
    // routes behind the platform extractor below; EXIT is platform-scoped so it
    // is reachable with the operator's platform token, never the view_as token.
    let router = view_as::routes(router).with_state(state);
    // PLATFORM extractor: resolves the PlatformPrincipal and REJECTS any tenant
    // token. Deliberately NOT the tenant org middleware â the platform tier is
    // not tenant-scoped, and each handler arms the TARGET org per action.
    console_platform_request_context::with_platform_context(router, verifier, pool)
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateOrgRequest {
    slug: String,
    name: String,
}

#[derive(Debug, Serialize)]
struct OrgResponse {
    id: Uuid,
    slug: String,
    name: String,
    status: String,
    group_id: Option<Uuid>,
    group_slug: Option<String>,
    group_name: Option<String>,
    // `time::OffsetDateTime` derives a numeric-array Serialize by default; the
    // console reads these as rfc3339 strings (`new Date(created_at)`), so emit
    // rfc3339 like every other tenant DTO (e.g. financial `created_at`).
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

impl From<OrganizationSummary> for OrgResponse {
    fn from(o: OrganizationSummary) -> Self {
        Self {
            id: o.id,
            slug: o.slug,
            name: o.name,
            status: o.status,
            group_id: o.group_id,
            group_slug: o.group_slug,
            group_name: o.group_name,
            created_at: o.created_at,
            updated_at: o.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
struct CreateGroupRequest {
    slug: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct UpdateGroupRequest {
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// New group lifecycle status: ACTIVE | SUSPENDED | ARCHIVED.
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Serialize)]
struct GroupMemberResponse {
    id: Uuid,
    slug: String,
    name: String,
    status: String,
}

impl From<GroupMemberSummary> for GroupMemberResponse {
    fn from(member: GroupMemberSummary) -> Self {
        Self {
            id: member.id,
            slug: member.slug,
            name: member.name,
            status: member.status,
        }
    }
}

#[derive(Debug, Serialize)]
struct GroupResponse {
    id: Uuid,
    slug: String,
    name: String,
    status: String,
    member_count: i64,
    members: Vec<GroupMemberResponse>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

impl From<GroupSummary> for GroupResponse {
    fn from(group: GroupSummary) -> Self {
        Self {
            id: group.id,
            slug: group.slug,
            name: group.name,
            status: group.status,
            member_count: group.member_count,
            members: group
                .members
                .into_iter()
                .map(GroupMemberResponse::from)
                .collect(),
            created_at: group.created_at,
            updated_at: group.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
struct CreateGroupAccountRequest {
    org_id: Uuid,
    display_name: String,
    #[serde(default)]
    phone: Option<String>,
    /// Tenant-local roles for the account's home org. Defaults to MEMBER so
    /// group authority remains explicit in group_role_grants.
    #[serde(default)]
    tenant_roles: Option<Vec<String>>,
    /// GROUP_ADMIN | GROUP_VIEWER | GROUP_FINANCE. Defaults to GROUP_ADMIN for
    /// the platform "add group account" workflow.
    #[serde(default)]
    group_role: Option<String>,
}

#[derive(Debug, Serialize)]
struct GroupAccountResponse {
    user_id: Uuid,
    display_name: String,
    phone: Option<String>,
    tenant_roles: Vec<String>,
    is_active: bool,
    has_passkey: bool,
    account_status: String,
    org_id: Uuid,
    org_slug: String,
    org_name: String,
    group_roles: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

impl From<GroupAccountSummary> for GroupAccountResponse {
    fn from(account: GroupAccountSummary) -> Self {
        Self {
            user_id: account.user_id,
            display_name: account.display_name,
            phone: account.phone,
            tenant_roles: account.tenant_roles,
            is_active: account.is_active,
            has_passkey: account.has_passkey,
            account_status: account.account_status,
            org_id: account.org_id,
            org_slug: account.org_slug,
            org_name: account.org_name,
            group_roles: account.group_roles,
            created_at: account.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
struct CreateGroupAccountResponse {
    account: GroupAccountResponse,
    /// One-time setup code for the created tenant account. Returned once only.
    otp: String,
    #[serde(with = "time::serde::rfc3339")]
    otp_expires_at: OffsetDateTime,
}

impl From<GroupAccountOnboarding> for CreateGroupAccountResponse {
    fn from(value: GroupAccountOnboarding) -> Self {
        Self {
            account: GroupAccountResponse::from(value.account),
            otp: value.otp.as_str().to_owned(),
            otp_expires_at: value.otp_expires_at,
        }
    }
}

#[derive(Debug, Serialize)]
struct OnboardingResponse {
    // The console's `OnboardOrgResponse` reads `{ org, otp }`; keep those exact
    // field names so the one-time OTP and the new org actually surface in the UI.
    org: OrgResponse,
    admin_user_id: Uuid,
    /// The ONE-TIME OTP for the new tenant's first SUPER_ADMIN. Returned exactly
    /// once, to be delivered out-of-band; never logged or stored in cleartext.
    otp: String,
    #[serde(with = "time::serde::rfc3339")]
    admin_otp_expires_at: OffsetDateTime,
}

impl From<TenantOnboarding> for OnboardingResponse {
    fn from(o: TenantOnboarding) -> Self {
        Self {
            org: o.organization.into(),
            admin_user_id: o.admin_user_id,
            otp: o.admin_otp.as_str().to_owned(),
            admin_otp_expires_at: o.admin_otp_expires_at,
        }
    }
}

#[derive(Debug, Deserialize)]
struct UpdateOrgRequest {
    /// New tenant status: ACTIVE | SUSPENDED | ARCHIVED.
    status: String,
}

/// Query params for `DELETE /api/platform/orgs/{id}`.
#[derive(Debug, Deserialize)]
struct DeleteOrgQuery {
    /// Opt-in FORCE removal: when true, take the DESTRUCTIVE path that erases the
    /// tenant AND all of its data (requires the tenant to be ARCHIVED first).
    /// Defaults to false — the GUARDED path that removes only an empty shell.
    #[serde(default)]
    delete_data: bool,
}

#[derive(Debug, Serialize)]
struct TenantHealthResponse {
    id: Uuid,
    slug: String,
    name: String,
    status: String,
    group_id: Option<Uuid>,
    group_slug: Option<String>,
    group_name: Option<String>,
    user_count: i64,
    active_user_count: i64,
    active_work_orders: i64,
    open_work_orders: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    last_activity_at: Option<OffsetDateTime>,
    route_adoption: Vec<RouteAdoptionMetricResponse>,
    zero_legacy_release_cycles: i64,
}

#[derive(Debug, Serialize)]
struct RouteAdoptionMetricResponse {
    release_cycle: String,
    console_route_events: i64,
    legacy_route_events: i64,
    rum_error_events: i64,
    rum_perf_p95_ms: Option<i64>,
    #[serde(with = "time::serde::rfc3339")]
    last_event_at: OffsetDateTime,
}

impl From<RouteAdoptionMetric> for RouteAdoptionMetricResponse {
    fn from(metric: RouteAdoptionMetric) -> Self {
        Self {
            release_cycle: metric.release_cycle,
            console_route_events: metric.console_route_events,
            legacy_route_events: metric.legacy_route_events,
            rum_error_events: metric.rum_error_events,
            rum_perf_p95_ms: metric.rum_perf_p95_ms,
            last_event_at: metric.last_event_at,
        }
    }
}

impl From<TenantHealth> for TenantHealthResponse {
    fn from(h: TenantHealth) -> Self {
        Self {
            id: h.id,
            slug: h.slug,
            name: h.name,
            status: h.status,
            group_id: h.group_id,
            group_slug: h.group_slug,
            group_name: h.group_name,
            user_count: h.user_count,
            active_user_count: h.active_user_count,
            active_work_orders: h.active_work_orders,
            open_work_orders: h.open_work_orders,
            last_activity_at: h.last_activity_at,
            route_adoption: h
                .route_adoption
                .into_iter()
                .map(RouteAdoptionMetricResponse::from)
                .collect(),
            zero_legacy_release_cycles: h.zero_legacy_release_cycles,
        }
    }
}

#[derive(Debug, Serialize)]
struct PlatformOpsResponse {
    tenants: Vec<TenantHealthResponse>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/platform/orgs — onboard a NEW tenant (the only place org rows are
/// created by the app), seed its first SUPER_ADMIN, and return a one-time OTP.
async fn create_org(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
    Json(body): Json<CreateOrgRequest>,
) -> Result<Response, PlatformError> {
    principal
        .authorize(PlatformFeature::TenantCreate)
        .map_err(|_| PlatformError::forbidden("platform principal cannot create tenants"))?;

    let onboarding = state
        .provisioner
        .onboard_tenant(
            &state.pool,
            Some(principal.user_id),
            &body.slug,
            &body.name,
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(PlatformError::from_provisioning)?;

    // Provision the standard governed-config object catalog (§4-26 SLO settings,
    // §19 console views) for the new tenant THROUGH the ontology engine (the
    // App-injected seeder), scoped to its freshly-created org so the registry
    // writes pass FORCE-RLS. The engine uses its own connection, so this runs
    // after the onboarding tx commits; a rare seeding failure surfaces as a 500
    // rather than silently leaving a tenant without its config catalog.
    // The seed's registry rows are `created_by` the actor, whose `(id, org_id)`
    // must reference a user IN the new org (FORCE-RLS + FK), so the actor is the
    // freshly-seeded tenant admin — never the platform principal (a sentinel-org
    // user that would fail the tenant-scoped FK).
    // ponytail: not atomic with org creation (separate engine connection) — the
    // org row persists if this fails. Acceptable for a rare admin action; make the
    // seed tx-scoped only if half-provisioned tenants become an operational problem.
    if let Some(seeder) = &state.tenant_config_seeder {
        seeder(
            OrgId::from_uuid(onboarding.organization.id),
            UserId::from_uuid(onboarding.admin_user_id),
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "seeding governed config object types for new tenant failed");
            PlatformError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal server error",
            )
        })?;
    }

    Ok((
        StatusCode::CREATED,
        Json(OnboardingResponse::from(onboarding)),
    )
        .into_response())
}

/// GET /api/platform/orgs — list all tenants (cross-tenant, audited read).
async fn list_orgs(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
) -> Result<Response, PlatformError> {
    principal
        .authorize(PlatformFeature::TenantList)
        .map_err(|_| PlatformError::forbidden("platform principal cannot list tenants"))?;

    let orgs = state
        .provisioner
        .list_tenants(
            &state.pool,
            Some(principal.user_id),
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(PlatformError::from_provisioning)?;

    // The console's `listPlatformOrgs` reads a bare array, so return the orgs
    // directly rather than wrapping them in an envelope.
    let items: Vec<OrgResponse> = orgs.into_iter().map(OrgResponse::from).collect();
    Ok(Json(items).into_response())
}

/// GET /api/platform/groups — list all top-level groups and their member org identities.
async fn list_groups(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
) -> Result<Response, PlatformError> {
    principal
        .authorize(PlatformFeature::GroupManage)
        .map_err(|_| PlatformError::forbidden("platform principal cannot list groups"))?;

    let groups = state
        .provisioner
        .list_groups(
            &state.pool,
            Some(principal.user_id),
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(PlatformError::from_provisioning)?;

    let items: Vec<GroupResponse> = groups.into_iter().map(GroupResponse::from).collect();
    Ok(Json(items).into_response())
}

/// POST /api/platform/groups — create a group identity (not a tenant).
async fn create_group(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
    Json(body): Json<CreateGroupRequest>,
) -> Result<Response, PlatformError> {
    principal
        .authorize(PlatformFeature::GroupManage)
        .map_err(|_| PlatformError::forbidden("platform principal cannot create groups"))?;

    let group = state
        .provisioner
        .create_group(
            &state.pool,
            Some(principal.user_id),
            &body.slug,
            &body.name,
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(PlatformError::from_provisioning)?;

    Ok((StatusCode::CREATED, Json(GroupResponse::from(group))).into_response())
}

/// PATCH /api/platform/groups/{id} — update group identity/status.
async fn update_group(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateGroupRequest>,
) -> Result<Response, PlatformError> {
    principal
        .authorize(PlatformFeature::GroupManage)
        .map_err(|_| PlatformError::forbidden("platform principal cannot update groups"))?;

    let group = state
        .provisioner
        .update_group(
            &state.pool,
            Some(principal.user_id),
            id,
            body.slug.as_deref(),
            body.name.as_deref(),
            body.status.as_deref(),
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(PlatformError::from_provisioning)?;

    Ok(Json(GroupResponse::from(group)).into_response())
}

/// GET /api/platform/groups/{id}/accounts — list tenant-anchored group accounts.
async fn list_group_accounts(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
    Path(id): Path<Uuid>,
) -> Result<Response, PlatformError> {
    principal
        .authorize(PlatformFeature::GroupManage)
        .map_err(|_| PlatformError::forbidden("platform principal cannot list group accounts"))?;

    let accounts = state
        .provisioner
        .list_group_accounts(
            &state.pool,
            Some(principal.user_id),
            id,
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(PlatformError::from_provisioning)?;

    let items: Vec<GroupAccountResponse> = accounts
        .into_iter()
        .map(GroupAccountResponse::from)
        .collect();
    Ok(Json(items).into_response())
}

/// POST /api/platform/groups/{id}/accounts — create a tenant-anchored group account.
async fn create_group_account(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateGroupAccountRequest>,
) -> Result<Response, PlatformError> {
    principal
        .authorize(PlatformFeature::GroupManage)
        .map_err(|_| PlatformError::forbidden("platform principal cannot create group accounts"))?;

    let tenant_roles = body
        .tenant_roles
        .unwrap_or_else(|| vec!["MEMBER".to_owned()]);
    let group_role = body.group_role.unwrap_or_else(|| "GROUP_ADMIN".to_owned());
    let created = state
        .provisioner
        .create_group_account(
            &state.pool,
            Some(principal.user_id),
            id,
            body.org_id,
            &body.display_name,
            body.phone.as_deref(),
            &tenant_roles,
            &group_role,
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(PlatformError::from_provisioning)?;

    Ok((
        StatusCode::CREATED,
        Json(CreateGroupAccountResponse::from(created)),
    )
        .into_response())
}

/// DELETE /api/platform/groups/{id}/accounts/{user_id}/roles/{group_role}.
async fn revoke_group_role(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
    Path((id, user_id, group_role)): Path<(Uuid, Uuid, String)>,
) -> Result<Response, PlatformError> {
    principal
        .authorize(PlatformFeature::GroupManage)
        .map_err(|_| PlatformError::forbidden("platform principal cannot revoke group roles"))?;

    state
        .provisioner
        .revoke_group_role(
            &state.pool,
            Some(principal.user_id),
            id,
            user_id,
            &group_role,
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(PlatformError::from_provisioning)?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// PUT /api/platform/groups/{id}/organizations/{org_id} — assign/move org into group.
async fn assign_org_to_group(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
    Path((id, org_id)): Path<(Uuid, Uuid)>,
) -> Result<Response, PlatformError> {
    principal
        .authorize(PlatformFeature::GroupManage)
        .map_err(|_| PlatformError::forbidden("platform principal cannot assign group members"))?;

    let org = state
        .provisioner
        .assign_org_to_group(
            &state.pool,
            Some(principal.user_id),
            id,
            org_id,
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(PlatformError::from_provisioning)?;

    Ok(Json(OrgResponse::from(org)).into_response())
}

/// DELETE /api/platform/groups/{id}/organizations/{org_id} — remove org from group.
async fn remove_org_from_group(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
    Path((id, org_id)): Path<(Uuid, Uuid)>,
) -> Result<Response, PlatformError> {
    principal
        .authorize(PlatformFeature::GroupManage)
        .map_err(|_| PlatformError::forbidden("platform principal cannot remove group members"))?;

    let org = state
        .provisioner
        .remove_org_from_group(
            &state.pool,
            Some(principal.user_id),
            id,
            org_id,
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(PlatformError::from_provisioning)?;

    Ok(Json(OrgResponse::from(org)).into_response())
}

/// GET /api/platform/ops — cross-tenant ops health rollup (audited platform read).
///
/// Aggregates per-tenant health/usage numbers for EVERY tenant via the
/// SECURITY DEFINER `platform_org_health()` function â the only sanctioned
/// cross-tenant path. The read is audited (`platform.tenant.health`); a tenant
/// token is rejected with 403 by the platform extractor before this runs.
async fn ops_dashboard(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
) -> Result<Response, PlatformError> {
    principal
        .authorize(PlatformFeature::TenantHealthRead)
        .map_err(|_| PlatformError::forbidden("platform principal cannot read tenant health"))?;

    let health = state
        .provisioner
        .list_tenant_health(
            &state.pool,
            Some(principal.user_id),
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(PlatformError::from_provisioning)?;

    let tenants = health.into_iter().map(TenantHealthResponse::from).collect();
    Ok(Json(PlatformOpsResponse { tenants }).into_response())
}

/// PATCH /api/platform/orgs/{id} — suspend / reactivate a tenant (audited).
async fn update_org(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateOrgRequest>,
) -> Result<Response, PlatformError> {
    principal
        .authorize(PlatformFeature::TenantSuspend)
        .map_err(|_| PlatformError::forbidden("platform principal cannot change tenant status"))?;

    let org = state
        .provisioner
        .set_tenant_status(
            &state.pool,
            Some(principal.user_id),
            id,
            &body.status,
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(PlatformError::from_provisioning)?;

    Ok(Json(OrgResponse::from(org)).into_response())
}

/// DELETE /api/platform/orgs/{id}[?delete_data=true] — remove a tenant.
///
/// Platform-super-admin (vendor tier) ONLY â identical gate to `update_org`; a
/// tenant's own admin can never reach this (the platform extractor rejects a
/// tenant token with 403 before this runs).
///
/// Two paths, selected by the opt-in `delete_data` query param (default false):
///   * `delete_data=false` (default) â GUARDED removal (`platform.tenant.remove`).
///     Removes only an empty/test tenant's onboarding shell; REFUSES with 409
///     (code `tenant_has_data`) when the tenant owns real operational data,
///     telling the operator to archive instead. Unchanged behaviour.
///   * `delete_data=true` â FORCE removal (`platform.tenant.force_remove`). The
///     DESTRUCTIVE path: erases the org AND all of its data. Fail-closed by a
///     status rail â REFUSES with 409 (code `tenant_active`) unless the tenant is
///     ARCHIVED, telling the operator to archive (reversible) before force-
///     removing. Erasing real data is the whole point, so there is no has_data
///     guard on this path.
///
/// Both paths delete in ONE transaction and preserve the tenant's immutable audit
/// trail (re-homed to the platform sentinel). A missing tenant is 404.
async fn delete_org(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
    Path(id): Path<Uuid>,
    Query(query): Query<DeleteOrgQuery>,
) -> Result<Response, PlatformError> {
    // Same authz gate for BOTH paths: platform super-admin only. A tenant token is
    // already rejected (403) by the platform extractor before this handler runs.
    principal
        .authorize(PlatformFeature::TenantRemove)
        .map_err(|_| PlatformError::forbidden("platform principal cannot remove tenants"))?;

    let actor = Some(principal.user_id);
    let now = OffsetDateTime::now_utc();
    let outcome = if query.delete_data {
        let force_pool = state.force_remove_command_pool.as_ref().ok_or_else(|| {
            PlatformError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "platform_force_command_unavailable",
                "platform force-remove command capability is unavailable",
            )
        })?;
        state
            .provisioner
            .force_remove_tenant(force_pool, actor, id, now)
            .await
    } else {
        state
            .provisioner
            .remove_tenant(&state.pool, actor, id, now)
            .await
    }
    .map_err(PlatformError::from_provisioning)?;

    match outcome {
        TenantRemovalOutcome::Removed => Ok(StatusCode::NO_CONTENT.into_response()),
        TenantRemovalOutcome::BlockedHasData => Err(PlatformError::new(
            StatusCode::CONFLICT,
            "tenant_has_data",
            "tenant has operational data and cannot be removed; archive it instead",
        )),
        TenantRemovalOutcome::BlockedActive => Err(PlatformError::new(
            StatusCode::CONFLICT,
            "tenant_active",
            "archive the tenant before force-removing",
        )),
        TenantRemovalOutcome::NotFound => Err(PlatformError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "no such tenant",
        )),
    }
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct PlatformError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl PlatformError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    fn validation(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "validation", message)
    }

    fn from_provisioning(err: ProvisioningError) -> Self {
        match err {
            // Caller-facing input problems map to 422; everything else is logged
            // and collapsed to a generic 500 so no DB/constraint detail leaks.
            ProvisioningError::InvalidRoster(message) => Self::validation(message),
            ProvisioningError::ActiveBootstrapCredentialExists => Self::new(
                StatusCode::CONFLICT,
                "conflict",
                "tenant admin already has an active bootstrap credential",
            ),
            ProvisioningError::NotFound(message) => {
                Self::new(StatusCode::NOT_FOUND, "not_found", message)
            }
            ProvisioningError::Conflict(message) => {
                Self::new(StatusCode::CONFLICT, "conflict", message)
            }
            other => {
                tracing::error!(error = %other, "platform provisioning error");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal server error",
                )
            }
        }
    }
}

impl IntoResponse for PlatformError {
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

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
}
