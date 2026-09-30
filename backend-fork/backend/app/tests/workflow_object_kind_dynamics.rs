#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-AUTO slice 2 E2E — object-type-bound blocks (dynamics↔ontology) over the
//! REAL router on a genuine non-owner `console_rt` pool (RLS enforced), JWT-authed.
//!
//! Proves:
//!   * a trigger binding declares a `subject_kind` validated against the global
//!     object_types registry (unknown kind → 422; deny-by-omission for a
//!     non-manager → 403);
//!   * a definition's `object_kinds` chain must reference registered kinds
//!     (unknown → 422);
//!   * GET .../definitions/by-object-kind/{kind} returns the definitions (by
//!     primary object_type OR declared object_kinds) and the bindings scoped to
//!     that kind — the explore "작용 자동화" panel source.

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

async fn seed_user(pool: &PgPool, user_id: UserId, role: &str) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("dyn-{role}-{}", user_id.as_uuid()))
        .bind(vec![role])
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

fn bearer(keys: &Keys, user_id: UserId, role: &str) -> String {
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
    issuer
        .issue_access_token(AccessTokenInput {
            subject: user_id,
            org_id: OrgId::knl(),
            roles: vec![role.to_owned()],
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
        })
        .unwrap()
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

async fn post(service: axum::Router, uri: &str, token: &str, body: Value) -> JsonResponse {
    send(service, "POST", uri, token, Some(body)).await
}
async fn get(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    send(service, "GET", uri, token, None).await
}

fn exec_definition(object_kinds: Value) -> Value {
    json!({
        "schema_version": "wf.exec.v1",
        "object_kinds": object_kinds,
        "nodes": [{ "node_key": "gate", "node_type": "object_gate" }],
        "edges": []
    })
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn subject_kind_binding_and_by_object_kind_panel(pool: PgPool) {
    let keys = keys();
    let admin = UserId::new();
    seed_user(&pool, admin, "SUPER_ADMIN").await;
    let non_manager = UserId::new();
    seed_user(&pool, non_manager, "MECHANIC").await;
    let service = build_router(app_state(
        runtime_role_pool(&pool).await,
        keys.public_pem.clone(),
    ));
    let admin_token = bearer(&keys, admin, "SUPER_ADMIN");
    let tech_token = bearer(&keys, non_manager, "MECHANIC");

    // Create a definition whose primary object_type is work_order.
    let created = post(
        service.clone(),
        "/api/v1/workflow-studio/definitions",
        &admin_token,
        json!({
            "workflow_key": "automation.on_wo",
            "display_name": "On work order",
            "object_type": "work_order",
            "definition": exec_definition(json!(["work_order"]))
        }),
    )
    .await;
    assert_eq!(created.status, StatusCode::OK, "{:?}", created.json);
    let definition_id = created.json["id"].as_str().unwrap().to_owned();
    assert_eq!(created.json["object_kinds"][0], "work_order");

    // Bind with a subject_kind that exists → 200, echoed back.
    let bound = post(
        service.clone(),
        "/api/v1/workflow-studio/trigger-bindings",
        &admin_token,
        json!({
            "definition_id": definition_id,
            "trigger_type": "OBJECT_EVENT",
            "event_key": "work_order.completed",
            "subject_kind": "work_order"
        }),
    )
    .await;
    assert_eq!(bound.status, StatusCode::OK, "{:?}", bound.json);
    assert_eq!(bound.json["subject_kind"], "work_order");

    // A subject_kind that is not a registered object type → 422 (FK-backed).
    let bad = post(
        service.clone(),
        "/api/v1/workflow-studio/trigger-bindings",
        &admin_token,
        json!({
            "definition_id": definition_id,
            "trigger_type": "OBJECT_EVENT",
            "event_key": "work_order.completed",
            "subject_kind": "ghost_kind"
        }),
    )
    .await;
    assert_eq!(
        bad.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        bad.json
    );

    // The panel: definitions + bindings touching work_order.
    let panel = get(
        service.clone(),
        "/api/v1/workflow-studio/definitions/by-object-kind/work_order",
        &admin_token,
    )
    .await;
    assert_eq!(panel.status, StatusCode::OK, "{:?}", panel.json);
    assert_eq!(panel.json["kind"], "work_order");
    let defs = panel.json["definitions"].as_array().unwrap();
    assert!(defs.iter().any(|d| d["id"] == definition_id));
    let bindings = panel.json["bindings"].as_array().unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0]["subject_kind"], "work_order");

    // deny-by-omission: a non-manager cannot read the automation panel.
    let denied = get(
        service,
        "/api/v1/workflow-studio/definitions/by-object-kind/work_order",
        &tech_token,
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{:?}", denied.json);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn definition_object_kinds_must_exist_and_drive_the_panel(pool: PgPool) {
    let keys = keys();
    let admin = UserId::new();
    seed_user(&pool, admin, "SUPER_ADMIN").await;
    let service = build_router(app_state(
        runtime_role_pool(&pool).await,
        keys.public_pem.clone(),
    ));
    let token = bearer(&keys, admin, "SUPER_ADMIN");

    // An object_kinds chain referencing an unregistered kind is rejected.
    let bad = post(
        service.clone(),
        "/api/v1/workflow-studio/definitions",
        &token,
        json!({
            "workflow_key": "automation.bad_chain",
            "display_name": "Bad chain",
            "object_type": "work_order",
            "definition": exec_definition(json!(["work_order", "ghost_kind"]))
        }),
    )
    .await;
    assert_eq!(
        bad.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        bad.json
    );

    // A chain of registered kinds is accepted; the panel finds it by a
    // non-primary kind in the chain (object_type=work_order, chain touches person).
    let ok = post(
        service.clone(),
        "/api/v1/workflow-studio/definitions",
        &token,
        json!({
            "workflow_key": "automation.good_chain",
            "display_name": "Good chain",
            "object_type": "work_order",
            "definition": exec_definition(json!(["work_order", "person"]))
        }),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.json);
    let definition_id = ok.json["id"].as_str().unwrap().to_owned();

    let panel = get(
        service,
        "/api/v1/workflow-studio/definitions/by-object-kind/person",
        &token,
    )
    .await;
    assert_eq!(panel.status, StatusCode::OK, "{:?}", panel.json);
    let defs = panel.json["definitions"].as_array().unwrap();
    assert!(
        defs.iter().any(|d| d["id"] == definition_id),
        "the panel must surface a definition whose object_kinds chain touches the kind"
    );
}
