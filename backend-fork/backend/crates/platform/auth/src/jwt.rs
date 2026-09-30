use console_kernel_core::{AccessScope, AccessScopeLevel, BranchId, OrgId, ScopeNodeId, UserId};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use time::Duration;
use uuid::Uuid;

use crate::AuthError;

#[derive(Debug, Clone)]
pub struct JwtSettings {
    pub issuer: String,
    pub audience: String,
    pub access_token_ttl: Duration,
}

#[derive(Debug, Clone)]
pub struct AccessTokenInput {
    pub subject: UserId,
    /// The tenant the authenticated user belongs to. Sourced from
    /// `users.org_id` at issuance — never a hardcoded default — and stamped into
    /// the `org` claim so every downstream request can arm `app.current_org`.
    ///
    /// For a PLATFORM token (`platform = true`) this carries the platform
    /// sentinel [`OrgId::platform`]: a platform principal is NOT tenant-scoped,
    /// so its `org` is a non-tenant marker that can never arm a real tenant's
    /// RLS GUC.
    pub org_id: OrgId,
    pub roles: Vec<String>,
    pub branches: Vec<BranchId>,
    /// `true` marks a SaaS-vendor PLATFORM token (the tier ABOVE all tenants).
    /// A platform token is rejected on tenant `/api/*` routes, and a tenant
    /// token is rejected on `/api/platform/*` — the two tiers are strictly
    /// separated. Tenant tokens always set this `false`.
    pub platform: bool,
    /// PLATFORM "view as" impersonation marker. When `true` this is a short-lived
    /// token minted by a platform operator to view a tenant strictly READ-ONLY.
    /// It is a TENANT-tier token (`platform = false`, `org_id = acting tenant`),
    /// so it flows through the tenant org middleware and arms `app.current_org`
    /// to the target tenant — but the blanket read-only method gate rejects every
    /// non-GET/HEAD request that carries it. An ordinary token always sets this
    /// `false`.
    pub view_as: bool,
    /// Companion to [`Self::view_as`]: the impersonation token is read-only. Kept
    /// as a distinct flag (not implied by `view_as`) so the read-only contract is
    /// explicit in the token and a future non-read-only impersonation mode would
    /// be an additive change, never a silent reinterpretation.
    pub read_only: bool,
    /// The authenticated user's display name, stamped into the optional `name`
    /// claim for DISPLAY ONLY (topbar identity, etc.). Never consulted for
    /// authorization. `None` omits the claim entirely, which keeps view-as and
    /// any future operator-less mint backward compatible.
    pub display_name: Option<String>,
    /// Runtime-effective custom-role feature keys resolved at token issuance for
    /// client-side UI hints only. Backend authorization ignores this claim and
    /// re-resolves effective custom-role grants from the database on every
    /// request, so a stale token can hide or reveal UI chrome but cannot grant
    /// access.
    pub feature_grants: Vec<String>,
    /// Subject authorization freshness snapshot, sourced from the DB at mint time
    /// (Cedar/PBAC activation, ADR-0021). Ordinary sessions must match current
    /// subject/session generations at the request boundary. Delegated tokens
    /// preserve target freshness here and validate the home account separately
    /// through `actor_session`. Zero is valid only while no version row exists.
    pub authz_subject_version: u64,
    pub authz_policy_version: u64,
    pub session_generation: u64,
    pub issued_at: time::OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantAccessContext {
    /// A short-lived tenant context minted for a live GROUP_ADMIN so they can
    /// manage one subsidiary without becoming that tenant's SUPER_ADMIN.
    GroupAdmin,
}

/// Original authenticated account binding, distinct from a delegated target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorSession {
    pub family_id: Uuid,
    pub expires_at: i64,
    pub home_org: OrgId,
    pub subject_version: u64,
    pub session_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessClaims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub iat: i64,
    pub nbf: i64,
    pub exp: i64,
    pub jti: String,
    /// The user's tenant id, as a UUID string. Verification rejects a token
    /// whose `org` does not parse as a UUID, so callers can trust it. On a
    /// PLATFORM token this is the platform sentinel [`OrgId::platform`].
    pub org: String,
    pub roles: Vec<String>,
    pub branches: Vec<String>,
    /// `true` for a SaaS-vendor PLATFORM token (cross-tenant, NOT tenant-scoped).
    /// Absent in legacy tokens → defaults to `false` (a tenant token), so old
    /// tenant tokens keep their exact meaning and can never be mistaken for a
    /// platform token.
    #[serde(default)]
    pub platform: bool,
    /// `true` for a PLATFORM "view as" impersonation token (read-only tenant
    /// view minted by a platform operator). Absent in every ordinary token →
    /// defaults to `false`, so a normal tenant/platform token is never mistaken
    /// for an impersonation token. The read-only method gate keys off this flag.
    #[serde(default)]
    pub view_as: bool,
    /// `true` when the `view_as` token is read-only. Defaults to `false` when
    /// absent. Today every `view_as` token sets this `true`; it is a separate,
    /// explicit claim so the read-only contract is self-describing.
    #[serde(default)]
    pub read_only: bool,
    /// The user's display name, for DISPLAY ONLY (e.g. the topbar identity).
    /// Optional and never used for authorization. Absent in legacy tokens and
    /// omitted from the wire when `None`, so old tokens remain valid verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional hierarchy-scope level. Missing with `scope_node` missing means
    /// the legacy tenant-wide scope from the `org` claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_level: Option<AccessScopeLevel>,
    /// Optional hierarchy-scope node id. Must be present exactly when
    /// `scope_level` is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_node: Option<ScopeNodeId>,
    /// Group roles are carried for future group-scoped authorization. They are
    /// never allowed on read-only platform view-as tokens.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_roles: Vec<String>,
    /// Optional delegated tenant-context marker. Ordinary tenant tokens omit it.
    /// A `group_admin` token must carry `group_context_id`, must not carry
    /// `SUPER_ADMIN`, and must be live-revalidated by the request-context layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_context: Option<TenantAccessContext>,
    /// Group id associated with a delegated group-admin tenant context.
    /// Required only when `tenant_context = group_admin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_context_id: Option<String>,
    /// The delegated actor's source/home tenant, distinct from `org`, which is
    /// the target tenant context. Retained as an optional wire field for legacy
    /// codec compatibility; current group delegation requires it to match
    /// `actor_session` at the request boundary. Invalid on non-group tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_home_org: Option<String>,
    /// Delegation preserves the validated source generations, never refreshed
    /// generations read after source authentication. Legacy delegates without
    /// this binding must authenticate again at the request boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_session: Option<ActorSession>,
    /// Ordinary access is bound to an existing live refresh family. Legacy
    /// unbound signatures remain decodable but cannot authenticate requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_family_id: Option<Uuid>,
    /// Runtime-effective custom-role feature keys for client-side nav/route
    /// gating hints. These are never consulted by backend authz; request
    /// principals resolve the live custom policy from the DB on every request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feature_grants: Vec<String>,
    /// Target subject/policy/session freshness for Cedar. Ordinary sessions
    /// also validate subject/session against current PostgreSQL authority on
    /// every request. Delegated home freshness is in `actor_session`. Legacy
    /// absent claims default to zero and pass only an unchanged zero baseline.
    #[serde(default)]
    pub authz_subject_version: u64,
    #[serde(default)]
    pub authz_policy_version: u64,
    #[serde(default)]
    pub session_generation: u64,
    pub alg: String,
}

