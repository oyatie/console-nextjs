#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Dynamic proof for the multi-tenant ROLLOUT (migrations 0032..0035): RLS
//! isolates two orgs end-to-end across a REPRESENTATIVE SET of the rolled-out
//! tables, not just the original slice that `rls_isolation.rs` covers.
//!
//! This is PART 2 of the tenant-isolation CI gate. The static scan
//! (`console-gate-tenant-isolation`) proves every tenant table DECLARES org_id +
//! ENABLE/FORCE RLS + an org policy; this integration test proves those
//! declarations actually ISOLATE data at runtime as the non-owner `console_rt` role.
//!
//! For each representative table it asserts, running as `console_rt`:
//!   1. GUC = A → SELECT sees ONLY org A's row (B invisible); GUC = B → only B.
//!   2. GUC unset → ZERO rows (fail-closed).
//!   3. A cross-org INSERT (a row tagged org B while the GUC is armed to A) is
//!      rejected by the WITH CHECK policy.
//!
//! It deliberately covers a handful of tables (work_orders, messenger_messages,
//! registry_equipment, p1_dispatches, support_tickets, financial_rental_quotes),
//! one per major domain, to keep the test fast while generalizing the slice
//! proof across the rollout.

use sqlx::{PgPool, Row};
use uuid::Uuid;

const ORG_A: Uuid = Uuid::from_u128(0xA11A_A11A_A11A_A11A_A11A_A11A_A11A_A11A);
const ORG_B: Uuid = Uuid::from_u128(0xB22B_B22B_B22B_B22B_B22B_B22B_B22B_B22B);

/// The non-owner runtime role the application connects as in production. A
/// static literal so sqlx accepts it without an injection-audit override.
const SET_RUNTIME_ROLE: &str = "SET LOCAL ROLE console_rt";

/// The id of one seeded row per representative table, so the cross-org write
/// test can target a specific org-B row and the count test knows what to expect.
#[derive(Clone)]
struct SeededRollout {
    branch: Uuid,
    equipment: Uuid,
    work_order: Uuid,
    user: Uuid,
}

/// Arm the non-owner role + transaction-local tenant GUC, exactly as the app's
/// `with_audit` / `with_org_conn` helpers do after BEGIN.
async fn set_role_and_org(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, org: Option<Uuid>) {
    sqlx::query(SET_RUNTIME_ROLE)
        .execute(&mut **tx)
        .await
        .unwrap();
    if let Some(org) = org {
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org.to_string())
            .execute(&mut **tx)
            .await
            .unwrap();
    }
}

