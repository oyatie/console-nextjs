#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const CONSOLE_ROLLOUT_FLAG: &str = "console_carbon_copy";
const CONSOLE_KILL_SWITCH_FLAG: &str = "console_legacy_kill_switch";

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn console_kill_switch_forces_legacy_for_all_users_and_audits(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Console Rollout Region", "Console Rollout Branch")
        .await
        .unwrap();
    let admin_id = UserId::new();
    let member_id = UserId::new();
    seed_user_with_branch(&pool, admin_id, "SUPER_ADMIN", branch_id)
        .await
        .unwrap();
    seed_user_with_branch(&pool, member_id, "MEMBER", branch_id)
        .await
        .unwrap();
    seed_org_runtime_flag(&pool, CONSOLE_ROLLOUT_FLAG, true, admin_id)
        .await
        .unwrap();
    seed_user_console_opt_in(&pool, member_id, true)
        .await
        .unwrap();

    let admin_token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        admin_id,
        vec!["SUPER_ADMIN".to_owned()],
        vec![branch_id],
    )
    .await
    .unwrap();
    let member_token = issue_token(
        &pool,
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        member_id,
        vec!["MEMBER".to_owned()],
        vec![branch_id],
    )
    .await
    .unwrap();
    let service = build_router(app_state(pool.clone(), public_key_pem).unwrap());

    let before = get_json(
        service.clone(),
        "/api/v1/console/rollout",
        &member_token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(before["org_rollout_enabled"], true);
    assert_eq!(before["user_opted_in"], true);
    assert_eq!(before["legacy_kill_switch_enabled"], false);
    assert_eq!(before["effective_new_console"], true);
    assert_eq!(before["effective_route"], "new_console");
    assert_eq!(before["effective_route_for_opted_in_user"], "new_console");
    assert_eq!(before["effective_route_for_opted_out_user"], "legacy");

    let activated = post_json(
        service.clone(),
        "/api/v1/console/kill-switch",
        &admin_token,
        json!({
            "enabled": true,
            "reason": "incident rollback: route every org user to the legacy console"
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(activated["legacy_kill_switch_enabled"], true);
    assert_eq!(activated["effective_route_for_opted_in_user"], "legacy");
    assert_eq!(activated["effective_route_for_opted_out_user"], "legacy");
    assert_eq!(activated["overrides_individual_toggles"], true);

    let after = get_json(
        service,
        "/api/v1/console/rollout",
        &member_token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(after["org_rollout_enabled"], true);
    assert_eq!(after["user_opted_in"], true);
    assert_eq!(after["legacy_kill_switch_enabled"], true);
    assert_eq!(after["effective_new_console"], false);
    assert_eq!(after["effective_route"], "legacy");
    assert_eq!(after["effective_route_for_opted_in_user"], "legacy");
    assert_eq!(after["effective_route_for_opted_out_user"], "legacy");

    let flag_row: (bool, Option<String>, Option<uuid::Uuid>) = sqlx::query_as(
        "SELECT enabled, rollout_note, set_by FROM org_runtime_flags WHERE org_id = $1 AND flag_key = $2",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(CONSOLE_KILL_SWITCH_FLAG)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(flag_row.0);
    assert_eq!(
        flag_row.1.as_deref(),
        Some("incident rollback: route every org user to the legacy console")
    );
    assert_eq!(flag_row.2, Some(*admin_id.as_uuid()));

    let audit: Value = sqlx::query_scalar(
        "SELECT after_snap FROM audit_events WHERE org_id = $1 AND actor = $2 AND action = 'console.kill_switch' AND target_type = 'org_runtime_flag' AND target_id = $3",
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(*admin_id.as_uuid())
    .bind(CONSOLE_KILL_SWITCH_FLAG)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit["enabled"], true);
    assert_eq!(audit["effective_route_for_opted_in_user"], "legacy");
    assert_eq!(audit["effective_route_for_opted_out_user"], "legacy");
}

async fn get_json(service: axum::Router, uri: &str, token: &str, expected: StatusCode) -> Value {
    let response = service
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), expected);
    body_json(response).await
}

async fn post_json(
    service: axum::Router,
    uri: &str,
    token: &str,
    body: Value,
    expected: StatusCode,
) -> Value {
    let response = service
        .oneshot(
            Request::builder()
                .uri(uri)
                .method("POST")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), expected);
    body_json(response).await
}

async fn body_json(response: http::Response<Body>) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
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
        .bind(format!("Console User {role}"))
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

async fn seed_org_runtime_flag(
    pool: &PgPool,
    flag_key: &str,
    enabled: bool,
    actor: UserId,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO org_runtime_flags (org_id, flag_key, enabled, rollout_note, set_by)
        VALUES ($1, $2, $3, 'test rollout enabled', $4)
        "#,
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(flag_key)
    .bind(enabled)
    .bind(*actor.as_uuid())
    .execute(pool)
    .await?;
    Ok(())
}

async fn seed_user_console_opt_in(
    pool: &PgPool,
    user_id: UserId,
    opt_in: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO user_feature_preferences (
            org_id, user_id, feature_key, preferences_json, schema_version
        ) VALUES ($1, $2, 'console_rollout', $3, 1)
        "#,
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(*user_id.as_uuid())
    .bind(json!({ "opt_in": opt_in }))
    .execute(pool)
    .await?;
    Ok(())
}