impl AccessClaims {
    /// No grace beyond a signed session deadline, including after database waits.
    pub fn validate_expiry(&self) -> Result<(), AuthError> {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        if self.exp <= now
            || self
                .actor_session
                .is_some_and(|s| s.expires_at <= now || self.exp > s.expires_at)
        {
            return Err(AuthError::Jwt(
                jsonwebtoken::errors::ErrorKind::ExpiredSignature.into(),
            ));
        }
        Ok(())
    }

    /// Resolve the claim-level hierarchy scope.
    ///
    /// Back-compatibility rule: tokens without scope claims keep today's
    /// meaning, i.e. tenant-wide access to the `org` claim.
    pub fn access_scope(&self) -> Result<AccessScope, AuthError> {
        match (self.scope_level, self.scope_node) {
            (None, None) => {
                let org_id = OrgId::from_str(&self.org).map_err(|_| {
                    AuthError::InvalidStoredData("token org claim is not a valid uuid".to_owned())
                })?;
                Ok(AccessScope::legacy_org(org_id))
            }
            (Some(level), Some(node_id)) => Ok(AccessScope::new(level, node_id)),
            _ => Err(AuthError::InvalidStoredData(
                "token scope claims must include both scope_level and scope_node".to_owned(),
            )),
        }
    }
}

