#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS + freshness gate for `subject_authz_versions` (Cedar/PBAC
//! activation, ADR-0021, migration 0096).
//!
//! Two concerns proven as the GENUINE non-owner runtime role `console_rt`
//! (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) — never the BYPASSRLS superuser the
//! default `#[sqlx::test]` pool connects as, which would green-light a broken or
//! leaking policy:
//!   1. Tenant isolation: under org A's armed GUC, only A's freshness row is
//!      visible; B's is invisible. console_rt may NOT DELETE (REVOKE DELETE governance
//!      history), so a runtime actor can never erase a subject's freshness back to
//!      the absent-row "0" baseline.
//!   2. Sourcing: `PgOrgStore::get_subject_authz_versions` reads the current
//!      `(version, session_generation)` and returns `(0, 0)` before any bump; a
//!      role change bumps the version and a deactivation bumps session_generation,
//!      each inside the existing audited transaction.
//!
//! SLICE-2 is additive: no authorization decision consults this table yet.

use console_identity_adapter_postgres::PgOrgStore;
use console_identity_application::{
    CreatePolicyAssignmentPreviewReceiptCommand, DeactivateUserCommand, DirectoryListQuery,
    UpdateUserCommand, UserListQuery,
};
use console_kernel_core::{BranchScope, OrgId, TraceContext, UserId};
use console_platform_request_context::CURRENT_ORG;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// A second, non-KNL tenant id, to prove cross-tenant isolation under `console_rt`.
const ORG_B: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);

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

/// Seed an ACTIVE user (owner pool, row_security off). Audit actors and freshness
/// rows both FK to `users`, so every fixture user is a real row.
async fn seed_active_user(owner_pool: &PgPool, org: Uuid) -> UserId {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id, is_active) VALUES ($1, $2, $3, true) RETURNING id",
    )
    .bind(format!("User {}", Uuid::new_v4()))
    .bind(vec!["MECHANIC".to_string()])
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    UserId::from_uuid(user_id)
}

/// Insert a freshness row directly as the owner (row_security off), so the RLS
/// isolation test starts from a known, tenant-tagged row per org.
async fn seed_freshness_row(owner_pool: &PgPool, org: Uuid, user: UserId) {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO subject_authz_versions (org_id, user_id, version, session_generation) VALUES ($1, $2, 1, 1)",
    )
    .bind(org)
    .bind(*user.as_uuid())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// Count freshness rows visible to `console_rt` under the given tenant GUC.
async fn count_as_runtime(rt_pool: &PgPool, org: Uuid) -> i64 {
    let mut tx = rt_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM subject_authz_versions")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    count
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn subject_authz_versions_isolate_tenants_and_deny_delete_as_runtime_role(
    owner_pool: PgPool,
) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org_a = *OrgId::knl().as_uuid();
    let org_b = ORG_B;
    seed_org(&owner_pool, org_a, "A").await;
    seed_org(&owner_pool, org_b, "B").await;
    let user_a = seed_active_user(&owner_pool, org_a).await;
    let user_b = seed_active_user(&owner_pool, org_b).await;
    seed_freshness_row(&owner_pool, org_a, user_a).await;
    seed_freshness_row(&owner_pool, org_b, user_b).await;

    // (1) Under each tenant's GUC, console_rt sees ONLY that tenant's freshness row.
    assert_eq!(
        count_as_runtime(&rt_pool, org_a).await,
        1,
        "org A must see exactly its own freshness row"
    );
    assert_eq!(
        count_as_runtime(&rt_pool, org_b).await,
        1,
        "org B must see exactly its own freshness row"
    );

    // (2) Cross-tenant: under org A's GUC, B's row is invisible (queried by id).
    {
        let mut tx = rt_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org_a.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        let visible: i64 =
            sqlx::query_scalar("SELECT count(*) FROM subject_authz_versions WHERE user_id = $1")
                .bind(*user_b.as_uuid())
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            visible, 0,
            "org B's freshness row must be invisible under A"
        );
    }

    // (3) console_rt may NEVER DELETE freshness rows (REVOKE DELETE governance guard),
    // so it cannot erase a subject's freshness back to the absent-row baseline.
    {
        let mut tx = rt_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org_a.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        let err = sqlx::query("DELETE FROM subject_authz_versions")
            .execute(&mut *tx)
            .await
            .expect_err("console_rt must not DELETE subject_authz_versions")
            .to_string();
        assert!(
            err.contains("permission denied"),
            "DELETE as console_rt must be denied by the REVOKE, got: {err}"
        );
        let _ = tx.rollback().await;
    }
}

