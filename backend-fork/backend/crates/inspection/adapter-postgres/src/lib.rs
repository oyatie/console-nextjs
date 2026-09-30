//! Postgres inspection adapter.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_inspection_application::{
    CompleteInspectionRoundCommand, CreateInspectionScheduleCommand, InspectionRoundSummary,
    InspectionSchedulePage, InspectionScheduleSummary, ListInspectionSchedulesQuery,
    ListMyInspectionSchedulesQuery, inspection_audit_event,
};
use console_inspection_domain::{
    InspectionRoundOutcome, InspectionScheduleStatus, validate_interval_days,
};
use console_kernel_core::{
    BranchId, BranchScope, EquipmentId, ErrorKind, InspectionRoundId, InspectionScheduleId,
    KernelError, UserId,
};
use console_platform_db::{DbError, with_audit, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};

/// Hard server-side cap on a single schedule page, so a wide date range can
/// never trigger an unbounded fetch even if the client over-asks.
const MAX_SCHEDULE_LIMIT: i64 = 500;

#[derive(Debug, thiserror::Error)]
pub enum PgInspectionError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl PgInspectionError {
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

impl From<sqlx::Error> for PgInspectionError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

#[derive(Debug, Clone)]
pub struct PgInspectionStore {
    pool: PgPool,
}

impl PgInspectionStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn create_schedule(
        &self,
        command: CreateInspectionScheduleCommand,
    ) -> Result<InspectionScheduleSummary, PgInspectionError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        validate_interval_days(command.interval_days)?;
        let schedule_id = InspectionScheduleId::new();
        let event = inspection_audit_event(
            "inspection.schedule.create",
            command.actor,
            command.branch_id,
            "regular_inspection_schedule",
            schedule_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        with_audit::<_, InspectionScheduleSummary, PgInspectionError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let equipment_branch = equipment_branch_tx(tx, command.equipment_id).await?;
                ensure_branch(equipment_branch, command.branch_id)?;
                ensure_prevention_mechanic_tx(tx, command.mechanic_id, command.branch_id).await?;
                let note = command
                    .note
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty());

                sqlx::query(
                    r#"
                    INSERT INTO regular_inspection_schedules (
                        id, branch_id, equipment_id, mechanic_id, cycle, interval_days,
                        due_date, status, note, created_by, created_at, updated_at, org_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, 'SCHEDULED', $8, $9, $10, $10, $11)
                    "#,
                )
                .bind(*schedule_id.as_uuid())
                .bind(*command.branch_id.as_uuid())
                .bind(*command.equipment_id.as_uuid())
                .bind(*command.mechanic_id.as_uuid())
                .bind(command.cycle.as_db_str())
                .bind(command.interval_days)
                .bind(command.due_date)
                .bind(note)
                .bind(*command.actor.as_uuid())
                .bind(command.occurred_at)
                .bind(org_uuid)
                .execute(tx.as_mut())
                .await?;

