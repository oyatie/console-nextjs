#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Idempotent user lifecycle (console-cg6): replaying `deactivate_user` /
//! `activate_user` on a user already in the target state must NOT mint a second
//! transition audit trail or bump the subject authorization version.
//!
//! The replay is detected as a no-op and returned as a `Conflict`, exactly like
//! `deactivate_region` / `deactivate_branch` already do for their no-op replays.
//! The deactivate no-op path still runs the idempotent credential sweep (and its
//! two sweep audit rows) before surfacing the Conflict, so a credential that
//! raced the original deactivation cannot survive (console-cg6 review); only the
//! transition rows (`user.deactivate`, `policy.account.archive`) are never
//! re-minted.
//!
//! Cross-tenant: armed as org B, `deactivate_user` on org A's user is
//! not_found/omit — no org-A audit row, and the user stays active.

use console_identity_adapter_postgres::PgOrgStore;
use console_identity_application::{ActivateUserCommand, DeactivateUserCommand};
use console_kernel_core::{ErrorKind, OrgId, TraceContext, UserId};
use console_platform_request_context::CURRENT_ORG;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use uuid::Uuid;

/// A second, non-KNL tenant id, to prove cross-tenant isolation under `console_rt`.
const ORG_B: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);

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

/// Seed an organization + one user as the OWNER with `row_security` off.
/// Returns the seeded user id. `is_active` chooses the starting lifecycle state.
async fn seed_org_and_user(owner_pool: &PgPool, org: Uuid, is_active: bool) -> Uuid {
    seed_org(owner_pool, org, "knl").await;
    seed_user(owner_pool, org, is_active).await
}

/// Seed a tenant as the OWNER with `row_security` off. Slug is unique per `tag`.
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

/// Seed a user into an already-inserted org as the OWNER (`row_security` off).
async fn seed_user(owner_pool: &PgPool, org: Uuid, is_active: bool) -> Uuid {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id, is_active) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind("Lifecycle User")
    .bind(vec!["MECHANIC".to_string()])
    .bind(org)
    .bind(is_active)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    user_id
}

/// Count audit rows for one action + target, read as the OWNER (row_security off).
async fn audit_count(owner_pool: &PgPool, action: &str, user_id: Uuid) -> i64 {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = $1 AND target_id = $2",
    )
    .bind(action)
    .bind(user_id.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    count
}

/// Seed a WebAuthn credential row for the user as the OWNER (row_security off),
/// simulating a `finish_registration` that committed after the deactivation sweep.
async fn seed_credential(owner_pool: &PgPool, org: Uuid, user_id: Uuid, credential_id: &str) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO auth_webauthn_credentials
            (id, user_id, credential_id, passkey_json, created_at, org_id)
        VALUES ($1, $2, $3, $4, now(), $5)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(credential_id)
    .bind(serde_json::json!({ "stub": true }))
    .bind(org)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// Count the user's remaining WebAuthn credentials as the OWNER (row_security off).
async fn credential_count(owner_pool: &PgPool, user_id: Uuid) -> i64 {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM auth_webauthn_credentials WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    count
}

