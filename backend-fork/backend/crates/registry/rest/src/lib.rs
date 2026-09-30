//! Registry REST API.
//!
//! This layer handles JWT authentication, branch-scoped authorization, and
//! HTTP error mapping for equipment registry use cases. State-changing
//! substitute assignment operations remain in the Postgres adapter and route
//! through `with_audit`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_kernel_core::{
    ADDRESS_MAX_CHARS, AuditEventId, BranchId, BranchScope, CITY_MAX_CHARS,
    CONTACT_EMAIL_MAX_CHARS, CONTACT_NAME_MAX_CHARS, CONTACT_PHONE_MAX_CHARS,
    CUSTOMER_SITE_NAME_MAX_CHARS, CustomerId, EquipmentId, EquipmentSubstitutionId, ErrorKind,
    KernelError, POSTAL_CODE_MAX_CHARS, PROVINCE_MAX_CHARS, SiteId, TraceContext, UserId,
    validate_bounded_text, validate_coordinate_pair,
};
use console_platform_auth::{JwtVerifier, PasskeyAuthenticationCredential, PasskeyService};
use console_platform_authz::{
    Action, Feature, Principal, Role, authorize_capability, authorize_org_wide,
};
use console_registry_adapter_postgres::{PgRegistryError, PgRegistryStore};
use console_registry_application::{
    CreateCustomerCommand, CreateEquipmentCommand, CreateEquipmentOwnershipTransferCommand,
    CreateSiteCommand, CreatedCustomer, CreatedSite, DecideEquipmentOwnershipTransferCommand,
    DeleteEquipmentCommand, EquipmentByLocationQuery, EquipmentListItem, EquipmentListQuery,
    EquipmentOwnershipTransferDecision, EquipmentOwnershipTransferRequest,
    EquipmentOwnershipTransferStatus, EquipmentOwnershipTransferStepKey, EquipmentReadQuery,
    EquipmentSortBy, EquipmentTimelineGraph, EquipmentTimelineGraphQuery, RegistryImportReport,
    RollbackEquipmentCommand, SiteLocationGroup, SubstituteAssignment, SubstituteAssignmentCommand,
    SubstituteCandidate, SubstituteReturnCommand, SubstituteSearch, UpdateEquipmentCommand,
    UpdateEquipmentFields, UpdateSiteCommand, UpdateSiteFields,
};
use console_registry_domain::{EquipmentNo, EquipmentStatus, MoneyWon, SubstituteMatchKind, Ton};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::Date;
use time::OffsetDateTime;
use uuid::Uuid;

pub const EQUIPMENT_PATH: &str = "/api/v1/equipment";
pub const EQUIPMENT_LIST_PATH: &str = "/api/v1/equipment/list";
pub const EQUIPMENT_IMPORT_PATH: &str = "/api/v1/equipment/import";
pub const EQUIPMENT_ID_PATH_TEMPLATE: &str = "/api/v1/equipment/{id}";
pub const EQUIPMENT_VERSIONS_PATH_TEMPLATE: &str = "/api/v1/equipment/{id}/versions";
pub const EQUIPMENT_VERSION_ROLLBACK_PATH_TEMPLATE: &str =
    "/api/v1/equipment/{id}/versions/{version}/rollback";
pub const EQUIPMENT_TIMELINE_GRAPH_PATH_TEMPLATE: &str = "/api/v1/equipment/{id}/timeline-graph";
pub const EQUIPMENT_SUBSTITUTES_PATH_TEMPLATE: &str = "/api/v1/equipment/{id}/substitutes";
pub const EQUIPMENT_SUBSTITUTIONS_PATH: &str = "/api/v1/equipment-substitutions";
pub const EQUIPMENT_SUBSTITUTION_RETURN_PATH_TEMPLATE: &str =
    "/api/v1/equipment-substitutions/{id}/return";
pub const EQUIPMENT_OWNERSHIP_TRANSFERS_PATH_TEMPLATE: &str =
    "/api/v1/equipment/{id}/ownership-transfer-requests";
pub const EQUIPMENT_OWNERSHIP_TRANSFER_DECISION_PATH_TEMPLATE: &str =
    "/api/v1/equipment/ownership-transfer-requests/{id}/decisions";
pub const EQUIPMENT_BY_LOCATION_PATH: &str = "/api/v1/equipment-by-location";
pub const OBJECT_ACTION_CATALOG_PATH: &str = "/api/v1/object-actions/catalog";
pub const OBJECT_ACTION_EXECUTE_PATH: &str = "/api/v1/object-actions/execute";
pub const CUSTOMERS_PATH: &str = "/api/v1/customers";
pub const SITES_PATH: &str = "/api/v1/sites";
pub const SITE_ID_PATH_TEMPLATE: &str = "/api/v1/sites/{id}";
pub const REGISTRY_ROUTE_PATHS: &[&str] = &[
    EQUIPMENT_PATH,
    EQUIPMENT_LIST_PATH,
    EQUIPMENT_IMPORT_PATH,
    EQUIPMENT_ID_PATH_TEMPLATE,
    EQUIPMENT_VERSIONS_PATH_TEMPLATE,
    EQUIPMENT_VERSION_ROLLBACK_PATH_TEMPLATE,
    EQUIPMENT_TIMELINE_GRAPH_PATH_TEMPLATE,
    EQUIPMENT_SUBSTITUTES_PATH_TEMPLATE,
    EQUIPMENT_SUBSTITUTIONS_PATH,
    EQUIPMENT_SUBSTITUTION_RETURN_PATH_TEMPLATE,
    EQUIPMENT_OWNERSHIP_TRANSFERS_PATH_TEMPLATE,
    EQUIPMENT_OWNERSHIP_TRANSFER_DECISION_PATH_TEMPLATE,
    EQUIPMENT_BY_LOCATION_PATH,
    OBJECT_ACTION_CATALOG_PATH,
    OBJECT_ACTION_EXECUTE_PATH,
    CUSTOMERS_PATH,
    SITES_PATH,
    SITE_ID_PATH_TEMPLATE,
];

/// Hard cap on an uploaded master-list workbook. The reference master-list is a
/// few hundred KiB; 16 MiB leaves generous headroom while bounding memory.
const MAX_IMPORT_BYTES: usize = 16 * 1024 * 1024;
const EQUIPMENT_UPDATE_PROFILE_ACTION_ID: &str = "equipment.update_profile";
const OBJECT_ACTION_EXECUTION_TOTAL: &str = "object_action_execution_total";

#[derive(Clone)]
pub struct RegistryRestState {
    store: PgRegistryStore,
    jwt_verifier: Option<JwtVerifier>,
    passkey_step_up: Option<PasskeyService>,
}

impl RegistryRestState {
    #[must_use]
    pub fn new(store: PgRegistryStore, jwt_verifier: Option<JwtVerifier>) -> Self {
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
}

pub fn router(state: RegistryRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(
            EQUIPMENT_SUBSTITUTES_PATH_TEMPLATE,
            get(list_equipment_substitutes),
        )
        .route(
            EQUIPMENT_SUBSTITUTIONS_PATH,
            post(assign_equipment_substitute),
        )
        .route(
            EQUIPMENT_SUBSTITUTION_RETURN_PATH_TEMPLATE,
            post(return_equipment_substitute),
        )
        .route(
            EQUIPMENT_OWNERSHIP_TRANSFERS_PATH_TEMPLATE,
            get(list_equipment_ownership_transfers).post(create_equipment_ownership_transfer),
        )
        .route(
            EQUIPMENT_OWNERSHIP_TRANSFER_DECISION_PATH_TEMPLATE,
            post(decide_equipment_ownership_transfer),
        )
        .route(EQUIPMENT_LIST_PATH, get(list_equipment))
        .route(EQUIPMENT_PATH, post(create_equipment))
        // The import route accepts a workbook upload, so it overrides axum's
        // 2 MiB default body limit up to MAX_IMPORT_BYTES — a tower-level cap
        // that rejects oversized uploads before the handler reads them. The
        // streaming check in `read_xlsx_upload` enforces the same bound on the
        // body itself (chunked transfer with no honest Content-Length).
        .route(
            EQUIPMENT_IMPORT_PATH,
            post(import_master_list).layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES)),
        )
        .route(
            EQUIPMENT_TIMELINE_GRAPH_PATH_TEMPLATE,
            get(get_equipment_timeline_graph),
        )
        .route(OBJECT_ACTION_CATALOG_PATH, get(get_object_action_catalog))
        .route(OBJECT_ACTION_EXECUTE_PATH, post(execute_object_action))
        .route(
            EQUIPMENT_ID_PATH_TEMPLATE,
            get(get_equipment)
                .patch(update_equipment)
                .delete(delete_equipment),
        )
        .route(
            EQUIPMENT_VERSIONS_PATH_TEMPLATE,
            get(list_equipment_versions),
        )
        .route(
            EQUIPMENT_VERSION_ROLLBACK_PATH_TEMPLATE,
            post(rollback_equipment),
        )
        .route(EQUIPMENT_BY_LOCATION_PATH, get(equipment_by_location))
        .route(CUSTOMERS_PATH, post(create_customer))
        .route(SITES_PATH, post(create_site))
        .route(SITE_ID_PATH_TEMPLATE, axum::routing::patch(update_site))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

