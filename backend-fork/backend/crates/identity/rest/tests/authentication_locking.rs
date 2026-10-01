//! Both real passkey-management routers and bound handoff HTTP transactions.
//! Runtime pools enforce RLS; soft passkeys sign actual challenges. No browser claim.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use console_identity_adapter_postgres::PgOrgStore;
use console_identity_rest::IdentityRestState;
use console_kernel_core::{OrgId, UserId};
use console_platform_auth::{
    AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier, PasskeyRegistrationStart,
    PasskeyService, WebauthnSettings,
};
use console_platform_auth_rest::{
    AuthRestConfig, AuthRestState, DEVICE_LOGIN_APPROVE_PATH, DEVICE_LOGIN_APPROVE_SESSION_PATH,
    DEVICE_LOGIN_POLL_PATH, DEVICE_LOGIN_START_PATH, PASSKEY_ENROLL_HANDOFF_PATH,
};
use p256::{
    ecdsa::SigningKey,
    elliptic_curve::rand_core::OsRng,
    pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding},
};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use time::{Duration, OffsetDateTime};
use tokio::time::{sleep, timeout};
use tower::ServiceExt;
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

struct Keys {
    private: String,
    public: String,
}
impl Keys {
    fn new() -> Self {
        let key = SigningKey::random(&mut OsRng);
        Self {
            private: key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
            public: key
                .verifying_key()
                .to_public_key_pem(LineEnding::LF)
                .unwrap(),
        }
    }
    fn settings() -> JwtSettings {
        JwtSettings {
            issuer: "console-platform-auth".into(),
            audience: "console-api".into(),
            access_token_ttl: Duration::minutes(15),
        }
    }
    fn app(&self, pool: PgPool, identity: bool) -> Router {
        if identity {
            let verifier =
                JwtVerifier::from_es256_public_pem(Self::settings(), self.public.as_bytes())
                    .unwrap();
            console_identity_rest::router(IdentityRestState::new(
                PgOrgStore::new(pool),
                Some(verifier),
            ))
        } else {
            console_platform_auth_rest::router(
                AuthRestState::new(
                    pool,
                    AuthRestConfig {
                        rp_id: "example.com".into(),
                        rp_origin: "https://auth.example.com".into(),
                        rp_name: "Console".into(),
                        ceremony_ttl: Duration::minutes(5),
                        jwt_issuer: "console-platform-auth".into(),
                        jwt_audience: "console-api".into(),
                        jwt_private_key_pem: self.private.clone(),
                        jwt_public_key_pem: self.public.clone(),
                        refresh_token_ttl: Duration::days(1),
                        refresh_family_absolute_ttl: Duration::days(1),
                        cookie_secure: false,
                    },
                )
                .unwrap(),
            )
        }
    }
    async fn token(&self, rt: &PgPool, user: Uuid) -> String {
        let issuer = JwtIssuer::from_es256_pem(
            Self::settings(),
            self.private.as_bytes(),
            self.public.as_bytes(),
        )
        .unwrap();
        console_platform_test_support::issue_session_token(
            rt,
            &issuer,
            AccessTokenInput {
                subject: UserId::from_uuid(user),
                org_id: OrgId::knl(),
                roles: vec!["MEMBER".into()],
                branches: vec![],
                platform: false,
                view_as: false,
                read_only: false,
                display_name: None,
                feature_grants: vec![],
                authz_subject_version: 0,
                authz_policy_version: 0,
                session_generation: 0,
                issued_at: OffsetDateTime::now_utc(),
            },
            None,
            vec![],
        )
        .await
    }
}
async fn request(
    app: Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let req = if let Some(body) = body {
        req.header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    } else {
        req.body(Body::empty()).unwrap()
    };
    let response = timeout(WAIT, app.oneshot(req))
        .await
        .expect("request must finish, including with one pool connection")
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    if method == "DELETE" && !status.is_success() {
        eprintln!(
            "{method} {path}: {status}, code={:?}, message={:?}",
            body.get("code"),
            body.get("message")
        );
    }
    (status, body)
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
async fn passkey(owner: &PgPool, user: Uuid) -> (Uuid, String, WebauthnAuthenticator<SoftPasskey>) {
    let svc = service();
    let start = svc
        .start_registration(
            owner,
            OrgId::knl(),
            PasskeyRegistrationStart {
                user_id: user,
                username: "Lock test".into(),
                display_name: "Lock test".into(),
            },
        )
        .await
        .unwrap();
    let mut client = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let cred = client
        .do_registration(
            Url::parse("https://auth.example.com").unwrap(),
            start.challenge,
        )
        .unwrap();
    let stored = svc
        .finish_registration(owner, OrgId::knl(), start.ceremony_id, cred)
        .await
        .unwrap();
    let id = sqlx::query_scalar("SELECT id FROM auth_webauthn_credentials WHERE credential_id=$1")
        .bind(&stored.credential_id)
        .fetch_one(owner)
        .await
        .unwrap();
    (id, stored.credential_id, client)
}
async fn assertion(
    owner: &PgPool,
    id: &str,
    client: &mut WebauthnAuthenticator<SoftPasskey>,
) -> Value {
    let start = service().start_authentication(owner).await.unwrap();
    let mut challenge = serde_json::to_value(start.challenge).unwrap();
    challenge["publicKey"]["allowCredentials"] = json!([{"type":"public-key","id":id}]);
    let signed = client
        .do_authentication(
            Url::parse("https://auth.example.com").unwrap(),
            serde_json::from_value(challenge).unwrap(),
        )
        .unwrap();
    json!({"ceremony_id":start.ceremony_id,"credential":signed})
}
fn delete_path(identity: bool, id: Uuid) -> String {
    format!(
        "/api/v1/{}passkeys/{id}",
        if identity { "" } else { "auth/" }
    )
}
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn both_delete_aliases_serialize_before_keys_and_preserve_last_key(owner: PgPool) {
    for (first_alias, second_alias) in [(false, false), (true, true), (false, true)] {
        let u = user(&owner).await;
        add_branch_membership(&owner, u).await;
        let keys = Keys::new();
        let rt1 = runtime(&owner, "delete-one").await;
        let rt2 = runtime(&owner, "delete-two").await;
        let token = keys.token(&rt1, u).await;
        let (key1, _, _) = passkey(&owner, u).await;
        let (key2, _, _) = passkey(&owner, u).await;
        let app1 = keys.app(rt1, first_alias);
        let app2 = keys.app(rt2, second_alias);
        let mut held = owner.begin().await.unwrap();
        sqlx::query("SELECT id FROM auth_webauthn_credentials WHERE id=$1 FOR UPDATE")
            .bind(key1)
            .fetch_one(&mut *held)
            .await
            .unwrap();
        let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *held)
            .await
            .unwrap();
        let token1 = token.clone();
        let t1 = tokio::spawn(async move {
            request(
                app1,
                "DELETE",
                &delete_path(first_alias, key1),
                Some(&token1),
                None,
            )
            .await
        });
        let first_pid = blocked(&owner, "delete-one", pid).await;
        let t2 = tokio::spawn(async move {
            request(
                app2,
                "DELETE",
                &delete_path(second_alias, key2),
                Some(&token),
                None,
            )
            .await
        });
        blocked(&owner, "delete-two", first_pid).await;
        let mut probe = owner.begin().await.unwrap();
        sqlx::query("SELECT id FROM auth_webauthn_credentials WHERE id=$1 FOR UPDATE NOWAIT")
            .bind(key2)
            .fetch_one(&mut *probe)
            .await
            .unwrap();
        probe.rollback().await.unwrap();
        held.rollback().await.unwrap();
        assert_eq!(t1.await.unwrap().0, StatusCode::NO_CONTENT);
        assert_eq!(t2.await.unwrap().0, StatusCode::CONFLICT);
        let state:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM auth_webauthn_credentials WHERE user_id=$1),(SELECT count(*) FROM audit_events WHERE actor=$1 AND action='auth.passkey.revoke')").bind(u).fetch_one(&owner).await.unwrap();
        assert_eq!(state, (1, 1));
        let removed: Vec<Uuid> = sqlx::query_scalar("SELECT credential_row_id FROM auth_security.credential_removals WHERE account_id=$1 ORDER BY credential_row_id").bind(u).fetch_all(&owner).await.unwrap();
        assert_eq!(removed, vec![key1]);
    }
}
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn both_delete_aliases_recheck_active_account_after_wait(owner: PgPool) {
    for identity in [false, true] {
        let u = user(&owner).await;
        add_branch_membership(&owner, u).await;
        let keys = Keys::new();
        let rt = runtime(&owner, "delete-inactive").await;
        let token = keys.token(&rt, u).await;
        let (key, _, _) = passkey(&owner, u).await;
        passkey(&owner, u).await;
        let app = keys.app(rt, identity);
        let (mut held, pid) = account(&owner, u).await;
        let task = tokio::spawn(async move {
            request(
                app,
                "DELETE",
                &delete_path(identity, key),
                Some(&token),
                None,
            )
            .await
        });
        blocked(&owner, "delete-inactive", pid).await;
        sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
            .bind(u)
            .execute(&mut *held)
            .await
            .unwrap();
        held.commit().await.unwrap();
        assert_eq!(task.await.unwrap().0, StatusCode::UNAUTHORIZED);
        let state:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM auth_webauthn_credentials WHERE user_id=$1),(SELECT count(*) FROM audit_events WHERE actor=$1 AND action='auth.passkey.revoke')").bind(u).fetch_one(&owner).await.unwrap();
        assert_eq!(state, (2, 0));
    }
}
async fn start_handoff(owner: &PgPool, app: Router) -> (Uuid, Value, String) {
    let (status, result) = request(app, "POST", DEVICE_LOGIN_START_PATH, None, None).await;
    assert_eq!(status, StatusCode::OK);
    let url = Url::parse(result["approve_url"].as_str().unwrap()).unwrap();
    let approve = url::form_urlencoded::parse(url.fragment().unwrap().as_bytes())
        .find(|(key, _)| key == "desktop_approve")
        .unwrap()
        .1
        .into_owned();
    let id = sqlx::query_scalar(
        "SELECT id FROM auth_device_login_handoffs ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(owner)
    .await
    .unwrap();
    (id, result, approve)
}
async fn assert_account_locked(owner: &PgPool, u: Uuid) {
    let mut probe = owner.begin().await.unwrap();
    let err = sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE NOWAIT")
        .bind(u)
        .fetch_one(&mut *probe)
        .await
        .unwrap_err();
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("55P03")
    );
    probe.rollback().await.unwrap();
}
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn proof_and_session_handoff_approval_hold_account_before_handoff_row(owner: PgPool) {
    for session in [false, true] {
        let u = user(&owner).await;
        let keys = Keys::new();
        let rt = runtime(&owner, "handoff-approve").await;
        let token = keys.token(&rt, u).await;
        let (_, key, mut client) = passkey(&owner, u).await;
        let app = keys.app(rt, false);
        let (id, _, approve) = start_handoff(&owner, app.clone()).await;
        // Bind the standard handoff's target for the session alias; no approval is fabricated.
        sqlx::query(
            "UPDATE auth_device_login_handoffs SET target_user_id=$2,target_org_id=$3 WHERE id=$1",
        )
        .bind(id)
        .bind(u)
        .bind(*OrgId::knl().as_uuid())
        .execute(&owner)
        .await
        .unwrap();
        let mut body = if session {
            json!({})
        } else {
            assertion(&owner, &key, &mut client).await
        };
        body["approve_token"] = json!(approve);
        let mut held = owner.begin().await.unwrap();
        sqlx::query("SELECT id FROM auth_device_login_handoffs WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *held)
            .await
            .unwrap();
        let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *held)
            .await
            .unwrap();
        let path = if session {
            DEVICE_LOGIN_APPROVE_SESSION_PATH
        } else {
            DEVICE_LOGIN_APPROVE_PATH
        };
        let task =
            tokio::spawn(async move { request(app, "POST", path, Some(&token), Some(body)).await });
        blocked(&owner, "handoff-approve", pid).await;
        assert_account_locked(&owner, u).await;
        held.rollback().await.unwrap();
        assert_eq!(task.await.unwrap().0, StatusCode::NO_CONTENT);
        let approved: Option<Uuid> = sqlx::query_scalar(
            "SELECT approved_user_id FROM auth_device_login_handoffs WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&owner)
        .await
        .unwrap();
        assert_eq!(approved, Some(u));
    }
}
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn session_handoff_rechecks_revoked_family_after_account_wait(owner: PgPool) {
    let u = user(&owner).await;
    let keys = Keys::new();
    let rt = runtime(&owner, "approval-revoked").await;
    let token = keys.token(&rt, u).await;
    passkey(&owner, u).await;
    let app = keys.app(rt, false);
    let (id, _, approve) = start_handoff(&owner, app.clone()).await;
    sqlx::query(
        "UPDATE auth_device_login_handoffs SET target_user_id=$2,target_org_id=$3 WHERE id=$1",
    )
    .bind(id)
    .bind(u)
    .bind(*OrgId::knl().as_uuid())
    .execute(&owner)
    .await
    .unwrap();

    let (mut held, pid) = account(&owner, u).await;
    let task = tokio::spawn(async move {
        request(
            app,
            "POST",
            DEVICE_LOGIN_APPROVE_SESSION_PATH,
            Some(&token),
            Some(json!({"approve_token": approve})),
        )
        .await
    });
    blocked(&owner, "approval-revoked", pid).await;
    sqlx::query(
        "UPDATE auth_refresh_token_families SET revoked_at=clock_timestamp(),revoked_reason='logout' WHERE user_id=$1 AND revoked_at IS NULL",
    )
    .bind(u)
    .execute(&mut *held)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE auth_refresh_tokens SET revoked_at=clock_timestamp() WHERE user_id=$1 AND revoked_at IS NULL",
    )
    .bind(u)
    .execute(&mut *held)
    .await
    .unwrap();
    held.commit().await.unwrap();

    let (status, response) = task.await.unwrap();
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{response}");
    let state: (bool, i64) = sqlx::query_as(
        "SELECT (SELECT approved_at IS NULL AND consumed_at IS NULL FROM auth_device_login_handoffs WHERE id=$1), \
         (SELECT count(*) FROM audit_events WHERE actor=$2 AND action='auth.device_login.approve_session')",
    )
    .bind(id)
    .bind(u)
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(state, (true, 0));
}
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn bound_enrollment_creation_holds_account_through_insert(owner: PgPool) {
    let u = user(&owner).await;
    let keys = Keys::new();
    let rt = runtime(&owner, "handoff-enroll").await;
    let token = keys.token(&rt, u).await;
    let (_, key, mut client) = passkey(&owner, u).await;
    let proof = assertion(&owner, &key, &mut client).await;
    let app = keys.app(rt, false);
    let mut held = owner.begin().await.unwrap();
    sqlx::query("LOCK TABLE auth_device_login_handoffs IN SHARE MODE")
        .execute(&mut *held)
        .await
        .unwrap();
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let task = tokio::spawn(async move {
        request(
            app,
            "POST",
            PASSKEY_ENROLL_HANDOFF_PATH,
            Some(&token),
            Some(json!({"step_up":proof})),
        )
        .await
    });
    blocked(&owner, "handoff-enroll", pid).await;
    assert_account_locked(&owner, u).await;
    held.rollback().await.unwrap();
    assert_eq!(task.await.unwrap().0, StatusCode::OK);
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM auth_device_login_handoffs WHERE target_user_id=$1 AND approved_at IS NULL").bind(u).fetch_one(&owner).await.unwrap();
    assert_eq!(count, 1);
}
async fn approved_handoff(owner: &PgPool, app: Router, u: Uuid) -> (Uuid, Value) {
    let (_, key, mut client) = passkey(owner, u).await;
    let (id, result, approve) = start_handoff(owner, app.clone()).await;
    let mut proof = assertion(owner, &key, &mut client).await;
    proof["approve_token"] = json!(approve);
    assert_eq!(
        request(app, "POST", DEVICE_LOGIN_APPROVE_PATH, None, Some(proof))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    (id, result)
}
async fn poll_wait(owner: &PgPool, change: u8) {
    let u = user(owner).await;
    let b = user(owner).await;
    let keys = Keys::new();
    let rt = runtime(owner, "handoff-poll").await;
    let app = keys.app(rt, false);
    let (id, handoff) = approved_handoff(owner, app.clone(), u).await;
    if change == 1 {
        sqlx::query("UPDATE auth_device_login_handoffs SET expires_at=clock_timestamp()+interval '2 seconds' WHERE id=$1").bind(id).execute(owner).await.unwrap();
    }
    let (mut held, pid) = account(owner, u).await;
    let task = tokio::spawn(async move {
        request(
            app,
            "POST",
            DEVICE_LOGIN_POLL_PATH,
            None,
            Some(json!({"poll_token":handoff["poll_token"]})),
        )
        .await
    });
    blocked(owner, "handoff-poll", pid).await;
    let mut probe = owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM auth_device_login_handoffs WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(id)
        .fetch_one(&mut *probe)
        .await
        .unwrap();
    probe.rollback().await.unwrap();
    if change == 1 {
        timeout(WAIT,async{loop{let expired:bool=sqlx::query_scalar("SELECT clock_timestamp()>=expires_at FROM auth_device_login_handoffs WHERE id=$1").bind(id).fetch_one(owner).await.unwrap();if expired{break;}sleep(std::time::Duration::from_millis(5)).await;}}).await.unwrap();
    } else if change == 2 {
        sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
            .bind(u)
            .execute(&mut *held)
            .await
            .unwrap();
    } else if change == 3 {
        sqlx::query("UPDATE auth_device_login_handoffs SET approved_user_id=$2 WHERE id=$1")
            .bind(id)
            .bind(b)
            .execute(owner)
            .await
            .unwrap();
    }
    held.commit().await.unwrap();
    let (status, result) = task.await.unwrap();
    if change == 0 {
        assert_eq!(status, StatusCode::OK);
        assert_eq!(result["status"], "approved");
    } else if change == 1 {
        assert!(
            status == StatusCode::UNAUTHORIZED
                || (status == StatusCode::OK && result["status"] == "expired")
        );
    } else {
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let state:(bool,i64,i64)=sqlx::query_as("SELECT (SELECT consumed_at IS NOT NULL FROM auth_device_login_handoffs WHERE id=$1),(SELECT count(*) FROM auth_refresh_token_families WHERE user_id=ANY($2)),(SELECT count(*) FROM audit_events WHERE actor=ANY($2) AND action='auth.device_login.consume')").bind(id).bind(vec![u,b]).fetch_one(owner).await.unwrap();
    assert_eq!(
        state,
        if change == 0 {
            (true, 1, 1)
        } else {
            (false, 0, 0)
        }
    );
}
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn approved_handoff_poll_waits_before_consumption_and_issues_once(owner: PgPool) {
    poll_wait(&owner, 0).await;
}
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn approved_handoff_poll_expired_during_wait_does_not_consume(owner: PgPool) {
    poll_wait(&owner, 1).await;
}
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn approved_handoff_poll_inactive_during_wait_does_not_consume(owner: PgPool) {
    poll_wait(&owner, 2).await;
}
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn approved_handoff_poll_changed_account_is_not_adopted(owner: PgPool) {
    poll_wait(&owner, 3).await;
}

async fn approval_after_wait(owner: &PgPool, change: u8) {
    for session in [false, true] {
        let u = user(owner).await;
        let other = user(owner).await;
        let keys = Keys::new();
        let rt = runtime(owner, "handoff-approval-state").await;
        let token = keys.token(&rt, u).await;
        let (_, key, mut client) = passkey(owner, u).await;
        let app = keys.app(rt, false);
        let (id, _, approve) = start_handoff(owner, app.clone()).await;
        sqlx::query(
            "UPDATE auth_device_login_handoffs SET target_user_id=$2,target_org_id=$3 WHERE id=$1",
        )
        .bind(id)
        .bind(u)
        .bind(*OrgId::knl().as_uuid())
        .execute(owner)
        .await
        .unwrap();
        if change == 1 {
            sqlx::query("UPDATE auth_device_login_handoffs SET expires_at=clock_timestamp()+interval '2 seconds' WHERE id=$1").bind(id).execute(owner).await.unwrap();
        }
        let mut body = if session {
            json!({})
        } else {
            assertion(owner, &key, &mut client).await
        };
        body["approve_token"] = json!(approve);
        let (mut held, pid) = account(owner, u).await;
        let path = if session {
            DEVICE_LOGIN_APPROVE_SESSION_PATH
        } else {
            DEVICE_LOGIN_APPROVE_PATH
        };
        let task =
            tokio::spawn(async move { request(app, "POST", path, Some(&token), Some(body)).await });
        blocked(owner, "handoff-approval-state", pid).await;
        let mut probe = owner.begin().await.unwrap();
        sqlx::query("SELECT id FROM auth_device_login_handoffs WHERE id=$1 FOR UPDATE NOWAIT")
            .bind(id)
            .fetch_one(&mut *probe)
            .await
            .unwrap();
        probe.rollback().await.unwrap();
        if change == 1 {
            timeout(WAIT,async{loop{let expired:bool=sqlx::query_scalar("SELECT clock_timestamp()>=expires_at FROM auth_device_login_handoffs WHERE id=$1").bind(id).fetch_one(owner).await.unwrap();if expired{break;}sleep(std::time::Duration::from_millis(5)).await;}}).await.unwrap();
        } else if change == 2 {
            sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
                .bind(u)
                .execute(&mut *held)
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE auth_device_login_handoffs SET target_user_id=$2 WHERE id=$1")
                .bind(id)
                .bind(other)
                .execute(owner)
                .await
                .unwrap();
        }
        held.commit().await.unwrap();
        assert_eq!(task.await.unwrap().0, StatusCode::UNAUTHORIZED);
        let state:(bool,i64)=sqlx::query_as("SELECT (SELECT approved_at IS NULL AND consumed_at IS NULL FROM auth_device_login_handoffs WHERE id=$1),(SELECT count(*) FROM audit_events WHERE actor=$2 AND action IN ('auth.device_login.approve','auth.device_login.approve_session'))").bind(id).bind(u).fetch_one(owner).await.unwrap();
        assert_eq!(state, (true, 0));
    }
}
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn both_handoff_approval_paths_recheck_expiry_after_account_wait(owner: PgPool) {
    approval_after_wait(&owner, 1).await;
}
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn both_handoff_approval_paths_deny_deactivation_during_wait(owner: PgPool) {
    approval_after_wait(&owner, 2).await;
}
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn both_handoff_approval_paths_deny_changed_target_during_wait(owner: PgPool) {
    approval_after_wait(&owner, 3).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn approval_and_poll_recheck_expiry_after_last_handoff_row_wait(owner: PgPool) {
    for mode in 0..3 {
        let u = user(&owner).await;
        let keys = Keys::new();
        let rt = runtime(&owner, "handoff-last-wait").await;
        let token = keys.token(&rt, u).await;
        let (_, key, mut client) = passkey(&owner, u).await;
        let app = keys.app(rt, false);
        let (id, handoff, approve) = start_handoff(&owner, app.clone()).await;
        sqlx::query(
            "UPDATE auth_device_login_handoffs SET target_user_id=$2,target_org_id=$3 WHERE id=$1",
        )
        .bind(id)
        .bind(u)
        .bind(*OrgId::knl().as_uuid())
        .execute(&owner)
        .await
        .unwrap();
        let mut body = if mode == 1 {
            json!({})
        } else {
            assertion(&owner, &key, &mut client).await
        };
        body["approve_token"] = json!(approve);
        if mode == 2 {
            assert_eq!(
                request(
                    app.clone(),
                    "POST",
                    DEVICE_LOGIN_APPROVE_PATH,
                    None,
                    Some(body)
                )
                .await
                .0,
                StatusCode::NO_CONTENT
            );
            body = json!({"poll_token":handoff["poll_token"]});
        }
        sqlx::query("UPDATE auth_device_login_handoffs SET expires_at=clock_timestamp()+interval '2 seconds' WHERE id=$1").bind(id).execute(&owner).await.unwrap();
        let mut held = owner.begin().await.unwrap();
        sqlx::query("SELECT id FROM auth_device_login_handoffs WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *held)
            .await
            .unwrap();
        let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *held)
            .await
            .unwrap();
        let path = match mode {
            0 => DEVICE_LOGIN_APPROVE_PATH,
            1 => DEVICE_LOGIN_APPROVE_SESSION_PATH,
            _ => DEVICE_LOGIN_POLL_PATH,
        };
        let task =
            tokio::spawn(async move { request(app, "POST", path, Some(&token), Some(body)).await });
        blocked(&owner, "handoff-last-wait", pid).await;
        timeout(WAIT,async{loop{let expired:bool=sqlx::query_scalar("SELECT clock_timestamp()>=expires_at FROM auth_device_login_handoffs WHERE id=$1").bind(id).fetch_one(&owner).await.unwrap();if expired{break;}sleep(std::time::Duration::from_millis(5)).await;}}).await.unwrap();
        held.rollback().await.unwrap();
        let (status, result) = task.await.unwrap();
        assert!(
            status == StatusCode::UNAUTHORIZED
                || (mode == 2 && status == StatusCode::OK && result["status"] == "expired")
        );
        let state:(bool,bool,i64,i64)=sqlx::query_as("SELECT (SELECT approved_at IS NOT NULL FROM auth_device_login_handoffs WHERE id=$1),(SELECT consumed_at IS NOT NULL FROM auth_device_login_handoffs WHERE id=$1),(SELECT count(*) FROM auth_refresh_token_families WHERE user_id=$2),(SELECT count(*) FROM audit_events WHERE actor=$2 AND action IN ('auth.device_login.approve','auth.device_login.approve_session','auth.device_login.consume'))").bind(id).bind(u).fetch_one(&owner).await.unwrap();
        assert_eq!(state, (mode == 2, false, 1, if mode == 2 { 1 } else { 0 }));
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn session_handoff_does_not_adopt_target_changed_to_caller(owner: PgPool) {
    let caller = user(&owner).await;
    let other = user(&owner).await;
    let keys = Keys::new();
    let rt = runtime(&owner, "handoff-inverse-target").await;
    let token = keys.token(&rt, caller).await;
    passkey(&owner, caller).await;
    let app = keys.app(rt, false);
    let (id, _, approve) = start_handoff(&owner, app.clone()).await;
    sqlx::query(
        "UPDATE auth_device_login_handoffs SET target_user_id=$2,target_org_id=$3 WHERE id=$1",
    )
    .bind(id)
    .bind(other)
    .bind(*OrgId::knl().as_uuid())
    .execute(&owner)
    .await
    .unwrap();
    let (held, pid) = account(&owner, caller).await;
    let mut task = tokio::spawn(async move {
        request(
            app,
            "POST",
            DEVICE_LOGIN_APPROVE_SESSION_PATH,
            Some(&token),
            Some(json!({"approve_token":approve})),
        )
        .await
    });
    let (status, expected_target) = tokio::select! {
        response = &mut task => {
            held.rollback().await.unwrap();
            (response.unwrap().0, other)
        }
        _ = blocked(&owner, "handoff-inverse-target", pid) => {
            sqlx::query("UPDATE auth_device_login_handoffs SET target_user_id=$2 WHERE id=$1")
                .bind(id).bind(caller).execute(&owner).await.unwrap();
            held.rollback().await.unwrap();
            (timeout(WAIT, task).await.unwrap().unwrap().0, caller)
        }
    };
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let target: Uuid =
        sqlx::query_scalar("SELECT target_user_id FROM auth_device_login_handoffs WHERE id=$1")
            .bind(id)
            .fetch_one(&owner)
            .await
            .unwrap();
    assert_eq!(target, expected_target);
    let state: (bool, i64) = sqlx::query_as("SELECT (SELECT approved_at IS NULL AND consumed_at IS NULL FROM auth_device_login_handoffs WHERE id=$1),(SELECT count(*) FROM audit_events WHERE actor=$2 AND action='auth.device_login.approve_session')")
        .bind(id).bind(caller).fetch_one(&owner).await.unwrap();
    assert_eq!(state, (true, 0));
}

// The identity alias retains its existing branch-capability gate. Supply actual
// membership so these concurrency probes reach the owner without elevating roles.
async fn add_branch_membership(owner: &PgPool, user: Uuid) {
    let org = *OrgId::knl().as_uuid();
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions(name,org_id) VALUES($1,$2) RETURNING id")
            .bind(format!("Lock region {}", Uuid::new_v4()))
            .bind(org)
            .fetch_one(owner)
            .await
            .unwrap();
    let branch: Uuid = sqlx::query_scalar(
        "INSERT INTO branches(region_id,name,org_id) VALUES($1,$2,$3) RETURNING id",
    )
    .bind(region)
    .bind(format!("Lock branch {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_branches(user_id,branch_id,org_id) VALUES($1,$2,$3)")
        .bind(user)
        .bind(branch)
        .bind(org)
        .execute(owner)
        .await
        .unwrap();
}

// The source key must still exist when an approved QR handoff is consumed.
struct SourceHandoff {
    app: Router,
    user: Uuid,
    key: Uuid,
    survivor: Uuid,
    survivor_credential: String,
    survivor_client: WebauthnAuthenticator<SoftPasskey>,
    id: Uuid,
    poll_body: Value,
}
impl SourceHandoff {
    async fn new(owner: &PgPool, session: bool, label: &str) -> Self {
        let u = user(owner).await;
        add_branch_membership(owner, u).await;
        let keys = Keys::new();
        let rt = runtime(owner, label).await;
        let token = keys.token(&rt, u).await;
        // A is deliberately newer than B so session approval really records A.
        let (survivor, survivor_credential, survivor_client) = passkey(owner, u).await;
        let (key, credential, mut client) = passkey(owner, u).await;
        let app = keys.app(rt, false);
        let (id, handoff, approve) = start_handoff(owner, app.clone()).await;
        let mut body = if session {
            sqlx::query("UPDATE auth_device_login_handoffs SET target_user_id=$2,target_org_id=$3 WHERE id=$1")
                .bind(id).bind(u).bind(*OrgId::knl().as_uuid()).execute(owner).await.unwrap();
            json!({})
        } else {
            assertion(owner, &credential, &mut client).await
        };
        body["approve_token"] = json!(approve);
        let path = if session {
            DEVICE_LOGIN_APPROVE_SESSION_PATH
        } else {
            DEVICE_LOGIN_APPROVE_PATH
        };
        assert_eq!(
            request(app.clone(), "POST", path, Some(&token), Some(body))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        let recorded: (Option<Uuid>, Option<Uuid>, Option<Uuid>) = sqlx::query_as(
            "SELECT approved_passkey_id,approved_user_id,approved_org_id FROM auth_device_login_handoffs WHERE id=$1")
            .bind(id).fetch_one(owner).await.unwrap();
        assert_eq!(
            recorded,
            (Some(key), Some(u), Some(*OrgId::knl().as_uuid()))
        );
        Self {
            app,
            user: u,
            key,
            survivor,
            survivor_credential,
            survivor_client,
            id,
            poll_body: json!({"poll_token":handoff["poll_token"]}),
        }
    }
    async fn poll(&self) -> (StatusCode, Value) {
        request(
            self.app.clone(),
            "POST",
            DEVICE_LOGIN_POLL_PATH,
            None,
            Some(self.poll_body.clone()),
        )
        .await
    }
    async fn state(&self, owner: &PgPool) -> (bool, i64, i64, i64) {
        sqlx::query_as("SELECT (SELECT consumed_at IS NOT NULL FROM auth_device_login_handoffs WHERE id=$1),(SELECT count(*) FROM auth_refresh_token_families WHERE user_id=$2),(SELECT count(*) FROM auth_refresh_tokens WHERE user_id=$2),(SELECT count(*) FROM audit_events WHERE actor=$2 AND action='auth.device_login.consume')")
            .bind(self.id).bind(self.user).fetch_one(owner).await.unwrap()
    }
    async fn assert_denied(&self, owner: &PgPool, before: (bool, i64, i64, i64)) {
        for _ in 0..2 {
            assert_eq!(
                self.poll().await.0,
                StatusCode::UNAUTHORIZED,
                "recorded source key is no longer authorized"
            );
            assert_eq!(
                self.state(owner).await,
                before,
                "denied poll must not consume, issue or audit a session"
            );
        }
    }
    async fn survivor_login(&mut self, owner: &PgPool) {
        let body = assertion(owner, &self.survivor_credential, &mut self.survivor_client).await;
        let (status, result) = request(
            self.app.clone(),
            "POST",
            console_platform_auth_rest::PASSKEY_LOGIN_FINISH_PATH,
            None,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            result["access_token"]
                .as_str()
                .is_some_and(|token| !token.is_empty())
        );
    }
}
async fn removed_source(owner: &PgPool, session: bool, identity: bool) {
    let mut handoff = SourceHandoff::new(owner, session, "source-remove").await;
    // Both real aliases run the same audited removal owner under runtime RLS.
    // The family-bound helper token is already included in the issuance baseline.
    let keys = Keys::new();
    let rt = runtime(owner, "source-delete").await;
    let token = keys.token(&rt, handoff.user).await;
    assert_eq!(
        request(
            keys.app(rt, identity),
            "DELETE",
            &delete_path(identity, handoff.key),
            Some(&token),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let remaining: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM auth_webauthn_credentials WHERE user_id=$1")
            .bind(handoff.user)
            .fetch_all(owner)
            .await
            .unwrap();
    assert_eq!(remaining, vec![handoff.survivor]);
    let receipt: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_security.credential_removals WHERE account_id=$1 AND credential_row_id=$2")
        .bind(handoff.user).bind(handoff.key).fetch_one(owner).await.unwrap();
    assert_eq!(receipt, 1);
    let before = handoff.state(owner).await;
    handoff.assert_denied(owner, before).await;
    handoff.survivor_login(owner).await;
}
async fn source_removed_while_waiting(owner: &PgPool, session: bool) {
    let handoff = SourceHandoff::new(owner, session, "source-poll-wait").await;
    let before = handoff.state(owner).await;
    let (mut held, controller) = account(owner, handoff.user).await;
    let app = handoff.app.clone();
    let body = handoff.poll_body.clone();
    let task = tokio::spawn(async move {
        request(app, "POST", DEVICE_LOGIN_POLL_PATH, None, Some(body)).await
    });
    let pid = blocked(owner, "source-poll-wait", controller).await;
    let query: String = sqlx::query_scalar("SELECT query FROM pg_stat_activity WHERE pid=$1")
        .bind(pid)
        .fetch_one(owner)
        .await
        .unwrap();
    assert_eq!(
        query,
        "SELECT is_active FROM users WHERE id = $1 AND org_id = $2 FOR NO KEY UPDATE"
    );
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(OrgId::knl().to_string())
        .execute(&mut *held)
        .await
        .unwrap();
    let audit = console_platform_auth::delete_self_passkey_tx(
        &mut held,
        OrgId::knl(),
        handoff.user,
        handoff.key,
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    console_platform_db::insert_audit_event(&mut held, &audit)
        .await
        .unwrap();
    held.commit().await.unwrap();
    assert_eq!(
        task.await.unwrap().0,
        StatusCode::UNAUTHORIZED,
        "revocation won the actual Account lock"
    );
    assert_eq!(handoff.state(owner).await, before);
    handoff.assert_denied(owner, before).await;
    let remaining: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM auth_webauthn_credentials WHERE user_id=$1")
            .bind(handoff.user)
            .fetch_all(owner)
            .await
            .unwrap();
    assert_eq!(remaining, vec![handoff.survivor]);
}
async fn foreign_source(owner: &PgPool, company: bool) -> Uuid {
    let org = if company {
        OrgId::platform()
    } else {
        OrgId::knl()
    };
    let u: Uuid = sqlx::query_scalar("INSERT INTO users(display_name,org_id,roles) VALUES('Foreign source fixture',$1,ARRAY['MEMBER']) RETURNING id")
        .bind(*org.as_uuid()).fetch_one(owner).await.unwrap();
    let svc = service();
    let start = svc
        .start_registration(
            owner,
            org,
            PasskeyRegistrationStart {
                user_id: u,
                username: u.to_string(),
                display_name: "Foreign source fixture".into(),
            },
        )
        .await
        .unwrap();
    let mut client = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = client
        .do_registration(
            Url::parse("https://auth.example.com").unwrap(),
            start.challenge,
        )
        .unwrap();
    let stored = svc
        .finish_registration(owner, org, start.ceremony_id, credential)
        .await
        .unwrap();
    let actual: (Uuid, Uuid) =
        sqlx::query_as("SELECT user_id,org_id FROM auth_webauthn_credentials WHERE id=$1")
            .bind(stored.id)
            .fetch_one(owner)
            .await
            .unwrap();
    assert_eq!(actual, (u, *org.as_uuid()));
    stored.id
}
async fn corrupt_source(owner: &PgPool, session: bool, foreign: Option<bool>) {
    let handoff = SourceHandoff::new(owner, session, "source-corrupt").await;
    let source = match foreign {
        Some(company) => Some(foreign_source(owner, company).await),
        None => None,
    };
    // Hostile owner fixture only: neither public approval producer records this source.
    sqlx::query("UPDATE auth_device_login_handoffs SET approved_passkey_id=$2 WHERE id=$1")
        .bind(handoff.id)
        .bind(source)
        .execute(owner)
        .await
        .unwrap();
    let before = handoff.state(owner).await;
    handoff.assert_denied(owner, before).await;
}
async fn retained_source(owner: &PgPool, session: bool) {
    let handoff = SourceHandoff::new(owner, session, "source-retained").await;
    let (newer, _, _) = passkey(owner, handoff.user).await;
    let latest: Uuid = sqlx::query_scalar("SELECT id FROM auth_webauthn_credentials WHERE user_id=$1 ORDER BY created_at DESC,id DESC LIMIT 1")
        .bind(handoff.user).fetch_one(owner).await.unwrap();
    assert_eq!(latest, newer);
    assert_ne!(latest, handoff.key);
    let before = handoff.state(owner).await;
    let (status, result) = handoff.poll().await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["status"], "approved");
    assert!(
        result["access_token"]
            .as_str()
            .is_some_and(|token| !token.is_empty())
    );
    let after = (true, before.1 + 1, before.2 + 1, before.3 + 1);
    assert_eq!(handoff.state(owner).await, after);
    let recorded: Option<Uuid> = sqlx::query_scalar(
        "SELECT approved_passkey_id FROM auth_device_login_handoffs WHERE id=$1",
    )
    .bind(handoff.id)
    .fetch_one(owner)
    .await
    .unwrap();
    assert_eq!(recorded, Some(handoff.key));
    for _ in 0..2 {
        assert_eq!(handoff.poll().await.0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(handoff.state(owner).await, after);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn signed_handoff_removed_source_via_auth_denies(owner: PgPool) {
    removed_source(&owner, false, false).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn signed_handoff_removed_source_via_identity_denies(owner: PgPool) {
    removed_source(&owner, false, true).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn signed_handoff_source_removed_during_account_wait_denies(owner: PgPool) {
    source_removed_while_waiting(&owner, false).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn signed_handoff_null_source_denies(owner: PgPool) {
    corrupt_source(&owner, false, None).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn signed_handoff_foreign_account_source_denies(owner: PgPool) {
    corrupt_source(&owner, false, Some(false)).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn signed_handoff_foreign_company_source_denies(owner: PgPool) {
    corrupt_source(&owner, false, Some(true)).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn signed_handoff_retained_source_after_newer_key_issues_once(owner: PgPool) {
    retained_source(&owner, false).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn session_handoff_removed_source_via_auth_denies(owner: PgPool) {
    removed_source(&owner, true, false).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn session_handoff_removed_source_via_identity_denies(owner: PgPool) {
    removed_source(&owner, true, true).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn session_handoff_source_removed_during_account_wait_denies(owner: PgPool) {
    source_removed_while_waiting(&owner, true).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn session_handoff_null_source_denies(owner: PgPool) {
    corrupt_source(&owner, true, None).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn session_handoff_foreign_account_source_denies(owner: PgPool) {
    corrupt_source(&owner, true, Some(false)).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn session_handoff_foreign_company_source_denies(owner: PgPool) {
    corrupt_source(&owner, true, Some(true)).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn session_handoff_retained_source_after_newer_key_issues_once(owner: PgPool) {
    retained_source(&owner, true).await;
}
