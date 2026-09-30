#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use console_financial_adapter_postgres::PgFinancialStore;
use console_financial_rest::{FINANCIAL_PURCHASE_REQUESTS_PATH, FinancialRestState, router};
use console_kernel_core::{BranchId, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_test_support::runtime_role_pool;
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

const ISSUER: &str = "console-platform-auth";
const AUDIENCE: &str = "console-api";
const ORG_B: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);

struct Keys {
    private_pem: String,
    public_pem: String,
}

fn keys() -> Keys {
    let key = SigningKey::random(&mut OsRng);
    Keys {
        private_pem: key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
        public_pem: key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap(),
    }
}

fn bearer(keys: &Keys, user: UserId, org: OrgId, role: &str) -> String {
    JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: ISSUER.to_owned(),
            audience: AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
    )
    .unwrap()
    .issue_access_token(AccessTokenInput {
        subject: user,
        org_id: org,
        roles: vec![role.to_owned()],
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

fn app(pool: PgPool, keys: &Keys) -> axum::Router {
    let verifier = JwtVerifier::from_es256_public_pem(
        JwtSettings {
            issuer: ISSUER.to_owned(),
            audience: AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        keys.public_pem.as_bytes(),
    )
    .unwrap();
    router(FinancialRestState::new(
        PgFinancialStore::new(pool),
        Some(verifier),
    ))
}

async fn get(service: axum::Router, uri: &str, token: &str) -> (StatusCode, Value) {
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
    (
        status,
        serde_json::from_slice(&body).unwrap_or_else(|_| json!({})),
    )
}

async fn seed_org(pool: &PgPool, org: Uuid, tag: &str) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
        .bind(org)
        .bind(format!("purchase-list-{tag}"))
        .bind(format!("Purchase List {tag}"))
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_branch(pool: &PgPool, org: Uuid, tag: &str) -> BranchId {
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("purchase-list-region-{tag}-{}", Uuid::new_v4()))
            .bind(org)
            .fetch_one(pool)
            .await
            .unwrap();
    BranchId::from_uuid(
        sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(region)
        .bind(format!("purchase-list-branch-{tag}-{}", Uuid::new_v4()))
        .bind(org)
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}

async fn seed_user(pool: &PgPool, org: Uuid, role: &str) -> UserId {
    let user = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user.as_uuid())
        .bind(format!("purchase-list-user-{}", user.as_uuid()))
        .bind(vec![role.to_owned()])
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
    user
}

async fn seed_request(
    pool: &PgPool,
    org: Uuid,
    branch: BranchId,
    requester: UserId,
    status: &str,
    created_at: OffsetDateTime,
) {
    sqlx::query(
        "INSERT INTO financial_purchase_requests (id, branch_id, equipment_id, statement_evidence_id, purchase_type, vendor_name, amount_won, memo, status, requested_by, depreciation_method, useful_life_months, residual_rate_bps, declining_balance_rate_bps, management_fee_rate_bps, profit_rate_bps, floor_negative_quote_residual, executive_threshold_won, created_at, updated_at, org_id) VALUES ($1, $2, NULL, NULL, 'LEGACY_MANUAL', 'List test vendor', 1000, 'List test request', $3, $4, 'STRAIGHT_LINE', 60, 1000, 2000, 1000, 500, true, 2000000, $5, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(*branch.as_uuid())
    .bind(status)
    .bind(*requester.as_uuid())
    .bind(created_at)
    .bind(org)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn purchase_request_queue_is_mounted_authorized_strict_and_rls_scoped(pool: PgPool) {
    let keys = keys();
    let org_a = OrgId::knl();
    seed_org(&pool, *org_a.as_uuid(), "a").await;
    seed_org(&pool, ORG_B, "b").await;
    let branch_a = seed_branch(&pool, *org_a.as_uuid(), "a").await;
    let branch_b = seed_branch(&pool, ORG_B, "b").await;
    let admin_a = seed_user(&pool, *org_a.as_uuid(), "SUPER_ADMIN").await;
    let member_a = seed_user(&pool, *org_a.as_uuid(), "MEMBER").await;
    let admin_b = seed_user(&pool, ORG_B, "SUPER_ADMIN").await;
    seed_request(
        &pool,
        *org_a.as_uuid(),
        branch_a,
        admin_a,
        "STATEMENT_ATTACHED",
        OffsetDateTime::UNIX_EPOCH,
    )
    .await;
    seed_request(
        &pool,
        *org_a.as_uuid(),
        branch_a,
        admin_a,
        "REQUEST_SUBMITTED",
        OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
    )
    .await;
    seed_request(
        &pool,
        ORG_B,
        branch_b,
        admin_b,
        "EXECUTED",
        OffsetDateTime::UNIX_EPOCH,
    )
    .await;

    let service = app(runtime_role_pool(&pool).await, &keys);
    let admin_a_token = bearer(&keys, admin_a, org_a, "SUPER_ADMIN");
    let member_a_token = bearer(&keys, member_a, org_a, "MEMBER");
    let admin_b_token = bearer(&keys, admin_b, OrgId::from_uuid(ORG_B), "SUPER_ADMIN");

    let (status, page) = get(service.clone(), &format!("{FINANCIAL_PURCHASE_REQUESTS_PATH}?branch_id={branch_a}&status=STATEMENT_ATTACHED&status=REQUEST_SUBMITTED&limit=1&offset=0"), &admin_a_token).await;
    assert_eq!(status, StatusCode::OK, "{page:?}");
    assert_eq!(page["total"], 2);
    assert_eq!(page["limit"], 1);
    assert_eq!(page["offset"], 0);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["status"], "REQUEST_SUBMITTED");

    let (status, page) = get(
        service.clone(),
        &format!(
            "{FINANCIAL_PURCHASE_REQUESTS_PATH}?branch_id={branch_a}&status=STATEMENT_ATTACHED&status=REQUEST_SUBMITTED&limit=1&offset=1"
        ),
        &admin_a_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page:?}");
    assert_eq!(page["total"], 2);
    assert_eq!(page["offset"], 1);
    assert_eq!(page["items"][0]["status"], "STATEMENT_ATTACHED");

    let (status, page) = get(
        service.clone(),
        &format!(
            "{FINANCIAL_PURCHASE_REQUESTS_PATH}?branch_id={branch_a}&status[]=REQUEST_SUBMITTED"
        ),
        &admin_a_token,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{page:?}");

    let (status, page) = get(
        service.clone(),
        &format!("{FINANCIAL_PURCHASE_REQUESTS_PATH}?branch_id={branch_a}"),
        &member_a_token,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{page:?}");

    let (status, page) = get(
        service,
        &format!("{FINANCIAL_PURCHASE_REQUESTS_PATH}?branch_id={branch_a}"),
        &admin_b_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page:?}");
    assert_eq!(page["total"], 0, "cross-tenant rows must be hidden by RLS");
}
