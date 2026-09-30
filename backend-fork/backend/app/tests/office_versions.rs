#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS + version-domain gate for the in-console office editor (slice 0).
//!
//! Mirrors the comms `*_rls_surfaces_as_runtime_role` tests: we SEED as the
//! owner (raw inserts, row_security off) and MUTATE/READ as the genuine
//! non-owner runtime role `console_rt` (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) — the
//! only faithful exercise of the `org_isolation` policy. The default
//! `#[sqlx::test]` pool is a BYPASSRLS superuser and would green-light a
//! broken/leaking path.
//!
//! What CI runs here (no container needed): the immutable version domain —
//! record → append (v1), callback save (v2, idempotent per editing-session
//! key), non-destructive restore (v3 re-publishing v1's blob), the version
//! list, `with_audit` coverage, and the cross-tenant DENY negative.
//!
//! What CI CANNOT run (needs the DocumentServer container + object store): the
//! full editor round-trip through `POST /office/sessions` (presign) and the
//! HTTP `POST /office/callback` (fetch-produced-doc → store). Those exercise the
//! `OfficeBlobStore` external-IO boundary; the JWT sign/verify + config shape
//! are covered by the in-module unit tests (`cargo test -p console-app office::`).

use console_app::office::{
    DocumentVersion, NewVersion, issue_session_version, list_versions, record_version,
    restore_version,
};
use console_kernel_core::{OrgId, UserId};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

const ORG_B: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);
const DOC: &str = "DOC-OFFICE-1";

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

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

async fn seed_active_user(owner_pool: &PgPool, org: Uuid) -> UserId {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id, is_active) VALUES ($1, $2, $3, true) RETURNING id",
    )
    .bind(format!("User {}", Uuid::new_v4()))
    .bind(vec!["ADMIN".to_string()])
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    UserId::from_uuid(user_id)
}

async fn audit_count(owner_pool: &PgPool, action: &str, target_id: &str) -> i64 {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = $1 AND target_id = $2",
    )
    .bind(action)
    .bind(target_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    count
}

fn record(
    org: OrgId,
    actor: Option<UserId>,
    storage_key: &str,
    content_hash: &str,
    source_key: Option<&str>,
) -> NewVersion {
    NewVersion {
        org,
        actor,
        document_ref: DOC.to_owned(),
        file_type: "docx".to_owned(),
        storage_key: storage_key.to_owned(),
        content_hash: content_hash.to_owned(),
        byte_size: 1024,
        source_key: source_key.map(str::to_owned),
        restored_from: None,
    }
}

// ===========================================================================
// The full slice-0 domain lifecycle as `console_rt`: v1 → callback v2 (idempotent)
// → restore v3, audited, immutable, monotonic.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn version_lifecycle_as_runtime_role(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    seed_org(&owner_pool, *org.as_uuid(), "A").await;
    let actor = seed_active_user(&owner_pool, *org.as_uuid()).await;

    // v1: initial (ingested) version, no editing-session key.
    let v1 = record_version(
        &rt,
        record(org, Some(actor), "office/a/x/1.docx", "hash1", None),
    )
    .await
    .expect("record v1 as console_rt under the armed GUC");
    assert_eq!(v1.version_no, 1);
    assert_eq!(v1.restored_from, None);
    assert_eq!(
        audit_count(
            &owner_pool,
            "office.document_version.record",
            &v1.id.to_string()
        )
        .await,
        1,
        "v1 must be audited"
    );

    // v2: a force-save callback produced this, keyed by the editing session.
    let v2 = record_version(
        &rt,
        record(org, None, "office/a/x/2.docx", "hash2", Some("edit-key-1")),
    )
    .await
    .expect("record v2 (callback) as console_rt");
    assert_eq!(v2.version_no, 2);

    // Idempotent replay: DocumentServer retries the SAME callback key → the
    // already-stored version is returned, NO v3 is appended.
    let replay = record_version(
        &rt,
        record(
            org,
            None,
            "office/a/x/2b.docx",
            "hash2b",
            Some("edit-key-1"),
        ),
    )
    .await
    .expect("idempotent callback replay");
    assert_eq!(
        replay.id, v2.id,
        "a retried callback must not append a duplicate"
    );
    assert_eq!(replay.version_no, 2);

    // Restore v1 → NON-DESTRUCTIVELY re-publishes it as v3, blob reused, lineage
    // recorded. Restore is a distinct audited action.
    let v3 = restore_version(&rt, org, actor, DOC, 1)
        .await
        .expect("restore v1 as console_rt");
    assert_eq!(v3.version_no, 3);
    assert_eq!(v3.restored_from, Some(1));
    assert_eq!(
        audit_count(
            &owner_pool,
            "office.document_version.restore",
            &v3.id.to_string()
        )
        .await,
        1,
        "restore must be audited under its own action"
    );

    // The append-only history is exactly v1, v2, v3 (newest first).
    let versions: Vec<DocumentVersion> = list_versions(&rt, org, DOC).await.unwrap();
    assert_eq!(
        versions.iter().map(|v| v.version_no).collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
}

