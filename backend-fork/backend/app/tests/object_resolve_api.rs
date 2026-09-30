#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-OBJ object resolve endpoint (GET /api/objects/{kind}/{id}).
//!
//! Proves the resolver framework and both authorization modes end-to-end:
//! org-scoped (person), branch-scoped deny-by-omission (org_unit), and
//! initiator-scoped (approval_run). Cross-scope and absent objects both resolve
//! to `exists: false` (indistinguishable — the deny-by-omission guarantee); a
//! well-formed but unregistered kind is 404. The work_order / equipment /
//! support_ticket resolvers reuse the identical branch_visible + org-RLS read
//! machinery proven here.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings};
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::Value;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn resolves_kinds_and_denies_by_omission(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    let caller = UserId::new();
    let branch_x = seed_branch(&pool, "Region X", "Branch X").await;
    let branch_y = seed_branch(&pool, "Region Y", "Branch Y").await;
    seed_user_in_branch(&pool, caller, "ADMIN", branch_x).await;
    // The caller's branch scope is exactly {branch_x}.
    let token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        caller,
        vec![branch_x],
    );

    // person (org-scoped): a seeded user resolves with its display name.
    let subject = UserId::new();
    seed_user_in_branch(&pool, subject, "MECHANIC", branch_x).await;
    let person = resolve(
        &pool,
        &public_key_pem,
        &token,
        "person",
        &subject.as_uuid().to_string(),
    )
    .await;
    assert_eq!(person.0, StatusCode::OK);
    assert_eq!(person.1["exists"], true, "person resolves: {}", person.1);
    assert!(
        person.1["title"]
            .as_str()
            .unwrap()
            .starts_with("User MECHANIC")
    );
    assert!(
        person.1.get("url_path").is_none(),
        "ObjectHead must not carry a route/URL field — objectRegistry is the \
         sole kind->URL authority: {}",
        person.1
    );

    // person absent: a random id resolves to exists=false (not an error).
    let absent = resolve(
        &pool,
        &public_key_pem,
        &token,
        "person",
        &Uuid::new_v4().to_string(),
    )
    .await;
    assert_eq!(absent.0, StatusCode::OK);
    assert_eq!(absent.1["exists"], false);

    // Auth gate before kind-specific resolution: even an otherwise valid,
    // in-scope person id must be rejected before `resolve_person` runs when the
    // principal has no Login-authorizing role/grant.
    let no_login_caller = UserId::new();
    seed_user_in_branch(&pool, no_login_caller, "MECHANIC", branch_x).await;
    let no_login_token = issue_token_with_roles(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        no_login_caller,
        vec![branch_x],
        Vec::new(),
    );
    let forbidden = resolve(
        &pool,
        &public_key_pem,
        &no_login_token,
        "person",
        &subject.as_uuid().to_string(),
    )
    .await;
    assert_eq!(forbidden.0, StatusCode::FORBIDDEN);

    // person cross-branch: a user who belongs ONLY to branch_y (not the caller's
    // scope) must resolve exists=false — no cross-branch PII/existence leak.
    let branch_b_user = UserId::new();
    seed_user_in_branch(&pool, branch_b_user, "MECHANIC", branch_y).await;
    let cross = resolve(
        &pool,
        &public_key_pem,
        &token,
        "person",
        &branch_b_user.as_uuid().to_string(),
    )
    .await;
    assert_eq!(
        cross.1["exists"], false,
        "branch-B-only user must be denied by omission: {}",
        cross.1
    );

    // person inactive: a deactivated user in the caller's own branch must also
    // resolve exists=false (no deactivation oracle).
    let inactive = UserId::new();
    seed_user_in_branch(&pool, inactive, "MECHANIC", branch_x).await;
    sqlx::query("UPDATE users SET is_active = false WHERE id = $1")
        .bind(*inactive.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let inactive_res = resolve(
        &pool,
        &public_key_pem,
        &token,
        "person",
        &inactive.as_uuid().to_string(),
    )
    .await;
    assert_eq!(
        inactive_res.1["exists"], false,
        "inactive user must be denied by omission: {}",
        inactive_res.1
    );

    // org_unit in scope: branch_x is in the caller's scope -> exists=true.
    let unit_in = resolve(
        &pool,
        &public_key_pem,
        &token,
        "org_unit",
        &branch_x.as_uuid().to_string(),
    )
    .await;
    assert_eq!(
        unit_in.1["exists"], true,
        "in-scope org_unit: {}",
        unit_in.1
    );
    assert_eq!(unit_in.1["title"], "Branch X");

    // org_unit cross-branch: branch_y exists but is NOT in the caller's scope ->
    // exists=false, identical to a missing id (deny-by-omission).
    let unit_out = resolve(
        &pool,
        &public_key_pem,
        &token,
        "org_unit",
        &branch_y.as_uuid().to_string(),
    )
    .await;
    assert_eq!(unit_out.0, StatusCode::OK);
    assert_eq!(
        unit_out.1["exists"], false,
        "cross-branch org_unit must be denied by omission: {}",
        unit_out.1
    );

    // approval_run initiator-scoped: the caller's own run resolves; another
    // initiator's run does not.
    let other = UserId::new();
    seed_user_in_branch(&pool, other, "MECHANIC", branch_x).await;
    let mine = seed_workflow_run(&pool, Some(caller)).await;
    let theirs = seed_workflow_run(&pool, Some(other)).await;
    let run_mine = resolve(
        &pool,
        &public_key_pem,
        &token,
        "approval_run",
        &mine.to_string(),
    )
    .await;
    assert_eq!(
        run_mine.1["exists"], true,
        "own run resolves: {}",
        run_mine.1
    );
    assert_eq!(run_mine.1["status"], "RUNNING");
    let run_theirs = resolve(
        &pool,
        &public_key_pem,
        &token,
        "approval_run",
        &theirs.to_string(),
    )
    .await;
    assert_eq!(
        run_theirs.1["exists"], false,
        "another initiator's run is denied by omission: {}",
        run_theirs.1
    );

    // assignee-role holder: an unclaimed OPEN task routed to a role key the
    // caller holds also makes the run resolvable, matching the waiting-task
    // inbox widening without requiring a claim first.
    let role_routed = seed_workflow_run(&pool, Some(other)).await;
    seed_role_task(&pool, role_routed, "admin").await;
    let run_role_routed = resolve(
        &pool,
        &public_key_pem,
        &token,
        "approval_run",
        &role_routed.to_string(),
    )
    .await;
    assert_eq!(
        run_role_routed.1["exists"], true,
        "an assignee-role holder resolves the unclaimed routed run: {}",
        run_role_routed.1
    );

    // approver-on-the-line: once the caller has CLAIMED a task on theirs, they
    // resolve the run (widened beyond initiator to the inbox's scoping).
    seed_claimed_task(&pool, theirs, caller).await;
    let run_claimed = resolve(
        &pool,
        &public_key_pem,
        &token,
        "approval_run",
        &theirs.to_string(),
    )
    .await;
    assert_eq!(
        run_claimed.1["exists"], true,
        "a claimer on the line resolves the run: {}",
        run_claimed.1
    );

    // Unknown (well-formed but unregistered) kind -> 404.
    let unknown = resolve(
        &pool,
        &public_key_pem,
        &token,
        "banana",
        &Uuid::new_v4().to_string(),
    )
    .await;
    assert_eq!(unknown.0, StatusCode::NOT_FOUND);
}

