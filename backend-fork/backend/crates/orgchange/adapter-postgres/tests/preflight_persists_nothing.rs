#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Org-change preflight is a READ (HANDOFF §15/§16 P3).
//!
//! Two contracts, proved through the shipped store entry points the REST
//! handlers call (`PgOrgChangeStore::preflight` / `::submit`), never through a
//! private helper:
//!
//! 1. `preflight` persists NOTHING — no `PRECHECKED` state flip, no
//!    `org_change_events` row, no `audit_events` row, no column rewrite
//!    anywhere. Proved by a whole-database content fingerprint (one digest per
//!    base table, so an INSERT, a DELETE *and* an in-place UPDATE all move it)
//!    taken immediately before and after the call.
//! 2. `submit` does not strand rows written in the old shape: it accepts a
//!    pre-existing `DRAFT` **and** a pre-existing `PRECHECKED` row and
//!    recomputes the preflight inside its own transaction rather than trusting
//!    whatever receipt is stored on the row.

use std::collections::BTreeMap;

use console_kernel_core::{OrgId, UserId};
use console_orgchange_adapter_postgres::{CreateOrgChange, PgOrgChangeStore};
use console_orgchange_domain::{
    OrgChangeKind, OrgChangeStatus, OrgChangeTarget, OrgProposalOp, TargetKind,
};
use serde_json::json;
use sqlx::{PgPool, Row};
use time::{Date, OffsetDateTime, macros::offset};
use uuid::Uuid;

fn today_kst() -> Date {
    OffsetDateTime::now_utc().to_offset(offset!(+9)).date()
}

