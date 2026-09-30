//! Account serialization around actual refresh-family/token owner transactions.
//! SET ROLE is an enforcement fixture, not proof of password isolation.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use console_kernel_core::OrgId;
use console_platform_auth::{RefreshTokenStore, RefreshTokenUseError};
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
async fn issue(pool: &PgPool, user: Uuid) -> console_platform_auth::RefreshTokenIssue {
    RefreshTokenStore
        .issue_family(
            pool,
            user,
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::minutes(10),
        )
        .await
        .unwrap()
}
async fn token_and_family_unlocked(owner: &PgPool, token: Uuid, family: Uuid) {
    let mut probe = owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM auth_refresh_token_families WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(family)
        .fetch_one(&mut *probe)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM auth_refresh_tokens WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(token)
        .fetch_one(&mut *probe)
        .await
        .unwrap();
    probe.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn family_issuance_waits_and_rechecks_active_account(owner: PgPool) {
    let user = user(&owner).await;
    let rt = runtime(&owner, "family-issue-account").await;
    let (mut held, pid) = account(&owner, user).await;
    let task = tokio::spawn(async move {
        RefreshTokenStore
            .issue_family(
                &rt,
                user,
                OrgId::knl(),
                OffsetDateTime::now_utc(),
                Duration::minutes(10),
            )
            .await
    });
    blocked(&owner, "family-issue-account", pid).await;
    sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
        .bind(user)
        .execute(&mut *held)
        .await
        .unwrap();
    held.commit().await.unwrap();
    assert!(timeout(WAIT, task).await.unwrap().unwrap().is_err());
    let state:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM auth_refresh_token_families WHERE user_id=$1),(SELECT count(*) FROM auth_refresh_tokens WHERE user_id=$1),(SELECT count(*) FROM audit_events WHERE actor=$1 AND action='auth.refresh.issue')")
        .bind(user).fetch_one(&owner).await.unwrap();
    assert_eq!(state, (0, 0, 0));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn rotation_waits_before_locking_family_and_token(owner: PgPool) {
    let user = user(&owner).await;
    let rt = runtime(&owner, "rotate-account").await;
    let first = issue(&rt, user).await;
    let (held, pid) = account(&owner, user).await;
    let family = first.family_id;
    let token = first.token_id;
    let task = tokio::spawn(async move {
        RefreshTokenStore
            .rotate(
                &rt,
                first.token.as_str(),
                OffsetDateTime::now_utc(),
                Duration::minutes(10),
                Duration::days(1),
            )
            .await
    });
    blocked(&owner, "rotate-account", pid).await;
    token_and_family_unlocked(&owner, token, family).await;
    held.rollback().await.unwrap();
    let second = timeout(WAIT, task).await.unwrap().unwrap().unwrap();
    assert_eq!(second.family_id, family);
    assert_eq!(second.user_id, user);
    assert_ne!(second.token_id, token);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn rotation_expired_during_account_wait_has_no_replacement(owner: PgPool) {
    let user = user(&owner).await;
    let rt = runtime(&owner, "rotate-account-expiry").await;
    let first = issue(&rt, user).await;
    let token = first.token_id;
    let family = first.family_id;
    sqlx::query("UPDATE auth_refresh_tokens SET expires_at=clock_timestamp()+interval '2 seconds' WHERE id=$1")
        .bind(token).execute(&owner).await.unwrap();
    let (held, pid) = account(&owner, user).await;
    let task = tokio::spawn(async move {
        RefreshTokenStore
            .rotate(
                &rt,
                first.token.as_str(),
                OffsetDateTime::now_utc(),
                Duration::minutes(10),
                Duration::days(1),
            )
            .await
    });
    blocked(&owner, "rotate-account-expiry", pid).await;
    timeout(WAIT, async {
        loop {
            let expired: bool = sqlx::query_scalar(
                "SELECT clock_timestamp()>=expires_at FROM auth_refresh_tokens WHERE id=$1",
            )
            .bind(token)
            .fetch_one(&owner)
            .await
            .unwrap();
            if expired {
                break;
            }
            sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    held.rollback().await.unwrap();
    assert_eq!(
        timeout(WAIT, task).await.unwrap().unwrap().unwrap_err(),
        RefreshTokenUseError::Expired
    );
    let state:(i64,bool,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM auth_refresh_tokens WHERE family_id=$1),(SELECT used_at IS NULL AND replaced_by IS NULL FROM auth_refresh_tokens WHERE id=$2),(SELECT count(*) FROM audit_events WHERE target_id=$3 AND action='auth.refresh')")
        .bind(family).bind(token).bind(family.to_string()).fetch_one(&owner).await.unwrap();
    assert_eq!(state, (1, true, 0));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn logout_serializes_inactive_cleanup_before_child_mutations(owner: PgPool) {
    let user = user(&owner).await;
    let rt = runtime(&owner, "logout-account").await;
    let first = issue(&rt, user).await;
    let family = first.family_id;
    let token = first.token_id;
    sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
        .bind(user)
        .execute(&owner)
        .await
        .unwrap();
    let already_revoked: bool = sqlx::query_scalar(
        "SELECT revoked_at IS NOT NULL FROM auth_refresh_token_families WHERE id=$1",
    )
    .bind(family)
    .fetch_one(&owner)
    .await
    .unwrap();
    let (held, pid) = account(&owner, user).await;
    let task = tokio::spawn(async move {
        RefreshTokenStore
            .revoke_family_for_logout(&rt, first.token.as_str(), OffsetDateTime::now_utc())
            .await
    });
    blocked(&owner, "logout-account", pid).await;
    token_and_family_unlocked(&owner, token, family).await;
    held.rollback().await.unwrap();
    let result = timeout(WAIT, task).await.unwrap().unwrap();
    if already_revoked {
        assert_eq!(result.unwrap_err(), RefreshTokenUseError::FamilyRevoked);
    } else {
        result.unwrap();
    }
    let state:(bool,i64,i64)=sqlx::query_as("SELECT (SELECT revoked_at IS NOT NULL FROM auth_refresh_token_families WHERE id=$1),(SELECT count(*) FROM auth_refresh_tokens WHERE family_id=$1 AND revoked_at IS NULL),(SELECT count(*) FROM audit_events WHERE target_id=$2 AND action='auth.logout')")
        .bind(family).bind(family.to_string()).fetch_one(&owner).await.unwrap();
    assert_eq!(state, (true, 0, if already_revoked { 0 } else { 1 }));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn session_owner_mutex_allows_child_fk_key_share(owner: PgPool) {
    let user = user(&owner).await;
    let rt = runtime(&owner, "session-fk-share").await;
    let mut fk = owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR KEY SHARE")
        .bind(user)
        .fetch_one(&mut *fk)
        .await
        .unwrap();
    sqlx::query("SET lock_timeout='500ms'")
        .execute(&rt)
        .await
        .unwrap();
    let first = issue(&rt, user).await;
    let next = RefreshTokenStore
        .rotate(
            &rt,
            first.token.as_str(),
            OffsetDateTime::now_utc(),
            Duration::minutes(10),
            Duration::days(1),
        )
        .await
        .unwrap();
    RefreshTokenStore
        .revoke_family_for_logout(&rt, next.token.as_str(), OffsetDateTime::now_utc())
        .await
        .unwrap();
    fk.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn refresh_locator_replacement_does_not_switch_locked_account(owner: PgPool) {
    for logout in [false, true] {
        let a = user(&owner).await;
        let b = user(&owner).await;
        let rt = runtime(&owner, "refresh-locator-rebind").await;
        let first = issue(&rt, a).await;
        let token = first.token_id;
        let family = first.family_id;
        let hash: Vec<u8> =
            sqlx::query_scalar("SELECT token_hash FROM auth_refresh_tokens WHERE id=$1")
                .bind(token)
                .fetch_one(&owner)
                .await
                .unwrap();
        let (held, pid) = account(&owner, a).await;
        let task = tokio::spawn(async move {
            if logout {
                RefreshTokenStore
                    .revoke_family_for_logout(&rt, first.token.as_str(), OffsetDateTime::now_utc())
                    .await
            } else {
                RefreshTokenStore
                    .rotate(
                        &rt,
                        first.token.as_str(),
                        OffsetDateTime::now_utc(),
                        Duration::minutes(10),
                        Duration::days(1),
                    )
                    .await
                    .map(|_| ())
            }
        });
        blocked(&owner, "refresh-locator-rebind", pid).await;
        let mut replacement = owner.begin().await.unwrap();
        sqlx::query("SET LOCAL lock_timeout='500ms'")
            .execute(&mut *replacement)
            .await
            .unwrap();
        sqlx::query("DELETE FROM auth_refresh_tokens WHERE family_id=$1")
            .bind(family)
            .execute(&mut *replacement)
            .await
            .unwrap();
        sqlx::query("DELETE FROM auth_refresh_token_families WHERE id=$1")
            .bind(family)
            .execute(&mut *replacement)
            .await
            .unwrap();
        let new_family = Uuid::new_v4();
        let new_token = Uuid::new_v4();
        sqlx::query("INSERT INTO auth_refresh_token_families(id,user_id,created_at,org_id) VALUES($1,$2,clock_timestamp(),$3)")
        .bind(new_family).bind(b).bind(*OrgId::knl().as_uuid()).execute(&mut *replacement).await.unwrap();
        sqlx::query("INSERT INTO auth_refresh_tokens(id,family_id,user_id,token_hash,issued_at,expires_at,org_id) VALUES($1,$2,$3,$4,clock_timestamp(),clock_timestamp()+interval '10 minutes',$5)")
        .bind(new_token).bind(new_family).bind(b).bind(hash).bind(*OrgId::knl().as_uuid()).execute(&mut *replacement).await.unwrap();
        replacement.commit().await.unwrap();
        held.rollback().await.unwrap();
        assert_eq!(
            timeout(WAIT, task).await.unwrap().unwrap().unwrap_err(),
            RefreshTokenUseError::InvalidToken
        );
        let state:(i64,bool,i64,bool)=sqlx::query_as("SELECT (SELECT count(*) FROM auth_refresh_tokens WHERE family_id=$1),(SELECT used_at IS NULL AND replaced_by IS NULL AND revoked_at IS NULL FROM auth_refresh_tokens WHERE id=$2),(SELECT count(*) FROM audit_events WHERE actor=$3 AND action IN ('auth.refresh','auth.logout')),(SELECT revoked_at IS NULL FROM auth_refresh_token_families WHERE id=$1)")
        .bind(new_family).bind(new_token).bind(b).fetch_one(&owner).await.unwrap();
        assert_eq!(state, (1, true, 0, true));
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn different_families_serialize_per_account_without_global_lock(owner: PgPool) {
    let a = user(&owner).await;
    let b = user(&owner).await;
    let rt1 = runtime(&owner, "session-family-one").await;
    let rt2 = runtime(&owner, "session-family-two").await;
    let independent = runtime(&owner, "session-independent").await;
    let first = issue(&rt1, a).await;
    let sibling = issue(&rt2, a).await;
    let other = issue(&independent, b).await;
    let mut held = owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM auth_refresh_token_families WHERE id=$1 FOR UPDATE")
        .bind(first.family_id)
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let task1 = tokio::spawn(async move {
        RefreshTokenStore
            .rotate(
                &rt1,
                first.token.as_str(),
                OffsetDateTime::now_utc(),
                Duration::minutes(10),
                Duration::days(1),
            )
            .await
    });
    let first_pid = blocked(&owner, "session-family-one", pid).await;
    let sibling_token = sibling.token_id;
    let sibling_family = sibling.family_id;
    let task2 = tokio::spawn(async move {
        RefreshTokenStore
            .rotate(
                &rt2,
                sibling.token.as_str(),
                OffsetDateTime::now_utc(),
                Duration::minutes(10),
                Duration::days(1),
            )
            .await
    });
    blocked(&owner, "session-family-two", first_pid).await;
    token_and_family_unlocked(&owner, sibling_token, sibling_family).await;
    let rotated_other = timeout(
        WAIT,
        RefreshTokenStore.rotate(
            &independent,
            other.token.as_str(),
            OffsetDateTime::now_utc(),
            Duration::minutes(10),
            Duration::days(1),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(rotated_other.user_id, b);
    held.rollback().await.unwrap();
    assert_eq!(
        timeout(WAIT, task1)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .user_id,
        a
    );
    assert_eq!(
        timeout(WAIT, task2)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .user_id,
        a
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn rotation_denies_account_deactivated_during_wait(owner: PgPool) {
    let user = user(&owner).await;
    let rt = runtime(&owner, "rotate-account-inactive").await;
    let first = issue(&rt, user).await;
    let family = first.family_id;
    let token = first.token_id;
    let (mut held, pid) = account(&owner, user).await;
    let task = tokio::spawn(async move {
        RefreshTokenStore
            .rotate(
                &rt,
                first.token.as_str(),
                OffsetDateTime::now_utc(),
                Duration::minutes(10),
                Duration::days(1),
            )
            .await
    });
    blocked(&owner, "rotate-account-inactive", pid).await;
    token_and_family_unlocked(&owner, token, family).await;
    sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
        .bind(user)
        .execute(&mut *held)
        .await
        .unwrap();
    held.commit().await.unwrap();
    assert!(matches!(
        timeout(WAIT, task).await.unwrap().unwrap(),
        Err(RefreshTokenUseError::InvalidToken | RefreshTokenUseError::FamilyRevoked)
    ));
    let state:(i64,bool,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM auth_refresh_tokens WHERE family_id=$1),(SELECT used_at IS NULL AND replaced_by IS NULL FROM auth_refresh_tokens WHERE id=$2),(SELECT count(*) FROM audit_events WHERE actor=$3 AND action='auth.refresh')").bind(family).bind(token).bind(user).fetch_one(&owner).await.unwrap();
    assert_eq!(state, (1, true, 0));
}
