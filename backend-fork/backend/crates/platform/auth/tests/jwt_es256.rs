#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_kernel_core::{AccessScope, AccessScopeLevel, BranchId, OrgId, ScopeNodeId, UserId};
use console_platform_auth::{
    AccessClaims, AccessTokenInput, JwtIssuer, JwtSettings, TenantAccessContext,
};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use time::{Duration, OffsetDateTime};

fn es256_material() -> (JwtIssuer, String, String) {
    let signing_key = SigningKey::random(&mut OsRng);
    let private_pem = signing_key.to_pkcs8_pem(LineEnding::LF).unwrap();
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();

    let issuer = JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: "console-platform-auth".to_owned(),
            audience: "console-api".to_owned(),
            access_token_ttl: Duration::minutes(15),
        },
        private_pem.as_bytes(),
        public_pem.as_bytes(),
    )
    .unwrap();

    (issuer, private_pem.to_string(), public_pem)
}

fn es256_issuer() -> JwtIssuer {
    es256_material().0
}

#[test]
fn es256_access_token_round_trips_with_expected_claims() {
    let issuer = es256_issuer();

    let user_id = UserId::new();
    let branch_id = BranchId::new();
    let now = OffsetDateTime::now_utc();

    let token = issuer
        .issue_access_token(AccessTokenInput {
            subject: user_id,
            org_id: OrgId::knl(),
            roles: vec!["MECHANIC".to_owned()],
            branches: vec![branch_id],
            platform: false,
            view_as: false,
            read_only: false,
            display_name: None,
            feature_grants: Vec::new(),
            authz_subject_version: 0,
            authz_policy_version: 0,
            session_generation: 0,
            issued_at: now,
        })
        .unwrap();

    let claims = issuer.verify_access_token(&token).unwrap();

    assert_eq!(claims.sub, user_id.to_string());
    assert_eq!(claims.iss, "console-platform-auth");
    assert_eq!(claims.aud, "console-api");
    assert_eq!(claims.roles, vec!["MECHANIC"]);
    assert_eq!(claims.branches, vec![branch_id.to_string()]);
    assert_eq!(claims.iat, now.unix_timestamp());
    assert_eq!(claims.nbf, now.unix_timestamp());
    assert_eq!(claims.exp, (now + Duration::minutes(15)).unix_timestamp());
    assert_eq!(claims.alg, "ES256");
    // No display name supplied -> the optional `name` claim is absent.
    assert_eq!(claims.name, None);
    assert_eq!(
        claims.access_scope().unwrap(),
        AccessScope::legacy_org(OrgId::knl())
    );
    assert!(claims.group_roles.is_empty());
    assert!(claims.feature_grants.is_empty());
}

#[test]
fn es256_access_token_carries_feature_grant_ui_hints() {
    let issuer = es256_issuer();

    let token = issuer
        .issue_access_token(AccessTokenInput {
            subject: UserId::new(),
            org_id: OrgId::knl(),
            roles: vec!["MEMBER".to_owned()],
            branches: vec![],
            platform: false,
            view_as: false,
            read_only: false,
            display_name: None,
            feature_grants: vec!["mail_use".to_owned(), "role_manage".to_owned()],
            authz_subject_version: 0,
            authz_policy_version: 0,
            session_generation: 0,
            issued_at: OffsetDateTime::now_utc(),
        })
        .unwrap();

    let claims = issuer.verify_access_token(&token).unwrap();
    assert_eq!(claims.feature_grants, vec!["mail_use", "role_manage"]);
}

#[test]
fn es256_rejects_actor_home_org_on_non_delegated_tokens() {
    let (issuer, private_pem, _) = es256_material();
    let token = issuer
        .issue_access_token(AccessTokenInput {
            subject: UserId::new(),
            org_id: OrgId::knl(),
            roles: vec!["ADMIN".to_owned()],
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
        .unwrap();
    let mut claims = issuer.verify_access_token(&token).unwrap();
    claims.actor_home_org = Some(OrgId::new().to_string());
    let forged = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256),
        &claims,
        &jsonwebtoken::EncodingKey::from_ec_pem(private_pem.as_bytes()).unwrap(),
    )
    .unwrap();

    let err = issuer.verify_access_token(&forged).unwrap_err();
    assert!(
        err.to_string()
            .contains("actor_home_org requires group-admin tenant context")
    );
}

