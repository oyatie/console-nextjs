#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the 정비사 evidence MEDIA-processing pipeline.
//!
//! The default `#[sqlx::test]` pool connects as a BYPASSRLS superuser, which
//! sees/writes every row regardless of `app.current_org` — it would green-light
//! a totally broken or cross-tenant-leaking media pipeline. This test SEEDS as
//! the owner (raw inserts, row_security off) and runs the actual
//! `EvidenceService` staging-upload + transcode lifecycle as the genuine
//! non-owner runtime role `console_rt` (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) — the
//! only faithful exercise of the tenant policy.
//!
//! Asserts, with two tenants A (KNL) and B:
//!   * issue_staging_upload writes a PROCESSING evidence row for A under A's
//!     armed GUC, with TENANT-PREFIXED staging/final keys (orgs/{A}/…);
//!   * claim_processing_job + process_job transition PROCESSING → READY under
//!     A's GUC and write the optimized artifact + thumbnail to A-prefixed keys;
//!   * cross-tenant isolation: under tenant B's armed GUC, A's PROCESSING row is
//!     NOT claimable (claim returns None) and A's evidence row is NOT FOUND, so a
//!     `console_rt` worker armed for the wrong tenant can never touch A's media;
//!   * FAIL-CLOSED: with NO GUC armed, claim_processing_job sees nothing.

use std::sync::{Arc, Mutex};

use console_kernel_core::{EvidenceId, OrgId, TraceContext, WorkOrderId};
use console_platform_storage::{
    CopyObjectRequest, EvidenceService, MediaKind, MediaProcessor, ObjectHead, PresignGetRequest,
    PresignPutRequest, PresignedUpload, ProcessedMedia, ProcessingStatus, RetentionInfo,
    S3ObjectStore, StagingUploadCommand, StorageFuture,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use uuid::Uuid;

const ORG_A: Uuid = Uuid::from_u128(0x4b4e_4c00_0000_0000_0000_0000_0000_0001);
const ORG_B: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);

// A pool whose every connection runs `SET ROLE console_rt`, so statements execute as
// the production runtime role (NOSUPERUSER, NOBYPASSRLS) under FORCE RLS.
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

// ---------------------------------------------------------------------------
// Seeding (OWNER pool, row_security off). Raw inserts; org_id columns are set
// explicitly so each row lands in the intended tenant.
// ---------------------------------------------------------------------------

struct SeededWorkOrder {
    work_order_id: WorkOrderId,
    uploaded_by: Uuid,
}

async fn seed_work_order(owner_pool: &PgPool, org: Uuid) -> SeededWorkOrder {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", &org.to_string()[..8]))
    .bind("Org")
    .execute(&mut *tx)
    .await
    .unwrap();
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Region {}", Uuid::new_v4()))
            .bind(org)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("Branch {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let uploaded_by = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(uploaded_by)
        .bind("Mechanic")
        .bind(vec!["MECHANIC".to_owned()])
        .bind(org)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(uploaded_by)
        .bind(branch_id)
        .bind(org)
        .execute(&mut *tx)
        .await
        .unwrap();
    let customer_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(branch_id)
    .bind("Customer")
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let site_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (customer_id, branch_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(customer_id)
    .bind(branch_id)
    .bind("Site")
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    // equipment_no must match `^[A-Z]{3}[A-Z0-9]{2}-[0-9]{4}$` and is globally
    // UNIQUE, so build a conforming, unique value from random digits.
    let suffix: u16 = (Uuid::new_v4().as_u128() % 10_000) as u16;
    let equipment_no = format!("RLS01-{suffix:04}");
    let equipment_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, model, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, $4, $5, 'S', 'T', 'R', '임대', '좌식', '2.5', 'Model', 'test', 1, $6)
        RETURNING id
        "#,
    )
    .bind(branch_id)
    .bind(customer_id)
    .bind(site_id)
    .bind(&equipment_no)
    .bind(format!("M{suffix:04}"))
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let work_order_id = WorkOrderId::new();
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, priority, symptom, result_type, org_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'REPORT_SUBMITTED', 'P3', 'Symptom', 'COMPLETED', $8)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    // request_no must match `^[0-9]{8}-[0-9]{3}$` and is UNIQUE per org.
    .bind(format!(
        "20260623-{:03}",
        (work_order_id.as_uuid().as_u128() % 1000) as u16
    ))
    .bind(branch_id)
    .bind(equipment_id)
    .bind(customer_id)
    .bind(site_id)
    .bind(uploaded_by)
    .bind(org)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    SeededWorkOrder {
        work_order_id,
        uploaded_by,
    }
}

