//! A successful PostgreSQL COMMIT is not a two-site durability receipt.
//! Run against an isolated PostgreSQL 18 primary with no physical standby.

use axum::body::Body;
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;

#[sqlx::test]
async fn committed_mutation_is_not_ready_without_independent_replica(
    pool: PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let is_standby: bool = sqlx::query_scalar("SELECT pg_is_in_recovery()")
        .fetch_one(&pool)
        .await?;
    assert!(!is_standby, "probe must run against a primary");
    let synchronous_standby_names: String = sqlx::query_scalar("SHOW synchronous_standby_names")
        .fetch_one(&pool)
        .await?;
    assert!(
        synchronous_standby_names.is_empty(),
        "probe requires a disposable primary with no configured synchronous standby"
    );

    sqlx::query("CREATE TABLE v1_durability_probe (id bigint PRIMARY KEY)")
        .execute(&pool)
        .await?;
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL statement_timeout = '5s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL synchronous_commit = 'remote_apply'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO v1_durability_probe (id) VALUES (1)")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let inserted: i64 = sqlx::query_scalar("SELECT count(*) FROM v1_durability_probe")
        .fetch_one(&pool)
        .await?;
    assert_eq!(inserted, 1, "the mutation really committed");
    let post_commit_lsn: String = sqlx::query_scalar("SELECT pg_current_wal_insert_lsn()::text")
        .fetch_one(&pool)
        .await?;
    assert_ne!(post_commit_lsn, "0/0", "the WAL upper bound must exist");
    let physical_standbys: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_replication")
        .fetch_one(&pool)
        .await?;
    assert_eq!(physical_standbys, 0, "probe requires no physical standby");

    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Worker.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
    ])?;
    let state = AppState::new(config, DatabaseDependency::Postgres(pool))?;
    let response = build_router(state)
        .oneshot(Request::builder().uri("/readyz").body(Body::empty())?)
        .await?;

    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    let readiness: serde_json::Value = serde_json::from_slice(&body)?;
    assert!(
        status == StatusCode::SERVICE_UNAVAILABLE
            || (status == StatusCode::OK && readiness["write_ready"] == false),
        "a reachable primary with a committed row and remote_apply but no standby cannot claim write readiness: status={status}, body={readiness}"
    );
    Ok(())
}
