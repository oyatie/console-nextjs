#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use console_consulting_rest::{CONSULTING_ENGAGEMENTS_PATH, ConsultingRestState, router};
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_request_context::TrustedClientIp;
use console_platform_test_support::runtime_role_pool;
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const ISSUER: &str = "console-platform-auth";
const AUDIENCE: &str = "console-api";
const INBOUND_TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
const INBOUND_TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const INBOUND_SPAN_ID: &str = "00f067aa0ba902b7";
const CLIENT_IP: &str = "203.0.113.44";
const USER_AGENT: &str = "console-consulting-integration/1.0";
const DEVICE_ID: &str = "field-tablet-17";
const CONCURRENT_A_TRACEPARENT: &str = "00-11111111111111111111111111111111-1111111111111111-01";
const CONCURRENT_A_TRACE_ID: &str = "11111111111111111111111111111111";
const CONCURRENT_A_SPAN_ID: &str = "1111111111111111";
const CONCURRENT_A_IP: &str = "203.0.113.45";
const CONCURRENT_A_USER_AGENT: &str = "console-consulting-concurrent-a/1.0";
const CONCURRENT_A_DEVICE: &str = "field-tablet-a";
const CONCURRENT_B_TRACEPARENT: &str = "00-22222222222222222222222222222222-2222222222222222-01";
const CONCURRENT_B_TRACE_ID: &str = "22222222222222222222222222222222";
const CONCURRENT_B_SPAN_ID: &str = "2222222222222222";
const CONCURRENT_B_IP: &str = "203.0.113.46";
const CONCURRENT_B_USER_AGENT: &str = "console-consulting-concurrent-b/1.0";
const CONCURRENT_B_DEVICE: &str = "field-tablet-b";

#[derive(Clone, Copy)]
struct RequestMetadata {
    traceparent: &'static str,
    ip: &'static str,
    user_agent: &'static str,
    device: &'static str,
}

const DEFAULT_REQUEST_METADATA: RequestMetadata = RequestMetadata {
    traceparent: INBOUND_TRACEPARENT,
    ip: CLIENT_IP,
    user_agent: USER_AGENT,
    device: DEVICE_ID,
};

const CONCURRENT_A_REQUEST_METADATA: RequestMetadata = RequestMetadata {
    traceparent: CONCURRENT_A_TRACEPARENT,
    ip: CONCURRENT_A_IP,
    user_agent: CONCURRENT_A_USER_AGENT,
    device: CONCURRENT_A_DEVICE,
};

const CONCURRENT_B_REQUEST_METADATA: RequestMetadata = RequestMetadata {
    traceparent: CONCURRENT_B_TRACEPARENT,
    ip: CONCURRENT_B_IP,
    user_agent: CONCURRENT_B_USER_AGENT,
    device: CONCURRENT_B_DEVICE,
};

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct Harness {
    app: axum::Router,
    token: String,
    org: OrgId,
    customer_id: Uuid,
}

impl Harness {
    async fn new(owner_pool: &PgPool) -> Self {
        let keys = keys();
        let org = OrgId::new();
        let branch = seed_org_actor_customer(owner_pool, org).await;
        let actor = seed_actor(owner_pool, org, branch).await;
        let runtime_pool = runtime_role_pool(owner_pool).await;
        let verifier = JwtVerifier::from_es256_public_pem(
            JwtSettings {
                issuer: ISSUER.to_owned(),
                audience: AUDIENCE.to_owned(),
                access_token_ttl: Duration::minutes(15),
            },
            keys.public_pem.as_bytes(),
        )
        .unwrap();
        let token = bearer(&keys, actor, org, branch);
        let customer_id = sqlx::query_scalar(
            "SELECT id FROM registry_customers WHERE org_id = $1 ORDER BY created_at LIMIT 1",
        )
        .bind(*org.as_uuid())
        .fetch_one(owner_pool)
        .await
        .unwrap();

        Self {
            app: router(ConsultingRestState::new(runtime_pool, Some(verifier))),
            token,
            org,
            customer_id,
        }
    }

    fn body(&self, idempotency_key: &str) -> Value {
        json!({
            "customerId": self.customer_id,
            "customerDocumentId": null,
            "ontologyInstanceId": null,
            "title": "Traceable operating-model review",
            "idempotencyKey": idempotency_key,
        })
    }

    async fn create(&self, body: Value, metadata: RequestMetadata) -> (StatusCode, Value) {
        send_create(self.app.clone(), &self.token, body, metadata).await
    }
}

