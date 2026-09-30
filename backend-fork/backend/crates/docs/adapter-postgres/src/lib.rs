//! Postgres adapter for docs/evidence EV objects.
//!
//! Reads run through `with_org_conn`; mutations run through `with_audits` and
//! attach `org_id` to every emitted audit event. Runtime SQL keeps this crate
//! SQLx-offline friendly; no `.sqlx` cache entries are required.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_docs_application::{
    AppendCustodyEventCommand, ApplyLegalHoldCommand, CreateEvidenceObjectCommand,
    CustodyEventView, DisposeEvidenceObjectCommand, EvidenceCopyView, EvidenceExportView,
    EvidenceObjectCursor, EvidenceObjectDetail, EvidenceObjectPage, EvidenceObjectView,
    LegalHoldRecordView, ListEvidenceObjectsQuery, RecomputeAdmissibilityCommand,
    RecordTsaProofCommand, RegisterEvidenceCopyCommand, RegisterEvidenceCopyInput,
    ReleaseLegalHoldCommand, TimestampAuthorityProofInput, TimestampAuthorityProofView,
    evidence_audit_event,
};
use console_docs_domain::{
    AdmissibilityInputs, AdmissibilityReason, AdmissibilityStatus, CustodyStage, DerivativeKind,
    EvidenceClassification, EvidenceCode, EvidenceCopyEvidentiaryStatus, EvidenceCopyKind,
    EvidenceSourceRef, EvidenceSourceType, EvidenceStorageRef, LegalHoldState, LegalHoldStatus,
    Sha256Digest, TsaProofStatus, WormStorageStatus, evaluate_admissibility,
};
use console_kernel_core::{
    AuditEventId, ErrorKind, EvidenceCopyId, EvidenceCustodyEventId, EvidenceExportId, EvidenceId,
    EvidenceLegalHoldId, EvidenceObjectId, EvidenceTsaProofId, KernelError, OrgId, Timestamp,
    UserId,
};
use console_platform_db::{DbError, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

#[derive(Debug, thiserror::Error)]
pub enum PgDocsError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl PgDocsError {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Domain(error) => error.kind,
            Self::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => ErrorKind::NotFound,
            Self::Db(DbError::Sqlx(sqlx::Error::Database(error)))
                if error.code().is_some_and(|code| code == "23505") =>
            {
                ErrorKind::Conflict
            }
            Self::Db(_) => ErrorKind::Internal,
        }
    }
}

impl From<sqlx::Error> for PgDocsError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

#[derive(Debug, Clone)]
pub struct PgDocsStore {
    pool: PgPool,
}

