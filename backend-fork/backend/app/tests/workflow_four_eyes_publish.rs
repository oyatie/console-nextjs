#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-AUTO slice 2 E2E — four-eyes definition publish (pendingRev) over the REAL
//! router on a genuine non-owner `console_rt` pool (RLS enforced), JWT-authed, with a
//! REAL passkey step-up (SoftPasskey) satisfying the sensitive-publish gate.
//!
//! Proves:
//!   * a NEW definition (no active version) publishes DIRECTLY (activates);
//!   * publishing a revision to an ALREADY-ACTIVE definition STAGES it (the
//!     active version keeps serving; pending_version is set) instead of applying;
//!   * a SECOND, distinct actor's approve APPLIES the revision (active flips,
//!     pending cleared) and writes NO self-approval finding;
//!   * titles do not permit self-approval; missing staging identity fails closed;
//!   * withdraw discards the staged revision (pending cleared, active unchanged).
//!
//! §16 org-scope automation gate (85 판정, BE-ingest-checklist-gates): direct
//! activation of an org-scope automation additionally requires a distinct
//! four-eyes approval — `create_and_activate` seeds one via
//! `PgGovernanceStore::decide_approval` (mirrors the ontology action lane's
//! test pattern) so the five pre-existing SoD tests above keep exercising a
//! genuine org-scope definition. `org_scope_direct_activate_requires_four_eyes`
//! and `personal_scope_direct_activate_stays_direct` at the bottom of this file
//! prove the gate itself: deny + zero rows without it, admit with it, and a
//! personal-scope (§3.9.0-①) definition skips the gate entirely.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_governance_adapter_postgres::PgGovernanceStore;
use console_governance_application::{ApprovalDecision, DecideApprovalCommand};
use console_kernel_core::{BranchId, OrgId, TraceContext, UserId};
use console_platform_auth::{
    AccessTokenInput, JwtIssuer, JwtSettings, PasskeyRegistrationStart, PasskeyService,
    WebauthnSettings,
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
use url::Url;
use webauthn_authenticator_rs::prelude::{RequestChallengeResponse, WebauthnAuthenticator};
use webauthn_authenticator_rs::softpasskey::SoftPasskey;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const RP_ID: &str = "localhost";
const RP_ORIGIN: &str = "http://localhost";

struct Keys {
    private_pem: String,
    public_pem: String,
}
struct JsonResponse {
    status: StatusCode,
    json: Value,
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

fn passkey_service() -> PasskeyService {
    PasskeyService::new(WebauthnSettings {
        rp_id: RP_ID.to_owned(),
        rp_origin: Url::parse(RP_ORIGIN).unwrap(),
        rp_name: "Console".to_owned(),
        extra_allowed_origins: vec![],
        ceremony_ttl: Duration::minutes(5),
    })
    .unwrap()
}

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(6)
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

fn app_state(pool: PgPool, keys: &Keys) -> AppState {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", keys.public_pem.clone()),
        ("CONSOLE_JWT_PRIVATE_KEY_PEM", keys.private_pem.clone()),
        ("CONSOLE_WEBAUTHN_RP_ID", RP_ID.to_owned()),
        ("CONSOLE_WEBAUTHN_RP_ORIGIN", RP_ORIGIN.to_owned()),
    ])
    .unwrap();
    AppState::new(config, DatabaseDependency::Postgres(pool)).unwrap()
}

async fn seed_super_admin(pool: &PgPool, user_id: UserId, label: &str) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("four-eyes-{label}"))
        .bind(vec!["SUPER_ADMIN"])
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

