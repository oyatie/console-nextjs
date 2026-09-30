#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! HTTP-level person-scoping + passkey-gated receipt confirmation for the
//! statutory-notice vault.
//!
//! Proves over the real router that:
//!   * the recipient is bound from the JWT, never the request (B gets 404
//!     reading/confirming A's legal notice — deny-by-omission, not a leak);
//!   * a locked legal notice's body is withheld until receipt is confirmed;
//!   * confirm-receipt without a fresh passkey step-up is 428 (precondition
//!     required), and a valid step-up confirms + unlocks + is idempotent;
//!   * ESS 명세서 lives here (`filter=payslip`/`filter=pay`, `kind=payslip`),
//!     not payroll REST `/payroll/payslips/me`: a MEMBER JWT with empty
//!     `feature_grants` (no `PayrollRunRead`) still GETs 200 with their
//!     payslip and body without receipt/passkey, `filter=payslip` excludes a
//!     co-emitted legal notice, `filter=all` returns both, and another user
//!     is omitted. `/api/v1` is Bearer-only: missing Authorization is 401,
//!     including `Cookie: console_access=<valid access JWT>`. Recipient is
//!     the JWT subject; `recipient=` query or body does not override it.

use axum::body::{Body, to_bytes};
use console_inbox_adapter_postgres::PgInboxStore;
use console_inbox_application::EmitInboxDocCommand;
use console_inbox_domain::{InboxDocKind, NewInboxDoc};
use console_inbox_rest::{InboxRestState, ME_INBOX_DOCS_PATH, router};
use console_kernel_core::{AuditAction, AuditEvent, OrgId, TraceContext, UserId};
use console_platform_auth::{
    AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier, PasskeyRegistrationStart,
    PasskeyService, WebauthnSettings,
};
use console_platform_authz::Feature;
use console_platform_db::{DbError, with_audit};
use console_platform_test_support::runtime_role_pool;
use http::{Request, StatusCode, header};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use url::Url;
use webauthn_authenticator_rs::prelude::{RequestChallengeResponse, WebauthnAuthenticator};
use webauthn_authenticator_rs::softpasskey::SoftPasskey;

const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

fn passkey_service() -> PasskeyService {
    PasskeyService::new(WebauthnSettings {
        rp_id: "example.com".to_owned(),
        rp_origin: Url::parse("https://auth.example.com").unwrap(),
        rp_name: "Console".to_owned(),
        extra_allowed_origins: vec![],
        ceremony_ttl: Duration::minutes(5),
    })
    .unwrap()
}

/// Register a passkey for `user_id`, then start + finish an authentication
/// ceremony, returning the `{ ceremony_id, credential }` step-up body.
async fn fresh_step_up(pool: &PgPool, user_id: UserId, display_name: &str) -> Value {
    let service = passkey_service();
    let registration = service
        .start_registration(
            pool,
            OrgId::knl(),
            PasskeyRegistrationStart {
                user_id: *user_id.as_uuid(),
                username: format!("{user_id}.example"),
                display_name: display_name.to_owned(),
            },
        )
        .await
        .unwrap();
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = authenticator
        .do_registration(
            Url::parse("https://auth.example.com").unwrap(),
            registration.challenge,
        )
        .unwrap();
    let stored = service
        .finish_registration(pool, OrgId::knl(), registration.ceremony_id, credential)
        .await
        .unwrap();

    let authentication = service.start_authentication(pool).await.unwrap();
    let challenge = inject_allow_credential(authentication.challenge, &stored.credential_id);
    let assertion = authenticator
        .do_authentication(Url::parse("https://auth.example.com").unwrap(), challenge)
        .unwrap();
    json!({ "ceremony_id": authentication.ceremony_id, "credential": assertion })
}