/// The generic resolver must enforce the same feature guards as the domain
/// read endpoints it aggregates: work_order and equipment GETs require
/// `WorkOrderReadAll`, and account GETs require `UserManage` (identity/rest
/// get_user/list_users/deactivate_user) — all of which a MEMBER (Login-only,
/// matrix index 0) is denied. Without the kind-level gate a MEMBER could read
/// heads its role forbids by harvesting ids from object_links and resolving
/// them here (an account head leaks display_name + active/inactive lifecycle
/// status). The deny fires before any lookup (id-independent), so it
/// introduces no existence oracle; membership-gated kinds (support_ticket)
/// stay at membership parity.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn resolve_enforces_domain_feature_guards(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    let branch = seed_branch(&pool, "Region G", "Branch G").await;
    let member = UserId::new();
    seed_user_in_branch(&pool, member, "MEMBER", branch).await;
    let member_token = issue_token_with_roles(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        member,
        vec![branch],
        vec!["MEMBER".to_owned()],
    );
    let admin = UserId::new();
    seed_user_in_branch(&pool, admin, "ADMIN", branch).await;
    let admin_token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        admin,
        vec![branch],
    );

    // MEMBER holds Login but neither WorkOrderReadAll nor UserManage: all three
    // guarded kinds are 403 regardless of the id — the gate fires before
    // resolution.
    for kind in ["work_order", "equipment", "account"] {
        let denied = resolve(
            &pool,
            &public_key_pem,
            &member_token,
            kind,
            &Uuid::new_v4().to_string(),
        )
        .await;
        assert_eq!(
            denied.0,
            StatusCode::FORBIDDEN,
            "MEMBER must be denied {kind} heads: {}",
            denied.1
        );
    }

    // Membership-gated kinds keep membership parity for the same MEMBER.
    let ticket = resolve(
        &pool,
        &public_key_pem,
        &member_token,
        "support_ticket",
        &Uuid::new_v4().to_string(),
    )
    .await;
    assert_eq!(ticket.0, StatusCode::OK);
    assert_eq!(ticket.1["exists"], false);

    // Control: a WorkOrderReadAll-holding role resolves the guarded kinds
    // (absent id -> exists:false, not an authz error).
    let allowed = resolve(
        &pool,
        &public_key_pem,
        &admin_token,
        "work_order",
        &Uuid::new_v4().to_string(),
    )
    .await;
    assert_eq!(allowed.0, StatusCode::OK);
    assert_eq!(allowed.1["exists"], false);

    // Control: a UserManage-holding role (ADMIN) resolves an in-scope account —
    // proving the gate admits the privileged caller, not just denies the MEMBER.
    let subject = UserId::new();
    seed_user_in_branch(&pool, subject, "MECHANIC", branch).await;
    let acct = resolve(
        &pool,
        &public_key_pem,
        &admin_token,
        "account",
        &subject.as_uuid().to_string(),
    )
    .await;
    assert_eq!(acct.0, StatusCode::OK);
    assert_eq!(
        acct.1["exists"], true,
        "ADMIN resolves in-scope account: {}",
        acct.1
    );
    assert_eq!(acct.1["status"], "active");
}