fn keys() -> Keys {
    let key = SigningKey::random(&mut OsRng);
    Keys {
        private_pem: key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
        public_pem: key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap(),
    }
}

fn bearer(keys: &Keys, user: UserId, org: OrgId, branch: BranchId) -> String {
    JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: ISSUER.to_owned(),
            audience: AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
    )
    .unwrap()
    .issue_access_token(AccessTokenInput {
        subject: user,
        org_id: org,
        roles: vec!["SUPER_ADMIN".to_owned()],
        branches: vec![branch],
        platform: false,
        view_as: false,
        read_only: false,
        display_name: None,
        feature_grants: Vec::new(),
        authz_subject_version: 0,
        authz_policy_version: 0,
        session_generation: 0,
        issued_at: OffsetDateTime::now_utc(),
    })
    .unwrap()
}

async fn send_create(
    service: axum::Router,
    token: &str,
    body: Value,
    metadata: RequestMetadata,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(CONSULTING_ENGAGEMENTS_PATH)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header("traceparent", metadata.traceparent)
        .header("x-forwarded-for", "198.51.100.250")
        .header(header::USER_AGENT, metadata.user_agent)
        .header("x-device-id", metadata.device)
        .body(Body::from(body.to_string()))
        .unwrap();
    request.extensions_mut().insert(TrustedClientIp::new(
        metadata.ip.parse().expect("trusted client IP"),
    ));
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&body).unwrap_or_else(|_| json!({})),
    )
}

async fn seed_org_actor_customer(owner_pool: &PgPool, org: OrgId) -> BranchId {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(*org.as_uuid())
        .bind(format!(
            "consulting-{}",
            &org.as_uuid().simple().to_string()[..12]
        ))
        .bind("Consulting audit integration")
        // rls-arming: ok integration fixture setup runs as the migration owner before runtime-role requests
        .execute(owner_pool)
        .await
        .unwrap();
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("consulting-audit-region-{}", Uuid::new_v4()))
            .bind(*org.as_uuid())
            // rls-arming: ok integration fixture setup runs as the migration owner before runtime-role requests
            .fetch_one(owner_pool)
            .await
            .unwrap();
    let branch = BranchId::from_uuid(
        sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(region)
        .bind(format!("consulting-audit-branch-{}", Uuid::new_v4()))
        .bind(*org.as_uuid())
        // rls-arming: ok integration fixture setup runs as the migration owner before runtime-role requests
        .fetch_one(owner_pool)
        .await
        .unwrap(),
    );
    sqlx::query("INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3)")
        .bind(*branch.as_uuid())
        .bind("Consulting audit customer")
        .bind(*org.as_uuid())
        // rls-arming: ok integration fixture setup runs as the migration owner before runtime-role requests
        .execute(owner_pool)
        .await
        .unwrap();
    branch
}

