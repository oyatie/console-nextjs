#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Cursor;

use console_kernel_core::{BranchId, BranchScope, OrgId, TraceContext, UserId};
use console_platform_excel::umya_spreadsheet::{self, Workbook, Worksheet};
use console_platform_test_support::{seed_branch, seed_user};
use console_reporting_adapter_postgres::PgReportingRepository;
use console_reporting_application::{
    ReportingExportPort, ReportingExportQuery, WorkDiaryActionEntry, WorkDiaryBody,
    WorkDiaryConfirmCommand, WorkDiaryDraftPort, WorkDiaryQuery, WorkDiaryUpdateCommand,
};
use sqlx::PgPool;
use time::{Date, Duration, OffsetDateTime, Time, macros::datetime};

const EXPORT_DATE: Date = time::macros::date!(2026 - 06 - 12);
const EXPORT_START: OffsetDateTime = datetime!(2026-06-12 00:00 UTC);

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn daily_status_export_maps_live_work_orders_to_template_sections(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_export_dataset(&pool).await;
        let repo = PgReportingRepository::new(pool.clone());

        let export = repo
            .export_daily_status(export_query(seeded.actor, seeded.branch))
            .await
            .unwrap();

        assert_eq!(
            export.file_name, "daily-status-2026-06-12.xlsx",
            "download filename should be deterministic"
        );
        let book = load_workbook(&export.bytes);
        let ws = sheet(&book, "6월05일");

        assert_eq!(cell_value(ws, 4, 4), "완료현장");
        assert_eq!(cell_value(ws, 5, 4), "#900");
        assert_eq!(cell_value(ws, 8, 4), "충전불가");
        assert_eq!(cell_value(ws, 9, 4), "정비완료");
        assert_eq!(cell_value(ws, 11, 4), "2026-06-12");
        assert_eq!(cell_value(ws, 12, 4), "충전기 교체 완료");
        assert_eq!(cell_value(ws, 13, 4), "Priority#1");

        assert_eq!(cell_value(ws, 4, 26), "계획현장");
        assert_eq!(cell_value(ws, 5, 26), "#901");
        assert_eq!(cell_value(ws, 8, 26), "유압 누유");
        assert_eq!(cell_value(ws, 9, 26), "정비계획");
        assert_eq!(cell_value(ws, 10, 26), "2026-06-12");
        assert_eq!(cell_value(ws, 13, 26), "Priority#2");

        assert_eq!(cell_value(ws, 4, 46), "미결현장-01");
        assert_eq!(cell_value(ws, 5, 46), "#1001");
        assert_eq!(cell_value(ws, 13, 46), "ASSIGNED");
        assert_eq!(cell_value(ws, 4, 78), "미결현장-33");
        assert_eq!(cell_value(ws, 5, 78), "#1033");
        assert_eq!(
            cell_value(ws, 4, 79),
            "계획현장",
            "planned-but-open work orders remain in the unbounded backlog"
        );
        assert_eq!(
            cell_value(ws, 2, 81),
            "4. 정기검사",
            "34 open rows should force section 4 below the original template range"
        );
        assert_eq!(cell_value(ws, 3, 83), "검사현장");
        assert_eq!(cell_value(ws, 4, 83), "검사차량-904");
        assert_eq!(cell_value(ws, 5, 83), "#904");
        assert_eq!(cell_value(ws, 6, 83), "GTS30D");
        assert_eq!(cell_value(ws, 7, 83), "VIN-904");
        assert_eq!(cell_value(ws, 8, 83), "정기검사 예정");
        assert_eq!(cell_value(ws, 9, 83), "월간 / 2026-06-12");
        assert_eq!(cell_value(ws, 12, 83), "월간 안전점검");

        let export_log_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM excel_export_logs WHERE export_kind = 'daily_status'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(export_log_count, 1);
        let audit_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE action = 'export.daily_status'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audit_count, 1);
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn work_diary_draft_can_be_generated_edited_confirmed_and_exported(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_export_dataset(&pool).await;
        let repo = PgReportingRepository::new(pool.clone());

        let generated = repo
            .get_or_generate_work_diary(work_diary_query(seeded.actor, seeded.branch))
            .await
            .unwrap();
        assert_eq!(generated.status.as_str(), "DRAFT");
        assert!(generated.body.previous_results.contains("완료현장"));
        assert!(generated.body.today_plans.contains("익일예정"));
        assert_eq!(
            generated.body.urgent_actions.first().unwrap().diagnosis,
            "배터리 전압 저하"
        );

        let edited_body = WorkDiaryBody {
            previous_results: "편집된 전일실적".to_owned(),
            today_plans: "편집된 금일예정".to_owned(),
            urgent_actions: vec![WorkDiaryActionEntry {
                site_name: "편집현장".to_owned(),
                management_no: "#999".to_owned(),
                diagnosis: "편집 점검".to_owned(),
                action_taken: "편집 조치".to_owned(),
            }],
            source_notes: generated.body.source_notes.clone(),
        };

        let edited = repo
            .update_work_diary(WorkDiaryUpdateCommand {
                actor: seeded.actor,
                date: EXPORT_DATE,
                branch_scope: BranchScope::single(seeded.branch),
                body: edited_body,
                trace: TraceContext::generate(),
                occurred_at: EXPORT_START + Duration::hours(18),
            })
            .await
            .unwrap();
        assert_eq!(edited.body.previous_results, "편집된 전일실적");

        let confirmed = repo
            .confirm_work_diary(WorkDiaryConfirmCommand {
                actor: seeded.actor,
                date: EXPORT_DATE,
                branch_scope: BranchScope::single(seeded.branch),
                trace: TraceContext::generate(),
                occurred_at: EXPORT_START + Duration::hours(19),
            })
            .await
            .unwrap();
        assert_eq!(confirmed.status.as_str(), "CONFIRMED");

        let export = repo
            .export_work_diary(export_query(seeded.actor, seeded.branch))
            .await
            .unwrap();
        assert_eq!(export.file_name, "work-diary-2026-06-12.xlsx");
        let book = load_workbook(&export.bytes);
        let ws = sheet(&book, "06월 12일");
        assert!(cell_value(ws, 2, 3).contains("2026. 06. 12"));
        assert_eq!(cell_value(ws, 2, 10), "편집된 전일실적");
        assert_eq!(cell_value(ws, 6, 10), "편집된 금일예정");
        assert!(cell_value(ws, 2, 15).contains("▶ 편집현장 #999"));
        assert!(cell_value(ws, 2, 15).contains("1) 점검 : 편집 점검"));
        assert!(cell_value(ws, 2, 15).contains("2) 조치 : 편집 조치"));
        assert!(
            book.sheet_by_name("2026.05월(계획)").is_ok(),
            "monthly plan calendar sheet must pass through untouched"
        );

        let actions: Vec<String> =
            sqlx::query_scalar("SELECT action FROM audit_events ORDER BY occurred_at, action")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(actions.contains(&"work_diary.generate".to_owned()));
        assert!(actions.contains(&"work_diary.update".to_owned()));
        assert!(actions.contains(&"work_diary.confirm".to_owned()));
        assert!(actions.contains(&"export.work_diary".to_owned()));
    })
    .await;
}

