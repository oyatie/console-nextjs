//! `JobPositionPort` — the Postgres implementation of `ObjectKey::JobPosition`.
//!
//! Owned tables, verbatim from the contract: `job_positions`,
//! `job_position_revisions`. Recruiting postings and employee position strings
//! are not canonical positions — this port never reads `recruit_postings` or
//! `employees.position`, never invents a row from free text, and carries no
//! legacy `SourceBinding` (unlike `OrgUnitPort`). Authority for position
//! identity is the UUID returned on create/revise and readable via
//! [`PgJobPositionPort::get`] / [`PgJobPositionPort::list`] /
//! [`PgJobPositionPort::list_for_org_unit`].
//!
//! # The one asymmetry that is not copied from `PersonPort`
//!
//! `persons` is a bare identity anchor, so `PgPersonPort` never updates it.
//! `job_positions` is NOT: migration 0215 gives it `org_unit_id`, because the
//! contract says "Heads/revisions referencing OrgUnit" and a reference is a
//! FOREIGN KEY or it is not enforced. A reorganisation MOVES a position between
//! units, so that column is mutable, `job_positions` carries no append-only
//! trigger, and `console_rt` holds UPDATE on it by GRANT rather than by the
//! default-privilege accident. [`JobPositionQuery::Revise`] therefore takes an
//! OPTIONAL `org_unit_id`: `None` revises attributes only, `Some` also performs
//! the move. `job_position_revisions` stays append-only either way — the head
//! moves, the history does not.
//!
//! The unit itself is NOT written here. `org_units` belongs to `OrgUnitPort`
//! (`src/org_unit.rs`), the same crate and a different module; a caller supplies
//! an org unit that already exists and the `(org_id, org_unit_id)` foreign key
//! on `job_positions` refuses one that does not.
//!
//! # Where the receipt is stored
//!
//! In `ont_action_command_receipts` — the shared store 0177 created, the one
//! `console-ontology-rest` and `PgPersonPort` already write. Its
//! `PRIMARY KEY (org_id, command_id)` is what makes a command id tenant-global
//! across OWNERS, which is the property `CommandId` and `ReceiptOwner` state in
//! `canonical-domain`. A private per-object store would have given this port its
//! own namespace, and one client idempotency key would then have meant two
//! accepted commands.
//!
//! 0177 carries no `owner` and no `target` column yet — the widening migration
//! is specified in `ReceiptOwner`'s doc and unwritten — so the `DispatchTarget`
//! travels inside the receipt JSONB as its wire string and read-back parses it
//! with `FromStr`, spelling the thirteen target literals once, in
//! `canonical-domain`. A stored row that names no target is refused, never
//! replayed.
//!
//! # Append-only, enforced by the database
//!
//! `job_position_revisions` refuses UPDATE and DELETE (0215's
//! `canonical_org_structure_row_immutable` trigger), so a revision is never
//! edited: `Revise` appends `MAX(version) + 1`. The TRIGGER is the whole of that
//! enforcement, not a privilege — 0215's own header records that
//! `ops/postgres-reconcile-topology.sh` grants `console_rt` UPDATE on every
//! table a migration creates before those `GRANT` lines run.
//!
//! # Synchronous port, async driver
//!
//! `CanonicalPort::execute` is synchronous and `sqlx` is async-only, so
//! [`PgJobPositionPort`] holds a `tokio::runtime::Handle` and blocks on it.
//! `Handle::block_on` panics when called from a runtime worker thread; an async
//! caller must therefore reach `execute` through `spawn_blocking`.
//! ponytail: one runtime handle, no thread pool of its own — revisit only if
//! the trait ever gains an async form.

use console_kernel_core::{KernelError, OrgId, UserId};
use console_ontology_canonical_domain::{
    CanonicalPort, CanonicalPortError, CanonicalQuery, CommandId, CommandReceipt, DispatchTarget,
    JobPosition, ObjectKey, Preflight, ReceiptOwner,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::str::FromStr;
use time::OffsetDateTime;
use uuid::Uuid;

/// The current head of one canonical JobPosition — the authority readback
/// surface for position identity. Distinct from `employees.position` TEXT and
/// from recruiting postings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobPositionView {
    pub job_position_id: Uuid,
    pub org_unit_id: Uuid,
    pub version: i64,
    pub attributes: serde_json::Value,
}

