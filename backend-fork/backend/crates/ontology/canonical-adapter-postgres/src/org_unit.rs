//! `OrgUnitPort` — the Postgres implementation of `ObjectKey::OrgUnit`.
//!
//! Owned tables, verbatim from the contract: `org_units`,
//! `org_unit_revisions`, `org_unit_source_bindings`. Sites stay operational and
//! are not OrgUnits.
//!
//! # What this port does NOT touch
//!
//! `regions` and `branches` are the branch-scoped AUTHORIZATION spine and have
//! three legitimate writers of their own; the contract names neither, and bead
//! console-1qw.3 was closed decided-no on exactly that question. A branch is a
//! SOURCE that is BOUND to a canonical org unit, so binding one writes a row in
//! `org_unit_source_bindings` — never a write to `branches`. That is the whole
//! of the seam.
//!
//! # Where the receipt is stored
//!
//! In `ont_action_command_receipts` — the generalised store 0177 created and the
//! one `PersonPort` already shares. Its `PRIMARY KEY (org_id, command_id)` is
//! what makes a command id tenant-global across OWNERS, which is the property
//! `CommandId` states in `canonical-domain`. A private per-object store would
//! have given this port its own namespace, and one client idempotency key would
//! then have meant two accepted commands.
//!
//! 0177 carries no `owner` and no `target` column yet — the widening migration
//! is specified in `ReceiptOwner`'s doc, unwritten, and this lane may not write
//! `backend/crates/platform/db/migrations/**` — so the [`DispatchTarget`]
//! travels inside the receipt JSONB as its wire string and read-back parses it
//! with `FromStr`, spelling the thirteen target literals once, in
//! `canonical-domain`. A stored row that names no target is refused, never
//! replayed.
//!
//! # Append-only, enforced by the database
//!
//! `org_unit_revisions` refuses UPDATE and DELETE (0215's
//! `canonical_org_structure_row_immutable` trigger), so a revision is never
//! edited: [`OrgUnitQuery::Revise`] appends `MAX(version) + 1`. `org_units`
//! carries no trigger and needs none — it is an identity anchor with no mutable
//! state, every attribute living in the revision.
//!
//! `org_unit_source_bindings` refuses UPDATE but PERMITS DELETE, and that
//! asymmetry is load-bearing: silently re-pointing a legacy record at a
//! different unit by editing a column is what an audit must not tolerate, so a
//! rebind is an explicit DELETE then INSERT, while DELETE itself stays available
//! for erasure. Its `PRIMARY KEY (org_id, source_kind, source_id)` makes "one
//! legacy record resolves to at most one canonical unit" unrepresentable rather
//! than merely discouraged; the reverse is deliberately not unique, because one
//! unit legitimately absorbs several legacy records.
//!
//! The TRIGGER is the whole of that enforcement, not a privilege:
//! `ops/postgres-reconcile-topology.sh` grants `console_rt` UPDATE and DELETE on
//! every table a migration creates, which 0215's own header states, so the
//! runtime role holds UPDATE on `org_unit_revisions` in the deployed database.
//!
//! # Synchronous port, async driver
//!
//! `CanonicalPort::execute` is synchronous and `sqlx` is async-only, so
//! [`PgOrgUnitPort`] holds a `tokio::runtime::Handle` and blocks on it.
//! `Handle::block_on` panics when called from a runtime worker thread; an async
//! caller must therefore reach `execute` through `spawn_blocking`.
//! ponytail: one runtime handle, no thread pool of its own — revisit only if
//! the trait ever gains an async form.

use console_kernel_core::{KernelError, OrgId, UserId};
use console_ontology_canonical_domain::{
    CanonicalPort, CanonicalPortError, CanonicalQuery, CommandId, CommandReceipt, DispatchTarget,
    ObjectKey, OrgUnit, Preflight, ReceiptOwner,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::collections::HashSet;
use std::str::FromStr;
use time::OffsetDateTime;
use uuid::Uuid;

/// The legacy record an org unit is built from. `source_kind` and `source_id`
/// are TEXT in 0215 and constrained only to be non-empty: the closed set of
/// kinds is not yet enumerable and legacy identifiers are not all UUIDs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SourceBinding {
    pub kind: String,
    pub id: String,
}

