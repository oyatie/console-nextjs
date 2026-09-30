#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_financial_adapter_postgres::PgFinancialStore;
use console_financial_application::{
    CreatePurchaseRequestCommand, FinancialConfigSnapshot, PrepareExpenditureCommand,
    PurchaseApprovalCommand, PurchaseRequestLineInput, PurchaseSubmitCommand, PurchaseType,
};
use console_financial_domain::DepreciationMethod;
use console_kernel_core::{
    BranchId, EquipmentId, EvidenceId, OrgId, PurchaseRequestId, TraceContext, UserId, WorkOrderId,
};
use console_platform_provisioning::BootstrapCredentialStore;
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::net::SocketAddr;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use url::Url;
use uuid::Uuid;
use webauthn_authenticator_rs::prelude::WebauthnAuthenticator;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse};

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";
const TEST_ORIGIN: &str = "https://auth.example.com";

#[derive(Debug, Deserialize)]
struct RegisterStartResponse {
    ceremony_id: Uuid,
    challenge: CreationChallengeResponse,
}

#[derive(Debug, Deserialize)]
struct RegisterFinishResponse {
    credential_id: String,
}

#[derive(Debug, Deserialize)]
struct LoginStartResponse {
    ceremony_id: Uuid,
    challenge: RequestChallengeResponse,
}

#[derive(Debug, Deserialize)]
struct TokenPairResponse {
    access_token: String,
    /// Present (body transport, mobile) or null (cookie transport, web).
    refresh_token: Option<String>,
    #[serde(default)]
    requires_passkey_setup: bool,
}

#[derive(Debug, Deserialize)]
struct DeviceLoginStartResponse {
    approve_url: String,
    poll_token: String,
}

#[derive(Debug, Deserialize)]
struct OtpRedeemResponse {
    access_token: String,
    /// Present (body transport, mobile) or null (cookie transport, web).
    refresh_token: Option<String>,
    requires_passkey_setup: bool,
}

#[derive(Debug, Deserialize)]
struct AdminIssueOtpResponse {
    otp: String,
    user_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct AdminCredentialResetResponse {
    otp: String,
    user_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct PrivacyConsentStatusResponse {
    policy_version: String,
    accepted: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    accepted_at: Option<OffsetDateTime>,
}

/// End-to-end: an admin issues a one-time code; the new user signs in for the
/// FIRST time by redeeming it (minting a session, flagged for passkey setup),
/// enrolls a passkey from that authenticated session, and then signs in again
/// usernamelessly (discoverable) with no user_id. Refresh reuse still revokes the
/// family.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn otp_first_signin_then_passkey_enrollment_then_usernameless_login(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Auth Region", "Auth Branch").await;
    // The admin who issues codes.
    let admin_id =
        seed_user_with_branch(&pool, "Branch Admin", "010-4000-0000", "ADMIN", branch_id).await;
    // The pre-provisioned new user who will do their first sign-in via OTP.
    let new_user_id =
        seed_user_with_branch(&pool, "New User", "010-4000-0001", "MECHANIC", branch_id).await;
    seed_equipment(&pool, branch_id, "290").await;

    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );

    // The admin first signs in (cold start in this test uses a directly-issued
    // OTP for the admin) and enrolls a passkey so it can call admin endpoints.
    let admin_access = admin_session_via_otp(&service, &pool, admin_id).await;

    // Admin issues a one-time code for the new user.
    let issued: AdminIssueOtpResponse = post_json(
        service.clone(),
        "/api/v1/auth/admin/otp/issue",
        Some(&admin_access),
        json!({ "user_id": new_user_id.as_uuid(), "branch_id": branch_id }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(&issued.user_id, new_user_id.as_uuid());
    assert_eq!(issued.otp.chars().count(), 8, "issued OTP must be 8 chars");

    // FIRST SIGN-IN: the new user redeems the OTP -> session + setup flag.
    let redeem: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issued.otp }),
        StatusCode::OK,
    )
    .await;
    assert!(
        redeem.requires_passkey_setup,
        "a zero-passkey user must be flagged for passkey setup"
    );
    assert!(
        !redeem.access_token.is_empty()
            && redeem
                .refresh_token
                .as_ref()
                .is_some_and(|token| !token.is_empty()),
        "an OTP redeem is a first sign-in: it must mint a full session (access + refresh tokens)"
    );

    // INITIAL SETTINGS: the OTP-signed-in user enrolls a passkey via the
    // authenticated register path (no bootstrap token).
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential_id = enroll_passkey(&service, &mut authenticator, &redeem.access_token).await;

    // A second OTP redeem is rejected: single-use.
    let replay = post_raw(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issued.otp }),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);

    // USERNAMELESS SIGN-IN: no user_id, discoverable assertion -> token pair.
    let first_tokens = usernameless_login(&service, &mut authenticator, &credential_id).await;
    // Initial authentication must bind both issued credentials to the same
    // real family, before any refresh has had a chance to repair the binding.
    let verifier = console_platform_auth::JwtVerifier::from_es256_public_pem(
        console_platform_auth::JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(10),
        },
        public_key_pem.as_bytes(),
    )
    .unwrap();
    for (access, refresh) in [
        (&redeem.access_token, redeem.refresh_token.as_ref().unwrap()),
        (
            &first_tokens.access_token,
            first_tokens.refresh_token.as_ref().unwrap(),
        ),
    ] {
        use sha2::{Digest, Sha256};
        let claims = serde_json::to_value(verifier.verify_access_token(access).unwrap()).unwrap();
        let family: Uuid = sqlx::query_scalar("SELECT family_id FROM auth_refresh_tokens WHERE token_hash=$1 AND user_id=$2 AND org_id=$3")
            .bind(Sha256::digest(refresh.as_bytes()).to_vec()).bind(*new_user_id.as_uuid())
            .bind(*OrgId::knl().as_uuid()).fetch_one(&pool).await.unwrap();
        assert_eq!(claims["session_family_id"], json!(family));
    }
    let work_order: Value = post_json(
        service.clone(),
        "/api/work-orders",
        Some(&first_tokens.access_token),
        json!({
            "branch_id": branch_id,
            "management_no": "#290",
            "symptom": "Hydraulic oil leak"
        }),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(work_order["status"], "RECEIVED");

    // Refresh rotation + reuse-detection still holds (mobile/body transport).
    let first_refresh = first_tokens
        .refresh_token
        .clone()
        .expect("body-transport login must return a refresh token");
    let rotated: TokenPairResponse = post_json(
        service.clone(),
        "/api/v1/auth/token/refresh",
        None,
        json!({ "refresh_token": first_refresh }),
        StatusCode::OK,
    )
    .await;
    assert_ne!(rotated.refresh_token, first_tokens.refresh_token);

    let reuse = post_raw(
        service.clone(),
        "/api/v1/auth/token/refresh",
        None,
        json!({ "refresh_token": first_refresh }),
    )
    .await;
    assert_eq!(reuse.status(), StatusCode::UNAUTHORIZED);

    assert_audit_count(&pool, "auth.otp.signin", 2).await; // admin + new user
    assert_audit_count(&pool, "auth.login", 1).await; // usernameless login
}

/// If the family deadline passes after rotation starts, JWT signing must fail
/// before the replacement is committed. Otherwise a 500 hides the only usable
/// refresh token and retrying the old one revokes the family as reuse.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn refresh_signing_failure_does_not_commit_undelivered_replacement(pool: PgPool) {
    use sha2::{Digest, Sha256};

    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch = seed_branch(&pool, "Refresh Deadline Region", "Refresh Deadline Branch").await;
    let user = seed_user_with_branch(
        &pool,
        "Refresh Deadline User",
        "010-4100-0094",
        "MECHANIC",
        branch,
    )
    .await;
    let service =
        build_router(app_state(pool.clone(), private_key_pem.to_string(), public_key_pem).unwrap());
    let otp = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            &pool,
            *user.as_uuid(),
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::hours(1),
        )
        .await
        .unwrap();
    let session: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": otp.token.as_str() }),
        StatusCode::OK,
    )
    .await;
    let refresh = session.refresh_token.unwrap();
    let hash = Sha256::digest(refresh.as_bytes()).to_vec();
    let family: Uuid = sqlx::query_scalar(
        "SELECT family_id FROM auth_refresh_tokens WHERE token_hash=$1 AND user_id=$2",
    )
    .bind(&hash)
    .bind(*user.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    // The test app's absolute family cap is 24h. The successful refresh audit
    // delays the owner transaction past that cap after its first deadline check.
    sqlx::query(
        "UPDATE auth_refresh_token_families \
         SET created_at=clock_timestamp()-interval '24 hours'+interval '4 seconds' WHERE id=$1",
    )
    .bind(family)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(
        "CREATE FUNCTION test_delay_refresh_audit() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN PERFORM pg_sleep(5); RETURN NEW; END $$; \
         CREATE TRIGGER test_delay_refresh_audit AFTER INSERT ON audit_events \
         FOR EACH ROW WHEN (NEW.action='auth.refresh') \
         EXECUTE FUNCTION test_delay_refresh_audit()",
    )
    .execute(&pool)
    .await
    .unwrap();

    let started = std::time::Instant::now();
    let response = post_raw(
        service,
        "/api/v1/auth/token/refresh",
        None,
        json!({ "refresh_token": refresh }),
    )
    .await;
    assert!(!response.status().is_success());
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(4),
        "the request must reach the delayed success audit before the deadline"
    );
    let (tokens, used): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE used_at IS NOT NULL) \
         FROM auth_refresh_tokens WHERE family_id=$1",
    )
    .bind(family)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        tokens, 1,
        "a failed response cannot hide a committed replacement"
    );
    assert_eq!(used, 0, "the presented token must remain unspent");
    assert_audit_count(&pool, "auth.refresh", 0).await;
}

