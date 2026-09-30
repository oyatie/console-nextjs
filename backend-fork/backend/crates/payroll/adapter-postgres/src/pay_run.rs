//! `PayRunPort` — the Postgres implementation of `ObjectKey::PayRun`, and the
//! home of the ONE `payroll_draft_runs` statement that used to live outside the
//! owning crate.
//!
//! Owned tables, verbatim from the contract
//! (`backend/crates/ontology/canonical-domain/src/lib.rs`):
//! `payroll_draft_runs`, `payroll_draft_lines`, `payroll_line_calculations`,
//! `payroll_run_exceptions`, `payroll_disbursements`,
//! `payroll_payslip_deliveries`. Every one of them already exists (migrations
//! 0074 and 0186); this lane creates no table and appends to no table list.
//!
//! # This port WRAPS the existing writer; it does not become a second one
//!
//! The contract is explicit that `PayRunPort` reuses the payroll tables rather
//! than adding a parallel set, and the writer-ownership gate exists to reject
//! exactly the defect a fresh set of statements here would be. So:
//!
//! * `payroll.submit_run` calls [`crate::lifecycle::submit_run_in_tx`],
//! * `payroll.decide_run` calls [`crate::lifecycle::decide_run_in_tx`],
//!
//! both already in this crate, both already carrying their own state machine
//! (`CALCULATED → SUBMITTED → APPROVED/REJECTED`), their exception gate and
//! their segregation-of-duties check. The port adds the canonical receipt and
//! the idempotency replay around them; it does not restate their SQL.
//!
//! `payroll.create_run` is the one target with no pre-existing owner statement,
//! because the statement that created a draft run lived in
//! `console-workflow-runtime-adapter-postgres` — the ratcheted dual writer this
//! lane removes. [`stage_draft_run_in_tx`] is that statement, moved here
//! unchanged in meaning, and it now has two callers: this port, and the JOB
//! outbox drain through [`PgPayRunPort`]'s [`PayrollDraftStaging`] impl.
//!
//! # Why the drain reaches this crate through a trait
//!
//! `backend/ci/gates/layer-boundary` forbids an adapter crate depending on
//! another adapter crate (`Layer::Adapter`'s allowed edges are application,
//! contracts, domain, kernel and platform — not adapter). The workflow runtime
//! adapter therefore CANNOT name `console-payroll-adapter-postgres` as a
//! dependency, and a "just call the owner" edit would have swapped a
//! writer-ownership violation for a layer violation.
//!
//! The seam is [`console_workflow_domain::PayrollDraftStaging`], declared by the
//! CONSUMER in its own domain crate — the same shape as
//! `console_notifications_application::NotificationSink`, which the very same
//! drain loop already uses for the notification bridge. `console-app` wires the
//! implementer in, exactly as it wires the notification sink.
//!
//! # What moved, and the one property that changed with it
//!
//! Before: claim → INSERT draft → ack DELIVERED → audit, all in ONE
//! `with_audits` transaction. After: claim (its own read transaction) → stage
//! the draft through this port (its own transaction) → ack + audit
//! (`with_audits`). The staging happens BEFORE the ack, and the ack no longer
//! shares the draft's transaction.
//!
//! That is safe in the direction that matters and the ORDER is what makes it so:
//!
//! * crash after staging, before the ack — the event stays `PENDING`, the next
//!   tick re-claims it, [`stage_draft_run_in_tx`]'s
//!   `ON CONFLICT (org_id, period_start, period_end, source_label)` restage
//!   returns `false` when the provenance matches, and the ack then lands. No
//!   double draft, no lost draft. A restage whose connector/job differs is
//!   refused fail-closed ([`StageDraftError::ProvenanceMismatch`]) rather than
//!   silently absorbed;
//! * crash before staging — nothing was written and the event is still
//!   `PENDING`.
//!
//! The reverse order would be the unsafe one: acking first could lose a draft
//! forever. The property genuinely given up is "the draft and its ack roll back
//! together", which was tidiness rather than safety — the natural key was always
//! the exactly-once mechanism, as 0074's `UNIQUE (org_id, period_start,
//! period_end, source_label)` and the drain's own doc comment already said. It
//! is the identical trade `drain_notification_outbox` documents three functions
//! further down the same file ("emit + mark are each idempotent, so a re-claim
//! is safe").
//!
//! Because staging now opens its own transaction, [`PayrollDraftStaging::stage`]
//! re-runs the freeze-window gate inside that transaction — folded INTO the
//! staging INSERT's `WHERE NOT EXISTS` so the check and the write share one
//! snapshot. The drain's phase-1 read and the staging write are two
//! transactions, so a payroll period lock acquired between them is refused by
//! the write atomically, not by a separate SELECT that a concurrent lock INSERT
//! can slip past under READ COMMITTED.
//!
//! # Where the receipt is stored
//!
//! In `ont_action_command_receipts`, the 0177 store, exactly as `PersonPort` and
//! `EmploymentPort` do. `PRIMARY KEY (org_id, command_id)` is what makes a
//! command id tenant-global ACROSS owners, so one client idempotency key can
//! never mean two accepted commands. 0177 carries no `owner`/`target` column
//! yet, so the `DispatchTarget` travels inside the receipt JSONB as its wire
//! string and read-back parses it with `FromStr`. A stored row that names no
//! target is refused, never replayed.
//!
//! # Synchronous port, async driver
//!
//! `CanonicalPort::execute` is synchronous and `sqlx` is async-only, so
//! [`PgPayRunPort`] holds a `tokio::runtime::Handle` and blocks on it.
//! `Handle::block_on` panics when called from a runtime worker thread; an async
//! caller must reach `execute` through `spawn_blocking`. The
//! [`PayrollDraftStaging`] impl is async and does NOT go through `execute`, so
//! the drain — which is already on a runtime — never blocks a worker.
//! ponytail: one runtime handle, no thread pool of its own — revisit only if the
//! trait ever gains an async form.

