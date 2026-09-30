#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Cedar/PBAC subject-freshness mint-path hardening tests.
//!
//! The non-login access-token mint paths (platform view-as / tenant-context and
//! the group-admin tenant-context) used to stamp a hardcoded ZERO freshness,
//! which a promoted Cedar guard would deny as `MissingSubjectFreshness` /
//! `StaleSubject`. They now source REAL freshness for the token's own
//! `(org, user)` via `console_platform_db::read_subject_authz_freshness`.
//!
//! These live under `backend/app/tests/` (not the `rest/`-path crates) so the
//! `audit-coverage` CI gate does not scan the TEST-ONLY seed helpers as if they
//! were state-changing handlers — the same relocation `cedar_shadow_role_manage`
//! and `m2_real_engine_drive` use.
//!
//! Everything runs as the REAL `console_rt` runtime role (NOSUPERUSER, NOBYPASSRLS)
//! under FORCE RLS — never a BYPASSRLS superuser pool, which would mask a broken
//! read path (rls-verify-as-runtime-role).
//!
//! Coverage:
//!   * the shared freshness read returns the DB-current values under console_rt RLS,
//!     and reads absent rows as the safe 0 baseline;
//!   * the group-admin tenant-context issuer (site 3) carries the sourced
//!     freshness onto the verified token claims (the construction seam the two
//!     platform mints share — those two get end-to-end HTTP coverage in
//!     `platform-rest/tests/view_as.rs`);
//!   * a token minted with the sourced snapshot SATISFIES the guard-time
//!     requirement (neither missing nor stale), and after a version bump an older
//!     carried snapshot trips `StaleSubject` — proving the freshness comparison is
//!     live.

use std::collections::BTreeSet;

use console_kernel_core::{BranchScope, OrgId, UserId};
use console_platform_auth::{AccessTokenInput, JwtIssuer, JwtSettings, JwtVerifier};
use console_platform_authz::cedar_pbac::engine;
use console_platform_authz::{
    Action, AuthorizationRequest, AuthorizationResource, CedarEvaluation, CoexistenceMapEntry,
    CompiledBundleCacheKey, DecisionEffect, DecisionReason, DualEngineMode, Feature, Principal,
    RlsScopeProof, Role, SubjectFreshness, SubjectFreshnessRequirement,
    evaluate_cedar_pbac_boundary,
};
use console_platform_db::read_subject_authz_freshness;
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const ORG_A: Uuid = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);
const ORG_B: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);
const RESOURCE_TYPE: &str = "identity.policy_role";
const DOMAIN: &str = "identity.policy";
const TEST_ISSUER: &str = "console-platform-auth";
const TEST_AUDIENCE: &str = "console-api";

/// A pool whose every connection runs `SET ROLE console_rt` — the production runtime
/// role under FORCE RLS, never the BYPASSRLS superuser the default `#[sqlx::test]`
/// pool connects as.
async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

async fn seed_user(owner_pool: &PgPool, org: Uuid, role: &str) -> UserId {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id, is_active) VALUES ($1, $2, $3, true) RETURNING id",
    )
    .bind(format!("User {}", Uuid::new_v4()))
    .bind(vec![role.to_owned()])
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    UserId::from_uuid(user_id)
}

/// Seed the per-org policy revision (owner pool, RLS off).
async fn set_policy_version(owner_pool: &PgPool, org: Uuid, version: i64) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO policy_versions (org_id, version, updated_at)
        VALUES ($1, $2, now())
        ON CONFLICT (org_id) DO UPDATE SET version = EXCLUDED.version, updated_at = now()
        "#,
    )
    .bind(org)
    .bind(version)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// Seed a subject's `(version, session_generation)` counters (owner pool, RLS