/// The typed read this port answers: the write a caller intends, and nothing
/// about how it is performed. Each variant is bound to exactly one of the two
/// dispatch targets the contract assigns to `JobPosition`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "target")]
pub enum JobPositionQuery {
    /// `organization.create_job_position`. The position is created under an org
    /// unit that must already exist.
    #[serde(rename = "organization.create_job_position")]
    Create {
        org_unit_id: Uuid,
        attributes: serde_json::Value,
    },
    /// `organization.revise_job_position`. Appends a revision, and optionally
    /// MOVES the position to another org unit — the reorganisation case the
    /// mutable head column exists for.
    #[serde(rename = "organization.revise_job_position")]
    Revise {
        job_position_id: Uuid,
        #[serde(default)]
        org_unit_id: Option<Uuid>,
        attributes: serde_json::Value,
    },
}

impl JobPositionQuery {
    /// The dispatch target this query is, spelled once in `canonical-domain`.
    #[must_use]
    pub const fn target(&self) -> DispatchTarget {
        match self {
            Self::Create { .. } => DispatchTarget::OrganizationCreateJobPosition,
            Self::Revise { .. } => DispatchTarget::OrganizationReviseJobPosition,
        }
    }

    #[must_use]
    pub const fn attributes(&self) -> &serde_json::Value {
        match self {
            Self::Create { attributes, .. } | Self::Revise { attributes, .. } => attributes,
        }
    }

    /// The unit this command names, if it names one. `None` is a revise that
    /// leaves the position where it is.
    #[must_use]
    pub const fn org_unit_id(&self) -> Option<Uuid> {
        match self {
            Self::Create { org_unit_id, .. } => Some(*org_unit_id),
            Self::Revise { org_unit_id, .. } => *org_unit_id,
        }
    }
}

impl CanonicalQuery for JobPositionQuery {
    fn dispatch_target(&self) -> DispatchTarget {
        self.target()
    }

    fn subject_id(&self) -> Option<Uuid> {
        match self {
            Self::Create { .. } => None,
            Self::Revise {
                job_position_id, ..
            } => Some(*job_position_id),
        }
    }
}

/// The typed write this port accepts. `org_id` is the RLS key and `command_id`
/// the tenant-global idempotency key; a repeat replays the stored receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobPositionCommand {
    pub org_id: OrgId,
    pub command_id: CommandId,
    pub actor_id: UserId,
    pub query: JobPositionQuery,
    pub action_key: String,
    pub object_type_id: Uuid,
}

#[derive(Debug, thiserror::Error)]
pub enum JobPositionError {
    #[error("preflight blocked the command: {0:?}")]
    Blocked(Vec<String>),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("command {0} was already applied with a different payload")]
    DigestConflict(Uuid),
    #[error("stored receipt for command {0} names no dispatch target: {1}")]
    UnreadableReceipt(Uuid, String),
}

impl CanonicalPortError for JobPositionError {
    fn into_kernel_error(self) -> KernelError {
        let message = self.to_string();
        match self {
            Self::Blocked(_) => KernelError::validation(message),
            Self::DigestConflict(_) => KernelError::conflict(message),
            Self::Database(_) | Self::UnreadableReceipt(_, _) => KernelError::internal(message),
        }
    }
}

/// The one permitted holder of production DML against `job_positions` and
/// `job_position_revisions`.
#[derive(Debug, Clone)]
pub struct PgJobPositionPort {
    pool: PgPool,
    runtime: tokio::runtime::Handle,
}

impl PgJobPositionPort {
    #[must_use]
    pub const fn new(pool: PgPool, runtime: tokio::runtime::Handle) -> Self {
        Self { pool, runtime }
    }

    /// Read the current head of a JobPosition. Tenant-armed via `app.current_org`;
    /// a foreign tenant's id is omit-by-RLS (`None`), never disclosed.
    pub fn get(
        &self,
        org_id: OrgId,
        job_position_id: Uuid,
    ) -> Result<Option<JobPositionView>, JobPositionError> {
        self.runtime
            .block_on(self.read_one(*org_id.as_uuid(), job_position_id))
    }

    /// List current heads under one OrgUnit. Empty when the unit has no
    /// positions or is invisible to the armed tenant — never invents rows from
    /// free-text employee/recruiting data.
    pub fn list_for_org_unit(
        &self,
        org_id: OrgId,
        org_unit_id: Uuid,
    ) -> Result<Vec<JobPositionView>, JobPositionError> {
        self.runtime
            .block_on(self.read_for_unit(*org_id.as_uuid(), org_unit_id))
    }

    /// Current heads in the armed tenant. Empty when none are visible — never
    /// invents rows from free-text employee/recruiting data.
    pub fn list(&self, org_id: OrgId) -> Result<Vec<JobPositionView>, JobPositionError> {
        self.runtime.block_on(self.read_all(*org_id.as_uuid()))
    }

