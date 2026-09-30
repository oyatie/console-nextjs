//! `EmploymentPort` — the Postgres implementation of `ObjectKey::Employment`,
//! and the home of every `employees` statement that used to live outside the
//! owning crate. Org-change `ReassignOrgUnit` is PORT-ROUTED: `apply_op` calls
//! [`reassign_org_unit_via_transfers_in_tx`], which emits one `hr.transfer`
//! per matched ACTIVE employee inside the apply transaction (no raw bulk
//! rewrite as authority; EXITED heads are excluded; unknown from/to OrgUnits
//! fail closed).
//!
//! Owned tables, verbatim from the contract
//! (`backend/crates/ontology/canonical-domain/src/lib.rs`): `employees`,
//! `employment_heads`, `employment_revisions`, `employment_source_bindings`.
//! `employees` is the LEGACY COMPATIBILITY HEAD — 0063 created it, the rest of
//! the tree still reads it, and `employment_source_bindings` is the surface
//! that resolves a canonical employment back to it.
//!
//! # Why the two legacy-head writes are public
//!
//! Before this module, `console-app` held three `employees` statements in
//! `backend/app/src/hr.rs` and `console-gate-writer-ownership` recorded that as
//! a ratcheted dual writer. The REST handlers own the surrounding transaction —
//! the employee row, its employment profile, its lifecycle event, the audit
//! chain and the idempotency reservation all commit or roll back together — so
//! moving those writes behind a port that opens its own connection would have
//! traded a writer-ownership violation for an atomicity one.
//!
//! [`insert_employee_record`] and [`apply_employment_change`] therefore take the
//! CALLER's transaction and hold the SQL here, in the owning crate. They are not
//! wrappers around the port: they are the port's own legacy-head writes, called
//! by [`PgEmploymentPort::execute`] for `hr.promote` and `hr.transfer` and by
//! the REST handlers for the same change arriving over HTTP. One statement, one
//! crate, two callers.
//!
//! Three statements collapsed to two: the confirmed-exit write was
//! `SET employment_status = 'EXITED', exit_date = $3` and the lifecycle write
//! is that plus the company/org-unit/position triple, whose `CASE WHEN $6 =
//! 'EXITED'` arm sets the same `exit_date`. Passing the employee's current
//! company/org-unit/position through the lifecycle statement produces the
//! identical row, so the exit path is a call to the general one. The non-EXITED
//! arm clears `exit_date`: reactivation (EXITED → ACTIVE|UNKNOWN) must not leave
//! a termination date on a non-exited status (console-90h).
//!
//! # Append-only, enforced by the database
//!
//! 0214's `canonical_employment_row_immutable` trigger refuses UPDATE and DELETE
//! on `employment_revisions` and UPDATE on `employment_source_bindings`, so a
//! revision is never edited: `Promote`/`Transfer` append `MAX(version) + 1`.
//! `employment_heads` carries a narrow trigger — `valid_to` is the one
//! legitimate mutation. When a Promote/Transfer revision carries
//! `employment_status = EXITED`, this port sets `employment_heads.valid_to` to
//! that revision's `valid_from` in the same transaction that appends the
//! revision and updates the legacy head. There is no separate `hr.exit`
//! dispatch target: exit is expressed as Promote/Transfer with EXITED.
//!
//! A revision stores only `valid_from`; its half-open interval ends at the next
//! revision's `valid_from`, or at the head's `valid_to`. That is 0214's temporal
//! contract, and it is why there is no `valid_to` on the revision to write.
//!
//! # Where the receipt is stored
//!
//! In `ont_action_command_receipts`, the generalised 0177 store, exactly as
//! `PersonPort` does: `PRIMARY KEY (org_id, command_id)` is what makes a command
//! id tenant-global ACROSS owners, so one client idempotency key can never mean
//! two accepted commands. 0177 carries no `owner`/`target` column yet, so the
//! `DispatchTarget` travels inside the receipt JSONB as its wire string and
//! read-back parses it with `FromStr`. A stored row that names no target is
//! refused, never replayed.
//!
//! # Synchronous port, async driver
//!
//! `CanonicalPort::execute` is synchronous and `sqlx` is async-only, so
//! [`PgEmploymentPort`] holds a `tokio::runtime::Handle` and blocks on it.
//! `Handle::block_on` panics when called from a runtime worker thread; an async
//! caller must reach `execute` through `spawn_blocking`.
//! ponytail: one runtime handle, no thread pool of its own — revisit only if the
//! trait ever gains an async form.

use console_kernel_core::{KernelError, OrgId, UserId};
use console_ontology_canonical_domain::{
    CanonicalPort, CanonicalPortError, CanonicalQuery, CommandId, CommandReceipt, DispatchTarget,
    Employment, ObjectKey, Preflight, ReceiptOwner,
};
use console_platform_db::{PeriodLockDomain, assert_period_open, lock_period_lock_key};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::str::FromStr;
use time::{Date, OffsetDateTime, macros::offset};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// The legacy compatibility head
// ---------------------------------------------------------------------------

/// The `employees` row an appointment creates.
///
/// `home_branch_id` is deliberately absent: since 0166 the branch routing
/// authority is command-only and the `console_rt` guard rejects it here, so the
/// caller establishes it post-commit.
#[derive(Debug, Clone, Copy)]
pub struct NewEmployeeRecord<'a> {
    pub employee_id: Uuid,
    pub company: &'a str,
    pub name: &'a str,
    pub employee_number: &'a str,
    pub org_unit: &'a str,
    pub position: &'a str,
    pub worksite_name: &'a str,
}

/// One employment change applied to the legacy head: the company/org-unit/
/// position triple plus the status. `effective_date` becomes `exit_date` when
/// the status is `EXITED`; any other admitted status clears `exit_date` so a
/// reactivation cannot leave status and exit date disagreeing.
#[derive(Debug, Clone, Copy)]
pub struct EmploymentChange<'a> {
    pub company: &'a str,
    pub org_unit: Option<&'a str>,
    pub position: Option<&'a str>,
    pub employment_status: &'a str,
    pub effective_date: &'a str,
}

