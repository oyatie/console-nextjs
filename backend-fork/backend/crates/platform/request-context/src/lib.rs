//! Per-request tenant context for the multi-tenant FSM.
//!
//! The application connects to Postgres as the non-owner, RLS-enforced `console_rt`
//! role. Every tenant-scoped query must arm the `app.current_org` GUC with the
//! org of the *authenticated request*, or RLS returns zero rows (fail-closed).
//!
//! This crate is the single source of that org for the lifetime of one request:
//!
//! * [`CURRENT_ORG`] is a tokio [`task_local!`] holding the request's [`OrgId`].
//!   The shared middleware enters it with [`tokio::task::LocalKey::scope`] around
//!   the downstream handler, so any code running on that task can read it.
//! * [`current_org`] reads it and FAILS CLOSED when unset — it never defaults to
//!   a tenant. Adapter read paths call `with_org_conn(pool, current_org()?, ..)`.
//! * [`resolve_principal`] is the one merged copy of the per-crate
//!   `principal_from_headers` extractors: bearer → verify → claims → org from the
//!   verified `org` claim → branch scope re-resolved from the DB (the safer
//!   policy: a membership revocation takes effect immediately).
//!
//! Note on `tokio::spawn`: a freshly spawned task does NOT inherit the
//! task-local. A handler that spawns work which itself touches tenant-scoped
//! tables must re-enter the scope, e.g. `CURRENT_ORG.scope(org, async { .. })`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use console_kernel_core::{
    AccessScope, AuditRequestContext, BranchScope, ErrorKind, KernelError, OrgId, TraceContext,
    UserId,
};
use console_platform_auth::{AccessClaims, ActorSession, JwtVerifier, TenantAccessContext};
use console_platform_authz::{
    PlatformPrincipal, Principal, Role, SubjectFreshness, effective_branch_scope_for_tenant,
    resolve_branch_scope_in_org, resolve_effective_feature_grants_in_org,
};
use console_platform_db::{DbError, with_org_conn};
use console_platform_group::group_admin_member_orgs;
use http::{HeaderMap, StatusCode};
use ipnet::IpNet;
use sqlx::PgPool;
use std::collections::BTreeSet;

tokio::task_local! {
    /// The tenant of the in-flight request. Set once per request by the shared
    /// middleware; read by [`current_org`].
    pub static CURRENT_ORG: OrgId;

    /// Trace and transport metadata captured once at the authenticated HTTP
    /// boundary, then reused by every audit event emitted by that request.
    static CURRENT_AUDIT_CONTEXT: RequestAuditContext;
}

/// Request-correlated metadata suitable for an [`console_kernel_core::AuditEvent`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestAuditContext {
    pub trace: TraceContext,
    pub request: AuditRequestContext,
}

/// Client address resolved by a trusted ingress boundary.
///
/// Request-context middleware never interprets forwarding headers. The ingress
/// that owns proxy trust policy must validate the chain and insert this
/// extension. Without it, audit metadata falls back only to the transport peer
/// supplied by Axum's [`ConnectInfo`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustedClientIp(IpAddr);

impl TrustedClientIp {
    #[must_use]
    pub const fn new(ip: IpAddr) -> Self {
        Self(ip)
    }

    #[must_use]
    pub const fn get(self) -> IpAddr {
        self.0
    }
}

/// Resolve the client IP at the sole trusted HTTP ingress boundary.
///
/// A forwarding header is considered only when the deployment explicitly
/// configures one or more trusted proxy hops *and* Axum supplied the direct
/// transport peer. `trusted_proxy_count` includes that direct peer; every
/// remaining expected proxy hop is validated right-to-left against the trusted
/// CIDRs before the client value immediately to its left is accepted. There must
/// be exactly one `X-Forwarded-For` field and every comma-delimited token must
/// be a non-empty IP address. Duplicate fields, malformed or incomplete chains
/// fall back to the direct peer; they never promote an attacker-controlled
/// header value. This is the only place that interprets `X-Forwarded-For` in the
/// HTTP process.
#[must_use]
pub fn resolve_trusted_client_ip(
    headers: &HeaderMap,
    peer: SocketAddr,
    trusted_proxy_count: usize,
    trusted_proxy_cidrs: &[IpNet],
) -> IpAddr {
    if trusted_proxy_count == 0
        || !trusted_proxy_cidrs
            .iter()
            .any(|network| network.contains(&peer.ip()))
    {
        return peer.ip();
    }

    let forwarded_values = headers.get_all("x-forwarded-for");
    if forwarded_values.iter().count() != 1 {
        return peer.ip();
    }
    let Some(forwarded) = forwarded_values
        .iter()
        .next()
        .and_then(|value| value.to_str().ok())
    else {
        return peer.ip();
    };

    let entries = forwarded
        .split(',')
        .map(str::trim)
        .map(|entry| {
            if entry.is_empty() {
                Err(())
            } else {
                entry.parse::<IpAddr>().map_err(|_| ())
            }
        })
        .collect::<Result<Vec<_>, _>>();
    let Ok(entries) = entries else {
        return peer.ip();
    };

    // `trusted_proxy_count` includes the direct peer. The remaining trusted
    // proxy hops must be the rightmost XFF entries. Validate that complete
    // suffix before accepting the entry immediately to its left as the client.
    // This rejects a caller-prepended chain that merely happens to be long
    // enough, instead of treating an arbitrary untrusted suffix as a proxy.
    let Some(client_index) = entries.len().checked_sub(trusted_proxy_count) else {
        return peer.ip();
    };
    if entries[client_index + 1..].iter().any(|hop| {
        !trusted_proxy_cidrs
            .iter()
            .any(|network| network.contains(hop))
    }) {
        return peer.ip();
    }

    entries[client_index]
}

