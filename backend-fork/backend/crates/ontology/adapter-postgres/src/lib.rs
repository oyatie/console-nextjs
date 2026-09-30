//! Postgres ontology-registry adapter (§18 registry + §3a schema lifecycle).
//!
//! Each object type is a VERSIONED complete schema snapshot: one row per
//! `(org, stable_key, schema_version)` in `ont_object_types`, with its
//! property/link/action/analytic children hung off that version's id. Creating a
//! draft, staging a v+1 revision, and advancing the lifecycle FSM all enter the
//! database-owned single-writer routines, which mutate content and append exactly
//! one audit event atomically. All reads and writes arm `app.current_org` so
//! Postgres RLS scopes every row to the tenant.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod instances;
pub mod seed;

use console_kernel_core::{KernelError, TraceContext, UserId};
use console_ontology_domain::{
    ActionDispatch, ActionTypeId, AnalyticId, BackingKind, FieldKind, LinkCardinality, LinkTypeId,
    ObjectTypeId, PropertyDefId, SchemaLifecycleState,
};
use console_platform_db::{DbError, with_org_conn};
use console_platform_request_context::current_org;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum PgOntologyError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),

    #[error("ontology object type write precondition failed")]
    PreconditionFailed { current: ObjectTypeWriteVersion },

    #[error("ontology action revision precondition failed")]
    ActionPreconditionFailed { current: i64 },

    #[error("ontology command database capability is unavailable")]
    CommandUnavailable,
}

impl From<sqlx::Error> for PgOntologyError {
    fn from(value: sqlx::Error) -> Self {
        if let sqlx::Error::Database(database) = &value {
            let code = database.code();
            if let Some(error) = ontology_database_kernel_error(
                code.as_deref(),
                database.message(),
                database.constraint(),
            ) {
                return Self::Domain(error);
            }
        }

        Self::Db(DbError::Sqlx(value))
    }
}

fn ontology_database_kernel_error(
    code: Option<&str>,
    message: &str,
    constraint: Option<&str>,
) -> Option<KernelError> {
    match (code, message) {
        (
            Some("42501"),
            "ontology_write.tenant_context_mismatch"
            | "ontology_write.actor_forbidden"
            | "ontology_write.publish_approval_required",
        ) => Some(KernelError::forbidden(message)),
        (
            Some("22023"),
            "ontology_write.invalid_trace"
            | "ontology_write.invalid_snapshot_shape"
            | "ontology_write.stable_key_mismatch",
        ) => Some(KernelError::validation(message)),
        (
            Some("P0002"),
            "ontology_write.key_not_found" | "ontology_write.object_type_not_found",
        ) => Some(KernelError::not_found(message)),
        (
            Some("23514"),
            "ontology_write.review_required" | "ontology_write.illegal_lifecycle_transition",
        ) => Some(KernelError::invalid_transition(message)),
        (
            Some("23505"),
            "ontology_write.property_snapshot_conflict"
            | "ontology_write.link_snapshot_conflict"
            | "ontology_write.action_snapshot_conflict"
            | "ontology_write.analytic_snapshot_conflict"
            | "ontology_write.property_key_conflict"
            | "ontology_write.link_key_conflict"
            | "ontology_write.action_key_conflict"
            | "ontology_write.analytic_key_conflict",
        )
        | (Some("23514"), "ontology_write.review_pending_immutable")
        | (Some("40001"), "ontology_write.lifecycle_changed") => {
            Some(KernelError::conflict(message))
        }
        (Some("23505"), _) if constraint == Some("ont_object_type_key_revisions_pkey") => {
            Some(KernelError::conflict(message))
        }
        _ => None,
    }
}