/// Identity object kinds (Identity Console UI-M13 / charter G-b): account
/// (person's branch-membership semantics, but the lifecycle object — shows
/// deactivated in-scope accounts + status), and the self-owned passkey/consent
/// kinds. Negative coverage is mandatory (this resolver's history includes a
/// cross-branch leak): cross-branch account, non-self passkey, and non-self /
/// unaccepted consent must all be denied by omission.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn resolves_identity_kinds_and_denies_by_omission(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    let caller = UserId::new();
    let branch_x = seed_branch(&pool, "Region X", "Branch X").await;
    let branch_y = seed_branch(&pool, "Region Y", "Branch Y").await;
    // ADMIN -> branch-bounded scope {branch_x} (not All), so cross-branch denial
    // is proven at the application level, not only via RLS.
    seed_user_in_branch(&pool, caller, "ADMIN", branch_x).await;
    let token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        caller,
        vec![branch_x],
    );

    // account (in-scope, active): resolves with status=active.
    let subject = UserId::new();
    seed_user_in_branch(&pool, subject, "MECHANIC", branch_x).await;
    let account = resolve(
        &pool,
        &public_key_pem,
        &token,
        "account",
        &subject.as_uuid().to_string(),
    )
    .await;
    assert_eq!(account.0, StatusCode::OK);
    assert_eq!(account.1["exists"], true, "in-scope account: {}", account.1);
    assert_eq!(account.1["status"], "active");

    // account (in-scope, DEACTIVATED): unlike `person` (which hides inactive),
    // the account lifecycle object still resolves, surfacing status=inactive —
    // this is what the S2 activate/deactivate transition renders.
    let deactivated = UserId::new();
    seed_user_in_branch(&pool, deactivated, "MECHANIC", branch_x).await;
    sqlx::query("UPDATE users SET is_active = false WHERE id = $1")
        .bind(*deactivated.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let acct_inactive = resolve(
        &pool,
        &public_key_pem,
        &token,
        "account",
        &deactivated.as_uuid().to_string(),
    )
    .await;
    assert_eq!(
        acct_inactive.1["exists"], true,
        "deactivated in-scope account still resolves: {}",
        acct_inactive.1
    );
    assert_eq!(acct_inactive.1["status"], "inactive");
    // The person kind for the SAME user stays hidden (no deactivation oracle on
    // the person surface).
    let person_inactive = resolve(
        &pool,
        &public_key_pem,
        &token,
        "person",
        &deactivated.as_uuid().to_string(),
    )
    .await;
    assert_eq!(person_inactive.1["exists"], false);

    // account cross-branch: a user only in branch_y must be denied by omission.
    let cross_branch = UserId::new();
    seed_user_in_branch(&pool, cross_branch, "MECHANIC", branch_y).await;
    let acct_cross = resolve(
        &pool,
        &public_key_pem,
        &token,
        "account",
        &cross_branch.as_uuid().to_string(),
    )
    .await;
    assert_eq!(
        acct_cross.1["exists"], false,
        "cross-branch account must be denied by omission: {}",
        acct_cross.1
    );

    // SUPER_ADMIN resolves to BranchScope::All, so the account lifecycle
    // resolver must cover both active and deactivated accounts without relying
    // on branch membership narrowing.
    let all_scope_caller = UserId::new();
    seed_user_in_branch(&pool, all_scope_caller, "SUPER_ADMIN", branch_y).await;
    let all_scope_token = issue_token_with_roles(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        all_scope_caller,
        vec![],
        vec!["SUPER_ADMIN".to_owned()],
    );
    let acct_all_active = resolve(
        &pool,
        &public_key_pem,
        &all_scope_token,
        "account",
        &subject.as_uuid().to_string(),
    )
    .await;
    assert_eq!(
        acct_all_active.1["exists"], true,
        "All-scope account resolves active users: {}",
        acct_all_active.1
    );
    assert_eq!(acct_all_active.1["status"], "active");
    let acct_all_inactive = resolve(
        &pool,
        &public_key_pem,
        &all_scope_token,
        "account",
        &deactivated.as_uuid().to_string(),
    )
    .await;
    assert_eq!(
        acct_all_inactive.1["exists"], true,
        "All-scope account resolves deactivated users: {}",
        acct_all_inactive.1
    );
    assert_eq!(acct_all_inactive.1["status"], "inactive");

    // passkey (self-owned): the caller's own passkey resolves.
    let my_passkey = seed_passkey(&pool, caller).await;
    let pk_mine = resolve(
        &pool,
        &public_key_pem,
        &token,
        "passkey",
        &my_passkey.to_string(),
    )
    .await;
    assert_eq!(pk_mine.0, StatusCode::OK);
    assert_eq!(pk_mine.1["exists"], true, "own passkey: {}", pk_mine.1);

    // passkey NON-SELF: a passkey owned by another user is denied by omission —
    // no cross-user credential-enumeration oracle.
    let their_passkey = seed_passkey(&pool, subject).await;
    let pk_theirs = resolve(
        &pool,
        &public_key_pem,
        &token,
        "passkey",
        &their_passkey.to_string(),
    )
    .await;
    assert_eq!(
        pk_theirs.1["exists"], false,
        "another user's passkey must be denied by omission: {}",
        pk_theirs.1
    );

    // consent (self-owned, versioned): id IS the policy version string. The
    // caller's accepted version resolves; an unaccepted version does not.
    let version = "kr-pipa-v1-2026-06-25";
    seed_consent(&pool, caller, version).await;
    let consent_mine = resolve(&pool, &public_key_pem, &token, "consent", version).await;
    assert_eq!(consent_mine.0, StatusCode::OK);
    assert_eq!(
        consent_mine.1["exists"], true,
        "own accepted consent: {}",
        consent_mine.1
    );
    assert_eq!(consent_mine.1["status"], "accepted");
    assert_eq!(consent_mine.1["title"], version);

    let consent_absent = resolve(
        &pool,
        &public_key_pem,
        &token,
        "consent",
        "kr-pipa-v9-2099-01-01",
    )
    .await;
    assert_eq!(
        consent_absent.1["exists"], false,
        "unaccepted consent version is denied by omission: {}",
        consent_absent.1
    );

    // consent NON-SELF: a version accepted only by ANOTHER user is not visible
    // to the caller (actor-scoped).
    let their_version = "kr-pipa-v2-2026-06-25";
    seed_consent(&pool, subject, their_version).await;
    let consent_theirs = resolve(&pool, &public_key_pem, &token, "consent", their_version).await;
    assert_eq!(
        consent_theirs.1["exists"], false,
        "another user's consent must be denied by omission: {}",
        consent_theirs.1
    );
}

