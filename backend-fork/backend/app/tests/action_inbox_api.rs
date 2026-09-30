#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Unified action inbox (`GET /api/v1/me/action-inbox`).
//!
//! Drives the REAL router on a genuine non-owner `console_rt` pool (RLS actually
//! enforced, never a BYPASSRLS superuser). Locks the two visibility invariants
//! for the typed work-order source of the aggregate:
//!   * an item surfaces ONLY when it is assigned to the caller — a work order
//!     assigned to another user in the SAME org never appears (no widening);
//!   * the aggregate is org-scoped — a work order assigned to the caller's user
//!     id but living in ANOTHER org is invisible (RLS confinement of the raw
//!     query under `with_org_conn`).

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, SupportTicketId, UserId, WorkOrderId};
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

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU16, Ordering};

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const PATH: &str = "/api/v1/me/action-inbox";
static EQUIPMENT_SEQUENCE: AtomicU16 = AtomicU16::new(1);

struct Keys {
    private_pem: String,
    public_pem: String,
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

// ===========================================================================
// The inbox surfaces only the caller's own assigned work orders. A WO assigned
// to another user in the same org must never appear.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn action_inbox_returns_only_my_assigned_work_orders(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool, OrgId::knl(), "리전", "지사").await;

    let alice = UserId::new();
    seed_user(&pool, OrgId::knl(), alice, "MEMBER", branch).await;
    let bob = UserId::new();
    seed_user(&pool, OrgId::knl(), bob, "MEMBER", branch).await;