                fetch_schedule_summary_tx(tx, schedule_id).await
            })
        })
        .await
    }

    pub async fn complete_round(
        &self,
        command: CompleteInspectionRoundCommand,
    ) -> Result<InspectionRoundSummary, PgInspectionError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let round_id = InspectionRoundId::new();

        with_audits::<_, InspectionRoundSummary, PgInspectionError>(&self.pool, org, |tx| {
            Box::pin(async move {
                if command.findings.trim().is_empty() {
                    return Err(KernelError::validation("inspection findings are required").into());
                }
                let schedule = lock_schedule_tx(tx, command.schedule_id).await?;
                if schedule.status == InspectionScheduleStatus::Cancelled {
                    return Err(KernelError::conflict(
                        "cancelled inspection schedule cannot be completed",
                    )
                    .into());
                }
                if schedule.status == InspectionScheduleStatus::Completed {
                    return Err(
                        KernelError::conflict("inspection schedule is already completed").into(),
                    );
                }
                if schedule.mechanic_id != command.actor {
                    return Err(KernelError::forbidden(
                        "inspection round must be completed by the assigned mechanic",
                    )
                    .into());
                }
                ensure_prevention_mechanic_tx(tx, command.actor, schedule.branch_id).await?;
                let note = command
                    .note
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty());

                sqlx::query(
                    r#"
                    INSERT INTO inspection_rounds (
                        id, schedule_id, branch_id, equipment_id, mechanic_id, completed_by,
                        outcome, findings, note, completed_at, created_at, org_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $5, $6, $7, $8, $9, $10, $11)
                    "#,
                )
                .bind(*round_id.as_uuid())
                .bind(*schedule.id.as_uuid())
                .bind(*schedule.branch_id.as_uuid())
                .bind(*schedule.equipment_id.as_uuid())
                .bind(*schedule.mechanic_id.as_uuid())
                .bind(command.outcome.as_db_str())
                .bind(command.findings.trim())
                .bind(note)
                .bind(command.completed_at)
                .bind(command.occurred_at)
                .bind(org_uuid)
                .execute(tx.as_mut())
                .await?;

                sqlx::query(
                    r#"
                    UPDATE regular_inspection_schedules
                    SET status = 'COMPLETED',
                        completed_at = $2,
                        completed_by = $3,
                        updated_at = $4
                    WHERE id = $1
                    "#,
                )
                .bind(*schedule.id.as_uuid())
                .bind(command.completed_at)
                .bind(*command.actor.as_uuid())
                .bind(command.occurred_at)
                .execute(tx.as_mut())
                .await?;

                let round = fetch_round_summary_tx(tx, round_id).await?;
                let round_event = inspection_audit_event(
                    "inspection.round.complete",
                    command.actor,
                    schedule.branch_id,
                    "inspection_round",
                    round_id,
                    command.trace.clone(),
                    command.occurred_at,
                )?
                .with_snapshots(
                    None,
                    Some(serde_json::json!({
                        "schedule_id": schedule.id.to_string(),
                        "outcome": command.outcome.as_db_str(),
                        "completed_at": command.completed_at.to_string(),
                    })),
                )
                .with_org(org);
                let schedule_event = inspection_audit_event(
                    "inspection.schedule.complete",
                    command.actor,
                    schedule.branch_id,
                    "regular_inspection_schedule",
                    schedule.id,
                    command.trace,
                    command.occurred_at,
                )?
                .with_snapshots(
                    Some(schedule.audit_snapshot()),
                    Some(serde_json::json!({
                        "status": InspectionScheduleStatus::Completed.as_db_str(),
                        "completed_at": command.completed_at.to_string(),
                        "completed_by": command.actor.to_string(),
                    })),
                )
                .with_org(org);
                Ok((round, vec![round_event, schedule_event]))
            })
        })
        .await
    }

    pub async fn list_due_schedules(
        &self,
        query: ListInspectionSchedulesQuery,
    ) -> Result<InspectionSchedulePage, PgInspectionError> {
        self.list_due_schedules_inner(
            query.branch_scope,
            None,
            query.due_start,
            query.due_end,
            query.limit,
            query.offset,
        )
        .await
    }

    pub async fn list_my_due_schedules(
        &self,
        query: ListMyInspectionSchedulesQuery,
    ) -> Result<InspectionSchedulePage, PgInspectionError> {
        self.list_due_schedules_inner(
            query.branch_scope,
            Some(query.mechanic_id),
            query.due_start,
            query.due_end,
            query.limit,
            query.offset,
        )
        .await
    }

    async fn list_due_schedules_inner(
        &self,
        branch_scope: BranchScope,
        mechanic_id: Option<UserId>,
        due_start: time::Date,
        due_end: time::Date,
        requested_limit: i64,
        requested_offset: i64,
    ) -> Result<InspectionSchedulePage, PgInspectionError> {
        if due_start >= due_end {
            return Err(
                KernelError::validation("inspection due_start must be before due_end").into(),
            );
        }
        let limit = requested_limit.clamp(1, MAX_SCHEDULE_LIMIT);
        let offset = requested_offset.max(0);

        // Unpaged total for the same date range + branch scope, so the console
        // can show an honest count and page beyond the cap.
        let mut count_builder = QueryBuilder::<Postgres>::new(
            "SELECT COUNT(*) FROM regular_inspection_schedules s WHERE s.due_date >= ",
        );
        count_builder.push_bind(due_start);
        count_builder.push(" AND s.due_date < ");
        count_builder.push_bind(due_end);
        count_builder.push(" AND s.status <> 'CANCELLED' AND ");
        push_branch_column_filter(&mut count_builder, &branch_scope, "s.branch_id");
        if let Some(mechanic_id) = mechanic_id.as_ref() {
            count_builder.push(" AND s.mechanic_id = ");
            count_builder.push_bind(*mechanic_id.as_uuid());
        }

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT
                s.id, s.branch_id, s.equipment_id, s.mechanic_id, s.cycle,
                s.interval_days, s.due_date, s.status, s.completed_at, s.note,
                site.name AS site_name, e.management_no, e.model,
                mech.display_name AS mechanic_display_name,
                s.created_at, s.updated_at
            FROM regular_inspection_schedules s
            JOIN registry_equipment e ON e.id = s.equipment_id
            JOIN registry_sites site ON site.id = e.site_id
            -- Same-org LEFT JOIN: `users` is RLS-scoped to app.current_org like
            -- the schedules, so a mechanic only resolves within the caller's
            -- tenant; a hard-deleted mechanic simply yields NULL.
            LEFT JOIN users mech ON mech.id = s.mechanic_id
            WHERE s.due_date >=
            "#,
        );
        builder.push_bind(due_start);
        builder.push(" AND s.due_date < ");
        builder.push_bind(due_end);
        builder.push(" AND s.status <> 'CANCELLED' AND ");
        push_branch_column_filter(&mut builder, &branch_scope, "s.branch_id");
        if let Some(mechanic_id) = mechanic_id.as_ref() {
            builder.push(" AND s.mechanic_id = ");
            builder.push_bind(*mechanic_id.as_uuid());
        }
        builder.push(" ORDER BY s.due_date, site.name, e.management_no, s.id LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let org = current_org().map_err(KernelError::from)?;
        let (total, rows) = with_org_conn::<_, _, PgInspectionError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let total: i64 = count_builder
                    .build_query_scalar()
                    .fetch_one(tx.as_mut())
                    .await?;
                let rows = builder.build().fetch_all(tx.as_mut()).await?;
                Ok((total, rows))
            })
        })
        .await?;
        let items = rows
            .iter()
            .map(schedule_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(InspectionSchedulePage {
            items,
            limit,
            offset,
            total,
        })
    }

    pub async fn schedule_branch(
        &self,
        schedule_id: InspectionScheduleId,
    ) -> Result<BranchId, PgInspectionError> {
        let schedule_uuid = *schedule_id.as_uuid();
        let org = current_org().map_err(KernelError::from)?;
        let branch_id: uuid::Uuid =
            with_org_conn::<_, _, PgInspectionError>(&self.pool, org, move |tx| {
                Box::pin(async move {
                    Ok(sqlx::query_scalar(
                        "SELECT branch_id FROM regular_inspection_schedules WHERE id = $1",
                    )
                    .bind(schedule_uuid)
                    .fetch_one(tx.as_mut())
                    .await?)
                })
            })
            .await?;
        Ok(BranchId::from_uuid(branch_id))
    }

    pub async fn schedule_branch_in_scope(
        &self,
        schedule_id: InspectionScheduleId,
        branch_scope: &BranchScope,
    ) -> Result<BranchId, PgInspectionError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT branch_id FROM regular_inspection_schedules WHERE id = ",
        );
        builder.push_bind(*schedule_id.as_uuid());
        builder.push(" AND ");
        push_branch_column_filter(&mut builder, branch_scope, "branch_id");

        let org = current_org().map_err(KernelError::from)?;
        let branch_id = with_org_conn::<_, _, PgInspectionError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(builder
                    .build_query_scalar::<uuid::Uuid>()
                    .fetch_optional(tx.as_mut())
                    .await?)
            })
        })
        .await?
        .ok_or_else(|| KernelError::not_found("inspection schedule was not found"))?;
        Ok(BranchId::from_uuid(branch_id))
    }
}

