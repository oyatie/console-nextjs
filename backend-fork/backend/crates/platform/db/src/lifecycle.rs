//! Object lifecycle engine MVP — a generic per-object FSM keyed by
//! `(object_type, object_id)` with an append-only transition log.
//!
//! State sets and legal transitions live in the global seeded
//! `lifecycle_transition_rules` table (migration 0107 seeds the `document`
//! chain: draft → submitted → approved → active → revised → archived →
//! disposed). A transition is refused unless a matching rule row exists, and
//! the terminal `disposed` transition is additionally gated on legal hold and
//! retention: it fails closed while `legal_hold` is set or `retention_until`
//! lies in the future.
//!
//! Approval transitions are additionally gated by the maker–checker (SoD)
//! rule in [`CHECKER_TRANSITIONS`]: this is the chokepoint every domain routes
//! through, so the four-eyes control lives here rather than in each caller.
//!
//! These are transaction-level primitives: the REST layer wraps them in
//! `with_audit` (transitions and holds are mutations) or `with_org_conn`
//! (reads), so RLS tenancy and audit atomicity come from the caller's
//! transaction.

use console_kernel_core::KernelError;
use sqlx::{Postgres, Row, Transaction};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::error::DbError;

/// The state a brand-new lifecycle row starts in.
pub const INITIAL_STATE: &str = "draft";
/// The terminal state guarded by legal hold / retention.
pub const DISPOSED_STATE: &str = "disposed";

/// Maker–checker (SoD) transitions: `(object_type, from_state, to_state)`
/// triples whose actor MUST differ from the maker who moved the object into
/// `from_state`.
///
/// **Requirement.** DESIGN §3.9.1 «maker-checker · 직무분리(SoD, SOX): 기안자 ≠
/// 승인자. 승인 매트릭스·전결규정(DoA)», §3.10 ③ «기안자≠검토자, 승인 전 제2자
/// 검토 … 자기검토 시스템 차단» and ④ 결재(SoD·DoA), §3.9.3 anti-pattern
/// «기안자=승인자(SoD 위반)», HANDOFF §15 «maker-checker/SoD(SOX): 기안자≠승인자»
/// and §16 «approval(SoD/DoA) … fail-closed».
///
/// The set is deliberately **narrow — only the approval step of each chain**.
/// The design names 기안자≠승인자, not "every step needs two people": authoring
/// (draft→submitted, active→revised), publication of an already-approved object
/// (approved→active, finalized→implemented — DESIGN §3.9.0 whitelist ④,
/// 게시 단계 권한 + 감사), scheduled retirement (retiring→retired, 폐지일 preset)
/// and retention-gated disposal are legitimate single-actor acts. A blanket
/// same-actor ban would break them.
///
/// - `document` (0107): draft→submitted→**approved**→active→revised→archived→disposed.
/// - `benefit_catalog_item` (0157/0175; DESIGN §3.9 복리후생 정책 수명주기
///   «초안(draft) → 승인 대기(pending) → 확정(finalized) → 시행(implemented) →
///   폐지 예정(retiring) → 폐지(retired)»): draft→pending→**finalized**→…
///
/// Adding an object type to `lifecycle_transition_rules` therefore also means
/// deciding, here, which of its transitions is the 승인.
const CHECKER_TRANSITIONS: &[(&str, &str, &str)] = &[
    ("document", "submitted", "approved"),
    ("benefit_catalog_item", "pending", "finalized"),
];

/// One object's lifecycle row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LifecycleRecord {
    pub id: Uuid,
    pub object_type: String,
    pub object_id: Uuid,
    pub current_state: String,
    pub legal_hold: bool,
    pub retention_until: Option<Date>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// One append-only transition log row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LifecycleTransitionRecord {
    pub from_state: String,
    pub to_state: String,
    pub actor: Option<Uuid>,
    pub reason: String,
    pub occurred_at: OffsetDateTime,
}

fn lifecycle_from_row(row: &sqlx::postgres::PgRow) -> Result<LifecycleRecord, DbError> {
    Ok(LifecycleRecord {
        id: row.try_get("id").map_err(DbError::Sqlx)?,
        object_type: row.try_get("object_type").map_err(DbError::Sqlx)?,
        object_id: row.try_get("object_id").map_err(DbError::Sqlx)?,
        current_state: row.try_get("current_state").map_err(DbError::Sqlx)?,
        legal_hold: row.try_get("legal_hold").map_err(DbError::Sqlx)?,
        retention_until: row.try_get("retention_until").map_err(DbError::Sqlx)?,
        created_at: row.try_get("created_at").map_err(DbError::Sqlx)?,
        updated_at: row.try_get("updated_at").map_err(DbError::Sqlx)?,
    })
}

