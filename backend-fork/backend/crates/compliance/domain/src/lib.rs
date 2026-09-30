//! Compliance domain.
//!
//! Pure state machines and value objects only. Postgres partitioning,
//! transactions, and audit persistence live in outer crates.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::{
    BranchId, ConsentId, KernelError, LocationPingId, Timestamp, Transition, TransitionError,
    UserId,
};
use time::Duration;

/// Location-consent FSM states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LocationConsentState {
    /// No persisted consent record exists yet.
    NoRecord,
    /// Individual consent is active; on-duty pings may be collected.
    Granted,
    /// The non-refusable off switch is active; collection must stop.
    Suspended,
    /// Consent was withdrawn and destructible ping data must be destroyed.
    Withdrawn,
}

impl LocationConsentState {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::NoRecord => "NO_RECORD",
            Self::Granted => "GRANTED",
            Self::Suspended => "SUSPENDED",
            Self::Withdrawn => "WITHDRAWN",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "NO_RECORD" => Ok(Self::NoRecord),
            "GRANTED" => Ok(Self::Granted),
            "SUSPENDED" => Ok(Self::Suspended),
            "WITHDRAWN" => Ok(Self::Withdrawn),
            other => Err(KernelError::validation(format!(
                "unknown location consent state {other:?}"
            ))),
        }
    }
}

/// Current consent ledger head for one user.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LocationConsent {
    id: ConsentId,
    user_id: UserId,
    branch_id: BranchId,
    state: LocationConsentState,
    granted_at: Option<Timestamp>,
    suspended_at: Option<Timestamp>,
    resumed_at: Option<Timestamp>,
    withdrawn_at: Option<Timestamp>,
    updated_at: Option<Timestamp>,
}

/// Database row shape used by outer adapters to rehydrate a consent ledger head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedLocationConsent {
    pub id: ConsentId,
    pub user_id: UserId,
    pub branch_id: BranchId,
    pub state: LocationConsentState,
    pub granted_at: Option<Timestamp>,
    pub suspended_at: Option<Timestamp>,
    pub resumed_at: Option<Timestamp>,
    pub withdrawn_at: Option<Timestamp>,
    pub updated_at: Timestamp,
}

impl LocationConsent {
    #[must_use]
    pub fn unrecorded(user_id: UserId, branch_id: BranchId) -> Self {
        Self {
            id: ConsentId::new(),
            user_id,
            branch_id,
            state: LocationConsentState::NoRecord,
            granted_at: None,
            suspended_at: None,
            resumed_at: None,
            withdrawn_at: None,
            updated_at: None,
        }
    }

    #[must_use]
    pub fn from_persisted(row: PersistedLocationConsent) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            branch_id: row.branch_id,
            state: row.state,
            granted_at: row.granted_at,
            suspended_at: row.suspended_at,
            resumed_at: row.resumed_at,
            withdrawn_at: row.withdrawn_at,
            updated_at: Some(row.updated_at),
        }
    }

    #[must_use]
    pub const fn id(&self) -> ConsentId {
        self.id
    }

    #[must_use]
    pub const fn user_id(&self) -> UserId {
        self.user_id
    }

    #[must_use]
    pub const fn branch_id(&self) -> BranchId {
        self.branch_id
    }

    #[must_use]
    pub const fn state(&self) -> LocationConsentState {
        self.state
    }

    #[must_use]
    pub const fn granted_at(&self) -> Option<Timestamp> {
        self.granted_at
    }

    #[must_use]
    pub const fn suspended_at(&self) -> Option<Timestamp> {
        self.suspended_at
    }

    #[must_use]
    pub const fn resumed_at(&self) -> Option<Timestamp> {
        self.resumed_at
    }

    #[must_use]
    pub const fn withdrawn_at(&self) -> Option<Timestamp> {
        self.withdrawn_at
    }

    #[must_use]
    pub const fn updated_at(&self) -> Option<Timestamp> {
        self.updated_at
    }

    pub fn grant(
        &mut self,
        at: Timestamp,
    ) -> Result<Transition<LocationConsentState>, KernelError> {
        match self.state {
            LocationConsentState::NoRecord | LocationConsentState::Withdrawn => {
                let transition = self.transition_to(LocationConsentState::Granted, at);
                self.granted_at = Some(at);
                self.suspended_at = None;
                self.resumed_at = None;
                self.withdrawn_at = None;
                Ok(transition)
            }
            from => Err(illegal(from, LocationConsentState::Granted)),
        }
    }

    pub fn suspend(
        &mut self,
        at: Timestamp,
    ) -> Result<Transition<LocationConsentState>, KernelError> {
        match self.state {
            LocationConsentState::Granted => {
                let transition = self.transition_to(LocationConsentState::Suspended, at);
                self.suspended_at = Some(at);
                Ok(transition)
            }
            from => Err(illegal(from, LocationConsentState::Suspended)),
        }
    }

    pub fn resume(
        &mut self,
        at: Timestamp,
    ) -> Result<Transition<LocationConsentState>, KernelError> {
        match self.state {
            LocationConsentState::Suspended => {
                let transition = self.transition_to(LocationConsentState::Granted, at);
                self.suspended_at = None;
                self.resumed_at = Some(at);
                Ok(transition)
            }
            from => Err(illegal(from, LocationConsentState::Granted)),
        }
    }

    pub fn withdraw(
        &mut self,
        at: Timestamp,
    ) -> Result<Transition<LocationConsentState>, KernelError> {
        match self.state {
            LocationConsentState::Granted | LocationConsentState::Suspended => {
                let transition = self.transition_to(LocationConsentState::Withdrawn, at);
                self.suspended_at = None;
                self.withdrawn_at = Some(at);
                Ok(transition)
            }
            from => Err(illegal(from, LocationConsentState::Withdrawn)),
        }
    }

    fn transition_to(
        &mut self,
        to: LocationConsentState,
        at: Timestamp,
    ) -> Transition<LocationConsentState> {
        let from = self.state;
        self.state = to;
        self.updated_at = Some(at);
        Transition { from, to }
    }
}

