//! Tenant-scoped persistence for attendance.  All SQL is bind-parameterized;
//! reads use RLS-bound connections and mutations write the domain audit event
//! in the same transaction as their state transition.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::BTreeMap;

use console_attendance_application::{
    self as app, AcknowledgeWeek52, AmendClose, AssignSubstitute, AttendanceEvidence,
    AttendanceExceptionRead, AttendanceObjectLink, AttendancePage, AttendanceSubstitutionRead,
    CallerScope, CancelSubstitution, CloseAmendmentRead, CloseCheckRead, CloseChecks, CloseMonth,
    ClosePreflightRead, ExceptionResolutionRead, ListExceptions, ListSubstitutions, MonthCloseRead,
    RaiseException, ResolveException, SubstitutionCandidateFacts, SubstitutionCandidateQuery,
    SubstitutionCandidateRead, Week52AcknowledgementRead, Week52Read,
};
use console_attendance_domain::{
    AttendanceDateRange, ExceptionKind, HistoricalAbsence, StrictDurationEvent,
    StrictDurationEventKind, StrictDurationWindow, strict_pair_seconds,
};
use console_kernel_core::{AuditAction, AuditEvent, BranchId, OrgId, TraceContext};
use console_platform_db::{DbError, issue_code, with_audits, with_org_conn};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use time::{Date, Duration, OffsetDateTime, UtcOffset};
use uuid::Uuid;

mod self_service;

const CANCEL_SUBSTITUTION_SQL: &str = "UPDATE attendance_substitutions SET status='CANCELLED', cancel_reason=$1 WHERE id=$2 AND status='ASSIGNED'";
const SUBSTITUTION_CANDIDATES_SQL: &str = "SELECT e.id,e.name,e.home_branch_id,TRUE AS employment_active,FALSE AS conflicts_with_assigned_substitution,FALSE AS approved_leave_covers_window,FALSE AS has_open_no_show FROM employees e JOIN employee_employment_profiles p ON p.employee_id=e.id AND p.org_id=e.org_id WHERE e.home_branch_id=$1 AND e.id <> $2 AND e.employment_status='ACTIVE' AND ($6::text IS NULL OR e.name ILIKE '%' || $6 || '%') AND NOT EXISTS (SELECT 1 FROM attendance_substitutions s WHERE s.worker_employee_id=e.id AND s.status='ASSIGNED' AND s.cover_date=$3 AND s.from_minutes < $5 AND s.to_minutes > $4) AND NOT EXISTS (SELECT 1 FROM leave_requests l WHERE l.subject_employee_id=e.id AND l.status IN ('approved','APPROVED') AND l.start_date <= $3 AND l.end_date >= $3 AND (l.leave_type='annual' OR (l.leave_type='half_day' AND ((l.partial_day_period='am' AND $4 < 720) OR (l.partial_day_period='pm' AND $5 > 720))))) AND NOT EXISTS (SELECT 1 FROM attendance_exceptions x WHERE x.employee_id=e.id AND x.kind='NO_SHOW' AND x.status='OPEN' AND x.work_date=$3) ORDER BY e.name,e.id LIMIT $7 OFFSET $8";
const SUBSTITUTION_CANDIDATE_TOTAL_SQL: &str = "SELECT count(*) FROM employees e JOIN employee_employment_profiles p ON p.employee_id=e.id AND p.org_id=e.org_id WHERE e.home_branch_id=$1 AND e.id <> $2 AND e.employment_status='ACTIVE' AND ($6::text IS NULL OR e.name ILIKE '%' || $6 || '%') AND NOT EXISTS (SELECT 1 FROM attendance_substitutions s WHERE s.worker_employee_id=e.id AND s.status='ASSIGNED' AND s.cover_date=$3 AND s.from_minutes < $5 AND s.to_minutes > $4) AND NOT EXISTS (SELECT 1 FROM leave_requests l WHERE l.subject_employee_id=e.id AND l.status IN ('approved','APPROVED') AND l.start_date <= $3 AND l.end_date >= $3 AND (l.leave_type='annual' OR (l.leave_type='half_day' AND ((l.partial_day_period='am' AND $4 < 720) OR (l.partial_day_period='pm' AND $5 > 720))))) AND NOT EXISTS (SELECT 1 FROM attendance_exceptions x WHERE x.employee_id=e.id AND x.kind='NO_SHOW' AND x.status='OPEN' AND x.work_date=$3)";
const LIST_EXCEPTIONS_SQL: &str = "\
    SELECT e.id,e.code,e.kind,e.status,e.employee_id,employee.name AS employee_name,\
           employee.org_unit AS team,e.branch_id,e.work_date,e.created_at AS occurred_at,\
           e.detail,e.evidence,e.links,e.created_at,r.action AS resolution_action,\
           r.reason AS resolution_reason,r.linked_work_ref,r.ot_hours::text AS ot_hours,r.actor_user_id,\
           r.resolved_at AS resolved_at \
    FROM attendance_exceptions e \
    JOIN employees employee ON employee.id=e.employee_id AND employee.org_id=e.org_id \
    LEFT JOIN attendance_exception_resolutions r \
           ON r.exception_id=e.id AND r.org_id=e.org_id \
    WHERE e.work_date >= $1 AND e.work_date < $2 \
      AND ($3::uuid IS NULL OR e.branch_id=$3) \
      AND ($4::text IS NULL OR e.status=$4) \
      AND ($5::uuid IS NULL OR e.employee_id=$5) \
    ORDER BY e.work_date DESC,e.created_at DESC,e.id DESC \
    LIMIT $6 OFFSET $7";
const EXCEPTION_BY_ID_SQL: &str = "\
    SELECT e.id,e.code,e.kind,e.status,e.employee_id,employee.name AS employee_name,\
           employee.org_unit AS team,e.branch_id,e.work_date,e.created_at AS occurred_at,\
           e.detail,e.evidence,e.links,e.created_at,r.action AS resolution_action,\
           r.reason AS resolution_reason,r.linked_work_ref,r.ot_hours::text AS ot_hours,r.actor_user_id,\
           r.resolved_at AS resolved_at \
    FROM attendance_exceptions e \
    JOIN employees employee ON employee.id=e.employee_id AND employee.org_id=e.org_id \
    LEFT JOIN attendance_exception_resolutions r \
           ON r.exception_id=e.id AND r.org_id=e.org_id \
    WHERE e.id=$1";
const SUBSTITUTION_ELIGIBILITY_LOCK_SQL: &str =
    "SELECT public.console_employee_day_eligibility_lock($1,$2,$3)";

