#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use axum::body::{Body, to_bytes};
use console_kernel_core::{AuditAction, AuditEvent, BranchId, OrgId, TraceContext, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_db::{DbError, with_audit};
use console_platform_jobs::{BoxFuture, JobId, JobQueue, JobQueueError, JobRequest};
use console_platform_storage::{
    CopyObjectRequest, EvidenceService, ObjectHead, PresignGetRequest, PresignPutRequest,
    PresignedUpload, RetentionInfo, S3ObjectStore, StorageError, StorageFuture,
};
use console_platform_test_support::runtime_role_pool;
use console_workorder_adapter_postgres::PgWorkOrderStore;
use console_workorder_rest::{MobileRestState, mobile_router};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

#[path = "../../../../test_support/mobile_evidence_fixtures.rs"]
#[allow(dead_code)]
mod mobile_evidence_fixtures;

use mobile_evidence_fixtures::{
    seed_assigned_work_order, seed_branch, seed_equipment, seed_terminal_work_order,
    seed_user_with_branch,
};

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

#[derive(Debug, Clone)]
struct StaticObjectStore;

impl S3ObjectStore for StaticObjectStore {
    fn presign_put(&self, request: PresignPutRequest) -> StorageFuture<'_, PresignedUpload> {
        Box::pin(async move {
            Ok(PresignedUpload {
                method: "PUT".to_owned(),
                url: format!("http://storage.local/{}/{}", request.bucket, request.key),
                headers: vec![
                    ("content-type".to_owned(), request.content_type),
                    ("content-length".to_owned(), request.size_bytes.to_string()),
                ],
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
                size_bytes: 1024,
                e_tag: Some("\"etag\"".to_owned()),
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
    ) -> Pin<Box<dyn Future<Output = Result<RetentionInfo, StorageError>> + Send + '_>> {
        Box::pin(async {
            Ok(RetentionInfo {
                mode: Some("COMPLIANCE".to_owned()),
                retain_until: Some("2026-06-13T00:00:00Z".to_owned()),
            })
        })
    }

    fn get_object(
        &self,
        _bucket: String,
        _key: String,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, StorageError>> + Send + '_>> {
        Box::pin(async { Ok(b"original".to_vec()) })
    }

    fn put_object(
        &self,
        _bucket: String,
        _key: String,
        _content_type: String,
        _body: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<(), StorageError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn delete_object(
        &self,
        _bucket: String,
        _key: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), StorageError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn evidence_presign_confirm_flow_is_authorized_and_audited(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let signing_key = SigningKey::random(&mut OsRng);
        let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_key_pem = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let branch_id =
            seed_branch(&pool, "Evidence Mobile Region", "Evidence Mobile Branch").await;
        let mechanic = UserId::new();
        let receptionist = UserId::new();
        seed_user_with_branch(&pool, mechanic, "MECHANIC", branch_id).await;
        seed_user_with_branch(&pool, receptionist, "RECEPTIONIST", branch_id).await;
        let equipment_id = seed_equipment(&pool, branch_id, "291").await;
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

        let presign = post_json(
            service.clone(),
            "/api/v1/evidence/presign",
            &token,
            json!({
                "work_order_id": work_order_id,
                "stage": "AFTER",
                "content_type": "image/jpeg",
                "size_bytes": 1024
            }),
        )
        .await;
        assert_eq!(presign.status, StatusCode::OK, "{:?}", presign.json);
        assert_eq!(presign.json["stage"], "AFTER");
        assert_eq!(presign.json["upload"]["method"], "PUT");
        let evidence_id = presign.json["id"].as_str().unwrap();

        let confirm = post_json(
            service,
            &format!("/api/v1/evidence/{evidence_id}/confirm"),
            &token,
            json!({}),
        )
        .await;
        assert_eq!(confirm.status, StatusCode::OK, "{:?}", confirm.json);
        assert_eq!(confirm.json["stage"], "AFTER");
        assert_eq!(confirm.json["worm_replica_status"], "VERIFIED");
        let response_verified_at =
            OffsetDateTime::parse(confirm.json["verified_at"].as_str().unwrap(), &Rfc3339).unwrap();

        let (stored_stage, stored_worm_replica_status, stored_verified_at): (
            String,
            String,
            Option<OffsetDateTime>,
        ) = sqlx::query_as(
            "SELECT stage, worm_replica_status, verified_at FROM evidence_media WHERE id = $1",
        )
        .bind(uuid::Uuid::parse_str(evidence_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stored_stage, "AFTER");
        assert_eq!(stored_worm_replica_status, "VERIFIED");
        assert_eq!(stored_verified_at, Some(response_verified_at));

        let audit_rows: Vec<(String, Option<uuid::Uuid>)> = sqlx::query_as(
            "SELECT action, org_id FROM audit_events WHERE target_id = $1 ORDER BY occurred_at, created_at",
        )
        .bind(evidence_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        let actions: Vec<String> = audit_rows.iter().map(|(action, _)| action.clone()).collect();
        assert!(actions.contains(&"evidence.upload".to_owned()));
        assert!(actions.contains(&"evidence.presign".to_owned()));
        assert!(actions.contains(&"evidence.confirm".to_owned()));
        assert!(actions.contains(&"evidence.verify".to_owned()));
        // Every evidence audit row must be tenant-scoped, never the NULL
        // platform tier — regression guard for the missing `.with_org(org)`
        // that let evidence.confirm/evidence.verify leak into org_id IS NULL.
        for (action, org_id) in &audit_rows {
            assert_eq!(
                *org_id,
                Some(*OrgId::knl().as_uuid()),
                "audit row for {action} must carry the tenant org_id, not NULL"
            );
        }

        let confirmed_at: Option<OffsetDateTime> =
            sqlx::query_scalar("SELECT upload_confirmed_at FROM evidence_media WHERE id = $1")
                .bind(uuid::Uuid::parse_str(evidence_id).unwrap())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(confirmed_at.is_some());
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn evidence_confirm_fails_when_post_replication_media_reload_fails(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let signing_key = SigningKey::random(&mut OsRng);
        let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_key_pem = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let branch_id = seed_branch(&pool, "Reload Failure Region", "Reload Failure Branch").await;
        let mechanic = UserId::new();
        let receptionist = UserId::new();
        seed_user_with_branch(&pool, mechanic, "MECHANIC", branch_id).await;
        seed_user_with_branch(&pool, receptionist, "RECEPTIONIST", branch_id).await;
        let equipment_id = seed_equipment(&pool, branch_id, "293").await;
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
        let evidence = EvidenceService::new(
            pool.clone(),
            StaticObjectStore,
            "primary".to_owned(),
            "replica".to_owned(),
        );
        let service = mobile_router(MobileRestState::new(
            pool.clone(),
            PgWorkOrderStore::new(pool.clone()),
            Some(verifier),
            Some(evidence),
        ));

        let presign = post_json(
            service.clone(),
            "/api/v1/evidence/presign",
            &token,
            json!({
                "work_order_id": work_order_id,
                "stage": "AFTER",
                "content_type": "image/jpeg",
                "size_bytes": 1024
            }),
        )
        .await;
        assert_eq!(presign.status, StatusCode::OK, "{:?}", presign.json);
        let evidence_id = presign.json["id"].as_str().unwrap().to_owned();
        install_post_replication_reload_failure(&pool).await;

        let confirm = post_json(
            service,
            &format!("/api/v1/evidence/{evidence_id}/confirm"),
            &token,
            json!({}),
        )
        .await;
        assert_eq!(confirm.status, StatusCode::NOT_FOUND, "{:?}", confirm.json);
        assert_eq!(confirm.json["error"]["code"], "not_found");

        let actions: Vec<String> = sqlx::query_scalar(
            "SELECT action FROM audit_events WHERE target_id = $1 ORDER BY occurred_at, created_at",
        )
        .bind(&evidence_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(actions.contains(&"evidence.confirm".to_owned()));
        assert!(actions.contains(&"evidence.verify".to_owned()));
    })
    .await;
}

// FIX 3 (REST layer): a presign request for AFTER evidence on a terminal work
// order is rejected with a 409-class error and no evidence/audit rows persist.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn presign_after_evidence_rejected_on_final_completed_work_order(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let signing_key = SigningKey::random(&mut OsRng);
        let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_key_pem = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let branch_id = seed_branch(&pool, "Terminal WORM Region", "Terminal WORM Branch").await;
        let mechanic = UserId::new();
        let receptionist = UserId::new();
        seed_user_with_branch(&pool, mechanic, "MECHANIC", branch_id).await;
        seed_user_with_branch(&pool, receptionist, "RECEPTIONIST", branch_id).await;
        let equipment_id = seed_equipment(&pool, branch_id, "292").await;
        let work_order_id = seed_terminal_work_order(
            &pool,
            branch_id,
            equipment_id,
            receptionist,
            mechanic,
            "FINAL_COMPLETED",
        )
        .await;
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

        let presign = post_json(
            service,
            "/api/v1/evidence/presign",
            &token,
            json!({
                "work_order_id": work_order_id,
                "stage": "AFTER",
                "content_type": "image/jpeg",
                "size_bytes": 1024
            }),
        )
        .await;
        assert_eq!(presign.status, StatusCode::CONFLICT, "{:?}", presign.json);
        assert_eq!(presign.json["error"]["code"], "conflict");
        assert!(
            presign.json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("terminal"),
            "{:?}",
            presign.json
        );

        let media_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM evidence_media WHERE work_order_id = $1")
                .bind(*work_order_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(media_count, 0);
    })
    .await;
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

async fn post_json(service: axum::Router, uri: &str, token: &str, body: Value) -> JsonResponse {
    let response = service
        .oneshot(
            Request::builder()
                .uri(uri)
                .method("POST")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
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

async fn install_post_replication_reload_failure(pool: &PgPool) {
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.install_reload_failure").unwrap(),
        "test_trigger",
        "test_delete_evidence_after_replication",
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(OrgId::knl());
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                r#"
                CREATE OR REPLACE FUNCTION test_delete_evidence_after_replication()
                RETURNS trigger
                LANGUAGE plpgsql
                AS $$
                BEGIN
                    DELETE FROM evidence_media WHERE id = NEW.id;
                    RETURN NULL;
                END;
                $$
                "#,
            )
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            sqlx::query(
                "DROP TRIGGER IF EXISTS test_delete_evidence_after_replication ON evidence_media",
            )
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            sqlx::query(
                r#"
                CREATE CONSTRAINT TRIGGER test_delete_evidence_after_replication
                AFTER UPDATE ON evidence_media
                DEFERRABLE INITIALLY DEFERRED
                FOR EACH ROW
                WHEN (
                    OLD.worm_replica_status IS DISTINCT FROM NEW.worm_replica_status
                    AND NEW.worm_replica_status = 'VERIFIED'
                )
                EXECUTE FUNCTION test_delete_evidence_after_replication()
                "#,
            )
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
}

async fn get_json(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
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
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
}

/// Records the jobs the staging-presign handler enqueues so the test can assert
/// a transcode job is scheduled for the right evidence id.
#[derive(Clone, Default)]
struct RecordingQueue {
    enqueued: Arc<Mutex<Vec<JobRequest>>>,
}

impl JobQueue for RecordingQueue {
    fn enqueue<'a>(&'a self, request: JobRequest) -> BoxFuture<'a, Result<JobId, JobQueueError>> {
        let enqueued = self.enqueued.clone();
        Box::pin(async move {
            let key = request.idempotency_key.as_str().to_owned();
            enqueued.lock().unwrap().push(request);
            Ok(JobId::from_key(key))
        })
    }

    fn schedule_at<'a>(
        &'a self,
        request: JobRequest,
        _scheduled_at: console_kernel_core::Timestamp,
    ) -> BoxFuture<'a, Result<JobId, JobQueueError>> {
        self.enqueue(request)
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn evidence_staging_presign_creates_processing_row_and_enqueues_transcode(pool: PgPool) {
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let signing_key = SigningKey::random(&mut OsRng);
        let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_key_pem = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let branch_id = seed_branch(&pool, "Staging Region", "Staging Branch").await;
        let mechanic = UserId::new();
        let outsider = UserId::new();
        let receptionist = UserId::new();
        seed_user_with_branch(&pool, mechanic, "MECHANIC", branch_id).await;
        seed_user_with_branch(&pool, outsider, "MECHANIC", branch_id).await;
        seed_user_with_branch(&pool, receptionist, "RECEPTIONIST", branch_id).await;
        let equipment_id = seed_equipment(&pool, branch_id, "292").await;
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
        // A different mechanic, NOT assigned to this work order.
        let outsider_token = issue_token(
            private_pem.as_bytes(),
            public_key_pem.as_bytes(),
            outsider,
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
        let queue = RecordingQueue::default();
        let service = mobile_router(
            MobileRestState::new(
                rt_pool.clone(),
                PgWorkOrderStore::new(rt_pool),
                Some(verifier),
                Some(evidence),
            )
            .with_job_queue(Some(Arc::new(queue.clone()))),
        );

        // A non-assigned mechanic is FORBIDDEN.
        let forbidden = post_json(
            service.clone(),
            "/api/v1/evidence/staging-presign",
            &outsider_token,
            json!({
                "work_order_id": work_order_id,
                "stage": "DURING",
                "content_type": "video/quicktime",
                "size_bytes": 5_000_000
            }),
        )
        .await;
        assert_eq!(
            forbidden.status,
            StatusCode::FORBIDDEN,
            "{:?}",
            forbidden.json
        );

        // A disallowed MIME (pdf) is rejected before any presign.
        let bad_mime = post_json(
            service.clone(),
            "/api/v1/evidence/staging-presign",
            &token,
            json!({
                "work_order_id": work_order_id,
                "stage": "DURING",
                "content_type": "application/pdf",
                "size_bytes": 1024
            }),
        )
        .await;
        assert_eq!(
            bad_mime.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{:?}",
            bad_mime.json
        );

        // An oversize video (> 200 MiB) is rejected before any presign.
        let oversize = post_json(
            service.clone(),
            "/api/v1/evidence/staging-presign",
            &token,
            json!({
                "work_order_id": work_order_id,
                "stage": "DURING",
                "content_type": "video/mp4",
                "size_bytes": 300_000_000_i64
            }),
        )
        .await;
        assert_eq!(
            oversize.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{:?}",
            oversize.json
        );

        // The assigned mechanic gets a staging presign + a PROCESSING row.
        let presign = post_json(
            service.clone(),
            "/api/v1/evidence/staging-presign",
            &token,
            json!({
                "work_order_id": work_order_id,
                "stage": "DURING",
                "content_type": "video/quicktime",
                "size_bytes": 5_000_000
            }),
        )
        .await;
        assert_eq!(presign.status, StatusCode::OK, "{:?}", presign.json);
        assert_eq!(presign.json["media_kind"], "VIDEO");
        assert_eq!(presign.json["processing_status"], "PROCESSING");
        assert_eq!(presign.json["upload"]["method"], "PUT");
        let evidence_id = presign.json["id"].as_str().unwrap().to_owned();
        // The presigned URL is TENANT-PREFIXED.
        let org_prefix = format!("orgs/{}/", OrgId::knl().as_uuid());
        assert!(
            presign.json["upload"]["url"]
                .as_str()
                .unwrap()
                .contains(&org_prefix),
            "presigned staging URL must be org-prefixed: {}",
            presign.json["upload"]["url"]
        );

        // A transcode job was enqueued for this evidence id. Snapshot the keys
        // inside a tight scope so the mutex guard is released before any await.
        let enqueued_keys: Vec<String> = {
            let enqueued = queue.enqueued.lock().unwrap();
            enqueued
                .iter()
                .map(|req| req.idempotency_key.as_str().to_owned())
                .collect()
        };
        assert_eq!(enqueued_keys.len(), 1);
        assert_eq!(
            enqueued_keys[0],
            format!("evidence-transcode:{evidence_id}")
        );

        // The status endpoint reports PROCESSING for the assigned mechanic.
        let status = get_json(
            service.clone(),
            &format!("/api/v1/evidence/{evidence_id}/status"),
            &token,
        )
        .await;
        assert_eq!(status.status, StatusCode::OK, "{:?}", status.json);
        assert_eq!(status.json["processing_status"], "PROCESSING");

        // The audit trail recorded the staging presign.
        let actions: Vec<String> = sqlx::query_scalar(
            "SELECT action FROM audit_events WHERE target_id = $1 ORDER BY occurred_at, created_at",
        )
        .bind(&evidence_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(actions.contains(&"evidence.staging".to_owned()));
        assert!(actions.contains(&"evidence.staging.presign".to_owned()));
    })
    .await;
}
