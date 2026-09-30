//! Postgres leave-request adapter.
//!
//! Tenant scoping is armed by [`with_audit`]/[`with_org_conn`] from the
//! request-context `app.current_org` GUC; RLS narrows every statement to the
//! caller's org. On top of RLS, the approval queue and decide path are
//! *branch*-scoped in code from the caller's resolved [`BranchScope`], so an
//! approver only ever sees and decides requests in their own branches
//! (deny-by-omission on an empty scope).
//!
//! Two invariants this adapter enforces that the schema cannot:
//!   * **SoD** — the decider must not be the request's requester (mirrors the
//!     workflow-engine initiator guard, #205). Backed by a DB CHECK too, so a
//!     bug cannot stamp a self-decision.
//!   * **approve writes the ledger** — an `approve` decision moves the subject
//!     employee's leave balance with exact fixed-scale units
//!     in the SAME audited transaction as the status change, so the two never
//!     diverge.
//!
//! The §61 statutory push delivers a receipt-gated notice into the target's
//! 개인 수신함 through the inbox crate's [`InboxDocSink`] (the concrete legal
//! delivery) and records the push row. Starting the engine AP- run is gated on
//! the 연차촉진 submittable definition (gap #1); until it exists the push is
//! recorded with `ap_run_id = NULL` and an honest `pending_engine_definition`
//! status — never a fabricated run.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::sync::Arc;

use console_inbox_application::{EmitInboxDocCommand, InboxDocSink};
use console_inbox_domain::{InboxDocKind, NewInboxDoc};
use console_kernel_core::{
    BranchScope, Date, ErrorKind, KernelError, LeavePromotionId, LeaveRequestId, OrgId, UserId,
};
use console_leave_application::{
    ApSubmission, CreateLeaveRequestCommand, DecideLeaveRequestCommand,
    ImportEmployeeLeaveBalanceCommand, ImportEmployeeLeaveBalanceResult, LeaveBalancePage,
    LeaveBalanceTone, LeaveBalanceView, LeaveChargeResolutionView, LeaveRequestPage,
    LeaveRequestView, ListLeaveRequestsQuery, ListSelfLeaveRequestsQuery,
    ProposeAlternateDatesCommand, ResolveLeaveChargeCommand, ResolveLeaveChargeQuery,
    SelfLeaveBalanceView, SelfLeaveFilingState, StatutoryPushCommand, StatutoryPushView,
    WorkCalendarPort, leave_promotion_audit_event,
};
use console_leave_domain::{
    LeaveBalanceAmount, LeaveChargeAssessment, LeaveChargeResolutionOrigin,
    LeaveChargeReviewReason, LeaveChargeState, LeaveDateCharge, LeaveStatus, LeaveType, LeaveUnits,
    NewLeaveRequest, PartialDayPeriod, PromotionContext, PromotionKind, PromotionTrack,
    RecordedLeaveChargeSnapshot, SourceRevisionRef, TimeChangeGroundsCode, WorkObligation,
    alternate_leave_request, first_round_window, second_round_deadline, validate_alternate_dates,
    validate_designated_dates, validate_push,
};
use console_platform_db::{DbError, with_audit, with_org_conn};
use console_platform_request_context::current_org;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use time::macros::offset;

const REQUEST_COLUMNS: &str = "id, branch_id, requester_user_id, subject_employee_id, leave_type, \
     COALESCE(charge_units, legacy_days)::float8 AS days, \
     CASE WHEN charge_units IS NULL THEN NULL ELSE (charge_units * 1000000)::bigint END AS charge_micros, \
     charge_state, charge_review_reasons, request_version, charge_version, partial_day_period, \
     (SELECT server_digest FROM leave_charge_resolutions lcr \
       WHERE lcr.id = leave_requests.current_charge_resolution_id) AS charge_digest, \
     (SELECT resolved_by FROM leave_charge_resolutions lcr \
       WHERE lcr.id = leave_requests.current_charge_resolution_id) AS charge_resolved_by, \
     (SELECT resolution_origin FROM leave_charge_resolutions lcr \
       WHERE lcr.id = leave_requests.current_charge_resolution_id) AS charge_resolution_origin, \
     start_date, end_date, reason, status, decided_by, decided_at, decision_comment, \
     time_change_grounds, time_change_evidence, \
     alternate_start_date, alternate_end_date, alternate_partial_day_period, alternate_proposed_at, \
     ap_run_id, created_at";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromotionInsert {
    Inserted,
    Duplicate,
}

#[derive(Debug, Clone)]
struct PromotionRecord {
    id: LeavePromotionId,
    inbox_doc_id: uuid::Uuid,
    /// The KST date the notice was served — the anchor for the §61 reply window
    /// and ordering checks.
    served_on: Date,
    /// The delivered notice's `source_id`, which encodes the 연차 사용기간 the
    /// push belongs to. Used to tell a genuine re-push apart from a *different*
    /// leave period colliding on the current per-employee unique key.
    source_id: Option<String>,
    /// The delivered notice's payload — the round-2 designation is read back
    /// from here rather than restated by the refusal's caller.
    payload: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum PgLeaveError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),

    #[error("active employee has no explicit home branch")]
    MissingHomeBranch,

    #[error("leave charge requires review before approval: {0:?}")]
    ChargeReviewRequired(Vec<LeaveChargeReviewReason>),

    #[error("leave request version changed concurrently")]
    ConcurrentModification,

    #[error("leave command database capability is unavailable")]
    CommandUnavailable,
}

impl PgLeaveError {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Domain(err) => err.kind,
            Self::MissingHomeBranch | Self::ChargeReviewRequired(_) => ErrorKind::Conflict,
            Self::ConcurrentModification => ErrorKind::Conflict,
            Self::CommandUnavailable => ErrorKind::Internal,
            Self::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => ErrorKind::NotFound,
            Self::Db(_) => ErrorKind::Internal,
        }
    }
}

impl From<sqlx::Error> for PgLeaveError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

impl From<PgLeaveError> for KernelError {
    fn from(value: PgLeaveError) -> Self {
        match value {
            PgLeaveError::Domain(err) => err,
            PgLeaveError::Db(err) => KernelError::internal(format!("{err:?}")),
            PgLeaveError::MissingHomeBranch => {
                KernelError::conflict("active employee has no explicit home branch")
            }
            PgLeaveError::ChargeReviewRequired(_) => {
                KernelError::conflict("leave charge requires review before approval")
            }
            PgLeaveError::ConcurrentModification => {
                KernelError::conflict("leave request version changed concurrently")
            }
            PgLeaveError::CommandUnavailable => {
                KernelError::internal("leave command database capability is unavailable")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmployeeHomeBranchUpdate {
    pub employee_id: uuid::Uuid,
    pub home_branch_id: uuid::Uuid,
    pub updated_at: time::OffsetDateTime,
}

/// Validated input for the atomic People employee-create command.
#[derive(Debug)]
pub struct CreateEmployeeCommand {
    pub employee_id: uuid::Uuid,
    pub employee_number: String,
    pub name: String,
    pub company: String,
    pub employment_type: String,
    pub phone_e164: String,
    pub org_unit: String,
    pub position: String,
    pub site: String,
    pub home_branch_id: uuid::Uuid,
    pub base_pay: String,
    pub idempotency_key: String,
    pub request_hash: String,
    pub actor: UserId,
    pub trace: console_kernel_core::TraceContext,
}

/// Result of the atomic People employee-create command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmployeeCreateResult {
    pub employee_id: uuid::Uuid,
    pub replayed: bool,
}

#[derive(Clone)]
pub struct PgLeaveStore {
    pool: PgPool,
    leave_command_pool: Option<PgPool>,
    inbox: Arc<dyn InboxDocSink>,
    work_calendar: Arc<dyn WorkCalendarPort>,
}

#[derive(Debug, Default)]
pub struct ManualReferenceCalendar;

impl WorkCalendarPort for ManualReferenceCalendar {
    fn resolve_charge(
        &self,
        _query: ResolveLeaveChargeQuery,
    ) -> console_leave_application::LeaveChargeFuture<'_> {
        Box::pin(async {
            LeaveChargeAssessment::review_required(vec![
                LeaveChargeReviewReason::MissingCalendar,
                LeaveChargeReviewReason::MissingPolicy,
            ])
        })
    }
}

impl std::fmt::Debug for PgLeaveStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgLeaveStore").finish_non_exhaustive()
    }
}

impl PgLeaveStore {
    #[must_use]
    pub fn new(pool: PgPool, inbox: Arc<dyn InboxDocSink>) -> Self {
        Self {
            pool,
            leave_command_pool: None,
            inbox,
            work_calendar: Arc::new(ManualReferenceCalendar),
        }
    }

    #[must_use]
    pub fn with_work_calendar(
        pool: PgPool,
        inbox: Arc<dyn InboxDocSink>,
        work_calendar: Arc<dyn WorkCalendarPort>,
    ) -> Self {
        Self {
            pool,
            leave_command_pool: None,
            inbox,
            work_calendar,
        }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Bind the isolated EXECUTE-only `console_leave_cmd` pool used for all leave
    /// mutations. Read paths continue to use the ordinary `console_rt` pool.
    #[must_use]
    pub fn with_leave_command_pool(mut self, pool: PgPool) -> Self {
        self.leave_command_pool = Some(pool);
        self
    }

    /// Whether the isolated leave command capability is bound. Callers that
    /// must fail closed BEFORE creating dependent rows (employee creation and
    /// the recruiting hire handshake pre-check home-branch assignability) use
    /// this instead of discovering `CommandUnavailable` after a partial write.
    #[must_use]
    pub fn has_leave_command_pool(&self) -> bool {
        self.leave_command_pool.is_some()
    }

    fn command_pool(&self) -> Result<&PgPool, PgLeaveError> {
        self.leave_command_pool
            .as_ref()
            .ok_or(PgLeaveError::CommandUnavailable)
    }

    /// Create one employee and its employment facts through the isolated
    /// command capability. The database owns idempotency, branch authority,
    /// lifecycle evidence, and audit in one transaction.
    pub async fn create_employee(
        &self,
        command: CreateEmployeeCommand,
    ) -> Result<EmployeeCreateResult, PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let row = sqlx::query(
            "SELECT * FROM leave_api.create_employee(\
             $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12::numeric,$13,$14,$15,$16,$17)",
        )
        .bind(org.as_uuid())
        .bind(command.employee_id)
        .bind(command.employee_number)
        .bind(command.name)
        .bind(command.company)
        .bind(command.employment_type)
        .bind(command.phone_e164)
        .bind(command.org_unit)
        .bind(command.position)
        .bind(command.site)
        .bind(command.home_branch_id)
        .bind(command.base_pay)
        .bind(command.idempotency_key)
        .bind(command.request_hash)
        .bind(command.actor.as_uuid())
        .bind(command.trace.trace_id())
        .bind(command.trace.span_id())
        .fetch_one(self.command_pool()?)
        .await
        .map_err(map_leave_command_sqlx)?;
        Ok(EmployeeCreateResult {
            employee_id: row.try_get("employee_id")?,
            replayed: row.try_get("replayed")?,
        })
    }