/// Insert a [`TrustedClientIp`] extension at the process ingress.
///
/// Call this once on the fully-composed app router. Domain routers and rate
/// limiters consume the extension and never parse raw forwarding headers.
pub fn with_trusted_client_ip<S>(
    router: axum::Router<S>,
    trusted_proxy_count: usize,
    trusted_proxy_cidrs: Vec<IpNet>,
) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let trusted_proxy_cidrs: Arc<[IpNet]> = trusted_proxy_cidrs.into();
    router.layer(axum::middleware::from_fn(
        move |mut request: Request, next: Next| {
            let trusted_proxy_cidrs = Arc::clone(&trusted_proxy_cidrs);
            async move {
                if let Some(ConnectInfo(peer)) =
                    request.extensions().get::<ConnectInfo<SocketAddr>>()
                {
                    let client_ip = resolve_trusted_client_ip(
                        request.headers(),
                        *peer,
                        trusted_proxy_count,
                        &trusted_proxy_cidrs,
                    );
                    request
                        .extensions_mut()
                        .insert(TrustedClientIp::new(client_ip));
                }
                next.run(request).await
            }
        },
    ))
}

/// Why a request could not be given a tenant context.
#[derive(Debug, thiserror::Error)]
pub enum RequestContextError {
    /// No tenant is bound to the current task — the request never passed through
    /// the org middleware, or read code ran outside the request task (e.g. a bare
    /// `tokio::spawn` that did not re-enter [`CURRENT_ORG`]). Fail closed.
    #[error("no tenant context is bound to the current request")]
    MissingOrg,

    /// The Authorization header was absent or malformed.
    #[error("missing or malformed bearer token")]
    MissingBearer,

    /// The bearer token failed verification.
    #[error("invalid bearer token")]
    InvalidToken,

