#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-OBJ slice 3 — ontology depth: type registry, SR- series, edge-type
//! registry.
//!
//! Every HTTP test drives the real router through a genuine `console_rt` pool
//! (NOBYPASSRLS, FORCE RLS) — NOT the default `#[sqlx::test]` BYPASSRLS owner,
//! which would green-light a broken tenant/branch filter — so the visibility
//! guarantees (per-kind visibility of the type counts, deny-by-omission on
//! series instances, org isolation of series) are proven as the real runtime
//! role. Negative cross-branch / insufficient-role / already-in-series /
//! unregistered-edge-type cases are mandatory coverage here.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{BranchId, OrgId, UserId};
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
use uuid::Uuid;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const OTHER_ORG: Uuid = Uuid::from_u128(0x0b1e_0b1e_0b1e_0b1e_0b1e_0b1e_0b1e_0b1e);

// ===========================================================================
// Surface 1 — type registry: counts respect per-kind visibility.
// ===========================================================================

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn type_registry_counts_respect_per_kind_visibility(owner_pool: PgPool) {
    let (private_pem, public_pem) = keys();
    let branch_x = seed_branch(&owner_pool, "Region X", "Branch X").await;
    let branch_y = seed_branch(&owner_pool, "Region Y", "Branch Y").await;

    // 4 active users in branch_x (caller + two subjects + a MEMBER), 1 in
    // branch_y (out of the caller's scope).
    let caller = UserId::new();
    seed_user_in_branch(&owner_pool, caller, "ADMIN", branch_x).await;
    seed_user_in_branch(&owner_pool, UserId::new(), "MECHANIC", branch_x).await;
    seed_user_in_branch(&owner_pool, UserId::new(), "MECHANIC", branch_x).await;
    let member = UserId::new();
    seed_user_in_branch(&owner_pool, member, "MEMBER", branch_x).await;
    seed_user_in_branch(&owner_pool, UserId::new(), "MECHANIC", branch_y).await;

    let rt = runtime_role_pool(&owner_pool).await;
    let admin = issue_token(
        &private_pem,
        &public_pem,
        caller,
        vec![branch_x],
        vec!["ADMIN"],
    );

    // ADMIN scoped to {branch_x}: list carries counts that respect visibility.
    let (status, list) = get(&rt, &public_pem, "/api/v1/object-types", &admin).await;
    assert_eq!(status, StatusCode::OK, "list body: {list}");
    let types = list.as_array().unwrap();

    // person (membership-gated): the 4 branch_x users; the branch_y user is NOT
    // counted — the branch filter is real, not org-wide.
    assert_eq!(count_of(types, "person"), 4, "person count (branch_x only)");
    // account (UserManage-gated): ADMIN holds it -> same 4 in-scope users.
    assert_eq!(count_of(types, "account"), 4, "account count for ADMIN");
    // org_unit: only branch_x is in scope (branch_y excluded).
    assert_eq!(
        count_of(types, "org_unit"),
        1,
        "org_unit count (in-scope only)"
    );

    // Metadata: code_prefix + status surface from the registry.
    let support = find_type(types, "support_ticket");
    assert_eq!(support["code_prefix"], "CS-");
    assert_eq!(support["status"], "active");
    assert_eq!(find_type(types, "series")["code_prefix"], "SR-");

    // Single-kind endpoint agrees with the list.
    let (s2, one) = get(&rt, &public_pem, "/api/v1/object-types/account", &admin).await;
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(one["active_count"], 4);
    // Unknown (well-formed) kind -> 404.
    let (s3, _) = get(&rt, &public_pem, "/api/v1/object-types/banana", &admin).await;
    assert_eq!(s3, StatusCode::NOT_FOUND);

    // MEMBER (Login only, no UserManage / WorkOrderReadAll): the feature-gated
    // kinds count 0 for them even though the rows exist, exactly as resolve
    // would deny every instance; membership-gated kinds stay at parity.
    let member_token = issue_token(
        &private_pem,
        &public_pem,
        member,
        vec![branch_x],
        vec!["MEMBER"],
    );
    let (_, mlist) = get(&rt, &public_pem, "/api/v1/object-types", &member_token).await;
    let mtypes = mlist.as_array().unwrap();
    assert_eq!(
        count_of(mtypes, "account"),
        0,
        "MEMBER cannot count accounts"
    );
    assert_eq!(
        count_of(mtypes, "org_unit"),
        1,
        "MEMBER still sees in-scope org_unit"
    );
    assert_eq!(
        count_of(mtypes, "person"),
        4,
        "person is membership-gated, not feature-gated"
    );
}

