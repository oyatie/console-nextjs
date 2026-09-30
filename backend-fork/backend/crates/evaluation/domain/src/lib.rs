//! Evaluation-console finite-state vocabulary (CAP-EVALUATION-CONSOLE).
//!
//! The cycle is the lifecycle object (design charter §3.9/§15): a strict
//! linear FSM whose forward edges are preflight-gated at the adapter. Reviews
//! are one-way DRAFT → SUBMITTED documents; the per-subject chip state is
//! derived, never stored.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;
use serde::{Deserialize, Serialize};

/// `evaluation_cycles.stage` — DRAFT → OPEN → CALIBRATION → FINALIZED → ARCHIVED.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CycleStage {
    Draft,
    Open,
    Calibration,
    Finalized,
    Archived,
}

impl CycleStage {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Open => "OPEN",
            Self::Calibration => "CALIBRATION",
            Self::Finalized => "FINALIZED",
            Self::Archived => "ARCHIVED",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "DRAFT" => Ok(Self::Draft),
            "OPEN" => Ok(Self::Open),
            "CALIBRATION" => Ok(Self::Calibration),
            "FINALIZED" => Ok(Self::Finalized),
            "ARCHIVED" => Ok(Self::Archived),
            _ => Err(KernelError::conflict("unknown evaluation cycle stage")),
        }
    }

    /// The single forward transition available from this stage, if any.
    #[must_use]
    pub const fn next_transition(self) -> Option<CycleTransition> {
        match self {
            Self::Draft => Some(CycleTransition::Open),
            Self::Open => Some(CycleTransition::StartCalibration),
            Self::Calibration => Some(CycleTransition::Finalize),
            Self::Finalized => Some(CycleTransition::Archive),
            Self::Archived => None,
        }
    }
}

/// A named forward edge of the cycle FSM (also the preflight vocabulary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleTransition {
    Open,
    StartCalibration,
    Finalize,
    Archive,
}

impl CycleTransition {
    /// The stage this transition may only depart from.
    #[must_use]
    pub const fn from_stage(self) -> CycleStage {
        match self {
            Self::Open => CycleStage::Draft,
            Self::StartCalibration => CycleStage::Open,
            Self::Finalize => CycleStage::Calibration,
            Self::Archive => CycleStage::Finalized,
        }
    }

    /// The stage this transition lands in.
    #[must_use]
    pub const fn to_stage(self) -> CycleStage {
        match self {
            Self::Open => CycleStage::Open,
            Self::StartCalibration => CycleStage::Calibration,
            Self::Finalize => CycleStage::Finalized,
            Self::Archive => CycleStage::Archived,
        }
    }

    /// Fail closed unless the cycle is exactly at this transition's origin.
    pub fn guard(self, current: CycleStage) -> Result<(), KernelError> {
        if current == self.from_stage() {
            Ok(())
        } else {
            Err(KernelError::conflict(format!(
                "cycle stage {} does not allow this transition",
                current.as_db()
            )))
        }
    }
}

/// `evaluation_cycles.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CycleKind {
    Regular,
    Probation,
}

impl CycleKind {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Regular => "REGULAR",
            Self::Probation => "PROBATION",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "REGULAR" => Ok(Self::Regular),
            "PROBATION" => Ok(Self::Probation),
            _ => Err(KernelError::conflict("unknown evaluation cycle kind")),
        }
    }
}

/// The five-tier grade scale used by drafts, calibration, and the ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Grade {
    S,
    A,
    B,
    C,
    D,
}

impl Grade {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::S => "S",
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "S" => Ok(Self::S),
            "A" => Ok(Self::A),
            "B" => Ok(Self::B),
            "C" => Ok(Self::C),
            "D" => Ok(Self::D),
            _ => Err(KernelError::conflict("unknown evaluation grade")),
        }
    }
}

/// `evaluation_reviews.kind` — who the recorded assessment speaks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewKind {
    #[serde(rename = "SELF")]
    SelfReview,
    #[serde(rename = "MANAGER")]
    Manager,
}

impl ReviewKind {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::SelfReview => "SELF",
            Self::Manager => "MANAGER",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "SELF" => Ok(Self::SelfReview),
            "MANAGER" => Ok(Self::Manager),
            _ => Err(KernelError::conflict("unknown evaluation review kind")),
        }
    }

    /// Parse the lowercase REST path segment (`self` | `manager`).
    pub fn from_path(value: &str) -> Result<Self, KernelError> {
        match value {
            "self" => Ok(Self::SelfReview),
            "manager" => Ok(Self::Manager),
            _ => Err(KernelError::not_found("unknown review kind")),
        }
    }
}

/// `evaluation_reviews.status` — DRAFT → SUBMITTED (terminal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewStatus {
    Draft,
    Submitted,
}

impl ReviewStatus {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Submitted => "SUBMITTED",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "DRAFT" => Ok(Self::Draft),
            "SUBMITTED" => Ok(Self::Submitted),
            _ => Err(KernelError::conflict("unknown evaluation review status")),
        }
    }
}

/// `evaluation_goals.metric_kind` — goals are typed, never prose (§4-19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MetricKind {
    Kpi,
    Attendance,
    Task,
    Custom,
}

impl MetricKind {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Kpi => "KPI",
            Self::Attendance => "ATTENDANCE",
            Self::Task => "TASK",
            Self::Custom => "CUSTOM",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "KPI" => Ok(Self::Kpi),
            "ATTENDANCE" => Ok(Self::Attendance),
            "TASK" => Ok(Self::Task),
            "CUSTOM" => Ok(Self::Custom),
            _ => Err(KernelError::conflict("unknown evaluation metric kind")),
        }
    }
}