/// off). The FK requires the `(user_id, org_id)` pair to exist in `users`.
async fn set_subject_versions(
    owner_pool: &PgPool,
    org: Uuid,
    user: UserId,
    version: i64,
    session_generation: i64,
) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO subject_authz_versions (org_id, user_id, version, session_generation, updated_at)
        VALUES ($1, $2, $3, $4, now())
        ON CONFLICT (org_id, user_id) DO UPDATE
        SET version = EXCLUDED.version, session_generation = EXCLUDED.session_generation
        "#,
    )
    .bind(org)
    .bind(*user.as_uuid())
    .bind(version)
    .bind(session_generation)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

struct Keys {
    private_pem: String,
    public_pem: String,
}

fn keypair() -> Keys {
    let signing_key = SigningKey::random(&mut OsRng);
    Keys {
        private_pem: signing_key
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap()
            .to_string(),
        public_pem: signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap(),
    }
}

fn jwt_settings() -> JwtSettings {
    JwtSettings {
        issuer: TEST_ISSUER.to_owned(),
        audience: TEST_AUDIENCE.to_owned(),
        access_token_ttl: Duration::minutes(15),
    }
}

fn shadow_entry(bundle_key: CompiledBundleCacheKey) -> CoexistenceMapEntry {
    CoexistenceMapEntry::new(
        format!("{DOMAIN}.role_manage"),
        DOMAIN,
        Feature::RoleManage,
        RESOURCE_TYPE,
        DualEngineMode::CedarShadowLegacyEnforce,
        Some(bundle_key),
    )
}

/// Build the audit-only shadow boundary request exactly as the live guard
/// (`authorize_org_manage_observed`) does: same domain/resource, the token's
/// carried freshness, the DB-current required freshness, and the runtime-role RLS
/// proof.
fn guard_request(
    principal: &Principal,
    org: OrgId,
    carried: SubjectFreshness,
    required: SubjectFreshnessRequirement,
) -> AuthorizationRequest {
    AuthorizationRequest::new(
        principal.clone(),
        Action::new(Feature::RoleManage),
        AuthorizationResource::org_wide(org, RESOURCE_TYPE),
    )
    .with_policy_domain(DOMAIN)
    .with_subject_freshness(carried)
    .requiring_freshness(required)
    .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(org))
}

// ===========================================================================
// The shared freshness read returns the DB-current values under console_rt RLS, and
// reads absent rows as the safe 0 baseline.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn read_returns_db_current_freshness_as_runtime_role(owner_pool: PgPool) {
    seed_org(&owner_pool, ORG_A, "A").await;
    seed_org(&owner_pool, ORG_B, "B").await;
    let user_a = seed_user(&owner_pool, ORG_A, "SUPER_ADMIN").await;
    let user_b = seed_user(&owner_pool, ORG_B, "SUPER_ADMIN").await;
    set_policy_version(&owner_pool, ORG_A, 5).await;
    set_subject_versions(&owner_pool, ORG_A, user_a, 4, 3).await;

    let rt_pool = runtime_role_pool(&owner_pool).await;

    // Org A has real rows → non-zero DB-current, read under console_rt RLS.
    let a = read_subject_authz_freshness(&rt_pool, OrgId::from_uuid(ORG_A), user_a)
        .await
        .unwrap();
    assert_eq!(
        a.policy_version, 5,
        "must read the real per-org policy version"
    );
    assert_eq!(a.subject_version, 4, "must read the real subject version");
    assert_eq!(
        a.session_generation, 3,
        "must read the real session generation"
    );

    // Org B has NO policy_versions/subject rows → absent 0 baseline (not an error,
    // not a spurious non-zero).
    let b = read_subject_authz_freshness(&rt_pool, OrgId::from_uuid(ORG_B), user_b)
        .await
        .unwrap();
    assert_eq!(
        (b.policy_version, b.subject_version, b.session_generation),
        (0, 0, 0),
        "absent rows read as the safe 0 baseline"
    );

    // Cross-org isolation: reading org A's subject under org B's arming must NOT
    // leak A's subject counters (RLS scopes the subject read to the armed org).
    let leak = read_subject_authz_freshness(&rt_pool, OrgId::from_uuid(ORG_B), user_a)
        .await
        .unwrap();
    assert_eq!(
        leak.subject_version, 0,
        "org A's subject row must be invisible under org B's armed GUC"
    );
}

