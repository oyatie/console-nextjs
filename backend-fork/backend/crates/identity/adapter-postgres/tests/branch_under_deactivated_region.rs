#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Regression target retained from the former identity-owned branch writer.
//!
//! The positive deactivated-region apply guard now lives and remains tested in
//! orgchange/adapter-postgres/tests/apply_refuses_deactivated_region.rs. At the
//! retired identity boundary, both former entry points must refuse before SQL
//! regardless of parent state, leaving the seeded rows untouched.

use console_identity_adapter_postgres::{PgOrgError, PgOrgStore};
use console_identity_application::{CreateBranchCommand, UpdateBranchCommand};
use console_kernel_core::{
    BranchId, BranchScope, ErrorKind, OrgId, RegionId, TraceContext, UserId,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

fn assert_direct_refusal<T: std::fmt::Debug>(result: Result<T, PgOrgError>) {
    assert!(
        matches!(result, Err(PgOrgError::Domain(ref error)) if error.kind == ErrorKind::Forbidden),
        "identity must refuse the retired direct writer: {result:?}"
    );
}

async fn seed_org(pool: &PgPool) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(format!("org-{}", Uuid::new_v4()))
    .bind("Deactivated region tombstone test")
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_region(pool: &PgPool, name: &str, deactivated: bool) -> RegionId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO regions (name, org_id, deactivated_at) \
         VALUES ($1, $2, CASE WHEN $3 THEN now() ELSE NULL END) RETURNING id",
    )
    .bind(name)
    .bind(*OrgId::knl().as_uuid())
    .bind(deactivated)
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

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_branch_under_deactivated_region_is_refused_before_sql(pool: PgPool) {
    seed_org(&pool).await;
    let dead_region = seed_region(&pool, "권역-비활성", true).await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM branches")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_direct_refusal(
        PgOrgStore::new(pool.clone())
            .create_branch(CreateBranchCommand {
                actor: UserId::new(),
                region_id: dead_region,
                name: "숨겨질지점".into(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await,
    );

    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, before);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn reparenting_branch_under_deactivated_region_is_refused_before_sql(pool: PgPool) {
    seed_org(&pool).await;
    let live_region = seed_region(&pool, "권역-활성", false).await;
    let dead_region = seed_region(&pool, "권역-비활성", true).await;
    let branch = seed_branch(&pool, live_region, "강남지점").await;

    assert_direct_refusal(
        PgOrgStore::new(pool.clone())
            .update_branch(UpdateBranchCommand {
                actor: UserId::new(),
                branch_scope: BranchScope::All,
                branch_id: branch,
                region_id: Some(dead_region),
                name: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await,
    );

    let parent: Uuid = sqlx::query_scalar("SELECT region_id FROM branches WHERE id = $1")
        .bind(*branch.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(parent, *live_region.as_uuid());
}
