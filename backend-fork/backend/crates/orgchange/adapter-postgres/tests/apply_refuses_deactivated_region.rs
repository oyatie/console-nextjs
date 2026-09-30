#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Apply-time inverse of `DeactivateRegion`'s active-branch guard (console-k6wm /
//! lx6 sibling): `CreateBranch` and `RenameBranch{region_id: Some(_)}` must
//! refuse a deactivated target region with Conflict (409-equivalent) so a
//! proposal cannot land a live branch under a soft-deleted region that
//! `list_regions` hides.
//!
//! Driven through `PgOrgChangeStore::effectuate` (the shipped apply entry
//! point), never a private helper. Requests are seeded `APPROVED` so the
//! oracle isolates the apply transaction — the defect is apply-time, not
//! draft/submit/approval.

use console_kernel_core::{ErrorKind, OrgId, UserId};
use console_orgchange_adapter_postgres::PgOrgChangeStore;
use console_orgchange_domain::{OrgChangeStatus, OrgProposalOp};
use sqlx::{PgPool, Row};
use time::{Date, OffsetDateTime, macros::offset};
use uuid::Uuid;

fn today_kst() -> Date {
    OffsetDateTime::now_utc().to_offset(offset!(+9)).date()
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn effectuate_create_and_reparent_refuse_deactivated_region(pool: PgPool) {
    let org = seed_org(&pool).await;
    console_platform_request_context::scope_org(org, async move {
        let actor = seed_user(&pool, org, "EXECUTIVE").await;
        let active_region = seed_region(&pool, org, "활성권역").await;
        let dead_region = seed_region(&pool, org, "비활성권역").await;
        deactivate_region(&pool, org, dead_region).await;

        let store = PgOrgChangeStore::new(pool.clone());

        // (a) CreateBranch under a deactivated region → conflict, no ghost row.
        let create_id = seed_approved(
            &pool,
            org,
            actor,
            dead_region,
            "k6wm-create-dead-region-0001",
            vec![OrgProposalOp::CreateBranch {
                region_id: dead_region,
                name: "유령지점".into(),
            }],
        )
        .await;
        let create_blocked = store.effectuate(actor, create_id).await;
        match create_blocked {
            Err(err) => assert_eq!(
                err.kind(),
                ErrorKind::Conflict,
                "CreateBranch into a deactivated region must be 409, never orphan: {err:?}"
            ),
            Ok(detail) => panic!(
                "expected CreateBranch conflict under deactivated region, got APPLIED: {detail:?}"
            ),
        }
        assert_eq!(
            branch_count_by_name(&pool, org, "유령지점").await,
            0,
            "refused CreateBranch must insert no branch row"
        );
        assert_eq!(
            request_status(&pool, org, create_id).await,
            "APPROVED",
            "refused effectuate must leave the request APPROVED (no partial apply)"
        );

        // (b) RenameBranch re-parent into the deactivated region → conflict.
        let live_branch = seed_branch(&pool, org, active_region, "실지점").await;
        let reparent_id = seed_approved(
            &pool,
            org,
            actor,
            active_region,
            "k6wm-reparent-dead-region-0001",
            vec![OrgProposalOp::RenameBranch {
                branch_id: live_branch,
                name: None,
                region_id: Some(dead_region),
            }],
        )
        .await;
        let reparent_blocked = store.effectuate(actor, reparent_id).await;
        match reparent_blocked {
            Err(err) => assert_eq!(
                err.kind(),
                ErrorKind::Conflict,
                "RenameBranch into a deactivated region must be 409: {err:?}"
            ),
            Ok(detail) => panic!(
                "expected RenameBranch conflict under deactivated region, got APPLIED: {detail:?}"
            ),
        }
        assert_eq!(
            branch_region(&pool, org, live_branch).await,
            active_region,
            "refused re-parent must leave region_id unchanged"
        );
        assert_eq!(request_status(&pool, org, reparent_id).await, "APPROVED");

        // (c) Non-regression: CreateBranch + RenameBranch under ACTIVE regions still apply.
        let other_active = seed_region(&pool, org, "다른활성권역").await;
        let ok_create_id = seed_approved(
            &pool,
            org,
            actor,
            other_active,
            "k6wm-create-active-region-0001",
            vec![OrgProposalOp::CreateBranch {
                region_id: other_active,
                name: "정상신설".into(),
            }],
        )
        .await;
        let created = store
            .effectuate(actor, ok_create_id)
            .await
            .expect("CreateBranch under an active region must still apply");
        assert_eq!(created.summary.status, OrgChangeStatus::Applied);
        assert_eq!(branch_count_by_name(&pool, org, "정상신설").await, 1);

        let move_target = seed_region(&pool, org, "이동활성권역").await;
        let ok_reparent_id = seed_approved(
            &pool,
            org,
            actor,
            active_region,
            "k6wm-reparent-active-region-0001",
            vec![OrgProposalOp::RenameBranch {
                branch_id: live_branch,
                name: None,
                region_id: Some(move_target),
            }],
        )
        .await;
        store
            .effectuate(actor, ok_reparent_id)
            .await
            .expect("RenameBranch under an active region must still apply");
        assert_eq!(branch_region(&pool, org, live_branch).await, move_target);
    })
    .await;
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

async fn seed_org(pool: &PgPool) -> OrgId {
    let slug = format!("k6{}", &Uuid::new_v4().simple().to_string()[..12]);
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO organizations (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(&slug)
            .bind(format!("org-{slug}"))
            .fetch_one(pool)
            .await
            .unwrap();
    OrgId::from_uuid(id)
}

async fn seed_region(pool: &PgPool, org: OrgId, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(*org.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn deactivate_region(pool: &PgPool, org: OrgId, region_id: Uuid) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let changed = sqlx::query("UPDATE regions SET deactivated_at = now() WHERE id = $1")
        .bind(region_id)
        .execute(tx.as_mut())
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(changed, 1, "fixture must deactivate the seeded region");
    tx.commit().await.unwrap();
}

async fn seed_branch(pool: &PgPool, org: OrgId, region: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region)
    .bind(name)
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_user(pool: &PgPool, org: OrgId, role: &str) -> UserId {
    let user = UserId::new();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, is_active, org_id) \
         VALUES ($1, $2, $3, true, $4)",
    )
    .bind(*user.as_uuid())
    .bind(format!("k6wm-{user}"))
    .bind(vec![role])
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    user
}

/// Insert a request already in `APPROVED` so effectuate exercises apply_ops
/// without the SoD chain. Fingerprint is a valid 64-hex sha; proposal JSON is
/// the typed `OrgProposalOp` wire shape.
async fn seed_approved(
    pool: &PgPool,
    org: OrgId,
    actor: UserId,
    target_region: Uuid,
    idempotency_key: &str,
    proposal: Vec<OrgProposalOp>,
) -> Uuid {
    let id = Uuid::new_v4();
    let proposal_json = serde_json::to_value(&proposal).unwrap();
    let fingerprint = format!("{:064x}", id.as_u128());
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    // Mirror the production allocator instead of truncating a UUID into a
    // collision-prone four-digit fixture code. Calls in this story are
    // sequential and the tenant-armed transaction sees every prior seed.
    let year = today_kst().year();
    let prefix = format!("OC-{year}-");
    let last: Option<i64> = sqlx::query_scalar(
        "SELECT max((substring(code FROM 9))::bigint) \
         FROM org_change_requests WHERE code LIKE $1",
    )
    .bind(format!("{prefix}%"))
    .fetch_one(tx.as_mut())
    .await
    .unwrap();
    let code = format!("{prefix}{:04}", last.unwrap_or(0) + 1);
    sqlx::query(
        "INSERT INTO org_change_requests \
         (id, org_id, code, kind, status, target_kind, target_ref, target_label, \
          effective_date, reason, proposal, drafted_by, idempotency_key, request_fingerprint) \
         VALUES ($1,$2,$3,'REORG','APPROVED','REGION',$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(id)
    .bind(*org.as_uuid())
    .bind(&code)
    .bind(target_region.to_string())
    .bind("k6wm deactivated-region probe")
    .bind(today_kst())
    .bind("console-k6wm apply inverse guard")
    .bind(&proposal_json)
    .bind(*actor.as_uuid())
    .bind(idempotency_key)
    .bind(&fingerprint)
    .execute(tx.as_mut())
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

async fn branch_count_by_name(pool: &PgPool, org: OrgId, name: &str) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM branches WHERE name = $1")
        .bind(name)
        .fetch_one(tx.as_mut())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    count
}

async fn branch_region(pool: &PgPool, org: OrgId, branch_id: Uuid) -> Uuid {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let region: Uuid = sqlx::query_scalar("SELECT region_id FROM branches WHERE id = $1")
        .bind(branch_id)
        .fetch_one(tx.as_mut())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    region
}

async fn request_status(pool: &PgPool, org: OrgId, id: Uuid) -> String {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let row = sqlx::query("SELECT status FROM org_change_requests WHERE id = $1")
        .bind(id)
        .fetch_one(tx.as_mut())
        .await
        .unwrap();
    let status: String = row.try_get("status").unwrap();
    tx.commit().await.unwrap();
    status
}
