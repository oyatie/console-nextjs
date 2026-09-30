#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{
    AccessScope, AccessScopeLevel, AuditAction, AuditClassification, AuditEvent,
    AuditRequestContext, BranchId, OrgId, ScopeNodeId, TraceContext, UserId,
};
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

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn admin_reads_only_branch_scoped_audits_and_read_access_is_audited(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let user_id = UserId::new();
    let branch_id = seed_branch(&pool, "Admin Region", "Admin Branch")
        .await
        .unwrap();
    seed_user_with_branch(&pool, user_id, "ADMIN", branch_id)
        .await
        .unwrap();
    let token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        user_id,
        vec!["ADMIN".to_owned()],
        vec![branch_id],
    )
    .await
    .unwrap();
    let other_branch = seed_branch(&pool, "Other Region", "Other Branch")
        .await
        .unwrap();
    insert_audit(&pool, Some(user_id), "work_order", "wo-in-scope", branch_id)
        .await
        .unwrap();
    insert_audit(
        &pool,
        Some(user_id),
        "work_order",
        "wo-out-of-scope",
        other_branch,
    )
    .await
    .unwrap();
    let service = build_router(app_state(pool.clone(), public_key_pem).unwrap());

    let response = service
        .oneshot(
            Request::builder()
                .uri("/api/audit?target_type=work_order&limit=10&offset=0")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["target_id"], "wo-in-scope");
    assert_eq!(items[0]["branch_id"], branch_id.to_string());
    assert_eq!(items[0]["ip"], "203.0.113.10");
    assert_eq!(items[0]["user_agent"], "Maintenance Console/1.0");
    assert_eq!(items[0]["auth_method"], "passkey");
    assert_eq!(items[0]["device"], "desktop-web");
    assert_eq!(items[0]["classification_badges"][0], "민감정보");
    assert_eq!(items[0]["classification_badges"][1], "대외비");
    assert_eq!(items[0]["anomaly"], true);
    assert_eq!(items[0]["reason"], "manager override");

    let read_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE actor = $1 AND action = 'audit.read'",
    )
    .bind(*user_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(read_count, 1);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn audit_read_rechecks_live_branch_membership_after_token_issue(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch = seed_branch(&pool, "Revoked Audit Region", "Revoked Audit Branch")
        .await
        .unwrap();
    let user = UserId::new();
    seed_user_with_branch(&pool, user, "ADMIN", branch)
        .await
        .unwrap();
    insert_audit(
        &pool,
        Some(user),
        "work_order",
        "revoked-branch-audit",
        branch,
    )
    .await
    .unwrap();

    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private_pem.as_bytes(),
        public_pem.as_bytes(),
    )
    .unwrap();
    let runtime = runtime_role_pool(&pool).await;
    let token = console_platform_test_support::issue_session_token(
        &runtime,
        &issuer,
        AccessTokenInput {
            subject: user,
            org_id: OrgId::knl(),
            roles: vec!["ADMIN".to_owned()],
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
        },
        None,
        vec![],
    )
    .await;
    let service = build_router(app_state(runtime, public_pem).unwrap());
    let request = || {
        Request::builder()
            .uri("/api/audit?target_type=work_order")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    };
    let before = service.clone().oneshot(request()).await.unwrap();
    assert_eq!(before.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(before.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["items"][0]["target_id"], "revoked-branch-audit");

    let removed =
        sqlx::query("DELETE FROM user_branches WHERE user_id=$1 AND branch_id=$2 AND org_id=$3")
            .bind(*user.as_uuid())
            .bind(*branch.as_uuid())
            .bind(*OrgId::knl().as_uuid())
            .execute(&pool)
            .await
            .unwrap();
    assert_eq!(removed.rows_affected(), 1);
    let after = service.oneshot(request()).await.unwrap();
    assert_eq!(
        after.status(),
        StatusCode::FORBIDDEN,
        "an unchanged signed token must not restore a revoked branch's audit authority"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn branch_scoped_super_admin_cannot_attest_whole_company(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch = seed_branch(
        &pool,
        "Scoped Attestation Region",
        "Scoped Attestation Branch",
    )
    .await
    .unwrap();
    let user = UserId::new();
    seed_user_with_branch(&pool, user, "SUPER_ADMIN", branch)
        .await
        .unwrap();
    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private_pem.as_bytes(),
        public_pem.as_bytes(),
    )
    .unwrap();
    let token = console_platform_test_support::issue_session_token(
        &pool,
        &issuer,
        AccessTokenInput {
            subject: user,
            org_id: OrgId::knl(),
            roles: vec!["SUPER_ADMIN".to_owned()],
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
        },
        Some(AccessScope::new(
            AccessScopeLevel::Branch,
            ScopeNodeId::from_uuid(*branch.as_uuid()),
        )),
        Vec::new(),
    )
    .await;
    let service = build_router(app_state(runtime_role_pool(&pool).await, public_pem).unwrap());
    let response = service
        .oneshot(
            Request::builder()
                .uri("/api/v1/audit/attestation")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn mechanic_role_is_denied_audit_read(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Denied Region", "Denied Branch")
        .await
        .unwrap();
    let user_id = UserId::new();
    seed_user_with_branch(&pool, user_id, "MECHANIC", branch_id)
        .await
        .unwrap();
    let token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        user_id,
        vec!["MECHANIC".to_owned()],
        vec![branch_id],
    )
    .await
    .unwrap();
    let service = build_router(app_state(pool.clone(), public_key_pem).unwrap());

    let response = service
        .oneshot(
            Request::builder()
                .uri("/api/audit")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let read_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE actor = $1 AND action = 'audit.read'",
    )
    .bind(*user_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(read_count, 0);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn audit_attestation_requires_org_wide_audit_authority(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Attestation Region", "Attestation Branch")
        .await
        .unwrap();
    let admin_id = UserId::new();
    seed_user_with_branch(&pool, admin_id, "ADMIN", branch_id)
        .await
        .unwrap();
    let admin_token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        admin_id,
        vec!["ADMIN".to_owned()],
        vec![branch_id],
    )
    .await
    .unwrap();
    let service = build_router(app_state(pool.clone(), public_key_pem.clone()).unwrap());

    let response = service
        .oneshot(
            Request::builder()
                .uri("/api/v1/audit/attestation")
                .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "branch-scoped ADMIN can read branch-filtered /api/audit, but must not get a whole-tenant chain attestation"
    );

    let admin_no_branch_token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        admin_id,
        vec!["ADMIN".to_owned()],
        Vec::new(),
    )
    .await
    .unwrap();
    let service = build_router(app_state(pool.clone(), public_key_pem.clone()).unwrap());

    let response = service
        .oneshot(
            Request::builder()
                .uri("/api/v1/audit/attestation")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {admin_no_branch_token}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "built-in ADMIN without branch claims is not upgraded into org-wide attestation authority"
    );

    let super_admin_id = UserId::new();
    seed_user_with_branch(&pool, super_admin_id, "SUPER_ADMIN", branch_id)
        .await
        .unwrap();
    let super_admin_token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        super_admin_id,
        vec!["SUPER_ADMIN".to_owned()],
        Vec::new(),
    )
    .await
    .unwrap();
    let service = build_router(app_state(pool.clone(), public_key_pem).unwrap());

    let response = service
        .oneshot(
            Request::builder()
                .uri("/api/v1/audit/attestation")
                .header(header::AUTHORIZATION, format!("Bearer {super_admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["org_id"], OrgId::knl().as_uuid().to_string());
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "ok");
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn target_id_filter_isolates_one_object_and_trace_id_correlates_across_objects(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let user_id = UserId::new();
    let branch_id = seed_branch(&pool, "Audit Region", "Audit Branch")
        .await
        .unwrap();
    seed_user_with_branch(&pool, user_id, "ADMIN", branch_id)
        .await
        .unwrap();
    let token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        user_id,
        vec!["ADMIN".to_owned()],
        vec![branch_id],
    )
    .await
    .unwrap();

    // A shared trace threads two different objects (the approval that authorized
    // work order A, then A itself); a second work order B is on its own trace.
    let shared_trace = TraceContext::generate();
    insert_audit_with_trace(
        &pool,
        Some(user_id),
        "approval",
        "approval-1",
        branch_id,
        &shared_trace,
    )
    .await
    .unwrap();
    insert_audit_with_trace(
        &pool,
        Some(user_id),
        "work_order",
        "wo-a",
        branch_id,
        &shared_trace,
    )
    .await
    .unwrap();
    insert_audit_with_trace(
        &pool,
        Some(user_id),
        "work_order",
        "wo-b",
        branch_id,
        &TraceContext::generate(),
    )
    .await
    .unwrap();

    let service = build_router(app_state(pool.clone(), public_key_pem.clone()).unwrap());

    // target_id filter returns ONLY that object's events (not the sibling wo-b).
    let response = service
        .oneshot(
            Request::builder()
                .uri("/api/audit?target_type=work_order&target_id=wo-a")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "target_id filter isolates one object");
    assert_eq!(items[0]["target_id"], "wo-a");

    // trace_id filter correlates events ACROSS objects (approval-1 + wo-a share
    // the trace; wo-b does not appear).
    let service = build_router(app_state(pool.clone(), public_key_pem).unwrap());
    let response = service
        .oneshot(
            Request::builder()
                .uri(format!("/api/audit?trace_id={}", shared_trace.trace_id()))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let items = json["items"].as_array().unwrap();
    let target_ids: Vec<&str> = items
        .iter()
        .map(|item| item["target_id"].as_str().unwrap())
        .collect();
    assert!(
        target_ids.contains(&"approval-1") && target_ids.contains(&"wo-a"),
        "trace_id correlates across objects: {target_ids:?}"
    );
    assert!(
        !target_ids.contains(&"wo-b"),
        "an unrelated trace's object must not appear: {target_ids:?}"
    );
}

/// Regression for the #206 audit finding: the `/api/audit` read path never
/// armed `app.current_org`, so as the real `console_rt` role the FORCE-RLS
/// `audit_events` SELECT failed CLOSED (zero rows). A SUPER_ADMIN has
/// `BranchScope::All`, so the branch filter is `WHERE TRUE` and ONLY the org
/// GUC/RLS separates tenants — making this the sharpest proof of the fix.
///
/// RED (before `audit_read_event` stamps `.with_org`): armed=none ⇒ zero rows,
/// so the KNL row is invisible (the `assert_eq!(items.len(), 1)` fails).
/// GREEN: the KNL admin sees its own KNL audit row and NOT the other tenant's.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn super_admin_audit_read_arms_org_and_isolates_cross_org_as_runtime_role(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let user_id = UserId::new();
    let knl_branch = seed_branch(&pool, "KNL Region", "KNL Branch")
        .await
        .unwrap();
    seed_user_with_branch(&pool, user_id, "SUPER_ADMIN", knl_branch)
        .await
        .unwrap();
    insert_audit_for_org(
        &pool,
        Some(user_id),
        "work_order",
        "wo-knl",
        knl_branch,
        OrgId::knl(),
    )
    .await
    .unwrap();

    // A row owned by a DIFFERENT tenant, on a DIFFERENT branch. Because the
    // SUPER_ADMIN's branch filter is `WHERE TRUE`, only the org GUC/RLS can keep
    // this out of the KNL admin's view.
    let other_org = seed_org(&pool, "other-tenant").await.unwrap();
    let other_branch = seed_branch_for_org(&pool, other_org, "Other Region", "Other Branch")
        .await
        .unwrap();
    insert_audit_for_org(
        &pool,
        None,
        "work_order",
        "wo-other",
        other_branch,
        other_org,
    )
    .await
    .unwrap();

    let token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        user_id,
        vec!["SUPER_ADMIN".to_owned()],
        Vec::new(),
    )
    .await
    .unwrap();

    // Build the router off the `console_rt` pool so FORCE RLS actually applies —
    // an owner/BYPASSRLS pool would mask the unarmed-GUC break.
    let service =
        build_router(app_state(runtime_role_pool(&pool).await, public_key_pem.clone()).unwrap());
    let response = service
        .oneshot(
            Request::builder()
                .uri("/api/audit?target_type=work_order&limit=10&offset=0")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let items = json["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "armed KNL admin must see its own audit row (unarmed GUC would fail closed to zero rows): {items:?}"
    );
    assert_eq!(items[0]["target_id"], "wo-knl");
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

async fn seed_org(pool: &PgPool, slug: &str) -> Result<OrgId, sqlx::Error> {
    let org_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO organizations (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(slug)
            .bind(format!("Org {slug}"))
            .fetch_one(pool)
            .await?;
    Ok(OrgId::from_uuid(org_id))
}

async fn seed_branch_for_org(
    pool: &PgPool,
    org: OrgId,
    region_name: &str,
    branch_name: &str,
) -> Result<BranchId, sqlx::Error> {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(region_name)
            .bind(*org.as_uuid())
            .fetch_one(pool)
            .await?;
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(branch_name)
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await?;
    Ok(BranchId::from_uuid(branch_id))
}

async fn insert_audit_for_org(
    pool: &PgPool,
    actor: Option<UserId>,
    target_type: &str,
    target_id: &str,
    branch_id: BranchId,
    org: OrgId,
) -> Result<(), Box<dyn std::error::Error>> {
    let event = AuditEvent::new(
        actor,
        AuditAction::new("work_order.view")?,
        target_type,
        target_id,
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_branch(branch_id);
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor, action, target_type, target_id,
            branch_id, trace_id, span_id, occurred_at, org_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(*event.id.as_uuid())
    .bind(actor.map(|user_id| *user_id.as_uuid()))
    .bind(event.action.as_str())
    .bind(event.target_type)
    .bind(event.target_id)
    .bind(*branch_id.as_uuid())
    .bind(event.trace.trace_id())
    .bind(event.trace.span_id())
    .bind(event.occurred_at)
    .bind(*org.as_uuid())
    .execute(pool)
    .await?;
    Ok(())
}

async fn issue_token(
    pool: &PgPool,
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    roles: Vec<String>,
    branches: Vec<BranchId>,
) -> Result<String, Box<dyn std::error::Error>> {
    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private_key_pem,
        public_key_pem,
    )?;

    Ok(console_platform_test_support::issue_session_token(
        pool,
        &issuer,
        AccessTokenInput {
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
        },
        None,
        Vec::new(),
    )
    .await)
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

async fn seed_branch(
    pool: &PgPool,
    region_name: &str,
    branch_name: &str,
) -> Result<BranchId, sqlx::Error> {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(region_name)
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await?;
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(branch_name)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await?;
    Ok(BranchId::from_uuid(branch_id))
}

async fn seed_user_with_branch(
    pool: &PgPool,
    user_id: UserId,
    role: &str,
    branch_id: BranchId,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("User {role}"))
        .bind(Vec::from([role]))
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await?;
    Ok(())
}

async fn insert_audit(
    pool: &PgPool,
    actor: Option<UserId>,
    target_type: &str,
    target_id: &str,
    branch_id: BranchId,
) -> Result<(), Box<dyn std::error::Error>> {
    insert_audit_with_trace(
        pool,
        actor,
        target_type,
        target_id,
        branch_id,
        &TraceContext::generate(),
    )
    .await
}

async fn insert_audit_with_trace(
    pool: &PgPool,
    actor: Option<UserId>,
    target_type: &str,
    target_id: &str,
    branch_id: BranchId,
    trace: &TraceContext,
) -> Result<(), Box<dyn std::error::Error>> {
    let event = AuditEvent::new(
        actor,
        AuditAction::new("work_order.view")?,
        target_type,
        target_id,
        trace.clone(),
        OffsetDateTime::now_utc(),
    )
    .with_branch(branch_id)
    .with_org(OrgId::knl())
    .with_request_context(AuditRequestContext {
        ip: Some("203.0.113.10".to_owned()),
        user_agent: Some("Maintenance Console/1.0".to_owned()),
        auth_method: Some("passkey".to_owned()),
        device: Some("desktop-web".to_owned()),
    })
    .with_classification(AuditClassification {
        badges: Some(vec!["민감정보".to_owned(), "대외비".to_owned()]),
        anomaly: Some(true),
        reason: Some("manager override".to_owned()),
    });
    let actor_uuid = actor.map(|user_id| *user_id.as_uuid());
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor, action, target_type, target_id,
            branch_id, trace_id, span_id, occurred_at, org_id,
            ip, user_agent, auth_method, device,
            classification_badges, anomaly, reason
        ) VALUES (
            $1, $2, $3, $4, $5,
            $6, $7, $8, $9, $10,
            $11, $12, $13, $14,
            $15, $16, $17
        )
        "#,
    )
    .bind(*event.id.as_uuid())
    .bind(actor_uuid)
    .bind(event.action.as_str())
    .bind(event.target_type)
    .bind(event.target_id)
    .bind(*branch_id.as_uuid())
    .bind(event.trace.trace_id())
    .bind(event.trace.span_id())
    .bind(event.occurred_at)
    .bind(*event.org_id.expect("test event has org").as_uuid())
    .bind(event.request_context.ip.as_deref())
    .bind(event.request_context.user_agent.as_deref())
    .bind(event.request_context.auth_method.as_deref())
    .bind(event.request_context.device.as_deref())
    .bind(event.classification.badges.clone())
    .bind(event.classification.anomaly)
    .bind(event.classification.reason.as_deref())
    .execute(pool)
    .await?;
    Ok(())
}