#[derive(Debug, Deserialize)]
struct SubstituteQuery {
    all_branches: Option<bool>,
}

#[derive(Debug, Serialize)]
struct SubstituteCandidatePage {
    items: Vec<SubstituteCandidateResponse>,
    total: usize,
}

#[derive(Debug, Serialize)]
struct SubstituteCandidateResponse {
    equipment_id: EquipmentId,
    branch_id: BranchId,
    equipment_no: String,
    management_no: Option<String>,
    model: Option<String>,
    status: EquipmentStatus,
    specification: String,
    ton_text: String,
    ton_milli: Option<i32>,
    power_code: String,
    power_label: Option<String>,
    customer_name: String,
    site_name: String,
    placement_location: Option<String>,
    match_kind: SubstituteMatchKind,
    ton_delta_milli: Option<i32>,
}

async fn list_equipment_substitutes(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Path(equipment_id): Path<EquipmentId>,
    Query(query): Query<SubstituteQuery>,
) -> Result<Json<SubstituteCandidatePage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_read_access(&principal)?;
    let include_all_branches = query.all_branches.unwrap_or(false);
    if include_all_branches && !is_super_admin(&principal) {
        return Err(RestError::forbidden(
            "all_branches substitute search requires SUPER_ADMIN",
        ));
    }

    let items = state
        .store
        .substitute_candidates(SubstituteSearch {
            equipment_id,
            branch_scope: principal.branch_scope,
            include_all_branches,
        })
        .await
        .map_err(RestError::from_store)?
        .into_iter()
        .map(SubstituteCandidateResponse::from)
        .collect::<Vec<_>>();
    let total = items.len();

    Ok(Json(SubstituteCandidatePage { items, total }))
}

impl From<SubstituteCandidate> for SubstituteCandidateResponse {
    fn from(value: SubstituteCandidate) -> Self {
        Self {
            equipment_id: value.equipment_id,
            branch_id: value.branch_id,
            equipment_no: value.equipment_no.to_string(),
            management_no: value.management_no,
            model: value.model,
            status: value.status,
            specification: value.specification,
            ton_text: value.ton.as_text().to_owned(),
            ton_milli: value.ton.milli_tons(),
            power_code: value.power_code,
            power_label: value.power_label,
            customer_name: value.customer_name,
            site_name: value.site_name,
            placement_location: value.placement_location,
            match_kind: value.match_kind,
            ton_delta_milli: value.ton_delta_milli,
        }
    }
}

// ---------------------------------------------------------------------------
// Equipment-by-location — dispatch-map aggregation (read, all read-access roles)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct EquipmentByLocationPage {
    items: Vec<SiteLocationResponse>,
    total: usize,
}

#[derive(Debug, Serialize)]
struct SiteLocationResponse {
    site_id: SiteId,
    site_name: String,
    customer_id: CustomerId,
    customer_name: String,
    branch_id: BranchId,
    address: Option<String>,
    postal_code: Option<String>,
    province: Option<String>,
    city: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    geofence_radius_m: Option<f64>,
    contact_name: Option<String>,
    contact_phone: Option<String>,
    contact_email: Option<String>,
    equipment_count: i64,
    rented_count: i64,
    spare_count: i64,
    substitution_active_count: i64,
}

impl From<SiteLocationGroup> for SiteLocationResponse {
    fn from(value: SiteLocationGroup) -> Self {
        Self {
            site_id: value.site_id,
            site_name: value.site_name,
            customer_id: value.customer_id,
            customer_name: value.customer_name,
            branch_id: value.branch_id,
            address: value.address,
            postal_code: value.postal_code,
            province: value.province,
            city: value.city,
            latitude: value.latitude,
            longitude: value.longitude,
            geofence_radius_m: value.geofence_radius_m,
            contact_name: value.contact_name,
            contact_phone: value.contact_phone,
            contact_email: value.contact_email,
            equipment_count: value.equipment_count,
            rented_count: value.rented_count,
            spare_count: value.spare_count,
            substitution_active_count: value.substitution_active_count,
        }
    }
}

/// GET /api/v1/equipment-by-location — every site visible to the principal with
/// its equipment counts and admin-entered coordinates, for the dispatch map.
/// Read access (WorkOrderReadAll, all roles); branch-scoped like the substitute
/// search so a non-SUPER_ADMIN only sees their own branches.
async fn equipment_by_location(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
) -> Result<Json<EquipmentByLocationPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_read_access(&principal)?;

    let items = state
        .store
        .equipment_by_location(EquipmentByLocationQuery {
            branch_scope: principal.branch_scope,
        })
        .await
        .map_err(RestError::from_store)?
        .into_iter()
        .map(SiteLocationResponse::from)
        .collect::<Vec<_>>();
    let total = items.len();

    Ok(Json(EquipmentByLocationPage { items, total }))
}

// ---------------------------------------------------------------------------
// Equipment list — paginated browse (read-access, branch-scoped)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct EquipmentListQueryParams {
    q: Option<String>,
    status: Option<EquipmentStatus>,
    branch_id: Option<BranchId>,
    customer_id: Option<CustomerId>,
    site_id: Option<SiteId>,
    model: Option<String>,
    maker: Option<String>,
    sort: Option<EquipmentSortBy>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Serialize)]
struct EquipmentListResponse {
    items: Vec<EquipmentListItemResponse>,
    total: i64,
    limit: i64,
    offset: i64,
}

#[derive(Debug, Serialize)]
struct EquipmentListItemResponse {
    equipment_id: EquipmentId,
    branch_id: BranchId,
    equipment_no: String,
    management_no: Option<String>,
    status: EquipmentStatus,
    model: Option<String>,
    maker: Option<String>,
    specification: String,
    ton_text: String,
    customer_name: String,
    site_name: String,
    asset_owner: Option<String>,
    vin: Option<String>,
    updated_at: OffsetDateTime,
}

impl From<EquipmentListItem> for EquipmentListItemResponse {
    fn from(item: EquipmentListItem) -> Self {
        Self {
            equipment_id: item.equipment_id,
            branch_id: item.branch_id,
            equipment_no: item.equipment_no,
            management_no: item.management_no,
            status: item.status,
            model: item.model,
            maker: item.maker,
            specification: item.specification,
            ton_text: item.ton_text,
            customer_name: item.customer_name,
            site_name: item.site_name,
            asset_owner: item.asset_owner,
            vin: item.vin,
            updated_at: item.updated_at,
        }
    }
}

/// GET /api/v1/equipment/list — paginated, filterable, branch-scoped equipment
/// list. Read access (WorkOrderReadAll, all authenticated roles). Non-SUPER_ADMIN
/// principals see only rows in their own branch(es) — the same scope guard used
/// by the substitute-search and dispatch-map endpoints. The `q` parameter is
/// normalized like the 호기-lookup: strips a leading '#' and a trailing '호기'
/// suffix then matches leading-zero-insensitively, so the floor-typed '10호기'
/// and the stored '010' resolve to the same row.
async fn list_equipment(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Query(params): Query<EquipmentListQueryParams>,
) -> Result<Json<EquipmentListResponse>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_read_access(&principal)?;

    let page = state
        .store
        .list_equipment(EquipmentListQuery {
            branch_scope: principal.branch_scope,
            q: params.q,
            status: params.status,
            branch_id: params.branch_id,
            customer_id: params.customer_id,
            site_id: params.site_id,
            model: params.model,
            maker: params.maker,
            sort: params.sort.unwrap_or_default(),
            limit: params.limit.unwrap_or(50),
            offset: params.offset.unwrap_or(0),
        })
        .await
        .map_err(RestError::from_store)?;

    Ok(Json(EquipmentListResponse {
        items: page.items.into_iter().map(Into::into).collect(),
        total: page.total,
        limit: page.limit,
        offset: page.offset,
    }))
}

