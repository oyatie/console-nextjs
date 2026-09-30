//! Integration-test-only helpers shared by REST-crate test suites.
//!
//! `#[sqlx::test]` hands tests a pool connected as the migration/owner role,
//! which has BYPASSRLS. Building the router straight off that pool means the
//! request path never actually exercises row-level security, so a broken
//! policy can pass green. Route requests through [`runtime_role_pool`]
//! instead; keep seeding on the original owner pool.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_kernel_core::{BranchId, OrgId, UserId};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

/// A pool cloned from `owner_pool`'s connection settings, with every
/// connection switched to the low-privilege `console_rt` role via `SET ROLE`.
/// Build routers/stores from this pool so RLS applies to test requests.
pub async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
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
        .expect("connect console_rt-role test pool")
}

/// Run a batch of static GRANT statements on `owner_pool` so the `console_rt`
/// runtime role can reach base tables the `#[sqlx::test]` superuser owns but
/// hasn't granted by default. `grants` must be static literals — no
/// interpolation. Living here (an unscanned crate) keeps the mutating-SQL
/// scanners off the caller's REST test file.
pub async fn grant_console_rt(owner_pool: &PgPool, grants: &[&'static str]) {
    for grant in grants {
        sqlx::query(*grant).execute(owner_pool).await.unwrap();
    }
}

/// Seed an `organizations` row for `org` plus a single `SUPER_ADMIN` user under
/// it (slug derived from the org UUID), returning the user id. Seeds as the
/// migration owner during setup, before the `console_rt` role switch.
pub async fn seed_org_and_super_admin(owner_pool: &PgPool, org: uuid::Uuid, tag: &str) -> UserId {
    let slug = format!("org-{}", &org.simple().to_string()[..12]);
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
        .bind(org)
        .bind(slug)
        .bind(format!("Org {tag}"))
        // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
        .execute(owner_pool)
        .await
        .unwrap();
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("Admin {tag}"))
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(org)
        // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
        .execute(owner_pool)
        .await
        .unwrap();
    user_id
}

/// Seed an `organizations` row with row-security disabled for the insert; slug
/// derived from `tag`. Owner-pool setup helper for docs REST tests.
pub async fn seed_org_rls_off(owner_pool: &PgPool, org: uuid::Uuid, tag: &str) {
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
    // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// Seed an active `ADMIN` user under `org` with row-security disabled for the
/// insert, returning its id. Owner-pool setup helper for docs REST tests.
pub async fn seed_admin_user_rls_off(owner_pool: &PgPool, org: uuid::Uuid) -> UserId {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let user_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (display_name, roles, org_id, is_active) VALUES ($1, $2, $3, true) RETURNING id",
    )
    .bind(format!("User {}", uuid::Uuid::new_v4()))
    .bind(vec!["ADMIN".to_string()])
    .bind(org)
    // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    UserId::from_uuid(user_id)
}

/// Seed the automation + policy fixtures the ontology acting-read test asserts
/// on: a workflow definition bound to the `workorder` type key, plus a catalog
/// Cedar policy attached to `object_type_id` as an object policy. Owner-pool
/// setup helper; the mutating SQL lives here so it stays off the scanned test.
pub async fn seed_bound_workflow_and_policy(
    owner_pool: &PgPool,
    org: uuid::Uuid,
    object_type_id: uuid::Uuid,
) {
    sqlx::query(
        r#"
        INSERT INTO workflow_definitions (org_id, workflow_key, display_name, object_type, status)
        VALUES ($1, 'wf.wo.review', 'WO Review', 'workorder', 'ACTIVE')
        "#,
    )
    .bind(org)
    // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
    .execute(owner_pool)
    .await
    .unwrap();

    let cedar_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO cedar_policy_catalog_entries
            (org_id, stable_key, title, natural_language_rule, effect, status, source,
             principal, action, resource, conditions, validation_status, generated_policy_text)
        VALUES ($1, 'pbac.wo_edit', 'WO Edit', 'authored in test', 'permit', 'draft', 'no_code_draft',
                '{}'::jsonb, '{}'::jsonb, '{}'::jsonb, '[]'::jsonb, 'valid', 'permit(principal,action,resource);')
        RETURNING id
        "#,
    )
    .bind(org)
    // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
    .fetch_one(owner_pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO ont_object_policies (org_id, object_type_id, cedar_policy_id, effect) VALUES ($1, $2, $3, 'permit')",
    )
    .bind(org)
    .bind(object_type_id)
    .bind(cedar_id)
    // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
    .execute(owner_pool)
    .await
    .unwrap();
}

