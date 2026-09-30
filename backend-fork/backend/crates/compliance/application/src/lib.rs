//! Compliance application layer.
//!
//! Use-case commands and ports live here; concrete storage remains in adapter
//! crates.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_compliance_domain::{
    ComplianceControl, ComplianceFramework, ComplianceObligation, ComplianceRiskLevel,
    ComplianceScope, ControlCadence, ControlStatus, ControlType, CoverageLevel, EvidenceBinding,
    EvidenceBindingStatus, EvidenceConfidence, EvidenceTargetType, FrameworkKind, FrameworkStatus,
    LocationConsent, LocationConsentState, ObligationRegulationRelationship, ObligationStatus,
    ObligationType, RegulationImpact, RegulationImpactStatus, ReviewCadence,
};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, KernelError, Timestamp, TraceContext,
    Transition, UserId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Consent transition requested by an outer adapter/use case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentTransitionKind {
    Grant,
    Suspend,
    Resume,
    Withdraw,
}

impl ConsentTransitionKind {
    #[must_use]
    pub const fn audit_action(self) -> &'static str {
        match self {
            Self::Grant => "consent.grant",
            Self::Suspend => "consent.suspend",
            Self::Resume => "consent.resume",
            Self::Withdraw => "consent.withdraw",
        }
    }

    pub fn apply(
        self,
        consent: &mut LocationConsent,
        occurred_at: Timestamp,
    ) -> Result<Transition<LocationConsentState>, KernelError> {
        match self {
            Self::Grant => consent.grant(occurred_at),
            Self::Suspend => consent.suspend(occurred_at),
            Self::Resume => consent.resume(occurred_at),
            Self::Withdraw => consent.withdraw(occurred_at),
        }
    }
}

/// Command data required to mutate the LocationConsent ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentTransitionCommand {
    pub kind: ConsentTransitionKind,
    pub actor: Option<UserId>,
    pub user_id: UserId,
    pub branch_id: BranchId,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

pub fn consent_audit_event(
    command: &ConsentTransitionCommand,
    before: &LocationConsent,
    after: &LocationConsent,
) -> Result<AuditEvent, KernelError> {
    let action = AuditAction::new(command.kind.audit_action())?;
    let before_json = serde_json::to_value(before).map_err(|err| {
        KernelError::internal(format!(
            "failed to serialize consent before snapshot: {err}"
        ))
    })?;
    let after_json = serde_json::to_value(after).map_err(|err| {
        KernelError::internal(format!("failed to serialize consent after snapshot: {err}"))
    })?;

    Ok(AuditEvent::new(
        command.actor,
        action,
        "location_consent",
        after.id().to_string(),
        command.trace.clone(),
        command.occurred_at,
    )
    .with_branch(command.branch_id)
    .with_snapshots(Some(before_json), Some(after_json)))
}

/// Filters for reading the audited consent lifecycle ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationConsentLedgerQuery {
    pub user_id: Option<UserId>,
    pub branch_id: Option<BranchId>,
    pub limit: i64,
    pub offset: i64,
}

/// One audited consent lifecycle transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationConsentLedgerEntry {
    pub id: String,
    pub consent_id: String,
    pub user_id: UserId,
    pub branch_id: BranchId,
    pub actor: Option<UserId>,
    pub action: String,
    pub from_status: LocationConsentState,
    pub to_status: LocationConsentState,
    pub occurred_at: Timestamp,
    pub created_at: Timestamp,
}

/// Paged consent lifecycle ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationConsentLedgerPage {
    pub items: Vec<LocationConsentLedgerEntry>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

/// Filters for reading the site arrival/departure events log (issue #13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrivalEventQuery {
    pub user_id: Option<UserId>,
    pub branch_id: Option<BranchId>,
    pub limit: i64,
    pub offset: i64,
}

/// One site arrival or departure — a coordinate-free attendance fact, hydrated
/// with work-order, mechanic, customer, and admin-entered site coordinate data
/// for dispatch-map display. The raw phone GPS ping is intentionally not exposed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArrivalEvent {
    pub id: String,
    pub work_order_id: String,
    pub site_id: String,
    pub work_order_no: String,
    pub site_name: String,
    pub customer_name: String,
    pub mechanic_name: String,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub kind: String,
    pub occurred_at: Timestamp,
}

/// Paged site arrival/departure events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArrivalEventPage {
    pub items: Vec<ArrivalEvent>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

