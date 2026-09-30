//! `PersonPort` — the Postgres implementation of `ObjectKey::Person`.
//!
//! Owned tables, verbatim from the contract: `persons`, `person_revisions`,
//! `employee_person_bindings`.
//!
//! # Where the receipt is stored
//!
//! In `ont_action_command_receipts` — the generalised store 0177 created, the
//! one `console-ontology-rest` already writes. Its `PRIMARY KEY (org_id,
//! command_id)` is what makes a command id tenant-global across OWNERS, which
//! is the property `CommandId` and `ReceiptOwner` state in `canonical-domain`
//! ("the owner column is deliberately NOT part of the key, so the same command
//! id cannot be replayed under a different owner"). A private per-object store
//! would have given this port its own namespace, and one client idempotency key
//! would then have meant two accepted commands.
//!
//! 0177 carries no `owner` and no `target` column yet — the widening migration
//! is specified in `ReceiptOwner`'s doc, unwritten, and this lane may not write
//! `backend/crates/platform/db/migrations/**` — so the `DispatchTarget` travels
//! inside the receipt JSONB as its wire string and read-back parses it with
//! `FromStr`, spelling the thirteen target literals once, in `canonical-domain`.
//! A stored row that names no target is refused, never replayed.
//!
//! `person_revisions.receipt` is NOT NULL, so the revision row carries the same
//! JSON; the row read on replay is the 0177 one.
//!
//! # Append-only, enforced by the database
//!
//! `person_revisions` refuses UPDATE and DELETE (0213's
//! `canonical_person_row_immutable` trigger), so a revision is never edited:
//! `PersonQuery::Revise` appends `MAX(version) + 1`. `employee_person_bindings`
//! refuses UPDATE and carries `PRIMARY KEY (org_id, employee_id)`, so an
//! employee cannot acquire a second binding — while two employees binding to
//! ONE person stays representable on purpose, because that is what the
//! distinct-natural-person four-eyes bar has to detect.
//!
//! # Deterministic bindings (P5 / console-dgo.2)
//!
//! A trusted uniquely-resolved employee is bound with `person_id = employee_id`:
//! `PersonQuery::Create` with `Some(employee_id)` inserts `persons.id` equal to
//! that employee id, then writes the binding row. Omitting `employee_id`
//! (duplicate / review-required imports) creates the person with **no** binding.
//! Bindings are never inferred from name, phone, org text, or other attributes —
//! only an explicit `employee_id` on the command creates a row in
//! `employee_person_bindings`.
//!
//! The TRIGGER is the whole of that enforcement, not a privilege:
//! `ops/postgres-reconcile-topology.sh` grants `console_rt` UPDATE and DELETE on
//! every table a migration creates, which 0213's own header states, so the
//! runtime role holds UPDATE on `person_revisions` in the deployed database.
//!
//! # Synchronous port, async driver
//!
//! `CanonicalPort::execute` is synchronous and `sqlx` is async-only, so
//! [`PgPersonPort`] holds a `tokio::runtime::Handle` and blocks on it.
//! `Handle::block_on` panics when called from a runtime worker thread; an async
//! caller must therefore reach `execute` through `spawn_blocking`.
//! ponytail: one runtime handle, no thread pool of its own — revisit only if
//! the trait ever gains an async form.