/// GET /api/v1/equipment/{id} — branch-scoped equipment object read. This uses
/// the same read authorization and branch filter as the browse list, but avoids
/// client-side page scanning for direct object links.
async fn get_equipment(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Path(equipment_id): Path<EquipmentId>,
) -> Result<Json<EquipmentListItemResponse>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_read_access(&principal)?;

    let item = state
        .store
        .get_equipment(EquipmentReadQuery {
            branch_scope: principal.branch_scope,
            equipment_id,
        })
        .await
        .map_err(RestError::from_store)?
        .ok_or_else(|| RestError::from_kernel(KernelError::not_found("equipment was not found")))?;

    Ok(Json(item.into()))
}

/// GET /api/v1/equipment/{id}/timeline-graph — branch-scoped lifecycle ribbon
/// and customer/site/equipment/work-order relationship graph for one equipment
/// object. This is a read-only lens: it uses the same read authorization and
/// branch scope as `GET /api/v1/equipment/{id}` and returns 404 for a foreign or
/// missing equipment id without revealing existence.
async fn get_equipment_timeline_graph(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Path(equipment_id): Path<EquipmentId>,
) -> Result<Json<EquipmentTimelineGraph>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_read_access(&principal)?;

    let lens = state
        .store
        .equipment_timeline_graph(EquipmentTimelineGraphQuery {
            branch_scope: principal.branch_scope,
            equipment_id,
        })
        .await
        .map_err(RestError::from_store)?
        .ok_or_else(|| RestError::from_kernel(KernelError::not_found("equipment was not found")))?;

    Ok(Json(lens))
}

// ---------------------------------------------------------------------------
// Customer / site direct creation — audited writes (EquipmentManage)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateCustomerRequest {
    name: String,
}

#[derive(Debug, Serialize)]
struct CreatedCustomerResponse {
    id: CustomerId,
    branch_id: BranchId,
    name: String,
}

impl From<CreatedCustomer> for CreatedCustomerResponse {
    fn from(value: CreatedCustomer) -> Self {
        Self {
            id: value.id,
            branch_id: value.branch_id,
            name: value.name,
        }
    }
}

/// POST /api/v1/customers — create a customer (고객) directly in the caller's org
/// on the caller's branch for branch-scoped admins, or default HQ for org-wide
/// principals. Admin-gated (EquipmentManage), the same feature as the site PATCH.
/// The name is trimmed, required, and bounded; a same-name
/// customer already on the branch is a 409 conflict (an explicit create is a
/// distinct intent from the importer's idempotent upsert, so it is surfaced, not
/// silently merged). Returns the created customer so the console can show it.
async fn create_customer(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateCustomerRequest>,
) -> Result<(StatusCode, Json<CreatedCustomerResponse>), RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_equipment_feature(&principal, Feature::EquipmentManage)?;

    let name = require_bounded_name(body.name, "name")?;
    let customer = state
        .store
        .create_customer(CreateCustomerCommand {
            actor: principal.user_id,
            branch_id: principal_create_branch(&principal),
            name,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(customer.into())))
}

#[derive(Debug, Deserialize)]
struct CreateSiteRequest {
    customer_id: CustomerId,
    name: String,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    province: Option<String>,
    #[serde(default)]
    city: Option<String>,
    #[serde(default)]
    postal_code: Option<String>,
    #[serde(default)]
    latitude: Option<f64>,
    #[serde(default)]
    longitude: Option<f64>,
    #[serde(default)]
    geofence_radius_m: Option<f64>,
    #[serde(default)]
    contact_name: Option<String>,
    #[serde(default)]
    contact_phone: Option<String>,
    #[serde(default)]
    contact_email: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreatedSiteResponse {
    id: SiteId,
    customer_id: CustomerId,
    branch_id: BranchId,
    name: String,
    address: Option<String>,
    province: Option<String>,
    city: Option<String>,
    postal_code: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    geofence_radius_m: Option<f64>,
    contact_name: Option<String>,
    contact_phone: Option<String>,
    contact_email: Option<String>,
}

impl From<CreatedSite> for CreatedSiteResponse {
    fn from(value: CreatedSite) -> Self {
        Self {
            id: value.id,
            customer_id: value.customer_id,
            branch_id: value.branch_id,
            name: value.name,
            address: value.address,
            province: value.province,
            city: value.city,
            postal_code: value.postal_code,
            latitude: value.latitude,
            longitude: value.longitude,
            geofence_radius_m: value.geofence_radius_m,
            contact_name: value.contact_name,
            contact_phone: value.contact_phone,
            contact_email: value.contact_email,
        }
    }
}

/// POST /api/v1/sites — create a site (현장) under an existing customer in the
/// caller's org. Admin-gated (EquipmentManage). The customer must belong to the
/// caller's org: an unknown or foreign-org `customer_id` is a 404 (RLS hides
/// another tenant's customer, so it is never revealed). A customer in another
/// branch of the same org is likewise returned as 404. Name is required and
/// bounded; optional address/coordinate/contact fields are validated to the same
/// WGS84 ranges and length bounds as the site PATCH (a one-sided coordinate or an
/// over-long value is a 422 before the write). A duplicate site name under the
/// same customer is a 409. Returns the created site so it appears in the list/map
/// immediately.
async fn create_site(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateSiteRequest>,
) -> Result<(StatusCode, Json<CreatedSiteResponse>), RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_equipment_feature(&principal, Feature::EquipmentManage)?;

    let name = require_bounded_name(body.name, "name")?;
    // Coordinates: both or neither, each in WGS84 range (mirrors the PATCH path
    // and the registry_sites_lat_lon_paired CHECK).
    validate_coordinate_pair(body.latitude, body.longitude).map_err(RestError::from_kernel)?;
    validate_create_site_geofence_radius(body.geofence_radius_m)?;
    // Bound the optional address/region/contact text to the same limits as the
    // registry_sites CHECKs, so an over-long value is a 422 not a raw DB error.
    for (value, max, field) in [
        (body.address.as_deref(), ADDRESS_MAX_CHARS, "address"),
        (body.province.as_deref(), PROVINCE_MAX_CHARS, "province"),
        (body.city.as_deref(), CITY_MAX_CHARS, "city"),
        (
            body.postal_code.as_deref(),
            POSTAL_CODE_MAX_CHARS,
            "postal_code",
        ),
        (
            body.contact_name.as_deref(),
            CONTACT_NAME_MAX_CHARS,
            "contact_name",
        ),
        (
            body.contact_phone.as_deref(),
            CONTACT_PHONE_MAX_CHARS,
            "contact_phone",
        ),
        (
            body.contact_email.as_deref(),
            CONTACT_EMAIL_MAX_CHARS,
            "contact_email",
        ),
    ] {
        if let Some(text) = value {
            validate_bounded_text(text, max, field).map_err(RestError::from_kernel)?;
        }
    }

    let site = state
        .store
        .create_site(CreateSiteCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope.clone(),
            customer_id: body.customer_id,
            name,
            address: normalize_optional(body.address),
            province: normalize_optional(body.province),
            city: normalize_optional(body.city),
            postal_code: normalize_optional(body.postal_code),
            latitude: body.latitude,
            longitude: body.longitude,
            geofence_radius_m: body.geofence_radius_m,
            contact_name: normalize_optional(body.contact_name),
            contact_phone: normalize_optional(body.contact_phone),
            contact_email: normalize_optional(body.contact_email),
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(site.into())))
}

/// The branch a direct create should land on for this principal. A branch-scoped
/// admin creates on its own (first) branch, so the new row is immediately visible
/// to that admin's branch-scoped registry reads (the by-location list). An
/// org-wide principal (SUPER_ADMIN/EXECUTIVE, `BranchScope::All`) has no single
/// branch and sees every branch anyway, so it returns `None` — the adapter then
/// lands the row on the org's default HQ branch.
fn principal_create_branch(principal: &Principal) -> Option<BranchId> {
    match &principal.branch_scope {
        BranchScope::All => None,
        // fabricated-branch: ok selects the target for row CREATION; never authorizes
        BranchScope::Branches(branches) => branches.iter().next().copied(),
    }
}