fn inject_allow_credential(
    challenge: RequestChallengeResponse,
    credential_id: &str,
) -> RequestChallengeResponse {
    let mut value = serde_json::to_value(&challenge).unwrap();
    let allow = value
        .get_mut("publicKey")
        .and_then(|pk| pk.get_mut("allowCredentials"))
        .and_then(Value::as_array_mut)
        .expect("authentication challenge must have an allowCredentials array");
    allow.push(json!({ "type": "public-key", "id": credential_id }));
    serde_json::from_value(value).unwrap()
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn inbox_receipt_flow_is_person_scoped_and_passkey_gated(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    let user_a = UserId::new();
    let user_b = UserId::new();
    seed_user(&pool, user_a, "Employee A", &["ADMIN"]).await;
    seed_user(&pool, user_b, "Employee B", &["ADMIN"]).await;

    // Seed a legal notice for A via the write port (owner pool, scoped to knl).
    let doc = console_platform_request_context::scope_org(OrgId::knl(), async {
        PgInboxStore::new(pool.clone())
            .emit_inbox_doc(legal_notice_to(user_a))
            .await
    })
    .await
    .expect("emit legal notice to A");

    let verifier = JwtVerifier::from_es256_public_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        public_key_pem.as_bytes(),
    )
    .unwrap();
    let rt_pool = runtime_role_pool(&pool).await;
    let service = router(
        InboxRestState::new(PgInboxStore::new(rt_pool), Some(verifier))
            .with_passkey_step_up(Some(passkey_service())),
    );
    let token_a = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        user_a,
        &["ADMIN"],
    );
    let token_b = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        user_b,
        &["ADMIN"],
    );

    // A lists action-required: sees exactly its own locked legal notice.
    let list = get_json(
        service.clone(),
        "/api/v1/me/inbox-docs?filter=action",
        &token_a,
    )
    .await;
    assert_eq!(list.status, StatusCode::OK, "{:?}", list.json);
    let items = list.json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"].as_str().unwrap(), doc.id.to_string());
    assert_eq!(items[0]["locked"].as_bool(), Some(true));

    // A reads the locked doc: metadata yes, body withheld, not auto-confirmed.
    let locked = get_json(
        service.clone(),
        &format!("/api/v1/me/inbox-docs/{}", doc.id),
        &token_a,
    )
    .await;
    assert_eq!(locked.status, StatusCode::OK, "{:?}", locked.json);
    assert_eq!(locked.json["locked"].as_bool(), Some(true));
    assert!(
        locked.json.get("payload").is_none(),
        "a locked legal notice must not disclose its body"
    );

    // B cannot read A's doc -> 404 (deny-by-omission).
    let cross_read = get_json(
        service.clone(),
        &format!("/api/v1/me/inbox-docs/{}", doc.id),
        &token_b,
    )
    .await;
    assert_eq!(cross_read.status, StatusCode::NOT_FOUND);

    // Confirm without a step-up -> 428 precondition required.
    let no_stepup = post_json(
        service.clone(),
        &format!("/api/v1/me/inbox-docs/{}/confirm-receipt", doc.id),
        &token_a,
        json!({}),
    )
    .await;
    assert_eq!(
        no_stepup.status,
        StatusCode::PRECONDITION_REQUIRED,
        "{:?}",
        no_stepup.json
    );
    assert_eq!(no_stepup.json["error"]["code"], "passkey_step_up_required");

    // B cannot confirm A's receipt even WITH B's own valid step-up -> 404.
    let b_stepup = fresh_step_up(&pool, user_b, "Employee B").await;
    let cross_confirm = post_json(
        service.clone(),
        &format!("/api/v1/me/inbox-docs/{}/confirm-receipt", doc.id),
        &token_b,
        json!({ "step_up": b_stepup }),
    )
    .await;
    assert_eq!(
        cross_confirm.status,
        StatusCode::NOT_FOUND,
        "B cannot forge A's legal receipt: {:?}",
        cross_confirm.json
    );

    // A confirms its own receipt with a fresh step-up -> 200, unlocked.
    let a_stepup = fresh_step_up(&pool, user_a, "Employee A").await;
    let confirmed = post_json(
        service.clone(),
        &format!("/api/v1/me/inbox-docs/{}/confirm-receipt", doc.id),
        &token_a,
        json!({ "step_up": a_stepup }),
    )
    .await;
    assert_eq!(confirmed.status, StatusCode::OK, "{:?}", confirmed.json);
    assert_eq!(confirmed.json["locked"].as_bool(), Some(false));
    assert_eq!(
        confirmed.json["confirmed_by"].as_str().unwrap(),
        user_a.to_string()
    );

    // Body is now disclosed on read.
    let unlocked = get_json(
        service.clone(),
        &format!("/api/v1/me/inbox-docs/{}", doc.id),
        &token_a,
    )
    .await;
    assert!(
        unlocked.json.get("payload").is_some(),
        "body is disclosed after receipt: {:?}",
        unlocked.json
    );

    // Idempotent re-confirm with a new step-up -> 200, same stamp.
    let a_stepup2 = fresh_step_up(&pool, user_a, "Employee A").await;
    let again = post_json(
        service.clone(),
        &format!("/api/v1/me/inbox-docs/{}/confirm-receipt", doc.id),
        &token_a,
        json!({ "step_up": a_stepup2 }),
    )
    .await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(again.json["confirmed_at"], confirmed.json["confirmed_at"]);

    // Unauthenticated request is rejected.
    let anon = service
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/me/inbox-docs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn inbox_payslip_filter_is_person_scoped_and_not_receipt_gated(pool: PgPool) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_key_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    let user_a = UserId::new();
    let user_b = UserId::new();
    seed_user(&pool, user_a, "Employee A", &["MEMBER"]).await;
    seed_user(&pool, user_b, "Employee B", &["MEMBER"]).await;

    let payslip = console_platform_request_context::scope_org(OrgId::knl(), async {
        PgInboxStore::new(pool.clone())
            .emit_inbox_doc(payslip_to(user_a))
            .await
    })
    .await
    .expect("emit payslip to A");
    let legal = console_platform_request_context::scope_org(OrgId::knl(), async {
        PgInboxStore::new(pool.clone())
            .emit_inbox_doc(legal_notice_to(user_a))
            .await
    })
    .await
    .expect("emit legal notice to A");
    assert_ne!(payslip.id, legal.id);

    let token_a = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        user_a,
        &["MEMBER"],
    );
    let token_b = issue_token(
        private_pem.as_bytes(),
        public_key_pem.as_bytes(),
        user_b,
        &["MEMBER"],
    );
    let verifier = JwtVerifier::from_es256_public_pem(
        JwtSettings {
            issuer: TEST_ISSUER.to_owned(),
            audience: TEST_AUDIENCE.to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        public_key_pem.as_bytes(),
    )
    .unwrap();
    // Recipient JWT already has empty feature_grants — no PayrollRunRead.
    // GET filter=payslip must still be 200 with the issued 명세서: this is
    // ESS vault (`/api/v1/me/inbox-docs`), not payroll REST `/payslips/me`.
    let recipient_claims = verifier
        .verify_access_token(&token_a)
        .expect("recipient JWT");
    assert!(
        recipient_claims.feature_grants.is_empty(),
        "ESS vault recipient JWT must carry empty feature_grants (no {}): {:?}",
        Feature::PayrollRunRead.as_str(),
        recipient_claims.feature_grants
    );
    assert!(
        !recipient_claims
            .feature_grants
            .iter()
            .any(|grant| grant == Feature::PayrollRunRead.as_str()),
        "GET filter=payslip must not require {}: {:?}",
        Feature::PayrollRunRead.as_str(),
        recipient_claims.feature_grants
    );
    let rt_pool = runtime_role_pool(&pool).await;
    let service = router(
        InboxRestState::new(PgInboxStore::new(rt_pool), Some(verifier))
            .with_passkey_step_up(Some(passkey_service())),
    );
    let payslip_id = payslip.id.to_string();
    let legal_id = legal.id.to_string();

    // MEMBER A lists 급여명세 via both aliases without PayrollRunRead: own
    // payslip only, never locked. ESS vault, not payroll REST.
    for filter in ["payslip", "pay"] {
        let list = get_json(
            service.clone(),
            &format!("{ME_INBOX_DOCS_PATH}?filter={filter}"),
            &token_a,
        )
        .await;
        assert_eq!(
            list.status,
            StatusCode::OK,
            "GET {ME_INBOX_DOCS_PATH}?filter={filter} is ESS vault, not payroll REST, and must be 200 without {}: {:?}",
            Feature::PayrollRunRead.as_str(),
            list.json
        );
        let items = list.json["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "{filter} must exclude legal_notice");
        assert_eq!(items[0]["id"].as_str().unwrap(), payslip_id);
        assert_eq!(items[0]["kind"].as_str(), Some("payslip"));
        assert_eq!(items[0]["locked"].as_bool(), Some(false));
        assert!(
            items[0].get("run_id").is_none(),
            "ESS vault list is not payroll REST /payslips/me: {:?}",
            items[0]
        );
        assert!(
            items[0].get("payload").is_none(),
            "list is metadata only: {:?}",
            items[0]
        );
    }

    // 전체 returns the payslip and the co-emitted legal_notice.
    let all = get_json(
        service.clone(),
        "/api/v1/me/inbox-docs?filter=all",
        &token_a,
    )
    .await;
    assert_eq!(all.status, StatusCode::OK, "{:?}", all.json);
    let all_items = all.json["items"].as_array().unwrap();
    assert_eq!(
        all_items.len(),
        2,
        "filter=all must return payslip and legal_notice: {:?}",
        all.json
    );
    let mut got: Vec<_> = all_items
        .iter()
        .map(|item| {
            (
                item["id"].as_str().unwrap().to_owned(),
                item["kind"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    got.sort_unstable();
    let mut expected = vec![
        (payslip_id.clone(), "payslip".to_owned()),
        (legal_id.clone(), "legal_notice".to_owned()),
    ];
    expected.sort_unstable();
    assert_eq!(got, expected, "filter=all: {:?}", all.json);
    for item in all_items {
        assert!(
            item.get("payload").is_none(),
            "list is metadata only: {item:?}"
        );
    }

    // Body is disclosed on GET with no receipt confirm and no passkey 428.
    let self_read = get_json(
        service.clone(),
        &format!("/api/v1/me/inbox-docs/{}", payslip.id),
        &token_a,
    )
    .await;
    assert_eq!(
        self_read.status,
        StatusCode::OK,
        "payslip self-view must not demand passkey step-up: {:?}",
        self_read.json
    );
    assert_eq!(self_read.json["kind"].as_str(), Some("payslip"));
    assert_eq!(self_read.json["locked"].as_bool(), Some(false));
    assert!(
        self_read.json.get("payload").is_some(),
        "payslip body is not receipt-gated: {:?}",
        self_read.json
    );
    assert_eq!(self_read.json["payload"]["net"], json!(3_120_000));

    // B lists empty (deny-by-omission) and cannot read A's documents.
    // Recipient is the JWT subject: B's token omits A's payslip even when
    // query/body try to name A.
    for filter in ["payslip", "all"] {
        let b_list = get_json(
            service.clone(),
            &format!("/api/v1/me/inbox-docs?filter={filter}"),
            &token_b,
        )
        .await;
        assert_eq!(b_list.status, StatusCode::OK, "{filter}: {:?}", b_list.json);
        assert_eq!(
            b_list.json["items"].as_array().unwrap().len(),
            0,
            "B must not see A's docs via {filter}: {:?}",
            b_list.json
        );
    }
    let b_recipient_query = get_json(
        service.clone(),
        &format!("{ME_INBOX_DOCS_PATH}?filter=payslip&recipient={user_a}"),
        &token_b,
    )
    .await;
    assert_eq!(
        b_recipient_query.status,
        StatusCode::OK,
        "recipient query must not change authz: {:?}",
        b_recipient_query.json
    );
    assert_eq!(
        b_recipient_query.json["items"].as_array().unwrap().len(),
        0,
        "recipient= must not widen B onto A's payslip: {:?}",
        b_recipient_query.json
    );
    let a_recipient_query = get_json(
        service.clone(),
        &format!("{ME_INBOX_DOCS_PATH}?filter=payslip&recipient={user_b}"),
        &token_a,
    )
    .await;
    assert_eq!(
        a_recipient_query.status,
        StatusCode::OK,
        "{:?}",
        a_recipient_query.json
    );
    let a_override_items = a_recipient_query.json["items"].as_array().unwrap();
    assert_eq!(
        a_override_items.len(),
        1,
        "recipient= must not switch A off their own payslip: {:?}",
        a_recipient_query.json
    );
    assert_eq!(a_override_items[0]["id"].as_str().unwrap(), payslip_id);
    let b_recipient_body = service
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("{ME_INBOX_DOCS_PATH}?filter=payslip"))
                .header(header::AUTHORIZATION, format!("Bearer {token_b}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "recipient": user_a })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let b_recipient_body = into_json(b_recipient_body).await;
    assert_eq!(
        b_recipient_body.status,
        StatusCode::OK,
        "recipient body must not change authz: {:?}",
        b_recipient_body.json
    );
    assert_eq!(
        b_recipient_body.json["items"].as_array().unwrap().len(),
        0,
        "JSON recipient body must not widen B onto A's payslip: {:?}",
        b_recipient_body.json
    );
    for (label, id) in [("payslip", &payslip_id), ("legal_notice", &legal_id)] {
        let cross_read = get_json(
            service.clone(),
            &format!("/api/v1/me/inbox-docs/{id}"),
            &token_b,
        )
        .await;
        assert_eq!(
            cross_read.status,
            StatusCode::NOT_FOUND,
            "B cannot read A's {label}: {:?}",
            cross_read.json
        );
    }

    // /api/v1 is Bearer-only. Missing Authorization is 401; an access cookie
    // (valid JWT or garbage) must not authorize REST.
    let payslip_uri = format!("{ME_INBOX_DOCS_PATH}?filter=payslip");
    let anon = get_without_authorization(service.clone(), &payslip_uri, None).await;
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);
    let cookie_only = get_without_authorization(
        service.clone(),
        &payslip_uri,
        Some(&format!("console_access={token_a}")),
    )
    .await;
    assert_eq!(
        cookie_only.status(),
        StatusCode::UNAUTHORIZED,
        "/api/v1 is Bearer-only; access cookie must not authorize REST"
    );
    let garbage_cookie = get_without_authorization(
        service.clone(),
        &payslip_uri,
        Some("console_access=not-a-jwt"),
    )
    .await;
    assert_eq!(garbage_cookie.status(), StatusCode::UNAUTHORIZED);
}

fn legal_notice_to(recipient: UserId) -> EmitInboxDocCommand {
    EmitInboxDocCommand {
        actor: None,
        recipient,
        doc: NewInboxDoc::new(
            InboxDocKind::LegalNotice,
            "연차 사용 촉진 통지 (1차)",
            Some("연차촉진"),
            Some("근로기준법 §61"),
            Some("workflow_run"),
            Some("AP-3111"),
            json!({ "paragraphs": ["귀하의 미사용 연차 사용을 촉진합니다."] }),
        )
        .unwrap(),
        dedup_key: None,
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    }
}

fn payslip_to(recipient: UserId) -> EmitInboxDocCommand {
    EmitInboxDocCommand {
        actor: None,
        recipient,
        doc: NewInboxDoc::new(
            InboxDocKind::Payslip,
            "6월 급여명세",
            None,
            None,
            Some("payroll_run"),
            Some("PR-2026-06"),
            json!({ "net": 3_120_000, "base": 2_800_000 }),
        )
        .unwrap(),
        dedup_key: None,
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    }
}

struct JsonResponse {
    status: StatusCode,
    json: Value,
}

async fn get_json(service: axum::Router, uri: &str, token: &str) -> JsonResponse {
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
    into_json(response).await
}

async fn get_without_authorization(
    service: axum::Router,
    uri: &str,
    cookie: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    service
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn post_json(service: axum::Router, uri: &str, token: &str, body: Value) -> JsonResponse {
    let response = service
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    into_json(response).await
}

async fn into_json(response: axum::response::Response) -> JsonResponse {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    JsonResponse { status, json }
}

fn issue_token(
    private_key_pem: &[u8],
    public_key_pem: &[u8],
    user_id: UserId,
    roles: &[&str],
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

async fn seed_user(pool: &PgPool, user_id: UserId, name: &str, roles: &[&str]) {
    let name = name.to_owned();
    let roles = roles
        .iter()
        .map(|role| (*role).to_owned())
        .collect::<Vec<_>>();
    let event = AuditEvent::new(
        None,
        AuditAction::new("test.seed_user").unwrap(),
        "user",
        user_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(OrgId::knl());
    with_audit(pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)",
            )
            .bind(user_id.as_uuid())
            .bind(format!("{name} {}", uuid::Uuid::new_v4()))
            .bind(roles)
            .bind(OrgId::knl().as_uuid())
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
}