/// A storage failure after a signed assertion must leave the single-use proof
/// available for the same request to finish, with no orphan session or audit.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn passkey_login_session_issue_failure_preserves_signed_proof(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Atomic Login Region", "Atomic Login Branch").await;
    let user_id = seed_user_with_branch(
        &pool,
        "Atomic Login User",
        "010-4100-0091",
        "MECHANIC",
        branch_id,
    )
    .await;
    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );
    let initial_access = admin_session_via_otp(&service, &pool, user_id).await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential_id = enroll_passkey(&service, &mut authenticator, &initial_access).await;

    let start: LoginStartResponse = post_json(
        service.clone(),
        "/api/v1/auth/passkey/login/start",
        None,
        json!({}),
        StatusCode::OK,
    )
    .await;
    let assertion = authenticator
        .do_authentication(
            Url::parse(TEST_ORIGIN).unwrap(),
            inject_allow_credential(start.challenge, &credential_id),
        )
        .unwrap();
    let finish_body = json!({ "ceremony_id": start.ceremony_id, "credential": assertion });
    let before_families: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_refresh_token_families WHERE user_id=$1")
            .bind(user_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    let before_tokens: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_refresh_tokens WHERE user_id=$1")
            .bind(user_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    let before_key: (Value, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT passkey_json, last_used_at FROM auth_webauthn_credentials WHERE credential_id=$1",
    )
    .bind(&credential_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    sqlx::query("ALTER TABLE auth_refresh_token_families ADD CONSTRAINT test_block_login_family CHECK (false) NOT VALID")
        .execute(&pool).await.unwrap();
    let failed = post_raw(
        service.clone(),
        "/api/v1/auth/passkey/login/finish",
        None,
        finish_body.clone(),
    )
    .await;
    sqlx::query("ALTER TABLE auth_refresh_token_families DROP CONSTRAINT test_block_login_family")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let consumed: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_webauthn_ceremonies WHERE id=$1")
            .bind(start.ceremony_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        consumed.is_none(),
        "failed issuance must not consume signed proof"
    );
    let after_key: (Value, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT passkey_json, last_used_at FROM auth_webauthn_credentials WHERE credential_id=$1",
    )
    .bind(&credential_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        after_key, before_key,
        "failed issuance must not advance key state"
    );
    let families: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_refresh_token_families WHERE user_id=$1")
            .bind(user_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    let tokens: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_refresh_tokens WHERE user_id=$1")
            .bind(user_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((families, tokens), (before_families, before_tokens));
    assert_audit_count(&pool, "auth.login", 0).await;

    let accepted: TokenPairResponse = post_json(
        service.clone(),
        "/api/v1/auth/passkey/login/finish",
        None,
        finish_body.clone(),
        StatusCode::OK,
    )
    .await;
    let verifier = console_platform_auth::JwtVerifier::from_es256_public_pem(
        console_platform_auth::JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(10),
        },
        public_key_pem.as_bytes(),
    )
    .unwrap();
    let family_id = verifier
        .verify_access_token(&accepted.access_token)
        .unwrap()
        .session_family_id
        .unwrap();
    let persisted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_refresh_tokens WHERE family_id=$1 AND user_id=$2",
    )
    .bind(family_id)
    .bind(user_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(persisted, 1);
    assert_audit_count(&pool, "auth.login", 1).await;
    let replay = post_raw(
        service,
        "/api/v1/auth/passkey/login/finish",
        None,
        finish_body,
    )
    .await;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn mobile_bound_step_up_start_gates_mobile_approval_and_poll_vote(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Mobile Step-Up Region", "Mobile Step-Up Branch").await;
    let admin_id = seed_user_with_branch(
        &pool,
        "Mobile Step-Up Admin",
        "010-4100-0000",
        "ADMIN",
        branch_id,
    )
    .await;
    let executive_id = seed_user_with_branch(
        &pool,
        "Mobile Step-Up Executive",
        "010-4100-0001",
        "EXECUTIVE",
        branch_id,
    )
    .await;
    let mechanic_id = seed_user_with_branch(
        &pool,
        "Mobile Step-Up Mechanic",
        "010-4100-0002",
        "MECHANIC",
        branch_id,
    )
    .await;
    let receptionist_id = seed_user_with_branch(
        &pool,
        "Mobile Step-Up Reception",
        "010-4100-0003",
        "RECEPTIONIST",
        branch_id,
    )
    .await;
    seed_equipment(&pool, branch_id, "4100").await;
    let work_order_id = seed_mobile_step_up_work_order(
        &pool,
        branch_id,
        receptionist_id,
        mechanic_id,
        admin_id,
        executive_id,
    )
    .await;
    let (poll_id, poll_option_id) = seed_mobile_step_up_poll(&pool, admin_id).await;

    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );
    let admin_access = admin_session_via_otp(&service, &pool, admin_id).await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential_id = enroll_passkey(&service, &mut authenticator, &admin_access).await;

    let approval_path = format!("/api/v1/mobile/work-orders/{work_order_id}/approve");
    let missing_approval = post_raw(
        service.clone(),
        &approval_path,
        Some(&admin_access),
        json!({ "comment": "missing step-up must not mutate" }),
    )
    .await;
    assert_eq!(missing_approval.status(), StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(
        body_json(missing_approval).await["error"]["code"],
        "passkey_step_up_required"
    );
    assert_work_order_status(&pool, work_order_id, "REPORT_SUBMITTED").await;
    assert_audit_count(&pool, "work_order.approve", 0).await;

    let missing_replay_attempt_start = post_raw(
        service.clone(),
        "/api/v1/auth/passkey/step-up/start",
        Some(&admin_access),
        json!({
            "binding": {
                "action_kind": "APPROVAL_DECISION",
                "object_id": work_order_id,
                "reason_key": "operations_passkey_approval_decision"
            }
        }),
    )
    .await;
    assert_eq!(
        missing_replay_attempt_start.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let invalid_replay_attempt_start = post_raw(
        service.clone(),
        "/api/v1/auth/passkey/step-up/start",
        Some(&admin_access),
        json!({
            "binding": {
                "action_kind": "APPROVAL_DECISION",
                "object_id": work_order_id,
                "reason_key": "operations_passkey_approval_decision",
                "replay_attempt": 0
            }
        }),
    )
    .await;
    assert_eq!(
        invalid_replay_attempt_start.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let approval_binding = json!({
        "action_kind": "APPROVAL_DECISION",
        "object_id": work_order_id,
        "reason_key": "operations_passkey_approval_decision",
        "replay_attempt": null
    });
    let approval_step_up = start_mobile_step_up_assertion(
        &service,
        &mut authenticator,
        &credential_id,
        &admin_access,
        approval_binding.clone(),
    )
    .await;
    assert_persisted_mobile_step_up_binding(&pool, &approval_step_up).await;

    let executive_access = admin_session_via_otp(&service, &pool, executive_id).await;
    let mut executive_authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let executive_credential_id =
        enroll_passkey(&service, &mut executive_authenticator, &executive_access).await;
    let wrong_user_step_up = start_mobile_step_up_assertion(
        &service,
        &mut executive_authenticator,
        &executive_credential_id,
        &executive_access,
        approval_binding.clone(),
    )
    .await;

    let wrong_user_approval = post_raw(
        service.clone(),
        &approval_path,
        Some(&admin_access),
        json!({
            "comment": "wrong user step-up must not mutate",
            "step_up": wrong_user_step_up
        }),
    )
    .await;
    assert_eq!(wrong_user_approval.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        body_json(wrong_user_approval).await["error"]["code"],
        "passkey_step_up_failed"
    );
    assert_work_order_status(&pool, work_order_id, "REPORT_SUBMITTED").await;
    assert_audit_count(&pool, "work_order.approve", 0).await;

    let mismatched_approval = post_raw(
        service.clone(),
        &approval_path,
        Some(&admin_access),
        json!({
            "comment": "mismatched step-up must not mutate",
            "step_up": {
                "binding": {
                    "action_kind": "APPROVAL_DECISION",
                    "object_id": Uuid::new_v4(),
                    "reason_key": "operations_passkey_approval_decision",
                    "replay_attempt": null
                },
                "assertion": approval_step_up["assertion"].clone()
            }
        }),
    )
    .await;
    assert_eq!(mismatched_approval.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        body_json(mismatched_approval).await["error"]["code"],
        "passkey_step_up_binding_mismatch"
    );
    assert_work_order_status(&pool, work_order_id, "REPORT_SUBMITTED").await;
    assert_audit_count(&pool, "work_order.approve", 0).await;

    let approved = post_json::<Value>(
        service.clone(),
        &approval_path,
        Some(&admin_access),
        json!({
            "comment": "bound step-up approved",
            "step_up": approval_step_up
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(approved["status"], "ADMIN_REVIEW");
    assert_audit_count(&pool, "work_order.approve", 1).await;

    let replayed_approval = post_raw(
        service.clone(),
        &approval_path,
        Some(&admin_access),
        json!({
            "comment": "replayed step-up must not mutate",
            "step_up": approval_step_up
        }),
    )
    .await;
    assert_eq!(replayed_approval.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        body_json(replayed_approval).await["error"]["code"],
        "passkey_step_up_failed"
    );
    assert_work_order_status(&pool, work_order_id, "ADMIN_REVIEW").await;
    assert_audit_count(&pool, "work_order.approve", 1).await;

    let poll_path = format!("/api/v1/mobile/collaboration/polls/{poll_id}/vote");
    let missing_poll = post_raw(
        service.clone(),
        &poll_path,
        Some(&admin_access),
        json!({ "selected_option_ids": [poll_option_id] }),
    )
    .await;
    assert_eq!(missing_poll.status(), StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(
        body_json(missing_poll).await["error"]["code"],
        "passkey_step_up_required"
    );
    assert_poll_vote_count(&pool, poll_id, 0).await;
    assert_audit_count(&pool, "collaboration.poll.vote", 0).await;

    let poll_binding = json!({
        "action_kind": "POLL_VOTE",
        "object_id": poll_id,
        "reason_key": "operations_passkey_poll_vote",
        "replay_attempt": null
    });
    let poll_step_up = start_mobile_step_up_assertion(
        &service,
        &mut authenticator,
        &credential_id,
        &admin_access,
        poll_binding.clone(),
    )
    .await;
    assert_persisted_mobile_step_up_binding(&pool, &poll_step_up).await;

    let voted = post_json::<Value>(
        service.clone(),
        &poll_path,
        Some(&admin_access),
        json!({
            "selected_option_ids": [poll_option_id],
            "step_up": poll_step_up
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(voted["my_vote"]["submitted"], true);
    assert_poll_vote_count(&pool, poll_id, 1).await;
    assert_audit_count(&pool, "collaboration.poll.vote", 1).await;

    let replayed_poll = post_raw(
        service.clone(),
        &poll_path,
        Some(&admin_access),
        json!({
            "selected_option_ids": [poll_option_id],
            "step_up": poll_step_up
        }),
    )
    .await;
    assert_eq!(replayed_poll.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        body_json(replayed_poll).await["error"]["code"],
        "passkey_step_up_failed"
    );
    assert_poll_vote_count(&pool, poll_id, 1).await;
    assert_audit_count(&pool, "collaboration.poll.vote", 1).await;

    let replay_poll_binding = json!({
        "action_kind": "POLL_VOTE",
        "object_id": poll_id,
        "reason_key": "operations_passkey_poll_vote",
        "replay_attempt": 1
    });
    let replay_poll_step_up = start_mobile_step_up_assertion(
        &service,
        &mut authenticator,
        &credential_id,
        &admin_access,
        replay_poll_binding,
    )
    .await;
    assert_persisted_mobile_step_up_binding(&pool, &replay_poll_step_up).await;
    let replay_attempt_voted = post_json::<Value>(
        service.clone(),
        &poll_path,
        Some(&admin_access),
        json!({
            "selected_option_ids": [poll_option_id],
            "step_up": replay_poll_step_up
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay_attempt_voted["my_vote"]["submitted"], true);
    assert_poll_vote_count(&pool, poll_id, 1).await;
    assert_audit_count(&pool, "collaboration.poll.vote", 2).await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn financial_purchase_sensitive_actions_require_fresh_passkey_step_up(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(
        &pool,
        "Financial Step-Up Region",
        "Financial Step-Up Branch",
    )
    .await;
    let requester_id = seed_user_with_branch(
        &pool,
        "Financial Step-Up Requester",
        "010-4200-0000",
        "MECHANIC",
        branch_id,
    )
    .await;
    let receptionist_id = seed_user_with_branch(
        &pool,
        "Financial Step-Up Reception",
        "010-4200-0001",
        "RECEPTIONIST",
        branch_id,
    )
    .await;
    let admin_id = seed_user_with_branch(
        &pool,
        "Financial Step-Up Admin",
        "010-4200-0002",
        "ADMIN",
        branch_id,
    )
    .await;
    let executive_id = seed_user_with_branch(
        &pool,
        "Financial Step-Up Executive",
        "010-4200-0003",
        "EXECUTIVE",
        branch_id,
    )
    .await;
    let fixture =
        seed_financial_step_up_fixture(&pool, branch_id, requester_id, receptionist_id, admin_id)
            .await;

    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );
    let admin_access = admin_session_via_otp(&service, &pool, admin_id).await;
    let mut admin_authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let admin_credential_id =
        enroll_passkey(&service, &mut admin_authenticator, &admin_access).await;
    let receptionist_access = admin_session_via_otp(&service, &pool, receptionist_id).await;
    let mut receptionist_authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let receptionist_credential_id = enroll_passkey(
        &service,
        &mut receptionist_authenticator,
        &receptionist_access,
    )
    .await;
    let executive_access = admin_session_via_otp(&service, &pool, executive_id).await;
    let mut executive_authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let executive_credential_id =
        enroll_passkey(&service, &mut executive_authenticator, &executive_access).await;

    let admin_approve_purchase = submitted_financial_purchase(&pool, fixture, 900_000).await;
    let admin_approve_path =
        format!("/api/v1/financial/purchase-requests/{admin_approve_purchase}/approve-admin");
    assert_financial_step_up_denied(
        service.clone(),
        &pool,
        &admin_access,
        &admin_approve_path,
        json!({}),
        admin_approve_purchase,
        "REQUEST_SUBMITTED",
        "purchase.admin.approve",
        "purchase.admin.approve",
        StatusCode::PRECONDITION_REQUIRED,
        "passkey_step_up_required",
        "missing",
    )
    .await;
    let invalid_admin_step_up = start_step_up_assertion(
        &service,
        &mut executive_authenticator,
        &executive_credential_id,
    )
    .await;
    assert_financial_step_up_denied(
        service.clone(),
        &pool,
        &admin_access,
        &admin_approve_path,
        json!({ "step_up": invalid_admin_step_up }),
        admin_approve_purchase,
        "REQUEST_SUBMITTED",
        "purchase.admin.approve",
        "purchase.admin.approve",
        StatusCode::UNAUTHORIZED,
        "passkey_step_up_failed",
        "invalid_or_expired",
    )
    .await;
    let valid_admin_step_up =
        start_step_up_assertion(&service, &mut admin_authenticator, &admin_credential_id).await;
    let admin_approved = post_json::<Value>(
        service.clone(),
        &admin_approve_path,
        Some(&admin_access),
        json!({ "step_up": valid_admin_step_up }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(admin_approved["status"], "ADMIN_APPROVED");
    assert_purchase_status(&pool, admin_approve_purchase, "ADMIN_APPROVED").await;
    assert_financial_audit_count(&pool, "purchase.admin.approve", admin_approve_purchase, 1).await;

    let prepare_purchase = admin_approved_financial_purchase(&pool, fixture, 3_000_000).await;
    let prepare_path =
        format!("/api/v1/financial/purchase-requests/{prepare_purchase}/prepare-expenditure");
    assert_financial_step_up_denied(
        service.clone(),
        &pool,
        &admin_access,
        &prepare_path,
        json!({ "expenditure_no": "EXP-MISSING-001" }),
        prepare_purchase,
        "ADMIN_APPROVED",
        "purchase.expenditure.prepare",
        "purchase.expenditure.prepare",
        StatusCode::PRECONDITION_REQUIRED,
        "passkey_step_up_required",
        "missing",
    )
    .await;
    let invalid_prepare_step_up = start_step_up_assertion(
        &service,
        &mut executive_authenticator,
        &executive_credential_id,
    )
    .await;
    assert_financial_step_up_denied(
        service.clone(),
        &pool,
        &admin_access,
        &prepare_path,
        json!({ "expenditure_no": "EXP-INVALID-001", "step_up": invalid_prepare_step_up }),
        prepare_purchase,
        "ADMIN_APPROVED",
        "purchase.expenditure.prepare",
        "purchase.expenditure.prepare",
        StatusCode::UNAUTHORIZED,
        "passkey_step_up_failed",
        "invalid_or_expired",
    )
    .await;
    let valid_prepare_step_up =
        start_step_up_assertion(&service, &mut admin_authenticator, &admin_credential_id).await;
    let prepared = post_json::<Value>(
        service.clone(),
        &prepare_path,
        Some(&admin_access),
        json!({ "expenditure_no": "EXP-VALID-001", "step_up": valid_prepare_step_up }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(prepared["status"], "EXECUTIVE_PENDING");
    assert_purchase_status(&pool, prepare_purchase, "EXECUTIVE_PENDING").await;
    assert_financial_audit_count(&pool, "purchase.expenditure.prepare", prepare_purchase, 1).await;

    let executive_purchase = executive_pending_financial_purchase(&pool, fixture).await;
    let executive_path =
        format!("/api/v1/financial/purchase-requests/{executive_purchase}/approve-executive");
    assert_financial_step_up_denied(
        service.clone(),
        &pool,
        &executive_access,
        &executive_path,
        json!({}),
        executive_purchase,
        "EXECUTIVE_PENDING",
        "purchase.executive.approve",
        "purchase.executive.approve",
        StatusCode::PRECONDITION_REQUIRED,
        "passkey_step_up_required",
        "missing",
    )
    .await;
    let invalid_executive_step_up =
        start_step_up_assertion(&service, &mut admin_authenticator, &admin_credential_id).await;
    assert_financial_step_up_denied(
        service.clone(),
        &pool,
        &executive_access,
        &executive_path,
        json!({ "step_up": invalid_executive_step_up }),
        executive_purchase,
        "EXECUTIVE_PENDING",
        "purchase.executive.approve",
        "purchase.executive.approve",
        StatusCode::UNAUTHORIZED,
        "passkey_step_up_failed",
        "invalid_or_expired",
    )
    .await;
    let valid_executive_step_up = start_step_up_assertion(
        &service,
        &mut executive_authenticator,
        &executive_credential_id,
    )
    .await;
    let executive_approved = post_json::<Value>(
        service.clone(),
        &executive_path,
        Some(&executive_access),
        json!({ "step_up": valid_executive_step_up }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(executive_approved["status"], "READY_TO_EXECUTE");
    assert_purchase_status(&pool, executive_purchase, "READY_TO_EXECUTE").await;
    assert_financial_audit_count(&pool, "purchase.executive.approve", executive_purchase, 1).await;

    let reject_purchase = submitted_financial_purchase(&pool, fixture, 900_000).await;
    let reject_path = format!("/api/v1/financial/purchase-requests/{reject_purchase}/reject");
    assert_financial_step_up_denied(
        service.clone(),
        &pool,
        &admin_access,
        &reject_path,
        json!({ "memo": "missing proof" }),
        reject_purchase,
        "REQUEST_SUBMITTED",
        "purchase.reject",
        "purchase.reject",
        StatusCode::PRECONDITION_REQUIRED,
        "passkey_step_up_required",
        "missing",
    )
    .await;
    let invalid_reject_step_up = start_step_up_assertion(
        &service,
        &mut executive_authenticator,
        &executive_credential_id,
    )
    .await;
    assert_financial_step_up_denied(
        service.clone(),
        &pool,
        &admin_access,
        &reject_path,
        json!({ "memo": "invalid proof", "step_up": invalid_reject_step_up }),
        reject_purchase,
        "REQUEST_SUBMITTED",
        "purchase.reject",
        "purchase.reject",
        StatusCode::UNAUTHORIZED,
        "passkey_step_up_failed",
        "invalid_or_expired",
    )
    .await;
    let valid_reject_step_up =
        start_step_up_assertion(&service, &mut admin_authenticator, &admin_credential_id).await;
    let rejected = post_json::<Value>(
        service.clone(),
        &reject_path,
        Some(&admin_access),
        json!({ "memo": "valid rejection", "step_up": valid_reject_step_up }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(rejected["status"], "REJECTED");
    assert_purchase_status(&pool, reject_purchase, "REJECTED").await;
    assert_financial_audit_count(&pool, "purchase.reject", reject_purchase, 1).await;

    let execute_purchase = ready_to_execute_financial_purchase(&pool, fixture).await;
    let execute_path = format!("/api/v1/financial/purchase-requests/{execute_purchase}/execute");
    assert_financial_step_up_denied(
        service.clone(),
        &pool,
        &receptionist_access,
        &execute_path,
        json!({}),
        execute_purchase,
        "READY_TO_EXECUTE",
        "purchase.execute",
        "purchase.execute",
        StatusCode::PRECONDITION_REQUIRED,
        "passkey_step_up_required",
        "missing",
    )
    .await;
    let invalid_execute_step_up =
        start_step_up_assertion(&service, &mut admin_authenticator, &admin_credential_id).await;
    assert_financial_step_up_denied(
        service.clone(),
        &pool,
        &receptionist_access,
        &execute_path,
        json!({ "step_up": invalid_execute_step_up }),
        execute_purchase,
        "READY_TO_EXECUTE",
        "purchase.execute",
        "purchase.execute",
        StatusCode::UNAUTHORIZED,
        "passkey_step_up_failed",
        "invalid_or_expired",
    )
    .await;
    let valid_execute_step_up = start_step_up_assertion(
        &service,
        &mut receptionist_authenticator,
        &receptionist_credential_id,
    )
    .await;
    let executed = post_json::<Value>(
        service.clone(),
        &execute_path,
        Some(&receptionist_access),
        json!({ "step_up": valid_execute_step_up }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(executed["status"], "EXECUTED");
    assert_purchase_status(&pool, execute_purchase, "EXECUTED").await;
    assert_financial_audit_count(&pool, "purchase.execute", execute_purchase, 1).await;
}

/// Regression for desktop/phone onboarding: a zero-passkey user can refresh the
/// OTP-minted session before enrollment completes. Refresh must keep carrying the
/// setup flag, otherwise a hard reload recreates a normal session and lets the
/// user into the app without registering a passkey.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn refresh_keeps_zero_passkey_user_in_setup_mode_until_enrolled(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Refresh Setup Region", "Refresh Setup Branch").await;
    let user_id = seed_user_with_branch(
        &pool,
        "Refresh Setup User",
        "010-4090-0001",
        "MECHANIC",
        branch_id,
    )
    .await;
    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );

    let issue = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            &pool,
            *user_id.as_uuid(),
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::hours(24),
        )
        .await
        .unwrap();
    let redeem: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issue.token.as_str() }),
        StatusCode::OK,
    )
    .await;
    assert!(redeem.requires_passkey_setup);

    let refresh_token = redeem
        .refresh_token
        .clone()
        .expect("body transport must return refresh token");
    let refreshed: TokenPairResponse = post_json(
        service.clone(),
        "/api/v1/auth/token/refresh",
        None,
        json!({ "refresh_token": refresh_token }),
        StatusCode::OK,
    )
    .await;
    assert!(
        refreshed.requires_passkey_setup,
        "refresh before enrollment must keep the client locked on passkey setup"
    );

    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    enroll_passkey(&service, &mut authenticator, &refreshed.access_token).await;
    let post_enrollment_refresh = refreshed
        .refresh_token
        .clone()
        .expect("rotated body refresh token must be returned");
    let refreshed_after_enrollment: TokenPairResponse = post_json(
        service.clone(),
        "/api/v1/auth/token/refresh",
        None,
        json!({ "refresh_token": post_enrollment_refresh }),
        StatusCode::OK,
    )
    .await;
    assert!(
        !refreshed_after_enrollment.requires_passkey_setup,
        "refresh after successful passkey enrollment should clear the setup flag"
    );
}

/// `/device-login/approve-session` is only for the first-enrollment QR path,
/// where the desktop handoff is pinned to the OTP user/org. Generic desktop QR
/// logins must still require a fresh WebAuthn assertion through
/// `/device-login/approve`; a normal bearer session is not enough.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn approve_session_rejects_generic_desktop_handoff_without_target(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Device QR Region", "Device QR Branch").await;
    let user_id = seed_user_with_branch(
        &pool,
        "Device QR User",
        "010-4090-0002",
        "MECHANIC",
        branch_id,
    )
    .await;
    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );

    let issue = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            &pool,
            *user_id.as_uuid(),
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::hours(24),
        )
        .await
        .unwrap();
    let redeem: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issue.token.as_str() }),
        StatusCode::OK,
    )
    .await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    enroll_passkey(&service, &mut authenticator, &redeem.access_token).await;

    let handoff: DeviceLoginStartResponse = post_json(
        service.clone(),
        "/api/v1/auth/device-login/start",
        None,
        json!({}),
        StatusCode::OK,
    )
    .await;
    let approve_url = Url::parse(&handoff.approve_url).unwrap();
    let approve_token = approve_url
        .fragment()
        .and_then(|fragment| {
            url::form_urlencoded::parse(fragment.as_bytes())
                .find(|(key, _)| key == "desktop_approve")
                .map(|(_, value)| value.into_owned())
        })
        .expect("desktop approve token must be in the URL fragment");

    let response = post_raw(
        service.clone(),
        "/api/v1/auth/device-login/approve-session",
        Some(&redeem.access_token),
        json!({ "approve_token": approve_token }),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "generic desktop QR handoffs require a fresh passkey assertion"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn approve_session_rejects_access_token_expired_during_family_lock(pool: PgPool) {
    use sha2::{Digest, Sha256};

    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key
        .to_pkcs8_pem(LineEnding::LF)
        .unwrap()
        .to_string();
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch = seed_branch(&pool, "Expiry Region", "Expiry Branch").await;
    let user =
        seed_user_with_branch(&pool, "Expiry User", "010-4090-0093", "MECHANIC", branch).await;
    let mut runtime_probe = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(OrgId::knl().to_string())
        .execute(runtime_probe.as_mut())
        .await
        .unwrap();
    sqlx::query("SET LOCAL ROLE console_rt")
        .execute(runtime_probe.as_mut())
        .await
        .unwrap();
    let status: bool = sqlx::query_scalar("SELECT status='ACTIVE' FROM organizations WHERE id=$1")
        .bind(*OrgId::knl().as_uuid())
        .fetch_one(runtime_probe.as_mut())
        .await
        .unwrap();
    assert!(status);
    runtime_probe.rollback().await.unwrap();
    let service =
        build_router(app_state(pool.clone(), private_pem.clone(), public_pem.clone()).unwrap());
    let bootstrap = admin_session_via_otp(&service, &pool, user).await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = enroll_passkey(&service, &mut authenticator, &bootstrap).await;
    let session = usernameless_login(&service, &mut authenticator, &credential).await;

    let approve_token = format!("console_dla_{}", "a".repeat(64));
    let poll_token = format!("console_dlp_{}", "b".repeat(64));
    let handoff: Uuid = sqlx::query_scalar(
        "INSERT INTO auth_device_login_handoffs (poll_token_hash,approve_token_hash,issued_at,expires_at,target_user_id,target_org_id) VALUES ($1,$2,clock_timestamp(),clock_timestamp()+interval '5 minutes',$3,$4) RETURNING id",
    )
    .bind(Sha256::digest(poll_token.as_bytes()).to_vec())
    .bind(Sha256::digest(approve_token.as_bytes()).to_vec())
    .bind(*user.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let verifier = console_platform_auth::JwtVerifier::from_es256_public_pem(
        console_platform_auth::JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(10),
        },
        public_pem.as_bytes(),
    )
    .unwrap();
    let mut claims =
        serde_json::to_value(verifier.verify_access_token(&session.access_token).unwrap()).unwrap();
    let family: Uuid = serde_json::from_value(claims["session_family_id"].clone()).unwrap();
    let deadline = OffsetDateTime::now_utc().unix_timestamp() + 3;
    claims["exp"] = json!(deadline);
    let expiring = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256),
        &claims,
        &jsonwebtoken::EncodingKey::from_ec_pem(private_pem.as_bytes()).unwrap(),
    )
    .unwrap();

    let mut held = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM auth_refresh_token_families WHERE id=$1 FOR UPDATE")
        .bind(family)
        .fetch_one(held.as_mut())
        .await
        .unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(held.as_mut())
        .await
        .unwrap();
    let route = service.clone();
    let token = approve_token.clone();
    let request = tokio::spawn(async move {
        post_raw(
            route,
            "/api/v1/auth/device-login/approve-session",
            Some(&expiring),
            json!({ "approve_token": token }),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))",
            )
            .bind(blocker)
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("approval must reach the refresh-family lock before token expiry");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while OffsetDateTime::now_utc().unix_timestamp() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    held.rollback().await.unwrap();
    let rejected = tokio::time::timeout(std::time::Duration::from_secs(5), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    let approved_at: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT approved_at FROM auth_device_login_handoffs WHERE id=$1")
            .bind(handoff)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(approved_at.is_none());
    assert_audit_count(&pool, "auth.device_login.approve_session", 0).await;
    let accepted = post_raw(
        service.clone(),
        "/api/v1/auth/device-login/approve-session",
        Some(&session.access_token),
        json!({ "approve_token": approve_token }),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::NO_CONTENT);

    let families_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_refresh_token_families WHERE user_id=$1")
            .bind(*user.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut held_handoff = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM auth_device_login_handoffs WHERE id=$1 FOR UPDATE")
        .bind(handoff)
        .fetch_one(held_handoff.as_mut())
        .await
        .unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(held_handoff.as_mut())
        .await
        .unwrap();
    let route = service.clone();
    let token = poll_token.clone();
    let poll = tokio::spawn(async move {
        post_raw(
            route,
            "/api/v1/auth/device-login/poll",
            None,
            json!({ "poll_token": token }),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))",
            )
            .bind(blocker)
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("poll must reach the handoff lock while Company is active");
    sqlx::query("UPDATE organizations SET status='SUSPENDED' WHERE id=$1")
        .bind(*OrgId::knl().as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    held_handoff.rollback().await.unwrap();
    let suspended = tokio::time::timeout(std::time::Duration::from_secs(5), poll)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(suspended.status(), StatusCode::UNAUTHORIZED);
    let consumed: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_device_login_handoffs WHERE id=$1")
            .bind(handoff)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(consumed.is_none());
    let families_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_refresh_token_families WHERE user_id=$1")
            .bind(*user.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(families_after, families_before);
    sqlx::query("UPDATE organizations SET status='ACTIVE' WHERE id=$1")
        .bind(*OrgId::knl().as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let resumed = post_raw(
        service,
        "/api/v1/auth/device-login/poll",
        None,
        json!({ "poll_token": poll_token }),
    )
    .await;
    assert_eq!(resumed.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn platform_admin_can_approve_and_consume_bound_device_handoff(pool: PgPool) {
    use sha2::{Digest, Sha256};

    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key
        .to_pkcs8_pem(LineEnding::LF)
        .unwrap()
        .to_string();
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let user = UserId::new();
    sqlx::query("INSERT INTO users(id,display_name,roles,is_active,org_id) VALUES($1,'Platform Device Admin',ARRAY['SUPER_ADMIN'],true,$2)")
        .bind(*user.as_uuid())
        .bind(*OrgId::platform().as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let platform_status: String =
        sqlx::query_scalar("SELECT status FROM organizations WHERE id=$1")
            .bind(*OrgId::platform().as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(platform_status, "ARCHIVED");
    let service = build_router(app_state(pool.clone(), private_pem, public_pem.clone()).unwrap());
    let issue = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            &pool,
            *user.as_uuid(),
            OrgId::platform(),
            OffsetDateTime::now_utc(),
            Duration::hours(1),
        )
        .await
        .unwrap();
    let bootstrap: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issue.token.as_str() }),
        StatusCode::OK,
    )
    .await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = enroll_passkey(&service, &mut authenticator, &bootstrap.access_token).await;
    let session = usernameless_login(&service, &mut authenticator, &credential).await;
    let approve_token = format!("console_dla_{}", "c".repeat(64));
    let poll_token = format!("console_dlp_{}", "d".repeat(64));
    sqlx::query("INSERT INTO auth_device_login_handoffs(poll_token_hash,approve_token_hash,issued_at,expires_at,target_user_id,target_org_id) VALUES($1,$2,clock_timestamp(),clock_timestamp()+interval '5 minutes',$3,$4)")
        .bind(Sha256::digest(poll_token.as_bytes()).to_vec())
        .bind(Sha256::digest(approve_token.as_bytes()).to_vec())
        .bind(*user.as_uuid())
        .bind(*OrgId::platform().as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let approval = post_raw(
        service.clone(),
        "/api/v1/auth/device-login/approve-session",
        Some(&session.access_token),
        json!({ "approve_token": approve_token }),
    )
    .await;
    assert_eq!(approval.status(), StatusCode::NO_CONTENT);
    let consumed: Value = post_json(
        service,
        "/api/v1/auth/device-login/poll",
        None,
        json!({ "poll_token": poll_token }),
        StatusCode::OK,
    )
    .await;
    let verifier = console_platform_auth::JwtVerifier::from_es256_public_pem(
        console_platform_auth::JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(10),
        },
        public_pem.as_bytes(),
    )
    .unwrap();
    let claims = verifier
        .verify_access_token(consumed["access_token"].as_str().unwrap())
        .unwrap();
    assert!(claims.platform);
    assert_eq!(claims.org, OrgId::platform().to_string());
}

/// A failed handoff approval or session issue must preserve the exact signed
/// proof/poll permit, so a retry completes once without an orphan token family.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn device_login_failures_preserve_proof_and_poll_permit(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Atomic Device Region", "Atomic Device Branch").await;
    let user_id = seed_user_with_branch(
        &pool,
        "Atomic Device User",
        "010-4090-0092",
        "MECHANIC",
        branch_id,
    )
    .await;
    let service =
        build_router(app_state(pool.clone(), private_key_pem.to_string(), public_key_pem).unwrap());
    let initial_access = admin_session_via_otp(&service, &pool, user_id).await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential_id = enroll_passkey(&service, &mut authenticator, &initial_access).await;
    let handoff: DeviceLoginStartResponse = post_json(
        service.clone(),
        "/api/v1/auth/device-login/start",
        None,
        json!({}),
        StatusCode::OK,
    )
    .await;
    let approve_token = Url::parse(&handoff.approve_url)
        .unwrap()
        .fragment()
        .and_then(|fragment| {
            url::form_urlencoded::parse(fragment.as_bytes())
                .find(|(key, _)| key == "desktop_approve")
                .map(|(_, value)| value.into_owned())
        })
        .unwrap();
    let handoff_id: Uuid = sqlx::query_scalar("SELECT id FROM auth_device_login_handoffs")
        .fetch_one(&pool)
        .await
        .unwrap();
    let login_start: LoginStartResponse = post_json(
        service.clone(),
        "/api/v1/auth/passkey/login/start",
        None,
        json!({}),
        StatusCode::OK,
    )
    .await;
    let assertion = authenticator
        .do_authentication(
            Url::parse(TEST_ORIGIN).unwrap(),
            inject_allow_credential(login_start.challenge, &credential_id),
        )
        .unwrap();
    let approval = json!({
        "approve_token": approve_token,
        "ceremony_id": login_start.ceremony_id,
        "credential": assertion,
    });

    sqlx::query("ALTER TABLE auth_device_login_handoffs ADD CONSTRAINT test_block_handoff_approval CHECK (approved_at IS NULL) NOT VALID")
        .execute(&pool).await.unwrap();
    let failed_approval = post_raw(
        service.clone(),
        "/api/v1/auth/device-login/approve",
        None,
        approval.clone(),
    )
    .await;
    sqlx::query(
        "ALTER TABLE auth_device_login_handoffs DROP CONSTRAINT test_block_handoff_approval",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(failed_approval.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let ceremony_consumed: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_webauthn_ceremonies WHERE id=$1")
            .bind(login_start.ceremony_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(ceremony_consumed.is_none());
    let approved_at: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT approved_at FROM auth_device_login_handoffs WHERE id=$1")
            .bind(handoff_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(approved_at.is_none());
    assert_audit_count(&pool, "auth.device_login.approve", 0).await;
    let approved = post_raw(
        service.clone(),
        "/api/v1/auth/device-login/approve",
        None,
        approval.clone(),
    )
    .await;
    assert_eq!(approved.status(), StatusCode::NO_CONTENT);
    assert_audit_count(&pool, "auth.device_login.approve", 1).await;
    let approval_replay = post_raw(
        service.clone(),
        "/api/v1/auth/device-login/approve",
        None,
        approval,
    )
    .await;
    assert_eq!(approval_replay.status(), StatusCode::UNAUTHORIZED);

    let before_families: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_refresh_token_families WHERE user_id=$1")
            .bind(user_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("ALTER TABLE auth_refresh_token_families ADD CONSTRAINT test_block_device_family CHECK (false) NOT VALID")
        .execute(&pool).await.unwrap();
    let poll = json!({ "poll_token": handoff.poll_token });
    let failed_poll = post_raw(
        service.clone(),
        "/api/v1/auth/device-login/poll",
        None,
        poll.clone(),
    )
    .await;
    sqlx::query("ALTER TABLE auth_refresh_token_families DROP CONSTRAINT test_block_device_family")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(failed_poll.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let consumed_at: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_device_login_handoffs WHERE id=$1")
            .bind(handoff_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(consumed_at.is_none());
    let families: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_refresh_token_families WHERE user_id=$1")
            .bind(user_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(families, before_families);
    assert_audit_count(&pool, "auth.device_login.consume", 0).await;
    let accepted = post_json::<Value>(
        service.clone(),
        "/api/v1/auth/device-login/poll",
        None,
        poll.clone(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(accepted["status"], "approved");
    assert!(accepted["access_token"].as_str().is_some());
    assert_audit_count(&pool, "auth.device_login.consume", 1).await;
    let poll_replay = post_raw(service, "/api/v1/auth/device-login/poll", None, poll).await;
    assert_eq!(poll_replay.status(), StatusCode::UNAUTHORIZED);
}

/// Initial passkey enrollment is gated on separate privacy/data-collection and
/// service-terms agreements. A freshly OTP-authenticated user can read the
/// required version, cannot start enrollment until both required boxes are true,
/// and can proceed after acceptance is recorded.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn first_passkey_enrollment_requires_privacy_terms(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Privacy Region", "Privacy Branch").await;
    let user_id = seed_user_with_branch(
        &pool,
        "Privacy User",
        "010-4050-0001",
        "MECHANIC",
        branch_id,
    )
    .await;
    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );

    let issue = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            &pool,
            *user_id.as_uuid(),
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::hours(24),
        )
        .await
        .unwrap();
    let redeem: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issue.token.as_str() }),
        StatusCode::OK,
    )
    .await;
    assert!(redeem.requires_passkey_setup);

    let initial_status: PrivacyConsentStatusResponse = post_json(
        service.clone(),
        "/api/v1/auth/privacy-consent/status",
        Some(&redeem.access_token),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert!(!initial_status.accepted);
    assert!(initial_status.accepted_at.is_none());

    let blocked = post_raw(
        service.clone(),
        "/api/v1/auth/passkey/register/start",
        Some(&redeem.access_token),
        json!({ "username": "privacy.user", "display_name": "Privacy User" }),
    )
    .await;
    assert_eq!(blocked.status(), StatusCode::FORBIDDEN);

    let bundled_or_partial_consent = post_raw(
        service.clone(),
        "/api/v1/auth/privacy-consent/accept",
        Some(&redeem.access_token),
        json!({
            "policy_version": initial_status.policy_version,
            "privacy_collection": true,
            "terms_of_service": false
        }),
    )
    .await;
    assert_eq!(bundled_or_partial_consent.status(), StatusCode::BAD_REQUEST);

    let accepted = accept_required_privacy_consent(&service, &redeem.access_token).await;
    assert!(accepted.accepted);
    assert!(accepted.accepted_at.is_some());

    let allowed: RegisterStartResponse = post_json(
        service.clone(),
        "/api/v1/auth/passkey/register/start",
        Some(&redeem.access_token),
        json!({ "username": "privacy.user", "display_name": "Privacy User" }),
        StatusCode::OK,
    )
    .await;
    assert_ne!(allowed.ceremony_id, Uuid::nil());
}

/// The one-time code is consumed on PASSKEY REGISTRATION, not on redeem. A redeem
/// only mints a session, so a failed/incomplete enrollment never burns the code —
/// the user can re-redeem (within the TTL) until a passkey actually sticks. Once a
/// passkey is registered the code is consumed atomically and can never be reused.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn otp_is_consumed_on_passkey_registration_not_on_redeem(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "OTP Region", "OTP Branch").await;
    let admin_id =
        seed_user_with_branch(&pool, "Issuer Admin", "010-4100-0000", "ADMIN", branch_id).await;
    let new_user_id = seed_user_with_branch(
        &pool,
        "Pending User",
        "010-4100-0001",
        "MECHANIC",
        branch_id,
    )
    .await;
    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );

    let admin_access = admin_session_via_otp(&service, &pool, admin_id).await;
    let issued: AdminIssueOtpResponse = post_json(
        service.clone(),
        "/api/v1/auth/admin/otp/issue",
        Some(&admin_access),
        json!({ "user_id": new_user_id.as_uuid(), "branch_id": branch_id }),
        StatusCode::OK,
    )
    .await;

    // First redeem -> session, code NOT consumed.
    let first: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issued.otp }),
        StatusCode::OK,
    )
    .await;
    assert!(first.requires_passkey_setup);

    // Re-redeem BEFORE enrolling a passkey -> STILL succeeds (a failed enrollment
    // must not lock the user out of their own code).
    let second: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issued.otp }),
        StatusCode::OK,
    )
    .await;
    assert!(
        second.requires_passkey_setup,
        "the code must remain redeemable until a passkey is actually registered"
    );

    // Enroll a passkey from the session -> consumes the code atomically with the
    // passkey insert.
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    enroll_passkey(&service, &mut authenticator, &second.access_token).await;

    // Now the code is dead: a further redeem is rejected.
    let after = post_raw(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issued.otp }),
    )
    .await;
    assert_eq!(
        after.status(),
        StatusCode::UNAUTHORIZED,
        "the code is consumed once a passkey is registered"
    );

    // DB: exactly one consumed credential for this user (consumed at enrollment).
    let consumed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_bootstrap_credentials \
         WHERE user_id = $1 AND consumed_at IS NOT NULL",
    )
    .bind(new_user_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(consumed, 1);
}

/// A session minted from an OTP cannot turn a later revoked OTP into first
/// enrollment authority. The signed WebAuthn result is genuine, but its source
/// authorization must still be live when the credential is committed.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn first_passkey_registration_rejects_revoked_bootstrap_after_start(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Revoked OTP Region", "Revoked OTP Branch").await;
    let user_id = seed_user_with_branch(
        &pool,
        "Revoked OTP User",
        "010-4100-0003",
        "MECHANIC",
        branch_id,
    )
    .await;
    let service =
        build_router(app_state(pool.clone(), private_key_pem.to_string(), public_key_pem).unwrap());
    let issue = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            &pool,
            *user_id.as_uuid(),
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::hours(1),
        )
        .await
        .unwrap();
    let session: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issue.token.as_str() }),
        StatusCode::OK,
    )
    .await;
    accept_required_privacy_consent(&service, &session.access_token).await;
    let registration: RegisterStartResponse = post_json(
        service.clone(),
        "/api/v1/auth/passkey/register/start",
        Some(&session.access_token),
        json!({ "username": "revoked.otp", "display_name": "Revoked OTP User" }),
        StatusCode::OK,
    )
    .await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = authenticator
        .do_registration(Url::parse(TEST_ORIGIN).unwrap(), registration.challenge)
        .unwrap();

    let revoked = sqlx::query("UPDATE auth_bootstrap_credentials SET revoked_at=clock_timestamp(), revoked_reason='test_revoked' WHERE id=$1 AND user_id=$2 AND consumed_at IS NULL AND revoked_at IS NULL")
        .bind(issue.credential_id)
        .bind(user_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        revoked.rows_affected(),
        1,
        "the exact OTP source must be revoked"
    );
    let response = post_raw(
        service,
        "/api/v1/auth/passkey/register/finish",
        Some(&session.access_token),
        json!({ "ceremony_id": registration.ceremony_id, "credential": credential }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let keys: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_webauthn_credentials WHERE user_id=$1")
            .bind(user_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(keys, 0, "failed registration must roll back the key");
    let registration_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action='auth.passkey.register'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        registration_audits, 0,
        "failed registration must not be audited as accepted"
    );
    let consumed: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_webauthn_ceremonies WHERE id=$1")
            .bind(registration.ceremony_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        consumed.is_none(),
        "failed registration must preserve the ceremony"
    );
}

/// Refreshing the original session must not change which enrollment source the
/// signed ceremony belongs to. A new QR code for the same Account supersedes the
/// original OTP; neither the old nor the rotated session may finish that ceremony.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn first_passkey_registration_rejects_replaced_source_after_refresh(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Replaced OTP Region", "Replaced OTP Branch").await;
    let user_id = seed_user_with_branch(
        &pool,
        "Replaced OTP User",
        "010-4100-0004",
        "MECHANIC",
        branch_id,
    )
    .await;
    let service =
        build_router(app_state(pool.clone(), private_key_pem.to_string(), public_key_pem).unwrap());
    let original = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            &pool,
            *user_id.as_uuid(),
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::hours(1),
        )
        .await
        .unwrap();
    let session: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": original.token.as_str() }),
        StatusCode::OK,
    )
    .await;
    accept_required_privacy_consent(&service, &session.access_token).await;
    let registration: RegisterStartResponse = post_json(
        service.clone(),
        "/api/v1/auth/passkey/register/start",
        Some(&session.access_token),
        json!({ "username": "replaced.otp", "display_name": "Replaced OTP User" }),
        StatusCode::OK,
    )
    .await;
    let mut original_authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let signed = original_authenticator
        .do_registration(Url::parse(TEST_ORIGIN).unwrap(), registration.challenge)
        .unwrap();
    let rotated: TokenPairResponse = post_json(
        service.clone(),
        "/api/v1/auth/token/refresh",
        None,
        json!({ "refresh_token": session.refresh_token.as_ref().unwrap() }),
        StatusCode::OK,
    )
    .await;
    let replacement: Value = post_json(
        service.clone(),
        "/api/v1/auth/passkey/enroll-handoff",
        Some(&rotated.access_token),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let replacement_otp = replacement["otp"].as_str().unwrap();
    assert_ne!(replacement_otp, original.token.as_str());
    let original_status: (Option<OffsetDateTime>, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT revoked_at, consumed_at FROM auth_bootstrap_credentials WHERE id=$1",
    )
    .bind(original.credential_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(original_status.0.is_some());
    assert!(original_status.1.is_none());

    for access_token in [&session.access_token, &rotated.access_token] {
        let response = post_raw(
            service.clone(),
            "/api/v1/auth/passkey/register/finish",
            Some(access_token),
            json!({ "ceremony_id": registration.ceremony_id, "credential": signed }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let key_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_webauthn_credentials WHERE user_id=$1")
            .bind(user_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(key_count, 0);
    let ceremony_consumed: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_webauthn_ceremonies WHERE id=$1")
            .bind(registration.ceremony_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(ceremony_consumed.is_none());
    assert_audit_count(&pool, "auth.passkey.register", 0).await;
    assert_audit_count(&pool, "auth.otp.consume", 0).await;

    let replacement_session: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": replacement_otp }),
        StatusCode::OK,
    )
    .await;
    let mut replacement_authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    enroll_passkey(
        &service,
        &mut replacement_authenticator,
        &replacement_session.access_token,
    )
    .await;
    assert_audit_count(&pool, "auth.passkey.register", 1).await;
}

/// Replacing an enrollment OTP must not leave its existing family with business
/// authority. Both access tokens from that family and any later refresh must be
/// denied for ordinary work; the replacement code can still complete enrollment.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn replaced_otp_family_cannot_create_business_work(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Replaced Family Region", "Replaced Family Branch").await;
    let user_id = seed_user_with_branch(
        &pool,
        "Replaced Family User",
        "010-4100-0008",
        "MECHANIC",
        branch_id,
    )
    .await;
    seed_equipment(&pool, branch_id, "293").await;
    let service =
        build_router(app_state(pool.clone(), private_key_pem.to_string(), public_key_pem).unwrap());
    let original = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            &pool,
            *user_id.as_uuid(),
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::hours(1),
        )
        .await
        .unwrap();
    let enrollment: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": original.token.as_str() }),
        StatusCode::OK,
    )
    .await;
    // A legacy client can retry the same verify-only code after losing a
    // response. Replacement must fence every family proven by that source.
    let retry: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": original.token.as_str() }),
        StatusCode::OK,
    )
    .await;
    accept_required_privacy_consent(&service, &enrollment.access_token).await;
    let rotated: TokenPairResponse = post_json(
        service.clone(),
        "/api/v1/auth/token/refresh",
        None,
        json!({ "refresh_token": enrollment.refresh_token.as_ref().unwrap() }),
        StatusCode::OK,
    )
    .await;
    let replacement: Value = post_json(
        service.clone(),
        "/api/v1/auth/passkey/enroll-handoff",
        Some(&rotated.access_token),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let replacement_otp = replacement["otp"].as_str().unwrap();
    assert_ne!(replacement_otp, original.token.as_str());
    let original_revoked: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT revoked_at FROM auth_bootstrap_credentials WHERE id=$1 AND user_id=$2",
    )
    .bind(original.credential_id)
    .bind(user_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(original_revoked.is_some(), "the old OTP must be superseded");

    for access_token in [
        &enrollment.access_token,
        &rotated.access_token,
        &retry.access_token,
    ] {
        let response = post_raw(
            service.clone(),
            "/api/work-orders",
            Some(access_token),
            json!({ "branch_id": branch_id, "management_no": "#293", "symptom": "Hydraulic oil leak" }),
        )
        .await;
        assert!(
            matches!(
                response.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ),
            "a replaced OTP family's access token must not create work, got {}",
            response.status()
        );
    }
    let refreshed = post_raw(
        service.clone(),
        "/api/v1/auth/token/refresh",
        None,
        json!({ "refresh_token": rotated.refresh_token.as_ref().unwrap() }),
    )
    .await;
    match refreshed.status() {
        StatusCode::OK => {
            let refreshed: TokenPairResponse = refreshed.into_json(StatusCode::OK).await;
            let response = post_raw(
                service.clone(),
                "/api/work-orders",
                Some(&refreshed.access_token),
                json!({ "branch_id": branch_id, "management_no": "#293", "symptom": "Hydraulic oil leak" }),
            )
            .await;
            assert!(
                matches!(
                    response.status(),
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                ),
                "refreshing a replaced OTP family must not create work, got {}",
                response.status()
            );
        }
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {}
        other => panic!("replaced OTP family refresh returned unexpected status {other}"),
    }
    let retry_refresh = post_raw(
        service.clone(),
        "/api/v1/auth/token/refresh",
        None,
        json!({ "refresh_token": retry.refresh_token.as_ref().unwrap() }),
    )
    .await;
    assert_eq!(retry_refresh.status(), StatusCode::UNAUTHORIZED);
    let work_orders: i64 =
        sqlx::query_scalar("SELECT count(*) FROM work_orders WHERE branch_id=$1")
            .bind(branch_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(work_orders, 0);

    let replacement_session: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": replacement_otp }),
        StatusCode::OK,
    )
    .await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential_id = enroll_passkey(
        &service,
        &mut authenticator,
        &replacement_session.access_token,
    )
    .await;
    let normal = usernameless_login(&service, &mut authenticator, &credential_id).await;
    let created = post_raw(
        service,
        "/api/work-orders",
        Some(&normal.access_token),
        json!({ "branch_id": branch_id, "management_no": "#293", "symptom": "Hydraulic oil leak" }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
}

/// An established Account can add its own verified device with a fresh UV
/// assertion. The first-enrollment OTP rule cannot turn a zero-row consume into
/// a denial for this separate, already-enrolled purpose.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn existing_passkey_addition_succeeds_without_open_bootstrap(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Add Device Region", "Add Device Branch").await;
    let user_id = seed_user_with_branch(
        &pool,
        "Add Device User",
        "010-4100-0005",
        "MECHANIC",
        branch_id,
    )
    .await;
    let service =
        build_router(app_state(pool.clone(), private_key_pem.to_string(), public_key_pem).unwrap());
    let access = admin_session_via_otp(&service, &pool, user_id).await;
    let mut first_authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let first_credential = enroll_passkey(&service, &mut first_authenticator, &access).await;
    let open_bootstrap: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_bootstrap_credentials WHERE user_id=$1 AND consumed_at IS NULL AND revoked_at IS NULL",
    )
    .bind(user_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(open_bootstrap, 0);

    let session = usernameless_login(&service, &mut first_authenticator, &first_credential).await;
    let step_up =
        start_step_up_assertion(&service, &mut first_authenticator, &first_credential).await;
    let registration: RegisterStartResponse = post_json(
        service.clone(),
        "/api/v1/auth/passkey/register/start",
        Some(&session.access_token),
        json!({ "username": "add.device", "display_name": "Add Device User", "step_up": step_up }),
        StatusCode::OK,
    )
    .await;
    let mut second_authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let signed = second_authenticator
        .do_registration(Url::parse(TEST_ORIGIN).unwrap(), registration.challenge)
        .unwrap();
    let response = post_raw(
        service,
        "/api/v1/auth/passkey/register/finish",
        Some(&session.access_token),
        json!({ "ceremony_id": registration.ceremony_id, "credential": signed }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let key_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_webauthn_credentials WHERE user_id=$1")
            .bind(user_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(key_count, 2);
}

/// Initial enrollment has no step-up, so its first JWT family check runs before
/// the Account lock. Expiry during that wait must still reject registration.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn registration_start_rechecks_expired_bearer_after_account_wait(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Initial Wait Region", "Initial Wait Branch").await;
    let user_id = seed_user_with_branch(
        &pool,
        "Initial Wait User",
        "010-4100-0011",
        "MECHANIC",
        branch_id,
    )
    .await;
    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );
    let access = admin_session_via_otp(&service, &pool, user_id).await;
    accept_required_privacy_consent(&service, &access).await;
    let verifier = console_platform_auth::JwtVerifier::from_es256_public_pem(
        console_platform_auth::JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(10),
        },
        public_key_pem.as_bytes(),
    )
    .unwrap();
    let mut claims = serde_json::to_value(verifier.verify_access_token(&access).unwrap()).unwrap();
    let deadline = OffsetDateTime::now_utc().unix_timestamp() + 3;
    claims["exp"] = json!(deadline);
    let expiring = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256),
        &claims,
        &jsonwebtoken::EncodingKey::from_ec_pem(private_key_pem.as_bytes()).unwrap(),
    )
    .unwrap();

    let mut held = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE")
        .bind(user_id.as_uuid())
        .fetch_one(held.as_mut())
        .await
        .unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(held.as_mut())
        .await
        .unwrap();
    let route = service.clone();
    let start = tokio::spawn(async move {
        post_raw(
            route,
            "/api/v1/auth/passkey/register/start",
            Some(&expiring),
            json!({}),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() \
                 AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))",
            )
            .bind(blocker)
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("start must reach the Account lock after its first JWT check");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while OffsetDateTime::now_utc().unix_timestamp() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("access token must expire while Account remains locked");
    held.rollback().await.unwrap();
    assert_eq!(start.await.unwrap().status(), StatusCode::UNAUTHORIZED);
    let bound: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_legacy_registration_bindings WHERE user_id=$1",
    )
    .bind(user_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(bound, 0, "expired bearer must not bind a challenge");
}

/// Logout or access expiry can occur while registration waits for the Account
/// mutex. An already-validated bearer must not finish the ceremony afterward.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn registration_rechecks_revoked_family_after_account_wait(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Family Wait Region", "Family Wait Branch").await;
    let user_id = seed_user_with_branch(
        &pool,
        "Family Wait User",
        "010-4100-0009",
        "MECHANIC",
        branch_id,
    )
    .await;
    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );
    let bootstrap = admin_session_via_otp(&service, &pool, user_id).await;
    let mut first_authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let first_credential = enroll_passkey(&service, &mut first_authenticator, &bootstrap).await;
    let session = usernameless_login(&service, &mut first_authenticator, &first_credential).await;
    let step_up =
        start_step_up_assertion(&service, &mut first_authenticator, &first_credential).await;
    let registration: RegisterStartResponse = post_json(
        service.clone(),
        "/api/v1/auth/passkey/register/start",
        Some(&session.access_token),
        json!({ "step_up": step_up }),
        StatusCode::OK,
    )
    .await;
    let mut second_authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let signed = second_authenticator
        .do_registration(Url::parse(TEST_ORIGIN).unwrap(), registration.challenge)
        .unwrap();
    let finish_body = json!({ "ceremony_id": registration.ceremony_id, "credential": signed });
    let verifier = console_platform_auth::JwtVerifier::from_es256_public_pem(
        console_platform_auth::JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(10),
        },
        public_key_pem.as_bytes(),
    )
    .unwrap();
    let claims = verifier.verify_access_token(&session.access_token).unwrap();
    let family_id = claims.session_family_id.unwrap();
    let mut expiring_claims = serde_json::to_value(claims).unwrap();
    let deadline = OffsetDateTime::now_utc().unix_timestamp() + 3;
    expiring_claims["exp"] = json!(deadline);
    let expiring = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256),
        &expiring_claims,
        &jsonwebtoken::EncodingKey::from_ec_pem(private_key_pem.as_bytes()).unwrap(),
    )
    .unwrap();

    let mut held_expiry = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE")
        .bind(user_id.as_uuid())
        .fetch_one(held_expiry.as_mut())
        .await
        .unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(held_expiry.as_mut())
        .await
        .unwrap();
    let route = service.clone();
    let expiry_body = finish_body.clone();
    let expiry_finish = tokio::spawn(async move {
        post_raw(
            route,
            "/api/v1/auth/passkey/register/finish",
            Some(&expiring),
            expiry_body,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() \
                 AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))",
            )
            .bind(blocker)
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("registration must reach the Account lock before access expiry");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while OffsetDateTime::now_utc().unix_timestamp() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("access token must expire while Account remains locked");
    held_expiry.rollback().await.unwrap();
    assert_eq!(
        expiry_finish.await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let consumed: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT consumed_at FROM auth_webauthn_ceremonies WHERE id=$1")
            .bind(registration.ceremony_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(consumed.is_none(), "expiry must roll back the ceremony");

    let mut held = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE")
        .bind(user_id.as_uuid())
        .fetch_one(held.as_mut())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE auth_refresh_token_families SET revoked_at=clock_timestamp(), \
         revoked_reason='test_logout' WHERE id=$1",
    )
    .bind(family_id)
    .execute(held.as_mut())
    .await
    .unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(held.as_mut())
        .await
        .unwrap();
    let route = service.clone();
    let access = session.access_token.clone();
    let finish = tokio::spawn(async move {
        post_raw(
            route,
            "/api/v1/auth/passkey/register/finish",
            Some(&access),
            finish_body,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() \
                 AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))",
            )
            .bind(blocker)
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("registration must reach the Account lock before revocation commits");
    held.commit().await.unwrap();
    let rejected = finish.await.unwrap();
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    let key_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_webauthn_credentials WHERE user_id=$1")
            .bind(user_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(key_count, 1);
    assert_audit_count(&pool, "auth.passkey.register", 1).await;
}

/// An OTP proves possession of a short-lived enrollment source, not normal
/// business authority. Its restricted session cannot create a work order before
/// a real passkey enrollment and fresh login.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn otp_enrollment_session_cannot_create_business_work(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Enrollment Scope Region", "Enrollment Scope Branch").await;
    let user_id = seed_user_with_branch(
        &pool,
        "Enrollment Scope User",
        "010-4100-0006",
        "MECHANIC",
        branch_id,
    )
    .await;
    seed_equipment(&pool, branch_id, "291").await;
    let service =
        build_router(app_state(pool.clone(), private_key_pem.to_string(), public_key_pem).unwrap());
    let access = admin_session_via_otp(&service, &pool, user_id).await;
    let _: PrivacyConsentStatusResponse = post_json(
        service.clone(),
        "/api/v1/auth/privacy-consent/status",
        Some(&access),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let response = post_raw(
        service.clone(),
        "/api/work-orders",
        Some(&access),
        json!({
            "branch_id": branch_id,
            "management_no": "#291",
            "symptom": "Hydraulic oil leak"
        }),
    )
    .await;
    assert!(
        matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "an enrollment-only session must be denied by business APIs, got {}",
        response.status()
    );
    let work_orders: i64 =
        sqlx::query_scalar("SELECT count(*) FROM work_orders WHERE branch_id=$1")
            .bind(branch_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(work_orders, 0);

    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential_id = enroll_passkey(&service, &mut authenticator, &access).await;
    let normal = usernameless_login(&service, &mut authenticator, &credential_id).await;
    let created = post_raw(
        service,
        "/api/work-orders",
        Some(&normal.access_token),
        json!({
            "branch_id": branch_id,
            "management_no": "#291",
            "symptom": "Hydraulic oil leak"
        }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
}

/// Finishing enrollment never promotes an OTP family's authority. The old
/// family may be revoked or remain refreshable for enrollment-only use; a fresh
/// passkey login is required for ordinary work.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn otp_family_cannot_gain_business_authority_after_passkey_registration(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(
        &pool,
        "Refreshed Enrollment Region",
        "Refreshed Enrollment Branch",
    )
    .await;
    let user_id = seed_user_with_branch(
        &pool,
        "Refreshed Enrollment User",
        "010-4100-0007",
        "MECHANIC",
        branch_id,
    )
    .await;
    seed_equipment(&pool, branch_id, "292").await;
    let service =
        build_router(app_state(pool.clone(), private_key_pem.to_string(), public_key_pem).unwrap());
    let issue = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            &pool,
            *user_id.as_uuid(),
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::hours(1),
        )
        .await
        .unwrap();
    let enrollment: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issue.token.as_str() }),
        StatusCode::OK,
    )
    .await;
    accept_required_privacy_consent(&service, &enrollment.access_token).await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential_id =
        enroll_passkey(&service, &mut authenticator, &enrollment.access_token).await;
    let refresh = post_raw(
        service.clone(),
        "/api/v1/auth/token/refresh",
        None,
        json!({ "refresh_token": enrollment.refresh_token.as_ref().unwrap() }),
    )
    .await;
    match refresh.status() {
        StatusCode::OK => {
            let refreshed: TokenPairResponse = refresh.into_json(StatusCode::OK).await;
            let response = post_raw(
                service.clone(),
                "/api/work-orders",
                Some(&refreshed.access_token),
                json!({ "branch_id": branch_id, "management_no": "#292", "symptom": "Hydraulic oil leak" }),
            )
            .await;
            assert!(
                matches!(
                    response.status(),
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                ),
                "refreshing an OTP family must not grant business authority, got {}",
                response.status()
            );
        }
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {}
        other => panic!("OTP family refresh returned unexpected status {other}"),
    }
    let work_orders: i64 =
        sqlx::query_scalar("SELECT count(*) FROM work_orders WHERE branch_id=$1")
            .bind(branch_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(work_orders, 0);

    let normal = usernameless_login(&service, &mut authenticator, &credential_id).await;
    let created = post_raw(
        service,
        "/api/work-orders",
        Some(&normal.access_token),
        json!({ "branch_id": branch_id, "management_no": "#292", "symptom": "Hydraulic oil leak" }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
}

/// The admin issue-OTP endpoint is authz-gated: a non-admin session is forbidden.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn admin_issue_otp_rejects_non_admin(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "AZ Region", "AZ Branch").await;
    let mechanic_id = seed_user_with_branch(
        &pool,
        "Plain Mechanic",
        "010-5000-0001",
        "MECHANIC",
        branch_id,
    )
    .await;
    let target_id =
        seed_user_with_branch(&pool, "Target User", "010-5000-0002", "MECHANIC", branch_id).await;

    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );

    // A mechanic signs in via OTP and tries to issue a code -> 403.
    let mechanic_access = admin_session_via_otp(&service, &pool, mechanic_id).await;
    let forbidden = post_raw(
        service.clone(),
        "/api/v1/auth/admin/otp/issue",
        Some(&mechanic_access),
        json!({ "user_id": target_id.as_uuid(), "branch_id": branch_id }),
    )
    .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    // No bearer at all -> 401.
    let unauth = post_raw(
        service,
        "/api/v1/auth/admin/otp/issue",
        None,
        json!({ "user_id": target_id.as_uuid(), "branch_id": branch_id }),
    )
    .await;
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
}

/// IDOR: a branch-A admin must NOT be able to mint a sign-in OTP for a user who
/// belongs only to branch B. Authorization is bound to the TARGET's real branch
/// scope, not the client-supplied branch_id.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn admin_issue_otp_rejects_cross_branch_target(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_a = seed_branch(&pool, "Region A", "Branch A").await;
    let branch_b = seed_branch(&pool, "Region B", "Branch B").await;
    let admin_a = seed_user_with_branch(&pool, "Admin A", "010-6000-0000", "ADMIN", branch_a).await;
    // The target belongs ONLY to branch B.
    let target_b =
        seed_user_with_branch(&pool, "User B", "010-6000-0001", "MECHANIC", branch_b).await;

    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );

    let admin_access = admin_session_via_otp(&service, &pool, admin_a).await;

    // Even when the admin lies and passes its own branch_a as branch_id, the
    // target's REAL scope (branch B) is what is authorized against -> 403.
    let forbidden = post_raw(
        service.clone(),
        "/api/v1/auth/admin/otp/issue",
        Some(&admin_access),
        json!({ "user_id": target_b.as_uuid(), "branch_id": branch_a }),
    )
    .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    // Passing the target's real branch_b also fails — admin A has no authority there.
    let forbidden_real_branch = post_raw(
        service,
        "/api/v1/auth/admin/otp/issue",
        Some(&admin_access),
        json!({ "user_id": target_b.as_uuid(), "branch_id": branch_b }),
    )
    .await;
    assert_eq!(forbidden_real_branch.status(), StatusCode::FORBIDDEN);
}

/// IDOR: a branch admin must NOT be able to mint a sign-in OTP for a privileged
/// (SUPER_ADMIN or EXECUTIVE) target. Only a SUPER_ADMIN caller may do so.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn admin_issue_otp_rejects_privileged_target(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Priv Region", "Priv Branch").await;
    let admin_id =
        seed_user_with_branch(&pool, "Branch Admin", "010-6100-0000", "ADMIN", branch_id).await;
    // A SUPER_ADMIN target that also (incidentally) belongs to the admin's branch.
    let super_admin_target = seed_user_with_branch(
        &pool,
        "Super Admin Target",
        "010-6100-0001",
        "SUPER_ADMIN",
        branch_id,
    )
    .await;
    // An EXECUTIVE target in the same branch.
    let executive_target = seed_user_with_branch(
        &pool,
        "Executive Target",
        "010-6100-0002",
        "EXECUTIVE",
        branch_id,
    )
    .await;

    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );

    let admin_access = admin_session_via_otp(&service, &pool, admin_id).await;

    let super_admin_forbidden = post_raw(
        service.clone(),
        "/api/v1/auth/admin/otp/issue",
        Some(&admin_access),
        json!({ "user_id": super_admin_target.as_uuid(), "branch_id": branch_id }),
    )
    .await;
    assert_eq!(super_admin_forbidden.status(), StatusCode::FORBIDDEN);

    let executive_forbidden = post_raw(
        service,
        "/api/v1/auth/admin/otp/issue",
        Some(&admin_access),
        json!({ "user_id": executive_target.as_uuid(), "branch_id": branch_id }),
    )
    .await;
    assert_eq!(executive_forbidden.status(), StatusCode::FORBIDDEN);
}