fn illegal(from: LocationConsentState, to: LocationConsentState) -> KernelError {
    TransitionError { from, to }.into()
}

/// A validated GPS coordinate pair.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Coordinates {
    latitude: f64,
    longitude: f64,
}

impl Coordinates {
    pub fn new(latitude: f64, longitude: f64) -> Result<Self, KernelError> {
        if !latitude.is_finite() || !(-90.0..=90.0).contains(&latitude) {
            return Err(KernelError::validation(format!(
                "latitude must be finite and within -90..=90, got {latitude}"
            )));
        }
        if !longitude.is_finite() || !(-180.0..=180.0).contains(&longitude) {
            return Err(KernelError::validation(format!(
                "longitude must be finite and within -180..=180, got {longitude}"
            )));
        }
        Ok(Self {
            latitude,
            longitude,
        })
    }

    #[must_use]
    pub const fn latitude(self) -> f64 {
        self.latitude
    }

    #[must_use]
    pub const fn longitude(self) -> f64 {
        self.longitude
    }
}

/// Destructible, non-audited location ping.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LocationPing {
    id: LocationPingId,
    user_id: UserId,
    branch_id: BranchId,
    coordinates: Coordinates,
    accuracy_m: Option<f64>,
    recorded_at: Timestamp,
    on_duty: bool,
}

impl LocationPing {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: LocationPingId,
        user_id: UserId,
        branch_id: BranchId,
        latitude: f64,
        longitude: f64,
        accuracy_m: Option<f64>,
        recorded_at: Timestamp,
        on_duty: bool,
    ) -> Result<Self, KernelError> {
        if let Some(accuracy) = accuracy_m
            && (!accuracy.is_finite() || accuracy < 0.0)
        {
            return Err(KernelError::validation(format!(
                "accuracy_m must be finite and non-negative, got {accuracy}"
            )));
        }

        Ok(Self {
            id,
            user_id,
            branch_id,
            coordinates: Coordinates::new(latitude, longitude)?,
            accuracy_m,
            recorded_at,
            on_duty,
        })
    }

    #[must_use]
    pub const fn id(&self) -> LocationPingId {
        self.id
    }

    #[must_use]
    pub const fn user_id(&self) -> UserId {
        self.user_id
    }

    #[must_use]
    pub const fn branch_id(&self) -> BranchId {
        self.branch_id
    }

    #[must_use]
    pub const fn latitude(&self) -> f64 {
        self.coordinates.latitude()
    }

    #[must_use]
    pub const fn longitude(&self) -> f64 {
        self.coordinates.longitude()
    }

    #[must_use]
    pub const fn accuracy_m(&self) -> Option<f64> {
        self.accuracy_m
    }

    #[must_use]
    pub const fn recorded_at(&self) -> Timestamp {
        self.recorded_at
    }

    #[must_use]
    pub const fn on_duty(&self) -> bool {
        self.on_duty
    }
}

/// Upper bound for retained ping rows: users × on-duty window × ping rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PingVolumeBound {
    user_count: u64,
    max_rows: u64,
}

impl PingVolumeBound {
    pub fn new(
        user_count: u64,
        on_duty_window: Duration,
        min_ping_interval: Duration,
    ) -> Result<Self, KernelError> {
        let window_seconds = positive_seconds(on_duty_window, "on_duty_window")?;
        let interval_seconds = positive_seconds(min_ping_interval, "min_ping_interval")?;
        let pings_per_user = window_seconds.div_ceil(interval_seconds);
        let max_rows = user_count
            .checked_mul(pings_per_user)
            .ok_or_else(|| KernelError::validation("ping volume bound overflows u64"))?;

        Ok(Self {
            user_count,
            max_rows,
        })
    }

    #[must_use]
    pub const fn user_count(self) -> u64 {
        self.user_count
    }

