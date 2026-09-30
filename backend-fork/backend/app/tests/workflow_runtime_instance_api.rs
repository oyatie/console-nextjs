#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Instance / task REST surface E2E (engine-gen spike §"Instance/Task REST Surface").
//!
//! Drives the REAL router on a genuine non-owner `console_rt` pool (RLS actually
//! enforced, never a BYPASSRLS superuser) exactly as `workflow_runtime_finalize_api`
//! does, covering the spike's closing matrix items:
//!   * start idempotency replay (same key → same run; mismatched key → 409),
//!   * decision comment-required 422 (reject/return need a non-empty comment),
//!   * deny-by-omission inbox listing (a persona without the policy sees an EMPTY
//!     list, not a 403), plus the core claim/decide/advance + submission-box paths.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, ErrorKind, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use console_workflow_domain::WaitingTaskStatus;
use console_workflow_runtime_adapter_postgres::{
    DecideWaitingTaskCommand, PgWorkflowRuntimeStore, TaskDecision,
};
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

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

// ===========================================================================
// 1. Start: parks the first approval task, and replays idempotently.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn start_run_parks_first_task_and_replays_idempotently(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.instance.start").await;
    let other_definition_id = seed_approval_definition(&pool, "approval.instance.other").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&pool, &keys, initiator, "SUPER_ADMIN", branch).await;

    let object_id = Uuid::new_v4();
    let body = json!({
        "definition_id": definition_id,
        "object_type": "approval_document",
        "object_id": object_id,
        "trigger_type": "MANUAL",
        "idempotency_key": "instance-start-key-000001",
        "input_payload": { "reason": "annual" }
    });

    let first = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &token,
        body.clone(),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.json);
    assert_eq!(first.json["run"]["status"], "WAITING");
    assert_eq!(first.json["run"]["initiated_by"], initiator.to_string());
    assert_eq!(first.json["next_task"]["waiting_key"], "review.hr");
    assert_eq!(first.json["next_task"]["assignee_role_key"], "hr_reviewer");
    let run_id = first.json["run"]["id"].as_str().unwrap().to_owned();

    // Replay with the SAME idempotency_key returns the existing run, no second run.
    let replay = post(service.clone(), "/api/v1/workflow-runs", &token, body).await;
    assert_eq!(replay.status, StatusCode::OK);
    assert_eq!(replay.json["run"]["id"], run_id);
    let run_count = count_runs(&pool).await;
    assert_eq!(run_count, 1, "replay must NOT create a second run");

    // Same key, different definition (mismatch) → 409.
    let mismatch = post(
        service,
        "/api/v1/workflow-runs",
        &token,
        json!({
            "definition_id": other_definition_id,
            "object_type": "approval_document",
            "object_id": object_id,
            "trigger_type": "MANUAL",
            "idempotency_key": "instance-start-key-000001",
            "input_payload": {}
        }),
    )
    .await;
    assert_eq!(mismatch.status, StatusCode::CONFLICT);
    assert_eq!(mismatch.json["error"]["code"], "conflict");
}

// ===========================================================================
// 2. Decide: reject/return require a non-empty comment (422); reject cancels run.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn decision_reject_requires_comment_and_cancels_run(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.instance.reject").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&pool, &keys, initiator, "SUPER_ADMIN", branch).await;

    let started = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &token,
        json!({
            "definition_id": definition_id,
            "trigger_type": "MANUAL",
            "idempotency_key": "instance-reject-key-000001",
            "input_payload": {}
        }),
    )
    .await;
    assert_eq!(started.status, StatusCode::OK, "{:?}", started.json);
    let task_id = started.json["next_task"]["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run_id = started.json["run"]["id"].as_str().unwrap().to_owned();

    // reject WITHOUT a comment → 422.
    let no_comment = post(
        service.clone(),
        &format!("/api/v1/workflow-tasks/{task_id}/decide"),
        &token,
        json!({ "decision": "reject", "idempotency_key": "reject-no-comment-000001" }),
    )
    .await;
    assert_eq!(no_comment.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(no_comment.json["error"]["code"], "validation");
    assert!(
        no_comment.json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("comment")
    );

    // return with only whitespace → still 422.
    let blank_comment = post(
        service.clone(),
        &format!("/api/v1/workflow-tasks/{task_id}/decide"),
        &token,
        json!({ "decision": "return", "comment": "   ", "idempotency_key": "return-blank-000001" }),
    )
    .await;
    assert_eq!(blank_comment.status, StatusCode::UNPROCESSABLE_ENTITY);

    // reject WITH a comment → task REJECTED, run CANCELLED (no reopen).
    let rejected = post(
        service,
        &format!("/api/v1/workflow-tasks/{task_id}/decide"),
        &token,
        json!({ "decision": "reject", "comment": "insufficient evidence", "idempotency_key": "reject-with-comment-01" }),
    )
    .await;
    assert_eq!(rejected.status, StatusCode::OK, "{:?}", rejected.json);
    assert_eq!(rejected.json["task"]["status"], "REJECTED");
    assert_eq!(rejected.json["run"]["status"], "CANCELLED");
    assert_eq!(
        rejected.json["task"]["decision_payload"]["comment"],
        "insufficient evidence"
    );

    let run_status = run_status(&pool, &run_id).await;
    assert_eq!(run_status, "CANCELLED");
}

// ===========================================================================
// 3. Decide approve advances the approval line to the next human task.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn decision_approve_advances_the_line(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.instance.advance").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&pool, &keys, initiator, "SUPER_ADMIN", branch).await;

    let started = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &token,
        json!({
            "definition_id": definition_id,
            "trigger_type": "MANUAL",
            "idempotency_key": "instance-advance-key-00001",
            "input_payload": {}
        }),
    )
    .await;
    let task_id = started.json["next_task"]["task_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let approver = UserId::new();
    seed_user(&pool, approver, "SUPER_ADMIN", branch).await;
    let approval_token = bearer(&pool, &keys, approver, "SUPER_ADMIN", branch).await;
    let approved = post(
        service,
        &format!("/api/v1/workflow-tasks/{task_id}/decide"),
        &approval_token,
        json!({ "decision": "approve", "idempotency_key": "approve-review-000001" }),
    )
    .await;
    assert_eq!(approved.status, StatusCode::OK, "{:?}", approved.json);
    assert_eq!(approved.json["task"]["status"], "APPROVED");
    assert_eq!(approved.json["run"]["status"], "WAITING");
    assert_eq!(approved.json["next_task"]["waiting_key"], "approve.manager");
    assert_eq!(
        approved.json["next_task"]["assignee_role_key"],
        "manager_approver"
    );
}