/// Bound a required customer/site name: trim, reject empty (mirrors the
/// `name <> ''` CHECK), and cap the length (mirrors the migration-0047 CHECK and
/// the `CUSTOMER_SITE_NAME_MAX_CHARS` domain bound) so an over-long name is a 422.
fn require_bounded_name(value: String, field: &str) -> Result<String, RestError> {
    let trimmed = require_nonempty(value, field)?;
    validate_bounded_text(&trimmed, CUSTOMER_SITE_NAME_MAX_CHARS, field)
        .map_err(RestError::from_kernel)?;
    Ok(trimmed)
}

/// Trim an optional free-text field, collapsing an empty/whitespace-only string to
/// `None` so the row stores NULL rather than "" (matching the PATCH path, which
/// treats a blank field as "no value").
fn normalize_optional(value: Option<String>) -> Option<String> {
    value.and_then(|text| {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    })
}

/// Bound the optional geofence radius on a site create to the same range as the
/// `registry_sites_geofence_radius_positive` CHECK (migration 0041): a present
/// value must be finite, > 0, and ≤ 100 000 m. Absent needs no check.
fn validate_create_site_geofence_radius(radius: Option<f64>) -> Result<(), RestError> {
    if let Some(radius) = radius
        && (!radius.is_finite() || radius <= 0.0 || radius > 100_000.0)
    {
        return Err(RestError::from_kernel(KernelError::validation(
            "geofence_radius_m must be greater than 0 and at most 100000 metres",
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Site coordinate/address update — audited write (EquipmentManage)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UpdateSiteRequest {
    #[serde(default, deserialize_with = "double_option")]
    address: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    province: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    city: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    postal_code: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    latitude: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    longitude: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    geofence_radius_m: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    contact_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    contact_phone: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    contact_email: Option<Option<String>>,
}

/// PATCH /api/v1/sites/{id} — the ONLY coordinate entry point. Admin-gated
/// (EquipmentManage); the lat/lon ranges and pairing are validated in the
/// kernel before the audited write opens a transaction, so a bad value is a 422
/// rather than a DB error. No geocoding service: coordinates exist only because
/// an admin typed them here.
async fn update_site(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Path(site_id): Path<SiteId>,
    Json(body): Json<UpdateSiteRequest>,
) -> Result<StatusCode, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_equipment_feature(&principal, Feature::EquipmentManage)?;

    // Validate the supplied coordinate edits in the domain. A present-but-null
    // pair (clearing both) is allowed; a one-sided present value is rejected to
    // mirror the registry_sites_lat_lon_paired CHECK.
    validate_site_coordinates(&body)?;
    validate_site_contact(&body)?;
    validate_site_address_fields(&body)?;
    validate_site_geofence_radius(&body)?;

    let fields = UpdateSiteFields {
        address: body.address,
        province: body.province,
        city: body.city,
        postal_code: body.postal_code,
        latitude: body.latitude,
        longitude: body.longitude,
        geofence_radius_m: body.geofence_radius_m,
        contact_name: body.contact_name,
        contact_phone: body.contact_phone,
        contact_email: body.contact_email,
    };
    if fields.is_empty() {
        return Err(RestError::from_kernel(KernelError::validation(
            "no site fields to update",
        )));
    }
    state
        .store
        .update_site(UpdateSiteCommand {
            actor: principal.user_id,
            site_id,
            fields,
            branch_scope: principal.branch_scope.clone(),
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Validate the coordinate edits on a site PATCH. Only validates when at least
/// one coordinate field is present in the request; when neither key is supplied
/// the existing stored pair is untouched and needs no check here.
fn validate_site_coordinates(body: &UpdateSiteRequest) -> Result<(), RestError> {
    match (&body.latitude, &body.longitude) {
        // Neither coordinate field present — nothing to validate.
        (None, None) => Ok(()),
        // Both present: must be jointly set or jointly cleared, and in range.
        (Some(lat), Some(lon)) => {
            validate_coordinate_pair(*lat, *lon).map_err(RestError::from_kernel)
        }
        // Exactly one coordinate key present: a half-update would leave the row
        // with one coordinate set and the other stale, breaking the pairing
        // invariant. Reject it explicitly.
        _ => Err(RestError::from_kernel(KernelError::validation(
            "latitude and longitude must be updated together",
        ))),
    }
}

/// Bound the optional representative-contact text fields on a site PATCH to the
/// same limits as the `registry_sites` contact CHECKs (migration 0040), so an
/// over-long value is rejected with a 422 rather than surfacing as a raw DB CHECK
/// error. Only present (`Some(Some(_))`) values are checked; absent or
/// explicit-null fields need no bound.
fn validate_site_contact(body: &UpdateSiteRequest) -> Result<(), RestError> {
    for (change, max, field) in [
        (&body.contact_name, CONTACT_NAME_MAX_CHARS, "contact_name"),
        (
            &body.contact_phone,
            CONTACT_PHONE_MAX_CHARS,
            "contact_phone",
        ),
        (
            &body.contact_email,
            CONTACT_EMAIL_MAX_CHARS,
            "contact_email",
        ),
    ] {
        if let Some(Some(text)) = change {
            validate_bounded_text(text, max, field).map_err(RestError::from_kernel)?;
        }
    }
    Ok(())
}

/// Bound the optional address/region fields on a site PATCH to the same lengths
/// as the migration 0039 CHECKs, returning a 422 before the write rather than
/// surfacing an over-long value as a raw 500 DB CHECK error. Absent and
/// explicit-null fields need no bound.
fn validate_site_address_fields(body: &UpdateSiteRequest) -> Result<(), RestError> {
    for (change, max, field) in [
        (&body.address, ADDRESS_MAX_CHARS, "address"),
        (&body.province, PROVINCE_MAX_CHARS, "province"),
        (&body.city, CITY_MAX_CHARS, "city"),
        (&body.postal_code, POSTAL_CODE_MAX_CHARS, "postal_code"),
    ] {
        if let Some(Some(text)) = change {
            validate_bounded_text(text, max, field).map_err(RestError::from_kernel)?;
        }
    }
    Ok(())
}

/// Bound the optional per-site geofence radius on a site PATCH to the same range
/// as the `registry_sites_geofence_radius_positive` CHECK (migration 0041): a
/// present value must be finite, > 0, and ≤ 100 000 m. An explicit-null (clear →
/// fall back to the system default) or an absent field needs no check.
fn validate_site_geofence_radius(body: &UpdateSiteRequest) -> Result<(), RestError> {
    if let Some(Some(radius)) = body.geofence_radius_m
        && (!radius.is_finite() || radius <= 0.0 || radius > 100_000.0)
    {
        return Err(RestError::from_kernel(KernelError::validation(
            "geofence_radius_m must be greater than 0 and at most 100000 metres",
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Substitute (대차) assignment / return — audited equipment-lifecycle writes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AssignSubstituteRequest {
    source_equipment_id: EquipmentId,
    substitute_equipment_id: EquipmentId,
    #[serde(default)]
    assigned_to: Option<UserId>,
    assignment_location: String,
}

#[derive(Debug, Deserialize)]
struct ReturnSubstituteRequest {
    #[serde(default)]
    return_note: Option<String>,
}

#[derive(Debug, Serialize)]
struct SubstituteAssignmentResponse {
    id: EquipmentSubstitutionId,
    branch_id: BranchId,
    source_equipment_id: EquipmentId,
    substitute_equipment_id: EquipmentId,
    assigned_by: UserId,
    assigned_to: Option<UserId>,
    assignment_location: String,
    assigned_at: OffsetDateTime,
    returned_by: Option<UserId>,
    returned_at: Option<OffsetDateTime>,
    return_note: Option<String>,
}

impl From<SubstituteAssignment> for SubstituteAssignmentResponse {
    fn from(value: SubstituteAssignment) -> Self {
        Self {
            id: value.id,
            branch_id: value.branch_id,
            source_equipment_id: value.source_equipment_id,
            substitute_equipment_id: value.substitute_equipment_id,
            assigned_by: value.assigned_by,
            assigned_to: value.assigned_to,
            assignment_location: value.assignment_location,
            assigned_at: value.assigned_at,
            returned_by: value.returned_by,
            returned_at: value.returned_at,
            return_note: value.return_note,
        }
    }
}

async fn assign_equipment_substitute(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Json(body): Json<AssignSubstituteRequest>,
) -> Result<(StatusCode, Json<SubstituteAssignmentResponse>), RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_equipment_feature(&principal, Feature::EquipmentManage)?;

    let command = SubstituteAssignmentCommand {
        actor: principal.user_id,
        branch_scope: principal.branch_scope.clone(),
        source_equipment_id: body.source_equipment_id,
        substitute_equipment_id: body.substitute_equipment_id,
        assigned_to: body.assigned_to,
        assignment_location: require_nonempty(body.assignment_location, "assignment_location")?,
        trace: TraceContext::generate(),
        assigned_at: OffsetDateTime::now_utc(),
    };
    let assignment = state
        .store
        .assign_substitute(command)
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(assignment.into())))
}

async fn return_equipment_substitute(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Path(substitution_id): Path<EquipmentSubstitutionId>,
    Json(body): Json<ReturnSubstituteRequest>,
) -> Result<Json<SubstituteAssignmentResponse>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_equipment_feature(&principal, Feature::EquipmentManage)?;

    let command = SubstituteReturnCommand {
        actor: principal.user_id,
        branch_scope: principal.branch_scope.clone(),
        substitution_id,
        trace: TraceContext::generate(),
        returned_at: OffsetDateTime::now_utc(),
        return_note: body.return_note,
    };
    let assignment = state
        .store
        .return_substitute(command)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(assignment.into()))
}

// ---------------------------------------------------------------------------
// Equipment legal ownership transfer — request + ordered signoff lifecycle
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateOwnershipTransferRequest {
    to_owner: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct DecideOwnershipTransferRequest {
    decision: EquipmentOwnershipTransferDecision,
    comment: String,
}

#[derive(Debug, Serialize)]
struct OwnershipTransferPage {
    items: Vec<OwnershipTransferResponse>,
}

#[derive(Debug, Serialize)]
struct OwnershipTransferResponse {
    id: Uuid,
    equipment_id: EquipmentId,
    branch_id: BranchId,
    from_owner: String,
    to_owner: String,
    reason: String,
    status: EquipmentOwnershipTransferStatus,
    current_step: Option<EquipmentOwnershipTransferStepKey>,
    approval_line: Value,
    requested_by: Option<UserId>,
    requested_at: OffsetDateTime,
    decided_at: Option<OffsetDateTime>,
    completed_at: Option<OffsetDateTime>,
}

impl TryFrom<EquipmentOwnershipTransferRequest> for OwnershipTransferResponse {
    type Error = RestError;

    fn try_from(value: EquipmentOwnershipTransferRequest) -> Result<Self, Self::Error> {
        let approval_line = serde_json::to_value(&value.approval_line)
            .map_err(|err| RestError::internal(format!("invalid approval line: {err}")))?;
        Ok(Self {
            id: value.id,
            equipment_id: value.equipment_id,
            branch_id: value.branch_id,
            from_owner: value.from_owner,
            to_owner: value.to_owner,
            reason: value.reason,
            status: value.status,
            current_step: value.current_step,
            approval_line,
            requested_by: value.requested_by,
            requested_at: value.requested_at,
            decided_at: value.decided_at,
            completed_at: value.completed_at,
        })
    }
}

async fn list_equipment_ownership_transfers(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Path(equipment_id): Path<EquipmentId>,
) -> Result<Json<OwnershipTransferPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_equipment_feature(&principal, Feature::EquipmentManage)?;

    let items = state
        .store
        .list_equipment_ownership_transfers(equipment_id, &principal.branch_scope)
        .await
        .map_err(RestError::from_store)?
        .into_iter()
        .map(OwnershipTransferResponse::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(OwnershipTransferPage { items }))
}

async fn create_equipment_ownership_transfer(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Path(equipment_id): Path<EquipmentId>,
    Json(body): Json<CreateOwnershipTransferRequest>,
) -> Result<(StatusCode, Json<OwnershipTransferResponse>), RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_equipment_feature(&principal, Feature::EquipmentManage)?;

    let command = CreateEquipmentOwnershipTransferCommand {
        actor: principal.user_id,
        branch_scope: principal.branch_scope.clone(),
        equipment_id,
        to_owner: require_nonempty(body.to_owner, "to_owner")?,
        reason: require_nonempty(body.reason, "reason")?,
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    };
    let request = state
        .store
        .create_equipment_ownership_transfer(command)
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(request.try_into()?)))
}

async fn decide_equipment_ownership_transfer(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Path(request_id): Path<Uuid>,
    Json(body): Json<DecideOwnershipTransferRequest>,
) -> Result<Json<OwnershipTransferResponse>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    // Decision execution is still gated by EquipmentManage; the request itself
    // carries the ordered sending-org, receiving-org, legal, and accounting
    // steps. Future custom policy can split those step permissions without
    // changing the immutable workflow/event contract.
    authorize_equipment_feature(&principal, Feature::EquipmentManage)?;

    let command = DecideEquipmentOwnershipTransferCommand {
        actor: principal.user_id,
        branch_scope: principal.branch_scope.clone(),
        request_id,
        decision: body.decision,
        comment: require_nonempty(body.comment, "comment")?,
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    };
    let request = state
        .store
        .decide_equipment_ownership_transfer(command)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(request.try_into()?))
}

// ---------------------------------------------------------------------------
// Equipment master import (org-wide authority; SUPER_ADMIN for built-in roles)
// ---------------------------------------------------------------------------

async fn import_master_list(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<Json<RegistryImportReport>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    // The importer always targets the tenant's default HQ branch and may update
    // every matching equipment number there. It carries no branch scope, so a
    // branch-scoped ADMIN must not authorize it through a representative branch.
    authorize_master_list_import(&principal)?;

    let upload = read_xlsx_upload(multipart).await?;
    let report = state
        .store
        .import_master_list_bytes(principal.user_id, &upload.filename, &upload.bytes)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(report))
}

struct XlsxUpload {
    filename: String,
    bytes: Vec<u8>,
}

async fn read_xlsx_upload(mut multipart: Multipart) -> Result<XlsxUpload, RestError> {
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|err| RestError::from_kernel(KernelError::validation(err.to_string())))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field
            .file_name()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "master-list.xlsx".to_owned());
        // Stream the field chunk-by-chunk and abort the moment the running total
        // exceeds MAX_IMPORT_BYTES. `Field::bytes()` would buffer the entire
        // upload into memory *before* any size check, so a malicious or
        // misconfigured client could exhaust the worker's heap before the cap
        // fired — the check has to bound the read incrementally, not after.
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|err| RestError::from_kernel(KernelError::validation(err.to_string())))?
        {
            if bytes.len() + chunk.len() > MAX_IMPORT_BYTES {
                return Err(RestError::from_kernel(KernelError::validation(
                    "uploaded file exceeds the maximum import size",
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(RestError::from_kernel(KernelError::validation(
                "uploaded file is empty",
            )));
        }
        return Ok(XlsxUpload { filename, bytes });
    }
    Err(RestError::from_kernel(KernelError::validation(
        "multipart upload is missing the 'file' field",
    )))
}

// ---------------------------------------------------------------------------
// Object actions (CAP-5 governed write-back)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ObjectActionCatalogQueryParams {
    object_type: String,
    object_id: EquipmentId,
}

#[derive(Debug, Serialize)]
struct ObjectActionCatalogResponse {
    object_type: String,
    object_id: String,
    actions: Vec<ObjectActionDescriptor>,
}

#[derive(Debug, Serialize)]
struct ObjectActionDescriptor {
    action_id: String,
    object_type: String,
    object_id: String,
    label: String,
    description: String,
    submit_label: String,
    requires_passkey_step_up: bool,
    risk_level: String,
    fields: Vec<ObjectActionFieldDescriptor>,
}

#[derive(Debug, Serialize)]
struct ObjectActionFieldDescriptor {
    field_key: String,
    label: String,
    field_type: String,
    required: bool,
    current_value: Option<String>,
    options: Vec<ObjectActionFieldOption>,
}

#[derive(Debug, Serialize)]
struct ObjectActionFieldOption {
    value: String,
    label: String,
}

#[derive(Debug, Deserialize)]
struct ExecuteObjectActionRequest {
    action_id: String,
    object_type: String,
    object_id: EquipmentId,
    input: Value,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    step_up: Option<ObjectActionStepUpAssertionRequest>,
}

#[derive(Debug, Deserialize)]
struct ObjectActionStepUpAssertionRequest {
    ceremony_id: Uuid,
    credential: PasskeyAuthenticationCredential,
}

#[derive(Debug, Serialize)]
struct ObjectActionExecutionResponse {
    execution_id: Uuid,
    action_id: String,
    object_type: String,
    object_id: String,
    status: String,
    audit_event_id: AuditEventId,
    target_href: String,
    message: String,
}

async fn get_object_action_catalog(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Query(query): Query<ObjectActionCatalogQueryParams>,
) -> Result<Json<ObjectActionCatalogResponse>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_equipment_feature(&principal, Feature::EquipmentManage)?;
    let object_type = normalize_object_type(&query.object_type)?;
    let equipment = state
        .store
        .get_equipment(EquipmentReadQuery {
            branch_scope: principal.branch_scope.clone(),
            equipment_id: query.object_id,
        })
        .await
        .map_err(RestError::from_store)?
        .ok_or_else(|| RestError::from_kernel(KernelError::not_found("equipment was not found")))?;

    Ok(Json(ObjectActionCatalogResponse {
        object_type: object_type.to_owned(),
        object_id: equipment.equipment_id.to_string(),
        actions: vec![equipment_update_profile_action(&equipment)],
    }))
}

