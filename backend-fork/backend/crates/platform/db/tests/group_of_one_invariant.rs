#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Mandatory Group-of-one probes for `organizations.group_id`.
//!
//! After migrate, every organization has a real Group row, a unique membership,
//! and `group_id` NOT NULL. `console_rt` remains SELECT-only on organizations
//! and has no raw DML or EXECUTE on mint helpers / membership/grants.

use console_kernel_core::OrgId;
use sqlx::{PgPool, Row};
use uuid::Uuid;

const SET_RUNTIME_ROLE: &str = "SET LOCAL ROLE console_rt";
const GROUPS_SLUG_CHECK: &str = r"^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$";

fn knl_id() -> Uuid {
    *OrgId::knl().as_uuid()
}

fn platform_id() -> Uuid {
    *OrgId::platform().as_uuid()
}

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

fn database_error_code(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()?
        .code()
        .map(|code| code.into_owned())
}

async fn assert_slug_satisfies_groups_check(pool: &PgPool, slug: &str) {
    let ok: bool = sqlx::query_scalar("SELECT $1 ~ '^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$'")
        .bind(slug)
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(
        ok,
        "groups.slug {slug:?} must satisfy CHECK {GROUPS_SLUG_CHECK}"
    );
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

/// T1: after full migrate, no organization may lack `group_id`.
#[sqlx::test(migrations = "./migrations")]
async fn t1_every_organization_has_group_id(pool: PgPool) {
    let nulls: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM organizations WHERE group_id IS NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        nulls, 0,
        "COUNT(*) FROM organizations WHERE group_id IS NULL must be 0"
    );
}

/// T2: `organizations.group_id` is NOT NULL in the catalog.
#[sqlx::test(migrations = "./migrations")]
async fn t2_organizations_group_id_is_not_null(pool: PgPool) {
    let nullable: String = sqlx::query_scalar(
        "SELECT is_nullable
         FROM information_schema.columns
         WHERE table_schema = 'public'
           AND table_name = 'organizations'
           AND column_name = 'group_id'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        nullable, "NO",
        "organizations.group_id must be NOT NULL in information_schema"
    );
}

/// T3: KNL has exactly one Group-of-one (distinct id, unique membership, CHECK slug).
#[sqlx::test(migrations = "./migrations")]
async fn t3_knl_has_exactly_one_group_of_one(pool: PgPool) {
    let knl = knl_id();
    let group_id: Option<Uuid> =
        sqlx::query_scalar("SELECT group_id FROM organizations WHERE id = $1")
            .bind(knl)
            .fetch_one(&pool)
            .await
            .unwrap();
    let group_id = group_id.expect("KNL organizations.group_id must be set");
    assert_ne!(group_id, knl, "groups.id must not equal KNL org id");

    let group_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM groups WHERE id = $1")
        .bind(group_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(group_count, 1, "KNL must have exactly one groups row");

    let membership_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_memberships WHERE org_id = $1")
            .bind(knl)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        membership_count, 1,
        "KNL must have exactly one group_memberships row"
    );

    let membership_group: Uuid =
        sqlx::query_scalar("SELECT group_id FROM group_memberships WHERE org_id = $1")
            .bind(knl)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(membership_group, group_id);

    assert!(
        unique_org_id_on_memberships(&pool).await,
        "group_memberships must UNIQUE (org_id)"
    );

    let slug: String = sqlx::query_scalar("SELECT slug FROM groups WHERE id = $1")
        .bind(group_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_slug_satisfies_groups_check(&pool, &slug).await;
    assert!(
        is_group_of_one_slug(&slug, knl),
        "KNL group slug {slug:?} must be go-||32hex (or gN- if go- is taken); knl hex {}",
        hyphenless_hex(knl)
    );
}

/// T4: owner SQL for the platform sentinel; list omits its group and includes KNL.
#[sqlx::test(migrations = "./migrations")]
async fn t4_sentinel_group_archived_omitted_from_list_knl_included(pool: PgPool) {
    let platform = platform_id();
    let knl = knl_id();

    let sentinel = sqlx::query(
        "SELECT o.group_id, g.id AS gid, g.status
         FROM organizations o
         JOIN groups g ON g.id = o.group_id
         WHERE o.id = $1",
    )
    .bind(platform)
    .fetch_optional(&pool)
    .await
    .unwrap()
    .expect("sentinel org must JOIN a groups row on organizations.group_id");

    let group_id: Uuid = sentinel.get("group_id");
    let gid: Uuid = sentinel.get("gid");
    let status: String = sentinel.get("status");
    assert_eq!(gid, group_id);
    assert_ne!(gid, platform, "sentinel groups.id must not equal org id");
    assert_eq!(status, "ARCHIVED");

    let knl_group_id: Uuid =
        sqlx::query_scalar::<_, Option<Uuid>>("SELECT group_id FROM organizations WHERE id = $1")
            .bind(knl)
            .fetch_one(&pool)
            .await
            .unwrap()
            .expect("KNL must have group_id for the list assertion");

    let listed: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM platform_list_groups()")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(
        !listed.contains(&group_id),
        "platform_list_groups must omit the sentinel group id {group_id}"
    );
    assert!(
        listed.contains(&knl_group_id),
        "platform_list_groups must include KNL's Group-of-one {knl_group_id}"
    );
}