// ===========================================================================
// Supplementary issuer-seam check: the group-admin tenant-context ISSUER carries
// all THREE freshness fields (policy, subject, session) onto the verified claims
// without transposition. The REAL site-3 handler wiring is proven end-to-end in
// `auth-rest/tests/group_admin_tenant_context.rs` (that cross-org actor has no
// subject row, so subject/session are 0 there); this in-org seam fills the
// (subject, session) non-zero gap the handler test cannot exercise.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn group_admin_issuer_seam_carries_all_three_fields(owner_pool: PgPool) {
    seed_org(&owner_pool, ORG_A, "A").await;
    // An in-org subject with a real subject row lets this seam exercise the FULL
    // (policy, subject, session) non-zero path. In production the group-admin
    // actor is typically cross-org, so subject/session would be the absent 0
    // baseline; that shape is covered by the platform-mint HTTP tests.
    let actor = seed_user(&owner_pool, ORG_A, "ADMIN").await;
    set_policy_version(&owner_pool, ORG_A, 5).await;
    set_subject_versions(&owner_pool, ORG_A, actor, 4, 3).await;

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let freshness = read_subject_authz_freshness(&rt_pool, OrgId::from_uuid(ORG_A), actor)
        .await
        .unwrap();

    let keys = keypair();
    let issuer = JwtIssuer::from_es256_pem(
        jwt_settings(),
        keys.private_pem.as_bytes(),
        keys.public_pem.as_bytes(),
    )
    .unwrap();
    let verifier =
        JwtVerifier::from_es256_public_pem(jwt_settings(), keys.public_pem.as_bytes()).unwrap();

    // Mint exactly as `start_group_admin_tenant_context` does: same input shape,
    // the sourced freshness, the group-admin issuer.
    let token = issuer
        .issue_group_admin_tenant_context_access_token(
            AccessTokenInput {
                subject: actor,
                org_id: OrgId::from_uuid(ORG_A),
                roles: vec!["ADMIN".to_owned()],
                branches: Vec::new(),
                platform: false,
                view_as: false,
                read_only: false,
                display_name: None,
                feature_grants: Vec::new(),
                authz_subject_version: freshness.subject_version,
                authz_policy_version: freshness.policy_version,
                session_generation: freshness.session_generation,
                issued_at: OffsetDateTime::now_utc(),
            },
            Uuid::new_v4(),
            console_platform_auth::ActorSession {
                family_id: console_platform_auth::RefreshTokenStore
                    .issue_family(
                        &rt_pool,
                        *actor.as_uuid(),
                        OrgId::from_uuid(ORG_A),
                        OffsetDateTime::now_utc(),
                        Duration::hours(1),
                    )
                    .await
                    .unwrap()
                    .family_id,
                expires_at: (OffsetDateTime::now_utc() + Duration::minutes(15)).unix_timestamp(),
                home_org: OrgId::from_uuid(ORG_A),
                subject_version: 0,
                session_generation: 0,
            },
            Duration::minutes(15),
        )
        .unwrap();

    let claims = verifier.verify_access_token(&token).unwrap();
    assert_eq!(
        claims.actor_home_org,
        Some(OrgId::from_uuid(ORG_A).to_string())
    );
    assert_eq!(
        claims.authz_policy_version, 5,
        "group-admin token must carry the real policy_version, not 0"
    );
    assert_eq!(
        claims.authz_subject_version, 4,
        "group-admin token must carry the real subject_version, not 0"
    );
    assert_eq!(
        claims.session_generation, 3,
        "group-admin token must carry the real session_generation, not 0"
    );
}