    let equipment = seed_equipment(&pool, OrgId::knl(), branch, "0001").await;
    let alice_wo = seed_assigned_work_order(
        &pool,
        OrgId::knl(),
        branch,
        equipment,
        alice,
        "20260701-101",
    )
    .await;
    // Bob's own assigned work order in the SAME org — must not leak to alice.
    let _bob_wo =
        seed_assigned_work_order(&pool, OrgId::knl(), branch, equipment, bob, "20260701-102").await;

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    let resp = get(
        service,
        PATH,
        &bearer(&keys, OrgId::knl(), alice, "MEMBER", branch),
    )
    .await;
    assert_eq!(resp.status, StatusCode::OK, "{:?}", resp.json);
    let items = resp.json["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "alice must see exactly her one assigned WO: {:?}",
        resp.json
    );
    assert_eq!(items[0]["kind"], "work");
    assert_eq!(items[0]["id"], format!("work:{}", alice_wo));
    assert_eq!(items[0]["ref"], "20260701-101");
    assert_eq!(items[0]["done"], false);
    assert_eq!(resp.json["total"], 1);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn action_inbox_work_orders_are_fail_closed_to_the_token_branch_scope(pool: PgPool) {
    let keys = keys();
    let allowed_branch = seed_branch(&pool, OrgId::knl(), "허용 리전", "허용 지사").await;
    let other_branch = seed_branch(&pool, OrgId::knl(), "제외 리전", "제외 지사").await;
    let alice = UserId::new();
    seed_user(&pool, OrgId::knl(), alice, "MEMBER", allowed_branch).await;
    let allowed_equipment =
        seed_equipment(&pool, OrgId::knl(), allowed_branch, "branch-allowed").await;
    let other_equipment = seed_equipment(&pool, OrgId::knl(), other_branch, "branch-other").await;
    let allowed_work = seed_assigned_work_order(
        &pool,
        OrgId::knl(),
        allowed_branch,
        allowed_equipment,
        alice,
        "20260703-101",
    )
    .await;
    let _other_work = seed_assigned_work_order(
        &pool,
        OrgId::knl(),
        other_branch,
        other_equipment,
        alice,
        "20260703-102",
    )
    .await;

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let response = get(
        service,
        PATH,
        &bearer(&keys, OrgId::knl(), alice, "MEMBER", allowed_branch),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK, "{:?}", response.json);
    assert_eq!(response.json["total"], 1, "{:?}", response.json);
    assert_eq!(
        response.json["items"].as_array().map(Vec::len),
        Some(1),
        "{:?}",
        response.json
    );
    assert_eq!(
        response.json["items"][0]["id"],
        format!("work:{allowed_work}")
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn action_inbox_keyset_pages_past_the_old_two_hundred_item_cap(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool, OrgId::knl(), "페이지 리전", "페이지 지사").await;
    let alice = UserId::new();
    seed_user(&pool, OrgId::knl(), alice, "MEMBER", branch).await;
    let equipment = seed_equipment(&pool, OrgId::knl(), branch, "page").await;
    for index in 0..205 {
        seed_assigned_work_order(
            &pool,
            OrgId::knl(),
            branch,
            equipment,
            alice,
            &format!("20260702-{index:03}"),
        )
        .await;
    }

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&keys, OrgId::knl(), alice, "MEMBER", branch);
    let first = get(service.clone(), &format!("{PATH}?limit=200"), &token).await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.json);
    assert_eq!(first.json["items"].as_array().map(Vec::len), Some(200));
    assert_eq!(first.json["total"], 205);
    assert_eq!(first.json["total_is_exact"], true);
    let cursor = first.json["next_cursor"].as_str().expect("second page");
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("limit", "200")
        .append_pair("cursor", cursor)
        .finish();
    let second = get(service, &format!("{PATH}?{query}"), &token).await;
    assert_eq!(second.status, StatusCode::OK, "{:?}", second.json);
    assert_eq!(second.json["items"].as_array().map(Vec::len), Some(5));
    assert!(second.json["next_cursor"].is_null());
    let first_ids = first.json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert!(
        second.json["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| !first_ids.contains(item["id"].as_str().unwrap()))
    );
}

// ===========================================================================
// The aggregate is org-scoped: a WO assigned to alice's user id but in ANOTHER
// org is invisible to her KNL-scoped token (RLS confines the raw query).
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn action_inbox_is_org_scoped(pool: PgPool) {
    let keys = keys();

    // Alice belongs to KNL (her real org / token org).
    let knl_branch = seed_branch(&pool, OrgId::knl(), "리전", "지사").await;
    let alice = UserId::new();
    seed_user(&pool, OrgId::knl(), alice, "MEMBER", knl_branch).await;

    // A work order in ANOTHER org, assigned to alice's user id. RLS must hide it
    // from alice's KNL-scoped read even though the assignment matches her uuid.
    let other_org = seed_org(&pool, "other-co", "Other Co").await;
    let other_branch = seed_branch(&pool, other_org, "리전", "지사").await;
    let equipment = seed_equipment(&pool, other_org, other_branch, "0001").await;
    let _wo = seed_assigned_work_order(
        &pool,
        other_org,
        other_branch,
        equipment,
        alice,
        "20260701-201",
    )
    .await;

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    // Alice authenticates against KNL, where she has no assigned work.
    let resp = get(
        service,
        PATH,
        &bearer(&keys, OrgId::knl(), alice, "MEMBER", knl_branch),
    )
    .await;
    assert_eq!(resp.status, StatusCode::OK, "{:?}", resp.json);
    assert_eq!(
        resp.json["items"].as_array().map(Vec::len),
        Some(0),
        "another org's work order must never surface: {:?}",
        resp.json
    );
    assert_eq!(resp.json["total"], 0);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn immutable_keyset_traverses_mixed_sources_despite_due_mutation(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool, OrgId::knl(), "혼합 리전", "혼합 지사").await;
    let alice = UserId::new();
    seed_user(&pool, OrgId::knl(), alice, "MEMBER", branch).await;
    let bob = UserId::new();
    seed_user(&pool, OrgId::knl(), bob, "MEMBER", branch).await;
    let equipment = seed_equipment(&pool, OrgId::knl(), branch, "mixed").await;
    let base = OffsetDateTime::now_utc() - Duration::hours(1);

    let work = seed_assigned_work_order(
        &pool,
        OrgId::knl(),
        branch,
        equipment,
        alice,
        "20260704-101",
    )
    .await;
    set_work_times(&pool, work, base, base + Duration::hours(4)).await;
    let support = seed_assigned_support_ticket(&pool, OrgId::knl(), branch, alice, base).await;
    let hidden_workflow = seed_assigned_workflow_task(
        &pool,
        OrgId::knl(),
        alice,
        base - Duration::seconds(1),
        Some("unknown_policy"),
    )
    .await;
    let workflow = seed_assigned_workflow_task(&pool, OrgId::knl(), alice, base, None).await;
    let dispatch_work =
        seed_assigned_work_order(&pool, OrgId::knl(), branch, equipment, bob, "20260704-102").await;
    let dispatch =
        seed_dispatch_offer(&pool, OrgId::knl(), branch, dispatch_work, bob, alice, base).await;

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&keys, OrgId::knl(), alice, "MEMBER", branch);
    let first = get(service.clone(), &format!("{PATH}?limit=2"), &token).await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.json);
    assert_eq!(first.json["items"].as_array().map(Vec::len), Some(2));
    assert_eq!(first.json["total"], 4);
    assert_eq!(first.json["total_is_exact"], true);
    let cursor = first.json["next_cursor"].as_str().expect("second page");

    sqlx::query(
        "UPDATE work_orders SET target_due_at = target_due_at + interval '30 days' WHERE id = $1",
    )
    .bind(*work.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE support_tickets SET due_at = now() - interval '30 days' WHERE id = $1")
        .bind(*support.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE workflow_waiting_tasks SET due_at = now() + interval '45 days' WHERE id = $1",
    )
    .bind(workflow)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE p1_dispatches SET accept_window_ends_at = now() + interval '45 days' WHERE id = $1",
    )
    .bind(dispatch)
    .execute(&pool)
    .await
    .unwrap();
    let post_snapshot = seed_assigned_work_order(
        &pool,
        OrgId::knl(),
        branch,
        equipment,
        alice,
        "20260704-103",
    )
    .await;
    let after_as_of = OffsetDateTime::now_utc() + Duration::days(1);
    set_work_times(&pool, post_snapshot, after_as_of, after_as_of).await;

    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("limit", "2")
        .append_pair("cursor", cursor)
        .finish();
    let second = get(service, &format!("{PATH}?{query}"), &token).await;
    assert_eq!(second.status, StatusCode::OK, "{:?}", second.json);
    assert_eq!(second.json["items"].as_array().map(Vec::len), Some(2));
    assert_eq!(second.json["total"], 4);
    assert_eq!(second.json["total_is_exact"], true);
    assert!(second.json["next_cursor"].is_null());
    let ids = first.json["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second.json["items"].as_array().unwrap())
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let unique = ids.iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(
        ids.len(),
        4,
        "all pre-as_of mixed rows must traverse: {ids:?}"
    );
    assert_eq!(unique.len(), 4, "immutable keyset must not duplicate rows");
    assert!(
        !ids.contains(&format!("approval:{hidden_workflow}")),
        "authorization-filtered workflow rows must not inflate totals or consume the page"
    );
    assert_eq!(
        ids,
        vec![
            format!("approval:{workflow}"),
            format!("dispatch:{dispatch}"),
            format!("support:{support}"),
            format!("work:{work}"),
        ],
        "equal-created-at rows must use the global namespaced-id tie break"
    );
    assert!(!ids.contains(&format!("work:{post_snapshot}")));
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn live_dispatch_expiry_can_disappear_between_pages(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool, OrgId::knl(), "라이브 리전", "라이브 지사").await;
    let alice = UserId::new();
    seed_user(&pool, OrgId::knl(), alice, "MEMBER", branch).await;
    let bob = UserId::new();
    seed_user(&pool, OrgId::knl(), bob, "MEMBER", branch).await;
    let equipment = seed_equipment(&pool, OrgId::knl(), branch, "live").await;
    let first_work = seed_assigned_work_order(
        &pool,
        OrgId::knl(),
        branch,
        equipment,
        alice,
        "20260705-101",
    )
    .await;
    let dispatch_work =
        seed_assigned_work_order(&pool, OrgId::knl(), branch, equipment, bob, "20260705-102").await;
    let first_created_at = OffsetDateTime::now_utc() - Duration::minutes(2);
    set_work_times(
        &pool,
        first_work,
        first_created_at,
        OffsetDateTime::now_utc(),
    )
    .await;
    let dispatch = seed_dispatch_offer(
        &pool,
        OrgId::knl(),
        branch,
        dispatch_work,
        bob,
        alice,
        OffsetDateTime::now_utc() - Duration::minutes(1),
    )
    .await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&keys, OrgId::knl(), alice, "MEMBER", branch);
    let first = get(service.clone(), &format!("{PATH}?limit=1"), &token).await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.json);
    assert_eq!(first.json["items"][0]["id"], format!("work:{first_work}"));
    let cursor = first.json["next_cursor"].as_str().expect("second page");
    sqlx::query("UPDATE p1_dispatches SET accept_window_ends_at = now() - interval '1 second' WHERE id = $1")
        .bind(dispatch)
        .execute(&pool)
        .await
        .unwrap();
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("limit", "1")
        .append_pair("cursor", cursor)
        .finish();
    let second = get(service, &format!("{PATH}?{query}"), &token).await;
    assert_eq!(second.status, StatusCode::OK, "{:?}", second.json);
    assert_eq!(second.json["items"].as_array().map(Vec::len), Some(0));
    assert_eq!(
        second.json["total"], 1,
        "total is live-at-as_of, not frozen"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn later_source_failure_returns_no_partial_queue(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool, OrgId::knl(), "실패 리전", "실패 지사").await;
    let alice = UserId::new();
    seed_user(&pool, OrgId::knl(), alice, "MEMBER", branch).await;
    sqlx::query("DROP TABLE support_tickets CASCADE")
        .execute(&pool)
        .await
        .unwrap();
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let resp = get(
        service,
        PATH,
        &bearer(&keys, OrgId::knl(), alice, "MEMBER", branch),
    )
    .await;
    assert_eq!(
        resp.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "{:?}",
        resp.json
    );
    assert!(
        resp.json.get("items").is_none(),
        "partial items must not escape"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn rejects_forged_and_future_cursors(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool, OrgId::knl(), "커서 리전", "커서 지사").await;
    let alice = UserId::new();
    seed_user(&pool, OrgId::knl(), alice, "MEMBER", branch).await;
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());
    let token = bearer(&keys, OrgId::knl(), alice, "MEMBER", branch);
    let forged = get(
        service.clone(),
        &format!("{PATH}?cursor=not-a-cursor"),
        &token,
    )
    .await;
    assert_eq!(forged.status, StatusCode::UNPROCESSABLE_ENTITY);
    let future = OffsetDateTime::now_utc() + Duration::days(1);
    let raw = format!(
        "{}~{}~work:{}",
        future.unix_timestamp_nanos(),
        future.unix_timestamp_nanos(),
        uuid::Uuid::new_v4()
    );
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("cursor", &raw)
        .finish();
    let future_resp = get(service, &format!("{PATH}?{query}"), &token).await;
    assert_eq!(future_resp.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn action_inbox_payroll_source_enforces_sod_and_org_wide_manage(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool, OrgId::knl(), "급여 리전", "급여 지사").await;
    let submitter = UserId::new();
    seed_user(&pool, OrgId::knl(), submitter, "EXECUTIVE", branch).await;
    let approver = UserId::new();
    seed_user(&pool, OrgId::knl(), approver, "EXECUTIVE", branch).await;
    let admin = UserId::new();
    seed_user(&pool, OrgId::knl(), admin, "ADMIN", branch).await;

    let submitted_at = OffsetDateTime::now_utc() - Duration::minutes(1);
    let run = seed_submitted_payroll_run(
        &pool,
        OrgId::knl(),
        submitter,
        "inbox-sod-run",
        submitted_at,
    )
    .await;
    let _calculated = seed_payroll_run(
        &pool,
        OrgId::knl(),
        "inbox-calc-run",
        "CALCULATED",
        None,
        None,
    )
    .await;

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    let submitter_resp = get(
        service.clone(),
        PATH,
        &bearer(&keys, OrgId::knl(), submitter, "EXECUTIVE", branch),
    )
    .await;
    assert_eq!(
        submitter_resp.status,
        StatusCode::OK,
        "{:?}",
        submitter_resp.json
    );
    assert_eq!(
        payroll_ids(&submitter_resp.json),
        Vec::<String>::new(),
        "submitter must not see their own SUBMITTED run: {:?}",
        submitter_resp.json
    );
    assert_eq!(submitter_resp.json["total"], 0);

    // Workbench reuses PgActionInboxSources via list_complete_action_inbox.
    let submitter_workbench = get(
        service.clone(),
        "/api/v1/me/workbench",
        &bearer(&keys, OrgId::knl(), submitter, "EXECUTIVE", branch),
    )
    .await;
    assert_eq!(
        submitter_workbench.status,
        StatusCode::OK,
        "{:?}",
        submitter_workbench.json
    );
    assert_eq!(
        submitter_workbench.json["action_inbox"]["status"], "ok",
        "{:?}",
        submitter_workbench.json
    );
    assert_eq!(
        payroll_ids(&submitter_workbench.json["action_inbox"]),
        Vec::<String>::new(),
        "submitter must not see their own SUBMITTED run through workbench list_source_page: {:?}",
        submitter_workbench.json
    );
    assert_eq!(submitter_workbench.json["action_inbox"]["total"], 0);

    let approver_resp = get(
        service.clone(),
        PATH,
        &bearer(&keys, OrgId::knl(), approver, "EXECUTIVE", branch),
    )
    .await;
    assert_eq!(
        approver_resp.status,
        StatusCode::OK,
        "{:?}",
        approver_resp.json
    );
    assert_eq!(
        payroll_ids(&approver_resp.json),
        vec![format!("payroll:{run}")],
        "{:?}",
        approver_resp.json
    );
    assert_eq!(approver_resp.json["items"][0]["kind"], "payroll");
    assert_eq!(approver_resp.json["items"][0]["ref"], "inbox-sod-run");
    assert_eq!(approver_resp.json["total"], 1);

    let approver_workbench = get(
        service.clone(),
        "/api/v1/me/workbench",
        &bearer(&keys, OrgId::knl(), approver, "EXECUTIVE", branch),
    )
    .await;
    assert_eq!(
        approver_workbench.status,
        StatusCode::OK,
        "{:?}",
        approver_workbench.json
    );
    assert_eq!(
        approver_workbench.json["action_inbox"]["status"], "ok",
        "{:?}",
        approver_workbench.json
    );
    assert_eq!(
        payroll_ids(&approver_workbench.json["action_inbox"]),
        vec![format!("payroll:{run}")],
        "{:?}",
        approver_workbench.json
    );
    assert_eq!(approver_workbench.json["action_inbox"]["total"], 1);

    let admin_resp = get(
        service,
        PATH,
        &bearer(&keys, OrgId::knl(), admin, "ADMIN", branch),
    )
    .await;
    assert_eq!(admin_resp.status, StatusCode::OK, "{:?}", admin_resp.json);
    assert_eq!(
        payroll_ids(&admin_resp.json),
        Vec::<String>::new(),
        "branch-scoped ADMIN must see zero payroll inbox items: {:?}",
        admin_resp.json
    );
    assert_eq!(admin_resp.json["total"], 0);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn action_inbox_governance_source_enforces_requester_sod(pool: PgPool) {
    let keys = keys();
    let branch = seed_branch(&pool, OrgId::knl(), "결재 리전", "결재 지사").await;
    let requester = UserId::new();
    seed_user(&pool, OrgId::knl(), requester, "SUPER_ADMIN", branch).await;
    let approver = UserId::new();
    seed_user(&pool, OrgId::knl(), approver, "SUPER_ADMIN", branch).await;
    let member = UserId::new();
    seed_user(&pool, OrgId::knl(), member, "MEMBER", branch).await;
    // company.*/hr.*/payroll.* four-eyes is a distinct-natural-person bar.
    bind_user_to_new_person(&pool, OrgId::knl(), requester, "gov-inbox-req", member).await;
    bind_user_to_new_person(&pool, OrgId::knl(), approver, "gov-inbox-app", member).await;

    let created_at = OffsetDateTime::now_utc() - Duration::minutes(1);
    let company =
        seed_pending_approval(&pool, OrgId::knl(), requester, "company.revise", created_at).await;
    let hr = seed_pending_approval(
        &pool,
        OrgId::knl(),
        requester,
        "hr.appoint",
        created_at + Duration::seconds(1),
    )
    .await;
    let payroll = seed_pending_approval(
        &pool,
        OrgId::knl(),
        requester,
        "payroll.create_run",
        created_at + Duration::seconds(2),
    )
    .await;
    let decided = seed_pending_approval(
        &pool,
        OrgId::knl(),
        requester,
        "override",
        created_at - Duration::seconds(1),
    )
    .await;
    seed_approval_decision(
        &pool,
        OrgId::knl(),
        decided.request_ref,
        requester,
        approver,
    )
    .await;
    let expected_governance = vec![
        format!("governance:{}", company.id),
        format!("governance:{}", hr.id),
        format!("governance:{}", payroll.id),
    ];

    let service =
        build_router(app_state(runtime_role_pool(&pool).await, keys.public_pem.clone()).unwrap());

    let requester_resp = get(
        service.clone(),
        PATH,
        &bearer(&keys, OrgId::knl(), requester, "SUPER_ADMIN", branch),
    )
    .await;
    assert_eq!(
        requester_resp.status,
        StatusCode::OK,
        "{:?}",
        requester_resp.json
    );
    assert_eq!(
        governance_ids(&requester_resp.json),
        Vec::<String>::new(),
        "requester must not see their own pending company.*/hr.*/payroll.* items: {:?}",
        requester_resp.json
    );
    assert_eq!(requester_resp.json["total"], 0);

    // Workbench reuses PgActionInboxSources via list_complete_action_inbox.
    let requester_workbench = get(
        service.clone(),
        "/api/v1/me/workbench",
        &bearer(&keys, OrgId::knl(), requester, "SUPER_ADMIN", branch),
    )
    .await;
    assert_eq!(
        requester_workbench.status,
        StatusCode::OK,
        "{:?}",
        requester_workbench.json
    );
    assert_eq!(
        requester_workbench.json["action_inbox"]["status"], "ok",
        "{:?}",
        requester_workbench.json
    );
    assert_eq!(
        governance_ids(&requester_workbench.json["action_inbox"]),
        Vec::<String>::new(),
        "requester must not see their own pending four-eyes item through workbench list_source_page: {:?}",
        requester_workbench.json
    );
    assert_eq!(requester_workbench.json["action_inbox"]["total"], 0);

    let approver_resp = get(
        service.clone(),
        PATH,
        &bearer(&keys, OrgId::knl(), approver, "SUPER_ADMIN", branch),
    )
    .await;
    assert_eq!(
        approver_resp.status,
        StatusCode::OK,
        "{:?}",
        approver_resp.json
    );
    assert_eq!(
        governance_ids(&approver_resp.json),
        expected_governance,
        "{:?}",
        approver_resp.json
    );
    assert_eq!(approver_resp.json["items"][0]["kind"], "governance");
    assert_eq!(approver_resp.json["items"][0]["title"], "company.revise");
    assert_eq!(approver_resp.json["items"][1]["title"], "hr.appoint");
    assert_eq!(
        approver_resp.json["items"][2]["title"],
        "payroll.create_run"
    );
    assert_eq!(
        approver_resp.json["items"][0]["ref"],
        company.request_ref.to_string()
    );
    assert_eq!(approver_resp.json["total"], 3);

    let approver_workbench = get(
        service.clone(),
        "/api/v1/me/workbench",
        &bearer(&keys, OrgId::knl(), approver, "SUPER_ADMIN", branch),
    )
    .await;
    assert_eq!(
        approver_workbench.status,
        StatusCode::OK,
        "{:?}",
        approver_workbench.json
    );
    assert_eq!(
        approver_workbench.json["action_inbox"]["status"], "ok",
        "{:?}",
        approver_workbench.json
    );
    // Workbench merges sources; membership is the SoD lock, not inter-source
    // sort order. `/action-inbox` above already pins created_at order.
    let workbench_ids: BTreeSet<String> = governance_ids(&approver_workbench.json["action_inbox"])
        .into_iter()
        .collect();
    let expected_ids: BTreeSet<String> = expected_governance.iter().cloned().collect();
    assert_eq!(workbench_ids, expected_ids, "{:?}", approver_workbench.json);
    assert_eq!(approver_workbench.json["action_inbox"]["total"], 3);

    let member_resp = get(
        service,
        PATH,
        &bearer(&keys, OrgId::knl(), member, "MEMBER", branch),
    )
    .await;
    assert_eq!(member_resp.status, StatusCode::OK, "{:?}", member_resp.json);
    assert_eq!(
        governance_ids(&member_resp.json),
        Vec::<String>::new(),
        "a caller who cannot decide must not see pending four-eyes: {:?}",
        member_resp.json
    );
    assert_eq!(member_resp.json["total"], 0);
}

// ===========================================================================
// Helpers.
// ===========================================================================

async fn seed_org(pool: &PgPool, slug: &str, name: &str) -> OrgId {
    let id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(slug)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    OrgId::from_uuid(id)
}

async fn seed_branch(pool: &PgPool, org: OrgId, region_name: &str, branch_name: &str) -> BranchId {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(region_name)
            .bind(*org.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(branch_name)
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(pool: &PgPool, org: OrgId, user_id: UserId, role: &str, branch: BranchId) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("inbox-{role}-{}", user_id.as_uuid()))
        .bind(vec![role])
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch.as_uuid())
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_equipment(pool: &PgPool, org: OrgId, branch: BranchId, tag: &str) -> uuid::Uuid {
    let sequence = EQUIPMENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    assert!(sequence <= 9_999, "equipment fixture sequence exhausted");
    let customer_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(format!("Customer {tag}"))
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(customer_id)
    .bind(format!("Site {tag}"))
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, model, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, $4, $5,
                'A', 'B', 'C', '임대', '좌식', '2.5', 'GTS25DE', 'test', 1, $6)
        RETURNING id
        "#,
    )
    .bind(*branch.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(format!("ABC12-{sequence:04}"))
    .bind(format!("MG-{sequence:04}"))
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_assigned_work_order(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    equipment_id: uuid::Uuid,
    mechanic: UserId,
    request_no: &str,
) -> WorkOrderId {
    let work_order_id = WorkOrderId::new();
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, priority, symptom, org_id
        )
        SELECT $1, $2, $3, e.id, e.customer_id, e.site_id,
               $4, 'ASSIGNED', 'UNSET', 'inbox fixture', $6
        FROM registry_equipment e
        WHERE e.id = $5
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(request_no)
    .bind(*branch.as_uuid())
    .bind(*mechanic.as_uuid())
    .bind(equipment_id)
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO work_order_assignments (work_order_id, mechanic_id, role, assigned_at, org_id)
        VALUES ($1, $2, 'PRIMARY', now(), $3)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*mechanic.as_uuid())
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    work_order_id
}

