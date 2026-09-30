//! Postgres work-order adapter.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::sync::Arc;

use console_kernel_core::{
    BranchId, BranchScope, CustomerId, DailyPlanId, EquipmentId, ErrorKind, KernelError, SiteId,
    UserId, VendorId, WorkOrderId,
};
use console_platform_db::{DbError, with_audit, with_org_conn};
use console_platform_request_context::current_org;
use console_workorder_application::{
    ActionInboxPosition, ActionInboxWorkOrderFuture, ActionInboxWorkOrderPort,
    AssignedActionInboxWorkOrder, AssignedActionInboxWorkOrderPage, CreateDailyPlanCommand,
    CreateOutsourceWorkCommand, CreateSettlementCommand, CreateWorkOrderCommand,
    DailyPlanItemSummary, DailyPlanListPage, DailyPlanListQuery, DailyPlanStatus, DailyPlanSummary,
    OutsourceWorkStatus, OutsourceWorkSummary, RejectWorkOrderCommand, ReviewDailyPlanCommand,
    ReviewSettlementCommand, ReviewTargetChangeCommand, SendDailyPlanForReviewCommand,
    SettlementLineSummary, SettlementReviewDecision, SettlementSummary, SubmitReportCommand,
    SubmitSettlementCommand, TargetChangeDecision, TargetChangeRequestCommand,
    TargetChangeRequestSummary, TargetChangeStatus, UpdatePriorityCommand,
    UpdateWorkOrderIntakeCommand, VoidSettlementCommand, WorkOrderApprovalCommand,
    WorkOrderAssignmentCommand, WorkOrderCreatedEvent, WorkOrderCreatedListener,
    WorkOrderStartCommand, WorkOrderSummary, daily_plan_audit_event, settlement_audit_event,
    work_order_audit_event,
};
use console_workorder_domain::{
    ApprovalRole, AssignmentRole, MaintenanceCause, MaintenanceType, PriorityLevel,
    SETTLEMENT_ELIGIBLE_WORK_ORDER_STATUSES, SettlementLineKind, SettlementStatus, TransitionActor,
    TransitionGuardContext, WorkOrderAssignment, WorkOrderAssignments, WorkOrderStatus,
    WorkResultType, validate_settlement_transition, validate_status_transition,
};
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};
use time::Date;

#[derive(Debug, thiserror::Error)]
pub enum PgWorkOrderError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl PgWorkOrderError {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Domain(err) => err.kind,
            Self::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => ErrorKind::NotFound,
            Self::Db(DbError::Sqlx(sqlx::Error::Database(err)))
                if err.code().is_some_and(|code| code == "23505") =>
            {
                ErrorKind::Conflict
            }
            Self::Db(_) => ErrorKind::Internal,
        }
    }
}

impl From<sqlx::Error> for PgWorkOrderError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

pub struct PgWorkOrderStore {
    pool: PgPool,
    created_listener: Option<Arc<dyn WorkOrderCreatedListener>>,
}

impl std::fmt::Debug for PgWorkOrderStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgWorkOrderStore")
            .field("pool", &self.pool)
            .field("has_created_listener", &self.created_listener.is_some())
            .finish()
    }
}

impl Clone for PgWorkOrderStore {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            created_listener: self.created_listener.clone(),
        }
    }
}

