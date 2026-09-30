//! `CompanyPort` — the Postgres implementation of `ObjectKey::Company`.
//!
//! Owned tables, verbatim from the contract: `organizations`,
//! `company_revisions`. `organizations` stays the tenant/current head and
//! `company_revisions` is the append-only history; no `companies` table is
//! created.
//!
//! # Why this port writes ONE of its two owned tables
//!
//! `organizations` is owned by Company and is DELIBERATELY unwritable by the
//! runtime role. `0031_runtime_role_and_immutable_org.sql` grants `console_rt`
//! `SELECT` on it and then `REVOKE INSERT, UPDATE, DELETE ON organizations FROM
//! console_rt`, in its own words because "provisioning a tenant is an owner
//! operation, so the runtime role must never INSERT/UPDATE/DELETE org rows" —
//! the one deliberate exception to the `ALTER DEFAULT PRIVILEGES` blanket grant
//! two lines below it. So an `UPDATE organizations` here would not be a stronger
//! port; it would be a statement that raises `42501 permission denied for table
//! organizations` in every deployed database.
//!
//! Ownership in the contract is the right to write, not an obligation to. The
//! canonical company state therefore lives entirely in
//! `company_revisions.attributes`, and `organizations` keeps exactly the role
//! 0026 and 0031 gave it: the provisioning-owned identity of the tenant, and the
//! `REFERENCES organizations(id)` this table's `org_id` hangs off.
//! `tests/company_port_as_runtime_role.rs::the_runtime_role_may_not_write_the_organizations_head`
//! measures both halves of that — the three revoked privileges and the real
//! refusal — so the claim is executed rather than argued.
//!
//! One consequence to state plainly: `console-gate-writer-ownership` can never
//! be made non-vacuous for `organizations` by this port, because there is no
//! legitimate writer to be a second one of. Its coverage of that table is proven
//! by injection instead (a second writer added to a non-owner crate is charged),
//! not by a green run.
//!
//! # Where the receipt is stored
//!
//! In `ont_action_command_receipts` — the generalised store 0177 created, shared
//! with `console-ontology-rest` and with `PersonPort`. Its `PRIMARY KEY (org_id,
//! command_id)` is what makes a command id tenant-global across OWNERS, which is
//! the property `CommandId` and `ReceiptOwner` state in `canonical-domain`. A
//! private per-object store would have given this port its own namespace, and
//! one client idempotency key would then have meant two accepted commands.
//!
//! 0177 carries no `owner` and no `target` column yet, so the `DispatchTarget`
//! travels inside the receipt JSONB as its wire string and read-back parses it
//! with `FromStr`, spelling the target literals once, in `canonical-domain`. A
//! stored row that names no target is refused, never replayed.
//!
//! # Append-only, enforced by the database
//!
//! `company_revisions` refuses UPDATE and DELETE (0215's
//! `canonical_org_structure_row_immutable` trigger), so a revision is never
//! edited: a revise appends `MAX(version) + 1`. `UNIQUE (org_id, version)`
//! carries no object id because there is exactly one company per tenant — the
//! tenant IS the company.
//!
//! The TRIGGER is the whole of that enforcement, not a privilege:
//! `ops/postgres-reconcile-topology.sh` grants `console_rt` UPDATE and DELETE on
//! every table a migration creates, which 0215's own header states, so the
//! runtime role holds both on `company_revisions` in the deployed database.
//!
//! # Synchronous port, async driver
//!
//! `CanonicalPort::execute` is synchronous and `sqlx` is async-only, so
//! [`PgCompanyPort`] holds a `tokio::runtime::Handle` and blocks on it.
//! `Handle::block_on` panics when called from a runtime worker thread; an async
//! caller must therefore reach `execute` through `spawn_blocking`.
//! ponytail: one runtime handle, no thread pool of its own — revisit only if the
//! trait ever gains an async form.

use console_kernel_core::{KernelError, OrgId, UserId};
use console_ontology_canonical_domain::{
    CanonicalPort, CanonicalPortError, CanonicalQuery, CommandId, CommandReceipt, Company,
    DispatchTarget, ObjectKey, Preflight, ReceiptOwner,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::str::FromStr;
use time::OffsetDateTime;
use uuid::Uuid;

/// The typed read this port answers: the write a caller intends, and nothing
/// about how it is performed.
///
/// A STRUCT, not an enum. `Company` has exactly one dispatch target in the
/// contract — `company.revise` — so a one-variant enum would be a `match` with
/// a single arm in every method, and `the_contract_identity_is_copied_verbatim…`
/// asserts that one-ness against `DispatchTarget::ALL` so a second target makes
/// the test fail rather than this shape silently go wrong.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CompanyQuery {
    /// The company's canonical state at the new version. The attribute schema
    /// belongs to this port: [`crate::catalog::COMPANY_LEGAL_NAME`] is required
    /// (PURE preflight). 0215 constrains the column only to be a JSON object.
    pub attributes: serde_json::Value,
}