    #[must_use]
    pub const fn max_rows(self) -> u64 {
        self.max_rows
    }

    #[must_use]
    pub const fn allows(self, row_count: u64) -> bool {
        row_count <= self.max_rows
    }
}

fn positive_seconds(duration: Duration, field: &str) -> Result<u64, KernelError> {
    let seconds = duration.whole_seconds();
    if seconds <= 0 {
        return Err(KernelError::validation(format!(
            "{field} must be positive, got {seconds} seconds"
        )));
    }
    u64::try_from(seconds).map_err(|_| {
        KernelError::validation(format!(
            "{field} is too large to convert into an unsigned second count"
        ))
    })
}

/// A geofence boundary crossing derived from one location ping (issue #13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeofenceCrossing {
    /// Moved from outside (or unknown) to inside a site geofence.
    Arrival,
    /// Moved from inside to outside a site geofence.
    Departure,
}

impl GeofenceCrossing {
    /// The audited action string (`site.arrival` / `site.departure`).
    #[must_use]
    pub fn audit_action(self) -> &'static str {
        match self {
            Self::Arrival => "site.arrival",
            Self::Departure => "site.departure",
        }
    }

    /// The `site_attendance_events.kind` value persisted for this crossing.
    #[must_use]
    pub fn kind(self) -> &'static str {
        match self {
            Self::Arrival => "ARRIVAL",
            Self::Departure => "DEPARTURE",
        }
    }
}

/// Evaluate one on-duty location ping against a site geofence.
///
/// Pure: given the ping and site coordinates, the effective radius (metres), and
/// the prior inside/outside state for this (user × work order × site) — `None`
/// when no record exists yet — returns whether the user is now inside and the
/// crossing. The crossing is `Some` ONLY on an edge (a state change), so a steady
/// stream of inside (or outside) pings emits nothing; a first-seen-inside is an
/// arrival but a first-seen-outside is not an event. Distance uses the shared
/// kernel haversine, so this stays dependency-free of the dispatch crate.
#[must_use]
pub fn evaluate_geofence(
    ping_latitude: f64,
    ping_longitude: f64,
    site_latitude: f64,
    site_longitude: f64,
    radius_meters: f64,
    prior_inside: Option<bool>,
) -> (bool, Option<GeofenceCrossing>) {
    // Unrounded distance: rounding to whole metres before the compare would push
    // the boundary out ~0.5 m, which is material for a small geofence radius.
    let distance = console_kernel_core::haversine_meters_f64(
        ping_latitude,
        ping_longitude,
        site_latitude,
        site_longitude,
    );
    let now_inside = distance <= radius_meters;
    let crossing = match prior_inside {
        None => now_inside.then_some(GeofenceCrossing::Arrival),
        Some(was) if was == now_inside => None,
        Some(_) => Some(if now_inside {
            GeofenceCrossing::Arrival
        } else {
            GeofenceCrossing::Departure
        }),
    };
    (now_inside, crossing)
}

#[cfg(test)]
mod geofence_tests {
    use super::{GeofenceCrossing, evaluate_geofence};

    // Seoul City Hall, a point ~30 m north (inside a 150 m radius), and a point
    // ~2.5 km away (outside).
    const SITE_LAT: f64 = 37.5665;
    const SITE_LON: f64 = 126.9780;
    const NEAR_LAT: f64 = 37.56677;
    const NEAR_LON: f64 = 126.9780;
    const FAR_LAT: f64 = 37.5796;
    const FAR_LON: f64 = 126.9770;
    const RADIUS: f64 = 150.0;

    #[test]
    fn first_ping_inside_is_arrival() {
        let (inside, crossing) =
            evaluate_geofence(NEAR_LAT, NEAR_LON, SITE_LAT, SITE_LON, RADIUS, None);
        assert!(inside);
        assert_eq!(crossing, Some(GeofenceCrossing::Arrival));
    }

    #[test]
    fn first_ping_outside_is_not_an_event() {
        let (inside, crossing) =
            evaluate_geofence(FAR_LAT, FAR_LON, SITE_LAT, SITE_LON, RADIUS, None);
        assert!(!inside);
        assert_eq!(crossing, None);
    }

    #[test]
    fn staying_inside_emits_nothing() {
        let (inside, crossing) =
            evaluate_geofence(NEAR_LAT, NEAR_LON, SITE_LAT, SITE_LON, RADIUS, Some(true));
        assert!(inside);
        assert_eq!(crossing, None);
    }

    #[test]
    fn leaving_is_departure() {
        let (inside, crossing) =
            evaluate_geofence(FAR_LAT, FAR_LON, SITE_LAT, SITE_LON, RADIUS, Some(true));
        assert!(!inside);
        assert_eq!(crossing, Some(GeofenceCrossing::Departure));
    }

    #[test]
    fn returning_is_arrival() {
        let (inside, crossing) =
            evaluate_geofence(NEAR_LAT, NEAR_LON, SITE_LAT, SITE_LON, RADIUS, Some(false));
        assert!(inside);
        assert_eq!(crossing, Some(GeofenceCrossing::Arrival));
    }
}

