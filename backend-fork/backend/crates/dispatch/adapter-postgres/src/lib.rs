//! Postgres adapter for the P1 emergency dispatch engine.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_dispatch_application::{
    ActionInboxDispatchOffer, DispatchCandidatePage, DispatchCandidateSummary, DispatchQueueCursor,
    DispatchQueueDispatch, DispatchQueueItem, DispatchQueuePage, DispatchQueueStats,
    ExpireP1DispatchCommand, ForceAssignP1DispatchCommand, IncidentLocationInput,
    ListDispatchQueueQuery, MyDispatchOffer, P1DispatchResponsePage, P1DispatchResponseSummary,
    P1DispatchSummary, RespondP1DispatchCommand, StartP1DispatchCommand, dispatch_audit_event,
    resolution_after_snapshot, response_after_snapshot, start_after_snapshot,
};
use console_dispatch_domain::{
    CandidateScore, DispatchCandidate, DispatchResponseKind, DispatchStatus, DispatchTimerConfig,
    GeoPoint, P1Dispatch, TechnicianLoad, score_candidate,
};
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, ErrorKind, KernelError, OrgId,
    P1DispatchAlertId, P1DispatchId, TraceContext, UserId, WorkOrderId,
};
use console_platform_db::{DbError, insert_audit_event, with_audit, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use console_workorder_application::work_order_audit_event;
use console_workorder_domain::{
    ApprovalRole, AssignmentRole, PriorityLevel, TransitionGuardContext, WorkOrderStatus,
    validate_status_transition,
};
use sqlx::{PgPool, Postgres, Row, Transaction};
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum PgDispatchError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl PgDispatchError {
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

impl From<sqlx::Error> for PgDispatchError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

#[derive(Debug, Clone)]
pub struct PgDispatchStore {
    pool: PgPool,
}

impl PgDispatchStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn start_dispatch(
        &self,
        command: StartP1DispatchCommand,
        timers: DispatchTimerConfig,
    ) -> Result<P1DispatchSummary, PgDispatchError> {
        // The unique work-order constraint is an idempotency boundary, not an
        // error a retry must guess around.  Same intent replays silently;
        // changed intent is a conflict and never adds fan-out or audit rows.
        if let Some(existing) = existing_dispatch_intent(&self.pool, command.work_order_id).await? {
            if existing.include_region == command.include_region
                && existing.incident_location == command.incident_location
            {
                return self.dispatch(existing.dispatch_id).await;
            }
            return Err(KernelError::conflict(
                "dispatch already exists with different start input",
            )
            .into());
        }
        let dispatch_id = P1DispatchId::new();
        let dispatch = P1Dispatch::start(
            dispatch_id,
            command.work_order_id,
            command.occurred_at,
            timers,
        )?;
        let work_order = work_order_head(&self.pool, command.work_order_id).await?;
        if work_order.priority != PriorityLevel::P1 {
            return Err(
                KernelError::conflict("P1 dispatch requires work-order priority P1").into(),
            );
        }
        let on_duty_since = command
            .occurred_at
            .checked_sub(timers.gps_ping_freshness)
            .ok_or_else(|| KernelError::validation("dispatch GPS freshness overflows time"))?;
        let target_count = count_dispatch_targets(
            &self.pool,
            work_order.branch_id,
            command.include_region,
            on_duty_since,
        )
        .await?;
        let incident = command
            .incident_location
            .map(validate_incident)
            .transpose()?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let event = dispatch_audit_event(
            "p1_dispatch.start",
            Some(command.actor),
            work_order.branch_id,
            dispatch_id,
            command.trace.clone(),
            command.occurred_at,
        )?
        .with_org(org)
        .with_snapshots(
            None,
            Some(start_after_snapshot(
                command.work_order_id,
                target_count,
                command.include_region,
            )),
        );
        let actor = command.actor;
        let include_region = command.include_region;
        let occurred_at = command.occurred_at;
        let work_order_id = command.work_order_id;
        let branch_id = work_order.branch_id;

        let requested_include_region = command.include_region;
        let requested_incident_location = command.incident_location;
        let result = with_audit::<_, P1DispatchSummary, PgDispatchError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let locked = lock_work_order(tx, work_order_id).await?;
                if locked.branch_id != branch_id {
                    return Err(KernelError::conflict("work-order branch changed").into());
                }
                if locked.priority != PriorityLevel::P1 {
                    return Err(KernelError::conflict(
                        "P1 dispatch requires work-order priority P1",
                    )
                    .into());
                }
                insert_dispatch_row(
                    tx,
                    InsertDispatchRow {
                        dispatch,
                        branch_id,
                        actor,
                        incident,
                        include_region,
                        occurred_at,
                        org_uuid,
                    },
                )
                .await?;
                insert_dispatch_targets(
                    tx,
                    dispatch_id,
                    branch_id,
                    include_region,
                    on_duty_since,
                    occurred_at,
                    org_uuid,
                )
                .await?;
                insert_push_alerts_for_targets(tx, dispatch_id, occurred_at).await?;
                fetch_dispatch_summary_tx(tx, dispatch_id).await
            })
        })
        .await;

        // A concurrent first request can pass the preflight read before the
        // winner commits the unique work-order row. Treat only that database
        // uniqueness race as the same idempotency boundary; the failed
        // transaction rolls back its audit event, then replay the winner or
        // report a changed intent without additional fan-out.
        match result {
            Ok(summary) => Ok(summary),
            Err(error) if is_unique_work_order_dispatch_conflict(&error) => {
                let Some(existing) =
                    existing_dispatch_intent(&self.pool, command.work_order_id).await?
                else {
                    return Err(error);
                };
                if existing.include_region == requested_include_region
                    && existing.incident_location == requested_incident_location
                {
                    self.dispatch(existing.dispatch_id).await
                } else {
                    Err(
                        KernelError::conflict("dispatch already exists with different start input")
                            .into(),
                    )
                }
            }
            Err(error) => Err(error),
        }
    }

    pub async fn record_response(
        &self,
        command: RespondP1DispatchCommand,
        timers: DispatchTimerConfig,
    ) -> Result<P1DispatchSummary, PgDispatchError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let actor = command.actor;
        let dispatch_id = command.dispatch_id;
        let response = command.response;
        let occurred_at = command.occurred_at;
        let trace = command.trace.clone();

        with_audits::<_, P1DispatchSummary, PgDispatchError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let mut dispatch = lock_dispatch(tx, dispatch_id).await?;
                ensure_technician_target(tx, dispatch_id, actor).await?;
                if let Some(existing_response) = response_for_target(tx, dispatch_id, actor).await?
                {
                    if existing_response != response {
                        return Err(KernelError::conflict(
                            "dispatch target already responded with a different response",
                        )
                        .into());
                    }
                    return Ok((
                        fetch_dispatch_summary_tx(tx, dispatch_id).await?,
                        Vec::new(),
                    ));
                }

                dispatch.record_response(response, occurred_at)?;
                insert_response(tx, dispatch_id, actor, response, occurred_at, org_uuid).await?;

                let event = dispatch_audit_event(
                    "p1_dispatch.respond",
                    Some(actor),
                    branch_for_work_order_tx(tx, dispatch.work_order_id).await?,
                    dispatch_id,
                    trace.clone(),
                    occurred_at,
                )?
                .with_org(org)
                .with_snapshots(None, Some(response_after_snapshot(response)));

                let accepted_count = accepted_count_tx(tx, dispatch_id).await?;
                if response == DispatchResponseKind::Accept
                    && accepted_count >= 2
                    && dispatch.status == DispatchStatus::Broadcasting
                {
                    let score = auto_assign_tx(
                        tx,
                        &mut dispatch,
                        actor,
                        trace,
                        occurred_at,
                        timers,
                        org_uuid,
                    )
                    .await?;
                    update_score_columns(tx, dispatch_id, score).await?;
                }

                Ok((
                    fetch_dispatch_summary_tx(tx, dispatch_id).await?,
                    vec![event],
                ))
            })
        })
        .await
    }

    pub async fn expire_accept_window(
        &self,
        command: ExpireP1DispatchCommand,
    ) -> Result<P1DispatchSummary, PgDispatchError> {
        let org = current_org().map_err(KernelError::from)?;
        let dispatch_id = command.dispatch_id;
        let occurred_at = command.occurred_at;
        let trace = command.trace;

        with_audits::<_, P1DispatchSummary, PgDispatchError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let dispatch = lock_dispatch(tx, dispatch_id).await?;
                if dispatch.status != DispatchStatus::Broadcasting
                    || accepted_count_tx(tx, dispatch_id).await? >= 2
                {
                    return Ok((
                        fetch_dispatch_summary_tx(tx, dispatch_id).await?,
                        Vec::new(),
                    ));
                }
                sqlx::query(
                    r#"
                    UPDATE p1_dispatches
                    SET status = 'MANAGER_FORCE_PENDING',
                        manager_force_pending_at = $2,
                        updated_at = $2
                    WHERE id = $1
                    "#,
                )
                .bind(*dispatch_id.as_uuid())
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;
                insert_manager_force_alerts(tx, dispatch_id, occurred_at).await?;
                let summary = fetch_dispatch_summary_tx(tx, dispatch_id).await?;
                let event = dispatch_audit_event(
                    "p1_dispatch.force_pending",
                    None,
                    summary.branch_id,
                    dispatch_id,
                    trace,
                    occurred_at,
                )?
                .with_org(org);
                Ok((summary, vec![event]))
            })
        })
        .await
    }

    pub async fn mark_alimtalk_no_ack(
        &self,
        command: ExpireP1DispatchCommand,
    ) -> Result<P1DispatchSummary, PgDispatchError> {
        let org = current_org().map_err(KernelError::from)?;
        let dispatch_id = command.dispatch_id;
        let occurred_at = command.occurred_at;
        let trace = command.trace;

        with_audits::<_, P1DispatchSummary, PgDispatchError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let dispatch = lock_dispatch(tx, dispatch_id).await?;
                if dispatch.status != DispatchStatus::Broadcasting {
                    return Ok((fetch_dispatch_summary_tx(tx, dispatch_id).await?, Vec::new()));
                }
                let inserted = sqlx::query(
                    r#"
                    INSERT INTO p1_dispatch_alerts (
                        dispatch_id, recipient_user_id, alert_type, status, created_at, org_id
                    )
                    SELECT t.dispatch_id, t.user_id, 'ALIMTALK_NO_ACK',
                           CASE WHEN u.phone IS NOT NULL AND btrim(u.phone) <> '' THEN 'PENDING' ELSE 'SKIPPED' END,
                           $2, t.org_id
                    FROM p1_dispatch_targets t
                    JOIN users u ON u.id = t.user_id
                    LEFT JOIN p1_dispatch_responses r ON r.dispatch_id = t.dispatch_id AND r.user_id = t.user_id
                    WHERE t.dispatch_id = $1 AND t.target_role = 'TECHNICIAN' AND r.id IS NULL
                      AND NOT EXISTS (
                          SELECT 1 FROM p1_dispatch_alerts existing
                          WHERE existing.dispatch_id = t.dispatch_id
                            AND existing.recipient_user_id = t.user_id
                            AND existing.alert_type = 'ALIMTALK_NO_ACK'
                      )
                    "#,
                )
                .bind(*dispatch_id.as_uuid())
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                let summary = fetch_dispatch_summary_tx(tx, dispatch_id).await?;
                if inserted == 0 {
                    return Ok((summary, Vec::new()));
                }
                let event = dispatch_audit_event(
                    "p1_dispatch.alimtalk_no_ack", None, summary.branch_id, dispatch_id, trace, occurred_at,
                )?
                .with_org(org);
                Ok((summary, vec![event]))
            })
        })
        .await
    }

    pub async fn mark_manual_call_required(
        &self,
        command: ExpireP1DispatchCommand,
    ) -> Result<P1DispatchSummary, PgDispatchError> {
        let org = current_org().map_err(KernelError::from)?;
        let dispatch_id = command.dispatch_id;
        let occurred_at = command.occurred_at;
        let trace = command.trace;

        with_audits::<_, P1DispatchSummary, PgDispatchError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let dispatch = lock_dispatch(tx, dispatch_id).await?;
                let already_marked: bool = sqlx::query_scalar(
                    "SELECT manual_call_required_at IS NOT NULL FROM p1_dispatches WHERE id = $1",
                )
                .bind(*dispatch_id.as_uuid())
                .fetch_one(tx.as_mut())
                .await?;
                if dispatch.status == DispatchStatus::AutoAssigned || already_marked {
                    return Ok((
                        fetch_dispatch_summary_tx(tx, dispatch_id).await?,
                        Vec::new(),
                    ));
                }
                sqlx::query(
                    r#"
                    UPDATE p1_dispatches
                    SET manual_call_required_at = $2,
                        manual_call_cleared_at = NULL,
                        updated_at = $2
                    WHERE id = $1
                    "#,
                )
                .bind(*dispatch_id.as_uuid())
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;
                let summary = fetch_dispatch_summary_tx(tx, dispatch_id).await?;
                let event = dispatch_audit_event(
                    "dispatch.escalation.manual_call_required",
                    None,
                    summary.branch_id,
                    dispatch_id,
                    trace,
                    occurred_at,
                )?
                .with_org(org)
                .with_snapshots(
                    None,
                    Some(serde_json::json!({
                        "manual_call_required": true,
                        "manual_call_required_at": occurred_at,
                    })),
                );
                Ok((summary, vec![event]))
            })
        })
        .await
    }

    pub async fn force_assign(
        &self,
        command: ForceAssignP1DispatchCommand,
    ) -> Result<P1DispatchSummary, PgDispatchError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let actor = command.actor;
        let dispatch_id = command.dispatch_id;
        let mechanic_id = command.mechanic_id;
        let trace = command.trace;
        let occurred_at = command.occurred_at;

        with_audits::<_, P1DispatchSummary, PgDispatchError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let mut dispatch = lock_dispatch(tx, dispatch_id).await?;
                if dispatch.status == DispatchStatus::AutoAssigned {
                    let assigned: Option<uuid::Uuid> = sqlx::query_scalar(
                        "SELECT auto_assigned_mechanic_id FROM p1_dispatches WHERE id = $1",
                    )
                    .bind(*dispatch_id.as_uuid())
                    .fetch_one(tx.as_mut())
                    .await?;
                    if assigned == Some(*mechanic_id.as_uuid()) {
                        return Ok((fetch_dispatch_summary_tx(tx, dispatch_id).await?, Vec::new()));
                    }
                    return Err(KernelError::conflict(
                        "dispatch already force-assigned to a different mechanic",
                    )
                    .into());
                }
                ensure_technician_target(tx, dispatch_id, mechanic_id).await?;
                if target_declined(tx, dispatch_id, mechanic_id).await? {
                    return Err(KernelError::conflict(
                        "cannot force-assign a mechanic who declined this dispatch",
                    )
                    .into());
                }
                dispatch.force_assign()?;
                assign_work_order_tx(
                    tx,
                    dispatch.work_order_id,
                    mechanic_id,
                    actor,
                    trace.clone(),
                    occurred_at,
                    org_uuid,
                )
                .await?;
                sqlx::query(
                    r#"
                    UPDATE p1_dispatches
                    SET status = 'AUTO_ASSIGNED', auto_assigned_mechanic_id = $2,
                        manual_call_cleared_at = CASE WHEN manual_call_required_at IS NOT NULL THEN $3 ELSE manual_call_cleared_at END,
                        updated_at = $3
                    WHERE id = $1
                    "#,
                )
                .bind(*dispatch_id.as_uuid())
                .bind(*mechanic_id.as_uuid())
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;
                let summary = fetch_dispatch_summary_tx(tx, dispatch_id).await?;
                let event = dispatch_audit_event(
                    "p1_dispatch.force_assign",
                    Some(actor),
                    summary.branch_id,
                    dispatch_id,
                    trace,
                    occurred_at,
                )?
                .with_org(org)
                .with_snapshots(
                    None,
                    Some(resolution_after_snapshot(
                        DispatchStatus::AutoAssigned,
                        summary.accepted_count,
                        Some(mechanic_id),
                    )),
                );
                Ok((summary, vec![event]))
            })
        })
        .await
    }

    pub async fn list_dispatch_queue(
        &self,
        query: ListDispatchQueueQuery,
    ) -> Result<DispatchQueuePage, PgDispatchError> {
        let org = current_org().map_err(KernelError::from)?;
        let statuses = query
            .statuses
            .iter()
            .map(|s| s.as_db_str())
            .collect::<Vec<_>>();
        let (all_branches, branches) = match query.branch_scope {
            BranchScope::All => (true, Vec::new()),
            BranchScope::Branches(values) => (
                false,
                values.iter().map(|id| *id.as_uuid()).collect::<Vec<_>>(),
            ),
        };
        let cursor = query.after;
        let now = query.now;
        let limit = query.limit.clamp(1, 200);
        with_org_conn::<_, _, PgDispatchError>(&self.pool, org, move |tx| Box::pin(async move {
            let (as_of, cursor_due, cursor_updated, cursor_id) = match cursor {
                Some(cursor) => (cursor.as_of(), cursor.target_due_at(), Some(cursor.updated_at()), Some(*cursor.work_order_id().as_uuid())),
                None => (now, None, None, None),
            };
            let rows = sqlx::query(r#"
                SELECT w.id, w.request_no, w.branch_id, w.status, w.priority, w.symptom,
                       w.equipment_id, w.customer_id, w.site_id, w.target_due_at, w.updated_at,
                       primary_assignment.mechanic_id AS assigned_mechanic_id,
                       d.id AS dispatch_id, d.status AS dispatch_status, d.accept_window_ends_at,
                       d.manual_call_required_at IS NOT NULL AND d.manual_call_cleared_at IS NULL AS manual_call_required,
                       COALESCE(targets.target_count, 0) AS target_count,
                       COALESCE(responses.accepted_count, 0) AS accepted_count,
                       COALESCE(responses.declined_count, 0) AS declined_count
                FROM work_orders w
                LEFT JOIN work_order_assignments primary_assignment
                  ON primary_assignment.work_order_id = w.id AND primary_assignment.role = 'PRIMARY'
                LEFT JOIN p1_dispatches d ON d.work_order_id = w.id
                LEFT JOIN LATERAL (SELECT count(*)::bigint AS target_count FROM p1_dispatch_targets t WHERE t.dispatch_id=d.id) targets ON d.id IS NOT NULL
                LEFT JOIN LATERAL (SELECT count(*) FILTER (WHERE r.response='ACCEPT')::bigint AS accepted_count,
                                          count(*) FILTER (WHERE r.response='DECLINE')::bigint AS declined_count
                                   FROM p1_dispatch_responses r WHERE r.dispatch_id=d.id) responses ON d.id IS NOT NULL
                WHERE w.status = ANY($1) AND w.updated_at <= $2
                  AND ($3::boolean OR w.branch_id = ANY($4))
                  AND ($5::timestamptz IS NULL
                       OR CASE WHEN $6::timestamptz IS NULL THEN
                            w.target_due_at IS NULL AND (w.updated_at < $7::timestamptz OR (w.updated_at = $7::timestamptz AND w.id > $8::uuid))
                          ELSE w.target_due_at IS NULL OR w.target_due_at > $6
                            OR (w.target_due_at = $6 AND (w.updated_at < $7::timestamptz OR (w.updated_at = $7::timestamptz AND w.id > $8::uuid))) END)
                ORDER BY w.target_due_at ASC NULLS LAST, w.updated_at DESC, w.id ASC
                LIMIT $9
            "#)
            .bind(&statuses).bind(as_of).bind(all_branches).bind(&branches)
            .bind(cursor_updated).bind(cursor_due).bind(cursor_updated).bind(cursor_id).bind(limit + 1)
            .fetch_all(tx.as_mut()).await?;
            let has_more = rows.len() > usize::try_from(limit).unwrap_or(200);
            let rows = rows.into_iter().take(usize::try_from(limit).unwrap_or(200)).collect::<Vec<_>>();
            let mut items = Vec::with_capacity(rows.len());
            for row in rows {
                let due: Option<OffsetDateTime> = row.try_get("target_due_at")?;
                let updated: OffsetDateTime = row.try_get("updated_at")?;
                let id = WorkOrderId::from_uuid(row.try_get("id")?);
                let dispatch_id: Option<uuid::Uuid> = row.try_get("dispatch_id")?;
                let dispatch = match dispatch_id {
                    Some(id) => Some(DispatchQueueDispatch { id: P1DispatchId::from_uuid(id), status: DispatchStatus::from_db_str(row.try_get("dispatch_status")?)?, accept_window_ends_at: row.try_get("accept_window_ends_at")?, target_count: row.try_get("target_count")?, accepted_count: row.try_get("accepted_count")?, declined_count: row.try_get("declined_count")?, manual_call_required: row.try_get("manual_call_required")? }),
                    None => None,
                };
                items.push(DispatchQueueItem { work_order_id:id, request_no:row.try_get("request_no")?, branch_id:BranchId::from_uuid(row.try_get("branch_id")?), status:WorkOrderStatus::from_db_str(row.try_get("status")?)?, priority:PriorityLevel::from_db_str(row.try_get("priority")?)?, symptom:row.try_get("symptom")?, equipment_id:row.try_get("equipment_id")?, customer_id:row.try_get("customer_id")?, site_id:row.try_get("site_id")?, target_due_at:due, assigned_mechanic_id:row.try_get::<Option<uuid::Uuid>, _>("assigned_mechanic_id")?.map(UserId::from_uuid), dispatch, updated_at:updated });
            }
            let stats_row = sqlx::query(r#"SELECT count(*) FILTER (WHERE a.mechanic_id IS NULL)::bigint AS unassigned_count,
                count(*) FILTER (WHERE w.target_due_at IS NOT NULL AND w.target_due_at <= $5)::bigint AS sla_due_count
                FROM work_orders w LEFT JOIN work_order_assignments a ON a.work_order_id=w.id AND a.role='PRIMARY'
                WHERE w.status=ANY($1) AND w.updated_at <= $2 AND ($3::boolean OR w.branch_id=ANY($4))"#)
                .bind(&statuses).bind(as_of).bind(all_branches).bind(&branches).bind(now).fetch_one(tx.as_mut()).await?;
            let next_after = if has_more {
                items
                    .last()
                    .map(|item| {
                        DispatchQueueCursor::encode(
                            as_of,
                            item.target_due_at,
                            item.updated_at,
                            item.work_order_id,
                        )
                    })
                    .transpose()?
            } else {
                None
            };
            Ok(DispatchQueuePage { items, next_after, stats: DispatchQueueStats { unassigned_count:stats_row.try_get("unassigned_count")?, sla_due_count:stats_row.try_get("sla_due_count")? } })
        })).await
    }

    /// Candidate ranking deliberately computes only from a transient query. It
    /// never updates response score columns and never exposes coordinates.
    pub async fn dispatch_candidates(
        &self,
        dispatch_id: P1DispatchId,
        now: OffsetDateTime,
        timers: DispatchTimerConfig,
    ) -> Result<DispatchCandidatePage, PgDispatchError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgDispatchError>(&self.pool, org, move |tx| Box::pin(async move {
            let dispatch=sqlx::query("SELECT incident_latitude, incident_longitude FROM p1_dispatches WHERE id=$1").bind(*dispatch_id.as_uuid()).fetch_one(tx.as_mut()).await?;
            let incident=match (dispatch.try_get::<Option<f64>,_>("incident_latitude")?, dispatch.try_get::<Option<f64>,_>("incident_longitude")?) { (Some(lat),Some(lon))=>Some(GeoPoint::new(lat,lon)?), _=>None };
            let rows=sqlx::query(r#"SELECT t.user_id, r.response, r.responded_at, lc.status='GRANTED' AS consent_granted, lp.latitude, lp.longitude, lp.recorded_at,
                    COALESCE(load.p1_count,0) AS p1_count, COALESCE(load.p2_count,0) AS p2_count, COALESCE(load.p3_count,0) AS p3_count, COALESCE(load.other_count,0) AS other_count
                FROM p1_dispatch_targets t LEFT JOIN p1_dispatch_responses r ON r.dispatch_id=t.dispatch_id AND r.user_id=t.user_id
                LEFT JOIN location_consents lc ON lc.user_id=t.user_id
                LEFT JOIN LATERAL (SELECT latitude,longitude,recorded_at FROM location_pings WHERE user_id=t.user_id ORDER BY recorded_at DESC LIMIT 1) lp ON lc.status = 'GRANTED'
                LEFT JOIN LATERAL (SELECT count(*) FILTER(WHERE w.priority='P1')::bigint p1_count,count(*) FILTER(WHERE w.priority='P2')::bigint p2_count,count(*) FILTER(WHERE w.priority='P3')::bigint p3_count,count(*) FILTER(WHERE w.priority NOT IN('P1','P2','P3'))::bigint other_count FROM work_order_assignments a JOIN work_orders w ON w.id=a.work_order_id WHERE a.mechanic_id=t.user_id AND w.status NOT IN('FINAL_COMPLETED','CANCELLED','ARCHIVED')) load ON TRUE
                WHERE t.dispatch_id=$1 AND t.target_role='TECHNICIAN'"#).bind(*dispatch_id.as_uuid()).fetch_all(tx.as_mut()).await?;
            let mut items=Vec::with_capacity(rows.len());
            for row in rows { let latest=match(row.try_get::<Option<f64>,_>("latitude")?,row.try_get::<Option<f64>,_>("longitude")?){(Some(lat),Some(lon))=>Some(GeoPoint::new(lat,lon)?),_=>None}; let workload=TechnicianLoad{p1:row.try_get("p1_count")?,p2:row.try_get("p2_count")?,p3:row.try_get("p3_count")?,other:row.try_get("other_count")?}; let score=score_candidate(DispatchCandidate{mechanic_id:UserId::from_uuid(row.try_get("user_id")?),latest_location:latest,incident_location:incident,location_recorded_at:row.try_get("recorded_at")?,location_consent_granted:row.try_get::<Option<bool>,_>("consent_granted")?.unwrap_or(false),workload},now,timers.gps_ping_freshness); items.push(DispatchCandidateSummary{mechanic_id:score.mechanic_id,score_milli:score.score_milli,gps_ranked:score.gps_ranked,distance_meters:score.distance_meters,location_recorded_at:row.try_get("recorded_at")?,workload,score_reason:score.reason().to_owned(),response:row.try_get::<Option<&str>,_>("response")?.map(DispatchResponseKind::from_db_str).transpose()?,responded_at:row.try_get("responded_at")?}); }
            items.sort_by_key(|item|(item.score_milli,*item.mechanic_id.as_uuid())); Ok(DispatchCandidatePage{items})
        })).await
    }
    pub async fn dispatch_responses(
        &self,
        dispatch_id: P1DispatchId,
    ) -> Result<P1DispatchResponsePage, PgDispatchError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_,_,PgDispatchError>(&self.pool,org,move|tx|Box::pin(async move { let rows=sqlx::query("SELECT dispatch_id,user_id,response,responded_at,score_milli,gps_ranked,distance_meters,score_reason FROM p1_dispatch_responses WHERE dispatch_id=$1 ORDER BY responded_at ASC,user_id ASC").bind(*dispatch_id.as_uuid()).fetch_all(tx.as_mut()).await?; let mut items=Vec::with_capacity(rows.len()); for row in rows {items.push(P1DispatchResponseSummary{dispatch_id:P1DispatchId::from_uuid(row.try_get("dispatch_id")?),user_id:UserId::from_uuid(row.try_get("user_id")?),response:DispatchResponseKind::from_db_str(row.try_get("response")?)?,responded_at:row.try_get("responded_at")?,score_milli:row.try_get("score_milli")?,gps_ranked:row.try_get("gps_ranked")?,distance_meters:row.try_get("distance_meters")?,score_reason:row.try_get("score_reason")?});} Ok(P1DispatchResponsePage{items})})).await
    }

    pub async fn dispatch(
        &self,
        dispatch_id: P1DispatchId,
    ) -> Result<P1DispatchSummary, PgDispatchError> {
        fetch_dispatch_summary(&self.pool, dispatch_id).await
    }

    pub async fn work_order_branch(
        &self,
        work_order_id: WorkOrderId,
    ) -> Result<BranchId, PgDispatchError> {
        Ok(work_order_head(&self.pool, work_order_id).await?.branch_id)
    }

    /// List the caller's pending P1 offers: BROADCASTING dispatches that
    /// fanned out to the caller as a TECHNICIAN, still inside the accept
    /// window, with no response from the caller yet. Read-only and
    /// person-scoped — `user` is bound from the authenticated principal, so a
    /// caller can only ever see offers addressed to them (deny-by-omission).
    pub async fn list_my_pending_offers(
        &self,
        user: UserId,
        now: OffsetDateTime,
    ) -> Result<Vec<MyDispatchOffer>, PgDispatchError> {
        let org = current_org().map_err(KernelError::from)?;
        let user_uuid = *user.as_uuid();
        let rows = with_org_conn::<_, _, PgDispatchError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(
                    r#"
                    SELECT d.id AS dispatch_id, d.work_order_id, d.branch_id,
                           w.request_no, d.accept_window_started_at, d.accept_window_ends_at
                    FROM p1_dispatches d
                    JOIN p1_dispatch_targets t
                      ON t.dispatch_id = d.id
                     AND t.user_id = $1
                     AND t.target_role = 'TECHNICIAN'
                    JOIN work_orders w ON w.id = d.work_order_id
                    WHERE d.status = 'BROADCASTING'
                      AND d.accept_window_ends_at > $2
                      AND NOT EXISTS (
                          SELECT 1 FROM p1_dispatch_responses r
                          WHERE r.dispatch_id = d.id AND r.user_id = $1
                      )
                    ORDER BY d.accept_window_ends_at ASC
                    "#,
                )
                .bind(user_uuid)
                .bind(now)
                .fetch_all(tx.as_mut())
                .await?)
            })
        })
        .await?;

        rows.iter()
            .map(|row| {
                Ok(MyDispatchOffer {
                    dispatch_id: P1DispatchId::from_uuid(row.try_get("dispatch_id")?),
                    work_order_id: WorkOrderId::from_uuid(row.try_get("work_order_id")?),
                    branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
                    request_no: row.try_get("request_no")?,
                    accept_window_started_at: row.try_get("accept_window_started_at")?,
                    accept_window_ends_at: row.try_get("accept_window_ends_at")?,
                })
            })
            .collect()
    }

    /// Bounded global-order page for the unified action inbox. The predicate is
    /// the same person- and branch-scoped pending-offer predicate as
    /// [`Self::list_my_pending_offers`], with immutable keyset and `as_of`
    /// admission constraints added.
    pub async fn list_my_pending_offers_action_page(
        &self,
        user: UserId,
        branch_scope: &BranchScope,
        now: OffsetDateTime,
        as_of: OffsetDateTime,
        after: Option<(OffsetDateTime, String)>,
        limit: i64,
    ) -> Result<(Vec<ActionInboxDispatchOffer>, i64, bool), PgDispatchError> {
        let limit = limit.clamp(1, 200);
        let org = current_org().map_err(KernelError::from)?;
        let user_uuid = *user.as_uuid();
        let (all_branches, branch_ids) = match branch_scope {
            BranchScope::All => (true, Vec::new()),
            BranchScope::Branches(branches) => (
                false,
                branches
                    .iter()
                    .map(|branch| *branch.as_uuid())
                    .collect::<Vec<_>>(),
            ),
        };
        let (after_created_at, after_id) = after.map_or((None, None), |(created_at, id)| {
            (Some(created_at), Some(id))
        });
        with_org_conn::<_, _, PgDispatchError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let total: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM p1_dispatches d \
                     JOIN p1_dispatch_targets t ON t.dispatch_id = d.id \
                       AND t.user_id = $1 AND t.target_role = 'TECHNICIAN' \
                     WHERE d.status = 'BROADCASTING' AND d.accept_window_ends_at > $2 \
                       AND d.created_at <= $3 \
                       AND ($4::boolean OR d.branch_id = ANY($5::uuid[])) \
                       AND NOT EXISTS (SELECT 1 FROM p1_dispatch_responses r \
                         WHERE r.dispatch_id = d.id AND r.user_id = $1)",
                )
                .bind(user_uuid)
                .bind(now)
                .bind(as_of)
                .bind(all_branches)
                .bind(&branch_ids)
                .fetch_one(tx.as_mut())
                .await?;
                let rows = sqlx::query(
                    "SELECT d.id AS dispatch_id, d.work_order_id, \
                            w.request_no, d.created_at, d.accept_window_started_at, d.accept_window_ends_at \
                     FROM p1_dispatches d \
                     JOIN p1_dispatch_targets t ON t.dispatch_id = d.id \
                       AND t.user_id = $1 AND t.target_role = 'TECHNICIAN' \
                     JOIN work_orders w ON w.id = d.work_order_id \
                     WHERE d.status = 'BROADCASTING' AND d.accept_window_ends_at > $2 \
                       AND d.created_at <= $3 \
                       AND ($4::boolean OR d.branch_id = ANY($5::uuid[])) \
                       AND NOT EXISTS (SELECT 1 FROM p1_dispatch_responses r \
                         WHERE r.dispatch_id = d.id AND r.user_id = $1) \
                       AND ($7::text IS NULL OR d.created_at > $6 \
                         OR (d.created_at = $6 AND ('dispatch:' || d.id::text) > $7)) \
                     ORDER BY d.created_at ASC, ('dispatch:' || d.id::text) ASC \
                     LIMIT $8",
                )
                .bind(user_uuid)
                .bind(now)
                .bind(as_of)
                .bind(all_branches)
                .bind(branch_ids)
                .bind(after_created_at)
                .bind(after_id.as_deref())
                .bind(limit.clamp(1, 200) + 1)
                .fetch_all(tx.as_mut())
                .await?;
                let has_more = i64::try_from(rows.len()).unwrap_or(0) > limit;
                let items = rows
                    .iter()
                    .take(usize::try_from(limit).unwrap_or(rows.len()))
                    .map(|row| {
                        Ok(ActionInboxDispatchOffer {
                            dispatch_id: P1DispatchId::from_uuid(row.try_get("dispatch_id")?),
                            work_order_id: WorkOrderId::from_uuid(row.try_get("work_order_id")?),
                            request_no: row.try_get("request_no")?,
                            created_at: row.try_get("created_at")?,
                            accept_window_started_at: row.try_get("accept_window_started_at")?,
                            accept_window_ends_at: row.try_get("accept_window_ends_at")?,
                        })
                    })
                    .collect::<Result<Vec<_>, PgDispatchError>>()?;
                Ok((items, total, has_more))
            })
        })
        .await
    }

    /// Reclaim any alert whose SENDING lease has expired (a crashed worker) back
    /// to PENDING so it can be re-claimed (FIX 4).
    async fn reclaim_expired_leases(
        &self,
        dispatch_id: P1DispatchId,
        now: OffsetDateTime,
    ) -> Result<(), PgDispatchError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgDispatchError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
            UPDATE p1_dispatch_alerts
            SET status = 'PENDING',
                lease_token = NULL,
                lease_expires_at = NULL
            WHERE dispatch_id = $1
              AND status = 'SENDING'
              AND lease_expires_at <= $2
            "#,
                )
                .bind(*dispatch_id.as_uuid())
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                Ok(())
            })
        })
        .await
    }

    /// Atomically claim pending FCM alerts (PENDING -> SENDING + lease) that have
    /// a deliverable push token, so concurrent/retried workers cannot double-send
    /// (FIX 4). Expired leases are reclaimed first.
    pub async fn claim_fcm_pushes(
        &self,
        dispatch_id: P1DispatchId,
        alert_type: &'static str,
        now: OffsetDateTime,
    ) -> Result<Vec<PendingFcmPush>, PgDispatchError> {
        self.reclaim_expired_leases(dispatch_id, now).await?;
        let lease_expires_at = now
            .checked_add(ALERT_LEASE_TTL)
            .ok_or_else(|| KernelError::validation("alert lease expiry overflows time"))?;
        let fcm_query = sqlx::query(
            r#"
            WITH claimable AS (
                SELECT DISTINCT a.id
                FROM p1_dispatch_alerts a
                JOIN registered_devices rd ON rd.user_id = a.recipient_user_id
                WHERE a.dispatch_id = $1
                  AND a.alert_type = $2
                  AND a.status = 'PENDING'
                  AND rd.push_token IS NOT NULL
                  AND btrim(rd.push_token) <> ''
            ),
            claimed AS (
                UPDATE p1_dispatch_alerts a
                SET status = 'SENDING',
                    lease_token = gen_random_uuid(),
                    lease_expires_at = $3,
                    idempotency_key = COALESCE(
                        a.idempotency_key,
                        a.dispatch_id::text || ':' || a.id::text
                    )
                FROM claimable c
                WHERE a.id = c.id
                RETURNING a.id, a.dispatch_id, a.recipient_user_id,
                          a.lease_token, a.idempotency_key
            )
            SELECT DISTINCT ON (claimed.id)
                claimed.id AS alert_id,
                d.work_order_id,
                claimed.recipient_user_id,
                claimed.lease_token,
                claimed.idempotency_key,
                rd.push_token
            FROM claimed
            JOIN p1_dispatches d ON d.id = claimed.dispatch_id
            JOIN registered_devices rd ON rd.user_id = claimed.recipient_user_id
            WHERE rd.push_token IS NOT NULL
              AND btrim(rd.push_token) <> ''
            ORDER BY claimed.id, rd.updated_at DESC
            "#,
        )
        .bind(*dispatch_id.as_uuid())
        .bind(alert_type)
        .bind(lease_expires_at);
        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgDispatchError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(fcm_query.fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(PendingFcmPush {
                    alert_id: P1DispatchAlertId::from_uuid(row.try_get("alert_id")?),
                    dispatch_id,
                    work_order_id: WorkOrderId::from_uuid(row.try_get("work_order_id")?),
                    user_id: UserId::from_uuid(row.try_get("recipient_user_id")?),
                    push_token: row.try_get("push_token")?,
                    lease_token: row.try_get("lease_token")?,
                    idempotency_key: row.try_get("idempotency_key")?,
                })
            })
            .collect()
    }

    pub async fn claim_alimtalk_no_ack_alerts(
        &self,
        dispatch_id: P1DispatchId,
        now: OffsetDateTime,
    ) -> Result<Vec<PendingAlimtalkAlert>, PgDispatchError> {
        self.claim_alimtalk_alerts(dispatch_id, "ALIMTALK_NO_ACK", now)
            .await
    }

    pub async fn claim_manager_force_alimtalks(
        &self,
        dispatch_id: P1DispatchId,
        now: OffsetDateTime,
    ) -> Result<Vec<PendingAlimtalkAlert>, PgDispatchError> {
        self.claim_alimtalk_alerts(dispatch_id, "MANAGER_FORCE_ASSIGN", now)
            .await
    }

    /// Skip still-PENDING ALIMTALK_NO_ACK alerts directly (PENDING -> SKIPPED)
    /// without ever transitioning through SENDING. Used when Alimtalk delivery is
    /// disabled: the alerts are not deliverable, so claiming a delivery lease (and
    /// transiently entering SENDING) would be incorrect — a crash mid-window must
    /// not leave a non-deliverable alert reclaimable as deliverable. Returns the
    /// number of alerts skipped. Each skip emits one audit row.
    pub async fn skip_pending_alimtalk_no_ack_alerts(
        &self,
        dispatch_id: P1DispatchId,
        reason: &str,
        trace: TraceContext,
        occurred_at: OffsetDateTime,
    ) -> Result<u64, PgDispatchError> {
        let org = current_org().map_err(KernelError::from)?;
        let branch_uuid = with_org_conn::<_, _, PgDispatchError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query_scalar::<_, uuid::Uuid>(
                    "SELECT branch_id FROM p1_dispatches WHERE id = $1",
                )
                .bind(*dispatch_id.as_uuid())
                .fetch_one(tx.as_mut())
                .await?)
            })
        })
        .await?;
        let branch_id = BranchId::from_uuid(branch_uuid);
        let reason = reason.to_owned();
        let org = current_org().map_err(KernelError::from)?;

        with_audits::<_, u64, PgDispatchError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    r#"
                    UPDATE p1_dispatch_alerts
                    SET status = 'SKIPPED',
                        failure_reason = $3,
                        lease_token = NULL,
                        lease_expires_at = NULL
                    WHERE dispatch_id = $1
                      AND alert_type = 'ALIMTALK_NO_ACK'
                      AND status = 'PENDING'
                    RETURNING id
                    "#,
                )
                .bind(*dispatch_id.as_uuid())
                .bind(occurred_at)
                .bind(&reason)
                .fetch_all(tx.as_mut())
                .await?;

                let mut events = Vec::with_capacity(rows.len());
                for row in &rows {
                    let alert_id = P1DispatchAlertId::from_uuid(row.try_get("id")?);
                    events.push(
                        AuditEvent::new(
                            None,
                            AuditAction::new("p1_dispatch.alert_status")?,
                            "p1_dispatch_alert",
                            alert_id.to_string(),
                            trace.clone(),
                            occurred_at,
                        )
                        .with_branch(branch_id)
                        .with_org(org)
                        .with_snapshots(None, Some(serde_json::json!({ "status": "SKIPPED" }))),
                    );
                }
                let skipped = u64::try_from(rows.len())
                    .map_err(|_| KernelError::validation("skipped count overflows u64"))?;
                Ok((skipped, events))
            })
        })
        .await
    }

    /// Atomically claim pending Alimtalk alerts (PENDING -> SENDING + lease) for
    /// recipients with a phone number (FIX 4). Expired leases are reclaimed first.
    async fn claim_alimtalk_alerts(
        &self,
        dispatch_id: P1DispatchId,
        alert_type: &'static str,
        now: OffsetDateTime,
    ) -> Result<Vec<PendingAlimtalkAlert>, PgDispatchError> {
        self.reclaim_expired_leases(dispatch_id, now).await?;
        let lease_expires_at = now
            .checked_add(ALERT_LEASE_TTL)
            .ok_or_else(|| KernelError::validation("alert lease expiry overflows time"))?;
        let alimtalk_query = sqlx::query(
            r#"
            WITH claimable AS (
                SELECT a.id
                FROM p1_dispatch_alerts a
                JOIN users u ON u.id = a.recipient_user_id
                WHERE a.dispatch_id = $1
                  AND a.alert_type = $2
                  AND a.status = 'PENDING'
                  AND u.phone IS NOT NULL
                  AND btrim(u.phone) <> ''
            ),
            claimed AS (
                UPDATE p1_dispatch_alerts a
                SET status = 'SENDING',
                    lease_token = gen_random_uuid(),
                    lease_expires_at = $3,
                    idempotency_key = COALESCE(
                        a.idempotency_key,
                        a.dispatch_id::text || ':' || a.id::text
                    )
                FROM claimable c
                WHERE a.id = c.id
                RETURNING a.id, a.dispatch_id, a.recipient_user_id, a.created_at,
                          a.lease_token, a.idempotency_key
            )
            SELECT
                claimed.id AS alert_id,
                d.work_order_id,
                claimed.recipient_user_id,
                claimed.lease_token,
                claimed.idempotency_key,
                u.phone
            FROM claimed
            JOIN p1_dispatches d ON d.id = claimed.dispatch_id
            JOIN users u ON u.id = claimed.recipient_user_id
            ORDER BY claimed.created_at, claimed.id
            "#,
        )
        .bind(*dispatch_id.as_uuid())
        .bind(alert_type)
        .bind(lease_expires_at);
        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgDispatchError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(alimtalk_query.fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(PendingAlimtalkAlert {
                    alert_id: P1DispatchAlertId::from_uuid(row.try_get("alert_id")?),
                    dispatch_id,
                    work_order_id: WorkOrderId::from_uuid(row.try_get("work_order_id")?),
                    user_id: UserId::from_uuid(row.try_get("recipient_user_id")?),
                    phone: row.try_get("phone")?,
                    lease_token: row.try_get("lease_token")?,
                    idempotency_key: row.try_get("idempotency_key")?,
                })
            })
            .collect()
    }

    /// Mark a claimed alert SENT, but only while this worker still holds the
    /// lease (FIX 4). Returns `false` if the lease was lost (e.g. reclaimed after
    /// a crash) so the caller knows the row was handled elsewhere.
    pub async fn mark_alert_sent(
        &self,
        alert_id: P1DispatchAlertId,
        lease_token: uuid::Uuid,
        provider_message_id: Option<String>,
        trace: TraceContext,
        occurred_at: OffsetDateTime,
    ) -> Result<bool, PgDispatchError> {
        self.update_alert_status(AlertStatusUpdate {
            alert_id,
            lease_token: Some(lease_token),
            status: "SENT",
            provider_message_id,
            failure_reason: None,
            trace,
            occurred_at,
        })
        .await
    }

    pub async fn mark_alert_failed(
        &self,
        alert_id: P1DispatchAlertId,
        lease_token: uuid::Uuid,
        failure_reason: String,
        trace: TraceContext,
        occurred_at: OffsetDateTime,
    ) -> Result<bool, PgDispatchError> {
        self.update_alert_status(AlertStatusUpdate {
            alert_id,
            lease_token: Some(lease_token),
            status: "FAILED",
            provider_message_id: None,
            failure_reason: Some(failure_reason),
            trace,
            occurred_at,
        })
        .await
    }

    /// Mark an alert SKIPPED. Skips can originate from un-leased PENDING alerts
    /// (e.g. Alimtalk disabled) so the lease token is optional; when provided it
    /// is enforced like SENT/FAILED.
    pub async fn mark_alert_skipped(
        &self,
        alert_id: P1DispatchAlertId,
        lease_token: Option<uuid::Uuid>,
        reason: String,
        trace: TraceContext,
        occurred_at: OffsetDateTime,
    ) -> Result<bool, PgDispatchError> {
        self.update_alert_status(AlertStatusUpdate {
            alert_id,
            lease_token,
            status: "SKIPPED",
            provider_message_id: None,
            failure_reason: Some(reason),
            trace,
            occurred_at,
        })
        .await
    }

    async fn update_alert_status(
        &self,
        update: AlertStatusUpdate,
    ) -> Result<bool, PgDispatchError> {
        let AlertStatusUpdate {
            alert_id,
            lease_token,
            status,
            provider_message_id,
            failure_reason,
            trace,
            occurred_at,
        } = update;
        let org = current_org().map_err(KernelError::from)?;
        let row = with_org_conn::<_, _, PgDispatchError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(
                    r#"
            SELECT a.id, d.branch_id
            FROM p1_dispatch_alerts a
            JOIN p1_dispatches d ON d.id = a.dispatch_id
            WHERE a.id = $1
            "#,
                )
                .bind(*alert_id.as_uuid())
                .fetch_one(tx.as_mut())
                .await?)
            })
        })
        .await?;
        let branch_id = BranchId::from_uuid(row.try_get("branch_id")?);
        let alert_target = alert_id.to_string();
        let org = current_org().map_err(KernelError::from)?;

        with_audits::<_, bool, PgDispatchError>(&self.pool, org, |tx| {
            Box::pin(async move {
                // When a lease token is supplied, only the worker still holding
                // the lease may transition the alert; a reclaimed lease causes a
                // no-op so a crashed worker's late mark cannot overwrite a
                // re-delivery. The lease is cleared on the terminal transition.
                let affected = sqlx::query(
                    r#"
                    UPDATE p1_dispatch_alerts
                    SET status = $2,
                        provider_message_id = $3,
                        failure_reason = $4,
                        sent_at = CASE WHEN $2 = 'SENT' THEN $5 ELSE sent_at END,
                        lease_token = NULL,
                        lease_expires_at = NULL
                    WHERE id = $1
                      AND ($6::uuid IS NULL OR lease_token = $6)
                    "#,
                )
                .bind(*alert_id.as_uuid())
                .bind(status)
                .bind(provider_message_id)
                .bind(failure_reason)
                .bind(occurred_at)
                .bind(lease_token)
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                // Only audit a transition that actually applied — a lost lease
                // (reclaimed after a crash) is a no-op and emits no audit row.
                let events = if affected > 0 {
                    vec![
                        AuditEvent::new(
                            None,
                            AuditAction::new("p1_dispatch.alert_status")?,
                            "p1_dispatch_alert",
                            alert_target,
                            trace,
                            occurred_at,
                        )
                        .with_branch(branch_id)
                        .with_org(org)
                        .with_snapshots(None, Some(serde_json::json!({ "status": status }))),
                    ]
                } else {
                    Vec::new()
                };
                Ok((affected > 0, events))
            })
        })
        .await
    }
}