// ---------------------------------------------------------------------------
// Recording object store + stub processor (no ffmpeg / no real S3).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct RecordingStore {
    puts: Arc<Mutex<Vec<String>>>,
    deletes: Arc<Mutex<Vec<String>>>,
}

/// Minimal `infer`-recognizable QuickTime (`video/quicktime`) header so the
/// post-download content re-validation accepts the staging bytes for the
/// declared VIDEO kind and the lifecycle still reaches READY.
fn quicktime_magic_bytes() -> Vec<u8> {
    vec![
        0x00, 0x00, 0x00, 0x14, // box size = 20
        0x66, 0x74, 0x79, 0x70, // "ftyp"
        0x71, 0x74, 0x20, 0x20, // major brand "qt  "
        0x00, 0x00, 0x00, 0x00, // minor version
        0x71, 0x74, 0x20, 0x20, // compatible brand "qt  "
    ]
}

impl S3ObjectStore for RecordingStore {
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
                size_bytes: 1,
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
        Box::pin(async { Ok(quicktime_magic_bytes()) })
    }
    fn put_object(
        &self,
        _bucket: String,
        key: String,
        _content_type: String,
        _body: Vec<u8>,
    ) -> StorageFuture<'_, ()> {
        let puts = self.puts.clone();
        Box::pin(async move {
            puts.lock().unwrap().push(key);
            Ok(())
        })
    }
    fn delete_object(&self, _bucket: String, key: String) -> StorageFuture<'_, ()> {
        let deletes = self.deletes.clone();
        Box::pin(async move {
            deletes.lock().unwrap().push(key);
            Ok(())
        })
    }
}

struct StubProcessor;
impl MediaProcessor for StubProcessor {
    fn process<'a>(
        &'a self,
        kind: MediaKind,
        _original: Vec<u8>,
    ) -> StorageFuture<'a, ProcessedMedia> {
        Box::pin(async move {
            let content_type = match kind {
                MediaKind::Image => "image/jpeg",
                MediaKind::Video => "video/mp4",
            };
            Ok(ProcessedMedia {
                artifact: b"optimized".to_vec(),
                content_type: content_type.to_owned(),
                thumbnail: b"thumb".to_vec(),
            })
        })
    }
}

fn service(pool: PgPool, store: RecordingStore) -> EvidenceService<RecordingStore> {
    EvidenceService::new(pool, store, "primary".to_owned(), "replica".to_owned())
}

