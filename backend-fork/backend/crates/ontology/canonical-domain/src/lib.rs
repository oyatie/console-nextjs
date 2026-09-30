//! Canonical object write ports.
//!
//! Six objects own every write to their tables: `CompanyPort`, `OrgUnitPort`,
//! `JobPositionPort`, `PersonPort`, `EmploymentPort`, `PayRunPort`. Each has a
//! typed query, a PURE preflight, and an execute that returns an immutable
//! receipt.
//!
//! This crate is the single source of three things that must never drift apart:
//!
//! 1. the six projected stable object keys,
//! 2. the thirteen dispatch targets, each bound to exactly one key,
//! 3. the writer-ownership registry — which crate may hold DML against which
//!    table — consumed by `console-gate-writer-ownership` (the static half) and
//!    by the database role topology (the load-bearing half).
//!
//! `ObjectKey` and its registry are declared by one macro invocation, so a
//! seventh object cannot be added without also declaring its owner crate and
//! its tables; the gate's roster is sized against `ObjectKey::ALL`, so it cannot
//! pass vacuously.
//!
//! Layer: domain. No sqlx, no axum, no tokio — the layer-boundary gate enforces
//! that, which is what keeps `preflight` honest about being pure.

use console_kernel_core::{KernelError, OrgId, UserId};

// ---------------------------------------------------------------------------
// Object keys and the writer-ownership registry
// ---------------------------------------------------------------------------

/// Declares [`ObjectKey`], its roster, and the writer-ownership registry from
/// one token list. A variant can only be added inside the single invocation
/// below, which extends `ALL`, the key string, the owning crate, and the owned
/// tables in the same edit.
macro_rules! object_keys {
    ($(
        $(#[$doc:meta])*
        $variant:ident => $key:literal, owner = $owner:literal, tables = [$($table:literal),* $(,)?]
    );+ $(;)?) => {
        /// A projected stable object key.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum ObjectKey {
            $($(#[$doc])* $variant,)+
        }

        impl ObjectKey {
            /// Every projected object, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// The projected stable key. Wire-visible; never rename.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $key,)+ }
            }

            /// The one crate permitted to hold production DML against
            /// [`Self::owned_tables`]. Anything else is a second writer.
            #[must_use]
            pub const fn owner_crate(self) -> &'static str {
                match self { $(Self::$variant => $owner,)+ }
            }

            /// Tables this object owns end to end.
            #[must_use]
            pub const fn owned_tables(self) -> &'static [&'static str] {
                match self { $(Self::$variant => &[$($table,)*],)+ }
            }
        }

        impl std::str::FromStr for ObjectKey {
            type Err = UnknownValue;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $($key => Ok(Self::$variant),)+
                    _ => Err(UnknownValue(value.to_owned())),
                }
            }
        }
    };
}

/// A stored string that names no member of a roster.
///
/// Every roster in this crate is write-only without an inverse: dispatch and
/// receipt read-back would each have to re-spell all thirteen target literals,
/// which is precisely the drift this crate exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownValue(pub String);

impl std::fmt::Display for UnknownValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "not a member of the roster: {}", self.0)
    }
}

impl std::error::Error for UnknownValue {}

