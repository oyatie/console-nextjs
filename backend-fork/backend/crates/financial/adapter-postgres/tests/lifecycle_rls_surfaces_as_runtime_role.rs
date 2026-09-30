#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the per-asset lifecycle-cost read.
//!
//! `PgFinancialStore::lifecycle_cost_for_equipment` runs every SELECT
//! (registry_equipment, equipment_cost_ledger SUM-by-source, outsource_works via
//! work_orders, sales_listings) inside ONE `with_org_conn(current_org()?, ..)`
//! closure. A *static* gate (`console-gate-rls-arming`) proves the wrapping is
//! present in source; this test proves it WORKS AT RUNTIME when the read
//! executes as the genuine non-owner runtime role `console_rt` (NOSUPERUSER,
//! NOBYPASSRLS, FORCE RLS) — the only faithful exercise of the tenant policy.
//!
//! Why `console_rt` and not the default `#[sqlx::test]` pool: that pool connects as a
//! BYPASSRLS superuser, which sees every row regardless of `app.current_org` and
//! would green-light a totally broken (or leaking) read. We SEED as the owner
//! (raw inserts, wrapped writes under `scope_org`) and READ as `console_rt`.
//!
//! Asserts, with two tenants A and B each holding one asset + ledger entries +
//! a SOLD listing:
//!   (a) under org-A's armed GUC, the rollup returns A's acquisition +
//!       maintenance (MANUAL_ADMIN + PURCHASE_EXECUTION) + sale, and NEVER sums
//!       B's data;
//!   (b) under org-A's armed GUC, B's asset is NOT FOUND (cross-tenant
//!       isolation holds under RLS as `console_rt`);
//!   (c) FAIL-CLOSED: with no GUC armed the read returns not-found (zero rows),
//!       never a leak.

use console_financial_adapter_postgres::PgFinancialStore;
use console_financial_application::{
    AppendCostLedgerEntryCommand, CostLedgerSource, CreatePurchaseRequestCommand,
    FinancialConfigSnapshot, PurchaseRequestLineInput, PurchaseSubmitCommand, PurchaseType,
};
use console_financial_domain::{AcquisitionBasis, DepreciationMethod, PurchaseStatus};
use console_kernel_core::{
    BranchId, EquipmentId, EvidenceId, OrgId, TraceContext, UserId, WorkOrderId,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

/// A second, non-KNL tenant id, to prove cross-tenant isolation under `console_rt`.
const ORG_B: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);

// ===========================================================================
// Runtime-role pool: every connection becomes the genuine non-owner `console_rt`.
// Copied from workorder/adapter-postgres/tests/rls_read_surfaces_as_runtime_role.rs
// so RLS is ACTUALLY enforced — BYPASSRLS does not apply, FORCE RLS does.
// ===========================================================================
async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    // Production runs migrations as `console_app`, so 0031's
    // `ALTER DEFAULT PRIVILEGES FOR ROLE console_app` auto-grants `console_rt` SELECT on
    // every table `console_app` later creates (including sales_listings in 0043).
    // The #[sqlx::test] harness runs migrations as a different superuser, so that
    // default-privilege auto-grant never fires for sales_listings; replicate the
    // production grant here so the runtime-role read is exercised faithfully.
    sqlx::query("GRANT SELECT ON sales_listings TO console_rt")
        .execute(owner_pool)
        .await
        .unwrap();
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

fn purchase_line(item: &str, amount_won: i64) -> PurchaseRequestLineInput {
    PurchaseRequestLineInput {
        item: item.to_owned(),
        quantity: 1,
        unit_supply_price_won: amount_won,
        vat_won: Some(0),
    }
}

// ===========================================================================
// Seeding (OWNER pool). Raw inserts bypass RLS as superuser; org_id columns are
// set explicitly so each row lands in the intended tenant.
// ===========================================================================

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
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

async fn seed_user(owner_pool: &PgPool, org: Uuid, role: &str, branch_id: BranchId) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("User {}", Uuid::new_v4()))
        .bind(Vec::from([role]))
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

