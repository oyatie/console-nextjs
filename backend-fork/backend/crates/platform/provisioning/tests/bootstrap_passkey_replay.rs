//! Regression test for atomic single-use OTP consumption.
//!
//! A redeem is verify-only and never consumes, so two concurrent redeems both
//! succeed. The single-use invariant lives at CONSUMPTION (passkey registration):
//! two concurrent `consume_open_credentials_tx` calls burn the code EXACTLY once —
//! the harden-1 atomic `UPDATE ... WHERE consumed_at IS NULL RETURNING` makes the
//! losing call match 0 rows, so the credential is consumed once and audited once.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use console_kernel_core::OrgId;
use console_platform_provisioning::BootstrapCredentialStore;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tokio::sync::Barrier;

async fn seed_user(pool: &PgPool) -> uuid::Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (display_name, phone, roles, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind("Cold Start Replay User")
    .bind("010-3000-9999")
    .bind(Vec::<String>::from(["MECHANIC".to_owned()]))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Concurrent redeems both succeed (verify-only); concurrent registration-consumes
/// burn the code EXACTLY once.
#[sqlx::test(migrations = "../db/migrations")]
async fn concurrent_consume_burns_the_otp_exactly_once(pool: PgPool) {
    let user_id = seed_user(&pool).await;
    let store = BootstrapCredentialStore;
    let now = OffsetDateTime::now_utc();

    let issue = store
        .issue_for_zero_credential_user(&pool, user_id, OrgId::knl(), now, Duration::hours(24))
        .await
        .unwrap();

    // Two concurrent redeems both succeed: a redeem verifies, it never consumes.
    let barrier = Arc::new(Barrier::new(2));
    let (store_a, store_b) = (store, store);
    let (pool_a, pool_b) = (pool.clone(), pool.clone());
    let (token_a, token_b) = (
        issue.token.as_str().to_owned(),
        issue.token.as_str().to_owned(),
    );
    let (barrier_a, barrier_b) = (Arc::clone(&barrier), Arc::clone(&barrier));
    let handle_a = tokio::spawn(async move {
        barrier_a.wait().await;
        store_a.redeem_otp(&pool_a, &token_a, now).await
    });
    let handle_b = tokio::spawn(async move {
        barrier_b.wait().await;
        store_b.redeem_otp(&pool_b, &token_b, now).await
    });
    assert!(
        handle_a.await.unwrap().is_ok() && handle_b.await.unwrap().is_ok(),
        "verify-only redeems must both succeed concurrently"
    );

    let consumed_at: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_bootstrap_credentials WHERE id = $1")
            .bind(issue.credential_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(consumed_at.is_none(), "a redeem must not consume the code");

    // Two concurrent registration-consumes: exactly one matches the open row.
    let barrier = Arc::new(Barrier::new(2));
    let (store_a, store_b) = (store, store);
    let (pool_a, pool_b) = (pool.clone(), pool.clone());
    let (barrier_a, barrier_b) = (Arc::clone(&barrier), Arc::clone(&barrier));
    let consume_a = tokio::spawn(async move {
        barrier_a.wait().await;
        let mut tx = pool_a.begin().await.unwrap();
        store_a
            .consume_open_credentials_tx(&mut tx, OrgId::knl(), user_id, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    });
    let consume_b = tokio::spawn(async move {
        barrier_b.wait().await;
        let mut tx = pool_b.begin().await.unwrap();
        store_b
            .consume_open_credentials_tx(&mut tx, OrgId::knl(), user_id, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    });
    consume_a.await.unwrap();
    consume_b.await.unwrap();

    let consumed_at: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_bootstrap_credentials WHERE id = $1")
            .bind(issue.credential_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(consumed_at.is_some(), "registration consumes the code");

    // Exactly one consume audit: only one concurrent consume matched the open row.
    let consume_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'auth.otp.consume' AND target_id = $1",
    )
    .bind(issue.credential_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        consume_audits, 1,
        "the code must be consumed exactly once, even under concurrency"
    );

    // A subsequent redeem is rejected: the code is dead.
    assert!(
        store
            .redeem_otp(&pool, issue.token.as_str(), now)
            .await
            .is_err(),
        "a consumed code must not redeem again"
    );
}
