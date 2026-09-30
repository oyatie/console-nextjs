#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS cross-org gate for the three Slack-parity tables added in
//! migrations 0128/0129: `messenger_message_acks`, `messenger_presence`,
//! `messenger_thread_mutes`. Proven as the genuine non-owner runtime role
//! `console_rt` (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) — the crate's other ack/
//! presence/mute tests (`use_cases.rs`) run on the default `#[sqlx::test]`
//! BYPASSRLS superuser pool, which sees every row regardless of the
//! `app.current_org` GUC and would mask a broken `org_isolation` policy
//! entirely (rls-verify-as-runtime-role discipline).
//!
//! Seeds one row per table in org A, then as `console_rt` with the GUC armed to
//! org B: (a) a plain `SELECT count(*)` must see ZERO org-A rows (invisible),
//! and (b) an INSERT tagged `org_id = A` while the GUC is B must be rejected
//! by the policy's `WITH CHECK`.

use console_kernel_core::OrgId;
use sqlx::{PgPool, Row};
use uuid::Uuid;

const ORG_B: Uuid = Uuid::from_u128(0x8a55_8a55_8a55_8a55_8a55_8a55_8a55_8a55);

const SET_RUNTIME_ROLE: &str = "SET LOCAL ROLE console_rt";

/// The org-A message and thread the seeded ack/mute rows FK to, for the
/// cross-org INSERT-rejection test to reference.
struct SeededA {
    message_id: Uuid,
    thread_id: Uuid,
}

async fn seed_org(pool: &PgPool, org: Uuid, tag: &str) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(pool)
    .await
    .unwrap();
}

/// Seed org A's full slice as the OWNER pool (bypasses RLS) — an ack/presence/
/// mute row, plus the message/thread/user it references.
async fn seed_org_a(pool: &PgPool) -> SeededA {
    let org = *OrgId::knl().as_uuid();
    seed_org(pool, org, "knl").await;

    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Region {}", Uuid::new_v4()))
            .bind(org)
            .fetch_one(pool)
            .await
            .unwrap();
    let branch: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region)
    .bind(format!("Branch {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap();

    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(user_id)
        .bind(format!("User {user_id}"))
        .bind(vec!["MECHANIC".to_string()])
        .bind(org)
        .execute(pool)
        .await
        .unwrap();

    let thread_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO messenger_threads (kind, visibility, branch_id, title, created_by, org_id)
        VALUES ('team', 'direct', $1, 'Seed Thread', $2, $3)
        RETURNING id
        "#,
    )
    .bind(branch)
    .bind(user_id)
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap();

    let message_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO messenger_messages (thread_id, branch_id, sender_id, body, sent_at, org_id)
        VALUES ($1, $2, $3, 'seed body', now(), $4)
        RETURNING id
        "#,
    )
    .bind(thread_id)
    .bind(branch)
    .bind(user_id)
    .bind(org)
    .fetch_one(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO messenger_message_acks (message_id, user_id, org_id, acked_at) VALUES ($1, $2, $3, now())",
    )
    .bind(message_id)
    .bind(user_id)
    .bind(org)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO messenger_presence (user_id, org_id, last_activity_at, updated_at) VALUES ($1, $2, now(), now())",
    )
    .bind(user_id)
    .bind(org)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO messenger_thread_mutes (thread_id, user_id, org_id, muted_at) VALUES ($1, $2, $3, now())",
    )
    .bind(thread_id)
    .bind(user_id)
    .bind(org)
    .execute(pool)
    .await
    .unwrap();

    SeededA {
        message_id,
        thread_id,
    }
}

/// Drop to the non-owner runtime role and arm `app.current_org` for `org`.
async fn set_role_and_org(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, org: Uuid) {
    sqlx::query(SET_RUNTIME_ROLE)
        .execute(&mut **tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.to_string())
        .execute(&mut **tx)
        .await
        .unwrap();
}