/// B1: a DIRECT top-level resolve of a `person` card records a `person.view`
/// audit for a NON-SELF subject that actually resolves — mirroring
/// GET /api/messenger/members/{id}'s "who looked up whom" evidence. A self-view
/// records nothing; an out-of-scope/absent subject (resolves to exists:false)
/// records nothing; and a person reached incidentally by an object_graph WALK
/// records nothing (traversing a graph is not viewing a card).
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn person_direct_resolve_audits_non_self_only(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    let caller = UserId::new();
    let branch_x = seed_branch(&pool, "Region X", "Branch X").await;
    let branch_y = seed_branch(&pool, "Region Y", "Branch Y").await;
    seed_user_in_branch(&pool, caller, "ADMIN", branch_x).await;
    let token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        caller,
        vec![branch_x],
    );

    // Non-self, resolves -> exactly one person.view for the subject.
    let subject = UserId::new();
    seed_user_in_branch(&pool, subject, "MECHANIC", branch_x).await;
    let seen = resolve(
        &pool,
        &public_key_pem,
        &token,
        "person",
        &subject.as_uuid().to_string(),
    )
    .await;
    assert_eq!(seen.1["exists"], true);
    assert_eq!(
        person_view_count(&pool, subject).await,
        1,
        "non-self direct resolve records one person.view"
    );

    // Self-view -> no audit.
    let _self_view = resolve(
        &pool,
        &public_key_pem,
        &token,
        "person",
        &caller.as_uuid().to_string(),
    )
    .await;
    assert_eq!(
        person_view_count(&pool, caller).await,
        0,
        "self-view records no person.view"
    );

    // Out-of-scope subject -> exists:false and no audit (no row, no view).
    let out = UserId::new();
    seed_user_in_branch(&pool, out, "MECHANIC", branch_y).await;
    let out_res = resolve(
        &pool,
        &public_key_pem,
        &token,
        "person",
        &out.as_uuid().to_string(),
    )
    .await;
    assert_eq!(out_res.1["exists"], false);
    assert_eq!(
        person_view_count(&pool, out).await,
        0,
        "out-of-scope resolve records no person.view"
    );

    // A person discovered by a graph WALK is not a card view -> no audit.
    let walked = UserId::new();
    seed_user_in_branch(&pool, walked, "MECHANIC", branch_x).await;
    seed_object_link(
        &pool,
        "org_unit",
        &branch_x.as_uuid().to_string(),
        "person",
        &walked,
    )
    .await;
    let graph = resolve_graph(
        &pool,
        &public_key_pem,
        &token,
        "org_unit",
        &branch_x.as_uuid().to_string(),
    )
    .await;
    assert_eq!(graph.0, StatusCode::OK, "graph body: {}", graph.1);
    // The walked person is present as a node (proves the walk reached it)...
    assert!(
        graph.1["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["kind"] == "person" && n["id"] == walked.as_uuid().to_string()),
        "graph walk reaches the linked person: {}",
        graph.1
    );
    // ...yet no person.view was recorded for it.
    assert_eq!(
        person_view_count(&pool, walked).await,
        0,
        "a person reached by a graph walk records no person.view"
    );

    // Graph-ROOTING a non-self person IS a direct lookup -> audits once, exactly
    // like resolve_object (otherwise B1 is trivially evadable via the graph).
    let graph_subject = UserId::new();
    seed_user_in_branch(&pool, graph_subject, "MECHANIC", branch_x).await;
    let rooted = resolve_graph(
        &pool,
        &public_key_pem,
        &token,
        "person",
        &graph_subject.as_uuid().to_string(),
    )
    .await;
    assert_eq!(rooted.0, StatusCode::OK, "graph body: {}", rooted.1);
    assert_eq!(
        person_view_count(&pool, graph_subject).await,
        1,
        "graph-root of a non-self person audits once, like a direct resolve"
    );

    // Graph-rooting SELF -> no audit (mirrors self direct-resolve).
    let _self_root = resolve_graph(
        &pool,
        &public_key_pem,
        &token,
        "person",
        &caller.as_uuid().to_string(),
    )
    .await;
    assert_eq!(
        person_view_count(&pool, caller).await,
        0,
        "graph-root of self records no person.view"
    );
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

async fn person_view_count(pool: &PgPool, target: UserId) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'person.view' AND target_id = $1",
    )
    .bind(target.as_uuid().to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_object_link(
    pool: &PgPool,
    src_kind: &str,
    src_id: &str,
    dst_kind: &str,
    dst: &UserId,
) {
    sqlx::query(
        "INSERT INTO object_links (org_id, src_kind, src_id, dst_kind, dst_id, link_type) \
         VALUES ($1, $2, $3, $4, $5, 'relates_to')",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(src_kind)
    .bind(src_id)
    .bind(dst_kind)
    .bind(dst.as_uuid().to_string())
    .execute(pool)
    .await
    .unwrap();
}

async fn resolve_graph(
    pool: &PgPool,
    public_key_pem: &str,
    token: &str,
    kind: &str,
    id: &str,
) -> (StatusCode, Value) {
    let service = build_router(app_state(pool.clone(), public_key_pem.to_owned()).unwrap());
    let response = service
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/objects/{kind}/{id}/graph"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, json)
}

async fn seed_passkey(pool: &PgPool, user_id: UserId) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO auth_webauthn_credentials (user_id, credential_id, passkey_json, org_id) \
         VALUES ($1, $2, '{}'::jsonb, $3) RETURNING id",
    )
    .bind(*user_id.as_uuid())
    .bind(format!("cred-{}", Uuid::new_v4()))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_consent(pool: &PgPool, actor: UserId, version: &str) {
    sqlx::query(
        "INSERT INTO audit_events \
         (actor, action, target_type, target_id, trace_id, span_id, occurred_at, org_id) \
         VALUES ($1, 'privacy.required_accept', 'privacy_terms', $2, $3, $4, now(), $5)",
    )
    .bind(*actor.as_uuid())
    .bind(version)
    .bind("0".repeat(32))
    .bind("0".repeat(16))
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn resolve(
    pool: &PgPool,
    public_key_pem: &str,
    token: &str,
    kind: &str,
    id: &str,
) -> (StatusCode, Value) {
    let service = build_router(app_state(pool.clone(), public_key_pem.to_owned()).unwrap());
    let response = service
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/objects/{kind}/{id}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, json)
}

async fn seed_branch(pool: &PgPool, region: &str, branch: &str) -> BranchId {
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(region)
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(branch)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user_in_branch(pool: &PgPool, user_id: UserId, role: &str, branch: BranchId) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("User {role} {}", Uuid::new_v4()))
        .bind(Vec::from([role]))
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_workflow_run(pool: &PgPool, initiated_by: Option<UserId>) -> Uuid {
    // workflow_runs.(definition_id, org_id) FKs to workflow_definitions, so seed
    // a definition first (unique workflow_key per call).
    let definition_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO workflow_definitions (id, org_id, workflow_key, display_name, object_type)
        VALUES ($1, $2, $3, 'Test Approval', 'work_order')
        "#,
    )
    .bind(definition_id)
    .bind(*OrgId::knl().as_uuid())
    .bind(format!("test.wf_{}", definition_id.simple()))
    .execute(pool)
    .await
    .unwrap();
    // The run also FKs (definition_id, definition_version) -> version rows.
    sqlx::query(
        r#"
        INSERT INTO workflow_definition_versions (id, org_id, definition_id, version, status)
        VALUES ($1, $2, $3, 1, 'PUBLISHED')
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(*OrgId::knl().as_uuid())
    .bind(definition_id)
    .execute(pool)
    .await
    .unwrap();

    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO workflow_runs (
            id, org_id, definition_id, definition_version, status, trigger_type,
            idempotency_key, correlation_id, initiated_by
        )
        VALUES ($1, $2, $3, 1, 'RUNNING', 'MANUAL', $4, $5, $6)
        "#,
    )
    .bind(id)
    .bind(*OrgId::knl().as_uuid())
    .bind(definition_id)
    .bind(format!("idem-{}", Uuid::new_v4()))
    .bind(format!("corr-{}", Uuid::new_v4()))
    .bind(initiated_by.map(|u| *u.as_uuid()))
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn seed_claimed_task(pool: &PgPool, run_id: Uuid, claimed_by: UserId) {
    sqlx::query(
        r#"
        INSERT INTO workflow_waiting_tasks (
            id, org_id, run_id, waiting_key, title, status, required_policy, claimed_by
        )
        VALUES ($1, $2, $3, 'approval_step', 'Approve', 'CLAIMED', 'approval_finalize', $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(*OrgId::knl().as_uuid())
    .bind(run_id)
    .bind(*claimed_by.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_role_task(pool: &PgPool, run_id: Uuid, assignee_role_key: &str) {
    sqlx::query(
        r#"
        INSERT INTO workflow_waiting_tasks (
            id, org_id, run_id, waiting_key, title, assignee_role_key, required_policy
        )
        VALUES ($1, $2, $3, $4, 'Approve as role', $5, 'approval_finalize')
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(*OrgId::knl().as_uuid())
    .bind(run_id)
    .bind(format!("approval_step_{}", Uuid::new_v4().simple()))
    .bind(assignee_role_key)
    .execute(pool)
    .await
    .unwrap();
}

fn issue_token(
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    branches: Vec<BranchId>,
) -> String {
    issue_token_with_roles(
        private_key_pem,
        public_key_pem,
        user_id,
        branches,
        vec!["ADMIN".to_owned()],
    )
}

fn issue_token_with_roles(
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    branches: Vec<BranchId>,
    roles: Vec<String>,
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