// ===========================================================================
// A token minted with the sourced snapshot SATISFIES the guard-time requirement
// (neither missing nor stale); after a version bump an older carried snapshot
// trips `StaleSubject`. Proves the freshness comparison is live and that a fresh
// legit operator token keeps the shadow lane silent.
// ===========================================================================
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn fresh_token_satisfies_guard_and_stale_after_bump(owner_pool: PgPool) {
    seed_org(&owner_pool, ORG_A, "A").await;
    let user = seed_user(&owner_pool, ORG_A, "SUPER_ADMIN").await;
    set_policy_version(&owner_pool, ORG_A, 5).await;
    set_subject_versions(&owner_pool, ORG_A, user, 4, 3).await;
    let org = OrgId::from_uuid(ORG_A);

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let principal = Principal::new(
        user,
        org,
        BTreeSet::from([Role::SuperAdmin]),
        BranchScope::All,
    );

    // Mint-time snapshot for (org, user) — what the 3 mint sites now stamp.
    let minted = read_subject_authz_freshness(&rt_pool, org, user)
        .await
        .unwrap();
    let carried = SubjectFreshness {
        policy_version: minted.policy_version,
        subject_version: minted.subject_version,
        session_generation: minted.session_generation,
        step_up_generation: None,
    };

    // Guard-time DB-current requirement, read the same way the live guard reads it.
    let guard_now = read_subject_authz_freshness(&rt_pool, org, user)
        .await
        .unwrap();
    let required = SubjectFreshnessRequirement {
        min_policy_version: guard_now.policy_version,
        min_subject_version: guard_now.subject_version,
        min_session_generation: guard_now.session_generation,
        required_step_up_generation: None,
    };

    let bundle = engine::compile_bundle(org, carried.policy_version).unwrap();
    let entry = shadow_entry(bundle.key.clone());

    // (1) A freshly-minted legit SUPER_ADMIN token: the boundary passes the
    // freshness preconditions (no MissingSubjectFreshness, no StaleSubject) and
    // the shadow observation follows the legacy ALLOW.
    let fresh = guard_request(&principal, org, carried, required);
    let decision = evaluate_cedar_pbac_boundary(
        &fresh,
        Some(&entry),
        CedarEvaluation::Allow {
            bundle_key: bundle.key.clone(),
        },
    );
    assert_ne!(
        decision.reason,
        DecisionReason::MissingSubjectFreshness,
        "a fresh token with real material must not be MissingSubjectFreshness"
    );
    assert_ne!(
        decision.reason,
        DecisionReason::StaleSubject,
        "carried == DB-current must satisfy the freshness requirement"
    );
    assert_eq!(
        decision.effect,
        DecisionEffect::Allow,
        "a fresh SUPER_ADMIN token clears freshness and follows the legacy allow"
    );

    // (2) Bump the per-org policy version: the DB-current requirement advances past
    // the token's carried snapshot, so the SAME token is now StaleSubject (deny).
    set_policy_version(&owner_pool, ORG_A, 6).await;
    let bumped = read_subject_authz_freshness(&rt_pool, org, user)
        .await
        .unwrap();
    assert_eq!(
        bumped.policy_version, 6,
        "the bump must be observed under console_rt"
    );
    let required_after = SubjectFreshnessRequirement {
        min_policy_version: bumped.policy_version,
        min_subject_version: bumped.subject_version,
        min_session_generation: bumped.session_generation,
        required_step_up_generation: None,
    };
    let stale = guard_request(&principal, org, carried, required_after);
    let decision = evaluate_cedar_pbac_boundary(
        &stale,
        Some(&entry),
        CedarEvaluation::Allow {
            bundle_key: bundle.key.clone(),
        },
    );
    assert_eq!(
        decision.reason,
        DecisionReason::StaleSubject,
        "an older carried snapshot must trip StaleSubject after a bump"
    );
    assert_eq!(
        decision.effect,
        DecisionEffect::Deny,
        "StaleSubject is a boundary deny (audit-only in shadow)"
    );
}