async fn count_as_runtime(pool: &PgPool, org: Uuid, count_query: &'static str) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    set_role_and_org(&mut tx, org).await;
    let count: i64 = sqlx::query(count_query)
        .fetch_one(&mut *tx)
        .await
        .unwrap()
        .get(0);
    tx.commit().await.unwrap();
    count
}

const COUNT_ACKS: &str = "SELECT count(*) FROM messenger_message_acks";
const COUNT_PRESENCE: &str = "SELECT count(*) FROM messenger_presence";
const COUNT_MUTES: &str = "SELECT count(*) FROM messenger_thread_mutes";

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn parity_tables_are_invisible_across_org_as_runtime_role(pool: PgPool) {
    seed_org(&pool, ORG_B, "b").await;
    seed_org_a(&pool).await;

    for (table, count_query) in [
        ("messenger_message_acks", COUNT_ACKS),
        ("messenger_presence", COUNT_PRESENCE),
        ("messenger_thread_mutes", COUNT_MUTES),
    ] {
        assert_eq!(
            count_as_runtime(&pool, *OrgId::knl().as_uuid(), count_query).await,
            1,
            "org A must see exactly its own {table} row"
        );
        assert_eq!(
            count_as_runtime(&pool, ORG_B, count_query).await,
            0,
            "org B must see ZERO rows of org A's {table} (cross-org invisible)"
        );
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn parity_tables_reject_cross_org_insert_as_runtime_role(pool: PgPool) {
    seed_org(&pool, ORG_B, "b").await;
    let a = seed_org_a(&pool).await;
    let org_a = *OrgId::knl().as_uuid();

    // A second org-A user for the ack/mute rows, so the (message_id, user_id) /
    // (thread_id, user_id) primary key of the already-seeded row doesn't collide.
    let other_user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(other_user_id)
        .bind(format!("User {other_user_id}"))
        .bind(vec!["MECHANIC".to_string()])
        .bind(org_a)
        .execute(&pool)
        .await
        .unwrap();

    // GUC = org B, INSERT a row tagged org_id = A → rejected by WITH CHECK.
    {
        let mut tx = pool.begin().await.unwrap();
        set_role_and_org(&mut tx, ORG_B).await;
        let res = sqlx::query(
            "INSERT INTO messenger_message_acks (message_id, user_id, org_id, acked_at) VALUES ($1, $2, $3, now())",
        )
        .bind(a.message_id)
        .bind(other_user_id)
        .bind(org_a)
        .execute(&mut *tx)
        .await;
        let err = res
            .expect_err("cross-org ack INSERT must be rejected")
            .to_string();
        assert!(
            err.contains("row-level security"),
            "cross-org messenger_message_acks INSERT must be rejected by RLS, got: {err}"
        );
        let _ = tx.rollback().await;
    }

    {
        let mut tx = pool.begin().await.unwrap();
        set_role_and_org(&mut tx, ORG_B).await;
        let res = sqlx::query(
            "INSERT INTO messenger_presence (user_id, org_id, last_activity_at, updated_at) VALUES ($1, $2, now(), now())",
        )
        .bind(other_user_id)
        .bind(org_a)
        .execute(&mut *tx)
        .await;
        let err = res
            .expect_err("cross-org presence INSERT must be rejected")
            .to_string();
        assert!(
            err.contains("row-level security"),
            "cross-org messenger_presence INSERT must be rejected by RLS, got: {err}"
        );
        let _ = tx.rollback().await;
    }

    {
        let mut tx = pool.begin().await.unwrap();
        set_role_and_org(&mut tx, ORG_B).await;
        let res = sqlx::query(
            "INSERT INTO messenger_thread_mutes (thread_id, user_id, org_id, muted_at) VALUES ($1, $2, $3, now())",
        )
        .bind(a.thread_id)
        .bind(other_user_id)
        .bind(org_a)
        .execute(&mut *tx)
        .await;
        let err = res
            .expect_err("cross-org mute INSERT must be rejected")
            .to_string();
        assert!(
            err.contains("row-level security"),
            "cross-org messenger_thread_mutes INSERT must be rejected by RLS, got: {err}"
        );
        let _ = tx.rollback().await;
    }
}