async fn set_work_times(
    pool: &PgPool,
    work_order: WorkOrderId,
    created_at: OffsetDateTime,
    due_at: OffsetDateTime,
) {
    sqlx::query("UPDATE work_orders SET created_at = $2, target_due_at = $3 WHERE id = $1")
        .bind(*work_order.as_uuid())
        .bind(created_at)
        .bind(due_at)
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_assigned_support_ticket(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    assignee: UserId,
    created_at: OffsetDateTime,
) -> SupportTicketId {
    let id = SupportTicketId::new();
    sqlx::query(
        "INSERT INTO support_tickets (id, branch_id, origin, category, priority, status, \
         title, body, requester_user_id, assignee_user_id, due_at, created_at, updated_at, org_id) \
         VALUES ($1, $2, 'INTERNAL', 'OPERATIONAL', 'MEDIUM', 'OPEN', $3, 'details', \
                 $4, $4, $5, $6, $6, $7)",
    )
    .bind(*id.as_uuid())
    .bind(*branch.as_uuid())
    .bind(format!("ticket-{id}"))
    .bind(*assignee.as_uuid())
    .bind(created_at + Duration::hours(2))
    .bind(created_at)
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn seed_assigned_workflow_task(
    pool: &PgPool,
    org: OrgId,
    assignee: UserId,
    created_at: OffsetDateTime,
    required_policy: Option<&str>,
) -> uuid::Uuid {
    let definition_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_definitions \
         (org_id, workflow_key, display_name, object_type, status, latest_version, active_version) \
         VALUES ($1, $2, 'Inbox fixture', 'work_order', 'ACTIVE', 1, 1) RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(format!("inbox.fixture_{}", uuid::Uuid::new_v4().simple()))
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_definition_versions \
         (org_id, definition_id, version, status, definition) \
         VALUES ($1, $2, 1, 'PUBLISHED', $3)",
    )
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(json!({
        "schema_version": "wf.exec.v1",
        "workflow_key": "inbox.fixture",
        "nodes": [{"node_key": "review", "node_type": "human_task", "title": "Review"}],
        "edges": []
    }))
    .execute(pool)
    .await
    .unwrap();
    let run_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_runs \
         (id, org_id, definition_id, definition_version, status, trigger_type, \
          idempotency_key, correlation_id, initiated_by) \
         VALUES ($1, $2, $3, 1, 'WAITING', 'MANUAL', $4, $5, $6)",
    )
    .bind(run_id)
    .bind(*org.as_uuid())
    .bind(definition_id)
    .bind(format!("inbox-idempotency-{run_id}"))
    .bind(format!("inbox-correlation-{run_id}"))
    .bind(*assignee.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    let task_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_waiting_tasks \
         (id, org_id, run_id, waiting_key, title, status, assignee_role_key, claimed_by, \
          claimed_at, required_policy, due_at, created_at, updated_at) \
         VALUES ($1, $2, $3, 'review', 'Inbox approval', 'CLAIMED', 'branch_manager', $4, \
                 $7, $5, $6, $7, $7)",
    )
    .bind(task_id)
    .bind(*org.as_uuid())
    .bind(run_id)
    .bind(*assignee.as_uuid())
    .bind(required_policy)
    .bind(created_at + Duration::hours(3))
    .bind(created_at)
    .execute(pool)
    .await
    .unwrap();
    task_id
}

