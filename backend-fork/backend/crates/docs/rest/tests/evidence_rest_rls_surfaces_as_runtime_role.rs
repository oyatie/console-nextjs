#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS + integrity gate for the console EvidenceCard REST surface.
//!
//! Every assertion runs as the genuine non-owner runtime role `console_rt`
//! (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) — NOT the default `#[sqlx::test]`
//! BYPASSRLS superuser pool, which would see every tenant's rows and green-light
//! a cross-org read. It proves the four contract points the EvidenceCard rests
//! on:
//!   1. list is RLS-scoped — an object registered in org A is invisible under B;
//!   2. detail carries the custody-event chain (the REGISTERED event is present);
//!   3. `verify` performs a REAL fixity check against the WORM store metadata and
//!      DETECTS a tampered hash (store checksum ≠ registered digest ⇒ Mismatch),
//!      while a matching object verifies;
//!   4. an applied legal hold BLOCKS disposal, and RELEASING it is fail-closed
//!      behind a DISTINCT-approver four-eyes decision (no approval ⇒ refused;
//!      a distinct approver's `approved` decision ⇒ release ⇒ disposal unblocked).

use std::collections::HashMap;
use std::sync::Arc;

use console_docs_adapter_postgres::PgDocsStore;
use console_docs_application::{
    ApplyLegalHoldCommand, CreateEvidenceObjectCommand, DisposeEvidenceObjectCommand,
    ListEvidenceObjectsQuery, RegisterEvidenceCopyCommand, RegisterEvidenceCopyInput,
    ReleaseLegalHoldCommand,
};
use console_docs_domain::{
    DerivativeKind, EvidenceClassification, EvidenceCopyEvidentiaryStatus, EvidenceCopyKind,
    EvidenceSourceRef, EvidenceSourceType, EvidenceStorageRef, Sha256Digest, WormStorageStatus,
};
use console_docs_rest::{DocsRestState, FixityStatus, HoldError, VerifyOutcome};
use console_governance_adapter_postgres::PgGovernanceStore;
use console_governance_application::{ApprovalDecision, DecideApprovalCommand};
use console_kernel_core::{EvidenceId, EvidenceObjectId, OrgId, TraceContext, UserId};
use console_platform_storage::{
    CopyObjectRequest, ObjectHead, PresignGetRequest, PresignPutRequest, PresignedUpload,
    RetentionInfo, S3ObjectStore, StorageFuture,
};
use console_platform_test_support::{
    grant_console_rt, runtime_role_pool, seed_admin_user_rls_off, seed_org_rls_off,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

const WORM_BUCKET: &str = "console-worm";
const ORG_A: Uuid = Uuid::from_u128(0xA1A1_A1A1_A1A1_A1A1_A1A1_A1A1_A1A1_A1A1);
const ORG_B: Uuid = Uuid::from_u128(0xB2B2_B2B2_B2B2_B2B2_B2B2_B2B2_B2B2_B2B2);
const MIGRATION_0195: &str = include_str!("../../../platform/db/migrations/0195_docs_gaps.sql");

// ---------------------------------------------------------------------------
// A stub WORM object store: HEAD returns a preconfigured checksum per key, so a
// test can present the store's recorded SHA-256 as matching or mismatching the
// digest persisted at registration. Only `head_object` is exercised.
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct StubWormStore {
    /// key -> lowercase-hex SHA-256 the store "recorded" for that object.
    heads: HashMap<String, String>,
}

impl StubWormStore {
    fn with(mut self, key: &str, hex_digest: &str) -> Self {
        self.heads.insert(key.to_owned(), hex_digest.to_owned());
        self
    }
}

fn base64_of_hex(hex_digest: &str) -> String {
    use base64::Engine as _;
    let bytes = hex::decode(hex_digest).expect("valid hex digest");
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

impl S3ObjectStore for StubWormStore {
    fn head_object(&self, _bucket: String, key: String) -> StorageFuture<'_, ObjectHead> {
        let checksum = self.heads.get(&key).map(|hex| base64_of_hex(hex));
        Box::pin(async move {
            Ok(ObjectHead {
                size_bytes: 42,
                e_tag: Some("\"etag\"".to_owned()),
                checksum_sha256: checksum,
                object_lock_mode: Some("COMPLIANCE".to_owned()),
                retain_until: None,
            })
        })
    }

    fn presign_put(&self, _r: PresignPutRequest) -> StorageFuture<'_, PresignedUpload> {
        Box::pin(async { unreachable!("stub: presign_put unused") })
    }
    fn presign_get(&self, _r: PresignGetRequest) -> StorageFuture<'_, String> {
        Box::pin(async { unreachable!("stub: presign_get unused") })
    }
    fn copy_object(&self, _r: CopyObjectRequest) -> StorageFuture<'_, ()> {
        Box::pin(async { unreachable!("stub: copy_object unused") })
    }
    fn get_object_retention(&self, _b: String, _k: String) -> StorageFuture<'_, RetentionInfo> {
        Box::pin(async { unreachable!("stub: get_object_retention unused") })
    }
    fn get_object(&self, _b: String, _k: String) -> StorageFuture<'_, Vec<u8>> {
        Box::pin(async { unreachable!("stub: get_object unused") })
    }
    fn put_object(
        &self,
        _b: String,
        _k: String,
        _c: String,
        _body: Vec<u8>,
    ) -> StorageFuture<'_, ()> {
        Box::pin(async { unreachable!("stub: put_object unused") })
    }
    fn delete_object(&self, _b: String, _k: String) -> StorageFuture<'_, ()> {
        Box::pin(async { unreachable!("stub: delete_object unused") })
    }
}