/// Mint the impact-preview receipt that a system-role replacement now requires
/// (adapter consumes it against the locked baseline inside `update_user`). The
/// target here has no policy roles/branches and the org seeds no `policy_versions`
/// row, so every baseline field but the system-role set is empty.
// ponytail: policy_version hardcoded 0 = the seeded baseline (no policy_versions
// row → lock_policy_version_tx unwrap_or(0)); read it from the DB if a future
// fixture starts bumping org policy_version.
async fn mint_role_change_receipt(
    store: &PgOrgStore,
    org: OrgId,
    actor: UserId,
    target: UserId,
    current_system_roles: Vec<String>,
    new_system_roles: Vec<String>,
) -> Uuid {
    CURRENT_ORG
        .scope(
            org,
            store.create_policy_assignment_preview_receipt(
                CreatePolicyAssignmentPreviewReceiptCommand {
                    actor,
                    user_id: target,
                    current_branch_ids: Vec::new(),
                    current_system_roles,
                    current_role_ids: Vec::new(),
                    branch_ids: Vec::new(),
                    system_roles: new_system_roles,
                    role_ids: Vec::new(),
                    policy_version: 0,
                    expires_at: OffsetDateTime::now_utc() + Duration::hours(1),
                },
            ),
        )
        .await
        .expect("minting a preview receipt must succeed as console_rt under the armed GUC")
        .id
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn bump_and_get_subject_authz_versions_via_store(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid, "A").await;
    let actor = seed_active_user(&owner_pool, org_uuid).await;
    let target = seed_active_user(&owner_pool, org_uuid).await;

    let store = PgOrgStore::new(rt_pool.clone());

    // Before any bump there is no row → the safe (0, 0) baseline.
    let initial = CURRENT_ORG
        .scope(org, store.get_subject_authz_versions(target))
        .await
        .expect("get must succeed as console_rt under the armed GUC");
    assert_eq!(initial, (0, 0), "no bump yet must read as (0, 0)");

    // A system-role change bumps the subject version in the same audited tx. The
    // first bump creates the row at the (1, 1) monotonic baseline.
    let receipt = mint_role_change_receipt(
        &store,
        org,
        actor,
        target,
        vec!["MECHANIC".to_owned()],
        vec!["ADMIN".to_owned()],
    )
    .await;
    CURRENT_ORG
        .scope(
            org,
            store.update_user(UpdateUserCommand {
                actor,
                user_id: target,
                display_name: None,
                employee_id: None,
                phone: None,
                team: None,
                roles: Some(vec!["ADMIN".to_owned()]),
                branch_ids: None,
                preview_receipt_id: Some(receipt),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            }),
        )
        .await
        .expect("update_user must succeed as console_rt under the armed GUC");
    assert_eq!(
        CURRENT_ORG
            .scope(org, store.get_subject_authz_versions(target))
            .await
            .unwrap(),
        (1, 1),
        "first role change must create the row at (version 1, session_generation 1)"
    );

    // A second role change increments the version only.
    let receipt = mint_role_change_receipt(
        &store,
        org,
        actor,
        target,
        vec!["ADMIN".to_owned()],
        vec!["SUPER_ADMIN".to_owned()],
    )
    .await;
    CURRENT_ORG
        .scope(
            org,
            store.update_user(UpdateUserCommand {
                actor,
                user_id: target,
                display_name: None,
                employee_id: None,
                phone: None,
                team: None,
                roles: Some(vec!["SUPER_ADMIN".to_owned()]),
                branch_ids: None,
                preview_receipt_id: Some(receipt),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            }),
        )
        .await
        .expect("second update_user must succeed");
    assert_eq!(
        CURRENT_ORG
            .scope(org, store.get_subject_authz_versions(target))
            .await
            .unwrap(),
        (2, 1),
        "second role change must increment version, leave session_generation"
    );

    // Deactivation (credential + session revocation) bumps session_generation.
    CURRENT_ORG
        .scope(
            org,
            store.deactivate_user(DeactivateUserCommand {
                actor,
                user_id: target,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            }),
        )
        .await
        .expect("deactivate_user must succeed");
    assert_eq!(
        CURRENT_ORG
            .scope(org, store.get_subject_authz_versions(target))
            .await
            .unwrap(),
        (2, 2),
        "deactivation must increment session_generation, leave version"
    );

    // A profile-only edit (no roles) must NOT bump either counter.
    CURRENT_ORG
        .scope(
            org,
            store.update_user(UpdateUserCommand {
                actor,
                user_id: target,
                display_name: Some("Renamed".to_owned()),
                employee_id: None,
                phone: None,
                team: None,
                roles: None,
                branch_ids: None,
                // Profile-only edit: no role/scope replacement, so no receipt required.
                preview_receipt_id: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            }),
        )
        .await
        .expect("profile-only update_user must succeed");
    assert_eq!(
        CURRENT_ORG
            .scope(org, store.get_subject_authz_versions(target))
            .await
            .unwrap(),
        (2, 2),
        "a profile-only edit must not touch authorization freshness"
    );

    // The row is confined to this tenant: as the owner (bypassing RLS) exactly one
    // row exists for this user and org, proving no untenanted / duplicate write.
    let rows: i64 = {
        let mut tx = owner_pool.begin().await.unwrap();
        sqlx::query("SET LOCAL row_security = off")
            .execute(&mut *tx)
            .await
            .unwrap();
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM subject_authz_versions WHERE user_id = $1 AND org_id = $2",
        )
        .bind(*target.as_uuid())
        .bind(org_uuid)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        n
    };
    assert_eq!(rows, 1, "exactly one freshness row for the target subject");
}

