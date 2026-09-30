//! Generic object layer: cross-object links (BE-OBJ slice 1, stage 2) and the
//! object resolve endpoint (stage 3).
//!
//! `object_links` is the generic, org-scoped, audited edge store the design's
//! "related objects" / pin-A-to-B panels need. Links connect two known object
//! kinds (validated against the seeded `object_types` registry) and are
//! removable; every create/delete is audited via `with_audit`. Tenant isolation
//! is enforced by FORCE RLS on `object_links` keyed on `app.current_org`; the
//! `with_org_conn` / `with_audit` wrappers arm that GUC, so a cross-org read
//! returns nothing and a cross-org delete is a 404 (deny-by-omission).

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, ErrorKind, KernelError, OrgId, TraceContext,
    UserId,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::{
    AuthorizationResource, Feature, PermissionLevel, Principal, permission_for,
};
use console_platform_db::{DbError, with_audit, with_audits, with_org_conn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

pub const OBJECT_LINKS_PATH: &str = "/api/v1/object-links";
pub const OBJECT_LINK_BY_ID_PATH_TEMPLATE: &str = "/api/v1/object-links/{id}";
pub const OBJECT_RESOLVE_PATH_TEMPLATE: &str = "/api/objects/{kind}/{id}";
pub const OBJECT_GRAPH_PATH_TEMPLATE: &str = "/api/objects/{kind}/{id}/graph";
pub const SEARCH_PATH: &str = "/api/v1/search";
pub const OBJECT_TYPES_PATH: &str = "/api/v1/object-types";
pub const OBJECT_TYPE_BY_KIND_PATH_TEMPLATE: &str = "/api/v1/object-types/{kind}";
pub const LINK_TYPES_PATH: &str = "/api/v1/link-types";
pub const SERIES_PATH: &str = "/api/v1/series";
pub const SERIES_BY_ID_PATH_TEMPLATE: &str = "/api/v1/series/{id}";
pub const SERIES_INSTANCES_PATH_TEMPLATE: &str = "/api/v1/series/{id}/instances";
pub const SERIES_BY_INSTANCE_PATH: &str = "/api/v1/series/by-instance";

pub const OBJECT_ROUTE_PATHS: &[&str] = &[
    OBJECT_LINKS_PATH,
    OBJECT_LINK_BY_ID_PATH_TEMPLATE,
    OBJECT_RESOLVE_PATH_TEMPLATE,
    OBJECT_GRAPH_PATH_TEMPLATE,
    SEARCH_PATH,
    OBJECT_TYPES_PATH,
    OBJECT_TYPE_BY_KIND_PATH_TEMPLATE,
    LINK_TYPES_PATH,
    SERIES_PATH,
    SERIES_BY_ID_PATH_TEMPLATE,
    SERIES_INSTANCES_PATH_TEMPLATE,
    SERIES_BY_INSTANCE_PATH,
];

/// Max hits returned per object kind (the palette groups by kind). A single
/// bounded, ordered, `LIMIT`ed query runs per kind, so the total is bounded by
/// `SEARCHABLE kinds × limit`.
const SEARCH_LIMIT_DEFAULT: i64 = 20;
const SEARCH_LIMIT_MAX: i64 = 50;
const SEARCH_QUERY_MAX_LEN: usize = 200;

/// Bounds on the graph walk: depth is clamped into `1..=GRAPH_MAX_DEPTH`, and
/// the returned node set is capped at `GRAPH_MAX_NODES` (a maintenance
/// object's neighborhood is small; this is a circuit breaker against a
/// pathologically dense link graph, not an expected ceiling). The walk is a
/// Rust-side level-by-level BFS (never a recursive SQL CTE): a `seen` set
/// makes every (kind, id) enter the frontier at most once, so a cycle in
/// object_links cannot cause unbounded/exponential work — cost is bounded by
/// (edges actually touched, ≤ GRAPH_MAX_DEPTH batch queries), not
/// degree^depth.
const GRAPH_MAX_DEPTH: i64 = 5;
const GRAPH_MAX_NODES: usize = 200;

/// Defense-in-depth caps on the two otherwise-unbounded `object_links` scans.
/// Neither is an expected ceiling — a real object's neighborhood is tiny — they
/// bound a pathological fan-out so a single densely-linked object cannot
/// materialize an unbounded row set before the node cap / list trims.
/// `OBJECT_LINK_LIST_MAX` caps each direction of `list_object_links` (which has
/// no pagination params), matching sibling list caps and `GRAPH_MAX_NODES`;
/// `GRAPH_MAX_LINKS_PER_LEVEL` caps one BFS level's link fetch — set well above
/// any realistic frontier's edge count so a normal graph is never trimmed.
const OBJECT_LINK_LIST_MAX: i64 = 200;
const GRAPH_MAX_LINKS_PER_LEVEL: i64 = 1000;

/// The authorization a resolvable kind requires, DECLARED per kind rather than
/// hand-checked at each call site. `Feature(f)` = the caller must hold feature
/// `f` (parity with the domain read endpoint the head aggregates);
/// `MembershipOnly` = any authenticated tenant member (the base
/// `authorize_object_member` gate already applied — no extra feature);
/// `SelfScoped` = the resolver itself filters to the caller's own rows
/// (passkey/consent), so membership is enough at this layer.
#[derive(Debug, Clone, Copy)]
enum RequiredAuth {
    Feature(Feature),
    MembershipOnly,
    SelfScoped,
}

/// SINGLE SOURCE OF TRUTH for the resolve layer: every kind it can dereference,
/// paired with the auth that kind requires. A kind absent from this table is
/// unresolvable — a direct resolve is 404, a graph node is omitted — and there
/// is no fall-through-to-Login default, so a NEW resolvable kind cannot ship
/// without an explicit auth decision here. (That fall-through was the recurring
/// bug: #220 work_order+equipment and #239 account each shipped gated only at
/// Login until caught, because the guard was a hand-maintained special case,
/// not a declared property of the kind.) Both the 404-for-unknown-kind check
/// and the per-call feature gates derive from this table.
///
/// SECURITY (B3 interaction): adding a currently-NON-resolvable kind here makes
/// pre-existing `object_links` to objects of that kind — created under B3's
/// "non-resolvable kinds skip the endpoint-visibility gate" pass-through —
/// RETROACTIVELY resolvable, with NO backfill re-check. Auditing existing links
/// of the kind is part of adding it.
const RESOLVABLE_KIND_AUTH: &[(&str, RequiredAuth)] = &[
    (
        "work_order",
        RequiredAuth::Feature(Feature::WorkOrderReadAll),
    ),
    (
        "equipment",
        RequiredAuth::Feature(Feature::WorkOrderReadAll),
    ),
    ("account", RequiredAuth::Feature(Feature::UserManage)),
    ("support_ticket", RequiredAuth::MembershipOnly),
    ("org_unit", RequiredAuth::MembershipOnly),
    ("person", RequiredAuth::MembershipOnly),
    ("approval_run", RequiredAuth::MembershipOnly),
    ("passkey", RequiredAuth::SelfScoped),
    ("consent", RequiredAuth::SelfScoped),
    ("series", RequiredAuth::MembershipOnly),
];

/// The declared auth for `kind`, or `None` if the kind is not resolvable at all.
fn required_auth_for_kind(kind: &str) -> Option<RequiredAuth> {
    RESOLVABLE_KIND_AUTH
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, auth)| *auth)
}

/// Whether the caller satisfies a kind's declared auth. `MembershipOnly` and
/// `SelfScoped` are satisfied by any member (the base gate / the resolver's own
/// caller filter carry them); a `Feature` requirement is satisfied only when the
/// caller holds that feature (precomputed in `satisfied_features`).
fn auth_satisfied(auth: RequiredAuth, satisfied_features: &[Feature]) -> bool {
    match auth {
        RequiredAuth::Feature(feature) => satisfied_features.contains(&feature),
        RequiredAuth::MembershipOnly | RequiredAuth::SelfScoped => true,
    }
}

/// The subset of features named in `RESOLVABLE_KIND_AUTH` that this caller
/// actually holds, computed ONCE per request and threaded into every
/// `resolve_head` call so each kind's declared `Feature` gate is a set lookup,
/// not a repeated permission scan.
fn satisfied_features(principal: &Principal) -> Vec<Feature> {
    RESOLVABLE_KIND_AUTH
        .iter()
        .filter_map(|(_, auth)| match auth {
            RequiredAuth::Feature(feature) => Some(*feature),
            RequiredAuth::MembershipOnly | RequiredAuth::SelfScoped => None,
        })
        .filter(|feature| authorize_object_feature(principal, *feature).is_ok())
        .collect()
}

const ID_MAX: usize = 200;

#[derive(Debug, Clone)]
pub struct ObjectState {
    pool: PgPool,
    jwt_verifier: Option<JwtVerifier>,
}

impl ObjectState {
    #[must_use]
    pub fn new(pool: PgPool, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self { pool, jwt_verifier }
    }
}

pub fn router(state: ObjectState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.pool.clone();
    let router = Router::new()
        .route(
            OBJECT_LINKS_PATH,
            get(list_object_links).post(create_object_link),
        )
        .route(OBJECT_LINK_BY_ID_PATH_TEMPLATE, delete(delete_object_link))
        .route(OBJECT_RESOLVE_PATH_TEMPLATE, get(resolve_object))
        .route(OBJECT_GRAPH_PATH_TEMPLATE, get(object_graph))
        .route(SEARCH_PATH, get(search_objects))
        .route(OBJECT_TYPES_PATH, get(list_object_types))
        .route(OBJECT_TYPE_BY_KIND_PATH_TEMPLATE, get(get_object_type))
        .route(LINK_TYPES_PATH, get(list_link_types))
        .route(SERIES_PATH, post(create_series))
        .route(SERIES_BY_INSTANCE_PATH, get(series_by_instance))
        .route(SERIES_BY_ID_PATH_TEMPLATE, get(get_series))
        .route(SERIES_INSTANCES_PATH_TEMPLATE, post(attach_series_instance))
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

// ---------------------------------------------------------------------------
// Wire shapes.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateObjectLinkRequest {
    src_kind: String,
    src_id: String,
    dst_kind: String,
    dst_id: String,
    link_type: String,
}

#[derive(Debug, Deserialize)]
struct ListObjectLinksQuery {
    kind: String,
    id: String,
}