/// A unique `equipment_no` matching the `^[A-Z]{3}[A-Z0-9]{2}-[0-9]{4}$` check.
fn unique_equipment_no() -> String {
    let n = Uuid::new_v4().as_u128() % 10_000;
    format!("ABC12-{n:04}")
}

/// Seed an asset with an explicit acquisition cost + hours in `org`/`branch_id`.
#[allow(clippy::too_many_arguments)]
async fn seed_equipment(
    owner_pool: &PgPool,
    org: Uuid,
    branch_id: BranchId,
    management_no: &str,
    acquisition_cost_won: i64,
    vehicle_value: i64,
    residual_value: i64,
    hours: i64,
) -> EquipmentId {
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
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
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
            acquisition_cost_won, acquisition_date, hours,
            asset_registered_on, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, $4, $5, 'A', 'B', 'C', '임대',
                '좌식', '2.5T', $6, $7, $8, DATE '2024-06-01', $9,
                DATE '2024-06-01', 'lifecycle-rls-test', 1, $10)
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(unique_equipment_no())
    .bind(management_no)
    .bind(vehicle_value)
    .bind(residual_value)
    .bind(acquisition_cost_won)
    .bind(hours)
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    EquipmentId::from_uuid(equipment_id)
}

/// Seed a BARE acquisition-only asset: `acquisition_cost_won` is set but
/// `vehicle_value` (the depreciation base) is NULL — the #33 regression fixture.
/// Mirrors `seed_equipment` but binds NULL for `vehicle_value` / `residual_value`
/// so the lifecycle read must anchor TCO on the acquisition cost alone.
async fn seed_acquisition_only_equipment(
    owner_pool: &PgPool,
    org: Uuid,
    branch_id: BranchId,
    management_no: &str,
    acquisition_cost_won: i64,
    hours: i64,
) -> EquipmentId {
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
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
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
            acquisition_cost_won, acquisition_date, hours,
            asset_registered_on, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, $4, $5, 'A', 'B', 'C', '임대',
                '좌식', '2.5T', NULL, NULL, $6, DATE '2024-06-01', $7,
                DATE '2024-06-01', 'lifecycle-rls-test', 1, $8)
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(unique_equipment_no())
    .bind(management_no)
    .bind(acquisition_cost_won)
    .bind(hours)
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    EquipmentId::from_uuid(equipment_id)
}

async fn seed_work_order(
    owner_pool: &PgPool,
    org: Uuid,
    branch_id: BranchId,
    requested_by: UserId,
    equipment_id: EquipmentId,
    request_no: &str,
) -> WorkOrderId {
    let row: (Uuid, Uuid) =
        sqlx::query_as("SELECT customer_id, site_id FROM registry_equipment WHERE id = $1")
            .bind(*equipment_id.as_uuid())
            .fetch_one(owner_pool)
            .await
            .unwrap();
    let work_order_id = WorkOrderId::new();
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, symptom, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'RECEIVED', 'lifecycle fixture', $8)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(request_no)
    .bind(*branch_id.as_uuid())
    .bind(*equipment_id.as_uuid())
    .bind(row.0)
    .bind(row.1)
    .bind(*requested_by.as_uuid())
    .bind(org)
    .execute(owner_pool)
    .await
    .unwrap();
    work_order_id
}

/// Seed a SOLD sales listing tied to `equipment_id` in `org`.
async fn seed_sold_listing(
    owner_pool: &PgPool,
    org: Uuid,
    equipment_id: EquipmentId,
    price_won: i64,
) {
    sqlx::query(
        r#"
        INSERT INTO sales_listings (
            org_id, equipment_id, kind, model_name, price_won, status, updated_at
        )
        VALUES ($1, $2, 'DIESEL', 'GTS25', $3, 'SOLD', now())
        "#,
    )
    .bind(org)
    .bind(*equipment_id.as_uuid())
    .bind(price_won)
    .execute(owner_pool)
    .await
    .unwrap();
}

