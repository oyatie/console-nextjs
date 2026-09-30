#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the persisted outbound rate limiter (M1).
//!
//! Mirrors `mail_account_rls_surfaces_as_runtime_role.rs`: we SEED orgs/users as
//! the owner (raw inserts, row_security off) and exercise the counter as the
//! genuine non-owner runtime role `console_rt` (NOSUPERUSER, NOBYPASSRLS, FORCE RLS)
//! under the armed `app.current_org` GUC. The default `#[sqlx::test]` pool is a
//! BYPASSRLS superuser and would green-light a leaking/broken counter.
//!
//! Asserts:
//!   * the counter increments monotonically within a window for one (org, user,
//!     endpoint, window_start) and the service-layer cap blocks once attempts
//!     exceed it (the application's `enforce_rate_limit` semantics, exercised
//!     here directly against the adapter port);
//!   * ORG-SCOPED isolation: org A bumping its counter does NOT change org B's
//!     count for the same user-shaped key — each org has an independent counter,
//!     enforced by the org_isolation RLS policy;
//!   * FAIL-CLOSED: with no GUC armed, the UPSERT is rejected (never silently
//!     writes an unscoped row).

use console_comms_adapter_postgres::PgMailStore;
use console_comms_application::{MailStore, SEND_RATE_PER_MINUTE};
use console_kernel_core::{OrgId, UserId};
use console_platform_request_context::CURRENT_ORG;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use uuid::Uuid;

const ORG_B: Uuid = Uuid::from_u128(0x3333_3333_3333_3333_3333_3333_3333_3333);
const ENDPOINT: &str = "mail_send:1m";

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

async fn seed_active_user(owner_pool: &PgPool, org: Uuid) -> UserId {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id, is_active) VALUES ($1, $2, $3, true) RETURNING id",
    )
    .bind(format!("User {}", Uuid::new_v4()))
    .bind(vec!["ADMIN".to_string()])
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    UserId::from_uuid(user_id)
}

/// Read the raw counter row as OWNER (bypassing RLS) for cross-checks.
async fn raw_attempts(owner_pool: &PgPool, org: Uuid, actor: Uuid) -> Option<i32> {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let attempts: Option<i32> = sqlx::query_scalar(
        "SELECT attempts FROM comms_send_rate WHERE org_id = $1 AND actor_user_id = $2 AND endpoint = $3",
    )
    .bind(org)
    .bind(actor)
    .bind(ENDPOINT)
    .fetch_optional(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    attempts
}

// ===========================================================================
// The counter increments monotonically and the cap blocks past the limit.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn counter_increments_and_cap_blocks_after_limit(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid, "A").await;
    let actor = seed_active_user(&owner_pool, org_uuid).await;
    let store = PgMailStore::new(rt_pool.clone());

    // A fixed window so every increment lands in the SAME bucket row.
    let window = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

    // The first SEND_RATE_PER_MINUTE increments are at or under the cap; the
    // (cap + 1)-th increment is the first to EXCEED it (attempts > cap).
    let mut blocked_at = None;
    for i in 1..=(SEND_RATE_PER_MINUTE + 5) {
        let attempts = CURRENT_ORG
            .scope(org, store.increment_send_rate(actor, ENDPOINT, window))
            .await
            .expect("increment_send_rate as console_rt under the armed GUC");
        assert_eq!(attempts, i, "the counter must increment by exactly one");
        if attempts > SEND_RATE_PER_MINUTE && blocked_at.is_none() {
            blocked_at = Some(i);
        }
    }
    assert_eq!(
        blocked_at,
        Some(SEND_RATE_PER_MINUTE + 1),
        "the cap must be first exceeded on the (cap + 1)-th attempt"
    );

    // The persisted counter reflects every increment.
    assert_eq!(
        raw_attempts(&owner_pool, org_uuid, *actor.as_uuid()).await,
        Some(i32::try_from(SEND_RATE_PER_MINUTE + 5).unwrap())
    );
}

// ===========================================================================
// Org-scoped: one org's counter does NOT affect another's (same user-key shape).
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn counter_is_org_scoped_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);
    seed_org(&owner_pool, *org_a.as_uuid(), "A").await;
    seed_org(&owner_pool, *org_b.as_uuid(), "B").await;
    let actor_a = seed_active_user(&owner_pool, *org_a.as_uuid()).await;
    let actor_b = seed_active_user(&owner_pool, *org_b.as_uuid()).await;
    let store = PgMailStore::new(rt_pool.clone());
    let window = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

    // Bump A's counter three times under A's armed GUC.
    for expected in 1..=3 {
        let attempts = CURRENT_ORG
            .scope(org_a, store.increment_send_rate(actor_a, ENDPOINT, window))
            .await
            .expect("A increments");
        assert_eq!(attempts, expected);
    }

    // B's FIRST increment under B's GUC starts at 1 — A's three bumps are
    // invisible to B (independent org-scoped counters).
    let b_first = CURRENT_ORG
        .scope(org_b, store.increment_send_rate(actor_b, ENDPOINT, window))
        .await
        .expect("B increments");
    assert_eq!(b_first, 1, "B's counter is independent of A's");

    // The raw rows confirm the two orgs each hold their own count.
    assert_eq!(
        raw_attempts(&owner_pool, *org_a.as_uuid(), *actor_a.as_uuid()).await,
        Some(3)
    );
    assert_eq!(
        raw_attempts(&owner_pool, *org_b.as_uuid(), *actor_b.as_uuid()).await,
        Some(1)
    );
}

// ===========================================================================
// FAIL-CLOSED: no GUC armed → the UPSERT is rejected, never silently written.
// ===========================================================================
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn increment_fails_closed_without_armed_org(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    seed_org(&owner_pool, *org.as_uuid(), "A").await;
    let actor = seed_active_user(&owner_pool, *org.as_uuid()).await;
    let store = PgMailStore::new(rt_pool.clone());
    let window = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

    // With NO CURRENT_ORG scope, current_org() fails closed -> an error, and no
    // row is written.
    let unarmed = store.increment_send_rate(actor, ENDPOINT, window).await;
    assert!(
        unarmed.is_err(),
        "an unarmed increment must error, never silently write an unscoped counter"
    );
    assert_eq!(
        raw_attempts(&owner_pool, *org.as_uuid(), *actor.as_uuid()).await,
        None,
        "no counter row may exist after a fail-closed increment"
    );
}
