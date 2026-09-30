#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Real `console_rt` coverage for the recruiting pipeline (STORY-RECRUITING-001).
//! It crosses the assembled HTTP router: posting draft → preflight-gated
//! publish → applicant stages → assessment-gated offer → accepted-offer hire
//! through the HR-owned employee core — plus PBAC denial without leakage,
//! cross-tenant concealment, and audit readback.

use axum::body::{Body, to_bytes};
use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use console_kernel_core::{OrgId, UserId};
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
const POSTINGS: &str = "/api/v1/recruiting/postings";
const APPLICANTS: &str = "/api/v1/recruiting/applicants";
const OFFERS: &str = "/api/v1/recruiting/offers";

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn recruiting_pipeline_publish_offer_hire_with_full_audit_lineage(pool: PgPool) {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("error")
        .try_init();
    let keys = Keys::generate();
    let org = OrgId::knl();
    let admin = UserId::new();
    seed_user(&pool, org, admin, &["SUPER_ADMIN"]).await;
    let branch = seed_branch(&pool, org, "recruit-hq").await;
    let service = build_router(
        app_state(
            runtime_role_pool(&pool).await,
            leave_command_role_pool(&pool).await,
            keys.public_pem.clone(),
        )
        .unwrap(),
    );
    let token = keys.token(admin, org, &["SUPER_ADMIN"]);

    // Draft: signature field vector from the composer.
    let (status, posting) = send(
        &service,
        "POST",
        POSTINGS,
        &token,
        Some(json!({
            "role_title": "지게차 정비 기술자",
            "company": "KNL",
            "worksite": "창원 성산",
            "employment_type": "REGULAR",
            "scope": "EXTERNAL",
            "headcount": 1,
            "deadline": "2026-08-31",
            "requirements": ["경력 3년 이상"],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create posting: {posting}");
    assert_eq!(posting["status"], "DRAFT");
    assert_eq!(posting["posting_no"], "JP-0001");
    let posting_id = posting["id"].as_str().unwrap().to_owned();
    let updated_at = posting["updated_at"].as_str().unwrap().to_owned();

    // Draft-only edit with optimistic concurrency: a stale token conflicts.
    let update_body = |expected: &str| {
        json!({
            "role_title": "지게차 정비 기술자",
            "company": "KNL",
            "worksite": "창원 성산",
            "employment_type": "REGULAR",
            "scope": "EXTERNAL",
            "headcount": 1,
            "deadline": null,
            "requirements": ["경력 3년 이상", "지게차 운전기능사"],
            "expected_updated_at": expected,
        })
    };
    let (status, updated) = send(
        &service,
        "PUT",
        &format!("{POSTINGS}/{posting_id}"),
        &token,
        Some(update_body(&updated_at)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "draft edit: {updated}");
    let (status, stale) = send(
        &service,
        "PUT",
        &format!("{POSTINGS}/{posting_id}"),
        &token,
        Some(update_body(&updated_at)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "stale edit must 409: {stale}");
    let updated_at = updated["updated_at"].as_str().unwrap().to_owned();

    // Preflight is read-only and publish fails closed without the attest.
    let (status, preflight) = send(
        &service,
        "POST",
        &format!("{POSTINGS}/{posting_id}/preflight"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "preflight: {preflight}");
    assert_eq!(preflight["publishable"], true);
    assert_eq!(preflight["checks"].as_array().unwrap().len(), 4);
    let (status, unattested) = send(
        &service,
        "POST",
        &format!("{POSTINGS}/{posting_id}/publish"),
        &token,
        Some(json!({ "attest_exposure_scope": false, "expected_updated_at": updated_at })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "publish without attest must fail closed: {unattested}"
    );
    assert_eq!(unattested["error"]["code"], "PREFLIGHT_FAILED");
    assert!(
        unattested["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check["key"] == "exposure_attested" && check["ok"] == false),
        "check vector must name the unmet attest: {unattested}"
    );
    let (status, published) = send(
        &service,
        "POST",
        &format!("{POSTINGS}/{posting_id}/publish"),
        &token,
        Some(json!({ "attest_exposure_scope": true, "expected_updated_at": updated_at })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "publish: {published}");
    assert_eq!(published["status"], "PUBLISHED");
    let (status, sealed) = send(
        &service,
        "PUT",
        &format!("{POSTINGS}/{posting_id}"),
        &token,
        Some(update_body(published["updated_at"].as_str().unwrap())),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "published is immutable: {sealed}"
    );

    // Applicant intake and single-step stage walk; INTERVIEW→OFFER is
    // offer-only and the offer is assessment-gated (both fail closed).
    let (status, applicant) = send(
        &service,
        "POST",
        &format!("{POSTINGS}/{posting_id}/applicants"),
        &token,
        Some(json!({
            "name": "김지원",
            "profile_lines": ["지게차 정비 경력 5년", "산업기사 보유"],
            "source_document": "resume-kim.pdf",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "applicant intake: {applicant}");
    assert_eq!(applicant["stage"], "APPLIED");
    assert_eq!(applicant["applicant_no"], "APL-0001");
    let applicant_id = applicant["id"].as_str().unwrap().to_owned();
    let mut applicant_updated_at = applicant["updated_at"].as_str().unwrap().to_owned();
    for expected_stage in ["SCREENING", "INTERVIEW"] {
        let (status, advanced) = send(
            &service,
            "POST",
            &format!("{APPLICANTS}/{applicant_id}/advance"),
            &token,
            Some(json!({ "expected_updated_at": applicant_updated_at })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "advance: {advanced}");
        assert_eq!(advanced["stage"], expected_stage);
        applicant_updated_at = advanced["updated_at"].as_str().unwrap().to_owned();
    }
    let (status, blocked) = send(
        &service,
        "POST",
        &format!("{APPLICANTS}/{applicant_id}/advance"),
        &token,
        Some(json!({ "expected_updated_at": applicant_updated_at })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "interview advances only through an offer: {blocked}"
    );
    let (status, unassessed) = send(
        &service,
        "POST",
        &format!("{APPLICANTS}/{applicant_id}/offer"),
        &token,
        Some(json!({
            "amount": "3500000",
            "amount_period": "MONTHLY",
            "reply_deadline": "2026-08-15",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "offer before assessment must fail closed: {unassessed}"
    );
    assert_eq!(unassessed["error"]["code"], "ASSESSMENT_REQUIRED");
    let (status, assessed) = send(
        &service,
        "POST",
        &format!("{APPLICANTS}/{applicant_id}/assess"),
        &token,
        Some(json!({ "score": "SUITABLE" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "assess: {assessed}");
    assert_eq!(assessed["assessment"]["score"], "SUITABLE");

    // Offer: extend → adjust (immutable v+1, prior superseded) → accepted.
    let (status, offer) = send(
        &service,
        "POST",
        &format!("{APPLICANTS}/{applicant_id}/offer"),
        &token,
        Some(json!({
            "amount": "3500000",
            "amount_period": "MONTHLY",
            "reply_deadline": "2026-08-15",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "extend offer: {offer}");
    assert_eq!(offer["status"], "EXTENDED");
    assert_eq!(offer["version"], 1);
    let first_offer_id = offer["id"].as_str().unwrap().to_owned();
    let (status, adjusted) = send(
        &service,
        "POST",
        &format!("{OFFERS}/{first_offer_id}/adjust"),
        &token,
        Some(json!({ "amount": "3800000" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "adjust offer: {adjusted}");
    assert_eq!(adjusted["version"], 2);
    assert_eq!(adjusted["amount"], "3800000.00");
    let live_offer_id = adjusted["id"].as_str().unwrap().to_owned();
    let (status, replied) = send(
        &service,
        "POST",
        &format!("{OFFERS}/{live_offer_id}/record-reply"),
        &token,
        Some(json!({ "decision": "ACCEPTED" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "record reply: {replied}");
    assert_eq!(replied["status"], "ACCEPTED");

    // Hire: ONE transaction through the HR-owned employee core; base_pay
    // defaults from the accepted monthly offer.
    let hire_body = json!({
        "employee_number": "RCT-0001",
        "phone": "010-2345-6789",
        "org_unit": "정비",
        "position": "기술자",
        "site": "창원 성산",
        "home_branch_id": branch,
    });
    let (status, hired) = send(
        &service,
        "POST",
        &format!("{APPLICANTS}/{applicant_id}/hire"),
        &token,
        Some(hire_body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "hire: {hired}");
    let employee_id = hired["employee_id"].as_str().unwrap().to_owned();
    assert_eq!(hired["applicant"]["stage"], "HIRED");
    assert_eq!(
        hired["applicant"]["hired_employee_id"],
        employee_id.as_str()
    );
    assert_eq!(hired["posting"]["hired_count"], 1);
    let employee_row: (String, String, String, Option<Uuid>) = sqlx::query_as(
        "SELECT e.name, e.company, p.base_pay::TEXT, e.home_branch_id FROM employees e \
         JOIN employee_employment_profiles p ON p.employee_id = e.id AND p.org_id = e.org_id \
         WHERE e.id = $1 AND e.org_id = $2",
    )
    .bind(Uuid::parse_str(&employee_id).unwrap())
    .bind(*org.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        employee_row,
        (
            "김지원".to_owned(),
            "KNL".to_owned(),
            "3800000.00".to_owned(),
            Some(branch),
        ),
        "employee must carry the applicant identity, the accepted offer amount, and the command-assigned home branch"
    );
    let branch_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'employee.home_branch_set' AND target_id = $1",
    )
    .bind(&employee_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        branch_audits, 1,
        "home-branch authority must be established once through the command capability"
    );
    let reservation_key: String = sqlx::query_scalar(
        "SELECT idempotency_key FROM employee_create_idempotency WHERE org_id = $1 AND employee_id = $2",
    )
    .bind(*org.as_uuid())
    .bind(Uuid::parse_str(&employee_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(reservation_key, format!("recruit-hire-{applicant_id}"));

    // Idempotent replay: already-hired reports 409 with the linked employee.
    let (status, replay) = send(
        &service,
        "POST",
        &format!("{APPLICANTS}/{applicant_id}/hire"),
        &token,
        Some(hire_body),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "hire replay: {replay}");
    assert_eq!(replay["employee_id"], employee_id.as_str());

    // History layer: the applicant timeline is served from stage events, and
    // the PII detail read is itself audited.
    let (status, detail) = send(
        &service,
        "GET",
        &format!("{APPLICANTS}/{applicant_id}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "applicant detail: {detail}");
    assert_eq!(detail["offers"].as_array().unwrap().len(), 2);
    let actions: Vec<&str> = detail["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["action"].as_str().unwrap())
        .collect();
    assert_eq!(
        actions,
        [
            "APPLY",
            "ADVANCE",
            "ADVANCE",
            "ASSESS",
            "OFFER_EXTEND",
            "OFFER_ADJUST",
            "OFFER_REPLY",
            "HIRE"
        ],
        "stage-event history must record the full lifecycle"
    );

    // Audit readback: recruiting lifecycle actions AND exactly one
    // employee.create from the reused HR core — two events, one transaction.
    let audit_actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_events WHERE target_id = $1 ORDER BY occurred_at, action",
    )
    .bind(&applicant_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    for expected in [
        "recruiting.applicant.create",
        "recruiting.applicant.advance",
        "recruiting.applicant.assess",
        "recruiting.applicant.hire",
        "recruiting.applicant.view",
    ] {
        assert!(
            audit_actions.iter().any(|action| action == expected),
            "audit stream must contain {expected}: {audit_actions:?}"
        );
    }
    let employee_creates: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'employee.create' AND target_id = $1",
    )
    .bind(&employee_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(employee_creates, 1, "exactly one employee.create audit");
    let publish_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'recruiting.posting.publish' AND target_id = $1",
    )
    .bind(&posting_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        publish_audits, 1,
        "publish must audit once with its snapshot"
    );

    // Close remains available from PUBLISHED — and posting detail serves
    // applicant SUMMARIES only: the PII surfaces (profile, provenance, note,
    // assessment signature) live exclusively behind the audited detail read.
    let (_, current) = send(
        &service,
        "GET",
        &format!("{POSTINGS}/{posting_id}"),
        &token,
        None,
    )
    .await;
    let summary = &current["applicants"][0];
    assert_eq!(summary["applicant_no"], "APL-0001");
    assert_eq!(summary["stage"], "HIRED");
    assert_eq!(summary["assessed"], true);
    for pii_field in [
        "profile_lines",
        "source_document",
        "reject_note",
        "assessment",
    ] {
        assert!(
            summary.get(pii_field).is_none(),
            "posting detail must not leak the unaudited PII surface {pii_field}: {summary}"
        );
    }
    let (status, closed) = send(
        &service,
        "POST",
        &format!("{POSTINGS}/{posting_id}/close"),
        &token,
        Some(json!({ "expected_updated_at": current["posting"]["updated_at"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "close: {closed}");
    assert_eq!(closed["status"], "CLOSED");
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn recruiting_denies_without_leakage_and_conceals_other_tenants(pool: PgPool) {
    let keys = Keys::generate();
    let org = OrgId::knl();
    let admin = UserId::new();
    let executive = UserId::new();
    let member = UserId::new();
    seed_user(&pool, org, admin, &["SUPER_ADMIN"]).await;
    seed_user(&pool, org, executive, &["EXECUTIVE"]).await;
    seed_user(&pool, org, member, &["MEMBER"]).await;
    let service = build_router(
        app_state(
            runtime_role_pool(&pool).await,
            leave_command_role_pool(&pool).await,
            keys.public_pem.clone(),
        )
        .unwrap(),
    );
    let admin_token = keys.token(admin, org, &["SUPER_ADMIN"]);

    // Deny-by-default: MEMBER cannot read at all; EXECUTIVE reads but cannot
    // manage; neither response leaks whether anything exists.
    let member_token = keys.token(member, org, &["MEMBER"]);
    let (status, denied) = send(&service, "GET", POSTINGS, &member_token, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "member read: {denied}");
    let executive_token = keys.token(executive, org, &["EXECUTIVE"]);
    let (status, listed) = send(&service, "GET", POSTINGS, &executive_token, None).await;
    assert_eq!(status, StatusCode::OK, "executive read: {listed}");
    let (status, denied) = send(
        &service,
        "POST",
        POSTINGS,
        &executive_token,
        Some(json!({
            "role_title": "생산 관리",
            "company": "KNL",
            "worksite": "창원",
            "employment_type": "REGULAR",
            "scope": "INTERNAL",
            "headcount": 1,
            "requirements": [],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "executive manage: {denied}");

    // Seed one published posting + applicant, then reject into the talent
    // pool with the enum reason and reinstate to the prior stage.
    let (_, posting) = send(
        &service,
        "POST",
        POSTINGS,
        &admin_token,
        Some(json!({
            "role_title": "현장 정비",
            "company": "KNL",
            "worksite": "창원",
            "employment_type": "REGULAR",
            "scope": "EXTERNAL",
            "headcount": 1,
            "requirements": [],
        })),
    )
    .await;
    let posting_id = posting["id"].as_str().unwrap().to_owned();
    let (status, published) = send(
        &service,
        "POST",
        &format!("{POSTINGS}/{posting_id}/publish"),
        &admin_token,
        Some(json!({
            "attest_exposure_scope": true,
            "expected_updated_at": posting["updated_at"],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "publish: {published}");
    let (_, applicant) = send(
        &service,
        "POST",
        &format!("{POSTINGS}/{posting_id}/applicants"),
        &admin_token,
        Some(json!({ "name": "박후보", "profile_lines": ["경력 2년"] })),
    )
    .await;
    let applicant_id = applicant["id"].as_str().unwrap().to_owned();
    let (status, invalid) = send(
        &service,
        "POST",
        &format!("{APPLICANTS}/{applicant_id}/reject"),
        &admin_token,
        Some(json!({ "reason": "BAD_VIBES" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "reject requires the enum reason: {invalid}"
    );
    let (status, rejected) = send(
        &service,
        "POST",
        &format!("{APPLICANTS}/{applicant_id}/reject"),
        &admin_token,
        Some(json!({ "reason": "CAREER_SHORTFALL", "note": "경력 미달" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "reject: {rejected}");
    assert_eq!(rejected["reject_reason"], "CAREER_SHORTFALL");
    let (status, pool_list) = send(
        &service,
        "GET",
        "/api/v1/recruiting/talent-pool",
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "talent pool: {pool_list}");
    assert_eq!(pool_list["items"].as_array().unwrap().len(), 1);
    assert_eq!(pool_list["items"][0]["reason"], "CAREER_SHORTFALL");
    let (status, held) = send(
        &service,
        "POST",
        &format!("{APPLICANTS}/{applicant_id}/hold"),
        &admin_token,
        Some(json!({ "hold": true })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "rejected applicant must be reinstated before other mutations: {held}"
    );
    let (status, reinstated) = send(
        &service,
        "POST",
        &format!("{APPLICANTS}/{applicant_id}/reinstate"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "reinstate: {reinstated}");
    assert_eq!(reinstated["stage"], "APPLIED");
    assert!(reinstated["reject_reason"].is_null());

    // Cross-tenant concealment: an org-B super admin sees nothing — 404 on
    // ids, an empty list, no existence signal.
    let other_org = OrgId::from_uuid(Uuid::new_v4());
    seed_org(&pool, other_org).await;
    let outsider = UserId::new();
    seed_user(&pool, other_org, outsider, &["SUPER_ADMIN"]).await;
    let outsider_token = keys.token(outsider, other_org, &["SUPER_ADMIN"]);
    let (status, concealed) = send(
        &service,
        "GET",
        &format!("{POSTINGS}/{posting_id}"),
        &outsider_token,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cross-org posting: {concealed}"
    );
    let (status, concealed) = send(
        &service,
        "GET",
        &format!("{APPLICANTS}/{applicant_id}"),
        &outsider_token,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cross-org applicant: {concealed}"
    );
    let (status, empty) = send(&service, "GET", POSTINGS, &outsider_token, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        empty["items"].as_array().unwrap().len(),
        0,
        "cross-org list must be empty: {empty}"
    );
    let (status, denied) = send(
        &service,
        "POST",
        &format!("{APPLICANTS}/{applicant_id}/advance"),
        &outsider_token,
        Some(json!({ "expected_updated_at": reinstated["updated_at"] })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cross-org mutation must 404, not 403: {denied}"
    );
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

    fn token(&self, user: UserId, org: OrgId, roles: &[&str]) -> String {
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
}

async fn runtime_role_pool(owner: &PgPool) -> PgPool {
    role_pool(owner, "SET ROLE console_rt").await
}

/// The isolated leave command capability (home-branch authority), armed the
/// way production arms LEAVE_COMMAND_DATABASE_URL.
async fn leave_command_role_pool(owner: &PgPool) -> PgPool {
    role_pool(owner, "SET ROLE console_leave_cmd").await
}

async fn role_pool(owner: &PgPool, set_role: &'static str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(8)
        .after_connect(move |conn, _| {
            Box::pin(async move {
                sqlx::query(set_role).execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(owner.connect_options().as_ref().clone())
        .await
        .unwrap()
}

fn app_state(
    pool: PgPool,
    leave_command_pool: PgPool,
    public_key: String,
) -> Result<AppState, console_app::AppError> {
    Ok(AppState::new(
        AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
            ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".into()),
            ("CONSOLE_JWT_ISSUER", ISSUER.into()),
            ("CONSOLE_JWT_AUDIENCE", AUDIENCE.into()),
            ("CONSOLE_JWT_PUBLIC_KEY_PEM", public_key),
        ])?,
        DatabaseDependency::Postgres(pool),
    )?
    .with_leave_command_database(leave_command_pool))
}

async fn seed_org(pool: &PgPool, org: OrgId) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(*org.as_uuid())
        .bind(format!("rc-{}", org.as_uuid().simple()))
        .bind("Recruiting test org")
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_user(pool: &PgPool, org: OrgId, user: UserId, roles: &[&str]) {
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user.as_uuid())
        .bind(format!("recruit-{user}"))
        .bind(
            roles
                .iter()
                .map(|role| (*role).to_owned())
                .collect::<Vec<_>>(),
        )
        .bind(*org.as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_branch(pool: &PgPool, org: OrgId, name: &str) -> Uuid {
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("{name}-region"))
            .bind(*org.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region)
    .bind(name)
    .bind(*org.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn send(
    service: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let request = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        builder.body(Body::from(body.to_string())).unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let response = service.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}))
        },
    )
}