/// The typed read this port answers: the write a caller intends, and nothing
/// about how it is performed. Each variant is bound to exactly one of the two
/// dispatch targets the contract assigns to `OrgUnit`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "target")]
pub enum OrgUnitQuery {
    /// `organization.create_org_unit`. Optionally binds the new unit to the
    /// legacy record it was built from, in the same command.
    #[serde(rename = "organization.create_org_unit")]
    Create {
        #[serde(default)]
        source: Option<SourceBinding>,
        attributes: serde_json::Value,
    },
    /// `organization.revise_org_unit`. Appends a revision, and optionally binds
    /// a FURTHER legacy record to the same canonical unit.
    #[serde(rename = "organization.revise_org_unit")]
    Revise {
        org_unit_id: Uuid,
        #[serde(default)]
        source: Option<SourceBinding>,
        attributes: serde_json::Value,
    },
}

impl OrgUnitQuery {
    /// The dispatch target this query is, spelled once in `canonical-domain`.
    #[must_use]
    pub const fn target(&self) -> DispatchTarget {
        match self {
            Self::Create { .. } => DispatchTarget::OrganizationCreateOrgUnit,
            Self::Revise { .. } => DispatchTarget::OrganizationReviseOrgUnit,
        }
    }

    #[must_use]
    pub const fn attributes(&self) -> &serde_json::Value {
        match self {
            Self::Create { attributes, .. } | Self::Revise { attributes, .. } => attributes,
        }
    }

    #[must_use]
    pub const fn source(&self) -> Option<&SourceBinding> {
        match self {
            Self::Create { source, .. } | Self::Revise { source, .. } => source.as_ref(),
        }
    }
}

impl CanonicalQuery for OrgUnitQuery {
    fn dispatch_target(&self) -> DispatchTarget {
        self.target()
    }

    fn subject_id(&self) -> Option<Uuid> {
        match self {
            Self::Create { .. } => None,
            Self::Revise { org_unit_id, .. } => Some(*org_unit_id),
        }
    }
}

/// The typed write this port accepts. `org_id` is the RLS key and `command_id`
/// the tenant-global idempotency key; a repeat replays the stored receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgUnitCommand {
    pub org_id: OrgId,
    pub command_id: CommandId,
    pub actor_id: UserId,
    pub query: OrgUnitQuery,
    pub action_key: String,
    pub object_type_id: Uuid,
}

#[derive(Debug, thiserror::Error)]
pub enum OrgUnitError {
    #[error("preflight blocked the command: {0:?}")]
    Blocked(Vec<String>),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("command {0} was already applied with a different payload")]
    DigestConflict(Uuid),
    #[error("stored receipt for command {0} names no dispatch target: {1}")]
    UnreadableReceipt(Uuid, String),
}

impl CanonicalPortError for OrgUnitError {
    fn into_kernel_error(self) -> KernelError {
        let message = self.to_string();
        match self {
            Self::Blocked(_) => KernelError::validation(message),
            Self::DigestConflict(_) => KernelError::conflict(message),
            Self::Database(_) | Self::UnreadableReceipt(_, _) => KernelError::internal(message),
        }
    }
}

/// Current canonical OrgUnit head. `name` / `parent_id` are parsed from the
/// latest revision's attributes; neither is a column on `org_units`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgUnitHead {
    pub id: Uuid,
    pub name: Option<String>,
    pub parent_id: Option<Uuid>,
    pub version: i64,
}

/// The one permitted holder of production DML against `org_units`,
/// `org_unit_revisions` and `org_unit_source_bindings`.
#[derive(Debug, Clone)]
pub struct PgOrgUnitPort {
    pool: PgPool,
    runtime: tokio::runtime::Handle,
}

