//! Proves that onboarding a NEW tenant (`POST /api/platform/orgs`) provisions the
//! standard governed-config object catalog THROUGH the ontology engine: the
//! `support_slo_setting` (§4-26) and `console_view` (§19) object types land
//! PUBLISHED and org-scoped for the new tenant — created by the tenant's own admin
//! (so the registry FK to `users(id, org_id)` holds) and isolated from other orgs.
//!
//! The onboarding shell and catalog reads use a pool whose connections `SET ROLE
//! console_rt`, while catalog mutation uses a distinct pool whose connections `SET
//! ROLE console_ontology_cmd`. This mirrors the production runtime/command capability
//! split while keeping FORCE RLS active. The assertions read `ont_object_types` as
//! the OWNER with RLS off, so they see the truth regardless of any armed GUC.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::{Body, to_bytes};
use console_kernel_core::{OrgId, UserId};
use console_ontology_adapter_postgres::PgOntologyStore;
use console_ontology_adapter_postgres::seed::seed_governed_config_object_types;
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_provisioning::PlatformProvisioner;
use console_platform_rest::{PLATFORM_ORGS_PATH, PlatformRestState, TenantConfigSeeder, router};
use http::{Request, StatusCode, header};
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

/// The engine-backed catalog seeder, mirroring the production impl the App tier
/// injects (`console_app::tenant_config_seeder`). Kept here because the platform tier
/// only depends on the ontology adapter as a dev-dependency.
fn seeder(store: PgOntologyStore) -> TenantConfigSeeder {
    std::sync::Arc::new(move |org, actor, at| {
        let store = store.clone();
        Box::pin(async move {
            console_platform_request_context::scope_org(
                org,
                seed_governed_config_object_types(&store, actor, at),
            )
            .await
            .map(|_| ())
            .map_err(|err| err.to_string())
        })
    })
}

struct Harness {
    private_pem: String,
    public_pem: String,
    rt_pool: PgPool,
    command_pool: PgPool,
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
            command_pool: command_role_pool(owner_pool).await,
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
        router(
            PlatformRestState::new(
                self.rt_pool.clone(),
                Some(verifier),
                PlatformProvisioner::new(Duration::minutes(15)),
            )
            .with_tenant_config_seeder(Some(seeder(
                PgOntologyStore::new(self.rt_pool.clone())
                    .with_command_pool(self.command_pool.clone()),
            ))),
        )
    }

    async fn platform_token(&self, user_id: UserId) -> String {
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
                org_id: OrgId::platform(),
                roles: vec!["SUPER_ADMIN".to_owned()],
                branches: vec![],
                platform: true,
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

async fn command_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_ontology_cmd")
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

/// Insert the platform-admin user under the sentinel org so the onboarding audit
/// actor FK is satisfiable. Runs as the owner pool (RLS off for setup).
async fn seed_platform_admin(owner_pool: &PgPool) -> UserId {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, status) VALUES ($1, 'platform', 'Platform', 'ARCHIVED') ON CONFLICT (id) DO NOTHING",
    )
    .bind(*OrgId::platform().as_uuid())
    .execute(owner_pool)
    .await
    .unwrap();
    let id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*id.as_uuid())
        .bind("Platform Admin")
        .bind(vec!["SUPER_ADMIN".to_owned()])
        .bind(*OrgId::platform().as_uuid())
        .execute(owner_pool)
        .await
        .unwrap();
    id
}

async fn onboard(service: &Router, platform_token: &str, slug: &str) -> Uuid {
    let body = format!(r#"{{"slug":"{slug}","name":"Tenant {slug}"}}"#);
    let response = service
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(PLATFORM_ORGS_PATH)
                .header(header::AUTHORIZATION, format!("Bearer {platform_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    Uuid::parse_str(json["org"]["id"].as_str().unwrap()).unwrap()
}

/// The PUBLISHED object-type stable keys for `org_id`, read as the OWNER with RLS
/// off (so the assertion sees the truth regardless of any armed GUC).
async fn published_object_types(owner_pool: &PgPool, org_id: Uuid) -> Vec<String> {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let keys: Vec<String> = sqlx::query_scalar(
        // COLLATE "C" pins byte-order sort so the result matches the Rust
        // byte-sorted `expected` vec; the DB's default locale collation orders
        // `_` differently (workflow_definition before work_order) and diverges.
        "SELECT stable_key FROM ont_object_types \
         WHERE org_id = $1 AND lifecycle_state = 'published' ORDER BY stable_key COLLATE \"C\"",
    )
    .bind(org_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    keys
}

// ---------------------------------------------------------------------------
// Onboarding seeds the standard governed-config catalog, org-scoped + isolated.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../db/migrations")]
async fn onboarding_seeds_governed_config_object_types(owner_pool: PgPool) {
    let harness = Harness::new(&owner_pool).await;
    let admin = seed_platform_admin(&owner_pool).await;
    let token = harness.platform_token(admin).await;
    let service = harness.service();

    let org_a = onboard(&service, &token, "acme").await;
    let org_b = onboard(&service, &token, "beta").await;

    // Both new tenants get exactly the standard catalog, PUBLISHED and org-scoped.
    // The catalog has grown as parallel lanes extended
    // `seed_governed_config_object_types`: the 2 original governed-config types,
    // the niche instance-backed types (§A.2), the C- chain (contract → position →
    // posting), and the BE-semantic-backfill projected domain types (coverage
    // matrix gap lane #4) — kept alphabetically sorted to match the query.
    let expected: Vec<String> = vec![
        "approval",
        "compliance_framework",
        "compliance_obligation",
        "compliance_regulation",
        "console_view",
        "contract",
        "customer",
        "employee",
        "equipment",
        "evidence",
        "handover_policy",
        "labor_refusal",
        "leave_request",
        "mail",
        "messenger_thread",
        "position",
        "posting",
        "profitability_analytic",
        "regulation_param",
        "shift_timetable",
        "site",
        "site_coverage",
        "sla_setting",
        "support_slo_setting",
        "support_ticket",
        "work_order",
        "workflow_definition",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(
        published_object_types(&owner_pool, org_a).await,
        expected,
        "org A must be provisioned with the standard governed-config catalog"
    );
    assert_eq!(
        published_object_types(&owner_pool, org_b).await,
        expected,
        "org B must be provisioned independently with the same catalog"
    );

    // The seed rows are created_by the TENANT admin (a user in the new org), not
    // the platform principal — so the registry FK to users(id, org_id) holds and
    // no row is created_by the sentinel-org admin.
    let created_by_platform_admin: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ont_object_types WHERE created_by = $1")
            .bind(*admin.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(
        created_by_platform_admin, 0,
        "no seeded object type may be created_by the platform (sentinel-org) admin"
    );
}