/// `evaluation_evidence_links.object_kind` — typed drillable refs (§4.7-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvidenceKind {
    Attendance,
    WorkOrder,
    Approval,
    Kpi,
    Other,
}

impl EvidenceKind {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Attendance => "ATTENDANCE",
            Self::WorkOrder => "WORK_ORDER",
            Self::Approval => "APPROVAL",
            Self::Kpi => "KPI",
            Self::Other => "OTHER",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, KernelError> {
        match value {
            "ATTENDANCE" => Ok(Self::Attendance),
            "WORK_ORDER" => Ok(Self::WorkOrder),
            "APPROVAL" => Ok(Self::Approval),
            "KPI" => Ok(Self::Kpi),
            "OTHER" => Ok(Self::Other),
            _ => Err(KernelError::conflict("unknown evaluation evidence kind")),
        }
    }
}

/// Derived per-subject chip state (no stored column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SubjectState {
    Enrolled,
    InReview,
    Reviewed,
    Calibrated,
    Finalized,
}

/// Derive the subject chip from persisted facts, most-advanced first.
#[must_use]
pub const fn derive_subject_state(
    has_any_review: bool,
    manager_submitted: bool,
    calibrated: bool,
    finalized: bool,
) -> SubjectState {
    if finalized {
        SubjectState::Finalized
    } else if calibrated {
        SubjectState::Calibrated
    } else if manager_submitted {
        SubjectState::Reviewed
    } else if has_any_review {
        SubjectState::InReview
    } else {
        SubjectState::Enrolled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_transitions_form_one_forward_chain() {
        for (stage, transition) in [
            (CycleStage::Draft, CycleTransition::Open),
            (CycleStage::Open, CycleTransition::StartCalibration),
            (CycleStage::Calibration, CycleTransition::Finalize),
            (CycleStage::Finalized, CycleTransition::Archive),
        ] {
            assert_eq!(stage.next_transition(), Some(transition));
            transition
                .guard(stage)
                .unwrap_or_else(|_| panic!("{stage:?} must allow {transition:?}"));
        }
        assert_eq!(CycleStage::Archived.next_transition(), None);
    }

    #[test]
    fn transitions_fail_closed_from_any_other_stage() {
        for transition in [
            CycleTransition::Open,
            CycleTransition::StartCalibration,
            CycleTransition::Finalize,
            CycleTransition::Archive,
        ] {
            for stage in [
                CycleStage::Draft,
                CycleStage::Open,
                CycleStage::Calibration,
                CycleStage::Finalized,
                CycleStage::Archived,
            ] {
                assert_eq!(
                    transition.guard(stage).is_ok(),
                    stage == transition.from_stage()
                );
            }
        }
    }

    #[test]
    fn database_vocabulary_round_trips() {
        for stage in [
            CycleStage::Draft,
            CycleStage::Open,
            CycleStage::Calibration,
            CycleStage::Finalized,
            CycleStage::Archived,
        ] {
            assert_eq!(CycleStage::from_db(stage.as_db()).ok(), Some(stage));
        }
        for kind in [CycleKind::Regular, CycleKind::Probation] {
            assert_eq!(CycleKind::from_db(kind.as_db()).ok(), Some(kind));
        }
        for grade in [Grade::S, Grade::A, Grade::B, Grade::C, Grade::D] {
            assert_eq!(Grade::from_db(grade.as_db()).ok(), Some(grade));
        }
        for kind in [ReviewKind::SelfReview, ReviewKind::Manager] {
            assert_eq!(ReviewKind::from_db(kind.as_db()).ok(), Some(kind));
        }
        for status in [ReviewStatus::Draft, ReviewStatus::Submitted] {
            assert_eq!(ReviewStatus::from_db(status.as_db()).ok(), Some(status));
        }
        for metric in [
            MetricKind::Kpi,
            MetricKind::Attendance,
            MetricKind::Task,
            MetricKind::Custom,
        ] {
            assert_eq!(MetricKind::from_db(metric.as_db()).ok(), Some(metric));
        }
        for evidence in [
            EvidenceKind::Attendance,
            EvidenceKind::WorkOrder,
            EvidenceKind::Approval,
            EvidenceKind::Kpi,
            EvidenceKind::Other,
        ] {
            assert_eq!(EvidenceKind::from_db(evidence.as_db()).ok(), Some(evidence));
        }
        assert!(CycleStage::from_db("UNKNOWN").is_err());
        assert!(Grade::from_db("F").is_err());
    }

    #[test]
    fn review_kind_path_segments_parse_and_reject() {
        assert_eq!(
            ReviewKind::from_path("self").ok(),
            Some(ReviewKind::SelfReview)
        );
        assert_eq!(
            ReviewKind::from_path("manager").ok(),
            Some(ReviewKind::Manager)
        );
        assert!(ReviewKind::from_path("SELF").is_err());
        assert!(ReviewKind::from_path("peer").is_err());
    }

    #[test]
    fn subject_state_prefers_the_most_advanced_fact() {
        assert_eq!(
            derive_subject_state(false, false, false, false),
            SubjectState::Enrolled
        );
        assert_eq!(
            derive_subject_state(true, false, false, false),
            SubjectState::InReview
        );
        assert_eq!(
            derive_subject_state(true, true, false, false),
            SubjectState::Reviewed
        );
        assert_eq!(
            derive_subject_state(true, true, true, false),
            SubjectState::Calibrated
        );
        assert_eq!(
            derive_subject_state(true, true, true, true),
            SubjectState::Finalized
        );
        // Finalization wins even over an inconsistent review projection.
        assert_eq!(
            derive_subject_state(false, false, false, true),
            SubjectState::Finalized
        );
    }
}
