#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-LC period-lock enforcement on the REAL financial write path, executed as
//! the genuine non-owner runtime role `console_rt`:
//!
//!   (a) an active `accounting` period lock makes `append_cost_ledger_entry`
//!       (the shared insert both manual admin entries and purchase execution
//!       funnel through) fail closed with a conflict, and neither the ledger
//!       row nor any audit row lands (atomic rollback);
//!   (b) unlocking restores the write path — while a DIFFERENT tenant's active
//!       lock over the same window stays irrelevant (RLS isolation).

use console_financial_adapter_postgres::PgFinancialStore;
use console_financial_application::{
    AppendCostLedgerEntryCommand, CostLedgerSource, FinancialConfigSnapshot,
};
use console_financial_domain::DepreciationMethod;
use console_kernel_core::{BranchId, EquipmentId, OrgId, TraceContext, UserId};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

const ORG_B: Uuid = Uuid::from_u128(0x6666_6666_6666_6666_6666_6666_6666_6666);

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

async fn seed_admin(owner_pool: &PgPool, org: Uuid, branch_id: BranchId) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("Admin {}", Uuid::new_v4()))
        .bind(vec!["ADMIN"])
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    user_id
}

fn unique_equipment_no() -> String {
    let n = Uuid::new_v4().as_u128() % 10_000;
    format!("ABC12-{n:04}")
}

async fn seed_equipment(owner_pool: &PgPool, org: Uuid, branch_id: BranchId) -> EquipmentId {
    let customer_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(format!("Customer {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let site_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(format!("Site {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let equipment_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, vehicle_value, residual_value,
            asset_registered_on, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, $4, $5, 'A', 'B', 'C', '임대',
                '좌식', '2.5T', 30000000, 12000000,
                DATE '2024-06-01', 'period-lock-test', 1, $6)
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(unique_equipment_no())
    .bind(format!("MGMT-{}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    EquipmentId::from_uuid(equipment_id)
}

fn financial_config() -> FinancialConfigSnapshot {
    FinancialConfigSnapshot {
        depreciation_method: DepreciationMethod::StraightLine,
        useful_life_months: 60,
        residual_rate_bps: 1_000,
        declining_balance_rate_bps: 2_000,
        management_fee_rate_bps: 1_000,
        profit_rate_bps: 500,
        floor_negative_quote_residual: true,
        executive_approval_threshold_won: 2_000_000,
    }
}

fn ledger_command(
    actor: UserId,
    branch_id: BranchId,
    equipment_id: EquipmentId,
) -> AppendCostLedgerEntryCommand {
    AppendCostLedgerEntryCommand {
        actor,
        branch_id,
        equipment_id,
        work_order_id: None,
        source: CostLedgerSource::ManualAdmin,
        amount_won: 1_500_000,
        memo: "6월 유압펌프 수리".to_owned(),
        config: financial_config(),
        trace: TraceContext::generate(),
        // Business date INSIDE the locked June window.
        occurred_at: datetime!(2026-06-15 09:00 UTC),
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn accounting_period_lock_blocks_cost_ledger_and_unlock_restores(owner_pool: PgPool) {
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid, "A").await;
    seed_org(&owner_pool, ORG_B, "B").await;
    let branch_id = seed_branch(&owner_pool, org_uuid).await;
    let admin = seed_admin(&owner_pool, org_uuid, branch_id).await;
    let equipment_id = seed_equipment(&owner_pool, org_uuid, branch_id).await;

    // Active June accounting locks: one in org A (must block) and one in org B
    // (must be IRRELEVANT to org A under RLS).
    let lock_a: Uuid = sqlx::query_scalar(
        "INSERT INTO period_locks (org_id, domain, period_start, period_end, reason) \
         VALUES ($1, 'accounting', DATE '2026-06-01', DATE '2026-06-30', '6월 회계 마감') \
         RETURNING id",
    )
    .bind(org_uuid)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO period_locks (org_id, domain, period_start, period_end, reason) \
         VALUES ($1, 'accounting', DATE '2026-06-01', DATE '2026-06-30', 'B 마감')",
    )
    .bind(ORG_B)
    .execute(&owner_pool)
    .await
    .unwrap();

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let store = PgFinancialStore::new(rt_pool.clone());

    // (a) Locked period → the REAL mutation fails closed with a conflict.
    let blocked = console_platform_request_context::scope_org(org, async {
        store
            .append_cost_ledger_entry(ledger_command(admin, branch_id, equipment_id))
            .await
    })
    .await;
    let err = blocked.expect_err("cost ledger write inside a locked accounting period must fail");
    let message = err.to_string();
    assert!(
        message.contains("locked"),
        "error must name the period lock: {message}"
    );

    // Nothing landed: no ledger row, no residual recompute audit.
    let ledger_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM equipment_cost_ledger WHERE equipment_id = $1")
            .bind(*equipment_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(ledger_rows, 0, "refused write must roll back atomically");

    // (b) Unlock org A (org B's lock stays active) → write succeeds.
    sqlx::query(
        "UPDATE period_locks SET unlocked_at = now(), unlock_reason = '정정 재개' WHERE id = $1",
    )
    .bind(lock_a)
    .execute(&owner_pool)
    .await
    .unwrap();

    let entry = console_platform_request_context::scope_org(org, async {
        store
            .append_cost_ledger_entry(ledger_command(admin, branch_id, equipment_id))
            .await
    })
    .await
    .expect("after unlock the cost ledger write must succeed (org B's lock is foreign)");
    assert_eq!(entry.amount_won, 1_500_000);

    let ledger_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM equipment_cost_ledger WHERE equipment_id = $1")
            .bind(*equipment_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(ledger_rows, 1);
}