// ===========================================================================
// Surface 2 — SR- series: create / attach / read / by-instance.
// ===========================================================================

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn series_lifecycle_and_deny_by_omission(owner_pool: PgPool) {
    let (private_pem, public_pem) = keys();
    let branch_x = seed_branch(&owner_pool, "Region X", "Branch X").await;
    let branch_y = seed_branch(&owner_pool, "Region Y", "Branch Y").await;
    let caller = UserId::new();
    seed_user_in_branch(&owner_pool, caller, "ADMIN", branch_x).await;
    let subj1 = UserId::new();
    let subj2 = UserId::new();
    seed_user_in_branch(&owner_pool, subj1, "MECHANIC", branch_x).await;
    seed_user_in_branch(&owner_pool, subj2, "MECHANIC", branch_x).await;
    let subj_y = UserId::new();
    seed_user_in_branch(&owner_pool, subj_y, "MECHANIC", branch_y).await;

    let rt = runtime_role_pool(&owner_pool).await;
    let token = issue_token(
        &private_pem,
        &public_pem,
        caller,
        vec![branch_x],
        vec!["ADMIN"],
    );

    // Create a series seeded with subj1 -> SR- code, one instance.
    let created = post(
        &rt,
        &public_pem,
        "/api/v1/series",
        &token,
        json!({ "label": "정비 이력", "kind": "person", "id": id(subj1) }),
    )
    .await;
    assert_eq!(created.0, StatusCode::OK, "create body: {}", created.1);
    let code = created.1["code"].as_str().unwrap().to_owned();
    assert!(code.starts_with("SR-"), "canonical SR- code: {code}");
    let series_id = created.1["id"].as_str().unwrap().to_owned();
    assert_eq!(created.1["instances"].as_array().unwrap().len(), 1);

    // Founding a series on an object the caller cannot see is denied (404).
    let cross = post(
        &rt,
        &public_pem,
        "/api/v1/series",
        &token,
        json!({ "label": "x", "kind": "person", "id": id(subj_y) }),
    )
    .await;
    assert_eq!(
        cross.0,
        StatusCode::NOT_FOUND,
        "cross-branch founder denied"
    );

    // Attach subj2 (in scope) -> ok.
    let attach = post(
        &rt,
        &public_pem,
        &format!("/api/v1/series/{series_id}/instances"),
        &token,
        json!({ "kind": "person", "id": id(subj2) }),
    )
    .await;
    assert_eq!(attach.0, StatusCode::OK, "attach body: {}", attach.1);

    // Attach a cross-branch instance -> 404 (deny-by-omission, not resolvable).
    let attach_cross = post(
        &rt,
        &public_pem,
        &format!("/api/v1/series/{series_id}/instances"),
        &token,
        json!({ "kind": "person", "id": id(subj_y) }),
    )
    .await;
    assert_eq!(attach_cross.0, StatusCode::NOT_FOUND);

    // Re-attaching subj1 (already in this series) -> 409 (one-series invariant).
    let dup = post(
        &rt,
        &public_pem,
        &format!("/api/v1/series/{series_id}/instances"),
        &token,
        json!({ "kind": "person", "id": id(subj1) }),
    )
    .await;
    assert_eq!(dup.0, StatusCode::CONFLICT);

    // Attaching to an unknown series -> 404.
    let missing = post(
        &rt,
        &public_pem,
        &format!("/api/v1/series/{}/instances", Uuid::new_v4()),
        &token,
        json!({ "kind": "person", "id": id(subj2) }),
    )
    .await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);

    // Read: two instances, ordered by attach time (subj1 then subj2).
    let (rstatus, detail) = get(
        &rt,
        &public_pem,
        &format!("/api/v1/series/{series_id}"),
        &token,
    )
    .await;
    assert_eq!(rstatus, StatusCode::OK);
    let instances = detail["instances"].as_array().unwrap();
    assert_eq!(instances.len(), 2, "both resolved instances: {detail}");
    assert_eq!(instances[0]["id"], id(subj1));
    assert_eq!(instances[1]["id"], id(subj2));

    // by-instance: subj1 -> the created series.
    let (bstatus, by) = get(
        &rt,
        &public_pem,
        &format!("/api/v1/series/by-instance?kind=person&id={}", id(subj1)),
        &token,
    )
    .await;
    assert_eq!(bstatus, StatusCode::OK);
    assert_eq!(by["series"]["code"], code);

    // by-instance for an unattached (but in-scope) object -> null.
    let (_, none) = get(
        &rt,
        &public_pem,
        &format!("/api/v1/series/by-instance?kind=person&id={}", id(caller)),
        &token,
    )
    .await;
    assert!(none["series"].is_null(), "unattached object: {none}");

    // by-instance for a cross-branch object -> null (never an existence oracle,
    // even though the row physically exists in another branch).
    let (_, oracle) = get(
        &rt,
        &public_pem,
        &format!("/api/v1/series/by-instance?kind=person&id={}", id(subj_y)),
        &token,
    )
    .await;
    assert!(oracle["series"].is_null(), "cross-branch not revealed");

    // Insufficient role: a principal without Login cannot create a series.
    let no_login = UserId::new();
    seed_user_in_branch(&owner_pool, no_login, "MECHANIC", branch_x).await;
    let no_login_token = issue_token(&private_pem, &public_pem, no_login, vec![branch_x], vec![]);
    let denied = post(
        &rt,
        &public_pem,
        "/api/v1/series",
        &no_login_token,
        json!({ "label": "x", "kind": "person", "id": id(subj1) }),
    )
    .await;
    assert_eq!(denied.0, StatusCode::FORBIDDEN);

    // Audit: create emitted series.create + series.attach; the second attach
    // added one more series.attach (3 attach-family events total: 2 attaches +
    // 1 create).
    let create_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = 'series.create'")
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(create_events, 1);
    let attach_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = 'series.attach'")
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(attach_events, 2, "first-instance attach + explicit attach");
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn series_cross_org_isolation_as_runtime_role(owner_pool: PgPool) {
    let knl = *OrgId::knl().as_uuid();
    seed_org(&owner_pool, OTHER_ORG, "Other").await;

    // Owner (BYPASSRLS) plants a KNL series + its membership row directly.
    let series_id = Uuid::new_v4();
    sqlx::query("INSERT INTO series (id, org_id, code, label) VALUES ($1, $2, 'SR-1', 'KNL')")
        .bind(series_id)
        .bind(knl)
        .execute(&owner_pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO series_instances (org_id, series_id, member_kind, member_id) \
         VALUES ($1, $2, 'person', 'p-1')",
    )
    .bind(knl)
    .bind(series_id)
    .execute(&owner_pool)
    .await
    .unwrap();

    let rt = runtime_role_pool(&owner_pool).await;
    assert_eq!(
        count_scoped(&rt, "series", OTHER_ORG).await,
        0,
        "org B blind to A's series"
    );
    assert_eq!(
        count_scoped(&rt, "series", knl).await,
        1,
        "org A sees its series"
    );
    assert_eq!(
        count_scoped(&rt, "series_instances", OTHER_ORG).await,
        0,
        "org B blind to A's membership rows"
    );
    assert_eq!(count_scoped(&rt, "series_instances", knl).await, 1);
}