/// How long a claimed alert lease is valid before another worker may reclaim it
/// (FIX 4). A crashed worker's SENDING alert becomes reclaimable after this TTL.
pub const ALERT_LEASE_TTL: time::Duration = time::Duration::minutes(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingFcmPush {
    pub alert_id: P1DispatchAlertId,
    pub dispatch_id: P1DispatchId,
    pub work_order_id: WorkOrderId,
    pub user_id: UserId,
    pub push_token: String,
    /// Lease token held by the claiming worker; required to mark SENT/FAILED.
    pub lease_token: uuid::Uuid,
    /// Stable provider idempotency key (dispatch_id:alert_id).
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAlimtalkAlert {
    pub alert_id: P1DispatchAlertId,
    pub dispatch_id: P1DispatchId,
    pub work_order_id: WorkOrderId,
    pub user_id: UserId,
    pub phone: String,
    /// Lease token held by the claiming worker; required to mark SENT/FAILED.
    pub lease_token: uuid::Uuid,
    /// Stable provider idempotency key (dispatch_id:alert_id).
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Copy)]
struct WorkOrderHead {
    branch_id: BranchId,
    status: WorkOrderStatus,
    priority: PriorityLevel,
}

struct AlertStatusUpdate {
    alert_id: P1DispatchAlertId,
    lease_token: Option<uuid::Uuid>,
    status: &'static str,
    provider_message_id: Option<String>,
    failure_reason: Option<String>,
    trace: TraceContext,
    occurred_at: OffsetDateTime,
}

struct InsertDispatchRow {
    dispatch: P1Dispatch,
    branch_id: BranchId,
    actor: UserId,
    incident: Option<IncidentLocationInput>,
    include_region: bool,
    occurred_at: OffsetDateTime,
    org_uuid: uuid::Uuid,
}

struct ApprovalStepInsert<'a> {
    work_order_id: WorkOrderId,
    step_order: i16,
    role: ApprovalRole,
    approver_id: Option<UserId>,
    status: &'a str,
    requested_at: Option<OffsetDateTime>,
    org_uuid: uuid::Uuid,
}

