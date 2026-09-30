#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Group-of-one mint probes for `PlatformProvisioner::onboard_tenant` (PR1).
//!
//! Production create is `platform_create_organization` as `console_rt`. After
//! onboard, `organizations.group_id` is a real Group, membership is unique on
//! that org, and `with_org_conn` arms `app.current_org` with the org id, never
//! the group id.

use console_kernel_core::OrgId;
use console_platform_db::{DbError, with_org_conn};
use console_platform_provisioning::PlatformProvisioner;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const GROUPS_SLUG_CHECK: &str = r"^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$";

fn hyphenless_hex(id: Uuid) -> String {
    id.simple().to_string()
}

fn is_group_of_one_slug(slug: &str, org_id: Uuid) -> bool {
    let hex = hyphenless_hex(org_id);
    if slug == format!("go-{hex}") {
        return true;
    }
    (2..=9).any(|n| slug == format!("g{n}-{hex}"))
}

/// Build a pool whose every connection runs `SET ROLE console_rt` on checkout,
/// matching `rls_auth_chain_as_runtime_role.rs`.
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

async fn unique_org_id_on_memberships(pool: &PgPool) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1
            FROM pg_constraint c
            JOIN pg_attribute a
              ON a.attrelid = c.conrelid
             AND a.attnum = c.conkey[1]
            WHERE c.conrelid = 'public.group_memberships'::regclass
              AND c.contype = 'u'
              AND array_length(c.conkey, 1) = 1
              AND a.attname = 'org_id'
        )",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// T7: `onboard_tenant` as `console_rt` mints a Group-of-one distinct from the org.
#[sqlx::test(migrations = "../db/migrations")]
async fn t7_onboard_tenant_as_console_rt_mints_group_of_one(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let onboarded = PlatformProvisioner::new(Duration::hours(24))
        .onboard_tenant(
            &rt_pool,
            None,
            "t7-go-onboard",
            "T7 Group of One",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();

    let org_id = onboarded.organization.id;
    let group_id = onboarded
        .organization
        .group_id
        .expect("onboard_tenant must set organization.group_id");
    assert_ne!(group_id, org_id, "group_id must not equal org id");

    let group_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM groups WHERE id = $1")
        .bind(group_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert_eq!(group_count, 1, "onboard must insert exactly one groups row");

    let membership_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_memberships WHERE org_id = $1")
            .bind(org_id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(
        membership_count, 1,
        "onboarded org must have exactly one membership"
    );

    let membership_group: Uuid =
        sqlx::query_scalar("SELECT group_id FROM group_memberships WHERE org_id = $1")
            .bind(org_id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(membership_group, group_id);

    assert!(
        unique_org_id_on_memberships(&owner_pool).await,
        "group_memberships must UNIQUE (org_id)"
    );

    let slug: String = sqlx::query_scalar("SELECT slug FROM groups WHERE id = $1")
        .bind(group_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    let check_ok: bool = sqlx::query_scalar("SELECT $1 ~ '^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$'")
        .bind(&slug)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert!(
        check_ok,
        "minted group slug {slug:?} must satisfy CHECK {GROUPS_SLUG_CHECK}"
    );
    assert!(
        is_group_of_one_slug(&slug, org_id),
        "minted group slug {slug:?} must be go-||32hex (or gN- if go- is taken)"
    );
}

/// T9 onboard variant: after onboard, `with_org_conn` arms the new org id, never the group id.
#[sqlx::test(migrations = "../db/migrations")]
async fn t9_onboard_with_org_conn_arms_org_id_never_group_id(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let onboarded = PlatformProvisioner::new(Duration::hours(24))
        .onboard_tenant(
            &rt_pool,
            None,
            "t9-go-onboard",
            "T9 Group of One",
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();

    let org_id = onboarded.organization.id;
    let group_id = onboarded
        .organization
        .group_id
        .expect("onboard_tenant must set organization.group_id");
    assert_ne!(group_id, org_id);

    let armed: String = with_org_conn(&rt_pool, OrgId::from_uuid(org_id), |tx| {
        Box::pin(async move {
            sqlx::query_scalar("SELECT current_setting('app.current_org', true)")
                .fetch_one(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)
        })
    })
    .await
    .unwrap();

    assert_eq!(
        armed,
        org_id.to_string(),
        "with_org_conn must arm app.current_org with the new org id"
    );
    assert_ne!(
        armed,
        group_id.to_string(),
        "app.current_org must never equal the group id"
    );
}

/// T14: `remove(A, org)` while the org is in B does not remint and does not snap.
#[sqlx::test(migrations = "../db/migrations")]
async fn t14_stale_remove_does_not_remint_or_snap(owner_pool: PgPool) {
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let provisioner = PlatformProvisioner::new(Duration::hours(24));
    let now = OffsetDateTime::now_utc();

    let onboarded = provisioner
        .onboard_tenant(&rt_pool, None, "t14-stay-b", "T14 Stay in B", now)
        .await
        .unwrap();
    let org_id = onboarded.organization.id;

    let group_a = provisioner
        .create_group(&rt_pool, None, "t14-group-a", "T14 A", now)
        .await
        .unwrap();
    let group_b = provisioner
        .create_group(&rt_pool, None, "t14-group-b", "T14 B", now)
        .await
        .unwrap();

    let assigned = provisioner
        .assign_org_to_group(&rt_pool, None, group_b.id, org_id, now)
        .await
        .unwrap();
    assert_eq!(assigned.group_id, Some(group_b.id));

    let removed = provisioner
        .remove_org_from_group(&rt_pool, None, group_a.id, org_id, now)
        .await
        .unwrap();
    assert_eq!(
        removed.group_id,
        Some(group_b.id),
        "stale remove(A) must leave group_id = B"
    );

    let membership_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_memberships WHERE org_id = $1")
            .bind(org_id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(membership_count, 1);
    let membership_group: Uuid =
        sqlx::query_scalar("SELECT group_id FROM group_memberships WHERE org_id = $1")
            .bind(org_id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(membership_group, group_b.id);

    let after_snap: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT after_snap FROM audit_events
         WHERE action = 'platform.group.remove_org' AND target_id = $1
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(format!("{}:{org_id}", group_a.id))
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert!(
        after_snap.is_none(),
        "stale remove must not snap a remint, got {after_snap:?}"
    );
}