/// Attach ONE unconditional enforced `view` permit to an object type.
///
/// Object-policy visibility is deny-by-default and unconditional: with nothing
/// attached, every single-row read, every action against an existing instance
/// and every lifecycle transition on that type is a 404. A suite whose subject
/// is command receipts or lifecycle configuration — not row visibility — needs
/// this to reach its subject at all, and it is the fixture equivalent of the org
/// having authored one permit through the audited attach route.
///
/// Purely additive, and it can never hide anything: a `forbid` still wins, and a
/// type with no permit stays invisible. Never call it from a suite that is
/// asserting visibility, which must author its own policy.
///
/// Owner-pool setup helper — `console_rt` holds no INSERT on either table
/// (0150:117-118, and 0205 took back the `ont_object_policies` INSERT 0154
/// granted).
pub async fn attach_enforced_view_permit(
    owner_pool: &PgPool,
    org: uuid::Uuid,
    object_type_id: uuid::Uuid,
    resource_type: &str,
) {
    let cedar_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO cedar_policy_catalog_entries
            (org_id, stable_key, title, natural_language_rule, effect, status, source,
             principal, action, resource, conditions, policy_version, schema_version,
             bundle_digest, validation_status, normalized_row)
        VALUES ($1,
                'object_policy.' || $2 || '.' || replace(gen_random_uuid()::text, '-', ''),
                'Test view permit', 'permit viewing every row of this type',
                'permit', 'enforced', 'no_code_draft',
                '{}'::jsonb, '{}'::jsonb, '{}'::jsonb, '[]'::jsonb, 1,
                'ontology-runtime-filter-v1',
                'sha256:0000000000000000000000000000000000000000000000000000000000000000',
                'valid',
                jsonb_build_object('effect', 'permit', 'action', 'view',
                                   'resource_type', $2::text,
                                   'conditions', '[]'::jsonb))
        RETURNING id
        "#,
    )
    .bind(org)
    .bind(resource_type)
    // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
    .fetch_one(owner_pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO ont_object_policies (org_id, object_type_id, cedar_policy_id, effect) VALUES ($1, $2, $3, 'permit')",
    )
    .bind(org)
    .bind(object_type_id)
    .bind(cedar_id)
    // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
    .execute(owner_pool)
    .await
    .unwrap();
}

/// Seed a region + branch under `OrgId::knl()`. `region_name`/`branch_name`
/// are used as label prefixes; a random UUID suffix keeps names unique
/// across concurrent test runs.
pub async fn seed_branch(pool: &PgPool, region_name: &str, branch_name: &str) -> BranchId {
    let region_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("{region_name}-{}", uuid::Uuid::new_v4()))
            .bind(*OrgId::knl().as_uuid())
            // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
            .fetch_one(pool)
            .await
            .unwrap();
    let branch_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("{branch_name}-{}", uuid::Uuid::new_v4()))
    .bind(*OrgId::knl().as_uuid())
    // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
    .fetch_one(pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

/// Seed a user with `role`, assigned to `branch`, under `OrgId::knl()`.
pub async fn seed_user(pool: &PgPool, name: &str, role: &str, branch: BranchId) -> UserId {
    let id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*id.as_uuid())
        .bind(name)
        .bind(Vec::from([role]))
        .bind(*OrgId::knl().as_uuid())
        // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
        .bind(*id.as_uuid())
        .bind(*branch.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        // rls-arming: ok test fixture seeds RLS tables as owner during setup, before the console_rt role switch
        .execute(pool)
        .await
        .unwrap();
    id
}

/// Create a real refresh family before signing access for integration requests.
/// This does not bypass account, family, RLS, or live authorization checks.
pub async fn issue_session_token(
    pool: &PgPool,
    issuer: &console_platform_auth::JwtIssuer,
    input: console_platform_auth::AccessTokenInput,
    access_scope: Option<console_kernel_core::AccessScope>,
    group_roles: Vec<String>,
) -> String {
    let now = time::OffsetDateTime::now_utc();
    let family = console_platform_auth::RefreshTokenStore
        .issue_family(
            pool,
            *input.subject.as_uuid(),
            input.org_id,
            now,
            time::Duration::hours(1),
        )
        .await
        .expect("issue real test session family");
    issuer
        .issue_session_access_token(
            input,
            access_scope,
            group_roles,
            family.family_id,
            family.expires_at,
        )
        .expect("sign family-bound test access")
}