macro_rules! compliance_enum {
    (
        pub enum $name:ident {
            $($variant:ident => $wire:literal),+ $(,)?
        }
    ) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
            serde::Serialize, serde::Deserialize,
        )]
        pub enum $name {
            $(#[serde(rename = $wire)] $variant,)+
        }

        impl $name {
            #[must_use]
            pub const fn as_db_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }

            pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
                match value {
                    $($wire => Ok(Self::$variant),)+
                    other => Err(KernelError::validation(format!(
                        "unknown {} value {other:?}",
                        stringify!($name)
                    ))),
                }
            }
        }
    };
}

compliance_enum! {
    pub enum ComplianceRiskLevel { Info => "INFO", Low => "LOW", Medium => "MEDIUM", High => "HIGH", Critical => "CRITICAL" }
}

compliance_enum! {
    pub enum RegulationImpactStatus { Draft => "DRAFT", Active => "ACTIVE", Superseded => "SUPERSEDED", Archived => "ARCHIVED" }
}

compliance_enum! {
    pub enum ObligationType { Legal => "LEGAL", Regulatory => "REGULATORY", Contractual => "CONTRACTUAL", InternalPolicy => "INTERNAL_POLICY", ControlRequirement => "CONTROL_REQUIREMENT" }
}

compliance_enum! {
    pub enum ComplianceScopeKind { Org => "ORG", Branch => "BRANCH", Site => "SITE", Team => "TEAM", Role => "ROLE" }
}

compliance_enum! {
    pub enum ObligationStatus { Draft => "DRAFT", Active => "ACTIVE", Waived => "WAIVED", Superseded => "SUPERSEDED", Archived => "ARCHIVED" }
}

compliance_enum! {
    pub enum ReviewCadence { Monthly => "MONTHLY", Quarterly => "QUARTERLY", SemiAnnual => "SEMI_ANNUAL", Annual => "ANNUAL", EventDriven => "EVENT_DRIVEN" }
}

compliance_enum! {
    pub enum FrameworkKind { LegalBaseline => "LEGAL_BASELINE", InternalControl => "INTERNAL_CONTROL", CustomerControl => "CUSTOMER_CONTROL", SecurityStandard => "SECURITY_STANDARD", SafetyStandard => "SAFETY_STANDARD", AuditProgram => "AUDIT_PROGRAM" }
}

compliance_enum! {
    pub enum FrameworkStatus { Draft => "DRAFT", Active => "ACTIVE", Retired => "RETIRED", Archived => "ARCHIVED" }
}

compliance_enum! {
    pub enum ControlType { Preventive => "PREVENTIVE", Detective => "DETECTIVE", Corrective => "CORRECTIVE", Directive => "DIRECTIVE", Compensating => "COMPENSATING" }
}

compliance_enum! {
    pub enum ControlCadence { Continuous => "CONTINUOUS", Daily => "DAILY", Weekly => "WEEKLY", Monthly => "MONTHLY", Quarterly => "QUARTERLY", Annual => "ANNUAL", EventDriven => "EVENT_DRIVEN" }
}

compliance_enum! {
    pub enum ControlStatus { Draft => "DRAFT", Active => "ACTIVE", Retired => "RETIRED", Archived => "ARCHIVED" }
}

compliance_enum! {
    pub enum ObligationRegulationRelationship { DerivedFrom => "DERIVED_FROM", AmendedBy => "AMENDED_BY", SupersededBy => "SUPERSEDED_BY", Interprets => "INTERPRETS", Evidences => "EVIDENCES" }
}

compliance_enum! {
    pub enum CoverageLevel { Primary => "PRIMARY", Partial => "PARTIAL", Supporting => "SUPPORTING", Compensating => "COMPENSATING" }
}

compliance_enum! {
    pub enum CoverageStatus { Active => "ACTIVE", Retired => "RETIRED" }
}

compliance_enum! {
    pub enum EvidenceTargetType { AuditEvent => "audit_event", EvidenceMedia => "evidence_media", WorkflowRun => "workflow_run", WorkflowTask => "workflow_task", ObjectLink => "object_link", GovernanceFinding => "governance_finding", ExternalDocument => "external_document", FutureEvObject => "future_ev_object" }
}

compliance_enum! {
    pub enum EvidenceBindingStatus { Proposed => "PROPOSED", Accepted => "ACCEPTED", Rejected => "REJECTED", Expired => "EXPIRED", Retracted => "RETRACTED" }
}

compliance_enum! {
    pub enum EvidenceConfidence { Low => "LOW", Medium => "MEDIUM", High => "HIGH", System => "SYSTEM" }
}

/// Caller-independent tenant scope for CP obligations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ComplianceScope {
    pub kind: ComplianceScopeKind,
    pub scope_ref: Option<uuid::Uuid>,
    pub branch_id: Option<BranchId>,
    pub site_id: Option<console_kernel_core::SiteId>,
}

