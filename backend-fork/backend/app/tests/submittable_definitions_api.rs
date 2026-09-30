#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Submittable-templates catalog (GET /api/v1/workflow-studio/submittable-definitions).
//!
//! Runs the REAL router on a genuine non-owner `console_rt` pool. Proves the 기안
//! gallery source is all-employee AND deny-by-omission on start authority:
//! - a member sees ACTIVE self-service definitions (no affordance gap);
//! - DRAFT / PAUSED definitions are never listed;
//! - a definition carrying a `start_policy` the caller cannot pass is omitted for
//!   that caller but visible to a privileged one (mirrors the start endpoint —
//!   the catalog never advertises a definition the caller would 403 starting);
//! - the entry-node `required_policy` fallback is treated as start authority, and
//!   malformed/no-entry graphs are fail-closed omitted;
//! - a principal without Login is refused (403).

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const PATH: &str = "/api/v1/workflow-studio/submittable-definitions";

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn member_sees_active_self_service_but_not_draft_or_paused(pool: PgPool) {
    let keys = Keys::new();
    let branch = seed_branch(&pool).await;
    let member = UserId::new();
    seed_user(&pool, member, "MEMBER", branch).await;

    let active = seed_definition(&pool, "appr.selfservice", "ACTIVE", None).await;
    let draft = seed_definition(&pool, "appr.draft", "DRAFT", None).await;
    let paused = seed_definition(&pool, "appr.paused", "PAUSED", None).await;

    let token = keys.token(member, &["MEMBER"], branch);
    let (status, body) = list(&pool, &keys, &token).await;
    assert_eq!(status, StatusCode::OK, "member is admitted: {body:?}");

    let ids = ids(&body);
    assert!(
        ids.contains(&active.to_string()),
        "ACTIVE self-service definition must be listed: {body:?}"
    );
    assert!(
        !ids.contains(&draft.to_string()),
        "DRAFT definition must never be listed: {body:?}"
    );
    assert!(
        !ids.contains(&paused.to_string()),
        "PAUSED definition must never be listed: {body:?}"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn definition_with_start_policy_is_deny_by_omission(pool: PgPool) {
    let keys = Keys::new();
    let branch = seed_branch(&pool).await;
    let member = UserId::new();
    seed_user(&pool, member, "MEMBER", branch).await;
    let admin = UserId::new();
    seed_user(&pool, admin, "ADMIN", branch).await;

    // ACTIVE, but gated by a start_policy only a completion-review authority
    // holds (ADMIN yes, MEMBER no) — the exact guard the start endpoint enforces.
    let restricted =
        seed_definition(&pool, "ops.restricted", "ACTIVE", Some("completion_review")).await;
    // Control: a self-service ACTIVE definition both callers can start.
    let open = seed_definition(&pool, "appr.open", "ACTIVE", None).await;

    let member_token = keys.token(member, &["MEMBER"], branch);
    let (_, member_body) = list(&pool, &keys, &member_token).await;
    let member_ids = ids(&member_body);
    assert!(
        !member_ids.contains(&restricted.to_string()),
        "MEMBER must not see a definition they cannot start: {member_body:?}"
    );
    assert!(
        member_ids.contains(&open.to_string()),
        "MEMBER still sees the self-service definition: {member_body:?}"
    );

    let admin_token = keys.token(admin, &["ADMIN"], branch);
    let (_, admin_body) = list(&pool, &keys, &admin_token).await;
    let admin_ids = ids(&admin_body);
    assert!(
        admin_ids.contains(&restricted.to_string()),
        "ADMIN (completion_review) sees the gated definition: {admin_body:?}"
    );
    assert!(
        admin_ids.contains(&open.to_string()),
        "ADMIN also sees the self-service definition: {admin_body:?}"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn entry_node_required_policy_and_no_entry_graph_are_deny_by_omission(pool: PgPool) {
    let keys = Keys::new();
    let branch = seed_branch(&pool).await;
    let member = UserId::new();
    seed_user(&pool, member, "MEMBER", branch).await;
    let admin = UserId::new();
    seed_user(&pool, admin, "ADMIN", branch).await;

    // No top-level start_policy: the entry node itself is a human_task whose
    // required_policy is the fallback start authority.
    let entry_gated =
        seed_entry_policy_definition(&pool, "ops.entry_gated", "completion_review").await;
    // A graph with no entry node (cycle) parses but is not startable; the catalog
    // must fail closed and omit it for every caller.
    let no_entry = seed_no_entry_definition(&pool, "ops.no_entry").await;

    let member_token = keys.token(member, &["MEMBER"], branch);
    let (_, member_body) = list(&pool, &keys, &member_token).await;
    let member_ids = ids(&member_body);
    assert!(
        !member_ids.contains(&entry_gated.to_string()),
        "MEMBER must not see an entry-policy-gated definition: {member_body:?}"
    );
    assert!(
        !member_ids.contains(&no_entry.to_string()),
        "no-entry graph must be omitted for MEMBER: {member_body:?}"
    );

    let admin_token = keys.token(admin, &["ADMIN"], branch);
    let (_, admin_body) = list(&pool, &keys, &admin_token).await;
    let admin_ids = ids(&admin_body);
    assert!(
        admin_ids.contains(&entry_gated.to_string()),
        "ADMIN (completion_review) sees the entry-policy-gated definition: {admin_body:?}"
    );
    assert!(
        !admin_ids.contains(&no_entry.to_string()),
        "no-entry graph must be omitted even for ADMIN: {admin_body:?}"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn non_member_is_refused(pool: PgPool) {
    let keys = Keys::new();
    let branch = seed_branch(&pool).await;
    let nobody = UserId::new();
    // A token with no roles grants no Login capability -> 403 before any listing.
    let token = keys.token(nobody, &[], branch);
    let (status, _) = list(&pool, &keys, &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn ids(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|i| i["id"].as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

async fn list(pool: &PgPool, keys: &Keys, token: &str) -> (StatusCode, Value) {
    let service = build_router(app_state(
        runtime_role_pool(pool).await,
        keys.public_pem.clone(),
    ));
    let response = service
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(PATH)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, json)
}

async fn seed_branch(pool: &PgPool) -> BranchId {
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind("Submittable Region")
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind("Submittable Branch")
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(pool: &PgPool, user_id: UserId, role: &str, branch: BranchId) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("Sub {role} {}", Uuid::new_v4()))
        .bind(Vec::from([role]))
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

/// Seed a definition + its (published) active version. The entry node is an
/// `object_gate` (self-service), matching the shipped approval-template shape.
/// `start_policy`, when set, gates who may start it.
async fn seed_definition(
    pool: &PgPool,
    workflow_key: &str,
    status: &str,
    start_policy: Option<&str>,
) -> Uuid {
    let org = OrgId::knl();
    let mut definition = json!({
        "schema_version": "wf.exec.v1",
        "workflow_key": workflow_key,
        "nodes": [
            { "node_key": "submit", "node_type": "object_gate", "title": "Submit" },
            { "node_key": "review.hr", "node_type": "human_task", "title": "HR review",
              "assignee_role_key": "hr_reviewer", "required_policy": "approval_review" }
        ],
        "edges": [ { "from": "submit", "to": "review.hr" } ]
    });
    if let Some(policy) = start_policy {
        definition["start_policy"] = json!(policy);
    }
    // ACTIVE rows carry active_version = 1; DRAFT/PAUSED leave active_version NULL
    // so the catalog's `status = 'ACTIVE' AND active_version IS NOT NULL` filter
    // excludes them.
    let active_version = if status == "ACTIVE" { Some(1) } else { None };
    let definition_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_definitions \
             (org_id, workflow_key, display_name, object_type, status, latest_version, active_version) \
         VALUES ($1, $2, $3, 'approval_document', $4, 1, $5) RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(workflow_key)
    .bind(format!("Template {workflow_key}"))
    .bind(status)
    .bind(active_version)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_definition_versions \
             (org_id, definition_id, version, status, definition, required_approval_line, required_payment_line) \
         VALUES ($1, $2, 1, 'PUBLISHED', $3, TRUE, FALSE)",
    )
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(definition)
    .execute(pool)
    .await
    .unwrap();
    definition_id
}

async fn seed_entry_policy_definition(
    pool: &PgPool,
    workflow_key: &str,
    required_policy: &str,
) -> Uuid {
    let definition = json!({
        "schema_version": "wf.exec.v1",
        "workflow_key": workflow_key,
        "nodes": [
            { "node_key": "entry.review", "node_type": "human_task", "title": "Entry review",
              "assignee_role_key": "entry_reviewer", "required_policy": required_policy }
        ],
        "edges": []
    });
    seed_active_definition_with_graph(pool, workflow_key, definition).await
}

async fn seed_no_entry_definition(pool: &PgPool, workflow_key: &str) -> Uuid {
    let definition = json!({
        "schema_version": "wf.exec.v1",
        "workflow_key": workflow_key,
        "nodes": [
            { "node_key": "a", "node_type": "object_gate", "title": "A" },
            { "node_key": "b", "node_type": "object_gate", "title": "B" }
        ],
        "edges": [
            { "from": "a", "to": "b" },
            { "from": "b", "to": "a" }
        ]
    });
    seed_active_definition_with_graph(pool, workflow_key, definition).await
}

async fn seed_active_definition_with_graph(
    pool: &PgPool,
    workflow_key: &str,
    definition: Value,
) -> Uuid {
    let org = OrgId::knl();
    let definition_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_definitions \
             (org_id, workflow_key, display_name, object_type, status, latest_version, active_version) \
         VALUES ($1, $2, $3, 'approval_document', 'ACTIVE', 1, 1) RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(workflow_key)
    .bind(format!("Template {workflow_key}"))
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_definition_versions \
             (org_id, definition_id, version, status, definition, required_approval_line, required_payment_line) \
         VALUES ($1, $2, 1, 'PUBLISHED', $3, TRUE, FALSE)",
    )
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(definition)
    .execute(pool)
    .await
    .unwrap();
    definition_id
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

fn app_state(pool: PgPool, public_key_pem: String) -> AppState {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key_pem),
    ])
    .unwrap();
    AppState::new(config, DatabaseDependency::Postgres(pool)).unwrap()
}

struct Keys {
    private_pem: String,
    public_pem: String,
}

impl Keys {
    fn new() -> Self {
        let signing_key = SigningKey::random(&mut OsRng);
        Self {
            private_pem: signing_key
                .to_pkcs8_pem(LineEnding::LF)
                .unwrap()
                .to_string(),
            public_pem: signing_key
                .verifying_key()
                .to_public_key_pem(LineEnding::LF)
                .unwrap(),
        }
    }

    fn token(&self, user_id: UserId, roles: &[&str], branch: BranchId) -> String {
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
        issuer
            .issue_access_token(AccessTokenInput {
                subject: user_id,
                org_id: OrgId::knl(),
                roles: roles.iter().map(|r| (*r).to_owned()).collect(),
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
}