object_keys! {
    /// `organizations` stays the tenant/current head; `company_revisions` is the
    /// append-only history. No `companies` table is created.
    ///
    /// Current production writers of `organizations`: none. The four matches in
    /// the tree are three `#[cfg(test)]` fixtures (`console-platform-group`,
    /// `console-platform-db`, `console-platform-auth-rest`) and
    /// `console-platform-test-support`, a dev-dependency fixture crate. All four
    /// are justified test-scaffolding exceptions, not port-routed writers, and
    /// the gate excludes both surfaces by rule rather than by name.
    Company => "company",
        owner = "console-ontology-canonical-adapter-postgres",
        tables = ["organizations", "company_revisions"];

    /// New heads/revisions plus unique source-kind/source-ID bindings. Sites stay
    /// operational and are not OrgUnits.
    OrgUnit => "org_unit",
        owner = "console-ontology-canonical-adapter-postgres",
        tables = ["org_units", "org_unit_revisions", "org_unit_source_bindings"];

    /// Heads/revisions referencing OrgUnit. Recruiting postings and employee
    /// position strings are not canonical positions.
    JobPosition => "job_position",
        owner = "console-ontology-canonical-adapter-postgres",
        tables = ["job_positions", "job_position_revisions"];

    /// `persons`/`person_revisions` plus `employee_person_bindings`.
    Person => "person",
        owner = "console-ontology-canonical-adapter-postgres",
        tables = ["persons", "person_revisions", "employee_person_bindings"];

    /// `employees` remains the legacy compatibility head; heads/revisions carry
    /// non-overlapping `[valid_from, valid_to)` history.
    ///
    /// Current production writer of `employees` and the canonical employment
    /// tables: `console-ontology-canonical-adapter-postgres`
    /// (`src/employment.rs`). The `EmploymentPort` lane has landed, fulfilling
    /// the promise this entry documented: org-change reassignment is
    /// PORT-ROUTED, emitting canonical Employment transfer commands rather than
    /// holding its own `employees` DML. The interim owner
    /// `console-orgchange-adapter-postgres` — named here only until that lane
    /// landed, so the gate still rejected any *additional* writer — is
    /// retargeted to the canonical adapter.
    Employment => "employment",
        owner = "console-ontology-canonical-adapter-postgres",
        tables = [
            "employees",
            "employment_heads",
            "employment_revisions",
            "employment_source_bindings",
        ];

    /// The existing payroll tables are REUSED; `PayRunPort` wraps the existing
    /// writer in `console-payroll-adapter-postgres` rather than adding a second
    /// one. `payroll_attendance_material_refs` and `payroll_statutory_rates` are
    /// deliberately NOT listed: they are attendance material and rate reference
    /// data, not the run/line/calculation/exception/disbursement/payslip set.
    PayRun => "pay_run",
        owner = "console-payroll-adapter-postgres",
        tables = [
            "payroll_draft_runs",
            "payroll_draft_lines",
            "payroll_line_calculations",
            "payroll_run_exceptions",
            "payroll_disbursements",
            "payroll_payslip_deliveries",
        ];
}

// ---------------------------------------------------------------------------
// Dispatch targets
// ---------------------------------------------------------------------------

/// Declares [`DispatchTarget`], its roster, and the object each target belongs
/// to from one token list, so a target cannot exist without an owning object.
macro_rules! dispatch_targets {
    ($($variant:ident => $target:literal, $object:ident);+ $(;)?) => {
        /// One accepted command target.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum DispatchTarget {
            $($variant,)+
        }

        impl DispatchTarget {
            /// Every dispatch target, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// The wire string. Never rename.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $target,)+ }
            }

            /// The object that owns this target's writes.
            #[must_use]
            pub const fn object(self) -> ObjectKey {
                match self { $(Self::$variant => ObjectKey::$object,)+ }
            }
        }

        impl std::str::FromStr for DispatchTarget {
            type Err = UnknownValue;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $($target => Ok(Self::$variant),)+
                    _ => Err(UnknownValue(value.to_owned())),
                }
            }
        }
    };
}

dispatch_targets! {
    CompanyRevise => "company.revise", Company;
    OrganizationCreateOrgUnit => "organization.create_org_unit", OrgUnit;
    OrganizationReviseOrgUnit => "organization.revise_org_unit", OrgUnit;
    OrganizationCreateJobPosition => "organization.create_job_position", JobPosition;
    OrganizationReviseJobPosition => "organization.revise_job_position", JobPosition;
    PeopleCreatePerson => "people.create_person", Person;
    PeopleRevisePerson => "people.revise_person", Person;
    HrAppoint => "hr.appoint", Employment;
    HrPromote => "hr.promote", Employment;
    HrTransfer => "hr.transfer", Employment;
    PayrollCreateRun => "payroll.create_run", PayRun;
    PayrollSubmitRun => "payroll.submit_run", PayRun;
    PayrollDecideRun => "payroll.decide_run", PayRun;
}