// ===========================================================================
// 3b. SoD / maker-checker (기안자 ≠ 승인자): the run initiator cannot APPROVE
//     their own approval task; a different approver can. Exercised at the store
//     boundary (like `financial`'s self-approval tests) so the actor is the
//     genuine decider — the REST policy guard only admits SUPER_ADMIN-tier roles
//     in this harness, whose self-approval denial is covered below.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn initiator_cannot_approve_own_run_but_other_approver_can(pool: PgPool) {
    let branch = seed_branch(&pool).await;
    // ADMIN is a valid, NON-exempt role (not SUPER_ADMIN, not org 대표) — the
    // case the SoD guard must hard-block.
    let initiator = UserId::new();
    seed_user(&pool, initiator, "ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.instance.sod").await;
    // An OPEN approval task owned by `initiator`, carrying a real decide policy.
    let (_run_id, task_id) = seed_run_with_open_task(
        &pool,
        definition_id,
        initiator,
        "manager_approver",
        Some("approval_decide"),
    )
    .await;
    let store = PgWorkflowRuntimeStore::new(runtime_role_pool(&pool).await);

    // (a) The initiator approves their own run → 403 forbidden (SoD), rolled back.
    let blocked = store
        .decide_waiting_task(
            OrgId::knl(),
            DecideWaitingTaskCommand {
                task_id,
                actor: initiator,
                decision: TaskDecision::Approve,
                comment: None,
                idempotency_key: "sod-store-self-approve-01".to_owned(),
                transition_audits: Vec::new(),
            },
        )
        .await;
    let err = blocked.expect_err("initiator must not approve their own run");
    assert_eq!(err.kind, ErrorKind::Forbidden, "{err:?}");
    assert!(err.message.contains("승인"), "{err:?}");
    // A hard block writes NO governance finding, and the tx rolled back — the
    // task is still OPEN and decidable by someone else.
    assert_eq!(governance_finding_count(&pool).await, 0);
    let status: String =
        sqlx::query_scalar("SELECT status FROM workflow_waiting_tasks WHERE id = $1")
            .bind(task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "OPEN");

    // (b) A DIFFERENT approver (not the initiator) decides the same task → allowed.
    let approver = UserId::new();
    seed_user(&pool, approver, "ADMIN", branch).await;
    let decided = store
        .decide_waiting_task(
            OrgId::knl(),
            DecideWaitingTaskCommand {
                task_id,
                actor: approver,
                decision: TaskDecision::Approve,
                comment: None,
                idempotency_key: "sod-store-other-approve-1".to_owned(),
                transition_audits: Vec::new(),
            },
        )
        .await
        .expect("a non-initiator approver may decide");
    assert_eq!(decided.status, WaitingTaskStatus::Approved);
    // A legitimate, non-self approval writes no governance finding.
    assert_eq!(governance_finding_count(&pool).await, 0);
}