// ===========================================================================
// Surface 3 — edge-type registry + link validation.
// ===========================================================================

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn link_types_registry_and_link_validation(owner_pool: PgPool) {
    let (private_pem, public_pem) = keys();
    let caller = UserId::new();
    let branch = seed_branch(&owner_pool, "Region L", "Branch L").await;
    seed_user_in_branch(&owner_pool, caller, "ADMIN", branch).await;
    let rt = runtime_role_pool(&owner_pool).await;
    let token = issue_token(
        &private_pem,
        &public_pem,
        caller,
        vec![branch],
        vec!["ADMIN"],
    );

    // Registry lists the seeded vocabulary with status.
    let (status, list) = get(&rt, &public_pem, "/api/v1/link-types", &token).await;
    assert_eq!(status, StatusCode::OK);
    let types = list.as_array().unwrap();
    let names: Vec<&str> = types
        .iter()
        .map(|t| t["link_type"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"relates_to"), "vocabulary: {names:?}");
    assert!(names.contains(&"depends_on"));
    assert!(names.contains(&"authorized_by"));
    assert_eq!(types[0]["status"], "active");

    // A registered link_type is accepted. Use pure link-target kinds here so
    // this test isolates the edge-type registry; resolvable endpoint visibility
    // is covered by object_links_api::create_link_requires_visible_endpoints.
    let ok = post(
        &rt,
        &public_pem,
        "/api/v1/object-links",
        &token,
        json!({
            "src_kind": "document", "src_id": "doc-1",
            "dst_kind": "voucher", "dst_id": "vou-1",
            "link_type": "relates_to"
        }),
    )
    .await;
    assert_eq!(
        ok.0,
        StatusCode::OK,
        "registered link_type accepted: {}",
        ok.1
    );

    // A well-formed but UNREGISTERED link_type is rejected before the FK fires.
    let bad = post(
        &rt,
        &public_pem,
        "/api/v1/object-links",
        &token,
        json!({
            "src_kind": "document", "src_id": "doc-2",
            "dst_kind": "voucher", "dst_id": "vou-2",
            "link_type": "totally_made_up"
        }),
    )
    .await;
    assert_eq!(
        bad.0,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unregistered edge type rejected: {}",
        bad.1
    );
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn keys() -> (Vec<u8>, String) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    (private_pem.as_bytes().to_vec(), public_pem)
}