// ---------------------------------------------------------------------------
// Receipts — migration 0177 generalized, not redesigned
// ---------------------------------------------------------------------------

/// The owner of a stored command receipt.
///
/// # The widening migration, as it must actually be written
///
/// The store is `ont_action_command_receipts`, created by
/// `0177_ontology_action_command_receipts.sql`. An earlier draft of this doc
/// specified `ADD COLUMN owner TEXT NOT NULL`, which is NOT APPLICABLE: it fails
/// on any database that already holds a receipt, and it would have broken the
/// live writer at `backend/crates/ontology/rest/src/lib.rs:1744`, whose INSERT
/// names its columns explicitly
/// (`org_id, command_id, actor_id, payload_digest, receipt, created_at`) and
/// would not have supplied `owner`. It also could not be repaired with a
/// backfill `UPDATE`, because 0177's `BEFORE UPDATE OR DELETE` trigger RAISEs on
/// every row.
///
/// The applicable form needs no backfill and no trigger surgery — `ADD COLUMN`
/// with a DEFAULT is DDL, so no row trigger fires, and `ADD CONSTRAINT` then
/// validates the pre-existing rows as they already stand:
///
/// ```sql
/// ALTER TABLE ont_action_command_receipts
///     ADD COLUMN owner  TEXT NOT NULL DEFAULT 'ontology.action',
///     ADD COLUMN target TEXT;
///
/// ALTER TABLE ont_action_command_receipts
///     ADD CONSTRAINT ont_action_command_receipts_owner_check
///         CHECK (owner IN (…[`ReceiptOwner::owner_check_constraint_sql`]…)),
///     ADD CONSTRAINT ont_action_command_receipts_target_check
///         CHECK (…[`ReceiptOwner::target_check_constraint_sql`]…);
/// ```
///
/// Ordering and callers:
///
/// * The DEFAULT is what makes the existing INSERT at `rest/src/lib.rs:1744`
///   keep working unchanged, so the migration may land BEFORE any caller edit.
/// * `target` is NULLABLE and is constrained to be present exactly when the
///   owner is canonical: the pre-existing `ontology.action` rows have no
///   [`DispatchTarget`], and inventing one for them would be a lie in the store.
///   [`CommandReceipt`] is therefore the CANONICAL receipt only — an
///   `ontology.action` row does not round-trip into one, by construction.
/// * The follow-up (not this bead) drops the DEFAULT once
///   `rest/src/lib.rs:1744` passes `owner` explicitly.
///
/// # The six properties of 0177, preserved
///
/// 1. `PRIMARY KEY (org_id, command_id)` — tenant-global uniqueness. Unchanged;
///    the owner column is deliberately NOT part of the key, so the same command
///    id cannot be replayed under a different owner.
/// 2. `FOREIGN KEY (actor_id, org_id) REFERENCES users(id, org_id)` — actor
///    binding. Unchanged; mirrored by [`CommandReceipt::actor_id`] being
///    inseparable from [`CommandReceipt::org_id`].
/// 3. `payload_digest BYTEA CHECK (octet_length(payload_digest) = 32)` — digest
///    comparison. Unchanged; mirrored by `[u8; 32]`.
/// 4. `BEFORE UPDATE OR DELETE` trigger that RAISEs — rows are immutable.
///    Unchanged; mirrored by [`CommandReceipt`] having no `&mut` accessor and no
///    public field.
/// 5. `receipt JSONB` — stored-result replay. Unchanged.
/// 6. `ENABLE`/`FORCE ROW LEVEL SECURITY` with the `org_isolation` policy.
///    Unchanged; policies survive `ALTER TABLE ... ADD COLUMN`.
///
/// The table deliberately keeps its name. Renaming it would rewrite every query
/// in crates this lane may not touch for zero behavioural gain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReceiptOwner {
    /// The pre-existing owner: ontology instance-action commands.
    OntologyAction,
    /// One of the six canonical objects.
    Canonical(ObjectKey),
}