/// All-objects list query (every filter unset).
fn list_all() -> ListEvidenceObjectsQuery {
    ListEvidenceObjectsQuery {
        q: None,
        source_type: None,
        source_id: None,
        admissibility_status: None,
        legal_hold_state: None,
        custody_stage: None,
        classification: None,
        limit: None,
        offset: None,
        as_of: None,
        cursor: None,
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A `console_rt`-role pool (statements run under FORCE RLS), after granting the
/// base-table privileges that default grants don't cover under the
/// `#[sqlx::test]` superuser. The grant loop itself lives in
/// `console_platform_test_support::grant_console_rt` (an unscanned crate); the static
/// GRANT literals stay here.
async fn rt_pool(owner_pool: &PgPool) -> PgPool {
    grant_console_rt(
        owner_pool,
        &[
            "GRANT SELECT, INSERT ON audit_events TO console_rt",
            "GRANT SELECT, INSERT ON gov_approvals TO console_rt",
            "GRANT SELECT ON users TO console_rt",
            "GRANT SELECT ON organizations TO console_rt",
            "GRANT SELECT ON evidence_media TO console_rt",
        ],
    )
    .await;
    runtime_role_pool(owner_pool).await
}

fn state(pool: PgPool, stub: StubWormStore) -> DocsRestState {
    DocsRestState::new(
        PgDocsStore::new(pool.clone()),
        PgGovernanceStore::new(pool),
        Some(Arc::new(stub)),
        WORM_BUCKET.to_owned(),
        None, // JWT verifier unused: tests drive the state methods directly.
    )
}

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_760_000_000).unwrap()
}

/// Reconstruct the populated pre-0195 shape inside this isolated sqlx database,
/// so the real migration text—not a hand-copied approximation—upgrades legacy
/// EV rows in the regression below.
async fn restore_pre_0195_docs_shape(owner_pool: &PgPool) {
    sqlx::raw_sql(
        r#"
        DROP TRIGGER IF EXISTS docs_evidence_objects_register_sequence_immutable
            ON docs_evidence_objects;
        DROP FUNCTION IF EXISTS docs_evidence_object_register_sequence_guard();
        DROP INDEX IF EXISTS docs_evidence_objects_org_register_sequence;
        ALTER TABLE docs_evidence_objects
            DROP CONSTRAINT IF EXISTS docs_evidence_objects_register_sequence_positive;
        ALTER TABLE docs_evidence_objects
            ALTER COLUMN register_sequence DROP IDENTITY IF EXISTS;
        ALTER TABLE docs_evidence_objects
            DROP COLUMN IF EXISTS register_sequence;

        DROP TRIGGER IF EXISTS docs_evidence_copies_bind_storage_attestation
            ON docs_evidence_copies;
        DROP FUNCTION IF EXISTS docs_evidence_copy_bind_storage_attestation();
        DROP INDEX IF EXISTS idx_docs_evidence_copies_evidentiary_status;
        ALTER TABLE docs_evidence_copies
            DROP CONSTRAINT IF EXISTS docs_evidence_copies_evidentiary_status_check;
        ALTER TABLE docs_evidence_copies
            DROP COLUMN IF EXISTS evidentiary_status;

        GRANT UPDATE ON docs_evidence_copies TO console_rt;
        "#,
    )
    .execute(owner_pool)
    .await
    .expect("restore populated pre-0195 docs shape");
}