impl PgWorkOrderStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            created_listener: None,
        }
    }

    #[must_use]
    pub fn with_created_listener(mut self, listener: Arc<dyn WorkOrderCreatedListener>) -> Self {
        self.created_listener = Some(listener);
        self
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn create_work_order(
        &self,
        command: CreateWorkOrderCommand,
    ) -> Result<WorkOrderSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let work_order_id = WorkOrderId::new();
        let normalized_management_no = normalize_management_no(&command.management_no)?;
        let branch_id = command.branch_id;
        let branch_uuid = *branch_id.as_uuid();
        let actor = command.actor;
        let symptom = command.symptom;
        let customer_request = command.customer_request;
        let target_due_at = command.target_due_at;
        let maintenance_type = command.maintenance_type;
        let maintenance_cause = command.maintenance_cause;
        let occurred_at = command.occurred_at;
        let trace = command.trace.clone();
        let request_date = occurred_at.date();
        let event = work_order_audit_event(
            "work_order.create",
            actor,
            org,
            branch_id,
            work_order_id,
            command.trace,
            occurred_at,
        )?;

        let summary =
            with_audit::<_, WorkOrderSummary, PgWorkOrderError>(&self.pool, event, |tx| {
                Box::pin(async move {
                    if symptom.trim().is_empty() {
                        return Err(
                            KernelError::validation("work-order symptom is required").into()
                        );
                    }

                    let equipment = lookup_equipment_for_management_no(
                        tx,
                        branch_uuid,
                        &normalized_management_no,
                    )
                    .await?;
                    let request_no = next_request_no(tx, request_date, org_uuid).await?;
                    let id_uuid = *work_order_id.as_uuid();

                    sqlx::query(
                        r#"
                    INSERT INTO work_orders (
                        id, request_no, branch_id, equipment_id, customer_id, site_id,
                        requested_by, status, priority, symptom, customer_request,
                        target_due_at, maintenance_type, maintenance_cause,
                        created_at, updated_at, org_id
                    )
                    VALUES (
                        $1, $2, $3, $4, $5, $6,
                        $7, $8, $9, $10, $11,
                        $12, $13, $14,
                        $15, $15, $16
                    )
                    "#,
                    )
                    .bind(id_uuid)
                    .bind(request_no)
                    .bind(branch_uuid)
                    .bind(equipment.equipment_id)
                    .bind(equipment.customer_id)
                    .bind(equipment.site_id)
                    .bind(*actor.as_uuid())
                    .bind(WorkOrderStatus::Received.as_db_str())
                    .bind(PriorityLevel::Unset.as_db_str())
                    .bind(symptom.trim())
                    .bind(customer_request.as_deref().map(str::trim))
                    .bind(target_due_at)
                    .bind(maintenance_type.map(MaintenanceType::as_db_str))
                    .bind(maintenance_cause.map(MaintenanceCause::as_db_str))
                    .bind(occurred_at)
                    .bind(org_uuid)
                    .execute(tx.as_mut())
                    .await?;

                    insert_status_history(
                        tx,
                        work_order_id,
                        Some(actor),
                        "work_order.create",
                        None,
                        WorkOrderStatus::Received,
                        occurred_at,
                        org_uuid,
                    )
                    .await?;

                    fetch_work_order_summary_tx(tx, work_order_id).await
                })
            })
            .await?;

        if let Some(listener) = &self.created_listener {
            listener
                .work_order_created(WorkOrderCreatedEvent {
                    actor,
                    branch_id,
                    work_order_id: summary.id,
                    trace,
                    occurred_at,
                })
                .await?;
        }

        Ok(summary)
    }

    pub async fn update_priority(
        &self,
        command: UpdatePriorityCommand,
    ) -> Result<WorkOrderSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let branch_id = self.branch_for_work_order(command.work_order_id).await?;
        let work_order_id = command.work_order_id;
        let priority = command.priority;
        let actor = command.actor;
        let occurred_at = command.occurred_at;
        let event = work_order_audit_event(
            "work_order.priority",
            actor,
            org,
            branch_id,
            work_order_id,
            command.trace,
            occurred_at,
        )?;

        with_audit::<_, WorkOrderSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let row = lock_work_order(tx, work_order_id).await?;
                sqlx::query(
                    r#"
                    UPDATE work_orders
                    SET priority = $2, updated_at = $3
                    WHERE id = $1
                    "#,
                )
                .bind(row.id)
                .bind(priority.as_db_str())
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;

                fetch_work_order_summary_tx(tx, work_order_id).await
            })
        })
        .await
    }

    pub async fn update_work_order_intake(
        &self,
        command: UpdateWorkOrderIntakeCommand,
    ) -> Result<WorkOrderSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let branch_id = self.branch_for_work_order(command.work_order_id).await?;
        let work_order_id = command.work_order_id;
        let actor = command.actor;
        let occurred_at = command.occurred_at;
        let symptom = command.symptom.map(|value| value.trim().to_owned());
        if symptom.as_deref().is_some_and(str::is_empty) {
            return Err(KernelError::validation("work-order symptom is required").into());
        }
        let customer_request_was_set = command.customer_request.is_some();
        let customer_request = command
            .customer_request
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let maintenance_type = command.maintenance_type;
        let maintenance_cause = command.maintenance_cause;
        if symptom.is_none()
            && !customer_request_was_set
            && maintenance_type.is_none()
            && maintenance_cause.is_none()
        {
            return Err(
                KernelError::validation("no work-order intake fields were provided").into(),
            );
        }
        let event = work_order_audit_event(
            "work_order.update_intake",
            actor,
            org,
            branch_id,
            work_order_id,
            command.trace,
            occurred_at,
        )?;

        with_audit::<_, WorkOrderSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let row = lock_work_order(tx, work_order_id).await?;
                sqlx::query(
                    r#"
                    UPDATE work_orders
                    SET symptom = COALESCE($2, symptom),
                        customer_request = CASE WHEN $3 THEN $4 ELSE customer_request END,
                        maintenance_type = COALESCE($5, maintenance_type),
                        maintenance_cause = COALESCE($6, maintenance_cause),
                        updated_at = $7
                    WHERE id = $1
                    "#,
                )
                .bind(row.id)
                .bind(symptom.as_deref())
                .bind(customer_request_was_set)
                .bind(customer_request.as_deref())
                .bind(maintenance_type.map(MaintenanceType::as_db_str))
                .bind(maintenance_cause.map(MaintenanceCause::as_db_str))
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;

                fetch_work_order_summary_tx(tx, work_order_id).await
            })
        })
        .await
    }

    pub async fn assign_work_order(
        &self,
        command: WorkOrderAssignmentCommand,
    ) -> Result<WorkOrderSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let branch_id = self.branch_for_work_order(command.work_order_id).await?;
        let work_order_id = command.work_order_id;
        let actor = command.actor;
        let assignments = command.assignments;
        let admin_approver_id = command.admin_approver_id;
        let executive_approver_id = command.executive_approver_id;
        let occurred_at = command.occurred_at;
        let event = work_order_audit_event(
            "work_order.assign",
            actor,
            org,
            branch_id,
            work_order_id,
            command.trace,
            occurred_at,
        )?;

        with_audit::<_, WorkOrderSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let domain_assignments = assignments
                    .iter()
                    .map(|assignment| {
                        WorkOrderAssignment::new(assignment.mechanic_id, assignment.role)
                    })
                    .collect();
                let domain_assignments = WorkOrderAssignments::new(domain_assignments)?;
                let primary_mechanic = domain_assignments.primary().mechanic_id();
                let row = lock_work_order(tx, work_order_id).await?;
                validate_status_transition(
                    row.status,
                    WorkOrderStatus::Assigned,
                    TransitionGuardContext::admin(),
                )?;

                sqlx::query("DELETE FROM work_order_assignments WHERE work_order_id = $1")
                    .bind(row.id)
                    .execute(tx.as_mut())
                    .await?;
                sqlx::query("DELETE FROM work_order_approval_steps WHERE work_order_id = $1")
                    .bind(row.id)
                    .execute(tx.as_mut())
                    .await?;

                for assignment in assignments {
                    sqlx::query(
                        r#"
                        INSERT INTO work_order_assignments (
                            work_order_id, mechanic_id, role, assigned_at, org_id
                        )
                        VALUES ($1, $2, $3, $4, $5)
                        "#,
                    )
                    .bind(row.id)
                    .bind(*assignment.mechanic_id.as_uuid())
                    .bind(assignment.role.as_db_str())
                    .bind(occurred_at)
                    .bind(org_uuid)
                    .execute(tx.as_mut())
                    .await?;
                }

                insert_approval_step(
                    tx,
                    work_order_id,
                    1,
                    ApprovalRole::Mechanic,
                    Some(primary_mechanic),
                    "PENDING",
                    Some(occurred_at),
                    org_uuid,
                )
                .await?;
                insert_approval_step(
                    tx,
                    work_order_id,
                    2,
                    ApprovalRole::Admin,
                    admin_approver_id,
                    "NOT_STARTED",
                    None,
                    org_uuid,
                )
                .await?;
                insert_approval_step(
                    tx,
                    work_order_id,
                    3,
                    ApprovalRole::Executive,
                    executive_approver_id,
                    "NOT_STARTED",
                    None,
                    org_uuid,
                )
                .await?;

                update_status(
                    tx,
                    row,
                    actor,
                    "work_order.assign",
                    WorkOrderStatus::Assigned,
                    occurred_at,
                    org_uuid,
                )
                .await
            })
        })
        .await
    }

    pub async fn start_work(
        &self,
        command: WorkOrderStartCommand,
    ) -> Result<WorkOrderSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let branch_id = self.branch_for_work_order(command.work_order_id).await?;
        let work_order_id = command.work_order_id;
        let actor = command.actor;
        let occurred_at = command.occurred_at;
        let event = work_order_audit_event(
            "work_order.start",
            actor,
            org,
            branch_id,
            work_order_id,
            command.trace,
            occurred_at,
        )?;

        with_audit::<_, WorkOrderSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let row = lock_work_order(tx, work_order_id).await?;
                ensure_actor_assignment(tx, work_order_id, actor, AssignmentRequirement::Optional)
                    .await?;
                // Claim-and-start: an urgent unassigned order (RECEIVED/UNASSIGNED)
                // may be started directly by a mechanic. Record the actor as the
                // PRIMARY assignee so downstream steps (e.g. report submission, which
                // requires an existing assignment) succeed for the mechanic who took
                // the work. No-op when the order already has assignees.
                self_claim_if_unassigned(tx, work_order_id, actor, occurred_at, org_uuid).await?;
                validate_status_transition(
                    row.status,
                    WorkOrderStatus::InProgress,
                    TransitionGuardContext::mechanic(),
                )?;

                update_status(
                    tx,
                    row,
                    actor,
                    "work_order.start",
                    WorkOrderStatus::InProgress,
                    occurred_at,
                    org_uuid,
                )
                .await
            })
        })
        .await
    }

    pub async fn submit_report(
        &self,
        command: SubmitReportCommand,
    ) -> Result<WorkOrderSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let branch_id = self.branch_for_work_order(command.work_order_id).await?;
        let work_order_id = command.work_order_id;
        let actor = command.actor;
        let result_type = command.result_type;
        let diagnosis = command.diagnosis;
        let action_taken = command.action_taken;
        let occurred_at = command.occurred_at;
        let event = work_order_audit_event(
            "work_order.report",
            actor,
            org,
            branch_id,
            work_order_id,
            command.trace,
            occurred_at,
        )?;

        with_audit::<_, WorkOrderSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                if diagnosis.trim().is_empty() || action_taken.trim().is_empty() {
                    return Err(KernelError::validation(
                        "diagnosis and action taken are required for report submission",
                    )
                    .into());
                }
                let row = lock_work_order(tx, work_order_id).await?;
                ensure_actor_assignment(tx, work_order_id, actor, AssignmentRequirement::Required)
                    .await?;
                validate_status_transition(
                    row.status,
                    WorkOrderStatus::ReportSubmitted,
                    TransitionGuardContext::mechanic(),
                )?;

                sqlx::query(
                    r#"
                    UPDATE work_orders
                    SET result_type = $2,
                        diagnosis = $3,
                        action_taken = $4,
                        report_submitted_by = $5,
                        report_submitted_at = $6
                    WHERE id = $1
                    "#,
                )
                .bind(row.id)
                .bind(result_type.as_db_str())
                .bind(diagnosis.trim())
                .bind(action_taken.trim())
                .bind(*actor.as_uuid())
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;

                sqlx::query(
                    r#"
                    UPDATE work_order_approval_steps
                    SET status = 'APPROVED',
                        approved_at = $2,
                        approved_by_id = COALESCE(approver_id, $3),
                        updated_at = $2
                    WHERE work_order_id = $1
                      AND role = 'MECHANIC'
                      AND status = 'PENDING'
                    "#,
                )
                .bind(row.id)
                .bind(occurred_at)
                .bind(*actor.as_uuid())
                .execute(tx.as_mut())
                .await?;

                sqlx::query(
                    r#"
                    UPDATE work_order_approval_steps
                    SET status = 'PENDING',
                        requested_at = $2,
                        updated_at = $2
                    WHERE work_order_id = $1
                      AND role = 'ADMIN'
                      AND status = 'NOT_STARTED'
                    "#,
                )
                .bind(row.id)
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;

                update_status(
                    tx,
                    row,
                    actor,
                    "work_order.report",
                    WorkOrderStatus::ReportSubmitted,
                    occurred_at,
                    org_uuid,
                )
                .await
            })
        })
        .await
    }

    pub async fn approve_work_order(
        &self,
        command: WorkOrderApprovalCommand,
    ) -> Result<WorkOrderSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let branch_id = self.branch_for_work_order(command.work_order_id).await?;
        let work_order_id = command.work_order_id;
        let actor = command.actor;
        let occurred_at = command.occurred_at;
        let comment = command.comment.trim().to_owned();
        if comment.is_empty() {
            return Err(KernelError::validation("approval comment is required").into());
        }
        let event = work_order_audit_event(
            "work_order.approve",
            actor,
            org,
            branch_id,
            work_order_id,
            command.trace,
            occurred_at,
        )?
        .with_snapshots(None, Some(serde_json::json!({ "comment": comment })));

        with_audit::<_, WorkOrderSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let row = lock_work_order(tx, work_order_id).await?;
                let pending = pending_non_mechanic_approval_step(tx, work_order_id).await?;
                if pending.approver_id.is_none() {
                    return Err(KernelError::conflict(
                        "pending approval step has no assigned approver",
                    )
                    .into());
                }
                if let Some(approver_id) = pending.approver_id
                    && approver_id != actor
                {
                    return Err(KernelError::forbidden(
                        "actor is not assigned to the pending approval step",
                    )
                    .into());
                }

                let to = match pending.role {
                    ApprovalRole::Admin => WorkOrderStatus::AdminReview,
                    ApprovalRole::Executive if row.result_type == WorkResultType::Completed => {
                        WorkOrderStatus::FinalCompleted
                    }
                    ApprovalRole::Executive => WorkOrderStatus::TemporaryAction,
                    ApprovalRole::Mechanic => {
                        return Err(KernelError::conflict(
                            "mechanic approval is automatic when the report is submitted",
                        )
                        .into());
                    }
                };
                let context = TransitionGuardContext {
                    actor: TransitionActor::Admin,
                    approval_line_complete: pending.role == ApprovalRole::Executive,
                    completion_evidence_verified: to != WorkOrderStatus::FinalCompleted
                        || row.evidence_verified,
                };
                validate_status_transition(row.status, to, context)?;

                if pending.role == ApprovalRole::Admin {
                    let next_approver: Option<uuid::Uuid> = sqlx::query_scalar(
                        r#"
                        SELECT approver_id
                        FROM work_order_approval_steps
                        WHERE work_order_id = $1
                          AND role = 'EXECUTIVE'
                        "#,
                    )
                    .bind(row.id)
                    .fetch_optional(tx.as_mut())
                    .await?
                    .flatten();
                    if next_approver.is_none() {
                        return Err(KernelError::conflict(
                            "next approval step has no assigned approver",
                        )
                        .into());
                    }
                }

                sqlx::query(
                    r#"
                    UPDATE work_order_approval_steps
                    SET status = 'APPROVED',
                        approved_at = $2,
                        approved_by_id = $3,
                        decision_comment = $4,
                        updated_at = $2
                    WHERE id = $1
                    "#,
                )
                .bind(pending.id)
                .bind(occurred_at)
                .bind(*actor.as_uuid())
                .bind(&comment)
                .execute(tx.as_mut())
                .await?;

                if pending.role == ApprovalRole::Admin {
                    sqlx::query(
                        r#"
                        UPDATE work_order_approval_steps
                        SET status = 'PENDING',
                            requested_at = $2,
                            updated_at = $2
                        WHERE work_order_id = $1
                          AND role = 'EXECUTIVE'
                          AND status = 'NOT_STARTED'
                        "#,
                    )
                    .bind(row.id)
                    .bind(occurred_at)
                    .execute(tx.as_mut())
                    .await?;
                }

                let completed_work_order = (to == WorkOrderStatus::FinalCompleted)
                    .then_some((WorkOrderId::from_uuid(row.id), row.equipment_id));
                let summary = update_status(
                    tx,
                    row,
                    actor,
                    "work_order.approve",
                    to,
                    occurred_at,
                    org_uuid,
                )
                .await?;
                if let Some((completed_work_order_id, equipment_id)) = completed_work_order {
                    append_equipment_maintenance_history(
                        tx,
                        completed_work_order_id,
                        equipment_id,
                        occurred_at,
                        org_uuid,
                    )
                    .await?;
                }
                Ok(summary)
            })
        })
        .await
    }

    pub async fn reject_work_order(
        &self,
        command: RejectWorkOrderCommand,
    ) -> Result<WorkOrderSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let branch_id = self.branch_for_work_order(command.work_order_id).await?;
        let work_order_id = command.work_order_id;
        let actor = command.actor;
        let memo = command.memo;
        let occurred_at = command.occurred_at;
        let memo = memo.trim().to_owned();
        if memo.is_empty() {
            return Err(KernelError::validation("reject memo is required").into());
        }
        let event = work_order_audit_event(
            "work_order.reject",
            actor,
            org,
            branch_id,
            work_order_id,
            command.trace,
            occurred_at,
        )?
        .with_snapshots(None, Some(serde_json::json!({ "memo": memo })));

        with_audit::<_, WorkOrderSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let row = lock_work_order(tx, work_order_id).await?;
                validate_status_transition(
                    row.status,
                    WorkOrderStatus::Rejected,
                    TransitionGuardContext::admin(),
                )?;

                sqlx::query(
                    r#"
                    UPDATE work_order_approval_steps
                    SET status = 'REJECTED',
                        approved_at = $2,
                        approved_by_id = $3,
                        decision_comment = $4,
                        updated_at = $2
                    WHERE work_order_id = $1
                      AND status = 'PENDING'
                    "#,
                )
                .bind(row.id)
                .bind(occurred_at)
                .bind(*actor.as_uuid())
                .bind(&memo)
                .execute(tx.as_mut())
                .await?;

                update_status(
                    tx,
                    row,
                    actor,
                    "work_order.reject",
                    WorkOrderStatus::Rejected,
                    occurred_at,
                    org_uuid,
                )
                .await
            })
        })
        .await
    }

    pub async fn work_order(
        &self,
        work_order_id: WorkOrderId,
    ) -> Result<WorkOrderSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgWorkOrderError>(&self.pool, org, move |tx| {
            Box::pin(async move { fetch_work_order_summary_pool(tx, work_order_id).await })
        })
        .await
    }

    pub async fn target_change_request(
        &self,
        request_id: uuid::Uuid,
    ) -> Result<TargetChangeRequestSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let row = with_org_conn::<_, _, PgWorkOrderError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(
                    r#"
            SELECT t.id, t.work_order_id, w.branch_id, t.requested_target_due_at, t.status
            FROM target_change_requests t
            JOIN work_orders w ON w.id = t.work_order_id
            WHERE t.id = $1
            "#,
                )
                .bind(request_id)
                .fetch_optional(tx.as_mut())
                .await?)
            })
        })
        .await?
        .ok_or_else(|| KernelError::not_found("target change request was not found"))?;
        let status: String = row.try_get("status")?;
        Ok(TargetChangeRequestSummary {
            id: row.try_get("id")?,
            work_order_id: WorkOrderId::from_uuid(row.try_get("work_order_id")?),
            branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
            requested_target_due_at: row.try_get("requested_target_due_at")?,
            status: TargetChangeStatus::from_db_str(&status)?,
        })
    }

    pub async fn request_target_change(
        &self,
        command: TargetChangeRequestCommand,
    ) -> Result<TargetChangeRequestSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let branch_id = self.branch_for_work_order(command.work_order_id).await?;
        let work_order_id = command.work_order_id;
        let actor = command.actor;
        let requested_target_due_at = command.requested_target_due_at;
        let reason = command.reason;
        let occurred_at = command.occurred_at;
        let event = work_order_audit_event(
            "target_change.request",
            actor,
            org,
            branch_id,
            work_order_id,
            command.trace,
            occurred_at,
        )?;

        with_audit::<_, TargetChangeRequestSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                if reason.trim().is_empty() {
                    return Err(KernelError::validation("target change reason is required").into());
                }
                let row = lock_work_order(tx, work_order_id).await?;
                let request_id: uuid::Uuid = sqlx::query_scalar(
                    r#"
                        INSERT INTO target_change_requests (
                            work_order_id, requested_by, requested_target_due_at,
                            reason, status, created_at, org_id
                        )
                        VALUES ($1, $2, $3, $4, 'REQUESTED', $5, $6)
                        RETURNING id
                        "#,
                )
                .bind(row.id)
                .bind(*actor.as_uuid())
                .bind(requested_target_due_at)
                .bind(reason.trim())
                .bind(occurred_at)
                .bind(org_uuid)
                .fetch_one(tx.as_mut())
                .await?;

                Ok(TargetChangeRequestSummary {
                    id: request_id,
                    work_order_id,
                    branch_id,
                    requested_target_due_at,
                    status: TargetChangeStatus::Requested,
                })
            })
        })
        .await
    }

    pub async fn review_target_change(
        &self,
        command: ReviewTargetChangeCommand,
    ) -> Result<TargetChangeRequestSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let target = self.target_change_target(command.request_id).await?;
        let actor = command.actor;
        let request_id = command.request_id;
        let decision = command.decision;
        let memo = command.memo;
        let occurred_at = command.occurred_at;
        let event = work_order_audit_event(
            "target_change.review",
            actor,
            org,
            target.branch_id,
            target.work_order_id,
            command.trace,
            occurred_at,
        )?;

        with_audit::<_, TargetChangeRequestSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    r#"
                        SELECT t.work_order_id, t.requested_target_due_at, t.status, w.branch_id
                        FROM target_change_requests t
                        JOIN work_orders w ON w.id = t.work_order_id
                        WHERE t.id = $1
                        FOR UPDATE OF t
                        "#,
                )
                .bind(request_id)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("target change request was not found"))?;
                let status: String = row.try_get("status")?;
                if TargetChangeStatus::from_db_str(&status)? != TargetChangeStatus::Requested {
                    return Err(KernelError::conflict(
                        "target change request has already been reviewed",
                    )
                    .into());
                }

                let request_work_order_uuid: uuid::Uuid = row.try_get("work_order_id")?;
                let requested_target_due_at = row.try_get("requested_target_due_at")?;
                let branch_uuid: uuid::Uuid = row.try_get("branch_id")?;
                let status = TargetChangeStatus::from(decision);
                sqlx::query(
                    r#"
                        UPDATE target_change_requests
                        SET status = $2,
                            reviewed_by = $3,
                            reviewed_at = $4,
                            review_memo = $5
                        WHERE id = $1
                        "#,
                )
                .bind(request_id)
                .bind(status.as_db_str())
                .bind(*actor.as_uuid())
                .bind(occurred_at)
                .bind(memo.as_deref().map(str::trim))
                .execute(tx.as_mut())
                .await?;

                if decision == TargetChangeDecision::Approved {
                    sqlx::query(
                        r#"
                            UPDATE work_orders
                            SET target_due_at = $2, updated_at = $3
                            WHERE id = $1
                            "#,
                    )
                    .bind(request_work_order_uuid)
                    .bind(requested_target_due_at)
                    .bind(occurred_at)
                    .execute(tx.as_mut())
                    .await?;
                }

                Ok(TargetChangeRequestSummary {
                    id: request_id,
                    work_order_id: WorkOrderId::from_uuid(request_work_order_uuid),
                    branch_id: BranchId::from_uuid(branch_uuid),
                    requested_target_due_at,
                    status,
                })
            })
        })
        .await
    }

    pub async fn create_daily_plan(
        &self,
        command: CreateDailyPlanCommand,
    ) -> Result<DailyPlanSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let plan_id = DailyPlanId::new();
        let branch_id = command.branch_id;
        let actor = command.actor;
        let mechanic_id = command.mechanic_id;
        let plan_date = command.plan_date;
        let items = command.items;
        let occurred_at = command.occurred_at;
        let event = daily_plan_audit_event(
            "daily_plan.create",
            actor,
            org,
            branch_id,
            plan_id,
            command.trace,
            occurred_at,
        )?;

        with_audit::<_, DailyPlanSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                if items.is_empty() {
                    return Err(KernelError::validation(
                        "daily plan must include at least one item",
                    )
                    .into());
                }
                sqlx::query(
                    r#"
                    INSERT INTO daily_work_plans (
                        id, branch_id, mechanic_id, plan_date, status, created_at, updated_at, org_id
                    )
                    VALUES ($1, $2, $3, $4, 'DRAFT', $5, $5, $6)
                    "#,
                )
                .bind(*plan_id.as_uuid())
                .bind(*branch_id.as_uuid())
                .bind(*mechanic_id.as_uuid())
                .bind(plan_date)
                .bind(occurred_at)
                .bind(org_uuid)
                .execute(tx.as_mut())
                .await?;

                for (index, item) in items.into_iter().enumerate() {
                    let sort_order = i32::try_from(index + 1)
                        .map_err(|_| KernelError::validation("daily plan item index overflow"))?;
                    if item.description.trim().is_empty() {
                        return Err(KernelError::validation(
                            "daily plan item description is required",
                        )
                        .into());
                    }
                    let work_order_branch: Option<uuid::Uuid> = sqlx::query_scalar(
                        r#"
                        SELECT branch_id
                        FROM work_orders
                        WHERE id = $1 AND org_id = $2
                        "#,
                    )
                    .bind(*item.work_order_id.as_uuid())
                    .bind(org_uuid)
                    .fetch_optional(tx.as_mut())
                    .await?;
                    match work_order_branch {
                        Some(found_branch) if found_branch == *branch_id.as_uuid() => {}
                        Some(_) => {
                            return Err(KernelError::validation(
                                "daily plan work order must belong to the selected branch",
                            )
                            .into());
                        }
                        None => {
                            return Err(KernelError::not_found(
                                "daily plan source work order was not found",
                            )
                            .into());
                        }
                    }
                    let description = item.description.trim().to_owned();
                    sqlx::query(
                        r#"
                        INSERT INTO daily_work_plan_items (
                            plan_id, work_order_id, description, sort_order, org_id
                        )
                        VALUES ($1, $2, $3, $4, $5)
                        "#,
                    )
                    .bind(*plan_id.as_uuid())
                    .bind(*item.work_order_id.as_uuid())
                    .bind(&description)
                    .bind(sort_order)
                    .bind(org_uuid)
                    .execute(tx.as_mut())
                    .await?;
                }
                let stored_items = daily_plan_items(tx, plan_id).await?;

                Ok(DailyPlanSummary {
                    id: plan_id,
                    branch_id,
                    mechanic_id,
                    plan_date,
                    status: DailyPlanStatus::Draft,
                    items: stored_items,
                })
            })
        })
        .await
    }

    /// The approval-queue list of daily work plans (#19.17). Branch-scoped to the
    /// caller and armed via `with_org_conn(current_org())`, so it surfaces only
    /// the tenant's plans as `console_rt`. Carries NO status filter — DRAFT/REQUESTED
    /// plans MUST appear to the approver — and is ordered newest-plan-first.
    pub async fn list_daily_plans(
        &self,
        query: DailyPlanListQuery,
    ) -> Result<DailyPlanListPage, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let items = with_org_conn::<_, _, PgWorkOrderError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let mut builder = QueryBuilder::<Postgres>::new(
                    r#"
                    SELECT id, branch_id, mechanic_id, plan_date, status
                    FROM daily_work_plans
                    WHERE
                    "#,
                );
                match &query.branch_scope {
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
                if let Some(plan_date) = query.plan_date {
                    builder.push(" AND plan_date = ");
                    builder.push_bind(plan_date);
                }
                builder.push(" ORDER BY plan_date DESC, created_at DESC, id DESC");
                // Bound the unfiltered queue. The ORDER BY puts the newest
                // plan_date first, so a just-created today plan (#19.17) always
                // sorts to the top and stays within the cap — the bound trims
                // only the long tail of old plans, never today's DRAFT/REQUESTED.
                // ponytail: fixed cap; add keyset pagination if a tenant's recent
                // queue ever exceeds 200 same-day plans.
                if query.plan_date.is_none() {
                    builder.push(" LIMIT 200");
                }
                let rows = builder.build().fetch_all(tx.as_mut()).await?;
                let mut plans = Vec::with_capacity(rows.len());
                for row in rows {
                    let mut plan = daily_plan_summary_from_row(&row)?;
                    plan.items = daily_plan_items(tx, plan.id).await?;
                    plans.push(plan);
                }
                Ok(plans)
            })
        })
        .await?;
        Ok(DailyPlanListPage { items })
    }

    pub async fn request_daily_plan_review(
        &self,
        command: SendDailyPlanForReviewCommand,
    ) -> Result<DailyPlanSummary, PgWorkOrderError> {
        self.transition_daily_plan(DailyPlanTransition {
            actor: command.actor,
            plan_id: command.plan_id,
            action: "daily_plan.request",
            trace: command.trace,
            occurred_at: command.occurred_at,
            expected: DailyPlanStatus::Draft,
            next: DailyPlanStatus::Requested,
            memo: None,
        })
        .await
    }

    pub async fn review_daily_plan(
        &self,
        command: ReviewDailyPlanCommand,
    ) -> Result<DailyPlanSummary, PgWorkOrderError> {
        if !matches!(
            command.decision,
            DailyPlanStatus::Approved | DailyPlanStatus::Rejected
        ) {
            return Err(KernelError::validation(
                "daily plan review decision must be APPROVED or REJECTED",
            )
            .into());
        }
        self.transition_daily_plan(DailyPlanTransition {
            actor: command.actor,
            plan_id: command.plan_id,
            action: "daily_plan.review",
            trace: command.trace,
            occurred_at: command.occurred_at,
            expected: DailyPlanStatus::Requested,
            next: command.decision,
            memo: command.memo,
        })
        .await
    }

    pub async fn confirm_daily_plan(
        &self,
        command: SendDailyPlanForReviewCommand,
    ) -> Result<DailyPlanSummary, PgWorkOrderError> {
        self.transition_daily_plan(DailyPlanTransition {
            actor: command.actor,
            plan_id: command.plan_id,
            action: "daily_plan.confirm",
            trace: command.trace,
            occurred_at: command.occurred_at,
            expected: DailyPlanStatus::Approved,
            next: DailyPlanStatus::FinalConfirmed,
            memo: None,
        })
        .await
    }

    pub async fn create_outsource_work(
        &self,
        command: CreateOutsourceWorkCommand,
    ) -> Result<OutsourceWorkSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let branch_id = self.branch_for_work_order(command.work_order_id).await?;
        let actor = command.actor;
        let work_order_id = command.work_order_id;
        let vendor_name = command.vendor_name;
        let vendor_contact = command.vendor_contact;
        let reason = command.reason;
        let occurred_at = command.occurred_at;
        let event = work_order_audit_event(
            "work_order.outsource",
            actor,
            org,
            branch_id,
            work_order_id,
            command.trace,
            occurred_at,
        )?;

        with_audit::<_, OutsourceWorkSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                if vendor_name.trim().is_empty() || reason.trim().is_empty() {
                    return Err(KernelError::validation(
                        "vendor name and outsource reason are required",
                    )
                    .into());
                }
                let row = lock_work_order(tx, work_order_id).await?;
                let vendor_row = sqlx::query(
                    r#"
                    INSERT INTO outsource_vendors (branch_id, name, contact, org_id)
                    VALUES ($1, $2, $3, $4)
                    ON CONFLICT (branch_id, name) DO UPDATE
                    SET contact = COALESCE(EXCLUDED.contact, outsource_vendors.contact),
                        updated_at = now()
                    RETURNING id, name
                    "#,
                )
                .bind(*branch_id.as_uuid())
                .bind(vendor_name.trim())
                .bind(vendor_contact.as_deref().map(str::trim))
                .bind(org_uuid)
                .fetch_one(tx.as_mut())
                .await?;
                let vendor_uuid: uuid::Uuid = vendor_row.try_get("id")?;
                let stored_vendor_name: String = vendor_row.try_get("name")?;
                let outsource_id: uuid::Uuid = sqlx::query_scalar(
                    r#"
                    INSERT INTO outsource_works (
                        work_order_id, vendor_id, status, reason, requested_at, org_id
                    )
                    VALUES ($1, $2, 'REQUESTED', $3, $4, $5)
                    RETURNING id
                    "#,
                )
                .bind(row.id)
                .bind(vendor_uuid)
                .bind(reason.trim())
                .bind(occurred_at)
                .bind(org_uuid)
                .fetch_one(tx.as_mut())
                .await?;

                sqlx::query(
                    r#"
                    UPDATE work_orders
                    SET priority = $2, updated_at = $3
                    WHERE id = $1
                    "#,
                )
                .bind(row.id)
                .bind(PriorityLevel::Outsource.as_db_str())
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;

                Ok(OutsourceWorkSummary {
                    id: outsource_id,
                    work_order_id,
                    vendor_id: VendorId::from_uuid(vendor_uuid),
                    vendor_name: stored_vendor_name,
                    status: OutsourceWorkStatus::Requested,
                })
            })
        })
        .await
    }

    pub async fn daily_plan(
        &self,
        plan_id: DailyPlanId,
    ) -> Result<DailyPlanSummary, PgWorkOrderError> {
        self.daily_plan_target(plan_id).await
    }

    /// Open a DRAFT cost settlement on a settlement-eligible work order.
    ///
    /// Replaying the same `Idempotency-Key` with an identical request returns
    /// the existing settlement; a different request under the same key
    /// conflicts. At most one live (non-VOID) settlement may exist per order
    /// (partial unique index).
    pub async fn create_settlement(
        &self,
        command: CreateSettlementCommand,
    ) -> Result<SettlementSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        if command.lines.is_empty() {
            return Err(
                KernelError::validation("settlement requires at least one cost line").into(),
            );
        }
        let mut total_amount_krw: i64 = 0;
        for line in &command.lines {
            if line.label.trim().is_empty() {
                return Err(KernelError::validation("settlement line label is required").into());
            }
            if line.amount_krw < 0 {
                return Err(
                    KernelError::validation("settlement line amount must not be negative").into(),
                );
            }
            total_amount_krw = total_amount_krw
                .checked_add(line.amount_krw)
                .ok_or_else(|| KernelError::validation("settlement total overflows"))?;
        }

        // Idempotent replay: read outside the audited transaction (a replay
        // must not append a second audit event). A create racing this check
        // falls through to the unique (org_id, idempotency_key) index → 409.
        let idempotency_key = command.idempotency_key.clone();
        let replay = with_org_conn::<_, _, PgWorkOrderError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(
                    r#"
                    SELECT id, work_order_id, request_hash
                    FROM work_order_settlements
                    WHERE idempotency_key = $1
                    "#,
                )
                .bind(idempotency_key)
                .fetch_optional(tx.as_mut())
                .await?)
            })
        })
        .await?;
        if let Some(row) = replay {
            let existing_hash: String = row.try_get("request_hash")?;
            let existing_work_order: uuid::Uuid = row.try_get("work_order_id")?;
            if existing_hash == command.request_hash
                && existing_work_order == *command.work_order_id.as_uuid()
            {
                let existing_id: uuid::Uuid = row.try_get("id")?;
                return self.settlement(existing_id).await;
            }
            return Err(
                KernelError::conflict("idempotency key was used with a different request").into(),
            );
        }

        let branch_id = self.branch_for_work_order(command.work_order_id).await?;
        let settlement_id = uuid::Uuid::new_v4();
        let work_order_id = command.work_order_id;
        let actor = command.actor;
        let lines = command.lines;
        let note = command.note;
        let idempotency_key = command.idempotency_key;
        let request_hash = command.request_hash;
        let require_actor_assignment = command.require_actor_assignment;
        let occurred_at = command.occurred_at;
        let event = settlement_audit_event(
            "work_order_settlement.create",
            actor,
            org,
            branch_id,
            settlement_id,
            command.trace,
            occurred_at,
        )?;

        with_audit::<_, SettlementSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let order = lock_work_order(tx, work_order_id).await?;
                if !SETTLEMENT_ELIGIBLE_WORK_ORDER_STATUSES.contains(&order.status) {
                    return Err(KernelError::conflict(
                        "settlement requires a submitted report, admin review, or final completion",
                    )
                    .into());
                }
                if require_actor_assignment {
                    ensure_actor_assignment(
                        tx,
                        work_order_id,
                        actor,
                        AssignmentRequirement::Required,
                    )
                    .await?;
                }

                sqlx::query(
                    r#"
                    INSERT INTO work_order_settlements (
                        id, org_id, work_order_id, branch_id, status, total_amount_krw,
                        note, idempotency_key, request_hash, created_by, created_at, updated_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $11)
                    "#,
                )
                .bind(settlement_id)
                .bind(org_uuid)
                .bind(*work_order_id.as_uuid())
                .bind(*branch_id.as_uuid())
                .bind(SettlementStatus::Draft.as_db_str())
                .bind(total_amount_krw)
                .bind(note.as_deref().map(str::trim))
                .bind(idempotency_key)
                .bind(request_hash)
                .bind(*actor.as_uuid())
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;

                for (index, line) in lines.iter().enumerate() {
                    sqlx::query(
                        r#"
                        INSERT INTO work_order_settlement_lines (
                            org_id, settlement_id, kind, label, amount_krw, source_ref, sort_order
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7)
                        "#,
                    )
                    .bind(org_uuid)
                    .bind(settlement_id)
                    .bind(line.kind.as_db_str())
                    .bind(line.label.trim())
                    .bind(line.amount_krw)
                    .bind(line.source_ref.as_deref().map(str::trim))
                    .bind(
                        line.sort_order
                            .unwrap_or(i32::try_from(index).unwrap_or(i32::MAX)),
                    )
                    .execute(tx.as_mut())
                    .await?;
                }

                fetch_settlement_summary_tx(tx, settlement_id).await
            })
        })
        .await
    }

    pub async fn settlement(
        &self,
        settlement_id: uuid::Uuid,
    ) -> Result<SettlementSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgWorkOrderError>(&self.pool, org, move |tx| {
            Box::pin(async move { fetch_settlement_summary_tx(tx, settlement_id).await })
        })
        .await
    }

    /// The live (non-VOID) settlement of a work order, if any.
    pub async fn live_settlement_for_work_order(
        &self,
        work_order_id: WorkOrderId,
    ) -> Result<Option<SettlementSummary>, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgWorkOrderError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let id: Option<uuid::Uuid> = sqlx::query_scalar(
                    r#"
                    SELECT id
                    FROM work_order_settlements
                    WHERE work_order_id = $1 AND status <> 'VOID'
                    "#,
                )
                .bind(*work_order_id.as_uuid())
                .fetch_optional(tx.as_mut())
                .await?;
                match id {
                    Some(id) => Ok(Some(fetch_settlement_summary_tx(tx, id).await?)),
                    None => Ok(None),
                }
            })
        })
        .await
    }

    pub async fn submit_settlement(
        &self,
        command: SubmitSettlementCommand,
    ) -> Result<SettlementSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let current = self.settlement(command.settlement_id).await?;
        let settlement_id = command.settlement_id;
        let actor = command.actor;
        let occurred_at = command.occurred_at;
        let event = settlement_audit_event(
            "work_order_settlement.submit",
            actor,
            org,
            current.branch_id,
            settlement_id,
            command.trace,
            occurred_at,
        )?;

        with_audit::<_, SettlementSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let row = lock_settlement(tx, settlement_id).await?;
                validate_settlement_transition(row.status, SettlementStatus::Submitted)?;
                sqlx::query(
                    r#"
                    UPDATE work_order_settlements
                    SET status = $2, submitted_by = $3, submitted_at = $4, updated_at = $4
                    WHERE id = $1
                    "#,
                )
                .bind(settlement_id)
                .bind(SettlementStatus::Submitted.as_db_str())
                .bind(*actor.as_uuid())
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;
                fetch_settlement_summary_tx(tx, settlement_id).await
            })
        })
        .await
    }

    /// Four-eyes review of a SUBMITTED settlement: APPROVED closes it, RETURNED
    /// sends it back to DRAFT (comment required — recorded on the audit event).
    pub async fn review_settlement(
        &self,
        command: ReviewSettlementCommand,
    ) -> Result<SettlementSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let comment = command
            .comment
            .as_deref()
            .map(str::trim)
            .filter(|comment| !comment.is_empty())
            .map(str::to_owned);
        if command.decision == SettlementReviewDecision::Returned && comment.is_none() {
            return Err(
                KernelError::validation("a comment is required to return a settlement").into(),
            );
        }
        let current = self.settlement(command.settlement_id).await?;
        let settlement_id = command.settlement_id;
        let actor = command.actor;
        let decision = command.decision;
        let occurred_at = command.occurred_at;
        let event = settlement_audit_event(
            "work_order_settlement.review",
            actor,
            org,
            current.branch_id,
            settlement_id,
            command.trace,
            occurred_at,
        )?
        .with_snapshots(
            Some(serde_json::json!({ "status": current.status.as_db_str() })),
            Some(serde_json::json!({
                "decision": decision.as_db_str(),
                "comment": comment,
            })),
        );

        with_audit::<_, SettlementSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let row = lock_settlement(tx, settlement_id).await?;
                if row.submitted_by == Some(*actor.as_uuid()) {
                    return Err(KernelError::forbidden(
                        "settlement review requires a reviewer other than the submitter",
                    )
                    .into());
                }
                match decision {
                    SettlementReviewDecision::Approved => {
                        validate_settlement_transition(row.status, SettlementStatus::Approved)?;
                        sqlx::query(
                            r#"
                            UPDATE work_order_settlements
                            SET status = $2, approved_by = $3, approved_at = $4, updated_at = $4
                            WHERE id = $1
                            "#,
                        )
                        .bind(settlement_id)
                        .bind(SettlementStatus::Approved.as_db_str())
                        .bind(*actor.as_uuid())
                        .bind(occurred_at)
                        .execute(tx.as_mut())
                        .await?;
                    }
                    SettlementReviewDecision::Returned => {
                        validate_settlement_transition(row.status, SettlementStatus::Draft)?;
                        sqlx::query(
                            r#"
                            UPDATE work_order_settlements
                            SET status = $2, submitted_by = NULL, submitted_at = NULL,
                                updated_at = $3
                            WHERE id = $1
                            "#,
                        )
                        .bind(settlement_id)
                        .bind(SettlementStatus::Draft.as_db_str())
                        .bind(occurred_at)
                        .execute(tx.as_mut())
                        .await?;
                    }
                }
                fetch_settlement_summary_tx(tx, settlement_id).await
            })
        })
        .await
    }

    /// Void a DRAFT or SUBMITTED settlement (reason required — recorded on the
    /// audit event). VOID frees the one-live-settlement slot; nothing is
    /// hard-deleted.
    pub async fn void_settlement(
        &self,
        command: VoidSettlementCommand,
    ) -> Result<SettlementSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let reason = command.reason.trim().to_owned();
        if reason.is_empty() {
            return Err(
                KernelError::validation("a reason is required to void a settlement").into(),
            );
        }
        let current = self.settlement(command.settlement_id).await?;
        let settlement_id = command.settlement_id;
        let actor = command.actor;
        let occurred_at = command.occurred_at;
        let event = settlement_audit_event(
            "work_order_settlement.void",
            actor,
            org,
            current.branch_id,
            settlement_id,
            command.trace,
            occurred_at,
        )?
        .with_snapshots(
            Some(serde_json::json!({ "status": current.status.as_db_str() })),
            Some(serde_json::json!({ "reason": reason })),
        );

        with_audit::<_, SettlementSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let row = lock_settlement(tx, settlement_id).await?;
                validate_settlement_transition(row.status, SettlementStatus::Void)?;
                sqlx::query(
                    r#"
                    UPDATE work_order_settlements
                    SET status = $2, updated_at = $3
                    WHERE id = $1
                    "#,
                )
                .bind(settlement_id)
                .bind(SettlementStatus::Void.as_db_str())
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;
                fetch_settlement_summary_tx(tx, settlement_id).await
            })
        })
        .await
    }

    async fn branch_for_work_order(
        &self,
        work_order_id: WorkOrderId,
    ) -> Result<BranchId, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let branch_uuid: uuid::Uuid =
            with_org_conn::<_, _, PgWorkOrderError>(&self.pool, org, move |tx| {
                Box::pin(async move {
                    Ok(
                        sqlx::query_scalar("SELECT branch_id FROM work_orders WHERE id = $1")
                            .bind(*work_order_id.as_uuid())
                            .fetch_optional(tx.as_mut())
                            .await?,
                    )
                })
            })
            .await?
            .ok_or_else(|| KernelError::not_found("work order was not found"))?;
        Ok(BranchId::from_uuid(branch_uuid))
    }

    async fn target_change_target(
        &self,
        request_id: uuid::Uuid,
    ) -> Result<TargetChangeTarget, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let row = with_org_conn::<_, _, PgWorkOrderError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(
                    r#"
            SELECT t.work_order_id, w.branch_id
            FROM target_change_requests t
            JOIN work_orders w ON w.id = t.work_order_id
            WHERE t.id = $1
            "#,
                )
                .bind(request_id)
                .fetch_optional(tx.as_mut())
                .await?)
            })
        })
        .await?
        .ok_or_else(|| KernelError::not_found("target change request was not found"))?;
        Ok(TargetChangeTarget {
            work_order_id: WorkOrderId::from_uuid(row.try_get("work_order_id")?),
            branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        })
    }

    async fn transition_daily_plan(
        &self,
        transition: DailyPlanTransition,
    ) -> Result<DailyPlanSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let DailyPlanTransition {
            actor,
            plan_id,
            action,
            trace,
            occurred_at,
            expected,
            next,
            memo,
        } = transition;
        let plan = self.daily_plan_target(plan_id).await?;
        let event = daily_plan_audit_event(
            action,
            actor,
            org,
            plan.branch_id,
            plan_id,
            trace,
            occurred_at,
        )?;

        with_audit::<_, DailyPlanSummary, PgWorkOrderError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let current = lock_daily_plan(tx, plan_id).await?;
                if current.status != expected {
                    return Err(KernelError::conflict(format!(
                        "daily plan must be {} before {}",
                        expected.as_db_str(),
                        action
                    ))
                    .into());
                }

                match action {
                    "daily_plan.request" => {
                        sqlx::query(
                            r#"
                            UPDATE daily_work_plans
                            SET status = $2, requested_at = $3, updated_at = $3
                            WHERE id = $1
                            "#,
                        )
                        .bind(*plan_id.as_uuid())
                        .bind(next.as_db_str())
                        .bind(occurred_at)
                        .execute(tx.as_mut())
                        .await?;
                    }
                    "daily_plan.review" => {
                        sqlx::query(
                            r#"
                            UPDATE daily_work_plans
                            SET status = $2,
                                reviewed_by = $3,
                                reviewed_at = $4,
                                review_memo = $5,
                                updated_at = $4
                            WHERE id = $1
                            "#,
                        )
                        .bind(*plan_id.as_uuid())
                        .bind(next.as_db_str())
                        .bind(*actor.as_uuid())
                        .bind(occurred_at)
                        .bind(memo.as_deref().map(str::trim))
                        .execute(tx.as_mut())
                        .await?;
                    }
                    "daily_plan.confirm" => {
                        sqlx::query(
                            r#"
                            UPDATE daily_work_plans
                            SET status = $2, confirmed_at = $3, updated_at = $3
                            WHERE id = $1
                            "#,
                        )
                        .bind(*plan_id.as_uuid())
                        .bind(next.as_db_str())
                        .bind(occurred_at)
                        .execute(tx.as_mut())
                        .await?;
                    }
                    _ => {
                        return Err(KernelError::validation("unknown daily plan action").into());
                    }
                }

                Ok(DailyPlanSummary {
                    status: next,
                    ..current
                })
            })
        })
        .await
    }

    async fn daily_plan_target(
        &self,
        plan_id: DailyPlanId,
    ) -> Result<DailyPlanSummary, PgWorkOrderError> {
        let org = current_org().map_err(KernelError::from)?;
        let plan = with_org_conn::<_, _, PgWorkOrderError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    r#"
            SELECT id, branch_id, mechanic_id, plan_date, status
            FROM daily_work_plans
            WHERE id = $1
            "#,
                )
                .bind(*plan_id.as_uuid())
                .fetch_optional(tx.as_mut())
                .await?;
                match row {
                    Some(row) => {
                        let mut plan = daily_plan_summary_from_row(&row)?;
                        plan.items = daily_plan_items(tx, plan_id).await?;
                        Ok(Some(plan))
                    }
                    None => Ok(None),
                }
            })
        })
        .await?
        .ok_or_else(|| KernelError::not_found("daily plan was not found"))?;
        Ok(plan)
    }
}

