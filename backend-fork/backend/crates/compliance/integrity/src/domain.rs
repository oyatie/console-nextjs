//! Pure domain types for governance findings.
//!
//! No I/O, no async, no sqlx. Everything here is callable from any layer.

use console_kernel_core::{KernelError, OrgId, Timestamp, UserId};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Finding severity and status
// ---------------------------------------------------------------------------

/// Severity of a governance finding. Maps to the `severity` CHECK column.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FindingSeverity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl FindingSeverity {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Low => "LOW",
            Self::Medium => "MEDIUM",
            Self::High => "HIGH",
            Self::Critical => "CRITICAL",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "INFO" => Ok(Self::Info),
            "LOW" => Ok(Self::Low),
            "MEDIUM" => Ok(Self::Medium),
            "HIGH" => Ok(Self::High),
            "CRITICAL" => Ok(Self::Critical),
            other => Err(KernelError::validation(format!(
                "unknown finding severity {other:?}"
            ))),
        }
    }

    /// Map a suspicion score [0.0, 1.0] from `PriceIntelResult::suspicion_score`
    /// to a severity level.
    #[must_use]
    pub fn from_suspicion_score(score: f64) -> Self {
        if score >= 0.85 {
            Self::Critical
        } else if score >= 0.65 {
            Self::High
        } else if score >= 0.45 {
            Self::Medium
        } else if score >= 0.25 {
            Self::Low
        } else {
            Self::Info
        }
    }
}

/// Lifecycle status of a governance finding. Maps to the `status` CHECK column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FindingStatus {
    Open,
    Reviewed,
    Dismissed,
    Escalated,
}

impl FindingStatus {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::Reviewed => "REVIEWED",
            Self::Dismissed => "DISMISSED",
            Self::Escalated => "ESCALATED",
        }
    }

    pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
        match value {
            "OPEN" => Ok(Self::Open),
            "REVIEWED" => Ok(Self::Reviewed),
            "DISMISSED" => Ok(Self::Dismissed),
            "ESCALATED" => Ok(Self::Escalated),
            other => Err(KernelError::validation(format!(
                "unknown finding status {other:?}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// GovernanceFinding
// ---------------------------------------------------------------------------

/// A persisted governance finding as returned by list/detail queries.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GovernanceFinding {
    pub id: Uuid,
    pub org_id: OrgId,
    pub detector_id: String,
    pub entity_type: String,
    pub entity_id: String,
    pub source_audit_event_id: Option<Uuid>,
    pub subject_user_id: Option<UserId>,
    pub score: f64,
    pub severity: FindingSeverity,
    pub evidence: serde_json::Value,
    pub status: FindingStatus,
    #[serde(with = "time::serde::rfc3339")]
    pub detected_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: Timestamp,
    pub reviewed_by: Option<UserId>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub reviewed_at: Option<Timestamp>,
    pub review_memo: Option<String>,
}

// ---------------------------------------------------------------------------
// Triage command
// ---------------------------------------------------------------------------

/// Valid target statuses for triage (OPEN → one of these).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TriageTarget {
    Reviewed,
    Dismissed,
    Escalated,
}

impl TriageTarget {
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Reviewed => "REVIEWED",
            Self::Dismissed => "DISMISSED",
            Self::Escalated => "ESCALATED",
        }
    }
}

/// Command to triage (update the status of) a governance finding.
#[derive(Debug, Clone)]
pub struct TriageFindingCommand {
    pub finding_id: Uuid,
    pub reviewer: UserId,
    pub new_status: TriageTarget,
    /// Optional memo (max 2000 chars). Required for DISMISSED and ESCALATED.
    pub memo: Option<String>,
    pub occurred_at: Timestamp,
    pub trace: console_kernel_core::TraceContext,
}