    /// A claim in an otherwise-valid token did not parse (subject, role, or org).
    #[error("token claim is invalid: {0}")]
    InvalidClaim(&'static str),

    /// JWT verification is not configured for this deployment.
    #[error("jwt verification is not configured")]
    VerifierUnavailable,

    /// Resolving the live branch scope from the database failed.
    #[error("failed to resolve branch scope: {0}")]
    BranchScope(String),

    /// Resolving runtime-effective custom policy grants from the database failed.
    #[error("failed to resolve effective policy: {0}")]
    EffectivePolicy(String),

    /// The verified JWT's hierarchy scope is not valid for this tenant route.
    #[error("access scope is not valid for this route: {0}")]
    AccessScope(KernelError),

    /// A PLATFORM token was presented to a tenant (`/api/*`) route, or a TENANT
    /// token was presented to a `/api/platform/*` route. The two tiers are strictly
    /// separated; crossing them is rejected before any handler runs.
    #[error("token tier is not valid for this route")]
    WrongTokenTier,
}

impl From<RequestContextError> for KernelError {
    /// Adapters surface tenancy failures through their domain error, which
    /// already converts from [`KernelError`]. A missing/invalid request context
    /// at a data-access site is an internal invariant violation (a tenant-scoped
    /// query reached the DB without a bound org), so it maps to an internal
    /// error — the request never produces tenant data on this path.
    fn from(err: RequestContextError) -> Self {
        match err {
            RequestContextError::AccessScope(error) => error,
            err => KernelError::internal(err.to_string()),
        }
    }
}

/// Read the tenant bound to the current request task.
///
/// FAILS CLOSED: returns [`RequestContextError::MissingOrg`] when no org is in
/// scope. It NEVER falls back to a default tenant. Adapter reads wrap their query
/// in `with_org_conn(&self.pool, current_org()?, ..)`.
pub fn current_org() -> Result<OrgId, RequestContextError> {
    CURRENT_ORG
        .try_with(|org| *org)
        .map_err(|_| RequestContextError::MissingOrg)
}

/// Return the audit context bound by [`with_request_context`].
///
/// `None` means the caller is outside an authenticated HTTP request. Mutation
/// handlers must treat that as an invariant failure instead of fabricating an
/// unrelated trace at the persistence boundary.
#[must_use]
pub fn current_audit_context() -> Option<RequestAuditContext> {
    CURRENT_AUDIT_CONTEXT.try_with(Clone::clone).ok()
}

/// Extract the raw bearer token from an Authorization header.
fn bearer_token(headers: &HeaderMap) -> Result<&str, RequestContextError> {
    headers
        .get(http::header::AUTHORIZATION)
        .ok_or(RequestContextError::MissingBearer)?
        .to_str()
        .map_err(|_| RequestContextError::MissingBearer)?
        .strip_prefix("Bearer ")
        .filter(|token| !token.trim().is_empty())
        .ok_or(RequestContextError::MissingBearer)
}

/// Resolve the authenticated [`Principal`] for a request from its headers.
///
/// This is the single merged copy of the formerly-duplicated
/// `principal_from_headers` extractors:
/// 1. parse the bearer token,
/// 2. verify it (the verifier already rejects a token whose `org` claim is not a
///    valid UUID),
/// 3. parse subject and roles,
/// 4. take the tenant from the verified `org` claim,
/// 5. re-resolve the live branch scope from the database rather than trusting the
///    token's `branches` claim, so a membership revocation takes effect at once.
pub async fn resolve_principal(
    verifier: &JwtVerifier,
    pool: &PgPool,
    headers: &HeaderMap,
) -> Result<Principal, RequestContextError> {
    let token = bearer_token(headers)?;
    resolve_principal_from_bearer_token(verifier, pool, token).await
}

/// Validate a verified token against its current account and Company authority.
/// This is authentication at the request boundary; domain owners must still
/// check current policy and revisions inside consequential transactions.
pub async fn validate_current_claims(
    pool: &PgPool,
    claims: &AccessClaims,
) -> Result<(UserId, OrgId), RequestContextError> {
    let user = UserId::from_str(&claims.sub).map_err(|_| RequestContextError::InvalidToken)?;
    let target = OrgId::from_str(&claims.org).map_err(|_| RequestContextError::InvalidToken)?;
    let roles = claims
        .roles
        .iter()
        .map(|role| Role::from_str(role))
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|_| RequestContextError::InvalidToken)?;
    if roles.is_empty() || claims.platform != (target == OrgId::platform()) {
        return Err(RequestContextError::InvalidToken);
    }
    let delegated = claims.actor_session.is_some();
    let session = match claims.actor_session {
        Some(session) if !claims.platform && claims.session_family_id.is_none() => {
            match claims.tenant_context {
                Some(TenantAccessContext::GroupAdmin) => {
                    if session.home_org == OrgId::platform()
                        || claims.actor_home_org.as_deref()
                            != Some(session.home_org.to_string().as_str())
                    {
                        return Err(RequestContextError::InvalidToken);
                    }
                }
                None => {
                    if session.home_org != OrgId::platform()
                        || claims.view_as != claims.read_only
                        || (claims.view_as && roles.len() != 1)
                        || (!claims.view_as && roles != BTreeSet::from([Role::SuperAdmin]))
                        || !claims.group_roles.is_empty()
                    {
                        return Err(RequestContextError::InvalidToken);
                    }
                }
            }
            session
        }
        None if claims.tenant_context.is_none() && !claims.view_as && !claims.read_only => {
            ActorSession {
                family_id: claims
                    .session_family_id
                    .ok_or(RequestContextError::InvalidToken)?,
                expires_at: claims.exp,
                home_org: target,
                subject_version: claims.authz_subject_version,
                session_generation: claims.session_generation,
            }
        }
        _ => return Err(RequestContextError::InvalidToken),
    };
    if claims.platform && !roles.contains(&Role::SuperAdmin) {
        return Err(RequestContextError::InvalidToken);
    }
    let home = session.home_org;
    let row = with_org_conn::<_, _, DbError>(pool, home, |tx| Box::pin(async move {
        sqlx::query_as::<_, (bool, Vec<String>, String, i64, i64)>(
            "SELECT u.is_active, u.roles, o.status, COALESCE(v.version,0), COALESCE(v.session_generation,0) \
             FROM users u JOIN organizations o ON o.id=u.org_id \
             JOIN auth_refresh_token_families f ON f.user_id=u.id AND f.org_id=u.org_id AND f.id=$3 AND f.revoked_at IS NULL \
             LEFT JOIN subject_authz_versions v ON v.org_id=u.org_id AND v.user_id=u.id \
             WHERE u.id=$1 AND u.org_id=$2")
            .bind(*user.as_uuid()).bind(*home.as_uuid()).bind(session.family_id).fetch_optional(tx.as_mut()).await.map_err(DbError::Sqlx)
    })).await.map_err(|_| RequestContextError::EffectivePolicy("current account authority unavailable".into()))?
        .ok_or(RequestContextError::InvalidToken)?;
    let (active, live_roles, status, subject_version, session_generation) = row;
    if !active
        || live_roles.is_empty()
        || (home != OrgId::platform() && status != "ACTIVE")
        || u64::try_from(subject_version).ok() != Some(session.subject_version)
        || u64::try_from(session_generation).ok() != Some(session.session_generation)
        || (!delegated && claims.roles.iter().any(|role| !live_roles.contains(role)))
        || (home == OrgId::platform() && !live_roles.iter().any(|role| role == "SUPER_ADMIN"))
    {
        return Err(RequestContextError::InvalidToken);
    }
    if target != home {
        let active = with_org_conn::<_, _, DbError>(pool, target, |tx| {
            Box::pin(async move {
                sqlx::query_scalar::<_, bool>(
                    "SELECT status='ACTIVE' FROM organizations WHERE id=$1",
                )
                .bind(*target.as_uuid())
                .fetch_optional(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)
            })
        })
        .await
        .map_err(|_| {
            RequestContextError::EffectivePolicy("target Company authority unavailable".into())
        })?;
        if active != Some(true) {
            return Err(RequestContextError::InvalidToken);
        }
    }
    claims
        .validate_expiry()
        .map_err(|_| RequestContextError::InvalidToken)?;
    Ok((user, target))
}