/// The happy path still works: a branch admin issues a code for an in-branch
/// subordinate (a non-privileged user whose only branch is the admin's).
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn admin_issue_otp_allows_in_branch_subordinate(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Sub Region", "Sub Branch").await;
    let admin_id =
        seed_user_with_branch(&pool, "Branch Admin", "010-6200-0000", "ADMIN", branch_id).await;
    let subordinate =
        seed_user_with_branch(&pool, "Subordinate", "010-6200-0001", "MECHANIC", branch_id).await;

    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );

    let admin_access = admin_session_via_otp(&service, &pool, admin_id).await;

    let issued: AdminIssueOtpResponse = post_json(
        service,
        "/api/v1/auth/admin/otp/issue",
        Some(&admin_access),
        json!({ "user_id": subordinate.as_uuid(), "branch_id": branch_id }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(&issued.user_id, subordinate.as_uuid());
    assert_eq!(issued.otp.chars().count(), 8);
}

/// Lost-device recovery: a branch admin can reset an in-branch subordinate that
/// already has a passkey. The old passkey is revoked and the fresh one-time code
/// redeems so the user can enroll a replacement device.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn admin_credential_reset_recovers_in_branch_subordinate(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Reset Region", "Reset Branch").await;
    let admin_id =
        seed_user_with_branch(&pool, "Reset Admin", "010-6300-0000", "ADMIN", branch_id).await;
    let subordinate = seed_user_with_branch(
        &pool,
        "Locked Mechanic",
        "010-6300-0001",
        "MECHANIC",
        branch_id,
    )
    .await;
    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );

    let subordinate_access = admin_session_via_otp(&service, &pool, subordinate).await;
    let mut old_authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let old_credential_id =
        enroll_passkey(&service, &mut old_authenticator, &subordinate_access).await;

    let admin_access = admin_session_via_otp(&service, &pool, admin_id).await;
    let reset: AdminCredentialResetResponse = post_json(
        service.clone(),
        "/api/v1/auth/admin/credential-reset",
        Some(&admin_access),
        json!({ "user_id": subordinate.as_uuid() }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(&reset.user_id, subordinate.as_uuid());
    assert_eq!(reset.otp.chars().count(), 8);

    let remaining_passkeys: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_webauthn_credentials WHERE user_id = $1")
            .bind(subordinate.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining_passkeys, 0);

    let login_start: LoginStartResponse = post_raw(
        service.clone(),
        "/api/v1/auth/passkey/login/start",
        None,
        json!({}),
    )
    .await
    .into_json(StatusCode::OK)
    .await;
    let challenge = inject_allow_credential(login_start.challenge, &old_credential_id);
    let assertion = old_authenticator
        .do_authentication(Url::parse(TEST_ORIGIN).unwrap(), challenge)
        .unwrap();
    let old_passkey_login = post_raw(
        service.clone(),
        "/api/v1/auth/passkey/login/finish",
        None,
        json!({ "ceremony_id": login_start.ceremony_id, "credential": assertion }),
    )
    .await;
    assert_eq!(old_passkey_login.status(), StatusCode::UNAUTHORIZED);

    let recovered: OtpRedeemResponse = post_json(
        service,
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": reset.otp }),
        StatusCode::OK,
    )
    .await;
    assert!(recovered.requires_passkey_setup);
}

/// The DB-backed per-IP rate limiter's cap/window/reset behavior is covered
/// deterministically in `auth-rest`'s own unit test
/// (`rate_limit_trips_at_cap_and_resets_after_window`), which drives `now` as
/// a synthetic clock instead of racing this HTTP-level test's real
/// round-trips against the wall clock's minute boundary — that race was the
/// CI flake (twelve sequential requests could straddle a minute and reset the
/// bucket before the cap tripped). This is the REAL-clock smoke: a handful of
/// requests through the actual HTTP path must behave normally, proving
/// `OffsetDateTime::now_utc()` still wires into `rate_limit` end-to-end, and
/// that per-IP buckets stay independent.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn otp_redeem_rate_limit_wires_up_on_real_clock_path(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    let service = build_router(
        app_state_with_trusted_proxy(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );

    // The limiter buckets into a FIXED one-minute TUMBLING window -- see
    // `floor_to_window` in crates/platform/auth-rest/src/lib.rs, which floors to
    // `unix - unix.rem_euclid(60)` -- not a sliding one. Every request below
    // must therefore land inside the SAME window: if the loop straddles a minute
    // boundary the counter resets mid-loop and the eleventh request comes back
    // 401 instead of 429.
    //
    // That is not hypothetical. This test drives eleven real DB-backed requests,
    // so on a loaded runner it occupies a meaningful fraction of the window and
    // fails whenever it starts late in one. It did exactly that on run
    // 32225725163, on a pull request that changed nothing near auth.
    //
    // The sibling test named in this test's own doc comment
    // (`rate_limit_trips_at_cap_and_resets_after_window`) drives `now` directly
    // and so has no such exposure; this one exists to prove the REAL clock path
    // is wired, so it cannot inject a clock -- but it can decline to start near
    // a boundary.
    let into_window = OffsetDateTime::now_utc().unix_timestamp().rem_euclid(60);
    const WINDOW_SECS: i64 = 60;
    // Budget generously: the eleven requests each round-trip to PostgreSQL.
    const NEEDED_SECS: i64 = 30;
    if into_window > WINDOW_SECS - NEEDED_SECS {
        let wait = (WINDOW_SECS - into_window + 1) as u64;
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
    }

    // Drive the real ingress boundary: the XFF identity is accepted only from
    // the configured trusted transport peer. The first identity exhausts its
    // own bucket while the second remains independently usable.
    for i in 0..10 {
        let response = post_raw_with_trusted_ip(
            service.clone(),
            "/api/v1/auth/otp/redeem",
            "203.0.113.7",
            json!({ "otp": "badcode1" }),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "request {i} within the first identity's cap must not be rate limited"
        );
    }

    let exhausted = post_raw_with_trusted_ip(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        "203.0.113.7",
        json!({ "otp": "badcode1" }),
    )
    .await;
    assert_eq!(
        exhausted.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the trusted ingress identity must select the first per-IP bucket"
    );

    let other_ip = post_raw_with_trusted_ip(
        service,
        "/api/v1/auth/otp/redeem",
        "203.0.113.99",
        json!({ "otp": "badcode2" }),
    )
    .await;
    assert_eq!(
        other_ip.status(),
        StatusCode::UNAUTHORIZED,
        "a second trusted ingress identity must have a separate per-IP bucket"
    );
}

