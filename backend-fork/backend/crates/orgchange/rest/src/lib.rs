//! Authenticated org-change routes (STORY-ORG-001).
//!
//! Deny-by-default authorization: every route checks an explicit role floor
//! before touching the store, and RLS `org_isolation` conceals out-of-scope
//! rows (absent and forbidden objects are both 404 — deny by omission).
//!
//! Feature-floor note: the `org_change_read/draft/approve/apply` feature keys
//! are registered in `feature_catalog` (migration 0189) but the shared
//! `console_platform_authz::Feature` enum is integrator-owned (see the lane's
//! integration-manifest.json). Until the enum gains the variants, custom-role
//! grants for these keys cannot exist (`Feature::from_str` skips unknown keys
//! fail-closed), so the built-in role floors below are the complete authorized
//! surface and byte-for-byte match `permission_for` for the scout floors:
//! read/draft `[D,D,D,A,A,A]`, approve/apply `[D,D,D,D,A,A]`. Swapping to
//! `authorize(...)` once the variants land is mechanical.
mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use console_kernel_core::{BranchId, ErrorKind, KernelError, UserId};
use console_orgchange_adapter_postgres::{
    CreateOrgChange, DraftPatch, ListFilter, PgOrgChangeError, PgOrgChangeStore,
};
use console_orgchange_domain::{
    OrgChangeKind, OrgChangeStatus, OrgChangeTarget, OrgProposalOp, TargetKind,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{Action, Feature, Principal, Role, authorize_capability};
use console_platform_request_context::RequestContextError;
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::{Date, OffsetDateTime, macros::offset};
use uuid::Uuid;

// The workspace `time` build has no `serde-human-readable`, so a bare `Date`
// would be read off the wire as `[year, ordinal]`; this pins `YYYY-MM-DD`.
time::serde::format_description!(iso_date, Date, "[year]-[month]-[day]");

pub const ORG_CHANGE_ROUTE_PATHS: &[&str] = &[
    "/api/v1/regions",
    "/api/v1/regions/{id}",
    "/api/v1/branches",
    "/api/v1/branches/{id}",
    "/api/v1/org-changes",
    "/api/v1/org-changes/{id}",
    "/api/v1/org-changes/{id}/preflight",
    "/api/v1/org-changes/{id}/submit",
    "/api/v1/org-changes/{id}/approval-steps/{step_id}/decision",
    "/api/v1/org-changes/{id}/effectuate",
    "/api/v1/org-changes/{id}/settlement-items/{item_id}/complete",
    "/api/v1/org-changes/{id}/archive",
    "/api/v1/org-changes/{id}/cancel",
    "/api/v1/org-entities",
];

#[derive(Clone)]
pub struct OrgChangeRestState {
    store: PgOrgChangeStore,
    jwt: Option<JwtVerifier>,
}

impl OrgChangeRestState {
    #[must_use]
    pub fn new(store: PgOrgChangeStore, jwt: Option<JwtVerifier>) -> Self {
        Self { store, jwt }
    }

    /// SSR composition helper: the same org-entity listing as GET
    /// `/org-entities`, or empty (omit) when unauthenticated, unauthorized,
    /// or the listing fails.
    pub async fn visible_org_entities(&self, headers: &HeaderMap) -> Vec<VisibleOrgEntity> {
        list_visible_org_entities(self, headers)
            .await
            .unwrap_or_default()
    }
}

/// Org entity already authorized for SSR. Required `OrgEntitySummary` keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibleOrgEntity {
    pub org_id: String,
    pub slug: String,
    pub name: String,
    pub status: String,
}

