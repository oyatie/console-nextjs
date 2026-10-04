//! Native enrollment diagnostics and C2 strict new-key sign-in prerequisite.
//! Not production Next sign-in or browser-session R6 acceptance.
//! After a production Next build, run this target with --features frontend-e2e,
//! FRONTEND_ROOT and pgtest.sh's disposable password-authenticated database.
use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration as StdDuration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::timeout;

const MAX_FRAME: usize = 65_536;
const MAX_FRAMES: u64 = 128;
const PHASE: StdDuration = StdDuration::from_secs(20);

// Errors deliberately contain no response/proof, subprocess output or DSN.
type ProbeResult<T> = Result<T, &'static str>;

struct Browser {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    sent: u64,
    received: u64,
    stderr: tokio::task::JoinHandle<usize>,
    cleanup_ack: Option<Value>,
}

impl Browser {
    fn start(root: PathBuf) -> ProbeResult<Self> {
        if !root.join(".next/standalone/server.js").is_file() {
            return Err("infrastructure: production standalone build missing");
        }
        let mut command = Command::new("node");
        command
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../tools/test-native-browser-enrollment.mjs"),
            )
            .current_dir(root)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for key in [
            "PATH",
            "HOME",
            "TMPDIR",
            "LANG",
            "LC_ALL",
            "PLAYWRIGHT_BROWSERS_PATH",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let mut child = command
            .spawn()
            .map_err(|_| "infrastructure: Node start failed")?;
        let input = child.stdin.take().ok_or("infrastructure: stdin missing")?;
        let output = BufReader::new(
            child
                .stdout
                .take()
                .ok_or("infrastructure: stdout missing")?,
        );
        let mut error_output = child
            .stderr
            .take()
            .ok_or("infrastructure: stderr missing")?;
        let stderr = tokio::spawn(async move {
            let mut count = 0_usize;
            let mut bytes = [0_u8; 8192];
            while let Ok(length) = error_output.read(&mut bytes).await {
                if length == 0 {
                    break;
                }
                count = count.saturating_add(length);
            }
            count
        });
        Ok(Self {
            child,
            input: Some(input),
            output,
            sent: 0,
            received: 0,
            stderr,
            cleanup_ack: None,
        })
    }

    async fn read(&mut self, kind: &str) -> ProbeResult<Value> {
        if self.received >= MAX_FRAMES {
            return Err("infrastructure: frame count");
        }
        let frame = timeout(PHASE, async {
            let length = self
                .output
                .read_u32()
                .await
                .map_err(|_| "infrastructure: frame header")? as usize;
            if length == 0 || length > MAX_FRAME {
                return Err("infrastructure: frame size");
            }
            let mut body = vec![0_u8; length];
            self.output
                .read_exact(&mut body)
                .await
                .map_err(|_| "infrastructure: frame body")?;
            serde_json::from_slice::<Value>(&body).map_err(|_| "infrastructure: frame JSON")
        })
        .await
        .map_err(|_| "infrastructure: frame deadline")??;
        if frame["v"] != 1 || frame["seq"] != self.received {
            return Err("infrastructure: frame identity");
        }
        self.received += 1;
        if frame["kind"] != kind {
            if frame["kind"] == "cleaned" {
                self.cleanup_ack = Some(frame);
            }
            return Err("infrastructure: browser phase ended before native progress");
        }
        Ok(frame)
    }

    async fn write(&mut self, mut frame: Value) -> ProbeResult<()> {
        if self.sent >= MAX_FRAMES {
            return Err("infrastructure: frame count");
        }
        frame["v"] = json!(1);
        frame["seq"] = json!(self.sent);
        let bytes = serde_json::to_vec(&frame).map_err(|_| "infrastructure: frame encoding")?;
        if bytes.is_empty() || bytes.len() > MAX_FRAME {
            return Err("infrastructure: frame size");
        }
        let input = self.input.as_mut().ok_or("infrastructure: stdin closed")?;
        timeout(PHASE, async {
            input.write_u32(bytes.len() as u32).await?;
            input.write_all(&bytes).await?;
            input.flush().await
        })
        .await
        .map_err(|_| "infrastructure: write deadline")?
        .map_err(|_| "infrastructure: frame write")?;
        self.sent += 1;
        Ok(())
    }