#[derive(Debug, Serialize)]
struct ObjectLinkResponse {
    id: Uuid,
    src_kind: String,
    src_id: String,
    dst_kind: String,
    dst_id: String,
    link_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

/// Links touching one object, in both directions: `outgoing` are edges where
/// the object is the source, `incoming` where it is the destination.
#[derive(Debug, Serialize)]
struct ObjectLinksListResponse {
    outgoing: Vec<ObjectLinkResponse>,
    incoming: Vec<ObjectLinkResponse>,
}

#[derive(Debug)]
struct NormalizedLink {
    src_kind: String,
    src_id: String,
    dst_kind: String,
    dst_id: String,
    link_type: String,
}

// ---------------------------------------------------------------------------
// Handlers.
// ---------------------------------------------------------------------------

async fn create_object_link(
    State(state): State<ObjectState>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<CreateObjectLinkRequest>,
) -> Result<Json<ObjectLinkResponse>, ObjectError> {
    authorize_object_member(&principal)?;
    let link = normalize_link(body)?;
    let link_id = Uuid::new_v4();
    let org = principal.org_id;
    let actor = principal.user_id;
    let now = OffsetDateTime::now_utc();
    // Same per-kind visibility inputs resolve_object threads into resolve_head,
    // so the endpoint-existence guard below denies exactly what a direct resolve
    // would (B3).
    let scope = principal.branch_scope.clone();
    let caller = *principal.user_id.as_uuid();
    let satisfied = satisfied_features(&principal);
    let held_role_keys = crate::workflow_studio::held_authority_role_keys(
        &principal,
        org,
        crate::workflow_studio::WORKFLOW_SPINE_HAS_NO_BRANCH,
    );
    let audit_after = json!({
        "id": link_id,
        "src_kind": link.src_kind,
        "src_id": link.src_id,
        "dst_kind": link.dst_kind,
        "dst_id": link.dst_id,
        "link_type": link.link_type,
    });
    let audit_event = AuditEvent::new(
        Some(actor),
        AuditAction::new("object_link.create")?,
        "object_link",
        link_id.to_string(),
        TraceContext::generate(),
        now,
    )
    .with_org(org)
    .with_snapshots(None, Some(audit_after));

    let response = with_audit::<_, _, ObjectError>(&state.pool, audit_event, move |tx| {
        Box::pin(async move {
            // Both kinds must be known (clean 422 rather than a raw FK 500).
            ensure_kinds_exist(tx, &link.src_kind, &link.dst_kind).await?;
            // link_type must be a registered edge type (edge-type registry,
            // slice 3): a clean validation error rather than the raw FK 500 the
            // object_links_link_type_fkey would otherwise produce.
            ensure_link_type_exists(tx, &link.link_type).await?;
            // B3: both endpoints must resolve for the caller — an edge can only
            // connect objects it can actually see. Same tx as the insert, so the
            // check and the write are atomic.
            ensure_endpoint_resolvable(
                tx,
                &scope,
                caller,
                &held_role_keys,
                &satisfied,
                &link.src_kind,
                &link.src_id,
            )
            .await?;
            ensure_endpoint_resolvable(
                tx,
                &scope,
                caller,
                &held_role_keys,
                &satisfied,
                &link.dst_kind,
                &link.dst_id,
            )
            .await?;
            let insert = sqlx::query(
                r#"
                INSERT INTO object_links (
                    id, org_id, src_kind, src_id, dst_kind, dst_id, link_type, created_by
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                RETURNING id, src_kind, src_id, dst_kind, dst_id, link_type,
                          created_by, created_at
                "#,
            )
            .bind(link_id)
            .bind(*org.as_uuid())
            .bind(&link.src_kind)
            .bind(&link.src_id)
            .bind(&link.dst_kind)
            .bind(&link.dst_id)
            .bind(&link.link_type)
            .bind(*actor.as_uuid())
            .fetch_one(tx.as_mut())
            .await;
            match insert {
                Ok(row) => object_link_from_row(&row),
                Err(err) if is_unique_violation(&err) => Err(ObjectError::conflict(
                    "an identical object link already exists",
                )),
                Err(err) => Err(ObjectError::from(err)),
            }
        })
    })
    .await?;
    Ok(Json(response))
}

async fn delete_object_link(
    State(state): State<ObjectState>,
    Extension(principal): Extension<Principal>,
    Path(link_id): Path<Uuid>,
) -> Result<StatusCode, ObjectError> {
    authorize_object_member(&principal)?;
    // A link is an audited (often governance) edge, so deletion is NOT open to
    // every member the way create/list are: only the edge's own creator, or a
    // UserManage-tier admin, may remove it. `created_by` is the stored author
    // (NULL for system-planted rows -> only an admin can delete those).
    let can_manage = authorize_object_feature(&principal, Feature::UserManage).is_ok();
    let org = principal.org_id;
    let actor = principal.user_id;
    let now = OffsetDateTime::now_utc();

    // The audit before-snapshot cannot be known until the row is loaded inside
    // the tx, so use with_audits (event computed in-transaction).
    with_audits::<_, (), ObjectError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            // Load first (under armed RLS): unknown id OR another tenant's link
            // are indistinguishable — both 404, the deny-by-omission guarantee.
            let existing = sqlx::query(
                r#"
                SELECT id, src_kind, src_id, dst_kind, dst_id, link_type,
                       created_by, created_at
                FROM object_links
                WHERE id = $1
                "#,
            )
            .bind(link_id)
            .fetch_optional(tx.as_mut())
            .await?;
            let Some(row) = existing else {
                return Err(ObjectError::not_found("object link not found"));
            };
            let before = object_link_from_row(&row)?;
            // Authorize deletion against the loaded row. The row is already
            // org-RLS-scoped, so a cross-org id was 404 above — this 403 only
            // fires for an in-org link the caller can already see via list, so
            // it reveals no existence the list did not.
            let is_creator = before.created_by == Some(*actor.as_uuid());
            if !is_creator && !can_manage {
                return Err(ObjectError::from_kernel(KernelError::forbidden(
                    "only the link's creator or a user-manager may delete it",
                )));
            }
            sqlx::query("DELETE FROM object_links WHERE id = $1")
                .bind(link_id)
                .execute(tx.as_mut())
                .await?;
            let event = AuditEvent::new(
                Some(actor),
                AuditAction::new("object_link.delete")?,
                "object_link",
                link_id.to_string(),
                TraceContext::generate(),
                now,
            )
            .with_org(org)
            .with_snapshots(audit_delete_snapshot(&before), None);
            Ok(((), vec![event]))
        })
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_object_links(
    State(state): State<ObjectState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<ListObjectLinksQuery>,
) -> Result<Json<ObjectLinksListResponse>, ObjectError> {
    authorize_object_member(&principal)?;
    let kind = normalize_kind(&query.kind, "kind")?;
    let id = normalize_object_id(&query.id, "id")?;
    let org = principal.org_id;

    let response = with_org_conn::<_, _, ObjectError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let outgoing_rows = sqlx::query(
                r#"
                SELECT id, src_kind, src_id, dst_kind, dst_id, link_type,
                       created_by, created_at
                FROM object_links
                WHERE src_kind = $1 AND src_id = $2
                ORDER BY created_at DESC, id DESC
                LIMIT $3
                "#,
            )
            .bind(&kind)
            .bind(&id)
            .bind(OBJECT_LINK_LIST_MAX)
            .fetch_all(tx.as_mut())
            .await?;
            let incoming_rows = sqlx::query(
                r#"
                SELECT id, src_kind, src_id, dst_kind, dst_id, link_type,
                       created_by, created_at
                FROM object_links
                WHERE dst_kind = $1 AND dst_id = $2
                ORDER BY created_at DESC, id DESC
                LIMIT $3
                "#,
            )
            .bind(&kind)
            .bind(&id)
            .bind(OBJECT_LINK_LIST_MAX)
            .fetch_all(tx.as_mut())
            .await?;
            let outgoing = outgoing_rows
                .iter()
                .map(object_link_from_row)
                .collect::<Result<Vec<_>, _>>()?;
            let incoming = incoming_rows
                .iter()
                .map(object_link_from_row)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ObjectLinksListResponse { outgoing, incoming })
        })
    })
    .await?;
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// Resolve: GET /api/objects/{kind}/{id} -> compact ObjectHead.
// ---------------------------------------------------------------------------

/// Compact, kind-agnostic head for any object. `exists` is the deny-by-omission
/// signal for a SINGLE resolve: an object outside the caller's org/branch
/// scope resolves the SAME as a genuinely-absent id (`exists: false`), so the
/// caller can never distinguish "not yours" from "not there". A well-formed
/// but unregistered kind is 404. In a graph response (`ObjectGraphResponse`),
/// by contrast, an unresolvable node is never present at all (OMITTED, not an
/// `exists: false` stub) — deny-by-omission governs discovery itself there,
/// not just display, so `exists` is always `true` for a graph node.
///
/// No route/URL field: `objectRegistry` (`web/src/lib/objectRegistry.ts`) is
/// the sole kind->URL authority (charter decision, option b) — the backend
/// never issues routes.
#[derive(Debug, Serialize)]
struct ObjectHead {
    kind: String,
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    exists: bool,
}