async fn seed_actor(owner_pool: &PgPool, org: OrgId, branch: BranchId) -> UserId {
    let actor = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*actor.as_uuid())
        .bind("Consulting audit actor")
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(*org.as_uuid())
        // rls-arming: ok integration fixture setup runs as the migration owner before runtime-role requests
        .execute(owner_pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*actor.as_uuid())
        .bind(*branch.as_uuid())
        .bind(*org.as_uuid())
        // rls-arming: ok integration fixture setup runs as the migration owner before runtime-role requests
        .execute(owner_pool)
        .await
        .unwrap();
    let role_id: Uuid = sqlx::query_scalar(
        "INSERT INTO policy_roles \
         (org_id, role_key, display_name, status, is_system, created_by, updated_by) \
         VALUES ($1, $2, $3, 'ACTIVE', false, $4, $4) RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(format!(
        "consulting_audit_{}",
        &actor.as_uuid().simple().to_string()[..8]
    ))
    .bind("Consulting audit test role")
    .bind(*actor.as_uuid())
    // rls-arming: ok integration fixture setup runs as the migration owner before runtime-role requests
    .fetch_one(owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO policy_role_permissions \
         (org_id, role_id, feature_key, permission_level) \
         VALUES ($1, $2, 'consulting_manage', 'allow')",
    )
    .bind(*org.as_uuid())
    .bind(role_id)
    // rls-arming: ok integration fixture setup runs as the migration owner before runtime-role requests
    .execute(owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_role_assignments \
         (org_id, user_id, role_id, assigned_by) VALUES ($1, $2, $3, $2)",
    )
    .bind(*org.as_uuid())
    .bind(*actor.as_uuid())
    .bind(role_id)
    // rls-arming: ok integration fixture setup runs as the migration owner before runtime-role requests
    .execute(owner_pool)
    .await
    .unwrap();
    actor
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn first_create_writes_one_org_bound_request_correlated_audit(owner_pool: PgPool) {
    let harness = Harness::new(&owner_pool).await;
    let (status, response) = harness
        .create(
            harness.body("consulting-audit-first"),
            DEFAULT_REQUEST_METADATA,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{response:?}");
    let engagement_id = response["id"].as_str().unwrap();

    let rows = sqlx::query(
        "SELECT action, org_id, trace_id, span_id, ip, user_agent, auth_method, device \
         FROM audit_events WHERE target_id = $1 ORDER BY occurred_at, id",
    )
    .bind(engagement_id)
    // rls-arming: ok integration verification reads committed state as the migration owner
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(
        row.try_get::<String, _>("action").unwrap(),
        "engagement.created"
    );
    assert_eq!(
        row.try_get::<Uuid, _>("org_id").unwrap(),
        *harness.org.as_uuid()
    );
    assert_eq!(
        row.try_get::<String, _>("trace_id").unwrap(),
        INBOUND_TRACE_ID
    );
    assert_eq!(
        row.try_get::<String, _>("span_id").unwrap(),
        INBOUND_SPAN_ID
    );
    assert_eq!(row.try_get::<String, _>("ip").unwrap(), CLIENT_IP);
    assert_eq!(row.try_get::<String, _>("user_agent").unwrap(), USER_AGENT);
    assert_eq!(row.try_get::<String, _>("auth_method").unwrap(), "bearer");
    assert_eq!(row.try_get::<String, _>("device").unwrap(), DEVICE_ID);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn sequential_and_concurrent_replay_leave_one_domain_history_and_audit(owner_pool: PgPool) {
    let harness = Harness::new(&owner_pool).await;
    let body = harness.body("consulting-audit-replay");
    let (first_status, first) = harness.create(body.clone(), DEFAULT_REQUEST_METADATA).await;
    assert_eq!(first_status, StatusCode::CREATED, "{first:?}");
    let (sequential_status, sequential) =
        harness.create(body.clone(), DEFAULT_REQUEST_METADATA).await;
    assert_eq!(sequential_status, StatusCode::CREATED, "{sequential:?}");
    assert_eq!(sequential["id"], first["id"]);

    let concurrent_a = harness.create(body.clone(), CONCURRENT_A_REQUEST_METADATA);
    let concurrent_b = harness.create(body, CONCURRENT_B_REQUEST_METADATA);
    let ((status_a, response_a), (status_b, response_b)) = tokio::join!(concurrent_a, concurrent_b);
    assert_eq!(status_a, StatusCode::CREATED, "{response_a:?}");
    assert_eq!(status_b, StatusCode::CREATED, "{response_b:?}");
    assert_eq!(response_a["id"], first["id"]);
    assert_eq!(response_b["id"], first["id"]);

    let engagement_id = first["id"].as_str().unwrap();
    let engagement_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM consulting_engagements WHERE org_id = $1 AND idempotency_key = $2",
    )
    .bind(*harness.org.as_uuid())
    .bind("consulting-audit-replay")
    // rls-arming: ok integration verification reads committed state as the migration owner
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    let history_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM consulting_engagement_history WHERE org_id = $1 AND engagement_id = $2",
    )
    .bind(*harness.org.as_uuid())
    .bind(Uuid::parse_str(engagement_id).unwrap())
    // rls-arming: ok integration verification reads committed state as the migration owner
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    let audit_rows = sqlx::query(
        "SELECT trace_id, span_id, ip, user_agent, auth_method, device \
         FROM audit_events \
         WHERE org_id = $1 AND target_id = $2 AND action = 'engagement.created'",
    )
    .bind(*harness.org.as_uuid())
    .bind(engagement_id)
    // rls-arming: ok integration verification reads committed state as the migration owner
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(engagement_count, 1);
    assert_eq!(history_count, 1);
    assert_eq!(audit_rows.len(), 1);
    assert_request_metadata(
        &audit_rows[0],
        INBOUND_TRACE_ID,
        INBOUND_SPAN_ID,
        DEFAULT_REQUEST_METADATA,
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn concurrent_creates_retain_independent_request_contexts(owner_pool: PgPool) {
    let harness = Harness::new(&owner_pool).await;
    let create_a = harness.create(
        harness.body("consulting-audit-concurrent-a"),
        CONCURRENT_A_REQUEST_METADATA,
    );
    let create_b = harness.create(
        harness.body("consulting-audit-concurrent-b"),
        CONCURRENT_B_REQUEST_METADATA,
    );
    let ((status_a, response_a), (status_b, response_b)) = tokio::join!(create_a, create_b);
    assert_eq!(status_a, StatusCode::CREATED, "{response_a:?}");
    assert_eq!(status_b, StatusCode::CREATED, "{response_b:?}");

    let target_a = response_a["id"].as_str().unwrap();
    let target_b = response_b["id"].as_str().unwrap();
    let rows = sqlx::query(
        "SELECT target_id, trace_id, span_id, ip, user_agent, auth_method, device \
         FROM audit_events \
         WHERE org_id = $1 AND action = 'engagement.created' AND target_id IN ($2, $3)",
    )
    .bind(*harness.org.as_uuid())
    .bind(target_a)
    .bind(target_b)
    // rls-arming: ok integration verification reads committed state as the migration owner
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);

    let row_a = rows
        .iter()
        .find(|row| row.try_get::<String, _>("target_id").unwrap() == target_a)
        .expect("request A audit");
    assert_request_metadata(
        row_a,
        CONCURRENT_A_TRACE_ID,
        CONCURRENT_A_SPAN_ID,
        CONCURRENT_A_REQUEST_METADATA,
    );

    let row_b = rows
        .iter()
        .find(|row| row.try_get::<String, _>("target_id").unwrap() == target_b)
        .expect("request B audit");
    assert_request_metadata(
        row_b,
        CONCURRENT_B_TRACE_ID,
        CONCURRENT_B_SPAN_ID,
        CONCURRENT_B_REQUEST_METADATA,
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn audit_insert_failure_rolls_back_engagement_and_history(owner_pool: PgPool) {
    let harness = Harness::new(&owner_pool).await;
    sqlx::query(
        r#"
        CREATE FUNCTION reject_consulting_audit_for_test()
        RETURNS TRIGGER LANGUAGE plpgsql AS $$
        BEGIN
          IF NEW.action = 'engagement.created' THEN
            RAISE EXCEPTION 'forced consulting audit failure';
          END IF;
          RETURN NEW;
        END;
        $$
        "#,
    )
    // rls-arming: ok test-only failure injector is installed by the migration owner
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_consulting_audit_for_test \
         BEFORE INSERT ON audit_events \
         FOR EACH ROW EXECUTE FUNCTION reject_consulting_audit_for_test()",
    )
    // rls-arming: ok test-only failure injector is installed by the migration owner
    .execute(&owner_pool)
    .await
    .unwrap();

    let (status, response) = harness
        .create(
            harness.body("consulting-audit-rollback"),
            DEFAULT_REQUEST_METADATA,
        )
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{response:?}");

    let engagement_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM consulting_engagements WHERE org_id = $1 AND idempotency_key = $2",
    )
    .bind(*harness.org.as_uuid())
    .bind("consulting-audit-rollback")
    // rls-arming: ok integration verification reads committed state as the migration owner
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    let history_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM consulting_engagement_history WHERE org_id = $1")
            .bind(*harness.org.as_uuid())
            // rls-arming: ok integration verification reads committed state as the migration owner
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    let audit_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE org_id = $1")
            .bind(*harness.org.as_uuid())
            // rls-arming: ok integration verification reads committed state as the migration owner
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(engagement_count, 0);
    assert_eq!(history_count, 0);
    assert_eq!(audit_count, 0);
}

fn assert_request_metadata(
    row: &sqlx::postgres::PgRow,
    trace_id: &str,
    span_id: &str,
    metadata: RequestMetadata,
) {
    assert_eq!(row.try_get::<String, _>("trace_id").unwrap(), trace_id);
    assert_eq!(row.try_get::<String, _>("span_id").unwrap(), span_id);
    assert_eq!(row.try_get::<String, _>("ip").unwrap(), metadata.ip);
    assert_eq!(
        row.try_get::<String, _>("user_agent").unwrap(),
        metadata.user_agent
    );
    assert_eq!(row.try_get::<String, _>("auth_method").unwrap(), "bearer");
    assert_eq!(row.try_get::<String, _>("device").unwrap(), metadata.device);
}