async fn bearer(pool: &PgPool, keys: &Keys, user_id: UserId) -> String {
    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
    )
    .unwrap();
    console_platform_test_support::issue_session_token(
        pool,
        &issuer,
        AccessTokenInput {
            subject: user_id,
            org_id: OrgId::knl(),
            roles: vec!["SUPER_ADMIN".to_owned()],
            branches: Vec::<BranchId>::new(),
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
    .await
}

/// Inject one allowCredentials entry so the resident-key-less SoftPasskey can
/// locate its key (copied from the auth crate's ceremony harness).
fn inject_allow_credential(
    challenge: RequestChallengeResponse,
    credential_id: &str,
) -> RequestChallengeResponse {
    let mut value = serde_json::to_value(&challenge).unwrap();
    value
        .get_mut("publicKey")
        .and_then(|pk| pk.get_mut("allowCredentials"))
        .and_then(Value::as_array_mut)
        .expect("discoverable challenge must have an allowCredentials array")
        .push(json!({ "type": "public-key", "id": credential_id }));
    serde_json::from_value(value).unwrap()
}

/// Register a discoverable passkey for `user_id`; returns the authenticator (kept
/// for later step-up assertions) and the stored credential id.
async fn register_passkey(
    owner_pool: &PgPool,
    svc: &PasskeyService,
    user_id: UserId,
    username: &str,
) -> (WebauthnAuthenticator<SoftPasskey>, String) {
    let reg = svc
        .start_registration(
            owner_pool,
            OrgId::knl(),
            PasskeyRegistrationStart {
                user_id: *user_id.as_uuid(),
                username: username.to_owned(),
                display_name: username.to_owned(),
            },
        )
        .await
        .unwrap();
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = authenticator
        .do_registration(Url::parse(RP_ORIGIN).unwrap(), reg.challenge)
        .unwrap();
    let stored = svc
        .finish_registration(owner_pool, OrgId::knl(), reg.ceremony_id, credential)
        .await
        .unwrap();
    (authenticator, stored.credential_id)
}

/// Produce a fresh step-up assertion body `{ step_up: { ceremony_id, credential } }`
/// for a request that requires passkey step-up.
async fn step_up(
    owner_pool: &PgPool,
    svc: &PasskeyService,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
    credential_id: &str,
) -> Value {
    let auth = svc.start_authentication(owner_pool).await.unwrap();
    let challenge = inject_allow_credential(auth.challenge, credential_id);
    let assertion = authenticator
        .do_authentication(Url::parse(RP_ORIGIN).unwrap(), challenge)
        .unwrap();
    json!({ "step_up": { "ceremony_id": auth.ceremony_id, "credential": assertion } })
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

fn exec_definition() -> Value {
    exec_definition_scoped("org")
}

/// `exec_definition()` with an explicit `metadata.owner_scope.type` — "org" (the
/// default the gate assumes for a scope-less definition too) or "personal"
/// (§3.9.0-①, skips the direct-activate four-eyes gate).
fn exec_definition_scoped(owner_scope: &str) -> Value {
    json!({
        "schema_version": "wf.exec.v1",
        "metadata": { "owner_scope": { "type": owner_scope } },
        "object_kinds": ["work_order"],
        "nodes": [{ "node_key": "gate", "node_type": "object_gate" }],
        "edges": []
    })
}

/// Seed an approved four-eyes decision (`gov_approvals`, distinct approver) bound
/// to (`kind`, `target`) under `org`, returning its `request_ref` — mirrors the
/// ontology action lane's `PgGovernanceStore::decide_approval` test pattern. The
/// gate now binds the approval to the exact action kind and target object, so the
/// seed must match what the publish/run gate derives server-side (the workflow
/// definition id). The approver must be a real seeded user
/// (`gov_approvals.approver_id` FKs to `users`).
async fn seed_four_eyes_approval(
    owner_pool: &PgPool,
    rt: &PgPool,
    org: OrgId,
    requested_by: UserId,
    kind: &str,
    target: uuid::Uuid,
) -> uuid::Uuid {
    let request_ref = uuid::Uuid::new_v4();
    let approver = UserId::new();
    seed_super_admin(owner_pool, approver, "four-eyes-gate-approver").await;
    console_platform_request_context::scope_org(org, async {
        PgGovernanceStore::new(rt.clone())
            .decide_approval(DecideApprovalCommand {
                approver,
                request_ref,
                kind: kind.to_owned(),
                requested_by,
                target_ref: Some(target),
                decision: ApprovalDecision::Approved,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .expect("record four-eyes approval")
    })
    .await;
    request_ref
}

// ponytail: test-only helper threading the full HTTP+passkey harness plus the
// new §16 four-eyes requester — an args struct would only move the noise, not
// remove it, for a function every existing test call site already threads
// positionally.
#[allow(clippy::too_many_arguments)]
async fn create_and_activate(
    service: &axum::Router,
    owner_pool: &PgPool,
    svc: &PasskeyService,
    token: &str,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
    cred_id: &str,
    workflow_key: &str,
    requester: UserId,
) -> String {
    let created = send(
        service.clone(),
        "POST",
        "/api/v1/workflow-studio/definitions",
        token,
        Some(json!({
            "workflow_key": workflow_key,
            "display_name": "Four eyes",
            "object_type": "work_order",
            "definition": exec_definition()
        })),
    )
    .await;
    assert_eq!(created.status, StatusCode::OK, "{:?}", created.json);
    let id = created.json["id"].as_str().unwrap().to_owned();

    // Direct activate (never-published definition). exec_definition() is
    // org-scope, so the §16 gate needs a distinct four-eyes approval too, bound to
    // THIS definition (the publish gate derives target = the definition id).
    let request_ref = seed_four_eyes_approval(
        owner_pool,
        &runtime_role_pool(owner_pool).await,
        OrgId::knl(),
        requester,
        "workflow.publish",
        id.parse().unwrap(),
    )
    .await;
    let mut body = step_up(owner_pool, svc, authenticator, cred_id).await;
    body["four_eyes_request_ref"] = json!(request_ref);
    let published = send(
        service.clone(),
        "POST",
        &format!("/api/v1/workflow-studio/definitions/{id}/publish"),
        token,
        Some(body),
    )
    .await;
    assert_eq!(published.status, StatusCode::OK, "{:?}", published.json);
    assert_eq!(published.json["status"], "ACTIVE");
    assert!(published.json["active_version"].as_i64().unwrap() >= 1);
    assert!(published.json["pending_version"].is_null());
    id
}

async fn edit_and_stage(
    service: &axum::Router,
    owner_pool: &PgPool,
    svc: &PasskeyService,
    token: &str,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
    cred_id: &str,
    id: &str,
) -> (i64, i64) {
    // Edit the live definition → stages a DRAFT revision, active keeps serving.
    let edited = send(
        service.clone(),
        "PATCH",
        &format!("/api/v1/workflow-studio/definitions/{id}"),
        token,
        Some(json!({ "display_name": "Four eyes (revised)" })),
    )
    .await;
    assert_eq!(edited.status, StatusCode::OK, "{:?}", edited.json);
    assert_eq!(
        edited.json["status"], "ACTIVE",
        "editing a live def must not take it out of service"
    );
    let active_before = edited.json["active_version"].as_i64().unwrap();

    // Publish → STAGE (does not apply).
    let body = step_up(owner_pool, svc, authenticator, cred_id).await;
    let staged = send(
        service.clone(),
        "POST",
        &format!("/api/v1/workflow-studio/definitions/{id}/publish"),
        token,
        Some(body),
    )
    .await;
    assert_eq!(staged.status, StatusCode::OK, "{:?}", staged.json);
    assert_eq!(staged.json["status"], "ACTIVE");
    assert_eq!(
        staged.json["active_version"].as_i64().unwrap(),
        active_before,
        "staging must NOT change the active version"
    );
    let pending = staged.json["pending_version"].as_i64().unwrap();
    assert!(
        pending > active_before,
        "the staged pending revision is a newer version"
    );
    (active_before, pending)
}

/// Start a run over the real router. `version` pins `definition_version`; `None`
/// resolves the definition's active version.
async fn start_run(
    service: &axum::Router,
    token: &str,
    definition_id: &str,
    version: Option<i64>,
    idempotency_key: &str,
) -> JsonResponse {
    let mut body = json!({
        "definition_id": definition_id,
        "trigger_type": "MANUAL",
        "idempotency_key": idempotency_key,
    });
    if let Some(version) = version {
        body["definition_version"] = json!(version);
    }
    send(
        service.clone(),
        "POST",
        "/api/v1/workflow-runs",
        token,
        Some(body),
    )
    .await
}

async fn finding_count(owner_pool: &PgPool, definition_id: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM governance_findings \
         WHERE detector_id = 'anomaly.self_approval' \
           AND entity_type = 'workflow_definition' AND entity_id = $1",
    )
    .bind(definition_id)
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn distinct_approver_applies_staged_revision_without_finding(owner_pool: PgPool) {
    let keys = keys();
    let svc = passkey_service();
    let publisher = UserId::new();
    let approver = UserId::new();
    seed_super_admin(&owner_pool, publisher, "publisher").await;
    seed_super_admin(&owner_pool, approver, "approver").await;
    let (mut pub_auth, pub_cred) =
        register_passkey(&owner_pool, &svc, publisher, "publisher").await;
    let (mut app_auth, app_cred) = register_passkey(&owner_pool, &svc, approver, "approver").await;
    let service = build_router(app_state(runtime_role_pool(&owner_pool).await, &keys));
    let pub_token = bearer(&owner_pool, &keys, publisher).await;
    let app_token = bearer(&owner_pool, &keys, approver).await;

    let id = create_and_activate(
        &service,
        &owner_pool,
        &svc,
        &pub_token,
        &mut pub_auth,
        &pub_cred,
        "four.eyes.distinct",
        publisher,
    )
    .await;
    let (active_before, pending) = edit_and_stage(
        &service,
        &owner_pool,
        &svc,
        &pub_token,
        &mut pub_auth,
        &pub_cred,
        &id,
    )
    .await;

    // A DISTINCT actor approves → applies (active flips, pending cleared).
    let body = step_up(&owner_pool, &svc, &mut app_auth, &app_cred).await;
    let approved = send(
        service.clone(),
        "POST",
        &format!("/api/v1/workflow-studio/definitions/{id}/revisions/{pending}/approve"),
        &app_token,
        Some(body),
    )
    .await;
    assert_eq!(approved.status, StatusCode::OK, "{:?}", approved.json);
    assert_eq!(approved.json["status"], "ACTIVE");
    assert!(
        approved.json["active_version"].as_i64().unwrap() > active_before,
        "approval must flip the active version to the applied revision"
    );
    assert!(approved.json["pending_version"].is_null());
    assert_eq!(
        finding_count(&owner_pool, &id).await,
        0,
        "a distinct approver is not a self-approval — no governance finding"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn titles_and_missing_staging_actor_cannot_approve_revision(owner_pool: PgPool) {
    let keys = keys();
    let svc = passkey_service();
    let publisher = UserId::new();
    seed_super_admin(&owner_pool, publisher, "solo").await;
    let (mut auth, cred) = register_passkey(&owner_pool, &svc, publisher, "solo").await;
    let service = build_router(app_state(runtime_role_pool(&owner_pool).await, &keys));
    let token = bearer(&owner_pool, &keys, publisher).await;
    let id = create_and_activate(
        &service,
        &owner_pool,
        &svc,
        &token,
        &mut auth,
        &cred,
        "four.eyes.solo",
        publisher,
    )
    .await;
    let (active, pending) =
        edit_and_stage(&service, &owner_pool, &svc, &token, &mut auth, &cred, &id).await;
    for (lead, missing) in [(false, false), (true, false), (true, true)] {
        sqlx::query("UPDATE users SET is_org_lead=$2 WHERE id=$1")
            .bind(*publisher.as_uuid())
            .bind(lead)
            .execute(&owner_pool)
            .await
            .unwrap();
        if missing {
            sqlx::query("UPDATE workflow_definitions SET pending_staged_by=NULL WHERE id=$1")
                .bind(uuid::Uuid::parse_str(&id).unwrap())
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        let before = revision_evidence(&owner_pool, &id).await;
        let body = step_up(&owner_pool, &svc, &mut auth, &cred).await;
        let denied = send(
            service.clone(),
            "POST",
            &format!("/api/v1/workflow-studio/definitions/{id}/revisions/{pending}/approve"),
            &token,
            Some(body),
        )
        .await;
        assert_eq!(denied.status, StatusCode::FORBIDDEN, "{:?}", denied.json);
        assert_eq!(revision_evidence(&owner_pool, &id).await, before);
        let states: (Option<i32>, Option<i32>) = sqlx::query_as(
            "SELECT active_version,pending_version FROM workflow_definitions WHERE id=$1",
        )
        .bind(uuid::Uuid::parse_str(&id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(states, (Some(active as i32), Some(pending as i32)));
        assert_eq!(finding_count(&owner_pool, &id).await, 0);
    }
}

async fn revision_evidence(pool: &PgPool, id: &str) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_array(to_jsonb(d),
        (SELECT jsonb_agg(to_jsonb(v) ORDER BY version) FROM workflow_definition_versions v WHERE definition_id=d.id),
        (SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM workflow_definition_events e WHERE definition_id=d.id),
        (SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM audit_events a WHERE target_id=d.id::text AND action='workflow_definition.approve_revision'))
        FROM workflow_definitions d WHERE id=$1")
        .bind(uuid::Uuid::parse_str(id).unwrap()).fetch_one(pool).await.unwrap()
}

/// Security: the start path must resolve ONLY an approved (PUBLISHED) version.
/// A staged pending revision is an unapproved DRAFT — an initiator who pins its
/// version number must be denied (422), or the four-eyes control this PR adds is
/// bypassed. The active version starts normally; the revision becomes startable
/// only after a DISTINCT actor approves (which appends a new PUBLISHED version).
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn start_gates_on_approved_version_only(owner_pool: PgPool) {
    let keys = keys();
    let svc = passkey_service();
    let publisher = UserId::new();
    let approver = UserId::new();
    seed_super_admin(&owner_pool, publisher, "start-publisher").await;
    seed_super_admin(&owner_pool, approver, "start-approver").await;
    let (mut pub_auth, pub_cred) =
        register_passkey(&owner_pool, &svc, publisher, "start-publisher").await;
    let (mut app_auth, app_cred) =
        register_passkey(&owner_pool, &svc, approver, "start-approver").await;
    let service = build_router(app_state(runtime_role_pool(&owner_pool).await, &keys));
    let pub_token = bearer(&owner_pool, &keys, publisher).await;
    let app_token = bearer(&owner_pool, &keys, approver).await;

    let id = create_and_activate(
        &service,
        &owner_pool,
        &svc,
        &pub_token,
        &mut pub_auth,
        &pub_cred,
        "four.eyes.start",
        publisher,
    )
    .await;

    // The active (PUBLISHED) version starts normally (no pin → active version).
    let ok = start_run(&service, &pub_token, &id, None, "start-active-v1-000000").await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.json);
    let active_version = ok.json["run"]["definition_version"].as_i64().unwrap();

    // Stage a revision (pending DRAFT); the active version keeps serving.
    let (active_before, pending) = edit_and_stage(
        &service,
        &owner_pool,
        &svc,
        &pub_token,
        &mut pub_auth,
        &pub_cred,
        &id,
    )
    .await;
    assert_eq!(
        active_before, active_version,
        "staging must not change the active version"
    );
    assert!(pending > active_before, "the staged revision is newer");

    // The initiator pins the pending-staged (unapproved DRAFT) version → DENIED.
    // This is the four-eyes bypass the review flagged; it must be a 422.
    let denied = start_run(
        &service,
        &pub_token,
        &id,
        Some(pending),
        "start-pending-denied-01",
    )
    .await;
    assert_eq!(
        denied.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "starting an unapproved staged revision must be denied: {:?}",
        denied.json
    );

    // Pinning the active (PUBLISHED) version is still allowed (historical pin).
    let pinned = start_run(
        &service,
        &pub_token,
        &id,
        Some(active_before),
        "start-pin-active-v1-01",
    )
    .await;
    assert_eq!(pinned.status, StatusCode::OK, "{:?}", pinned.json);
    assert_eq!(
        pinned.json["run"]["definition_version"].as_i64().unwrap(),
        active_before
    );

    // A DISTINCT actor approves → a new PUBLISHED version becomes active.
    let body = step_up(&owner_pool, &svc, &mut app_auth, &app_cred).await;
    let approved = send(
        service.clone(),
        "POST",
        &format!("/api/v1/workflow-studio/definitions/{id}/revisions/{pending}/approve"),
        &app_token,
        Some(body),
    )
    .await;
    assert_eq!(approved.status, StatusCode::OK, "{:?}", approved.json);
    let new_active = approved.json["active_version"].as_i64().unwrap();
    assert!(
        new_active > active_before,
        "approval flips the active version to the applied revision"
    );

    // The approved revision now starts (default resolves to the new PUBLISHED version).
    let after = start_run(&service, &pub_token, &id, None, "start-after-approve-0001").await;
    assert_eq!(after.status, StatusCode::OK, "{:?}", after.json);
    assert_eq!(
        after.json["run"]["definition_version"].as_i64().unwrap(),
        new_active
    );

    // The staged DRAFT version itself is never startable (append-only, stays DRAFT).
    let still_denied = start_run(
        &service,
        &pub_token,
        &id,
        Some(pending),
        "start-pending-denied-02",
    )
    .await;
    assert_eq!(
        still_denied.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the unapproved DRAFT version is never startable: {:?}",
        still_denied.json
    );
}

/// Regression (round 2 review): the four-eyes gate must apply ONLY to a pinned
/// version that is NOT the current active version. `active_version` is
/// lifecycle-trusted (set only by publish/approve/rollback, all RoleManage +
/// step-up gated) — a rolled-back definition's new active version carries
/// `version_status = 'ROLLED_BACK'`, not `PUBLISHED`, and manual start must
/// still resolve and run it without a pin (the default/unpinned path).
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn rollback_then_manual_start_uses_rolled_back_active_version(owner_pool: PgPool) {
    let keys = keys();
    let svc = passkey_service();
    let actor = UserId::new();
    seed_super_admin(&owner_pool, actor, "rollback-actor").await;
    let (mut auth, cred) = register_passkey(&owner_pool, &svc, actor, "rollback-actor").await;
    let service = build_router(app_state(runtime_role_pool(&owner_pool).await, &keys));
    let token = bearer(&owner_pool, &keys, actor).await;

    let id = create_and_activate(
        &service,
        &owner_pool,
        &svc,
        &token,
        &mut auth,
        &cred,
        "four.eyes.rollback",
        actor,
    )
    .await;
    let v1 = start_run(&service, &token, &id, None, "rollback-before-start-01")
        .await
        .json["run"]["definition_version"]
        .as_i64()
        .unwrap();

    // Stage + approve a second revision so there is a v1 to roll back to.
    let (active_before, pending) =
        edit_and_stage(&service, &owner_pool, &svc, &token, &mut auth, &cred, &id).await;
    assert_eq!(active_before, v1);
    let approver = UserId::new();
    seed_super_admin(&owner_pool, approver, "rollback-approver").await;
    let (mut approval_auth, approval_cred) =
        register_passkey(&owner_pool, &svc, approver, "rollback-approver").await;
    let approval_token = bearer(&owner_pool, &keys, approver).await;
    let body = step_up(&owner_pool, &svc, &mut approval_auth, &approval_cred).await;
    let approved = send(
        service.clone(),
        "POST",
        &format!("/api/v1/workflow-studio/definitions/{id}/revisions/{pending}/approve"),
        &approval_token,
        Some(body),
    )
    .await;
    assert_eq!(approved.status, StatusCode::OK, "{:?}", approved.json);
    let v2 = approved.json["active_version"].as_i64().unwrap();
    assert!(v2 > v1);

    // Roll back to v1: appends a NEW version (ROLLED_BACK status) and flips
    // active_version to it — active is trusted even though not PUBLISHED.
    let body = step_up(&owner_pool, &svc, &mut auth, &cred).await;
    let mut rollback_body = json!({ "target_version": v1 });
    rollback_body["step_up"] = body["step_up"].clone();
    let rolled_back = send(
        service.clone(),
        "POST",
        &format!("/api/v1/workflow-studio/definitions/{id}/rollback"),
        &token,
        Some(rollback_body),
    )
    .await;
    assert_eq!(rolled_back.status, StatusCode::OK, "{:?}", rolled_back.json);
    let v3 = rolled_back.json["active_version"].as_i64().unwrap();
    assert!(v3 > v2, "rollback appends a new version and activates it");

    // Manual (unpinned) start after rollback must succeed — NOT 422 — and bind
    // to the rolled-back active version.
    let after_rollback = start_run(&service, &token, &id, None, "rollback-after-start-01").await;
    assert_eq!(
        after_rollback.status,
        StatusCode::OK,
        "manual start must resolve the rollback-produced active version: {:?}",
        after_rollback.json
    );
    assert_eq!(
        after_rollback.json["run"]["definition_version"]
            .as_i64()
            .unwrap(),
        v3
    );

    // A pin naming that same rolled-back active version also succeeds (it IS
    // the active version, exempt from the PUBLISHED-only check).
    let pinned_active =
        start_run(&service, &token, &id, Some(v3), "rollback-pin-active-0001").await;
    assert_eq!(
        pinned_active.status,
        StatusCode::OK,
        "{:?}",
        pinned_active.json
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn withdraw_discards_staged_revision(owner_pool: PgPool) {
    let keys = keys();
    let svc = passkey_service();
    let publisher = UserId::new();
    seed_super_admin(&owner_pool, publisher, "withdrawer").await;
    let (mut auth, cred) = register_passkey(&owner_pool, &svc, publisher, "withdrawer").await;
    let service = build_router(app_state(runtime_role_pool(&owner_pool).await, &keys));
    let token = bearer(&owner_pool, &keys, publisher).await;

    let id = create_and_activate(
        &service,
        &owner_pool,
        &svc,
        &token,
        &mut auth,
        &cred,
        "four.eyes.withdraw",
        publisher,
    )
    .await;
    let (active_before, pending) =
        edit_and_stage(&service, &owner_pool, &svc, &token, &mut auth, &cred, &id).await;

    // Withdraw (no step-up) clears the pending pointer; active stays put.
    let withdrawn = send(
        service.clone(),
        "POST",
        &format!("/api/v1/workflow-studio/definitions/{id}/revisions/{pending}/withdraw"),
        &token,
        None,
    )
    .await;
    assert_eq!(withdrawn.status, StatusCode::OK, "{:?}", withdrawn.json);
    assert!(withdrawn.json["pending_version"].is_null());
    assert_eq!(
        withdrawn.json["active_version"].as_i64().unwrap(),
        active_before,
        "withdraw must not change the active version"
    );
}

// ===========================================================================
// §16 org-scope automation gate (85 판정, BE-ingest-checklist-gates).
// ===========================================================================

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn org_scope_direct_activate_requires_four_eyes(owner_pool: PgPool) {
    let keys = keys();
    let svc = passkey_service();
    let publisher = UserId::new();
    seed_super_admin(&owner_pool, publisher, "org-gate").await;
    let (mut auth, cred) = register_passkey(&owner_pool, &svc, publisher, "org-gate").await;
    let service = build_router(app_state(runtime_role_pool(&owner_pool).await, &keys));
    let token = bearer(&owner_pool, &keys, publisher).await;

    let created = send(
        service.clone(),
        "POST",
        "/api/v1/workflow-studio/definitions",
        &token,
        Some(json!({
            "workflow_key": "four.eyes.org.gate",
            "display_name": "Org gate",
            "object_type": "work_order",
            "definition": exec_definition_scoped("org")
        })),
    )
    .await;
    assert_eq!(created.status, StatusCode::OK, "{:?}", created.json);
    let id = created.json["id"].as_str().unwrap().to_owned();

    // Publish WITHOUT a four-eyes ref: the §16 gate denies, nothing activates.
    let body = step_up(&owner_pool, &svc, &mut auth, &cred).await;
    let denied = send(
        service.clone(),
        "POST",
        &format!("/api/v1/workflow-studio/definitions/{id}/publish"),
        &token,
        Some(body),
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{:?}", denied.json);

    let listed = send(
        service.clone(),
        "GET",
        "/api/v1/workflow-studio/definitions",
        &token,
        None,
    )
    .await;
    let row = listed.json["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id)
        .expect("definition must still exist");
    assert_eq!(row["status"], "DRAFT", "a denied gate must not activate");
    assert!(
        row["active_version"].is_null(),
        "a denied gate must write zero rows"
    );

    // With an approved four-eyes ref bound to this definition, the same publish
    // call now activates.
    let request_ref = seed_four_eyes_approval(
        &owner_pool,
        &runtime_role_pool(&owner_pool).await,
        OrgId::knl(),
        publisher,
        "workflow.publish",
        id.parse().unwrap(),
    )
    .await;
    let mut body = step_up(&owner_pool, &svc, &mut auth, &cred).await;
    body["four_eyes_request_ref"] = json!(request_ref);
    let published = send(
        service.clone(),
        "POST",
        &format!("/api/v1/workflow-studio/definitions/{id}/publish"),
        &token,
        Some(body),
    )
    .await;
    assert_eq!(published.status, StatusCode::OK, "{:?}", published.json);
    assert_eq!(published.json["status"], "ACTIVE");
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn personal_scope_direct_activate_stays_direct(owner_pool: PgPool) {
    let keys = keys();
    let svc = passkey_service();
    let publisher = UserId::new();
    seed_super_admin(&owner_pool, publisher, "personal-gate").await;
    let (mut auth, cred) = register_passkey(&owner_pool, &svc, publisher, "personal-gate").await;
    let service = build_router(app_state(runtime_role_pool(&owner_pool).await, &keys));
    let token = bearer(&owner_pool, &keys, publisher).await;

    let created = send(
        service.clone(),
        "POST",
        "/api/v1/workflow-studio/definitions",
        &token,
        Some(json!({
            "workflow_key": "four.eyes.personal.gate",
            "display_name": "Personal gate",
            "object_type": "work_order",
            "definition": exec_definition_scoped("personal")
        })),
    )
    .await;
    assert_eq!(created.status, StatusCode::OK, "{:?}", created.json);
    let id = created.json["id"].as_str().unwrap().to_owned();

    // §3.9.0-①: personal-scope skips the four-eyes gate — direct activate with
    // only the mandatory passkey step-up, no four_eyes_request_ref.
    let body = step_up(&owner_pool, &svc, &mut auth, &cred).await;
    let published = send(
        service.clone(),
        "POST",
        &format!("/api/v1/workflow-studio/definitions/{id}/publish"),
        &token,
        Some(body),
    )
    .await;
    assert_eq!(published.status, StatusCode::OK, "{:?}", published.json);
    assert_eq!(published.json["status"], "ACTIVE");
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn org_scope_run_requires_four_eyes(owner_pool: PgPool) {
    let keys = keys();
    let svc = passkey_service();
    let publisher = UserId::new();
    seed_super_admin(&owner_pool, publisher, "run-gate").await;
    let (mut auth, cred) = register_passkey(&owner_pool, &svc, publisher, "run-gate").await;
    let service = build_router(app_state(runtime_role_pool(&owner_pool).await, &keys));
    let token = bearer(&owner_pool, &keys, publisher).await;

    // Direct-activate an org-scope definition (create_and_activate already
    // satisfies its OWN four-eyes gate for publish).
    let id = create_and_activate(
        &service,
        &owner_pool,
        &svc,
        &token,
        &mut auth,
        &cred,
        "four.eyes.run.gate",
        publisher,
    )
    .await;

    // Trigger a run WITHOUT a four-eyes ref: the §16 gate denies, no run row.
    let denied = send(
        service.clone(),
        "POST",
        &format!("/api/v1/workflow-studio/definitions/{id}/run"),
        &token,
        Some(json!({})),
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{:?}", denied.json);
    let runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow_runs WHERE definition_id = $1::uuid")
            .bind(&id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(runs, 0, "a denied run gate must write zero rows");

    // With an approved four-eyes ref bound to this definition, the run triggers.
    let request_ref = seed_four_eyes_approval(
        &owner_pool,
        &runtime_role_pool(&owner_pool).await,
        OrgId::knl(),
        publisher,
        "workflow.run",
        id.parse().unwrap(),
    )
    .await;
    let admitted = send(
        service.clone(),
        "POST",
        &format!("/api/v1/workflow-studio/definitions/{id}/run"),
        &token,
        Some(json!({ "four_eyes_request_ref": request_ref })),
    )
    .await;
    assert_eq!(admitted.status, StatusCode::OK, "{:?}", admitted.json);
    assert_eq!(admitted.json["status"], "RUNNING");
}

/// A run four-eyes ref is SINGLE-USE: once a run consumes it, replaying the same
/// ref on a fresh run is denied — the M1 replay hole closed end-to-end.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn org_scope_run_four_eyes_ref_is_single_use(owner_pool: PgPool) {
    let keys = keys();
    let svc = passkey_service();
    let publisher = UserId::new();
    seed_super_admin(&owner_pool, publisher, "run-replay").await;
    let (mut auth, cred) = register_passkey(&owner_pool, &svc, publisher, "run-replay").await;
    let service = build_router(app_state(runtime_role_pool(&owner_pool).await, &keys));
    let token = bearer(&owner_pool, &keys, publisher).await;

    let id = create_and_activate(
        &service,
        &owner_pool,
        &svc,
        &token,
        &mut auth,
        &cred,
        "four.eyes.run.replay",
        publisher,
    )
    .await;

    let request_ref = seed_four_eyes_approval(
        &owner_pool,
        &runtime_role_pool(&owner_pool).await,
        OrgId::knl(),
        publisher,
        "workflow.run",
        id.parse().unwrap(),
    )
    .await;

    // First run consumes the approval.
    let first = send(
        service.clone(),
        "POST",
        &format!("/api/v1/workflow-studio/definitions/{id}/run"),
        &token,
        Some(json!({ "four_eyes_request_ref": request_ref })),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.json);

    // Replaying the SAME ref on a fresh run is denied (already consumed).
    let replay = send(
        service.clone(),
        "POST",
        &format!("/api/v1/workflow-studio/definitions/{id}/run"),
        &token,
        Some(json!({ "four_eyes_request_ref": request_ref })),
    )
    .await;
    assert_eq!(
        replay.status,
        StatusCode::FORBIDDEN,
        "a consumed four-eyes ref must not admit a second run: {:?}",
        replay.json
    );
    let runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow_runs WHERE definition_id = $1::uuid")
            .bind(&id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(runs, 1, "the replay must have written no second run");
}