#[derive(Debug, Default)]
struct ResolvedHead {
    code: Option<String>,
    title: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphQuery {
    depth: Option<i64>,
}

/// The bounded neighborhood of an object: every node the caller can actually
/// resolve (each passed through the same per-kind visibility guard as
/// `resolve_object`; an unresolvable node is OMITTED, never included as a
/// stub, and the walk never expands through it) plus the edges between
/// resolved nodes (an edge touching an omitted node is omitted too).
#[derive(Debug, Serialize)]
struct ObjectGraphResponse {
    nodes: Vec<ObjectHead>,
    edges: Vec<ObjectLinkResponse>,
    /// `true` if the walk returned a partial (but still correctly scoped)
    /// neighborhood: either `GRAPH_MAX_NODES` was hit before `depth` was
    /// exhausted, or a level's link fetch hit `GRAPH_MAX_LINKS_PER_LEVEL` (edges
    /// beyond the cap were dropped).
    truncated: bool,
}

/// The `person.view` audit a DIRECT lookup of a person card emits — shared by
/// `resolve_object` and the `object_graph` ROOT (both are direct lookups). One
/// event iff the target is a NON-SELF person that actually resolved (mirrors
/// get_member); empty otherwise. A WALKED graph node never calls this:
/// traversal is not a card view.
fn person_view_events(
    kind: &str,
    id: &str,
    resolved: &Option<ResolvedHead>,
    caller: Uuid,
    actor: UserId,
    org: OrgId,
) -> Result<Vec<AuditEvent>, ObjectError> {
    if kind != "person" || resolved.is_none() {
        return Ok(Vec::new());
    }
    let Some(subject) = parse_uuid(id).filter(|s| *s != caller) else {
        return Ok(Vec::new());
    };
    Ok(vec![
        AuditEvent::new(
            Some(actor),
            AuditAction::new("person.view")?,
            "person",
            subject.to_string(),
            TraceContext::generate(),
            OffsetDateTime::now_utc(),
        )
        .with_org(org),
    ])
}

async fn resolve_object(
    State(state): State<ObjectState>,
    Extension(principal): Extension<Principal>,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Json<ObjectHead>, ObjectError> {
    authorize_object_member(&principal)?;
    // A malformed slug is a client error; a well-formed unregistered kind is 404.
    let kind = normalize_kind(&kind, "kind")?;
    let Some(required) = required_auth_for_kind(&kind) else {
        return Err(ObjectError::not_found("unknown object kind"));
    };
    // Feature parity with the domain read endpoints, DECLARED per kind in
    // RESOLVABLE_KIND_AUTH (e.g. work_order/equipment -> WorkOrderReadAll like
    // workorder/registry rest, account -> UserManage like identity rest) instead
    // of hand-maintained special cases — otherwise a MEMBER (Login-only) could
    // read heads its role is denied (an account head leaks display_name +
    // active/inactive lifecycle status). The deny is kind-level, independent of
    // the id, so it cannot become an existence oracle. This is the LOUD form
    // (403) for a directly-requested top-level kind; resolve_head (shared with
    // object_graph) carries the quiet per-node form of the same declared gate
    // (omit, not error) for kinds discovered mid-walk.
    let satisfied = satisfied_features(&principal);
    let legacy_allowed = auth_satisfied(required, &satisfied);

    // Enrollment wave 2 (read surface): audit-only Cedar parity observation.
    // Recorded BEFORE the kind-level 403 below so both allow and deny directions
    // are measured. Only the capability-gated kinds (`RequiredAuth::Feature`)
    // carry a Cedar-expressible visibility decision (work_order/equipment ->
    // WorkOrderReadAll, account -> UserManage); `MembershipOnly`/`SelfScoped`
    // kinds are pure branch-scope/RLS with no capability verdict to compare.
    // Legacy remains the sole enforcer — this never gates.
    if let RequiredAuth::Feature(feature) = required {
        let resource = AuthorizationResource::org_wide(principal.org_id, kind.as_str().to_owned())
            .with_resource_id(id.clone());
        crate::cedar_parity::observe_parity(
            &state.pool,
            &principal,
            principal.org_id,
            feature,
            resource,
            crate::cedar_parity::OBJECT_RESOLVE_DOMAIN,
            crate::cedar_parity::CEDAR_PBAC_SHADOW_OBJECT_RESOLVE_FLAG,
            legacy_allowed,
        )
        .await;
    }

    if !legacy_allowed {
        return Err(ObjectError::from_kernel(KernelError::forbidden(
            "insufficient permission for this object kind",
        )));
    }
    let org = principal.org_id;
    let scope = principal.branch_scope.clone();
    let caller = *principal.user_id.as_uuid();
    // Authority role keys the caller holds, computed exactly as the waiting-task
    // inbox does, so approval_run resolves for the same principals the inbox
    // exposes the run to (claimers + assignee-role holders), not just initiators.
    let held_role_keys = crate::workflow_studio::held_authority_role_keys(
        &principal,
        org,
        crate::workflow_studio::WORKFLOW_SPINE_HAS_NO_BRANCH,
    );

    let actor = principal.user_id;

    // Every resolver reads under the caller's armed org (RLS) and, for
    // branch-scoped kinds, drops rows outside the caller's branch scope — so an
    // out-of-scope object is indistinguishable from a missing one. Run through
    // with_audits so a NON-SELF person card view records its person.view (B1) in
    // the SAME read tx (get_member's guarantee); every other kind commits with an
    // empty events vec, which is byte-identical to with_org_conn.
    let resolved = with_audits::<_, Option<ResolvedHead>, ObjectError>(&state.pool, org, {
        let kind = kind.clone();
        let id = id.clone();
        move |tx| {
            Box::pin(async move {
                let head =
                    resolve_head(tx, &scope, caller, &held_role_keys, &satisfied, &kind, &id)
                        .await?;
                let events = person_view_events(&kind, &id, &head, caller, actor, org)?;
                Ok((head, events))
            })
        }
    })
    .await?;

    Ok(Json(object_head_from_resolved(kind, id, resolved)))
}

/// Dispatch a single (kind, id) to its per-kind resolver. Shared by
/// `resolve_object` (one object, 404/403 up front) and `object_graph` (many
/// nodes; an unregistered/unresolvable/insufficiently-privileged kind is just
/// omitted, never an error). The kind's DECLARED auth (`RESOLVABLE_KIND_AUTH`)
/// is the single visibility gate here, applied identically to both callers, so
/// a node in the graph can never be more visible than a direct resolve of that
/// node would be. `resolve_object` raises the same unmet `Feature` as a LOUD
/// 403; this shared path stays quiet (omit) for kinds discovered mid-walk.
async fn resolve_head(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &BranchScope,
    caller: Uuid,
    held_role_keys: &[String],
    satisfied_features: &[Feature],
    kind: &str,
    id: &str,
) -> Result<Option<ResolvedHead>, ObjectError> {
    // An unmapped kind, or one whose declared Feature the caller lacks, is
    // invisible: Ok(None) is byte-identical to "not found"/"not visible", so no
    // existence oracle. Membership-only and self-scoped kinds pass this gate
    // (their scoping lives inside the resolver / the base member check).
    let Some(required) = required_auth_for_kind(kind) else {
        return Ok(None);
    };
    if !auth_satisfied(required, satisfied_features) {
        return Ok(None);
    }
    match kind {
        "work_order" => resolve_work_order(tx, scope, id).await,
        "equipment" => resolve_equipment(tx, scope, id).await,
        "support_ticket" => resolve_support_ticket(tx, scope, id).await,
        "org_unit" => resolve_org_unit(tx, scope, id).await,
        "person" => resolve_person(tx, scope, id).await,
        "approval_run" => resolve_approval_run(tx, caller, held_role_keys, id).await,
        // Identity kinds (Identity Console UI-M13 / charter G-b). account =
        // person's branch-membership visibility (lifecycle object, so it shows
        // deactivated in-scope accounts + status); passkey/consent are self-owned
        // (only the caller resolves their own credential/consent). Their auth is
        // declared in RESOLVABLE_KIND_AUTH and already checked above.
        "account" => resolve_account(tx, scope, id).await,
        "passkey" => resolve_passkey(tx, caller, id).await,
        "consent" => resolve_consent(tx, caller, id).await,
        // series is org-scoped (not branch-scoped: no branch_id column), so
        // membership + the armed RLS org scope are the whole gate — same
        // shape as support_ticket/org_unit/person/approval_run above.
        "series" => resolve_series(tx, id).await,
        // A kind declared in RESOLVABLE_KIND_AUTH but with no arm here (or vice
        // versa) resolves as invisible — deny by default. The tests assert the
        // two stay in sync.
        _ => Ok(None),
    }
}

fn object_head_from_resolved(
    kind: String,
    id: String,
    resolved: Option<ResolvedHead>,
) -> ObjectHead {
    match resolved {
        Some(fields) => ObjectHead {
            kind,
            id,
            code: fields.code,
            title: fields.title,
            status: fields.status,
            exists: true,
        },
        None => ObjectHead {
            kind,
            id,
            code: None,
            title: None,
            status: None,
            exists: false,
        },
    }
}

// ---------------------------------------------------------------------------
// Graph: GET /api/objects/{kind}/{id}/graph?depth=N -> bounded neighborhood.
// ---------------------------------------------------------------------------

async fn object_graph(
    State(state): State<ObjectState>,
    Extension(principal): Extension<Principal>,
    Path((kind, id)): Path<(String, String)>,
    Query(query): Query<GraphQuery>,
) -> Result<Json<ObjectGraphResponse>, ObjectError> {
    authorize_object_member(&principal)?;
    let kind = normalize_kind(&kind, "kind")?;
    let depth = query.depth.unwrap_or(1).clamp(1, GRAPH_MAX_DEPTH);
    let org = principal.org_id;
    let scope = principal.branch_scope.clone();
    let caller = *principal.user_id.as_uuid();
    let actor = principal.user_id;
    // Same declared per-kind gates resolve_object enforces loudly (403) for a
    // directly-requested kind; here they are quiet (a node the caller lacks the
    // required feature for is simply omitted from the walk, never a 403 for the
    // whole graph request).
    let satisfied = satisfied_features(&principal);
    let held_role_keys = crate::workflow_studio::held_authority_role_keys(
        &principal,
        org,
        crate::workflow_studio::WORKFLOW_SPINE_HAS_NO_BRANCH,
    );

    // Rust-side level-by-level BFS over object_links, NOT a recursive SQL CTE:
    // `seen` admits every (kind, id) into the frontier at most once, so a
    // cycle cannot cause unbounded/exponential work regardless of graph
    // density. At most GRAPH_MAX_DEPTH batch link queries run (one per level,
    // each scoped to the current frontier only) plus one resolve_head call
    // per newly-discovered candidate. Deny-by-omission governs discovery
    // itself: a node that does not resolve is OMITTED (never a stub) and the
    // walk never expands through it, so the caller only ever sees their own
    // visible subgraph.
    let (nodes, edges, truncated) = with_audits::<_, _, ObjectError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let root_key = (kind.clone(), id.clone());
            let mut seen: HashSet<(String, String)> = HashSet::new();
            seen.insert(root_key.clone());

            let mut nodes: Vec<ObjectHead> = Vec::new();
            // Raw edges touched while walking the frontier, keyed by link
            // id to dedupe re-fetches across levels; filtered down to the
            // resolved-only induced subgraph once the walk finishes.
            let mut edges_by_id: HashMap<Uuid, ObjectLinkResponse> = HashMap::new();
            let mut truncated = false;

            let root_resolved =
                resolve_head(tx, &scope, caller, &held_role_keys, &satisfied, &kind, &id).await?;
            // The ROOT is a direct lookup, so a non-self person root audits
            // exactly like resolve_object (B1). Only the root — walked nodes
            // below never build events. Computed before root_fields is moved.
            let root_events = person_view_events(&kind, &id, &root_resolved, caller, actor, org)?;
            let Some(root_fields) = root_resolved else {
                // The root itself is invisible/absent: an empty graph,
                // never a stub — matches resolve_object's own
                // deny-by-omission guarantee for this same (kind, id).
                return Ok(((Vec::new(), Vec::new(), false), Vec::new()));
            };
            nodes.push(object_head_from_resolved(
                kind.clone(),
                id.clone(),
                Some(root_fields),
            ));

            // `frontier` = nodes resolved in the PREVIOUS round whose
            // links have not been fetched yet.
            let mut frontier: Vec<(String, String)> = vec![root_key];

            for _hop in 0..depth {
                if frontier.is_empty() || nodes.len() >= GRAPH_MAX_NODES {
                    if nodes.len() >= GRAPH_MAX_NODES {
                        truncated = true;
                    }
                    break;
                }

                let frontier_kinds: Vec<String> = frontier.iter().map(|(k, _)| k.clone()).collect();
                let frontier_ids: Vec<String> = frontier.iter().map(|(_, i)| i.clone()).collect();
                let link_rows = sqlx::query(
                    r#"
                        SELECT l.id, l.src_kind, l.src_id, l.dst_kind, l.dst_id, l.link_type,
                               l.created_by, l.created_at
                        FROM object_links l
                        WHERE EXISTS (
                            SELECT 1 FROM unnest($1::text[], $2::text[]) AS f(kind, id)
                            WHERE (l.src_kind = f.kind AND l.src_id = f.id)
                               OR (l.dst_kind = f.kind AND l.dst_id = f.id)
                        )
                        ORDER BY l.created_at DESC, l.id DESC
                        LIMIT $3
                        "#,
                )
                .bind(&frontier_kinds)
                .bind(&frontier_ids)
                .bind(GRAPH_MAX_LINKS_PER_LEVEL)
                .fetch_all(tx.as_mut())
                .await?;
                // NOTE: this LIMIT clips RAW links — before resolve_head
                // visibility filtering AND before seen-dedup — so the graph is a
                // best-effort BOUNDED view: truncated=true means "incomplete",
                // NOT "the complete visible set minus N". A clipped level can
                // drop any edge, including a cross-edge between two nodes both in
                // the result. Completeness under adversarial edge-flooding (an
                // actor crowding a shared frontier node to bury a victim's edge)
                // is a SEPARATE follow-up, not solved here.
                if link_rows.len() as i64 >= GRAPH_MAX_LINKS_PER_LEVEL {
                    truncated = true;
                }

                // Every link touching the frontier is recorded (even one
                // between two already-resolved nodes — a cross edge, not
                // just BFS-tree edges); candidate neighbors not yet seen
                // are queued for resolution below.
                let mut candidates: Vec<(String, String)> = Vec::new();
                for row in &link_rows {
                    let link = object_link_from_row(row)?;
                    let src = (link.src_kind.clone(), link.src_id.clone());
                    let dst = (link.dst_kind.clone(), link.dst_id.clone());
                    edges_by_id.insert(link.id, link);
                    for candidate in [src, dst] {
                        if seen.insert(candidate.clone()) {
                            candidates.push(candidate);
                        }
                    }
                }

                let mut next_frontier: Vec<(String, String)> = Vec::new();
                for (node_kind, node_id) in candidates {
                    if nodes.len() >= GRAPH_MAX_NODES {
                        truncated = true;
                        break;
                    }
                    let resolved = resolve_head(
                        tx,
                        &scope,
                        caller,
                        &held_role_keys,
                        &satisfied,
                        &node_kind,
                        &node_id,
                    )
                    .await?;
                    // Unresolved: dropped here — omitted from `nodes`,
                    // never added to `next_frontier` (never expanded),
                    // and any edge touching it is pruned in the filter
                    // below.
                    if let Some(fields) = resolved {
                        nodes.push(object_head_from_resolved(
                            node_kind.clone(),
                            node_id.clone(),
                            Some(fields),
                        ));
                        next_frontier.push((node_kind, node_id));
                    }
                }
                frontier = next_frontier;
            }

            // Keep only edges where BOTH endpoints resolved: an edge
            // touching an omitted node is omitted too, never leaked as a
            // dangling reference to something the caller cannot see.
            let resolved_keys: HashSet<(String, String)> = nodes
                .iter()
                .map(|n| (n.kind.clone(), n.id.clone()))
                .collect();
            let edges: Vec<ObjectLinkResponse> = edges_by_id
                .into_values()
                .filter(|e| {
                    resolved_keys.contains(&(e.src_kind.clone(), e.src_id.clone()))
                        && resolved_keys.contains(&(e.dst_kind.clone(), e.dst_id.clone()))
                })
                .collect();

            Ok(((nodes, edges, truncated), root_events))
        })
    })
    .await?;

    Ok(Json(ObjectGraphResponse {
        nodes,
        edges,
        truncated,
    }))
}