/// Fetch one lifecycle row (RLS-scoped), `None` when the object has no
/// lifecycle yet.
pub async fn get_lifecycle(
    tx: &mut Transaction<'_, Postgres>,
    object_type: &str,
    object_id: Uuid,
) -> Result<Option<LifecycleRecord>, DbError> {
    let row = sqlx::query(
        "SELECT id, object_type, object_id, current_state, legal_hold, retention_until, \
                created_at, updated_at \
         FROM object_lifecycles WHERE object_type = $1 AND object_id = $2",
    )
    .bind(object_type)
    .bind(object_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(DbError::Sqlx)?;
    row.as_ref().map(lifecycle_from_row).transpose()
}

/// Fetch the transition log for a lifecycle, newest first (RLS-scoped).
pub async fn list_transitions(
    tx: &mut Transaction<'_, Postgres>,
    lifecycle_id: Uuid,
) -> Result<Vec<LifecycleTransitionRecord>, DbError> {
    let rows = sqlx::query(
        "SELECT from_state, to_state, actor, reason, occurred_at \
         FROM object_lifecycle_transitions WHERE lifecycle_id = $1 \
         ORDER BY occurred_at DESC, id DESC",
    )
    .bind(lifecycle_id)
    .fetch_all(tx.as_mut())
    .await
    .map_err(DbError::Sqlx)?;
    rows.into_iter()
        .map(|row| {
            Ok(LifecycleTransitionRecord {
                from_state: row.try_get("from_state").map_err(DbError::Sqlx)?,
                to_state: row.try_get("to_state").map_err(DbError::Sqlx)?,
                actor: row.try_get("actor").map_err(DbError::Sqlx)?,
                reason: row.try_get("reason").map_err(DbError::Sqlx)?,
                occurred_at: row.try_get("occurred_at").map_err(DbError::Sqlx)?,
            })
        })
        .collect()
}

/// True when the seeded rule table knows `object_type` at all.
async fn object_type_known(
    tx: &mut Transaction<'_, Postgres>,
    object_type: &str,
) -> Result<bool, DbError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM lifecycle_transition_rules WHERE object_type = $1",
    )
    .bind(object_type)
    .fetch_one(tx.as_mut())
    .await
    .map_err(DbError::Sqlx)?;
    Ok(count > 0)
}

async fn transition_allowed(
    tx: &mut Transaction<'_, Postgres>,
    object_type: &str,
    from_state: &str,
    to_state: &str,
) -> Result<bool, DbError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM lifecycle_transition_rules \
         WHERE object_type = $1 AND from_state = $2 AND to_state = $3",
    )
    .bind(object_type)
    .bind(from_state)
    .bind(to_state)
    .fetch_one(tx.as_mut())
    .await
    .map_err(DbError::Sqlx)?;
    Ok(count > 0)
}

fn is_checker_transition(object_type: &str, from_state: &str, to_state: &str) -> bool {
    CHECKER_TRANSITIONS
        .iter()
        .any(|&(ot, from, to)| ot == object_type && from == from_state && to == to_state)
}

/// Require identified, distinct maker and checker at the locked approval boundary.
async fn enforce_maker_checker(
    tx: &mut Transaction<'_, Postgres>,
    current: &LifecycleRecord,
    actor: Option<Uuid>,
) -> Result<(), KernelError> {
    // The newest transition into this state owns the current proposal.
    let maker: Option<Option<Uuid>> = sqlx::query_scalar(
        "SELECT actor FROM object_lifecycle_transitions \
         WHERE lifecycle_id = $1 AND to_state = $2 \
         ORDER BY occurred_at DESC, id DESC LIMIT 1",
    )
    .bind(current.id)
    .bind(&current.current_state)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(|e| internal(DbError::Sqlx(e)))?;
    let Some(maker) = maker.flatten() else {
        return Err(KernelError::forbidden(format!(
            "{} {} 의 '{}' 기안 기록이 없어 승인할 수 없습니다",
            current.object_type, current.object_id, current.current_state
        )));
    };
    let Some(actor) = actor else {
        return Err(KernelError::forbidden("승인자를 확인할 수 없습니다"));
    };
    if maker == actor {
        return Err(KernelError::forbidden(
            "본인이 기안한 건은 승인할 수 없습니다",
        ));
    }
    Ok(())
}

