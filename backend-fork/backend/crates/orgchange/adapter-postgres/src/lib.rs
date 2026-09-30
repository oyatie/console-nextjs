//! Tenant-armed persistence for the org-change lifecycle engine
//! (draft → preflight → ordered SoD approval → effective-dated apply).
//!
//! Every mutation runs inside one `with_audits` transaction: the request row is
//! locked `FOR UPDATE`, the FSM transition is validated by the domain crate,
//! the append-only `org_change_events` row and the audit-chain rows commit
//! together, and any failure rolls the whole business action back.
//!
//! Apply-time note (scout gap): the identity/registry application commands each
//! open their OWN audited transaction, so replaying an approved proposal
//! through them could commit a partial apply. The executor therefore performs
//! the op SQL itself inside the single apply transaction, re-implementing the
//! SAME referential guards identity/registry enforce (active branches / active
//! users / non-terminal equipment / **active target region** for
//! `CreateBranch` and `RenameBranch{region_id: Some(_)}` — console-k6wm inverse
//! of `DeactivateRegion`); the DB constraints remain the second net.
/// OrgUnit owner-crate binding seam, compiled in-place via `#[path]` so this
/// adapter never takes a Cargo dependency on
/// `console-ontology-canonical-adapter-postgres` (adapter→adapter is illegal).
/// Writer-ownership still attributes the SQL to the owner path on disk.
#[path = "../../../ontology/canonical-adapter-postgres/src/org_unit_binding.rs"]
pub mod org_unit_binding;

use console_governance_domain::{Dependent, OnDelete, assess_impact};
use console_kernel_core::{
    AuditAction, AuditClassification, AuditEvent, ErrorKind, KernelError, OrgId, TraceContext,
    UserId,
};
use console_orgchange_domain::{
    ApprovalRoleKey, ApprovalStepView, CanonicalResolutionStatus, OrgChangeDetail,
    OrgChangeEventView, OrgChangeKind, OrgChangePage, OrgChangeStatus, OrgChangeSummary,
    OrgChangeTarget, OrgEntitySummary, OrgProposalOp, OrgUnitReference, PreflightBlocker,
    PreflightReport, PreflightWarning, SettlementItemView, SettlementKey, StepDecision, TargetKind,
    validate_proposal,
};
use console_platform_db::{
    DbError, PeriodLockDomain, assert_period_open, with_audit, with_audits, with_org_conn,
};
use console_platform_request_context::current_org;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use std::{collections::BTreeMap, sync::Arc};
use time::{Date, OffsetDateTime, macros::offset};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PgOrgChangeError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Domain(#[from] KernelError),
    /// §3.9.1 변경 동결 창: a live org mutation was refused because the period
    /// it takes effect in is closed. Distinct from `Domain` so the caller can
    /// record the refused attempt (`record_freeze_refusal`) before the 409
    /// leaves the store — the business transaction, and any audit row inside
    /// it, has already rolled back by then.
    #[error(transparent)]
    Frozen(KernelError),
}

impl From<sqlx::Error> for PgOrgChangeError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

impl PgOrgChangeError {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Domain(e) | Self::Frozen(e) => e.kind,
            Self::Db(DbError::Sqlx(sqlx::Error::RowNotFound)) => ErrorKind::NotFound,
            Self::Db(DbError::Sqlx(sqlx::Error::Database(e)))
                // unique violation (idempotency/code race), CHECK violation
                // (gov_approvals self-approval), FK violation (referential net)
                if matches!(e.code().as_deref(), Some("23505" | "23514" | "23503")) =>
            {
                ErrorKind::Conflict
            }
            Self::Db(_) => ErrorKind::Internal,
        }
    }

    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Domain(e) | Self::Frozen(e) => e.message.clone(),
            Self::Db(DbError::Sqlx(sqlx::Error::Database(e))) => match e.code().as_deref() {
                Some("23514") => "기안자는 자신의 조직 개편을 승인할 수 없습니다.".to_owned(),
                Some("23505") => "a conflicting record already exists".to_owned(),
                Some("23503") => "a referenced record does not exist".to_owned(),
                _ => "internal server error".to_owned(),
            },
            Self::Db(_) => "internal server error".to_owned(),
        }
    }
}

type Tx<'a, 'b> = &'a mut Transaction<'b, Postgres>;

/// The future an [`EmploymentTransferPort::reassign_org_unit`] call returns. It
/// borrows the caller's apply transaction for its whole duration — the same
/// lifetime the `with_audits` closure family already pins in
/// `console-platform-db`.
pub type TransferPortFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<u64, KernelError>> + Send + 'a>>;

/// The Employment-port write seam for `ReassignOrgUnit`, declared by the
/// CONSUMER because the layer-boundary gate forbids one adapter depending on
/// another: this adapter may reach `console-ontology-canonical-domain` but never
/// `console-ontology-canonical-adapter-postgres`, which the contract names as
/// `ObjectKey::Employment`'s sole writer. The composition root (`console-app`)
/// injects the owner's `reassign_org_unit_via_transfers_in_tx` through this
/// seam, so the owning crate keeps the DML and this crate keeps the org-change
/// apply transaction, the domain validation, and the org_unit reads. The
/// orgchange crate therefore compiles no Employment DML at all.
pub trait EmploymentTransferPort: Send + Sync {
    /// Emit one `hr.transfer` per matched ACTIVE employee inside the caller's
    /// org-armed apply transaction, returning the number moved. Fail-closed on
    /// free-text team labels and on unknown from/to OrgUnits, exactly as the
    /// owning port's own execute path does.
    #[allow(clippy::too_many_arguments)] // tx + org/actor/op ids + from/to/company/valid_from mirror the apply-op surface
    fn reassign_org_unit<'tx, 's>(
        &self,
        tx: &'tx mut Transaction<'_, Postgres>,
        org: OrgId,
        actor: UserId,
        op_command_id: Uuid,
        from_org_unit: &'s str,
        to_org_unit: &'s str,
        company: &'s str,
        valid_from: OffsetDateTime,
    ) -> TransferPortFuture<'tx>;
}

#[derive(Debug, Clone)]
pub struct CreateOrgChange {
    pub kind: OrgChangeKind,
    pub target: OrgChangeTarget,
    pub effective_date: Date,
    pub reason: String,
    pub proposal: Vec<OrgProposalOp>,
    pub supersedes_id: Option<Uuid>,
    pub idempotency_key: String,
    pub fingerprint_input: Value,
}

#[derive(Debug, Clone, Default)]
pub struct DraftPatch {
    pub kind: Option<OrgChangeKind>,
    pub effective_date: Option<Date>,
    pub reason: Option<String>,
    pub proposal: Option<Vec<OrgProposalOp>>,
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub status: Option<OrgChangeStatus>,
    pub kind: Option<OrgChangeKind>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Clone)]
pub struct PgOrgChangeStore {
    pool: PgPool,
    /// The owning Employment port's transfer write, injected by the composition
    /// root. `None` (fail-closed) until wired: `ReassignOrgUnit` refuses rather
    /// than silently skipping the move.
    employment_transfer: Option<Arc<dyn EmploymentTransferPort>>,
}

const SELECT_REQUEST: &str = "SELECT id, code, kind, status, target_kind, target_ref, \
     target_label, effective_date, reason, proposal, preflight, headcount, site_count, \
     team_count, supersedes_id, drafted_by, created_at, updated_at \
     FROM org_change_requests WHERE id = $1";
const SELECT_REQUEST_FOR_UPDATE: &str = "SELECT id, code, kind, status, target_kind, \
     target_ref, target_label, effective_date, reason, proposal, preflight, headcount, \
     site_count, team_count, supersedes_id, drafted_by, created_at, updated_at \
     FROM org_change_requests WHERE id = $1 FOR UPDATE";
const SELECT_REQUEST_PAGE: &str = "SELECT id, code, kind, status, target_kind, target_ref, \
     target_label, effective_date, reason, proposal, preflight, headcount, site_count, \
     team_count, supersedes_id, drafted_by, created_at, updated_at \
     FROM org_change_requests \
     WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR kind = $2) \
     ORDER BY created_at DESC, id LIMIT $3 OFFSET $4";

/// The org-local business date (KST) used for effective-date gating.
fn today_kst() -> Date {
    OffsetDateTime::now_utc().to_offset(offset!(+9)).date()
}

fn bounded_text(value: &str, name: &str, max: usize) -> Result<(), KernelError> {
    if value.trim().is_empty() || value.chars().count() > max {
        return Err(KernelError::validation(format!(
            "{name} is required and must be at most {max} characters"
        )));
    }
    Ok(())
}

fn audit(
    org: OrgId,
    actor: UserId,
    action: &str,
    target_kind: &str,
    target_id: String,
    at: OffsetDateTime,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new(action)?,
        target_kind,
        target_id,
        TraceContext::generate(),
        at,
    )
    .with_org(org))
}

fn fingerprint(value: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.to_string());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

struct RequestRow {
    kind: OrgChangeKind,
    status: OrgChangeStatus,
    target: OrgChangeTarget,
    effective_date: Date,
    proposal: Vec<OrgProposalOp>,
    drafted_by: Uuid,
}

fn parse_proposal(value: &Value) -> Result<Vec<OrgProposalOp>, PgOrgChangeError> {
    serde_json::from_value(value.clone())
        .map_err(|_| KernelError::internal("stored proposal is not parseable").into())
}

fn request_row(row: &PgRow) -> Result<RequestRow, PgOrgChangeError> {
    let proposal_json: Value = row.try_get("proposal")?;
    Ok(RequestRow {
        kind: OrgChangeKind::from_db(row.try_get::<String, _>("kind")?.as_str())?,
        status: OrgChangeStatus::from_db(row.try_get::<String, _>("status")?.as_str())?,
        target: OrgChangeTarget {
            kind: TargetKind::from_db(row.try_get::<String, _>("target_kind")?.as_str())?,
            target_ref: row.try_get("target_ref")?,
            label: row.try_get("target_label")?,
        },
        effective_date: row.try_get("effective_date")?,
        proposal: parse_proposal(&proposal_json)?,
        drafted_by: row.try_get("drafted_by")?,
    })
}