use console_kernel_core::{KernelError, OrgId, UserId};
use console_ontology_canonical_domain::{
    CanonicalPort, CanonicalPortError, CanonicalQuery, CommandId, CommandReceipt, DispatchTarget,
    ObjectKey, PayRun, Preflight, ReceiptOwner,
};
use console_workflow_domain::{PayrollDraftStaging, PortFuture, StagePayrollDraft};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::str::FromStr;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// The one statement that moved
// ---------------------------------------------------------------------------

/// Error raised while staging one `payroll_draft_runs` row. Three arms: the
/// driver failed; the natural key already held a row whose PROVENANCE
/// (`connector`/`job` in `source_summary`) differs from the requested write; or
/// the payroll period is frozen (the drain's gated staging write refused).
#[derive(Debug, thiserror::Error)]
pub enum StageDraftError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),

    /// A fresh command/event reusing the same run and period with a DIFFERENT
    /// connector/job. Fail-closed: the requested provenance is never silently
    /// dropped onto an existing row.
    #[error("a payroll draft for this run and period already exists with different provenance")]
    ProvenanceMismatch,

    /// The staging INSERT's atomic freeze-window gate saw an active
    /// `period_locks` row and refused the write (drain path only).
    #[error("payroll period is locked; draft not staged")]
    PeriodLocked,
}

/// The `source_summary` JSONB a staging write requests. Spelled once, because
/// the fail-closed conflict arm must compare what was REQUESTED against what is
/// STORED, and both must be built from the same field set.
fn draft_source_summary(draft: &StagePayrollDraft) -> serde_json::Value {
    serde_json::json!({
        "outbox_event_id": draft.outbox_event_id,
        "run_id": draft.run_id,
        "connector": draft.connector,
        "job": draft.job,
    })
}

/// Whether a stored `source_summary`'s provenance matches the requested one.
///
/// Only `connector` and `job` are compared: `run_id` and the period are in the
/// natural key, so on conflict they are equal by construction, and
/// `outbox_event_id` is the caller's correlation id (the command id / outbox
/// event), not provenance — a re-claim or a fresh command with the SAME
/// connector/job is an idempotent replay, not a mismatch.
///
/// The comparison is on field presence, JSON type, AND value: the unconstrained
/// JSONB column may hold a missing key (its `{}` default) or a non-string, and a
/// stored `connector`/`job` of either shape is REFUSED as a mismatch, never
/// normalized to absence via `as_str()`.
fn provenance_matches(stored: &serde_json::Value, requested: &serde_json::Value) -> bool {
    stored.get("connector") == requested.get("connector")
        && stored.get("job") == requested.get("job")
}

/// The staging INSERT WITHOUT the freeze-window gate — the canonical
/// `payroll.create_run` path. The gated variant below differs from it by exactly
/// one `WHERE NOT EXISTS`, so the two statements cannot drift apart.
const INSERT_DRAFT_RUN_SQL: &str = "INSERT INTO payroll_draft_runs \
         (org_id, period_start, period_end, source_label, status, source_summary) \
     VALUES ($1, $2, $3, $4, 'BLOCKED_LEGAL_GATE', $5) \
     ON CONFLICT (org_id, period_start, period_end, source_label) \
     DO UPDATE SET source_label = EXCLUDED.source_label \
     RETURNING id, source_summary, (xmax = 0) AS created";