fn push_action_inbox_branch_scope(
    query: &mut QueryBuilder<Postgres>,
    branch_ids: Option<&[uuid::Uuid]>,
) {
    match branch_ids {
        None => {}
        Some([]) => {
            query.push(" AND FALSE");
        }
        Some(ids) => {
            query.push(" AND w.branch_id = ANY(");
            query.push_bind(ids.to_vec());
            query.push(")");
        }
    }
}

impl ActionInboxWorkOrderPort for PgWorkOrderStore {
    fn list_assigned_action_inbox_page(
        &self,
        org: console_kernel_core::OrgId,
        branch_scope: BranchScope,
        mechanic: UserId,
        as_of: time::OffsetDateTime,
        after: Option<ActionInboxPosition>,
        limit: i64,
    ) -> ActionInboxWorkOrderFuture<'_> {
        let pool = self.pool.clone();
        Box::pin(async move {
            let limit = limit.clamp(1, 200);
            let mechanic_id = *mechanic.as_uuid();
            let (after_created_at, after_id) = after.map_or((None, None), |position| {
                (Some(position.created_at), Some(position.id))
            });
            with_org_conn::<_, _, PgWorkOrderError>(&pool, org, move |tx| {
                Box::pin(async move {
                    let branch_ids = match branch_scope {
                        BranchScope::All => None,
                        BranchScope::Branches(branches) => Some(
                            branches
                                .into_iter()
                                .map(|branch| *branch.as_uuid())
                                .collect::<Vec<_>>(),
                        ),
                    };
                    let mut count = QueryBuilder::<Postgres>::new(
                        "SELECT COUNT(*) FROM work_orders w \
                         WHERE w.status NOT IN ('FINAL_COMPLETED', 'REJECTED', 'CANCELLED') \
                           AND w.created_at <= ",
                    );
                    count.push_bind(as_of);
                    count.push(
                        " AND EXISTS (SELECT 1 FROM work_order_assignments a \
                         WHERE a.work_order_id = w.id AND a.mechanic_id = ",
                    );
                    count.push_bind(mechanic_id);
                    count.push(")");
                    push_action_inbox_branch_scope(&mut count, branch_ids.as_deref());
                    let total: i64 = count.build_query_scalar().fetch_one(tx.as_mut()).await?;

                    let mut query = QueryBuilder::<Postgres>::new(
                        "SELECT w.id, w.request_no, w.target_due_at, w.created_at, \
                                s.name AS site_name \
                         FROM work_orders w \
                         JOIN registry_sites s ON s.id = w.site_id \
                         WHERE w.status NOT IN ('FINAL_COMPLETED', 'REJECTED', 'CANCELLED') \
                           AND w.created_at <= ",
                    );
                    query.push_bind(as_of);
                    query.push(
                        " AND EXISTS (SELECT 1 FROM work_order_assignments a \
                         WHERE a.work_order_id = w.id AND a.mechanic_id = ",
                    );
                    query.push_bind(mechanic_id);
                    query.push(")");
                    push_action_inbox_branch_scope(&mut query, branch_ids.as_deref());
                    query.push(" AND (");
                    query.push_bind(after_id.as_deref());
                    query.push("::text IS NULL OR w.created_at > ");
                    query.push_bind(after_created_at);
                    query.push(" OR (w.created_at = ");
                    query.push_bind(after_created_at);
                    query.push(" AND ('work:' || w.id::text) > ");
                    query.push_bind(after_id.as_deref());
                    query.push(")) ORDER BY w.created_at ASC, ('work:' || w.id::text) ASC LIMIT ");
                    query.push_bind(limit + 1);
                    let rows = query.build().fetch_all(tx.as_mut()).await?;
                    let has_more = i64::try_from(rows.len()).unwrap_or(0) > limit;
                    let items = rows
                        .iter()
                        .take(usize::try_from(limit).unwrap_or(rows.len()))
                        .map(|row| {
                            Ok(AssignedActionInboxWorkOrder {
                                id: WorkOrderId::from_uuid(row.try_get("id")?),
                                request_no: row.try_get("request_no")?,
                                target_due_at: row.try_get("target_due_at")?,
                                created_at: row.try_get("created_at")?,
                                site_name: row.try_get("site_name")?,
                            })
                        })
                        .collect::<Result<Vec<_>, PgWorkOrderError>>()?;
                    Ok(AssignedActionInboxWorkOrderPage {
                        items,
                        total,
                        has_more,
                    })
                })
            })
            .await
            .map_err(|error| {
                KernelError::internal(format!("work-order inbox read failed: {error}"))
            })
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct EquipmentLookup {
    equipment_id: uuid::Uuid,
    customer_id: uuid::Uuid,
    site_id: uuid::Uuid,
}

#[derive(Debug, Clone)]
struct WorkOrderRow {
    id: uuid::Uuid,
    request_no: String,
    branch_id: BranchId,
    equipment_id: EquipmentId,
    customer_id: CustomerId,
    site_id: SiteId,
    status: WorkOrderStatus,
    priority: PriorityLevel,
    result_type: WorkResultType,
    maintenance_type: Option<MaintenanceType>,
    maintenance_cause: Option<MaintenanceCause>,
    evidence_verified: bool,
}

#[derive(Debug, Clone, Copy)]
struct PendingApprovalStep {
    id: uuid::Uuid,
    role: ApprovalRole,
    approver_id: Option<UserId>,
}

#[derive(Debug, Clone, Copy)]
struct TargetChangeTarget {
    work_order_id: WorkOrderId,
    branch_id: BranchId,
}

#[derive(Debug, Clone)]
struct DailyPlanTransition {
    actor: UserId,
    plan_id: DailyPlanId,
    action: &'static str,
    trace: console_kernel_core::TraceContext,
    occurred_at: console_kernel_core::Timestamp,
    expected: DailyPlanStatus,
    next: DailyPlanStatus,
    memo: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssignmentRequirement {
    Optional,
    Required,
}

/// Normalize a typed management number for equipment resolution.
///
/// Receptionists file work orders with values like `3호기`, `#3`, `#3호기` or a
/// bare `3`; equipment is stored with the bare number. This MUST strip both the
/// leading `#` AND the trailing `호기` so the create write-lookup matches the
/// same forms the REST equipment-lookup/autocomplete handlers accept — otherwise
/// a perfectly valid `3호기` fails the create with a confusing error.
fn normalize_management_no(value: &str) -> Result<String, KernelError> {
    let normalized = value
        .trim()
        .trim_start_matches('#')
        .trim()
        .trim_end_matches("호기")
        .trim();
    if normalized.is_empty() {
        return Err(KernelError::validation("management number is required"));
    }
    Ok(normalized.to_owned())
}

async fn lookup_equipment_for_management_no(
    tx: &mut Transaction<'_, Postgres>,
    branch_uuid: uuid::Uuid,
    management_no: &str,
) -> Result<EquipmentLookup, PgWorkOrderError> {
    // 1. EXACT match first: a stored `10` and a stored `0010` are DIFFERENT
    //    equipment, so an exact hit on the typed (normalized) value is always
    //    the unambiguous answer and must win over the leading-zero-insensitive
    //    fallback below.
    let exact = sqlx::query(
        r#"
        SELECT id, customer_id, site_id
        FROM registry_equipment
        WHERE branch_id = $1 AND management_no = $2
        LIMIT 2
        "#,
    )
    .bind(branch_uuid)
    .bind(management_no)
    .fetch_all(tx.as_mut())
    .await?;
    match exact.as_slice() {
        [row] => {
            return Ok(EquipmentLookup {
                equipment_id: row.try_get("id")?,
                customer_id: row.try_get("customer_id")?,
                site_id: row.try_get("site_id")?,
            });
        }
        [] => {} // fall through to normalized fallback
        _ => {
            return Err(KernelError::conflict(
                "여러 장비의 관리번호가 같습니다. 관리번호가 중복되지 않도록 정비하세요 \
                 (multiple equipment share the same exact management number — deduplicate them)",
            )
            .into());
        }
    }

    // 2. Leading-zero-insensitive fallback so a stored `010` resolves the
    //    normalized `10` / `10호기` / `#10` the receptionist typed, mirroring the
    //    REST lookup_equipment handler. But this must NOT guess: if several
    //    rows share a normalized management_no (e.g. `10` vs `0010`), binding the
    //    work order to whichever was updated last risks the wrong
    //    equipment/customer/site — so we require EXACTLY ONE normalized match
    //    and otherwise raise a conflict for the operator to disambiguate.
    let rows = sqlx::query(
        r#"
        SELECT id, customer_id, site_id
        FROM registry_equipment
        WHERE branch_id = $1 AND ltrim(management_no, '0') = ltrim($2, '0')
        LIMIT 2
        "#,
    )
    .bind(branch_uuid)
    .bind(management_no)
    .fetch_all(tx.as_mut())
    .await?;
    match rows.as_slice() {
        [] => Err(KernelError::not_found("no equipment matches that 호기 number").into()),
        [row] => Ok(EquipmentLookup {
            equipment_id: row.try_get("id")?,
            customer_id: row.try_get("customer_id")?,
            site_id: row.try_get("site_id")?,
        }),
        _ => Err(KernelError::conflict(
            "여러 장비의 관리번호가 같은 번호로 정규화됩니다. 앞자리 0을 포함한 정확한 관리번호를 입력하세요 \
             (multiple equipment share this normalized management number — enter the exact management_no including any leading zeros)",
        )
        .into()),
    }
}

async fn next_request_no(
    tx: &mut Transaction<'_, Postgres>,
    date: Date,
    org_uuid: uuid::Uuid,
) -> Result<String, PgWorkOrderError> {
    let sequence: i32 = sqlx::query_scalar(
        r#"
        INSERT INTO work_order_request_counters (org_id, request_date, last_sequence)
        VALUES ($2, $1, 1)
        ON CONFLICT (org_id, request_date) DO UPDATE
        SET last_sequence = work_order_request_counters.last_sequence + 1
        RETURNING last_sequence
        "#,
    )
    .bind(date)
    .bind(org_uuid)
    .fetch_one(tx.as_mut())
    .await?;
    if sequence > 999 {
        return Err(
            KernelError::conflict("daily work-order request number sequence exceeded 999").into(),
        );
    }

    Ok(format!(
        "{:04}{:02}{:02}-{sequence:03}",
        date.year(),
        u8::from(date.month()),
        date.day()
    ))
}

async fn lock_work_order(
    tx: &mut Transaction<'_, Postgres>,
    work_order_id: WorkOrderId,
) -> Result<WorkOrderRow, PgWorkOrderError> {
    let row = sqlx::query(
        r#"
        SELECT id, request_no, branch_id, equipment_id, customer_id, site_id,
               status, priority, result_type, maintenance_type, maintenance_cause,
               EXISTS (
                   SELECT 1
                   FROM evidence_media e
                   WHERE e.work_order_id = work_orders.id
                     AND e.stage IN ('AFTER', 'REPORT')
                     AND e.worm_replica_status = 'VERIFIED'
               ) AND NOT EXISTS (
                   SELECT 1
                   FROM evidence_media e
                   WHERE e.work_order_id = work_orders.id
                     AND e.stage IN ('AFTER', 'REPORT')
                     AND e.worm_replica_status <> 'VERIFIED'
               ) AS evidence_verified
        FROM work_orders
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("work order was not found"))?;

    work_order_row_from_row(&row)
}

async fn fetch_work_order_summary_tx(
    tx: &mut Transaction<'_, Postgres>,
    work_order_id: WorkOrderId,
) -> Result<WorkOrderSummary, PgWorkOrderError> {
    let row = sqlx::query(
        r#"
        SELECT id, request_no, branch_id, equipment_id, customer_id, site_id,
               status, priority, result_type, maintenance_type, maintenance_cause,
               EXISTS (
                   SELECT 1
                   FROM evidence_media e
                   WHERE e.work_order_id = work_orders.id
                     AND e.stage IN ('AFTER', 'REPORT')
                     AND e.worm_replica_status = 'VERIFIED'
               ) AND NOT EXISTS (
                   SELECT 1
                   FROM evidence_media e
                   WHERE e.work_order_id = work_orders.id
                     AND e.stage IN ('AFTER', 'REPORT')
                     AND e.worm_replica_status <> 'VERIFIED'
               ) AS evidence_verified
        FROM work_orders
        WHERE id = $1
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("work order was not found"))?;
    work_order_summary_from_row(&row)
}

async fn fetch_work_order_summary_pool(
    tx: &mut Transaction<'_, Postgres>,
    work_order_id: WorkOrderId,
) -> Result<WorkOrderSummary, PgWorkOrderError> {
    let row = sqlx::query(
        r#"
        SELECT id, request_no, branch_id, equipment_id, customer_id, site_id,
               status, priority, result_type, maintenance_type, maintenance_cause,
               EXISTS (
                   SELECT 1
                   FROM evidence_media e
                   WHERE e.work_order_id = work_orders.id
                     AND e.stage IN ('AFTER', 'REPORT')
                     AND e.worm_replica_status = 'VERIFIED'
               ) AND NOT EXISTS (
                   SELECT 1
                   FROM evidence_media e
                   WHERE e.work_order_id = work_orders.id
                     AND e.stage IN ('AFTER', 'REPORT')
                     AND e.worm_replica_status <> 'VERIFIED'
               ) AS evidence_verified
        FROM work_orders
        WHERE id = $1
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("work order was not found"))?;
    work_order_summary_from_row(&row)
}

fn work_order_row_from_row(row: &sqlx::postgres::PgRow) -> Result<WorkOrderRow, PgWorkOrderError> {
    let status: String = row.try_get("status")?;
    let priority: String = row.try_get("priority")?;
    let result_type: String = row.try_get("result_type")?;
    let maintenance_type: Option<String> = row.try_get("maintenance_type")?;
    let maintenance_cause: Option<String> = row.try_get("maintenance_cause")?;
    Ok(WorkOrderRow {
        id: row.try_get("id")?,
        request_no: row.try_get("request_no")?,
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        equipment_id: EquipmentId::from_uuid(row.try_get("equipment_id")?),
        customer_id: CustomerId::from_uuid(row.try_get("customer_id")?),
        site_id: SiteId::from_uuid(row.try_get("site_id")?),
        status: WorkOrderStatus::from_db_str(&status)?,
        priority: PriorityLevel::from_db_str(&priority)?,
        result_type: WorkResultType::from_db_str(&result_type)?,
        maintenance_type: maintenance_type
            .as_deref()
            .map(MaintenanceType::from_db_str)
            .transpose()?,
        maintenance_cause: maintenance_cause
            .as_deref()
            .map(MaintenanceCause::from_db_str)
            .transpose()?,
        evidence_verified: row.try_get("evidence_verified")?,
    })
}

fn work_order_summary_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<WorkOrderSummary, PgWorkOrderError> {
    let row = work_order_row_from_row(row)?;
    Ok(WorkOrderSummary {
        id: WorkOrderId::from_uuid(row.id),
        request_no: row.request_no,
        branch_id: row.branch_id,
        equipment_id: row.equipment_id,
        customer_id: row.customer_id,
        site_id: row.site_id,
        status: row.status,
        priority: row.priority,
        result_type: row.result_type,
        maintenance_type: row.maintenance_type,
        maintenance_cause: row.maintenance_cause,
        evidence_verified: row.evidence_verified,
    })
}

/// Minimal locked view of a settlement for transition decisions.
struct SettlementRow {
    status: SettlementStatus,
    submitted_by: Option<uuid::Uuid>,
}

async fn lock_settlement(
    tx: &mut Transaction<'_, Postgres>,
    settlement_id: uuid::Uuid,
) -> Result<SettlementRow, PgWorkOrderError> {
    let row = sqlx::query(
        r#"
        SELECT status, submitted_by
        FROM work_order_settlements
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(settlement_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("settlement was not found"))?;
    let status: String = row.try_get("status")?;
    Ok(SettlementRow {
        status: SettlementStatus::from_db_str(&status)?,
        submitted_by: row.try_get("submitted_by")?,
    })
}

async fn fetch_settlement_summary_tx(
    tx: &mut Transaction<'_, Postgres>,
    settlement_id: uuid::Uuid,
) -> Result<SettlementSummary, PgWorkOrderError> {
    let row = sqlx::query(
        r#"
        SELECT id, work_order_id, branch_id, status, total_amount_krw, voucher_ref,
               note, created_by, submitted_by, submitted_at, approved_by, approved_at,
               created_at, updated_at
        FROM work_order_settlements
        WHERE id = $1
        "#,
    )
    .bind(settlement_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("settlement was not found"))?;
    let status: String = row.try_get("status")?;
    let line_rows = sqlx::query(
        r#"
        SELECT id, kind, label, amount_krw, source_ref, sort_order
        FROM work_order_settlement_lines
        WHERE settlement_id = $1
        ORDER BY sort_order, id
        "#,
    )
    .bind(settlement_id)
    .fetch_all(tx.as_mut())
    .await?;
    let lines = line_rows
        .iter()
        .map(|line| {
            let kind: String = line.try_get("kind")?;
            Ok(SettlementLineSummary {
                id: line.try_get("id")?,
                kind: SettlementLineKind::from_db_str(&kind)?,
                label: line.try_get("label")?,
                amount_krw: line.try_get("amount_krw")?,
                source_ref: line.try_get("source_ref")?,
                sort_order: line.try_get("sort_order")?,
            })
        })
        .collect::<Result<Vec<_>, PgWorkOrderError>>()?;

    Ok(SettlementSummary {
        id: row.try_get("id")?,
        work_order_id: WorkOrderId::from_uuid(row.try_get("work_order_id")?),
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        status: SettlementStatus::from_db_str(&status)?,
        total_amount_krw: row.try_get("total_amount_krw")?,
        voucher_ref: row.try_get("voucher_ref")?,
        note: row.try_get("note")?,
        lines,
        created_by: UserId::from_uuid(row.try_get("created_by")?),
        submitted_by: row
            .try_get::<Option<uuid::Uuid>, _>("submitted_by")?
            .map(UserId::from_uuid),
        submitted_at: row.try_get("submitted_at")?,
        approved_by: row
            .try_get::<Option<uuid::Uuid>, _>("approved_by")?
            .map(UserId::from_uuid),
        approved_at: row.try_get("approved_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

#[allow(clippy::too_many_arguments)]
async fn insert_status_history(
    tx: &mut Transaction<'_, Postgres>,
    work_order_id: WorkOrderId,
    actor: Option<UserId>,
    action: &str,
    from_status: Option<WorkOrderStatus>,
    to_status: WorkOrderStatus,
    occurred_at: console_kernel_core::Timestamp,
    org_uuid: uuid::Uuid,
) -> Result<(), PgWorkOrderError> {
    sqlx::query(
        r#"
        INSERT INTO work_order_status_history (
            work_order_id, actor, action, from_status, to_status, occurred_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(actor.map(|user| *user.as_uuid()))
    .bind(action)
    .bind(from_status.map(WorkOrderStatus::as_db_str))
    .bind(to_status.as_db_str())
    .bind(occurred_at)
    .bind(org_uuid)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

/// Append the terminal maintenance snapshot after the current work-order FSM
/// has reached FINAL_COMPLETED. The DB migration repeats all terminal/evidence
/// and tenant checks; this adapter only projects already-existing evidence and
/// cost-ledger identities, never inventing a cost.
async fn append_equipment_maintenance_history(
    tx: &mut Transaction<'_, Postgres>,
    work_order_id: WorkOrderId,
    equipment_id: EquipmentId,
    completed_at: console_kernel_core::Timestamp,
    org_uuid: uuid::Uuid,
) -> Result<(), PgWorkOrderError> {
    sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT append_equipment_maintenance_history($1, $2, $3, $4)",
    )
    .bind(org_uuid)
    .bind(*equipment_id.as_uuid())
    .bind(*work_order_id.as_uuid())
    .bind(completed_at)
    .fetch_one(tx.as_mut())
    .await?;

    Ok(())
}

async fn update_status(
    tx: &mut Transaction<'_, Postgres>,
    row: WorkOrderRow,
    actor: UserId,
    action: &str,
    to_status: WorkOrderStatus,
    occurred_at: console_kernel_core::Timestamp,
    org_uuid: uuid::Uuid,
) -> Result<WorkOrderSummary, PgWorkOrderError> {
    sqlx::query(
        r#"
        UPDATE work_orders
        SET status = $2, updated_at = $3
        WHERE id = $1
        "#,
    )
    .bind(row.id)
    .bind(to_status.as_db_str())
    .bind(occurred_at)
    .execute(tx.as_mut())
    .await?;
    insert_status_history(
        tx,
        WorkOrderId::from_uuid(row.id),
        Some(actor),
        action,
        Some(row.status),
        to_status,
        occurred_at,
        org_uuid,
    )
    .await?;
    fetch_work_order_summary_tx(tx, WorkOrderId::from_uuid(row.id)).await
}

#[allow(clippy::too_many_arguments)]
async fn insert_approval_step(
    tx: &mut Transaction<'_, Postgres>,
    work_order_id: WorkOrderId,
    step_order: i16,
    role: ApprovalRole,
    approver_id: Option<UserId>,
    status: &str,
    requested_at: Option<console_kernel_core::Timestamp>,
    org_uuid: uuid::Uuid,
) -> Result<(), PgWorkOrderError> {
    sqlx::query(
        r#"
        INSERT INTO work_order_approval_steps (
            work_order_id, step_order, role, approver_id, status, requested_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(step_order)
    .bind(role.as_db_str())
    .bind(approver_id.map(|user| *user.as_uuid()))
    .bind(status)
    .bind(requested_at)
    .bind(org_uuid)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn ensure_actor_assignment(
    tx: &mut Transaction<'_, Postgres>,
    work_order_id: WorkOrderId,
    actor: UserId,
    requirement: AssignmentRequirement,
) -> Result<(), PgWorkOrderError> {
    // One pass over the work order's assignments: total rows and the subset
    // assigned to the actor, via a filtered aggregate.
    let row = sqlx::query(
        r#"
        SELECT
            COUNT(*) AS total,
            COUNT(*) FILTER (WHERE mechanic_id = $2) AS assigned
        FROM work_order_assignments
        WHERE work_order_id = $1
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*actor.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    let total: i64 = row.try_get("total")?;
    let assigned: i64 = row.try_get("assigned")?;

    // `work_order_assignments` has UNIQUE (work_order_id, mechanic_id), so
    // `assigned` is always 0 or 1; `>= 1` is used (rather than `== 1`) as a
    // defensive equivalent that stays correct if that constraint were ever
    // relaxed to allow a mechanic to hold multiple rows (e.g. multiple roles).
    if assigned >= 1 || (requirement == AssignmentRequirement::Optional && total == 0) {
        return Ok(());
    }

    Err(KernelError::forbidden("actor is not assigned to this work order").into())
}

/// Record the actor as the PRIMARY assignee when the work order has no assignees
/// yet (claim-and-start path). No-op if any assignment already exists, so an order
/// that was formally assigned keeps its existing assignees untouched.
async fn self_claim_if_unassigned(
    tx: &mut Transaction<'_, Postgres>,
    work_order_id: WorkOrderId,
    actor: UserId,
    occurred_at: time::OffsetDateTime,
    org_uuid: uuid::Uuid,
) -> Result<(), PgWorkOrderError> {
    let total: i64 = sqlx::query(
        "SELECT COUNT(*) AS total FROM work_order_assignments WHERE work_order_id = $1",
    )
    .bind(*work_order_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?
    .try_get("total")?;
    if total > 0 {
        return Ok(());
    }
    sqlx::query(
        r#"
        INSERT INTO work_order_assignments (
            work_order_id, mechanic_id, role, assigned_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*actor.as_uuid())
    .bind(AssignmentRole::Primary.as_db_str())
    .bind(occurred_at)
    .bind(org_uuid)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn pending_non_mechanic_approval_step(
    tx: &mut Transaction<'_, Postgres>,
    work_order_id: WorkOrderId,
) -> Result<PendingApprovalStep, PgWorkOrderError> {
    let row = sqlx::query(
        r#"
        SELECT id, role, approver_id
        FROM work_order_approval_steps
        WHERE work_order_id = $1
          AND role IN ('ADMIN', 'EXECUTIVE')
          AND status = 'PENDING'
        ORDER BY step_order
        LIMIT 1
        FOR UPDATE
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::conflict("no pending non-mechanic approval step"))?;

    let role: String = row.try_get("role")?;
    let role = match role.as_str() {
        "ADMIN" => ApprovalRole::Admin,
        "EXECUTIVE" => ApprovalRole::Executive,
        other => {
            return Err(KernelError::validation(format!("unknown approval role {other:?}")).into());
        }
    };
    let approver_uuid: Option<uuid::Uuid> = row.try_get("approver_id")?;
    Ok(PendingApprovalStep {
        id: row.try_get("id")?,
        role,
        approver_id: approver_uuid.map(UserId::from_uuid),
    })
}

async fn lock_daily_plan(
    tx: &mut Transaction<'_, Postgres>,
    plan_id: DailyPlanId,
) -> Result<DailyPlanSummary, PgWorkOrderError> {
    let row = sqlx::query(
        r#"
        SELECT id, branch_id, mechanic_id, plan_date, status
        FROM daily_work_plans
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(*plan_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("daily plan was not found"))?;
    let mut plan = daily_plan_summary_from_row(&row)?;
    plan.items = daily_plan_items(tx, plan_id).await?;
    Ok(plan)
}

fn daily_plan_summary_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<DailyPlanSummary, PgWorkOrderError> {
    let status: String = row.try_get("status")?;
    Ok(DailyPlanSummary {
        id: DailyPlanId::from_uuid(row.try_get("id")?),
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        mechanic_id: UserId::from_uuid(row.try_get("mechanic_id")?),
        plan_date: row.try_get("plan_date")?,
        status: DailyPlanStatus::from_db_str(&status)?,
        items: Vec::new(),
    })
}

async fn daily_plan_items(
    tx: &mut Transaction<'_, Postgres>,
    plan_id: DailyPlanId,
) -> Result<Vec<DailyPlanItemSummary>, PgWorkOrderError> {
    let rows = sqlx::query(
        r#"
        SELECT
            i.work_order_id,
            w.request_no,
            e.equipment_no,
            e.management_no,
            c.name AS customer_name,
            s.name AS site_name,
            i.description,
            i.sort_order
        FROM daily_work_plan_items i
        LEFT JOIN work_orders w ON w.id = i.work_order_id AND w.org_id = i.org_id
        LEFT JOIN registry_equipment e ON e.id = w.equipment_id AND e.org_id = w.org_id
        LEFT JOIN registry_customers c ON c.id = w.customer_id AND c.org_id = w.org_id
        LEFT JOIN registry_sites s ON s.id = w.site_id AND s.org_id = w.org_id
        WHERE i.plan_id = $1
        ORDER BY i.sort_order ASC, i.id ASC
        "#,
    )
    .bind(*plan_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await?;
    rows.iter()
        .map(|row| {
            let work_order_id: Option<uuid::Uuid> = row.try_get("work_order_id")?;
            Ok(DailyPlanItemSummary {
                work_order_id: work_order_id.map(WorkOrderId::from_uuid),
                request_no: row.try_get("request_no")?,
                equipment_no: row.try_get("equipment_no")?,
                management_no: row.try_get("management_no")?,
                customer_name: row.try_get("customer_name")?,
                site_name: row.try_get("site_name")?,
                description: row.try_get("description")?,
                sort_order: row.try_get("sort_order")?,
            })
        })
        .collect()
}