// ===========================================================================
// Inputs (a complete schema snapshot for one object-type version).
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyDefInput {
    pub key: String,
    pub title: String,
    /// The discriminated-union tag (§3c). Stored verbatim; unknown tags degrade
    /// on read to [`FieldKind::Unknown`] rather than failing.
    pub field_type: String,
    #[serde(default = "empty_json_object")]
    pub config: serde_json::Value,
    #[serde(default)]
    pub backing_column: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub in_property_policy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkTypeInput {
    pub stable_key: String,
    pub title: String,
    /// Optional reverse (back-)link name (design change-log 74).
    #[serde(default)]
    pub reverse_title: Option<String>,
    #[serde(default)]
    pub to_object_type_id: Option<ObjectTypeId>,
    pub cardinality: LinkCardinality,
    #[serde(default = "default_true")]
    pub traversable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionTypeInput {
    pub stable_key: String,
    pub title: String,
    #[serde(default = "empty_json_object")]
    pub params_schema: serde_json::Value,
    #[serde(default = "empty_json_array")]
    pub edits: serde_json::Value,
    #[serde(default = "empty_json_array")]
    pub submission_criteria: serde_json::Value,
    #[serde(default = "empty_json_array")]
    pub side_effects: serde_json::Value,
    pub dispatch: ActionDispatch,
    #[serde(default)]
    pub dispatch_target: Option<String>,
    #[serde(default = "empty_json_array")]
    pub control_points: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyticInput {
    pub key: String,
    pub title: String,
    #[serde(default = "empty_json_object")]
    pub formula: serde_json::Value,
    #[serde(default = "empty_json_object")]
    pub result_type: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateObjectTypeDraft {
    pub stable_key: String,
    pub title: String,
    #[serde(default)]
    pub title_property_key: Option<String>,
    pub backing_kind: BackingKind,
    #[serde(default)]
    pub backing_table: Option<String>,
    #[serde(default)]
    pub primary_key_property: Option<String>,
    #[serde(default)]
    pub properties: Vec<PropertyDefInput>,
    #[serde(default)]
    pub links: Vec<LinkTypeInput>,
    #[serde(default)]
    pub actions: Vec<ActionTypeInput>,
    #[serde(default)]
    pub analytics: Vec<AnalyticInput>,
}

const fn default_true() -> bool {
    true
}

// ===========================================================================
// Summaries (read models).
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectTypeWriteVersion {
    pub etag: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectTypeWritePrecondition {
    pub validator_id: uuid::Uuid,
    pub revision: i64,
}

fn object_type_key_etag(validator_id: uuid::Uuid, revision: i64) -> String {
    format!(
        "\"ont-object-type-key:{}:r{revision}\"",
        validator_id.simple()
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectTypeSummary {
    pub id: ObjectTypeId,
    pub stable_key: String,
    pub title: String,
    pub backing_kind: BackingKind,
    pub schema_version: i64,
    pub lifecycle_state: SchemaLifecycleState,
    pub key_write_revision: i64,
    pub key_write_etag: String,
    #[serde(skip, default = "uuid::Uuid::nil")]
    key_write_validator_id: uuid::Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinCatalogInstall {
    pub installed: bool,
    pub object_type_count: i64,
}

impl ObjectTypeSummary {
    #[must_use]
    pub fn write_version(&self) -> ObjectTypeWriteVersion {
        ObjectTypeWriteVersion {
            etag: self.key_write_etag.clone(),
            revision: self.key_write_revision,
        }
    }

    #[must_use]
    pub fn write_precondition(&self) -> ObjectTypeWritePrecondition {
        ObjectTypeWritePrecondition {
            validator_id: self.key_write_validator_id,
            revision: self.key_write_revision,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyDefSummary {
    pub id: PropertyDefId,
    pub key: String,
    pub title: String,
    /// Raw stored tag.
    pub field_type: String,
    /// Parsed tag; [`FieldKind::Unknown`] for a tag this build does not know.
    pub field_kind: FieldKind,
    pub config: serde_json::Value,
    pub backing_column: Option<String>,
    pub required: bool,
    pub in_property_policy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkTypeSummary {
    pub id: LinkTypeId,
    pub stable_key: String,
    pub title: String,
    /// Optional reverse (back-)link name (design change-log 74).
    pub reverse_title: Option<String>,
    pub to_object_type_id: Option<ObjectTypeId>,
    pub cardinality: LinkCardinality,
    pub traversable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionTypeSummary {
    pub id: ActionTypeId,
    pub stable_key: String,
    pub title: String,
    pub params_schema: serde_json::Value,
    pub edits: serde_json::Value,
    pub submission_criteria: serde_json::Value,
    pub side_effects: serde_json::Value,
    pub dispatch: ActionDispatch,
    pub dispatch_target: Option<String>,
    pub control_points: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticSummary {
    pub id: AnalyticId,
    pub key: String,
    pub title: String,
    pub formula: serde_json::Value,
    pub result_type: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectTypeDetail {
    pub object_type: ObjectTypeSummary,
    pub title_property_key: Option<String>,
    pub backing_table: Option<String>,
    pub primary_key_property: Option<String>,
    pub properties: Vec<PropertyDefSummary>,
    pub links: Vec<LinkTypeSummary>,
    pub actions: Vec<ActionTypeSummary>,
    pub analytics: Vec<AnalyticSummary>,
}

// ===========================================================================
// Store.
// ===========================================================================

#[derive(Debug, Clone)]
pub struct PgOntologyStore {
    pool: PgPool,
    command_pool: Option<PgPool>,
}

impl PgOntologyStore {
    /// Construct a read-only ontology store. Every mutation fails closed until
    /// an isolated `console_ontology_cmd` pool is explicitly attached.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            command_pool: None,
        }
    }

    /// Production constructor: reads use the general RLS runtime pool, while all
    /// object-type mutations use the execute-only `console_ontology_cmd` credential.
    #[must_use]
    pub fn new_with_command_pool(pool: PgPool, command_pool: PgPool) -> Self {
        Self {
            pool,
            command_pool: Some(command_pool),
        }
    }

    #[must_use]
    pub fn with_command_pool(mut self, command_pool: PgPool) -> Self {
        self.command_pool = Some(command_pool);
        self
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    fn command_pool(&self) -> Result<&PgPool, PgOntologyError> {
        self.command_pool
            .as_ref()
            .ok_or(PgOntologyError::CommandUnavailable)
    }

    /// The attached command pool, if any — read-only, and `None` on a read-only
    /// store. `OntologyRestState::new` uses it to wire the object-policy attach
    /// path with the SAME `console_ontology_cmd` credential the caller already
    /// supplied here, so no composition root or fixture has to pass it twice.
    ///
    /// Declared BELOW `command_pool()` on purpose: `company_conformance/harness.rs`
    /// cites `command_pool()`'s line range, and that file is owned outside this
    /// lane. Inserting above it silently moved the citation onto this doc comment.
    #[must_use]
    pub fn command_pool_opt(&self) -> Option<&PgPool> {
        self.command_pool.as_ref()
    }

    /// Atomically install the migration-allowlisted built-in ontology catalog.
    /// The database canonicalizes and hashes the logical JSONB manifest, resolves
    /// same-catalog link stable keys to tenant-local IDs, and rejects every
    /// unknown version/digest or non-empty tenant without partial mutation.
    pub async fn install_builtin_catalog(
        &self,
        actor: UserId,
        catalog_version: &str,
        manifest: serde_json::Value,
        trace: TraceContext,
        _occurred_at: OffsetDateTime,
    ) -> Result<BuiltinCatalogInstall, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let catalog_version = catalog_version.to_owned();
        with_org_conn::<_, BuiltinCatalogInstall, PgOntologyError>(self.command_pool()?, org, |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT installed, object_type_count FROM ontology_api.install_builtin_catalog($1,$2,$3,$4,$5,$6)",
                )
                .bind(org_uuid)
                .bind(catalog_version)
                .bind(manifest)
                .bind(*actor.as_uuid())
                .bind(trace.trace_id())
                .bind(trace.span_id())
                .fetch_one(tx.as_mut())
                .await?;
                Ok(BuiltinCatalogInstall {
                    installed: row.try_get("installed")?,
                    object_type_count: row.try_get("object_type_count")?,
                })
            })
        })
        .await
    }

    /// Create a brand-new object type as schema_version 1 in `draft`, together
    /// with its full child snapshot and mandatory audit row in one DB-owned write.
    pub async fn create_object_type(
        &self,
        actor: UserId,
        draft: CreateObjectTypeDraft,
        trace: TraceContext,
        _occurred_at: OffsetDateTime,
    ) -> Result<ObjectTypeSummary, PgOntologyError> {
        validate_draft(&draft)?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let snapshot = serde_json::to_value(&draft).map_err(|error| {
            KernelError::validation(format!("invalid ontology snapshot: {error}"))
        })?;
        with_org_conn::<_, ObjectTypeSummary, PgOntologyError>(self.command_pool()?, org, |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT object_type_id AS id, stable_key, title, backing_kind, schema_version,
                       lifecycle_state, key_write_validator_id, key_write_revision
                FROM ontology_api.create_object_type($1, $2, $3, $4, $5)",
                )
                .bind(org_uuid)
                .bind(snapshot)
                .bind(*actor.as_uuid())
                .bind(trace.trace_id())
                .bind(trace.span_id())
                .fetch_one(tx.as_mut())
                .await?;
                object_type_summary_from_row(&row)
            })
        })
        .await
    }

    /// Stage a v+1 revision or append compatible definitions to the mutable draft.
    /// Version choice, CAS, child reconciliation, and audit are database-owned.
    pub async fn stage_revision(
        &self,
        actor: UserId,
        stable_key: &str,
        expected: ObjectTypeWritePrecondition,
        draft: CreateObjectTypeDraft,
        trace: TraceContext,
        _occurred_at: OffsetDateTime,
    ) -> Result<ObjectTypeSummary, PgOntologyError> {
        if draft.stable_key != stable_key {
            return Err(
                KernelError::validation("draft stable_key must match the revised type").into(),
            );
        }
        validate_draft(&draft)?;
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let stable_key = stable_key.to_owned();
        let stable_key_for_command = stable_key.clone();
        let snapshot = serde_json::to_value(&draft).map_err(|error| {
            KernelError::validation(format!("invalid ontology snapshot: {error}"))
        })?;
        let result = with_org_conn::<_, Option<ObjectTypeSummary>, PgOntologyError>(
            self.command_pool()?,
            org,
            |tx| {
                Box::pin(async move {
                    let row = sqlx::query(
                        r#"
                        SELECT object_type_id AS id, stable_key, title, backing_kind,
                               schema_version, lifecycle_state,
                               key_write_validator_id, key_write_revision
                        FROM ontology_api.stage_object_type($1,$2,$3,$4,$5,$6,$7,$8)
                        "#,
                    )
                    .bind(org_uuid)
                    .bind(&stable_key_for_command)
                    .bind(expected.validator_id)
                    .bind(expected.revision)
                    .bind(snapshot)
                    .bind(*actor.as_uuid())
                    .bind(trace.trace_id())
                    .bind(trace.span_id())
                    .fetch_optional(tx.as_mut())
                    .await?;
                    row.as_ref().map(object_type_summary_from_row).transpose()
                })
            },
        )
        .await?;
        if let Some(summary) = result {
            return Ok(summary);
        }
        with_org_conn::<_, ObjectTypeSummary, PgOntologyError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let (_, current) =
                    current_object_type_write_version_tx(tx, org_uuid, &stable_key).await?;
                Err(PgOntologyError::PreconditionFailed { current })
            })
        })
        .await
    }

    /// Advance one object-type version along the database-owned lifecycle FSM.
    /// The legacy boolean is ignored: draft publication is never available to
    /// console_rt; publication consumes target-bound four-eyes evidence atomically.
    #[allow(clippy::too_many_arguments)]
    pub async fn transition_lifecycle(
        &self,
        actor: UserId,
        object_type_id: ObjectTypeId,
        expected: ObjectTypeWritePrecondition,
        to: SchemaLifecycleState,
        _protection_enabled: bool,
        trace: TraceContext,
        _occurred_at: OffsetDateTime,
    ) -> Result<ObjectTypeSummary, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let result = with_org_conn::<_, Option<ObjectTypeSummary>, PgOntologyError>(
            self.command_pool()?,
            org,
            |tx| {
                Box::pin(async move {
                    let row = sqlx::query(
                        r#"
                        SELECT object_type_id AS id, stable_key, title, backing_kind,
                               schema_version, lifecycle_state,
                               key_write_validator_id, key_write_revision
                        FROM ontology_api.transition_object_type($1,$2,$3,$4,$5,$6,$7,$8)
                        "#,
                    )
                    .bind(org_uuid)
                    .bind(*object_type_id.as_uuid())
                    .bind(expected.validator_id)
                    .bind(expected.revision)
                    .bind(to.as_db_str())
                    .bind(*actor.as_uuid())
                    .bind(trace.trace_id())
                    .bind(trace.span_id())
                    .fetch_optional(tx.as_mut())
                    .await?;
                    row.as_ref().map(object_type_summary_from_row).transpose()
                })
            },
        )
        .await?;
        if let Some(summary) = result {
            return Ok(summary);
        }
        with_org_conn::<_, ObjectTypeSummary, PgOntologyError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let stable_key: String = sqlx::query_scalar(
                    "SELECT stable_key FROM ont_object_types WHERE id=$1 AND org_id=$2",
                )
                .bind(*object_type_id.as_uuid())
                .bind(org_uuid)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("object type version was not found"))?;
                let (_, current) =
                    current_object_type_write_version_tx(tx, org_uuid, &stable_key).await?;
                Err(PgOntologyError::PreconditionFailed { current })
            })
        })
        .await
    }

    /// The `(stable_key, schema_version)` of ONE object-type version, addressed
    /// by its own id and tenant-scoped by RLS.
    ///
    /// [`Self::list_object_types`] and [`Self::get_object_type`] are both
    /// `DISTINCT ON (o.stable_key)`: they answer "the current head for this key"
    /// and CANNOT resolve a superseded version id at all. An instance keeps the
    /// `object_type_id` it was created under for life — `stage_object_type`
    /// inserts a brand-new row with a fresh id (`0165:884-900`) and nothing
    /// re-points `ont_instances` — so anything resolving an INSTANCE's type must
    /// come through here. Resolving it through the head instead makes every row
    /// created before a revision unreadable the moment the next version
    /// publishes.
    pub async fn object_type_version(
        &self,
        object_type_id: ObjectTypeId,
    ) -> Result<(String, i64), PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, (String, i64), PgOntologyError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                sqlx::query_as(
                    "SELECT stable_key, schema_version FROM ont_object_types WHERE id = $1",
                )
                .bind(*object_type_id.as_uuid())
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("object type was not found").into())
            })
        })
        .await
    }

    /// List the current head per object-type key (the published version if one
    /// exists, else the highest schema_version), tenant-scoped by RLS.
    pub async fn list_object_types(&self) -> Result<Vec<ObjectTypeSummary>, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, Vec<ObjectTypeSummary>, PgOntologyError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    r#"
                    SELECT DISTINCT ON (o.stable_key)
                        o.id, o.stable_key, o.title, o.backing_kind, o.schema_version,
                        o.lifecycle_state, k.validator_id AS key_write_validator_id,
                        k.revision AS key_write_revision
                    FROM ont_object_types o
                    JOIN ont_object_type_key_revisions k USING (org_id, stable_key)
                    ORDER BY o.stable_key,
                             (o.lifecycle_state = 'published') DESC,
                             o.schema_version DESC
                    "#,
                )
                .fetch_all(tx.as_mut())
                .await?;
                rows.iter().map(object_type_summary_from_row).collect()
            })
        })
        .await
    }

    /// Fetch one object-type version plus its full child snapshot. With
    /// `version = None` the head (published, else latest) is returned; with an
    /// explicit version, that exact revision (as-of schema).
    pub async fn get_object_type(
        &self,
        stable_key: &str,
        version: Option<i64>,
    ) -> Result<ObjectTypeDetail, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let stable_key = stable_key.to_owned();
        with_org_conn::<_, ObjectTypeDetail, PgOntologyError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let head = sqlx::query(
                    r#"
                    SELECT DISTINCT ON (o.stable_key)
                        o.id, o.stable_key, o.title, o.title_property_key, o.backing_kind,
                        o.backing_table, o.primary_key_property, o.schema_version,
                        o.lifecycle_state, k.validator_id AS key_write_validator_id,
                        k.revision AS key_write_revision
                    FROM ont_object_types o
                    JOIN ont_object_type_key_revisions k USING (org_id, stable_key)
                    WHERE o.stable_key = $1
                      AND ($2::BIGINT IS NULL OR o.schema_version = $2)
                      AND o.org_id = $3
                    ORDER BY o.stable_key,
                             (o.lifecycle_state = 'published') DESC,
                             o.schema_version DESC
                    "#,
                )
                .bind(&stable_key)
                .bind(version)
                .bind(org_uuid)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("object type was not found"))?;

                let object_type = object_type_summary_from_row(&head)?;
                let type_id = *object_type.id.as_uuid();

                let properties = sqlx::query(
                    r#"
                    SELECT id, key, title, type, config, backing_column, required, in_property_policy
                    FROM ont_property_defs
                    WHERE object_type_id = $1 AND org_id = $2
                    ORDER BY key
                    "#,
                )
                .bind(type_id)
                .bind(org_uuid)
                .fetch_all(tx.as_mut())
                .await?
                .iter()
                .map(property_def_from_row)
                .collect::<Result<Vec<_>, _>>()?;

                let links = sqlx::query(
                    r#"
                    SELECT id, stable_key, title, reverse_title, to_object_type_id,
                           cardinality, traversable
                    FROM ont_link_types
                    WHERE object_type_id = $1 AND org_id = $2
                    ORDER BY stable_key
                    "#,
                )
                .bind(type_id)
                .bind(org_uuid)
                .fetch_all(tx.as_mut())
                .await?
                .iter()
                .map(link_type_from_row)
                .collect::<Result<Vec<_>, _>>()?;

                let actions = sqlx::query(
                    r#"
                    SELECT id, stable_key, title, params_schema, edits, submission_criteria,
                           side_effects, dispatch, dispatch_target, control_points
                    FROM ont_action_types
                    WHERE object_type_id = $1 AND org_id = $2
                    ORDER BY stable_key
                    "#,
                )
                .bind(type_id)
                .bind(org_uuid)
                .fetch_all(tx.as_mut())
                .await?
                .iter()
                .map(action_type_from_row)
                .collect::<Result<Vec<_>, _>>()?;

                let analytics = sqlx::query(
                    r#"
                    SELECT id, key, title, formula, result_type
                    FROM ont_analytics
                    WHERE object_type_id = $1 AND org_id = $2
                    ORDER BY key
                    "#,
                )
                .bind(type_id)
                .bind(org_uuid)
                .fetch_all(tx.as_mut())
                .await?
                .iter()
                .map(analytic_from_row)
                .collect::<Result<Vec<_>, _>>()?;

                Ok(ObjectTypeDetail {
                    object_type,
                    title_property_key: head.try_get("title_property_key")?,
                    backing_table: head.try_get("backing_table")?,
                    primary_key_property: head.try_get("primary_key_property")?,
                    properties,
                    links,
                    actions,
                    analytics,
                })
            })
        })
        .await
    }

    /// Resolve one action type by its `(object_type_id, stable_key)` — the pair
    /// that uniquely identifies an action within the tenant (action `stable_key`
    /// is a single segment, unique per object-type version). RLS-scoped: an action
    /// belonging to another tenant returns `None`, never leaks. `None` = the key
    /// is unknown to this tenant/object type.
    pub async fn get_action_type(
        &self,
        object_type_id: ObjectTypeId,
        action_key: &str,
    ) -> Result<Option<ActionTypeSummary>, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let action_key = action_key.to_owned();
        with_org_conn::<_, Option<ActionTypeSummary>, PgOntologyError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    r#"
                    SELECT id, stable_key, title, params_schema, edits, submission_criteria,
                           side_effects, dispatch, dispatch_target, control_points
                    FROM ont_action_types
                    WHERE object_type_id = $1 AND stable_key = $2 AND org_id = $3
                    "#,
                )
                .bind(*object_type_id.as_uuid())
                .bind(&action_key)
                .bind(org_uuid)
                .fetch_optional(tx.as_mut())
                .await?;
                row.as_ref().map(action_type_from_row).transpose()
            })
        })
        .await
    }

    /// Read-only §2 "dynamics": the automation rules + PBAC policies acting on this
    /// instance's object type — workflow definitions bound to the type key, plus
    /// object-policy (row) and property-policy (field) attachments for the type.
    /// RLS-scoped; a cross-org / unknown id yields `NotFound` (no existence leak).
    pub async fn acting_on_instance(
        &self,
        instance_id: uuid::Uuid,
    ) -> Result<Vec<ActingRule>, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        with_org_conn::<_, Vec<ActingRule>, PgOntologyError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                // Resolve the instance's type (RLS-scoped): missing ⇒ NotFound.
                let type_row = sqlx::query(
                    r#"
                    SELECT o.stable_key AS stable_key, i.object_type_id AS object_type_id
                    FROM ont_instances i
                    JOIN ont_object_types o ON o.id = i.object_type_id
                    WHERE i.id = $1
                    "#,
                )
                .bind(instance_id)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("instance was not found"))?;
                let stable_key: String = type_row.try_get("stable_key")?;
                let object_type_id: uuid::Uuid = type_row.try_get("object_type_id")?;
                acting_rules_tx(tx, org_uuid, &stable_key, object_type_id).await
            })
        })
        .await
    }

    /// The same §2 "dynamics" read as `acting_on_instance`, keyed by the object
    /// type itself (the 자동화 subtab of the Ontology Manager, which is
    /// type-centric and may have no instances). RLS-scoped; an unknown key yields
    /// `NotFound`. Resolves the key to its head version so property-policy
    /// attachments (per-version `property_def_id`) line up with the shown schema.
    pub async fn acting_on_type(
        &self,
        stable_key: &str,
    ) -> Result<Vec<ActingRule>, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let stable_key = stable_key.to_owned();
        with_org_conn::<_, Vec<ActingRule>, PgOntologyError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                // Head version (published preferred, else highest) — same
                // resolution as get_object_type, RLS-scoped.
                let object_type_id: uuid::Uuid = sqlx::query_scalar(
                    r#"
                    SELECT id FROM ont_object_types
                    WHERE stable_key = $1 AND org_id = $2
                    ORDER BY (lifecycle_state = 'published') DESC, schema_version DESC
                    LIMIT 1
                    "#,
                )
                .bind(&stable_key)
                .bind(org_uuid)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("object type was not found"))?;
                acting_rules_tx(tx, org_uuid, &stable_key, object_type_id).await
            })
        })
        .await
    }

    /// Resolve a human-facing instance `code` (the head revision's `code` attribute)
    /// to its identity, for run-log chips + drag-drop lookups. RLS-scoped and
    /// deny-by-omission: another tenant's code — or an unknown one — resolves to
    /// `None`, which the caller renders as a 404 (never a 403), so the endpoint
    /// leaks neither existence nor cross-tenant membership.
    // ponytail: matches the head revision's `code` attribute; lift to a per-type
    // configurable code property if a type ever names its code column differently.
    pub async fn resolve_by_code(
        &self,
        code: &str,
    ) -> Result<Option<ResolvedInstance>, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        let code = code.to_owned();
        with_org_conn::<_, Option<ResolvedInstance>, PgOntologyError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    r#"
                    SELECT i.id AS id, o.stable_key AS type_key, i.title AS title
                    FROM ont_instances i
                    JOIN ont_object_types o ON o.id = i.object_type_id
                    JOIN ont_instance_revisions r
                      ON r.instance_id = i.id AND r.valid_to IS NULL
                    WHERE r.attributes->>'code' = $1
                    LIMIT 1
                    "#,
                )
                .bind(&code)
                .fetch_optional(tx.as_mut())
                .await?;
                row.as_ref()
                    .map(|row| {
                        Ok::<_, PgOntologyError>(ResolvedInstance {
                            id: row.try_get("id")?,
                            type_key: row.try_get("type_key")?,
                            title: row.try_get("title")?,
                        })
                    })
                    .transpose()
            })
        })
        .await
    }
}