/// WEB dual-transport: when `X-Auth-Transport: cookie` is present, an OTP redeem
/// sets the refresh token as an HttpOnly `console_refresh` cookie and OMITS it from
/// the JSON body, while the access token stays in the body. The cookie carries
/// the CSRF-safe attributes (HttpOnly, SameSite=Strict, Path=/api/v1/auth).
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn cookie_mode_redeem_sets_httponly_cookie_and_omits_body_refresh(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Cookie Region", "Cookie Branch").await;
    let user_id =
        seed_user_with_branch(&pool, "Web User", "010-7000-0000", "MECHANIC", branch_id).await;
    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );

    let issue = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            &pool,
            *user_id.as_uuid(),
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::hours(24),
        )
        .await
        .unwrap();

    let response = post_cookie_mode(
        service,
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issue.token.as_str() }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let set_cookie = console_refresh_set_cookie(&response)
        .expect("cookie-mode redeem must set an console_refresh cookie");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
    assert!(set_cookie.contains("SameSite=Strict"), "{set_cookie}");
    assert!(set_cookie.contains("Path=/api/v1/auth"), "{set_cookie}");
    assert!(set_cookie.contains("Max-Age="), "{set_cookie}");
    // Local-dev config leaves CONSOLE_COOKIE_SECURE at its default (true) in this
    // test harness, so Secure must be present.
    assert!(set_cookie.contains("Secure"), "{set_cookie}");
    assert!(
        !cookie_token(&set_cookie).is_empty(),
        "cookie must carry the refresh token value"
    );

    let body = body_json(response).await;
    assert!(
        !body["access_token"].as_str().unwrap().is_empty(),
        "access token must always be in the body"
    );
    assert!(
        body["refresh_token"].is_null(),
        "cookie mode must NOT leak the refresh token into the JSON body, got {body}"
    );
}

