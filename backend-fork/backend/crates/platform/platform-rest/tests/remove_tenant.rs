//! GUARDED tenant hard-removal (`DELETE /api/platform/orgs/{id}`) tests.
//!
//! Proves the enterprise invariants of the removal:
//!   (a) a platform-super-admin can remove an EMPTY tenant — its shell rows are
//!       gone and the org is gone, and its immutable audit trail SURVIVES,
//!       re-homed to the platform sentinel;
//!   (b) removal of a tenant WITH real data (equipment) is REFUSED with 409 and
//!       nothing is deleted (the transaction rolled back);
//!   (c) a non-platform (tenant) principal is rejected with 403;
//!   (d) cross-tenant: removing org A does not touch org B's rows;
//!   (e) audit-immutability is intact — a direct `console_rt` UPDATE on audit_events
//!       still raises (the removal's re-home is the ONLY sanctioned path).
//!
//! The app router runs against a pool whose connections `SET ROLE console_rt`, so the
//! removal exercises the production runtime role under FORCE RLS — a superuser
//! pool would mask whether the SECURITY DEFINER escape actually works for `console_rt`
//! (per the project's rls-verify-as-runtime-role discipline).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::{Body, to_bytes};
use console_kernel_core::{OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_provisioning::PlatformProvisioner;
use console_platform_rest::{PLATFORM_ORGS_PATH, PlatformRestState, router};
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

struct Harness {
    private_pem: String,
    public_pem: String,
    /// The runtime-role pool the app router uses (every connection is `console_rt`).
    rt_pool: PgPool,
    force_pool: PgPool,
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
            force_pool: command_role_pool(owner_pool).await,
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
            .with_force_remove_command_pool(Some(self.force_pool.clone())),
        )
    }

    fn service_without_force_command(&self) -> Router {
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

/// A pool whose every connection runs `SET ROLE console_rt`, so the app router's
/// statements execute as the production runtime role (NOSUPERUSER, NOBYPASSRLS)
/// under FORCE RLS — the same pattern the identity/auth runtime-RLS tests use.
async fn command_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_platform_force_cmd")
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
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

/// Onboard a tenant THROUGH the platform API (so it gets the exact onboarding
/// shell: org + one SUPER_ADMIN + a bootstrap OTP + the create audit row).
/// Returns the new org id.
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

async fn delete_org(service: &Router, token: &str, org_id: Uuid) -> (StatusCode, Value) {
    delete_org_inner(service, token, &format!("/api/platform/orgs/{org_id}")).await
}

/// FORCE delete: `DELETE /api/platform/orgs/{id}?delete_data=true`.
async fn force_delete_org(service: &Router, token: &str, org_id: Uuid) -> (StatusCode, Value) {
    delete_org_inner(
        service,
        token,
        &format!("/api/platform/orgs/{org_id}?delete_data=true"),
    )
    .await
}

async fn delete_org_inner(service: &Router, token: &str, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("DELETE")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = service.clone().oneshot(request).await.unwrap();
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

/// Archive a tenant through the platform API (PATCH status=ARCHIVED), the
/// reversible step that must precede a force-removal.
async fn archive_org(service: &Router, token: &str, org_id: Uuid) {
    let request = Request::builder()
        .method("PATCH")
        .uri(format!("/api/platform/orgs/{org_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"status":"ARCHIVED"}"#))
        .unwrap();
    let response = service.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "archiving a tenant must succeed"
    );
}

/// Insert the platform-admin user under the sentinel org so the audit-event actor
/// FK is satisfiable. Runs as the owner pool (bypasses RLS for setup).
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

/// Count this org's rows in a tenant table, as the OWNER with RLS off (so the
/// assertion sees the truth regardless of any armed GUC). `table` is always a
/// hardcoded test literal (never request-derived), so `AssertSqlSafe` is sound.
async fn count_in_org(owner_pool: &PgPool, table: &str, org_id: Uuid) -> i64 {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let sql = sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table} WHERE org_id = $1"));
    let count: i64 = sqlx::query_scalar(sql)
        .bind(org_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    count
}

/// Whether an organizations row still exists (owner, RLS off).
async fn org_exists(owner_pool: &PgPool, org_id: Uuid) -> bool {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM organizations WHERE id = $1)")
            .bind(org_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    exists
}

/// Seed real registry data (a branch + a registry_customers row) for `org_id` so
/// the tenant counts as "in real use" and removal must be refused. A customer is
/// the minimal guarded "real data" row and avoids registry_equipment's many
/// NOT NULL / CHECK columns. Runs as the owner with RLS off (so the WITH CHECK +
/// org-immutability constraints accept the inserts).
async fn seed_real_data(owner_pool: &PgPool, org_id: Uuid) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Region {}", &org_id.simple().to_string()[..8]))
            .bind(org_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind("Main")
    .bind(org_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3)")
        .bind(branch_id)
        .bind("Acme Logistics")
        .bind(org_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

/// Seed real operational data spanning the operational, financial, and messenger
/// domains for `org_id`. This is the data a FORCE removal must erase, and it
/// exercises the deep RESTRICT FK chains (work_orders → equipment/customer/site,
/// evidence → work_order, financial purchases → evidence, audit → actor + branch).
/// Runs as the owner with RLS off so the WITH CHECK + org-immutability + CHECK
/// constraints accept the inserts. Returns the work_order id.
async fn seed_force_data(owner_pool: &PgPool, org_id: Uuid) -> Uuid {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind("Force Region")
            .bind(org_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind("Force Main")
    .bind(org_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind("Force Mechanic")
    .bind(vec!["MECHANIC".to_owned()])
    .bind(org_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let customer_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(branch_id)
    .bind("Force Customer")
    .bind(org_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let site_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(branch_id)
    .bind(customer_id)
    .bind("Force Site")
    .bind(org_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let equipment_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment
            (branch_id, customer_id, site_id, equipment_no, manufacturer_code, kind_code,
             power_code, status, specification, ton_text, source_sheet, source_row, org_id)
        VALUES ($1, $2, $3, 'ABCDE-0001', 'M', 'K', 'P', '임대', 'spec', '2.5', 'sheet', 1, $4)
        RETURNING id
        "#,
    )
    .bind(branch_id)
    .bind(customer_id)
    .bind(site_id)
    .bind(org_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    // request_no must match ^[0-9]{8}-[0-9]{3}$ (e.g. 20260624-001). Derive a
    // valid, per-org-unique value from the org id's hex nibbles mapped to decimal
    // digits so two seeded orgs never collide on the request_no unique index.
    let digits: String = org_id
        .simple()
        .to_string()
        .chars()
        .map(|c| {
            let n = c.to_digit(16).unwrap_or(0) % 10;
            char::from_digit(n, 10).unwrap_or('0')
        })
        .take(11)
        .collect();
    let request_no = format!("{}-{}", &digits[..8], &digits[8..11]);
    let work_order_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO work_orders
            (request_no, branch_id, equipment_id, customer_id, site_id, requested_by,
             status, symptom, org_id)
        VALUES ($1, $2, $3, $4, $5, $6, 'RECEIVED', 'symptom', $7)
        RETURNING id
        "#,
    )
    .bind(&request_no)
    .bind(branch_id)
    .bind(equipment_id)
    .bind(customer_id)
    .bind(site_id)
    .bind(user_id)
    .bind(org_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let evidence_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO evidence_media
            (org_id, uploaded_by, work_order_id, stage, s3_key, content_type, size_bytes,
             worm_replica_status, processing_status)
        VALUES ($1, $2, $3, 'AFTER', $4, 'image/jpeg', 100, 'PENDING', 'READY')
        RETURNING id
        "#,
    )
    .bind(org_id)
    .bind(user_id)
    .bind(work_order_id)
    .bind(format!("k/{org_id}"))
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO financial_rental_quotes
            (branch_id, equipment_id, created_by, acquisition_value_won,
             current_residual_value_won, effective_residual_value_won,
             cumulative_repair_cost_won, depreciation_method, useful_life_months,
             residual_rate_bps, declining_balance_rate_bps, management_fee_rate_bps,
             profit_rate_bps, floor_negative_quote_residual, monthly_total_won,
             created_at, org_id)
        VALUES ($1, $2, $3, 1000, 100, 100, 0, 'STRAIGHT_LINE', 60,
                1000, 1000, 1000, 1000, true, 500, now(), $4)
        "#,
    )
    .bind(branch_id)
    .bind(equipment_id)
    .bind(user_id)
    .bind(org_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO financial_purchase_requests
            (branch_id, equipment_id, work_order_id, statement_evidence_id,
             vendor_name, amount_won, memo, status, requested_by,
             depreciation_method, useful_life_months, residual_rate_bps,
             declining_balance_rate_bps, management_fee_rate_bps, profit_rate_bps,
             floor_negative_quote_residual, executive_threshold_won, created_at, org_id)
        VALUES ($1, $2, $3, $4, 'Force Vendor', 1000, 'Force purchase',
                'STATEMENT_ATTACHED', $5, 'STRAIGHT_LINE', 60, 1000, 1000,
                1000, 1000, true, 10000, now(), $6)
        "#,
    )
    .bind(branch_id)
    .bind(equipment_id)
    .bind(work_order_id)
    .bind(evidence_id)
    .bind(user_id)
    .bind(org_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO messenger_threads
            (kind, visibility, branch_id, work_order_id, created_by, org_id)
        VALUES ('work_order', 'direct', $1, $2, $3, $4)
        "#,
    )
    .bind(branch_id)
    .bind(work_order_id)
    .bind(user_id)
    .bind(org_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    // An audit_events row carrying actor + branch_id, so the force-removal's
    // re-home must release those tenant FKs before the user/branch deletes.
    sqlx::query(
        r#"
        INSERT INTO audit_events
            (org_id, actor, branch_id, action, target_type, target_id,
             trace_id, span_id, occurred_at)
        VALUES ($1, $2, $3, 'work_order.create', 'work_orders', $4,
                '00000000000000000000000000000001', '0000000000000001', now())
        "#,
    )
    .bind(org_id)
    .bind(user_id)
    .bind(branch_id)
    .bind(work_order_id.to_string())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    work_order_id
}

// ---------------------------------------------------------------------------
// (a) Empty tenant: removed; shell + org gone; audit trail survives re-homed.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../db/migrations")]
async fn platform_admin_removes_empty_tenant_and_preserves_audit(owner_pool: PgPool) {
    let harness = Harness::new(&owner_pool).await;
    let admin = seed_platform_admin(&owner_pool).await;
    let platform_token = harness.token(admin, OrgId::platform(), true).await;
    let service = harness.service();

    let org_id = onboard(&service, &platform_token, "acme").await;

    // The onboarding shell + the create audit row exist.
    assert_eq!(count_in_org(&owner_pool, "users", org_id).await, 1);
    assert_eq!(
        count_in_org(&owner_pool, "auth_bootstrap_credentials", org_id).await,
        1
    );
    let audit_before = count_in_org(&owner_pool, "audit_events", org_id).await;
    assert!(audit_before >= 1, "onboarding writes an audit row");

    let (status, body) = delete_org(&service, &platform_token, org_id).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body:?}");

    // The org and every shell table are gone.
    assert!(
        !org_exists(&owner_pool, org_id).await,
        "org must be deleted"
    );
    for table in [
        "users",
        "user_branches",
        "branches",
        "regions",
        "auth_bootstrap_credentials",
    ] {
        assert_eq!(
            count_in_org(&owner_pool, table, org_id).await,
            0,
            "{table} rows must be gone after removal"
        );
    }

    // The tenant's audit rows are NOT destroyed — re-homed to the platform
    // sentinel (so the immutable trail survives the tenant).
    assert_eq!(
        count_in_org(&owner_pool, "audit_events", org_id).await,
        0,
        "no audit rows should still carry the removed org id"
    );
    let rehomed = count_in_org(&owner_pool, "audit_events", *OrgId::platform().as_uuid()).await;
    assert!(
        rehomed >= audit_before,
        "the tenant's audit rows must survive re-homed to the sentinel"
    );

    // The removal itself is audited as platform.tenant.remove by the operator.
    let remove_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'platform.tenant.remove' AND actor = $1",
    )
    .bind(*admin.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(remove_audits, 1, "removal must be audited exactly once");
}

// ---------------------------------------------------------------------------
// (b) Tenant WITH real data: refused 409; NOTHING deleted (tx rolled back).
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../db/migrations")]
async fn removal_of_tenant_with_data_is_refused_and_rolls_back(owner_pool: PgPool) {
    let harness = Harness::new(&owner_pool).await;
    let admin = seed_platform_admin(&owner_pool).await;
    let platform_token = harness.token(admin, OrgId::platform(), true).await;
    let service = harness.service();

    let org_id = onboard(&service, &platform_token, "inuse").await;
    seed_real_data(&owner_pool, org_id).await;

    let (status, body) = delete_org(&service, &platform_token, org_id).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body:?}");
    assert_eq!(
        body["error"]["code"], "tenant_has_data",
        "the 409 must carry the archive-instead guard code: {body:?}"
    );

    // Nothing was removed: org, shell, and the real data all remain.
    assert!(
        org_exists(&owner_pool, org_id).await,
        "org must still exist"
    );
    assert_eq!(count_in_org(&owner_pool, "users", org_id).await, 1);
    assert_eq!(
        count_in_org(&owner_pool, "registry_customers", org_id).await,
        1
    );
    // No removal audit row was written.
    let remove_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'platform.tenant.remove'",
    )
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(remove_audits, 0, "a refused removal must not be audited");
}

// ---------------------------------------------------------------------------
// (c) A non-platform (tenant) principal is rejected with 403.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../db/migrations")]
async fn tenant_token_cannot_remove_a_tenant(owner_pool: PgPool) {
    let harness = Harness::new(&owner_pool).await;
    let admin = seed_platform_admin(&owner_pool).await;
    let platform_token = harness.token(admin, OrgId::platform(), true).await;
    let service = harness.service();

    let org_id = onboard(&service, &platform_token, "victim").await;

    // A normal TENANT token (platform = false).
    let tenant_token = harness
        .token(
            console_platform_test_support::seed_org_and_super_admin(
                &owner_pool,
                *OrgId::knl().as_uuid(),
                "tier-denial",
            )
            .await,
            OrgId::knl(),
            false,
        )
        .await;
    let (status, _body) = delete_org(&service, &tenant_token, org_id).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a tenant token must never reach DELETE /api/platform/orgs/{{id}}"
    );

    // The tenant is untouched.
    assert!(
        org_exists(&owner_pool, org_id).await,
        "org must still exist"
    );
}

// ---------------------------------------------------------------------------
// (d) Cross-tenant: removing org A leaves org B's rows intact.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../db/migrations")]
async fn removing_one_tenant_does_not_touch_another(owner_pool: PgPool) {
    let harness = Harness::new(&owner_pool).await;
    let admin = seed_platform_admin(&owner_pool).await;
    let platform_token = harness.token(admin, OrgId::platform(), true).await;
    let service = harness.service();

    let org_a = onboard(&service, &platform_token, "alpha").await;
    let org_b = onboard(&service, &platform_token, "beta").await;

    let (status, body) = delete_org(&service, &platform_token, org_a).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body:?}");

    assert!(!org_exists(&owner_pool, org_a).await, "org A must be gone");
    assert!(org_exists(&owner_pool, org_b).await, "org B must survive");
    assert_eq!(
        count_in_org(&owner_pool, "users", org_b).await,
        1,
        "org B's shell user must be untouched"
    );
    assert_eq!(
        count_in_org(&owner_pool, "auth_bootstrap_credentials", org_b).await,
        1,
        "org B's bootstrap credential must be untouched"
    );
}

// ---------------------------------------------------------------------------
// (e) Audit-immutability is intact: a direct console_rt UPDATE still raises (the
// removal re-home is the ONLY path that may release audit references).
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../db/migrations")]
async fn direct_runtime_update_on_audit_events_is_still_rejected(owner_pool: PgPool) {
    let harness = Harness::new(&owner_pool).await;
    let admin = seed_platform_admin(&owner_pool).await;
    let platform_token = harness.token(admin, OrgId::platform(), true).await;
    let service = harness.service();

    let org_id = onboard(&service, &platform_token, "audited").await;

    // As console_rt, with the org armed, try to re-home an audit row WITHOUT the
    // sanctioned DEFINER GUC. The append-only trigger must reject it.
    let mut tx = harness.rt_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let result = sqlx::query("UPDATE audit_events SET actor = NULL WHERE org_id = $1")
        .bind(org_id)
        .execute(&mut *tx)
        .await;
    assert!(
        result.is_err(),
        "a direct console_rt UPDATE on audit_events must be rejected by the append-only trigger"
    );
    let _ = tx.rollback().await;
}

// ===========================================================================
#[sqlx::test(migrations = "../db/migrations")]
async fn force_removal_without_command_pool_fails_closed_and_preserves_target(owner_pool: PgPool) {
    let harness = Harness::new(&owner_pool).await;
    let admin = seed_platform_admin(&owner_pool).await;
    let token = harness.token(admin, OrgId::platform(), true).await;
    let configured = harness.service();
    let org = onboard(&configured, &token, "no-force-pool").await;
    archive_org(&configured, &token, org).await;
    let (status, body) =
        force_delete_org(&harness.service_without_force_command(), &token, org).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body:?}");
    assert_eq!(body["error"]["code"], "platform_force_command_unavailable");
    assert!(
        org_exists(&owner_pool, org).await,
        "missing command capability must not mutate target"
    );
}

// FORCE removal (DELETE /api/platform/orgs/{id}?delete_data=true).
// ===========================================================================

// ---------------------------------------------------------------------------
// (f) THE critical test: force-removing an ARCHIVED tenant WITH data erases ALL
//     of its rows across every touched table and the org itself; its audit trail
//     survives re-homed to the sentinel; and a SECOND tenant's data is COMPLETELY
//     untouched (tenant isolation — a force-delete must never cross orgs).
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../db/migrations")]
async fn force_removes_archived_tenant_with_data_and_isolates_other_tenant(owner_pool: PgPool) {
    let harness = Harness::new(&owner_pool).await;
    let admin = seed_platform_admin(&owner_pool).await;
    let platform_token = harness.token(admin, OrgId::platform(), true).await;
    let service = harness.service();

    // Org A (to be force-wiped) and org B (the KNL-like bystander).
    let org_a = onboard(&service, &platform_token, "doomed").await;
    let org_b = onboard(&service, &platform_token, "keeper").await;

    // Both tenants accumulate real operational data spanning several tables.
    let wo_a = seed_force_data(&owner_pool, org_a).await;
    let wo_b = seed_force_data(&owner_pool, org_b).await;
    assert_ne!(wo_a, wo_b);

    // Snapshot org B's footprint BEFORE the wipe so we can prove byte-for-byte
    // isolation afterward.
    let tables = [
        "users",
        "regions",
        "branches",
        "registry_customers",
        "registry_sites",
        "registry_equipment",
        "work_orders",
        "evidence_media",
        "audit_events",
        "auth_bootstrap_credentials",
    ];
    let mut b_before = std::collections::HashMap::new();
    for table in tables {
        b_before.insert(table, count_in_org(&owner_pool, table, org_b).await);
    }
    // A force-remove on an ACTIVE tenant must be REFUSED (the safety rail): A is
    // still ACTIVE here, so this proves we cannot force-wipe an unarchived tenant.
    let (status, body) = force_delete_org(&service, &platform_token, org_a).await;
    assert_eq!(status, StatusCode::CONFLICT, "active tenant: {body:?}");
    assert_eq!(
        body["error"]["code"], "tenant_active",
        "the 409 must tell the operator to archive first: {body:?}"
    );
    assert!(
        org_exists(&owner_pool, org_a).await,
        "the refused force-remove must change NOTHING"
    );
    assert_eq!(
        count_in_org(&owner_pool, "work_orders", org_a).await,
        1,
        "A's data must be intact after the refused force-remove"
    );

    // Archive A (the reversible mandatory first step), THEN force-remove it.
    archive_org(&service, &platform_token, org_a).await;
    // Snapshot the bounded receipt inputs at the actual pre-delete boundary.
    // Archiving appends a legitimate tenant audit event, so a snapshot taken
    // before the mandatory archive transition would be stale by construction.
    let mut a_before = std::collections::HashMap::new();
    for table in [
        "users",
        "registry_customers",
        "registry_sites",
        "registry_equipment",
        "work_orders",
        "financial_rental_quotes",
        "financial_purchase_requests",
        "messenger_threads",
        "evidence_media",
        "audit_events",
    ] {
        a_before.insert(table, count_in_org(&owner_pool, table, org_a).await);
    }
    let a_audit_before = count_in_org(&owner_pool, "audit_events", org_a).await;
    assert!(a_audit_before >= 1, "org A has audit rows to re-home");
    let sentinel_audit_before =
        count_in_org(&owner_pool, "audit_events", *OrgId::platform().as_uuid()).await;

    let (status, body) = force_delete_org(&service, &platform_token, org_a).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "force remove: {body:?}");

    // Org A is GONE: the org row and every touched table.
    assert!(
        !org_exists(&owner_pool, org_a).await,
        "org A must be force-removed"
    );
    for table in tables {
        assert_eq!(
            count_in_org(&owner_pool, table, org_a).await,
            0,
            "every {table} row for org A must be erased by the force-remove"
        );
    }

    // Org A's audit trail SURVIVES, re-homed to the platform sentinel.
    let sentinel_audit_after =
        count_in_org(&owner_pool, "audit_events", *OrgId::platform().as_uuid()).await;
    assert!(
        sentinel_audit_after >= sentinel_audit_before + a_audit_before,
        "org A's audit rows must survive re-homed to the sentinel"
    );

    // The force-removal itself is audited DISTINCTLY as platform.tenant.force_remove.
    let force_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'platform.tenant.force_remove' AND actor = $1",
    )
    .bind(*admin.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        force_audits, 1,
        "the force-removal must be audited exactly once, distinctly"
    );
    for key in [
        "users",
        "registry_customers",
        "registry_sites",
        "registry_equipment",
        "work_orders",
        "financial_rental_quotes",
        "financial_purchase_requests",
        "messenger_threads",
        "evidence_media",
        "audit_events",
    ] {
        let expected = a_before[key];
        let value: i64 = sqlx::query_scalar("SELECT (before_snap->'wiped'->>$1)::bigint FROM audit_events WHERE action = 'platform.tenant.force_remove' AND target_id = $2")
            .bind(key).bind(org_a.to_string()).fetch_one(&owner_pool).await.unwrap();
        assert_eq!(
            value, expected,
            "receipt key {key} must equal its pre-delete bounded count"
        );
    }

    // THE CRITICAL ASSERTION: org B is byte-for-byte untouched.
    assert!(
        org_exists(&owner_pool, org_b).await,
        "org B must survive a force-removal of org A"
    );
    for table in tables {
        assert_eq!(
            count_in_org(&owner_pool, table, org_b).await,
            b_before[table],
            "org B's {table} rows must be COMPLETELY untouched by org A's force-removal"
        );
    }
}

// ---------------------------------------------------------------------------
// (g) Force-remove of the platform sentinel is a no-op 404 (never wipe the
//     platform tier's own anchor row).
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../db/migrations")]
async fn force_remove_of_sentinel_is_not_found(owner_pool: PgPool) {
    let harness = Harness::new(&owner_pool).await;
    let admin = seed_platform_admin(&owner_pool).await;
    let platform_token = harness.token(admin, OrgId::platform(), true).await;
    let service = harness.service();

    let (status, _body) =
        force_delete_org(&service, &platform_token, *OrgId::platform().as_uuid()).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the platform sentinel must never be force-removable"
    );
    assert!(
        org_exists(&owner_pool, *OrgId::platform().as_uuid()).await,
        "the sentinel org must still exist"
    );
}

// ---------------------------------------------------------------------------
// (h) A non-platform (tenant) principal cannot reach the force path (403).
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../db/migrations")]
async fn tenant_token_cannot_force_remove_a_tenant(owner_pool: PgPool) {
    let harness = Harness::new(&owner_pool).await;
    let admin = seed_platform_admin(&owner_pool).await;
    let platform_token = harness.token(admin, OrgId::platform(), true).await;
    let service = harness.service();

    let org_id = onboard(&service, &platform_token, "fvictim").await;
    archive_org(&service, &platform_token, org_id).await;

    let tenant_token = harness
        .token(
            console_platform_test_support::seed_org_and_super_admin(
                &owner_pool,
                *OrgId::knl().as_uuid(),
                "tier-denial",
            )
            .await,
            OrgId::knl(),
            false,
        )
        .await;
    let (status, _body) = force_delete_org(&service, &tenant_token, org_id).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a tenant token must never reach the force-remove path"
    );
    assert!(
        org_exists(&owner_pool, org_id).await,
        "the tenant must be untouched after a rejected force-remove"
    );
}