impl PgDocsStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn get_object(
        &self,
        evidence_object_id: EvidenceObjectId,
    ) -> Result<Option<EvidenceObjectDetail>, PgDocsError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, Option<EvidenceObjectDetail>, PgDocsError>(&self.pool, org, move |tx| {
            Box::pin(async move { fetch_detail_tx(tx, evidence_object_id).await })
        })
        .await
    }

    pub async fn list_objects(
        &self,
        query: ListEvidenceObjectsQuery,
    ) -> Result<EvidenceObjectPage, PgDocsError> {
        let limit = normalized_limit(query.limit);
        let offset = query.offset.unwrap_or(0).max(0);
        let using_cursor = query.cursor.is_some();
        if using_cursor && offset != 0 {
            return Err(
                KernelError::validation("EV cursor paging cannot be combined with offset").into(),
            );
        }
        if let (Some(as_of), Some(cursor)) = (query.as_of, query.cursor.as_ref())
            && as_of != cursor.snapshot_sequence
        {
            return Err(KernelError::validation("EV cursor snapshot does not match as_of").into());
        }

        let org = current_org().map_err(KernelError::from)?;
        let (snapshot_sequence, total, rows) =
            with_org_conn::<_, _, PgDocsError>(&self.pool, org, move |tx| {
                Box::pin(async move {
                    let snapshot_sequence = match query.cursor.as_ref() {
                        Some(cursor) => cursor.snapshot_sequence,
                        None => match query.as_of {
                            Some(as_of) => as_of,
                            None => sqlx::query_scalar(
                                "SELECT COALESCE(MAX(register_sequence), 0) FROM docs_evidence_objects",
                            )
                            .fetch_one(tx.as_mut())
                            .await?,
                        },
                    };
                    let query_for_count = query.clone();
                    let mut count_builder = QueryBuilder::<Postgres>::new(
                        "SELECT COUNT(*) FROM docs_evidence_objects WHERE ",
                    );
                    push_object_filters(&mut count_builder, &query_for_count)?;
                    push_register_snapshot(&mut count_builder, snapshot_sequence, None);

                    let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
                    builder.push(OBJECT_COLUMNS);
                    builder.push(" FROM docs_evidence_objects WHERE ");
                    push_object_filters(&mut builder, &query)?;
                    push_register_snapshot(
                        &mut builder,
                        snapshot_sequence,
                        query.cursor.as_ref(),
                    );
                    builder.push(" ORDER BY register_sequence DESC, id DESC LIMIT ");
                    builder.push_bind(limit);
                    if query.cursor.is_none() {
                        builder.push(" OFFSET ");
                        builder.push_bind(offset);
                    }
                    let total: i64 = count_builder
                        .build_query_scalar()
                        .fetch_one(tx.as_mut())
                        .await?;
                    let rows = builder.build().fetch_all(tx.as_mut()).await?;
                    Ok((snapshot_sequence, total, rows))
                })
            })
            .await?;
        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            items.push(object_from_row(row)?);
        }
        let next_cursor = if items.len() == limit as usize {
            rows.last()
                .map(|row| -> Result<EvidenceObjectCursor, PgDocsError> {
                    Ok(EvidenceObjectCursor {
                        snapshot_sequence,
                        register_sequence: row.try_get("register_sequence")?,
                        id: EvidenceObjectId::from_uuid(row.try_get("id")?),
                    })
                })
                .transpose()?
        } else {
            None
        };
        Ok(EvidenceObjectPage {
            items,
            limit,
            offset: if using_cursor { 0 } else { offset },
            total,
            as_of: snapshot_sequence,
            next_cursor,
        })
    }

    // console-gate: state-changing-handler
    pub async fn create_object(
        &self,
        command: CreateEvidenceObjectCommand,
    ) -> Result<EvidenceObjectDetail, PgDocsError> {
        let title = normalize_required_text(&command.title, 200, "title")?;
        let description =
            normalize_optional_text(command.description.clone(), 4_000, "description")?;
        let initial_reason = normalize_required_text(
            &command.initial_custody_reason,
            2_000,
            "initial_custody_reason",
        )?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let object_id = EvidenceObjectId::new();
        let actor = command.actor;

        with_audits::<_, EvidenceObjectDetail, PgDocsError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let code = issue_evidence_code_tx(tx, org, command.occurred_at).await?;
                sqlx::query(
                    r#"
                    INSERT INTO docs_evidence_objects (
                        id, org_id, code, title, description, source_type, source_id, source_code,
                        classification, record_owner_user_id, current_custody_stage,
                        legal_hold_state, admissibility_status, created_by, updated_by,
                        created_at, updated_at
                    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                              'REGISTERED', 'CLEAR', 'REVIEW_NEEDED', $11, $11, $12, $12)
                    "#,
                )
                .bind(*object_id.as_uuid())
                .bind(org_uuid)
                .bind(code.as_str())
                .bind(&title)
                .bind(description.as_deref())
                .bind(command.source.source_type.as_db_str())
                .bind(&command.source.source_id)
                .bind(command.source.source_code.as_deref())
                .bind(command.classification.as_db_str())
                .bind(command.record_owner_user_id.map(|id| *id.as_uuid()))
                .bind(*actor.as_uuid())
                .bind(command.occurred_at)
                .execute(tx.as_mut())
                .await?;

                insert_custody_event_tx(
                    tx,
                    org,
                    object_id,
                    CustodyStage::Registered,
                    actor,
                    None,
                    None,
                    None,
                    initial_reason,
                    Some(command.source.clone()),
                    command.occurred_at,
                    true,
                )
                .await?;

                let mut copied = None;
                if let Some(original) = command.original.clone() {
                    if original.copy_kind != EvidenceCopyKind::Original {
                        return Err(KernelError::validation(
                            "create_object original copy must use copy_kind ORIGINAL",
                        )
                        .into());
                    }
                    let copy =
                        insert_copy_tx(tx, org, object_id, actor, original, command.occurred_at)
                            .await?;
                    copied = Some(copy);
                }

                if let Some(proof) = command.tsa_proof.clone() {
                    let Some(copy) = copied.as_ref() else {
                        return Err(KernelError::validation(
                            "tsa_proof requires an original copy in create_object",
                        )
                        .into());
                    };
                    insert_tsa_proof_tx(
                        tx,
                        org,
                        object_id,
                        copy.id,
                        copy.digest_sha256.clone(),
                        actor,
                        proof,
                        command.occurred_at,
                    )
                    .await?;
                }

                recompute_admissibility_tx(tx, object_id, actor, command.occurred_at).await?;
                let detail = fetch_detail_tx(tx, object_id)
                    .await?
                    .ok_or_else(|| KernelError::internal("created EV object was not readable"))?;
                let audit = evidence_audit_event(
                    "evidence_object.register",
                    Some(actor),
                    "evidence_object",
                    object_id,
                    command.trace,
                    command.occurred_at,
                )?
                .with_org(org)
                .with_snapshots(None, Some(object_snapshot(&detail.object)));
                Ok((detail, vec![audit]))
            })
        })
        .await
    }

    // console-gate: state-changing-handler
    pub async fn register_copy(
        &self,
        command: RegisterEvidenceCopyCommand,
    ) -> Result<EvidenceCopyView, PgDocsError> {
        let org = current_org().map_err(KernelError::from)?;
        let actor = command.actor;
        with_audits::<_, EvidenceCopyView, PgDocsError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let before = fetch_object_tx(tx, command.evidence_object_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("EV object was not found"))?;
                if before.disposed_at.is_some() {
                    return Err(KernelError::conflict(
                        "disposed EV object cannot accept new copies",
                    )
                    .into());
                }
                let copy = insert_copy_tx(
                    tx,
                    org,
                    command.evidence_object_id,
                    actor,
                    command.copy,
                    command.occurred_at,
                )
                .await?;
                let stage = if copy.worm_status.is_verified() {
                    CustodyStage::WormReplicated
                } else {
                    CustodyStage::HashRecorded
                };
                insert_custody_event_tx(
                    tx,
                    org,
                    command.evidence_object_id,
                    stage,
                    actor,
                    None,
                    None,
                    None,
                    format!("registered {} evidence copy", copy.copy_kind.as_db_str()),
                    None,
                    command.occurred_at,
                    true,
                )
                .await?;
                recompute_admissibility_tx(
                    tx,
                    command.evidence_object_id,
                    actor,
                    command.occurred_at,
                )
                .await?;
                let audit = evidence_audit_event(
                    if copy.copy_kind == EvidenceCopyKind::Original {
                        "evidence_copy.register_original"
                    } else {
                        "evidence_copy.register_derivative"
                    },
                    Some(actor),
                    "evidence_copy",
                    copy.id,
                    command.trace,
                    command.occurred_at,
                )?
                .with_org(org)
                .with_snapshots(None, Some(copy_snapshot(&copy)));
                Ok((copy, vec![audit]))
            })
        })
        .await
    }

    // console-gate: state-changing-handler
    pub async fn record_tsa_proof(
        &self,
        command: RecordTsaProofCommand,
    ) -> Result<TimestampAuthorityProofView, PgDocsError> {
        let org = current_org().map_err(KernelError::from)?;
        let actor = command.actor;
        with_audits::<_, TimestampAuthorityProofView, PgDocsError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let copy = fetch_copy_tx(tx, command.copy_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("EV copy was not found"))?;
                if copy.evidence_object_id != command.evidence_object_id {
                    return Err(
                        KernelError::not_found("EV copy was not found for this object").into(),
                    );
                }
                let proof = insert_tsa_proof_tx(
                    tx,
                    org,
                    command.evidence_object_id,
                    command.copy_id,
                    copy.digest_sha256,
                    actor,
                    command.proof,
                    command.occurred_at,
                )
                .await?;
                let stage = if proof.status.is_verified() {
                    CustodyStage::TsaVerified
                } else {
                    CustodyStage::TsaSubmitted
                };
                insert_custody_event_tx(
                    tx,
                    org,
                    command.evidence_object_id,
                    stage,
                    actor,
                    None,
                    None,
                    None,
                    format!("recorded TSA proof status {}", proof.status.as_db_str()),
                    None,
                    command.occurred_at,
                    true,
                )
                .await?;
                recompute_admissibility_tx(
                    tx,
                    command.evidence_object_id,
                    actor,
                    command.occurred_at,
                )
                .await?;
                let audit = evidence_audit_event(
                    if proof.status.is_verified() {
                        "evidence_tsa.verify"
                    } else if proof.status == TsaProofStatus::Failed {
                        "evidence_tsa.fail"
                    } else {
                        "evidence_tsa.record"
                    },
                    Some(actor),
                    "evidence_tsa_proof",
                    proof.id,
                    command.trace,
                    command.occurred_at,
                )?
                .with_org(org)
                .with_snapshots(None, Some(tsa_snapshot(&proof)));
                Ok((proof, vec![audit]))
            })
        })
        .await
    }

    // console-gate: state-changing-handler
    pub async fn append_custody_event(
        &self,
        command: AppendCustodyEventCommand,
    ) -> Result<CustodyEventView, PgDocsError> {
        let org = current_org().map_err(KernelError::from)?;
        let actor = command.actor;
        with_audits::<_, CustodyEventView, PgDocsError>(&self.pool, org, |tx| {
            Box::pin(async move {
                fetch_object_tx(tx, command.evidence_object_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("EV object was not found"))?;
                let event = insert_custody_event_tx(
                    tx,
                    org,
                    command.evidence_object_id,
                    command.stage,
                    actor,
                    command.from_custodian,
                    command.to_custodian,
                    command.location_label,
                    command.reason,
                    command.source_ref,
                    command.occurred_at,
                    true,
                )
                .await?;
                recompute_admissibility_tx(
                    tx,
                    command.evidence_object_id,
                    actor,
                    command.occurred_at,
                )
                .await?;
                let audit = evidence_audit_event(
                    "evidence_custody.transition",
                    Some(actor),
                    "evidence_custody_event",
                    event.id,
                    command.trace,
                    command.occurred_at,
                )?
                .with_org(org)
                .with_snapshots(None, Some(custody_snapshot(&event)));
                Ok((event, vec![audit]))
            })
        })
        .await
    }

    // console-gate: state-changing-handler
    pub async fn apply_legal_hold(
        &self,
        command: ApplyLegalHoldCommand,
    ) -> Result<LegalHoldRecordView, PgDocsError> {
        let org = current_org().map_err(KernelError::from)?;
        let actor = command.actor;
        with_audits::<_, LegalHoldRecordView, PgDocsError>(&self.pool, org, |tx| {
            Box::pin(async move {
                fetch_object_for_update_tx(tx, command.evidence_object_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("EV object was not found"))?;
                let hold_id = EvidenceLegalHoldId::new();
                let case_ref = normalize_required_text(&command.case_ref, 200, "case_ref")?;
                let basis = normalize_required_text(&command.basis, 2_000, "basis")?;
                let reason = normalize_required_text(&command.reason, 2_000, "reason")?;
                sqlx::query(
                    r#"
                    INSERT INTO docs_evidence_legal_holds (
                        id, org_id, evidence_object_id, status, case_ref, basis, reason,
                        applied_by, applied_at
                    ) VALUES ($1, $2, $3, 'ACTIVE', $4, $5, $6, $7, $8)
                    "#,
                )
                .bind(*hold_id.as_uuid())
                .bind(*org.as_uuid())
                .bind(*command.evidence_object_id.as_uuid())
                .bind(&case_ref)
                .bind(&basis)
                .bind(&reason)
                .bind(*actor.as_uuid())
                .bind(command.occurred_at)
                .execute(tx.as_mut())
                .await?;
                sqlx::query(
                    "UPDATE docs_evidence_objects SET legal_hold_state = 'ACTIVE', updated_by = $2, updated_at = $3 WHERE id = $1",
                )
                .bind(*command.evidence_object_id.as_uuid())
                .bind(*actor.as_uuid())
                .bind(command.occurred_at)
                .execute(tx.as_mut())
                .await?;
                insert_custody_event_tx(
                    tx,
                    org,
                    command.evidence_object_id,
                    CustodyStage::LegalHoldApplied,
                    actor,
                    None,
                    None,
                    None,
                    format!("legal hold applied: {case_ref}"),
                    None,
                    command.occurred_at,
                    true,
                )
                .await?;
                recompute_admissibility_tx(tx, command.evidence_object_id, actor, command.occurred_at)
                    .await?;
                let hold = fetch_legal_hold_tx(tx, hold_id)
                    .await?
                    .ok_or_else(|| KernelError::internal("created EV legal hold was not readable"))?;
                let audit = evidence_audit_event(
                    "evidence_legal_hold.apply",
                    Some(actor),
                    "evidence_legal_hold",
                    hold_id,
                    command.trace,
                    command.occurred_at,
                )?
                .with_org(org)
                .with_snapshots(None, Some(legal_hold_snapshot(&hold)));
                Ok((hold, vec![audit]))
            })
        })
        .await
    }

    // console-gate: state-changing-handler
    pub async fn release_legal_hold(
        &self,
        command: ReleaseLegalHoldCommand,
    ) -> Result<LegalHoldRecordView, PgDocsError> {
        let org = current_org().map_err(KernelError::from)?;
        let actor = command.actor;
        with_audits::<_, LegalHoldRecordView, PgDocsError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let before = fetch_legal_hold_for_update_tx(tx, command.hold_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("EV legal hold was not found"))?;
                if before.evidence_object_id != command.evidence_object_id {
                    return Err(KernelError::not_found("EV legal hold was not found for this object").into());
                }
                if before.status == LegalHoldStatus::Released {
                    return Err(KernelError::conflict("EV legal hold is already released").into());
                }
                let release_reason =
                    normalize_required_text(&command.release_reason, 2_000, "release_reason")?;
                sqlx::query(
                    r#"
                    UPDATE docs_evidence_legal_holds
                    SET status = 'RELEASED', released_by = $2, released_at = $3, release_reason = $4
                    WHERE id = $1
                    "#,
                )
                .bind(*command.hold_id.as_uuid())
                .bind(*actor.as_uuid())
                .bind(command.occurred_at)
                .bind(&release_reason)
                .execute(tx.as_mut())
                .await?;
                let active_count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM docs_evidence_legal_holds WHERE evidence_object_id = $1 AND status = 'ACTIVE'",
                )
                .bind(*command.evidence_object_id.as_uuid())
                .fetch_one(tx.as_mut())
                .await?;
                let hold_state = if active_count == 0 { "CLEAR" } else { "ACTIVE" };
                sqlx::query(
                    "UPDATE docs_evidence_objects SET legal_hold_state = $2, updated_by = $3, updated_at = $4 WHERE id = $1",
                )
                .bind(*command.evidence_object_id.as_uuid())
                .bind(hold_state)
                .bind(*actor.as_uuid())
                .bind(command.occurred_at)
                .execute(tx.as_mut())
                .await?;
                insert_custody_event_tx(
                    tx,
                    org,
                    command.evidence_object_id,
                    CustodyStage::LegalHoldReleased,
                    actor,
                    None,
                    None,
                    None,
                    format!("legal hold released: {}", before.case_ref),
                    None,
                    command.occurred_at,
                    true,
                )
                .await?;
                recompute_admissibility_tx(tx, command.evidence_object_id, actor, command.occurred_at)
                    .await?;
                let hold = fetch_legal_hold_tx(tx, command.hold_id)
                    .await?
                    .ok_or_else(|| KernelError::internal("released EV legal hold was not readable"))?;
                let audit = evidence_audit_event(
                    "evidence_legal_hold.release",
                    Some(actor),
                    "evidence_legal_hold",
                    command.hold_id,
                    command.trace,
                    command.occurred_at,
                )?
                .with_org(org)
                .with_snapshots(Some(legal_hold_snapshot(&before)), Some(legal_hold_snapshot(&hold)));
                Ok((hold, vec![audit]))
            })
        })
        .await
    }

    // console-gate: state-changing-handler
    pub async fn dispose_object(
        &self,
        command: DisposeEvidenceObjectCommand,
    ) -> Result<EvidenceObjectView, PgDocsError> {
        let org = current_org().map_err(KernelError::from)?;
        let actor = command.actor;
        with_audits::<_, EvidenceObjectView, PgDocsError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let before = fetch_object_for_update_tx(tx, command.evidence_object_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("EV object was not found"))?;
                before
                    .legal_hold_state
                    .ensure_destructive_operation_allowed()?;
                if before.disposed_at.is_some() {
                    return Err(KernelError::conflict("EV object is already disposed").into());
                }
                let reason = normalize_required_text(&command.reason, 2_000, "reason")?;
                let event = insert_custody_event_tx(
                    tx,
                    org,
                    command.evidence_object_id,
                    CustodyStage::Disposed,
                    actor,
                    None,
                    None,
                    None,
                    reason.clone(),
                    None,
                    command.occurred_at,
                    false,
                )
                .await?;
                let _ = event;
                sqlx::query(
                    r#"
                    UPDATE docs_evidence_objects
                    SET current_custody_stage = 'DISPOSED', disposed_at = $2,
                        disposal_reason = $3, disposed_by = $4, updated_by = $4, updated_at = $2
                    WHERE id = $1
                    "#,
                )
                .bind(*command.evidence_object_id.as_uuid())
                .bind(command.occurred_at)
                .bind(&reason)
                .bind(*actor.as_uuid())
                .execute(tx.as_mut())
                .await?;
                recompute_admissibility_tx(
                    tx,
                    command.evidence_object_id,
                    actor,
                    command.occurred_at,
                )
                .await?;
                let after = fetch_object_tx(tx, command.evidence_object_id)
                    .await?
                    .ok_or_else(|| KernelError::internal("disposed EV object was not readable"))?;
                let audit = evidence_audit_event(
                    "evidence_disposal.complete",
                    Some(actor),
                    "evidence_object",
                    command.evidence_object_id,
                    command.trace,
                    command.occurred_at,
                )?
                .with_org(org)
                .with_snapshots(
                    Some(object_snapshot(&before)),
                    Some(object_snapshot(&after)),
                );
                Ok((after, vec![audit]))
            })
        })
        .await
    }

    // console-gate: state-changing-handler
    pub async fn recompute_admissibility(
        &self,
        command: RecomputeAdmissibilityCommand,
    ) -> Result<EvidenceObjectView, PgDocsError> {
        let org = current_org().map_err(KernelError::from)?;
        let actor = command.actor;
        with_audits::<_, EvidenceObjectView, PgDocsError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let before = fetch_object_for_update_tx(tx, command.evidence_object_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("EV object was not found"))?;
                recompute_admissibility_tx(
                    tx,
                    command.evidence_object_id,
                    actor,
                    command.occurred_at,
                )
                .await?;
                let after = fetch_object_tx(tx, command.evidence_object_id)
                    .await?
                    .ok_or_else(|| {
                        KernelError::internal("recomputed EV object was not readable")
                    })?;
                let audit = evidence_audit_event(
                    "evidence_admissibility.recompute",
                    Some(actor),
                    "evidence_object",
                    command.evidence_object_id,
                    command.trace,
                    command.occurred_at,
                )?
                .with_org(org)
                .with_snapshots(
                    Some(object_snapshot(&before)),
                    Some(object_snapshot(&after)),
                );
                Ok((after, vec![audit]))
            })
        })
        .await
    }
}