/// T5: owner `INSERT INTO organizations (slug, name)` mints group_id + unique membership.
#[sqlx::test(migrations = "./migrations")]
async fn t5_owner_insert_organizations_mints_group_of_one(pool: PgPool) {
    let inserted = sqlx::query(
        "INSERT INTO organizations (slug, name)
         VALUES ('t5-go-insert', 'T5 Group of One')
         RETURNING id, group_id",
    )
    .fetch_one(&pool)
    .await
    .expect("owner INSERT INTO organizations (slug, name) must succeed");

    let org_id: Uuid = inserted.get("id");
    let group_id: Option<Uuid> = inserted.get("group_id");
    let group_id = group_id.expect("inserted organization.group_id must be NOT NULL");
    assert_ne!(group_id, org_id);

    let membership_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_memberships WHERE org_id = $1")
            .bind(org_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        membership_count, 1,
        "inserted org must have exactly one membership"
    );

    let membership_group: Uuid =
        sqlx::query_scalar("SELECT group_id FROM group_memberships WHERE org_id = $1")
            .bind(org_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(membership_group, group_id);

    let members_of_group: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_memberships WHERE group_id = $1")
            .bind(group_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        members_of_group, 1,
        "minted Group-of-one must have unique membership on that group_id"
    );

    assert!(
        unique_org_id_on_memberships(&pool).await,
        "group_memberships must UNIQUE (org_id)"
    );
}

/// T6: `console_rt` still cannot INSERT `organizations` (42501).
#[sqlx::test(migrations = "./migrations")]
async fn t6_console_rt_insert_organizations_is_42501(pool: PgPool) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(SET_RUNTIME_ROLE)
        .execute(&mut *tx)
        .await
        .unwrap();
    let err =
        sqlx::query("INSERT INTO organizations (slug, name) VALUES ('t6-rt-insert', 'T6 Denied')")
            .execute(&mut *tx)
            .await
            .expect_err("console_rt must not INSERT organizations");
    assert_eq!(
        database_error_code(&err).as_deref(),
        Some("42501"),
        "console_rt INSERT INTO organizations must be 42501, got: {err}"
    );
}

/// T10: `console_rt` SELECT on owner-only membership/grants is permission denied.
#[sqlx::test(migrations = "./migrations")]
async fn t10_console_rt_select_memberships_and_grants_denied(pool: PgPool) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(SET_RUNTIME_ROLE)
        .execute(&mut *tx)
        .await
        .unwrap();
    let memberships_err = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM group_memberships")
        .fetch_one(&mut *tx)
        .await
        .expect_err("console_rt must not read owner-only group_memberships")
        .to_string();
    assert!(
        memberships_err.contains("permission denied"),
        "raw group_memberships read as console_rt must be denied, got: {memberships_err}"
    );
    let _ = tx.rollback().await;

    let mut tx = pool.begin().await.unwrap();
    sqlx::query(SET_RUNTIME_ROLE)
        .execute(&mut *tx)
        .await
        .unwrap();
    let grants_err = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM group_role_grants")
        .fetch_one(&mut *tx)
        .await
        .expect_err("console_rt must not read owner-only group_role_grants")
        .to_string();
    assert!(
        grants_err.contains("permission denied"),
        "raw group_role_grants read as console_rt must be denied, got: {grants_err}"
    );
}

