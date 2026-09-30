//! Actual provisioning transactions and Account/child lock ordering.
//! SET ROLE tests RLS/privilege enforcement, not password isolation.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use console_kernel_core::OrgId;
use console_platform_auth::{PasskeyRegistrationStart, PasskeyService, WebauthnSettings};
use console_platform_provisioning::{
    BootstrapCredentialIssue, BootstrapCredentialStore, ProvisioningError, RosterProvisioner,
};
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use time::{Duration, OffsetDateTime};
use tokio::time::{sleep, timeout};
use url::Url;
use uuid::Uuid;
use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};
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

async fn provision(
    pool: &PgPool,
    user: Uuid,
    mode: u8,
) -> Result<BootstrapCredentialIssue, ProvisioningError> {
    let now = OffsetDateTime::now_utc();
    let ttl = Duration::minutes(5);
    match mode {
        0 => {
            BootstrapCredentialStore
                .issue_for_zero_credential_user(pool, user, OrgId::knl(), now, ttl)
                .await
        }
        1 => {
            BootstrapCredentialStore
                .reset_credentials_for_user(pool, user, OrgId::knl(), now, ttl)
                .await
        }
        2 => {
            BootstrapCredentialStore
                .issue_self_enroll_handoff(pool, user, OrgId::knl(), now, ttl)
                .await
        }
        _ => panic!("unknown test mode"),
    }
}
async fn register(pool: &PgPool, user: Uuid) -> Uuid {
    let origin = Url::parse("https://auth.example.com").unwrap();
    let service = PasskeyService::new(WebauthnSettings {
        rp_id: "example.com".into(),
        rp_origin: origin.clone(),
        rp_name: "Console".into(),
        extra_allowed_origins: vec![],
        ceremony_ttl: Duration::minutes(5),
    })
    .unwrap();
    let start = service
        .start_registration(
            pool,
            OrgId::knl(),
            PasskeyRegistrationStart {
                user_id: user,
                username: "locking".into(),
                display_name: "Locking".into(),
            },
        )
        .await
        .unwrap();
    let mut client = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = client.do_registration(origin, start.challenge).unwrap();
    let stored = service
        .finish_registration(pool, OrgId::knl(), start.ceremony_id, credential)
        .await
        .unwrap();
    sqlx::query_scalar("SELECT id FROM auth_webauthn_credentials WHERE credential_id=$1")
        .bind(stored.credential_id)
        .fetch_one(pool)
        .await
        .unwrap()
}
async fn child_probe(owner: &PgPool, user: Uuid) {
    let mut tx = owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM auth_bootstrap_credentials WHERE user_id=$1 FOR UPDATE NOWAIT")
        .bind(user)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM auth_webauthn_credentials WHERE user_id=$1 FOR UPDATE NOWAIT")
        .bind(user)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
}
async fn producer_wait(owner: &PgPool, mode: u8) {
    let user = user(owner).await;
    let rt = runtime(owner, "bootstrap-owner-account").await;
    let old = provision(&rt, user, 0).await.unwrap();
    sqlx::query("UPDATE auth_bootstrap_credentials SET issued_at=clock_timestamp()-interval '2 minutes', expires_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(old.credential_id).execute(owner).await.unwrap();
    let key = if mode > 0 {
        Some(register(owner, user).await)
    } else {
        None
    };
    let (held, pid) = account(owner, user).await;
    let task = tokio::spawn(async move { provision(&rt, user, mode).await });
    blocked(owner, "bootstrap-owner-account", pid).await;
    child_probe(owner, user).await;
    held.rollback().await.unwrap();
    let result = timeout(WAIT, task).await.unwrap().unwrap().unwrap();
    assert_ne!(result.credential_id, old.credential_id);
    let old_revoked: bool = sqlx::query_scalar(
        "SELECT revoked_at IS NOT NULL FROM auth_bootstrap_credentials WHERE id=$1",
    )
    .bind(old.credential_id)
    .fetch_one(owner)
    .await
    .unwrap();
    assert!(old_revoked);
    if let Some(key) = key {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM auth_webauthn_credentials WHERE id=$1)",
        )
        .bind(key)
        .fetch_one(owner)
        .await
        .unwrap();
        assert_eq!(exists, mode == 2);
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn bootstrap_issue_locks_account_before_expired_child(owner: PgPool) {
    producer_wait(&owner, 0).await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn reset_locks_account_before_key_and_bootstrap_children(owner: PgPool) {
    producer_wait(&owner, 1).await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn self_enroll_handoff_locks_account_before_bootstrap_child(owner: PgPool) {
    producer_wait(&owner, 2).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn all_bootstrap_producers_deny_deactivation_during_wait(owner: PgPool) {
    for mode in 0..3 {
        let user = user(&owner).await;
        let rt = runtime(&owner, "bootstrap-inactive").await;
        let (mut held, pid) = account(&owner, user).await;
        let task = tokio::spawn(async move { provision(&rt, user, mode).await });
        blocked(&owner, "bootstrap-inactive", pid).await;
        sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
            .bind(user)
            .execute(&mut *held)
            .await
            .unwrap();
        held.commit().await.unwrap();
        assert!(timeout(WAIT, task).await.unwrap().unwrap().is_err());
        let state:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM auth_bootstrap_credentials WHERE user_id=$1),(SELECT count(*) FROM audit_events WHERE actor=$1 OR target_id=$2)").bind(user).bind(user.to_string()).fetch_one(&owner).await.unwrap();
        assert_eq!(state, (0, 0));
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn bootstrap_producers_allow_child_fk_key_share(owner: PgPool) {
    for mode in 0..3 {
        let user = user(&owner).await;
        let rt = runtime(&owner, "bootstrap-key-share").await;
        let mut held = owner.begin().await.unwrap();
        sqlx::query("SELECT id FROM users WHERE id=$1 FOR KEY SHARE")
            .bind(user)
            .fetch_one(&mut *held)
            .await
            .unwrap();
        sqlx::query("SET lock_timeout='500ms'")
            .execute(&rt)
            .await
            .unwrap();
        provision(&rt, user, mode).await.unwrap();
        held.rollback().await.unwrap();
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn otp_redeem_rechecks_expiry_after_account_wait(owner: PgPool) {
    let user = user(&owner).await;
    let rt = runtime(&owner, "redeem-account-expiry").await;
    let issued = provision(&rt, user, 0).await.unwrap();
    let id = issued.credential_id;
    sqlx::query("UPDATE auth_bootstrap_credentials SET expires_at=clock_timestamp()+interval '2 seconds' WHERE id=$1").bind(id).execute(&owner).await.unwrap();
    let (held, pid) = account(&owner, user).await;
    let task = tokio::spawn(async move {
        BootstrapCredentialStore
            .redeem_otp(&rt, issued.token.as_str(), OffsetDateTime::now_utc())
            .await
    });
    blocked(&owner, "redeem-account-expiry", pid).await;
    child_probe(&owner, user).await;
    timeout(WAIT, async {
        loop {
            let expired: bool = sqlx::query_scalar(
                "SELECT clock_timestamp()>=expires_at FROM auth_bootstrap_credentials WHERE id=$1",
            )
            .bind(id)
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
    assert!(matches!(
        timeout(WAIT, task).await.unwrap().unwrap(),
        Err(ProvisioningError::InvalidBootstrapCredential)
    ));
    let state:(bool,i64)=sqlx::query_as("SELECT (SELECT consumed_at IS NULL FROM auth_bootstrap_credentials WHERE id=$1),(SELECT count(*) FROM audit_events WHERE actor=$2 AND action='auth.otp.redeem')").bind(id).bind(user).fetch_one(&owner).await.unwrap();
    assert_eq!(state, (true, 0));
}
#[sqlx::test(migrations = "../db/migrations")]
async fn bootstrap_consume_waits_before_child_and_retains_caller_rollback(owner: PgPool) {
    let user = user(&owner).await;
    let rt = runtime(&owner, "consume-account").await;
    let issued = provision(&rt, user, 0).await.unwrap();
    let (held, pid) = account(&owner, user).await;
    let task = tokio::spawn(async move {
        let mut tx = rt.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org',$1,true)")
            .bind(OrgId::knl().as_uuid().to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        BootstrapCredentialStore
            .consume_open_credentials_tx(&mut tx, OrgId::knl(), user, OffsetDateTime::now_utc())
            .await
            .unwrap();
        let consumed: bool = sqlx::query_scalar(
            "SELECT consumed_at IS NOT NULL FROM auth_bootstrap_credentials WHERE id=$1",
        )
        .bind(issued.credential_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert!(consumed);
        tx.rollback().await.unwrap();
    });
    blocked(&owner, "consume-account", pid).await;
    child_probe(&owner, user).await;
    held.rollback().await.unwrap();
    timeout(WAIT, task).await.unwrap().unwrap();
    let state:(bool,i64)=sqlx::query_as("SELECT (SELECT consumed_at IS NULL FROM auth_bootstrap_credentials WHERE id=$1),(SELECT count(*) FROM audit_events WHERE actor=$2 AND action='auth.otp.consume')").bind(issued.credential_id).bind(user).fetch_one(&owner).await.unwrap();
    assert_eq!(state, (true, 0));
}
async fn roster_user(owner: &PgPool, id: Uuid, phone: &str) {
    sqlx::query("INSERT INTO users(id,display_name,phone,roles,org_id) VALUES($1,'Original',$2,ARRAY['MEMBER'],$3)").bind(id).bind(phone).bind(*OrgId::knl().as_uuid()).execute(owner).await.unwrap();
}
fn roster(phones: &[&str]) -> String {
    serde_json::json!({"users":phones.iter().map(|p|serde_json::json!({"display_name":"Updated","phone":p,"roles":["MEMBER"],"branches":[]})).collect::<Vec<_>>()}).to_string()
}
async fn import(
    rt: &PgPool,
    phones: &[&str],
) -> Result<console_platform_provisioning::RosterImportReport, ProvisioningError> {
    RosterProvisioner::new(Duration::minutes(5))
        .import_json(rt, &roster(phones), OffsetDateTime::now_utc())
        .await
}
#[sqlx::test(migrations = "../db/migrations")]
async fn roster_locks_complete_existing_set_in_uuid_order_before_writes(owner: PgPool) {
    let low = Uuid::from_u128(1);
    let high = Uuid::from_u128(2);
    roster_user(&owner, low, "010-2000-0002").await;
    roster_user(&owner, high, "010-2000-0001").await;
    let rt = runtime(&owner, "roster-sorted").await;
    let (held, pid) = account(&owner, low).await;
    let task = tokio::spawn(async move { import(&rt, &["010-2000-0001", "010-2000-0002"]).await });
    blocked(&owner, "roster-sorted", pid).await;
    let mut probe = owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE NOWAIT")
        .bind(high)
        .fetch_one(&mut *probe)
        .await
        .unwrap();
    probe.rollback().await.unwrap();
    held.rollback().await.unwrap();
    let result = timeout(WAIT, task).await.unwrap().unwrap().unwrap();
    assert_eq!(result.users_updated, 2);
    assert_eq!(result.users_created, 0);
    assert!(result.bootstrap_credentials_issued.is_empty());
}
#[sqlx::test(migrations = "../db/migrations")]
async fn roster_mapping_change_during_wait_rolls_back_entire_import(owner: PgPool) {
    let low = Uuid::from_u128(1);
    let high = Uuid::from_u128(2);
    roster_user(&owner, low, "010-2100-0001").await;
    roster_user(&owner, high, "010-2100-0002").await;
    let rt = runtime(&owner, "roster-renamed").await;
    let (held, pid) = account(&owner, low).await;
    let task = tokio::spawn(async move { import(&rt, &["010-2100-0001", "010-2100-0002"]).await });
    blocked(&owner, "roster-renamed", pid).await;
    sqlx::query("UPDATE users SET phone='010-2100-9999' WHERE id=$1")
        .bind(high)
        .execute(&owner)
        .await
        .unwrap();
    held.rollback().await.unwrap();
    assert!(matches!(
        timeout(WAIT, task).await.unwrap().unwrap(),
        Err(ProvisioningError::Conflict(_))
    ));
    let state:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM users WHERE id=ANY($1) AND display_name='Original'),(SELECT count(*) FROM users WHERE phone='010-2100-0002'),(SELECT count(*) FROM audit_events WHERE action='roster.import')").bind(vec![low,high]).fetch_one(&owner).await.unwrap();
    assert_eq!(state, (2, 0, 0));
}
#[sqlx::test(migrations = "../db/migrations")]
async fn roster_does_not_adopt_account_created_after_mapping_snapshot(owner: PgPool) {
    let low = Uuid::from_u128(1);
    let intervening = Uuid::from_u128(2);
    roster_user(&owner, low, "010-2200-0001").await;
    let rt = runtime(&owner, "roster-intervening").await;
    let (held, pid) = account(&owner, low).await;
    let task = tokio::spawn(async move { import(&rt, &["010-2200-0001", "010-2200-0002"]).await });
    blocked(&owner, "roster-intervening", pid).await;
    roster_user(&owner, intervening, "010-2200-0002").await;
    held.rollback().await.unwrap();
    assert!(matches!(
        timeout(WAIT, task).await.unwrap().unwrap(),
        Err(ProvisioningError::Conflict(_))
    ));
    let state:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM users WHERE id=ANY($1) AND display_name='Original'),(SELECT count(*) FROM auth_bootstrap_credentials WHERE user_id=ANY($1)),(SELECT count(*) FROM audit_events WHERE action='roster.import')").bind(vec![low,intervening]).fetch_one(&owner).await.unwrap();
    assert_eq!(state, (2, 0, 0));
}
#[sqlx::test(migrations = "../db/migrations")]
async fn new_roster_phones_sort_before_insert_and_uniqueness_race_rolls_back(owner: PgPool) {
    let rt = runtime(&owner, "roster-new-conflict").await;
    let mut first = owner.begin().await.unwrap();
    let low = Uuid::from_u128(1);
    sqlx::query("INSERT INTO users(id,display_name,phone,roles,org_id) VALUES($1,'Independent','010-2300-0001',ARRAY['MEMBER'],$2)").bind(low).bind(*OrgId::knl().as_uuid()).execute(&mut *first).await.unwrap();
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *first)
        .await
        .unwrap();
    let task = tokio::spawn(async move { import(&rt, &["010-2300-0002", "010-2300-0001"]).await });
    blocked(&owner, "roster-new-conflict", pid).await;
    let mut second = owner.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='500ms'")
        .execute(&mut *second)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users(display_name,phone,roles,org_id) VALUES('Independent','010-2300-0002',ARRAY['MEMBER'],$1)").bind(*OrgId::knl().as_uuid()).execute(&mut *second).await.unwrap();
    second.commit().await.unwrap();
    first.commit().await.unwrap();
    assert!(timeout(WAIT, task).await.unwrap().unwrap().is_err());
    let state:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM users WHERE phone LIKE '010-2300-%' AND display_name='Independent'),(SELECT count(*) FROM auth_bootstrap_credentials c JOIN users u ON u.id=c.user_id WHERE u.phone LIKE '010-2300-%'),(SELECT count(*) FROM audit_events WHERE action='roster.import')").fetch_one(&owner).await.unwrap();
    assert_eq!(state, (2, 0, 0));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn otp_redeem_denies_deactivation_during_account_wait(owner: PgPool) {
    let user = user(&owner).await;
    let rt = runtime(&owner, "redeem-inactive").await;
    let issued = provision(&rt, user, 0).await.unwrap();
    let (mut held, pid) = account(&owner, user).await;
    let task = tokio::spawn(async move {
        BootstrapCredentialStore
            .redeem_otp(&rt, issued.token.as_str(), OffsetDateTime::now_utc())
            .await
    });
    blocked(&owner, "redeem-inactive", pid).await;
    sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
        .bind(user)
        .execute(&mut *held)
        .await
        .unwrap();
    held.commit().await.unwrap();
    assert!(matches!(
        timeout(WAIT, task).await.unwrap().unwrap(),
        Err(ProvisioningError::InvalidBootstrapCredential)
    ));
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE actor=$1 AND action='auth.otp.redeem'",
    )
    .bind(user)
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(audits, 0);
}
#[sqlx::test(migrations = "../db/migrations")]
async fn otp_locator_replacement_cannot_switch_locked_account(owner: PgPool) {
    let a = user(&owner).await;
    let b = user(&owner).await;
    let rt = runtime(&owner, "redeem-rebind").await;
    let issued = provision(&rt, a, 0).await.unwrap();
    let old_id = issued.credential_id;
    let hash: Vec<u8> =
        sqlx::query_scalar("SELECT token_hash FROM auth_bootstrap_credentials WHERE id=$1")
            .bind(old_id)
            .fetch_one(&owner)
            .await
            .unwrap();
    let (held, pid) = account(&owner, a).await;
    let task = tokio::spawn(async move {
        BootstrapCredentialStore
            .redeem_otp(&rt, issued.token.as_str(), OffsetDateTime::now_utc())
            .await
    });
    blocked(&owner, "redeem-rebind", pid).await;
    let mut replacement = owner.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='500ms'")
        .execute(&mut *replacement)
        .await
        .unwrap();
    sqlx::query("DELETE FROM auth_bootstrap_credentials WHERE id=$1")
        .bind(old_id)
        .execute(&mut *replacement)
        .await
        .unwrap();
    let id:Uuid=sqlx::query_scalar("INSERT INTO auth_bootstrap_credentials(user_id,token_hash,issued_at,expires_at,org_id) VALUES($1,$2,clock_timestamp(),clock_timestamp()+interval '5 minutes',$3) RETURNING id").bind(b).bind(hash).bind(*OrgId::knl().as_uuid()).fetch_one(&mut *replacement).await.unwrap();
    replacement.commit().await.unwrap();
    held.rollback().await.unwrap();
    assert!(matches!(
        timeout(WAIT, task).await.unwrap().unwrap(),
        Err(ProvisioningError::InvalidBootstrapCredential)
    ));
    let state:(bool,i64)=sqlx::query_as("SELECT (SELECT consumed_at IS NULL AND revoked_at IS NULL FROM auth_bootstrap_credentials WHERE id=$1),(SELECT count(*) FROM audit_events WHERE actor=ANY($2) AND action='auth.otp.redeem')").bind(id).bind(vec![a,b]).fetch_one(&owner).await.unwrap();
    assert_eq!(state, (true, 0));
}
async fn consume_after_wait(owner: &PgPool, inactive: bool) {
    let user = user(owner).await;
    let rt = runtime(owner, "consume-fresh-state").await;
    let issued = provision(&rt, user, 0).await.unwrap();
    let id = issued.credential_id;
    if !inactive {
        sqlx::query("UPDATE auth_bootstrap_credentials SET expires_at=clock_timestamp()+interval '2 seconds' WHERE id=$1").bind(id).execute(owner).await.unwrap();
    }
    let (mut held, pid) = account(owner, user).await;
    let task = tokio::spawn(async move {
        let mut tx = rt.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org',$1,true)")
            .bind(OrgId::knl().as_uuid().to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        let result = BootstrapCredentialStore
            .consume_open_credentials_tx(&mut tx, OrgId::knl(), user, OffsetDateTime::now_utc())
            .await;
        if result.is_ok() {
            tx.commit().await.unwrap();
        } else {
            tx.rollback().await.unwrap();
        }
        result
    });
    blocked(owner, "consume-fresh-state", pid).await;
    if inactive {
        sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
            .bind(user)
            .execute(&mut *held)
            .await
            .unwrap();
        held.commit().await.unwrap();
    } else {
        timeout(WAIT,async {loop {
            let expired:bool=sqlx::query_scalar("SELECT clock_timestamp()>=expires_at FROM auth_bootstrap_credentials WHERE id=$1").bind(id).fetch_one(owner).await.unwrap();
            if expired {break;} sleep(std::time::Duration::from_millis(5)).await;
        }}).await.unwrap();
        held.rollback().await.unwrap();
    }
    let result = timeout(WAIT, task).await.unwrap().unwrap();
    if inactive {
        assert!(result.is_err());
    } else {
        result.unwrap();
    }
    let state:(bool,i64)=sqlx::query_as("SELECT (SELECT consumed_at IS NULL FROM auth_bootstrap_credentials WHERE id=$1),(SELECT count(*) FROM audit_events WHERE actor=$2 AND action='auth.otp.consume')").bind(id).bind(user).fetch_one(owner).await.unwrap();
    assert_eq!(state, (true, 0));
}
#[sqlx::test(migrations = "../db/migrations")]
async fn bootstrap_consume_expired_during_wait_is_not_consumed(owner: PgPool) {
    consume_after_wait(&owner, false).await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn bootstrap_consume_inactive_during_wait_is_denied(owner: PgPool) {
    consume_after_wait(&owner, true).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn roster_locks_all_accounts_before_mutating_first_accounts_children(owner: PgPool) {
    let low = Uuid::from_u128(1);
    let high = Uuid::from_u128(2);
    roster_user(&owner, low, "010-2400-0001").await;
    roster_user(&owner, high, "010-2400-0002").await;
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions(name,org_id) VALUES('Lock test',$1) RETURNING id")
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(&owner)
            .await
            .unwrap();
    let branch: Uuid = sqlx::query_scalar(
        "INSERT INTO branches(name,region_id,org_id) VALUES('Lock test',$1,$2) RETURNING id",
    )
    .bind(region)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(&owner)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_branches(user_id,branch_id,org_id) VALUES($1,$2,$3)")
        .bind(low)
        .bind(branch)
        .bind(*OrgId::knl().as_uuid())
        .execute(&owner)
        .await
        .unwrap();
    let rt = runtime(&owner, "roster-all-before-children").await;
    let (held, pid) = account(&owner, high).await;
    let task = tokio::spawn(async move { import(&rt, &["010-2400-0001", "010-2400-0002"]).await });
    blocked(&owner, "roster-all-before-children", pid).await;
    let mut probe = owner.begin().await.unwrap();
    sqlx::query(
        "SELECT user_id FROM user_branches WHERE user_id=$1 AND branch_id=$2 FOR UPDATE NOWAIT",
    )
    .bind(low)
    .bind(branch)
    .fetch_one(&mut *probe)
    .await
    .unwrap();
    probe.rollback().await.unwrap();
    held.rollback().await.unwrap();
    let result = timeout(WAIT, task).await.unwrap().unwrap().unwrap();
    assert_eq!(result.branch_memberships_removed, 1);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM user_branches WHERE user_id=$1")
        .bind(low)
        .fetch_one(&owner)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
#[sqlx::test(migrations = "../db/migrations")]
async fn roster_mutex_allows_child_fk_key_share(owner: PgPool) {
    let a = Uuid::from_u128(1);
    let b = Uuid::from_u128(2);
    roster_user(&owner, a, "010-2500-0001").await;
    roster_user(&owner, b, "010-2500-0002").await;
    let rt = runtime(&owner, "roster-key-share").await;
    let mut held = owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=ANY($1) FOR KEY SHARE")
        .bind(vec![a, b])
        .fetch_all(&mut *held)
        .await
        .unwrap();
    sqlx::query("SET lock_timeout='500ms'")
        .execute(&rt)
        .await
        .unwrap();
    let report = import(&rt, &["010-2500-0002", "010-2500-0001"])
        .await
        .unwrap();
    assert_eq!(report.users_updated, 2);
    held.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn bootstrap_consumption_rechecks_expiry_after_child_row_wait(owner: PgPool) {
    let user = user(&owner).await;
    let rt = runtime(&owner, "consume-last-wait").await;
    let issued = provision(&rt, user, 0).await.unwrap();
    let id = issued.credential_id;
    sqlx::query("UPDATE auth_bootstrap_credentials SET expires_at=clock_timestamp()+interval '2 seconds' WHERE id=$1").bind(id).execute(&owner).await.unwrap();
    let mut held = owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM auth_bootstrap_credentials WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let task = tokio::spawn(async move {
        let mut tx = rt.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org',$1,true)")
            .bind(OrgId::knl().as_uuid().to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        BootstrapCredentialStore
            .consume_open_credentials_tx(&mut tx, OrgId::knl(), user, OffsetDateTime::now_utc())
            .await
            .unwrap();
        tx.commit().await.unwrap();
    });
    blocked(&owner, "consume-last-wait", pid).await;
    timeout(WAIT, async {
        loop {
            let expired: bool = sqlx::query_scalar(
                "SELECT clock_timestamp()>=expires_at FROM auth_bootstrap_credentials WHERE id=$1",
            )
            .bind(id)
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
    timeout(WAIT, task).await.unwrap().unwrap();
    let state:(bool,i64)=sqlx::query_as("SELECT (SELECT consumed_at IS NULL FROM auth_bootstrap_credentials WHERE id=$1),(SELECT count(*) FROM audit_events WHERE actor=$2 AND action='auth.otp.consume')").bind(id).bind(user).fetch_one(&owner).await.unwrap();
    assert_eq!(state, (true, 0));
}
