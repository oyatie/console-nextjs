//! Postgres reporting adapter.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::io::Cursor;

use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, KernelError, RegionId, Timestamp, TraceContext,
    UserId,
};
use console_platform_db::{DbError, with_audit, with_org_conn};
use console_platform_excel::{
    CellWrite, DAILY_STATUS_TEMPLATE, DailyStatusSection, SectionFill, TemplateRow,
    fill_template_bytes, umya_spreadsheet,
};
use console_platform_request_context::current_org;
use console_reporting_application::{
    ExportedWorkbook, KpiExportQuery, KpiQuery, KpiQueryError, KpiQueryPort, OpsSummaryPort,
    OpsSummaryQuery, ReportingExportError, ReportingExportPort, ReportingExportQuery,
    WorkDiaryConfirmCommand, WorkDiaryDraftPort, WorkDiaryQuery, WorkDiaryUpdateCommand,
};
use console_reporting_domain::{
    DailyStatusReport, DailyStatusRow, ExportSourceNote, KPI_EXPORT_HEADERS, KpiInputRecord,
    KpiInspectionRecord, KpiMetric, KpiP1Record, KpiPriorityLevel, KpiReport, KpiRollupScope,
    KpiScope, KpiWorkOrderStatus, KpiWorkResultType, OpsEquipmentStatus, OpsFunnel,
    OpsMechanicLoad, OpsSummary, PeriodicInspectionRow, UnavailableMetric, WorkDiaryActionEntry,
    WorkDiaryBody, WorkDiaryDraft, WorkDiaryStatus, calculate_kpi_report, kpi_export_rows,
};
use console_workorder_domain::{PriorityLevel, WorkOrderStatus, WorkResultType};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use time::{Date, Duration, OffsetDateTime, Time};

const DAILY_STATUS_TEMPLATE_BYTES: &[u8] =
    include_bytes!("../../../../../docs/reference/일일업무진행현황_0605.xlsx");
const WORK_DIARY_TEMPLATE_BYTES: &[u8] =
    include_bytes!("../../../../../docs/reference/업무일지_26.05.27.xlsx");
const EXCEL_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
const KPI_SHEET_NAME: &str = "KPI";

struct ExportLogCommand<'a> {
    export_kind: &'static str,
    action: &'static str,
    actor: UserId,
    branch_scope: BranchScope,
    export_date: Date,
    file_name: &'a str,
    source_notes: &'a [ExportSourceNote],
    trace: TraceContext,
    occurred_at: Timestamp,
}

/// Hard row cap for the date-bounded list `fetch_all` queries. The date window
/// already bounds normal output; this is only a memory backstop so a pathological
/// all-time `BranchScope::All` rollup can never materialize an unbounded result
/// set. It is deliberately well above any realistic single-window row count and
/// does not change the date-window semantics.
const MAX_LIST_ROWS: i64 = 10_000;

#[derive(Debug, thiserror::Error)]
enum PgReportingError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),

    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("workbook error: {0}")]
    Workbook(String),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<PgReportingError> for ReportingExportError {
    fn from(value: PgReportingError) -> Self {
        match value {
            PgReportingError::Db(error) => Self::Database(error.to_string()),
            PgReportingError::Domain(error) => Self::Kernel(error),
            PgReportingError::Sqlx(error) => Self::Database(error.to_string()),
            PgReportingError::Workbook(error) => Self::Workbook(error),
            PgReportingError::Json(error) => Self::Database(error.to_string()),
        }
    }
}

/// Resolve `id -> name` for a small set of ids using a fixed, literal SQL string
/// that selects the id and a `name`-aliased display column. The query text is a
/// `&'static str` (never built from input), so it satisfies the workspace's
/// `SqlSafeStr` gate; only the id list is bound. Runs inside the caller's
/// already-armed, org-scoped transaction so the lookup is RLS-bounded to the
/// caller's tenant.
async fn name_map(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    sql: &'static str,
    ids: &[uuid::Uuid],
) -> Result<std::collections::HashMap<uuid::Uuid, String>, PgReportingError> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let rows = sqlx::query(sql).bind(ids).fetch_all(tx.as_mut()).await?;
    let mut map = std::collections::HashMap::with_capacity(rows.len());
    for row in &rows {
        let id: uuid::Uuid = row.try_get("id")?;
        let name: String = row.try_get("name")?;
        map.insert(id, name);
    }
    Ok(map)
}

#[derive(Debug, Clone)]
pub struct PgKpiRepository {
    pool: PgPool,
}

pub type PgReportingRepository = PgKpiRepository;

impl PgKpiRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    async fn query_kpis_inner(&self, query: KpiQuery) -> Result<KpiReport, KpiQueryError> {
        if query.period.start >= query.period.end {
            return Err(KernelError::validation("KPI period start must be before end").into());
        }