impl CompanyQuery {
    /// The dispatch target this query is, spelled once in `canonical-domain`.
    #[must_use]
    pub const fn target(&self) -> DispatchTarget {
        DispatchTarget::CompanyRevise
    }
}

impl CanonicalQuery for CompanyQuery {
    fn dispatch_target(&self) -> DispatchTarget {
        self.target()
    }

    fn subject_id(&self) -> Option<Uuid> {
        // Company revise is the tenant itself (`org_id`), not a payload row id.
        None
    }
}

/// The typed write this port accepts. `org_id` is the RLS key, the tenant, AND
/// the company; `command_id` is the tenant-global idempotency key, and a repeat
/// replays the stored receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanyCommand {
    pub org_id: OrgId,
    pub command_id: CommandId,
    pub actor_id: UserId,
    pub query: CompanyQuery,
    pub action_key: String,
    pub object_type_id: Uuid,
}

#[derive(Debug, thiserror::Error)]
pub enum CompanyError {
    #[error("preflight blocked the command: {0:?}")]
    Blocked(Vec<String>),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("command {0} was already applied with a different payload")]
    DigestConflict(Uuid),
    #[error("stored receipt for command {0} names no dispatch target: {1}")]
    UnreadableReceipt(Uuid, String),
}

impl CanonicalPortError for CompanyError {
    fn into_kernel_error(self) -> KernelError {
        let message = self.to_string();
        match self {
            Self::Blocked(_) => KernelError::validation(message),
            Self::DigestConflict(_) => KernelError::conflict(message),
            Self::Database(_) | Self::UnreadableReceipt(_, _) => KernelError::internal(message),
        }
    }
}

/// Current canonical company head for one tenant.
///
/// `legal_name` / `reg_no` come from the latest revision's attributes. They are
/// never copied from `organizations` — the runtime role is SELECT-only there,
/// and the provisioning-owned `name` is not the canonical company state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanyHead {
    pub org_id: Uuid,
    pub legal_name: Option<String>,
    pub reg_no: Option<String>,
    pub version: i64,
}

/// The one permitted holder of production DML against `company_revisions`, and
/// the declared owner of `organizations` — which no runtime identity may write.
#[derive(Debug, Clone)]
pub struct PgCompanyPort {
    pool: PgPool,
    runtime: tokio::runtime::Handle,
}

impl PgCompanyPort {
    #[must_use]
    pub const fn new(pool: PgPool, runtime: tokio::runtime::Handle) -> Self {
        Self { pool, runtime }
    }

    /// Latest `company_revisions` row for the armed tenant. No revision, an
    /// invisible org, or unset RLS is `None` — never a fabricated head.
    pub fn get(&self, org_id: OrgId) -> Result<Option<CompanyHead>, CompanyError> {
        self.runtime.block_on(self.read_head(*org_id.as_uuid()))
    }

    /// Zero or one current company head for the armed tenant.
    pub fn list(&self, org_id: OrgId) -> Result<Vec<CompanyHead>, CompanyError> {
        self.runtime
            .block_on(self.read_head(*org_id.as_uuid()))
            .map(|head| head.into_iter().collect())
    }