    async fn reap(&mut self, phase_ok: bool) -> ProbeResult<()> {
        // Failure uses EOF to abort. Success keeps the pipe open until Node has
        // consumed done and acknowledged completed cleanup; EOF cannot win it.
        if !phase_ok {
            drop(self.input.take());
        }
        let acknowledgement = match self.cleanup_ack.take() {
            Some(acknowledgement) => Ok(acknowledgement),
            None => timeout(StdDuration::from_secs(8), self.read("cleaned"))
                .await
                .unwrap_or(Err("infrastructure: cleanup acknowledgement deadline")),
        };
        drop(self.input.take());
        let status = match timeout(StdDuration::from_secs(8), self.child.wait()).await {
            Ok(status) => status.map_err(|_| "infrastructure: child wait")?,
            Err(_) => {
                let _ = self.child.kill().await;
                self.stderr.abort();
                return Err("infrastructure: cleanup unconfirmed; possible orphan");
            }
        };
        let count = timeout(StdDuration::from_secs(2), &mut self.stderr)
            .await
            .map_err(|_| "infrastructure: stderr drain deadline")?
            .map_err(|_| "infrastructure: stderr drain failed")?;
        if count > 1_048_576 {
            return Err("infrastructure: output limit");
        }
        let acknowledgement = acknowledgement?;
        let expected_exit = match acknowledgement["outcome"].as_str() {
            Some("completed") if phase_ok => 0,
            Some("aborted") if !phase_ok => 2,
            Some("failed") if !phase_ok => 1,
            _ => return Err("infrastructure: cleanup outcome mismatch"),
        };
        if status.code() != Some(expected_exit) {
            return Err("infrastructure: cleanup exit mismatch");
        }
        Ok(())
    }
}

