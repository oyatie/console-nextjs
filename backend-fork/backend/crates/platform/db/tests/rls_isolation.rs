#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Acceptance gate for multi-tenant phase 1: Postgres RLS proves end-to-end
//! tenant isolation on the vertical slice (organizations → regions → branches →
//! users → registry → work_orders) **while running as a genuine NON-OWNER
//! login-equivalent role**.
//!
//! ## Why the test switches role — and to which role
//! RLS is enforced only for roles that are NOT superusers and do NOT carry
//! BYPASSRLS. `sqlx::test` connects as the database owner/superuser, which
//! bypasses RLS. Production connects as `console_rt` — the least-privilege RUNTIME
//! role (migration 0031): NOSUPERUSER, NOBYPASSRLS, owns nothing. Every
//! tenant-scoped statement below runs inside a transaction that first
//! `SET LOCAL ROLE console_rt` and then arms `app.current_org`, so the test sees
//! exactly what production sees. `FORCE ROW LEVEL SECURITY` additionally
//! subjects the table owner to the policies, closing the owner-bypass hole.
//!
//! Crucially, the DDL-denial block runs as `console_rt` too: a non-owner cannot
//! `DROP POLICY` / `DISABLE ROW LEVEL SECURITY` / `DISABLE TRIGGER`. That is the
//! regression guard for the CRITICAL "de-own the runtime role" fix — if the app
//! ever reverts to connecting as the owner, those statements would succeed and
//! this test would fail.
//!
//! ## What it asserts (definition of done)
//!  1. GUC = A → SELECT returns ONLY org A's rows (B invisible), and vice versa.
//!  2. GUC unset → ZERO rows; empty-string GUC → ZERO rows (fail-closed, both).
//!  3. cross-org INSERT (org_id = B under GUC = A) rejected by WITH CHECK.
//!  4. cross-org UPDATE-move (SET org_id = B under GUC = A) rejected.
//!  5. cross-org UPDATE/DELETE of an org-B row under GUC = A affects 0 rows.
//!  6. org_id immutability: SET org_id = <same A> allowed; changing it fires the
//!     trigger and aborts.
//!  7. DDL-denial as the non-owner role: DROP POLICY, DISABLE ROW LEVEL
//!     SECURITY, DISABLE TRIGGER on audit_events all raise insufficient-privilege.

use console_kernel_core::OrgId;
use console_platform_db::{DbError, with_org_conn, with_org_snapshot};
use sqlx::{PgPool, Row};
use tokio::sync::oneshot;
use uuid::Uuid;

const ORG_A: Uuid = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);
const ORG_B: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);

/// The non-owner runtime role the application connects as in production.
/// A static literal so sqlx accepts it without an injection-audit override.
const SET_RUNTIME_ROLE: &str = "SET LOCAL ROLE console_rt";

/// Seed one org plus a full slice (region → branch → user → customer → site →
/// equipment → work_order) as the unprivileged runtime role with the tenant GUC
/// armed, so the rows pass the WITH CHECK policy on the way in. A *known* region
/// id is returned so cross-org write tests can target an org-B row by id.
struct Seeded {
    region: Uuid,
}

