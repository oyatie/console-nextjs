#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct FinalizeFixture {
    task_id: Uuid,
    run_id: Uuid,
    branch_id: BranchId,
    author_id: UserId,
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn delegated_finalize_endpoint_requires_reason_and_authorization(pool: PgPool) {
    let keys = keys();
    let fixture = seed_finalize_waiting_task(&pool).await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    let super_admin = UserId::new();
    seed_user(&pool, super_admin, "SUPER_ADMIN", fixture.branch_id).await;
    let allowed_token = issue_token(
        &pool,
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
        super_admin,
        vec!["SUPER_ADMIN".to_owned()],
        vec![fixture.branch_id],
    )
    .await
    .unwrap();

    let missing_reason = post_finalize(
        service.clone(),
        fixture.task_id,
        &allowed_token,
        json!({
            "mode": "delegate",
            "idempotency_key": "finalize-missing-reason-0001"
        }),
    )
    .await;
    assert_eq!(missing_reason.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(missing_reason.json["error"]["code"], "validation");
    assert!(
        missing_reason.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("reason")
    );

    // MECHANIC is a valid DB role and is denied approval_finalize by the legacy
    // matrix ([D,D,D,A,A,A] over [Member,Receptionist,Mechanic,Admin,Executive,SuperAdmin]).
    let member = UserId::new();
    seed_user(&pool, member, "MECHANIC", fixture.branch_id).await;
    let denied_token = issue_token(
        &pool,
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
        member,
        vec!["MECHANIC".to_owned()],
        vec![fixture.branch_id],
    )
    .await
    .unwrap();
    let denied = post_finalize(
        service.clone(),
        fixture.task_id,
        &denied_token,
        json!({
            "mode": "delegate",
            "reason": "author is unavailable",
            "idempotency_key": "finalize-denied-member-0001"
        }),
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert_eq!(denied.json["error"]["code"], "forbidden");

    let response = post_finalize(
        service,
        fixture.task_id,
        &allowed_token,
        json!({
            "mode": "delegate",
            "reason": "author is unavailable",
            "idempotency_key": "finalize-success-0001"
        }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.json["task"]["id"], fixture.task_id.to_string());
    assert_eq!(response.json["task"]["status"], "APPROVED");
    assert_eq!(
        response.json["task"]["completed_by"],
        super_admin.to_string()
    );
    assert_eq!(
        response.json["task"]["decision_payload"]["mode"],
        "delegate"
    );
    assert_eq!(
        response.json["task"]["decision_payload"]["delegated_reason"],
        "author is unavailable"
    );
    assert_eq!(response.json["run"]["status"], "SUCCEEDED");

    let task_status: String =
        sqlx::query_scalar("SELECT status FROM workflow_waiting_tasks WHERE id = $1")
            .bind(fixture.task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(task_status, "APPROVED");
    let finalize_audit = sqlx::query(
        "SELECT actor, target_type, target_id, before_snap, after_snap \
         FROM audit_events \
         WHERE action = 'workflow_task.finalize' AND target_id = $1",
    )
    .bind(fixture.task_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    let finalize_actor: Uuid = finalize_audit.try_get("actor").unwrap();
    let finalize_target_type: String = finalize_audit.try_get("target_type").unwrap();
    let finalize_target_id: String = finalize_audit.try_get("target_id").unwrap();
    let finalize_before: Value = finalize_audit.try_get("before_snap").unwrap();
    let finalize_after: Value = finalize_audit.try_get("after_snap").unwrap();
    assert_eq!(finalize_actor, *super_admin.as_uuid());
    assert_eq!(finalize_target_type, "workflow_waiting_task");
    assert_eq!(finalize_target_id, fixture.task_id.to_string());
    assert_eq!(finalize_before["status"], "OPEN");
    assert_eq!(finalize_after["status"], "APPROVED");
    assert_eq!(finalize_after["mode"], "delegate");
    assert_eq!(finalize_after["delegated_reason"], "author is unavailable");

    let shadow_audit = sqlx::query(
        "SELECT actor, target_type, target_id, after_snap \
         FROM audit_events \
         WHERE action = 'workflow_runtime.cedar_shadow' AND target_id = $1",
    )
    .bind(fixture.task_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    let shadow_actor: Uuid = shadow_audit.try_get("actor").unwrap();
    let shadow_target_type: String = shadow_audit.try_get("target_type").unwrap();
    let shadow_target_id: String = shadow_audit.try_get("target_id").unwrap();
    let shadow_after: Value = shadow_audit.try_get("after_snap").unwrap();
    assert_eq!(shadow_actor, *super_admin.as_uuid());
    assert_eq!(shadow_target_type, "workflow_waiting_task");
    assert_eq!(shadow_target_id, fixture.task_id.to_string());
    assert_eq!(shadow_after["decision"]["effect"], "allow");
    assert_eq!(shadow_after["decision"]["engine"], "legacy");
    assert_eq!(shadow_after["decision"]["reason"], "legacy_allowed");
    assert_eq!(shadow_after["decision"]["mode"], "legacy_only");
    assert_eq!(shadow_after["action"], "approval_finalize");
    assert_eq!(
        shadow_after["requestDomain"],
        "workflow.waiting_task_completion"
    );
    assert_eq!(shadow_after["resourceType"], "approval_document");
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn finalization_with_receipt_step_keeps_run_waiting_and_opens_receipt_task(pool: PgPool) {
    let keys = keys();
    let fixture = seed_finalize_waiting_task_with_receipt(&pool).await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    let author = UserId::new();
    seed_user(&pool, author, "ADMIN", fixture.branch_id).await;
    sqlx::query("UPDATE workflow_runs SET initiated_by = $1 WHERE id = $2")
        .bind(*author.as_uuid())
        .bind(fixture.run_id)
        .execute(&pool)
        .await
        .unwrap();
    let token = issue_token(
        &pool,
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
        author,
        vec!["ADMIN".to_owned()],
        vec![fixture.branch_id],
    )
    .await
    .unwrap();

    let response = post_finalize(
        service,
        fixture.task_id,
        &token,
        json!({
            "mode": "author",
            "idempotency_key": "finalize-awaits-receipt-0001"
        }),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.json["task"]["status"], "APPROVED");
    assert_eq!(response.json["run"]["status"], "WAITING");

    let run_status: String = sqlx::query_scalar("SELECT status FROM workflow_runs WHERE id = $1")
        .bind(fixture.run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(run_status, "WAITING");

    let receipt = sqlx::query(
        "SELECT t.status, t.required_policy, n.status AS node_status \
         FROM workflow_waiting_tasks t \
         JOIN workflow_node_runs n ON n.id = t.node_run_id AND n.org_id = t.org_id \
         WHERE t.run_id = $1 AND t.waiting_key = 'receipt.target'",
    )
    .bind(fixture.run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let receipt_task_status: String = receipt.try_get("status").unwrap();
    let receipt_policy: String = receipt.try_get("required_policy").unwrap();
    let receipt_node_status: String = receipt.try_get("node_status").unwrap();
    assert_eq!(receipt_task_status, "OPEN");
    assert_eq!(receipt_policy, "approval_receipt");
    assert_eq!(receipt_node_status, "WAITING");

    let receipt_audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events \
         WHERE action = 'workflow_node.commit' AND after_snap->>'status' = 'WAITING'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        receipt_audit_count >= 1,
        "receipt waiting-node creation must be audited"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn post_finalization_rejection_creates_compensation_without_reopening_run(pool: PgPool) {
    let keys = keys();
    let fixture = seed_finalize_waiting_task(&pool).await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    let reviewer = UserId::new();
    let approver = UserId::new();
    seed_user(&pool, reviewer, "ADMIN", fixture.branch_id).await;
    seed_user(&pool, approver, "ADMIN", fixture.branch_id).await;
    seed_completed_line_task(&pool, fixture.run_id, "review.hr", reviewer).await;
    seed_completed_line_task(&pool, fixture.run_id, "approve.manager", approver).await;

    let actor = UserId::new();
    seed_user(&pool, actor, "SUPER_ADMIN", fixture.branch_id).await;
    let token = issue_token(
        &pool,
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
        actor,
        vec!["SUPER_ADMIN".to_owned()],
        vec![fixture.branch_id],
    )
    .await
    .unwrap();

    let finalized = post_finalize(
        service.clone(),
        fixture.task_id,
        &token,
        json!({
            "mode": "delegate",
            "reason": "author is unavailable",
            "idempotency_key": "finalize-before-post-reject-0001"
        }),
    )
    .await;
    assert_eq!(finalized.status, StatusCode::OK);
    assert_eq!(finalized.json["run"]["status"], "SUCCEEDED");

    let response = post_post_finalization_rejection(
        service,
        fixture.run_id,
        &token,
        json!({
            "reason": "receipt evidence was invalid",
            "idempotency_key": "post-finalization-reject-0001"
        }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.json["compensation"]["original_run_id"],
        fixture.run_id.to_string()
    );
    assert_eq!(
        response.json["compensation"]["reason"],
        "receipt evidence was invalid"
    );
    assert_eq!(response.json["run"]["status"], "SUCCEEDED");

    let run_status: String = sqlx::query_scalar("SELECT status FROM workflow_runs WHERE id = $1")
        .bind(fixture.run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(run_status, "SUCCEEDED");

    let compensation = sqlx::query(
        "SELECT original_run_id, compensation_type, reason, created_by \
         FROM workflow_compensating_documents \
         WHERE id = $1",
    )
    .bind(
        Uuid::parse_str(
            response.json["compensation"]["id"]
                .as_str()
                .expect("compensation id"),
        )
        .unwrap(),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let original_run_id: Uuid = compensation.try_get("original_run_id").unwrap();
    let compensation_type: String = compensation.try_get("compensation_type").unwrap();
    let reason: String = compensation.try_get("reason").unwrap();
    let created_by: Uuid = compensation.try_get("created_by").unwrap();
    assert_eq!(original_run_id, fixture.run_id);
    assert_eq!(compensation_type, "POST_FINALIZATION_REJECTION");
    assert_eq!(reason, "receipt evidence was invalid");
    assert_eq!(created_by, *actor.as_uuid());

    let compensation_audit = sqlx::query(
        "SELECT target_type, target_id, after_snap FROM audit_events \
         WHERE action = 'workflow_compensation.create_post_finalization_rejection' \
           AND target_id = $1",
    )
    .bind(response.json["compensation"]["id"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    let compensation_audit_target_type: String = compensation_audit.try_get("target_type").unwrap();
    let compensation_audit_target_id: String = compensation_audit.try_get("target_id").unwrap();
    let compensation_audit_after: Value = compensation_audit.try_get("after_snap").unwrap();
    assert_eq!(
        compensation_audit_target_type,
        "workflow_compensating_document"
    );
    assert_eq!(
        compensation_audit_target_id,
        response.json["compensation"]["id"]
    );
    assert_eq!(
        compensation_audit_after["original_run_id"],
        fixture.run_id.to_string()
    );
    assert_eq!(
        compensation_audit_after["compensation_id"],
        response.json["compensation"]["id"]
    );

    let notification = sqlx::query(
        "SELECT destination_ref, payload FROM workflow_outbox_events \
         WHERE run_id = $1 \
           AND channel = 'NOTIFICATION' \
           AND payload->>'event' = 'post_finalization_rejection'",
    )
    .bind(fixture.run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let destination_ref: String = notification.try_get("destination_ref").unwrap();
    let payload: Value = notification.try_get("payload").unwrap();
    assert_eq!(destination_ref, "approval_line");
    assert_eq!(payload["event"], "post_finalization_rejection");
    assert_eq!(
        payload["compensation_id"],
        response.json["compensation"]["id"]
    );
    assert_eq!(payload["original_run_id"], fixture.run_id.to_string());
    assert_eq!(payload["reason"], "receipt evidence was invalid");
    let recipients = payload["recipients"]
        .as_array()
        .expect("line-wide notification recipients")
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        recipients,
        BTreeSet::from([
            fixture.author_id.to_string(),
            reviewer.to_string(),
            approver.to_string(),
            actor.to_string()
        ])
    );
}

// Security L6: an idempotency_key already used to compensate one run must 409
// when re-presented against a DIFFERENT run (cross-run reuse), never silently
// replay the stored compensation.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn post_finalization_idempotency_key_cannot_cross_runs(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let actor = UserId::new();
    seed_user(&pool, actor, "SUPER_ADMIN", branch).await;
    let definition_id = seed_definition_with_receipt(&pool, false).await;
    let run_a = seed_succeeded_run(&pool, definition_id, actor).await;
    let run_b = seed_succeeded_run(&pool, definition_id, actor).await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = issue_token(
        &pool,
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
        actor,
        vec!["SUPER_ADMIN".to_owned()],
        vec![branch],
    )
    .await
    .unwrap();

    let key = "cross-run-reuse-key-0001";
    let first = post_post_finalization_rejection(
        service.clone(),
        run_a,
        &token,
        json!({ "reason": "invalid evidence", "idempotency_key": key }),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.json);

    // Same key, DIFFERENT run → 409, not a replay of run_a's compensation.
    let reused = post_post_finalization_rejection(
        service,
        run_b,
        &token,
        json!({ "reason": "invalid evidence", "idempotency_key": key }),
    )
    .await;
    assert_eq!(reused.status, StatusCode::CONFLICT, "{:?}", reused.json);
}

async fn seed_succeeded_run(pool: &PgPool, definition_id: Uuid, initiated_by: UserId) -> Uuid {
    let org = OrgId::knl();
    let run_id = Uuid::new_v4();
    let suffix = run_id.simple().to_string();
    sqlx::query(
        "INSERT INTO workflow_runs \
             (id, org_id, definition_id, definition_version, status, trigger_type, \
              idempotency_key, correlation_id, initiated_by, completed_at) \
         VALUES ($1, $2, $3, 1, 'SUCCEEDED', 'MANUAL', $4, $5, $6, now())",
    )
    .bind(run_id)
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(format!("succeeded-run-idem-{suffix}"))
    .bind(format!("succeeded-run-corr-{suffix}"))
    .bind(*initiated_by.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    run_id
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn author_finalize_replays_same_idempotency_key_as_200(pool: PgPool) {
    let keys = keys();
    let fixture = seed_finalize_waiting_task(&pool).await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    // The fixture run's initiator is the seeded author; author-mode finalize is owner-checked.
    let token = issue_token(
        &pool,
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
        fixture.author_id,
        vec!["ADMIN".to_owned()],
        vec![fixture.branch_id],
    )
    .await
    .unwrap();

    let body = json!({ "mode": "author", "idempotency_key": "author-finalize-replay-0001" });
    let first = post_finalize(service.clone(), fixture.task_id, &token, body.clone()).await;
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(first.json["task"]["status"], "APPROVED");
    assert_eq!(first.json["run"]["status"], "SUCCEEDED");

    let replay = post_finalize(service, fixture.task_id, &token, body).await;
    assert_eq!(replay.status, StatusCode::OK);
    assert_eq!(replay.json["task"]["status"], "APPROVED");
    assert_eq!(replay.json["run"]["status"], "SUCCEEDED");
    assert_eq!(replay.json["task"]["id"], first.json["task"]["id"]);

    // The replay is a pure read-back: exactly one finalize audit, no double-write.
    let finalize_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events \
         WHERE action = 'workflow_task.finalize' AND target_id = $1",
    )
    .bind(fixture.task_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(finalize_audits, 1);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn finalize_conflicts_when_task_claimed_by_another_user(pool: PgPool) {
    let keys = keys();
    let fixture = seed_finalize_waiting_task(&pool).await;

    // A different user holds the claim on the finalization task.
    let other = UserId::new();
    seed_user(&pool, other, "ADMIN", fixture.branch_id).await;
    sqlx::query(
        "UPDATE workflow_waiting_tasks \
         SET status = 'CLAIMED', claimed_by = $1, claimed_at = now() WHERE id = $2",
    )
    .bind(*other.as_uuid())
    .bind(fixture.task_id)
    .execute(&pool)
    .await
    .unwrap();

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = issue_token(
        &pool,
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
        fixture.author_id,
        vec!["ADMIN".to_owned()],
        vec![fixture.branch_id],
    )
    .await
    .unwrap();

    let response = post_finalize(
        service,
        fixture.task_id,
        &token,
        json!({ "mode": "author", "idempotency_key": "finalize-claimed-by-other-0001" }),
    )
    .await;
    assert_eq!(response.status, StatusCode::CONFLICT);
    assert_eq!(response.json["error"]["code"], "conflict");

    // The stale conflict left the run and task untouched (no partial advance).
    let run_status: String = sqlx::query_scalar("SELECT status FROM workflow_runs WHERE id = $1")
        .bind(fixture.run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(run_status, "WAITING");
    let task_status: String =
        sqlx::query_scalar("SELECT status FROM workflow_waiting_tasks WHERE id = $1")
            .bind(fixture.task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(task_status, "CLAIMED");
}

async fn post_finalize(
    service: axum::Router,
    task_id: Uuid,
    token: &str,
    body: Value,
) -> JsonResponse {
    let response = service
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/workflow-tasks/{task_id}/finalize"))
                .method("POST")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
}

async fn post_post_finalization_rejection(
    service: axum::Router,
    run_id: Uuid,
    token: &str,
    body: Value,
) -> JsonResponse {
    let response = service
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/workflow-runs/{run_id}/post-finalization-rejection"
                ))
                .method("POST")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
}

async fn seed_finalize_waiting_task(pool: &PgPool) -> FinalizeFixture {
    seed_finalize_waiting_task_for_definition(pool, false).await
}

async fn seed_finalize_waiting_task_with_receipt(pool: &PgPool) -> FinalizeFixture {
    seed_finalize_waiting_task_for_definition(pool, true).await
}

async fn seed_finalize_waiting_task_for_definition(
    pool: &PgPool,
    receipt_required: bool,
) -> FinalizeFixture {
    let org = OrgId::knl();
    let branch_id = seed_branch(pool).await;
    let author_id = UserId::new();
    seed_user(pool, author_id, "ADMIN", branch_id).await;
    let definition_id = seed_definition_with_receipt(pool, receipt_required).await;
    let run_id = Uuid::new_v4();
    let node_run_id = Uuid::new_v4();
    let task_id = Uuid::new_v4();

    sqlx::query(
        "INSERT INTO workflow_runs \
             (id, org_id, definition_id, definition_version, status, trigger_type, \
              object_type, object_id, idempotency_key, correlation_id, input_payload, initiated_by) \
         VALUES ($1, $2, $3, 1, 'WAITING', 'MANUAL', 'approval_document', $4, \
                 'finalize-run-key-0001', 'finalize-correlation-0001', '{}'::jsonb, $5)",
    )
    .bind(run_id)
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(Uuid::new_v4())
    .bind(*author_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_node_runs \
             (id, org_id, run_id, node_key, node_type, status, attempt, idempotency_key, input_payload, started_at) \
         VALUES ($1, $2, $3, 'finalize.author', 'human_task', 'WAITING', 1, \
                 'finalize-node-key-0001', '{}'::jsonb, now())",
    )
    .bind(node_run_id)
    .bind(*org.as_uuid())
    .bind(run_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_waiting_tasks \
             (id, org_id, run_id, node_run_id, waiting_key, title, status, assignee_role_key, required_policy, form_payload) \
         VALUES ($1, $2, $3, $4, 'finalize.author', 'Author finalize', 'OPEN', \
                 'initiator', 'approval_finalize', '{}'::jsonb)",
    )
    .bind(task_id)
    .bind(*org.as_uuid())
    .bind(run_id)
    .bind(node_run_id)
    .execute(pool)
    .await
    .unwrap();

    FinalizeFixture {
        task_id,
        run_id,
        branch_id,
        author_id,
    }
}

async fn seed_completed_line_task(pool: &PgPool, run_id: Uuid, waiting_key: &str, user_id: UserId) {
    sqlx::query(
        "INSERT INTO workflow_waiting_tasks \
             (org_id, run_id, waiting_key, title, status, assignee_user_id, required_policy, \
              form_payload, decision_payload, completed_by, completed_at) \
         VALUES ($1, $2, $3, $4, 'APPROVED', $5, 'approval_decide', '{}'::jsonb, \
                 '{\"decision\":\"approve\"}'::jsonb, $5, now())",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(run_id)
    .bind(waiting_key)
    .bind(format!("Completed {waiting_key}"))
    .bind(*user_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_definition_with_receipt(pool: &PgPool, receipt_required: bool) -> Uuid {
    let org = OrgId::knl();
    let workflow_key = if receipt_required {
        "approval.leave"
    } else {
        "approval.general"
    };
    let definition = if receipt_required {
        json!({
            "schema_version": "wf.exec.v1",
            "workflow_key": workflow_key,
            "nodes": [
                {
                    "node_key": "finalize.author",
                    "node_type": "human_task",
                    "title": "Author finalize",
                    "required_policy": "approval_finalize",
                    "assignee_role_key": "initiator"
                },
                {
                    "node_key": "receipt.target",
                    "node_type": "human_task",
                    "title": "Receipt confirmation",
                    "required_policy": "approval_receipt",
                    "assignee_role_key": "receipt_subject"
                }
            ],
            "edges": [{ "from": "finalize.author", "to": "receipt.target" }]
        })
    } else {
        json!({ "schema_version": "wf.exec.v1", "workflow_key": workflow_key })
    };
    let definition_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_definitions \
             (org_id, workflow_key, display_name, object_type, status, latest_version, active_version) \
         VALUES ($1, $2, 'General Approval', 'approval_document', 'ACTIVE', 1, 1) \
         RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(workflow_key)
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

async fn seed_branch(pool: &PgPool) -> BranchId {
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind("Workflow Runtime Region")
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind("Workflow Runtime Branch")
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(pool: &PgPool, user_id: UserId, role: &str, _branch_id: BranchId) {
    // `users` has no branch_id column — the principal's branch scope is carried by
    // the JWT, not the user row. The DB role only needs to satisfy the roles CHECK
    // and back the audit actor FK.
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("workflow-runtime-{role}"))
        .bind(vec![role])
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

fn keys() -> Keys {
    let signing_key = SigningKey::random(&mut OsRng);
    Keys {
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

async fn issue_token(
    pool: &PgPool,
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    roles: Vec<String>,
    branches: Vec<BranchId>,
) -> Result<String, Box<dyn std::error::Error>> {
    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private_key_pem,
        public_key_pem,
    )?;

    Ok(console_platform_test_support::issue_session_token(
        pool,
        &issuer,
        AccessTokenInput {
            subject: user_id,
            org_id: OrgId::knl(),
            roles,
            branches,
            platform: false,
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
        Vec::new(),
    )
    .await)
}

/// Runtime-role pool: every connection assumes the genuine non-owner `console_rt`, so
/// RLS is ACTUALLY enforced (a superuser pool would BYPASSRLS and mask breakage).
/// The router under test runs on this pool exactly as production does; fixture
/// seeding uses the owner `pool` (which may write across tenants for setup).
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

fn app_state(pool: PgPool, public_key_pem: String) -> Result<AppState, console_app::AppError> {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key_pem),
    ])?;

    AppState::new(config, DatabaseDependency::Postgres(pool))
}