// The HTTP boundary must deny SUPER_ADMIN self-approval too.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn super_admin_self_approval_is_denied_without_effect(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.instance.sodexempt").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&pool, &keys, initiator, "SUPER_ADMIN", branch).await;
    let started = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &token,
        json!({
            "definition_id": definition_id, "trigger_type": "MANUAL",
            "idempotency_key": "instance-sodexempt-key-01", "input_payload": {}
        }),
    )
    .await;
    assert_eq!(started.status, StatusCode::OK, "{:?}", started.json);
    let task_id = Uuid::parse_str(started.json["next_task"]["task_id"].as_str().unwrap()).unwrap();
    let before = task_evidence(&pool, task_id).await;
    let denied = post(
        service,
        &format!("/api/v1/workflow-tasks/{task_id}/decide"),
        &token,
        json!({"decision":"approve", "idempotency_key":"sod-self-denied-01"}),
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{:?}", denied.json);
    assert_eq!(task_evidence(&pool, task_id).await, before);
}

async fn task_evidence(pool: &PgPool, task_id: Uuid) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_array(to_jsonb(t), to_jsonb(r),
        (SELECT jsonb_agg(to_jsonb(f) ORDER BY f.id) FROM governance_findings f),
        (SELECT jsonb_agg(to_jsonb(a) ORDER BY a.id) FROM audit_events a WHERE action='workflow_task.decide'))
        FROM workflow_waiting_tasks t JOIN workflow_runs r ON r.id=t.run_id WHERE t.id=$1")
        .bind(task_id).fetch_one(pool).await.unwrap()
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn org_lead_and_missing_initiator_cannot_approve_but_can_return(pool: PgPool) {
    let branch = seed_branch(&pool).await;
    let actor = UserId::new();
    seed_user(&pool, actor, "ADMIN", branch).await;
    sqlx::query("UPDATE users SET is_org_lead=true WHERE id=$1")
        .bind(*actor.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let definition = seed_approval_definition(&pool, "approval.missing.initiator").await;
    let store = PgWorkflowRuntimeStore::new(runtime_role_pool(&pool).await);
    for missing in [false, true] {
        let (run, task) = seed_run_with_open_task(
            &pool,
            definition,
            actor,
            "manager_approver",
            Some("approval_decide"),
        )
        .await;
        if missing {
            sqlx::query("UPDATE workflow_runs SET initiated_by=NULL WHERE id=$1")
                .bind(run)
                .execute(&pool)
                .await
                .unwrap();
        }
        let before = task_evidence(&pool, task).await;
        let command = |decision| DecideWaitingTaskCommand {
            task_id: task,
            actor,
            decision,
            comment: Some("검토 후 반려".to_owned()),
            idempotency_key: format!("missing-maker-{task}"),
            transition_audits: Vec::new(),
        };
        assert_eq!(
            store
                .decide_waiting_task(OrgId::knl(), command(TaskDecision::Approve))
                .await
                .unwrap_err()
                .kind,
            ErrorKind::Forbidden
        );
        assert_eq!(task_evidence(&pool, task).await, before);
        assert_eq!(
            store
                .decide_waiting_task(OrgId::knl(), command(TaskDecision::Return))
                .await
                .unwrap()
                .status,
            WaitingTaskStatus::Rejected
        );
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn historical_self_approval_replay_preserves_result_and_finding(pool: PgPool) {
    let branch = seed_branch(&pool).await;
    let actor = UserId::new();
    seed_user(&pool, actor, "SUPER_ADMIN", branch).await;
    let definition = seed_approval_definition(&pool, "approval.historical.replay").await;
    let (run, task) = seed_run_with_open_task(
        &pool,
        definition,
        actor,
        "manager_approver",
        Some("approval_decide"),
    )
    .await;
    sqlx::query(r#"UPDATE workflow_waiting_tasks SET status='APPROVED', completed_by=$2, completed_at=now(),
        decision_payload='{"decision":"approve","idempotency_key":"historical-approved-key"}'::jsonb WHERE id=$1"#)
        .bind(task).bind(*actor.as_uuid()).execute(&pool).await.unwrap();
    sqlx::query(r#"INSERT INTO governance_findings (org_id,detector_id,entity_type,entity_id,subject_user_id,score,severity,detected_at,evidence)
        VALUES ($1,'anomaly.self_approval','workflow_run',$2,$3,1,'HIGH',now(),'{"exemption_reason":"super_admin_exempt"}')"#)
        .bind(*OrgId::knl().as_uuid()).bind(run.to_string()).bind(*actor.as_uuid()).execute(&pool).await.unwrap();
    let before = task_evidence(&pool, task).await;
    let store = PgWorkflowRuntimeStore::new(runtime_role_pool(&pool).await);
    let replay = store
        .decide_waiting_task(
            OrgId::knl(),
            DecideWaitingTaskCommand {
                task_id: task,
                actor,
                decision: TaskDecision::Approve,
                comment: None,
                idempotency_key: "historical-approved-key".to_owned(),
                transition_audits: Vec::new(),
            },
        )
        .await
        .unwrap();
    assert_eq!(replay.status, WaitingTaskStatus::Approved);
    assert_eq!(task_evidence(&pool, task).await, before);
}

// ===========================================================================
// 4. Deny-by-omission: a persona without the policy sees an EMPTY list (not 403).
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn role_inbox_denies_by_omission(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.instance.inbox").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let starter_token = bearer(&pool, &keys, initiator, "SUPER_ADMIN", branch).await;

    // Park a review.hr task (required_policy approval_review → completion_review).
    let started = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &starter_token,
        json!({
            "definition_id": definition_id,
            "trigger_type": "MANUAL",
            "idempotency_key": "instance-inbox-key-000001",
            "input_payload": {}
        }),
    )
    .await;
    assert_eq!(started.status, StatusCode::OK, "{:?}", started.json);

    // MECHANIC is DENIED completion_review → sees an empty list (deny-by-omission).
    let mechanic = UserId::new();
    seed_user(&pool, mechanic, "MECHANIC", branch).await;
    let denied = get(
        service.clone(),
        "/api/v1/workflow-tasks?role_key=hr_reviewer&status=OPEN",
        &bearer(&pool, &keys, mechanic, "MECHANIC", branch).await,
    )
    .await;
    assert_eq!(
        denied.status,
        StatusCode::OK,
        "deny-by-omission is 200 empty, never 403"
    );
    assert_eq!(denied.json["items"].as_array().unwrap().len(), 0);

    // ADMIN is ALLOWED completion_review → sees the row.
    let reviewer = UserId::new();
    seed_user(&pool, reviewer, "SUPER_ADMIN", branch).await;
    let allowed = get(
        service,
        "/api/v1/workflow-tasks?role_key=hr_reviewer&status=OPEN",
        &bearer(&pool, &keys, reviewer, "SUPER_ADMIN", branch).await,
    )
    .await;
    assert_eq!(allowed.status, StatusCode::OK);
    let items = allowed.json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["waiting_key"], "review.hr");
}

// ===========================================================================
// 5. Claim: OPEN → CLAIMED, same-user replay 200, other-user 409.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn claim_transitions_open_to_claimed_with_replay_and_conflict(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.instance.claim").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    let started = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &bearer(&pool, &keys, initiator, "SUPER_ADMIN", branch).await,
        json!({
            "definition_id": definition_id,
            "trigger_type": "MANUAL",
            "idempotency_key": "instance-claim-key-000001",
            "input_payload": {}
        }),
    )
    .await;
    let task_id = started.json["next_task"]["task_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let claimer = UserId::new();
    seed_user(&pool, claimer, "SUPER_ADMIN", branch).await;
    let claimer_token = bearer(&pool, &keys, claimer, "SUPER_ADMIN", branch).await;

    let claimed = post(
        service.clone(),
        &format!("/api/v1/workflow-tasks/{task_id}/claim"),
        &claimer_token,
        json!({ "idempotency_key": "claim-key-first-000001" }),
    )
    .await;
    assert_eq!(claimed.status, StatusCode::OK, "{:?}", claimed.json);
    assert_eq!(claimed.json["task"]["status"], "CLAIMED");
    assert_eq!(claimed.json["task"]["claimed_by"], claimer.to_string());

    // Same-user replay → 200.
    let replay = post(
        service.clone(),
        &format!("/api/v1/workflow-tasks/{task_id}/claim"),
        &claimer_token,
        json!({ "idempotency_key": "claim-key-replay-00001" }),
    )
    .await;
    assert_eq!(replay.status, StatusCode::OK);
    assert_eq!(replay.json["task"]["status"], "CLAIMED");

    // A different user → 409.
    let other = UserId::new();
    seed_user(&pool, other, "SUPER_ADMIN", branch).await;
    let conflict = post(
        service,
        &format!("/api/v1/workflow-tasks/{task_id}/claim"),
        &bearer(&pool, &keys, other, "SUPER_ADMIN", branch).await,
        json!({ "idempotency_key": "claim-key-other-000001" }),
    )
    .await;
    assert_eq!(conflict.status, StatusCode::CONFLICT);
    assert_eq!(conflict.json["error"]["code"], "conflict");
}

// ===========================================================================
// 6. Submission box lists the runs the principal initiated.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn submission_box_lists_initiated_runs(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.instance.mine").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&pool, &keys, initiator, "SUPER_ADMIN", branch).await;

    post(
        service.clone(),
        "/api/v1/workflow-runs",
        &token,
        json!({
            "definition_id": definition_id,
            "object_type": "approval_document",
            "object_id": Uuid::new_v4(),
            "trigger_type": "MANUAL",
            "idempotency_key": "instance-mine-key-0000001",
            "input_payload": {}
        }),
    )
    .await;

    let mine = get(service.clone(), "/api/v1/workflow-runs/mine", &token).await;
    assert_eq!(mine.status, StatusCode::OK);
    let items = mine.json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    // A WAITING (final-approved-but-not-finalized-style non-terminal) run stays visible.
    assert_eq!(items[0]["status"], "WAITING");
    assert_eq!(items[0]["object_type"], "approval_document");

    // A different principal's submission box is empty (initiated_by scoping).
    let stranger = UserId::new();
    seed_user(&pool, stranger, "SUPER_ADMIN", branch).await;
    let empty = get(
        service,
        "/api/v1/workflow-runs/mine",
        &bearer(&pool, &keys, stranger, "SUPER_ADMIN", branch).await,
    )
    .await;
    assert_eq!(empty.status, StatusCode::OK);
    assert_eq!(empty.json["items"].as_array().unwrap().len(), 0);
}

// ===========================================================================
// 6b. Engine-Gen follow-up: `?q=` free-text filter narrows the submission box.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn submission_box_free_text_filter_narrows_results(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.instance.qfilter").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&pool, &keys, initiator, "SUPER_ADMIN", branch).await;

    // Two runs with distinguishable human-readable content in input_payload.
    let annual = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &token,
        json!({
            "definition_id": definition_id,
            "object_type": "approval_document",
            "object_id": Uuid::new_v4(),
            "trigger_type": "MANUAL",
            "idempotency_key": "instance-qfilter-annual-01",
            "input_payload": { "reason": "annual leave request" }
        }),
    )
    .await;
    assert_eq!(annual.status, StatusCode::OK, "{:?}", annual.json);
    let annual_run_id = annual.json["run"]["id"].as_str().unwrap().to_owned();

    let expense = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &token,
        json!({
            "definition_id": definition_id,
            "object_type": "approval_document",
            "object_id": Uuid::new_v4(),
            "trigger_type": "MANUAL",
            "idempotency_key": "instance-qfilter-expense-1",
            "input_payload": { "reason": "business expense claim" }
        }),
    )
    .await;
    assert_eq!(expense.status, StatusCode::OK, "{:?}", expense.json);

    // `?q=annual` (case-insensitive) narrows to the matching run only.
    let filtered = get(
        service.clone(),
        "/api/v1/workflow-runs/mine?q=ANNUAL",
        &token,
    )
    .await;
    assert_eq!(filtered.status, StatusCode::OK);
    let items = filtered.json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "q=annual must match exactly one run");
    assert_eq!(items[0]["run_id"], annual_run_id);

    // Empty q returns all rows (as before the filter was applied).
    let empty_q = get(service.clone(), "/api/v1/workflow-runs/mine?q=", &token).await;
    assert_eq!(empty_q.status, StatusCode::OK);
    assert_eq!(empty_q.json["items"].as_array().unwrap().len(), 2);

    // Absent q returns all rows.
    let no_q = get(service.clone(), "/api/v1/workflow-runs/mine", &token).await;
    assert_eq!(no_q.status, StatusCode::OK);
    assert_eq!(no_q.json["items"].as_array().unwrap().len(), 2);

    // A q that matches nothing returns empty.
    let miss = get(
        service.clone(),
        "/api/v1/workflow-runs/mine?q=zzznomatch",
        &token,
    )
    .await;
    assert_eq!(miss.status, StatusCode::OK);
    assert_eq!(miss.json["items"].as_array().unwrap().len(), 0);

    // RLS + initiator scoping still holds: a stranger's filtered box is empty.
    let stranger = UserId::new();
    seed_user(&pool, stranger, "SUPER_ADMIN", branch).await;
    let stranger_view = get(
        service,
        "/api/v1/workflow-runs/mine?q=annual",
        &bearer(&pool, &keys, stranger, "SUPER_ADMIN", branch).await,
    )
    .await;
    assert_eq!(stranger_view.status, StatusCode::OK);
    assert_eq!(stranger_view.json["items"].as_array().unwrap().len(), 0);
}