// ---------------------------------------------------------------------------
// 1. preflight writes zero rows and rewrites zero columns
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn preflight_persists_nothing_across_every_table(pool: PgPool) {
    let org = seed_org(&pool).await;
    console_platform_request_context::scope_org(org, async move {
        let region = seed_region(&pool, org, "리전-무기록").await;
        let drafter = seed_user(&pool, org, "EXECUTIVE").await;
        let store = PgOrgChangeStore::new(pool.clone());

        let (created, _) = store
            .create(drafter, clean_reorg(region, "preflight-zero-write-0001"))
            .await
            .unwrap();
        let id = created.summary.id;
        assert_eq!(created.summary.status, OrgChangeStatus::Draft);

        let before = db_fingerprint(&pool, org).await;
        // The oracle must be able to SEE the tables preflight used to write; a
        // fingerprint that read them empty would pass vacuously.
        for table in ["org_change_requests", "org_change_events", "audit_events"] {
            assert!(
                before.get(table).is_some_and(|digest| !digest.is_empty()),
                "fingerprint cannot see {table}; the zero-write assertion would be vacuous"
            );
        }

        let detail = store.preflight(drafter, id).await.unwrap();

        let after = db_fingerprint(&pool, org).await;
        assert_eq!(
            diff(&before, &after),
            Vec::<String>::new(),
            "preflight must persist nothing"
        );

        // It still has to REPORT the receipt it computed, and it must not have
        // promoted the request out of DRAFT.
        assert_eq!(detail.summary.status, OrgChangeStatus::Draft);
        let report = detail.preflight.expect("preflight report is returned");
        assert!(report.blockers.is_empty(), "clean proposal: {report:?}");
        assert!(!report.stale);

        // The proposal was valid when drafted, but its CreateBranch target can
        // become unusable before approval. Preflight must both detect that
        // transition and remain a strict zero-write read.
        deactivate_region(&pool, org, region).await;
        let unavailable_before = db_fingerprint(&pool, org).await;
        let unavailable = store.preflight(drafter, id).await.unwrap();
        let unavailable_after = db_fingerprint(&pool, org).await;
        assert_eq!(
            diff(&unavailable_before, &unavailable_after),
            Vec::<String>::new(),
            "target revalidation during preflight must persist nothing"
        );
        let report = unavailable
            .preflight
            .expect("unavailable-target preflight report is returned");
        assert!(
            report
                .blockers
                .iter()
                .any(|blocker| blocker.code == "REGION_TARGET_UNAVAILABLE" && blocker.count == 1),
            "a deactivated CreateBranch destination must block approval: {report:?}"
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn preflight_persists_nothing_even_when_it_finds_blockers(pool: PgPool) {
    let org = seed_org(&pool).await;
    console_platform_request_context::scope_org(org, async move {
        let region = seed_region(&pool, org, "리전-차단").await;
        let branch = seed_branch(&pool, org, region, "지점-차단").await;
        let drafter = seed_user(&pool, org, "EXECUTIVE").await;
        // An active resident user makes DEACTIVATE_BRANCH a REORG blocker.
        seed_user_in_branch(&pool, org, "MECHANIC", branch).await;
        let store = PgOrgChangeStore::new(pool.clone());

        let (created, _) = store
            .create(
                drafter,
                blocked_reorg(region, branch, "preflight-zero-write-0002"),
            )
            .await
            .unwrap();
        let id = created.summary.id;
        deactivate_branch(&pool, org, branch).await;

        let before = db_fingerprint(&pool, org).await;
        let detail = store.preflight(drafter, id).await.unwrap();
        let after = db_fingerprint(&pool, org).await;

        assert_eq!(
            diff(&before, &after),
            Vec::<String>::new(),
            "a blocking preflight must persist nothing either"
        );
        let report = detail.preflight.expect("preflight report is returned");
        assert!(
            report.blockers.iter().any(|b| b.code == "ACTIVE_USERS"),
            "active resident user is a blocker: {report:?}"
        );
        assert!(
            report
                .blockers
                .iter()
                .any(|b| b.code == "BRANCH_TARGET_UNAVAILABLE" && b.count == 1),
            "a deactivated proposal target is independently unavailable: {report:?}"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// 2. submit does not strand rows written in the old shape
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn submit_accepts_a_pre_existing_draft_row_and_recomputes(pool: PgPool) {
    let org = seed_org(&pool).await;
    console_platform_request_context::scope_org(org, async move {
        let region = seed_region(&pool, org, "리전-초안").await;
        let drafter = seed_user(&pool, org, "EXECUTIVE").await;
        let store = PgOrgChangeStore::new(pool.clone());

        let (created, _) = store
            .create(drafter, clean_reorg(region, "submit-from-draft-0001"))
            .await
            .unwrap();
        let id = created.summary.id;
        assert_eq!(created.summary.status, OrgChangeStatus::Draft);
        assert!(
            created.preflight.is_none(),
            "a fresh DRAFT carries no stored receipt"
        );

        let submitted = store.submit(drafter, id).await.unwrap();

        assert_eq!(submitted.summary.status, OrgChangeStatus::InApproval);
        assert_eq!(submitted.approval_steps.len(), 4);
        let report = submitted
            .preflight
            .expect("submit stores the receipt it recomputed");
        assert!(report.blockers.is_empty());
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn submit_accepts_a_pre_existing_prechecked_row_and_recomputes(pool: PgPool) {
    let org = seed_org(&pool).await;
    console_platform_request_context::scope_org(org, async move {
        let region = seed_region(&pool, org, "리전-기존").await;
        let drafter = seed_user(&pool, org, "EXECUTIVE").await;
        let store = PgOrgChangeStore::new(pool.clone());

        let (created, _) = store
            .create(drafter, clean_reorg(region, "submit-legacy-prechecked-01"))
            .await
            .unwrap();
        let id = created.summary.id;
        // A row already written in the OLD shape: preflight flipped it to
        // PRECHECKED and left a receipt behind.
        let seeded_at = legacy_prechecked(&pool, org, id, json!([])).await;

        let submitted = store.submit(drafter, id).await.unwrap();

        assert_eq!(submitted.summary.status, OrgChangeStatus::InApproval);
        assert_eq!(submitted.approval_steps.len(), 4);
        let report = submitted
            .preflight
            .expect("submit stores the receipt it recomputed");
        assert!(report.blockers.is_empty());
        assert!(
            report.computed_at > seeded_at,
            "submit must replace the legacy receipt with one computed in its own \
             transaction (stored {}, seeded {seeded_at})",
            report.computed_at
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn submit_refuses_a_pre_existing_prechecked_row_whose_stored_receipt_lies(pool: PgPool) {
    let org = seed_org(&pool).await;
    console_platform_request_context::scope_org(org, async move {
        let region = seed_region(&pool, org, "리전-거짓").await;
        let branch = seed_branch(&pool, org, region, "지점-거짓").await;
        let drafter = seed_user(&pool, org, "EXECUTIVE").await;
        seed_user_in_branch(&pool, org, "MECHANIC", branch).await;
        let store = PgOrgChangeStore::new(pool.clone());

        let (created, _) = store
            .create(
                drafter,
                blocked_reorg(region, branch, "submit-legacy-lying-01"),
            )
            .await
            .unwrap();
        let id = created.summary.id;
        // The stored receipt claims a clean run; reality has an ACTIVE_USERS
        // blocker. Only an in-transaction recompute catches this.
        legacy_prechecked(&pool, org, id, json!([])).await;

        let refused = store.submit(drafter, id).await.unwrap_err();
        assert_eq!(refused.kind(), console_kernel_core::ErrorKind::Conflict);

        let status = read_status(&pool, org, id).await;
        assert_eq!(
            status, "PRECHECKED",
            "the refused submit rolls back; the legacy row is untouched"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// Whole-database content fingerprint
// ---------------------------------------------------------------------------

/// `table -> md5 digest of every row's text form`, for every base table in
/// `public`. Order-independent, so it moves on INSERT, DELETE and UPDATE alike
/// — an in-place `UPDATE org_change_requests SET status = 'PRECHECKED'` leaves
/// row COUNTS untouched, so a count-based delta would not see the defect.
///
/// `app.current_org` is armed so this stays correct if the test role ever loses
/// its RLS bypass; the org-change and audit tables are FORCE RLS (migrations
/// 0035/0198). The callers' sentinel assertion is what actually proves the
/// fingerprint is not reading empty.
async fn db_fingerprint(pool: &PgPool, org: OrgId) -> BTreeMap<String, String> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_type = 'BASE TABLE' ORDER BY table_name",
    )
    .fetch_all(tx.as_mut())
    .await
    .unwrap();
    let mut out = BTreeMap::new();
    for table in tables {
        // Audited: `table` is a catalog-supplied identifier, double-quoted here,
        // and no user input reaches this string.
        let digest: Option<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT md5(string_agg(t::text, '|' ORDER BY t::text)) FROM public.\"{table}\" t"
        )))
        .fetch_one(tx.as_mut())
        .await
        .unwrap();
        out.insert(table, digest.unwrap_or_default());
    }
    tx.commit().await.unwrap();
    out
}

fn diff(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> Vec<String> {
    let mut changed = Vec::new();
    for (table, digest) in before {
        match after.get(table) {
            Some(next) if next == digest => {}
            Some(_) => changed.push(format!("{table}: content changed")),
            None => changed.push(format!("{table}: table vanished")),
        }
    }
    for table in after.keys() {
        if !before.contains_key(table) {
            changed.push(format!("{table}: table appeared"));
        }
    }
    changed
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn clean_reorg(region: Uuid, idempotency_key: &str) -> CreateOrgChange {
    let proposal = vec![OrgProposalOp::CreateBranch {
        region_id: region,
        name: "신설지점".into(),
    }];
    CreateOrgChange {
        kind: OrgChangeKind::Reorg,
        target: OrgChangeTarget {
            kind: TargetKind::Region,
            target_ref: region.to_string(),
            label: "수도권 개편".into(),
        },
        effective_date: today_kst(),
        reason: "지점 신설".into(),
        proposal: proposal.clone(),
        supersedes_id: None,
        idempotency_key: idempotency_key.to_owned(),
        fingerprint_input: json!({"key": idempotency_key}),
    }
}

fn blocked_reorg(region: Uuid, branch: Uuid, idempotency_key: &str) -> CreateOrgChange {
    CreateOrgChange {
        kind: OrgChangeKind::Reorg,
        target: OrgChangeTarget {
            kind: TargetKind::Region,
            target_ref: region.to_string(),
            label: "수도권 축소".into(),
        },
        effective_date: today_kst(),
        reason: "지점 폐쇄".into(),
        proposal: vec![OrgProposalOp::DeactivateBranch { branch_id: branch }],
        supersedes_id: None,
        idempotency_key: idempotency_key.to_owned(),
        fingerprint_input: json!({"key": idempotency_key}),
    }
}

/// Rewrite `id` into the pre-P3 on-disk shape: `PRECHECKED` plus a stored
/// receipt. Returns the receipt's `computedAt`.
async fn legacy_prechecked(
    pool: &PgPool,
    org: OrgId,
    id: Uuid,
    blockers: serde_json::Value,
) -> OffsetDateTime {
    let computed_at = OffsetDateTime::now_utc() - time::Duration::hours(6);
    let receipt = json!({
        "computedAt": computed_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap(),
        "stale": false,
        "blockers": blockers,
        "warnings": [],
        "headcount": 0,
        "dependentsTotal": 0,
    });
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE org_change_requests SET status = 'PRECHECKED', preflight = $2, updated_at = $3 \
         WHERE id = $1",
    )
    .bind(id)
    .bind(&receipt)
    .bind(computed_at)
    .execute(tx.as_mut())
    .await
    .unwrap();
    tx.commit().await.unwrap();
    computed_at
}

/// RLS-armed status readback (`org_change_requests` is FORCE RLS).
async fn read_status(pool: &PgPool, org: OrgId, id: Uuid) -> String {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let row = sqlx::query("SELECT status FROM org_change_requests WHERE id = $1")
        .bind(id)
        .fetch_one(tx.as_mut())
        .await
        .unwrap();
    let status: String = row.try_get("status").unwrap();
    tx.commit().await.unwrap();
    status
}

async fn seed_org(pool: &PgPool) -> OrgId {
    let slug = format!("oc{}", &Uuid::new_v4().simple().to_string()[..12]);
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO organizations (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(&slug)
            .bind(format!("org-{slug}"))
            .fetch_one(pool)
            .await
            .unwrap();
    OrgId::from_uuid(id)
}

async fn seed_region(pool: &PgPool, org: OrgId, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(*org.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn deactivate_region(pool: &PgPool, org: OrgId, region: Uuid) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let changed = sqlx::query("UPDATE regions SET deactivated_at = now() WHERE id = $1")
        .bind(region)
        .execute(tx.as_mut())
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(changed, 1, "fixture must deactivate the proposal target");
    tx.commit().await.unwrap();
}

async fn deactivate_branch(pool: &PgPool, org: OrgId, branch: Uuid) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.as_uuid().to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let changed = sqlx::query("UPDATE branches SET deactivated_at = now() WHERE id = $1")
        .bind(branch)
        .execute(tx.as_mut())
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(changed, 1, "fixture must deactivate the proposal target");
    tx.commit().await.unwrap();
}

async fn seed_branch(pool: &PgPool, org: OrgId, region: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region)
    .bind(name)
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_user(pool: &PgPool, org: OrgId, role: &str) -> UserId {
    let user = UserId::new();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, is_active, org_id) \
         VALUES ($1, $2, $3, true, $4)",
    )
    .bind(*user.as_uuid())
    .bind(format!("p3-{user}"))
    .bind(vec![role])
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    user
}

async fn seed_user_in_branch(pool: &PgPool, org: OrgId, role: &str, branch: Uuid) -> UserId {
    let user = seed_user(pool, org, role).await;
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user.as_uuid())
        .bind(branch)
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    user
}