impl PgOrgUnitPort {
    #[must_use]
    pub const fn new(pool: PgPool, runtime: tokio::runtime::Handle) -> Self {
        Self { pool, runtime }
    }

    /// Current head of one OrgUnit. A foreign tenant's id is omit-by-RLS
    /// (`None`), never a fabricated row.
    pub fn get(
        &self,
        org_id: OrgId,
        org_unit_id: Uuid,
    ) -> Result<Option<OrgUnitHead>, OrgUnitError> {
        self.runtime
            .block_on(self.read_heads(*org_id.as_uuid(), Some(org_unit_id)))
            .map(|heads| heads.into_iter().next())
    }

    /// Current heads in the armed tenant. Empty when none are visible.
    pub fn list(&self, org_id: OrgId) -> Result<Vec<OrgUnitHead>, OrgUnitError> {
        self.runtime
            .block_on(self.read_heads(*org_id.as_uuid(), None))
    }

    async fn arm_org<'e, E>(&self, executor: E, org: Uuid) -> Result<(), OrgUnitError>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org.to_string())
            .execute(executor)
            .await?;
        Ok(())
    }

    async fn read_heads(
        &self,
        org: Uuid,
        org_unit_id: Option<Uuid>,
    ) -> Result<Vec<OrgUnitHead>, OrgUnitError> {
        let mut tx = self.pool.begin().await?;
        self.arm_org(&mut *tx, org).await?;
        let rows = sqlx::query(
            "SELECT u.id, r.version, r.attributes \
             FROM org_units u \
             JOIN org_unit_revisions r \
               ON r.org_id = u.org_id AND r.org_unit_id = u.id \
             WHERE u.org_id = $1 AND ($2::uuid IS NULL OR u.id = $2) \
               AND r.version = ( \
                 SELECT MAX(version) FROM org_unit_revisions \
                 WHERE org_id = u.org_id AND org_unit_id = u.id \
               ) \
             ORDER BY u.id",
        )
        .bind(org)
        .bind(org_unit_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let attributes: serde_json::Value = row.get("attributes");
                OrgUnitHead {
                    id: row.get("id"),
                    name: attr_string(&attributes, "name"),
                    parent_id: attr_uuid(&attributes, "parent_id"),
                    version: row.get("version"),
                }
            })
            .collect())
    }

    async fn write(&self, command: &OrgUnitCommand) -> Result<CommandReceipt, OrgUnitError> {
        let preflight = <Self as CanonicalPort>::preflight(&command.query);
        if !preflight.is_ok() {
            return Err(OrgUnitError::Blocked(preflight.blockers().to_vec()));
        }

        let digest = payload_digest(command);
        let org = *command.org_id.as_uuid();
        let actor = *command.actor_id.as_uuid();
        let command_uuid = *command.command_id.as_uuid();

        let mut tx = self.pool.begin().await?;
        // Transaction-local, so it is cleared on COMMIT/ROLLBACK and never
        // leaks to the next checkout of a pooled connection. Unset fails
        // closed: RLS shows no rows and accepts no writes.
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org.to_string())
            .execute(&mut *tx)
            .await?;

        if let Some(stored) = sqlx::query(
            "SELECT actor_id, payload_digest, receipt, created_at \
             FROM ont_action_command_receipts WHERE org_id = $1 AND command_id = $2",
        )
        .bind(org)
        .bind(command_uuid)
        .fetch_optional(&mut *tx)
        .await?
        {
            let stored_digest: Vec<u8> = stored.get("payload_digest");
            if stored_digest != digest {
                return Err(OrgUnitError::DigestConflict(command_uuid));
            }
            let result: serde_json::Value = stored.get("receipt");
            let target = stored_target(command_uuid, &result)?;
            let stored_actor: Uuid = stored.get("actor_id");
            let created_at: OffsetDateTime = stored.get("created_at");
            return Ok(receipt(
                command,
                target,
                UserId::from_uuid(stored_actor),
                digest,
                result,
                created_at,
            ));
        }

        let kind_blockers = Self::kind_execute_blockers(&mut tx, org, &command.query).await?;
        if !kind_blockers.is_empty() {
            return Err(OrgUnitError::Blocked(kind_blockers));
        }

        let target = command.query.target();
        let (org_unit_id, version) = match &command.query {
            OrgUnitQuery::Create { .. } => {
                let org_unit_id: Uuid =
                    sqlx::query_scalar("INSERT INTO org_units (org_id) VALUES ($1) RETURNING id")
                        .bind(org)
                        .fetch_one(&mut *tx)
                        .await?;
                (org_unit_id, 1_i64)
            }
            OrgUnitQuery::Revise { org_unit_id, .. } => {
                // ponytail: MAX + 1 under the row's own transaction. A
                // concurrent revise of the same unit loses to
                // UNIQUE (org_id, org_unit_id, version) with 23505 rather than
                // silently overwriting; add SELECT ... FOR UPDATE on `org_units`
                // if that contention is ever measured.
                let next: i64 = sqlx::query_scalar(
                    "SELECT COALESCE(MAX(version), 0) + 1 FROM org_unit_revisions \
                     WHERE org_id = $1 AND org_unit_id = $2",
                )
                .bind(org)
                .bind(org_unit_id)
                .fetch_one(&mut *tx)
                .await?;
                (*org_unit_id, next)
            }
        };

        let result = serde_json::json!({
            "org_unit_id": org_unit_id.to_string(),
            "version": version,
            "target": target.as_str(),
        });

        let created_at: OffsetDateTime = sqlx::query_scalar(
            "INSERT INTO org_unit_revisions \
             (org_id, org_unit_id, version, command_id, actor_id, payload_digest, attributes, receipt) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING created_at",
        )
        .bind(org)
        .bind(org_unit_id)
        .bind(version)
        .bind(command_uuid)
        .bind(actor)
        .bind(digest.as_slice())
        .bind(command.query.attributes())
        .bind(&result)
        .fetch_one(&mut *tx)
        .await?;

        if let Some(source) = command.query.source() {
            sqlx::query(
                "INSERT INTO org_unit_source_bindings \
                 (org_id, source_kind, source_id, org_unit_id, actor_id, payload_digest) \
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(org)
            .bind(&source.kind)
            .bind(&source.id)
            .bind(org_unit_id)
            .bind(actor)
            .bind(digest.as_slice())
            .execute(&mut *tx)
            .await?;
        }

        // The receipt store, and with it the tenant-global command-id
        // namespace this port shares with every other receipt owner.
        // Attribute the receipt to the object whose action it records.
        //
        // DERIVED from the command's own query, which already implements
        // `dispatch_target()` -- the same value the projected-dispatch path uses.
        // NOT from `action_key`: that is "unique only per object type" (a bare
        // "revise"), so it cannot name a target on its own, and the internal
        // reassign path carries "internal.reassign_org_unit", which names none at
        // all. The query knows; the string does not.
        //
        // Without this the row takes the `owner` DEFAULT of 'ontology.action',
        // filing a canonical receipt under the pre-existing instance-action path
        // -- a wrong attribution recorded as fact.
        let receipt_target = command.query.dispatch_target();
        let receipt_owner = ReceiptOwner::Canonical(receipt_target.object());
        sqlx::query(
            "INSERT INTO ont_action_command_receipts \
             (org_id, command_id, actor_id, payload_digest, receipt, action_key, object_type_id, created_at, owner, target) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(org)
        .bind(command_uuid)
        .bind(actor)
        .bind(digest.as_slice())
        .bind(&result)
        .bind(&command.action_key)
        .bind(command.object_type_id)
        .bind(created_at)
        .bind(receipt_owner.as_str())
        .bind(receipt_target.as_str())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(receipt(
            command,
            target,
            command.actor_id,
            digest,
            result,
            created_at,
        ))
    }

    async fn kind_execute_blockers(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        org: Uuid,
        query: &OrgUnitQuery,
    ) -> Result<Vec<String>, OrgUnitError> {
        let attributes = query.attributes();
        let Some(kind) = attr_string(attributes, crate::catalog::ORG_UNIT_KIND) else {
            return Ok(Vec::new());
        };
        if let OrgUnitQuery::Revise { org_unit_id, .. } = query {
            if let Some(stored) = Self::head_attributes(tx, org, *org_unit_id).await?
                && let Some(stored_kind) = attr_string(&stored, crate::catalog::ORG_UNIT_KIND)
                && stored_kind != kind
            {
                return Ok(vec!["kind is immutable".to_owned()]);
            }
            if let Some(parent_id) = attr_uuid(attributes, "parent_id")
                && parent_id == *org_unit_id
            {
                return Ok(vec!["parent_id must not be self".to_owned()]);
            }
        }
        let Some(parent_id) = attr_uuid(attributes, "parent_id") else {
            return Ok(Vec::new());
        };
        let Some(parent_attrs) = Self::head_attributes(tx, org, parent_id).await? else {
            return Ok(vec![
                "parent_id must refer to an OrgUnit in this organization".to_owned(),
            ]);
        };
        let parent_kind = attr_string(&parent_attrs, crate::catalog::ORG_UNIT_KIND);
        let kind_blockers = match kind.as_str() {
            "department" if parent_kind.as_deref() != Some("site") => {
                vec!["department parent must be a site".to_owned()]
            }
            "team"
                if parent_kind.as_deref() != Some("department")
                    && parent_kind.as_deref() != Some("team") =>
            {
                vec!["team parent must be a department or team".to_owned()]
            }
            _ => Vec::new(),
        };
        if !kind_blockers.is_empty() {
            return Ok(kind_blockers);
        }
        // Self-parent is already refused above. Walk the proposed parent's
        // ancestor chain so team→team (and any longer) cycles fail closed
        // with no mutation, the same as length-1.
        if let OrgUnitQuery::Revise { org_unit_id, .. } = query {
            let mut cursor = Some(parent_id);
            let mut seen = HashSet::new();
            while let Some(id) = cursor {
                if id == *org_unit_id {
                    return Ok(vec!["parent_id must not form a cycle".to_owned()]);
                }
                if !seen.insert(id) {
                    return Ok(vec!["parent_id must not form a cycle".to_owned()]);
                }
                cursor = match Self::head_attributes(tx, org, id).await? {
                    Some(attrs) => attr_uuid(&attrs, "parent_id"),
                    None => None,
                };
            }
        }
        Ok(Vec::new())
    }

    async fn head_attributes(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        org: Uuid,
        org_unit_id: Uuid,
    ) -> Result<Option<serde_json::Value>, OrgUnitError> {
        Ok(sqlx::query_scalar(
            "SELECT r.attributes FROM org_unit_revisions r \
             WHERE r.org_id = $1 AND r.org_unit_id = $2 \
               AND r.version = ( \
                 SELECT MAX(version) FROM org_unit_revisions \
                 WHERE org_id = $1 AND org_unit_id = $2 \
               )",
        )
        .bind(org)
        .bind(org_unit_id)
        .fetch_optional(&mut **tx)
        .await?)
    }
}