        let records = self.load_work_order_records(&query).await?;
        let inspection_records = self.load_inspection_records(&query).await?;
        let p1_records = self.load_p1_records(&query).await?;
        let unavailable_metrics = self.unavailable_source_metrics().await?;
        let mut report = calculate_kpi_report(
            query.period,
            query.scope,
            &records,
            &inspection_records,
            &p1_records,
            unavailable_metrics,
        );
        self.resolve_scope_names(&mut report).await?;
        Ok(report)
    }

    /// Fill `scope_display_name` on every rollup so the console shows each scope
    /// by name (region/branch/mechanic) instead of a raw id. The pure domain
    /// calculation cannot do this — it has no DB — so we resolve the distinct
    /// scope ids here in ONE same-org-armed transaction. The `regions`,
    /// `branches` and `users` lookups are RLS-scoped to `app.current_org`, so
    /// they can only resolve names within the caller's tenant; an id from a
    /// since-deleted row simply stays `None` and the web falls back via
    /// `safeLabel`. The company-wide scope has no id and keeps `None`.
    async fn resolve_scope_names(&self, report: &mut KpiReport) -> Result<(), KpiQueryError> {
        let mut region_ids: Vec<uuid::Uuid> = Vec::new();
        let mut branch_ids: Vec<uuid::Uuid> = Vec::new();
        let mut user_ids: Vec<uuid::Uuid> = Vec::new();
        for rollup in &report.rollups {
            match rollup.scope {
                KpiRollupScope::Company => {}
                KpiRollupScope::Region(id) => region_ids.push(*id.as_uuid()),
                KpiRollupScope::Branch(id) => branch_ids.push(*id.as_uuid()),
                KpiRollupScope::Technician(id) => user_ids.push(*id.as_uuid()),
            }
        }
        if region_ids.is_empty() && branch_ids.is_empty() && user_ids.is_empty() {
            return Ok(());
        }

        let org = current_org().map_err(|e| KpiQueryError::Database(e.to_string()))?;
        let (regions, branches, users) =
            with_org_conn::<_, _, PgReportingError>(&self.pool, org, move |tx| {
                Box::pin(async move {
                    let regions = name_map(
                        tx,
                        "SELECT id, name AS name FROM regions WHERE id = ANY($1)",
                        &region_ids,
                    )
                    .await?;
                    let branches = name_map(
                        tx,
                        "SELECT id, name AS name FROM branches WHERE id = ANY($1)",
                        &branch_ids,
                    )
                    .await?;
                    let users = name_map(
                        tx,
                        "SELECT id, display_name AS name FROM users WHERE id = ANY($1)",
                        &user_ids,
                    )
                    .await?;
                    Ok((regions, branches, users))
                })
            })
            .await
            .map_err(|e| KpiQueryError::Database(e.to_string()))?;

        for rollup in &mut report.rollups {
            rollup.scope_display_name = match rollup.scope {
                KpiRollupScope::Company => None,
                KpiRollupScope::Region(id) => regions.get(id.as_uuid()).cloned(),
                KpiRollupScope::Branch(id) => branches.get(id.as_uuid()).cloned(),
                KpiRollupScope::Technician(id) => users.get(id.as_uuid()).cloned(),
            };
        }
        Ok(())
    }

    async fn load_work_order_records(
        &self,
        query: &KpiQuery,
    ) -> Result<Vec<KpiInputRecord>, KpiQueryError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT
                w.id AS work_order_id,
                w.branch_id,
                b.region_id,
                primary_assignment.mechanic_id AS technician_id,
                w.status,
                w.priority,
                w.result_type,
                w.delay_reason,
                w.created_at,
                first_start.occurred_at AS first_in_progress_at,
                completion_approval.approved_at,
                w.target_due_at
            FROM work_orders w
            JOIN branches b ON b.id = w.branch_id
            JOIN LATERAL (
                SELECT approved_at
                FROM work_order_approval_steps
                WHERE work_order_id = w.id
                  AND role IN ('EXECUTIVE', 'ADMIN')
                  AND status = 'APPROVED'
                  AND approved_at IS NOT NULL
                ORDER BY CASE role WHEN 'EXECUTIVE' THEN 1 ELSE 2 END, step_order DESC
                LIMIT 1
            ) completion_approval ON TRUE
            LEFT JOIN LATERAL (
                SELECT occurred_at
                FROM work_order_status_history
                WHERE work_order_id = w.id
                  AND to_status = 'IN_PROGRESS'
                ORDER BY occurred_at, id
                LIMIT 1
            ) first_start ON TRUE
            LEFT JOIN LATERAL (
                SELECT mechanic_id
                FROM work_order_assignments
                WHERE work_order_id = w.id
                  AND role = 'PRIMARY'
                ORDER BY assigned_at, id
                LIMIT 1
            ) primary_assignment ON TRUE
            WHERE completion_approval.approved_at >=
            "#,
        );
        builder.push_bind(query.period.start);
        builder.push(" AND completion_approval.approved_at < ");
        builder.push_bind(query.period.end);
        builder.push(" AND w.kpi_excluded = FALSE");
        builder.push(
            r#"
            AND NOT EXISTS (
                SELECT 1
                FROM kpi_exclusions ex
                WHERE ex.scope = 'WORK_ORDER'
                  AND ex.target_id = w.id
                  AND ex.revoked_at IS NULL
            )
            AND NOT EXISTS (
                SELECT 1
                FROM outsource_works ow
                JOIN kpi_exclusions ex
                  ON ex.scope = 'OUTSOURCE'
                 AND ex.target_id = ow.id
                 AND ex.revoked_at IS NULL
                WHERE ow.work_order_id = w.id
            )
            AND
            "#,
        );
        push_branch_scope_filter(&mut builder, &query.branch_scope);
        push_requested_scope_filter(&mut builder, query.scope);
        builder.push(" ORDER BY completion_approval.approved_at, w.id LIMIT ");
        builder.push_bind(MAX_LIST_ROWS);

        let org = current_org().map_err(|e| KpiQueryError::Database(e.to_string()))?;
        let rows = with_org_conn::<_, _, PgReportingError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await
        .map_err(|e| KpiQueryError::Database(e.to_string()))?;

        rows.iter()
            .map(record_from_row)
            .collect::<Result<Vec<_>, _>>()
    }

    async fn unavailable_source_metrics(&self) -> Result<Vec<UnavailableMetric>, KpiQueryError> {
        let mut unavailable = Vec::new();
        if !any_table_exists(&self.pool, &["regular_inspection_schedules"])
            .await
            .map_err(|err| KpiQueryError::Database(err.to_string()))?
        {
            unavailable.push(UnavailableMetric {
                metric: KpiMetric::InspectionPlanCompletionRate,
                source_domain: "inspection".to_owned(),
                reason:
                    "regular inspection schedule source tables are not present in this database"
                        .to_owned(),
            });
        }
        if !any_table_exists(&self.pool, &["p1_dispatches", "p1_dispatch_responses"])
            .await
            .map_err(|err| KpiQueryError::Database(err.to_string()))?
        {
            unavailable.push(UnavailableMetric {
                metric: KpiMetric::P1AcceptanceRate,
                source_domain: "dispatch".to_owned(),
                reason: "P1 dispatch source tables are not present in this database".to_owned(),
            });
        }
        Ok(unavailable)
    }

    async fn load_inspection_records(
        &self,
        query: &KpiQuery,
    ) -> Result<Vec<KpiInspectionRecord>, KpiQueryError> {
        if !any_table_exists(&self.pool, &["regular_inspection_schedules"])
            .await
            .map_err(|err| KpiQueryError::Database(err.to_string()))?
        {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT
                s.id AS schedule_id,
                s.branch_id,
                b.region_id,
                s.mechanic_id AS technician_id,
                s.completed_at IS NOT NULL AND s.completed_at <
            "#,
        );
        builder.push_bind(query.period.end);
        builder.push(
            r#" AS completed
            FROM regular_inspection_schedules s
            JOIN branches b ON b.id = s.branch_id
            WHERE s.status <> 'CANCELLED'
              AND s.due_date >=
            "#,
        );
        builder.push_bind(query.period.start.date());
        builder.push(" AND s.due_date < ");
        builder.push_bind(query.period.end.date());
        builder.push(" AND ");
        push_branch_column_filter(&mut builder, &query.branch_scope, "s.branch_id");
        push_requested_inspection_scope_filter(&mut builder, query.scope);
        builder.push(" ORDER BY s.due_date, s.id LIMIT ");
        builder.push_bind(MAX_LIST_ROWS);

        let org = current_org().map_err(|e| KpiQueryError::Database(e.to_string()))?;
        let rows = with_org_conn::<_, _, PgReportingError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await
        .map_err(|e| KpiQueryError::Database(e.to_string()))?;
        rows.iter()
            .map(inspection_record_from_row)
            .collect::<Result<Vec<_>, _>>()
    }

    async fn load_p1_records(&self, query: &KpiQuery) -> Result<Vec<KpiP1Record>, KpiQueryError> {
        if !any_table_exists(&self.pool, &["p1_dispatches", "p1_dispatch_responses"])
            .await
            .map_err(|err| KpiQueryError::Database(err.to_string()))?
        {
            return Ok(Vec::new());
        }

        // P1 acceptance rate: of P1 emergency dispatches whose broadcast accept
        // window opened within the period, the share that were accepted by a
        // mechanic — i.e. the dispatch auto-assigned (AUTO_ASSIGNED), or at least
        // one target answered ACCEPT — rather than falling through to a manager
        // force-assign. The accepting technician is attributed via the accepting
        // response (or the auto-assigned mechanic) for the technician rollup.
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT
                d.id AS dispatch_id,
                d.branch_id,
                b.region_id,
                COALESCE(d.auto_assigned_mechanic_id, accept.user_id) AS technician_id,
                (d.status = 'AUTO_ASSIGNED' OR accept.user_id IS NOT NULL) AS accepted
            FROM p1_dispatches d
            JOIN branches b ON b.id = d.branch_id
            LEFT JOIN LATERAL (
                SELECT user_id
                FROM p1_dispatch_responses
                WHERE dispatch_id = d.id
                  AND response = 'ACCEPT'
                ORDER BY responded_at, id
                LIMIT 1
            ) accept ON TRUE
            WHERE d.accept_window_started_at >=
            "#,
        );
        builder.push_bind(query.period.start);
        builder.push(" AND d.accept_window_started_at < ");
        builder.push_bind(query.period.end);
        builder.push(" AND ");
        push_branch_column_filter(&mut builder, &query.branch_scope, "d.branch_id");
        push_requested_p1_scope_filter(&mut builder, query.scope);
        builder.push(" ORDER BY d.accept_window_started_at, d.id LIMIT ");
        builder.push_bind(MAX_LIST_ROWS);

        let org = current_org().map_err(|e| KpiQueryError::Database(e.to_string()))?;
        let rows = with_org_conn::<_, _, PgReportingError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await
        .map_err(|e| KpiQueryError::Database(e.to_string()))?;
        rows.iter()
            .map(p1_record_from_row)
            .collect::<Result<Vec<_>, _>>()
    }

    async fn export_daily_status_inner(
        &self,
        query: ReportingExportQuery,
    ) -> Result<ExportedWorkbook, PgReportingError> {
        let report = self.daily_status_report(&query).await?;
        let bytes = render_daily_status(&report)?;
        let file_name = format!("daily-status-{}.xlsx", iso_date(query.date));
        self.record_export_log(ExportLogCommand {
            export_kind: "daily_status",
            action: "export.daily_status",
            actor: query.actor,
            branch_scope: query.branch_scope,
            export_date: query.date,
            file_name: &file_name,
            source_notes: &report.source_notes,
            trace: query.trace,
            occurred_at: query.occurred_at,
        })
        .await?;

        Ok(ExportedWorkbook {
            file_name,
            content_type: EXCEL_CONTENT_TYPE,
            bytes,
        })
    }

    async fn export_work_diary_inner(
        &self,
        query: ReportingExportQuery,
    ) -> Result<ExportedWorkbook, PgReportingError> {
        let draft = self
            .get_or_generate_work_diary_inner(WorkDiaryQuery {
                actor: query.actor,
                date: query.date,
                branch_scope: query.branch_scope.clone(),
                trace: query.trace.clone(),
                occurred_at: query.occurred_at,
            })
            .await?;
        let bytes = render_work_diary(query.date, &draft.body)?;
        let file_name = format!("work-diary-{}.xlsx", iso_date(query.date));
        self.record_export_log(ExportLogCommand {
            export_kind: "work_diary",
            action: "export.work_diary",
            actor: query.actor,
            branch_scope: query.branch_scope,
            export_date: query.date,
            file_name: &file_name,
            source_notes: &draft.body.source_notes,
            trace: query.trace,
            occurred_at: query.occurred_at,
        })
        .await?;

        Ok(ExportedWorkbook {
            file_name,
            content_type: EXCEL_CONTENT_TYPE,
            bytes,
        })
    }

    async fn export_kpi_inner(
        &self,
        query: KpiExportQuery,
    ) -> Result<ExportedWorkbook, ReportingExportError> {
        // Build the workbook from the SAME aggregation the JSON KPI endpoint
        // serves, so the export can never diverge from the on-screen numbers.
        let period = query.period;
        let report = self
            .query_kpis_inner(KpiQuery {
                period,
                scope: query.scope,
                branch_scope: query.branch_scope.clone(),
            })
            .await?;
        let bytes = render_kpi(&report)?;
        let file_name = format!(
            "kpi-{}-to-{}.xlsx",
            iso_date(period.start.date()),
            iso_date(period.end.date())
        );
        let source_notes: Vec<ExportSourceNote> = report
            .unavailable_metrics
            .iter()
            .map(|metric| ExportSourceNote {
                source_domain: metric.source_domain.clone(),
                reason: metric.reason.clone(),
            })
            .collect();
        // Audited exactly like the sibling daily-status / work-diary exports:
        // one excel_export_logs row + one audit_events row, under RLS as console_rt.
        self.record_export_log(ExportLogCommand {
            export_kind: "kpi",
            action: "export.kpi",
            actor: query.actor,
            branch_scope: query.branch_scope,
            export_date: period.start.date(),
            file_name: &file_name,
            source_notes: &source_notes,
            trace: query.trace,
            occurred_at: query.occurred_at,
        })
        .await?;
        Ok(ExportedWorkbook {
            file_name,
            content_type: EXCEL_CONTENT_TYPE,
            bytes,
        })
    }

    async fn get_or_generate_work_diary_inner(
        &self,
        query: WorkDiaryQuery,
    ) -> Result<WorkDiaryDraft, PgReportingError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let scope_key = scope_key(&query.branch_scope);
        if let Some(draft) = self.fetch_work_diary(query.date, &scope_key).await? {
            return Ok(draft);
        }

        let body = self.generate_work_diary_body(&query).await?;
        let body_value = serde_json::to_value(&body)?;
        let draft_id = uuid::Uuid::new_v4();
        let branch_id = single_branch(&query.branch_scope);
        let event = audit_event(
            "work_diary.generate",
            query.actor,
            "work_diary_draft",
            draft_id,
            branch_id,
            query.trace,
            query.occurred_at,
        )?
        .with_org(org);

        let actor = *query.actor.as_uuid();
        let date = query.date;
        let status = WorkDiaryStatus::Draft.as_str();
        let scope_key_for_insert = scope_key.clone();
        let branch_uuid = branch_id.map(|id| *id.as_uuid());
        let occurred_at = query.occurred_at;
        with_audit::<_, (), PgReportingError>(&self.pool, event, |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    INSERT INTO work_diary_drafts (
                        id, diary_date, branch_id, scope_key, status, body,
                        generated_by, generated_at, created_at, updated_at, org_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, $8, $9)
                    "#,
                )
                .bind(draft_id)
                .bind(date)
                .bind(branch_uuid)
                .bind(scope_key_for_insert)
                .bind(status)
                .bind(body_value)
                .bind(actor)
                .bind(occurred_at)
                .bind(org_uuid)
                .execute(tx.as_mut())
                .await?;
                Ok(())
            })
        })
        .await?;

        self.fetch_work_diary(query.date, &scope_key)
            .await?
            .ok_or_else(|| {
                KernelError::not_found("generated work diary draft was not found after insert")
                    .into()
            })
    }

    async fn update_work_diary_inner(
        &self,
        command: WorkDiaryUpdateCommand,
    ) -> Result<WorkDiaryDraft, PgReportingError> {
        let org = current_org().map_err(KernelError::from)?;
        let scope_key = scope_key(&command.branch_scope);
        let existing = self
            .fetch_work_diary(command.date, &scope_key)
            .await?
            .ok_or_else(|| KernelError::not_found("work diary draft was not found"))?;
        if existing.status != WorkDiaryStatus::Draft {
            return Err(KernelError::conflict("confirmed work diary cannot be edited").into());
        }

        let body_value = serde_json::to_value(&command.body)?;
        let event = audit_event(
            "work_diary.update",
            command.actor,
            "work_diary_draft",
            existing.id,
            single_branch(&command.branch_scope),
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        let actor = *command.actor.as_uuid();
        let date = command.date;
        let occurred_at = command.occurred_at;
        let scope_key_for_update = scope_key.clone();
        with_audit::<_, (), PgReportingError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    r#"
                    UPDATE work_diary_drafts
                    SET body = $3,
                        edited_by = $4,
                        edited_at = $5,
                        updated_at = $5
                    WHERE diary_date = $1
                      AND scope_key = $2
                      AND status = 'DRAFT'
                    "#,
                )
                .bind(date)
                .bind(scope_key_for_update)
                .bind(body_value)
                .bind(actor)
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                if rows != 1 {
                    return Err(KernelError::conflict("work diary draft was not editable").into());
                }
                Ok(())
            })
        })
        .await?;

        self.fetch_work_diary(command.date, &scope_key)
            .await?
            .ok_or_else(|| KernelError::not_found("work diary draft was not found").into())
    }

    async fn confirm_work_diary_inner(
        &self,
        command: WorkDiaryConfirmCommand,
    ) -> Result<WorkDiaryDraft, PgReportingError> {
        let org = current_org().map_err(KernelError::from)?;
        let scope_key = scope_key(&command.branch_scope);
        let existing = self
            .fetch_work_diary(command.date, &scope_key)
            .await?
            .ok_or_else(|| KernelError::not_found("work diary draft was not found"))?;
        if existing.status != WorkDiaryStatus::Draft {
            return Err(KernelError::conflict("work diary is already confirmed").into());
        }

        let event = audit_event(
            "work_diary.confirm",
            command.actor,
            "work_diary_draft",
            existing.id,
            single_branch(&command.branch_scope),
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);

        let actor = *command.actor.as_uuid();
        let date = command.date;
        let occurred_at = command.occurred_at;
        let scope_key_for_update = scope_key.clone();
        with_audit::<_, (), PgReportingError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    r#"
                    UPDATE work_diary_drafts
                    SET status = 'CONFIRMED',
                        confirmed_by = $3,
                        confirmed_at = $4,
                        updated_at = $4
                    WHERE diary_date = $1
                      AND scope_key = $2
                      AND status = 'DRAFT'
                    "#,
                )
                .bind(date)
                .bind(scope_key_for_update)
                .bind(actor)
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?
                .rows_affected();
                if rows != 1 {
                    return Err(
                        KernelError::conflict("work diary draft was not confirmable").into(),
                    );
                }
                Ok(())
            })
        })
        .await?;

        self.fetch_work_diary(command.date, &scope_key)
            .await?
            .ok_or_else(|| KernelError::not_found("work diary draft was not found").into())
    }

    async fn daily_status_report(
        &self,
        query: &ReportingExportQuery,
    ) -> Result<DailyStatusReport, PgReportingError> {
        let source_notes = self.export_source_notes().await?;
        Ok(DailyStatusReport {
            date: query.date,
            results: self
                .load_completed_rows(query.date, &query.branch_scope)
                .await?,
            plans: self.load_plan_rows(query.date, &query.branch_scope).await?,
            pending_backlog: self.load_pending_rows(&query.branch_scope).await?,
            periodic_inspections: self
                .load_periodic_inspection_rows(query.date, &query.branch_scope)
                .await?,
            source_notes,
        })
    }

    async fn generate_work_diary_body(
        &self,
        query: &WorkDiaryQuery,
    ) -> Result<WorkDiaryBody, PgReportingError> {
        let source_notes = self.export_source_notes().await?;
        let previous_results = self
            .load_completed_rows(query.date, &query.branch_scope)
            .await?;
        let next_date = query
            .date
            .next_day()
            .ok_or_else(|| KernelError::validation("work diary date overflow"))?;
        let today_plans = self.load_plan_rows(next_date, &query.branch_scope).await?;
        let urgent_actions = self
            .load_work_diary_action_entries(query.date, &query.branch_scope)
            .await?;

        Ok(WorkDiaryBody {
            previous_results: diary_results_text(&previous_results),
            today_plans: diary_plans_text(&today_plans),
            urgent_actions,
            source_notes,
        })
    }

    async fn record_export_log(
        &self,
        command: ExportLogCommand<'_>,
    ) -> Result<(), PgReportingError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let log_id = uuid::Uuid::new_v4();
        let branch_id = single_branch(&command.branch_scope);
        // scope_key (NOT NULL) is the authoritative scope discriminator: "ALL" for
        // company-wide rollups, "region:<id>" for region rollups, or the branch UUID
        // string for branch-scope rows. branch_id is a nullable denormalized FK that is
        // populated ONLY when the scope resolves to exactly one branch; company/region
        // rollup rows carry NULL branch_id by design (finding-#6 disposition).
        let event = audit_event(
            command.action,
            command.actor,
            "excel_export",
            log_id,
            branch_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);
        let actor_uuid = *command.actor.as_uuid();
        let branch_uuid = branch_id.map(|id| *id.as_uuid());
        let scope_key = scope_key(&command.branch_scope);
        let source_notes = serde_json::to_value(command.source_notes)?;
        let file_name = command.file_name.to_owned();
        let export_kind = command.export_kind;
        let export_date = command.export_date;
        let occurred_at = command.occurred_at;

        with_audit::<_, (), PgReportingError>(&self.pool, event, |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    INSERT INTO excel_export_logs (
                        id, actor, branch_id, scope_key, export_kind, export_date,
                        file_name, source_notes, created_at, org_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                    "#,
                )
                .bind(log_id)
                .bind(actor_uuid)
                .bind(branch_uuid)
                .bind(scope_key)
                .bind(export_kind)
                .bind(export_date)
                .bind(file_name)
                .bind(source_notes)
                .bind(occurred_at)
                .bind(org_uuid)
                .execute(tx.as_mut())
                .await?;
                Ok(())
            })
        })
        .await
    }

    async fn fetch_work_diary(
        &self,
        date: Date,
        scope_key: &str,
    ) -> Result<Option<WorkDiaryDraft>, PgReportingError> {
        let scope_key = scope_key.to_owned();
        let org = current_org().map_err(KernelError::from)?;
        let row = with_org_conn::<_, _, PgReportingError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                Ok(sqlx::query(
                    r#"
            SELECT id, diary_date, status, body, confirmed_by, confirmed_at
            FROM work_diary_drafts
            WHERE diary_date = $1
              AND scope_key = $2
            "#,
                )
                .bind(date)
                .bind(scope_key)
                .fetch_optional(tx.as_mut())
                .await?)
            })
        })
        .await?;

        row.map(|row| draft_from_row(&row)).transpose()
    }

    async fn load_completed_rows(
        &self,
        date: Date,
        branch_scope: &BranchScope,
    ) -> Result<Vec<DailyStatusRow>, PgReportingError> {
        let (start, end) = day_bounds(date);
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT
                w.created_at::date AS request_date,
                s.name AS site_name,
                e.management_no,
                e.model,
                e.vin,
                w.symptom,
                primary_assignment.mechanic_name,
                w.target_due_at::date AS scheduled_date,
                completion_approval.approved_at::date AS completed_date,
                w.action_taken AS result_note,
                w.priority,
                w.status
            FROM work_orders w
            JOIN registry_equipment e ON e.id = w.equipment_id
            JOIN registry_sites s ON s.id = w.site_id
            JOIN LATERAL (
                SELECT approved_at
                FROM work_order_approval_steps
                WHERE work_order_id = w.id
                  AND role IN ('EXECUTIVE', 'ADMIN')
                  AND status = 'APPROVED'
                  AND approved_at IS NOT NULL
                ORDER BY CASE role WHEN 'EXECUTIVE' THEN 1 ELSE 2 END, step_order DESC
                LIMIT 1
            ) completion_approval ON TRUE
            LEFT JOIN LATERAL (
                SELECT u.display_name AS mechanic_name
                FROM work_order_assignments a
                JOIN users u ON u.id = a.mechanic_id
                WHERE a.work_order_id = w.id
                  AND a.role = 'PRIMARY'
                ORDER BY a.assigned_at, a.id
                LIMIT 1
            ) primary_assignment ON TRUE
            WHERE w.status = 'FINAL_COMPLETED'
              AND completion_approval.approved_at >=
            "#,
        );
        builder.push_bind(start);
        builder.push(" AND completion_approval.approved_at < ");
        builder.push_bind(end);
        builder.push(" AND ");
        push_branch_column_filter(&mut builder, branch_scope, "w.branch_id");
        builder.push(" ORDER BY completion_approval.approved_at, w.request_no LIMIT ");
        builder.push_bind(MAX_LIST_ROWS);

        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgReportingError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        rows.iter().map(daily_status_row_from_row).collect()
    }

    async fn load_plan_rows(
        &self,
        date: Date,
        branch_scope: &BranchScope,
    ) -> Result<Vec<DailyStatusRow>, PgReportingError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT
                w.created_at::date AS request_date,
                COALESCE(s.name, i.description) AS site_name,
                e.management_no,
                e.model,
                e.vin,
                COALESCE(w.symptom, i.description) AS symptom,
                u.display_name AS mechanic_name,
                COALESCE(w.target_due_at::date, p.plan_date) AS scheduled_date,
                NULL::date AS completed_date,
                i.description AS result_note,
                COALESCE(w.priority, 'UNSET') AS priority,
                COALESCE(w.status, p.status) AS status
            FROM daily_work_plans p
            JOIN daily_work_plan_items i ON i.plan_id = p.id
            JOIN users u ON u.id = p.mechanic_id
            LEFT JOIN work_orders w ON w.id = i.work_order_id
            LEFT JOIN registry_equipment e ON e.id = w.equipment_id
            LEFT JOIN registry_sites s ON s.id = w.site_id
            WHERE p.plan_date =
            "#,
        );
        builder.push_bind(date);
        builder.push(" AND p.status IN ('APPROVED', 'FINAL_CONFIRMED') AND ");
        push_branch_column_filter(&mut builder, branch_scope, "p.branch_id");
        builder.push(" ORDER BY u.display_name, i.sort_order, i.id LIMIT ");
        builder.push_bind(MAX_LIST_ROWS);

        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgReportingError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        rows.iter().map(daily_status_row_from_row).collect()
    }

    async fn load_pending_rows(
        &self,
        branch_scope: &BranchScope,
    ) -> Result<Vec<DailyStatusRow>, PgReportingError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT
                w.created_at::date AS request_date,
                s.name AS site_name,
                e.management_no,
                e.model,
                e.vin,
                w.symptom,
                primary_assignment.mechanic_name,
                w.target_due_at::date AS scheduled_date,
                NULL::date AS completed_date,
                w.action_taken AS result_note,
                w.priority,
                w.status
            FROM work_orders w
            JOIN registry_equipment e ON e.id = w.equipment_id
            JOIN registry_sites s ON s.id = w.site_id
            LEFT JOIN LATERAL (
                SELECT u.display_name AS mechanic_name
                FROM work_order_assignments a
                JOIN users u ON u.id = a.mechanic_id
                WHERE a.work_order_id = w.id
                  AND a.role = 'PRIMARY'
                ORDER BY a.assigned_at, a.id
                LIMIT 1
            ) primary_assignment ON TRUE
            WHERE w.status NOT IN ('FINAL_COMPLETED', 'REJECTED', 'ARCHIVED', 'CANCELLED')
              AND
            "#,
        );
        push_branch_column_filter(&mut builder, branch_scope, "w.branch_id");
        builder.push(" ORDER BY w.created_at, w.request_no LIMIT ");
        builder.push_bind(MAX_LIST_ROWS);

        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgReportingError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        rows.iter().map(daily_status_row_from_row).collect()
    }

    async fn load_work_diary_action_entries(
        &self,
        date: Date,
        branch_scope: &BranchScope,
    ) -> Result<Vec<WorkDiaryActionEntry>, PgReportingError> {
        let (start, end) = day_bounds(date);
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT
                s.name AS site_name,
                e.management_no,
                w.diagnosis,
                w.action_taken
            FROM work_orders w
            JOIN registry_equipment e ON e.id = w.equipment_id
            JOIN registry_sites s ON s.id = w.site_id
            JOIN LATERAL (
                SELECT approved_at
                FROM work_order_approval_steps
                WHERE work_order_id = w.id
                  AND role IN ('EXECUTIVE', 'ADMIN')
                  AND status = 'APPROVED'
                  AND approved_at IS NOT NULL
                ORDER BY CASE role WHEN 'EXECUTIVE' THEN 1 ELSE 2 END, step_order DESC
                LIMIT 1
            ) completion_approval ON TRUE
            WHERE completion_approval.approved_at >=
            "#,
        );
        builder.push_bind(start);
        builder.push(" AND completion_approval.approved_at < ");
        builder.push_bind(end);
        builder.push(" AND (w.diagnosis IS NOT NULL OR w.action_taken IS NOT NULL) AND ");
        push_branch_column_filter(&mut builder, branch_scope, "w.branch_id");
        builder.push(" ORDER BY completion_approval.approved_at, w.request_no LIMIT ");
        builder.push_bind(MAX_LIST_ROWS);

        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgReportingError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        rows.iter().map(action_entry_from_row).collect()
    }

    async fn load_periodic_inspection_rows(
        &self,
        date: Date,
        branch_scope: &BranchScope,
    ) -> Result<Vec<PeriodicInspectionRow>, PgReportingError> {
        if !any_table_exists(&self.pool, &["regular_inspection_schedules"]).await? {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT
                site.name AS site_name,
                e.vehicle_registration_no AS vehicle_no,
                e.management_no,
                e.model,
                e.vin AS serial_no,
                CASE
                    WHEN s.status = 'COMPLETED' THEN COALESCE(r.findings, '정기검사 완료')
                    ELSE '정기검사 예정'
                END AS issue,
                CONCAT(
                    CASE s.cycle
                        WHEN 'DAILY' THEN '일간'
                        WHEN 'WEEKLY' THEN '주간'
                        WHEN 'MONTHLY' THEN '월간'
                        WHEN 'QUARTERLY' THEN '분기'
                        WHEN 'YEARLY' THEN '연간'
                        ELSE '사용자 지정'
                    END,
                    ' / ',
                    s.due_date::TEXT
                ) AS inspection_period,
                COALESCE(r.note, s.note) AS note
            FROM regular_inspection_schedules s
            JOIN registry_equipment e ON e.id = s.equipment_id
            JOIN registry_sites site ON site.id = e.site_id
            LEFT JOIN LATERAL (
                SELECT findings, note
                FROM inspection_rounds
                WHERE schedule_id = s.id
                ORDER BY completed_at DESC, id DESC
                LIMIT 1
            ) r ON TRUE
            WHERE s.status <> 'CANCELLED'
              AND s.due_date =
            "#,
        );
        builder.push_bind(date);
        builder.push(" AND ");
        push_branch_column_filter(&mut builder, branch_scope, "s.branch_id");
        builder.push(" ORDER BY site.name, e.management_no, s.id LIMIT ");
        builder.push_bind(MAX_LIST_ROWS);

        let org = current_org().map_err(KernelError::from)?;
        let rows = with_org_conn::<_, _, PgReportingError>(&self.pool, org, move |tx| {
            Box::pin(async move { Ok(builder.build().fetch_all(tx.as_mut()).await?) })
        })
        .await?;
        rows.iter().map(periodic_inspection_row_from_row).collect()
    }

    async fn export_source_notes(&self) -> Result<Vec<ExportSourceNote>, PgReportingError> {
        if any_table_exists(&self.pool, &["regular_inspection_schedules"]).await? {
            return Ok(Vec::new());
        }
        Ok(vec![ExportSourceNote {
            source_domain: "inspection".to_owned(),
            reason: "regular inspection schedule source tables are not present in this database"
                .to_owned(),
        }])
    }

    /// Compute the per-tenant operational rollup in ONE org-scoped transaction.
    ///
    /// Every count is a tenant-local aggregate: the closure runs inside
    /// `with_org_conn(current_org())`, so Postgres RLS narrows every table to the
    /// requesting org and a second tenant's rows are never observable. The
    /// individual aggregates are cheap (indexed status columns / partial unique
    /// indexes); see `idx_work_orders_branch_status`,
    /// `idx_registry_equipment_status_*`, and the substitution partial indexes.
    async fn ops_summary_inner(&self, query: OpsSummaryQuery) -> Result<OpsSummary, KpiQueryError> {
        let aging_hours = i64::from(query.aging_hours);
        let at_risk_minutes = i64::from(query.at_risk_minutes);
        let top_mechanics = i64::from(query.top_mechanics);

        let org = current_org().map_err(KernelError::from)?;
        let summary = with_org_conn::<_, _, PgReportingError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                // Work-order funnel + aging, computed in a single scan.
                let funnel_row = sqlx::query!(
                    r#"
                    SELECT
                        COUNT(*) FILTER (
                            WHERE status IN ('RECEIVED', 'UNASSIGNED')
                        ) AS "received!",
                        COUNT(*) FILTER (WHERE status = 'ASSIGNED') AS "assigned!",
                        COUNT(*) FILTER (
                            WHERE status IN ('IN_PROGRESS', 'REPORT_SUBMITTED', 'ADMIN_REVIEW')
                        ) AS "in_progress!",
                        COUNT(*) FILTER (WHERE status = 'FINAL_COMPLETED') AS "completed!",
                        COUNT(*) FILTER (
                            WHERE status NOT IN (
                                'FINAL_COMPLETED', 'REJECTED', 'ARCHIVED', 'CANCELLED'
                            )
                            AND created_at < now() - make_interval(hours => $1::int)
                        ) AS "aging!"
                    FROM work_orders
                    "#,
                    aging_hours as i32,
                )
                .fetch_one(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;

                // P1 SLA risk: dispatches still broadcasting whose accept window
                // has expired (breached) or expires within the lead window.
                let sla_row = sqlx::query!(
                    r#"
                    SELECT
                        COUNT(*) FILTER (
                            WHERE accept_window_ends_at < now()
                        ) AS "breached!",
                        COUNT(*) FILTER (
                            WHERE accept_window_ends_at >= now()
                            AND accept_window_ends_at
                                < now() + make_interval(mins => $1::int)
                        ) AS "at_risk!"
                    FROM p1_dispatches
                    WHERE status = 'BROADCASTING'
                    "#,
                    at_risk_minutes as i32,
                )
                .fetch_one(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;

                // Equipment lifecycle distribution (Korean enum values).
                let equipment_row = sqlx::query!(
                    r#"
                    SELECT
                        COUNT(*) FILTER (WHERE status = '임대') AS "rented!",
                        COUNT(*) FILTER (WHERE status = '예비') AS "spare!",
                        COUNT(*) FILTER (WHERE status = '폐기') AS "scrapped!",
                        COUNT(*) FILTER (WHERE status = '대체') AS "replacement!",
                        COUNT(*) FILTER (WHERE status = '매각') AS "sold!"
                    FROM registry_equipment
                    "#,
                )
                .fetch_one(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;

                // Active substitutions (대차): not yet returned.
                let active_substitutions = sqlx::query_scalar!(
                    r#"SELECT COUNT(*) AS "count!"
                       FROM equipment_substitutions
                       WHERE returned_at IS NULL"#,
                )
                .fetch_one(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;

                // Pending approvals queue depth.
                let pending_approvals = sqlx::query_scalar!(
                    r#"SELECT COUNT(*) AS "count!"
                       FROM work_order_approval_steps
                       WHERE status = 'PENDING'"#,
                )
                .fetch_one(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;

                // Open support tickets (not yet resolved/closed).
                let open_support_tickets = sqlx::query_scalar!(
                    r#"SELECT COUNT(*) AS "count!"
                       FROM support_tickets
                       WHERE status IN ('OPEN', 'IN_PROGRESS', 'ON_HOLD')"#,
                )
                .fetch_one(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;

                // Mechanic utilization: active assignments per mechanic, top-N.
                // An assignment is "active" while its work order is not terminal.
                let mechanic_rows = sqlx::query!(
                    r#"
                    SELECT
                        a.mechanic_id AS "mechanic_id!",
                        u.display_name AS "display_name!",
                        COUNT(*) AS "active_assignments!"
                    FROM work_order_assignments a
                    JOIN work_orders w ON w.id = a.work_order_id
                    JOIN users u ON u.id = a.mechanic_id
                    WHERE w.status NOT IN (
                        'FINAL_COMPLETED', 'REJECTED', 'ARCHIVED', 'CANCELLED'
                    )
                    GROUP BY a.mechanic_id, u.display_name
                    ORDER BY COUNT(*) DESC, u.display_name ASC
                    LIMIT $1
                    "#,
                    top_mechanics,
                )
                .fetch_all(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;

                let mechanic_load = mechanic_rows
                    .into_iter()
                    .map(|row| OpsMechanicLoad {
                        mechanic_id: row.mechanic_id,
                        display_name: row.display_name,
                        active_assignments: clamp_count(row.active_assignments),
                    })
                    .collect();

                Ok(OpsSummary {
                    funnel: OpsFunnel {
                        received: clamp_count(funnel_row.received),
                        assigned: clamp_count(funnel_row.assigned),
                        in_progress: clamp_count(funnel_row.in_progress),
                        completed: clamp_count(funnel_row.completed),
                    },
                    aging_hours: query.aging_hours,
                    aging_work_orders: clamp_count(funnel_row.aging),
                    sla_breached: clamp_count(sla_row.breached),
                    sla_at_risk: clamp_count(sla_row.at_risk),
                    mechanic_load,
                    equipment_status: OpsEquipmentStatus {
                        rented: clamp_count(equipment_row.rented),
                        spare: clamp_count(equipment_row.spare),
                        scrapped: clamp_count(equipment_row.scrapped),
                        replacement: clamp_count(equipment_row.replacement),
                        sold: clamp_count(equipment_row.sold),
                    },
                    active_substitutions: clamp_count(active_substitutions),
                    pending_approvals: clamp_count(pending_approvals),
                    open_support_tickets: clamp_count(open_support_tickets),
                })
            })
        })
        .await
        .map_err(|err| KpiQueryError::Database(err.to_string()))?;

        Ok(summary)
    }
}

/// Saturating, sign-safe `COUNT(*)` (i64) → `u32` for serialized rollup fields.
/// Postgres counts are non-negative; the clamp is a defensive narrowing.
fn clamp_count(value: i64) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

/// Whether any of `table_names` exists in the connected database. Used to
/// degrade gracefully when an optional source schema (e.g. inspection tables)
/// has not been migrated. Returns the raw `sqlx::Error` so each caller maps it
/// into its own error type.
async fn any_table_exists(pool: &PgPool, table_names: &[&str]) -> Result<bool, sqlx::Error> {
    for table_name in table_names {
        let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(table_name)
            // rls-arming: ok to_regclass($1) queries pg_catalog metadata, not a tenant table (no org_id, no RLS)
            .fetch_one(pool)
            .await?;
        if exists {
            return Ok(true);
        }
    }
    Ok(false)
}

impl KpiQueryPort for PgKpiRepository {
    async fn query_kpis(&self, query: KpiQuery) -> Result<KpiReport, KpiQueryError> {
        self.query_kpis_inner(query).await
    }
}

impl OpsSummaryPort for PgKpiRepository {
    async fn ops_summary(&self, query: OpsSummaryQuery) -> Result<OpsSummary, KpiQueryError> {
        self.ops_summary_inner(query).await
    }
}

impl ReportingExportPort for PgKpiRepository {
    async fn export_daily_status(
        &self,
        query: ReportingExportQuery,
    ) -> Result<ExportedWorkbook, ReportingExportError> {
        self.export_daily_status_inner(query)
            .await
            .map_err(Into::into)
    }

    async fn export_work_diary(
        &self,
        query: ReportingExportQuery,
    ) -> Result<ExportedWorkbook, ReportingExportError> {
        self.export_work_diary_inner(query)
            .await
            .map_err(Into::into)
    }

    async fn export_kpi(
        &self,
        query: KpiExportQuery,
    ) -> Result<ExportedWorkbook, ReportingExportError> {
        self.export_kpi_inner(query).await
    }
}

impl WorkDiaryDraftPort for PgKpiRepository {
    async fn get_or_generate_work_diary(
        &self,
        query: WorkDiaryQuery,
    ) -> Result<WorkDiaryDraft, ReportingExportError> {
        self.get_or_generate_work_diary_inner(query)
            .await
            .map_err(Into::into)
    }

    async fn update_work_diary(
        &self,
        command: WorkDiaryUpdateCommand,
    ) -> Result<WorkDiaryDraft, ReportingExportError> {
        self.update_work_diary_inner(command)
            .await
            .map_err(Into::into)
    }

    async fn confirm_work_diary(
        &self,
        command: WorkDiaryConfirmCommand,
    ) -> Result<WorkDiaryDraft, ReportingExportError> {
        self.confirm_work_diary_inner(command)
            .await
            .map_err(Into::into)
    }
}

fn push_branch_scope_filter(builder: &mut QueryBuilder<Postgres>, branch_scope: &BranchScope) {
    push_branch_column_filter(builder, branch_scope, "w.branch_id");
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

fn push_requested_scope_filter(builder: &mut QueryBuilder<Postgres>, scope: KpiScope) {
    match scope {
        KpiScope::Company => {}
        KpiScope::Region(region_id) => {
            builder.push(" AND b.region_id = ");
            builder.push_bind(*region_id.as_uuid());
        }
        KpiScope::Branch(branch_id) => {
            builder.push(" AND w.branch_id = ");
            builder.push_bind(*branch_id.as_uuid());
        }
        KpiScope::Technician(user_id) => {
            builder.push(" AND primary_assignment.mechanic_id = ");
            builder.push_bind(*user_id.as_uuid());
        }
    }
}

fn push_requested_inspection_scope_filter(builder: &mut QueryBuilder<Postgres>, scope: KpiScope) {
    match scope {
        KpiScope::Company => {}
        KpiScope::Region(region_id) => {
            builder.push(" AND b.region_id = ");
            builder.push_bind(*region_id.as_uuid());
        }
        KpiScope::Branch(branch_id) => {
            builder.push(" AND s.branch_id = ");
            builder.push_bind(*branch_id.as_uuid());
        }
        KpiScope::Technician(user_id) => {
            builder.push(" AND s.mechanic_id = ");
            builder.push_bind(*user_id.as_uuid());
        }
    }
}

fn push_requested_p1_scope_filter(builder: &mut QueryBuilder<Postgres>, scope: KpiScope) {
    match scope {
        KpiScope::Company => {}
        KpiScope::Region(region_id) => {
            builder.push(" AND b.region_id = ");
            builder.push_bind(*region_id.as_uuid());
        }
        KpiScope::Branch(branch_id) => {
            builder.push(" AND d.branch_id = ");
            builder.push_bind(*branch_id.as_uuid());
        }
        // Technician rollups for P1 acceptance are derived in the domain from the
        // accepting/auto-assigned mechanic, so no SQL-side technician filter here.
        KpiScope::Technician(_) => {}
    }
}

fn p1_record_from_row(row: &sqlx::postgres::PgRow) -> Result<KpiP1Record, KpiQueryError> {
    Ok(KpiP1Record {
        dispatch_id: row.try_get("dispatch_id").map_err(row_error)?,
        branch_id: BranchId::from_uuid(row.try_get("branch_id").map_err(row_error)?),
        region_id: RegionId::from_uuid(row.try_get("region_id").map_err(row_error)?),
        technician_id: row
            .try_get::<Option<uuid::Uuid>, _>("technician_id")
            .map_err(row_error)?
            .map(UserId::from_uuid),
        accepted: row.try_get("accepted").map_err(row_error)?,
    })
}

fn record_from_row(row: &sqlx::postgres::PgRow) -> Result<KpiInputRecord, KpiQueryError> {
    let work_order_id = row
        .try_get::<uuid::Uuid, _>("work_order_id")
        .map_err(row_error)?;
    let branch_id = row
        .try_get::<uuid::Uuid, _>("branch_id")
        .map_err(row_error)?;
    let region_id = row
        .try_get::<uuid::Uuid, _>("region_id")
        .map_err(row_error)?;
    let technician_id = row
        .try_get::<Option<uuid::Uuid>, _>("technician_id")
        .map_err(row_error)?;
    let status_raw: String = row.try_get("status").map_err(row_error)?;
    let priority_raw: String = row.try_get("priority").map_err(row_error)?;
    let result_type_raw: String = row.try_get("result_type").map_err(row_error)?;
    Ok(KpiInputRecord {
        work_order_id,
        branch_id: BranchId::from_uuid(branch_id),
        region_id: RegionId::from_uuid(region_id),
        technician_id: technician_id.map(UserId::from_uuid),
        status: parse_kpi_status(work_order_id, &status_raw)?,
        priority: parse_kpi_priority(work_order_id, &priority_raw)?,
        result_type: parse_kpi_result_type(work_order_id, &result_type_raw)?,
        delay_reason: row.try_get("delay_reason").map_err(row_error)?,
        created_at: row.try_get("created_at").map_err(row_error)?,
        first_in_progress_at: row.try_get("first_in_progress_at").map_err(row_error)?,
        approved_at: row.try_get("approved_at").map_err(row_error)?,
        target_due_at: row.try_get("target_due_at").map_err(row_error)?,
    })
}

fn parse_kpi_status(
    work_order_id: uuid::Uuid,
    value: &str,
) -> Result<KpiWorkOrderStatus, KpiQueryError> {
    let status = WorkOrderStatus::from_db_str(value)
        .map_err(|error| kpi_enum_error(work_order_id, "status", value, error))?;
    Ok(match status {
        WorkOrderStatus::FinalCompleted => KpiWorkOrderStatus::FinalCompleted,
        _ => KpiWorkOrderStatus::Other,
    })
}

fn parse_kpi_priority(
    work_order_id: uuid::Uuid,
    value: &str,
) -> Result<KpiPriorityLevel, KpiQueryError> {
    let priority = PriorityLevel::from_db_str(value)
        .map_err(|error| kpi_enum_error(work_order_id, "priority", value, error))?;
    Ok(match priority {
        PriorityLevel::P1 => KpiPriorityLevel::P1,
        PriorityLevel::P2 => KpiPriorityLevel::P2,
        PriorityLevel::P3 => KpiPriorityLevel::P3,
        PriorityLevel::Outsource | PriorityLevel::Unset => KpiPriorityLevel::Other,
    })
}

fn parse_kpi_result_type(
    work_order_id: uuid::Uuid,
    value: &str,
) -> Result<KpiWorkResultType, KpiQueryError> {
    let result_type = WorkResultType::from_db_str(value)
        .map_err(|error| kpi_enum_error(work_order_id, "result_type", value, error))?;
    Ok(match result_type {
        WorkResultType::RevisitRequired => KpiWorkResultType::RevisitRequired,
        WorkResultType::Completed
        | WorkResultType::TemporaryAction
        | WorkResultType::Incomplete
        | WorkResultType::Unknown => KpiWorkResultType::Other,
    })
}

fn kpi_enum_error(
    work_order_id: uuid::Uuid,
    field: &'static str,
    value: &str,
    source: KernelError,
) -> KpiQueryError {
    KernelError::validation(format!(
        "invalid KPI work_order enum work_order_id={work_order_id} field={field} value={value:?}: {source}"
    ))
    .into()
}

fn inspection_record_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<KpiInspectionRecord, KpiQueryError> {
    Ok(KpiInspectionRecord {
        schedule_id: row.try_get("schedule_id").map_err(row_error)?,
        branch_id: BranchId::from_uuid(row.try_get("branch_id").map_err(row_error)?),
        region_id: RegionId::from_uuid(row.try_get("region_id").map_err(row_error)?),
        technician_id: UserId::from_uuid(row.try_get("technician_id").map_err(row_error)?),
        completed: row.try_get("completed").map_err(row_error)?,
    })
}

fn row_error(error: sqlx::Error) -> KpiQueryError {
    KpiQueryError::Database(error.to_string())
}

fn day_bounds(date: Date) -> (OffsetDateTime, OffsetDateTime) {
    let start = date.with_time(Time::MIDNIGHT).assume_utc();
    (start, start + Duration::days(1))
}

fn daily_status_row_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<DailyStatusRow, PgReportingError> {
    Ok(DailyStatusRow {
        request_date: row.try_get("request_date")?,
        site_name: row.try_get("site_name")?,
        management_no: row.try_get("management_no")?,
        model: row.try_get("model")?,
        vin: row.try_get("vin")?,
        symptom: row.try_get("symptom")?,
        mechanic_name: row.try_get("mechanic_name")?,
        scheduled_date: row.try_get("scheduled_date")?,
        completed_date: row.try_get("completed_date")?,
        result_note: row.try_get("result_note")?,
        priority: row.try_get("priority")?,
        status: row.try_get("status")?,
    })
}

fn periodic_inspection_row_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<PeriodicInspectionRow, PgReportingError> {
    Ok(PeriodicInspectionRow {
        site_name: row.try_get("site_name")?,
        vehicle_no: row.try_get("vehicle_no")?,
        management_no: row.try_get("management_no")?,
        model: row.try_get("model")?,
        serial_no: row.try_get("serial_no")?,
        issue: row.try_get("issue")?,
        inspection_period: row.try_get("inspection_period")?,
        note: row.try_get("note")?,
    })
}

fn action_entry_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<WorkDiaryActionEntry, PgReportingError> {
    Ok(WorkDiaryActionEntry {
        site_name: row.try_get("site_name")?,
        management_no: management_no_display(row.try_get::<Option<String>, _>("management_no")?)
            .unwrap_or_default(),
        diagnosis: row
            .try_get::<Option<String>, _>("diagnosis")?
            .unwrap_or_default(),
        action_taken: row
            .try_get::<Option<String>, _>("action_taken")?
            .unwrap_or_default(),
    })
}

fn draft_from_row(row: &sqlx::postgres::PgRow) -> Result<WorkDiaryDraft, PgReportingError> {
    let status_raw: String = row.try_get("status")?;
    let body: serde_json::Value = row.try_get("body")?;
    let confirmed_by: Option<uuid::Uuid> = row.try_get("confirmed_by")?;
    Ok(WorkDiaryDraft {
        id: row.try_get("id")?,
        date: row.try_get("diary_date")?,
        status: WorkDiaryStatus::from_db_str(&status_raw)?,
        body: serde_json::from_value(body)?,
        confirmed_by: confirmed_by.map(UserId::from_uuid),
        confirmed_at: row.try_get("confirmed_at")?,
    })
}

fn render_daily_status(report: &DailyStatusReport) -> Result<Vec<u8>, PgReportingError> {
    fill_template_bytes(
        DAILY_STATUS_TEMPLATE_BYTES,
        &DAILY_STATUS_TEMPLATE,
        &[
            SectionFill::new(
                DailyStatusSection::Results,
                status_rows_to_template(&report.results, SectionKind::Results),
            ),
            SectionFill::new(
                DailyStatusSection::Plans,
                status_rows_to_template(&report.plans, SectionKind::Plans),
            ),
            SectionFill::new(
                DailyStatusSection::PendingBacklog,
                status_rows_to_template(&report.pending_backlog, SectionKind::Pending),
            ),
            SectionFill::new(
                DailyStatusSection::PeriodicInspection,
                inspection_rows_to_template(&report.periodic_inspections),
            ),
        ],
    )
    .map_err(|error| PgReportingError::Workbook(error.to_string()))
}

/// Render the KPI rollups into a fresh single-sheet workbook. The KPI report has
/// no fixed Korean form to fill (unlike daily-status/work-diary), so this builds
/// the sheet from scratch: a header row from `KPI_EXPORT_HEADERS` followed by one
/// row per rollup produced by the pure `kpi_export_rows` shaping function.
fn render_kpi(report: &KpiReport) -> Result<Vec<u8>, PgReportingError> {
    let mut workbook = umya_spreadsheet::new_file();
    workbook
        .set_sheet_name(0, KPI_SHEET_NAME)
        .map_err(|error| PgReportingError::Workbook(error.to_string()))?;
    let sheet = workbook
        .sheet_by_name_mut(KPI_SHEET_NAME)
        .map_err(|error| PgReportingError::Workbook(error.to_string()))?;

    for (column, header) in (1u32..).zip(KPI_EXPORT_HEADERS.iter()) {
        write_cell(sheet, column, 1, *header);
    }
    for (row_number, row) in (2u32..).zip(kpi_export_rows(report)) {
        for (column, value) in (1u32..).zip(row) {
            write_cell(sheet, column, row_number, value);
        }
    }

    let mut output = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&workbook, &mut output)
        .map_err(|error| PgReportingError::Workbook(error.to_string()))?;
    Ok(output)
}

#[derive(Clone, Copy)]
enum SectionKind {
    Results,
    Plans,
    Pending,
}

fn status_rows_to_template(rows: &[DailyStatusRow], section: SectionKind) -> Vec<TemplateRow> {
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let mut cells = vec![
                CellWrite::text(1, "미"),
                CellWrite::text(2, (index + 1).to_string()),
                CellWrite::text(3, date_text(row.request_date)),
                CellWrite::text(4, row.site_name.clone()),
                CellWrite::text(
                    5,
                    management_no_display(row.management_no.clone()).unwrap_or_default(),
                ),
                CellWrite::text(6, row.model.clone().unwrap_or_default()),
                CellWrite::text(7, row.vin.clone().unwrap_or_default()),
                CellWrite::text(8, row.symptom.clone()),
                CellWrite::text(9, row.mechanic_name.clone().unwrap_or_default()),
                CellWrite::text(10, date_text(row.scheduled_date)),
                CellWrite::text(11, date_text(row.completed_date)),
                CellWrite::text(12, row.result_note.clone().unwrap_or_default()),
            ];
            match section {
                SectionKind::Results | SectionKind::Plans => {
                    cells.push(CellWrite::text(13, priority_warning(&row.priority)));
                }
                SectionKind::Pending => {
                    cells.push(CellWrite::text(13, row.status.clone()));
                    cells.push(CellWrite::text(
                        14,
                        if row.priority == "OUTSOURCE" {
                            "외"
                        } else {
                            ""
                        },
                    ));
                }
            }
            TemplateRow::new(cells)
        })
        .collect()
}