    async fn arm_org<'e, E>(&self, executor: E, org: Uuid) -> Result<(), CompanyError>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(org.to_string())
            .execute(executor)
            .await?;
        Ok(())
    }

    async fn read_head(&self, org: Uuid) -> Result<Option<CompanyHead>, CompanyError> {
        let mut tx = self.pool.begin().await?;
        self.arm_org(&mut *tx, org).await?;
        let row = sqlx::query(
            "SELECT r.org_id, r.version, r.attributes \
             FROM organizations o \
             JOIN company_revisions r ON r.org_id = o.id \
             WHERE o.id = $1 \
               AND r.version = ( \
                 SELECT MAX(version) FROM company_revisions WHERE org_id = o.id \
               )",
        )
        .bind(org)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row.map(|row| {
            let attributes: serde_json::Value = row.get("attributes");
            CompanyHead {
                org_id: row.get("org_id"),
                legal_name: attr_string(&attributes, "legal_name"),
                reg_no: attr_string(&attributes, "reg_no"),
                version: row.get("version"),
            }
        }))
    }

    async fn write(&self, command: &CompanyCommand) -> Result<CommandReceipt, CompanyError> {
        let preflight = <Self as CanonicalPort>::preflight(&command.query);
        if !preflight.is_ok() {
            return Err(CompanyError::Blocked(preflight.blockers().to_vec()));
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
                return Err(CompanyError::DigestConflict(command_uuid));
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

        // ponytail: MAX + 1 under the row's own transaction. A concurrent revise
        // of the same tenant loses to UNIQUE (org_id, version) with 23505 rather
        // than silently overwriting; add SELECT ... FOR UPDATE on the head if
        // that contention is ever measured.
        let version: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM company_revisions WHERE org_id = $1",
        )
        .bind(org)
        .fetch_one(&mut *tx)
        .await?;

        let target = command.query.target();
        let result = serde_json::json!({
            "org_id": org.to_string(),
            "version": version,
            "target": target.as_str(),
        });

        let created_at: OffsetDateTime = sqlx::query_scalar(
            "INSERT INTO company_revisions \
             (org_id, version, command_id, actor_id, payload_digest, attributes, receipt) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING created_at",
        )
        .bind(org)
        .bind(version)
        .bind(command_uuid)
        .bind(actor)
        .bind(digest.as_slice())
        .bind(&command.query.attributes)
        .bind(&result)
        .fetch_one(&mut *tx)
        .await?;

        // The receipt store, and with it the tenant-global command-id namespace
        // this port shares with every other receipt owner.
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

impl CanonicalPort for PgCompanyPort {
    type Object = Company;
    type Query = CompanyQuery;
    type Command = CompanyCommand;
    type Error = CompanyError;

    /// PURE: no `&self`, no IO, no persistence. A blocked preflight has written
    /// nothing, so it can never spend an approval.
    fn preflight(query: &Self::Query) -> Preflight {
        let blockers = crate::catalog::require_text_property(
            &query.attributes,
            crate::catalog::COMPANY_LEGAL_NAME,
        );
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
        CompanyCommand {
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
    command: &CompanyCommand,
    target: DispatchTarget,
    actor_id: UserId,
    digest: [u8; 32],
    result: serde_json::Value,
    created_at: OffsetDateTime,
) -> CommandReceipt {
    CommandReceipt::new(
        command.org_id,
        command.command_id,
        ReceiptOwner::Canonical(ObjectKey::Company),
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
) -> Result<DispatchTarget, CompanyError> {
    let stored = result["target"]
        .as_str()
        .ok_or_else(|| CompanyError::UnreadableReceipt(command_id, result.to_string()))?;
    DispatchTarget::from_str(stored)
        .map_err(|error| CompanyError::UnreadableReceipt(command_id, error.to_string()))
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
/// [`CompanyError::DigestConflict`] instead of the documented replay.
fn payload_digest(command: &CompanyCommand) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(command.org_id.as_uuid().as_bytes());
    hasher.update(command.command_id.as_uuid().as_bytes());
    hasher.update(command.actor_id.as_uuid().as_bytes());
    hasher.update(command.query.target().as_str().as_bytes());
    hasher.update(
        canonical_json(&command.query.attributes)
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

fn attr_string(attributes: &serde_json::Value, key: &str) -> Option<String> {
    attributes
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Whether `company_revisions` has any row for the tenant (CompanyPort has run).
///
/// `organizations.id` is always the company id; this only answers resolution
/// status for the L5-ORG reference surface.
pub async fn company_has_revision<'e, E>(executor: E, org_id: OrgId) -> Result<bool, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM company_revisions WHERE org_id = $1)")
        .bind(*org_id.as_uuid())
        .fetch_one(executor)
        .await
}

#[cfg(test)]
mod port_error_kind_tests {
    use super::*;
    use console_kernel_core::ErrorKind;
    use console_ontology_canonical_domain::CanonicalPortError;

    #[test]
    fn digest_conflict_is_conflict_not_internal() {
        let kernel = CompanyError::DigestConflict(Uuid::nil()).into_kernel_error();
        assert_eq!(kernel.kind, ErrorKind::Conflict);
    }

    #[test]
    fn blocked_is_validation_and_database_is_internal() {
        assert_eq!(
            CompanyError::Blocked(vec!["x".into()])
                .into_kernel_error()
                .kind,
            ErrorKind::Validation
        );
        assert_eq!(
            CompanyError::Database(sqlx::Error::RowNotFound)
                .into_kernel_error()
                .kind,
            ErrorKind::Internal
        );
    }
}
