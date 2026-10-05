//! Explicit server-to-server browser transport over the native passkey owner.
use super::*;
use axum::body::to_bytes;
use axum::extract::{DefaultBodyLimit, Request};
use axum::middleware::{self, Next};
use console_platform_auth::browser_session::{self, Binding, Handle, StoredProof};
use console_platform_request_context::{
    VerifiedForwardedClientIp, audit_context_for_method, scope_audit_context,
};
use sqlx::{FromRow, Postgres, Transaction};

pub const BROWSER_START_PATH: &str = "/api/v1/auth/browser-session/start";
pub const BROWSER_LOGIN_PATH: &str = "/api/v1/auth/browser-session/login";
pub const BROWSER_LOGOUT_PATH: &str = "/api/v1/auth/browser-session/logout";

pub(super) fn router(state: AuthRestState) -> Router<AuthRestState> {
    with_browser_ingress(
        Router::new()
            .route(BROWSER_START_PATH, post(start))
            .route(BROWSER_LOGIN_PATH, post(login))
            .route(
                BROWSER_LOGOUT_PATH,
                post(logout).layer(DefaultBodyLimit::max(4096)),
            ),
        state,
    )
}

/// Require both authenticated service custody and the ingress owner's positive
/// forwarding witness. A direct-peer fallback is insufficient for this route.
pub fn with_browser_ingress<S>(router: Router<S>, state: AuthRestState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.route_layer(middleware::from_fn(move |request: Request, next: Next| {
        let state = state.clone();
        async move {
            let services = match configured_services(&state) {
                Ok(services) => services,
                Err(error) => return error.into_response(),
            };
            let mut values = request
                .headers()
                .get_all("x-console-browser-ingress")
                .iter();
            let valid = values
                .next()
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| {
                    services
                        .browser_ingress_key
                        .as_ref()
                        .is_some_and(|key| key.accepts_wire(value))
                })
                && values.next().is_none()
                && request
                    .extensions()
                    .get::<VerifiedForwardedClientIp>()
                    .is_some();
            if !valid {
                return denied().into_response();
            }
            let audit = audit_context_for_method(&request, "browser_session");
            scope_audit_context(audit, next.run(request)).await
        }
    }))
}

fn configured_services(state: &AuthRestState) -> Result<&AuthServices, RestError> {
    let services = state.services()?;
    if services.browser_session_key.is_none() || services.browser_ingress_key.is_none() {
        return Err(RestError::unavailable(
            "browser session transport is not configured",
        ));
    }
    Ok(services)
}

fn denied() -> RestError {
    RestError::unauthorized("browser session is unavailable or invalid")
}

async fn start(
    State(state): State<AuthRestState>,
    headers: HeaderMap,
    trusted_ip: Option<Extension<TrustedClientIp>>,
    request: Request,
) -> Result<Json<LoginStartResponse>, RestError> {
    let body = to_bytes(request.into_body(), 1)
        .await
        .map_err(|_| RestError::validation("browser session start requires an empty body"))?;
    if !body.is_empty() {
        return Err(RestError::validation(
            "browser session start requires an empty body",
        ));
    }
    start_login_with_mediation(state, headers, trusted_ip, true).await
}

#[derive(Serialize)]
struct LoginResponse {
    session_token: String,
    context_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    expires_at: OffsetDateTime,
}