/// Creates the legacy `employees` row for a new appointment, inside the
/// caller's transaction.
///
/// # Errors
/// Returns the driver error verbatim so the caller can map `23505` (employee
/// number already in use) to its own conflict.
pub async fn insert_employee_record(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    record: NewEmployeeRecord<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO employees (
            id, org_id, company, name, employee_number, org_unit, position,
            worksite_name, source_filename, source_sheet,
            source_row, source_key, raw_row, source_metadata,
            identity_resolution_strategy, identity_resolution_confidence,
            identity_review_required, identity_name_only_merge
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, 'console', 'people', 1,
            $9, '{}'::jsonb, '{}'::jsonb, 'employee_number', 'high', FALSE, FALSE
        )"#,
    )
    .bind(record.employee_id)
    .bind(org_id)
    .bind(record.company)
    .bind(record.name)
    .bind(record.employee_number)
    .bind(record.org_unit)
    .bind(record.position)
    .bind(record.worksite_name)
    .bind(format!("console:{}", record.employee_number))
    .execute(tx.as_mut())
    .await
    .map(|_| ())
}

/// Applies one employment change to the legacy `employees` head, inside the
/// caller's transaction.
///
/// §3.9.1 변경 동결 창: before the UPDATE, the effective date is checked against
/// the platform period locks (Payroll + Accounting). A change whose effective
/// date falls inside a closed period is refused and the caller's transaction
/// rolls back whole. This statement is the LIVE-head rewrite, so the port calls
/// it only for non-backdated Promote/Transfer; the REST lifecycle handlers call
/// it directly, which is why the gate lives here rather than only in the port.
///
/// # Errors
/// `Frozen` when a period lock closes the effective date (or the effective date
/// is not an ISO date, mapped to a validation refusal); `Database` verbatim
/// from the driver.
pub async fn apply_employment_change(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    employee_id: Uuid,
    change: EmploymentChange<'_>,
) -> Result<(), EmploymentError> {
    let effective_date = Date::parse(
        change.effective_date,
        &time::format_description::well_known::Iso8601::DATE,
    )
    .map_err(|error| {
        EmploymentError::Frozen(KernelError::validation(format!(
            "employment change effective_date {:?} is not an ISO date: {error}",
            change.effective_date
        )))
    })?;
    assert_employment_change_window_open(tx, org_id, effective_date).await?;
    sqlx::query(
        r#"
        UPDATE employees
        SET
            company = $3,
            org_unit = $4,
            position = $5,
            employment_status = $6,
            exit_date = CASE WHEN $6 = 'EXITED' THEN $7 ELSE NULL END,
            updated_at = now()
        WHERE org_id = $1 AND id = $2
        "#,
    )
    .bind(org_id)
    .bind(employee_id)
    .bind(change.company)
    .bind(change.org_unit)
    .bind(change.position)
    .bind(change.employment_status)
    .bind(change.effective_date)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

/// The freeze domains §3.9.1 names — 급여 마감 and 회계 결산. One list, mirroring
/// the org-change adapter's `FREEZE_DOMAINS`, so an employment change and an org
/// change can never freeze on a different set of windows.
const EMPLOYMENT_FREEZE_DOMAINS: [PeriodLockDomain; 2] =
    [PeriodLockDomain::Payroll, PeriodLockDomain::Accounting];

/// The tenant business date (KST, UTC+9) of an instant, mirroring the org-change
/// adapter's `today_kst` offset. The lock date is derived from this fixed offset,
/// never from the caller-supplied RFC3339 offset, so the same instant cannot be
/// submitted as two different calendar days and bypass a closed window.
fn business_date(instant: OffsetDateTime) -> Date {
    instant.to_offset(offset!(+9)).date()
}

/// §3.9.1 변경 동결 창 — refuse an employment mutation whose effective date falls
/// inside a closed payroll or accounting period. Runs in the caller's already-
/// armed transaction, so the lookup is RLS-scoped to the caller's tenant and the
/// refusal rolls the whole write back.
///
/// Before the read it takes the same per-tenant, per-domain advisory key the
/// period-lock CREATE path takes ([`console_platform_db::lock_period_lock_key`]),
/// so a lock committed concurrently cannot slip between this read and the
/// write's commit: the two order strictly and the writer either sees the
/// committed lock (refused) or commits before the lock exists (a draft that
/// predates the freeze).
async fn assert_employment_change_window_open(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    effective_date: Date,
) -> Result<(), EmploymentError> {
    for domain in EMPLOYMENT_FREEZE_DOMAINS {
        lock_period_lock_key(tx, domain, org_id).await?;
        assert_period_open(tx, domain, effective_date)
            .await
            .map_err(EmploymentError::Frozen)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The canonical port
// ---------------------------------------------------------------------------

/// The canonical state of one employment at one version. Typed rather than raw
/// JSONB because the port applies the same four values to the legacy head, and
/// `employees.employment_status` carries a CHECK that a free-form payload would
/// only discover at 23514.
///
/// `org_unit_id` / `job_position_id` are canonical OrgUnit / JobPosition UUIDs
/// (L5-ORG / L5-JOB). Free-text team or title labels are not authority. The
/// legacy `employees.org_unit` / `employees.position` TEXT columns store the
/// UUID string form so REST/org-chart readers keep a single column without a
/// parallel free-text namespace.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EmploymentAttributes {
    pub company: String,
    pub org_unit_id: Option<Uuid>,
    pub job_position_id: Option<Uuid>,
    pub employment_status: String,
}

/// The three statuses `employees.employment_status` admits (0066's CHECK).
const EMPLOYMENT_STATUSES: [&str; 3] = ["ACTIVE", "EXITED", "UNKNOWN"];

impl EmploymentAttributes {
    /// The revision's `attributes` JSONB. Keys are written in sorted order, so
    /// the value and its digest do not depend on this struct's field order.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "company": self.company,
            "employment_status": self.employment_status,
            "job_position_id": self.job_position_id.map(|id| id.to_string()),
            "org_unit_id": self.org_unit_id.map(|id| id.to_string()),
        })
    }

    /// Legacy-head TEXT projections of the canonical UUID attrs.
    #[must_use]
    pub fn legacy_org_unit_text(&self) -> Option<String> {
        self.org_unit_id.map(|id| id.to_string())
    }

    /// Legacy-head TEXT projections of the canonical UUID attrs.
    #[must_use]
    pub fn legacy_position_text(&self) -> Option<String> {
        self.job_position_id.map(|id| id.to_string())
    }
}