#[test]
fn es256_access_token_carries_optional_display_name_claim() {
    let issuer = es256_issuer();

    let token = issuer
        .issue_access_token(AccessTokenInput {
            subject: UserId::new(),
            org_id: OrgId::knl(),
            roles: vec!["ADMIN".to_owned()],
            branches: vec![],
            platform: false,
            view_as: false,
            read_only: false,
            display_name: Some("홍길동".to_owned()),
            feature_grants: Vec::new(),
            authz_subject_version: 0,
            authz_policy_version: 0,
            session_generation: 0,
            issued_at: OffsetDateTime::now_utc(),
        })
        .unwrap();

    // The display name round-trips in the `name` claim (display only; the
    // verifier never authorizes off it). The round-trip through encode/verify
    // proves the claim is serialized into and parsed back out of the JWT, which
    // is exactly what the web client decodes for the topbar identity.
    let claims = issuer.verify_access_token(&token).unwrap();
    assert_eq!(claims.name.as_deref(), Some("홍길동"));
}

#[test]
fn es256_access_token_can_carry_group_roles_without_widening_scope() {
    let issuer = es256_issuer();
    let org_id = OrgId::knl();

    let token = issuer
        .issue_access_token_with_group_roles(
            AccessTokenInput {
                subject: UserId::new(),
                org_id,
                roles: vec!["MEMBER".to_owned()],
                branches: vec![],
                platform: false,
                view_as: false,
                read_only: false,
                display_name: None,
                feature_grants: Vec::new(),
                authz_subject_version: 0,
                authz_policy_version: 0,
                session_generation: 0,
                issued_at: OffsetDateTime::now_utc(),
            },
            vec!["GROUP_ADMIN".to_owned()],
        )
        .unwrap();

    let claims = issuer.verify_access_token(&token).unwrap();
    assert_eq!(claims.group_roles, vec!["GROUP_ADMIN"]);
    assert_eq!(
        claims.access_scope().unwrap(),
        AccessScope::legacy_org(org_id),
        "group-role claims are UI hints; backend endpoints re-resolve live grants",
    );
}

#[test]
fn group_admin_tenant_context_token_is_bounded_and_distinct_from_super_admin() {
    let issuer = es256_issuer();
    let group_id = uuid::Uuid::new_v4();
    let target_org = OrgId::new();
    let actor_home_org = OrgId::knl();

    let token = issuer
        .issue_group_admin_tenant_context_access_token(
            AccessTokenInput {
                subject: UserId::new(),
                org_id: target_org,
                roles: vec!["ADMIN".to_owned()],
                branches: vec![],
                platform: false,
                view_as: false,
                read_only: false,
                display_name: None,
                feature_grants: Vec::new(),
                authz_subject_version: 0,
                authz_policy_version: 0,
                session_generation: 0,
                issued_at: OffsetDateTime::now_utc(),
            },
            group_id,
            console_platform_auth::ActorSession {
                family_id: uuid::Uuid::new_v4(),
                expires_at: (OffsetDateTime::now_utc() + Duration::minutes(15)).unix_timestamp(),
                home_org: actor_home_org,
                subject_version: 0,
                session_generation: 0,
            },
            Duration::minutes(15),
        )
        .unwrap();

    let claims = issuer.verify_access_token(&token).unwrap();
    assert_eq!(claims.roles, vec!["ADMIN"]);
    assert!(!claims.roles.iter().any(|role| role == "SUPER_ADMIN"));
    assert_eq!(claims.group_roles, vec!["GROUP_ADMIN"]);
    assert_eq!(claims.tenant_context, Some(TenantAccessContext::GroupAdmin));
    assert_eq!(claims.group_context_id, Some(group_id.to_string()));
    assert_eq!(claims.actor_home_org, Some(actor_home_org.to_string()));
    assert_eq!(claims.org, target_org.to_string());
    assert_ne!(claims.actor_home_org.as_deref(), Some(claims.org.as_str()));
}