impl CanonicalPort for PgOrgUnitPort {
    type Object = OrgUnit;
    type Query = OrgUnitQuery;
    type Command = OrgUnitCommand;
    type Error = OrgUnitError;

    /// PURE: no `&self`, no IO, no persistence. A blocked preflight has written
    /// nothing, so it can never spend an approval.
    fn preflight(query: &Self::Query) -> Preflight {
        let attributes = query.attributes();
        let mut blockers =
            crate::catalog::require_text_property(attributes, crate::catalog::ORG_UNIT_NAME);
        blockers.extend(kind_preflight(attributes));
        if let OrgUnitQuery::Revise { org_unit_id, .. } = query
            && org_unit_id.is_nil()
        {
            blockers.push("org_unit_id must not be nil".to_owned());
        }
        // 0215's `CHECK (source_kind <> '')` and `CHECK (source_id <> '')`,
        // restated purely so a caller learns both at once instead of one round
        // trip at a time. The CHECK remains the enforcement.
        if let Some(source) = query.source() {
            if source.kind.is_empty() {
                blockers.push("source_kind must not be empty".to_owned());
            }
            if source.id.is_empty() {
                blockers.push("source_id must not be empty".to_owned());
            }
        }
        if blockers.is_empty() {
            Preflight::ok()
        } else {
            Preflight::blocked(blockers)
        }
    }