/// Regression test for finding #6 (correctness-data-concurrency review):
/// company-scope (BranchScope::All) export logs carry NULL branch_id with a
/// non-empty scope_key ("ALL"). This is intentional — scope_key is the
/// authoritative rollup discriminator; branch_id is a convenience FK that is
/// NULL for company/region rollups by design. The export must still be fully
/// audited (audit_events row present).
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn company_scope_export_log_persists_null_branch_id_with_authoritative_scope_key(
    pool: PgPool,
) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        // Seed a minimal branch so we can create the actor user; the export itself
        // is company-wide (BranchScope::All) and does not filter to this branch.
        let region_id: uuid::Uuid =
            sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
                .bind(format!("전국-{}", uuid::Uuid::new_v4()))
                .bind(*OrgId::knl().as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        let branch_id: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(region_id)
        .bind(format!("본사-{}", uuid::Uuid::new_v4()))
        .bind(*OrgId::knl().as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        let actor = UserId::new();
        sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
            .bind(*actor.as_uuid())
            .bind("총괄임원")
            .bind(Vec::from(["EXECUTIVE"]))
            .bind(*OrgId::knl().as_uuid())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
            .bind(*actor.as_uuid())
            .bind(branch_id)
            .bind(*OrgId::knl().as_uuid())
            .execute(&pool)
            .await
            .unwrap();

        let repo = PgReportingRepository::new(pool.clone());

        // Company-scope export: BranchScope::All → scope_key="ALL", branch_id=NULL
        let export = repo
            .export_daily_status(ReportingExportQuery {
                actor,
                date: EXPORT_DATE,
                branch_scope: BranchScope::All,
                trace: TraceContext::generate(),
                occurred_at: EXPORT_START + Duration::hours(9),
            })
            .await
            .unwrap();

        assert_eq!(
            export.file_name, "daily-status-2026-06-12.xlsx",
            "filename should be deterministic regardless of scope"
        );

        // The export_log row must have NULL branch_id and non-empty scope_key="ALL"
        let (logged_branch_id, logged_scope_key): (Option<uuid::Uuid>, String) = sqlx::query_as(
            "SELECT branch_id, scope_key FROM excel_export_logs WHERE export_kind = 'daily_status'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert!(
            logged_branch_id.is_none(),
            "company-scope export log must carry NULL branch_id (rollup exception, finding #6)"
        );
        assert!(
            !logged_scope_key.is_empty(),
            "scope_key must be non-empty (it is the authoritative scope discriminator)"
        );
        assert_eq!(
            logged_scope_key, "ALL",
            "company rollup scope_key must be 'ALL'"
        );

        // Audit coverage: the export must still be recorded in audit_events
        let audit_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE action = 'export.daily_status'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            audit_count, 1,
            "company-scope export must produce exactly one audit_events row"
        );
    })
    .await;
}

