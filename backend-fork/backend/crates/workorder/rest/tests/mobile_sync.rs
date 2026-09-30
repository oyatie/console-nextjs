#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_storage::{
    CopyObjectRequest, EvidenceService, ObjectHead, PresignGetRequest, PresignPutRequest,
    PresignedUpload, RetentionInfo, S3ObjectStore, StorageFuture,
};
use console_platform_test_support::runtime_role_pool;
use console_workorder_adapter_postgres::PgWorkOrderStore;
use console_workorder_rest::{MobileRestState, mobile_router};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;
use time::macros::datetime;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

/// The mobile wire contract serializes `created_at` as an RFC 3339 string.
fn created_at_value(dt: OffsetDateTime) -> Value {
    Value::String(dt.format(&Rfc3339).unwrap())
}

#[path = "../../../../test_support/mobile_evidence_fixtures.rs"]
#[allow(dead_code)]
mod mobile_evidence_fixtures;

use mobile_evidence_fixtures::{
    seed_assigned_work_order, seed_branch, seed_crashed_sync_request, seed_equipment,
    seed_user_with_branch,
};

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const DEVICE_ID: &str = "test-device-0001";

#[derive(Debug, Clone)]
struct StaticObjectStore;

impl S3ObjectStore for StaticObjectStore {
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

    fn presign_get(&self, request: PresignGetRequest) -> StorageFuture<'_, String> {
        Box::pin(async move {
            Ok(format!(
                "http://storage.local/{}/{}?X-Amz-Signature=test",
                request.bucket, request.key
            ))
        })
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
                object_lock_mode: Some("COMPLIANCE".to_owned()),
                retain_until: Some("2026-06-13T00:00:00Z".to_owned()),
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
                mode: Some("COMPLIANCE".to_owned()),
                retain_until: Some("2026-06-13T00:00:00Z".to_owned()),
            })
        })
    }

    fn get_object(&self, _bucket: String, _key: String) -> StorageFuture<'_, Vec<u8>> {
        Box::pin(async { Ok(b"original".to_vec()) })
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
    work_order_id: uuid::Uuid,
    pool: PgPool,
}

async fn harness(pool: PgPool) -> Harness {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Sync Region", "Sync Branch").await;
    let mechanic = UserId::new();
    let receptionist = UserId::new();
    seed_user_with_branch(&pool, mechanic, "MECHANIC", branch_id).await;
    seed_user_with_branch(&pool, receptionist, "RECEPTIONIST", branch_id).await;
    let equipment_id = seed_equipment(&pool, branch_id, "501").await;
    let work_order_id =
        seed_assigned_work_order(&pool, branch_id, equipment_id, receptionist, mechanic).await;
    let token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        mechanic,
        vec!["MECHANIC".to_owned()],
        vec![branch_id],
    )
    .unwrap();
    let verifier = JwtVerifier::from_es256_public_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        public_key_pem.as_bytes(),
    )
    .unwrap();
    let rt_pool = runtime_role_pool(&pool).await;
    let evidence = EvidenceService::new(
        rt_pool.clone(),
        StaticObjectStore,
        "primary".to_owned(),
        "replica".to_owned(),
    );
    let service = mobile_router(MobileRestState::new(
        rt_pool.clone(),
        PgWorkOrderStore::new(rt_pool),
        Some(verifier),
        Some(evidence),
    ));
    Harness {
        service,
        token,
        work_order_id: *work_order_id.as_uuid(),
        pool,
    }
}

// FIX 1: same request_id + same payload returns the cached (idempotent) response.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn replay_same_payload_returns_cached_response(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let h = harness(pool).await;
        let body = json!({
            "sync_id": "sync-1",
            "operations": [{
                "request_id": "op-1",
                "operation": "WORK_ORDER_START",
                "created_at": created_at_value(datetime!(2026-06-12 09:00:00 UTC)),
                "payload": { "work_order_id": h.work_order_id }
            }]
        });

        let first = post_sync(h.service.clone(), &h.token, body.clone()).await;
        assert_eq!(first.status, StatusCode::OK, "{:?}", first.json);
        assert_eq!(first.json["results"][0]["status"], "APPLIED");
        assert_eq!(first.json["results"][0]["replayed"], false);

        let second = post_sync(h.service.clone(), &h.token, body).await;
        assert_eq!(second.status, StatusCode::OK, "{:?}", second.json);
        assert_eq!(second.json["results"][0]["status"], "APPLIED");
        assert_eq!(second.json["results"][0]["replayed"], true);
        assert_eq!(
            first.json["results"][0]["result"],
            second.json["results"][0]["result"]
        );

        // The work order transitioned exactly once.
        let history: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_order_status_history WHERE work_order_id = $1 AND action = 'work_order.start'",
        )
        .bind(h.work_order_id)
        .fetch_one(&h.pool)
        .await
        .unwrap();
        assert_eq!(history, 1);
    })
    .await;
}