#[test]
fn group_admin_tenant_context_token_rejects_super_admin_role() {
    let issuer = es256_issuer();

    let err = issuer
        .issue_group_admin_tenant_context_access_token(
            AccessTokenInput {
                subject: UserId::new(),
                org_id: OrgId::knl(),
                roles: vec!["SUPER_ADMIN".to_owned()],
                branches: vec![],
                platform: false,
                view_as: false,
                read_only: false,
                display_name: None,
                feature_grants: Vec::new(),
                authz_subject_version: 0,
                authz_policy_version: 0,
                session_generation: 0,
                issued_at: OffsetDateTime::now_utc(),
            },
            uuid::Uuid::new_v4(),
            console_platform_auth::ActorSession {
                family_id: uuid::Uuid::new_v4(),
                expires_at: (OffsetDateTime::now_utc() + Duration::minutes(15)).unix_timestamp(),
                home_org: OrgId::knl(),
                subject_version: 0,
                session_generation: 0,
            },
            Duration::minutes(15),
        )
        .unwrap_err();

    assert!(err.to_string().contains("cannot carry SUPER_ADMIN"));
}

#[test]
fn es256_access_token_round_trips_explicit_access_scope_claims() {
    let issuer = es256_issuer();

    let scope = AccessScope::new(
        AccessScopeLevel::Group,
        ScopeNodeId::from_uuid(uuid::Uuid::new_v4()),
    );
    let token = issuer
        .issue_scoped_access_token(
            AccessTokenInput {
                subject: UserId::new(),
                org_id: OrgId::knl(),
                roles: vec!["ADMIN".to_owned()],
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
            },
            scope,
            vec!["GROUP_ADMIN".to_owned()],
        )
        .unwrap();

    let claims = issuer.verify_access_token(&token).unwrap();
    assert_eq!(claims.scope_level, Some(AccessScopeLevel::Group));
    assert_eq!(claims.scope_node, Some(scope.node_id));
    assert_eq!(claims.access_scope().unwrap(), scope);
    assert_eq!(claims.group_roles, vec!["GROUP_ADMIN"]);
}

#[test]
fn es256_scoped_token_rejects_unknown_group_role_on_issue() {
    let issuer = es256_issuer();

    let err = issuer
        .issue_scoped_access_token(
            AccessTokenInput {
                subject: UserId::new(),
                org_id: OrgId::knl(),
                roles: vec!["ADMIN".to_owned()],
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
            },
            AccessScope::legacy_org(OrgId::knl()),
            vec!["group_admin".to_owned()],
        )
        .unwrap_err();

    assert!(err.to_string().contains("unknown group role code"));
}

#[test]
fn es256_scoped_token_rejects_unknown_group_role_on_verify() {
    let (issuer, private_pem, _) = es256_material();
    let now = OffsetDateTime::now_utc();
    let claims = AccessClaims {
        iss: "console-platform-auth".to_owned(),
        aud: "console-api".to_owned(),
        sub: UserId::new().to_string(),
        iat: now.unix_timestamp(),
        nbf: now.unix_timestamp(),
        exp: (now + Duration::minutes(15)).unix_timestamp(),
        jti: uuid::Uuid::new_v4().to_string(),
        org: OrgId::knl().to_string(),
        roles: vec!["ADMIN".to_owned()],
        branches: Vec::new(),
        platform: false,
        view_as: false,
        read_only: false,
        name: None,
        scope_level: Some(AccessScopeLevel::Group),
        scope_node: Some(ScopeNodeId::from_uuid(uuid::Uuid::new_v4())),
        group_roles: vec!["GROUP_OWNER".to_owned()],
        tenant_context: None,
        group_context_id: None,
        actor_home_org: None,
        actor_session: None,
        session_family_id: None,
        feature_grants: Vec::new(),
        authz_subject_version: 0,
        authz_policy_version: 0,
        session_generation: 0,
        alg: "ES256".to_owned(),
    };
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256),
        &claims,
        &jsonwebtoken::EncodingKey::from_ec_pem(private_pem.as_bytes()).unwrap(),
    )
    .unwrap();

    let err = issuer.verify_access_token(&token).unwrap_err();
    assert!(err.to_string().contains("unknown group role code"));
}

