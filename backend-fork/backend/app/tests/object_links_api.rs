#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-OBJ object_links: HTTP contract round-trip + a RUNTIME-role RLS proof.
//!
//! The round-trip test drives the real router (create/list/delete, duplicate
//! rejection, unknown-kind rejection, audit emission). The isolation test runs
//! as the genuine non-owner `console_rt` role (NOBYPASSRLS, FORCE RLS) — NOT the
//! default `#[sqlx::test]` BYPASSRLS superuser, which would see every row and
//! green-light a broken tenant filter — to prove org B cannot see org A's link.

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

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn create_list_delete_roundtrip_and_audit_events(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let user_id = UserId::new();
    seed_user(&pool, user_id, "ADMIN").await;
    let token = issue_token(private_pem.as_bytes(), public_key_pem.as_bytes(), user_id);

    // Create: document doc-1 --authorized_by--> voucher vou-1. Both are
    // registered-but-non-resolvable kinds (pure link targets), so this contract
    // test exercises create/list/delete/audit without the B3 endpoint-visibility
    // gate (which only applies to resolvable kinds); that gate is proven
    // separately in `create_link_requires_visible_endpoints`.
    let created = request(
        &pool,
        &public_key_pem,
        Request::builder()
            .method("POST")
            .uri("/api/v1/object-links")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "src_kind": "document",
                    "src_id": "doc-1",
                    "dst_kind": "voucher",
                    "dst_id": "vou-1",
                    "link_type": "authorized_by"
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(created.0, StatusCode::OK, "create body: {}", created.1);
    let link_id = created.1["id"].as_str().unwrap().to_owned();

    // List by source end: one outgoing, no incoming.
    let src_list = request(
        &pool,
        &public_key_pem,
        get("/api/v1/object-links?kind=document&id=doc-1", &token),
    )
    .await;
    assert_eq!(src_list.0, StatusCode::OK);
    assert_eq!(src_list.1["outgoing"].as_array().unwrap().len(), 1);
    assert_eq!(src_list.1["incoming"].as_array().unwrap().len(), 0);

    // List by destination end: same link appears as incoming.
    let dst_list = request(
        &pool,
        &public_key_pem,
        get("/api/v1/object-links?kind=voucher&id=vou-1", &token),
    )
    .await;
    assert_eq!(dst_list.1["incoming"].as_array().unwrap().len(), 1);
    assert_eq!(dst_list.1["outgoing"].as_array().unwrap().len(), 0);
    assert_eq!(dst_list.1["incoming"][0]["id"], link_id);

    // Duplicate is rejected (409), not silently doubled.
    let dup = request(
        &pool,
        &public_key_pem,
        Request::builder()
            .method("POST")
            .uri("/api/v1/object-links")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "src_kind": "document", "src_id": "doc-1",
                    "dst_kind": "voucher", "dst_id": "vou-1",
                    "link_type": "authorized_by"
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(dup.0, StatusCode::CONFLICT);

    // Unknown kind is rejected (422).
    let unknown = request(
        &pool,
        &public_key_pem,
        Request::builder()
            .method("POST")
            .uri("/api/v1/object-links")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "src_kind": "document", "src_id": "doc-1",
                    "dst_kind": "not_a_real_kind", "dst_id": "x-1",
                    "link_type": "relates_to"
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(unknown.0, StatusCode::UNPROCESSABLE_ENTITY);

    // Delete: 204, then the object has no links.
    let del = request(
        &pool,
        &public_key_pem,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/object-links/{link_id}"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(del.0, StatusCode::NO_CONTENT);

    let after = request(
        &pool,
        &public_key_pem,
        get("/api/v1/object-links?kind=document&id=doc-1", &token),
    )
    .await;
    assert_eq!(after.1["outgoing"].as_array().unwrap().len(), 0);

    // Deleting an unknown id is 404 (deny-by-omission).
    let missing = request(
        &pool,
        &public_key_pem,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/object-links/{}", Uuid::new_v4()))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);

    // Audit: create + delete each emitted exactly one event; the delete carries
    // the removed edge as its before-snapshot.
    let create_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = 'object_link.create'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(create_count, 1);
    let delete_before: Option<Value> = sqlx::query_scalar(
        "SELECT before_snap FROM audit_events WHERE action = 'object_link.delete'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let before = delete_before.expect("delete audit has a before-snapshot");
    assert_eq!(before["link_type"], "authorized_by");
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn rls_cross_org_isolation_as_runtime_role(owner_pool: PgPool) {
    let knl = *OrgId::knl().as_uuid();
    seed_org(&owner_pool, OTHER_ORG, "Other").await;

    // Owner (BYPASSRLS) plants an org-A (KNL) link directly.
    let link_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO object_links (id, org_id, src_kind, src_id, dst_kind, dst_id, link_type)
        VALUES ($1, $2, 'work_order', 'wo-1', 'equipment', 'eq-1', 'uses')
        "#,
    )
    .bind(link_id)
    .bind(knl)
    .execute(&owner_pool)
    .await
    .unwrap();

    let rt_pool = runtime_role_pool(&owner_pool).await;

    // As console_rt under org B's GUC: A's link is invisible (FORCE RLS).
    let seen_by_other = count_links_scoped(&rt_pool, OTHER_ORG).await;
    assert_eq!(seen_by_other, 0, "org B must not see org A's link");

    // As console_rt under org A's GUC: the link is visible.
    let seen_by_owner = count_links_scoped(&rt_pool, knl).await;
    assert_eq!(seen_by_owner, 1, "org A sees its own link");
}

/// B2: a link is an audited edge, so deletion is restricted to its creator or a
/// UserManage-tier admin — NOT open to every Login member the way create/list
/// are. A non-creator, non-manager member gets 403; the creator can still
/// delete (proving the guard denies the outsider, not the owner).
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn delete_link_denied_for_non_creator_non_manager(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    // Creator (ADMIN, holds UserManage) plants a link; created_by = creator.
    let creator = UserId::new();
    seed_user(&pool, creator, "ADMIN").await;
    let creator_token = issue_token(private_pem.as_bytes(), public_key_pem.as_bytes(), creator);
    let created = request(
        &pool,
        &public_key_pem,
        Request::builder()
            .method("POST")
            .uri("/api/v1/object-links")
            .header(header::AUTHORIZATION, format!("Bearer {creator_token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "src_kind": "document", "src_id": "doc-1",
                    "dst_kind": "voucher", "dst_id": "vou-1",
                    "link_type": "uses"
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(created.0, StatusCode::OK, "create body: {}", created.1);
    let link_id = created.1["id"].as_str().unwrap().to_owned();

    // A different MEMBER: holds Login (can create/list) but not UserManage, and
    // is not the creator -> delete is 403, not a silent success.
    let outsider = UserId::new();
    seed_user(&pool, outsider, "MEMBER").await;
    let outsider_token = issue_token_with_roles(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        outsider,
        vec!["MEMBER".to_owned()],
    );
    let denied = request(
        &pool,
        &public_key_pem,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/object-links/{link_id}"))
            .header(header::AUTHORIZATION, format!("Bearer {outsider_token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        denied.0,
        StatusCode::FORBIDDEN,
        "non-creator non-manager must be denied delete: {}",
        denied.1
    );

    // The link still exists: its creator can delete it -> 204.
    let by_creator = request(
        &pool,
        &public_key_pem,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/object-links/{link_id}"))
            .header(header::AUTHORIZATION, format!("Bearer {creator_token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(by_creator.0, StatusCode::NO_CONTENT);
}

/// B3: an object_link may only connect endpoints the caller can actually
/// resolve. A link to a resolvable object that is absent OR out of the caller's
/// branch scope is rejected (422, deny-by-omission — one message, no
/// absent-vs-invisible oracle); a link between two visible objects is created.
/// Endpoints of non-resolvable kinds still pass through (no visibility surface)
/// — proven by the roundtrip test's document->voucher link.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn create_link_requires_visible_endpoints(pool: PgPool) {
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
    // Caller's branch scope is exactly {branch_x}.
    let token = issue_token_in_branches(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        caller,
        vec![branch_x],
    );

    // Two persons the caller can see (active, in branch_x).
    let subject_a = UserId::new();
    let subject_b = UserId::new();
    seed_user_in_branch(&pool, subject_a, "MECHANIC", branch_x).await;
    seed_user_in_branch(&pool, subject_b, "MECHANIC", branch_x).await;
    // A person only in branch_y — outside the caller's scope.
    let branch_y_user = UserId::new();
    seed_user_in_branch(&pool, branch_y_user, "MECHANIC", branch_y).await;

    let person = |u: &UserId| u.as_uuid().to_string();

    // Both endpoints visible -> created.
    let ok = post_link(
        &pool,
        &public_key_pem,
        &token,
        json!({
            "src_kind": "person", "src_id": person(&subject_a),
            "dst_kind": "person", "dst_id": person(&subject_b),
            "link_type": "relates_to"
        }),
    )
    .await;
    assert_eq!(ok.0, StatusCode::OK, "both-visible link body: {}", ok.1);

    // dst absent (random id) -> rejected.
    let absent = post_link(
        &pool,
        &public_key_pem,
        &token,
        json!({
            "src_kind": "person", "src_id": person(&subject_a),
            "dst_kind": "person", "dst_id": Uuid::new_v4().to_string(),
            "link_type": "relates_to"
        }),
    )
    .await;
    assert_eq!(
        absent.0,
        StatusCode::UNPROCESSABLE_ENTITY,
        "link to absent object must be rejected: {}",
        absent.1
    );

    // dst out of the caller's branch scope -> rejected, byte-identical to absent.
    let out_of_scope = post_link(
        &pool,
        &public_key_pem,
        &token,
        json!({
            "src_kind": "person", "src_id": person(&subject_a),
            "dst_kind": "person", "dst_id": person(&branch_y_user),
            "link_type": "relates_to"
        }),
    )
    .await;
    assert_eq!(
        out_of_scope.0,
        StatusCode::UNPROCESSABLE_ENTITY,
        "link to out-of-scope object must be denied by omission: {}",
        out_of_scope.1
    );
    assert_eq!(
        out_of_scope.1["error"]["message"], absent.1["error"]["message"],
        "absent and out-of-scope must share one message (no existence oracle)"
    );

    // src invisible (out of scope) with a visible dst -> also rejected.
    let bad_src = post_link(
        &pool,
        &public_key_pem,
        &token,
        json!({
            "src_kind": "person", "src_id": person(&branch_y_user),
            "dst_kind": "person", "dst_id": person(&subject_b),
            "link_type": "relates_to"
        }),
    )
    .await;
    assert_eq!(
        bad_src.0,
        StatusCode::UNPROCESSABLE_ENTITY,
        "link from an invisible src must be rejected: {}",
        bad_src.1
    );
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

async fn post_link(
    pool: &PgPool,
    public_key_pem: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    request(
        pool,
        public_key_pem,
        Request::builder()
            .method("POST")
            .uri("/api/v1/object-links")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
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

fn issue_token_in_branches(
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    branches: Vec<BranchId>,
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
            roles: vec!["ADMIN".to_owned()],
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

fn get(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

async fn request(pool: &PgPool, public_key_pem: &str, req: Request<Body>) -> (StatusCode, Value) {
    let service = build_router(app_state(pool.clone(), public_key_pem.to_owned()).unwrap());
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

async fn count_links_scoped(rt_pool: &PgPool, org: Uuid) -> i64 {
    console_platform_db::with_org_conn::<_, i64, console_platform_db::DbError>(
        rt_pool,
        OrgId::from_uuid(org),
        move |tx| {
            Box::pin(async move {
                Ok(sqlx::query_scalar("SELECT COUNT(*) FROM object_links")
                    .fetch_one(tx.as_mut())
                    .await?)
            })
        },
    )
    .await
    .unwrap()
}

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    for grant in [
        "GRANT SELECT, INSERT, DELETE ON object_links TO console_rt",
        "GRANT SELECT ON object_types TO console_rt",
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

async fn seed_user(pool: &PgPool, user_id: UserId, role: &str) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("User {role} {}", Uuid::new_v4()))
        .bind(Vec::from([role]))
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

fn issue_token(private_key_pem: &[u8], public_key_pem: &[u8], user_id: UserId) -> String {
    issue_token_with_roles(
        private_key_pem,
        public_key_pem,
        user_id,
        vec!["ADMIN".to_owned()],
    )
}

fn issue_token_with_roles(
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
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
