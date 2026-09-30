#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Expand-contract probes for 0225 (T8/T12).
//!
//! Must not use `#[sqlx::test(migrations = "./migrations")]`: that would apply
//! 0225 before the ungrouped insert. Apply 0001–0224, seed, then execute 0225
//! as `console_app` BYPASSRLS (production migrate class).

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

const MIGRATION_0225: &str = include_str!("../migrations/0225_mandatory_group_of_one.sql");
const MIGRATOR_PASSWORD: &str = "group-of-one-mint-0225";
const GROUPS_SLUG_CHECK: &str = r"^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$";

const ORG_U: Uuid = Uuid::from_u128(0x0225_0000_0000_0000_0000_0000_0000_0001);
const ORG_A: Uuid = Uuid::from_u128(0x0225_0000_0000_0000_0000_0000_0000_000a);
const ORG_B: Uuid = Uuid::from_u128(0x0225_0000_0000_0000_0000_0000_0000_000b);
const GROUP_G: Uuid = Uuid::from_u128(0x0225_0000_0000_0000_0000_0000_0000_00aa);

const ORG_F: Uuid = Uuid::from_u128(0x0225_0000_0000_0000_0000_0000_0000_000f);
const ORG_X1: Uuid = Uuid::from_u128(0x0225_0000_0000_0000_0000_0000_0000_0011);
const ORG_X2: Uuid = Uuid::from_u128(0x0225_0000_0000_0000_0000_0000_0000_0012);
const GROUP_FOREIGN_GO: Uuid = Uuid::from_u128(0x0225_0000_0000_0000_0000_0000_0000_00f1);

fn hyphenless_hex(id: Uuid) -> String {
    id.simple().to_string()
}

fn go_slug(org_id: Uuid) -> String {
    format!("go-{}", hyphenless_hex(org_id))
}

fn g2_slug(org_id: Uuid) -> String {
    format!("g2-{}", hyphenless_hex(org_id))
}

async fn apply_through_0224(pool: &PgPool) {
    sqlx::migrate!("./migrations")
        .run_to(224, pool)
        .await
        .expect("0001–0224 must apply");
}

async fn login_role_pool(owner_pool: &PgPool, role: &str, password: &str) -> PgPool {
    let options = owner_pool
        .connect_options()
        .as_ref()
        .clone()
        .username(role)
        .password(password);
    PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap()
}

async fn prepare_console_app_applier(owner_pool: &PgPool) {
    sqlx::raw_sql(
        r#"
        ALTER ROLE console_app LOGIN INHERIT NOSUPERUSER BYPASSRLS
            NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD 'group-of-one-mint-0225';

        DO $owner$
        BEGIN
            EXECUTE format('ALTER DATABASE %I OWNER TO console_app', current_database());
        END
        $owner$;

        ALTER TABLE organizations OWNER TO console_app;
        ALTER TABLE groups OWNER TO console_app;
        ALTER TABLE group_memberships OWNER TO console_app;
        ALTER FUNCTION platform_remove_org_from_group(UUID, UUID) OWNER TO console_app;
        ALTER FUNCTION platform_list_groups() OWNER TO console_app;
        ALTER FUNCTION platform_create_organization(TEXT, TEXT) OWNER TO console_app;
        GRANT CREATE ON SCHEMA public TO console_app;
        "#,
    )
    .execute(owner_pool)
    .await
    .unwrap();
}

async fn apply_0225_as_console_app_bypassrls(owner_pool: &PgPool) {
    prepare_console_app_applier(owner_pool).await;
    let app_pool = login_role_pool(owner_pool, "console_app", MIGRATOR_PASSWORD).await;
    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = on")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::raw_sql(MIGRATION_0225)
        .execute(&mut *tx)
        .await
        .expect("0225 must apply as console_app BYPASSRLS");
    tx.commit().await.unwrap();
}

async fn insert_org(pool: &PgPool, id: Uuid, slug: &str, name: &str) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(slug)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_conglomerate(pool: &PgPool, group_id: Uuid, slug: &str, members: &[Uuid]) {
    sqlx::query("INSERT INTO groups (id, slug, name) VALUES ($1, $2, $3)")
        .bind(group_id)
        .bind(slug)
        .bind(format!("Group {slug}"))
        .execute(pool)
        .await
        .unwrap();
    for member in members {
        sqlx::query("INSERT INTO group_memberships (group_id, org_id) VALUES ($1, $2)")
            .bind(group_id)
            .bind(member)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("UPDATE organizations SET group_id = $1 WHERE id = $2")
            .bind(group_id)
            .bind(member)
            .execute(pool)
            .await
            .unwrap();
    }
}