/// The typed read this port answers: the write a caller intends, and nothing
/// about how it is performed. Each variant is bound to exactly one of the three
/// dispatch targets the contract assigns to `Employment`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "target")]
pub enum EmploymentQuery {
    /// `hr.appoint`. Opens a head at `valid_from`, appends revision 1, and binds
    /// the head to the legacy `employees` row this employment IS. The legacy row
    /// is not rewritten: it was just created with these values.
    #[serde(rename = "hr.appoint")]
    Appoint {
        employee_id: Uuid,
        #[serde(with = "time::serde::rfc3339")]
        valid_from: OffsetDateTime,
        attributes: EmploymentAttributes,
    },
    /// `hr.promote`. Appends a revision to an existing head and carries the new
    /// state onto the legacy head.
    #[serde(rename = "hr.promote")]
    Promote {
        employment_id: Uuid,
        #[serde(with = "time::serde::rfc3339")]
        valid_from: OffsetDateTime,
        attributes: EmploymentAttributes,
    },
    /// `hr.transfer`. Same shape as a promotion — the difference is the
    /// dispatch target the receipt records, which is what an auditor reads.
    #[serde(rename = "hr.transfer")]
    Transfer {
        employment_id: Uuid,
        #[serde(with = "time::serde::rfc3339")]
        valid_from: OffsetDateTime,
        attributes: EmploymentAttributes,
    },
}

impl EmploymentQuery {
    /// The dispatch target this query is, spelled once in `canonical-domain`.
    #[must_use]
    pub const fn target(&self) -> DispatchTarget {
        match self {
            Self::Appoint { .. } => DispatchTarget::HrAppoint,
            Self::Promote { .. } => DispatchTarget::HrPromote,
            Self::Transfer { .. } => DispatchTarget::HrTransfer,
        }
    }

    #[must_use]
    pub const fn attributes(&self) -> &EmploymentAttributes {
        match self {
            Self::Appoint { attributes, .. }
            | Self::Promote { attributes, .. }
            | Self::Transfer { attributes, .. } => attributes,
        }
    }

    #[must_use]
    pub const fn valid_from(&self) -> OffsetDateTime {
        match self {
            Self::Appoint { valid_from, .. }
            | Self::Promote { valid_from, .. }
            | Self::Transfer { valid_from, .. } => *valid_from,
        }
    }
}

impl CanonicalQuery for EmploymentQuery {
    fn dispatch_target(&self) -> DispatchTarget {
        self.target()
    }

    /// Promote/Transfer name the employment head they revise. Without this, the
    /// projected-dispatch `target_id` bind skips (trait default `None`) and a
    /// four-eyes approval for employment I can be spent while the port revises J.
    fn subject_id(&self) -> Option<Uuid> {
        match self {
            Self::Appoint { .. } => None,
            Self::Promote { employment_id, .. } | Self::Transfer { employment_id, .. } => {
                Some(*employment_id)
            }
        }
    }
}

/// The typed write this port accepts. `org_id` is the RLS key and `command_id`
/// the tenant-global idempotency key; a repeat replays the stored receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmploymentCommand {
    pub org_id: OrgId,
    pub command_id: CommandId,
    pub actor_id: UserId,
    pub query: EmploymentQuery,
    pub action_key: String,
    pub object_type_id: Uuid,
}

#[derive(Debug, thiserror::Error)]
pub enum EmploymentError {
    #[error("preflight blocked the command: {0:?}")]
    Blocked(Vec<String>),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    /// §3.9.1 변경 동결 창: the period-lock gate refused a live employment
    /// mutation, or the gate itself could not run (unparseable effective date,
    /// lock-lookup failure). The carried `KernelError` preserves its kind so
    /// `into_kernel_error` maps a closed window to Conflict (409) and a gate
    /// failure to Internal (500).
    #[error(transparent)]
    Frozen(KernelError),
    #[error("command {0} was already applied with a different payload")]
    DigestConflict(Uuid),
    #[error("stored receipt for command {0} names no dispatch target: {1}")]
    UnreadableReceipt(Uuid, String),
    /// `employment_source_bindings` PK is `(org_id, employee_id)`, so one
    /// `employment_id` can appear on N rows. Reads and revisions must not pick.
    #[error(
        "employment {employment_id} has {binding_count} source bindings; refuse ambiguous promote/transfer"
    )]
    AmbiguousSourceBinding {
        employment_id: Uuid,
        binding_count: usize,
    },
    #[error("employment {employment_id} has unreadable effective state: {reason}")]
    UnreadableEffectiveState { employment_id: Uuid, reason: String },
    #[error("employment {0} has an ACTIVE revision after its stored close; correction required")]
    UnresolvedReactivation(Uuid),
    #[error("org_unit_id {0} is not a known OrgUnit in this tenant")]
    UnknownOrgUnit(Uuid),
    #[error("job_position_id {0} is not a known JobPosition in this tenant")]
    UnknownJobPosition(Uuid),
    #[error(
        "employee {employee_id} matched ReassignOrgUnit but has no employment_source_bindings row; refuse unbound transfer"
    )]
    UnboundEmployeeForTransfer { employee_id: Uuid },
    #[error("fromOrgUnit/toOrgUnit must be OrgUnit UUIDs, not free-text team labels")]
    OrgUnitRefNotUuid,
}

impl CanonicalPortError for EmploymentError {
    fn into_kernel_error(self) -> KernelError {
        let message = self.to_string();
        match self {
            Self::Blocked(_)
            | Self::AmbiguousSourceBinding { .. }
            | Self::UnknownOrgUnit(_)
            | Self::UnknownJobPosition(_)
            | Self::UnboundEmployeeForTransfer { .. }
            | Self::OrgUnitRefNotUuid => KernelError::validation(message),
            Self::DigestConflict(_) | Self::UnresolvedReactivation(_) => {
                KernelError::conflict(message)
            }
            Self::Frozen(error) => error,
            Self::Database(_)
            | Self::UnreadableReceipt(_, _)
            | Self::UnreadableEffectiveState { .. } => KernelError::internal(message),
        }
    }
}

/// Canonical Employment state for an open head, or for an effective instant
/// inside a head's lifetime when read through `get_as_of`.
///
/// Not-yet-open and closed windows are omitted by list/get; `get_as_of` can return
/// their earlier state before `valid_to`. `org_unit_id` / `job_position_id`
/// come from the latest *effective* revision at the requested instant
/// (`MAX(valid_from)` — a backdated history insert is a later version with an
/// earlier effect). `person_id` is the unique current
/// source-binding → person-binding path; ambiguous or unbound identities are
/// omitted, never invented from `employee_id`. `appointed_on` is the head's
/// opening `valid_from`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmploymentHead {
    pub id: Uuid,
    pub person_id: Option<Uuid>,
    pub org_unit_id: Option<Uuid>,
    pub job_position_id: Option<Uuid>,
    pub appointed_on: OffsetDateTime,
}