/// Server-owned key for the CEO/top-clearance covert audit stream.
pub const CEO_COVERT_AUDIT_STREAM_KEY: &str = "ceo_covert_audit";

/// Tenant-default clearance fact required by the first B26b Cedar bundle.
pub const CEO_COVERT_AUDIT_CLEARANCE_KEY: &str = "audit.ceo_covert.read";

/// Sensitivity label persisted for rows included in the CEO/covert stream.
pub const CEO_COVERT_AUDIT_SENSITIVITY: &str = "CEO_COVERT";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditStreamReadKind {
    Events,
    AccessEvents,
}

impl AuditStreamReadKind {
    #[must_use]
    pub const fn cedar_action(self) -> &'static str {
        match self {
            Self::Events => "audit_stream_read",
            Self::AccessEvents => "audit_stream_access_log_read",
        }
    }

    #[must_use]
    pub const fn access_audit_action(self) -> &'static str {
        match self {
            Self::Events => "audit_stream.ceo_read",
            Self::AccessEvents => "audit_stream.access_log_read",
        }
    }

    #[must_use]
    pub const fn response_kind(self) -> &'static str {
        match self {
            Self::Events => "events",
            Self::AccessEvents => "access_events",
        }
    }
}

/// DB-current authorization facts loaded under the caller's armed org before
/// the REST layer builds a Cedar request. Revoked/expired assignments are
/// omitted; absence of the required key makes Cedar deny by omission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditStreamAuthorizationFacts {
    pub active_clearance_keys: BTreeSet<String>,
    pub policy_version: i64,
    pub subject_version: i64,
    pub session_generation: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditStreamQuery {
    pub limit: i64,
    pub offset: i64,
    pub purpose: String,
    pub channel: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditStreamRecord {
    pub id: String,
    pub actor: Option<UserId>,
    pub action: String,
    pub target_type: String,
    pub target_id: String,
    pub sensitivity: String,
    pub before_snap: Option<serde_json::Value>,
    pub after_snap: Option<serde_json::Value>,
    pub trace_id: String,
    pub span_id: String,
    pub occurred_at: Timestamp,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditStreamPage {
    pub items: Vec<AuditStreamRecord>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
    pub stream_key: String,
    pub read_kind: AuditStreamReadKind,
    pub access_audit_id: String,
}

pub fn audit_stream_access_event(
    actor: UserId,
    read_kind: AuditStreamReadKind,
    query: &AuditStreamQuery,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    let after = serde_json::json!({
        "stream_key": CEO_COVERT_AUDIT_STREAM_KEY,
        "read_kind": read_kind.response_kind(),
        "purpose": query.purpose.as_str(),
        "channel": query.channel.as_str(),
        "limit": query.limit,
        "offset": query.offset,
    });
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new(read_kind.access_audit_action())?,
        "audit_stream",
        CEO_COVERT_AUDIT_STREAM_KEY,
        trace,
        occurred_at,
    )
    .with_snapshots(None, Some(after)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    pub limit: i64,
    pub offset: i64,
}

impl PageRequest {
    pub fn new(limit: Option<i64>, offset: Option<i64>) -> Result<Self, KernelError> {
        let offset = offset.unwrap_or(0);
        if offset < 0 {
            return Err(KernelError::validation("offset must be non-negative"));
        }
        Ok(Self {
            limit: limit.unwrap_or(100).clamp(1, 1_000),
            offset,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegulationImpactPage {
    pub items: Vec<RegulationImpact>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComplianceObligationPage {
    pub items: Vec<ComplianceObligation>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComplianceFrameworkPage {
    pub items: Vec<ComplianceFramework>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComplianceControlPage {
    pub items: Vec<ComplianceControl>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceBindingPage {
    pub items: Vec<EvidenceBinding>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegulationImpactQuery {
    pub status: Option<RegulationImpactStatus>,
    pub risk_level: Option<ComplianceRiskLevel>,
    pub q: Option<String>,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComplianceObligationQuery {
    pub branch_scope: BranchScope,
    pub status: Option<ObligationStatus>,
    pub severity: Option<ComplianceRiskLevel>,
    pub scope_type: Option<console_compliance_domain::ComplianceScopeKind>,
    pub branch_id: Option<BranchId>,
    pub site_id: Option<console_kernel_core::SiteId>,
    pub q: Option<String>,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComplianceFrameworkQuery {
    pub status: Option<FrameworkStatus>,
    pub kind: Option<FrameworkKind>,
    pub q: Option<String>,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComplianceControlQuery {
    pub framework_id: uuid::Uuid,
    pub status: Option<ControlStatus>,
    pub q: Option<String>,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvidenceBindingQuery {
    pub control_id: Option<uuid::Uuid>,
    pub obligation_id: Option<uuid::Uuid>,
    pub target_type: Option<EvidenceTargetType>,
    pub status: Option<EvidenceBindingStatus>,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateRegulationImpactCommand {
    pub actor: UserId,
    pub title: String,
    pub jurisdiction: String,
    pub regulator: Option<String>,
    pub citation: String,
    pub source_url: Option<String>,
    pub impact_area: String,
    pub impact_summary: String,
    pub risk_level: ComplianceRiskLevel,
    pub effective_from: Option<time::Date>,
    pub effective_to: Option<time::Date>,
    pub review_due_on: Option<time::Date>,
    pub owner_user_id: Option<UserId>,
    pub metadata: serde_json::Value,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateComplianceObligationCommand {
    pub actor: UserId,
    pub title: String,
    pub description: String,
    pub obligation_type: ObligationType,
    pub scope: ComplianceScope,
    pub owner_user_id: Option<UserId>,
    pub severity: ComplianceRiskLevel,
    pub effective_from: Option<time::Date>,
    pub effective_to: Option<time::Date>,
    pub review_cadence: Option<ReviewCadence>,
    pub next_review_on: Option<time::Date>,
    pub metadata: serde_json::Value,
    pub regulation_links: Vec<CreateObligationRegulationLink>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateObligationRegulationLink {
    pub regulation_impact_id: uuid::Uuid,
    pub relationship: ObligationRegulationRelationship,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkObligationRegulationCommand {
    pub actor: UserId,
    pub obligation_id: uuid::Uuid,
    pub regulation_impact_id: uuid::Uuid,
    pub relationship: ObligationRegulationRelationship,
    pub rationale: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateComplianceFrameworkCommand {
    pub actor: UserId,
    pub name: String,
    pub version_label: String,
    pub framework_kind: FrameworkKind,
    pub owner_user_id: Option<UserId>,
    pub effective_from: Option<time::Date>,
    pub effective_to: Option<time::Date>,
    pub metadata: serde_json::Value,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateComplianceControlCommand {
    pub actor: UserId,
    pub framework_id: uuid::Uuid,
    pub control_key: String,
    pub title: String,
    pub objective: String,
    pub control_type: ControlType,
    pub cadence: Option<ControlCadence>,
    pub evidence_requirements: serde_json::Value,
    pub owner_user_id: Option<UserId>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkControlObligationCommand {
    pub actor: UserId,
    pub control_id: uuid::Uuid,
    pub obligation_id: uuid::Uuid,
    pub coverage_level: CoverageLevel,
    pub coverage_rationale: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateEvidenceBindingCommand {
    pub actor: UserId,
    pub control_id: uuid::Uuid,
    pub obligation_id: Option<uuid::Uuid>,
    pub evidence_target_type: EvidenceTargetType,
    pub evidence_target_id: String,
    pub source_audit_event_id: Option<uuid::Uuid>,
    pub confidence: EvidenceConfidence,
    pub collected_at: Option<Timestamp>,
    pub collected_by: Option<UserId>,
    pub valid_from: Option<time::Date>,
    pub valid_to: Option<time::Date>,
    pub hash_sha256: Option<String>,
    pub metadata: serde_json::Value,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Authenticated action that accepts a proposed evidence binding. Catalog
/// identity and evidence provenance remain immutable; only this lifecycle edge
/// may change the binding status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptEvidenceBindingCommand {
    pub actor: UserId,
    pub id: uuid::Uuid,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComplianceStatusCommand<S> {
    pub actor: UserId,
    pub id: uuid::Uuid,
    pub status: S,
    pub memo: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

// ponytail: peer-owned signature, redesign deferred
#[allow(clippy::too_many_arguments)]
pub fn compliance_audit_event(
    action: &str,
    actor: UserId,
    target_type: &str,
    target_id: impl ToString,
    trace: TraceContext,
    occurred_at: Timestamp,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new(action)?,
        target_type,
        target_id.to_string(),
        trace,
        occurred_at,
    )
    .with_snapshots(before, after))
}

pub fn relation_audit_snapshot<T: Serialize>(value: &T) -> Result<serde_json::Value, KernelError> {
    serde_json::to_value(value).map_err(|err| {
        KernelError::internal(format!("failed to serialize compliance snapshot: {err}"))
    })
}