// ===========================================================================
// 7. Security H1(b): deciding a policy-less legacy task fails closed (403).
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn decide_on_policy_less_task_is_forbidden(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let initiator = UserId::new();
    seed_user(&pool, initiator, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.h1b").await;
    // A legacy row with an assignee_role_key but NO required_policy — no
    // authorization boundary.
    let (_run_id, task_id) =
        seed_run_with_open_task(&pool, definition_id, initiator, "admin", None).await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    // Even a SUPER_ADMIN cannot decide a task that carries no policy.
    let actor = UserId::new();
    seed_user(&pool, actor, "SUPER_ADMIN", branch).await;
    let denied = post(
        service,
        &format!("/api/v1/workflow-tasks/{task_id}/decide"),
        &bearer(&pool, &keys, actor, "SUPER_ADMIN", branch).await,
        json!({ "decision": "approve", "idempotency_key": "decide-policyless-0001" }),
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{:?}", denied.json);
    assert!(
        denied.json["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("authorization boundary"),
        "{:?}",
        denied.json
    );
}

// ===========================================================================
// 8. Security M2: per-definition start authority + payload size cap.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn start_policy_gates_run_initiation_and_caps_payload(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    // Operational pipeline: start_policy = completion_review (NOT self-service).
    let payroll_definition = seed_start_policy_definition(&pool, "ops.completion.start").await;
    // Approval template: no start_policy — self-service 기안/상신 (DESIGN §4.8).
    let approval_definition = seed_approval_definition(&pool, "approval.selfservice").await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    // A non-privileged persona (MECHANIC lacks completion_review) is DENIED the
    // operational pipeline start.
    let mechanic = UserId::new();
    seed_user(&pool, mechanic, "MECHANIC", branch).await;
    let denied = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &bearer(&pool, &keys, mechanic, "MECHANIC", branch).await,
        json!({
            "definition_id": payroll_definition,
            "trigger_type": "MANUAL",
            "idempotency_key": "start-policy-denied-0001",
            "input_payload": {}
        }),
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{:?}", denied.json);

    // A privileged persona (SUPER_ADMIN holds completion_review) MAY start it.
    let admin = UserId::new();
    seed_user(&pool, admin, "SUPER_ADMIN", branch).await;
    let allowed = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &bearer(&pool, &keys, admin, "SUPER_ADMIN", branch).await,
        json!({
            "definition_id": payroll_definition,
            "trigger_type": "MANUAL",
            "idempotency_key": "start-policy-allowed-0001",
            "input_payload": {}
        }),
    )
    .await;
    assert_eq!(allowed.status, StatusCode::OK, "{:?}", allowed.json);

    // The self-service approval template is startable by the SAME non-privileged
    // persona (no start_policy).
    let self_service = post(
        service.clone(),
        "/api/v1/workflow-runs",
        &bearer(&pool, &keys, mechanic, "MECHANIC", branch).await,
        json!({
            "definition_id": approval_definition,
            "trigger_type": "MANUAL",
            "idempotency_key": "start-selfservice-0001",
            "input_payload": {}
        }),
    )
    .await;
    assert_eq!(
        self_service.status,
        StatusCode::OK,
        "{:?}",
        self_service.json
    );
    assert_eq!(self_service.json["run"]["status"], "WAITING");

    // An oversize payload is rejected (422) regardless of start authority.
    let oversize = post(
        service,
        "/api/v1/workflow-runs",
        &bearer(&pool, &keys, admin, "SUPER_ADMIN", branch).await,
        json!({
            "definition_id": approval_definition,
            "trigger_type": "MANUAL",
            "idempotency_key": "start-oversize-000001",
            "input_payload": { "blob": "x".repeat(70_000) }
        }),
    )
    .await;
    assert_eq!(
        oversize.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        oversize.json
    );
}

// ===========================================================================
// 9. Security M3: personal inbox OPEN tasks are scoped by role + ownership.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn assignee_me_open_tasks_scoped_by_role_and_ownership(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let definition_id = seed_approval_definition(&pool, "approval.m3").await;

    // An OPEN `initiator` finalize task carries the broad `approval_finalize`
    // policy but is addressed to the run's initiator by OWNERSHIP, not policy.
    let owner = UserId::new();
    seed_user(&pool, owner, "SUPER_ADMIN", branch).await;
    let (_run_id, owner_task) = seed_run_with_open_task(
        &pool,
        definition_id,
        owner,
        "initiator",
        Some("approval_finalize"),
    )
    .await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    // The owner sees their own initiator task.
    let owner_inbox = get(
        service.clone(),
        "/api/v1/workflow-tasks?assignee=me&status=OPEN",
        &bearer(&pool, &keys, owner, "SUPER_ADMIN", branch).await,
    )
    .await;
    assert_eq!(owner_inbox.status, StatusCode::OK, "{:?}", owner_inbox.json);
    assert!(
        inbox_contains(&owner_inbox.json, owner_task),
        "owner must see their own initiator task: {:?}",
        owner_inbox.json
    );

    // A DIFFERENT user who ALSO holds approval_finalize (org-wide) must NOT see the
    // owner's initiator task in their personal inbox — ownership, not policy, is the
    // fence. This is the exact over-share the policy layer alone cannot close.
    let other_admin = UserId::new();
    seed_user(&pool, other_admin, "SUPER_ADMIN", branch).await;
    let other_inbox = get(
        service.clone(),
        "/api/v1/workflow-tasks?assignee=me&status=OPEN",
        &bearer(&pool, &keys, other_admin, "SUPER_ADMIN", branch).await,
    )
    .await;
    assert_eq!(other_inbox.status, StatusCode::OK, "{:?}", other_inbox.json);
    assert!(
        !inbox_contains(&other_inbox.json, owner_task),
        "a non-initiator admin must not see the owner's initiator task: {:?}",
        other_inbox.json
    );

    // A MECHANIC holds no authority role and owns nothing → empty personal inbox
    // even though an OPEN authority task exists.
    let (_hr_run, hr_task) = seed_run_with_open_task(
        &pool,
        definition_id,
        owner,
        "hr_reviewer",
        Some("approval_review"),
    )
    .await;
    let mechanic = UserId::new();
    seed_user(&pool, mechanic, "MECHANIC", branch).await;
    let mechanic_inbox = get(
        service.clone(),
        "/api/v1/workflow-tasks?assignee=me&status=OPEN",
        &bearer(&pool, &keys, mechanic, "MECHANIC", branch).await,
    )
    .await;
    assert_eq!(mechanic_inbox.status, StatusCode::OK);
    assert!(
        !inbox_contains(&mechanic_inbox.json, hr_task),
        "a mechanic must not see an hr_reviewer OPEN task: {:?}",
        mechanic_inbox.json
    );

    // A completion_review holder DOES hold the hr_reviewer authority role → the
    // hr_reviewer OPEN task surfaces in their personal inbox.
    let admin_inbox = get(
        service,
        "/api/v1/workflow-tasks?assignee=me&status=OPEN",
        &bearer(&pool, &keys, other_admin, "SUPER_ADMIN", branch).await,
    )
    .await;
    assert_eq!(admin_inbox.status, StatusCode::OK);
    assert!(
        inbox_contains(&admin_inbox.json, hr_task),
        "a completion_review holder must see the hr_reviewer OPEN task via authority: {:?}",
        admin_inbox.json
    );
}