impl ComplianceScope {
    #[must_use]
    pub const fn org() -> Self {
        Self {
            kind: ComplianceScopeKind::Org,
            scope_ref: None,
            branch_id: None,
            site_id: None,
        }
    }

    #[must_use]
    pub fn branch(branch_id: BranchId) -> Self {
        Self {
            kind: ComplianceScopeKind::Branch,
            scope_ref: Some(*branch_id.as_uuid()),
            branch_id: Some(branch_id),
            site_id: None,
        }
    }

    #[must_use]
    pub fn site(branch_id: BranchId, site_id: console_kernel_core::SiteId) -> Self {
        Self {
            kind: ComplianceScopeKind::Site,
            scope_ref: Some(*site_id.as_uuid()),
            branch_id: Some(branch_id),
            site_id: Some(site_id),
        }
    }

    pub fn validate(self) -> Result<(), KernelError> {
        match self.kind {
            ComplianceScopeKind::Org => {
                if self.scope_ref.is_some() || self.branch_id.is_some() || self.site_id.is_some() {
                    return Err(KernelError::validation(
                        "ORG compliance scope must not carry scope_ref, branch_id, or site_id",
                    ));
                }
            }
            ComplianceScopeKind::Branch => {
                let branch_id = self.branch_id.ok_or_else(|| {
                    KernelError::validation("BRANCH compliance scope requires branch_id")
                })?;
                if self.scope_ref != Some(*branch_id.as_uuid()) || self.site_id.is_some() {
                    return Err(KernelError::validation(
                        "BRANCH compliance scope requires scope_ref=branch_id and no site_id",
                    ));
                }
            }
            ComplianceScopeKind::Site => {
                let site_id = self.site_id.ok_or_else(|| {
                    KernelError::validation("SITE compliance scope requires site_id")
                })?;
                if self.branch_id.is_none() || self.scope_ref != Some(*site_id.as_uuid()) {
                    return Err(KernelError::validation(
                        "SITE compliance scope requires branch_id and scope_ref=site_id",
                    ));
                }
            }
            ComplianceScopeKind::Team | ComplianceScopeKind::Role => {
                return Err(KernelError::validation(
                    "TEAM and ROLE compliance scopes require same-org owner validation and are not accepted yet",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RegulationImpact {
    pub id: uuid::Uuid,
    pub code: String,
    pub title: String,
    pub jurisdiction: String,
    pub regulator: Option<String>,
    pub citation: String,
    pub source_url: Option<String>,
    pub impact_area: String,
    pub impact_summary: String,
    pub risk_level: ComplianceRiskLevel,
    pub status: RegulationImpactStatus,
    pub effective_from: Option<time::Date>,
    pub effective_to: Option<time::Date>,
    pub review_due_on: Option<time::Date>,
    pub owner_user_id: Option<UserId>,
    pub metadata: serde_json::Value,
    pub created_by: UserId,
    pub updated_by: UserId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ComplianceObligation {
    pub id: uuid::Uuid,
    pub code: String,
    pub title: String,
    pub description: String,
    pub obligation_type: ObligationType,
    pub scope: ComplianceScope,
    pub owner_user_id: Option<UserId>,
    pub severity: ComplianceRiskLevel,
    pub status: ObligationStatus,
    pub effective_from: Option<time::Date>,
    pub effective_to: Option<time::Date>,
    pub review_cadence: Option<ReviewCadence>,
    pub next_review_on: Option<time::Date>,
    pub metadata: serde_json::Value,
    pub created_by: UserId,
    pub updated_by: UserId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ObligationRegulationLink {
    pub id: uuid::Uuid,
    pub obligation_id: uuid::Uuid,
    pub regulation_impact_id: uuid::Uuid,
    pub relationship: ObligationRegulationRelationship,
    pub rationale: Option<String>,
    pub created_by: UserId,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ComplianceFramework {
    pub id: uuid::Uuid,
    pub code: String,
    pub name: String,
    pub version_label: String,
    pub framework_kind: FrameworkKind,
    pub status: FrameworkStatus,
    pub owner_user_id: Option<UserId>,
    pub effective_from: Option<time::Date>,
    pub effective_to: Option<time::Date>,
    pub metadata: serde_json::Value,
    pub created_by: UserId,
    pub updated_by: UserId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ComplianceControl {
    pub id: uuid::Uuid,
    pub framework_id: uuid::Uuid,
    pub control_key: String,
    pub title: String,
    pub objective: String,
    pub control_type: ControlType,
    pub cadence: Option<ControlCadence>,
    pub status: ControlStatus,
    pub evidence_requirements: serde_json::Value,
    pub owner_user_id: Option<UserId>,
    pub created_by: UserId,
    pub updated_by: UserId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ControlObligationCoverage {
    pub id: uuid::Uuid,
    pub control_id: uuid::Uuid,
    pub obligation_id: uuid::Uuid,
    pub coverage_level: CoverageLevel,
    pub coverage_rationale: Option<String>,
    pub status: CoverageStatus,
    pub created_by: UserId,
    pub updated_by: UserId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EvidenceBinding {
    pub id: uuid::Uuid,
    pub control_id: uuid::Uuid,
    pub obligation_id: Option<uuid::Uuid>,
    pub evidence_target_type: EvidenceTargetType,
    pub evidence_target_id: String,
    pub source_audit_event_id: Option<uuid::Uuid>,
    pub status: EvidenceBindingStatus,
    pub confidence: EvidenceConfidence,
    pub collected_at: Option<Timestamp>,
    pub collected_by: Option<UserId>,
    pub valid_from: Option<time::Date>,
    pub valid_to: Option<time::Date>,
    pub hash_sha256: Option<String>,
    pub metadata: serde_json::Value,
    pub created_by: UserId,
    pub updated_by: UserId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

pub fn validate_prefixed_code(prefix: &str, code: &str) -> Result<(), KernelError> {
    let Some(rest) = code
        .strip_prefix(prefix)
        .and_then(|value| value.strip_prefix('-'))
    else {
        return Err(KernelError::validation(format!(
            "compliance code {code:?} must start with {prefix}-"
        )));
    };
    if rest.len() < 4 || !rest.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(KernelError::validation(format!(
            "compliance code {code:?} must use at least four digits after {prefix}-"
        )));
    }
    Ok(())
}

pub fn validate_required_text(
    field: &str,
    value: &str,
    max_chars: usize,
) -> Result<(), KernelError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(KernelError::validation(format!("{field} is required")));
    }
    if trimmed.chars().count() > max_chars {
        return Err(KernelError::validation(format!(
            "{field} must be at most {max_chars} characters"
        )));
    }
    Ok(())
}

pub fn validate_optional_text(
    field: &str,
    value: Option<&str>,
    max_chars: usize,
) -> Result<(), KernelError> {
    if let Some(value) = value {
        if value.trim().is_empty() {
            return Err(KernelError::validation(format!(
                "{field} must be omitted or non-empty"
            )));
        }
        if value.trim().chars().count() > max_chars {
            return Err(KernelError::validation(format!(
                "{field} must be at most {max_chars} characters"
            )));
        }
    }
    Ok(())
}

pub fn validate_date_range(
    field: &str,
    start: Option<time::Date>,
    end: Option<time::Date>,
) -> Result<(), KernelError> {
    if let (Some(start), Some(end)) = (start, end)
        && end < start
    {
        return Err(KernelError::validation(format!(
            "{field} end date must be on or after start date"
        )));
    }
    Ok(())
}

pub fn validate_metadata_object(value: &serde_json::Value) -> Result<(), KernelError> {
    if value.is_object() {
        Ok(())
    } else {
        Err(KernelError::validation("metadata must be a JSON object"))
    }
}

pub fn validate_evidence_requirements(value: &serde_json::Value) -> Result<(), KernelError> {
    if value.is_array() {
        Ok(())
    } else {
        Err(KernelError::validation(
            "evidence requirements must be a JSON array",
        ))
    }
}

pub fn validate_control_key(value: &str) -> Result<(), KernelError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 64 {
        return Err(KernelError::validation(
            "control_key must be 1..=64 characters",
        ));
    }
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return Err(KernelError::validation("control_key is required"));
    };
    if !first.is_ascii_uppercase() && !first.is_ascii_digit() {
        return Err(KernelError::validation(
            "control_key must start with an uppercase ASCII letter or digit",
        ));
    }
    if !chars
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-'))
    {
        return Err(KernelError::validation(
            "control_key may contain only uppercase ASCII letters, digits, dot, underscore, or hyphen",
        ));
    }
    Ok(())
}

pub fn validate_hash_sha256(value: Option<&str>) -> Result<(), KernelError> {
    if let Some(value) = value
        && (value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(KernelError::validation(
            "hash_sha256 must be exactly 64 hexadecimal characters",
        ));
    }
    Ok(())
}

pub fn validate_status_memo(
    status: &str,
    memo: Option<&str>,
    statuses_requiring_memo: &[&str],
) -> Result<(), KernelError> {
    if statuses_requiring_memo.contains(&status) {
        validate_required_text("status memo", memo.unwrap_or_default(), 2_000)?;
    }
    Ok(())
}

pub fn validate_regulation_status_transition(
    from: RegulationImpactStatus,
    to: RegulationImpactStatus,
) -> Result<(), KernelError> {
    let allowed = matches!(
        (from, to),
        (
            RegulationImpactStatus::Draft,
            RegulationImpactStatus::Active | RegulationImpactStatus::Archived
        ) | (
            RegulationImpactStatus::Active,
            RegulationImpactStatus::Superseded | RegulationImpactStatus::Archived
        ) | (
            RegulationImpactStatus::Superseded,
            RegulationImpactStatus::Archived
        )
    );
    validate_transition(
        "regulation impact",
        from.as_db_str(),
        to.as_db_str(),
        allowed,
    )
}

pub fn validate_obligation_status_transition(
    from: ObligationStatus,
    to: ObligationStatus,
) -> Result<(), KernelError> {
    let allowed = matches!(
        (from, to),
        (
            ObligationStatus::Draft,
            ObligationStatus::Active | ObligationStatus::Archived
        ) | (
            ObligationStatus::Active,
            ObligationStatus::Waived | ObligationStatus::Superseded | ObligationStatus::Archived
        ) | (
            ObligationStatus::Waived,
            ObligationStatus::Active | ObligationStatus::Archived
        ) | (ObligationStatus::Superseded, ObligationStatus::Archived)
    );
    validate_transition(
        "compliance obligation",
        from.as_db_str(),
        to.as_db_str(),
        allowed,
    )
}

pub fn validate_framework_status_transition(
    from: FrameworkStatus,
    to: FrameworkStatus,
) -> Result<(), KernelError> {
    let allowed = matches!(
        (from, to),
        (
            FrameworkStatus::Draft,
            FrameworkStatus::Active | FrameworkStatus::Archived
        ) | (
            FrameworkStatus::Active,
            FrameworkStatus::Retired | FrameworkStatus::Archived
        ) | (FrameworkStatus::Retired, FrameworkStatus::Archived)
    );
    validate_transition(
        "compliance framework",
        from.as_db_str(),
        to.as_db_str(),
        allowed,
    )
}

pub fn validate_control_status_transition(
    from: ControlStatus,
    to: ControlStatus,
) -> Result<(), KernelError> {
    let allowed = matches!(
        (from, to),
        (
            ControlStatus::Draft,
            ControlStatus::Active | ControlStatus::Archived
        ) | (
            ControlStatus::Active,
            ControlStatus::Retired | ControlStatus::Archived
        ) | (ControlStatus::Retired, ControlStatus::Archived)
    );
    validate_transition(
        "compliance control",
        from.as_db_str(),
        to.as_db_str(),
        allowed,
    )
}

pub fn validate_evidence_status_transition(
    from: EvidenceBindingStatus,
    to: EvidenceBindingStatus,
) -> Result<(), KernelError> {
    let allowed = matches!(
        (from, to),
        (
            EvidenceBindingStatus::Proposed,
            EvidenceBindingStatus::Accepted
                | EvidenceBindingStatus::Rejected
                | EvidenceBindingStatus::Retracted
        ) | (
            EvidenceBindingStatus::Accepted,
            EvidenceBindingStatus::Expired | EvidenceBindingStatus::Retracted
        ) | (
            EvidenceBindingStatus::Expired,
            EvidenceBindingStatus::Retracted
        )
    );
    validate_transition(
        "compliance evidence binding",
        from.as_db_str(),
        to.as_db_str(),
        allowed,
    )
}

fn validate_transition(
    object: &str,
    from: &str,
    to: &str,
    allowed: bool,
) -> Result<(), KernelError> {
    if from == to || allowed {
        Ok(())
    } else {
        Err(KernelError::conflict(format!(
            "illegal {object} status transition {from} -> {to}"
        )))
    }
}

#[cfg(test)]
mod compliance_domain_tests {
    use super::*;

    fn assert_enum_wire_value<T>(value: T, wire: &str)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug + PartialEq,
    {
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            format!("\"{wire}\"")
        );
        let decoded: T = serde_json::from_str(&format!("\"{wire}\"")).unwrap();
        assert_eq!(decoded, value);
    }

    macro_rules! assert_enum_wire {
        ($($value:path => $wire:literal),+ $(,)?) => {
            $(
                assert_enum_wire_value($value, $wire);
            )+
        };
    }

    #[test]
    fn compliance_enums_use_persisted_wire_values_for_json() {
        assert_enum_wire!(
            ComplianceRiskLevel::Info => "INFO", ComplianceRiskLevel::Low => "LOW", ComplianceRiskLevel::Medium => "MEDIUM", ComplianceRiskLevel::High => "HIGH", ComplianceRiskLevel::Critical => "CRITICAL",
            RegulationImpactStatus::Draft => "DRAFT", RegulationImpactStatus::Active => "ACTIVE", RegulationImpactStatus::Superseded => "SUPERSEDED", RegulationImpactStatus::Archived => "ARCHIVED",
            ObligationType::Legal => "LEGAL", ObligationType::Regulatory => "REGULATORY", ObligationType::Contractual => "CONTRACTUAL", ObligationType::InternalPolicy => "INTERNAL_POLICY", ObligationType::ControlRequirement => "CONTROL_REQUIREMENT",
            ComplianceScopeKind::Org => "ORG", ComplianceScopeKind::Branch => "BRANCH", ComplianceScopeKind::Site => "SITE", ComplianceScopeKind::Team => "TEAM", ComplianceScopeKind::Role => "ROLE",
            ObligationStatus::Draft => "DRAFT", ObligationStatus::Active => "ACTIVE", ObligationStatus::Waived => "WAIVED", ObligationStatus::Superseded => "SUPERSEDED", ObligationStatus::Archived => "ARCHIVED",
            ReviewCadence::Monthly => "MONTHLY", ReviewCadence::Quarterly => "QUARTERLY", ReviewCadence::SemiAnnual => "SEMI_ANNUAL", ReviewCadence::Annual => "ANNUAL", ReviewCadence::EventDriven => "EVENT_DRIVEN",
            FrameworkKind::LegalBaseline => "LEGAL_BASELINE", FrameworkKind::InternalControl => "INTERNAL_CONTROL", FrameworkKind::CustomerControl => "CUSTOMER_CONTROL", FrameworkKind::SecurityStandard => "SECURITY_STANDARD", FrameworkKind::SafetyStandard => "SAFETY_STANDARD", FrameworkKind::AuditProgram => "AUDIT_PROGRAM",
            FrameworkStatus::Draft => "DRAFT", FrameworkStatus::Active => "ACTIVE", FrameworkStatus::Retired => "RETIRED", FrameworkStatus::Archived => "ARCHIVED",
            ControlType::Preventive => "PREVENTIVE", ControlType::Detective => "DETECTIVE", ControlType::Corrective => "CORRECTIVE", ControlType::Directive => "DIRECTIVE", ControlType::Compensating => "COMPENSATING",
            ControlCadence::Continuous => "CONTINUOUS", ControlCadence::Daily => "DAILY", ControlCadence::Weekly => "WEEKLY", ControlCadence::Monthly => "MONTHLY", ControlCadence::Quarterly => "QUARTERLY", ControlCadence::Annual => "ANNUAL", ControlCadence::EventDriven => "EVENT_DRIVEN",
            ControlStatus::Draft => "DRAFT", ControlStatus::Active => "ACTIVE", ControlStatus::Retired => "RETIRED", ControlStatus::Archived => "ARCHIVED",
            ObligationRegulationRelationship::DerivedFrom => "DERIVED_FROM", ObligationRegulationRelationship::AmendedBy => "AMENDED_BY", ObligationRegulationRelationship::SupersededBy => "SUPERSEDED_BY", ObligationRegulationRelationship::Interprets => "INTERPRETS", ObligationRegulationRelationship::Evidences => "EVIDENCES",
            CoverageLevel::Primary => "PRIMARY", CoverageLevel::Partial => "PARTIAL", CoverageLevel::Supporting => "SUPPORTING", CoverageLevel::Compensating => "COMPENSATING",
            CoverageStatus::Active => "ACTIVE", CoverageStatus::Retired => "RETIRED",
            EvidenceTargetType::AuditEvent => "audit_event", EvidenceTargetType::EvidenceMedia => "evidence_media", EvidenceTargetType::WorkflowRun => "workflow_run", EvidenceTargetType::WorkflowTask => "workflow_task", EvidenceTargetType::ObjectLink => "object_link", EvidenceTargetType::GovernanceFinding => "governance_finding", EvidenceTargetType::ExternalDocument => "external_document", EvidenceTargetType::FutureEvObject => "future_ev_object",
            EvidenceBindingStatus::Proposed => "PROPOSED", EvidenceBindingStatus::Accepted => "ACCEPTED", EvidenceBindingStatus::Rejected => "REJECTED", EvidenceBindingStatus::Expired => "EXPIRED", EvidenceBindingStatus::Retracted => "RETRACTED",
            EvidenceConfidence::Low => "LOW", EvidenceConfidence::Medium => "MEDIUM", EvidenceConfidence::High => "HIGH", EvidenceConfidence::System => "SYSTEM",
        );
    }

    #[test]
    fn org_scope_rejects_resource_refs() {
        let mut scope = ComplianceScope::org();
        scope.scope_ref = Some(uuid::Uuid::new_v4());
        assert!(scope.validate().is_err());
    }

    #[test]
    fn site_scope_requires_branch_for_deny_by_omission_filtering() {
        let scope = ComplianceScope {
            kind: ComplianceScopeKind::Site,
            scope_ref: Some(uuid::Uuid::new_v4()),
            branch_id: None,
            site_id: Some(console_kernel_core::SiteId::new()),
        };
        assert!(scope.validate().is_err());
    }

    #[test]
    fn evidence_status_rejects_terminal_reopen() {
        assert!(
            validate_evidence_status_transition(
                EvidenceBindingStatus::Rejected,
                EvidenceBindingStatus::Accepted
            )
            .is_err()
        );
    }

    #[test]
    fn code_validation_requires_prefix_and_four_digits() {
        assert!(validate_prefixed_code("CP", "CP-0001").is_ok());
        assert!(validate_prefixed_code("CP", "RG-0001").is_err());
        assert!(validate_prefixed_code("CP", "CP-12").is_err());
    }
}
