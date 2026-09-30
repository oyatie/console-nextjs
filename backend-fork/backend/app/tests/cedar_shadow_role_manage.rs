#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Cedar/PBAC role_manage shadow-wiring DB tests (activation slice 4).
//!
//! These live under `backend/app/tests/` rather than inline in `identity/rest`
//! because the `audit-coverage` CI gate statically scans any file on a `rest/`
//! path for `sqlx::query*` + INSERT/UPDATE/DELETE and (not honoring `#[cfg(test)]`)
//! would flag the TEST-ONLY seed helpers as state-changing handlers missing
//! `with_audit`. The `app` crate path has no `rest`/`application`/`worker`
//! component, so the gate does not scan it — the same relocation M2 used
//! (`m2_real_engine_drive.rs`). The pure, non-sqlx safety unit test stays inline.
//!
//! The load-bearing property (ADR-0021 HIGH finding): the legacy authorization
//! result is the SOLE enforcer; the Cedar shadow lane can NEVER change a live
//! outcome. These end-to-end tests exercise the real `authorize_org_manage_observed`
//! wrapper with the dark default, flag-on observation, and — critically — as the
//! genuine `console_rt` runtime role under FORCE RLS, never a BYPASSRLS superuser.

use std::collections::BTreeSet;

use console_identity_adapter_postgres::PgOrgStore;
use console_identity_rest::{
    CEDAR_PBAC_SHADOW_AUDIT_ACTION, CEDAR_PBAC_SHADOW_ROLE_MANAGE_FLAG, IdentityRestState,
    authorize_org_manage_observed,
};
use console_kernel_core::{BranchScope, OrgId, UserId};
use console_platform_authz::cedar_pbac::engine;
use console_platform_authz::{
    Action, AuthorizationRequest, AuthorizationResource, CedarEvaluation, CoexistenceMapEntry,
    CompiledBundleCacheKey, DecisionEffect, DecisionReason, DualEngineMode, Feature, Principal,
    RlsScopeProof, Role, SubjectFreshness, SubjectFreshnessRequirement,
    evaluate_cedar_pbac_boundary,
};
use console_platform_request_context::CURRENT_ORG;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

const RESOURCE_TYPE: &str = "identity.policy_role";
const DOMAIN: &str = "identity.policy";
const ORG_B: Uuid = Uuid::from_u128(0x3333_3333_3333_3333_3333_3333_3333_3333);

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

/// A pool whose every connection runs `SET ROLE console_rt`, so statements execute
/// as the production runtime role (NOSUPERUSER, NOBYPASSRLS) under FORCE RLS —
/// never the BYPASSRLS superuser the default `#[sqlx::test]` pool connects as.
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

/// Enable the shadow dark switch for one tenant (TEST-ONLY: production ships
/// zero enabled rows). Owner insert with row_security off.
async fn enable_shadow_flag(owner_pool: &PgPool, org: Uuid) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO org_runtime_flags (org_id, flag_key, enabled) VALUES ($1, $2, true)")
        .bind(org)
        .bind(CEDAR_PBAC_SHADOW_ROLE_MANAGE_FLAG)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

/// Count persisted shadow audit rows for a tenant (owner read, row_security
/// off), so the assertion is not itself subject to RLS.
async fn count_shadow_audit_rows(owner_pool: &PgPool, org: Uuid) -> i64 {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = $1 AND org_id = $2")
            .bind(CEDAR_PBAC_SHADOW_AUDIT_ACTION)
            .bind(org)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    count
}

async fn shadow_audit_effect(owner_pool: &PgPool, org: Uuid) -> Option<String> {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let after: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT after_snap FROM audit_events WHERE action = $1 AND org_id = $2 LIMIT 1",
    )
    .bind(CEDAR_PBAC_SHADOW_AUDIT_ACTION)
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    after.and_then(|value| value["decision"]["effect"].as_str().map(str::to_owned))
}

