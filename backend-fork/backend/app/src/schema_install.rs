//! Fresh schema installation and checksum-preserving historical upgrades.
//! The caller holds SQLx's database lock on a dedicated owner connection.

use std::borrow::Cow;

use sqlx::PgConnection;
use sqlx::migrate::{Migrate, Migrator};

use crate::{AppError, MIGRATOR};

static BASELINE: Migrator = sqlx::migrate!("../crates/platform/db/baseline");
const BASELINE_THROUGH: i64 = 225;

pub(super) async fn run(connection: &mut PgConnection) -> Result<(), AppError> {
    let exists: bool =
        sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
            .fetch_one(&mut *connection)
            .await?;
    if !exists {
        require_empty_schema(connection).await?;
    }
    connection
        .ensure_migrations_table(&MIGRATOR.table_name)
        .await
        .map_err(migration_error)?;
    protect_ledger(connection).await?;

    let applied = connection
        .list_applied_migrations(&MIGRATOR.table_name)
        .await
        .map_err(migration_error)?;
    if applied.is_empty() {
        require_empty_schema(connection).await?;
    }
    let fresh = applied.first().is_none_or(|entry| entry.version == 0);
    let migrations = if fresh {
        BASELINE
            .iter()
            .chain(
                MIGRATOR
                    .iter()
                    .filter(|migration| migration.version > BASELINE_THROUGH),
            )
            .cloned()
            .collect()
    } else {
        MIGRATOR.iter().cloned().collect()
    };
    let migrator = Migrator {
        migrations: Cow::Owned(migrations),
        // The caller retains the same SQLx lock through Apalis reconciliation.
        locking: false,
        ..Migrator::DEFAULT
    };

    // SQLx itself accepts gaps and checks later checksums only after applying
    // earlier missing entries. Prevalidate the entire prefix before any SQL.
    if applied.len() > migrator.iter().len()
        || applied
            .iter()
            .zip(migrator.iter())
            .any(|(actual, expected)| {
                actual.version != expected.version || actual.checksum != expected.checksum
            })
    {
        return Err(AppError::Config(
            "migration ledger is not an unchanged contiguous prefix of its install lineage".into(),
        ));
    }
    tracing::info!(fresh_baseline = fresh, "validated schema install lineage");
    migrator
        .run(&mut *connection)
        .await
        .map_err(migration_error)
}

fn migration_error(error: sqlx::migrate::MigrateError) -> AppError {
    AppError::Internal(format!("migration run failed: {error}"))
}

async fn protect_ledger(connection: &mut PgConnection) -> Result<(), AppError> {
    let owned: bool = sqlx::query_scalar(
        "SELECT relkind = 'r' AND relowner = current_user::regrole
         FROM pg_class WHERE oid = 'public._sqlx_migrations'::regclass",
    )
    .fetch_one(&mut *connection)
    .await?;
    if !owned {
        return Err(AppError::Config(
            "migration ledger must be owned by the migration role".into(),
        ));
    }
    // Creation can inherit table defaults. Runtime credentials must never be
    // able to erase the baseline entry and make an old binary replay seeds.
    sqlx::raw_sql(
        "REVOKE ALL ON public._sqlx_migrations FROM PUBLIC, console_rt,
         console_leave_cmd, console_ontology_cmd, console_platform_force_cmd,
         console_leave_definer, console_ontology_writer;
         REVOKE ALL (version, description, installed_on, success, checksum, execution_time)
         ON public._sqlx_migrations FROM PUBLIC, console_rt,
         console_leave_cmd, console_ontology_cmd, console_platform_force_cmd,
         console_leave_definer, console_ontology_writer",
    )
    .execute(&mut *connection)
    .await?;
    let private: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS (
            SELECT 1 FROM pg_class c,
            LATERAL aclexplode(coalesce(c.relacl, acldefault('r', c.relowner))) a
            WHERE c.oid = 'public._sqlx_migrations'::regclass AND a.grantee <> c.relowner
            UNION ALL
            SELECT 1 FROM pg_attribute column_acl
            JOIN pg_class c ON c.oid = column_acl.attrelid,
            LATERAL aclexplode(column_acl.attacl) a
            WHERE c.oid = 'public._sqlx_migrations'::regclass AND a.grantee <> c.relowner
        )",
    )
    .fetch_one(&mut *connection)
    .await?;
    if !private {
        return Err(AppError::Config(
            "unexpected non-owner migration-ledger grant".into(),
        ));
    }
    Ok(())
}