/// Effective revision state, including bound employments not yet effective.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmploymentEffectiveStatus {
    NotYetEffective,
    Active,
    Exited,
    Unknown,
}

/// Non-serving owner projection at one captured instant. Attributes are absent
/// before the first effective revision; only `Active` is active. Unlike
/// `EmploymentHead`, this projection retains exited and unknown states even
/// when the head's lifetime has closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmploymentEffectiveProjection {
    pub employment_id: Uuid,
    pub captured_as_of: OffsetDateTime,
    pub state: EmploymentEffectiveStatus,
    pub is_active: bool,
    pub company: Option<String>,
    pub org_unit_id: Option<Uuid>,
    pub job_position_id: Option<Uuid>,
}

fn parse_effective_attributes(
    employment_id: Uuid,
    value: serde_json::Value,
) -> Result<EmploymentAttributes, EmploymentError> {
    let attributes: EmploymentAttributes = serde_json::from_value(value).map_err(|error| {
        EmploymentError::UnreadableEffectiveState {
            employment_id,
            reason: error.to_string(),
        }
    })?;
    if attributes.company.trim().is_empty()
        || !EMPLOYMENT_STATUSES.contains(&attributes.employment_status.as_str())
        || attributes.org_unit_id == Some(Uuid::nil())
        || attributes.job_position_id == Some(Uuid::nil())
    {
        return Err(EmploymentError::UnreadableEffectiveState {
            employment_id,
            reason: "invalid company, status, or reference".to_owned(),
        });
    }
    Ok(attributes)
}

/// The one permitted holder of production DML against `employees`,
/// `employment_heads`, `employment_revisions` and
/// `employment_source_bindings`.
#[derive(Debug, Clone)]
pub struct PgEmploymentPort {
    pool: PgPool,
    runtime: tokio::runtime::Handle,
}

impl PgEmploymentPort {
    #[must_use]
    pub const fn new(pool: PgPool, runtime: tokio::runtime::Handle) -> Self {
        Self { pool, runtime }
    }

    /// Head effective at the database clock. Not-yet-open or already closed
    /// windows and foreign tenant ids are `None` — never a fabricated row.
    pub fn get(
        &self,
        org_id: OrgId,
        employment_id: Uuid,
    ) -> Result<Option<EmploymentHead>, EmploymentError> {
        self.runtime
            .block_on(self.read_heads(*org_id.as_uuid(), Some(employment_id), None))
            .map(|heads| heads.into_iter().next())
    }

    /// Employment state effective at `at`, using revisions known now. The
    /// opening and exit instants are half-open; a later backdated correction
    /// changes this answer without rewriting the legacy current head.
    pub fn get_as_of(
        &self,
        org_id: OrgId,
        employment_id: Uuid,
        at: OffsetDateTime,
    ) -> Result<Option<EmploymentHead>, EmploymentError> {
        self.runtime
            .block_on(self.read_heads(*org_id.as_uuid(), Some(employment_id), Some(at)))
            .map(|heads| heads.into_iter().next())
    }

    /// Heads effective at one database instant in the armed tenant.
    pub fn list(&self, org_id: OrgId) -> Result<Vec<EmploymentHead>, EmploymentError> {
        self.runtime
            .block_on(self.read_heads(*org_id.as_uuid(), None, None))
    }

    /// State effective at `as_of`, using revisions known now. An unknown or
    /// foreign Company source is `None`; an ambiguous reverse binding is refused.
    pub fn read_effective_for_employee(
        &self,
        org_id: OrgId,
        employee_id: Uuid,
        as_of: OffsetDateTime,
    ) -> Result<Option<EmploymentEffectiveProjection>, EmploymentError> {
        self.runtime
            .block_on(self.read_effective(*org_id.as_uuid(), employee_id, Some(as_of)))
    }

    /// Current state with its database instant captured once inside the read
    /// transaction. Async callers must use `spawn_blocking`, as for `get`.
    pub fn read_current_effective_for_employee(
        &self,
        org_id: OrgId,
        employee_id: Uuid,
    ) -> Result<Option<EmploymentEffectiveProjection>, EmploymentError> {
        self.runtime
            .block_on(self.read_effective(*org_id.as_uuid(), employee_id, None))
    }