impl ReceiptOwner {
    /// Every accepted owner. Sized against [`ObjectKey::ALL`], so a seventh
    /// object key cannot reach the receipt store without a CHECK-roster edit.
    pub const ALL: &'static [Self] = &[
        Self::OntologyAction,
        Self::Canonical(ObjectKey::Company),
        Self::Canonical(ObjectKey::OrgUnit),
        Self::Canonical(ObjectKey::JobPosition),
        Self::Canonical(ObjectKey::Person),
        Self::Canonical(ObjectKey::Employment),
        Self::Canonical(ObjectKey::PayRun),
    ];

    /// The table every stored receipt lands in.
    ///
    /// Named once, here, beside the roster that partitions it. The
    /// writer-ownership gate reads this rather than repeating the literal, so a
    /// rename cannot leave the gate guarding a table that no longer exists while
    /// still reporting success.
    pub const RECEIPT_STORE: &'static str = "ont_action_command_receipts";

    /// The single production crate permitted to write THIS owner's receipt rows.
    ///
    /// Derived from [`ObjectKey::owner_crate`] for canonical owners, so the
    /// permitted set cannot drift from the writer-ownership registry: adding a
    /// seventh object key extends this set through the same edit that gives the
    /// key its owner, and no second place needs updating.
    ///
    /// `OntologyAction` is the pre-existing instance-action path, whose writer
    /// is the ontology REST crate — the rows that predate the canonical ports
    /// and are why the store is shared at all.
    #[must_use]
    pub const fn writer_crate(self) -> &'static str {
        match self {
            Self::OntologyAction => "console-ontology-rest",
            Self::Canonical(key) => key.owner_crate(),
        }
    }

    /// The stored `owner` value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OntologyAction => "ontology.action",
            Self::Canonical(key) => key.as_str(),
        }
    }

    /// The exact `owner` CHECK body the widening migration must carry, derived
    /// from [`Self::ALL`] so the SQL cannot be written from a stale roster.
    ///
    /// This is a SPEC artifact, not a runtime path: this lane may not write
    /// `backend/crates/platform/db/migrations/**`, so nothing executes it yet.
    /// It exists so that the migration, when it is written, is copied from the
    /// roster rather than retyped from it — and so a seventh object key makes
    /// `receipt_owner_roster_is_sized_against_object_key_all` fail rather than
    /// silently shipping a CHECK that rejects the new owner.
    #[must_use]
    pub fn owner_check_constraint_sql() -> String {
        let values: Vec<String> = Self::ALL
            .iter()
            .map(|owner| format!("'{}'", owner.as_str()))
            .collect();
        format!("CHECK (owner IN ({}))", values.join(", "))
    }

    /// The `target` CHECK body: present exactly when the owner is canonical,
    /// and then one of the thirteen dispatch targets.
    ///
    /// [`CommandReceipt`] carries a non-optional [`DispatchTarget`], so without
    /// this column a receipt could not survive a store round-trip at all.
    #[must_use]
    pub fn target_check_constraint_sql() -> String {
        let values: Vec<String> = DispatchTarget::ALL
            .iter()
            .map(|target| format!("'{}'", target.as_str()))
            .collect();
        format!(
            "CHECK ((owner = '{}') = (target IS NULL) AND (target IS NULL OR target IN ({})))",
            Self::OntologyAction.as_str(),
            values.join(", ")
        )
    }
}

impl std::str::FromStr for ReceiptOwner {
    type Err = UnknownValue;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "ontology.action" {
            return Ok(Self::OntologyAction);
        }
        value.parse::<ObjectKey>().map(Self::Canonical)
    }
}

/// The client-supplied idempotency key. Unique per tenant, tenant-globally —
/// never per owner, so the same id cannot be replayed under a second owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandId(uuid::Uuid);

impl CommandId {
    #[must_use]
    pub const fn from_uuid(value: uuid::Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_uuid(&self) -> &uuid::Uuid {
        &self.0
    }
}

/// An immutable receipt for one accepted command.
///
/// Constructed once and never mutated: there is no setter and no public field,
/// mirroring the `BEFORE UPDATE OR DELETE` trigger on the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandReceipt {
    org_id: OrgId,
    command_id: CommandId,
    owner: ReceiptOwner,
    target: DispatchTarget,
    actor_id: UserId,
    payload_digest: [u8; 32],
    result: serde_json::Value,
    created_at: time::OffsetDateTime,
}

