//! PostgreSQL reads for the authenticated employee's attendance data.
//!
//! Each read resolves `users.employee_id` after the tenant transaction is
//! armed, then queries that one employee directly.  Do not route these methods
//! through the manager list or Week52 aggregations and filter afterwards.

use console_attendance_application::{
    self as app, AttendanceEvidence, AttendancePage, ListOwnExceptions, OwnAttendanceExceptionPage,
    OwnAttendanceExceptionRead, OwnExceptionResolutionRead, OwnWeek52Read, ReadOwnWeek52,
    SelfAttendanceScope,
};
use console_attendance_domain::{ExceptionKind, ResolutionAction, StrictDurationEvent};
use console_kernel_core::OrgId;
use console_platform_db::with_org_conn;
use sqlx::{Postgres, Row, Transaction};
use time::Duration;
use uuid::Uuid;

use crate::{
    AttendanceStoreError, PgAttendanceStore, strict_duration_event_kind, week52_boundary,
    week52_hours,
};

const LIST_OWN_EXCEPTIONS_SQL: &str = "\
    SELECT e.id,e.code,e.kind,e.status,e.work_date,e.occurred_at,e.detail,e.evidence,e.created_at,\
           r.action AS resolution_action,r.reason AS resolution_reason,\
           r.ot_hours::text AS ot_hours,r.resolved_at \
    FROM attendance_exceptions e \
    LEFT JOIN attendance_exception_resolutions r \
           ON r.exception_id=e.id AND r.org_id=e.org_id \
    WHERE e.employee_id=$1 AND e.work_date >= $2 AND e.work_date < $3 \
      AND ($4::text IS NULL OR e.status=$4) \
    ORDER BY e.work_date DESC,e.created_at DESC,e.id DESC \
    LIMIT $5 OFFSET $6";

impl PgAttendanceStore {
    /// Lists only the authenticated user's linked employee exceptions. An
    /// unlinked/inactive user receives an empty page, which a REST boundary can
    /// represent as a normal 200 response without revealing tenant state.
    pub async fn list_own_exceptions(
        &self,
        scope: SelfAttendanceScope,
        query: ListOwnExceptions,
    ) -> Result<OwnAttendanceExceptionPage, AttendanceStoreError> {
        let org = OrgId::from_uuid(scope.org_id);
        with_org_conn::<_, _, AttendanceStoreError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let Some(employee_id) = linked_employee_id(tx, scope.user_id).await? else {
                    return Ok(AttendancePage {
                        items: Vec::new(),
                        total: 0,
                        limit: query.limit,
                        offset: query.offset,
                    });
                };
                let rows = sqlx::query(LIST_OWN_EXCEPTIONS_SQL)
                    .bind(employee_id)
                    .bind(query.range.from)
                    .bind(query.range.to_exclusive)
                    .bind(&query.status)
                    .bind(query.limit)
                    .bind(query.offset)
                    .fetch_all(tx.as_mut())
                    .await?;
                let total = sqlx::query_scalar(
                    "SELECT count(*) FROM attendance_exceptions \
                     WHERE employee_id=$1 AND work_date >= $2 AND work_date < $3 \
                       AND ($4::text IS NULL OR status=$4)",
                )
                .bind(employee_id)
                .bind(query.range.from)
                .bind(query.range.to_exclusive)
                .bind(&query.status)
                .fetch_one(tx.as_mut())
                .await?;
                let items = rows
                    .iter()
                    .map(own_exception_read)
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

    /// Reads the linked employee's Week52 total without inspecting any other
    /// employee timeline. A malformed timeline for someone else therefore
    /// cannot poison this caller's read.
    pub async fn read_own_week52(
        &self,
        scope: SelfAttendanceScope,
        query: ReadOwnWeek52,
    ) -> Result<Option<OwnWeek52Read>, AttendanceStoreError> {
        let org = OrgId::from_uuid(scope.org_id);
        let week_start = query.week_start()?;
        let week_end = week_start + Duration::days(7);
        let week_start_at = week52_boundary(week_start)?;
        let week_end_at = week52_boundary(week_end)?;
        with_org_conn::<_, _, AttendanceStoreError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let Some(employee_id) = linked_employee_id(tx, scope.user_id).await? else {
                    return Ok(None);
                };
                let rows = sqlx::query(
                    "SELECT id,employee_id,kind,occurred_at FROM employee_attendance_records \
                     WHERE employee_id=$1 ORDER BY occurred_at,id",
                )
                .bind(employee_id)
                .fetch_all(tx.as_mut())
                .await?;
                let events = rows
                    .iter()
                    .map(|row| {
                        Ok(StrictDurationEvent {
                            id: row.try_get("id")?,
                            employee_id: row.try_get("employee_id")?,
                            kind: strict_duration_event_kind(&row.try_get::<String, _>("kind")?),
                            occurred_at: row.try_get("occurred_at")?,
                        })
                    })
                    .collect::<Result<Vec<_>, AttendanceStoreError>>()?;
                let hours = week52_hours(&events, week_start_at, week_end_at)?;
                let acknowledged_at = sqlx::query_scalar(
                    "SELECT acknowledged_at FROM attendance_week52_acknowledgements \
                     WHERE employee_id=$1 AND week_start=$2",
                )
                .bind(employee_id)
                .bind(week_start)
                .fetch_optional(tx.as_mut())
                .await?;
                let current_hours = hours.get(&employee_id).copied().unwrap_or(0.0);
                let input = app::Week52Input {
                    employee_id,
                    week_start,
                    current_hours,
                    projected_hours: current_hours,
                    acknowledged_at,
                };
                Ok(Some(OwnWeek52Read {
                    week_start,
                    current_hours,
                    projected_hours: current_hours,
                    tone: app::week52_tone(&input),
                    acknowledged_at,
                }))
            })
        })
        .await
    }
}

async fn linked_employee_id(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Option<Uuid>, AttendanceStoreError> {
    sqlx::query_scalar(
        "SELECT u.employee_id FROM users u \
         JOIN employees e ON e.id=u.employee_id AND e.org_id=u.org_id \
         WHERE u.id=$1 AND u.employee_id IS NOT NULL \
           AND u.is_active AND e.employment_status='ACTIVE'",
    )
    .bind(user_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(AttendanceStoreError::from)
}

fn own_exception_read(
    row: &sqlx::postgres::PgRow,
) -> Result<OwnAttendanceExceptionRead, AttendanceStoreError> {
    let evidence: Vec<AttendanceEvidence> = serde_json::from_value(row.try_get("evidence")?)
        .map_err(|_| AttendanceStoreError::Conflict)?;
    let resolution = match row.try_get::<Option<String>, _>("resolution_action")? {
        None => None,
        Some(action) => Some(OwnExceptionResolutionRead {
            action: ResolutionAction::parse(&action)
                .map_err(app::AttendanceApplicationError::from)?,
            reason: row.try_get("resolution_reason")?,
            ot_hours: row.try_get("ot_hours")?,
            resolved_at: row.try_get("resolved_at")?,
        }),
    };
    Ok(OwnAttendanceExceptionRead {
        id: row.try_get("id")?,
        code: row.try_get("code")?,
        kind: ExceptionKind::parse(&row.try_get::<String, _>("kind")?)
            .map_err(app::AttendanceApplicationError::from)?,
        status: row.try_get("status")?,
        work_date: row.try_get("work_date")?,
        occurred_at: row.try_get("occurred_at")?,
        detail: row.try_get("detail")?,
        evidence,
        resolution,
        created_at: row.try_get("created_at")?,
    })
}
