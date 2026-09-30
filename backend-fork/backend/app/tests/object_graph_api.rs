#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-OBJ object graph traversal (GET /api/objects/{kind}/{id}/graph).
//!
//! Runs as the genuine non-owner runtime role `console_rt` (NOBYPASSRLS, FORCE
//! RLS) — NOT the default `#[sqlx::test]` BYPASSRLS superuser pool
//! `object_resolve_api.rs` uses. That distinction matters here specifically:
//! the resolve leak caught in the slice-1 review makes this endpoint the #1
//! scrutiny point, so this test exercises real RLS enforcement rather than an
//! owner connection that would green-light a broken filter.
//!
//! Proves: (a) the walk finds nodes across multiple hops and returns the
//! induced edge set between them; (b) EVERY node — at any depth — goes
//! through the identical per-kind visibility guard `resolve_object` uses, and
//! deny-by-omission governs DISCOVERY itself: an out-of-scope/unresolvable
//! node is OMITTED (never returned as a stub), any edge touching it is
//! omitted too, and the walk never expands through it; (c) a link planted
//! under a DIFFERENT org (even reusing the same kind+id text as the root)
//! never surfaces in the walk — RLS on `object_links`, not app-level
//! filtering; (d) the walk is a bounded Rust-side BFS, not a recursive SQL
//! CTE: a dense, cyclic subgraph terminates promptly and the node cap is
//! enforced (`truncated: true`) rather than materializing the whole graph.

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
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const OTHER_ORG: Uuid = Uuid::from_u128(0x0bad_0bad_0bad_0bad_0bad_0bad_0bad_0bad);

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn walks_hops_omits_out_of_scope_nodes_and_ignores_cross_org_links(owner_pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    let branch_x = seed_branch(&owner_pool, "Region X", "Branch X").await;
    let branch_y = seed_branch(&owner_pool, "Region Y", "Branch Y").await;
    seed_org(&owner_pool, OTHER_ORG, "Other").await;

    let caller = UserId::new();
    seed_user_in_branch(&owner_pool, caller, "ADMIN", branch_x).await;
    let token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        caller,
        vec![branch_x],
    );

    // P1 shares the caller's branch (in scope); P2 is branch_y-only (out of
    // the caller's scope, exactly like object_resolve_api.rs's cross-branch
    // deny-by-omission case).
    let p1 = UserId::new();
    seed_user_in_branch(&owner_pool, p1, "MECHANIC", branch_x).await;
    let p2 = UserId::new();
    seed_user_in_branch(&owner_pool, p2, "MECHANIC", branch_y).await;

    // 1 hop from the root org_unit: -> P1 (in scope) and -> P2 (out of scope).
    let branch_x_id = branch_x.as_uuid().to_string();
    let branch_y_id = branch_y.as_uuid().to_string();
    let p1_id = p1.as_uuid().to_string();
    let p2_id = p2.as_uuid().to_string();
    seed_link(
        &owner_pool,
        OrgId::knl(),
        "org_unit",
        &branch_x_id,
        "person",
        &p1_id,
    )
    .await;
    seed_link(
        &owner_pool,
        OrgId::knl(),
        "org_unit",
        &branch_x_id,
        "person",
        &p2_id,
    )
    .await;
    // 2nd hop: P1 -> branch_y (also out of the caller's scope).
    seed_link(
        &owner_pool,
        OrgId::knl(),
        "person",
        &p1_id,
        "org_unit",
        &branch_y_id,
    )
    .await;
    // A link planted under a DIFFERENT org, reusing the root's exact (kind,
    // id) text — must never surface in org KNL's walk (RLS, not app filtering).
    seed_link(
        &owner_pool,
        OrgId::from_uuid(OTHER_ORG),
        "org_unit",
        &branch_x_id,
        "equipment",
        "leaked-eq",
    )
    .await;

    let rt_pool = runtime_role_pool(&owner_pool).await;

    // depth=1: root + P1 only. P2 is out-of-scope -> OMITTED entirely (not a
    // stub), so its edge from root is omitted too: 1 node pair, 1 edge.
    let one_hop = graph(
        &rt_pool,
        &public_key_pem,
        &token,
        "org_unit",
        &branch_x_id,
        Some(1),
    )
    .await;
    assert_eq!(one_hop.0, StatusCode::OK, "body: {}", one_hop.1);
    assert_eq!(one_hop.1["truncated"], false);
    let nodes = one_hop.1["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2, "nodes: {nodes:?}");
    let root_node = find_node(nodes, "org_unit", &branch_x_id);
    assert_eq!(root_node["exists"], true);
    assert_eq!(root_node["title"], "Branch X");
    let p1_node = find_node(nodes, "person", &p1_id);
    assert_eq!(
        p1_node["exists"], true,
        "in-scope P1 must resolve: {p1_node}"
    );
    assert!(
        find_node_opt(nodes, "person", &p2_id).is_none(),
        "out-of-scope P2 must be OMITTED entirely (deny-by-omission governs \
         discovery, not a redacted stub): {nodes:?}"
    );
    let edges = one_hop.1["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1, "edges: {edges:?}");
    assert!(
        edges
            .iter()
            .all(|e| e["dst_id"] != p2_id.as_str() && e["src_id"] != p2_id.as_str()),
        "the edge to the omitted P2 must be omitted too: {edges:?}"
    );
    assert!(
        edges
            .iter()
            .all(|e| e["src_kind"] != "equipment" && e["dst_kind"] != "equipment"),
        "the other org's link must never appear: {edges:?}"
    );

    // depth=2: the only expansion path from P1 leads to branch_y, which is
    // ALSO out-of-scope -> omitted, and the walk must not expand through it.
    // Result is identical to depth=1: deny-by-omission blocks the walk, not
    // just the display of the unreachable node.
    let two_hop = graph(
        &rt_pool,
        &public_key_pem,
        &token,
        "org_unit",
        &branch_x_id,
        Some(2),
    )
    .await;
    assert_eq!(two_hop.0, StatusCode::OK, "body: {}", two_hop.1);
    let nodes2 = two_hop.1["nodes"].as_array().unwrap();
    assert_eq!(nodes2.len(), 2, "nodes: {nodes2:?}");
    assert!(
        find_node_opt(nodes2, "org_unit", &branch_y_id).is_none(),
        "cross-branch org_unit reached only via an omitted path must itself \
         be omitted, not a stub: {nodes2:?}"
    );
    let edges2 = two_hop.1["edges"].as_array().unwrap();
    assert_eq!(
        edges2.len(),
        1,
        "the edge to branch_y must be omitted (unreachable through an \
         omitted node): {edges2:?}"
    );
}

/// A dense, cyclic cluster (K5: 5 nodes, every pair linked) plus a wide
/// star exceeding `GRAPH_MAX_NODES` — proves the walk is a bounded,
/// cycle-safe Rust-side BFS (never a recursive SQL CTE that would
/// materialize ~degree^depth before truncating) and that the node cap is
/// enforced mid-walk with `truncated: true`.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn terminates_on_dense_cyclic_graph_and_respects_node_cap(owner_pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    let branch_x = seed_branch(&owner_pool, "Region K5", "Branch K5").await;
    let caller = UserId::new();
    seed_user_in_branch(&owner_pool, caller, "ADMIN", branch_x).await;
    let token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        caller,
        vec![branch_x],
    );

    // --- Scenario 1: K5 cluster with cycles, well under the node cap. ---
    let root = branch_x.as_uuid().to_string();
    let hubs: Vec<UserId> = (0..5).map(|_| UserId::new()).collect();
    for hub in &hubs {
        seed_user_in_branch(&owner_pool, *hub, "MECHANIC", branch_x).await;
        seed_link(
            &owner_pool,
            OrgId::knl(),
            "org_unit",
            &root,
            "person",
            &hub.as_uuid().to_string(),
        )
        .await;
    }
    // Every pair of hubs linked -> a 5-clique with multiple cycles.
    for i in 0..hubs.len() {
        for j in (i + 1)..hubs.len() {
            seed_link(
                &owner_pool,
                OrgId::knl(),
                "person",
                &hubs[i].as_uuid().to_string(),
                "person",
                &hubs[j].as_uuid().to_string(),
            )
            .await;
        }
    }

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let dense = graph(
        &rt_pool,
        &public_key_pem,
        &token,
        "org_unit",
        &root,
        Some(5),
    )
    .await;
    assert_eq!(dense.0, StatusCode::OK, "body: {}", dense.1);
    let dense_nodes = dense.1["nodes"].as_array().unwrap();
    // root + 5 hubs, each appearing exactly once despite the cycles.
    assert_eq!(dense_nodes.len(), 6, "nodes: {dense_nodes:?}");
    let mut dense_keys: Vec<(String, String)> = dense_nodes
        .iter()
        .map(|n| {
            (
                n["kind"].as_str().unwrap().to_owned(),
                n["id"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    dense_keys.sort();
    dense_keys.dedup();
    assert_eq!(
        dense_keys.len(),
        dense_nodes.len(),
        "no duplicate nodes despite the cycle: {dense_nodes:?}"
    );
    // 5 root-hub edges + C(5,2)=10 hub-hub edges = 15.
    let dense_edges = dense.1["edges"].as_array().unwrap();
    assert_eq!(dense_edges.len(), 15, "edges: {dense_edges:?}");
    assert_eq!(dense.1["truncated"], false);

    // --- Scenario 2: widen the SAME root's star past GRAPH_MAX_NODES (200). ---
    for _ in 0..210 {
        let leaf = UserId::new();
        seed_user_in_branch(&owner_pool, leaf, "MECHANIC", branch_x).await;
        seed_link(
            &owner_pool,
            OrgId::knl(),
            "org_unit",
            &root,
            "person",
            &leaf.as_uuid().to_string(),
        )
        .await;
    }

    let wide = graph(
        &rt_pool,
        &public_key_pem,
        &token,
        "org_unit",
        &root,
        Some(1),
    )
    .await;
    assert_eq!(wide.0, StatusCode::OK, "body: {}", wide.1);
    let wide_nodes = wide.1["nodes"].as_array().unwrap();
    // 210 leaves + root + the 5 K5 hubs (also linked to this same root) all
    // resolve, comfortably exceeding the cap -> capped at exactly 200.
    assert_eq!(
        wide_nodes.len(),
        200,
        "node cap must be enforced: {}",
        wide_nodes.len()
    );
    assert_eq!(
        wide.1["truncated"], true,
        "truncated must be set once the cap is hit"
    );
}

/// Composition test for the WorkOrderReadAll feature gate (#222) reconciled
/// into the BFS walk's `resolve_head`: a MEMBER (Login-only, no
/// WorkOrderReadAll) walking a graph that happens to touch a work_order node
/// must get 200 with that node OMITTED — never a 403 for the whole graph
/// request. A 403 here would be the safe-but-wrong failure mode; a VISIBLE
/// work_order node would be the real leak (the one #222 fixed for
/// resolve_object but which the graph's independent node-discovery path could
/// have reopened without this composition).
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn graph_omits_work_order_node_for_member_without_feature_grant(owner_pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    let branch_x = seed_branch(&owner_pool, "Region F", "Branch F").await;
    let member = UserId::new();
    seed_user_in_branch(&owner_pool, member, "MEMBER", branch_x).await;
    let member_token = issue_token_with_roles(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        member,
        vec![branch_x],
        vec!["MEMBER".to_owned()],
    );

    let root = branch_x.as_uuid().to_string();
    let work_order_id = seed_work_order(&owner_pool, branch_x, member).await;
    seed_link(
        &owner_pool,
        OrgId::knl(),
        "org_unit",
        &root,
        "work_order",
        &work_order_id.to_string(),
    )
    .await;

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let result = graph(
        &rt_pool,
        &public_key_pem,
        &member_token,
        "org_unit",
        &root,
        Some(1),
    )
    .await;
    assert_eq!(
        result.0,
        StatusCode::OK,
        "a graph containing a guarded node must still 200, not 403: {}",
        result.1
    );
    let nodes = result.1["nodes"].as_array().unwrap();
    assert!(
        find_node_opt(nodes, "work_order", &work_order_id.to_string()).is_none(),
        "work_order node must be OMITTED for a MEMBER without WorkOrderReadAll: {nodes:?}"
    );
    let edges = result.1["edges"].as_array().unwrap();
    assert!(
        edges
            .iter()
            .all(|e| e["src_kind"] != "work_order" && e["dst_kind"] != "work_order"),
        "the edge to the omitted work_order must be omitted too: {edges:?}"
    );
}

/// Composition test for the account/UserManage feature gate reconciled into the
/// BFS walk's `resolve_head`: a MEMBER (Login-only, no UserManage) walking a
/// graph that touches an account node must get 200 with that node OMITTED,
/// while an ADMIN's same walk includes the account lifecycle head. This keeps
/// the graph's quiet omission path in lockstep with `resolve_object`'s direct
/// 403 for account heads without creating an existence oracle mid-walk.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn graph_omits_account_node_for_member_without_user_manage(owner_pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    let branch_x = seed_branch(&owner_pool, "Region U", "Branch U").await;
    let member = UserId::new();
    seed_user_in_branch(&owner_pool, member, "MEMBER", branch_x).await;
    let member_token = issue_token_with_roles(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        member,
        vec![branch_x],
        vec!["MEMBER".to_owned()],
    );

    let admin = UserId::new();
    seed_user_in_branch(&owner_pool, admin, "ADMIN", branch_x).await;
    let admin_token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        admin,
        vec![branch_x],
    );

    let subject = UserId::new();
    seed_user_in_branch(&owner_pool, subject, "MECHANIC", branch_x).await;
    let root = branch_x.as_uuid().to_string();
    let account_id = subject.as_uuid().to_string();
    seed_link(
        &owner_pool,
        OrgId::knl(),
        "org_unit",
        &root,
        "account",
        &account_id,
    )
    .await;

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let member_result = graph(
        &rt_pool,
        &public_key_pem,
        &member_token,
        "org_unit",
        &root,
        Some(1),
    )
    .await;
    assert_eq!(
        member_result.0,
        StatusCode::OK,
        "a graph containing a UserManage-gated node must still 200, not 403: {}",
        member_result.1
    );
    let member_nodes = member_result.1["nodes"].as_array().unwrap();
    assert!(
        find_node_opt(member_nodes, "account", &account_id).is_none(),
        "account node must be OMITTED for a MEMBER without UserManage: {member_nodes:?}"
    );
    let member_edges = member_result.1["edges"].as_array().unwrap();
    assert!(
        member_edges
            .iter()
            .all(|e| e["src_kind"] != "account" && e["dst_kind"] != "account"),
        "the edge to the omitted account must be omitted too: {member_edges:?}"
    );

    let admin_result = graph(
        &rt_pool,
        &public_key_pem,
        &admin_token,
        "org_unit",
        &root,
        Some(1),
    )
    .await;
    assert_eq!(
        admin_result.0,
        StatusCode::OK,
        "ADMIN graph should resolve the account node: {}",
        admin_result.1
    );
    let admin_nodes = admin_result.1["nodes"].as_array().unwrap();
    let account_node = find_node(admin_nodes, "account", &account_id);
    assert_eq!(
        account_node["exists"], true,
        "ADMIN resolves in-scope account node: {account_node}"
    );
    assert_eq!(account_node["status"], "active");
    let admin_edges = admin_result.1["edges"].as_array().unwrap();
    assert!(
        admin_edges
            .iter()
            .any(|e| e["src_kind"] == "org_unit" && e["dst_kind"] == "account"),
        "ADMIN graph should include the edge to the visible account: {admin_edges:?}"
    );
}

/// The per-level `GRAPH_MAX_LINKS_PER_LEVEL` clip must mark the response
/// `truncated` — otherwise a level with more links than the cap silently drops
/// edges (including cross-edges between two nodes both in the result) while
/// truncated stays false. Seed > the cap outgoing links from the root and
/// assert truncated even though the node cap is never hit (dst nodes are
/// non-resolvable, so no extra nodes are added).
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn link_limit_clip_marks_truncated(owner_pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    let branch_x = seed_branch(&owner_pool, "Region L", "Branch L").await;
    let caller = UserId::new();
    seed_user_in_branch(&owner_pool, caller, "ADMIN", branch_x).await;
    let token = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        caller,
        vec![branch_x],
    );

    let root = branch_x.as_uuid().to_string();
    // ponytail: 1001 == GRAPH_MAX_LINKS_PER_LEVEL (1000, private in objects.rs) + 1.
    // Bump if that const rises. dst kind `document` is non-resolvable, so these
    // never become nodes — truncated must come purely from the link-limit clip.
    sqlx::query(
        "INSERT INTO object_links (org_id, src_kind, src_id, dst_kind, dst_id, link_type) \
         SELECT $1, 'org_unit', $2, 'document', 'doc-' || g, 'relates_to' \
         FROM generate_series(1, 1001) g",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(&root)
    .execute(&owner_pool)
    .await
    .unwrap();

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let res = graph(
        &rt_pool,
        &public_key_pem,
        &token,
        "org_unit",
        &root,
        Some(1),
    )
    .await;
    assert_eq!(res.0, StatusCode::OK, "body: {}", res.1);
    assert_eq!(
        res.1["truncated"], true,
        "a clipped link level must mark truncated: {}",
        res.1
    );
    // Only the root resolves (document dsts are omitted), so the node cap is
    // never the cause — this isolates the link-limit clip as the trigger.
    assert_eq!(
        res.1["nodes"].as_array().unwrap().len(),
        1,
        "only the root node resolves: {}",
        res.1["nodes"]
    );
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

async fn graph(
    pool: &PgPool,
    public_key_pem: &str,
    token: &str,
    kind: &str,
    id: &str,
    depth: Option<i64>,
) -> (StatusCode, Value) {
    let service = build_router(app_state(pool.clone(), public_key_pem.to_owned()).unwrap());
    let uri = match depth {
        Some(d) => format!("/api/objects/{kind}/{id}/graph?depth={d}"),
        None => format!("/api/objects/{kind}/{id}/graph"),
    };
    let response = service
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
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

fn find_node<'a>(nodes: &'a [Value], kind: &str, id: &str) -> &'a Value {
    nodes
        .iter()
        .find(|n| n["kind"] == kind && n["id"] == id)
        .unwrap_or_else(|| panic!("node {kind}/{id} not found in {nodes:?}"))
}

fn find_node_opt<'a>(nodes: &'a [Value], kind: &str, id: &str) -> Option<&'a Value> {
    nodes.iter().find(|n| n["kind"] == kind && n["id"] == id)
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

