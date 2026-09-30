//! REST API for Location Information Act consent controls.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod openapi;
pub use openapi::OPENAPI_FRAGMENT;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_compliance_adapter_postgres::{PgComplianceError, PgComplianceStore};
use console_compliance_application::{
    AcceptEvidenceBindingCommand, ArrivalEventPage, ArrivalEventQuery, AuditStreamPage,
    AuditStreamQuery, AuditStreamReadKind, CEO_COVERT_AUDIT_STREAM_KEY, ComplianceControlQuery,
    ComplianceFrameworkQuery, ComplianceObligationQuery, ConsentTransitionCommand,
    ConsentTransitionKind, CreateComplianceControlCommand, CreateComplianceFrameworkCommand,
    CreateComplianceObligationCommand, CreateEvidenceBindingCommand, CreateRegulationImpactCommand,
    EvidenceBindingQuery, LinkControlObligationCommand, LinkObligationRegulationCommand,
    LocationConsentLedgerEntry, LocationConsentLedgerPage, LocationConsentLedgerQuery, PageRequest,
    RegulationImpactQuery,
};
use console_compliance_domain::{
    ComplianceRiskLevel, ComplianceScope, ComplianceScopeKind, ControlCadence, ControlStatus,
    ControlType, CoverageLevel, EvidenceBindingStatus, EvidenceConfidence, EvidenceTargetType,
    FrameworkKind, FrameworkStatus, LocationConsent, LocationConsentState, LocationPing,
    ObligationRegulationRelationship, ObligationStatus, ObligationType, RegulationImpactStatus,
    ReviewCadence,
};
use console_kernel_core::{
    BranchId, BranchScope, ErrorKind, KernelError, LocationPingId, Timestamp, TraceContext, UserId,
};
use console_platform_auth::JwtVerifier;
use console_platform_authz::cedar_pbac::engine;
use console_platform_authz::{
    Action, AuthorizationContext, AuthorizationRequest, AuthorizationResource, CoexistenceMapEntry,
    DualEngineMode, Feature, Principal, RlsScopeProof, SubjectFreshnessRequirement, authorize,
    authorize_org_wide, evaluate_cedar_pbac_boundary,
};
use serde::{Deserialize, Serialize};

pub const COMPLIANCE_ROUTE_PATHS: &[&str] = &[
    "/api/v1/compliance/regulations",
    "/api/v1/compliance/obligations",
    "/api/v1/compliance/obligation-regulation-links",
    "/api/v1/compliance/frameworks",
    "/api/v1/compliance/framework-controls",
    "/api/v1/compliance/control-obligation-coverage",
    "/api/v1/compliance/evidence-bindings",
    "/api/v1/compliance/evidence-bindings/{id}/accept",
    "/api/v1/location-consent/status",
    "/api/v1/location-consent/grant",
    "/api/v1/location-consent/suspend",
    "/api/v1/location-consent/resume",
    "/api/v1/location-consent/withdraw",
    "/api/v1/location-pings",
    "/api/v1/location-consents/ledger",
    "/api/v1/location-consents/ledger.csv",
    "/api/v1/location/arrival-events",
    "/api/v1/audit-streams/ceo-covert/events",
    "/api/v1/audit-streams/ceo-covert/access-events",
];

#[derive(Clone)]
pub struct ComplianceRestState {
    store: PgComplianceStore,
    jwt_verifier: Option<JwtVerifier>,
}

impl ComplianceRestState {
    #[must_use]
    pub fn new(store: PgComplianceStore, jwt_verifier: Option<JwtVerifier>) -> Self {
        Self {
            store,
            jwt_verifier,
        }
    }
}

pub fn router(state: ComplianceRestState) -> Router {
    let verifier = state.jwt_verifier.clone();
    let pool = state.store.pool().clone();
    let router = Router::new()
        .route(
            "/api/v1/compliance/regulations",
            get(list_regulations).post(create_regulation),
        )
        .route(
            "/api/v1/compliance/obligations",
            get(list_obligations).post(create_obligation),
        )
        .route(
            "/api/v1/compliance/obligation-regulation-links",
            post(link_obligation_regulation),
        )
        .route(
            "/api/v1/compliance/frameworks",
            get(list_frameworks).post(create_framework),
        )
        .route(
            "/api/v1/compliance/framework-controls",
            get(list_framework_controls).post(create_framework_control),
        )
        .route(
            "/api/v1/compliance/control-obligation-coverage",
            post(link_control_obligation),
        )
        .route(
            "/api/v1/compliance/evidence-bindings",
            get(list_evidence_bindings).post(create_evidence_binding),
        )
        .route(
            "/api/v1/compliance/evidence-bindings/{id}/accept",
            post(accept_evidence_binding),
        )
        .route("/api/v1/location-consent/status", get(get_status))
        .route("/api/v1/location-consent/grant", post(grant_consent))
        .route("/api/v1/location-consent/suspend", post(suspend_consent))
        .route("/api/v1/location-consent/resume", post(resume_consent))
        .route("/api/v1/location-consent/withdraw", post(withdraw_consent))
        .route("/api/v1/location-pings", post(record_location_ping))
        .route("/api/v1/location-consents/ledger", get(list_ledger))
        .route(
            "/api/v1/location-consents/ledger.csv",
            get(export_ledger_csv),
        )
        .route("/api/v1/location/arrival-events", get(list_arrival_events))
        .route(
            "/api/v1/audit-streams/ceo-covert/events",
            get(list_ceo_covert_audit_events),
        )
        .route(
            "/api/v1/audit-streams/ceo-covert/access-events",
            get(list_ceo_covert_audit_access_events),
        )
        .with_state(state);
    console_platform_request_context::with_request_context(router, verifier, pool)
}

