#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the PM (정기 예방정비) schedule create + list (#19.22).
//!
//! The reported symptom — "won't register / doesn't appear" — is NOT a missing
//! arm: `create_schedule` writes correctly and `list_due_schedules` is armed.
//! The defect is the read DATE WINDOW: the console defaulted to `[today,
//! today+30)`, so a schedule whose `due_date` falls OUTSIDE that window (a
//! backfilled past-due date, or one further out than 30 days) is invisible even
//! though it was created. This test proves, AS the genuine non-owner runtime
//! role `console_rt` (NOSUPERUSER, NOBYPASSRLS, FORCE RLS):
//!   * a `due_date` BEFORE the window and one AFTER it both return `total == 0`
//!     under the default `[today, today+30)` window — reproducing the bug;
//!   * a corrected window that spans those dates returns BOTH rows;
//!   * cross-tenant isolation: a second org's schedule is INVISIBLE under KNL's
//!     armed GUC, even with a wide window.
//!
//! Why `console_rt` and not the default `#[sqlx::test]` pool: that pool connects as a
//! BYPASSRLS superuser, which sees every row regardless of `app.current_org`. We
//! SEED as the owner and CREATE/LIST as `console_rt` under the armed GUC.

use console_inspection_adapter_postgres::PgInspectionStore;
use console_inspection_application::{
    CreateInspectionScheduleCommand, ListInspectionSchedulesQuery,
};
use console_inspection_domain::InspectionCycle;
use console_kernel_core::{BranchId, BranchScope, EquipmentId, OrgId, TraceContext, UserId};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// A second, non-KNL tenant id, to prove cross-tenant isolation under `console_rt`.
const ORG_T2: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);

const MAX_LIMIT: i64 = 200;

/// A pool whose every connection runs `SET ROLE console_rt`, so statements execute as
/// the production runtime role (NOSUPERUSER, NOBYPASSRLS) under FORCE RLS.
async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

// ===========================================================================
// Seeding (OWNER pool). org_id columns are set explicitly so each row lands in
// the intended tenant; raw inserts bypass RLS as the superuser owner.
// ===========================================================================

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(owner_pool)
    .await
    .unwrap();
}