async fn require_empty_schema(connection: &mut PgConnection) -> Result<(), AppError> {
    // SQLx can leave an empty ledger after an interrupted transactional apply.
    // Only that table, its index and its row/array types are safe retry artifacts.
    let empty: bool = sqlx::query_scalar(
        "WITH ledger AS (SELECT to_regclass('public._sqlx_migrations') AS id),
         user_schemas AS (
             SELECT oid FROM pg_namespace
             WHERE NOT starts_with(nspname, 'pg_') AND nspname <> 'information_schema'
         )
         SELECT NOT (
             EXISTS (SELECT 1 FROM pg_namespace WHERE NOT starts_with(nspname, 'pg_')
                     AND nspname NOT IN ('public', 'information_schema'))
             OR EXISTS (SELECT 1 FROM pg_class c WHERE c.relnamespace IN (SELECT oid FROM user_schemas)
                        AND c.oid IS DISTINCT FROM (SELECT id FROM ledger)
                        AND NOT EXISTS (SELECT 1 FROM pg_index i
                                        WHERE i.indexrelid = c.oid AND i.indrelid = (SELECT id FROM ledger)))
             OR EXISTS (SELECT 1 FROM pg_proc WHERE pronamespace IN (SELECT oid FROM user_schemas))
             OR EXISTS (SELECT 1 FROM pg_type t WHERE t.typnamespace IN (SELECT oid FROM user_schemas)
                        AND t.typrelid IS DISTINCT FROM (SELECT id FROM ledger)
                        AND NOT EXISTS (SELECT 1 FROM pg_type row_type WHERE row_type.oid = t.typelem
                                        AND row_type.typrelid = (SELECT id FROM ledger)))
             OR EXISTS (SELECT 1 FROM pg_extension WHERE extname <> 'plpgsql')
             OR EXISTS (SELECT 1 FROM pg_collation WHERE collnamespace IN (SELECT oid FROM user_schemas))
             OR EXISTS (SELECT 1 FROM pg_operator WHERE oprnamespace IN (SELECT oid FROM user_schemas))
             OR EXISTS (SELECT 1 FROM pg_conversion WHERE connamespace IN (SELECT oid FROM user_schemas))
             OR EXISTS (SELECT 1 FROM pg_ts_config WHERE cfgnamespace IN (SELECT oid FROM user_schemas))
             OR EXISTS (SELECT 1 FROM pg_ts_dict WHERE dictnamespace IN (SELECT oid FROM user_schemas))
             OR EXISTS (SELECT 1 FROM pg_ts_parser WHERE prsnamespace IN (SELECT oid FROM user_schemas))
             OR EXISTS (SELECT 1 FROM pg_ts_template WHERE tmplnamespace IN (SELECT oid FROM user_schemas))
             OR EXISTS (SELECT 1 FROM pg_largeobject_metadata)
             OR EXISTS (SELECT 1 FROM pg_event_trigger)
             OR EXISTS (SELECT 1 FROM pg_foreign_server)
             OR EXISTS (SELECT 1 FROM pg_foreign_data_wrapper)
             OR EXISTS (SELECT 1 FROM pg_publication)
             OR EXISTS (
                 SELECT 1 FROM pg_default_acl d
                 WHERE NOT (
                     d.defaclrole = 'console_app'::regrole
                     AND d.defaclnamespace = 'public'::regnamespace
                     AND d.defaclobjtype = 'r'
                     AND (SELECT count(*) FROM aclexplode(d.defaclacl)) = 4
                     AND NOT EXISTS (
                         SELECT 1 FROM aclexplode(d.defaclacl) a
                         WHERE a.grantee <> 'console_rt'::regrole OR a.is_grantable
                            OR a.privilege_type NOT IN ('SELECT', 'INSERT', 'UPDATE', 'DELETE')
                     )
                 )
             )
         )",
    )
    .fetch_one(&mut *connection)
    .await?;
    if !empty {
        return Err(AppError::Config(
            "refusing fresh installation into an occupied database without a proven migration lineage".into(),
        ));
    }
    Ok(())
}
