//! Postgres adapter for governance findings.
//!
//! Reads via `with_org_conn` (RLS-armed read-only path).
//! Triage writes via `with_audit` (audited mutation path).

use console_kernel_core::{AuditAction, AuditEvent, ErrorKind, KernelError, OrgId, UserId};
use console_platform_db::{
    DbError, OpenFinding, upsert_open_finding_tx, with_audit, with_org_conn,
};
use console_platform_request_context::current_org;
use sqlx::{PgPool, Row};

use crate::domain::{
    FindingSeverity, FindingStatus, GovernanceFinding, PriceOutlierOutput, TriageFindingCommand,
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum PgIntegrityError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl From<sqlx::Error> for PgIntegrityError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

impl PgIntegrityError {
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        match self {
            Self::Domain(e) => e.kind,
            Self::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => ErrorKind::NotFound,
            Self::Db(_) => ErrorKind::Internal,
        }
    }
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PgIntegrityStore {
    pool: PgPool,
}

impl PgIntegrityStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// List open (and recently reviewed) governance findings for the current org.
    /// Returns findings ordered by severity DESC, detected_at DESC.
    /// `status_filter` is optional; if `None`, returns all statuses.
    pub async fn list_findings(
        &self,
        status_filter: Option<&str>,
    ) -> Result<Vec<GovernanceFinding>, PgIntegrityError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgIntegrityError>(&self.pool, org, move |tx| {
            let status_filter = status_filter.map(ToString::to_string);
            Box::pin(async move {
                let rows = if let Some(status) = status_filter {
                    sqlx::query(FINDING_SELECT_SQL)
                        .bind(status)
                        .fetch_all(tx.as_mut())
                        .await?
                } else {
                    sqlx::query(FINDING_SELECT_ALL_SQL)
                        .fetch_all(tx.as_mut())
                        .await?
                };
                rows.into_iter()
                    .map(finding_from_row)
                    .collect::<Result<Vec<_>, _>>()
            })
        })
        .await
    }

    /// Triage a finding: transition status from OPEN → REVIEWED / DISMISSED / ESCALATED.
    /// The transition itself is audited via `with_audit`.
    pub async fn triage_finding(
        &self,
        command: TriageFindingCommand,
    ) -> Result<GovernanceFinding, PgIntegrityError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let action = AuditAction::new("anomaly.finding.triage")?;
        let event = AuditEvent::new(
            Some(command.reviewer),
            action,
            "governance_finding",
            command.finding_id.to_string(),
            command.trace,
            command.occurred_at,
        )
        .with_org(org);

        with_audit::<_, GovernanceFinding, PgIntegrityError>(&self.pool, event, |tx| {
            let finding_id = command.finding_id;
            let reviewer = command.reviewer;
            let new_status = command.new_status.as_db_str();
            let memo = command.memo.clone();
            let occurred_at = command.occurred_at;
            Box::pin(async move {
                // Verify the finding exists and belongs to this org.
                let existing = sqlx::query(
                    "SELECT id, status FROM governance_findings WHERE id = $1 AND org_id = $2",
                )
                .bind(finding_id)
                .bind(org_uuid)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("governance finding was not found"))?;

                let current_status: String = existing.try_get("status")?;
                if current_status != "OPEN" {
                    return Err(KernelError::conflict("only OPEN findings can be triaged").into());
                }

                sqlx::query(
                    r#"
                    UPDATE governance_findings
                    SET status      = $2,
                        reviewed_by = $3,
                        reviewed_at = $4,
                        review_memo = $5,
                        updated_at  = $4
                    WHERE id = $1
                    "#,
                )
                .bind(finding_id)
                .bind(new_status)
                .bind(*reviewer.as_uuid())
                .bind(occurred_at)
                .bind(memo.as_deref())
                .execute(tx.as_mut())
                .await?;

                // Re-fetch the updated row.
                let row = sqlx::query(FINDING_BY_ID_SQL)
                    .bind(finding_id)
                    .fetch_one(tx.as_mut())
                    .await?;
                finding_from_row(row)
            })
        })
        .await
    }

    /// Write a price-outlier finding (idempotent upsert). Called OnWrite when a
    /// purchase request is submitted. No-ops when `output.is_sparse`.
    pub async fn upsert_price_outlier_finding(
        &self,
        output: PriceOutlierOutput,
    ) -> Result<(), PgIntegrityError> {
        if output.is_sparse {
            return Ok(());
        }
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();

        with_org_conn::<_, _, PgIntegrityError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let PriceOutlierOutput {
                    detector_id,
                    entity_type,
                    entity_id,
                    score,
                    severity,
                    evidence,
                    ..
                } = output;
                upsert_open_finding_tx(
                    tx,
                    OrgId::from_uuid(org_uuid),
                    OpenFinding {
                        detector_id,
                        entity_type,
                        entity_id: &entity_id,
                        // Price-outlier findings are about a purchase, not a user.
                        subject_user_id: None,
                        score,
                        severity: severity.as_db_str(),
                        evidence,
                    },
                )
                .await?;
                Ok(())
            })
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// SQL helpers
// ---------------------------------------------------------------------------

