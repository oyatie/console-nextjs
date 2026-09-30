//! PLATFORM tenant-context helpers — read-only "view as", writable tenant
//! management context, and the blanket read-only method gate.
//!
//! # What this is
//! A platform super-admin (vendor tier; the `platform` JWT claim) can mint a
//! SHORT-LIVED token to view ANY tenant org AND role exactly as that tenant/role
//! sees it — strictly READ-ONLY — for troubleshooting, then exit.
//!
//! # Why it is safe (defense-in-depth)
//! "View as" deliberately crosses the tenant-isolation boundary, so security is
//! paramount. Three independent guarantees stack:
//!
//! 1. **Only the platform tier can START.** [`start_view_as`] runs behind the
//!    PLATFORM extractor ([`console_platform_request_context::with_platform_context`]),
//!    which rejects any tenant token with 403 before the handler runs. The real
//!    operator id is taken from the VERIFIED platform token (the `PlatformPrincipal`
//!    in the request extensions), never from the request body — it is unspoofable.
//!
//! 2. **The minted token is a TENANT token pinned to the target tenant.** It sets
//!    `platform = false`, `org = acting_org_id`, `roles = [acting_role]`, and the
//!    flags `view_as = true` + `read_only = true`. Because it is an ordinary
//!    tenant token in every structural respect, it flows through the unchanged
//!    tenant org middleware, which arms `app.current_org = acting_org_id` (RLS
//!    returns the TARGET tenant's rows) and builds a `Principal` with
//!    `roles = [acting_role]` (authz sees exactly what that role sees). It can read
//!    ONLY `acting_org_id` — RLS makes any other tenant invisible.
//!
//! 3. **A blanket read-only method gate.** [`with_view_as_read_only_gate`] wraps
//!    the WHOLE tenant router. For ANY request whose verified bearer token carries
//!    `view_as = true`, it REJECTS every method that is not GET/HEAD with 403
//!    `view_as_read_only` — BEFORE any handler or per-handler authz runs. No
//!    mutation handler is reachable, regardless of the acting role. This does NOT
//!    rely on per-handler authz; it is an orthogonal, method-level wall.
//!
//! # Exit
//! [`exit_view_as`] is a PLATFORM-tier endpoint (so it is reachable while a view_as
//! token is NOT in play — the web app holds the operator's platform token to call
//! it). It audits `platform.view_as.stop`. The token is stateless and also simply
//! expires on its own short TTL; EXIT exists for the audit trail and the explicit
//! UX of leaving.
//!
//! Writable tenant management uses the same issuer and platform-only envelope,
//! but mints a short-lived tenant token with `SUPER_ADMIN`, `view_as=false`, and
//! `read_only=false`. That path is intentionally pinned to exactly one active org
//! and audited separately (`platform.tenant_context.*`) so platform operators can
//! fix tenant org/user setup without introducing unscoped cross-tenant writes.

use std::str::FromStr;