/// Validate a triage memo: required for DISMISSED/ESCALATED, optional for REVIEWED.
/// Max 2000 chars (mirrors DB CHECK).
pub fn validate_triage_memo(
    target: TriageTarget,
    memo: &Option<String>,
) -> Result<(), KernelError> {
    match memo {
        Some(m) if m.trim().is_empty() => {
            Err(KernelError::validation("review_memo must not be blank"))
        }
        Some(m) if m.len() > 2000 => Err(KernelError::validation(
            "review_memo must not exceed 2000 characters",
        )),
        None if matches!(target, TriageTarget::Dismissed | TriageTarget::Escalated) => {
            Err(KernelError::validation(
                "review_memo is required when dismissing or escalating a finding",
            ))
        }
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Detector trait
// ---------------------------------------------------------------------------

/// A governance detector. Detectors are called OnWrite (cheap) or in batch.
///
/// The trait is pure: it takes context and returns a finding payload.
/// The caller is responsible for persisting the result to `governance_findings`.
pub trait Detector: Send + Sync {
    /// Dot-namespaced detector identifier, e.g. `"integrity.price_outlier"`.
    fn detector_id(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// Price-outlier detector output
// ---------------------------------------------------------------------------

/// The result of running the price-outlier detector on a single purchase request.
/// The caller (store layer) decides whether to persist this as a finding.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PriceOutlierOutput {
    pub detector_id: &'static str,
    pub entity_type: &'static str,
    pub entity_id: String,
    pub score: f64,
    pub severity: FindingSeverity,
    /// The full `PriceIntelResult` evidence, serialised as JSON.
    pub evidence: serde_json::Value,
    /// `true` if the result is sparse (too few peers) — caller should NOT persist.
    pub is_sparse: bool,
}

/// Run the price-outlier detector for a single purchase amount against the
/// `peers` distribution for its (vendor, category) bucket.
///
/// Returns `None` if `is_sparse` — no finding should be persisted.
///
/// This is a pure function callable from any layer.
#[must_use]
pub fn run_price_outlier_detector(
    purchase_request_id: uuid::Uuid,
    amount_won: i64,
    peers: &[i64],
) -> PriceOutlierOutput {
    use console_kernel_core::compute_price_intel;

    let result = compute_price_intel(amount_won, peers);
    let is_sparse = result.is_sparse();
    let score = result.suspicion_score().unwrap_or(0.0);
    let severity = FindingSeverity::from_suspicion_score(score);

    let evidence = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);

    PriceOutlierOutput {
        detector_id: "anomaly.price_outlier",
        entity_type: "financial_purchase_request",
        entity_id: purchase_request_id.to_string(),
        score,
        severity,
        evidence,
        is_sparse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_roundtrip() {
        for sev in [
            FindingSeverity::Info,
            FindingSeverity::Low,
            FindingSeverity::Medium,
            FindingSeverity::High,
            FindingSeverity::Critical,
        ] {
            assert_eq!(FindingSeverity::from_db_str(sev.as_db_str()).unwrap(), sev);
        }
    }

    #[test]
    fn status_roundtrip() {
        for status in [
            FindingStatus::Open,
            FindingStatus::Reviewed,
            FindingStatus::Dismissed,
            FindingStatus::Escalated,
        ] {
            assert_eq!(
                FindingStatus::from_db_str(status.as_db_str()).unwrap(),
                status
            );
        }
    }

    #[test]
    fn severity_from_score() {
        assert_eq!(
            FindingSeverity::from_suspicion_score(0.0),
            FindingSeverity::Info
        );
        assert_eq!(
            FindingSeverity::from_suspicion_score(0.25),
            FindingSeverity::Low
        );
        assert_eq!(
            FindingSeverity::from_suspicion_score(0.45),
            FindingSeverity::Medium
        );
        assert_eq!(
            FindingSeverity::from_suspicion_score(0.65),
            FindingSeverity::High
        );
        assert_eq!(
            FindingSeverity::from_suspicion_score(0.85),
            FindingSeverity::Critical
        );
    }

    #[test]
    fn triage_memo_validation() {
        // Reviewed: memo optional.
        assert!(validate_triage_memo(TriageTarget::Reviewed, &None).is_ok());
        // Dismissed: memo required.
        assert!(validate_triage_memo(TriageTarget::Dismissed, &None).is_err());
        assert!(validate_triage_memo(TriageTarget::Dismissed, &Some("valid memo".into())).is_ok());
        // Blank memo rejected.
        assert!(validate_triage_memo(TriageTarget::Reviewed, &Some("   ".into())).is_err());
        // Too long.
        let long = "x".repeat(2001);
        assert!(validate_triage_memo(TriageTarget::Reviewed, &Some(long)).is_err());
    }

    #[test]
    fn price_outlier_sparse_below_min_sample() {
        let peers: Vec<i64> = vec![100_000, 90_000, 80_000];
        let out = run_price_outlier_detector(uuid::Uuid::new_v4(), 200_000, &peers);
        assert!(out.is_sparse, "should be sparse with 3 peers");
    }

    #[test]
    fn price_outlier_detects_extreme_value() {
        let peers: Vec<i64> = (0..10).map(|i| 100_000 + i * 1_000).collect();
        // 10x the median is a clear outlier.
        let out = run_price_outlier_detector(uuid::Uuid::new_v4(), 1_000_000, &peers);
        assert!(!out.is_sparse);
        assert!(
            out.score > 0.5,
            "extreme value should have high score: {}",
            out.score
        );
    }
}