    fn command(
        org_id: OrgId,
        command_id: CommandId,
        actor_id: UserId,
        query: Self::Query,
        action_key: &str,
        object_type_id: Uuid,
    ) -> Self::Command {
        OrgUnitCommand {
            org_id,
            command_id,
            actor_id,
            query,
            action_key: action_key.to_owned(),
            object_type_id,
        }
    }

    fn execute(&self, command: &Self::Command) -> Result<CommandReceipt, Self::Error> {
        self.runtime.block_on(self.write(command))
    }
}

fn receipt(
    command: &OrgUnitCommand,
    target: DispatchTarget,
    actor_id: UserId,
    digest: [u8; 32],
    result: serde_json::Value,
    created_at: OffsetDateTime,
) -> CommandReceipt {
    CommandReceipt::new(
        command.org_id,
        command.command_id,
        ReceiptOwner::Canonical(ObjectKey::OrgUnit),
        target,
        actor_id,
        digest,
        result,
        created_at,
    )
}

/// The target a stored receipt names, read back through the roster's `FromStr`
/// rather than re-spelled here.
fn stored_target(
    command_id: Uuid,
    result: &serde_json::Value,
) -> Result<DispatchTarget, OrgUnitError> {
    let stored = result["target"]
        .as_str()
        .ok_or_else(|| OrgUnitError::UnreadableReceipt(command_id, result.to_string()))?;
    DispatchTarget::from_str(stored)
        .map_err(|error| OrgUnitError::UnreadableReceipt(command_id, error.to_string()))
}