async fn execute_object_action(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Json(body): Json<ExecuteObjectActionRequest>,
) -> Result<Json<ObjectActionExecutionResponse>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let action_id = object_action_metric_id(&body.action_id);
    let result = execute_object_action_inner(&state, &principal, body).await;
    match &result {
        Ok(_) => record_object_action_execution(EQUIPMENT_UPDATE_PROFILE_ACTION_ID, "succeeded"),
        Err(error) => {
            record_object_action_execution(action_id, "rejected");
            tracing::warn!(
                event = "object_action_execution_rejected",
                action_id,
                error_code = error.code,
                actor_user_id = %principal.user_id,
                "object action execution rejected"
            );
        }
    }
    result.map(Json)
}

async fn execute_object_action_inner(
    state: &RegistryRestState,
    principal: &Principal,
    body: ExecuteObjectActionRequest,
) -> Result<ObjectActionExecutionResponse, RestError> {
    let object_type = normalize_object_type(&body.object_type)?;
    if body.action_id != EQUIPMENT_UPDATE_PROFILE_ACTION_ID {
        return Err(RestError::validation("unknown object action"));
    }
    if body
        .idempotency_key
        .as_deref()
        .is_some_and(|key| key.len() > 128)
    {
        return Err(RestError::validation(
            "idempotency_key must be 128 characters or fewer",
        ));
    }
    authorize_equipment_feature(principal, Feature::EquipmentManage)?;
    verify_object_action_step_up(state, principal, body.step_up).await?;

    let input: UpdateEquipmentRequest = serde_json::from_value(body.input)
        .map_err(|_| RestError::validation("input does not match the equipment action schema"))?;
    let fields = equipment_fields_from_request(input)?;
    let trace = TraceContext::generate();
    let audit_event_id = state
        .store
        .update_equipment(UpdateEquipmentCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope.clone(),
            equipment_id: body.object_id,
            fields,
            trace,
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;

    let equipment_id = body.object_id.to_string();
    Ok(ObjectActionExecutionResponse {
        execution_id: Uuid::new_v4(),
        action_id: EQUIPMENT_UPDATE_PROFILE_ACTION_ID.to_owned(),
        object_type: object_type.to_owned(),
        object_id: equipment_id.clone(),
        status: "succeeded".to_owned(),
        audit_event_id,
        target_href: format!("/equipment/{equipment_id}"),
        message: "장비 정보가 감사 로그와 함께 업데이트되었습니다.".to_owned(),
    })
}

async fn verify_object_action_step_up(
    state: &RegistryRestState,
    principal: &Principal,
    step_up: Option<ObjectActionStepUpAssertionRequest>,
) -> Result<(), RestError> {
    let step_up = step_up.ok_or_else(|| {
        RestError::new(
            StatusCode::PRECONDITION_REQUIRED,
            "passkey_step_up_required",
            "object action execution requires a fresh passkey step-up",
        )
    })?;
    let verifier = state.passkey_step_up.as_ref().ok_or_else(|| {
        RestError::unavailable("passkey step-up is not configured for registry API")
    })?;
    verifier
        .verify_step_up_for_user(
            state.store.pool(),
            step_up.ceremony_id,
            step_up.credential,
            *principal.user_id.as_uuid(),
        )
        .await
        .map_err(|_| RestError::unauthorized("passkey step-up failed"))?;
    Ok(())
}

fn normalize_object_type(raw: &str) -> Result<&'static str, RestError> {
    match raw.trim() {
        "equipment" => Ok("equipment"),
        _ => Err(RestError::validation("object_type must be equipment")),
    }
}