pub fn router(state: OrgChangeRestState) -> Router {
    let verifier = state.jwt.clone();
    let pool = state.store.pool().clone();
    let r = Router::new()
        // Legacy org-setup mutations remain at their established URLs, but the
        // orgchange feature owns the handlers and persists only typed drafts.
        // Identity mounts the GET methods on these same paths.
        .route("/api/v1/regions", post(legacy_create_region))
        .route(
            "/api/v1/regions/{id}",
            axum::routing::patch(legacy_update_region).delete(legacy_deactivate_region),
        )
        .route("/api/v1/branches", post(legacy_create_branch))
        .route(
            "/api/v1/branches/{id}",
            axum::routing::patch(legacy_update_branch).delete(legacy_deactivate_branch),
        )
        .route("/api/v1/org-changes", post(create).get(list))
        .route("/api/v1/org-changes/{id}", get(detail).patch(update_draft))
        .route("/api/v1/org-changes/{id}/preflight", post(preflight))
        .route("/api/v1/org-changes/{id}/submit", post(submit))
        .route(
            "/api/v1/org-changes/{id}/approval-steps/{step_id}/decision",
            post(decide),
        )
        .route("/api/v1/org-changes/{id}/effectuate", post(effectuate))
        .route(
            "/api/v1/org-changes/{id}/settlement-items/{item_id}/complete",
            post(complete_settlement),
        )
        .route("/api/v1/org-changes/{id}/archive", post(archive))
        .route("/api/v1/org-changes/{id}/cancel", post(cancel))
        .route("/api/v1/org-entities", get(org_entities))
        .with_state(state);
    console_platform_request_context::with_request_context(r, verifier, pool)
}