/// T11: `console_rt` INSERT membership / UPDATE groups is permission denied.
#[sqlx::test(migrations = "./migrations")]
async fn t11_console_rt_insert_memberships_and_update_groups_denied(pool: PgPool) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(SET_RUNTIME_ROLE)
        .execute(&mut *tx)
        .await
        .unwrap();
    let insert_err =
        sqlx::query("INSERT INTO group_memberships (group_id, org_id) VALUES ($1, $2)")
            .bind(Uuid::from_u128(0xaaaa_bbbb_cccc_dddd_eeee_ffff_0000_0001))
            .bind(knl_id())
            .execute(&mut *tx)
            .await
            .expect_err("console_rt must not INSERT group_memberships")
            .to_string();
    assert!(
        insert_err.contains("permission denied"),
        "console_rt INSERT group_memberships must be denied, got: {insert_err}"
    );
    let _ = tx.rollback().await;

    let mut tx = pool.begin().await.unwrap();
    sqlx::query(SET_RUNTIME_ROLE)
        .execute(&mut *tx)
        .await
        .unwrap();
    let update_err = sqlx::query("UPDATE groups SET name = 'x' WHERE id = $1")
        .bind(knl_id())
        .execute(&mut *tx)
        .await
        .expect_err("console_rt must not UPDATE groups")
        .to_string();
    assert!(
        update_err.contains("permission denied"),
        "console_rt UPDATE groups must be denied, got: {update_err}"
    );
}

