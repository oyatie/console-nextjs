#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! Every shipped `Feature` must be grantable.
//!
//! `policy_role_permissions.feature_key` FKs to `feature_catalog` (0065), so a `Feature` with no
//! catalog row cannot be granted to any tenant role — whatever the compile-time matrix says it
//! permits. That failure is silent in both directions: nothing panics, no route 500s, the
//! capability simply cannot be handed to anyone.
//!
//! Eleven keys were in exactly that state until migration 0209, including `payroll_run_read` and
//! `payroll_run_manage` — which together are the separation-of-duties split for payroll, so no
//! reviewer-who-cannot-pay role could exist — and `approval_finalize`.
//!
//! This asserts one direction only: every `Feature` has a row. The reverse (a catalog row with no
//! `Feature`) is deliberately not asserted. Thirteen such rows exist, and deleting one would break
//! the FK of any grant that already names it; a row no code reads is inert, whereas a missing row
//! removes capability. Only the second is worth failing a build over.

use sqlx::PgPool;

use console_platform_authz::Feature;

#[sqlx::test(migrations = "./migrations")]
async fn every_feature_has_a_catalog_row(pool: PgPool) {
    let catalog: Vec<String> = sqlx::query_scalar("SELECT feature_key FROM feature_catalog")
        .fetch_all(&pool)
        .await
        .expect("read feature_catalog");

    let missing: Vec<&str> = Feature::ALL
        .iter()
        .map(|feature| feature.as_str())
        .filter(|key| !catalog.iter().any(|row| row == key))
        .collect();

    assert!(
        missing.is_empty(),
        "{} Feature(s) have no feature_catalog row and are therefore ungrantable through \
         policy_role_permissions, no matter what the compile-time matrix permits: {missing:?}",
        missing.len(),
    );
}

/// The catalog is not allowed to be empty or trivially small, which would make the assertion
/// above pass for the wrong reason if a migration ever truncated it.
#[sqlx::test(migrations = "./migrations")]
async fn the_catalog_is_populated_rather_than_vacuously_satisfied(pool: PgPool) {
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM feature_catalog")
        .fetch_one(&pool)
        .await
        .expect("count feature_catalog");

    assert!(
        rows >= Feature::ALL.len() as i64,
        "feature_catalog holds {rows} rows but there are {} Features; the coverage assertion \
         above would still pass on a truncated catalog only if Feature::ALL were also empty",
        Feature::ALL.len(),
    );
}
