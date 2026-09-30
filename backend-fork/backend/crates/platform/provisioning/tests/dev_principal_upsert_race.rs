//! Proves `DevPrincipalProvisioner::upsert` is race-safe for the FIRST mint of
//! a given (org, role): two concurrent callers must not both try to INSERT and
//! 500 on the `idx_users_phone_unique_present` unique-index conflict. A
//! `SELECT ... FOR UPDATE` on zero matching rows locks nothing, so this only
//! holds with the `INSERT ... ON CONFLICT (phone) DO UPDATE` rewrite.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_kernel_core::OrgId;
use console_platform_provisioning::{DevPrincipalProvisioner, DevPrincipalRequest};
use sqlx::PgPool;
use time::OffsetDateTime;

#[sqlx::test(migrations = "../db/migrations")]
async fn concurrent_first_mints_for_the_same_org_role_never_error(pool: PgPool) {
    let now = OffsetDateTime::now_utc();
    let request = || DevPrincipalRequest {
        org_id: OrgId::knl(),
        display_name: "Concurrent Dev".to_owned(),
        role: "MECHANIC".to_owned(),
        branch_ids: Vec::new(),
    };

    let (first, second) = tokio::join!(
        DevPrincipalProvisioner.upsert(&pool, request(), now),
        DevPrincipalProvisioner.upsert(&pool, request(), now),
    );

    let first = first.expect("first concurrent upsert must not error");
    let second = second.expect("second concurrent upsert must not error");
    assert_eq!(
        first.user_id, second.user_id,
        "both callers must resolve to the SAME dev principal row"
    );

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users WHERE org_id = $1 AND phone LIKE 'dev-auth:%'",
    )
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "exactly one row, not a duplicate");
}
