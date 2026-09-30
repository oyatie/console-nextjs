#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Global object/person search (GET /api/v1/search?q=).
//!
//! Runs the REAL router on a genuine non-owner `console_rt` pool (FORCE RLS actually
//! enforced, not bypassed by the table owner) and proves the leak-critical
//! guarantees the palette/compose-picker/explore surfaces depend on:
//! - person directory hits are messenger-member-scoped (active + shared branch);
//!   a cross-branch or inactive user never appears;
//! - work_order/equipment hits are gated on `WorkOrderReadAll` exactly as
//!   `resolveObject` — a MEMBER lacking it gets zero hits of those kinds
//!   (unauthorized-kind omission), an ADMIN holding it sees them;
//! - support_ticket and org_unit hits obey the same branch-visible omission as
//!   resolveObject (no cross-branch leak through search);
//! - cross-org callers see nothing (RLS), never a 403.

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
const OTHER_ORG: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_9999);

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn search_persons_scoped_to_active_and_shared_branch(pool: PgPool) {
    let keys = Keys::new();
    let branch_x = seed_branch(&pool, "Region X", "Branch X").await;
    let branch_y = seed_branch(&pool, "Region Y", "Branch Y").await;

    let caller = UserId::new();
    seed_named_user(&pool, caller, "ADMIN", "Caller Admin", branch_x, true).await;

    // In-scope active person -> visible.
    let in_scope = UserId::new();
    seed_named_user(
        &pool,
        in_scope,
        "MECHANIC",
        "Searchable Alpha",
        branch_x,
        true,
    )
    .await;
    // Cross-branch person (branch_y only) -> must NOT appear.
    let cross_branch = UserId::new();
    seed_named_user(
        &pool,
        cross_branch,
        "MECHANIC",
        "Searchable Bravo",
        branch_y,
        true,
    )
    .await;
    // In-scope but INACTIVE person -> must NOT appear (no deactivation oracle).
    let inactive = UserId::new();
    seed_named_user(
        &pool,
        inactive,
        "MECHANIC",
        "Searchable Charlie",
        branch_x,
        false,
    )
    .await;

    let token = keys.token(caller, OrgId::knl().as_uuid(), &["ADMIN"], &[branch_x]);
    let hits = search(&pool, &keys, &token, "searchable", None).await;
    assert_eq!(hits.0, StatusCode::OK);
    let persons = person_ids(&hits.1);
    assert!(
        persons.contains(&in_scope.as_uuid().to_string()),
        "in-scope active person must be found: {:?}",
        hits.1
    );
    assert!(
        !persons.contains(&cross_branch.as_uuid().to_string()),
        "cross-branch person must be omitted: {:?}",
        hits.1
    );
    assert!(
        !persons.contains(&inactive.as_uuid().to_string()),
        "inactive person must be omitted: {:?}",
        hits.1
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn search_equipment_gated_by_work_order_read_all(pool: PgPool) {
    let keys = Keys::new();
    let branch = seed_branch(&pool, "Region G", "Branch G").await;

    let admin = UserId::new();
    seed_named_user(&pool, admin, "ADMIN", "Gate Admin", branch, true).await;
    let member = UserId::new();
    seed_named_user(&pool, member, "MEMBER", "Gate Member", branch, true).await;

    // registry_equipment shares the EXACT WorkOrderReadAll gate + branch
    // predicate with the work_order kind, so seeding the (lighter) equipment
    // chain proves the gate for both.
    let equipment_no = "ZEB99-0007";
    seed_equipment(&pool, branch, equipment_no).await;

    // ADMIN (WorkOrderReadAll) sees the equipment hit.
    let admin_token = keys.token(admin, OrgId::knl().as_uuid(), &["ADMIN"], &[branch]);
    let admin_hits = search(&pool, &keys, &admin_token, "zeb99", None).await;
    assert_eq!(admin_hits.0, StatusCode::OK);
    assert!(
        equipment_codes(&admin_hits.1).contains(&equipment_no.to_owned()),
        "ADMIN must see the equipment hit: {:?}",
        admin_hits.1
    );

    // MEMBER (Login only, no WorkOrderReadAll) gets ZERO equipment/work_order
    // hits — the kind is not even queried (unauthorized-kind omission), and it is
    // a 200 with absence, never a 403.
    let member_token = keys.token(member, OrgId::knl().as_uuid(), &["MEMBER"], &[branch]);
    let member_hits = search(&pool, &keys, &member_token, "zeb99", None).await;
    assert_eq!(member_hits.0, StatusCode::OK);
    assert!(
        !kinds_present(&member_hits.1).contains(&"equipment".to_owned()),
        "MEMBER must get no equipment hits: {:?}",
        member_hits.1
    );
    assert!(
        !kinds_present(&member_hits.1).contains(&"work_order".to_owned()),
        "MEMBER must get no work_order hits: {:?}",
        member_hits.1
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn search_support_ticket_and_org_unit_are_branch_scoped(pool: PgPool) {
    let keys = Keys::new();
    let branch_in = seed_branch(&pool, "Region Scope In", "Scoped Visible Branch").await;
    let branch_out = seed_branch(&pool, "Region Scope Out", "Scoped Hidden Branch").await;

    let caller = UserId::new();
    seed_named_user(&pool, caller, "ADMIN", "Scoped Caller", branch_in, true).await;
    let out_requester = UserId::new();
    seed_named_user(
        &pool,
        out_requester,
        "MEMBER",
        "Out Requester",
        branch_out,
        true,
    )
    .await;

    seed_support_ticket(&pool, branch_in, caller, "Scoped Visible Support Ticket").await;
    seed_support_ticket(
        &pool,
        branch_out,
        out_requester,
        "Scoped Hidden Support Ticket",
    )
    .await;

    let token = keys.token(caller, OrgId::knl().as_uuid(), &["ADMIN"], &[branch_in]);
    let hits = search(&pool, &keys, &token, "scoped", None).await;
    assert_eq!(hits.0, StatusCode::OK);

    let support_titles = titles_for_kind(&hits.1, "support_ticket");
    assert!(
        support_titles.contains(&"Scoped Visible Support Ticket".to_owned()),
        "in-scope support_ticket must be visible: {:?}",
        hits.1
    );
    assert!(
        !support_titles.contains(&"Scoped Hidden Support Ticket".to_owned()),
        "out-of-scope support_ticket must be omitted: {:?}",
        hits.1
    );

    let org_unit_titles = titles_for_kind(&hits.1, "org_unit");
    assert!(
        org_unit_titles.contains(&"Scoped Visible Branch".to_owned()),
        "in-scope org_unit must be visible: {:?}",
        hits.1
    );
    assert!(
        !org_unit_titles.contains(&"Scoped Hidden Branch".to_owned()),
        "out-of-scope org_unit must be omitted: {:?}",
        hits.1
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn search_denies_cross_org_by_omission(pool: PgPool) {
    let keys = Keys::new();
    let branch = seed_branch(&pool, "Region KNL", "Branch KNL").await;
    let knl_user = UserId::new();
    seed_named_user(&pool, knl_user, "ADMIN", "Searchable Knl", branch, true).await;
    seed_org(&pool, OTHER_ORG, "Other").await;

    // A caller whose token is scoped to a DIFFERENT org: even a SUPER_ADMIN sees
    // nothing of KNL — app.current_org (FORCE RLS) hides every row.
    let outsider = UserId::new();
    let outsider_token = keys.token(outsider, &OTHER_ORG, &["SUPER_ADMIN"], &[]);
    let hits = search(&pool, &keys, &outsider_token, "searchable", None).await;
    assert_eq!(hits.0, StatusCode::OK);
    assert!(
        hits.1["results"].as_array().unwrap().is_empty(),
        "cross-org caller must see no hits: {:?}",
        hits.1
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn search_empty_query_is_empty_and_limit_clamps(pool: PgPool) {
    let keys = Keys::new();
    let branch = seed_branch(&pool, "Region E", "Branch E").await;
    let caller = UserId::new();
    seed_named_user(&pool, caller, "ADMIN", "Caller E", branch, true).await;
    let token = keys.token(caller, OrgId::knl().as_uuid(), &["ADMIN"], &[branch]);

    // Empty/whitespace query -> 200 empty (no 422 noise for eager palette calls).
    let empty = search(&pool, &keys, &token, "   ", None).await;
    assert_eq!(empty.0, StatusCode::OK);
    assert!(empty.1["results"].as_array().unwrap().is_empty());

    // A wildcard in the query is matched literally (escaped), not as a LIKE
    // wildcard: '%' matches no display name here.
    let literal = search(&pool, &keys, &token, "%", None).await;
    assert_eq!(literal.0, StatusCode::OK);
    assert!(
        literal.1["results"].as_array().unwrap().is_empty(),
        "'%' must be a literal, not a wildcard: {:?}",
        literal.1
    );

    // limit is accepted (out-of-range clamps rather than erroring).
    let clamped = search(&pool, &keys, &token, "caller", Some(9999)).await;
    assert_eq!(clamped.0, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn person_ids(body: &Value) -> Vec<String> {
    body["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|h| h["kind"] == "person")
                .filter_map(|h| h["id"].as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn equipment_codes(body: &Value) -> Vec<String> {
    body["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|h| h["kind"] == "equipment")
                .filter_map(|h| h["code"].as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn kinds_present(body: &Value) -> Vec<String> {
    body["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|h| h["kind"].as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn titles_for_kind(body: &Value, kind: &str) -> Vec<String> {
    body["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|h| h["kind"] == kind)
                .filter_map(|h| h["title"].as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

async fn search(
    pool: &PgPool,
    keys: &Keys,
    token: &str,
    q: &str,
    limit: Option<i64>,
) -> (StatusCode, Value) {
    let service = build_router(app_state(
        runtime_role_pool(pool).await,
        keys.public_pem.clone(),
    ));
    let mut uri = format!("/api/v1/search?q={}", urlencoding(q));
    if let Some(limit) = limit {
        uri.push_str(&format!("&limit={limit}"));
    }
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

/// Minimal percent-encoding for the query-string values used in these tests
/// (spaces and `%`).
fn urlencoding(raw: &str) -> String {
    raw.chars()
        .map(|c| match c {
            ' ' => "%20".to_owned(),
            '%' => "%25".to_owned(),
            other => other.to_string(),
        })
        .collect()
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

async fn seed_named_user(
    pool: &PgPool,
    user_id: UserId,
    role: &str,
    display_name: &str,
    branch: BranchId,
    active: bool,
) {
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, org_id, is_active) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(*user_id.as_uuid())
    .bind(display_name)
    .bind(Vec::from([role]))
    .bind(*OrgId::knl().as_uuid())
    .bind(active)
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

async fn seed_equipment(pool: &PgPool, branch: BranchId, equipment_no: &str) {
    let customer_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind("Search Customer")
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch.as_uuid())
    .bind(customer_id)
    .bind("Search Site")
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO registry_equipment \
             (branch_id, customer_id, site_id, equipment_no, manufacturer_code, kind_code, \
              power_code, status, specification, ton_text, manager_name, source_sheet, source_row, org_id) \
         VALUES ($1, $2, $3, $4, 'MFG', 'KND', 'PWR', '임대', 'spec', '3.5t', 'Manager Kim', 'seed', 1, $5)",
    )
    .bind(*branch.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(equipment_no)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_support_ticket(pool: &PgPool, branch: BranchId, requester: UserId, title: &str) {
    sqlx::query(
        "INSERT INTO support_tickets \
             (branch_id, origin, category, priority, status, title, body, requester_user_id, org_id) \
         VALUES ($1, 'INTERNAL', 'SYSTEM_BUG', 'LOW', 'OPEN', $2, 'Search branch scope', $3, $4)",
    )
    .bind(*branch.as_uuid())
    .bind(title)
    .bind(*requester.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
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

fn app_state(pool: PgPool, public_key_pem: String) -> AppState {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key_pem),
    ])
    .unwrap();
    AppState::new(config, DatabaseDependency::Postgres(pool)).unwrap()
}

struct Keys {
    private_pem: String,
    public_pem: String,
}

impl Keys {
    fn new() -> Self {
        let signing_key = SigningKey::random(&mut OsRng);
        Self {
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

    fn token(&self, user_id: UserId, org: &Uuid, roles: &[&str], branches: &[BranchId]) -> String {
        let issuer = JwtIssuer::from_es256_pem(
            JwtSettings {
                issuer: TEST_ISSUER.to_owned(),
                audience: TEST_AUDIENCE.to_owned(),
                access_token_ttl: Duration::minutes(15),
            },
            self.private_pem.as_bytes(),
            self.public_pem.as_bytes(),
        )
        .unwrap();
        issuer
            .issue_access_token(AccessTokenInput {
                subject: user_id,
                org_id: OrgId::from_uuid(*org),
                roles: roles.iter().map(|r| (*r).to_owned()).collect(),
                branches: branches.to_vec(),
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