#[derive(Debug, thiserror::Error)]
pub enum AttendanceStoreError {
    #[error(transparent)]
    Application(#[from] app::AttendanceApplicationError),
    #[error(transparent)]
    Database(#[from] DbError),
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
    #[error("not found")]
    NotFound,
    #[error("conflict")]
    Conflict,
    #[error("close is blocked")]
    CloseBlocked,
    #[error("invalid close month")]
    InvalidCloseMonth,
}

#[derive(Debug, Clone)]
pub struct PgAttendanceStore {
    pool: PgPool,
}
impl PgAttendanceStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn list_substitutions(
        &self,
        caller: &CallerScope,
        query: ListSubstitutions,
    ) -> Result<AttendancePage<AttendanceSubstitutionRead>, AttendanceStoreError> {
        app::ensure_scope(caller, query.branch_id)?;
        let org = OrgId::from_uuid(caller.org_id);
        with_org_conn::<_, _, AttendanceStoreError>(&self.pool, org, move |tx| Box::pin(async move {
            let rows = sqlx::query("SELECT s.id,s.site,s.branch_id,s.role,s.cover_date,s.from_minutes,s.to_minutes,s.covered_employee_id,cov.name AS covered_name,s.reason_kind,s.reason_detail,s.worker_employee_id,s.worker_name,s.worker_type,s.worker_rate,s.status,s.exception_id,s.created_by,s.created_at FROM attendance_substitutions s JOIN employees cov ON cov.id=s.covered_employee_id AND cov.org_id=s.org_id WHERE s.cover_date >= $1 AND s.cover_date < $2 AND ($3::uuid IS NULL OR s.branch_id=$3) ORDER BY s.cover_date,s.from_minutes,s.created_at LIMIT $4 OFFSET $5")
                .bind(query.range.from).bind(query.range.to_exclusive).bind(query.branch_id).bind(query.limit).bind(query.offset).fetch_all(tx.as_mut()).await?;
            let total: i64 = sqlx::query_scalar("SELECT count(*) FROM attendance_substitutions WHERE cover_date >= $1 AND cover_date < $2 AND ($3::uuid IS NULL OR branch_id=$3)")
                .bind(query.range.from).bind(query.range.to_exclusive).bind(query.branch_id).fetch_one(tx.as_mut()).await?;
            let items = rows.iter().map(substitution_read).collect::<Result<Vec<_>, _>>()?;
            Ok(AttendancePage { items, total, limit: query.limit, offset: query.offset })
        })).await
    }

    pub async fn list_substitution_candidates(
        &self,
        caller: &CallerScope,
        query: SubstitutionCandidateQuery,
    ) -> Result<AttendancePage<SubstitutionCandidateRead>, AttendanceStoreError> {
        app::ensure_scope(caller, Some(query.branch_id))?;
        let window = query.window()?;
        let org = OrgId::from_uuid(caller.org_id);
        with_org_conn::<_, _, AttendanceStoreError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let rows = sqlx::query(SUBSTITUTION_CANDIDATES_SQL)
                    .bind(query.branch_id)
                    .bind(query.covered_employee_id)
                    .bind(window.cover_date)
                    .bind(window.from_minutes)
                    .bind(window.to_minutes)
                    .bind(&query.search)
                    .bind(query.limit)
                    .bind(query.offset)
                    .fetch_all(tx.as_mut())
                    .await?;
                let total: i64 = sqlx::query_scalar(SUBSTITUTION_CANDIDATE_TOTAL_SQL)
                    .bind(query.branch_id)
                    .bind(query.covered_employee_id)
                    .bind(window.cover_date)
                    .bind(window.from_minutes)
                    .bind(window.to_minutes)
                    .bind(&query.search)
                    .fetch_one(tx.as_mut())
                    .await?;
                let items = rows
                    .iter()
                    .filter_map(|row| {
                        candidate_read(row, query.branch_id, query.covered_employee_id).transpose()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(AttendancePage {
                    items,
                    total,
                    limit: query.limit,
                    offset: query.offset,
                })
            })
        })
        .await
    }

    pub async fn list_exceptions(
        &self,
        caller: &CallerScope,
        query: ListExceptions,
    ) -> Result<AttendancePage<AttendanceExceptionRead>, AttendanceStoreError> {
        app::ensure_scope(caller, query.branch_id)?;
        let org = OrgId::from_uuid(caller.org_id);
        with_org_conn::<_,_,AttendanceStoreError>(&self.pool,org,move|tx|Box::pin(async move { let rows=sqlx::query(LIST_EXCEPTIONS_SQL).bind(query.range.from).bind(query.range.to_exclusive).bind(query.branch_id).bind(&query.status).bind(query.employee_id).bind(query.limit).bind(query.offset).fetch_all(tx.as_mut()).await?; let total:i64=sqlx::query_scalar("SELECT count(*) FROM attendance_exceptions e WHERE e.work_date >= $1 AND e.work_date < $2 AND ($3::uuid IS NULL OR e.branch_id=$3) AND ($4::text IS NULL OR e.status=$4) AND ($5::uuid IS NULL OR e.employee_id=$5)").bind(query.range.from).bind(query.range.to_exclusive).bind(query.branch_id).bind(&query.status).bind(query.employee_id).fetch_one(tx.as_mut()).await?; Ok(AttendancePage{items:rows.iter().map(exception_read).collect::<Result<Vec<_>,_>>()?,total,limit:query.limit,offset:query.offset})})).await
    }

    pub async fn raise_exception(
        &self,
        caller: &CallerScope,
        command: RaiseException,
    ) -> Result<AttendanceExceptionRead, AttendanceStoreError> {
        let key = app::validate_idempotency_key(&command.idempotency_key)?;
        let detail = app::validate_text(&command.detail, "detail", 2000)?;
        let org = OrgId::from_uuid(caller.org_id);
        let caller = caller.clone();
        let actor = caller.user_id;
        let id = Uuid::new_v4();
        with_audits::<_, _, AttendanceStoreError>(&self.pool, org, move |tx| Box::pin(async move {
            idempotency_lock(tx, caller.org_id, &key).await?;
            let existing = sqlx::query_as::<_,(Uuid,String,Option<Uuid>)>("SELECT id,request_fingerprint,branch_id FROM attendance_exceptions WHERE idempotency_key=$1").bind(&key).fetch_optional(tx.as_mut()).await?;
            if let Some((_, _, existing_branch)) = existing.as_ref() {
                app::ensure_scope(&caller, *existing_branch)?;
            }
            let branch = active_employee_branch(tx, command.employee_id).await?;
            if command.branch_id != Some(branch) { return Err(AttendanceStoreError::Conflict); }
            app::ensure_scope(&caller, Some(branch))?;
            let request=json!({"kind":command.kind,"employeeId":command.employee_id,"branchId":branch,"workDate":command.work_date,"detail":detail,"evidence":command.evidence}); let fp=fingerprint(&request);
            let evidence_json = serde_json::to_value(&command.evidence).map_err(|_| AttendanceStoreError::Conflict)?;
            if let Some((existing, stored, _)) = existing { if stored==fp { return Ok((exception_by_id(tx,existing).await?,Vec::new())); } return Err(AttendanceStoreError::Conflict); }
            let code=issue_code(tx, OrgId::from_uuid(caller.org_id), "attendance_exception").await?;
            sqlx::query("INSERT INTO attendance_exceptions (id,org_id,code,kind,employee_id,branch_id,work_date,detail,evidence,links,idempotency_key,request_fingerprint,created_by) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'[]'::jsonb,$10,$11,$12)")
                .bind(id).bind(caller.org_id).bind(&code).bind(command.kind.as_db()).bind(command.employee_id).bind(branch).bind(command.work_date).bind(&detail).bind(evidence_json).bind(&key).bind(&fp).bind(actor).execute(tx.as_mut()).await?;
            let view=exception_by_id(tx,id).await?; let audit=event(&caller,"attendance.exception.raise","attendance_exception",id,Some(branch),Some(json!({"code":code})))?; Ok((view,vec![audit]))
        })).await
    }

    /// Returns the branch for a tenant-scoped exception before a REST boundary
    /// authorizes a resource-id mutation. The caller maps inaccessible rows to
    /// the same 404 as absent rows to avoid branch-existence disclosure.
    pub async fn exception_branch(
        &self,
        org_id: Uuid,
        exception_id: Uuid,
    ) -> Result<Option<Option<Uuid>>, AttendanceStoreError> {
        with_org_conn::<_, _, AttendanceStoreError>(
            &self.pool,
            OrgId::from_uuid(org_id),
            move |tx| {
                Box::pin(async move {
                    sqlx::query_scalar("SELECT branch_id FROM attendance_exceptions WHERE id=$1")
                        .bind(exception_id)
                        .fetch_optional(tx.as_mut())
                        .await
                        .map_err(AttendanceStoreError::from)
                })
            },
        )
        .await
    }

    pub async fn exception_detail(
        &self,
        caller: &CallerScope,
        exception_id: Uuid,
    ) -> Result<AttendanceExceptionRead, AttendanceStoreError> {
        let org = OrgId::from_uuid(caller.org_id);
        let caller = caller.clone();
        with_org_conn::<_, _, AttendanceStoreError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let row = sqlx::query("SELECT branch_id FROM attendance_exceptions WHERE id=$1")
                    .bind(exception_id)
                    .fetch_optional(tx.as_mut())
                    .await?
                    .ok_or(AttendanceStoreError::NotFound)?;
                app::ensure_scope(&caller, row.try_get("branch_id")?)?;
                exception_by_id(tx, exception_id).await
            })
        })
        .await
    }

    pub async fn resolve_exception(
        &self,
        caller: &CallerScope,
        command: ResolveException,
    ) -> Result<AttendanceExceptionRead, AttendanceStoreError> {
        let reason = app::validate_text(&command.reason, "reason", 2000)?;
        let org = OrgId::from_uuid(caller.org_id);
        let caller = caller.clone();
        let actor = caller.user_id;
        with_audits::<_, _, AttendanceStoreError>(&self.pool, org, move |tx| Box::pin(async move {
            let row=sqlx::query("SELECT kind,branch_id,status FROM attendance_exceptions WHERE id=$1 FOR UPDATE").bind(command.exception_id).fetch_optional(tx.as_mut()).await?.ok_or(AttendanceStoreError::NotFound)?;
            let branch=row.try_get::<Option<Uuid>,_>("branch_id")?; app::ensure_scope(&caller,branch)?; if row.try_get::<String,_>("status")? != "OPEN" { return Err(AttendanceStoreError::Conflict); }
            let kind = ExceptionKind::parse(&row.try_get::<String,_>("kind")?).map_err(app::AttendanceApplicationError::from)?;
            command.action.validate_for(kind, command.linked_work_ref.as_deref(), command.overtime_minutes).map_err(app::AttendanceApplicationError::from)?;
            let overtime_hours = command.overtime_minutes.map(|m| format!("{:.2}", f64::from(m) / 60.0));
            let inserted=sqlx::query("INSERT INTO attendance_exception_resolutions (id,org_id,exception_id,action,reason,linked_work_ref,ot_hours,actor_user_id) VALUES ($1,$2,$3,$4,$5,$6,$7::numeric,$8) ON CONFLICT (org_id,exception_id) DO NOTHING").bind(Uuid::new_v4()).bind(caller.org_id).bind(command.exception_id).bind(command.action.as_db()).bind(&reason).bind(&command.linked_work_ref).bind(overtime_hours).bind(actor).execute(tx.as_mut()).await?;
            if inserted.rows_affected()!=1 { return Err(AttendanceStoreError::Conflict); }
            sqlx::query("UPDATE attendance_exceptions SET status='RESOLVED' WHERE id=$1 AND status='OPEN'").bind(command.exception_id).execute(tx.as_mut()).await?;
            let view=exception_by_id(tx,command.exception_id).await?; Ok((view,vec![event(&caller,"attendance.exception.resolve","attendance_exception",command.exception_id,branch,Some(json!({"action":command.action})))?]))
        })).await
    }

    pub async fn assign_substitute(
        &self,
        caller: &CallerScope,
        command: AssignSubstitute,
    ) -> Result<AttendanceSubstitutionRead, AttendanceStoreError> {
        let key = app::validate_idempotency_key(&command.idempotency_key)?;
        let mut command = command;
        command.site = app::validate_text(&command.site, "site", 120)?;
        command.role = app::validate_text(&command.role, "role", 120)?;
        command.reason_kind = app::validate_text(&command.reason_kind, "reasonKind", 60)?;
        command.reason_detail =
            app::normalize_optional_text(command.reason_detail, "reasonDetail", 500)?;
        let org = OrgId::from_uuid(caller.org_id);
        let caller = caller.clone();
        let id = Uuid::new_v4();
        with_audits::<_, _, AttendanceStoreError>(&self.pool,org,move|tx|Box::pin(async move {
            idempotency_lock(tx, caller.org_id, &key).await?;
            // The idempotency contract intentionally excludes the display and
            // compensation snapshot: both are derived from HR only for a new
            // assignment and may legitimately change after it was created.
            let fp = fingerprint(&substitution_fingerprint(&caller, &command));
            let existing = sqlx::query_as::<_, (Uuid, String, Option<Uuid>, Option<Uuid>, String, String, Option<String>)>(
                "SELECT id,request_fingerprint,branch_id,worker_employee_id,worker_name,worker_type,worker_rate FROM attendance_substitutions WHERE idempotency_key=$1",
            )
            .bind(&key)
            .fetch_optional(tx.as_mut())
            .await?;
            if let Some((existing, stored, existing_branch, worker_employee_id, worker_name, worker_type, worker_rate)) = existing {
                app::ensure_scope(&caller, existing_branch)?;
                let legacy_fp = fingerprint(&legacy_v1_substitution_fingerprint(
                    &caller,
                    &command,
                    worker_employee_id,
                    &worker_name,
                    &worker_type,
                    worker_rate.as_deref(),
                ));
                if stored == fp || stored == legacy_fp {
                    return Ok((substitution_by_id(tx, existing).await?, Vec::new()));
                }
                return Err(AttendanceStoreError::Conflict);
            }
            let branch = active_employee_branch(tx, command.covered_employee_id).await?;
            if command.branch_id != Some(branch) { return Err(AttendanceStoreError::Conflict); }
            let worker_employee_id = app::require_worker_employee_id(command.worker_employee_id)?;
            app::ensure_scope(&caller, Some(branch))?;
            command.branch_id = Some(branch);
            substitution_eligibility_lock(
                tx,
                caller.org_id,
                worker_employee_id,
                command.window.cover_date,
            )
            .await?;
            let worker = worker_snapshot_and_eligibility(tx, worker_employee_id, &command.window).await?;
            if !worker.facts.is_eligible_for(branch, command.covered_employee_id) {
                return Err(AttendanceStoreError::Conflict);
            }
            // Employee identity comes from canonical HR facts in this transaction,
            // never from the client-supplied display snapshot.
            command.worker_employee_id = Some(worker_employee_id);
            command.worker_name = worker.employee_name;
            command.worker_type = worker.employment_type;
            // `base_pay` is canonical HR compensation but does not carry the
            // amount/currency/unit semantics required by `worker_rate`.
            // Never reinterpret the client rate or HR base pay as one.
            command.worker_rate = None;
            ensure_historical_coverage(tx,command.covered_employee_id,&command.window,command.exception_id).await?;
            sqlx::query("INSERT INTO attendance_substitutions (id,org_id,site,branch_id,role,cover_date,from_minutes,to_minutes,covered_employee_id,reason_kind,reason_detail,worker_employee_id,worker_name,worker_type,worker_rate,exception_id,idempotency_key,request_fingerprint,created_by) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)")
                .bind(id).bind(caller.org_id).bind(&command.site).bind(command.branch_id).bind(&command.role).bind(command.window.cover_date).bind(command.window.from_minutes).bind(command.window.to_minutes).bind(command.covered_employee_id).bind(&command.reason_kind).bind(&command.reason_detail).bind(command.worker_employee_id).bind(&command.worker_name).bind(&command.worker_type).bind(&command.worker_rate).bind(command.exception_id).bind(&key).bind(&fp).bind(caller.user_id).execute(tx.as_mut()).await.map_err(eligibility_guard_conflict)?;
            let view=substitution_by_id(tx,id).await?; Ok((view,vec![event(&caller,"attendance.substitution.assign","attendance_substitution",id,command.branch_id,Some(json!({"coveredEmployeeId":command.covered_employee_id})))?]))
        })).await
    }

    pub async fn substitution_branch(
        &self,
        org_id: Uuid,
        substitution_id: Uuid,
    ) -> Result<Option<Option<Uuid>>, AttendanceStoreError> {
        with_org_conn::<_, _, AttendanceStoreError>(
            &self.pool,
            OrgId::from_uuid(org_id),
            move |tx| {
                Box::pin(async move {
                    sqlx::query_scalar("SELECT branch_id FROM attendance_substitutions WHERE id=$1")
                        .bind(substitution_id)
                        .fetch_optional(tx.as_mut())
                        .await
                        .map_err(AttendanceStoreError::from)
                })
            },
        )
        .await
    }

    pub async fn cancel_substitution(
        &self,
        caller: &CallerScope,
        command: CancelSubstitution,
    ) -> Result<AttendanceSubstitutionRead, AttendanceStoreError> {
        let reason = app::validate_text(&command.reason, "reason", 2000)?;
        let org = OrgId::from_uuid(caller.org_id);
        let caller = caller.clone();
        with_audits::<_, _, AttendanceStoreError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let branch: Option<Uuid> = sqlx::query_scalar(
                    "SELECT branch_id FROM attendance_substitutions WHERE id=$1 FOR UPDATE",
                )
                .bind(command.substitution_id)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or(AttendanceStoreError::NotFound)?;
                app::ensure_scope(&caller, branch)?;
                let changed = sqlx::query(CANCEL_SUBSTITUTION_SQL)
                    .bind(&reason)
                    .bind(command.substitution_id)
                    .execute(tx.as_mut())
                    .await?;
                if changed.rows_affected() != 1 {
                    return Err(AttendanceStoreError::Conflict);
                }
                Ok((
                    substitution_by_id(tx, command.substitution_id).await?,
                    vec![event(
                        &caller,
                        "attendance.substitution.cancel",
                        "attendance_substitution",
                        command.substitution_id,
                        branch,
                        Some(json!({"reason":reason})),
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn close_checks(
        &self,
        caller: &CallerScope,
        close: &CloseMonth,
    ) -> Result<ClosePreflightRead, AttendanceStoreError> {
        app::ensure_scope(caller, close.branch_scope)?;
        let range = AttendanceDateRange::selected_month_with_buffer(&close.month)
            .map_err(app::AttendanceApplicationError::from)?;
        let branch_scope = close.branch_scope;
        let org = OrgId::from_uuid(caller.org_id);
        with_org_conn::<_, _, AttendanceStoreError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let checks = close_checks(tx, range.from, branch_scope).await?;
                Ok(ClosePreflightRead {
                    month: range.from,
                    branch_id: branch_scope,
                    checks: close_checks_read(&checks),
                    can_close: checks.ready(),
                })
            })
        })
        .await
    }
    pub async fn close_month(
        &self,
        caller: &CallerScope,
        close: CloseMonth,
    ) -> Result<MonthCloseRead, AttendanceStoreError> {
        if !close.attest {
            return Err(AttendanceStoreError::Application(
                app::AttendanceApplicationError::MissingAttestation,
            ));
        }
        app::ensure_scope(caller, close.branch_scope)?;
        let month = AttendanceDateRange::selected_month_with_buffer(&close.month)
            .map_err(app::AttendanceApplicationError::from)?
            .from;
        let next = month_after(month)?;
        let last = next - Duration::days(1);
        let org = OrgId::from_uuid(caller.org_id);
        let caller = caller.clone();
        let id = Uuid::new_v4();
        with_audits::<_, _, AttendanceStoreError>(&self.pool, org, move |tx| Box::pin(async move {
            close_month_lock(tx, caller.org_id, month).await?;
            let checks = close_checks(tx, month, close.branch_scope).await?;
            if !checks.ready() { return Err(AttendanceStoreError::CloseBlocked); }
            let checks_json = json!({
                "open_exceptions": checks.open_exceptions,
                "pending_leave": checks.pending_leave,
                "already_closed": checks.already_closed
            });
            // A branch close is an attendance evidence snapshot only. It must
            // never acquire or create the organization-wide payroll period lock.
            // Organization-wide closes reuse or create that lock atomically.
            let (lock_id, created_period_lock) = if close.branch_scope.is_some() {
                (None, false)
            } else {
                // Serialize the payroll period-lock CREATE with the drain's
                // gated staging write: both take this per-org advisory lock, so
                // a lock cannot commit between the staging INSERT's snapshot and
                // its commit (the READ COMMITTED gap the NOT EXISTS alone cannot
                // close).
                console_platform_db::lock_period_lock_key(
                    tx,
                    console_platform_db::PeriodLockDomain::Payroll,
                    caller.org_id,
                )
                .await?;
                let existing: Option<Uuid> = sqlx::query_scalar(
                    "SELECT id FROM period_locks WHERE domain='payroll' AND unlocked_at IS NULL AND period_start <= $1 AND period_end >= $2 ORDER BY locked_at DESC LIMIT 1",
                ).bind(month).bind(last).fetch_optional(tx.as_mut()).await?;
                match existing {
                    Some(id) => (Some(id), false),
                    None => {
                        let overlaps: i64 = sqlx::query_scalar(
                            "SELECT count(*) FROM period_locks WHERE domain='payroll' AND unlocked_at IS NULL AND period_start <= $2 AND period_end >= $1",
                        ).bind(month).bind(last).fetch_one(tx.as_mut()).await?;
                        if overlaps != 0 { return Err(AttendanceStoreError::Conflict); }
                        let id = sqlx::query_scalar(
                            "INSERT INTO period_locks (org_id,domain,period_start,period_end,reason,locked_by) VALUES ($1,'payroll',$2,$3,$4,$5) RETURNING id",
                        ).bind(caller.org_id).bind(month).bind(last).bind(format!("attendance close {}", close.month)).bind(caller.user_id).fetch_one(tx.as_mut()).await?;
                        (Some(id), true)
                    }
                }
            };
            let inserted = sqlx::query(
                "INSERT INTO attendance_month_closes (id,org_id,month,branch_id,checks,attested_by,period_lock_id) VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT (org_id,month,branch_id) DO NOTHING",
            ).bind(id).bind(caller.org_id).bind(month).bind(close.branch_scope).bind(&checks_json).bind(caller.user_id).bind(lock_id).execute(tx.as_mut()).await?;
            if inserted.rows_affected() != 1 { return Err(AttendanceStoreError::Conflict); }
            let view = MonthCloseRead { id, month, branch_id: close.branch_scope, checks: close_checks_read(&checks), attested_by: caller.user_id, attested_at: OffsetDateTime::now_utc(), period_lock_id: lock_id, closed_at: OffsetDateTime::now_utc(), amendments: Vec::new() };
            let mut audits = vec![event(&caller,"attendance.close.confirm","attendance_month_close",id,close.branch_scope,Some(json!({"periodLockId":lock_id})))?];
            if let Some(audit) = created_period_lock_audit(
                &caller,
                created_period_lock,
                lock_id,
                month,
                last,
            )? {
                audits.push(audit);
            }
            Ok((view, audits))
        })).await
    }

    pub async fn list_closes(
        &self,
        caller: &CallerScope,
        branch_id: Option<Uuid>,
        month: Date,
    ) -> Result<Vec<MonthCloseRead>, AttendanceStoreError> {
        app::ensure_scope(caller, branch_id)?;
        let org = OrgId::from_uuid(caller.org_id);
        with_org_conn::<_, _, AttendanceStoreError>(&self.pool, org, move |tx| Box::pin(async move {
            let rows = sqlx::query("SELECT id,month,branch_id,checks,attested_by,attested_at,period_lock_id,closed_at FROM attendance_month_closes WHERE branch_id IS NOT DISTINCT FROM $1 AND month=$2 ORDER BY month DESC").bind(branch_id).bind(month).fetch_all(tx.as_mut()).await?;
            let mut result=Vec::with_capacity(rows.len()); for row in &rows { result.push(close_read(tx,row).await?); } Ok(result)
        })).await
    }

    pub async fn close_branch(
        &self,
        org_id: Uuid,
        close_id: Uuid,
    ) -> Result<Option<Option<Uuid>>, AttendanceStoreError> {
        with_org_conn::<_, _, AttendanceStoreError>(
            &self.pool,
            OrgId::from_uuid(org_id),
            move |tx| {
                Box::pin(async move {
                    sqlx::query_scalar("SELECT branch_id FROM attendance_month_closes WHERE id=$1")
                        .bind(close_id)
                        .fetch_optional(tx.as_mut())
                        .await
                        .map_err(AttendanceStoreError::from)
                })
            },
        )
        .await
    }

    pub async fn amend_close(
        &self,
        caller: &CallerScope,
        command: AmendClose,
    ) -> Result<CloseAmendmentRead, AttendanceStoreError> {
        let reason = app::validate_text(&command.reason, "reason", 2000)?;
        let detail = app::validate_text(&command.detail, "detail", 4000)?;
        let reference = app::normalize_optional_text(command.reference, "ref", 240)?;
        let key = app::validate_idempotency_key(&command.idempotency_key)?;
        let org = OrgId::from_uuid(caller.org_id);
        let caller = caller.clone();
        with_audits::<_, _, AttendanceStoreError>(&self.pool, org, move |tx| Box::pin(async move {
            idempotency_lock(tx, caller.org_id, &key).await?;
            let branch: Option<Uuid> = sqlx::query_scalar("SELECT branch_id FROM attendance_month_closes WHERE id=$1").bind(command.close_id).fetch_optional(tx.as_mut()).await?.ok_or(AttendanceStoreError::NotFound)?;
            app::ensure_scope(&caller, branch)?;
            let fingerprint_value = fingerprint(&json!({"close_id":command.close_id,"reason":reason,"detail":detail,"ref":reference}));
            let existing: Option<(Uuid, String)> = sqlx::query_as("SELECT id,request_fingerprint FROM attendance_close_amendments WHERE org_id=$1 AND idempotency_key=$2").bind(caller.org_id).bind(&key).fetch_optional(tx.as_mut()).await?;
            if let Some((id, stored)) = existing { if stored == fingerprint_value { return Ok((close_amendment_read(tx, id).await?, Vec::new())); } return Err(AttendanceStoreError::Conflict); }
            let amendment_id = Uuid::new_v4();
            sqlx::query("INSERT INTO attendance_close_amendments (id,org_id,close_id,reason,detail,ref,idempotency_key,request_fingerprint,actor_user_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(amendment_id).bind(caller.org_id).bind(command.close_id).bind(&reason).bind(&detail).bind(&reference).bind(&key).bind(&fingerprint_value).bind(caller.user_id).execute(tx.as_mut()).await?;
            Ok((close_amendment_read(tx, amendment_id).await?, vec![event(&caller,"attendance.close.amend","attendance_month_close",command.close_id,branch,Some(json!({"amendmentId":amendment_id})))?]))
        })).await
    }

    /// Resolves an active employee's server-owned home branch under the tenant
    /// connection. `None` deliberately covers cross-tenant, inactive, and
    /// branchless employees without leaking which condition applied.
    pub async fn active_employee_home_branch(
        &self,
        org_id: Uuid,
        employee_id: Uuid,
    ) -> Result<Option<Uuid>, AttendanceStoreError> {
        let branch: Option<Option<Uuid>> = with_org_conn::<_, _, AttendanceStoreError>(&self.pool, OrgId::from_uuid(org_id), move |tx| Box::pin(async move {
            sqlx::query_scalar("SELECT home_branch_id FROM employees WHERE id=$1 AND employment_status='ACTIVE' AND home_branch_id IS NOT NULL")
                .bind(employee_id).fetch_optional(tx.as_mut()).await.map_err(AttendanceStoreError::from)
        })).await?;
        branch
            .flatten()
            .ok_or(AttendanceStoreError::NotFound)
            .map(Some)
    }

    pub async fn acknowledge_week52(
        &self,
        caller: &CallerScope,
        command: AcknowledgeWeek52,
    ) -> Result<Week52AcknowledgementRead, AttendanceStoreError> {
        app::validate_week52_start(command.week_start)?;
        let org = OrgId::from_uuid(caller.org_id);
        let caller = caller.clone();
        with_audits::<_, _, AttendanceStoreError>(&self.pool, org, move |tx| Box::pin(async move {
            let branch: Option<Uuid> = sqlx::query_scalar("SELECT home_branch_id FROM employees WHERE id=$1 AND employment_status='ACTIVE' AND home_branch_id IS NOT NULL")
                .bind(command.employee_id).fetch_optional(tx.as_mut()).await?.flatten();
            let branch = branch.ok_or(AttendanceStoreError::NotFound)?;
            app::ensure_scope(&caller, Some(branch))?;
            let inserted: Option<OffsetDateTime> = sqlx::query_scalar("INSERT INTO attendance_week52_acknowledgements (org_id,employee_id,week_start,acknowledged_by_user_id) VALUES ($1,$2,$3,$4) ON CONFLICT (org_id,employee_id,week_start) DO NOTHING RETURNING acknowledged_at")
                .bind(caller.org_id).bind(command.employee_id).bind(command.week_start).bind(caller.user_id).fetch_optional(tx.as_mut()).await?;
            let acknowledged_at = match inserted {
                Some(value) => value,
                None => sqlx::query_scalar("SELECT acknowledged_at FROM attendance_week52_acknowledgements WHERE employee_id=$1 AND week_start=$2")
                    .bind(command.employee_id).bind(command.week_start).fetch_optional(tx.as_mut()).await?.ok_or(AttendanceStoreError::NotFound)?,
            };
            week52_acknowledgement_response(
                &caller,
                command.employee_id,
                command.week_start,
                acknowledged_at,
                inserted.is_some(),
                branch,
            )
        })).await
    }

    pub async fn week52_inputs(
        &self,
        caller: &CallerScope,
        week_start: Date,
        branch_id: Option<Uuid>,
    ) -> Result<Vec<Week52Read>, AttendanceStoreError> {
        app::ensure_scope(caller, branch_id)?;
        app::validate_week52_start(week_start)?;
        let org = OrgId::from_uuid(caller.org_id);
        let end = week_start + Duration::days(7);
        let week_start_at = week52_boundary(week_start)?;
        let week_end_at = week52_boundary(end)?;
        with_org_conn::<_, _, AttendanceStoreError>(&self.pool, org, move |tx| Box::pin(async move {
            let active_employees = sqlx::query_as::<_, (Uuid, String, Option<String>)>("SELECT id,name,org_unit FROM employees WHERE employment_status='ACTIVE' AND ($1::uuid IS NULL OR home_branch_id=$1) ORDER BY id")
                .bind(branch_id).fetch_all(tx.as_mut()).await?;
            // Pair the complete employee timeline. Filtering by `work_date` (or by either
            // week boundary) loses a side of a shift that crosses that boundary.
            let rows = sqlx::query("SELECT r.id,r.employee_id,r.kind,r.occurred_at FROM employee_attendance_records r JOIN employees e ON e.id=r.employee_id AND e.org_id=r.org_id WHERE e.employment_status='ACTIVE' AND ($1::uuid IS NULL OR e.home_branch_id=$1) ORDER BY r.employee_id,r.occurred_at,r.id")
                .bind(branch_id).fetch_all(tx.as_mut()).await?;
            let events = rows.iter().map(|row| Ok(StrictDurationEvent {
                id: row.try_get("id")?,
                employee_id: row.try_get("employee_id")?,
                kind: strict_duration_event_kind(&row.try_get::<String, _>("kind")?),
                occurred_at: row.try_get("occurred_at")?,
            })).collect::<Result<Vec<_>, AttendanceStoreError>>()?;
            let hours = week52_hours(&events, week_start_at, week_end_at)?;
            let acknowledgements = sqlx::query("SELECT a.employee_id,a.acknowledged_at FROM attendance_week52_acknowledgements a JOIN employees e ON e.id=a.employee_id AND e.org_id=a.org_id WHERE a.week_start=$1 AND e.employment_status='ACTIVE' AND ($2::uuid IS NULL OR e.home_branch_id=$2)")
                .bind(week_start).bind(branch_id).fetch_all(tx.as_mut()).await?.into_iter().map(|row| Ok((row.try_get::<Uuid,_>("employee_id")?, row.try_get::<OffsetDateTime,_>("acknowledged_at")?))).collect::<Result<BTreeMap<_,_>, AttendanceStoreError>>()?;
            Ok(week52_inputs_for_active(active_employees, week_start, hours, acknowledgements))
        })).await
    }
}

fn week52_hours(
    events: &[StrictDurationEvent],
    week_start: OffsetDateTime,
    week_end: OffsetDateTime,
) -> Result<BTreeMap<Uuid, f64>, AttendanceStoreError> {
    let window = StrictDurationWindow::new(week_start, week_end)
        .map_err(|_| AttendanceStoreError::Conflict)?;
    Ok(strict_pair_seconds(events, window)
        .map_err(|_| AttendanceStoreError::Conflict)?
        .into_iter()
        .map(|(employee, seconds)| (employee, seconds as f64 / 3600.0))
        .collect())
}

fn strict_duration_event_kind(kind: &str) -> StrictDurationEventKind {
    match kind {
        "CLOCK_IN" => StrictDurationEventKind::ClockIn,
        "CLOCK_OUT" => StrictDurationEventKind::ClockOut,
        _ => StrictDurationEventKind::Other,
    }
}

fn week52_inputs_for_active(
    active_employees: impl IntoIterator<Item = (Uuid, String, Option<String>)>,
    week_start: Date,
    hours: BTreeMap<Uuid, f64>,
    acknowledgements: BTreeMap<Uuid, OffsetDateTime>,
) -> Vec<Week52Read> {
    active_employees
        .into_iter()
        .map(|(employee_id, name, team)| {
            let current_hours = hours.get(&employee_id).copied().unwrap_or(0.0);
            Week52Read {
                employee_id,
                name,
                team,
                week_start,
                current_hours,
                projected_hours: current_hours,
                acknowledged_at: acknowledgements.get(&employee_id).copied(),
            }
        })
        .collect()
}

fn week52_boundary(date: Date) -> Result<OffsetDateTime, AttendanceStoreError> {
    let local_midnight = date
        .with_hms(0, 0, 0)
        .map_err(|_| AttendanceStoreError::Conflict)?;
    let seoul = UtcOffset::from_hms(9, 0, 0).map_err(|_| AttendanceStoreError::Conflict)?;
    Ok(local_midnight.assume_offset(seoul))
}

fn week52_acknowledgement_response(
    caller: &CallerScope,
    employee_id: Uuid,
    week_start: Date,
    acknowledged_at: OffsetDateTime,
    inserted: bool,
    branch: Uuid,
) -> Result<(Week52AcknowledgementRead, Vec<AuditEvent>), AttendanceStoreError> {
    let audits = if inserted {
        vec![event(
            caller,
            "attendance.week52.acknowledge",
            "attendance_week52",
            employee_id,
            Some(branch),
            Some(json!({"weekStart":week_start,"acknowledgedAt":acknowledged_at})),
        )?]
    } else {
        Vec::new()
    };
    Ok((
        Week52AcknowledgementRead {
            employee_id,
            week_start,
            acknowledged_at,
        },
        audits,
    ))
}

fn month_after(month: Date) -> Result<Date, AttendanceStoreError> {
    if month.month() == time::Month::December {
        let year = month
            .year()
            .checked_add(1)
            .ok_or(AttendanceStoreError::InvalidCloseMonth)?;
        Date::from_calendar_date(year, time::Month::January, 1)
            .map_err(|_| AttendanceStoreError::InvalidCloseMonth)
    } else {
        Date::from_calendar_date(month.year(), month.month().next(), 1)
            .map_err(|_| AttendanceStoreError::InvalidCloseMonth)
    }
}

fn created_period_lock_audit(
    caller: &CallerScope,
    created_period_lock: bool,
    lock_id: Option<Uuid>,
    month: Date,
    last: Date,
) -> Result<Option<AuditEvent>, AttendanceStoreError> {
    if !created_period_lock {
        return Ok(None);
    }
    let lock_id = lock_id.ok_or(AttendanceStoreError::Conflict)?;
    Ok(Some(event(
        caller,
        "period_lock.create",
        "period_lock",
        lock_id,
        None,
        Some(json!({"domain":"payroll","periodStart":month,"periodEnd":last})),
    )?))
}

fn fingerprint(value: &Value) -> String {
    hex::encode(Sha256::digest(value.to_string()))
}
fn event(
    caller: &CallerScope,
    action: &str,
    kind: &str,
    id: Uuid,
    branch: Option<Uuid>,
    after: Option<Value>,
) -> Result<AuditEvent, AttendanceStoreError> {
    let mut e = AuditEvent::new(
        Some(console_kernel_core::UserId::from_uuid(caller.user_id)),
        AuditAction::new(action).map_err(|_| {
            AttendanceStoreError::Application(app::AttendanceApplicationError::InvalidText(
                "audit action",
            ))
        })?,
        kind,
        id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(OrgId::from_uuid(caller.org_id))
    .with_snapshots(None, after);
    if let Some(branch) = branch {
        e = e.with_branch(BranchId::from_uuid(branch));
    }
    Ok(e)
}
async fn ensure_historical_coverage(
    tx: &mut Transaction<'_, Postgres>,
    employee: Uuid,
    window: &console_attendance_domain::SubstitutionWindow,
    _exception_id: Option<Uuid>,
) -> Result<(), AttendanceStoreError> {
    if window.cover_date
        >= OffsetDateTime::now_utc()
            .to_offset(UtcOffset::from_hms(9, 0, 0).map_err(|_| AttendanceStoreError::Conflict)?)
            .date()
    {
        return Ok(());
    }
    // Historical substitution admits only an approved leave intent that fully covers
    // the requested window; unknown/legacy intent is deliberately not coverage.
    let row = sqlx::query(HISTORICAL_LEAVE_COVERAGE_SQL)
        .bind(employee)
        .bind(window.cover_date)
        .fetch_optional(tx.as_mut())
        .await?;
    let Some(row) = row else {
        return Err(AttendanceStoreError::Conflict);
    };
    let leave_type: String = row.try_get("leave_type")?;
    let partial_day_period: Option<String> = row.try_get("partial_day_period")?;
    let (from_minutes, to_minutes) =
        leave_coverage_window(&leave_type, partial_day_period.as_deref())
            .ok_or(AttendanceStoreError::Conflict)?;
    let absence = HistoricalAbsence::new(
        row.try_get("employee_id")?,
        window.cover_date,
        from_minutes,
        to_minutes,
    )
    .map_err(app::AttendanceApplicationError::from)?;
    if absence.fully_covers(employee, window) {
        Ok(())
    } else {
        Err(AttendanceStoreError::Conflict)
    }
}

async fn active_employee_branch(
    tx: &mut Transaction<'_, Postgres>,
    employee_id: Uuid,
) -> Result<Uuid, AttendanceStoreError> {
    sqlx::query_scalar("SELECT home_branch_id FROM employees WHERE id=$1 AND employment_status='ACTIVE' AND home_branch_id IS NOT NULL")
        .bind(employee_id)
        .fetch_optional(tx.as_mut())
        .await?
        .flatten()
        .ok_or(AttendanceStoreError::NotFound)
}

struct WorkerSnapshot {
    employee_name: String,
    employment_type: String,
    facts: SubstitutionCandidateFacts,
}

async fn worker_snapshot_and_eligibility(
    tx: &mut Transaction<'_, Postgres>,
    worker_employee_id: Uuid,
    window: &console_attendance_domain::SubstitutionWindow,
) -> Result<WorkerSnapshot, AttendanceStoreError> {
    // Lock identity first; eligibility must be a separate, fresh statement
    // after the platform-owned employee/day lock has been acquired.
    sqlx::query("SELECT id FROM employees WHERE id=$1 FOR UPDATE")
        .bind(worker_employee_id)
        .fetch_optional(tx.as_mut())
        .await?
        .ok_or(AttendanceStoreError::NotFound)?;
    let row = sqlx::query(
        "SELECT e.id,e.name,e.home_branch_id,e.employment_status='ACTIVE' AS employment_active,p.employment_type, EXISTS (SELECT 1 FROM attendance_substitutions s WHERE s.worker_employee_id=e.id AND s.status='ASSIGNED' AND s.cover_date=$2 AND s.from_minutes < $4 AND s.to_minutes > $3) AS conflicts_with_assigned_substitution, EXISTS (SELECT 1 FROM leave_requests l WHERE l.subject_employee_id=e.id AND l.status IN ('approved','APPROVED') AND l.start_date <= $2 AND l.end_date >= $2 AND (l.leave_type='annual' OR (l.leave_type='half_day' AND ((l.partial_day_period='am' AND $3 < 720) OR (l.partial_day_period='pm' AND $4 > 720))))) AS approved_leave_covers_window, EXISTS (SELECT 1 FROM attendance_exceptions x WHERE x.employee_id=e.id AND x.kind='NO_SHOW' AND x.status='OPEN' AND x.work_date=$2) AS has_open_no_show FROM employees e JOIN employee_employment_profiles p ON p.employee_id=e.id AND p.org_id=e.org_id WHERE e.id=$1",
    )
    .bind(worker_employee_id)
    .bind(window.cover_date)
    .bind(window.from_minutes)
    .bind(window.to_minutes)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or(AttendanceStoreError::Conflict)?;
    Ok(WorkerSnapshot {
        employee_name: row.try_get("name")?,
        employment_type: row.try_get("employment_type")?,
        facts: SubstitutionCandidateFacts {
            employee_id: row.try_get("id")?,
            employment_active: row.try_get("employment_active")?,
            home_branch_id: row.try_get("home_branch_id")?,
            conflicts_with_assigned_substitution: row
                .try_get("conflicts_with_assigned_substitution")?,
            approved_leave_covers_window: row.try_get("approved_leave_covers_window")?,
            has_open_no_show: row.try_get("has_open_no_show")?,
        },
    })
}

fn candidate_read(
    row: &sqlx::postgres::PgRow,
    branch_id: Uuid,
    covered_employee_id: Uuid,
) -> Result<Option<SubstitutionCandidateRead>, AttendanceStoreError> {
    let facts = SubstitutionCandidateFacts {
        employee_id: row.try_get("id")?,
        employment_active: row.try_get("employment_active")?,
        home_branch_id: row.try_get("home_branch_id")?,
        conflicts_with_assigned_substitution: row
            .try_get("conflicts_with_assigned_substitution")?,
        approved_leave_covers_window: row.try_get("approved_leave_covers_window")?,
        has_open_no_show: row.try_get("has_open_no_show")?,
    };
    let employee_name: String = row.try_get("name")?;
    Ok(facts
        .is_eligible_for(branch_id, covered_employee_id)
        .then_some(SubstitutionCandidateRead {
            employee_id: facts.employee_id,
            employee_name,
            branch_id,
        }))
}

const HISTORICAL_LEAVE_COVERAGE_SQL: &str = "SELECT subject_employee_id AS employee_id, leave_type, partial_day_period FROM leave_requests WHERE subject_employee_id=$1 AND status IN ('approved','APPROVED') AND start_date <= $2 AND end_date >= $2 ORDER BY start_date DESC LIMIT 1";

fn leave_coverage_window(leave_type: &str, partial_day_period: Option<&str>) -> Option<(i32, i32)> {
    match (leave_type, partial_day_period) {
        ("annual", None) => Some((0, 1440)),
        ("half_day", Some("am")) => Some((0, 720)),
        ("half_day", Some("pm")) => Some((720, 1440)),
        _ => None,
    }
}

async fn close_month_lock(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    month: Date,
) -> Result<(), AttendanceStoreError> {
    let material = format!("attendance-close-v1|{}|{}", org_id, month);
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(material)
        .execute(tx.as_mut())
        .await?;
    Ok(())
}

async fn idempotency_lock(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    normalized_key: &str,
) -> Result<(), AttendanceStoreError> {
    let material = format!(
        "attendance-idempotency-v1|{}|{}|{}",
        org_id,
        normalized_key.len(),
        normalized_key
    );
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(material)
        .execute(tx.as_mut())
        .await?;
    Ok(())
}

/// The platform-owned employee/day lock serializes this fresh eligibility read with every
/// cross-domain employee/day writer (leave and NO_SHOW transitions included).
async fn substitution_eligibility_lock(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    worker_employee_id: Uuid,
    cover_date: Date,
) -> Result<(), AttendanceStoreError> {
    sqlx::query(SUBSTITUTION_ELIGIBILITY_LOCK_SQL)
        .bind(org_id)
        .bind(worker_employee_id)
        .bind(cover_date)
        .execute(tx.as_mut())
        .await?;
    Ok(())
}

/// The migration-owned guard is a normal optimistic-race outcome. Other
/// database failures retain their original error surface for observability.
fn eligibility_guard_conflict(error: sqlx::Error) -> AttendanceStoreError {
    if let sqlx::Error::Database(database) = &error
        && database.code().as_deref() == Some("23514")
        && database.message() == "attendance_substitutions_worker_eligibility_guard"
    {
        return AttendanceStoreError::Conflict;
    }
    AttendanceStoreError::Sql(error)
}

fn substitution_fingerprint(caller: &CallerScope, command: &AssignSubstitute) -> Value {
    // This is the immutable client request contract. HR-derived display and
    // compensation values are persisted snapshots, never replay inputs.
    json!({"v":2,"orgId":caller.org_id,"branchPresent":command.branch_id.is_some(),"branchId":command.branch_id,"coverDate":command.window.cover_date,"from":command.window.from_minutes,"to":command.window.to_minutes,"coveredEmployeeId":command.covered_employee_id,"reasonKind":command.reason_kind,"reasonDetailPresent":command.reason_detail.is_some(),"reasonDetail":command.reason_detail,"site":command.site,"role":command.role,"workerEmployeePresent":command.worker_employee_id.is_some(),"workerEmployeeId":command.worker_employee_id,"exceptionPresent":command.exception_id.is_some(),"exceptionId":command.exception_id})
}

/// Exact historical v1 hash input for rows committed before the immutable v2
/// contract. Its snapshot fields come from the persisted row, never today's HR
/// profile, so profile/name changes cannot break a legitimate old replay.
fn legacy_v1_substitution_fingerprint(
    caller: &CallerScope,
    command: &AssignSubstitute,
    worker_employee_id: Option<Uuid>,
    worker_name: &str,
    worker_type: &str,
    worker_rate: Option<&str>,
) -> Value {
    json!({"v":1,"orgId":caller.org_id,"branchPresent":command.branch_id.is_some(),"branchId":command.branch_id,"coverDate":command.window.cover_date,"from":command.window.from_minutes,"to":command.window.to_minutes,"coveredEmployeeId":command.covered_employee_id,"reasonKind":command.reason_kind,"reasonDetailPresent":command.reason_detail.is_some(),"reasonDetail":command.reason_detail,"site":command.site,"role":command.role,"workerEmployeePresent":worker_employee_id.is_some(),"workerEmployeeId":worker_employee_id,"workerName":worker_name,"workerType":worker_type,"workerRatePresent":worker_rate.is_some(),"workerRate":worker_rate,"exceptionPresent":command.exception_id.is_some(),"exceptionId":command.exception_id})
}

async fn close_checks(
    tx: &mut Transaction<'_, Postgres>,
    month: Date,
    branch: Option<Uuid>,
) -> Result<CloseChecks, AttendanceStoreError> {
    let next = month_after(month)?;
    let open:i64=sqlx::query_scalar("SELECT count(*) FROM attendance_exceptions WHERE status='OPEN' AND work_date >= $1 AND work_date < $2 AND ($3::uuid IS NULL OR branch_id=$3)").bind(month).bind(next).bind(branch).fetch_one(tx.as_mut()).await?;
    let pending:i64=sqlx::query_scalar("SELECT count(*) FROM leave_requests WHERE status IN ('pending','PENDING') AND start_date < $2 AND end_date >= $1 AND ($3::uuid IS NULL OR branch_id=$3)").bind(month).bind(next).bind(branch).fetch_one(tx.as_mut()).await?;
    let closed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM attendance_month_closes WHERE month=$1 AND branch_id IS NOT DISTINCT FROM $2)",
    )
    .bind(month)
    .bind(branch)
    .fetch_one(tx.as_mut())
    .await?;
    Ok(CloseChecks {
        open_exceptions: open,
        pending_leave: pending,
        already_closed: closed,
    })
}
async fn exception_by_id(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<AttendanceExceptionRead, AttendanceStoreError> {
    let r = sqlx::query(EXCEPTION_BY_ID_SQL)
        .bind(id)
        .fetch_one(tx.as_mut())
        .await?;
    exception_read(&r)
}
fn exception_read(
    r: &sqlx::postgres::PgRow,
) -> Result<AttendanceExceptionRead, AttendanceStoreError> {
    let evidence: Vec<AttendanceEvidence> = serde_json::from_value(r.try_get("evidence")?)
        .map_err(|_| AttendanceStoreError::Conflict)?;
    let links: Vec<AttendanceObjectLink> =
        serde_json::from_value(r.try_get("links")?).map_err(|_| AttendanceStoreError::Conflict)?;
    let resolution_action: Option<String> = r.try_get("resolution_action")?;
    let resolution = match resolution_action {
        None => None,
        Some(action) => Some(ExceptionResolutionRead {
            action: console_attendance_domain::ResolutionAction::parse(&action)
                .map_err(app::AttendanceApplicationError::from)?,
            reason: r.try_get("resolution_reason")?,
            linked_work_ref: r.try_get("linked_work_ref")?,
            ot_hours: r.try_get("ot_hours")?,
            actor: r.try_get("actor_user_id")?,
            resolved_at: r.try_get("resolved_at")?,
        }),
    };
    Ok(AttendanceExceptionRead {
        id: r.try_get("id")?,
        code: r.try_get("code")?,
        kind: ExceptionKind::parse(&r.try_get::<String, _>("kind")?)
            .map_err(app::AttendanceApplicationError::from)?,
        status: r.try_get("status")?,
        employee_id: r.try_get("employee_id")?,
        employee_name: r.try_get("employee_name")?,
        team: r.try_get("team")?,
        branch_id: r.try_get("branch_id")?,
        work_date: r.try_get("work_date")?,
        occurred_at: r.try_get("occurred_at")?,
        detail: r.try_get("detail")?,
        evidence,
        links,
        resolution,
        created_at: r.try_get("created_at")?,
    })
}
async fn substitution_by_id(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<AttendanceSubstitutionRead, AttendanceStoreError> {
    let r=sqlx::query("SELECT s.id,s.site,s.branch_id,s.role,s.cover_date,s.from_minutes,s.to_minutes,s.covered_employee_id,cov.name AS covered_name,s.reason_kind,s.reason_detail,s.worker_employee_id,s.worker_name,s.worker_type,s.worker_rate,s.status,s.exception_id,s.created_by,s.created_at FROM attendance_substitutions s JOIN employees cov ON cov.id=s.covered_employee_id AND cov.org_id=s.org_id WHERE s.id=$1").bind(id).fetch_one(tx.as_mut()).await?;
    substitution_read(&r)
}
fn substitution_read(
    r: &sqlx::postgres::PgRow,
) -> Result<AttendanceSubstitutionRead, AttendanceStoreError> {
    let status: String = r.try_get("status")?;
    Ok(AttendanceSubstitutionRead {
        id: r.try_get("id")?,
        site: r.try_get("site")?,
        branch_id: r.try_get("branch_id")?,
        role: r.try_get("role")?,
        cover_date: r.try_get("cover_date")?,
        from_minutes: r.try_get("from_minutes")?,
        to_minutes: r.try_get("to_minutes")?,
        covered_employee_id: r.try_get("covered_employee_id")?,
        covered_name: r.try_get("covered_name")?,
        reason_kind: r.try_get("reason_kind")?,
        reason_detail: r.try_get("reason_detail")?,
        worker_employee_id: r.try_get("worker_employee_id")?,
        worker_name: r.try_get("worker_name")?,
        worker_type: r.try_get("worker_type")?,
        worker_rate: r.try_get("worker_rate")?,
        status: match status.as_str() {
            "ASSIGNED" | "CANCELLED" => status,
            _ => return Err(AttendanceStoreError::Conflict),
        },
        exception_id: r.try_get("exception_id")?,
        created_by: r.try_get("created_by")?,
        created_at: r.try_get("created_at")?,
    })
}
async fn close_amendment_read(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<CloseAmendmentRead, AttendanceStoreError> {
    let r = sqlx::query(
        "SELECT id,reason,actor_user_id,created_at FROM attendance_close_amendments WHERE id=$1",
    )
    .bind(id)
    .fetch_one(tx.as_mut())
    .await?;
    Ok(CloseAmendmentRead {
        id: r.try_get("id")?,
        reason: r.try_get("reason")?,
        actor: r.try_get("actor_user_id")?,
        created_at: r.try_get("created_at")?,
    })
}
fn decode_close_checks(value: &Value) -> Result<CloseChecks, AttendanceStoreError> {
    let open_exceptions = value
        .get("open_exceptions")
        .and_then(Value::as_i64)
        .ok_or(AttendanceStoreError::Conflict)?;
    let pending_leave = value
        .get("pending_leave")
        .and_then(Value::as_i64)
        .ok_or(AttendanceStoreError::Conflict)?;
    let already_closed = value
        .get("already_closed")
        .and_then(Value::as_bool)
        .ok_or(AttendanceStoreError::Conflict)?;
    if open_exceptions < 0 || pending_leave < 0 {
        return Err(AttendanceStoreError::Conflict);
    }
    Ok(CloseChecks {
        open_exceptions,
        pending_leave,
        already_closed,
    })
}

fn close_checks_read(checks: &CloseChecks) -> Vec<CloseCheckRead> {
    vec![
        CloseCheckRead {
            key: "open_exceptions".into(),
            ok: checks.open_exceptions == 0,
            warn: None,
            note: Some(checks.open_exceptions.to_string()),
        },
        CloseCheckRead {
            key: "pending_leave".into(),
            ok: true,
            warn: Some(checks.pending_leave > 0),
            note: Some(checks.pending_leave.to_string()),
        },
        CloseCheckRead {
            key: "not_already_closed".into(),
            ok: !checks.already_closed,
            warn: None,
            note: None,
        },
    ]
}
async fn close_read(
    tx: &mut Transaction<'_, Postgres>,
    r: &sqlx::postgres::PgRow,
) -> Result<MonthCloseRead, AttendanceStoreError> {
    let id: Uuid = r.try_get("id")?;
    let rows=sqlx::query("SELECT id,reason,actor_user_id,created_at FROM attendance_close_amendments WHERE close_id=$1 ORDER BY created_at").bind(id).fetch_all(tx.as_mut()).await?;
    let amendments = rows
        .iter()
        .map(|a| {
            Ok(CloseAmendmentRead {
                id: a.try_get("id")?,
                reason: a.try_get("reason")?,
                actor: a.try_get("actor_user_id")?,
                created_at: a.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>, AttendanceStoreError>>()?;
    let checks: Value = r.try_get("checks")?;
    let c = decode_close_checks(&checks)?;
    Ok(MonthCloseRead {
        id,
        month: r.try_get("month")?,
        branch_id: r.try_get("branch_id")?,
        checks: close_checks_read(&c),
        attested_by: r.try_get("attested_by")?,
        attested_at: r.try_get("attested_at")?,
        period_lock_id: r.try_get("period_lock_id")?,
        closed_at: r.try_get("closed_at")?,
        amendments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_attendance_domain::SubstitutionWindow;
    use time::{Date, Month};

    fn command() -> AssignSubstitute {
        AssignSubstitute {
            window: SubstitutionWindow::new(
                Date::from_calendar_date(2026, Month::July, 2).unwrap(),
                540,
                1020,
            )
            .unwrap(),
            branch_id: Some(Uuid::new_v4()),
            site: "Seoul".into(),
            role: "Operator".into(),
            covered_employee_id: Uuid::new_v4(),
            reason_kind: "APPROVED_LEAVE".into(),
            reason_detail: Some("medical".into()),
            worker_employee_id: Some(Uuid::new_v4()),
            worker_name: "Kim".into(),
            worker_type: "EMPLOYEE".into(),
            worker_rate: Some("10000".into()),
            exception_id: Some(Uuid::new_v4()),
            idempotency_key: "attendance-test-key-0001".into(),
        }
    }
    fn scope() -> CallerScope {
        CallerScope {
            org_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            branch_ids: vec![],
            org_wide: true,
        }
    }
    fn canonicalize(command: &mut AssignSubstitute) {
        command.site = app::validate_text(&command.site, "site", 120).unwrap();
        command.role = app::validate_text(&command.role, "role", 120).unwrap();
        command.reason_kind = app::validate_text(&command.reason_kind, "reasonKind", 60).unwrap();
        command.reason_detail =
            app::normalize_optional_text(command.reason_detail.take(), "reasonDetail", 500)
                .unwrap();
        command.worker_name = app::validate_text(&command.worker_name, "workerName", 120).unwrap();
        command.worker_type = app::validate_text(&command.worker_type, "workerType", 60).unwrap();
        command.worker_rate =
            app::normalize_optional_text(command.worker_rate.take(), "workerRate", 60).unwrap();
    }
    fn assert_fingerprint_changes(
        caller: &CallerScope,
        base: &AssignSubstitute,
        changed: AssignSubstitute,
    ) {
        assert_ne!(
            fingerprint(&substitution_fingerprint(caller, base)),
            fingerprint(&substitution_fingerprint(caller, &changed))
        );
    }
    #[test]
    fn close_checks_decoder_rejects_missing_wrong_and_negative_values() {
        assert!(
            decode_close_checks(
                &json!({"open_exceptions": 0, "pending_leave": 0, "already_closed": false})
            )
            .is_ok()
        );
        assert!(decode_close_checks(&json!({"open_exceptions": 0, "pending_leave": 0})).is_err());
        assert!(
            decode_close_checks(
                &json!({"open_exceptions": "0", "pending_leave": 0, "already_closed": false})
            )
            .is_err()
        );
        assert!(
            decode_close_checks(
                &json!({"open_exceptions": -1, "pending_leave": 0, "already_closed": false})
            )
            .is_err()
        );
    }

    #[test]
    fn month_after_advances_december_and_rejects_an_unrepresentable_next_month() {
        let december = Date::from_calendar_date(2026, Month::December, 1).unwrap();
        assert_eq!(
            month_after(december).unwrap(),
            Date::from_calendar_date(2027, Month::January, 1).unwrap()
        );
        assert!(matches!(
            month_after(Date::MAX),
            Err(AttendanceStoreError::InvalidCloseMonth)
        ));
    }

    #[test]
    fn created_period_lock_requires_and_emits_its_audit_receipt() {
        let caller = scope();
        let lock_id = Uuid::new_v4();
        let month = Date::from_calendar_date(2026, Month::July, 1).unwrap();
        let last = Date::from_calendar_date(2026, Month::July, 31).unwrap();

        let audit = created_period_lock_audit(&caller, true, Some(lock_id), month, last)
            .unwrap()
            .expect("created period lock requires an audit receipt");
        assert_eq!(audit.action.as_str(), "period_lock.create");
        assert_eq!(audit.target_type, "period_lock");
        assert_eq!(audit.target_id, lock_id.to_string());
        assert_eq!(
            audit.after,
            Some(json!({"domain":"payroll","periodStart":month,"periodEnd":last}))
        );
        assert!(matches!(
            created_period_lock_audit(&caller, true, None, month, last),
            Err(AttendanceStoreError::Conflict)
        ));
        assert!(
            created_period_lock_audit(&caller, false, None, month, last)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn cancellation_statement_targets_migration_0188_state_columns() {
        let sql = CANCEL_SUBSTITUTION_SQL;
        assert!(sql.contains("cancel_reason"));
        assert!(sql.contains("status='ASSIGNED'"));
        assert!(!sql.contains("cancellation_reason"));
    }

    #[test]
    fn candidate_queries_enforce_every_eligibility_exclusion() {
        for sql in [
            SUBSTITUTION_CANDIDATES_SQL,
            SUBSTITUTION_CANDIDATE_TOTAL_SQL,
        ] {
            assert!(sql.contains("employment_status='ACTIVE'"));
            assert!(sql.contains("JOIN employee_employment_profiles p"));
            assert!(sql.contains("home_branch_id=$1"));
            assert!(sql.contains("e.id <> $2"));
            assert!(sql.contains("s.from_minutes < $5 AND s.to_minutes > $4"));
            assert!(sql.contains("l.status IN ('approved','APPROVED')"));
            assert!(sql.contains("x.kind='NO_SHOW' AND x.status='OPEN'"));
        }
    }

    #[test]
    fn candidate_query_uses_half_open_substitution_intervals() {
        assert!(SUBSTITUTION_CANDIDATES_SQL.contains("s.from_minutes < $5 AND s.to_minutes > $4"));
        assert!(!SUBSTITUTION_CANDIDATES_SQL.contains("s.from_minutes <= $5"));
    }

    #[test]
    fn substitution_eligibility_lock_uses_the_platform_coordination_contract() {
        assert!(
            SUBSTITUTION_ELIGIBILITY_LOCK_SQL
                .contains("public.console_employee_day_eligibility_lock($1,$2,$3)")
        );
        assert!(!SUBSTITUTION_ELIGIBILITY_LOCK_SQL.contains("pg_advisory_xact_lock"));
    }

    #[test]
    fn unrelated_sql_errors_are_not_downgraded_to_conflicts() {
        assert!(matches!(
            eligibility_guard_conflict(sqlx::Error::Protocol("unrelated database failure".into())),
            AttendanceStoreError::Sql(sqlx::Error::Protocol(_))
        ));
    }

    #[test]
    fn exception_projections_use_migration_0188_resolution_timestamp() {
        for sql in [LIST_EXCEPTIONS_SQL, EXCEPTION_BY_ID_SQL] {
            assert!(sql.contains("r.resolved_at AS resolved_at"));
            assert!(!sql.contains("r.created_at AS resolved_at"));
        }
    }

    #[test]
    fn week52_active_rows_preserve_identity_shape() {
        let rows = week52_inputs_for_active(
            [(Uuid::nil(), "Kim".to_owned(), Some("Operations".to_owned()))],
            Date::from_calendar_date(2026, Month::July, 6).unwrap(),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        assert_eq!(rows[0].name, "Kim");
        assert_eq!(rows[0].team.as_deref(), Some("Operations"));
    }

    #[test]
    fn fingerprint_changes_for_every_immutable_request_field() {
        let caller = scope();
        let base = command();
        let mut c = base.clone();
        c.branch_id = Some(Uuid::new_v4());
        assert_fingerprint_changes(&caller, &base, c);
        let mut c = base.clone();
        c.window.cover_date += Duration::days(1);
        assert_fingerprint_changes(&caller, &base, c);
        let mut c = base.clone();
        c.window.from_minutes = 541;
        assert_fingerprint_changes(&caller, &base, c);
        let mut c = base.clone();
        c.window.to_minutes = 1019;
        assert_fingerprint_changes(&caller, &base, c);
        let mut c = base.clone();
        c.covered_employee_id = Uuid::new_v4();
        assert_fingerprint_changes(&caller, &base, c);
        let mut c = base.clone();
        c.reason_kind = "OTHER".into();
        assert_fingerprint_changes(&caller, &base, c);
        let mut c = base.clone();
        c.reason_detail = None;
        assert_fingerprint_changes(&caller, &base, c);
        let mut c = base.clone();
        c.site = "Busan".into();
        assert_fingerprint_changes(&caller, &base, c);
        let mut c = base.clone();
        c.role = "Lead".into();
        assert_fingerprint_changes(&caller, &base, c);
        let mut c = base.clone();
        c.worker_employee_id = None;
        assert_fingerprint_changes(&caller, &base, c);
        let mut c = base.clone();
        c.worker_name = "Park".into();
        assert_eq!(
            substitution_fingerprint(&caller, &base),
            substitution_fingerprint(&caller, &c)
        );
        let mut c = base.clone();
        c.worker_type = "CONTRACTOR".into();
        assert_eq!(
            substitution_fingerprint(&caller, &base),
            substitution_fingerprint(&caller, &c)
        );
        let mut c = base.clone();
        c.worker_rate = None;
        assert_eq!(
            substitution_fingerprint(&caller, &base),
            substitution_fingerprint(&caller, &c)
        );
        let mut c = base.clone();
        c.exception_id = None;
        assert_fingerprint_changes(&caller, &base, c);
        let mut other_org = caller.clone();
        other_org.org_id = Uuid::new_v4();
        assert_ne!(
            fingerprint(&substitution_fingerprint(&caller, &base)),
            fingerprint(&substitution_fingerprint(&other_org, &base))
        );
    }
    #[test]
    fn canonical_whitespace_has_the_same_persisted_fingerprint() {
        let caller = scope();
        let mut canonical = command();
        let mut whitespace = canonical.clone();
        whitespace.site = " Seoul ".into();
        whitespace.role = " Operator ".into();
        whitespace.reason_kind = " APPROVED_LEAVE ".into();
        whitespace.reason_detail = Some(" medical ".into());
        whitespace.worker_name = " Kim ".into();
        whitespace.worker_type = " EMPLOYEE ".into();
        whitespace.worker_rate = Some(" 10000 ".into());
        canonicalize(&mut canonical);
        canonicalize(&mut whitespace);
        assert_eq!(
            substitution_fingerprint(&caller, &canonical),
            substitution_fingerprint(&caller, &whitespace)
        );
    }
    fn event_at(
        employee_id: Uuid,
        id: u128,
        kind: StrictDurationEventKind,
        hours: i64,
    ) -> StrictDurationEvent {
        StrictDurationEvent {
            employee_id,
            id: Uuid::from_u128(id),
            kind,
            occurred_at: OffsetDateTime::UNIX_EPOCH + Duration::hours(hours),
        }
    }
    #[test]
    fn week52_derives_only_real_complete_clock_pairs() {
        let employee = Uuid::new_v4();
        let hours = week52_hours(
            &[
                event_at(employee, 1, StrictDurationEventKind::ClockIn, 0),
                event_at(employee, 2, StrictDurationEventKind::Other, 1),
                event_at(employee, 3, StrictDurationEventKind::ClockOut, 9),
            ],
            OffsetDateTime::UNIX_EPOCH,
            OffsetDateTime::UNIX_EPOCH + Duration::days(7),
        )
        .unwrap();
        assert_eq!(hours.get(&employee), Some(&9.0));
    }
    #[test]
    fn week52_fails_closed_for_repeated_or_missing_clock_pairs() {
        let employee = Uuid::new_v4();
        let start = OffsetDateTime::UNIX_EPOCH;
        let end = start + Duration::days(7);
        assert!(
            week52_hours(
                &[event_at(employee, 1, StrictDurationEventKind::ClockOut, 1)],
                start,
                end
            )
            .is_err()
        );
        assert!(
            week52_hours(
                &[
                    event_at(employee, 1, StrictDurationEventKind::ClockIn, 0),
                    event_at(employee, 2, StrictDurationEventKind::ClockIn, 1)
                ],
                start,
                end
            )
            .is_err()
        );
        assert!(
            week52_hours(
                &[event_at(employee, 1, StrictDurationEventKind::ClockIn, 0)],
                start,
                end
            )
            .is_err()
        );
    }
    #[test]
    fn week52_clips_pairs_crossing_both_week_boundaries() {
        let employee = Uuid::new_v4();
        let start = OffsetDateTime::UNIX_EPOCH + Duration::days(7);
        let end = start + Duration::days(7);
        let hours = week52_hours(
            &[
                event_at(employee, 1, StrictDurationEventKind::ClockIn, 7 * 24 - 2),
                event_at(employee, 2, StrictDurationEventKind::ClockOut, 7 * 24 + 3),
                event_at(employee, 3, StrictDurationEventKind::ClockIn, 14 * 24 - 3),
                event_at(employee, 4, StrictDurationEventKind::ClockOut, 14 * 24 + 2),
            ],
            start,
            end,
        )
        .unwrap();
        assert_eq!(hours.get(&employee), Some(&6.0));
    }
    #[test]
    fn week52_pairs_persisted_events_by_timestamp_not_input_order() {
        let employee = Uuid::new_v4();
        let start = OffsetDateTime::UNIX_EPOCH;
        let end = start + Duration::days(7);
        let hours = week52_hours(
            &[
                event_at(employee, 2, StrictDurationEventKind::ClockOut, 9),
                event_at(employee, 1, StrictDurationEventKind::ClockIn, 0),
            ],
            start,
            end,
        )
        .unwrap();
        assert_eq!(hours.get(&employee), Some(&9.0));
    }
    #[test]
    fn week52_includes_authorized_active_employee_with_zero_hours_and_stable_ack() {
        let employee = Uuid::new_v4();
        let week_start = Date::from_calendar_date(2026, Month::July, 20).unwrap();
        let acknowledged_at = OffsetDateTime::UNIX_EPOCH + Duration::hours(1);
        let inputs = week52_inputs_for_active(
            [(employee, "Kim".to_owned(), Some("Operations".to_owned()))],
            week_start,
            BTreeMap::new(),
            BTreeMap::from([(employee, acknowledged_at)]),
        );
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].employee_id, employee);
        assert_eq!(inputs[0].name, "Kim");
        assert_eq!(inputs[0].team.as_deref(), Some("Operations"));
        assert_eq!(inputs[0].current_hours, 0.0);
        assert_eq!(inputs[0].acknowledged_at, Some(acknowledged_at));

        let replay = week52_inputs_for_active(
            [(employee, "Kim".to_owned(), Some("Operations".to_owned()))],
            week_start,
            BTreeMap::new(),
            BTreeMap::from([(employee, acknowledged_at)]),
        );
        assert_eq!(replay[0].acknowledged_at, inputs[0].acknowledged_at);
    }
    #[test]
    fn week52_ack_replay_keeps_timestamp_and_writes_no_duplicate_audit() {
        let caller = scope();
        let employee = Uuid::new_v4();
        let branch = Uuid::new_v4();
        let week_start = Date::from_calendar_date(2026, Month::July, 20).unwrap();
        let acknowledged_at = OffsetDateTime::UNIX_EPOCH + Duration::hours(1);
        let (first_response, first_audits) = week52_acknowledgement_response(
            &caller,
            employee,
            week_start,
            acknowledged_at,
            true,
            branch,
        )
        .unwrap();
        let (replay_response, replay_audits) = week52_acknowledgement_response(
            &caller,
            employee,
            week_start,
            acknowledged_at,
            false,
            branch,
        )
        .unwrap();
        assert_eq!(first_response.employee_id, replay_response.employee_id);
        assert_eq!(first_response.week_start, replay_response.week_start);
        assert_eq!(
            first_response.acknowledged_at,
            replay_response.acknowledged_at
        );
        assert_eq!(first_audits.len(), 1);
        assert!(replay_audits.is_empty());
    }
    #[test]
    fn historical_interval_contract_covers_full_partial_and_no_coverage() {
        let employee = Uuid::new_v4();
        let date = Date::from_calendar_date(2026, Month::July, 2).unwrap();
        let window = SubstitutionWindow::new(date, 540, 1020).unwrap();
        assert!(
            HistoricalAbsence::new(employee, date, 480, 1080)
                .unwrap()
                .fully_covers(employee, &window)
        );
        assert!(
            !HistoricalAbsence::new(employee, date, 600, 1020)
                .unwrap()
                .fully_covers(employee, &window)
        );
        assert!(
            !HistoricalAbsence::new(Uuid::new_v4(), date, 480, 1080)
                .unwrap()
                .fully_covers(employee, &window)
        );
    }
    #[test]
    fn historical_leave_coverage_uses_schema_backed_full_and_half_day_windows() {
        assert_eq!(leave_coverage_window("annual", None), Some((0, 1440)));
        assert_eq!(
            leave_coverage_window("half_day", Some("am")),
            Some((0, 720))
        );
        assert_eq!(
            leave_coverage_window("half_day", Some("pm")),
            Some((720, 1440))
        );
    }
    #[test]
    fn historical_leave_coverage_fails_closed_for_unknown_or_malformed_intent() {
        assert_eq!(leave_coverage_window("annual", Some("am")), None);
        assert_eq!(leave_coverage_window("half_day", None), None);
        assert_eq!(leave_coverage_window("half_day", Some("night")), None);
        assert_eq!(leave_coverage_window("legacy", None), None);
    }
    #[test]
    fn historical_leave_coverage_query_uses_authoritative_leave_columns() {
        assert!(HISTORICAL_LEAVE_COVERAGE_SQL.contains("leave_type"));
        assert!(HISTORICAL_LEAVE_COVERAGE_SQL.contains("partial_day_period"));
        assert!(!HISTORICAL_LEAVE_COVERAGE_SQL.contains("start_minutes"));
        assert!(!HISTORICAL_LEAVE_COVERAGE_SQL.contains("end_minutes"));
    }
}