/// WEB dual-transport: passkey login finish in cookie mode sets the cookie and
/// nulls the body refresh token; the cookie value then authorizes a refresh that
/// reads the token from the cookie (no body token) and rotates the cookie.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn cookie_mode_login_then_refresh_reads_and_rotates_cookie(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Cookie Region", "Cookie Branch").await;
    let user_id =
        seed_user_with_branch(&pool, "Web User", "010-7100-0000", "MECHANIC", branch_id).await;
    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );

    // First sign-in via OTP (cookie mode) then enroll a passkey so we can do a
    // real cookie-mode passkey login.
    let issue = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            &pool,
            *user_id.as_uuid(),
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::hours(24),
        )
        .await
        .unwrap();
    let redeem = post_cookie_mode(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issue.token.as_str() }),
    )
    .await;
    assert_eq!(redeem.status(), StatusCode::OK);
    let access_token = body_json(redeem).await["access_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential_id = enroll_passkey(&service, &mut authenticator, &access_token).await;

    // Cookie-mode usernameless passkey login -> cookie set, body refresh null.
    let login = cookie_mode_usernameless_login(&service, &mut authenticator, &credential_id).await;
    let login_cookie = console_refresh_set_cookie(&login)
        .expect("cookie-mode login must set an console_refresh cookie");
    let cookie_value = cookie_token(&login_cookie).to_owned();
    let login_body = body_json(login).await;
    assert!(login_body["refresh_token"].is_null());

    // Refresh reading the token from the cookie (NO body token) rotates and sets
    // a fresh cookie whose value differs from the one presented.
    let refreshed = post_cookie_mode(
        service.clone(),
        "/api/v1/auth/token/refresh",
        Some(&cookie_value),
        json!({}),
    )
    .await;
    assert_eq!(refreshed.status(), StatusCode::OK);
    let rotated_cookie = console_refresh_set_cookie(&refreshed)
        .expect("cookie-mode refresh must rotate the console_refresh cookie");
    let rotated_value = cookie_token(&rotated_cookie).to_owned();
    assert_ne!(
        rotated_value, cookie_value,
        "the refresh token cookie must rotate on use"
    );
    assert!(body_json(refreshed).await["refresh_token"].is_null());

    // Logout in cookie mode clears the cookie (Max-Age=0) and revokes the family,
    // so the rotated cookie can no longer refresh. (Reuse-detection of the old
    // pre-rotation token is covered by the body-transport end-to-end test; here we
    // logout with the LIVE rotated cookie, the one a browser would actually hold.)
    let logout = post_cookie_mode(
        service.clone(),
        "/api/v1/auth/logout",
        Some(&rotated_value),
        json!({}),
    )
    .await;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let clear_cookie = console_refresh_set_cookie(&logout)
        .expect("logout must emit a clearing console_refresh cookie");
    assert!(clear_cookie.contains("Max-Age=0"), "{clear_cookie}");
    assert!(clear_cookie.contains("Path=/api/v1/auth"), "{clear_cookie}");

    let after_logout = post_cookie_mode(
        service,
        "/api/v1/auth/token/refresh",
        Some(&rotated_value),
        json!({}),
    )
    .await;
    assert_eq!(after_logout.status(), StatusCode::UNAUTHORIZED);
}