fn export_query(actor: UserId, branch: BranchId) -> ReportingExportQuery {
    ReportingExportQuery {
        actor,
        date: EXPORT_DATE,
        branch_scope: BranchScope::single(branch),
        trace: TraceContext::generate(),
        occurred_at: EXPORT_START + Duration::hours(17),
    }
}

fn work_diary_query(actor: UserId, branch: BranchId) -> WorkDiaryQuery {
    WorkDiaryQuery {
        actor,
        date: EXPORT_DATE,
        branch_scope: BranchScope::single(branch),
        trace: TraceContext::generate(),
        occurred_at: EXPORT_START + Duration::hours(16),
    }
}

fn load_workbook(bytes: &[u8]) -> Workbook {
    umya_spreadsheet::reader::xlsx::read_reader(Cursor::new(bytes), true)
        .expect("exported workbook should be readable")
}

fn sheet<'a>(book: &'a Workbook, name: &str) -> &'a Worksheet {
    book.sheet_by_name(name)
        .unwrap_or_else(|err| panic!("sheet {name} should exist: {err}"))
}

fn cell_value(ws: &Worksheet, col: u32, row: u32) -> String {
    ws.cell((col, row))
        .map(|cell| cell.value().to_string())
        .unwrap_or_default()
}

struct SeededExportDataset {
    branch: BranchId,
    actor: UserId,
}