fn object_action_metric_id(raw: &str) -> &'static str {
    match raw {
        EQUIPMENT_UPDATE_PROFILE_ACTION_ID => EQUIPMENT_UPDATE_PROFILE_ACTION_ID,
        _ => "unknown",
    }
}

fn record_object_action_execution(action_id: &'static str, outcome: &'static str) {
    metrics::counter!(
        OBJECT_ACTION_EXECUTION_TOTAL,
        "action_id" => action_id,
        "outcome" => outcome,
    )
    .increment(1);
}

fn equipment_update_profile_action(item: &EquipmentListItem) -> ObjectActionDescriptor {
    ObjectActionDescriptor {
        action_id: EQUIPMENT_UPDATE_PROFILE_ACTION_ID.to_owned(),
        object_type: "equipment".to_owned(),
        object_id: item.equipment_id.to_string(),
        label: "장비 정보 수정".to_owned(),
        description:
            "고객, 현장, 상태, 모델, 규격 같은 장비 마스터 정보를 감사 로그와 함께 수정합니다."
                .to_owned(),
        submit_label: "패스키로 수정 실행".to_owned(),
        requires_passkey_step_up: true,
        risk_level: "sensitive_write".to_owned(),
        fields: vec![
            ObjectActionFieldDescriptor {
                field_key: "status".to_owned(),
                label: "상태".to_owned(),
                field_type: "select".to_owned(),
                required: false,
                current_value: Some(equipment_status_wire_value(item.status).to_owned()),
                options: equipment_status_action_options(),
            },
            text_action_field("customer_name", "고객명", Some(&item.customer_name)),
            text_action_field("site_name", "현장명", Some(&item.site_name)),
            text_action_field("management_no", "관리번호", item.management_no.as_deref()),
            text_action_field("model", "모델", item.model.as_deref()),
            text_action_field("maker", "제조사", item.maker.as_deref()),
            text_action_field("specification", "규격", Some(&item.specification)),
            text_action_field("ton_text", "톤수", Some(&item.ton_text)),
            text_action_field("asset_owner", "법적 소유자", item.asset_owner.as_deref()),
        ],
    }
}

fn text_action_field(
    field_key: &str,
    label: &str,
    current_value: Option<&str>,
) -> ObjectActionFieldDescriptor {
    ObjectActionFieldDescriptor {
        field_key: field_key.to_owned(),
        label: label.to_owned(),
        field_type: "text".to_owned(),
        required: false,
        current_value: current_value.map(ToOwned::to_owned),
        options: vec![],
    }
}

fn equipment_status_action_options() -> Vec<ObjectActionFieldOption> {
    [
        (EquipmentStatus::Rented, "임대"),
        (EquipmentStatus::Spare, "예비"),
        (EquipmentStatus::Disposed, "폐기"),
        (EquipmentStatus::Replacement, "대차"),
        (EquipmentStatus::Sold, "매각"),
    ]
    .into_iter()
    .map(|(status, label)| ObjectActionFieldOption {
        value: equipment_status_wire_value(status).to_owned(),
        label: label.to_owned(),
    })
    .collect()
}

fn equipment_status_wire_value(status: EquipmentStatus) -> &'static str {
    match status {
        EquipmentStatus::Rented => "rented",
        EquipmentStatus::Spare => "spare",
        EquipmentStatus::Disposed => "disposed",
        EquipmentStatus::Replacement => "replacement",
        EquipmentStatus::Sold => "sold",
    }
}