fn summary_from_row(row: &PgRow) -> Result<OrgChangeSummary, PgOrgChangeError> {
    Ok(OrgChangeSummary {
        id: row.try_get("id")?,
        code: row.try_get("code")?,
        kind: OrgChangeKind::from_db(row.try_get::<String, _>("kind")?.as_str())?,
        status: OrgChangeStatus::from_db(row.try_get::<String, _>("status")?.as_str())?,
        target: OrgChangeTarget {
            kind: TargetKind::from_db(row.try_get::<String, _>("target_kind")?.as_str())?,
            target_ref: row.try_get("target_ref")?,
            label: row.try_get("target_label")?,
        },
        effective_date: row.try_get("effective_date")?,
        reason: row.try_get("reason")?,
        headcount: row.try_get("headcount")?,
        site_count: row.try_get("site_count")?,
        team_count: row.try_get("team_count")?,
        drafted_by: row.try_get("drafted_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        supersedes_id: row.try_get("supersedes_id")?,
    })
}

fn preflight_from_row(
    row: &PgRow,
    status: OrgChangeStatus,
) -> Result<Option<PreflightReport>, PgOrgChangeError> {
    let raw: Option<Value> = row.try_get("preflight")?;
    let Some(raw) = raw else { return Ok(None) };
    let mut report: PreflightReport = serde_json::from_value(raw)
        .map_err(|_| KernelError::internal("stored preflight receipt is not parseable"))?;
    let updated_at: OffsetDateTime = row.try_get("updated_at")?;
    // A receipt is stale only while the draft is still editable and was
    // edited after the receipt was computed; post-submit receipts were
    // re-verified inside the submit transaction.
    report.stale = status.is_draft_editable() && report.computed_at < updated_at;
    Ok(Some(report))
}

async fn load_detail(
    tx: Tx<'_, '_>,
    id: Uuid,
) -> Result<Option<OrgChangeDetail>, PgOrgChangeError> {
    let Some(row) = sqlx::query(SELECT_REQUEST)
        .bind(id)
        .fetch_optional(tx.as_mut())
        .await?
    else {
        return Ok(None);
    };
    let summary = summary_from_row(&row)?;
    let proposal_json: Value = row.try_get("proposal")?;
    let proposal = parse_proposal(&proposal_json)?;
    let preflight = preflight_from_row(&row, summary.status)?;

    let step_rows = sqlx::query(
        "SELECT id, step_order, role_key, decision, decided_by, decided_at, memo \
         FROM org_change_approval_steps WHERE request_id = $1 ORDER BY step_order",
    )
    .bind(id)
    .fetch_all(tx.as_mut())
    .await?;
    let mut approval_steps = Vec::with_capacity(step_rows.len());
    for step in &step_rows {
        approval_steps.push(ApprovalStepView {
            id: step.try_get("id")?,
            step_order: step.try_get("step_order")?,
            role_key: ApprovalRoleKey::from_db(step.try_get::<String, _>("role_key")?.as_str())?,
            decision: StepDecision::from_db(step.try_get::<String, _>("decision")?.as_str())?,
            decided_by: step.try_get("decided_by")?,
            decided_at: step.try_get("decided_at")?,
            memo: step.try_get("memo")?,
        });
    }

    let item_rows = sqlx::query(
        "SELECT id, item_key, done, done_by, done_at, memo \
         FROM org_change_settlement_items WHERE request_id = $1",
    )
    .bind(id)
    .fetch_all(tx.as_mut())
    .await?;
    let mut settlement_items = Vec::with_capacity(item_rows.len());
    for item in &item_rows {
        let key = item.try_get::<String, _>("item_key")?;
        let key = SettlementKey::ALL
            .into_iter()
            .find(|k| k.as_db() == key)
            .ok_or_else(|| KernelError::internal("unknown settlement item key"))?;
        settlement_items.push(SettlementItemView {
            id: item.try_get("id")?,
            item_key: key,
            label: key.label().to_owned(),
            done: item.try_get("done")?,
            done_by: item.try_get("done_by")?,
            done_at: item.try_get("done_at")?,
            memo: item.try_get("memo")?,
        });
    }
    // Design order (§3.9.3), not key order.
    settlement_items.sort_by_key(|item| {
        SettlementKey::ALL
            .iter()
            .position(|k| *k == item.item_key)
            .unwrap_or(usize::MAX)
    });

    let event_rows = sqlx::query(
        "SELECT actor, action, from_status, to_status, reason, created_at \
         FROM org_change_events WHERE request_id = $1 ORDER BY created_at, id",
    )
    .bind(id)
    .fetch_all(tx.as_mut())
    .await?;
    let mut events = Vec::with_capacity(event_rows.len());
    for event in &event_rows {
        events.push(OrgChangeEventView {
            at: event.try_get("created_at")?,
            actor: event.try_get("actor")?,
            action: event.try_get("action")?,
            from_status: event.try_get("from_status")?,
            to_status: event.try_get("to_status")?,
            reason: event.try_get("reason")?,
        });
    }

    Ok(Some(OrgChangeDetail {
        summary,
        proposal,
        preflight,
        approval_steps,
        settlement_items,
        events,
    }))
}

async fn require_detail(tx: Tx<'_, '_>, id: Uuid) -> Result<OrgChangeDetail, PgOrgChangeError> {
    load_detail(tx, id)
        .await?
        .ok_or_else(|| KernelError::internal("org-change request vanished mid-transaction").into())
}

async fn lock_request(tx: Tx<'_, '_>, id: Uuid) -> Result<RequestRow, PgOrgChangeError> {
    let row = sqlx::query(SELECT_REQUEST_FOR_UPDATE)
        .bind(id)
        .fetch_optional(tx.as_mut())
        .await?
        .ok_or_else(|| KernelError::not_found("org-change request was not found"))?;
    request_row(&row)
}

#[allow(clippy::too_many_arguments)]
async fn insert_event(
    tx: Tx<'_, '_>,
    org: OrgId,
    request_id: Uuid,
    actor: UserId,
    action: &str,
    from_status: Option<OrgChangeStatus>,
    to_status: Option<OrgChangeStatus>,
    reason: Option<&str>,
    at: OffsetDateTime,
) -> Result<(), PgOrgChangeError> {
    sqlx::query(
        "INSERT INTO org_change_events \
         (org_id, request_id, actor, action, from_status, to_status, reason, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(*org.as_uuid())
    .bind(request_id)
    .bind(*actor.as_uuid())
    .bind(action)
    .bind(from_status.map(OrgChangeStatus::as_db))
    .bind(to_status.map(OrgChangeStatus::as_db))
    .bind(reason)
    .bind(at)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Preflight computation (deterministic, in the caller's transaction)
// ---------------------------------------------------------------------------

async fn count_by_uuid(
    tx: Tx<'_, '_>,
    sql: &'static str,
    bind: Uuid,
) -> Result<i64, PgOrgChangeError> {
    Ok(sqlx::query_scalar(sql)
        .bind(bind)
        .fetch_one(tx.as_mut())
        .await?)
}

async fn count_by_text(
    tx: Tx<'_, '_>,
    sql: &'static str,
    bind: &str,
) -> Result<i64, PgOrgChangeError> {
    Ok(sqlx::query_scalar(sql)
        .bind(bind)
        .fetch_one(tx.as_mut())
        .await?)
}

async fn scope_headcount(
    tx: Tx<'_, '_>,
    target: &OrgChangeTarget,
) -> Result<i64, PgOrgChangeError> {
    let count = match target.kind {
        TargetKind::Entity => {
            count_by_text(
                tx,
                "SELECT count(*) FROM employees \
                 WHERE company = $1 AND employment_status = 'ACTIVE'",
                &target.label,
            )
            .await?
        }
        TargetKind::OrgUnit => {
            count_by_text(
                tx,
                "SELECT count(*) FROM employees \
                 WHERE org_unit = $1 AND employment_status = 'ACTIVE'",
                &target.target_ref,
            )
            .await?
        }
        TargetKind::Branch => {
            let branch = parse_ref_uuid(target)?;
            count_by_uuid(
                tx,
                "SELECT count(*) FROM employees \
                 WHERE home_branch_id = $1 AND employment_status = 'ACTIVE'",
                branch,
            )
            .await?
        }
        TargetKind::Region => {
            let region = parse_ref_uuid(target)?;
            count_by_uuid(
                tx,
                "SELECT count(*) FROM employees \
                 WHERE employment_status = 'ACTIVE' AND home_branch_id IN \
                 (SELECT id FROM branches WHERE region_id = $1)",
                region,
            )
            .await?
        }
        // Customer sites carry no employees; an honest zero, never a guess.
        TargetKind::Site => 0,
    };
    Ok(count)
}

fn parse_ref_uuid(target: &OrgChangeTarget) -> Result<Uuid, PgOrgChangeError> {
    Uuid::parse_str(&target.target_ref).map_err(|_| {
        PgOrgChangeError::from(KernelError::validation(
            "target.ref must be a UUID for REGION/BRANCH/SITE targets",
        ))
    })
}

fn proposal_stats(target: &OrgChangeTarget, proposal: &[OrgProposalOp]) -> (i64, i64) {
    let mut sites: i64 = i64::from(target.kind == TargetKind::Site);
    let mut teams = std::collections::BTreeSet::new();
    if target.kind == TargetKind::OrgUnit {
        teams.insert(target.target_ref.clone());
    }
    for op in proposal {
        match op {
            OrgProposalOp::CreateSite { .. } | OrgProposalOp::UpdateSite { .. } => sites += 1,
            OrgProposalOp::ReassignOrgUnit {
                from_org_unit,
                to_org_unit,
                ..
            } => {
                teams.insert(from_org_unit.clone());
                teams.insert(to_org_unit.clone());
            }
            _ => {}
        }
    }
    (sites, i64::try_from(teams.len()).unwrap_or(i64::MAX))
}

async fn compute_preflight(
    tx: Tx<'_, '_>,
    kind: OrgChangeKind,
    target: &OrgChangeTarget,
    proposal: &[OrgProposalOp],
    effective_date: Date,
    now: OffsetDateTime,
) -> Result<PreflightReport, PgOrgChangeError> {
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();
    let mut restrict_dependents: Vec<Dependent> = Vec::new();
    let mut dependents_total: i64 = 0;

    // Proposal targets can disappear or become unusable after a DRAFT is
    // written. Re-check them on every preflight/submit so an approval chain is
    // never spent on a request that apply already knows it cannot effectuate.
    // Requirements are coalesced by UUID; Active dominates Exists when several
    // ops refer to the same row.
    let (region_requirements, branch_requirements) = org_unit_requirements(proposal);
    let mut unavailable_regions = Vec::new();
    for (region_id, required) in region_requirements {
        let row: Option<(Option<OffsetDateTime>,)> =
            sqlx::query_as("SELECT deactivated_at FROM regions WHERE id = $1")
                .bind(region_id)
                .fetch_optional(tx.as_mut())
                .await?;
        let available = row.is_some_and(|(deactivated_at,)| {
            required == RequiredTargetState::Exists || deactivated_at.is_none()
        });
        if !available {
            unavailable_regions.push(region_id);
        }
    }
    let mut unavailable_branches = Vec::new();
    for (branch_id, required) in branch_requirements {
        let row: Option<(Option<OffsetDateTime>,)> =
            sqlx::query_as("SELECT deactivated_at FROM branches WHERE id = $1")
                .bind(branch_id)
                .fetch_optional(tx.as_mut())
                .await?;
        let available = row.is_some_and(|(deactivated_at,)| {
            required == RequiredTargetState::Exists || deactivated_at.is_none()
        });
        if !available {
            unavailable_branches.push(branch_id);
        }
    }
    if !unavailable_regions.is_empty() {
        let count = i64::try_from(unavailable_regions.len()).unwrap_or(i64::MAX);
        blockers.push(PreflightBlocker {
            code: "REGION_TARGET_UNAVAILABLE".to_owned(),
            label: "대상 지역을 찾을 수 없거나 비활성 상태입니다.".to_owned(),
            dependent_kind: "region".to_owned(),
            count,
        });
        dependents_total += count;
        restrict_dependents.extend(unavailable_regions.into_iter().map(|id| Dependent {
            kind: "region".to_owned(),
            id: id.to_string(),
            on_delete: OnDelete::Restrict,
        }));
    }
    if !unavailable_branches.is_empty() {
        let count = i64::try_from(unavailable_branches.len()).unwrap_or(i64::MAX);
        blockers.push(PreflightBlocker {
            code: "BRANCH_TARGET_UNAVAILABLE".to_owned(),
            label: "대상 지점을 찾을 수 없거나 비활성 상태입니다.".to_owned(),
            dependent_kind: "branch".to_owned(),
            count,
        });
        dependents_total += count;
        restrict_dependents.extend(unavailable_branches.into_iter().map(|id| Dependent {
            kind: "branch".to_owned(),
            id: id.to_string(),
            on_delete: OnDelete::Restrict,
        }));
    }

    // Restrict-dependent scan per deactivation op — the same guards the
    // identity referential nets enforce at apply time, surfaced early.
    for op in proposal {
        match op {
            OrgProposalOp::DeactivateBranch { branch_id } => {
                let users = count_by_uuid(
                    tx,
                    "SELECT count(*) FROM user_branches ub \
                     JOIN users u ON u.id = ub.user_id \
                     WHERE ub.branch_id = $1 AND u.is_active = true",
                    *branch_id,
                )
                .await?;
                let equipment = count_by_uuid(
                    tx,
                    "SELECT count(*) FROM registry_equipment \
                     WHERE branch_id = $1 AND status NOT IN ('폐기', '매각')",
                    *branch_id,
                )
                .await?;
                push_dependent(
                    kind,
                    &mut blockers,
                    &mut warnings,
                    &mut restrict_dependents,
                    "ACTIVE_USERS",
                    "활성 사용자 재배정 필요",
                    "user",
                    branch_id.to_string(),
                    users,
                );
                push_dependent(
                    kind,
                    &mut blockers,
                    &mut warnings,
                    &mut restrict_dependents,
                    "ACTIVE_EQUIPMENT",
                    "장비 이동·폐기·매각 처리 필요",
                    "equipment",
                    branch_id.to_string(),
                    equipment,
                );
                dependents_total += users + equipment;
            }
            OrgProposalOp::DeactivateRegion { region_id } => {
                let branches = count_by_uuid(
                    tx,
                    "SELECT count(*) FROM branches \
                     WHERE region_id = $1 AND deactivated_at IS NULL",
                    *region_id,
                )
                .await?;
                push_dependent(
                    kind,
                    &mut blockers,
                    &mut warnings,
                    &mut restrict_dependents,
                    "ACTIVE_BRANCHES",
                    "활성 지점 비활성화·이동 필요",
                    "branch",
                    region_id.to_string(),
                    branches,
                );
                dependents_total += branches;
            }
            _ => {}
        }
    }

    let headcount = scope_headcount(tx, target).await?;
    if kind == OrgChangeKind::Dissolve && headcount > 0 {
        // Settlement (전보·전적) clears these after 발효 — a settlement-stage
        // dependency, not a submit blocker; archive re-verifies in-transaction.
        warnings.push(PreflightWarning {
            code: "EMPLOYEES_SETTLEMENT".to_owned(),
            label: "소속 직원 전보·전적 정산 필요".to_owned(),
            dependent_kind: Some("employee".to_owned()),
            count: Some(headcount),
        });
        dependents_total += headcount;
    }

    // Design §3.9.1 review chip: an imperative reminder, not a computed claim —
    // the open-docs signal read is a registered follow-up.
    warnings.push(PreflightWarning {
        code: "OPEN_DOCS_REVIEW".to_owned(),
        label: "진행 중 공고·결재 종결 필요".to_owned(),
        dependent_kind: None,
        count: None,
    });
    // §3.9.1 동결 창 — computed, not a reminder: literally the read
    // `assert_change_window_open` enforces at apply time, so the chip and the
    // refusal can never disagree. Surfaced early so the approval chain is not
    // spent on a change effectuate would refuse. It stays a warning rather than
    // a blocker because a lock can be lifted before the effective date arrives;
    // the hard stop lives at effectuate.
    let mut frozen = Vec::new();
    for domain in FREEZE_DOMAINS {
        match assert_period_open(tx, domain, effective_date).await {
            Ok(()) => {}
            Err(refusal) if refusal.kind == ErrorKind::Conflict => {
                frozen.push(freeze_domain_label(domain));
            }
            Err(other) => return Err(other.into()),
        }
    }
    // One chip however many domains block: the console keys this list by `code`
    // (`web/src/console/org/OrgChangeModal.tsx`), so a second row under the same
    // code is a duplicate React key, not a second signal.
    if !frozen.is_empty() {
        warnings.push(PreflightWarning {
            code: "FREEZE_WINDOW_REVIEW".to_owned(),
            label: format!("{} 동결 — 발효일 조정 필요", frozen.join("·")),
            dependent_kind: None,
            count: None,
        });
    }

    // Governance verdict: Restrict dependents ⇒ deny (arch §15).
    let impact = assess_impact(restrict_dependents);
    if impact.allow != blockers.is_empty() {
        return Err(KernelError::internal("impact assessment disagrees with blocker scan").into());
    }

    Ok(PreflightReport {
        computed_at: now,
        stale: false,
        blockers,
        warnings,
        headcount,
        dependents_total,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum RequiredTargetState {
    Exists,
    Active,
}

fn require_target_state(
    requirements: &mut BTreeMap<Uuid, RequiredTargetState>,
    id: Uuid,
    required: RequiredTargetState,
) {
    requirements
        .entry(id)
        .and_modify(|current| *current = (*current).max(required))
        .or_insert(required);
}

fn org_unit_requirements(
    proposal: &[OrgProposalOp],
) -> (
    BTreeMap<Uuid, RequiredTargetState>,
    BTreeMap<Uuid, RequiredTargetState>,
) {
    let mut regions = BTreeMap::new();
    let mut branches = BTreeMap::new();
    for op in proposal {
        match op {
            OrgProposalOp::CreateRegion { .. } => {}
            OrgProposalOp::RenameRegion { region_id, .. } => {
                require_target_state(&mut regions, *region_id, RequiredTargetState::Exists);
            }
            OrgProposalOp::DeactivateRegion { region_id }
            | OrgProposalOp::CreateBranch { region_id, .. } => {
                require_target_state(&mut regions, *region_id, RequiredTargetState::Active);
            }
            OrgProposalOp::RenameBranch {
                branch_id,
                region_id,
                ..
            } => {
                require_target_state(&mut branches, *branch_id, RequiredTargetState::Exists);
                if let Some(region_id) = region_id {
                    require_target_state(&mut regions, *region_id, RequiredTargetState::Active);
                }
            }
            OrgProposalOp::DeactivateBranch { branch_id } => {
                require_target_state(&mut branches, *branch_id, RequiredTargetState::Active);
            }
            OrgProposalOp::CreateSite { .. }
            | OrgProposalOp::UpdateSite { .. }
            | OrgProposalOp::ReassignOrgUnit { .. } => {}
        }
    }
    (regions, branches)
}

#[allow(clippy::too_many_arguments)]
fn push_dependent(
    kind: OrgChangeKind,
    blockers: &mut Vec<PreflightBlocker>,
    warnings: &mut Vec<PreflightWarning>,
    restrict: &mut Vec<Dependent>,
    code: &str,
    label: &str,
    dependent_kind: &str,
    dependent_id: String,
    count: i64,
) {
    if count == 0 {
        return;
    }
    if kind == OrgChangeKind::Dissolve {
        // Dissolve resolves dependents during 정산; archive re-verifies.
        warnings.push(PreflightWarning {
            code: code.to_owned(),
            label: label.to_owned(),
            dependent_kind: Some(dependent_kind.to_owned()),
            count: Some(count),
        });
    } else {
        blockers.push(PreflightBlocker {
            code: code.to_owned(),
            label: label.to_owned(),
            dependent_kind: dependent_kind.to_owned(),
            count,
        });
        restrict.push(Dependent {
            kind: dependent_kind.to_owned(),
            id: dependent_id,
            on_delete: OnDelete::Restrict,
        });
    }
}

async fn store_preflight(
    tx: Tx<'_, '_>,
    id: Uuid,
    report: &PreflightReport,
    status: OrgChangeStatus,
    site_count: i64,
    team_count: i64,
    now: OffsetDateTime,
) -> Result<(), PgOrgChangeError> {
    let receipt = serde_json::to_value(report)
        .map_err(|_| KernelError::internal("preflight receipt serialization failed"))?;
    sqlx::query(
        "UPDATE org_change_requests SET preflight = $2, headcount = $3, site_count = $4, \
         team_count = $5, status = $6, updated_at = $7 WHERE id = $1",
    )
    .bind(id)
    .bind(receipt)
    .bind(report.headcount)
    .bind(site_count)
    .bind(team_count)
    .bind(status.as_db())
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Apply executor — replays the approved proposal inside the caller's tx.
// ---------------------------------------------------------------------------

/// The freeze domains §3.9.1 names — 급여 마감 and 회계 결산. One list, so the
/// preflight chip and the apply gate can never check a different set.
const FREEZE_DOMAINS: [PeriodLockDomain; 2] =
    [PeriodLockDomain::Payroll, PeriodLockDomain::Accounting];

/// Console-facing name of a freeze domain. The platform refusal message is
/// English and shared with the financial surface; the preflight chip sits in a
/// Korean list (`OrgChangeModal.tsx`), so it names the domain in Korean.
const fn freeze_domain_label(domain: PeriodLockDomain) -> &'static str {
    match domain {
        PeriodLockDomain::Payroll => "급여 마감",
        PeriodLockDomain::Accounting => "회계 결산",
    }
}

/// §3.9.1 변경 동결 창 — refuse a live org mutation whose effective date falls
/// inside a closed payroll or accounting period, in which case the run whose
/// scope and attribution it would rewrite is already sealed.
///
/// This is the platform period-lock mechanism (migration 0107,
/// `console_platform_db::period_lock`), not a second freeze concept: the lookup
/// runs in the caller's already-armed transaction, so RLS confines it to the
/// caller's own tenant and the refusal rolls the whole apply back.
async fn assert_change_window_open(
    tx: Tx<'_, '_>,
    effective_date: Date,
) -> Result<(), PgOrgChangeError> {
    for domain in FREEZE_DOMAINS {
        match assert_period_open(tx, domain, effective_date).await {
            Ok(()) => {}
            // Conflict = that window is closed. The platform phrases its own
            // refusal in English for the financial back office; this crate's
            // conflicts reach the approval modal verbatim (`orgApi.ts` →
            // `OrgChangeModal.tsx`) beside 발효일/SoD refusals that are Korean,
            // so the freeze refusal is named the same way there.
            Err(refusal) if refusal.kind == ErrorKind::Conflict => {
                return Err(PgOrgChangeError::Frozen(KernelError::conflict(format!(
                    "{} 기간에 포함된 발효일({effective_date})에는 조직 변경을 적용할 수 없습니다.",
                    freeze_domain_label(domain)
                ))));
            }
            Err(other) => return Err(other.into()),
        }
    }
    Ok(())
}

/// Inverse of `DeactivateRegion`'s active-branch guard (console-k6wm / identity
/// lx6 sibling). Called by both `CreateBranch` and `RenameBranch` region_id arms
/// so the region-active invariant cannot drift between the two write paths.
/// Missing region → conflict (matches other apply_op not-found style);
/// deactivated → conflict (409). A CHECK/partial index cannot express this
/// cross-row rule.
async fn require_active_region_tx(tx: Tx<'_, '_>, region_id: Uuid) -> Result<(), PgOrgChangeError> {
    let row: Option<(Uuid, Option<OffsetDateTime>)> =
        sqlx::query_as("SELECT id, deactivated_at FROM regions WHERE id = $1 FOR UPDATE")
            .bind(region_id)
            .fetch_optional(tx.as_mut())
            .await?;
    let Some((_, deactivated_at)) = row else {
        return Err(KernelError::conflict("region was not found").into());
    };
    if deactivated_at.is_some() {
        return Err(KernelError::conflict("비활성화된 지역에는 지점을 배정할 수 없습니다.").into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // tx + org/actor/op ids + proposal + freeze date + the injected transfer port
async fn apply_ops(
    tx: Tx<'_, '_>,
    org: OrgId,
    actor: UserId,
    _request_id: Uuid,
    ops: &[OrgProposalOp],
    effective_date: Date,
    now: OffsetDateTime,
    employment_transfer: Option<&dyn EmploymentTransferPort>,
) -> Result<Vec<AuditEvent>, PgOrgChangeError> {
    // Both live-apply entry points (effectuate for NEW/REORG, archive for the
    // deferred DISSOLVE ops) route through here, so the freeze gate sits here
    // rather than in each caller.
    assert_change_window_open(tx, effective_date).await?;
    // Fail closed on the wiring defect BEFORE the per-op conflict mapping: a
    // missing port is a composition error (→ 500), not a proposal conflict
    // (→ 409). The per-op `ok_or_else` in `apply_op` stays as a second net, but
    // through this entry point it is unreachable for the None case.
    if employment_transfer.is_none()
        && ops
            .iter()
            .any(|op| matches!(op, OrgProposalOp::ReassignOrgUnit { .. }))
    {
        return Err(PgOrgChangeError::from(KernelError::internal(
            "employment transfer port is not wired; ReassignOrgUnit cannot apply",
        )));
    }
    let mut audits = Vec::with_capacity(ops.len());
    for (index, op) in ops.iter().enumerate() {
        let command_id = Uuid::new_v4();
        let event = apply_op(tx, org, actor, command_id, op, now, employment_transfer)
            .await
            .map_err(|e| {
                // Preserve the op's kind AND the freeze marker: a closed window
                // stays Frozen(Conflict) so `record_freeze_refusal` fires, while
                // a gate/lookup failure stays Internal (500) instead of being
                // collapsed into a retry-never 409.
                let wrapped = KernelError::new(
                    e.kind(),
                    format!("proposal op {index} failed: {}", e.message()),
                );
                if matches!(&e, PgOrgChangeError::Frozen(_)) {
                    PgOrgChangeError::Frozen(wrapped)
                } else {
                    PgOrgChangeError::from(wrapped)
                }
            })?;
        audits.push(event);
    }
    Ok(audits)
}

async fn apply_op(
    tx: Tx<'_, '_>,
    org: OrgId,
    actor: UserId,
    command_id: Uuid,
    op: &OrgProposalOp,
    now: OffsetDateTime,
    employment_transfer: Option<&dyn EmploymentTransferPort>,
) -> Result<AuditEvent, PgOrgChangeError> {
    let (target_kind, target_id) = match op {
        OrgProposalOp::CreateRegion { name } => {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO regions (id, name, created_at, org_id) VALUES ($1, $2, $3, $4)",
            )
            .bind(id)
            .bind(name)
            .bind(now)
            .bind(*org.as_uuid())
            .execute(tx.as_mut())
            .await?;
            emit_region_or_branch_binding(
                tx,
                org,
                actor,
                command_id,
                org_unit_binding::SOURCE_KIND_REGION,
                id,
                name,
            )
            .await?;
            ("region", id)
        }
        OrgProposalOp::RenameRegion { region_id, name } => {
            let changed = sqlx::query("UPDATE regions SET name = $2 WHERE id = $1")
                .bind(region_id)
                .bind(name)
                .execute(tx.as_mut())
                .await?
                .rows_affected();
            if changed != 1 {
                return Err(KernelError::conflict("region was not found").into());
            }
            ("region", *region_id)
        }
        OrgProposalOp::DeactivateRegion { region_id } => {
            let row: Option<(Uuid, Option<OffsetDateTime>)> =
                sqlx::query_as("SELECT id, deactivated_at FROM regions WHERE id = $1 FOR UPDATE")
                    .bind(region_id)
                    .fetch_optional(tx.as_mut())
                    .await?;
            let Some((_, deactivated_at)) = row else {
                return Err(KernelError::conflict("region was not found").into());
            };
            if deactivated_at.is_some() {
                return Err(KernelError::conflict("region is already deactivated").into());
            }
            let active: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM branches WHERE region_id = $1 AND deactivated_at IS NULL",
            )
            .bind(region_id)
            .fetch_one(tx.as_mut())
            .await?;
            if active > 0 {
                return Err(
                    KernelError::conflict("active branches remain under the region").into(),
                );
            }
            sqlx::query("UPDATE regions SET deactivated_at = $2 WHERE id = $1")
                .bind(region_id)
                .bind(now)
                .execute(tx.as_mut())
                .await?;
            ("region", *region_id)
        }
        OrgProposalOp::CreateBranch { region_id, name } => {
            // Inverse of DeactivateRegion's active-branch guard (console-k6wm /
            // identity lx6 sibling): refuse assigning a live branch under a
            // soft-deleted region that list_regions hides.
            require_active_region_tx(tx, *region_id).await?;
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO branches (id, region_id, name, created_at, org_id) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(region_id)
            .bind(name)
            .bind(now)
            .bind(*org.as_uuid())
            .execute(tx.as_mut())
            .await?;
            emit_region_or_branch_binding(
                tx,
                org,
                actor,
                command_id,
                org_unit_binding::SOURCE_KIND_BRANCH,
                id,
                name,
            )
            .await?;
            ("branch", id)
        }
        OrgProposalOp::RenameBranch {
            branch_id,
            name,
            region_id,
        } => {
            let exists: Option<Uuid> =
                sqlx::query_scalar("SELECT id FROM branches WHERE id = $1 FOR UPDATE")
                    .bind(branch_id)
                    .fetch_optional(tx.as_mut())
                    .await?;
            if exists.is_none() {
                return Err(KernelError::conflict("branch was not found").into());
            }
            if let Some(region_id) = region_id {
                require_active_region_tx(tx, *region_id).await?;
                sqlx::query("UPDATE branches SET region_id = $2 WHERE id = $1")
                    .bind(branch_id)
                    .bind(region_id)
                    .execute(tx.as_mut())
                    .await?;
            }
            if let Some(name) = name {
                sqlx::query("UPDATE branches SET name = $2 WHERE id = $1")
                    .bind(branch_id)
                    .bind(name)
                    .execute(tx.as_mut())
                    .await?;
            }
            ("branch", *branch_id)
        }
        OrgProposalOp::DeactivateBranch { branch_id } => {
            let row: Option<(Uuid, Option<OffsetDateTime>)> =
                sqlx::query_as("SELECT id, deactivated_at FROM branches WHERE id = $1 FOR UPDATE")
                    .bind(branch_id)
                    .fetch_optional(tx.as_mut())
                    .await?;
            let Some((_, deactivated_at)) = row else {
                return Err(KernelError::conflict("branch was not found").into());
            };
            if deactivated_at.is_some() {
                return Err(KernelError::conflict("branch is already deactivated").into());
            }
            let users: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM user_branches ub JOIN users u ON u.id = ub.user_id \
                 WHERE ub.branch_id = $1 AND u.is_active = true",
            )
            .bind(branch_id)
            .fetch_one(tx.as_mut())
            .await?;
            if users > 0 {
                return Err(
                    KernelError::conflict("active users are still assigned to the branch").into(),
                );
            }
            let equipment: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM registry_equipment \
                 WHERE branch_id = $1 AND status NOT IN ('폐기', '매각')",
            )
            .bind(branch_id)
            .fetch_one(tx.as_mut())
            .await?;
            if equipment > 0 {
                return Err(
                    KernelError::conflict("non-terminal equipment remains in the branch").into(),
                );
            }
            sqlx::query("UPDATE branches SET deactivated_at = $2 WHERE id = $1")
                .bind(branch_id)
                .bind(now)
                .execute(tx.as_mut())
                .await?;
            ("branch", *branch_id)
        }
        OrgProposalOp::CreateSite { customer_id, name } => {
            let branch: Option<Uuid> =
                sqlx::query_scalar("SELECT branch_id FROM registry_customers WHERE id = $1")
                    .bind(customer_id)
                    .fetch_optional(tx.as_mut())
                    .await?;
            let Some(branch) = branch else {
                return Err(KernelError::conflict("customer was not found").into());
            };
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO registry_sites (id, branch_id, customer_id, name, org_id) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(branch)
            .bind(customer_id)
            .bind(name)
            .bind(*org.as_uuid())
            .execute(tx.as_mut())
            .await?;
            ("site", id)
        }
        OrgProposalOp::UpdateSite { site_id, name } => {
            let changed =
                sqlx::query("UPDATE registry_sites SET name = $2, updated_at = $3 WHERE id = $1")
                    .bind(site_id)
                    .bind(name)
                    .bind(now)
                    .execute(tx.as_mut())
                    .await?
                    .rows_affected();
            if changed != 1 {
                return Err(KernelError::conflict("site was not found").into());
            }
            ("site", *site_id)
        }
        OrgProposalOp::ReassignOrgUnit {
            from_org_unit,
            to_org_unit,
            scope,
        } => {
            let port = employment_transfer.ok_or_else(|| {
                PgOrgChangeError::from(KernelError::internal(
                    "employment transfer port is not wired; ReassignOrgUnit cannot apply",
                ))
            })?;
            let moved = port
                .reassign_org_unit(
                    tx,
                    org,
                    actor,
                    command_id,
                    from_org_unit,
                    to_org_unit,
                    &scope.company,
                    now,
                )
                .await
                .map_err(|err| {
                    // Preserve the employment error's kind AND the freeze marker
                    // through the trait seam: the seam carries a `KernelError`,
                    // and `Conflict` reaches here ONLY from a freeze refusal (the
                    // app-side adapter maps every non-freeze failure to Internal),
                    // so Conflict re-emits `Frozen` and fires
                    // `record_freeze_refusal`; Internal stays 500 rather than
                    // collapsing into a retry-never 409.
                    let wrapped = KernelError::new(
                        err.kind,
                        format!("REASSIGN_ORG_UNIT via hr.transfer failed: {}", err.message),
                    );
                    if err.kind == ErrorKind::Conflict {
                        PgOrgChangeError::Frozen(wrapped)
                    } else {
                        PgOrgChangeError::from(wrapped)
                    }
                })?;
            let event = audit(
                org,
                actor,
                "org_change.apply.op",
                "org_unit",
                format!("{}/{}", scope.company, to_org_unit),
                now,
            )?
            .with_snapshots(
                Some(serde_json::json!({ "orgUnit": from_org_unit })),
                Some(serde_json::json!({ "orgUnit": to_org_unit, "employeesMoved": moved })),
            );
            return Ok(event);
        }
    };
    Ok(audit(
        org,
        actor,
        "org_change.apply.op",
        target_kind,
        target_id.to_string(),
        now,
    )?)
}

/// L5-ORG production binding: unambiguous region/branch UUID → OrgUnit owner seam.
/// Free-text team labels never reach this helper (typed `Uuid` only).
async fn emit_region_or_branch_binding(
    tx: Tx<'_, '_>,
    org: OrgId,
    actor: UserId,
    command_id: Uuid,
    source_kind: &str,
    legacy_id: Uuid,
    name: &str,
) -> Result<(), PgOrgChangeError> {
    org_unit_binding::ensure_unambiguous_legacy_binding_in_tx(
        tx,
        org,
        actor,
        source_kind,
        legacy_id,
        serde_json::json!({ "name": name }),
        command_id,
    )
    .await
    .map_err(|err| {
        PgOrgChangeError::from(KernelError::conflict(format!(
            "org-unit binding for {source_kind}/{legacy_id} failed: {err}"
        )))
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// Restore the target-state contract of the legacy region/branch mutators
/// while they act as governed proposal inputs. The caller invokes this only
/// after resolving an idempotent replay, so a valid retry remains a replay even
/// when the live target changed after the original DRAFT was accepted.
async fn validate_legacy_org_setup_targets(
    tx: Tx<'_, '_>,
    proposal: &[OrgProposalOp],
) -> Result<(), PgOrgChangeError> {
    let [op] = proposal else {
        return Err(KernelError::validation(
            "legacy org-setup proposals must contain exactly one operation",
        )
        .into());
    };
    match op {
        OrgProposalOp::CreateRegion { .. } => Ok(()),
        OrgProposalOp::RenameRegion { region_id, .. } => {
            let exists: Option<Uuid> =
                sqlx::query_scalar("SELECT id FROM regions WHERE id = $1 FOR UPDATE")
                    .bind(region_id)
                    .fetch_optional(tx.as_mut())
                    .await?;
            if exists.is_none() {
                return Err(KernelError::not_found("region not found").into());
            }
            Ok(())
        }
        OrgProposalOp::DeactivateRegion { region_id } => {
            let row: Option<(Uuid, Option<OffsetDateTime>)> =
                sqlx::query_as("SELECT id, deactivated_at FROM regions WHERE id = $1 FOR UPDATE")
                    .bind(region_id)
                    .fetch_optional(tx.as_mut())
                    .await?;
            let Some((_, deactivated_at)) = row else {
                return Err(KernelError::not_found("region not found").into());
            };
            if deactivated_at.is_some() {
                return Err(KernelError::conflict("이미 비활성화된 지역입니다.").into());
            }
            Ok(())
        }
        OrgProposalOp::CreateBranch { region_id, .. } => {
            validate_legacy_destination_region(tx, *region_id).await
        }
        OrgProposalOp::RenameBranch {
            branch_id,
            region_id,
            ..
        } => {
            let exists: Option<Uuid> =
                sqlx::query_scalar("SELECT id FROM branches WHERE id = $1 FOR UPDATE")
                    .bind(branch_id)
                    .fetch_optional(tx.as_mut())
                    .await?;
            if exists.is_none() {
                return Err(KernelError::not_found("branch not found").into());
            }
            if let Some(region_id) = region_id {
                validate_legacy_destination_region(tx, *region_id).await?;
            }
            Ok(())
        }
        OrgProposalOp::DeactivateBranch { branch_id } => {
            let row: Option<(Uuid, Option<OffsetDateTime>)> =
                sqlx::query_as("SELECT id, deactivated_at FROM branches WHERE id = $1 FOR UPDATE")
                    .bind(branch_id)
                    .fetch_optional(tx.as_mut())
                    .await?;
            let Some((_, deactivated_at)) = row else {
                return Err(KernelError::not_found("branch not found").into());
            };
            if deactivated_at.is_some() {
                return Err(KernelError::conflict("이미 비활성화된 지점입니다.").into());
            }
            Ok(())
        }
        OrgProposalOp::CreateSite { .. }
        | OrgProposalOp::UpdateSite { .. }
        | OrgProposalOp::ReassignOrgUnit { .. } => Err(KernelError::validation(
            "legacy org-setup proposals only accept region or branch operations",
        )
        .into()),
    }
}

async fn validate_legacy_destination_region(
    tx: Tx<'_, '_>,
    region_id: Uuid,
) -> Result<(), PgOrgChangeError> {
    let row: Option<(Uuid, Option<OffsetDateTime>)> =
        sqlx::query_as("SELECT id, deactivated_at FROM regions WHERE id = $1 FOR UPDATE")
            .bind(region_id)
            .fetch_optional(tx.as_mut())
            .await?;
    let Some((_, deactivated_at)) = row else {
        return Err(KernelError::not_found("region not found").into());
    };
    if deactivated_at.is_some() {
        return Err(KernelError::conflict(
            "비활성화된 지역에는 지점을 추가하거나 이동할 수 없습니다.",
        )
        .into());
    }
    Ok(())
}

impl PgOrgChangeStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            employment_transfer: None,
        }
    }

    /// Wire the owning Employment port's transfer write. Without it
    /// `ReassignOrgUnit` fails closed at apply time rather than silently
    /// skipping the employee move.
    #[must_use]
    pub fn with_employment_transfer(mut self, port: Arc<dyn EmploymentTransferPort>) -> Self {
        self.employment_transfer = Some(port);
        self
    }

    /// The wired transfer port, as an owned `Arc` clone so the `with_audits`
    /// closure can hold it for the whole apply without borrowing `self`. `None`
    /// is legal for proposals with no `ReassignOrgUnit` op; the reassign arm
    /// itself fails closed when no port is wired.
    fn employment_transfer(&self) -> Option<Arc<dyn EmploymentTransferPort>> {
        self.employment_transfer.clone()
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn create(
        &self,
        actor: UserId,
        command: CreateOrgChange,
    ) -> Result<(OrgChangeDetail, bool), PgOrgChangeError> {
        self.create_inner(actor, command, false).await
    }

    /// Compatibility entry point for the six legacy region/branch mutation
    /// URLs. It shares the normal DRAFT/idempotency transaction, then restores
    /// their tenant-scoped target validation without granting the REST adapter
    /// a second SQL writer.
    pub async fn create_legacy_org_setup_proposal(
        &self,
        actor: UserId,
        command: CreateOrgChange,
    ) -> Result<(OrgChangeDetail, bool), PgOrgChangeError> {
        self.create_inner(actor, command, true).await
    }

    async fn create_inner(
        &self,
        actor: UserId,
        command: CreateOrgChange,
        validate_legacy_targets: bool,
    ) -> Result<(OrgChangeDetail, bool), PgOrgChangeError> {
        command.target.validate()?;
        validate_proposal(&command.proposal)?;
        bounded_text(&command.reason, "reason", 4000)?;
        let key = command.idempotency_key.trim();
        // Characters, not bytes — the DB CHECK is char_length(); a byte count
        // would let a short multibyte key through to a raw CHECK violation.
        let key_chars = key.chars().count();
        if !(16..=200).contains(&key_chars) {
            return Err(
                KernelError::validation("Idempotency-Key must be 16..200 characters").into(),
            );
        }
        if command.effective_date < today_kst() {
            return Err(
                KernelError::validation("effectiveDate must be today or later (KST)").into(),
            );
        }
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        let key = key.to_owned();
        let fp = fingerprint(&command.fingerprint_input);
        let proposal_json = serde_json::to_value(&command.proposal)
            .map_err(|_| KernelError::internal("proposal serialization failed"))?;

        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                if let Some(row) = sqlx::query(
                    "SELECT id, request_fingerprint FROM org_change_requests \
                     WHERE idempotency_key = $1",
                )
                .bind(&key)
                .fetch_optional(tx.as_mut())
                .await?
                {
                    let prior: String = row.try_get("request_fingerprint")?;
                    if prior != fp {
                        return Err(KernelError::conflict(
                            "idempotency key was reused with a different request",
                        )
                        .into());
                    }
                    let id: Uuid = row.try_get("id")?;
                    let detail = require_detail(tx, id).await?;
                    return Ok(((detail, true), vec![]));
                }

                if validate_legacy_targets {
                    validate_legacy_org_setup_targets(tx, &command.proposal).await?;
                }

                if let Some(supersedes) = command.supersedes_id {
                    let status: Option<String> = sqlx::query_scalar(
                        "SELECT status FROM org_change_requests WHERE id = $1",
                    )
                    .bind(supersedes)
                    .fetch_optional(tx.as_mut())
                    .await?;
                    match status.as_deref() {
                        None => {
                            return Err(KernelError::not_found(
                                "superseded org-change request was not found",
                            )
                            .into());
                        }
                        Some("REJECTED") => {}
                        Some(_) => {
                            return Err(KernelError::conflict(
                                "only a REJECTED request can be superseded by a revision",
                            )
                            .into());
                        }
                    }
                }

                let year = today_kst().year();
                let prefix = format!("OC-{year}-");
                // Numeric max, not lexicographic: 'OC-2026-9999' > 'OC-2026-10000'
                // as text, which would repeat 10000 forever (permanent 409s).
                let last: Option<i64> = sqlx::query_scalar(
                    "SELECT max((substring(code FROM 9))::bigint) \
                     FROM org_change_requests WHERE code LIKE $1",
                )
                .bind(format!("{prefix}%"))
                .fetch_one(tx.as_mut())
                .await?;
                let next = last.unwrap_or(0) + 1;
                let code = format!("{prefix}{next:04}");

                let id = Uuid::new_v4();
                sqlx::query(
                    "INSERT INTO org_change_requests \
                     (id, org_id, code, kind, status, target_kind, target_ref, target_label, \
                      effective_date, reason, proposal, supersedes_id, drafted_by, \
                      idempotency_key, request_fingerprint, created_at, updated_at) \
                     VALUES ($1, $2, $3, $4, 'DRAFT', $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $15)",
                )
                .bind(id)
                .bind(*org.as_uuid())
                .bind(&code)
                .bind(command.kind.as_db())
                .bind(command.target.kind.as_db())
                .bind(&command.target.target_ref)
                .bind(&command.target.label)
                .bind(command.effective_date)
                .bind(&command.reason)
                .bind(&proposal_json)
                .bind(command.supersedes_id)
                .bind(*actor.as_uuid())
                .bind(&key)
                .bind(&fp)
                .bind(now)
                .execute(tx.as_mut())
                .await?;

                insert_event(
                    tx,
                    org,
                    id,
                    actor,
                    "create",
                    None,
                    Some(OrgChangeStatus::Draft),
                    Some(&command.reason),
                    now,
                )
                .await?;
                let detail = require_detail(tx, id).await?;
                Ok((
                    (detail, false),
                    vec![audit(
                        org,
                        actor,
                        "org_change.create",
                        "org_change_request",
                        id.to_string(),
                        now,
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn update_draft(
        &self,
        actor: UserId,
        id: Uuid,
        patch: DraftPatch,
    ) -> Result<OrgChangeDetail, PgOrgChangeError> {
        if let Some(reason) = &patch.reason {
            bounded_text(reason, "reason", 4000)?;
        }
        if let Some(proposal) = &patch.proposal {
            validate_proposal(proposal)?;
        }
        if let Some(date) = patch.effective_date
            && date < today_kst()
        {
            return Err(
                KernelError::validation("effectiveDate must be today or later (KST)").into(),
            );
        }
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();

        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let request = lock_request(tx, id).await?;
                if !request.status.is_draft_editable() {
                    return Err(KernelError::conflict(
                        "only a DRAFT or PRECHECKED request can be edited",
                    )
                    .into());
                }
                let proposal_json = match &patch.proposal {
                    Some(proposal) => Some(
                        serde_json::to_value(proposal)
                            .map_err(|_| KernelError::internal("proposal serialization failed"))?,
                    ),
                    None => None,
                };
                sqlx::query(
                    "UPDATE org_change_requests SET \
                     kind = COALESCE($2, kind), \
                     effective_date = COALESCE($3, effective_date), \
                     reason = COALESCE($4, reason), \
                     proposal = COALESCE($5, proposal), \
                     status = 'DRAFT', updated_at = $6 \
                     WHERE id = $1",
                )
                .bind(id)
                .bind(patch.kind.map(OrgChangeKind::as_db))
                .bind(patch.effective_date)
                .bind(patch.reason.as_deref())
                .bind(proposal_json)
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                insert_event(
                    tx,
                    org,
                    id,
                    actor,
                    "draft.update",
                    Some(request.status),
                    Some(OrgChangeStatus::Draft),
                    None,
                    now,
                )
                .await?;
                let detail = require_detail(tx, id).await?;
                Ok((
                    detail,
                    vec![audit(
                        org,
                        actor,
                        "org_change.draft.update",
                        "org_change_request",
                        id.to_string(),
                        now,
                    )?],
                ))
            })
        })
        .await
    }

    /// Preflight is a READ. It computes the receipt and returns it, and it
    /// persists NOTHING — no `PRECHECKED` status flip, no `org_change_events`
    /// row, no audit row, no column rewrite. `submit` recomputes the same
    /// receipt inside its own transaction and is the only writer, so there is
    /// no stored receipt that can disagree with the state at submit time.
    ///
    /// The request row is still read under a `SELECT ... FOR UPDATE` so the
    /// receipt is computed against a state no concurrent draft edit can shift
    /// mid-scan; the lock is released by the read transaction's commit.
    pub async fn preflight(
        &self,
        _actor: UserId,
        id: Uuid,
    ) -> Result<OrgChangeDetail, PgOrgChangeError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_org_conn(&self.pool, org, |tx| {
            Box::pin(async move {
                let request = lock_request(tx, id).await?;
                if !request.status.is_draft_editable() {
                    return Err(KernelError::conflict(
                        "preflight runs only on a DRAFT or PRECHECKED request",
                    )
                    .into());
                }
                let report = compute_preflight(
                    tx,
                    request.kind,
                    &request.target,
                    &request.proposal,
                    request.effective_date,
                    now,
                )
                .await?;
                let (site_count, team_count) = proposal_stats(&request.target, &request.proposal);
                let mut detail = require_detail(tx, id).await?;
                // The receipt and the counts derived from it are reported, not
                // stored; the persisted columns keep whatever the last WRITE
                // (create/update_draft/submit) left there.
                detail.summary.headcount = report.headcount;
                detail.summary.site_count = site_count;
                detail.summary.team_count = team_count;
                detail.preflight = Some(report);
                Ok(detail)
            })
        })
        .await
    }

    pub async fn submit(
        &self,
        actor: UserId,
        id: Uuid,
    ) -> Result<OrgChangeDetail, PgOrgChangeError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let request = lock_request(tx, id).await?;
                // Preflight persists nothing, so a request now reaches submit
                // as DRAFT. Rows written before that change are PRECHECKED on
                // disk and must keep working: both are accepted and the
                // receipt is recomputed below either way.
                if !request.status.is_draft_editable() {
                    return Err(KernelError::conflict(
                        "submit requires a DRAFT or PRECHECKED request",
                    )
                    .into());
                }
                request
                    .status
                    .can_transition_to(OrgChangeStatus::InApproval)?;
                // Fresh, in-transaction recheck: a stored receipt can go stale
                // between preflight and submit; the current state decides.
                let report = compute_preflight(
                    tx,
                    request.kind,
                    &request.target,
                    &request.proposal,
                    request.effective_date,
                    now,
                )
                .await?;
                if !report.blockers.is_empty() {
                    return Err(KernelError::conflict(
                        "preflight blockers are present; resolve them and re-run preflight",
                    )
                    .into());
                }
                let (site_count, team_count) = proposal_stats(&request.target, &request.proposal);
                store_preflight(
                    tx,
                    id,
                    &report,
                    OrgChangeStatus::InApproval,
                    site_count,
                    team_count,
                    now,
                )
                .await?;
                for (index, role) in ApprovalRoleKey::ORDER.iter().enumerate() {
                    sqlx::query(
                        "INSERT INTO org_change_approval_steps \
                         (org_id, request_id, step_order, role_key) VALUES ($1, $2, $3, $4)",
                    )
                    .bind(*org.as_uuid())
                    .bind(id)
                    .bind(i16::try_from(index + 1).unwrap_or(i16::MAX))
                    .bind(role.as_db())
                    .execute(tx.as_mut())
                    .await?;
                }
                insert_event(
                    tx,
                    org,
                    id,
                    actor,
                    "submit",
                    Some(request.status),
                    Some(OrgChangeStatus::InApproval),
                    None,
                    now,
                )
                .await?;
                let detail = require_detail(tx, id).await?;
                Ok((
                    detail,
                    vec![audit(
                        org,
                        actor,
                        "org_change.submit",
                        "org_change_request",
                        id.to_string(),
                        now,
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn decide_step(
        &self,
        actor: UserId,
        id: Uuid,
        step_id: Uuid,
        approved: bool,
        memo: Option<String>,
    ) -> Result<OrgChangeDetail, PgOrgChangeError> {
        if let Some(memo) = &memo {
            bounded_text(memo, "memo", 2000)?;
        }
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let request = lock_request(tx, id).await?;
                if request.status != OrgChangeStatus::InApproval {
                    return Err(KernelError::conflict(
                        "approval decisions apply only while the request is IN_APPROVAL",
                    )
                    .into());
                }
                if request.drafted_by == *actor.as_uuid() {
                    return Err(KernelError::conflict(
                        "기안자는 자신의 조직 개편을 승인할 수 없습니다.",
                    )
                    .into());
                }
                let step = sqlx::query(
                    "SELECT step_order, role_key, decision FROM org_change_approval_steps \
                     WHERE id = $1 AND request_id = $2 FOR UPDATE",
                )
                .bind(step_id)
                .bind(id)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("approval step was not found"))?;
                let step_order: i16 = step.try_get("step_order")?;
                let role_key: String = step.try_get("role_key")?;
                let decision =
                    StepDecision::from_db(step.try_get::<String, _>("decision")?.as_str())?;
                if decision != StepDecision::Pending {
                    return Err(KernelError::conflict("approval step is already decided").into());
                }
                let lowest_pending: Option<i16> = sqlx::query_scalar(
                    "SELECT min(step_order) FROM org_change_approval_steps \
                     WHERE request_id = $1 AND decision = 'PENDING'",
                )
                .bind(id)
                .fetch_one(tx.as_mut())
                .await?;
                if lowest_pending != Some(step_order) {
                    return Err(
                        KernelError::conflict("approval steps must be decided in order").into(),
                    );
                }

                // Record the decision through gov_approvals so the DB-level
                // approver <> requester CHECK is the second SoD net.
                sqlx::query(
                    "INSERT INTO gov_approvals \
                     (org_id, request_ref, kind, requested_by, approver_id, decision, decided_at) \
                     VALUES ($1, $2, 'org_change_step', $3, $4, $5, $6)",
                )
                .bind(*org.as_uuid())
                .bind(step_id)
                .bind(request.drafted_by)
                .bind(*actor.as_uuid())
                .bind(if approved { "approved" } else { "rejected" })
                .bind(now)
                .execute(tx.as_mut())
                .await?;

                let next_decision = if approved {
                    StepDecision::Approved
                } else {
                    StepDecision::Rejected
                };
                sqlx::query(
                    "UPDATE org_change_approval_steps SET decision = $2, decided_by = $3, \
                     decided_at = $4, memo = $5 WHERE id = $1",
                )
                .bind(step_id)
                .bind(next_decision.as_db())
                .bind(*actor.as_uuid())
                .bind(now)
                .bind(memo.as_deref())
                .execute(tx.as_mut())
                .await?;

                let next_status = if approved {
                    let pending: i64 = sqlx::query_scalar(
                        "SELECT count(*) FROM org_change_approval_steps \
                         WHERE request_id = $1 AND decision = 'PENDING'",
                    )
                    .bind(id)
                    .fetch_one(tx.as_mut())
                    .await?;
                    if pending == 0 {
                        Some(OrgChangeStatus::Approved)
                    } else {
                        None
                    }
                } else {
                    Some(OrgChangeStatus::Rejected)
                };
                if let Some(next) = next_status {
                    request.status.can_transition_to(next)?;
                    sqlx::query(
                        "UPDATE org_change_requests SET status = $2, updated_at = $3 WHERE id = $1",
                    )
                    .bind(id)
                    .bind(next.as_db())
                    .bind(now)
                    .execute(tx.as_mut())
                    .await?;
                }
                insert_event(
                    tx,
                    org,
                    id,
                    actor,
                    "step.decide",
                    Some(request.status),
                    next_status,
                    Some(&format!("{role_key}:{}", next_decision.as_db())),
                    now,
                )
                .await?;
                let detail = require_detail(tx, id).await?;
                Ok((
                    detail,
                    vec![audit(
                        org,
                        actor,
                        "org_change.step.decide",
                        "org_change_approval_step",
                        step_id.to_string(),
                        now,
                    )?],
                ))
            })
        })
        .await
    }

    /// Commit the §3.10-⑥ detection record for a freeze-window refusal.
    ///
    /// The refused business transaction rolled back and took every audit row
    /// inside it with it, so the attempt is recorded in its own transaction —
    /// otherwise a blocked apply would leave no trace that it was tried.
    /// Anything but a `Frozen` outcome passes through untouched.
    async fn record_freeze_refusal(
        &self,
        org: OrgId,
        actor: UserId,
        id: Uuid,
        action: &str,
        outcome: Result<OrgChangeDetail, PgOrgChangeError>,
    ) -> Result<OrgChangeDetail, PgOrgChangeError> {
        let Err(PgOrgChangeError::Frozen(refusal)) = &outcome else {
            return outcome;
        };
        let event = audit(
            org,
            actor,
            action,
            "org_change_request",
            id.to_string(),
            OffsetDateTime::now_utc(),
        )?
        .with_classification(AuditClassification {
            badges: None,
            anomaly: Some(true),
            reason: Some(refusal.message.clone()),
        });
        with_audit(&self.pool, event, |_tx| {
            Box::pin(async { Ok::<(), PgOrgChangeError>(()) })
        })
        .await?;
        outcome
    }

    pub async fn effectuate(
        &self,
        actor: UserId,
        id: Uuid,
    ) -> Result<OrgChangeDetail, PgOrgChangeError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        let employment_transfer = self.employment_transfer();
        let outcome = with_audits(&self.pool, org, move |tx| {
            Box::pin(async move {
                let request = lock_request(tx, id).await?;
                if request.status != OrgChangeStatus::Approved {
                    return Err(KernelError::conflict(
                        "only an APPROVED request can be effectuated",
                    )
                    .into());
                }
                if today_kst() < request.effective_date {
                    return Err(KernelError::conflict("발효일 이전에는 적용할 수 없습니다.").into());
                }
                let mut audits = Vec::new();
                let next = match request.kind {
                    OrgChangeKind::New | OrgChangeKind::Reorg => {
                        audits = apply_ops(
                            tx,
                            org,
                            actor,
                            id,
                            &request.proposal,
                            request.effective_date,
                            now,
                            employment_transfer.as_deref(),
                        )
                        .await?;
                        OrgChangeStatus::Applied
                    }
                    OrgChangeKind::Dissolve => {
                        // Dissolve does not deactivate yet: it opens settlement;
                        // the proposal ops replay at archive after every item
                        // is settled (참조 무결성). Opening settlement is itself
                        // a live org change, so it takes the same freeze gate
                        // `apply_ops` applies to the other kinds.
                        assert_change_window_open(tx, request.effective_date).await?;
                        for key in SettlementKey::ALL {
                            sqlx::query(
                                "INSERT INTO org_change_settlement_items \
                                 (org_id, request_id, item_key) VALUES ($1, $2, $3)",
                            )
                            .bind(*org.as_uuid())
                            .bind(id)
                            .bind(key.as_db())
                            .execute(tx.as_mut())
                            .await?;
                        }
                        OrgChangeStatus::Settling
                    }
                };
                request.status.can_transition_to(next)?;
                sqlx::query(
                    "UPDATE org_change_requests SET status = $2, updated_at = $3 WHERE id = $1",
                )
                .bind(id)
                .bind(next.as_db())
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                insert_event(
                    tx,
                    org,
                    id,
                    actor,
                    "effectuate",
                    Some(OrgChangeStatus::Approved),
                    Some(next),
                    Some(&format!("ops={}", request.proposal.len())),
                    now,
                )
                .await?;
                audits.push(audit(
                    org,
                    actor,
                    "org_change.effectuate",
                    "org_change_request",
                    id.to_string(),
                    now,
                )?);
                let detail = require_detail(tx, id).await?;
                Ok((detail, audits))
            })
        })
        .await;
        self.record_freeze_refusal(org, actor, id, "org_change.effectuate.refused", outcome)
            .await
    }

    pub async fn complete_settlement_item(
        &self,
        actor: UserId,
        id: Uuid,
        item_id: Uuid,
        memo: Option<String>,
    ) -> Result<OrgChangeDetail, PgOrgChangeError> {
        if let Some(memo) = &memo {
            bounded_text(memo, "memo", 2000)?;
        }
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let request = lock_request(tx, id).await?;
                if request.status != OrgChangeStatus::Settling {
                    return Err(KernelError::conflict(
                        "settlement items complete only while the request is SETTLING",
                    )
                    .into());
                }
                let item = sqlx::query(
                    "SELECT item_key, done FROM org_change_settlement_items \
                     WHERE id = $1 AND request_id = $2 FOR UPDATE",
                )
                .bind(item_id)
                .bind(id)
                .fetch_optional(tx.as_mut())
                .await?
                .ok_or_else(|| KernelError::not_found("settlement item was not found"))?;
                if item.try_get::<bool, _>("done")? {
                    return Err(KernelError::conflict("settlement item is already complete").into());
                }
                let item_key: String = item.try_get("item_key")?;
                sqlx::query(
                    "UPDATE org_change_settlement_items SET done = true, done_by = $2, \
                     done_at = $3, memo = $4 WHERE id = $1",
                )
                .bind(item_id)
                .bind(*actor.as_uuid())
                .bind(now)
                .bind(memo.as_deref())
                .execute(tx.as_mut())
                .await?;
                sqlx::query("UPDATE org_change_requests SET updated_at = $2 WHERE id = $1")
                    .bind(id)
                    .bind(now)
                    .execute(tx.as_mut())
                    .await?;
                insert_event(
                    tx,
                    org,
                    id,
                    actor,
                    "settlement.complete",
                    None,
                    None,
                    Some(&item_key),
                    now,
                )
                .await?;
                let detail = require_detail(tx, id).await?;
                Ok((
                    detail,
                    vec![audit(
                        org,
                        actor,
                        "org_change.settlement.complete",
                        "org_change_settlement_item",
                        item_id.to_string(),
                        now,
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn archive(
        &self,
        actor: UserId,
        id: Uuid,
    ) -> Result<OrgChangeDetail, PgOrgChangeError> {
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        let employment_transfer = self.employment_transfer();
        let outcome = with_audits(&self.pool, org, move |tx| {
            Box::pin(async move {
                let request = lock_request(tx, id).await?;
                if request.status != OrgChangeStatus::Settling {
                    return Err(
                        KernelError::conflict("only a SETTLING request can be archived").into(),
                    );
                }
                request
                    .status
                    .can_transition_to(OrgChangeStatus::Archived)?;
                let unsettled: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM org_change_settlement_items \
                     WHERE request_id = $1 AND done = false",
                )
                .bind(id)
                .fetch_one(tx.as_mut())
                .await?;
                if unsettled > 0 {
                    return Err(KernelError::conflict(
                        "unsettled items remain; complete every settlement item first",
                    )
                    .into());
                }
                // The deferred dissolve ops run now, inside this one tx; the
                // referential guards re-verify (settlement must have actually
                // cleared the dependents, or this rolls back with a conflict).
                let mut audits = apply_ops(
                    tx,
                    org,
                    actor,
                    id,
                    &request.proposal,
                    request.effective_date,
                    now,
                    employment_transfer.as_deref(),
                )
                .await?;
                sqlx::query(
                    "UPDATE org_change_requests SET status = 'ARCHIVED', updated_at = $2 \
                     WHERE id = $1",
                )
                .bind(id)
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                insert_event(
                    tx,
                    org,
                    id,
                    actor,
                    "archive",
                    Some(OrgChangeStatus::Settling),
                    Some(OrgChangeStatus::Archived),
                    None,
                    now,
                )
                .await?;
                audits.push(audit(
                    org,
                    actor,
                    "org_change.archive",
                    "org_change_request",
                    id.to_string(),
                    now,
                )?);
                let detail = require_detail(tx, id).await?;
                Ok((detail, audits))
            })
        })
        .await;
        self.record_freeze_refusal(org, actor, id, "org_change.archive.refused", outcome)
            .await
    }

    pub async fn cancel(
        &self,
        actor: UserId,
        id: Uuid,
        reason: String,
    ) -> Result<OrgChangeDetail, PgOrgChangeError> {
        bounded_text(&reason, "reason", 4000)?;
        let org = current_org().map_err(KernelError::from)?;
        let now = OffsetDateTime::now_utc();
        with_audits(&self.pool, org, |tx| {
            Box::pin(async move {
                let request = lock_request(tx, id).await?;
                request
                    .status
                    .can_transition_to(OrgChangeStatus::Cancelled)?;
                sqlx::query(
                    "UPDATE org_change_requests SET status = 'CANCELLED', updated_at = $2 \
                     WHERE id = $1",
                )
                .bind(id)
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                insert_event(
                    tx,
                    org,
                    id,
                    actor,
                    "cancel",
                    Some(request.status),
                    Some(OrgChangeStatus::Cancelled),
                    Some(&reason),
                    now,
                )
                .await?;
                let detail = require_detail(tx, id).await?;
                Ok((
                    detail,
                    vec![audit(
                        org,
                        actor,
                        "org_change.cancel",
                        "org_change_request",
                        id.to_string(),
                        now,
                    )?],
                ))
            })
        })
        .await
    }

    pub async fn list(&self, filter: ListFilter) -> Result<OrgChangePage, PgOrgChangeError> {
        let org = current_org().map_err(KernelError::from)?;
        let limit = filter.limit.unwrap_or(50).clamp(1, 200);
        let offset = filter.offset.unwrap_or(0).clamp(0, 100_000);
        let status = filter.status.map(OrgChangeStatus::as_db);
        let kind = filter.kind.map(OrgChangeKind::as_db);
        with_org_conn(&self.pool, org, |tx| {
            Box::pin(async move {
                let rows = sqlx::query(SELECT_REQUEST_PAGE)
                    .bind(status)
                    .bind(kind)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(tx.as_mut())
                    .await?;
                let total: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM org_change_requests \
                     WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR kind = $2)",
                )
                .bind(status)
                .bind(kind)
                .fetch_one(tx.as_mut())
                .await?;
                let mut items = Vec::with_capacity(rows.len());
                for row in &rows {
                    items.push(summary_from_row(row)?);
                }
                Ok(OrgChangePage { items, total })
            })
        })
        .await
    }

    pub async fn get(&self, id: Uuid) -> Result<OrgChangeDetail, PgOrgChangeError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn(&self.pool, org, |tx| {
            Box::pin(async move {
                load_detail(tx, id)
                    .await?
                    // RLS conceals other tenants' rows: out-of-scope is
                    // indistinguishable from absent (deny-by-omission).
                    .ok_or_else(|| {
                        KernelError::not_found("org-change request was not found").into()
                    })
            })
        })
        .await
    }

    /// The 법인 list = active member orgs of every group the actor holds a
    /// live grant for (fail-closed empty; design gap-analysis §2).
    ///
    /// L5-ORG: each row also carries Company/OrgUnit canonical reference fields
    /// on the preserved `/api/v1/org-entities` namespace (no parallel `/companies`).
    pub async fn org_entities(
        &self,
        actor: UserId,
    ) -> Result<Vec<OrgEntitySummary>, PgOrgChangeError> {
        let group_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT grants.group_id \
             FROM group_role_grants_for_user($1) AS grants \
             JOIN groups g ON g.id = grants.group_id \
             WHERE g.status = 'ACTIVE' \
             ORDER BY grants.group_id",
        )
        .bind(*actor.as_uuid())
        // rls-arming: ok identity-only SECURITY DEFINER grants resolver + safe groups metadata
        .fetch_all(&self.pool)
        .await?;
        let mut by_org = std::collections::BTreeMap::new();
        for group_id in group_ids {
            for member in
                console_platform_group::group_member_orgs(&self.pool, group_id, actor).await?
            {
                by_org.insert(
                    *member.org_id.as_uuid(),
                    (member.slug, member.name, member.status, member.org_id),
                );
            }
        }
        let mut out = Vec::with_capacity(by_org.len());
        for (org_uuid, (slug, name, status, org_id)) in by_org {
            // company_revisions / org_unit_source_bindings / regions / branches
            // are FORCE RLS — arm app.current_org per member tenant (not the
            // bare pool used for the identity-only grants resolver above).
            let (company_resolved, org_units) = with_org_conn(&self.pool, org_id, |tx| {
                Box::pin(async move {
                    // SELECT-only: writer-ownership does not charge reads.
                    // Kept local so this adapter never depends on the Company
                    // owner adapter (adapter→adapter is illegal).
                    let company_resolved: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM company_revisions WHERE org_id = $1)",
                    )
                    .bind(*org_id.as_uuid())
                    .fetch_one(tx.as_mut())
                    .await?;
                    let org_units = load_org_unit_references_tx(tx, org_id).await?;
                    Ok::<_, PgOrgChangeError>((company_resolved, org_units))
                })
            })
            .await?;
            out.push(OrgEntitySummary {
                org_id: org_uuid,
                slug,
                name,
                status,
                company_id: org_uuid,
                company_resolution_status: if company_resolved {
                    CanonicalResolutionStatus::Resolved
                } else {
                    CanonicalResolutionStatus::Unbound
                },
                org_units,
            });
        }
        Ok(out)
    }
}

/// Region/branch rows for a tenant, LEFT JOIN'd to `org_unit_source_bindings`.
/// Unbound legacy rows stay visible with `org_unit_id: None` (gap is observable).
/// Caller must already have armed `app.current_org` for `org_id`.
async fn load_org_unit_references_tx(
    tx: Tx<'_, '_>,
    org_id: OrgId,
) -> Result<Vec<OrgUnitReference>, PgOrgChangeError> {
    let org = *org_id.as_uuid();
    let rows = sqlx::query(
        "SELECT source_kind, source_id, org_unit_id, resolution FROM ( \
             SELECT 'region'::text AS source_kind, \
                    r.id::text AS source_id, \
                    b.org_unit_id AS org_unit_id, \
                    CASE WHEN b.org_unit_id IS NULL THEN 'UNBOUND' ELSE 'RESOLVED' END AS resolution \
             FROM regions r \
             LEFT JOIN org_unit_source_bindings b \
               ON b.org_id = r.org_id \
              AND b.source_kind = 'region' \
              AND b.source_id = r.id::text \
             WHERE r.org_id = $1 \
             UNION ALL \
             SELECT 'branch'::text, \
                    br.id::text, \
                    b.org_unit_id, \
                    CASE WHEN b.org_unit_id IS NULL THEN 'UNBOUND' ELSE 'RESOLVED' END \
             FROM branches br \
             LEFT JOIN org_unit_source_bindings b \
               ON b.org_id = br.org_id \
              AND b.source_kind = 'branch' \
              AND b.source_id = br.id::text \
             WHERE br.org_id = $1 \
         ) ref \
         ORDER BY source_kind, source_id",
    )
    .bind(org)
    .fetch_all(tx.as_mut())
    .await?;

    let mut refs = Vec::with_capacity(rows.len());
    for row in rows {
        let status: String = row.get("resolution");
        let resolution_status = match status.as_str() {
            "RESOLVED" => CanonicalResolutionStatus::Resolved,
            _ => CanonicalResolutionStatus::Unbound,
        };
        refs.push(OrgUnitReference {
            org_unit_id: row.get("org_unit_id"),
            source_kind: row.get("source_kind"),
            source_id: row.get("source_id"),
            resolution_status,
        });
    }
    Ok(refs)
}
