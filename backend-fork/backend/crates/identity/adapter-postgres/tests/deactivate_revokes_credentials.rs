#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Offboarding closure: `deactivate_user` must revoke EVERY credential + session,
//! not merely flip `is_active`.
//!
//! A deactivated user who keeps an enrolled passkey or a live refresh-token
//! family is still a hole: the passkey authenticates and the family rotates until
//! natural expiry. This test runs the REAL `PgOrgStore::deactivate_user` as the
//! genuine non-owner `console_rt` role (FORCE RLS applies, exactly like prod) and
//! proves that after deactivation:
//!   * the user's WebAuthn credential rows are GONE (passkeys can't authenticate),
//!   * every refresh-token family + token is revoked (refresh fails closed,
//!     verified by a real `RefreshTokenStore::rotate` that now returns
//!     `FamilyRevoked`), and
//!   * each sub-action is audited.

use console_identity_adapter_postgres::PgOrgStore;
use console_identity_application::DeactivateUserCommand;
use console_kernel_core::{OrgId, TraceContext, UserId};
use console_platform_auth::{RefreshTokenStore, RefreshTokenUseError};
use console_platform_request_context::CURRENT_ORG;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

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
async fn seed_org_and_user(owner_pool: &PgPool, org: Uuid) -> Uuid {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind("org-knl")
    .bind("Org KNL")
    .execute(&mut *tx)
    .await
    .unwrap();
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id, is_active) VALUES ($1, $2, $3, true) RETURNING id",
    )
    .bind("Offboard User")
    .bind(vec!["MECHANIC".to_string()])
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    user_id
}

/// Insert a WebAuthn credential row for the user (owner pool, row_security off).
/// The `passkey_json` payload is opaque to deactivation — only the row's presence
/// matters for the revoke assertion.
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

async fn count_credentials_as_runtime(rt_pool: &PgPool, org: OrgId, user_id: Uuid) -> i64 {
    let mut tx = rt_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
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

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn deactivate_revokes_passkeys_and_sessions_as_runtime_role(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let knl = OrgId::knl();
    let user_id = seed_org_and_user(&owner_pool, *knl.as_uuid()).await;
    // The actor must be a real user (audit_events.actor FKs to users).
    let actor_id = seed_org_and_user(&owner_pool, *knl.as_uuid()).await;

    // The user has an enrolled passkey ...
    seed_credential(&owner_pool, *knl.as_uuid(), user_id, "cred-offboard-1").await;
    assert_eq!(
        count_credentials_as_runtime(&rt_pool, knl, user_id).await,
        1
    );

    // ... and a live refresh-token family (their session), minted as console_rt.
    let now = OffsetDateTime::now_utc();
    let family = RefreshTokenStore
        .issue_family(&rt_pool, user_id, knl, now, Duration::days(30))
        .await
        .expect("issue_family must pass RLS as console_rt");

    // Deactivate via the REAL adapter, with the org task-local armed exactly as
    // the request-context middleware arms it on the authenticated route.
    let store = PgOrgStore::new(rt_pool.clone());
    let summary = CURRENT_ORG
        .scope(
            knl,
            store.deactivate_user(DeactivateUserCommand {
                actor: UserId::from_uuid(actor_id),
                user_id: UserId::from_uuid(user_id),
                trace: TraceContext::generate(),
                occurred_at: now,
            }),
        )
        .await
        .expect("deactivate_user must succeed as console_rt");
    assert!(!summary.is_active, "the user is soft-deactivated");

    // 1) Every passkey is gone: a deactivated user can no longer authenticate.
    assert_eq!(
        count_credentials_as_runtime(&rt_pool, knl, user_id).await,
        0,
        "deactivation must DELETE all of the user's passkeys"
    );

    let removed: Vec<String> = sqlx::query_scalar(
        "SELECT credential_id FROM auth_security.credential_removals WHERE account_id=$1",
    )
    .bind(user_id)
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(removed, vec!["cred-offboard-1".to_string()]);

    // 2) The refresh family is dead: rotating the issued token now fails closed
    //    with FamilyRevoked (a real rotation, not a DB peek).
    let rotate = RefreshTokenStore
        .rotate(
            &rt_pool,
            family.token.as_str(),
            now + Duration::minutes(1),
            Duration::days(30),
            Duration::days(30),
        )
        .await
        .expect_err("a deactivated user's refresh token must not rotate");
    assert_eq!(rotate, RefreshTokenUseError::FamilyRevoked);

    // 3) Each sub-action is audited (deactivate + passkey revoke + session revoke).
    assert_eq!(
        audit_count(&owner_pool, "user.deactivate", user_id).await,
        1
    );
    assert_eq!(
        audit_count(&owner_pool, "auth.passkey.revoke_all", user_id).await,
        1
    );
    assert_eq!(
        audit_count(&owner_pool, "auth.refresh.revoke_all", user_id).await,
        1
    );
}