fn inspection_rows_to_template(rows: &[PeriodicInspectionRow]) -> Vec<TemplateRow> {
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            TemplateRow::new([
                CellWrite::text(2, (index + 1).to_string()),
                CellWrite::text(3, row.site_name.clone()),
                CellWrite::text(4, row.vehicle_no.clone().unwrap_or_default()),
                CellWrite::text(
                    5,
                    management_no_display(row.management_no.clone()).unwrap_or_default(),
                ),
                CellWrite::text(6, row.model.clone().unwrap_or_default()),
                CellWrite::text(7, row.serial_no.clone().unwrap_or_default()),
                CellWrite::text(8, row.issue.clone()),
                CellWrite::text(9, row.inspection_period.clone().unwrap_or_default()),
                CellWrite::text(12, row.note.clone().unwrap_or_default()),
            ])
        })
        .collect()
}

fn render_work_diary(date: Date, body: &WorkDiaryBody) -> Result<Vec<u8>, PgReportingError> {
    let mut workbook =
        umya_spreadsheet::reader::xlsx::read_reader(Cursor::new(WORK_DIARY_TEMPLATE_BYTES), true)
            .map_err(|error| PgReportingError::Workbook(error.to_string()))?;
    let sheet_name = work_diary_sheet_name(date);
    workbook
        .set_sheet_name(0, sheet_name.clone())
        .map_err(|error| PgReportingError::Workbook(error.to_string()))?;
    let next_date = date
        .next_day()
        .ok_or_else(|| KernelError::validation("work diary date overflow"))?;
    let sheet = workbook
        .sheet_by_name_mut(&sheet_name)
        .map_err(|error| PgReportingError::Workbook(error.to_string()))?;

    write_cell(sheet, 2, 3, format!("작성일자 : {}", dotted_date(date)));
    write_cell(
        sheet,
        2,
        5,
        format!(" ( {} ) 특  기  사  항", korean_month_day(date)),
    );
    write_cell(
        sheet,
        2,
        8,
        format!(" 전 일 진 행 업 무 ({})", korean_month_day(date)),
    );
    write_cell(
        sheet,
        6,
        8,
        format!("금 일 예 정 업 무 ({})", korean_month_day(next_date)),
    );
    write_cell(sheet, 2, 10, body.previous_results.clone());
    write_cell(sheet, 6, 10, body.today_plans.clone());

    for row in 15..=31 {
        write_cell(sheet, 2, row, "");
    }
    for (offset, entry) in body.urgent_actions.iter().take(17).enumerate() {
        let row = 15
            + u32::try_from(offset).map_err(|_| {
                PgReportingError::Workbook("too many urgent action rows".to_owned())
            })?;
        write_cell(
            sheet,
            2,
            row,
            format!(
                "▶ {} {}\n   1) 점검 : {}\n   2) 조치 : {}",
                entry.site_name, entry.management_no, entry.diagnosis, entry.action_taken
            ),
        );
    }

    let mut output = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(&workbook, &mut output)
        .map_err(|error| PgReportingError::Workbook(error.to_string()))?;
    Ok(output)
}