    async fn arm_org<'e, E>(&self, executor: E, org: Uuid) -> Result<(), EmploymentError>
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
        employment_id: Option<Uuid>,
        as_of: Option<OffsetDateTime>,
    ) -> Result<Vec<EmploymentHead>, EmploymentError> {
        let mut tx = self.pool.begin().await?;
        self.arm_org(&mut *tx, org).await?;
        let rows = sqlx::query(
            "WITH point_in_time AS MATERIALIZED ( \
               SELECT COALESCE($3::timestamptz, statement_timestamp()) AS at \
             ) \
             SELECT h.id, h.valid_from AS appointed_on, h.valid_to AS closed_at, \
                    t.at AS captured_as_of, r.valid_from AS revision_valid_from, \
                    r.attributes, \
                    bind.n AS binding_count, bind.person_id \
             FROM employment_heads h \
             CROSS JOIN point_in_time t \
             LEFT JOIN LATERAL ( \
               SELECT valid_from, attributes FROM employment_revisions \
               WHERE org_id = h.org_id AND employment_id = h.id \
                 AND valid_from <= t.at \
               ORDER BY valid_from DESC LIMIT 1 \
             ) r ON true \
             LEFT JOIN LATERAL ( \
               SELECT COUNT(*)::bigint AS n, (ARRAY_AGG(p.person_id))[1] AS person_id \
               FROM employment_source_bindings b \
               LEFT JOIN employee_person_bindings p \
                 ON p.org_id = b.org_id AND p.employee_id = b.employee_id \
               WHERE b.org_id = h.org_id AND b.employment_id = h.id \
             ) bind ON true \
             WHERE h.org_id = $1 AND ($2::uuid IS NULL OR h.id = $2) \
               AND h.valid_from <= t.at \
             ORDER BY h.id",
        )
        .bind(org)
        .bind(employment_id)
        .bind(as_of)
        .fetch_all(&mut *tx)
        .await?;
        let mut heads = Vec::new();
        for row in rows {
            let id: Uuid = row.get("id");
            let appointed_on: OffsetDateTime = row.get("appointed_on");
            let binding_count: i64 = row.get("binding_count");
            if binding_count != 1 {
                return Err(EmploymentError::AmbiguousSourceBinding {
                    employment_id: id,
                    binding_count: binding_count as usize,
                });
            }
            let revision_valid_from: Option<OffsetDateTime> = row.get("revision_valid_from");
            if revision_valid_from.is_some_and(|valid_from| valid_from < appointed_on) {
                return Err(EmploymentError::UnreadableEffectiveState {
                    employment_id: id,
                    reason: "revision predates relationship opening".to_owned(),
                });
            }
            let value: Option<serde_json::Value> = row.get("attributes");
            let value = value.ok_or_else(|| EmploymentError::UnreadableEffectiveState {
                employment_id: id,
                reason: "no effective revision after relationship opening".to_owned(),
            })?;
            let attributes = parse_effective_attributes(id, value)?;
            if attributes.employment_status != "ACTIVE" {
                continue;
            }
            let captured_as_of: OffsetDateTime = row.get("captured_as_of");
            let closed_at: Option<OffsetDateTime> = row.get("closed_at");
            if closed_at.is_some_and(|closed_at| closed_at <= captured_as_of) {
                return Err(EmploymentError::UnresolvedReactivation(id));
            }
            heads.push(EmploymentHead {
                id,
                person_id: row.get("person_id"),
                org_unit_id: attributes.org_unit_id,
                job_position_id: attributes.job_position_id,
                appointed_on,
            });
        }
        tx.commit().await?;
        Ok(heads)
    }

    async fn read_effective(
        &self,
        org: Uuid,
        employee_id: Uuid,
        as_of: Option<OffsetDateTime>,
    ) -> Result<Option<EmploymentEffectiveProjection>, EmploymentError> {
        let mut tx = self.pool.begin().await?;
        self.arm_org(&mut *tx, org).await?;
        // The captured instant, binding, head and revision share one statement
        // snapshot. Do not filter on valid_to: EXITED remains a readable state.
        let row = sqlx::query(
            "WITH point_in_time AS MATERIALIZED ( \
               SELECT COALESCE($3::timestamptz, statement_timestamp()) AS at \
             ) \
             SELECT b.employment_id, h.valid_from AS opened_at, h.valid_to AS closed_at, \
                    t.at AS captured_as_of, r.valid_from AS revision_valid_from, \
                    r.attributes, \
                    (SELECT COUNT(*) FROM employment_source_bindings reverse_bind \
                     WHERE reverse_bind.org_id = b.org_id \
                       AND reverse_bind.employment_id = b.employment_id) AS binding_count \
             FROM employment_source_bindings b \
             JOIN employment_heads h \
               ON h.org_id = b.org_id AND h.id = b.employment_id \
             CROSS JOIN point_in_time t \
             LEFT JOIN LATERAL ( \
               SELECT valid_from, attributes FROM employment_revisions \
               WHERE org_id = b.org_id AND employment_id = b.employment_id \
                 AND valid_from <= t.at \
               ORDER BY valid_from DESC LIMIT 1 \
             ) r ON true \
             WHERE b.org_id = $1 AND b.employee_id = $2",
        )
        .bind(org)
        .bind(employee_id)
        .bind(as_of)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let employment_id = row.get("employment_id");
        let binding_count: i64 = row.get("binding_count");
        if binding_count != 1 {
            return Err(EmploymentError::AmbiguousSourceBinding {
                employment_id,
                binding_count: binding_count as usize,
            });
        }
        let captured_as_of: OffsetDateTime = row.get("captured_as_of");
        let opened_at: OffsetDateTime = row.get("opened_at");
        let closed_at: Option<OffsetDateTime> = row.get("closed_at");
        let revision_valid_from: Option<OffsetDateTime> = row.get("revision_valid_from");
        if revision_valid_from.is_some_and(|valid_from| valid_from < opened_at) {
            return Err(EmploymentError::UnreadableEffectiveState {
                employment_id,
                reason: "revision predates relationship opening".to_owned(),
            });
        }
        let attributes: Option<serde_json::Value> = row.get("attributes");
        let attributes = if captured_as_of < opened_at {
            if attributes.is_some() {
                return Err(EmploymentError::UnreadableEffectiveState {
                    employment_id,
                    reason: "revision predates relationship opening".to_owned(),
                });
            }
            None
        } else {
            let value = attributes.ok_or_else(|| EmploymentError::UnreadableEffectiveState {
                employment_id,
                reason: "no effective revision after relationship opening".to_owned(),
            })?;
            Some(parse_effective_attributes(employment_id, value)?)
        };
        let state = match attributes
            .as_ref()
            .map(|attributes| attributes.employment_status.as_str())
        {
            None => EmploymentEffectiveStatus::NotYetEffective,
            Some("ACTIVE") => EmploymentEffectiveStatus::Active,
            Some("EXITED") => EmploymentEffectiveStatus::Exited,
            Some("UNKNOWN") => EmploymentEffectiveStatus::Unknown,
            Some(_) => {
                return Err(EmploymentError::UnreadableEffectiveState {
                    employment_id,
                    reason: "invalid employment status".to_owned(),
                });
            }
        };
        if state == EmploymentEffectiveStatus::Active
            && closed_at.is_some_and(|closed_at| closed_at <= captured_as_of)
        {
            return Err(EmploymentError::UnresolvedReactivation(employment_id));
        }
        let projection = EmploymentEffectiveProjection {
            employment_id,
            captured_as_of,
            state,
            is_active: state == EmploymentEffectiveStatus::Active,
            company: attributes.as_ref().map(|attrs| attrs.company.clone()),
            org_unit_id: attributes.as_ref().and_then(|attrs| attrs.org_unit_id),
            job_position_id: attributes.as_ref().and_then(|attrs| attrs.job_position_id),
        };
        tx.commit().await?;
        Ok(Some(projection))
    }

    async fn write(&self, command: &EmploymentCommand) -> Result<CommandReceipt, EmploymentError> {
        let mut tx = self.pool.begin().await?;
        // Transaction-local, so it is cleared on COMMIT/ROLLBACK and never
        // leaks to the next checkout of a pooled connection. Unset fails
        // closed: RLS shows no rows and accepts no writes.
        sqlx::query("SELECT set_config('app.current_org', $1, true)")
            .bind(command.org_id.as_uuid().to_string())
            .execute(&mut *tx)
            .await?;
        let receipt = write_in_tx(&mut tx, command).await?;
        tx.commit().await?;
        Ok(receipt)
    }
}