fn validate_incident(input: IncidentLocationInput) -> Result<IncidentLocationInput, KernelError> {
    GeoPoint::new(input.latitude, input.longitude)?;
    Ok(input)
}

struct ExistingDispatchIntent {
    dispatch_id: P1DispatchId,
    include_region: bool,
    incident_location: Option<IncidentLocationInput>,
}

fn is_unique_work_order_dispatch_conflict(error: &PgDispatchError) -> bool {
    matches!(
        error,
        PgDispatchError::Db(DbError::Sqlx(sqlx::Error::Database(database)))
            if database.code().is_some_and(|code| code == "23505")
    )
}

async fn existing_dispatch_intent(
    pool: &PgPool,
    work_order_id: WorkOrderId,
) -> Result<Option<ExistingDispatchIntent>, PgDispatchError> {
    let org = current_org().map_err(KernelError::from)?;
    with_org_conn::<_, _, PgDispatchError>(pool, org, move |tx| {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT id, include_region, incident_latitude, incident_longitude FROM p1_dispatches WHERE work_order_id = $1",
            )
            .bind(*work_order_id.as_uuid())
            .fetch_optional(tx.as_mut())
            .await?;
            let Some(row) = row else { return Ok(None); };
            let incident_location = match (
                row.try_get::<Option<f64>, _>("incident_latitude")?,
                row.try_get::<Option<f64>, _>("incident_longitude")?,
            ) {
                (Some(latitude), Some(longitude)) => Some(IncidentLocationInput { latitude, longitude }),
                _ => None,
            };
            Ok(Some(ExistingDispatchIntent {
                dispatch_id: P1DispatchId::from_uuid(row.try_get("id")?),
                include_region: row.try_get("include_region")?,
                incident_location,
            }))
        })
    })
    .await
}