    async fn arm_org<'e, E>(&self, executor: E, org: Uuid) -> Result<(), JobPositionError>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org.to_string())
            .execute(executor)
            .await?;
        Ok(())
    }

    async fn read_one(
        &self,
        org: Uuid,
        job_position_id: Uuid,
    ) -> Result<Option<JobPositionView>, JobPositionError> {
        let mut tx = self.pool.begin().await?;
        self.arm_org(&mut *tx, org).await?;
        let row = sqlx::query(
            "SELECT p.id AS job_position_id, p.org_unit_id, r.version, r.attributes \
             FROM job_positions p \
             JOIN job_position_revisions r \
               ON r.org_id = p.org_id AND r.job_position_id = p.id \
             WHERE p.org_id = $1 AND p.id = $2 \
               AND r.version = ( \
                 SELECT MAX(version) FROM job_position_revisions \
                 WHERE org_id = p.org_id AND job_position_id = p.id \
               )",
        )
        .bind(org)
        .bind(job_position_id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row.map(|row| JobPositionView {
            job_position_id: row.get("job_position_id"),
            org_unit_id: row.get("org_unit_id"),
            version: row.get("version"),
            attributes: row.get("attributes"),
        }))
    }

    async fn read_for_unit(
        &self,
        org: Uuid,
        org_unit_id: Uuid,
    ) -> Result<Vec<JobPositionView>, JobPositionError> {
        let mut tx = self.pool.begin().await?;
        self.arm_org(&mut *tx, org).await?;
        let rows = sqlx::query(
            "SELECT p.id AS job_position_id, p.org_unit_id, r.version, r.attributes \
             FROM job_positions p \
             JOIN job_position_revisions r \
               ON r.org_id = p.org_id AND r.job_position_id = p.id \
             WHERE p.org_id = $1 AND p.org_unit_id = $2 \
               AND r.version = ( \
                 SELECT MAX(version) FROM job_position_revisions \
                 WHERE org_id = p.org_id AND job_position_id = p.id \
               ) \
             ORDER BY p.id",
        )
        .bind(org)
        .bind(org_unit_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(|row| JobPositionView {
                job_position_id: row.get("job_position_id"),
                org_unit_id: row.get("org_unit_id"),
                version: row.get("version"),
                attributes: row.get("attributes"),
            })
            .collect())
    }

    async fn read_all(&self, org: Uuid) -> Result<Vec<JobPositionView>, JobPositionError> {
        let mut tx = self.pool.begin().await?;
        self.arm_org(&mut *tx, org).await?;
        let rows = sqlx::query(
            "SELECT p.id AS job_position_id, p.org_unit_id, r.version, r.attributes \
             FROM job_positions p \
             JOIN job_position_revisions r \
               ON r.org_id = p.org_id AND r.job_position_id = p.id \
             WHERE p.org_id = $1 \
               AND r.version = ( \
                 SELECT MAX(version) FROM job_position_revisions \
                 WHERE org_id = p.org_id AND job_position_id = p.id \
               ) \
             ORDER BY p.id",
        )
        .bind(org)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(|row| JobPositionView {
                job_position_id: row.get("job_position_id"),
                org_unit_id: row.get("org_unit_id"),
                version: row.get("version"),
                attributes: row.get("attributes"),
            })
            .collect())
    }

    async fn write(
        &self,
        command: &JobPositionCommand,
    ) -> Result<CommandReceipt, JobPositionError> {
        let preflight = <Self as CanonicalPort>::preflight(&command.query);
        if !preflight.is_ok() {
            return Err(JobPositionError::Blocked(preflight.blockers().to_vec()));
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
                return Err(JobPositionError::DigestConflict(command_uuid));
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

        let target = command.query.target();
        let (job_position_id, head_org_unit_id, version) = match &command.query {
            JobPositionQuery::Create { org_unit_id, .. } => {
                let job_position_id: Uuid = sqlx::query_scalar(
                    "INSERT INTO job_positions (org_id, org_unit_id) VALUES ($1, $2) RETURNING id",
                )
                .bind(org)
                .bind(org_unit_id)
                .fetch_one(&mut *tx)
                .await?;
                (job_position_id, *org_unit_id, 1_i64)
            }
            JobPositionQuery::Revise {
                job_position_id,
                org_unit_id,
                ..
            } => {
                // ponytail: MAX + 1 under the row's own transaction. A
                // concurrent revise of the same position loses to
                // UNIQUE (org_id, job_position_id, version) with 23505 rather
                // than silently overwriting; add SELECT ... FOR UPDATE on
                // `job_positions` if that contention is ever measured.
                let next: i64 = sqlx::query_scalar(
                    "SELECT COALESCE(MAX(version), 0) + 1 FROM job_position_revisions \
                     WHERE org_id = $1 AND job_position_id = $2",
                )
                .bind(org)
                .bind(job_position_id)
                .fetch_one(&mut *tx)
                .await?;
                let head_unit = match org_unit_id {
                    Some(unit) => *unit,
                    None => {
                        sqlx::query_scalar(
                            "SELECT org_unit_id FROM job_positions \
                             WHERE org_id = $1 AND id = $2",
                        )
                        .bind(org)
                        .bind(job_position_id)
                        .fetch_one(&mut *tx)
                        .await?
                    }
                };
                (*job_position_id, head_unit, next)
            }
        };

        // Receipt carries the canonical IDs clients round-trip on the preserved
        // ontology action namespace. `org_unit_id` is the head after this write.
        let result = serde_json::json!({
            "job_position_id": job_position_id.to_string(),
            "org_unit_id": head_org_unit_id.to_string(),
            "version": version,
            "target": target.as_str(),
        });

        let created_at: OffsetDateTime = sqlx::query_scalar(
            "INSERT INTO job_position_revisions \
             (org_id, job_position_id, version, command_id, actor_id, payload_digest, attributes, \
              receipt) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING created_at",
        )
        .bind(org)
        .bind(job_position_id)
        .bind(version)
        .bind(command_uuid)
        .bind(actor)
        .bind(digest.as_slice())
        .bind(command.query.attributes())
        .bind(&result)
        .fetch_one(&mut *tx)
        .await?;

        // The reorganisation move. Only a `Revise` reaches this — a `Create`
        // already set the column — and only one that names a unit. The head is
        // mutable BY DESIGN (0215 gives it no immutability trigger); the
        // revision row inserted above is not, so the history of the move
        // survives it.
        if let JobPositionQuery::Revise {
            org_unit_id: Some(org_unit_id),
            ..
        } = &command.query
        {
            sqlx::query("UPDATE job_positions SET org_unit_id = $1 WHERE org_id = $2 AND id = $3")
                .bind(org_unit_id)
                .bind(org)
                .bind(job_position_id)
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
}

impl CanonicalPort for PgJobPositionPort {
    type Object = JobPosition;
    type Query = JobPositionQuery;
    type Command = JobPositionCommand;
    type Error = JobPositionError;

    /// PURE: no `&self`, no IO, no persistence. A blocked preflight has written
    /// nothing, so it can never spend an approval.
    fn preflight(query: &Self::Query) -> Preflight {
        let mut blockers = crate::catalog::require_text_property(
            query.attributes(),
            crate::catalog::JOB_POSITION_TITLE,
        );
        if let JobPositionQuery::Revise {
            job_position_id, ..
        } = query
            && job_position_id.is_nil()
        {
            blockers.push("job_position_id must not be nil".to_owned());
        }
        // Covers both variants: `Create` always names a unit, `Revise` names one
        // only when it is a move. A nil UUID would reach the foreign key as a
        // real lookup, so it is refused here instead.
        if query.org_unit_id().is_some_and(|unit| unit.is_nil()) {
            blockers.push("org_unit_id must not be nil".to_owned());
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
        JobPositionCommand {
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
    command: &JobPositionCommand,
    target: DispatchTarget,
    actor_id: UserId,
    digest: [u8; 32],
    result: serde_json::Value,
    created_at: OffsetDateTime,
) -> CommandReceipt {
    CommandReceipt::new(
        command.org_id,
        command.command_id,
        ReceiptOwner::Canonical(ObjectKey::JobPosition),
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
) -> Result<DispatchTarget, JobPositionError> {
    let stored = result["target"]
        .as_str()
        .ok_or_else(|| JobPositionError::UnreadableReceipt(command_id, result.to_string()))?;
    DispatchTarget::from_str(stored)
        .map_err(|error| JobPositionError::UnreadableReceipt(command_id, error.to_string()))
}

/// The 32 bytes the `payload_digest` CHECK is sized for.
///
/// The attributes go in through [`canonical_json`], never through
/// `Value::to_string()` directly: `serde_json` resolves with `preserve_order`
/// in this workspace, so a `Value` serialises its object keys in INSERTION
/// order and two payloads that compare EQUAL serialise to different bytes. The
/// retry a client performs after a timeout — or after a round-trip through the
/// `attributes` JSONB column, which PostgreSQL stores in its own key order —
/// must digest to the same 32 bytes, or it comes back as a
/// [`JobPositionError::DigestConflict`] instead of the documented replay.
fn payload_digest(command: &JobPositionCommand) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(command.org_id.as_uuid().as_bytes());
    hasher.update(command.command_id.as_uuid().as_bytes());
    hasher.update(command.actor_id.as_uuid().as_bytes());
    hasher.update(command.query.target().as_str().as_bytes());
    if let JobPositionQuery::Revise {
        job_position_id, ..
    } = &command.query
    {
        hasher.update(job_position_id.as_bytes());
    }
    if let Some(org_unit_id) = command.query.org_unit_id() {
        hasher.update(org_unit_id.as_bytes());
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