/// Apply one Employment command inside a caller-owned, org-armed transaction.
///
/// Org-change `ReassignOrgUnit` uses this so each transfer shares the apply
/// transaction rather than opening a nested connection. Callers must already
/// have set `app.current_org`.
pub async fn write_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    command: &EmploymentCommand,
) -> Result<CommandReceipt, EmploymentError> {
    let preflight = <PgEmploymentPort as CanonicalPort>::preflight(&command.query);
    if !preflight.is_ok() {
        return Err(EmploymentError::Blocked(preflight.blockers().to_vec()));
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
            return Err(EmploymentError::DigestConflict(command_uuid));
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

    ensure_attribute_refs(tx, org, command.query.attributes()).await?;

    let target = command.query.target();
    let valid_from = command.query.valid_from();
    let attributes = command.query.attributes();

    // §3.9.1: every dated mutation is checked, not just the legacy-head rewrite.
    // This covers Appoint and backdated Promote/Transfer too, so a command dated
    // inside a closed Payroll/Accounting window is refused before any insert.
    assert_employment_change_window_open(tx, org, business_date(valid_from)).await?;

    let (employment_id, version, backdated) = match &command.query {
        EmploymentQuery::Appoint { .. } => {
            let employment_id: Uuid = sqlx::query_scalar(
                "INSERT INTO employment_heads (org_id, valid_from) VALUES ($1, $2) \
                 RETURNING id",
            )
            .bind(org)
            .bind(valid_from)
            .fetch_one(tx.as_mut())
            .await?;
            (employment_id, 1_i64, false)
        }
        EmploymentQuery::Promote { employment_id, .. }
        | EmploymentQuery::Transfer { employment_id, .. } => {
            // ponytail: MAX + 1 under the row's own transaction. A
            // concurrent revise of the same employment loses to
            // UNIQUE (org_id, employment_id, version) with 23505 rather
            // than silently overwriting; add SELECT ... FOR UPDATE on
            // `employment_heads` if that contention is ever measured.
            let next: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(version), 0) + 1 FROM employment_revisions \
                 WHERE org_id = $1 AND employment_id = $2",
            )
            .bind(org)
            .bind(employment_id)
            .fetch_one(tx.as_mut())
            .await?;
            // A revise before the head's opening bound is outside the
            // employment's half-open lifetime and must be refused, not appended
            // as history. `employment_heads.valid_from` is that opening bound.
            let opened: OffsetDateTime = sqlx::query_scalar(
                "SELECT valid_from FROM employment_heads \
                 WHERE org_id = $1 AND id = $2",
            )
            .bind(org)
            .bind(employment_id)
            .fetch_one(tx.as_mut())
            .await?;
            if valid_from < opened {
                return Err(EmploymentError::Blocked(vec![
                    "revision valid_from predates the employment head opening".to_owned(),
                ]));
            }
            // A revision backdated before the current head is a legitimate
            // history insert (0214 is append-only), but the legacy `employees`
            // head is a projection of the LATEST effective state, so the live
            // rewrite below must be skipped for it. `valid_from` was read
            // before this command's revision is appended, so MAX(valid_from)
            // still names the previous head.
            let latest: Option<OffsetDateTime> = sqlx::query_scalar(
                "SELECT MAX(valid_from) FROM employment_revisions \
                 WHERE org_id = $1 AND employment_id = $2",
            )
            .bind(org)
            .bind(employment_id)
            .fetch_one(tx.as_mut())
            .await?;
            (
                *employment_id,
                next,
                latest.is_some_and(|latest| valid_from < latest),
            )
        }
    };

    let result = serde_json::json!({
        "employment_id": employment_id.to_string(),
        "version": version,
        "target": target.as_str(),
    });

    let created_at: OffsetDateTime = sqlx::query_scalar(
        "INSERT INTO employment_revisions \
         (org_id, employment_id, version, command_id, actor_id, payload_digest, \
          valid_from, attributes, receipt) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING created_at",
    )
    .bind(org)
    .bind(employment_id)
    .bind(version)
    .bind(command_uuid)
    .bind(actor)
    .bind(digest.as_slice())
    .bind(valid_from)
    .bind(attributes.to_json())
    .bind(&result)
    .fetch_one(tx.as_mut())
    .await?;

    match &command.query {
        EmploymentQuery::Appoint { employee_id, .. } => {
            sqlx::query(
                "INSERT INTO employment_source_bindings \
                 (org_id, employee_id, employment_id, actor_id, payload_digest) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(org)
            .bind(employee_id)
            .bind(employment_id)
            .bind(actor)
            .bind(digest.as_slice())
            .execute(tx.as_mut())
            .await?;
        }
        // The legacy compatibility head carries the new state, through the
        // same statement the REST lifecycle handler calls. `valid_from` is
        // the effective date: EXITED stamps `exit_date` from it; ACTIVE|
        // UNKNOWN clears `exit_date` so reactivation cannot disagree with
        // status. The canonical head closes in the same transaction when
        // EXITED: `valid_to` = this revision's `valid_from`.
        //
        // A backdated revision is appended to history above, but must NOT move
        // the live legacy head (or the head's close date) backward: both mirror
        // the LATEST effective state, not the most recently appended one. The
        // receipt store below still records the accepted history insert, so a
        // retry of the same command id replays rather than re-appending.
        EmploymentQuery::Promote { .. } | EmploymentQuery::Transfer { .. } => {
            // The source binding must resolve for every revise — backdated or
            // not — so a deleted or ambiguous binding cannot append a revision
            // (or store a receipt) that the legacy head could never show.
            let employee_id = bound_employee(tx, org, employment_id).await?;
            if !backdated {
                // The legacy head mirrors the LATEST effective state. Its date
                // string is the KST business date, the same instant the top-of-
                // write gate froze on, so the two never disagree about which
                // calendar day is sealed.
                let effective_date = business_date(valid_from).to_string();
                let org_unit_text = attributes.legacy_org_unit_text();
                let position_text = attributes.legacy_position_text();
                apply_employment_change(
                    tx,
                    org,
                    employee_id,
                    EmploymentChange {
                        company: &attributes.company,
                        org_unit: org_unit_text.as_deref(),
                        position: position_text.as_deref(),
                        employment_status: &attributes.employment_status,
                        effective_date: &effective_date,
                    },
                )
                .await?;
                if attributes.employment_status == "EXITED" {
                    sqlx::query(
                        "UPDATE employment_heads SET valid_to = $3 \
                         WHERE org_id = $1 AND id = $2",
                    )
                    .bind(org)
                    .bind(employment_id)
                    .bind(valid_from)
                    .execute(tx.as_mut())
                    .await?;
                }
            }
        }
    }

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

/// Org-change `ReassignOrgUnit` → one `hr.transfer` per matched **ACTIVE** employee.
///
/// `from_org_unit` / `to_org_unit` must be OrgUnit UUID strings (fail closed on
/// free-text team labels). Both source and destination OrgUnits must exist —
/// a syntactically valid but unknown `from` must not audit as `moved=0` success.
/// Only `employment_status = 'ACTIVE'` rows are selected, matching preflight
/// `scope_headcount` (ACTIVE-only) so EXITED heads are not rewritten via
/// Transfer. Employees without an `employment_source_bindings` row are refused
/// — a bulk rewrite of unbound heads is not a transfer.
#[allow(clippy::too_many_arguments)] // tx + org/actor/op ids + from/to/company/valid_from mirror the apply-op surface
pub async fn reassign_org_unit_via_transfers_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    org: OrgId,
    actor: UserId,
    op_command_id: Uuid,
    from_org_unit: &str,
    to_org_unit: &str,
    company: &str,
    valid_from: OffsetDateTime,
) -> Result<u64, EmploymentError> {
    let from_id =
        Uuid::parse_str(from_org_unit.trim()).map_err(|_| EmploymentError::OrgUnitRefNotUuid)?;
    let to_id =
        Uuid::parse_str(to_org_unit.trim()).map_err(|_| EmploymentError::OrgUnitRefNotUuid)?;
    if from_id == to_id {
        return Err(EmploymentError::Blocked(vec![
            "REASSIGN_ORG_UNIT source and target must differ".to_owned(),
        ]));
    }

    // Fail closed on both ends before SELECT — unknown source must not return Ok(0).
    ensure_org_unit_exists(tx, *org.as_uuid(), from_id).await?;
    ensure_org_unit_exists(tx, *org.as_uuid(), to_id).await?;

    let from_text = from_id.to_string();
    let rows = sqlx::query(
        "SELECT id, position, employment_status FROM employees \
         WHERE org_id = $1 AND company = $2 AND org_unit = $3 \
           AND employment_status = 'ACTIVE'",
    )
    .bind(org.as_uuid())
    .bind(company)
    .bind(&from_text)
    .fetch_all(tx.as_mut())
    .await?;

    let mut moved = 0_u64;
    for row in rows {
        let employee_id: Uuid = row.get("id");
        let employment_status: String = row.get("employment_status");
        let position_text: Option<String> = row.get("position");
        let job_position_id = position_text
            .as_deref()
            .and_then(|value| Uuid::parse_str(value.trim()).ok());

        let employment_id = employment_id_for_employee(tx, *org.as_uuid(), employee_id).await?;
        // Per-employee command id: tenant-global uniqueness + deterministic for
        // the same (apply-op command, employee) pair. v5 is not enabled on the
        // workspace uuid crate; SHA-256 truncated to 16 bytes is enough here.
        let mut hasher = Sha256::new();
        hasher.update(op_command_id.as_bytes());
        hasher.update(employee_id.as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        let command_id = Uuid::from_bytes(bytes);
        let command = EmploymentCommand {
            org_id: org,
            command_id: CommandId::from_uuid(command_id),
            actor_id: actor,
            query: EmploymentQuery::Transfer {
                employment_id,
                valid_from,
                attributes: EmploymentAttributes {
                    company: company.to_owned(),
                    org_unit_id: Some(to_id),
                    job_position_id,
                    employment_status,
                },
            },
            action_key: "internal.reassign_org_unit".to_owned(),
            object_type_id: Uuid::nil(),
        };
        write_in_tx(tx, &command).await?;
        moved += 1;
    }
    Ok(moved)
}

async fn employment_id_for_employee(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    employee_id: Uuid,
) -> Result<Uuid, EmploymentError> {
    let rows: Vec<Uuid> = sqlx::query_scalar(
        "SELECT employment_id FROM employment_source_bindings \
         WHERE org_id = $1 AND employee_id = $2",
    )
    .bind(org_id)
    .bind(employee_id)
    .fetch_all(tx.as_mut())
    .await?;
    match rows.as_slice() {
        [] => Err(EmploymentError::UnboundEmployeeForTransfer { employee_id }),
        [employment_id] => Ok(*employment_id),
        _ => Err(EmploymentError::AmbiguousSourceBinding {
            employment_id: rows[0],
            binding_count: rows.len(),
        }),
    }
}

async fn ensure_attribute_refs(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    attributes: &EmploymentAttributes,
) -> Result<(), EmploymentError> {
    if let Some(org_unit_id) = attributes.org_unit_id {
        ensure_org_unit_exists(tx, org_id, org_unit_id).await?;
    }
    if let Some(job_position_id) = attributes.job_position_id {
        ensure_job_position_exists(tx, org_id, job_position_id).await?;
    }
    Ok(())
}

pub async fn ensure_org_unit_exists(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    org_unit_id: Uuid,
) -> Result<(), EmploymentError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM org_units WHERE org_id = $1 AND id = $2)")
            .bind(org_id)
            .bind(org_unit_id)
            .fetch_one(tx.as_mut())
            .await?;
    if exists {
        Ok(())
    } else {
        Err(EmploymentError::UnknownOrgUnit(org_unit_id))
    }
}