// ===========================================================================
// 9b. Bulk approval inbox: authorization filtering happens before page shape.
// ==========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn bulk_approval_inbox_keysets_past_interspersed_denied_tasks(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let approver = UserId::new();
    seed_user(&pool, approver, "SUPER_ADMIN", branch).await;
    let definition_id = seed_approval_definition(&pool, "approval.bulk.keyset").await;
    let base = OffsetDateTime::now_utc() - Duration::hours(1);

    // 205 authorized rows are deliberately interspersed with 205 rows that
    // this endpoint must omit. A raw 200-row source page therefore cannot be
    // used as a public page or count without creating a hidden-data side
    // channel or a trailing empty client page.
    for index in 0..410_i64 {
        let required_policy = if index % 2 == 0 {
            Some("approval_decide")
        } else {
            Some("approval_review")
        };
        let (_run_id, task_id) = seed_run_with_open_task(
            &pool,
            definition_id,
            approver,
            if index % 2 == 0 {
                "manager_approver"
            } else {
                "hr_reviewer"
            },
            required_policy,
        )
        .await;
        sqlx::query("UPDATE workflow_waiting_tasks SET created_at = $2 WHERE id = $1")
            .bind(task_id)
            .bind(base + Duration::milliseconds(index))
            .execute(&pool)
            .await
            .unwrap();
    }

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&pool, &keys, approver, "SUPER_ADMIN", branch).await;
    let first = get(
        service.clone(),
        "/api/v1/approval-inbox/bulk-tasks?limit=200",
        &token,
    )
    .await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.json);
    assert_eq!(first.json["items"].as_array().map(Vec::len), Some(200));
    assert!(
        first.json.get("total").is_none(),
        "denied rows must not affect a public total"
    );
    let cursor = first.json["next_cursor"]
        .as_str()
        .expect("five authorized rows remain");
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("limit", "200")
        .append_pair("cursor", cursor)
        .finish();
    let second = get(
        service,
        &format!("/api/v1/approval-inbox/bulk-tasks?{query}"),
        &token,
    )
    .await;
    assert_eq!(second.status, StatusCode::OK, "{:?}", second.json);
    assert_eq!(second.json["items"].as_array().map(Vec::len), Some(5));
    assert_eq!(
        second.json["has_more"], false,
        "no denied-only trailing page"
    );
    assert!(second.json["next_cursor"].is_null());
    assert!(
        second.json["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["bulk_decision"]["decidable"] == true),
        "the endpoint must authorize/filter before exposing every public row: {:?}",
        second.json
    );
}

