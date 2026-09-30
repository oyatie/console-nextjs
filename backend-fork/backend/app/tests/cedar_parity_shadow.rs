#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Cedar/PBAC enrollment wave 2 — parity shadow DB tests (console_rt, FORCE RLS).
//!
//! These live under `backend/app/tests/` (not inline in a `rest/` crate) for the
//! same reason as `cedar_shadow_role_manage.rs`: the `audit-coverage` CI gate
//! statically scans `rest/` paths for `sqlx::query*` + INSERT and would flag the
//! TEST-ONLY seed helpers. The `app` test path is not scanned.
//!
//! Load-bearing property: the shadow parity lane is AUDIT-ONLY. `observe_parity`
//! never returns a decision to the caller (legacy already enforced), and here we
//! prove it (1) is dark with the flag absent, (2) records a real Cedar-vs-legacy
//! divergence the report surfaces, (3) records agreements, and (4) stays
//! org-scoped under FORCE RLS as the real `console_rt` runtime role — never a
//! BYPASSRLS superuser.

use std::collections::BTreeSet;

use console_app::cedar_parity::{
    CEDAR_PBAC_PARITY_AUDIT_ACTION, CEDAR_PBAC_SHADOW_OBJECT_RESOLVE_FLAG, OBJECT_RESOLVE_DOMAIN,
    ParityObservation, aggregate, observe_parity,
};
use console_kernel_core::{BranchScope, OrgId, UserId};
use console_platform_authz::{
    AuthorizationResource, DecisionEffect, Feature, Principal, Role, SubjectFreshness,
};
use console_platform_request_context::CURRENT_ORG;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

const ORG_B: Uuid = Uuid::from_u128(0x4444_4444_4444_4444_4444_4444_4444_4444);

async fn disable_row_security(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) {
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut **tx)
        .await
        .unwrap();
}

/// A pool whose every connection runs `SET ROLE console_rt`, so statements execute as
/// the production runtime role (NOSUPERUSER, NOBYPASSRLS) under FORCE RLS.
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
    disable_row_security(&mut tx).await;
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
    disable_row_security(&mut tx).await;
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

/// Seed the DB-current freshness rows (both default to version 1) so the shadow
/// boundary's freshness preconditions PASS and Cedar actually evaluates policy —
/// otherwise every observation would be a preflight freshness deny, not a policy
/// comparison.
async fn seed_freshness(owner_pool: &PgPool, org: Uuid, user: UserId) {
    let mut tx = owner_pool.begin().await.unwrap();
    disable_row_security(&mut tx).await;
    sqlx::query("INSERT INTO policy_versions (org_id) VALUES ($1) ON CONFLICT (org_id) DO NOTHING")
        .bind(org)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO subject_authz_versions (org_id, user_id) VALUES ($1, $2) \
         ON CONFLICT (org_id, user_id) DO NOTHING",
    )
    .bind(org)
    .bind(user.as_uuid())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

