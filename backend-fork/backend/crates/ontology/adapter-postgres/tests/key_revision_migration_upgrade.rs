#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

const ORG_A: Uuid = Uuid::from_u128(0xa165_a165_a165_a165_a165_a165_a165_a165);
const ORG_B: Uuid = Uuid::from_u128(0xb165_b165_b165_b165_b165_b165_b165_b165);
const LEGACY_ORG: Uuid = Uuid::from_u128(0xc165_c165_c165_c165_c165_c165_c165_c165);
const LEGACY_ACTOR: Uuid = Uuid::from_u128(0xc165_a165_c165_a165_c165_a165_c165_a165);
const MIGRATION_0165: &str =
    include_str!("../../../platform/db/migrations/0165_ontology_object_type_key_revisions.sql");
const MIGRATOR_PASSWORD: &str = "migration-owner-a165";
const COMMAND_PASSWORD: &str = "ontology-command-a165";
const MIGRATION_LOCK_TIMEOUT: &str = "5s";
const MIGRATION_STATEMENT_TIMEOUT: &str = "1min";

fn database_error_code(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()?
        .code()
        .map(|code| code.into_owned())
}

async fn restore_pre_0165_schema(pool: &PgPool) {
    sqlx::raw_sql(
        r#"
        DROP SCHEMA ontology_api CASCADE;
        DROP TABLE ont_builtin_catalog_installs;
        DROP TABLE ont_builtin_catalog_allowlist;
        ALTER TABLE ont_object_types DROP CONSTRAINT fk_ont_object_types_key_revision;
        DROP TABLE ont_object_type_key_revisions;

        -- Recreate the cluster topology that provisioning must establish before
        -- 0165. console_app gets only the two direct SET+INHERIT edges needed by
        -- the ontology and leave NOLOGIN ownership boundaries.
        ALTER ROLE console_app LOGIN INHERIT NOSUPERUSER BYPASSRLS NOCREATEDB NOCREATEROLE NOREPLICATION
            PASSWORD 'migration-owner-a165';
        ALTER ROLE console_ontology_writer NOLOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT
            NOCREATEDB NOCREATEROLE NOREPLICATION;
        ALTER ROLE console_leave_definer NOLOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT
            NOCREATEDB NOCREATEROLE NOREPLICATION;
        ALTER ROLE console_ontology_cmd LOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT
            NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD 'ontology-command-a165';
        REVOKE console_ontology_writer, console_leave_definer FROM console_rt, console_ontology_cmd;
        REVOKE console_rt, console_ontology_cmd FROM console_app, console_ontology_writer, console_leave_definer;
        GRANT console_ontology_writer TO console_app
            WITH ADMIN FALSE, INHERIT TRUE, SET TRUE;
        GRANT console_leave_definer TO console_app
            WITH ADMIN FALSE, INHERIT TRUE, SET TRUE;

        DO $db_owner$
        BEGIN
            EXECUTE format('ALTER DATABASE %I OWNER TO console_app', current_database());
        END
        $db_owner$;

        ALTER TABLE organizations OWNER TO console_app;
        ALTER TABLE users OWNER TO console_app;
        ALTER TABLE audit_events OWNER TO console_app;
        ALTER TABLE gov_approval_requests OWNER TO console_app;
        ALTER TABLE gov_approvals OWNER TO console_app;
        ALTER TABLE gov_approval_consumptions OWNER TO console_app;
        ALTER TABLE ont_object_types OWNER TO console_app;
        ALTER TABLE ont_property_defs OWNER TO console_app;
        ALTER TABLE ont_link_types OWNER TO console_app;
        ALTER TABLE ont_action_types OWNER TO console_app;
        ALTER TABLE ont_analytics OWNER TO console_app;

        -- Restore the exact broad runtime grants shipped by 0152. Without this,
        -- post-replay revocation assertions could pass because the first 0165
        -- application already removed the privileges.
        REVOKE ALL ON ont_object_types, ont_property_defs, ont_link_types,
            ont_action_types, ont_analytics FROM PUBLIC, console_rt,
            console_ontology_cmd, console_ontology_writer;
        GRANT SELECT, INSERT, UPDATE ON ont_object_types TO console_rt;
        GRANT SELECT, INSERT ON ont_property_defs, ont_link_types, ont_action_types, ont_analytics TO console_rt;
        "#,
    )
    .execute(pool)
    .await
    .expect("the fully migrated fixture must be reducible to the 0164 shape");

    let pre_0165 = sqlx::query(
        r#"
        SELECT
            has_table_privilege('console_rt', 'ont_object_types', 'INSERT,UPDATE') AS parent_write,
            has_table_privilege('console_rt', 'ont_property_defs', 'INSERT') AS property_write,
            has_table_privilege('console_rt', 'ont_link_types', 'INSERT') AS link_write,
            has_table_privilege('console_rt', 'ont_action_types', 'INSERT') AS action_write,
            has_table_privilege('console_rt', 'ont_analytics', 'INSERT') AS analytic_write
        "#,
    )
    .fetch_one(pool)
    .await
    .unwrap();
    for column in [
        "parent_write",
        "property_write",
        "link_write",
        "action_write",
        "analytic_write",
    ] {
        assert!(
            pre_0165.get::<bool, _>(column),
            "fixture must restore pre-0165 {column}"
        );
    }
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

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _metadata| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn seed_organization(pool: &PgPool, id: Uuid, slug: &str) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(slug)
        .bind(format!("Migration fixture {slug}"))
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_legacy_object_type(
    pool: &PgPool,
    org_id: Uuid,
    stable_key: &str,
    schema_version: i64,
    lifecycle_state: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO ont_object_types (
            org_id,
            stable_key,
            title,
            backing_kind,
            schema_version,
            lifecycle_state
        )
        VALUES ($1, $2, $3, 'instance', $4, $5)
        "#,
    )
    .bind(org_id)
    .bind(stable_key)
    .bind(format!("{stable_key} v{schema_version}"))
    .bind(schema_version)
    .bind(lifecycle_state)
    .execute(pool)
    .await
    .unwrap();
}