fn validate_group_roles(group_roles: &[String]) -> Result<(), AuthError> {
    for role in group_roles {
        match role.as_str() {
            "GROUP_ADMIN" | "GROUP_VIEWER" | "GROUP_FINANCE" => {}
            _ => {
                return Err(AuthError::InvalidStoredData(format!(
                    "unknown group role code: {role}"
                )));
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_tenant_context(
    tenant_context: Option<TenantAccessContext>,
    group_context_id: Option<&str>,
    actor_home_org: Option<&str>,
    roles: &[String],
    group_roles: &[String],
    platform: bool,
    view_as: bool,
    read_only: bool,
) -> Result<(), AuthError> {
    match tenant_context {
        None => {
            if group_context_id.is_some() {
                return Err(AuthError::InvalidStoredData(
                    "group_context_id requires tenant_context".to_owned(),
                ));
            }
            if actor_home_org.is_some() {
                return Err(AuthError::InvalidStoredData(
                    "actor_home_org requires group-admin tenant context".to_owned(),
                ));
            }
        }
        Some(TenantAccessContext::GroupAdmin) => {
            if platform || view_as || read_only {
                return Err(AuthError::InvalidStoredData(
                    "group-admin tenant context must be writable tenant-tier only".to_owned(),
                ));
            }
            let Some(group_context_id) = group_context_id else {
                return Err(AuthError::InvalidStoredData(
                    "group-admin tenant context requires group_context_id".to_owned(),
                ));
            };
            Uuid::parse_str(group_context_id).map_err(|_| {
                AuthError::InvalidStoredData(
                    "group-admin tenant context group_context_id is not a uuid".to_owned(),
                )
            })?;
            if let Some(actor_home_org) = actor_home_org {
                Uuid::parse_str(actor_home_org).map_err(|_| {
                    AuthError::InvalidStoredData(
                        "group-admin tenant context actor_home_org is not a uuid".to_owned(),
                    )
                })?;
            }
            if !group_roles.iter().any(|role| role == "GROUP_ADMIN") {
                return Err(AuthError::InvalidStoredData(
                    "group-admin tenant context requires GROUP_ADMIN group role".to_owned(),
                ));
            }
            if roles.iter().any(|role| role == "SUPER_ADMIN") {
                return Err(AuthError::InvalidStoredData(
                    "group-admin tenant context cannot carry SUPER_ADMIN".to_owned(),
                ));
            }
            if roles.len() != 1 || roles.first().map(String::as_str) != Some("ADMIN") {
                return Err(AuthError::InvalidStoredData(
                    "group-admin tenant context must carry only ADMIN".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct JwtIssuer {
    settings: JwtSettings,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
}

#[derive(Clone, Debug)]
pub struct JwtVerifier {
    settings: JwtSettings,
    decoding_key: DecodingKey,
}

impl JwtIssuer {
    pub fn from_es256_pem(
        settings: JwtSettings,
        private_key_pem: &[u8],
        public_key_pem: &[u8],
    ) -> Result<Self, AuthError> {
        Ok(Self {
            settings,
            encoding_key: EncodingKey::from_ec_pem(private_key_pem)?,
            decoding_key: DecodingKey::from_ec_pem(public_key_pem)?,
        })
    }

    /// Sign an unbound compatibility token. Runtime sessions must use
    /// `issue_session_access_token`; an unbound signature cannot authenticate.
    pub fn issue_access_token(&self, input: AccessTokenInput) -> Result<String, AuthError> {
        self.issue_access_token_with_ttl(input, self.settings.access_token_ttl)
    }

    /// Mint ordinary runtime access, preserving explicit scope and bounded by
    /// the refresh owner's deadline. Scope never replaces the home Company.
    pub fn issue_session_access_token(
        &self,
        input: AccessTokenInput,
        access_scope: Option<AccessScope>,
        group_roles: Vec<String>,
        family_id: Uuid,
        deadline: time::OffsetDateTime,
    ) -> Result<String, AuthError> {
        let ttl = self
            .settings
            .access_token_ttl
            .min(deadline - input.issued_at);
        if (input.issued_at + ttl).unix_timestamp()
            <= time::OffsetDateTime::now_utc().unix_timestamp()
        {
            return Err(AuthError::Jwt(
                jsonwebtoken::errors::ErrorKind::ExpiredSignature.into(),
            ));
        }
        self.issue_access_token_inner(
            input,
            ttl,
            access_scope,
            group_roles,
            None,
            None,
            None,
            None,
            Some(family_id),
        )
    }

    /// Sign an unbound compatibility token that also carries group-role claims.
    ///
    /// The token remains tenant-scoped by its `org` claim; group authority is
    /// still re-resolved by backend endpoints from the live owner-only grant
    /// resolver before any cross-tenant action. The claim exists only so clients
    /// can reveal the group-admin console without granting access by itself.
    pub fn issue_access_token_with_group_roles(
        &self,
        input: AccessTokenInput,
        group_roles: Vec<String>,
    ) -> Result<String, AuthError> {
        self.issue_access_token_inner(
            input,
            self.settings.access_token_ttl,
            None,
            group_roles,
            None,
            None,
            None,
            None,
            None,
        )
    }

    pub fn issue_scoped_access_token(
        &self,
        input: AccessTokenInput,
        access_scope: AccessScope,
        group_roles: Vec<String>,
    ) -> Result<String, AuthError> {
        self.issue_access_token_inner(
            input,
            self.settings.access_token_ttl,
            Some(access_scope),
            group_roles,
            None,
            None,
            None,
            None,
            None,
        )
    }

    /// Mint the short-lived tenant token used by a GROUP_ADMIN to manage one
    /// resolver-authorized subsidiary. This is deliberately NOT a normal
    /// SUPER_ADMIN token: downstream request context re-checks the live group
    /// membership on every request and builds a bounded delegated principal.
    pub fn issue_group_admin_tenant_context_access_token(
        &self,
        input: AccessTokenInput,
        group_id: Uuid,
        actor_session: ActorSession,
        ttl: Duration,
    ) -> Result<String, AuthError> {
        self.issue_access_token_inner(
            input,
            ttl,
            None,
            vec!["GROUP_ADMIN".to_owned()],
            Some(TenantAccessContext::GroupAdmin),
            Some(group_id.to_string()),
            Some(actor_session.home_org.to_string()),
            Some(actor_session),
            None,
        )
    }

    /// Mint an access token with an EXPLICIT lifetime, overriding the issuer's
    /// default `access_token_ttl`.
    ///
    /// Used by the PLATFORM "view as" START path, which must mint a deliberately
    /// SHORT-LIVED (≤30 min) impersonation token regardless of the configured
    /// session TTL. `ttl` is clamped to a positive duration; a non-positive value
    /// is treated as a zero-length (immediately-expired) token rather than a
    /// long-lived one, so a misconfiguration fails closed.
    pub fn issue_access_token_with_ttl(
        &self,
        input: AccessTokenInput,
        ttl: Duration,
    ) -> Result<String, AuthError> {
        self.issue_access_token_inner(input, ttl, None, Vec::new(), None, None, None, None, None)
    }

    /// Platform delegation carries an explicit original account binding.
    pub fn issue_platform_tenant_context_access_token(
        &self,
        input: AccessTokenInput,
        actor_session: ActorSession,
        ttl: Duration,
    ) -> Result<String, AuthError> {
        self.issue_access_token_inner(
            input,
            ttl,
            None,
            Vec::new(),
            None,
            None,
            None,
            Some(actor_session),
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn issue_access_token_inner(
        &self,
        input: AccessTokenInput,
        ttl: Duration,
        access_scope: Option<AccessScope>,
        group_roles: Vec<String>,
        tenant_context: Option<TenantAccessContext>,
        group_context_id: Option<String>,
        actor_home_org: Option<String>,
        actor_session: Option<ActorSession>,
        session_family_id: Option<Uuid>,
    ) -> Result<String, AuthError> {
        if input.view_as && !group_roles.is_empty() {
            return Err(AuthError::InvalidStoredData(
                "view-as tokens cannot carry group roles".to_owned(),
            ));
        }
        validate_group_roles(&group_roles)?;
        validate_tenant_context(
            tenant_context,
            group_context_id.as_deref(),
            actor_home_org.as_deref(),
            &input.roles,
            &group_roles,
            input.platform,
            input.view_as,
            input.read_only,
        )?;
        let ttl = if ttl.is_positive() {
            ttl
        } else {
            Duration::ZERO
        };
        let issued_at = input.issued_at.unix_timestamp();
        let mut expires_at = (input.issued_at + ttl).unix_timestamp();
        if let Some(source) = actor_session {
            expires_at = expires_at.min(source.expires_at);
            if expires_at <= time::OffsetDateTime::now_utc().unix_timestamp() {
                return Err(AuthError::Jwt(
                    jsonwebtoken::errors::ErrorKind::ExpiredSignature.into(),
                ));
            }
        }
        let (scope_level, scope_node) = access_scope
            .map(|scope| (Some(scope.level), Some(scope.node_id)))
            .unwrap_or((None, None));
        let claims = AccessClaims {
            iss: self.settings.issuer.clone(),
            aud: self.settings.audience.clone(),
            sub: input.subject.to_string(),
            iat: issued_at,
            nbf: issued_at,
            exp: expires_at,
            jti: Uuid::new_v4().to_string(),
            org: input.org_id.to_string(),
            roles: input.roles,
            branches: input
                .branches
                .into_iter()
                .map(|branch| branch.to_string())
                .collect(),
            platform: input.platform,
            view_as: input.view_as,
            read_only: input.read_only,
            name: input.display_name,
            scope_level,
            scope_node,
            group_roles,
            tenant_context,
            group_context_id,
            actor_home_org,
            actor_session,
            session_family_id,
            feature_grants: input.feature_grants,
            authz_subject_version: input.authz_subject_version,
            authz_policy_version: input.authz_policy_version,
            session_generation: input.session_generation,
            alg: "ES256".to_owned(),
        };

        Ok(encode(
            &Header::new(Algorithm::ES256),
            &claims,
            &self.encoding_key,
        )?)
    }

    pub fn verify_access_token(&self, token: &str) -> Result<AccessClaims, AuthError> {
        verify_access_token(token, &self.decoding_key, &self.settings)
    }
}

impl JwtVerifier {
    pub fn from_es256_public_pem(
        settings: JwtSettings,
        public_key_pem: &[u8],
    ) -> Result<Self, AuthError> {
        Ok(Self {
            settings,
            decoding_key: DecodingKey::from_ec_pem(public_key_pem)?,
        })
    }

    pub fn verify_access_token(&self, token: &str) -> Result<AccessClaims, AuthError> {
        verify_access_token(token, &self.decoding_key, &self.settings)
    }
}

fn verify_access_token(
    token: &str,
    decoding_key: &DecodingKey,
    settings: &JwtSettings,
) -> Result<AccessClaims, AuthError> {
    let mut validation = Validation::new(Algorithm::ES256);
    validation.leeway = 0;
    validation.set_issuer(&[settings.issuer.as_str()]);
    validation.set_audience(&[settings.audience.as_str()]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    let token = decode::<AccessClaims>(token, decoding_key, &validation)?;
    token.claims.validate_expiry()?;
    // Fail closed on a malformed tenant claim: the `org` claim arms
    // `app.current_org` for RLS, so a token whose `org` is not a valid UUID must
    // never be accepted — it could otherwise reach the DB with an unparseable or
    // empty tenant context.
    OrgId::from_str(&token.claims.org).map_err(|_| {
        AuthError::InvalidStoredData("token org claim is not a valid uuid".to_owned())
    })?;
    token.claims.access_scope()?;
    if token.claims.view_as && !token.claims.group_roles.is_empty() {
        return Err(AuthError::InvalidStoredData(
            "view-as tokens cannot carry group roles".to_owned(),
        ));
    }
    validate_group_roles(&token.claims.group_roles)?;
    validate_tenant_context(
        token.claims.tenant_context,
        token.claims.group_context_id.as_deref(),
        token.claims.actor_home_org.as_deref(),
        &token.claims.roles,
        &token.claims.group_roles,
        token.claims.platform,
        token.claims.view_as,
        token.claims.read_only,
    )?;
    Ok(token.claims)
}