async fn native_post(
    client: &reqwest::Client,
    origin: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
    status: StatusCode,
) -> ProbeResult<Value> {
    let bytes = serde_json::to_vec(&body).map_err(|_| "native: request codec")?;
    if bytes.len() > MAX_FRAME {
        return Err("native: request size");
    }
    let mut request = client
        .post(format!("{origin}{path}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(bytes);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let mut response = request.send().await.map_err(|_| "native: request failed")?;
    if response.status() != status {
        return Err("native: unexpected status");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "native: response failed")?
    {
        if bytes.len() + chunk.len() > MAX_FRAME {
            return Err("native: response size");
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "native: response JSON")
}

async fn enrollment_digest(owner: &PgPool) -> ProbeResult<[u8; 32]> {
    // Baseline custody only: the proposed browser mapping table does not exist.
    let state: Value = sqlx::query_scalar("SELECT jsonb_build_object(
        'keys',(SELECT jsonb_agg(to_jsonb(k) ORDER BY id) FROM auth_webauthn_credentials k),
        'ceremonies',(SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM auth_webauthn_ceremonies c),
        'bootstrap',(SELECT jsonb_agg(to_jsonb(b) ORDER BY id) FROM auth_bootstrap_credentials b),
        'families',(SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM auth_refresh_token_families f),
        'tokens',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM auth_refresh_tokens t),
        'binding',(SELECT jsonb_agg(to_jsonb(r) ORDER BY ceremony_id) FROM auth_legacy_registration_bindings r),
        'audits',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM audit_events a))")
        .fetch_one(owner).await.map_err(|_| "fixture: enrollment custody")?;
    Ok(Sha256::digest(serde_json::to_vec(&state).map_err(|_| "fixture: custody codec")?).into())
}

async fn registration_probe(
    owner: &PgPool,
    runtime: &PgPool,
    browser: &mut Browser,
    server: &mut Option<tokio::task::JoinHandle<()>>,
    interrupt_after_creation: bool,
    discoverable: bool,
) -> ProbeResult<Vec<Value>> {
    let first = browser.read("origin").await?;
    let origin = first["origin"]
        .as_str()
        .ok_or("infrastructure: origin missing")?;
    let url = Url::parse(origin).map_err(|_| "infrastructure: origin invalid")?;
    if url.scheme() != "http"
        || url.host_str() != Some("localhost")
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("infrastructure: origin boundary");
    }
    let signing = SigningKey::random(&mut OsRng);
    let private = signing
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|_| "fixture: signing key")?;
    let public = signing
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .map_err(|_| "fixture: verification key")?;
    let verifier = console_platform_auth::JwtVerifier::from_es256_public_pem(
        console_platform_auth::JwtSettings {
            issuer: TEST_ISSUER.into(),
            audience: TEST_AUDIENCE.into(),
            access_token_ttl: Duration::minutes(15),
        },
        public.as_bytes(),
    )
    .map_err(|_| "fixture: native verifier")?;
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".into()),
        ("CONSOLE_JWT_ISSUER", TEST_ISSUER.into()),
        ("CONSOLE_JWT_AUDIENCE", TEST_AUDIENCE.into()),
        ("CONSOLE_JWT_PRIVATE_KEY_PEM", private.to_string()),
        ("CONSOLE_JWT_PUBLIC_KEY_PEM", public),
        ("CONSOLE_WEBAUTHN_RP_ID", "localhost".into()),
        ("CONSOLE_WEBAUTHN_RP_ORIGIN", origin.into()),
        (
            "CONSOLE_WEBAUTHN_RP_NAME",
            "Console browser prerequisite".into(),
        ),
    ])
    .map_err(|_| "fixture: native configuration")?;
    let router = build_router(
        AppState::new(config, DatabaseDependency::Postgres(runtime.clone()))
            .map_err(|_| "fixture: native state")?,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| "fixture: native listener")?;
    let native_origin = format!(
        "http://{}",
        listener
            .local_addr()
            .map_err(|_| "fixture: native address")?
    );
    *server = Some(tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    }));
    let ready = browser.read("ready").await?;
    if ready["origin"] != origin {
        return Err("infrastructure: ready origin mismatch");
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(StdDuration::from_secs(10))
        .build()
        .map_err(|_| "fixture: HTTP client")?;
    let mut observations = Vec::new();
    for actor in ["a", "b"] {
        let org: Uuid = sqlx::query_scalar(
            "INSERT INTO organizations(slug,name,status) VALUES($1,$2,'ACTIVE') RETURNING id",
        )
        .bind(format!("native-{}", Uuid::new_v4().simple()))
        .bind(format!("브라우저 {actor} 법인"))
        .fetch_one(owner)
        .await
        .map_err(|_| "fixture: Company insert")?;
        let user = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users(id,display_name,roles,org_id) VALUES($1,$2,ARRAY['MEMBER'],$3)",
        )
        .bind(user)
        .bind(format!("브라우저 {actor} 사용자"))
        .bind(org)
        .execute(owner)
        .await
        .map_err(|_| "fixture: Account insert")?;
        let issue = BootstrapCredentialStore
            .issue_for_zero_credential_user(
                runtime,
                user,
                OrgId::from_uuid(org),
                OffsetDateTime::now_utc(),
                Duration::hours(1),
            )
            .await
            .map_err(|_| "native: OTP issue")?;
        let enrollment = native_post(
            &client,
            &native_origin,
            "/api/v1/auth/otp/redeem",
            None,
            json!({"otp": issue.token.as_str()}),
            StatusCode::OK,
        )
        .await?;
        if enrollment["requires_passkey_setup"] != true {
            return Err("native: expected enrollment-only proof");
        }
        let token = enrollment["access_token"]
            .as_str()
            .ok_or("native: enrollment proof missing")?;
        let consent = native_post(
            &client,
            &native_origin,
            "/api/v1/auth/privacy-consent/status",
            Some(token),
            json!({}),
            StatusCode::OK,
        )
        .await?;
        native_post(&client, &native_origin, "/api/v1/auth/privacy-consent/accept", Some(token),
            json!({"policy_version":consent["policy_version"], "privacy_collection":true, "terms_of_service":true}),
            StatusCode::OK).await?;
        let mut registration = json!({"username":format!("browser.{actor}"), "display_name":format!("브라우저 {actor} 사용자")});
        if discoverable {
            registration["require_discoverable"] = json!(true);
        }
        let start = native_post(
            &client,
            &native_origin,
            "/api/v1/auth/passkey/register/start",
            Some(token),
            registration,
            StatusCode::OK,
        )
        .await?;
        let ceremony = start["ceremony_id"]
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or("native: ceremony missing")?;
        let stored: Value = sqlx::query_scalar("SELECT challenge_json FROM auth_webauthn_ceremonies WHERE id=$1 AND user_id=$2 AND consumed_at IS NULL")
            .bind(ceremony).bind(user).fetch_one(owner).await.map_err(|_| "fixture: creation options custody")?;
        if stored != start["challenge"] {
            return Err("native: stored creation options mismatch");
        }
        browser
            .write(
                json!({"kind":"register", "actor":actor, "ceremony":ceremony,
            "mode":if discoverable { "discoverable" } else { "legacy" },
            "options":start["challenge"]}),
            )
            .await?;
        let created = browser.read("registered").await?;
        if created["actor"] != actor
            || created["ceremony"] != ceremony.to_string()
            || !(created["resident_key"].is_boolean() || created["resident_key"].is_null())
        {
            return Err("infrastructure: registration binding mismatch");
        }
        if interrupt_after_creation {
            return Err("fixture: injected native-phase interruption");
        }
        let finish_body = json!({"ceremony_id":ceremony, "credential":created["credential"]});
        let finish = native_post(
            &client,
            &native_origin,
            "/api/v1/auth/passkey/register/finish",
            Some(token),
            finish_body.clone(),
            StatusCode::CREATED,
        )
        .await?;
        let credential_id = finish["credential_id"]
            .as_str()
            .ok_or("native: credential identity missing")?;
        if created["credential"]["id"] != credential_id {
            return Err("native: browser credential identity mismatch");
        }
        let passkey_id = finish["passkey_id"]
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or("native: stored passkey identity missing")?;
        if finish["user_id"] != user.to_string() {
            return Err("native: registration Account mismatch");
        }
        let effects: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM auth_webauthn_credentials WHERE id=$6 AND user_id=$1 AND org_id=$2 AND credential_id=$3),
                    (SELECT count(*) FROM auth_webauthn_ceremonies WHERE id=$4 AND user_id=$1 AND consumed_at IS NOT NULL),
                    (SELECT count(*) FROM auth_bootstrap_credentials WHERE id=$5 AND user_id=$1 AND org_id=$2 AND consumed_at IS NOT NULL),
                    (SELECT count(*) FROM audit_events WHERE actor=$1 AND org_id=$2 AND action='auth.passkey.register' AND target_type='auth_webauthn_credential' AND target_id=$6::text)"
        ).bind(user).bind(org).bind(credential_id).bind(ceremony).bind(issue.credential_id).bind(passkey_id).fetch_one(owner).await
            .map_err(|_| "fixture: enrollment effects")?;
        if effects != (1, 1, 1, 1) {
            return Err("native: enrollment effects mismatch");
        }
        let before = enrollment_digest(owner).await?;
        native_post(
            &client,
            &native_origin,
            "/api/v1/auth/passkey/register/finish",
            Some(token),
            finish_body,
            StatusCode::UNAUTHORIZED,
        )
        .await?;
        if before != enrollment_digest(owner).await? {
            return Err("native: enrollment replay effects");
        }
        let login_start = native_post(
            &client,
            &native_origin,
            if discoverable {
                "/api/v1/auth/passkey/login/explicit/start"
            } else {
                "/api/v1/auth/passkey/login/start"
            },
            None,
            json!({}),
            StatusCode::OK,
        )
        .await?;
        let login_ceremony = login_start["ceremony_id"]
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or("native: login ceremony missing")?;
        let stored: Value = sqlx::query_scalar("SELECT challenge_json FROM auth_webauthn_ceremonies WHERE id=$1 AND user_id IS NULL AND consumed_at IS NULL")
            .bind(login_ceremony).fetch_one(owner).await.map_err(|_| "fixture: authentication options custody")?;
        if stored != login_start["challenge"] {
            return Err("native: stored authentication options mismatch");
        }
        let before_login = enrollment_digest(owner).await?;
        let before_counter: i64 = sqlx::query_scalar("SELECT (passkey_json->'cred'->>'counter')::bigint FROM auth_webauthn_credentials WHERE id=$1 AND user_id=$2 AND org_id=$3")
            .bind(passkey_id).bind(user).bind(org).fetch_one(owner).await.map_err(|_| "fixture: initial credential counter")?;
        browser
            .write(
                json!({"kind":"login", "actor":actor, "ceremony":login_ceremony,
            "options":login_start["challenge"]}),
            )
            .await?;
        let attempted = browser.read("login-result").await?;
        if attempted["actor"] != actor || attempted["ceremony"] != login_ceremony.to_string() {
            return Err("infrastructure: login attempt binding mismatch");
        }
        if !attempted["authenticator_is_resident"].is_boolean()
            || !attempted["browser_environment"]["secure_context"].is_boolean()
            || !attempted["browser_environment"]["document_has_focus"].is_boolean()
            || !matches!(
                attempted["browser_environment"]["visibility_state"].as_str(),
                Some("visible" | "hidden")
            )
            || attempted["browser_environment"]
                .get("conditional_mediation_available")
                .is_none()
            || !(attempted["browser_environment"]["conditional_mediation_available"].is_boolean()
                || attempted["browser_environment"]["conditional_mediation_available"].is_null())
        {
            return Err("infrastructure: browser metadata shape");
        }
        let outcome = attempted["outcome"]
            .as_str()
            .ok_or("infrastructure: login outcome missing")?;
        if discoverable
            && (attempted["authenticator_is_resident"] != true || outcome != "assertion")
        {
            return Err("native: required discoverable assertion missing");
        }
        if outcome == "assertion" {
            if attempted["credential"]["id"] != created["credential"]["id"] {
                return Err("native: asserted credential mismatch");
            }
            let auth_data = URL_SAFE_NO_PAD
                .decode(
                    attempted["credential"]["response"]["authenticatorData"]
                        .as_str()
                        .ok_or("native: authenticator data missing")?,
                )
                .map_err(|_| "native: authenticator data codec")?;
            if auth_data.len() < 37 || auth_data.len() > MAX_FRAME {
                return Err("native: authenticator data size");
            }
            let signed_counter = u32::from_be_bytes(
                auth_data[33..37]
                    .try_into()
                    .map_err(|_| "native: counter bytes")?,
            ) as i64;
            if discoverable && signed_counter <= before_counter {
                return Err("native: signed counter did not increase");
            }
            let body = json!({"ceremony_id":login_ceremony,"credential":attempted["credential"]});
            let issued = native_post(
                &client,
                &native_origin,
                "/api/v1/auth/passkey/login/finish",
                None,
                body.clone(),
                StatusCode::OK,
            )
            .await?;
            let token = issued["access_token"]
                .as_str()
                .ok_or("native: login proof missing")?;
            let claims = verifier
                .verify_access_token(token)
                .map_err(|_| "native: login signature")?;
            if claims.sub != user.to_string()
                || claims.org != org.to_string()
                || claims.platform
                || claims.view_as
                || claims.read_only
                || claims.actor_session.is_some()
                || claims.tenant_context.is_some()
            {
                return Err("native: login identity mismatch");
            }
            let family = claims
                .session_family_id
                .ok_or("native: login family missing")?;
            let refresh_hash = Sha256::digest(
                issued["refresh_token"]
                    .as_str()
                    .ok_or("native: refresh proof missing")?
                    .as_bytes(),
            )
            .to_vec();
            let login_effects: (i64,i64,i64,i64,i64) = sqlx::query_as(
                "SELECT (SELECT count(*) FROM auth_webauthn_ceremonies WHERE id=$1 AND ceremony_kind='authentication' AND user_id IS NULL AND consumed_at IS NOT NULL),
                        (SELECT count(*) FROM auth_refresh_token_families WHERE id=$3 AND user_id=$2 AND org_id=$4 AND revoked_at IS NULL),
                        (SELECT count(*) FROM auth_refresh_tokens WHERE family_id=$3 AND user_id=$2 AND org_id=$4 AND revoked_at IS NULL AND token_hash=$7),
                        (SELECT count(*) FROM audit_events WHERE actor=$2 AND org_id=$4 AND action='auth.login'
                         AND target_type='users' AND target_id=$2::text
                         AND after_snap->>'passkey_id'=$5 AND after_snap->>'refresh_family_id'=$3::text),
                        (SELECT count(*) FROM auth_webauthn_credentials WHERE id=$5::uuid AND user_id=$2 AND org_id=$4
                         AND credential_id=$6 AND last_used_at=(SELECT consumed_at FROM auth_webauthn_ceremonies WHERE id=$1))"
            ).bind(login_ceremony).bind(user).bind(family).bind(org).bind(passkey_id.to_string())
                .bind(credential_id).bind(refresh_hash)
                .fetch_one(owner).await.map_err(|_| "fixture: login effects")?;
            if login_effects != (1, 1, 1, 1, 1) {
                return Err("native: login effects mismatch");
            }
            let stored_counter: i64 = sqlx::query_scalar("SELECT (passkey_json->'cred'->>'counter')::bigint FROM auth_webauthn_credentials WHERE id=$1 AND user_id=$2 AND org_id=$3")
                .bind(passkey_id).bind(user).bind(org).fetch_one(owner).await.map_err(|_| "fixture: committed credential counter")?;
            if stored_counter != signed_counter {
                return Err("native: signed counter was not persisted");
            }
            let committed = enrollment_digest(owner).await?;
            native_post(
                &client,
                &native_origin,
                "/api/v1/auth/passkey/login/finish",
                None,
                body,
                StatusCode::UNAUTHORIZED,
            )
            .await?;
            if enrollment_digest(owner).await? != committed {
                return Err("native: login replay effects");
            }
        } else {
            if ![
                "deadline-aborted",
                "null",
                "NotAllowedError",
                "AbortError",
                "SecurityError",
                "NotSupportedError",
                "InvalidStateError",
                "other-rejection",
            ]
            .contains(&outcome)
            {
                return Err("infrastructure: unexpected login outcome");
            }
            let unused: bool = sqlx::query_scalar(
                "SELECT consumed_at IS NULL FROM auth_webauthn_ceremonies WHERE id=$1",
            )
            .bind(login_ceremony)
            .fetch_one(owner)
            .await
            .map_err(|_| "fixture: pending login custody")?;
            if !unused || enrollment_digest(owner).await? != before_login {
                return Err("native: unsubmitted login effects");
            }
        }
        observations.push(json!({"actor":actor, "native_registration":"accepted",
            "requested_resident_key":start["challenge"]["publicKey"]["authenticatorSelection"]["residentKey"],
            "browser_cred_props_rk":created["resident_key"],
            "authenticator_is_resident":attempted["authenticator_is_resident"],
            "browser_environment":{
                "secure_context":attempted["browser_environment"]["secure_context"],
                "document_has_focus":attempted["browser_environment"]["document_has_focus"],
                "visibility_state":attempted["browser_environment"]["visibility_state"],
                "conditional_mediation_available":attempted["browser_environment"]["conditional_mediation_available"]
            },
            "requested_require_resident_key":start["challenge"]["publicKey"]["authenticatorSelection"]["requireResidentKey"],
            "usernameless_login":outcome,
            "scope":if discoverable { "Strict native new-key prerequisite on production RP origin; not product sign-in acceptance" }
                else { "Exact-options diagnostic on not-found RP origin; not product sign-in acceptance" }}));
    }
    browser.write(json!({"kind":"done"})).await?;
    Ok(observations)
}

