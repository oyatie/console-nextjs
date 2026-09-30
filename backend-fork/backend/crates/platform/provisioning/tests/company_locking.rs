//! Real removal owners, row-lock upgrades and auth/FK concurrency in disposable databases.
//! This tests mechanical containment, not R4 authority or hard-deletion policy acceptance.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use console_kernel_core::{OrgId, UserId};
use console_platform_auth::{
    PasskeyRegistrationStart, PasskeyService, RefreshTokenStore, WebauthnSettings,
};
use console_platform_provisioning::{PlatformProvisioner, ProvisioningError, TenantRemovalOutcome};
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

async fn fixture(owner: &PgPool, force: bool) -> (Uuid, [Uuid; 2], Uuid) {
    let org:Uuid=sqlx::query_scalar("INSERT INTO organizations(slug,name,status) VALUES($1,'Removal lock test',$2) RETURNING id").bind(format!("lock-{}", &Uuid::new_v4().simple().to_string()[..16])).bind(if force{"ARCHIVED"}else{"ACTIVE"}).fetch_one(owner).await.unwrap();
    let mut users = [Uuid::new_v4(), Uuid::new_v4()];
    users.sort();
    for id in users {
        sqlx::query("INSERT INTO users(id,display_name,roles,org_id) VALUES($1,'Lock target',ARRAY['MEMBER'],$2)").bind(id).bind(org).execute(owner).await.unwrap();
    }
    let actor:Uuid=sqlx::query_scalar("INSERT INTO users(display_name,roles,org_id) VALUES('Separate actor',ARRAY['SUPER_ADMIN'],$1) RETURNING id").bind(*OrgId::platform().as_uuid()).fetch_one(owner).await.unwrap();
    (org, users, actor)
}
async fn remover(owner: &PgPool, force: bool) -> PgPool {
    if !force {
        return runtime(owner, "company-removal").await;
    }
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_platform_force_cmd")
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(
            owner
                .connect_options()
                .as_ref()
                .clone()
                .application_name("company-removal"),
        )
        .await
        .unwrap();
    let row: (String, bool, bool) = sqlx::query_as(
        "SELECT current_user::text,rolsuper,rolbypassrls FROM pg_roles WHERE rolname=current_user",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, ("console_platform_force_cmd".into(), false, false));
    pool
}
async fn remove(
    pool: &PgPool,
    force: bool,
    org: Uuid,
    actor: Uuid,
) -> Result<TenantRemovalOutcome, ProvisioningError> {
    let service = PlatformProvisioner::new(Duration::minutes(5));
    if force {
        service
            .force_remove_tenant(
                pool,
                Some(UserId::from_uuid(actor)),
                org,
                OffsetDateTime::now_utc(),
            )
            .await
    } else {
        service
            .remove_tenant(
                pool,
                Some(UserId::from_uuid(actor)),
                org,
                OffsetDateTime::now_utc(),
            )
            .await
    }
}
async fn opaque_key(owner: &PgPool, org: Uuid, user: Uuid) -> Uuid {
    // Unverified legacy row used only to block a DELETE, never to authenticate.
    sqlx::query_scalar("INSERT INTO auth_webauthn_credentials(user_id,credential_id,passkey_json,org_id) VALUES($1,$2,'{}',$3) RETURNING id").bind(user).bind(Uuid::new_v4().to_string()).bind(org).fetch_one(owner).await.unwrap()
}
async fn intact(owner: &PgPool, org: Uuid, users: [Uuid; 2]) {
    let state:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM organizations WHERE id=$1),(SELECT count(*) FROM users WHERE id=ANY($2)),(SELECT count(*) FROM audit_events WHERE target_id=$3 AND action IN ('platform.tenant.remove','platform.tenant.force_remove'))").bind(org).bind(users.to_vec()).bind(org.to_string()).fetch_one(owner).await.unwrap();
    assert_eq!(state, (1, 2, 0));
}
fn service() -> PasskeyService {
    PasskeyService::new(WebauthnSettings {
        rp_id: "example.com".into(),
        rp_origin: Url::parse("https://auth.example.com").unwrap(),
        rp_name: "Console".into(),
        extra_allowed_origins: vec![],
        ceremony_ttl: Duration::minutes(5),
    })
    .unwrap()
}
#[sqlx::test(migrations = "../db/migrations")]
async fn both_removal_owners_wait_for_sorted_accounts_before_children(owner: PgPool) {
    for force in [false, true] {
        let (org, users, actor) = fixture(&owner, force).await;
        let key = opaque_key(&owner, org, users[1]).await;
        let pool = remover(&owner, force).await;
        let (held, pid) = account(&owner, users[0]).await;
        let task = tokio::spawn(async move { remove(&pool, force, org, actor).await });
        blocked(&owner, "company-removal", pid).await;
        let mut probe = owner.begin().await.unwrap();
        sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE NOWAIT")
            .bind(users[1])
            .fetch_one(&mut *probe)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM auth_webauthn_credentials WHERE id=$1 FOR UPDATE NOWAIT")
            .bind(key)
            .fetch_one(&mut *probe)
            .await
            .unwrap();
        probe.rollback().await.unwrap();
        held.rollback().await.unwrap();
        assert!(matches!(
            timeout(WAIT, task).await.unwrap().unwrap().unwrap(),
            TenantRemovalOutcome::Removed
        ));
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn both_removal_owners_do_not_lock_company_before_authentication_finishes(owner: PgPool) {
    for force in [false, true] {
        let (org, users, actor) = fixture(&owner, force).await;
        let rt = runtime(&owner, "company-auth-first").await;
        let svc = service();
        let start = svc
            .start_registration(
                &rt,
                OrgId::from_uuid(org),
                PasskeyRegistrationStart {
                    user_id: users[0],
                    username: "Lock auth".into(),
                    display_name: "Lock auth".into(),
                },
            )
            .await
            .unwrap();
        let mut client = WebauthnAuthenticator::new(SoftPasskey::new(true));
        let signed = client
            .do_registration(
                Url::parse("https://auth.example.com").unwrap(),
                start.challenge,
            )
            .unwrap();
        let mut tx = rt.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org',$1,true)")
            .bind(org.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE")
            .bind(users[0])
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let pool = remover(&owner, force).await;
        let task = tokio::spawn(async move { remove(&pool, force, org, actor).await });
        blocked(&owner, "company-removal", pid).await;
        // Actual registration inserts key + audit, including Company FK checks,
        // while the waiting remover must not hold an incompatible Company lock.
        timeout(
            WAIT,
            svc.finish_registration_in_tx(
                &mut tx,
                OrgId::from_uuid(org),
                start.ceremony_id,
                signed,
                OffsetDateTime::now_utc(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        tx.commit().await.unwrap();
        assert!(matches!(
            timeout(WAIT, task).await.unwrap().unwrap().unwrap(),
            TenantRemovalOutcome::Removed
        ));
    }
}
async fn key_share_conflict(owner: &PgPool, company: bool) {
    for force in [false, true] {
        let (org, users, actor) = fixture(owner, force).await;
        let key = opaque_key(owner, org, users[0]).await;
        let mut held = owner.begin().await.unwrap();
        if company {
            sqlx::query("SELECT id FROM organizations WHERE id=$1 FOR KEY SHARE")
                .bind(org)
                .fetch_one(&mut *held)
                .await
                .unwrap();
        } else {
            sqlx::query("SELECT id FROM users WHERE id=$1 FOR KEY SHARE")
                .bind(users[0])
                .fetch_one(&mut *held)
                .await
                .unwrap();
        }
        let pool = remover(owner, force).await;
        // No lock_timeout: only the owner's explicit NOWAIT should end this.
        let err = timeout(
            std::time::Duration::from_secs(2),
            remove(&pool, force, org, actor),
        )
        .await
        .expect("exclusive upgrade must not wait for FK holders")
        .unwrap_err();
        assert!(
            matches!(err,ProvisioningError::Sqlx(ref error) if error.as_database_error().and_then(|e|e.code()).as_deref()==Some("55P03"))
        );
        intact(owner, org, users).await;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM auth_webauthn_credentials WHERE id=$1)",
        )
        .bind(key)
        .fetch_one(owner)
        .await
        .unwrap();
        assert!(exists);
        held.rollback().await.unwrap();
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn account_fk_holder_causes_removal_upgrade_conflict_without_partial_effect(owner: PgPool) {
    key_share_conflict(&owner, false).await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn company_fk_holder_causes_removal_upgrade_conflict_without_partial_effect(owner: PgPool) {
    key_share_conflict(&owner, true).await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn new_account_during_removal_wait_is_never_adopted_or_deleted(owner: PgPool) {
    for force in [false, true] {
        let (org, users, actor) = fixture(&owner, force).await;
        let pool = remover(&owner, force).await;
        let (held, pid) = account(&owner, users[0]).await;
        let task = tokio::spawn(async move { remove(&pool, force, org, actor).await });
        blocked(&owner, "company-removal", pid).await;
        let new_user:Uuid=sqlx::query_scalar("INSERT INTO users(display_name,roles,org_id) VALUES('Intervening',ARRAY['MEMBER'],$1) RETURNING id").bind(org).fetch_one(&owner).await.unwrap();
        held.rollback().await.unwrap();
        let err = timeout(WAIT, task).await.unwrap().unwrap().unwrap_err();
        assert!(
            matches!(err,ProvisioningError::Sqlx(ref error) if error.as_database_error().and_then(|e|e.code()).as_deref()==Some("40001"))
        );
        intact(&owner, org, users).await;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1)")
            .bind(new_user)
            .fetch_one(&owner)
            .await
            .unwrap();
        assert!(exists);
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn removal_guard_blocks_auth_and_new_account_until_delete_commits(owner: PgPool) {
    for force in [false, true] {
        let (org, users, actor) = fixture(&owner, force).await;
        let key = opaque_key(&owner, org, users[0]).await;
        let mut held = owner.begin().await.unwrap();
        sqlx::query("SELECT id FROM auth_webauthn_credentials WHERE id=$1 FOR UPDATE")
            .bind(key)
            .fetch_one(&mut *held)
            .await
            .unwrap();
        let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *held)
            .await
            .unwrap();
        let pool = remover(&owner, force).await;
        let removal = tokio::spawn(async move { remove(&pool, force, org, actor).await });
        let remover_pid = blocked(&owner, "company-removal", pid).await;
        let auth = runtime(&owner, "company-auth-second").await;
        let issuing = tokio::spawn(async move {
            RefreshTokenStore
                .issue_family(
                    &auth,
                    users[0],
                    OrgId::from_uuid(org),
                    OffsetDateTime::now_utc(),
                    Duration::minutes(5),
                )
                .await
        });
        blocked(&owner, "company-auth-second", remover_pid).await;
        let adding = runtime(&owner, "company-new-second").await;
        let addition = tokio::spawn(async move {
            let mut tx = adding.begin().await.unwrap();
            sqlx::query("SELECT set_config('app.current_org',$1,true)")
                .bind(org.to_string())
                .execute(&mut *tx)
                .await
                .unwrap();
            let result = sqlx::query(
                "INSERT INTO users(display_name,roles,org_id) VALUES('Late',ARRAY['MEMBER'],$1)",
            )
            .bind(org)
            .execute(&mut *tx)
            .await;
            if result.is_ok() {
                tx.commit().await.unwrap();
            } else {
                tx.rollback().await.unwrap();
            }
            result
        });
        blocked(&owner, "company-new-second", remover_pid).await;
        held.rollback().await.unwrap();
        assert!(matches!(
            timeout(WAIT, removal).await.unwrap().unwrap().unwrap(),
            TenantRemovalOutcome::Removed
        ));
        assert!(timeout(WAIT, issuing).await.unwrap().unwrap().is_err());
        let err = timeout(WAIT, addition).await.unwrap().unwrap().unwrap_err();
        assert_eq!(
            err.as_database_error().unwrap().code().as_deref(),
            Some("23503")
        );
        let state:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM users WHERE org_id=$1),(SELECT count(*) FROM auth_refresh_token_families WHERE org_id=$1)").bind(org).fetch_one(&owner).await.unwrap();
        assert_eq!(state, (0, 0));
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn both_removal_owners_lock_every_account_before_first_child_sweep(owner: PgPool) {
    for force in [false, true] {
        let (org, users, actor) = fixture(&owner, force).await;
        let key = opaque_key(&owner, org, users[0]).await;
        let pool = remover(&owner, force).await;
        let (held, pid) = account(&owner, users[1]).await;
        let task = tokio::spawn(async move { remove(&pool, force, org, actor).await });
        blocked(&owner, "company-removal", pid).await;
        let mut probe = owner.begin().await.unwrap();
        sqlx::query("SELECT id FROM auth_webauthn_credentials WHERE id=$1 FOR UPDATE NOWAIT")
            .bind(key)
            .fetch_one(&mut *probe)
            .await
            .unwrap();
        probe.rollback().await.unwrap();
        held.rollback().await.unwrap();
        assert!(matches!(
            timeout(WAIT, task).await.unwrap().unwrap().unwrap(),
            TenantRemovalOutcome::Removed
        ));
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn company_status_change_during_account_wait_aborts_both_removal_paths(owner: PgPool) {
    for force in [false, true] {
        let (org, users, actor) = fixture(&owner, force).await;
        let key = opaque_key(&owner, org, users[0]).await;
        let pool = remover(&owner, force).await;
        let (held, pid) = account(&owner, users[0]).await;
        let task = tokio::spawn(async move { remove(&pool, force, org, actor).await });
        blocked(&owner, "company-removal", pid).await;
        let status = if force { "ACTIVE" } else { "SUSPENDED" };
        sqlx::query("UPDATE organizations SET status=$2 WHERE id=$1")
            .bind(org)
            .bind(status)
            .execute(&owner)
            .await
            .unwrap();
        held.rollback().await.unwrap();
        let err = timeout(WAIT, task).await.unwrap().unwrap().unwrap_err();
        assert!(
            matches!(err,ProvisioningError::Sqlx(ref error) if error.as_database_error().and_then(|e|e.code()).as_deref()==Some("40001"))
        );
        intact(&owner, org, users).await;
        let remains: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM auth_webauthn_credentials WHERE id=$1)",
        )
        .bind(key)
        .fetch_one(&owner)
        .await
        .unwrap();
        assert!(remains);
        let current: String = sqlx::query_scalar("SELECT status FROM organizations WHERE id=$1")
            .bind(org)
            .fetch_one(&owner)
            .await
            .unwrap();
        assert_eq!(current, status);
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn removal_internal_functions_cannot_bypass_receipt_command(owner: PgPool) {
    for role in [
        "SET LOCAL ROLE console_platform_force_cmd",
        "SET LOCAL ROLE console_rt",
    ] {
        for function in [
            "SELECT platform_force_remove_organization($1)",
            "SELECT platform_lock_organization_accounts_for_removal($1)",
            "SELECT platform_force_remove_direct_org_children($1)",
        ] {
            let mut tx = owner.begin().await.unwrap();
            sqlx::query(role).execute(&mut *tx).await.unwrap();
            let result = sqlx::query(function)
                .bind(*OrgId::platform().as_uuid())
                .execute(&mut *tx)
                .await;
            tx.rollback().await.unwrap();
            let error =
                result.expect_err("internal removal owners must not be directly executable");
            assert_eq!(
                error.as_database_error().and_then(|e| e.code()).as_deref(),
                Some("42501"),
                "{role}: {function}"
            );
        }
    }
    let permissions: (bool, bool) = sqlx::query_as("SELECT has_function_privilege('console_platform_force_cmd','platform_force_remove_organization_command(uuid,uuid,character,character,timestamp with time zone)','EXECUTE'),has_function_privilege('console_rt','platform_force_remove_organization_command(uuid,uuid,character,character,timestamp with time zone)','EXECUTE')")
        .fetch_one(&owner).await.unwrap();
    assert_eq!(permissions, (true, false));
}