// ---------------------------------------------------------------------------
// Search: GET /api/v1/search?q=&limit= -> cross-kind + person-directory hits.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct SearchResponse {
    /// Caller-visible hits grouped by kind (person first, then work items). Each
    /// hit is an ObjectHead identical in scoping to what `resolveObject` returns
    /// for that (kind, id) — a hit the caller could not resolve never appears.
    results: Vec<ObjectHead>,
}

/// `GET /api/v1/search?q=` — global object + person search backing the ⌘K
/// palette, compose target picker, explore search, and token-grammar dropdowns.
///
/// Searches the same object kinds `resolveObject` serves that have a
/// human-searchable name/code (work_order, equipment, support_ticket, org_unit)
/// plus the person directory (messenger member semantics: active + shared
/// branch). Every hit is produced under the EXACT per-kind visibility guard
/// `resolve_head` uses — the WorkOrderReadAll feature gate for
/// work_order/equipment (a caller lacking it gets zero hits of those kinds:
/// unauthorized-kind omission), and the `branch_visible` branch predicate for
/// every branch-scoped kind — applied inside the SQL so the `LIMIT` counts only
/// visible rows. Org isolation is FORCE-RLS via `with_org_conn`'s armed
/// `app.current_org`. Deny-by-omission throughout: no 403 for a kind the caller
/// cannot see, just absence.
async fn search_objects(
    State(state): State<ObjectState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, ObjectError> {
    authorize_object_member(&principal)?;
    let raw = query.q.trim();
    if raw.is_empty() {
        // An empty query has no matches; return an empty page rather than 422 so
        // the palette can call it eagerly without special-casing.
        return Ok(Json(SearchResponse { results: vec![] }));
    }
    if raw.chars().count() > SEARCH_QUERY_MAX_LEN {
        return Err(ObjectError::validation(format!(
            "q must be {SEARCH_QUERY_MAX_LEN} characters or less"
        )));
    }
    let limit = query
        .limit
        .unwrap_or(SEARCH_LIMIT_DEFAULT)
        .clamp(1, SEARCH_LIMIT_MAX);
    // Case-insensitive substring match with LIKE metacharacters escaped so a `%`
    // or `_` in user input is a literal, not a wildcard.
    let pattern = format!("%{}%", escape_like(raw));

    let org = principal.org_id;
    let scope = principal.branch_scope.clone();
    // Same feature gate `resolve_head` applies to work_order/equipment. When the
    // caller lacks it those kinds are not queried at all (unauthorized-kind
    // omission) — never a 403 for the whole search.
    let can_read_work_order =
        authorize_object_feature(&principal, Feature::WorkOrderReadAll).is_ok();
    let (scope_all, branch_ids) = branch_scope_params(&scope);

    let results = with_org_conn::<_, Vec<ObjectHead>, ObjectError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let mut hits: Vec<ObjectHead> = Vec::new();
            // person directory (messenger member semantics: active + shared branch).
            hits.extend(search_persons(tx, scope_all, &branch_ids, &pattern, limit).await?);
            if can_read_work_order {
                hits.extend(
                    search_branch_kind(
                        tx,
                        "work_order",
                        "SELECT id, request_no AS code, NULL::text AS title, status \
                         FROM work_orders \
                         WHERE request_no ILIKE $1 \
                           AND ($2 OR branch_id = ANY($3) OR branch_id IS NULL) \
                         ORDER BY request_no ASC LIMIT $4",
                        scope_all,
                        &branch_ids,
                        &pattern,
                        limit,
                    )
                    .await?,
                );
                hits.extend(
                    search_branch_kind(
                        tx,
                        "equipment",
                        "SELECT id, equipment_no AS code, manager_name AS title, status \
                         FROM registry_equipment \
                         WHERE (equipment_no ILIKE $1 OR model ILIKE $1 OR manager_name ILIKE $1) \
                           AND ($2 OR branch_id = ANY($3) OR branch_id IS NULL) \
                         ORDER BY equipment_no ASC LIMIT $4",
                        scope_all,
                        &branch_ids,
                        &pattern,
                        limit,
                    )
                    .await?,
                );
            }
            hits.extend(
                search_branch_kind(
                    tx,
                    "support_ticket",
                    "SELECT id, NULL::text AS code, title, status \
                     FROM support_tickets \
                     WHERE title ILIKE $1 \
                       AND ($2 OR branch_id = ANY($3) OR branch_id IS NULL) \
                     ORDER BY created_at DESC LIMIT $4",
                    scope_all,
                    &branch_ids,
                    &pattern,
                    limit,
                )
                .await?,
            );
            hits.extend(
                search_branch_kind(
                    tx,
                    "org_unit",
                    // org_unit IS a branch: its own id is the branch-scope key
                    // (never NULL), so the predicate has no IS NULL arm.
                    "SELECT id, NULL::text AS code, name AS title, NULL::text AS status \
                     FROM branches \
                     WHERE name ILIKE $1 AND ($2 OR id = ANY($3)) \
                     ORDER BY name ASC LIMIT $4",
                    scope_all,
                    &branch_ids,
                    &pattern,
                    limit,
                )
                .await?,
            );
            Ok(hits)
        })
    })
    .await?;

    Ok(Json(SearchResponse { results }))
}

/// Escape SQL-LIKE metacharacters so caller input matches literally.
fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// `(scope_all, branch_ids)` for the SQL branch predicate, mirroring
/// [`branch_visible`]: `All` → `(true, [])` (predicate always passes), a bounded
/// scope → `(false, [branches])`.
fn branch_scope_params(scope: &BranchScope) -> (bool, Vec<Uuid>) {
    match scope {
        BranchScope::All => (true, Vec::new()),
        BranchScope::Branches(set) => (false, set.iter().map(|b| *b.as_uuid()).collect()),
    }
}

/// Person-directory search: EXACTLY `resolve_person`'s guard (is_active = true
/// AND shares a branch with the caller's scope), applied to a name prefix/
/// substring match. A cross-branch or inactive user produces no row — no
/// cross-branch PII/existence leak.
async fn search_persons(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope_all: bool,
    branch_ids: &[Uuid],
    pattern: &str,
    limit: i64,
) -> Result<Vec<ObjectHead>, ObjectError> {
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT u.id, u.display_name
        FROM users u
        LEFT JOIN user_branches ub ON ub.user_id = u.id
        WHERE u.is_active
          AND u.display_name ILIKE $1
          AND ($2 OR ub.branch_id = ANY($3))
        ORDER BY u.display_name ASC
        LIMIT $4
        "#,
    )
    .bind(pattern)
    .bind(scope_all)
    .bind(branch_ids)
    .bind(limit)
    .fetch_all(tx.as_mut())
    .await?;
    rows.iter()
        .map(|row| {
            let id: Uuid = row.try_get("id")?;
            Ok(ObjectHead {
                kind: "person".to_owned(),
                id: id.to_string(),
                code: None,
                title: row.try_get("display_name")?,
                status: None,
                exists: true,
            })
        })
        .collect()
}

/// Branch-scoped domain-kind search. The SQL supplied by the caller MUST select
/// `id`, `code`, `title`, `status` and apply the `($2 OR branch_id = ANY($3) …)`
/// branch predicate — the exact `branch_visible` guard, in SQL, so the `LIMIT`
/// counts only rows the caller may actually resolve.
async fn search_branch_kind(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    kind: &str,
    sql: &'static str,
    scope_all: bool,
    branch_ids: &[Uuid],
    pattern: &str,
    limit: i64,
) -> Result<Vec<ObjectHead>, ObjectError> {
    let rows = sqlx::query(sql)
        .bind(pattern)
        .bind(scope_all)
        .bind(branch_ids)
        .bind(limit)
        .fetch_all(tx.as_mut())
        .await?;
    rows.iter()
        .map(|row| {
            let id: Uuid = row.try_get("id")?;
            Ok(ObjectHead {
                kind: kind.to_owned(),
                id: id.to_string(),
                code: row.try_get("code")?,
                title: row.try_get("title")?,
                status: row.try_get("status")?,
                exists: true,
            })
        })
        .collect()
}

