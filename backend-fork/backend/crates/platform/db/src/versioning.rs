//! Generic non-destructive object versioning — migration 0069's proven
//! workflow-definition pattern (append-only versions table + trigger
//! protection + rollback-as-new-version) extracted into a reusable shape.
//!
//! # Adopting a domain
//! 1. **One migration** — copy the `registry_equipment_versions` block from
//!    migration 0107, substituting your base table name:
//!
//!    ```sql
//!    -- console-gate: audited-table <base>_versions
//!    CREATE TABLE <base>_versions (
//!        id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
//!        org_id         UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
//!        object_id      UUID        NOT NULL REFERENCES <base>(id) ON DELETE CASCADE,
//!        version        INTEGER     NOT NULL CHECK (version >= 1),
//!        status         TEXT        NOT NULL CHECK (status IN ('CAPTURED', 'ROLLBACK')),
//!        source_version INTEGER     NULL CHECK (source_version IS NULL OR source_version >= 1),
//!        content        JSONB       NOT NULL CHECK (jsonb_typeof(content) = 'object'),
//!        created_by     UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
//!        created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
//!        UNIQUE (org_id, object_id, version)
//!    );
//!    -- + index, append-only triggers (platform_append_only_immutable),
//!    --   RLS ENABLE/FORCE + org_isolation, GRANT SELECT, INSERT TO console_rt
//!    ```
//!
//! 2. **A few lines of Rust** — declare the table once and call the helpers
//!    inside your existing audited write path:
//!
//!    ```ignore
//!    const EQUIPMENT_VERSIONS: ObjectVersions =
//!        ObjectVersions::new("registry_equipment_versions");
//!    // inside the with_audit closure of the domain's update:
//!    EQUIPMENT_VERSIONS.capture(tx, org, id, &before, &after, actor).await?;
//!    ```
//!
//! Rollback never mutates history: it re-applies an old version's content as a
//! brand-new version (`status = 'ROLLBACK'`, `source_version = <target>`), so
//! the version list is a complete forward-only ledger.

use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::DbError;

/// One version row, as returned by [`ObjectVersions::list`] / `get`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ObjectVersionRecord {
    pub version: i32,
    pub status: String,
    pub source_version: Option<i32>,
    pub content: Value,
    pub created_by: Option<Uuid>,
    pub created_at: OffsetDateTime,
}

/// Handle to one domain's `<base>_versions` table.
///
/// The table name is a compile-time constant supplied by the adopting domain —
/// never user input — so interpolating it into SQL is safe.
#[derive(Debug, Clone, Copy)]
pub struct ObjectVersions {
    table: &'static str,
}

impl ObjectVersions {
    #[must_use]
    pub const fn new(table: &'static str) -> Self {
        Self { table }
    }

    /// Append the next version for `object_id` with the given content.
    ///
    /// Concurrency: `UNIQUE (org_id, object_id, version)` makes a racing
    /// append fail loudly instead of silently forking history; callers run
    /// inside row-locked update transactions, so in practice this never races.
    #[allow(clippy::too_many_arguments)] // mirrors the versions-table columns; a struct adds nothing
    pub async fn append(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        org_id: Uuid,
        object_id: Uuid,
        content: &Value,
        created_by: Option<Uuid>,
        status: &str,
        source_version: Option<i32>,
    ) -> Result<i32, DbError> {
        let sql = format!(
            "INSERT INTO {table} (org_id, object_id, version, status, source_version, content, created_by) \
             SELECT $1, $2, COALESCE(MAX(version), 0) + 1, $3, $4, $5, $6 \
             FROM {table} WHERE org_id = $1 AND object_id = $2 \
             RETURNING version",
            table = self.table
        );
        let version: i32 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(org_id)
            .bind(object_id)
            .bind(status)
            .bind(source_version)
            .bind(content)
            .bind(created_by)
            .fetch_one(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
        Ok(version)
    }

    /// Capture an update non-destructively. On the FIRST capture for an object
    /// the pre-update content is backfilled as version 1, so a rollback can
    /// always reach the original state; the post-update content then lands as
    /// the next version. Returns the version number of the `after` content.
    pub async fn capture(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        org_id: Uuid,
        object_id: Uuid,
        before: &Value,
        after: &Value,
        created_by: Option<Uuid>,
    ) -> Result<i32, DbError> {
        let sql = format!(
            "SELECT COUNT(*) FROM {table} WHERE org_id = $1 AND object_id = $2",
            table = self.table
        );
        let existing: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(org_id)
            .bind(object_id)
            .fetch_one(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
        if existing == 0 {
            self.append(tx, org_id, object_id, before, created_by, "CAPTURED", None)
                .await?;
        }
        self.append(tx, org_id, object_id, after, created_by, "CAPTURED", None)
            .await
    }

    /// List all versions for an object, newest first (RLS-scoped).
    pub async fn list(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        object_id: Uuid,
    ) -> Result<Vec<ObjectVersionRecord>, DbError> {
        let sql = format!(
            "SELECT version, status, source_version, content, created_by, created_at \
             FROM {table} WHERE object_id = $1 ORDER BY version DESC",
            table = self.table
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(object_id)
            .fetch_all(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
        rows.into_iter()
            .map(|row| {
                Ok(ObjectVersionRecord {
                    version: row.try_get("version").map_err(DbError::Sqlx)?,
                    status: row.try_get("status").map_err(DbError::Sqlx)?,
                    source_version: row.try_get("source_version").map_err(DbError::Sqlx)?,
                    content: row.try_get("content").map_err(DbError::Sqlx)?,
                    created_by: row.try_get("created_by").map_err(DbError::Sqlx)?,
                    created_at: row.try_get("created_at").map_err(DbError::Sqlx)?,
                })
            })
            .collect()
    }

    /// Fetch one version for an object (RLS-scoped), `None` when absent.
    pub async fn get(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        object_id: Uuid,
        version: i32,
    ) -> Result<Option<ObjectVersionRecord>, DbError> {
        let sql = format!(
            "SELECT version, status, source_version, content, created_by, created_at \
             FROM {table} WHERE object_id = $1 AND version = $2",
            table = self.table
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(object_id)
            .bind(version)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
        row.map(|row| {
            Ok(ObjectVersionRecord {
                version: row.try_get("version").map_err(DbError::Sqlx)?,
                status: row.try_get("status").map_err(DbError::Sqlx)?,
                source_version: row.try_get("source_version").map_err(DbError::Sqlx)?,
                content: row.try_get("content").map_err(DbError::Sqlx)?,
                created_by: row.try_get("created_by").map_err(DbError::Sqlx)?,
                created_at: row.try_get("created_at").map_err(DbError::Sqlx)?,
            })
        })
        .transpose()
    }
}
