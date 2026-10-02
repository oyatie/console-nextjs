//! Positive legacy OTP classification must survive enrollment and refresh.
//! Requests use real WebAuthn signatures and the non-owner runtime role.
use super::reset_sessions::{runtime, transaction_pid, waiting};
use super::*;
use console_platform_auth::{
    AccessClaims, AccessTokenInput, ActorSession, JwtIssuer, JwtSettings, JwtVerifier,
};
use console_platform_request_context::{
    resolve_platform_principal, resolve_principal_from_bearer_token, validate_current_claims,
};
use tokio::time::timeout;

const ATTENDANCE: &str = "/api/v1/hr/attendance-records/me";
const WAIT: std::time::Duration = std::time::Duration::from_secs(10);

struct Fixture {
    router: axum::Router,
    runtime: PgPool,
    verifier: JwtVerifier,
    issuer: JwtIssuer,
    user: UserId,
    branch: BranchId,
}

async fn fixture(owner: &PgPool, role: &str, home: OrgId, label: &str) -> Fixture {
    let key = SigningKey::random(&mut OsRng);
    let private = key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string();
    let public = key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let settings = JwtSettings {
        issuer: TEST_ISSUER.into(),
        audience: TEST_AUDIENCE.into(),
        access_token_ttl: Duration::minutes(10),
    };
    let issuer =
        JwtIssuer::from_es256_pem(settings.clone(), private.as_bytes(), public.as_bytes()).unwrap();
    let verifier = JwtVerifier::from_es256_public_pem(settings, public.as_bytes()).unwrap();
    let branch = seed_branch(
        owner,
        &format!("Region {label}"),
        &format!("Branch {label}"),
    )
    .await;
    let user = if home == OrgId::knl() {
        seed_user_with_branch(owner, "Purpose subject", "010-8900-0001", role, branch).await
    } else {
        let user = UserId::new();
        sqlx::query("INSERT INTO users(id,display_name,roles,org_id,is_active) VALUES($1,'Purpose platform subject',$2,$3,true)")
            .bind(*user.as_uuid()).bind(vec![role]).bind(*home.as_uuid()).execute(owner).await.unwrap();
        user
    };
    let rt = runtime(owner, label).await;
    let router = build_router(app_state(rt.clone(), private, public).unwrap());
    Fixture {
        router,
        runtime: rt,
        verifier,
        issuer,
        user,
        branch,
    }
}

async fn bootstrap(f: &Fixture, home: OrgId) -> OtpRedeemResponse {
    let issue = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            &f.runtime,
            *f.user.as_uuid(),
            home,
            OffsetDateTime::now_utc(),
            Duration::hours(1),
        )
        .await
        .unwrap();
    let pair: OtpRedeemResponse = post_json(
        f.router.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({"otp":issue.token.as_str()}),
        StatusCode::OK,
    )
    .await;
    assert!(pair.requires_passkey_setup);
    pair
}

async fn get(f: &Fixture, path: &str, token: &str) -> http::Response<Body> {
    f.router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

fn headers(token: &str) -> http::HeaderMap {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    headers
}

fn denied(response: &http::Response<Body>, why: &str) {
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "{why}: enrollment family must not acquire ordinary authority"
    );
}

async fn attendance_counts(owner: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM employee_attendance_records), \
        (SELECT count(*) FROM payroll_attendance_material_refs), \
        (SELECT count(*) FROM audit_events WHERE action IN ('employee_attendance.record','payroll_attendance.link'))")
        .fetch_one(owner).await.unwrap()
}

async fn auth_effects(owner: &PgPool) -> Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object( \
        'handoffs',(SELECT jsonb_agg(to_jsonb(h) ORDER BY id) FROM auth_device_login_handoffs h), \
        'families',(SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM auth_refresh_token_families f), \
        'tokens',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM auth_refresh_tokens t), \
        'keys',(SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM auth_webauthn_credentials c), \
        'sources',(SELECT jsonb_agg(to_jsonb(b) ORDER BY id) FROM auth_bootstrap_credentials b), \
        'ceremonies',(SELECT jsonb_agg(to_jsonb(w) ORDER BY id) FROM auth_webauthn_ceremonies w), \
        'audits',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM audit_events a))",
    )
    .fetch_one(owner)
    .await
    .unwrap()
}

async fn refresh(f: &Fixture, token: &str) -> TokenPairResponse {
    post_json(
        f.router.clone(),
        "/api/v1/auth/token/refresh",
        None,
        json!({"refresh_token":token}),
        StatusCode::OK,
    )
    .await
}