async fn resolve_work_order(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &BranchScope,
    id: &str,
) -> Result<Option<ResolvedHead>, ObjectError> {
    let Some(uuid) = parse_uuid(id) else {
        return Ok(None);
    };
    let row = sqlx::query("SELECT request_no, status, branch_id FROM work_orders WHERE id = $1")
        .bind(uuid)
        .fetch_optional(tx.as_mut())
        .await?;
    let Some(row) = row else { return Ok(None) };
    let branch_id: Uuid = row.try_get("branch_id")?;
    if !branch_visible(scope, Some(branch_id)) {
        return Ok(None);
    }
    Ok(Some(ResolvedHead {
        code: Some(row.try_get("request_no")?),
        title: None,
        status: Some(row.try_get("status")?),
    }))
}

async fn resolve_equipment(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &BranchScope,
    id: &str,
) -> Result<Option<ResolvedHead>, ObjectError> {
    let Some(uuid) = parse_uuid(id) else {
        return Ok(None);
    };
    let row =
        sqlx::query("SELECT manager_name, status, branch_id FROM registry_equipment WHERE id = $1")
            .bind(uuid)
            .fetch_optional(tx.as_mut())
            .await?;
    let Some(row) = row else { return Ok(None) };
    let branch_id: Uuid = row.try_get("branch_id")?;
    if !branch_visible(scope, Some(branch_id)) {
        return Ok(None);
    }
    Ok(Some(ResolvedHead {
        code: None,
        title: row.try_get("manager_name")?,
        status: Some(row.try_get("status")?),
    }))
}

async fn resolve_support_ticket(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &BranchScope,
    id: &str,
) -> Result<Option<ResolvedHead>, ObjectError> {
    let Some(uuid) = parse_uuid(id) else {
        return Ok(None);
    };
    let row = sqlx::query("SELECT title, status, branch_id FROM support_tickets WHERE id = $1")
        .bind(uuid)
        .fetch_optional(tx.as_mut())
        .await?;
    let Some(row) = row else { return Ok(None) };
    // branch_id is nullable: a NULL (org-wide) ticket is visible to any member.
    let branch_id: Option<Uuid> = row.try_get("branch_id")?;
    if !branch_visible(scope, branch_id) {
        return Ok(None);
    }
    Ok(Some(ResolvedHead {
        code: None,
        title: Some(row.try_get("title")?),
        status: Some(row.try_get("status")?),
    }))
}

async fn resolve_org_unit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &BranchScope,
    id: &str,
) -> Result<Option<ResolvedHead>, ObjectError> {
    let Some(uuid) = parse_uuid(id) else {
        return Ok(None);
    };
    // The org unit IS a branch: it is visible only if in the caller's scope.
    if !branch_visible(scope, Some(uuid)) {
        return Ok(None);
    }
    let row = sqlx::query("SELECT name FROM branches WHERE id = $1")
        .bind(uuid)
        .fetch_optional(tx.as_mut())
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ResolvedHead {
        code: None,
        title: Some(row.try_get("name")?),
        status: None,
    }))
}

struct ScopedUserRow {
    display_name: String,
    is_active: bool,
}

async fn resolve_user_by_branch_scope(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &BranchScope,
    user_id: Uuid,
    active_only: bool,
) -> Result<Option<ScopedUserRow>, ObjectError> {
    let row = match scope {
        BranchScope::All => {
            sqlx::query(
                r#"
                SELECT display_name, is_active
                FROM users
                WHERE id = $1 AND ($2 = false OR is_active = true)
                "#,
            )
            .bind(user_id)
            .bind(active_only)
            .fetch_optional(tx.as_mut())
            .await?
        }
        BranchScope::Branches(set) => {
            let branches: Vec<Uuid> = set.iter().map(|b| *b.as_uuid()).collect();
            sqlx::query(
                r#"
                SELECT u.display_name, u.is_active
                FROM users u
                JOIN user_branches ub ON ub.user_id = u.id AND ub.branch_id = ANY($2)
                WHERE u.id = $1 AND ($3 = false OR u.is_active = true)
                LIMIT 1
                "#,
            )
            .bind(user_id)
            .bind(branches)
            .bind(active_only)
            .fetch_optional(tx.as_mut())
            .await?
        }
    };

    let Some(row) = row else { return Ok(None) };
    Ok(Some(ScopedUserRow {
        display_name: row.try_get("display_name")?,
        is_active: row.try_get("is_active")?,
    }))
}

async fn resolve_person(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &BranchScope,
    id: &str,
) -> Result<Option<ResolvedHead>, ObjectError> {
    let Some(uuid) = parse_uuid(id) else {
        return Ok(None);
    };
    // Match GET /api/messenger/members exactly (messenger adapter list_members):
    // is_active = true AND the user shares a branch with the caller's scope.
    // Out-of-scope OR inactive -> no row -> exists:false, byte-identical to a
    // missing id (no cross-branch existence/deactivation oracle).
    let Some(row) = resolve_user_by_branch_scope(tx, scope, uuid, true).await? else {
        return Ok(None);
    };
    Ok(Some(ResolvedHead {
        code: None,
        title: Some(row.display_name),
        status: None,
    }))
}

/// Account = the login object behind a person. SAME branch-membership guard as
/// `resolve_person` (a cross-branch/cross-org account is byte-identical to a
/// missing one — the leak class caught in review), but it is the admin
/// LIFECYCLE object, so unlike `person` it does NOT filter `is_active`: it
/// surfaces deactivated in-scope accounts with `status = active|inactive` (what
/// the S2 activate/deactivate transition renders). Deactivation preserves
/// `user_branches`, so the join still matches a deactivated in-scope user.
async fn resolve_account(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &BranchScope,
    id: &str,
) -> Result<Option<ResolvedHead>, ObjectError> {
    let Some(uuid) = parse_uuid(id) else {
        return Ok(None);
    };
    let Some(row) = resolve_user_by_branch_scope(tx, scope, uuid, false).await? else {
        return Ok(None);
    };
    Ok(Some(ResolvedHead {
        code: None,
        title: Some(row.display_name),
        status: Some(if row.is_active { "active" } else { "inactive" }.to_owned()),
    }))
}

/// Passkey = self-owned WebAuthn credential. Visible ONLY to its owner: the row
/// must exist AND belong to the caller. A passkey owned by anyone else (or a
/// missing id) is `exists:false` — no cross-user credential-enumeration oracle.
/// `auth_webauthn_credentials` gained a NOT NULL `org_id` + FORCE RLS in
/// migrations 0032/0034/0035, so this is defense in depth: `with_org_conn`
/// already arms `app.current_org` (RLS drops cross-org rows before this query
/// runs); the explicit `user_id = caller` filter additionally scopes to the
/// caller's OWN credentials within their org.
async fn resolve_passkey(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    caller: Uuid,
    id: &str,
) -> Result<Option<ResolvedHead>, ObjectError> {
    let Some(uuid) = parse_uuid(id) else {
        return Ok(None);
    };
    let row = sqlx::query(
        "SELECT last_used_at FROM auth_webauthn_credentials WHERE id = $1 AND user_id = $2",
    )
    .bind(uuid)
    .bind(caller)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(row) = row else { return Ok(None) };
    let last_used_at: Option<OffsetDateTime> = row.try_get("last_used_at")?;
    Ok(Some(ResolvedHead {
        code: None,
        title: None,
        status: Some(
            if last_used_at.is_some() {
                "used"
            } else {
                "unused"
            }
            .to_owned(),
        ),
    }))
}

/// Consent = self-owned versioned policy acceptance. There is no consent table:
/// acceptance is recorded as an `audit_events` row (`privacy.required_accept` /
/// `target_type = 'privacy_terms'`), so the consent object's id IS the policy
/// version string (e.g. `kr-pipa-v1-2026-06-25`), NOT a UUID. Visible only when
/// the CALLER has accepted that version (`actor = caller`); another user's
/// consent, or an unaccepted version, is `exists:false`. `audit_events` is
/// org-RLS-scoped (GUC armed by `with_org_conn`), so this cannot cross orgs.
async fn resolve_consent(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    caller: Uuid,
    id: &str,
) -> Result<Option<ResolvedHead>, ObjectError> {
    let accepted: Option<OffsetDateTime> = sqlx::query_scalar(
        r#"
        SELECT occurred_at
        FROM audit_events
        WHERE actor = $1
          AND action = 'privacy.required_accept'
          AND target_type = 'privacy_terms'
          AND target_id = $2
        ORDER BY occurred_at DESC
        LIMIT 1
        "#,
    )
    .bind(caller)
    .bind(id)
    .fetch_optional(tx.as_mut())
    .await?;
    if accepted.is_none() {
        return Ok(None);
    }
    Ok(Some(ResolvedHead {
        code: None,
        title: Some(id.to_owned()),
        status: Some("accepted".to_owned()),
    }))
}

async fn resolve_approval_run(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    caller: Uuid,
    held_role_keys: &[String],
    id: &str,
) -> Result<Option<ResolvedHead>, ObjectError> {
    let Some(uuid) = parse_uuid(id) else {
        return Ok(None);
    };
    // Visible to the same principals the waiting-task inbox exposes the run to:
    // the initiator, anyone who has claimed a task on it, and holders of a role
    // a task is routed to. Everyone else -> exists:false (deny-by-omission).
    let row = sqlx::query(
        r#"
        SELECT r.status
        FROM workflow_runs r
        WHERE r.id = $1
          AND (
              r.initiated_by = $2
              OR EXISTS (
                  SELECT 1 FROM workflow_waiting_tasks t
                  WHERE t.run_id = r.id AND t.org_id = r.org_id
                    AND t.status IN ('OPEN', 'CLAIMED')
                    AND (t.claimed_by = $2 OR t.assignee_role_key = ANY($3))
              )
          )
        "#,
    )
    .bind(uuid)
    .bind(caller)
    .bind(held_role_keys)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ResolvedHead {
        code: None,
        title: None,
        status: Some(row.try_get("status")?),
    }))
}

/// series has no branch_id (org-scoped, not branch-scoped — see the `series`
/// table): the armed RLS org scope on `tx` is the entire visibility gate,
/// matching what `get_series`/`create_series`/`attach_series_instance` already
/// enforce (membership only, no extra feature).
async fn resolve_series(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: &str,
) -> Result<Option<ResolvedHead>, ObjectError> {
    let Some(uuid) = parse_uuid(id) else {
        return Ok(None);
    };
    let row = sqlx::query("SELECT code, label FROM series WHERE id = $1")
        .bind(uuid)
        .fetch_optional(tx.as_mut())
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ResolvedHead {
        code: Some(row.try_get("code")?),
        title: Some(row.try_get("label")?),
        status: None,
    }))
}