/// All compliance catalog writes derive the actor, trace, timestamp, and tenant
/// from authenticated request context. Callers may choose a business owner, but
/// may never impersonate the actor or select an organization.
#[derive(Debug, Deserialize)]
struct RegulationQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    q: Option<String>,
    status: Option<RegulationImpactStatus>,
    risk_level: Option<ComplianceRiskLevel>,
}

#[derive(Debug, Deserialize)]
struct ObligationQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    q: Option<String>,
    status: Option<ObligationStatus>,
    severity: Option<ComplianceRiskLevel>,
    scope_type: Option<ComplianceScopeKind>,
    branch_id: Option<BranchId>,
    site_id: Option<console_kernel_core::SiteId>,
}

#[derive(Debug, Deserialize)]
struct FrameworkQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    q: Option<String>,
    status: Option<FrameworkStatus>,
    kind: Option<FrameworkKind>,
}

#[derive(Debug, Deserialize)]
struct ControlQuery {
    framework_id: String,
    limit: Option<i64>,
    offset: Option<i64>,
    q: Option<String>,
    status: Option<ControlStatus>,
}

#[derive(Debug, Deserialize)]
struct EvidenceQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    control_id: Option<String>,
    obligation_id: Option<String>,
    target_type: Option<EvidenceTargetType>,
    status: Option<EvidenceBindingStatus>,
}