const OBJECT_COLUMNS: &str = "id, code, title, description, source_type, source_id, source_code, \
    classification, record_owner_user_id, current_custody_stage, legal_hold_state, \
    admissibility_status, admissibility_reasons, admissibility_inputs, created_by, \
    updated_by, created_at, updated_at, disposed_at, register_sequence";

fn normalized_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

fn push_register_snapshot(
    builder: &mut QueryBuilder<Postgres>,
    snapshot_sequence: i64,
    cursor: Option<&EvidenceObjectCursor>,
) {
    // `register_sequence` is database-assigned and immutable. Unlike an event
    // timestamp, a post-snapshot insert cannot be backdated into this scan.
    builder.push(" AND register_sequence <= ");
    builder.push_bind(snapshot_sequence);
    if let Some(cursor) = cursor {
        builder.push(" AND (register_sequence, id) < (");
        builder.push_bind(cursor.register_sequence);
        builder.push(", ");
        builder.push_bind(*cursor.id.as_uuid());
        builder.push(")");
    }
}

fn push_object_filters(
    builder: &mut QueryBuilder<Postgres>,
    query: &ListEvidenceObjectsQuery,
) -> Result<(), KernelError> {
    builder.push("TRUE");
    if let Some(source_type) = query.source_type {
        builder.push(" AND source_type = ");
        builder.push_bind(source_type.as_db_str());
    }
    if let Some(source_id) = query
        .source_id
        .as_ref()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        builder.push(" AND source_id = ");
        builder.push_bind(source_id.to_owned());
    }
    if let Some(status) = query.admissibility_status {
        builder.push(" AND admissibility_status = ");
        builder.push_bind(status.as_db_str());
    }
    if let Some(hold) = query.legal_hold_state {
        builder.push(" AND legal_hold_state = ");
        builder.push_bind(hold.as_db_str());
    }
    if let Some(stage) = query.custody_stage {
        builder.push(" AND current_custody_stage = ");
        builder.push_bind(stage.as_db_str());
    }
    if let Some(classification) = query.classification {
        builder.push(" AND classification = ");
        builder.push_bind(classification.as_db_str());
    }
    if let Some(q) = query.q.as_ref().map(|q| q.trim()).filter(|q| !q.is_empty()) {
        if q.chars().count() > 200 {
            return Err(KernelError::validation("evidence q is too long"));
        }
        let pattern = format!("%{q}%");
        builder.push(" AND (code ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR title ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR source_id ILIKE ");
        builder.push_bind(pattern);
        builder.push(")");
    }
    Ok(())
}