/// DARK: with no flag row, the shadow lane never runs — zero audit rows — and the
/// enforced decision is the legacy ALLOW for SUPER_ADMIN. (The legacy-equality
/// comparison the inline unit test performs is retained there; here we assert the
/// observed enforcement directly, since `authorize_org_manage` stays private.)
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn shadow_lane_is_dark_when_flag_absent(pool: PgPool) {
    let org_uuid = *OrgId::knl().as_uuid();
    seed_org(&pool, org_uuid, "A").await;
    let user = seed_user(&pool, org_uuid, "SUPER_ADMIN").await;

    // Code-under-test runs as the real console_rt runtime role (NOSUPERUSER,
    // NOBYPASSRLS) under FORCE RLS — never the BYPASSRLS superuser the default
    // `#[sqlx::test]` pool connects as, which would mask a broken read path.
    // Seeding + audit assertions stay on the owner `pool` (row_security off).
    let rt_pool = runtime_role_pool(&pool).await;
    let state = IdentityRestState::new(PgOrgStore::new(rt_pool.clone()), None);
    let principal = Principal::new(
        user,
        OrgId::knl(),
        BTreeSet::from([Role::SuperAdmin]),
        BranchScope::All,
    );

    let observed = CURRENT_ORG
        .scope(
            OrgId::knl(),
            authorize_org_manage_observed(&state, &principal, Feature::RoleManage),
        )
        .await;
    assert!(
        observed.is_ok(),
        "SUPER_ADMIN role_manage must be allowed by legacy under the dark default"
    );
    assert_eq!(
        count_shadow_audit_rows(&pool, org_uuid).await,
        0,
        "dark default must write ZERO shadow audit rows"
    );
}

/// Flag ON, SUPER_ADMIN whom legacy ALLOWS: the shadow boundary denies (a real
/// deny — the default token freshness is stale vs the guard-time requirement)
/// and records it, yet the enforced decision returned by the wrapper is still
/// the legacy ALLOW. Proves a shadow deny cannot flip a live allow.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn shadow_deny_does_not_flip_legacy_allow_with_flag_on(pool: PgPool) {
    let org_uuid = *OrgId::knl().as_uuid();
    seed_org(&pool, org_uuid, "A").await;
    let user = seed_user(&pool, org_uuid, "SUPER_ADMIN").await;
    enable_shadow_flag(&pool, org_uuid).await;

    // Real console_rt: exercises the flag/version READS and the audit WRITE
    // (persist_cedar_shadow_audit via with_audit) under FORCE RLS, not superuser.
    let rt_pool = runtime_role_pool(&pool).await;
    let state = IdentityRestState::new(PgOrgStore::new(rt_pool.clone()), None);
    let principal = Principal::new(
        user,
        OrgId::knl(),
        BTreeSet::from([Role::SuperAdmin]),
        BranchScope::All,
    );

    let observed = CURRENT_ORG
        .scope(
            OrgId::knl(),
            authorize_org_manage_observed(&state, &principal, Feature::RoleManage),
        )
        .await;
    assert!(
        observed.is_ok(),
        "SUPER_ADMIN allow must stand even though the shadow boundary denied"
    );
    assert_eq!(
        count_shadow_audit_rows(&pool, org_uuid).await,
        1,
        "flag ON must record exactly one shadow observation"
    );
    assert_eq!(
        shadow_audit_effect(&pool, org_uuid).await.as_deref(),
        Some("deny"),
        "the audit-only boundary observation denied (stale/missing freshness)"
    );
}

/// Flag ON, non-SUPER_ADMIN whom legacy DENIES: enforced decision stays DENY,
/// and the shadow still records an observation.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn shadow_does_not_grant_when_legacy_denies_with_flag_on(pool: PgPool) {
    let org_uuid = *OrgId::knl().as_uuid();
    seed_org(&pool, org_uuid, "A").await;
    let user = seed_user(&pool, org_uuid, "MECHANIC").await;
    enable_shadow_flag(&pool, org_uuid).await;

    // Real console_rt: exercises the flag/version READS and the audit WRITE
    // (persist_cedar_shadow_audit via with_audit) under FORCE RLS, not superuser.
    let rt_pool = runtime_role_pool(&pool).await;
    let state = IdentityRestState::new(PgOrgStore::new(rt_pool.clone()), None);
    let principal = Principal::new(
        user,
        OrgId::knl(),
        BTreeSet::from([Role::Mechanic]),
        BranchScope::All,
    );

    let observed = CURRENT_ORG
        .scope(
            OrgId::knl(),
            authorize_org_manage_observed(&state, &principal, Feature::RoleManage),
        )
        .await;
    assert!(
        observed.is_err(),
        "MECHANIC role_manage must remain DENIED with the shadow flag on"
    );
    assert_eq!(
        count_shadow_audit_rows(&pool, org_uuid).await,
        1,
        "flag ON must record exactly one shadow observation even on a deny"
    );
}