async fn enable_flag(owner_pool: &PgPool, org: Uuid, flag: &str) {
    let mut tx = owner_pool.begin().await.unwrap();
    disable_row_security(&mut tx).await;
    sqlx::query("INSERT INTO org_runtime_flags (org_id, flag_key, enabled) VALUES ($1, $2, true)")
        .bind(org)
        .bind(flag)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

/// Read back the persisted parity observations for a tenant (owner read,
/// row_security off — the cross-tenant operator read the report binary performs).
async fn read_parity_observations(owner_pool: &PgPool, org: Uuid) -> Vec<ParityObservation> {
    let mut tx = owner_pool.begin().await.unwrap();
    disable_row_security(&mut tx).await;
    let rows: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT after_snap FROM audit_events WHERE action = $1 AND org_id = $2")
            .bind(CEDAR_PBAC_PARITY_AUDIT_ACTION)
            .bind(org)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    rows.into_iter()
        .map(|v| serde_json::from_value(v).unwrap())
        .collect()
}

/// Fresh principal whose carried snapshot matches the seeded DB-current versions
/// (1/1/1), so the boundary's freshness gate passes and Cedar runs.
fn fresh_principal(user: UserId, role: Role) -> Principal {
    Principal::new(user, OrgId::knl(), BTreeSet::from([role]), BranchScope::All)
        .with_authz_freshness(SubjectFreshness {
            policy_version: 1,
            subject_version: 1,
            session_generation: 1,
            step_up_generation: None,
        })
}

fn resolve_resource() -> AuthorizationResource {
    AuthorizationResource::org_wide(OrgId::knl(), "work_order").with_resource_id("wo-parity-1")
}

/// DARK: no flag row ⇒ the parity lane never runs ⇒ zero recorded observations.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn parity_lane_is_dark_when_flag_absent(pool: PgPool) {
    let org = *OrgId::knl().as_uuid();
    seed_org(&pool, org, "A").await;
    let user = seed_user(&pool, org, "SUPER_ADMIN").await;
    seed_freshness(&pool, org, user).await;

    let rt_pool = runtime_role_pool(&pool).await;
    let principal = fresh_principal(user, Role::SuperAdmin);

    CURRENT_ORG
        .scope(
            OrgId::knl(),
            observe_parity(
                &rt_pool,
                &principal,
                OrgId::knl(),
                Feature::WorkOrderReadAll,
                resolve_resource(),
                OBJECT_RESOLVE_DOMAIN,
                CEDAR_PBAC_SHADOW_OBJECT_RESOLVE_FLAG,
                true,
            ),
        )
        .await;

    assert!(
        read_parity_observations(&pool, org).await.is_empty(),
        "dark default must record ZERO parity observations"
    );
}

/// Flag ON, MEMBER whom legacy ALLOWS (we pass `legacy_allowed = true`) but whom
/// Cedar DENIES (Member is Deny for WorkOrderReadAll in the matrix): the lane
/// records exactly one divergent observation, and the report surfaces it. The
/// call returns `()` — it never gates anything.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn divergence_is_recorded_and_report_surfaces_it(pool: PgPool) {
    let org = *OrgId::knl().as_uuid();
    seed_org(&pool, org, "A").await;
    let user = seed_user(&pool, org, "MEMBER").await;
    seed_freshness(&pool, org, user).await;
    enable_flag(&pool, org, CEDAR_PBAC_SHADOW_OBJECT_RESOLVE_FLAG).await;

    let rt_pool = runtime_role_pool(&pool).await;
    let principal = fresh_principal(user, Role::Member);

    CURRENT_ORG
        .scope(
            OrgId::knl(),
            observe_parity(
                &rt_pool,
                &principal,
                OrgId::knl(),
                Feature::WorkOrderReadAll,
                resolve_resource(),
                OBJECT_RESOLVE_DOMAIN,
                CEDAR_PBAC_SHADOW_OBJECT_RESOLVE_FLAG,
                true, // legacy "allowed" — the seeded divergence
            ),
        )
        .await;

    let observations = read_parity_observations(&pool, org).await;
    assert_eq!(
        observations.len(),
        1,
        "flag ON records exactly one observation"
    );
    let obs = &observations[0];
    assert!(
        obs.divergent,
        "MEMBER allow-by-legacy vs deny-by-cedar diverges"
    );
    assert_eq!(obs.legacy_effect, DecisionEffect::Allow);
    assert_eq!(obs.shadow_effect, DecisionEffect::Deny);
    assert_eq!(obs.action, "work_order_read_all");
    assert_eq!(obs.resource_kind, "work_order");
    assert_eq!(obs.principal_roles, vec!["MEMBER".to_owned()]);

    // The promotion evidence artifact surfaces this concrete divergence per site.
    let report = aggregate(observations.into_iter().map(|o| (org.to_string(), o)));
    assert_eq!(report.disagree, 1);
    assert!(!report.all_sites_clean());
    let site = &report.per_site[&org.to_string()];
    assert_eq!(site.disagree, 1);
    assert_eq!(site.divergences.len(), 1);
    assert_eq!(site.divergences[0].action, "work_order_read_all");
    assert_eq!(site.divergences[0].shadow_effect, DecisionEffect::Deny);
    assert_eq!(site.divergences[0].legacy_effect, DecisionEffect::Allow);
}