/// The staging INSERT with the freeze-window gate folded INTO the statement —
/// the drain path. The `WHERE NOT EXISTS` re-reads `period_locks` under the SAME
/// snapshot as the write, so a lock committed after the drain's phase-1 read but
/// before this statement is refused atomically instead of slipped past (the
/// READ COMMITTED gap between a separate SELECT and the INSERT). `$2`/`$3` are
/// `NULL` for a periodless draft, which makes the guard vacuous and lets the
/// `NOT NULL` constraint refuse it exactly as before.
const INSERT_DRAFT_RUN_GATED_SQL: &str = "INSERT INTO payroll_draft_runs \
         (org_id, period_start, period_end, source_label, status, source_summary) \
     SELECT $1, $2, $3, $4, 'BLOCKED_LEGAL_GATE', $5 \
     WHERE NOT EXISTS ( \
         SELECT 1 FROM period_locks \
         WHERE domain = 'payroll' AND unlocked_at IS NULL \
           AND period_start <= $3 AND period_end >= $2 \
     ) \
     ON CONFLICT (org_id, period_start, period_end, source_label) \
     DO UPDATE SET source_label = EXCLUDED.source_label \
     RETURNING id, source_summary, (xmax = 0) AS created";

/// The shared staging write, optionally carrying the freeze-window gate.
///
/// The gated (drain) path first takes the per-org advisory lock
/// ([`console_platform_db::lock_period_lock_key`]), which serializes staging
/// with lock creation, then re-reads any EXISTING natural-key row so an
/// already-staged draft with identical provenance is acknowledged (`Ok(false)`)
/// even when the period has since been locked — the freeze gate applies only to
/// a genuinely NEW write ([`INSERT_DRAFT_RUN_GATED_SQL`]); when that gate
/// refuses, no row comes back and the caller sees [`StageDraftError::PeriodLocked`].
/// The provenance check runs on every conflict either way.
/// Materialise the roster for a staged run, in the caller's transaction.
///
/// Called from EVERY success path of `stage_draft_run_inner`, including the
/// idempotent one that returns `created = false`. Gating this on `created` would
/// mean a run whose header exists but whose roster was never written — because a
/// previous attempt died between the two — could never acquire one, and after the
/// close preflight learned to require `roster_total > 0` that run is stuck
/// forever with no way to repair it.
///
/// An empty roster is NOT an error here. The drain leaves a failed event PENDING
/// without incrementing `attempt_count`, so returning `Err` would be an unbounded
/// hot retry; `close_preflight` already refuses an empty roster legibly.
async fn materialise_roster_for(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    id: Uuid,
    draft: &StagePayrollDraft,
) -> Result<(), StageDraftError> {
    // A draft with no declared period has no scope, so there is no set of import
    // rows it could honestly claim. Writing nothing is the only truthful option:
    // guessing a period is exactly the fabricated provenance migration 0224 exists
    // to remove. The run is still created; the close preflight refuses its empty
    // roster legibly.
    let (Some(period_start), Some(period_end)) = (draft.period_start, draft.period_end) else {
        return Ok(());
    };
    crate::roster::materialise_roster_in_tx(tx, org_id, id, period_start, period_end).await?;
    Ok(())
}