async fn seed_export_dataset(pool: &PgPool) -> SeededExportDataset {
    let branch = seed_branch(pool, "수도권", "서울").await;
    let actor = seed_user(pool, "관리자", "ADMIN", branch).await;
    let completed_tech = seed_user(pool, "정비완료", "MECHANIC", branch).await;
    let planned_tech = seed_user(pool, "정비계획", "MECHANIC", branch).await;

    let completed_equipment = seed_equipment(pool, branch, "900", "완료현장", "CMPAA-0900").await;
    let planned_equipment = seed_equipment(pool, branch, "901", "계획현장", "PLNAA-0901").await;
    let completed_work_order = seed_work_order(
        pool,
        WorkOrderSeed {
            branch,
            equipment: completed_equipment,
            requested_by: actor,
            request_no: "20260612-001",
            status: "FINAL_COMPLETED",
            priority: "P1",
            symptom: "충전불가",
            diagnosis: Some("배터리 전압 저하"),
            action_taken: Some("충전기 교체 완료"),
            report_submitted_by: Some(completed_tech),
            report_submitted_at: Some(EXPORT_START + Duration::hours(14)),
            target_due_at: Some(EXPORT_START + Duration::hours(15)),
            created_at: EXPORT_START + Duration::hours(8),
            approved_at: Some(EXPORT_START + Duration::hours(16)),
        },
    )
    .await;
    seed_assignment(
        pool,
        completed_work_order,
        completed_tech,
        EXPORT_START + Duration::hours(9),
    )
    .await;
    seed_approval(
        pool,
        completed_work_order,
        actor,
        EXPORT_START + Duration::hours(16),
    )
    .await;

    let planned_work_order = seed_work_order(
        pool,
        WorkOrderSeed {
            branch,
            equipment: planned_equipment,
            requested_by: actor,
            request_no: "20260612-002",
            status: "ASSIGNED",
            priority: "P2",
            symptom: "유압 누유",
            diagnosis: None,
            action_taken: None,
            report_submitted_by: None,
            report_submitted_at: None,
            target_due_at: Some(EXPORT_START + Duration::hours(10)),
            created_at: EXPORT_START + Duration::hours(7),
            approved_at: None,
        },
    )
    .await;
    seed_assignment(
        pool,
        planned_work_order,
        planned_tech,
        EXPORT_START + Duration::hours(8),
    )
    .await;
    seed_daily_plan(
        pool,
        branch,
        planned_tech,
        EXPORT_DATE,
        planned_work_order,
        "계획현장 유압 누유 점검",
    )
    .await;

    seed_daily_plan(
        pool,
        branch,
        planned_tech,
        EXPORT_DATE.next_day().unwrap(),
        planned_work_order,
        "익일예정 예방점검",
    )
    .await;

    let inspection_equipment = seed_equipment(pool, branch, "904", "검사현장", "INSPQ-0904").await;
    seed_inspection_schedule(
        pool,
        branch,
        inspection_equipment,
        planned_tech,
        actor,
        EXPORT_DATE,
        "월간 안전점검",
    )
    .await;

    for index in 1..=33 {
        let management_no = format!("{}", 1000 + index);
        let site_name = format!("미결현장-{index:02}");
        let equipment_no = format!("PND{index:02}-{index:04}");
        let equipment =
            seed_equipment(pool, branch, &management_no, &site_name, &equipment_no).await;
        let work_order = seed_work_order(
            pool,
            WorkOrderSeed {
                branch,
                equipment,
                requested_by: actor,
                request_no: Box::leak(format!("20260611-{index:03}").into_boxed_str()),
                status: "ASSIGNED",
                priority: "P3",
                symptom: Box::leak(format!("미결 증상 {index:02}").into_boxed_str()),
                diagnosis: None,
                action_taken: None,
                report_submitted_by: None,
                report_submitted_at: None,
                target_due_at: Some(EXPORT_START + Duration::days(1)),
                created_at: EXPORT_START - Duration::days(1) + Duration::minutes(index),
                approved_at: None,
            },
        )
        .await;
        seed_assignment(
            pool,
            work_order,
            planned_tech,
            EXPORT_START + Duration::minutes(index),
        )
        .await;
    }

    SeededExportDataset { branch, actor }
}