/// Browser hard navigations drop the in-memory access token and rebuild the
/// session from the HttpOnly refresh cookie on each document load. That normal
/// pattern must have a wider refresh budget than OTP/passkey credential
/// submission, while still retaining a bounded per-device refresh limiter.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn cookie_mode_refresh_allows_rapid_navigation_burst_with_device_id(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Refresh Nav Region", "Refresh Nav Branch").await;
    let user_id = seed_user_with_branch(
        &pool,
        "Refresh Nav User",
        "010-7500-0000",
        "SUPER_ADMIN",
        branch_id,
    )
    .await;
    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );

    let issue = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            &pool,
            *user_id.as_uuid(),
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::hours(24),
        )
        .await
        .unwrap();
    let redeem = post_cookie_mode(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issue.token.as_str() }),
    )
    .await;
    assert_eq!(redeem.status(), StatusCode::OK);
    let access_token = body_json(redeem).await["access_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential_id = enroll_passkey(&service, &mut authenticator, &access_token).await;

    let login = cookie_mode_usernameless_login(&service, &mut authenticator, &credential_id).await;
    let login_cookie = console_refresh_set_cookie(&login)
        .expect("cookie-mode login must set an console_refresh cookie");
    let mut cookie_value = cookie_token(&login_cookie).to_owned();

    for attempt in 1..=12 {
        let refreshed = post_cookie_mode_with_device_id(
            service.clone(),
            "/api/v1/auth/token/refresh",
            Some(&cookie_value),
            "browser-nav-device-01",
            json!({}),
        )
        .await;
        assert_eq!(
            refreshed.status(),
            StatusCode::OK,
            "refresh attempt {attempt} should stay within the normal browser navigation budget"
        );
        let rotated_cookie = console_refresh_set_cookie(&refreshed)
            .expect("cookie-mode refresh must rotate the console_refresh cookie");
        cookie_value = cookie_token(&rotated_cookie).to_owned();
        assert!(body_json(refreshed).await["refresh_token"].is_null());
    }
}

