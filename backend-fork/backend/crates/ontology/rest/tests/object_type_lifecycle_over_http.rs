#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The object engine, driven end to end over HTTP with ZERO Rust written per
//! type: author a type -> review -> a second principal approves -> publish ->
//! execute the auto-attached generic `create` action -> read the instance back.
//!
//! Everything here goes through `console_ontology_rest::router` merged with
//! `console_governance_rest::router`, exactly as `build_router` merges them
//! (`backend/app/src/lib.rs:3241-3253`). Nothing calls a store directly, because
//! the point of the test is that a *client* can do this.
//!
//! The store is wired `PgOntologyStore::new(rt).with_command_pool(cmd)`: object-
//! TYPE writes execute `ontology_api.*`, whose EXECUTE is granted only to
//! `console_ontology_cmd` (0165_ontology_object_type_key_revisions.sql:1232-1236).
//! Reads and instance writes stay on the non-superuser `console_rt`.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use console_governance_adapter_postgres::PgGovernanceStore;
use console_governance_rest::GovernanceRestState;
use console_kernel_core::{OrgId, UserId};
use console_ontology_adapter_postgres::PgOntologyStore;
use console_ontology_adapter_postgres::instances::PgInstanceStore;
use console_ontology_rest::OntologyRestState;
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_test_support::{runtime_role_pool, seed_org_and_super_admin};
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

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// The command pool. Without it every object-TYPE write is 503
/// `ontology_command_unavailable` — the same failure a deployment that never
/// sets `ONTOLOGY_COMMAND_DATABASE_URL` would produce.
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

fn settings() -> JwtSettings {
    JwtSettings {
        issuer: ISSUER.to_owned(),
        audience: AUDIENCE.to_owned(),
        access_token_ttl: Duration::minutes(15),
    }
}

fn token(issuer: &JwtIssuer, subject: UserId, org: OrgId) -> String {
    issuer
        .issue_access_token(AccessTokenInput {
            subject,
            org_id: org,
            roles: vec!["SUPER_ADMIN".to_owned()],
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
        .expect("issue access token")
}

struct Http {
    service: Router,
    /// The principal that authors, requests approval and publishes.
    admin: String,
    /// A DISTINCT principal — `gov_approvals` has `CHECK (approver_id <>
    /// requested_by)` (0153_create_governance.sql:74), so this is load-bearing.
    approver: String,
    actor: UserId,
}

struct Res {
    status: StatusCode,
    etag: Option<String>,
    cache_control: Option<String>,
    body: Value,
}

impl Res {
    fn etag(&self) -> String {
        self.etag
            .clone()
            .unwrap_or_else(|| panic!("no ETag on {} {}", self.status, self.body))
    }

    fn code(&self) -> &str {
        self.body["error"]["code"].as_str().unwrap_or("<none>")
    }
}

impl Http {
    async fn send(
        &self,
        method: &str,
        uri: &str,
        token: &str,
        if_match: Option<&str>,
        body: Value,
    ) -> Res {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        if let Some(etag) = if_match {
            builder = builder.header(header::IF_MATCH, etag);
        }
        let payload = if body == Value::Null {
            Body::empty()
        } else {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&body).unwrap())
        };
        let response = self
            .service
            .clone()
            .oneshot(builder.body(payload).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let header_value = |name: header::HeaderName| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(ToOwned::to_owned)
        };
        let etag = header_value(header::ETAG);
        let cache_control = header_value(header::CACHE_CONTROL);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&bytes) }));
        Res {
            status,
            etag,
            cache_control,
            body,
        }
    }

    /// Author a fresh draft object type.
    async fn author(&self, stable_key: &str, actions: Value) -> Res {
        let res = self
            .send(
                "POST",
                "/api/v1/ontology/object-types",
                &self.admin,
                None,
                json!({
                    "stable_key": stable_key,
                    "title": "핸드오버 정책",
                    "title_property_key": "policy_name",
                    "backing_kind": "instance",
                    "properties": [{
                        "key": "policy_name",
                        "title": "정책명",
                        "field_type": "text",
                        "config": {},
                        "required": true
                    }],
                    "links": [],
                    "actions": actions,
                    "analytics": []
                }),
            )
            .await;
        assert_eq!(res.status, StatusCode::CREATED, "author: {}", res.body);
        res
    }

    async fn lifecycle(&self, key: &str, etag: &str, to_state: &str) -> Res {
        self.send(
            "POST",
            &format!("/api/v1/ontology/object-types/{key}/lifecycle"),
            &self.admin,
            Some(etag),
            json!({ "to_state": to_state }),
        )
        .await
    }

    /// The four-eyes half of publishing: the SAME actor requests, a DISTINCT one
    /// decides. `payload_summary.key_revision` must be the revision AFTER the
    /// draft -> review_pending bump (0165:1003 re-reads it at publish time).
    async fn approve_publish_of(&self, type_id: &str, key_revision: i64) {
        let request_ref = Uuid::new_v4();
        let requested = self
            .send(
                "POST",
                "/api/v1/governance/approvals",
                &self.admin,
                None,
                json!({
                    "request_ref": request_ref,
                    "kind": "ontology.schema.publish",
                    "target_ref": type_id,
                    "payload_summary": { "key_revision": key_revision }
                }),
            )
            .await;
        assert_eq!(
            requested.status,
            StatusCode::CREATED,
            "approval request: {}",
            requested.body
        );
        let decided = self
            .send(
                "POST",
                "/api/v1/governance/approvals/decide",
                &self.approver,
                None,
                json!({
                    "request_ref": request_ref,
                    "kind": "ontology.schema.publish",
                    "requested_by": self.actor.as_uuid().to_string(),
                    "decision": "approved"
                }),
            )
            .await;
        assert_eq!(
            decided.status,
            StatusCode::CREATED,
            "approval decision: {}",
            decided.body
        );
    }

    /// draft -> review_pending -> (four eyes) -> published. Returns the ETag
    /// left behind by the publish.
    async fn publish(&self, key: &str, type_id: &str, draft_etag: &str) -> String {
        let reviewed = self.lifecycle(key, draft_etag, "review_pending").await;
        assert_eq!(
            reviewed.status,
            StatusCode::OK,
            "draft -> review_pending: {}",
            reviewed.body
        );
        let key_revision = reviewed.body["key_write_revision"].as_i64().unwrap();
        self.approve_publish_of(type_id, key_revision).await;
        let published = self.lifecycle(key, &reviewed.etag(), "published").await;
        assert_eq!(
            published.status,
            StatusCode::OK,
            "review_pending -> published: {}",
            published.body
        );
        assert_eq!(published.body["lifecycle_state"], json!("published"));
        published.etag()
    }
}