    /// Set the authoritative employee approval-routing branch through the
    /// isolated command capability. The database locks the employee and both
    /// authorization branches, applies optimistic concurrency, and owns audit.
    pub async fn set_employee_home_branch(
        &self,
        employee_id: uuid::Uuid,
        home_branch_id: uuid::Uuid,
        expected_updated_at: time::OffsetDateTime,
        actor: UserId,
        trace: console_kernel_core::TraceContext,
    ) -> Result<EmployeeHomeBranchUpdate, PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let row = sqlx::query(
            "SELECT * FROM leave_api.set_employee_home_branch(\
             $1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(org.as_uuid())
        .bind(employee_id)
        .bind(home_branch_id)
        .bind(expected_updated_at)
        .bind(actor.as_uuid())
        .bind(trace.trace_id())
        .bind(trace.span_id())
        .fetch_one(self.command_pool()?)
        .await
        .map_err(map_leave_command_sqlx)?;
        Ok(EmployeeHomeBranchUpdate {
            employee_id: row.try_get("employee_id")?,
            home_branch_id: row.try_get("home_branch_id")?,
            updated_at: row.try_get("updated_at")?,
        })
    }

    /// Establish the FIRST home-branch routing authority for an employee just
    /// created by the create flow, through the dedicated create-path command
    /// (`leave_api.set_employee_home_branch_create`). Unlike the general
    /// `set_employee_home_branch`, this is create-scoped: the database verifies
    /// server-side creation evidence (an `employee.create` audit by the actor for
    /// THIS employee with the requested branch) and authorizes with
    /// `assert_employee_directory_manager`, so an org-wide directory manager is
    /// not 403ed after committing the row. The general route function is
    /// unchanged and keeps `assert_org_admin` for arbitrary first assignments.
    pub async fn set_employee_home_branch_create(
        &self,
        employee_id: uuid::Uuid,
        home_branch_id: uuid::Uuid,
        expected_updated_at: time::OffsetDateTime,
        actor: UserId,
        trace: console_kernel_core::TraceContext,
    ) -> Result<EmployeeHomeBranchUpdate, PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let row = sqlx::query(
            "SELECT * FROM leave_api.set_employee_home_branch_create(\
             $1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(org.as_uuid())
        .bind(employee_id)
        .bind(home_branch_id)
        .bind(expected_updated_at)
        .bind(actor.as_uuid())
        .bind(trace.trace_id())
        .bind(trace.span_id())
        .fetch_one(self.command_pool()?)
        .await
        .map_err(map_leave_command_sqlx)?;
        Ok(EmployeeHomeBranchUpdate {
            employee_id: row.try_get("employee_id")?,
            home_branch_id: row.try_get("home_branch_id")?,
            updated_at: row.try_get("updated_at")?,
        })
    }

    /// Apply an imported exact balance snapshot through the database-owned,
    /// payload-bound idempotent command and return its CAS result.
    pub async fn import_employee_leave_balance(
        &self,
        command: ImportEmployeeLeaveBalanceCommand,
    ) -> Result<ImportEmployeeLeaveBalanceResult, PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let accrued = command.accrued.map(|value| value.canonical_decimal());
        let used = command.used.map(|value| value.canonical_decimal());
        let remaining = command.remaining.map(|value| value.canonical_decimal());
        let row = sqlx::query(
            "SELECT * FROM leave_api.import_employee_leave_balance(\
             $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        )
        .bind(org.as_uuid())
        .bind(command.employee_id)
        .bind(command.expected_updated_at)
        .bind(accrued.as_deref())
        .bind(used.as_deref())
        .bind(remaining.as_deref())
        .bind(&command.source_kind)
        .bind(&command.source_ref)
        .bind(&command.idempotency_key)
        .bind(command.actor.as_uuid())
        .bind(command.trace.trace_id())
        .bind(command.trace.span_id())
        .fetch_one(self.command_pool()?)
        .await
        .map_err(map_leave_command_sqlx)?;
        Ok(ImportEmployeeLeaveBalanceResult {
            employee_id: row.try_get("employee_id")?,
            updated_at: row.try_get("updated_at")?,
            changed: row.try_get("changed")?,
            replayed: row.try_get("replayed")?,
        })
    }

    /// Atomically apply an employee roster import, its protected balance
    /// snapshots and receipts, and optional staged-run completion.
    pub async fn apply_employee_import_batch(
        &self,
        run_id: Option<uuid::Uuid>,
        source_ref: &str,
        rows: &serde_json::Value,
        actor: UserId,
        apply_audit: &serde_json::Value,
        trace: console_kernel_core::TraceContext,
    ) -> Result<serde_json::Value, PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let row = sqlx::query(
            "SELECT * FROM leave_api.apply_employee_import_batch(\
             $1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(org.as_uuid())
        .bind(run_id)
        .bind(source_ref)
        .bind(rows)
        .bind(actor.as_uuid())
        .bind(apply_audit)
        .bind(trace.trace_id())
        .bind(trace.span_id())
        .fetch_one(self.command_pool()?)
        .await
        .map_err(map_leave_command_sqlx)?;
        Ok(row.try_get("report")?)
    }

    // -----------------------------------------------------------------------
    // Create (crate-level write port — see application docs)
    // -----------------------------------------------------------------------

    /// Insert a pending leave request. Domain validation happens in the caller
    /// (`NewLeaveRequest`); the row is created `pending`, unassigned.
    pub async fn create_request(
        &self,
        command: CreateLeaveRequestCommand,
    ) -> Result<LeaveRequestView, PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let (subject_employee_id, branch_id) = self
            .resolve_self_filing_context(command.requester_user_id)
            .await?;
        if subject_employee_id != command.subject_employee_id {
            return Err(KernelError::forbidden(
                "self-service leave subject must be the caller's linked employee",
            )
            .into());
        }
        let id = LeaveRequestId::new();
        let request = command.request;
        let assessment = self
            .work_calendar
            .resolve_charge(ResolveLeaveChargeQuery {
                org_id: org,
                branch_id,
                subject_employee_id,
                leave_type: request.leave_type,
                start_date: request.start_date,
                end_date: request.end_date,
                partial_day_period: request.partial_day_period,
                as_of: command.occurred_at,
            })
            .await?;

        let (review_reasons, date_charges, calendar_ref, policy_ref, supporting_refs) =
            match assessment {
                LeaveChargeAssessment::ReviewRequired { reasons } => {
                    if reasons.is_empty() {
                        return Err(KernelError::internal(
                            "work-calendar adapter returned review-required without reasons",
                        )
                        .into());
                    }
                    (
                        reasons
                            .into_iter()
                            .map(|reason| reason.as_str().to_owned())
                            .collect(),
                        None,
                        None,
                        None,
                        None,
                    )
                }
                LeaveChargeAssessment::Resolved { evidence } => {
                    if evidence.home_branch_id != branch_id {
                        return Err(KernelError::conflict(
                            "work-calendar evidence does not match the pinned home branch",
                        )
                        .into());
                    }
                    // Preserve rich domain validation at the adapter edge. The
                    // database repeats the security-critical shape/total checks
                    // and derives the persisted total, version, and digest.
                    let snapshot = canonical_snapshot(
                        branch_id,
                        request.leave_type,
                        request.partial_day_period,
                        request.start_date,
                        request.end_date,
                        evidence.date_charges,
                        evidence.calendar_revision_ref,
                        evidence.policy_revision_ref,
                        evidence.supporting_source_refs,
                    )?;
                    (
                        Vec::new(),
                        Some(
                            serde_json::to_value(&snapshot.date_charges).map_err(|error| {
                                PgLeaveError::Domain(KernelError::internal(error.to_string()))
                            })?,
                        ),
                        Some(
                            serde_json::to_value(&snapshot.calendar_revision_ref).map_err(
                                |error| {
                                    PgLeaveError::Domain(KernelError::internal(error.to_string()))
                                },
                            )?,
                        ),
                        Some(serde_json::to_value(&snapshot.policy_revision_ref).map_err(
                            |error| PgLeaveError::Domain(KernelError::internal(error.to_string())),
                        )?),
                        Some(
                            serde_json::to_value(&snapshot.supporting_source_refs).map_err(
                                |error| {
                                    PgLeaveError::Domain(KernelError::internal(error.to_string()))
                                },
                            )?,
                        ),
                    )
                }
            };

        let created = sqlx::query(
            "SELECT * FROM leave_api.create_request(\
             $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
        )
        .bind(org.as_uuid())
        .bind(id.as_uuid())
        .bind(command.requester_user_id.as_uuid())
        .bind(request.leave_type.as_str())
        .bind(request.start_date)
        .bind(request.end_date)
        .bind(request.partial_day_period.map(|period| period.as_str()))
        .bind(review_reasons)
        .bind(date_charges.as_ref().map(|_| branch_id))
        .bind(date_charges)
        .bind(calendar_ref)
        .bind(policy_ref)
        .bind(supporting_refs)
        .bind(command.idempotency_key)
        .bind(command.trace.trace_id())
        .bind(command.trace.span_id())
        .fetch_one(self.command_pool()?)
        .await
        .map_err(map_leave_command_sqlx)?;
        let committed_id = LeaveRequestId::from_uuid(created.try_get("request_id")?);