pub async fn ensure_job_position_exists(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    job_position_id: Uuid,
) -> Result<(), EmploymentError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM job_positions WHERE org_id = $1 AND id = $2)",
    )
    .bind(org_id)
    .bind(job_position_id)
    .fetch_one(tx.as_mut())
    .await?;
    if exists {
        Ok(())
    } else {
        Err(EmploymentError::UnknownJobPosition(job_position_id))
    }
}

/// The legacy `employees` row this employment IS, read through the binding.
///
/// `RowNotFound` when nothing is bound is deliberate: a promotion whose legacy
/// head cannot be found must fail the whole command rather than append a
/// canonical revision the rest of the tree cannot see. Multiple bindings for
/// the same `employment_id` are refused — `fetch_one` would silently pick.
async fn bound_employee(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    employment_id: Uuid,
) -> Result<Uuid, EmploymentError> {
    let rows: Vec<Uuid> = sqlx::query_scalar(
        "SELECT employee_id FROM employment_source_bindings \
         WHERE org_id = $1 AND employment_id = $2",
    )
    .bind(org_id)
    .bind(employment_id)
    .fetch_all(tx.as_mut())
    .await?;
    match rows.as_slice() {
        [] => Err(EmploymentError::Database(sqlx::Error::RowNotFound)),
        [employee_id] => Ok(*employee_id),
        _ => Err(EmploymentError::AmbiguousSourceBinding {
            employment_id,
            binding_count: rows.len(),
        }),
    }
}