/// One automation rule or PBAC policy acting on an object type (§2 dynamics).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActingRule {
    pub id: uuid::Uuid,
    pub label: String,
    pub kind: ActingKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActingKind {
    Automation,
    Policy,
}

/// A code→instance resolution: the instance identity + its object-type key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedInstance {
    pub id: uuid::Uuid,
    #[serde(rename = "type")]
    pub type_key: String,
    pub title: String,
}

// ===========================================================================
// tx helpers
// ===========================================================================

async fn current_object_type_write_version_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: uuid::Uuid,
    stable_key: &str,
) -> Result<(uuid::Uuid, ObjectTypeWriteVersion), PgOntologyError> {
    let row = sqlx::query(
        r#"
        SELECT validator_id, revision
        FROM ont_object_type_key_revisions
        WHERE org_id = $1 AND stable_key = $2
        "#,
    )
    .bind(org_uuid)
    .bind(stable_key)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("object type was not found"))?;
    let validator_id: uuid::Uuid = row.try_get("validator_id")?;
    let revision: i64 = row.try_get("revision")?;
    Ok((
        validator_id,
        ObjectTypeWriteVersion {
            etag: object_type_key_etag(validator_id, revision),
            revision,
        },
    ))
}

/// §2 dynamics for one object type: live workflow definitions bound to its key
/// (automations) plus object-/property-policy attachments (policies), labelled by
/// the authored catalog title. Shared by the instance- and type-keyed reads.
async fn acting_rules_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: uuid::Uuid,
    stable_key: &str,
    object_type_id: uuid::Uuid,
) -> Result<Vec<ActingRule>, PgOntologyError> {
    let mut acting = Vec::new();

    let automations = sqlx::query(
        r#"
        SELECT id, display_name
        FROM workflow_definitions
        WHERE object_type = $1 AND org_id = $2 AND status <> 'RETIRED'
        ORDER BY updated_at DESC
        "#,
    )
    .bind(stable_key)
    .bind(org_uuid)
    .fetch_all(tx.as_mut())
    .await?;
    for row in &automations {
        acting.push(ActingRule {
            id: row.try_get("id")?,
            label: row.try_get("display_name")?,
            kind: ActingKind::Automation,
        });
    }

    let policies = sqlx::query(
        r#"
        SELECT c.id AS id, c.title AS title
        FROM ont_object_policies a
        JOIN cedar_policy_catalog_entries c
          ON c.id = a.cedar_policy_id AND c.org_id = a.org_id
        WHERE a.object_type_id = $1 AND a.org_id = $2
        UNION
        SELECT c.id AS id, c.title AS title
        FROM ont_property_policies a
        JOIN cedar_policy_catalog_entries c
          ON c.id = a.cedar_policy_id AND c.org_id = a.org_id
        WHERE a.org_id = $2
          AND a.property_def_id IN (
            SELECT id FROM ont_property_defs WHERE object_type_id = $1 AND org_id = $2
        )
        "#,
    )
    .bind(object_type_id)
    .bind(org_uuid)
    .fetch_all(tx.as_mut())
    .await?;
    for row in &policies {
        acting.push(ActingRule {
            id: row.try_get("id")?,
            label: row.try_get("title")?,
            kind: ActingKind::Policy,
        });
    }

    Ok(acting)
}