use console_kernel_core::{KernelError, OrgId, UserId};
use console_ontology_canonical_domain::{
    CanonicalPort, CanonicalPortError, CanonicalQuery, CommandId, CommandReceipt, DispatchTarget,
    ObjectKey, Person, Preflight, ReceiptOwner,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::str::FromStr;
use time::OffsetDateTime;
use uuid::Uuid;

/// The typed read this port answers: the write a caller intends, and nothing
/// about how it is performed. Each variant is bound to exactly one of the two
/// dispatch targets the contract assigns to `Person`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "target")]
pub enum PersonQuery {
    /// `people.create_person`. When `employee_id` is set (trusted uniquely
    /// resolved), the new person's id **is** that employee id and a binding
    /// row is written. When omitted, the person is created unbound — attributes
    /// never invent a binding.
    #[serde(rename = "people.create_person")]
    Create {
        #[serde(default)]
        employee_id: Option<Uuid>,
        attributes: serde_json::Value,
    },
    /// `people.revise_person`. Appends a revision, and optionally binds a
    /// FURTHER employee record to the same natural person (second employment
    /// of one person — person_id need not equal that further employee_id).
    #[serde(rename = "people.revise_person")]
    Revise {
        person_id: Uuid,
        #[serde(default)]
        employee_id: Option<Uuid>,
        attributes: serde_json::Value,
    },
}

impl PersonQuery {
    /// The dispatch target this query is, spelled once in `canonical-domain`.
    #[must_use]
    pub const fn target(&self) -> DispatchTarget {
        match self {
            Self::Create { .. } => DispatchTarget::PeopleCreatePerson,
            Self::Revise { .. } => DispatchTarget::PeopleRevisePerson,
        }
    }

    #[must_use]
    pub const fn attributes(&self) -> &serde_json::Value {
        match self {
            Self::Create { attributes, .. } | Self::Revise { attributes, .. } => attributes,
        }
    }

    #[must_use]
    pub const fn employee_id(&self) -> Option<Uuid> {
        match self {
            Self::Create { employee_id, .. } | Self::Revise { employee_id, .. } => *employee_id,
        }
    }
}

impl CanonicalQuery for PersonQuery {
    fn dispatch_target(&self) -> DispatchTarget {
        self.target()
    }

    fn subject_id(&self) -> Option<Uuid> {
        match self {
            Self::Create { .. } => None,
            Self::Revise { person_id, .. } => Some(*person_id),
        }
    }
}

/// The typed write this port accepts. `org_id` is the RLS key and `command_id`
/// the tenant-global idempotency key; a repeat replays the stored receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonCommand {
    pub org_id: OrgId,
    pub command_id: CommandId,
    pub actor_id: UserId,
    pub query: PersonQuery,
    pub action_key: String,
    pub object_type_id: Uuid,
}

impl PersonCommand {
    /// Trusted uniquely-resolved directory create: `person_id = employee_id`.
    #[must_use]
    pub fn create_bound(
        org_id: OrgId,
        actor_id: UserId,
        employee_id: Uuid,
        legal_name: &str,
    ) -> Self {
        Self {
            org_id,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id,
            query: PersonQuery::Create {
                employee_id: Some(employee_id),
                attributes: serde_json::json!({ "legal_name": legal_name }),
            },
            action_key: "create_person".to_owned(),
            object_type_id: Uuid::nil(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PersonError {
    #[error("preflight blocked the command: {0:?}")]
    Blocked(Vec<String>),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("command {0} was already applied with a different payload")]
    DigestConflict(Uuid),
    #[error("stored receipt for command {0} names no dispatch target: {1}")]
    UnreadableReceipt(Uuid, String),
}

impl CanonicalPortError for PersonError {
    fn into_kernel_error(self) -> KernelError {
        let message = self.to_string();
        match self {
            Self::Blocked(_) => KernelError::validation(message),
            Self::DigestConflict(_) => KernelError::conflict(message),
            Self::Database(_) | Self::UnreadableReceipt(_, _) => KernelError::internal(message),
        }
    }
}

/// Current canonical Person head. Display/legal names are parsed from the
/// latest revision's attributes; a missing key is omitted, never invented.
///
/// The head is a closed four-field projection. `get`/`list` never copy
/// `person_revisions.attributes` onto the wire, so a stored `phone` (or
/// payroll-ish `salary` / `bank_account` / `rrn`) cannot appear even when
/// those keys sit on the revision JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PersonHead {
    pub id: Uuid,
    pub display_name: Option<String>,
    pub legal_name: Option<String>,
    pub version: i64,
}

/// The one permitted holder of production DML against `persons`,
/// `person_revisions` and `employee_person_bindings`.
#[derive(Debug, Clone)]
pub struct PgPersonPort {
    pool: PgPool,
    runtime: tokio::runtime::Handle,
}

impl PgPersonPort {
    #[must_use]
    pub const fn new(pool: PgPool, runtime: tokio::runtime::Handle) -> Self {
        Self { pool, runtime }
    }

    /// Current head of one Person. A foreign tenant's id is omit-by-RLS
    /// (`None`), never a fabricated row.
    pub fn get(&self, org_id: OrgId, person_id: Uuid) -> Result<Option<PersonHead>, PersonError> {
        self.runtime
            .block_on(self.read_heads(*org_id.as_uuid(), Some(person_id)))
            .map(|heads| heads.into_iter().next())
    }

    /// Current heads in the armed tenant. Empty when none are visible.
    pub fn list(&self, org_id: OrgId) -> Result<Vec<PersonHead>, PersonError> {
        self.runtime
            .block_on(self.read_heads(*org_id.as_uuid(), None))
    }

    async fn arm_org<'e, E>(&self, executor: E, org: Uuid) -> Result<(), PersonError>
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
        person_id: Option<Uuid>,
    ) -> Result<Vec<PersonHead>, PersonError> {
        let mut tx = self.pool.begin().await?;
        self.arm_org(tx.as_mut(), org).await?;
        let rows = sqlx::query(
            "SELECT p.id, r.version, r.attributes \
             FROM persons p \
             JOIN person_revisions r \
               ON r.org_id = p.org_id AND r.person_id = p.id \
             WHERE p.org_id = $1 AND ($2::uuid IS NULL OR p.id = $2) \
               AND r.version = ( \
                 SELECT MAX(version) FROM person_revisions \
                 WHERE org_id = p.org_id AND person_id = p.id \
               ) \
             ORDER BY p.id",
        )
        .bind(org)
        .bind(person_id)
        .fetch_all(tx.as_mut())
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let attributes: serde_json::Value = row.get("attributes");
                person_head(row.get("id"), row.get("version"), &attributes)
            })
            .collect())
    }

    async fn write(&self, command: &PersonCommand) -> Result<CommandReceipt, PersonError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(command.org_id.as_uuid().to_string())
            .execute(tx.as_mut())
            .await?;
        let receipt = write_in_tx(&mut tx, command).await?;
        tx.commit().await?;
        Ok(receipt)
    }
}

