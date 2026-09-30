//! Postgres owned instance store (§1b) for user-authored (`instance`-backed)
//! object types.
//!
//! Current state is a FOLD over immutable, fixity-stamped revisions, never an
//! in-place mutate. A create appends v1; an edit stages a v+1 revision and closes
//! the prior interval (`valid_to`). Reads answer either the current head
//! (`valid_to IS NULL`) or as-of some instant (`valid_from <= t < valid_to`).
//! Every revision is bound into a per-`(org, instance)` SHA-256 hash chain, so a
//! rewritten revision is detectable by recomputing the chain (§1b fixity). The
//! §2 search-around graph is a bounded BFS over the effective-dated `ont_links`.
//!
//! Like the registry adapter, mutations wrap [`with_audit`] (mutation + audit in
//! one tx, `app.current_org` armed) and reads wrap [`with_org_conn`], so Postgres
//! RLS scopes every row to the tenant and an unset org fails closed.

use std::collections::{BTreeMap, HashMap, HashSet};

use console_kernel_core::{AuditAction, AuditEvent, KernelError, OrgId, TraceContext, UserId};
use console_ontology_domain::{
    ActionTypeId, FieldKind, InstanceId, InstanceLifecycleState, InstanceLinkId,
    InstanceRevisionId, LinkTypeId, ObjectTypeId, validate_instance_transition,
};
use console_platform_authz::cedar_pbac::residual::{
    LoweringTarget, ObjectPolicy, SqlValue, SubjectAttrs, lower,
};
use console_platform_db::{with_audit, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::PgOntologyError;

/// Genesis `prev_hash` for the first revision of an instance (64 hex zeros).
const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Hard cap on traversal hops. A user-supplied `depth` is clamped to this so a
/// pathological ontology graph can never fan out unbounded.
// ponytail: fixed cap; lift only if a real Object-Explorer view needs deeper walks.
const MAX_TRAVERSAL_DEPTH: u32 = 8;

// ===========================================================================
// Inputs
// ===========================================================================

/// Create a brand-new instance (revision v1) of an `instance`-backed object type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateInstance {
    /// The object-type VERSION snapshot (0105 head id) this instance conforms to.
    pub object_type_id: ObjectTypeId,
    pub title: String,
    /// Attribute bag validated against the type's property schema (§1b).
    #[serde(default)]
    pub attributes: serde_json::Value,
    /// Effective start; defaults to the mutation's `occurred_at`. Future-dating
    /// (`valid_from` in the future) is allowed.
    #[serde(default)]
    pub valid_from: Option<OffsetDateTime>,
    #[serde(default)]
    pub action_type_id: Option<ActionTypeId>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Stage a v+1 revision of an existing instance (effective-dated).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageRevision {
    #[serde(default)]
    pub attributes: serde_json::Value,
    /// When the new revision becomes effective; defaults to `occurred_at`. Must be
    /// strictly after the current revision's `valid_from`.
    #[serde(default)]
    pub valid_from: Option<OffsetDateTime>,
    #[serde(default)]
    pub action_type_id: Option<ActionTypeId>,
    #[serde(default)]
    pub reason: Option<String>,
}

// ===========================================================================
// Read models
// ===========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionSummary {
    pub id: InstanceRevisionId,
    pub instance_id: InstanceId,
    pub version: i64,
    pub attributes: serde_json::Value,
    #[serde(with = "time::serde::rfc3339")]
    pub valid_from: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub valid_to: Option<OffsetDateTime>,
    pub action_type_id: Option<ActionTypeId>,
    pub actor: Option<UserId>,
    pub reason: Option<String>,
    pub prev_hash: String,
    pub row_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceHead {
    pub id: InstanceId,
    pub object_type_id: ObjectTypeId,
    pub title: String,
    pub current_revision_id: Option<InstanceRevisionId>,
    pub lifecycle_state: InstanceLifecycleState,
}

/// An instance projected at some point in time (current or as-of): the head plus
/// the effective revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceState {
    pub instance: InstanceHead,
    pub revision: RevisionSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalNode {
    pub instance_id: InstanceId,
    pub object_type_id: ObjectTypeId,
    pub title: String,
    pub lifecycle_state: InstanceLifecycleState,
    /// Distance in hops from the traversal root (root = 0).
    pub depth: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalEdge {
    pub id: InstanceLinkId,
    pub link_type_id: LinkTypeId,
    pub from_instance_id: InstanceId,
    pub to_instance_id: InstanceId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalGraph {
    pub root: InstanceId,
    pub nodes: Vec<TraversalNode>,
    pub edges: Vec<TraversalEdge>,
}

/// Allowlisted group key for [`PgInstanceStore::aggregate_instances`].
///
/// Free-form SQL expressions are rejected: only these shapes may be spliced
/// into the SELECT list, and attribute keys must pass [`is_safe_ident`] (bound,
/// never interpolated) — the same injection discipline as residual lowering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregateGroupBy {
    LifecycleState,
    ObjectTypeId,
    /// `r.attributes ->> $key` for a declared property key (caller validates
    /// declaration; this adapter only enforces identifier safety).
    Attribute(String),
}

/// One bucket from [`PgInstanceStore::aggregate_instances`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AggregateBucket {
    /// Text form of the group value. Absent JSON attributes yield `None`.
    pub key: Option<String>,
    pub count: i64,
}

// ===========================================================================
// Store
// ===========================================================================

#[derive(Debug, Clone)]
pub struct PgInstanceStore {
    pool: sqlx::PgPool,
}

impl PgInstanceStore {
    #[must_use]
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }

    /// Create a new instance as revision v1 in `draft`, in one audited tx.
    pub async fn create_instance(
        &self,
        actor: UserId,
        input: CreateInstance,
        trace: TraceContext,
        occurred_at: OffsetDateTime,
    ) -> Result<InstanceState, PgOntologyError> {
        if input.title.trim().is_empty() {
            return Err(KernelError::validation("instance title is required").into());
        }
        let org = current_org().map_err(KernelError::from)?;
        with_audits::<_, InstanceState, PgOntologyError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let state = create_instance_in_tx(tx, actor, org, input, occurred_at).await?;
                let event = AuditEvent::new(
                    Some(actor),
                    AuditAction::new("ontology.instance.create")?,
                    "ont_instances",
                    state.instance.id.to_string(),
                    trace,
                    occurred_at,
                )
                .with_org(org)
                .with_snapshots(
                    None,
                    Some(serde_json::json!({
                        "object_type_id": state.instance.object_type_id.to_string(),
                        "title": state.instance.title,
                        "version": state.revision.version,
                        "lifecycle_state": state.instance.lifecycle_state.as_db_str(),
                    })),
                );
                Ok((state, vec![event]))
            })
        })
        .await
    }

    /// Stage a v+1 revision: close the current interval and append a new open
    /// revision effective at `valid_from`, chained onto the current row_hash.
    pub async fn stage_revision(
        &self,
        actor: UserId,
        instance_id: InstanceId,
        input: StageRevision,
        trace: TraceContext,
        occurred_at: OffsetDateTime,
    ) -> Result<InstanceState, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        with_audits::<_, InstanceState, PgOntologyError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let state =
                    stage_revision_in_tx(tx, actor, org, instance_id, input, occurred_at).await?;
                let event = AuditEvent::new(
                    Some(actor),
                    AuditAction::new("ontology.instance.stage_revision")?,
                    "ont_instance_revisions",
                    instance_id.to_string(),
                    trace,
                    occurred_at,
                )
                .with_org(org)
                .with_snapshots(
                    None,
                    Some(serde_json::json!({
                        "version": state.revision.version,
                        "attributes": state.revision.attributes,
                    })),
                );
                Ok((state, vec![event]))
            })
        })
        .await
    }

    /// Advance the instance lifecycle FSM (§3b), validated against the built-in
    /// transition table.
    pub async fn transition_lifecycle(
        &self,
        actor: UserId,
        instance_id: InstanceId,
        to: InstanceLifecycleState,
        trace: TraceContext,
        occurred_at: OffsetDateTime,
    ) -> Result<InstanceHead, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        let event = AuditEvent::new(
            Some(actor),
            AuditAction::new("ontology.instance.transition")?,
            "ont_instances",
            instance_id.to_string(),
            trace,
            occurred_at,
        )
        .with_org(org);

        with_audit::<_, InstanceHead, PgOntologyError>(&self.pool, event, |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT lifecycle_state FROM ont_instances WHERE id = $1 FOR UPDATE",
                )
                .bind(*instance_id.as_uuid())
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("instance was not found"))?;
                let from = InstanceLifecycleState::from_db_str(row.try_get("lifecycle_state")?)?;
                validate_instance_transition(from, to)?;

                sqlx::query(
                    "UPDATE ont_instances SET lifecycle_state = $2, updated_at = $3 WHERE id = $1",
                )
                .bind(*instance_id.as_uuid())
                .bind(to.as_db_str())
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;

                load_instance_head_tx(tx, instance_id).await
            })
        })
        .await
    }

    /// Create an effective-dated link (edge) between two instances (§2).
    #[allow(clippy::too_many_arguments)]
    pub async fn create_link(
        &self,
        actor: UserId,
        link_type_id: LinkTypeId,
        from_instance_id: InstanceId,
        to_instance_id: InstanceId,
        valid_from: Option<OffsetDateTime>,
        trace: TraceContext,
        occurred_at: OffsetDateTime,
    ) -> Result<InstanceLinkId, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let link_id = InstanceLinkId::new();
        let valid_from = valid_from.unwrap_or(occurred_at);
        let event = AuditEvent::new(
            Some(actor),
            AuditAction::new("ontology.link.create")?,
            "ont_links",
            link_id.to_string(),
            trace,
            occurred_at,
        )
        .with_org(org);

        with_audit::<_, InstanceLinkId, PgOntologyError>(&self.pool, event, |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    INSERT INTO ont_links (
                        id, org_id, link_type_id, from_instance_id, to_instance_id,
                        valid_from, created_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7)
                    "#,
                )
                .bind(*link_id.as_uuid())
                .bind(org_uuid)
                .bind(*link_type_id.as_uuid())
                .bind(*from_instance_id.as_uuid())
                .bind(*to_instance_id.as_uuid())
                .bind(valid_from)
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;
                Ok(link_id)
            })
        })
        .await
    }

    /// Current state (the open revision, `valid_to IS NULL`).
    pub async fn get_current(
        &self,
        instance_id: InstanceId,
    ) -> Result<InstanceState, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, InstanceState, PgOntologyError>(&self.pool, org, |tx| {
            Box::pin(async move { load_current_state_tx(tx, instance_id).await })
        })
        .await
    }

    /// As-of state: the revision whose interval covers `at`
    /// (`valid_from <= at < coalesce(valid_to, ∞)`).
    pub async fn get_as_of(
        &self,
        instance_id: InstanceId,
        at: OffsetDateTime,
    ) -> Result<InstanceState, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, InstanceState, PgOntologyError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let row = sqlx::query(
                    r#"
                    SELECT
                        i.id AS instance_id, i.object_type_id, i.title,
                        i.current_revision_id, i.lifecycle_state,
                        r.id AS revision_id, r.version, r.attributes,
                        r.valid_from, r.valid_to, r.action_type_id, r.actor,
                        r.reason, r.prev_hash, r.row_hash
                    FROM ont_instances i
                    JOIN ont_instance_revisions r ON r.instance_id = i.id
                    WHERE i.id = $1
                      AND r.valid_from <= $2
                      AND (r.valid_to IS NULL OR $2 < r.valid_to)
                    "#,
                )
                .bind(*instance_id.as_uuid())
                .bind(at)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| {
                    KernelError::not_found("instance had no revision effective at that instant")
                })?;
                instance_state_from_row(&row)
            })
        })
        .await
    }

    /// Full revision history (oldest → newest), hashes included, for chain verify.
    pub async fn history(
        &self,
        instance_id: InstanceId,
    ) -> Result<Vec<RevisionSummary>, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, Vec<RevisionSummary>, PgOntologyError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    r#"
                    SELECT id, instance_id, version, attributes, valid_from, valid_to,
                           action_type_id, actor, reason, prev_hash, row_hash
                    FROM ont_instance_revisions
                    WHERE instance_id = $1
                    ORDER BY version
                    "#,
                )
                .bind(*instance_id.as_uuid())
                .fetch_all(tx.as_mut())
                .await?;
                rows.iter().map(revision_from_row).collect()
            })
        })
        .await
    }

    /// List current-state instances of one object type (RLS-scoped).
    ///
    /// Dispatches on the type's `backing_kind` (BE-semantic-backfill): an
    /// `instance`-backed type reads its owned revision store as before; a
    /// `projected` type (registered domain table, arch §1a) reads the real
    /// domain rows directly — read path only, the domain crate's own
    /// use-case remains the sole writer.
    pub async fn list_instances(
        &self,
        object_type_id: ObjectTypeId,
    ) -> Result<Vec<InstanceState>, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, Vec<InstanceState>, PgOntologyError>(&self.pool, org, |tx| {
            Box::pin(async move {
                match backing_kind_tx(tx, object_type_id).await? {
                    console_ontology_domain::BackingKind::Instance => {
                        let rows = sqlx::query(
                            r#"
                            SELECT
                                i.id AS instance_id, i.object_type_id, i.title,
                                i.current_revision_id, i.lifecycle_state,
                                r.id AS revision_id, r.version, r.attributes,
                                r.valid_from, r.valid_to, r.action_type_id, r.actor,
                                r.reason, r.prev_hash, r.row_hash
                            FROM ont_instances i
                            JOIN ont_instance_revisions r ON r.instance_id = i.id AND r.valid_to IS NULL
                            WHERE i.object_type_id = $1
                            ORDER BY i.created_at DESC
                            "#,
                        )
                        .bind(*object_type_id.as_uuid())
                        .fetch_all(tx.as_mut())
                        .await?;
                        rows.iter().map(instance_state_from_row).collect()
                    }
                    console_ontology_domain::BackingKind::Projected => {
                        list_projected_rows_tx(tx, object_type_id).await
                    }
                }
            })
        })
        .await
    }

    /// Current-state instances of one object type that the Cedar object-row
    /// residual (arch §5d) admits — the discretionary deny-by-omission filter
    /// composed on top of the hard RLS org floor, pushed into SQL.
    ///
    /// This is the ONE place the residual meets a row, whether the caller is the
    /// list endpoint or a single-instance read. `ids` narrows the candidate set
    /// (`None` = the whole type); it is bound as a nullable `uuid[]` rather than
    /// branching the SQL text, so the residual's first placeholder is the
    /// constant `3` and the "did I use 2 or 3" placeholder-shift footgun cannot
    /// exist.
    ///
    /// `policies` is the applicable object-policy set the caller collected from the
    /// catalog for `(subject, view, object_type)`; `subject` supplies the concrete
    /// attribute values its conditions reference. Composition is
    /// `WHERE i.object_type_id = $1 AND (<ids>) AND (<residual>)`: RLS (armed on the
    /// connection) is the tenant floor the residual can only narrow, never widen.
    /// Fail-closed — no applicable permit, or any untranslatable term, yields
    /// `WHERE FALSE` (zero rows), and `forbid` policies exclude rows a permit would
    /// otherwise show (see
    /// [`console_platform_authz::cedar_pbac::residual::lower`]).
    pub async fn visible_instances(
        &self,
        object_type_id: ObjectTypeId,
        ids: Option<&[Uuid]>,
        subject: &SubjectAttrs,
        policies: &[ObjectPolicy],
    ) -> Result<Vec<InstanceState>, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        // `$1` is object_type_id and `$2` the nullable id filter, so the
        // residual's own binds start at `$3` — unconditionally.
        let residual = lower(
            LoweringTarget::Instance {
                attributes_column: "r.attributes",
            },
            subject,
            policies,
            3,
        );
        // Audited SQL-safe: the ONLY interpolated fragment is `residual.where_sql`,
        // which the residual lowering emits from gate-checked column identifiers and
        // bound placeholders only (values and instance field keys are never
        // formatted — see `residual::lower`). Every runtime value is bound below.
        let sql = sqlx::AssertSqlSafe(format!(
            r#"
            SELECT
                i.id AS instance_id, i.object_type_id, i.title,
                i.current_revision_id, i.lifecycle_state,
                r.id AS revision_id, r.version, r.attributes,
                r.valid_from, r.valid_to, r.action_type_id, r.actor,
                r.reason, r.prev_hash, r.row_hash
            FROM ont_instances i
            JOIN ont_instance_revisions r ON r.instance_id = i.id AND r.valid_to IS NULL
            WHERE i.object_type_id = $1
              AND ($2::uuid[] IS NULL OR i.id = ANY($2))
              AND ({residual})
            ORDER BY i.created_at DESC
            "#,
            residual = residual.where_sql,
        ));
        let ids = ids.map(<[Uuid]>::to_vec);
        with_org_conn::<_, Vec<InstanceState>, PgOntologyError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let mut query = sqlx::query(sql).bind(*object_type_id.as_uuid()).bind(ids);
                for value in &residual.binds {
                    query = match value {
                        SqlValue::Text(text) => query.bind(text),
                        SqlValue::Int(int) => query.bind(int),
                        SqlValue::Bool(boolean) => query.bind(boolean),
                        SqlValue::TextArray(array) => query.bind(array),
                    };
                }
                let rows = query.fetch_all(tx.as_mut()).await?;
                rows.iter().map(instance_state_from_row).collect()
            })
        })
        .await
    }

    /// [`Self::visible_instances`] over the whole object type — the list surface.
    /// Signature and semantics are byte-identical to what shipped, so the
    /// CI-required read-path proof stays a valid regression witness.
    pub async fn list_instances_filtered(
        &self,
        object_type_id: ObjectTypeId,
        subject: &SubjectAttrs,
        policies: &[ObjectPolicy],
    ) -> Result<Vec<InstanceState>, PgOntologyError> {
        self.visible_instances(object_type_id, None, subject, policies)
            .await
    }

    /// Subject-scoped aggregate over current-state instances of one object type.
    ///
    /// Built like [`Self::visible_instances`]: the same
    /// [`console_platform_authz::cedar_pbac::residual::lower`] call
    /// (`LoweringTarget::Instance`, correct bind offset) supplies the residual.
    /// Returns `GROUP BY` counts without materialising instance rows. A
    /// materialized view cannot carry a per-(subject, policy-set) residual, so
    /// this path is the capability ADR-0030 §7 row 5 actually needs.
    pub async fn aggregate_instances(
        &self,
        object_type_id: ObjectTypeId,
        group_by: AggregateGroupBy,
        subject: &SubjectAttrs,
        policies: &[ObjectPolicy],
    ) -> Result<Vec<AggregateBucket>, PgOntologyError> {
        let org = current_org().map_err(KernelError::from)?;
        let (group_sql, first_bind, prefix_binds) = match &group_by {
            AggregateGroupBy::LifecycleState => {
                let column = "i.lifecycle_state";
                if !is_safe_column_ref(column) {
                    return Err(KernelError::validation(
                        "aggregate group key failed the column allowlist",
                    )
                    .into());
                }
                (column.to_owned(), 2usize, Vec::<SqlValue>::new())
            }
            AggregateGroupBy::ObjectTypeId => {
                let column = "i.object_type_id";
                if !is_safe_column_ref(column) {
                    return Err(KernelError::validation(
                        "aggregate group key failed the column allowlist",
                    )
                    .into());
                }
                // Cast so the bucket key is text, matching lifecycle / attribute.
                (format!("{column}::text"), 2usize, Vec::new())
            }
            AggregateGroupBy::Attribute(key) => {
                if !is_safe_ident(key) {
                    return Err(KernelError::validation(
                        "aggregate attribute group key is not a safe identifier",
                    )
                    .into());
                }
                let attributes_column = "r.attributes";
                if !is_safe_column_ref(attributes_column) {
                    return Err(KernelError::validation(
                        "aggregate attributes column failed the column allowlist",
                    )
                    .into());
                }
                // Key bound at $2 (never interpolated); residual starts at $3.
                (
                    format!("({attributes_column} ->> $2)"),
                    3usize,
                    vec![SqlValue::Text(key.clone())],
                )
            }
        };

        let residual = lower(
            LoweringTarget::Instance {
                attributes_column: "r.attributes",
            },
            subject,
            policies,
            first_bind,
        );
        // Audited SQL-safe: `group_sql` is either a gate-checked column ref or
        // `(r.attributes ->> $2)` with the key bound; `residual.where_sql` is the
        // same residual::lower fragment the list path interpolates.
        let sql = sqlx::AssertSqlSafe(format!(
            r#"
            SELECT {group_sql} AS group_key, COUNT(*)::bigint AS bucket_count
            FROM ont_instances i
            JOIN ont_instance_revisions r ON r.instance_id = i.id AND r.valid_to IS NULL
            WHERE i.object_type_id = $1
              AND ({residual})
            GROUP BY 1
            ORDER BY 1 NULLS LAST
            "#,
            group_sql = group_sql,
            residual = residual.where_sql,
        ));
        with_org_conn::<_, Vec<AggregateBucket>, PgOntologyError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let mut query = sqlx::query(sql).bind(*object_type_id.as_uuid());
                for value in prefix_binds.iter().chain(residual.binds.iter()) {
                    query = match value {
                        SqlValue::Text(text) => query.bind(text),
                        SqlValue::Int(int) => query.bind(int),
                        SqlValue::Bool(boolean) => query.bind(boolean),
                        SqlValue::TextArray(array) => query.bind(array),
                    };
                }
                let rows = query.fetch_all(tx.as_mut()).await?;
                rows.iter()
                    .map(|row| {
                        Ok(AggregateBucket {
                            key: row.try_get("group_key")?,
                            count: row.try_get("bucket_count")?,
                        })
                    })
                    .collect()
            })
        })
        .await
    }

    /// §2 search-around: bounded outgoing BFS over live (`valid_to IS NULL`) links
    /// up to `depth` hops, optionally filtered to one link type.
    pub async fn traverse(
        &self,
        root: InstanceId,
        link_type_id: Option<LinkTypeId>,
        depth: u32,
    ) -> Result<TraversalGraph, PgOntologyError> {
        let depth = depth.min(MAX_TRAVERSAL_DEPTH);
        let org = current_org().map_err(KernelError::from)?;
        let link_type_uuid = link_type_id.map(|id| *id.as_uuid());
        with_org_conn::<_, TraversalGraph, PgOntologyError>(&self.pool, org, |tx| {
            Box::pin(async move {
                let root_uuid = *root.as_uuid();
                // depth per discovered node (root = 0); doubles as the visited set.
                let mut node_depth: BTreeMap<Uuid, u32> = BTreeMap::new();
                node_depth.insert(root_uuid, 0);
                let mut edges: Vec<TraversalEdge> = Vec::new();
                let mut frontier: Vec<Uuid> = vec![root_uuid];

                for hop in 0..depth {
                    if frontier.is_empty() {
                        break;
                    }
                    let rows = sqlx::query(
                        r#"
                        SELECT id, link_type_id, from_instance_id, to_instance_id
                        FROM ont_links
                        WHERE from_instance_id = ANY($1)
                          AND valid_to IS NULL
                          AND ($2::uuid IS NULL OR link_type_id = $2)
                        "#,
                    )
                    .bind(&frontier)
                    .bind(link_type_uuid)
                    .fetch_all(tx.as_mut())
                    .await?;

                    let mut next: Vec<Uuid> = Vec::new();
                    for row in &rows {
                        let to: Uuid = row.try_get("to_instance_id")?;
                        edges.push(TraversalEdge {
                            id: InstanceLinkId::from_uuid(row.try_get("id")?),
                            link_type_id: LinkTypeId::from_uuid(row.try_get("link_type_id")?),
                            from_instance_id: InstanceId::from_uuid(
                                row.try_get("from_instance_id")?,
                            ),
                            to_instance_id: InstanceId::from_uuid(to),
                        });
                        if let std::collections::btree_map::Entry::Vacant(e) = node_depth.entry(to)
                        {
                            e.insert(hop + 1);
                            next.push(to);
                        }
                    }
                    frontier = next;
                }

                // Hydrate node payloads for every visited instance in one query.
                let ids: Vec<Uuid> = node_depth.keys().copied().collect();
                let node_rows = sqlx::query(
                    r#"
                    SELECT id, object_type_id, title, lifecycle_state
                    FROM ont_instances
                    WHERE id = ANY($1)
                    "#,
                )
                .bind(&ids)
                .fetch_all(tx.as_mut())
                .await?;

                let mut nodes: Vec<TraversalNode> = node_rows
                    .iter()
                    .map(|row| {
                        let id: Uuid = row.try_get("id")?;
                        Ok::<_, PgOntologyError>(TraversalNode {
                            instance_id: InstanceId::from_uuid(id),
                            object_type_id: ObjectTypeId::from_uuid(row.try_get("object_type_id")?),
                            title: row.try_get("title")?,
                            lifecycle_state: InstanceLifecycleState::from_db_str(
                                row.try_get("lifecycle_state")?,
                            )?,
                            depth: node_depth.get(&id).copied().unwrap_or(0),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                nodes.sort_by_key(|n| (n.depth, *n.instance_id.as_uuid()));

                Ok(TraversalGraph { root, nodes, edges })
            })
        })
        .await
    }
}

// ===========================================================================
// In-transaction append helpers.
//
// These carry the whole create/revise logic (validate → fixity → insert) but run
// on a caller-supplied transaction WITHOUT opening their own `with_audit`, so the
// L-ONT-actions execute path can dispatch a revision inside the SAME writeback tx
// that re-checks its §16 gates and writes the action's audit row (TOCTOU-safe,
// one atomic mutation). The public `create_instance`/`stage_revision` methods
// above are thin `with_audits` wrappers over exactly these fns.
// ===========================================================================

/// Append revision v1 for a new instance on `tx`. The caller's tx must already
/// have `app.current_org` armed (RLS); `org` is the tenant the rows are stamped
/// with. Returns the created current state.
pub async fn create_instance_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    actor: UserId,
    org: OrgId,
    input: CreateInstance,
    occurred_at: OffsetDateTime,
) -> Result<InstanceState, PgOntologyError> {
    if input.title.trim().is_empty() {
        return Err(KernelError::validation("instance title is required").into());
    }
    let org_uuid = *org.as_uuid();
    let instance_id = InstanceId::new();
    let revision_id = InstanceRevisionId::new();
    let valid_from = input.valid_from.unwrap_or(occurred_at);

    let mut attributes = require_object(&input.attributes)?;
    require_instance_backed_object_type(tx, input.object_type_id).await?;
    let props = load_property_defs_tx(tx, input.object_type_id).await?;
    // Before validation, so the derived value is shape-checked like any other,
    // and before `canonical_revision` below, so it is inside the fixity hash.
    resolve_derived_attributes_tx(tx, &props, &mut attributes, valid_from).await?;
    validate_attributes(&props, &attributes)?;

    let canonical = canonical_revision(
        instance_id,
        1,
        &attributes,
        valid_from,
        input.action_type_id,
        actor,
        input.reason.as_deref(),
    );
    let row_hash = revision_row_hash(GENESIS_HASH, &canonical)?;

    insert_instance_head_tx(
        tx,
        instance_id,
        org_uuid,
        input.object_type_id,
        input.title.trim(),
        occurred_at,
    )
    .await?;
    insert_revision_tx(
        tx,
        revision_id,
        org_uuid,
        instance_id,
        1,
        &attributes,
        valid_from,
        input.action_type_id,
        actor,
        input.reason.as_deref(),
        GENESIS_HASH,
        &row_hash,
        occurred_at,
    )
    .await?;
    set_head_pointer_tx(tx, instance_id, revision_id, occurred_at).await?;
    // AFTER the head insert: `ont_links`'s `(from_instance_id, org_id)` FK is
    // immediate (`0155:76`), so the edge cannot precede the row it points from.
    sync_property_links_tx(
        tx,
        org_uuid,
        input.object_type_id,
        instance_id,
        &props,
        &attributes,
        valid_from,
    )
    .await?;
    load_current_state_tx(tx, instance_id).await
}

/// Stage a v+1 revision of an existing instance on `tx` (close the current
/// interval, append the chained revision). Same tenant/RLS contract as
/// [`create_instance_in_tx`].
pub async fn stage_revision_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    actor: UserId,
    org: OrgId,
    instance_id: InstanceId,
    input: StageRevision,
    occurred_at: OffsetDateTime,
) -> Result<InstanceState, PgOntologyError> {
    let org_uuid = *org.as_uuid();
    let revision_id = InstanceRevisionId::new();
    let valid_from = input.valid_from.unwrap_or(occurred_at);
    let mut attributes = require_object(&input.attributes)?;

    // Lock the head; a disposed instance is terminal and un-revisable.
    let head = sqlx::query(
        "SELECT object_type_id, lifecycle_state FROM ont_instances WHERE id = $1 FOR UPDATE",
    )
    .bind(*instance_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("instance was not found"))?;
    let object_type_id = ObjectTypeId::from_uuid(head.try_get("object_type_id")?);
    let state = InstanceLifecycleState::from_db_str(head.try_get("lifecycle_state")?)?;
    if state == InstanceLifecycleState::Disposed {
        return Err(KernelError::conflict("a disposed instance cannot be revised").into());
    }

    let props = load_property_defs_tx(tx, object_type_id).await?;
    // Both revision writers derive, not just `create_instance_in_tx`: a caller
    // may send `gross_total` on an edit, and only the second write would keep it.
    resolve_derived_attributes_tx(tx, &props, &mut attributes, valid_from).await?;
    validate_attributes(&props, &attributes)?;

    // Lock the current open revision to chain onto it.
    let cur = sqlx::query(
        r#"
        SELECT id, version, valid_from, row_hash
        FROM ont_instance_revisions
        WHERE instance_id = $1 AND valid_to IS NULL
        FOR UPDATE
        "#,
    )
    .bind(*instance_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("instance has no open revision"))?;
    let cur_version: i64 = cur.try_get("version")?;
    let cur_valid_from: OffsetDateTime = cur.try_get("valid_from")?;
    let cur_row_hash: String = cur.try_get("row_hash")?;
    if valid_from <= cur_valid_from {
        return Err(KernelError::validation(
            "new revision valid_from must be after the current revision's valid_from",
        )
        .into());
    }
    let next_version = cur_version + 1;

    let canonical = canonical_revision(
        instance_id,
        next_version,
        &attributes,
        valid_from,
        input.action_type_id,
        actor,
        input.reason.as_deref(),
    );
    let row_hash = revision_row_hash(&cur_row_hash, &canonical)?;

    // Close the prior interval (the append-only trigger permits only a
    // NULL -> value flip of valid_to; all fixity columns stay immutable).
    sqlx::query(
        "UPDATE ont_instance_revisions SET valid_to = $2 WHERE instance_id = $1 AND valid_to IS NULL",
    )
    .bind(*instance_id.as_uuid())
    .bind(valid_from)
    .execute(tx.as_mut())
    .await?;

    insert_revision_tx(
        tx,
        revision_id,
        org_uuid,
        instance_id,
        next_version,
        &attributes,
        valid_from,
        input.action_type_id,
        actor,
        input.reason.as_deref(),
        &cur_row_hash,
        &row_hash,
        occurred_at,
    )
    .await?;
    set_head_pointer_tx(tx, instance_id, revision_id, occurred_at).await?;
    // The revision's `valid_from`, never `occurred_at`: a wall-clock stamp on an
    // edge effective at T0 breaks every effective-dated read.
    sync_property_links_tx(
        tx,
        org_uuid,
        object_type_id,
        instance_id,
        &props,
        &attributes,
        valid_from,
    )
    .await?;
    load_current_state_tx(tx, instance_id).await
}

/// Materialise this revision's DECLARED property -> link edges (§2), on `tx`.
///
/// A relationship the business declares must be TRAVERSABLE, not merely named in
/// the attribute bag: a `reference` property round-trips for free, and nothing
/// downstream can walk it. The binding is therefore data the object type carries,
/// resolved generically here — a property whose `ont_property_defs.config` holds
///
/// ```json
/// {"link": {"stable_key": "<ont_link_types.stable_key on this type>",
///           "to_type": "<referent ont_object_types.stable_key>"}}
/// ```
///
/// gets a real `ont_links` edge from THIS instance (child) to its referent
/// (parent) on every revision. No type is named in this function, and no type
/// needs engine code: a new one contributes a JSON blob to its own declaration.
///
/// `to_type` is matched on `stable_key`, never on `object_type_id`:
/// `ont_object_types` is one row per `(org, stable_key, schema_version)`
/// (`0152:32`), so id equality silently starts failing for instances created
/// under an earlier version the moment the type is revised.
///
/// Referential integrity is pre-checked in Rust rather than left to the FK,
/// because a 23503 is not mapped by `ontology_database_kernel_error` and reaches
/// callers as an opaque `PgOntologyError::Db` (a 500), not the 4xx a bad
/// reference deserves.
async fn sync_property_links_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: Uuid,
    object_type_id: ObjectTypeId,
    instance_id: InstanceId,
    props: &[PropDef],
    attributes: &serde_json::Value,
    valid_from: OffsetDateTime,
) -> Result<(), PgOntologyError> {
    // Phase 1 — resolve EVERY binding before writing anything. Two properties may
    // share one link type; closing and inserting per property would let the second
    // property's close sweep away the edge the first just wrote.
    let mut link_type_ids: Vec<Uuid> = Vec::new();
    let mut edges: Vec<(Uuid, Uuid)> = Vec::new();
    for prop in props {
        let Some(link) = prop.config.get("link") else {
            continue;
        };
        // Fail closed on a malformed declaration. Skipping it would surface as an
        // empty traversal, reading as "the mechanism is broken" rather than "the
        // declaration has a typo".
        let (Some(stable_key), Some(to_type)) = (
            link.get("stable_key").and_then(serde_json::Value::as_str),
            link.get("to_type").and_then(serde_json::Value::as_str),
        ) else {
            return Err(KernelError::validation(format!(
                "property '{}' declares config.link without string `stable_key` and `to_type`",
                prop.key
            ))
            .into());
        };

        let link_type_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM ont_link_types WHERE object_type_id = $1 AND stable_key = $2",
        )
        .bind(*object_type_id.as_uuid())
        .bind(stable_key)
        .fetch_optional(tx.as_mut())
        .await?
        .ok_or_else(|| {
            KernelError::validation(format!(
                "property '{}' declares link type '{stable_key}', which this object type does not \
                 define",
                prop.key
            ))
        })?;
        link_type_ids.push(link_type_id);

        let referents: Vec<&str> = match attributes.get(&prop.key) {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(serde_json::Value::String(one)) => vec![one.as_str()],
            Some(serde_json::Value::Array(many)) => many
                .iter()
                .map(|item| {
                    item.as_str().ok_or_else(|| {
                        KernelError::validation(format!(
                            "linked attribute '{}' must hold instance id strings",
                            prop.key
                        ))
                    })
                })
                .collect::<Result<_, KernelError>>()?,
            Some(_) => {
                return Err(KernelError::validation(format!(
                    "linked attribute '{}' must be an instance id string, an array of them, or null",
                    prop.key
                ))
                .into());
            }
        };

        for referent in referents {
            let to_instance_id: Uuid = referent.parse().map_err(|_| {
                KernelError::validation(format!(
                    "linked attribute '{}' is not a valid instance id",
                    prop.key
                ))
            })?;
            // RLS also renders a cross-tenant instance missing here, which is the
            // correct answer: it does not exist for this tenant.
            let referent_type: Option<String> = sqlx::query_scalar(
                r#"
                SELECT t.stable_key
                FROM ont_instances i
                JOIN ont_object_types t ON t.id = i.object_type_id
                WHERE i.id = $1
                "#,
            )
            .bind(to_instance_id)
            .fetch_optional(tx.as_mut())
            .await?;
            match referent_type {
                None => {
                    return Err(KernelError::not_found(format!(
                        "linked attribute '{}' references an instance that does not exist",
                        prop.key
                    ))
                    .into());
                }
                Some(actual) if actual != to_type => {
                    return Err(KernelError::validation(format!(
                        "linked attribute '{}' must reference a '{to_type}', not a '{actual}'",
                        prop.key
                    ))
                    .into());
                }
                Some(_) => {}
            }
            edges.push((link_type_id, to_instance_id));
        }
    }

    // Phase 2 — a type that declares no link touches `ont_links` at all. Every
    // built-in catalog type is in this branch.
    if link_type_ids.is_empty() {
        return Ok(());
    }

    // Phase 3 — read the live edge set and act only on the DIFFERENCE.
    //
    // Closing and reopening every declared edge on every revision would be far
    // simpler, and the conformance suite cannot tell the two apart: `traverse`
    // reads `valid_to IS NULL` only, so the graph looks identical either way. It
    // would also be a lie. `ont_links` is effective-dated history, and an
    // unconditional sweep records a close and a reopen at T1 for a referent that
    // never changed — a type carrying two link properties (`employment` has
    // exactly that) would report a position change on every unrelated transfer.
    // The whole claim of this system is that the history is true, so the edge is
    // touched only when the referent set actually differs.
    let live: Vec<(Uuid, Uuid)> = sqlx::query(
        r#"
        SELECT link_type_id, to_instance_id
        FROM ont_links
        WHERE from_instance_id = $1 AND link_type_id = ANY($2) AND valid_to IS NULL
        "#,
    )
    .bind(*instance_id.as_uuid())
    .bind(link_type_ids.as_slice())
    .fetch_all(tx.as_mut())
    .await?
    .into_iter()
    .map(|row| Ok((row.try_get("link_type_id")?, row.try_get("to_instance_id")?)))
    .collect::<Result<_, sqlx::Error>>()?;

    // A declaration repeated across two properties must not open two edges.
    let mut desired = edges;
    desired.sort_unstable();
    desired.dedup();

    let stale: Vec<(Uuid, Uuid)> = live
        .iter()
        .filter(|edge| !desired.contains(edge))
        .copied()
        .collect();
    let fresh: Vec<(Uuid, Uuid)> = desired
        .iter()
        .filter(|edge| !live.contains(edge))
        .copied()
        .collect();

    // Close BEFORE inserting: an insert-first order would let the sweep close the
    // edge it just opened. `valid_from < $4` mirrors `CHECK (valid_to IS NULL OR
    // valid_to > valid_from)` (`0155:75`) — closing an edge at the instant it
    // opened raises an unmappable 23514 that aborts the whole writeback
    // transaction. Unreachable today, because `stage_revision_in_tx` already
    // rejects a `valid_from` at or before the current revision's; kept because
    // without it a violated invariant takes down the transaction, and with it the
    // violation leaves a detectable stale edge instead.
    if !stale.is_empty() {
        let (stale_types, stale_referents): (Vec<Uuid>, Vec<Uuid>) = stale.into_iter().unzip();
        sqlx::query(
            r#"
            UPDATE ont_links
            SET valid_to = $4
            WHERE from_instance_id = $1
              AND valid_to IS NULL
              AND valid_from < $4
              AND (link_type_id, to_instance_id)
                  IN (SELECT * FROM UNNEST($2::UUID[], $3::UUID[]))
            "#,
        )
        .bind(*instance_id.as_uuid())
        .bind(stale_types.as_slice())
        .bind(stale_referents.as_slice())
        .bind(valid_from)
        .execute(tx.as_mut())
        .await?;
    }

    // Phase 4 — open the edges that are genuinely new. Direction is CHILD ->
    // PARENT: `from` is the instance carrying the property, `to` is the referent.
    // `id`/`created_at` take their column defaults.
    for (link_type_id, to_instance_id) in fresh {
        sqlx::query(
            r#"
            INSERT INTO ont_links (
                org_id, link_type_id, from_instance_id, to_instance_id, valid_from
            )
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(org_uuid)
        .bind(link_type_id)
        .bind(*instance_id.as_uuid())
        .bind(to_instance_id)
        .bind(valid_from)
        .execute(tx.as_mut())
        .await?;
    }
    Ok(())
}

