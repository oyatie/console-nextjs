#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Contract tests for `GET /api/v1/workflow-runs/for-object`.
//!
//! These tests intentionally drive the composition-root router on a non-owner
//! `console_rt` pool.  The serial integrator enables them by mounting
//! `workflow_object_context::router`; they protect the narrow Wave-1 contract
//! rather than asserting a universal ontology projection.

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

const ISSUER: &str = "console-platform-auth";
const AUDIENCE: &str = "console-api";
const PATH: &str = "/api/v1/workflow-runs/for-object";

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn workflow_object_context_is_exact_pair_scoped_and_read_only(pool: PgPool) {
    let keys = keys();
    let org = OrgId::knl();
    let branch_a = seed_branch(&pool, org, "A").await;
    let branch_b = seed_branch(&pool, org, "B").await;
    let initiator = UserId::new();
    let same_branch_stranger = UserId::new();
    let unrelated_same_branch_user = UserId::new();
    let cross_branch_user = UserId::new();
    seed_user(&pool, org, initiator, "ADMIN", branch_a).await;
    seed_user(&pool, org, same_branch_stranger, "ADMIN", branch_a).await;
    seed_user(&pool, org, unrelated_same_branch_user, "ADMIN", branch_a).await;
    seed_user(&pool, org, cross_branch_user, "ADMIN", branch_b).await;
    let other_org = OrgId::from_uuid(Uuid::from_u128(0x2));
    seed_org(&pool, other_org).await;
    let other_branch = seed_branch(&pool, other_org, "other").await;
    let other_org_user = UserId::new();
    seed_user(&pool, other_org, other_org_user, "ADMIN", other_branch).await;

    let definition = seed_definition(&pool, org).await;
    let first_subject = seed_ticket(&pool, org, branch_a, initiator, "first").await;
    let empty_subject = seed_ticket(&pool, org, branch_a, initiator, "empty").await;
    let other_subject = seed_ticket(&pool, org, branch_a, initiator, "other").await;
    let null_branch_subject = seed_untriaged_ticket(&pool, org, "untriaged").await;
    let work_order = seed_work_order(&pool, org, branch_a, initiator, "20260722-001").await;
    let run_a = seed_run(&pool, org, definition, initiator, first_subject, "a", 1).await;
    let run_b = seed_run(&pool, org, definition, initiator, first_subject, "b", 2).await;
    let invisible_run = seed_run(
        &pool,
        org,
        definition,
        same_branch_stranger,
        first_subject,
        "invisible",
        3,
    )
    .await;
    let work_order_run = seed_run_for_kind(
        &pool,
        org,
        definition,
        initiator,
        "work_order",
        work_order,
        "work-order",
        4,
    )
    .await;
    let mutation_before = mutation_snapshot(&pool).await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let initiator_token = bearer(&keys, org, initiator, "ADMIN");

    // Stable `(updated_at DESC, run_id DESC)` page with opaque UUID cursor.
    let first = get(
        service.clone(),
        &format!("{PATH}?object_type=support_ticket&object_id={first_subject}&limit=1"),
        &initiator_token,
    )
    .await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.json);
    assert_eq!(first.json["subject"]["object_type"], "support_ticket");
    assert_eq!(first.json["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(first.json["items"][0]["run_id"], run_a.to_string());
    assert_eq!(
        first.json["items"][0]["detail_target"]["kind"],
        "workflow_run_detail"
    );
    let before = first.json["next_before"].as_str().unwrap();
    let second = get(
        service.clone(),
        &format!(
            "{PATH}?object_type=support_ticket&object_id={first_subject}&limit=1&before={before}"
        ),
        &initiator_token,
    )
    .await;
    assert_eq!(second.status, StatusCode::OK, "{:?}", second.json);
    assert_eq!(second.json["items"][0]["run_id"], run_b.to_string());

    let work_order_context = get(
        service.clone(),
        &format!("{PATH}?object_type=work_order&object_id={work_order}"),
        &initiator_token,
    )
    .await;
    assert_eq!(
        work_order_context.status,
        StatusCode::OK,
        "{:?}",
        work_order_context.json
    );
    assert_eq!(
        work_order_context.json["items"][0]["run_id"],
        work_order_run.to_string()
    );

    // A caller who can view the subject but cannot view either run gets an
    // empty page.  This proves subject visibility is ANDed with run visibility.
    let no_run_visibility = get(
        service.clone(),
        &format!("{PATH}?object_type=support_ticket&object_id={first_subject}"),
        &bearer(&keys, org, unrelated_same_branch_user, "ADMIN"),
    )
    .await;
    assert_eq!(
        no_run_visibility.status,
        StatusCode::OK,
        "{:?}",
        no_run_visibility.json
    );
    assert_eq!(
        no_run_visibility.json["items"].as_array().map(Vec::len),
        Some(0)
    );

    // A cross-branch subject is indistinguishable from an absent subject.
    let cross_branch = get(
        service.clone(),
        &format!("{PATH}?object_type=support_ticket&object_id={first_subject}"),
        &bearer(&keys, org, cross_branch_user, "ADMIN"),
    )
    .await;
    assert_eq!(
        cross_branch.status,
        StatusCode::NOT_FOUND,
        "{:?}",
        cross_branch.json
    );

    let untriaged = get(
        service.clone(),
        &format!("{PATH}?object_type=support_ticket&object_id={null_branch_subject}"),
        &initiator_token,
    )
    .await;
    assert_eq!(
        untriaged.status,
        StatusCode::NOT_FOUND,
        "{:?}",
        untriaged.json
    );

    // Org RLS executes inside the same read boundary as subject/cursor lookup.
    let cross_org = get(
        service.clone(),
        &format!("{PATH}?object_type=support_ticket&object_id={first_subject}"),
        &bearer(&keys, other_org, other_org_user, "ADMIN"),
    )
    .await;
    assert_eq!(
        cross_org.status,
        StatusCode::NOT_FOUND,
        "{:?}",
        cross_org.json
    );

    // A well-formed cursor from another exact subject must not reveal the run.
    let pair_mismatch = get(
        service.clone(),
        &format!("{PATH}?object_type=support_ticket&object_id={other_subject}&before={run_a}"),
        &initiator_token,
    )
    .await;
    assert_eq!(pair_mismatch.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(pair_mismatch.json["error"]["code"], "invalid_cursor");

    // The cursor belongs to the exact pair but its run is invisible to this
    // caller.  It is still the same non-leaking invalid_cursor response.
    let invisible_cursor = get(
        service.clone(),
        &format!(
            "{PATH}?object_type=support_ticket&object_id={first_subject}&before={invisible_run}"
        ),
        &initiator_token,
    )
    .await;
    assert_eq!(invisible_cursor.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(invisible_cursor.json["error"]["code"], "invalid_cursor");

    let unknown_cursor = get(
        service.clone(),
        &format!(
            "{PATH}?object_type=support_ticket&object_id={first_subject}&before={}",
            Uuid::new_v4()
        ),
        &initiator_token,
    )
    .await;
    assert_eq!(unknown_cursor.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(unknown_cursor.json["error"]["code"], "invalid_cursor");

    // Subject authorization is deliberately evaluated before cursor lookup.
    // A cross-branch subject paired with any UUID remains one anti-oracle 404.
    let invisible_subject_cursor = get(
        service.clone(),
        &format!("{PATH}?object_type=support_ticket&object_id={first_subject}&before={run_a}"),
        &bearer(&keys, org, cross_branch_user, "ADMIN"),
    )
    .await;
    assert_eq!(invisible_subject_cursor.status, StatusCode::NOT_FOUND);

    let malformed = get(
        service.clone(),
        &format!("{PATH}?object_type=dispatch&object_id=not-a-uuid"),
        &initiator_token,
    )
    .await;
    assert_eq!(malformed.status, StatusCode::UNPROCESSABLE_ENTITY);

    // An authorized, valid subject with no history is a truthful empty page;
    // it must not fabricate action/history/audit fields.
    let empty = get(
        service,
        &format!("{PATH}?object_type=support_ticket&object_id={empty_subject}"),
        &initiator_token,
    )
    .await;
    assert_eq!(empty.status, StatusCode::OK, "{:?}", empty.json);
    assert_eq!(empty.json["items"].as_array().map(Vec::len), Some(0));
    assert!(empty.json.get("history").is_none());
    assert!(empty.json.get("actions").is_none());
    assert!(empty.json.get("audit").is_none());
    assert_eq!(mutation_snapshot(&pool).await, mutation_before);
}

#[derive(Debug, Eq, PartialEq)]
struct MutationSnapshot {
    workflow_runs: i64,
    workflow_waiting_tasks: i64,
    notifications: i64,
    workflow_outbox_events: i64,
    audit_events: i64,
    object_links: i64,
    support_tickets: i64,
    work_orders: i64,
}

async fn mutation_snapshot(pool: &PgPool) -> MutationSnapshot {
    MutationSnapshot {
        workflow_runs: count(pool, "workflow_runs").await,
        workflow_waiting_tasks: count(pool, "workflow_waiting_tasks").await,
        notifications: count(pool, "notifications").await,
        workflow_outbox_events: count(pool, "workflow_outbox_events").await,
        audit_events: count(pool, "audit_events").await,
        object_links: count(pool, "object_links").await,
        support_tickets: count(pool, "support_tickets").await,
        work_orders: count(pool, "work_orders").await,
    }
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    let sql = match table {
        "workflow_runs" => "SELECT count(*) FROM workflow_runs",
        "workflow_waiting_tasks" => "SELECT count(*) FROM workflow_waiting_tasks",
        "notifications" => "SELECT count(*) FROM notifications",
        "workflow_outbox_events" => "SELECT count(*) FROM workflow_outbox_events",
        "audit_events" => "SELECT count(*) FROM audit_events",
        "object_links" => "SELECT count(*) FROM object_links",
        "support_tickets" => "SELECT count(*) FROM support_tickets",
        "work_orders" => "SELECT count(*) FROM work_orders",
        _ => panic!("test count table must be an approved literal: {table}"),
    };
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

async fn seed_branch(pool: &PgPool, org: OrgId, tag: &str) -> BranchId {
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("workflow-context-{tag}"))
            .bind(*org.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    BranchId::from_uuid(
        sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(region)
        .bind(format!("workflow-context-{tag}"))
        .bind(*org.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}

async fn seed_org(pool: &PgPool, org: OrgId) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(*org.as_uuid())
    // organizations_slug_check caps slugs at 40 characters. The compact UUID
    // keeps this fixture unique without relying on another test to mask an
    // invalid insert through ON CONFLICT.
    .bind(format!("wfctx-{}", org.as_uuid().simple()))
    .bind("Workflow Context Other Org")
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_user(pool: &PgPool, org: OrgId, user: UserId, role: &str, branch: BranchId) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user.as_uuid())
        .bind(format!("workflow-context-{role}-{user}"))
        .bind(vec![role])
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user.as_uuid())
        .bind(*branch.as_uuid())
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_definition(pool: &PgPool, org: OrgId) -> Uuid {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_definitions \
         (org_id, workflow_key, display_name, object_type, status, latest_version, active_version) \
         VALUES ($1, $2, 'Workflow context', 'support_ticket', 'ACTIVE', 1, 1) RETURNING id",
    )
    .bind(*org.as_uuid())
    // workflow_key is a dotted identifier, not an arbitrary display slug.
    .bind(format!("workflow.context_{}", Uuid::new_v4().simple()))
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_definition_versions \
         (org_id, definition_id, version, status, definition, required_approval_line, required_payment_line) \
         VALUES ($1, $2, 1, 'PUBLISHED', '{}'::jsonb, FALSE, FALSE)",
    )
    .bind(*org.as_uuid())
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn seed_ticket(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    requester: UserId,
    tag: &str,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO support_tickets \
         (branch_id, origin, category, priority, status, title, body, requester_user_id, org_id) \
         VALUES ($1, 'INTERNAL', 'OTHER', 'LOW', 'OPEN', $2, 'body', $3, $4) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(format!("workflow context {tag}"))
    .bind(*requester.as_uuid())
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_untriaged_ticket(pool: &PgPool, org: OrgId, tag: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO support_tickets \
         (branch_id, origin, category, priority, status, title, body, requester_name, requester_contact, org_id) \
         VALUES (NULL, 'CUSTOMER', 'OTHER', 'LOW', 'OPEN', $1, 'body', 'customer', 'customer@example.test', $2) \
         RETURNING id",
    )
    .bind(format!("workflow context {tag}"))
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_work_order(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    requester: UserId,
    request_no: &str,
) -> Uuid {
    let customer: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, 'Workflow Context Customer', $2) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, 'Workflow Context Site', $3) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(customer)
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let equipment: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_equipment \
         (branch_id, customer_id, site_id, equipment_no, management_no, manufacturer_code, kind_code, power_code, status, specification, ton_text, model, source_sheet, source_row, org_id) \
         VALUES ($1, $2, $3, 'CTX01-0001', 'CTX-MGMT-001', 'A', 'B', 'C', '임대', 'spec', '2.5', 'model', 'test', 1, $4) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(customer)
    .bind(site)
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO work_orders \
         (request_no, branch_id, equipment_id, customer_id, site_id, requested_by, status, priority, symptom, org_id) \
         VALUES ($1, $2, $3, $4, $5, $6, 'RECEIVED', 'UNSET', 'workflow context', $7) RETURNING id",
    )
    .bind(request_no)
    .bind(*branch.as_uuid())
    .bind(equipment)
    .bind(customer)
    .bind(site)
    .bind(*requester.as_uuid())
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_run(
    pool: &PgPool,
    org: OrgId,
    definition: Uuid,
    actor: UserId,
    subject: Uuid,
    tag: &str,
    seconds_ago: i64,
) -> Uuid {
    seed_run_for_kind(
        pool,
        org,
        definition,
        actor,
        "support_ticket",
        subject,
        tag,
        seconds_ago,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn seed_run_for_kind(
    pool: &PgPool,
    org: OrgId,
    definition: Uuid,
    actor: UserId,
    object_type: &str,
    subject: Uuid,
    tag: &str,
    seconds_ago: i64,
) -> Uuid {
    let run = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_runs \
         (id, org_id, definition_id, definition_version, status, trigger_type, object_type, object_id, \
          idempotency_key, correlation_id, initiated_by, started_at, updated_at) \
         VALUES ($1, $2, $3, 1, 'WAITING', 'MANUAL', $4, $5, $6, $7, $8, \
                 now() - ($9::text || ' seconds')::interval, now() - ($9::text || ' seconds')::interval)",
    )
    .bind(run)
    .bind(*org.as_uuid())
    .bind(definition)
    .bind(object_type)
    .bind(subject)
    .bind(format!("workflow-context-idem-{tag}-{run}"))
    .bind(format!("workflow-context-corr-{tag}-{run}"))
    .bind(*actor.as_uuid())
    .bind(seconds_ago.to_string())
    .execute(pool)
    .await
    .unwrap();
    run
}

async fn get(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    let response = service
        .oneshot(
            Request::builder()
                .uri(uri)
                .method("GET")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    JsonResponse {
        status,
        json: serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({})),
    }
}

fn keys() -> Keys {
    let signing = SigningKey::random(&mut OsRng);
    Keys {
        private_pem: signing.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
        public_pem: signing
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap(),
    }
}

fn bearer(keys: &Keys, org: OrgId, user: UserId, role: &str) -> String {
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
        roles: vec![role.to_owned()],
        branches: Vec::new(),
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

async fn runtime_role_pool(owner: &PgPool) -> PgPool {
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(owner.connect_options().as_ref().clone())
        .await
        .unwrap()
}

fn app_state(pool: PgPool, public_key: String) -> Result<AppState, console_app::AppError> {
    AppState::new(
        AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
            ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
            ("CONSOLE_JWT_ISSUER", ISSUER.to_owned()),
            ("CONSOLE_JWT_AUDIENCE", AUDIENCE.to_owned()),
            ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key),
        ])?,
        DatabaseDependency::Postgres(pool),
    )
}
