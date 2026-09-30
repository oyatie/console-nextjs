//! Tenant-armed persistence for the evaluation console.
//!
//! Every mutation runs through `with_audits`: the state change, any derived
//! writes (RV- issuance), and the audit rows commit in one transaction with
//! `app.current_org` armed. Reads run through `with_org_conn` so RLS narrows
//! them to the request tenant; the person-ledger read is itself audited
//! (design §4.5). Preflight gates are re-computed inside the transition
//! transaction — the GET report is advisory, the transaction is the enforcer.

use console_evaluation_application::{
    CreateCycleInput, CycleDetail, CyclePage, CycleQuery, CycleSummary, EvidenceInput,
    EvidenceLinkView, GoalInput, GoalView, LedgerEntry, LedgerPage, PreflightItem, PreflightReport,
    ReviewDraftInput, ReviewView, SubjectDetail, SubjectSummary, TaskItem, TaskPage, UnitProgress,
};
use console_evaluation_domain::{
    CycleKind, CycleStage, CycleTransition, EvidenceKind, Grade, MetricKind, ReviewKind,
    ReviewStatus, derive_subject_state,
};
use console_kernel_core::{AuditAction, AuditEvent, KernelError, OrgId, TraceContext, UserId};
use console_platform_db::{DbError, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use serde_json::json;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PgEvaluationError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl From<sqlx::Error> for PgEvaluationError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

/// Minimal per-subject facts the REST boundary needs for its deny-by-omission
/// authorization decision, resolved under the request tenant (RLS conceals
/// cross-org subjects entirely).
#[derive(Debug, Clone, Copy)]
pub struct SubjectGate {
    pub cycle_id: Uuid,
    pub manager_user_id: UserId,
    pub cycle_stage: CycleStage,
}

#[derive(Debug, Clone)]
pub struct PgEvaluationStore {
    pool: PgPool,
}

impl PgEvaluationStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // -----------------------------------------------------------------------
    // Cycles
    // -----------------------------------------------------------------------

    pub async fn create_cycle(
        &self,
        actor: UserId,
        input: CreateCycleInput,
    ) -> Result<CycleDetail, PgEvaluationError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        let id = Uuid::new_v4();
        with_audits(&self.pool, org, move |tx| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO evaluation_cycles \
                       (id, org_id, name, kind, period_label, due_date, created_by, created_at, updated_at) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)",
                )
                .bind(id)
                .bind(*org.as_uuid())
                .bind(&input.name)
                .bind(input.kind.as_db())
                .bind(&input.period_label)
                .bind(input.due_date)
                .bind(*actor.as_uuid())
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                let detail = load_cycle_detail(tx, id)
                    .await?
                    .ok_or_else(|| KernelError::internal("created cycle did not load"))?;
                let event = audit_event(
                    org,
                    actor,
                    "evaluation.cycle.created",
                    "evaluation_cycle",
                    id,
                    now,
                    None,
                    Some(json!({
                        "name": input.name,
                        "kind": input.kind.as_db(),
                        "period_label": input.period_label,
                        "stage": "DRAFT",
                    })),
                )?;
                Ok((detail, vec![event]))
            })
        })
        .await
    }

    pub async fn list_cycles(&self, query: CycleQuery) -> Result<CyclePage, PgEvaluationError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn(&self.pool, org, move |tx| {
            Box::pin(async move {
                let stage = query.stage.map(CycleStage::as_db);
                let rows = sqlx::query(
                    "SELECT c.id, c.name, c.kind, c.period_label, c.due_date, c.stage, c.created_at, \
                            (SELECT count(*) FROM evaluation_subjects s WHERE s.cycle_id = c.id) AS subjects_total, \
                            (SELECT count(*) FROM evaluation_subjects s \
                               WHERE s.cycle_id = c.id AND EXISTS (SELECT 1 FROM evaluation_reviews r \
                                 WHERE r.subject_id = s.id AND r.kind = 'MANAGER' AND r.status = 'SUBMITTED')) AS manager_submitted, \
                            (SELECT count(*) FROM evaluation_subjects s \
                               WHERE s.cycle_id = c.id AND EXISTS (SELECT 1 FROM evaluation_reviews r \
                                 WHERE r.subject_id = s.id AND r.kind = 'SELF' AND r.status = 'SUBMITTED')) AS self_submitted, \
                            (SELECT count(*) FROM evaluation_subjects s \
                               WHERE s.cycle_id = c.id AND s.calibrated_grade IS NOT NULL) AS calibrated, \
                            (SELECT count(*) FROM evaluation_subjects s \
                               WHERE s.cycle_id = c.id AND s.final_grade IS NOT NULL) AS finalized \
                     FROM evaluation_cycles c \
                     WHERE ($1::text IS NULL AND c.stage <> 'ARCHIVED') OR c.stage = $1 \
                     ORDER BY c.created_at DESC \
                     LIMIT $2 OFFSET $3",
                )
                .bind(stage)
                .bind(query.limit)
                .bind(query.offset)
                .fetch_all(tx.as_mut())
                .await?;
                let total: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM evaluation_cycles c \
                     WHERE ($1::text IS NULL AND c.stage <> 'ARCHIVED') OR c.stage = $1",
                )
                .bind(stage)
                .fetch_one(tx.as_mut())
                .await?;
                let items = rows
                    .into_iter()
                    .map(cycle_summary_from_row)
                    .collect::<Result<Vec<_>, PgEvaluationError>>()?;
                Ok(CyclePage { items, total })
            })
        })
        .await
    }

    pub async fn get_cycle(
        &self,
        cycle_id: Uuid,
    ) -> Result<Option<CycleDetail>, PgEvaluationError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn(&self.pool, org, move |tx| {
            Box::pin(async move { load_cycle_detail(tx, cycle_id).await })
        })
        .await
    }

    pub async fn preflight(
        &self,
        cycle_id: Uuid,
    ) -> Result<Option<PreflightReport>, PgEvaluationError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn(&self.pool, org, move |tx| {
            Box::pin(async move {
                let Some(stage) = cycle_stage(tx, cycle_id, false).await? else {
                    return Ok(None);
                };
                let report = compute_preflight(tx, cycle_id, stage).await?;
                Ok(Some(report))
            })
        })
        .await
    }

    pub async fn transition_cycle(
        &self,
        actor: UserId,
        cycle_id: Uuid,
        transition: CycleTransition,
    ) -> Result<CycleDetail, PgEvaluationError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, move |tx| {
            Box::pin(async move {
                let stage = cycle_stage(tx, cycle_id, true)
                    .await?
                    .ok_or_else(|| KernelError::not_found("evaluation cycle was not found"))?;
                transition.guard(stage)?;
                let report = compute_preflight(tx, cycle_id, stage).await?;
                if !report.blockers.is_empty() {
                    let joined = report
                        .blockers
                        .iter()
                        .map(|item| item.message.as_str())
                        .collect::<Vec<_>>()
                        .join("; ");
                    return Err(KernelError::conflict(format!("preflight blocked: {joined}")).into());
                }
                let next = transition.to_stage();
                // One literal statement per transition: sqlx 0.9 (rightly)
                // rejects runtime-assembled SQL strings.
                let stamp_update = match transition {
                    CycleTransition::Open => {
                        "UPDATE evaluation_cycles SET stage = $1, opened_at = $2, updated_at = $2 WHERE id = $3"
                    }
                    CycleTransition::StartCalibration => {
                        "UPDATE evaluation_cycles SET stage = $1, calibration_started_at = $2, updated_at = $2 WHERE id = $3"
                    }
                    CycleTransition::Finalize => {
                        "UPDATE evaluation_cycles SET stage = $1, finalized_at = $2, updated_at = $2 WHERE id = $3"
                    }
                    CycleTransition::Archive => {
                        "UPDATE evaluation_cycles SET stage = $1, archived_at = $2, updated_at = $2 WHERE id = $3"
                    }
                };
                sqlx::query(stamp_update)
                .bind(next.as_db())
                .bind(now)
                .bind(cycle_id)
                .execute(tx.as_mut())
                .await?;

                let mut events = Vec::new();
                if transition == CycleTransition::Finalize {
                    events = finalize_subjects(tx, org, actor, cycle_id, now).await?;
                }
                let action = match transition {
                    CycleTransition::Open => "evaluation.cycle.opened",
                    CycleTransition::StartCalibration => "evaluation.calibration.started",
                    CycleTransition::Finalize => "evaluation.cycle.finalized",
                    CycleTransition::Archive => "evaluation.cycle.archived",
                };
                events.push(audit_event(
                    org,
                    actor,
                    action,
                    "evaluation_cycle",
                    cycle_id,
                    now,
                    Some(json!({ "stage": stage.as_db() })),
                    Some(json!({ "stage": next.as_db() })),
                )?);
                let detail = load_cycle_detail(tx, cycle_id)
                    .await?
                    .ok_or_else(|| KernelError::internal("transitioned cycle did not load"))?;
                Ok((detail, events))
            })
        })
        .await
    }

    // -----------------------------------------------------------------------
    // Subjects and goals
    // -----------------------------------------------------------------------

    pub async fn add_subject(
        &self,
        actor: UserId,
        cycle_id: Uuid,
        employee_id: Uuid,
        manager_user_id: UserId,
    ) -> Result<SubjectDetail, PgEvaluationError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        let id = Uuid::new_v4();
        with_audits(&self.pool, org, move |tx| {
            Box::pin(async move {
                let stage = cycle_stage(tx, cycle_id, true)
                    .await?
                    .ok_or_else(|| KernelError::not_found("evaluation cycle was not found"))?;
                if !matches!(stage, CycleStage::Draft | CycleStage::Open) {
                    return Err(KernelError::conflict(
                        "subjects can be enrolled only while the cycle is draft or open",
                    )
                    .into());
                }
                let employee: Option<PgRow> =
                    sqlx::query("SELECT name FROM employees WHERE id = $1")
                        .bind(employee_id)
                        .fetch_optional(tx.as_mut())
                        .await?;
                if employee.is_none() {
                    return Err(KernelError::not_found("employee was not found").into());
                }
                let manager: Option<i32> =
                    sqlx::query_scalar("SELECT 1 FROM users WHERE id = $1 AND is_active")
                        .bind(*manager_user_id.as_uuid())
                        .fetch_optional(tx.as_mut())
                        .await?;
                if manager.is_none() {
                    return Err(KernelError::not_found("manager user was not found").into());
                }
                let duplicate: Option<i32> = sqlx::query_scalar(
                    "SELECT 1 FROM evaluation_subjects WHERE cycle_id = $1 AND employee_id = $2",
                )
                .bind(cycle_id)
                .bind(employee_id)
                .fetch_optional(tx.as_mut())
                .await?;
                if duplicate.is_some() {
                    return Err(KernelError::conflict(
                        "employee is already enrolled in this cycle",
                    )
                    .into());
                }
                sqlx::query(
                    "INSERT INTO evaluation_subjects \
                       (id, org_id, cycle_id, employee_id, manager_user_id, created_at, updated_at) \
                     VALUES ($1, $2, $3, $4, $5, $6, $6)",
                )
                .bind(id)
                .bind(*org.as_uuid())
                .bind(cycle_id)
                .bind(employee_id)
                .bind(*manager_user_id.as_uuid())
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                let detail = load_subject_detail(tx, id)
                    .await?
                    .ok_or_else(|| KernelError::internal("enrolled subject did not load"))?;
                let event = audit_event(
                    org,
                    actor,
                    "evaluation.subject.added",
                    "evaluation_subject",
                    id,
                    now,
                    None,
                    Some(json!({
                        "cycle_id": cycle_id,
                        "employee_id": employee_id,
                        "manager_user_id": manager_user_id,
                    })),
                )?;
                Ok((detail, vec![event]))
            })
        })
        .await
    }

    pub async fn get_subject(
        &self,
        subject_id: Uuid,
    ) -> Result<Option<SubjectDetail>, PgEvaluationError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn(&self.pool, org, move |tx| {
            Box::pin(async move { load_subject_detail(tx, subject_id).await })
        })
        .await
    }

    pub async fn subject_gate(
        &self,
        subject_id: Uuid,
    ) -> Result<Option<SubjectGate>, PgEvaluationError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn(&self.pool, org, move |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT s.cycle_id, s.manager_user_id, c.stage \
                     FROM evaluation_subjects s \
                     JOIN evaluation_cycles c ON c.id = s.cycle_id \
                     WHERE s.id = $1",
                )
                .bind(subject_id)
                .fetch_optional(tx.as_mut())
                .await?;
                match row {
                    None => Ok(None),
                    Some(row) => {
                        let stage: String = row.try_get("stage")?;
                        Ok(Some(SubjectGate {
                            cycle_id: row.try_get("cycle_id")?,
                            manager_user_id: UserId::from_uuid(row.try_get("manager_user_id")?),
                            cycle_stage: CycleStage::from_db(&stage)?,
                        }))
                    }
                }
            })
        })
        .await
    }

    /// Load a subject for a submit-capable actor only when that actor holds a
    /// canonical, current relationship to it. The subject, identity, and
    /// detail are handled in one RLS-armed transaction: a concurrent employee
    /// relink or unlink cannot slip between the relationship predicate and
    /// the response body.
    pub async fn get_subject_for_review_actor(
        &self,
        actor: UserId,
        subject_id: Uuid,
    ) -> Result<Option<SubjectDetail>, PgEvaluationError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn(&self.pool, org, move |tx| {
            Box::pin(async move {
                let Some(subject) = lock_subject(tx, subject_id).await? else {
                    return Ok(None);
                };
                let actor_employee_id = lock_actor_employee_id(tx, actor).await?;
                let allowed = review_relationship_allows(
                    actor,
                    actor_employee_id,
                    subject.employee_id,
                    subject.manager_user_id,
                    ReviewKind::SelfReview,
                ) || review_relationship_allows(
                    actor,
                    actor_employee_id,
                    subject.employee_id,
                    subject.manager_user_id,
                    ReviewKind::Manager,
                );
                if !allowed {
                    return Ok(None);
                }
                load_subject_detail(tx, subject_id).await
            })
        })
        .await
    }

    pub async fn replace_goals(
        &self,
        actor: UserId,
        subject_id: Uuid,
        goals: Vec<GoalInput>,
    ) -> Result<SubjectDetail, PgEvaluationError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, move |tx| {
            Box::pin(async move {
                let locked = lock_subject(tx, subject_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("evaluation subject was not found"))?;
                if !matches!(locked.cycle_stage, CycleStage::Draft | CycleStage::Open) {
                    return Err(KernelError::conflict(
                        "goals are editable only while the cycle is draft or open",
                    )
                    .into());
                }
                let before = load_goals(tx, subject_id).await?;
                sqlx::query("DELETE FROM evaluation_goals WHERE subject_id = $1")
                    .bind(subject_id)
                    .execute(tx.as_mut())
                    .await?;
                for (index, goal) in goals.iter().enumerate() {
                    sqlx::query(
                        "INSERT INTO evaluation_goals \
                           (org_id, subject_id, title, metric_kind, target_label, weight_pct, sort_order) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7)",
                    )
                    .bind(*org.as_uuid())
                    .bind(subject_id)
                    .bind(&goal.title)
                    .bind(goal.metric_kind.as_db())
                    .bind(&goal.target_label)
                    .bind(goal.weight_pct)
                    .bind(i32::try_from(index).map_err(|_| {
                        KernelError::validation("too many goals")
                    })?)
                    .execute(tx.as_mut())
                    .await?;
                }
                sqlx::query("UPDATE evaluation_subjects SET updated_at = $1 WHERE id = $2")
                    .bind(now)
                    .bind(subject_id)
                    .execute(tx.as_mut())
                    .await?;
                let detail = load_subject_detail(tx, subject_id)
                    .await?
                    .ok_or_else(|| KernelError::internal("subject did not load after goals"))?;
                let event = audit_event(
                    org,
                    actor,
                    "evaluation.goals.replaced",
                    "evaluation_subject",
                    subject_id,
                    now,
                    Some(json!({ "goals": before })),
                    Some(json!({ "goals": detail.goals })),
                )?;
                Ok((detail, vec![event]))
            })
        })
        .await
    }

    // -----------------------------------------------------------------------
    // Reviews
    // -----------------------------------------------------------------------

    pub async fn save_review(
        &self,
        actor: UserId,
        subject_id: Uuid,
        kind: ReviewKind,
        input: ReviewDraftInput,
    ) -> Result<ReviewView, PgEvaluationError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, move |tx| {
            Box::pin(async move {
                let locked = lock_subject(tx, subject_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("evaluation subject was not found"))?;
                require_review_relationship(tx, actor, &locked, kind).await?;
                if locked.cycle_stage != CycleStage::Open {
                    return Err(KernelError::conflict(
                        "reviews are recorded only while the cycle is open",
                    )
                    .into());
                }
                let existing = sqlx::query(
                    "SELECT id, status FROM evaluation_reviews \
                     WHERE subject_id = $1 AND kind = $2 FOR UPDATE",
                )
                .bind(subject_id)
                .bind(kind.as_db())
                .fetch_optional(tx.as_mut())
                .await?;
                let review_id = match existing {
                    Some(row) => {
                        let status: String = row.try_get("status")?;
                        if ReviewStatus::from_db(&status)? == ReviewStatus::Submitted {
                            return Err(KernelError::conflict(
                                "submitted review is immutable",
                            )
                            .into());
                        }
                        let id: Uuid = row.try_get("id")?;
                        sqlx::query(
                            "UPDATE evaluation_reviews \
                             SET grade = $1, note = $2, evaluator_user_id = $3, updated_at = $4 \
                             WHERE id = $5",
                        )
                        .bind(input.grade.map(Grade::as_db))
                        .bind(input.note.as_deref())
                        .bind(*actor.as_uuid())
                        .bind(now)
                        .bind(id)
                        .execute(tx.as_mut())
                        .await?;
                        id
                    }
                    None => {
                        let id = Uuid::new_v4();
                        sqlx::query(
                            "INSERT INTO evaluation_reviews \
                               (id, org_id, subject_id, kind, status, evaluator_user_id, grade, note, created_at, updated_at) \
                             VALUES ($1, $2, $3, $4, 'DRAFT', $5, $6, $7, $8, $8)",
                        )
                        .bind(id)
                        .bind(*org.as_uuid())
                        .bind(subject_id)
                        .bind(kind.as_db())
                        .bind(*actor.as_uuid())
                        .bind(input.grade.map(Grade::as_db))
                        .bind(input.note.as_deref())
                        .bind(now)
                        .execute(tx.as_mut())
                        .await?;
                        id
                    }
                };
                replace_evidence(tx, org, review_id, &input.evidence_links).await?;
                let view = load_review(tx, review_id)
                    .await?
                    .ok_or_else(|| KernelError::internal("saved review did not load"))?;
                let event = audit_event(
                    org,
                    actor,
                    "evaluation.review.saved",
                    "evaluation_review",
                    review_id,
                    now,
                    None,
                    Some(json!({
                        "subject_id": subject_id,
                        "kind": kind.as_db(),
                        "grade": view.grade.map(Grade::as_db),
                        "evidence_links": view.evidence_links.len(),
                    })),
                )?;
                Ok((view, vec![event]))
            })
        })
        .await
    }

    pub async fn submit_review(
        &self,
        actor: UserId,
        subject_id: Uuid,
        kind: ReviewKind,
    ) -> Result<ReviewView, PgEvaluationError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, move |tx| {
            Box::pin(async move {
                let locked = lock_subject(tx, subject_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("evaluation subject was not found"))?;
                require_review_relationship(tx, actor, &locked, kind).await?;
                if locked.cycle_stage != CycleStage::Open {
                    return Err(KernelError::conflict(
                        "reviews are submitted only while the cycle is open",
                    )
                    .into());
                }
                let row = sqlx::query(
                    "SELECT id, status, grade FROM evaluation_reviews \
                     WHERE subject_id = $1 AND kind = $2 FOR UPDATE",
                )
                .bind(subject_id)
                .bind(kind.as_db())
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::conflict("no draft review to submit"))?;
                let status: String = row.try_get("status")?;
                if ReviewStatus::from_db(&status)? == ReviewStatus::Submitted {
                    return Err(KernelError::conflict("review is already submitted").into());
                }
                let grade: Option<String> = row.try_get("grade")?;
                if grade.is_none() {
                    return Err(KernelError::conflict("grade is required to submit").into());
                }
                let review_id: Uuid = row.try_get("id")?;
                if kind == ReviewKind::Manager {
                    let evidence: i64 = sqlx::query_scalar(
                        "SELECT count(*) FROM evaluation_evidence_links WHERE review_id = $1",
                    )
                    .bind(review_id)
                    .fetch_one(tx.as_mut())
                    .await?;
                    if evidence == 0 {
                        return Err(KernelError::conflict(
                            "manager review requires at least one evidence link",
                        )
                        .into());
                    }
                }
                sqlx::query(
                    "UPDATE evaluation_reviews \
                     SET status = 'SUBMITTED', submitted_at = $1, evaluator_user_id = $2, updated_at = $1 \
                     WHERE id = $3",
                )
                .bind(now)
                .bind(*actor.as_uuid())
                .bind(review_id)
                .execute(tx.as_mut())
                .await?;
                let view = load_review(tx, review_id)
                    .await?
                    .ok_or_else(|| KernelError::internal("submitted review did not load"))?;
                let event = audit_event(
                    org,
                    actor,
                    "evaluation.review.submitted",
                    "evaluation_review",
                    review_id,
                    now,
                    None,
                    Some(json!({
                        "subject_id": subject_id,
                        "kind": kind.as_db(),
                        "grade": view.grade.map(Grade::as_db),
                        "evidence_refs": view
                            .evidence_links
                            .iter()
                            .map(|link| link.object_ref.clone())
                            .collect::<Vec<_>>(),
                    })),
                )?;
                Ok((view, vec![event]))
            })
        })
        .await
    }

    // -----------------------------------------------------------------------
    // Calibration
    // -----------------------------------------------------------------------

    pub async fn calibrate(
        &self,
        actor: UserId,
        subject_id: Uuid,
        final_grade: Grade,
        reason: Option<String>,
    ) -> Result<SubjectDetail, PgEvaluationError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, move |tx| {
            Box::pin(async move {
                let locked = lock_subject(tx, subject_id)
                    .await?
                    .ok_or_else(|| KernelError::not_found("evaluation subject was not found"))?;
                if locked.cycle_stage != CycleStage::Calibration {
                    return Err(KernelError::conflict(
                        "calibration is recorded only while the cycle is in calibration",
                    )
                    .into());
                }
                let manager_review = sqlx::query(
                    "SELECT evaluator_user_id, grade FROM evaluation_reviews \
                     WHERE subject_id = $1 AND kind = 'MANAGER' AND status = 'SUBMITTED'",
                )
                .bind(subject_id)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::conflict("manager review is not submitted"))?;
                let evaluator: Uuid = manager_review.try_get("evaluator_user_id")?;
                if evaluator == *actor.as_uuid() {
                    return Err(KernelError::conflict(
                        "calibration requires four eyes: the calibrator must differ from the manager evaluator",
                    )
                    .into());
                }
                let manager_grade: Option<String> = manager_review.try_get("grade")?;
                let manager_grade = manager_grade
                    .as_deref()
                    .map(Grade::from_db)
                    .transpose()?
                    .ok_or_else(|| KernelError::conflict("manager review has no grade"))?;
                if final_grade != manager_grade && reason.is_none() {
                    return Err(KernelError::conflict(
                        "calibration reason is required when the final grade differs from the manager grade",
                    )
                    .into());
                }
                let before: Option<String> = sqlx::query_scalar(
                    "SELECT calibrated_grade FROM evaluation_subjects WHERE id = $1",
                )
                .bind(subject_id)
                .fetch_one(tx.as_mut())
                .await?;
                sqlx::query(
                    "UPDATE evaluation_subjects \
                     SET calibrated_grade = $1, calibration_reason = $2, calibrated_by = $3, \
                         calibrated_at = $4, updated_at = $4 \
                     WHERE id = $5",
                )
                .bind(final_grade.as_db())
                .bind(reason.as_deref())
                .bind(*actor.as_uuid())
                .bind(now)
                .bind(subject_id)
                .execute(tx.as_mut())
                .await?;
                let detail = load_subject_detail(tx, subject_id)
                    .await?
                    .ok_or_else(|| KernelError::internal("calibrated subject did not load"))?;
                let event = audit_event(
                    org,
                    actor,
                    "evaluation.subject.calibrated",
                    "evaluation_subject",
                    subject_id,
                    now,
                    Some(json!({ "calibrated_grade": before })),
                    Some(json!({
                        "calibrated_grade": final_grade.as_db(),
                        "reason": reason,
                        "manager_grade": manager_grade.as_db(),
                    })),
                )?;
                Ok((detail, vec![event]))
            })
        })
        .await
    }

    // -----------------------------------------------------------------------
    // Tasks and person ledger
    // -----------------------------------------------------------------------

    pub async fn my_tasks(&self, caller: UserId) -> Result<TaskPage, PgEvaluationError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn(&self.pool, org, move |tx| {
            Box::pin(async move {
                // A task is an authorization result, not a cacheable view of
                // a JWT claim. Lock the current user-to-employee relation in
                // this request transaction before selecting SELF work so an
                // unlink or relink cannot race the returned assignment.
                let Some(actor_employee_id) = lock_actor_employee_id(tx, caller).await? else {
                    return Ok(TaskPage { items: Vec::new() });
                };
                let rows = sqlx::query(
                    "SELECT s.id AS subject_id, c.id AS cycle_id, c.name AS cycle_name, \
                            c.due_date, s.employee_id, e.name AS employee_name, \
                            k.kind, r.status AS review_status \
                     FROM evaluation_subjects s \
                     JOIN evaluation_cycles c ON c.id = s.cycle_id AND c.stage = 'OPEN' \
                     JOIN employees e ON e.id = s.employee_id \
                     CROSS JOIN (VALUES ('SELF'), ('MANAGER')) AS k(kind) \
                     LEFT JOIN evaluation_reviews r ON r.subject_id = s.id AND r.kind = k.kind \
                     WHERE ((k.kind = 'SELF' AND s.employee_id = $1) \
                         OR (k.kind = 'MANAGER' AND s.manager_user_id = $2)) \
                       AND (r.id IS NULL OR r.status = 'DRAFT') \
                     ORDER BY c.due_date, e.name, k.kind",
                )
                .bind(actor_employee_id)
                .bind(*caller.as_uuid())
                .fetch_all(tx.as_mut())
                .await?;
                let mut items = Vec::with_capacity(rows.len());
                for row in rows {
                    let kind: String = row.try_get("kind")?;
                    let status: Option<String> = row.try_get("review_status")?;
                    items.push(TaskItem {
                        subject_id: row.try_get("subject_id")?,
                        cycle_id: row.try_get("cycle_id")?,
                        cycle_name: row.try_get("cycle_name")?,
                        due_date: row.try_get::<Date, _>("due_date")?,
                        employee_id: row.try_get("employee_id")?,
                        employee_name: row.try_get("employee_name")?,
                        kind: ReviewKind::from_db(&kind)?,
                        review_status: status.as_deref().map(ReviewStatus::from_db).transpose()?,
                    });
                }
                Ok(TaskPage { items })
            })
        })
        .await
    }

    /// Audited read (design §4.5): viewing a person's finalized evaluation
    /// history writes `evaluation.history.viewed` in the same transaction.
    pub async fn employee_ledger(
        &self,
        actor: UserId,
        employee_id: Uuid,
    ) -> Result<LedgerPage, PgEvaluationError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, move |tx| {
            Box::pin(async move {
                let exists: Option<i32> =
                    sqlx::query_scalar("SELECT 1 FROM employees WHERE id = $1")
                        .bind(employee_id)
                        .fetch_optional(tx.as_mut())
                        .await?;
                if exists.is_none() {
                    return Err(KernelError::not_found("employee was not found").into());
                }
                let rows = sqlx::query(
                    "SELECT s.rv_code, s.cycle_id, c.name AS cycle_name, c.period_label, \
                            s.final_grade, s.finalized_at, s.id AS subject_id \
                     FROM evaluation_subjects s \
                     JOIN evaluation_cycles c ON c.id = s.cycle_id \
                     WHERE s.employee_id = $1 AND s.final_grade IS NOT NULL \
                     ORDER BY s.finalized_at DESC",
                )
                .bind(employee_id)
                .fetch_all(tx.as_mut())
                .await?;
                let mut items = Vec::with_capacity(rows.len());
                for row in rows {
                    let grade: String = row.try_get("final_grade")?;
                    items.push(LedgerEntry {
                        rv_code: row.try_get("rv_code")?,
                        cycle_id: row.try_get("cycle_id")?,
                        cycle_name: row.try_get("cycle_name")?,
                        period_label: row.try_get("period_label")?,
                        final_grade: Grade::from_db(&grade)?,
                        finalized_at: row.try_get("finalized_at")?,
                        subject_id: row.try_get("subject_id")?,
                    });
                }
                let event = audit_event(
                    org,
                    actor,
                    "evaluation.history.viewed",
                    "employee",
                    employee_id,
                    now,
                    None,
                    None,
                )?;
                Ok((LedgerPage { items }, vec![event]))
            })
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// Shared transaction helpers
// ---------------------------------------------------------------------------

struct LockedSubject {
    cycle_stage: CycleStage,
    employee_id: Uuid,
    manager_user_id: UserId,
}

/// Lock the subject row and its owning cycle row together so a concurrent
/// stage transition cannot slip between the guard and the write.
async fn lock_subject(
    tx: &mut Transaction<'_, Postgres>,
    subject_id: Uuid,
) -> Result<Option<LockedSubject>, PgEvaluationError> {
    let row = sqlx::query(
        "SELECT c.stage, s.employee_id, s.manager_user_id \
         FROM evaluation_subjects s \
         JOIN evaluation_cycles c ON c.id = s.cycle_id \
         WHERE s.id = $1 FOR UPDATE",
    )
    .bind(subject_id)
    .fetch_optional(tx.as_mut())
    .await?;
    match row {
        None => Ok(None),
        Some(row) => {
            let stage: String = row.try_get("stage")?;
            Ok(Some(LockedSubject {
                cycle_stage: CycleStage::from_db(&stage)?,
                employee_id: row.try_get("employee_id")?,
                manager_user_id: UserId::from_uuid(row.try_get("manager_user_id")?),
            }))
        }
    }
}

/// Resolve the canonical `users.employee_id` relation in the same RLS-armed
/// transaction as the protected review write. A missing link is never
/// inferred from names, roles, or client-provided identity.
async fn require_review_relationship(
    tx: &mut Transaction<'_, Postgres>,
    actor: UserId,
    subject: &LockedSubject,
    kind: ReviewKind,
) -> Result<(), PgEvaluationError> {
    let actor_employee_id = lock_actor_employee_id(tx, actor).await?;
    let permitted = review_relationship_allows(
        actor,
        actor_employee_id,
        subject.employee_id,
        subject.manager_user_id,
        kind,
    );
    if permitted {
        Ok(())
    } else {
        // Keep relationship denials indistinguishable from an RLS-hidden
        // subject; a submit-capable caller must not enumerate review
        // assignments by probing subject ids or review kinds.
        Err(KernelError::not_found("evaluation subject was not found").into())
    }
}

/// Lock the canonical actor identity with a mode that conflicts with the
/// `UPDATE users SET employee_id = ...` path used for link/unlink changes.
/// This keeps a review authorization decision and its protected operation
/// serialized with identity relinking, without trusting a JWT employee claim.
async fn lock_actor_employee_id(
    tx: &mut Transaction<'_, Postgres>,
    actor: UserId,
) -> Result<Option<Uuid>, PgEvaluationError> {
    sqlx::query_scalar("SELECT employee_id FROM users WHERE id = $1 FOR NO KEY UPDATE")
        .bind(*actor.as_uuid())
        .fetch_optional(tx.as_mut())
        .await
        .map(|employee_id: Option<Option<Uuid>>| employee_id.flatten())
        .map_err(Into::into)
}

fn review_relationship_allows(
    actor: UserId,
    actor_employee_id: Option<Uuid>,
    subject_employee_id: Uuid,
    manager_user_id: UserId,
    kind: ReviewKind,
) -> bool {
    match kind {
        ReviewKind::SelfReview => actor_employee_id == Some(subject_employee_id),
        ReviewKind::Manager => actor_employee_id.is_some() && manager_user_id == actor,
    }
}

async fn cycle_stage(
    tx: &mut Transaction<'_, Postgres>,
    cycle_id: Uuid,
    lock: bool,
) -> Result<Option<CycleStage>, PgEvaluationError> {
    let query = if lock {
        "SELECT stage FROM evaluation_cycles WHERE id = $1 FOR UPDATE"
    } else {
        "SELECT stage FROM evaluation_cycles WHERE id = $1"
    };
    let stage: Option<String> = sqlx::query_scalar(query)
        .bind(cycle_id)
        .fetch_optional(tx.as_mut())
        .await?;
    stage
        .as_deref()
        .map(CycleStage::from_db)
        .transpose()
        .map_err(PgEvaluationError::from)
}

/// One row per subject with exactly the facts the preflight gates need.
struct SubjectFacts {
    id: Uuid,
    employee_name: String,
    goals: i64,
    manager_submitted: bool,
    self_submitted: bool,
    calibrated: bool,
}

async fn subject_facts(
    tx: &mut Transaction<'_, Postgres>,
    cycle_id: Uuid,
) -> Result<Vec<SubjectFacts>, PgEvaluationError> {
    let rows = sqlx::query(
        "SELECT s.id, e.name AS employee_name, \
                (SELECT count(*) FROM evaluation_goals g WHERE g.subject_id = s.id) AS goals, \
                EXISTS (SELECT 1 FROM evaluation_reviews r \
                        WHERE r.subject_id = s.id AND r.kind = 'MANAGER' AND r.status = 'SUBMITTED') AS manager_submitted, \
                EXISTS (SELECT 1 FROM evaluation_reviews r \
                        WHERE r.subject_id = s.id AND r.kind = 'SELF' AND r.status = 'SUBMITTED') AS self_submitted, \
                s.calibrated_grade IS NOT NULL AS calibrated \
         FROM evaluation_subjects s \
         JOIN employees e ON e.id = s.employee_id \
         WHERE s.cycle_id = $1 \
         ORDER BY e.name, s.created_at",
    )
    .bind(cycle_id)
    .fetch_all(tx.as_mut())
    .await?;
    let mut facts = Vec::with_capacity(rows.len());
    for row in rows {
        facts.push(SubjectFacts {
            id: row.try_get("id")?,
            employee_name: row.try_get("employee_name")?,
            goals: row.try_get("goals")?,
            manager_submitted: row.try_get("manager_submitted")?,
            self_submitted: row.try_get("self_submitted")?,
            calibrated: row.try_get("calibrated")?,
        });
    }
    Ok(facts)
}

async fn compute_preflight(
    tx: &mut Transaction<'_, Postgres>,
    cycle_id: Uuid,
    stage: CycleStage,
) -> Result<PreflightReport, PgEvaluationError> {
    let Some(transition) = stage.next_transition() else {
        return Ok(PreflightReport {
            next_transition: None,
            blockers: Vec::new(),
            advisories: Vec::new(),
        });
    };
    let mut blockers = Vec::new();
    let mut advisories = Vec::new();
    match transition {
        CycleTransition::Open => {
            let facts = subject_facts(tx, cycle_id).await?;
            if facts.is_empty() {
                blockers.push(PreflightItem {
                    code: "no_subjects".into(),
                    message: "cycle has no enrolled subjects".into(),
                    subject_id: None,
                });
            }
            for fact in &facts {
                if fact.goals == 0 {
                    blockers.push(PreflightItem {
                        code: "subject_without_goals".into(),
                        message: format!("{} has no goals", fact.employee_name),
                        subject_id: Some(fact.id),
                    });
                }
            }
        }
        CycleTransition::StartCalibration => {
            let facts = subject_facts(tx, cycle_id).await?;
            for fact in &facts {
                if !fact.manager_submitted {
                    blockers.push(PreflightItem {
                        code: "manager_review_missing".into(),
                        message: format!("{} has no submitted manager review", fact.employee_name),
                        subject_id: Some(fact.id),
                    });
                }
                if !fact.self_submitted {
                    advisories.push(PreflightItem {
                        code: "self_review_missing".into(),
                        message: format!("{} has no submitted self review", fact.employee_name),
                        subject_id: Some(fact.id),
                    });
                }
            }
        }
        CycleTransition::Finalize => {
            let facts = subject_facts(tx, cycle_id).await?;
            for fact in &facts {
                if !fact.calibrated {
                    blockers.push(PreflightItem {
                        code: "subject_not_calibrated".into(),
                        message: format!("{} is not calibrated", fact.employee_name),
                        subject_id: Some(fact.id),
                    });
                }
            }
        }
        CycleTransition::Archive => {}
    }
    Ok(PreflightReport {
        next_transition: Some(transition),
        blockers,
        advisories,
    })
}

/// Issue one RV- code per subject under the per-org counter lock and stamp the
/// final grade; returns one audit event per finalized subject.
async fn finalize_subjects(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    actor: UserId,
    cycle_id: Uuid,
    now: OffsetDateTime,
) -> Result<Vec<AuditEvent>, PgEvaluationError> {
    let subjects = sqlx::query(
        "SELECT id, calibrated_grade FROM evaluation_subjects \
         WHERE cycle_id = $1 ORDER BY created_at FOR UPDATE",
    )
    .bind(cycle_id)
    .fetch_all(tx.as_mut())
    .await?;
    sqlx::query(
        "INSERT INTO evaluation_code_counters (org_id) VALUES ($1) \
         ON CONFLICT (org_id) DO NOTHING",
    )
    .bind(*org.as_uuid())
    .execute(tx.as_mut())
    .await?;
    let mut next: i32 = sqlx::query_scalar(
        "SELECT next_value FROM evaluation_code_counters WHERE org_id = $1 FOR UPDATE",
    )
    .bind(*org.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    let mut events = Vec::with_capacity(subjects.len());
    for row in subjects {
        let subject_id: Uuid = row.try_get("id")?;
        let grade: Option<String> = row.try_get("calibrated_grade")?;
        let grade = grade.ok_or_else(|| KernelError::conflict("subject is not calibrated"))?;
        let rv_code = format!("RV-{next}");
        sqlx::query(
            "UPDATE evaluation_subjects \
             SET final_grade = $1, rv_code = $2, finalized_at = $3, updated_at = $3 \
             WHERE id = $4",
        )
        .bind(&grade)
        .bind(&rv_code)
        .bind(now)
        .bind(subject_id)
        .execute(tx.as_mut())
        .await?;
        events.push(audit_event(
            org,
            actor,
            "evaluation.subject.finalized",
            "evaluation_subject",
            subject_id,
            now,
            None,
            Some(json!({ "rv_code": rv_code, "final_grade": grade })),
        )?);
        next = next
            .checked_add(1)
            .ok_or_else(|| KernelError::internal("evaluation code counter overflow"))?;
    }
    sqlx::query("UPDATE evaluation_code_counters SET next_value = $1 WHERE org_id = $2")
        .bind(next)
        .bind(*org.as_uuid())
        .execute(tx.as_mut())
        .await?;
    Ok(events)
}

async fn load_goals(
    tx: &mut Transaction<'_, Postgres>,
    subject_id: Uuid,
) -> Result<Vec<GoalView>, PgEvaluationError> {
    let rows = sqlx::query(
        "SELECT id, title, metric_kind, target_label, weight_pct, sort_order \
         FROM evaluation_goals WHERE subject_id = $1 ORDER BY sort_order",
    )
    .bind(subject_id)
    .fetch_all(tx.as_mut())
    .await?;
    let mut goals = Vec::with_capacity(rows.len());
    for row in rows {
        let metric: String = row.try_get("metric_kind")?;
        goals.push(GoalView {
            id: row.try_get("id")?,
            title: row.try_get("title")?,
            metric_kind: MetricKind::from_db(&metric)?,
            target_label: row.try_get("target_label")?,
            weight_pct: row.try_get("weight_pct")?,
            sort_order: row.try_get("sort_order")?,
        });
    }
    Ok(goals)
}

async fn load_review(
    tx: &mut Transaction<'_, Postgres>,
    review_id: Uuid,
) -> Result<Option<ReviewView>, PgEvaluationError> {
    let row = sqlx::query(
        "SELECT id, subject_id, kind, status, evaluator_user_id, grade, note, submitted_at, updated_at \
         FROM evaluation_reviews WHERE id = $1",
    )
    .bind(review_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(review_from_row(tx, &row).await?))
}

async fn load_reviews_for_subject(
    tx: &mut Transaction<'_, Postgres>,
    subject_id: Uuid,
) -> Result<Vec<ReviewView>, PgEvaluationError> {
    let rows = sqlx::query(
        "SELECT id, subject_id, kind, status, evaluator_user_id, grade, note, submitted_at, updated_at \
         FROM evaluation_reviews WHERE subject_id = $1 ORDER BY kind",
    )
    .bind(subject_id)
    .fetch_all(tx.as_mut())
    .await?;
    let mut reviews = Vec::with_capacity(rows.len());
    for row in rows {
        reviews.push(review_from_row(tx, &row).await?);
    }
    Ok(reviews)
}

async fn review_from_row(
    tx: &mut Transaction<'_, Postgres>,
    row: &PgRow,
) -> Result<ReviewView, PgEvaluationError> {
    let id: Uuid = row.try_get("id")?;
    let kind: String = row.try_get("kind")?;
    let status: String = row.try_get("status")?;
    let grade: Option<String> = row.try_get("grade")?;
    let links = sqlx::query(
        "SELECT id, object_kind, object_ref, label, sort_order \
         FROM evaluation_evidence_links WHERE review_id = $1 ORDER BY sort_order",
    )
    .bind(id)
    .fetch_all(tx.as_mut())
    .await?;
    let mut evidence_links = Vec::with_capacity(links.len());
    for link in links {
        let object_kind: String = link.try_get("object_kind")?;
        evidence_links.push(EvidenceLinkView {
            id: link.try_get("id")?,
            object_kind: EvidenceKind::from_db(&object_kind)?,
            object_ref: link.try_get("object_ref")?,
            label: link.try_get("label")?,
            sort_order: link.try_get("sort_order")?,
        });
    }
    Ok(ReviewView {
        id,
        subject_id: row.try_get("subject_id")?,
        kind: ReviewKind::from_db(&kind)?,
        status: ReviewStatus::from_db(&status)?,
        evaluator_user_id: row.try_get("evaluator_user_id")?,
        grade: grade.as_deref().map(Grade::from_db).transpose()?,
        note: row.try_get("note")?,
        evidence_links,
        submitted_at: row.try_get("submitted_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn replace_evidence(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    review_id: Uuid,
    links: &[EvidenceInput],
) -> Result<(), PgEvaluationError> {
    sqlx::query("DELETE FROM evaluation_evidence_links WHERE review_id = $1")
        .bind(review_id)
        .execute(tx.as_mut())
        .await?;
    for (index, link) in links.iter().enumerate() {
        sqlx::query(
            "INSERT INTO evaluation_evidence_links \
               (org_id, review_id, object_kind, object_ref, label, sort_order) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(*org.as_uuid())
        .bind(review_id)
        .bind(link.object_kind.as_db())
        .bind(&link.object_ref)
        .bind(&link.label)
        .bind(i32::try_from(index).map_err(|_| KernelError::validation("too many evidence links"))?)
        .execute(tx.as_mut())
        .await?;
    }
    Ok(())
}

async fn load_subject_detail(
    tx: &mut Transaction<'_, Postgres>,
    subject_id: Uuid,
) -> Result<Option<SubjectDetail>, PgEvaluationError> {
    let row = sqlx::query(
        "SELECT s.id, s.cycle_id, s.employee_id, e.name AS employee_name, e.org_unit, \
                s.manager_user_id, s.calibrated_grade, s.calibration_reason, s.calibrated_by, \
                s.calibrated_at, s.final_grade, s.rv_code, s.finalized_at, \
                EXISTS (SELECT 1 FROM evaluation_reviews r WHERE r.subject_id = s.id) AS has_review, \
                EXISTS (SELECT 1 FROM evaluation_reviews r \
                        WHERE r.subject_id = s.id AND r.kind = 'MANAGER' AND r.status = 'SUBMITTED') AS manager_submitted \
         FROM evaluation_subjects s \
         JOIN employees e ON e.id = s.employee_id \
         WHERE s.id = $1",
    )
    .bind(subject_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let calibrated_grade: Option<String> = row.try_get("calibrated_grade")?;
    let final_grade: Option<String> = row.try_get("final_grade")?;
    let has_review: bool = row.try_get("has_review")?;
    let manager_submitted: bool = row.try_get("manager_submitted")?;
    let state = derive_subject_state(
        has_review,
        manager_submitted,
        calibrated_grade.is_some(),
        final_grade.is_some(),
    );
    let goals = load_goals(tx, subject_id).await?;
    let reviews = load_reviews_for_subject(tx, subject_id).await?;
    Ok(Some(SubjectDetail {
        id: row.try_get("id")?,
        cycle_id: row.try_get("cycle_id")?,
        employee_id: row.try_get("employee_id")?,
        employee_name: row.try_get("employee_name")?,
        org_unit: row.try_get("org_unit")?,
        manager_user_id: row.try_get("manager_user_id")?,
        state,
        final_grade: final_grade.as_deref().map(Grade::from_db).transpose()?,
        rv_code: row.try_get("rv_code")?,
        goals,
        reviews,
        calibrated_grade: calibrated_grade
            .as_deref()
            .map(Grade::from_db)
            .transpose()?,
        calibration_reason: row.try_get("calibration_reason")?,
        calibrated_by: row.try_get("calibrated_by")?,
        calibrated_at: row.try_get("calibrated_at")?,
        finalized_at: row.try_get("finalized_at")?,
    }))
}

async fn load_cycle_detail(
    tx: &mut Transaction<'_, Postgres>,
    cycle_id: Uuid,
) -> Result<Option<CycleDetail>, PgEvaluationError> {
    let row = sqlx::query(
        "SELECT id, name, kind, period_label, due_date, stage, created_by, created_at, \
                opened_at, calibration_started_at, finalized_at, archived_at \
         FROM evaluation_cycles WHERE id = $1",
    )
    .bind(cycle_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let kind: String = row.try_get("kind")?;
    let stage: String = row.try_get("stage")?;

    let subject_rows = sqlx::query(
        "SELECT s.id, s.cycle_id, s.employee_id, e.name AS employee_name, e.org_unit, \
                s.manager_user_id, s.calibrated_grade, s.final_grade, s.rv_code, \
                EXISTS (SELECT 1 FROM evaluation_reviews r WHERE r.subject_id = s.id) AS has_review, \
                EXISTS (SELECT 1 FROM evaluation_reviews r \
                        WHERE r.subject_id = s.id AND r.kind = 'MANAGER' AND r.status = 'SUBMITTED') AS manager_submitted, \
                EXISTS (SELECT 1 FROM evaluation_reviews r \
                        WHERE r.subject_id = s.id AND r.kind = 'SELF' AND r.status = 'SUBMITTED') AS self_submitted \
         FROM evaluation_subjects s \
         JOIN employees e ON e.id = s.employee_id \
         WHERE s.cycle_id = $1 \
         ORDER BY e.name, s.created_at",
    )
    .bind(cycle_id)
    .fetch_all(tx.as_mut())
    .await?;

    let mut subjects = Vec::with_capacity(subject_rows.len());
    let mut manager_submitted_total = 0_i64;
    let mut self_submitted_total = 0_i64;
    let mut calibrated_total = 0_i64;
    let mut finalized_total = 0_i64;
    let mut units: Vec<UnitProgress> = Vec::new();
    for subject in subject_rows {
        let calibrated_grade: Option<String> = subject.try_get("calibrated_grade")?;
        let final_grade: Option<String> = subject.try_get("final_grade")?;
        let has_review: bool = subject.try_get("has_review")?;
        let manager_submitted: bool = subject.try_get("manager_submitted")?;
        let self_submitted: bool = subject.try_get("self_submitted")?;
        let org_unit: Option<String> = subject.try_get("org_unit")?;
        manager_submitted_total += i64::from(manager_submitted);
        self_submitted_total += i64::from(self_submitted);
        calibrated_total += i64::from(calibrated_grade.is_some());
        finalized_total += i64::from(final_grade.is_some());
        match units.iter_mut().find(|unit| unit.org_unit == org_unit) {
            Some(unit) => {
                unit.total += 1;
                unit.manager_submitted += i64::from(manager_submitted);
            }
            None => units.push(UnitProgress {
                org_unit: org_unit.clone(),
                total: 1,
                manager_submitted: i64::from(manager_submitted),
            }),
        }
        subjects.push(SubjectSummary {
            id: subject.try_get("id")?,
            cycle_id: subject.try_get("cycle_id")?,
            employee_id: subject.try_get("employee_id")?,
            employee_name: subject.try_get("employee_name")?,
            org_unit,
            manager_user_id: subject.try_get("manager_user_id")?,
            state: derive_subject_state(
                has_review,
                manager_submitted,
                calibrated_grade.is_some(),
                final_grade.is_some(),
            ),
            final_grade: final_grade.as_deref().map(Grade::from_db).transpose()?,
            rv_code: subject.try_get("rv_code")?,
        });
    }

    Ok(Some(CycleDetail {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        kind: CycleKind::from_db(&kind)?,
        period_label: row.try_get("period_label")?,
        due_date: row.try_get::<Date, _>("due_date")?,
        stage: CycleStage::from_db(&stage)?,
        subjects_total: i64::try_from(subjects.len())
            .map_err(|_| KernelError::internal("subject count overflow"))?,
        manager_submitted: manager_submitted_total,
        self_submitted: self_submitted_total,
        calibrated: calibrated_total,
        finalized: finalized_total,
        created_at: row.try_get("created_at")?,
        opened_at: row.try_get("opened_at")?,
        calibration_started_at: row.try_get("calibration_started_at")?,
        finalized_at: row.try_get("finalized_at")?,
        archived_at: row.try_get("archived_at")?,
        created_by: row.try_get("created_by")?,
        progress_by_unit: units,
        subjects,
    }))
}

fn cycle_summary_from_row(row: PgRow) -> Result<CycleSummary, PgEvaluationError> {
    let kind: String = row.try_get("kind")?;
    let stage: String = row.try_get("stage")?;
    Ok(CycleSummary {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        kind: CycleKind::from_db(&kind)?,
        period_label: row.try_get("period_label")?,
        due_date: row.try_get::<Date, _>("due_date")?,
        stage: CycleStage::from_db(&stage)?,
        subjects_total: row.try_get("subjects_total")?,
        manager_submitted: row.try_get("manager_submitted")?,
        self_submitted: row.try_get("self_submitted")?,
        calibrated: row.try_get("calibrated")?,
        finalized: row.try_get("finalized")?,
        created_at: row.try_get("created_at")?,
    })
}

#[allow(clippy::too_many_arguments)]
fn audit_event(
    org: OrgId,
    actor: UserId,
    action: &str,
    target_type: &str,
    target_id: Uuid,
    at: OffsetDateTime,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
) -> Result<AuditEvent, PgEvaluationError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new(action).map_err(PgEvaluationError::Domain)?,
        target_type,
        target_id.to_string(),
        TraceContext::generate(),
        at,
    )
    .with_org(org)
    .with_snapshots(before, after))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_relationships_are_kind_aware_and_fail_closed_without_employee_link() {
        let subject_employee = Uuid::new_v4();
        let manager_employee = Uuid::new_v4();
        let subject_user = UserId::new();
        let manager_user = UserId::new();

        assert!(review_relationship_allows(
            subject_user,
            Some(subject_employee),
            subject_employee,
            manager_user,
            ReviewKind::SelfReview,
        ));
        assert!(!review_relationship_allows(
            subject_user,
            Some(subject_employee),
            subject_employee,
            manager_user,
            ReviewKind::Manager,
        ));
        assert!(review_relationship_allows(
            manager_user,
            Some(manager_employee),
            subject_employee,
            manager_user,
            ReviewKind::Manager,
        ));
        assert!(!review_relationship_allows(
            manager_user,
            Some(manager_employee),
            subject_employee,
            manager_user,
            ReviewKind::SelfReview,
        ));
        assert!(!review_relationship_allows(
            manager_user,
            None,
            subject_employee,
            manager_user,
            ReviewKind::Manager,
        ));
    }
}