async fn seed_legacy_object(
    owner_pool: &PgPool,
    org: Uuid,
    actor: UserId,
    id: Uuid,
    code: &str,
    created_at: OffsetDateTime,
) {
    sqlx::query(
        r#"
        INSERT INTO docs_evidence_objects (
            id, org_id, code, title, source_type, source_id, classification,
            created_by, updated_by, created_at, updated_at
        ) VALUES ($1, $2, $3, $4, 'external_document', $3, 'INTERNAL',
                  $5, $5, $6, $6)
        "#,
    )
    .bind(id)
    .bind(org)
    .bind(code)
    .bind(format!("Legacy {code}"))
    .bind(*actor.as_uuid())
    .bind(created_at)
    .execute(owner_pool)
    .await
    .expect("seed pre-0195 EV row");
}

/// Register an EV object with a WORM-verified original copy whose stored digest
/// is `digest_hex` and whose storage key is `storage_key`. Returns the id.
/// Seed the storage service's persisted WORM-replica result.  This fixture
/// models the output of `EvidenceStorageService::replicate_once`, rather than
/// an EV-command claim: the only fields the EV trigger trusts are the row's
/// tenant, verified replica state/time, immutable object key, and checksum.
async fn seed_verified_storage_attestation(
    owner_pool: &PgPool,
    org: Uuid,
    actor: UserId,
    storage_key: &str,
    digest_hex: &str,
) -> EvidenceId {
    let mut tx = owner_pool
        .begin()
        .await
        .expect("begin owner seed transaction");
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .expect("disable RLS for owner fixture");

    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Evidence region {}", Uuid::new_v4()))
            .bind(org)
            .fetch_one(&mut *tx)
            .await
            .expect("seed region");
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("Evidence branch {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .expect("seed branch");
    let customer_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(branch_id)
    .bind("Evidence customer")
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .expect("seed customer");
    let site_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (customer_id, branch_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(customer_id)
    .bind(branch_id)
    .bind("Evidence site")
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .expect("seed site");
    let suffix = (Uuid::new_v4().as_u128() % 10_000) as u16;
    let equipment_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, model, source_sheet, source_row, org_id
        ) VALUES ($1, $2, $3, $4, $5, 'S', 'T', 'R', '임대', '좌식', '2.5', 'Model', 'test', 1, $6)
        RETURNING id
        "#,
    )
    .bind(branch_id)
    .bind(customer_id)
    .bind(site_id)
    .bind(format!("EVD01-{suffix:04}"))
    .bind(format!("M{suffix:04}"))
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .expect("seed equipment");
    let work_order_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, priority, symptom, result_type, org_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'REPORT_SUBMITTED', 'P3', 'Evidence fixture', 'COMPLETED', $8)
        "#,
    )
    .bind(work_order_id)
    // request_no CHECK: ^[0-9]{8}-[0-9]{3}$ (0008_create_work_orders).
    // Using uuid%1000 for the suffix collides across tests on one disposable DB
    // (unique on (org_id, request_no); main CI 31015221797). Derive both halves
    // from the work_order UUID so format + uniqueness hold together.
    .bind({
        let n = work_order_id.as_u128();
        format!("{:08}-{:03}", (n % 100_000_000) as u32, ((n / 100_000_000) % 1000) as u32)
    })
    .bind(branch_id)
    .bind(equipment_id)
    .bind(customer_id)
    .bind(site_id)
    .bind(*actor.as_uuid())
    .bind(org)
    .execute(&mut *tx)
    .await
    .expect("seed work order");

    let media_id = EvidenceId::new();
    sqlx::query(
        r#"
        INSERT INTO evidence_media (
            id, work_order_id, stage, s3_key, content_type, size_bytes,
            checksum_sha256, uploaded_by, worm_replica_status, verified_at,
            retry_count, next_retry_at, org_id
        ) VALUES ($1, $2, 'REPORT', $3, 'application/pdf', 42,
                  $4, $5, 'VERIFIED', $6, 0, $6, $7)
        "#,
    )
    .bind(*media_id.as_uuid())
    .bind(work_order_id)
    .bind(storage_key)
    .bind(base64_of_hex(digest_hex))
    .bind(*actor.as_uuid())
    .bind(now())
    .bind(org)
    .execute(&mut *tx)
    .await
    .expect("seed verified storage attestation");
    tx.commit()
        .await
        .expect("commit storage attestation fixture");
    media_id
}

