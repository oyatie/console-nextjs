#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Authenticated, runtime-role (`console_rt`) story for the notice board across
//! the ASSEMBLED app router (not per-crate routers): ES256 signature chain,
//! PBAC denial without leakage (draft 404/omission for non-managers, 403 only
//! on manager-only aggregates), branch-scoped audience snapshot + 수령확인,
//! cross-tenant isolation, and audit readback.

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

const ISSUER: &str = "console-platform-auth";
const AUDIENCE: &str = "console-api";
const NOTICES: &str = "/api/v1/notices";
const OTHER_ORG: Uuid = Uuid::from_u128(0x8404_8404_8404_8404_8404_8404_8404_8404);

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn board_notice_publishes_to_scoped_audience_with_ack_tracking(pool: PgPool) {
    let keys = Keys::generate();
    grant_runtime_role(&pool).await;
    let rt = runtime_role_pool(&pool).await;
    let knl = OrgId::knl();

    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, 'org-other', 'Org Other')")
        .bind(OTHER_ORG)
        .execute(&pool)
        .await
        .unwrap();

    let branch_a = seed_branch(&pool, *knl.as_uuid(), "창원지사").await;
    let branch_b = seed_branch(&pool, *knl.as_uuid(), "부산지사").await;
    let manager = seed_user(&pool, *knl.as_uuid(), "총무 매니저").await;
    let member_in = seed_user(&pool, *knl.as_uuid(), "창원 대원").await;
    let member_out = seed_user(&pool, *knl.as_uuid(), "부산 대원").await;
    let outsider = seed_user(&pool, OTHER_ORG, "타사 직원").await;
    join_branch(&pool, *knl.as_uuid(), member_in, branch_a).await;
    join_branch(&pool, *knl.as_uuid(), member_out, branch_b).await;

    let manager_token = keys.token(manager, knl, &["SUPER_ADMIN"], vec![]);
    let in_token = keys.token(member_in, knl, &["ADMIN"], vec![branch_a]);
    let out_token = keys.token(member_out, knl, &["ADMIN"], vec![branch_b]);
    let cross_token = keys.token(outsider, OrgId::from_uuid(OTHER_ORG), &["ADMIN"], vec![]);

    // Signature chain: no bearer and a token signed by a foreign key are both
    // rejected before any data access.
    let (status, _) = send(&rt, &keys, "GET", NOTICES, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "missing bearer");
    let rogue = Keys::generate();
    let forged = rogue.token(manager, knl, &["SUPER_ADMIN"], vec![]);
    let (status, _) = send(&rt, &keys, "GET", NOTICES, Some(&forged), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "foreign-key signature");

    // Manager drafts a branch-scoped 법정 통지.
    let (status, created) = send(
        &rt,
        &keys,
        "POST",
        NOTICES,
        Some(&manager_token),
        Some(json!({
            "title": "법정 안전교육 통지",
            "body": "창원지사 전 대원은 기한 내 수령확인 바랍니다.",
            "category": "legal",
            "audience": {"scope": "branches", "branch_ids": [branch_a]}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create draft: {created}");
    let notice_id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["audience_branches"][0]["name"], "창원지사");

    // PBAC denial without leakage: to a non-manager the draft does not exist
    // (404 + list omission), while manager-only aggregates answer 403.
    let get_one = format!("{NOTICES}/{notice_id}");
    let (status, leak) = send(&rt, &keys, "GET", &get_one, Some(&in_token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "draft must not leak: {leak}");
    let (status, list) = send(&rt, &keys, "GET", NOTICES, Some(&in_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(list.as_array().unwrap().is_empty(), "draft omitted: {list}");
    let (status, denied) = send(
        &rt,
        &keys,
        "PATCH",
        &get_one,
        Some(&in_token),
        Some(json!({"title": "탈취 시도"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "member PATCH: {denied}");
    let receipts_path = format!("{NOTICES}/{notice_id}/receipts");
    let (status, _) = send(&rt, &keys, "GET", &receipts_path, Some(&in_token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "member receipts drill");

    // Draft edit before publish is audited and effective.
    let (status, edited) = send(
        &rt,
        &keys,
        "PATCH",
        &get_one,
        Some(&manager_token),
        Some(json!({"category": "training"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "manager draft edit: {edited}");
    assert_eq!(edited["category"], "training");

    // Publish: NT- code issued, only branch A snapshotted.
    let publish_path = format!("{NOTICES}/{notice_id}/publish");
    let (status, published) = send(
        &rt,
        &keys,
        "POST",
        &publish_path,
        Some(&manager_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "publish: {published}");
    assert!(published["code"].as_str().unwrap().starts_with("NT-"));
    assert_eq!(published["progress"]["total"].as_i64(), Some(1));

    // 수령확인: the audience member acks (idempotent); the out-of-audience
    // member gets 404 — never a receipt, never a 403 that would confirm
    // membership semantics.
    let ack_path = format!("{NOTICES}/{notice_id}/ack");
    for _ in 0..2 {
        let (status, ack) = send(&rt, &keys, "POST", &ack_path, Some(&in_token), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "member ack: {ack}");
    }
    let (status, outsider_ack) = send(&rt, &keys, "POST", &ack_path, Some(&out_token), None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "out-of-audience ack: {outsider_ack}"
    );

    // The published notice is org-readable; the receipt/progress truth stays
    // tiered (member row: my_receipt yes, progress null).
    let (status, member_view) = send(&rt, &keys, "GET", &get_one, Some(&out_token), None).await;
    assert_eq!(status, StatusCode::OK, "published open read: {member_view}");
    assert!(member_view["my_receipt"].is_null());
    let (status, in_list) = send(&rt, &keys, "GET", NOTICES, Some(&in_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let row = &in_list.as_array().unwrap()[0];
    assert!(row["my_receipt"]["acknowledged_at"].is_string(), "{row}");
    assert!(row["progress"].is_null(), "{row}");

    // Manager tracking to completion: progress 1/1 + receipts drill.
    let (status, receipts) = send(
        &rt,
        &keys,
        "GET",
        &receipts_path,
        Some(&manager_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "receipts: {receipts}");
    assert_eq!(receipts["total"].as_i64(), Some(1));
    assert!(receipts["items"][0]["acknowledged_at"].is_string());

    // Cross-tenant isolation: the other org sees nothing, by id or list.
    let (status, cross_get) = send(&rt, &keys, "GET", &get_one, Some(&cross_token), None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cross-tenant get: {cross_get}"
    );
    let (status, cross_list) = send(&rt, &keys, "GET", NOTICES, Some(&cross_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(cross_list.as_array().unwrap().is_empty(), "{cross_list}");

    // Audit readback: every lifecycle mutation left its audited trace.
    for action in [
        "notice.create_draft",
        "notice.update_draft",
        "notice.publish",
        "notice.publish_recipients",
        "notice.acknowledge",
    ] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE target_id = $1 AND action = $2",
        )
        .bind(&notice_id)
        .bind(action)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(count >= 1, "missing audit action {action}");
    }
    let ack_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE target_id = $1 AND action = 'notice.acknowledge' AND actor = $2",
    )
    .bind(&notice_id)
    .bind(member_in.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(ack_audits, 2, "both ack attempts audited to the recipient");
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

    fn token(&self, user: UserId, org: OrgId, roles: &[&str], branches: Vec<BranchId>) -> String {
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
            roles: roles.iter().map(|role| (*role).to_owned()).collect(),
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
}

/// The tables 0031 predates and 0162 relies on production default-privileges
/// for; in `#[sqlx::test]` the migrations run as the test superuser, so the
/// `ALTER DEFAULT PRIVILEGES FOR ROLE console_app` auto-grant never fires and the
/// runtime grants must be issued explicitly (same list as the crate tests).
async fn grant_runtime_role(owner: &PgPool) {
    for grant in [
        "GRANT SELECT, INSERT, UPDATE ON notices TO console_rt",
        "GRANT SELECT, INSERT, UPDATE ON notice_receipts TO console_rt",
        "GRANT SELECT, INSERT, UPDATE ON notifications TO console_rt",
        "GRANT SELECT, INSERT, UPDATE ON object_code_counters TO console_rt",
        "GRANT SELECT ON object_types TO console_rt",
    ] {
        sqlx::query(grant).execute(owner).await.unwrap();
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
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request_body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        }
        None => Body::empty(),
    };
    let response = build_router(app_state(pool.clone(), keys.public_pem.clone()).unwrap())
        .oneshot(builder.body(request_body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
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

async fn seed_branch(pool: &PgPool, org: Uuid, name: &str) -> BranchId {
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("region-{name}"))
            .bind(org)
            .fetch_one(pool)
            .await
            .unwrap();
    BranchId::from_uuid(
        sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(region)
        .bind(name)
        .bind(org)
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}

async fn seed_user(pool: &PgPool, org: Uuid, name: &str) -> UserId {
    let user = UserId::new();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, org_id, is_active) VALUES ($1, $2, $3, $4, true)",
    )
    .bind(user.as_uuid())
    .bind(format!("{name} {}", Uuid::new_v4()))
    .bind(Vec::from(["ADMIN"]))
    .bind(org)
    .execute(pool)
    .await
    .unwrap();
    user
}

async fn join_branch(pool: &PgPool, org: Uuid, user: UserId, branch: BranchId) {
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(user.as_uuid())
        .bind(branch.as_uuid())
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
}