/// Seed one org across the representative rolled-out tables, as the unprivileged
/// runtime role with the tenant GUC armed (so each row passes WITH CHECK).
async fn seed_rollout(pool: &PgPool, org: Uuid, tag: &str) -> SeededRollout {
    // The org row is an OWNER/superuser operation (console_rt is SELECT-only on
    // organizations) — insert it as the pool role, which also bypasses RLS.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(org)
        .bind(format!("org-rollout-{}", tag.to_lowercase()))
        .bind(format!("Rollout Org {tag}"))
        .execute(&mut *tx)
        .await
        .unwrap();

    // Drop to console_rt + arm the GUC; every child row passes WITH CHECK as the app
    // would write it.
    set_role_and_org(&mut tx, Some(org)).await;

    let region = Uuid::new_v4();
    sqlx::query("INSERT INTO regions (id, name, org_id) VALUES ($1, $2, $3)")
        .bind(region)
        .bind(format!("Region {tag}"))
        .bind(org)
        .execute(&mut *tx)
        .await
        .unwrap();

    let branch = Uuid::new_v4();
    sqlx::query("INSERT INTO branches (id, region_id, name, org_id) VALUES ($1, $2, $3, $4)")
        .bind(branch)
        .bind(region)
        .bind(format!("Branch {tag}"))
        .bind(org)
        .execute(&mut *tx)
        .await
        .unwrap();

    let user = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(user)
        .bind(format!("User {tag}"))
        .bind(vec!["MECHANIC".to_string()])
        .bind(org)
        .execute(&mut *tx)
        .await
        .unwrap();

    let customer = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO registry_customers (id, branch_id, name, org_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(customer)
    .bind(branch)
    .bind(format!("Customer {tag}"))
    .bind(org)
    .execute(&mut *tx)
    .await
    .unwrap();

    let site = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO registry_sites (id, branch_id, customer_id, name, org_id) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(site)
    .bind(branch)
    .bind(customer)
    .bind(format!("Site {tag}"))
    .bind(org)
    .execute(&mut *tx)
    .await
    .unwrap();

    let equipment = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO registry_equipment \
            (id, branch_id, customer_id, site_id, equipment_no, manufacturer_code, \
             kind_code, power_code, status, specification, ton_text, source_sheet, \
             source_row, org_id) \
         VALUES ($1, $2, $3, $4, 'RLL01-0001', 'M', 'K', 'P', '임대', 'spec', '1t', 's', 1, $5)",
    )
    .bind(equipment)
    .bind(branch)
    .bind(customer)
    .bind(site)
    .bind(org)
    .execute(&mut *tx)
    .await
    .unwrap();

    let work_order = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO work_orders \
            (id, request_no, branch_id, equipment_id, customer_id, site_id, \
             requested_by, status, symptom, org_id) \
         VALUES ($1, '20260618-900', $2, $3, $4, $5, $6, 'RECEIVED', 'sym', $7)",
    )
    .bind(work_order)
    .bind(branch)
    .bind(equipment)
    .bind(customer)
    .bind(site)
    .bind(user)
    .bind(org)
    .execute(&mut *tx)
    .await
    .unwrap();

    // ── messenger: thread + message ─────────────────────────────────────────
    let thread = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO messenger_threads (id, kind, visibility, branch_id, work_order_id, created_by, org_id) \
         VALUES ($1, 'work_order', 'direct', $2, $3, $4, $5)",
    )
    .bind(thread)
    .bind(branch)
    .bind(work_order)
    .bind(user)
    .bind(org)
    .execute(&mut *tx)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO messenger_messages (thread_id, branch_id, sender_id, body, sent_at, org_id) \
         VALUES ($1, $2, $3, 'hello', now(), $4)",
    )
    .bind(thread)
    .bind(branch)
    .bind(user)
    .bind(org)
    .execute(&mut *tx)
    .await
    .unwrap();

    // ── p1 dispatch ─────────────────────────────────────────────────────────
    sqlx::query(
        "INSERT INTO p1_dispatches \
            (work_order_id, branch_id, status, accept_window_started_at, \
             accept_window_ends_at, created_by, created_at, updated_at, org_id) \
         VALUES ($1, $2, 'BROADCASTING', now(), now() + interval '1 hour', $3, now(), now(), $4)",
    )
    .bind(work_order)
    .bind(branch)
    .bind(user)
    .bind(org)
    .execute(&mut *tx)
    .await
    .unwrap();

    // ── support ticket (internal channel) ───────────────────────────────────
    sqlx::query(
        "INSERT INTO support_tickets \
            (branch_id, origin, category, priority, status, title, body, \
             requester_user_id, org_id) \
         VALUES ($1, 'INTERNAL', 'OPERATIONAL', 'LOW', 'OPEN', 'title', 'body', $2, $3)",
    )
    .bind(branch)
    .bind(user)
    .bind(org)
    .execute(&mut *tx)
    .await
    .unwrap();

    // ── financial rental quote ──────────────────────────────────────────────
    sqlx::query(
        "INSERT INTO financial_rental_quotes \
            (branch_id, equipment_id, created_by, acquisition_value_won, \
             current_residual_value_won, effective_residual_value_won, \
             cumulative_repair_cost_won, depreciation_method, useful_life_months, \
             residual_rate_bps, declining_balance_rate_bps, management_fee_rate_bps, \
             profit_rate_bps, floor_negative_quote_residual, monthly_total_won, \
             created_at, org_id) \
         VALUES ($1, $2, $3, 1000, 100, 100, 0, 'STRAIGHT_LINE', 60, \
                 1000, 1000, 1000, 1000, true, 500, now(), $4)",
    )
    .bind(branch)
    .bind(equipment)
    .bind(user)
    .bind(org)
    .execute(&mut *tx)
    .await
    .unwrap();

    tx.commit().await.unwrap();
    SeededRollout {
        branch,
        equipment,
        work_order,
        user,
    }
}

/// Count rows in `count_query` (a static literal) as the runtime role with the
/// tenant GUC set to `org`, or left unset when `org` is None (fail-closed path).
async fn count_as_runtime(pool: &PgPool, org: Option<Uuid>, count_query: &'static str) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    set_role_and_org(&mut tx, org).await;
    let count: i64 = sqlx::query_scalar(count_query)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    count
}

/// Each representative table with the static COUNT(*) the isolation assertions
/// use. One per major rolled-out domain.
fn representative_counts() -> &'static [(&'static str, &'static str)] {
    &[
        ("work_orders", "SELECT count(*) FROM work_orders"),
        (
            "registry_equipment",
            "SELECT count(*) FROM registry_equipment",
        ),
        (
            "messenger_messages",
            "SELECT count(*) FROM messenger_messages",
        ),
        ("p1_dispatches", "SELECT count(*) FROM p1_dispatches"),
        ("support_tickets", "SELECT count(*) FROM support_tickets"),
        (
            "financial_rental_quotes",
            "SELECT count(*) FROM financial_rental_quotes",
        ),
    ]
}

#[sqlx::test(migrations = "./migrations")]
async fn rollout_rls_isolates_two_tenants_across_representative_tables(pool: PgPool) {
    seed_rollout(&pool, ORG_A, "A").await;
    seed_rollout(&pool, ORG_B, "B").await;

    for (table, count_query) in representative_counts() {
        // (1) GUC = A → exactly org A's one row; GUC = B → exactly org B's.
        assert_eq!(
            count_as_runtime(&pool, Some(ORG_A), count_query).await,
            1,
            "org A must see exactly its own {table} row"
        );
        assert_eq!(
            count_as_runtime(&pool, Some(ORG_B), count_query).await,
            1,
            "org B must see exactly its own {table} row"
        );

        // (2) Unset GUC → ZERO rows (fail-closed).
        assert_eq!(
            count_as_runtime(&pool, None, count_query).await,
            0,
            "unset GUC must reveal ZERO {table} rows (fail-closed)"
        );
    }
}