        self.fetch_request_scoped(org, committed_id, &BranchScope::All)
            .await?
            .ok_or_else(|| {
                PgLeaveError::Domain(KernelError::internal(
                    "leave command committed but created request was not readable",
                ))
            })
    }

    /// Resolve the caller's OWN filing context for a self-service leave request:
    /// the employee record linked to their account and the branch its approval
    /// routes to. Both come from the caller's trusted server-side identity, never
    /// from request input, so a caller can only ever file for their own employee.
    /// Fail-closed: routing uses only the active employee's explicit home branch;
    /// user-branch memberships are authorization scope, never routing authority.
    pub async fn resolve_self_filing_context(
        &self,
        user_id: UserId,
    ) -> Result<(uuid::Uuid, uuid::Uuid), PgLeaveError> {
        let (employee_id, branch_id) = self.resolve_self_employee_identity(user_id).await?;
        Ok((
            employee_id,
            branch_id.ok_or(PgLeaveError::MissingHomeBranch)?,
        ))
    }

    async fn resolve_self_employee_identity(
        &self,
        user_id: UserId,
    ) -> Result<(uuid::Uuid, Option<uuid::Uuid>), PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let uid = *user_id.as_uuid();
        let row = with_org_conn::<_, _, PgLeaveError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(
                    "SELECT e.id AS employee_id, \
                            CASE WHEN b.deactivated_at IS NULL THEN e.home_branch_id ELSE NULL END AS branch_id \
                     FROM users u \
                     JOIN employees e ON e.id = u.employee_id AND e.org_id = u.org_id \
                     LEFT JOIN branches b ON b.id = e.home_branch_id AND b.org_id = e.org_id \
                     WHERE u.id = $1 AND u.employee_id IS NOT NULL \
                       AND u.is_active AND e.employment_status = 'ACTIVE'",
                )
                .bind(uid)
                .fetch_optional(tx.as_mut())
                .await?)
            })
        })
        .await?;
        let row = row.ok_or_else(|| {
            PgLeaveError::Domain(KernelError::forbidden(
                "no active linked employee for self-service leave",
            ))
        })?;
        Ok((row.try_get("employee_id")?, row.try_get("branch_id")?))
    }

    // -----------------------------------------------------------------------
    // Queue read (branch-scoped)
    // -----------------------------------------------------------------------

    pub async fn list_requests(
        &self,
        query: ListLeaveRequestsQuery,
    ) -> Result<LeaveRequestPage, PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let limit = query.limit.clamp(1, 200);
        let rows = with_org_conn::<_, _, PgLeaveError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let cursor = if let Some(cursor_id) = query.cursor {
                    let mut cursor_builder = QueryBuilder::<Postgres>::new(
                        "SELECT CASE WHEN status = 'pending' THEN 1 ELSE 0 END AS pending_rank, created_at, id FROM leave_requests WHERE id = ",
                    );
                    cursor_builder.push_bind(*cursor_id.as_uuid());
                    cursor_builder.push(" AND ");
                    push_branch_scope(&mut cursor_builder, &query.branch_scope);
                    if let Some(status) = query.status {
                        cursor_builder.push(" AND status = ");
                        cursor_builder.push_bind(status.as_str());
                    }
                    let row = cursor_builder
                        .build()
                        .fetch_optional(tx.as_mut())
                        .await?
                        .ok_or_else(|| {
                            KernelError::not_found("leave request cursor was not found")
                        })?;
                    Some((
                        row.try_get::<i32, _>("pending_rank")?,
                        row.try_get::<time::OffsetDateTime, _>("created_at")?,
                        row.try_get::<uuid::Uuid, _>("id")?,
                    ))
                } else {
                    None
                };

                let mut builder = QueryBuilder::<Postgres>::new(format!(
                    "SELECT {REQUEST_COLUMNS} FROM leave_requests WHERE "
                ));
                push_branch_scope(&mut builder, &query.branch_scope);
                if let Some(status) = query.status {
                    builder.push(" AND status = ");
                    builder.push_bind(status.as_str());
                }
                if let Some((pending_rank, created_at, id)) = cursor {
                    builder.push(
                        " AND (CASE WHEN status = 'pending' THEN 1 ELSE 0 END, created_at, id) < (",
                    );
                    builder.push_bind(pending_rank);
                    builder.push(", ");
                    builder.push_bind(created_at);
                    builder.push(", ");
                    builder.push_bind(id);
                    builder.push(")");
                }
                // Pending first (the actionable queue), then newest. Fetch one
                // look-ahead row so the response never silently truncates.
                builder.push(
                    " ORDER BY CASE WHEN status = 'pending' THEN 1 ELSE 0 END DESC, created_at DESC, id DESC LIMIT ",
                );
                builder.push_bind(limit + 1);
                Ok(builder.build().fetch_all(tx.as_mut()).await?)
            })
        })
        .await?;
        let mut items = rows
            .iter()
            .map(request_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if i64::try_from(items.len()).unwrap_or(0) > limit {
            items.truncate(usize::try_from(limit).unwrap_or(items.len()));
            items.last().map(|request| request.id)
        } else {
            None
        };
        Ok(LeaveRequestPage { items, next_cursor })
    }

    /// Return only the authenticated requester's own leave history. Both the
    /// requester and subject employee predicates are required so a stale or
    /// corrupted user link cannot broaden this read.
    pub async fn list_self_requests(
        &self,
        query: ListSelfLeaveRequestsQuery,
    ) -> Result<LeaveRequestPage, PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let (employee_id, _) = self.resolve_self_employee_identity(query.requester).await?;
        let requester = *query.requester.as_uuid();
        let limit = query.limit.clamp(1, 200);
        let rows = with_org_conn::<_, _, PgLeaveError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let cursor = if let Some(cursor_id) = query.cursor {
                    let row = sqlx::query(
                        "SELECT created_at, id FROM leave_requests \
                         WHERE id = $1 AND requester_user_id = $2 AND subject_employee_id = $3",
                    )
                    .bind(*cursor_id.as_uuid())
                    .bind(requester)
                    .bind(employee_id)
                    .fetch_optional(tx.as_mut())
                    .await?
                    .ok_or_else(|| KernelError::not_found("leave request cursor was not found"))?;
                    Some((
                        row.try_get::<time::OffsetDateTime, _>("created_at")?,
                        row.try_get::<uuid::Uuid, _>("id")?,
                    ))
                } else {
                    None
                };

                let mut builder = QueryBuilder::<Postgres>::new(format!(
                    "SELECT {REQUEST_COLUMNS} FROM leave_requests \
                     WHERE requester_user_id = "
                ));
                builder.push_bind(requester);
                builder.push(" AND subject_employee_id = ");
                builder.push_bind(employee_id);
                if let Some((created_at, id)) = cursor {
                    builder.push(" AND (created_at, id) < (");
                    builder.push_bind(created_at);
                    builder.push(", ");
                    builder.push_bind(id);
                    builder.push(")");
                }
                builder.push(" ORDER BY created_at DESC, id DESC LIMIT ");
                builder.push_bind(limit + 1);
                Ok(builder.build().fetch_all(tx.as_mut()).await?)
            })
        })
        .await?;
        let mut items = rows
            .iter()
            .map(request_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if i64::try_from(items.len()).unwrap_or(0) > limit {
            items.truncate(usize::try_from(limit).unwrap_or(items.len()));
            items.last().map(|request| request.id)
        } else {
            None
        };
        Ok(LeaveRequestPage { items, next_cursor })
    }

    pub async fn get_self_balance(
        &self,
        requester: UserId,
    ) -> Result<SelfLeaveBalanceView, PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let (employee_id, home_branch_id) = self.resolve_self_employee_identity(requester).await?;
        let row = with_org_conn::<_, _, PgLeaveError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(
                    "SELECT id, name, \
                     (leave_accrued * 1000000)::bigint AS accrued_micros, \
                     (leave_used * 1000000)::bigint AS used_micros, \
                     (leave_remaining * 1000000)::bigint AS remaining_micros \
                     FROM employees WHERE id = $1",
                )
                .bind(employee_id)
                .fetch_one(tx.as_mut())
                .await?)
            })
        })
        .await?;
        Ok(SelfLeaveBalanceView {
            employee_id: row.try_get("id")?,
            name: row.try_get("name")?,
            accrued_units: optional_units(&row, "accrued_micros")?,
            used_units: optional_units(&row, "used_micros")?,
            remaining_units: optional_units(&row, "remaining_micros")?,
            filing_state: if home_branch_id.is_some() {
                SelfLeaveFilingState::Ready
            } else {
                SelfLeaveFilingState::HomeBranchRequired
            },
            home_branch_id,
        })
    }

    // -----------------------------------------------------------------------
    // Resolve exact charge / decide
    // -----------------------------------------------------------------------

    pub async fn resolve_charge(
        &self,
        command: ResolveLeaveChargeCommand,
    ) -> Result<LeaveChargeResolutionView, PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let existing = self
            .fetch_request_scoped(org, command.request_id, &command.branch_scope)
            .await?
            .ok_or_else(|| KernelError::not_found("leave request not found"))?;
        if existing.status != LeaveStatus::Pending {
            return Err(KernelError::conflict(
                "only a pending leave request can receive a charge resolution",
            )
            .into());
        }
        if existing.requester_user_id == command.resolver {
            return Err(
                KernelError::forbidden("requester cannot resolve their own leave charge").into(),
            );
        }
        if existing.request_version != command.expected_version {
            return Err(PgLeaveError::ConcurrentModification);
        }

        let snapshot = canonical_snapshot(
            existing.branch_id,
            existing.leave_type,
            existing.partial_day_period,
            existing.start_date,
            existing.end_date,
            command.date_charges,
            command.calendar_revision_ref,
            command.policy_revision_ref,
            command.supporting_source_refs,
        )?;
        let date_charges = serde_json::to_value(&snapshot.date_charges)
            .map_err(|error| KernelError::internal(error.to_string()))?;
        let calendar_ref = serde_json::to_value(&snapshot.calendar_revision_ref)
            .map_err(|error| KernelError::internal(error.to_string()))?;
        let policy_ref = serde_json::to_value(&snapshot.policy_revision_ref)
            .map_err(|error| KernelError::internal(error.to_string()))?;
        let supporting_refs = serde_json::to_value(&snapshot.supporting_source_refs)
            .map_err(|error| KernelError::internal(error.to_string()))?;
        let row = sqlx::query(
            "SELECT * FROM leave_api.resolve_charge(\
             $1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(org.as_uuid())
        .bind(command.request_id.as_uuid())
        .bind(command.resolver.as_uuid())
        .bind(command.expected_version)
        .bind(date_charges)
        .bind(calendar_ref)
        .bind(policy_ref)
        .bind(supporting_refs)
        .bind(command.trace.trace_id())
        .bind(command.trace.span_id())
        .fetch_one(self.command_pool()?)
        .await
        .map_err(map_leave_command_sqlx)?;
        let charge_micros: i64 = row.try_get("charge_micros")?;
        Ok(LeaveChargeResolutionView {
            request_id: command.request_id,
            request_version: row.try_get("request_version")?,
            charge_units: LeaveUnits::from_micros(charge_micros)?,
            charge_state: LeaveChargeState::Resolved,
            charge_version: row.try_get("charge_version")?,
            server_digest: row.try_get("server_digest")?,
            resolution_origin: LeaveChargeResolutionOrigin::Manual,
            resolved_by: Some(command.resolver),
        })
    }

    pub async fn decide(
        &self,
        command: DecideLeaveRequestCommand,
    ) -> Result<LeaveRequestView, PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let request_id = command.request_id;
        let decider = command.decider;

        // Read-first to classify before mutating: NotFound (or out-of-branch),
        // already-decided (409), or self-decision (SoD, 403). A branch-scoped
        // read means an out-of-scope request is indistinguishable from a
        // missing one — deny-by-omission.
        let existing = self
            .fetch_request_scoped(org, request_id, &command.branch_scope)
            .await?
            .ok_or_else(|| KernelError::not_found("leave request not found"))?;
        if existing.status != LeaveStatus::Pending {
            return Err(KernelError::conflict(format!(
                "leave request is already {} and cannot be decided again",
                existing.status.as_str()
            ))
            .into());
        }
        if existing.requester_user_id == decider {
            return Err(KernelError::forbidden(
                "a leave request cannot be decided by its own requester (separation of duties)",
            )
            .into());
        }
        if let Some(expected_version) = command.expected_version
            && existing.request_version != expected_version
        {
            return Err(PgLeaveError::ConcurrentModification);
        }

        let decision = command.decision;
        if decision.writes_ledger() && existing.charge_resolved_by == Some(decider) {
            return Err(KernelError::forbidden(
                "leave charge resolver cannot approve the same request",
            )
            .into());
        }

        let row = sqlx::query(
            "SELECT * FROM leave_api.decide_request(\
             $1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(org.as_uuid())
        .bind(request_id.as_uuid())
        .bind(decider.as_uuid())
        .bind(command.expected_version)
        .bind(decision.as_str())
        .bind(command.comment)
        .bind(command.trace.trace_id())
        .bind(command.trace.span_id())
        .fetch_one(self.command_pool()?)
        .await
        .map_err(map_leave_command_sqlx)?;
        let outcome: String = row.try_get("outcome")?;
        if outcome == "charge_review_required" {
            return Err(PgLeaveError::ChargeReviewRequired(
                existing.charge_review_reasons,
            ));
        }
        if outcome != "decided" {
            return Err(PgLeaveError::Domain(KernelError::internal(
                "leave decision routine returned an unknown outcome",
            )));
        }
        self.fetch_request_scoped(org, request_id, &command.branch_scope)
            .await?
            .ok_or_else(|| {
                PgLeaveError::Domain(KernelError::internal(
                    "leave command committed but decided request was not readable",
                ))
            })
    }

    /// Worker-only alternate 시기 on a `time_change_consult` row (§60⑤).
    pub async fn propose_alternate_dates(
        &self,
        command: ProposeAlternateDatesCommand,
    ) -> Result<LeaveRequestView, PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let existing = self
            .fetch_request_for_requester(org, command.request_id, command.proposer)
            .await?
            .ok_or_else(|| KernelError::not_found("leave request not found"))?;
        if existing.status != LeaveStatus::TimeChangeConsult {
            return Err(KernelError::conflict(
                "alternate dates may only be proposed on a time_change_consult request",
            )
            .into());
        }
        if existing.requester_user_id != command.proposer {
            return Err(KernelError::forbidden(
                "alternate dates may only be proposed by the original requester",
            )
            .into());
        }
        let original = NewLeaveRequest::new(
            existing.leave_type,
            existing.start_date,
            existing.end_date,
            existing.partial_day_period,
        )?;
        let proposed = alternate_leave_request(
            existing.leave_type,
            command.start_date,
            command.end_date,
            existing.partial_day_period,
        )?;
        validate_alternate_dates(&original, proposed)?;

        let row = sqlx::query(
            "SELECT * FROM leave_api.propose_alternate_dates(\
             $1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(org.as_uuid())
        .bind(command.request_id.as_uuid())
        .bind(command.proposer.as_uuid())
        .bind(command.start_date)
        .bind(command.end_date)
        .bind(command.trace.trace_id())
        .bind(command.trace.span_id())
        .fetch_one(self.command_pool()?)
        .await
        .map_err(map_leave_command_sqlx)?;
        let request_id = LeaveRequestId::from_uuid(row.try_get("request_id")?);
        self.fetch_request_for_requester(org, request_id, command.proposer)
            .await?
            .ok_or_else(|| {
                PgLeaveError::Domain(KernelError::internal(
                    "leave command committed but consult request was not readable",
                ))
            })
    }

    async fn fetch_request_for_requester(
        &self,
        org: OrgId,
        id: LeaveRequestId,
        requester: UserId,
    ) -> Result<Option<LeaveRequestView>, PgLeaveError> {
        let id_uuid = *id.as_uuid();
        let requester_uuid = *requester.as_uuid();
        let row = with_org_conn::<_, _, PgLeaveError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let mut builder = QueryBuilder::<Postgres>::new(format!(
                    "SELECT {REQUEST_COLUMNS} FROM leave_requests WHERE id = "
                ));
                builder.push_bind(id_uuid);
                builder.push(" AND requester_user_id = ");
                builder.push_bind(requester_uuid);
                Ok(builder.build().fetch_optional(tx.as_mut()).await?)
            })
        })
        .await?;
        row.as_ref().map(request_from_row).transpose()
    }

    async fn fetch_request_scoped(
        &self,
        org: OrgId,
        id: LeaveRequestId,
        branch_scope: &BranchScope,
    ) -> Result<Option<LeaveRequestView>, PgLeaveError> {
        let id_uuid = *id.as_uuid();
        let branch_scope = branch_scope.clone();
        let row = with_org_conn::<_, _, PgLeaveError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let mut builder = QueryBuilder::<Postgres>::new(format!(
                    "SELECT {REQUEST_COLUMNS} FROM leave_requests WHERE id = "
                ));
                builder.push_bind(id_uuid);
                builder.push(" AND ");
                push_branch_scope(&mut builder, &branch_scope);
                Ok(builder.build().fetch_optional(tx.as_mut()).await?)
            })
        })
        .await?;
        row.as_ref().map(request_from_row).transpose()
    }

    // -----------------------------------------------------------------------
    // Balances roster (reads the existing employees leave ledger; no duplicate)
    // -----------------------------------------------------------------------

    pub async fn list_balances(
        &self,
        branch_scope: BranchScope,
    ) -> Result<LeaveBalancePage, PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgLeaveError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let mut builder = QueryBuilder::<Postgres>::new(
                    "SELECT id, name, org_unit, \
                            COALESCE(leave_accrued, 0)::float8 AS grant_days, \
                            COALESCE(leave_used, 0)::float8 AS used_days, \
                            COALESCE(leave_remaining, 0)::float8 AS left_days \
                     FROM employees WHERE (leave_accrued IS NOT NULL \
                        OR leave_used IS NOT NULL OR leave_remaining IS NOT NULL) AND ",
                );
                push_employee_branch_scope(&mut builder, &branch_scope);
                builder.push(" ORDER BY name ASC, id ASC");
                Ok(builder.build().fetch_all(tx.as_mut()).await?)
            })
        })
        .await?;
        let items = rows
            .iter()
            .map(balance_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(LeaveBalancePage { items })
    }

    // -----------------------------------------------------------------------
    // §61 statutory push
    // -----------------------------------------------------------------------

    /// Verify the statutory-push target is the employee linked to the target
    /// user and that the user is assigned to the branch being managed.
    pub async fn verify_statutory_push_target(
        &self,
        branch_id: uuid::Uuid,
        target_user_id: UserId,
        target_employee_id: uuid::Uuid,
    ) -> Result<(), PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let user_id = *target_user_id.as_uuid();
        let matches_target = with_org_conn::<_, _, PgLeaveError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(
                         SELECT 1
                         FROM users u
                         JOIN user_branches ub
                           ON ub.user_id = u.id
                          AND ub.org_id = u.org_id
                         JOIN employees e
                           ON e.id = u.employee_id
                          AND e.org_id = u.org_id
                         WHERE u.id = $1
                           AND u.employee_id = $2
                           AND ub.branch_id = $3
                           AND e.id = $2
                     )",
                )
                .bind(user_id)
                .bind(target_employee_id)
                .bind(branch_id)
                .fetch_one(tx.as_mut())
                .await?)
            })
        })
        .await?;

        if matches_target {
            Ok(())
        } else {
            Err(KernelError::forbidden(
                "statutory-push target user/employee must match and belong to the target branch",
            )
            .into())
        }
    }

    /// Serve a §61 promotion (1차/2차) or a 노무수령거부 notice: deliver the
    /// receipt-gated document into the target's 개인 수신함 and record the push.
    ///
    /// Every step is judged against 근로기준법 제61조 by
    /// [`console_leave_domain::promotion`] before anything is delivered: round 1
    /// must fall inside its statutory window, round 2 must follow a recorded
    /// round 1 by more than the worker's 10-day reply period and land on or
    /// before the 2차 통보 기한, and a 노무수령거부 must follow a recorded round-2
    /// designation. A push that misses its window is refused rather than
    /// delivered as a legally void notice.
    ///
    /// Idempotent per `(org, target, kind, round)` **within one 연차 사용기간** —
    /// a push for a different period collides on the current unique key and is
    /// refused (fail closed) until `leave_promotions` carries the period; see
    /// `docs/evidence/console/hotfix/leave-promotion/manifests/`.
    pub async fn statutory_push(
        &self,
        command: StatutoryPushCommand,
    ) -> Result<StatutoryPushView, PgLeaveError> {
        let org = current_org().map_err(KernelError::from)?;
        let notice_type = command.kind.notice_type();
        let legal_basis = command.kind.legal_basis();
        let kind = command.kind.as_str();
        // §61 deadlines are org-local. Korea has observed a fixed +09:00 since
        // 1988, so the business date is the UTC instant at that offset.
        let served_on = command.occurred_at.to_offset(offset!(+9)).date();
        let tag = period_tag(command.track, command.leave_period_end);

        // §61①1 requires the notice to state the employee's own 미사용 휴가
        // 일수. It is read from the authoritative roster, never from the caller.
        let unused_days = self
            .unused_leave_days(org, command.target_employee_id)
            .await?;

        // The slot this push occupies: a refusal is stored against the round it
        // follows.
        let slot_round = if command.kind == PromotionKind::Refusal {
            2
        } else {
            command.round
        };
        let dedup_key = promotion_dedup_key(kind, command.target_employee_id, slot_round, &tag);

        if let Some(existing) = self
            .find_promotion(org, command.target_employee_id, kind, slot_round)
            .await?
        {
            ensure_same_period(&existing, &dedup_key, "이 단계의 §61 통지가")?;
            return Ok(StatutoryPushView {
                id: existing.id,
                kind: command.kind,
                round: slot_round,
                target_user_id: command.target_user_id,
                inbox_doc_id: existing.inbox_doc_id,
                ap_run_id: None,
                ap_submission: ApSubmission::PendingEngineDefinition,
            });
        }

        // Only the predecessor this step actually depends on is read, and it
        // must belong to the same 연차 사용기간.
        let predecessor = match (command.kind, command.round) {
            (PromotionKind::Promotion, 2) => {
                let record = self
                    .find_promotion(org, command.target_employee_id, "promotion", 1)
                    .await?;
                if let Some(record) = record.as_ref() {
                    let expected =
                        promotion_dedup_key("promotion", command.target_employee_id, 1, &tag);
                    ensure_same_period(record, &expected, "1차 촉구가")?;
                }
                record
            }
            (PromotionKind::Refusal, _) => {
                let record = self
                    .find_promotion(org, command.target_employee_id, "promotion", 2)
                    .await?;
                if let Some(record) = record.as_ref() {
                    let expected =
                        promotion_dedup_key("promotion", command.target_employee_id, 2, &tag);
                    ensure_same_period(record, &expected, "2차 통보가")?;
                }
                record
            }
            _ => None,
        };

        let context = PromotionContext {
            track: command.track,
            period_end: command.leave_period_end,
            served_on,
            first_round_served_on: match (command.kind, command.round) {
                (PromotionKind::Promotion, 2) => predecessor.as_ref().map(|r| r.served_on),
                _ => None,
            },
            second_round_served_on: match command.kind {
                PromotionKind::Refusal => predecessor.as_ref().map(|r| r.served_on),
                PromotionKind::Promotion => None,
            },
        };
        let round =
            validate_push(command.kind, command.round, &context).map_err(PgLeaveError::Domain)?;

        // §61①2 / §61②2 require the 2차 통보 to *designate the dates*. A refusal
        // refuses labour on exactly the dates that designation named, read back
        // from the recorded notice rather than restated by the caller.
        let designated_dates = match (command.kind, round) {
            (PromotionKind::Promotion, 2) => {
                validate_designated_dates(
                    &command.designated_dates,
                    served_on,
                    command.leave_period_end,
                )
                .map_err(PgLeaveError::Domain)?;
                command.designated_dates.clone()
            }
            (PromotionKind::Refusal, _) => {
                if !command.designated_dates.is_empty() {
                    return Err(KernelError::validation(
                        "노무수령거부 통지는 2차 통보에 기록된 지정 사용 시기를 그대로 \
                         인용합니다 — 별도의 지정 날짜를 받지 않습니다",
                    )
                    .into());
                }
                let recorded = predecessor
                    .as_ref()
                    .map(|record| recorded_designated_dates(&record.payload))
                    .transpose()?
                    .unwrap_or_default();
                if recorded.is_empty() {
                    return Err(KernelError::conflict(
                        "기록된 2차 통보에 지정된 사용 시기가 없어 노무수령거부를 \
                         특정할 수 없습니다 (근로기준법 제61조제1항제2호)",
                    )
                    .into());
                }
                recorded
            }
            _ => {
                if !command.designated_dates.is_empty() {
                    return Err(KernelError::validation(
                        "1차 촉구는 사용 시기를 지정하지 않습니다 — 근로자가 정하여 \
                         회신합니다 (근로기준법 제61조제1항제1호)",
                    )
                    .into());
                }
                Vec::new()
            }
        };

        let statutory_basis =
            statutory_basis_snapshot(&command, round, served_on, &designated_dates)?;
        let (title, body) = notice_body(&command, round, &unused_days, &designated_dates);
        let doc = NewInboxDoc::new(
            InboxDocKind::LegalNotice,
            &title,
            Some(notice_type),
            Some(legal_basis),
            Some("leave_promotion"),
            // Deterministic source id ties the notice back to the push row and
            // pins it to one 연차 사용기간.
            Some(&dedup_key),
            body,
        )
        .map_err(PgLeaveError::Domain)?;

        // 1. Deliver the receipt-gated notice (own audited, idempotent tx).
        let emitted = self
            .inbox
            .emit(EmitInboxDocCommand {
                actor: Some(command.actor),
                recipient: command.target_user_id,
                doc,
                dedup_key: Some(dedup_key),
                trace: command.trace.clone(),
                occurred_at: command.occurred_at,
            })
            .await
            .map_err(PgLeaveError::Domain)?;

        // 2. Record the push, referencing the delivered notice. The engine AP-
        //    run stays NULL until the 연차촉진 submittable definition exists
        //    (gap #1); we never fabricate a run.
        let id = LeavePromotionId::new();
        let inbox_doc_id = *emitted.id.as_uuid();
        let target_user = *command.target_user_id.as_uuid();
        let actor = *command.actor.as_uuid();
        let event = leave_promotion_audit_event(
            "leave_promotion.push",
            Some(command.actor),
            id,
            command.trace,
            command.occurred_at,
        )?
        .with_branch(console_kernel_core::BranchId::from_uuid(command.branch_id))
        .with_org(org)
        .with_snapshots(
            None,
            Some(serde_json::json!({
                "kind": command.kind.as_str(),
                "round": round,
                "target_user_id": target_user,
                "inbox_doc_id": inbox_doc_id,
                "legal_basis": legal_basis,
                "unused_days": unused_days,
                "statutory_basis": statutory_basis,
                "ap_submission": "pending_engine_definition",
            })),
        );

        let branch_id = command.branch_id;
        let target_employee_id = command.target_employee_id;
        let existing_id =
            with_audit::<_, PromotionInsert, PgLeaveError>(&self.pool, event, move |tx| {
                Box::pin(async move {
                    let inserted = sqlx::query(
                        "INSERT INTO leave_promotions \
                         (id, org_id, branch_id, target_user_id, target_employee_id, kind, round, \
                          inbox_doc_id, legal_basis, created_by) \
                         VALUES ($1, NULLIF(current_setting('app.current_org', true), '')::uuid, \
                                 $2, $3, $4, $5, $6, $7, $8, $9) \
                         ON CONFLICT (org_id, target_employee_id, kind, round) DO NOTHING \
                         RETURNING id",
                    )
                    .bind(id.as_uuid())
                    .bind(branch_id)
                    .bind(target_user)
                    .bind(target_employee_id)
                    .bind(kind)
                    .bind(round)
                    .bind(inbox_doc_id)
                    .bind(legal_basis)
                    .bind(actor)
                    .fetch_optional(tx.as_mut())
                    .await?;
                    Ok(match inserted {
                        Some(_) => PromotionInsert::Inserted,
                        None => PromotionInsert::Duplicate,
                    })
                })
            })
            .await;

        let push = match existing_id {
            Ok(PromotionInsert::Inserted) => (id, inbox_doc_id),
            // Duplicate push: return the already-recorded row idempotently.
            Ok(PromotionInsert::Duplicate) => self
                .find_promotion(org, target_employee_id, kind, round)
                .await?
                .map(|record| (record.id, record.inbox_doc_id))
                .ok_or_else(|| {
                    KernelError::internal("duplicate push but no existing promotion row")
                })?,
            Err(other) => return Err(other),
        };

        Ok(StatutoryPushView {
            id: push.0,
            kind: command.kind,
            round,
            target_user_id: command.target_user_id,
            inbox_doc_id: push.1,
            ap_run_id: None,
            ap_submission: ApSubmission::PendingEngineDefinition,
        })
    }

    /// The employee's 미사용 연차 일수, exactly as the roster records it
    /// (`employees.leave_remaining NUMERIC(16,6)` since migration 0166, rendered
    /// as text — never a float on a legal notice). `trim_scale` drops the storage
    /// scale's trailing zeros without changing the value, so the notice served to
    /// a worker reads `13일` / `13.5일`, not `13.000000일`.
    /// Doubles as the target-exists check: a missing employee is `not_found`,
    /// an employee whose roster figure was never established is a conflict, and
    /// neither ever becomes a fabricated zero.
    async fn unused_leave_days(
        &self,
        org: OrgId,
        target_employee_id: uuid::Uuid,
    ) -> Result<String, PgLeaveError> {
        let row = with_org_conn::<_, _, PgLeaveError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query_scalar::<_, Option<String>>(
                    "SELECT trim_scale(leave_remaining)::text FROM employees WHERE id = $1",
                )
                .bind(target_employee_id)
                .fetch_optional(tx.as_mut())
                .await?)
            })
        })
        .await?;
        match row {
            None => {
                Err(KernelError::not_found("target employee not found for leave promotion").into())
            }
            Some(None) => Err(KernelError::conflict(
                "대상 근로자의 미사용 연차 일수가 확정되지 않아 §61 통지를 발송할 수 \
                 없습니다 (근로기준법 제61조제1항제1호는 미사용 휴가 일수의 고지를 \
                 요구합니다)",
            )
            .into()),
            Some(Some(days)) => Ok(days),
        }
    }

    async fn find_promotion(
        &self,
        org: OrgId,
        target_employee_id: uuid::Uuid,
        kind: &str,
        round: i16,
    ) -> Result<Option<PromotionRecord>, PgLeaveError> {
        let kind = kind.to_owned();
        let row = with_org_conn::<_, _, PgLeaveError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(
                    "SELECT p.id, p.inbox_doc_id, \
                            (p.created_at AT TIME ZONE 'Asia/Seoul')::date AS served_on, \
                            d.source_id, d.payload \
                       FROM leave_promotions p \
                       JOIN inbox_docs d ON d.id = p.inbox_doc_id AND d.org_id = p.org_id \
                      WHERE p.target_employee_id = $1 AND p.kind = $2 AND p.round = $3",
                )
                .bind(target_employee_id)
                .bind(kind)
                .bind(round)
                .fetch_optional(tx.as_mut())
                .await?)
            })
        })
        .await?;
        row.map(|row| {
            Ok(PromotionRecord {
                id: LeavePromotionId::from_uuid(row.try_get("id")?),
                inbox_doc_id: row.try_get("inbox_doc_id")?,
                served_on: row.try_get("served_on")?,
                source_id: row.try_get("source_id")?,
                payload: row.try_get("payload")?,
            })
        })
        .transpose()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn map_leave_command_sqlx(error: sqlx::Error) -> PgLeaveError {
    let message = error
        .as_database_error()
        .map(sqlx::error::DatabaseError::message)
        .unwrap_or_default();
    match message {
        "leave_create.home_branch_required" => PgLeaveError::MissingHomeBranch,
        "leave_resolve.concurrent_modification"
        | "leave_decide.concurrent_modification"
        | "leave_home_branch.concurrent_modification"
        | "leave_balance_import.concurrent_modification" => PgLeaveError::ConcurrentModification,
        "leave_resolve.not_found"
        | "leave_decide.not_found"
        | "leave_home_branch.employee_not_found"
        | "leave_balance_import.employee_not_found"
        | "employee_import_batch.run_not_found"
        | "leave_home_branch.active_branch_required"
        | "employee_create.active_branch_required" => {
            PgLeaveError::Domain(KernelError::not_found(message.to_owned()))
        }
        "leave_resolve.not_pending"
        | "leave_decide.not_pending"
        | "employee_import_batch.run_not_dry_run"
        | "leave_create.idempotency_conflict"
        | "employee_create.idempotency_conflict"
        | "employee_create.employee_number_conflict"
        | "leave_balance_import.idempotency_conflict"
        | "leave_home_branch.already_assigned" => {
            PgLeaveError::Domain(KernelError::conflict(message.to_owned()))
        }
        "leave_write.actor_forbidden"
        | "leave_write.branch_forbidden"
        | "leave_write.org_admin_required"
        | "leave_balance_import.actor_forbidden"
        | "employee_create.actor_forbidden"
        | "employee_create.home_branch_create_required"
        | "leave_create.self_employee_required"
        | "leave_resolve.requester_forbidden"
        | "leave_decide.requester_forbidden"
        | "leave_decide.resolver_forbidden" => {
            PgLeaveError::Domain(KernelError::forbidden(message.to_owned()))
        }
        "leave_decide.insufficient_balance" => PgLeaveError::Domain(KernelError::validation(
            "insufficient exact leave balance for approval",
        )),
        "leave_decide.time_change_ineligible" => PgLeaveError::Domain(KernelError::validation(
            "time_change is ineligible: branch coverage evidence does not meet §60⑤ 요건",
        )),
        "leave_alternate.not_found" => {
            PgLeaveError::Domain(KernelError::not_found("leave request not found"))
        }
        "leave_alternate.not_consult" | "leave_alternate.already_proposed" => {
            PgLeaveError::Domain(KernelError::conflict(message.to_owned()))
        }
        "leave_alternate.requester_only" => PgLeaveError::Domain(KernelError::forbidden(
            "alternate dates may only be proposed by the original requester",
        )),
        message
            if message.starts_with("leave_create.")
                || message.starts_with("employee_create.")
                || message.starts_with("leave_charge.")
                || message.starts_with("leave_resolve.")
                || message.starts_with("leave_decide.")
                || message.starts_with("leave_alternate.")
                || message.starts_with("leave_home_branch.")
                || message.starts_with("leave_balance_import.")
                || message.starts_with("employee_import_batch.")
                || message.starts_with("leave_write.") =>
        {
            PgLeaveError::Domain(KernelError::validation(message.to_owned()))
        }
        _ => PgLeaveError::from(error),
    }
}