/// Flag ON, SUPER_ADMIN whom BOTH engines allow: an AGREEMENT is recorded, and
/// the site reports clean (the zero-divergence promotion signal).
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn agreement_is_recorded_and_site_reports_clean(pool: PgPool) {
    let org = *OrgId::knl().as_uuid();
    seed_org(&pool, org, "A").await;
    let user = seed_user(&pool, org, "SUPER_ADMIN").await;
    seed_freshness(&pool, org, user).await;
    enable_flag(&pool, org, CEDAR_PBAC_SHADOW_OBJECT_RESOLVE_FLAG).await;

    let rt_pool = runtime_role_pool(&pool).await;
    let principal = fresh_principal(user, Role::SuperAdmin);

    CURRENT_ORG
        .scope(
            OrgId::knl(),
            observe_parity(
                &rt_pool,
                &principal,
                OrgId::knl(),
                Feature::WorkOrderReadAll,
                resolve_resource(),
                OBJECT_RESOLVE_DOMAIN,
                CEDAR_PBAC_SHADOW_OBJECT_RESOLVE_FLAG,
                true,
            ),
        )
        .await;

    let observations = read_parity_observations(&pool, org).await;
    assert_eq!(observations.len(), 1);
    assert!(
        !observations[0].divergent,
        "SUPER_ADMIN agrees on both engines"
    );
    assert_eq!(observations[0].shadow_effect, DecisionEffect::Allow);

    let report = aggregate(observations.into_iter().map(|o| (org.to_string(), o)));
    assert_eq!(report.disagree, 0);
    assert!(report.all_sites_clean());
    assert!(report.per_site[&org.to_string()].clean);
}

/// console_rt + FORCE RLS: the parity observation lands ONLY under the armed tenant.
/// Under org A's GUC the write is scoped to A; org B (flag also on) records
/// nothing, and A's row is invisible to a B-armed console_rt reader.
#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn parity_observation_stays_org_scoped_as_runtime_role(owner_pool: PgPool) {
    let org_a = *OrgId::knl().as_uuid();
    seed_org(&owner_pool, org_a, "A").await;
    seed_org(&owner_pool, ORG_B, "B").await;
    let user_a = seed_user(&owner_pool, org_a, "MEMBER").await;
    seed_freshness(&owner_pool, org_a, user_a).await;
    enable_flag(&owner_pool, org_a, CEDAR_PBAC_SHADOW_OBJECT_RESOLVE_FLAG).await;
    enable_flag(&owner_pool, ORG_B, CEDAR_PBAC_SHADOW_OBJECT_RESOLVE_FLAG).await;

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let principal = fresh_principal(user_a, Role::Member);

    CURRENT_ORG
        .scope(
            OrgId::knl(),
            observe_parity(
                &rt_pool,
                &principal,
                OrgId::knl(),
                Feature::WorkOrderReadAll,
                resolve_resource(),
                OBJECT_RESOLVE_DOMAIN,
                CEDAR_PBAC_SHADOW_OBJECT_RESOLVE_FLAG,
                true,
            ),
        )
        .await;

    assert_eq!(
        read_parity_observations(&owner_pool, org_a).await.len(),
        1,
        "the observation lands under the armed tenant A"
    );
    assert!(
        read_parity_observations(&owner_pool, ORG_B)
            .await
            .is_empty(),
        "no parity row may land under the other tenant B"
    );

    // A B-armed console_rt reader cannot see A's parity row (FORCE RLS isolation).
    {
        let mut tx = rt_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(ORG_B.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        let visible: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_events WHERE action = $1 AND org_id = $2",
        )
        .bind(CEDAR_PBAC_PARITY_AUDIT_ACTION)
        .bind(org_a)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(visible, 0, "org B must not see org A's parity row");
    }
}