impl CanonicalPort for PgEmploymentPort {
    type Object = Employment;
    type Query = EmploymentQuery;
    type Command = EmploymentCommand;
    type Error = EmploymentError;

    /// PURE: no `&self`, no IO, no persistence. A blocked preflight has written
    /// nothing, so it can never spend an approval.
    fn preflight(query: &Self::Query) -> Preflight {
        let mut blockers = Vec::new();
        let attributes = query.attributes();
        if attributes.company.trim().is_empty() {
            blockers.push("company must not be blank".to_owned());
        }
        if !EMPLOYMENT_STATUSES.contains(&attributes.employment_status.as_str()) {
            blockers.push(format!(
                "employment_status must be one of {EMPLOYMENT_STATUSES:?}"
            ));
        }
        if attributes.org_unit_id == Some(Uuid::nil()) {
            blockers.push("org_unit_id must not be nil".to_owned());
        }
        if attributes.job_position_id == Some(Uuid::nil()) {
            blockers.push("job_position_id must not be nil".to_owned());
        }
        match query {
            EmploymentQuery::Appoint { employee_id, .. } if employee_id.is_nil() => {
                blockers.push("employee_id must not be nil".to_owned());
            }
            EmploymentQuery::Promote { employment_id, .. }
            | EmploymentQuery::Transfer { employment_id, .. }
                if employment_id.is_nil() =>
            {
                blockers.push("employment_id must not be nil".to_owned());
            }
            _ => {}
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
        EmploymentCommand {
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
    command: &EmploymentCommand,
    target: DispatchTarget,
    actor_id: UserId,
    digest: [u8; 32],
    result: serde_json::Value,
    created_at: OffsetDateTime,
) -> CommandReceipt {
    CommandReceipt::new(
        command.org_id,
        command.command_id,
        ReceiptOwner::Canonical(ObjectKey::Employment),
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
) -> Result<DispatchTarget, EmploymentError> {
    let stored = result["target"]
        .as_str()
        .ok_or_else(|| EmploymentError::UnreadableReceipt(command_id, result.to_string()))?;
    DispatchTarget::from_str(stored)
        .map_err(|error| EmploymentError::UnreadableReceipt(command_id, error.to_string()))
}

/// The 32 bytes the `payload_digest` CHECK is sized for.
///
/// Every field is hashed in a fixed order from TYPED values, never from a
/// `serde_json::Value`: `serde_json` resolves with `preserve_order` in this
/// workspace, so a `Value` serialises its object keys in INSERTION order and two
/// payloads that compare EQUAL serialise to different bytes. The retry a client
/// performs after a timeout must digest to the same 32 bytes, or it comes back
/// as a [`EmploymentError::DigestConflict`] instead of the documented replay.
/// `None` and `Some("")` are separated by a tag byte, so an absent org unit and
/// a blank one are not the same command.
fn payload_digest(command: &EmploymentCommand) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(command.org_id.as_uuid().as_bytes());
    hasher.update(command.command_id.as_uuid().as_bytes());
    hasher.update(command.actor_id.as_uuid().as_bytes());
    hasher.update(command.query.target().as_str().as_bytes());
    match &command.query {
        EmploymentQuery::Appoint { employee_id, .. } => hasher.update(employee_id.as_bytes()),
        EmploymentQuery::Promote { employment_id, .. }
        | EmploymentQuery::Transfer { employment_id, .. } => {
            hasher.update(employment_id.as_bytes());
        }
    }
    hasher.update(
        command
            .query
            .valid_from()
            .unix_timestamp_nanos()
            .to_be_bytes(),
    );
    let attributes = command.query.attributes();
    // Tag + 16 raw bytes for UUID options; tag-only for None. Length-prefixed
    // UTF-8 would let a free-text label collide with a UUID string encoding.
    hasher.update(attributes.company.as_bytes());
    match attributes.org_unit_id {
        Some(id) => {
            hasher.update([1_u8]);
            hasher.update(id.as_bytes());
        }
        None => hasher.update([0_u8]),
    }
    match attributes.job_position_id {
        Some(id) => {
            hasher.update([1_u8]);
            hasher.update(id.as_bytes());
        }
        None => hasher.update([0_u8]),
    }
    hasher.update(attributes.employment_status.as_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod port_error_kind_tests {
    use super::*;
    use console_kernel_core::ErrorKind;
    use console_ontology_canonical_domain::CanonicalPortError;

    #[test]
    fn digest_conflict_is_conflict_not_internal() {
        assert_eq!(
            EmploymentError::DigestConflict(Uuid::nil())
                .into_kernel_error()
                .kind,
            ErrorKind::Conflict
        );
    }

    #[test]
    fn ambiguous_binding_is_validation() {
        assert_eq!(
            EmploymentError::AmbiguousSourceBinding {
                employment_id: Uuid::nil(),
                binding_count: 2,
            }
            .into_kernel_error()
            .kind,
            ErrorKind::Validation
        );
    }

    #[test]
    fn frozen_window_is_conflict_not_internal() {
        assert_eq!(
            EmploymentError::Frozen(KernelError::conflict("payroll period locked".to_owned()))
                .into_kernel_error()
                .kind,
            ErrorKind::Conflict
        );
    }

    #[test]
    fn frozen_unparseable_date_is_validation_not_internal() {
        assert_eq!(
            EmploymentError::Frozen(KernelError::validation("unparseable date".to_owned()))
                .into_kernel_error()
                .kind,
            ErrorKind::Validation
        );
    }
}