/// Materialise this revision's DECLARED derived attributes (§2), on `tx`.
///
/// Some values the business needs are not sent, they are COMPUTED from other
/// instances: a pay run's gross total is the sum of the referenced employments'
/// salaries. `apply_edits` resolves an edit to a constant or a named param and
/// does no arithmetic, so a declarative type cannot produce that number — and
/// per-type arithmetic in the engine would end the declarative model outright.
/// The computation is therefore data the object type carries, resolved
/// generically here, exactly as the property -> link binding is: a property
/// whose `ont_property_defs.config` holds
///
/// ```json
/// {"derive": {"op": "sum", "over": "<id-array property on this type>",
///             "of": "<integer property on the referent>",
///             "to_type": "<referent ont_object_types.stable_key>"}}
/// ```
///
/// is computed on every revision write. No type is named in this function, and a
/// new derived property contributes a JSON blob to its own declaration.
///
/// `op` is stated rather than implied, and an unimplemented one is refused: a
/// money field whose operation lives only in engine source is not a
/// declaration, and a `_ => sum` default would let a typo silently compute the
/// wrong thing.
///
/// Runs BEFORE `validate_attributes`, and therefore before `canonical_revision`.
/// Before validation, so the computed value is shape-checked by the same
/// validator every caller-sent value goes through (a `derive` mis-declared onto
/// a `text` property is then a 4xx, not a stored lie). Before canonicalization,
/// because the fixity chain hashes the stored bag: a value written afterwards
/// would make `verify_chain` report the revision as the first chain break,
/// indistinguishable from tamper.
///
/// The write is UNCONDITIONAL. The auto-attached `create` action emits one edit
/// per declared property (`0165_ontology_object_type_key_revisions.sql:1031`)
/// and `apply_edits` resolves an absent param to `Value::Null` and inserts it
/// (`application/src/lib.rs:288`), so the key always arrives — as null when the
/// caller behaved, and as whatever they sent when they did not. "Caller sent
/// null" and "the action filled null" are therefore indistinguishable, so
/// filling only an absent key would never fire; and since `params_schema` lists
/// the derived property, a caller CAN send one. Overwriting is what makes the
/// value derived rather than merely defaulted.
///
/// Referents are read AS OF this revision's `valid_from`, never at the head. A
/// pay run for a period is a fact about that period: a head read would let an
/// unrelated later raise change a total already sealed into the hash chain, so
/// the number could no longer be reconstructed from the effective-dated history.
/// Same predicate as [`PgInstanceStore::get_as_of`], and the same argument
/// `sync_property_links_tx`'s call sites make for stamping edges with
/// `valid_from` rather than `occurred_at`.
///
/// Like its sibling, referential failures are pre-checked in Rust and returned
/// as `KernelError`, never left to a SQLSTATE that would surface as an opaque
/// 500. A referent that is missing, cross-tenant (RLS renders it missing), of
/// the wrong type, or has no revision effective at `valid_from` FAILS the write.
/// Dropping its term instead would store a smaller, entirely plausible total.
async fn resolve_derived_attributes_tx(
    tx: &mut Transaction<'_, Postgres>,
    props: &[PropDef],
    attributes: &mut serde_json::Value,
    valid_from: OffsetDateTime,
) -> Result<(), PgOntologyError> {
    for prop in props {
        let Some(derive) = prop.config.get("derive") else {
            continue;
        };
        let (Some(op), Some(over), Some(of), Some(to_type)) = (
            derive.get("op").and_then(serde_json::Value::as_str),
            derive.get("over").and_then(serde_json::Value::as_str),
            derive.get("of").and_then(serde_json::Value::as_str),
            derive.get("to_type").and_then(serde_json::Value::as_str),
        ) else {
            return Err(KernelError::validation(format!(
                "property '{}' declares config.derive without string `op`, `over`, `of` and \
                 `to_type`",
                prop.key
            ))
            .into());
        };
        if op != "sum" {
            return Err(KernelError::validation(format!(
                "property '{}' declares derivation op '{op}', which this engine does not \
                 implement",
                prop.key
            ))
            .into());
        }

        // `over` is the one declaration field the bag read below cannot police:
        // a key the type does not declare reads as absent, which is
        // indistinguishable from declared-but-empty, so the sum would be 0 and
        // that 0 would be hash-sealed as truth. An empty ARRAY is a legitimate 0
        // (a pay run with no members); an UNDECLARED `over` is a declaration bug
        // and must be a 4xx, the same argument the closed `op` match wins above.
        if !props.iter().any(|p| p.key == over) {
            return Err(KernelError::validation(format!(
                "property '{}' sums over '{over}', which this object type does not declare",
                prop.key
            ))
            .into());
        }

        // The source is a collection, so unlike the link binding a bare scalar is
        // not accepted: summing over one id is `[id]`.
        let ids: Vec<Uuid> = match attributes.get(over) {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(serde_json::Value::Array(many)) => many
                .iter()
                .map(|item| {
                    item.as_str().and_then(|id| id.parse().ok()).ok_or_else(|| {
                        KernelError::validation(format!(
                            "derived property '{}' sums over '{over}', which must hold \
                                 instance id strings",
                            prop.key
                        ))
                    })
                })
                .collect::<Result<_, KernelError>>()?,
            Some(_) => {
                return Err(KernelError::validation(format!(
                    "derived property '{}' sums over '{over}', which must be an array of \
                     instance ids or null",
                    prop.key
                ))
                .into());
            }
        };

        // One round trip for N referents: an id-at-a-time loop would issue N
        // queries inside the writeback transaction, which already holds the head
        // `FOR UPDATE`. A plain SELECT takes no row locks, so it cannot deadlock
        // against them. `= ANY` is SET semantics — the sum walks the DECLARED id
        // list against this map, so a bag naming the same referent twice pays it
        // twice rather than being silently de-duplicated.
        let mut found: HashMap<Uuid, (String, serde_json::Value)> = HashMap::new();
        for row in sqlx::query(
            r#"
            SELECT i.id, t.stable_key, r.attributes
            FROM ont_instances i
            JOIN ont_object_types t ON t.id = i.object_type_id
            JOIN ont_instance_revisions r
              ON r.instance_id = i.id
             AND r.valid_from <= $2
             AND (r.valid_to IS NULL OR $2 < r.valid_to)
            WHERE i.id = ANY($1)
            "#,
        )
        .bind(ids.as_slice())
        .bind(valid_from)
        .fetch_all(tx.as_mut())
        .await?
        {
            found.insert(
                row.try_get("id")?,
                (row.try_get("stable_key")?, row.try_get("attributes")?),
            );
        }

        let mut total: i64 = 0;
        for id in &ids {
            let Some((actual, referent)) = found.get(id) else {
                return Err(KernelError::not_found(format!(
                    "derived property '{}' sums over '{over}', which references instance {id}, \
                     with no revision effective at this instant",
                    prop.key
                ))
                .into());
            };
            if actual != to_type {
                return Err(KernelError::validation(format!(
                    "derived property '{}' sums over '{over}', which must reference a \
                     '{to_type}', not a '{actual}'",
                    prop.key
                ))
                .into());
            }
            let term = referent
                .get(of)
                .and_then(serde_json::Value::as_i64)
                .ok_or_else(|| {
                    KernelError::validation(format!(
                        "derived property '{}' sums '{of}', which referent {id} does not carry \
                         as an integer",
                        prop.key
                    ))
                })?;
            // Never a bare `+`: an overflow panic inside the writeback
            // transaction is not a mappable 4xx, and money sums are where it
            // happens.
            total = total.checked_add(term).ok_or_else(|| {
                KernelError::validation(format!("derived property '{}' overflowed", prop.key))
            })?;
        }

        attributes
            .as_object_mut()
            .ok_or_else(|| KernelError::validation("attributes must be a JSON object"))?
            .insert(prop.key.clone(), serde_json::Value::from(total));
    }
    Ok(())
}