fn write_cell(
    sheet: &mut umya_spreadsheet::Worksheet,
    column: u32,
    row: u32,
    value: impl Into<String>,
) {
    sheet.cell_mut((column, row)).set_value(value);
}

fn diary_results_text(rows: &[DailyStatusRow]) -> String {
    if rows.is_empty() {
        return "완료 업무 없음".to_owned();
    }
    rows.iter()
        .map(|row| {
            format!(
                "▶ {} {} {}",
                row.site_name,
                management_no_display(row.management_no.clone()).unwrap_or_default(),
                row.symptom
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn diary_plans_text(rows: &[DailyStatusRow]) -> String {
    if rows.is_empty() {
        return "예정 업무 없음".to_owned();
    }
    rows.iter()
        .map(|row| {
            let plan_text = row
                .result_note
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(&row.symptom);
            format!(
                "▶ {} {} {}",
                row.site_name,
                management_no_display(row.management_no.clone()).unwrap_or_default(),
                plan_text
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn audit_event(
    action: &'static str,
    actor: UserId,
    target_type: &'static str,
    target_id: uuid::Uuid,
    branch_id: Option<BranchId>,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, PgReportingError> {
    let event = AuditEvent::new(
        Some(actor),
        AuditAction::new(action)?,
        target_type,
        target_id.to_string(),
        trace,
        occurred_at,
    );
    Ok(if let Some(branch_id) = branch_id {
        event.with_branch(branch_id)
    } else {
        event
    })
}

fn single_branch(scope: &BranchScope) -> Option<BranchId> {
    match scope {
        BranchScope::All => None,
        BranchScope::Branches(branches) if branches.len() == 1 => branches.iter().next().copied(),
        BranchScope::Branches(_) => None,
    }
}

fn scope_key(scope: &BranchScope) -> String {
    match scope {
        BranchScope::All => "ALL".to_owned(),
        BranchScope::Branches(branches) => branches
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(","),
    }
}

fn management_no_display(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty()).map(|value| {
        if value.starts_with('#') {
            value
        } else {
            format!("#{value}")
        }
    })
}

fn priority_warning(priority: &str) -> String {
    match priority {
        "P1" => "Priority#1".to_owned(),
        "P2" => "Priority#2".to_owned(),
        "P3" => "Priority#3".to_owned(),
        "OUTSOURCE" => "OUTSOURCE".to_owned(),
        _ => String::new(),
    }
}

fn date_text(value: Option<Date>) -> String {
    value.map(iso_date).unwrap_or_default()
}

fn iso_date(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

fn dotted_date(date: Date) -> String {
    format!(
        "{:04}. {:02}. {:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

fn korean_month_day(date: Date) -> String {
    format!("{:02}월 {:02}일", u8::from(date.month()), date.day())
}

fn work_diary_sheet_name(date: Date) -> String {
    korean_month_day(date)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work_order_id() -> uuid::Uuid {
        uuid::Uuid::from_u128(0x6842_defe)
    }

    #[test]
    fn kpi_row_parser_accepts_known_work_order_enums() {
        assert_eq!(
            parse_kpi_status(work_order_id(), "FINAL_COMPLETED").unwrap(),
            KpiWorkOrderStatus::FinalCompleted
        );
        assert_eq!(
            parse_kpi_priority(work_order_id(), "P2").unwrap(),
            KpiPriorityLevel::P2
        );
        assert_eq!(
            parse_kpi_result_type(work_order_id(), "REVISIT_REQUIRED").unwrap(),
            KpiWorkResultType::RevisitRequired
        );
    }

    #[test]
    fn kpi_row_parser_rejects_unknown_status_with_field_value_context() {
        let error = parse_kpi_status(work_order_id(), "DONE-ish").unwrap_err();
        let message = error.to_string();

        assert!(message.contains("work_order_id"));
        assert!(message.contains("status"));
        assert!(message.contains("DONE-ish"));
    }

    #[test]
    fn kpi_row_parser_rejects_unknown_priority_with_field_value_context() {
        let error = parse_kpi_priority(work_order_id(), "P0").unwrap_err();
        let message = error.to_string();

        assert!(message.contains("work_order_id"));
        assert!(message.contains("priority"));
        assert!(message.contains("P0"));
    }

    #[test]
    fn kpi_row_parser_rejects_unknown_result_type_with_field_value_context() {
        let error = parse_kpi_result_type(work_order_id(), "PARTIAL").unwrap_err();
        let message = error.to_string();

        assert!(message.contains("work_order_id"));
        assert!(message.contains("result_type"));
        assert!(message.contains("PARTIAL"));
    }
}