// ===========================================================================
// row → summary mappers
// ===========================================================================

fn object_type_summary_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<ObjectTypeSummary, PgOntologyError> {
    Ok(ObjectTypeSummary {
        id: ObjectTypeId::from_uuid(row.try_get("id")?),
        stable_key: row.try_get("stable_key")?,
        title: row.try_get("title")?,
        backing_kind: BackingKind::from_db_str(row.try_get("backing_kind")?)?,
        schema_version: row.try_get("schema_version")?,
        lifecycle_state: SchemaLifecycleState::from_db_str(row.try_get("lifecycle_state")?)?,
        key_write_revision: row.try_get("key_write_revision")?,
        key_write_etag: object_type_key_etag(
            row.try_get("key_write_validator_id")?,
            row.try_get("key_write_revision")?,
        ),
        key_write_validator_id: row.try_get("key_write_validator_id")?,
    })
}

fn property_def_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<PropertyDefSummary, PgOntologyError> {
    let field_type: String = row.try_get("type")?;
    let field_kind = FieldKind::parse(&field_type);
    Ok(PropertyDefSummary {
        id: PropertyDefId::from_uuid(row.try_get("id")?),
        key: row.try_get("key")?,
        title: row.try_get("title")?,
        field_type,
        field_kind,
        config: row.try_get("config")?,
        backing_column: row.try_get("backing_column")?,
        required: row.try_get("required")?,
        in_property_policy: row.try_get("in_property_policy")?,
    })
}