// ---------------------------------------------------------------------------
// Type registry: GET /api/v1/object-types [+ /{kind}].
// ---------------------------------------------------------------------------

/// A row from the seeded object-type registry, plus the caller-visible active
/// instance count. The count respects the SAME per-kind visibility as
/// `resolve_object`: it is computed under the caller's armed org (RLS drops
/// cross-org rows, so there is no cross-org counting), further narrowed by the
/// caller's branch scope and the domain feature gate. A kind the caller cannot
/// read at all (e.g. `work_order` without `WorkOrderReadAll`) counts 0, exactly
/// as resolve would deny every instance; a kind with no resolver counts 0.
#[derive(Debug, Serialize)]
struct ObjectTypeResponse {
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code_prefix: Option<String>,
    description: String,
    status: String,
    active_count: i64,
}

#[derive(Debug)]
struct ObjectTypeRow {
    kind: String,
    code_prefix: Option<String>,
    description: String,
    status: String,
}

fn object_type_row_from(row: &sqlx::postgres::PgRow) -> Result<ObjectTypeRow, ObjectError> {
    Ok(ObjectTypeRow {
        kind: row.try_get("kind")?,
        code_prefix: row.try_get("code_prefix")?,
        description: row.try_get("description")?,
        status: row.try_get("status")?,
    })
}

async fn list_object_types(
    State(state): State<ObjectState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<ObjectTypeResponse>>, ObjectError> {
    authorize_object_member(&principal)?;
    let org = principal.org_id;
    let scope = principal.branch_scope.clone();
    let caller = *principal.user_id.as_uuid();
    let satisfied = satisfied_features(&principal);
    let held_role_keys = crate::workflow_studio::held_authority_role_keys(
        &principal,
        org,
        crate::workflow_studio::WORKFLOW_SPINE_HAS_NO_BRANCH,
    );

    let types = with_org_conn::<_, _, ObjectError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT kind, code_prefix, description, status FROM object_types ORDER BY kind",
            )
            .fetch_all(tx.as_mut())
            .await?;
            let mut out = Vec::with_capacity(rows.len());
            for row in &rows {
                let t = object_type_row_from(row)?;
                let active_count =
                    count_kind(tx, &scope, caller, &held_role_keys, &satisfied, &t.kind).await?;
                out.push(ObjectTypeResponse {
                    kind: t.kind,
                    code_prefix: t.code_prefix,
                    description: t.description,
                    status: t.status,
                    active_count,
                });
            }
            Ok(out)
        })
    })
    .await?;
    Ok(Json(types))
}

async fn get_object_type(
    State(state): State<ObjectState>,
    Extension(principal): Extension<Principal>,
    Path(kind): Path<String>,
) -> Result<Json<ObjectTypeResponse>, ObjectError> {
    authorize_object_member(&principal)?;
    let kind = normalize_kind(&kind, "kind")?;
    let org = principal.org_id;
    let scope = principal.branch_scope.clone();
    let caller = *principal.user_id.as_uuid();
    let satisfied = satisfied_features(&principal);
    let held_role_keys = crate::workflow_studio::held_authority_role_keys(
        &principal,
        org,
        crate::workflow_studio::WORKFLOW_SPINE_HAS_NO_BRANCH,
    );

    let response = with_org_conn::<_, Option<ObjectTypeResponse>, ObjectError>(
        &state.pool,
        org,
        move |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT kind, code_prefix, description, status FROM object_types WHERE kind = $1",
                )
                .bind(&kind)
                .fetch_optional(tx.as_mut())
                .await?;
                let Some(row) = row else { return Ok(None) };
                let t = object_type_row_from(&row)?;
                let active_count =
                    count_kind(tx, &scope, caller, &held_role_keys, &satisfied, &t.kind).await?;
                Ok(Some(ObjectTypeResponse {
                    kind: t.kind,
                    code_prefix: t.code_prefix,
                    description: t.description,
                    status: t.status,
                    active_count,
                }))
            })
        },
    )
    .await?;
    response
        .map(Json)
        .ok_or_else(|| ObjectError::not_found("unknown object type"))
}

/// Branch ids the caller is scoped to, or `None` for an unbounded (`All`) scope.
fn scope_branch_ids(scope: &BranchScope) -> Option<Vec<Uuid>> {
    match scope {
        BranchScope::All => None,
        BranchScope::Branches(set) => Some(set.iter().map(|b| *b.as_uuid()).collect()),
    }
}

/// Count the instances of `kind` visible to the caller, mirroring the
/// `resolve_head` visibility rules where the kind has a resolvable endpoint
/// (org via armed RLS, branch scope, and the domain feature gate). Any
/// non-resolvable registry kind counts 0; resolvable kinds that are missing a
/// count arm fail closed with an internal error so dispatch drift cannot
/// silently undercount.
async fn count_kind(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &BranchScope,
    caller: Uuid,
    held_role_keys: &[String],
    satisfied_features: &[Feature],
    kind: &str,
) -> Result<i64, ObjectError> {
    // The feature gate uses the SAME declared source of truth as resolve_head:
    // a caller who cannot resolve the kind's declared Feature counts 0 of it.
    let can_read_work_order = satisfied_features.contains(&Feature::WorkOrderReadAll);
    let can_manage_users = satisfied_features.contains(&Feature::UserManage);
    let branch_ids = scope_branch_ids(scope);
    let all = branch_ids.is_none();
    let ids = branch_ids.unwrap_or_default();
    let count: i64 = match kind {
        "work_order" if can_read_work_order => {
            sqlx::query_scalar("SELECT count(*) FROM work_orders WHERE ($1 OR branch_id = ANY($2))")
                .bind(all)
                .bind(&ids)
                .fetch_one(tx.as_mut())
                .await?
        }
        "equipment" if can_read_work_order => {
            sqlx::query_scalar(
                "SELECT count(*) FROM registry_equipment WHERE ($1 OR branch_id = ANY($2))",
            )
            .bind(all)
            .bind(&ids)
            .fetch_one(tx.as_mut())
            .await?
        }
        "support_ticket" => {
            sqlx::query_scalar(
                "SELECT count(*) FROM support_tickets \
             WHERE ($1 OR branch_id = ANY($2) OR branch_id IS NULL)",
            )
            .bind(all)
            .bind(&ids)
            .fetch_one(tx.as_mut())
            .await?
        }
        "org_unit" => {
            sqlx::query_scalar("SELECT count(*) FROM branches WHERE ($1 OR id = ANY($2))")
                .bind(all)
                .bind(&ids)
                .fetch_one(tx.as_mut())
                .await?
        }
        "person" if all => {
            sqlx::query_scalar("SELECT count(*) FROM users WHERE is_active = true")
                .fetch_one(tx.as_mut())
                .await?
        }
        "person" => {
            sqlx::query_scalar(
                "SELECT count(DISTINCT u.id) FROM users u \
             JOIN user_branches ub ON ub.user_id = u.id AND ub.branch_id = ANY($1) \
             WHERE u.is_active = true",
            )
            .bind(&ids)
            .fetch_one(tx.as_mut())
            .await?
        }
        "account" if can_manage_users && all => {
            sqlx::query_scalar("SELECT count(*) FROM users")
                .fetch_one(tx.as_mut())
                .await?
        }
        "account" if can_manage_users => {
            sqlx::query_scalar(
                "SELECT count(DISTINCT u.id) FROM users u \
             JOIN user_branches ub ON ub.user_id = u.id AND ub.branch_id = ANY($1)",
            )
            .bind(&ids)
            .fetch_one(tx.as_mut())
            .await?
        }
        "approval_run" => {
            sqlx::query_scalar(
                r#"
            SELECT count(*) FROM workflow_runs r
            WHERE (
                r.initiated_by = $1
                OR EXISTS (
                    SELECT 1 FROM workflow_waiting_tasks t
                    WHERE t.run_id = r.id AND t.org_id = r.org_id
                      AND t.status IN ('OPEN', 'CLAIMED')
                      AND (t.claimed_by = $1 OR t.assignee_role_key = ANY($2))
                )
            )
            "#,
            )
            .bind(caller)
            .bind(held_role_keys)
            .fetch_one(tx.as_mut())
            .await?
        }
        "passkey" => {
            sqlx::query_scalar("SELECT count(*) FROM auth_webauthn_credentials WHERE user_id = $1")
                .bind(caller)
                .fetch_one(tx.as_mut())
                .await?
        }
        "consent" => {
            sqlx::query_scalar(
                "SELECT count(DISTINCT target_id) FROM audit_events \
             WHERE actor = $1 AND action = 'privacy.required_accept' \
               AND target_type = 'privacy_terms'",
            )
            .bind(caller)
            .fetch_one(tx.as_mut())
            .await?
        }
        "series" => {
            sqlx::query_scalar("SELECT count(*) FROM series")
                .fetch_one(tx.as_mut())
                .await?
        }
        // Feature-gated resolvable kinds without their required feature: nothing
        // this caller can resolve -> 0.
        "work_order" | "equipment" | "account" => 0,
        kind if required_auth_for_kind(kind).is_some() => {
            return Err(ObjectError::internal(format!(
                "count_kind missing dispatch for resolvable kind `{kind}`"
            )));
        }
        // Non-resolvable registry kinds (document/voucher/...) have no endpoint
        // visibility to mirror, so they do not contribute active counts.
        _ => 0,
    };
    Ok(count)
}

// ---------------------------------------------------------------------------
// Edge-type registry: GET /api/v1/link-types.
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct LinkTypeResponse {
    link_type: String,
    description: String,
    status: String,
}

async fn list_link_types(
    State(state): State<ObjectState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<LinkTypeResponse>>, ObjectError> {
    authorize_object_member(&principal)?;
    let org = principal.org_id;
    let response = with_org_conn::<_, _, ObjectError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT link_type, description, status FROM link_types ORDER BY link_type",
            )
            .fetch_all(tx.as_mut())
            .await?;
            rows.iter()
                .map(|row| {
                    Ok(LinkTypeResponse {
                        link_type: row.try_get("link_type")?,
                        description: row.try_get("description")?,
                        status: row.try_get("status")?,
                    })
                })
                .collect::<Result<Vec<_>, ObjectError>>()
        })
    })
    .await?;
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// SR- series: create (+ first instance), attach, read, by-instance lookup.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateSeriesRequest {
    label: String,
    /// The first instance to seed the series with (kind + id).
    kind: String,
    id: String,
}

#[derive(Debug, Deserialize)]
struct AttachInstanceRequest {
    kind: String,
    id: String,
}

#[derive(Debug, Deserialize)]
struct SeriesByInstanceQuery {
    kind: String,
    id: String,
}