async fn stage_draft_run_inner(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    draft: &StagePayrollDraft,
    gated: bool,
) -> Result<(Uuid, bool), StageDraftError> {
    let requested = draft_source_summary(draft);
    if gated {
        // Serialize with period-lock creation: both this gated write and the
        // close-month lock creator take the same per-org advisory lock, so a
        // lock cannot commit between this statement's snapshot and its commit
        // (the remaining READ COMMITTED gap the NOT EXISTS alone cannot close).
        console_platform_db::lock_period_lock_key(
            tx,
            console_platform_db::PeriodLockDomain::Payroll,
            org_id,
        )
        .await?;
        // Idempotency BEFORE the freeze gate: an already-staged draft with
        // identical provenance must be acknowledged so the drain can ack its
        // outbox event, even though no new write is needed and the period is
        // now locked.
        let existing: Option<(Uuid, serde_json::Value)> = sqlx::query_as(
            "SELECT id, source_summary FROM payroll_draft_runs \
             WHERE org_id = $1 AND period_start IS NOT DISTINCT FROM $2 \
               AND period_end IS NOT DISTINCT FROM $3 AND source_label = $4 \
             LIMIT 1",
        )
        .bind(org_id)
        .bind(draft.period_start)
        .bind(draft.period_end)
        .bind(draft.source_label())
        .fetch_optional(tx.as_mut())
        .await?;
        if let Some((id, stored)) = existing {
            if !provenance_matches(&stored, &requested) {
                return Err(StageDraftError::ProvenanceMismatch);
            }
            materialise_roster_for(tx, org_id, id, draft).await?;
            return Ok((id, false));
        }
        // No existing draft: the freeze-window gate applies to this NEW write.
        let row: Option<(Uuid, serde_json::Value, bool)> =
            sqlx::query_as(INSERT_DRAFT_RUN_GATED_SQL)
                .bind(org_id)
                .bind(draft.period_start)
                .bind(draft.period_end)
                .bind(draft.source_label())
                .bind(&requested)
                .fetch_optional(tx.as_mut())
                .await?;
        let Some((id, stored, created)) = row else {
            return Err(StageDraftError::PeriodLocked);
        };
        if !created && !provenance_matches(&stored, &requested) {
            return Err(StageDraftError::ProvenanceMismatch);
        }
        materialise_roster_for(tx, org_id, id, draft).await?;
        Ok((id, created))
    } else {
        let row: (Uuid, serde_json::Value, bool) = sqlx::query_as(INSERT_DRAFT_RUN_SQL)
            .bind(org_id)
            .bind(draft.period_start)
            .bind(draft.period_end)
            .bind(draft.source_label())
            .bind(&requested)
            .fetch_one(tx.as_mut())
            .await?;
        let (id, stored, created) = row;
        if !created && !provenance_matches(&stored, &requested) {
            return Err(StageDraftError::ProvenanceMismatch);
        }
        materialise_roster_for(tx, org_id, id, draft).await?;
        Ok((id, created))
    }
}

/// Stages one `payroll_draft_runs` row for the DRAIN path, idempotently on the
/// natural key AND gated on the payroll freeze window — both in ONE statement.
/// `true` when this call created the row.
///
/// The freeze-window gate is the same predicate as
/// [`console_platform_db::assert_period_open_range`], but folded into the INSERT
/// itself so the check and the write share one snapshot. The statement is also
/// serialized against period-lock creation by the per-org advisory lock
/// ([`console_platform_db::lock_period_lock_key`]), so a lock committed either
/// between the drain's phase-1 read and this write, or between this statement's
/// snapshot and its commit, is refused as [`StageDraftError::PeriodLocked`] —
/// never slipped past.
///
/// The status is `BLOCKED_LEGAL_GATE` and `calculation_enabled` is left at its
/// column default (`FALSE`), so nothing calculates without the legal gate —
/// 0074's `payroll_draft_runs_enabled_requires_review` CHECK would refuse the
/// combination anyway, and stating the status rather than defaulting it is what
/// the workflow drain did.
///
/// `period_start`/`period_end` are bound as `Option`: an outbox payload that
/// carries neither produces the same `23502 not_null_violation` the previous
/// `(payload->>'period_start')::date` form produced, rather than a silently
/// defaulted period.
///
/// # Errors
/// Returns the driver error verbatim so the caller can distinguish a missing
/// period (`23502`) from a tenant it may not write (`42501`). A conflict on the
/// natural key with a DIFFERENT connector/job is
/// [`StageDraftError::ProvenanceMismatch`]; a locked period is
/// [`StageDraftError::PeriodLocked`].
pub async fn stage_draft_run_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    draft: &StagePayrollDraft,
) -> Result<bool, StageDraftError> {
    stage_draft_run_inner(tx, org_id, draft, true)
        .await
        .map(|(_, created)| created)
}