fn link_type_from_row(row: &sqlx::postgres::PgRow) -> Result<LinkTypeSummary, PgOntologyError> {
    let to: Option<uuid::Uuid> = row.try_get("to_object_type_id")?;
    Ok(LinkTypeSummary {
        id: LinkTypeId::from_uuid(row.try_get("id")?),
        stable_key: row.try_get("stable_key")?,
        title: row.try_get("title")?,
        reverse_title: row.try_get("reverse_title")?,
        to_object_type_id: to.map(ObjectTypeId::from_uuid),
        cardinality: LinkCardinality::from_db_str(row.try_get("cardinality")?)?,
        traversable: row.try_get("traversable")?,
    })
}

fn action_type_from_row(row: &sqlx::postgres::PgRow) -> Result<ActionTypeSummary, PgOntologyError> {
    Ok(ActionTypeSummary {
        id: ActionTypeId::from_uuid(row.try_get("id")?),
        stable_key: row.try_get("stable_key")?,
        title: row.try_get("title")?,
        params_schema: row.try_get("params_schema")?,
        edits: row.try_get("edits")?,
        submission_criteria: row.try_get("submission_criteria")?,
        side_effects: row.try_get("side_effects")?,
        dispatch: ActionDispatch::from_db_str(row.try_get("dispatch")?)?,
        dispatch_target: row.try_get("dispatch_target")?,
        control_points: row.try_get("control_points")?,
    })
}

