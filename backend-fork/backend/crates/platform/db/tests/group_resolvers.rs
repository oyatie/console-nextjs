#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Runtime-role proof for the org-hierarchy P0 SECURITY DEFINER resolvers.
//!
//! The group membership and group-role tables are cross-tenant authorization
//! metadata. They must stay owner-only: production `console_rt` may call narrow
//! identity resolvers, but it must not read the raw tables.

use sqlx::{PgPool, Row};
use uuid::Uuid;

const ORG_A: Uuid = Uuid::from_u128(0xA002_A002_A002_A002_A002_A002_A002_A002);
const ORG_B: Uuid = Uuid::from_u128(0xB002_B002_B002_B002_B002_B002_B002_B002);
const GROUP: Uuid = Uuid::from_u128(0x9002_9002_9002_9002_9002_9002_9002_9002);
const GROUP_ADMIN: Uuid = Uuid::from_u128(0x1002_1002_1002_1002_1002_1002_1002_1002);
const OUTSIDER: Uuid = Uuid::from_u128(0x2002_2002_2002_2002_2002_2002_2002_2002);
const SET_RUNTIME_ROLE: &str = "SET LOCAL ROLE console_rt";

async fn seed_group(pool: &PgPool) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES \
            ($1, 'g002-a', 'G002 A'), ($2, 'g002-b', 'G002 B')",
    )
    .bind(ORG_A)
    .bind(ORG_B)
    .execute(&mut *tx)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO users (id, display_name, roles, org_id) VALUES \
            ($1, 'Group Admin', ARRAY['ADMIN'], $3), \
            ($2, 'Outsider', ARRAY['ADMIN'], $4)",
    )
    .bind(GROUP_ADMIN)
    .bind(OUTSIDER)
    .bind(ORG_A)
    .bind(ORG_B)
    .execute(&mut *tx)
    .await
    .unwrap();

    sqlx::query("INSERT INTO groups (id, slug, name) VALUES ($1, 'coss-family', 'COSS Family')")
        .bind(GROUP)
        .execute(&mut *tx)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO group_memberships (group_id, org_id) VALUES ($1, $2), ($1, $3)
         ON CONFLICT (org_id) DO UPDATE SET group_id = EXCLUDED.group_id",
    )
    .bind(GROUP)
    .bind(ORG_A)
    .bind(ORG_B)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("UPDATE organizations SET group_id = $1 WHERE id IN ($2, $3)")
        .bind(GROUP)
        .bind(ORG_A)
        .bind(ORG_B)
        .execute(&mut *tx)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO group_role_grants (group_id, user_id, group_role, granted_by) \
         VALUES ($1, $2, 'GROUP_ADMIN', NULL)",
    )
    .bind(GROUP)
    .bind(GROUP_ADMIN)
    .execute(&mut *tx)
    .await
    .unwrap();

    tx.commit().await.unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn group_member_resolver_returns_only_authorized_group_members(pool: PgPool) {
    seed_group(&pool).await;

    let mut tx = pool.begin().await.unwrap();
    sqlx::query(SET_RUNTIME_ROLE)
        .execute(&mut *tx)
        .await
        .unwrap();

    let rows = sqlx::query("SELECT org_id, slug FROM group_member_org_ids($1, $2)")
        .bind(GROUP)
        .bind(GROUP_ADMIN)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    let org_ids: Vec<Uuid> = rows.iter().map(|row| row.get("org_id")).collect();
    assert_eq!(org_ids, vec![ORG_A, ORG_B]);

    let outsider_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_member_org_ids($1, $2)")
            .bind(GROUP)
            .bind(OUTSIDER)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        outsider_count, 0,
        "users without a group grant see no members"
    );

    let row_security: String = sqlx::query_scalar("SHOW row_security")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(row_security, "on", "resolver must restore row_security");
}

#[sqlx::test(migrations = "./migrations")]
async fn runtime_role_can_resolve_own_grants_but_not_read_raw_group_auth_tables(pool: PgPool) {
    seed_group(&pool).await;

    let mut tx = pool.begin().await.unwrap();
    sqlx::query(SET_RUNTIME_ROLE)
        .execute(&mut *tx)
        .await
        .unwrap();

    let roles: Vec<String> =
        sqlx::query_scalar("SELECT group_role FROM group_role_grants_for_user($1)")
            .bind(GROUP_ADMIN)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(roles, vec!["GROUP_ADMIN".to_string()]);

    let no_roles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM group_role_grants_for_user($1)")
        .bind(OUTSIDER)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(no_roles, 0);

    let memberships_err = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM group_memberships")
        .fetch_one(&mut *tx)
        .await
        .expect_err("console_rt must not read owner-only group_memberships")
        .to_string();
    assert!(
        memberships_err.contains("permission denied"),
        "raw group_memberships read as console_rt must be denied, got: {memberships_err}"
    );
    let _ = tx.rollback().await;

    let mut tx = pool.begin().await.unwrap();
    sqlx::query(SET_RUNTIME_ROLE)
        .execute(&mut *tx)
        .await
        .unwrap();
    let grants_err = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM group_role_grants")
        .fetch_one(&mut *tx)
        .await
        .expect_err("console_rt must not read owner-only group_role_grants")
        .to_string();
    assert!(
        grants_err.contains("permission denied"),
        "raw group_role_grants read as console_rt must be denied, got: {grants_err}"
    );
}