async fn seed_equipment(
    pool: &PgPool,
    branch: BranchId,
    management_no: &str,
    site_name: &str,
    equipment_no: &str,
) -> uuid::Uuid {
    let customer_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(format!("고객-{site_name}"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(customer_id)
    .bind(site_name)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no, model, vin,
            vehicle_registration_no, manufacturer_code, kind_code, power_code, status, specification, ton_text,
            source_sheet, source_row, updated_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5, 'GTS30D', $6, $7,
                'GLD', 'FBR', 'BATTERY', '임대', '입식', '3톤', 'exports', 1, now(), $8)
        RETURNING id
        "#,
    )
    .bind(*branch.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(equipment_no)
    .bind(management_no)
    .bind(format!("VIN-{management_no}"))
    .bind(format!("검사차량-{management_no}"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_inspection_schedule(
    pool: &PgPool,
    branch: BranchId,
    equipment: uuid::Uuid,
    mechanic: UserId,
    actor: UserId,
    due_date: Date,
    note: &str,
) -> uuid::Uuid {
    sqlx::query_scalar(
        r#"
        INSERT INTO regular_inspection_schedules (
            branch_id, equipment_id, mechanic_id, cycle, interval_days, due_date,
            status, note, created_by, created_at, updated_at, org_id
        )
        VALUES ($1, $2, $3, 'MONTHLY', 30, $4, 'SCHEDULED', $5, $6, $7, $7, $8)
        RETURNING id
        "#,
    )
    .bind(*branch.as_uuid())
    .bind(equipment)
    .bind(*mechanic.as_uuid())
    .bind(due_date)
    .bind(note)
    .bind(*actor.as_uuid())
    .bind(EXPORT_START)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

struct WorkOrderSeed<'a> {
    branch: BranchId,
    equipment: uuid::Uuid,
    requested_by: UserId,
    request_no: &'a str,
    status: &'a str,
    priority: &'a str,
    symptom: &'a str,
    diagnosis: Option<&'a str>,
    action_taken: Option<&'a str>,
    report_submitted_by: Option<UserId>,
    report_submitted_at: Option<OffsetDateTime>,
    target_due_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    approved_at: Option<OffsetDateTime>,
}

async fn seed_work_order(pool: &PgPool, seed: WorkOrderSeed<'_>) -> uuid::Uuid {
    let (customer_id, site_id): (uuid::Uuid, uuid::Uuid) =
        sqlx::query_as("SELECT customer_id, site_id FROM registry_equipment WHERE id = $1")
            .bind(seed.equipment)
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query_scalar(
        r#"
        INSERT INTO work_orders (
            request_no, branch_id, equipment_id, customer_id, site_id, requested_by,
            status, priority, symptom, target_due_at, result_type, diagnosis, action_taken,
            report_submitted_by, report_submitted_at, created_at, updated_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6,
                $7, $8, $9, $10, 'COMPLETED', $11, $12,
                $13, $14, $15, $16, $17)
        RETURNING id
        "#,
    )
    .bind(seed.request_no)
    .bind(*seed.branch.as_uuid())
    .bind(seed.equipment)
    .bind(customer_id)
    .bind(site_id)
    .bind(*seed.requested_by.as_uuid())
    .bind(seed.status)
    .bind(seed.priority)
    .bind(seed.symptom)
    .bind(seed.target_due_at)
    .bind(seed.diagnosis)
    .bind(seed.action_taken)
    .bind(seed.report_submitted_by.map(|id| *id.as_uuid()))
    .bind(seed.report_submitted_at)
    .bind(seed.created_at)
    .bind(seed.approved_at.unwrap_or(seed.created_at))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_assignment(
    pool: &PgPool,
    work_order_id: uuid::Uuid,
    tech: UserId,
    assigned_at: OffsetDateTime,
) {
    sqlx::query(
        "INSERT INTO work_order_assignments (work_order_id, mechanic_id, role, assigned_at, org_id) VALUES ($1, $2, 'PRIMARY', $3, $4)",
    )
    .bind(work_order_id)
    .bind(*tech.as_uuid())
    .bind(assigned_at)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_approval(
    pool: &PgPool,
    work_order_id: uuid::Uuid,
    approver: UserId,
    approved_at: OffsetDateTime,
) {
    sqlx::query(
        r#"
        INSERT INTO work_order_approval_steps (
            work_order_id, step_order, role, approver_id, status,
            requested_at, approved_at, approved_by_id, org_id
        )
        VALUES ($1, 3, 'EXECUTIVE', $2, 'APPROVED', $3, $3, $2, $4)
        "#,
    )
    .bind(work_order_id)
    .bind(*approver.as_uuid())
    .bind(approved_at)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_daily_plan(
    pool: &PgPool,
    branch: BranchId,
    mechanic: UserId,
    date: Date,
    work_order_id: uuid::Uuid,
    description: &str,
) {
    let plan_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO daily_work_plans (
            branch_id, mechanic_id, plan_date, status, requested_at, reviewed_at, confirmed_at, org_id
        )
        VALUES ($1, $2, $3, 'FINAL_CONFIRMED', $4, $4, $4, $5)
        RETURNING id
        "#,
    )
    .bind(*branch.as_uuid())
    .bind(*mechanic.as_uuid())
    .bind(date)
    .bind(date.with_time(Time::MIDNIGHT).assume_utc())
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO daily_work_plan_items (plan_id, work_order_id, description, sort_order, org_id) VALUES ($1, $2, $3, 1, $4)",
    )
    .bind(plan_id)
    .bind(work_order_id)
    .bind(description)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
}