use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use console_kernel_core::{AuditAction, AuditEvent, OrgId, TraceContext, UserId};
use console_platform_auth::{AccessClaims, AccessTokenInput, ActorSession, JwtVerifier};
use console_platform_authz::{PlatformPrincipal, Role};
use console_platform_db::{
    DbError, SubjectAuthzFreshness, read_subject_authz_freshness, with_audit,
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{PlatformError, PlatformRestState};

pub const PLATFORM_VIEW_AS_START_PATH: &str = "/api/platform/view-as";
pub const PLATFORM_VIEW_AS_EXIT_PATH: &str = "/api/platform/view-as/exit";
pub const PLATFORM_TENANT_CONTEXT_START_PATH: &str = "/api/platform/tenant-context";
pub const PLATFORM_TENANT_CONTEXT_EXIT_PATH: &str = "/api/platform/tenant-context/exit";

/// Hard cap on an impersonation token's lifetime. The START handler clamps any
/// requested TTL to this ceiling; the spec requires ≤30 min so a leaked
/// impersonation token's blast radius stays small.
pub const VIEW_AS_TOKEN_TTL: Duration = Duration::minutes(30);
/// Hard cap on a platform-managed tenant context. Keep it short like view-as so
/// a leaked context token cannot stay writable for a normal session lifetime.
pub const TENANT_CONTEXT_TOKEN_TTL: Duration = Duration::minutes(30);

/// Error code returned by the read-only gate for a blocked mutation. A stable
/// string so the web client can detect it and surface "read-only" UX.
pub const VIEW_AS_READ_ONLY_CODE: &str = "view_as_read_only";

// ---------------------------------------------------------------------------
// START / EXIT DTOs
// ---------------------------------------------------------------------------

/// START request body. Carries ONLY the target tenant + role; the operator
/// identity is taken from the verified platform token, never from here.
#[derive(Debug, Deserialize)]
struct ViewAsStartRequest {
    /// The tenant to view. Must be a real, ACTIVE organization.
    org_id: Uuid,
    /// The tenant role to impersonate, e.g. `ADMIN` / `MECHANIC`. Validated
    /// against the canonical [`Role`] set; an unknown code is rejected.
    role: String,
}

/// The minted impersonation token plus the context the web app needs to render
/// its banner. The token is a normal bearer access token to attach as
/// `Authorization: Bearer` on tenant `/api/v1/*` calls.
#[derive(Debug, Serialize)]
struct ViewAsStartResponse {
    /// The short-lived impersonation access token (view_as = read_only = true).
    access_token: String,
    token_type: &'static str,
    /// Acting tenant id (echoed for the client banner).
    acting_org_id: Uuid,
    /// Acting tenant name (for the banner label).
    acting_org_name: String,
    /// Acting role (echoed for the client banner).
    acting_role: String,
    /// Absolute expiry of the impersonation token.
    #[serde(with = "time::serde::rfc3339")]
    expires_at: OffsetDateTime,
}

/// START request for a writable platform-managed tenant context. The platform
/// operator supplies only the target tenant; the role is fixed to SUPER_ADMIN so
/// the UI can manage that tenant's org/users through ordinary tenant routes
/// without adding a second cross-tenant write API surface.
#[derive(Debug, Deserialize)]
struct TenantContextStartRequest {
    /// The tenant to manage. Must be a real, ACTIVE organization.
    org_id: Uuid,
}

/// The short-lived tenant-admin token plus banner context.
#[derive(Debug, Serialize)]
struct TenantContextStartResponse {
    /// The short-lived tenant access token (view_as = read_only = false).
    access_token: String,
    token_type: &'static str,
    /// Acting tenant id (echoed for the client banner).
    acting_org_id: Uuid,
    /// Acting tenant name (for the banner label).
    acting_org_name: String,
    /// Fixed acting role (`SUPER_ADMIN`) used by the tenant app.
    acting_role: String,
    /// Absolute expiry of the tenant context token.
    #[serde(with = "time::serde::rfc3339")]
    expires_at: OffsetDateTime,
}

/// EXIT acknowledgement.
#[derive(Debug, Serialize)]
struct ViewAsExitResponse {
    ended: bool,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Append the view-as START + EXIT routes to the platform `/api/platform/*`
/// router. Both are PLATFORM-tier routes (gated by the platform extractor the
/// caller applies); EXIT must be platform-scoped so it is reachable with the
/// operator's platform token rather than the impersonation token.
pub(crate) fn routes(router: Router<PlatformRestState>) -> Router<PlatformRestState> {
    router
        .route(PLATFORM_VIEW_AS_START_PATH, post(start_view_as))
        .route(PLATFORM_VIEW_AS_EXIT_PATH, post(exit_view_as))
        .route(
            PLATFORM_TENANT_CONTEXT_START_PATH,
            post(start_tenant_context),
        )
        .route(PLATFORM_TENANT_CONTEXT_EXIT_PATH, post(exit_tenant_context))
}

// ---------------------------------------------------------------------------
// START
// ---------------------------------------------------------------------------

/// POST /api/platform/view-as — mint a short-lived READ-ONLY impersonation token.
///
/// AUTHZ: behind the PLATFORM extractor, so a tenant token is already rejected
/// (403) before this runs. The `PlatformPrincipal` (operator) comes from the
/// verified platform token in the request extensions — unspoofable.
///
/// The minted token is a TENANT token pinned to the target tenant + role with
/// `view_as = read_only = true`. Audited as `platform.view_as.start` with the
/// REAL operator id and org_id = NULL (platform tier).
async fn start_view_as(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
    Extension(source_claims): Extension<AccessClaims>,
    Json(body): Json<ViewAsStartRequest>,
) -> Result<Response, PlatformError> {
    // The view-as capability is part of the platform tier's authority. Today the
    // platform principal holds the full set; gate on TenantHealthRead (the
    // read-oriented capability) so a future subsetting of platform RBAC can deny
    // troubleshooting view-as without touching code.
    principal
        .authorize(console_platform_authz::PlatformFeature::TenantHealthRead)
        .map_err(|_| PlatformError::forbidden("platform principal cannot view as a tenant"))?;

    let issuer = state.view_as_issuer.as_ref().ok_or_else(|| {
        PlatformError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            "token issuance is not configured",
        )
    })?;

    // Validate the requested role against the canonical set. An unknown code is a
    // client error, not a 500.
    let acting_role =
        Role::from_str(&body.role).map_err(|_| PlatformError::validation("unknown role code"))?;

    let acting_org = OrgId::from_uuid(body.org_id);

    // The platform sentinel is NOT a real tenant: refuse to "view as" it so the
    // impersonation token can never be minted against a non-tenant org.
    if acting_org == OrgId::platform() {
        return Err(PlatformError::validation(
            "cannot view as the platform tier",
        ));
    }

    // Confirm the target tenant exists AND is ACTIVE via the sanctioned
    // cross-tenant read (`platform_list_organizations()` is SECURITY DEFINER).
    // Refusing a missing/suspended/archived tenant keeps impersonation scoped to
    // live tenants and gives the operator a clear 404/409 instead of a token that
    // silently sees zero rows.
    let org = lookup_active_org(&state.pool, body.org_id).await?;

    // Source REAL subject freshness for the token's OWN (target org, operator) so
    // a promoted Cedar guard — which re-reads exactly that (org, user) at guard
    // time — does not falsely deny this token as stale/missing. The operator has
    // no `users` row in the target tenant, so subject/session read as the absent
    // 0 baseline; the target org's `policy_version` is real.
    let freshness = read_freshness_for_mint(&state.pool, acting_org, principal.user_id).await?;

    let now = OffsetDateTime::now_utc();
    let expires_at = (now + VIEW_AS_TOKEN_TTL).min(
        OffsetDateTime::from_unix_timestamp(source_claims.exp).map_err(|_| {
            PlatformError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "invalid session expiry",
            )
        })?,
    );

    // Mint the impersonation token. It is a TENANT token (`platform = false`)
    // pinned to the TARGET org and role, carrying the read-only flags. `sub` is
    // the REAL operator id from the verified platform token — never the body.
    let access_token = issuer
        .issue_platform_tenant_context_access_token(
            AccessTokenInput {
                subject: principal.user_id,
                org_id: acting_org,
                roles: vec![acting_role.as_str().to_owned()],
                // No branch claim: the tenant middleware re-resolves the live
                // branch scope from the DB for the acting role under the target
                // org's RLS, so the operator sees exactly that role's scope.
                branches: Vec::new(),
                platform: false,
                view_as: true,
                read_only: true,
                // No display name on an impersonation token: the persistent
                // view-as banner already names the acting tenant/role, and the
                // operator's own identity is audited separately at start/exit.
                display_name: None,
                feature_grants: Vec::new(),
                // Real subject freshness for (target org, operator): a promoted
                // Cedar guard re-reads the same (org, user), so a fresh token
                // satisfies the freshness requirement instead of tripping a false
                // MissingSubjectFreshness/StaleSubject deny.
                authz_subject_version: freshness.subject_version,
                authz_policy_version: freshness.policy_version,
                session_generation: freshness.session_generation,
                issued_at: now,
            },
            ActorSession {
                family_id: source_claims.session_family_id.ok_or_else(|| {
                    PlatformError::new(
                        StatusCode::UNAUTHORIZED,
                        "unauthorized",
                        "missing session family",
                    )
                })?,
                expires_at: source_claims.exp,
                home_org: OrgId::platform(),
                subject_version: source_claims.authz_subject_version,
                session_generation: source_claims.session_generation,
            },
            VIEW_AS_TOKEN_TTL,
        )
        .map_err(|err| {
            tracing::error!(error = %err, "failed to mint view-as token");
            PlatformError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal server error",
            )
        })?;

    // Audit the START with the REAL operator id and org_id = NULL (platform tier).
    // The target tenant + role go in the snapshot for the trail. No GUC is armed
    // (org_id None), so the NULL-org platform audit row is allowed.
    let after = serde_json::json!({
        "acting_org_id": body.org_id,
        "acting_role": acting_role.as_str(),
        "read_only": true,
    });
    write_platform_audit(
        &state.pool,
        principal.user_id,
        "platform.view_as.start",
        body.org_id.to_string(),
        after,
        now,
    )
    .await?;

    Ok(Json(ViewAsStartResponse {
        access_token,
        token_type: "Bearer",
        acting_org_id: body.org_id,
        acting_org_name: org.name,
        acting_role: acting_role.as_str().to_owned(),
        expires_at,
    })
    .into_response())
}