#[derive(Debug, Serialize, Clone)]
struct SeriesHead {
    id: Uuid,
    code: String,
    label: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

/// A series with its members resolved to ObjectHeads, ordered by attach time.
/// Members that no longer resolve for the caller are OMITTED (deny-by-omission,
/// exactly like the object graph) — membership never becomes a back-door read
/// of an object the caller cannot otherwise see.
#[derive(Debug, Serialize)]
struct SeriesDetailResponse {
    #[serde(flatten)]
    head: SeriesHead,
    instances: Vec<ObjectHead>,
}

#[derive(Debug, Serialize)]
struct SeriesByInstanceResponse {
    series: Option<SeriesHead>,
}

fn series_head_from_row(row: &sqlx::postgres::PgRow) -> Result<SeriesHead, ObjectError> {
    Ok(SeriesHead {
        id: row.try_get("id")?,
        code: row.try_get("code")?,
        label: row.try_get("label")?,
        created_at: row.try_get("created_at")?,
    })
}

fn normalize_label(raw: &str) -> Result<String, ObjectError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(ObjectError::validation("label is required"));
    }
    if value.chars().count() > 200 {
        return Err(ObjectError::validation(
            "label must be 200 characters or less",
        ));
    }
    Ok(value.to_owned())
}

async fn create_series(
    State(state): State<ObjectState>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<CreateSeriesRequest>,
) -> Result<Json<SeriesDetailResponse>, ObjectError> {
    authorize_object_member(&principal)?;
    let label = normalize_label(&body.label)?;
    let member_kind = normalize_kind(&body.kind, "kind")?;
    let member_id = normalize_object_id(&body.id, "id")?;
    let org = principal.org_id;
    let actor = principal.user_id;
    let caller = *actor.as_uuid();
    let scope = principal.branch_scope.clone();
    let satisfied = satisfied_features(&principal);
    let held_role_keys = crate::workflow_studio::held_authority_role_keys(
        &principal,
        org,
        crate::workflow_studio::WORKFLOW_SPINE_HAS_NO_BRANCH,
    );
    let now = OffsetDateTime::now_utc();
    let series_id = Uuid::new_v4();

    let detail = with_audits::<_, SeriesDetailResponse, ObjectError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            // The first instance must resolve for the caller (deny-by-omission):
            // you cannot found a series on an object you cannot see.
            let resolved = resolve_head(
                tx,
                &scope,
                caller,
                &held_role_keys,
                &satisfied,
                &member_kind,
                &member_id,
            )
            .await?;
            let Some(fields) = resolved else {
                return Err(ObjectError::not_found(
                    "first instance is not resolvable for the caller",
                ));
            };

            let code = console_platform_db::issue_code(tx, org, "series")
                .await
                .map_err(ObjectError::from)?;

            let series_row = sqlx::query(
                r#"
                INSERT INTO series (id, org_id, code, label, created_by)
                VALUES ($1, $2, $3, $4, $5)
                RETURNING id, code, label, created_at
                "#,
            )
            .bind(series_id)
            .bind(*org.as_uuid())
            .bind(&code)
            .bind(&label)
            .bind(caller)
            .fetch_one(tx.as_mut())
            .await?;
            let head = series_head_from_row(&series_row)?;

            attach_instance_row(tx, org, series_id, caller, &member_kind, &member_id).await?;

            let create_event = AuditEvent::new(
                Some(actor),
                AuditAction::new("series.create")?,
                "series",
                series_id.to_string(),
                TraceContext::generate(),
                now,
            )
            .with_org(org)
            .with_snapshots(None, Some(json!({ "code": code, "label": label })));
            let attach_event = AuditEvent::new(
                Some(actor),
                AuditAction::new("series.attach")?,
                "series",
                series_id.to_string(),
                TraceContext::generate(),
                now,
            )
            .with_org(org)
            .with_snapshots(
                None,
                Some(json!({ "member_kind": member_kind, "member_id": member_id })),
            );

            let instances = vec![object_head_from_resolved(
                member_kind,
                member_id,
                Some(fields),
            )];
            Ok((
                SeriesDetailResponse { head, instances },
                vec![create_event, attach_event],
            ))
        })
    })
    .await?;
    Ok(Json(detail))
}

async fn attach_series_instance(
    State(state): State<ObjectState>,
    Extension(principal): Extension<Principal>,
    Path(series_id): Path<Uuid>,
    Json(body): Json<AttachInstanceRequest>,
) -> Result<Json<ObjectLinkAttachAck>, ObjectError> {
    authorize_object_member(&principal)?;
    let member_kind = normalize_kind(&body.kind, "kind")?;
    let member_id = normalize_object_id(&body.id, "id")?;
    let org = principal.org_id;
    let actor = principal.user_id;
    let caller = *actor.as_uuid();
    let scope = principal.branch_scope.clone();
    let satisfied = satisfied_features(&principal);
    let held_role_keys = crate::workflow_studio::held_authority_role_keys(
        &principal,
        org,
        crate::workflow_studio::WORKFLOW_SPINE_HAS_NO_BRANCH,
    );
    let now = OffsetDateTime::now_utc();

    with_audits::<_, (), ObjectError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            // Series must exist for the caller (RLS-scoped): an unknown id or
            // another tenant's series is 404 (deny-by-omission).
            let exists: Option<Uuid> = sqlx::query_scalar("SELECT id FROM series WHERE id = $1")
                .bind(series_id)
                .fetch_optional(tx.as_mut())
                .await?;
            if exists.is_none() {
                return Err(ObjectError::not_found("series not found"));
            }
            // The instance must resolve for the caller.
            let resolved = resolve_head(
                tx,
                &scope,
                caller,
                &held_role_keys,
                &satisfied,
                &member_kind,
                &member_id,
            )
            .await?;
            if resolved.is_none() {
                return Err(ObjectError::not_found(
                    "instance is not resolvable for the caller",
                ));
            }
            attach_instance_row(tx, org, series_id, caller, &member_kind, &member_id).await?;
            let event = AuditEvent::new(
                Some(actor),
                AuditAction::new("series.attach")?,
                "series",
                series_id.to_string(),
                TraceContext::generate(),
                now,
            )
            .with_org(org)
            .with_snapshots(
                None,
                Some(json!({ "member_kind": member_kind, "member_id": member_id })),
            );
            Ok(((), vec![event]))
        })
    })
    .await?;
    Ok(Json(ObjectLinkAttachAck { attached: true }))
}

#[derive(Debug, Serialize)]
struct ObjectLinkAttachAck {
    attached: bool,
}