// ===========================================================================
// Fixity (§1b) — per-(org,instance) SHA-256 hash chain.
//
// No shared canonicalizer helper exists in the tree today (the crate the arch
// doc names, `crates/compliance/integrity`, is the governance-findings engine,
// not a hash-chain), so this is a self-contained canonicalization. `valid_to` is
// deliberately EXCLUDED — it legitimately changes when an interval is closed, so
// it is metadata, not fixity-covered content.
//
// KNOWN DEFECT, PRE-EXISTING, NOT INTRODUCED BY THE DERIVATION ABOVE. This
// comment used to claim "serde_json serializes object keys in sorted order (no
// `preserve_order` feature in the workspace → BTreeMap), giving a stable byte
// string". That is FALSE, and it is false in the direction that matters: a
// comment asserting a hash chain is sound is exactly what stops the next reader
// from checking. Measured with `cargo tree -e features -i serde_json`:
//
//     serde_json feature "preserve_order"  ←  cedar-policy-core v4.11.2
//
// which reaches this crate through `console-platform-authz`. `serde_json::Map`
// is therefore an insertion-ordered IndexMap, not a BTreeMap, so this
// canonicalization is order-DEPENDENT: an attribute bag round-tripped through
// the database can serialize to different bytes than the bag that was hashed,
// and `verify_chain` reports a break on untampered data. It bites any object
// with two or more attribute keys whose insertion order differs from read-back
// order.
//
// This is why `company_conformance.rs` asserts chain LINKAGE (each revision's
// `prev_hash` matching its predecessor's `row_hash`) rather than calling
// `verify_chain`, and says so at its own :103-129. The suite is green because
// it does not recompute hashes — not because recomputation would succeed.
//
// The fix is to sort keys explicitly here rather than rely on the map type, and
// it is deliberately NOT made in this change: it alters every existing
// `row_hash`, so it needs a re-seal or migration decision and an audit-chain
// owner, not a drive-by edit inside a lane. Escalated rather than settled.
// ===========================================================================