/// The same staging INSERT for the CANONICAL `payroll.create_run` path,
/// returning the row's own identifier. NOT gated on the freeze window: the
/// canonical port reaches this directly, and the period gate belongs to the
/// drain seam, not this caller.
///
/// # Why this exists, and why `DO NOTHING` could not be kept
///
/// `payroll_draft_runs.id` is `DEFAULT gen_random_uuid()` (migration 0074), and
/// `StagePayrollDraft::run_id` is a different identifier entirely: the workflow
/// drain's deterministic natural key, which reaches this table only inside
/// `source_label` and the `source_summary` JSONB. That split is by design and
/// predates the canonical port.
///
/// What did NOT predate it is a caller who must go from one to the other.
/// `PayRunQuery::SubmitRun` and `DecideRun` resolve through `run_head`, which is
/// `WHERE id = $1` against the PRIMARY KEY, so a `CreateRun` receipt carrying
/// only the workflow run id names a row that `WHERE id` can never find. The
/// three targets are one lifecycle and must be traversable in order.
///
/// `DO NOTHING` cannot serve that: on conflict it returns no row, so precisely
/// the idempotent replay — the case the receipt store exists for — would get no
/// identifier. The conflict arm is therefore a no-op `DO UPDATE` that rewrites
/// `source_label` to the value it already holds, purely so `RETURNING` yields
/// the row. `xmax = 0` distinguishes a fresh insert from a conflict, which is
/// what the previous `is_some()` meant.
///
/// # Fail-closed on a changed provenance
///
/// The conflict arm returns the stored `source_summary`, and a conflict whose
/// provenance (`connector`/`job`) differs from the request is refused as
/// [`StageDraftError::ProvenanceMismatch`]. Before this check, a fresh
/// `command_id` reusing the same run id and period with a different
/// connector/job got `created:false` plus success while the requested
/// provenance was never stored — and the port handed back the row's identifier,
/// so a caller could act on a row whose provenance disagreed with the request.
/// Refusing is consistent with [`PayRunError::DigestConflict`]'s treatment of a
/// changed payload under one command id.
pub async fn stage_draft_run_returning_id_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    draft: &StagePayrollDraft,
) -> Result<(Uuid, bool), StageDraftError> {
    stage_draft_run_inner(tx, org_id, draft, false).await
}

// ---------------------------------------------------------------------------
// The canonical port
// ---------------------------------------------------------------------------

/// The two decisions `payroll.decide_run` admits, spelled here so a typo is a
/// preflight blocker rather than a `LifecycleError::Validation` discovered after
/// a transaction is already open.
const DECISIONS: [&str; 2] = ["APPROVE", "REJECT"];

/// The typed read this port answers: the write a caller intends, one variant per
/// dispatch target the contract assigns to `PayRun`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "target")]
pub enum PayRunQuery {
    /// `payroll.create_run`. Stages a draft run under a caller-chosen source
    /// label, idempotent on the natural key.
    #[serde(rename = "payroll.create_run")]
    CreateRun {
        run_id: Uuid,
        period_start: Date,
        period_end: Date,
        #[serde(default)]
        connector: Option<String>,
        #[serde(default)]
        job: Option<String>,
    },
    /// `payroll.submit_run`. `CALCULATED → SUBMITTED`, refused while any
    /// exception is open.
    #[serde(rename = "payroll.submit_run")]
    SubmitRun { run_id: Uuid },
    /// `payroll.decide_run`. `SUBMITTED → APPROVED|REJECTED`, refused when the
    /// decider is the submitter.
    #[serde(rename = "payroll.decide_run")]
    DecideRun {
        run_id: Uuid,
        decision: String,
        #[serde(default)]
        reason: Option<String>,
    },
}

impl PayRunQuery {
    /// The dispatch target this query is, spelled once in `canonical-domain`.
    #[must_use]
    pub const fn target(&self) -> DispatchTarget {
        match self {
            Self::CreateRun { .. } => DispatchTarget::PayrollCreateRun,
            Self::SubmitRun { .. } => DispatchTarget::PayrollSubmitRun,
            Self::DecideRun { .. } => DispatchTarget::PayrollDecideRun,
        }
    }

    #[must_use]
    pub const fn run_id(&self) -> Uuid {
        match self {
            Self::CreateRun { run_id, .. }
            | Self::SubmitRun { run_id }
            | Self::DecideRun { run_id, .. } => *run_id,
        }
    }
}

impl CanonicalQuery for PayRunQuery {
    fn dispatch_target(&self) -> DispatchTarget {
        self.target()
    }

    fn subject_id(&self) -> Option<Uuid> {
        Some(self.run_id())
    }
}

/// The typed write this port accepts. `org_id` is the RLS key and `command_id`
/// the tenant-global idempotency key; a repeat replays the stored receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayRunCommand {
    pub org_id: OrgId,
    pub command_id: CommandId,
    pub actor_id: UserId,
    pub query: PayRunQuery,
    pub action_key: String,
    pub object_type_id: Uuid,
}