/// Resolve a tenant [`Principal`] from an already-extracted bearer token.
///
/// Realtime WebSocket handshakes may carry the token in `Sec-WebSocket-Protocol`
/// rather than `Authorization`, but the security path after extraction must be
/// identical: verify, reject platform tier, parse roles/org/access scope,
/// re-resolve live branch memberships, and narrow by [`AccessScope`].
pub async fn resolve_principal_from_bearer_token(
    verifier: &JwtVerifier,
    pool: &PgPool,
    token: &str,
) -> Result<Principal, RequestContextError> {
    let claims = verifier
        .verify_access_token(token)
        .map_err(|_| RequestContextError::InvalidToken)?;

    // Tier separation: a PLATFORM token must NEVER resolve to a tenant principal.
    // Reject it here so a platform actor can never reach a tenant `/api/*` route
    // (and so its non-tenant `org` sentinel can never arm a real tenant GUC).
    if claims.platform {
        return Err(RequestContextError::WrongTokenTier);
    }

    let (user_id, org_id) = validate_current_claims(pool, &claims).await?;
    let access_scope = claims
        .access_scope()
        .map_err(|_| RequestContextError::InvalidClaim("access scope is invalid"))?;
    let roles = claims
        .roles
        .iter()
        .map(|role| {
            Role::from_str(role).map_err(|_| RequestContextError::InvalidClaim("unknown role"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;

    // Preserve target-owned Cedar freshness. Delegated home-account freshness
    // was independently validated above; it must not replace this snapshot.
    let authz_freshness = SubjectFreshness {
        policy_version: claims.authz_policy_version,
        subject_version: claims.authz_subject_version,
        session_generation: claims.session_generation,
        step_up_generation: None,
    };

    if claims.tenant_context == Some(TenantAccessContext::GroupAdmin) {
        return resolve_group_admin_tenant_context_principal(
            pool,
            user_id,
            org_id,
            access_scope,
            roles,
            claims.group_context_id.as_deref(),
            authz_freshness,
        )
        .await;
    }

    let role_vec = roles.iter().copied().collect::<Vec<_>>();
    let live_branch_scope = resolve_branch_scope_in_org(pool, org_id, user_id, &role_vec)
        .await
        .map_err(|err| RequestContextError::BranchScope(err.to_string()))?;
    let branch_scope = effective_branch_scope_for_tenant(live_branch_scope, access_scope, org_id)
        .map_err(RequestContextError::AccessScope)?;
    let effective_feature_grants =
        resolve_effective_feature_grants_in_org(pool, org_id, user_id, &branch_scope)
            .await
            .map_err(|err| RequestContextError::EffectivePolicy(err.to_string()))?;

    Ok(Principal::new(user_id, org_id, roles, branch_scope)
        .with_access_scope(access_scope)
        .with_effective_feature_grants(effective_feature_grants)
        .with_authz_freshness(authz_freshness))
}

async fn resolve_group_admin_tenant_context_principal(
    pool: &PgPool,
    user_id: UserId,
    org_id: OrgId,
    access_scope: AccessScope,
    roles: BTreeSet<Role>,
    group_context_id: Option<&str>,
    authz_freshness: SubjectFreshness,
) -> Result<Principal, RequestContextError> {
    let expected_roles = BTreeSet::from([Role::Admin]);
    if roles != expected_roles {
        return Err(RequestContextError::InvalidClaim(
            "group-admin tenant context must carry only ADMIN",
        ));
    }
    let group_id = group_context_id
        .ok_or(RequestContextError::InvalidClaim(
            "group-admin tenant context is missing group id",
        ))?
        .parse::<uuid::Uuid>()
        .map_err(|_| RequestContextError::InvalidClaim("group id is not a valid uuid"))?;

    let members = group_admin_member_orgs(pool, group_id, user_id)
        .await
        .map_err(|err| RequestContextError::BranchScope(err.to_string()))?;
    if !members
        .iter()
        .any(|member| member.org_id == org_id && member.status == "ACTIVE")
    {
        return Err(RequestContextError::AccessScope(KernelError::forbidden(
            "group-admin tenant context is no longer authorized for this organization",
        )));
    }

    // The live group resolver proves the actor still administers this
    // subsidiary. Project through the token's scope so future narrower
    // hierarchy scopes cannot widen here, then build a bounded tenant principal:
    // ADMIN permissions, all-branch only for this subsidiary, never SUPER_ADMIN.
    let branch_scope = effective_branch_scope_for_tenant(BranchScope::All, access_scope, org_id)
        .map_err(RequestContextError::AccessScope)?;
    let effective_feature_grants =
        resolve_effective_feature_grants_in_org(pool, org_id, user_id, &branch_scope)
            .await
            .map_err(|err| RequestContextError::EffectivePolicy(err.to_string()))?;

    Ok(
        Principal::new(user_id, org_id, expected_roles, branch_scope)
            .with_access_scope(access_scope)
            .with_effective_feature_grants(effective_feature_grants)
            .with_authz_freshness(authz_freshness),
    )
}

// ---------------------------------------------------------------------------
// Axum middleware
// ---------------------------------------------------------------------------

/// Apply the per-request tenant-context middleware to one authenticated router.
///
/// Called by each domain `router()` (so the behavior is testable per crate and
/// composes in the app). For every route on `router` it resolves the
/// [`Principal`], stores it in the request extensions (handlers can read
/// `Extension<Principal>`), and runs the downstream handler inside the
/// [`CURRENT_ORG`] scope — arming the request's tenant for every adapter
/// read/write.
///
/// Fail-closed: a request that cannot be resolved to a principal is rejected
/// before any handler runs, so no tenant-scoped query can execute without an org.
///
/// Pass the router's own `jwt_verifier` and a clone of its `pool`. Do NOT apply
/// it to pre-auth routes (login/refresh) or the realtime WS upgrade.
pub fn with_request_context<S>(
    router: axum::Router<S>,
    verifier: Option<JwtVerifier>,
    pool: PgPool,
) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.layer(axum::middleware::from_fn(
        move |mut request: Request, next: Next| {
            let verifier = verifier.clone();
            let pool = pool.clone();
            async move {
                let Some(verifier) = verifier.as_ref() else {
                    return error_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "JWT verification is not configured",
                    );
                };
                let principal = match resolve_principal(verifier, &pool, request.headers()).await {
                    Ok(principal) => principal,
                    Err(err) => return error_response_for(&err),
                };
                let org = principal.org_id;
                let audit_context = request_audit_context(&request);
                request.extensions_mut().insert(principal);
                CURRENT_ORG
                    .scope(
                        org,
                        CURRENT_AUDIT_CONTEXT.scope(audit_context, next.run(request)),
                    )
                    .await
            }
        },
    ))
}

fn request_audit_context(request: &Request) -> RequestAuditContext {
    let headers = request.headers();
    RequestAuditContext {
        trace: trace_context(headers),
        request: AuditRequestContext {
            ip: trusted_or_direct_client_ip(request).map(|ip| ip.to_string()),
            user_agent: header_text(headers, http::header::USER_AGENT.as_str()).map(str::to_owned),
            auth_method: Some("bearer".to_owned()),
            device: header_text(headers, "x-device-id").map(str::to_owned),
        },
    }
}

fn trusted_or_direct_client_ip(request: &Request) -> Option<IpAddr> {
    request
        .extensions()
        .get::<TrustedClientIp>()
        .copied()
        .map(TrustedClientIp::get)
        .or_else(|| {
            request
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ConnectInfo(peer)| peer.ip())
        })
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)?
        .to_str()
        .ok()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn trace_context(headers: &HeaderMap) -> TraceContext {
    header_text(headers, "traceparent")
        .and_then(parse_traceparent)
        .unwrap_or_else(TraceContext::generate)
}

fn parse_traceparent(value: &str) -> Option<TraceContext> {
    let mut fields = value.split('-');
    let version = fields.next()?;
    let trace_id = fields.next()?;
    let span_id = fields.next()?;
    let flags = fields.next()?;
    if fields.next().is_some()
        || version == "ff"
        || !is_lower_hex(version, 2)
        || !is_lower_hex(flags, 2)
        || trace_id.bytes().all(|byte| byte == b'0')
        || span_id.bytes().all(|byte| byte == b'0')
    {
        return None;
    }
    TraceContext::new(trace_id, span_id).ok()
}

fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, message.to_owned()).into_response()
}