async fn seed_org(pool: &PgPool, org: Uuid, tag: &str) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(pool)
    .await
    .unwrap();
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

/// Seeds a minimal work_order (plus its required customer/site/equipment
/// FKs) directly via the owner/BYPASSRLS pool, mirroring the working pattern
/// in `platform/db/tests/rls_isolation.rs`'s `seed_org`. Returns the
/// work_order id.
async fn seed_work_order(pool: &PgPool, branch: BranchId, requested_by: UserId) -> Uuid {
    let branch_uuid = *branch.as_uuid();
    let org = *OrgId::knl().as_uuid();

    let customer = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO registry_customers (id, branch_id, name, org_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(customer)
    .bind(branch_uuid)
    .bind(format!("Customer {customer}"))
    .bind(org)
    .execute(pool)
    .await
    .unwrap();

    let site = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO registry_sites (id, branch_id, customer_id, name, org_id) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(site)
    .bind(branch_uuid)
    .bind(customer)
    .bind(format!("Site {site}"))
    .bind(org)
    .execute(pool)
    .await
    .unwrap();

    let equipment = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO registry_equipment \
            (id, branch_id, customer_id, site_id, equipment_no, manufacturer_code, \
             kind_code, power_code, status, specification, ton_text, source_sheet, \
             source_row, org_id) \
         VALUES ($1, $2, $3, $4, 'ABC01-0001', 'M', 'K', 'P', '임대', 'spec', '1t', 's', 1, $5)",
    )
    .bind(equipment)
    .bind(branch_uuid)
    .bind(customer)
    .bind(site)
    .bind(org)
    .execute(pool)
    .await
    .unwrap();

    let work_order = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO work_orders \
            (id, request_no, branch_id, equipment_id, customer_id, site_id, \
             requested_by, status, symptom, org_id) \
         VALUES ($1, '20260709-001', $2, $3, $4, $5, $6, 'RECEIVED', 'sym', $7)",
    )
    .bind(work_order)
    .bind(branch_uuid)
    .bind(equipment)
    .bind(customer)
    .bind(site)
    .bind(*requested_by.as_uuid())
    .bind(org)
    .execute(pool)
    .await
    .unwrap();
    work_order
}