// ---------------------------------------------------------------------------
// EXIT
// ---------------------------------------------------------------------------

/// POST /api/platform/view-as/exit — end an impersonation session (audit only).
///
/// PLATFORM-tier: reached with the operator's platform token (NOT the
/// impersonation token), so the operator id is again unspoofable. The
/// impersonation token is stateless and also expires on its own; this endpoint
/// records `platform.view_as.stop` and lets the web app drop the token and
/// return to the console.
async fn exit_view_as(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
) -> Result<Response, PlatformError> {
    principal
        .authorize(console_platform_authz::PlatformFeature::TenantHealthRead)
        .map_err(|_| PlatformError::forbidden("platform principal cannot exit view as"))?;

    let now = OffsetDateTime::now_utc();
    write_platform_audit(
        &state.pool,
        principal.user_id,
        "platform.view_as.stop",
        "exit".to_owned(),
        serde_json::json!({ "ended": true }),
        now,
    )
    .await?;

    Ok(Json(ViewAsExitResponse { ended: true }).into_response())
}

// ---------------------------------------------------------------------------
// WRITABLE TENANT MANAGEMENT CONTEXT
// ---------------------------------------------------------------------------

/// POST /api/platform/tenant-context — mint a short-lived WRITABLE tenant token.
///
/// AUTHZ: platform tier only. Unlike read-only view-as, this token intentionally
/// sets `view_as=false` + `read_only=false` and carries `SUPER_ADMIN` so the
/// platform operator can manage one selected tenant through the existing
/// tenant-scoped routes and RLS boundary. It is still pinned to exactly one org,
/// short-lived, and audited with the real platform operator.
async fn start_tenant_context(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
    Extension(source_claims): Extension<AccessClaims>,
    Json(body): Json<TenantContextStartRequest>,
) -> Result<Response, PlatformError> {
    principal
        .authorize(console_platform_authz::PlatformFeature::TenantManage)
        .map_err(|_| PlatformError::forbidden("platform principal cannot manage a tenant"))?;

    let issuer = state.view_as_issuer.as_ref().ok_or_else(|| {
        PlatformError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            "token issuance is not configured",
        )
    })?;

    let acting_org = OrgId::from_uuid(body.org_id);
    if acting_org == OrgId::platform() {
        return Err(PlatformError::validation(
            "cannot manage the platform tier as a tenant",
        ));
    }

    let org = lookup_active_org(&state.pool, body.org_id).await?;

    // Real subject freshness for the token's OWN (target org, operator) — see
    // `start_view_as`. This is the highest-blast-radius mint (writable
    // SUPER_ADMIN), so carrying a matching snapshot is what lets the token pass a
    // promoted Cedar freshness gate rather than being denied as stale.
    let freshness = read_freshness_for_mint(&state.pool, acting_org, principal.user_id).await?;

    let now = OffsetDateTime::now_utc();
    let expires_at = (now + TENANT_CONTEXT_TOKEN_TTL).min(
        OffsetDateTime::from_unix_timestamp(source_claims.exp).map_err(|_| {
            PlatformError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "invalid session expiry",
            )
        })?,
    );
    let acting_role = Role::SuperAdmin;

    let access_token = issuer
        .issue_platform_tenant_context_access_token(
            AccessTokenInput {
                subject: principal.user_id,
                org_id: acting_org,
                roles: vec![acting_role.as_str().to_owned()],
                branches: Vec::new(),
                platform: false,
                view_as: false,
                read_only: false,
                display_name: None,
                feature_grants: Vec::new(),
                // Real subject freshness for (target org, operator) — see
                // `start_view_as`: a promoted Cedar guard re-reads the same
                // (org, user), so a fresh token is not falsely denied as stale.
                authz_subject_version: freshness.subject_version,
                authz_policy_version: freshness.policy_version,
                session_generation: freshness.session_generation,
                issued_at: now,
            },
            ActorSession {
                family_id: source_claims.session_family_id.ok_or_else(|| {
                    PlatformError::new(
                        StatusCode::UNAUTHORIZED,
                        "unauthorized",
                        "missing session family",
                    )
                })?,
                expires_at: source_claims.exp,
                home_org: OrgId::platform(),
                subject_version: source_claims.authz_subject_version,
                session_generation: source_claims.session_generation,
            },
            TENANT_CONTEXT_TOKEN_TTL,
        )
        .map_err(|err| {
            tracing::error!(error = %err, "failed to mint tenant management token");
            PlatformError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal server error",
            )
        })?;

    write_platform_audit(
        &state.pool,
        principal.user_id,
        "platform.tenant_context.start",
        body.org_id.to_string(),
        serde_json::json!({
            "acting_org_id": body.org_id,
            "acting_role": acting_role.as_str(),
            "read_only": false,
        }),
        now,
    )
    .await?;

    Ok(Json(TenantContextStartResponse {
        access_token,
        token_type: "Bearer",
        acting_org_id: body.org_id,
        acting_org_name: org.name,
        acting_role: acting_role.as_str().to_owned(),
        expires_at,
    })
    .into_response())
}