// FIX 1: same request_id with a DIFFERENT payload is rejected (no stale return).
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn replay_different_payload_is_rejected(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let h = harness(pool).await;
        let other_wo = uuid::Uuid::new_v4();
        let first = post_sync(
            h.service.clone(),
            &h.token,
            json!({
                "sync_id": "sync-1",
                "operations": [{
                    "request_id": "op-1",
                    "operation": "WORK_ORDER_START",
                    "created_at": created_at_value(datetime!(2026-06-12 09:00:00 UTC)),
                    "payload": { "work_order_id": h.work_order_id }
                }]
            }),
        )
        .await;
        assert_eq!(first.json["results"][0]["status"], "APPLIED");

        let mismatch = post_sync(
            h.service.clone(),
            &h.token,
            json!({
                "sync_id": "sync-1",
                "operations": [{
                    "request_id": "op-1",
                    "operation": "WORK_ORDER_START",
                    "created_at": created_at_value(datetime!(2026-06-12 09:00:00 UTC)),
                    "payload": { "work_order_id": other_wo }
                }]
            }),
        )
        .await;
        assert_eq!(mismatch.json["results"][0]["status"], "FAILED");
        assert_eq!(mismatch.json["results"][0]["http_status"], 409);
        assert_eq!(mismatch.json["results"][0]["error"]["code"], "conflict");
        assert!(
            mismatch.json["results"][0]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("different"),
            "{:?}",
            mismatch.json
        );
        // The stale response was NOT returned.
        assert!(mismatch.json["results"][0]["result"].is_null());
    })
    .await;
}

// FIX 1: a duplicate request_id within a single batch is rejected.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn duplicate_request_id_in_batch_is_rejected(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let h = harness(pool).await;
        let resp = post_sync(
            h.service.clone(),
            &h.token,
            json!({
                "sync_id": "sync-1",
                "operations": [
                    {
                        "request_id": "dup",
                        "operation": "WORK_ORDER_START",
                        "created_at": created_at_value(datetime!(2026-06-12 09:00:00 UTC)),
                        "payload": { "work_order_id": h.work_order_id }
                    },
                    {
                        "request_id": "dup",
                        "operation": "WORK_ORDER_REPORT",
                        "created_at": created_at_value(datetime!(2026-06-12 09:01:00 UTC)),
                        "payload": {
                            "work_order_id": h.work_order_id,
                            "result_type": "COMPLETED",
                            "diagnosis": "x",
                            "action_taken": "y"
                        }
                    }
                ]
            }),
        )
        .await;
        assert_eq!(resp.status, StatusCode::OK, "{:?}", resp.json);
        assert_eq!(resp.json["results"][0]["status"], "APPLIED");
        assert_eq!(resp.json["results"][1]["status"], "FAILED");
        assert_eq!(resp.json["results"][1]["http_status"], 409);
        assert!(
            resp.json["results"][1]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("duplicate request_id"),
            "{:?}",
            resp.json
        );
    })
    .await;
}

// An over-large /sync batch is rejected (422) before any allocation/replay so a
// single principal cannot monopolize a pooled DB connection.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn oversized_sync_batch_is_rejected(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let h = harness(pool).await;
        let operations: Vec<Value> = (0..201)
            .map(|i| {
                json!({
                    "request_id": format!("op-{i}"),
                    "operation": "WORK_ORDER_START",
                    "created_at": created_at_value(datetime!(2026-06-12 09:00:00 UTC)),
                    "payload": { "work_order_id": h.work_order_id }
                })
            })
            .collect();
        let resp = post_sync(
            h.service.clone(),
            &h.token,
            json!({ "sync_id": "sync-big", "operations": operations }),
        )
        .await;
        assert_eq!(
            resp.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{:?}",
            resp.json
        );
        assert!(
            resp.json["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("maximum of 200"),
            "{:?}",
            resp.json
        );

        // Nothing was replayed: no sync rows were created for the oversized batch.
        let synced: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM offline_sync_requests WHERE sync_id = 'sync-big'",
        )
        .fetch_one(&h.pool)
        .await
        .unwrap();
        assert_eq!(synced, 0);
    })
    .await;
}

