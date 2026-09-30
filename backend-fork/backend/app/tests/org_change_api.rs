#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Authenticated, runtime-role (`console_rt`) story for the org-change lifecycle
//! engine (STORY-ORG-001): draft → preflight → ordered SoD approval →
//! effective-dated apply, plus deny-by-omission authorization, cross-tenant
//! concealment, and audit readback. It crosses the assembled HTTP router.

use std::collections::BTreeMap;

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime, macros::offset};
use tower::ServiceExt;
use uuid::Uuid;

/// `(org_id, actor, anomaly, reason)` as read back from `audit_events` — every
/// column is nullable in the table, so the row shape is wide enough that
/// `clippy::type_complexity` (denied workspace-wide) rejects it inline.
type RefusalAuditRow = (Option<Uuid>, Option<Uuid>, Option<bool>, Option<String>);

const ISSUER: &str = "console-platform-auth";
const AUDIENCE: &str = "console-api";
const CHANGES: &str = "/api/v1/org-changes";

fn today_kst() -> time::Date {
    OffsetDateTime::now_utc().to_offset(offset!(+9)).date()
}

/// The legacy org-setup URLs are compatibility inputs only: orgchange owns
/// their mutation methods, persists one typed DRAFT, and leaves identity's live
/// region/branch tables untouched until the normal approval/apply lifecycle.
async fn assert_legacy_org_setup_mutations_create_governed_drafts_without_direct_apply(
    pool: &PgPool,
    rt: &PgPool,
    keys: &Keys,
    org: OrgId,
) {
    let proposals_before: i64 = sqlx::query_scalar("SELECT count(*) FROM org_change_requests")
        .fetch_one(pool)
        .await
        .unwrap();
    let parent_region = seed_region(pool, org, "legacy-proposal-parent").await;
    let admin_branch = seed_branch(pool, org, parent_region, "legacy-proposal-admin").await;
    let admin = seed_user_in_branch(pool, org, "ADMIN", admin_branch).await;
    let admin_token = keys.token(admin, org, &["ADMIN"]);
    let regions_before: i64 = sqlx::query_scalar("SELECT count(*) FROM regions")
        .fetch_one(pool)
        .await
        .unwrap();
    let branches_before: i64 = sqlx::query_scalar("SELECT count(*) FROM branches")
        .fetch_one(pool)
        .await
        .unwrap();

    let region_body = json!({"name": "  legacy-proposal-region  "});
    let region_key = "legacy-region-proposal-0001";
    let (status, region_draft) = send(
        rt,
        keys,
        "POST",
        "/api/v1/regions",
        &admin_token,
        Some(region_body.clone()),
        Some(region_key),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "region draft: {region_draft}");
    assert_eq!(region_draft["status"], "DRAFT");
    assert_eq!(region_draft["kind"], "NEW");
    assert_eq!(region_draft["proposal"][0]["op"], "CREATE_REGION");
    assert_eq!(
        region_draft["proposal"][0]["name"],
        "legacy-proposal-region"
    );
    let regions_after: i64 = sqlx::query_scalar("SELECT count(*) FROM regions")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        regions_after, regions_before,
        "legacy POST must not apply any region row"
    );

    let (status, replayed_region) = send(
        rt,
        keys,
        "POST",
        "/api/v1/regions",
        &admin_token,
        Some(region_body),
        Some(region_key),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "region replay: {replayed_region}");
    assert_eq!(replayed_region["id"], region_draft["id"]);

    let branch_body = json!({
        "region_id": parent_region,
        "name": "  legacy-proposal-branch  "
    });
    let branch_key = "legacy-branch-proposal-0001";
    let (status, branch_draft) = send(
        rt,
        keys,
        "POST",
        "/api/v1/branches",
        &admin_token,
        Some(branch_body.clone()),
        Some(branch_key),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "branch draft: {branch_draft}");
    assert_eq!(branch_draft["status"], "DRAFT");
    assert_eq!(branch_draft["kind"], "NEW");
    assert_eq!(branch_draft["proposal"][0]["op"], "CREATE_BRANCH");
    assert_eq!(
        branch_draft["proposal"][0]["regionId"],
        parent_region.to_string()
    );
    assert_eq!(
        branch_draft["proposal"][0]["name"],
        "legacy-proposal-branch"
    );
    let branches_after: i64 = sqlx::query_scalar("SELECT count(*) FROM branches")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        branches_after, branches_before,
        "legacy POST must not apply any branch row"
    );

    let (status, replayed_branch) = send(
        rt,
        keys,
        "POST",
        "/api/v1/branches",
        &admin_token,
        Some(branch_body.clone()),
        Some(branch_key),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "branch replay: {replayed_branch}");
    assert_eq!(replayed_branch["id"], branch_draft["id"]);

    let (status, renamed_region) = send(
        rt,
        keys,
        "PATCH",
        &format!("/api/v1/regions/{parent_region}"),
        &admin_token,
        Some(json!({"name": "legacy-proposal-parent-renamed"})),
        Some("legacy-region-rename-0001"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "region rename: {renamed_region}"
    );
    assert_eq!(renamed_region["proposal"][0]["op"], "RENAME_REGION");
    assert_eq!(
        renamed_region["proposal"][0]["regionId"],
        parent_region.to_string()
    );
    let (status, mismatched_region) = send(
        rt,
        keys,
        "PATCH",
        &format!("/api/v1/regions/{parent_region}"),
        &admin_token,
        Some(json!({"name": "legacy-proposal-different-name"})),
        Some("legacy-region-rename-0001"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "PATCH idempotency mismatch: {mismatched_region}"
    );
    let applied_region_name: String = sqlx::query_scalar("SELECT name FROM regions WHERE id = $1")
        .bind(parent_region)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(applied_region_name, "legacy-proposal-parent");

    let (status, deactivated_region) = send(
        rt,
        keys,
        "DELETE",
        &format!("/api/v1/regions/{parent_region}"),
        &admin_token,
        None,
        Some("legacy-region-deactivate-0001"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "region deactivate: {deactivated_region}"
    );
    assert_eq!(deactivated_region["proposal"][0]["op"], "DEACTIVATE_REGION");
    let (status, missing_region_key) = send(
        rt,
        keys,
        "DELETE",
        &format!("/api/v1/regions/{parent_region}"),
        &admin_token,
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "region DELETE requires Idempotency-Key: {missing_region_key}"
    );
    let applied_region_deactivated_at: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT deactivated_at FROM regions WHERE id = $1")
            .bind(parent_region)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(applied_region_deactivated_at.is_none());

    let (status, renamed_branch) = send(
        rt,
        keys,
        "PATCH",
        &format!("/api/v1/branches/{admin_branch}"),
        &admin_token,
        Some(json!({"name": "legacy-proposal-admin-renamed"})),
        Some("legacy-branch-rename-0001"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "branch rename: {renamed_branch}"
    );
    assert_eq!(renamed_branch["proposal"][0]["op"], "RENAME_BRANCH");
    assert_eq!(
        renamed_branch["proposal"][0]["branchId"],
        admin_branch.to_string()
    );
    let applied_branch_name: String = sqlx::query_scalar("SELECT name FROM branches WHERE id = $1")
        .bind(admin_branch)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(applied_branch_name, "legacy-proposal-admin");

    let (status, deactivated_branch) = send(
        rt,
        keys,
        "DELETE",
        &format!("/api/v1/branches/{admin_branch}"),
        &admin_token,
        None,
        Some("legacy-branch-deactivate-0001"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "branch deactivate: {deactivated_branch}"
    );
    assert_eq!(deactivated_branch["proposal"][0]["op"], "DEACTIVATE_BRANCH");
    let (status, missing_branch_key) = send(
        rt,
        keys,
        "DELETE",
        &format!("/api/v1/branches/{admin_branch}"),
        &admin_token,
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "branch DELETE requires Idempotency-Key: {missing_branch_key}"
    );
    let applied_branch_deactivated_at: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT deactivated_at FROM branches WHERE id = $1")
            .bind(admin_branch)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(applied_branch_deactivated_at.is_none());

    let proposals: i64 = sqlx::query_scalar("SELECT count(*) FROM org_change_requests")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        proposals,
        proposals_before + 6,
        "each exact replay must reuse its single draft"
    );

    // Target-state parity with the removed direct mutators: tenant-hidden or
    // missing targets are 404, while already-deactivated targets and inactive
    // branch destinations are 409. None may create a seventh proposal.
    let super_admin = seed_user(pool, org, "SUPER_ADMIN").await;
    let super_token = keys.token(super_admin, org, &["SUPER_ADMIN"]);
    let other_org = seed_org(pool, "legacy-proposal-other").await;
    let other_region = seed_region(pool, other_org, "legacy-other-region").await;
    let other_branch = seed_branch(pool, other_org, other_region, "legacy-other-branch").await;
    let missing_region = Uuid::new_v4();
    let missing_branch = Uuid::new_v4();
    let inactive_region = seed_region(pool, org, "legacy-inactive-region").await;
    let inactive_branch = seed_branch(pool, org, parent_region, "legacy-inactive-branch").await;
    sqlx::query("UPDATE regions SET deactivated_at = now() WHERE id = $1")
        .bind(inactive_region)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE branches SET deactivated_at = now() WHERE id = $1")
        .bind(inactive_branch)
        .execute(pool)
        .await
        .unwrap();

    for (id, label) in [
        (missing_region, "missing region"),
        (other_region, "cross-tenant region"),
    ] {
        let (status, body) = send(
            rt,
            keys,
            "PATCH",
            &format!("/api/v1/regions/{id}"),
            &super_token,
            Some(json!({"name": "must-not-draft"})),
            Some(&format!("legacy-invalid-region-{id}")),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{label}: {body}");
        let (status, body) = send(
            rt,
            keys,
            "DELETE",
            &format!("/api/v1/regions/{id}"),
            &super_token,
            None,
            Some(&format!("legacy-invalid-region-delete-{id}")),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{label} delete: {body}");
    }
    let (status, body) = send(
        rt,
        keys,
        "DELETE",
        &format!("/api/v1/regions/{inactive_region}"),
        &super_token,
        None,
        Some("legacy-inactive-region-0001"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "inactive region: {body}");

    for (region_id, expected, label) in [
        (missing_region, StatusCode::NOT_FOUND, "missing destination"),
        (
            other_region,
            StatusCode::NOT_FOUND,
            "cross-tenant destination",
        ),
        (
            inactive_region,
            StatusCode::CONFLICT,
            "inactive destination",
        ),
    ] {
        let (status, body) = send(
            rt,
            keys,
            "POST",
            "/api/v1/branches",
            &super_token,
            Some(json!({"region_id": region_id, "name": "must-not-draft"})),
            Some(&format!("legacy-invalid-branch-region-{region_id}")),
        )
        .await;
        assert_eq!(status, expected, "{label}: {body}");
    }

    for (branch_id, expected, label) in [
        (missing_branch, StatusCode::NOT_FOUND, "missing branch"),
        (other_branch, StatusCode::NOT_FOUND, "cross-tenant branch"),
        (inactive_branch, StatusCode::CONFLICT, "inactive branch"),
    ] {
        let (status, body) = send(
            rt,
            keys,
            "DELETE",
            &format!("/api/v1/branches/{branch_id}"),
            &super_token,
            None,
            Some(&format!("legacy-invalid-branch-{branch_id}")),
        )
        .await;
        assert_eq!(status, expected, "{label}: {body}");
    }
    for (branch_id, label) in [
        (missing_branch, "missing branch"),
        (other_branch, "cross-tenant branch"),
    ] {
        let (status, body) = send(
            rt,
            keys,
            "PATCH",
            &format!("/api/v1/branches/{branch_id}"),
            &super_token,
            Some(json!({"name": "must-not-draft"})),
            Some(&format!("legacy-invalid-branch-patch-{branch_id}")),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{label} patch: {body}");
    }
    for (region_id, expected, label) in [
        (missing_region, StatusCode::NOT_FOUND, "missing move target"),
        (
            other_region,
            StatusCode::NOT_FOUND,
            "cross-tenant move target",
        ),
        (
            inactive_region,
            StatusCode::CONFLICT,
            "inactive move target",
        ),
    ] {
        let (status, body) = send(
            rt,
            keys,
            "PATCH",
            &format!("/api/v1/branches/{admin_branch}"),
            &super_token,
            Some(json!({"region_id": region_id})),
            Some(&format!("legacy-invalid-branch-move-{region_id}")),
        )
        .await;
        assert_eq!(status, expected, "{label}: {body}");
    }
    let after_invalid_targets: i64 = sqlx::query_scalar("SELECT count(*) FROM org_change_requests")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        after_invalid_targets, proposals,
        "invalid or tenant-hidden targets must write no draft"
    );

    let other_branch = seed_branch(pool, org, parent_region, "legacy-proposal-outside").await;
    let (status, concealed) = send(
        rt,
        keys,
        "PATCH",
        &format!("/api/v1/branches/{other_branch}"),
        &admin_token,
        Some(json!({"name": "legacy-proposal-outside-renamed"})),
        Some("legacy-branch-outside-0001"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "scope concealment: {concealed}"
    );

    let mechanic = seed_user_in_branch(pool, org, "MECHANIC", admin_branch).await;
    let mechanic_token = keys.token(mechanic, org, &["MECHANIC"]);
    let (status, denied) = send(
        rt,
        keys,
        "POST",
        "/api/v1/regions",
        &mechanic_token,
        Some(json!({"name": "legacy-proposal-denied"})),
        Some("legacy-region-denied-0001"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "mechanic denial: {denied}");
    let after_denial: i64 = sqlx::query_scalar("SELECT count(*) FROM org_change_requests")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        after_denial, proposals,
        "authorization denial must write no draft"
    );

    // Replay lookup precedes live target validation. The original CreateBranch
    // DRAFT remains replayable after its destination is deactivated, and its
    // server-owned KST effective date/fingerprint remain the original values.
    sqlx::query("UPDATE regions SET deactivated_at = now() WHERE id = $1")
        .bind(parent_region)
        .execute(pool)
        .await
        .unwrap();
    let (status, replay_after_target_change) = send(
        rt,
        keys,
        "POST",
        "/api/v1/branches",
        &admin_token,
        Some(branch_body),
        Some(branch_key),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "exact replay must survive later target deactivation: {replay_after_target_change}"
    );
    assert_eq!(replay_after_target_change["id"], branch_draft["id"]);
    let after_late_replay: i64 = sqlx::query_scalar("SELECT count(*) FROM org_change_requests")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(after_late_replay, proposals);
}

/// Full REORG lifecycle: idempotent create, preflight as a zero-write READ,
/// ordered SoD chain with self-approval + out-of-order refusals,
/// effective-date gate, one-transaction apply, and audit readback.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn reorg_lifecycle_runs_draft_to_applied_with_ordered_sod(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let org = OrgId::knl();
    let region = seed_region(&pool, org, "리전-개편").await;
    // The drafter is an EXECUTIVE so the self-approval probe below reaches the
    // SoD gate itself instead of stopping at the role floor.
    let drafter = seed_user(&pool, org, "EXECUTIVE").await;
    let approver = seed_user(&pool, org, "EXECUTIVE").await;
    let draft_token = keys.token(drafter, org, &["EXECUTIVE"]);
    let exec_token = keys.token(approver, org, &["EXECUTIVE"]);

    let body = json!({
        "kind": "REORG",
        "target": {"kind": "REGION", "ref": region, "label": "수도권 개편"},
        "effectiveDate": today_kst().to_string(),
        "reason": "지점 신설 및 팀 재배치",
        "proposal": [
            {"op": "CREATE_BRANCH", "regionId": region, "name": "신설지점"},
            {"op": "RENAME_REGION", "regionId": region, "name": "수도권"}
        ]
    });
    let idem = "org-change-story-0001";
    let (status, created) = send(
        &rt,
        &keys,
        "POST",
        CHANGES,
        &draft_token,
        Some(body.clone()),
        Some(idem),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {created}");
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["status"], "DRAFT");
    assert!(
        created["code"].as_str().unwrap().starts_with("OC-"),
        "server-issued code: {created}"
    );

    // Byte-identical replay returns the SAME request (200, not a second row);
    // a changed body under the same key must conflict.
    let (status, replayed) = send(
        &rt,
        &keys,
        "POST",
        CHANGES,
        &draft_token,
        Some(body.clone()),
        Some(idem),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "idempotent replay: {replayed}");
    assert_eq!(replayed["id"].as_str().unwrap(), id);
    let mut changed = body.clone();
    changed["reason"] = json!("다른 사유");
    let (status, conflicted) = send(
        &rt,
        &keys,
        "POST",
        CHANGES,
        &draft_token,
        Some(changed),
        Some(idem),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "changed replay: {conflicted}");

    // Preflight is a READ: it reports the receipt it computed and persists
    // nothing — no status flip, no stored verdict. There is no PRECHECKED step
    // to reach, because submit recomputes the receipt in its own transaction.
    let (status, checked) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/preflight"),
        &draft_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "preflight: {checked}");
    assert_eq!(
        checked["status"], "DRAFT",
        "preflight promotes nothing: {checked}"
    );
    assert_eq!(checked["preflight"]["blockers"], json!([]));
    assert_eq!(checked["preflight"]["stale"], false);
    // ...and the verdict it just reported is not on the row: the next reader
    // sees a DRAFT with no receipt at all.
    let (status, reread) = send(
        &rt,
        &keys,
        "GET",
        &format!("{CHANGES}/{id}"),
        &draft_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "post-preflight read: {reread}");
    assert_eq!(reread["status"], "DRAFT");
    assert_eq!(
        reread["preflight"],
        Value::Null,
        "the preflight verdict is reported, never stored: {reread}"
    );

    // A draft edit leaves it DRAFT, and there is still no stored receipt that
    // could go stale — submit's in-transaction recompute replaced staleness as
    // the defence, so the strongest statement is that nothing is persisted.
    let (status, edited) = send(
        &rt,
        &keys,
        "PATCH",
        &format!("{CHANGES}/{id}"),
        &draft_token,
        Some(json!({"reason": "지점 신설 및 팀 재배치 (수정)"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "draft edit: {edited}");
    assert_eq!(edited["status"], "DRAFT");
    assert_eq!(
        edited["preflight"],
        Value::Null,
        "no stored receipt exists to go stale: {edited}"
    );
    let (status, rechecked) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/preflight"),
        &draft_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "re-preflight: {rechecked}");

    // A row WRITTEN BEFORE P3 still carries a stored receipt: the old preflight
    // saved one, and a later draft edit moved `updated_at` without clearing it.
    // Nothing in the P3 write path can produce that shape any more, so seeding
    // it is the only way to keep the `stale` derivation — which is DERIVED on
    // read, not stored, and is published in the API — under test.
    let legacy_at = seed_legacy_receipt(&pool, &id, "DRAFT").await;
    let (status, legacy) = send(
        &rt,
        &keys,
        "GET",
        &format!("{CHANGES}/{id}"),
        &draft_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "legacy read: {legacy}");
    assert_eq!(
        parse_rfc3339(&legacy["preflight"]["computedAt"]),
        legacy_at,
        "the stored receipt is served as stored: {legacy}"
    );
    assert_eq!(
        legacy["preflight"]["stale"], true,
        "a receipt older than the last draft edit reads back stale: {legacy}"
    );

    let (status, submitted) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/submit"),
        &draft_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "submit: {submitted}");
    assert_eq!(submitted["status"], "IN_APPROVAL");
    // Submit accepted the legacy row and refused to trust its receipt: the
    // receipt now stored is one submit computed inside its own transaction, so
    // it is strictly newer than the seeded one. `stale` is NOT the oracle here
    // — it is derived false for every non-draft-editable status, so it could
    // not fail on an IN_APPROVAL readback whatever submit stored.
    assert_eq!(submitted["preflight"]["blockers"], json!([]));
    assert!(
        parse_rfc3339(&submitted["preflight"]["computedAt"]) > legacy_at,
        "submit recomputed the receipt instead of trusting the stored one: {submitted}"
    );
    let steps = submitted["approvalSteps"].as_array().unwrap();
    assert_eq!(
        steps
            .iter()
            .map(|s| s["roleKey"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["hr", "finance", "legal", "executive"],
        "ordered SoD chain: {submitted}"
    );

    // Post-submit the draft window is closed.
    let (status, locked) = send(
        &rt,
        &keys,
        "PATCH",
        &format!("{CHANGES}/{id}"),
        &draft_token,
        Some(json!({"reason": "너무 늦은 수정"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "post-submit edit: {locked}");

    // Effectuate before approval completes fails closed.
    let (status, premature) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/effectuate"),
        &exec_token,
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "pre-approval apply: {premature}"
    );

    // The drafter may never approve (SoD), and steps decide strictly in order.
    let step_ids: Vec<String> = steps
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect();
    let decision = json!({"decision": "APPROVED"});
    let (status, sod) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/approval-steps/{}/decision", step_ids[0]),
        &draft_token,
        Some(decision.clone()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "self-approval: {sod}");
    let (status, out_of_order) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/approval-steps/{}/decision", step_ids[2]),
        &exec_token,
        Some(decision.clone()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "out of order: {out_of_order}");

    let mut last = Value::Null;
    for step_id in &step_ids {
        let (status, decided) = send(
            &rt,
            &keys,
            "POST",
            &format!("{CHANGES}/{id}/approval-steps/{step_id}/decision"),
            &exec_token,
            Some(decision.clone()),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "decide {step_id}: {decided}");
        last = decided;
    }
    assert_eq!(last["status"], "APPROVED", "all four decided: {last}");
    // Each decision moved `updated_at` past the receipt's `computedAt`, so the
    // only thing holding `stale` false here is the draft-editable guard in the
    // derivation: an APPROVED request cannot be edited, so no edit can have
    // overtaken its receipt.
    assert_eq!(
        last["preflight"]["stale"], false,
        "staleness applies only while the draft is still editable: {last}"
    );
    let (status, redecided) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/approval-steps/{}/decision", step_ids[0]),
        &exec_token,
        Some(decision),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "re-decide: {redecided}");

    let (status, applied) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/effectuate"),
        &exec_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "effectuate: {applied}");
    assert_eq!(applied["status"], "APPLIED");
    let (status, terminal) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/effectuate"),
        &exec_token,
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "terminal re-apply: {terminal}"
    );

    // The proposal really applied, in one transaction, as tenant data.
    let renamed: String = sqlx::query_scalar("SELECT name FROM regions WHERE id = $1")
        .bind(region)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(renamed, "수도권");
    let branches: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM branches WHERE region_id = $1 AND name = '신설지점'",
    )
    .bind(region)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(branches, 1, "CREATE_BRANCH op applied");

    // History layer: the transition chain is readable and complete.
    let (status, detail) = send(
        &rt,
        &keys,
        "GET",
        &format!("{CHANGES}/{id}"),
        &draft_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "detail: {detail}");
    // A COUNTED census, not a set: this lifecycle ran two preflights, and both
    // a `contains` probe and a deduplicated set stay green if a preflight
    // appends an extra row under a name that is already on the list. Row counts
    // are what actually move when a read starts writing.
    let mut events: BTreeMap<&str, usize> = BTreeMap::new();
    for event in detail["events"].as_array().unwrap() {
        *events.entry(event["action"].as_str().unwrap()).or_default() += 1;
    }
    assert_eq!(
        events.into_iter().collect::<Vec<_>>(),
        [
            ("create", 1),
            ("draft.update", 1),
            ("effectuate", 1),
            ("step.decide", 4),
            ("submit", 1),
        ],
        "exact transition chain and row counts; `preflight` is a READ and writes \
         no event: {detail}"
    );

    // Audit spine readback, likewise counted. `#[sqlx::test]` gives this test
    // its own database, so this is every audit row the run produced — a
    // `count >= 1` loop over the actions we WANT would pass while preflight
    // quietly persisted its verdict beside them, and so would a DISTINCT set.
    let audited: Vec<(String, i64)> = sqlx::query_as(
        "SELECT action, count(*) FROM audit_events WHERE action LIKE 'org_change.%' \
         GROUP BY action ORDER BY action",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        audited,
        [
            // Two proposal ops applied: CREATE_BRANCH and RENAME_REGION.
            ("org_change.apply.op".to_owned(), 2),
            ("org_change.create".to_owned(), 1),
            ("org_change.draft.update".to_owned(), 1),
            ("org_change.effectuate".to_owned(), 1),
            ("org_change.step.decide".to_owned(), 4),
            ("org_change.submit".to_owned(), 1),
        ],
        "exact audit spine and row counts; preflight is a READ and appends nothing"
    );
    // Each SoD decision also passed through the gov_approvals second net.
    let gov: i64 =
        sqlx::query_scalar("SELECT count(*) FROM gov_approvals WHERE kind = 'org_change_step'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(gov, 4, "four step decisions recorded through gov_approvals");

    // Exercise the compatibility URLs only after the exact lifecycle audit
    // census above so their own draft audits cannot dilute that oracle.
    assert_legacy_org_setup_mutations_create_governed_drafts_without_direct_apply(
        &pool, &rt, &keys, org,
    )
    .await;
}

/// DISSOLVE defers its ops to archive: effectuate opens the six settlement
/// items, archive fails closed while dependents remain, and the deferred
/// deactivation applies only after settlement genuinely cleared them.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn dissolve_settles_then_archives_with_referential_net(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let org = OrgId::knl();
    let region = seed_region(&pool, org, "리전-폐지").await;
    let branch = seed_branch(&pool, org, region, "폐지지점").await;
    let resident = seed_user_in_branch(&pool, org, "MEMBER", branch).await;
    let drafter = seed_user(&pool, org, "ADMIN").await;
    let approver = seed_user(&pool, org, "EXECUTIVE").await;
    let draft_token = keys.token(drafter, org, &["ADMIN"]);
    let exec_token = keys.token(approver, org, &["EXECUTIVE"]);

    let (status, created) = send(
        &rt,
        &keys,
        "POST",
        CHANGES,
        &draft_token,
        Some(json!({
            "kind": "DISSOLVE",
            "target": {"kind": "BRANCH", "ref": branch, "label": "폐지지점"},
            "effectiveDate": today_kst().to_string(),
            "reason": "지점 폐지",
            "proposal": [{"op": "DEACTIVATE_BRANCH", "branchId": branch}]
        })),
        Some("org-change-dissolve-01"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {created}");
    let id = created["id"].as_str().unwrap().to_owned();

    // Dissolve dependents surface as settlement WARNINGS, never blockers — and
    // the read that surfaces them leaves the request DRAFT, because preflight
    // persists nothing.
    let (status, checked) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/preflight"),
        &draft_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "preflight: {checked}");
    assert_eq!(
        checked["status"], "DRAFT",
        "preflight promotes nothing: {checked}"
    );
    assert_eq!(checked["preflight"]["blockers"], json!([]));
    assert!(
        checked["preflight"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "ACTIVE_USERS"),
        "resident user surfaces as a settlement warning: {checked}"
    );

    // A request written before P3 is on disk as PRECHECKED with a stored
    // receipt. Submit must still accept it — refusing would strand every row
    // written before this change — and must recompute rather than trust it.
    let legacy_at = seed_legacy_receipt(&pool, &id, "PRECHECKED").await;
    let (status, submitted) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/submit"),
        &draft_token,
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "submit a legacy PRECHECKED row: {submitted}"
    );
    assert_eq!(submitted["status"], "IN_APPROVAL");
    assert!(
        parse_rfc3339(&submitted["preflight"]["computedAt"]) > legacy_at,
        "submit recomputed the receipt instead of trusting the stored one: {submitted}"
    );
    for step in submitted["approvalSteps"].as_array().unwrap() {
        let step_id = step["id"].as_str().unwrap();
        let (status, decided) = send(
            &rt,
            &keys,
            "POST",
            &format!("{CHANGES}/{id}/approval-steps/{step_id}/decision"),
            &exec_token,
            Some(json!({"decision": "APPROVED"})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "decide: {decided}");
    }

    let (status, settling) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/effectuate"),
        &exec_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "effectuate: {settling}");
    assert_eq!(settling["status"], "SETTLING");
    let items = settling["settlementItems"].as_array().unwrap();
    assert_eq!(items.len(), 6, "six settlement items seeded: {settling}");

    // Archive is double-gated: first on unsettled items, then on the real
    // referential state (the resident user has not actually been cleared).
    let (status, unsettled) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/archive"),
        &exec_token,
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "unsettled archive: {unsettled}"
    );
    for item in items {
        let item_id = item["id"].as_str().unwrap();
        let (status, done) = send(
            &rt,
            &keys,
            "POST",
            &format!("{CHANGES}/{id}/settlement-items/{item_id}/complete"),
            &exec_token,
            Some(json!({"memo": "정산 완료"})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "complete item: {done}");
    }
    let (status, dishonest) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/archive"),
        &exec_token,
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "checked-off settlement cannot beat the referential net: {dishonest}"
    );

    // Actually clear the dependent, then archive applies the deactivation.
    sqlx::query("UPDATE users SET is_active = false WHERE id = $1")
        .bind(*resident.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    // §3.9.1 동결 창 on the OTHER live-apply path: a DISSOLVE defers its
    // deactivation to archive, so a lock opened after effectuate must still
    // refuse it — and record the attempt.
    let lock = seed_period_lock(
        &pool,
        org,
        "accounting",
        today_kst(),
        today_kst(),
        "결산 중",
    )
    .await;
    let (status, frozen) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/archive"),
        &exec_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "frozen archive: {frozen}");
    assert!(
        frozen["error"]["message"]
            .as_str()
            .unwrap()
            .contains("회계 결산"),
        "refusal names the blocking window: {frozen}"
    );
    let still_active: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT deactivated_at FROM branches WHERE id = $1")
            .bind(branch)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(still_active.is_none(), "frozen archive applied nothing");
    let refused: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'org_change.archive.refused' \
         AND target_id = $1",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(refused, 1, "the refused archive is on the record");
    sqlx::query(
        "UPDATE period_locks SET unlocked_at = now(), unlock_reason = '결산 완료' WHERE id = $1",
    )
    .bind(lock)
    .execute(&pool)
    .await
    .unwrap();

    let (status, archived) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/archive"),
        &exec_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "archive: {archived}");
    assert_eq!(archived["status"], "ARCHIVED");

    // Effective-date gate: a fully-approved change refuses to apply before
    // its effective date (발효일), and the past is rejected at the draft door.
    let (status, past) = send(
        &rt,
        &keys,
        "POST",
        CHANGES,
        &draft_token,
        Some(json!({
            "kind": "REORG",
            "target": {"kind": "REGION", "ref": region, "label": "소급 개편"},
            "effectiveDate": (today_kst() - Duration::days(1)).to_string(),
            "reason": "소급 적용 시도",
            "proposal": []
        })),
        Some("org-change-past-00001"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "past date: {past}"
    );
    let (status, future) = send(
        &rt,
        &keys,
        "POST",
        CHANGES,
        &draft_token,
        Some(json!({
            "kind": "REORG",
            "target": {"kind": "REGION", "ref": region, "label": "예정 개편"},
            "effectiveDate": (today_kst() + Duration::days(7)).to_string(),
            "reason": "발효일 이전 적용 차단 확인",
            "proposal": []
        })),
        Some("org-change-future-0001"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "future draft: {future}");
    let future_id = future["id"].as_str().unwrap().to_owned();
    send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{future_id}/preflight"),
        &draft_token,
        None,
        None,
    )
    .await;
    let (_, submitted) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{future_id}/submit"),
        &draft_token,
        None,
        None,
    )
    .await;
    for step in submitted["approvalSteps"].as_array().unwrap() {
        let step_id = step["id"].as_str().unwrap();
        let (status, decided) = send(
            &rt,
            &keys,
            "POST",
            &format!("{CHANGES}/{future_id}/approval-steps/{step_id}/decision"),
            &exec_token,
            Some(json!({"decision": "APPROVED"})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "decide future: {decided}");
    }
    let (status, early) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{future_id}/effectuate"),
        &exec_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "before 발효일: {early}");
    let deactivated: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT deactivated_at FROM branches WHERE id = $1")
            .bind(branch)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        deactivated.is_some(),
        "deferred deactivation applied at archive"
    );
}

/// Deny-by-default + deny-by-omission: unauthenticated 401, floor-denied 403
/// with the canonical envelope and zero data, REORG blockers refuse submit,
/// cross-tenant rows are concealed as 404/empty, and the entity read fails
/// closed to an empty list without group grants.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn authorization_denies_without_leakage_and_conceals_other_tenants(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let org = OrgId::knl();
    let region = seed_region(&pool, org, "리전-차단").await;
    let branch = seed_branch(&pool, org, region, "상주지점").await;
    let _resident = seed_user_in_branch(&pool, org, "MEMBER", branch).await;
    let drafter = seed_user(&pool, org, "ADMIN").await;
    let member = seed_user(&pool, org, "MEMBER").await;
    let draft_token = keys.token(drafter, org, &["ADMIN"]);
    let member_token = keys.token(member, org, &["MEMBER"]);

    let (status, _) = send(&rt, &keys, "GET", CHANGES, "", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "missing bearer");

    let (status, denied) = send(&rt, &keys, "GET", CHANGES, &member_token, None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "member floor: {denied}");
    assert!(denied["error"]["code"].is_string(), "envelope: {denied}");
    assert!(denied.get("items").is_none(), "no data leaks: {denied}");
    let (status, denied) = send(
        &rt,
        &keys,
        "POST",
        CHANGES,
        &member_token,
        Some(json!({
            "kind": "REORG",
            "target": {"kind": "REGION", "ref": region, "label": "차단"},
            "effectiveDate": today_kst().to_string(),
            "reason": "차단 확인",
            "proposal": []
        })),
        Some("org-change-denied-0001"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "member draft: {denied}");

    // Approve floor is higher than draft floor: ADMIN drafts but cannot decide.
    let (status, created) = send(
        &rt,
        &keys,
        "POST",
        CHANGES,
        &draft_token,
        Some(json!({
            "kind": "REORG",
            "target": {"kind": "BRANCH", "ref": branch, "label": "상주지점 개편"},
            "effectiveDate": today_kst().to_string(),
            "reason": "개편 차단 시나리오",
            "proposal": [{"op": "DEACTIVATE_BRANCH", "branchId": branch}]
        })),
        Some("org-change-blocked-001"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {created}");
    let id = created["id"].as_str().unwrap().to_owned();

    // REORG deactivation with a resident active user is a submit BLOCKER.
    let (status, blocked) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/preflight"),
        &draft_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "preflight: {blocked}");
    assert_eq!(blocked["status"], "DRAFT", "blockers hold DRAFT: {blocked}");
    assert!(
        blocked["preflight"]["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["code"] == "ACTIVE_USERS"),
        "resident user blocks a REORG deactivation: {blocked}"
    );
    let (status, refused) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/submit"),
        &draft_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "blocked submit: {refused}");

    // Cross-tenant concealment: an ADMIN of another org sees nothing — the
    // detail is 404 (not 403) and the list is RLS-empty.
    let other_org = seed_org(&pool, "other-tenant").await;
    let outsider = seed_user(&pool, other_org, "ADMIN").await;
    let outsider_token = keys.token(outsider, other_org, &["ADMIN"]);
    let (status, concealed) = send(
        &rt,
        &keys,
        "GET",
        &format!("{CHANGES}/{id}"),
        &outsider_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "concealed: {concealed}");
    let (status, listed) = send(&rt, &keys, "GET", CHANGES, &outsider_token, None, None).await;
    assert_eq!(status, StatusCode::OK, "outsider list: {listed}");
    assert_eq!(listed["total"], 0, "RLS-filtered list: {listed}");
    let (status, mutated) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/preflight"),
        &outsider_token,
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "concealed mutation: {mutated}"
    );

    // Rejection ends the request; revision is a NEW row via supersedesId, and
    // only a REJECTED request may be superseded.
    let exec = seed_user(&pool, org, "EXECUTIVE").await;
    let exec_token = keys.token(exec, org, &["EXECUTIVE"]);
    let (status, clean) = send(
        &rt,
        &keys,
        "POST",
        CHANGES,
        &draft_token,
        Some(json!({
            "kind": "REORG",
            "target": {"kind": "REGION", "ref": region, "label": "반려 시나리오"},
            "effectiveDate": today_kst().to_string(),
            "reason": "반려 후 재기안 확인",
            "proposal": []
        })),
        Some("org-change-reject-0001"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {clean}");
    let clean_id = clean["id"].as_str().unwrap().to_owned();
    send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{clean_id}/preflight"),
        &draft_token,
        None,
        None,
    )
    .await;
    let (_, submitted) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{clean_id}/submit"),
        &draft_token,
        None,
        None,
    )
    .await;
    let first_step = submitted["approvalSteps"][0]["id"].as_str().unwrap();
    let (status, rejected) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{clean_id}/approval-steps/{first_step}/decision"),
        &exec_token,
        Some(json!({"decision": "REJECTED", "memo": "예산 근거 부족"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "reject: {rejected}");
    assert_eq!(rejected["status"], "REJECTED");
    let (status, revision) = send(
        &rt,
        &keys,
        "POST",
        CHANGES,
        &draft_token,
        Some(json!({
            "kind": "REORG",
            "target": {"kind": "REGION", "ref": region, "label": "반려 시나리오"},
            "effectiveDate": today_kst().to_string(),
            "reason": "예산 근거 보강 재기안",
            "proposal": [],
            "supersedesId": clean_id
        })),
        Some("org-change-reject-0002"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "revision: {revision}");
    assert_eq!(revision["supersedesId"].as_str().unwrap(), clean_id);
    let (status, bad_supersede) = send(
        &rt,
        &keys,
        "POST",
        CHANGES,
        &draft_token,
        Some(json!({
            "kind": "REORG",
            "target": {"kind": "REGION", "ref": region, "label": "잘못된 재기안"},
            "effectiveDate": today_kst().to_string(),
            "reason": "반려되지 않은 건 재기안 시도",
            "proposal": [],
            "supersedesId": id
        })),
        Some("org-change-reject-0003"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "only REJECTED may be superseded: {bad_supersede}"
    );

    // Entity list fails closed to empty without live group grants.
    let (status, entities) = send(
        &rt,
        &keys,
        "GET",
        "/api/v1/org-entities",
        &draft_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "entities: {entities}");
    assert_eq!(entities, json!([]), "fail-closed empty: {entities}");

    // With a live group grant the SECURITY DEFINER resolvers surface the
    // member 법인 — the positive path, not just the fail-closed one.
    let group_id: Uuid = sqlx::query_scalar(
        "INSERT INTO groups (slug, name) VALUES ('knl-holdings', '지주회사') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO group_memberships (group_id, org_id) VALUES ($1, $2)
         ON CONFLICT (org_id) DO UPDATE SET group_id = EXCLUDED.group_id",
    )
    .bind(group_id)
    .bind(*org.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE organizations SET group_id = $1 WHERE id = $2")
        .bind(group_id)
        .bind(*org.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO group_role_grants (group_id, user_id, group_role) \
         VALUES ($1, $2, 'GROUP_VIEWER')",
    )
    .bind(group_id)
    .bind(*drafter.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let (status, entities) = send(
        &rt,
        &keys,
        "GET",
        "/api/v1/org-entities",
        &draft_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "granted entities: {entities}");
    let listed = entities.as_array().unwrap();
    assert!(
        listed
            .iter()
            .any(|e| e["orgId"].as_str() == Some(&org.as_uuid().to_string())),
        "granted 법인 listed: {entities}"
    );
    // The outsider's grant-less token still sees nothing.
    let (status, outsider_entities) = send(
        &rt,
        &keys,
        "GET",
        "/api/v1/org-entities",
        &outsider_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "outsider entities");
    assert_eq!(outsider_entities, json!([]), "no grant, no 법인 list");
}

/// §3.9.1 변경 동결 창: an APPROVED change may not be applied while the period
/// it takes effect in is closed for 급여 마감 or 회계 결산. The refusal is
/// fail-closed and names the blocking window, the request and the org tree are
/// untouched, the attempt itself is audited even though its transaction rolled
/// back, and another tenant's lock over the same window never bites.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn effectuate_is_frozen_inside_a_locked_period_and_records_the_attempt(pool: PgPool) {
    let keys = Keys::generate();
    let rt = runtime_role_pool(&pool).await;
    let org = OrgId::knl();
    let outsider = seed_org(&pool, "freeze-outsider").await;
    let region = seed_region(&pool, org, "리전-동결").await;
    let drafter = seed_user(&pool, org, "ADMIN").await;
    let approver = seed_user(&pool, org, "EXECUTIVE").await;
    let draft_token = keys.token(drafter, org, &["ADMIN"]);
    let exec_token = keys.token(approver, org, &["EXECUTIVE"]);

    let (status, created) = send(
        &rt,
        &keys,
        "POST",
        CHANGES,
        &draft_token,
        Some(json!({
            "kind": "REORG",
            "target": {"kind": "REGION", "ref": region, "label": "동결 검증"},
            "effectiveDate": today_kst().to_string(),
            "reason": "동결 창 적용 확인",
            "proposal": [
                {"op": "CREATE_BRANCH", "regionId": region, "name": "동결지점"},
                {"op": "RENAME_REGION", "regionId": region, "name": "동결후이름"}
            ]
        })),
        Some("org-change-freeze-0001"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {created}");
    let id = created["id"].as_str().unwrap().to_owned();

    // A foreign tenant's lock over the very same window must stay irrelevant,
    // both to the preflight read and to the apply gate.
    seed_period_lock(
        &pool,
        outsider,
        "payroll",
        today_kst(),
        today_kst(),
        "타 법인 급여 마감",
    )
    .await;
    let window_start = today_kst() - Duration::days(3);
    let payroll_lock = seed_period_lock(
        &pool,
        org,
        "payroll",
        window_start,
        today_kst() + Duration::days(3),
        "급여 마감",
    )
    .await;
    let accounting_lock = seed_period_lock(
        &pool,
        org,
        "accounting",
        window_start,
        today_kst() + Duration::days(3),
        "회계 결산",
    )
    .await;

    // Preflight computes the freeze signal from the same locks — it is a read,
    // not a reminder chip, so it names the domains that would refuse the apply.
    // With BOTH domains locked it stays ONE warning: the console keys the
    // warning list by `code`, so a second `FREEZE_WINDOW_REVIEW` row would be a
    // duplicate React key rather than a second signal.
    let (status, prechecked) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/preflight"),
        &draft_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "preflight: {prechecked}");
    let freeze_labels: Vec<&str> = prechecked["preflight"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["code"] == "FREEZE_WINDOW_REVIEW")
        .map(|w| w["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        freeze_labels.len(),
        1,
        "one freeze warning however many domains block: {prechecked}"
    );
    assert!(
        freeze_labels[0].contains("급여 마감") && freeze_labels[0].contains("회계 결산"),
        "the one chip names every blocking domain, in the console's language: {}",
        freeze_labels[0]
    );

    let (status, submitted) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/submit"),
        &draft_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "submit: {submitted}");
    for step in submitted["approvalSteps"].as_array().unwrap() {
        let step_id = step["id"].as_str().unwrap();
        let (status, decided) = send(
            &rt,
            &keys,
            "POST",
            &format!("{CHANGES}/{id}/approval-steps/{step_id}/decision"),
            &exec_token,
            Some(json!({"decision": "APPROVED"})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "decide: {decided}");
    }

    // Both freeze domains block, and each refusal names its own window. The
    // second pass proves the accounting domain gates on its own, with the
    // payroll lock already lifted.
    for (domain, lock) in [("급여 마감", payroll_lock), ("회계 결산", accounting_lock)] {
        let (status, frozen) = send(
            &rt,
            &keys,
            "POST",
            &format!("{CHANGES}/{id}/effectuate"),
            &exec_token,
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{domain} freeze: {frozen}");
        let message = frozen["error"]["message"].as_str().unwrap();
        assert!(
            message.contains(domain) && message.contains(&today_kst().to_string()),
            "refusal names the blocking {domain} window in the console's language: {message}"
        );
        let (_, still) = send(
            &rt,
            &keys,
            "GET",
            &format!("{CHANGES}/{id}"),
            &draft_token,
            None,
            None,
        )
        .await;
        assert_eq!(still["status"], "APPROVED", "refused apply did not advance");
        sqlx::query(
            "UPDATE period_locks SET unlocked_at = now(), unlock_reason = '검증 해제' \
             WHERE id = $1",
        )
        .bind(lock)
        .execute(&pool)
        .await
        .unwrap();
    }

    // Nothing was applied and no success audit landed.
    let branches: i64 = sqlx::query_scalar("SELECT count(*) FROM branches WHERE name = '동결지점'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(branches, 0, "refused apply wrote no org rows");
    let applied: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'org_change.effectuate'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(applied, 0, "a refused apply is not an apply");

    // The attempt IS on the record — its own committed transaction, anomaly
    // flagged, reason naming the window, scoped to this tenant only.
    let refusals: Vec<RefusalAuditRow> = sqlx::query_as(
        "SELECT org_id, actor, anomaly, reason FROM audit_events \
             WHERE action = 'org_change.effectuate.refused' AND target_id = $1 \
             ORDER BY occurred_at",
    )
    .bind(&id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(refusals.len(), 2, "one detection row per refused attempt");
    for (index, domain) in ["급여 마감", "회계 결산"].iter().enumerate() {
        let (row_org, actor, anomaly, reason) = &refusals[index];
        assert_eq!(row_org.as_ref(), Some(org.as_uuid()), "tenant-scoped");
        assert_eq!(actor.as_ref(), Some(approver.as_uuid()), "actor recorded");
        assert_eq!(*anomaly, Some(true), "freeze refusal is an anomaly");
        assert!(
            reason.as_deref().unwrap().contains(domain),
            "reason names the blocking domain: {reason:?}"
        );
    }

    // Outside every lock the legitimate path is preserved — even with an
    // ACTIVE own-tenant lock over a different period and the outsider's lock
    // still covering today.
    seed_period_lock(
        &pool,
        org,
        "payroll",
        today_kst() - Duration::days(40),
        today_kst() - Duration::days(10),
        "지난달 급여 마감",
    )
    .await;
    let (status, applied) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{id}/effectuate"),
        &exec_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "outside the window: {applied}");
    assert_eq!(applied["status"], "APPLIED");
    let renamed: String = sqlx::query_scalar("SELECT name FROM regions WHERE id = $1")
        .bind(region)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(renamed, "동결후이름", "the unfrozen apply really ran");

    // …and with those same two locks still ACTIVE, a fresh draft whose
    // effective date falls outside them carries NO freeze warning: the signal
    // is computed per effective date, not pushed on every preflight.
    let (_, clean) = send(
        &rt,
        &keys,
        "POST",
        CHANGES,
        &draft_token,
        Some(json!({
            "kind": "REORG",
            "target": {"kind": "REGION", "ref": region, "label": "동결 외"},
            "effectiveDate": today_kst().to_string(),
            "reason": "동결창 밖 확인",
            "proposal": []
        })),
        Some("org-change-freeze-0002"),
    )
    .await;
    let clean_id = clean["id"].as_str().unwrap();
    let (status, clean_report) = send(
        &rt,
        &keys,
        "POST",
        &format!("{CHANGES}/{clean_id}/preflight"),
        &draft_token,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "clean preflight: {clean_report}");
    assert!(
        !clean_report["preflight"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "FREEZE_WINDOW_REVIEW"),
        "no lock covers this effective date: {clean_report}"
    );
}

async fn seed_period_lock(
    pool: &PgPool,
    org: OrgId,
    domain: &str,
    start: time::Date,
    end: time::Date,
    reason: &str,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO period_locks (org_id, domain, period_start, period_end, reason) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(domain)
    .bind(start)
    .bind(end)
    .bind(reason)
    .fetch_one(pool)
    .await
    .unwrap()
}

struct Keys {
    private_pem: String,
    public_pem: String,
}

impl Keys {
    fn generate() -> Self {
        let key = SigningKey::random(&mut OsRng);
        Self {
            private_pem: key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
            public_pem: key
                .verifying_key()
                .to_public_key_pem(LineEnding::LF)
                .unwrap(),
        }
    }

    fn token(&self, user: UserId, org: OrgId, roles: &[&str]) -> String {
        JwtIssuer::from_es256_pem(
            JwtSettings {
                issuer: ISSUER.into(),
                audience: AUDIENCE.into(),
                access_token_ttl: Duration::minutes(15),
            },
            self.private_pem.as_bytes(),
            self.public_pem.as_bytes(),
        )
        .unwrap()
        .issue_access_token(AccessTokenInput {
            subject: user,
            org_id: org,
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
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
        .unwrap()
    }
}

async fn runtime_role_pool(owner: &PgPool) -> PgPool {
    PgPoolOptions::new()
        .max_connections(8)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(owner.connect_options().as_ref().clone())
        .await
        .unwrap()
}

async fn send(
    pool: &PgPool,
    keys: &Keys,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
    idempotency_key: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if !token.is_empty() {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(key) = idempotency_key {
        builder = builder.header("Idempotency-Key", key);
    }
    let request = builder
        .body(
            body.map(|v| Body::from(serde_json::to_vec(&v).unwrap()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap();
    let response = build_router(app_state(pool.clone(), keys.public_pem.clone()).unwrap())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            // The request-context layer's own rejections are not JSON; keep
            // the raw text so status assertions still see the real body.
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        },
    )
}

fn app_state(pool: PgPool, public_key: String) -> Result<AppState, console_app::AppError> {
    AppState::new(
        AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
            ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".into()),
            ("CONSOLE_JWT_ISSUER", ISSUER.into()),
            ("CONSOLE_JWT_AUDIENCE", AUDIENCE.into()),
            ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key),
        ])?,
        DatabaseDependency::Postgres(pool),
    )
}

fn parse_rfc3339(value: &Value) -> OffsetDateTime {
    OffsetDateTime::parse(
        value.as_str().expect("an rfc3339 timestamp string"),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
}

/// Rewrite `id` into a pre-P3 on-disk shape: `status` plus a stored preflight
/// receipt six hours older than the row's last write. `updated_at` is left
/// where the last real write put it, so the derived `stale` flag has a later
/// edit to compare the receipt against. Returns the receipt's `computedAt`.
async fn seed_legacy_receipt(pool: &PgPool, id: &str, status: &str) -> OffsetDateTime {
    let computed_at = OffsetDateTime::now_utc() - Duration::hours(6);
    sqlx::query("UPDATE org_change_requests SET status = $2, preflight = $3 WHERE id = $1")
        .bind(Uuid::parse_str(id).unwrap())
        .bind(status)
        .bind(json!({
            "computedAt": computed_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
            "stale": false,
            "blockers": [],
            "warnings": [],
            "headcount": 0,
            "dependentsTotal": 0,
        }))
        .execute(pool)
        .await
        .unwrap();
    computed_at
}

async fn seed_org(pool: &PgPool, slug: &str) -> OrgId {
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO organizations (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(slug)
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
    .bind(format!("story-{user}"))
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