// ===========================================================================
// 10. Security L5: an over-long decision comment is a 422.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn decide_comment_over_limit_is_rejected(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool).await;
    let actor = UserId::new();
    seed_user(&pool, actor, "SUPER_ADMIN", branch).await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    // The comment bound is enforced before any task lookup, so any task id serves.
    let rejected = post(
        service,
        &format!("/api/v1/workflow-tasks/{}/decide", Uuid::new_v4()),
        &bearer(&pool, &keys, actor, "SUPER_ADMIN", branch).await,
        json!({
            "decision": "approve",
            "comment": "x".repeat(4001),
            "idempotency_key": "decide-comment-cap-0001"
        }),
    )
    .await;
    assert_eq!(
        rejected.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        rejected.json
    );
    assert!(
        rejected.json["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("4000 characters"),
        "{:?}",
        rejected.json
    );
}

// ---------------------------------------------------------------------------
// Fixtures + helpers.
// ---------------------------------------------------------------------------

fn inbox_contains(body: &Value, task_id: Uuid) -> bool {
    body["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .any(|item| item["task_id"] == json!(task_id.to_string()))
        })
        .unwrap_or(false)
}

/// A `wf.exec.v1` definition with a top-level `start_policy` (security M2): an
/// object_gate entry feeding a completion_review human task. Only a
/// completion_review-holder may START it. Seeded ACTIVE / PUBLISHED.
async fn seed_start_policy_definition(pool: &PgPool, workflow_key: &str) -> Uuid {
    let org = OrgId::knl();
    let definition = json!({
        "schema_version": "wf.exec.v1",
        "workflow_key": workflow_key,
        "start_policy": "completion_review",
        "nodes": [
            { "node_key": "capture", "node_type": "object_gate", "title": "Capture" },
            { "node_key": "review.admin", "node_type": "human_task", "title": "Admin review",
              "assignee_role_key": "admin", "required_policy": "completion_review" }
        ],
        "edges": [ { "from": "capture", "to": "review.admin" } ]
    });
    let definition_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_definitions \
             (org_id, workflow_key, display_name, object_type, status, latest_version, active_version) \
         VALUES ($1, $2, 'Start-Policy Pipeline', 'work_order', 'ACTIVE', 1, 1) \
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
         VALUES ($1, $2, 1, 'PUBLISHED', $3, FALSE, FALSE)",
    )
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(definition)
    .execute(pool)
    .await
    .unwrap();
    definition_id
}