struct PendingApproval {
    id: uuid::Uuid,
    request_ref: uuid::Uuid,
}

fn payroll_ids(json: &Value) -> Vec<String> {
    json["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item["id"]
                        .as_str()
                        .filter(|id| id.starts_with("payroll:"))
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn governance_ids(json: &Value) -> Vec<String> {
    json["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item["id"]
                        .as_str()
                        .filter(|id| id.starts_with("governance:"))
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn seed_submitted_payroll_run(
    pool: &PgPool,
    org: OrgId,
    submitter: UserId,
    source_label: &str,
    submitted_at: OffsetDateTime,
) -> uuid::Uuid {
    seed_payroll_run(
        pool,
        org,
        source_label,
        "SUBMITTED",
        Some(submitter),
        Some(submitted_at),
    )
    .await
}

async fn seed_payroll_run(
    pool: &PgPool,
    org: OrgId,
    source_label: &str,
    status: &str,
    submitter: Option<UserId>,
    submitted_at: Option<OffsetDateTime>,
) -> uuid::Uuid {
    sqlx::query_scalar(
        "INSERT INTO payroll_draft_runs \
         (org_id, period_start, period_end, source_label, status, submitted_by, submitted_at) \
         VALUES ($1, DATE '2026-07-01', DATE '2026-07-31', $2, $3, $4, $5) \
         RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(source_label)
    .bind(status)
    .bind(submitter.map(|user| *user.as_uuid()))
    .bind(submitted_at)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn bind_user_to_new_person(
    pool: &PgPool,
    org: OrgId,
    user: UserId,
    source_key: &str,
    binder: UserId,
) {
    let employee_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO employees \
         (org_id, company, name, source_filename, source_sheet, source_row, source_key) \
         VALUES ($1, 'KNL', $2, 'inbox.xlsx', 'Sheet1', 1, $2) RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(source_key)
    .fetch_one(pool)
    .await
    .unwrap();
    let person_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO persons (org_id) VALUES ($1) RETURNING id")
            .bind(*org.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query("UPDATE users SET employee_id = $1 WHERE id = $2 AND org_id = $3")
        .bind(employee_id)
        .bind(*user.as_uuid())
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO employee_person_bindings \
         (org_id, employee_id, person_id, actor_id, payload_digest) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(*org.as_uuid())
    .bind(employee_id)
    .bind(person_id)
    .bind(*binder.as_uuid())
    .bind([7_u8; 32].as_slice())
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_pending_approval(
    pool: &PgPool,
    org: OrgId,
    requester: UserId,
    kind: &str,
    created_at: OffsetDateTime,
) -> PendingApproval {
    let request_ref = uuid::Uuid::new_v4();
    let id = sqlx::query_scalar(
        "INSERT INTO gov_approval_requests \
         (org_id, request_ref, kind, requested_by, payload_summary, created_at) \
         VALUES ($1, $2, $3, $4, '{}'::jsonb, $5) \
         RETURNING id",
    )
    .bind(*org.as_uuid())
    .bind(request_ref)
    .bind(kind)
    .bind(*requester.as_uuid())
    .bind(created_at)
    .fetch_one(pool)
    .await
    .unwrap();
    PendingApproval { id, request_ref }
}

async fn seed_approval_decision(
    pool: &PgPool,
    org: OrgId,
    request_ref: uuid::Uuid,
    requester: UserId,
    approver: UserId,
) {
    sqlx::query(
        "INSERT INTO gov_approvals \
         (org_id, request_ref, kind, requested_by, approver_id, decision, decided_at) \
         VALUES ($1, $2, 'override', $3, $4, 'approved', now())",
    )
    .bind(*org.as_uuid())
    .bind(request_ref)
    .bind(*requester.as_uuid())
    .bind(*approver.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_dispatch_offer(
    pool: &PgPool,
    org: OrgId,
    branch: BranchId,
    work_order: WorkOrderId,
    creator: UserId,
    target: UserId,
    created_at: OffsetDateTime,
) -> uuid::Uuid {
    let dispatch_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO p1_dispatches \
         (id, work_order_id, branch_id, status, include_region, accept_window_started_at, \
          accept_window_ends_at, created_by, created_at, updated_at, org_id) \
         VALUES ($1, $2, $3, 'BROADCASTING', FALSE, $4, $5, $6, $4, $4, $7)",
    )
    .bind(dispatch_id)
    .bind(*work_order.as_uuid())
    .bind(*branch.as_uuid())
    .bind(created_at)
    .bind(OffsetDateTime::now_utc() + Duration::days(2))
    .bind(*creator.as_uuid())
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO p1_dispatch_targets \
         (dispatch_id, user_id, target_role, push_token_count, fanout_created_at, org_id) \
         VALUES ($1, $2, 'TECHNICIAN', 0, $3, $4)",
    )
    .bind(dispatch_id)
    .bind(*target.as_uuid())
    .bind(created_at)
    .bind(*org.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    dispatch_id
}

async fn get(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
    let request = Request::builder()
        .uri(uri)
        .method("GET")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    JsonResponse { status, json }
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

fn bearer(keys: &Keys, org: OrgId, user_id: UserId, role: &str, branch: BranchId) -> String {
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
            org_id: org,
            roles: vec![role.to_owned()],
            branches: vec![branch],
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

fn app_state(pool: PgPool, public_key_pem: String) -> Result<AppState, console_app::AppError> {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key_pem),
    ])?;
    AppState::new(config, DatabaseDependency::Postgres(pool))
}