async fn issue_evidence_code_tx(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    now: Timestamp,
) -> Result<EvidenceCode, PgDocsError> {
    let issued: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO docs_evidence_code_counters (org_id, object_prefix, next_value, updated_at)
        VALUES ($1, 'EV', 2, $2)
        ON CONFLICT (org_id, object_prefix)
        DO UPDATE SET next_value = docs_evidence_code_counters.next_value + 1,
                      updated_at = EXCLUDED.updated_at
        RETURNING next_value - 1
        "#,
    )
    .bind(*org.as_uuid())
    .bind(now)
    .fetch_one(tx.as_mut())
    .await?;
    EvidenceCode::new(format!("EV-{issued:06}")).map_err(PgDocsError::from)
}

async fn insert_copy_tx(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    evidence_object_id: EvidenceObjectId,
    actor: UserId,
    input: RegisterEvidenceCopyInput,
    occurred_at: Timestamp,
) -> Result<EvidenceCopyView, PgDocsError> {
    input
        .copy_kind
        .validate(input.parent_copy_id, input.derivative_kind)?;
    let content_type = normalize_required_text(&input.content_type, 160, "content_type")?;
    if input.size_bytes < 0 {
        return Err(KernelError::validation("size_bytes must be non-negative").into());
    }
    if input.copy_kind == EvidenceCopyKind::Original {
        let original_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM docs_evidence_copies WHERE evidence_object_id = $1 AND copy_kind = 'ORIGINAL'",
        )
        .bind(*evidence_object_id.as_uuid())
        .fetch_one(tx.as_mut())
        .await?;
        if original_count > 0 {
            return Err(KernelError::conflict("EV object already has an original copy").into());
        }
    }
    if let Some(parent_copy_id) = input.parent_copy_id {
        let parent_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM docs_evidence_copies WHERE id = $1 AND evidence_object_id = $2)",
        )
        .bind(*parent_copy_id.as_uuid())
        .bind(*evidence_object_id.as_uuid())
        .fetch_one(tx.as_mut())
        .await?;
        if !parent_exists {
            return Err(KernelError::not_found("derivative parent copy was not found").into());
        }
    }
    // WORM state is never a command assertion. The migration-owned BEFORE INSERT
    // trigger derives it from the storage service's immutable replica attestation:
    // same tenant, media identity, object key, SHA-256, and verified timestamp.
    // Always insert PENDING here so a caller cannot manufacture verification while
    // bypassing the adapter's intent; only that trigger can promote the row.
    let copy_id = EvidenceCopyId::new();
    let worm_status = WormStorageStatus::Pending;
    let verified_at = None::<Timestamp>;
    sqlx::query(
        r#"
        INSERT INTO docs_evidence_copies (
            id, org_id, evidence_object_id, copy_kind, derivative_kind, parent_copy_id,
            storage_provider, storage_object_id, storage_key_ref, storage_version_id,
            source_evidence_media_id, digest_sha256, content_type, size_bytes, worm_status,
            created_by, created_at, verified_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                  $15, $16, $17, $18)
        "#,
    )
    .bind(*copy_id.as_uuid())
    .bind(*org.as_uuid())
    .bind(*evidence_object_id.as_uuid())
    .bind(input.copy_kind.as_db_str())
    .bind(input.derivative_kind.map(DerivativeKind::as_db_str))
    .bind(input.parent_copy_id.map(|id| *id.as_uuid()))
    .bind(&input.storage.provider)
    .bind(&input.storage.object_id)
    .bind(input.storage.key_ref.as_deref())
    .bind(input.storage.version_id.as_deref())
    .bind(input.source_evidence_media_id.map(|id| *id.as_uuid()))
    .bind(input.digest_sha256.as_str())
    .bind(&content_type)
    .bind(input.size_bytes)
    .bind(worm_status.as_db_str())
    .bind(*actor.as_uuid())
    .bind(occurred_at)
    .bind(verified_at)
    .execute(tx.as_mut())
    .await?;
    fetch_copy_tx(tx, copy_id)
        .await?
        .ok_or_else(|| KernelError::internal("created EV copy was not readable").into())
}