impl CommandReceipt {
    /// Records an accepted command.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        org_id: OrgId,
        command_id: CommandId,
        owner: ReceiptOwner,
        target: DispatchTarget,
        actor_id: UserId,
        payload_digest: [u8; 32],
        result: serde_json::Value,
        created_at: time::OffsetDateTime,
    ) -> Self {
        Self {
            org_id,
            command_id,
            owner,
            target,
            actor_id,
            payload_digest,
            result,
            created_at,
        }
    }

    /// The tenant. Also the RLS key: a receipt cannot exist without one.
    #[must_use]
    pub const fn org_id(&self) -> OrgId {
        self.org_id
    }

    /// Half of the tenant-global primary key.
    #[must_use]
    pub const fn command_id(&self) -> CommandId {
        self.command_id
    }

    #[must_use]
    pub const fn owner(&self) -> ReceiptOwner {
        self.owner
    }

    #[must_use]
    pub const fn target(&self) -> DispatchTarget {
        self.target
    }

    /// Bound to [`Self::org_id`] by the composite foreign key on the store.
    #[must_use]
    pub const fn actor_id(&self) -> UserId {
        self.actor_id
    }

    /// Exactly 32 bytes, by type. A different digest under the same command id
    /// is a conflict, never a replay.
    #[must_use]
    pub const fn payload_digest(&self) -> &[u8; 32] {
        &self.payload_digest
    }

    /// The stored result replayed verbatim on a repeat of the same command.
    #[must_use]
    pub const fn result(&self) -> &serde_json::Value {
        &self.result
    }

    #[must_use]
    pub const fn created_at(&self) -> time::OffsetDateTime {
        self.created_at
    }
}

// ---------------------------------------------------------------------------
// Ports
// ---------------------------------------------------------------------------

/// A canonical object, as a type. Lets a port trait pin its object in the type
/// system instead of trusting an overridable associated const.
pub trait CanonicalObject {
    /// The projected stable key.
    const KEY: ObjectKey;
}

/// Blockers found by a preflight. Empty means the command may be submitted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preflight {
    blockers: Vec<String>,
}

impl Preflight {
    /// No blockers.
    #[must_use]
    pub const fn ok() -> Self {
        Self {
            blockers: Vec::new(),
        }
    }

    /// Blocked, for the given reasons.
    #[must_use]
    pub fn blocked(blockers: Vec<String>) -> Self {
        Self { blockers }
    }

    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.blockers.is_empty()
    }

    #[must_use]
    pub fn blockers(&self) -> &[String] {
        &self.blockers
    }
}

/// A typed query that knows which member of the dispatch roster it is.
///
/// This is the edge a dispatcher is DERIVED over rather than enumerated on. A
/// registry that maps target → handler must hold one entry per action, so it
/// grows by hand-written Rust every time [`dispatch_targets!`] grows. The
/// inverse edge does not: a decoded query names its own target, so an engine
/// can decode a payload, ask it what it is, and refuse it when the answer is
/// not the target the caller asked for — with no per-action table anywhere.
///
/// Ports implement this by delegating to the inherent `target()` each query
/// already has, so the roster is still spelled exactly once, in the port.
pub trait CanonicalQuery {
    /// The dispatch target this query is.
    ///
    /// Named `dispatch_target`, not `target`, so it can never shadow — or be
    /// shadowed by — the inherent `target()` every port's query already has.
    /// The impls delegate to that inherent one; a same-named trait method would
    /// make the delegation resolve by precedence rules instead of by spelling.
    fn dispatch_target(&self) -> DispatchTarget;