/// POST /api/platform/tenant-context/exit — end a writable tenant context
/// (audit only). Like view-as exit, the SPA calls this with the operator's real
/// platform token after restoring the local platform session.
async fn exit_tenant_context(
    State(state): State<PlatformRestState>,
    Extension(principal): Extension<PlatformPrincipal>,
) -> Result<Response, PlatformError> {
    principal
        .authorize(console_platform_authz::PlatformFeature::TenantManage)
        .map_err(|_| PlatformError::forbidden("platform principal cannot exit tenant context"))?;

    let now = OffsetDateTime::now_utc();
    write_platform_audit(
        &state.pool,
        principal.user_id,
        "platform.tenant_context.stop",
        "exit".to_owned(),
        serde_json::json!({ "ended": true }),
        now,
    )
    .await?;

    Ok(Json(ViewAsExitResponse { ended: true }).into_response())
}

// ---------------------------------------------------------------------------
// READ-ONLY METHOD GATE (highest-risk control)
// ---------------------------------------------------------------------------

/// Wrap a router so that ANY request carrying a `view_as` token may ONLY use a
/// safe method (GET/HEAD). Every other method is rejected with 403
/// `view_as_read_only` BEFORE the inner router (and any handler) runs.
///
/// This is the bulletproof, defense-in-depth read-only wall: it is a blanket
/// method gate keyed purely on the token's `view_as` claim, independent of the
/// acting role and of any per-handler authz. A mutation handler is simply
/// unreachable while impersonating.
///
/// It verifies the bearer token with the provided [`JwtVerifier`]. A token that
/// is absent/invalid/non-view_as is passed through untouched — this layer ONLY
/// blocks unsafe methods on a *valid view_as* token; ordinary auth/authz for
/// every other case stays exactly where it was (the tenant middleware downstream).
pub fn with_view_as_read_only_gate<S>(router: Router<S>, verifier: Option<JwtVerifier>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.layer(axum::middleware::from_fn(
        move |request: Request, next: Next| {
            let verifier = verifier.clone();
            async move {
                if request_is_view_as(verifier.as_ref(), &request)
                    && !is_safe_method(request.method())
                {
                    return read_only_rejection();
                }
                next.run(request).await
            }
        },
    ))
}