async fn register_object(
    store: &PgDocsStore,
    actor: UserId,
    digest_hex: &str,
    storage_key: &str,
    title: &str,
) -> EvidenceObjectId {
    register_object_at(store, actor, digest_hex, storage_key, title, now()).await
}

async fn register_object_at(
    store: &PgDocsStore,
    actor: UserId,
    digest_hex: &str,
    storage_key: &str,
    title: &str,
    occurred_at: OffsetDateTime,
) -> EvidenceObjectId {
    let detail = store
        .create_object(CreateEvidenceObjectCommand {
            actor,
            title: title.to_owned(),
            description: None,
            source: EvidenceSourceRef::new(EvidenceSourceType::ExternalDocument, "src-1", None)
                .unwrap(),
            classification: EvidenceClassification::Internal,
            record_owner_user_id: None,
            initial_custody_reason: "registered for test".to_owned(),
            original: Some(RegisterEvidenceCopyInput {
                copy_kind: EvidenceCopyKind::Original,
                derivative_kind: None,
                parent_copy_id: None,
                storage: EvidenceStorageRef::new("seaweedfs-worm", storage_key, None, None)
                    .unwrap(),
                source_evidence_media_id: None,
                digest_sha256: Sha256Digest::new(digest_hex).unwrap(),
                content_type: "application/pdf".to_owned(),
                size_bytes: 42,
                worm_status: WormStorageStatus::Verified,
                verified_at: Some(occurred_at),
            }),
            tsa_proof: None,
            trace: TraceContext::generate(),
            occurred_at,
        })
        .await
        .expect("create_object under armed org as console_rt");
    detail.object.id
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn list_is_rls_scoped_and_detail_carries_custody_chain(owner_pool: PgPool) {
    let rt = rt_pool(&owner_pool).await;
    seed_org_rls_off(&owner_pool, ORG_A, "A").await;
    seed_org_rls_off(&owner_pool, ORG_B, "B").await;
    let actor = seed_admin_user_rls_off(&owner_pool, ORG_A).await;
    let st = state(rt, StubWormStore::default());

    let digest = "11".repeat(32);
    let id = console_platform_request_context::scope_org(OrgId::from_uuid(ORG_A), async {
        register_object(st.docs_store(), actor, &digest, "worm/a-1", "Contract A").await
    })
    .await;

    // (1) list is visible in the SAME org, invisible under a DIFFERENT org.
    let in_a = console_platform_request_context::scope_org(OrgId::from_uuid(ORG_A), async {
        st.docs_store().list_objects(list_all()).await
    })
    .await
    .unwrap();
    assert_eq!(in_a.total, 1, "the object is visible to its own tenant");

    let in_b = console_platform_request_context::scope_org(OrgId::from_uuid(ORG_B), async {
        st.docs_store().list_objects(list_all()).await
    })
    .await
    .unwrap();
    assert_eq!(
        in_b.total, 0,
        "FORCE RLS hides the object from another tenant"
    );

    // (2) detail carries the custody-event chain (REGISTERED present).
    let detail = console_platform_request_context::scope_org(OrgId::from_uuid(ORG_A), async {
        st.docs_store().get_object(id).await
    })
    .await
    .unwrap()
    .expect("detail for the object exists in its own tenant");
    assert!(
        !detail.custody_history.is_empty(),
        "the custody chain is surfaced in the detail payload"
    );
    assert_eq!(detail.copies.len(), 1, "the original copy is present");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn migration_0195_backfills_legacy_register_order_and_reseeds_identity(owner_pool: PgPool) {
    restore_pre_0195_docs_shape(&owner_pool).await;
    seed_org_rls_off(&owner_pool, ORG_A, "A").await;
    let actor = seed_admin_user_rls_off(&owner_pool, ORG_A).await;
    let oldest = Uuid::from_u128(0x1905_0000_0000_0000_0000_0000_0000_0001);
    let newest = Uuid::from_u128(0x1905_0000_0000_0000_0000_0000_0000_0002);

    // These rows exist before the real migration runs. Their IDs intentionally
    // sort opposite to their creation times so the deterministic legacy order is
    // proved to be `created_at, id`, not accidental heap or UUID order.
    seed_legacy_object(&owner_pool, ORG_A, actor, newest, "EV-LEGACY-002", now()).await;
    seed_legacy_object(
        &owner_pool,
        ORG_A,
        actor,
        oldest,
        "EV-LEGACY-001",
        now() - time::Duration::days(1),
    )
    .await;

    sqlx::raw_sql(MIGRATION_0195)
        .execute(&owner_pool)
        .await
        .expect("0195 upgrades a populated pre-0195 EV register");

    let legacy_rows: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT id, register_sequence FROM docs_evidence_objects ORDER BY register_sequence ASC",
    )
    .fetch_all(&owner_pool)
    .await
    .expect("read deterministic legacy registration order");
    assert_eq!(legacy_rows, vec![(oldest, 1), (newest, 2)]);

    let next_sequence: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO docs_evidence_objects (
            id, org_id, code, title, source_type, source_id, classification,
            created_by, updated_by
        ) VALUES ($1, $2, 'EV-LEGACY-003', 'Post migration',
                  'external_document', 'EV-LEGACY-003', 'INTERNAL', $3, $3)
        RETURNING register_sequence
        "#,
    )
    .bind(Uuid::from_u128(0x1905_0000_0000_0000_0000_0000_0000_0003))
    .bind(ORG_A)
    .bind(*actor.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .expect("identity sequence is reseeded above legacy rows");
    assert_eq!(next_sequence, 3);

    let rt = rt_pool(&owner_pool).await;
    let listed = console_platform_request_context::scope_org(OrgId::from_uuid(ORG_A), async {
        PgDocsStore::new(rt).list_objects(list_all()).await
    })
    .await
    .expect("legacy rows remain listable through the runtime adapter");
    assert_eq!(listed.total, 3);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn cursor_scan_is_stable_across_the_immutable_register(owner_pool: PgPool) {
    let rt = rt_pool(&owner_pool).await;
    seed_org_rls_off(&owner_pool, ORG_A, "A").await;
    let actor = seed_admin_user_rls_off(&owner_pool, ORG_A).await;
    let st = state(rt, StubWormStore::default());
    let org = OrgId::from_uuid(ORG_A);

    console_platform_request_context::scope_org(org, async {
        register_object(
            st.docs_store(),
            actor,
            &"13".repeat(32),
            "worm/cursor-1",
            "First",
        )
        .await;
        register_object(
            st.docs_store(),
            actor,
            &"14".repeat(32),
            "worm/cursor-2",
            "Second",
        )
        .await;
    })
    .await;

    let first = console_platform_request_context::scope_org(org, async {
        st.docs_store()
            .list_objects(ListEvidenceObjectsQuery {
                limit: Some(1),
                ..list_all()
            })
            .await
    })
    .await
    .expect("first cursor page succeeds");
    let cursor = first.next_cursor.clone().expect("first page has cursor");
    assert_eq!(first.total, 2);
    assert_eq!(first.items.len(), 1);

    // The third registration occurs after the first page captured its DB
    // sequence boundary, but its business timestamp is deliberately backdated.
    // It must not enter the already-open scan.
    let backdated_id = console_platform_request_context::scope_org(org, async {
        register_object_at(
            st.docs_store(),
            actor,
            &"15".repeat(32),
            "worm/cursor-backdated",
            "Backdated after snapshot",
            now() - time::Duration::days(1),
        )
        .await
    })
    .await;

    let second = console_platform_request_context::scope_org(org, async {
        st.docs_store()
            .list_objects(ListEvidenceObjectsQuery {
                limit: Some(1),
                as_of: Some(first.as_of),
                cursor: Some(cursor),
                ..list_all()
            })
            .await
    })
    .await
    .expect("second cursor page succeeds");
    assert_eq!(second.as_of, first.as_of);
    assert_eq!(second.total, 2);
    assert_eq!(second.items.len(), 1);
    assert_ne!(first.items[0].id, second.items[0].id);
    assert_ne!(second.items[0].id, backdated_id);

    let fresh = console_platform_request_context::scope_org(org, async {
        st.docs_store().list_objects(list_all()).await
    })
    .await
    .expect("fresh scan succeeds");
    assert_eq!(fresh.total, 3);
    assert!(fresh.items.iter().any(|object| object.id == backdated_id));
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn verify_detects_a_tampered_hash(owner_pool: PgPool) {
    let rt = rt_pool(&owner_pool).await;
    seed_org_rls_off(&owner_pool, ORG_A, "A").await;
    let actor = seed_admin_user_rls_off(&owner_pool, ORG_A).await;

    let good_digest = "aa".repeat(32);
    let bad_digest = "bb".repeat(32);
    // The store reports the TRUE digest for the good object's key, but a
    // DIFFERENT digest for the tampered object's key.
    let stub = StubWormStore::default()
        .with("worm/good", &good_digest)
        .with("worm/tampered", &"cc".repeat(32));
    let st = state(rt, stub);

    let (good_id, bad_id) =
        console_platform_request_context::scope_org(OrgId::from_uuid(ORG_A), async {
            let good =
                register_object(st.docs_store(), actor, &good_digest, "worm/good", "Good").await;
            let bad = register_object(
                st.docs_store(),
                actor,
                &bad_digest,
                "worm/tampered",
                "Tampered",
            )
            .await;
            (good, bad)
        })
        .await;

    let good_report = console_platform_request_context::scope_org(OrgId::from_uuid(ORG_A), async {
        st.verify_object_fixity(actor, good_id, TraceContext::generate(), now())
            .await
    })
    .await
    .expect("verify succeeds for the intact object");
    assert_eq!(good_report.outcome, VerifyOutcome::Verified);
    assert_eq!(good_report.copies[0].status, FixityStatus::Match);

    let bad_report = console_platform_request_context::scope_org(OrgId::from_uuid(ORG_A), async {
        st.verify_object_fixity(actor, bad_id, TraceContext::generate(), now())
            .await
    })
    .await
    .expect("verify itself succeeds — it REPORTS the mismatch, not errors");
    assert_eq!(
        bad_report.outcome,
        VerifyOutcome::Mismatch,
        "the store's recorded checksum differs from the registered digest ⇒ tamper detected"
    );
    assert_eq!(bad_report.copies[0].status, FixityStatus::Mismatch);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn hold_blocks_disposal_and_release_requires_a_distinct_approver(owner_pool: PgPool) {
    let rt = rt_pool(&owner_pool).await;
    seed_org_rls_off(&owner_pool, ORG_A, "A").await;
    let requester = seed_admin_user_rls_off(&owner_pool, ORG_A).await;
    let approver = seed_admin_user_rls_off(&owner_pool, ORG_A).await;
    let st = state(rt, StubWormStore::default());
    let org = OrgId::from_uuid(ORG_A);

    let digest = "dd".repeat(32);
    let id = console_platform_request_context::scope_org(org, async {
        register_object(st.docs_store(), requester, &digest, "worm/h-1", "Held").await
    })
    .await;

    // Apply a legal hold, then disposal must be BLOCKED.
    let hold = console_platform_request_context::scope_org(org, async {
        st.apply_hold(ApplyLegalHoldCommand {
            actor: requester,
            evidence_object_id: id,
            case_ref: "CASE-1".to_owned(),
            basis: "litigation".to_owned(),
            reason: "preserve".to_owned(),
            trace: TraceContext::generate(),
            occurred_at: now(),
        })
        .await
    })
    .await
    .expect("apply_hold succeeds");

    let dispose_blocked = console_platform_request_context::scope_org(org, async {
        st.docs_store()
            .dispose_object(DisposeEvidenceObjectCommand {
                actor: requester,
                evidence_object_id: id,
                reason: "cleanup".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now(),
            })
            .await
    })
    .await;
    assert!(
        dispose_blocked.is_err(),
        "an ACTIVE legal hold must block disposal"
    );

    // Release with NO four-eyes approval is refused (fail-closed).
    let refused = console_platform_request_context::scope_org(org, async {
        st.release_hold(
            Uuid::new_v4(),
            ReleaseLegalHoldCommand {
                actor: approver,
                evidence_object_id: id,
                hold_id: hold.id,
                release_reason: "cleared".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now(),
            },
        )
        .await
    })
    .await;
    assert!(
        matches!(refused, Err(HoldError::FourEyesRequired)),
        "release without a distinct-approver approval is refused"
    );

    // Record a DISTINCT approver's approval, then release succeeds.
    let request_ref = Uuid::new_v4();
    console_platform_request_context::scope_org(org, async {
        st.governance_store()
            .decide_approval(DecideApprovalCommand {
                approver,
                request_ref,
                // Must match the release gate's server-derived binding: the console
                // kind `evidence.hold.release` bound to the hold being released.
                kind: "evidence.hold.release".to_owned(),
                requested_by: requester,
                target_ref: Some(*hold.id.as_uuid()),
                decision: ApprovalDecision::Approved,
                trace: TraceContext::generate(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .expect("a distinct approver records an approval");

    console_platform_request_context::scope_org(org, async {
        st.release_hold(
            request_ref,
            ReleaseLegalHoldCommand {
                actor: approver,
                evidence_object_id: id,
                hold_id: hold.id,
                release_reason: "cleared".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now(),
            },
        )
        .await
    })
    .await
    .expect("release succeeds once a distinct approver has approved");

    // With the hold released, disposal is now permitted.
    console_platform_request_context::scope_org(org, async {
        st.docs_store()
            .dispose_object(DisposeEvidenceObjectCommand {
                actor: requester,
                evidence_object_id: id,
                reason: "cleanup".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: now(),
            })
            .await
    })
    .await
    .expect("disposal is unblocked after the hold is released");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn storage_attestation_controls_original_verification_and_derivative_meaning(
    owner_pool: PgPool,
) {
    let rt = rt_pool(&owner_pool).await;
    seed_org_rls_off(&owner_pool, ORG_A, "A").await;
    let actor = seed_admin_user_rls_off(&owner_pool, ORG_A).await;
    let st = state(rt.clone(), StubWormStore::default());
    let org = OrgId::from_uuid(ORG_A);

    // RED regression: command-provided VERIFIED + verified_at without an
    // authoritative storage attestation cannot create a verified original.
    let unproven_id = console_platform_request_context::scope_org(org, async {
        register_object(
            st.docs_store(),
            actor,
            &"ab".repeat(32),
            "worm/unproven-original",
            "Caller asserted original",
        )
        .await
    })
    .await;
    let unproven = console_platform_request_context::scope_org(org, async {
        st.docs_store()
            .get_object(unproven_id)
            .await
            .expect("unproven detail query succeeds")
            .expect("unproven original exists")
            .copies
            .into_iter()
            .next()
            .expect("unproven original copy exists")
    })
    .await;
    assert_eq!(unproven.worm_status, WormStorageStatus::Pending);
    assert_eq!(unproven.verified_at, None);
    assert_eq!(
        unproven.evidentiary_status,
        EvidenceCopyEvidentiaryStatus::OriginalUnverified
    );

    // The caller has the same authenticated runtime role and tenant GUC as a
    // normal request, but cannot forge the status transition with direct SQL.
    // The storage-attestation trigger remains available only on INSERT.
    let mut forged_update_tx = rt.begin().await.expect("begin runtime transaction");
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(&mut *forged_update_tx)
        .await
        .expect("arm runtime tenant GUC");
    let forged_update = sqlx::query(
        "UPDATE docs_evidence_copies SET worm_status = 'VERIFIED', verified_at = now() WHERE id = $1",
    )
    .bind(*unproven.id.as_uuid())
    .execute(&mut *forged_update_tx)
    .await
    .expect_err("console_rt must not be able to forge PENDING to VERIFIED");
    assert!(
        forged_update.to_string().contains("permission denied"),
        "direct runtime UPDATE must fail by privilege before a mutable status can be forged: {forged_update}"
    );
    drop(forged_update_tx);

    let has_update: bool = sqlx::query_scalar(
        "SELECT has_table_privilege('console_rt', 'docs_evidence_copies', 'UPDATE')",
    )
    .fetch_one(&owner_pool)
    .await
    .expect("inspect runtime copy-update privilege");
    assert!(!has_update, "0195 revokes direct runtime copy UPDATE");

    let (security_definer, function_config, runtime_can_execute): (
        bool,
        Option<Vec<String>>,
        bool,
    ) = sqlx::query_as(
        r#"
            SELECT p.prosecdef,
                   p.proconfig,
                   has_function_privilege('console_rt', p.oid, 'EXECUTE')
            FROM pg_proc AS p
            WHERE p.oid = 'docs_evidence_copy_bind_storage_attestation()'::regprocedure
            "#,
    )
    .fetch_one(&owner_pool)
    .await
    .expect("inspect storage-attestation trigger function hardening");
    assert!(
        security_definer,
        "attestation reads run through the owner-only server path"
    );
    assert!(
        function_config
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|setting| setting == "search_path=pg_catalog, public"),
        "SECURITY DEFINER trigger function pins a safe search_path"
    );
    assert!(
        !runtime_can_execute,
        "console_rt has no direct EXECUTE capability on the attestation trigger function"
    );

    // Existing storage-service proof path: replicate_once writes a VERIFIED
    // evidence_media row. The EV trigger promotes only when its key + SHA-256
    // match; the requested command status remains PENDING.
    let original_digest = "bc".repeat(32);
    let original_media = seed_verified_storage_attestation(
        &owner_pool,
        ORG_A,
        actor,
        "worm/attested-original",
        &original_digest,
    )
    .await;
    let attested_id = console_platform_request_context::scope_org(org, async {
        st.docs_store()
            .create_object(CreateEvidenceObjectCommand {
                actor,
                title: "Attested original".to_owned(),
                description: None,
                source: EvidenceSourceRef::new(
                    EvidenceSourceType::WorkOrderEvidenceMedia,
                    original_media.to_string(),
                    None,
                )
                .expect("valid source"),
                classification: EvidenceClassification::Internal,
                record_owner_user_id: None,
                initial_custody_reason: "registered from storage attestation".to_owned(),
                original: Some(RegisterEvidenceCopyInput {
                    copy_kind: EvidenceCopyKind::Original,
                    derivative_kind: None,
                    parent_copy_id: None,
                    storage: EvidenceStorageRef::new(
                        "seaweedfs-worm",
                        "worm/attested-original",
                        None,
                        None,
                    )
                    .expect("valid storage ref"),
                    source_evidence_media_id: Some(original_media),
                    digest_sha256: Sha256Digest::new(&original_digest).expect("valid digest"),
                    content_type: "application/pdf".to_owned(),
                    size_bytes: 42,
                    worm_status: WormStorageStatus::Pending,
                    verified_at: None,
                }),
                tsa_proof: None,
                trace: TraceContext::generate(),
                occurred_at: now(),
            })
            .await
            .expect("attested original registration succeeds")
            .object
            .id
    })
    .await;
    let attested_original = console_platform_request_context::scope_org(org, async {
        st.docs_store()
            .get_object(attested_id)
            .await
            .expect("attested detail query succeeds")
            .expect("attested original exists")
            .copies
            .into_iter()
            .next()
            .expect("attested original copy exists")
    })
    .await;
    assert_eq!(attested_original.worm_status, WormStorageStatus::Verified);
    assert_eq!(attested_original.verified_at, Some(now()));
    assert_eq!(
        attested_original.evidentiary_status,
        EvidenceCopyEvidentiaryStatus::VerifiedOriginal
    );

    let derivative_digest = "cd".repeat(32);
    let derivative_media = seed_verified_storage_attestation(
        &owner_pool,
        ORG_A,
        actor,
        "worm/attested-redaction",
        &derivative_digest,
    )
    .await;
    let derivative = console_platform_request_context::scope_org(org, async {
        st.docs_store()
            .register_copy(RegisterEvidenceCopyCommand {
                actor,
                evidence_object_id: attested_id,
                copy: RegisterEvidenceCopyInput {
                    copy_kind: EvidenceCopyKind::Derivative,
                    derivative_kind: Some(DerivativeKind::Redacted),
                    parent_copy_id: Some(attested_original.id),
                    storage: EvidenceStorageRef::new(
                        "seaweedfs-worm",
                        "worm/attested-redaction",
                        None,
                        None,
                    )
                    .expect("valid derivative storage ref"),
                    source_evidence_media_id: Some(derivative_media),
                    digest_sha256: Sha256Digest::new(&derivative_digest).expect("valid digest"),
                    content_type: "application/pdf".to_owned(),
                    size_bytes: 23,
                    worm_status: WormStorageStatus::Pending,
                    verified_at: None,
                },
                trace: TraceContext::generate(),
                occurred_at: now(),
            })
            .await
            .expect("attested derivative registration succeeds")
    })
    .await;
    assert_eq!(derivative.worm_status, WormStorageStatus::Verified);
    assert_eq!(
        derivative.evidentiary_status,
        EvidenceCopyEvidentiaryStatus::NonEvidentiaryDerivative,
        "a storage-verified derivative must never be presented as the evidentiary original"
    );
}