/// T14: stale `remove(A, org)` while the org is in B is a no-op (no remint).
#[sqlx::test(migrations = "./migrations")]
async fn t14_stale_remove_leaves_org_in_current_group(pool: PgPool) {
    let org = sqlx::query(
        "INSERT INTO organizations (slug, name)
         VALUES ('t14-stay-b', 'T14 Stay in B')
         RETURNING id, group_id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let org_id: Uuid = org.get("id");
    let minted: Uuid = org.get("group_id");

    let group_a: Uuid = sqlx::query_scalar("SELECT platform_create_group('t14-group-a', 'T14 A')")
        .fetch_one(&pool)
        .await
        .unwrap();
    let group_b: Uuid = sqlx::query_scalar("SELECT platform_create_group('t14-group-b', 'T14 B')")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_ne!(group_a, minted);

    let assigned: Uuid = sqlx::query_scalar("SELECT platform_assign_org_to_group($1, $2)")
        .bind(group_b)
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(assigned, org_id);
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>("SELECT group_id FROM organizations WHERE id = $1")
            .bind(org_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        group_b
    );

    let removed: Uuid = sqlx::query_scalar("SELECT platform_remove_org_from_group($1, $2)")
        .bind(group_a)
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(removed, org_id);

    let after: Uuid = sqlx::query_scalar("SELECT group_id FROM organizations WHERE id = $1")
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, group_b, "stale remove(A) must not remint out of B");

    let membership_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_memberships WHERE org_id = $1")
            .bind(org_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(membership_count, 1);
    let membership_group: Uuid =
        sqlx::query_scalar("SELECT group_id FROM group_memberships WHERE org_id = $1")
            .bind(org_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(membership_group, group_b);
}

/// T15: `console_rt` cannot EXECUTE mint helpers or trigger wrappers.
#[sqlx::test(migrations = "./migrations")]
async fn t15_console_rt_execute_mint_helpers_denied(pool: PgPool) {
    for (name, args) in [
        ("platform_attach_group_of_one", "uuid"),
        ("platform_mint_group_row", "uuid, text, text"),
        ("platform_attach_membership", "uuid, uuid"),
        ("platform_mint_missing_group_of_one", "uuid"),
        ("organizations_before_insert_mint_group", ""),
        ("organizations_after_insert_attach_membership", ""),
    ] {
        let reg: Option<String> = sqlx::query_scalar("SELECT to_regprocedure($1)::text")
            .bind(format!("{name}({args})"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            reg.is_some(),
            "mint helper {name}({args}) must exist after 0225"
        );
        let allowed: bool = sqlx::query_scalar(
            "SELECT has_function_privilege('console_rt', $1::regprocedure, 'EXECUTE')",
        )
        .bind(reg.unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            !allowed,
            "console_rt must not have EXECUTE on {name}({args})"
        );
    }

    const DENIED: &[&str] = &[
        "SELECT platform_attach_group_of_one('00000000-0000-0000-0000-0000000000a1')",
        "SELECT platform_mint_group_row('00000000-0000-0000-0000-0000000000a1', 'x', 'ACTIVE')",
        "SELECT platform_attach_membership('00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-0000000000a1')",
        "SELECT platform_mint_missing_group_of_one('00000000-0000-0000-0000-0000000000a1')",
        "SELECT organizations_before_insert_mint_group()",
        "SELECT organizations_after_insert_attach_membership()",
    ];
    for sql in DENIED {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(SET_RUNTIME_ROLE)
            .execute(&mut *tx)
            .await
            .unwrap();
        let err = sqlx::query(*sql)
            .execute(&mut *tx)
            .await
            .expect_err("console_rt must not EXECUTE mint helpers")
            .to_string();
        assert!(
            err.contains("permission denied"),
            "console_rt EXECUTE must be denied, got: {err} (sql: {sql})"
        );
        let _ = tx.rollback().await;
    }
}

/// T16: list_groups cardinality covers non-sentinel orgs; leftover empty go- stays listed.
#[sqlx::test(migrations = "./migrations")]
async fn t16_list_groups_includes_leftover_empty_group_of_ones(pool: PgPool) {
    let non_sentinel: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM organizations
         WHERE id <> '00000000-0000-0000-0000-00000000face'::uuid",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let listed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM platform_list_groups()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        listed >= non_sentinel,
        "platform_list_groups cardinality {listed} must be >= non-sentinel org count {non_sentinel}"
    );

    let conglomerate: Uuid =
        sqlx::query_scalar("SELECT platform_create_group('t16-holdings', 'T16 Holdings')")
            .fetch_one(&pool)
            .await
            .unwrap();
    let assigned_org = sqlx::query(
        "INSERT INTO organizations (slug, name)
         VALUES ('t16-assign-org', 'T16 Assign')
         RETURNING id, group_id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let assigned_id: Uuid = assigned_org.get("id");
    let leftover_go: Uuid = assigned_org.get("group_id");
    sqlx::query_scalar::<_, Uuid>("SELECT platform_assign_org_to_group($1, $2)")
        .bind(conglomerate)
        .bind(assigned_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    let leftover_listed =
        sqlx::query("SELECT id, member_count FROM platform_list_groups() WHERE id = $1")
            .bind(leftover_go)
            .fetch_optional(&pool)
            .await
            .unwrap()
            .expect("leftover empty go- after assign must remain listed");
    let leftover_count: i64 = leftover_listed.get("member_count");
    assert_eq!(leftover_count, 0);

    let deleted = sqlx::query(
        "INSERT INTO organizations (slug, name)
         VALUES ('t16-delete-org', 'T16 Delete')
         RETURNING id, group_id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let deleted_id: Uuid = deleted.get("id");
    let deleted_go: Uuid = deleted.get("group_id");
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(deleted_id)
        .execute(&pool)
        .await
        .unwrap();

    let after_delete =
        sqlx::query("SELECT id, member_count FROM platform_list_groups() WHERE id = $1")
            .bind(deleted_go)
            .fetch_optional(&pool)
            .await
            .unwrap()
            .expect("go-/gN- group must remain listed after owner DELETE of the org");
    let after_count: i64 = after_delete.get("member_count");
    assert_eq!(after_count, 0);
}
