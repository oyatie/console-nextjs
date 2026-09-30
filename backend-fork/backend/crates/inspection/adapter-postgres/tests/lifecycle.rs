#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_inspection_adapter_postgres::PgInspectionStore;
use console_inspection_application::{
    CompleteInspectionRoundCommand, CreateInspectionScheduleCommand, ListInspectionSchedulesQuery,
    ListMyInspectionSchedulesQuery,
};
use console_inspection_domain::{InspectionCycle, InspectionRoundOutcome};
use console_kernel_core::{BranchId, BranchScope, EquipmentId, OrgId, TraceContext, UserId};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime, macros::datetime};

const NOW: OffsetDateTime = datetime!(2026-06-12 09:00 UTC);

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn schedule_lifecycle_requires_prevention_mechanic_and_audits_completion(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch = seed_branch(&pool, "수도권", "서울").await;
        let admin = seed_user(&pool, "관리자", "ADMIN", Some("관리"), branch).await;
        let prevention_mechanic = seed_user(&pool, "예방기사", "MECHANIC", Some("예방"), branch).await;
        let repair_mechanic = seed_user(&pool, "정비기사", "MECHANIC", Some("정비"), branch).await;
        let equipment = seed_equipment(&pool, branch, "904", "검사현장", "INSPQ-0904").await;
        let store = PgInspectionStore::new(pool.clone());

        let rejected = store
            .create_schedule(CreateInspectionScheduleCommand {
                actor: admin,
                branch_id: branch,
                equipment_id: equipment,
                mechanic_id: repair_mechanic,
                cycle: InspectionCycle::Monthly,
                interval_days: 30,
                due_date: time::macros::date!(2026 - 06 - 12),
                note: Some("월간 안전점검".to_owned()),
                trace: TraceContext::generate(),
                occurred_at: NOW,
            })
            .await
            .unwrap_err();
        assert!(
            rejected.to_string().contains("prevention"),
            "assignment to a non-prevention mechanic should fail: {rejected}"
        );

        let schedule = store
            .create_schedule(CreateInspectionScheduleCommand {
                actor: admin,
                branch_id: branch,
                equipment_id: equipment,
                mechanic_id: prevention_mechanic,
                cycle: InspectionCycle::Monthly,
                interval_days: 30,
                due_date: time::macros::date!(2026 - 06 - 12),
                note: Some("월간 안전점검".to_owned()),
                trace: TraceContext::generate(),
                occurred_at: NOW,
            })
            .await
            .unwrap();
        assert_eq!(schedule.branch_id, branch);
        assert_eq!(schedule.mechanic_id, prevention_mechanic);
        assert_eq!(schedule.completed_at, None);

        let round = store
            .complete_round(CompleteInspectionRoundCommand {
                actor: prevention_mechanic,
                schedule_id: schedule.id,
                outcome: InspectionRoundOutcome::FollowUpRequired,
                completed_at: NOW + Duration::hours(2),
                findings: "브레이크 정상".to_owned(),
                note: Some("배터리 보충 권고".to_owned()),
                trace: TraceContext::generate(),
                occurred_at: NOW + Duration::hours(2),
            })
            .await
            .unwrap();

        assert_eq!(round.schedule_id, schedule.id);
        assert_eq!(round.mechanic_id, prevention_mechanic);
        assert_eq!(round.outcome, InspectionRoundOutcome::FollowUpRequired);
        assert_eq!(round.findings, "브레이크 정상");

        let completed_at: Option<OffsetDateTime> =
            sqlx::query_scalar("SELECT completed_at FROM regular_inspection_schedules WHERE id = $1")
                .bind(*schedule.id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(completed_at, Some(NOW + Duration::hours(2)));

        let round_audit_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE action = 'inspection.round.complete' AND target_id = $1",
        )
        .bind(round.id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(round_audit_count, 1);

        let (schedule_audit_count, before_snap, after_snap): (
            i64,
            Option<serde_json::Value>,
            Option<serde_json::Value>,
        ) = sqlx::query_as(
            r#"
            SELECT COUNT(*) OVER () AS count, before_snap, after_snap
            FROM audit_events
            WHERE action = 'inspection.schedule.complete'
              AND target_id = $1
            "#,
        )
        .bind(schedule.id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(schedule_audit_count, 1);
        assert_eq!(
            before_snap.unwrap()["status"],
            serde_json::Value::String("SCHEDULED".to_owned())
        );
        assert_eq!(
            after_snap.unwrap()["status"],
            serde_json::Value::String("COMPLETED".to_owned())
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn due_schedule_listing_respects_branch_scope(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let branch_a = seed_branch(&pool, "수도권", "서울").await;
        let branch_b = seed_branch(&pool, "충청", "천안").await;
        let admin_a = seed_user(&pool, "관리자A", "ADMIN", Some("관리"), branch_a).await;
        let mechanic_a = seed_user(&pool, "예방A", "MECHANIC", Some("예방"), branch_a).await;
        let mechanic_b = seed_user(&pool, "예방B", "MECHANIC", Some("예방"), branch_b).await;
        let equipment_a = seed_equipment(&pool, branch_a, "904", "서울현장", "INSPA-0904").await;
        let equipment_b = seed_equipment(&pool, branch_b, "905", "천안현장", "INSPB-0905").await;
        let store = PgInspectionStore::new(pool.clone());

        for (branch, equipment, mechanic) in [
            (branch_a, equipment_a, mechanic_a),
            (branch_b, equipment_b, mechanic_b),
        ] {
            store
                .create_schedule(CreateInspectionScheduleCommand {
                    actor: admin_a,
                    branch_id: branch,
                    equipment_id: equipment,
                    mechanic_id: mechanic,
                    cycle: InspectionCycle::Monthly,
                    interval_days: 30,
                    due_date: time::macros::date!(2026 - 06 - 12),
                    note: None,
                    trace: TraceContext::generate(),
                    occurred_at: NOW,
                })
                .await
                .unwrap();
        }

        let visible = store
            .list_due_schedules(ListInspectionSchedulesQuery {
                branch_scope: BranchScope::single(branch_a),
                due_start: time::macros::date!(2026 - 06 - 01),
                due_end: time::macros::date!(2026 - 07 - 01),
                limit: 50,
                offset: 0,
            })
            .await
            .unwrap();

        assert_eq!(visible.total, 1);
        assert_eq!(visible.items.len(), 1);
        assert_eq!(visible.items[0].branch_id, branch_a);
        // The same-org LEFT JOIN resolves the assigned mechanic's display name
        // (seed_user stamps "예방A-<uuid>") — no raw-UUID leak.
        assert!(
            visible.items[0]
                .mechanic_display_name
                .as_deref()
                .is_some_and(|name| name.starts_with("예방A")),
            "expected mechanic_display_name to resolve to 예방A, got {:?}",
            visible.items[0].mechanic_display_name
        );

        let mine = store
            .list_my_due_schedules(ListMyInspectionSchedulesQuery {
                branch_scope: BranchScope::All,
                mechanic_id: mechanic_a,
                due_start: time::macros::date!(2026 - 06 - 01),
                due_end: time::macros::date!(2026 - 07 - 01),
                limit: 50,
                offset: 0,
            })
            .await
            .unwrap();
        assert_eq!(mine.total, 1);
        assert_eq!(mine.items[0].mechanic_id, mechanic_a);

        // Offset paging: a second page past the only row is empty, but total
        // still reflects the full match count.
        let page2 = store
            .list_due_schedules(ListInspectionSchedulesQuery {
                branch_scope: BranchScope::single(branch_a),
                due_start: time::macros::date!(2026 - 06 - 01),
                due_end: time::macros::date!(2026 - 07 - 01),
                limit: 50,
                offset: 1,
            })
            .await
            .unwrap();
        assert_eq!(page2.total, 1);
        assert!(page2.items.is_empty());
    })
    .await;
}

async fn seed_branch(pool: &PgPool, region_name: &str, branch_name: &str) -> BranchId {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("{region_name}-{}", uuid::Uuid::new_v4()))
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("{branch_name}-{}", uuid::Uuid::new_v4()))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(
    pool: &PgPool,
    name: &str,
    role: &str,
    team: Option<&str>,
    branch: BranchId,
) -> UserId {
    let id = UserId::new();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, team, org_id) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(*id.as_uuid())
    .bind(format!("{name}-{}", uuid::Uuid::new_v4()))
    .bind(Vec::from([role]))
    .bind(team)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*id.as_uuid())
        .bind(*branch.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn seed_equipment(
    pool: &PgPool,
    branch: BranchId,
    management_no: &str,
    site_name: &str,
    equipment_no: &str,
) -> EquipmentId {
    let customer_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(format!("고객-{site_name}-{}", uuid::Uuid::new_v4()))
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
    let id = sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no, model, vin,
            vehicle_registration_no, manufacturer_code, kind_code, power_code, status,
            specification, ton_text, source_sheet, source_row, updated_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5, 'GTS30D', $6, $7,
                'GLD', 'FBR', 'BATTERY', '임대', '입식', '3톤', 'inspection', 1, now(), $8)
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
    .unwrap();
    EquipmentId::from_uuid(id)
}