#[derive(Debug, Deserialize)]
struct CreateRegulationRequest {
    title: String,
    jurisdiction: String,
    regulator: Option<String>,
    citation: String,
    source_url: Option<String>,
    impact_area: String,
    impact_summary: String,
    risk_level: ComplianceRiskLevel,
    effective_from: Option<time::Date>,
    effective_to: Option<time::Date>,
    review_due_on: Option<time::Date>,
    owner_user_id: Option<UserId>,
    #[serde(default)]
    metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct RegulationLinkRequest {
    regulation_impact_id: String,
    relationship: ObligationRegulationRelationship,
    rationale: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateObligationRequest {
    title: String,
    description: String,
    obligation_type: ObligationType,
    scope: ComplianceScope,
    owner_user_id: Option<UserId>,
    severity: ComplianceRiskLevel,
    effective_from: Option<time::Date>,
    effective_to: Option<time::Date>,
    review_cadence: Option<ReviewCadence>,
    next_review_on: Option<time::Date>,
    #[serde(default)]
    metadata: serde_json::Value,
    #[serde(default)]
    regulation_links: Vec<RegulationLinkRequest>,
}

#[derive(Debug, Deserialize)]
struct LinkObligationRegulationRequest {
    obligation_id: String,
    regulation_impact_id: String,
    relationship: ObligationRegulationRelationship,
    rationale: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateFrameworkRequest {
    name: String,
    version_label: String,
    framework_kind: FrameworkKind,
    owner_user_id: Option<UserId>,
    effective_from: Option<time::Date>,
    effective_to: Option<time::Date>,
    #[serde(default)]
    metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct CreateControlRequest {
    framework_id: String,
    control_key: String,
    title: String,
    objective: String,
    control_type: ControlType,
    cadence: Option<ControlCadence>,
    #[serde(default)]
    evidence_requirements: serde_json::Value,
    owner_user_id: Option<UserId>,
}

#[derive(Debug, Deserialize)]
struct LinkControlObligationRequest {
    control_id: String,
    obligation_id: String,
    coverage_level: CoverageLevel,
    coverage_rationale: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateEvidenceBindingRequest {
    control_id: String,
    obligation_id: Option<String>,
    evidence_target_type: EvidenceTargetType,
    evidence_target_id: String,
    source_audit_event_id: Option<String>,
    confidence: EvidenceConfidence,
    #[serde(with = "time::serde::rfc3339::option")]
    collected_at: Option<Timestamp>,
    collected_by: Option<UserId>,
    valid_from: Option<time::Date>,
    valid_to: Option<time::Date>,
    hash_sha256: Option<String>,
    #[serde(default)]
    metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct StatusQuery {
    branch_id: Option<BranchId>,
}

#[derive(Debug, Deserialize)]
struct TransitionRequest {
    branch_id: Option<BranchId>,
}

#[derive(Debug, Deserialize)]
struct LocationPingRequest {
    branch_id: Option<BranchId>,
    latitude: f64,
    longitude: f64,
    accuracy_m: Option<f64>,
    // OpenAPI exposes this as a date-time string. The default OffsetDateTime
    // serde representation is an internal numeric tuple and rejects the ISO
    // 8601 value sent by mobile clients.
    #[serde(with = "time::serde::rfc3339")]
    recorded_at: Timestamp,
    on_duty: bool,
}

#[derive(Debug, Deserialize)]
struct LedgerRequest {
    user_id: Option<UserId>,
    branch_id: Option<BranchId>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct AuditStreamRequest {
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Serialize)]
struct LocationConsentStatusResponse {
    consent_id: String,
    user_id: UserId,
    branch_id: BranchId,
    state: LocationConsentState,
    may_collect: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    granted_at: Option<Timestamp>,
    #[serde(with = "time::serde::rfc3339::option")]
    suspended_at: Option<Timestamp>,
    #[serde(with = "time::serde::rfc3339::option")]
    resumed_at: Option<Timestamp>,
    #[serde(with = "time::serde::rfc3339::option")]
    withdrawn_at: Option<Timestamp>,
    #[serde(with = "time::serde::rfc3339::option")]
    updated_at: Option<Timestamp>,
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

/// Compliance catalog authorization distinguishes tenant-wide rows from
/// branch-scoped obligations. Never authorize a tenant-wide resource against an
/// arbitrary branch from a multi-branch principal.
fn require_compliance_read(principal: &Principal) -> Result<(), RestError> {
    require_org_catalog_feature(principal, Feature::ComplianceDomainRead)
}

fn require_compliance_manage(principal: &Principal) -> Result<(), RestError> {
    require_org_catalog_feature(principal, Feature::ComplianceDomainManage)
}

fn require_compliance_evidence_link(principal: &Principal) -> Result<(), RestError> {
    require_org_catalog_feature(principal, Feature::ComplianceEvidenceLink)
}

fn require_evidence_link_with_read(principal: &Principal) -> Result<(), RestError> {
    require_compliance_read(principal)?;
    require_compliance_evidence_link(principal)
}

fn require_evidence_accept_with_read(principal: &Principal) -> Result<(), RestError> {
    require_compliance_read(principal)?;
    require_compliance_manage(principal)
}

fn require_org_catalog_feature(principal: &Principal, feature: Feature) -> Result<(), RestError> {
    match &principal.branch_scope {
        BranchScope::All => authorize_org_wide(principal, Action::new(feature)),
        BranchScope::Branches(_) => Err(KernelError::forbidden(
            "org-wide compliance catalog access requires an org-wide principal",
        )),
    }
    .map_err(RestError::from_kernel)
}

fn require_branch_compliance_feature(
    principal: &Principal,
    feature: Feature,
    branch_id: BranchId,
) -> Result<(), RestError> {
    authorize(principal, Action::new(feature), branch_id).map_err(RestError::from_kernel)
}

fn catalog_page(limit: Option<i64>, offset: Option<i64>) -> Result<PageRequest, RestError> {
    PageRequest::new(limit, offset).map_err(RestError::from_kernel)
}

fn require_obligation_scope_manage(
    principal: &Principal,
    scope: ComplianceScope,
) -> Result<(), RestError> {
    if let Some(branch_id) = scope.branch_id {
        return require_branch_compliance_feature(
            principal,
            Feature::ComplianceDomainManage,
            branch_id,
        );
    }
    require_compliance_manage(principal)
}

async fn list_regulations(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Query(query): Query<RegulationQuery>,
) -> Result<Json<console_compliance_application::RegulationImpactPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_compliance_read(&principal)?;
    let q = query.q.clone();
    let page = catalog_page(query.limit, query.offset)?;
    let result = state
        .store
        .list_regulation_impacts(RegulationImpactQuery {
            status: query.status,
            risk_level: query.risk_level,
            q,
            page,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(result))
}

async fn create_regulation(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateRegulationRequest>,
) -> Result<Json<console_compliance_domain::RegulationImpact>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_compliance_manage(&principal)?;
    let result = state
        .store
        .create_regulation_impact(CreateRegulationImpactCommand {
            actor: principal.user_id,
            title: body.title,
            jurisdiction: body.jurisdiction,
            regulator: body.regulator,
            citation: body.citation,
            source_url: body.source_url,
            impact_area: body.impact_area,
            impact_summary: body.impact_summary,
            risk_level: body.risk_level,
            effective_from: body.effective_from,
            effective_to: body.effective_to,
            review_due_on: body.review_due_on,
            owner_user_id: body.owner_user_id,
            metadata: body.metadata,
            trace: current_trace_context(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(result))
}

async fn list_obligations(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Query(query): Query<ObligationQuery>,
) -> Result<Json<console_compliance_application::ComplianceObligationPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    if let Some(branch_id) = query.branch_id {
        require_branch_compliance_feature(&principal, Feature::ComplianceDomainRead, branch_id)?;
    } else {
        // Without a branch predicate this store query can return every
        // obligation in the tenant, including branch-scoped rows.
        require_compliance_read(&principal)?;
    }
    let q = query.q.clone();
    let page = catalog_page(query.limit, query.offset)?;
    let result = state
        .store
        .list_compliance_obligations(ComplianceObligationQuery {
            branch_scope: principal.branch_scope,
            status: query.status,
            severity: query.severity,
            scope_type: query.scope_type,
            branch_id: query.branch_id,
            site_id: query.site_id,
            q,
            page,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(result))
}

async fn create_obligation(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateObligationRequest>,
) -> Result<Json<console_compliance_domain::ComplianceObligation>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_obligation_scope_manage(&principal, body.scope)?;
    let regulation_links = body
        .regulation_links
        .into_iter()
        .map(|link| {
            Ok(
                console_compliance_application::CreateObligationRegulationLink {
                    regulation_impact_id: link.regulation_impact_id.parse().map_err(|_| {
                        RestError::from_kernel(KernelError::validation(
                            "regulation_impact_id must be a UUID",
                        ))
                    })?,
                    relationship: link.relationship,
                    rationale: link.rationale,
                },
            )
        })
        .collect::<Result<Vec<_>, RestError>>()?;
    let result = state
        .store
        .create_compliance_obligation(CreateComplianceObligationCommand {
            actor: principal.user_id,
            title: body.title,
            description: body.description,
            obligation_type: body.obligation_type,
            scope: body.scope,
            owner_user_id: body.owner_user_id,
            severity: body.severity,
            effective_from: body.effective_from,
            effective_to: body.effective_to,
            review_cadence: body.review_cadence,
            next_review_on: body.next_review_on,
            metadata: body.metadata,
            regulation_links,
            trace: current_trace_context(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(result))
}

async fn link_obligation_regulation(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Json(body): Json<LinkObligationRegulationRequest>,
) -> Result<Json<console_compliance_domain::ObligationRegulationLink>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_compliance_manage(&principal)?;
    let result = state
        .store
        .link_obligation_regulation(LinkObligationRegulationCommand {
            actor: principal.user_id,
            obligation_id: body.obligation_id.parse().map_err(|_| {
                RestError::from_kernel(KernelError::validation("obligation_id must be a UUID"))
            })?,
            regulation_impact_id: body.regulation_impact_id.parse().map_err(|_| {
                RestError::from_kernel(KernelError::validation(
                    "regulation_impact_id must be a UUID",
                ))
            })?,
            relationship: body.relationship,
            rationale: body.rationale,
            trace: current_trace_context(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(result))
}

async fn list_frameworks(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Query(query): Query<FrameworkQuery>,
) -> Result<Json<console_compliance_application::ComplianceFrameworkPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_compliance_read(&principal)?;
    let q = query.q.clone();
    let page = catalog_page(query.limit, query.offset)?;
    let result = state
        .store
        .list_compliance_frameworks(ComplianceFrameworkQuery {
            status: query.status,
            kind: query.kind,
            q,
            page,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(result))
}

async fn create_framework(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateFrameworkRequest>,
) -> Result<Json<console_compliance_domain::ComplianceFramework>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_compliance_manage(&principal)?;
    let result = state
        .store
        .create_compliance_framework(CreateComplianceFrameworkCommand {
            actor: principal.user_id,
            name: body.name,
            version_label: body.version_label,
            framework_kind: body.framework_kind,
            owner_user_id: body.owner_user_id,
            effective_from: body.effective_from,
            effective_to: body.effective_to,
            metadata: body.metadata,
            trace: current_trace_context(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(result))
}

async fn list_framework_controls(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Query(query): Query<ControlQuery>,
) -> Result<Json<console_compliance_application::ComplianceControlPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_compliance_read(&principal)?;
    let framework_id = query.framework_id.parse().map_err(|_| {
        RestError::from_kernel(KernelError::validation("framework_id must be a UUID"))
    })?;
    let q = query.q.clone();
    let page = catalog_page(query.limit, query.offset)?;
    let result = state
        .store
        .list_compliance_controls(ComplianceControlQuery {
            framework_id,
            status: query.status,
            q,
            page,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(result))
}

async fn create_framework_control(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateControlRequest>,
) -> Result<Json<console_compliance_domain::ComplianceControl>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_compliance_manage(&principal)?;
    let result = state
        .store
        .create_compliance_control(CreateComplianceControlCommand {
            actor: principal.user_id,
            framework_id: body.framework_id.parse().map_err(|_| {
                RestError::from_kernel(KernelError::validation("framework_id must be a UUID"))
            })?,
            control_key: body.control_key,
            title: body.title,
            objective: body.objective,
            control_type: body.control_type,
            cadence: body.cadence,
            evidence_requirements: body.evidence_requirements,
            owner_user_id: body.owner_user_id,
            trace: current_trace_context(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(result))
}

async fn link_control_obligation(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Json(body): Json<LinkControlObligationRequest>,
) -> Result<Json<console_compliance_domain::ControlObligationCoverage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_compliance_manage(&principal)?;
    let result = state
        .store
        .link_control_obligation(LinkControlObligationCommand {
            actor: principal.user_id,
            control_id: body.control_id.parse().map_err(|_| {
                RestError::from_kernel(KernelError::validation("control_id must be a UUID"))
            })?,
            obligation_id: body.obligation_id.parse().map_err(|_| {
                RestError::from_kernel(KernelError::validation("obligation_id must be a UUID"))
            })?,
            coverage_level: body.coverage_level,
            coverage_rationale: body.coverage_rationale,
            trace: current_trace_context(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(result))
}

async fn list_evidence_bindings(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Query(query): Query<EvidenceQuery>,
) -> Result<Json<console_compliance_application::EvidenceBindingPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_compliance_read(&principal)?;
    let control_id = query
        .control_id
        .map(|id| {
            id.parse().map_err(|_| {
                RestError::from_kernel(KernelError::validation("control_id must be a UUID"))
            })
        })
        .transpose()?;
    let obligation_id = query
        .obligation_id
        .map(|id| {
            id.parse().map_err(|_| {
                RestError::from_kernel(KernelError::validation("obligation_id must be a UUID"))
            })
        })
        .transpose()?;
    let page = catalog_page(query.limit, query.offset)?;
    let result = state
        .store
        .list_evidence_bindings(EvidenceBindingQuery {
            control_id,
            obligation_id,
            target_type: query.target_type,
            status: query.status,
            page,
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(result))
}

/// Accept a proposed evidence binding. This is an explicit lifecycle action,
/// not a generic catalog update: the binding's evidence identity/provenance
/// stays immutable and the adapter records the audited status edge atomically.
async fn accept_evidence_binding(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<console_compliance_domain::EvidenceBinding>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_evidence_accept_with_read(&principal)?;
    let id = id.parse().map_err(|_| {
        RestError::from_kernel(KernelError::validation(
            "evidence binding id must be a UUID",
        ))
    })?;
    let result = state
        .store
        .accept_evidence_binding(AcceptEvidenceBindingCommand {
            actor: principal.user_id,
            id,
            trace: current_trace_context(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(result))
}

async fn create_evidence_binding(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Json(body): Json<CreateEvidenceBindingRequest>,
) -> Result<Json<console_compliance_domain::EvidenceBinding>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    require_evidence_link_with_read(&principal)?;
    let result = state
        .store
        .create_evidence_binding(CreateEvidenceBindingCommand {
            actor: principal.user_id,
            control_id: body.control_id.parse().map_err(|_| {
                RestError::from_kernel(KernelError::validation("control_id must be a UUID"))
            })?,
            obligation_id: body
                .obligation_id
                .map(|id| {
                    id.parse().map_err(|_| {
                        RestError::from_kernel(KernelError::validation(
                            "obligation_id must be a UUID",
                        ))
                    })
                })
                .transpose()?,
            evidence_target_type: body.evidence_target_type,
            evidence_target_id: body.evidence_target_id,
            source_audit_event_id: body
                .source_audit_event_id
                .map(|id| {
                    id.parse().map_err(|_| {
                        RestError::from_kernel(KernelError::validation(
                            "source_audit_event_id must be a UUID",
                        ))
                    })
                })
                .transpose()?,
            confidence: body.confidence,
            collected_at: body.collected_at,
            collected_by: body.collected_by,
            valid_from: body.valid_from,
            valid_to: body.valid_to,
            hash_sha256: body.hash_sha256,
            metadata: body.metadata,
            trace: current_trace_context(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(result))
}

async fn get_status(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Query(query): Query<StatusQuery>,
) -> Result<Json<LocationConsentStatusResponse>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let branch_id = resolve_requested_branch(&principal, query.branch_id)?;
    authorize(&principal, Action::new(Feature::Login), branch_id)
        .map_err(RestError::from_kernel)?;
    let consent = state
        .store
        .current_consent(principal.user_id, branch_id)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(status_response(consent)))
}

async fn grant_consent(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Json(body): Json<TransitionRequest>,
) -> Result<Json<LocationConsentStatusResponse>, RestError> {
    transition_consent(state, headers, body, ConsentTransitionKind::Grant).await
}

async fn suspend_consent(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Json(body): Json<TransitionRequest>,
) -> Result<Json<LocationConsentStatusResponse>, RestError> {
    transition_consent(state, headers, body, ConsentTransitionKind::Suspend).await
}

async fn resume_consent(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Json(body): Json<TransitionRequest>,
) -> Result<Json<LocationConsentStatusResponse>, RestError> {
    transition_consent(state, headers, body, ConsentTransitionKind::Resume).await
}

async fn withdraw_consent(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Json(body): Json<TransitionRequest>,
) -> Result<Json<LocationConsentStatusResponse>, RestError> {
    transition_consent(state, headers, body, ConsentTransitionKind::Withdraw).await
}

async fn transition_consent(
    state: ComplianceRestState,
    headers: HeaderMap,
    body: TransitionRequest,
    kind: ConsentTransitionKind,
) -> Result<Json<LocationConsentStatusResponse>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let branch_id = resolve_requested_branch(&principal, body.branch_id)?;
    authorize(&principal, Action::new(Feature::Login), branch_id)
        .map_err(RestError::from_kernel)?;
    let consent = state
        .store
        .transition_consent(ConsentTransitionCommand {
            kind,
            actor: Some(principal.user_id),
            user_id: principal.user_id,
            branch_id,
            trace: current_trace_context(),
            occurred_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(status_response(consent)))
}

// The HTTP surface only validates input and delegates; the audit carve-out is
// bound to the REAL writer (compliance adapter-postgres `record_location_ping`),
// so this handler carries no audit-exempt marker.
async fn record_location_ping(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Json(body): Json<LocationPingRequest>,
) -> Result<StatusCode, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let branch_id = resolve_requested_branch(&principal, body.branch_id)?;
    authorize(&principal, Action::new(Feature::Login), branch_id)
        .map_err(RestError::from_kernel)?;
    let ping = LocationPing::new(
        LocationPingId::new(),
        principal.user_id,
        branch_id,
        body.latitude,
        body.longitude,
        body.accuracy_m,
        body.recorded_at,
        body.on_duty,
    )
    .map_err(RestError::from_kernel)?;
    state
        .store
        .record_location_ping(ping)
        .await
        .map_err(RestError::from_store)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_ledger(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Query(query): Query<LedgerRequest>,
) -> Result<Json<LocationConsentLedgerPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_ledger_read(&principal, query.branch_id)?;
    let page = state
        .store
        .list_location_consent_ledger(&principal.branch_scope, normalize_ledger_query(query)?)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page))
}

async fn export_ledger_csv(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Query(query): Query<LedgerRequest>,
) -> Result<Response, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_ledger_read(&principal, query.branch_id)?;
    let page = state
        .store
        .list_location_consent_ledger(&principal.branch_scope, normalize_ledger_query(query)?)
        .await
        .map_err(RestError::from_store)?;

    let mut response = ledger_csv(&page.items).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"location-consent-ledger.csv\""),
    );
    Ok(response)
}

fn status_response(consent: LocationConsent) -> LocationConsentStatusResponse {
    LocationConsentStatusResponse {
        consent_id: consent.id().to_string(),
        user_id: consent.user_id(),
        branch_id: consent.branch_id(),
        state: consent.state(),
        may_collect: consent.state() == LocationConsentState::Granted,
        granted_at: consent.granted_at(),
        suspended_at: consent.suspended_at(),
        resumed_at: consent.resumed_at(),
        withdrawn_at: consent.withdrawn_at(),
        updated_at: consent.updated_at(),
    }
}

fn normalize_ledger_query(query: LedgerRequest) -> Result<LocationConsentLedgerQuery, RestError> {
    let limit = query.limit.unwrap_or(100).clamp(1, 1_000);
    let offset = query.offset.unwrap_or(0);
    if offset < 0 {
        return Err(RestError::from_kernel(KernelError::validation(
            "offset must be non-negative",
        )));
    }

    Ok(LocationConsentLedgerQuery {
        user_id: query.user_id,
        branch_id: query.branch_id,
        limit,
        offset,
    })
}

fn authorize_ledger_read(
    principal: &Principal,
    branch_id: Option<BranchId>,
) -> Result<(), RestError> {
    match branch_id {
        Some(branch_id) => authorize(principal, Action::new(Feature::AuditLogRead), branch_id)
            .map_err(RestError::from_kernel),
        None => match &principal.branch_scope {
            BranchScope::All => authorize_org_wide(principal, Action::new(Feature::AuditLogRead))
                .map_err(RestError::from_kernel),
            BranchScope::Branches(branches) if branches.len() == 1 => {
                let Some(branch_id) = branches.iter().copied().next() else {
                    return Err(RestError::from_kernel(KernelError::validation(
                        "branch_id is required",
                    )));
                };
                authorize(principal, Action::new(Feature::AuditLogRead), branch_id)
                    .map_err(RestError::from_kernel)
            }
            BranchScope::Branches(_) => Err(RestError::from_kernel(KernelError::validation(
                "branch_id is required for multi-branch ledger reads",
            ))),
        },
    }
}

/// GET /api/v1/location/arrival-events — the ops-facing site arrival/departure
/// feed (issue #13). Tenant-scoped + branch-filtered, OpsDashboardRead-gated.
async fn list_arrival_events(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Query(query): Query<LedgerRequest>,
) -> Result<Json<ArrivalEventPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    authorize_arrival_read(&principal, query.branch_id)?;
    let page = state
        .store
        .list_arrival_events(&principal.branch_scope, normalize_arrival_query(query)?)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page))
}

fn normalize_arrival_query(query: LedgerRequest) -> Result<ArrivalEventQuery, RestError> {
    let limit = query.limit.unwrap_or(100).clamp(1, 1_000);
    let offset = query.offset.unwrap_or(0);
    if offset < 0 {
        return Err(RestError::from_kernel(KernelError::validation(
            "offset must be non-negative",
        )));
    }
    Ok(ArrivalEventQuery {
        user_id: query.user_id,
        branch_id: query.branch_id,
        limit,
        offset,
    })
}

fn authorize_arrival_read(
    principal: &Principal,
    branch_id: Option<BranchId>,
) -> Result<(), RestError> {
    match branch_id {
        Some(branch_id) => authorize(principal, Action::new(Feature::OpsDashboardRead), branch_id)
            .map_err(RestError::from_kernel),
        None => match &principal.branch_scope {
            BranchScope::All => {
                authorize_org_wide(principal, Action::new(Feature::OpsDashboardRead))
                    .map_err(RestError::from_kernel)
            }
            BranchScope::Branches(branches) if branches.len() == 1 => {
                let Some(branch_id) = branches.iter().copied().next() else {
                    return Err(RestError::from_kernel(KernelError::validation(
                        "branch_id is required",
                    )));
                };
                authorize(principal, Action::new(Feature::OpsDashboardRead), branch_id)
                    .map_err(RestError::from_kernel)
            }
            BranchScope::Branches(_) => Err(RestError::from_kernel(KernelError::validation(
                "branch_id is required for multi-branch arrival reads",
            ))),
        },
    }
}

async fn list_ceo_covert_audit_events(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Query(query): Query<AuditStreamRequest>,
) -> Result<Json<AuditStreamPage>, RestError> {
    list_ceo_covert_audit_stream(state, headers, query, AuditStreamReadKind::Events).await
}

async fn list_ceo_covert_audit_access_events(
    State(state): State<ComplianceRestState>,
    headers: HeaderMap,
    Query(query): Query<AuditStreamRequest>,
) -> Result<Json<AuditStreamPage>, RestError> {
    list_ceo_covert_audit_stream(state, headers, query, AuditStreamReadKind::AccessEvents).await
}

async fn list_ceo_covert_audit_stream(
    state: ComplianceRestState,
    headers: HeaderMap,
    query: AuditStreamRequest,
    read_kind: AuditStreamReadKind,
) -> Result<Json<AuditStreamPage>, RestError> {
    let principal = principal_from_headers(&state, &headers).await?;
    let query = normalize_audit_stream_query(query)?;
    authorize_ceo_covert_audit_stream(&state, &principal, read_kind, &query).await?;
    let page = state
        .store
        .list_ceo_covert_audit_stream(principal.user_id, read_kind, query)
        .await
        .map_err(RestError::from_store)?;
    Ok(Json(page))
}

fn normalize_audit_stream_query(query: AuditStreamRequest) -> Result<AuditStreamQuery, RestError> {
    let limit = query.limit.unwrap_or(100).clamp(1, 1_000);
    let offset = query.offset.unwrap_or(0);
    if offset < 0 {
        return Err(RestError::from_kernel(KernelError::validation(
            "offset must be non-negative",
        )));
    }
    Ok(AuditStreamQuery {
        limit,
        offset,
        purpose: "ceo_covert_audit_review".to_owned(),
        channel: "web".to_owned(),
    })
}

async fn authorize_ceo_covert_audit_stream(
    state: &ComplianceRestState,
    principal: &Principal,
    read_kind: AuditStreamReadKind,
    query: &AuditStreamQuery,
) -> Result<(), RestError> {
    let org =
        console_platform_request_context::current_org().map_err(rest_error_from_request_context)?;
    let facts = state
        .store
        .audit_stream_authorization_facts(principal.user_id, CEO_COVERT_AUDIT_STREAM_KEY)
        .await
        .map_err(RestError::from_store)?;
    let policy_version = freshness_u64("policy_version", facts.policy_version)?;
    let subject_version = freshness_u64("subject_version", facts.subject_version)?;
    let session_generation = freshness_u64("session_generation", facts.session_generation)?;
    let bundle = engine::compile_audit_stream_bundle(org, policy_version).map_err(|err| {
        RestError::internal(format!("Cedar audit-stream policy unavailable: {err}"))
    })?;
    let feature = match read_kind {
        AuditStreamReadKind::Events => Feature::AuditStreamRead,
        AuditStreamReadKind::AccessEvents => Feature::AuditStreamAccessLogRead,
    };
    let resource = AuthorizationResource::org_wide(org, "audit_stream")
        .with_resource_id(CEO_COVERT_AUDIT_STREAM_KEY);
    let mut request = AuthorizationRequest::new(principal.clone(), Action::new(feature), resource)
        .with_policy_domain("compliance.audit_stream")
        .with_subject_freshness(principal.authz_freshness)
        .with_clearance_keys(facts.active_clearance_keys)
        .requiring_freshness(SubjectFreshnessRequirement {
            min_policy_version: policy_version,
            min_subject_version: subject_version,
            min_session_generation: session_generation,
            required_step_up_generation: None,
        })
        .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(org));
    request.context = AuthorizationContext {
        purpose: Some(query.purpose.clone()),
        channel: Some(query.channel.clone()),
        request_id: None,
    };
    let entry = CoexistenceMapEntry::new(
        format!("compliance.audit_stream.{}", feature.as_str()),
        "compliance.audit_stream",
        feature,
        "audit_stream",
        DualEngineMode::CedarOnly,
        Some(bundle.key.clone()),
    );
    let cedar = engine::evaluate(&request, &bundle);
    let decision = evaluate_cedar_pbac_boundary(&request, Some(&entry), cedar);
    if decision.effect.is_allow() {
        Ok(())
    } else {
        Err(RestError::from_kernel(KernelError::forbidden(format!(
            "Cedar denied CEO audit stream access: {:?}",
            decision.reason
        ))))
    }
}

fn freshness_u64(field: &'static str, value: i64) -> Result<u64, RestError> {
    u64::try_from(value)
        .map_err(|_| RestError::internal(format!("authorization {field} is negative")))
}

fn resolve_requested_branch(
    principal: &Principal,
    requested: Option<BranchId>,
) -> Result<BranchId, RestError> {
    if let Some(branch_id) = requested {
        if principal.branch_scope.allows(branch_id) {
            return Ok(branch_id);
        }
        return Err(RestError::from_kernel(KernelError::forbidden(
            "resource branch is outside principal scope",
        )));
    }

    match &principal.branch_scope {
        BranchScope::Branches(branches) if branches.len() == 1 => {
            branches.iter().copied().next().ok_or_else(|| {
                RestError::from_kernel(KernelError::validation("branch_id is required"))
            })
        }
        BranchScope::Branches(_) | BranchScope::All => Err(RestError::from_kernel(
            KernelError::validation("branch_id is required"),
        )),
    }
}

fn ledger_csv(items: &[LocationConsentLedgerEntry]) -> String {
    let mut csv =
        "id,consent_id,user_id,branch_id,actor,action,from_status,to_status,occurred_at,created_at\n"
            .to_owned();
    for item in items {
        let actor = item
            .actor
            .map(|actor| actor.to_string())
            .unwrap_or_default();
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{}\n",
            csv_field(&item.id),
            csv_field(&item.consent_id),
            csv_field(&item.user_id.to_string()),
            csv_field(&item.branch_id.to_string()),
            csv_field(&actor),
            csv_field(&item.action),
            csv_field(item.from_status.as_db_str()),
            csv_field(item.to_status.as_db_str()),
            csv_field(&item.occurred_at.to_string()),
            csv_field(&item.created_at.to_string()),
        ));
    }
    csv
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

async fn principal_from_headers(
    state: &ComplianceRestState,
    headers: &HeaderMap,
) -> Result<Principal, RestError> {
    let verifier = state.jwt_verifier.as_ref().ok_or_else(|| {
        RestError::unavailable("JWT verification is not configured for compliance API")
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
            RestError::unavailable("JWT verification is not configured for compliance API")
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

fn current_trace_context() -> TraceContext {
    TraceContext::generate()
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

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
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
            ErrorKind::Validation => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                error.message,
            ),
            ErrorKind::Forbidden => Self::new(StatusCode::FORBIDDEN, "forbidden", error.message),
            ErrorKind::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", error.message),
            ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                Self::new(StatusCode::CONFLICT, "conflict", error.message)
            }
            ErrorKind::Internal => Self::internal(error.message),
        }
    }

    fn from_store(error: PgComplianceError) -> Self {
        match error.kind() {
            ErrorKind::Validation => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                error.to_string(),
            ),
            ErrorKind::Forbidden => {
                Self::new(StatusCode::FORBIDDEN, "forbidden", error.to_string())
            }
            ErrorKind::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", error.to_string()),
            ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                Self::new(StatusCode::CONFLICT, "conflict", error.to_string())
            }
            ErrorKind::Internal => Self::internal("internal server error"),
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

#[cfg(test)]
mod tests {
    use super::{
        LocationPingRequest, RestError, require_evidence_accept_with_read,
        require_evidence_link_with_read,
    };
    use axum::{http::StatusCode, response::IntoResponse};
    use console_kernel_core::{BranchScope, KernelError, OrgId, UserId};
    use console_platform_authz::{EffectiveFeatureGrant, Feature, PermissionLevel, Principal};
    use std::collections::BTreeSet;

    fn principal_with(feature: Feature) -> Principal {
        Principal::new(
            UserId::new(),
            OrgId::knl(),
            BTreeSet::new(),
            BranchScope::All,
        )
        .with_effective_feature_grants(vec![EffectiveFeatureGrant::new(
            feature,
            PermissionLevel::Allow,
            BranchScope::All,
        )])
    }

    #[test]
    fn evidence_link_without_compliance_read_is_forbidden() {
        let error =
            require_evidence_link_with_read(&principal_with(Feature::ComplianceEvidenceLink))
                .expect_err("an action grant cannot imply compliance catalog read");
        assert_eq!(error.into_response().status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn evidence_accept_without_compliance_read_is_forbidden() {
        let error =
            require_evidence_accept_with_read(&principal_with(Feature::ComplianceDomainManage))
                .expect_err("an action grant cannot imply compliance catalog read");
        assert_eq!(error.into_response().status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn evidence_acceptance_error_mapping_preserves_forbidden_conflict_and_unavailable_truth() {
        assert_eq!(
            RestError::from_kernel(KernelError::forbidden("not allowed"))
                .into_response()
                .status(),
            StatusCode::FORBIDDEN,
        );
        assert_eq!(
            RestError::from_kernel(KernelError::conflict("already accepted"))
                .into_response()
                .status(),
            StatusCode::CONFLICT,
        );
        assert_eq!(
            RestError::unavailable("auth dependency unavailable")
                .into_response()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE,
        );
    }

    #[test]
    fn location_ping_accepts_fractional_rfc3339_timestamp() {
        let request: LocationPingRequest = serde_json::from_str(
            r#"{
                "latitude": 37.5665,
                "longitude": 126.9780,
                "recorded_at": "2026-07-22T12:34:56.123456789Z",
                "on_duty": true
            }"#,
        )
        .unwrap();

        assert_eq!(request.recorded_at.nanosecond(), 123_456_789);
    }
}