/// Lifecycle flag as the OWNER (row_security off), so RLS cannot hide a leak.
async fn user_is_active(owner_pool: &PgPool, user_id: Uuid) -> bool {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let is_active: bool = sqlx::query_scalar("SELECT is_active FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    is_active
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn replaying_deactivate_writes_no_second_transition_audit_row(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);
    let user_id = seed_org_and_user(&owner_pool, *knl.as_uuid(), true).await;
    // The actor must be a real user (audit_events.actor FKs to users).
    let actor_id = seed_org_and_user(&owner_pool, *knl.as_uuid(), true).await;
    seed_org(&owner_pool, ORG_B, "B").await;
    let actor_b = seed_user(&owner_pool, ORG_B, true).await;
    let store = PgOrgStore::new(rt_pool.clone());

    let command = DeactivateUserCommand {
        actor: UserId::from_uuid(actor_id),
        user_id: UserId::from_uuid(user_id),
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    };

    // Armed as org B, org A's user is omitted (not_found): no org-A audit, still active.
    let cross = CURRENT_ORG
        .scope(
            org_b,
            store.deactivate_user(DeactivateUserCommand {
                actor: UserId::from_uuid(actor_b),
                user_id: UserId::from_uuid(user_id),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            }),
        )
        .await
        .expect_err("cross-tenant deactivate must omit org A's user");
    assert_eq!(cross.kind(), ErrorKind::NotFound);
    assert!(
        user_is_active(&owner_pool, user_id).await,
        "org A's user must stay active after a cross-tenant deactivate"
    );
    assert_eq!(
        audit_count(&owner_pool, "user.deactivate", user_id).await,
        0,
        "cross-tenant deactivate must not write an audit row in org A"
    );
    assert_eq!(
        audit_count(&owner_pool, "policy.account.archive", user_id).await,
        0
    );

    let first = CURRENT_ORG
        .scope(knl, store.deactivate_user(command.clone()))
        .await
        .expect("first deactivate must succeed as console_rt");
    assert!(!first.is_active, "the user is soft-deactivated");

    // The no-op replay must be a Conflict, not a second transition.
    let replay = CURRENT_ORG
        .scope(knl, store.deactivate_user(command))
        .await
        .expect_err("replayed deactivate must be a Conflict no-op");
    assert_eq!(replay.kind(), ErrorKind::Conflict);

    // The transition rows are never re-minted: exactly one each.
    assert_eq!(
        audit_count(&owner_pool, "user.deactivate", user_id).await,
        1
    );
    assert_eq!(
        audit_count(&owner_pool, "policy.account.archive", user_id).await,
        1
    );
    // The no-op replay re-runs the idempotent credential sweep, so its two sweep
    // rows are written a second time (no racing credential exists here, so the
    // counts in those rows are 0).
    assert_eq!(
        audit_count(&owner_pool, "auth.passkey.revoke_all", user_id).await,
        2
    );
    assert_eq!(
        audit_count(&owner_pool, "auth.refresh.revoke_all", user_id).await,
        2
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn replaying_deactivate_revokes_a_racing_credential(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let user_id = seed_org_and_user(&owner_pool, *knl.as_uuid(), true).await;
    let actor_id = seed_org_and_user(&owner_pool, *knl.as_uuid(), true).await;
    let store = PgOrgStore::new(rt_pool.clone());

    let command = DeactivateUserCommand {
        actor: UserId::from_uuid(actor_id),
        user_id: UserId::from_uuid(user_id),
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    };

    let first = CURRENT_ORG
        .scope(knl, store.deactivate_user(command.clone()))
        .await
        .expect("first deactivate must succeed as console_rt");
    assert!(!first.is_active, "the user is soft-deactivated");

    // A passkey registration racing the deactivation commits AFTER the original
    // sweep (finish_registration on an already-inactive user is the live hole).
    seed_credential(&owner_pool, *knl.as_uuid(), user_id, "cred-race-1").await;
    assert_eq!(credential_count(&owner_pool, user_id).await, 1);

    // The no-op replay must still revoke the racing credential, while returning
    // Conflict rather than a second transition.
    let replay = CURRENT_ORG
        .scope(knl, store.deactivate_user(command))
        .await
        .expect_err("replayed deactivate must be a Conflict no-op");
    assert_eq!(replay.kind(), ErrorKind::Conflict);
    assert_eq!(
        credential_count(&owner_pool, user_id).await,
        0,
        "the replay's idempotent sweep must revoke the racing credential"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn replaying_activate_writes_no_second_transition_audit_row(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let user_id = seed_org_and_user(&owner_pool, *knl.as_uuid(), false).await;
    let actor_id = seed_org_and_user(&owner_pool, *knl.as_uuid(), true).await;
    let store = PgOrgStore::new(rt_pool.clone());

    let command = ActivateUserCommand {
        actor: UserId::from_uuid(actor_id),
        user_id: UserId::from_uuid(user_id),
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    };

    let first = CURRENT_ORG
        .scope(knl, store.activate_user(command.clone()))
        .await
        .expect("first activate must succeed as console_rt");
    assert!(first.is_active, "the user is reactivated");

    let replay = CURRENT_ORG
        .scope(knl, store.activate_user(command))
        .await
        .expect_err("replayed activate must be a Conflict no-op");
    assert_eq!(replay.kind(), ErrorKind::Conflict);

    assert_eq!(audit_count(&owner_pool, "user.activate", user_id).await, 1);
    assert_eq!(
        audit_count(&owner_pool, "policy.account.activate", user_id).await,
        1
    );
}