async fn runtime_login(owner: &PgPool) -> PgPool {
    let options = owner
        .connect_options()
        .as_ref()
        .clone()
        .username("console_rt")
        .password(
            &std::env::var("CONSOLE_TEST_RUNTIME_PASSWORD").expect("disposable runtime password"),
        )
        .application_name("native-browser-enrollment");
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
        "trust-auth is not acceptance"
    );
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    let identity: (String, String, bool, bool) = sqlx::query_as(
        "SELECT session_user::text,current_user::text,rolsuper,rolbypassrls FROM pg_roles WHERE rolname=current_user"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(
        identity,
        ("console_rt".into(), "console_rt".into(), false, false)
    );
    pool
}

async fn run_probe(
    pool: &PgPool,
    interrupt_after_creation: bool,
    discoverable: bool,
) -> ProbeResult<Vec<Value>> {
    let runtime = runtime_login(pool).await;
    let root = PathBuf::from(
        std::env::var_os("FRONTEND_ROOT").expect("FRONTEND_ROOT must be a built frontend"),
    );
    let mut browser = Browser::start(root).expect("start isolated browser prerequisite");
    let mut server = None;
    let result = timeout(
        StdDuration::from_secs(130),
        registration_probe(
            pool,
            &runtime,
            &mut browser,
            &mut server,
            interrupt_after_creation,
            discoverable,
        ),
    )
    .await
    .unwrap_or(Err("infrastructure: overall deadline"));
    if let Some(server) = server {
        server.abort();
        let _ = server.await;
    }
    browser.reap(result.is_ok()).await?;
    result
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn genuine_browser_enrollment_preserves_native_options_and_owned_effects(pool: PgPool) {
    let observations = run_probe(&pool, false, false)
        .await
        .expect("native browser enrollment prerequisite");
    println!(
        "NATIVE_BROWSER_ENROLLMENT {}",
        serde_json::to_string(&observations).unwrap()
    );
    assert_eq!(observations.len(), 2);
    // Enrollment acceptance is distinct from discoverability and login. A false
    // rk is reported honestly for prerequisite review, never relabeled sign-in.
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn genuine_browser_failed_native_phase_is_preserved_after_confirmed_cleanup(pool: PgPool) {
    assert!(matches!(
        run_probe(&pool, true, false).await,
        Err("fixture: injected native-phase interruption")
    ));
    let custody: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM auth_webauthn_credentials),
                (SELECT count(*) FROM auth_webauthn_ceremonies WHERE ceremony_kind='registration' AND consumed_at IS NULL),
                (SELECT count(*) FROM auth_bootstrap_credentials WHERE consumed_at IS NOT NULL)"
    ).fetch_one(&pool).await.expect("read interrupted enrollment custody");
    assert_eq!(
        custody,
        (0, 1, 0),
        "interruption cannot become native enrollment success"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn genuine_browser_discoverable_enrollment_requires_two_native_logins(pool: PgPool) {
    let observations = run_probe(&pool, false, true)
        .await
        .expect("strict native new-key sign-in");
    assert_eq!(observations.len(), 2);
    for observation in &observations {
        assert_eq!(observation["requested_resident_key"], "required");
        assert_eq!(observation["requested_require_resident_key"], true);
        assert_eq!(observation["authenticator_is_resident"], true);
        assert_eq!(observation["usernameless_login"], "assertion");
    }
    println!(
        "NATIVE_DISCOVERABLE_SIGNIN {}",
        serde_json::to_string(&observations).unwrap()
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn native_registration_opt_in_preserves_legacy_options_and_authority(pool: PgPool) {
    let signing = SigningKey::random(&mut OsRng);
    let private = signing.to_pkcs8_pem(LineEnding::LF).unwrap().to_string();
    let public = signing
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let runtime = runtime_login(&pool).await;
    let router = build_router(app_state(runtime.clone(), private, public).unwrap());
    let branch = seed_branch(&pool, "Discoverable Region", "Discoverable Branch").await;
    let user = seed_user_with_branch(
        &pool,
        "Discoverable User",
        "010-4800-0001",
        "MEMBER",
        branch,
    )
    .await;
    let token = admin_session_via_otp(&router, &runtime, user).await;
    let path = "/api/v1/auth/passkey/register/start";
    let before = enrollment_digest(&pool).await.unwrap();
    assert_eq!(
        post_raw(
            router.clone(),
            path,
            None,
            json!({"require_discoverable":true})
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    for value in [json!("true"), Value::Null, json!(1), json!({}), json!([])] {
        assert_eq!(
            post_raw(
                router.clone(),
                path,
                Some(&token),
                json!({"require_discoverable":value})
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let absent_json = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(absent_json.status(), StatusCode::BAD_REQUEST);
    let missing_content_type = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        missing_content_type.status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(
        post_raw(
            router.clone(),
            path,
            Some(&token),
            json!({"require_discoverable":true})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        enrollment_digest(&pool).await.unwrap(),
        before,
        "denial cannot create ceremony or custody effects"
    );
    accept_required_privacy_consent(&router, &token).await;
    for (input, resident, required) in [
        (json!({}), "discouraged", false),
        (json!({"require_discoverable":false}), "discouraged", false),
        (json!({"require_discoverable":true}), "required", true),
    ] {
        let response: Value =
            post_json(router.clone(), path, Some(&token), input, StatusCode::OK).await;
        assert_eq!(
            response["challenge"]["publicKey"]["authenticatorSelection"]["residentKey"],
            resident
        );
        assert_eq!(
            response["challenge"]["publicKey"]["authenticatorSelection"]["requireResidentKey"],
            required
        );
        assert_eq!(
            response["challenge"]["publicKey"]["authenticatorSelection"]["userVerification"],
            "required"
        );
        assert_eq!(
            response["challenge"]["publicKey"]["rp"]["id"],
            "example.com"
        );
        let ceremony = Uuid::parse_str(response["ceremony_id"].as_str().unwrap()).unwrap();
        let stored: Value = sqlx::query_scalar("SELECT challenge_json FROM auth_webauthn_ceremonies WHERE id=$1 AND user_id=$2 AND consumed_at IS NULL")
            .bind(ceremony).bind(user.as_uuid()).fetch_one(&pool).await.unwrap();
        assert_eq!(stored, response["challenge"]);
    }
    // This fixture verifies old-key cryptographic compatibility only; the real
    // browser scenario above does not inject credentials or rewrite options.
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = enroll_passkey(&router, &mut authenticator, &token).await;
    let session = usernameless_login(&router, &mut authenticator, &credential).await;
    let before = enrollment_digest(&pool).await.unwrap();
    assert_eq!(
        post_raw(
            router.clone(),
            path,
            Some(&session.access_token),
            json!({"require_discoverable":true})
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(enrollment_digest(&pool).await.unwrap(), before);
    let proof = start_step_up_assertion(&router, &mut authenticator, &credential).await;
    let response: Value = post_json(
        router,
        path,
        Some(&session.access_token),
        json!({"require_discoverable":true,"step_up":proof}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        response["challenge"]["publicKey"]["authenticatorSelection"]["residentKey"],
        "required"
    );
    assert!(
        response["challenge"]["publicKey"]["excludeCredentials"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == credential)
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn native_explicit_login_is_bodyless_and_shares_conditional_limits(pool: PgPool) {
    let signing = SigningKey::random(&mut OsRng);
    let private = signing.to_pkcs8_pem(LineEnding::LF).unwrap().to_string();
    let public = signing
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let runtime = runtime_login(&pool).await;
    let router = build_router(app_state_with_trusted_proxy(runtime, private, public).unwrap());
    let request = |path: &str, method: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .extension(ConnectInfo("10.0.0.5:12345".parse::<SocketAddr>().unwrap()))
            .header("x-forwarded-for", "203.0.113.44")
            .header("x-device-id", "passkey-mode-probe")
            .body(Body::empty())
            .unwrap()
    };
    let paths = [
        "/api/v1/auth/passkey/login/start",
        "/api/v1/auth/passkey/login/explicit/start",
    ];
    for path in paths {
        let response = router.clone().oneshot(request(path, "POST")).await.unwrap();
        let response: Value = response.into_json(StatusCode::OK).await;
        let options = &response["challenge"];
        assert_eq!(options["publicKey"]["allowCredentials"], json!([]));
        assert_eq!(options["publicKey"]["userVerification"], "required");
        assert_eq!(options["publicKey"]["rpId"], "example.com");
        if path == paths[0] {
            assert_eq!(options["mediation"], "conditional");
        } else {
            assert!(options.get("mediation").is_none());
        }
        let ceremony = Uuid::parse_str(response["ceremony_id"].as_str().unwrap()).unwrap();
        let row: (Option<Uuid>, Value, OffsetDateTime, Option<OffsetDateTime>) = sqlx::query_as(
            "SELECT user_id,challenge_json,expires_at,consumed_at FROM auth_webauthn_ceremonies WHERE id=$1 AND ceremony_kind='authentication'"
        ).bind(ceremony).fetch_one(&pool).await.unwrap();
        assert_eq!(row.0, None);
        assert_eq!(&row.1, options);
        assert!(row.2 > OffsetDateTime::now_utc());
        assert_eq!(row.3, None);
        let wire_expiry = OffsetDateTime::parse(
            response["expires_at"].as_str().unwrap(),
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        // PostgreSQL's codec stores microseconds; the owner emits RFC3339 nanos.
        assert_eq!(
            wire_expiry.unix_timestamp_nanos() / 1000,
            row.2.unix_timestamp_nanos() / 1000
        );
        let wrong_method = router.clone().oneshot(request(path, "GET")).await.unwrap();
        assert_eq!(wrong_method.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
    let counters: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT client_key,endpoint,sum(attempts)::bigint FROM auth_rate_limit GROUP BY client_key,endpoint ORDER BY client_key,endpoint"
    ).fetch_all(&pool).await.unwrap();
    assert_eq!(
        counters,
        vec![
            ("dev:passkey-mode-probe".into(), "login_start".into(), 2),
            ("global".into(), "login_start".into(), 2),
            ("ip:203.0.113.44".into(), "login_start".into(), 2)
        ],
        "mode changes retain the same trusted-IP, device and global rate buckets"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_webauthn_ceremonies")
        .fetch_one(&pool)
        .await
        .unwrap();
    for (key, cap) in [
        ("ip:203.0.113.44", 10),
        ("dev:passkey-mode-probe", 10),
        ("global", 100),
    ] {
        sqlx::query("UPDATE auth_rate_limit SET attempts=0")
            .execute(&pool)
            .await
            .unwrap();
        // Cover both possible clock windows without changing the serving clock.
        sqlx::query("INSERT INTO auth_rate_limit(client_key,endpoint,window_start,attempts)
            SELECT $1,'login_start',date_trunc('minute',clock_timestamp())+i*interval '1 minute',$2 FROM generate_series(0,1) i
            ON CONFLICT(client_key,endpoint,window_start) DO UPDATE SET attempts=EXCLUDED.attempts")
            .bind(key).bind(cap).execute(&pool).await.unwrap();
        for path in paths {
            let response = router.clone().oneshot(request(path, "POST")).await.unwrap();
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM auth_webauthn_ceremonies")
                .fetch_one(&pool)
                .await
                .unwrap(),
            count
        );
    }
}