#[allow(clippy::too_many_arguments)]
async fn insert_tsa_proof_tx(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    evidence_object_id: EvidenceObjectId,
    copy_id: EvidenceCopyId,
    copy_digest: Sha256Digest,
    actor: UserId,
    input: TimestampAuthorityProofInput,
    occurred_at: Timestamp,
) -> Result<TimestampAuthorityProofView, PgDocsError> {
    let provider = normalize_required_text(&input.provider, 120, "provider")?;
    let hash_algorithm = input.hash_algorithm.trim().to_ascii_uppercase();
    if hash_algorithm != "SHA-256" {
        return Err(KernelError::validation("TSA hash_algorithm must be SHA-256").into());
    }
    if input.status.is_verified() {
        let Some(imprint) = input.message_imprint_sha256.as_ref() else {
            return Err(KernelError::validation(
                "verified TSA proof requires message_imprint_sha256",
            )
            .into());
        };
        if imprint != &copy_digest {
            return Err(KernelError::conflict("TSA imprint does not match EV copy digest").into());
        }
    }
    let proof_id = EvidenceTsaProofId::new();
    let token_storage = input.token_storage.clone();
    sqlx::query(
        r#"
        INSERT INTO docs_evidence_tsa_proofs (
            id, org_id, evidence_object_id, copy_id, status, provider, policy_oid,
            serial_number, hash_algorithm, message_imprint_sha256, generated_at,
            accuracy_millis, ordering, tsa_cert_fingerprint_sha256, token_digest_sha256,
            token_storage_provider, token_storage_object_id, token_storage_key_ref,
            token_storage_version_id, verified_at, failure_reason, created_by, created_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'SHA-256', $9, $10, $11, $12,
                  $13, $14, $15, $16, $17, $18, $19, $20, $21, $22)
        "#,
    )
    .bind(*proof_id.as_uuid())
    .bind(*org.as_uuid())
    .bind(*evidence_object_id.as_uuid())
    .bind(*copy_id.as_uuid())
    .bind(input.status.as_db_str())
    .bind(&provider)
    .bind(normalize_optional_text(input.policy_oid, 120, "policy_oid")?.as_deref())
    .bind(normalize_optional_text(input.serial_number, 200, "serial_number")?.as_deref())
    .bind(
        input
            .message_imprint_sha256
            .as_ref()
            .map(Sha256Digest::as_str),
    )
    .bind(input.generated_at)
    .bind(input.accuracy_millis)
    .bind(input.ordering)
    .bind(
        input
            .tsa_cert_fingerprint_sha256
            .as_ref()
            .map(Sha256Digest::as_str),
    )
    .bind(input.token_digest_sha256.as_ref().map(Sha256Digest::as_str))
    .bind(
        token_storage
            .as_ref()
            .map(|storage| storage.provider.as_str()),
    )
    .bind(
        token_storage
            .as_ref()
            .map(|storage| storage.object_id.as_str()),
    )
    .bind(
        token_storage
            .as_ref()
            .and_then(|storage| storage.key_ref.as_deref()),
    )
    .bind(
        token_storage
            .as_ref()
            .and_then(|storage| storage.version_id.as_deref()),
    )
    .bind(
        input
            .verified_at
            .or_else(|| input.status.is_verified().then_some(occurred_at)),
    )
    .bind(normalize_optional_text(input.failure_reason, 2_000, "failure_reason")?.as_deref())
    .bind(*actor.as_uuid())
    .bind(occurred_at)
    .execute(tx.as_mut())
    .await?;
    fetch_tsa_proof_tx(tx, proof_id)
        .await?
        .ok_or_else(|| KernelError::internal("created EV TSA proof was not readable").into())
}

#[allow(clippy::too_many_arguments)]
async fn insert_custody_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    evidence_object_id: EvidenceObjectId,
    stage: CustodyStage,
    actor: UserId,
    from_custodian: Option<serde_json::Value>,
    to_custodian: Option<serde_json::Value>,
    location_label: Option<String>,
    reason: String,
    source_ref: Option<EvidenceSourceRef>,
    occurred_at: Timestamp,
    update_object_stage: bool,
) -> Result<CustodyEventView, PgDocsError> {
    let reason = normalize_required_text(&reason, 2_000, "custody reason")?;
    let location_label = normalize_optional_text(location_label, 200, "location_label")?;
    let previous_event_id: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM docs_evidence_custody_events WHERE evidence_object_id = $1 ORDER BY occurred_at DESC, id DESC LIMIT 1",
    )
    .bind(*evidence_object_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?;
    let event_id = EvidenceCustodyEventId::new();
    let digest = custody_event_digest(
        evidence_object_id,
        previous_event_id.map(EvidenceCustodyEventId::from_uuid),
        stage,
        actor,
        &reason,
        occurred_at,
    )?;
    let source_ref_json = source_ref
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|err| {
            PgDocsError::Domain(KernelError::internal(format!(
                "source_ref serialization failed: {err}"
            )))
        })?;
    sqlx::query(
        r#"
        INSERT INTO docs_evidence_custody_events (
            id, org_id, evidence_object_id, stage, actor_user_id, from_custodian,
            to_custodian, location_label, reason, source_ref, previous_event_id,
            event_digest_sha256, occurred_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        "#,
    )
    .bind(*event_id.as_uuid())
    .bind(*org.as_uuid())
    .bind(*evidence_object_id.as_uuid())
    .bind(stage.as_db_str())
    .bind(*actor.as_uuid())
    .bind(from_custodian.as_ref())
    .bind(to_custodian.as_ref())
    .bind(location_label.as_deref())
    .bind(&reason)
    .bind(source_ref_json.as_ref())
    .bind(previous_event_id)
    .bind(digest.as_str())
    .bind(occurred_at)
    .execute(tx.as_mut())
    .await?;
    if update_object_stage {
        sqlx::query(
            "UPDATE docs_evidence_objects SET current_custody_stage = $2, updated_by = $3, updated_at = $4 WHERE id = $1",
        )
        .bind(*evidence_object_id.as_uuid())
        .bind(stage.as_db_str())
        .bind(*actor.as_uuid())
        .bind(occurred_at)
        .execute(tx.as_mut())
        .await?;
    }
    fetch_custody_event_tx(tx, event_id)
        .await?
        .ok_or_else(|| KernelError::internal("created EV custody event was not readable").into())
}