async fn seed_org(pool: &PgPool, org: Uuid, tag: &str) -> Seeded {
    // Provisioning an organization is an OWNER operation (console_rt has SELECT-only
    // on organizations), so insert the org row as the owner/superuser pool role
    // — which also bypasses RLS — BEFORE dropping to console_rt for the tenant rows.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(org)
        .bind(format!("org-{}", tag.to_lowercase()))
        .bind(format!("Org {tag}"))
        .execute(&mut *tx)
        .await
        .unwrap();

    // Now drop to the non-owner runtime role + arm the tenant GUC; every child
    // row below passes the WITH CHECK policy as the app would write it.
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
    // equipment_no is intentionally IDENTICAL across orgs to prove the unique
    // key is now per-org, not global.
    sqlx::query(
        "INSERT INTO registry_equipment \
            (id, branch_id, customer_id, site_id, equipment_no, manufacturer_code, \
             kind_code, power_code, status, specification, ton_text, source_sheet, \
             source_row, org_id) \
         VALUES ($1, $2, $3, $4, 'ABC01-0001', 'M', 'K', 'P', '임대', 'spec', '1t', 's', 1, $5)",
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
    // request_no is also identical across orgs — same per-org-unique proof.
    sqlx::query(
        "INSERT INTO work_orders \
            (id, request_no, branch_id, equipment_id, customer_id, site_id, \
             requested_by, status, symptom, org_id) \
         VALUES ($1, '20260618-001', $2, $3, $4, $5, $6, 'RECEIVED', 'sym', $7)",
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

    tx.commit().await.unwrap();
    Seeded { region }
}

/// Drop to the non-owner runtime role and (optionally) arm the tenant GUC.
/// `org = None` leaves the GUC unset (fail-closed path); `org = Some("")` is the
/// empty-string fail-closed path, handled by the dedicated helper below.
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

/// What GUC state to put the transaction in for a fail-closed assertion.
enum Guc {
    Set(Uuid),
    Unset,
    Empty,
}

/// Run a `SELECT count(*)` (static, injection-safe literal) as the runtime role
/// with the tenant GUC in the requested state.
async fn count_as_runtime(pool: &PgPool, guc: Guc, count_query: &'static str) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(SET_RUNTIME_ROLE)
        .execute(&mut *tx)
        .await
        .unwrap();
    match guc {
        Guc::Set(org) => {
            sqlx::query("SELECT set_config('app.current_org', $1, true)")
                .bind(org.to_string())
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        Guc::Empty => {
            sqlx::query("SELECT set_config('app.current_org', '', true)")
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        Guc::Unset => {}
    }
    let count: i64 = sqlx::query_scalar(count_query)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    count
}

const COUNT_WORK_ORDERS: &str = "SELECT count(*) FROM work_orders";
const COUNT_EQUIPMENT: &str = "SELECT count(*) FROM registry_equipment";
const COUNT_REGIONS: &str = "SELECT count(*) FROM regions";

#[sqlx::test(migrations = "./migrations")]
async fn rls_isolates_two_tenants_and_fails_closed(pool: PgPool) {
    seed_org(&pool, ORG_A, "A").await;
    seed_org(&pool, ORG_B, "B").await;

    // (1) GUC = A → only A's single row per table; GUC = B → only B's.
    assert_eq!(
        count_as_runtime(&pool, Guc::Set(ORG_A), COUNT_WORK_ORDERS).await,
        1,
        "org A must see exactly its own work_order"
    );
    assert_eq!(
        count_as_runtime(&pool, Guc::Set(ORG_A), COUNT_EQUIPMENT).await,
        1,
        "org A must see exactly its own equipment"
    );
    assert_eq!(
        count_as_runtime(&pool, Guc::Set(ORG_B), COUNT_WORK_ORDERS).await,
        1,
        "org B must see exactly its own work_order"
    );
    assert_eq!(
        count_as_runtime(&pool, Guc::Set(ORG_B), COUNT_EQUIPMENT).await,
        1,
        "org B must see exactly its own equipment"
    );

    // (2) Fail-closed: unset GUC → ZERO; empty-string GUC → ZERO. Both
    // directions of the NULLIF(..., '') collapse in the policy.
    assert_eq!(
        count_as_runtime(&pool, Guc::Unset, COUNT_WORK_ORDERS).await,
        0,
        "unset GUC must reveal ZERO work_orders (fail-closed)"
    );
    assert_eq!(
        count_as_runtime(&pool, Guc::Unset, COUNT_EQUIPMENT).await,
        0,
        "unset GUC must reveal ZERO equipment (fail-closed)"
    );
    assert_eq!(
        count_as_runtime(&pool, Guc::Empty, COUNT_WORK_ORDERS).await,
        0,
        "empty-string GUC must reveal ZERO work_orders (fail-closed)"
    );
    assert_eq!(
        count_as_runtime(&pool, Guc::Empty, COUNT_REGIONS).await,
        0,
        "empty-string GUC must reveal ZERO regions (fail-closed)"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn cross_org_writes_are_rejected(pool: PgPool) {
    seed_org(&pool, ORG_A, "A").await;
    let seeded_b = seed_org(&pool, ORG_B, "B").await;

    // (3) GUC = A, INSERT a row tagged org B → rejected by WITH CHECK.
    {
        let mut tx = pool.begin().await.unwrap();
        set_role_and_org(&mut tx, Some(ORG_A)).await;
        let res = sqlx::query("INSERT INTO regions (name, org_id) VALUES ('smuggled', $1)")
            .bind(ORG_B)
            .execute(&mut *tx)
            .await;
        let err = res
            .expect_err("cross-org INSERT must be rejected")
            .to_string();
        assert!(
            err.contains("row-level security"),
            "cross-org INSERT must be rejected by the RLS policy, got: {err}"
        );
        let _ = tx.rollback().await;
    }

    // (4) GUC = A, UPDATE-move org A's region to org B → rejected. We update by
    // name (org A's own row is visible under GUC = A); the new org_id = B fails
    // the WITH CHECK.
    {
        let mut tx = pool.begin().await.unwrap();
        set_role_and_org(&mut tx, Some(ORG_A)).await;
        let res = sqlx::query("UPDATE regions SET org_id = $1 WHERE name = 'Region A'")
            .bind(ORG_B)
            .execute(&mut *tx)
            .await;
        let err = res
            .expect_err("cross-org UPDATE-move must be rejected")
            .to_string();
        assert!(
            // Either the RLS WITH CHECK or the immutability trigger may fire
            // first; both are correct rejections of a tenant move.
            err.contains("row-level security") || err.contains("org_id is immutable"),
            "UPDATE-move to another org must be rejected, got: {err}"
        );
        let _ = tx.rollback().await;
    }

    // (5) GUC = A, UPDATE/DELETE an org-B row by id → 0 rows affected (the row
    // is invisible under A's USING clause, so the statement matches nothing).
    {
        let mut tx = pool.begin().await.unwrap();
        set_role_and_org(&mut tx, Some(ORG_A)).await;
        let updated = sqlx::query("UPDATE regions SET name = 'hijacked' WHERE id = $1")
            .bind(seeded_b.region)
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected();
        assert_eq!(
            updated, 0,
            "UPDATE of an org-B row under GUC = A must affect 0 rows"
        );

        let deleted = sqlx::query("DELETE FROM regions WHERE id = $1")
            .bind(seeded_b.region)
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected();
        assert_eq!(
            deleted, 0,
            "DELETE of an org-B row under GUC = A must affect 0 rows"
        );
        tx.commit().await.unwrap();
    }

    // org B's region still exists and is unmodified (proves 5 was a true no-op).
    let still_there = count_as_runtime(&pool, Guc::Set(ORG_B), COUNT_REGIONS).await;
    assert_eq!(
        still_there, 1,
        "org B's region must survive org A's attempts"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn org_id_is_immutable(pool: PgPool) {
    seed_org(&pool, ORG_A, "A").await;

    // (6a) Re-stating the SAME org_id is allowed (NEW IS NOT DISTINCT FROM OLD).
    {
        let mut tx = pool.begin().await.unwrap();
        set_role_and_org(&mut tx, Some(ORG_A)).await;
        let affected = sqlx::query("UPDATE regions SET org_id = $1 WHERE name = 'Region A'")
            .bind(ORG_A)
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected();
        assert_eq!(
            affected, 1,
            "setting org_id to its current value must be a normal UPDATE"
        );
        tx.commit().await.unwrap();
    }

    // (6b) Changing org_id to ANY other value fires the trigger and aborts —
    // even to an org the row's writer does not control. Using a third uuid
    // isolates the trigger from the RLS WITH CHECK (a value never seeded).
    {
        let third = Uuid::from_u128(0x3333_3333_3333_3333_3333_3333_3333_3333);
        let mut tx = pool.begin().await.unwrap();
        set_role_and_org(&mut tx, Some(ORG_A)).await;
        let res = sqlx::query("UPDATE regions SET org_id = $1 WHERE name = 'Region A'")
            .bind(third)
            .execute(&mut *tx)
            .await;
        let err = res
            .expect_err("changing org_id must be rejected")
            .to_string();
        assert!(
            err.contains("org_id is immutable") || err.contains("row-level security"),
            "changing org_id must be rejected by the immutability trigger, got: {err}"
        );
        let _ = tx.rollback().await;
    }
}

/// (7) DDL-denial — the regression guard for the CRITICAL de-owning fix. As the
/// NON-OWNER runtime role, attempts to dismantle the tenant boundary must all
/// raise insufficient-privilege. If the app ever reverts to connecting as the
/// table owner, these would succeed and this test fails.
#[sqlx::test(migrations = "./migrations")]
async fn non_owner_cannot_disable_the_tenant_boundary(pool: PgPool) {
    // DROP POLICY org_isolation ON work_orders.
    {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(SET_RUNTIME_ROLE)
            .execute(&mut *tx)
            .await
            .unwrap();
        let err = sqlx::query("DROP POLICY org_isolation ON work_orders")
            .execute(&mut *tx)
            .await
            .expect_err("non-owner must not DROP POLICY")
            .to_string();
        assert!(
            err.contains("must be owner") || err.contains("permission denied"),
            "DROP POLICY as non-owner must be denied, got: {err}"
        );
        let _ = tx.rollback().await;
    }

    // ALTER TABLE work_orders DISABLE ROW LEVEL SECURITY.
    {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(SET_RUNTIME_ROLE)
            .execute(&mut *tx)
            .await
            .unwrap();
        let err = sqlx::query("ALTER TABLE work_orders DISABLE ROW LEVEL SECURITY")
            .execute(&mut *tx)
            .await
            .expect_err("non-owner must not DISABLE ROW LEVEL SECURITY")
            .to_string();
        assert!(
            err.contains("must be owner") || err.contains("permission denied"),
            "DISABLE ROW LEVEL SECURITY as non-owner must be denied, got: {err}"
        );
        let _ = tx.rollback().await;
    }

    // ALTER TABLE audit_events DISABLE TRIGGER <real name from 0003>.
    {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(SET_RUNTIME_ROLE)
            .execute(&mut *tx)
            .await
            .unwrap();
        let err =
            sqlx::query("ALTER TABLE audit_events DISABLE TRIGGER trg_audit_events_no_update")
                .execute(&mut *tx)
                .await
                .expect_err("non-owner must not DISABLE TRIGGER on audit_events")
                .to_string();
        assert!(
            err.contains("must be owner") || err.contains("permission denied"),
            "DISABLE TRIGGER on audit_events as non-owner must be denied, got: {err}"
        );
        let _ = tx.rollback().await;
    }
}

/// Proves the `with_org_conn` propagation helper actually arms
/// `app.current_org` for the connection it hands to the closure.
#[sqlx::test(migrations = "./migrations")]
async fn with_org_conn_sets_the_current_org_guc(pool: PgPool) {
    let org = OrgId::from_uuid(ORG_A);
    let seen: String = with_org_conn(&pool, org, |tx| {
        Box::pin(async move {
            let row = sqlx::query("SELECT current_setting('app.current_org', true) AS v")
                .fetch_one(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;
            Ok::<String, DbError>(row.get::<Option<String>, _>("v").unwrap_or_default())
        })
    })
    .await
    .unwrap();

    assert_eq!(
        seen,
        ORG_A.to_string(),
        "with_org_conn must set app.current_org"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn with_org_snapshot_arms_rls_before_repeatable_read_only_queries(pool: PgPool) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, 'snapshot-org', 'Before')")
        .bind(ORG_A)
        .execute(&pool)
        .await
        .unwrap();
    let (first_read_tx, first_read_rx) = oneshot::channel();
    let (updated_tx, updated_rx) = oneshot::channel();
    let writer_pool = pool.clone();
    let writer = tokio::spawn(async move {
        first_read_rx.await.unwrap();
        sqlx::query("UPDATE organizations SET name = 'After' WHERE id = $1")
            .bind(ORG_A)
            .execute(&writer_pool)
            .await
            .unwrap();
        updated_tx.send(()).unwrap();
    });
    let observed: (String, String, String) =
        with_org_snapshot(&pool, OrgId::from_uuid(ORG_A), |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT current_setting('app.current_org', true) AS org, \
                            current_setting('transaction_isolation') AS isolation, \
                            current_setting('transaction_read_only') AS read_only",
                )
                .fetch_one(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;
                let first_name: String =
                    sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
                        .bind(ORG_A)
                        .fetch_one(tx.as_mut())
                        .await
                        .map_err(DbError::Sqlx)?;
                first_read_tx.send(()).unwrap();
                updated_rx.await.unwrap();
                let second_name: String =
                    sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
                        .bind(ORG_A)
                        .fetch_one(tx.as_mut())
                        .await
                        .map_err(DbError::Sqlx)?;
                assert_eq!(
                    (first_name.as_str(), second_name.as_str()),
                    ("Before", "Before")
                );
                Ok::<_, DbError>((row.get("org"), row.get("isolation"), row.get("read_only")))
            })
        })
        .await
        .unwrap();
    writer.await.unwrap();

    assert_eq!(
        observed,
        (ORG_A.to_string(), "repeatable read".into(), "on".into())
    );
}