async fn build(owner_pool: &PgPool) -> Http {
    let org = OrgId::knl();
    let actor = seed_org_and_super_admin(owner_pool, *org.as_uuid(), "lifecycle-actor").await;
    let approver = seed_org_and_super_admin(owner_pool, *org.as_uuid(), "lifecycle-approver").await;
    let rt = runtime_role_pool(owner_pool).await;
    let cmd = command_role_pool(owner_pool).await;

    // ONE keypair: two would make the second principal's token a 401, which
    // reads like an authz failure and is not one.
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let issuer =
        JwtIssuer::from_es256_pem(settings(), private_pem.as_bytes(), public_pem.as_bytes())
            .unwrap();
    let verifier = JwtVerifier::from_es256_public_pem(settings(), public_pem.as_bytes()).unwrap();

    let ontology = console_ontology_rest::router(OntologyRestState::new(
        PgOntologyStore::new(rt.clone()).with_command_pool(cmd),
        PgInstanceStore::new(rt.clone()),
        PgGovernanceStore::new(rt.clone()),
        Some(verifier.clone()),
    ));
    let governance = console_governance_rest::router(GovernanceRestState::new(
        PgGovernanceStore::new(rt.clone()),
        Some(verifier),
    ));

    Http {
        service: ontology.merge(governance),
        admin: token(&issuer, actor, org),
        approver: token(&issuer, approver, org),
        actor,
    }
}