// ---------------------------------------------------------------------------
// Equipment CRUD (admin-gated)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateEquipmentRequest {
    equipment_no: String,
    customer_name: String,
    site_name: String,
    status: EquipmentStatus,
    specification: String,
    ton_text: String,
    #[serde(default)]
    management_no: Option<String>,
    #[serde(default)]
    power_label: Option<String>,
    #[serde(default)]
    manager_name: Option<String>,
    #[serde(default)]
    placement_location: Option<String>,
    #[serde(default)]
    placement_no: Option<String>,
    #[serde(default)]
    operation_shift: Option<String>,
    #[serde(default)]
    maker: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    vin: Option<String>,
    #[serde(default)]
    year: Option<Date>,
    #[serde(default)]
    hours: Option<i64>,
    #[serde(default)]
    vehicle_registration_no: Option<String>,
    #[serde(default)]
    insured: Option<bool>,
    #[serde(default)]
    insurer: Option<String>,
    #[serde(default)]
    policy_holder: Option<String>,
    #[serde(default)]
    insured_party: Option<String>,
    #[serde(default)]
    asset_owner: Option<String>,
    #[serde(default)]
    asset_registered_on: Option<Date>,
    #[serde(default)]
    rental_started_on: Option<Date>,
    #[serde(default)]
    rental_fee: Option<i64>,
    #[serde(default)]
    vehicle_value: Option<i64>,
    #[serde(default)]
    residual_value: Option<i64>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateEquipmentResponse {
    id: EquipmentId,
}

async fn create_equipment(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateEquipmentRequest>,
) -> Result<(StatusCode, Json<CreateEquipmentResponse>), RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_equipment_feature(&principal, Feature::EquipmentManage)?;

    let equipment_no = EquipmentNo::parse(body.equipment_no).map_err(RestError::from_kernel)?;
    let command = CreateEquipmentCommand {
        actor: principal.user_id,
        branch_id: principal_create_branch(&principal),
        equipment_no,
        customer_name: require_nonempty(body.customer_name, "customer_name")?,
        site_name: require_nonempty(body.site_name, "site_name")?,
        status: body.status,
        specification: require_nonempty(body.specification, "specification")?,
        ton: Ton::parse(&body.ton_text),
        management_no: body.management_no,
        power_label: body.power_label,
        manager_name: body.manager_name,
        placement_location: body.placement_location,
        placement_no: body.placement_no,
        operation_shift: body.operation_shift,
        maker: body.maker,
        model: body.model,
        vin: body.vin,
        year: body.year,
        hours: body.hours,
        vehicle_registration_no: body.vehicle_registration_no,
        insured: body.insured,
        insurer: body.insurer,
        policy_holder: body.policy_holder,
        insured_party: body.insured_party,
        asset_owner: body.asset_owner,
        asset_registered_on: body.asset_registered_on,
        rental_started_on: body.rental_started_on,
        rental_fee: body.rental_fee.map(MoneyWon::new),
        vehicle_value: body.vehicle_value.map(MoneyWon::new),
        residual_value: body.residual_value.map(MoneyWon::new),
        note: body.note,
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    };
    let id = state
        .store
        .create_equipment(command)
        .await
        .map_err(RestError::from_store)?;
    Ok((StatusCode::CREATED, Json(CreateEquipmentResponse { id })))
}

#[derive(Debug, Deserialize)]
struct UpdateEquipmentRequest {
    #[serde(default)]
    customer_name: Option<String>,
    #[serde(default)]
    site_name: Option<String>,
    #[serde(default)]
    status: Option<EquipmentStatus>,
    #[serde(default)]
    specification: Option<String>,
    #[serde(default)]
    ton_text: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    management_no: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    power_label: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    manager_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    placement_location: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    placement_no: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    operation_shift: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    maker: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    model: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    vin: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    year: Option<Option<Date>>,
    #[serde(default, deserialize_with = "double_option")]
    hours: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    vehicle_registration_no: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    insured: Option<Option<bool>>,
    #[serde(default, deserialize_with = "double_option")]
    insurer: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    policy_holder: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    insured_party: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    asset_owner: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    asset_registered_on: Option<Option<Date>>,
    #[serde(default, deserialize_with = "double_option")]
    rental_started_on: Option<Option<Date>>,
    #[serde(default, deserialize_with = "double_option")]
    rental_fee: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    vehicle_value: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    residual_value: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    acquisition_cost_won: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option_iso_date")]
    acquisition_date: Option<Option<Date>>,
    #[serde(default, deserialize_with = "double_option")]
    note: Option<Option<String>>,
}

/// Distinguish "key absent" (leave unchanged) from "key present but null"
/// (clear the column) for nullable PATCH fields.
fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

/// `double_option` for an ISO-8601 calendar date (`"YYYY-MM-DD"`): "key absent"
/// leaves the column unchanged, an explicit `null` clears it, and a date string
/// sets it. Uses the `format: date` wire contract (an honest string), matching
/// the regenerated OpenAPI/typed client.
fn double_option_iso_date<'de, D>(deserializer: D) -> Result<Option<Option<Date>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<String> = Deserialize::deserialize(deserializer)?;
    match raw {
        Some(value) => Date::parse(&value, &time::format_description::well_known::Iso8601::DATE)
            .map(|date| Some(Some(date)))
            .map_err(serde::de::Error::custom),
        None => Ok(Some(None)),
    }
}

async fn update_equipment(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Path(equipment_id): Path<EquipmentId>,
    Json(body): Json<UpdateEquipmentRequest>,
) -> Result<StatusCode, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_equipment_feature(&principal, Feature::EquipmentManage)?;

    let fields = equipment_fields_from_request(body)?;
    state
        .store
        .update_equipment(UpdateEquipmentCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope.clone(),
            equipment_id,
            fields,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EquipmentVersionResponse {
    version: i32,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_version: Option<i32>,
    content: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct EquipmentVersionListResponse {
    items: Vec<EquipmentVersionResponse>,
}

/// GET /api/v1/equipment/{id}/versions — append-only version history, newest
/// first. Read tier: same as the equipment detail read.
async fn list_equipment_versions(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Path(equipment_id): Path<EquipmentId>,
) -> Result<Json<EquipmentVersionListResponse>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_read_access(&principal)?;

    let versions = state
        .store
        .list_equipment_versions(equipment_id, &principal.branch_scope)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(EquipmentVersionListResponse {
        items: versions
            .into_iter()
            .map(|record| EquipmentVersionResponse {
                version: record.version,
                status: record.status,
                source_version: record.source_version,
                content: record.content,
                created_by: record.created_by,
                created_at: record.created_at,
            })
            .collect(),
    }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EquipmentRollbackResponse {
    version: i32,
}

/// POST /api/v1/equipment/{id}/versions/{version}/rollback — re-applies the
/// target version's content as a NEW version; history is never mutated. Gated
/// like the equipment update it is.
async fn rollback_equipment(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Path((equipment_id, version)): Path<(EquipmentId, i32)>,
) -> Result<(StatusCode, Json<EquipmentRollbackResponse>), RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_equipment_feature(&principal, Feature::EquipmentManage)?;

    let new_version = state
        .store
        .rollback_equipment(RollbackEquipmentCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope.clone(),
            equipment_id,
            version,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok((
        StatusCode::CREATED,
        Json(EquipmentRollbackResponse {
            version: new_version,
        }),
    ))
}

fn equipment_fields_from_request(
    body: UpdateEquipmentRequest,
) -> Result<UpdateEquipmentFields, RestError> {
    let fields = UpdateEquipmentFields {
        customer_name: body.customer_name,
        site_name: body.site_name,
        status: body.status,
        specification: body.specification,
        ton: body.ton_text.as_deref().map(Ton::parse),
        management_no: body.management_no,
        power_label: body.power_label,
        manager_name: body.manager_name,
        placement_location: body.placement_location,
        placement_no: body.placement_no,
        operation_shift: body.operation_shift,
        maker: body.maker,
        model: body.model,
        vin: body.vin,
        year: body.year,
        hours: body.hours,
        vehicle_registration_no: body.vehicle_registration_no,
        insured: body.insured,
        insurer: body.insurer,
        policy_holder: body.policy_holder,
        insured_party: body.insured_party,
        asset_owner: body.asset_owner,
        asset_registered_on: body.asset_registered_on,
        rental_started_on: body.rental_started_on,
        rental_fee: body.rental_fee.map(|value| value.map(MoneyWon::new)),
        vehicle_value: body.vehicle_value.map(|value| value.map(MoneyWon::new)),
        residual_value: body.residual_value.map(|value| value.map(MoneyWon::new)),
        acquisition_cost_won: body
            .acquisition_cost_won
            .map(|value| value.map(MoneyWon::new)),
        acquisition_date: body.acquisition_date,
        note: body.note,
    };
    if fields.is_empty() {
        return Err(RestError::from_kernel(KernelError::validation(
            "no equipment fields to update",
        )));
    }
    Ok(fields)
}

async fn delete_equipment(
    State(state): State<RegistryRestState>,
    headers: HeaderMap,
    Path(equipment_id): Path<EquipmentId>,
) -> Result<StatusCode, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_equipment_feature(&principal, Feature::EquipmentManage)?;

    state
        .store
        .soft_delete_equipment(DeleteEquipmentCommand {
            actor: principal.user_id,
            branch_scope: principal.branch_scope.clone(),
            equipment_id,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(StatusCode::NO_CONTENT)
}

fn require_nonempty(value: String, field: &str) -> Result<String, RestError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(RestError::from_kernel(KernelError::validation(format!(
            "{field} must not be empty"
        ))))
    } else {
        Ok(trimmed.to_owned())
    }
}

/// Feature preflight for registry operations whose store command either carries
/// the principal's complete branch scope or creates a row in a branch selected
/// from that scope. It deliberately does not invent a resource branch. The one
/// tenant-wide operation, master-list import, uses `authorize_org_wide` directly.
fn authorize_equipment_feature(principal: &Principal, feature: Feature) -> Result<(), RestError> {
    authorize_capability(principal, Action::new(feature)).map_err(RestError::from_kernel)
}

fn authorize_master_list_import(principal: &Principal) -> Result<(), RestError> {
    authorize_org_wide(principal, Action::new(Feature::MasterListImport))
        .map_err(RestError::from_kernel)
}

async fn principal_from_headers(
    state: &RegistryRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for registry API")
    })?;
    console_platform_request_context::resolve_principal(verifier, state.store.pool(), headers)
        .await
        .map_err(rest_error_from_request_context)
}