#[derive(Debug)]
struct LockedSchedule {
    id: InspectionScheduleId,
    branch_id: BranchId,
    equipment_id: EquipmentId,
    mechanic_id: UserId,
    status: InspectionScheduleStatus,
    completed_at: Option<time::OffsetDateTime>,
    completed_by: Option<UserId>,
}

impl LockedSchedule {
    fn audit_snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "status": self.status.as_db_str(),
            "completed_at": self.completed_at.map(|value| value.to_string()),
            "completed_by": self.completed_by.map(|value| value.to_string()),
        })
    }
}

async fn equipment_branch_tx(
    tx: &mut Transaction<'_, Postgres>,
    equipment_id: EquipmentId,
) -> Result<BranchId, PgInspectionError> {
    let branch_id: uuid::Uuid =
        sqlx::query_scalar("SELECT branch_id FROM registry_equipment WHERE id = $1")
            .bind(*equipment_id.as_uuid())
            .fetch_one(tx.as_mut())
            .await?;
    Ok(BranchId::from_uuid(branch_id))
}

async fn ensure_prevention_mechanic_tx(
    tx: &mut Transaction<'_, Postgres>,
    mechanic_id: UserId,
    branch_id: BranchId,
) -> Result<(), PgInspectionError> {
    let valid: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM users u
            JOIN user_branches ub ON ub.user_id = u.id
            WHERE u.id = $1
              AND ub.branch_id = $2
              AND u.is_active = TRUE
              AND u.team = '예방'
              AND u.roles @> ARRAY['MECHANIC']::TEXT[]
        )
        "#,
    )
    .bind(*mechanic_id.as_uuid())
    .bind(*branch_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;

    if valid {
        Ok(())
    } else {
        Err(KernelError::validation(
            "inspection schedule mechanic must be an active prevention mechanic in the branch",
        )
        .into())
    }
}