/// Insert one membership row, mapping the UNIQUE(org, member_kind, member_id)
/// violation to a clean 409 (the instance is already in a series — the
/// "not-yet-in-a-series" promotion invariant).
async fn attach_instance_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org: console_kernel_core::OrgId,
    series_id: Uuid,
    caller: Uuid,
    member_kind: &str,
    member_id: &str,
) -> Result<(), ObjectError> {
    let insert = sqlx::query(
        r#"
        INSERT INTO series_instances (org_id, series_id, member_kind, member_id, added_by)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(*org.as_uuid())
    .bind(series_id)
    .bind(member_kind)
    .bind(member_id)
    .bind(caller)
    .execute(tx.as_mut())
    .await;
    match insert {
        Ok(_) => Ok(()),
        Err(err) if is_unique_violation(&err) => Err(ObjectError::conflict(
            "this object already belongs to a series",
        )),
        Err(err) => Err(ObjectError::from(err)),
    }
}

async fn get_series(
    State(state): State<ObjectState>,
    Extension(principal): Extension<Principal>,
    Path(series_id): Path<Uuid>,
) -> Result<Json<SeriesDetailResponse>, ObjectError> {
    authorize_object_member(&principal)?;
    let org = principal.org_id;
    let scope = principal.branch_scope.clone();
    let caller = *principal.user_id.as_uuid();
    let satisfied = satisfied_features(&principal);
    let held_role_keys = crate::workflow_studio::held_authority_role_keys(
        &principal,
        org,
        crate::workflow_studio::WORKFLOW_SPINE_HAS_NO_BRANCH,
    );

    let detail = with_org_conn::<_, Option<SeriesDetailResponse>, ObjectError>(
        &state.pool,
        org,
        move |tx| {
            Box::pin(async move {
                let Some(series_row) =
                    sqlx::query("SELECT id, code, label, created_at FROM series WHERE id = $1")
                        .bind(series_id)
                        .fetch_optional(tx.as_mut())
                        .await?
                else {
                    return Ok(None);
                };
                let head = series_head_from_row(&series_row)?;
                let member_rows = sqlx::query(
                    "SELECT member_kind, member_id FROM series_instances \
                     WHERE series_id = $1 ORDER BY added_at, id",
                )
                .bind(series_id)
                .fetch_all(tx.as_mut())
                .await?;
                let mut instances = Vec::new();
                for row in &member_rows {
                    let member_kind: String = row.try_get("member_kind")?;
                    let member_id: String = row.try_get("member_id")?;
                    // Resolve each member under the caller's visibility; omit
                    // any that no longer resolve (deny-by-omission).
                    if let Some(fields) = resolve_head(
                        tx,
                        &scope,
                        caller,
                        &held_role_keys,
                        &satisfied,
                        &member_kind,
                        &member_id,
                    )
                    .await?
                    {
                        instances.push(object_head_from_resolved(
                            member_kind,
                            member_id,
                            Some(fields),
                        ));
                    }
                }
                Ok(Some(SeriesDetailResponse { head, instances }))
            })
        },
    )
    .await?;
    detail
        .map(Json)
        .ok_or_else(|| ObjectError::not_found("series not found"))
}

async fn series_by_instance(
    State(state): State<ObjectState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<SeriesByInstanceQuery>,
) -> Result<Json<SeriesByInstanceResponse>, ObjectError> {
    authorize_object_member(&principal)?;
    let member_kind = normalize_kind(&query.kind, "kind")?;
    let member_id = normalize_object_id(&query.id, "id")?;
    let org = principal.org_id;
    let scope = principal.branch_scope.clone();
    let caller = *principal.user_id.as_uuid();
    let satisfied = satisfied_features(&principal);
    let held_role_keys = crate::workflow_studio::held_authority_role_keys(
        &principal,
        org,
        crate::workflow_studio::WORKFLOW_SPINE_HAS_NO_BRANCH,
    );

    let series = with_org_conn::<_, Option<SeriesHead>, ObjectError>(&state.pool, org, move |tx| {
        Box::pin(async move {
            // Reveal the membership only if the caller can actually resolve the
            // instance — otherwise series membership would be an existence
            // oracle for objects outside the caller's scope.
            let resolved = resolve_head(
                tx,
                &scope,
                caller,
                &held_role_keys,
                &satisfied,
                &member_kind,
                &member_id,
            )
            .await?;
            if resolved.is_none() {
                return Ok(None);
            }
            let row = sqlx::query(
                r#"
                SELECT s.id, s.code, s.label, s.created_at
                FROM series_instances si
                JOIN series s ON s.id = si.series_id
                WHERE si.member_kind = $1 AND si.member_id = $2
                "#,
            )
            .bind(&member_kind)
            .bind(&member_id)
            .fetch_optional(tx.as_mut())
            .await?;
            match row {
                Some(row) => Ok(Some(series_head_from_row(&row)?)),
                None => Ok(None),
            }
        })
    })
    .await?;
    Ok(Json(SeriesByInstanceResponse { series }))
}

/// Confirm `link_type` is a registered edge type, inside the write tx so the
/// check and the insert are atomic. Turns the raw FK violation into a clean
/// validation error.
async fn ensure_link_type_exists(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    link_type: &str,
) -> Result<(), ObjectError> {
    let found: i64 = sqlx::query_scalar("SELECT count(*) FROM link_types WHERE link_type = $1")
        .bind(link_type)
        .fetch_one(tx.as_mut())
        .await?;
    if found == 1 {
        Ok(())
    } else {
        Err(ObjectError::validation(
            "link_type must be a registered edge type",
        ))
    }
}

fn parse_uuid(id: &str) -> Option<Uuid> {
    Uuid::parse_str(id.trim()).ok()
}

/// Branch-scope visibility. `All` sees everything; a bounded scope sees a row
/// only when its branch is in scope. A NULL branch (org-wide row) is visible to
/// any org member.
fn branch_visible(scope: &BranchScope, branch_id: Option<Uuid>) -> bool {
    match (scope, branch_id) {
        (BranchScope::All, _) => true,
        (BranchScope::Branches(_), None) => true,
        (BranchScope::Branches(set), Some(branch)) => set.contains(&BranchId::from_uuid(branch)),
    }
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// Deny creating a link whose endpoint the caller cannot actually see (B3):
/// once a link is stored, slice-2 denormalized-title / graph-traversal surfaces
/// turn it into a trust edge, so an edge must only ever connect objects the
/// caller can resolve. Reuses `resolve_head` — the SAME per-kind visibility/RLS
/// gate the resolve endpoint applies — so a missing OR out-of-scope endpoint is
/// rejected with one indistinguishable message (deny-by-omission: no
/// "absent" vs "not visible" oracle).
///
/// A kind with NO resolver (document/voucher/… — registered purely as a link
/// target) has no title/graph exposure to leak and cannot be visibility-checked
/// at this layer, so it passes through unchanged, exactly as before this guard.
async fn ensure_endpoint_resolvable(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &BranchScope,
    caller: Uuid,
    held_role_keys: &[String],
    satisfied_features: &[Feature],
    kind: &str,
    id: &str,
) -> Result<(), ObjectError> {
    if required_auth_for_kind(kind).is_none() {
        return Ok(());
    }
    let resolved = resolve_head(
        tx,
        scope,
        caller,
        held_role_keys,
        satisfied_features,
        kind,
        id,
    )
    .await?;
    if resolved.is_none() {
        return Err(ObjectError::validation(
            "src and dst must reference objects you can access",
        ));
    }
    Ok(())
}

/// Confirm both link endpoints reference a seeded kind, inside the write tx so
/// the check and the insert are atomic. Runs on `tx.as_mut()` (armed), never a
/// bare pool.
async fn ensure_kinds_exist(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    src_kind: &str,
    dst_kind: &str,
) -> Result<(), ObjectError> {
    let found: i64 =
        sqlx::query_scalar("SELECT COUNT(DISTINCT kind) FROM object_types WHERE kind = ANY($1)")
            .bind(vec![src_kind.to_owned(), dst_kind.to_owned()])
            .fetch_one(tx.as_mut())
            .await?;
    // Distinct kinds requested: 1 if src==dst (rejected earlier), else 2.
    let expected = if src_kind == dst_kind { 1 } else { 2 };
    if found == expected {
        Ok(())
    } else {
        Err(ObjectError::validation(
            "src_kind and dst_kind must be known object kinds",
        ))
    }
}

fn normalize_link(body: CreateObjectLinkRequest) -> Result<NormalizedLink, ObjectError> {
    let src_kind = normalize_kind(&body.src_kind, "src_kind")?;
    let dst_kind = normalize_kind(&body.dst_kind, "dst_kind")?;
    let src_id = normalize_object_id(&body.src_id, "src_id")?;
    let dst_id = normalize_object_id(&body.dst_id, "dst_id")?;
    let link_type = normalize_kind(&body.link_type, "link_type")?;
    if src_kind == dst_kind && src_id == dst_id {
        return Err(ObjectError::validation(
            "an object cannot be linked to itself",
        ));
    }
    Ok(NormalizedLink {
        src_kind,
        src_id,
        dst_kind,
        dst_id,
        link_type,
    })
}

/// Slug validation matching the DB CHECK (`^[a-z][a-z0-9_]{1,63}$`): a leading
/// lowercase letter then 1..63 more of lowercase/digit/underscore.
fn normalize_kind(raw: &str, field: &'static str) -> Result<String, ObjectError> {
    let value = raw.trim().to_ascii_lowercase();
    let mut chars = value.chars();
    let leads = chars.next().is_some_and(|ch| ch.is_ascii_lowercase());
    let rest_ok = chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_');
    if leads && rest_ok && (2..=64).contains(&value.len()) {
        Ok(value)
    } else {
        Err(ObjectError::validation(format!(
            "{field} is not a valid kind"
        )))
    }
}

fn normalize_object_id(raw: &str, field: &'static str) -> Result<String, ObjectError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(ObjectError::validation(format!("{field} is required")));
    }
    if value.chars().count() > ID_MAX {
        return Err(ObjectError::validation(format!(
            "{field} must be {ID_MAX} characters or less"
        )));
    }
    Ok(value.to_owned())
}

fn object_link_from_row(row: &sqlx::postgres::PgRow) -> Result<ObjectLinkResponse, ObjectError> {
    Ok(ObjectLinkResponse {
        id: row.try_get("id")?,
        src_kind: row.try_get("src_kind")?,
        src_id: row.try_get("src_id")?,
        dst_kind: row.try_get("dst_kind")?,
        dst_id: row.try_get("dst_id")?,
        link_type: row.try_get("link_type")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
    })
}

fn audit_delete_snapshot(before: &ObjectLinkResponse) -> Option<Value> {
    Some(json!({
        "id": before.id,
        "src_kind": before.src_kind,
        "src_id": before.src_id,
        "dst_kind": before.dst_kind,
        "dst_id": before.dst_id,
        "link_type": before.link_type,
    }))
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error().and_then(|db| db.code()).as_deref() == Some("23505")
}

fn authorize_object_member(principal: &Principal) -> Result<(), ObjectError> {
    authorize_object_feature(principal, Feature::Login).map_err(|_| {
        ObjectError::from_kernel(KernelError::forbidden(
            "object links require an authenticated tenant member",
        ))
    })
}

fn authorize_object_feature(principal: &Principal, feature: Feature) -> Result<(), ObjectError> {
    let allowed_by_role = principal
        .roles
        .iter()
        .any(|role| permission_for(*role, feature) == PermissionLevel::Allow);
    let allowed_by_custom_grant = principal
        .effective_feature_grants
        .iter()
        .any(|grant| grant.feature == feature && grant.permission == PermissionLevel::Allow);
    if allowed_by_role || allowed_by_custom_grant {
        return Ok(());
    }
    Err(ObjectError::from_kernel(KernelError::forbidden(
        "insufficient permission for this object kind",
    )))
}

// ---------------------------------------------------------------------------
// Error type (mirrors the collaboration module's shape).
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ObjectError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ObjectError {
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

    fn validation(message: impl Into<String>) -> Self {
        Self::from_kernel(KernelError::validation(message.into()))
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::from_kernel(KernelError::not_found(message.into()))
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self::from_kernel(KernelError::conflict(message.into()))
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: message.into(),
        }
    }
}

impl From<KernelError> for ObjectError {
    fn from(error: KernelError) -> Self {
        Self::from_kernel(error)
    }
}

impl From<DbError> for ObjectError {
    fn from(value: DbError) -> Self {
        tracing::error!(error = %value, "object-layer database operation failed");
        Self::internal("object-layer request failed")
    }
}

impl From<sqlx::Error> for ObjectError {
    fn from(value: sqlx::Error) -> Self {
        Self::from(DbError::Sqlx(value))
    }
}

impl IntoResponse for ObjectError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response()
    }
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
    use super::*;

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn rejects_self_link_and_bad_kinds() {
        let self_link = CreateObjectLinkRequest {
            src_kind: "work_order".into(),
            src_id: "wo-1".into(),
            dst_kind: "work_order".into(),
            dst_id: "wo-1".into(),
            link_type: "relates_to".into(),
        };
        assert!(normalize_link(self_link).is_err());

        assert!(normalize_kind("Work Order", "src_kind").is_err());
        assert!(normalize_kind("1bad", "src_kind").is_err());
        assert!(normalize_kind("work_order", "src_kind").is_ok());
        assert!(normalize_object_id("  ", "id").is_err());
    }

    /// Every kind `resolve_head` can dispatch MUST have a declared auth in
    /// `RESOLVABLE_KIND_AUTH`, and every declared kind must be dispatchable —
    /// the two cannot drift. This is the structural guarantee that a new
    /// resolvable kind cannot ship gated only at Login by a default fall-through
    /// (the #220 / #239 bug class): visibility now requires a declared auth
    /// decision, and this test fails loudly if a resolver arm is added without
    /// one (or a declaration is added without a resolver).
    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn resolvable_kinds_and_declared_auth_stay_in_sync() {
        // Mirror of resolve_head's dispatch arms.
        const DISPATCHED_KINDS: &[&str] = &[
            "work_order",
            "equipment",
            "support_ticket",
            "org_unit",
            "person",
            "approval_run",
            "account",
            "passkey",
            "consent",
            "series",
        ];
        for kind in DISPATCHED_KINDS {
            assert!(
                required_auth_for_kind(kind).is_some(),
                "dispatchable kind `{kind}` has no RESOLVABLE_KIND_AUTH entry"
            );
        }
        for (kind, _) in RESOLVABLE_KIND_AUTH {
            assert!(
                DISPATCHED_KINDS.contains(kind),
                "RESOLVABLE_KIND_AUTH declares `{kind}` but resolve_head has no arm"
            );
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn count_kind_and_resolvable_kinds_stay_in_sync() {
        // Mirror of count_kind's resolvable match arms. Non-resolvable registry
        // kinds are intentionally absent because they count 0, but every
        // resolve_head-dispatchable kind must have a count arm (even when the arm
        // returns 0 for a caller lacking the required feature).
        const COUNTED_RESOLVABLE_KINDS: &[&str] = &[
            "work_order",
            "equipment",
            "support_ticket",
            "org_unit",
            "person",
            "account",
            "approval_run",
            "passkey",
            "consent",
            "series",
        ];
        for (kind, _) in RESOLVABLE_KIND_AUTH {
            assert!(
                COUNTED_RESOLVABLE_KINDS.contains(kind),
                "RESOLVABLE_KIND_AUTH declares `{kind}` but count_kind has no arm"
            );
        }
        for kind in COUNTED_RESOLVABLE_KINDS {
            assert!(
                required_auth_for_kind(kind).is_some(),
                "count_kind has an arm for `{kind}` but resolve_head cannot dispatch it"
            );
        }
    }
}