fn rest_error_from_request_context(
    err: console_platform_request_context::RequestContextError,
) -> RestError {
    match err {
        console_platform_request_context::RequestContextError::VerifierUnavailable => {
            RestError::unavailable("JWT verification is not configured for registry API")
        }
        console_platform_request_context::RequestContextError::WrongTokenTier => {
            RestError::forbidden("token tier is not valid for this route")
        }
        console_platform_request_context::RequestContextError::AccessScope(error) => {
            RestError::from_kernel(error)
        }
        console_platform_request_context::RequestContextError::BranchScope(message)
        | console_platform_request_context::RequestContextError::EffectivePolicy(message) => {
            RestError::internal(message)
        }
        console_platform_request_context::RequestContextError::MissingOrg => {
            RestError::internal("no tenant context is bound to the current request")
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

/// Feature-tier gate for registry reads. Every caller also passes the principal's
/// complete branch scope to the store, which filters lists and resolves a
/// by-primary-key row as 404 outside that scope.
fn authorize_read_access(principal: &Principal) -> Result<(), RestError> {
    authorize_capability(principal, Action::new(Feature::WorkOrderReadAll))
        .map_err(RestError::from_kernel)
}

fn is_super_admin(principal: &Principal) -> bool {
    principal.roles.contains(&Role::SuperAdmin)
}

#[derive(Debug)]
struct RestError {
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

    fn from_store(error: PgRegistryError) -> Self {
        match error {
            PgRegistryError::Domain(error) => Self::from_kernel(error),
            // A DB or workbook failure becomes an opaque 500 for the client, but
            // the cause MUST be in the logs: in production the import 500 carried
            // no root cause (only tower_http's generic "response failed"), which
            // hid an RLS WITH CHECK rejection for ~two uploads. Log the error
            // Display server-side (the cause, e.g. the Postgres error code/table)
            // before mapping. The message is the DB/IO error text, never raw PII
            // (the pii-no-logs gate also scans this), so no secret/PII leaks.
            PgRegistryError::Db(err) => {
                tracing::error!(error = %err, "registry database operation failed");
                Self::internal("registry request failed")
            }
            PgRegistryError::Workbook(err) => {
                tracing::error!(error = %err, "registry workbook import failed");
                Self::internal("registry request failed")
            }
        }
    }

    fn from_kernel(error: KernelError) -> Self {
        let status = match error.kind {
            ErrorKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorKind::NotFound => StatusCode::NOT_FOUND,
            ErrorKind::Forbidden => StatusCode::FORBIDDEN,
            ErrorKind::Conflict | ErrorKind::InvalidTransition => StatusCode::CONFLICT,
            ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            code: error_code(error.kind),
            message: error.message,
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: message.into(),
        }
    }

    fn validation(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "validation",
            message: message.into(),
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "forbidden",
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "unavailable",
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: message.into(),
        }
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

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
}

fn error_code(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Validation => "validation",
        ErrorKind::NotFound => "not_found",
        ErrorKind::Forbidden => "forbidden",
        ErrorKind::Conflict => "conflict",
        ErrorKind::InvalidTransition => "invalid_transition",
        ErrorKind::Internal => "internal",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        authorize_equipment_feature, authorize_master_list_import, authorize_read_access,
        principal_create_branch,
    };
    use console_kernel_core::{BranchId, BranchScope, OrgId, UserId};
    use console_platform_authz::{
        EffectiveFeatureGrant, Feature, PermissionLevel, Principal, Role,
    };
    use std::collections::BTreeSet;

    fn principal(roles: BTreeSet<Role>, scope: BranchScope) -> Principal {
        Principal::new(UserId::new(), OrgId::new(), roles, scope)
    }

    #[test]
    fn registry_capability_preflights_preserve_scope_for_the_store() {
        let branch = BranchId::new();
        let admin = principal(BTreeSet::from([Role::Admin]), BranchScope::single(branch));
        assert!(authorize_equipment_feature(&admin, Feature::EquipmentManage).is_ok());
        assert_eq!(principal_create_branch(&admin), Some(branch));

        let mechanic = principal(
            BTreeSet::from([Role::Mechanic]),
            BranchScope::single(branch),
        );
        assert!(authorize_read_access(&mechanic).is_ok());
        assert!(authorize_equipment_feature(&mechanic, Feature::EquipmentManage).is_err());

        let empty = principal(BTreeSet::from([Role::Admin]), BranchScope::none());
        assert!(authorize_read_access(&empty).is_err());
        assert!(authorize_equipment_feature(&empty, Feature::EquipmentManage).is_err());

        let all_admin = principal(BTreeSet::from([Role::Admin]), BranchScope::All);
        assert!(authorize_equipment_feature(&all_admin, Feature::EquipmentManage).is_ok());
        assert_eq!(principal_create_branch(&all_admin), None);
    }

    #[test]
    fn tenant_wide_master_import_requires_org_wide_authority() {
        let branch = BranchId::new();
        let branch_admin = principal(BTreeSet::from([Role::Admin]), BranchScope::single(branch));
        assert!(authorize_master_list_import(&branch_admin).is_err());

        let all_admin = principal(BTreeSet::from([Role::Admin]), BranchScope::All);
        assert!(authorize_master_list_import(&all_admin).is_err());

        let super_admin = principal(BTreeSet::from([Role::SuperAdmin]), BranchScope::All);
        assert!(authorize_master_list_import(&super_admin).is_ok());

        let custom_all = principal(BTreeSet::new(), BranchScope::All)
            .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
                Feature::MasterListImport,
                PermissionLevel::Allow,
                BranchScope::All,
            )]);
        assert!(authorize_master_list_import(&custom_all).is_ok());

        let partial =
            principal(BTreeSet::new(), BranchScope::All).with_effective_feature_grants(vec![
                EffectiveFeatureGrant::new(
                    Feature::MasterListImport,
                    PermissionLevel::Allow,
                    BranchScope::single(branch),
                ),
            ]);
        assert!(authorize_master_list_import(&partial).is_err());
    }
}