async fn lock_schedule_tx(
    tx: &mut Transaction<'_, Postgres>,
    schedule_id: InspectionScheduleId,
) -> Result<LockedSchedule, PgInspectionError> {
    let row = sqlx::query(
        r#"
        SELECT id, branch_id, equipment_id, mechanic_id, status, completed_at, completed_by
        FROM regular_inspection_schedules
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(*schedule_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;

    let status_raw: String = row.try_get("status")?;
    Ok(LockedSchedule {
        id: InspectionScheduleId::from_uuid(row.try_get("id")?),
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        equipment_id: EquipmentId::from_uuid(row.try_get("equipment_id")?),
        mechanic_id: UserId::from_uuid(row.try_get("mechanic_id")?),
        status: InspectionScheduleStatus::from_db_str(&status_raw)?,
        completed_at: row.try_get("completed_at")?,
        completed_by: row
            .try_get::<Option<uuid::Uuid>, _>("completed_by")?
            .map(UserId::from_uuid),
    })
}

async fn fetch_schedule_summary_tx(
    tx: &mut Transaction<'_, Postgres>,
    schedule_id: InspectionScheduleId,
) -> Result<InspectionScheduleSummary, PgInspectionError> {
    let row = sqlx::query(
        r#"
        SELECT
            s.id, s.branch_id, s.equipment_id, s.mechanic_id, s.cycle,
            s.interval_days, s.due_date, s.status, s.completed_at, s.note,
            site.name AS site_name, e.management_no, e.model,
            mech.display_name AS mechanic_display_name,
            s.created_at, s.updated_at
        FROM regular_inspection_schedules s
        JOIN registry_equipment e ON e.id = s.equipment_id
        JOIN registry_sites site ON site.id = e.site_id
        LEFT JOIN users mech ON mech.id = s.mechanic_id
        WHERE s.id = $1
        "#,
    )
    .bind(*schedule_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    schedule_from_row(&row)
}

async fn fetch_round_summary_tx(
    tx: &mut Transaction<'_, Postgres>,
    round_id: InspectionRoundId,
) -> Result<InspectionRoundSummary, PgInspectionError> {
    let row = sqlx::query(
        r#"
        SELECT id, schedule_id, branch_id, equipment_id, mechanic_id, completed_by,
               outcome, findings, note, completed_at
        FROM inspection_rounds
        WHERE id = $1
        "#,
    )
    .bind(*round_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    round_from_row(&row)
}

fn schedule_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<InspectionScheduleSummary, PgInspectionError> {
    let cycle_raw: String = row.try_get("cycle")?;
    let status_raw: String = row.try_get("status")?;
    Ok(InspectionScheduleSummary {
        id: InspectionScheduleId::from_uuid(row.try_get("id")?),
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        equipment_id: EquipmentId::from_uuid(row.try_get("equipment_id")?),
        mechanic_id: UserId::from_uuid(row.try_get("mechanic_id")?),
        mechanic_display_name: row.try_get("mechanic_display_name")?,
        cycle: console_inspection_domain::InspectionCycle::from_db_str(&cycle_raw)?,
        interval_days: row.try_get("interval_days")?,
        due_date: row.try_get("due_date")?,
        status: InspectionScheduleStatus::from_db_str(&status_raw)?,
        completed_at: row.try_get("completed_at")?,
        note: row.try_get("note")?,
        site_name: row.try_get("site_name")?,
        management_no: row.try_get("management_no")?,
        model: row.try_get("model")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn round_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<InspectionRoundSummary, PgInspectionError> {
    let outcome_raw: String = row.try_get("outcome")?;
    Ok(InspectionRoundSummary {
        id: InspectionRoundId::from_uuid(row.try_get("id")?),
        schedule_id: InspectionScheduleId::from_uuid(row.try_get("schedule_id")?),
        branch_id: BranchId::from_uuid(row.try_get("branch_id")?),
        equipment_id: EquipmentId::from_uuid(row.try_get("equipment_id")?),
        mechanic_id: UserId::from_uuid(row.try_get("mechanic_id")?),
        completed_by: UserId::from_uuid(row.try_get("completed_by")?),
        outcome: InspectionRoundOutcome::from_db_str(&outcome_raw)?,
        findings: row.try_get("findings")?,
        note: row.try_get("note")?,
        completed_at: row.try_get("completed_at")?,
    })
}

fn ensure_branch(actual: BranchId, expected: BranchId) -> Result<(), PgInspectionError> {
    if actual == expected {
        Ok(())
    } else {
        Err(
            KernelError::validation("inspection equipment must belong to the schedule branch")
                .into(),
        )
    }
}

fn push_branch_column_filter(
    builder: &mut QueryBuilder<Postgres>,
    branch_scope: &BranchScope,
    column: &'static str,
) {
    match branch_scope {
        BranchScope::All => {
            builder.push("TRUE");
        }
        BranchScope::Branches(branches) if branches.is_empty() => {
            builder.push("FALSE");
        }
        BranchScope::Branches(branches) => {
            let branch_ids = branches
                .iter()
                .map(|branch_id| *branch_id.as_uuid())
                .collect::<Vec<_>>();
            builder.push(column);
            builder.push(" = ANY(");
            builder.push_bind(branch_ids);
            builder.push(")");
        }
    };
}