async fn recompute_admissibility_tx(
    tx: &mut Transaction<'_, Postgres>,
    evidence_object_id: EvidenceObjectId,
    actor: UserId,
    occurred_at: Timestamp,
) -> Result<(), PgDocsError> {
    let object = fetch_object_tx(tx, evidence_object_id)
        .await?
        .ok_or_else(|| KernelError::not_found("EV object was not found"))?;
    let original_row = sqlx::query(
        "SELECT id, digest_sha256, worm_status FROM docs_evidence_copies WHERE evidence_object_id = $1 AND copy_kind = 'ORIGINAL' ORDER BY created_at ASC LIMIT 1",
    )
    .bind(*evidence_object_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?;
    let mut original_present = false;
    let mut original_worm_verified = false;
    let mut original_copy_id = None;
    let mut original_digest = None;
    if let Some(row) = original_row {
        original_present = true;
        let status: String = row.try_get("worm_status")?;
        original_worm_verified = status == "VERIFIED";
        original_copy_id = Some(EvidenceCopyId::from_uuid(row.try_get("id")?));
        let digest: String = row.try_get("digest_sha256")?;
        original_digest = Some(Sha256Digest::new(digest)?);
    }
    let latest_tsa = if let Some(copy_id) = original_copy_id {
        sqlx::query(
            "SELECT status, message_imprint_sha256 FROM docs_evidence_tsa_proofs WHERE copy_id = $1 ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(*copy_id.as_uuid())
        .fetch_optional(tx.as_mut())
        .await?
    } else {
        None
    };
    let mut tsa_status = None;
    let mut tsa_imprint_matches = false;
    if let Some(row) = latest_tsa {
        let status: String = row.try_get("status")?;
        tsa_status = Some(TsaProofStatus::parse(&status)?);
        let imprint: Option<String> = row.try_get("message_imprint_sha256")?;
        tsa_imprint_matches = match (imprint, original_digest) {
            (Some(imprint), Some(original_digest)) => {
                Sha256Digest::new(imprint)? == original_digest
            }
            _ => false,
        };
    }
    let custody_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM docs_evidence_custody_events WHERE evidence_object_id = $1",
    )
    .bind(*evidence_object_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    let inputs = AdmissibilityInputs {
        original_copy_present: original_present,
        original_worm_verified,
        tsa_status,
        tsa_imprint_matches_original: tsa_imprint_matches,
        custody_chain_intact: custody_count > 0,
        source_resolvable: true,
        disposed: object.disposed_at.is_some(),
        active_legal_hold: object.legal_hold_state == LegalHoldState::Active,
    };
    let summary = evaluate_admissibility(inputs);
    let reasons = summary
        .reasons
        .iter()
        .map(|reason| reason.as_db_str())
        .collect::<Vec<_>>();
    let reasons_json = serde_json::json!(reasons);
    let inputs_json = serde_json::to_value(summary.inputs).map_err(|err| {
        PgDocsError::Domain(KernelError::internal(format!(
            "admissibility serialization failed: {err}"
        )))
    })?;
    sqlx::query(
        r#"
        UPDATE docs_evidence_objects
        SET admissibility_status = $2, admissibility_reasons = $3,
            admissibility_inputs = $4, updated_by = $5, updated_at = $6
        WHERE id = $1
        "#,
    )
    .bind(*evidence_object_id.as_uuid())
    .bind(summary.status.as_db_str())
    .bind(reasons_json)
    .bind(inputs_json)
    .bind(*actor.as_uuid())
    .bind(occurred_at)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn fetch_detail_tx(
    tx: &mut Transaction<'_, Postgres>,
    evidence_object_id: EvidenceObjectId,
) -> Result<Option<EvidenceObjectDetail>, PgDocsError> {
    let Some(object) = fetch_object_tx(tx, evidence_object_id).await? else {
        return Ok(None);
    };
    let copies = fetch_copies_tx(tx, evidence_object_id).await?;
    let tsa_proofs = fetch_tsa_proofs_tx(tx, evidence_object_id).await?;
    let custody_history = fetch_custody_events_tx(tx, evidence_object_id).await?;
    let legal_holds = fetch_legal_holds_tx(tx, evidence_object_id).await?;
    let exports = fetch_exports_tx(tx, evidence_object_id).await?;
    Ok(Some(EvidenceObjectDetail {
        object,
        copies,
        tsa_proofs,
        custody_history,
        legal_holds,
        exports,
    }))
}

async fn fetch_object_tx(
    tx: &mut Transaction<'_, Postgres>,
    evidence_object_id: EvidenceObjectId,
) -> Result<Option<EvidenceObjectView>, PgDocsError> {
    fetch_object_inner_tx(tx, evidence_object_id, false).await
}

async fn fetch_object_for_update_tx(
    tx: &mut Transaction<'_, Postgres>,
    evidence_object_id: EvidenceObjectId,
) -> Result<Option<EvidenceObjectView>, PgDocsError> {
    fetch_object_inner_tx(tx, evidence_object_id, true).await
}

async fn fetch_object_inner_tx(
    tx: &mut Transaction<'_, Postgres>,
    evidence_object_id: EvidenceObjectId,
    for_update: bool,
) -> Result<Option<EvidenceObjectView>, PgDocsError> {
    let mut sql = format!("SELECT {OBJECT_COLUMNS} FROM docs_evidence_objects WHERE id = $1");
    if for_update {
        sql.push_str(" FOR UPDATE");
    }
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(*evidence_object_id.as_uuid())
        .fetch_optional(tx.as_mut())
        .await?;
    row.as_ref().map(object_from_row).transpose()
}

async fn fetch_copy_tx(
    tx: &mut Transaction<'_, Postgres>,
    copy_id: EvidenceCopyId,
) -> Result<Option<EvidenceCopyView>, PgDocsError> {
    let row = sqlx::query(
        r#"
        SELECT id, evidence_object_id, copy_kind, evidentiary_status, derivative_kind, parent_copy_id,
               storage_provider, storage_object_id, storage_key_ref, storage_version_id,
               source_evidence_media_id, digest_sha256, content_type, size_bytes,
               worm_status, verified_at, created_by, created_at
        FROM docs_evidence_copies WHERE id = $1
        "#,
    )
    .bind(*copy_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?;
    row.as_ref().map(copy_from_row).transpose()
}

async fn fetch_copies_tx(
    tx: &mut Transaction<'_, Postgres>,
    evidence_object_id: EvidenceObjectId,
) -> Result<Vec<EvidenceCopyView>, PgDocsError> {
    let rows = sqlx::query(
        r#"
        SELECT id, evidence_object_id, copy_kind, evidentiary_status, derivative_kind, parent_copy_id,
               storage_provider, storage_object_id, storage_key_ref, storage_version_id,
               source_evidence_media_id, digest_sha256, content_type, size_bytes,
               worm_status, verified_at, created_by, created_at
        FROM docs_evidence_copies WHERE evidence_object_id = $1 ORDER BY created_at ASC, id ASC
        "#,
    )
    .bind(*evidence_object_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await?;
    rows.iter().map(copy_from_row).collect()
}

async fn fetch_tsa_proof_tx(
    tx: &mut Transaction<'_, Postgres>,
    proof_id: EvidenceTsaProofId,
) -> Result<Option<TimestampAuthorityProofView>, PgDocsError> {
    let row = sqlx::query(TSA_SELECT_SQL)
        .bind(*proof_id.as_uuid())
        .fetch_optional(tx.as_mut())
        .await?;
    row.as_ref().map(tsa_from_row).transpose()
}

async fn fetch_tsa_proofs_tx(
    tx: &mut Transaction<'_, Postgres>,
    evidence_object_id: EvidenceObjectId,
) -> Result<Vec<TimestampAuthorityProofView>, PgDocsError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{TSA_SELECT_SQL_PREFIX} WHERE evidence_object_id = $1 ORDER BY created_at DESC, id DESC"
    )))
    .bind(*evidence_object_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await?;
    rows.iter().map(tsa_from_row).collect()
}

const TSA_SELECT_SQL_PREFIX: &str = "SELECT id, copy_id, status, provider, policy_oid, serial_number, \
    hash_algorithm, message_imprint_sha256, generated_at, accuracy_millis, ordering, \
    tsa_cert_fingerprint_sha256, token_digest_sha256, token_storage_provider, \
    token_storage_object_id, token_storage_key_ref, token_storage_version_id, \
    verified_at, failure_reason, created_by, created_at FROM docs_evidence_tsa_proofs";
const TSA_SELECT_SQL: &str = "SELECT id, copy_id, status, provider, policy_oid, serial_number, \
    hash_algorithm, message_imprint_sha256, generated_at, accuracy_millis, ordering, \
    tsa_cert_fingerprint_sha256, token_digest_sha256, token_storage_provider, \
    token_storage_object_id, token_storage_key_ref, token_storage_version_id, \
    verified_at, failure_reason, created_by, created_at FROM docs_evidence_tsa_proofs WHERE id = $1";

async fn fetch_custody_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    event_id: EvidenceCustodyEventId,
) -> Result<Option<CustodyEventView>, PgDocsError> {
    let row = sqlx::query(CUSTODY_SELECT_SQL)
        .bind(*event_id.as_uuid())
        .fetch_optional(tx.as_mut())
        .await?;
    row.as_ref().map(custody_from_row).transpose()
}

async fn fetch_custody_events_tx(
    tx: &mut Transaction<'_, Postgres>,
    evidence_object_id: EvidenceObjectId,
) -> Result<Vec<CustodyEventView>, PgDocsError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{CUSTODY_SELECT_SQL_PREFIX} WHERE evidence_object_id = $1 ORDER BY occurred_at DESC, id DESC"
    )))
    .bind(*evidence_object_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await?;
    rows.iter().map(custody_from_row).collect()
}

const CUSTODY_SELECT_SQL_PREFIX: &str = "SELECT id, evidence_object_id, stage, actor_user_id, \
    from_custodian, to_custodian, location_label, reason, source_ref, audit_event_id, \
    previous_event_id, event_digest_sha256, occurred_at, created_at FROM docs_evidence_custody_events";
const CUSTODY_SELECT_SQL: &str = "SELECT id, evidence_object_id, stage, actor_user_id, \
    from_custodian, to_custodian, location_label, reason, source_ref, audit_event_id, \
    previous_event_id, event_digest_sha256, occurred_at, created_at FROM docs_evidence_custody_events WHERE id = $1";

async fn fetch_legal_hold_tx(
    tx: &mut Transaction<'_, Postgres>,
    hold_id: EvidenceLegalHoldId,
) -> Result<Option<LegalHoldRecordView>, PgDocsError> {
    fetch_legal_hold_inner_tx(tx, hold_id, false).await
}

async fn fetch_legal_hold_for_update_tx(
    tx: &mut Transaction<'_, Postgres>,
    hold_id: EvidenceLegalHoldId,
) -> Result<Option<LegalHoldRecordView>, PgDocsError> {
    fetch_legal_hold_inner_tx(tx, hold_id, true).await
}

async fn fetch_legal_hold_inner_tx(
    tx: &mut Transaction<'_, Postgres>,
    hold_id: EvidenceLegalHoldId,
    for_update: bool,
) -> Result<Option<LegalHoldRecordView>, PgDocsError> {
    let mut sql = LEGAL_HOLD_SELECT_SQL.to_owned();
    if for_update {
        sql.push_str(" FOR UPDATE");
    }
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(*hold_id.as_uuid())
        .fetch_optional(tx.as_mut())
        .await?;
    row.as_ref().map(legal_hold_from_row).transpose()
}