fn canonical_revision(
    instance_id: InstanceId,
    version: i64,
    attributes: &serde_json::Value,
    valid_from: OffsetDateTime,
    action_type_id: Option<ActionTypeId>,
    actor: UserId,
    reason: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "instance_id": instance_id.to_string(),
        "version": version,
        "attributes": attributes,
        // nanosecond epoch as a string: deterministic, no float, no format fallible-ness.
        "valid_from": valid_from.unix_timestamp_nanos().to_string(),
        "action_type_id": action_type_id.map(|a| a.to_string()),
        "actor": actor.to_string(),
        "reason": reason,
    })
}

fn revision_row_hash(
    prev_hash: &str,
    canonical: &serde_json::Value,
) -> Result<String, PgOntologyError> {
    let bytes = serde_json::to_vec(canonical).map_err(|e| {
        KernelError::validation(format!("canonical revision did not serialize: {e}"))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(prev_hash.as_bytes());
    hasher.update(&bytes);
    Ok(hex::encode(hasher.finalize()))
}

/// Recompute the chain over an ordered revision history and report the first
/// break (or `None` if every `row_hash` and `prev_hash` link verifies). A tamper
/// of any fixity-covered column makes the recomputed hash diverge here.
#[must_use]
pub fn verify_chain(revisions: &[RevisionSummary]) -> Option<InstanceRevisionId> {
    let mut prev = GENESIS_HASH.to_owned();
    for rev in revisions {
        if rev.prev_hash != prev {
            return Some(rev.id);
        }
        let canonical = canonical_revision(
            rev.instance_id,
            rev.version,
            &rev.attributes,
            rev.valid_from,
            rev.action_type_id,
            rev.actor.unwrap_or_else(|| UserId::from_uuid(Uuid::nil())),
            rev.reason.as_deref(),
        );
        match revision_row_hash(&prev, &canonical) {
            Ok(recomputed) if recomputed == rev.row_hash => prev = rev.row_hash.clone(),
            _ => return Some(rev.id),
        }
    }
    None
}

// ===========================================================================
// Attribute validation vs the property schema (§1b — bag, not EAV).
// ===========================================================================

struct PropDef {
    key: String,
    field_kind: FieldKind,
    required: bool,
    /// The type's own declaration data. Carries the property -> link binding read
    /// by [`sync_property_links_tx`]; `NOT NULL DEFAULT '{}'` (`0152:56`), so no
    /// `Option`.
    config: serde_json::Value,
}

async fn load_property_defs_tx(
    tx: &mut Transaction<'_, Postgres>,
    object_type_id: ObjectTypeId,
) -> Result<Vec<PropDef>, PgOntologyError> {
    let rows = sqlx::query(
        "SELECT key, type, required, config FROM ont_property_defs WHERE object_type_id = $1",
    )
    .bind(*object_type_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await?;
    rows.iter()
        .map(|row| {
            let field_type: String = row.try_get("type")?;
            Ok(PropDef {
                key: row.try_get("key")?,
                field_kind: FieldKind::parse(&field_type),
                required: row.try_get("required")?,
                config: row.try_get("config")?,
            })
        })
        .collect()
}

async fn require_instance_backed_object_type(
    tx: &mut Transaction<'_, Postgres>,
    object_type_id: ObjectTypeId,
) -> Result<(), PgOntologyError> {
    let kind: Option<String> =
        sqlx::query_scalar("SELECT backing_kind FROM ont_object_types WHERE id = $1")
            .bind(*object_type_id.as_uuid())
            .fetch_optional(tx.as_mut())
            .await?;
    match kind.as_deref() {
        Some("instance") => Ok(()),
        Some(_) => Err(KernelError::validation(
            "instances can only be created for instance-backed object types",
        )
        .into()),
        None => Err(KernelError::not_found("object type was not found").into()),
    }
}

// ===========================================================================
// Projected-type read path (BE-semantic-backfill, arch §1a).
//
// A `projected` object type owns no store of its own — its "instances" are
// rows of a real domain table, registered via `ont_object_types.backing_table`
// + each property's `backing_column`. This is a READ-ONLY view: the domain
// crate's own use-case remains the sole writer (arch §9.3), so there is no
// create/stage path here, only list.
// ===========================================================================

/// This version's `backing_kind`. `None` (missing row) is `NotFound`, never a
/// silent default — a caller must never treat an unknown type as either kind.
async fn backing_kind_tx(
    tx: &mut Transaction<'_, Postgres>,
    object_type_id: ObjectTypeId,
) -> Result<console_ontology_domain::BackingKind, PgOntologyError> {
    let kind: Option<String> =
        sqlx::query_scalar("SELECT backing_kind FROM ont_object_types WHERE id = $1")
            .bind(*object_type_id.as_uuid())
            .fetch_optional(tx.as_mut())
            .await?;
    match kind {
        Some(kind) => Ok(console_ontology_domain::BackingKind::from_db_str(&kind)?),
        None => Err(KernelError::not_found("object type was not found").into()),
    }
}

/// The real domain tables a `projected` type may back onto. A `backing_table`
/// value that doesn't match one of these literal names is refused — the
/// match arm returns the COMPILED-IN literal, so a caller-supplied string is
/// never itself interpolated into SQL as a table identifier (defense in depth
/// beneath the fact that `backing_table` is only ever admin-authored via
/// [`crate::seed`], never end-user input).
///
/// `ponytail:` a fixed allowlist, not a registry-driven one; add a table here
/// when a future lane projects a new one — the DB-level identifier-injection
/// guard belongs in code review either way, not config.
fn allowlisted_projected_table(name: &str) -> Option<&'static str> {
    Some(match name {
        "work_orders" => "work_orders",
        "employees" => "employees",
        "registry_equipment" => "registry_equipment",
        "registry_customers" => "registry_customers",
        "registry_sites" => "registry_sites",
        "support_tickets" => "support_tickets",
        "docs_evidence_objects" => "docs_evidence_objects",
        "compliance_obligations" => "compliance_obligations",
        "compliance_regulation_impacts" => "compliance_regulation_impacts",
        "compliance_frameworks" => "compliance_frameworks",
        "leave_requests" => "leave_requests",
        "workflow_definitions" => "workflow_definitions",
        "messenger_threads" => "messenger_threads",
        "email_messages" => "email_messages",
        "gov_approval_requests" => "gov_approval_requests",
        _ => return None,
    })
}

/// A lower-snake-case SQL identifier: the shape every real column/table name
/// in this schema has. Column names come from `ont_property_defs.backing_column`
/// (admin-authored, arch §18) — checked here so a malformed value fails
/// closed instead of ever reaching raw SQL text.
fn is_safe_ident(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && value.len() <= 63
}

/// Gate-check a `column` or `table.column` reference before splicing it into
/// SQL — same shape as `console_platform_authz::is_safe_column` (crate-private;
/// duplicated here so the aggregate path does not widen that crate's surface).
fn is_safe_column_ref(raw: &str) -> bool {
    let mut segments = raw.split('.');
    let first = segments.next();
    let second = segments.next();
    let too_many = segments.next().is_some();
    match (first, second, too_many) {
        (Some(column), None, false) => is_safe_ident(column),
        (Some(table), Some(column), false) => is_safe_ident(table) && is_safe_ident(column),
        _ => false,
    }
}

/// List current rows of a `projected` type's backing table as synthetic
/// [`InstanceState`]s: `id` = the row's primary key, `attributes` = each
/// registered property's backing column (stringified — every Postgres value
/// type round-trips through `::TEXT`, sidestepping a per-type decode matrix),
/// `valid_from` = the row's `created_at`. There is no owned revision store for
/// a projected type, so `version` is always 1 and the fixity hashes are empty
/// (the domain's own `audit_events` before/after snapshots are the real
/// history, per arch §1a — v1 gives current-state only).
async fn list_projected_rows_tx(
    tx: &mut Transaction<'_, Postgres>,
    object_type_id: ObjectTypeId,
) -> Result<Vec<InstanceState>, PgOntologyError> {
    let meta = sqlx::query(
        "SELECT backing_table, primary_key_property, title_property_key \
         FROM ont_object_types WHERE id = $1",
    )
    .bind(*object_type_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("object type was not found"))?;

    let backing_table: Option<String> = meta.try_get("backing_table")?;
    let table = backing_table
        .as_deref()
        .and_then(allowlisted_projected_table)
        .ok_or_else(|| {
            KernelError::validation("projected object type has no allowlisted backing table")
        })?;

    let pk_column: Option<String> = meta.try_get("primary_key_property")?;
    let pk_column = pk_column.filter(|c| is_safe_ident(c)).ok_or_else(|| {
        KernelError::validation("projected object type has no primary_key_property")
    })?;
    let title_column: Option<String> = meta.try_get("title_property_key")?;

    let prop_rows = sqlx::query(
        "SELECT key, backing_column FROM ont_property_defs \
         WHERE object_type_id = $1 AND backing_column IS NOT NULL ORDER BY key",
    )
    .bind(*object_type_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await?;

    let mut columns: Vec<(String, String)> = Vec::with_capacity(prop_rows.len());
    for row in &prop_rows {
        let key: String = row.try_get("key")?;
        let column: String = row.try_get("backing_column")?;
        if !is_safe_ident(&column) {
            return Err(KernelError::validation(format!(
                "property '{key}' has an unsafe backing_column"
            ))
            .into());
        }
        columns.push((key, column));
    }

    let select_list = columns
        .iter()
        .map(|(_, column)| format!("{column}::TEXT AS \"attr__{column}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let select_list = if select_list.is_empty() {
        String::new()
    } else {
        format!(", {select_list}")
    };
    // ponytail: a flat LIMIT, no pagination — this lane proves the read path;
    // add cursor pagination when a console list view needs more than 200 rows.
    let sql = format!(
        "SELECT {pk_column}::TEXT AS __pk, created_at AS __created_at{select_list} \
         FROM {table} ORDER BY created_at DESC LIMIT 200"
    );
    // SAFETY: `sql` interpolates only (a) `table`, the COMPILED-IN literal an
    // allowlist match returned (never the caller-supplied string itself) and
    // (b) column names that passed `is_safe_ident` above — never raw request
    // input. `AssertSqlSafe` is sound under those two checks.
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .fetch_all(tx.as_mut())
        .await?;

    rows.iter()
        .map(|row| {
            projected_instance_from_row(row, object_type_id, title_column.as_deref(), &columns)
        })
        .collect()
}

fn projected_instance_from_row(
    row: &sqlx::postgres::PgRow,
    object_type_id: ObjectTypeId,
    title_column: Option<&str>,
    columns: &[(String, String)],
) -> Result<InstanceState, PgOntologyError> {
    let pk_text: String = row.try_get("__pk")?;
    let pk = Uuid::parse_str(&pk_text)
        .map_err(|_| KernelError::internal("projected primary key was not a UUID"))?;
    let created_at: OffsetDateTime = row.try_get("__created_at")?;

    let mut attributes = serde_json::Map::with_capacity(columns.len());
    let mut title: Option<String> = None;
    for (key, column) in columns {
        let value: Option<String> = row.try_get(format!("attr__{column}").as_str())?;
        if title_column == Some(column.as_str()) {
            title = value.clone();
        }
        attributes.insert(
            key.clone(),
            value.map_or(serde_json::Value::Null, serde_json::Value::String),
        );
    }

    let instance_id = InstanceId::from_uuid(pk);
    Ok(InstanceState {
        instance: InstanceHead {
            id: instance_id,
            object_type_id,
            title: title.unwrap_or(pk_text),
            current_revision_id: None,
            lifecycle_state: InstanceLifecycleState::Active,
        },
        revision: RevisionSummary {
            id: InstanceRevisionId::from_uuid(pk),
            instance_id,
            version: 1,
            attributes: serde_json::Value::Object(attributes),
            valid_from: created_at,
            valid_to: None,
            action_type_id: None,
            actor: None,
            reason: None,
            prev_hash: String::new(),
            row_hash: String::new(),
        },
    })
}

fn require_object(value: &serde_json::Value) -> Result<serde_json::Value, PgOntologyError> {
    if value.is_null() {
        return Ok(serde_json::json!({}));
    }
    if value.is_object() {
        Ok(value.clone())
    } else {
        Err(KernelError::validation("attributes must be a JSON object").into())
    }
}

fn validate_attributes(
    props: &[PropDef],
    attributes: &serde_json::Value,
) -> Result<(), PgOntologyError> {
    let obj = attributes
        .as_object()
        .ok_or_else(|| KernelError::validation("attributes must be a JSON object"))?;
    let known: HashSet<&str> = props.iter().map(|p| p.key.as_str()).collect();
    for key in obj.keys() {
        if !known.contains(key.as_str()) {
            return Err(KernelError::validation(format!(
                "unknown attribute '{key}' is not in the object-type schema"
            ))
            .into());
        }
    }
    for prop in props {
        match obj.get(&prop.key) {
            None | Some(serde_json::Value::Null) => {
                if prop.required {
                    return Err(KernelError::validation(format!(
                        "required attribute '{}' is missing",
                        prop.key
                    ))
                    .into());
                }
            }
            Some(value) => check_field_shape(&prop.field_kind, &prop.key, value)?,
        }
    }
    Ok(())
}

/// Minimal shape check per field kind. An unknown (forward-compat) kind accepts
/// anything so an older binary never rejects a type a newer authoring surface
/// introduced (§3c).
// ponytail: coarse shape only (date/reference/geo checked as strings); tighten
// per-kind formats when a real authoring surface needs stricter validation.
fn check_field_shape(
    kind: &FieldKind,
    key: &str,
    value: &serde_json::Value,
) -> Result<(), PgOntologyError> {
    let ok = match kind {
        FieldKind::Text
        | FieldKind::Date
        | FieldKind::Timestamp
        | FieldKind::GeoPoint
        | FieldKind::Reference
        | FieldKind::Attachment
        | FieldKind::Choice => value.is_string(),
        FieldKind::Integer => value.is_i64() || value.is_u64(),
        FieldKind::Decimal => value.is_number(),
        FieldKind::Boolean => value.is_boolean(),
        FieldKind::MultiChoice => value.is_array(),
        FieldKind::Json | FieldKind::Unknown(_) => true,
    };
    if ok {
        Ok(())
    } else {
        Err(KernelError::validation(format!(
            "attribute '{key}' has the wrong type for field kind '{}'",
            kind.as_tag()
        ))
        .into())
    }
}

// ===========================================================================
// tx write helpers
// ===========================================================================

#[allow(clippy::too_many_arguments)]
async fn insert_instance_head_tx(
    tx: &mut Transaction<'_, Postgres>,
    instance_id: InstanceId,
    org_uuid: Uuid,
    object_type_id: ObjectTypeId,
    title: &str,
    occurred_at: OffsetDateTime,
) -> Result<(), PgOntologyError> {
    sqlx::query(
        r#"
        INSERT INTO ont_instances (
            id, org_id, object_type_id, title, current_revision_id,
            lifecycle_state, created_at, updated_at
        )
        VALUES ($1, $2, $3, $4, NULL, 'draft', $5, $5)
        "#,
    )
    .bind(*instance_id.as_uuid())
    .bind(org_uuid)
    .bind(*object_type_id.as_uuid())
    .bind(title)
    .bind(occurred_at)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_revision_tx(
    tx: &mut Transaction<'_, Postgres>,
    revision_id: InstanceRevisionId,
    org_uuid: Uuid,
    instance_id: InstanceId,
    version: i64,
    attributes: &serde_json::Value,
    valid_from: OffsetDateTime,
    action_type_id: Option<ActionTypeId>,
    actor: UserId,
    reason: Option<&str>,
    prev_hash: &str,
    row_hash: &str,
    occurred_at: OffsetDateTime,
) -> Result<(), PgOntologyError> {
    sqlx::query(
        r#"
        INSERT INTO ont_instance_revisions (
            id, org_id, instance_id, version, attributes, valid_from, valid_to,
            action_type_id, actor, reason, prev_hash, row_hash, created_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8, $9, $10, $11, $12)
        "#,
    )
    .bind(*revision_id.as_uuid())
    .bind(org_uuid)
    .bind(*instance_id.as_uuid())
    .bind(version)
    .bind(attributes)
    .bind(valid_from)
    .bind(action_type_id.map(|a| *a.as_uuid()))
    .bind(*actor.as_uuid())
    .bind(reason)
    .bind(prev_hash)
    .bind(row_hash)
    .bind(occurred_at)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn set_head_pointer_tx(
    tx: &mut Transaction<'_, Postgres>,
    instance_id: InstanceId,
    revision_id: InstanceRevisionId,
    occurred_at: OffsetDateTime,
) -> Result<(), PgOntologyError> {
    sqlx::query("UPDATE ont_instances SET current_revision_id = $2, updated_at = $3 WHERE id = $1")
        .bind(*instance_id.as_uuid())
        .bind(*revision_id.as_uuid())
        .bind(occurred_at)
        .execute(tx.as_mut())
        .await?;
    Ok(())
}

// ===========================================================================
// tx read helpers + row mappers
// ===========================================================================

async fn load_current_state_tx(
    tx: &mut Transaction<'_, Postgres>,
    instance_id: InstanceId,
) -> Result<InstanceState, PgOntologyError> {
    let row = sqlx::query(
        r#"
        SELECT
            i.id AS instance_id, i.object_type_id, i.title,
            i.current_revision_id, i.lifecycle_state,
            r.id AS revision_id, r.version, r.attributes,
            r.valid_from, r.valid_to, r.action_type_id, r.actor,
            r.reason, r.prev_hash, r.row_hash
        FROM ont_instances i
        JOIN ont_instance_revisions r ON r.instance_id = i.id AND r.valid_to IS NULL
        WHERE i.id = $1
        "#,
    )
    .bind(*instance_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("instance was not found"))?;
    instance_state_from_row(&row)
}

async fn load_instance_head_tx(
    tx: &mut Transaction<'_, Postgres>,
    instance_id: InstanceId,
) -> Result<InstanceHead, PgOntologyError> {
    let row = sqlx::query(
        r#"
        SELECT id, object_type_id, title, current_revision_id, lifecycle_state
        FROM ont_instances
        WHERE id = $1
        "#,
    )
    .bind(*instance_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("instance was not found"))?;
    instance_head_from_row(&row)
}

fn instance_head_from_row(row: &sqlx::postgres::PgRow) -> Result<InstanceHead, PgOntologyError> {
    let current: Option<Uuid> = row.try_get("current_revision_id")?;
    Ok(InstanceHead {
        id: InstanceId::from_uuid(row.try_get("id")?),
        object_type_id: ObjectTypeId::from_uuid(row.try_get("object_type_id")?),
        title: row.try_get("title")?,
        current_revision_id: current.map(InstanceRevisionId::from_uuid),
        lifecycle_state: InstanceLifecycleState::from_db_str(row.try_get("lifecycle_state")?)?,
    })
}

/// Maps a joined `(instance, revision)` row where the instance columns are
/// aliased `instance_id`/`revision_id` (the head + effective revision).
fn instance_state_from_row(row: &sqlx::postgres::PgRow) -> Result<InstanceState, PgOntologyError> {
    let current: Option<Uuid> = row.try_get("current_revision_id")?;
    let instance = InstanceHead {
        id: InstanceId::from_uuid(row.try_get("instance_id")?),
        object_type_id: ObjectTypeId::from_uuid(row.try_get("object_type_id")?),
        title: row.try_get("title")?,
        current_revision_id: current.map(InstanceRevisionId::from_uuid),
        lifecycle_state: InstanceLifecycleState::from_db_str(row.try_get("lifecycle_state")?)?,
    };
    let actor: Option<Uuid> = row.try_get("actor")?;
    let action_type_id: Option<Uuid> = row.try_get("action_type_id")?;
    let revision = RevisionSummary {
        id: InstanceRevisionId::from_uuid(row.try_get("revision_id")?),
        instance_id: instance.id,
        version: row.try_get("version")?,
        attributes: row.try_get("attributes")?,
        valid_from: row.try_get("valid_from")?,
        valid_to: row.try_get("valid_to")?,
        action_type_id: action_type_id.map(ActionTypeId::from_uuid),
        actor: actor.map(UserId::from_uuid),
        reason: row.try_get("reason")?,
        prev_hash: row.try_get("prev_hash")?,
        row_hash: row.try_get("row_hash")?,
    };
    Ok(InstanceState { instance, revision })
}

fn revision_from_row(row: &sqlx::postgres::PgRow) -> Result<RevisionSummary, PgOntologyError> {
    let actor: Option<Uuid> = row.try_get("actor")?;
    let action_type_id: Option<Uuid> = row.try_get("action_type_id")?;
    Ok(RevisionSummary {
        id: InstanceRevisionId::from_uuid(row.try_get("id")?),
        instance_id: InstanceId::from_uuid(row.try_get("instance_id")?),
        version: row.try_get("version")?,
        attributes: row.try_get("attributes")?,
        valid_from: row.try_get("valid_from")?,
        valid_to: row.try_get("valid_to")?,
        action_type_id: action_type_id.map(ActionTypeId::from_uuid),
        actor: actor.map(UserId::from_uuid),
        reason: row.try_get("reason")?,
        prev_hash: row.try_get("prev_hash")?,
        row_hash: row.try_get("row_hash")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixity_recomputes_and_detects_tamper() {
        let iid = InstanceId::new();
        let actor = UserId::new();
        let at = OffsetDateTime::UNIX_EPOCH;
        let attrs = serde_json::json!({"priority": "hi", "note": "n"});

        let c1 = canonical_revision(iid, 1, &attrs, at, None, actor, Some("init"));
        let h1 = revision_row_hash(GENESIS_HASH, &c1).unwrap();
        // Deterministic: identical content → identical hash (chain re-verifies).
        assert_eq!(h1, revision_row_hash(GENESIS_HASH, &c1).unwrap());
        assert_eq!(h1.len(), 64);

        // Tamper a fixity-covered field → hash diverges (tamper detected).
        let tampered = serde_json::json!({"priority": "lo", "note": "n"});
        let c1t = canonical_revision(iid, 1, &tampered, at, None, actor, Some("init"));
        assert_ne!(h1, revision_row_hash(GENESIS_HASH, &c1t).unwrap());

        // prev_hash binds the chain: same content, different predecessor → different hash.
        let c2 = canonical_revision(iid, 2, &attrs, at, None, actor, None);
        let h2_chained = revision_row_hash(&h1, &c2).unwrap();
        let h2_genesis = revision_row_hash(GENESIS_HASH, &c2).unwrap();
        assert_ne!(h2_chained, h2_genesis);
    }

    #[test]
    fn verify_chain_accepts_a_valid_chain_and_rejects_a_broken_one() {
        let iid = InstanceId::new();
        let actor = UserId::new();
        let at = OffsetDateTime::UNIX_EPOCH;
        let attrs = serde_json::json!({"k": "v"});

        let c1 = canonical_revision(iid, 1, &attrs, at, None, actor, None);
        let h1 = revision_row_hash(GENESIS_HASH, &c1).unwrap();
        let rev1 = RevisionSummary {
            id: InstanceRevisionId::new(),
            instance_id: iid,
            version: 1,
            attributes: attrs.clone(),
            valid_from: at,
            valid_to: Some(at + time::Duration::hours(1)),
            action_type_id: None,
            actor: Some(actor),
            reason: None,
            prev_hash: GENESIS_HASH.to_owned(),
            row_hash: h1.clone(),
        };
        let attrs2 = serde_json::json!({"k": "v2"});
        let c2 = canonical_revision(
            iid,
            2,
            &attrs2,
            at + time::Duration::hours(1),
            None,
            actor,
            None,
        );
        let h2 = revision_row_hash(&h1, &c2).unwrap();
        let rev2 = RevisionSummary {
            id: InstanceRevisionId::new(),
            instance_id: iid,
            version: 2,
            attributes: attrs2,
            valid_from: at + time::Duration::hours(1),
            valid_to: None,
            action_type_id: None,
            actor: Some(actor),
            reason: None,
            prev_hash: h1,
            row_hash: h2,
        };
        assert!(verify_chain(&[rev1.clone(), rev2.clone()]).is_none());

        // Break rev2's stored hash → verify reports it.
        let mut broken = rev2.clone();
        broken.row_hash = GENESIS_HASH.to_owned();
        assert_eq!(verify_chain(&[rev1, broken.clone()]), Some(broken.id));
    }
}