/// The 32 bytes the `payload_digest` CHECK is sized for.
///
/// The attributes go in through [`canonical_json`], never through
/// `Value::to_string()` directly: `serde_json` resolves with `preserve_order`
/// in this workspace, so a `Value` serialises its object keys in INSERTION
/// order and two payloads that compare EQUAL serialise to different bytes. The
/// retry a client performs after a timeout — or after a round-trip through the
/// `attributes` JSONB column, which PostgreSQL stores in its own key order —
/// must digest to the same 32 bytes, or it comes back as an
/// [`OrgUnitError::DigestConflict`] instead of the documented replay.
///
/// ponytail: a byte-for-byte twin of `person::payload_digest`. The two cannot
/// share one helper today — `src/lib.rs` is out of this lane's owned root, so no
/// `mod digest` can be declared, and the sibling's copy is private. Hoisting
/// both into one module is a follow-up for after the three port lanes land.
fn payload_digest(command: &OrgUnitCommand) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(command.org_id.as_uuid().as_bytes());
    hasher.update(command.command_id.as_uuid().as_bytes());
    hasher.update(command.actor_id.as_uuid().as_bytes());
    hasher.update(command.query.target().as_str().as_bytes());
    if let OrgUnitQuery::Revise { org_unit_id, .. } = &command.query {
        hasher.update(org_unit_id.as_bytes());
    }
    if let Some(source) = command.query.source() {
        // LENGTH-PREFIXED, because plain concatenation is ambiguous: ("hris", "emp-1") and
        // ("hrise", "mp-1") produce identical bytes and therefore an identical digest. The port
        // would then replay the FIRST command's receipt and report success for a source binding it
        // never wrote, and a reconciler reading that receipt records the mapping as synced.
        // `employment.rs` already hashes its variable-length fields this way; this was the one
        // canonical port that did not.
        for value in [source.kind.as_str(), source.id.as_str()] {
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value.as_bytes());
        }
    }
    hasher.update(
        canonical_json(command.query.attributes())
            .to_string()
            .as_bytes(),
    );
    hasher.finalize().into()
}