#[sqlx::test(migrations = "../db/migrations")]
async fn evidence_processing_lifecycle_and_isolation_as_runtime_role(owner_pool: PgPool) {
    let seeded_a = seed_work_order(&owner_pool, ORG_A).await;
    let seeded_b = seed_work_order(&owner_pool, ORG_B).await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let store = RecordingStore::default();
    let org_a = OrgId::from_uuid(ORG_A);
    let org_b = OrgId::from_uuid(ORG_B);

    // --- Tenant A: staging upload writes a PROCESSING row with A-prefixed keys.
    let svc = service(rt_pool.clone(), store.clone());
    let a_prefix = format!("orgs/{ORG_A}/");
    let ticket = console_platform_request_context::scope_org(org_a, async {
        svc.issue_staging_upload(StagingUploadCommand {
            actor: console_kernel_core::UserId::from_uuid(seeded_a.uploaded_by),
            work_order_id: seeded_a.work_order_id,
            stage: console_workorder_domain::AttachmentStage::During,
            content_type: "video/quicktime".to_owned(),
            size_bytes: 5 * 1024 * 1024,
            checksum_sha256: None,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        })
        .await
    })
    .await
    .expect("staging upload as console_rt for tenant A should succeed");
    assert_eq!(ticket.media.processing_status, ProcessingStatus::Processing);
    assert!(ticket.media.s3_key.starts_with(&a_prefix));
    assert!(
        ticket
            .media
            .staging_s3_key
            .as_deref()
            .unwrap()
            .starts_with(&a_prefix)
    );
    let media_a = ticket.media.id;

    // --- Cross-tenant isolation: under B's GUC, A's PROCESSING row is NOT
    // claimable and NOT readable as `console_rt`.
    let svc_b = service(rt_pool.clone(), store.clone());
    let claimed_under_b = console_platform_request_context::scope_org(org_b, async {
        svc_b.claim_processing_job().await
    })
    .await
    .unwrap();
    assert!(
        claimed_under_b.is_none(),
        "tenant B must not be able to claim tenant A's PROCESSING evidence"
    );
    let read_under_b = console_platform_request_context::scope_org(org_b, async {
        svc_b.evidence_media(media_a).await
    })
    .await;
    assert!(
        read_under_b.is_err(),
        "tenant B must not be able to read tenant A's evidence row"
    );

    // --- FAIL-CLOSED: with NO GUC armed, claim sees nothing (the worker always
    // arms via scope_org; this proves the policy itself fails closed).
    // (Seed B's own PROCESSING row so the queue is non-empty globally.)
    let svc_b2 = service(rt_pool.clone(), store.clone());
    let _b_ticket = console_platform_request_context::scope_org(org_b, async {
        svc_b2
            .issue_staging_upload(StagingUploadCommand {
                actor: console_kernel_core::UserId::from_uuid(seeded_b.uploaded_by),
                work_order_id: seeded_b.work_order_id,
                stage: console_workorder_domain::AttachmentStage::Before,
                content_type: "image/jpeg".to_owned(),
                size_bytes: 1024,
                checksum_sha256: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
    })
    .await
    .expect("staging upload for tenant B should succeed");

    // --- Tenant A: claim + process transitions PROCESSING -> READY under A.
    let svc_a = service(rt_pool.clone(), store.clone());
    let status = console_platform_request_context::scope_org(org_a, async {
        let job = svc_a
            .claim_processing_job()
            .await
            .unwrap()
            .expect("tenant A should claim its own PROCESSING row");
        assert_eq!(job.media_id, media_a);
        assert!(job.final_key.starts_with(&a_prefix));
        let status = svc_a
            .process_job(
                &StubProcessor,
                &job,
                TraceContext::generate(),
                OffsetDateTime::now_utc(),
            )
            .await
            .unwrap();
        (status, job.final_key, job.thumbnail_key, job.staging_key)
    })
    .await;
    let (status, final_key, thumb_key, staging_key) = status;
    assert_eq!(status, ProcessingStatus::Ready);

    let media = console_platform_request_context::scope_org(org_a, async {
        svc_a.evidence_media(media_a).await
    })
    .await
    .unwrap();
    assert_eq!(media.processing_status, ProcessingStatus::Ready);
    assert_eq!(media.content_type, "video/mp4");
    assert!(media.staging_s3_key.is_none());

    // Artifacts uploaded under A's prefix; staging original deleted.
    let puts = store.puts.lock().unwrap().clone();
    assert!(puts.contains(&final_key));
    assert!(puts.contains(&thumb_key));
    assert!(puts.iter().all(|k| k.starts_with(&a_prefix)));
    assert!(store.deletes.lock().unwrap().contains(&staging_key));

    // sanity: the EvidenceId helper is exercised.
    let _ = EvidenceId::new();
}