/// Assert that running `insert_sql` (a static literal that tags the new row with
/// org B) under GUC = A is rejected by the WITH CHECK policy. sqlx 0.9 only
/// accepts `&'static str` to keep SQL injection-audited, so each case is a
/// literal closure rather than a dynamic string.
async fn assert_cross_org_insert_rejected(
    pool: &PgPool,
    table: &str,
    bind_and_run: impl AsyncFnOnce(
        &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), sqlx::Error>,
) {
    let mut tx = pool.begin().await.unwrap();
    set_role_and_org(&mut tx, Some(ORG_A)).await;
    let err = bind_and_run(&mut tx)
        .await
        .expect_err(&format!("cross-org INSERT into {table} must be rejected"))
        .to_string();
    assert!(
        err.contains("row-level security"),
        "cross-org INSERT into {table} must be rejected by the RLS policy, got: {err}"
    );
    let _ = tx.rollback().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn rollout_cross_org_inserts_are_rejected(pool: PgPool) {
    let a = seed_rollout(&pool, ORG_A, "A").await;
    seed_rollout(&pool, ORG_B, "B").await;

    // (3) Under GUC = A, INSERT a row tagged org B into each representative table
    // → rejected by the WITH CHECK policy. We reuse org A's own visible parent
    // rows (branch/equipment/user/work_order) so only the org_id mismatch — not a
    // missing FK — triggers the rejection.

    // messenger_messages: borrow org A's visible thread.
    assert_cross_org_insert_rejected(&pool, "messenger_messages", async |tx| {
        sqlx::query(
            "INSERT INTO messenger_messages (thread_id, branch_id, sender_id, body, sent_at, org_id) \
             SELECT t.id, $1, $2, 'x', now(), $3 FROM messenger_threads t \
             WHERE t.branch_id = $1 LIMIT 1",
        )
        .bind(a.branch)
        .bind(a.user)
        .bind(ORG_B)
        .execute(&mut **tx)
        .await
        .map(|_| ())
    })
    .await;

    // support_tickets (internal channel).
    assert_cross_org_insert_rejected(&pool, "support_tickets", async |tx| {
        sqlx::query(
            "INSERT INTO support_tickets \
                (branch_id, origin, category, priority, status, title, body, requester_user_id, org_id) \
             VALUES ($1, 'INTERNAL', 'OPERATIONAL', 'LOW', 'OPEN', 't', 'b', $2, $3)",
        )
        .bind(a.branch)
        .bind(a.user)
        .bind(ORG_B)
        .execute(&mut **tx)
        .await
        .map(|_| ())
    })
    .await;

    // financial_rental_quotes.
    assert_cross_org_insert_rejected(&pool, "financial_rental_quotes", async |tx| {
        sqlx::query(
            "INSERT INTO financial_rental_quotes \
                (branch_id, equipment_id, created_by, acquisition_value_won, \
                 current_residual_value_won, effective_residual_value_won, \
                 cumulative_repair_cost_won, depreciation_method, useful_life_months, \
                 residual_rate_bps, declining_balance_rate_bps, management_fee_rate_bps, \
                 profit_rate_bps, floor_negative_quote_residual, monthly_total_won, \
                 created_at, org_id) \
             VALUES ($1, $2, $3, 1000, 100, 100, 0, 'STRAIGHT_LINE', 60, \
                     1000, 1000, 1000, 1000, true, 500, now(), $4)",
        )
        .bind(a.branch)
        .bind(a.equipment)
        .bind(a.user)
        .bind(ORG_B)
        .execute(&mut **tx)
        .await
        .map(|_| ())
    })
    .await;

    // work_orders: tagging a new work_order with org B under GUC = A must fail
    // WITH CHECK. Reuse org A's visible customer/site (looked up under GUC = A).
    {
        let mut tx = pool.begin().await.unwrap();
        set_role_and_org(&mut tx, Some(ORG_A)).await;
        let row = sqlx::query("SELECT customer_id, site_id FROM work_orders WHERE id = $1")
            .bind(a.work_order)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let customer: Uuid = row.get("customer_id");
        let site: Uuid = row.get("site_id");
        let err = sqlx::query(
            "INSERT INTO work_orders \
                (request_no, branch_id, equipment_id, customer_id, site_id, \
                 requested_by, status, symptom, org_id) \
             VALUES ('20260618-901', $1, $2, $3, $4, $5, 'RECEIVED', 'sym', $6)",
        )
        .bind(a.branch)
        .bind(a.equipment)
        .bind(customer)
        .bind(site)
        .bind(a.user)
        .bind(ORG_B)
        .execute(&mut *tx)
        .await
        .expect_err("cross-org work_orders INSERT must be rejected")
        .to_string();
        assert!(
            err.contains("row-level security"),
            "cross-org work_orders INSERT must be rejected, got: {err}"
        );
        let _ = tx.rollback().await;
    }
}