fn error_response_for(err: &RequestContextError) -> Response {
    let status = match err {
        RequestContextError::VerifierUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        RequestContextError::BranchScope(_) | RequestContextError::EffectivePolicy(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
        RequestContextError::AccessScope(error) if error.kind == ErrorKind::Forbidden => {
            StatusCode::FORBIDDEN
        }
        RequestContextError::AccessScope(_) => StatusCode::INTERNAL_SERVER_ERROR,
        // A valid token presented to the wrong tier is an authorization failure,
        // not an authentication one: the caller IS authenticated, just not for
        // this route. 403 keeps it distinct from "no/!invalid token" (401).
        RequestContextError::WrongTokenTier => StatusCode::FORBIDDEN,
        _ => StatusCode::UNAUTHORIZED,
    };
    error_response(status, &err.to_string())
}

// ---------------------------------------------------------------------------
// Platform tier extractor + middleware
// ---------------------------------------------------------------------------

/// Resolve the authenticated [`PlatformPrincipal`] for a request from its
/// headers.
///
/// Mirrors [`resolve_principal`] but for the SaaS-vendor PLATFORM tier:
/// 1. parse + verify the bearer token,
/// 2. REQUIRE `platform = true` — a tenant token is rejected here, so a tenant
///    admin can never reach `/api/platform/*`,
/// 3. parse the subject.
///
/// It deliberately resolves NO tenant org and NO branch scope: a platform
/// principal is not tenant-scoped, and platform handlers arm the specific
/// TARGET org themselves per action.
pub async fn resolve_platform_principal(
    verifier: &JwtVerifier,
    pool: &PgPool,
    headers: &HeaderMap,
) -> Result<PlatformPrincipal, RequestContextError> {
    let claims = resolve_platform_claims(verifier, pool, headers).await?;
    let user_id = UserId::from_str(&claims.sub).map_err(|_| RequestContextError::InvalidToken)?;
    Ok(PlatformPrincipal::new(user_id))
}

async fn resolve_platform_claims(
    verifier: &JwtVerifier,
    pool: &PgPool,
    headers: &HeaderMap,
) -> Result<AccessClaims, RequestContextError> {
    let token = bearer_token(headers)?;
    let claims = verifier
        .verify_access_token(token)
        .map_err(|_| RequestContextError::InvalidToken)?;

    // Tier separation: ONLY a platform token may reach a `/api/platform/*` route.
    if !claims.platform {
        return Err(RequestContextError::WrongTokenTier);
    }

    validate_current_claims(pool, &claims).await?;
    Ok(claims)
}

/// Apply the PLATFORM extractor middleware to a `/api/platform/*` router.
///
/// Resolves the [`PlatformPrincipal`] (rejecting any tenant token) and stores it
/// in the request extensions for handlers to read as `Extension<PlatformPrincipal>`.
/// It does NOT enter the [`CURRENT_ORG`] tenant scope: the platform tier is not
/// tenant-scoped, and each platform write arms the TARGET org explicitly.
///
/// Fail-closed: a request that cannot be resolved to a platform principal is
/// rejected before any handler runs.
pub fn with_platform_context<S>(
    router: axum::Router<S>,
    verifier: Option<JwtVerifier>,
    pool: PgPool,
) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.layer(axum::middleware::from_fn(
        move |mut request: Request, next: Next| {
            let verifier = verifier.clone();
            let pool = pool.clone();
            async move {
                let Some(verifier) = verifier.as_ref() else {
                    return error_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "JWT verification is not configured",
                    );
                };
                let claims = match resolve_platform_claims(verifier, &pool, request.headers()).await
                {
                    Ok(principal) => principal,
                    Err(err) => return error_response_for(&err),
                };
                let user_id = match UserId::from_str(&claims.sub) {
                    Ok(id) => id,
                    Err(_) => return error_response_for(&RequestContextError::InvalidToken),
                };
                request
                    .extensions_mut()
                    .insert(PlatformPrincipal::new(user_id));
                request.extensions_mut().insert(claims);
                next.run(request).await
            }
        },
    ))
}