/// The same value with every object's keys SORTED, at every depth — the form
/// `Value::to_string()` would already emit if `serde_json` were not built with
/// `preserve_order`. The sort is explicit because collecting back into a `Map`
/// preserves the source's iteration order under that feature, so a rebuild that
/// does not sort is a no-op.
fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(values) => {
            let mut entries: Vec<(String, serde_json::Value)> = values
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            serde_json::Value::Object(entries.into_iter().collect())
        }
        primitive => primitive.clone(),
    }
}

fn kind_preflight(attributes: &serde_json::Value) -> Vec<String> {
    let Some(object) = attributes.as_object() else {
        return Vec::new();
    };
    let mut blockers =
        crate::catalog::require_text_property(attributes, crate::catalog::ORG_UNIT_KIND);
    if !blockers.is_empty() {
        return blockers;
    }
    let Some(kind) = object
        .get(crate::catalog::ORG_UNIT_KIND)
        .and_then(serde_json::Value::as_str)
    else {
        return blockers;
    };
    if !matches!(kind, "site" | "department" | "team") {
        return vec!["kind must be site, department, or team".to_owned()];
    }
    let parent = object.get("parent_id");
    match kind {
        "site" => {
            if parent.is_some_and(|value| !value.is_null()) {
                blockers.push("site must not have parent_id".to_owned());
            }
        }
        "department" | "team" => match parent {
            None | Some(serde_json::Value::Null) => blockers.push(if kind == "department" {
                "department parent_id is required".to_owned()
            } else {
                "team parent_id is required".to_owned()
            }),
            Some(serde_json::Value::String(raw)) if Uuid::parse_str(raw).is_ok() => {}
            Some(_) => blockers.push("parent_id must be a uuid".to_owned()),
        },
        _ => {}
    }
    blockers
}

fn attr_string(attributes: &serde_json::Value, key: &str) -> Option<String> {
    attributes
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn attr_uuid(attributes: &serde_json::Value, key: &str) -> Option<Uuid> {
    attributes
        .get(key)
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

// ---------------------------------------------------------------------------
// L5-ORG binding seam — re-export owner-crate module (also #[path]'d by org-change)
// ---------------------------------------------------------------------------

pub use crate::org_unit_binding::{
    AmbiguousTextAuthority, SOURCE_KIND_BRANCH, SOURCE_KIND_REGION, SourceBindingResolution,
    count_org_units_named, resolve_source_binding, unambiguous_legacy_source_id,
};

/// Bind a legacy region or branch UUID to a canonical OrgUnit inside an open
/// transaction. Replays an existing binding; never accepts free-text source ids.
///
/// Thin wrapper preserving the historical `OrgUnitError` return type. SQL lives
/// in [`crate::org_unit_binding`] so writer-ownership stays on this owner crate
/// even when org-change compiles that file via `#[path]`.
pub async fn ensure_unambiguous_legacy_binding_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org_id: OrgId,
    actor_id: UserId,
    source_kind: &str,
    legacy_id: Uuid,
    attributes: serde_json::Value,
    command_id: Uuid,
) -> Result<Uuid, OrgUnitError> {
    crate::org_unit_binding::ensure_unambiguous_legacy_binding_in_tx(
        tx,
        org_id,
        actor_id,
        source_kind,
        legacy_id,
        attributes,
        command_id,
    )
    .await
    .map_err(|err| match err {
        crate::org_unit_binding::OrgUnitBindingError::Blocked(blockers) => {
            OrgUnitError::Blocked(blockers)
        }
        crate::org_unit_binding::OrgUnitBindingError::Database(error) => {
            OrgUnitError::Database(error)
        }
    })
}