#[derive(Debug, thiserror::Error)]
pub enum PayRunError {
    #[error("preflight blocked the command: {0:?}")]
    Blocked(Vec<String>),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Lifecycle(#[from] crate::lifecycle::LifecycleError),
    #[error("command {0} was already applied with a different payload")]
    DigestConflict(Uuid),
    #[error("a payroll draft for this run and period already exists with different provenance")]
    ProvenanceConflict,
    #[error("payroll period is locked; draft not staged")]
    PeriodLocked,
    #[error("stored receipt for command {0} names no dispatch target: {1}")]
    UnreadableReceipt(Uuid, String),
}

impl CanonicalPortError for PayRunError {
    fn into_kernel_error(self) -> KernelError {
        match self {
            Self::Blocked(_) => KernelError::validation(self.to_string()),
            Self::DigestConflict(_) | Self::ProvenanceConflict | Self::PeriodLocked => {
                KernelError::conflict(self.to_string())
            }
            Self::Lifecycle(err) => lifecycle_into_kernel_error(err),
            Self::Database(_) | Self::UnreadableReceipt(_, _) => {
                KernelError::internal(self.to_string())
            }
        }
    }
}

/// Map payroll lifecycle failures onto [`KernelError`] kinds that match the
/// REST `from_lifecycle` status surface (404 / 422 / 409 / 500), instead of
/// flattening every arm to `Internal` via [`PayRunError::Lifecycle`].
fn lifecycle_into_kernel_error(err: crate::lifecycle::LifecycleError) -> KernelError {
    use crate::lifecycle::LifecycleError as E;
    let message = err.to_string();
    match err {
        E::NotFound => KernelError::not_found(message),
        E::Validation(_) => KernelError::validation(message),
        E::InvalidTransition(_) => KernelError::invalid_transition(message),
        E::InvalidState(_)
        | E::PreflightBlocked(_)
        | E::ExceptionsOpen(_)
        | E::SodViolation
        | E::AlreadyResolved
        | E::LegalGate(_) => KernelError::conflict(message),
        E::Db(_) => KernelError::internal(message),
    }
}

/// Map the staging writer's failure arms onto the port's error surface: a
/// driver failure is [`PayRunError::Database`], a changed provenance is
/// [`PayRunError::ProvenanceConflict`], a locked period is
/// [`PayRunError::PeriodLocked`] (unreachable from the ungated canonical path).
impl From<StageDraftError> for PayRunError {
    fn from(err: StageDraftError) -> Self {
        match err {
            StageDraftError::Db(db) => Self::Database(db),
            StageDraftError::ProvenanceMismatch => Self::ProvenanceConflict,
            StageDraftError::PeriodLocked => Self::PeriodLocked,
        }
    }
}

/// The one permitted holder of production DML against the six `payroll_*` tables
/// the contract assigns to `ObjectKey::PayRun`.
#[derive(Debug, Clone)]
pub struct PgPayRunPort {
    pool: PgPool,
    runtime: tokio::runtime::Handle,
}

impl PgPayRunPort {
    #[must_use]
    pub const fn new(pool: PgPool, runtime: tokio::runtime::Handle) -> Self {
        Self { pool, runtime }
    }

