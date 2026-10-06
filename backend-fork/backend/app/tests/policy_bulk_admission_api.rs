#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The real policy REST owner rejects oversized batches before any decision write.

use axum::Router;
use axum::body::{Body, to_bytes};
use console_kernel_core::{OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_authz_rest::{CedarPolicyRestState, PgCedarPolicyStore, router};
use console_platform_test_support::{
    issue_session_token, runtime_role_pool, seed_org_and_super_admin,
};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const PATH: &str = "/api/v1/policy/authorize/bulk";
const RESPONSE_LIMIT: usize = 2 * 1024 * 1024;

async fn token(pool: &PgPool, issuer: &JwtIssuer, user: UserId, org: OrgId, role: &str) -> String {
    issue_session_token(
        pool,
        issuer,
        AccessTokenInput {
            subject: user,
            org_id: org,
            roles: vec![role.to_owned()],
            branches: vec![],
            platform: false,
            view_as: false,
            read_only: false,
            display_name: None,
            feature_grants: vec![],
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

fn payload(org: OrgId, user: UserId, count: usize) -> Value {
    json!({
        "subject": { "org": org, "user_id": user.to_string(), "roles": [], "clearance_keys": [] },
        "checks": (0..count).map(|index| json!({
            "action": "view",
            "resource": { "org": org, "resource_type": "work_order", "resource_id": format!("record-{index:03}") }
        })).collect::<Vec<_>>()
    })
}

async fn post(service: &Router, bearer: Option<&str>, body: Value) -> (StatusCode, Value, String) {
    let mut request = Request::builder()
        .method("POST")
        .uri(PATH)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(bearer) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    let response = service
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let content_type = response.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .to_owned();
    let bytes = to_bytes(response.into_body(), RESPONSE_LIMIT)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body, content_type)
}

async fn effects(owner: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM cedar_decision_log), \
         (SELECT count(*) FROM audit_events), \
         (SELECT count(*) FROM workflow_outbox_events)",
    )
    .fetch_one(owner)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn real_bulk_policy_admission_preserves_auth_empty_and_exact_batch_coverage(owner: PgPool) {
    let org = OrgId::from_uuid(Uuid::new_v4());
    let admin = seed_org_and_super_admin(&owner, *org.as_uuid(), "bulk-admin").await;
    let member = seed_org_and_super_admin(&owner, *org.as_uuid(), "bulk-member").await;
    sqlx::query("UPDATE users SET roles=ARRAY['MEMBER'] WHERE id=$1 AND org_id=$2")
        .bind(*member.as_uuid())
        .bind(*org.as_uuid())
        .execute(&owner)
        .await
        .unwrap();
    let runtime = runtime_role_pool(&owner).await;
    let role: (String, bool, bool) = sqlx::query_as(
        "SELECT current_user, rolsuper, rolbypassrls FROM pg_roles WHERE rolname=current_user",
    )
    .fetch_one(&runtime)
    .await
    .unwrap();
    assert_eq!(role, ("console_rt".to_owned(), false, false));

    let signing = SigningKey::random(&mut OsRng);
    let private = signing.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public = signing
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let settings = JwtSettings {
        issuer: "console-platform-auth".to_owned(),
        audience: "console-api".to_owned(),
        access_token_ttl: Duration::minutes(15),
    };
    let issuer =
        JwtIssuer::from_es256_pem(settings.clone(), private.as_bytes(), public.as_bytes()).unwrap();
    let verifier = JwtVerifier::from_es256_public_pem(settings, public.as_bytes()).unwrap();
    let admin_token = token(&runtime, &issuer, admin, org, "SUPER_ADMIN").await;
    let member_token = token(&runtime, &issuer, member, org, "MEMBER").await;
    let service = router(CedarPolicyRestState::new(
        PgCedarPolicyStore::new(runtime.clone()),
        Some(verifier),
    ));
    let before = effects(&owner).await;
    let unavailable = router(CedarPolicyRestState::new(
        PgCedarPolicyStore::new(runtime),
        None,
    ));
    let response = post(&unavailable, Some(&admin_token), payload(org, admin, 0)).await;
    assert_eq!(response.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.2, "text/plain; charset=utf-8");
    assert_eq!(effects(&owner).await, before);

    let missing = post(&service, None, payload(org, admin, 201)).await;
    assert_eq!(missing.0, StatusCode::UNAUTHORIZED);
    assert_eq!(missing.2, "text/plain; charset=utf-8");
    assert_eq!(effects(&owner).await, before);
    let denied = post(&service, Some(&member_token), payload(org, member, 201)).await;
    assert_eq!(denied.0, StatusCode::FORBIDDEN);
    assert_eq!(denied.2, "application/json");
    assert_eq!(effects(&owner).await, before);
    let (status, empty, content_type) =
        post(&service, Some(&admin_token), payload(org, admin, 0)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty, json!({ "decisions": [] }));
    assert_eq!(content_type, "application/json");
    assert_eq!(effects(&owner).await, before);

    let (status, oversized, content_type) =
        post(&service, Some(&admin_token), payload(org, admin, 201)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(oversized["error"]["code"], "validation");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        effects(&owner).await,
        before,
        "rejected batch must write no decisions, audit, or outbox rows"
    );

    let (status, accepted, content_type) =
        post(&service, Some(&admin_token), payload(org, admin, 200)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "application/json");
    let decisions = accepted["decisions"].as_array().unwrap();
    assert_eq!(
        decisions.len(),
        200,
        "every admitted check needs one indexed result"
    );
    assert!(
        decisions
            .iter()
            .all(|decision| decision["effect"] == "deny")
    );
    assert_eq!(effects(&owner).await, (before.0 + 200, before.1, before.2));
    let recorded: Vec<String> = sqlx::query_scalar(
        "SELECT resource_id FROM cedar_decision_log WHERE org_id=$1 AND actor=$2 ORDER BY resource_id",
    )
    .bind(*org.as_uuid())
    .bind(*admin.as_uuid())
    .fetch_all(&owner)
    .await
    .unwrap();
    assert_eq!(
        recorded,
        (0..200)
            .map(|index| format!("record-{index:03}"))
            .collect::<Vec<_>>()
    );
}