/// GET and HEAD are the only methods a view_as token may use. (OPTIONS is a
/// CORS preflight and carries no auth/side effects, but we are conservative:
/// only GET/HEAD pass, everything else is blocked. CORS preflights do not carry
/// the Authorization header, so they are not `view_as` requests and never reach
/// this branch.)
fn is_safe_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD)
}

/// True when the request presents a bearer token that verifies AND carries
/// `view_as = true`. A missing/invalid/non-view_as token is `false` (this gate
/// only acts on a *valid* impersonation token; all other auth is downstream).
fn request_is_view_as(verifier: Option<&JwtVerifier>, request: &Request) -> bool {
    let Some(verifier) = verifier else {
        return false;
    };
    let Some(token) = bearer_token(request) else {
        return false;
    };
    match verifier.verify_access_token(token) {
        Ok(claims) => claims.view_as,
        Err(_) => false,
    }
}

/// Extract a raw bearer token from the Authorization header, if present.
fn bearer_token(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

/// The 403 returned for a blocked mutation under a view_as token.
fn read_only_rejection() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": {
                "code": VIEW_AS_READ_ONLY_CODE,
                "message": "view-as sessions are read-only; mutations are not permitted",
            }
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A minimal view of a tenant org for the START response/validation.
struct ActiveOrg {
    name: String,
}

