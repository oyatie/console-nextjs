//! `POST /api/v1/devices` — the three branch-scope shapes, through the HANDLER.
//!
//! This route carries no feature gate: resolving the audit branch IS its only
//! authorization check beyond authenticating the token. The unit tests beside
//! `audit_branch_for_principal` prove the helper; they cannot prove the handler
//! calls it, nor that a refusal lands BEFORE the `registered_devices` INSERT.
//! That distinction is this repository's recurring failure — a guard test that
//! calls the store or the helper directly leaves the handler compile-checked
//! only — so all three cases are driven through `mobile_router` here:
//!
//! * `BranchScope::All` → 200, `audit_events.branch_id` NULL (no minted UUID).
//! * `BranchScope::Branches({b})` → 200, `audit_events.branch_id` = b.
//! * `BranchScope::Branches({})` → 403 at the handler, and NO device row.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_storage::{
    CopyObjectRequest, ObjectHead, PresignGetRequest, PresignPutRequest, PresignedUpload,
    RetentionInfo, S3ObjectStore, StorageFuture,
};
use console_platform_test_support::runtime_role_pool;
use console_workorder_adapter_postgres::PgWorkOrderStore;
use console_workorder_rest::{DEVICES_PATH, MobileRestState, mobile_router};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

#[path = "../../../../test_support/mobile_evidence_fixtures.rs"]
#[allow(dead_code)]
mod mobile_evidence_fixtures;

use mobile_evidence_fixtures::{seed_branch, seed_user_with_branch, seed_user_without_branch};

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const DEVICE_ID: &str = "device-registration-0001";

/// `mobile_router` is generic over the evidence object store; device
/// registration never touches it, so every method is a no-op.
#[derive(Debug, Clone)]
struct UnusedObjectStore;

impl S3ObjectStore for UnusedObjectStore {
    fn presign_put(&self, request: PresignPutRequest) -> StorageFuture<'_, PresignedUpload> {
        Box::pin(async move {
            Ok(PresignedUpload {
                method: "PUT".to_owned(),
                url: format!("http://storage.local/{}/{}", request.bucket, request.key),
                headers: vec![],
                expires_in_secs: request.expires_in.as_secs(),
            })
        })
    }

    fn presign_get(&self, _request: PresignGetRequest) -> StorageFuture<'_, String> {
        Box::pin(async { Ok("http://storage.local/get".to_owned()) })
    }

    fn copy_object(&self, _request: CopyObjectRequest) -> StorageFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn head_object(&self, _bucket: String, _key: String) -> StorageFuture<'_, ObjectHead> {
        Box::pin(async {
            Ok(ObjectHead {
                size_bytes: 0,
                e_tag: None,
                checksum_sha256: None,
                object_lock_mode: None,
                retain_until: None,
            })
        })
    }

    fn get_object_retention(
        &self,
        _bucket: String,
        _key: String,
    ) -> StorageFuture<'_, RetentionInfo> {
        Box::pin(async {
            Ok(RetentionInfo {
                mode: None,
                retain_until: None,
            })
        })
    }

    fn get_object(&self, _bucket: String, _key: String) -> StorageFuture<'_, Vec<u8>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn put_object(
        &self,
        _bucket: String,
        _key: String,
        _content_type: String,
        _body: Vec<u8>,
    ) -> StorageFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn delete_object(&self, _bucket: String, _key: String) -> StorageFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

struct Harness {
    service: axum::Router,
    token: String,
}

/// The mobile router on a `console_rt` pool — the role production runs as — plus
/// an access token for `user_id`.
async fn harness(pool: &PgPool, user_id: UserId, role: &str, branches: Vec<BranchId>) -> Harness {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        user_id,
        vec![role.to_owned()],
        branches,
    );
    let verifier = JwtVerifier::from_es256_public_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        public_key_pem.as_bytes(),
    )
    .unwrap();
    let rt_pool = runtime_role_pool(pool).await;
    let service = mobile_router(MobileRestState::<UnusedObjectStore>::new(
        rt_pool.clone(),
        PgWorkOrderStore::new(rt_pool),
        Some(verifier),
        None,
    ));
    Harness { service, token }
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

async fn post_device(service: axum::Router, token: &str) -> JsonResponse {
    let body = json!({
        "platform": "android",
        "app_version": "1.4.0",
        "push_token": "fcm-token"
    });
    let response = service
        .oneshot(
            Request::builder()
                .uri(DEVICES_PATH)
                .method("POST")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-device-id", DEVICE_ID)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
}

/// `BranchScope::All`: authorized, and the audit row's branch stays NULL rather
/// than carrying a minted UUID that belongs to no branch.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn all_scoped_actor_registers_and_the_audit_branch_is_null(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let branch_id = seed_branch(&pool, "Device Region A", "Device Branch A").await;
        let user = UserId::new();
        // SUPER_ADMIN short-circuits the membership read to BranchScope::All.
        seed_user_with_branch(&pool, user, "SUPER_ADMIN", branch_id).await;
        let h = harness(&pool, user, "SUPER_ADMIN", vec![branch_id]).await;

        let response = post_device(h.service, &h.token).await;

        assert_eq!(response.status, StatusCode::OK, "{:?}", response.json);
        assert_eq!(
            audit_branch(&pool, user).await,
            None,
            "an All-scoped actor must leave audit_events.branch_id NULL"
        );
    })
    .await;
}

/// `BranchScope::Branches({b})`: authorized, and the audit row carries the
/// ACTOR's branch.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn member_registers_and_the_audit_row_carries_the_actor_branch(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let branch_id = seed_branch(&pool, "Device Region B", "Device Branch B").await;
        let user = UserId::new();
        seed_user_with_branch(&pool, user, "MECHANIC", branch_id).await;
        let h = harness(&pool, user, "MECHANIC", vec![branch_id]).await;

        let response = post_device(h.service, &h.token).await;

        assert_eq!(response.status, StatusCode::OK, "{:?}", response.json);
        assert_eq!(audit_branch(&pool, user).await, Some(branch_id));
    })
    .await;
}

/// `BranchScope::Branches({})`: REFUSED AT THE HANDLER. A principal with no
/// branch membership belongs nowhere in this org; every other route in the crate
/// denies it (`authorize_capability` rejects an empty scope up front), so this
/// one must too. The device row must not exist afterwards — the refusal has to
/// land before the INSERT, not merely blank an audit column.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn empty_branch_scope_is_refused_at_the_handler(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let user = UserId::new();
        seed_user_without_branch(&pool, user, "MECHANIC").await;
        let h = harness(&pool, user, "MECHANIC", Vec::new()).await;

        let response = post_device(h.service, &h.token).await;

        assert_eq!(
            response.status,
            StatusCode::FORBIDDEN,
            "a principal with no branch membership must not register a device: {:?}",
            response.json
        );
        let devices: i64 =
            sqlx::query_scalar("SELECT count(*) FROM registered_devices WHERE user_id = $1")
                .bind(*user.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(devices, 0, "the refusal must land before the INSERT");
    })
    .await;
}

async fn audit_branch(pool: &PgPool, user: UserId) -> Option<BranchId> {
    let branch: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT branch_id FROM audit_events WHERE actor = $1 AND action = 'device.register'",
    )
    .bind(*user.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    branch.map(BranchId::from_uuid)
}

fn issue_token(
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    roles: Vec<String>,
    branches: Vec<BranchId>,
) -> String {
    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private_key_pem,
        public_key_pem,
    )
    .unwrap();

    issuer
        .issue_access_token(AccessTokenInput {
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
        })
        .unwrap()
}