/// Delta-scope enforcement: the impact-preview receipt gate fires on an actual
/// assignment CHANGE, not the mere presence of roles/branch_ids. The legacy
/// user-edit form re-sends the current roles on a profile edit — that must save
/// without a receipt and must not bump the subject version — while any real
/// change (including one that no longer matches its receipt) is still rejected.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn update_user_delta_scopes_the_preview_receipt_gate(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid, "A").await;
    let actor = seed_active_user(&owner_pool, org_uuid).await;
    let target = seed_active_user(&owner_pool, org_uuid).await; // seeded roles: ["MECHANIC"]
    let store = PgOrgStore::new(rt_pool.clone());

    // (1) No-op: a profile edit that re-sends the current role set with NO receipt
    // saves fine and does not bump the subject version (no role-change signal).
    CURRENT_ORG
        .scope(
            org,
            store.update_user(UpdateUserCommand {
                actor,
                user_id: target,
                display_name: None,
                employee_id: None,
                phone: Some(Some("010-1234-5678".to_owned())),
                team: None,
                roles: Some(vec!["MECHANIC".to_owned()]),
                branch_ids: None,
                preview_receipt_id: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            }),
        )
        .await
        .expect("re-sending the current roles on a profile edit must save without a receipt");
    assert_eq!(
        CURRENT_ORG
            .scope(org, store.get_subject_authz_versions(target))
            .await
            .unwrap(),
        (0, 0),
        "a no-op assignment resend must not bump the subject version",
    );

    let directory = CURRENT_ORG
        .scope(
            org,
            store.list_directory_people(
                &BranchScope::All,
                DirectoryListQuery {
                    search: None,
                    team: None,
                    branch_id: None,
                    include_inactive: false,
                    limit: None,
                    offset: None,
                },
            ),
        )
        .await
        .expect("directory list must succeed as console_rt");
    let listed = directory
        .items
        .iter()
        .find(|user| user.id == target)
        .expect("phone-bearing user must appear in the directory page");
    assert_eq!(
        listed.phone, None,
        "list_directory_people must not load users.phone"
    );
    let got = CURRENT_ORG
        .scope(org, store.get_user(target, &BranchScope::All))
        .await
        .expect("get_user must keep phone");
    assert_eq!(got.phone.as_deref(), Some("010-1234-5678"));
    let users = CURRENT_ORG
        .scope(
            org,
            store.list_users(
                &BranchScope::All,
                UserListQuery {
                    include_inactive: false,
                    limit: None,
                    offset: None,
                },
            ),
        )
        .await
        .expect("list_users must keep phone");
    let listed_user = users
        .items
        .iter()
        .find(|user| user.id == target)
        .expect("phone-bearing user must appear in list_users");
    assert_eq!(listed_user.phone.as_deref(), Some("010-1234-5678"));

    // (2) A real role change with NO receipt is rejected by the store
    // (enforcement-of-record) and does not bump either.
    CURRENT_ORG
        .scope(
            org,
            store.update_user(UpdateUserCommand {
                actor,
                user_id: target,
                display_name: None,
                employee_id: None,
                phone: None,
                team: None,
                roles: Some(vec!["ADMIN".to_owned()]),
                branch_ids: None,
                preview_receipt_id: None,
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            }),
        )
        .await
        .expect_err("a real role change without a receipt must be rejected by the store");
    assert_eq!(
        CURRENT_ORG
            .scope(org, store.get_subject_authz_versions(target))
            .await
            .unwrap(),
        (0, 0),
        "a rejected change must not bump the subject version",
    );

    // (3) A receipt minted for one transition must not authorize a different one
    // (stale / no-longer-matches — the TOCTOU guard on the security path).
    let receipt = mint_role_change_receipt(
        &store,
        org,
        actor,
        target,
        vec!["MECHANIC".to_owned()],
        vec!["ADMIN".to_owned()],
    )
    .await;
    CURRENT_ORG
        .scope(
            org,
            store.update_user(UpdateUserCommand {
                actor,
                user_id: target,
                display_name: None,
                employee_id: None,
                phone: None,
                team: None,
                roles: Some(vec!["SUPER_ADMIN".to_owned()]),
                branch_ids: None,
                preview_receipt_id: Some(receipt),
                trace: TraceContext::generate(),
                occurred_at: OffsetDateTime::now_utc(),
            }),
        )
        .await
        .expect_err(
            "a receipt minted for MECHANIC→ADMIN must not authorize a MECHANIC→SUPER_ADMIN change",
        );
}
