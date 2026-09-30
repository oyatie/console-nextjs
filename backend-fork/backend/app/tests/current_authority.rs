#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Signed requests against live PostgreSQL through the non-owner runtime role.
use console_kernel_core::{OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_request_context::resolve_principal_from_bearer_token;
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use sqlx::{PgPool, postgres::PgPoolOptions};
use time::{Duration, OffsetDateTime};

struct Fixture {
    private: String,
    public: String,
    owner: PgPool,
    runtime: PgPool,
    issuer: JwtIssuer,
    verifier: JwtVerifier,
    org: OrgId,
    user: UserId,
    family_id: uuid::Uuid,
}
impl Fixture {
    async fn new(owner: PgPool) -> Self {
        Self::with_home(owner, false).await
    }
    async fn with_home(owner: PgPool, platform: bool) -> Self {
        let runtime = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(
                owner
                    .connect_options()
                    .as_ref()
                    .clone()
                    .username("console_rt"),
            )
            .await
            .unwrap();
        let safe: bool = sqlx::query_scalar("SELECT NOT rolsuper AND NOT rolbypassrls AND current_user = 'console_rt' FROM pg_roles WHERE rolname = current_user")
            .fetch_one(&runtime).await.unwrap();
        assert!(safe);
        let org = OrgId::new();
        let user = UserId::new();
        sqlx::query("INSERT INTO organizations(id, slug, name) VALUES ($1, $2, 'Authority test')")
            .bind(*org.as_uuid())
            .bind(org.to_string())
            .execute(&owner)
            .await
            .unwrap();
        sqlx::query("INSERT INTO users(id, org_id, display_name, roles, is_active) VALUES ($1, $2, 'Authority subject', ARRAY['SUPER_ADMIN'], true)")
            .bind(*user.as_uuid()).bind(*if platform { OrgId::platform() } else { org }.as_uuid()).execute(&owner).await.unwrap();
        let key = SigningKey::random(&mut OsRng);
        let private = key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public = key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let settings = JwtSettings {
            issuer: "authority-test".into(),
            audience: "authority-test".into(),
            access_token_ttl: Duration::minutes(10),
        };
        let issuer =
            JwtIssuer::from_es256_pem(settings.clone(), private.as_bytes(), public.as_bytes())
                .unwrap();
        let verifier = JwtVerifier::from_es256_public_pem(settings, public.as_bytes()).unwrap();
        let family = console_platform_auth::RefreshTokenStore
            .issue_family(
                &runtime,
                *user.as_uuid(),
                if platform { OrgId::platform() } else { org },
                OffsetDateTime::now_utc(),
                Duration::hours(1),
            )
            .await
            .unwrap();
        Self {
            family_id: family.family_id,
            private: private.to_string(),
            public,
            owner,
            runtime,
            issuer,
            verifier,
            org,
            user,
        }
    }
    fn token(
        &self,
        org: OrgId,
        user: UserId,
        roles: &[&str],
        subject: u64,
        session: u64,
    ) -> String {
        self.issuer
            .issue_session_access_token(
                AccessTokenInput {
                    subject: user,
                    org_id: org,
                    roles: roles.iter().map(|r| (*r).to_owned()).collect(),
                    branches: vec![],
                    platform: false,
                    view_as: false,
                    read_only: false,
                    display_name: None,
                    feature_grants: vec![],
                    authz_subject_version: subject,
                    authz_policy_version: 0,
                    session_generation: session,
                    issued_at: OffsetDateTime::now_utc(),
                },
                None,
                vec![],
                self.family_id,
                OffsetDateTime::now_utc() + Duration::minutes(10),
            )
            .unwrap()
    }
    async fn allowed(&self, token: &str) -> bool {
        resolve_principal_from_bearer_token(&self.verifier, &self.runtime, token)
            .await
            .is_ok()
    }
    async fn versions(&self, subject: i64, session: i64) {
        sqlx::query("INSERT INTO subject_authz_versions(org_id, user_id, version, session_generation) VALUES ($1,$2,$3,$4) ON CONFLICT(org_id,user_id) DO UPDATE SET version = EXCLUDED.version, session_generation = EXCLUDED.session_generation")
            .bind(*self.org.as_uuid()).bind(*self.user.as_uuid()).bind(subject).bind(session)
            .execute(&self.owner).await.unwrap();
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn missing_and_cross_company_subjects_are_denied(owner: PgPool) {
    let f = Fixture::new(owner).await;
    assert!(
        f.allowed(&f.token(f.org, f.user, &["SUPER_ADMIN"], 0, 0))
            .await
    );
    assert!(
        !f.allowed(&f.token(f.org, UserId::new(), &["SUPER_ADMIN"], 0, 0))
            .await
    );
    assert!(
        !f.allowed(&f.token(OrgId::knl(), f.user, &["SUPER_ADMIN"], 0, 0))
            .await
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn deactivation_invalidates_the_exact_old_token(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let token = f.token(f.org, f.user, &["SUPER_ADMIN"], 0, 0);
    assert!(f.allowed(&token).await);
    sqlx::query("UPDATE users SET is_active = false WHERE id = $1")
        .bind(*f.user.as_uuid())
        .execute(&f.owner)
        .await
        .unwrap();
    assert!(!f.allowed(&token).await);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn live_role_removal_denies_and_role_addition_does_not_expand_tokens(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let token = f.token(f.org, f.user, &["SUPER_ADMIN"], 0, 0);
    sqlx::query("UPDATE users SET roles = ARRAY['MEMBER'] WHERE id = $1")
        .bind(*f.user.as_uuid())
        .execute(&f.owner)
        .await
        .unwrap();
    assert!(!f.allowed(&token).await);
    let member = f.token(f.org, f.user, &["MEMBER"], 0, 0);
    assert!(f.allowed(&member).await);
    sqlx::query("UPDATE users SET roles = ARRAY['MEMBER','SUPER_ADMIN'] WHERE id = $1")
        .bind(*f.user.as_uuid())
        .execute(&f.owner)
        .await
        .unwrap();
    let principal = resolve_principal_from_bearer_token(&f.verifier, &f.runtime, &member)
        .await
        .unwrap();
    assert_eq!(
        principal.roles,
        std::collections::BTreeSet::from([console_platform_authz::Role::Member])
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn subject_and_session_bumps_invalidate_old_tokens(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let old = f.token(f.org, f.user, &["SUPER_ADMIN"], 0, 0);
    f.versions(1, 1).await;
    assert!(!f.allowed(&old).await);
    let current = f.token(f.org, f.user, &["SUPER_ADMIN"], 1, 1);
    assert!(f.allowed(&current).await);
    f.versions(2, 1).await;
    assert!(!f.allowed(&current).await);
    let current = f.token(f.org, f.user, &["SUPER_ADMIN"], 2, 1);
    assert!(f.allowed(&current).await);
    f.versions(2, 2).await;
    assert!(!f.allowed(&current).await);
    assert!(
        f.allowed(&f.token(f.org, f.user, &["SUPER_ADMIN"], 2, 2))
            .await
    );
    assert!(
        !f.allowed(&f.token(f.org, f.user, &["SUPER_ADMIN"], 3, 3))
            .await
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn suspended_company_and_unavailable_store_fail_closed(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let token = f.token(f.org, f.user, &["SUPER_ADMIN"], 0, 0);
    sqlx::query("UPDATE organizations SET status = 'SUSPENDED' WHERE id = $1")
        .bind(*f.org.as_uuid())
        .execute(&f.owner)
        .await
        .unwrap();
    assert!(!f.allowed(&token).await);
    f.runtime.close().await;
    assert!(!f.allowed(&token).await);
}

use axum::{
    Router,
    body::{Body, to_bytes},
};
use http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
impl Fixture {
    fn resign(&self, token: &str, patch: Value) -> String {
        let mut claims =
            serde_json::to_value(self.verifier.verify_access_token(token).unwrap()).unwrap();
        claims
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256),
            &claims,
            &jsonwebtoken::EncodingKey::from_ec_pem(self.private.as_bytes()).unwrap(),
        )
        .unwrap()
    }
    fn auth(&self) -> Router {
        use console_platform_auth_rest::{AuthRestConfig, AuthRestState};
        console_platform_auth_rest::router(
            AuthRestState::new(
                self.runtime.clone(),
                AuthRestConfig {
                    rp_id: "example.com".into(),
                    rp_origin: "https://example.com".into(),
                    rp_name: "Authority test".into(),
                    ceremony_ttl: Duration::minutes(5),
                    jwt_issuer: "authority-test".into(),
                    jwt_audience: "authority-test".into(),
                    jwt_private_key_pem: self.private.clone(),
                    jwt_public_key_pem: self.public.clone(),
                    refresh_token_ttl: Duration::hours(1),
                    refresh_family_absolute_ttl: Duration::hours(2),
                    cookie_secure: true,
                },
            )
            .unwrap(),
        )
    }
    fn platform(&self) -> Router {
        console_platform_rest::router(
            console_platform_rest::PlatformRestState::new(
                self.runtime.clone(),
                Some(self.verifier.clone()),
                console_platform_provisioning::PlatformProvisioner::new(Duration::minutes(15)),
            )
            .with_view_as_issuer(Some(self.issuer.clone())),
        )
    }
    fn platform_token(&self, role: &str) -> String {
        self.resign(
            &self.token(OrgId::platform(), self.user, &[role], 0, 0),
            json!({"platform": true}),
        )
    }
    async fn group(&self) -> uuid::Uuid {
        let group = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO groups(id,slug,name) VALUES ($1,$2,'Authority group')")
            .bind(group)
            .bind(group.to_string())
            .execute(&self.owner)
            .await
            .unwrap();
        for org in [self.org, OrgId::knl()] {
            sqlx::query("SELECT platform_assign_org_to_group($1,$2)")
                .bind(group)
                .bind(*org.as_uuid())
                .execute(&self.owner)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO group_role_grants(group_id,user_id,group_role) VALUES ($1,$2,'GROUP_ADMIN')").bind(group).bind(*self.user.as_uuid()).execute(&self.owner).await.unwrap();
        group
    }
}
async fn request(
    router: Router,
    token: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = router
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn platform_roles_cannot_expand_old_tokens_and_revocation_is_live(owner: PgPool) {
    let f = Fixture::with_home(owner, true).await;
    let member = f.platform_token("MEMBER");
    // Current SUPER_ADMIN must not promote a signed MEMBER session.
    assert_eq!(
        request(
            f.platform(),
            &member,
            "GET",
            "/api/platform/orgs",
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let token = f.platform_token("SUPER_ADMIN");
    assert_eq!(
        request(
            f.platform(),
            &token,
            "GET",
            "/api/platform/orgs",
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
        .bind(*f.user.as_uuid())
        .execute(&f.owner)
        .await
        .unwrap();
    assert_eq!(
        request(
            f.platform(),
            &token,
            "GET",
            "/api/platform/orgs",
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn platform_delegation_binds_home_session_and_live_target(owner: PgPool) {
    let f = Fixture::with_home(owner, true).await;
    let source = f.platform_token("SUPER_ADMIN");
    for path in ["/api/platform/view-as", "/api/platform/tenant-context"] {
        let (status, body) = request(
            f.platform(),
            &source,
            "POST",
            path,
            json!({"org_id":f.org,"role":"SUPER_ADMIN"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        let token = body["access_token"].as_str().unwrap();
        let claims = serde_json::to_value(f.verifier.verify_access_token(token).unwrap()).unwrap();
        assert_eq!(
            claims["actor_session"],
            json!({"home_org":OrgId::platform(),"subject_version":0,"session_generation":0,"family_id":f.family_id,"expires_at":f.verifier.verify_access_token(&source).unwrap().exp})
        );
        assert_eq!(claims["authz_subject_version"], 0);
        assert!(f.allowed(token).await);
        assert!(
            !f.allowed(&f.resign(token, json!({"actor_session":null})))
                .await
        );
        assert!(!f.allowed(&f.resign(token, json!({"actor_session":{"home_org":f.org,"subject_version":0,"session_generation":0,"family_id":f.family_id,"expires_at":f.verifier.verify_access_token(&source).unwrap().exp}}))).await);
        sqlx::query("UPDATE organizations SET status='SUSPENDED' WHERE id=$1")
            .bind(*f.org.as_uuid())
            .execute(&f.owner)
            .await
            .unwrap();
        assert!(!f.allowed(token).await);
        sqlx::query("UPDATE organizations SET status='ACTIVE' WHERE id=$1")
            .bind(*f.org.as_uuid())
            .execute(&f.owner)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET is_active=false WHERE id=$1")
            .bind(*f.user.as_uuid())
            .execute(&f.owner)
            .await
            .unwrap();
        assert!(!f.allowed(token).await);
        sqlx::query("UPDATE users SET is_active=true WHERE id=$1")
            .bind(*f.user.as_uuid())
            .execute(&f.owner)
            .await
            .unwrap();
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn group_delegate_keeps_target_freshness_and_denies_home_revocation(owner: PgPool) {
    let f = Fixture::new(owner).await;
    f.group().await;
    f.versions(7, 3).await;
    let source = f.resign(
        &f.token(f.org, f.user, &["SUPER_ADMIN"], 7, 3),
        json!({"group_roles":["GROUP_ADMIN"]}),
    );
    let (status, body) = request(
        f.auth(),
        &source,
        "POST",
        "/api/v1/group-admin/tenant-context",
        json!({"org_id":OrgId::knl()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["access_token"].as_str().unwrap();
    let claims = serde_json::to_value(f.verifier.verify_access_token(token).unwrap()).unwrap();
    assert_eq!(claims["authz_subject_version"], 0);
    assert_eq!(claims["session_generation"], 0);
    assert_eq!(
        claims["actor_session"],
        json!({"home_org":f.org,"subject_version":7,"session_generation":3,"family_id":f.family_id,"expires_at":f.verifier.verify_access_token(&source).unwrap().exp})
    );
    assert!(f.allowed(token).await);
    assert!(
        !f.allowed(&f.resign(token, json!({"actor_session":null})))
            .await
    );
    assert!(
        !f.allowed(&f.resign(token, json!({"actor_home_org":OrgId::knl()})))
            .await
    );
    assert_eq!(
        request(f.auth(), token, "GET", "/api/v1/auth/passkeys", Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    f.versions(7, 4).await;
    assert!(!f.allowed(token).await);
    assert_eq!(
        request(
            f.auth(),
            &source,
            "GET",
            "/api/v1/group-admin/groups",
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn auth_self_service_rejects_stale_session(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let token = f.token(f.org, f.user, &["SUPER_ADMIN"], 0, 0);
    assert_eq!(
        request(
            f.auth(),
            &token,
            "GET",
            "/api/v1/auth/passkeys",
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    f.versions(1, 1).await;
    assert_eq!(
        request(
            f.auth(),
            &token,
            "GET",
            "/api/v1/auth/passkeys",
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            f.auth(),
            &token,
            "POST",
            "/api/v1/auth/passkey/register/start",
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn revocation_during_delegate_mint_cannot_renew_source_session(owner: PgPool) {
    let f = Fixture::with_home(owner, true).await;
    let source = f.platform_token("SUPER_ADMIN");
    let mut lock = f.owner.begin().await.unwrap();
    sqlx::query("LOCK TABLE policy_versions IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let router = f.platform();
    let target = f.org;
    let mint = tokio::spawn(async move {
        request(
            router,
            &source,
            "POST",
            "/api/platform/tenant-context",
            json!({"org_id":target}),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE usename='console_rt' AND wait_event_type='Lock' AND query LIKE '%policy_versions%')")
                .fetch_one(&f.owner).await.unwrap();
            if blocked { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("mint reached target freshness after source validation");
    sqlx::query("INSERT INTO subject_authz_versions(org_id,user_id,version,session_generation) VALUES ($1,$2,1,1)")
        .bind(*OrgId::platform().as_uuid()).bind(*f.user.as_uuid()).execute(&f.owner).await.unwrap();
    lock.rollback().await.unwrap();
    let (status, body) = mint.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["access_token"].as_str().unwrap();
    let claims = serde_json::to_value(f.verifier.verify_access_token(token).unwrap()).unwrap();
    assert_eq!(claims["actor_session"]["session_generation"], 0);
    assert!(!f.allowed(token).await);
}

struct SessionFixture {
    family_id: uuid::Uuid,
    original_refresh: String,
    access: String,
    refresh: String,
}
impl Fixture {
    async fn session(&self) -> SessionFixture {
        let home: uuid::Uuid = sqlx::query_scalar("SELECT org_id FROM users WHERE id=$1")
            .bind(*self.user.as_uuid())
            .fetch_one(&self.owner)
            .await
            .unwrap();
        let issued = console_platform_auth::RefreshTokenStore
            .issue_family(
                &self.runtime,
                *self.user.as_uuid(),
                OrgId::from_uuid(home),
                OffsetDateTime::now_utc(),
                Duration::hours(1),
            )
            .await
            .unwrap();
        let original_refresh = issued.token.as_str().to_owned();
        let (status, body) = request(
            self.auth(),
            "",
            "POST",
            console_platform_auth_rest::TOKEN_REFRESH_PATH,
            json!({"refresh_token":original_refresh}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        SessionFixture {
            family_id: issued.family_id,
            original_refresh,
            access: body["access_token"].as_str().unwrap().to_owned(),
            refresh: body["refresh_token"].as_str().unwrap().to_owned(),
        }
    }
    async fn logout(&self, session: &SessionFixture) {
        assert_eq!(
            request(
                self.auth(),
                "",
                "POST",
                "/api/v1/auth/logout",
                json!({"refresh_token":session.refresh})
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn logout_revokes_exact_access_token_and_preserves_other_family(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let first = f.session().await;
    let second = f.session().await;
    assert!(f.allowed(&first.access).await);
    assert!(f.allowed(&second.access).await);
    f.logout(&first).await;
    assert!(!f.allowed(&first.access).await);
    assert!(f.allowed(&second.access).await);
    assert_eq!(
        request(
            f.auth(),
            &first.access,
            "GET",
            "/api/v1/auth/passkeys",
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let claims =
        serde_json::to_value(f.verifier.verify_access_token(&second.access).unwrap()).unwrap();
    assert_eq!(claims["session_family_id"], json!(second.family_id));
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn refresh_reuse_revokes_old_and_new_access_tokens(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let session = f.session().await;
    let (status, body) = request(
        f.auth(),
        "",
        "POST",
        console_platform_auth_rest::TOKEN_REFRESH_PATH,
        json!({"refresh_token":session.refresh}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let newest = body["access_token"].as_str().unwrap();
    assert!(f.allowed(newest).await);
    assert_eq!(
        request(
            f.auth(),
            "",
            "POST",
            console_platform_auth_rest::TOKEN_REFRESH_PATH,
            json!({"refresh_token":session.original_refresh})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert!(!f.allowed(&session.access).await);
    assert!(!f.allowed(newest).await);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn absent_unknown_foreign_and_deleted_families_cannot_authorize(owner: PgPool) {
    let f = Fixture::new(owner.clone()).await;
    let other = Fixture::new(owner).await;
    let session = f.session().await;
    let foreign = other.session().await;
    let peer = UserId::new();
    sqlx::query(
        "INSERT INTO users(id, org_id, display_name, roles) VALUES ($1,$2,'Peer',ARRAY['MEMBER'])",
    )
    .bind(*peer.as_uuid())
    .bind(*f.org.as_uuid())
    .execute(&f.owner)
    .await
    .unwrap();
    let same_company = console_platform_auth::RefreshTokenStore
        .issue_family(
            &f.runtime,
            *peer.as_uuid(),
            f.org,
            OffsetDateTime::now_utc(),
            Duration::hours(1),
        )
        .await
        .unwrap();
    // The database's composite foreign key already rejects a family in a
    // different Company from its account. Preserve that independent boundary.
    let wrong_company = console_platform_auth::RefreshTokenStore
        .issue_family(
            &f.runtime,
            *f.user.as_uuid(),
            other.org,
            OffsetDateTime::now_utc(),
            Duration::hours(1),
        )
        .await;
    assert!(wrong_company.is_err());
    for family in [
        Value::Null,
        json!(uuid::Uuid::new_v4()),
        json!(foreign.family_id),
        json!(same_company.family_id),
    ] {
        assert!(
            !f.allowed(&f.resign(&session.access, json!({"session_family_id":family})))
                .await
        );
    }
    sqlx::query("DELETE FROM auth_refresh_token_families WHERE id=$1")
        .bind(session.family_id)
        .execute(&f.owner)
        .await
        .unwrap();
    assert!(!f.allowed(&session.access).await);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn group_delegate_cannot_outlive_or_escape_source_family(owner: PgPool) {
    let f = Fixture::new(owner).await;
    f.group().await;
    let session = f.session().await;
    let expiry = OffsetDateTime::now_utc().unix_timestamp() + 20;
    let source = f.resign(&session.access, json!({"exp":expiry}));
    let (status, body) = request(
        f.auth(),
        &source,
        "POST",
        "/api/v1/group-admin/tenant-context",
        json!({"org_id":OrgId::knl()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let child = body["access_token"].as_str().unwrap();
    let claims = serde_json::to_value(f.verifier.verify_access_token(child).unwrap()).unwrap();
    assert_eq!(
        claims["actor_session"]["family_id"],
        json!(session.family_id)
    );
    assert!(claims["exp"].as_i64().unwrap() <= expiry);
    assert!(f.allowed(child).await);
    let conflicting = f.resign(child, json!({"session_family_id": session.family_id}));
    assert!(
        !f.allowed(&conflicting).await,
        "delegates cannot also carry ordinary bindings"
    );
    let mut actor = claims["actor_session"].clone();
    actor["family_id"] = json!(uuid::Uuid::new_v4());
    assert!(
        !f.allowed(&f.resign(child, json!({"actor_session":actor})))
            .await
    );
    for deadline in [OffsetDateTime::now_utc().unix_timestamp(), expiry - 1] {
        let mut actor = claims["actor_session"].clone();
        actor["expires_at"] = json!(deadline);
        assert!(
            !f.allowed(&f.resign(child, json!({"actor_session":actor})))
                .await
        );
    }
    f.logout(&session).await;
    assert!(!f.allowed(child).await);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn family_revocation_during_platform_mint_cannot_create_a_live_child(owner: PgPool) {
    let f = Fixture::with_home(owner, true).await;
    let session = f.session().await;
    let mut lock = f.owner.begin().await.unwrap();
    sqlx::query("LOCK TABLE policy_versions IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let router = f.platform();
    let target = f.org;
    let source = session.access.clone();
    let mint = tokio::spawn(async move {
        request(
            router,
            &source,
            "POST",
            "/api/platform/tenant-context",
            json!({"org_id":target}),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE usename='console_rt' AND wait_event_type='Lock' AND query LIKE '%policy_versions%')").fetch_one(&f.owner).await.unwrap();
            if blocked {break;}
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("delegate mint reached target read after source validation");
    f.logout(&session).await;
    lock.rollback().await.unwrap();
    let (status, body) = mint.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let child = body["access_token"].as_str().unwrap();
    assert!(!f.allowed(child).await);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn existing_refresh_family_upgrades_without_extending_absolute_deadline(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let now = OffsetDateTime::now_utc();
    let issued = console_platform_auth::RefreshTokenStore
        .issue_family(
            &f.runtime,
            *f.user.as_uuid(),
            f.org,
            now,
            Duration::hours(1),
        )
        .await
        .unwrap();
    let created_at = now - Duration::hours(2) + Duration::seconds(20);
    sqlx::query("UPDATE auth_refresh_token_families SET created_at=$1 WHERE id=$2")
        .bind(created_at)
        .bind(issued.family_id)
        .execute(&f.owner)
        .await
        .unwrap();
    let (status, body) = request(
        f.auth(),
        "",
        "POST",
        console_platform_auth_rest::TOKEN_REFRESH_PATH,
        json!({"refresh_token":issued.token.as_str()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let access = body["access_token"].as_str().unwrap();
    let claims = serde_json::to_value(f.verifier.verify_access_token(access).unwrap()).unwrap();
    assert_eq!(claims["session_family_id"], json!(issued.family_id));
    assert!(claims["exp"].as_i64().unwrap() <= (created_at + Duration::hours(2)).unix_timestamp());
    assert!(f.allowed(access).await);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn absolute_deadline_and_access_expiry_are_strict(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let now = OffsetDateTime::now_utc();
    let issued = console_platform_auth::RefreshTokenStore
        .issue_family(
            &f.runtime,
            *f.user.as_uuid(),
            f.org,
            now,
            Duration::hours(1),
        )
        .await
        .unwrap();
    // Read PostgreSQL's stored timestamp precision for an exact equality test.
    let created: OffsetDateTime =
        sqlx::query_scalar("SELECT created_at FROM auth_refresh_token_families WHERE id=$1")
            .bind(issued.family_id)
            .fetch_one(&f.owner)
            .await
            .unwrap();
    let result = console_platform_auth::RefreshTokenStore
        .rotate(
            &f.runtime,
            issued.token.as_str(),
            created + Duration::minutes(5),
            Duration::hours(1),
            Duration::minutes(5),
        )
        .await;
    assert!(matches!(
        result,
        Err(console_platform_auth::RefreshTokenUseError::FamilyRevoked)
    ));
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn access_expiry_has_no_grace_period(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let session = f.session().await;
    for seconds_ago in [0, 1, 30] {
        let expired = f.resign(
            &session.access,
            json!({"exp":OffsetDateTime::now_utc().unix_timestamp()-seconds_ago}),
        );
        assert!(
            !f.allowed(&expired).await,
            "expiry must not use JWT library grace time"
        );
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn refresh_waiting_for_family_lock_cannot_cross_absolute_deadline(owner: PgPool) {
    let f = Fixture::new(owner).await;
    let now = OffsetDateTime::now_utc();
    let issued = console_platform_auth::RefreshTokenStore
        .issue_family(
            &f.runtime,
            *f.user.as_uuid(),
            f.org,
            now,
            Duration::hours(1),
        )
        .await
        .unwrap();
    let mut lock = f.owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM auth_refresh_token_families WHERE id=$1 FOR UPDATE")
        .bind(issued.family_id)
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    let runtime = f.runtime.clone();
    let token = issued.token.as_str().to_owned();
    let rotate = tokio::spawn(async move {
        console_platform_auth::RefreshTokenStore
            .rotate(
                &runtime,
                &token,
                now,
                Duration::hours(1),
                Duration::seconds(2),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE usename='console_rt' AND wait_event_type='Lock' AND query LIKE '%FOR UPDATE OF t, f%')")
                .fetch_one(&f.owner).await.unwrap();
            if blocked { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("refresh reached the family lock");
    while OffsetDateTime::now_utc() < now + Duration::seconds(2) {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    lock.rollback().await.unwrap();
    assert!(matches!(
        rotate.await.unwrap(),
        Err(console_platform_auth::RefreshTokenUseError::FamilyRevoked)
    ));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_refresh_tokens WHERE family_id=$1")
            .bind(issued.family_id)
            .fetch_one(&f.owner)
            .await
            .unwrap();
    assert_eq!(count, 1, "no replacement may be minted after the deadline");
}
