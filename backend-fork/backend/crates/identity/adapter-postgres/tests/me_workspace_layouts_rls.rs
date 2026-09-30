#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS for `me_workspace_layouts` (Oyatie Console workspace persistence,
//! UI-M1b, migration 0098).
//!
//! Proven as the GENUINE non-owner runtime role `console_rt` (NOSUPERUSER,
//! NOBYPASSRLS, FORCE RLS) — never the BYPASSRLS superuser the default
//! `#[sqlx::test]` pool connects as, which would green-light a broken or leaking
//! policy:
//!   1. Round-trip: `put_workspace_layout` upserts the caller's opaque layout and
//!      `get_workspace_layout` reads it back verbatim; a second put overwrites.
//!   2. Tenant isolation: under org A's armed GUC, a caller cannot read org B's
//!      workspace row — neither via the store (empty `{}` default) nor a direct
//!      by-user query (invisible).
//!   3. Governance: `console_rt` may NEVER DELETE a workspace row (REVOKE DELETE).

use console_identity_adapter_postgres::PgOrgStore;
use console_kernel_core::{OrgId, TraceContext, UserId};
use console_platform_request_context::CURRENT_ORG;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use uuid::Uuid;

/// A second, non-KNL tenant id, to prove cross-tenant isolation under `console_rt`.
const ORG_B: Uuid = Uuid::from_u128(0x3333_3333_3333_3333_3333_3333_3333_3333);

/// A pool whose every connection runs `SET ROLE console_rt`, so statements execute as
/// the production runtime role (NOSUPERUSER, NOBYPASSRLS) under FORCE RLS.
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

/// Seed an ACTIVE user (owner pool, row_security off). The workspace row FKs to
/// `users(id, org_id)`, so every fixture user is a real row.
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
    .bind(vec!["MECHANIC".to_string()])
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    UserId::from_uuid(user_id)
}

/// Insert a workspace row directly as the owner (row_security off), so the RLS
/// isolation test starts from a known, tenant-tagged row per org.
async fn seed_layout_row(owner_pool: &PgPool, org: Uuid, user: UserId, layout: serde_json::Value) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO me_workspace_layouts (org_id, user_id, layout) VALUES ($1, $2, $3)")
        .bind(org)
        .bind(*user.as_uuid())
        .bind(layout)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn put_get_roundtrip_and_overwrite_via_store_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid, "A").await;
    let user = seed_active_user(&owner_pool, org_uuid).await;

    let store = PgOrgStore::new(rt_pool.clone());

    // Before any save the caller reads the empty-default `{}`.
    let initial = CURRENT_ORG
        .scope(org, store.get_workspace_layout(user))
        .await
        .expect("get must succeed as console_rt under the armed GUC");
    assert_eq!(
        initial,
        serde_json::json!({}),
        "no save yet must read as {{}}"
    );

    // Upsert a layout, then read it back verbatim.
    let layout = serde_json::json!({
        "v": 1,
        "panels": [{ "key": "wo:1", "kindLabel": "정비", "code": "WO-1", "mode": "pinned", "quads": ["tr"] }]
    });
    let stored = CURRENT_ORG
        .scope(
            org,
            store.put_workspace_layout(
                user,
                layout.clone(),
                TraceContext::generate(),
                OffsetDateTime::now_utc(),
            ),
        )
        .await
        .expect("put must succeed as console_rt under the armed GUC");
    assert_eq!(stored, layout, "put returns the stored layout verbatim");
    assert_eq!(
        CURRENT_ORG
            .scope(org, store.get_workspace_layout(user))
            .await
            .unwrap(),
        layout,
        "get reads back the saved layout verbatim"
    );

    // A second upsert overwrites (idempotent single-row per user).
    let layout2 = serde_json::json!({ "v": 1, "panels": [] });
    CURRENT_ORG
        .scope(
            org,
            store.put_workspace_layout(
                user,
                layout2.clone(),
                TraceContext::generate(),
                OffsetDateTime::now_utc(),
            ),
        )
        .await
        .expect("second put must succeed");
    assert_eq!(
        CURRENT_ORG
            .scope(org, store.get_workspace_layout(user))
            .await
            .unwrap(),
        layout2,
        "second put overwrites the layout"
    );

    // Exactly one row for this (user, org): the upsert never duplicates.
    let rows: i64 = {
        let mut tx = owner_pool.begin().await.unwrap();
        sqlx::query("SET LOCAL row_security = off")
            .execute(&mut *tx)
            .await
            .unwrap();
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM me_workspace_layouts WHERE user_id = $1 AND org_id = $2",
        )
        .bind(*user.as_uuid())
        .bind(org_uuid)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        n
    };
    assert_eq!(rows, 1, "exactly one workspace row for the caller");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn isolate_tenants_and_deny_delete_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = *OrgId::knl().as_uuid();
    let org_b = ORG_B;
    seed_org(&owner_pool, org_a, "A").await;
    seed_org(&owner_pool, org_b, "B").await;
    let user_a = seed_active_user(&owner_pool, org_a).await;
    let user_b = seed_active_user(&owner_pool, org_b).await;
    seed_layout_row(
        &owner_pool,
        org_a,
        user_a,
        serde_json::json!({ "v": 1, "who": "A" }),
    )
    .await;
    seed_layout_row(
        &owner_pool,
        org_b,
        user_b,
        serde_json::json!({ "v": 1, "who": "B" }),
    )
    .await;

    let store = PgOrgStore::new(rt_pool.clone());

    // (1) Under org A's GUC the store reads A's own layout.
    assert_eq!(
        CURRENT_ORG
            .scope(OrgId::knl(), store.get_workspace_layout(user_a))
            .await
            .unwrap(),
        serde_json::json!({ "v": 1, "who": "A" }),
        "org A reads its own workspace layout"
    );

    // (2) Cross-tenant: under org A's GUC, B's row is invisible. The store returns
    //     the empty default and a direct by-user query counts zero.
    assert_eq!(
        CURRENT_ORG
            .scope(OrgId::knl(), store.get_workspace_layout(user_b))
            .await
            .unwrap(),
        serde_json::json!({}),
        "org B's workspace layout must be invisible under A (empty default)"
    );
    {
        let mut tx = rt_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org_a.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        let visible: i64 =
            sqlx::query_scalar("SELECT count(*) FROM me_workspace_layouts WHERE user_id = $1")
                .bind(*user_b.as_uuid())
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            visible, 0,
            "org B's workspace row must be invisible under A"
        );
    }

    // (3) console_rt may NEVER DELETE a workspace row (REVOKE DELETE governance guard).
    {
        let mut tx = rt_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org_a.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        let err = sqlx::query("DELETE FROM me_workspace_layouts")
            .execute(&mut *tx)
            .await
            .expect_err("console_rt must not DELETE me_workspace_layouts")
            .to_string();
        assert!(
            err.contains("permission denied"),
            "DELETE as console_rt must be denied by the REVOKE, got: {err}"
        );
        let _ = tx.rollback().await;
    }
}