async fn seed_branch(owner_pool: &PgPool, org: Uuid) -> BranchId {
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Region {}", Uuid::new_v4()))
            .bind(org)
            .fetch_one(owner_pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("Branch {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

/// Seed an active prevention mechanic (`team = '예방'`, `MECHANIC`) in `branch`,
/// satisfying the `ensure_prevention_mechanic_tx` guard at create time.
async fn seed_prevention_mechanic(owner_pool: &PgPool, org: Uuid, branch: BranchId) -> UserId {
    let id = UserId::new();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, team, org_id, is_active) VALUES ($1, $2, $3, '예방', $4, true)",
    )
    .bind(*id.as_uuid())
    .bind(format!("예방기사-{}", Uuid::new_v4()))
    .bind(Vec::from(["MECHANIC"]))
    .bind(org)
    .execute(owner_pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*id.as_uuid())
        .bind(*branch.as_uuid())
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    id
}

fn unique_equipment_no() -> String {
    let n = Uuid::new_v4().as_u128() % 10_000;
    format!("ABC12-{n:04}")
}

async fn seed_equipment(owner_pool: &PgPool, org: Uuid, branch: BranchId) -> EquipmentId {
    let customer_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(format!("Customer {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let site_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(customer_id)
    .bind(format!("Site {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, model, source_sheet, source_row, updated_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5,
                'A', 'B', 'C', '임대', '좌식', '2.5', 'GTS25DE', 'test', 1, now(), $6)
        RETURNING id
        "#,
    )
    .bind(*branch.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(unique_equipment_no())
    .bind(format!("MG-{}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    EquipmentId::from_uuid(id)
}

/// Create one schedule with the given `due_date` AS `console_rt` under the armed GUC,
/// exactly as the create handler does.
async fn create_schedule_as_rt(
    rt_pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    equipment: EquipmentId,
    mechanic: UserId,
    due_date: time::Date,
) {
    console_platform_request_context::scope_org(org, async {
        let store = PgInspectionStore::new(rt_pool.clone());
        store
            .create_schedule(CreateInspectionScheduleCommand {
                actor: mechanic,
                branch_id: branch,
                equipment_id: equipment,
                mechanic_id: mechanic,
                cycle: InspectionCycle::Monthly,
                interval_days: 31,
                due_date,
                note: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .expect("create_schedule must succeed as console_rt under the armed GUC");
    })
    .await;
}

/// List schedules in `[due_start, due_end)` AS `console_rt`, returning the unpaged
/// total for that window.
async fn list_total_as_rt(
    rt_pool: &PgPool,
    org: OrgId,
    branch_scope: BranchScope,
    due_start: time::Date,
    due_end: time::Date,
) -> i64 {
    console_platform_request_context::scope_org(org, async {
        let store = PgInspectionStore::new(rt_pool.clone());
        store
            .list_due_schedules(ListInspectionSchedulesQuery {
                branch_scope,
                due_start,
                due_end,
                limit: MAX_LIMIT,
                offset: 0,
            })
            .await
            .expect("list_due_schedules must succeed as console_rt under the armed GUC")
            .total
    })
    .await
}

// ===========================================================================
// The date-window read: the default [today, today+30) window HIDES a past-due
// and a far-future schedule; a corrected window surfaces both.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn schedule_window_hides_then_surfaces_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let knl_uuid = *knl.as_uuid();
    seed_org(&owner_pool, knl_uuid, "knl").await;
    let branch = seed_branch(&owner_pool, knl_uuid).await;
    let mechanic = seed_prevention_mechanic(&owner_pool, knl_uuid, branch).await;
    let equipment_past = seed_equipment(&owner_pool, knl_uuid, branch).await;
    let equipment_future = seed_equipment(&owner_pool, knl_uuid, branch).await;

    let today = OffsetDateTime::now_utc().date();
    let past_due = today - Duration::days(90);
    let future_due = today + Duration::days(60);

    create_schedule_as_rt(&rt_pool, knl, branch, equipment_past, mechanic, past_due).await;
    create_schedule_as_rt(
        &rt_pool,
        knl,
        branch,
        equipment_future,
        mechanic,
        future_due,
    )
    .await;

    // (a) The default [today, today+30) window sees NEITHER schedule — the bug.
    let default_window = list_total_as_rt(
        &rt_pool,
        knl,
        BranchScope::All,
        today,
        today + Duration::days(30),
    )
    .await;
    assert_eq!(
        default_window, 0,
        "the default [today, today+30) window must hide the past-due and far-future schedules"
    );

    // (b) A corrected window that spans both due dates surfaces BOTH (end is
    // exclusive, so it must be strictly after the latest due date).
    let wide = list_total_as_rt(
        &rt_pool,
        knl,
        BranchScope::All,
        past_due,
        future_due + Duration::days(1),
    )
    .await;
    assert_eq!(
        wide, 2,
        "a window spanning the due dates must surface both created schedules as console_rt"
    );
}

// ===========================================================================
// CROSS-TENANT ISOLATION: a SECOND org's schedule must NOT appear under KNL's
// armed GUC, even with a wide window. Proves the create arms the RIGHT tenant.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn cross_tenant_schedule_is_invisible_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let org2 = OrgId::from_uuid(ORG_T2);
    let knl_uuid = *knl.as_uuid();
    let org2_uuid = *org2.as_uuid();
    seed_org(&owner_pool, knl_uuid, "knl").await;
    seed_org(&owner_pool, org2_uuid, "t2").await;

    let today = OffsetDateTime::now_utc().date();
    let due = today + Duration::days(5);

    // One schedule in each tenant, on the same due date.
    let knl_branch = seed_branch(&owner_pool, knl_uuid).await;
    let knl_mechanic = seed_prevention_mechanic(&owner_pool, knl_uuid, knl_branch).await;
    let knl_equipment = seed_equipment(&owner_pool, knl_uuid, knl_branch).await;
    create_schedule_as_rt(&rt_pool, knl, knl_branch, knl_equipment, knl_mechanic, due).await;

    let t2_branch = seed_branch(&owner_pool, org2_uuid).await;
    let t2_mechanic = seed_prevention_mechanic(&owner_pool, org2_uuid, t2_branch).await;
    let t2_equipment = seed_equipment(&owner_pool, org2_uuid, t2_branch).await;
    create_schedule_as_rt(&rt_pool, org2, t2_branch, t2_equipment, t2_mechanic, due).await;

    // Under KNL's armed GUC, a wide window sees ONLY KNL's schedule, not org2's.
    let knl_total = list_total_as_rt(
        &rt_pool,
        knl,
        BranchScope::All,
        today - Duration::days(1),
        today + Duration::days(30),
    )
    .await;
    assert_eq!(
        knl_total, 1,
        "under KNL's armed GUC only KNL's schedule is visible — the other tenant's is INVISIBLE"
    );
}