// ---------------------------------------------------------------------------
// The acceptance path: a type authored, published and instantiated over HTTP.
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_type_is_authored_published_and_instantiated_over_http(owner_pool: PgPool) {
    let http = build(&owner_pool).await;

    let created = http.author("handover_policy", json!([])).await;
    let type_id = created.body["id"].as_str().unwrap().to_owned();
    assert_eq!(created.body["lifecycle_state"], json!("draft"));

    http.publish("handover_policy", &type_id, &created.etag())
        .await;

    // The generic `create` action is auto-attached by publishing (0165:1024-1042).
    // No Rust was written for this type; the client only ever spoke HTTP.
    let executed = http
        .send(
            "POST",
            "/api/v1/ontology/actions/create/execute",
            &http.admin,
            None,
            json!({
                "object_type_id": type_id,
                "title": "HO-1",
                "params": { "policy_name": "야간 인수인계" },
                "command_id": Uuid::new_v4()
            }),
        )
        .await;
    assert_eq!(
        executed.status,
        StatusCode::OK,
        "execute: {}",
        executed.body
    );
    // `ExecuteOutcome.instance` is an `InstanceState`, itself `{instance, revision}`.
    let instance_id = executed.body["instance"]["instance"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no instance id in {}", executed.body))
        .to_owned();

    let read = http
        .send(
            "GET",
            &format!("/api/v1/ontology/instances/{instance_id}"),
            &http.admin,
            None,
            Value::Null,
        )
        .await;
    // This type has no object policy attached, so it is invisible on EVERY read
    // path, not merely the list. The by-id assertion flipped from 200 to 404 when
    // `get_instance` started applying the same object-policy residual the list
    // applies; a denied row is deliberately indistinguishable from a nonexistent
    // one, because a 403 would make the status code an existence oracle.
    assert_eq!(
        read.status,
        StatusCode::NOT_FOUND,
        "get instance: {}",
        read.body
    );

    // NOT a baseline pin and NOT pending: `list_instances` lowers every object
    // policy to a residual and an empty permit set is `deny_all()`
    // (authz/src/cedar_pbac/residual.rs:200-203), so a type with no attached
    // permit lists nothing. That is deny-by-default working, and this assertion
    // is correct forever. Making it return rows would mean the engine assumed a
    // permit the org never authored.
    let listed = http
        .send(
            "GET",
            &format!("/api/v1/ontology/instances?type={type_id}"),
            &http.admin,
            None,
            Value::Null,
        )
        .await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(
        listed.body,
        json!([]),
        "no attached permit means no rows: deny-by-default, not a pending fix"
    );
}

// ---------------------------------------------------------------------------
// Every edge the route claims to support, and every edge it does not.
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn every_legal_edge_is_reachable_over_http(owner_pool: PgPool) {
    let http = build(&owner_pool).await;
    let created = http.author("edges", json!([])).await;
    let type_id = created.body["id"].as_str().unwrap().to_owned();

    // draft -> review_pending -> draft (a reviewer sends it back; no second
    // principal needed for this edge).
    let reviewed = http
        .lifecycle("edges", &created.etag(), "review_pending")
        .await;
    assert_eq!(reviewed.status, StatusCode::OK, "{}", reviewed.body);
    let back = http.lifecycle("edges", &reviewed.etag(), "draft").await;
    assert_eq!(
        back.status,
        StatusCode::OK,
        "review_pending -> draft: {}",
        back.body
    );
    assert_eq!(back.body["lifecycle_state"], json!("draft"));

    // review_pending -> published, then published -> superseded -> retired.
    let etag = http.publish("edges", &type_id, &back.etag()).await;
    let superseded = http.lifecycle("edges", &etag, "superseded").await;
    assert_eq!(
        superseded.status,
        StatusCode::OK,
        "published -> superseded: {}",
        superseded.body
    );
    let retired = http.lifecycle("edges", &superseded.etag(), "retired").await;
    assert_eq!(
        retired.status,
        StatusCode::OK,
        "superseded -> retired: {}",
        retired.body
    );
    assert_eq!(retired.body["lifecycle_state"], json!("retired"));
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn published_to_retired_is_reachable_without_superseding(owner_pool: PgPool) {
    let http = build(&owner_pool).await;
    let created = http.author("straight_to_retired", json!([])).await;
    let type_id = created.body["id"].as_str().unwrap().to_owned();
    let etag = http
        .publish("straight_to_retired", &type_id, &created.etag())
        .await;
    let retired = http
        .lifecycle("straight_to_retired", &etag, "retired")
        .await;
    assert_eq!(
        retired.status,
        StatusCode::OK,
        "published -> retired: {}",
        retired.body
    );
    assert_eq!(retired.body["lifecycle_state"], json!("retired"));
}

/// The FSM's one rejected forward edge, plus the edges that skip review entirely.
/// A 500 here would mean the SQL guard reached the client unmapped.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn illegal_edges_are_mapped_409s_not_500s(owner_pool: PgPool) {
    let http = build(&owner_pool).await;
    let created = http.author("short_circuit", json!([])).await;
    let etag = created.etag();

    for to in ["published", "superseded", "retired"] {
        let res = http.lifecycle("short_circuit", &etag, to).await;
        assert_eq!(
            res.status,
            StatusCode::CONFLICT,
            "draft -> {to}: {}",
            res.body
        );
        assert_eq!(
            res.code(),
            "invalid_transition",
            "draft -> {to}: {}",
            res.body
        );
    }
}

// ---------------------------------------------------------------------------
// The four-eyes ladder is load-bearing, not ceremony.
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn publishing_without_an_approval_is_403(owner_pool: PgPool) {
    let http = build(&owner_pool).await;
    let created = http.author("no_approval", json!([])).await;
    let reviewed = http
        .lifecycle("no_approval", &created.etag(), "review_pending")
        .await;
    assert_eq!(reviewed.status, StatusCode::OK, "{}", reviewed.body);

    let published = http
        .lifecycle("no_approval", &reviewed.etag(), "published")
        .await;
    assert_eq!(
        published.status,
        StatusCode::FORBIDDEN,
        "publish without four eyes: {}",
        published.body
    );
    assert_eq!(published.code(), "forbidden");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn the_requester_cannot_approve_its_own_publish(owner_pool: PgPool) {
    let http = build(&owner_pool).await;
    let created = http.author("self_approve", json!([])).await;
    let type_id = created.body["id"].as_str().unwrap().to_owned();
    let reviewed = http
        .lifecycle("self_approve", &created.etag(), "review_pending")
        .await;
    let key_revision = reviewed.body["key_write_revision"].as_i64().unwrap();

    let request_ref = Uuid::new_v4();
    let requested = http
        .send(
            "POST",
            "/api/v1/governance/approvals",
            &http.admin,
            None,
            json!({
                "request_ref": request_ref,
                "kind": "ontology.schema.publish",
                "target_ref": type_id,
                "payload_summary": { "key_revision": key_revision }
            }),
        )
        .await;
    assert_eq!(requested.status, StatusCode::CREATED, "{}", requested.body);

    // Same principal decides its own request.
    let decided = http
        .send(
            "POST",
            "/api/v1/governance/approvals/decide",
            &http.admin,
            None,
            json!({
                "request_ref": request_ref,
                "kind": "ontology.schema.publish",
                "requested_by": http.actor.as_uuid().to_string(),
                "decision": "approved"
            }),
        )
        .await;
    assert_eq!(
        decided.status,
        StatusCode::FORBIDDEN,
        "self-approval: {}",
        decided.body
    );

    let published = http
        .lifecycle("self_approve", &reviewed.etag(), "published")
        .await;
    assert_eq!(
        published.status,
        StatusCode::FORBIDDEN,
        "publish on a self-approval: {}",
        published.body
    );
}

// ---------------------------------------------------------------------------
// The If-Match CAS.
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn the_write_precondition_is_enforced_on_this_route_too(owner_pool: PgPool) {
    let http = build(&owner_pool).await;
    let created = http.author("cas", json!([])).await;
    let etag = created.etag();
    let uri = "/api/v1/ontology/object-types/cas/lifecycle";
    let body = json!({ "to_state": "review_pending" });

    let missing = http
        .send("POST", uri, &http.admin, None, body.clone())
        .await;
    assert_eq!(
        missing.status,
        StatusCode::PRECONDITION_REQUIRED,
        "{}",
        missing.body
    );
    assert_eq!(missing.code(), "ontology_write_precondition_required");

    for bad in [
        "W/\"ont-object-type-key:00000000000000000000000000000000:r1\"",
        "*",
        "not-a-validator",
    ] {
        let malformed = http
            .send("POST", uri, &http.admin, Some(bad), body.clone())
            .await;
        assert_eq!(
            malformed.status,
            StatusCode::BAD_REQUEST,
            "{bad}: {}",
            malformed.body
        );
        assert_eq!(malformed.code(), "invalid_ontology_write_precondition");
    }

    // Spend the validator, then replay it.
    let ok = http.lifecycle("cas", &etag, "review_pending").await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    let stale = http.lifecycle("cas", &etag, "draft").await;
    assert_eq!(
        stale.status,
        StatusCode::PRECONDITION_FAILED,
        "{}",
        stale.body
    );
    assert_eq!(stale.code(), "ontology_write_precondition_failed");
    assert_eq!(stale.body["error"]["current_key_write_revision"], json!(2));
    assert_eq!(
        stale.etag(),
        ok.etag(),
        "412 must carry the CURRENT validator"
    );
    assert_eq!(stale.cache_control.as_deref(), Some("no-store"));

    // And the state did not move.
    let read = http
        .send(
            "GET",
            "/api/v1/ontology/object-types/cas",
            &http.admin,
            None,
            Value::Null,
        )
        .await;
    assert_eq!(
        read.body["object_type"]["lifecycle_state"],
        json!("review_pending")
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_unknown_key_is_404_not_403(owner_pool: PgPool) {
    let http = build(&owner_pool).await;
    let created = http.author("known", json!([])).await;
    let res = http
        .lifecycle("no_such_type", &created.etag(), "review_pending")
        .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND, "{}", res.body);
    assert_eq!(res.code(), "not_found");
}

// ---------------------------------------------------------------------------
// `?version=`: without it a type freezes at v1 forever.
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_revision_staged_behind_a_published_head_needs_the_version_selector(owner_pool: PgPool) {
    let http = build(&owner_pool).await;
    let created = http.author("versioned", json!([])).await;
    let v1_id = created.body["id"].as_str().unwrap().to_owned();
    let etag = http.publish("versioned", &v1_id, &created.etag()).await;

    // Stage v2 behind the published v1.
    let staged = http
        .send(
            "PUT",
            "/api/v1/ontology/object-types/versioned",
            &http.admin,
            Some(&etag),
            json!({
                "stable_key": "versioned",
                "title": "핸드오버 정책 v2",
                "title_property_key": "policy_name",
                "backing_kind": "instance",
                "properties": [{
                    "key": "policy_name",
                    "title": "정책명",
                    "field_type": "text",
                    "config": {},
                    "required": true
                }],
                "links": [],
                "actions": [],
                "analytics": []
            }),
        )
        .await;
    assert_eq!(
        staged.status,
        StatusCode::CREATED,
        "stage v2: {}",
        staged.body
    );
    let v2_id = staged.body["id"].as_str().unwrap().to_owned();
    assert_ne!(v1_id, v2_id);
    assert_eq!(staged.body["schema_version"], json!(2));

    // Key-only addressing resolves the PUBLISHED head (adapter-postgres:624-626),
    // i.e. v1 — and published -> review_pending is not a legal edge.
    let key_only = http
        .lifecycle("versioned", &staged.etag(), "review_pending")
        .await;
    assert_eq!(
        key_only.status,
        StatusCode::CONFLICT,
        "key-only addressing must have hit published v1: {}",
        key_only.body
    );

    // Same call, same validator, `?version=2` — the only difference.
    let versioned = http
        .send(
            "POST",
            "/api/v1/ontology/object-types/versioned/lifecycle?version=2",
            &http.admin,
            Some(&staged.etag()),
            json!({ "to_state": "review_pending" }),
        )
        .await;
    assert_eq!(
        versioned.status,
        StatusCode::OK,
        "?version=2: {}",
        versioned.body
    );
    assert_eq!(versioned.body["id"], json!(v2_id));
    assert_eq!(versioned.body["lifecycle_state"], json!("review_pending"));
}

// ---------------------------------------------------------------------------
// Client-supplied stable keys are a trust boundary.
// ---------------------------------------------------------------------------

/// A client-authored `create` action with `dispatch = "projected_usecase"` does
/// NOT suppress the auto-attach (0165:1024-1029 keys on `instance_revision`), so
/// it collides on `UNIQUE (object_type_id, stable_key)` inside the publish
/// transaction. That must reach the client as a 409, never a 500.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_client_authored_create_action_collides_as_409(owner_pool: PgPool) {
    let http = build(&owner_pool).await;
    let created = http
        .author(
            "collider",
            json!([{
                "stable_key": "create",
                "title": "생성",
                "params_schema": {},
                "edits": [],
                "submission_criteria": [],
                "side_effects": [],
                "dispatch": "projected_usecase",
                "dispatch_target": "registry.update_equipment",
                "control_points": ["authority"]
            }]),
        )
        .await;
    let type_id = created.body["id"].as_str().unwrap().to_owned();

    let reviewed = http
        .lifecycle("collider", &created.etag(), "review_pending")
        .await;
    assert_eq!(reviewed.status, StatusCode::OK, "{}", reviewed.body);
    let key_revision = reviewed.body["key_write_revision"].as_i64().unwrap();
    http.approve_publish_of(&type_id, key_revision).await;

    let published = http
        .lifecycle("collider", &reviewed.etag(), "published")
        .await;
    assert_eq!(
        published.status,
        StatusCode::CONFLICT,
        "action-key collision: {}",
        published.body
    );
    assert_eq!(published.code(), "conflict");
    // ESCALATED, not fixed here: the message is "resource already exists" and
    // names neither the action key nor the constraint, because the 23505 arm is
    // generic (rest/src/lib.rs:1823-1828). Naming it belongs in
    // `adapter-postgres::ontology_database_kernel_error`, a different crate.
    assert_eq!(
        published.body["error"]["message"],
        json!("resource already exists")
    );
}