fn input(claims: &AccessClaims, org: OrgId, roles: Vec<String>) -> AccessTokenInput {
    AccessTokenInput {
        subject: UserId::from_uuid(Uuid::parse_str(&claims.sub).unwrap()),
        org_id: org,
        roles,
        branches: Vec::new(),
        platform: org == OrgId::platform(),
        view_as: false,
        read_only: false,
        display_name: None,
        feature_grants: Vec::new(),
        authz_subject_version: claims.authz_subject_version,
        authz_policy_version: claims.authz_policy_version,
        session_generation: claims.session_generation,
        issued_at: OffsetDateTime::now_utc(),
    }
}

fn actor(claims: &AccessClaims) -> ActorSession {
    ActorSession {
        family_id: claims.session_family_id.unwrap(),
        expires_at: claims.exp,
        home_org: OrgId::from_uuid(Uuid::parse_str(&claims.org).unwrap()),
        subject_version: claims.authz_subject_version,
        session_generation: claims.session_generation,
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn worker_attendance_requires_fresh_login_and_replays_without_effects(owner: PgPool) {
    let f = fixture(&owner, "MEMBER", OrgId::knl(), "purpose-worker").await;
    let employee = Uuid::new_v4();
    sqlx::query("INSERT INTO employees(id,org_id,company,name,source_filename,source_sheet,source_row,source_key,raw_row,source_metadata) \
        VALUES($1,$2,'검증 회사','검증 근로자','test.xlsx','직원',2,'purpose-worker','{}','{}')")
        .bind(employee).bind(*OrgId::knl().as_uuid()).execute(&owner).await.unwrap();
    sqlx::query("UPDATE users SET employee_id=$1 WHERE id=$2")
        .bind(employee)
        .bind(*f.user.as_uuid())
        .execute(&owner)
        .await
        .unwrap();
    let otp = bootstrap(&f, OrgId::knl()).await;
    let original = f.verifier.verify_access_token(&otp.access_token).unwrap();
    let body = json!({"kind":"CLOCK_IN","idempotency_key":"purpose-clock-in","note":"출근 확인"});
    denied(
        &post_raw(
            f.router.clone(),
            ATTENDANCE,
            Some(&otp.access_token),
            body.clone(),
        )
        .await,
        "before enrollment",
    );
    denied(
        &get(&f, ATTENDANCE, &otp.access_token).await,
        "ordinary read",
    );
    assert_eq!(attendance_counts(&owner).await, (0, 0, 0));
    let summaries = get(&f, "/api/v1/auth/passkeys", &otp.access_token).await;
    assert_eq!(summaries.status(), StatusCode::OK);
    assert_eq!(body_json(summaries).await, json!([]));
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = enroll_passkey(&f.router, &mut authenticator, &otp.access_token).await;
    let mut old = otp.refresh_token.unwrap();
    for _ in 0..2 {
        let pair = refresh(&f, &old).await;
        assert!(
            !pair.requires_passkey_setup,
            "setup presentation is independent of authority"
        );
        let claims = f.verifier.verify_access_token(&pair.access_token).unwrap();
        assert_eq!(claims.session_family_id, original.session_family_id);
        denied(
            &post_raw(
                f.router.clone(),
                ATTENDANCE,
                Some(&pair.access_token),
                body.clone(),
            )
            .await,
            "old family after enrollment/refresh",
        );
        assert_eq!(attendance_counts(&owner).await, (0, 0, 0));
        let own = get(&f, "/api/v1/auth/passkeys", &pair.access_token).await;
        assert_eq!(own.status(), StatusCode::OK);
        assert_eq!(body_json(own).await.as_array().unwrap().len(), 1);
        old = pair.refresh_token.unwrap();
    }
    let normal = usernameless_login(&f.router, &mut authenticator, &credential).await;
    assert_ne!(
        f.verifier
            .verify_access_token(&normal.access_token)
            .unwrap()
            .session_family_id,
        original.session_family_id
    );
    let normal = refresh(&f, normal.refresh_token.as_ref().unwrap()).await;
    let created: Value = post_json(
        f.router.clone(),
        ATTENDANCE,
        Some(&normal.access_token),
        body.clone(),
        StatusCode::OK,
    )
    .await;
    let id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
    let reference = Uuid::parse_str(created["payroll_material_ref_id"].as_str().unwrap()).unwrap();
    assert_eq!(created["employee_id"], employee.to_string());
    assert_eq!(created["payroll_link_status"], "LINKED");
    assert_eq!(created["duplicate"], false);
    let fact: (Uuid,Uuid,String,String,String) = sqlx::query_as("SELECT employee_id,actor_user_id,kind,state_after,note FROM employee_attendance_records WHERE id=$1")
        .bind(id).fetch_one(&owner).await.unwrap();
    assert_eq!(
        fact,
        (
            employee,
            *f.user.as_uuid(),
            "CLOCK_IN".into(),
            "CLOCKED_IN".into(),
            "출근 확인".into()
        )
    );
    let material: (Uuid,Uuid,String) = sqlx::query_as("SELECT attendance_record_id,employee_id,work_date::text FROM payroll_attendance_material_refs WHERE id=$1")
        .bind(reference).fetch_one(&owner).await.unwrap();
    assert_eq!(
        material,
        (id, employee, created["work_date"].as_str().unwrap().into())
    );
    let audits: Vec<(String,String,String,Uuid)> = sqlx::query_as("SELECT action,target_type,target_id,actor FROM audit_events WHERE action IN ('employee_attendance.record','payroll_attendance.link') ORDER BY action")
        .fetch_all(&owner).await.unwrap();
    assert_eq!(
        audits,
        vec![
            (
                "employee_attendance.record".into(),
                "employee_attendance_record".into(),
                id.to_string(),
                *f.user.as_uuid()
            ),
            (
                "payroll_attendance.link".into(),
                "payroll_attendance_material_ref".into(),
                reference.to_string(),
                *f.user.as_uuid()
            )
        ]
    );
    assert_eq!(attendance_counts(&owner).await, (1, 1, 2));
    let before = auth_effects(&owner).await;
    let replay: Value = post_json(
        f.router.clone(),
        ATTENDANCE,
        Some(&normal.access_token),
        body,
        StatusCode::OK,
    )
    .await;
    let mut expected = created.clone();
    expected["duplicate"] = json!(true);
    assert_eq!(replay, expected);
    assert_eq!(attendance_counts(&owner).await, (1, 1, 2));
    assert_eq!(auth_effects(&owner).await, before);
    let page: Value = get(&f, ATTENDANCE, &normal.access_token)
        .await
        .into_json(StatusCode::OK)
        .await;
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"], json!([created]));
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn retained_marker_denies_consumed_expired_revoked_and_missing_source(owner: PgPool) {
    let f = fixture(&owner, "MEMBER", OrgId::knl(), "purpose-source").await;
    let otp = bootstrap(&f, OrgId::knl()).await;
    let claims = f.verifier.verify_access_token(&otp.access_token).unwrap();
    let source: Uuid = sqlx::query_scalar(
        "SELECT source_id FROM auth_legacy_otp_family_sources WHERE family_id=$1",
    )
    .bind(claims.session_family_id.unwrap())
    .fetch_one(&owner)
    .await
    .unwrap();
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = enroll_passkey(&f.router, &mut authenticator, &otp.access_token).await;
    let consumed: bool = sqlx::query_scalar(
        "SELECT consumed_at IS NOT NULL FROM auth_bootstrap_credentials WHERE id=$1",
    )
    .bind(source)
    .fetch_one(&owner)
    .await
    .unwrap();
    assert!(consumed);
    let normal = usernameless_login(&f.router, &mut authenticator, &credential).await;
    for change in [
        None,
        Some(
            "UPDATE auth_bootstrap_credentials SET issued_at=clock_timestamp()-interval '2 hours',expires_at=clock_timestamp()-interval '1 hour' WHERE id=$1",
        ),
        Some("UPDATE auth_bootstrap_credentials SET revoked_at=clock_timestamp() WHERE id=$1"),
        Some("DELETE FROM auth_bootstrap_credentials WHERE id=$1"),
    ] {
        if let Some(sql) = change {
            sqlx::query(sql).bind(source).execute(&owner).await.unwrap();
        }
        let before = auth_effects(&owner).await;
        denied(
            &get(&f, ATTENDANCE, &otp.access_token).await,
            "retained classification independent of source state",
        );
        assert_eq!(auth_effects(&owner).await, before);
        assert_eq!(
            get(&f, ATTENDANCE, &normal.access_token).await.status(),
            StatusCode::OK
        );
    }
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_legacy_otp_family_sources WHERE family_id=$1",
    )
    .bind(claims.session_family_id.unwrap())
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(retained, 1);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn mismatched_marker_cannot_disappear_from_admission(owner: PgPool) {
    let f = fixture(&owner, "MEMBER", OrgId::knl(), "purpose-malformed").await;
    let otp = bootstrap(&f, OrgId::knl()).await;
    let claims = f.verifier.verify_access_token(&otp.access_token).unwrap();
    sqlx::query("UPDATE auth_legacy_otp_family_sources SET user_id=$1 WHERE family_id=$2")
        .bind(Uuid::new_v4())
        .bind(claims.session_family_id.unwrap())
        .execute(&owner)
        .await
        .unwrap();
    let before = auth_effects(&owner).await;
    denied(
        &get(&f, ATTENDANCE, &otp.access_token).await,
        "same-Company malformed marker",
    );
    assert_eq!(auth_effects(&owner).await, before);
    assert_eq!(attendance_counts(&owner).await, (0, 0, 0));
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn classification_lookup_failure_fails_closed_and_restores_without_promotion(owner: PgPool) {
    let f = fixture(&owner, "MEMBER", OrgId::knl(), "purpose-unavailable").await;
    let access = authenticated_session(&f.router, &f.runtime, f.user).await;
    assert_eq!(get(&f, ATTENDANCE, &access).await.status(), StatusCode::OK);
    sqlx::query("REVOKE SELECT ON auth_legacy_otp_family_sources FROM console_rt")
        .execute(&owner)
        .await
        .unwrap();
    let before = auth_effects(&owner).await;
    let unavailable = get(&f, ATTENDANCE, &access).await;
    assert_eq!(
        unavailable.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "missing classification is unavailable authority, not unmarked compatibility"
    );
    assert_eq!(auth_effects(&owner).await, before);
    sqlx::query("GRANT SELECT ON auth_legacy_otp_family_sources TO console_rt")
        .execute(&owner)
        .await
        .unwrap();
    assert_eq!(get(&f, ATTENDANCE, &access).await.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn dormant_v1_normal_family_is_denied_by_both_admission_paths(owner: PgPool) {
    let f = fixture(&owner, "MEMBER", OrgId::knl(), "purpose-v1").await;
    let normal = authenticated_session(&f.router, &f.runtime, f.user).await;
    let claims = f.verifier.verify_access_token(&normal).unwrap();
    let family = Uuid::new_v4();
    sqlx::query("INSERT INTO auth_refresh_token_families(id,user_id,org_id,provenance_version,auth_generation,session_purpose,source_kind,source_operation_id) \
        VALUES($1,$2,$3,1,1,'normal','passkey',$4)")
        .bind(family).bind(*f.user.as_uuid()).bind(*OrgId::knl().as_uuid()).bind(Uuid::new_v4()).execute(&owner).await.unwrap();
    let token = f
        .issuer
        .issue_session_access_token(
            input(&claims, OrgId::knl(), claims.roles.clone()),
            None,
            Vec::new(),
            family,
            OffsetDateTime::now_utc() + Duration::minutes(10),
        )
        .unwrap();
    let before = auth_effects(&owner).await;
    denied(
        &get(&f, ATTENDANCE, &token).await,
        "dormant ordinary provenance",
    );
    denied(
        &post_raw(
            f.router.clone(),
            "/api/v1/auth/privacy-consent/status",
            Some(&token),
            json!({}),
        )
        .await,
        "dormant enrollment admission",
    );
    assert_eq!(auth_effects(&owner).await, before);
    assert_eq!(
        get(&f, ATTENDANCE, &normal).await.status(),
        StatusCode::OK,
        "unmarked v0 retains inherited compatibility"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn dormant_v1_enrollment_command_cannot_release_http_authority(owner: PgPool) {
    use console_platform_provisioning::{FirstEnrollmentCommandPool, FirstEnrollmentOutcome};
    use sha2::{Digest, Sha256};
    use sqlx::postgres::PgPoolOptions;

    let f = fixture(&owner, "MEMBER", OrgId::knl(), "purpose-v1-enrollment").await;
    let otp = format!("test-first-{}", Uuid::new_v4());
    let now = OffsetDateTime::now_utc();
    sqlx::query("INSERT INTO auth_bootstrap_credentials(id,user_id,token_hash,issued_at,expires_at,org_id,issuance_version,issued_generation,issuance_purpose,source_operation_id) \
        VALUES($1,$2,$3,$4,$5,$6,1,1,'first_enrollment',$7)")
        .bind(Uuid::new_v4()).bind(*f.user.as_uuid()).bind(Sha256::digest(otp.as_bytes()).to_vec())
        .bind(now).bind(now+Duration::hours(1)).bind(*OrgId::knl().as_uuid()).bind(Uuid::new_v4())
        .execute(&owner).await.unwrap();
    let command_pool = PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_auth_cmd")
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(owner.connect_options().as_ref().clone())
        .await
        .unwrap();
    let identity: (String, bool, bool) = sqlx::query_as(
        "SELECT current_user::text,rolsuper,rolbypassrls FROM pg_roles WHERE rolname=current_user",
    )
    .fetch_one(&command_pool)
    .await
    .unwrap();
    assert_eq!(identity, ("console_auth_cmd".into(), false, false));
    let outcome = FirstEnrollmentCommandPool::new(command_pool)
        .admit(
            &otp,
            Uuid::new_v4(),
            Sha256::digest(Uuid::new_v4().as_bytes()).into(),
            now,
        )
        .await
        .unwrap();
    let (family, deadline) = match outcome {
        FirstEnrollmentOutcome::LocallyCommitted {
            refresh,
            enrollment_expires_at,
            ..
        } => (refresh.family_id, enrollment_expires_at),
        FirstEnrollmentOutcome::AlreadyCommitted { .. } => panic!("fresh operation must commit"),
    };
    let input = AccessTokenInput {
        subject: f.user,
        org_id: OrgId::knl(),
        roles: vec!["MEMBER".into()],
        branches: vec![f.branch],
        platform: false,
        view_as: false,
        read_only: false,
        display_name: None,
        feature_grants: Vec::new(),
        authz_subject_version: 0,
        authz_policy_version: 0,
        session_generation: 0,
        issued_at: now,
    };
    let token = f
        .issuer
        .issue_session_access_token(input, None, Vec::new(), family, deadline)
        .unwrap();
    let before = auth_effects(&owner).await;
    denied(
        &get(&f, ATTENDANCE, &token).await,
        "dormant enrollment purpose cannot become business authority",
    );
    denied(
        &post_raw(
            f.router.clone(),
            "/api/v1/auth/privacy-consent/status",
            Some(&token),
            json!({}),
        )
        .await,
        "no v1 HTTP release barrier",
    );
    assert_eq!(auth_effects(&owner).await, before);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn otp_admin_and_mobile_step_up_denials_have_no_effect(owner: PgPool) {
    let f = fixture(&owner, "ADMIN", OrgId::knl(), "purpose-admin").await;
    let target =
        seed_user_with_branch(&owner, "Target", "010-8900-0002", "MECHANIC", f.branch).await;
    let otp = bootstrap(&f, OrgId::knl()).await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = enroll_passkey(&f.router, &mut authenticator, &otp.access_token).await;
    for path in [
        "/api/v1/auth/admin/otp/issue",
        "/api/v1/auth/admin/credential-reset",
    ] {
        let before = auth_effects(&owner).await;
        denied(
            &post_raw(
                f.router.clone(),
                path,
                Some(&otp.access_token),
                json!({"user_id":target.as_uuid(),"branch_id":f.branch.as_uuid()}),
            )
            .await,
            "administration after enrollment",
        );
        assert_eq!(auth_effects(&owner).await, before);
    }
    let before = auth_effects(&owner).await;
    denied(&post_raw(f.router.clone(),"/api/v1/auth/passkey/step-up/start",Some(&otp.access_token),json!({"binding":{"action_kind":"APPROVAL_DECISION","object_id":Uuid::new_v4(),"reason_key":"operations_passkey_approval_decision","replay_attempt":null}})).await,"business step-up");
    assert_eq!(auth_effects(&owner).await, before);
    let normal = usernameless_login(&f.router, &mut authenticator, &credential).await;
    let issued: AdminIssueOtpResponse = post_json(
        f.router.clone(),
        "/api/v1/auth/admin/otp/issue",
        Some(&normal.access_token),
        json!({"user_id":target.as_uuid(),"branch_id":f.branch.as_uuid()}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(issued.user_id, *target.as_uuid());
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn otp_cannot_delete_own_credential_even_with_key_present(owner: PgPool) {
    let f = fixture(&owner, "MEMBER", OrgId::knl(), "purpose-delete").await;
    let otp = bootstrap(&f, OrgId::knl()).await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = enroll_passkey(&f.router, &mut authenticator, &otp.access_token).await;
    let normal = usernameless_login(&f.router, &mut authenticator, &credential).await;
    // Two real stored credential rows permit deletion; the second belongs to a
    // distinct authenticator and is registered through the existing step-up flow.
    let start: LoginStartResponse = post_json(
        f.router.clone(),
        "/api/v1/auth/passkey/login/start",
        None,
        json!({}),
        StatusCode::OK,
    )
    .await;
    let assertion = authenticator
        .do_authentication(
            Url::parse(TEST_ORIGIN).unwrap(),
            inject_allow_credential(start.challenge, &credential),
        )
        .unwrap();
    let registration: RegisterStartResponse = post_json(f.router.clone(),"/api/v1/auth/passkey/register/start",Some(&normal.access_token),json!({"username":"second","display_name":"Second","step_up":{"ceremony_id":start.ceremony_id,"credential":assertion}}),StatusCode::OK).await;
    let mut second = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let registered = second
        .do_registration(Url::parse(TEST_ORIGIN).unwrap(), registration.challenge)
        .unwrap();
    let _: RegisterFinishResponse = post_json(
        f.router.clone(),
        "/api/v1/auth/passkey/register/finish",
        Some(&normal.access_token),
        json!({"ceremony_id":registration.ceremony_id,"credential":registered}),
        StatusCode::CREATED,
    )
    .await;
    let key: Uuid = sqlx::query_scalar(
        "SELECT id FROM auth_webauthn_credentials WHERE user_id=$1 ORDER BY created_at,id LIMIT 1",
    )
    .bind(*f.user.as_uuid())
    .fetch_one(&owner)
    .await
    .unwrap();
    let path = format!("/api/v1/auth/passkeys/{key}");
    let before = auth_effects(&owner).await;
    let response = f
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(&path)
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", otp.access_token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    denied(&response, "own credential deletion");
    assert_eq!(auth_effects(&owner).await, before);
    let response = f
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(&path)
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", normal.access_token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_webauthn_credentials WHERE user_id=$1")
            .bind(*f.user.as_uuid())
            .fetch_one(&owner)
            .await
            .unwrap();
    assert_eq!(remaining, 1);
}

async fn enrollment_handoff(f: &Fixture, access: &str) -> Value {
    accept_required_privacy_consent(&f.router, access).await;
    post_json(
        f.router.clone(),
        "/api/v1/auth/passkey/enroll-handoff",
        Some(access),
        json!({}),
        StatusCode::OK,
    )
    .await
}

fn approve_token(handoff: &Value) -> String {
    let url = Url::parse(handoff["enroll_url"].as_str().unwrap()).unwrap();
    url::form_urlencoded::parse(url.fragment().unwrap().as_bytes())
        .find(|(key, _)| key == "desktop_approve")
        .unwrap()
        .1
        .into_owned()
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn enrollment_qr_requires_signed_approval_to_create_business_session(owner: PgPool) {
    let f = fixture(&owner, "MEMBER", OrgId::knl(), "purpose-qr").await;
    let original = bootstrap(&f, OrgId::knl()).await;
    let handoff = enrollment_handoff(&f, &original.access_token).await;
    let phone: OtpRedeemResponse = post_json(
        f.router.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({"otp":handoff["otp"]}),
        StatusCode::OK,
    )
    .await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = enroll_passkey(&f.router, &mut authenticator, &phone.access_token).await;
    let approval = approve_token(&handoff);
    let before = auth_effects(&owner).await;
    denied(
        &post_raw(
            f.router.clone(),
            "/api/v1/auth/device-login/approve-session",
            Some(&phone.access_token),
            json!({"approve_token":approval}),
        )
        .await,
        "phone enrollment family with new key",
    );
    assert_eq!(auth_effects(&owner).await, before);
    let start: LoginStartResponse = post_json(
        f.router.clone(),
        "/api/v1/auth/passkey/login/start",
        None,
        json!({}),
        StatusCode::OK,
    )
    .await;
    let signed = authenticator
        .do_authentication(
            Url::parse(TEST_ORIGIN).unwrap(),
            inject_allow_credential(start.challenge, &credential),
        )
        .unwrap();
    let response = post_raw(
        f.router.clone(),
        "/api/v1/auth/device-login/approve",
        None,
        json!({"approve_token":approval,"ceremony_id":start.ceremony_id,"credential":signed}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let consumed: Value = post_json(
        f.router.clone(),
        "/api/v1/auth/device-login/poll",
        None,
        json!({"poll_token":handoff["poll_token"]}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(consumed["status"], "approved");
    assert_eq!(
        get(&f, ATTENDANCE, consumed["access_token"].as_str().unwrap())
            .await
            .status(),
        StatusCode::OK
    );
    let before = auth_effects(&owner).await;
    assert_eq!(
        post_raw(
            f.router.clone(),
            "/api/v1/auth/device-login/poll",
            None,
            json!({"poll_token":handoff["poll_token"]})
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(auth_effects(&owner).await, before);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn session_approval_rechecks_classification_after_account_lock_wait(owner: PgPool) {
    let label = "purpose-approval-wait";
    let f = fixture(&owner, "MEMBER", OrgId::knl(), label).await;
    let initial = bootstrap(&f, OrgId::knl()).await;
    let handoff = enrollment_handoff(&f, &initial.access_token).await;
    let phone: OtpRedeemResponse = post_json(
        f.router.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({"otp":handoff["otp"]}),
        StatusCode::OK,
    )
    .await;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = enroll_passkey(&f.router, &mut authenticator, &phone.access_token).await;
    let normal = usernameless_login(&f.router, &mut authenticator, &credential).await;
    let family = f
        .verifier
        .verify_access_token(&normal.access_token)
        .unwrap()
        .session_family_id
        .unwrap();
    let source: Uuid = sqlx::query_scalar(
        "SELECT source_id FROM auth_legacy_otp_family_sources WHERE family_id=$1",
    )
    .bind(
        f.verifier
            .verify_access_token(&phone.access_token)
            .unwrap()
            .session_family_id
            .unwrap(),
    )
    .fetch_one(&owner)
    .await
    .unwrap();
    let before = auth_effects(&owner).await;
    let mut gate = owner.begin().await.unwrap();
    sqlx::query("SELECT is_active FROM users WHERE id = $1 AND org_id = $2 FOR NO KEY UPDATE")
        .bind(*f.user.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(gate.as_mut())
        .await
        .unwrap();
    let pid = transaction_pid(&mut gate).await;
    let router = f.router.clone();
    let access = normal.access_token.clone();
    let approval = approve_token(&handoff);
    let task = tokio::spawn(async move {
        post_raw(
            router,
            "/api/v1/auth/device-login/approve-session",
            Some(&access),
            json!({"approve_token":approval}),
        )
        .await
    });
    waiting(
        &owner,
        label,
        pid,
        "SELECT is_active FROM users WHERE id = $1 AND org_id = $2 FOR NO KEY UPDATE",
    )
    .await;
    // Adversarial classification correction, not a supported issuance path.
    sqlx::query("INSERT INTO auth_legacy_otp_family_sources(family_id,source_id,user_id,org_id) VALUES($1,$2,$3,$4)")
        .bind(family).bind(source).bind(*f.user.as_uuid()).bind(*OrgId::knl().as_uuid()).execute(gate.as_mut()).await.unwrap();
    gate.commit().await.unwrap();
    denied(
        &timeout(WAIT, task).await.unwrap().unwrap(),
        "purpose changed after outer admission",
    );
    assert_eq!(auth_effects(&owner).await, before);
    let pending: Value = post_json(
        f.router.clone(),
        "/api/v1/auth/device-login/poll",
        None,
        json!({"poll_token":handoff["poll_token"]}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(pending["status"], "pending");
    assert_eq!(auth_effects(&owner).await, before);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn group_mint_and_preminted_actor_use_original_home_family(owner: PgPool) {
    let f = fixture(&owner, "ADMIN", OrgId::knl(), "purpose-group").await;
    let target = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations(id,slug,name) VALUES($1,'purpose-target','Target')")
        .bind(target)
        .execute(&owner)
        .await
        .unwrap();
    let group: Uuid = sqlx::query_scalar(
        "INSERT INTO groups(slug,name) VALUES('purpose-group','Group') RETURNING id",
    )
    .fetch_one(&owner)
    .await
    .unwrap();
    sqlx::query("INSERT INTO group_memberships(group_id,org_id) VALUES($1,$2)")
        .bind(group)
        .bind(target)
        .execute(&owner)
        .await
        .unwrap();
    sqlx::query("UPDATE organizations SET group_id=$1 WHERE id=$2")
        .bind(group)
        .bind(target)
        .execute(&owner)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO group_role_grants(group_id,user_id,group_role) VALUES($1,$2,'GROUP_ADMIN')",
    )
    .bind(group)
    .bind(*f.user.as_uuid())
    .execute(&owner)
    .await
    .unwrap();
    let otp = bootstrap(&f, OrgId::knl()).await;
    let before = auth_effects(&owner).await;
    denied(
        &post_raw(
            f.router.clone(),
            "/api/v1/group-admin/tenant-context",
            Some(&otp.access_token),
            json!({"org_id":target}),
        )
        .await,
        "Group mint",
    );
    denied(
        &get(&f, "/api/v1/group-admin/groups", &otp.access_token).await,
        "Group listing",
    );
    assert_eq!(auth_effects(&owner).await, before);
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = enroll_passkey(&f.router, &mut authenticator, &otp.access_token).await;
    let normal = usernameless_login(&f.router, &mut authenticator, &credential).await;
    let minted: Value = post_json(
        f.router.clone(),
        "/api/v1/group-admin/tenant-context",
        Some(&normal.access_token),
        json!({"org_id":target}),
        StatusCode::OK,
    )
    .await;
    let token = minted["access_token"].as_str().unwrap();
    let delegated = f.verifier.verify_access_token(token).unwrap();
    let original = f
        .verifier
        .verify_access_token(&normal.access_token)
        .unwrap();
    assert_eq!(delegated.actor_session, Some(actor(&original)));
    assert_eq!(delegated.org, target.to_string());
    let target_user: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE id=$1 AND org_id=$2")
            .bind(*f.user.as_uuid())
            .bind(target)
            .fetch_one(&owner)
            .await
            .unwrap();
    assert_eq!(target_user, 0);
    assert!(
        resolve_principal_from_bearer_token(&f.verifier, &f.runtime, token)
            .await
            .is_ok()
    );
    let otp_claims = f.verifier.verify_access_token(&otp.access_token).unwrap();
    let mut target_input = input(&delegated, OrgId::from_uuid(target), vec!["ADMIN".into()]);
    target_input.platform = false;
    let historical = f
        .issuer
        .issue_group_admin_tenant_context_access_token(
            target_input,
            group,
            actor(&otp_claims),
            Duration::minutes(5),
        )
        .unwrap();
    let before = auth_effects(&owner).await;
    assert!(matches!(
        resolve_principal_from_bearer_token(&f.verifier, &f.runtime, &historical).await,
        Err(console_platform_request_context::RequestContextError::InvalidToken)
    ));
    denied(
        &get(&f, ATTENDANCE, &historical).await,
        "preminted delegated OTP home",
    );
    // Credential/consent exceptions cannot be used by delegated principals.
    assert_eq!(
        post_raw(
            f.router.clone(),
            "/api/v1/auth/privacy-consent/status",
            Some(token),
            json!({})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(auth_effects(&owner).await, before);
    sqlx::query("UPDATE organizations SET status='SUSPENDED' WHERE id=$1")
        .bind(target)
        .execute(&owner)
        .await
        .unwrap();
    assert!(
        validate_current_claims(&f.runtime, &delegated)
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn platform_enrollment_and_realtime_keep_ordinary_purpose_boundary(owner: PgPool) {
    let f = fixture(&owner, "SUPER_ADMIN", OrgId::platform(), "purpose-platform").await;
    let status: String = sqlx::query_scalar("SELECT status FROM organizations WHERE id=$1")
        .bind(*OrgId::platform().as_uuid())
        .fetch_one(&owner)
        .await
        .unwrap();
    assert_eq!(status, "ARCHIVED");
    let otp = bootstrap(&f, OrgId::platform()).await;
    let before = auth_effects(&owner).await;
    assert!(matches!(
        resolve_platform_principal(&f.verifier, &f.runtime, &headers(&otp.access_token)).await,
        Err(console_platform_request_context::RequestContextError::InvalidToken)
    ));
    assert_eq!(auth_effects(&owner).await, before);
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = enroll_passkey(&f.router, &mut authenticator, &otp.access_token).await;
    let own = get(&f, "/api/v1/auth/passkeys", &otp.access_token).await;
    assert_eq!(own.status(), StatusCode::OK);
    assert_eq!(body_json(own).await.as_array().unwrap().len(), 1);
    let normal = usernameless_login(&f.router, &mut authenticator, &credential).await;
    assert!(
        resolve_platform_principal(&f.verifier, &f.runtime, &headers(&normal.access_token))
            .await
            .is_ok()
    );
    let target = OrgId::knl();
    let platform_claims = f
        .verifier
        .verify_access_token(&normal.access_token)
        .unwrap();
    let mut delegated_input = input(&platform_claims, target, vec!["SUPER_ADMIN".into()]);
    delegated_input.authz_subject_version = 0;
    delegated_input.session_generation = 0;
    let normal_actor = f
        .issuer
        .issue_platform_tenant_context_access_token(
            delegated_input.clone(),
            actor(&platform_claims),
            Duration::minutes(5),
        )
        .unwrap();
    assert!(
        resolve_principal_from_bearer_token(&f.verifier, &f.runtime, &normal_actor)
            .await
            .is_ok()
    );
    let otp_claims = f.verifier.verify_access_token(&otp.access_token).unwrap();
    let otp_actor = f
        .issuer
        .issue_platform_tenant_context_access_token(
            delegated_input,
            actor(&otp_claims),
            Duration::minutes(5),
        )
        .unwrap();
    assert!(matches!(
        resolve_principal_from_bearer_token(&f.verifier, &f.runtime, &otp_actor).await,
        Err(console_platform_request_context::RequestContextError::InvalidToken)
    ));
    // This is the production realtime admission resolver, not a WebSocket or browser claim.
    let tenant = fixture(&owner, "MEMBER", OrgId::knl(), "purpose-realtime").await;
    let tenant_otp = bootstrap(&tenant, OrgId::knl()).await;
    assert!(matches!(
        resolve_principal_from_bearer_token(
            &tenant.verifier,
            &tenant.runtime,
            &tenant_otp.access_token
        )
        .await,
        Err(console_platform_request_context::RequestContextError::InvalidToken)
    ));
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential =
        enroll_passkey(&tenant.router, &mut authenticator, &tenant_otp.access_token).await;
    let signed = usernameless_login(&tenant.router, &mut authenticator, &credential).await;
    assert!(
        resolve_principal_from_bearer_token(
            &tenant.verifier,
            &tenant.runtime,
            &signed.access_token
        )
        .await
        .is_ok()
    );
    sqlx::query("UPDATE auth_refresh_token_families SET revoked_at=clock_timestamp() WHERE id=$1")
        .bind(
            tenant
                .verifier
                .verify_access_token(&signed.access_token)
                .unwrap()
                .session_family_id
                .unwrap(),
        )
        .execute(&owner)
        .await
        .unwrap();
    assert!(
        resolve_principal_from_bearer_token(
            &tenant.verifier,
            &tenant.runtime,
            &signed.access_token
        )
        .await
        .is_err()
    );
}