    /// The payload subject this query would write, when the command names one.
    ///
    /// `None` for create-style commands and for writes whose subject is the
    /// tenant itself (company revise). No default: every `CanonicalQuery` impl
    /// must spell the bind (or explicitly return `None`) so a new query cannot
    /// silently inherit "no subject" and skip projected-dispatch `target_id`
    /// comparison. The dispatcher only compares when both
    /// `ProjectedDispatch::target_id` and this value are present.
    fn subject_id(&self) -> Option<uuid::Uuid>;
}

/// Maps a port [`CanonicalPort::Error`] into the shared kernel taxonomy.
///
/// The projected dispatcher used to flatten every port failure through
/// [`KernelError::internal`], so a `DigestConflict` (retry-never) surfaced as
/// HTTP 500 and a `Blocked` preflight as an opaque internal fault. Implementors
/// must preserve the real distinction: conflict → 409, validation → 422,
/// database / unreadable receipt → 500.
pub trait CanonicalPortError: std::fmt::Display + Send {
    /// Consume the port error into a [`KernelError`].
    fn into_kernel_error(self) -> KernelError;
}

impl CanonicalPortError for std::convert::Infallible {
    fn into_kernel_error(self) -> KernelError {
        match self {}
    }
}

/// The shape every canonical object port has: a typed query, a pure preflight,
/// and an execute that returns an immutable receipt.
pub trait CanonicalPort {
    /// Pins which object this port writes.
    type Object: CanonicalObject;
    /// The typed read this port answers.
    type Query: CanonicalQuery;
    /// The typed write this port accepts.
    type Command;
    /// Failure of [`Self::execute`]. Bound so the dispatcher cannot forget the
    /// kind mapping and flatten every variant to internal again.
    type Error: CanonicalPortError;

    /// PURE. No `&self`, no IO, no async, no persistence: a preflight that
    /// cannot reach a connection cannot write PRECHECKED rows, events, audits,
    /// or consume an approval.
    fn preflight(query: &Self::Query) -> Preflight;

    /// Bind a decoded query to the tenant, idempotency key, actor and accepting
    /// action a caller arrived with. PURE, and deliberately not `&self`: it
    /// exists so a generic dispatcher can build `Self::Command` without naming
    /// the concrete type, which is what lets one dispatcher serve every target
    /// this port owns. `action_key` is the ontology action's stable key (unique
    /// only per object type); `object_type_id` disambiguates it, so ports bind
    /// BOTH into their receipt and a replay can reject a retry that reuses the
    /// same `command_id` through a different action or a different object type.
    #[allow(clippy::too_many_arguments)]
    fn command(
        org_id: OrgId,
        command_id: CommandId,
        actor_id: UserId,
        query: Self::Query,
        action_key: &str,
        object_type_id: uuid::Uuid,
    ) -> Self::Command;

    /// Performs the write and returns the stored receipt. A repeat of the same
    /// command id with the same actor and digest replays it.
    ///
    /// # Errors
    /// Returns [`Self::Error`] when authorization, validation, CAS, or the owner
    /// transaction fails. A failed mutation never spends an approval.
    fn execute(&self, command: &Self::Command) -> Result<CommandReceipt, Self::Error>;
}

/// Declares the object marker type, its [`CanonicalObject`] impl, the named port
/// trait, and the blanket impl that makes the name a pure alias for
/// `CanonicalPort<Object = _>` — so an implementer cannot claim the name while
/// writing a different object.
macro_rules! canonical_ports {
    ($($object:ident, $port:ident, $key:ident);+ $(;)?) => {
        $(
            /// Canonical object marker.
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub struct $object;

            impl CanonicalObject for $object {
                const KEY: ObjectKey = ObjectKey::$key;
            }

            /// The named write port for this object.
            pub trait $port: CanonicalPort<Object = $object> {}

            impl<P: CanonicalPort<Object = $object>> $port for P {}
        )+
    };
}

canonical_ports! {
    Company, CompanyPort, Company;
    OrgUnit, OrgUnitPort, OrgUnit;
    JobPosition, JobPositionPort, JobPosition;
    Person, PersonPort, Person;
    Employment, EmploymentPort, Employment;
    PayRun, PayRunPort, PayRun;
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod canonical_contract;