/// Look up a tenant by id via the sanctioned cross-tenant read and require it to
/// be ACTIVE. Returns 404 when no such tenant exists and 409 when it is not
/// ACTIVE (suspended/archived), so impersonation is scoped to live tenants.
async fn lookup_active_org(pool: &PgPool, org_id: Uuid) -> Result<ActiveOrg, PlatformError> {
    let mut tx = pool.begin().await.map_err(|err| internal(&err))?;
    let row = sqlx::query(
        r#"
        SELECT name, status
        FROM platform_list_organizations()
        WHERE id = $1
        "#,
    )
    .bind(org_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(|err| internal(&err))?;
    tx.commit().await.map_err(|err| internal(&err))?;

    let Some(row) = row else {
        return Err(PlatformError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "no such tenant",
        ));
    };
    let name: String = row.try_get("name").map_err(|err| internal(&err))?;
    let status: String = row.try_get("status").map_err(|err| internal(&err))?;
    if status != "ACTIVE" {
        return Err(PlatformError::new(
            StatusCode::CONFLICT,
            "conflict",
            "tenant is not active; cannot view as a suspended or archived tenant",
        ));
    }
    Ok(ActiveOrg { name })
}

/// Write a PLATFORM-tier audit row (org_id = NULL) with the real operator id.
///
/// Mirrors the existing platform audit pattern: `with_audit` with an event that
/// carries NO org (so the GUC is left unset and the NULL-org platform audit row
/// is permitted). The closure performs no mutation — the audit row IS the record.
async fn write_platform_audit(
    pool: &PgPool,
    operator: UserId,
    action: &str,
    target_id: String,
    after: serde_json::Value,
    now: OffsetDateTime,
) -> Result<(), PlatformError> {
    let event = AuditEvent::new(
        Some(operator),
        AuditAction::new(action).map_err(|err| {
            tracing::error!(error = %err, "invalid view-as audit action");
            PlatformError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal server error",
            )
        })?,
        "organizations",
        target_id,
        TraceContext::generate(),
        now,
    )
    .with_snapshots(None, Some(after));

    with_audit::<_, (), DbError>(pool, event, |_tx| Box::pin(async move { Ok(()) }))
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "failed to write view-as audit");
            PlatformError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal server error",
            )
        })
}

/// Read the DB-current subject freshness for a token's own `(org, user)` before
/// minting, mapping a read failure to a 500 (never a silent zero stamp). Both the
/// read-only view-as and the writable tenant-context mints share it. The `org` is
/// the TARGET tenant and `user` is the platform operator; the read arms that
/// org's RLS GUC internally.
async fn read_freshness_for_mint(
    pool: &PgPool,
    org: OrgId,
    user: UserId,
) -> Result<SubjectAuthzFreshness, PlatformError> {
    read_subject_authz_freshness(pool, org, user)
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "failed to read subject freshness for mint");
            PlatformError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal server error",
            )
        })
}

fn internal(err: &dyn std::fmt::Display) -> PlatformError {
    tracing::error!(error = %err, "view-as platform error");
    PlatformError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "internal server error",
    )
}