/// Apply one Person command inside a caller-owned, org-armed transaction.
///
/// People create uses this so the `employees` row and the Person binding share
/// the directory transaction. Callers must already have set `app.current_org`.
pub async fn write_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    command: &PersonCommand,
) -> Result<CommandReceipt, PersonError> {
    let preflight = <PgPersonPort as CanonicalPort>::preflight(&command.query);
    if !preflight.is_ok() {
        return Err(PersonError::Blocked(preflight.blockers().to_vec()));
    }

    let digest = payload_digest(command);
    let org = *command.org_id.as_uuid();
    let actor = *command.actor_id.as_uuid();
    let command_uuid = *command.command_id.as_uuid();

    if let Some(stored) = sqlx::query(
        "SELECT actor_id, payload_digest, receipt, created_at \
             FROM ont_action_command_receipts WHERE org_id = $1 AND command_id = $2",
    )
    .bind(org)
    .bind(command_uuid)
    .fetch_optional(tx.as_mut())
    .await?
    {
        let stored_digest: Vec<u8> = stored.get("payload_digest");
        if stored_digest != digest {
            return Err(PersonError::DigestConflict(command_uuid));
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
    let (person_id, version) = match &command.query {
        PersonQuery::Create { employee_id, .. } => {
            // Trusted uniquely-resolved: person_id = employee_id (P5).
            // Unbound / review-required: omit employee_id → random id, no binding.
            let person_id: Uuid = match employee_id {
                Some(trusted) => {
                    sqlx::query_scalar(
                        "INSERT INTO persons (org_id, id) VALUES ($1, $2) RETURNING id",
                    )
                    .bind(org)
                    .bind(trusted)
                    .fetch_one(tx.as_mut())
                    .await?
                }
                None => {
                    sqlx::query_scalar("INSERT INTO persons (org_id) VALUES ($1) RETURNING id")
                        .bind(org)
                        .fetch_one(tx.as_mut())
                        .await?
                }
            };
            (person_id, 1_i64)
        }
        PersonQuery::Revise { person_id, .. } => {
            // ponytail: MAX + 1 under the row's own transaction. A
            // concurrent revise of the same person loses to
            // UNIQUE (org_id, person_id, version) with 23505 rather than
            // silently overwriting; add SELECT ... FOR UPDATE on `persons`
            // if that contention is ever measured.
            let next: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(version), 0) + 1 FROM person_revisions \
                     WHERE org_id = $1 AND person_id = $2",
            )
            .bind(org)
            .bind(person_id)
            .fetch_one(tx.as_mut())
            .await?;
            (*person_id, next)
        }
    };

    let result = serde_json::json!({
        "person_id": person_id.to_string(),
        "version": version,
        "target": target.as_str(),
    });

    let created_at: OffsetDateTime = sqlx::query_scalar(
            "INSERT INTO person_revisions \
             (org_id, person_id, version, command_id, actor_id, payload_digest, attributes, receipt) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING created_at",
        )
        .bind(org)
        .bind(person_id)
        .bind(version)
        .bind(command_uuid)
        .bind(actor)
        .bind(digest.as_slice())
        .bind(command.query.attributes())
        .bind(&result)
        .fetch_one(tx.as_mut())
        .await?;

    if let Some(employee_id) = command.query.employee_id() {
        sqlx::query(
            "INSERT INTO employee_person_bindings \
                 (org_id, employee_id, person_id, actor_id, payload_digest) \
                 VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(org)
        .bind(employee_id)
        .bind(person_id)
        .bind(actor)
        .bind(digest.as_slice())
        .execute(tx.as_mut())
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
        .execute(tx.as_mut())
        .await?;

    Ok(receipt(
        command,
        target,
        command.actor_id,
        digest,
        result,
        created_at,
    ))
}

impl CanonicalPort for PgPersonPort {
    type Object = Person;
    type Query = PersonQuery;
    type Command = PersonCommand;
    type Error = PersonError;

    /// PURE: no `&self`, no IO, no persistence. A blocked preflight has written
    /// nothing, so it can never spend an approval.
    fn preflight(query: &Self::Query) -> Preflight {
        let mut blockers = Vec::new();
        if !query.attributes().is_object() {
            blockers.push("attributes must be a JSON object".to_owned());
        }
        if let PersonQuery::Revise { person_id, .. } = query
            && person_id.is_nil()
        {
            blockers.push("person_id must not be nil".to_owned());
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
        PersonCommand {
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
    command: &PersonCommand,
    target: DispatchTarget,
    actor_id: UserId,
    digest: [u8; 32],
    result: serde_json::Value,
    created_at: OffsetDateTime,
) -> CommandReceipt {
    CommandReceipt::new(
        command.org_id,
        command.command_id,
        ReceiptOwner::Canonical(ObjectKey::Person),
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
) -> Result<DispatchTarget, PersonError> {
    let stored = result["target"]
        .as_str()
        .ok_or_else(|| PersonError::UnreadableReceipt(command_id, result.to_string()))?;
    DispatchTarget::from_str(stored)
        .map_err(|error| PersonError::UnreadableReceipt(command_id, error.to_string()))
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
/// [`PersonError::DigestConflict`] instead of the documented replay.
fn payload_digest(command: &PersonCommand) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(command.org_id.as_uuid().as_bytes());
    hasher.update(command.command_id.as_uuid().as_bytes());
    hasher.update(command.actor_id.as_uuid().as_bytes());
    hasher.update(command.query.target().as_str().as_bytes());
    if let PersonQuery::Revise { person_id, .. } = &command.query {
        hasher.update(person_id.as_bytes());
    }
    if let Some(employee_id) = command.query.employee_id() {
        hasher.update(employee_id.as_bytes());
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

/// Closed head projection: names only. Does not flatten or pass through the
/// attributes object, so `phone` / `salary` / `bank_account` / `rrn` cannot
/// ride along even when they are present on the revision.
fn person_head(id: Uuid, version: i64, attributes: &serde_json::Value) -> PersonHead {
    PersonHead {
        id,
        display_name: attr_string(attributes, "display_name"),
        legal_name: attr_string(attributes, "legal_name"),
        version,
    }
}

fn attr_string(attributes: &serde_json::Value, key: &str) -> Option<String> {
    attributes
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