    async fn write(&self, command: &PayRunCommand) -> Result<CommandReceipt, PayRunError> {
        let preflight = <Self as CanonicalPort>::preflight(&command.query);
        if !preflight.is_ok() {
            return Err(PayRunError::Blocked(preflight.blockers().to_vec()));
        }

        let digest = payload_digest(command);
        let org = *command.org_id.as_uuid();
        let actor = *command.actor_id.as_uuid();
        let command_uuid = *command.command_id.as_uuid();

        let mut tx = self.pool.begin().await?;
        // Transaction-local, so it is cleared on COMMIT/ROLLBACK and never leaks
        // to the next checkout of a pooled connection. Unset fails closed: RLS
        // shows no rows and accepts no writes.
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
                return Err(PayRunError::DigestConflict(command_uuid));
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
        let run_id = command.query.run_id();
        // The row's own identifier, which is NOT `run_id`: see
        // `stage_draft_run_returning_id_in_tx`. Only CreateRun learns it, because only CreateRun
        // makes the row; Submit and Decide were handed it by a prior CreateRun receipt.
        let mut draft_run_id: Option<Uuid> = None;

        // The three arms are the three targets, and each one is a CALL into the
        // statements this crate already owns — never a restatement of them.
        let created = match &command.query {
            PayRunQuery::CreateRun {
                period_start,
                period_end,
                connector,
                job,
                ..
            } => {
                let draft = StagePayrollDraft {
                    org: command.org_id,
                    outbox_event_id: command_uuid,
                    run_id,
                    period_start: Some(*period_start),
                    period_end: Some(*period_end),
                    connector: connector.clone(),
                    job: job.clone(),
                };
                let (id, created) =
                    stage_draft_run_returning_id_in_tx(&mut tx, org, &draft).await?;
                draft_run_id = Some(id);
                created
            }
            PayRunQuery::SubmitRun { .. } => {
                crate::lifecycle::submit_run_in_tx(&mut tx, run_id, actor).await?;
                true
            }
            PayRunQuery::DecideRun {
                decision, reason, ..
            } => {
                crate::lifecycle::decide_run_in_tx(
                    &mut tx,
                    run_id,
                    actor,
                    decision,
                    reason.as_deref(),
                )
                .await?;
                true
            }
        };

        // `run_id` is echoed back because the caller chose it and correlates on it. `draft_run_id`
        // is the value `SubmitRun`/`DecideRun` must be given: they resolve through `run_head`, which
        // keys the PRIMARY KEY. Emitting only `run_id` shipped a create whose own successors could
        // never resolve its output, and the port's test masked it by fetching the id behind the port
        // through the owner pool -- a back channel no caller on the action surface has.
        let mut result = serde_json::json!({
            "run_id": run_id.to_string(),
            "created": created,
            "target": target.as_str(),
        });
        if let Some(id) = draft_run_id {
            result["draft_run_id"] = serde_json::Value::String(id.to_string());
        }

        // The receipt store, and with it the tenant-global command-id namespace
        // this port shares with every other receipt owner. `created_at` is
        // supplied rather than defaulted: 0177 declares it `NOT NULL` with NO
        // DEFAULT, so an INSERT that omits it is a `23502`.
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
        let created_at: OffsetDateTime = sqlx::query_scalar(
            "INSERT INTO ont_action_command_receipts \
             (org_id, command_id, actor_id, payload_digest, receipt, action_key, object_type_id, created_at, owner, target) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, now(), $8, $9) RETURNING created_at",
        )
        .bind(org)
        .bind(command_uuid)
        .bind(actor)
        .bind(digest.as_slice())
        .bind(&result)
        .bind(&command.action_key)
        .bind(command.object_type_id)
        .bind(receipt_owner.as_str())
        .bind(receipt_target.as_str())
        .fetch_one(&mut *tx)
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

impl CanonicalPort for PgPayRunPort {
    type Object = PayRun;
    type Query = PayRunQuery;
    type Command = PayRunCommand;
    type Error = PayRunError;

    /// PURE: no `&self`, no IO, no persistence. A blocked preflight has written
    /// nothing, so it can never spend an approval.
    fn preflight(query: &Self::Query) -> Preflight {
        let mut blockers = Vec::new();
        if query.run_id().is_nil() {
            blockers.push("run_id must not be nil".to_owned());
        }
        match query {
            PayRunQuery::CreateRun {
                period_start,
                period_end,
                ..
            } => {
                // 0074's `payroll_draft_runs_valid_period` CHECK, asked here so
                // an inverted period never opens a transaction.
                if period_end < period_start {
                    blockers.push("period_end must not precede period_start".to_owned());
                }
            }
            PayRunQuery::DecideRun {
                decision, reason, ..
            } => {
                if !DECISIONS.contains(&decision.as_str()) {
                    blockers.push(format!("decision must be one of {DECISIONS:?}"));
                }
                if decision == "REJECT" && reason.as_ref().is_none_or(|r| r.trim().is_empty()) {
                    blockers.push("a reason is required to reject a payroll run".to_owned());
                }
            }
            PayRunQuery::SubmitRun { .. } => {}
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
        PayRunCommand {
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

/// The seam the JOB outbox drain reaches this crate through. One transaction per
/// draft, armed for the tenant, idempotent on the natural key — which is what
/// lets the drain stage before it acks and still be exactly-once.
impl PayrollDraftStaging for PgPayRunPort {
    fn stage<'a>(&'a self, draft: StagePayrollDraft) -> PortFuture<'a, bool> {
        Box::pin(async move {
            let org = draft.org;
            console_platform_db::with_org_conn::<_, bool, crate::PgPayrollError>(
                &self.pool,
                org,
                move |tx| {
                    Box::pin(async move {
                        // `stage_draft_run_in_tx` folds the freeze-window gate
                        // INTO the INSERT (WHERE NOT EXISTS), so the check and
                        // the write share one snapshot — a period lock committed
                        // after the drain's phase-1 read but before this write is
                        // refused atomically, never slipped past.
                        stage_draft_run_in_tx(tx, *org.as_uuid(), &draft)
                            .await
                            .map_err(crate::PgPayrollError::from)
                    })
                },
            )
            .await
            .map_err(console_kernel_core::KernelError::from)
        })
    }
}

fn receipt(
    command: &PayRunCommand,
    target: DispatchTarget,
    actor_id: UserId,
    digest: [u8; 32],
    result: serde_json::Value,
    created_at: OffsetDateTime,
) -> CommandReceipt {
    CommandReceipt::new(
        command.org_id,
        command.command_id,
        ReceiptOwner::Canonical(ObjectKey::PayRun),
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
) -> Result<DispatchTarget, PayRunError> {
    let stored = result["target"]
        .as_str()
        .ok_or_else(|| PayRunError::UnreadableReceipt(command_id, result.to_string()))?;
    DispatchTarget::from_str(stored)
        .map_err(|error| PayRunError::UnreadableReceipt(command_id, error.to_string()))
}

/// The 32 bytes the `payload_digest` CHECK is sized for.
///
/// Every field is hashed in a fixed order from TYPED values, never from a
/// `serde_json::Value`: `serde_json` resolves with `preserve_order` in this
/// workspace, so a `Value` serialises its object keys in INSERTION order and two
/// payloads that compare EQUAL serialise to different bytes. The retry a client
/// performs after a timeout must digest to the same 32 bytes, or it comes back
/// as a [`PayRunError::DigestConflict`] instead of the documented replay.
/// `None` and `Some("")` are separated by a tag byte, so an absent reason and a
/// blank one are not the same command.
fn payload_digest(command: &PayRunCommand) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(command.org_id.as_uuid().as_bytes());
    hasher.update(command.command_id.as_uuid().as_bytes());
    hasher.update(command.actor_id.as_uuid().as_bytes());
    hasher.update(command.query.target().as_str().as_bytes());
    hasher.update(command.query.run_id().as_bytes());
    let fields: Vec<Option<String>> = match &command.query {
        PayRunQuery::CreateRun {
            period_start,
            period_end,
            connector,
            job,
            ..
        } => vec![
            Some(period_start.to_string()),
            Some(period_end.to_string()),
            connector.clone(),
            job.clone(),
        ],
        PayRunQuery::SubmitRun { .. } => Vec::new(),
        PayRunQuery::DecideRun {
            decision, reason, ..
        } => vec![Some(decision.clone()), reason.clone()],
    };
    for field in fields {
        match field {
            Some(value) => {
                hasher.update([1_u8]);
                hasher.update((value.len() as u64).to_be_bytes());
                hasher.update(value.as_bytes());
            }
            None => hasher.update([0_u8]),
        }
    }
    hasher.finalize().into()
}

#[cfg(test)]
mod port_error_kind_tests {
    use super::*;
    use crate::lifecycle::{ClosePreflight, LifecycleError};
    use console_kernel_core::ErrorKind;
    use console_ontology_canonical_domain::CanonicalPortError;
    use console_platform_db::DbError;

    fn assert_lifecycle_kind(err: LifecycleError, expected: ErrorKind) {
        let kind = PayRunError::Lifecycle(err).into_kernel_error().kind;
        assert_eq!(kind, expected);
    }

    #[test]
    fn digest_conflict_is_conflict_not_internal() {
        assert_eq!(
            PayRunError::DigestConflict(Uuid::nil())
                .into_kernel_error()
                .kind,
            ErrorKind::Conflict
        );
    }

    #[test]
    fn lifecycle_variants_preserve_kernel_kinds() {
        assert_lifecycle_kind(LifecycleError::NotFound, ErrorKind::NotFound);
        assert_lifecycle_kind(
            LifecycleError::Validation("bad input".into()),
            ErrorKind::Validation,
        );
        assert_lifecycle_kind(
            LifecycleError::InvalidTransition("draft→paid".into()),
            ErrorKind::InvalidTransition,
        );
        assert_lifecycle_kind(
            LifecycleError::InvalidState("cannot decide a run in status OPEN".into()),
            ErrorKind::Conflict,
        );
        assert_lifecycle_kind(
            LifecycleError::PreflightBlocked(ClosePreflight {
                checks: Vec::new(),
                can_close: false,
            }),
            ErrorKind::Conflict,
        );
        assert_lifecycle_kind(LifecycleError::ExceptionsOpen(3), ErrorKind::Conflict);
        assert_lifecycle_kind(LifecycleError::SodViolation, ErrorKind::Conflict);
        assert_lifecycle_kind(LifecycleError::AlreadyResolved, ErrorKind::Conflict);
        assert_lifecycle_kind(
            LifecycleError::LegalGate("missing reviewed_on".into()),
            ErrorKind::Conflict,
        );
        assert_lifecycle_kind(
            LifecycleError::Db(DbError::Sqlx(sqlx::Error::RowNotFound)),
            ErrorKind::Internal,
        );
    }
}