#[test]
fn es256_view_as_token_refuses_group_roles() {
    let issuer = es256_issuer();

    let err = issuer
        .issue_scoped_access_token(
            AccessTokenInput {
                subject: UserId::new(),
                org_id: OrgId::knl(),
                roles: vec!["ADMIN".to_owned()],
                branches: Vec::new(),
                platform: false,
                view_as: true,
                read_only: true,
                display_name: None,
                feature_grants: Vec::new(),
                authz_subject_version: 0,
                authz_policy_version: 0,
                session_generation: 0,
                issued_at: OffsetDateTime::now_utc(),
            },
            AccessScope::legacy_org(OrgId::knl()),
            vec!["GROUP_ADMIN".to_owned()],
        )
        .unwrap_err();

    assert!(
        err.to_string()
            .contains("view-as tokens cannot carry group roles")
    );
}

#[test]
fn access_scope_claims_must_be_a_complete_pair() {
    let claims = AccessClaims {
        iss: "console-platform-auth".to_owned(),
        aud: "console-api".to_owned(),
        sub: UserId::new().to_string(),
        iat: 1,
        nbf: 1,
        exp: 2,
        jti: uuid::Uuid::new_v4().to_string(),
        org: OrgId::knl().to_string(),
        roles: Vec::new(),
        branches: Vec::new(),
        platform: false,
        view_as: false,
        read_only: false,
        name: None,
        scope_level: Some(AccessScopeLevel::Org),
        scope_node: None,
        group_roles: Vec::new(),
        tenant_context: None,
        group_context_id: None,
        actor_home_org: None,
        actor_session: None,
        session_family_id: None,
        feature_grants: Vec::new(),
        authz_subject_version: 0,
        authz_policy_version: 0,
        session_generation: 0,
        alg: "ES256".to_owned(),
    };

    let err = claims.access_scope().unwrap_err();
    assert!(
        err.to_string()
            .contains("scope claims must include both scope_level and scope_node")
    );
}

// Cedar/PBAC activation (ADR-0021): the access token carries a subject
// authorization freshness snapshot. SLICE-2 sources it; no decision consults it.
#[test]
fn es256_access_token_stamps_subject_authz_freshness() {
    let issuer = es256_issuer();
    let now = OffsetDateTime::now_utc();

    let token = issuer
        .issue_access_token(AccessTokenInput {
            subject: UserId::new(),
            org_id: OrgId::knl(),
            roles: vec!["SUPER_ADMIN".to_owned()],
            branches: Vec::new(),
            platform: false,
            view_as: false,
            read_only: false,
            display_name: None,
            feature_grants: Vec::new(),
            authz_subject_version: 7,
            authz_policy_version: 3,
            session_generation: 5,
            issued_at: now,
        })
        .unwrap();

    let claims = issuer.verify_access_token(&token).unwrap();
    assert_eq!(claims.authz_subject_version, 7);
    assert_eq!(claims.authz_policy_version, 3);
    assert_eq!(claims.session_generation, 5);
}

// A token minted before the freshness claims existed simply omits them on the
// wire. #[serde(default)] must accept it and default all three to 0, so old
// tokens keep their exact meaning on every live path (a 0-carrying token is only
// ever denied on the still-unreachable Cedar path).
#[test]
fn legacy_access_token_without_freshness_claims_defaults_to_zero() {
    let (issuer, private_pem, _) = es256_material();
    let now = OffsetDateTime::now_utc();

    let legacy = serde_json::json!({
        "iss": "console-platform-auth",
        "aud": "console-api",
        "sub": UserId::new().to_string(),
        "iat": now.unix_timestamp(),
        "nbf": now.unix_timestamp(),
        "exp": (now + Duration::minutes(15)).unix_timestamp(),
        "jti": uuid::Uuid::new_v4().to_string(),
        "org": OrgId::knl().to_string(),
        "roles": ["MECHANIC"],
        "branches": [],
        "alg": "ES256",
    });
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256),
        &legacy,
        &jsonwebtoken::EncodingKey::from_ec_pem(private_pem.as_bytes()).unwrap(),
    )
    .unwrap();

    let claims = issuer.verify_access_token(&token).unwrap();
    assert_eq!(claims.authz_subject_version, 0);
    assert_eq!(claims.authz_policy_version, 0);
    assert_eq!(claims.session_generation, 0);
}
