//! Per-tenant operational dashboard (`ops_summary`) integration tests.
//!
//! These verify two things the ops console depends on:
//! 1. the rollup counts (funnel, aging, equipment, substitutions, approvals,
//!    support, mechanic utilization) are computed correctly, and
//! 2. the read is org-scoped under RLS — a SECOND org's rows are NEVER counted
//!    when the summary runs bound to the first org's tenant context.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_kernel_core::OrgId;
use console_reporting_adapter_postgres::PgKpiRepository;
use console_reporting_application::{OpsSummary, OpsSummaryPort, OpsSummaryQuery};
use sqlx::pool::PoolOptions;
use sqlx::{Executor, PgConnection, PgPool};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const QUERY: OpsSummaryQuery = OpsSummaryQuery {
    aging_hours: 24,
    at_risk_minutes: 5,
    top_mechanics: 10,
};

/// Minimal branch/region/equipment/site/customer scaffold for one org. Inserts
/// run as the test pool's owner role, which bypasses RLS, so we can stage rows
/// for any `org_id` directly (the production app always reads under RLS).
struct OrgFixture {
    org_id: Uuid,
    branch_id: Uuid,
    equipment_id: Uuid,
    customer_id: Uuid,
    site_id: Uuid,
}

async fn seed_org(pool: &PgPool, slug: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO organizations (slug, name) VALUES ($1, $2) RETURNING id")
        .bind(format!(
            "{slug}-{}",
            &Uuid::new_v4().simple().to_string()[..8]
        ))
        .bind(format!("Org {slug}"))
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn seed_fixture(pool: &PgPool, org_id: Uuid) -> OrgFixture {
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("R-{}", Uuid::new_v4()))
            .bind(org_id)
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("B-{}", Uuid::new_v4()))
    .bind(org_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let customer_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(branch_id)
    .bind(format!("C-{}", Uuid::new_v4()))
    .bind(org_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(branch_id)
    .bind(customer_id)
    .bind(format!("S-{}", Uuid::new_v4()))
    .bind(org_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let equipment_id = seed_equipment(pool, org_id, branch_id, customer_id, site_id, "임대").await;
    OrgFixture {
        org_id,
        branch_id,
        equipment_id,
        customer_id,
        site_id,
    }
}

#[allow(clippy::too_many_arguments)]
async fn seed_equipment(
    pool: &PgPool,
    org_id: Uuid,
    branch_id: Uuid,
    customer_id: Uuid,
    site_id: Uuid,
    status: &str,
) -> Uuid {
    sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no, model,
            manufacturer_code, kind_code, power_code, status, specification, ton_text,
            rental_fee, vehicle_value, residual_value, source_sheet, source_row, updated_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5, 'FBR', 'GLD', 'FBR', 'BATTERY', $6,
                '입식', '1.5톤', 700000, 10000000, 5000000, 'ops', 1, now(), $7)
        RETURNING id
        "#,
    )
    .bind(branch_id)
    .bind(customer_id)
    .bind(site_id)
    .bind(unique_equipment_no())
    .bind(format!("MNG-{}", &Uuid::new_v4().simple().to_string()[..6]))
    .bind(status)
    .bind(org_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_user(pool: &PgPool, org_id: Uuid, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(name.to_owned())
        .bind(vec!["MECHANIC".to_owned()])
        .bind(org_id)
        .execute(pool)
        .await
        .unwrap();
    id
}

/// Insert a work order with the given status. Returns its id.
async fn seed_work_order(pool: &PgPool, fx: &OrgFixture, requested_by: Uuid, status: &str) -> Uuid {
    let request_no = format!("{:08}-{:03}", rand_request(), rand_request() % 1000);
    sqlx::query_scalar(
        r#"
        INSERT INTO work_orders (
            request_no, branch_id, equipment_id, customer_id, site_id, requested_by,
            status, priority, symptom, created_at, updated_at, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'P1', 'ops symptom', now(), now(), $8)
        RETURNING id
        "#,
    )
    .bind(request_no)
    .bind(fx.branch_id)
    .bind(fx.equipment_id)
    .bind(fx.customer_id)
    .bind(fx.site_id)
    .bind(requested_by)
    .bind(status)
    .bind(fx.org_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn rand_request() -> u32 {
    // Deterministic-enough unique-ish 8-digit base from a fresh uuid.
    (Uuid::new_v4().as_u128() % 90_000_000) as u32 + 10_000_000
}

/// A unique `equipment_no` matching `^[A-Z]{3}[A-Z0-9]{2}-[0-9]{4}$`.
fn unique_equipment_no() -> String {
    const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let raw = Uuid::new_v4().as_u128();
    let pick = |shift: u32| LETTERS[((raw >> shift) as usize) % LETTERS.len()] as char;
    let digits = (raw % 10_000) as u16;
    format!(
        "{}{}{}{}{}-{:04}",
        pick(0),
        pick(8),
        pick(16),
        pick(24),
        pick(32),
        digits
    )
}

async fn assign(pool: &PgPool, org_id: Uuid, work_order_id: Uuid, mechanic_id: Uuid) {
    sqlx::query(
        "INSERT INTO work_order_assignments (work_order_id, mechanic_id, role, assigned_at, org_id) VALUES ($1, $2, 'PRIMARY', now(), $3)",
    )
    .bind(work_order_id)
    .bind(mechanic_id)
    .bind(org_id)
    .execute(pool)
    .await
    .unwrap();
}

/// Build a pool that connects under the unprivileged `console_rt` runtime role —
/// the SAME role the production app uses — so RLS is fully enforced for the
/// repository read. The `#[sqlx::test]` pool connects as the owner (which
/// bypasses non-FORCE RLS), so for an isolation-faithful read we must drop to
/// `console_rt`, exactly as the deployed app's connection does.
async fn console_rt_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options();
    PoolOptions::new()
        .max_connections(2)
        .after_connect(|conn: &mut PgConnection, _meta| {
            Box::pin(async move {
                // Static literal role name; mirrors rls_rollout_isolation.rs.
                conn.execute("SET ROLE console_rt").await?;
                Ok(())
            })
        })
        .connect_with((*options).clone())
        .await
        .unwrap()
}

async fn summary_for(pool: &PgPool, org_id: Uuid) -> OpsSummary {
    let rt_pool = console_rt_pool(pool).await;
    let repo = PgKpiRepository::new(rt_pool.clone());
    let summary =
        console_platform_request_context::scope_org(OrgId::from_uuid(org_id), async move {
            repo.ops_summary(QUERY).await.unwrap()
        })
        .await;
    rt_pool.close().await;
    summary
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn funnel_aging_and_distribution_counts_are_correct(pool: PgPool) {
    let org = seed_org(&pool, "primary").await;
    let fx = seed_fixture(&pool, org).await;
    let requester = seed_user(&pool, org, "요청자").await;
    let mechanic = seed_user(&pool, org, "정비사").await;

    // Funnel: 1 received, 1 assigned (also adds a mechanic load), 2 in-progress,
    // 1 completed.
    seed_work_order(&pool, &fx, requester, "RECEIVED").await;
    let assigned_wo = seed_work_order(&pool, &fx, requester, "ASSIGNED").await;
    assign(&pool, org, assigned_wo, mechanic).await;
    seed_work_order(&pool, &fx, requester, "IN_PROGRESS").await;
    seed_work_order(&pool, &fx, requester, "REPORT_SUBMITTED").await;
    seed_work_order(&pool, &fx, requester, "FINAL_COMPLETED").await;

    // An aging open work order: created 30h ago, still IN_PROGRESS.
    let aging = seed_work_order(&pool, &fx, requester, "IN_PROGRESS").await;
    sqlx::query("UPDATE work_orders SET created_at = $2 WHERE id = $1")
        .bind(aging)
        .bind(OffsetDateTime::now_utc() - Duration::hours(30))
        .execute(&pool)
        .await
        .unwrap();

    // Equipment distribution: the fixture seeds one 임대; add one 폐기 + one 예비.
    seed_equipment(&pool, org, fx.branch_id, fx.customer_id, fx.site_id, "폐기").await;
    seed_equipment(&pool, org, fx.branch_id, fx.customer_id, fx.site_id, "예비").await;

    let summary = summary_for(&pool, org).await;

    assert_eq!(summary.funnel.received, 1);
    assert_eq!(summary.funnel.assigned, 1);
    assert_eq!(summary.funnel.in_progress, 3, "2 in-progress + 1 aging");
    assert_eq!(summary.funnel.completed, 1);
    assert_eq!(summary.aging_hours, 24);
    assert_eq!(summary.aging_work_orders, 1);
    assert_eq!(summary.equipment_status.rented, 1);
    assert_eq!(summary.equipment_status.scrapped, 1);
    assert_eq!(summary.equipment_status.spare, 1);
    assert_eq!(summary.equipment_status.sold, 0);
    assert_eq!(summary.equipment_status.replacement, 0);

    // The assigned (non-terminal) work order contributes one active assignment.
    assert_eq!(summary.mechanic_load.len(), 1);
    assert_eq!(summary.mechanic_load[0].mechanic_id, mechanic);
    assert_eq!(summary.mechanic_load[0].active_assignments, 1);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn substitutions_approvals_and_support_counts_are_correct(pool: PgPool) {
    let org = seed_org(&pool, "aux").await;
    let fx = seed_fixture(&pool, org).await;
    let requester = seed_user(&pool, org, "요청자").await;

    // A pending approval step on a work order.
    let wo = seed_work_order(&pool, &fx, requester, "ADMIN_REVIEW").await;
    sqlx::query(
        "INSERT INTO work_order_approval_steps (work_order_id, role, status, step_order, org_id) VALUES ($1, 'ADMIN', 'PENDING', 1, $2)",
    )
    .bind(wo)
    .bind(org)
    .execute(&pool)
    .await
    .unwrap();

    // An active substitution (대차): a second equipment unit substitutes the first.
    let substitute =
        seed_equipment(&pool, org, fx.branch_id, fx.customer_id, fx.site_id, "대체").await;
    sqlx::query(
        r#"
        INSERT INTO equipment_substitutions (
            source_equipment_id, substitute_equipment_id, branch_id,
            assigned_by, assignment_location, assigned_at, org_id
        )
        VALUES ($1, $2, $3, $4, '현장', now(), $5)
        "#,
    )
    .bind(fx.equipment_id)
    .bind(substitute)
    .bind(fx.branch_id)
    .bind(requester)
    .bind(org)
    .execute(&pool)
    .await
    .unwrap();

    // Two open support tickets (OPEN, IN_PROGRESS) and one closed (excluded).
    for status in ["OPEN", "IN_PROGRESS", "CLOSED"] {
        seed_support_ticket(&pool, &fx, requester, status).await;
    }

    let summary = summary_for(&pool, org).await;
    assert_eq!(summary.pending_approvals, 1);
    assert_eq!(summary.active_substitutions, 1);
    assert_eq!(summary.open_support_tickets, 2);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn summary_is_org_scoped_second_org_data_is_excluded(pool: PgPool) {
    // Two tenants, each with their own work orders + equipment.
    let org_a = seed_org(&pool, "tenant-a").await;
    let fx_a = seed_fixture(&pool, org_a).await;
    let req_a = seed_user(&pool, org_a, "A요청자").await;

    let org_b = seed_org(&pool, "tenant-b").await;
    let fx_b = seed_fixture(&pool, org_b).await;
    let req_b = seed_user(&pool, org_b, "B요청자").await;

    // Org A: 1 received work order.
    seed_work_order(&pool, &fx_a, req_a, "RECEIVED").await;

    // Org B: 3 received work orders + extra equipment. None of these may appear
    // in org A's summary.
    for _ in 0..3 {
        seed_work_order(&pool, &fx_b, req_b, "RECEIVED").await;
    }
    seed_equipment(
        &pool,
        org_b,
        fx_b.branch_id,
        fx_b.customer_id,
        fx_b.site_id,
        "폐기",
    )
    .await;

    let summary_a = summary_for(&pool, org_a).await;
    assert_eq!(
        summary_a.funnel.received, 1,
        "org A sees only its own work order"
    );
    assert_eq!(
        summary_a.equipment_status.rented, 1,
        "org A sees only its fixture's one 임대 unit"
    );
    assert_eq!(
        summary_a.equipment_status.scrapped, 0,
        "org B's 폐기 unit must NOT leak into org A"
    );

    // And org B sees ITS own counts, not org A's.
    let summary_b = summary_for(&pool, org_b).await;
    assert_eq!(summary_b.funnel.received, 3);
    assert_eq!(summary_b.equipment_status.scrapped, 1);
}

async fn seed_support_ticket(
    pool: &PgPool,
    fx: &OrgFixture,
    requester_user_id: Uuid,
    status: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO support_tickets (
            branch_id, origin, category, priority, status, title, body,
            requester_user_id, org_id
        )
        VALUES ($1, 'INTERNAL', 'OPERATIONAL', 'MEDIUM', $2, '제목', '내용',
                $3, $4)
        "#,
    )
    .bind(fx.branch_id)
    .bind(status)
    .bind(requester_user_id)
    .bind(fx.org_id)
    .execute(pool)
    .await
    .unwrap();
}