// ===========================================================================
// Cross-tenant DENY: org B never sees org A's versions as `console_rt`, and an
// unarmed read surfaces nothing.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn versions_are_invisible_across_tenants_as_runtime_role(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);
    seed_org(&owner_pool, *org_a.as_uuid(), "A").await;
    seed_org(&owner_pool, *org_b.as_uuid(), "B").await;
    let actor_a = seed_active_user(&owner_pool, *org_a.as_uuid()).await;

    record_version(
        &rt,
        record(org_a, Some(actor_a), "office/a/x/1.docx", "h1", None),
    )
    .await
    .expect("seed A's version");

    // Under B's armed GUC, A's document is INVISIBLE.
    let seen_by_b = list_versions(&rt, org_b, DOC).await.expect("list as B");
    assert!(
        seen_by_b.is_empty(),
        "B must never see A's document versions"
    );

    // And restoring A's version while armed as B fails closed (not found).
    let cross = restore_version(&rt, org_b, actor_a, DOC, 1).await;
    assert!(
        cross.is_err(),
        "restore must fail closed when armed as the wrong tenant"
    );
}

// ===========================================================================
// Session issuance is a sensitive grant: it must be audited (review round —
// item 3), atomically with the read that resolves the latest version.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn issue_session_version_records_an_audit_event_as_runtime_role(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    seed_org(&owner_pool, *org.as_uuid(), "A").await;
    let actor = seed_active_user(&owner_pool, *org.as_uuid()).await;

    let v1 = record_version(
        &rt,
        record(org, Some(actor), "office/a/x/1.docx", "hash1", None),
    )
    .await
    .expect("record v1 as console_rt");

    let issued = issue_session_version(&rt, org, actor, DOC)
        .await
        .expect("issue a session for the latest version as console_rt");
    assert_eq!(issued.id, v1.id, "session must resolve the latest version");

    assert_eq!(
        audit_count(&owner_pool, "office.session.issue", &v1.id.to_string()).await,
        1,
        "session issuance is a sensitive grant and must be audited"
    );

    // No document ⇒ not_found, and no audit noise from the failed attempt.
    let missing = issue_session_version(&rt, org, actor, "DOC-NO-SUCH").await;
    assert!(missing.is_err());
    assert_eq!(
        audit_count(&owner_pool, "office.session.issue", "DOC-NO-SUCH").await,
        0,
        "a not_found session request must not persist a partial audit row"
    );
}

// ===========================================================================
// Replay hardening (review round — item 2): a callback replay carrying an
// OLD editing-session key — even after later versions were appended — must
// stay provably inert: no new version, the original row returned unchanged.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn replay_of_old_callback_key_after_later_versions_is_inert(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    seed_org(&owner_pool, *org.as_uuid(), "A").await;
    let actor = seed_active_user(&owner_pool, *org.as_uuid()).await;

    let v1 = record_version(
        &rt,
        record(org, None, "office/a/x/1.docx", "hash1", Some("edit-key-1")),
    )
    .await
    .expect("record v1 (callback) as console_rt");

    let v2 = record_version(
        &rt,
        record(org, None, "office/a/x/2.docx", "hash2", Some("edit-key-2")),
    )
    .await
    .expect("record v2 (callback) as console_rt");

    let v3 = restore_version(&rt, org, actor, DOC, 1)
        .await
        .expect("restore v1 as v3");
    assert_eq!(v3.version_no, 3);

    // A stale replay of v1's ORIGINAL editing-session key, well after v2 and
    // v3 exist, must resolve to v1 unchanged — no v4, no data corruption.
    let replay = record_version(
        &rt,
        record(
            org,
            None,
            "office/a/x/attacker-payload.docx",
            "attacker-hash",
            Some("edit-key-1"),
        ),
    )
    .await
    .expect("stale replay must be a no-op, not an error");
    assert_eq!(
        replay.id, v1.id,
        "replay must resolve to the ORIGINAL v1 row"
    );
    assert_eq!(
        replay.content_hash, "hash1",
        "replay must not overwrite v1's content"
    );

    let versions: Vec<DocumentVersion> = list_versions(&rt, org, DOC).await.unwrap();
    assert_eq!(
        versions.iter().map(|v| v.version_no).collect::<Vec<_>>(),
        vec![3, 2, 1],
        "the replay must not append a v4"
    );
    let _ = v2;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn document_ref_is_trimmed_before_storage(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    seed_org(&owner_pool, *org.as_uuid(), "A").await;
    let actor = seed_active_user(&owner_pool, *org.as_uuid()).await;

    let mut padded = record(org, Some(actor), "office/a/x/trimmed.docx", "hash1", None);
    padded.document_ref = format!("  {DOC}  ");

    let v1 = record_version(&rt, padded)
        .await
        .expect("record padded document_ref as normalized document");
    assert_eq!(v1.document_ref, DOC);

    let versions = list_versions(&rt, org, &format!("\t{DOC}\t"))
        .await
        .expect("list trims document_ref before lookup");
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].id, v1.id);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn restored_from_must_reference_an_existing_same_document_version(owner_pool: PgPool) {
    let org = *OrgId::knl().as_uuid();
    seed_org(&owner_pool, org, "A").await;

    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();

    let result = sqlx::query(
        "INSERT INTO document_versions \
         (org_id, document_ref, version_no, content_hash, storage_key, file_type, byte_size, restored_from) \
         VALUES ($1, $2, 1, $3, $4, 'docx', 1024, 99)",
    )
    .bind(org)
    .bind(DOC)
    .bind("hash1")
    .bind("office/a/x/bad-restore.docx")
    .execute(&mut *tx)
    .await;

    assert!(
        result.is_err(),
        "restored_from must not point at a missing version in the same document"
    );
    tx.rollback().await.unwrap();
}