fn id(u: UserId) -> String {
    u.as_uuid().to_string()
}

fn count_of(types: &[Value], kind: &str) -> i64 {
    find_type(types, kind)["active_count"].as_i64().unwrap()
}

fn find_type<'a>(types: &'a [Value], kind: &str) -> &'a Value {
    types
        .iter()
        .find(|t| t["kind"] == kind)
        .unwrap_or_else(|| panic!("kind {kind} missing from registry"))
}

async fn get(pool: &PgPool, public_pem: &str, uri: &str, token: &str) -> (StatusCode, Value) {
    request(
        pool,
        public_pem,
        Request::builder()
            .method("GET")
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn post(
    pool: &PgPool,
    public_pem: &str,
    uri: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    request(
        pool,
        public_pem,
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn request(pool: &PgPool, public_pem: &str, req: Request<Body>) -> (StatusCode, Value) {
    let service = build_router(app_state(pool.clone(), public_pem.to_owned()).unwrap());
    let response = service.oneshot(req).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, json)
}

async fn count_scoped(rt_pool: &PgPool, table: &str, org: Uuid) -> i64 {
    // sqlx 0.9 requires literal SQL; branch on the (test-fixed) table name.
    let sql: &'static str = match table {
        "series" => "SELECT count(*) FROM series",
        "series_instances" => "SELECT count(*) FROM series_instances",
        other => panic!("unsupported count table {other}"),
    };
    console_platform_db::with_org_conn::<_, i64, console_platform_db::DbError>(
        rt_pool,
        OrgId::from_uuid(org),
        move |tx| {
            Box::pin(async move { Ok(sqlx::query_scalar(sql).fetch_one(tx.as_mut()).await?) })
        },
    )
    .await
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

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(owner_pool)
    .await
    .unwrap();
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

fn issue_token(
    private_key_pem: &[u8],
    public_key_pem: &str,
    user_id: UserId,
    branches: Vec<BranchId>,
    roles: Vec<&str>,
) -> String {
    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private_key_pem,
        public_key_pem.as_bytes(),
    )
    .unwrap();
    issuer
        .issue_access_token(AccessTokenInput {
            subject: user_id,
            org_id: OrgId::knl(),
            roles: roles.into_iter().map(str::to_owned).collect(),
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
