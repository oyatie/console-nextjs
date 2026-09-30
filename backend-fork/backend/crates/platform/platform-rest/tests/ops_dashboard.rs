//! Platform ops dashboard (`GET /api/platform/ops`) integration tests.
//!
//! Verifies the two security invariants the cross-tenant dashboard depends on:
//! 1. a TENANT token is rejected with 403 (the platform extractor refuses any
//!    non-platform token before the handler runs), and
//! 2. a PLATFORM token aggregates per-tenant health AND records an audited
//!    cross-tenant read (`platform.tenant.health`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::{Body, to_bytes};
use console_kernel_core::{OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_provisioning::PlatformProvisioner;
use console_platform_rest::{PLATFORM_OPS_PATH, PlatformRestState, router};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::Value;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

struct Harness {
    private_pem: String,
    public_pem: String,
    pool: PgPool,
}

impl Harness {
    fn new(pool: PgPool) -> Self {
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
            pool,
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
            self.pool.clone(),
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
            &self.pool,
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

async fn get(service: &Router, path: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = service.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    // The platform extractor's tier-mismatch rejection is a plain-text body
    // (not JSON), so parse leniently and fall back to a string value.
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, json)
}

/// Insert the platform-admin user under the sentinel org so the audit-event
/// actor FK is satisfiable. Runs as the owner pool role (bypasses RLS).
async fn seed_platform_admin(pool: &PgPool) -> UserId {
    // The sentinel organizations row is seeded by migration 0036; ensure it.
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
async fn tenant_token_is_rejected_with_403(pool: PgPool) {
    let harness = Harness::new(pool.clone());
    // A normal TENANT token (platform = false) under a real tenant org.
    let tenant_token = harness
        .token(
            console_platform_test_support::seed_org_and_super_admin(
                &pool,
                *OrgId::knl().as_uuid(),
                "tier-denial",
            )
            .await,
            OrgId::knl(),
            false,
        )
        .await;

    let (status, _body) = get(&harness.service(), PLATFORM_OPS_PATH, &tenant_token).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a tenant token must never reach /api/platform/ops"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn platform_token_aggregates_tenant_health_and_audits_the_read(pool: PgPool) {
    let harness = Harness::new(pool.clone());
    let admin = seed_platform_admin(&pool).await;
    let platform_token = harness.token(admin, OrgId::platform(), true).await;

    // Two real tenants beyond the migration-seeded KNL.
    seed_tenant(&pool, "alpha").await;
    seed_tenant(&pool, "beta").await;

    let (status, body) = get(&harness.service(), PLATFORM_OPS_PATH, &platform_token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    let tenants = body["tenants"].as_array().expect("tenants array");
    // KNL (seeded) + alpha + beta = 3; the platform sentinel is excluded.
    assert_eq!(tenants.len(), 3, "{body:?}");
    let slugs: Vec<&str> = tenants
        .iter()
        .map(|t| t["slug"].as_str().unwrap())
        .collect();
    assert!(slugs.contains(&"knl"));
    assert!(
        !tenants.iter().any(|t| t["slug"] == "platform"),
        "the platform sentinel must never appear in the ops list"
    );
    // Each row carries the health numbers.
    for tenant in tenants {
        assert!(tenant["user_count"].is_number());
        assert!(tenant["active_work_orders"].is_number());
        assert!(tenant["open_work_orders"].is_number());
    }

    // The cross-tenant read is audited as platform.tenant.health by the admin.
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'platform.tenant.health' AND actor = $1",
    )
    .bind(*admin.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        audit_count, 1,
        "the cross-tenant read must be audited exactly once"
    );
}