async fn fetch_legal_holds_tx(
    tx: &mut Transaction<'_, Postgres>,
    evidence_object_id: EvidenceObjectId,
) -> Result<Vec<LegalHoldRecordView>, PgDocsError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{LEGAL_HOLD_SELECT_SQL_PREFIX} WHERE evidence_object_id = $1 ORDER BY applied_at DESC, id DESC"
    )))
    .bind(*evidence_object_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await?;
    rows.iter().map(legal_hold_from_row).collect()
}

const LEGAL_HOLD_SELECT_SQL_PREFIX: &str = "SELECT id, evidence_object_id, status, case_ref, \
    basis, reason, applied_by, applied_at, released_by, released_at, release_reason, audit_event_id \
    FROM docs_evidence_legal_holds";
const LEGAL_HOLD_SELECT_SQL: &str = "SELECT id, evidence_object_id, status, case_ref, \
    basis, reason, applied_by, applied_at, released_by, released_at, release_reason, audit_event_id \
    FROM docs_evidence_legal_holds WHERE id = $1";

async fn fetch_exports_tx(
    tx: &mut Transaction<'_, Postgres>,
    evidence_object_id: EvidenceObjectId,
) -> Result<Vec<EvidenceExportView>, PgDocsError> {
    let rows = sqlx::query(
        r#"
        SELECT id, evidence_object_id, manifest_digest_sha256, signature_algorithm,
               signature_ref, export_reason, exported_by, exported_at
        FROM docs_evidence_exports WHERE evidence_object_id = $1 ORDER BY exported_at DESC, id DESC
        "#,
    )
    .bind(*evidence_object_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await?;
    rows.iter().map(export_from_row).collect()
}

fn object_from_row(row: &sqlx::postgres::PgRow) -> Result<EvidenceObjectView, PgDocsError> {
    let source_type: String = row.try_get("source_type")?;
    let classification: String = row.try_get("classification")?;
    let stage: String = row.try_get("current_custody_stage")?;
    let hold: String = row.try_get("legal_hold_state")?;
    let admissibility: String = row.try_get("admissibility_status")?;
    let reasons_json: serde_json::Value = row.try_get("admissibility_reasons")?;
    let reasons = parse_admissibility_reasons(&reasons_json)?;
    Ok(EvidenceObjectView {
        id: EvidenceObjectId::from_uuid(row.try_get("id")?),
        code: EvidenceCode::new(row.try_get::<String, _>("code")?)?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        source: EvidenceSourceRef::new(
            EvidenceSourceType::parse(&source_type)?,
            row.try_get::<String, _>("source_id")?,
            row.try_get("source_code")?,
        )?,
        classification: EvidenceClassification::parse(&classification)?,
        record_owner_user_id: row
            .try_get::<Option<uuid::Uuid>, _>("record_owner_user_id")?
            .map(UserId::from_uuid),
        current_custody_stage: CustodyStage::parse(&stage)?,
        legal_hold_state: LegalHoldState::parse(&hold)?,
        admissibility_status: AdmissibilityStatus::parse(&admissibility)?,
        admissibility_reasons: reasons,
        admissibility_inputs: row.try_get("admissibility_inputs")?,
        created_by: UserId::from_uuid(row.try_get("created_by")?),
        updated_by: UserId::from_uuid(row.try_get("updated_by")?),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        disposed_at: row.try_get("disposed_at")?,
    })
}

fn copy_from_row(row: &sqlx::postgres::PgRow) -> Result<EvidenceCopyView, PgDocsError> {
    let copy_kind = EvidenceCopyKind::parse(&row.try_get::<String, _>("copy_kind")?)?;
    let worm_status = WormStorageStatus::parse(&row.try_get::<String, _>("worm_status")?)?;
    let evidentiary_status =
        EvidenceCopyEvidentiaryStatus::parse(&row.try_get::<String, _>("evidentiary_status")?)?;
    if evidentiary_status != EvidenceCopyEvidentiaryStatus::expected(copy_kind, worm_status) {
        return Err(KernelError::internal(
            "EV copy evidentiary status is inconsistent with kind/WORM state",
        )
        .into());
    }
    let derivative_kind: Option<String> = row.try_get("derivative_kind")?;
    Ok(EvidenceCopyView {
        id: EvidenceCopyId::from_uuid(row.try_get("id")?),
        evidence_object_id: EvidenceObjectId::from_uuid(row.try_get("evidence_object_id")?),
        copy_kind,
        evidentiary_status,
        derivative_kind: derivative_kind
            .as_deref()
            .map(DerivativeKind::parse)
            .transpose()?,
        parent_copy_id: row
            .try_get::<Option<uuid::Uuid>, _>("parent_copy_id")?
            .map(EvidenceCopyId::from_uuid),
        storage: EvidenceStorageRef::new(
            row.try_get::<String, _>("storage_provider")?,
            row.try_get::<String, _>("storage_object_id")?,
            row.try_get("storage_key_ref")?,
            row.try_get("storage_version_id")?,
        )?,
        source_evidence_media_id: row
            .try_get::<Option<uuid::Uuid>, _>("source_evidence_media_id")?
            .map(EvidenceId::from_uuid),
        digest_sha256: Sha256Digest::new(row.try_get::<String, _>("digest_sha256")?)?,
        content_type: row.try_get("content_type")?,
        size_bytes: row.try_get("size_bytes")?,
        worm_status,
        verified_at: row.try_get("verified_at")?,
        created_by: UserId::from_uuid(row.try_get("created_by")?),
        created_at: row.try_get("created_at")?,
    })
}

fn tsa_from_row(row: &sqlx::postgres::PgRow) -> Result<TimestampAuthorityProofView, PgDocsError> {
    let status: String = row.try_get("status")?;
    let token_provider: Option<String> = row.try_get("token_storage_provider")?;
    let token_object: Option<String> = row.try_get("token_storage_object_id")?;
    let token_storage = match (token_provider, token_object) {
        (Some(provider), Some(object_id)) => Some(EvidenceStorageRef::new(
            provider,
            object_id,
            row.try_get("token_storage_key_ref")?,
            row.try_get("token_storage_version_id")?,
        )?),
        _ => None,
    };
    Ok(TimestampAuthorityProofView {
        id: EvidenceTsaProofId::from_uuid(row.try_get("id")?),
        copy_id: EvidenceCopyId::from_uuid(row.try_get("copy_id")?),
        status: TsaProofStatus::parse(&status)?,
        provider: row.try_get("provider")?,
        policy_oid: row.try_get("policy_oid")?,
        serial_number: row.try_get("serial_number")?,
        hash_algorithm: row.try_get("hash_algorithm")?,
        message_imprint_sha256: row
            .try_get::<Option<String>, _>("message_imprint_sha256")?
            .map(Sha256Digest::new)
            .transpose()?,
        generated_at: row.try_get("generated_at")?,
        accuracy_millis: row.try_get("accuracy_millis")?,
        ordering: row.try_get("ordering")?,
        tsa_cert_fingerprint_sha256: row
            .try_get::<Option<String>, _>("tsa_cert_fingerprint_sha256")?
            .map(Sha256Digest::new)
            .transpose()?,
        token_digest_sha256: row
            .try_get::<Option<String>, _>("token_digest_sha256")?
            .map(Sha256Digest::new)
            .transpose()?,
        token_storage,
        verified_at: row.try_get("verified_at")?,
        failure_reason: row.try_get("failure_reason")?,
        created_by: UserId::from_uuid(row.try_get("created_by")?),
        created_at: row.try_get("created_at")?,
    })
}

fn custody_from_row(row: &sqlx::postgres::PgRow) -> Result<CustodyEventView, PgDocsError> {
    let stage: String = row.try_get("stage")?;
    let source_ref_json: Option<serde_json::Value> = row.try_get("source_ref")?;
    let source_ref = source_ref_json
        .map(serde_json::from_value)
        .transpose()
        .map_err(|err| {
            KernelError::internal(format!("source_ref deserialization failed: {err}"))
        })?;
    Ok(CustodyEventView {
        id: EvidenceCustodyEventId::from_uuid(row.try_get("id")?),
        evidence_object_id: EvidenceObjectId::from_uuid(row.try_get("evidence_object_id")?),
        stage: CustodyStage::parse(&stage)?,
        actor_user_id: UserId::from_uuid(row.try_get("actor_user_id")?),
        from_custodian: row.try_get("from_custodian")?,
        to_custodian: row.try_get("to_custodian")?,
        location_label: row.try_get("location_label")?,
        reason: row.try_get("reason")?,
        source_ref,
        audit_event_id: row
            .try_get::<Option<uuid::Uuid>, _>("audit_event_id")?
            .map(AuditEventId::from_uuid),
        previous_event_id: row
            .try_get::<Option<uuid::Uuid>, _>("previous_event_id")?
            .map(EvidenceCustodyEventId::from_uuid),
        event_digest_sha256: Sha256Digest::new(row.try_get::<String, _>("event_digest_sha256")?)?,
        occurred_at: row.try_get("occurred_at")?,
        created_at: row.try_get("created_at")?,
    })
}

fn legal_hold_from_row(row: &sqlx::postgres::PgRow) -> Result<LegalHoldRecordView, PgDocsError> {
    let status: String = row.try_get("status")?;
    Ok(LegalHoldRecordView {
        id: EvidenceLegalHoldId::from_uuid(row.try_get("id")?),
        evidence_object_id: EvidenceObjectId::from_uuid(row.try_get("evidence_object_id")?),
        status: LegalHoldStatus::parse(&status)?,
        case_ref: row.try_get("case_ref")?,
        basis: row.try_get("basis")?,
        reason: row.try_get("reason")?,
        applied_by: UserId::from_uuid(row.try_get("applied_by")?),
        applied_at: row.try_get("applied_at")?,
        released_by: row
            .try_get::<Option<uuid::Uuid>, _>("released_by")?
            .map(UserId::from_uuid),
        released_at: row.try_get("released_at")?,
        release_reason: row.try_get("release_reason")?,
        audit_event_id: row
            .try_get::<Option<uuid::Uuid>, _>("audit_event_id")?
            .map(AuditEventId::from_uuid),
    })
}

fn export_from_row(row: &sqlx::postgres::PgRow) -> Result<EvidenceExportView, PgDocsError> {
    Ok(EvidenceExportView {
        id: EvidenceExportId::from_uuid(row.try_get("id")?),
        evidence_object_id: EvidenceObjectId::from_uuid(row.try_get("evidence_object_id")?),
        manifest_digest_sha256: Sha256Digest::new(
            row.try_get::<String, _>("manifest_digest_sha256")?,
        )?,
        signature_algorithm: row.try_get("signature_algorithm")?,
        signature_ref: row.try_get("signature_ref")?,
        export_reason: row.try_get("export_reason")?,
        exported_by: UserId::from_uuid(row.try_get("exported_by")?),
        exported_at: row.try_get("exported_at")?,
    })
}

fn parse_admissibility_reasons(
    value: &serde_json::Value,
) -> Result<Vec<AdmissibilityReason>, PgDocsError> {
    let array = value
        .as_array()
        .ok_or_else(|| KernelError::internal("admissibility_reasons must be an array"))?;
    array
        .iter()
        .map(|value| {
            let text = value
                .as_str()
                .ok_or_else(|| KernelError::internal("admissibility reason must be a string"))?;
            AdmissibilityReason::parse(text).map_err(PgDocsError::from)
        })
        .collect()
}

fn custody_event_digest(
    evidence_object_id: EvidenceObjectId,
    previous_event_id: Option<EvidenceCustodyEventId>,
    stage: CustodyStage,
    actor: UserId,
    reason: &str,
    occurred_at: Timestamp,
) -> Result<Sha256Digest, PgDocsError> {
    let previous = previous_event_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "ROOT".to_owned());
    let payload = format!(
        "{}|{}|{}|{}|{}|{}",
        evidence_object_id,
        previous,
        stage.as_db_str(),
        actor,
        reason,
        occurred_at.unix_timestamp_nanos()
    );
    Sha256Digest::new(sha256_hex(payload.as_bytes())).map_err(PgDocsError::from)
}

