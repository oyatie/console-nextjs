//! Test preparation for approved R7c. No implementation/admission credit.
//! Design SHA: 8ef4ad7d85602c94aac05acfe0edf163b5ee3ede819d9e3cbb1e3c6d8b99b2ad.
//! Real signatures and nonowner PostgreSQL; literal missing routes yield behavior RED.
use super::*;
#[path = "browser_logout_review.rs"]
mod browser_logout_review;
#[path = "browser_payslips.rs"]
mod browser_payslips;
#[path = "resident_authenticator.rs"]
mod resident_authenticator;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use console_platform_auth::{JwtSettings, JwtVerifier};
use p256::elliptic_curve::rand_core::RngCore;
use resident_authenticator::ResidentAuthenticator;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use tokio::time::{sleep, timeout};

const WAIT: std::time::Duration = std::time::Duration::from_secs(10);

const LOGIN: &str = "/api/v1/auth/browser-session/login";
const START: &str = "/api/v1/auth/browser-session/start";
const LOGOUT: &str = "/api/v1/auth/browser-session/logout";
const HISTORY: &str = "/api/v1/hr/browser-session/attendance-records/me";
const MAPPING: &str = "auth_security.browser_sessions";

struct Fixture {
    router: axum::Router,
    runtime: PgPool,
    private: String,
    public: String,
    storage_key: String,
    verifier: JwtVerifier,
}

struct Actor {
    name: String,
    user: UserId,
    org: OrgId,
    source: Uuid,
    credential: String,
    authenticator: ResidentAuthenticator,
}

fn storage_key() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn configured_router(pool: PgPool, private: &str, public: &str, key: Option<&str>) -> axum::Router {
    configured_router_with(pool, private, public, key, &[])
}

fn configured_router_with(
    pool: PgPool,
    private: &str,
    public: &str,
    key: Option<&str>,
    extra: &[(&str, String)],
) -> axum::Router {
    let mut pairs = vec![
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".into()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.into()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.into()),
        ("CONSOLE_JWT_PRIVATE_KEY_PEM", private.into()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public.into()),
        ("CONSOLE_WEBAUTHN_RP_ID", "example.com".into()),
        ("CONSOLE_WEBAUTHN_RP_ORIGIN", TEST_ORIGIN.into()),
        ("CONSOLE_WEBAUTHN_RP_NAME", "Console".into()),
        (
            "CONSOLE_BROWSER_INGRESS_KEY",
            browser_ingress_key()[4..].into(),
        ),
        ("CONSOLE_TRUSTED_PROXY_COUNT", "1".into()),
        ("CONSOLE_TRUSTED_PROXY_CIDRS", "10.24.1.7/32".into()),
    ];
    if let Some(key) = key {
        pairs.push(("CONSOLE_BROWSER_SESSION_KEY_HEX", key.into()));
    }
    pairs.extend_from_slice(extra);
    // Baseline understands all original inputs and ignores the new key. This
    // compiles before implementation, rather than treating missing imports as RED.
    let config = AppConfig::from_pairs(pairs).unwrap();
    build_router(AppState::new(config, DatabaseDependency::Postgres(pool)).unwrap())
}

fn browser_ingress_key() -> &'static str {
    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        format!("bi1.{}", URL_SAFE_NO_PAD.encode(bytes))
    })
}