// ---------------------------------------------------------------------------
// Request DTOs
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateOrgChangeRequest {
    kind: OrgChangeKind,
    target: OrgChangeTarget,
    #[serde(with = "iso_date")]
    effective_date: Date,
    reason: String,
    proposal: Vec<OrgProposalOp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    supersedes_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyCreateRegionRequest {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyUpdateRegionRequest {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyCreateBranchRequest {
    region_id: Uuid,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyUpdateBranchRequest {
    #[serde(default)]
    region_id: Option<Uuid>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateOrgChangeDraftRequest {
    #[serde(default)]
    kind: Option<OrgChangeKind>,
    #[serde(default, with = "iso_date::option")]
    effective_date: Option<Date>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    proposal: Option<Vec<OrgProposalOp>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum WireDecision {
    Approved,
    Rejected,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OrgChangeDecisionRequest {
    decision: WireDecision,
    #[serde(default)]
    memo: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompleteSettlementItemRequest {
    #[serde(default)]
    memo: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CancelOrgChangeRequest {
    reason: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn legacy_create_region(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Json(b): Json<LegacyCreateRegionRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), RestError> {
    let p = principal(&s, &h).await?;
    allow_legacy_draft(&p, Feature::RegionManage)?;
    let name = b.name.trim().to_owned();
    draft_legacy_org_proposal(
        &s,
        &h,
        p.user_id,
        OrgChangeKind::New,
        OrgChangeTarget {
            kind: TargetKind::Region,
            target_ref: Uuid::nil().to_string(),
            label: name.clone(),
        },
        "Create region requested through the legacy org-setup API",
        vec![OrgProposalOp::CreateRegion { name }],
    )
    .await
}

async fn legacy_update_region(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<LegacyUpdateRegionRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), RestError> {
    let p = principal(&s, &h).await?;
    allow_legacy_draft(&p, Feature::RegionManage)?;
    let name = b.name.trim().to_owned();
    draft_legacy_org_proposal(
        &s,
        &h,
        p.user_id,
        OrgChangeKind::Reorg,
        OrgChangeTarget {
            kind: TargetKind::Region,
            target_ref: id.to_string(),
            label: name.clone(),
        },
        "Rename region requested through the legacy org-setup API",
        vec![OrgProposalOp::RenameRegion {
            region_id: id,
            name,
        }],
    )
    .await
}

async fn legacy_deactivate_region(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<serde_json::Value>), RestError> {
    let p = principal(&s, &h).await?;
    allow_legacy_draft(&p, Feature::RegionManage)?;
    draft_legacy_org_proposal(
        &s,
        &h,
        p.user_id,
        OrgChangeKind::Dissolve,
        OrgChangeTarget {
            kind: TargetKind::Region,
            target_ref: id.to_string(),
            label: format!("Region {id}"),
        },
        "Deactivate region requested through the legacy org-setup API",
        vec![OrgProposalOp::DeactivateRegion { region_id: id }],
    )
    .await
}

async fn legacy_create_branch(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Json(b): Json<LegacyCreateBranchRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), RestError> {
    let p = principal(&s, &h).await?;
    allow_legacy_draft(&p, Feature::BranchManage)?;
    let name = b.name.trim().to_owned();
    draft_legacy_org_proposal(
        &s,
        &h,
        p.user_id,
        OrgChangeKind::New,
        OrgChangeTarget {
            kind: TargetKind::Region,
            target_ref: b.region_id.to_string(),
            label: name.clone(),
        },
        "Create branch requested through the legacy org-setup API",
        vec![OrgProposalOp::CreateBranch {
            region_id: b.region_id,
            name,
        }],
    )
    .await
}

async fn legacy_update_branch(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<LegacyUpdateBranchRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), RestError> {
    let p = principal(&s, &h).await?;
    allow_legacy_draft(&p, Feature::BranchManage)?;
    conceal_out_of_scope_branch(&p, id)?;
    let name = b.name.map(|name| name.trim().to_owned());
    let label = name.clone().unwrap_or_else(|| format!("Branch {id}"));
    draft_legacy_org_proposal(
        &s,
        &h,
        p.user_id,
        OrgChangeKind::Reorg,
        OrgChangeTarget {
            kind: TargetKind::Branch,
            target_ref: id.to_string(),
            label,
        },
        "Update branch requested through the legacy org-setup API",
        vec![OrgProposalOp::RenameBranch {
            branch_id: id,
            name,
            region_id: b.region_id,
        }],
    )
    .await
}

async fn legacy_deactivate_branch(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<serde_json::Value>), RestError> {
    let p = principal(&s, &h).await?;
    allow_legacy_draft(&p, Feature::BranchManage)?;
    conceal_out_of_scope_branch(&p, id)?;
    draft_legacy_org_proposal(
        &s,
        &h,
        p.user_id,
        OrgChangeKind::Dissolve,
        OrgChangeTarget {
            kind: TargetKind::Branch,
            target_ref: id.to_string(),
            label: format!("Branch {id}"),
        },
        "Deactivate branch requested through the legacy org-setup API",
        vec![OrgProposalOp::DeactivateBranch { branch_id: id }],
    )
    .await
}

async fn draft_legacy_org_proposal(
    s: &OrgChangeRestState,
    h: &HeaderMap,
    actor: UserId,
    kind: OrgChangeKind,
    target: OrgChangeTarget,
    reason: &'static str,
    proposal: Vec<OrgProposalOp>,
) -> Result<(StatusCode, Json<serde_json::Value>), RestError> {
    let idempotency_key = idem_header(h)?;
    let effective_date = OffsetDateTime::now_utc().to_offset(offset!(+9)).date();
    // `effective_date` is a server-owned default, not part of the caller's
    // request. Keep it out of the fingerprint so an exact retry that crosses
    // KST midnight still replays the original draft instead of conflicting.
    let fingerprint_input = json!({
        "surface": "identity.org-structure-proposal",
        "kind": kind,
        "target": target,
        "reason": reason,
        "proposal": proposal,
    });
    let (detail, replayed) = s
        .store
        .create_legacy_org_setup_proposal(
            actor,
            CreateOrgChange {
                kind,
                target,
                effective_date,
                reason: reason.to_owned(),
                proposal,
                supersedes_id: None,
                idempotency_key,
                fingerprint_input,
            },
        )
        .await
        .map_err(RestError::store)?;
    let status = if replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    let body = serde_json::to_value(&detail)
        .map_err(|_| RestError::internal("response serialization failed"))?;
    Ok((status, Json(body)))
}

fn allow_legacy_draft(p: &Principal, feature: Feature) -> Result<(), RestError> {
    allow_draft(p)?;
    authorize_capability(p, Action::new(feature)).map_err(RestError::kernel)
}

fn conceal_out_of_scope_branch(p: &Principal, id: Uuid) -> Result<(), RestError> {
    if p.branch_scope.allows(BranchId::from_uuid(id)) {
        Ok(())
    } else {
        Err(RestError::kernel(KernelError::not_found(
            "branch was not found",
        )))
    }
}

async fn create(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Json(b): Json<CreateOrgChangeRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), RestError> {
    let p = principal(&s, &h).await?;
    allow_draft(&p)?;
    let key = idem_header(&h)?;
    let fingerprint_input = serde_json::to_value(&b)
        .map_err(|_| RestError::kernel(KernelError::validation("request body is not canonical")))?;
    let (detail, replayed) = s
        .store
        .create(
            p.user_id,
            CreateOrgChange {
                kind: b.kind,
                target: b.target,
                effective_date: b.effective_date,
                reason: b.reason,
                proposal: b.proposal,
                supersedes_id: b.supersedes_id,
                idempotency_key: key,
                fingerprint_input,
            },
        )
        .await
        .map_err(RestError::store)?;
    let status = if replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    let body = serde_json::to_value(&detail)
        .map_err(|_| RestError::internal("response serialization failed"))?;
    Ok((status, Json(body)))
}

async fn list(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, RestError> {
    let p = principal(&s, &h).await?;
    allow_read(&p)?;
    let status = q
        .status
        .as_deref()
        .map(OrgChangeStatus::from_db)
        .transpose()
        .map_err(|_| RestError::kernel(KernelError::validation("unknown status filter")))?;
    let kind = q
        .kind
        .as_deref()
        .map(OrgChangeKind::from_db)
        .transpose()
        .map_err(|_| RestError::kernel(KernelError::validation("unknown kind filter")))?;
    let page = s
        .store
        .list(ListFilter {
            status,
            kind,
            limit: q.limit,
            offset: q.offset,
        })
        .await
        .map_err(RestError::store)?;
    respond(&page)
}

async fn detail(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, RestError> {
    let p = principal(&s, &h).await?;
    allow_read(&p)?;
    respond(&s.store.get(id).await.map_err(RestError::store)?)
}

async fn update_draft(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<UpdateOrgChangeDraftRequest>,
) -> Result<Json<serde_json::Value>, RestError> {
    let p = principal(&s, &h).await?;
    allow_draft(&p)?;
    respond(
        &s.store
            .update_draft(
                p.user_id,
                id,
                DraftPatch {
                    kind: b.kind,
                    effective_date: b.effective_date,
                    reason: b.reason,
                    proposal: b.proposal,
                },
            )
            .await
            .map_err(RestError::store)?,
    )
}

async fn preflight(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, RestError> {
    let p = principal(&s, &h).await?;
    allow_draft(&p)?;
    respond(
        &s.store
            .preflight(p.user_id, id)
            .await
            .map_err(RestError::store)?,
    )
}

async fn submit(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, RestError> {
    let p = principal(&s, &h).await?;
    allow_draft(&p)?;
    respond(
        &s.store
            .submit(p.user_id, id)
            .await
            .map_err(RestError::store)?,
    )
}

async fn decide(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Path((id, step_id)): Path<(Uuid, Uuid)>,
    Json(b): Json<OrgChangeDecisionRequest>,
) -> Result<Json<serde_json::Value>, RestError> {
    let p = principal(&s, &h).await?;
    allow_approve(&p)?;
    let approved = matches!(b.decision, WireDecision::Approved);
    respond(
        &s.store
            .decide_step(p.user_id, id, step_id, approved, b.memo)
            .await
            .map_err(RestError::store)?,
    )
}

async fn effectuate(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, RestError> {
    let p = principal(&s, &h).await?;
    allow_apply(&p)?;
    respond(
        &s.store
            .effectuate(p.user_id, id)
            .await
            .map_err(RestError::store)?,
    )
}

async fn complete_settlement(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Path((id, item_id)): Path<(Uuid, Uuid)>,
    Json(b): Json<CompleteSettlementItemRequest>,
) -> Result<Json<serde_json::Value>, RestError> {
    let p = principal(&s, &h).await?;
    allow_apply(&p)?;
    respond(
        &s.store
            .complete_settlement_item(p.user_id, id, item_id, b.memo)
            .await
            .map_err(RestError::store)?,
    )
}

async fn archive(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, RestError> {
    let p = principal(&s, &h).await?;
    allow_apply(&p)?;
    respond(
        &s.store
            .archive(p.user_id, id)
            .await
            .map_err(RestError::store)?,
    )
}

async fn cancel(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<CancelOrgChangeRequest>,
) -> Result<Json<serde_json::Value>, RestError> {
    let p = principal(&s, &h).await?;
    allow_draft(&p)?;
    respond(
        &s.store
            .cancel(p.user_id, id, b.reason)
            .await
            .map_err(RestError::store)?,
    )
}

async fn org_entities(
    State(s): State<OrgChangeRestState>,
    h: HeaderMap,
) -> Result<Json<serde_json::Value>, RestError> {
    let p = principal(&s, &h).await?;
    allow_read(&p)?;
    respond(
        &s.store
            .org_entities(p.user_id)
            .await
            .map_err(RestError::store)?,
    )
}

async fn list_visible_org_entities(
    state: &OrgChangeRestState,
    headers: &HeaderMap,
) -> Result<Vec<VisibleOrgEntity>, RestError> {
    let principal = principal(state, headers).await?;
    allow_read(&principal)?;
    let entities = state
        .store
        .org_entities(principal.user_id)
        .await
        .map_err(RestError::store)?;
    Ok(entities
        .into_iter()
        .map(|entity| VisibleOrgEntity {
            org_id: entity.org_id.to_string(),
            slug: entity.slug,
            name: entity.name,
            status: entity.status,
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Authorization floors (see the module doc for the Feature-enum handoff)
// ---------------------------------------------------------------------------

fn role_floor(p: &Principal, floor: &[Role], denied: &'static str) -> Result<(), RestError> {
    if p.roles.iter().any(|role| floor.contains(role)) {
        Ok(())
    } else {
        Err(RestError::kernel(KernelError::forbidden(denied)))
    }
}

/// `org_change_read` / `org_change_draft` floor `[D,D,D,A,A,A]`.
fn allow_read(p: &Principal) -> Result<(), RestError> {
    role_floor(
        p,
        &[Role::Admin, Role::Executive, Role::SuperAdmin],
        "role is not allowed to read org changes",
    )
}

fn allow_draft(p: &Principal) -> Result<(), RestError> {
    role_floor(
        p,
        &[Role::Admin, Role::Executive, Role::SuperAdmin],
        "role is not allowed to draft org changes",
    )
}

/// `org_change_approve` / `org_change_apply` floor `[D,D,D,D,A,A]`.
fn allow_approve(p: &Principal) -> Result<(), RestError> {
    role_floor(
        p,
        &[Role::Executive, Role::SuperAdmin],
        "role is not allowed to approve org changes",
    )
}

fn allow_apply(p: &Principal) -> Result<(), RestError> {
    role_floor(
        p,
        &[Role::Executive, Role::SuperAdmin],
        "role is not allowed to apply org changes",
    )
}

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

fn respond<T: Serialize>(value: &T) -> Result<Json<serde_json::Value>, RestError> {
    serde_json::to_value(value)
        .map(Json)
        .map_err(|_| RestError::internal("response serialization failed"))
}

fn idem_header(h: &HeaderMap) -> Result<String, RestError> {
    h.get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| {
            RestError::kernel(KernelError::validation(
                "Idempotency-Key header is required",
            ))
        })
}

async fn principal(s: &OrgChangeRestState, h: &HeaderMap) -> Result<Principal, RestError> {
    let verifier = s.jwt.as_ref().ok_or_else(|| {
        RestError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "JWT verification is not configured",
        )
    })?;
    console_platform_request_context::resolve_principal(verifier, s.store.pool(), h)
        .await
        .map_err(|e| match e {
            RequestContextError::MissingBearer
            | RequestContextError::InvalidToken
            | RequestContextError::InvalidClaim(_) => RestError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing, malformed, or invalid bearer token",
            ),
            RequestContextError::WrongTokenTier | RequestContextError::AccessScope(_) => {
                RestError::kernel(KernelError::forbidden(
                    "token is not authorized for org changes",
                ))
            }
            RequestContextError::VerifierUnavailable => RestError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "JWT verification is not configured",
            ),
            RequestContextError::BranchScope(m) | RequestContextError::EffectivePolicy(m) => {
                RestError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", m)
            }
            RequestContextError::MissingOrg => RestError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "no tenant context is bound",
            ),
        })
}

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

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }

    fn kernel(e: KernelError) -> Self {
        match e.kind {
            ErrorKind::Validation => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "validation", e.message)
            }
            ErrorKind::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", e.message),
            ErrorKind::Forbidden => Self::new(StatusCode::FORBIDDEN, "forbidden", e.message),
            ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                Self::new(StatusCode::CONFLICT, "conflict", e.message)
            }
            ErrorKind::Internal => {
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.message)
            }
        }
    }

    fn store(e: PgOrgChangeError) -> Self {
        let message = e.message();
        match e.kind() {
            ErrorKind::Validation => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "validation", message)
            }
            ErrorKind::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", message),
            ErrorKind::Forbidden => Self::new(StatusCode::FORBIDDEN, "forbidden", message),
            ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                Self::new(StatusCode::CONFLICT, "conflict", message)
            }
            ErrorKind::Internal => {
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
            }
        }
    }
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error":{"code":self.code,"message":self.message}})),
        )
            .into_response()
    }
}