/// Transition one object's lifecycle to `to_state`, enforcing the seeded
/// allowed-transition rules, the maker–checker (SoD) gate on approval
/// transitions, and the dispose gate.
///
/// - Unknown `object_type` (no rule rows) → validation error, fail closed.
/// - Missing lifecycle row → the object implicitly starts at
///   [`INITIAL_STATE`]; the row is created here iff the first transition is
///   itself legal from that state.
/// - A [`CHECKER_TRANSITIONS`] transition → refused (403) when `actor` is the
///   maker who moved the object into its current state, regardless of title,
///   or either actor cannot be identified.
/// - `disposed` → refused while `legal_hold` is set or `retention_until` is in
///   the future.
///
/// Returns the updated lifecycle row. The caller MUST run this inside a
/// `with_audit` transaction so the transition, its log row, and the audit
/// event commit atomically.
#[allow(clippy::too_many_arguments)] // one call site per REST handler; a params struct adds nothing
pub async fn transition_lifecycle(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    object_type: &str,
    object_id: Uuid,
    to_state: &str,
    actor: Option<Uuid>,
    reason: &str,
    today: Date,
) -> Result<LifecycleRecord, KernelError> {
    if reason.trim().is_empty() {
        return Err(KernelError::validation(
            "transition reason must not be blank",
        ));
    }
    if !object_type_known(tx, object_type).await.map_err(internal)? {
        return Err(KernelError::validation(format!(
            "object type '{object_type}' has no lifecycle rules"
        )));
    }

    // Lock (or create) the lifecycle row.
    let existing = sqlx::query(
        "SELECT id, object_type, object_id, current_state, legal_hold, retention_until, \
                created_at, updated_at \
         FROM object_lifecycles WHERE object_type = $1 AND object_id = $2 FOR UPDATE",
    )
    .bind(object_type)
    .bind(object_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(|e| internal(DbError::Sqlx(e)))?;

    let current = match &existing {
        Some(row) => lifecycle_from_row(row).map_err(internal)?,
        None => {
            // Implicit registration: the object starts at INITIAL_STATE. Only
            // materialize the row when the requested first transition is legal.
            if !transition_allowed(tx, object_type, INITIAL_STATE, to_state)
                .await
                .map_err(internal)?
            {
                return Err(KernelError::invalid_transition(format!(
                    "invalid lifecycle transition {INITIAL_STATE} -> {to_state} for {object_type}"
                )));
            }
            let row = sqlx::query(
                "INSERT INTO object_lifecycles (org_id, object_type, object_id, current_state) \
                 VALUES ($1, $2, $3, $4) \
                 RETURNING id, object_type, object_id, current_state, legal_hold, \
                           retention_until, created_at, updated_at",
            )
            .bind(org_id)
            .bind(object_type)
            .bind(object_id)
            .bind(INITIAL_STATE)
            .fetch_one(tx.as_mut())
            .await
            .map_err(|e| internal(DbError::Sqlx(e)))?;
            lifecycle_from_row(&row).map_err(internal)?
        }
    };

    if existing.is_some()
        && !transition_allowed(tx, object_type, &current.current_state, to_state)
            .await
            .map_err(internal)?
    {
        return Err(KernelError::invalid_transition(format!(
            "invalid lifecycle transition {} -> {to_state} for {object_type}",
            current.current_state
        )));
    }

    // Maker–checker (SoD): the approver must not be the 기안자.
    if is_checker_transition(object_type, &current.current_state, to_state) {
        enforce_maker_checker(tx, &current, actor).await?;
    }

    // Dispose gate: legal hold and retention fail closed.
    if to_state == DISPOSED_STATE {
        if current.legal_hold {
            return Err(KernelError::conflict(format!(
                "{object_type} {object_id} is under legal hold; dispose refused"
            )));
        }
        if let Some(retention_until) = current.retention_until
            && retention_until > today
        {
            return Err(KernelError::conflict(format!(
                "{object_type} {object_id} is retained until {retention_until}; dispose refused"
            )));
        }
    }

    let updated = sqlx::query(
        "UPDATE object_lifecycles SET current_state = $2, updated_at = now() WHERE id = $1 \
         RETURNING id, object_type, object_id, current_state, legal_hold, retention_until, \
                   created_at, updated_at",
    )
    .bind(current.id)
    .bind(to_state)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|e| internal(DbError::Sqlx(e)))?;
    let updated = lifecycle_from_row(&updated).map_err(internal)?;

    sqlx::query(
        "INSERT INTO object_lifecycle_transitions \
             (org_id, lifecycle_id, from_state, to_state, actor, reason) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(org_id)
    .bind(current.id)
    .bind(&current.current_state)
    .bind(to_state)
    .bind(actor)
    .bind(reason.trim())
    .execute(tx.as_mut())
    .await
    .map_err(|e| internal(DbError::Sqlx(e)))?;

    Ok(updated)
}

/// Set or clear the legal hold / retention deadline on one object's lifecycle.
///
/// Creates the lifecycle row at [`INITIAL_STATE`] when absent, so a hold can be
/// placed before the object ever transitions. Caller wraps in `with_audit`.
pub async fn set_lifecycle_hold(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    object_type: &str,
    object_id: Uuid,
    legal_hold: bool,
    retention_until: Option<Date>,
) -> Result<LifecycleRecord, KernelError> {
    if !object_type_known(tx, object_type).await.map_err(internal)? {
        return Err(KernelError::validation(format!(
            "object type '{object_type}' has no lifecycle rules"
        )));
    }
    let row = sqlx::query(
        "INSERT INTO object_lifecycles \
             (org_id, object_type, object_id, current_state, legal_hold, retention_until) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (org_id, object_type, object_id) DO UPDATE \
             SET legal_hold = EXCLUDED.legal_hold, \
                 retention_until = EXCLUDED.retention_until, \
                 updated_at = now() \
         RETURNING id, object_type, object_id, current_state, legal_hold, retention_until, \
                   created_at, updated_at",
    )
    .bind(org_id)
    .bind(object_type)
    .bind(object_id)
    .bind(INITIAL_STATE)
    .bind(legal_hold)
    .bind(retention_until)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|e| internal(DbError::Sqlx(e)))?;
    lifecycle_from_row(&row).map_err(internal)
}

fn internal(err: DbError) -> KernelError {
    KernelError::internal(format!("lifecycle operation failed: {err}"))
}