/// Seed a WAITING run with a single OPEN waiting task (no node_run), for the
/// security inbox/decide tests. `required_policy = None` seeds a policy-less
/// legacy row.
async fn seed_run_with_open_task(
    pool: &PgPool,
    definition_id: Uuid,
    initiated_by: UserId,
    assignee_role_key: &str,
    required_policy: Option<&str>,
) -> (Uuid, Uuid) {
    let org = OrgId::knl();
    let run_id = Uuid::new_v4();
    let task_id = Uuid::new_v4();
    let suffix = run_id.simple().to_string();
    sqlx::query(
        "INSERT INTO workflow_runs \
             (id, org_id, definition_id, definition_version, status, trigger_type, \
              idempotency_key, correlation_id, initiated_by) \
         VALUES ($1, $2, $3, 1, 'WAITING', 'MANUAL', $4, $5, $6)",
    )
    .bind(run_id)
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(format!("seed-run-idem-{suffix}"))
    .bind(format!("seed-run-corr-{suffix}"))
    .bind(*initiated_by.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_waiting_tasks \
             (id, org_id, run_id, waiting_key, title, status, assignee_role_key, required_policy) \
         VALUES ($1, $2, $3, 'wait.node', 'Seed task', 'OPEN', $4, $5)",
    )
    .bind(task_id)
    .bind(*org.as_uuid())
    .bind(run_id)
    .bind(assignee_role_key)
    .bind(required_policy)
    .execute(pool)
    .await
    .unwrap();
    (run_id, task_id)
}