/// MOBILE (no transport header) is unchanged: refresh and logout read the token
/// from the request BODY, the response carries the refresh token in the body, and
/// NO Set-Cookie header is emitted.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn body_mode_without_header_is_unchanged_and_sets_no_cookie(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let branch_id = seed_branch(&pool, "Mobile Region", "Mobile Branch").await;
    let user_id =
        seed_user_with_branch(&pool, "Mobile User", "010-7200-0000", "MECHANIC", branch_id).await;
    let service = build_router(
        app_state(
            pool.clone(),
            private_key_pem.to_string(),
            public_key_pem.clone(),
        )
        .unwrap(),
    );

    let issue = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            &pool,
            *user_id.as_uuid(),
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::hours(24),
        )
        .await
        .unwrap();

    // Redeem with NO transport header -> body carries the refresh token, no cookie.
    let redeem = post_raw(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issue.token.as_str() }),
    )
    .await;
    assert_eq!(redeem.status(), StatusCode::OK);
    assert!(
        set_cookie_values(&redeem).is_empty(),
        "mobile/body mode must not set any cookie"
    );
    let redeem_body = body_json(redeem).await;
    let refresh_token = redeem_body["refresh_token"]
        .as_str()
        .expect("body mode must return the refresh token in the JSON body")
        .to_owned();

    // Body-mode refresh rotates using the body token and still returns no cookie.
    let refreshed = post_raw(
        service.clone(),
        "/api/v1/auth/token/refresh",
        None,
        json!({ "refresh_token": refresh_token }),
    )
    .await;
    assert_eq!(refreshed.status(), StatusCode::OK);
    assert!(
        set_cookie_values(&refreshed).is_empty(),
        "mobile/body mode refresh must not set any cookie"
    );
    let rotated = body_json(refreshed).await["refresh_token"]
        .as_str()
        .expect("body-mode refresh must return the new refresh token in the body")
        .to_owned();
    assert_ne!(rotated, refresh_token);

    // Body-mode logout accepts the body token and revokes the family.
    let logout = post_raw(
        service.clone(),
        "/api/v1/auth/logout",
        None,
        json!({ "refresh_token": rotated }),
    )
    .await;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert!(
        set_cookie_values(&logout).is_empty(),
        "mobile/body mode logout must not set any cookie"
    );
}

// --- helpers ---------------------------------------------------------------

/// Cookie-mode usernameless passkey login: mirrors `usernameless_login` but sends
/// the `X-Auth-Transport: cookie` header so the response carries a Set-Cookie and
/// a null body refresh token. Returns the raw response for header + body asserts.
async fn cookie_mode_usernameless_login(
    service: &axum::Router,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
    credential_id: &str,
) -> http::Response<Body> {
    let start: LoginStartResponse = post_raw(
        service.clone(),
        "/api/v1/auth/passkey/login/start",
        None,
        json!({}),
    )
    .await
    .into_json(StatusCode::OK)
    .await;
    let challenge = inject_allow_credential(start.challenge, credential_id);
    let assertion = authenticator
        .do_authentication(Url::parse(TEST_ORIGIN).unwrap(), challenge)
        .unwrap();
    post_cookie_mode(
        service.clone(),
        "/api/v1/auth/passkey/login/finish",
        None,
        json!({ "ceremony_id": start.ceremony_id, "credential": assertion }),
    )
    .await
}

async fn start_step_up_assertion(
    service: &axum::Router,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
    credential_id: &str,
) -> Value {
    let start: LoginStartResponse = post_raw(
        service.clone(),
        "/api/v1/auth/passkey/login/start",
        None,
        json!({}),
    )
    .await
    .into_json(StatusCode::OK)
    .await;
    let challenge = inject_allow_credential(start.challenge, credential_id);
    let assertion = authenticator
        .do_authentication(Url::parse(TEST_ORIGIN).unwrap(), challenge)
        .unwrap();

    json!({
        "ceremony_id": start.ceremony_id,
        "credential": assertion
    })
}

/// Sign a user in via a directly-issued OTP (used to bootstrap an authenticated
/// session for any role in tests without a pre-existing passkey).
async fn admin_session_via_otp(service: &axum::Router, pool: &PgPool, user_id: UserId) -> String {
    let issue = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            pool,
            *user_id.as_uuid(),
            OrgId::knl(),
            OffsetDateTime::now_utc(),
            Duration::hours(24),
        )
        .await
        .unwrap();
    let redeem: OtpRedeemResponse = post_json(
        service.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({ "otp": issue.token.as_str() }),
        StatusCode::OK,
    )
    .await;
    redeem.access_token
}

/// Enroll a passkey and return its credential id (base64url string).
async fn enroll_passkey(
    service: &axum::Router,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
    access_token: &str,
) -> String {
    accept_required_privacy_consent(service, access_token).await;
    let registration: RegisterStartResponse = post_json(
        service.clone(),
        "/api/v1/auth/passkey/register/start",
        Some(access_token),
        json!({ "username": "new.user", "display_name": "New User" }),
        StatusCode::OK,
    )
    .await;
    let credential = authenticator
        .do_registration(Url::parse(TEST_ORIGIN).unwrap(), registration.challenge)
        .unwrap();
    let finish: RegisterFinishResponse = post_json(
        service.clone(),
        "/api/v1/auth/passkey/register/finish",
        Some(access_token),
        json!({ "ceremony_id": registration.ceremony_id, "credential": credential }),
        StatusCode::CREATED,
    )
    .await;
    finish.credential_id
}

async fn accept_required_privacy_consent(
    service: &axum::Router,
    access_token: &str,
) -> PrivacyConsentStatusResponse {
    let status: PrivacyConsentStatusResponse = post_json(
        service.clone(),
        "/api/v1/auth/privacy-consent/status",
        Some(access_token),
        json!({}),
        StatusCode::OK,
    )
    .await;
    if status.accepted {
        return status;
    }

    let accepted: PrivacyConsentStatusResponse = post_json(
        service.clone(),
        "/api/v1/auth/privacy-consent/accept",
        Some(access_token),
        json!({
            "policy_version": status.policy_version,
            "privacy_collection": true,
            "terms_of_service": true
        }),
        StatusCode::OK,
    )
    .await;
    assert!(accepted.accepted);
    assert!(
        accepted.accepted_at.is_some(),
        "accepted consent must record an audit timestamp"
    );
    accepted
}

async fn usernameless_login(
    service: &axum::Router,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
    credential_id: &str,
) -> TokenPairResponse {
    // login/start takes NO body and NO user_id; the server returns a discoverable
    // challenge with an EMPTY allowCredentials list.
    let start: LoginStartResponse = post_raw(
        service.clone(),
        "/api/v1/auth/passkey/login/start",
        None,
        json!({}),
    )
    .await
    .into_json(StatusCode::OK)
    .await;

    // The SoftPasskey harness cannot resolve a resident credential from an empty
    // allowCredentials (it has no resident-key store), so the test injects the
    // known credential id to emulate what a real discoverable authenticator does
    // internally. The SERVER ceremony stays fully discoverable — see the report's
    // SoftPasskey compromise note. The returned assertion still carries the
    // credential id, which is what the server resolves the user by.
    let challenge = inject_allow_credential(start.challenge, credential_id);
    let assertion = authenticator
        .do_authentication(Url::parse(TEST_ORIGIN).unwrap(), challenge)
        .unwrap();

    post_json(
        service.clone(),
        "/api/v1/auth/passkey/login/finish",
        None,
        json!({ "ceremony_id": start.ceremony_id, "credential": assertion }),
        StatusCode::OK,
    )
    .await
}