fn browser_ingress(builder: http::request::Builder) -> http::request::Builder {
    builder
        .header("x-console-browser-ingress", browser_ingress_key())
        .header("x-forwarded-for", "198.51.100.24")
        .extension(ConnectInfo("10.24.1.7:443".parse::<SocketAddr>().unwrap()))
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
    if [START, LOGIN, LOGOUT, HISTORY].contains(&uri)
        || uri.starts_with("/api/v1/me/browser-session/payslips")
    {
        builder = browser_ingress(builder);
    }
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    service
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

async fn enroll_discoverable_passkey(
    service: &axum::Router,
    authenticator: &mut ResidentAuthenticator,
    access_token: &str,
) -> String {
    accept_required_privacy_consent(service, access_token).await;
    let registration: RegisterStartResponse = post_json(
        service.clone(),
        "/api/v1/auth/passkey/register/start",
        Some(access_token),
        json!({"username":"new.user","display_name":"New User","require_discoverable":true}),
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
        json!({"ceremony_id":registration.ceremony_id,"credential":credential}),
        StatusCode::CREATED,
    )
    .await;
    finish.credential_id
}

async fn fixture(owner: &PgPool, label: &str) -> Fixture {
    let signing = SigningKey::random(&mut OsRng);
    let private = signing.to_pkcs8_pem(LineEnding::LF).unwrap().to_string();
    let public = signing
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let rt = runtime_login(owner, false, label).await;
    let storage_key = storage_key();
    let router = configured_router(rt.clone(), &private, &public, Some(&storage_key));
    let verifier = JwtVerifier::from_es256_public_pem(
        JwtSettings {
            issuer: TEST_ISSUER.into(),
            audience: TEST_AUDIENCE.into(),
            access_token_ttl: Duration::minutes(15),
        },
        public.as_bytes(),
    )
    .unwrap();
    Fixture {
        router,
        runtime: rt,
        private,
        public,
        storage_key,
        verifier,
    }
}

// Authenticate as the real serving principal: SET ROLE on an owner connection
// would not prove that a deployed runtime login lacks owner credentials.
pub(super) async fn runtime_login(owner: &PgPool, force: bool, label: &str) -> PgPool {
    let (role, variable) = if force {
        ("console_platform_force_cmd", "CONSOLE_TEST_FORCE_PASSWORD")
    } else {
        ("console_rt", "CONSOLE_TEST_RUNTIME_PASSWORD")
    };
    let password = std::env::var(variable).expect("disposable role password must be supplied");
    let options = owner
        .connect_options()
        .as_ref()
        .clone()
        .username(role)
        .password(&password)
        .application_name(label);
    assert!(
        PgPoolOptions::new()
            .max_connections(1)
            .connect_with(
                options
                    .clone()
                    .password("deliberately-wrong-local-test-password")
            )
            .await
            .is_err(),
        "trust-authenticated databases are not acceptance evidence"
    );
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    let identity: (String, String, bool, bool) = sqlx::query_as(
        "SELECT session_user::text,current_user::text,rolsuper,rolbypassrls FROM pg_roles WHERE rolname=current_user"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(identity, (role.into(), role.into(), false, false));
    pool
}

async fn actor(owner: &PgPool, f: &Fixture, name: &str) -> Actor {
    let org: Uuid = sqlx::query_scalar(
        "INSERT INTO organizations(slug,name,status) VALUES($1,$2,'ACTIVE') RETURNING id",
    )
    .bind(format!("browser-{}", Uuid::new_v4().simple()))
    .bind(format!("{name} 법인"))
    .fetch_one(owner)
    .await
    .unwrap();
    let org = OrgId::from_uuid(org);
    actor_in_org(owner, f, name, org).await
}

async fn actor_in_org(owner: &PgPool, f: &Fixture, name: &str, org: OrgId) -> Actor {
    let user = UserId::new();
    sqlx::query("INSERT INTO users(id,display_name,roles,org_id) VALUES($1,$2,ARRAY['MEMBER'],$3)")
        .bind(*user.as_uuid())
        .bind(name)
        .bind(*org.as_uuid())
        .execute(owner)
        .await
        .unwrap();
    let issue = BootstrapCredentialStore
        .issue_for_zero_credential_user(
            &f.runtime,
            *user.as_uuid(),
            org,
            OffsetDateTime::now_utc(),
            Duration::hours(1),
        )
        .await
        .unwrap();
    let enrollment: OtpRedeemResponse = post_json(
        f.router.clone(),
        "/api/v1/auth/otp/redeem",
        None,
        json!({"otp":issue.token.as_str()}),
        StatusCode::OK,
    )
    .await;
    let mut authenticator = ResidentAuthenticator::new().expect("resident browser fixture");
    let credential =
        enroll_discoverable_passkey(&f.router, &mut authenticator, &enrollment.access_token).await;
    let source: Uuid = sqlx::query_scalar(
        "SELECT id FROM auth_webauthn_credentials WHERE user_id=$1 AND org_id=$2 AND credential_id=$3",
    ).bind(*user.as_uuid()).bind(*org.as_uuid()).bind(&credential)
        .fetch_one(owner).await.unwrap();
    Actor {
        name: name.into(),
        user,
        org,
        source,
        credential,
        authenticator,
    }
}

// Recovery requires a real Company-owned branch scope before authentication.
// Keep ordinary unassigned actor fixtures unchanged.
async fn give_recovery_subject_branch(owner: &PgPool, subject: &Actor) {
    let region: Uuid =
        sqlx::query_scalar("INSERT INTO regions(name,org_id) VALUES($1,$2) RETURNING id")
            .bind(format!("recovery-{}", Uuid::new_v4()))
            .bind(*subject.org.as_uuid())
            .fetch_one(owner)
            .await
            .unwrap();
    let branch: Uuid = sqlx::query_scalar(
        "INSERT INTO branches(region_id,name,org_id) VALUES($1,'복구 대상 지점',$2) RETURNING id",
    )
    .bind(region)
    .bind(*subject.org.as_uuid())
    .fetch_one(owner)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_branches(user_id,branch_id,org_id) VALUES($1,$2,$3)")
        .bind(*subject.user.as_uuid())
        .bind(branch)
        .bind(*subject.org.as_uuid())
        .execute(owner)
        .await
        .unwrap();
}

async fn assertion(f: &Fixture, a: &mut Actor) -> (Uuid, Value) {
    let response = f
        .router
        .clone()
        .oneshot(
            browser_ingress(Request::builder().method("POST").uri(START))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let start: LoginStartResponse = response.into_json(StatusCode::OK).await;
    let challenge = start.challenge;
    let proof = a
        .authenticator
        .do_authentication(Url::parse(TEST_ORIGIN).unwrap(), challenge)
        .unwrap();
    (
        start.ceremony_id,
        json!({"ceremony_id":start.ceremony_id,"credential":proof}),
    )
}

// Native transport setup uses the original challenge and owners. The browser
// fixture chooses modal WebAuthn UI for conditional starts, as the old software
// authenticator ignored presentation. This is not conditional-browser proof;
// the unchanged C2 production prerequisite owns that evidence.
async fn resident_native_login(
    service: &axum::Router,
    actor: &mut Actor,
    cookie: bool,
) -> http::Response<Body> {
    let start: LoginStartResponse = post_json(
        service.clone(),
        "/api/v1/auth/passkey/login/start",
        None,
        json!({}),
        StatusCode::OK,
    )
    .await;
    let assertion = actor
        .authenticator
        .do_authentication(Url::parse(TEST_ORIGIN).unwrap(), start.challenge)
        .unwrap();
    let assertion = serde_json::to_value(assertion).unwrap();
    assert_eq!(assertion["id"], actor.credential);
    let body = json!({"ceremony_id":start.ceremony_id,"credential":assertion});
    if cookie {
        post_cookie_mode(
            service.clone(),
            "/api/v1/auth/passkey/login/finish",
            None,
            body,
        )
        .await
    } else {
        post_raw(
            service.clone(),
            "/api/v1/auth/passkey/login/finish",
            None,
            body,
        )
        .await
    }
}

async fn login(f: &Fixture, a: &mut Actor) -> Value {
    let (generation, body) = assertion(f, a).await;
    let response = post_raw(f.router.clone(), LOGIN, None, body).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "fresh signed browser login must exist"
    );
    assert!(
        set_cookie_values(&response).is_empty(),
        "native transport does not set browser cookies"
    );
    let result = body_json(response).await;
    let fields = result
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        fields,
        std::collections::BTreeSet::from(["context_id", "expires_at", "session_token"])
    );
    assert!(result["context_id"] == generation.to_string());
    let token = result["session_token"].as_str().unwrap();
    assert!(token.starts_with("bs1.") && token.len() == 47);
    assert!(
        token[4..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );
    let decoded = URL_SAFE_NO_PAD.decode(&token[4..]).unwrap();
    assert_eq!(decoded.len(), 32);
    assert_eq!(URL_SAFE_NO_PAD.encode(decoded), token[4..]);
    let expiry = OffsetDateTime::parse(
        result["expires_at"].as_str().unwrap(),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    assert!(
        expiry > OffsetDateTime::now_utc()
            && expiry <= OffsetDateTime::now_utc() + Duration::minutes(15)
    );
    result
}

fn handle(session: &Value) -> Value {
    json!({"browser_context":session["context_id"],"session_token":session["session_token"]})
}

fn history_handle(session: &Value) -> Value {
    let mut body = handle(session);
    body["limit"] = json!(25);
    body["offset"] = json!(0);
    body
}

fn transport_body(path: &str, session: &Value) -> Value {
    if path == HISTORY {
        history_handle(session)
    } else {
        handle(session)
    }
}

async fn own_history(f: &Fixture, session: &Value) -> Value {
    let response = history_raw(f, session, json!({"limit":25,"offset":0})).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "matching browser context must read its own native history"
    );
    assert!(set_cookie_values(&response).is_empty());
    let result = body_json(response).await;
    assert_eq!(
        result
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["context", "history", "browser_context", "expires_at"])
    );
    assert_eq!(result["browser_context"], session["context_id"]);
    assert_eq!(result["expires_at"], session["expires_at"]);
    assert_eq!(
        result["context"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([
            "company_id",
            "company_name",
            "account_display_name",
            "employee_linked"
        ])
    );
    result
}

async fn history_raw(f: &Fixture, session: &Value, pagination: Value) -> http::Response<Body> {
    let mut body = handle(session);
    body.as_object_mut()
        .unwrap()
        .extend(pagination.as_object().unwrap().clone());
    post_raw(f.router.clone(), HISTORY, None, body).await
}

// Test custody only: decrypt the same family's retained signed proof. Never expose
// it through a production endpoint or print it; another login would weaken the
// original-family bearer revocation and audit-leak assertions.
async fn private_original_proof(owner: &PgPool, f: &Fixture, session: &Value) -> String {
    use openssl::symm::{Cipher, decrypt_aead};
    type OriginalProofRow = (
        Uuid,
        Uuid,
        Uuid,
        Uuid,
        Uuid,
        OffsetDateTime,
        i32,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
    );
    let row: OriginalProofRow = sqlx::query_as(
        "SELECT account_id,company_id,family_id,source_credential_id,context_id,expires_at,codec_version,ciphertext,nonce,tag FROM auth_security.browser_sessions WHERE context_id=$1")
        .bind(Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap()).fetch_one(owner).await.unwrap();
    assert_eq!(row.6, 1);
    let mut aad = Vec::with_capacity(125);
    aad.extend_from_slice(&29_u32.to_be_bytes());
    aad.extend_from_slice(b"console/browser-session-proof");
    aad.extend_from_slice(&1_u32.to_be_bytes());
    for id in [row.0, row.1, row.2, row.3, row.4] {
        aad.extend_from_slice(id.as_bytes());
    }
    aad.extend_from_slice(&row.5.unix_timestamp().to_be_bytes());
    assert_eq!(aad.len(), 125);
    let key = hex::decode(&f.storage_key).unwrap();
    let bytes = decrypt_aead(
        Cipher::aes_256_gcm(),
        &key,
        Some(&row.8),
        &aad,
        &row.7,
        &row.9,
    )
    .unwrap();
    let proof = String::from_utf8(bytes).unwrap();
    let claims = f.verifier.verify_access_token(&proof).unwrap();
    assert!(
        claims.sub == row.0.to_string()
            && claims.org == row.1.to_string()
            && claims.session_family_id == Some(row.2)
            && claims.exp == row.5.unix_timestamp()
    );
    proof
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

async fn family(owner: &PgPool, session: &Value) -> Uuid {
    sqlx::query_scalar("SELECT family_id FROM auth_security.browser_sessions WHERE context_id=$1")
        .bind(Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap())
        .fetch_one(owner)
        .await
        .unwrap()
}

async fn effects(owner: &PgPool) -> [u8; 32] {
    custody_digest(owner, false).await
}

async fn custody_digest(owner: &PgPool, exclude_sealed_proof: bool) -> [u8; 32] {
    let mut state: Value = sqlx::query_scalar("SELECT jsonb_build_object( \
        'mapping',(SELECT jsonb_agg(to_jsonb(s) ORDER BY context_id) FROM auth_security.browser_sessions s), \
        'families',(SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM auth_refresh_token_families f), \
        'tokens',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM auth_refresh_tokens t), \
        'audits',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM audit_events a), \
        'users',(SELECT jsonb_agg(to_jsonb(u) ORDER BY id) FROM users u), \
        'companies',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM organizations o), \
        'groups',(SELECT jsonb_agg(to_jsonb(g) ORDER BY id) FROM groups g), \
        'membership',(SELECT jsonb_agg(to_jsonb(m) ORDER BY group_id,org_id) FROM group_memberships m), \
        'keys',(SELECT jsonb_agg(to_jsonb(k) ORDER BY id) FROM auth_webauthn_credentials k), \
        'bootstrap',(SELECT jsonb_agg(to_jsonb(b) ORDER BY id) FROM auth_bootstrap_credentials b), \
        'otp_sources',(SELECT jsonb_agg(to_jsonb(b) ORDER BY family_id) FROM auth_legacy_otp_family_sources b), \
        'registration_bindings',(SELECT jsonb_agg(to_jsonb(b) ORDER BY ceremony_id) FROM auth_legacy_registration_bindings b), \
        'ceremony_bindings',(SELECT jsonb_agg(to_jsonb(b) ORDER BY ceremony_id) FROM auth_webauthn_ceremony_bindings b), \
        'ceremonies',(SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM auth_webauthn_ceremonies c), \
        'freshness',(SELECT jsonb_agg(to_jsonb(v) ORDER BY org_id,user_id) FROM subject_authz_versions v), \
        'account_state',(SELECT jsonb_agg(to_jsonb(s) ORDER BY account_id) FROM auth_security.account_state s), \
        'reservations',(SELECT jsonb_agg(to_jsonb(r) ORDER BY account_id) FROM auth_security.account_id_reservations r), \
        'removals',(SELECT jsonb_agg(to_jsonb(r) ORDER BY event_id) FROM auth_security.credential_removals r))")
    .fetch_one(owner)
    .await
    .unwrap();
    if exclude_sealed_proof {
        for mapping in state["mapping"].as_array_mut().into_iter().flatten() {
            let fields = mapping.as_object_mut().unwrap();
            for name in ["ciphertext", "nonce", "tag"] {
                fields.remove(name);
            }
        }
    }
    // Compare custody state without printing proof material on assertion failure.
    Sha256::digest(serde_json::to_vec(&state).unwrap()).into()
}

async fn complete_mapping_digest(owner: &PgPool, session: &Value) -> [u8; 32] {
    let row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(s) FROM auth_security.browser_sessions s WHERE context_id=$1",
    )
    .bind(Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap())
    .fetch_one(owner)
    .await
    .unwrap();
    Sha256::digest(serde_json::to_vec(&row).unwrap()).into()
}

async fn immutable_mapping(owner: &PgPool, session: &Value) -> [u8; 32] {
    let state: Value = sqlx::query_scalar(
        "SELECT to_jsonb(s)-'ciphertext'-'nonce'-'tag'-'closed_at'-'owner_removed_at' FROM auth_security.browser_sessions s WHERE context_id=$1",
    ).bind(Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap())
        .fetch_one(owner).await.unwrap();
    Sha256::digest(serde_json::to_vec(&state).unwrap()).into()
}

async fn assert_closed_mapping(owner: &PgPool, session: &Value, original: [u8; 32]) {
    let closed: bool = sqlx::query_scalar(
        "SELECT closed_at IS NOT NULL AND owner_removed_at IS NULL AND ciphertext IS NULL AND nonce IS NULL AND tag IS NULL FROM auth_security.browser_sessions WHERE context_id=$1",
    ).bind(Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap())
        .fetch_one(owner).await.unwrap();
    assert!(
        closed,
        "confirmed logout must record closure and clear sealed proof"
    );
    assert_eq!(immutable_mapping(owner, session).await, original);
}

async fn add_passkey(f: &Fixture, a: &mut Actor, bearer: &str) {
    let (_, step_up) = assertion(f, a).await;
    let start: RegisterStartResponse = post_json(
        f.router.clone(),
        "/api/v1/auth/passkey/register/start",
        Some(bearer),
        json!({"username":"second-device","display_name":"Second device","step_up":step_up}),
        StatusCode::OK,
    )
    .await;
    let mut other = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = other
        .do_registration(Url::parse(TEST_ORIGIN).unwrap(), start.challenge)
        .unwrap();
    let _: RegisterFinishResponse = post_json(
        f.router.clone(),
        "/api/v1/auth/passkey/register/finish",
        Some(bearer),
        json!({"ceremony_id":start.ceremony_id,"credential":credential}),
        StatusCode::CREATED,
    )
    .await;
}

// Positive prerequisite independent of the missing browser-session routes.
// Native owners must accept actual resident browser signatures and retain their
// body/cookie contracts before a missing-route RED can authorize implementation.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn resident_fixture_preserves_real_native_login_and_exact_family_logout(pool: PgPool) {
    let f = fixture(&pool, "resident-native-prerequisite").await;
    let mut a = actor(&pool, &f, "실제 상주 키").await;
    let first: TokenPairResponse = resident_native_login(&f.router, &mut a, false)
        .await
        .into_json(StatusCode::OK)
        .await;
    let claims = f.verifier.verify_access_token(&first.access_token).unwrap();
    assert_eq!(claims.sub, a.user.to_string());
    assert_eq!(claims.org, a.org.to_string());
    assert!(first.refresh_token.is_some());
    assert_eq!(
        get(&f, "/api/v1/hr/attendance-records/me", &first.access_token)
            .await
            .status(),
        StatusCode::OK
    );

    let second = resident_native_login(&f.router, &mut a, true).await;
    assert_eq!(second.status(), StatusCode::OK);
    let cookies = set_cookie_values(&second);
    assert_eq!(cookies.len(), 1);
    let cookie_pair = cookies[0].split(';').next().unwrap();
    let (name, cookie) = cookie_pair.split_once('=').unwrap();
    assert_eq!(name, "console_refresh");
    assert!(!cookie.is_empty());
    let cookie = cookie.to_owned();
    let second: TokenPairResponse = second.into_json(StatusCode::OK).await;
    assert!(second.refresh_token.is_none());
    let second_claims = f
        .verifier
        .verify_access_token(&second.access_token)
        .unwrap();
    assert_ne!(claims.session_family_id, second_claims.session_family_id);
    assert_eq!(
        get(&f, "/api/v1/hr/attendance-records/me", &second.access_token)
            .await
            .status(),
        StatusCode::OK
    );
    let logout = post_cookie_mode(
        f.router.clone(),
        "/api/v1/auth/logout",
        Some(&cookie),
        json!({}),
    )
    .await;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        get(&f, "/api/v1/hr/attendance-records/me", &second.access_token)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&f, "/api/v1/hr/attendance-records/me", &first.access_token)
            .await
            .status(),
        StatusCode::OK
    );
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action='auth.logout' AND org_id=$1",
    )
    .bind(*a.org.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
    a.authenticator
        .close()
        .expect("confirmed resident fixture cleanup");
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn browser_login_binds_original_proof_and_parallel_resolution_never_rotates(pool: PgPool) {
    let f = fixture(&pool, "browser-stable").await;
    let mut a = actor(&pool, &f, "안정 세션").await;
    let session = login(&f, &mut a).await;
    let resolved = own_history(&f, &session).await;
    let token = private_original_proof(&pool, &f, &session).await;
    let claims = f.verifier.verify_access_token(&token).unwrap();
    assert!(claims.sub == a.user.to_string());
    let hash = Sha256::digest(session["session_token"].as_str().unwrap().as_bytes()).to_vec();
    let bound: bool = sqlx::query_scalar("SELECT token_hash=$1 AND account_id=$2 AND company_id=$3 AND source_credential_id=$4 AND octet_length(nonce)=12 AND octet_length(tag)=16 AND octet_length(ciphertext)>0 AND expires_at=$5 FROM auth_security.browser_sessions WHERE context_id=$6")
        .bind(hash).bind(*a.user.as_uuid()).bind(*a.org.as_uuid()).bind(a.source)
        .bind(OffsetDateTime::from_unix_timestamp(claims.exp).unwrap()).bind(Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap())
        .fetch_one(&pool).await.unwrap();
    assert!(
        bound,
        "mapping must retain exact source, deadline and encrypted proof"
    );
    let before = effects(&pool).await;
    let (one, two, three) = tokio::join!(
        own_history(&f, &session),
        own_history(&f, &session),
        own_history(&f, &session)
    );
    assert!(one == resolved && two == resolved && three == resolved);
    assert_eq!(
        effects(&pool).await,
        before,
        "resolution cannot mint or rotate authority"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn browser_context_substitution_unknown_and_noncanonical_handles_deny_without_effect(
    pool: PgPool,
) {
    let f = fixture(&pool, "browser-binding").await;
    let mut a = actor(&pool, &f, "첫 법인").await;
    let mut b = actor(&pool, &f, "둘째 법인").await;
    let first = login(&f, &mut a).await;
    let second = login(&f, &mut b).await;
    let before = effects(&pool).await;
    let mismatched =
        json!({"browser_context":second["context_id"],"session_token":first["session_token"]});
    for body in [
        mismatched,
        json!({"browser_context":first["context_id"],"session_token":"bs1.invalid"}),
        json!({"browser_context":first["context_id"],"session_token":format!("{}=",first["session_token"].as_str().unwrap())}),
        json!({"browser_context":Uuid::new_v4(),"session_token":format!("bs1.{}",URL_SAFE_NO_PAD.encode([37_u8;32]))}),
    ] {
        for path in [HISTORY, LOGOUT] {
            let mut requested = body.clone();
            if path == HISTORY {
                requested["limit"] = json!(25);
                requested["offset"] = json!(0);
            }
            let response = post_raw(f.router.clone(), path, None, requested).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            let error = body_json(response).await;
            assert!(
                error.get("access_token").is_none()
                    && error.get("account_id").is_none()
                    && error.get("company_id").is_none()
            );
        }
    }
    assert_eq!(effects(&pool).await, before);
    own_history(&f, &first).await;
    own_history(&f, &second).await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn browser_logout_closes_only_bound_family_and_reconciles_idempotently(pool: PgPool) {
    let f = fixture(&pool, "browser-logout").await;
    let mut a = actor(&pool, &f, "가족 종료").await;
    let first = login(&f, &mut a).await;
    let second = login(&f, &mut a).await;
    let first_family = family(&pool, &first).await;
    let second_family = family(&pool, &second).await;
    assert_ne!(first_family, second_family);
    let original_bearer = private_original_proof(&pool, &f, &first).await;
    let immutable = immutable_mapping(&pool, &first).await;
    let response = post_raw(f.router.clone(), LOGOUT, None, handle(&first)).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        to_bytes(response.into_body(), 4096)
            .await
            .unwrap()
            .is_empty()
    );
    let closed: (bool,bool) = sqlx::query_as("SELECT (SELECT revoked_at IS NOT NULL FROM auth_refresh_token_families WHERE id=$1), (SELECT bool_and(revoked_at IS NOT NULL) FROM auth_refresh_tokens WHERE family_id=$1)")
        .bind(first_family).fetch_one(&pool).await.unwrap();
    assert_eq!(closed, (true, true));
    assert_closed_mapping(&pool, &first, immutable).await;
    assert_eq!(
        get(&f, "/api/v1/hr/attendance-records/me", &original_bearer)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_raw(f.router.clone(), HISTORY, None, history_handle(&first))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    own_history(&f, &second).await;
    let before = effects(&pool).await;
    assert_eq!(
        post_raw(f.router.clone(), LOGOUT, None, handle(&first))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        effects(&pool).await,
        before,
        "terminal retry creates no new audit/effect"
    );
    let logout_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action='auth.logout' AND target_id=$1",
    )
    .bind(first_family.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(logout_audits, 1);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn removed_exact_passkey_denies_browser_access_but_preserves_logout_identity(pool: PgPool) {
    let f = fixture(&pool, "browser-source-removal").await;
    let mut a = actor(&pool, &f, "기록된 인증키").await;
    let session = login(&f, &mut a).await;
    let bearer = private_original_proof(&pool, &f, &session).await;
    // The owner must preserve the sole-key guard, not weaken it for this path.
    let before = effects(&pool).await;
    let denied = f
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/auth/passkeys/{}", a.source))
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::CONFLICT);
    assert_eq!(effects(&pool).await, before);
    own_history(&f, &session).await;
    add_passkey(&f, &mut a, &bearer).await;
    let response = f
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/auth/passkeys/{}", a.source))
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        post_raw(f.router.clone(), HISTORY, None, history_handle(&session))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_raw(f.router.clone(), LOGOUT, None, handle(&session))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post_raw(f.router.clone(), LOGOUT, None, handle(&session))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn changed_current_account_role_generation_and_company_each_deny_original_browser_proof(
    pool: PgPool,
) {
    let f = fixture(&pool, "browser-current-authority").await;
    for (index,change) in [
        "UPDATE users SET is_active=false WHERE id=$1",
        "UPDATE users SET roles=ARRAY['MECHANIC'] WHERE id=$1",
        "UPDATE subject_authz_versions SET session_generation=session_generation+1 WHERE user_id=$1",
        "UPDATE subject_authz_versions SET version=version+1 WHERE user_id=$1",
        "UPDATE organizations SET status='SUSPENDED' WHERE id=$1",
    ].iter().enumerate() {
        let mut a=actor(&pool,&f,&format!("현재 권한 {index}")).await;
        sqlx::query("INSERT INTO subject_authz_versions(user_id,org_id,version,session_generation) VALUES($1,$2,1,1)")
            .bind(*a.user.as_uuid()).bind(*a.org.as_uuid()).execute(&pool).await.unwrap();
        let session=login(&f,&mut a).await;
        own_history(&f,&session).await;
        let query=sqlx::query(*change).bind(if index==4{*a.org.as_uuid()}else{*a.user.as_uuid()});
        query.execute(&pool).await.unwrap();
        let before=effects(&pool).await;
        assert_eq!(post_raw(f.router.clone(),HISTORY,None,history_handle(&session)).await.status(),StatusCode::UNAUTHORIZED);
        assert_eq!(effects(&pool).await,before);
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn missing_malformed_and_replaced_browser_key_never_weaken_original_native_login(
    pool: PgPool,
) {
    let f = fixture(&pool, "browser-key-boundary").await;
    let mut a = actor(&pool, &f, "키 경계").await;
    let session = login(&f, &mut a).await;
    for key in [None, Some("invalid"), Some("00")] {
        let router = configured_router(f.runtime.clone(), &f.private, &f.public, key);
        let before = effects(&pool).await;
        assert_eq!(
            post_raw(router.clone(), HISTORY, None, history_handle(&session))
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(effects(&pool).await, before);
        // Missing browser transport configuration must not disable native passkeys.
        let native = resident_native_login(&router, &mut a, false).await;
        let _: TokenPairResponse = native.into_json(StatusCode::OK).await;
    }

    let different = storage_key();
    assert!(different != f.storage_key);
    let replaced = configured_router(f.runtime.clone(), &f.private, &f.public, Some(&different));
    assert_eq!(
        post_raw(replaced, HISTORY, None, history_handle(&session))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    own_history(&f, &session).await;
}

async fn link_employee(owner: &PgPool, a: &Actor) -> Uuid {
    let employee = Uuid::new_v4();
    sqlx::query("INSERT INTO employees(id,org_id,company,name,source_filename,source_sheet,source_row,source_key,raw_row,source_metadata) VALUES($1,$2,'테스트',$3,'browser-test.xlsx','직원',2,$4,'{}','{}')")
        .bind(employee).bind(*a.org.as_uuid()).bind(&a.name)
        .bind(format!("browser-{employee}")).execute(owner).await.unwrap();
    sqlx::query("UPDATE users SET employee_id=$1 WHERE id=$2 AND org_id=$3")
        .bind(employee)
        .bind(*a.user.as_uuid())
        .bind(*a.org.as_uuid())
        .execute(owner)
        .await
        .unwrap();
    employee
}

async fn create_records(f: &Fixture, a: &Actor, bearer: &str, count: u32) {
    for index in 0..count {
        let response = post_raw(f.router.clone(), "/api/v1/hr/attendance-records/me", Some(bearer),
            json!({"kind":if index%2==0{"CLOCK_IN"}else{"CLOCK_OUT"},"idempotency_key":format!("browser-{}-{index}",a.user)})).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn browser_history_context_is_own_only_and_preserves_old_wire_contract(pool: PgPool) {
    let f = fixture(&pool, "browser-history").await;
    let mut a = actor(&pool, &f, "근태 주체").await;
    let mut b = actor(&pool, &f, "다른 법인 주체").await;
    let own = login(&f, &mut a).await;
    let other = login(&f, &mut b).await;
    let bearer = private_original_proof(&pool, &f, &own).await;
    let other_bearer = private_original_proof(&pool, &f, &other).await;
    let mut coworker = actor_in_org(&pool, &f, "같은 법인 동료", a.org).await;
    let coworker_session = login(&f, &mut coworker).await;
    let coworker_bearer = private_original_proof(&pool, &f, &coworker_session).await;
    let employee = link_employee(&pool, &a).await;
    for (actor, token) in [
        (&a, &bearer),
        (&b, &other_bearer),
        (&coworker, &coworker_bearer),
    ] {
        if actor.user != a.user {
            link_employee(&pool, actor).await;
        }
        create_records(&f, actor, token, 28).await;
    }
    let response = history_raw(&f, &own, json!({"limit":9,"offset":9})).await;
    assert_eq!(response.status(), StatusCode::OK);
    let result = body_json(response).await;
    assert!(result["context"]["company_id"] == a.org.to_string());
    assert_eq!(result["context"]["company_name"], "근태 주체 법인");
    assert_eq!(result["context"]["account_display_name"], "근태 주체");
    assert_eq!(result["context"]["employee_linked"], true);
    let legacy = body_json(
        get(
            &f,
            "/api/v1/hr/attendance-records/me?limit=9&offset=9",
            &bearer,
        )
        .await,
    )
    .await;
    assert_eq!(result["history"], legacy);
    assert_eq!(result["history"]["total"], 28);
    assert_eq!(result["history"]["items"].as_array().unwrap().len(), 9);
    for item in result["history"]["items"].as_array().unwrap() {
        assert_eq!(item["employee_id"], employee.to_string());
        assert_eq!(item["employee_display_name"], "근태 주체");
        assert_eq!(
            item["occurred_at"].as_array().unwrap().len(),
            9,
            "retain actual bare time tuple"
        );
    }
    let out = body_json(history_raw(&f, &own, json!({"offset":100,"limit":25})).await).await;
    assert_eq!(out["context"]["employee_linked"], true);
    assert_eq!(out["history"]["total"], 28);
    assert_eq!(out["history"]["offset"], 100);
    assert_eq!(out["history"]["items"], json!([]));
    assert_eq!(
        history_raw(
            &f,
            &own,
            json!({"limit":25,"offset":0,"employee_id":Uuid::new_v4()})
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn browser_context_link_count_and_rows_share_a_witnessed_snapshot(pool: PgPool) {
    let f = fixture(&pool, "browser-snapshot-reader").await;
    let mut a = actor(&pool, &f, "스냅샷 이전 주체").await;
    let mut coworker = actor_in_org(&pool, &f, "새 연결 동료", a.org).await;
    let first = login(&f, &mut a).await;
    let second = login(&f, &mut coworker).await;
    let bearer = private_original_proof(&pool, &f, &first).await;
    let other_bearer = private_original_proof(&pool, &f, &second).await;
    let original_employee = link_employee(&pool, &a).await;
    let replacement_employee = link_employee(&pool, &coworker).await;
    create_records(&f, &a, &bearer, 4).await;
    create_records(&f, &coworker, &other_bearer, 2).await;
    let before = own_history(&f, &first).await;
    assert_eq!(before["history"]["total"], 4);
    sqlx::raw_sql("CREATE FUNCTION public.test_browser_snapshot_gate() RETURNS boolean LANGUAGE plpgsql AS $$ BEGIN IF current_setting('application_name')='browser-snapshot-reader' THEN PERFORM pg_advisory_xact_lock(870241); END IF; RETURN true; END $$; CREATE POLICY test_browser_snapshot_gate ON public.employee_attendance_records AS RESTRICTIVE FOR SELECT TO console_rt USING(public.test_browser_snapshot_gate());")
        .execute(&pool).await.unwrap();
    let mut gate = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(870241)")
        .execute(gate.as_mut())
        .await
        .unwrap();
    let pid = super::reset_sessions::transaction_pid(&mut gate).await;
    let reader = f.router.clone();
    let mut request = handle(&first);
    request["limit"] = json!(25);
    request["offset"] = json!(0);
    let pending = tokio::spawn(async move {
        reader
            .oneshot(
                browser_ingress(
                    Request::builder()
                        .method("POST")
                        .uri(HISTORY)
                        .header(header::CONTENT_TYPE, "application/json"),
                )
                .body(Body::from(request.to_string()))
                .unwrap(),
            )
            .await
            .unwrap()
    });
    super::reset_sessions::waiting(
        &pool,
        "browser-snapshot-reader",
        pid,
        "employee_attendance_records",
    )
    .await;
    let writer = runtime_login(&pool, false, "browser-snapshot-writer").await;
    let writer_router = configured_router(writer, &f.private, &f.public, Some(&f.storage_key));
    let response = post_raw(
        writer_router,
        "/api/v1/hr/attendance-records/me",
        Some(&bearer),
        json!({"kind":"CLOCK_IN","idempotency_key":"snapshot-competing-event"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut change = pool.begin().await.unwrap();
    sqlx::query("UPDATE users SET employee_id=NULL WHERE id=$1 AND org_id=$2")
        .bind(*coworker.user.as_uuid())
        .bind(*a.org.as_uuid())
        .execute(change.as_mut())
        .await
        .unwrap();
    sqlx::query("UPDATE users SET display_name='스냅샷 이후 주체',employee_id=$1 WHERE id=$2")
        .bind(replacement_employee)
        .bind(*a.user.as_uuid())
        .execute(change.as_mut())
        .await
        .unwrap();
    sqlx::query("UPDATE organizations SET name='스냅샷 이후 법인' WHERE id=$1")
        .bind(*a.org.as_uuid())
        .execute(change.as_mut())
        .await
        .unwrap();
    change.commit().await.unwrap();
    gate.commit().await.unwrap();
    let response = timeout(WAIT, pending).await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot = body_json(response).await;
    assert_eq!(
        snapshot, before,
        "in-flight context, count and rows must preserve one authorized snapshot"
    );
    for item in snapshot["history"]["items"].as_array().unwrap() {
        assert_eq!(item["employee_id"], original_employee.to_string());
    }
    let after = own_history(&f, &first).await;
    assert_eq!(after["context"]["company_name"], "스냅샷 이후 법인");
    assert_eq!(after["context"]["account_display_name"], "스냅샷 이후 주체");
    assert_eq!(after["context"]["employee_linked"], true);
    assert_eq!(after["history"]["total"], 2);
    for item in after["history"]["items"].as_array().unwrap() {
        assert_eq!(item["employee_id"], replacement_employee.to_string());
    }
    sqlx::raw_sql("DROP POLICY test_browser_snapshot_gate ON public.employee_attendance_records; DROP FUNCTION public.test_browser_snapshot_gate();")
        .execute(&pool).await.unwrap();
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn unlinked_and_out_of_range_browser_history_are_real_distinct_states(pool: PgPool) {
    let f = fixture(&pool, "browser-empty-history").await;
    let mut a = actor(&pool, &f, "연결 전 계정").await;
    let session = login(&f, &mut a).await;
    let bearer = private_original_proof(&pool, &f, &session).await;
    let response = history_raw(&f, &session, json!({"offset":100,"limit":25})).await;
    assert_eq!(response.status(), StatusCode::OK);
    let result = body_json(response).await;
    assert_eq!(result["context"]["employee_linked"], false);
    let legacy = body_json(
        get(
            &f,
            "/api/v1/hr/attendance-records/me?limit=25&offset=100",
            &bearer,
        )
        .await,
    )
    .await;
    assert_eq!(result["history"], legacy);
    assert_eq!(result["history"]["total"], 0);
    assert_eq!(result["history"]["offset"], 100);
    assert_eq!(result["history"]["items"], json!([]));
    link_employee(&pool, &a).await;
    let linked_empty = own_history(&f, &session).await;
    assert_eq!(linked_empty["context"]["employee_linked"], true);
    assert_eq!(linked_empty["history"]["total"], 0);
    assert_eq!(linked_empty["history"]["items"], json!([]));
    assert_eq!(
        history_raw(&f, &session, json!({"limit":25,"offset":"invalid"}))
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let before = effects(&pool).await;
    for field in ["limit", "offset"] {
        let mut body = history_handle(&session);
        body.as_object_mut().unwrap().remove(field);
        let response = post_raw(f.router.clone(), HISTORY, None, body).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(effects(&pool).await, before);
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn browser_history_pagination_has_exact_native_integer_bounds_without_effects(pool: PgPool) {
    let f = fixture(&pool, "browser-pagination-bounds").await;
    let mut a = actor(&pool, &f, "페이지 경계 주체").await;
    let session = login(&f, &mut a).await;
    let proof = private_original_proof(&pool, &f, &session).await;
    link_employee(&pool, &a).await;
    create_records(&f, &a, &proof, 3).await;
    let before = effects(&pool).await;
    let mapping = immutable_mapping(&pool, &session).await;

    // The native owner sends offset directly to PostgreSQL without adding it
    // to limit. Its nonnegative i64 range is wider than the browser's safe
    // page arithmetic. Do not impose a new JS ceiling on the native contract.
    for (limit, offset, rows) in [
        (1_i64, 0_i64, 1_usize),
        (1000, 0, 3),
        (25, 9_007_199_254_740_992, 0),
        (1, i64::MAX - 1, 0),
        (1000, i64::MAX, 0),
    ] {
        let response = history_raw(&f, &session, json!({"limit":limit,"offset":offset})).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(set_cookie_values(&response).is_empty());
        let result = body_json(response).await;
        let legacy = get(
            &f,
            &format!("/api/v1/hr/attendance-records/me?limit={limit}&offset={offset}"),
            &proof,
        )
        .await;
        assert_eq!(legacy.status(), StatusCode::OK);
        assert_eq!(result["history"], body_json(legacy).await);
        assert_eq!(result["history"]["limit"], limit);
        assert_eq!(result["history"]["offset"], offset);
        assert_eq!(result["history"]["total"], 3);
        assert_eq!(result["history"]["items"].as_array().unwrap().len(), rows);
        assert_eq!(result["browser_context"], session["context_id"]);
        assert_eq!(result["context"]["company_id"], a.org.to_string());
        assert_eq!(result["context"]["account_display_name"], a.name);
        assert_eq!(effects(&pool).await, before);
        assert!(immutable_mapping(&pool, &session).await == mapping);
    }
    for pagination in [
        json!({"limit":0,"offset":0}),
        json!({"limit":-1,"offset":0}),
        json!({"limit":1001,"offset":0}),
        json!({"limit":0.5,"offset":0}),
        json!({"limit":25.0,"offset":0}),
        json!({"limit":null,"offset":0}),
        json!({"limit":"25","offset":0}),
        json!({"limit":true,"offset":0}),
        json!({"limit":9_007_199_254_740_992_u64,"offset":0}),
        json!({"limit":25,"offset":-1}),
        json!({"limit":25,"offset":0.5}),
        json!({"limit":25,"offset":0.0}),
        json!({"limit":25,"offset":null}),
        json!({"limit":25,"offset":"0"}),
        json!({"limit":25,"offset":false}),
        json!({"limit":25,"offset":i64::MAX as u64 + 1}),
        json!({"limit":25,"offset":u64::MAX}),
    ] {
        let response = history_raw(&f, &session, pagination).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(set_cookie_values(&response).is_empty());
        assert_eq!(effects(&pool).await, before);
        assert!(immutable_mapping(&pool, &session).await == mapping);
    }
    own_history(&f, &session).await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn consumed_native_assertion_and_bearer_otp_or_refresh_cannot_convert_to_browser_session(
    pool: PgPool,
) {
    let f = fixture(&pool, "browser-no-conversion").await;
    let mut a = actor(&pool, &f, "권한 변환 금지").await;
    login(&f, &mut a).await;
    let (_, signed) = assertion(&f, &mut a).await;
    let original = post_raw(
        f.router.clone(),
        "/api/v1/auth/passkey/login/finish",
        None,
        signed.clone(),
    )
    .await;
    assert_eq!(original.status(), StatusCode::OK);
    let original = body_json(original).await;
    let before = effects(&pool).await;
    assert_eq!(
        post_raw(f.router.clone(), LOGIN, None, signed)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for body in [
        json!({"access_token": original["access_token"]}),
        json!({"refresh_token": original["refresh_token"]}),
        json!({"otp":"not-a-signed-passkey-proof"}),
    ] {
        let response = post_raw(
            f.router.clone(),
            LOGIN,
            original["access_token"].as_str(),
            body,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(set_cookie_values(&response).is_empty());
    }
    assert_eq!(effects(&pool).await, before);
    assert_eq!(
        get(
            &f,
            "/api/v1/hr/attendance-records/me",
            original["access_token"].as_str().unwrap()
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn mapping_insert_failure_rolls_back_proof_counter_family_tokens_and_audits(pool: PgPool) {
    let f = fixture(&pool, "browser-mapping-atomicity").await;
    let mut a = actor(&pool, &f, "원자적 로그인").await;
    login(&f, &mut a).await;
    let (ceremony, signed) = assertion(&f, &mut a).await;
    let key_before: Value =
        sqlx::query_scalar("SELECT passkey_json FROM auth_webauthn_credentials WHERE id=$1")
            .bind(a.source)
            .fetch_one(&pool)
            .await
            .unwrap();
    let before = effects(&pool).await;
    sqlx::raw_sql("CREATE FUNCTION public.test_reject_browser_mapping() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'forced browser mapping failure' USING ERRCODE='23514'; END $$; CREATE TRIGGER test_reject_browser_mapping BEFORE INSERT ON auth_security.browser_sessions FOR EACH ROW EXECUTE FUNCTION public.test_reject_browser_mapping();")
        .execute(&pool).await.unwrap();
    let response = post_raw(f.router.clone(), LOGIN, None, signed.clone()).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(set_cookie_values(&response).is_empty());
    assert_eq!(effects(&pool).await, before);
    let unused: bool =
        sqlx::query_scalar("SELECT consumed_at IS NULL FROM auth_webauthn_ceremonies WHERE id=$1")
            .bind(ceremony)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        unused,
        "failed mapping commit cannot burn a signed assertion"
    );
    let key_after: Value =
        sqlx::query_scalar("SELECT passkey_json FROM auth_webauthn_credentials WHERE id=$1")
            .bind(a.source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        key_before == key_after,
        "failed mapping commit cannot advance credential state"
    );
    sqlx::query("DROP TRIGGER test_reject_browser_mapping ON auth_security.browser_sessions")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        post_raw(f.router.clone(), LOGIN, None, signed.clone())
            .await
            .status(),
        StatusCode::OK
    );
    let committed = effects(&pool).await;
    assert_eq!(
        post_raw(f.router.clone(), LOGIN, None, signed)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(effects(&pool).await, committed);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn encrypted_proof_or_metadata_tampering_is_denied_without_reminting_authority(pool: PgPool) {
    let f = fixture(&pool, "browser-encrypted-binding").await;
    for column in ["ciphertext", "nonce", "tag"] {
        let mut a = actor(&pool, &f, &format!("암호화 검증 {column}")).await;
        // Owner-only INSERT-time corruption preserves immutable UPDATE guards.
        // This is a storage fault, not a provider success or authentic issuance claim.
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE FUNCTION public.test_browser_corruption() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.{column}=set_byte(NEW.{column},0,get_byte(NEW.{column},0)#1); RETURN NEW; END $$; CREATE TRIGGER test_browser_corruption BEFORE INSERT ON {MAPPING} FOR EACH ROW EXECUTE FUNCTION public.test_browser_corruption();")))
            .execute(&pool).await.unwrap();
        let session = login(&f, &mut a).await;
        sqlx::raw_sql("DROP TRIGGER test_browser_corruption ON auth_security.browser_sessions; DROP FUNCTION public.test_browser_corruption();")
            .execute(&pool).await.unwrap();
        let before = effects(&pool).await;
        assert_eq!(
            post_raw(f.router.clone(), HISTORY, None, history_handle(&session))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(effects(&pool).await, before);
    }
    let mut a = actor(&pool, &f, "원본 출처 바인딩").await;
    let b = actor(&pool, &f, "대체 출처").await;
    let session = login(&f, &mut a).await;
    let before = effects(&pool).await;
    let error = sqlx::query(
        "UPDATE auth_security.browser_sessions SET source_credential_id=$1 WHERE context_id=$2",
    )
    .bind(b.source)
    .bind(Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap())
    .execute(&pool)
    .await
    .expect_err("accepted source cannot be rebound");
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    assert_eq!(effects(&pool).await, before);
    own_history(&f, &session).await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn browser_mapping_is_force_rls_company_scoped_with_narrow_locator_only(pool: PgPool) {
    let f = fixture(&pool, "browser-mapping-rls").await;
    let mut a = actor(&pool, &f, "첫 셀").await;
    let mut b = actor(&pool, &f, "둘째 셀").await;
    let first = login(&f, &mut a).await;
    let second = login(&f, &mut b).await;
    let forced:(bool,bool)=sqlx::query_as("SELECT relrowsecurity,relforcerowsecurity FROM pg_class WHERE oid='auth_security.browser_sessions'::regclass")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(forced, (true, true));
    let foreign_keys:i64=sqlx::query_scalar("SELECT count(*) FROM pg_constraint WHERE conrelid='auth_security.browser_sessions'::regclass AND contype='f'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(
        foreign_keys, 0,
        "retained custody cannot restrict or cascade with deleted roots"
    );
    let direct:bool=sqlx::query_scalar("SELECT has_schema_privilege('console_rt','auth_security','USAGE') OR has_table_privilege('console_rt','auth_security.browser_sessions','SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') OR has_any_column_privilege('console_rt','auth_security.browser_sessions','SELECT,INSERT,UPDATE,REFERENCES')")
        .fetch_one(&pool).await.unwrap();
    assert!(!direct, "preserve private schema and raw DML denials");
    let inherited_role:(bool,bool)=sqlx::query_as("SELECT rolcanlogin,EXISTS(SELECT 1 FROM pg_auth_members WHERE roleid=r.oid OR member=r.oid) FROM pg_roles r WHERE rolname='console_auth_cmd'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(
        inherited_role,
        (false, false),
        "dormant command role must remain dormant"
    );
    for query in [
        "SELECT * FROM auth_security.browser_sessions",
        "UPDATE auth_security.browser_sessions SET owner_removed_at=clock_timestamp()",
        "TRUNCATE auth_security.browser_sessions",
    ] {
        let error = sqlx::query(query)
            .execute(&f.runtime)
            .await
            .expect_err("runtime has no private raw access");
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("42501")
        );
    }
    let hash = Sha256::digest(first["session_token"].as_str().unwrap().as_bytes()).to_vec();
    let context = Uuid::parse_str(first["context_id"].as_str().unwrap()).unwrap();
    let found: Option<Uuid> =
        sqlx::query_scalar("SELECT public.platform_browser_session_company($1,$2)")
            .bind(&hash)
            .bind(context)
            .fetch_one(&f.runtime)
            .await
            .unwrap();
    assert_eq!(found, Some(*a.org.as_uuid()));
    for (candidate, candidate_context) in
        [(vec![37_u8; 32], context), (hash.clone(), Uuid::new_v4())]
    {
        let absent: Option<Uuid> =
            sqlx::query_scalar("SELECT public.platform_browser_session_company($1,$2)")
                .bind(candidate)
                .bind(candidate_context)
                .fetch_one(&f.runtime)
                .await
                .unwrap();
        assert_eq!(absent, None);
    }
    let mut tx = f.runtime.begin().await.unwrap();
    // No scope and wrong explicit Company return no rows, even to a definer.
    let absent: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.platform_browser_session_read($1,$2,$3)")
            .bind(*a.org.as_uuid())
            .bind(&hash)
            .bind(context)
            .fetch_one(tx.as_mut())
            .await
            .unwrap();
    assert_eq!(absent, 0);
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(a.org.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let own: Vec<Uuid> =
        sqlx::query_scalar("SELECT account_id FROM public.platform_browser_session_read($1,$2,$3)")
            .bind(*a.org.as_uuid())
            .bind(&hash)
            .bind(context)
            .fetch_all(tx.as_mut())
            .await
            .unwrap();
    assert_eq!(own, vec![*a.user.as_uuid()]);
    let other_hash = Sha256::digest(second["session_token"].as_str().unwrap().as_bytes()).to_vec();
    let other_context = Uuid::parse_str(second["context_id"].as_str().unwrap()).unwrap();
    for company in [*a.org.as_uuid(), *b.org.as_uuid()] {
        let denied: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM public.platform_browser_session_read($1,$2,$3)",
        )
        .bind(company)
        .bind(&other_hash)
        .bind(other_context)
        .fetch_one(tx.as_mut())
        .await
        .unwrap();
        assert_eq!(denied, 0);
    }
    tx.rollback().await.unwrap();
    own_history(&f, &first).await;
    own_history(&f, &second).await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn browser_storage_function_catalog_preserves_builtin_signatures_and_restricted_grants(
    pool: PgPool,
) {
    let f = fixture(&pool, "browser-function-catalog").await;
    let mut a = actor(&pool, &f, "정확한 함수 계약").await;
    login(&f, &mut a).await;
    for signature in [
        "public.platform_browser_session_company(bytea,uuid)",
        "public.platform_browser_session_read(uuid,bytea,uuid)",
        "public.platform_browser_session_insert(uuid,uuid,uuid,uuid,uuid,bytea,timestamp with time zone,bytea,bytea,bytea)",
        "public.platform_browser_session_lock(uuid,bytea,uuid)",
        "public.platform_browser_session_close(uuid,bytea,uuid)",
        "public.platform_browser_session_cleanup_companies(uuid,integer)",
        "public.platform_browser_session_cleanup(uuid,integer)",
    ] {
        let function:(bool,Vec<String>,String,bool,bool)=sqlx::query_as("SELECT p.prosecdef,p.proconfig,pg_get_userbyid(p.proowner),EXISTS(SELECT 1 FROM aclexplode(COALESCE(p.proacl,acldefault('f',p.proowner))) a WHERE a.grantee=0 AND a.privilege_type='EXECUTE'),has_function_privilege('console_rt',p.oid,'EXECUTE') FROM pg_proc p WHERE p.oid=to_regprocedure($1)")
            .bind(signature).fetch_one(&pool).await.unwrap();
        assert!(function.0 && !function.3 && function.4);
        assert!(function.1.iter().any(|v| v == "search_path=pg_catalog"));
        assert!(function.1.iter().any(|v| v == "row_security=on"));
        let owner: String = sqlx::query_scalar("SELECT current_user::text")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            function.2, owner,
            "SQLx owner exception is test-only; production owner proof remains required"
        );
        let private_type:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_type t JOIN pg_namespace n ON n.oid=t.typnamespace WHERE n.nspname <> 'pg_catalog' AND t.oid=ANY(COALESCE(p.proallargtypes,p.proargtypes::oid[]) || ARRAY[p.prorettype])) FROM pg_proc p WHERE p.oid=to_regprocedure($1)")
            .bind(signature).fetch_one(&pool).await.unwrap();
        assert!(
            !private_type,
            "public interface may not expose a private row/composite type"
        );
        for role in [
            "console_leave_cmd",
            "console_ontology_cmd",
            "console_platform_force_cmd",
            "console_auth_cmd",
        ] {
            let allowed: bool = sqlx::query_scalar(
                "SELECT has_function_privilege($1,to_regprocedure($2),'EXECUTE')",
            )
            .bind(role)
            .bind(signature)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(!allowed, "unrelated role must not execute {signature}");
        }
    }
    for name in ["read", "lock"] {
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT ARRAY(SELECT arg.name FROM unnest(p.proargnames,p.proargmodes) WITH ORDINALITY AS arg(name,mode,n) WHERE arg.mode IN ('o','b','t') ORDER BY arg.n) FROM pg_proc p WHERE p.oid=to_regprocedure($1)",
        ).bind(format!("public.platform_browser_session_{name}(uuid,bytea,uuid)"))
            .fetch_one(&pool).await.unwrap();
        assert_eq!(
            columns,
            [
                "account_id",
                "company_id",
                "family_id",
                "source_credential_id",
                "context_id",
                "expires_at",
                "codec_version",
                "ciphertext",
                "nonce",
                "tag",
                "closed_at",
                "owner_removed_at"
            ]
            .map(String::from)
        );
    }
    let locator_shape:(String,String)=sqlx::query_as("SELECT pg_get_function_identity_arguments(oid),pg_get_function_result(oid) FROM pg_proc WHERE oid='public.platform_browser_session_company(bytea,uuid)'::regprocedure")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(locator_shape.1, "uuid");
    assert!(locator_shape.0.contains("bytea") && locator_shape.0.contains("uuid"));
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn scoped_storage_functions_cannot_forge_closure_or_clear_active_proof(pool: PgPool) {
    let f = fixture(&pool, "browser-function-effects").await;
    let mut a = actor(&pool, &f, "종료 증거 보존").await;
    let mut b = actor(&pool, &f, "다른 종료 범위").await;
    let first = login(&f, &mut a).await;
    let other = login(&f, &mut b).await;
    let hash = Sha256::digest(first["session_token"].as_str().unwrap().as_bytes()).to_vec();
    let context = Uuid::parse_str(first["context_id"].as_str().unwrap()).unwrap();
    let before = effects(&pool).await;
    let mut tx = f.runtime.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(a.org.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    for company in [*a.org.as_uuid(), *b.org.as_uuid()] {
        let closed: bool =
            sqlx::query_scalar("SELECT public.platform_browser_session_close($1,$2,$3)")
                .bind(company)
                .bind(&hash)
                .bind(context)
                .fetch_one(tx.as_mut())
                .await
                .unwrap();
        assert!(
            !closed,
            "even correct scope cannot close an unrevoked family"
        );
        let cleared: i64 =
            sqlx::query_scalar("SELECT public.platform_browser_session_cleanup($1,$2)")
                .bind(company)
                .bind(2_i32)
                .fetch_one(tx.as_mut())
                .await
                .unwrap();
        assert_eq!(
            cleared, 0,
            "wrong scope and active proof must remain intact"
        );
    }
    tx.commit().await.unwrap();
    assert_eq!(effects(&pool).await, before);
    own_history(&f, &first).await;
    own_history(&f, &other).await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn insert_scope_is_checked_while_exact_verified_mapping_is_still_eligible(pool: PgPool) {
    let f = fixture(&pool, "browser-insert-scope").await;
    let mut a = actor(&pool, &f, "삽입 경계 주체").await;
    let other = actor(&pool, &f, "삽입 경계 다른 회사").await;
    // Probe inside genuine issuance, before the first row exists. A duplicate
    // key or an already-consumed conversion cannot accidentally mask a scope bug.
    let other_company = other.org.to_string();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(r#"
        CREATE FUNCTION public.test_browser_insert_scope() RETURNS trigger
        LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
        DECLARE saved text; candidate text;
        BEGIN
            IF pg_trigger_depth()>1 THEN RETURN NEW; END IF;
            saved := current_setting('app.current_org',true);
            FOREACH candidate IN ARRAY ARRAY['','{other_company}'] LOOP
                PERFORM set_config('app.current_org',candidate,true);
                BEGIN
                    PERFORM public.platform_browser_session_insert(NEW.company_id,NEW.account_id,NEW.family_id,NEW.source_credential_id,NEW.context_id,NEW.token_hash,NEW.expires_at,NEW.ciphertext,NEW.nonce,NEW.tag);
                EXCEPTION WHEN insufficient_privilege THEN NULL;
                END;
                IF EXISTS(SELECT 1 FROM auth_security.browser_sessions WHERE context_id=NEW.context_id) THEN
                    RAISE EXCEPTION 'test_browser_insert_scope_bypass';
                END IF;
            END LOOP;
            PERFORM set_config('app.current_org',saved,true);
            BEGIN
                PERFORM public.platform_browser_session_insert('{other_company}'::pg_catalog.uuid,NEW.account_id,NEW.family_id,NEW.source_credential_id,NEW.context_id,NEW.token_hash,NEW.expires_at,NEW.ciphertext,NEW.nonce,NEW.tag);
            EXCEPTION WHEN insufficient_privilege THEN NULL;
            END;
            IF EXISTS(SELECT 1 FROM auth_security.browser_sessions WHERE context_id=NEW.context_id) THEN
                RAISE EXCEPTION 'test_browser_insert_scope_bypass';
            END IF;
            RETURN NEW;
        END $$;
        REVOKE ALL ON FUNCTION public.test_browser_insert_scope() FROM PUBLIC;
        CREATE TRIGGER test_browser_insert_scope BEFORE INSERT ON auth_security.browser_sessions FOR EACH ROW EXECUTE FUNCTION public.test_browser_insert_scope();
    "#))).execute(&pool).await.unwrap();
    let session = login(&f, &mut a).await;
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_security.browser_sessions WHERE context_id=$1",
    )
    .bind(Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 1);
    own_history(&f, &session).await;
    sqlx::raw_sql("DROP TRIGGER test_browser_insert_scope ON auth_security.browser_sessions; DROP FUNCTION public.test_browser_insert_scope();")
        .execute(&pool).await.unwrap();
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn scoped_close_checks_eligible_rows_and_every_token_before_monotonic_closure(pool: PgPool) {
    let f = fixture(&pool, "browser-close-eligible").await;
    let mut a = actor(&pool, &f, "검증 가능한 종료").await;
    let b = actor(&pool, &f, "무관한 종료 범위").await;
    let session = login(&f, &mut a).await;
    let bound_family = family(&pool, &session).await;
    let hash = Sha256::digest(session["session_token"].as_str().unwrap().as_bytes()).to_vec();
    let context = Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap();

    // Privileged partial-revocation fault; this is not a legitimate logout receipt.
    sqlx::query("UPDATE auth_refresh_token_families SET revoked_at=clock_timestamp(),revoked_reason='test_partial_revocation' WHERE id=$1")
        .bind(bound_family).execute(&pool).await.unwrap();
    let before = effects(&pool).await;
    let mut tx = f.runtime.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(a.org.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let partial: bool =
        sqlx::query_scalar("SELECT public.platform_browser_session_close($1,$2,$3)")
            .bind(*a.org.as_uuid())
            .bind(&hash)
            .bind(context)
            .fetch_one(tx.as_mut())
            .await
            .unwrap();
    assert!(
        !partial,
        "revoking the family alone cannot certify closure while a token remains live"
    );
    tx.commit().await.unwrap();
    assert_eq!(effects(&pool).await, before);
    sqlx::query("UPDATE auth_refresh_tokens SET revoked_at=clock_timestamp() WHERE family_id=$1")
        .bind(bound_family)
        .execute(&pool)
        .await
        .unwrap();
    let before = effects(&pool).await;
    for (armed, supplied, candidate_hash, candidate_context) in [
        (None, a.org, hash.clone(), context),
        (Some(b.org), a.org, hash.clone(), context),
        (Some(a.org), b.org, hash.clone(), context),
        (Some(a.org), a.org, vec![17; 32], context),
        (Some(a.org), a.org, hash.clone(), Uuid::new_v4()),
    ] {
        let mut tx = f.runtime.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org',$1,true)")
            .bind(armed.map(|org| org.to_string()).unwrap_or_default())
            .execute(tx.as_mut())
            .await
            .unwrap();
        let closed: bool =
            sqlx::query_scalar("SELECT public.platform_browser_session_close($1,$2,$3)")
                .bind(*supplied.as_uuid())
                .bind(candidate_hash)
                .bind(candidate_context)
                .fetch_one(tx.as_mut())
                .await
                .unwrap();
        assert!(
            !closed,
            "an eligible row must still require exact scope/hash/context"
        );
        tx.commit().await.unwrap();
        assert_eq!(effects(&pool).await, before);
    }
    let mut tx = f.runtime.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(a.org.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let closed: bool = sqlx::query_scalar("SELECT public.platform_browser_session_close($1,$2,$3)")
        .bind(*a.org.as_uuid())
        .bind(&hash)
        .bind(context)
        .fetch_one(tx.as_mut())
        .await
        .unwrap();
    assert!(
        !closed,
        "previously committed manual revocation cannot certify native owner closure"
    );
    tx.commit().await.unwrap();
    assert_eq!(effects(&pool).await, before);
    // The positive runs through a genuine native logout of a fresh live family;
    // storage closure is exercised within that owner's transaction.
    let honest = login(&f, &mut a).await;
    let immutable = immutable_mapping(&pool, &honest).await;
    let honest_family = family(&pool, &honest).await;
    let response = post_raw(f.router.clone(), LOGOUT, None, handle(&honest)).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        to_bytes(response.into_body(), 4096)
            .await
            .unwrap()
            .is_empty()
    );
    assert_closed_mapping(&pool, &honest, immutable).await;
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action='auth.logout' AND target_id=$1",
    )
    .bind(honest_family.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        audit_count, 1,
        "positive closure requires the native owner audit"
    );
    let before = effects(&pool).await;
    assert_eq!(
        post_raw(f.router.clone(), LOGOUT, None, handle(&honest))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        effects(&pool).await,
        before,
        "terminal retry does not duplicate the native owner logout audit"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn expired_proof_cleanup_enforces_scope_bound_and_preserves_reconciliation(pool: PgPool) {
    let mut f = fixture(&pool, "browser-cleanup-eligible").await;
    let mut a = actor(&pool, &f, "만료 정리 범위").await;
    let mut b = actor(&pool, &f, "다른 만료 범위").await;
    f.router = configured_router_with(
        f.runtime.clone(),
        &f.private,
        &f.public,
        Some(&f.storage_key),
        &[("CONSOLE_REFRESH_FAMILY_ABSOLUTE_TTL_SECS", "6".into())],
    );
    let first = login(&f, &mut a).await;
    let second = login(&f, &mut a).await;
    let other = login(&f, &mut b).await;
    let immutable_first = immutable_mapping(&pool, &first).await;
    let immutable_second = immutable_mapping(&pool, &second).await;
    let deadline = [&first, &second, &other]
        .into_iter()
        .map(|session| {
            OffsetDateTime::parse(
                session["expires_at"].as_str().unwrap(),
                &time::format_description::well_known::Rfc3339,
            )
            .unwrap()
        })
        .max()
        .unwrap();
    sqlx::query(
        "SELECT pg_sleep(greatest(0,extract(epoch FROM ($1::timestamptz-clock_timestamp())))+0.1)",
    )
    .bind(deadline)
    .execute(&pool)
    .await
    .unwrap();
    f.router = configured_router(
        f.runtime.clone(),
        &f.private,
        &f.public,
        Some(&f.storage_key),
    );
    let live = login(&f, &mut a).await;
    let before = effects(&pool).await;
    for (armed, supplied) in [(None, a.org), (Some(b.org), a.org), (Some(a.org), b.org)] {
        let mut tx = f.runtime.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org',$1,true)")
            .bind(armed.map(|org| org.to_string()).unwrap_or_default())
            .execute(tx.as_mut())
            .await
            .unwrap();
        let cleared: i64 =
            sqlx::query_scalar("SELECT public.platform_browser_session_cleanup($1,$2)")
                .bind(*supplied.as_uuid())
                .bind(1_i32)
                .fetch_one(tx.as_mut())
                .await
                .unwrap();
        assert_eq!(
            cleared, 0,
            "eligible expired proof must not be cleared outside scope"
        );
        tx.commit().await.unwrap();
        assert_eq!(effects(&pool).await, before);
    }
    for expected in [1_i64, 1, 0] {
        let mut tx = f.runtime.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org',$1,true)")
            .bind(a.org.to_string())
            .execute(tx.as_mut())
            .await
            .unwrap();
        let cleared: i64 =
            sqlx::query_scalar("SELECT public.platform_browser_session_cleanup($1,$2)")
                .bind(*a.org.as_uuid())
                .bind(1_i32)
                .fetch_one(tx.as_mut())
                .await
                .unwrap();
        assert_eq!(
            cleared, expected,
            "bound one-row batches must make actual progress"
        );
        tx.commit().await.unwrap();
    }
    for (session, immutable) in [(&first, immutable_first), (&second, immutable_second)] {
        let intact: bool = sqlx::query_scalar("SELECT closed_at IS NULL AND owner_removed_at IS NULL AND ciphertext IS NULL AND nonce IS NULL AND tag IS NULL FROM auth_security.browser_sessions WHERE context_id=$1")
            .bind(Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap()).fetch_one(&pool).await.unwrap();
        assert!(
            intact,
            "expiry cleanup never certifies logout or owner removal"
        );
        assert_eq!(immutable_mapping(&pool, session).await, immutable);
        let family_live: bool = sqlx::query_scalar(
            "SELECT revoked_at IS NULL FROM auth_refresh_token_families WHERE id=$1",
        )
        .bind(family(&pool, session).await)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(family_live, "cleanup cannot revoke a native family");
        assert_eq!(
            post_raw(f.router.clone(), LOGOUT, None, handle(session))
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
        assert_closed_mapping(&pool, session, immutable).await;
    }
    let other_intact: bool = sqlx::query_scalar("SELECT ciphertext IS NOT NULL AND closed_at IS NULL FROM auth_security.browser_sessions WHERE context_id=$1")
        .bind(Uuid::parse_str(other["context_id"].as_str().unwrap()).unwrap()).fetch_one(&pool).await.unwrap();
    assert!(other_intact);
    own_history(&f, &live).await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn scoped_read_and_lock_reject_hostile_temporary_objects_and_exact_pair_substitutions(
    pool: PgPool,
) {
    let f = fixture(&pool, "browser-hostile-temp").await;
    let mut a = actor(&pool, &f, "실제 저장 소유자").await;
    let mut b = actor(&pool, &f, "임시 저장 위조자").await;
    let first = login(&f, &mut a).await;
    let other = login(&f, &mut b).await;
    let hash = Sha256::digest(first["session_token"].as_str().unwrap().as_bytes()).to_vec();
    let other_hash = Sha256::digest(other["session_token"].as_str().unwrap().as_bytes()).to_vec();
    let context = Uuid::parse_str(first["context_id"].as_str().unwrap()).unwrap();
    let other_context = Uuid::parse_str(other["context_id"].as_str().unwrap()).unwrap();
    let before = effects(&pool).await;
    let mut tx = f.runtime.begin().await.unwrap();
    sqlx::raw_sql("CREATE TEMP TABLE browser_sessions(token_hash pg_catalog.bytea,account_id pg_catalog.uuid,company_id pg_catalog.uuid,family_id pg_catalog.uuid,source_credential_id pg_catalog.uuid,context_id pg_catalog.uuid,expires_at pg_catalog.timestamptz,ciphertext pg_catalog.bytea,nonce pg_catalog.bytea,tag pg_catalog.bytea,closed_at pg_catalog.timestamptz,owner_removed_at pg_catalog.timestamptz); CREATE TEMP TABLE users(id pg_catalog.uuid,is_active bool,org_id pg_catalog.uuid); CREATE TEMP TABLE auth_refresh_token_families(id pg_catalog.uuid,org_id pg_catalog.uuid,user_id pg_catalog.uuid,revoked_at pg_catalog.timestamptz); CREATE DOMAIN pg_temp.uuid AS text; SET LOCAL search_path=pg_temp,public,pg_catalog;")
        .execute(tx.as_mut()).await.unwrap();
    sqlx::query("INSERT INTO pg_temp.browser_sessions(token_hash,account_id,company_id,context_id) VALUES($1,$2,$3,$4)")
        .bind(&hash).bind(*b.user.as_uuid()).bind(*a.org.as_uuid()).bind(context).execute(tx.as_mut()).await.unwrap();
    for name in ["read", "lock"] {
        for (armed, supplied, candidate_hash, candidate_context) in [
            (None, a.org, hash.clone(), context),
            (Some(b.org), a.org, hash.clone(), context),
            (Some(a.org), b.org, other_hash.clone(), other_context),
            (Some(a.org), a.org, other_hash.clone(), other_context),
            (Some(a.org), a.org, hash.clone(), Uuid::new_v4()),
            (Some(a.org), a.org, vec![29; 32], context),
        ] {
            sqlx::query("SELECT pg_catalog.set_config('app.current_org',$1,true)")
                .bind(armed.map(|org| org.to_string()).unwrap_or_default())
                .execute(tx.as_mut())
                .await
                .unwrap();
            let rows: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT count(*) FROM public.platform_browser_session_{name}($1,$2,$3)"
            )))
            .bind(*supplied.as_uuid())
            .bind(candidate_hash)
            .bind(candidate_context)
            .fetch_one(tx.as_mut())
            .await
            .unwrap();
            assert_eq!(rows, 0);
        }
        sqlx::query("SELECT pg_catalog.set_config('app.current_org',$1,true)")
            .bind(a.org.to_string())
            .execute(tx.as_mut())
            .await
            .unwrap();
        let subject: Uuid = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT account_id FROM public.platform_browser_session_{name}($1,$2,$3)"
        )))
        .bind(*a.org.as_uuid())
        .bind(&hash)
        .bind(context)
        .fetch_one(tx.as_mut())
        .await
        .unwrap();
        assert_eq!(
            subject,
            *a.user.as_uuid(),
            "fixed owner must ignore hostile temporary rows and type names"
        );
    }
    tx.rollback().await.unwrap();
    assert_eq!(effects(&pool).await, before);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn same_source_uuid_reinsert_does_not_renew_a_recorded_removed_browser_credential(
    pool: PgPool,
) {
    let f = fixture(&pool, "browser-source-reinsert").await;
    let mut a = actor(&pool, &f, "과거 인증키 제거").await;
    let session = login(&f, &mut a).await;
    let bearer = private_original_proof(&pool, &f, &session).await;
    add_passkey(&f, &mut a, &bearer).await;
    let original: Value =
        sqlx::query_scalar("SELECT to_jsonb(k) FROM auth_webauthn_credentials k WHERE id=$1")
            .bind(a.source)
            .fetch_one(&pool)
            .await
            .unwrap();
    let response = f
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/auth/passkeys/{}", a.source))
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    // Privileged stale-restore fault, not authorized production resurrection.
    // Reuse the original row/crypto bytes without clearing the removal ledger.
    sqlx::query("INSERT INTO public.auth_webauthn_credentials SELECT (jsonb_populate_record(NULL::public.auth_webauthn_credentials,$1)).*")
        .bind(original).execute(&pool).await.unwrap();
    let before = effects(&pool).await;
    assert_eq!(
        post_raw(f.router.clone(), HISTORY, None, history_handle(&session))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(effects(&pool).await, before);
    assert_eq!(
        post_raw(f.router.clone(), LOGOUT, None, handle(&session))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let history: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_security.credential_removals WHERE credential_row_id=$1",
    )
    .bind(a.source)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        history, 1,
        "restored credential may not forget known removal"
    );
}

async fn wait_blocked(owner: &PgPool, label: &str, blocker: i32) {
    timeout(WAIT, async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND application_name=$1 AND wait_event_type='Lock' AND $2=ANY(pg_blocking_pids(pid)))")
                .bind(label).bind(blocker).fetch_one(owner).await.unwrap();
            if blocked { break; }
            sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("a real database lock wait must be observed");
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn original_short_family_deadline_expires_during_lookup_wait_without_sliding(pool: PgPool) {
    let mut f = fixture(&pool, "browser-expiry-wait").await;
    let mut a = actor(&pool, &f, "대기 중 만료").await;
    f.router = configured_router_with(
        f.runtime.clone(),
        &f.private,
        &f.public,
        Some(&f.storage_key),
        &[("CONSOLE_REFRESH_FAMILY_ABSOLUTE_TTL_SECS", "6".into())],
    );
    for table in [
        "auth_security.browser_sessions",
        "auth_refresh_token_families",
    ] {
        let session = login(&f, &mut a).await;
        let original = private_original_proof(&pool, &f, &session).await;
        own_history(&f, &session).await;
        let claims = f.verifier.verify_access_token(&original).unwrap();
        let deadline = OffsetDateTime::parse(
            session["expires_at"].as_str().unwrap(),
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let bounded: bool = sqlx::query_scalar(
        "SELECT $2 <= created_at+interval '6 seconds' FROM auth_refresh_token_families WHERE id=$1",
    )
    .bind(family(&pool, &session).await)
    .bind(deadline)
    .fetch_one(&pool)
    .await
    .unwrap();
        assert!(bounded);
        assert_eq!(
            deadline,
            OffsetDateTime::from_unix_timestamp(claims.exp).unwrap()
        );
        let stored: OffsetDateTime = sqlx::query_scalar(
            "SELECT expires_at FROM auth_security.browser_sessions WHERE context_id=$1",
        )
        .bind(Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stored, deadline);
        let before = effects(&pool).await;
        let mut gate = pool.begin().await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "LOCK TABLE {table} IN ACCESS EXCLUSIVE MODE"
        )))
        .execute(gate.as_mut())
        .await
        .unwrap();
        let pid = super::reset_sessions::transaction_pid(&mut gate).await;
        let router = f.router.clone();
        let request = history_handle(&session);
        let pending = tokio::spawn(async move { post_raw(router, HISTORY, None, request).await });
        wait_blocked(&pool, "browser-expiry-wait", pid).await;
        // Wait on the real original deadline; no JWT/mapping/clock forgery.
        sqlx::query(
        "SELECT pg_sleep(greatest(0,extract(epoch FROM ($1::timestamptz-clock_timestamp())))+0.1)",
    )
    .bind(deadline)
    .execute(gate.as_mut())
    .await
    .unwrap();
        gate.commit().await.unwrap();
        assert_eq!(
            timeout(WAIT, pending).await.unwrap().unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(effects(&pool).await, before);
        assert_eq!(
            get(&f, "/api/v1/hr/attendance-records/me", &original)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        // Expired access does not destroy exact-identity logout reconciliation.
        assert_eq!(
            post_raw(f.router.clone(), LOGOUT, None, handle(&session))
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn recovery_revokes_browser_family_without_revoking_another_person(pool: PgPool) {
    let f = fixture(&pool, "browser-recovery").await;
    let mut subject = actor(&pool, &f, "복구 대상").await;
    give_recovery_subject_branch(&pool, &subject).await;
    let mut admin = actor_in_org(&pool, &f, "복구 담당자", subject.org).await;
    sqlx::query("UPDATE users SET roles=ARRAY['SUPER_ADMIN'] WHERE id=$1")
        .bind(*admin.user.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let first = login(&f, &mut subject).await;
    let second = login(&f, &mut subject).await;
    let administrator = login(&f, &mut admin).await;
    let admin_bearer = private_original_proof(&pool, &f, &administrator).await;
    let response = post_raw(
        f.router.clone(),
        "/api/v1/auth/admin/credential-reset",
        Some(&admin_bearer),
        json!({"user_id":subject.user.as_uuid()}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    for session in [&first, &second] {
        assert_eq!(
            post_raw(f.router.clone(), HISTORY, None, history_handle(session))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            post_raw(f.router.clone(), LOGOUT, None, handle(session))
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    own_history(&f, &administrator).await;
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_refresh_token_families WHERE user_id=$1 AND revoked_at IS NULL",
    )
    .bind(*subject.user.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live, 0);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn browser_family_and_login_audit_failures_roll_back_the_exact_signed_assertion(
    pool: PgPool,
) {
    let f = fixture(&pool, "browser-owner-atomicity").await;
    let mut a = actor(&pool, &f, "실패 후 재검증").await;
    login(&f, &mut a).await;
    for (table, condition) in [
        ("auth_refresh_token_families", "true"),
        ("audit_events", "NEW.action='auth.refresh.issue'"),
        ("audit_events", "NEW.action='auth.login'"),
    ] {
        let (ceremony, signed) = assertion(&f, &mut a).await;
        let before = effects(&pool).await;
        let key_before: Value =
            sqlx::query_scalar("SELECT passkey_json FROM auth_webauthn_credentials WHERE id=$1")
                .bind(a.source)
                .fetch_one(&pool)
                .await
                .unwrap();
        // Fixed table/condition allowlist; fault is private to this disposable DB.
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE FUNCTION public.test_browser_owner_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'forced browser owner failure'; END $$; CREATE TRIGGER test_browser_owner_failure BEFORE INSERT ON {table} FOR EACH ROW WHEN ({condition}) EXECUTE FUNCTION public.test_browser_owner_failure();")))
            .execute(&pool).await.unwrap();
        assert_eq!(
            post_raw(f.router.clone(), LOGIN, None, signed.clone())
                .await
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(effects(&pool).await, before);
        let state:(bool,Value)=sqlx::query_as("SELECT c.consumed_at IS NULL,k.passkey_json FROM auth_webauthn_ceremonies c CROSS JOIN auth_webauthn_credentials k WHERE c.id=$1 AND k.id=$2")
            .bind(ceremony).bind(a.source).fetch_one(&pool).await.unwrap();
        assert!(state.0 && state.1 == key_before);
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP TRIGGER test_browser_owner_failure ON {table}; DROP FUNCTION public.test_browser_owner_failure();")))
            .execute(&pool).await.unwrap();
        assert_eq!(
            post_raw(f.router.clone(), LOGIN, None, signed)
                .await
                .status(),
            StatusCode::OK
        );
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn failed_logout_audit_preserves_live_family_and_exact_retry_can_close_it(pool: PgPool) {
    let f = fixture(&pool, "browser-logout-rollback").await;
    let mut a = actor(&pool, &f, "종료 재시도").await;
    let session = login(&f, &mut a).await;
    let before = effects(&pool).await;
    sqlx::raw_sql("CREATE FUNCTION public.test_browser_logout_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.action='auth.logout' THEN RAISE EXCEPTION 'forced browser logout failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER test_browser_logout_failure BEFORE INSERT ON public.audit_events FOR EACH ROW EXECUTE FUNCTION public.test_browser_logout_failure();")
        .execute(&pool).await.unwrap();
    assert_eq!(
        post_raw(f.router.clone(), LOGOUT, None, handle(&session))
            .await
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(effects(&pool).await, before);
    own_history(&f, &session).await;
    sqlx::query("DROP TRIGGER test_browser_logout_failure ON public.audit_events")
        .execute(&pool)
        .await
        .unwrap();
    // Discard the committed response to model response loss, then reconcile.
    let response = post_raw(f.router.clone(), LOGOUT, None, handle(&session)).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    drop(response);
    let closed = effects(&pool).await;
    assert_eq!(
        post_raw(f.router.clone(), LOGOUT, None, handle(&session))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(effects(&pool).await, closed);
}

async fn removal_actor(owner: &PgPool) -> UserId {
    let id = UserId::new();
    sqlx::query("INSERT INTO users(id,display_name,roles,org_id) VALUES($1,'회사 제거 감사 주체',ARRAY['ADMIN'],$2)")
        .bind(*id.as_uuid()).bind(*OrgId::platform().as_uuid()).execute(owner).await.unwrap();
    id
}

async fn remove_company(
    pool: &PgPool,
    force: bool,
    org: OrgId,
    actor: UserId,
) -> Result<
    console_platform_provisioning::TenantRemovalOutcome,
    console_platform_provisioning::ProvisioningError,
> {
    let service = console_platform_provisioning::PlatformProvisioner::new(Duration::minutes(5));
    if force {
        service
            .force_remove_tenant(pool, Some(actor), *org.as_uuid(), OffsetDateTime::now_utc())
            .await
    } else {
        service
            .remove_tenant(pool, Some(actor), *org.as_uuid(), OffsetDateTime::now_utc())
            .await
    }
}

async fn assert_removed_session(owner: &PgPool, f: &Fixture, a: &Actor, session: &Value) {
    let cleared:bool=sqlx::query_scalar("SELECT ciphertext IS NULL AND nonce IS NULL AND tag IS NULL AND closed_at IS NOT NULL AND owner_removed_at IS NOT NULL FROM auth_security.browser_sessions WHERE context_id=$1 AND account_id=$2 AND company_id=$3")
        .bind(Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap())
        .bind(*a.user.as_uuid()).bind(*a.org.as_uuid()).fetch_one(owner).await.unwrap();
    assert!(
        cleared,
        "owner removal must clear live proof and retain terminal reconciliation"
    );
    assert_eq!(
        post_raw(f.router.clone(), HISTORY, None, history_handle(session))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let before = effects(owner).await;
    for _ in 0..2 {
        assert_eq!(
            post_raw(f.router.clone(), LOGOUT, None, handle(session))
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(
        effects(owner).await,
        before,
        "terminal removal cannot invent a logout audit or resurrect parents"
    );
    let retained:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM users WHERE id=$1),(SELECT count(*) FROM auth_refresh_token_families WHERE user_id=$1)")
        .bind(*a.user.as_uuid()).fetch_one(owner).await.unwrap();
    assert_eq!(retained, (0, 0));
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn real_session_survives_as_token_free_terminal_evidence_after_direct_and_company_removal(
    pool: PgPool,
) {
    use console_platform_provisioning::TenantRemovalOutcome;
    let f = fixture(&pool, "browser-owner-removal").await;
    let audit_actor = removal_actor(&pool).await;
    for mode in 0..3 {
        let mut a = actor(&pool, &f, &format!("제거 경로 {mode}")).await;
        let session = login(&f, &mut a).await;
        if mode == 0 {
            // A genuine login has immutable audit references: raw deletion must
            // first fail rather than pretending0241 may weaken that FK.
            let before = effects(&pool).await;
            let mut denied = f.runtime.begin().await.unwrap();
            sqlx::query("SELECT set_config('app.current_org',$1,true)")
                .bind(a.org.to_string())
                .execute(denied.as_mut())
                .await
                .unwrap();
            let error = sqlx::query("DELETE FROM users WHERE id=$1")
                .bind(*a.user.as_uuid())
                .execute(denied.as_mut())
                .await
                .expect_err("unclosed audit reference must restrict direct Account deletion");
            assert_eq!(
                error.as_database_error().unwrap().code().as_deref(),
                Some("23001")
            );
            denied.rollback().await.unwrap();
            assert_eq!(effects(&pool).await, before);
            own_history(&f, &session).await;
            // Owner-only fixture prerequisite, using the existing sanctioned
            // reference-rehome arm; no trigger/FK is disabled and no content is
            // rewritten. This is not an exposed Account-removal action.
            let content_sql = "SELECT COALESCE(jsonb_agg(to_jsonb(a)-'org_id'-'actor'-'branch_id' ORDER BY id),'[]'::jsonb) FROM audit_events a";
            let content: Value = sqlx::query_scalar(content_sql)
                .fetch_one(&pool)
                .await
                .unwrap();
            let content_digest = Sha256::digest(serde_json::to_vec(&content).unwrap());
            let mut closure = pool.begin().await.unwrap();
            sqlx::query("SELECT set_config('app.audit_rehome','on',true)")
                .execute(closure.as_mut())
                .await
                .unwrap();
            sqlx::query(
                "UPDATE audit_events SET org_id=$1,actor=NULL,branch_id=NULL WHERE actor=$2",
            )
            .bind(*OrgId::platform().as_uuid())
            .bind(*a.user.as_uuid())
            .execute(closure.as_mut())
            .await
            .unwrap();
            closure.commit().await.unwrap();
            let preserved: Value = sqlx::query_scalar(content_sql)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(
                Sha256::digest(serde_json::to_vec(&preserved).unwrap()),
                content_digest,
                "reference closure must preserve exact immutable audit content"
            );
            let mut tx = f.runtime.begin().await.unwrap();
            sqlx::query("SELECT set_config('app.current_org',$1,true)")
                .bind(a.org.to_string())
                .execute(tx.as_mut())
                .await
                .unwrap();
            sqlx::query("DELETE FROM users WHERE id=$1")
                .bind(*a.user.as_uuid())
                .execute(tx.as_mut())
                .await
                .unwrap();
            tx.commit().await.unwrap();
        } else {
            if mode == 2 {
                sqlx::query("UPDATE organizations SET status='ARCHIVED' WHERE id=$1")
                    .bind(*a.org.as_uuid())
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            let rt = runtime_login(&pool, mode == 2, "browser-removal-owner").await;
            assert_eq!(
                remove_company(&rt, mode == 2, a.org, audit_actor)
                    .await
                    .unwrap(),
                TenantRemovalOutcome::Removed
            );
            let remaining: i64 =
                sqlx::query_scalar("SELECT count(*) FROM organizations WHERE id=$1")
                    .bind(*a.org.as_uuid())
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(remaining, 0);
        }
        assert_removed_session(&pool, &f, &a, &session).await;
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn denied_removal_and_late_company_audit_failure_preserve_browser_proof(pool: PgPool) {
    use console_platform_provisioning::TenantRemovalOutcome;
    let f = fixture(&pool, "browser-removal-rollback").await;
    let audit_actor = removal_actor(&pool).await;
    let force = runtime_login(&pool, true, "browser-force-removal-rollback").await;
    let mut a = actor(&pool, &f, "활성 회사 보호").await;
    let session = login(&f, &mut a).await;
    let before = effects(&pool).await;
    assert_eq!(
        remove_company(&force, true, a.org, audit_actor)
            .await
            .unwrap(),
        TenantRemovalOutcome::BlockedActive
    );
    assert_eq!(effects(&pool).await, before);
    own_history(&f, &session).await;
    sqlx::raw_sql("CREATE TABLE public.test_browser_restricted_account(account_id UUID REFERENCES users(id) ON DELETE RESTRICT)").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO test_browser_restricted_account VALUES($1)")
        .bind(*a.user.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(*a.user.as_uuid())
            .execute(&pool)
            .await
            .is_err()
    );
    assert_eq!(effects(&pool).await, before);
    own_history(&f, &session).await;
    sqlx::query("DROP TABLE test_browser_restricted_account")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE FUNCTION public.test_browser_removal_audit_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.action IN ('platform.tenant.remove','platform.tenant.force_remove') THEN RAISE EXCEPTION 'browser_removal_audit_failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER test_browser_removal_audit_failure BEFORE INSERT ON audit_events FOR EACH ROW EXECUTE FUNCTION public.test_browser_removal_audit_failure();")
        .execute(&pool).await.unwrap();
    for force_mode in [false, true] {
        if force_mode {
            sqlx::query("UPDATE organizations SET status='ARCHIVED' WHERE id=$1")
                .bind(*a.org.as_uuid())
                .execute(&pool)
                .await
                .unwrap();
        }
        let before = effects(&pool).await;
        let rt = if force_mode { &force } else { &f.runtime };
        let error = remove_company(rt, force_mode, a.org, audit_actor)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("browser_removal_audit_failure"));
        assert_eq!(effects(&pool).await, before);
        let intact:bool=sqlx::query_scalar("SELECT owner_removed_at IS NULL AND ciphertext IS NOT NULL FROM auth_security.browser_sessions WHERE account_id=$1")
            .bind(*a.user.as_uuid()).fetch_one(&pool).await.unwrap();
        assert!(intact);
    }
    sqlx::query("DROP TRIGGER test_browser_removal_audit_failure ON public.audit_events")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE organizations SET status='ACTIVE' WHERE id=$1")
        .bind(*a.org.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    own_history(&f, &session).await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn deferred_mapping_commit_failure_does_not_consume_or_publish_the_assertion(pool: PgPool) {
    let f = fixture(&pool, "browser-deferred-commit").await;
    let mut a = actor(&pool, &f, "커밋 실패").await;
    login(&f, &mut a).await;
    let (_, signed) = assertion(&f, &mut a).await;
    let before = effects(&pool).await;
    sqlx::raw_sql("CREATE FUNCTION public.test_browser_commit_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'forced deferred browser commit failure'; END $$; CREATE CONSTRAINT TRIGGER test_browser_commit_failure AFTER INSERT ON auth_security.browser_sessions DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION public.test_browser_commit_failure();")
        .execute(&pool).await.unwrap();
    let response = post_raw(f.router.clone(), LOGIN, None, signed.clone()).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(set_cookie_values(&response).is_empty());
    assert_eq!(effects(&pool).await, before);
    sqlx::query("DROP TRIGGER test_browser_commit_failure ON auth_security.browser_sessions")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        post_raw(f.router.clone(), LOGIN, None, signed)
            .await
            .status(),
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn logout_waits_for_account_without_holding_browser_row_and_preserves_later_family(
    pool: PgPool,
) {
    let f = fixture(&pool, "browser-logout-account-wait").await;
    let mut a = actor(&pool, &f, "종료 잠금 순서").await;
    let first = login(&f, &mut a).await;
    let later = login(&f, &mut a).await;
    let mut root = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE")
        .bind(*a.user.as_uuid())
        .fetch_one(root.as_mut())
        .await
        .unwrap();
    let pid = super::reset_sessions::transaction_pid(&mut root).await;
    let router = f.router.clone();
    let request = handle(&first);
    let pending = tokio::spawn(async move { post_raw(router, LOGOUT, None, request).await });
    super::reset_sessions::waiting(
        &pool,
        "browser-logout-account-wait",
        pid,
        "SELECT is_active FROM users WHERE id = $1 AND org_id = $2 FOR NO KEY UPDATE",
    )
    .await;
    let mut browser = pool.begin().await.unwrap();
    sqlx::query("SELECT context_id FROM auth_security.browser_sessions WHERE context_id=$1 FOR UPDATE NOWAIT")
        .bind(Uuid::parse_str(first["context_id"].as_str().unwrap()).unwrap()).fetch_one(browser.as_mut()).await.unwrap();
    browser.rollback().await.unwrap();
    root.rollback().await.unwrap();
    assert_eq!(
        timeout(WAIT, pending).await.unwrap().unwrap().status(),
        StatusCode::NO_CONTENT
    );
    own_history(&f, &later).await;
    assert_eq!(
        post_raw(f.router.clone(), HISTORY, None, history_handle(&first))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn both_removal_owners_leave_browser_row_unlocked_while_waiting_for_account(pool: PgPool) {
    use console_platform_provisioning::TenantRemovalOutcome;
    let f = fixture(&pool, "browser-removal-account-wait").await;
    let audit_actor = removal_actor(&pool).await;
    for force in [false, true] {
        let mut a = actor(&pool, &f, &format!("제거 잠금 {force}")).await;
        let session = login(&f, &mut a).await;
        if force {
            sqlx::query("UPDATE organizations SET status='ARCHIVED' WHERE id=$1")
                .bind(*a.org.as_uuid())
                .execute(&pool)
                .await
                .unwrap();
        }
        let rt = runtime_login(&pool, force, "browser-remove-root-wait").await;
        let mut root = pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE")
            .bind(*a.user.as_uuid())
            .fetch_one(root.as_mut())
            .await
            .unwrap();
        let pid = super::reset_sessions::transaction_pid(&mut root).await;
        let org = a.org;
        let pending =
            tokio::spawn(async move { remove_company(&rt, force, org, audit_actor).await });
        wait_blocked(&pool, "browser-remove-root-wait", pid).await;
        let mut browser = pool.begin().await.unwrap();
        sqlx::query("SELECT context_id FROM auth_security.browser_sessions WHERE context_id=$1 FOR UPDATE NOWAIT")
            .bind(Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap()).fetch_one(browser.as_mut()).await.unwrap();
        browser.rollback().await.unwrap();
        root.rollback().await.unwrap();
        assert_eq!(
            timeout(WAIT, pending).await.unwrap().unwrap().unwrap(),
            TenantRemovalOutcome::Removed
        );
        assert_removed_session(&pool, &f, &a, &session).await;
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn native_browser_routes_never_take_authority_from_ambient_cookie_or_bearer(pool: PgPool) {
    let f = fixture(&pool, "browser-explicit-transport").await;
    let mut a = actor(&pool, &f, "명시적 자격 증명").await;
    let session = login(&f, &mut a).await;
    let bearer = private_original_proof(&pool, &f, &session).await;
    let mut b = actor(&pool, &f, "무관한 주변 자격 증명").await;
    let other = login(&f, &mut b).await;
    let other_bearer = private_original_proof(&pool, &f, &other).await;
    let native = resident_native_login(&f.router, &mut b, true).await;
    assert_eq!(native.status(), StatusCode::OK);
    let cookie = set_cookie_values(&native)
        .into_iter()
        .next()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let selected_history = own_history(&f, &session).await;
    let before = effects(&pool).await;
    for path in [HISTORY, LOGOUT] {
        let response = f
            .router
            .clone()
            .oneshot(
                browser_ingress(Request::builder().method("POST").uri(path))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {other_bearer}"))
                    .header(header::COOKIE, &cookie)
                    .body(Body::from("{}".to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(set_cookie_values(&response).is_empty());
    }
    assert_eq!(effects(&pool).await, before);
    let immutable = immutable_mapping(&pool, &session).await;
    for path in [HISTORY, LOGOUT] {
        let mut selected = handle(&session);
        if path == HISTORY {
            selected["limit"] = json!(25);
            selected["offset"] = json!(0);
        }
        let response = f
            .router
            .clone()
            .oneshot(
                browser_ingress(Request::builder().method("POST").uri(path))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {other_bearer}"))
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(serde_json::to_vec(&selected).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(set_cookie_values(&response).is_empty());
        if path == HISTORY {
            assert_eq!(response.status(), StatusCode::OK);
            let resolved = body_json(response).await;
            assert_eq!(
                resolved, selected_history,
                "ambient credentials cannot replace the body-selected Account or history"
            );
            assert_eq!(resolved["browser_context"], session["context_id"]);
            assert_eq!(resolved["context"]["company_id"], a.org.to_string());
            assert!(resolved.get("access_token").is_none());
            assert!(
                private_original_proof(&pool, &f, &session).await == bearer,
                "ambient credentials cannot replace the retained signed proof"
            );
            assert_eq!(effects(&pool).await, before);
        } else {
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
            assert_closed_mapping(&pool, &session, immutable).await;
        }
        own_history(&f, &other).await;
        assert_eq!(
            get(&f, "/api/v1/hr/attendance-records/me", &other_bearer)
                .await
                .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        post_raw(f.router.clone(), HISTORY, None, history_handle(&session))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let (_, signed) = assertion(&f, &mut a).await;
    let response = f
        .router
        .clone()
        .oneshot(
            browser_ingress(Request::builder().method("POST").uri(LOGIN))
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-auth-transport", "cookie")
                .body(Body::from(serde_json::to_vec(&signed).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        set_cookie_values(&response).is_empty(),
        "native browser endpoint never forwards refresh cookies"
    );
    let body = body_json(response).await;
    assert!(body.get("access_token").is_none() && body.get("refresh_token").is_none());
}

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../crates/platform/db/migrations");

async fn upgrade_state(owner: &PgPool) -> [u8; 32] {
    let mut state = serde_json::Map::new();
    // Fixed historical fixture allowlist; zero and populated tables are both
    // preserved. This is not a production seed or a v1-provenance claim.
    for table in [
        "public.organizations",
        "public.users",
        "public.groups",
        "public.group_memberships",
        "public.auth_webauthn_credentials",
        "public.auth_webauthn_ceremonies",
        "public.auth_webauthn_ceremony_bindings",
        "public.auth_refresh_token_families",
        "public.auth_refresh_tokens",
        "public.auth_bootstrap_credentials",
        "public.auth_legacy_otp_family_sources",
        "public.auth_legacy_registration_bindings",
        "public.audit_events",
        "public.subject_authz_versions",
        "auth_security.account_state",
        "auth_security.account_id_reservations",
        "auth_security.credential_removals",
        "auth_security.session_admissions",
        "auth_security.registration_intents",
    ] {
        let rows:Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COALESCE(jsonb_agg(to_jsonb(r) ORDER BY to_jsonb(r)::text),'[]'::jsonb) FROM {table} r")))
            .fetch_one(owner).await.unwrap();
        state.insert(table.into(), rows);
    }
    let ledger:Value = sqlx::query_scalar("SELECT COALESCE(jsonb_agg(to_jsonb(m) ORDER BY version),'[]'::jsonb) FROM _sqlx_migrations m WHERE version<=240")
        .fetch_one(owner).await.unwrap();
    state.insert("historical_migration_ledger".into(), ledger);
    Sha256::digest(serde_json::to_vec(&state).unwrap()).into()
}

async fn exact_upgrade_ledger(owner: &PgPool, through: i64) {
    let actual: Vec<(i64, Vec<u8>, bool)> =
        sqlx::query_as("SELECT version,checksum,success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(owner)
            .await
            .unwrap();
    let expected: Vec<_> = MIGRATOR
        .iter()
        .filter(|m| m.migration_type.is_up_migration() && m.version <= through)
        .map(|m| (m.version, m.checksum.to_vec(), true))
        .collect();
    assert!(
        actual == expected,
        "installed lineage must match exact locked versions/checksums/success"
    );
}

async fn complete_ledger_digest(owner: &PgPool) -> [u8; 32] {
    let ledger:Value = sqlx::query_scalar("SELECT COALESCE(jsonb_agg(to_jsonb(m) ORDER BY version),'[]'::jsonb) FROM _sqlx_migrations m")
        .fetch_one(owner).await.unwrap();
    Sha256::digest(serde_json::to_vec(&ledger).unwrap()).into()
}

#[sqlx::test(migrations = false)]
async fn actual_0240_populated_upgrade_preserves_native_identity_and_sessions(pool: PgPool) {
    MIGRATOR.run_to(240, &pool).await.unwrap();
    exact_upgrade_ledger(&pool, 240).await;
    let f = fixture(&pool, "browser-populated-upgrade").await;
    let mut a = actor(&pool, &f, "기존 계정").await;
    let start: LoginStartResponse = post_json(
        f.router.clone(),
        "/api/v1/auth/passkey/login/start",
        None,
        json!({}),
        StatusCode::OK,
    )
    .await;
    let challenge = start.challenge;
    let credential = a
        .authenticator
        .do_authentication(Url::parse(TEST_ORIGIN).unwrap(), challenge)
        .unwrap();
    let signed = json!({"ceremony_id":start.ceremony_id,"credential":credential});
    let native: TokenPairResponse = post_json(
        f.router.clone(),
        "/api/v1/auth/passkey/login/finish",
        None,
        signed,
        StatusCode::OK,
    )
    .await;
    for table in [
        "users",
        "auth_webauthn_credentials",
        "auth_refresh_token_families",
        "auth_refresh_tokens",
        "auth_bootstrap_credentials",
        "audit_events",
    ] {
        let count: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(count > 0, "{table} preservation must be nonvacuous");
    }
    let before = upgrade_state(&pool).await;
    MIGRATOR.run(&pool).await.unwrap();
    let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
        .bind(MAPPING)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        exists,
        "0241 must add private browser custody; baseline is behavior RED"
    );
    let additions: Vec<_> = MIGRATOR
        .iter()
        .filter(|m| m.migration_type.is_up_migration() && m.version > 240)
        .map(|m| m.version)
        .collect();
    assert_eq!(additions, vec![241]);
    exact_upgrade_ledger(&pool, 241).await;
    assert_eq!(upgrade_state(&pool).await, before);
    let manufactured: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_security.browser_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        manufactured, 0,
        "upgrade must not convert old families into browser authority"
    );
    let ledger = complete_ledger_digest(&pool).await;
    MIGRATOR.run(&pool).await.unwrap();
    exact_upgrade_ledger(&pool, 241).await;
    assert_eq!(upgrade_state(&pool).await, before);
    assert_eq!(complete_ledger_digest(&pool).await, ledger);
    assert_eq!(
        get(&f, "/api/v1/hr/attendance-records/me", &native.access_token)
            .await
            .status(),
        StatusCode::OK
    );
    let browser = login(&f, &mut a).await;
    own_history(&f, &browser).await;
}

async fn blocked_backend(owner: &PgPool, label: &str, blocker: i32) -> i32 {
    sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND application_name=$1 AND wait_event_type='Lock' AND $2=ANY(pg_blocking_pids(pid))")
        .bind(label).bind(blocker).fetch_one(owner).await.unwrap()
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn witnessed_login_reset_winners_never_leave_pre_reset_browser_authority(pool: PgPool) {
    for login_wins in [true, false] {
        let f = fixture(&pool, "browser-login-reset-winner").await;
        let mut subject = actor(&pool, &f, "경쟁 복구 대상").await;
        give_recovery_subject_branch(&pool, &subject).await;
        let mut admin = actor_in_org(&pool, &f, "경쟁 복구 담당자", subject.org).await;
        sqlx::query("UPDATE users SET roles=ARRAY['SUPER_ADMIN'] WHERE id=$1")
            .bind(*admin.user.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
        let administrator = login(&f, &mut admin).await;
        let admin_bearer = private_original_proof(&pool, &f, &administrator).await;
        let admin_state = immutable_mapping(&pool, &administrator).await;
        let (ceremony, signed) = assertion(&f, &mut subject).await;
        let reset_pool = runtime_login(&pool, false, "browser-reset-racer").await;
        let reset_router =
            configured_router(reset_pool, &f.private, &f.public, Some(&f.storage_key));
        let (table, event, condition, returned, query) = if login_wins {
            (
                "audit_events",
                "INSERT",
                format!(
                    "NEW.action='auth.login' AND NEW.actor='{}'::uuid",
                    subject.user
                ),
                "NEW",
                "INSERT INTO audit_events",
            )
        } else {
            (
                "auth_webauthn_credentials",
                "DELETE",
                format!("OLD.id='{}'::uuid", subject.source),
                "OLD",
                "DELETE FROM auth_webauthn_credentials",
            )
        };
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE FUNCTION public.test_browser_race_gate() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(870242); RETURN {returned}; END $$; CREATE TRIGGER test_browser_race_gate BEFORE {event} ON {table} FOR EACH ROW WHEN({condition}) EXECUTE FUNCTION public.test_browser_race_gate();")))
            .execute(&pool).await.unwrap();
        let before = effects(&pool).await;
        let mut gate = pool.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock(870242)")
            .execute(gate.as_mut())
            .await
            .unwrap();
        let gate_pid = super::reset_sessions::transaction_pid(&mut gate).await;
        let login_router = f.router.clone();
        let user = subject.user;
        let login_task;
        let reset_task;
        if login_wins {
            login_task =
                tokio::spawn(async move { post_raw(login_router, LOGIN, None, signed).await });
            super::reset_sessions::waiting(&pool, "browser-login-reset-winner", gate_pid, query)
                .await;
            let winner = blocked_backend(&pool, "browser-login-reset-winner", gate_pid).await;
            reset_task = tokio::spawn(async move {
                post_raw(
                    reset_router,
                    "/api/v1/auth/admin/credential-reset",
                    Some(&admin_bearer),
                    json!({"user_id":user.as_uuid()}),
                )
                .await
            });
            super::reset_sessions::waiting(
                &pool,
                "browser-reset-racer",
                winner,
                "SELECT is_active FROM users WHERE id = $1 AND org_id = $2 FOR NO KEY UPDATE",
            )
            .await;
        } else {
            reset_task = tokio::spawn(async move {
                post_raw(
                    reset_router,
                    "/api/v1/auth/admin/credential-reset",
                    Some(&admin_bearer),
                    json!({"user_id":user.as_uuid()}),
                )
                .await
            });
            super::reset_sessions::waiting(&pool, "browser-reset-racer", gate_pid, query).await;
            let winner = blocked_backend(&pool, "browser-reset-racer", gate_pid).await;
            login_task =
                tokio::spawn(async move { post_raw(login_router, LOGIN, None, signed).await });
            super::reset_sessions::waiting(
                &pool,
                "browser-login-reset-winner",
                winner,
                "SELECT is_active FROM users WHERE id = $1 AND org_id = $2 FOR NO KEY UPDATE",
            )
            .await;
        }
        assert_eq!(
            effects(&pool).await,
            before,
            "neither blocked owner may expose an uncommitted effect"
        );
        gate.commit().await.unwrap();
        let login_response = timeout(WAIT, login_task).await.unwrap().unwrap();
        let reset_response = timeout(WAIT, reset_task).await.unwrap().unwrap();
        assert_eq!(reset_response.status(), StatusCode::OK);
        if login_wins {
            assert_eq!(login_response.status(), StatusCode::OK);
            let session = body_json(login_response).await;
            assert_eq!(
                post_raw(f.router.clone(), HISTORY, None, history_handle(&session))
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED
            );
            let revoked:bool=sqlx::query_scalar("SELECT f.revoked_at IS NOT NULL AND NOT EXISTS(SELECT 1 FROM auth_refresh_tokens t WHERE t.family_id=f.id AND t.revoked_at IS NULL) FROM auth_refresh_token_families f WHERE id=$1")
                .bind(family(&pool,&session).await).fetch_one(&pool).await.unwrap();
            assert!(revoked);
        } else {
            assert_eq!(login_response.status(), StatusCode::UNAUTHORIZED);
            let retained:(bool,i64,i64)=sqlx::query_as("SELECT (SELECT consumed_at IS NULL FROM auth_webauthn_ceremonies WHERE id=$1),(SELECT count(*) FROM auth_security.browser_sessions WHERE account_id=$2),(SELECT count(*) FROM audit_events WHERE actor=$2 AND action='auth.login')")
                .bind(ceremony).bind(*subject.user.as_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!(
                retained,
                (true, 0, 0),
                "losing login cannot consume proof or create browser custody/login audit"
            );
        }
        own_history(&f, &administrator).await;
        assert_eq!(immutable_mapping(&pool, &administrator).await, admin_state);
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP TRIGGER test_browser_race_gate ON {table}; DROP FUNCTION public.test_browser_race_gate();")))
            .execute(&pool).await.unwrap();
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn witnessed_logout_and_both_company_removal_winners_preserve_terminal_outcomes(
    pool: PgPool,
) {
    use console_platform_provisioning::TenantRemovalOutcome;
    let f = fixture(&pool, "browser-logout-removal-winner").await;
    let audit_actor = removal_actor(&pool).await;
    for force in [false, true] {
        for logout_wins in [true, false] {
            let mut subject = actor(&pool, &f, "경쟁 회사 제거 대상").await;
            let session = login(&f, &mut subject).await;
            let bound_family = family(&pool, &session).await;
            if force {
                sqlx::query("UPDATE organizations SET status='ARCHIVED' WHERE id=$1")
                    .bind(*subject.org.as_uuid())
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            let removal_pool = runtime_login(&pool, force, "browser-removal-racer").await;
            let (table, event, condition, returned, query) = if logout_wins {
                (
                    "audit_events",
                    "INSERT",
                    format!(
                        "NEW.action='auth.logout' AND NEW.target_id='{}'",
                        bound_family
                    ),
                    "NEW",
                    "INSERT INTO audit_events",
                )
            } else {
                (
                    "users",
                    "DELETE",
                    format!("OLD.id='{}'::uuid", subject.user),
                    "OLD",
                    if force {
                        "platform_force_remove_organization_command"
                    } else {
                        "platform_remove_organization"
                    },
                )
            };
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE FUNCTION public.test_browser_remove_race() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(870243); RETURN {returned}; END $$; CREATE TRIGGER test_browser_remove_race BEFORE {event} ON {table} FOR EACH ROW WHEN({condition}) EXECUTE FUNCTION public.test_browser_remove_race();")))
                .execute(&pool).await.unwrap();
            let before = effects(&pool).await;
            let mut gate = pool.begin().await.unwrap();
            sqlx::query("SELECT pg_advisory_xact_lock(870243)")
                .execute(gate.as_mut())
                .await
                .unwrap();
            let gate_pid = super::reset_sessions::transaction_pid(&mut gate).await;
            let router = f.router.clone();
            let request = handle(&session);
            let company = subject.org;
            let logout_task;
            let removal_task;
            if logout_wins {
                logout_task =
                    tokio::spawn(async move { post_raw(router, LOGOUT, None, request).await });
                super::reset_sessions::waiting(
                    &pool,
                    "browser-logout-removal-winner",
                    gate_pid,
                    query,
                )
                .await;
                let winner =
                    blocked_backend(&pool, "browser-logout-removal-winner", gate_pid).await;
                removal_task = tokio::spawn(async move {
                    remove_company(&removal_pool, force, company, audit_actor).await
                });
                wait_blocked(&pool, "browser-removal-racer", winner).await;
            } else {
                removal_task = tokio::spawn(async move {
                    remove_company(&removal_pool, force, company, audit_actor).await
                });
                super::reset_sessions::waiting(&pool, "browser-removal-racer", gate_pid, query)
                    .await;
                let winner = blocked_backend(&pool, "browser-removal-racer", gate_pid).await;
                logout_task =
                    tokio::spawn(async move { post_raw(router, LOGOUT, None, request).await });
                super::reset_sessions::waiting(
                    &pool,
                    "browser-logout-removal-winner",
                    winner,
                    "SELECT is_active FROM users WHERE id = $1 AND org_id = $2 FOR NO KEY UPDATE",
                )
                .await;
            }
            assert_eq!(effects(&pool).await, before);
            gate.commit().await.unwrap();
            assert_eq!(
                timeout(WAIT, logout_task).await.unwrap().unwrap().status(),
                StatusCode::NO_CONTENT
            );
            assert_eq!(
                timeout(WAIT, removal_task).await.unwrap().unwrap().unwrap(),
                TenantRemovalOutcome::Removed
            );
            assert_removed_session(&pool, &f, &subject, &session).await;
            let audits: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM audit_events WHERE action='auth.logout' AND target_id=$1",
            )
            .bind(bound_family.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                audits,
                i64::from(logout_wins),
                "only a completed native logout creates its own audit"
            );
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP TRIGGER test_browser_remove_race ON {table}; DROP FUNCTION public.test_browser_remove_race();")))
                .execute(&pool).await.unwrap();
        }
    }
}

async fn prepare_real_worker_storage(pool: &PgPool) {
    use sqlx::Connection as _;
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(
        database.starts_with("_sqlx_test_"),
        "worker fixture may alter only its disposable test database"
    );
    let existing: Option<String> = sqlx::query_scalar("SELECT to_regnamespace('apalis')::text")
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(
        existing.is_none(),
        "never adopt somebody else's Apalis schema"
    );
    let ddl: String = sqlx::query_scalar(
        "SELECT format('ALTER DATABASE %I OWNER TO console_app',current_database())",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
        .execute(pool)
        .await
        .unwrap();
    let password = std::env::var("CONSOLE_TEST_OWNER_PASSWORD")
        .expect("disposable owner credential is a worker fixture prerequisite");
    let options = pool
        .connect_options()
        .as_ref()
        .clone()
        .username("console_app")
        .password(&password);
    assert!(
        sqlx::PgConnection::connect_with(
            &options
                .clone()
                .password("deliberately-wrong-owner-test-password")
        )
        .await
        .is_err(),
        "owner fixture requires real password authentication"
    );
    let mut owner = sqlx::PgConnection::connect_with(&options).await.unwrap();
    let identity:(String,String,String)=sqlx::query_as("SELECT session_user::text,current_user::text,pg_get_userbyid(datdba) FROM pg_database WHERE datname=current_database()")
        .fetch_one(&mut owner).await.unwrap();
    assert_eq!(
        identity,
        (
            "console_app".into(),
            "console_app".into(),
            "console_app".into()
        )
    );
    console_platform_jobs::migrate_and_reconcile_apalis_postgres(&mut owner)
        .await
        .expect("Apalis owner preparation must pass independently of cleanup behavior");
}

// Drain every byte without retaining or publishing subprocess error contents.
// Only bounded line prefixes are examined for fixed startup failure classes.
#[derive(Default)]
struct WorkerStderr {
    bytes: usize,
    app_error: Option<&'static str>,
    address_in_use: bool,
    panicked: bool,
    thread_spawn: bool,
    read_failed: bool,
}

impl WorkerStderr {
    fn classify(&mut self, prefix: &[u8]) {
        for variant in [
            "Config",
            "Database",
            "Internal",
            "Io",
            "Storage",
            "Telemetry",
            "Worker",
            "Realtime",
        ] {
            if prefix.starts_with(format!("Error: {variant}(").as_bytes()) {
                self.app_error = Some(variant);
                if variant == "Io" {
                    self.address_in_use |= prefix
                        .windows(b"kind: AddrInUse".len())
                        .any(|part| part == b"kind: AddrInUse");
                }
            }
        }
        self.panicked |= prefix
            .windows(b"panicked at".len())
            .any(|part| part == b"panicked at");
        self.thread_spawn |= prefix
            .windows(b"OS can't spawn worker thread:".len())
            .any(|part| part == b"OS can't spawn worker thread:");
    }

    fn drain(mut stderr: std::process::ChildStderr) -> Self {
        use std::io::Read as _;
        let mut result = Self::default();
        let mut bytes = [0_u8; 8192];
        let mut prefix = Vec::with_capacity(512);
        loop {
            match stderr.read(&mut bytes) {
                Ok(0) => break,
                Ok(length) => {
                    result.bytes = result.bytes.saturating_add(length);
                    for byte in &bytes[..length] {
                        if *byte == b'\n' {
                            result.classify(&prefix);
                            prefix.clear();
                        } else if prefix.len() < 512 {
                            prefix.push(*byte);
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    result.read_failed = true;
                    break;
                }
            }
        }
        result.classify(&prefix);
        result
    }
}

struct RealWorker {
    child: std::process::Child,
    ready_url: String,
    service: String,
    worker_id: String,
    stderr: Option<std::thread::JoinHandle<WorkerStderr>>,
    spawned_at: std::time::Instant,
    ready_verified: bool,
    last_ready: &'static str,
    last_registered: bool,
}

impl Drop for RealWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.stderr.take() {
            let _ = reader.join();
        }
    }
}

impl RealWorker {
    async fn start(owner: &PgPool, suffix: &str) -> Self {
        let database: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(owner)
            .await
            .unwrap();
        assert!(database.starts_with("_sqlx_test_"));
        let mut runtime_url = Url::parse(&std::env::var("DATABASE_URL").unwrap()).unwrap();
        runtime_url.set_username("console_rt").unwrap();
        runtime_url
            .set_password(Some(
                &std::env::var("CONSOLE_TEST_RUNTIME_PASSWORD").unwrap(),
            ))
            .unwrap();
        runtime_url.set_path(&format!("/{database}"));
        runtime_url.set_query(None);
        runtime_url.set_fragment(None);
        let reserve = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = reserve.local_addr().unwrap();
        let service = format!("browser-cleanup-{}", Uuid::new_v4().simple());
        let pod = format!("test-{suffix}");
        let worker_id = format!("{service}-{pod}-dispatch-worker");
        let already_registered: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM apalis.workers WHERE id=$1)")
                .bind(&worker_id)
                .fetch_one(owner)
                .await
                .unwrap();
        assert!(
            !already_registered,
            "fixture must use a genuinely new worker identity"
        );
        drop(reserve);
        let spawned_at = std::time::Instant::now();
        let child = std::process::Command::new(env!("CARGO_BIN_EXE_console-app"))
            .env_clear()
            .env("CONSOLE_APP_ROLE", "worker")
            .env("CONSOLE_SERVICE_NAME", &service)
            .env("CONSOLE_POD_NAME", &pod)
            .env("CONSOLE_HTTP_ADDR", addr.to_string())
            .env("DATABASE_URL", runtime_url.as_str())
            .env("CONSOLE_DISPATCH_JOBS_ENABLED", "true")
            .env("CONSOLE_SHUTDOWN_TIMEOUT_SECS", "2")
            .env("RUST_LOG", "error")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("worker fixture prerequisite: binary must spawn");
        let mut worker = Self {
            child,
            ready_url: format!("http://{addr}/readyz"),
            service,
            worker_id,
            stderr: None,
            spawned_at,
            ready_verified: false,
            last_ready: "unobserved",
            last_registered: false,
        };
        // The owner guard must exist before diagnostic thread creation can fail.
        let stderr = worker.child.stderr.take().expect("worker diagnostic pipe");
        worker.stderr = Some(
            std::thread::Builder::new()
                .spawn(move || WorkerStderr::drain(stderr))
                .unwrap_or_else(|_| panic!("worker diagnostic reader start")),
        );
        worker.require_ready(owner).await;
        worker
    }
    async fn require_ready(&mut self, owner: &PgPool) {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(1))
            .build()
            .unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(25);
        loop {
            self.require_alive();
            let mut ready = false;
            self.last_ready = "request_failed";
            if let Ok(response) = client.get(&self.ready_url).send().await {
                self.last_ready = "non_ok_status";
                if response.status() == reqwest::StatusCode::OK {
                    self.last_ready = "invalid_response";
                    if let Ok(bytes) = response.bytes().await
                        && let Ok(body) = serde_json::from_slice::<Value>(&bytes)
                    {
                        ready = body["service"].as_str() == Some(self.service.as_str())
                            && body["role"] == "worker"
                            && body["status"] == "ready"
                            && body["write_ready"] == false;
                        self.last_ready = if ready { "ready" } else { "identity_mismatch" };
                    }
                }
            }
            let registered:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM apalis.workers WHERE id=$1 AND worker_type='console.dispatch')")
                .bind(&self.worker_id).fetch_one(owner).await.unwrap();
            self.last_registered = registered;
            if ready && registered {
                self.require_alive();
                self.ready_verified = true;
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "worker prerequisite failed: no exact ready service and fresh Apalis registration"
            );
            sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    fn require_alive(&mut self) {
        if let Some(status) = self.child.try_wait().unwrap() {
            let drained = self.stderr.take().map(|reader| reader.join());
            let (drain_joined, evidence) = match drained {
                Some(Ok(evidence)) => (true, evidence),
                _ => (false, WorkerStderr::default()),
            };
            let phase = if self.ready_verified {
                "runtime"
            } else {
                "startup"
            };
            eprintln!(
                "REAL_WORKER_FAILURE exit_status={status:?} phase={phase} elapsed_ms={} last_ready={} registered={} drain_joined={drain_joined} stderr_bytes={} app_error={:?} address_in_use={} panicked={} thread_spawn={} read_failed={}",
                self.spawned_at.elapsed().as_millis(),
                self.last_ready,
                self.last_registered,
                evidence.bytes,
                evidence.app_error,
                evidence.address_in_use,
                evidence.panicked,
                evidence.thread_spawn,
                evidence.read_failed
            );
            panic!("worker infrastructure failure: process exited before cleanup proof");
        }
    }
    fn crash(&mut self) {
        self.child.kill().expect("crash real worker");
        self.child.wait().expect("reap crashed worker");
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn real_worker_boot_prerequisites_are_independent_of_new_browser_routes(pool: PgPool) {
    prepare_real_worker_storage(&pool).await;
    let mut worker = RealWorker::start(&pool, "boot-baseline").await;
    worker.require_alive();
    worker.crash();
}

async fn wait_for_proof_count(
    owner: &PgPool,
    company: OrgId,
    expected: i64,
    workers: &mut [&mut RealWorker],
) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(75);
    loop {
        for worker in workers.iter_mut() {
            worker.require_alive();
        }
        let pending:i64=sqlx::query_scalar("SELECT count(*) FROM auth_security.browser_sessions WHERE company_id=$1 AND expires_at<=clock_timestamp() AND ciphertext IS NOT NULL")
            .bind(*company.as_uuid()).fetch_one(owner).await.unwrap();
        if pending == expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "live native worker must clear eligible proof within its bounded observation window"
        );
        sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn real_workers_clear_expired_proof_without_root_locks_and_recover_after_crash(pool: PgPool) {
    let mut f = fixture(&pool, "browser-worker-fixture").await;
    let mut a = actor(&pool, &f, "정리 첫 회사").await;
    let mut b = actor(&pool, &f, "정리 둘째 회사").await;
    let mut removed = actor(&pool, &f, "정리 제거 회사").await;
    f.router = configured_router_with(
        f.runtime.clone(),
        &f.private,
        &f.public,
        Some(&f.storage_key),
        &[("CONSOLE_REFRESH_FAMILY_ABSOLUTE_TTL_SECS", "6".into())],
    );
    let one = login(&f, &mut a).await;
    let two = login(&f, &mut a).await;
    let three = login(&f, &mut a).await;
    let other = login(&f, &mut b).await;
    let retired = login(&f, &mut removed).await;
    let audit_actor = removal_actor(&pool).await;
    assert_eq!(
        remove_company(&f.runtime, false, removed.org, audit_actor)
            .await
            .unwrap(),
        console_platform_provisioning::TenantRemovalOutcome::Removed
    );
    assert_removed_session(&pool, &f, &removed, &retired).await;
    let retired_before: Value = sqlx::query_scalar(
        "SELECT to_jsonb(s) FROM auth_security.browser_sessions s WHERE context_id=$1",
    )
    .bind(Uuid::parse_str(retired["context_id"].as_str().unwrap()).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    let deadline = [&one, &two, &three, &other]
        .into_iter()
        .map(|session| {
            OffsetDateTime::parse(
                session["expires_at"].as_str().unwrap(),
                &time::format_description::well_known::Rfc3339,
            )
            .unwrap()
        })
        .max()
        .unwrap();
    sqlx::query(
        "SELECT pg_sleep(greatest(0,extract(epoch FROM ($1::timestamptz-clock_timestamp())))+0.1)",
    )
    .bind(deadline)
    .execute(&pool)
    .await
    .unwrap();
    f.router = configured_router(
        f.runtime.clone(),
        &f.private,
        &f.public,
        Some(&f.storage_key),
    );
    let live = login(&f, &mut a).await;
    let mut expected = vec![*a.org.as_uuid(), *b.org.as_uuid(), *removed.org.as_uuid()];
    expected.sort();
    for limit in [1_i32, 2] {
        let mut cursor: Option<Uuid> = None;
        let mut discovered = Vec::new();
        loop {
            let page: Vec<Uuid> = sqlx::query_scalar(
                "SELECT company_id FROM public.platform_browser_session_cleanup_companies($1,$2)",
            )
            .bind(cursor)
            .bind(limit)
            .fetch_all(&f.runtime)
            .await
            .unwrap();
            assert!(page.len() <= limit as usize);
            if page.is_empty() {
                break;
            }
            assert!(page.windows(2).all(|w| w[0] < w[1]));
            assert!(page.iter().all(|id| cursor.is_none_or(|prior| *id > prior)));
            cursor = page.last().copied();
            assert!(
                page.iter().all(|id| expected.contains(id)),
                "discovery cannot fabricate Company IDs"
            );
            discovered.extend(page);
            assert!(
                discovered.len() <= expected.len(),
                "discovery must terminate within the known fixture set"
            );
        }
        assert_eq!(
            discovered, expected,
            "bounded id-only discovery must include retained Companies after root deletion"
        );
    }
    let mut tx = f.runtime.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(removed.org.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let nothing: i64 = sqlx::query_scalar("SELECT public.platform_browser_session_cleanup($1,$2)")
        .bind(*removed.org.as_uuid())
        .bind(2_i32)
        .fetch_one(tx.as_mut())
        .await
        .unwrap();
    assert_eq!(
        nothing, 0,
        "rootless already-cleared custody needs no proof restoration"
    );
    tx.commit().await.unwrap();
    prepare_real_worker_storage(&pool).await;
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE")
        .bind(*a.user.as_uuid())
        .fetch_one(lock.as_mut())
        .await
        .unwrap();
    let mut row_lock = pool.begin().await.unwrap();
    sqlx::query(
        "SELECT context_id FROM auth_security.browser_sessions WHERE context_id=$1 FOR UPDATE",
    )
    .bind(Uuid::parse_str(one["context_id"].as_str().unwrap()).unwrap())
    .fetch_one(row_lock.as_mut())
    .await
    .unwrap();
    let bindings = [
        immutable_mapping(&pool, &one).await,
        immutable_mapping(&pool, &two).await,
        immutable_mapping(&pool, &three).await,
        immutable_mapping(&pool, &other).await,
    ];
    let unchanged = custody_digest(&pool, true).await;
    let live_before = complete_mapping_digest(&pool, &live).await;
    let mut first = RealWorker::start(&pool, "first").await;
    wait_for_proof_count(&pool, a.org, 1, &mut [&mut first]).await;
    wait_for_proof_count(&pool, b.org, 0, &mut [&mut first]).await;
    first.crash();
    let mut restart = RealWorker::start(&pool, "restart").await;
    let mut concurrent = RealWorker::start(&pool, "concurrent").await;
    row_lock.commit().await.unwrap();
    wait_for_proof_count(&pool, a.org, 0, &mut [&mut restart, &mut concurrent]).await;
    assert_eq!(
        custody_digest(&pool, true).await,
        unchanged,
        "workers may change only expired sealed proof"
    );
    assert_eq!(complete_mapping_digest(&pool, &live).await, live_before);
    lock.commit().await.unwrap();
    for (session, binding) in [&one, &two, &three, &other].into_iter().zip(bindings) {
        let preserved:bool=sqlx::query_scalar("SELECT closed_at IS NULL AND owner_removed_at IS NULL AND ciphertext IS NULL AND nonce IS NULL AND tag IS NULL FROM auth_security.browser_sessions WHERE context_id=$1")
            .bind(Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap()).fetch_one(&pool).await.unwrap();
        assert!(preserved);
        assert_eq!(immutable_mapping(&pool, session).await, binding);
        let family_live: bool = sqlx::query_scalar(
            "SELECT revoked_at IS NULL FROM auth_refresh_token_families WHERE id=$1",
        )
        .bind(family(&pool, session).await)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            family_live,
            "cleanup cannot claim logout or revoke native authority"
        );
    }
    let retired_after: Value = sqlx::query_scalar(
        "SELECT to_jsonb(s) FROM auth_security.browser_sessions s WHERE context_id=$1",
    )
    .bind(Uuid::parse_str(retired["context_id"].as_str().unwrap()).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        retired_after == retired_before,
        "restart/concurrent workers must preserve token-free rootless terminal state"
    );
    own_history(&f, &live).await;
    assert_removed_session(&pool, &f, &removed, &retired).await;
    restart.require_alive();
    concurrent.require_alive();
}

// R6 test preparation draft. Append only after active prerequisite compilation
// and independent test-source review; this is not compiled or admitted evidence.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn witnessed_fresh_login_and_old_family_logout_preserve_independent_families(pool: PgPool) {
    for login_wins in [true, false] {
        let f = fixture(&pool, "browser-fresh-login-racer").await;
        let mut subject = actor(&pool, &f, "새 로그인과 이전 세션 종료").await;
        let original = login(&f, &mut subject).await;
        let original_family = family(&pool, &original).await;
        let original_mapping = immutable_mapping(&pool, &original).await;
        let prior_families: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM auth_refresh_token_families WHERE user_id=$1 AND org_id=$2 ORDER BY id",
        )
        .bind(*subject.user.as_uuid())
        .bind(*subject.org.as_uuid())
        .fetch_all(&pool)
        .await
        .unwrap();
        let prior_tokens: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM auth_refresh_tokens WHERE user_id=$1 AND org_id=$2 ORDER BY id",
        )
        .bind(*subject.user.as_uuid())
        .bind(*subject.org.as_uuid())
        .fetch_all(&pool)
        .await
        .unwrap();
        let retained_sql = "SELECT jsonb_build_object(
            'families',(SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM auth_refresh_token_families f WHERE user_id=$1 AND org_id=$2 AND id<>ALL($3)),
            'tokens',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM auth_refresh_tokens t WHERE user_id=$1 AND org_id=$2 AND family_id<>ALL($3)))";
        let retained: Value = sqlx::query_scalar(retained_sql)
            .bind(*subject.user.as_uuid())
            .bind(*subject.org.as_uuid())
            .bind(vec![original_family])
            .fetch_one(&pool)
            .await
            .unwrap();
        let retained_digest = Sha256::digest(serde_json::to_vec(&retained).unwrap());
        let (ceremony, signed) = assertion(&f, &mut subject).await;
        let logout_pool = runtime_login(&pool, false, "browser-old-logout-racer").await;
        let logout_router =
            configured_router(logout_pool, &f.private, &f.public, Some(&f.storage_key));
        let condition = if login_wins {
            format!(
                "NEW.action='auth.login' AND NEW.actor='{}'::uuid",
                subject.user
            )
        } else {
            format!(
                "NEW.action='auth.logout' AND NEW.target_id='{}'",
                original_family
            )
        };
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE FUNCTION public.test_browser_fresh_old_gate() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(870244); RETURN NEW; END $$; CREATE TRIGGER test_browser_fresh_old_gate BEFORE INSERT ON audit_events FOR EACH ROW WHEN({condition}) EXECUTE FUNCTION public.test_browser_fresh_old_gate();")))
            .execute(&pool).await.unwrap();
        let before = effects(&pool).await;
        let mut gate = pool.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock(870244)")
            .execute(gate.as_mut())
            .await
            .unwrap();
        let gate_pid = super::reset_sessions::transaction_pid(&mut gate).await;
        let login_router = f.router.clone();
        let request = handle(&original);
        let login_task;
        let logout_task;
        if login_wins {
            login_task =
                tokio::spawn(async move { post_raw(login_router, LOGIN, None, signed).await });
            super::reset_sessions::waiting(
                &pool,
                "browser-fresh-login-racer",
                gate_pid,
                "INSERT INTO audit_events",
            )
            .await;
            let winner = blocked_backend(&pool, "browser-fresh-login-racer", gate_pid).await;
            logout_task =
                tokio::spawn(async move { post_raw(logout_router, LOGOUT, None, request).await });
            super::reset_sessions::waiting(
                &pool,
                "browser-old-logout-racer",
                winner,
                "SELECT is_active FROM users WHERE id = $1 AND org_id = $2 FOR NO KEY UPDATE",
            )
            .await;
        } else {
            logout_task =
                tokio::spawn(async move { post_raw(logout_router, LOGOUT, None, request).await });
            super::reset_sessions::waiting(
                &pool,
                "browser-old-logout-racer",
                gate_pid,
                "INSERT INTO audit_events",
            )
            .await;
            let winner = blocked_backend(&pool, "browser-old-logout-racer", gate_pid).await;
            login_task =
                tokio::spawn(async move { post_raw(login_router, LOGIN, None, signed).await });
            super::reset_sessions::waiting(
                &pool,
                "browser-fresh-login-racer",
                winner,
                "SELECT is_active FROM users WHERE id = $1 AND org_id = $2 FOR NO KEY UPDATE",
            )
            .await;
        }
        assert_eq!(
            effects(&pool).await,
            before,
            "blocked owners cannot publish their uncommitted custody"
        );
        gate.commit().await.unwrap();
        assert_eq!(
            timeout(WAIT, logout_task).await.unwrap().unwrap().status(),
            StatusCode::NO_CONTENT
        );
        let response = timeout(WAIT, login_task).await.unwrap().unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let fresh = body_json(response).await;
        assert_eq!(fresh["context_id"], ceremony.to_string());
        let fresh_family = family(&pool, &fresh).await;
        assert_ne!(fresh_family, original_family);
        let families: (bool, bool) = sqlx::query_as(
            "SELECT (SELECT revoked_at IS NOT NULL AND NOT EXISTS(SELECT 1 FROM auth_refresh_tokens t WHERE t.family_id=f.id AND t.revoked_at IS NULL) FROM auth_refresh_token_families f WHERE id=$1),
                (SELECT revoked_at IS NULL AND EXISTS(SELECT 1 FROM auth_refresh_tokens t WHERE t.family_id=f.id AND t.revoked_at IS NULL) FROM auth_refresh_token_families f WHERE id=$2)"
        ).bind(original_family).bind(fresh_family).fetch_one(&pool).await.unwrap();
        assert_eq!(families, (true, true));
        assert!(!prior_families.contains(&fresh_family));
        let mut expected_families = prior_families;
        expected_families.push(fresh_family);
        expected_families.sort();
        let actual_families: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM auth_refresh_token_families WHERE user_id=$1 AND org_id=$2 ORDER BY id",
        )
        .bind(*subject.user.as_uuid())
        .bind(*subject.org.as_uuid())
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(actual_families, expected_families);
        let preserved_tokens: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM auth_refresh_tokens WHERE user_id=$1 AND org_id=$2 AND family_id<>$3 ORDER BY id"
        ).bind(*subject.user.as_uuid()).bind(*subject.org.as_uuid()).bind(fresh_family).fetch_all(&pool).await.unwrap();
        assert_eq!(preserved_tokens, prior_tokens);
        let fresh_tokens: i64 =
            sqlx::query_scalar("SELECT count(*) FROM auth_refresh_tokens WHERE family_id=$1")
                .bind(fresh_family)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(fresh_tokens, 1);
        let preserved: Value = sqlx::query_scalar(retained_sql)
            .bind(*subject.user.as_uuid())
            .bind(*subject.org.as_uuid())
            .bind(vec![original_family, fresh_family])
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            Sha256::digest(serde_json::to_vec(&preserved).unwrap()),
            retained_digest,
            "the competing owners must preserve all unrelated enrollment family/token custody"
        );
        assert_closed_mapping(&pool, &original, original_mapping).await;
        assert_eq!(
            post_raw(f.router.clone(), HISTORY, None, history_handle(&original))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        own_history(&f, &fresh).await;
        let audits: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM audit_events WHERE action='auth.logout' AND target_id=$1),
                (SELECT count(*) FROM audit_events WHERE action='auth.login' AND actor=$2 AND org_id=$3)"
        ).bind(original_family.to_string()).bind(*subject.user.as_uuid()).bind(*subject.org.as_uuid()).fetch_one(&pool).await.unwrap();
        assert_eq!(audits, (1, 2));
        let final_state = effects(&pool).await;
        assert_eq!(
            post_raw(f.router.clone(), LOGOUT, None, handle(&original))
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            effects(&pool).await,
            final_state,
            "old logout retry cannot alter the new family"
        );
        sqlx::raw_sql("DROP TRIGGER test_browser_fresh_old_gate ON audit_events; DROP FUNCTION public.test_browser_fresh_old_gate();")
            .execute(&pool).await.unwrap();
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn witnessed_login_and_guarded_active_company_removal_preserve_exact_terminal_custody(
    pool: PgPool,
) {
    use console_platform_provisioning::TenantRemovalOutcome;
    let audit_actor = removal_actor(&pool).await;
    for login_wins in [true, false] {
        let f = fixture(&pool, "browser-login-guarded-remove-racer").await;
        let mut subject = actor(&pool, &f, "활성 회사 로그인과 제거").await;
        let original = login(&f, &mut subject).await;
        let (ceremony, signed) = assertion(&f, &mut subject).await;
        let removal_pool = runtime_login(&pool, false, "browser-guarded-remove-login-racer").await;
        let (table, event, condition, returned, query) = if login_wins {
            (
                "audit_events",
                "INSERT",
                format!(
                    "NEW.action='auth.login' AND NEW.actor='{}'::uuid",
                    subject.user
                ),
                "NEW",
                "INSERT INTO audit_events",
            )
        } else {
            (
                "users",
                "DELETE",
                format!("OLD.id='{}'::uuid", subject.user),
                "OLD",
                "platform_remove_organization",
            )
        };
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE FUNCTION public.test_browser_login_remove_gate() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(870245); RETURN {returned}; END $$; CREATE TRIGGER test_browser_login_remove_gate BEFORE {event} ON {table} FOR EACH ROW WHEN({condition}) EXECUTE FUNCTION public.test_browser_login_remove_gate();")))
            .execute(&pool).await.unwrap();
        let before = effects(&pool).await;
        let mut gate = pool.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock(870245)")
            .execute(gate.as_mut())
            .await
            .unwrap();
        let gate_pid = super::reset_sessions::transaction_pid(&mut gate).await;
        let router = f.router.clone();
        let company = subject.org;
        let login_task;
        let removal_task;
        if login_wins {
            login_task = tokio::spawn(async move { post_raw(router, LOGIN, None, signed).await });
            super::reset_sessions::waiting(
                &pool,
                "browser-login-guarded-remove-racer",
                gate_pid,
                query,
            )
            .await;
            let winner =
                blocked_backend(&pool, "browser-login-guarded-remove-racer", gate_pid).await;
            removal_task = tokio::spawn(async move {
                remove_company(&removal_pool, false, company, audit_actor).await
            });
            wait_blocked(&pool, "browser-guarded-remove-login-racer", winner).await;
        } else {
            removal_task = tokio::spawn(async move {
                remove_company(&removal_pool, false, company, audit_actor).await
            });
            super::reset_sessions::waiting(
                &pool,
                "browser-guarded-remove-login-racer",
                gate_pid,
                query,
            )
            .await;
            let winner =
                blocked_backend(&pool, "browser-guarded-remove-login-racer", gate_pid).await;
            login_task = tokio::spawn(async move { post_raw(router, LOGIN, None, signed).await });
            super::reset_sessions::waiting(
                &pool,
                "browser-login-guarded-remove-racer",
                winner,
                "SELECT is_active FROM users WHERE id = $1 AND org_id = $2 FOR NO KEY UPDATE",
            )
            .await;
        }
        assert_eq!(effects(&pool).await, before);
        gate.commit().await.unwrap();
        assert_eq!(
            timeout(WAIT, removal_task).await.unwrap().unwrap().unwrap(),
            TenantRemovalOutcome::Removed
        );
        let response = timeout(WAIT, login_task).await.unwrap().unwrap();
        assert_removed_session(&pool, &f, &subject, &original).await;
        if login_wins {
            assert_eq!(response.status(), StatusCode::OK);
            let fresh = body_json(response).await;
            assert_eq!(fresh["context_id"], ceremony.to_string());
            assert_removed_session(&pool, &f, &subject, &fresh).await;
        } else {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            let preserved: (Option<Uuid>, bool) = sqlx::query_as(
                "SELECT user_id,consumed_at IS NULL FROM auth_webauthn_ceremonies WHERE id=$1",
            )
            .bind(ceremony)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                preserved,
                (None, true),
                "losing login must preserve the original usernameless unconsumed ceremony"
            );
        }
        let retained: (i64, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM auth_security.browser_sessions WHERE account_id=$1 AND company_id=$2),
                (SELECT count(*) FROM audit_events WHERE action='auth.login' AND after_snap->>'passkey_id'=$3),
                (SELECT count(*) FROM audit_events WHERE action='auth.logout' AND target_id IN (SELECT family_id::text FROM auth_security.browser_sessions WHERE account_id=$1 AND company_id=$2)),
                (SELECT count(*) FROM auth_refresh_token_families WHERE user_id=$1),
                (SELECT count(*) FROM auth_refresh_tokens WHERE user_id=$1)"
        ).bind(*subject.user.as_uuid()).bind(*subject.org.as_uuid()).bind(subject.source.to_string()).fetch_one(&pool).await.unwrap();
        assert_eq!(
            retained,
            (
                if login_wins { 2 } else { 1 },
                if login_wins { 2 } else { 1 },
                0,
                0,
                0
            )
        );
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP TRIGGER test_browser_login_remove_gate ON {table}; DROP FUNCTION public.test_browser_login_remove_gate();")))
            .execute(&pool).await.unwrap();
    }
}

// Source preparation only; append inside browser_sessions after prerequisite proof.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn missing_family_without_owner_removal_cannot_certify_logout(pool: PgPool) {
    let f = fixture(&pool, "browser-missing-family").await;
    let mut a = actor(&pool, &f, "종료 근거 없는 가족 삭제").await;
    let missing = login(&f, &mut a).await;
    let other = login(&f, &mut a).await;
    let lost_family = family(&pool, &missing).await;
    let mapping_before = complete_mapping_digest(&pool, &missing).await;
    assert_eq!(
        sqlx::query("DELETE FROM public.auth_refresh_token_families WHERE id=$1")
            .bind(lost_family)
            .execute(&pool)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
    let retained: (i64, i64, i64, bool) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM auth_refresh_tokens WHERE family_id=$1),
                (SELECT count(*) FROM users WHERE id=$2),
                (SELECT count(*) FROM auth_webauthn_credentials WHERE id=$3),
                (SELECT closed_at IS NULL AND owner_removed_at IS NULL FROM auth_security.browser_sessions WHERE context_id=$4)",
    )
    .bind(lost_family)
    .bind(*a.user.as_uuid())
    .bind(a.source)
    .bind(Uuid::parse_str(missing["context_id"].as_str().unwrap()).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(retained, (0, 1, 1, true));
    assert_eq!(
        complete_mapping_digest(&pool, &missing).await,
        mapping_before
    );
    // The deliberate owner fault is the baseline, not a successful logout.
    let before = effects(&pool).await;
    for route in [HISTORY, LOGOUT] {
        let response = post_raw(
            f.router.clone(),
            route,
            None,
            transport_body(route, &missing),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(set_cookie_values(&response).is_empty());
        let denied = body_json(response).await;
        for field in ["access_token", "account_id", "company_id"] {
            assert!(denied.get(field).is_none(), "denial cannot reveal custody");
        }
        assert_eq!(effects(&pool).await, before);
    }
    own_history(&f, &other).await;
    assert_eq!(effects(&pool).await, before);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn forged_owner_removal_cannot_clear_live_browser_custody(pool: PgPool) {
    let f = fixture(&pool, "browser-forged-removal").await;
    let mut a = actor(&pool, &f, "위조 제거 거부").await;
    let session = login(&f, &mut a).await;
    let before = effects(&pool).await;
    let error = sqlx::query(
        "UPDATE auth_security.browser_sessions SET owner_removed_at=clock_timestamp(),
         closed_at=clock_timestamp(),ciphertext=NULL,nonce=NULL,tag=NULL WHERE context_id=$1",
    )
    .bind(Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap())
    .execute(&pool)
    .await
    .expect_err("accepted live custody cannot invent terminal owner removal");
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    assert_eq!(effects(&pool).await, before);
    own_history(&f, &session).await;
    assert_eq!(effects(&pool).await, before);
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn cleared_proof_and_terminal_browser_state_cannot_be_restored(pool: PgPool) {
    use console_platform_provisioning::TenantRemovalOutcome;
    let mut f = fixture(&pool, "browser-proof-restoration").await;
    let audit_actor = removal_actor(&pool).await;
    for mode in 0..3 {
        let mut a = actor(&pool, &f, &format!("종료 불가역 {mode}")).await;
        if mode == 1 {
            f.router = configured_router_with(
                f.runtime.clone(),
                &f.private,
                &f.public,
                Some(&f.storage_key),
                &[("CONSOLE_REFRESH_FAMILY_ABSOLUTE_TTL_SECS", "6".into())],
            );
        }
        let session = login(&f, &mut a).await;
        let context = Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap();
        let proof: (Vec<u8>, Vec<u8>, Vec<u8>) = sqlx::query_as(
            "SELECT ciphertext,nonce,tag FROM auth_security.browser_sessions WHERE context_id=$1",
        )
        .bind(context)
        .fetch_one(&pool)
        .await
        .unwrap();
        if mode == 0 {
            assert_eq!(
                post_raw(f.router.clone(), LOGOUT, None, handle(&session))
                    .await
                    .status(),
                StatusCode::NO_CONTENT
            );
        } else if mode == 1 {
            let deadline = OffsetDateTime::parse(
                session["expires_at"].as_str().unwrap(),
                &time::format_description::well_known::Rfc3339,
            )
            .unwrap();
            sqlx::query("SELECT pg_sleep(greatest(0,extract(epoch FROM ($1::timestamptz-clock_timestamp())))+0.1)")
                .bind(deadline).execute(&pool).await.unwrap();
            let mut tx = f.runtime.begin().await.unwrap();
            sqlx::query("SELECT set_config('app.current_org',$1,true)")
                .bind(a.org.to_string())
                .execute(tx.as_mut())
                .await
                .unwrap();
            let cleared: i64 =
                sqlx::query_scalar("SELECT public.platform_browser_session_cleanup($1,$2)")
                    .bind(*a.org.as_uuid())
                    .bind(1_i32)
                    .fetch_one(tx.as_mut())
                    .await
                    .unwrap();
            assert_eq!(cleared, 1);
            tx.commit().await.unwrap();
            f.router = configured_router(
                f.runtime.clone(),
                &f.private,
                &f.public,
                Some(&f.storage_key),
            );
        } else {
            assert_eq!(
                remove_company(&f.runtime, false, a.org, audit_actor)
                    .await
                    .unwrap(),
                TenantRemovalOutcome::Removed
            );
        }
        let state: (bool, bool, bool) = sqlx::query_as(
            "SELECT ciphertext IS NULL AND nonce IS NULL AND tag IS NULL,
                    closed_at IS NOT NULL,owner_removed_at IS NOT NULL
             FROM auth_security.browser_sessions WHERE context_id=$1",
        )
        .bind(context)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, (true, mode != 1, mode == 2));
        let before = effects(&pool).await;
        let mapping_before = complete_mapping_digest(&pool, &session).await;
        for statement in [
            "UPDATE auth_security.browser_sessions SET ciphertext=$1,nonce=$2,tag=$3 WHERE context_id=$4",
            "UPDATE auth_security.browser_sessions SET ciphertext=$1,nonce=$2,tag=$3,closed_at=NULL,owner_removed_at=NULL WHERE context_id=$4",
        ] {
            // Genuine original bytes defeat malformed-shape rejection masking.
            let error = sqlx::query(statement)
                .bind(&proof.0)
                .bind(&proof.1)
                .bind(&proof.2)
                .bind(context)
                .execute(&pool)
                .await
                .expect_err("cleared proof cannot be resurrected");
            assert_eq!(
                error.as_database_error().unwrap().code().as_deref(),
                Some("23514")
            );
            assert_eq!(effects(&pool).await, before);
            assert_eq!(
                complete_mapping_digest(&pool, &session).await,
                mapping_before
            );
        }
        if mode == 2 {
            for statement in [
                "UPDATE auth_security.browser_sessions SET owner_removed_at=NULL WHERE context_id=$1",
                "UPDATE auth_security.browser_sessions SET owner_removed_at=owner_removed_at+interval '1 second' WHERE context_id=$1",
            ] {
                let error = sqlx::query(statement)
                    .bind(context)
                    .execute(&pool)
                    .await
                    .expect_err("terminal removal timestamp is immutable");
                assert_eq!(
                    error.as_database_error().unwrap().code().as_deref(),
                    Some("23514")
                );
                assert_eq!(effects(&pool).await, before);
            }
            assert_removed_session(&pool, &f, &a, &session).await;
        } else if mode == 0 {
            for statement in [
                "UPDATE auth_security.browser_sessions SET closed_at=NULL WHERE context_id=$1",
                "UPDATE auth_security.browser_sessions SET owner_removed_at=clock_timestamp() WHERE context_id=$1",
            ] {
                // Cleared proof and revoked family cannot mask either guard.
                let error = sqlx::query(statement)
                    .bind(context)
                    .execute(&pool)
                    .await
                    .expect_err("closed custody cannot reopen or invent removal");
                assert_eq!(
                    error.as_database_error().unwrap().code().as_deref(),
                    Some("23514")
                );
                assert_eq!(effects(&pool).await, before);
                assert_eq!(
                    complete_mapping_digest(&pool, &session).await,
                    mapping_before
                );
            }
            assert_eq!(
                post_raw(f.router.clone(), LOGOUT, None, handle(&session))
                    .await
                    .status(),
                StatusCode::NO_CONTENT
            );
            assert_eq!(effects(&pool).await, before);
        } else {
            let live_family: bool = sqlx::query_scalar(
                "SELECT revoked_at IS NULL FROM auth_refresh_token_families WHERE id=$1",
            )
            .bind(family(&pool, &session).await)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(live_family, "expiry cleanup does not certify family logout");
        }
    }
}

// Preparation only; two real owner gates distinguish rollback from waiter effects.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn failed_guarded_removal_restores_custody_before_waiting_logout_commits(pool: PgPool) {
    let f = fixture(&pool, "browser-removal-rollback-owner").await;
    let audit_actor = removal_actor(&pool).await;
    let mut subject = actor(&pool, &f, "제거 롤백과 기다리는 종료").await;
    let original = login(&f, &mut subject).await;
    let later = login(&f, &mut subject).await;
    let original_family = family(&pool, &original).await;
    let immutable = immutable_mapping(&pool, &original).await;
    let original_tokens: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM auth_refresh_tokens WHERE family_id=$1 ORDER BY id")
            .bind(original_family)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(!original_tokens.is_empty());
    let original_sql = "SELECT jsonb_build_object(
        'family',(SELECT to_jsonb(f)-'revoked_at'-'revoked_reason' FROM auth_refresh_token_families f WHERE id=$1),
        'tokens',(SELECT jsonb_agg(to_jsonb(t)-'revoked_at' ORDER BY id) FROM auth_refresh_tokens t WHERE family_id=$1))";
    let original_custody: Value = sqlx::query_scalar(original_sql)
        .bind(original_family)
        .fetch_one(&pool)
        .await
        .unwrap();
    let original_digest = Sha256::digest(serde_json::to_vec(&original_custody).unwrap());
    let preserved_sql = "SELECT jsonb_build_object(
        'mapping',(SELECT jsonb_agg(to_jsonb(s) ORDER BY context_id) FROM auth_security.browser_sessions s WHERE account_id=$1 AND family_id<>$2),
        'families',(SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM auth_refresh_token_families f WHERE user_id=$1 AND id<>$2),
        'tokens',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM auth_refresh_tokens t WHERE user_id=$1 AND family_id<>$2),
        'keys',(SELECT jsonb_agg(to_jsonb(k) ORDER BY id) FROM auth_webauthn_credentials k WHERE user_id=$1))";
    let preserved: Value = sqlx::query_scalar(preserved_sql)
        .bind(*subject.user.as_uuid())
        .bind(original_family)
        .fetch_one(&pool)
        .await
        .unwrap();
    let preserved_digest = Sha256::digest(serde_json::to_vec(&preserved).unwrap());
    let logout_pool = runtime_login(&pool, false, "browser-rollback-waiting-logout").await;
    let logout_router = configured_router(logout_pool, &f.private, &f.public, Some(&f.storage_key));
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(r#"
        CREATE FUNCTION public.test_browser_removal_fail_gate() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN PERFORM pg_advisory_xact_lock(870250); RAISE EXCEPTION 'forced_removal_waiter_rollback' USING ERRCODE='23514'; END $$;
        CREATE TRIGGER test_browser_removal_fail_gate BEFORE INSERT ON audit_events FOR EACH ROW
        WHEN(NEW.action='platform.tenant.remove' AND NEW.target_id='{company}') EXECUTE FUNCTION public.test_browser_removal_fail_gate();
        CREATE FUNCTION public.test_browser_logout_second_gate() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN PERFORM pg_advisory_xact_lock(870251); RETURN NEW; END $$;
        CREATE TRIGGER test_browser_logout_second_gate BEFORE INSERT ON audit_events FOR EACH ROW
        WHEN(NEW.action='auth.logout' AND NEW.target_id='{original_family}') EXECUTE FUNCTION public.test_browser_logout_second_gate();
    "#, company=subject.org))).execute(&pool).await.unwrap();
    let before = effects(&pool).await;
    let mut first_gate = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(870250)")
        .execute(first_gate.as_mut())
        .await
        .unwrap();
    let first_pid = super::reset_sessions::transaction_pid(&mut first_gate).await;
    let mut second_gate = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(870251)")
        .execute(second_gate.as_mut())
        .await
        .unwrap();
    let second_pid = super::reset_sessions::transaction_pid(&mut second_gate).await;
    let company = subject.org;
    let removal_pool = f.runtime.clone();
    let removal =
        tokio::spawn(
            async move { remove_company(&removal_pool, false, company, audit_actor).await },
        );
    super::reset_sessions::waiting(
        &pool,
        "browser-removal-rollback-owner",
        first_pid,
        "INSERT INTO audit_events",
    )
    .await;
    let removing_pid = blocked_backend(&pool, "browser-removal-rollback-owner", first_pid).await;
    let request = handle(&original);
    let logout = tokio::spawn(async move { post_raw(logout_router, LOGOUT, None, request).await });
    super::reset_sessions::waiting(
        &pool,
        "browser-rollback-waiting-logout",
        removing_pid,
        "SELECT is_active FROM users WHERE id = $1 AND org_id = $2 FOR NO KEY UPDATE",
    )
    .await;
    assert_eq!(effects(&pool).await, before);
    first_gate.commit().await.unwrap();
    let error = timeout(WAIT, removal).await.unwrap().unwrap().unwrap_err();
    assert!(error.to_string().contains("forced_removal_waiter_rollback"));
    super::reset_sessions::waiting(
        &pool,
        "browser-rollback-waiting-logout",
        second_pid,
        "INSERT INTO audit_events",
    )
    .await;
    assert_eq!(
        effects(&pool).await,
        before,
        "complete removal rollback must precede the actual waiter's commit"
    );
    second_gate.commit().await.unwrap();
    assert_eq!(
        timeout(WAIT, logout).await.unwrap().unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert_closed_mapping(&pool, &original, immutable).await;
    let retained: (i64,i64,i64,bool,i64,i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM organizations WHERE id=$1 AND status='ACTIVE'),
                (SELECT count(*) FROM users WHERE id=$2 AND org_id=$1),
                (SELECT count(*) FROM auth_webauthn_credentials WHERE id=$3 AND user_id=$2 AND org_id=$1),
                (SELECT revoked_at IS NOT NULL AND NOT EXISTS(SELECT 1 FROM auth_refresh_tokens t WHERE t.family_id=f.id AND t.revoked_at IS NULL) FROM auth_refresh_token_families f WHERE id=$4),
                (SELECT count(*) FROM audit_events WHERE action='platform.tenant.remove' AND target_id=$1::text),
                (SELECT count(*) FROM audit_events WHERE action='auth.logout' AND target_id=$4::text)"
    ).bind(*subject.org.as_uuid()).bind(*subject.user.as_uuid()).bind(subject.source).bind(original_family)
        .fetch_one(&pool).await.unwrap();
    assert_eq!(retained, (1, 1, 1, true, 0, 1));
    let retained_tokens: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM auth_refresh_tokens WHERE family_id=$1 ORDER BY id")
            .bind(original_family)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(retained_tokens, original_tokens);
    let original_custody: Value = sqlx::query_scalar(original_sql)
        .bind(original_family)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        Sha256::digest(serde_json::to_vec(&original_custody).unwrap()),
        original_digest
    );
    let preserved: Value = sqlx::query_scalar(preserved_sql)
        .bind(*subject.user.as_uuid())
        .bind(original_family)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        Sha256::digest(serde_json::to_vec(&preserved).unwrap()),
        preserved_digest
    );
    own_history(&f, &later).await;
    let completed = effects(&pool).await;
    assert_eq!(
        post_raw(f.router.clone(), LOGOUT, None, handle(&original))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(effects(&pool).await, completed);
}

// Preparation only; complete failed-login rollback precedes a real reset waiter.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn failed_browser_login_rolls_back_before_waiting_credential_reset_commits(pool: PgPool) {
    let f = fixture(&pool, "browser-login-rollback-owner").await;
    let mut subject = actor(&pool, &f, "로그인 롤백과 기다리는 복구").await;
    give_recovery_subject_branch(&pool, &subject).await;
    let existing = login(&f, &mut subject).await;
    let mut admin = actor_in_org(&pool, &f, "독립 복구 담당자", subject.org).await;
    sqlx::query("UPDATE users SET roles=ARRAY['SUPER_ADMIN'] WHERE id=$1")
        .bind(*admin.user.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let administrator = login(&f, &mut admin).await;
    let admin_bearer = private_original_proof(&pool, &f, &administrator).await;
    let admin_sql = "SELECT jsonb_build_object(
        'mapping',(SELECT jsonb_agg(to_jsonb(s) ORDER BY context_id) FROM auth_security.browser_sessions s WHERE account_id=$1),
        'families',(SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM auth_refresh_token_families f WHERE user_id=$1),
        'tokens',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM auth_refresh_tokens t WHERE user_id=$1),
        'keys',(SELECT jsonb_agg(to_jsonb(k) ORDER BY id) FROM auth_webauthn_credentials k WHERE user_id=$1),
        'bootstrap',(SELECT jsonb_agg(to_jsonb(b) ORDER BY id) FROM auth_bootstrap_credentials b WHERE user_id=$1),
        'otp_sources',(SELECT jsonb_agg(to_jsonb(b) ORDER BY family_id) FROM auth_legacy_otp_family_sources b WHERE user_id=$1),
        'registration_bindings',(SELECT jsonb_agg(to_jsonb(b) ORDER BY ceremony_id) FROM auth_legacy_registration_bindings b WHERE user_id=$1),
        'ceremonies',(SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM auth_webauthn_ceremonies c WHERE user_id=$1
            OR id IN(SELECT ceremony_id FROM auth_legacy_registration_bindings WHERE user_id=$1)
            OR id IN(SELECT context_id FROM auth_security.browser_sessions WHERE account_id=$1)),
        'ceremony_bindings',(SELECT jsonb_agg(to_jsonb(b) ORDER BY ceremony_id) FROM auth_webauthn_ceremony_bindings b
            WHERE ceremony_id IN(SELECT id FROM auth_webauthn_ceremonies WHERE user_id=$1)
               OR ceremony_id IN(SELECT ceremony_id FROM auth_legacy_registration_bindings WHERE user_id=$1)
               OR ceremony_id IN(SELECT context_id FROM auth_security.browser_sessions WHERE account_id=$1)),
        'reservations',(SELECT to_jsonb(r) FROM auth_security.account_id_reservations r WHERE account_id=$1),
        'removals',(SELECT jsonb_agg(to_jsonb(r) ORDER BY event_id) FROM auth_security.credential_removals r WHERE account_id=$1),
        'user',(SELECT to_jsonb(u) FROM users u WHERE id=$1),
        'freshness',(SELECT to_jsonb(v) FROM subject_authz_versions v WHERE user_id=$1),
        'state',(SELECT to_jsonb(s) FROM auth_security.account_state s WHERE account_id=$1))";
    let admin_custody: Value = sqlx::query_scalar(admin_sql)
        .bind(*admin.user.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    let admin_digest = Sha256::digest(serde_json::to_vec(&admin_custody).unwrap());
    let families: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM auth_refresh_token_families WHERE user_id=$1 ORDER BY id",
    )
    .bind(*subject.user.as_uuid())
    .fetch_all(&pool)
    .await
    .unwrap();
    let tokens: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM auth_refresh_tokens WHERE user_id=$1 ORDER BY id")
            .bind(*subject.user.as_uuid())
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(!families.is_empty() && !tokens.is_empty());
    let retained_sql = "SELECT jsonb_build_object(
        'families',(SELECT jsonb_agg(to_jsonb(f)-'revoked_at'-'revoked_reason' ORDER BY id) FROM auth_refresh_token_families f WHERE user_id=$1),
        'tokens',(SELECT jsonb_agg(to_jsonb(t)-'revoked_at' ORDER BY id) FROM auth_refresh_tokens t WHERE user_id=$1))";
    let retained: Value = sqlx::query_scalar(retained_sql)
        .bind(*subject.user.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    let retained_digest = Sha256::digest(serde_json::to_vec(&retained).unwrap());
    let prior_login_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE actor=$1 AND action='auth.login'",
    )
    .bind(*subject.user.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let (ceremony, signed) = assertion(&f, &mut subject).await;
    let reset_pool = runtime_login(&pool, false, "browser-rollback-waiting-reset").await;
    let reset_router = configured_router(reset_pool, &f.private, &f.public, Some(&f.storage_key));
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(r#"
        CREATE FUNCTION public.test_browser_login_fail_gate() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN PERFORM pg_advisory_xact_lock(870248); RAISE EXCEPTION 'forced_login_waiter_rollback' USING ERRCODE='23514'; END $$;
        CREATE TRIGGER test_browser_login_fail_gate BEFORE INSERT ON audit_events FOR EACH ROW
        WHEN(NEW.action='auth.login' AND NEW.actor='{user}'::uuid) EXECUTE FUNCTION public.test_browser_login_fail_gate();
        CREATE FUNCTION public.test_browser_reset_second_gate() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN PERFORM pg_advisory_xact_lock(870249); RETURN OLD; END $$;
        CREATE TRIGGER test_browser_reset_second_gate BEFORE DELETE ON auth_webauthn_credentials FOR EACH ROW
        WHEN(OLD.id='{source}'::uuid) EXECUTE FUNCTION public.test_browser_reset_second_gate();
    "#,user=subject.user,source=subject.source))).execute(&pool).await.unwrap();
    let before = effects(&pool).await;
    let mut first_gate = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(870248)")
        .execute(first_gate.as_mut())
        .await
        .unwrap();
    let first_pid = super::reset_sessions::transaction_pid(&mut first_gate).await;
    let mut second_gate = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(870249)")
        .execute(second_gate.as_mut())
        .await
        .unwrap();
    let second_pid = super::reset_sessions::transaction_pid(&mut second_gate).await;
    let router = f.router.clone();
    let login_task = tokio::spawn(async move { post_raw(router, LOGIN, None, signed).await });
    super::reset_sessions::waiting(
        &pool,
        "browser-login-rollback-owner",
        first_pid,
        "INSERT INTO audit_events",
    )
    .await;
    let login_pid = blocked_backend(&pool, "browser-login-rollback-owner", first_pid).await;
    let user = subject.user;
    let reset_task = tokio::spawn(async move {
        post_raw(
            reset_router,
            "/api/v1/auth/admin/credential-reset",
            Some(&admin_bearer),
            json!({"user_id":user.as_uuid()}),
        )
        .await
    });
    super::reset_sessions::waiting(
        &pool,
        "browser-rollback-waiting-reset",
        login_pid,
        "SELECT is_active FROM users WHERE id = $1 AND org_id = $2 FOR NO KEY UPDATE",
    )
    .await;
    assert_eq!(effects(&pool).await, before);
    first_gate.commit().await.unwrap();
    let failed = timeout(WAIT, login_task).await.unwrap().unwrap();
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(set_cookie_values(&failed).is_empty());
    let failure = body_json(failed).await;
    for field in ["session_token", "access_token", "refresh_token"] {
        assert!(failure.get(field).is_none());
    }
    super::reset_sessions::waiting(
        &pool,
        "browser-rollback-waiting-reset",
        second_pid,
        "DELETE FROM auth_webauthn_credentials",
    )
    .await;
    assert_eq!(
        effects(&pool).await,
        before,
        "full failed-login rollback must precede real reset effects"
    );
    second_gate.commit().await.unwrap();
    let reset = timeout(WAIT, reset_task).await.unwrap().unwrap();
    assert_eq!(reset.status(), StatusCode::OK);
    let result = body_json(reset).await;
    assert_eq!(result["user_id"], subject.user.to_string());
    assert!(result["otp"].is_string() && result["expires_at"].is_string());
    let after_families: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM auth_refresh_token_families WHERE user_id=$1 ORDER BY id",
    )
    .bind(*subject.user.as_uuid())
    .fetch_all(&pool)
    .await
    .unwrap();
    let after_tokens: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM auth_refresh_tokens WHERE user_id=$1 ORDER BY id")
            .bind(*subject.user.as_uuid())
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(after_families, families);
    assert_eq!(after_tokens, tokens);
    let retained: Value = sqlx::query_scalar(retained_sql)
        .bind(*subject.user.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        Sha256::digest(serde_json::to_vec(&retained).unwrap()),
        retained_digest
    );
    let target_effects: (i64,i64,i64,bool,i64,i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM auth_refresh_token_families WHERE user_id=$1 AND revoked_at IS NULL),
                (SELECT count(*) FROM auth_refresh_tokens WHERE user_id=$1 AND revoked_at IS NULL),
                (SELECT count(*) FROM auth_webauthn_credentials WHERE user_id=$1),
                (SELECT user_id IS NULL AND consumed_at IS NULL FROM auth_webauthn_ceremonies WHERE id=$2),
                (SELECT count(*) FROM auth_security.browser_sessions WHERE context_id=$2),
                (SELECT count(*) FROM audit_events WHERE actor=$1 AND action='auth.login')"
    ).bind(*subject.user.as_uuid()).bind(ceremony).fetch_one(&pool).await.unwrap();
    assert_eq!(target_effects, (0, 0, 0, true, 0, prior_login_audits));
    let admin_custody: Value = sqlx::query_scalar(admin_sql)
        .bind(*admin.user.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        Sha256::digest(serde_json::to_vec(&admin_custody).unwrap()),
        admin_digest
    );
    own_history(&f, &administrator).await;
    assert_eq!(
        post_raw(f.router.clone(), HISTORY, None, history_handle(&existing))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

// Source preparation only; real assertion mutation, no provider substitution.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn malformed_browser_signature_preserves_custody_and_original_assertion(pool: PgPool) {
    let f = fixture(&pool, "browser-malformed-signature").await;
    let mut subject = actor(&pool, &f, "실제 서명 변조 검증").await;
    let (ceremony, original) = assertion(&f, &mut subject).await;
    let mut malformed = original.clone();
    let encoded = original["credential"]["response"]["signature"]
        .as_str()
        .unwrap();
    let mut signature = URL_SAFE_NO_PAD.decode(encoded).unwrap();
    assert!(!signature.is_empty());
    signature[0] ^= 1;
    malformed["credential"]["response"]["signature"] = json!(URL_SAFE_NO_PAD.encode(signature));
    let before = effects(&pool).await;
    let denied = post_raw(f.router.clone(), LOGIN, None, malformed).await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    assert!(set_cookie_values(&denied).is_empty());
    let failure = body_json(denied).await;
    for field in [
        "session_token",
        "access_token",
        "refresh_token",
        "account_id",
        "company_id",
    ] {
        assert!(failure.get(field).is_none());
    }
    assert_eq!(effects(&pool).await, before);
    let accepted = post_raw(f.router.clone(), LOGIN, None, original).await;
    assert_eq!(accepted.status(), StatusCode::OK);
    assert!(set_cookie_values(&accepted).is_empty());
    let session = body_json(accepted).await;
    assert_eq!(session["context_id"], ceremony.to_string());
    let proof = private_original_proof(&pool, &f, &session).await;
    own_history(&f, &session).await;
    let claims = f.verifier.verify_access_token(&proof).unwrap();
    assert!(claims.sub == subject.user.to_string() && claims.org == subject.org.to_string());
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn guarded_removal_with_actual_device_data_preserves_exact_browser_custody(pool: PgPool) {
    use console_platform_provisioning::TenantRemovalOutcome;
    let f = fixture(&pool, "browser-removal-has-data").await;
    let audit_actor = removal_actor(&pool).await;
    let mut a = actor(&pool, &f, "자료 있는 회사").await;
    let session = login(&f, &mut a).await;
    let device: Uuid = sqlx::query_scalar(
        "INSERT INTO registered_devices(user_id,device_hash,platform,app_version,last_registered_at,org_id) VALUES($1,repeat('a',64),'IOS','1',clock_timestamp(),$2) RETURNING id",
    ).bind(*a.user.as_uuid()).bind(*a.org.as_uuid()).fetch_one(&pool).await.unwrap();
    let before = effects(&pool).await;
    let mapping = complete_mapping_digest(&pool, &session).await;
    assert_eq!(
        remove_company(&f.runtime, false, a.org, audit_actor)
            .await
            .unwrap(),
        TenantRemovalOutcome::BlockedHasData
    );
    assert_eq!(effects(&pool).await, before);
    assert_eq!(complete_mapping_digest(&pool, &session).await, mapping);
    let retained: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM registered_devices WHERE id=$1 AND user_id=$2 AND org_id=$3)",
    )
    .bind(device)
    .bind(*a.user.as_uuid())
    .bind(*a.org.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(retained);
    own_history(&f, &session).await;
    assert_eq!(
        effects(&pool).await,
        before,
        "refusal and resolution preserve custody"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn committed_account_set_or_status_drift_aborts_removal_without_browser_effects(
    pool: PgPool,
) {
    use console_platform_provisioning::ProvisioningError;
    let f = fixture(&pool, "browser-removal-drift").await;
    let audit_actor = removal_actor(&pool).await;
    for force in [false, true] {
        for account_set in [false, true] {
            let mut a = actor(&pool, &f, "변경 중인 회사").await;
            let session = login(&f, &mut a).await;
            if force {
                sqlx::query("UPDATE organizations SET status='ARCHIVED' WHERE id=$1")
                    .bind(*a.org.as_uuid())
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            let rt = runtime_login(&pool, force, "browser-removal-drift-wait").await;
            let mut held = pool.begin().await.unwrap();
            sqlx::query("SELECT id FROM users WHERE id=$1 FOR NO KEY UPDATE")
                .bind(*a.user.as_uuid())
                .fetch_one(held.as_mut())
                .await
                .unwrap();
            let pid = super::reset_sessions::transaction_pid(&mut held).await;
            let org = a.org;
            let pending =
                tokio::spawn(async move { remove_company(&rt, force, org, audit_actor).await });
            wait_blocked(&pool, "browser-removal-drift-wait", pid).await;
            let new_account = if account_set {
                Some(sqlx::query_scalar::<_, Uuid>("INSERT INTO users(display_name,roles,org_id) VALUES('나중에 추가된 계정',ARRAY['MEMBER'],$1) RETURNING id")
                    .bind(*a.org.as_uuid()).fetch_one(&pool).await.unwrap())
            } else {
                sqlx::query("UPDATE organizations SET status=$2 WHERE id=$1")
                    .bind(*a.org.as_uuid())
                    .bind(if force { "ACTIVE" } else { "SUSPENDED" })
                    .execute(&pool)
                    .await
                    .unwrap();
                None
            };
            // The intentionally committed mutation belongs in the expected state;
            // capture it before releasing the witnessed Account blocker.
            let before = effects(&pool).await;
            let mapping = complete_mapping_digest(&pool, &session).await;
            held.rollback().await.unwrap();
            let error = timeout(WAIT, pending).await.unwrap().unwrap().unwrap_err();
            assert!(
                matches!(error, ProvisioningError::Sqlx(ref e) if e.as_database_error().and_then(|e|e.code()).as_deref()==Some("40001"))
            );
            assert_eq!(effects(&pool).await, before);
            assert_eq!(complete_mapping_digest(&pool, &session).await, mapping);
            if let Some(id) = new_account {
                let retained: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND org_id=$2)",
                )
                .bind(id)
                .bind(*a.org.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
                assert!(
                    retained,
                    "the remover cannot adopt or delete an intervening Account"
                );
            }
            if force || !account_set {
                // Restore live read eligibility only AFTER unchanged custody proof.
                sqlx::query("UPDATE organizations SET status='ACTIVE' WHERE id=$1")
                    .bind(*a.org.as_uuid())
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            let live = effects(&pool).await;
            own_history(&f, &session).await;
            assert_eq!(effects(&pool).await, live);
        }
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn account_and_company_fk_holders_refuse_removal_without_clearing_browser_proof(
    pool: PgPool,
) {
    use console_platform_provisioning::ProvisioningError;
    let f = fixture(&pool, "browser-removal-nowait").await;
    let audit_actor = removal_actor(&pool).await;
    for force in [false, true] {
        for company in [false, true] {
            let mut a = actor(&pool, &f, "외래 키 보유자").await;
            let session = login(&f, &mut a).await;
            if force {
                sqlx::query("UPDATE organizations SET status='ARCHIVED' WHERE id=$1")
                    .bind(*a.org.as_uuid())
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            let rt = runtime_login(&pool, force, "browser-removal-nowait-owner").await;
            let mut held = pool.begin().await.unwrap();
            if company {
                sqlx::query("SELECT id FROM organizations WHERE id=$1 FOR KEY SHARE")
                    .bind(*a.org.as_uuid())
                    .fetch_one(held.as_mut())
                    .await
                    .unwrap();
            } else {
                sqlx::query("SELECT id FROM users WHERE id=$1 FOR KEY SHARE")
                    .bind(*a.user.as_uuid())
                    .fetch_one(held.as_mut())
                    .await
                    .unwrap();
            }
            let before = effects(&pool).await;
            let mapping = complete_mapping_digest(&pool, &session).await;
            // No lock_timeout: only the owner's NOWAIT upgrade may reject this.
            let error = timeout(
                std::time::Duration::from_secs(2),
                remove_company(&rt, force, a.org, audit_actor),
            )
            .await
            .expect("removal must not wait for a foreign-key holder")
            .unwrap_err();
            assert!(
                matches!(error, ProvisioningError::Sqlx(ref e) if e.as_database_error().and_then(|e|e.code()).as_deref()==Some("55P03"))
            );
            assert_eq!(effects(&pool).await, before);
            assert_eq!(complete_mapping_digest(&pool, &session).await, mapping);
            held.rollback().await.unwrap();
            if force {
                sqlx::query("UPDATE organizations SET status='ACTIVE' WHERE id=$1")
                    .bind(*a.org.as_uuid())
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            let live = effects(&pool).await;
            own_history(&f, &session).await;
            assert_eq!(effects(&pool).await, live);
        }
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn successful_company_removal_audits_exclude_browser_proof_and_handle_material(pool: PgPool) {
    use console_platform_provisioning::TenantRemovalOutcome;
    use std::collections::BTreeSet;
    let f = fixture(&pool, "browser-removal-audit-secrecy").await;
    let audit_actor = removal_actor(&pool).await;
    for force in [false, true] {
        let mut a = actor(&pool, &f, "제거 감사 기밀성").await;
        let session = login(&f, &mut a).await;
        let original = immutable_mapping(&pool, &session).await;
        let resolved = private_original_proof(&pool, &f, &session).await;
        own_history(&f, &session).await;
        let proof: Value = sqlx::query_scalar("SELECT jsonb_build_object('token_hash',token_hash,'ciphertext',ciphertext,'nonce',nonce,'tag',tag) FROM auth_security.browser_sessions WHERE context_id=$1")
            .bind(Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap())
            .fetch_one(&pool).await.unwrap();
        let mut secrets = vec![
            session["session_token"].as_str().unwrap().to_owned(),
            resolved.clone(),
        ];
        for key in ["token_hash", "ciphertext", "nonce", "tag"] {
            let value = proof[key].as_str().unwrap();
            assert!(!value.is_empty());
            secrets.push(value.to_owned());
        }
        let slug: String = sqlx::query_scalar("SELECT slug FROM organizations WHERE id=$1")
            .bind(*a.org.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
        let before_audits: i64 =
            sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE org_id=$1")
                .bind(*a.org.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        if force {
            sqlx::query("UPDATE organizations SET status='ARCHIVED' WHERE id=$1")
                .bind(*a.org.as_uuid())
                .execute(&pool)
                .await
                .unwrap();
        }
        let rt = runtime_login(&pool, force, "browser-removal-audit-owner").await;
        assert_eq!(
            remove_company(&rt, force, a.org, audit_actor)
                .await
                .unwrap(),
            TenantRemovalOutcome::Removed
        );
        assert_eq!(immutable_mapping(&pool, &session).await, original);
        let removal: Value = sqlx::query_scalar("SELECT to_jsonb(a) FROM audit_events a WHERE target_type='organizations' AND target_id=$1 AND action=$2")
            .bind(a.org.to_string()).bind(if force { "platform.tenant.force_remove" } else { "platform.tenant.remove" })
            .fetch_one(&pool).await.unwrap();
        assert!(removal["org_id"].is_null() && removal["branch_id"].is_null());
        assert_eq!(removal["actor"], audit_actor.to_string());
        let snapshot = if force {
            &removal["before_snap"]
        } else {
            &removal["after_snap"]
        };
        let keys: BTreeSet<&str> = snapshot
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            if force {
                BTreeSet::from(["org_id", "slug", "name", "wiped"])
            } else {
                BTreeSet::from(["org_id", "slug"])
            }
        );
        assert_eq!(snapshot["org_id"], a.org.to_string());
        assert_eq!(snapshot["slug"], slug);
        assert!(
            (if force {
                &removal["after_snap"]
            } else {
                &removal["before_snap"]
            })
            .is_null()
        );
        if force {
            let wiped_keys: BTreeSet<&str> = snapshot["wiped"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                wiped_keys,
                BTreeSet::from([
                    "users",
                    "registry_customers",
                    "registry_sites",
                    "registry_equipment",
                    "work_orders",
                    "financial_rental_quotes",
                    "financial_purchase_requests",
                    "messenger_threads",
                    "evidence_media",
                    "audit_events"
                ])
            );
            assert_eq!(snapshot["wiped"]["users"], 1);
            assert_eq!(snapshot["wiped"]["audit_events"], before_audits);
            assert!(
                snapshot["wiped"]
                    .as_object()
                    .unwrap()
                    .values()
                    .all(Value::is_u64)
            );
        }
        let audits: Value = sqlx::query_scalar(
            "SELECT COALESCE(jsonb_agg(to_jsonb(a) ORDER BY id),'[]'::jsonb) FROM audit_events a",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let encoded = serde_json::to_string(&audits).unwrap();
        // Boolean-only assertions cannot print secret values on failure.
        assert!(
            secrets.iter().all(|secret| !encoded.contains(secret)),
            "audit JSON must exclude handle and proof bytes"
        );
        assert_removed_session(&pool, &f, &a, &session).await;
    }
}

fn ingress_probe(
    path: &str,
    body: &[u8],
    keys: &[&str],
    peer: Option<&str>,
    forwarded: &[&str],
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    for key in keys {
        builder = builder.header("x-console-browser-ingress", *key);
    }
    for ip in forwarded {
        builder = builder.header("x-forwarded-for", *ip);
    }
    if let Some(peer) = peer {
        builder = builder.extension(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    }
    builder.body(Body::from(body.to_vec())).unwrap()
}

fn ingress_config(f: &Fixture, key: Option<&str>) -> AppConfig {
    let mut pairs = vec![
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".into()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.into()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.into()),
        ("CONSOLE_JWT_PRIVATE_KEY_PEM", f.private.clone()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", f.public.clone()),
        ("CONSOLE_WEBAUTHN_RP_ID", "example.com".into()),
        ("CONSOLE_WEBAUTHN_RP_ORIGIN", TEST_ORIGIN.into()),
        ("CONSOLE_WEBAUTHN_RP_NAME", "Console".into()),
        ("CONSOLE_BROWSER_SESSION_KEY_HEX", f.storage_key.clone()),
        ("CONSOLE_TRUSTED_PROXY_COUNT", "1".into()),
        ("CONSOLE_TRUSTED_PROXY_CIDRS", "10.24.1.7/32".into()),
    ];
    if let Some(key) = key {
        pairs.push(("CONSOLE_BROWSER_INGRESS_KEY", key.into()));
    }
    AppConfig::from_pairs(pairs)
        .unwrap_or_else(|_| panic!("browser ingress configuration must not disable native auth"))
}

async fn ingress_state(owner: &PgPool) -> [u8; 32] {
    let rates: Value = sqlx::query_scalar("SELECT COALESCE(jsonb_agg(to_jsonb(r) ORDER BY client_key,endpoint,window_start),'[]'::jsonb) FROM auth_rate_limit r")
        .fetch_one(owner).await.unwrap();
    let mut digest = Sha256::new();
    digest.update(effects(owner).await);
    digest.update(serde_json::to_vec(&rates).unwrap());
    digest.finalize().into()
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn browser_ingress_gate_requires_single_canonical_key_and_verified_hop(pool: PgPool) {
    let f = fixture(&pool, "browser-ingress-gate").await;
    let mut a = actor(&pool, &f, "브라우저 진입 경계").await;
    let session = login(&f, &mut a).await;
    let (_, signed) = assertion(&f, &mut a).await;
    let probes = [
        (START, Vec::new()),
        (LOGIN, serde_json::to_vec(&signed).unwrap()),
        (LOGOUT, serde_json::to_vec(&handle(&session)).unwrap()),
        (HISTORY, serde_json::to_vec(&json!({"session_token":session["session_token"],"browser_context":session["context_id"],"limit":30,"offset":0})).unwrap()),
    ];
    let wire = browser_ingress_key();
    let mut wrong_bytes = URL_SAFE_NO_PAD.decode(&wire[4..]).unwrap();
    wrong_bytes[0] ^= 1;
    let wrong = format!("bi1.{}", URL_SAFE_NO_PAD.encode(wrong_bytes));
    let padded = format!("{wire}=");
    let unknown = wire.replacen("bi1.", "bi2.", 1);
    let comma = format!("{wire},{wire}");
    let spaced = format!("{wire} ");
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut noncanonical = wire.as_bytes().to_vec();
    let last = noncanonical.len() - 1;
    let index = alphabet
        .iter()
        .position(|b| *b == noncanonical[last])
        .unwrap();
    assert_eq!(index % 4, 0);
    noncanonical[last] = alphabet[index + 1];
    let noncanonical = String::from_utf8(noncanonical).unwrap();
    let before = ingress_state(&pool).await;
    for keys in [
        vec![],
        vec![wrong.as_str()],
        vec![&wire[4..]],
        vec![padded.as_str()],
        vec![unknown.as_str()],
        vec![comma.as_str()],
        vec![spaced.as_str()],
        vec![noncanonical.as_str()],
        vec![wire, wire],
    ] {
        for (path, body) in &probes {
            let response = f
                .router
                .clone()
                .oneshot(ingress_probe(
                    path,
                    body,
                    &keys,
                    Some("10.24.1.7:443"),
                    &["198.51.100.24"],
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert!(set_cookie_values(&response).is_empty());
            assert_eq!(ingress_state(&pool).await, before);
        }
    }
    for (peer, forwarded, inject_fallback) in [
        (None, vec!["198.51.100.24"], false),
        (None, vec!["198.51.100.24"], true),
        (Some("10.24.1.7:443"), vec![], false),
        (Some("203.0.113.7:443"), vec!["198.51.100.24"], false),
        (Some("10.24.1.7:443"), vec!["not-an-ip"], false),
        (
            Some("10.24.1.7:443"),
            vec!["198.51.100.24", "198.51.100.24"],
            false,
        ),
        (
            Some("10.24.1.7:443"),
            vec!["203.0.113.1, 198.51.100.24"],
            false,
        ),
    ] {
        for (path, body) in &probes {
            let mut request = ingress_probe(path, body, &[wire], peer, &forwarded);
            if inject_fallback {
                request.extensions_mut().insert(
                    console_platform_request_context::TrustedClientIp::new(
                        "198.51.100.24".parse().unwrap(),
                    ),
                );
            }
            let response = f.router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(ingress_state(&pool).await, before);
        }
    }
    for (hops, cidrs, forwarded) in [
        ("0", "", "198.51.100.24"),
        ("1", "10.24.1.8/32", "198.51.100.24"),
        ("2", "10.24.1.7/32", "198.51.100.24"),
        ("2", "10.24.1.7/32", "198.51.100.24, 203.0.113.7"),
        ("2", "10.24.1.7/32", "198.51.100.24, 10.24.1.7"),
    ] {
        let router = configured_router_with(
            f.runtime.clone(),
            &f.private,
            &f.public,
            Some(&f.storage_key),
            &[
                ("CONSOLE_TRUSTED_PROXY_COUNT", hops.into()),
                ("CONSOLE_TRUSTED_PROXY_CIDRS", cidrs.into()),
            ],
        );
        for (path, body) in &probes {
            let response = router
                .clone()
                .oneshot(ingress_probe(
                    path,
                    body,
                    &[wire],
                    Some("10.24.1.7:443"),
                    &[forwarded],
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(ingress_state(&pool).await, before);
        }
    }
    let response = f
        .router
        .clone()
        .oneshot(ingress_probe(
            LOGIN,
            &serde_json::to_vec(&signed).unwrap(),
            &[wire],
            Some("10.24.1.7:443"),
            &["198.51.100.24"],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(set_cookie_values(&response).is_empty());
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn browser_start_is_bodyless_explicit_and_shares_legacy_limits(pool: PgPool) {
    let f = fixture(&pool, "browser-start-owner").await;
    let conditional = "/api/v1/auth/passkey/login/start";
    let explicit = "/api/v1/auth/passkey/login/explicit/start";
    for path in [START, conditional, explicit] {
        let request = browser_ingress(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("x-device-id", "browser-shared-start-owner"),
        )
        .body(Body::empty())
        .unwrap();
        let response = f.router.clone().oneshot(request).await.unwrap();
        let response: Value = response.into_json(StatusCode::OK).await;
        assert_eq!(
            response
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from(["ceremony_id", "challenge", "expires_at"])
        );
        let options = &response["challenge"];
        assert_eq!(options["publicKey"]["allowCredentials"], json!([]));
        assert_eq!(options["publicKey"]["userVerification"], "required");
        assert_eq!(options["publicKey"]["rpId"], "example.com");
        if path == conditional {
            assert_eq!(options["mediation"], "conditional");
        } else {
            assert!(options.get("mediation").is_none());
        }
        let ceremony = Uuid::parse_str(response["ceremony_id"].as_str().unwrap()).unwrap();
        let row:(Option<Uuid>,Value,OffsetDateTime,Option<OffsetDateTime>) = sqlx::query_as("SELECT user_id,challenge_json,expires_at,consumed_at FROM auth_webauthn_ceremonies WHERE id=$1 AND ceremony_kind='authentication'").bind(ceremony).fetch_one(&pool).await.unwrap();
        assert_eq!(row.0, None);
        assert_eq!(&row.1, options);
        assert!(row.2 > OffsetDateTime::now_utc());
        assert_eq!(row.3, None);
        let expiry = OffsetDateTime::parse(
            response["expires_at"].as_str().unwrap(),
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        assert_eq!(
            expiry.unix_timestamp_nanos() / 1000,
            row.2.unix_timestamp_nanos() / 1000
        );
    }
    for key in [
        "ip:198.51.100.24",
        "dev:browser-shared-start-owner",
        "global",
    ] {
        let attempts:i64=sqlx::query_scalar("SELECT COALESCE(SUM(attempts),0)::bigint FROM auth_rate_limit WHERE endpoint='login_start' AND client_key=$1").bind(key).fetch_one(&pool).await.unwrap();
        assert_eq!(
            attempts, 3,
            "all three starts must share existing limiter buckets"
        );
    }
    let before = ingress_state(&pool).await;
    for body in [b"{}".as_slice(), b"null".as_slice()] {
        let response = f
            .router
            .clone()
            .oneshot(ingress_probe(
                START,
                body,
                &[browser_ingress_key()],
                Some("10.24.1.7:443"),
                &["198.51.100.24"],
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(set_cookie_values(&response).is_empty());
        assert_eq!(ingress_state(&pool).await, before);
    }
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn browser_ingress_configuration_and_debug_never_expose_service_credentials(pool: PgPool) {
    async fn assert_unmatched(router: &axum::Router) {
        for path in [
            "/api/v1/auth/browser-session/not-a-route",
            "/api/v1/hr/browser-session/not-a-route",
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert!(set_cookie_values(&response).is_empty());
        }
    }
    let f = fixture(&pool, "browser-ingress-configuration").await;
    let wire = browser_ingress_key();
    let raw = &wire[4..];
    let bytes = URL_SAFE_NO_PAD.decode(raw).unwrap();
    assert_eq!(raw.len(), 43);
    assert_eq!(wire.len(), 47);
    assert_eq!(bytes.len(), 32);
    let config = ingress_config(&f, Some(raw));
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    for debug in [
        format!("{config:?}"),
        format!("{:?}", config.auth_rest.as_ref().unwrap()),
    ] {
        for secret in [
            raw.to_owned(),
            wire.to_owned(),
            hex.clone(),
            format!("{bytes:?}"),
        ] {
            assert!(
                !debug.contains(&secret),
                "configuration Debug must redact ingress credentials"
            );
        }
    }
    let valid = build_router(
        AppState::new(config, DatabaseDependency::Postgres(f.runtime.clone())).unwrap(),
    );
    assert_unmatched(&valid).await;
    let response = valid
        .oneshot(ingress_probe(
            START,
            &[],
            &[wire],
            Some("10.24.1.7:443"),
            &["198.51.100.24"],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let padded = format!("{raw}=");
    let short = URL_SAFE_NO_PAD.encode([0_u8; 31]);
    let invalid = format!("invalid-{raw}");
    for key in [
        None,
        Some(""),
        Some(wire),
        Some(padded.as_str()),
        Some(short.as_str()),
        Some(invalid.as_str()),
    ] {
        let config = ingress_config(&f, key);
        let debug = format!("{config:?}");
        if let Some(value) = key.filter(|v| !v.is_empty()) {
            assert!(
                !debug.contains(value),
                "invalid credential configuration must also be redacted"
            );
        }
        assert!(
            !debug.contains(raw),
            "configuration must not retain plaintext ingress material"
        );
        let router = build_router(
            AppState::new(config, DatabaseDependency::Postgres(f.runtime.clone())).unwrap(),
        );
        assert_unmatched(&router).await;
        for path in [START, LOGIN, LOGOUT, HISTORY] {
            let body = if path == START {
                &[][..]
            } else {
                b"{}".as_slice()
            };
            let response = router
                .clone()
                .oneshot(ingress_probe(
                    path,
                    body,
                    &[wire],
                    Some("10.24.1.7:443"),
                    &["198.51.100.24"],
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert!(set_cookie_values(&response).is_empty());
        }
        for path in [
            "/api/v1/auth/passkey/login/start",
            "/api/v1/auth/passkey/login/explicit/start",
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
    }
}

// Proposed additive tests for R7c; not executed or independently approved.
// Append this module to browser_sessions.rs after root review.
// Existing API imports only: missing routes, rather than missing Rust symbols,
// can therefore remain behavioral RED. Fixtures are disposable test facts.
mod r7_native_additions {
    use super::*;
    use console_platform_auth::{
        AccessClaims, AccessTokenInput, ActorSession, JwtIssuer, PasskeyRegistrationStart,
        PasskeyService, RefreshTokenStore, WebauthnSettings,
    };
    use openssl::symm::{Cipher, decrypt_aead, encrypt_aead};

    const MAX_PROOF: usize = 1_048_576;
    const MAX_LOGIN_BODY: usize = 2 * 1024 * 1024;

    // No Debug implementation: these structures include sealed proof material.
    #[derive(Clone)]
    struct ProofRow {
        account: Uuid,
        company: Uuid,
        family: Uuid,
        source: Uuid,
        context: Uuid,
        expires: OffsetDateTime,
        hash: Vec<u8>,
        codec: i32,
        ciphertext: Vec<u8>,
        nonce: Vec<u8>,
        tag: Vec<u8>,
        handle: String,
    }

    impl ProofRow {
        fn response(&self) -> Value {
            json!({"context_id":self.context,"session_token":self.handle,
                "expires_at":self.expires.format(&time::format_description::well_known::Rfc3339).unwrap()})
        }
        fn aad(&self) -> Vec<u8> {
            // Test implementation is independent of the future production helper.
            let mut aad = Vec::with_capacity(125);
            aad.extend_from_slice(&29_u32.to_be_bytes());
            aad.extend_from_slice(b"console/browser-session-proof");
            aad.extend_from_slice(&1_u32.to_be_bytes());
            for id in [
                self.account,
                self.company,
                self.family,
                self.source,
                self.context,
            ] {
                aad.extend_from_slice(id.as_bytes());
            }
            aad.extend_from_slice(&self.expires.unix_timestamp().to_be_bytes());
            assert_eq!(aad.len(), 125);
            aad
        }
        fn seal(&mut self, f: &Fixture, plaintext: &[u8]) {
            let key = hex::decode(&f.storage_key).unwrap();
            self.tag = vec![0; 16];
            let aad = self.aad();
            self.ciphertext = encrypt_aead(
                Cipher::aes_256_gcm(),
                &key,
                Some(&self.nonce),
                &aad,
                plaintext,
                &mut self.tag,
            )
            .unwrap();
        }
    }

    async fn stored(owner: &PgPool, session: &Value) -> ProofRow {
        let context = Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap();
        type StoredMutationRow = (
            Uuid,
            Uuid,
            Uuid,
            Uuid,
            Uuid,
            OffsetDateTime,
            Vec<u8>,
            i32,
            Vec<u8>,
            Vec<u8>,
            Vec<u8>,
        );
        let row: StoredMutationRow =
            sqlx::query_as("SELECT account_id,company_id,family_id,source_credential_id,context_id,expires_at,token_hash,codec_version,ciphertext,nonce,tag FROM auth_security.browser_sessions WHERE context_id=$1")
                .bind(context).fetch_one(owner).await.unwrap();
        ProofRow {
            account: row.0,
            company: row.1,
            family: row.2,
            source: row.3,
            context: row.4,
            expires: row.5,
            hash: row.6,
            codec: row.7,
            ciphertext: row.8,
            nonce: row.9,
            tag: row.10,
            handle: session["session_token"].as_str().unwrap().to_owned(),
        }
    }

    fn signer(f: &Fixture) -> JwtIssuer {
        JwtIssuer::from_es256_pem(
            JwtSettings {
                issuer: TEST_ISSUER.into(),
                audience: TEST_AUDIENCE.into(),
                access_token_ttl: Duration::minutes(15),
            },
            f.private.as_bytes(),
            f.public.as_bytes(),
        )
        .unwrap()
    }

    fn sign_claims(f: &Fixture, claims: &AccessClaims) -> String {
        jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256),
            claims,
            &jsonwebtoken::EncodingKey::from_ec_pem(f.private.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    async fn legacy_signed(f: &Fixture, a: &mut Actor) -> (Uuid, Value) {
        let start: LoginStartResponse = post_json(
            f.router.clone(),
            "/api/v1/auth/passkey/login/start",
            None,
            json!({}),
            StatusCode::OK,
        )
        .await;
        let challenge = start.challenge;
        let credential = a
            .authenticator
            .do_authentication(Url::parse(TEST_ORIGIN).unwrap(), challenge)
            .unwrap();
        (
            start.ceremony_id,
            json!({"ceremony_id":start.ceremony_id,"credential":credential}),
        )
    }

    async fn legacy_login(f: &Fixture, a: &mut Actor) -> (Uuid, Value) {
        let (context, signed) = legacy_signed(f, a).await;
        let response = post_raw(
            f.router.clone(),
            "/api/v1/auth/passkey/login/finish",
            None,
            signed,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        (context, body_json(response).await)
    }

    // Genuine proof consumption and actual refresh owner create this fresh
    // transaction. The wrong sealed content added by tests is an explicit
    // storage fault, not a claim that production legitimately issued it.
    async fn fresh_fixture(
        pool: &PgPool,
        f: &Fixture,
        a: &mut Actor,
        template: &AccessClaims,
    ) -> (
        sqlx::Transaction<'static, sqlx::Postgres>,
        ProofRow,
        AccessClaims,
    ) {
        let (context, signed) = assertion(f, a).await;
        let service = PasskeyService::new(WebauthnSettings {
            rp_id: "example.com".into(),
            rp_origin: Url::parse(TEST_ORIGIN).unwrap(),
            rp_name: "Console".into(),
            extra_allowed_origins: vec![],
            ceremony_ttl: Duration::minutes(5),
        })
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let outcome = service
            .finish_authentication_in_tx(
                &mut tx,
                context,
                serde_json::from_value(signed["credential"].clone()).unwrap(),
            )
            .await
            .unwrap();
        assert!(
            outcome.user_id == *a.user.as_uuid()
                && outcome.org_id == a.org
                && outcome.passkey_id == a.source
        );
        sqlx::query("SELECT pg_catalog.set_config('app.current_org',$1,true)")
            .bind(a.org.to_string())
            .execute(tx.as_mut())
            .await
            .unwrap();
        let (family, audit) = RefreshTokenStore
            .issue_family_in_tx(
                &mut tx,
                outcome.user_id,
                outcome.org_id,
                OffsetDateTime::now_utc(),
                Duration::days(30),
            )
            .await
            .unwrap();
        console_platform_db::insert_audit_event(&mut tx, &audit)
            .await
            .unwrap();
        let mut claims = template.clone();
        claims.sub = a.user.to_string();
        claims.org = a.org.to_string();
        claims.session_family_id = Some(family.family_id);
        claims.iat = family.family_created_at.unix_timestamp();
        claims.nbf = claims.iat;
        claims.exp = (family.family_created_at + Duration::minutes(15)).unix_timestamp();
        claims.jti = Uuid::new_v4().to_string();
        let mut raw_handle = [0_u8; 32];
        OsRng.fill_bytes(&mut raw_handle);
        let handle = format!("bs1.{}", URL_SAFE_NO_PAD.encode(raw_handle));
        let mut nonce = vec![0_u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let mut row = ProofRow {
            account: outcome.user_id,
            company: *outcome.org_id.as_uuid(),
            family: family.family_id,
            source: outcome.passkey_id,
            context,
            expires: OffsetDateTime::from_unix_timestamp(claims.exp).unwrap(),
            hash: Sha256::digest(handle.as_bytes()).to_vec(),
            codec: 1,
            ciphertext: vec![],
            nonce,
            tag: vec![],
            handle,
        };
        row.seal(f, sign_claims(f, &claims).as_bytes());
        (tx, row, claims)
    }

    async fn insert_scoped(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        row: &ProofRow,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT public.platform_browser_session_insert($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
            .bind(row.company)
            .bind(row.account)
            .bind(row.family)
            .bind(row.source)
            .bind(row.context)
            .bind(&row.hash)
            .bind(row.expires)
            .bind(&row.ciphertext)
            .bind(&row.nonce)
            .bind(&row.tag)
            .execute(tx.as_mut())
            .await
            .map(|_| ())
    }

    async fn insert_privileged_fault(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        row: &ProofRow,
    ) {
        sqlx::query("INSERT INTO auth_security.browser_sessions(token_hash,account_id,company_id,family_id,source_credential_id,context_id,expires_at,codec_version,ciphertext,nonce,tag) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
            .bind(&row.hash).bind(row.account).bind(row.company).bind(row.family)
            .bind(row.source).bind(row.context).bind(row.expires).bind(row.codec)
            .bind(&row.ciphertext).bind(&row.nonce).bind(&row.tag)
            .execute(tx.as_mut()).await.unwrap();
    }

    async fn deny_scoped_insert(
        mut tx: sqlx::Transaction<'static, sqlx::Postgres>,
        row: &ProofRow,
    ) {
        let attempt = insert_scoped(&mut tx, row).await;
        if attempt.is_ok() {
            // Use the allowed exact id-only locator: a wrong-Company insert
            // must not be masked by the scoped reader correctly denying it.
            let found: Option<Uuid> =
                sqlx::query_scalar("SELECT public.platform_browser_session_company($1,$2)")
                    .bind(&row.hash)
                    .bind(row.context)
                    .fetch_one(tx.as_mut())
                    .await
                    .unwrap();
            assert!(
                found.is_none(),
                "ineligible fixture must not create custody"
            );
        }
        tx.rollback().await.unwrap();
    }

    fn padded_json(body: &Value, n: usize) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(body).unwrap();
        assert!(bytes.len() <= n);
        bytes.resize(n, b' ');
        bytes
    }

    async fn raw_browser(f: &Fixture, path: &str, bytes: Vec<u8>) -> http::Response<Body> {
        f.router
            .clone()
            .oneshot(
                browser_ingress(Request::builder().method("POST").uri(path))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(bytes))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn raw_browser_ip(
        f: &Fixture,
        path: &str,
        bytes: Vec<u8>,
        ip: &str,
    ) -> http::Response<Body> {
        let mut request = browser_ingress(Request::builder().method("POST").uri(path))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(bytes))
            .unwrap();
        // Replace the one field; do not append duplicates or manufacture the
        // ingress owner's private provenance extension. Independent test peers
        // keep unrelated shape probes below the real shared ten/IP limiter.
        request
            .headers_mut()
            .insert("x-forwarded-for", ip.parse().unwrap());
        f.router.clone().oneshot(request).await.unwrap()
    }

    async fn assertion_ip(f: &Fixture, a: &mut Actor, ip: &str) -> (Uuid, Value) {
        let response = raw_browser_ip(f, START, vec![], ip).await;
        let start: LoginStartResponse = response.into_json(StatusCode::OK).await;
        let challenge = start.challenge;
        let proof = a
            .authenticator
            .do_authentication(Url::parse(TEST_ORIGIN).unwrap(), challenge)
            .unwrap();
        (
            start.ceremony_id,
            json!({"ceremony_id":start.ceremony_id,"credential":proof}),
        )
    }

    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn browser_native_proof_matches_independent_aad_and_standard_aes_vectors(pool: PgPool) {
        let golden = ProofRow {
            account: Uuid::parse_str("00010203-0405-0607-0809-0a0b0c0d0e0f").unwrap(),
            company: Uuid::parse_str("10111213-1415-1617-1819-1a1b1c1d1e1f").unwrap(),
            family: Uuid::parse_str("20212223-2425-2627-2829-2a2b2c2d2e2f").unwrap(),
            source: Uuid::parse_str("30313233-3435-3637-3839-3a3b3c3d3e3f").unwrap(),
            context: Uuid::parse_str("40414243-4445-4647-4849-4a4b4c4d4e4f").unwrap(),
            expires: OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap(),
            hash: vec![0; 32],
            codec: 1,
            ciphertext: vec![],
            nonce: (0_u8..12).collect(),
            tag: vec![],
            handle: String::new(),
        };
        let literal = hex::decode(concat!(
            "0000001d636f6e736f6c652f62726f777365722d73657373696f6e2d70726f6f6600000001",
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            "202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f",
            "404142434445464748494a4b4c4d4e4f000000006b49d200"
        ))
        .unwrap();
        assert_eq!(golden.aad(), literal);
        let mut tag = [0_u8; 16];
        let key: Vec<u8> = (0_u8..32).collect();
        let ciphertext = encrypt_aead(
            Cipher::aes_256_gcm(),
            &key,
            Some(&golden.nonce),
            &literal,
            b"example.signed.proof",
            &mut tag,
        )
        .unwrap();
        assert_eq!(
            ciphertext,
            hex::decode("227ab776b589a735fe28f0e5d48d561df1b9e852").unwrap()
        );
        assert_eq!(
            tag.as_slice(),
            hex::decode("afbeaa6a75e8f86c0b15c192482ac59e").unwrap()
        );
        let zero_cipher = encrypt_aead(
            Cipher::aes_256_gcm(),
            &[0; 32],
            Some(&[0; 12]),
            &[],
            &[0; 16],
            &mut tag,
        )
        .unwrap();
        assert_eq!(
            zero_cipher,
            hex::decode("cea7403d4d606b6e074ec5d3baf39d18").unwrap()
        );
        assert_eq!(
            tag.as_slice(),
            hex::decode("d0d1c8a799996bf0265b98b5d48ab919").unwrap()
        );

        let f = fixture(&pool, "r7-native-golden").await;
        let mut a = actor(&pool, &f, "원본 증명 골든").await;
        let session = login(&f, &mut a).await;
        let row = stored(&pool, &session).await;
        assert!(
            row.codec == 1 && row.hash.len() == 32 && row.nonce.len() == 12 && row.tag.len() == 16
        );
        let key = hex::decode(&f.storage_key).unwrap();
        let plaintext = decrypt_aead(
            Cipher::aes_256_gcm(),
            &key,
            Some(&row.nonce),
            &row.aad(),
            &row.ciphertext,
            &row.tag,
        )
        .unwrap();
        let claims = f
            .verifier
            .verify_access_token(std::str::from_utf8(&plaintext).unwrap())
            .unwrap();
        assert!(
            claims.sub == a.user.to_string()
                && claims.org == a.org.to_string()
                && claims.session_family_id == Some(row.family)
                && claims.exp == row.expires.unix_timestamp()
        );
        assert!(
            row.source == a.source
                && row.context == Uuid::parse_str(session["context_id"].as_str().unwrap()).unwrap()
        );
        assert!(row.hash == Sha256::digest(row.handle.as_bytes()).to_vec());
        let mut independent = row.clone();
        independent.seal(&f, &plaintext);
        assert!(independent.ciphertext == row.ciphertext && independent.tag == row.tag);
        own_history(&f, &session).await;
    }

    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn browser_large_current_branch_proof_is_read_without_jwt_http_transport(pool: PgPool) {
        let f = fixture(&pool, "r7-large-branches").await;
        let mut a = actor(&pool, &f, "큰 지점 범위").await;
        let region: Uuid =
            sqlx::query_scalar("INSERT INTO regions(name,org_id) VALUES($1,$2) RETURNING id")
                .bind(format!("r7-{}", Uuid::new_v4()))
                .bind(*a.org.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query("INSERT INTO branches(region_id,name,org_id) SELECT $1,'지점 '||g::text,$2 FROM generate_series(1,3000) AS g")
            .bind(region).bind(*a.org.as_uuid()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO user_branches(user_id,branch_id,org_id) SELECT $1,id,org_id FROM branches WHERE region_id=$2 AND org_id=$3")
            .bind(*a.user.as_uuid()).bind(region).bind(*a.org.as_uuid()).execute(&pool).await.unwrap();
        let session = login(&f, &mut a).await;
        let proof = private_original_proof(&pool, &f, &session).await;
        assert!(proof.len() > 65_536 && proof.len() <= MAX_PROOF);
        let claims = f.verifier.verify_access_token(&proof).unwrap();
        let actual: Vec<Uuid> = sqlx::query_scalar(
            "SELECT branch_id FROM user_branches WHERE user_id=$1 AND org_id=$2 ORDER BY branch_id",
        )
        .bind(*a.user.as_uuid())
        .bind(*a.org.as_uuid())
        .fetch_all(&pool)
        .await
        .unwrap();
        let signed: std::collections::BTreeSet<Uuid> = claims
            .branches
            .iter()
            .map(|id| Uuid::parse_str(id).unwrap())
            .collect();
        let actual_set: std::collections::BTreeSet<Uuid> = actual.into_iter().collect();
        assert_eq!(signed, actual_set);
        assert_eq!(signed.len(), 3000);
        assert_eq!(claims.branches.len(), 3000);
        let mut body = handle(&session);
        body["limit"] = json!(25);
        body["offset"] = json!(0);
        let request = browser_ingress(Request::builder().method("POST").uri(HISTORY))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        assert!(!request.headers().contains_key(header::AUTHORIZATION));
        let before = effects(&pool).await;
        let response = f.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        assert!(
            !bytes
                .windows(proof.len())
                .any(|window| window == proof.as_bytes())
        );
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            result
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from([
                "context",
                "history",
                "browser_context",
                "expires_at"
            ])
        );
        assert_eq!(result["context"]["company_id"], a.org.to_string());
        assert_eq!(result["context"]["account_display_name"], a.name);
        assert_eq!(effects(&pool).await, before);
    }

    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn browser_nearest_signed_cap_rolls_back_atomically_without_changing_legacy_cap(
        pool: PgPool,
    ) {
        let f = fixture(&pool, "r7-signed-cap").await;
        let mut a = actor(&pool, &f, "서명 크기 경계").await;
        let first = login(&f, &mut a).await;
        let template = f
            .verifier
            .verify_access_token(&private_original_proof(&pool, &f, &first).await)
            .unwrap();
        let issuer = signer(&f);
        let issued_at = OffsetDateTime::now_utc();
        let issue_len = |n: usize| {
            issuer
                .issue_session_access_token(
                    AccessTokenInput {
                        subject: a.user,
                        org_id: a.org,
                        roles: template.roles.clone(),
                        branches: template
                            .branches
                            .iter()
                            .map(|id| BranchId::from_uuid(Uuid::parse_str(id).unwrap()))
                            .collect(),
                        platform: false,
                        view_as: false,
                        read_only: false,
                        display_name: Some("n".repeat(n)),
                        feature_grants: template.feature_grants.clone(),
                        authz_subject_version: template.authz_subject_version,
                        authz_policy_version: template.authz_policy_version,
                        session_generation: template.session_generation,
                        issued_at,
                    },
                    None,
                    template.group_roles.clone(),
                    template.session_family_id.unwrap(),
                    issued_at + Duration::hours(1),
                )
                .unwrap()
        };
        let (mut lo, mut hi) = (0_usize, MAX_PROOF + 1);
        assert!(issue_len(lo).len() <= MAX_PROOF && issue_len(hi).len() > MAX_PROOF);
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if issue_len(mid).len() <= MAX_PROOF {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let below = issue_len(lo);
        let above = issue_len(hi);
        assert!(below.len() <= MAX_PROOF && above.len() > MAX_PROOF && hi == lo + 1);
        eprintln!(
            "r7_signed_cap below_name_bytes={lo} below_jwt_bytes={} above_name_bytes={hi} above_jwt_bytes={}",
            below.len(),
            above.len()
        );
        sqlx::query("UPDATE users SET display_name=$1 WHERE id=$2 AND org_id=$3")
            .bind("n".repeat(lo))
            .bind(*a.user.as_uuid())
            .bind(*a.org.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
        let admitted = login(&f, &mut a).await;
        let admitted_proof = private_original_proof(&pool, &f, &admitted).await;
        assert_eq!(admitted_proof.len(), below.len());
        let admitted_claims = f.verifier.verify_access_token(&admitted_proof).unwrap();
        assert!(admitted_claims.name.as_deref() == Some("n".repeat(lo).as_str()));
        sqlx::query("UPDATE users SET display_name=$1 WHERE id=$2 AND org_id=$3")
            .bind("n".repeat(hi))
            .bind(*a.user.as_uuid())
            .bind(*a.org.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
        let (context, signed) = assertion(&f, &mut a).await;
        let key_before: Value =
            sqlx::query_scalar("SELECT to_jsonb(k) FROM auth_webauthn_credentials k WHERE id=$1")
                .bind(a.source)
                .fetch_one(&pool)
                .await
                .unwrap();
        let before = effects(&pool).await;
        let denied = post_raw(f.router.clone(), LOGIN, None, signed.clone()).await;
        assert_eq!(denied.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(set_cookie_values(&denied).is_empty());
        assert_eq!(effects(&pool).await, before);
        let unused: bool = sqlx::query_scalar(
            "SELECT consumed_at IS NULL FROM auth_webauthn_ceremonies WHERE id=$1",
        )
        .bind(context)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(unused);
        let key_after: Value =
            sqlx::query_scalar("SELECT to_jsonb(k) FROM auth_webauthn_credentials k WHERE id=$1")
                .bind(a.source)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            key_before == key_after,
            "over-cap rollback includes counters and last_used_at"
        );
        sqlx::query("UPDATE users SET display_name=$1 WHERE id=$2 AND org_id=$3")
            .bind(&a.name)
            .bind(*a.user.as_uuid())
            .bind(*a.org.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
        let retry = post_raw(f.router.clone(), LOGIN, None, signed.clone()).await;
        assert_eq!(retry.status(), StatusCode::OK);
        let recovered = body_json(retry).await;
        assert_eq!(recovered["context_id"], context.to_string());
        own_history(&f, &recovered).await;
        let committed = effects(&pool).await;
        assert_eq!(
            post_raw(f.router.clone(), LOGIN, None, signed)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(effects(&pool).await, committed);
        sqlx::query("UPDATE users SET display_name=$1 WHERE id=$2 AND org_id=$3")
            .bind("n".repeat(hi))
            .bind(*a.user.as_uuid())
            .bind(*a.org.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
        let (_, legacy) = legacy_login(&f, &mut a).await;
        let token = legacy["access_token"].as_str().unwrap();
        assert_eq!(token.len(), above.len());
        assert!(token.len() > MAX_PROOF && f.verifier.verify_access_token(token).is_ok());
        assert_eq!(
            get(&f, "/api/v1/hr/attendance-records/me", token)
                .await
                .status(),
            StatusCode::OK
        );
    }

    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn browser_valid_aead_wrong_signed_claims_are_denied_without_effect(pool: PgPool) {
        let f = fixture(&pool, "r7-wrong-signed-proof").await;
        let mut a = actor(&pool, &f, "바인딩 주체").await;
        let mut b = actor(&pool, &f, "다른 서명 주체").await;
        let own = login(&f, &mut a).await;
        let other = login(&f, &mut b).await;
        let alternative = login(&f, &mut a).await;
        let template = f
            .verifier
            .verify_access_token(&private_original_proof(&pool, &f, &own).await)
            .unwrap();
        let other_claims = f
            .verifier
            .verify_access_token(&private_original_proof(&pool, &f, &other).await)
            .unwrap();
        let alternative_claims = f
            .verifier
            .verify_access_token(&private_original_proof(&pool, &f, &alternative).await)
            .unwrap();
        for case in 0..7 {
            if case == 6 {
                // Explicit INSERT-time family storage fault. Tokens and the
                // real owner audit are still created normally in the same tx.
                sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE FUNCTION public.r7_test_family_provenance() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$ BEGIN IF NEW.user_id='{}'::pg_catalog.uuid THEN NEW.provenance_version:=1; NEW.auth_generation:=1; NEW.session_purpose:='normal'; NEW.source_kind:='passkey'; NEW.source_operation_id:=pg_catalog.gen_random_uuid(); END IF; RETURN NEW; END $$; CREATE TRIGGER r7_test_family_provenance BEFORE INSERT ON public.auth_refresh_token_families FOR EACH ROW EXECUTE FUNCTION public.r7_test_family_provenance();",a.user)))
                    .execute(&pool).await.unwrap();
            }
            let (mut tx, mut row, mut claims) = fresh_fixture(&pool, &f, &mut a, &template).await;
            match case {
                0 => claims.sub = other_claims.sub.clone(),
                1 => claims.org = other_claims.org.clone(),
                2 => claims.session_family_id = alternative_claims.session_family_id,
                3 => claims.exp -= 1,
                4 => claims.platform = true,
                5 => {
                    claims.view_as = true;
                    claims.read_only = true;
                    claims.exp = claims.exp.min(alternative_claims.exp);
                    claims.actor_session = Some(ActorSession {
                        family_id: alternative_claims.session_family_id.unwrap(),
                        expires_at: alternative_claims.exp,
                        home_org: a.org,
                        subject_version: alternative_claims.authz_subject_version,
                        session_generation: alternative_claims.session_generation,
                    });
                }
                6 => {}
                _ => unreachable!(),
            }
            let wrong = sign_claims(&f, &claims);
            assert!(
                f.verifier.verify_access_token(&wrong).is_ok(),
                "negative must retain a genuine valid signature"
            );
            row.seal(&f, wrong.as_bytes());
            // Explicit privileged INSERT-time sealed storage fault; accepted
            // mappings and production guards are never rewritten or disabled.
            insert_privileged_fault(&mut tx, &row).await;
            tx.commit().await.unwrap();
            if case == 6 {
                sqlx::raw_sql("DROP TRIGGER r7_test_family_provenance ON public.auth_refresh_token_families; DROP FUNCTION public.r7_test_family_provenance();")
                    .execute(&pool).await.unwrap();
                let state: (i16,i64)=sqlx::query_as("SELECT f.provenance_version,(SELECT count(*) FROM public.auth_refresh_tokens t WHERE t.family_id=f.id) FROM public.auth_refresh_token_families f WHERE f.id=$1")
                    .bind(row.family).fetch_one(&pool).await.unwrap();
                assert_eq!(state, (1, 1));
            }
            let before = effects(&pool).await;
            let response = history_raw(&f, &row.response(), json!({"limit":25,"offset":0})).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert!(set_cookie_values(&response).is_empty());
            assert_eq!(effects(&pool).await, before);
        }
        own_history(&f, &own).await;
        own_history(&f, &other).await;
        own_history(&f, &alternative).await;
    }

    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn browser_sql_insert_requires_fresh_ceremony_and_family_in_one_transaction(
        pool: PgPool,
    ) {
        let f = fixture(&pool, "r7-sql-freshness").await;
        let mut a = actor(&pool, &f, "새 증명 전용").await;
        let (old_context, legacy) = legacy_login(&f, &mut a).await;
        let old_claims = f
            .verifier
            .verify_access_token(legacy["access_token"].as_str().unwrap())
            .unwrap();
        let absent: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM auth_security.browser_sessions WHERE context_id=$1",
        )
        .bind(old_context)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            absent, 0,
            "old consumed ceremony must not be masked by a duplicate mapping"
        );
        let (mut tx, row, claims) = fresh_fixture(&f.runtime, &f, &mut a, &old_claims).await;
        let ceremony_shape: (bool,bool,bool)=sqlx::query_as("SELECT user_id IS NULL,consumed_at IS NOT NULL,xmin=pg_catalog.pg_current_xact_id()::pg_catalog.xid FROM auth_webauthn_ceremonies WHERE id=$1")
            .bind(row.context).fetch_one(tx.as_mut()).await.unwrap();
        assert_eq!(ceremony_shape, (true, true, true));
        let family_fresh: bool=sqlx::query_scalar("SELECT xmin=pg_catalog.pg_current_xact_id()::pg_catalog.xid FROM auth_refresh_token_families WHERE id=$1")
            .bind(row.family).fetch_one(tx.as_mut()).await.unwrap();
        assert!(family_fresh);
        insert_scoped(&mut tx, &row).await.unwrap();
        tx.commit().await.unwrap();
        own_history(&f, &row.response()).await;
        assert!(claims.session_family_id == Some(row.family));
        for stale_ceremony in [true, false] {
            let (tx, mut candidate, mut candidate_claims) =
                fresh_fixture(&f.runtime, &f, &mut a, &old_claims).await;
            if stale_ceremony {
                candidate.context = old_context;
            } else {
                candidate.family = old_claims.session_family_id.unwrap();
                candidate.expires = OffsetDateTime::from_unix_timestamp(old_claims.exp).unwrap();
                candidate_claims.session_family_id = Some(candidate.family);
                candidate_claims.exp = old_claims.exp;
            }
            candidate.seal(&f, sign_claims(&f, &candidate_claims).as_bytes());
            deny_scoped_insert(tx, &candidate).await;
        }
        // Unconsumed ceremony with an eligible new family. This is a real
        // pending authentication, not fabricated successful verification.
        let (context, _signed) = assertion(&f, &mut a).await;
        let mut tx = f.runtime.begin().await.unwrap();
        sqlx::query("SELECT pg_catalog.set_config('app.current_org',$1,true)")
            .bind(a.org.to_string())
            .execute(tx.as_mut())
            .await
            .unwrap();
        let (family, audit) = RefreshTokenStore
            .issue_family_in_tx(
                &mut tx,
                *a.user.as_uuid(),
                a.org,
                OffsetDateTime::now_utc(),
                Duration::days(30),
            )
            .await
            .unwrap();
        console_platform_db::insert_audit_event(&mut tx, &audit)
            .await
            .unwrap();
        let mut candidate = row.clone();
        candidate.context = context;
        candidate.family = family.family_id;
        let mut raw = [0_u8; 32];
        OsRng.fill_bytes(&mut raw);
        candidate.handle = format!("bs1.{}", URL_SAFE_NO_PAD.encode(raw));
        candidate.hash = Sha256::digest(candidate.handle.as_bytes()).to_vec();
        let mut candidate_claims = old_claims.clone();
        candidate_claims.session_family_id = Some(candidate.family);
        candidate_claims.exp = candidate.expires.unix_timestamp();
        candidate.seal(&f, sign_claims(&f, &candidate_claims).as_bytes());
        deny_scoped_insert(tx, &candidate).await;

        // Shape-valid privileged corruption of a fresh, uncommitted ceremony:
        // consumption is now after its recorded expiry. No accepted mapping or
        // guard is changed, and both xmin values still belong to this tx.
        let (mut tx, expired, _) = fresh_fixture(&pool, &f, &mut a, &old_claims).await;
        sqlx::query("UPDATE public.auth_webauthn_ceremonies SET created_at=consumed_at-interval '2 seconds',expires_at=consumed_at-interval '1 second' WHERE id=$1")
            .bind(expired.context).execute(tx.as_mut()).await.unwrap();
        let fresh_expired: bool = sqlx::query_scalar("SELECT c.created_at<c.expires_at AND c.expires_at<c.consumed_at AND c.xmin=pg_current_xact_id()::xid AND f.xmin=pg_current_xact_id()::xid FROM auth_webauthn_ceremonies c JOIN auth_refresh_token_families f ON f.id=$2 WHERE c.id=$1")
            .bind(expired.context).bind(expired.family).fetch_one(tx.as_mut()).await.unwrap();
        assert!(
            fresh_expired,
            "genuine fresh consumed ceremony must isolate expiry without violating its creation constraint"
        );
        deny_scoped_insert(tx, &expired).await;

        // Actual registration proof consumed in the same transaction as the
        // new family isolates the forbidden ceremony kind without a stale-xmin
        // or unconsumed-ceremony shortcut.
        let service = PasskeyService::new(WebauthnSettings {
            rp_id: "example.com".into(),
            rp_origin: Url::parse(TEST_ORIGIN).unwrap(),
            rp_name: "Console".into(),
            extra_allowed_origins: vec![],
            ceremony_ttl: Duration::minutes(5),
        })
        .unwrap();
        let registration = service
            .start_discoverable_registration(
                &f.runtime,
                a.org,
                PasskeyRegistrationStart {
                    user_id: *a.user.as_uuid(),
                    username: "kind-probe".into(),
                    display_name: a.name.clone(),
                },
            )
            .await
            .unwrap();
        let mut authenticator = ResidentAuthenticator::new().expect("resident browser fixture");
        let credential = authenticator
            .do_registration(Url::parse(TEST_ORIGIN).unwrap(), registration.challenge)
            .unwrap();
        let mut tx = f.runtime.begin().await.unwrap();
        let registered = service
            .finish_registration_in_tx(
                &mut tx,
                a.org,
                registration.ceremony_id,
                credential,
                OffsetDateTime::now_utc(),
            )
            .await
            .unwrap();
        let (family, audit) = RefreshTokenStore
            .issue_family_in_tx(
                &mut tx,
                *a.user.as_uuid(),
                a.org,
                OffsetDateTime::now_utc(),
                Duration::days(30),
            )
            .await
            .unwrap();
        console_platform_db::insert_audit_event(&mut tx, &audit)
            .await
            .unwrap();
        let mut wrong_kind = row.clone();
        wrong_kind.context = registration.ceremony_id;
        wrong_kind.family = family.family_id;
        wrong_kind.source = registered.id;
        OsRng.fill_bytes(&mut raw);
        wrong_kind.handle = format!("bs1.{}", URL_SAFE_NO_PAD.encode(raw));
        wrong_kind.hash = Sha256::digest(wrong_kind.handle.as_bytes()).to_vec();
        let mut kind_claims = old_claims.clone();
        kind_claims.session_family_id = Some(wrong_kind.family);
        kind_claims.exp = wrong_kind.expires.unix_timestamp();
        wrong_kind.seal(&f, sign_claims(&f, &kind_claims).as_bytes());
        deny_scoped_insert(tx, &wrong_kind).await;
    }

    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn browser_sql_insert_binds_discoverable_outcome_account_company_and_source(
        pool: PgPool,
    ) {
        let f = fixture(&pool, "r7-outcome-bindings").await;
        let mut a = actor(&pool, &f, "출처 소유자").await;
        let same = actor_in_org(&pool, &f, "같은 회사 다른 출처", a.org).await;
        let other = actor(&pool, &f, "다른 회사 출처").await;
        let session = login(&f, &mut a).await;
        let template = f
            .verifier
            .verify_access_token(&private_original_proof(&pool, &f, &session).await)
            .unwrap();
        for case in 0..4 {
            let (tx, mut candidate, claims) =
                fresh_fixture(&f.runtime, &f, &mut a, &template).await;
            match case {
                0 => candidate.account = *same.user.as_uuid(),
                1 => candidate.company = *other.org.as_uuid(),
                2 => candidate.source = same.source,
                3 => candidate.source = other.source,
                _ => unreachable!(),
            }
            candidate.seal(&f, sign_claims(&f, &claims).as_bytes());
            deny_scoped_insert(tx, &candidate).await;
        }
        let (mut tx, honest, _) = fresh_fixture(&f.runtime, &f, &mut a, &template).await;
        // Hostile type/relation names must not change the new freshness casts.
        sqlx::raw_sql("CREATE DOMAIN pg_temp.xid AS text; CREATE TEMP TABLE auth_webauthn_ceremonies(id uuid,user_id uuid,ceremony_kind text,consumed_at timestamptz,expires_at timestamptz); SET LOCAL search_path=pg_temp,public,pg_catalog;")
            .execute(tx.as_mut()).await.unwrap();
        insert_scoped(&mut tx, &honest).await.unwrap();
        tx.commit().await.unwrap();
        own_history(&f, &honest.response()).await;
        own_history(&f, &session).await;
    }

    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn browser_invalid_sql_proof_shapes_roll_back_genuine_login_without_constraint_bypass(
        pool: PgPool,
    ) {
        let f = fixture(&pool, "r7-invalid-sql-shapes").await;
        let mut a = actor(&pool, &f, "저장 형식 경계").await;
        login(&f, &mut a).await;
        for (index, (column, value)) in [
            ("codec_version", "0"),
            ("codec_version", "2"),
            (
                "token_hash",
                "pg_catalog.decode(pg_catalog.repeat('00',31),'hex')",
            ),
            (
                "token_hash",
                "pg_catalog.decode(pg_catalog.repeat('00',33),'hex')",
            ),
            (
                "nonce",
                "pg_catalog.decode(pg_catalog.repeat('00',11),'hex')",
            ),
            (
                "nonce",
                "pg_catalog.decode(pg_catalog.repeat('00',13),'hex')",
            ),
            ("tag", "pg_catalog.decode(pg_catalog.repeat('00',15),'hex')"),
            ("tag", "pg_catalog.decode(pg_catalog.repeat('00',17),'hex')"),
            ("ciphertext", "pg_catalog.decode('','hex')"),
            (
                "ciphertext",
                "pg_catalog.decode(pg_catalog.repeat('78',1048577),'hex')",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let ip = format!("198.51.100.{}", 80 + index);
            let (context, signed) = assertion_ip(&f, &mut a, &ip).await;
            let before = effects(&pool).await;
            let constraints: i64=sqlx::query_scalar("SELECT count(*) FROM pg_constraint WHERE conrelid='auth_security.browser_sessions'::regclass AND contype='c' AND convalidated")
                .fetch_one(&pool).await.unwrap();
            assert!(constraints > 0);
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE FUNCTION public.r7_test_bad_shape() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$ BEGIN NEW.{column}:={value}; RETURN NEW; END $$; CREATE TRIGGER r7_test_bad_shape BEFORE INSERT ON auth_security.browser_sessions FOR EACH ROW EXECUTE FUNCTION public.r7_test_bad_shape();")))
                .execute(&pool).await.unwrap();
            let failed = raw_browser_ip(&f, LOGIN, serde_json::to_vec(&signed).unwrap(), &ip).await;
            assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
            assert!(set_cookie_values(&failed).is_empty());
            assert_eq!(effects(&pool).await, before);
            let unused: bool = sqlx::query_scalar(
                "SELECT consumed_at IS NULL FROM auth_webauthn_ceremonies WHERE id=$1",
            )
            .bind(context)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(unused);
            let still_constraints: i64=sqlx::query_scalar("SELECT count(*) FROM pg_constraint WHERE conrelid='auth_security.browser_sessions'::regclass AND contype='c' AND convalidated")
                .fetch_one(&pool).await.unwrap();
            assert_eq!(still_constraints, constraints);
            sqlx::raw_sql("DROP TRIGGER r7_test_bad_shape ON auth_security.browser_sessions; DROP FUNCTION public.r7_test_bad_shape();")
                .execute(&pool).await.unwrap();
            let retry = raw_browser_ip(&f, LOGIN, serde_json::to_vec(&signed).unwrap(), &ip).await;
            assert_eq!(retry.status(), StatusCode::OK);
            let admitted = body_json(retry).await;
            assert_eq!(admitted["context_id"], context.to_string());
            own_history(&f, &admitted).await;
        }
        // Owner insertion bounds are separately checked with genuine fresh
        // metadata. The earlier INSERT-time faults tested table constraints.
        let initial = login(&f, &mut a).await;
        let template = f
            .verifier
            .verify_access_token(&private_original_proof(&pool, &f, &initial).await)
            .unwrap();
        for field in ["empty_cipher", "over_cipher", "nonce", "tag", "hash"] {
            let (tx, mut candidate, _) = fresh_fixture(&f.runtime, &f, &mut a, &template).await;
            match field {
                "empty_cipher" => candidate.ciphertext.clear(),
                "over_cipher" => candidate.ciphertext = vec![0; MAX_PROOF + 1],
                "nonce" => candidate.nonce = vec![0; 11],
                "tag" => candidate.tag = vec![0; 15],
                "hash" => candidate.hash = vec![0; 31],
                _ => unreachable!(),
            }
            deny_scoped_insert(tx, &candidate).await;
        }
        for n in [1, MAX_PROOF] {
            let (mut tx, mut boundary, _) = fresh_fixture(&f.runtime, &f, &mut a, &template).await;
            boundary.seal(&f, &vec![b'x'; n]);
            insert_scoped(&mut tx, &boundary).await.unwrap();
            tx.commit().await.unwrap();
            let saved = stored(&pool, &boundary.response()).await;
            assert_eq!(saved.ciphertext.len(), n);
            let before = effects(&pool).await;
            // Shape-valid, intentionally invalid signed-proof plaintext. This
            // never claims a legitimate browser login or business authority.
            assert_eq!(
                history_raw(&f, &boundary.response(), json!({"limit":25,"offset":0}))
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(effects(&pool).await, before);
        }
    }

    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn browser_native_body_caps_and_closed_routes_preserve_owner_effects(pool: PgPool) {
        let f = fixture(&pool, "r7-native-body-caps").await;
        let mut a = actor(&pool, &f, "본문 한도").await;
        let session = login(&f, &mut a).await;
        let proof = private_original_proof(&pool, &f, &session).await;
        let mut selected = handle(&session);
        selected["limit"] = json!(25);
        selected["offset"] = json!(0);
        let before = effects(&pool).await;
        let exact = raw_browser(&f, HISTORY, padded_json(&selected, 4096)).await;
        assert_eq!(exact.status(), StatusCode::OK);
        let bytes = to_bytes(exact.into_body(), 2 * 1024 * 1024).await.unwrap();
        assert!(
            !bytes
                .windows(proof.len())
                .any(|window| window == proof.as_bytes())
        );
        assert_eq!(
            raw_browser(&f, HISTORY, padded_json(&selected, 4097))
                .await
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        for field in ["employee_id", "company_id", "account_id", "context_id"] {
            let mut unknown = selected.clone();
            unknown[field] = json!(Uuid::new_v4());
            assert_eq!(
                post_raw(f.router.clone(), HISTORY, None, unknown)
                    .await
                    .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
        assert_eq!(
            raw_browser(
                &f,
                "/api/v1/auth/browser-session/resolve",
                serde_json::to_vec(&handle(&session)).unwrap()
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get(
                &f,
                "/api/v1/hr/attendance-records/me/browser-context",
                &proof
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(effects(&pool).await, before);
        assert_eq!(
            raw_browser(&f, LOGOUT, padded_json(&handle(&session), 4097))
                .await
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let mut extra = handle(&session);
        extra["limit"] = json!(25);
        assert_eq!(
            post_raw(f.router.clone(), LOGOUT, None, extra)
                .await
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(effects(&pool).await, before);
        let receipt = raw_browser(&f, LOGOUT, padded_json(&handle(&session), 4096)).await;
        assert_eq!(receipt.status(), StatusCode::NO_CONTENT);
        assert!(
            to_bytes(receipt.into_body(), 4096)
                .await
                .unwrap()
                .is_empty()
        );
        let (context, signed) = assertion(&f, &mut a).await;
        let before = effects(&pool).await;
        assert_eq!(
            raw_browser(&f, LOGIN, padded_json(&signed, MAX_LOGIN_BODY + 1))
                .await
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(effects(&pool).await, before);
        let admitted = raw_browser(&f, LOGIN, padded_json(&signed, MAX_LOGIN_BODY)).await;
        assert_eq!(admitted.status(), StatusCode::OK);
        assert!(set_cookie_values(&admitted).is_empty());
        let bytes = to_bytes(admitted.into_body(), 64 * 1024).await.unwrap();
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["context_id"], context.to_string());
        assert_eq!(
            result
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from(["context_id", "expires_at", "session_token"])
        );
        own_history(&f, &result).await;
    }
}

// Additive test preparation for R7c. Append inside auth_rest::browser_sessions.
// Uses actual top-level routes and the real single-connection SCRAM runtime pool.
// No test-only production handler, fabricated persisted read audit, or execution claim.
mod r7_request_context_additions {
    use super::*;
    use console_platform_request_context::{
        RequestAuditContext, current_audit_context, current_org,
    };
    use std::sync::{Arc, Mutex};

    const OBSERVATION_BOUND: usize = 128;

    #[derive(Clone, Debug)]
    struct Observation {
        org: Option<OrgId>,
        audit: Option<RequestAuditContext>,
    }

    #[derive(Default)]
    struct Observations {
        items: Vec<Observation>,
        overflow: bool,
    }

    fn take_observations(state: &Arc<Mutex<Observations>>) -> Vec<Observation> {
        let mut state = state.lock().expect("private acquisition observation lock");
        assert!(
            !state.overflow,
            "bounded private observation storage overflowed"
        );
        std::mem::take(&mut state.items)
    }

    fn assert_final_owner_context(
        observations: Vec<Observation>,
        actor: &Actor,
        method: &str,
        trace: &TraceContext,
        device: &str,
        agent: &str,
    ) {
        // The completed HR owner performs its repeatable-read snapshot last.
        // Accepting ANY matching earlier acquisition would allow an auth-only
        // scope to hide an unscoped HR owner. Retain None observations as well.
        let final_acquisition = observations
            .last()
            .expect("actual HR request must reuse the warmed runtime connection");
        assert_eq!(final_acquisition.org, Some(actor.org));
        let audit = final_acquisition
            .audit
            .as_ref()
            .expect("final HR owner acquisition requires actual request audit context");
        assert_eq!(&audit.trace, trace);
        assert_eq!(audit.request.ip.as_deref(), Some("198.51.100.24"));
        assert_eq!(audit.request.user_agent.as_deref(), Some(agent));
        assert_eq!(audit.request.auth_method.as_deref(), Some(method));
        assert_eq!(audit.request.device.as_deref(), Some(device));
    }

    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn browser_hr_owner_keeps_actual_request_context_and_distinct_auth_method(pool: PgPool) {
        let f = fixture(&pool, "browser-owner-context-setup").await;
        let mut a = actor(&pool, &f, "요청 문맥 주체").await;
        let mut b = actor(&pool, &f, "독립 법인 문맥 주체").await;
        let a_session = login(&f, &mut a).await;
        let b_session = login(&f, &mut b).await;
        let a_bearer = private_original_proof(&pool, &f, &a_session).await;
        let b_bearer = private_original_proof(&pool, &f, &b_session).await;
        let a_employee = link_employee(&pool, &a).await;
        let b_employee = link_employee(&pool, &b).await;
        create_records(&f, &a, &a_bearer, 2).await;
        create_records(&f, &b, &b_bearer, 2).await;

        let observations = Arc::new(Mutex::new(Observations::default()));
        let observed = Arc::clone(&observations);
        let runtime = PgPoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .max_lifetime(None)
            .idle_timeout(None)
            .before_acquire(move |_connection, _metadata| {
                let observation = Observation {
                    org: current_org().ok(),
                    audit: current_audit_context(),
                };
                let mut state = observed
                    .lock()
                    .expect("private acquisition observation lock");
                if state.items.len() < OBSERVATION_BOUND {
                    state.items.push(observation);
                } else {
                    state.overflow = true;
                }
                Box::pin(async { Ok(true) })
            })
            .connect_with(f.runtime.connect_options().as_ref().clone())
            .await
            .unwrap();
        // SQLx before_acquire runs on reused idle connections, not creation.
        // Warm exactly one authenticated nonowner connection and disable its
        // lifetime/idle replacement so missing scope cannot hide in a new one.
        let warm = runtime.acquire().await.unwrap();
        drop(warm);
        let _warm_only = take_observations(&observations);
        let router =
            configured_router(runtime.clone(), &f.private, &f.public, Some(&f.storage_key));
        assert!(current_org().is_err());
        assert!(current_audit_context().is_none());
        let custody_before = effects(&pool).await;
        let a_mapping = immutable_mapping(&pool, &a_session).await;
        let b_mapping = immutable_mapping(&pool, &b_session).await;

        for (actor, session, bearer, employee, device, agent, trace_id, span_id) in [
            (
                &a,
                &a_session,
                &a_bearer,
                a_employee,
                "r7-owner-context-a",
                "R7 genuine browser HR a",
                "0af7651916cd43dd8448eb211c80319c",
                "b7ad6b7169203331",
            ),
            (
                &b,
                &b_session,
                &b_bearer,
                b_employee,
                "r7-owner-context-b",
                "R7 genuine browser HR b",
                "5af7651916cd43dd8448eb211c80319c",
                "c7ad6b7169203331",
            ),
        ] {
            let trace = TraceContext::new(trace_id, span_id).unwrap();
            let traceparent = format!("00-{trace_id}-{span_id}-01");
            let response = router
                .clone()
                .oneshot(
                    browser_ingress(
                        Request::builder()
                            .method("POST")
                            .uri(HISTORY)
                            .header(header::CONTENT_TYPE, "application/json")
                            .header(header::USER_AGENT, agent)
                            .header("x-device-id", device)
                            .header("traceparent", &traceparent),
                    )
                    .body(Body::from(history_handle(session).to_string()))
                    .unwrap(),
                )
                .await
                .unwrap();
            let actual_owner_observations = take_observations(&observations);
            assert_eq!(response.status(), StatusCode::OK);
            assert!(set_cookie_values(&response).is_empty());
            assert_final_owner_context(
                actual_owner_observations,
                actor,
                "browser_session",
                &trace,
                device,
                agent,
            );
            assert!(current_org().is_err());
            assert!(current_audit_context().is_none());
            let browser = body_json(response).await;
            assert_eq!(browser["browser_context"], session["context_id"]);
            assert_eq!(browser["expires_at"], session["expires_at"]);
            assert_eq!(browser["context"]["company_id"], actor.org.to_string());
            assert_eq!(
                browser["context"]["company_name"],
                format!("{} 법인", actor.name)
            );
            assert_eq!(browser["context"]["account_display_name"], actor.name);
            assert_eq!(browser["context"]["employee_linked"], true);
            assert_eq!(browser["history"]["total"], 2);
            assert_eq!(browser["history"]["items"].as_array().unwrap().len(), 2);
            for item in browser["history"]["items"].as_array().unwrap() {
                assert_eq!(item["employee_id"], employee.to_string());
                assert_eq!(item["employee_display_name"], actor.name);
            }

            // The unchanged legacy top-level GET uses the same actual pool and
            // owner, while keeping bearer metadata. This is a direct transport
            // peer; no fabricated XFF/audit row supplies its observed address.
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri("/api/v1/hr/attendance-records/me?limit=25&offset=0")
                        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                        .header(header::USER_AGENT, agent)
                        .header("x-device-id", device)
                        .header("traceparent", &traceparent)
                        .extension(ConnectInfo(
                            "198.51.100.24:5443".parse::<SocketAddr>().unwrap(),
                        ))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let legacy_owner_observations = take_observations(&observations);
            assert_eq!(response.status(), StatusCode::OK);
            assert_final_owner_context(
                legacy_owner_observations,
                actor,
                "bearer",
                &trace,
                device,
                agent,
            );
            assert!(current_org().is_err());
            assert!(current_audit_context().is_none());
            assert_eq!(body_json(response).await, browser["history"]);
        }

        assert_eq!(effects(&pool).await, custody_before);
        assert_eq!(immutable_mapping(&pool, &a_session).await, a_mapping);
        assert_eq!(immutable_mapping(&pool, &b_session).await, b_mapping);
        assert!(take_observations(&observations).is_empty());
        runtime.close().await;
    }
}