// ---------------------------------------------------------------------------

/// A linear approval definition: submit (gate) → review.hr → approve.manager →
/// finalize.author. Seeded ACTIVE with one PUBLISHED wf.exec.v1 version.
async fn seed_approval_definition(pool: &PgPool, workflow_key: &str) -> Uuid {
    let org = OrgId::knl();
    let definition = json!({
        "schema_version": "wf.exec.v1",
        "workflow_key": workflow_key,
        "nodes": [
            { "node_key": "submit", "node_type": "object_gate", "title": "Submit" },
            { "node_key": "review.hr", "node_type": "human_task", "title": "HR review",
              "assignee_role_key": "hr_reviewer", "required_policy": "approval_review" },
            { "node_key": "approve.manager", "node_type": "human_task", "title": "Manager approval",
              "assignee_role_key": "manager_approver", "required_policy": "approval_decide" },
            { "node_key": "finalize.author", "node_type": "human_task", "title": "Author finalize",
              "assignee_role_key": "initiator", "required_policy": "approval_finalize" }
        ],
        "edges": [
            { "from": "submit", "to": "review.hr" },
            { "from": "review.hr", "to": "approve.manager" },
            { "from": "approve.manager", "to": "finalize.author" }
        ]
    });
    let definition_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_definitions \
             (org_id, workflow_key, display_name, object_type, status, latest_version, active_version) \
         VALUES ($1, $2, 'Instance Approval', 'approval_document', 'ACTIVE', 1, 1) \
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

async fn count_runs(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow_runs")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn governance_finding_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM governance_findings")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn run_status(pool: &PgPool, run_id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM workflow_runs WHERE id = $1")
        .bind(Uuid::parse_str(run_id).unwrap())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn post(service: axum::Router, uri: &str, token: &str, body: Value) -> JsonResponse {
    send(service, "POST", uri, token, Some(body)).await
}

async fn get(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    send(service, "GET", uri, token, None).await
}

async fn send(
    service: axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> JsonResponse {
    let mut builder = Request::builder()
        .uri(uri)
        .method(method)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let request = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        builder.body(Body::from(body.to_string())).unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
}

async fn seed_branch(pool: &PgPool) -> BranchId {
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind("Instance Region")
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind("Instance Branch")
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(pool: &PgPool, user_id: UserId, role: &str, _branch_id: BranchId) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("instance-{role}-{}", user_id.as_uuid()))
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

async fn bearer(
    pool: &PgPool,
    keys: &Keys,
    user_id: UserId,
    role: &str,
    branch: BranchId,
) -> String {
    issue_token(
        pool,
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
        user_id,
        vec![role.to_owned()],
        vec![branch],
    )
    .await
    .unwrap()
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
