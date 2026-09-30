#![allow(clippy::panic)]

use console_gate_tenant_isolation::owner_only_table_allowlist;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgConnection, PgPool};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const OWNER_ONLY_PROBE: &str = "public.ont_builtin_catalog_allowlist";

async fn effective_runtime_privileges(
    connection: &mut PgConnection,
    relation: &str,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT 'table:' || privilege
        FROM unnest(ARRAY[
            'SELECT', 'INSERT', 'UPDATE', 'DELETE', 'TRUNCATE',
            'REFERENCES', 'TRIGGER', 'MAINTAIN'
        ]::text[]) AS candidate(privilege)
        WHERE has_table_privilege('console_rt', $1, privilege)
        UNION ALL
        SELECT 'column:' || privilege
        FROM unnest(ARRAY['SELECT', 'INSERT', 'UPDATE', 'REFERENCES']::text[])
            AS candidate(privilege)
        WHERE has_any_column_privilege('console_rt', $1, privilege)
        ORDER BY 1
        "#,
    )
    .bind(relation)
    .fetch_all(connection)
    .await
}

async fn case_fold_equivalent_relations(
    connection: &mut PgConnection,
    canonical_name: &str,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT namespace.nspname::text, relation.relname::text
        FROM pg_catalog.pg_class AS relation
        JOIN pg_catalog.pg_namespace AS namespace
          ON namespace.oid = relation.relnamespace
        WHERE relation.relkind IN ('r', 'p', 'v', 'm', 'f')
          AND lower(relation.relname) = lower($1)
          AND namespace.nspname = 'public'
        ORDER BY namespace.nspname, relation.relname
        "#,
    )
    .bind(canonical_name)
    .fetch_all(connection)
    .await
}

async fn settable_runtime_roles(connection: &mut PgConnection) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT candidate.rolname::text
        FROM pg_catalog.pg_roles AS candidate
        WHERE candidate.rolname <> 'console_rt'
          AND pg_catalog.pg_has_role('console_rt', candidate.oid, 'SET')
        ORDER BY candidate.rolname
        "#,
    )
    .fetch_all(connection)
    .await
}

async fn assert_acl_mutation_is_detected(
    pool: &PgPool,
    label: &str,
    statements: &[&'static str],
) -> TestResult {
    let mut transaction = pool.begin().await?;
    for statement in statements {
        sqlx::query(*statement).execute(&mut *transaction).await?;
    }

    let privileges = effective_runtime_privileges(&mut transaction, OWNER_ONLY_PROBE).await?;
    assert!(
        !privileges.is_empty(),
        "{label} must produce effective console_rt access"
    );
    transaction.rollback().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL 18 migrated directly as console_app"]
async fn owner_only_acl_is_effectively_private_on_postgres18() -> TestResult {
    let database_url = std::env::var("DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await?;

    let current_user: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        current_user, "console_app",
        "ACL proof must use the production migration owner"
    );

    let server_version: String = sqlx::query_scalar("SHOW server_version_num")
        .fetch_one(&pool)
        .await?;
    let server_version: u32 = server_version.parse()?;
    assert!(
        (180_000..190_000).contains(&server_version),
        "ACL proof requires PostgreSQL 18, got server_version_num={server_version}"
    );

    let mut connection = pool.acquire().await?;
    let settable_roles = settable_runtime_roles(&mut connection).await?;
    assert!(
        settable_roles.is_empty(),
        "console_rt can SET ROLE to privilege-bearing identities: {settable_roles:?}"
    );

    for &(table, _) in owner_only_table_allowlist() {
        let relations = case_fold_equivalent_relations(&mut connection, table).await?;
        assert_eq!(
            relations,
            vec![("public".to_string(), table.to_string())],
            "owner-only table must have one canonical public relation and no case-distinct shadow"
        );

        let relation = format!("public.{table}");
        let privileges = effective_runtime_privileges(&mut connection, &relation).await?;
        assert!(
            privileges.is_empty(),
            "console_rt has effective access to owner-only {relation}: {privileges:?}"
        );
    }
    drop(connection);

    for (label, statements) in [
        (
            "mixed runtime grantee",
            &["GRANT SELECT ON public.ont_builtin_catalog_allowlist TO console_app, console_rt"]
                as &[&str],
        ),
        (
            "PUBLIC grantee",
            &["GRANT SELECT ON public.ont_builtin_catalog_allowlist TO console_app, PUBLIC"],
        ),
        (
            "Unicode-escaped runtime grantee",
            &[r#"GRANT SELECT ON public.ont_builtin_catalog_allowlist TO U&"console\005frt""#],
        ),
        (
            "Unicode-escaped owner-only target",
            &[
                r#"GRANT SELECT ON public.U&"ont\005fbuiltin\005fcatalog\005fallowlist" TO console_rt"#,
            ],
        ),
        (
            "quoted-semicolon column privilege",
            &[
                r#"ALTER TABLE public.ont_builtin_catalog_allowlist ADD COLUMN "x;y" text"#,
                r#"GRANT SELECT ("x;y") ON public.ont_builtin_catalog_allowlist TO console_rt"#,
            ],
        ),
        (
            "schema-wide table privilege",
            &["GRANT SELECT ON ALL TABLES IN SCHEMA public TO console_rt"],
        ),
    ] {
        assert_acl_mutation_is_detected(&pool, label, statements).await?;
    }

    let mut default_acl = pool.begin().await?;
    sqlx::query(
        "ALTER DEFAULT PRIVILEGES FOR ROLE console_app IN SCHEMA public \
         REVOKE ALL PRIVILEGES ON TABLES FROM console_rt, PUBLIC",
    )
    .execute(&mut *default_acl)
    .await?;
    sqlx::query(
        "ALTER DEFAULT PRIVILEGES FOR ROLE console_app IN SCHEMA public \
         GRANT SELECT ON TABLES TO console_rt",
    )
    .execute(&mut *default_acl)
    .await?;
    sqlx::query("CREATE TABLE public.console_owner_only_default_acl_probe (id bigint)")
        .execute(&mut *default_acl)
        .await?;
    let inherited = effective_runtime_privileges(
        &mut default_acl,
        "public.console_owner_only_default_acl_probe",
    )
    .await?;
    assert!(
        inherited
            .iter()
            .any(|privilege| privilege == "table:SELECT"),
        "default-privilege mutation was not observable: {inherited:?}"
    );
    default_acl.rollback().await?;

    let mut shadow = pool.begin().await?;
    sqlx::query(
        r#"CREATE VIEW public."ONT_BUILTIN_CATALOG_ALLOWLIST" AS
           SELECT catalog_version, manifest_digest
           FROM public.ont_builtin_catalog_allowlist"#,
    )
    .execute(&mut *shadow)
    .await?;
    let shadows =
        case_fold_equivalent_relations(&mut shadow, "ont_builtin_catalog_allowlist").await?;
    assert_eq!(
        shadows.len(),
        2,
        "case-distinct public owner-only view shadow must be observable: {shadows:?}"
    );
    shadow.rollback().await?;

    Ok(())
}