// FIX 2: a crash between the business mutation commit and the completion mark
// leaves an IN_PROGRESS sync row; a retry must reconcile it to the correct final
// response without double-mutating.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn crash_between_mutate_and_complete_reconciles_on_retry(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let h = harness(pool).await;
        let store = PgWorkOrderStore::new(h.pool.clone());

        // Simulate the committed business mutation: the WO is started.
        store
            .start_work(console_workorder_application::WorkOrderStartCommand {
                actor: assigned_mechanic(&h.pool, h.work_order_id).await,
                work_order_id: console_kernel_core::WorkOrderId::from_uuid(h.work_order_id),
                trace: console_kernel_core::TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();

        // Simulate the crashed sync claim: IN_PROGRESS row, payload hash set, no
        // response body (completion never ran). The fixture computes the same
        // canonical hash the REST layer derives from the retried operation.
        let mechanic = assigned_mechanic(&h.pool, h.work_order_id).await;
        let created_at = datetime!(2026-06-12 09:00:00 UTC);
        let device_hash = hex::encode(Sha256::digest(DEVICE_ID.as_bytes()));
        let payload = json!({ "work_order_id": h.work_order_id });
        seed_crashed_sync_request(
            &h.pool,
            mechanic,
            &device_hash,
            "op-crash",
            "sync-crash",
            "WORK_ORDER_START",
            created_at,
            &payload,
        )
        .await;

        // Retry the same operation: it must reconcile to the started summary, not
        // double-mutate or return an empty/in-progress response.
        let retry = post_sync(
            h.service.clone(),
            &h.token,
            json!({
                "sync_id": "sync-crash",
                "operations": [{
                    "request_id": "op-crash",
                    "operation": "WORK_ORDER_START",
                    "created_at": created_at_value(created_at),
                    "payload": payload
                }]
            }),
        )
        .await;
        assert_eq!(retry.status, StatusCode::OK, "{:?}", retry.json);
        assert_eq!(
            retry.json["results"][0]["status"], "APPLIED",
            "{:?}",
            retry.json
        );
        assert_eq!(retry.json["results"][0]["replayed"], true);
        assert_eq!(
            retry.json["results"][0]["result"]["status"], "IN_PROGRESS",
            "{:?}",
            retry.json
        );

        // The mutation applied exactly once (no double start).
        let started_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_order_status_history WHERE work_order_id = $1 AND to_status = 'IN_PROGRESS'",
        )
        .bind(h.work_order_id)
        .fetch_one(&h.pool)
        .await
        .unwrap();
        assert_eq!(started_count, 1, "start must have applied exactly once");

        // The sync row is now finalized with a response body.
        let (status, has_body): (String, bool) = sqlx::query_as(
            "SELECT status, response_body IS NOT NULL FROM offline_sync_requests WHERE request_id = 'op-crash'",
        )
        .fetch_one(&h.pool)
        .await
        .unwrap();
        assert_eq!(status, "APPLIED");
        assert!(has_body);
    })
    .await;
}

async fn assigned_mechanic(pool: &PgPool, work_order_id: uuid::Uuid) -> UserId {
    let id: uuid::Uuid = sqlx::query_scalar(
        "SELECT mechanic_id FROM work_order_assignments WHERE work_order_id = $1 LIMIT 1",
    )
    .bind(work_order_id)
    .fetch_one(pool)
    .await
    .unwrap();
    UserId::from_uuid(id)
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

async fn post_sync(service: axum::Router, token: &str, body: Value) -> JsonResponse {
    let response = service
        .oneshot(
            Request::builder()
                .uri("/api/v1/sync")
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

fn issue_token(
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

    Ok(issuer.issue_access_token(AccessTokenInput {
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
    })?)
}
