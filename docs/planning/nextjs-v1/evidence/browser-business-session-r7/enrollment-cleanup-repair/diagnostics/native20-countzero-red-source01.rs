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
    max_frames: u64,
    phase: StdDuration,
}

impl Browser {
    fn start(root: PathBuf) -> ProbeResult<Self> {
        for file in [
            ".next/BUILD_ID",
            "package-lock.json",
            "tools/production-runtime.mjs",
        ] {
            if !root.join(file).is_file() {
                return Err("infrastructure: production build/packager prerequisite missing");
            }
        }
        Self::start_runner(root, "test-native-browser-enrollment.mjs")
    }

    fn start_product(root: PathBuf) -> ProbeResult<Self> {
        for file in [
            ".next/BUILD_ID",
            "package-lock.json",
            "tools/production-runtime.mjs",
        ] {
            if !root.join(file).is_file() {
                return Err("infrastructure: product build/packager prerequisite missing");
            }
        }
        Self::start_runner(root, "test-browser-business-session.mjs")
    }

    fn start_runner(root: PathBuf, runner: &str) -> ProbeResult<Self> {
        let mut command = Command::new("node");
        command
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../tools")
                    .join(runner),
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
            max_frames: if runner == "test-browser-business-session.mjs" {
                2048
            } else {
                MAX_FRAMES
            },
            phase: if runner == "test-browser-business-session.mjs" {
                StdDuration::from_secs(90)
            } else {
                PHASE
            },
        })
    }

    async fn read_frame(&mut self) -> ProbeResult<Value> {
        if self.received >= self.max_frames {
            return Err("infrastructure: frame count");
        }
        let frame = timeout(self.phase, async {
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
        Ok(frame)
    }

    async fn read(&mut self, kind: &str) -> ProbeResult<Value> {
        let frame = self.read_frame().await?;
        if frame["kind"] != kind {
            if frame["kind"] == "cleaned" {
                self.cleanup_ack = Some(frame);
            }
            return Err("infrastructure: browser phase ended before native progress");
        }
        Ok(frame)
    }

    async fn read_product(&mut self) -> ProbeResult<Value> {
        let frame = self.read_frame().await?;
        match frame["kind"].as_str() {
            Some("checkpoint" | "report" | "control") => Ok(frame),
            Some("cleaned") => {
                self.cleanup_ack = Some(frame);
                Err("infrastructure: product ended before protocol completion")
            }
            _ => Err("infrastructure: unsupported product frame"),
        }
    }

    async fn write(&mut self, mut frame: Value) -> ProbeResult<()> {
        if self.sent >= self.max_frames {
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
        self.reap_owned(phase_ok, phase_ok).await
    }

    async fn reap_product(
        &mut self,
        protocol_completed: bool,
        all_passed: bool,
    ) -> ProbeResult<()> {
        self.reap_owned(protocol_completed, all_passed).await
    }

    async fn reap_owned(&mut self, phase_ok: bool, all_passed: bool) -> ProbeResult<()> {
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
            Some("completed") if phase_ok && all_passed => 0,
            Some("aborted") if !phase_ok => 2,
            Some("failed") if !all_passed => 1,
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
        eprintln!(
            "NATIVE_BROWSER_HTTP_STATUS expected={} actual={}",
            status.as_u16(),
            response.status().as_u16()
        );
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
    registration_probe_inner(
        owner,
        runtime,
        browser,
        server,
        interrupt_after_creation,
        discoverable,
        false,
    )
    .await
}

async fn product_registration_probe(
    owner: &PgPool,
    runtime: &PgPool,
    browser: &mut Browser,
    server: &mut Option<tokio::task::JoinHandle<()>>,
    interrupt_after_creation: bool,
    discoverable: bool,
) -> ProbeResult<Vec<Value>> {
    registration_probe_inner(
        owner,
        runtime,
        browser,
        server,
        interrupt_after_creation,
        discoverable,
        true,
    )
    .await
}

async fn registration_probe_inner(
    owner: &PgPool,
    runtime: &PgPool,
    browser: &mut Browser,
    server: &mut Option<tokio::task::JoinHandle<()>>,
    interrupt_after_creation: bool,
    discoverable: bool,
    product: bool,
) -> ProbeResult<Vec<Value>> {
    let first = browser.read("origin").await?;
    let origin = first["origin"]
        .as_str()
        .ok_or("infrastructure: origin missing")?;
    let url = Url::parse(origin).map_err(|_| "infrastructure: origin invalid")?;
    if url.scheme() != if product { "https" } else { "http" }
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
    let storage_key = product_random_key();
    let ingress_key = URL_SAFE_NO_PAD.encode(product_random_key());
    let faults = ProductFaults::new(owner.clone());
    let mut pairs = vec![
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
    ];
    if product {
        pairs.extend([
            (
                "CONSOLE_BROWSER_SESSION_KEY_HEX",
                storage_key
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            ),
            ("CONSOLE_BROWSER_INGRESS_KEY", ingress_key.clone()),
            ("CONSOLE_TRUSTED_PROXY_COUNT", "1".into()),
            ("CONSOLE_TRUSTED_PROXY_CIDRS", "127.0.0.1/32".into()),
        ]);
    }
    let config =
        AppConfig::from_pairs(pairs.clone()).map_err(|_| "fixture: native configuration")?;
    let router = build_router(
        AppState::new(config, DatabaseDependency::Postgres(runtime.clone()))
            .map_err(|_| "fixture: native state")?,
    );
    let router = if product {
        router.layer(axum::middleware::from_fn_with_state(
            faults.clone(),
            product_response_gate,
        ))
    } else {
        router
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| "fixture: native listener")?;
    let native_origin = format!(
        "http://{}",
        listener
            .local_addr()
            .map_err(|_| "fixture: native address")?
    );
    let mut short_origin = None;
    let mut short_service = None;
    if product {
        let short_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| "fixture: short native listener")?;
        short_origin = Some(format!(
            "http://{}",
            short_listener
                .local_addr()
                .map_err(|_| "fixture: short native address")?
        ));
        pairs.push(("CONSOLE_REFRESH_FAMILY_ABSOLUTE_TTL_SECS", "6".into()));
        let short_config =
            AppConfig::from_pairs(pairs).map_err(|_| "fixture: real short-family configuration")?;
        let short_router = build_router(
            AppState::new(short_config, DatabaseDependency::Postgres(runtime.clone()))
                .map_err(|_| "fixture: short native state")?,
        )
        .layer(axum::middleware::from_fn_with_state(
            faults.clone(),
            product_response_gate,
        ));
        short_service = Some((short_listener, short_router));
    }
    *server = Some(tokio::spawn(async move {
        let mut owned = tokio::task::JoinSet::new();
        owned.spawn(async move {
            let _ = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });
        if let Some((listener, router)) = short_service {
            owned.spawn(async move {
                let _ = axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .await;
            });
        }
        while owned.join_next().await.is_some() {}
    }));
    if product {
        let (certificate, key) = product_tls()?;
        browser.write(json!({"kind":"configure", "backend_origin":native_origin,
            "short_backend_origin":short_origin,"ingress_key":ingress_key,"tls_cert_pem":certificate,"tls_key_pem":key})).await?;
    }
    let ready = browser.read("ready").await?;
    if ready["origin"] != origin
        || (product
            && (ready["runtime_digest"] != first["runtime_digest"]
                || first["runtime_digest"].as_str().is_none_or(|digest| {
                    digest.len() != 64
                        || !digest
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                })))
    {
        return Err("infrastructure: ready origin mismatch");
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(StdDuration::from_secs(10))
        .build()
        .map_err(|_| "fixture: HTTP client")?;
    let product_context = ProductProbeContext {
        owner,
        runtime,
        verifier: &verifier,
        storage_key: &storage_key,
        client: &client,
        native_origin: &native_origin,
        ingress_key: &ingress_key,
        faults: &faults,
    };
    let mut observations = Vec::new();
    let mut product_actors = Vec::new();
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
        if product {
            // Establish fixture authority before native issuance; a later bump
            // correctly invalidates the original signed access proof.
            sqlx::query("INSERT INTO subject_authz_versions(user_id,org_id,version,session_generation) VALUES($1,$2,1,1) ON CONFLICT(org_id,user_id) DO NOTHING")
                .bind(user).bind(org).execute(owner).await.map_err(|_|"fixture: actual authority-version baseline")?;
        }
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
            if product {
                product_actors.push(
                    product_facts(
                        &product_context,
                        browser,
                        ProductEnrollment {
                            actor,
                            user,
                            org,
                            source: passkey_id,
                            credential: credential_id,
                            token,
                        },
                    )
                    .await?,
                );
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
    if product {
        observations.push(product_protocol(&product_context, browser, &mut product_actors).await?);
    } else {
        browser.write(json!({"kind":"done"})).await?;
    }
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
    if let Err(reason) = &result {
        eprintln!(
            "NATIVE_BROWSER_FAILURE {reason} checked_frames={}",
            browser.received
        );
    }
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

// Test-only real-response custody. This middleware never calls an owner twice,
// edits an accepted mapping, or manufactures a successful owner response.
use axum::response::IntoResponse;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Mutex, Notify};

const PRODUCT_NATIVE_PATHS: [&str; 4] = [
    "/api/v1/auth/browser-session/start",
    "/api/v1/auth/browser-session/login",
    "/api/v1/auth/browser-session/logout",
    "/api/v1/hr/browser-session/attendance-records/me",
];

#[derive(Clone)]
struct ProductFaults {
    owner: PgPool,
    gates: Arc<Mutex<Vec<Arc<ProductGate>>>>,
    witnesses: Arc<Mutex<std::collections::BTreeMap<Uuid, Value>>>,
    request_witnesses: Arc<Mutex<std::collections::BTreeMap<(String, Uuid), Value>>>,
    authority_prior: Arc<Mutex<std::collections::BTreeMap<Uuid, ProductAuthorityPrior>>>,
}

#[derive(Default)]
struct ProductAuthorityPrior {
    active: Option<bool>,
    status: Option<String>,
}

struct ProductGate {
    id: Uuid,
    path: String,
    binding: Option<Uuid>,
    claimed: AtomicBool,
    held: Mutex<Option<Value>>,
    held_notify: Notify,
    release: Mutex<Option<String>>,
    release_notify: Notify,
}

impl ProductFaults {
    fn new(owner: PgPool) -> Self {
        Self {
            owner,
            gates: Arc::new(Mutex::new(Vec::new())),
            witnesses: Arc::new(Mutex::new(std::collections::BTreeMap::new())),
            request_witnesses: Arc::new(Mutex::new(std::collections::BTreeMap::new())),
            authority_prior: Arc::new(Mutex::new(std::collections::BTreeMap::new())),
        }
    }

    async fn arm(&self, id: Uuid, path: &str, binding: Option<Uuid>) -> ProbeResult<()> {
        if !PRODUCT_NATIVE_PATHS.contains(&path) {
            return Err("infrastructure: fault route outside allowlist");
        }
        let mut gates = self.gates.lock().await;
        if gates.len() >= 64 || gates.iter().any(|gate| gate.id == id) {
            return Err("infrastructure: fault admission");
        }
        gates.push(Arc::new(ProductGate {
            id,
            path: path.into(),
            binding,
            claimed: AtomicBool::new(false),
            held: Mutex::new(None),
            held_notify: Notify::new(),
            release: Mutex::new(None),
            release_notify: Notify::new(),
        }));
        Ok(())
    }

    async fn gate(&self, id: Uuid) -> ProbeResult<Arc<ProductGate>> {
        self.gates
            .lock()
            .await
            .iter()
            .find(|gate| gate.id == id)
            .cloned()
            .ok_or("infrastructure: unknown fault gate")
    }

    async fn wait(&self, id: Uuid) -> ProbeResult<Value> {
        let gate = self.gate(id).await?;
        timeout(PHASE, async {
            loop {
                let notified = gate.held_notify.notified();
                if let Some(value) = gate.held.lock().await.clone() {
                    return value;
                }
                notified.await;
            }
        })
        .await
        .map_err(|_| "behavior: actual native response did not reach armed gate")
    }

    async fn release(&self, id: Uuid, action: &str) -> ProbeResult<()> {
        if !["intact", "drop", "malformed-json", "malformed-clock"].contains(&action) {
            return Err("infrastructure: fault action outside allowlist");
        }
        let gate = self.gate(id).await?;
        if gate.held.lock().await.is_none() {
            return Err("infrastructure: release before actual response");
        }
        let mut released = gate.release.lock().await;
        if released.is_some() {
            return Err("infrastructure: duplicate fault release");
        }
        *released = Some(action.into());
        gate.release_notify.notify_one();
        Ok(())
    }
}

async fn product_response_gate(
    axum::extract::State(faults): axum::extract::State<ProductFaults>,
    request: Request<Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = request.uri().path().to_owned();
    if !PRODUCT_NATIVE_PATHS.contains(&path.as_str()) {
        return next.run(request).await;
    }
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0.to_string());
    let forwarded = request
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let (device_id, traceparent, user_agent) = {
        let header_text = |name: &str| {
            request
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        (
            header_text("x-device-id"),
            header_text("traceparent"),
            header_text("user-agent"),
        )
    };
    let (parts, body) = request.into_parts();
    // Preserve the exact incoming bytes and headers before the actual owner.
    let bytes = match to_bytes(body, MAX_FRAME).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let binding = value["ceremony_id"]
        .as_str()
        .or_else(|| value["browser_context"].as_str())
        .and_then(|value| Uuid::parse_str(value).ok());
    let gate = faults
        .gates
        .lock()
        .await
        .iter()
        .find(|gate| {
            gate.path == path
                && (gate.binding.is_none() || gate.binding == binding)
                && gate
                    .claimed
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
        })
        .cloned();
    let response = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, MAX_FRAME).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let result: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if let Some(ceremony) = result["ceremony_id"]
        .as_str()
        .and_then(|value| Uuid::parse_str(value).ok())
    {
        faults.witnesses.lock().await.insert(
            ceremony,
            json!({"ceremony":ceremony,
            "native_peer":peer,"forwarded":forwarded}),
        );
    }
    let witness_context = result["context_id"]
        .as_str()
        .or_else(|| result["ceremony_id"].as_str())
        .or_else(|| value["browser_context"].as_str())
        .or_else(|| value["ceremony_id"].as_str())
        .and_then(|value| Uuid::parse_str(value).ok());
    if let Some(context) = witness_context {
        let witness = json!({"path":path,"context":context,"status":parts.status.as_u16(),
            "native_peer":peer,"forwarded":forwarded,"device_id":device_id,
            "traceparent":traceparent,"user_agent":user_agent});
        let mut witnesses = faults.request_witnesses.lock().await;
        if witnesses.len() >= 1024 && !witnesses.contains_key(&(path.clone(), context)) {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        witnesses.insert((path.clone(), context), witness);
    }
    let Some(gate) = gate else {
        return axum::response::Response::from_parts(parts, Body::from(bytes));
    };
    let context = result["context_id"]
        .as_str()
        .or_else(|| value["browser_context"].as_str())
        .or_else(|| {
            if path == PRODUCT_NATIVE_PATHS[1] {
                value["ceremony_id"].as_str()
            } else {
                None
            }
        });
    let ceremony = result["ceremony_id"].as_str();
    let committed = match context.and_then(|value| Uuid::parse_str(value).ok()) {
        Some(context) => match product_committed_observation(&faults.owner, context).await {
            Ok(value) => value,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        None => Value::Null,
    };
    let signed_counter = value["credential"]["response"]["authenticatorData"]
        .as_str()
        .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
        .filter(|bytes| bytes.len() >= 37)
        .map(|bytes| u32::from_be_bytes(bytes[33..37].try_into().unwrap()));
    if path == PRODUCT_NATIVE_PATHS[1]
        && parts.status.is_success()
        && committed["source_counter"].as_u64() != signed_counter.map(u64::from)
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    *gate.held.lock().await = Some(json!({"gate":gate.id,"status":parts.status.as_u16(),
        "context":context,"ceremony":ceremony,"expires":result["expires_at"],
        "signed_counter":signed_counter,"committed":committed}));
    gate.held_notify.notify_one();
    let action = timeout(StdDuration::from_secs(90), async {
        loop {
            let notified = gate.release_notify.notified();
            if let Some(action) = gate.release.lock().await.clone() {
                return action;
            }
            notified.await;
        }
    })
    .await
    .unwrap_or_else(|_| "drop".into());
    match action.as_str() {
        "intact" => axum::response::Response::from_parts(parts, Body::from(bytes)),
        "drop" => {
            // The real status/headers remain, but the actual body connection is
            // lost. Never fabricate a successful JSON payload or claim receipt.
            let stream = futures::stream::once(async {
                Err::<axum::body::Bytes, std::io::Error>(std::io::Error::new(
                    std::io::ErrorKind::ConnectionAborted,
                    "fixture dropped actual response",
                ))
            });
            axum::response::Response::from_parts(parts, Body::from_stream(stream))
        }
        "malformed-json" => {
            let malformed = bytes
                .get(..bytes.len().saturating_sub(1))
                .unwrap_or_default()
                .to_vec();
            parts.headers.remove(header::CONTENT_LENGTH);
            axum::response::Response::from_parts(parts, Body::from(malformed))
        }
        "malformed-clock" => {
            let mut malformed = result;
            if let Some(item) = malformed["history"]["items"]
                .as_array_mut()
                .and_then(|items| items.first_mut())
            {
                item["occurred_at"] = json!([2026, 367, 0, 0, 0, 0, 9, 0, 0]);
            }
            parts.headers.remove(header::CONTENT_LENGTH);
            axum::response::Response::from_parts(parts, Body::from(malformed.to_string()))
        }
        _ => unreachable!("closed fixture action"),
    }
}

fn product_control_fields(frame: &Value, fields: &[&str]) -> ProbeResult<()> {
    let expected = ["id", "kind", "op", "seq", "v"]
        .into_iter()
        .chain(fields.iter().copied())
        .collect::<std::collections::BTreeSet<_>>();
    let actual = frame
        .as_object()
        .ok_or("infrastructure: control object")?
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    if actual != expected {
        return Err("infrastructure: unknown control field");
    }
    Ok(())
}

fn product_control_uuid(frame: &Value, field: &str) -> ProbeResult<Uuid> {
    let text = frame[field]
        .as_str()
        .ok_or("infrastructure: control UUID missing")?;
    let id = Uuid::parse_str(text).map_err(|_| "infrastructure: control UUID invalid")?;
    if id.to_string() != text {
        return Err("infrastructure: control UUID noncanonical");
    }
    Ok(id)
}

async fn product_committed_observation(owner: &PgPool, context: Uuid) -> ProbeResult<Value> {
    let observation: (bool,bool,bool,bool,i64,Option<i64>) = sqlx::query_as(
        "SELECT closed_at IS NULL,
            EXISTS(SELECT 1 FROM auth_refresh_token_families f WHERE f.id=s.family_id AND f.user_id=s.account_id AND f.org_id=s.company_id AND f.revoked_at IS NULL),
            EXISTS(SELECT 1 FROM auth_webauthn_ceremonies c WHERE c.id=s.context_id AND c.consumed_at IS NOT NULL),
            EXISTS(SELECT 1 FROM auth_webauthn_credentials k WHERE k.id=s.source_credential_id AND k.user_id=s.account_id AND k.org_id=s.company_id),
            EXTRACT(EPOCH FROM s.expires_at)::bigint,
            (SELECT (k.passkey_json->'cred'->>'counter')::bigint FROM auth_webauthn_credentials k WHERE k.id=s.source_credential_id)
            FROM auth_security.browser_sessions s WHERE context_id=$1")
        .bind(context).fetch_one(owner).await.map_err(|_| "native: committed response custody missing")?;
    Ok(
        json!({"mapping_exists":true,"mapping_open":observation.0,"family_open":observation.1,
        "ceremony_consumed":observation.2,"source_exists":observation.3,"expires":observation.4,"source_counter":observation.5}),
    )
}

struct ProductProbeContext<'a> {
    owner: &'a PgPool,
    runtime: &'a PgPool,
    verifier: &'a console_platform_auth::JwtVerifier,
    storage_key: &'a [u8; 32],
    client: &'a reqwest::Client,
    native_origin: &'a str,
    ingress_key: &'a str,
    faults: &'a ProductFaults,
}

struct ProductEnrollment<'a> {
    actor: &'a str,
    user: Uuid,
    org: Uuid,
    source: Uuid,
    credential: &'a str,
    token: &'a str,
}

async fn product_control(
    context: &ProductProbeContext<'_>,
    actors: &mut [ProductActor],
    request: &Value,
) -> ProbeResult<Value> {
    let ProductProbeContext {
        owner,
        runtime,
        faults,
        client,
        native_origin,
        ingress_key,
        ..
    } = *context;
    let _id = product_control_uuid(request, "id")?;
    let op = request["op"]
        .as_str()
        .ok_or("infrastructure: control op missing")?;
    match op {
        "effects" => {
            product_control_fields(request, &[])?;
            let digest = product_state_digest(&product_custody_state(owner).await?)?;
            Ok(json!({"digest":digest.iter().map(|byte|format!("{byte:02x}")).collect::<String>()}))
        }
        "fixture-authority" => {
            product_control_fields(request, &["actor", "context", "change"])?;
            let actor = actors
                .iter()
                .find(|a| request["actor"] == a.actor)
                .ok_or("infrastructure: authority actor")?;
            let context = product_control_uuid(request, "context")?;
            let exact: (Uuid,Uuid,bool)=sqlx::query_as("SELECT account_id,company_id,closed_at IS NULL FROM auth_security.browser_sessions WHERE context_id=$1")
                .bind(context).fetch_one(owner).await.map_err(|_|"native: authority genuine context")?;
            if (exact.0, exact.1) != (actor.user, actor.org) || !exact.2 {
                return Err("native: authority exact fixture context");
            }
            let change = request["change"]
                .as_str()
                .ok_or("infrastructure: authority change")?;
            if ![
                "account-suspend",
                "account-restore",
                "company-suspend",
                "company-restore",
                "session-generation-advance",
                "subject-version-advance",
            ]
            .contains(&change)
            {
                return Err("infrastructure: authority fault outside allowlist");
            }
            let before = product_authority_preserved(owner, actor.user, actor.org, change).await?;
            let mut prior = faults.authority_prior.lock().await;
            let remembered = prior.entry(actor.user).or_default();
            let mut tx = owner
                .begin()
                .await
                .map_err(|_| "fixture: authority transaction")?;
            sqlx::query("SELECT set_config('app.current_org',$1,true)")
                .bind(actor.org.to_string())
                .execute(tx.as_mut())
                .await
                .map_err(|_| "fixture: authority Company scope")?;
            match change {
                "account-suspend" => {
                    if remembered.active.is_some() {
                        return Err("infrastructure: duplicate account fault");
                    }
                    let active: bool = sqlx::query_scalar(
                        "SELECT is_active FROM users WHERE id=$1 AND org_id=$2 FOR UPDATE",
                    )
                    .bind(actor.user)
                    .bind(actor.org)
                    .fetch_one(tx.as_mut())
                    .await
                    .map_err(|_| "fixture: exact active row")?;
                    if !active {
                        return Err("fixture: account already inactive");
                    }
                    let changed:bool=sqlx::query_scalar("UPDATE users SET is_active=false WHERE id=$1 AND org_id=$2 RETURNING is_active")
                        .bind(actor.user).bind(actor.org).fetch_one(tx.as_mut()).await.map_err(|_|"fixture: suspend exact Account")?;
                    if changed {
                        return Err("fixture: Account suspension did not apply");
                    }
                    remembered.active = Some(active);
                }
                "account-restore" => {
                    let active = remembered
                        .active
                        .ok_or("infrastructure: no captured account fault")?;
                    let changed:bool=sqlx::query_scalar("UPDATE users SET is_active=$3 WHERE id=$1 AND org_id=$2 AND is_active=false RETURNING is_active")
                        .bind(actor.user).bind(actor.org).bind(active).fetch_one(tx.as_mut()).await.map_err(|_|"fixture: restore captured Account state")?;
                    if changed != active {
                        return Err("fixture: Account restore mismatch");
                    }
                    remembered.active = None;
                }
                "company-suspend" => {
                    if remembered.status.is_some() {
                        return Err("infrastructure: duplicate Company fault");
                    }
                    let status: String = sqlx::query_scalar(
                        "SELECT status FROM organizations WHERE id=$1 FOR UPDATE",
                    )
                    .bind(actor.org)
                    .fetch_one(tx.as_mut())
                    .await
                    .map_err(|_| "fixture: exact Company state")?;
                    if status != "ACTIVE" {
                        return Err("fixture: Company already inactive");
                    }
                    let changed: String = sqlx::query_scalar(
                        "UPDATE organizations SET status='SUSPENDED' WHERE id=$1 RETURNING status",
                    )
                    .bind(actor.org)
                    .fetch_one(tx.as_mut())
                    .await
                    .map_err(|_| "fixture: suspend exact Company")?;
                    if changed != "SUSPENDED" {
                        return Err("fixture: Company suspension did not apply");
                    }
                    remembered.status = Some(status);
                }
                "company-restore" => {
                    let status = remembered
                        .status
                        .as_ref()
                        .ok_or("infrastructure: no captured Company fault")?;
                    let changed:String=sqlx::query_scalar("UPDATE organizations SET status=$2 WHERE id=$1 AND status='SUSPENDED' RETURNING status")
                        .bind(actor.org).bind(status).fetch_one(tx.as_mut()).await.map_err(|_|"fixture: restore captured Company state")?;
                    if &changed != status {
                        return Err("fixture: Company restore mismatch");
                    }
                    remembered.status = None;
                }
                "session-generation-advance" | "subject-version-advance" => {
                    let old:(i64,i64)=sqlx::query_as("SELECT version,session_generation FROM subject_authz_versions WHERE user_id=$1 AND org_id=$2 FOR UPDATE")
                        .bind(actor.user).bind(actor.org).fetch_one(tx.as_mut()).await.map_err(|_|"fixture: exact freshness row")?;
                    let query = if change == "session-generation-advance" {
                        "UPDATE subject_authz_versions SET session_generation=session_generation+1,updated_at=clock_timestamp() WHERE user_id=$1 AND org_id=$2 RETURNING version,session_generation"
                    } else {
                        "UPDATE subject_authz_versions SET version=version+1,updated_at=clock_timestamp() WHERE user_id=$1 AND org_id=$2 RETURNING version,session_generation"
                    };
                    let changed: (i64, i64) = sqlx::query_as(query)
                        .bind(actor.user)
                        .bind(actor.org)
                        .fetch_one(tx.as_mut())
                        .await
                        .map_err(|_| "fixture: forward-only freshness fault")?;
                    let expected = if change == "session-generation-advance" {
                        (
                            old.0,
                            old.1.checked_add(1).ok_or("fixture: generation bound")?,
                        )
                    } else {
                        (old.0.checked_add(1).ok_or("fixture: version bound")?, old.1)
                    };
                    if changed != expected {
                        return Err("fixture: forward-only freshness mismatch");
                    }
                }
                _ => unreachable!("closed fixture authority fault"),
            }
            tx.commit()
                .await
                .map_err(|_| "fixture: authority fault commit")?;
            if product_authority_preserved(owner, actor.user, actor.org, change).await? != before {
                return Err("native: fixture authority fault changed unrelated custody");
            }
            Ok(json!({"actor":actor.actor,"context":context,"change":change,"applied":true}))
        }
        "fault-arm" => {
            product_control_fields(request, &["gate", "path", "binding"])?;
            let gate = product_control_uuid(request, "gate")?;
            let binding = if request["binding"].is_null() {
                None
            } else {
                Some(product_control_uuid(request, "binding")?)
            };
            let path = request["path"]
                .as_str()
                .ok_or("infrastructure: fault path missing")?;
            faults.arm(gate, path, binding).await?;
            Ok(json!({"gate":gate}))
        }
        "fault-wait" => {
            product_control_fields(request, &["gate"])?;
            let gate = product_control_uuid(request, "gate")?;
            faults.wait(gate).await
        }
        "fault-release" => {
            product_control_fields(request, &["gate", "action"])?;
            let gate = product_control_uuid(request, "gate")?;
            faults
                .release(
                    gate,
                    request["action"]
                        .as_str()
                        .ok_or("infrastructure: action missing")?,
                )
                .await?;
            Ok(json!({"gate":gate,"released":true}))
        }
        "native-revoke" => {
            product_control_fields(request, &["actor", "context", "session_token"])?;
            let actor = actors
                .iter()
                .find(|a| request["actor"] == a.actor)
                .ok_or("infrastructure: revoke actor")?;
            let context = product_control_uuid(request, "context")?;
            let binding: (Uuid,Uuid,Uuid) = sqlx::query_as("SELECT account_id,company_id,family_id FROM auth_security.browser_sessions WHERE context_id=$1")
                .bind(context).fetch_one(owner).await.map_err(|_| "native: revoke exact context")?;
            if (binding.0, binding.1) != (actor.user, actor.org) {
                return Err("native: revoke context identity mismatch");
            }
            let handle = request["session_token"]
                .as_str()
                .ok_or("infrastructure: revoke handle")?;
            let before = product_unrelated_effects(owner, context, binding.2).await?;
            let response = client
                .post(format!("{native_origin}{}", PRODUCT_NATIVE_PATHS[2]))
                .header("x-console-browser-ingress", format!("bi1.{ingress_key}"))
                .header("x-forwarded-for", "127.0.0.1")
                .header(header::CONTENT_TYPE, "application/json")
                .body(json!({"session_token":handle,"browser_context":context}).to_string())
                .send()
                .await
                .map_err(|_| "native: actual revoke owner request")?;
            if response.status() != StatusCode::NO_CONTENT
                || !response
                    .bytes()
                    .await
                    .map_err(|_| "native: revoke bytes")?
                    .is_empty()
            {
                return Err("native: actual revoke owner not confirmed");
            }
            let result = product_committed_observation(owner, context).await?;
            if result["mapping_open"] != false || result["family_open"] != false {
                return Err("native: owner revoke did not close exact family");
            }
            let exact: (i64,bool,i64,bool)=sqlx::query_as(
                "SELECT (SELECT count(*) FROM auth_refresh_tokens WHERE family_id=$1),
                    (SELECT coalesce(bool_and(revoked_at IS NOT NULL),false) FROM auth_refresh_tokens WHERE family_id=$1),
                    (SELECT count(*) FROM audit_events WHERE action='auth.logout' AND target_id=$1::text AND actor=$3 AND org_id=$4),
                    (SELECT closed_at IS NOT NULL AND ciphertext IS NULL AND nonce IS NULL AND tag IS NULL AND owner_removed_at IS NULL FROM auth_security.browser_sessions WHERE context_id=$2)")
                .bind(binding.2).bind(context).bind(actor.user).bind(actor.org).fetch_one(owner).await
                .map_err(|_|"native: exact revoke tokens/audit/custody")?;
            if exact.0 < 1 || !exact.1 || exact.2 != 1 || !exact.3 {
                return Err("native: exact revoke token/audit/custody effect mismatch");
            }
            if product_unrelated_effects(owner, context, binding.2).await? != before {
                return Err("native: revoke changed unrelated native facts");
            }
            let mut result = result;
            result["status"] = json!(204);
            result["tokens_revoked"] = json!(exact.0);
            result["logout_audits"] = json!(exact.2);
            result["unrelated_unchanged"] = json!(true);
            Ok(result)
        }
        "cleanup" => {
            product_control_fields(request, &["actor"])?;
            let actor = actors
                .iter()
                .find(|a| request["actor"] == a.actor)
                .ok_or("infrastructure: cleanup actor")?;
            let mut tx = runtime
                .begin()
                .await
                .map_err(|_| "native: cleanup transaction")?;
            sqlx::query("SELECT set_config('app.current_org',$1,true)")
                .bind(actor.org.to_string())
                .execute(tx.as_mut())
                .await
                .map_err(|_| "native: cleanup Company scope")?;
            let cleared: i64 =
                sqlx::query_scalar("SELECT public.platform_browser_session_cleanup($1,$2)")
                    .bind(actor.org)
                    .bind(64_i32)
                    .fetch_one(tx.as_mut())
                    .await
                    .map_err(|_| "native: cleanup owner")?;
            tx.commit().await.map_err(|_| "native: cleanup commit")?;
            Ok(json!({"cleared":cleared}))
        }
        "fixture-history" => {
            product_control_fields(request, &["actor", "mode"])?;
            let actor = actors
                .iter_mut()
                .find(|a| request["actor"] == a.actor)
                .ok_or("infrastructure: fixture actor")?;
            let linked = match request["mode"].as_str() {
                Some("populated") => Some(actor.employee),
                Some("unlinked") => None,
                Some("linked-empty") => {
                    let employee = Uuid::new_v4();
                    sqlx::query("INSERT INTO employees(id,org_id,company,name,source_filename,source_sheet,source_row,source_key,raw_row,source_metadata) VALUES($1,$2,'테스트',$3,'browser-test.xlsx','직원',2,$4,'{}','{}')")
                        .bind(employee).bind(actor.org).bind(format!("브라우저 {} 사용자",actor.actor))
                        .bind(format!("browser-{employee}")).execute(owner).await.map_err(|_| "fixture: real empty employee")?;
                    Some(employee)
                }
                _ => return Err("infrastructure: fixture history mode"),
            };
            sqlx::query("UPDATE users SET employee_id=$1 WHERE id=$2 AND org_id=$3")
                .bind(linked)
                .bind(actor.user)
                .bind(actor.org)
                .execute(owner)
                .await
                .map_err(|_| "fixture: actual disposable employee linkage")?;
            let first =
                product_legacy_page(client, native_origin, &actor.fixture_bearer, 0).await?;
            let second =
                product_legacy_page(client, native_origin, &actor.fixture_bearer, 25).await?;
            Ok(
                json!({"actor":actor.actor,"company_name":format!("브라우저 {} 법인",actor.actor),
                "account_name":format!("브라우저 {} 사용자",actor.actor),"employee_linked":linked.is_some(),
                "first_page":first,"second_page":second}),
            )
        }
        "inspect-ip" => {
            product_control_fields(request, &["ceremony"])?;
            let ceremony = product_control_uuid(request, "ceremony")?;
            faults
                .witnesses
                .lock()
                .await
                .get(&ceremony)
                .cloned()
                .ok_or("native: actual ceremony/socket witness missing")
        }
        "inspect-request" => {
            product_control_fields(request, &["path", "context"])?;
            let path = request["path"]
                .as_str()
                .ok_or("infrastructure: request witness path")?;
            if !PRODUCT_NATIVE_PATHS.contains(&path) {
                return Err("infrastructure: witness path outside allowlist");
            }
            let context = product_control_uuid(request, "context")?;
            let mut witness = faults
                .request_witnesses
                .lock()
                .await
                .get(&(path.to_owned(), context))
                .cloned()
                .ok_or("native: actual route/context socket witness missing")?;
            witness["audit"] = Value::Null;
            if path == PRODUCT_NATIVE_PATHS[1] || path == PRODUCT_NATIVE_PATHS[2] {
                let binding:(Uuid,Uuid,Uuid)=sqlx::query_as("SELECT account_id,company_id,family_id FROM auth_security.browser_sessions WHERE context_id=$1")
                    .bind(context).fetch_one(owner).await.map_err(|_|"native: request witness exact mapping")?;
                let action = if path == PRODUCT_NATIVE_PATHS[1] {
                    "auth.login"
                } else {
                    "auth.logout"
                };
                let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('ip',ip,'device',device,'trace_id',trace_id,'span_id',span_id,'auth_method',auth_method,'user_agent',user_agent) FROM audit_events WHERE actor=$1 AND org_id=$2 AND action=$3 AND (($3='auth.login' AND after_snap->>'refresh_family_id'=$4::text) OR ($3='auth.logout' AND target_id=$4::text)) ORDER BY id")
                    .bind(binding.0).bind(binding.1).bind(action).bind(binding.2).fetch_all(owner).await
                    .map_err(|_|"native: actual correlated request audit metadata")?;
                let mut audit=rows.first().cloned().unwrap_or_else(||json!({"ip":null,"device":null,"trace_id":null,"span_id":null,"auth_method":null,"user_agent":null}));
                audit["count"] = json!(rows.len());
                witness["audit"] = audit;
            }
            Ok(witness)
        }
        "rate-status" => {
            product_control_fields(request, &["device"])?;
            let device = if request["device"].is_null() {
                ""
            } else {
                request["device"]
                    .as_str()
                    .ok_or("infrastructure: rate device")?
            };
            if device.len() > 128
                || !device
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
            {
                return Err("infrastructure: invalid bounded rate device");
            }
            let now: i64 =
                sqlx::query_scalar("SELECT floor(EXTRACT(EPOCH FROM clock_timestamp()))::bigint")
                    .fetch_one(owner)
                    .await
                    .map_err(|_| "native: real rate clock")?;
            let window = now - now.rem_euclid(60);
            let at = OffsetDateTime::from_unix_timestamp(window)
                .map_err(|_| "native: real rate window")?;
            let keys = vec![
                "ip:127.0.0.1".to_owned(),
                "ip:::1".to_owned(),
                "global".to_owned(),
                format!("dev:{device}"),
            ];
            let rows:Vec<(String,i32)>=sqlx::query_as("SELECT client_key,attempts FROM auth_rate_limit WHERE endpoint='login_start' AND window_start=$1 AND client_key=ANY($2)")
                .bind(at).bind(&keys).fetch_all(owner).await.map_err(|_|"native: actual rate buckets")?;
            let mut attempts = keys
                .into_iter()
                .map(|key| (key, 0_i32))
                .collect::<std::collections::BTreeMap<_, _>>();
            attempts.extend(rows);
            Ok(json!({"now":now,"window_start":window,"window_end":window+60,"attempts":attempts}))
        }
        _ => Err("infrastructure: control op outside allowlist"),
    }
}

async fn product_unrelated_effects(
    owner: &PgPool,
    context: Uuid,
    family: Uuid,
) -> ProbeResult<[u8; 32]> {
    let mut state = product_custody_state(owner).await?;
    for (table, field, id) in [
        ("mappings", "context_id", context),
        ("families", "id", family),
        ("tokens", "family_id", family),
    ] {
        if let Some(rows) = state[table].as_array_mut() {
            rows.retain(|row| row[field] != id.to_string());
        }
    }
    if let Some(rows) = state["audits"].as_array_mut() {
        rows.retain(|row| {
            !(row["action"] == "auth.logout" && row["target_id"] == family.to_string())
        });
    }
    product_state_digest(&state)
}

async fn product_custody_state(owner: &PgPool) -> ProbeResult<Value> {
    // Same complete auth/Company custody scope as browser_sessions::custody_digest,
    // with persisted admission-rate state. Only its bounded digest crosses IPC.
    sqlx::query_scalar("SELECT jsonb_build_object(
        'mappings',(SELECT coalesce(jsonb_agg(to_jsonb(s) ORDER BY context_id),'[]'::jsonb) FROM auth_security.browser_sessions s),
        'families',(SELECT coalesce(jsonb_agg(to_jsonb(f) ORDER BY id),'[]'::jsonb) FROM auth_refresh_token_families f),
        'tokens',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY id),'[]'::jsonb) FROM auth_refresh_tokens t),
        'audits',(SELECT coalesce(jsonb_agg(to_jsonb(a) ORDER BY id),'[]'::jsonb) FROM audit_events a),
        'users',(SELECT coalesce(jsonb_agg(to_jsonb(u) ORDER BY id),'[]'::jsonb) FROM users u),
        'companies',(SELECT coalesce(jsonb_agg(to_jsonb(o) ORDER BY id),'[]'::jsonb) FROM organizations o),
        'groups',(SELECT coalesce(jsonb_agg(to_jsonb(g) ORDER BY id),'[]'::jsonb) FROM groups g),
        'membership',(SELECT coalesce(jsonb_agg(to_jsonb(m) ORDER BY group_id,org_id),'[]'::jsonb) FROM group_memberships m),
        'keys',(SELECT coalesce(jsonb_agg(to_jsonb(k) ORDER BY id),'[]'::jsonb) FROM auth_webauthn_credentials k),
        'bootstrap',(SELECT coalesce(jsonb_agg(to_jsonb(b) ORDER BY id),'[]'::jsonb) FROM auth_bootstrap_credentials b),
        'otp_sources',(SELECT coalesce(jsonb_agg(to_jsonb(b) ORDER BY family_id),'[]'::jsonb) FROM auth_legacy_otp_family_sources b),
        'registration_bindings',(SELECT coalesce(jsonb_agg(to_jsonb(b) ORDER BY ceremony_id),'[]'::jsonb) FROM auth_legacy_registration_bindings b),
        'ceremony_bindings',(SELECT coalesce(jsonb_agg(to_jsonb(b) ORDER BY ceremony_id),'[]'::jsonb) FROM auth_webauthn_ceremony_bindings b),
        'ceremonies',(SELECT coalesce(jsonb_agg(to_jsonb(c) ORDER BY id),'[]'::jsonb) FROM auth_webauthn_ceremonies c),
        'freshness',(SELECT coalesce(jsonb_agg(to_jsonb(v) ORDER BY org_id,user_id),'[]'::jsonb) FROM subject_authz_versions v),
        'account_state',(SELECT coalesce(jsonb_agg(to_jsonb(s) ORDER BY account_id),'[]'::jsonb) FROM auth_security.account_state s),
        'reservations',(SELECT coalesce(jsonb_agg(to_jsonb(r) ORDER BY account_id),'[]'::jsonb) FROM auth_security.account_id_reservations r),
        'removals',(SELECT coalesce(jsonb_agg(to_jsonb(r) ORDER BY event_id),'[]'::jsonb) FROM auth_security.credential_removals r),
        'rates',(SELECT coalesce(jsonb_agg(to_jsonb(r) ORDER BY client_key,endpoint,window_start),'[]'::jsonb) FROM auth_rate_limit r))")
        .fetch_one(owner).await.map_err(|_|"native: complete bounded custody state")
}

fn product_state_digest(state: &Value) -> ProbeResult<[u8; 32]> {
    let bytes = serde_json::to_vec(state).map_err(|_| "native: custody digest codec")?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("infrastructure: bounded fixture custody census exceeded");
    }
    Ok(Sha256::digest(bytes).into())
}

async fn product_authority_preserved(
    owner: &PgPool,
    user: Uuid,
    org: Uuid,
    change: &str,
) -> ProbeResult<[u8; 32]> {
    let mut state = product_custody_state(owner).await?;
    let (table, id_field, id, fields) = match change {
        "account-suspend" | "account-restore" => {
            ("users", "id", user, &["is_active", "updated_at"][..])
        }
        "company-suspend" | "company-restore" => {
            ("companies", "id", org, &["status", "updated_at"][..])
        }
        "session-generation-advance" => (
            "freshness",
            "user_id",
            user,
            &["session_generation", "updated_at"][..],
        ),
        "subject-version-advance" => ("freshness", "user_id", user, &["version", "updated_at"][..]),
        _ => return Err("infrastructure: unknown authority preservation mask"),
    };
    for row in state[table]
        .as_array_mut()
        .into_iter()
        .flatten()
        .filter(|row| {
            row[id_field] == id.to_string()
                && (table != "freshness" || row["org_id"] == org.to_string())
        })
    {
        let row = row.as_object_mut().ok_or("native: custody row shape")?;
        for field in fields {
            row.remove(*field);
        }
    }
    product_state_digest(&state)
}

// Draft additions to genuine_browser.rs. Reuse the original C2 module helpers.
use p256::elliptic_curve::rand_core::RngCore;

struct ProductActor {
    actor: String,
    user: Uuid,
    org: Uuid,
    source: Uuid,
    credential: String,
    counter: i64,
    employee: Uuid,
    fixture_bearer: String,
}

fn product_random_key() -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

fn product_tls() -> ProbeResult<(String, String)> {
    use openssl::asn1::Asn1Time;
    use openssl::bn::{BigNum, MsbOption};
    use openssl::hash::MessageDigest;
    use openssl::pkey::PKey;
    use openssl::rsa::Rsa;
    use openssl::x509::{X509, X509NameBuilder, extension::SubjectAlternativeName};
    let key = PKey::from_rsa(Rsa::generate(2048).map_err(|_| "fixture: TLS key")?)
        .map_err(|_| "fixture: TLS private key")?;
    let mut name = X509NameBuilder::new().map_err(|_| "fixture: TLS name")?;
    name.append_entry_by_text("CN", "localhost")
        .map_err(|_| "fixture: TLS subject")?;
    let name = name.build();
    let mut builder = X509::builder().map_err(|_| "fixture: TLS certificate")?;
    builder.set_version(2).map_err(|_| "fixture: TLS version")?;
    let mut serial = BigNum::new().map_err(|_| "fixture: TLS serial")?;
    serial
        .rand(128, MsbOption::MAYBE_ZERO, false)
        .map_err(|_| "fixture: TLS serial RNG")?;
    let serial = serial
        .to_asn1_integer()
        .map_err(|_| "fixture: TLS serial codec")?;
    builder
        .set_serial_number(&serial)
        .map_err(|_| "fixture: TLS serial binding")?;
    builder
        .set_subject_name(&name)
        .map_err(|_| "fixture: TLS subject binding")?;
    builder
        .set_issuer_name(&name)
        .map_err(|_| "fixture: TLS issuer binding")?;
    builder
        .set_pubkey(&key)
        .map_err(|_| "fixture: TLS public key")?;
    let before = Asn1Time::days_from_now(0).map_err(|_| "fixture: TLS time")?;
    let after = Asn1Time::days_from_now(1).map_err(|_| "fixture: TLS time")?;
    builder
        .set_not_before(&before)
        .map_err(|_| "fixture: TLS time binding")?;
    builder
        .set_not_after(&after)
        .map_err(|_| "fixture: TLS time binding")?;
    let san = SubjectAlternativeName::new()
        .dns("localhost")
        .ip("127.0.0.1")
        .ip("::1")
        .build(&builder.x509v3_context(None, None))
        .map_err(|_| "fixture: TLS SAN")?;
    builder
        .append_extension(san)
        .map_err(|_| "fixture: TLS SAN binding")?;
    builder
        .sign(&key, MessageDigest::sha256())
        .map_err(|_| "fixture: TLS signing")?;
    let certificate = String::from_utf8(builder.build().to_pem().map_err(|_| "fixture: TLS PEM")?)
        .map_err(|_| "fixture: TLS certificate UTF8")?;
    let private = String::from_utf8(
        key.private_key_to_pem_pkcs8()
            .map_err(|_| "fixture: TLS PEM")?,
    )
    .map_err(|_| "fixture: TLS private UTF8")?;
    Ok((certificate, private))
}

async fn product_legacy_page(
    client: &reqwest::Client,
    native_origin: &str,
    token: &str,
    offset: i64,
) -> ProbeResult<Value> {
    let response = client
        .get(format!(
            "{native_origin}/api/v1/hr/attendance-records/me?limit=25&offset={offset}"
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "native: own history request")?;
    if response.status() != StatusCode::OK {
        return Err("native: own history status");
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "native: own history bytes")?;
    if bytes.len() > MAX_FRAME {
        return Err("native: fixture history IPC size");
    }
    serde_json::from_slice(&bytes).map_err(|_| "native: own history JSON")
}

async fn product_facts(
    context: &ProductProbeContext<'_>,
    browser: &mut Browser,
    enrollment: ProductEnrollment<'_>,
) -> ProbeResult<ProductActor> {
    let ProductProbeContext {
        owner,
        client,
        native_origin,
        ..
    } = *context;
    let ProductEnrollment {
        actor,
        user,
        org,
        source,
        credential,
        token,
    } = enrollment;
    let employee = Uuid::new_v4();
    sqlx::query("INSERT INTO employees(id,org_id,company,name,source_filename,source_sheet,source_row,source_key,raw_row,source_metadata) VALUES($1,$2,'테스트',$3,'browser-test.xlsx','직원',2,$4,'{}','{}')")
        .bind(employee).bind(org).bind(format!("브라우저 {actor} 사용자"))
        .bind(format!("browser-{employee}"))
        .execute(owner).await.map_err(|_| "fixture: employee linkage input")?;
    sqlx::query("UPDATE users SET employee_id=$1 WHERE id=$2 AND org_id=$3")
        .bind(employee)
        .bind(user)
        .bind(org)
        .execute(owner)
        .await
        .map_err(|_| "fixture: Account employee link")?;
    let count: i64 = if actor == "a" { 29 } else { 2 };
    let kinds = [
        "CLOCK_IN",
        "OUT_FOR_WORK",
        "RETURNED",
        "BUSINESS_TRIP",
        "RETURNED",
        "CLOCK_OUT",
    ];
    for index in 0..count {
        native_post(
            client,
            native_origin,
            "/api/v1/hr/attendance-records/me",
            Some(token),
            json!({"kind":kinds[index as usize % kinds.len()],
                "idempotency_key":format!("product-{user}-{index}"),
                "note":format!("{actor}-실제-테스트-작업-{index}")}),
            StatusCode::OK,
        )
        .await?;
    }
    let first = product_legacy_page(client, native_origin, token, 0).await?;
    let second = product_legacy_page(client, native_origin, token, 25).await?;
    if first["total"] != count
        || first["items"].as_array().map(Vec::len) != Some(count.min(25) as usize)
        || second["items"].as_array().map(Vec::len) != Some((count - 25).max(0) as usize)
    {
        return Err("native: full history population accounting");
    }
    let counter: i64 = sqlx::query_scalar("SELECT (passkey_json->'cred'->>'counter')::bigint FROM auth_webauthn_credentials WHERE id=$1 AND user_id=$2 AND org_id=$3 AND credential_id=$4")
        .bind(source).bind(user).bind(org).bind(credential).fetch_one(owner).await
        .map_err(|_| "fixture: genuine source counter")?;
    browser
        .write(json!({"kind":"fixture","actor":actor,
        "company_name":format!("브라우저 {actor} 법인"),
        "account_name":format!("브라우저 {actor} 사용자"),
        "first_page":first,"second_page":second}))
        .await?;
    Ok(ProductActor {
        actor: actor.into(),
        user,
        org,
        source,
        credential: credential.into(),
        counter,
        employee,
        fixture_bearer: token.into(),
    })
}

async fn product_checkpoint(
    context: &ProductProbeContext<'_>,
    actors: &[ProductActor],
    request: &Value,
    remembered: &mut std::collections::BTreeMap<Uuid, (Uuid, String, i64)>,
) -> ProbeResult<()> {
    let ProductProbeContext {
        owner,
        verifier,
        storage_key,
        client,
        native_origin,
        ..
    } = *context;
    use openssl::symm::{Cipher, decrypt_aead};
    let expected = [
        "actor",
        "context",
        "cookie_expires",
        "kind",
        "phase",
        "seq",
        "session_token",
        "signed_counter",
        "v",
    ];
    let keys = request
        .as_object()
        .ok_or("infrastructure: checkpoint object")?
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    if keys != expected.into_iter().collect() {
        return Err("infrastructure: checkpoint fields");
    }
    let actor = actors
        .iter()
        .find(|actor| request["actor"] == actor.actor)
        .ok_or("infrastructure: checkpoint actor")?;
    let context = request["context"]
        .as_str()
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or("infrastructure: checkpoint context")?;
    let handle = request["session_token"]
        .as_str()
        .ok_or("infrastructure: checkpoint handle")?;
    if !handle.starts_with("bs1.") || handle.len() != 47 {
        return Err("native: handle wire shape");
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(&handle[4..])
        .map_err(|_| "native: handle codec")?;
    if bytes.len() != 32 || URL_SAFE_NO_PAD.encode(&bytes) != handle[4..] {
        return Err("native: handle canonical codec");
    }
    let binding: (Uuid,Uuid,Uuid,Uuid,OffsetDateTime,bool,bool) = sqlx::query_as(
        "SELECT account_id,company_id,family_id,source_credential_id,expires_at,closed_at IS NOT NULL,token_hash=$2 FROM auth_security.browser_sessions WHERE context_id=$1")
        .bind(context).bind(Sha256::digest(handle.as_bytes()).to_vec()).fetch_one(owner).await
        .map_err(|_| "native: browser custody missing")?;
    if binding.0 != actor.user
        || binding.1 != actor.org
        || binding.3 != actor.source
        || !binding.6
        || request["cookie_expires"].as_i64() != Some(binding.4.unix_timestamp())
    {
        return Err("native: browser custody identity/hash mismatch");
    }
    if request["phase"] == "open" {
        if binding.5 || binding.4 <= OffsetDateTime::now_utc() {
            return Err("native: browser unexpectedly terminal");
        }
        let sealed: (i32,Vec<u8>,Vec<u8>,Vec<u8>) = sqlx::query_as(
            "SELECT codec_version,ciphertext,nonce,tag FROM auth_security.browser_sessions WHERE context_id=$1")
            .bind(context).fetch_one(owner).await.map_err(|_| "native: sealed proof missing")?;
        if sealed.0 != 1
            || sealed.1.is_empty()
            || sealed.1.len() > 1048576
            || sealed.2.len() != 12
            || sealed.3.len() != 16
        {
            return Err("native: sealed proof shape");
        }
        let mut aad = Vec::with_capacity(125);
        aad.extend_from_slice(&29_u32.to_be_bytes());
        aad.extend_from_slice(b"console/browser-session-proof");
        aad.extend_from_slice(&1_u32.to_be_bytes());
        for id in [binding.0, binding.1, binding.2, binding.3, context] {
            aad.extend_from_slice(id.as_bytes());
        }
        aad.extend_from_slice(&binding.4.unix_timestamp().to_be_bytes());
        if aad.len() != 125 {
            return Err("native: AAD shape");
        }
        let plaintext = decrypt_aead(
            Cipher::aes_256_gcm(),
            storage_key,
            Some(&sealed.2),
            &aad,
            &sealed.1,
            &sealed.3,
        )
        .map_err(|_| "native: retained original proof AEAD")?;
        let proof = String::from_utf8(plaintext).map_err(|_| "native: retained proof UTF8")?;
        let claims = verifier
            .verify_access_token(&proof)
            .map_err(|_| "native: retained proof signature")?;
        if claims.sub != actor.user.to_string()
            || claims.org != actor.org.to_string()
            || claims.session_family_id != Some(binding.2)
            || claims.exp != binding.4.unix_timestamp()
            || claims.platform
            || claims.view_as
            || claims.read_only
            || claims.actor_session.is_some()
            || claims.tenant_context.is_some()
        {
            return Err("native: retained proof exact authority mismatch");
        }
        let effects: (i64,i64,i64,i64,i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM auth_webauthn_ceremonies WHERE id=$1 AND ceremony_kind='authentication' AND user_id IS NULL AND consumed_at IS NOT NULL),
                    (SELECT count(*) FROM auth_refresh_token_families WHERE id=$2 AND user_id=$3 AND org_id=$4 AND revoked_at IS NULL),
                    (SELECT count(*) FROM auth_refresh_tokens WHERE family_id=$2),
                    (SELECT count(*) FROM audit_events WHERE actor=$3 AND org_id=$4 AND action='auth.login' AND after_snap->>'refresh_family_id'=$2::text AND after_snap->>'passkey_id'=$5::text),
                    (SELECT count(*) FROM auth_webauthn_credentials WHERE id=$5 AND user_id=$3 AND org_id=$4 AND credential_id=$6)")
            .bind(context).bind(binding.2).bind(actor.user).bind(actor.org).bind(actor.source).bind(&actor.credential)
            .fetch_one(owner).await.map_err(|_| "native: product login effects")?;
        if effects != (1, 1, 1, 1, 1) {
            return Err("native: product login effects mismatch");
        }
        let counter: i64 = sqlx::query_scalar("SELECT (passkey_json->'cred'->>'counter')::bigint FROM auth_webauthn_credentials WHERE id=$1")
            .bind(actor.source).fetch_one(owner).await.map_err(|_| "native: product credential counter")?;
        if let Some(previous) = remembered.get(&context) {
            if previous.0 != binding.2
                || previous.1 != proof
                || counter < previous.2
                || request["signed_counter"].as_i64() != Some(previous.2)
            {
                return Err("native: SSR silently changed session/key custody");
            }
        } else {
            if counter <= actor.counter || request["signed_counter"].as_i64() != Some(counter) {
                return Err("native: genuine product signature counter missing");
            }
            remembered.insert(context, (binding.2, proof, counter));
        }
    } else if request["phase"] == "closed" {
        let previous = remembered
            .get(&context)
            .ok_or("native: unobserved logout context")?;
        if !binding.5 || previous.0 != binding.2 {
            return Err("native: logout not closed");
        }
        let effects: (bool,bool,bool,i64) = sqlx::query_as(
            "SELECT (SELECT revoked_at IS NOT NULL FROM auth_refresh_token_families WHERE id=$1),
                    (SELECT bool_and(revoked_at IS NOT NULL) FROM auth_refresh_tokens WHERE family_id=$1),
                    (SELECT ciphertext IS NULL AND nonce IS NULL AND tag IS NULL AND owner_removed_at IS NULL FROM auth_security.browser_sessions WHERE context_id=$2),
                    (SELECT count(*) FROM audit_events WHERE actor=$3 AND org_id=$4 AND action='auth.logout' AND target_id=$1::text)")
            .bind(binding.2).bind(context).bind(actor.user).bind(actor.org).fetch_one(owner).await
            .map_err(|_| "native: logout effects")?;
        if effects != (true, true, true, 1) {
            return Err("native: logout effects mismatch");
        }
        let denied = client
            .get(format!("{native_origin}/api/v1/hr/attendance-records/me"))
            .bearer_auth(&previous.1)
            .send()
            .await
            .map_err(|_| "native: logout original proof read")?;
        if denied.status() != StatusCode::UNAUTHORIZED {
            return Err("native: original family proof survived logout");
        }
    } else {
        return Err("infrastructure: checkpoint phase");
    }
    Ok(())
}

async fn product_protocol(
    context: &ProductProbeContext<'_>,
    browser: &mut Browser,
    actors: &mut [ProductActor],
) -> ProbeResult<Value> {
    let mut remembered = std::collections::BTreeMap::new();
    loop {
        let frame = browser.read_product().await?;
        if frame["kind"] == "control" {
            let result = product_control(context, actors, &frame).await?;
            browser
                .write(
                    json!({"kind":"controlled","id":frame["id"],"op":frame["op"],"result":result}),
                )
                .await?;
        } else if frame["kind"] == "checkpoint" {
            product_checkpoint(context, actors, &frame, &mut remembered).await?;
            browser.write(json!({"kind":"checked","actor":frame["actor"],"context":frame["context"],"phase":frame["phase"]})).await?;
        } else if frame["kind"] == "report" {
            let items = frame["scenarios"]
                .as_array()
                .ok_or("infrastructure: scenario report")?;
            if frame["discovered"] != 17 || items.len() != 17 {
                return Err("infrastructure: scenario discovery count");
            }
            let expected_ids = (1..=17).map(|n| format!("P{n:02}")).collect::<Vec<_>>();
            for (item, id) in items.iter().zip(expected_ids) {
                if item["id"] != id
                    || !["passed", "failed", "unreached", "skipped"]
                        .contains(&item["status"].as_str().unwrap_or(""))
                {
                    return Err("infrastructure: scenario report identity");
                }
            }
            for status in ["passed", "failed", "unreached", "skipped"] {
                let count = items.iter().filter(|item| item["status"] == status).count();
                if frame["counts"][status] != count {
                    return Err("infrastructure: scenario report accounting");
                }
            }
            let executed = items
                .iter()
                .filter(|item| item["status"] == "passed" || item["status"] == "failed")
                .count();
            if frame["executed"] != executed {
                return Err("infrastructure: scenario execution count");
            }
            browser.write(json!({"kind":"done"})).await?;
            return Ok(frame);
        } else {
            return Err("infrastructure: unsupported product frame");
        }
    }
}

async fn run_product_probe(pool: &PgPool) -> ProbeResult<Vec<Value>> {
    let runtime = runtime_login(pool).await;
    let root = PathBuf::from(
        std::env::var_os("FRONTEND_ROOT").ok_or("infrastructure: FRONTEND_ROOT missing")?,
    );
    let mut browser = Browser::start_product(root)?;
    let mut server = None;
    let result = timeout(
        StdDuration::from_secs(1800),
        product_registration_probe(pool, &runtime, &mut browser, &mut server, false, true),
    )
    .await
    .unwrap_or(Err("infrastructure: product deadline"));
    if let Some(server) = server {
        server.abort();
        let _ = server.await;
    }
    let all_passed = result
        .as_ref()
        .ok()
        .and_then(|items| items.last())
        .is_some_and(|report| {
            report["counts"]["passed"] == 17
                && report["counts"]["failed"] == 0
                && report["counts"]["unreached"] == 0
                && report["counts"]["skipped"] == 0
        });
    if let Err(reason) = &result {
        eprintln!("PRODUCT_BROWSER_NATIVE_FAILURE {reason}");
    }
    browser.reap_product(result.is_ok(), all_passed).await?;
    result
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn genuine_production_browser_proposal_requires_all_seventeen_scenarios(pool: PgPool) {
    let observations = run_product_probe(&pool)
        .await
        .expect("real product browser probe or classified infrastructure failure");
    let report = observations.last().expect("scenario report");
    println!(
        "PRODUCT_BROWSER_SCENARIOS {}",
        serde_json::to_string(report).unwrap()
    );
    assert_eq!(report["discovered"], 17);
    assert_eq!(report["counts"]["failed"], 0);
    assert_eq!(report["counts"]["skipped"], 0);
    assert_eq!(
        report["counts"]["unreached"], 0,
        "required unauthored scenarios cannot be reported complete"
    );
}