/// Wrap a handler body in the tenant scope, for tests / non-router callers that
/// need to execute adapter code with a known tenant bound.
pub async fn scope_org<F, T>(org: OrgId, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_ORG.scope(org, fut).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::Extension;
    use axum::routing::get;
    use console_platform_authz::{Action, Feature, authorize_org_wide};
    use tower::Service;

    fn forwarded_headers(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", value.parse().unwrap());
        headers
    }

    fn request_with_network_metadata(
        forwarded_for: &str,
        direct_peer: &str,
        trusted_client: Option<&str>,
    ) -> Request {
        let mut request = Request::builder()
            .header("x-forwarded-for", forwarded_for)
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(
            direct_peer.parse::<SocketAddr>().expect("direct peer"),
        ));
        if let Some(trusted_client) = trusted_client {
            request.extensions_mut().insert(TrustedClientIp::new(
                trusted_client.parse().expect("trusted client IP"),
            ));
        }
        request
    }

    #[test]
    fn audit_ip_ignores_client_prepended_forwarded_value() {
        let request = request_with_network_metadata(
            "198.51.100.250, 203.0.113.7",
            "10.0.0.3:443",
            Some("203.0.113.7"),
        );
        assert_eq!(
            trusted_or_direct_client_ip(&request),
            Some("203.0.113.7".parse().unwrap())
        );
    }

    #[test]
    fn audit_ip_accepts_ingress_resolution_across_multiple_trusted_hops() {
        let request = request_with_network_metadata(
            "198.51.100.250, 203.0.113.7, 10.0.0.2",
            "10.0.0.3:443",
            Some("203.0.113.7"),
        );
        assert_eq!(
            trusted_or_direct_client_ip(&request),
            Some("203.0.113.7".parse().unwrap())
        );
    }

    #[test]
    fn audit_ip_uses_direct_peer_when_forwarded_chain_is_insufficient() {
        let request = request_with_network_metadata("203.0.113.7", "192.0.2.10:8443", None);
        assert_eq!(
            trusted_or_direct_client_ip(&request),
            Some("192.0.2.10".parse().unwrap())
        );
    }

    #[test]
    fn audit_ip_uses_direct_peer_when_forwarded_chain_is_invalid() {
        let request =
            request_with_network_metadata("not-an-ip, 203.0.113.7", "192.0.2.11:8443", None);
        assert_eq!(
            trusted_or_direct_client_ip(&request),
            Some("192.0.2.11".parse().unwrap())
        );
    }

    #[test]
    fn ingress_resolver_accepts_a_complete_trusted_proxy_suffix() {
        let headers = forwarded_headers("198.51.100.250, 10.0.0.2");
        let peer = "10.0.0.3:443".parse().unwrap();

        assert_eq!(
            resolve_trusted_client_ip(&headers, peer, 2, &["10.0.0.0/8".parse().unwrap()]),
            "198.51.100.250".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn ingress_resolver_rejects_duplicate_forwarded_fields() {
        let mut headers = HeaderMap::new();
        headers.append(
            "x-forwarded-for",
            "198.51.100.250, 10.0.0.2".parse().unwrap(),
        );
        headers.append(
            "x-forwarded-for",
            "198.51.100.251, 10.0.0.2".parse().unwrap(),
        );
        let peer = "10.0.0.3:443".parse().unwrap();

        assert_eq!(
            resolve_trusted_client_ip(&headers, peer, 2, &["10.0.0.0/8".parse().unwrap()]),
            peer.ip(),
            "ambiguous repeated forwarding fields must fail closed"
        );
    }

    #[test]
    fn ingress_resolver_rejects_empty_forwarded_tokens() {
        let headers = forwarded_headers("198.51.100.250, , 10.0.0.2");
        let peer = "10.0.0.3:443".parse().unwrap();

        assert_eq!(
            resolve_trusted_client_ip(&headers, peer, 2, &["10.0.0.0/8".parse().unwrap()]),
            peer.ip(),
            "empty forwarded tokens must not be silently discarded"
        );
    }

    #[test]
    fn ingress_resolver_rejects_malformed_forwarded_tokens() {
        let headers = forwarded_headers("198.51.100.250, not-an-ip, 10.0.0.2");
        let peer = "10.0.0.3:443".parse().unwrap();

        assert_eq!(
            resolve_trusted_client_ip(&headers, peer, 2, &["10.0.0.0/8".parse().unwrap()]),
            peer.ip(),
            "malformed forwarded tokens must fail closed"
        );
    }

    #[test]
    fn ingress_resolver_rejects_an_untrusted_configured_proxy_suffix() {
        let headers = forwarded_headers("198.51.100.250, 203.0.113.7");
        let peer = "10.0.0.3:443".parse().unwrap();

        assert_eq!(
            resolve_trusted_client_ip(&headers, peer, 2, &["10.0.0.0/8".parse().unwrap()]),
            peer.ip(),
            "an untrusted suffix cannot be treated as an intermediary proxy"
        );
    }

    #[test]
    fn ingress_resolver_never_uses_raw_xff_without_a_configured_proxy() {
        let headers = forwarded_headers("198.51.100.250");
        let peer = "10.0.0.3:443".parse().unwrap();

        assert_eq!(resolve_trusted_client_ip(&headers, peer, 0, &[]), peer.ip());
        assert_eq!(
            resolve_trusted_client_ip(&headers, peer, 2, &["10.0.0.0/8".parse().unwrap()]),
            peer.ip(),
            "a short chain cannot turn a caller header into the trusted client"
        );
    }

    #[test]
    fn ingress_resolver_rejects_xff_from_an_untrusted_transport_peer() {
        let headers = forwarded_headers("198.51.100.250");
        let peer = "192.0.2.10:443".parse().unwrap();
        assert_eq!(
            resolve_trusted_client_ip(&headers, peer, 1, &["10.0.0.0/8".parse().unwrap()]),
            peer.ip()
        );
    }

    #[tokio::test]
    async fn ingress_middleware_reuses_proxy_policy_and_rejects_a_spoofed_suffix() {
        let mut app = with_trusted_client_ip(
            axum::Router::new().route(
                "/",
                get(
                    |Extension(client_ip): Extension<TrustedClientIp>| async move {
                        client_ip.get().to_string()
                    },
                ),
            ),
            2,
            vec!["10.0.0.0/8".parse().unwrap()],
        );
        let peer: SocketAddr = "10.0.0.3:443".parse().unwrap();

        for (forwarded_for, expected) in [
            ("198.51.100.250, 10.0.0.2", "198.51.100.250"),
            ("198.51.100.250, 203.0.113.7", "10.0.0.3"),
        ] {
            let mut request = Request::builder()
                .uri("/")
                .header("x-forwarded-for", forwarded_for)
                .body(Body::empty())
                .unwrap();
            request.extensions_mut().insert(ConnectInfo(peer));

            let response = Service::call(&mut app, request).await.unwrap();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(body.as_ref(), expected.as_bytes());
        }
    }

    #[tokio::test]
    async fn trusted_request_metadata_propagates_exactly_to_audit_context() {
        let mut request = request_with_network_metadata(
            "198.51.100.250, 203.0.113.7",
            "10.0.0.3:443",
            Some("203.0.113.7"),
        );
        request.headers_mut().insert(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
                .parse()
                .unwrap(),
        );
        request
            .headers_mut()
            .insert(http::header::USER_AGENT, "audit-test/1.0".parse().unwrap());
        request
            .headers_mut()
            .insert("x-device-id", "trusted-device".parse().unwrap());

        let expected = RequestAuditContext {
            trace: TraceContext::new("4bf92f3577b34da6a3ce929d0e0e4736", "00f067aa0ba902b7")
                .unwrap(),
            request: AuditRequestContext {
                ip: Some("203.0.113.7".to_owned()),
                user_agent: Some("audit-test/1.0".to_owned()),
                auth_method: Some("bearer".to_owned()),
                device: Some("trusted-device".to_owned()),
            },
        };
        assert_eq!(request_audit_context(&request), expected);
        CURRENT_AUDIT_CONTEXT
            .scope(expected.clone(), async {
                assert_eq!(current_audit_context(), Some(expected));
            })
            .await;
        assert_eq!(current_audit_context(), None);
    }

    #[tokio::test]
    async fn nested_audit_context_scope_restores_outer_then_clears() {
        let outer = RequestAuditContext {
            trace: TraceContext::new("11111111111111111111111111111111", "1111111111111111")
                .unwrap(),
            request: AuditRequestContext {
                device: Some("outer-device".to_owned()),
                ..AuditRequestContext::default()
            },
        };
        let inner = RequestAuditContext {
            trace: TraceContext::new("22222222222222222222222222222222", "2222222222222222")
                .unwrap(),
            request: AuditRequestContext {
                device: Some("inner-device".to_owned()),
                ..AuditRequestContext::default()
            },
        };

        assert_eq!(current_audit_context(), None);
        CURRENT_AUDIT_CONTEXT
            .scope(outer.clone(), async {
                assert_eq!(current_audit_context(), Some(outer.clone()));
                CURRENT_AUDIT_CONTEXT
                    .scope(inner.clone(), async {
                        assert_eq!(current_audit_context(), Some(inner));
                    })
                    .await;
                assert_eq!(current_audit_context(), Some(outer));
            })
            .await;
        assert_eq!(current_audit_context(), None);
    }

    #[test]
    fn delegated_group_admin_principal_does_not_gain_executive_queue_triage() -> Result<(), String>
    {
        let principal = Principal::new(
            UserId::new(),
            OrgId::new(),
            BTreeSet::from([Role::Admin]),
            BranchScope::All,
        );

        let err = match authorize_org_wide(&principal, Action::new(Feature::OrgWideQueueTriage)) {
            Ok(()) => {
                return Err(
                    "delegated group-admin tenant context gained executive queue triage".to_owned(),
                );
            }
            Err(err) => err,
        };
        assert_eq!(err.kind, ErrorKind::Forbidden);
        Ok(())
    }
}