fn normalize_required_text(
    value: &str,
    max_chars: usize,
    field: &str,
) -> Result<String, KernelError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(KernelError::validation(format!("{field} is required")));
    }
    if trimmed.chars().count() > max_chars {
        return Err(KernelError::validation(format!("{field} is too long")));
    }
    Ok(trimmed.to_owned())
}

fn normalize_optional_text(
    value: Option<String>,
    max_chars: usize,
    field: &str,
) -> Result<Option<String>, KernelError> {
    value
        .map(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else if trimmed.chars().count() > max_chars {
                Err(KernelError::validation(format!("{field} is too long")))
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        })
        .transpose()
        .map(Option::flatten)
}

fn object_snapshot(object: &EvidenceObjectView) -> serde_json::Value {
    serde_json::json!({
        "id": object.id.to_string(),
        "code": object.code.as_str(),
        "title": object.title,
        "source": object.source,
        "classification": object.classification.as_db_str(),
        "custody_stage": object.current_custody_stage.as_db_str(),
        "legal_hold_state": object.legal_hold_state.as_db_str(),
        "admissibility_status": object.admissibility_status.as_db_str(),
        "admissibility_reasons": object.admissibility_reasons.iter().map(|r| r.as_db_str()).collect::<Vec<_>>(),
    })
}

fn copy_snapshot(copy: &EvidenceCopyView) -> serde_json::Value {
    serde_json::json!({
        "id": copy.id.to_string(),
        "evidence_object_id": copy.evidence_object_id.to_string(),
        "copy_kind": copy.copy_kind.as_db_str(),
        "derivative_kind": copy.derivative_kind.map(DerivativeKind::as_db_str),
        "parent_copy_id": copy.parent_copy_id.map(|id| id.to_string()),
        "digest_sha256": copy.digest_sha256.as_str(),
        "worm_status": copy.worm_status.as_db_str(),
        "source_evidence_media_id": copy.source_evidence_media_id.map(|id| id.to_string()),
    })
}

fn tsa_snapshot(proof: &TimestampAuthorityProofView) -> serde_json::Value {
    serde_json::json!({
        "id": proof.id.to_string(),
        "copy_id": proof.copy_id.to_string(),
        "status": proof.status.as_db_str(),
        "provider": proof.provider,
        "message_imprint_sha256": proof.message_imprint_sha256.as_ref().map(Sha256Digest::as_str),
        "token_digest_sha256": proof.token_digest_sha256.as_ref().map(Sha256Digest::as_str),
    })
}

fn custody_snapshot(event: &CustodyEventView) -> serde_json::Value {
    serde_json::json!({
        "id": event.id.to_string(),
        "evidence_object_id": event.evidence_object_id.to_string(),
        "stage": event.stage.as_db_str(),
        "actor_user_id": event.actor_user_id.to_string(),
        "event_digest_sha256": event.event_digest_sha256.as_str(),
    })
}

fn legal_hold_snapshot(hold: &LegalHoldRecordView) -> serde_json::Value {
    serde_json::json!({
        "id": hold.id.to_string(),
        "evidence_object_id": hold.evidence_object_id.to_string(),
        "status": hold.status.as_db_str(),
        "case_ref": hold.case_ref,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custody_event_digest_is_stable_sha256_hex() {
        let digest = custody_event_digest(
            EvidenceObjectId::from_uuid(uuid::Uuid::from_u128(1)),
            None,
            CustodyStage::Registered,
            UserId::from_uuid(uuid::Uuid::from_u128(2)),
            "registered",
            time::macros::datetime!(2026-07-09 00:00:00 UTC),
        )
        .unwrap();
        assert_eq!(digest.as_str().len(), 64);
        assert!(digest.as_str().chars().all(|ch| ch.is_ascii_hexdigit()));
    }
}