fn analytic_from_row(row: &sqlx::postgres::PgRow) -> Result<AnalyticSummary, PgOntologyError> {
    Ok(AnalyticSummary {
        id: AnalyticId::from_uuid(row.try_get("id")?),
        key: row.try_get("key")?,
        title: row.try_get("title")?,
        formula: row.try_get("formula")?,
        result_type: row.try_get("result_type")?,
    })
}

// ===========================================================================
// small helpers
// ===========================================================================

/// Serde default that preserves the object-vs-array contract. Explicit null
/// remains `Value::Null` so validation can reject it instead of changing intent.
fn empty_json_object() -> serde_json::Value {
    serde_json::json!({})
}

/// Array counterpart to [`empty_json_object`].
fn empty_json_array() -> serde_json::Value {
    serde_json::json!([])
}

fn validate_draft(draft: &CreateObjectTypeDraft) -> Result<(), PgOntologyError> {
    let stable_key = draft.stable_key.trim();
    if stable_key.is_empty() {
        return Err(KernelError::validation("object type stable_key is required").into());
    }
    if stable_key != draft.stable_key {
        return Err(KernelError::validation(
            "object type stable_key must not have surrounding whitespace",
        )
        .into());
    }
    if draft.title.trim().is_empty() {
        return Err(KernelError::validation("object type title is required").into());
    }
    let mut property_keys = HashSet::new();
    for property in &draft.properties {
        let key = property.key.trim();
        if !property_keys.insert(key) {
            return Err(KernelError::validation(format!(
                "duplicate property key {key:?} in object type draft"
            ))
            .into());
        }
        if !property.config.is_object() {
            return Err(KernelError::validation(format!(
                "property {:?} config must be a JSON object",
                key
            ))
            .into());
        }
    }
    let mut link_keys = HashSet::new();
    for link in &draft.links {
        let key = link.stable_key.trim();
        if !link_keys.insert(key) {
            return Err(KernelError::validation(format!(
                "duplicate link key {key:?} in object type draft"
            ))
            .into());
        }
    }
    let mut action_keys = HashSet::new();
    for action in &draft.actions {
        let key = action.stable_key.trim();
        if !action_keys.insert(key) {
            return Err(KernelError::validation(format!(
                "duplicate action key {key:?} in object type draft"
            ))
            .into());
        }
        if !action.params_schema.is_object() {
            return Err(KernelError::validation(format!(
                "action {:?} params_schema must be a JSON object",
                key
            ))
            .into());
        }
        for (field, value) in [
            ("edits", &action.edits),
            ("submission_criteria", &action.submission_criteria),
            ("side_effects", &action.side_effects),
            ("control_points", &action.control_points),
        ] {
            if !value.is_array() {
                return Err(KernelError::validation(format!(
                    "action {:?} {field} must be a JSON array",
                    key
                ))
                .into());
            }
        }
    }
    let mut analytic_keys = HashSet::new();
    for analytic in &draft.analytics {
        let key = analytic.key.trim();
        if !analytic_keys.insert(key) {
            return Err(KernelError::validation(format!(
                "duplicate analytic key {key:?} in object type draft"
            ))
            .into());
        }
        if !analytic.formula.is_object() {
            return Err(KernelError::validation(format!(
                "analytic {:?} formula must be a JSON object",
                key
            ))
            .into());
        }
        if !analytic.result_type.is_object() {
            return Err(KernelError::validation(format!(
                "analytic {:?} result_type must be a JSON object",
                key
            ))
            .into());
        }
    }
    match draft.backing_kind {
        BackingKind::Projected => {
            if draft
                .backing_table
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
                || draft
                    .primary_key_property
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
            {
                return Err(KernelError::validation(
                    "projected object types require backing_table and primary_key_property",
                )
                .into());
            }
        }
        BackingKind::Instance => {
            if draft.backing_table.is_some() || draft.primary_key_property.is_some() {
                return Err(KernelError::validation(
                    "instance object types must not carry a backing table",
                )
                .into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ontology_database_kernel_error;
    use console_kernel_core::ErrorKind;

    #[test]
    fn ontology_database_markers_map_only_by_exact_code_and_message() {
        for (code, message, expected) in [
            (
                "42501",
                "ontology_write.tenant_context_mismatch",
                ErrorKind::Forbidden,
            ),
            (
                "42501",
                "ontology_write.actor_forbidden",
                ErrorKind::Forbidden,
            ),
            (
                "42501",
                "ontology_write.publish_approval_required",
                ErrorKind::Forbidden,
            ),
            (
                "22023",
                "ontology_write.invalid_trace",
                ErrorKind::Validation,
            ),
            (
                "22023",
                "ontology_write.invalid_snapshot_shape",
                ErrorKind::Validation,
            ),
            (
                "22023",
                "ontology_write.stable_key_mismatch",
                ErrorKind::Validation,
            ),
            ("P0002", "ontology_write.key_not_found", ErrorKind::NotFound),
            (
                "P0002",
                "ontology_write.object_type_not_found",
                ErrorKind::NotFound,
            ),
            (
                "23514",
                "ontology_write.review_required",
                ErrorKind::InvalidTransition,
            ),
            (
                "23514",
                "ontology_write.illegal_lifecycle_transition",
                ErrorKind::InvalidTransition,
            ),
            (
                "23514",
                "ontology_write.review_pending_immutable",
                ErrorKind::Conflict,
            ),
            (
                "40001",
                "ontology_write.lifecycle_changed",
                ErrorKind::Conflict,
            ),
            (
                "23505",
                "ontology_write.property_snapshot_conflict",
                ErrorKind::Conflict,
            ),
            (
                "23505",
                "ontology_write.link_snapshot_conflict",
                ErrorKind::Conflict,
            ),
            (
                "23505",
                "ontology_write.action_snapshot_conflict",
                ErrorKind::Conflict,
            ),
            (
                "23505",
                "ontology_write.analytic_snapshot_conflict",
                ErrorKind::Conflict,
            ),
            (
                "23505",
                "ontology_write.property_key_conflict",
                ErrorKind::Conflict,
            ),
            (
                "23505",
                "ontology_write.link_key_conflict",
                ErrorKind::Conflict,
            ),
            (
                "23505",
                "ontology_write.action_key_conflict",
                ErrorKind::Conflict,
            ),
            (
                "23505",
                "ontology_write.analytic_key_conflict",
                ErrorKind::Conflict,
            ),
        ] {
            let mapped = ontology_database_kernel_error(Some(code), message, None)
                .unwrap_or_else(|| panic!("missing exact mapping for {code}/{message}"));
            assert_eq!(mapped.kind, expected, "wrong mapping for {code}/{message}");
            assert_eq!(mapped.message, message);
        }

        let pkey = ontology_database_kernel_error(
            Some("23505"),
            "duplicate key value violates unique constraint",
            Some("ont_object_type_key_revisions_pkey"),
        )
        .expect("the exact object-type key revision constraint is allowlisted");
        assert_eq!(pkey.kind, ErrorKind::Conflict);
    }

    #[test]
    fn unknown_database_errors_are_not_downgraded_to_domain_errors() {
        assert!(
            ontology_database_kernel_error(Some("XX000"), "ontology_write.review_required", None,)
                .is_none()
        );
        assert!(
            ontology_database_kernel_error(Some("23514"), "unrecognized.trigger_error", None)
                .is_none()
        );
        // The deferred audit constraint is a server-side integrity invariant;
        // keep it on the raw DB path so REST logs it and returns 500.
        assert!(
            ontology_database_kernel_error(
                Some("23514"),
                "ontology_write.exactly_one_current_transaction_audit_required",
                None,
            )
            .is_none()
        );
        assert!(
            ontology_database_kernel_error(
                Some("23505"),
                "duplicate key value violates unique constraint",
                Some("unrelated_unique_key"),
            )
            .is_none()
        );
    }
}
