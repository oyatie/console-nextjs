//! Tenant-armed persistence for the recruiting pipeline.
//!
//! All mutations use `with_audits`: row changes, the append-only stage-event
//! history, and the platform audit stream commit together. The hire handshake
//! is intentionally NOT a method here — the app-level handler owns that
//! transaction so the HR `create_employee_core` and the recruiting linkage
//! commit atomically; this crate only exposes the in-transaction pieces
//! ([`hire_context`] / [`apply_hire`]).
use console_kernel_core::{AuditAction, AuditEvent, KernelError, OrgId, TraceContext, UserId};
use console_platform_db::{DbError, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use console_recruiting_application::{ApplicantIntake, OfferTerms, PostingDraft, Rejection};
use console_recruiting_domain::{
    AmountPeriod, ApplicantStage, AssessmentScore, EmploymentType, OfferStatus,
    PREFLIGHT_EXPOSURE_ATTESTED, PREFLIGHT_NO_DUPLICATE_OPEN, PREFLIGHT_QUOTA_DEFINED,
    PREFLIGHT_ROLE_DEFINED, PostingStatus,
};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use time::{Date, OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PgRecruitingError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Domain(#[from] KernelError),
    /// Publish gate unmet: the full check vector rides back to the caller.
    #[error("publish preflight failed")]
    PreflightFailed(Vec<PreflightCheck>),
    /// Offer extension without a recorded interview assessment (fail-closed).
    #[error("assessment must be recorded before an offer")]
    AssessmentRequired,
    /// Idempotent hire replay: the applicant already links an employee.
    #[error("applicant is already hired")]
    AlreadyHired { employee_id: Uuid },
}

impl From<sqlx::Error> for PgRecruitingError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PreflightCheck {
    pub key: &'static str,
    pub ok: bool,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct PgRecruitingStore {
    pool: PgPool,
}

impl PgRecruitingStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn create_posting(
        &self,
        actor: UserId,
        draft: PostingDraft,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let id = Uuid::new_v4();
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let posting_no = next_code(tx, org, CodeSeries::Posting).await?;
                sqlx::query(
                    "INSERT INTO recruit_postings (id, org_id, posting_no, role_title, company, \
                     worksite, employment_type, scope, headcount, deadline, requirements, \
                     position_ref, created_by, created_at, updated_at) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$14)",
                )
                .bind(id)
                .bind(*org.as_uuid())
                .bind(&posting_no)
                .bind(&draft.role_title)
                .bind(&draft.company)
                .bind(&draft.worksite)
                .bind(draft.employment_type.as_db())
                .bind(draft.scope.as_db())
                .bind(draft.headcount)
                .bind(draft.deadline)
                .bind(json!(draft.requirements))
                .bind(&draft.position_ref)
                .bind(*actor.as_uuid())
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                let posting = load_posting(tx, id).await?;
                Ok((
                    posting,
                    vec![audit(
                        org,
                        actor,
                        "recruiting.posting.create",
                        "recruit_posting",
                        id,
                        now,
                        Some(json!({
                            "posting_no": posting_no,
                            "scope": draft.scope.as_db(),
                            "employment_type": draft.employment_type.as_db(),
                            "headcount": draft.headcount,
                        })),
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn list_postings(
        &self,
        status: Option<PostingStatus>,
        scope: Option<console_recruiting_domain::PostingScope>,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn(&self.pool, org, |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    "SELECT p.id, p.posting_no, p.role_title, p.company, p.worksite, \
                     p.employment_type, p.scope, p.headcount, p.hired_count, p.deadline, \
                     p.requirements, p.position_ref, p.status, p.published_at, p.closed_at, \
                     p.created_at, p.updated_at, \
                     count(a.id) FILTER (WHERE a.rejected_at IS NULL AND a.stage = 'APPLIED') AS stage_applied, \
                     count(a.id) FILTER (WHERE a.rejected_at IS NULL AND a.stage = 'SCREENING') AS stage_screening, \
                     count(a.id) FILTER (WHERE a.rejected_at IS NULL AND a.stage = 'INTERVIEW') AS stage_interview, \
                     count(a.id) FILTER (WHERE a.rejected_at IS NULL AND a.stage = 'OFFER') AS stage_offer \
                     FROM recruit_postings p \
                     LEFT JOIN recruit_applicants a ON a.posting_id = p.id AND a.org_id = p.org_id \
                     WHERE ($1::TEXT IS NULL OR p.status = $1) AND ($2::TEXT IS NULL OR p.scope = $2) \
                     GROUP BY p.id ORDER BY p.created_at DESC, p.posting_no DESC",
                )
                .bind(status.map(PostingStatus::as_db))
                .bind(scope.map(console_recruiting_domain::PostingScope::as_db))
                .fetch_all(tx.as_mut())
                .await?;
                let mut items = Vec::with_capacity(rows.len());
                for row in rows {
                    let mut posting = posting_json(&row)?;
                    posting["stage_counts"] = json!({
                        "applied": row.try_get::<i64, _>("stage_applied")?,
                        "screening": row.try_get::<i64, _>("stage_screening")?,
                        "interview": row.try_get::<i64, _>("stage_interview")?,
                        "offer": row.try_get::<i64, _>("stage_offer")?,
                    });
                    items.push(posting);
                }
                Ok(json!({ "items": items }))
            })
        })
        .await
    }

    /// Posting detail carries applicant SUMMARY rows only. The PII surfaces
    /// (profile lines, source document, reject note, assessment signature) are
    /// served exclusively by [`Self::applicant_detail`], which records the
    /// audited `recruiting.applicant.view` event — returning them here would
    /// bypass that audit. `assessed` is the existence flag the pipeline row
    /// needs to route its next action; the score itself stays behind the
    /// audited read.
    pub async fn get_posting(&self, posting_id: Uuid) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn(&self.pool, org, |tx| {
            Box::pin(async move {
                let posting = load_posting(tx, posting_id).await?;
                let rows = sqlx::query(
                    "SELECT id, posting_id, applicant_no, name, stage, hold, doc_requested, \
                     rejected_at, reject_reason, assessment_score IS NOT NULL AS assessed, \
                     hired_employee_id, created_at, updated_at \
                     FROM recruit_applicants WHERE posting_id = $1 \
                     ORDER BY created_at ASC, applicant_no ASC",
                )
                .bind(posting_id)
                .fetch_all(tx.as_mut())
                .await?;
                let applicants = rows
                    .iter()
                    .map(applicant_summary_json)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(json!({ "posting": posting, "applicants": applicants }))
            })
        })
        .await
    }

    pub async fn update_posting(
        &self,
        actor: UserId,
        posting_id: Uuid,
        draft: PostingDraft,
        expected_updated_at: OffsetDateTime,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let (status, updated_at) = lock_posting(tx, posting_id).await?;
                if status != PostingStatus::Draft {
                    return Err(KernelError::conflict(
                        "only a draft posting can be edited in place",
                    )
                    .into());
                }
                check_cas(updated_at, expected_updated_at, "posting")?;
                sqlx::query(
                    "UPDATE recruit_postings SET role_title=$2, company=$3, worksite=$4, \
                     employment_type=$5, scope=$6, headcount=$7, deadline=$8, requirements=$9, \
                     position_ref=$10, updated_at=$11 WHERE id=$1",
                )
                .bind(posting_id)
                .bind(&draft.role_title)
                .bind(&draft.company)
                .bind(&draft.worksite)
                .bind(draft.employment_type.as_db())
                .bind(draft.scope.as_db())
                .bind(draft.headcount)
                .bind(draft.deadline)
                .bind(json!(draft.requirements))
                .bind(&draft.position_ref)
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                let posting = load_posting(tx, posting_id).await?;
                Ok((
                    posting,
                    vec![audit(
                        org,
                        actor,
                        "recruiting.posting.update",
                        "recruit_posting",
                        posting_id,
                        now,
                        Some(json!({
                            "scope": draft.scope.as_db(),
                            "employment_type": draft.employment_type.as_db(),
                            "headcount": draft.headcount,
                        })),
                    )?],
                ))
            })
        })
        .await
    }

    /// Read-only preflight evaluation for the publish checklist modal. The
    /// attest row reflects the stored attestation (absent until publish); the
    /// `publishable` verdict covers the automatic checks only — publish itself
    /// re-evaluates everything atomically and fails closed.
    pub async fn preflight(&self, posting_id: Uuid) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn(&self.pool, org, |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT status, role_title, worksite, headcount, deadline, \
                     exposure_attested_at FROM recruit_postings WHERE id = $1",
                )
                .bind(posting_id)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(posting_not_found)?;
                let status = PostingStatus::from_db(row.try_get::<String, _>("status")?.as_str())?;
                if status != PostingStatus::Draft {
                    return Err(KernelError::conflict(
                        "publish preflight applies to a draft posting only",
                    )
                    .into());
                }
                let mut checks = auto_checks(tx, posting_id, &row).await?;
                let attested = row
                    .try_get::<Option<OffsetDateTime>, _>("exposure_attested_at")?
                    .is_some();
                let publishable = checks.iter().all(|check| check.ok);
                checks.push(PreflightCheck {
                    key: PREFLIGHT_EXPOSURE_ATTESTED,
                    ok: attested,
                    note: "exposure scope must be attested by the publisher".to_owned(),
                });
                Ok(json!({ "checks": checks, "publishable": publishable }))
            })
        })
        .await
    }

    pub async fn publish_posting(
        &self,
        actor: UserId,
        posting_id: Uuid,
        attest_exposure_scope: bool,
        expected_updated_at: OffsetDateTime,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT status, updated_at, role_title, worksite, headcount, deadline \
                     FROM recruit_postings WHERE id = $1 FOR UPDATE",
                )
                .bind(posting_id)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(posting_not_found)?;
                let status = PostingStatus::from_db(row.try_get::<String, _>("status")?.as_str())?;
                status.can_transition_to(PostingStatus::Published)?;
                check_cas(row.try_get("updated_at")?, expected_updated_at, "posting")?;
                let mut checks = auto_checks(tx, posting_id, &row).await?;
                checks.push(PreflightCheck {
                    key: PREFLIGHT_EXPOSURE_ATTESTED,
                    ok: attest_exposure_scope,
                    note: "exposure scope must be attested by the publisher".to_owned(),
                });
                if !checks.iter().all(|check| check.ok) {
                    return Err(PgRecruitingError::PreflightFailed(checks));
                }
                sqlx::query(
                    "UPDATE recruit_postings SET status='PUBLISHED', published_by=$2, \
                     published_at=$3, exposure_attested_by=$2, exposure_attested_at=$3, \
                     updated_at=$3 WHERE id=$1",
                )
                .bind(posting_id)
                .bind(*actor.as_uuid())
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                let posting = load_posting(tx, posting_id).await?;
                let snapshot = json!({ "checks": checks, "attest_exposure_scope": true });
                Ok((
                    posting,
                    vec![audit(
                        org,
                        actor,
                        "recruiting.posting.publish",
                        "recruit_posting",
                        posting_id,
                        now,
                        Some(snapshot),
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn close_posting(
        &self,
        actor: UserId,
        posting_id: Uuid,
        expected_updated_at: OffsetDateTime,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let (status, updated_at) = lock_posting(tx, posting_id).await?;
                status.can_transition_to(PostingStatus::Closed)?;
                check_cas(updated_at, expected_updated_at, "posting")?;
                sqlx::query(
                    "UPDATE recruit_postings SET status='CLOSED', closed_at=$2, updated_at=$2 \
                     WHERE id=$1",
                )
                .bind(posting_id)
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                let posting = load_posting(tx, posting_id).await?;
                Ok((
                    posting,
                    vec![audit(
                        org,
                        actor,
                        "recruiting.posting.close",
                        "recruit_posting",
                        posting_id,
                        now,
                        None,
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn create_applicant(
        &self,
        actor: UserId,
        posting_id: Uuid,
        intake: ApplicantIntake,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let id = Uuid::new_v4();
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let (status, _) = lock_posting(tx, posting_id).await?;
                if status != PostingStatus::Published {
                    return Err(KernelError::conflict(
                        "applicants can be registered only for a published posting",
                    )
                    .into());
                }
                let applicant_no = next_code(tx, org, CodeSeries::Applicant).await?;
                sqlx::query(
                    "INSERT INTO recruit_applicants (id, org_id, posting_id, applicant_no, name, \
                     profile, source_document, created_by, created_at, updated_at) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$9)",
                )
                .bind(id)
                .bind(*org.as_uuid())
                .bind(posting_id)
                .bind(&applicant_no)
                .bind(&intake.name)
                .bind(json!(intake.profile_lines))
                .bind(&intake.source_document)
                .bind(*actor.as_uuid())
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                stage_event(
                    tx,
                    org,
                    id,
                    "APPLY",
                    None,
                    Some(ApplicantStage::Applied.as_db()),
                    None,
                    actor,
                    now,
                )
                .await?;
                let applicant = load_applicant(tx, id).await?;
                Ok((
                    applicant,
                    vec![audit(
                        org,
                        actor,
                        "recruiting.applicant.create",
                        "recruit_applicant",
                        id,
                        now,
                        Some(json!({ "applicant_no": applicant_no, "posting_id": posting_id })),
                    )?],
                ))
            })
        })
        .await
    }

    /// Applicant detail is a PII surface (profile, assessment, offers): the
    /// server records an audited view event in the same transaction that
    /// serves the read.
    pub async fn applicant_detail(
        &self,
        actor: UserId,
        applicant_id: Uuid,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let applicant = load_applicant(tx, applicant_id).await?;
                let offers = sqlx::query(
                    "SELECT id, applicant_id, version, amount::TEXT AS amount, amount_period, \
                     currency, reply_deadline, status, withdraw_reason, extended_by, extended_at, resolved_at \
                     FROM recruit_offers WHERE applicant_id = $1 ORDER BY version DESC",
                )
                .bind(applicant_id)
                .fetch_all(tx.as_mut())
                .await?
                .iter()
                .map(offer_json)
                .collect::<Result<Vec<_>, PgRecruitingError>>()?;
                let events = sqlx::query(
                    "SELECT id, action, from_stage, to_stage, reason, actor, occurred_at \
                     FROM recruit_stage_events WHERE applicant_id = $1 \
                     ORDER BY occurred_at ASC, id ASC",
                )
                .bind(applicant_id)
                .fetch_all(tx.as_mut())
                .await?
                .iter()
                .map(event_json)
                .collect::<Result<Vec<_>, PgRecruitingError>>()?;
                Ok((
                    json!({ "applicant": applicant, "offers": offers, "events": events }),
                    vec![audit(
                        org,
                        actor,
                        "recruiting.applicant.view",
                        "recruit_applicant",
                        applicant_id,
                        now,
                        Some(json!({ "surfaces": ["profile", "assessment", "offers"] })),
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn advance_applicant(
        &self,
        actor: UserId,
        applicant_id: Uuid,
        expected_updated_at: OffsetDateTime,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let state = lock_applicant(tx, applicant_id).await?;
                state.require_active()?;
                check_cas(state.updated_at, expected_updated_at, "applicant")?;
                let next = state.stage.advance_target()?;
                sqlx::query("UPDATE recruit_applicants SET stage=$2, updated_at=$3 WHERE id=$1")
                    .bind(applicant_id)
                    .bind(next.as_db())
                    .bind(now)
                    .execute(tx.as_mut())
                    .await?;
                stage_event(
                    tx,
                    org,
                    applicant_id,
                    "ADVANCE",
                    Some(state.stage.as_db()),
                    Some(next.as_db()),
                    None,
                    actor,
                    now,
                )
                .await?;
                let applicant = load_applicant(tx, applicant_id).await?;
                Ok((
                    applicant,
                    vec![audit(
                        org,
                        actor,
                        "recruiting.applicant.advance",
                        "recruit_applicant",
                        applicant_id,
                        now,
                        Some(json!({ "from": state.stage.as_db(), "to": next.as_db() })),
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn assess_applicant(
        &self,
        actor: UserId,
        applicant_id: Uuid,
        score: AssessmentScore,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let state = lock_applicant(tx, applicant_id).await?;
                state.require_active()?;
                if state.stage != ApplicantStage::Interview {
                    return Err(KernelError::validation(
                        "assessment is recorded at the interview stage",
                    )
                    .into());
                }
                sqlx::query(
                    "UPDATE recruit_applicants SET assessment_score=$2, assessed_by=$3, \
                     assessed_at=$4, updated_at=$4 WHERE id=$1",
                )
                .bind(applicant_id)
                .bind(score.as_db())
                .bind(*actor.as_uuid())
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                stage_event(
                    tx,
                    org,
                    applicant_id,
                    "ASSESS",
                    None,
                    None,
                    Some(score.as_db()),
                    actor,
                    now,
                )
                .await?;
                let applicant = load_applicant(tx, applicant_id).await?;
                Ok((
                    applicant,
                    vec![audit(
                        org,
                        actor,
                        "recruiting.applicant.assess",
                        "recruit_applicant",
                        applicant_id,
                        now,
                        Some(json!({ "score": score.as_db() })),
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn set_hold(
        &self,
        actor: UserId,
        applicant_id: Uuid,
        hold: bool,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let state = lock_applicant(tx, applicant_id).await?;
                state.require_active()?;
                sqlx::query("UPDATE recruit_applicants SET hold=$2, updated_at=$3 WHERE id=$1")
                    .bind(applicant_id)
                    .bind(hold)
                    .bind(now)
                    .execute(tx.as_mut())
                    .await?;
                stage_event(
                    tx,
                    org,
                    applicant_id,
                    if hold { "HOLD" } else { "UNHOLD" },
                    None,
                    None,
                    None,
                    actor,
                    now,
                )
                .await?;
                let applicant = load_applicant(tx, applicant_id).await?;
                Ok((
                    applicant,
                    vec![audit(
                        org,
                        actor,
                        "recruiting.applicant.hold",
                        "recruit_applicant",
                        applicant_id,
                        now,
                        Some(json!({ "hold": hold })),
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn request_documents(
        &self,
        actor: UserId,
        applicant_id: Uuid,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let state = lock_applicant(tx, applicant_id).await?;
                state.require_active()?;
                sqlx::query(
                    "UPDATE recruit_applicants SET doc_requested=TRUE, updated_at=$2 WHERE id=$1",
                )
                .bind(applicant_id)
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                stage_event(
                    tx,
                    org,
                    applicant_id,
                    "REQUEST_DOCUMENTS",
                    None,
                    None,
                    None,
                    actor,
                    now,
                )
                .await?;
                let applicant = load_applicant(tx, applicant_id).await?;
                Ok((
                    applicant,
                    vec![audit(
                        org,
                        actor,
                        "recruiting.applicant.request_documents",
                        "recruit_applicant",
                        applicant_id,
                        now,
                        None,
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn reject_applicant(
        &self,
        actor: UserId,
        applicant_id: Uuid,
        rejection: Rejection,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let state = lock_applicant(tx, applicant_id).await?;
                if state.stage == ApplicantStage::Hired {
                    return Err(KernelError::conflict("hired applicant cannot be rejected").into());
                }
                if state.rejected {
                    return Err(KernelError::conflict("applicant is already rejected").into());
                }
                let live_offer: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM recruit_offers WHERE applicant_id = $1 \
                     AND status = 'EXTENDED')",
                )
                .bind(applicant_id)
                .fetch_one(tx.as_mut())
                .await?;
                if live_offer {
                    return Err(KernelError::conflict(
                        "withdraw the live offer before rejecting the applicant",
                    )
                    .into());
                }
                sqlx::query(
                    "UPDATE recruit_applicants SET rejected_at=$2, reject_reason=$3, \
                     reject_note=$4, updated_at=$2 WHERE id=$1",
                )
                .bind(applicant_id)
                .bind(now)
                .bind(rejection.reason.as_db())
                .bind(&rejection.note)
                .execute(tx.as_mut())
                .await?;
                stage_event(
                    tx,
                    org,
                    applicant_id,
                    "REJECT",
                    Some(state.stage.as_db()),
                    None,
                    Some(rejection.reason.as_db()),
                    actor,
                    now,
                )
                .await?;
                let applicant = load_applicant(tx, applicant_id).await?;
                Ok((
                    applicant,
                    vec![audit(
                        org,
                        actor,
                        "recruiting.applicant.reject",
                        "recruit_applicant",
                        applicant_id,
                        now,
                        Some(json!({ "reason": rejection.reason.as_db() })),
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn reinstate_applicant(
        &self,
        actor: UserId,
        applicant_id: Uuid,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let state = lock_applicant(tx, applicant_id).await?;
                if !state.rejected {
                    return Err(KernelError::conflict(
                        "only a rejected applicant can be reinstated",
                    )
                    .into());
                }
                sqlx::query(
                    "UPDATE recruit_applicants SET rejected_at=NULL, reject_reason=NULL, \
                     reject_note=NULL, updated_at=$2 WHERE id=$1",
                )
                .bind(applicant_id)
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                stage_event(
                    tx,
                    org,
                    applicant_id,
                    "REINSTATE",
                    None,
                    Some(state.stage.as_db()),
                    None,
                    actor,
                    now,
                )
                .await?;
                let applicant = load_applicant(tx, applicant_id).await?;
                Ok((
                    applicant,
                    vec![audit(
                        org,
                        actor,
                        "recruiting.applicant.reinstate",
                        "recruit_applicant",
                        applicant_id,
                        now,
                        None,
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn extend_offer(
        &self,
        actor: UserId,
        applicant_id: Uuid,
        terms: OfferTerms,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let id = Uuid::new_v4();
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let state = lock_applicant(tx, applicant_id).await?;
                state.require_active()?;
                if !matches!(
                    state.stage,
                    ApplicantStage::Interview | ApplicantStage::Offer
                ) {
                    return Err(KernelError::validation(
                        "an offer is extended from the interview stage",
                    )
                    .into());
                }
                if !state.assessed {
                    return Err(PgRecruitingError::AssessmentRequired);
                }
                let live_offer: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM recruit_offers WHERE applicant_id = $1 \
                     AND status = 'EXTENDED')",
                )
                .bind(applicant_id)
                .fetch_one(tx.as_mut())
                .await?;
                if live_offer {
                    return Err(KernelError::conflict(
                        "a live offer already exists for this applicant",
                    )
                    .into());
                }
                let version: i32 = sqlx::query_scalar(
                    "SELECT COALESCE(MAX(version), 0) + 1 FROM recruit_offers \
                     WHERE applicant_id = $1",
                )
                .bind(applicant_id)
                .fetch_one(tx.as_mut())
                .await?;
                insert_offer(tx, org, id, applicant_id, version, &terms, actor, now).await?;
                if state.stage == ApplicantStage::Interview {
                    sqlx::query(
                        "UPDATE recruit_applicants SET stage='OFFER', updated_at=$2 WHERE id=$1",
                    )
                    .bind(applicant_id)
                    .bind(now)
                    .execute(tx.as_mut())
                    .await?;
                }
                stage_event(
                    tx,
                    org,
                    applicant_id,
                    "OFFER_EXTEND",
                    Some(state.stage.as_db()),
                    Some(ApplicantStage::Offer.as_db()),
                    None,
                    actor,
                    now,
                )
                .await?;
                let offer = load_offer(tx, id).await?;
                Ok((
                    offer,
                    vec![audit(
                        org,
                        actor,
                        "recruiting.offer.extend",
                        "recruit_offer",
                        id,
                        now,
                        Some(json!({
                            "applicant_id": applicant_id,
                            "version": version,
                            "amount_period": terms.amount_period.as_db(),
                        })),
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn adjust_offer(
        &self,
        actor: UserId,
        offer_id: Uuid,
        amount: String,
        reply_deadline: Option<Date>,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let id = Uuid::new_v4();
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let offer = lock_offer(tx, offer_id).await?;
                if offer.status != OfferStatus::Extended {
                    return Err(KernelError::conflict("only a live offer can be adjusted").into());
                }
                sqlx::query(
                    "UPDATE recruit_offers SET status='SUPERSEDED', resolved_at=$2 WHERE id=$1",
                )
                .bind(offer_id)
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                let terms = OfferTerms {
                    amount,
                    amount_period: offer.amount_period,
                    reply_deadline: reply_deadline.unwrap_or(offer.reply_deadline),
                };
                insert_offer(
                    tx,
                    org,
                    id,
                    offer.applicant_id,
                    offer.version + 1,
                    &terms,
                    actor,
                    now,
                )
                .await?;
                stage_event(
                    tx,
                    org,
                    offer.applicant_id,
                    "OFFER_ADJUST",
                    None,
                    None,
                    None,
                    actor,
                    now,
                )
                .await?;
                let adjusted = load_offer(tx, id).await?;
                Ok((
                    adjusted,
                    vec![audit(
                        org,
                        actor,
                        "recruiting.offer.adjust",
                        "recruit_offer",
                        id,
                        now,
                        Some(json!({
                            "superseded_offer_id": offer_id,
                            "version": offer.version + 1,
                        })),
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn withdraw_offer(
        &self,
        actor: UserId,
        offer_id: Uuid,
        reason: String,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let offer = lock_offer(tx, offer_id).await?;
                if offer.status != OfferStatus::Extended {
                    return Err(KernelError::conflict("only a live offer can be withdrawn").into());
                }
                sqlx::query(
                    "UPDATE recruit_offers SET status='WITHDRAWN', withdraw_reason=$2, \
                     resolved_at=$3 WHERE id=$1",
                )
                .bind(offer_id)
                .bind(&reason)
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                sqlx::query(
                    "UPDATE recruit_applicants SET stage='INTERVIEW', updated_at=$2 WHERE id=$1",
                )
                .bind(offer.applicant_id)
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                stage_event(
                    tx,
                    org,
                    offer.applicant_id,
                    "OFFER_WITHDRAW",
                    Some(ApplicantStage::Offer.as_db()),
                    Some(ApplicantStage::Interview.as_db()),
                    Some(&reason),
                    actor,
                    now,
                )
                .await?;
                let withdrawn = load_offer(tx, offer_id).await?;
                Ok((
                    withdrawn,
                    vec![audit(
                        org,
                        actor,
                        "recruiting.offer.withdraw",
                        "recruit_offer",
                        offer_id,
                        now,
                        Some(json!({ "applicant_id": offer.applicant_id })),
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn record_offer_reply(
        &self,
        actor: UserId,
        offer_id: Uuid,
        decision: OfferStatus,
    ) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                if !matches!(decision, OfferStatus::Accepted | OfferStatus::Declined) {
                    return Err(
                        KernelError::validation("decision must be ACCEPTED or DECLINED").into(),
                    );
                }
                let offer = lock_offer(tx, offer_id).await?;
                if offer.status != OfferStatus::Extended {
                    return Err(
                        KernelError::conflict("only a live offer can record a reply").into(),
                    );
                }
                sqlx::query("UPDATE recruit_offers SET status=$2, resolved_at=$3 WHERE id=$1")
                    .bind(offer_id)
                    .bind(decision.as_db())
                    .bind(now)
                    .execute(tx.as_mut())
                    .await?;
                stage_event(
                    tx,
                    org,
                    offer.applicant_id,
                    "OFFER_REPLY",
                    None,
                    None,
                    Some(decision.as_db()),
                    actor,
                    now,
                )
                .await?;
                let replied = load_offer(tx, offer_id).await?;
                Ok((
                    replied,
                    vec![audit(
                        org,
                        actor,
                        "recruiting.offer.reply",
                        "recruit_offer",
                        offer_id,
                        now,
                        Some(json!({ "decision": decision.as_db() })),
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn talent_pool(&self) -> Result<Value, PgRecruitingError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn(&self.pool, org, |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    "SELECT a.id, a.applicant_no, a.name, p.role_title, a.reject_reason, \
                     a.rejected_at FROM recruit_applicants a \
                     JOIN recruit_postings p ON p.id = a.posting_id AND p.org_id = a.org_id \
                     WHERE a.rejected_at IS NOT NULL ORDER BY a.rejected_at DESC",
                )
                .fetch_all(tx.as_mut())
                .await?;
                let mut items = Vec::with_capacity(rows.len());
                for row in rows {
                    items.push(json!({
                        "applicant_id": row.try_get::<Uuid, _>("id")?,
                        "applicant_no": row.try_get::<String, _>("applicant_no")?,
                        "name": row.try_get::<String, _>("name")?,
                        "role_title": row.try_get::<String, _>("role_title")?,
                        "reason": row.try_get::<Option<String>, _>("reject_reason")?,
                        "rejected_at": opt_ts(row.try_get("rejected_at")?)?,
                    }));
                }
                Ok(json!({ "items": items }))
            })
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// Hire handshake pieces (called by the app-level handler inside ONE
// `with_audits` transaction together with the HR `create_employee_core`).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct HireContext {
    pub posting_id: Uuid,
    pub posting_no: String,
    pub applicant_no: String,
    pub candidate_name: String,
    pub company: String,
    pub employment_type: EmploymentType,
    /// Canonical decimal text of the accepted offer amount.
    pub accepted_amount: String,
    pub accepted_period: AmountPeriod,
}

/// Lock the applicant + posting rows and validate every hire precondition.
pub async fn hire_context(
    tx: &mut Transaction<'_, Postgres>,
    applicant_id: Uuid,
) -> Result<HireContext, PgRecruitingError> {
    let row = sqlx::query(
        "SELECT a.applicant_no, a.name, a.stage, a.rejected_at, a.hired_employee_id, \
         a.posting_id, p.posting_no, p.company, p.employment_type, p.status AS posting_status \
         FROM recruit_applicants a \
         JOIN recruit_postings p ON p.id = a.posting_id AND p.org_id = a.org_id \
         WHERE a.id = $1 FOR UPDATE OF a, p",
    )
    .bind(applicant_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(applicant_not_found)?;
    if let Some(employee_id) = row.try_get::<Option<Uuid>, _>("hired_employee_id")? {
        return Err(PgRecruitingError::AlreadyHired { employee_id });
    }
    if row
        .try_get::<Option<OffsetDateTime>, _>("rejected_at")?
        .is_some()
    {
        return Err(KernelError::conflict("rejected applicant cannot be hired").into());
    }
    let stage = ApplicantStage::from_db(row.try_get::<String, _>("stage")?.as_str())?;
    if stage != ApplicantStage::Offer {
        return Err(
            KernelError::validation("applicant must be at the offer stage to be hired").into(),
        );
    }
    let posting_status =
        PostingStatus::from_db(row.try_get::<String, _>("posting_status")?.as_str())?;
    if posting_status == PostingStatus::Closed {
        return Err(KernelError::conflict("posting is closed; hire is unavailable").into());
    }
    let offer = sqlx::query(
        "SELECT amount::TEXT AS amount, amount_period, status FROM recruit_offers \
         WHERE applicant_id = $1 ORDER BY version DESC LIMIT 1",
    )
    .bind(applicant_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let (accepted_amount, accepted_period) = match offer {
        Some(offer)
            if OfferStatus::from_db(offer.try_get::<String, _>("status")?.as_str())?
                == OfferStatus::Accepted =>
        {
            (
                offer.try_get::<String, _>("amount")?,
                AmountPeriod::from_db(offer.try_get::<String, _>("amount_period")?.as_str())?,
            )
        }
        _ => {
            return Err(
                KernelError::validation("the latest offer must be accepted before hire").into(),
            );
        }
    };
    Ok(HireContext {
        posting_id: row.try_get("posting_id")?,
        posting_no: row.try_get("posting_no")?,
        applicant_no: row.try_get("applicant_no")?,
        candidate_name: row.try_get("name")?,
        company: row.try_get("company")?,
        employment_type: EmploymentType::from_db(
            row.try_get::<String, _>("employment_type")?.as_str(),
        )?,
        accepted_amount,
        accepted_period,
    })
}

/// Link the hire result: applicant → HIRED with the employee id, posting fill
/// count +1 (DB check caps it at headcount), and the HIRE stage event.
pub async fn apply_hire(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    applicant_id: Uuid,
    posting_id: Uuid,
    employee_id: Uuid,
    actor: UserId,
    now: OffsetDateTime,
) -> Result<(), PgRecruitingError> {
    sqlx::query(
        "UPDATE recruit_applicants SET stage='HIRED', hired_employee_id=$2, updated_at=$3 \
         WHERE id=$1",
    )
    .bind(applicant_id)
    .bind(employee_id)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    sqlx::query(
        "UPDATE recruit_postings SET hired_count = hired_count + 1, updated_at=$2 WHERE id=$1",
    )
    .bind(posting_id)
    .bind(now)
    .execute(tx.as_mut())
    .await
    .map_err(|error| {
        if error
            .as_database_error()
            .and_then(|database| database.code())
            .is_some_and(|code| code == "23514")
        {
            PgRecruitingError::Domain(KernelError::conflict("posting headcount is already filled"))
        } else {
            PgRecruitingError::from(error)
        }
    })?;
    stage_event(
        tx,
        org,
        applicant_id,
        "HIRE",
        Some(ApplicantStage::Offer.as_db()),
        Some(ApplicantStage::Hired.as_db()),
        None,
        actor,
        now,
    )
    .await?;
    Ok(())
}

/// Build the platform audit event for a recruiting mutation. Recruiting rows
/// carry no branch column, so events are org-scoped.
pub fn audit(
    org: OrgId,
    actor: UserId,
    action: &str,
    kind: &str,
    id: Uuid,
    at: OffsetDateTime,
    after: Option<Value>,
) -> Result<AuditEvent, PgRecruitingError> {
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new(action).map_err(PgRecruitingError::Domain)?,
        kind,
        id.to_string(),
        TraceContext::generate(),
        at,
    )
    .with_org(org);
    Ok(match after {
        Some(after) => event.with_snapshots(None, Some(after)),
        None => event,
    })
}

// ---------------------------------------------------------------------------
// Shared row plumbing
// ---------------------------------------------------------------------------

enum CodeSeries {
    Posting,
    Applicant,
}

/// Allocate the next per-org display code (`JP-0001` / `APL-0001`) under an
/// advisory transaction lock; the unique index is the final guard.
async fn next_code(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    series: CodeSeries,
) -> Result<String, PgRecruitingError> {
    let (label, query, prefix) = match series {
        CodeSeries::Posting => (
            "recruit_postings",
            "SELECT COALESCE(MAX(substring(posting_no FROM 4)::int), 0) FROM recruit_postings",
            "JP",
        ),
        CodeSeries::Applicant => (
            "recruit_applicants",
            "SELECT COALESCE(MAX(substring(applicant_no FROM 5)::int), 0) FROM recruit_applicants",
            "APL",
        ),
    };
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("{label}:{}", org.as_uuid()))
        .execute(tx.as_mut())
        .await?;
    let current: i32 = sqlx::query_scalar(query).fetch_one(tx.as_mut()).await?;
    Ok(format!("{prefix}-{:04}", current + 1))
}

struct ApplicantState {
    stage: ApplicantStage,
    rejected: bool,
    assessed: bool,
    updated_at: OffsetDateTime,
}

impl ApplicantState {
    /// Reject/terminal guard shared by the pipeline mutations: rejected
    /// applicants must be reinstated first and HIRED is immutable.
    fn require_active(&self) -> Result<(), KernelError> {
        if self.rejected {
            return Err(KernelError::conflict(
                "rejected applicant must be reinstated first",
            ));
        }
        if self.stage == ApplicantStage::Hired {
            return Err(KernelError::conflict("hired applicant is terminal"));
        }
        Ok(())
    }
}

async fn lock_applicant(
    tx: &mut Transaction<'_, Postgres>,
    applicant_id: Uuid,
) -> Result<ApplicantState, PgRecruitingError> {
    let row = sqlx::query(
        "SELECT stage, rejected_at, assessment_score, updated_at FROM recruit_applicants \
         WHERE id = $1 FOR UPDATE",
    )
    .bind(applicant_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(applicant_not_found)?;
    Ok(ApplicantState {
        stage: ApplicantStage::from_db(row.try_get::<String, _>("stage")?.as_str())?,
        rejected: row
            .try_get::<Option<OffsetDateTime>, _>("rejected_at")?
            .is_some(),
        assessed: row
            .try_get::<Option<String>, _>("assessment_score")?
            .is_some(),
        updated_at: row.try_get("updated_at")?,
    })
}

async fn lock_posting(
    tx: &mut Transaction<'_, Postgres>,
    posting_id: Uuid,
) -> Result<(PostingStatus, OffsetDateTime), PgRecruitingError> {
    let row =
        sqlx::query("SELECT status, updated_at FROM recruit_postings WHERE id = $1 FOR UPDATE")
            .bind(posting_id)
            .fetch_optional(tx.as_mut())
            .await?
            .ok_or_else(posting_not_found)?;
    Ok((
        PostingStatus::from_db(row.try_get::<String, _>("status")?.as_str())?,
        row.try_get("updated_at")?,
    ))
}

struct OfferState {
    applicant_id: Uuid,
    version: i32,
    status: OfferStatus,
    amount_period: AmountPeriod,
    reply_deadline: Date,
}

async fn lock_offer(
    tx: &mut Transaction<'_, Postgres>,
    offer_id: Uuid,
) -> Result<OfferState, PgRecruitingError> {
    let row = sqlx::query(
        "SELECT applicant_id, version, status, amount_period, reply_deadline \
         FROM recruit_offers WHERE id = $1 FOR UPDATE",
    )
    .bind(offer_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| {
        PgRecruitingError::Domain(KernelError::not_found(
            "offer was not found in this organization",
        ))
    })?;
    Ok(OfferState {
        applicant_id: row.try_get("applicant_id")?,
        version: row.try_get("version")?,
        status: OfferStatus::from_db(row.try_get::<String, _>("status")?.as_str())?,
        amount_period: AmountPeriod::from_db(row.try_get::<String, _>("amount_period")?.as_str())?,
        reply_deadline: row.try_get("reply_deadline")?,
    })
}

#[allow(clippy::too_many_arguments)]
async fn insert_offer(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    id: Uuid,
    applicant_id: Uuid,
    version: i32,
    terms: &OfferTerms,
    actor: UserId,
    now: OffsetDateTime,
) -> Result<(), PgRecruitingError> {
    sqlx::query(
        "INSERT INTO recruit_offers (id, org_id, applicant_id, version, amount, amount_period, \
         reply_deadline, extended_by, extended_at) \
         VALUES ($1,$2,$3,$4,$5::numeric,$6,$7,$8,$9)",
    )
    .bind(id)
    .bind(*org.as_uuid())
    .bind(applicant_id)
    .bind(version)
    .bind(&terms.amount)
    .bind(terms.amount_period.as_db())
    .bind(terms.reply_deadline)
    .bind(*actor.as_uuid())
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn stage_event(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    applicant_id: Uuid,
    action: &str,
    from_stage: Option<&str>,
    to_stage: Option<&str>,
    reason: Option<&str>,
    actor: UserId,
    at: OffsetDateTime,
) -> Result<(), PgRecruitingError> {
    sqlx::query(
        "INSERT INTO recruit_stage_events (org_id, applicant_id, action, from_stage, to_stage, \
         reason, actor, occurred_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(*org.as_uuid())
    .bind(applicant_id)
    .bind(action)
    .bind(from_stage)
    .bind(to_stage)
    .bind(reason)
    .bind(*actor.as_uuid())
    .bind(at)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn auto_checks(
    tx: &mut Transaction<'_, Postgres>,
    posting_id: Uuid,
    row: &PgRow,
) -> Result<Vec<PreflightCheck>, PgRecruitingError> {
    let role_title: String = row.try_get("role_title")?;
    let worksite: String = row.try_get("worksite")?;
    let headcount: i32 = row.try_get("headcount")?;
    let deadline: Option<Date> = row.try_get("deadline")?;
    let duplicate_open: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM recruit_postings WHERE role_title = $1 \
         AND status = 'PUBLISHED' AND id <> $2)",
    )
    .bind(&role_title)
    .bind(posting_id)
    .fetch_one(tx.as_mut())
    .await?;
    Ok(vec![
        PreflightCheck {
            key: PREFLIGHT_ROLE_DEFINED,
            ok: !role_title.trim().is_empty() && !worksite.trim().is_empty(),
            note: "position and worksite are defined".to_owned(),
        },
        PreflightCheck {
            key: PREFLIGHT_QUOTA_DEFINED,
            ok: headcount >= 1,
            note: match deadline {
                Some(deadline) => format!("headcount {headcount}, deadline {deadline}"),
                None => format!("headcount {headcount}, open-ended deadline"),
            },
        },
        PreflightCheck {
            key: PREFLIGHT_NO_DUPLICATE_OPEN,
            ok: !duplicate_open,
            note: "no other published posting recruits the same position".to_owned(),
        },
    ])
}

pub async fn load_posting(
    tx: &mut Transaction<'_, Postgres>,
    posting_id: Uuid,
) -> Result<Value, PgRecruitingError> {
    let row = sqlx::query(
        "SELECT id, posting_no, role_title, company, worksite, employment_type, scope, \
         headcount, hired_count, deadline, requirements, position_ref, status, published_at, \
         closed_at, created_at, updated_at \
         FROM recruit_postings WHERE id = $1",
    )
    .bind(posting_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(posting_not_found)?;
    posting_json(&row)
}

pub async fn load_applicant(
    tx: &mut Transaction<'_, Postgres>,
    applicant_id: Uuid,
) -> Result<Value, PgRecruitingError> {
    let row = sqlx::query(
        "SELECT id, posting_id, applicant_no, name, profile, source_document, stage, \
         hold, doc_requested, rejected_at, reject_reason, reject_note, assessment_score, \
         assessed_by, assessed_at, hired_employee_id, created_at, updated_at \
         FROM recruit_applicants WHERE id = $1",
    )
    .bind(applicant_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(applicant_not_found)?;
    applicant_json(&row)
}

async fn load_offer(
    tx: &mut Transaction<'_, Postgres>,
    offer_id: Uuid,
) -> Result<Value, PgRecruitingError> {
    let row = sqlx::query(
        "SELECT id, applicant_id, version, amount::TEXT AS amount, amount_period, \
         currency, reply_deadline, status, withdraw_reason, extended_by, extended_at, resolved_at \
         FROM recruit_offers WHERE id = $1",
    )
    .bind(offer_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| {
        PgRecruitingError::Domain(KernelError::not_found(
            "offer was not found in this organization",
        ))
    })?;
    offer_json(&row)
}

fn posting_json(row: &PgRow) -> Result<Value, PgRecruitingError> {
    Ok(json!({
        "id": row.try_get::<Uuid, _>("id")?,
        "posting_no": row.try_get::<String, _>("posting_no")?,
        "role_title": row.try_get::<String, _>("role_title")?,
        "company": row.try_get::<String, _>("company")?,
        "worksite": row.try_get::<String, _>("worksite")?,
        "employment_type": row.try_get::<String, _>("employment_type")?,
        "scope": row.try_get::<String, _>("scope")?,
        "headcount": row.try_get::<i32, _>("headcount")?,
        "hired_count": row.try_get::<i32, _>("hired_count")?,
        "deadline": row.try_get::<Option<Date>, _>("deadline")?.map(|date| date.to_string()),
        "requirements": row.try_get::<Value, _>("requirements")?,
        "position_ref": row.try_get::<Option<String>, _>("position_ref")?,
        "status": row.try_get::<String, _>("status")?,
        "published_at": opt_ts(row.try_get("published_at")?)?,
        "closed_at": opt_ts(row.try_get("closed_at")?)?,
        "created_at": ts(row.try_get("created_at")?)?,
        "updated_at": ts(row.try_get("updated_at")?)?,
    }))
}

fn applicant_json(row: &PgRow) -> Result<Value, PgRecruitingError> {
    let assessment = match row.try_get::<Option<String>, _>("assessment_score")? {
        Some(score) => json!({
            "score": score,
            "by": row.try_get::<Option<Uuid>, _>("assessed_by")?,
            "at": opt_ts(row.try_get("assessed_at")?)?,
        }),
        None => Value::Null,
    };
    Ok(json!({
        "id": row.try_get::<Uuid, _>("id")?,
        "posting_id": row.try_get::<Uuid, _>("posting_id")?,
        "applicant_no": row.try_get::<String, _>("applicant_no")?,
        "name": row.try_get::<String, _>("name")?,
        "profile_lines": row.try_get::<Value, _>("profile")?,
        "source_document": row.try_get::<Option<String>, _>("source_document")?,
        "stage": row.try_get::<String, _>("stage")?,
        "hold": row.try_get::<bool, _>("hold")?,
        "doc_requested": row.try_get::<bool, _>("doc_requested")?,
        "rejected_at": opt_ts(row.try_get("rejected_at")?)?,
        "reject_reason": row.try_get::<Option<String>, _>("reject_reason")?,
        "reject_note": row.try_get::<Option<String>, _>("reject_note")?,
        "assessment": assessment,
        "hired_employee_id": row.try_get::<Option<Uuid>, _>("hired_employee_id")?,
        "created_at": ts(row.try_get("created_at")?)?,
        "updated_at": ts(row.try_get("updated_at")?)?,
    }))
}

/// The non-PII applicant projection for posting detail: pipeline routing
/// state only — no profile, provenance, note, or assessment signature.
fn applicant_summary_json(row: &PgRow) -> Result<Value, PgRecruitingError> {
    Ok(json!({
        "id": row.try_get::<Uuid, _>("id")?,
        "posting_id": row.try_get::<Uuid, _>("posting_id")?,
        "applicant_no": row.try_get::<String, _>("applicant_no")?,
        "name": row.try_get::<String, _>("name")?,
        "stage": row.try_get::<String, _>("stage")?,
        "hold": row.try_get::<bool, _>("hold")?,
        "doc_requested": row.try_get::<bool, _>("doc_requested")?,
        "rejected_at": opt_ts(row.try_get("rejected_at")?)?,
        "reject_reason": row.try_get::<Option<String>, _>("reject_reason")?,
        "assessed": row.try_get::<bool, _>("assessed")?,
        "hired_employee_id": row.try_get::<Option<Uuid>, _>("hired_employee_id")?,
        "created_at": ts(row.try_get("created_at")?)?,
        "updated_at": ts(row.try_get("updated_at")?)?,
    }))
}

fn offer_json(row: &PgRow) -> Result<Value, PgRecruitingError> {
    Ok(json!({
        "id": row.try_get::<Uuid, _>("id")?,
        "applicant_id": row.try_get::<Uuid, _>("applicant_id")?,
        "version": row.try_get::<i32, _>("version")?,
        "amount": row.try_get::<String, _>("amount")?,
        "amount_period": row.try_get::<String, _>("amount_period")?,
        "currency": row.try_get::<String, _>("currency")?,
        "reply_deadline": row.try_get::<Date, _>("reply_deadline")?.to_string(),
        "status": row.try_get::<String, _>("status")?,
        "withdraw_reason": row.try_get::<Option<String>, _>("withdraw_reason")?,
        "extended_by": row.try_get::<Uuid, _>("extended_by")?,
        "extended_at": ts(row.try_get("extended_at")?)?,
        "resolved_at": opt_ts(row.try_get("resolved_at")?)?,
    }))
}

fn event_json(row: &PgRow) -> Result<Value, PgRecruitingError> {
    Ok(json!({
        "id": row.try_get::<Uuid, _>("id")?,
        "action": row.try_get::<String, _>("action")?,
        "from_stage": row.try_get::<Option<String>, _>("from_stage")?,
        "to_stage": row.try_get::<Option<String>, _>("to_stage")?,
        "reason": row.try_get::<Option<String>, _>("reason")?,
        "actor": row.try_get::<Uuid, _>("actor")?,
        "occurred_at": ts(row.try_get("occurred_at")?)?,
    }))
}

fn check_cas(
    stored: OffsetDateTime,
    expected: OffsetDateTime,
    entity: &str,
) -> Result<(), PgRecruitingError> {
    if stored != expected {
        return Err(KernelError::conflict(format!(
            "{entity} changed since it was read; reload before retrying"
        ))
        .into());
    }
    Ok(())
}

fn posting_not_found() -> PgRecruitingError {
    PgRecruitingError::Domain(KernelError::not_found(
        "posting was not found in this organization",
    ))
}

fn applicant_not_found() -> PgRecruitingError {
    PgRecruitingError::Domain(KernelError::not_found(
        "applicant was not found in this organization",
    ))
}

fn ts(value: OffsetDateTime) -> Result<String, PgRecruitingError> {
    value
        .format(&Rfc3339)
        .map_err(|_| KernelError::internal("timestamp could not be formatted").into())
}

fn opt_ts(value: Option<OffsetDateTime>) -> Result<Value, PgRecruitingError> {
    Ok(match value {
        Some(value) => Value::String(ts(value)?),
        None => Value::Null,
    })
}