/// Seed outsource cost on a work order (read-only surface; must NOT enter TCO).
async fn seed_outsource_cost(
    owner_pool: &PgPool,
    org: Uuid,
    work_order_id: WorkOrderId,
    cost_won: i64,
) {
    // outsource_vendors / outsource_works both carry org_id NOT NULL (0034);
    // resolve the branch from the work order so the vendor lands in the right
    // branch + tenant.
    let vendor_id: Uuid = sqlx::query_scalar(
        "INSERT INTO outsource_vendors (branch_id, name, org_id) VALUES ((SELECT branch_id FROM work_orders WHERE id = $1), $2, $3) RETURNING id",
    )
    .bind(*work_order_id.as_uuid())
    .bind(format!("Vendor {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO outsource_works (work_order_id, vendor_id, status, reason, cost_won, requested_at, org_id)
        VALUES ($1, $2, 'COMPLETED', 'engine overhaul', $3, now(), $4)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(vendor_id)
    .bind(cost_won)
    .bind(org)
    .execute(owner_pool)
    .await
    .unwrap();
}

/// Full per-tenant fixture: org + branch + admin + asset (acquisition 30M,
/// vehicle 25M, hours 1200) + one MANUAL_ADMIN (1.5M) and one PURCHASE_EXECUTION
/// (3M) ledger entry via the wrapped write + a SOLD listing + outsource cost.
/// Returns the asset id and admin so the read can be exercised.
struct TenantFixture {
    equipment_id: EquipmentId,
}

async fn seed_tenant(
    owner_pool: &PgPool,
    org: OrgId,
    tag: &str,
    request_seq: u32,
) -> TenantFixture {
    let org_uuid = *org.as_uuid();
    seed_org(owner_pool, org_uuid, tag).await;
    let branch_id = seed_branch(owner_pool, org_uuid).await;
    let admin = seed_user(owner_pool, org_uuid, "ADMIN", branch_id).await;
    let equipment_id = seed_equipment(
        owner_pool,
        org_uuid,
        branch_id,
        &format!("{request_seq}"),
        30_000_000,
        25_000_000,
        9_000_000,
        1_200,
    )
    .await;
    let work_order_id = seed_work_order(
        owner_pool,
        org_uuid,
        branch_id,
        admin,
        equipment_id,
        &format!("20260612-{request_seq:03}"),
    )
    .await;

    // Ledger writes are wrapped (with_audits + current_org INSERT), so run them
    // under scope_org to arm the GUC exactly as the org middleware would.
    let occurred_at = datetime!(2026-06-12 12:00 UTC);
    console_platform_request_context::scope_org(org, async {
        let store = PgFinancialStore::new(owner_pool.clone());
        store
            .append_cost_ledger_entry(AppendCostLedgerEntryCommand {
                actor: admin,
                branch_id,
                equipment_id,
                work_order_id: Some(work_order_id),
                source: CostLedgerSource::ManualAdmin,
                amount_won: 1_500_000,
                memo: format!("{tag} manual repair"),
                config: financial_config(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .expect("seed: manual ledger write must succeed under armed owner pool");
        store
            .append_cost_ledger_entry(AppendCostLedgerEntryCommand {
                actor: admin,
                branch_id,
                equipment_id,
                work_order_id: Some(work_order_id),
                source: CostLedgerSource::PurchaseExecution,
                amount_won: 3_000_000,
                memo: format!("{tag} purchase execution"),
                config: financial_config(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
            .expect("seed: purchase ledger write must succeed under armed owner pool");
    })
    .await;

    seed_sold_listing(owner_pool, org_uuid, equipment_id, 28_000_000).await;
    seed_outsource_cost(owner_pool, org_uuid, work_order_id, 4_000_000).await;

    TenantFixture { equipment_id }
}

// ===========================================================================
// (a) Under org-A's armed GUC, the rollup returns A's data and never sums B.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn lifecycle_rollup_is_tenant_scoped_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);

    let a = seed_tenant(&owner_pool, org_a, "A", 1).await;
    let _b = seed_tenant(&owner_pool, org_b, "B", 2).await;

    let summary = console_platform_request_context::scope_org(org_a, async {
        let store = PgFinancialStore::new(rt_pool.clone());
        store.lifecycle_cost_for_equipment(a.equipment_id).await
    })
    .await
    .expect("org-A lifecycle read must surface A's asset as console_rt (GUC armed)");

    // Acquisition is explicit (30M), not the vehicle-value fallback.
    assert_eq!(summary.acquisition_cost_won, Some(30_000_000));
    assert_eq!(summary.acquisition_source, AcquisitionBasis::Explicit);

    // Maintenance sums BOTH sources, exactly once each: 1.5M + 3M = 4.5M.
    assert_eq!(summary.manual_total_won, 1_500_000);
    assert_eq!(summary.purchase_total_won, 3_000_000);
    assert_eq!(summary.maintenance_total_won, 4_500_000);
    assert_eq!(summary.entry_count, 2);

    // TCO = acquisition (30M) + maintenance (4.5M) = 34.5M. Outsource (4M) is
    // surfaced read-only and MUST NOT be inside TCO (double-count guard).
    assert_eq!(summary.tco_won, 34_500_000);
    assert_eq!(summary.outsource_unlinked_won, Some(4_000_000));

    // Realized SOLD price + gross margin = 28M − 34.5M = −6.5M (loss allowed).
    assert_eq!(summary.sale_price_won, Some(28_000_000));
    assert_eq!(summary.gross_margin_won, Some(-6_500_000));

    // Per-hour intensity: 4.5M / 1200 h = 3750/h.
    assert_eq!(summary.cost_per_hour_won, Some(3_750));

    assert_eq!(summary.timeline.len(), 2);
}

// ===========================================================================
// (b) Under org-A's armed GUC, B's asset is NOT FOUND (cross-tenant isolation).
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn cross_tenant_lifecycle_is_invisible_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);

    let _a = seed_tenant(&owner_pool, org_a, "A", 1).await;
    let b = seed_tenant(&owner_pool, org_b, "B", 2).await;

    let cross = console_platform_request_context::scope_org(org_a, async {
        let store = PgFinancialStore::new(rt_pool.clone());
        store.lifecycle_cost_for_equipment(b.equipment_id).await
    })
    .await;

    assert!(
        cross.is_err(),
        "org-B's asset must be INVISIBLE under org-A's GUC as console_rt (RLS isolates tenants)"
    );
}

// ===========================================================================
// (c) FAIL-CLOSED: with NO GUC armed the read returns not-found, never a leak.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn lifecycle_read_fails_closed_without_org_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let a = seed_tenant(&owner_pool, org_a, "A", 1).await;

    // No scope_org wrapper: current_org() is unset, so the read must fail closed
    // (MissingOrg / not-found), never leak the row.
    let store = PgFinancialStore::new(rt_pool.clone());
    let result = store.lifecycle_cost_for_equipment(a.equipment_id).await;
    assert!(
        result.is_err(),
        "with no org armed the lifecycle read must fail closed, never leak the asset"
    );
}

// ===========================================================================
// (#33 REGRESSION) An acquisition-only asset — acquisition_cost_won set,
// vehicle_value NULL — must still produce a TCO, not a 422. The REST handler
// `get_lifecycle_cost` first resolves the branch (equipment_branch, which used
// to hard-require vehicle_value and 422'd before the read even ran) and then
// performs the rollup; BOTH legs must succeed as the runtime role `console_rt`.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn acquisition_only_asset_returns_tco_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_uuid = *org_a.as_uuid();

    seed_org(&owner_pool, org_uuid, "A").await;
    let branch_id = seed_branch(&owner_pool, org_uuid).await;
    let admin = seed_user(&owner_pool, org_uuid, "ADMIN", branch_id).await;

    // Bare asset: acquisition 30M, NO vehicle_value (depreciation base NULL).
    let equipment_id =
        seed_acquisition_only_equipment(&owner_pool, org_uuid, branch_id, "33", 30_000_000, 1_200)
            .await;
    let work_order_id = seed_work_order(
        &owner_pool,
        org_uuid,
        branch_id,
        admin,
        equipment_id,
        "20260612-033",
    )
    .await;

    // One MANUAL_ADMIN repair (1.5M). The wrapped append path recomputes the
    // depreciation residual, which legitimately still requires vehicle_value, so
    // for this acquisition-only fixture we insert the ledger row directly to keep
    // the asset bare (vehicle_value NULL) while still giving the rollup a spend.
    sqlx::query(
        r#"
        INSERT INTO equipment_cost_ledger (
            id, branch_id, equipment_id, work_order_id, purchase_request_id,
            source, amount_won, memo, residual_before_won, residual_after_won,
            entry_at, created_by, org_id
        )
        VALUES ($1, $2, $3, $4, NULL, 'MANUAL_ADMIN', 1500000, 'bare-asset repair',
                0, 0, now(), $5, $6)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(*branch_id.as_uuid())
    .bind(*equipment_id.as_uuid())
    .bind(*work_order_id.as_uuid())
    .bind(*admin.as_uuid())
    .bind(org_uuid)
    .execute(&owner_pool)
    .await
    .unwrap();

    // Read everything the REST handler reads, as console_rt under org-A's armed GUC.
    let (branch, summary) = console_platform_request_context::scope_org(org_a, async {
        let store = PgFinancialStore::new(rt_pool.clone());
        // Leg 1: the branch lookup the handler runs FIRST (was the 422 source).
        let branch = store
            .equipment_branch(equipment_id)
            .await
            .expect("acquisition-only branch lookup must succeed, not 422 (vehicle_value NULL)");
        // Leg 2: the lifecycle rollup itself.
        let summary = store
            .lifecycle_cost_for_equipment(equipment_id)
            .await
            .expect("acquisition-only lifecycle read must return a TCO, not an error");
        (branch, summary)
    })
    .await;

    assert_eq!(
        branch, branch_id,
        "branch lookup resolved the asset's branch"
    );

    // TCO is anchored on the acquisition cost alone: 30M + 1.5M maintenance.
    assert_eq!(summary.acquisition_cost_won, Some(30_000_000));
    assert_eq!(summary.acquisition_source, AcquisitionBasis::Explicit);
    assert_eq!(summary.maintenance_total_won, 1_500_000);
    assert_eq!(summary.tco_won, 31_500_000);

    // Per-hour intensity still computes off maintenance: 1.5M / 1200h = 1250/h.
    assert_eq!(summary.cost_per_hour_won, Some(1_250));

    // No sale, no vehicle_value -> gross margin stays None (not an error).
    assert_eq!(summary.sale_price_won, None);
    assert_eq!(summary.gross_margin_won, None);
}

// ===========================================================================
// (#19.18) Purchase-request create + submit as the runtime role `console_rt`.
//
// The WORM-replica precondition is DEFERRED from CREATE to SUBMIT. This test
// proves, as the genuine non-owner `console_rt` under the armed GUC:
//   (a) a purchase request CREATES against still-replicating (UNVERIFIED)
//       REQUEST evidence — the prod-real unblock; before the fix create itself
//       4xx'd and the web swallowed the reason — and the created row is VISIBLE
//       to a fresh armed read (`purchase_request`), i.e. the approver can see it;
//   (b) SUBMIT is refused with the surfaced WORM reason while the replica is
//       UNVERIFIED, then SUCCEEDS once the replica is VERIFIED;
//   (c) cross-tenant: org-B's purchase request is INVISIBLE under org-A's GUC.
// ===========================================================================

/// Seed a REQUEST-stage statement evidence row in `org` with the given WORM
/// replica status, tied to `work_order_id`. OWNER pool (raw insert).
async fn seed_request_evidence(
    owner_pool: &PgPool,
    org: Uuid,
    work_order_id: WorkOrderId,
    uploaded_by: UserId,
    worm_replica_status: &str,
) -> EvidenceId {
    let evidence_id = EvidenceId::new();
    sqlx::query(
        r#"
        INSERT INTO evidence_media (
            id, work_order_id, stage, s3_key, content_type, size_bytes,
            uploaded_by, worm_replica_status, retry_count, org_id
        )
        VALUES ($1, $2, 'REQUEST', $3, 'application/pdf', 2048, $4, $5, 0, $6)
        "#,
    )
    .bind(*evidence_id.as_uuid())
    .bind(*work_order_id.as_uuid())
    .bind(format!(
        "work-orders/{work_order_id}/REQUEST/{evidence_id}.pdf"
    ))
    .bind(*uploaded_by.as_uuid())
    .bind(worm_replica_status)
    .bind(org)
    .execute(owner_pool)
    .await
    .unwrap();
    evidence_id
}

/// The grants the production default-privilege auto-grant would give `console_rt`
/// for the purchase-request write path; the `#[sqlx::test]` harness runs
/// migrations as a different superuser, so replicate them here. Each GRANT is a
/// static literal (no interpolation) to satisfy the dynamic-SQL audit lint.
async fn grant_purchase_path_to_runtime_role(owner_pool: &PgPool) {
    for grant in [
        "GRANT SELECT, INSERT, UPDATE ON evidence_media TO console_rt",
        "GRANT SELECT, INSERT, UPDATE ON work_orders TO console_rt",
        "GRANT SELECT, INSERT, UPDATE ON registry_equipment TO console_rt",
        "GRANT SELECT, INSERT, UPDATE ON financial_purchase_requests TO console_rt",
        "GRANT SELECT, INSERT, UPDATE ON financial_purchase_history TO console_rt",
        "GRANT SELECT, INSERT, UPDATE ON audit_events TO console_rt",
    ] {
        sqlx::query(grant).execute(owner_pool).await.unwrap();
    }
}

/// Seed org + branch + admin + asset + a REQUEST work order, returning the ids
/// the purchase-request create needs.
struct PurchaseFixture {
    branch_id: BranchId,
    admin: UserId,
    equipment_id: EquipmentId,
    work_order_id: WorkOrderId,
}

async fn seed_purchase_fixture(
    owner_pool: &PgPool,
    org: OrgId,
    tag: &str,
    request_seq: u32,
) -> PurchaseFixture {
    let org_uuid = *org.as_uuid();
    seed_org(owner_pool, org_uuid, tag).await;
    let branch_id = seed_branch(owner_pool, org_uuid).await;
    let admin = seed_user(owner_pool, org_uuid, "ADMIN", branch_id).await;
    let equipment_id = seed_equipment(
        owner_pool,
        org_uuid,
        branch_id,
        &format!("{request_seq}"),
        30_000_000,
        25_000_000,
        9_000_000,
        1_200,
    )
    .await;
    let work_order_id = seed_work_order(
        owner_pool,
        org_uuid,
        branch_id,
        admin,
        equipment_id,
        &format!("20260612-{request_seq:03}"),
    )
    .await;
    PurchaseFixture {
        branch_id,
        admin,
        equipment_id,
        work_order_id,
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn purchase_request_create_and_submit_as_runtime_role(owner_pool: PgPool) {
    grant_purchase_path_to_runtime_role(&owner_pool).await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let occurred_at = datetime!(2026-06-12 12:00 UTC);

    let fx = seed_purchase_fixture(&owner_pool, org_a, "A", 1).await;
    // Evidence is still replicating (UNVERIFIED) at create time.
    let evidence = seed_request_evidence(
        &owner_pool,
        *org_a.as_uuid(),
        fx.work_order_id,
        fx.admin,
        "PENDING",
    )
    .await;

    // (a) Create succeeds against PENDING evidence as armed console_rt, and the row
    //     is visible to a fresh armed read.
    let created = console_platform_request_context::scope_org(org_a, async {
        let store = PgFinancialStore::new(rt_pool.clone());
        store
            .create_purchase_request(CreatePurchaseRequestCommand {
                actor: fx.admin,
                branch_id: fx.branch_id,
                equipment_id: Some(fx.equipment_id),
                work_order_id: Some(fx.work_order_id),
                statement_evidence_id: Some(evidence),
                purchase_type: PurchaseType::LegacyManual,
                vendor_name: "한빛부품".to_owned(),
                amount_won: Some(500_000),
                lines: vec![purchase_line("정기 부품 교체", 500_000)],
                quote_attachment_ids: Vec::new(),
                memo: "정기 부품 교체".to_owned(),
                config: financial_config(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
    })
    .await
    .expect("create must succeed against PENDING evidence as armed console_rt");
    assert_eq!(created.status, PurchaseStatus::StatementAttached);

    let visible = console_platform_request_context::scope_org(org_a, async {
        let store = PgFinancialStore::new(rt_pool.clone());
        store.purchase_request(created.id).await
    })
    .await
    .expect("the approver must SEE the created request under the same armed GUC");
    assert_eq!(visible.id, created.id);
    assert_eq!(visible.status, PurchaseStatus::StatementAttached);

    // (b) SUBMIT is refused with the surfaced WORM reason while PENDING.
    let blocked = console_platform_request_context::scope_org(org_a, async {
        let store = PgFinancialStore::new(rt_pool.clone());
        store
            .submit_purchase_request(PurchaseSubmitCommand {
                actor: fx.admin,
                purchase_request_id: created.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
    })
    .await
    .unwrap_err();
    assert!(
        blocked.to_string().contains("WORM-verified"),
        "submit must surface the deferred WORM reason, got: {blocked}"
    );

    // The replica finishes verifying (the worker flips it to VERIFIED).
    sqlx::query("UPDATE evidence_media SET worm_replica_status = 'VERIFIED' WHERE id = $1")
        .bind(*evidence.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

    let submitted = console_platform_request_context::scope_org(org_a, async {
        let store = PgFinancialStore::new(rt_pool.clone());
        store
            .submit_purchase_request(PurchaseSubmitCommand {
                actor: fx.admin,
                purchase_request_id: created.id,
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
    })
    .await
    .expect("submit must succeed once the WORM replica is VERIFIED");
    assert_eq!(submitted.status, PurchaseStatus::RequestSubmitted);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn cross_tenant_purchase_request_is_invisible_as_runtime_role(owner_pool: PgPool) {
    grant_purchase_path_to_runtime_role(&owner_pool).await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);
    let occurred_at = datetime!(2026-06-12 12:00 UTC);

    // org-B owns a purchase request (created under B's armed GUC).
    let fx_b = seed_purchase_fixture(&owner_pool, org_b, "B", 2).await;
    let evidence_b = seed_request_evidence(
        &owner_pool,
        *org_b.as_uuid(),
        fx_b.work_order_id,
        fx_b.admin,
        "VERIFIED",
    )
    .await;
    let b_purchase = console_platform_request_context::scope_org(org_b, async {
        let store = PgFinancialStore::new(rt_pool.clone());
        store
            .create_purchase_request(CreatePurchaseRequestCommand {
                actor: fx_b.admin,
                branch_id: fx_b.branch_id,
                equipment_id: Some(fx_b.equipment_id),
                work_order_id: Some(fx_b.work_order_id),
                statement_evidence_id: Some(evidence_b),
                purchase_type: PurchaseType::LegacyManual,
                vendor_name: "Org B Vendor".to_owned(),
                amount_won: Some(500_000),
                lines: vec![purchase_line("org-b purchase", 500_000)],
                quote_attachment_ids: Vec::new(),
                memo: "org-b purchase".to_owned(),
                config: financial_config(),
                trace: TraceContext::generate(),
                occurred_at,
            })
            .await
    })
    .await
    .expect("org-B create must succeed under B's armed GUC");

    // Under org-A's GUC, B's purchase request is NOT FOUND (RLS isolates tenants).
    seed_org(&owner_pool, *org_a.as_uuid(), "A").await;
    let cross = console_platform_request_context::scope_org(org_a, async {
        let store = PgFinancialStore::new(rt_pool.clone());
        store.purchase_request(b_purchase.id).await
    })
    .await;
    assert!(
        cross.is_err(),
        "org-B's purchase request must be INVISIBLE under org-A's GUC as console_rt"
    );
}
