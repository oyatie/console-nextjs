//! Shared governance-finding upsert (BE-WF-HARDEN).
//!
//! Generalizes the near-identical `governance_findings` write that three
//! detectors each hand-rolled (financial self-approval, workflow self-approval,
//! integrity price-outlier) into one helper. Lives in `console-platform-db` (not
//! `console-integrity`, which owns the *domain* of governance findings) because
//! the layer-boundary gate forbids adapter-to-adapter dependencies: financial's
//! and workflow's adapter crates cannot depend on integrity's adapter crate,
//! but every adapter already depends on this platform crate.
//!
//! The caller supplies the transaction (already inside its own `with_audit`/
//! `with_audits` write), so the finding write shares the caller's tenant GUC
//! and commits atomically with the detection it records.

use console_kernel_core::OrgId;
use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

/// Shape of an OPEN `governance_findings` row, upserted by
/// [`upsert_open_finding_tx`]. The caller builds `evidence` (including any
/// `exemption_reason`) and chooses `severity`/`score`/`subject_user_id`; this
/// module owns only the row shape and the ON CONFLICT key.
pub struct OpenFinding<'a> {
    pub detector_id: &'a str,
    pub entity_type: &'a str,
    pub entity_id: &'a str,
    /// The user the finding is about (self-approval detectors), or `None` for
    /// object-scoped findings (price outlier).
    pub subject_user_id: Option<Uuid>,
    pub score: f64,
    pub severity: &'a str,
    pub evidence: serde_json::Value,
}

/// Idempotently upsert an OPEN governance finding inside the caller's
/// transaction, keyed by `(org_id, detector_id, entity_type, entity_id)`. A
/// re-detection re-opens the finding and refreshes score/severity/evidence.
///
/// This is the single owner of the `governance_findings` write shape, shared by
/// every detector that records a finding as a side effect of its own audited
/// write: financial + workflow self-approval SoD guards and the integrity
/// price-outlier detector. Runs on the armed `tx` (RLS-scoped as `console_rt`); the
/// caller commits.
pub async fn upsert_open_finding_tx(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    finding: OpenFinding<'_>,
) -> Result<(), sqlx::Error> {
    let now = OffsetDateTime::now_utc();
    sqlx::query(
        r#"
        INSERT INTO governance_findings
            (id, org_id, detector_id, entity_type, entity_id,
             subject_user_id, score, severity, evidence, status, detected_at, created_at, updated_at)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'OPEN', $10, $10, $10)
        ON CONFLICT (org_id, detector_id, entity_type, entity_id) DO UPDATE
            SET score       = EXCLUDED.score,
                severity    = EXCLUDED.severity,
                evidence    = EXCLUDED.evidence,
                status      = 'OPEN',
                detected_at = EXCLUDED.detected_at,
                updated_at  = EXCLUDED.updated_at
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(*org.as_uuid())
    .bind(finding.detector_id)
    .bind(finding.entity_type)
    .bind(finding.entity_id)
    .bind(finding.subject_user_id)
    .bind(finding.score)
    .bind(finding.severity)
    .bind(sqlx::types::Json(finding.evidence))
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}