async fn org_group_id(pool: &PgPool, org_id: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT group_id FROM organizations WHERE id = $1")
        .bind(org_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn membership_count(pool: &PgPool, org_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM group_memberships WHERE org_id = $1")
        .bind(org_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn group_member_count(pool: &PgPool, group_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM group_memberships WHERE group_id = $1")
        .bind(group_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn group_slug(pool: &PgPool, group_id: Uuid) -> String {
    sqlx::query_scalar("SELECT slug FROM groups WHERE id = $1")
        .bind(group_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn assert_slug_check(pool: &PgPool, slug: &str) {
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

/// T8: ungrouped U is minted; already-grouped A,B keep G with member_count 2.
#[sqlx::test(migrations = false)]
async fn t8_backfill_mints_ungrouped_and_preserves_conglomerate(pool: PgPool) {
    apply_through_0224(&pool).await;

    insert_org(&pool, ORG_U, "t8-u-org", "Ungrouped U").await;
    insert_org(&pool, ORG_A, "t8-a-org", "Member A").await;
    insert_org(&pool, ORG_B, "t8-b-org", "Member B").await;
    seed_conglomerate(&pool, GROUP_G, "t8-conglo", &[ORG_A, ORG_B]).await;

    let u_null: Option<Uuid> =
        sqlx::query_scalar("SELECT group_id FROM organizations WHERE id = $1")
            .bind(ORG_U)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(u_null.is_none(), "U must be ungrouped before 0225");

    apply_0225_as_console_app_bypassrls(&pool).await;

    let u_group = org_group_id(&pool, ORG_U).await;
    assert_ne!(u_group, ORG_U, "groups.id must not equal org id");
    assert_eq!(membership_count(&pool, ORG_U).await, 1);
    assert_eq!(group_member_count(&pool, u_group).await, 1);
    let u_slug = group_slug(&pool, u_group).await;
    assert_slug_check(&pool, &u_slug).await;
    assert_eq!(u_slug, go_slug(ORG_U));

    assert_eq!(org_group_id(&pool, ORG_A).await, GROUP_G);
    assert_eq!(org_group_id(&pool, ORG_B).await, GROUP_G);
    assert_eq!(group_member_count(&pool, GROUP_G).await, 2);

    let nullable: String = sqlx::query_scalar(
        "SELECT is_nullable FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name = 'organizations' AND column_name = 'group_id'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(nullable, "NO");
}

/// T12: foreign multi-member occupant of go-{hex} yields g2-; remint reuses g2-; CHECK holds.
#[sqlx::test(migrations = false)]
async fn t12_slug_loop_skips_foreign_go_and_reuses_g2_on_remint(pool: PgPool) {
    apply_through_0224(&pool).await;

    insert_org(&pool, ORG_F, "t12-f-org", "Foreign-hex F").await;
    insert_org(&pool, ORG_X1, "t12-x1-org", "Occupant X1").await;
    insert_org(&pool, ORG_X2, "t12-x2-org", "Occupant X2").await;
    seed_conglomerate(&pool, GROUP_FOREIGN_GO, &go_slug(ORG_F), &[ORG_X1, ORG_X2]).await;

    apply_0225_as_console_app_bypassrls(&pool).await;

    let f_group = org_group_id(&pool, ORG_F).await;
    assert_ne!(f_group, ORG_F);
    assert_ne!(f_group, GROUP_FOREIGN_GO);
    let minted_slug = group_slug(&pool, f_group).await;
    assert_eq!(
        minted_slug,
        g2_slug(ORG_F),
        "foreign multi-member go-{{hex}} must mint g2-{{hex}}"
    );
    assert_slug_check(&pool, &minted_slug).await;
    assert_eq!(membership_count(&pool, ORG_F).await, 1);
    assert_eq!(group_member_count(&pool, GROUP_FOREIGN_GO).await, 2);

    let remint: Uuid = sqlx::query_scalar("SELECT platform_remove_org_from_group($1, $2)")
        .bind(f_group)
        .bind(ORG_F)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remint, ORG_F);
    let after = org_group_id(&pool, ORG_F).await;
    assert_eq!(after, f_group, "remint must reuse the empty g2- group");
    assert_eq!(group_slug(&pool, after).await, g2_slug(ORG_F));
    assert_eq!(membership_count(&pool, ORG_F).await, 1);

    let slugs: Vec<String> = sqlx::query_scalar("SELECT slug FROM groups")
        .fetch_all(&pool)
        .await
        .unwrap();
    for slug in &slugs {
        assert_slug_check(&pool, slug).await;
    }
}