/// Narrow a query to the caller's branch scope. `BranchScope::All` sees every
/// branch-scoped row; an empty `Branches` set matches nothing (deny-by-omission).
fn push_branch_scope(builder: &mut QueryBuilder<Postgres>, scope: &BranchScope) {
    match scope {
        BranchScope::All => {
            builder.push("TRUE");
        }
        BranchScope::Branches(branches) if branches.is_empty() => {
            builder.push("FALSE");
        }
        BranchScope::Branches(branches) => {
            let ids = branches.iter().map(|b| *b.as_uuid()).collect::<Vec<_>>();
            builder.push("branch_id = ANY(");
            builder.push_bind(ids);
            builder.push(")");
        }
    }
}

fn push_employee_branch_scope(builder: &mut QueryBuilder<Postgres>, scope: &BranchScope) {
    match scope {
        BranchScope::All => {
            builder.push("TRUE");
        }
        BranchScope::Branches(branches) if branches.is_empty() => {
            builder.push("FALSE");
        }
        BranchScope::Branches(branches) => {
            let ids = branches
                .iter()
                .map(|branch| *branch.as_uuid())
                .collect::<Vec<_>>();
            builder.push("home_branch_id = ANY(");
            builder.push_bind(ids);
            builder.push(")");
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn canonical_snapshot(
    home_branch_id: uuid::Uuid,
    leave_type: LeaveType,
    partial_day_period: Option<PartialDayPeriod>,
    start_date: Date,
    end_date: Date,
    mut date_charges: Vec<LeaveDateCharge>,
    calendar_revision_ref: SourceRevisionRef,
    policy_revision_ref: SourceRevisionRef,
    supporting_source_refs: Vec<SourceRevisionRef>,
) -> Result<RecordedLeaveChargeSnapshot, PgLeaveError> {
    date_charges.sort_by_key(|charge| charge.date);
    let mut expected_date = start_date;
    let mut total = LeaveUnits::ZERO;
    for charge in &date_charges {
        if charge.date != expected_date {
            return Err(KernelError::validation(
                "charge evidence must contain every requested date exactly once",
            )
            .into());
        }
        match &charge.obligation {
            WorkObligation::Scheduled { minutes } if *minutes == 0 => {
                return Err(KernelError::validation(
                    "scheduled work obligation must contain positive minutes",
                )
                .into());
            }
            WorkObligation::Scheduled { .. } if charge.units == LeaveUnits::ZERO => {
                return Err(KernelError::validation(
                    "scheduled work obligation must carry an exact positive charge",
                )
                .into());
            }
            WorkObligation::Scheduled { .. } if charge.units > LeaveUnits::ONE_DAY => {
                return Err(KernelError::validation(
                    "a scheduled date cannot charge more than one leave day",
                )
                .into());
            }
            WorkObligation::NotScheduled { .. } if charge.units != LeaveUnits::ZERO => {
                return Err(KernelError::validation(
                    "a non-scheduled date must carry zero leave units",
                )
                .into());
            }
            WorkObligation::Scheduled { .. } | WorkObligation::NotScheduled { .. } => {}
        }
        total = total.checked_add(charge.units)?;
        expected_date = expected_date.next_day().ok_or_else(|| {
            KernelError::validation("leave date range exceeds the supported calendar")
        })?;
    }
    if date_charges.is_empty() || expected_date.previous_day() != Some(end_date) {
        return Err(KernelError::validation(
            "charge evidence must cover the complete requested date range",
        )
        .into());
    }
    match leave_type {
        LeaveType::Annual => {
            if partial_day_period.is_some()
                || date_charges.iter().any(|charge| {
                    matches!(charge.obligation, WorkObligation::Scheduled { .. })
                        && charge.units != LeaveUnits::ONE_DAY
                })
            {
                return Err(KernelError::validation(
                    "annual leave must charge each scheduled date exactly one day",
                )
                .into());
            }
        }
        LeaveType::HalfDay => {
            let scheduled = date_charges
                .iter()
                .filter(|charge| matches!(charge.obligation, WorkObligation::Scheduled { .. }))
                .collect::<Vec<_>>();
            if partial_day_period.is_none()
                || start_date != end_date
                || scheduled.len() != 1
                || scheduled[0].units == LeaveUnits::ZERO
                || scheduled[0].units >= LeaveUnits::ONE_DAY
            {
                return Err(KernelError::validation(
                    "half-day leave requires one policy-pinned scheduled date with an exact fraction below one day",
                )
                .into());
            }
        }
    }
    if total == LeaveUnits::ZERO {
        return Err(KernelError::validation(
            "resolved leave charge must total more than zero units",
        )
        .into());
    }
    if total > LeaveUnits::from_micros(366_000_000)? {
        return Err(
            KernelError::validation("resolved leave charge must not exceed 366 days").into(),
        );
    }
    let canonical = serde_json::to_vec(&(
        home_branch_id,
        leave_type,
        partial_day_period,
        &calendar_revision_ref,
        &policy_revision_ref,
        &supporting_source_refs,
        &date_charges,
        total.micros(),
    ))
    .map_err(|error| KernelError::internal(error.to_string()))?;
    let server_digest = hex::encode(Sha256::digest(canonical));
    Ok(RecordedLeaveChargeSnapshot {
        home_branch_id,
        leave_type,
        partial_day_period,
        calendar_revision_ref,
        policy_revision_ref,
        supporting_source_refs,
        date_charges,
        total_units: total,
        server_digest,
    })
}

fn request_from_row(row: &sqlx::postgres::PgRow) -> Result<LeaveRequestView, PgLeaveError> {
    let decided_by: Option<uuid::Uuid> = row.try_get("decided_by")?;
    let charge_resolved_by: Option<uuid::Uuid> = row.try_get("charge_resolved_by")?;
    let charge_resolution_origin: Option<String> = row.try_get("charge_resolution_origin")?;
    let ap_run_id: Option<uuid::Uuid> = row.try_get("ap_run_id")?;
    let charge_micros: Option<i64> = row.try_get("charge_micros")?;
    let review_reasons: Vec<String> = row.try_get("charge_review_reasons")?;
    Ok(LeaveRequestView {
        id: LeaveRequestId::from_uuid(row.try_get("id")?),
        branch_id: row.try_get("branch_id")?,
        requester_user_id: UserId::from_uuid(row.try_get("requester_user_id")?),
        subject_employee_id: row.try_get("subject_employee_id")?,
        leave_type: LeaveType::parse(row.try_get::<String, _>("leave_type")?.as_str())?,
        days: row.try_get("days")?,
        charge_units: charge_micros.map(LeaveUnits::from_micros).transpose()?,
        charge_state: LeaveChargeState::parse(row.try_get::<String, _>("charge_state")?.as_str())?,
        charge_review_reasons: review_reasons
            .iter()
            .map(|reason| LeaveChargeReviewReason::parse(reason))
            .collect::<Result<Vec<_>, _>>()?,
        request_version: row.try_get("request_version")?,
        charge_version: row.try_get("charge_version")?,
        charge_digest: row.try_get::<Option<String>, _>("charge_digest")?,
        charge_resolved_by: charge_resolved_by.map(UserId::from_uuid),
        charge_resolution_origin: charge_resolution_origin
            .as_deref()
            .map(LeaveChargeResolutionOrigin::parse)
            .transpose()?,
        partial_day_period: row
            .try_get::<Option<String>, _>("partial_day_period")?
            .map(|period| console_leave_domain::PartialDayPeriod::parse(&period))
            .transpose()?,
        start_date: row.try_get::<Date, _>("start_date")?,
        end_date: row.try_get::<Date, _>("end_date")?,
        reason: row.try_get("reason")?,
        status: LeaveStatus::parse(row.try_get::<String, _>("status")?.as_str())?,
        decided_by: decided_by.map(UserId::from_uuid),
        decided_at: row.try_get("decided_at")?,
        decision_comment: row.try_get("decision_comment")?,
        time_change_grounds: row
            .try_get::<Option<String>, _>("time_change_grounds")?
            .map(|code| TimeChangeGroundsCode::parse(&code))
            .transpose()?,
        time_change_evidence: row.try_get("time_change_evidence")?,
        alternate_start_date: row.try_get("alternate_start_date")?,
        alternate_end_date: row.try_get("alternate_end_date")?,
        alternate_partial_day_period: row
            .try_get::<Option<String>, _>("alternate_partial_day_period")?
            .map(|period| PartialDayPeriod::parse(&period))
            .transpose()?,
        alternate_proposed_at: row.try_get("alternate_proposed_at")?,
        ap_run_id,
        created_at: row.try_get("created_at")?,
    })
}

fn optional_units(
    row: &sqlx::postgres::PgRow,
    column: &str,
) -> Result<Option<LeaveBalanceAmount>, PgLeaveError> {
    row.try_get::<Option<i64>, _>(column)?
        .map(LeaveBalanceAmount::from_micros)
        .transpose()
        .map_err(PgLeaveError::Domain)
}

fn balance_from_row(row: &sqlx::postgres::PgRow) -> Result<LeaveBalanceView, PgLeaveError> {
    let grant: f64 = row.try_get("grant_days")?;
    let used: f64 = row.try_get("used_days")?;
    let left: f64 = row.try_get("left_days")?;
    Ok(LeaveBalanceView {
        employee_id: row.try_get("id")?,
        name: row.try_get("name")?,
        team: row.try_get("org_unit")?,
        grant,
        used,
        left,
        tone: balance_tone(grant, used, left),
    })
}

/// Bucket a balance for the bar color + 촉진 flag, mirroring the prototype:
/// mostly-unused (used < 50% of grant) ⇒ `promote` (촉진 대상); nearly-exhausted
/// (≤ 2 days left) ⇒ `low`; otherwise `ok`.
// ponytail: fixed thresholds; move to org policy if HR wants them configurable.
fn balance_tone(grant: f64, used: f64, left: f64) -> LeaveBalanceTone {
    if grant > 0.0 && used / grant < 0.5 {
        LeaveBalanceTone::Promote
    } else if left <= 2.0 {
        LeaveBalanceTone::Low
    } else {
        LeaveBalanceTone::Ok
    }
}

/// Render the statutory notice title + JSONB body delivered into the inbox.
/// The tag that binds a push to one 연차 사용기간. Until `leave_promotions`
/// carries the period itself, this is the only thing separating this year's
/// 촉진 from last year's on the per-employee unique key.
fn period_tag(track: PromotionTrack, period_end: Date) -> String {
    format!("{}-{period_end}", track.as_str())
}

fn promotion_dedup_key(
    kind: &str,
    target_employee_id: uuid::Uuid,
    round: i16,
    tag: &str,
) -> String {
    format!("leave-{kind}-{target_employee_id}-r{round}-{tag}")
}

/// Fail closed when a recorded push belongs to a *different* 연차 사용기간 than
/// the one being pushed. The `leave_promotions` unique key is per employee, not
/// per period, so without this check a second year's 촉진 would silently return
/// last year's row — a wrong answer wearing a success status.
fn ensure_same_period(
    record: &PromotionRecord,
    expected_source_id: &str,
    label: &str,
) -> Result<(), PgLeaveError> {
    if record.source_id.as_deref() == Some(expected_source_id) {
        return Ok(());
    }
    Err(KernelError::conflict(format!(
        "{label}다른 연차 사용기간으로 이미 기록되어 있습니다. 현재 스키마는 근로자당 \
         하나의 사용기간만 보관할 수 있어 이 통지를 발송할 수 없습니다 \
         (docs/evidence/console/hotfix/leave-promotion/manifests/)"
    ))
    .into())
}

/// Read the 사용 시기 a recorded 2차 통보 designated. The refusal notice must
/// refuse labour on exactly those days, so they are read back from the notice
/// rather than restated.
fn recorded_designated_dates(payload: &serde_json::Value) -> Result<Vec<Date>, PgLeaveError> {
    let Some(values) = payload.get("designated_dates").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .and_then(|text| {
                    Date::parse(text, &time::format_description::well_known::Iso8601::DATE).ok()
                })
                .ok_or_else(|| {
                    PgLeaveError::from(KernelError::internal(
                        "기록된 2차 통보의 지정 사용 시기를 해석할 수 없습니다",
                    ))
                })
        })
        .collect()
}

/// The §61 arithmetic this push was judged against, recorded in the audit
/// snapshot so the window can be re-derived from the trail alone.
fn statutory_basis_snapshot(
    command: &StatutoryPushCommand,
    round: i16,
    served_on: Date,
    designated_dates: &[Date],
) -> Result<serde_json::Value, PgLeaveError> {
    let mut basis = serde_json::json!({
        "statute": command.track.statute_paragraph(),
        "track": command.track.as_str(),
        "leave_period_end": command.leave_period_end.to_string(),
        "served_on": served_on.to_string(),
        "designated_dates": iso_dates(designated_dates),
    });
    match (command.kind, round) {
        (PromotionKind::Promotion, 1) => {
            let window = first_round_window(command.track, command.leave_period_end)
                .map_err(PgLeaveError::Domain)?;
            basis["first_round_window"] = serde_json::json!({
                "opens_on": window.opens_on.to_string(),
                "closes_on": window.closes_on.to_string(),
            });
        }
        (PromotionKind::Promotion, _) => {
            let deadline = second_round_deadline(command.track, command.leave_period_end)
                .map_err(PgLeaveError::Domain)?;
            basis["second_round_deadline"] = serde_json::json!(deadline.to_string());
        }
        (PromotionKind::Refusal, _) => {
            basis["refusal_authority"] = serde_json::json!(
                "대법원 2019다279283 (2020. 2. 27. 선고) — 노무수령 거부의사의 명확한 표시"
            );
        }
    }
    Ok(basis)
}

fn iso_dates(dates: &[Date]) -> Vec<String> {
    dates.iter().map(ToString::to_string).collect()
}

/// The body of a §61 notice.
///
/// Each round states only what the statute requires it to state, and the
/// refusal states only what the employer is *doing*. The previous refusal body
/// asserted "본 통지로써 해당 연차에 대한 사용자의 금전 보상 의무가 소멸함" — a legal
/// conclusion this system cannot reach: §61 relief additionally requires that
/// the worker did not in fact work the designated day and that the employer
/// maintained the refusal (대법원 2019다279283). That conclusion is now stated as
/// the condition it is, not as an accomplished fact.
fn notice_body(
    command: &StatutoryPushCommand,
    round: i16,
    unused_days: &str,
    designated_dates: &[Date],
) -> (String, serde_json::Value) {
    let legal_basis = command.kind.legal_basis();
    let statute = command.track.statute_paragraph();
    let period_end = command.leave_period_end;
    match (command.kind, round) {
        (PromotionKind::Promotion, 1) => (
            "연차 사용 촉진 통지 (1차 촉구)".to_owned(),
            serde_json::json!({
                "kind": "promotion",
                "round": 1,
                "target": command.target_name,
                "unused_days": unused_days,
                "leave_period_end": period_end.to_string(),
                "legal_basis": format!("{statute}제1호"),
                "paragraphs": [
                    format!(
                        "{}. 기준일 현재 귀하의 미사용 연차 유급휴가는 {unused_days}일입니다.",
                        command.target_name
                    ),
                    format!(
                        "위 휴가는 {period_end}까지 사용하지 아니하면 소멸됩니다 \
                         (근로기준법 제60조제7항)."
                    ),
                    format!(
                        "{statute}제1호에 따라, 본 촉구를 받은 날부터 10일 이내에 \
                         미사용 휴가의 사용 시기를 정하여 서면으로 회신하여 주시기 바랍니다."
                    ),
                    "회신이 없으면 사용자가 사용 시기를 지정하여 다시 서면으로 통보합니다."
                        .to_owned(),
                ],
            }),
        ),
        (PromotionKind::Promotion, _) => (
            "연차 사용 촉진 통지 (2차 사용 시기 지정)".to_owned(),
            serde_json::json!({
                "kind": "promotion",
                "round": 2,
                "target": command.target_name,
                "unused_days": unused_days,
                "leave_period_end": period_end.to_string(),
                "designated_dates": iso_dates(designated_dates),
                "legal_basis": format!("{statute}제2호"),
                "paragraphs": [
                    // The notice designates the days it lists — no more. Saying
                    // it designates "미사용 연차 {unused_days}일의 사용 시기"
                    // while listing two dates would be a claim the payload
                    // itself contradicts.
                    format!(
                        "{}. 1차 촉구에 대한 회신이 없어, {statute}제2호에 따라 사용자가 \
                         미사용 연차 {unused_days}일 중 아래 날짜에 대하여 사용 시기를 \
                         지정하여 통보합니다.",
                        command.target_name
                    ),
                    format!("지정 사용 시기: {}", iso_dates(designated_dates).join(", ")),
                    "지정된 날에는 근로 제공 의무가 없으므로 출근하지 마시기 바랍니다."
                        .to_owned(),
                ],
            }),
        ),
        (PromotionKind::Refusal, _) => (
            "노무수령거부 통지".to_owned(),
            serde_json::json!({
                "kind": "refusal",
                "round": 2,
                "target": command.target_name,
                "unused_days": unused_days,
                "leave_period_end": period_end.to_string(),
                "designated_dates": iso_dates(designated_dates),
                "legal_basis": legal_basis,
                "paragraphs": [
                    format!(
                        "{}. 2차 통보로 지정된 아래 휴가일에 대하여 사용자는 노무의 수령을 \
                         거부합니다.",
                        command.target_name
                    ),
                    format!("노무수령 거부일: {}", iso_dates(designated_dates).join(", ")),
                    "해당 일에 출근하시더라도 업무 지시를 하지 않으며, 제공하시는 노무를 \
                     수령하지 않습니다."
                        .to_owned(),
                    format!(
                        "{legal_basis}에 따른 미사용수당 보상 의무의 면제 여부는 촉진 절차의 \
                         적법성과 실제 휴가 미사용 사실에 따라 결정되며, 본 통지가 그 효과를 \
                         확정하지 않습니다 (대법원 2019다279283)."
                    ),
                ],
            }),
        ),
    }
}

#[cfg(test)]
mod exact_charge_tests {
    use super::*;
    use time::Month;

    fn test_date() -> Date {
        Date::from_calendar_date(2026, Month::July, 6).unwrap()
    }

    fn half_day_snapshot(micros: i64, policy_revision: &str) -> RecordedLeaveChargeSnapshot {
        canonical_snapshot(
            uuid::Uuid::new_v4(),
            LeaveType::HalfDay,
            Some(PartialDayPeriod::Am),
            test_date(),
            test_date(),
            vec![LeaveDateCharge {
                date: test_date(),
                obligation: WorkObligation::Scheduled { minutes: 480 },
                units: LeaveUnits::from_micros(micros).unwrap(),
            }],
            SourceRevisionRef::new("calendar", "kr-branch-calendar", "2026.7").unwrap(),
            SourceRevisionRef::new("policy", "collective-agreement", policy_revision).unwrap(),
            Vec::new(),
        )
        .unwrap()
    }

    #[test]
    fn policy_pinned_half_day_fractions_are_not_guessed() {
        for (micros, revision) in [(400_000, "four-tenths"), (500_000, "half")] {
            let snapshot = half_day_snapshot(micros, revision);
            assert_eq!(snapshot.total_units.micros(), micros);
            assert_eq!(snapshot.policy_revision_ref.revision(), revision);
        }
        assert_ne!(
            half_day_snapshot(400_000, "policy-a").server_digest,
            half_day_snapshot(400_000, "other-policy").server_digest
        );
    }

    #[test]
    fn one_date_can_never_charge_hundreds_of_days() {
        let error = canonical_snapshot(
            uuid::Uuid::new_v4(),
            LeaveType::Annual,
            None,
            test_date(),
            test_date(),
            vec![LeaveDateCharge {
                date: test_date(),
                obligation: WorkObligation::Scheduled { minutes: 480 },
                units: LeaveUnits::from_micros(366_000_000).unwrap(),
            }],
            SourceRevisionRef::new("calendar", "kr-branch-calendar", "2026.7").unwrap(),
            SourceRevisionRef::new("policy", "annual-leave", "v1").unwrap(),
            Vec::new(),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Validation);
    }
}

#[cfg(test)]
mod statutory_notice_tests {
    use super::*;
    use console_kernel_core::{TraceContext, UserId};
    use time::macros::{date, datetime};

    fn command(kind: PromotionKind, round: i16, designated: Vec<Date>) -> StatutoryPushCommand {
        StatutoryPushCommand {
            actor: UserId::new(),
            branch_id: uuid::Uuid::new_v4(),
            target_user_id: UserId::new(),
            target_employee_id: uuid::Uuid::new_v4(),
            target_name: "홍길동".to_owned(),
            kind,
            round,
            track: PromotionTrack::Annual,
            leave_period_end: date!(2026 - 12 - 31),
            designated_dates: designated,
            trace: TraceContext::generate(),
            occurred_at: datetime!(2026-10-31 10:00:00 +9),
        }
    }

    /// The fabrication this hotfix removes: the refusal notice used to tell the
    /// worker "본 통지로써 해당 연차에 대한 사용자의 금전 보상 의무가 소멸함". §61
    /// relief also requires that the worker did not in fact work the designated
    /// day and that the employer maintained the refusal (대법원 2019다279283), so
    /// no notice can announce that outcome.
    #[test]
    fn the_refusal_notice_never_announces_that_the_pay_obligation_is_extinguished() {
        let designated = vec![date!(2026 - 12 - 23)];
        let (title, body) = notice_body(
            &command(PromotionKind::Refusal, 2, Vec::new()),
            2,
            "13.5",
            &designated,
        );
        assert_eq!(title, "노무수령거부 통지");
        let text = body["paragraphs"].to_string();
        assert!(!text.contains("소멸함"), "{text}");
        assert!(!text.contains("의무가 소멸"), "{text}");
        assert!(text.contains("2019다279283"), "{text}");
        assert_eq!(
            body["designated_dates"],
            serde_json::json!(["2026-12-23"]),
            "the refusal names the days it refuses labour on"
        );
    }

    #[test]
    fn a_round_two_notice_states_the_designated_dates_it_claims_to_designate() {
        let designated = vec![date!(2026 - 12 - 23), date!(2026 - 12 - 24)];
        let (title, body) = notice_body(
            &command(PromotionKind::Promotion, 2, designated.clone()),
            2,
            "13.5",
            &designated,
        );
        assert_eq!(title, "연차 사용 촉진 통지 (2차 사용 시기 지정)");
        let text = body["paragraphs"].to_string();
        assert!(text.contains("2026-12-23, 2026-12-24"), "{text}");
        assert_eq!(body["unused_days"], serde_json::json!("13.5"));
        // Two designated dates against a 13.5-day balance: the notice must say
        // it designated part of the balance, not all of it. The payload would
        // otherwise contradict its own prose.
        assert!(text.contains("13.5일 중"), "{text}");
        assert!(!text.contains("13.5일의 사용 시기"), "{text}");
        assert_eq!(
            body["legal_basis"],
            serde_json::json!("근로기준법 제61조제1항제2호")
        );
    }

    #[test]
    fn a_round_one_notice_states_the_unused_day_count_and_the_ten_day_reply_duty() {
        let (title, body) = notice_body(
            &command(PromotionKind::Promotion, 1, Vec::new()),
            1,
            "13.5",
            &[],
        );
        assert_eq!(title, "연차 사용 촉진 통지 (1차 촉구)");
        let text = body["paragraphs"].to_string();
        assert!(text.contains("13.5일"), "{text}");
        assert!(text.contains("10일 이내"), "{text}");
        assert!(text.contains("2026-12-31"), "{text}");
    }

    /// The refusal reads its designated days back out of the recorded 2차 통보,
    /// so the two serialisations have to agree.
    #[test]
    fn designated_dates_survive_the_notice_payload_round_trip() {
        let designated = vec![date!(2026 - 12 - 23), date!(2026 - 12 - 24)];
        let (_, body) = notice_body(
            &command(PromotionKind::Promotion, 2, designated.clone()),
            2,
            "13.5",
            &designated,
        );
        assert_eq!(recorded_designated_dates(&body).unwrap(), designated);
        // A notice with no designation reads back empty, never as a fabricated
        // date.
        assert!(
            recorded_designated_dates(&serde_json::json!({}))
                .unwrap()
                .is_empty()
        );
        assert!(
            recorded_designated_dates(&serde_json::json!({"designated_dates": ["not-a-date"]}))
                .is_err()
        );
    }

    #[test]
    fn the_period_tag_separates_two_leave_years_on_the_same_employee() {
        let employee = uuid::Uuid::new_v4();
        let this_year = promotion_dedup_key(
            "promotion",
            employee,
            1,
            &period_tag(PromotionTrack::Annual, date!(2026 - 12 - 31)),
        );
        let next_year = promotion_dedup_key(
            "promotion",
            employee,
            1,
            &period_tag(PromotionTrack::Annual, date!(2027 - 12 - 31)),
        );
        assert_ne!(this_year, next_year);
        let record = PromotionRecord {
            id: LeavePromotionId::new(),
            inbox_doc_id: uuid::Uuid::new_v4(),
            served_on: date!(2026 - 07 - 01),
            source_id: Some(this_year.clone()),
            payload: serde_json::json!({}),
        };
        assert!(ensure_same_period(&record, &this_year, "1차 촉구가").is_ok());
        assert!(ensure_same_period(&record, &next_year, "1차 촉구가").is_err());
    }
}
