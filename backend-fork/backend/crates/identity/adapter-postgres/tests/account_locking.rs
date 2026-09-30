//! Lifecycle owner serialization, including already-inactive cleanup.
//! Cleanup fixtures do not stand in for cryptographic authentication proof.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use console_identity_adapter_postgres::PgOrgStore;
use console_identity_application::DeactivateUserCommand;
use console_kernel_core::{ErrorKind, OrgId, TraceContext, UserId};
use console_platform_auth::RefreshTokenStore;
use console_platform_request_context::CURRENT_ORG;
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use time::{Duration, OffsetDateTime};
use tokio::time::{sleep, timeout};
use uuid::Uuid;
const WAIT: std::time::Duration = std::time::Duration::from_secs(8);

async fn runtime(owner: &PgPool, label: &str) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(
            owner
                .connect_options()
                .as_ref()
                .clone()
                .application_name(label),
        )
        .await
        .unwrap();
    let identity: (String, bool, bool) = sqlx::query_as(
        "SELECT current_user::text,rolsuper,rolbypassrls FROM pg_roles WHERE rolname=current_user",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(identity, ("console_rt".into(), false, false));
    pool
}
async fn user(owner: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO users(display_name,org_id,roles) VALUES('Session lock test',$1,ARRAY['MEMBER']) RETURNING id")
        .bind(*OrgId::knl().as_uuid()).fetch_one(owner).await.unwrap()
}
async fn account(owner: &PgPool, user: Uuid) -> (Transaction<'_, Postgres>, i32) {
    let mut tx = owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE")
        .bind(user)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    (tx, pid)
}
async fn blocked(owner: &PgPool, label: &str, blocker: i32) -> i32 {
    timeout(WAIT,async {
        loop {
            let waiting:Option<i32>=sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND application_name=$1 AND wait_event_type='Lock' AND $2=ANY(pg_blocking_pids(pid))")
                .bind(label).bind(blocker).fetch_optional(owner).await.unwrap();
            if let Some(pid) = waiting {break pid;}
            sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("actual owner must wait for the specified blocker")
}

async fn deactivate(
    rt: PgPool,
    user: Uuid,
    actor: Uuid,
) -> Result<console_identity_application::UserSummary, console_identity_adapter_postgres::PgOrgError>
{
    CURRENT_ORG
        .scope(
            OrgId::knl(),
            PgOrgStore::new(rt).deactivate_user(DeactivateUserCommand {
                user_id: UserId::from_uuid(user),
                actor: UserId::from_uuid(actor),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            }),
        )
        .await
}
async fn lifecycle_wait(owner: &PgPool, inactive: bool) {
    let target = user(owner).await;
    let actor = user(owner).await;
    let rt = runtime(owner, "deactivate-account").await;
    let issued = RefreshTokenStore
        .issue_family(
            &rt,
            target,
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::minutes(5),
        )
        .await
        .unwrap();
    // Legacy opaque row: deletion scheduling only; no signature verification is claimed.
    let key:Uuid=sqlx::query_scalar("INSERT INTO auth_webauthn_credentials(user_id,credential_id,passkey_json,org_id) VALUES($1,$2,'{}',$3) RETURNING id").bind(target).bind(Uuid::new_v4().to_string()).bind(*OrgId::knl().as_uuid()).fetch_one(owner).await.unwrap();
    if inactive {
        sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
            .bind(target)
            .execute(owner)
            .await
            .unwrap();
    }
    let (held, pid) = account(owner, target).await;
    let task = tokio::spawn(deactivate(rt, target, actor));
    blocked(owner, "deactivate-account", pid).await;
    let mut probe = owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM auth_webauthn_credentials WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(key)
        .fetch_one(&mut *probe)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM auth_refresh_token_families WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(issued.family_id)
        .fetch_one(&mut *probe)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM auth_refresh_tokens WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(issued.token_id)
        .fetch_one(&mut *probe)
        .await
        .unwrap();
    probe.rollback().await.unwrap();
    held.rollback().await.unwrap();
    let result = timeout(WAIT, task).await.unwrap().unwrap();
    if inactive {
        assert_eq!(result.unwrap_err().kind(), ErrorKind::Conflict);
    } else {
        result.unwrap();
    }
    let state:(bool,i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT NOT is_active FROM users WHERE id=$1),(SELECT count(*) FROM auth_webauthn_credentials WHERE user_id=$1),(SELECT count(*) FROM auth_refresh_token_families WHERE user_id=$1 AND revoked_at IS NULL),(SELECT count(*) FROM auth_refresh_tokens WHERE user_id=$1 AND revoked_at IS NULL),(SELECT count(*) FROM audit_events WHERE target_id=$2 AND action='user.deactivate')").bind(target).bind(target.to_string()).fetch_one(owner).await.unwrap();
    assert_eq!(state, (true, 0, 0, 0, if inactive { 0 } else { 1 }));
}
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn deactivation_waits_before_auth_children(owner: PgPool) {
    lifecycle_wait(&owner, false).await;
}
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn already_inactive_cleanup_waits_before_auth_children(owner: PgPool) {
    lifecycle_wait(&owner, true).await;
}
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn lifecycle_mutex_allows_unrelated_actor_and_child_fk_key_share(owner: PgPool) {
    let target = user(&owner).await;
    let actor = user(&owner).await;
    let rt = runtime(&owner, "deactivate-key-share").await;
    let mut held = owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=ANY($1) FOR KEY SHARE")
        .bind(vec![target, actor])
        .fetch_all(&mut *held)
        .await
        .unwrap();
    sqlx::query("SET lock_timeout='500ms'")
        .execute(&rt)
        .await
        .unwrap();
    deactivate(rt, target, actor).await.unwrap();
    held.rollback().await.unwrap();
}