async fn start_mobile_step_up_assertion(
    service: &axum::Router,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
    credential_id: &str,
    access_token: &str,
    binding: Value,
) -> Value {
    let start = post_json::<Value>(
        service.clone(),
        "/api/v1/auth/passkey/step-up/start",
        Some(access_token),
        json!({ "binding": binding.clone() }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(start["binding"], binding);

    let ceremony_id = start["ceremony_id"]
        .as_str()
        .expect("step-up start must return ceremony id")
        .to_owned();
    let challenge: RequestChallengeResponse =
        serde_json::from_value(start["challenge"].clone()).unwrap();
    let challenge = inject_allow_credential(challenge, credential_id);
    let assertion = authenticator
        .do_authentication(Url::parse(TEST_ORIGIN).unwrap(), challenge)
        .unwrap();

    json!({
        "binding": binding,
        "assertion": {
            "ceremony_id": ceremony_id,
            "credential": assertion
        }
    })
}

async fn assert_persisted_mobile_step_up_binding(pool: &PgPool, step_up: &Value) {
    let ceremony_id = Uuid::parse_str(
        step_up["assertion"]["ceremony_id"]
            .as_str()
            .expect("step-up assertion must carry ceremony id"),
    )
    .unwrap();
    let row: (String, Uuid, String, Option<i32>) = sqlx::query_as(
        r#"
        SELECT action_kind, object_id, reason_key, replay_attempt
        FROM auth_webauthn_ceremony_bindings
        WHERE ceremony_id = $1
        "#,
    )
    .bind(ceremony_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(row.0, step_up["binding"]["action_kind"]);
    assert_eq!(row.1.to_string(), step_up["binding"]["object_id"]);
    assert_eq!(row.2, step_up["binding"]["reason_key"]);
    assert_eq!(
        row.3,
        step_up["binding"]["replay_attempt"]
            .as_i64()
            .map(|value| value as i32)
    );
}

#[derive(Clone, Copy)]
struct FinancialStepUpFixture {
    branch_id: BranchId,
    requester: UserId,
    receptionist: UserId,
    admin: UserId,
    equipment: EquipmentId,
    work_order: WorkOrderId,
    statement_evidence: EvidenceId,
}

async fn seed_financial_step_up_fixture(
    pool: &PgPool,
    branch_id: BranchId,
    requester: UserId,
    receptionist: UserId,
    admin: UserId,
) -> FinancialStepUpFixture {
    let equipment = seed_financial_step_up_equipment(pool, branch_id).await;
    let work_order =
        seed_financial_step_up_work_order(pool, branch_id, receptionist, equipment).await;
    let statement_evidence = seed_financial_step_up_statement(pool, work_order, requester).await;
    FinancialStepUpFixture {
        branch_id,
        requester,
        receptionist,
        admin,
        equipment,
        work_order,
        statement_evidence,
    }
}

async fn submitted_financial_purchase(
    pool: &PgPool,
    fixture: FinancialStepUpFixture,
    amount_won: i64,
) -> PurchaseRequestId {
    let purchase_id = create_financial_purchase(pool, fixture, amount_won).await;
    let pool = pool.clone();
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        PgFinancialStore::new(pool)
            .submit_purchase_request(PurchaseSubmitCommand {
                actor: fixture.receptionist,
                purchase_request_id: purchase_id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
    })
    .await;
    purchase_id
}

async fn admin_approved_financial_purchase(
    pool: &PgPool,
    fixture: FinancialStepUpFixture,
    amount_won: i64,
) -> PurchaseRequestId {
    let purchase_id = submitted_financial_purchase(pool, fixture, amount_won).await;
    let pool = pool.clone();
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        PgFinancialStore::new(pool)
            .approve_purchase_admin(PurchaseApprovalCommand {
                actor: fixture.admin,
                purchase_request_id: purchase_id,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
    })
    .await;
    purchase_id
}

async fn executive_pending_financial_purchase(
    pool: &PgPool,
    fixture: FinancialStepUpFixture,
) -> PurchaseRequestId {
    let purchase_id = admin_approved_financial_purchase(pool, fixture, 3_000_000).await;
    let pool = pool.clone();
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        PgFinancialStore::new(pool)
            .prepare_expenditure(PrepareExpenditureCommand {
                actor: fixture.admin,
                purchase_request_id: purchase_id,
                expenditure_no: format!("EXP-{}", Uuid::new_v4()),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
    })
    .await;
    purchase_id
}

async fn ready_to_execute_financial_purchase(
    pool: &PgPool,
    fixture: FinancialStepUpFixture,
) -> PurchaseRequestId {
    let purchase_id = admin_approved_financial_purchase(pool, fixture, 900_000).await;
    let pool = pool.clone();
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        PgFinancialStore::new(pool)
            .prepare_expenditure(PrepareExpenditureCommand {
                actor: fixture.admin,
                purchase_request_id: purchase_id,
                expenditure_no: format!("EXP-{}", Uuid::new_v4()),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
    })
    .await;
    purchase_id
}

async fn create_financial_purchase(
    pool: &PgPool,
    fixture: FinancialStepUpFixture,
    amount_won: i64,
) -> PurchaseRequestId {
    let pool = pool.clone();
    console_platform_request_context::scope_org(OrgId::knl(), async move {
        let purchase = PgFinancialStore::new(pool)
            .create_purchase_request(CreatePurchaseRequestCommand {
                actor: fixture.requester,
                branch_id: fixture.branch_id,
                equipment_id: Some(fixture.equipment),
                work_order_id: Some(fixture.work_order),
                statement_evidence_id: Some(fixture.statement_evidence),
                purchase_type: PurchaseType::LegacyManual,
                vendor_name: "Financial Step-Up Vendor".to_owned(),
                amount_won: Some(amount_won),
                lines: vec![financial_step_up_purchase_line(amount_won)],
                quote_attachment_ids: Vec::new(),
                memo: "financial step-up fixture".to_owned(),
                config: financial_step_up_config(),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap();
        purchase.id
    })
    .await
}

fn financial_step_up_config() -> FinancialConfigSnapshot {
    FinancialConfigSnapshot {
        depreciation_method: DepreciationMethod::StraightLine,
        useful_life_months: 60,
        residual_rate_bps: 1_000,
        declining_balance_rate_bps: 2_000,
        management_fee_rate_bps: 1_000,
        profit_rate_bps: 500,
        floor_negative_quote_residual: true,
        executive_approval_threshold_won: 2_000_000,
    }
}

fn financial_step_up_purchase_line(amount_won: i64) -> PurchaseRequestLineInput {
    PurchaseRequestLineInput {
        item: "step-up protected purchase".to_owned(),
        quantity: 1,
        unit_supply_price_won: amount_won,
        vat_won: Some(0),
    }
}

#[allow(clippy::too_many_arguments)]
async fn assert_financial_step_up_denied(
    service: axum::Router,
    pool: &PgPool,
    access_token: &str,
    path: &str,
    body: Value,
    purchase_request_id: PurchaseRequestId,
    unchanged_status: &str,
    mutation_action: &str,
    required_action: &str,
    expected_status: StatusCode,
    expected_code: &str,
    expected_failure_reason: &str,
) {
    let mutation_count_before =
        financial_audit_count(pool, mutation_action, purchase_request_id).await;
    let denial_count_before =
        financial_audit_count(pool, "purchase.step_up.denied", purchase_request_id).await;
    let rejected = post_raw(service, path, Some(access_token), body).await;
    assert_eq!(rejected.status(), expected_status);
    assert_eq!(body_json(rejected).await["error"]["code"], expected_code);
    assert_purchase_status(pool, purchase_request_id, unchanged_status).await;
    assert_financial_audit_count(
        pool,
        mutation_action,
        purchase_request_id,
        mutation_count_before,
    )
    .await;
    assert_financial_audit_count(
        pool,
        "purchase.step_up.denied",
        purchase_request_id,
        denial_count_before + 1,
    )
    .await;
    let after = latest_financial_step_up_denial_after(pool, purchase_request_id).await;
    assert_eq!(after["required_action"], required_action);
    assert_eq!(after["failure_code"], expected_code);
    assert_eq!(after["failure_reason"], expected_failure_reason);
    assert_eq!(after["step_up_verified"], false);
    assert!(after.get("credential").is_none());
    assert!(after.get("ceremony_id").is_none());
}

async fn assert_purchase_status(
    pool: &PgPool,
    purchase_request_id: PurchaseRequestId,
    expected: &str,
) {
    let status: String =
        sqlx::query_scalar("SELECT status FROM financial_purchase_requests WHERE id = $1")
            .bind(*purchase_request_id.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        status, expected,
        "unexpected status for {purchase_request_id}"
    );
}

async fn assert_financial_audit_count(
    pool: &PgPool,
    action: &str,
    purchase_request_id: PurchaseRequestId,
    expected: i64,
) {
    let count = financial_audit_count(pool, action, purchase_request_id).await;
    assert_eq!(
        count, expected,
        "unexpected audit count for {action} on {purchase_request_id}"
    );
}

async fn financial_audit_count(
    pool: &PgPool,
    action: &str,
    purchase_request_id: PurchaseRequestId,
) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM audit_events WHERE action = $1 AND target_id = $2",
    )
    .bind(action)
    .bind(purchase_request_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn latest_financial_step_up_denial_after(
    pool: &PgPool,
    purchase_request_id: PurchaseRequestId,
) -> Value {
    sqlx::query_scalar::<_, Option<Value>>(
        r#"
        SELECT after_snap
        FROM audit_events
        WHERE action = 'purchase.step_up.denied'
          AND target_id = $1
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(purchase_request_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
    .expect("step-up denial audit must include an after snapshot")
}

async fn seed_financial_step_up_equipment(pool: &PgPool, branch_id: BranchId) -> EquipmentId {
    let customer_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind("Financial Step-Up Customer")
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind("Financial Step-Up Site")
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let equipment_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, model, vehicle_value, residual_value,
            asset_registered_on, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, $4, $5,
                'A', 'B', 'C', '임대', '좌식', '2.5T', 'GTS25DE',
                12000000, 9000000, DATE '2024-01-01', 'financial-step-up-test', 1, $6)
        RETURNING id
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind("FST12-4200")
    .bind("FST-4200")
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    EquipmentId::from_uuid(equipment_id)
}

async fn seed_financial_step_up_work_order(
    pool: &PgPool,
    branch_id: BranchId,
    requested_by: UserId,
    equipment_id: EquipmentId,
) -> WorkOrderId {
    let row: (Uuid, Uuid) =
        sqlx::query_as("SELECT customer_id, site_id FROM registry_equipment WHERE id = $1")
            .bind(*equipment_id.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let work_order_id = WorkOrderId::new();
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, symptom, org_id
        )
        VALUES ($1, '20260709-420', $2, $3, $4, $5, $6, 'RECEIVED', 'financial step-up fixture', $7)
        "#,
    )
    .bind(*work_order_id.as_uuid())
    .bind(*branch_id.as_uuid())
    .bind(*equipment_id.as_uuid())
    .bind(row.0)
    .bind(row.1)
    .bind(*requested_by.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    work_order_id
}

async fn seed_financial_step_up_statement(
    pool: &PgPool,
    work_order_id: WorkOrderId,
    uploaded_by: UserId,
) -> EvidenceId {
    let evidence_id = EvidenceId::new();
    sqlx::query(
        r#"
        INSERT INTO evidence_media (
            id, work_order_id, stage, s3_key, content_type, size_bytes,
            uploaded_by, worm_replica_status, retry_count, org_id
        )
        VALUES ($1, $2, 'REQUEST', $3, 'application/pdf', 2048, $4, 'VERIFIED', 0, $5)
        "#,
    )
    .bind(*evidence_id.as_uuid())
    .bind(*work_order_id.as_uuid())
    .bind(format!(
        "work-orders/{work_order_id}/REQUEST/{evidence_id}.pdf"
    ))
    .bind(*uploaded_by.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    evidence_id
}

/// Inject one `allowCredentials` entry into a discoverable challenge so the
/// SoftPasskey harness can locate its key. Emulates resident-credential
/// discovery; production never does this.
fn inject_allow_credential(
    challenge: RequestChallengeResponse,
    credential_id: &str,
) -> RequestChallengeResponse {
    let mut value = serde_json::to_value(&challenge).unwrap();
    let allow = value
        .get_mut("publicKey")
        .and_then(|pk| pk.get_mut("allowCredentials"))
        .and_then(Value::as_array_mut)
        .expect("discoverable challenge must have an allowCredentials array");
    allow.push(json!({ "type": "public-key", "id": credential_id }));
    serde_json::from_value(value).unwrap()
}

trait ResponseExt {
    async fn into_json<T: for<'de> Deserialize<'de>>(self, expected: StatusCode) -> T;
}

impl ResponseExt for http::Response<Body> {
    async fn into_json<T: for<'de> Deserialize<'de>>(self, expected: StatusCode) -> T {
        assert_eq!(self.status(), expected);
        let bytes = to_bytes(self.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
}

async fn post_json<T>(
    service: axum::Router,
    uri: &str,
    bearer: Option<&str>,
    body: Value,
    expected: StatusCode,
) -> T
where
    T: for<'de> Deserialize<'de>,
{
    let response = post_raw(service, uri, bearer, body).await;
    assert_eq!(response.status(), expected);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn post_raw(
    service: axum::Router,
    uri: &str,
    bearer: Option<&str>,
    body: Value,
) -> http::Response<Body> {
    let mut builder = Request::builder()
        .uri(uri)
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    service
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

async fn post_raw_with_trusted_ip(
    service: axum::Router,
    uri: &str,
    ip: &str,
    body: Value,
) -> http::Response<Body> {
    let mut request = Request::builder()
        .uri(uri)
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-forwarded-for", ip)
        .body(Body::from(body.to_string()))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo("10.0.0.3:443".parse::<SocketAddr>().unwrap()));
    service.oneshot(request).await.unwrap()
}

/// POST as a WEB client: sends `X-Auth-Transport: cookie` and, optionally, a
/// `Cookie` header carrying `console_refresh=<token>` (what a browser would replay).
async fn post_cookie_mode(
    service: axum::Router,
    uri: &str,
    cookie: Option<&str>,
    body: Value,
) -> http::Response<Body> {
    let mut builder = Request::builder()
        .uri(uri)
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-auth-transport", "cookie");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, format!("console_refresh={cookie}"));
    }
    service
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

/// Same as `post_cookie_mode`, but includes a browser `X-Device-Id` header so
/// tests exercise the per-device auth rate-limit bucket that the web console
/// sends on every request.
async fn post_cookie_mode_with_device_id(
    service: axum::Router,
    uri: &str,
    cookie: Option<&str>,
    device_id: &str,
    body: Value,
) -> http::Response<Body> {
    let mut builder = Request::builder()
        .uri(uri)
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-auth-transport", "cookie")
        .header("x-device-id", device_id);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, format!("console_refresh={cookie}"));
    }
    service
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

/// Collect every `Set-Cookie` header value off a response as owned strings.
fn set_cookie_values(response: &http::Response<Body>) -> Vec<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .collect()
}

/// Find the `console_refresh` Set-Cookie attribute string, returning its full
/// directive (e.g. `console_refresh=abc; HttpOnly; SameSite=Strict; ...`).
fn console_refresh_set_cookie(response: &http::Response<Body>) -> Option<String> {
    set_cookie_values(response)
        .into_iter()
        .find(|value| value.starts_with("console_refresh="))
}

/// Pull the cookie's value (the substring between `console_refresh=` and the first `;`).
fn cookie_token(set_cookie: &str) -> &str {
    set_cookie
        .strip_prefix("console_refresh=")
        .and_then(|rest| rest.split(';').next())
        .unwrap()
}

async fn body_json(response: http::Response<Body>) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn app_state_with_trusted_proxy(
    pool: PgPool,
    private_key_pem: String,
    public_key_pem: String,
) -> Result<AppState, console_app::AppError> {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PRIVATE_KEY_PEM", private_key_pem),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key_pem),
        ("CONSOLE_WEBAUTHN_RP_ID", "example.com".to_owned()),
        ("CONSOLE_WEBAUTHN_RP_ORIGIN", TEST_ORIGIN.to_owned()),
        ("CONSOLE_WEBAUTHN_RP_NAME", "Console".to_owned()),
        ("CONSOLE_TRUSTED_PROXY_COUNT", "1".to_owned()),
        ("CONSOLE_TRUSTED_PROXY_CIDRS", "10.0.0.0/8".to_owned()),
    ])?;

    AppState::new(config, DatabaseDependency::Postgres(pool))
}

fn app_state(
    pool: PgPool,
    private_key_pem: String,
    public_key_pem: String,
) -> Result<AppState, console_app::AppError> {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.to_owned()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.to_owned()),
        ("CONSOLE_JWT_PRIVATE_KEY_PEM", private_key_pem),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key_pem),
        ("CONSOLE_WEBAUTHN_RP_ID", "example.com".to_owned()),
        ("CONSOLE_WEBAUTHN_RP_ORIGIN", TEST_ORIGIN.to_owned()),
        ("CONSOLE_WEBAUTHN_RP_NAME", "Console".to_owned()),
    ])?;

    AppState::new(config, DatabaseDependency::Postgres(pool))
}

async fn seed_branch(pool: &PgPool, region_name: &str, branch_name: &str) -> BranchId {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(region_name)
            .bind(*OrgId::knl().as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(branch_name)
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user_with_branch(
    pool: &PgPool,
    display_name: &str,
    phone: &str,
    role: &str,
    branch_id: BranchId,
) -> UserId {
    let user_id = UserId::new();
    sqlx::query(
        "INSERT INTO users (id, display_name, phone, roles, org_id) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(*user_id.as_uuid())
    .bind(display_name)
    .bind(phone)
    .bind(Vec::from([role]))
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*user_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    user_id
}

async fn seed_equipment(pool: &PgPool, branch_id: BranchId, management_no: &str) {
    let equipment_suffix = format!("{:0>4}", management_no);
    let customer_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(format!("Customer {management_no}"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let site_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(format!("Site {management_no}"))
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO registry_equipment (
            branch_id, customer_id, site_id, equipment_no, management_no,
            manufacturer_code, kind_code, power_code, status,
            specification, ton_text, model, source_sheet, source_row, org_id
        )
        VALUES ($1, $2, $3, $4, $5,
                'A', 'B', 'C', '임대', '좌식', '2.5', 'GTS25DE', 'test', 1, $6)
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(customer_id)
    .bind(site_id)
    .bind(format!("ABC12-{equipment_suffix}"))
    .bind(management_no)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_mobile_step_up_work_order(
    pool: &PgPool,
    branch_id: BranchId,
    receptionist: UserId,
    mechanic: UserId,
    admin: UserId,
    executive: UserId,
) -> Uuid {
    let (equipment_id, customer_id, site_id): (Uuid, Uuid, Uuid) = sqlx::query_as(
        r#"
        SELECT id, customer_id, site_id
        FROM registry_equipment
        WHERE branch_id = $1 AND org_id = $2
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(*branch_id.as_uuid())
    .bind(*OrgId::knl().as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    let work_order_id = Uuid::new_v4();
    let submitted_at = OffsetDateTime::now_utc() - Duration::hours(1);
    sqlx::query(
        r#"
        INSERT INTO work_orders (
            id, request_no, branch_id, equipment_id, customer_id, site_id,
            requested_by, status, priority, symptom, result_type, diagnosis,
            action_taken, target_due_at, report_submitted_by, report_submitted_at,
            created_at, updated_at, org_id
        )
        VALUES (
            $1, '20260709-410', $2, $3, $4, $5, $6, 'REPORT_SUBMITTED', 'P1',
            'Mobile step-up fixture', 'COMPLETED', 'diagnosis', 'action taken',
            $7, $8, $9, $9, $9, $10
        )
        "#,
    )
    .bind(work_order_id)
    .bind(*branch_id.as_uuid())
    .bind(equipment_id)
    .bind(customer_id)
    .bind(site_id)
    .bind(*receptionist.as_uuid())
    .bind(submitted_at + Duration::days(1))
    .bind(*mechanic.as_uuid())
    .bind(submitted_at)
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO work_order_assignments (work_order_id, mechanic_id, role, assigned_at, org_id)
        VALUES ($1, $2, 'PRIMARY', $3, $4)
        "#,
    )
    .bind(work_order_id)
    .bind(*mechanic.as_uuid())
    .bind(submitted_at - Duration::hours(1))
    .bind(*OrgId::knl().as_uuid())
    .execute(pool)
    .await
    .unwrap();
    for (step_order, role, approver_id, status, requested_at) in [
        (
            1_i16,
            "MECHANIC",
            Some(mechanic),
            "APPROVED",
            Some(submitted_at),
        ),
        (2_i16, "ADMIN", Some(admin), "PENDING", Some(submitted_at)),
        (3_i16, "EXECUTIVE", Some(executive), "NOT_STARTED", None),
    ] {
        sqlx::query(
            r#"
            INSERT INTO work_order_approval_steps (
                work_order_id, step_order, role, approver_id, status, requested_at, org_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(work_order_id)
        .bind(step_order)
        .bind(role)
        .bind(approver_id.map(|user| *user.as_uuid()))
        .bind(status)
        .bind(requested_at)
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
    }
    work_order_id
}

async fn seed_mobile_step_up_poll(pool: &PgPool, actor: UserId) -> (Uuid, Uuid) {
    let poll_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO collaboration_polls (
            id, org_id, target_scope_type, title, question, status, anonymity,
            allow_multiple, created_by, updated_by
        )
        VALUES ($1, $2, 'ORG', 'Mobile step-up poll', 'Select one', 'OPEN', 'NAMED', false, $3, $3)
        "#,
    )
    .bind(poll_id)
    .bind(*OrgId::knl().as_uuid())
    .bind(*actor.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    let option_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO collaboration_poll_options (org_id, poll_id, label, position)
        VALUES ($1, $2, 'Approve', 0)
        RETURNING id
        "#,
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(poll_id)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO collaboration_poll_options (org_id, poll_id, label, position)
        VALUES ($1, $2, 'Reject', 1)
        "#,
    )
    .bind(*OrgId::knl().as_uuid())
    .bind(poll_id)
    .execute(pool)
    .await
    .unwrap();
    (poll_id, option_id)
}

async fn assert_work_order_status(pool: &PgPool, work_order_id: Uuid, expected: &str) {
    let status: String = sqlx::query_scalar("SELECT status FROM work_orders WHERE id = $1")
        .bind(work_order_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(status, expected);
}

async fn assert_poll_vote_count(pool: &PgPool, poll_id: Uuid, expected: i64) {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM collaboration_poll_votes WHERE poll_id = $1")
            .bind(poll_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(count, expected, "unexpected vote count for poll {poll_id}");
}

async fn assert_audit_count(pool: &PgPool, action: &str, expected: i64) {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action = $1")
        .bind(action)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(count, expected, "unexpected audit count for {action}");
}