async fn target_declined(
    tx: &mut Transaction<'_, Postgres>,
    dispatch_id: P1DispatchId,
    user_id: UserId,
) -> Result<bool, PgDispatchError> {
    Ok(sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM p1_dispatch_responses WHERE dispatch_id=$1 AND user_id=$2 AND response='DECLINE')").bind(*dispatch_id.as_uuid()).bind(*user_id.as_uuid()).fetch_one(tx.as_mut()).await?)
}

async fn work_order_head(
    pool: &PgPool,
    work_order_id: WorkOrderId,
) -> Result<WorkOrderHead, PgDispatchError> {
    let org = current_org().map_err(KernelError::from)?;
    with_org_conn::<_, _, PgDispatchError>(pool, org, move |tx| {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT id, branch_id, status, priority FROM work_orders WHERE id = $1",
            )
            .bind(*work_order_id.as_uuid())
            .fetch_one(tx.as_mut())
            .await?;
            work_order_head_from_row(&row)
        })
    })
    .await
}

async fn lock_work_order(
    tx: &mut Transaction<'_, Postgres>,
    work_order_id: WorkOrderId,
) -> Result<WorkOrderHead, PgDispatchError> {
    let row = sqlx::query(
        "SELECT id, branch_id, status, priority FROM work_orders WHERE id = $1 FOR UPDATE",
    )
    .bind(*work_order_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    work_order_head_from_row(&row)
}

fn work_order_head_from_row(row: &sqlx::postgres::PgRow) -> Result<WorkOrderHead, PgDispatchError> {
    Ok(WorkOrderHead {
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        status: WorkOrderStatus::from_db_str(row.try_get::<&str, _>("status")?)?,
        priority: PriorityLevel::from_db_str(row.try_get::<&str, _>("priority")?)?,
    })
}

async fn insert_dispatch_row(
    tx: &mut Transaction<'_, Postgres>,
    row: InsertDispatchRow,
) -> Result<(), PgDispatchError> {
    let InsertDispatchRow {
        dispatch,
        branch_id,
        actor,
        incident,
        include_region,
        occurred_at,
        org_uuid,
    } = row;
    sqlx::query(
        r#"
        INSERT INTO p1_dispatches (
            id, work_order_id, branch_id, status, incident_latitude, incident_longitude,
            include_region, accept_window_started_at, accept_window_ends_at,
            created_by, created_at, updated_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $11, $12)
        "#,
    )
    .bind(*dispatch.id.as_uuid())
    .bind(*dispatch.work_order_id.as_uuid())
    .bind(*branch_id.as_uuid())
    .bind(dispatch.status.as_db_str())
    .bind(incident.map(|location| location.latitude))
    .bind(incident.map(|location| location.longitude))
    .bind(include_region)
    .bind(dispatch.accept_window_started_at)
    .bind(dispatch.accept_window_ends_at)
    .bind(*actor.as_uuid())
    .bind(occurred_at)
    .bind(org_uuid)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn count_dispatch_targets(
    pool: &PgPool,
    branch_id: BranchId,
    include_region: bool,
    on_duty_since: OffsetDateTime,
) -> Result<i64, PgDispatchError> {
    let org = current_org().map_err(KernelError::from)?;
    with_org_conn::<_, _, PgDispatchError>(pool, org, move |tx| {
        Box::pin(async move {
            let row = sqlx::query(
                r#"
        WITH selected_branches AS (
            SELECT b.id
            FROM branches b
            WHERE b.id = $1
               OR (
                    $2
                    AND b.region_id = (
                        SELECT region_id FROM branches WHERE id = $1
                    )
               )
        ),
        eligible_users AS (
            SELECT DISTINCT u.id
            FROM users u
            JOIN user_branches ub ON ub.user_id = u.id
            JOIN selected_branches sb ON sb.id = ub.branch_id
            WHERE u.is_active
              AND (
                'ADMIN' = ANY(u.roles)
                OR 'SUPER_ADMIN' = ANY(u.roles)
                OR (
                    'MECHANIC' = ANY(u.roles)
                    AND EXISTS (
                        SELECT 1
                        FROM location_pings lp
                        WHERE lp.user_id = u.id
                          AND lp.branch_id = ub.branch_id
                          AND lp.on_duty
                          AND lp.recorded_at >= $3
                    )
                )
              )
        )
        SELECT COUNT(DISTINCT u.id) AS target_count
        FROM users u
        JOIN eligible_users eu ON eu.id = u.id
        "#,
            )
            .bind(*branch_id.as_uuid())
            .bind(include_region)
            .bind(on_duty_since)
            .fetch_one(tx.as_mut())
            .await?;
            Ok(row.try_get("target_count")?)
        })
    })
    .await
}

async fn insert_dispatch_targets(
    tx: &mut Transaction<'_, Postgres>,
    dispatch_id: P1DispatchId,
    branch_id: BranchId,
    include_region: bool,
    on_duty_since: OffsetDateTime,
    occurred_at: OffsetDateTime,
    org_uuid: uuid::Uuid,
) -> Result<(), PgDispatchError> {
    sqlx::query(
        r#"
        WITH selected_branches AS (
            SELECT b.id
            FROM branches b
            WHERE b.id = $2
               OR (
                    $3
                    AND b.region_id = (
                        SELECT region_id FROM branches WHERE id = $2
                    )
               )
        ),
        target_users AS (
            SELECT DISTINCT
                u.id AS user_id,
                CASE
                    WHEN 'MECHANIC' = ANY(u.roles) THEN 'TECHNICIAN'
                    ELSE 'MANAGER'
                END AS target_role,
                COUNT(rd.id) FILTER (
                    WHERE rd.push_token IS NOT NULL AND btrim(rd.push_token) <> ''
                ) AS push_token_count
            FROM users u
            JOIN user_branches ub ON ub.user_id = u.id
            JOIN selected_branches sb ON sb.id = ub.branch_id
            LEFT JOIN registered_devices rd ON rd.user_id = u.id
            WHERE u.is_active
              AND (
                'ADMIN' = ANY(u.roles)
                OR 'SUPER_ADMIN' = ANY(u.roles)
                OR (
                    'MECHANIC' = ANY(u.roles)
                    AND EXISTS (
                        SELECT 1
                        FROM location_pings lp
                        WHERE lp.user_id = u.id
                          AND lp.branch_id = ub.branch_id
                          AND lp.on_duty
                          AND lp.recorded_at >= $4
                    )
                )
              )
            GROUP BY u.id, target_role
        )
        INSERT INTO p1_dispatch_targets (
            dispatch_id, user_id, target_role, push_token_count, fanout_created_at, org_id
        )
        SELECT $1, user_id, target_role, push_token_count, $5, $6
        FROM target_users
        "#,
    )
    .bind(*dispatch_id.as_uuid())
    .bind(*branch_id.as_uuid())
    .bind(include_region)
    .bind(on_duty_since)
    .bind(occurred_at)
    .bind(org_uuid)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn insert_push_alerts_for_targets(
    tx: &mut Transaction<'_, Postgres>,
    dispatch_id: P1DispatchId,
    occurred_at: OffsetDateTime,
) -> Result<(), PgDispatchError> {
    sqlx::query(
        r#"
        INSERT INTO p1_dispatch_alerts (
            dispatch_id, recipient_user_id, alert_type, status, created_at, org_id
        )
        SELECT dispatch_id, user_id, 'FCM_PUSH',
               CASE WHEN push_token_count > 0 THEN 'PENDING' ELSE 'SKIPPED' END,
               $2,
               org_id
        FROM p1_dispatch_targets
        WHERE dispatch_id = $1
        "#,
    )
    .bind(*dispatch_id.as_uuid())
    .bind(occurred_at)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn lock_dispatch(
    tx: &mut Transaction<'_, Postgres>,
    dispatch_id: P1DispatchId,
) -> Result<P1Dispatch, PgDispatchError> {
    let row = sqlx::query(
        r#"
        SELECT id, work_order_id, status, accept_window_started_at, accept_window_ends_at
        FROM p1_dispatches
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(*dispatch_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    Ok(P1Dispatch {
        id: P1DispatchId::from_uuid(row.try_get("id")?),
        work_order_id: WorkOrderId::from_uuid(row.try_get("work_order_id")?),
        status: DispatchStatus::from_db_str(row.try_get::<&str, _>("status")?)?,
        accept_window_started_at: row.try_get("accept_window_started_at")?,
        accept_window_ends_at: row.try_get("accept_window_ends_at")?,
    })
}

async fn ensure_technician_target(
    tx: &mut Transaction<'_, Postgres>,
    dispatch_id: P1DispatchId,
    actor: UserId,
) -> Result<(), PgDispatchError> {
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM p1_dispatch_targets
        WHERE dispatch_id = $1
          AND user_id = $2
          AND target_role = 'TECHNICIAN'
        "#,
    )
    .bind(*dispatch_id.as_uuid())
    .bind(*actor.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    if count == 0 {
        Err(KernelError::forbidden("dispatch response requires technician target").into())
    } else {
        Ok(())
    }
}

async fn response_for_target(
    tx: &mut Transaction<'_, Postgres>,
    dispatch_id: P1DispatchId,
    user_id: UserId,
) -> Result<Option<DispatchResponseKind>, PgDispatchError> {
    let response = sqlx::query_scalar::<_, String>(
        "SELECT response FROM p1_dispatch_responses WHERE dispatch_id = $1 AND user_id = $2",
    )
    .bind(*dispatch_id.as_uuid())
    .bind(*user_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?;
    response
        .as_deref()
        .map(DispatchResponseKind::from_db_str)
        .transpose()
        .map_err(Into::into)
}

async fn insert_response(
    tx: &mut Transaction<'_, Postgres>,
    dispatch_id: P1DispatchId,
    user_id: UserId,
    response: DispatchResponseKind,
    occurred_at: OffsetDateTime,
    org_uuid: uuid::Uuid,
) -> Result<(), PgDispatchError> {
    let result = sqlx::query(
        r#"
        INSERT INTO p1_dispatch_responses (
            dispatch_id, user_id, response, responded_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (dispatch_id, user_id) DO NOTHING
        "#,
    )
    .bind(*dispatch_id.as_uuid())
    .bind(*user_id.as_uuid())
    .bind(response.as_db_str())
    .bind(occurred_at)
    .bind(org_uuid)
    .execute(tx.as_mut())
    .await?;
    if result.rows_affected() == 0 {
        Err(KernelError::conflict("dispatch target already responded").into())
    } else {
        Ok(())
    }
}

async fn accepted_count_tx(
    tx: &mut Transaction<'_, Postgres>,
    dispatch_id: P1DispatchId,
) -> Result<i64, PgDispatchError> {
    let count = sqlx::query_scalar(
        "SELECT COUNT(*) FROM p1_dispatch_responses WHERE dispatch_id = $1 AND response = 'ACCEPT'",
    )
    .bind(*dispatch_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    Ok(count)
}

async fn auto_assign_tx(
    tx: &mut Transaction<'_, Postgres>,
    dispatch: &mut P1Dispatch,
    actor: UserId,
    trace: TraceContext,
    occurred_at: OffsetDateTime,
    timers: DispatchTimerConfig,
    org_uuid: uuid::Uuid,
) -> Result<CandidateScore, PgDispatchError> {
    let mut candidates = scored_candidates(tx, dispatch.id, occurred_at, timers).await?;
    candidates.sort_by_key(|score| (score.score_milli, *score.mechanic_id.as_uuid()));
    let winner = candidates
        .first()
        .copied()
        .ok_or_else(|| KernelError::conflict("no accepted dispatch candidates"))?;
    dispatch.resolve_with_accepts(candidates.len())?;
    assign_work_order_tx(
        tx,
        dispatch.work_order_id,
        winner.mechanic_id,
        actor,
        trace.clone(),
        occurred_at,
        org_uuid,
    )
    .await?;
    sqlx::query(
        r#"
        UPDATE p1_dispatches
        SET status = 'AUTO_ASSIGNED',
            auto_assigned_mechanic_id = $2,
            updated_at = $3
        WHERE id = $1
        "#,
    )
    .bind(*dispatch.id.as_uuid())
    .bind(*winner.mechanic_id.as_uuid())
    .bind(occurred_at)
    .execute(tx.as_mut())
    .await?;

    let event = dispatch_audit_event(
        "p1_dispatch.auto_assign",
        Some(actor),
        branch_for_work_order_tx(tx, dispatch.work_order_id).await?,
        dispatch.id,
        trace,
        occurred_at,
    )?
    .with_snapshots(
        None,
        Some(resolution_after_snapshot(
            DispatchStatus::AutoAssigned,
            i64::try_from(candidates.len())
                .map_err(|_| KernelError::validation("accepted count overflows i64"))?,
            Some(winner.mechanic_id),
        )),
    )
    .with_org(OrgId::from_uuid(org_uuid));
    insert_audit_event(tx, &event).await?;
    Ok(winner)
}

async fn scored_candidates(
    tx: &mut Transaction<'_, Postgres>,
    dispatch_id: P1DispatchId,
    occurred_at: OffsetDateTime,
    timers: DispatchTimerConfig,
) -> Result<Vec<CandidateScore>, PgDispatchError> {
    let dispatch_row = sqlx::query(
        "SELECT incident_latitude, incident_longitude FROM p1_dispatches WHERE id = $1",
    )
    .bind(*dispatch_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    let incident = match (
        dispatch_row.try_get::<Option<f64>, _>("incident_latitude")?,
        dispatch_row.try_get::<Option<f64>, _>("incident_longitude")?,
    ) {
        (Some(latitude), Some(longitude)) => Some(GeoPoint::new(latitude, longitude)?),
        _ => None,
    };
    let rows = sqlx::query(
        r#"
        SELECT
            r.user_id,
            r.responded_at,
            lc.status = 'GRANTED' AS consent_granted,
            lp.latitude,
            lp.longitude,
            lp.recorded_at,
            COALESCE(load.p1_count, 0) AS p1_count,
            COALESCE(load.p2_count, 0) AS p2_count,
            COALESCE(load.p3_count, 0) AS p3_count,
            COALESCE(load.other_count, 0) AS other_count
        FROM p1_dispatch_responses r
        JOIN p1_dispatches d ON d.id = r.dispatch_id
        -- Consent is per-user (location_consents UNIQUE (user_id)); join on
        -- user_id only so a multi-branch responder who consented in another
        -- branch is still GPS-ranked rather than silently demoted to fallback.
        LEFT JOIN location_consents lc
            ON lc.user_id = r.user_id
        LEFT JOIN LATERAL (
            SELECT latitude, longitude, recorded_at
            FROM location_pings
            WHERE user_id = r.user_id
            ORDER BY recorded_at DESC
            LIMIT 1
        ) lp ON TRUE
        LEFT JOIN LATERAL (
            SELECT
                COUNT(*) FILTER (WHERE w.priority = 'P1') AS p1_count,
                COUNT(*) FILTER (WHERE w.priority = 'P2') AS p2_count,
                COUNT(*) FILTER (WHERE w.priority = 'P3') AS p3_count,
                COUNT(*) FILTER (WHERE w.priority NOT IN ('P1','P2','P3')) AS other_count
            FROM work_order_assignments a
            JOIN work_orders w ON w.id = a.work_order_id
            WHERE a.mechanic_id = r.user_id
              AND w.status NOT IN ('FINAL_COMPLETED','CANCELLED','ARCHIVED')
        ) load ON TRUE
        WHERE r.dispatch_id = $1
          AND r.response = 'ACCEPT'
        "#,
    )
    .bind(*dispatch_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await?;

    let mut scores = Vec::with_capacity(rows.len());
    for row in rows {
        let latest = match (
            row.try_get::<Option<f64>, _>("latitude")?,
            row.try_get::<Option<f64>, _>("longitude")?,
        ) {
            (Some(latitude), Some(longitude)) => Some(GeoPoint::new(latitude, longitude)?),
            _ => None,
        };
        let load = TechnicianLoad {
            p1: row.try_get("p1_count")?,
            p2: row.try_get("p2_count")?,
            p3: row.try_get("p3_count")?,
            other: row.try_get("other_count")?,
        };
        let score = score_candidate(
            DispatchCandidate {
                mechanic_id: UserId::from_uuid(row.try_get("user_id")?),
                latest_location: latest,
                incident_location: incident,
                location_recorded_at: row.try_get("recorded_at")?,
                location_consent_granted: row
                    .try_get::<Option<bool>, _>("consent_granted")?
                    .unwrap_or(false),
                workload: load,
            },
            occurred_at,
            timers.gps_ping_freshness,
        );
        sqlx::query(
            r#"
            UPDATE p1_dispatch_responses
            SET score_milli = $3,
                gps_ranked = $4,
                distance_meters = $5,
                workload_weight = $6,
                score_reason = $7
            WHERE dispatch_id = $1 AND user_id = $2
            "#,
        )
        .bind(*dispatch_id.as_uuid())
        .bind(*score.mechanic_id.as_uuid())
        .bind(score.score_milli)
        .bind(score.gps_ranked)
        .bind(score.distance_meters)
        .bind(score.workload_weight)
        .bind(score.reason())
        .execute(tx.as_mut())
        .await?;
        scores.push(score);
    }
    Ok(scores)
}

async fn update_score_columns(
    tx: &mut Transaction<'_, Postgres>,
    dispatch_id: P1DispatchId,
    score: CandidateScore,
) -> Result<(), PgDispatchError> {
    sqlx::query(
        r#"
        UPDATE p1_dispatch_responses
        SET score_milli = $3,
            gps_ranked = $4,
            distance_meters = $5,
            workload_weight = $6,
            score_reason = $7
        WHERE dispatch_id = $1 AND user_id = $2
        "#,
    )
    .bind(*dispatch_id.as_uuid())
    .bind(*score.mechanic_id.as_uuid())
    .bind(score.score_milli)
    .bind(score.gps_ranked)
    .bind(score.distance_meters)
    .bind(score.workload_weight)
    .bind(score.reason())
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn assign_work_order_tx(
    tx: &mut Transaction<'_, Postgres>,
    work_order_id: WorkOrderId,
    mechanic_id: UserId,
    actor: UserId,
    trace: TraceContext,
    occurred_at: OffsetDateTime,
    org_uuid: uuid::Uuid,
) -> Result<(), PgDispatchError> {
    let row = lock_work_order(tx, work_order_id).await?;
    validate_status_transition(
        row.status,
        WorkOrderStatus::Assigned,
        TransitionGuardContext::admin(),
    )?;
    sqlx::query("DELETE FROM work_order_assignments WHERE work_order_id = $1")
        .bind(*work_order_id.as_uuid())
        .execute(tx.as_mut())
        .await?;
    sqlx::query("DELETE FROM work_order_approval_steps WHERE work_order_id = $1")
        .bind(*work_order_id.as_uuid())
        .execute(tx.as_mut())
        .await?;
    sqlx::query(
        r#"
        INSERT INTO work_order_assignments (
            work_order_id, mechanic_id, role, assigned_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*mechanic_id.as_uuid())
    .bind(AssignmentRole::Primary.as_db_str())
    .bind(occurred_at)
    .bind(org_uuid)
    .execute(tx.as_mut())
    .await?;
    insert_approval_step(
        tx,
        ApprovalStepInsert {
            work_order_id,
            step_order: 1,
            role: ApprovalRole::Mechanic,
            approver_id: Some(mechanic_id),
            status: "PENDING",
            requested_at: Some(occurred_at),
            org_uuid,
        },
    )
    .await?;
    insert_approval_step(
        tx,
        ApprovalStepInsert {
            work_order_id,
            step_order: 2,
            role: ApprovalRole::Admin,
            approver_id: None,
            status: "NOT_STARTED",
            requested_at: None,
            org_uuid,
        },
    )
    .await?;
    insert_approval_step(
        tx,
        ApprovalStepInsert {
            work_order_id,
            step_order: 3,
            role: ApprovalRole::Executive,
            approver_id: None,
            status: "NOT_STARTED",
            requested_at: None,
            org_uuid,
        },
    )
    .await?;
    sqlx::query("UPDATE work_orders SET status = 'ASSIGNED', updated_at = $2 WHERE id = $1")
        .bind(*work_order_id.as_uuid())
        .bind(occurred_at)
        .execute(tx.as_mut())
        .await?;
    sqlx::query(
        r#"
        INSERT INTO work_order_status_history (
            work_order_id, actor, action, from_status, to_status, occurred_at, org_id
        )
        VALUES ($1, $2, 'work_order.assign', $3, 'ASSIGNED', $4, $5)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*actor.as_uuid())
    .bind(row.status.as_db_str())
    .bind(occurred_at)
    .bind(org_uuid)
    .execute(tx.as_mut())
    .await?;
    let event = work_order_audit_event(
        "work_order.assign",
        actor,
        OrgId::from_uuid(org_uuid),
        row.branch_id,
        work_order_id,
        trace,
        occurred_at,
    )?;
    insert_audit_event(tx, &event).await?;
    Ok(())
}

async fn insert_approval_step(
    tx: &mut Transaction<'_, Postgres>,
    step: ApprovalStepInsert<'_>,
) -> Result<(), PgDispatchError> {
    let ApprovalStepInsert {
        work_order_id,
        step_order,
        role,
        approver_id,
        status,
        requested_at,
        org_uuid,
    } = step;
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
    .bind(approver_id.map(|id| *id.as_uuid()))
    .bind(status)
    .bind(requested_at)
    .bind(org_uuid)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn branch_for_work_order_tx(
    tx: &mut Transaction<'_, Postgres>,
    work_order_id: WorkOrderId,
) -> Result<BranchId, PgDispatchError> {
    let branch_id = sqlx::query_scalar("SELECT branch_id FROM work_orders WHERE id = $1")
        .bind(*work_order_id.as_uuid())
        .fetch_one(tx.as_mut())
        .await?;
    Ok(BranchId::from_uuid(branch_id))
}

async fn insert_manager_force_alerts(
    tx: &mut Transaction<'_, Postgres>,
    dispatch_id: P1DispatchId,
    occurred_at: OffsetDateTime,
) -> Result<(), PgDispatchError> {
    sqlx::query(
        r#"
        INSERT INTO p1_dispatch_alerts (
            dispatch_id, recipient_user_id, alert_type, status, created_at, org_id
        )
        SELECT t.dispatch_id,
               t.user_id,
               'MANAGER_FORCE_ASSIGN',
               CASE
                   WHEN t.push_token_count > 0
                        OR (u.phone IS NOT NULL AND btrim(u.phone) <> '')
                   THEN 'PENDING'
                   ELSE 'SKIPPED'
               END,
               $2,
               t.org_id
        FROM p1_dispatch_targets t
        JOIN users u ON u.id = t.user_id
        WHERE t.dispatch_id = $1
          AND t.target_role = 'MANAGER'
          AND NOT EXISTS (
              SELECT 1
              FROM p1_dispatch_alerts existing
              WHERE existing.dispatch_id = t.dispatch_id
                AND existing.recipient_user_id = t.user_id
                AND existing.alert_type = 'MANAGER_FORCE_ASSIGN'
          )
        "#,
    )
    .bind(*dispatch_id.as_uuid())
    .bind(occurred_at)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn fetch_dispatch_summary(
    pool: &PgPool,
    dispatch_id: P1DispatchId,
) -> Result<P1DispatchSummary, PgDispatchError> {
    let org = current_org().map_err(KernelError::from)?;
    with_org_conn::<_, _, PgDispatchError>(pool, org, move |tx| {
        Box::pin(async move { fetch_dispatch_summary_tx(tx, dispatch_id).await })
    })
    .await
}

async fn fetch_dispatch_summary_tx(
    tx: &mut Transaction<'_, Postgres>,
    dispatch_id: P1DispatchId,
) -> Result<P1DispatchSummary, PgDispatchError> {
    let row = sqlx::query(
        r#"
        SELECT
            d.id, d.work_order_id, d.branch_id, d.status,
            d.incident_latitude, d.incident_longitude,
            d.accept_window_started_at, d.accept_window_ends_at,
            d.auto_assigned_mechanic_id, d.manager_force_pending_at,
            d.manual_call_required_at, d.manual_call_cleared_at,
            COUNT(DISTINCT t.id) AS target_count,
            COUNT(DISTINCT r.id) FILTER (WHERE r.response = 'ACCEPT') AS accepted_count,
            COUNT(DISTINCT r.id) FILTER (WHERE r.response = 'DECLINE') AS declined_count
        FROM p1_dispatches d
        LEFT JOIN p1_dispatch_targets t ON t.dispatch_id = d.id
        LEFT JOIN p1_dispatch_responses r ON r.dispatch_id = d.id
        WHERE d.id = $1
        GROUP BY d.id
        "#,
    )
    .bind(*dispatch_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    let incident_location = match (
        row.try_get::<Option<f64>, _>("incident_latitude")?,
        row.try_get::<Option<f64>, _>("incident_longitude")?,
    ) {
        (Some(latitude), Some(longitude)) => Some(IncidentLocationInput {
            latitude,
            longitude,
        }),
        _ => None,
    };
    Ok(P1DispatchSummary {
        id: P1DispatchId::from_uuid(row.try_get("id")?),
        work_order_id: WorkOrderId::from_uuid(row.try_get("work_order_id")?),
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        status: DispatchStatus::from_db_str(row.try_get::<&str, _>("status")?)?,
        incident_location,
        accept_window_started_at: row.try_get("accept_window_started_at")?,
        accept_window_ends_at: row.try_get("accept_window_ends_at")?,
        auto_assigned_mechanic_id: row
            .try_get::<Option<uuid::Uuid>, _>("auto_assigned_mechanic_id")?
            .map(UserId::from_uuid),
        manager_force_pending_at: row.try_get("manager_force_pending_at")?,
        manual_call_required: row
            .try_get::<Option<OffsetDateTime>, _>("manual_call_required_at")?
            .is_some()
            && row
                .try_get::<Option<OffsetDateTime>, _>("manual_call_cleared_at")?
                .is_none(),
        manual_call_required_at: row.try_get("manual_call_required_at")?,
        manual_call_cleared_at: row.try_get("manual_call_cleared_at")?,
        target_count: row.try_get("target_count")?,
        accepted_count: row.try_get("accepted_count")?,
        declined_count: row.try_get("declined_count")?,
    })
}

pub async fn dispatch_response(
    pool: &PgPool,
    dispatch_id: P1DispatchId,
    user_id: UserId,
) -> Result<P1DispatchResponseSummary, PgDispatchError> {
    let org = current_org().map_err(KernelError::from)?;
    with_org_conn::<_, _, PgDispatchError>(pool, org, move |tx| {
        Box::pin(async move {
            let row = sqlx::query(
                r#"
        SELECT dispatch_id, user_id, response, responded_at, score_milli,
               gps_ranked, distance_meters, score_reason
        FROM p1_dispatch_responses
        WHERE dispatch_id = $1 AND user_id = $2
        "#,
            )
            .bind(*dispatch_id.as_uuid())
            .bind(*user_id.as_uuid())
            .fetch_one(tx.as_mut())
            .await?;
            Ok(P1DispatchResponseSummary {
                dispatch_id: P1DispatchId::from_uuid(row.try_get("dispatch_id")?),
                user_id: UserId::from_uuid(row.try_get("user_id")?),
                response: DispatchResponseKind::from_db_str(row.try_get::<&str, _>("response")?)?,
                responded_at: row.try_get("responded_at")?,
                score_milli: row.try_get("score_milli")?,
                gps_ranked: row.try_get("gps_ranked")?,
                distance_meters: row.try_get("distance_meters")?,
                score_reason: row.try_get("score_reason")?,
            })
        })
    })
    .await
}