/// console_rt RLS: the shadow lane's reads (flag, versions) and audit write run as
/// the real runtime role under an armed `app.current_org`; a tenant sees only
/// its own flag row; and a cross-org resource yields the boundary's
/// `RlsBoundaryMismatch` deny (audit-only) rather than reaching Cedar.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn shadow_reads_and_audits_as_runtime_role_and_stay_org_scoped(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = *OrgId::knl().as_uuid();
    seed_org(&owner_pool, org_a, "A").await;
    seed_org(&owner_pool, ORG_B, "B").await;
    let user_a = seed_user(&owner_pool, org_a, "SUPER_ADMIN").await;
    enable_shadow_flag(&owner_pool, org_a).await;
    enable_shadow_flag(&owner_pool, ORG_B).await;

    let state = IdentityRestState::new(PgOrgStore::new(rt_pool.clone()), None);
    let principal = Principal::new(
        user_a,
        OrgId::knl(),
        BTreeSet::from([Role::SuperAdmin]),
        BranchScope::All,
    );

    // (1) The whole lane runs as console_rt under org A's GUC: legacy allow stands
    // and exactly one shadow audit row is written for A (none for B).
    let observed = CURRENT_ORG
        .scope(
            OrgId::knl(),
            authorize_org_manage_observed(&state, &principal, Feature::RoleManage),
        )
        .await;
    assert!(
        observed.is_ok(),
        "SUPER_ADMIN allow must stand as console_rt"
    );
    assert_eq!(
        count_shadow_audit_rows(&owner_pool, org_a).await,
        1,
        "audit write must succeed as console_rt under the armed GUC"
    );
    assert_eq!(
        count_shadow_audit_rows(&owner_pool, ORG_B).await,
        0,
        "no shadow audit row may land under the other tenant"
    );

    // (2) Under org A's GUC, console_rt sees ONLY A's flag row; B's is invisible.
    {
        let mut tx = rt_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org_a.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM org_runtime_flags")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let other: i64 =
            sqlx::query_scalar("SELECT count(*) FROM org_runtime_flags WHERE org_id = $1")
                .bind(ORG_B)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(visible, 1, "org A must see exactly its own flag row");
        assert_eq!(other, 0, "org B's flag row must be invisible under A");
    }

    // (3) A cross-org resource (org B) under org A's subject/proof denies at the
    // boundary with RlsBoundaryMismatch — Cedar is never consulted, and this is
    // an audit-only observation, not an enforcement path.
    let bundle = engine::compile_bundle(OrgId::knl(), 1).unwrap();
    let entry = shadow_entry(bundle.key.clone());
    let cross_org = AuthorizationRequest::new(
        principal.clone(),
        Action::new(Feature::RoleManage),
        AuthorizationResource::org_wide(OrgId::from_uuid(ORG_B), RESOURCE_TYPE),
    )
    .with_policy_domain(DOMAIN)
    .with_subject_freshness(SubjectFreshness {
        policy_version: 1,
        subject_version: 1,
        session_generation: 1,
        step_up_generation: None,
    })
    .requiring_freshness(SubjectFreshnessRequirement {
        min_policy_version: 1,
        min_subject_version: 1,
        min_session_generation: 1,
        required_step_up_generation: None,
    })
    .with_rls_scope_proof(RlsScopeProof::runtime_role_guc(OrgId::knl()));
    let decision = evaluate_cedar_pbac_boundary(
        &cross_org,
        Some(&entry),
        CedarEvaluation::Allow {
            bundle_key: bundle.key.clone(),
        },
    );
    assert_eq!(decision.effect, DecisionEffect::Deny);
    assert_eq!(decision.reason, DecisionReason::RlsBoundaryMismatch);
}