const FINDING_SELECT_SQL: &str = r#"
    SELECT id, org_id, detector_id, entity_type, entity_id,
           source_audit_event_id, subject_user_id,
           score, severity, evidence, status,
           detected_at, created_at, updated_at,
           reviewed_by, reviewed_at, review_memo
    FROM governance_findings
    WHERE status = $1
    ORDER BY
        CASE severity WHEN 'CRITICAL' THEN 0 WHEN 'HIGH' THEN 1
                      WHEN 'MEDIUM' THEN 2 WHEN 'LOW' THEN 3 ELSE 4 END ASC,
        detected_at DESC
"#;

const FINDING_SELECT_ALL_SQL: &str = r#"
    SELECT id, org_id, detector_id, entity_type, entity_id,
           source_audit_event_id, subject_user_id,
           score, severity, evidence, status,
           detected_at, created_at, updated_at,
           reviewed_by, reviewed_at, review_memo
    FROM governance_findings
    ORDER BY
        CASE severity WHEN 'CRITICAL' THEN 0 WHEN 'HIGH' THEN 1
                      WHEN 'MEDIUM' THEN 2 WHEN 'LOW' THEN 3 ELSE 4 END ASC,
        detected_at DESC
"#;

const FINDING_BY_ID_SQL: &str = r#"
    SELECT id, org_id, detector_id, entity_type, entity_id,
           source_audit_event_id, subject_user_id,
           score, severity, evidence, status,
           detected_at, created_at, updated_at,
           reviewed_by, reviewed_at, review_memo
    FROM governance_findings
    WHERE id = $1
"#;

fn finding_from_row(row: sqlx::postgres::PgRow) -> Result<GovernanceFinding, PgIntegrityError> {
    let severity_str: String = row.try_get("severity")?;
    let status_str: String = row.try_get("status")?;
    let org_id_uuid: uuid::Uuid = row.try_get("org_id")?;
    let subject_user_uuid: Option<uuid::Uuid> = row.try_get("subject_user_id")?;
    let reviewed_by_uuid: Option<uuid::Uuid> = row.try_get("reviewed_by")?;
    let evidence: serde_json::Value = row.try_get("evidence")?;

    Ok(GovernanceFinding {
        id: row.try_get("id")?,
        org_id: OrgId::from_uuid(org_id_uuid),
        detector_id: row.try_get("detector_id")?,
        entity_type: row.try_get("entity_type")?,
        entity_id: row.try_get("entity_id")?,
        source_audit_event_id: row.try_get("source_audit_event_id")?,
        subject_user_id: subject_user_uuid.map(UserId::from_uuid),
        score: row.try_get("score")?,
        severity: FindingSeverity::from_db_str(&severity_str)?,
        evidence,
        status: FindingStatus::from_db_str(&status_str)?,
        detected_at: row.try_get("detected_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        reviewed_by: reviewed_by_uuid.map(UserId::from_uuid),
        reviewed_at: row.try_get("reviewed_at")?,
        review_memo: row.try_get("review_memo")?,
    })
}