async fn login(
    State(state): State<AuthRestState>,
    Json(body): Json<LoginFinishRequest>,
) -> Result<Json<LoginResponse>, RestError> {
    let services = configured_services(&state)?;
    console_platform_request_context::current_audit_context()
        .ok_or_else(|| RestError::internal("browser request audit context missing"))?;
    let context = body.ceremony_id;
    let mut tx = state.pool.begin().await.map_err(DbError::Sqlx)?;
    let (outcome, tokens) = prepare_login_in_tx(&mut tx, services, body).await?;
    if tokens.access_token.len() > browser_session::MAX_PROOF_BYTES {
        return Err(RestError {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "payload_too_large",
            message: "native proof exceeds browser transport admission".into(),
        });
    }
    let claims = services
        .jwt_verifier
        .verify_access_token(&tokens.access_token)
        .map_err(|_| denied())?;
    let binding = Binding {
        codec_version: browser_session::VERSION,
        account: outcome.user_id,
        company: *outcome.org_id.as_uuid(),
        family: tokens.family_id,
        source: outcome.passkey_id,
        context,
        expires_unix_seconds: claims.exp,
    };
    validate_bindings(&claims, &binding)?;
    let expires_at = OffsetDateTime::from_unix_timestamp(claims.exp).map_err(|_| denied())?;
    let family_created: OffsetDateTime = sqlx::query_scalar(
        "SELECT created_at FROM auth_refresh_token_families WHERE id=$1 AND user_id=$2 AND org_id=$3 AND provenance_version=0 AND revoked_at IS NULL",
    ).bind(binding.family).bind(binding.account).bind(binding.company)
        .fetch_one(tx.as_mut()).await.map_err(DbError::Sqlx)?;
    if expires_at > tokens.refresh_expires_at
        || expires_at > family_created + services.refresh_family_absolute_ttl
    {
        return Err(denied());
    }
    let handle = Handle::generate()
        .map_err(|_| RestError::unavailable("browser cryptography unavailable"))?;
    let proof = browser_session::seal(
        services.browser_session_key.as_ref().ok_or_else(denied)?,
        &binding,
        handle.hash(),
        &tokens.access_token,
    )
    .map_err(|_| RestError::unavailable("browser proof cannot be admitted"))?;
    sqlx::query("SELECT public.platform_browser_session_insert($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(binding.company)
        .bind(binding.account)
        .bind(binding.family)
        .bind(binding.source)
        .bind(binding.context)
        .bind(&proof.token_hash)
        .bind(expires_at)
        .bind(&proof.ciphertext)
        .bind(&proof.nonce)
        .bind(&proof.tag)
        .execute(tx.as_mut())
        .await
        .map_err(DbError::Sqlx)?;
    tx.commit().await.map_err(DbError::Sqlx)?;
    Ok(Json(LoginResponse {
        session_token: handle.as_str().to_owned(),
        context_id: context,
        expires_at,
    }))
}

fn validate_bindings(
    claims: &console_platform_auth::AccessClaims,
    binding: &Binding,
) -> Result<(), RestError> {
    if claims.sub != binding.account.to_string()
        || claims.org != binding.company.to_string()
        || claims.session_family_id != Some(binding.family)
        || claims.exp != binding.expires_unix_seconds
        || claims.platform
        || claims.view_as
        || claims.read_only
        || claims.tenant_context.is_some()
        || claims.group_context_id.is_some()
        || claims.actor_home_org.is_some()
        || claims.actor_session.is_some()
    {
        return Err(denied());
    }
    Ok(())
}

#[derive(FromRow)]
struct Mapping {
    account_id: Uuid,
    company_id: Uuid,
    family_id: Uuid,
    source_credential_id: Uuid,
    context_id: Uuid,
    expires_at: OffsetDateTime,
    codec_version: i32,
    ciphertext: Option<Vec<u8>>,
    nonce: Option<Vec<u8>>,
    tag: Option<Vec<u8>>,
    closed_at: Option<OffsetDateTime>,
    owner_removed_at: Option<OffsetDateTime>,
}

impl Mapping {
    fn binding(&self) -> Binding {
        Binding {
            codec_version: self.codec_version,
            account: self.account_id,
            company: self.company_id,
            family: self.family_id,
            source: self.source_credential_id,
            context: self.context_id,
            expires_unix_seconds: self.expires_at.unix_timestamp(),
        }
    }
}

fn parse_identity(token: &str, context: &str) -> Result<(Handle, Uuid), RestError> {
    let handle = Handle::parse(token).map_err(|_| denied())?;
    let id = Uuid::parse_str(context).map_err(|_| denied())?;
    if id.to_string() != context {
        return Err(denied());
    }
    Ok((handle, id))
}

async fn locate_and_arm(
    tx: &mut Transaction<'_, Postgres>,
    hash: &[u8],
    context: Uuid,
) -> Result<Uuid, RestError> {
    let company: Option<Uuid> =
        sqlx::query_scalar("SELECT public.platform_browser_session_company($1,$2)")
            .bind(hash)
            .bind(context)
            .fetch_one(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
    let company = company.ok_or_else(denied)?;
    sqlx::query("SELECT set_config('app.current_org',$1,true)")
        .bind(company.to_string())
        .execute(tx.as_mut())
        .await
        .map_err(DbError::Sqlx)?;
    Ok(company)
}

async fn read_mapping(
    tx: &mut Transaction<'_, Postgres>,
    company: Uuid,
    hash: &[u8],
    context: Uuid,
) -> Result<Mapping, RestError> {
    sqlx::query_as("SELECT * FROM public.platform_browser_session_read($1,$2,$3)")
        .bind(company)
        .bind(hash)
        .bind(context)
        .fetch_optional(tx.as_mut())
        .await
        .map_err(DbError::Sqlx)?
        .ok_or_else(denied)
}

/// Current authority only; never exposes the original proof outside native auth.
pub struct BrowserPrincipal {
    pub principal: Principal,
    pub browser_context: Uuid,
    pub expires_at: OffsetDateTime,
}

impl AuthRestState {
    pub async fn resolve_browser_session(
        &self,
        token: &str,
        context: &str,
    ) -> Result<BrowserPrincipal, impl IntoResponse> {
        self.resolve_browser_session_inner(token, context).await
    }

    async fn resolve_browser_session_inner(
        &self,
        token: &str,
        context: &str,
    ) -> Result<BrowserPrincipal, RestError> {
        let services = configured_services(self)?;
        let (handle, context) = parse_identity(token, context)?;
        let hash = handle.hash();
        let mut tx = self.pool.begin().await.map_err(DbError::Sqlx)?;
        let company = locate_and_arm(&mut tx, &hash, context).await?;
        let mut row = read_mapping(&mut tx, company, &hash, context).await?;
        if row.closed_at.is_some()
            || row.owner_removed_at.is_some()
            || row.expires_at <= OffsetDateTime::now_utc()
        {
            return Err(denied());
        }
        let binding = row.binding();
        let proof = StoredProof {
            codec_version: row.codec_version,
            token_hash: hash.to_vec(),
            ciphertext: row.ciphertext.take().ok_or_else(denied)?,
            nonce: row.nonce.take().ok_or_else(denied)?,
            tag: row.tag.take().ok_or_else(denied)?,
        };
        let opened = browser_session::open(
            services.browser_session_key.as_ref().ok_or_else(denied)?,
            &binding,
            &proof,
        )
        .map_err(|_| denied())?;
        let claims = services
            .jwt_verifier
            .verify_access_token(opened.as_str())
            .map_err(|_| denied())?;
        validate_bindings(&claims, &binding)?;
        let valid_source: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM auth_webauthn_credentials c WHERE c.id=$1 AND c.user_id=$2 AND c.org_id=$3) AND EXISTS(SELECT 1 FROM auth_refresh_token_families f WHERE f.id=$4 AND f.user_id=$2 AND f.org_id=$3 AND f.provenance_version=0 AND f.revoked_at IS NULL) AND NOT EXISTS(SELECT 1 FROM auth_legacy_otp_family_sources b WHERE b.family_id=$4)",
        ).bind(row.source_credential_id).bind(row.account_id).bind(company).bind(row.family_id)
            .fetch_one(tx.as_mut()).await.map_err(DbError::Sqlx)?;
        if !valid_source {
            return Err(denied());
        }
        tx.commit().await.map_err(DbError::Sqlx)?;
        let principal = console_platform_request_context::resolve_principal_from_bearer_token(
            &services.jwt_verifier,
            &self.pool,
            opened.as_str(),
        )
        .await
        .map_err(rest_error_from_request_context)?;
        if row.expires_at <= OffsetDateTime::now_utc() {
            return Err(denied());
        }
        Ok(BrowserPrincipal {
            principal,
            browser_context: context,
            expires_at: row.expires_at,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LogoutRequest {
    session_token: String,
    browser_context: String,
}

async fn logout(
    State(state): State<AuthRestState>,
    Json(body): Json<LogoutRequest>,
) -> Result<StatusCode, RestError> {
    let services = configured_services(&state)?;
    let (handle, context) = parse_identity(&body.session_token, &body.browser_context)?;
    let hash = handle.hash();
    let mut tx = state.pool.begin().await.map_err(DbError::Sqlx)?;
    let company = locate_and_arm(&mut tx, &hash, context).await?;
    let row = read_mapping(&mut tx, company, &hash, context).await?;
    if row.closed_at.is_some() && row.owner_removed_at.is_some() {
        tx.commit().await.map_err(DbError::Sqlx)?;
        return Ok(StatusCode::NO_CONTENT);
    }
    let org = OrgId::from_uuid(company);
    let account = console_platform_auth::lock_account_tx(&mut tx, org, row.account_id)
        .await
        .map_err(DbError::Sqlx)?;
    if account.is_none() {
        // Account removal may have won the lock wait. Only its committed marker
        // can reconcile closure after native parents disappear.
        let removed = read_mapping(&mut tx, company, &hash, context).await?;
        if removed.owner_removed_at.is_none() || removed.closed_at.is_none() {
            return Err(denied());
        }
        tx.commit().await.map_err(DbError::Sqlx)?;
        return Ok(StatusCode::NO_CONTENT);
    }
    let family: Option<Option<OffsetDateTime>> = sqlx::query_scalar(
        "SELECT revoked_at FROM auth_refresh_token_families WHERE id=$1 AND user_id=$2 AND org_id=$3 FOR UPDATE",
    ).bind(row.family_id).bind(row.account_id).bind(company)
        .fetch_optional(tx.as_mut()).await.map_err(DbError::Sqlx)?;
    if family.is_none() {
        return Err(denied());
    }
    let _: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM auth_refresh_tokens WHERE family_id=$1 AND user_id=$2 AND org_id=$3 ORDER BY id FOR UPDATE",
    ).bind(row.family_id).bind(row.account_id).bind(company)
        .fetch_all(tx.as_mut()).await.map_err(DbError::Sqlx)?;
    let locked: Mapping =
        sqlx::query_as("SELECT * FROM public.platform_browser_session_lock($1,$2,$3)")
            .bind(company)
            .bind(&hash[..])
            .bind(context)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?
            .ok_or_else(denied)?;
    if locked.closed_at.is_some() {
        tx.commit().await.map_err(DbError::Sqlx)?;
        return Ok(StatusCode::NO_CONTENT);
    }
    let now = console_platform_auth::authentication_time_tx(&mut tx, OffsetDateTime::now_utc())
        .await
        .map_err(DbError::Sqlx)?;
    let audit = console_platform_request_context::current_audit_context()
        .ok_or_else(|| RestError::internal("browser request audit context missing"))?;
    services
        .refresh_tokens
        .revoke_locked_family_for_logout_with_audit_context_in_tx(
            &mut tx,
            org,
            row.account_id,
            row.family_id,
            now,
            Some((audit.trace, audit.request)),
        )
        .await
        .map_err(|_| RestError::internal("native logout transaction failed"))?;
    let closed: bool = sqlx::query_scalar("SELECT public.platform_browser_session_close($1,$2,$3)")
        .bind(company)
        .bind(&hash[..])
        .bind(context)
        .fetch_one(tx.as_mut())
        .await
        .map_err(DbError::Sqlx)?;
    if !closed {
        return Err(denied());
    }
    tx.commit().await.map_err(DbError::Sqlx)?;
    Ok(StatusCode::NO_CONTENT)
}