/// Plants a link directly via the owner/BYPASSRLS pool, exactly like
/// `object_links_api.rs`'s cross-org isolation test — the fixture, not the
/// thing under test (`object_graph`'s own RLS-scoped read, exercised as
/// `console_rt` below, is what's actually asserted).
async fn seed_link(
    pool: &PgPool,
    org: OrgId,
    src_kind: &str,
    src_id: &str,
    dst_kind: &str,
    dst_id: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO object_links (id, org_id, src_kind, src_id, dst_kind, dst_id, link_type)
        VALUES ($1, $2, $3, $4, $5, $6, 'relates_to')
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(*org.as_uuid())
    .bind(src_kind)
    .bind(src_id)
    .bind(dst_kind)
    .bind(dst_id)
    .execute(pool)
    .await
    .unwrap();
}

/// A pool whose every connection drops to the genuine runtime role `console_rt`
/// (NOSUPERUSER, NOBYPASSRLS) before use. `user_branches` (needed by
/// `resolve_person`'s branch-scoped join) predates the runtime role and has no
/// explicit grant, and the `#[sqlx::test]` harness runs migrations as a
/// non-`console_app` superuser so the post-0031 default-privilege auto-grant never
/// fires for it either — so it, like `object_links`/`object_types`, needs an
/// explicit grant here to faithfully exercise the runtime read path.
async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    for grant in [
        "GRANT SELECT, INSERT, DELETE ON object_links TO console_rt",
        "GRANT SELECT ON object_types TO console_rt",
        "GRANT SELECT ON user_branches TO console_rt",
    ] {
        sqlx::query(grant).execute(owner_pool).await.unwrap();
    }
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