async fn apply_0165_as_migrator(pool: &PgPool) -> (String, String) {
    let migrator = login_role_pool(pool, "console_app", MIGRATOR_PASSWORD).await;
    let mut migration = migrator.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '5s'")
        .execute(&mut *migration)
        .await
        .unwrap();
    sqlx::query("SET LOCAL statement_timeout = '60s'")
        .execute(&mut *migration)
        .await
        .unwrap();
    let budgets: (String, String) = sqlx::query_as(
        "SELECT current_setting('lock_timeout'), current_setting('statement_timeout')",
    )
    .fetch_one(&mut *migration)
    .await
    .unwrap();
    sqlx::raw_sql(MIGRATION_0165)
        .execute(&mut *migration)
        .await
        .expect("the exact shipped 0165 migration must run within the rehearsal budgets");
    migration.commit().await.unwrap();
    budgets
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn migration_0165_upgrades_legacy_sibling_versions_without_tenant_leakage(pool: PgPool) {
    restore_pre_0165_schema(&pool).await;

    seed_organization(&pool, ORG_A, "migration-a165").await;
    seed_organization(&pool, ORG_B, "migration-b165").await;

    // A real pre-0165 shape: several immutable versions share one logical key,
    // while another key and another tenant require independent sidecars.
    seed_legacy_object_type(&pool, ORG_A, "ops.work_order", 1, "superseded").await;
    seed_legacy_object_type(&pool, ORG_A, "ops.work_order", 3, "superseded").await;
    seed_legacy_object_type(&pool, ORG_A, "ops.work_order", 7, "retired").await;
    seed_legacy_object_type(&pool, ORG_A, "ops.asset", 2, "retired").await;
    seed_legacy_object_type(&pool, ORG_B, "ops.work_order", 4, "retired").await;

    let migrator = login_role_pool(&pool, "console_app", MIGRATOR_PASSWORD).await;
    let migration_identity = sqlx::query("SELECT current_user, session_user")
        .fetch_one(&migrator)
        .await
        .unwrap();
    assert_eq!(
        migration_identity.get::<String, _>("current_user"),
        "console_app"
    );
    assert_eq!(
        migration_identity.get::<String, _>("session_user"),
        "console_app",
        "0165 must be exercised through a direct non-superuser migrator login"
    );
    sqlx::raw_sql(MIGRATION_0165)
        .execute(&migrator)
        .await
        .expect("the exact shipped 0165 migration must run as non-superuser console_app");

    let sidecars = sqlx::query(
        r#"
        SELECT org_id, stable_key, revision, validator_id
        FROM ont_object_type_key_revisions
        ORDER BY org_id, stable_key
        "#,
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(sidecars.len(), 3, "one sidecar must exist per tenant/key");

    let expected = [
        ((ORG_A, "ops.asset"), 2_i64),
        ((ORG_A, "ops.work_order"), 7_i64),
        ((ORG_B, "ops.work_order"), 4_i64),
    ];
    for ((org_id, stable_key), revision) in expected {
        let matches = sidecars
            .iter()
            .filter(|row| {
                row.get::<Uuid, _>("org_id") == org_id
                    && row.get::<String, _>("stable_key") == stable_key
            })
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1, "{org_id}/{stable_key} has one sidecar");
        assert_eq!(
            matches[0].get::<i64, _>("revision"),
            revision,
            "the legacy baseline must be MAX(schema_version)"
        );
    }

    let distinct_validators: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT validator_id) FROM ont_object_type_key_revisions",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        distinct_validators, 3,
        "every logical key has a unique validator"
    );
    let duplicate_validator = sidecars[0].get::<Uuid, _>("validator_id");
    let duplicate_validator_error = sqlx::query(
        r#"
        INSERT INTO ont_object_type_key_revisions (
            org_id, stable_key, validator_id, revision
        )
        VALUES ($1, 'ops.duplicate', $2, 1)
        "#,
    )
    .bind(ORG_A)
    .bind(duplicate_validator)
    .execute(&pool)
    .await
    .expect_err("validator identity must be database-enforced unique");
    assert_eq!(
        database_error_code(&duplicate_validator_error).as_deref(),
        Some("23505")
    );

    let foreign_key = sqlx::query(
        r#"
        SELECT convalidated, confdeltype = 'r' AS delete_restricted
        FROM pg_constraint
        WHERE conrelid = 'ont_object_types'::regclass
          AND conname = 'fk_ont_object_types_key_revision'
          AND contype = 'f'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("the logical-key foreign key must exist");
    assert!(foreign_key.get::<bool, _>("convalidated"));
    assert!(foreign_key.get::<bool, _>("delete_restricted"));

    let orphan_error = sqlx::query(
        r#"
        INSERT INTO ont_object_types (
            org_id, stable_key, title, backing_kind, schema_version, lifecycle_state
        )
        VALUES ($1, 'ops.orphan', 'Orphan', 'instance', 1, 'retired')
        "#,
    )
    .bind(ORG_A)
    .execute(&pool)
    .await
    .expect_err("object types without a logical-key sidecar must be rejected");
    assert_eq!(database_error_code(&orphan_error).as_deref(), Some("23503"));

    let rls = sqlx::query(
        r#"
        SELECT relrowsecurity, relforcerowsecurity
        FROM pg_class
        WHERE oid = 'ont_object_type_key_revisions'::regclass
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(rls.get::<bool, _>("relrowsecurity"));
    assert!(rls.get::<bool, _>("relforcerowsecurity"));

    let privileges = sqlx::query(
        r#"
        SELECT
            has_table_privilege('console_rt', 'ont_object_type_key_revisions', 'SELECT') AS can_select,
            has_table_privilege('console_rt', 'ont_object_type_key_revisions', 'INSERT') AS can_insert_all,
            has_table_privilege('console_rt', 'ont_object_type_key_revisions', 'UPDATE') AS can_update_all,
            has_table_privilege('console_rt', 'ont_object_type_key_revisions', 'DELETE') AS can_delete,
            has_table_privilege('console_rt', 'ont_object_type_key_revisions', 'TRUNCATE') AS can_truncate,
            has_column_privilege('console_rt', 'ont_object_type_key_revisions', 'org_id', 'INSERT') AS can_insert_org,
            has_column_privilege('console_rt', 'ont_object_type_key_revisions', 'stable_key', 'INSERT') AS can_insert_key,
            has_column_privilege('console_rt', 'ont_object_type_key_revisions', 'validator_id', 'INSERT') AS can_insert_validator,
            has_column_privilege('console_rt', 'ont_object_type_key_revisions', 'revision', 'UPDATE') AS can_update_revision,
            has_column_privilege('console_rt', 'ont_object_type_key_revisions', 'updated_at', 'UPDATE') AS can_update_timestamp,
            has_column_privilege('console_rt', 'ont_object_type_key_revisions', 'org_id', 'UPDATE') AS can_update_org,
            has_column_privilege('console_rt', 'ont_object_type_key_revisions', 'validator_id', 'UPDATE') AS can_update_validator
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(privileges.get::<bool, _>("can_select"));
    assert!(!privileges.get::<bool, _>("can_insert_all"));
    assert!(!privileges.get::<bool, _>("can_update_all"));
    assert!(!privileges.get::<bool, _>("can_delete"));
    assert!(!privileges.get::<bool, _>("can_truncate"));
    assert!(!privileges.get::<bool, _>("can_insert_org"));
    assert!(!privileges.get::<bool, _>("can_insert_key"));
    assert!(!privileges.get::<bool, _>("can_insert_validator"));
    assert!(!privileges.get::<bool, _>("can_update_revision"));
    assert!(!privileges.get::<bool, _>("can_update_timestamp"));
    assert!(!privileges.get::<bool, _>("can_update_org"));
    assert!(!privileges.get::<bool, _>("can_update_validator"));

    let guarded_boundary = sqlx::query(
        r#"
        SELECT
            has_table_privilege('console_rt', 'ont_object_types', 'INSERT,UPDATE') AS legacy_parent_write,
            has_table_privilege('console_rt', 'ont_property_defs', 'INSERT') AS legacy_property_write,
            has_table_privilege('console_rt', 'ont_link_types', 'INSERT') AS legacy_link_write,
            has_table_privilege('console_rt', 'ont_action_types', 'INSERT') AS legacy_action_write,
            has_table_privilege('console_rt', 'ont_analytics', 'INSERT') AS legacy_analytic_write,
            has_table_privilege('console_rt', 'ont_object_types', 'DELETE,TRUNCATE') AS destructive_parent_write,
            has_table_privilege('console_rt', 'ont_property_defs', 'UPDATE,DELETE,TRUNCATE') AS destructive_property_write,
            has_table_privilege('console_rt', 'ont_link_types', 'UPDATE,DELETE,TRUNCATE') AS destructive_link_write,
            has_table_privilege('console_rt', 'ont_action_types', 'UPDATE,DELETE,TRUNCATE') AS destructive_action_write,
            has_table_privilege('console_rt', 'ont_analytics', 'UPDATE,DELETE,TRUNCATE') AS destructive_analytic_write,
            has_schema_privilege('console_rt', 'ontology_api', 'USAGE') AS runtime_api_usage,
            has_schema_privilege('console_ontology_cmd', 'ontology_api', 'USAGE') AS command_api_usage,
            has_schema_privilege('public', 'ontology_api', 'USAGE') AS public_api_usage,
            has_function_privilege('console_rt', 'ontology_api.create_object_type(UUID, JSONB, UUID, TEXT, TEXT)', 'EXECUTE') AS runtime_can_create,
            has_function_privilege('console_ontology_cmd', 'ontology_api.create_object_type(UUID, JSONB, UUID, TEXT, TEXT)', 'EXECUTE') AS command_can_create,
            has_function_privilege('console_ontology_cmd', 'ontology_api.stage_object_type(UUID, TEXT, UUID, BIGINT, JSONB, UUID, TEXT, TEXT)', 'EXECUTE') AS command_can_stage,
            has_function_privilege('console_ontology_cmd', 'ontology_api.transition_object_type(UUID, UUID, UUID, BIGINT, TEXT, UUID, TEXT, TEXT)', 'EXECUTE') AS command_can_transition,
            has_function_privilege('console_ontology_cmd', 'ontology_api.install_builtin_catalog(UUID, TEXT, JSONB, UUID, TEXT, TEXT)', 'EXECUTE') AS command_can_install,
            has_function_privilege('console_ontology_cmd', 'ontology_api.insert_children(UUID, UUID, JSONB, BOOLEAN)', 'EXECUTE') AS command_can_call_helper,
            has_table_privilege('console_ontology_cmd', 'ont_object_types', 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE') AS command_parent_table_access,
            has_table_privilege('console_ontology_cmd', 'audit_events', 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE') AS command_audit_table_access
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    for column in [
        "legacy_parent_write",
        "legacy_property_write",
        "legacy_link_write",
        "legacy_action_write",
        "legacy_analytic_write",
    ] {
        assert!(
            guarded_boundary.get::<bool, _>(column),
            "0165 must retain the audited blue/green {column}"
        );
    }
    for column in [
        "destructive_parent_write",
        "destructive_property_write",
        "destructive_link_write",
        "destructive_action_write",
        "destructive_analytic_write",
    ] {
        assert!(
            !guarded_boundary.get::<bool, _>(column),
            "0165 must deny {column}"
        );
    }
    assert!(!guarded_boundary.get::<bool, _>("runtime_api_usage"));
    assert!(guarded_boundary.get::<bool, _>("command_api_usage"));
    assert!(!guarded_boundary.get::<bool, _>("public_api_usage"));
    assert!(!guarded_boundary.get::<bool, _>("runtime_can_create"));
    assert!(guarded_boundary.get::<bool, _>("command_can_create"));
    assert!(guarded_boundary.get::<bool, _>("command_can_stage"));
    assert!(guarded_boundary.get::<bool, _>("command_can_transition"));
    assert!(guarded_boundary.get::<bool, _>("command_can_install"));
    assert!(!guarded_boundary.get::<bool, _>("command_can_call_helper"));
    assert!(!guarded_boundary.get::<bool, _>("command_parent_table_access"));
    assert!(!guarded_boundary.get::<bool, _>("command_audit_table_access"));

    let owner = sqlx::query(
        "SELECT rolcanlogin, rolsuper, rolbypassrls, rolinherit, rolcreatedb, rolcreaterole, rolreplication FROM pg_roles WHERE rolname='console_ontology_writer'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    for column in [
        "rolcanlogin",
        "rolsuper",
        "rolbypassrls",
        "rolinherit",
        "rolcreatedb",
        "rolcreaterole",
        "rolreplication",
    ] {
        assert!(
            !owner.get::<bool, _>(column),
            "writer role must pin {column}=false"
        );
    }

    let command = sqlx::query(
        "SELECT rolcanlogin, rolsuper, rolbypassrls, rolinherit, rolcreatedb, rolcreaterole, rolreplication FROM pg_roles WHERE rolname='console_ontology_cmd'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        command.get::<bool, _>("rolcanlogin"),
        "migration must preserve out-of-band LOGIN"
    );
    for column in [
        "rolsuper",
        "rolbypassrls",
        "rolinherit",
        "rolcreatedb",
        "rolcreaterole",
        "rolreplication",
    ] {
        assert!(
            !command.get::<bool, _>(column),
            "command role must pin {column}=false"
        );
    }

    let migrator_role = sqlx::query(
        "SELECT rolcanlogin, rolinherit, rolsuper, rolbypassrls, rolcreatedb, rolcreaterole, rolreplication FROM pg_roles WHERE rolname='console_app'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(migrator_role.get::<bool, _>("rolcanlogin"));
    assert!(migrator_role.get::<bool, _>("rolinherit"));
    assert!(
        migrator_role.get::<bool, _>("rolbypassrls"),
        "the migration-only owner must cross FORCE RLS for multi-tenant upgrades"
    );
    for column in ["rolsuper", "rolcreatedb", "rolcreaterole", "rolreplication"] {
        assert!(
            !migrator_role.get::<bool, _>(column),
            "migration owner must pin {column}=false"
        );
    }

    let memberships = sqlx::query(
        r#"
        SELECT
            COUNT(*) FILTER (
                WHERE granted.rolname = 'console_ontology_writer'
                  AND member.rolname = 'console_app'
                  AND NOT am.admin_option
                  AND am.inherit_option
                  AND am.set_option
            ) AS exact_ontology_edge,
            COUNT(*) FILTER (
                WHERE granted.rolname = 'console_leave_definer'
                  AND member.rolname = 'console_app'
                  AND NOT am.admin_option
                  AND am.inherit_option
                  AND am.set_option
            ) AS exact_leave_edge,
            COUNT(*) FILTER (
                WHERE granted.rolname IN ('console_ontology_writer', 'console_leave_definer')
                   OR member.rolname IN ('console_ontology_writer', 'console_leave_definer')
                   OR granted.rolname IN ('console_rt', 'console_ontology_cmd')
                   OR member.rolname IN ('console_rt', 'console_ontology_cmd')
                   OR member.rolname = 'console_app'
            ) AS topology_edges,
            pg_has_role('console_app', 'console_ontology_writer', 'SET') AS migrator_can_set,
            pg_has_role('console_app', 'console_ontology_writer', 'USAGE') AS migrator_inherits,
            pg_has_role('console_app', 'console_leave_definer', 'SET') AS migrator_can_set_leave,
            pg_has_role('console_app', 'console_leave_definer', 'USAGE') AS migrator_inherits_leave,
            pg_has_role('console_rt', 'console_ontology_writer', 'SET') AS runtime_can_set,
            pg_has_role('console_ontology_cmd', 'console_ontology_writer', 'SET') AS command_can_set
        FROM pg_auth_members am
        JOIN pg_roles granted ON granted.oid = am.roleid
        JOIN pg_roles member ON member.oid = am.member
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(memberships.get::<i64, _>("exact_ontology_edge"), 1);
    assert_eq!(memberships.get::<i64, _>("exact_leave_edge"), 1);
    assert_eq!(
        memberships.get::<i64, _>("topology_edges"),
        2,
        "the two ownership-transfer edges must be the only application memberships"
    );
    assert!(memberships.get::<bool, _>("migrator_can_set"));
    assert!(memberships.get::<bool, _>("migrator_inherits"));
    assert!(memberships.get::<bool, _>("migrator_can_set_leave"));
    assert!(memberships.get::<bool, _>("migrator_inherits_leave"));
    assert!(!memberships.get::<bool, _>("runtime_can_set"));
    assert!(!memberships.get::<bool, _>("command_can_set"));

    let schema_acl = sqlx::query(
        r#"
        SELECT owner.rolname AS owner,
               ARRAY(
                   SELECT CASE WHEN acl.grantee = 0 THEN 'PUBLIC' ELSE grantee.rolname END
                          || ':' || acl.privilege_type
                   FROM aclexplode(n.nspacl) acl
                   LEFT JOIN pg_roles grantee ON grantee.oid = acl.grantee
                   ORDER BY 1
               ) AS acl
        FROM pg_namespace n
        JOIN pg_roles owner ON owner.oid = n.nspowner
        WHERE n.nspname = 'ontology_api'
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        schema_acl.get::<String, _>("owner"),
        "console_ontology_writer"
    );
    assert_eq!(
        schema_acl.get::<Vec<String>, _>("acl"),
        vec![
            "console_ontology_cmd:USAGE",
            "console_ontology_writer:CREATE",
            "console_ontology_writer:USAGE",
        ]
    );

    let table_owners = sqlx::query(
        r#"
        SELECT c.relname, owner.rolname AS owner,
               ARRAY(
                   SELECT CASE WHEN acl.grantee = 0 THEN 'PUBLIC' ELSE grantee.rolname END
                          || ':' || acl.privilege_type
                   FROM aclexplode(c.relacl) acl
                   LEFT JOIN pg_roles grantee ON grantee.oid = acl.grantee
                   WHERE acl.grantee IN (
                       0,
                       'console_rt'::regrole,
                       'console_ontology_cmd'::regrole,
                       'console_ontology_writer'::regrole
                   )
                   ORDER BY 1
               ) AS acl
        FROM pg_class c
        JOIN pg_roles owner ON owner.oid = c.relowner
        WHERE c.oid IN (
            'ont_object_types'::regclass,
            'ont_object_type_key_revisions'::regclass,
            'ont_property_defs'::regclass,
            'ont_link_types'::regclass,
            'ont_action_types'::regclass,
            'ont_analytics'::regclass,
            'ont_builtin_catalog_allowlist'::regclass,
            'ont_builtin_catalog_installs'::regclass
        )
        ORDER BY c.relname
        "#,
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(table_owners.len(), 8);
    for table in table_owners {
        let name = table.get::<String, _>("relname");
        assert_eq!(
            table.get::<String, _>("owner"),
            "console_app",
            "{} must remain migration-owned",
            name
        );
        let expected_acl = match name.as_str() {
            "ont_object_types" => vec![
                "console_ontology_writer:INSERT",
                "console_ontology_writer:SELECT",
                "console_ontology_writer:UPDATE",
                "console_rt:INSERT",
                "console_rt:SELECT",
                "console_rt:UPDATE",
            ],
            "ont_object_type_key_revisions" => vec![
                "console_ontology_writer:INSERT",
                "console_ontology_writer:SELECT",
                "console_ontology_writer:UPDATE",
                "console_rt:SELECT",
            ],
            "ont_builtin_catalog_allowlist" => vec!["console_ontology_writer:SELECT"],
            "ont_property_defs" | "ont_link_types" | "ont_action_types" | "ont_analytics" => vec![
                "console_ontology_writer:INSERT",
                "console_ontology_writer:SELECT",
                "console_rt:INSERT",
                "console_rt:SELECT",
            ],
            _ => vec![
                "console_ontology_writer:INSERT",
                "console_ontology_writer:SELECT",
                "console_rt:SELECT",
            ],
        };
        assert_eq!(
            table.get::<Vec<String>, _>("acl"),
            expected_acl,
            "{name} must have the exact runtime/writer ACL"
        );
    }

    let routines = sqlx::query(
        r#"
        SELECT p.proname, r.rolname AS owner, p.prosecdef, p.proconfig,
               ARRAY(
                   SELECT CASE WHEN acl.grantee = 0 THEN 'PUBLIC' ELSE grantee.rolname END
                          || ':' || acl.privilege_type
                   FROM aclexplode(p.proacl) acl
                   LEFT JOIN pg_roles grantee ON grantee.oid = acl.grantee
                   ORDER BY 1
               ) AS acl
        FROM pg_proc p
        JOIN pg_namespace n ON n.oid=p.pronamespace
        JOIN pg_roles r ON r.oid=p.proowner
        WHERE n.nspname='ontology_api'
        ORDER BY p.proname
        "#,
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(routines.len(), 11);
    for routine in routines {
        assert_eq!(routine.get::<String, _>("owner"), "console_ontology_writer");
        let name = routine.get::<String, _>("proname");
        assert!(
            routine.get::<bool, _>("prosecdef") || name == "invoker_role",
            "{name} must be SECURITY DEFINER unless it is the role-inspection helper"
        );
        let config = routine.get::<Vec<String>, _>("proconfig");
        let expected = if name == "invoker_role" {
            vec!["search_path=pg_catalog"]
        } else {
            vec!["search_path=pg_catalog", "row_security=on"]
        };
        assert_eq!(config, expected, "{name} must have exact safe proconfig");
        let acl = routine.get::<Vec<String>, _>("acl");
        let expected_acl = if matches!(
            name.as_str(),
            "create_object_type"
                | "stage_object_type"
                | "transition_object_type"
                | "install_builtin_catalog"
        ) {
            vec![
                "console_ontology_cmd:EXECUTE",
                "console_ontology_writer:EXECUTE",
            ]
        } else {
            vec!["console_ontology_writer:EXECUTE"]
        };
        assert_eq!(acl, expected_acl, "{name} must have the exact execute ACL");
    }

    let runtime = runtime_role_pool(&pool).await;
    let mut transaction = runtime.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(&mut *transaction)
        .await
        .unwrap();
    let visible = sqlx::query(
        "SELECT org_id, stable_key FROM ont_object_type_key_revisions ORDER BY stable_key",
    )
    .fetch_all(&mut *transaction)
    .await
    .unwrap();
    assert_eq!(
        visible.len(),
        2,
        "console_rt sees only the armed tenant's keys"
    );
    assert!(
        visible
            .iter()
            .all(|row| row.get::<Uuid, _>("org_id") == ORG_A)
    );
    assert!(
        visible
            .iter()
            .all(|row| row.get::<String, _>("stable_key") != "ops.orphan")
    );
    transaction.rollback().await.unwrap();

    let runtime_dml_error = sqlx::query(
        "INSERT INTO ont_object_type_key_revisions (org_id, stable_key) VALUES ($1, 'ops.runtime_bypass')",
    )
    .bind(ORG_A)
    .execute(&runtime)
    .await
    .expect_err("console_rt must not have a direct key-revision write path");
    assert_eq!(
        database_error_code(&runtime_dml_error).as_deref(),
        Some("42501")
    );

    let command_pool = login_role_pool(&pool, "console_ontology_cmd", COMMAND_PASSWORD).await;
    let command_dml_error = sqlx::query(
        "INSERT INTO ont_object_type_key_revisions (org_id, stable_key) VALUES ($1, 'ops.command_bypass')",
    )
    .bind(ORG_A)
    .execute(&command_pool)
    .await
    .expect_err("console_ontology_cmd must be limited to the audited command routines");
    assert_eq!(
        database_error_code(&command_dml_error).as_deref(),
        Some("42501")
    );
    let command_set_error = sqlx::query("SET ROLE console_ontology_writer")
        .execute(&command_pool)
        .await
        .expect_err("the command login must not be able to impersonate the writer");
    assert_eq!(
        database_error_code(&command_set_error).as_deref(),
        Some("42501")
    );

    // A replay reaches both startup's exact application-role preflight and the
    // migration topology assertion before any non-idempotent DDL. Create an
    // unexpected LOGIN that can SET ROLE to console_app transactionally; rollback
    // removes the cluster-global hostile role for sibling tests.
    let mut drift = pool.begin().await.unwrap();
    sqlx::query(
        r#"
        CREATE ROLE console_0165_hostile_login
            LOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOREPLICATION
        "#,
    )
    .execute(&mut *drift)
    .await
    .unwrap();
    sqlx::query(
        "GRANT console_app TO console_0165_hostile_login WITH ADMIN FALSE, INHERIT FALSE, SET TRUE",
    )
    .execute(&mut *drift)
    .await
    .unwrap();

    let application_preflight_detects_drift: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM pg_catalog.pg_auth_members AS membership
            JOIN pg_catalog.pg_roles AS granted ON granted.oid = membership.roleid
            JOIN pg_catalog.pg_roles AS member ON member.oid = membership.member
            WHERE (
                granted.rolname IN (
                    'console_app', 'console_rt', 'console_leave_definer', 'console_leave_cmd',
                    'console_ontology_writer', 'console_ontology_cmd'
                )
                OR member.rolname IN (
                    'console_app', 'console_rt', 'console_leave_definer', 'console_leave_cmd',
                    'console_ontology_writer', 'console_ontology_cmd'
                )
            )
            AND NOT (
                member.rolname = 'console_app'
                AND granted.rolname IN ('console_leave_definer', 'console_ontology_writer')
                AND NOT membership.admin_option
                AND membership.inherit_option
                AND membership.set_option
            )
        )
        "#,
    )
    .fetch_one(&mut *drift)
    .await
    .unwrap();
    assert!(
        application_preflight_detects_drift,
        "the application migration preflight must reject an incoming console_app edge"
    );

    let drift_error = sqlx::raw_sql(MIGRATION_0165)
        .execute(&mut *drift)
        .await
        .expect_err("0165 replay must reject ownership-membership drift before DDL");
    assert_eq!(database_error_code(&drift_error).as_deref(), Some("42501"));
    assert!(
        drift_error
            .as_database_error()
            .is_some_and(|error| error.message() == "ontology_role_topology.membership_drift")
    );
    drift.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn migration_0165_keeps_exact_old_binary_writes_audited_and_cas_consistent(pool: PgPool) {
    restore_pre_0165_schema(&pool).await;
    seed_organization(&pool, LEGACY_ORG, "migration-legacy-a165").await;
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, 'Legacy writer', ARRAY['SUPER_ADMIN'], $2)",
    )
    .bind(LEGACY_ACTOR)
    .bind(LEGACY_ORG)
    .execute(&pool)
    .await
    .unwrap();
    apply_0165_as_migrator(&pool).await;

    let runtime = runtime_role_pool(&pool).await;
    let first_id = Uuid::from_u128(0xc165_0000_0000_0000_0000_0000_0000_0001);
    let second_id = Uuid::from_u128(0xc165_0000_0000_0000_0000_0000_0000_0002);
    let first_at = time::macros::datetime!(2026-07-19 14:00 UTC);
    let second_at = time::macros::datetime!(2026-07-19 14:01 UTC);
    let transition_at = time::macros::datetime!(2026-07-19 14:02 UTC);

    // Exact retained-binary create shape: parent, append-only children, then
    // the with_audit row in the same transaction.
    let mut create_tx = runtime.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(LEGACY_ORG.to_string())
        .execute(&mut *create_tx)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO ont_object_types
           (id,org_id,stable_key,title,backing_kind,schema_version,lifecycle_state,created_by,created_at,updated_at)
           VALUES ($1,$2,'legacy.compat','Legacy v1','instance',1,'draft',$3,$4,$4)"#,
    )
    .bind(first_id)
    .bind(LEGACY_ORG)
    .bind(LEGACY_ACTOR)
    .bind(first_at)
    .execute(&mut *create_tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ont_property_defs (org_id,object_type_id,key,title,type,required) VALUES ($1,$2,'name','Name','text',true)",
    )
    .bind(LEGACY_ORG)
    .bind(first_id)
    .execute(&mut *create_tx)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO audit_events
           (id,actor,action,target_type,target_id,trace_id,span_id,occurred_at,org_id)
           VALUES (gen_random_uuid(),$1,'ontology.object_type.create','ont_object_types',$2,
                   '0123456789abcdef0123456789abcdef','0123456789abcdef',$3,$4)"#,
    )
    .bind(LEGACY_ACTOR)
    .bind(first_id.to_string())
    .bind(first_at)
    .bind(LEGACY_ORG)
    .execute(&mut *create_tx)
    .await
    .unwrap();
    create_tx.commit().await.unwrap();

    for (state, occurred_at, trace_prefix) in [
        (
            "review_pending",
            time::macros::datetime!(2026-07-19 14:00:20 UTC),
            '3',
        ),
        (
            "published",
            time::macros::datetime!(2026-07-19 14:00:40 UTC),
            '4',
        ),
    ] {
        let mut tx = runtime.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(LEGACY_ORG.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        if state == "published" {
            // The old binary adds the generic create action before publishing
            // an instance-backed type. The deferred child guard permits it
            // only because this same transaction later records the transition.
            sqlx::query(
                r#"INSERT INTO ont_action_types
                   (org_id,object_type_id,stable_key,title,dispatch)
                   VALUES ($1,$2,'create','Create','instance_revision')"#,
            )
            .bind(LEGACY_ORG)
            .bind(first_id)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        sqlx::query("UPDATE ont_object_types SET lifecycle_state=$2, updated_at=$3 WHERE id=$1")
            .bind(first_id)
            .bind(state)
            .bind(occurred_at)
            .execute(&mut *tx)
            .await
            .unwrap();
        let trace_id = format!("{trace_prefix}123456789abcdef0123456789abcdef");
        let span_id = format!("{trace_prefix}123456789abcdef");
        sqlx::query(
            r#"INSERT INTO audit_events
               (id,actor,action,target_type,target_id,trace_id,span_id,occurred_at,org_id)
               VALUES (gen_random_uuid(),$1,'ontology.object_type.transition','ont_object_types',$2,$3,$4,$5,$6)"#,
        )
        .bind(LEGACY_ACTOR)
        .bind(first_id.to_string())
        .bind(trace_id)
        .bind(span_id)
        .bind(occurred_at)
        .bind(LEGACY_ORG)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    // Exact retained-binary stage shape. The compatibility audit trigger, not
    // console_rt, owns the one-and-only CAS-sidecar advance.
    let mut stage_tx = runtime.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(LEGACY_ORG.to_string())
        .execute(&mut *stage_tx)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO ont_object_types
           (id,org_id,stable_key,title,backing_kind,schema_version,lifecycle_state,created_by,created_at,updated_at)
           VALUES ($1,$2,'legacy.compat','Legacy v2','instance',2,'draft',$3,$4,$4)"#,
    )
    .bind(second_id)
    .bind(LEGACY_ORG)
    .bind(LEGACY_ACTOR)
    .bind(second_at)
    .execute(&mut *stage_tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ont_property_defs (org_id,object_type_id,key,title,type,required) VALUES ($1,$2,'name','Name','text',true)",
    )
    .bind(LEGACY_ORG)
    .bind(second_id)
    .execute(&mut *stage_tx)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO audit_events
           (id,actor,action,target_type,target_id,trace_id,span_id,occurred_at,org_id)
           VALUES (gen_random_uuid(),$1,'ontology.object_type.stage_revision','ont_object_types',$2,
                   '1123456789abcdef0123456789abcdef','1123456789abcdef',$3,$4)"#,
    )
    .bind(LEGACY_ACTOR)
    .bind(second_id.to_string())
    .bind(second_at)
    .bind(LEGACY_ORG)
    .execute(&mut *stage_tx)
    .await
    .unwrap();
    stage_tx.commit().await.unwrap();

    let mut transition_tx = runtime.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(LEGACY_ORG.to_string())
        .execute(&mut *transition_tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE ont_object_types SET lifecycle_state='review_pending', updated_at=$2 WHERE id=$1",
    )
    .bind(second_id)
    .bind(transition_at)
    .execute(&mut *transition_tx)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO audit_events
           (id,actor,action,target_type,target_id,trace_id,span_id,occurred_at,org_id)
           VALUES (gen_random_uuid(),$1,'ontology.object_type.transition','ont_object_types',$2,
                   '2123456789abcdef0123456789abcdef','2123456789abcdef',$3,$4)"#,
    )
    .bind(LEGACY_ACTOR)
    .bind(second_id.to_string())
    .bind(transition_at)
    .bind(LEGACY_ORG)
    .execute(&mut *transition_tx)
    .await
    .unwrap();
    transition_tx.commit().await.unwrap();

    let state = sqlx::query(
        r#"SELECT k.revision,
                  (SELECT COUNT(*) FROM ont_object_types o WHERE o.org_id=k.org_id AND o.stable_key=k.stable_key) AS versions,
                  (SELECT COUNT(*) FROM ont_property_defs p WHERE p.org_id=k.org_id) AS properties,
                  (SELECT COUNT(*) FROM audit_events e WHERE e.org_id=k.org_id AND e.action LIKE 'ontology.object_type.%') AS audits
           FROM ont_object_type_key_revisions k
           WHERE k.org_id=$1 AND k.stable_key='legacy.compat'"#,
    )
    .bind(LEGACY_ORG)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state.get::<i64, _>("revision"), 5);
    assert_eq!(state.get::<i64, _>("versions"), 2);
    assert_eq!(state.get::<i64, _>("properties"), 2);
    assert_eq!(state.get::<i64, _>("audits"), 5);

    // A direct content edit is outside the compatibility contract.
    let mut content_tx = runtime.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(LEGACY_ORG.to_string())
        .execute(&mut *content_tx)
        .await
        .unwrap();
    let content_error = sqlx::query("UPDATE ont_object_types SET title='forged' WHERE id=$1")
        .bind(second_id)
        .execute(&mut *content_tx)
        .await
        .expect_err("legacy authority must be lifecycle-only on existing parents");
    assert_eq!(
        database_error_code(&content_error).as_deref(),
        Some("42501")
    );
    content_tx.rollback().await.unwrap();

    // Even a structurally valid lifecycle update is rolled back at COMMIT when
    // the same transaction did not append its protected audit fact.
    let mut unaudited_tx = runtime.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(LEGACY_ORG.to_string())
        .execute(&mut *unaudited_tx)
        .await
        .unwrap();
    sqlx::query("UPDATE ont_object_types SET lifecycle_state='draft', updated_at=statement_timestamp() WHERE id=$1")
        .bind(second_id)
        .execute(&mut *unaudited_tx)
        .await
        .unwrap();
    let commit_error = unaudited_tx
        .commit()
        .await
        .expect_err("unaudited compatibility writes must fail closed at commit");
    assert_eq!(database_error_code(&commit_error).as_deref(), Some("23514"));

    let final_row = sqlx::query("SELECT title,lifecycle_state FROM ont_object_types WHERE id=$1")
        .bind(second_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(final_row.get::<String, _>("title"), "Legacy v2");
    assert_eq!(
        final_row.get::<String, _>("lifecycle_state"),
        "review_pending"
    );

    // One legacy mutation must correlate to exactly one audit fact. Two
    // matching rows in the same transaction would otherwise advance the
    // key-revision sidecar twice and make the compatibility path diverge from
    // the new command CAS contract.
    let revision_before_duplicate: i64 = sqlx::query_scalar(
        "SELECT revision FROM ont_object_type_key_revisions WHERE org_id=$1 AND stable_key='legacy.compat'",
    )
    .bind(LEGACY_ORG)
    .fetch_one(&pool)
    .await
    .unwrap();
    let duplicate_at = time::macros::datetime!(2026-07-19 14:03 UTC);
    let mut duplicate_tx = runtime.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(LEGACY_ORG.to_string())
        .execute(&mut *duplicate_tx)
        .await
        .unwrap();
    sqlx::query("UPDATE ont_object_types SET lifecycle_state='draft', updated_at=$2 WHERE id=$1")
        .bind(second_id)
        .bind(duplicate_at)
        .execute(&mut *duplicate_tx)
        .await
        .unwrap();
    for suffix in ['5', '6'] {
        let trace_id = format!("{suffix}123456789abcdef0123456789abcdef");
        let span_id = format!("{suffix}123456789abcdef");
        sqlx::query(
            r#"INSERT INTO audit_events
               (id,actor,action,target_type,target_id,trace_id,span_id,occurred_at,org_id)
               VALUES (gen_random_uuid(),$1,'ontology.object_type.transition','ont_object_types',$2,$3,$4,$5,$6)"#,
        )
        .bind(LEGACY_ACTOR)
        .bind(second_id.to_string())
        .bind(trace_id)
        .bind(span_id)
        .bind(duplicate_at)
        .bind(LEGACY_ORG)
        .execute(&mut *duplicate_tx)
        .await
        .unwrap();
    }
    let duplicate_error = duplicate_tx
        .commit()
        .await
        .expect_err("duplicate compatibility audits must fail closed at commit");
    assert_eq!(
        database_error_code(&duplicate_error).as_deref(),
        Some("23514")
    );

    let state_after_duplicate = sqlx::query(
        r#"SELECT o.lifecycle_state, k.revision
           FROM ont_object_types o
           JOIN ont_object_type_key_revisions k
             ON k.org_id=o.org_id AND k.stable_key=o.stable_key
           WHERE o.id=$1"#,
    )
    .bind(second_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        state_after_duplicate.get::<String, _>("lifecycle_state"),
        "review_pending"
    );
    assert_eq!(
        state_after_duplicate.get::<i64, _>("revision"),
        revision_before_duplicate
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn migration_0165_rehearses_populated_expand_with_bounded_lock_and_statement_timeouts(
    pool: PgPool,
) {
    restore_pre_0165_schema(&pool).await;
    seed_organization(&pool, ORG_A, "migration-rehearsal-a165").await;
    seed_legacy_object_type(&pool, ORG_A, "ops.work_order", 2, "superseded").await;
    seed_legacy_object_type(&pool, ORG_A, "ops.work_order", 9, "retired").await;

    let budgets = apply_0165_as_migrator(&pool).await;
    assert_eq!(
        budgets,
        (
            MIGRATION_LOCK_TIMEOUT.into(),
            MIGRATION_STATEMENT_TIMEOUT.into()
        )
    );

    let sidecar: (i64, i64) = sqlx::query_as(
        "SELECT count(*), max(revision) FROM ont_object_type_key_revisions \
         WHERE org_id=$1 AND stable_key='ops.work_order'",
    )
    .bind(ORG_A)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(sidecar, (1, 9));
}
