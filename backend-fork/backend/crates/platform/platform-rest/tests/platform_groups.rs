//! Platform group management integration tests.
//!
//! Proves the platform console can manage group identity and subsidiary
//! membership through the runtime `console_rt` role without exposing raw
//! `group_memberships` to application SQL.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::{Body, to_bytes};
use console_kernel_core::{OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_provisioning::PlatformProvisioner;
use console_platform_rest::{PLATFORM_GROUPS_PATH, PlatformRestState, router};
use http::{Method, Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::Value;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

struct Harness {
    private_pem: String,
    public_pem: String,
    /// Runtime-role pool the app router uses (every connection is `console_rt`).
    rt_pool: PgPool,
}

impl Harness {
    async fn new(owner_pool: &PgPool) -> Self {
        let signing_key = SigningKey::random(&mut OsRng);
        let private_pem = signing_key
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap()
            .to_string();
        let public_pem = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        Self {
            private_pem,
            public_pem,
            rt_pool: runtime_role_pool(owner_pool).await,
        }
    }

    fn service(&self) -> Router {
        let verifier = JwtVerifier::from_es256_public_pem(
            JwtSettings {
                issuer: TEST_ISSUER.to_owned(),
                audience: TEST_AUDIENCE.to_owned(),
                access_token_ttl: Duration::minutes(15),
            },
            self.public_pem.as_bytes(),
        )
        .unwrap();
        router(PlatformRestState::new(
            self.rt_pool.clone(),
            Some(verifier),
            PlatformProvisioner::new(Duration::minutes(15)),
        ))
    }

    async fn token(&self, user_id: UserId, org_id: OrgId, platform: bool) -> String {
        let issuer = JwtIssuer::from_es256_pem(
            JwtSettings {
                issuer: TEST_ISSUER.to_owned(),
                audience: TEST_AUDIENCE.to_owned(),
                access_token_ttl: Duration::minutes(15),
            },
            self.private_pem.as_bytes(),
            self.public_pem.as_bytes(),
        )
        .unwrap();
        console_platform_test_support::issue_session_token(
            &self.rt_pool,
            &issuer,
            AccessTokenInput {
                subject: user_id,
                org_id,
                roles: vec!["SUPER_ADMIN".to_owned()],
                branches: vec![],
                platform,
                view_as: false,
                read_only: false,
                display_name: None,
                feature_grants: Vec::new(),
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

async fn request(
    service: &Router,
    method: Method,
    path: String,
    token: &str,
    body: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let response = service
        .clone()
        .oneshot(
            builder
                .body(Body::from(body.unwrap_or_default().to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, json)
}

async fn seed_platform_admin(pool: &PgPool) -> UserId {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, status) VALUES ($1, 'platform', 'Platform', 'ARCHIVED') ON CONFLICT (id) DO NOTHING",
    )
    .bind(*OrgId::platform().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    let id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*id.as_uuid())
        .bind("Platform Admin")
        .bind(vec!["SUPER_ADMIN".to_owned()])
        .bind(*OrgId::platform().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn seed_tenant(pool: &PgPool, slug: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO organizations (slug, name) VALUES ($1, $2) RETURNING id")
        .bind(format!(
            "{slug}-{}",
            &Uuid::new_v4().simple().to_string()[..8]
        ))
        .bind(format!("Tenant {slug}"))
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn platform_group_crud_assigns_subsidiaries_and_audits(pool: PgPool) {
    let harness = Harness::new(&pool).await;
    let admin = seed_platform_admin(&pool).await;
    let platform_token = harness.token(admin, OrgId::platform(), true).await;
    let tenant_a = seed_tenant(&pool, "alpha").await;
    let _tenant_b = seed_tenant(&pool, "beta").await;
    let service = harness.service();

    let (status, created) = request(
        &service,
        Method::POST,
        PLATFORM_GROUPS_PATH.to_owned(),
        &platform_token,
        Some(r#"{"slug":"group-test","name":"그룹"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let group_id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
    assert_eq!(created["member_count"], 0);

    let (status, updated) = request(
        &service,
        Method::PATCH,
        format!("/api/platform/groups/{group_id}"),
        &platform_token,
        Some(r#"{"slug":"group-renamed","name":"그룹 본사"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated:?}");
    assert_eq!(updated["slug"], "group-renamed");
    assert_eq!(updated["name"], "그룹 본사");

    let (status, assigned) = request(
        &service,
        Method::PUT,
        format!("/api/platform/groups/{group_id}/organizations/{tenant_a}"),
        &platform_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{assigned:?}");
    assert_eq!(assigned["group_id"].as_str().unwrap(), group_id.to_string());
    assert_eq!(assigned["group_name"], "그룹 본사");

    let (status, created_account) = request(
        &service,
        Method::POST,
        format!("/api/platform/groups/{group_id}/accounts"),
        &platform_token,
        Some(&format!(
            r#"{{
                "org_id":"{tenant_a}",
                "display_name":"개발자",
                "phone":"webservicepost@gmail.com",
                "tenant_roles":["MEMBER"],
                "group_role":"GROUP_ADMIN"
            }}"#
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(created_account["otp"].as_str().unwrap().len() >= 8);
    let account = &created_account["account"];
    let account_user_id = Uuid::parse_str(account["user_id"].as_str().unwrap()).unwrap();
    assert_eq!(account["display_name"], "개발자");
    assert_eq!(account["phone"], "webservicepost@gmail.com");
    assert_eq!(account["org_id"], tenant_a.to_string());
    assert_eq!(account["account_status"], "PENDING_SETUP");
    assert_eq!(account["tenant_roles"].as_array().unwrap()[0], "MEMBER");
    assert_eq!(account["group_roles"].as_array().unwrap()[0], "GROUP_ADMIN");

    let open_otp_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM auth_bootstrap_credentials WHERE user_id = $1 AND org_id = $2 AND consumed_at IS NULL AND revoked_at IS NULL",
    )
    .bind(account_user_id)
    .bind(tenant_a)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(open_otp_count, 1, "group account must get one setup OTP");

    let (status, accounts) = request(
        &service,
        Method::GET,
        format!("/api/platform/groups/{group_id}/accounts"),
        &platform_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{accounts:?}");
    assert_eq!(accounts.as_array().unwrap().len(), 1);
    assert_eq!(
        accounts.as_array().unwrap()[0]["user_id"],
        account_user_id.to_string()
    );

    let (status, body) = request(
        &service,
        Method::DELETE,
        format!("/api/platform/groups/{group_id}/accounts/{account_user_id}/roles/GROUP_ADMIN"),
        &platform_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body:?}");

    let (status, listed) = request(
        &service,
        Method::GET,
        PLATFORM_GROUPS_PATH.to_owned(),
        &platform_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed:?}");
    let groups = listed.as_array().expect("groups array");
    let group = groups
        .iter()
        .find(|item| item["id"].as_str() == Some(&group_id.to_string()))
        .expect("created group in list");
    assert_eq!(group["member_count"], 1);
    assert_eq!(
        group["members"].as_array().unwrap()[0]["id"],
        tenant_a.to_string()
    );

    let (status, removed) = request(
        &service,
        Method::DELETE,
        format!("/api/platform/groups/{group_id}/organizations/{tenant_a}"),
        &platform_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed:?}");
    let removed_group_id = Uuid::parse_str(
        removed["group_id"]
            .as_str()
            .expect("remove must return a Group-of-one group_id"),
    )
    .unwrap();
    assert_ne!(removed_group_id, tenant_a);
    assert_ne!(removed_group_id, group_id);
    let membership_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_memberships WHERE org_id = $1")
            .bind(tenant_a)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(membership_count, 1);
    let membership_group: Uuid =
        sqlx::query_scalar("SELECT group_id FROM group_memberships WHERE org_id = $1")
            .bind(tenant_a)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(membership_group, removed_group_id);

    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE actor = $1 AND action IN ('platform.group.create', 'platform.group.update', 'platform.group.assign_org', 'platform.group.accounts.list', 'platform.group.account.create', 'platform.group.account.revoke', 'platform.group.list', 'platform.group.remove_org')",
    )
    .bind(*admin.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 8, "every group action must be audited");

    let raw_read_err = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM group_memberships")
        .fetch_one(&harness.rt_pool)
        .await
        .expect_err("runtime role must not read raw group_memberships")
        .to_string();
    assert!(
        raw_read_err.contains("permission denied"),
        "raw group_memberships read as console_rt must be denied, got: {raw_read_err}"
    );

    let raw_grants_err = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM group_role_grants")
        .fetch_one(&harness.rt_pool)
        .await
        .expect_err("runtime role must not read raw group_role_grants")
        .to_string();
    assert!(
        raw_grants_err.contains("permission denied"),
        "raw group_role_grants read as console_rt must be denied, got: {raw_grants_err}"
    );
}
