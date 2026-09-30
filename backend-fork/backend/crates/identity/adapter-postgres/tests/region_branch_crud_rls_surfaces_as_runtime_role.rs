#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Negative runtime boundary for the retired identity-owned region/branch writes.
//!
//! Issue #707 moves all six mutations to the org-change lifecycle. These tests
//! retain the former integration target and its six-test census, but invert the
//! contract: every legacy adapter entry point must fail closed before SQL, leave
//! the owning tables byte-for-byte unchanged, and emit no legacy mutation audit.
//! The orgchange adapter's own suites retain positive apply/preflight coverage.

use std::fmt::Debug;

use console_identity_adapter_postgres::{PgOrgError, PgOrgStore};
use console_identity_application::{
    CreateBranchCommand, CreateRegionCommand, DeactivateBranchCommand, DeactivateRegionCommand,
    UpdateBranchCommand, UpdateRegionCommand,
};
use console_kernel_core::{
    BranchId, BranchScope, ErrorKind, OrgId, RegionId, TraceContext, UserId,
};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

fn assert_direct_refusal<T: Debug>(result: Result<T, PgOrgError>) {
    match result {
        Err(PgOrgError::Domain(error)) => assert_eq!(
            error.kind,
            ErrorKind::Forbidden,
            "retired identity write must point callers at OrgProposal"
        ),
        other => panic!("expected fail-closed direct-write refusal, got {other:?}"),
    }
}

async fn seed_org(pool: &PgPool) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(format!("org-{}", Uuid::new_v4()))
    .bind("Identity tombstone test")
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_region(pool: &PgPool, name: &str) -> RegionId {
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(name)
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    RegionId::from_uuid(id)
}

async fn seed_branch(pool: &PgPool, region: RegionId, name: &str) -> BranchId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*region.as_uuid())
    .bind(name)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(id)
}

async fn legacy_audit_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action IN \
         ('region.create', 'region.update', 'region.deactivate', \
          'branch.create', 'branch.update', 'branch.deactivate')",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

fn actor() -> UserId {
    UserId::new()
}

fn trace() -> TraceContext {
    TraceContext::generate()
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn direct_create_region_is_refused_without_a_row_or_audit(pool: PgPool) {
    seed_org(&pool).await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM regions")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_direct_refusal(
        PgOrgStore::new(pool.clone())
            .create_region(CreateRegionCommand {
                actor: actor(),
                name: "수도권".into(),
                trace: trace(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await,
    );

    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM regions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(legacy_audit_count(&pool).await, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn direct_update_region_is_refused_and_preserves_the_row(pool: PgPool) {
    seed_org(&pool).await;
    let region = seed_region(&pool, "수도권").await;

    assert_direct_refusal(
        PgOrgStore::new(pool.clone())
            .update_region(UpdateRegionCommand {
                actor: actor(),
                region_id: region,
                name: Some("탈취시도".into()),
                trace: trace(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await,
    );

    let name: String = sqlx::query_scalar("SELECT name FROM regions WHERE id = $1")
        .bind(*region.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "수도권");
    assert_eq!(legacy_audit_count(&pool).await, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn direct_deactivate_region_is_refused_and_preserves_the_row(pool: PgPool) {
    seed_org(&pool).await;
    let region = seed_region(&pool, "수도권").await;

    assert_direct_refusal(
        PgOrgStore::new(pool.clone())
            .deactivate_region(DeactivateRegionCommand {
                actor: actor(),
                region_id: region,
                trace: trace(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await,
    );

    let deactivated: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT deactivated_at FROM regions WHERE id = $1")
            .bind(*region.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(deactivated.is_none());
    assert_eq!(legacy_audit_count(&pool).await, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn direct_create_branch_is_refused_without_a_row_or_audit(pool: PgPool) {
    seed_org(&pool).await;
    let region = seed_region(&pool, "수도권").await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM branches")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_direct_refusal(
        PgOrgStore::new(pool.clone())
            .create_branch(CreateBranchCommand {
                actor: actor(),
                region_id: region,
                name: "강남지점".into(),
                trace: trace(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await,
    );

    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(legacy_audit_count(&pool).await, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn direct_update_branch_is_refused_and_preserves_the_row(pool: PgPool) {
    seed_org(&pool).await;
    let region = seed_region(&pool, "수도권").await;
    let branch = seed_branch(&pool, region, "강남지점").await;

    assert_direct_refusal(
        PgOrgStore::new(pool.clone())
            .update_branch(UpdateBranchCommand {
                actor: actor(),
                branch_scope: BranchScope::All,
                branch_id: branch,
                region_id: None,
                name: Some("무단변경".into()),
                trace: trace(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await,
    );

    let row = sqlx::query("SELECT name, region_id FROM branches WHERE id = $1")
        .bind(*branch.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.try_get::<String, _>("name").unwrap(), "강남지점");
    assert_eq!(
        row.try_get::<Uuid, _>("region_id").unwrap(),
        *region.as_uuid()
    );
    assert_eq!(legacy_audit_count(&pool).await, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn direct_deactivate_branch_is_refused_and_preserves_the_row(pool: PgPool) {
    seed_org(&pool).await;
    let region = seed_region(&pool, "수도권").await;
    let branch = seed_branch(&pool, region, "강남지점").await;

    assert_direct_refusal(
        PgOrgStore::new(pool.clone())
            .deactivate_branch(DeactivateBranchCommand {
                actor: actor(),
                branch_scope: BranchScope::All,
                branch_id: branch,
                trace: trace(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await,
    );

    let deactivated: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT deactivated_at FROM branches WHERE id = $1")
            .bind(*branch.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(deactivated.is_none());
    assert_eq!(legacy_audit_count(&pool).await, 0);
}
