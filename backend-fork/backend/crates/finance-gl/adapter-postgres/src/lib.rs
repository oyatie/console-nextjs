//! Postgres finance-GL (전표) adapter.
//!
//! Every mutation runs through `with_audit`/`with_audits`, which arms
//! `app.current_org` before the closure, so RLS scopes every read and write to
//! the tenant; every read runs through `with_org_conn` for the same reason. The
//! FSM balance gate and posted immutability are enforced BOTH here (use-case
//! gate, fail-closed) and by the DB triggers in migration 0160 (defense in
//! depth). Vouchers are never hard-deleted and posted lines are never mutated —
//! a reversal creates a NEW contra voucher.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_finance_gl_application::{
    AccountDrillEntry, CreateVoucherDraftCommand, CreateVoucherDraftFromSourceCommand,
    ReverseVoucherCommand, VoucherLineInput, VoucherLineSummary, VoucherSourceRef, VoucherSummary,
    VoucherTransitionCommand, voucher_audit_event,
};
use console_finance_gl_domain::{
    DebitCredit, VoucherId, VoucherStatus, compute_balance, ensure_balanced,
    validate_voucher_transition,
};
use console_kernel_core::{BranchId, KernelError, TraceContext, UserId};
use console_platform_db::{DbError, with_audits, with_org_conn};
use console_platform_request_context::current_org;
use sqlx::{PgPool, Postgres, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PgVoucherError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),
}

impl From<sqlx::Error> for PgVoucherError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

#[derive(Debug, Clone)]
pub struct PgVoucherStore {
    pool: PgPool,
}

impl PgVoucherStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Open a fresh draft voucher (기표). Lines may be unbalanced at draft time;
    /// the balance gate is enforced when the voucher is submitted for 차대검증.
    pub async fn create_draft(
        &self,
        command: CreateVoucherDraftCommand,
    ) -> Result<VoucherSummary, PgVoucherError> {
        // S7: a hand-keyed voucher may NOT assert a source-document linkage. Source
        // provenance is unverifiable from arbitrary caller input (a preparer could
        // fabricate "derived from approved expense NONEXISTENT"), so it is only
        // ever set by the trusted derive path [`create_draft_from_source`], which
        // is called with a source the approval engine has already resolved.
        if command.source.is_some() {
            return Err(KernelError::validation(
                "a hand-keyed voucher may not set a source reference; source \
                 provenance is recorded only by the approval-derived path",
            )
            .into());
        }
        self.insert_draft(
            command.actor,
            command.branch_id,
            &command.memo,
            None,
            &command.lines,
            None,
            "finance_gl.voucher.create",
            command.trace,
            command.occurred_at,
        )
        .await
    }

    /// Derive a draft voucher from an approved source 기안 (승인 → 전표 파생).
    /// Identical persistence to [`create_draft`], but the source ref is required
    /// so the approval chain always records voucher → source linkage for drill.
    pub async fn create_draft_from_source(
        &self,
        command: CreateVoucherDraftFromSourceCommand,
    ) -> Result<VoucherSummary, PgVoucherError> {
        self.insert_draft(
            command.actor,
            command.branch_id,
            &command.memo,
            Some(&command.source),
            &command.projected_lines,
            None,
            "finance_gl.voucher.derive_from_source",
            command.trace,
            command.occurred_at,
        )
        .await
    }

    /// 기표 → 차대검증. The balance gate: rejected (fail-closed) unless 차변 = 대변.
    pub async fn submit(
        &self,
        command: VoucherTransitionCommand,
    ) -> Result<VoucherSummary, PgVoucherError> {
        self.transition(
            command,
            VoucherStatus::BalanceChecked,
            "finance_gl.voucher.submit",
        )
        .await
    }

    /// 차대검증 → 승인.
    pub async fn approve(
        &self,
        command: VoucherTransitionCommand,
    ) -> Result<VoucherSummary, PgVoucherError> {
        self.transition(
            command,
            VoucherStatus::Approved,
            "finance_gl.voucher.approve",
        )
        .await
    }

    /// 승인 → 전기(posted). Stamps `posted_at`; lines become immutable.
    pub async fn post(
        &self,
        command: VoucherTransitionCommand,
    ) -> Result<VoucherSummary, PgVoucherError> {
        self.transition(command, VoucherStatus::Posted, "finance_gl.voucher.post")
            .await
    }

    /// 전기 → 역분개. Never mutates the posted voucher's lines — creates a linked
    /// contra voucher whose lines swap 차↔대 so the pair nets to zero, marks the
    /// original REVERSED, and returns the new contra voucher.
    pub async fn reverse(
        &self,
        command: ReverseVoucherCommand,
    ) -> Result<VoucherSummary, PgVoucherError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let contra_id = VoucherId::new();
        with_audits::<_, VoucherSummary, PgVoucherError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let original = lock_voucher(tx, command.voucher_id).await?;
                validate_voucher_transition(original.status, VoucherStatus::Reversed)?;
                // The original is POSTED, so it necessarily carries a distinct
                // approver; assert it fail-closed before deriving the contra from it.
                let Some(original_approver) = original.approved_by else {
                    return Err(KernelError::conflict(
                        "cannot reverse a voucher that has no recorded approver",
                    )
                    .into());
                };
                let lines = load_lines(tx, command.voucher_id).await?;

                // Build the contra voucher: DRAFT, swapped lines, then straight to
                // POSTED. It is balanced by construction (each side mirrors the
                // original), so it clears the balance trigger. The contra is the
                // mechanical inverse of an already-two-person-authorized voucher, so
                // it carries the ORIGINAL's preparer/approver forward — that keeps
                // the SoD invariant (approved_by <> created_by) satisfied without a
                // second approval step and without exempting contras from the CHECK.
                let contra_no = next_voucher_no(command.occurred_at);
                insert_voucher_row(
                    tx,
                    contra_id,
                    org_uuid,
                    original.branch_id,
                    &contra_no,
                    VoucherStatus::Draft,
                    &command.memo,
                    original.source.as_ref(),
                    Some(command.voucher_id),
                    original.created_by,
                    Some(original_approver),
                    None,
                    command.occurred_at,
                )
                .await?;
                for (idx, line) in lines.iter().enumerate() {
                    insert_line_row(
                        tx,
                        org_uuid,
                        contra_id,
                        i32::try_from(idx + 1).map_err(|_| {
                            KernelError::validation("voucher line count overflowed i32")
                        })?,
                        &line.account_code,
                        line.side.reversed(),
                        line.amount_won,
                        &line.memo,
                    )
                    .await?;
                }
                set_voucher_status(
                    tx,
                    contra_id,
                    VoucherStatus::Posted,
                    None,
                    Some(command.occurred_at),
                    None,
                    command.occurred_at,
                )
                .await?;

                // Mark the original REVERSED and point it at its contra.
                set_voucher_status(
                    tx,
                    command.voucher_id,
                    VoucherStatus::Reversed,
                    None,
                    None,
                    Some(contra_id),
                    command.occurred_at,
                )
                .await?;

                let reverse_event = voucher_audit_event(
                    "finance_gl.voucher.reverse",
                    command.actor,
                    original.branch_id,
                    command.voucher_id,
                    command.trace.clone(),
                    command.occurred_at,
                )?
                .with_org(org);
                let contra_event = voucher_audit_event(
                    "finance_gl.voucher.reverse_contra",
                    command.actor,
                    original.branch_id,
                    contra_id,
                    command.trace,
                    command.occurred_at,
                )?
                .with_org(org);

                let summary = load_summary(tx, contra_id).await?;
                Ok((summary, vec![reverse_event, contra_event]))
            })
        })
        .await
    }

    /// Fetch one voucher with its lines and source linkage.
    pub async fn get(&self, voucher_id: VoucherId) -> Result<VoucherSummary, PgVoucherError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, VoucherSummary, PgVoucherError>(&self.pool, org, move |tx| {
            Box::pin(async move { load_summary(tx, voucher_id).await })
        })
        .await
    }

    /// List vouchers in the tenant, most recent first, optionally narrowed to a
    /// branch and/or status.
    pub async fn list(
        &self,
        branch_id: Option<BranchId>,
        status: Option<VoucherStatus>,
    ) -> Result<Vec<VoucherSummary>, PgVoucherError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, Vec<VoucherSummary>, PgVoucherError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let ids: Vec<Uuid> = sqlx::query(
                    r#"
                    SELECT id FROM finance_gl_vouchers
                    WHERE ($1::uuid IS NULL OR branch_id = $1)
                      AND ($2::text IS NULL OR status = $2)
                    ORDER BY created_at DESC, id DESC
                    LIMIT 500
                    "#,
                )
                .bind(branch_id.map(|b| *b.as_uuid()))
                .bind(status.map(|s| s.as_db_str()))
                .fetch_all(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?
                .into_iter()
                .map(|row| row.get::<Uuid, _>("id"))
                .collect();

                let mut out = Vec::with_capacity(ids.len());
                for id in ids {
                    out.push(load_summary(tx, VoucherId::from_uuid(id)).await?);
                }
                Ok(out)
            })
        })
        .await
    }

    /// Account drill: every voucher line hitting `account_code`, each carrying its
    /// voucher + source-object linkage for a voucher → source walk.
    pub async fn account_drill(
        &self,
        account_code: &str,
    ) -> Result<Vec<AccountDrillEntry>, PgVoucherError> {
        let org = current_org().map_err(KernelError::from)?;
        let account_code = account_code.to_owned();
        with_org_conn::<_, Vec<AccountDrillEntry>, PgVoucherError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    r#"
                    SELECT l.id AS line_id, l.account_code, l.side, l.amount_won,
                           v.id AS voucher_id, v.voucher_no, v.status,
                           v.source_object_type, v.source_object_id, v.created_at
                    FROM finance_gl_voucher_lines AS l
                    JOIN finance_gl_vouchers AS v ON v.id = l.voucher_id
                    WHERE l.account_code = $1
                    ORDER BY v.created_at DESC, l.line_no ASC
                    LIMIT 1000
                    "#,
                )
                .bind(&account_code)
                .fetch_all(tx.as_mut())
                .await
                .map_err(DbError::Sqlx)?;

                let mut out = Vec::with_capacity(rows.len());
                for row in rows {
                    out.push(AccountDrillEntry {
                        voucher_id: VoucherId::from_uuid(row.get::<Uuid, _>("voucher_id")),
                        voucher_no: row.get::<String, _>("voucher_no"),
                        status: VoucherStatus::from_db_str(&row.get::<String, _>("status"))?,
                        line_id: row.get::<Uuid, _>("line_id"),
                        account_code: row.get::<String, _>("account_code"),
                        side: DebitCredit::from_db_str(&row.get::<String, _>("side"))?,
                        amount_won: row.get::<i64, _>("amount_won"),
                        source_object_type: row.get::<Option<String>, _>("source_object_type"),
                        source_object_id: row.get::<Option<String>, _>("source_object_id"),
                        entry_at: row.get::<OffsetDateTime, _>("created_at"),
                    });
                }
                Ok(out)
            })
        })
        .await
    }

    // ---- internals -------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    async fn insert_draft(
        &self,
        actor: UserId,
        branch_id: BranchId,
        memo: &str,
        source: Option<&VoucherSourceRef>,
        lines: &[VoucherLineInput],
        reversal_of: Option<VoucherId>,
        action: &str,
        trace: TraceContext,
        occurred_at: OffsetDateTime,
    ) -> Result<VoucherSummary, PgVoucherError> {
        if lines.is_empty() {
            return Err(KernelError::validation("voucher must have at least one line").into());
        }
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let voucher_id = VoucherId::new();
        let voucher_no = next_voucher_no(occurred_at);
        let memo = memo.to_owned();
        let source = source.cloned();
        let lines = lines.to_vec();
        let event = voucher_audit_event(action, actor, branch_id, voucher_id, trace, occurred_at)?
            .with_org(org);

        with_audits::<_, VoucherSummary, PgVoucherError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                insert_voucher_row(
                    tx,
                    voucher_id,
                    org_uuid,
                    branch_id,
                    &voucher_no,
                    VoucherStatus::Draft,
                    &memo,
                    source.as_ref(),
                    reversal_of,
                    actor,
                    None,
                    None,
                    occurred_at,
                )
                .await?;
                for (idx, line) in lines.iter().enumerate() {
                    if line.amount_won <= 0 {
                        return Err(KernelError::validation(
                            "voucher line amount must be strictly positive",
                        )
                        .into());
                    }
                    if line.account_code.trim().is_empty() {
                        return Err(KernelError::validation(
                            "voucher line account_code is required",
                        )
                        .into());
                    }
                    insert_line_row(
                        tx,
                        org_uuid,
                        voucher_id,
                        i32::try_from(idx + 1).map_err(|_| {
                            KernelError::validation("voucher line count overflowed i32")
                        })?,
                        line.account_code.trim(),
                        line.side,
                        line.amount_won,
                        line.memo.trim(),
                    )
                    .await?;
                }
                let summary = load_summary(tx, voucher_id).await?;
                Ok((summary, vec![event]))
            })
        })
        .await
    }

    async fn transition(
        &self,
        command: VoucherTransitionCommand,
        target: VoucherStatus,
        action: &str,
    ) -> Result<VoucherSummary, PgVoucherError> {
        let org = current_org().map_err(KernelError::from)?;
        let action = action.to_owned();
        with_audits::<_, VoucherSummary, PgVoucherError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let current = lock_voucher(tx, command.voucher_id).await?;
                validate_voucher_transition(current.status, target)?;

                // Use-case balance gate at 차대검증. The DB trigger is the
                // backstop; this returns a clean domain error first.
                if target == VoucherStatus::BalanceChecked {
                    let lines = load_lines(tx, command.voucher_id).await?;
                    let outcome = compute_balance(lines.iter().map(|l| (l.side, l.amount_won)))?;
                    ensure_balanced(outcome)?;
                }

                // Separation of duties at 승인: the approver MUST differ from the
                // 기표자 (created_by). Fail-closed here (clean 403) and at the DB
                // CHECK (`approved_by <> created_by`). Matches the four-eyes idiom
                // used for gov_approvals.
                if target == VoucherStatus::Approved && command.actor == current.created_by {
                    return Err(KernelError::forbidden(
                        "self-approval is not allowed: the voucher approver must differ from its preparer",
                    )
                    .into());
                }
                // Defense in depth at 전기: a voucher cannot post without a recorded
                // approver. The FSM only allows Approved → Posted, and 승인 stamps
                // approved_by, so this only fires if that invariant were bypassed.
                if target == VoucherStatus::Posted && current.approved_by.is_none() {
                    return Err(KernelError::conflict(
                        "voucher cannot be posted before it has been approved",
                    )
                    .into());
                }

                let approved_by =
                    (target == VoucherStatus::Approved).then_some(command.actor);
                let posted_at = (target == VoucherStatus::Posted).then_some(command.occurred_at);
                set_voucher_status(
                    tx,
                    command.voucher_id,
                    target,
                    approved_by,
                    posted_at,
                    None,
                    command.occurred_at,
                )
                .await?;

                let event = voucher_audit_event(
                    &action,
                    command.actor,
                    current.branch_id,
                    command.voucher_id,
                    command.trace,
                    command.occurred_at,
                )?
                .with_org(org);
                let summary = load_summary(tx, command.voucher_id).await?;
                Ok((summary, vec![event]))
            })
        })
        .await
    }
}

// ===========================================================================
// Row-level helpers (all run inside a tenant-armed transaction).
// ===========================================================================

struct VoucherRow {
    branch_id: BranchId,
    status: VoucherStatus,
    source: Option<VoucherSourceRef>,
    created_by: UserId,
    approved_by: Option<UserId>,
}

struct LineRow {
    account_code: String,
    side: DebitCredit,
    amount_won: i64,
    memo: String,
}

/// `SELECT ... FOR UPDATE` the voucher so the FSM check + status write are
/// TOCTOU-safe against a concurrent transition.
async fn lock_voucher(
    tx: &mut Transaction<'_, Postgres>,
    voucher_id: VoucherId,
) -> Result<VoucherRow, PgVoucherError> {
    let row = sqlx::query(
        r#"
        SELECT branch_id, status, source_object_type, source_object_id,
               created_by, approved_by
        FROM finance_gl_vouchers
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(*voucher_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await
    .map_err(DbError::Sqlx)?
    .ok_or_else(|| KernelError::not_found("voucher was not found"))?;

    Ok(VoucherRow {
        branch_id: BranchId::from_uuid(row.get::<Uuid, _>("branch_id")),
        status: VoucherStatus::from_db_str(&row.get::<String, _>("status"))?,
        source: source_ref(
            row.get::<Option<String>, _>("source_object_type"),
            row.get::<Option<String>, _>("source_object_id"),
        ),
        created_by: UserId::from_uuid(row.get::<Uuid, _>("created_by")),
        approved_by: row
            .get::<Option<Uuid>, _>("approved_by")
            .map(UserId::from_uuid),
    })
}

async fn load_lines(
    tx: &mut Transaction<'_, Postgres>,
    voucher_id: VoucherId,
) -> Result<Vec<LineRow>, PgVoucherError> {
    let rows = sqlx::query(
        r#"
        SELECT account_code, side, amount_won, memo
        FROM finance_gl_voucher_lines
        WHERE voucher_id = $1
        ORDER BY line_no ASC
        "#,
    )
    .bind(*voucher_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await
    .map_err(DbError::Sqlx)?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(LineRow {
            account_code: row.get::<String, _>("account_code"),
            side: DebitCredit::from_db_str(&row.get::<String, _>("side"))?,
            amount_won: row.get::<i64, _>("amount_won"),
            memo: row.get::<String, _>("memo"),
        });
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
async fn insert_voucher_row(
    tx: &mut Transaction<'_, Postgres>,
    voucher_id: VoucherId,
    org_uuid: Uuid,
    branch_id: BranchId,
    voucher_no: &str,
    status: VoucherStatus,
    memo: &str,
    source: Option<&VoucherSourceRef>,
    reversal_of: Option<VoucherId>,
    created_by: UserId,
    approved_by: Option<UserId>,
    posted_at: Option<OffsetDateTime>,
    occurred_at: OffsetDateTime,
) -> Result<(), PgVoucherError> {
    sqlx::query(
        r#"
        INSERT INTO finance_gl_vouchers (
            id, org_id, branch_id, voucher_no, status, memo,
            source_object_type, source_object_id, reversal_of_voucher_id,
            created_by, approved_by, posted_at, created_at, updated_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $13)
        "#,
    )
    .bind(*voucher_id.as_uuid())
    .bind(org_uuid)
    .bind(*branch_id.as_uuid())
    .bind(voucher_no)
    .bind(status.as_db_str())
    .bind(memo.trim())
    .bind(source.map(|s| s.object_type.trim().to_owned()))
    .bind(source.map(|s| s.object_id.trim().to_owned()))
    .bind(reversal_of.map(|id| *id.as_uuid()))
    .bind(*created_by.as_uuid())
    .bind(approved_by.map(|id| *id.as_uuid()))
    .bind(posted_at)
    .bind(occurred_at)
    .execute(tx.as_mut())
    .await
    .map_err(DbError::Sqlx)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_line_row(
    tx: &mut Transaction<'_, Postgres>,
    org_uuid: Uuid,
    voucher_id: VoucherId,
    line_no: i32,
    account_code: &str,
    side: DebitCredit,
    amount_won: i64,
    memo: &str,
) -> Result<(), PgVoucherError> {
    sqlx::query(
        r#"
        INSERT INTO finance_gl_voucher_lines (
            id, org_id, voucher_id, line_no, account_code, side, amount_won, memo
        )
        VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(org_uuid)
    .bind(*voucher_id.as_uuid())
    .bind(line_no)
    .bind(account_code)
    .bind(side.as_db_str())
    .bind(amount_won)
    .bind(memo)
    .execute(tx.as_mut())
    .await
    .map_err(DbError::Sqlx)?;
    Ok(())
}

/// Advance a voucher's status (and optionally stamp `posted_at` /
/// `reversed_by_voucher_id`). The DB trigger enforces the balance gate + posted
/// immutability regardless of this call site.
async fn set_voucher_status(
    tx: &mut Transaction<'_, Postgres>,
    voucher_id: VoucherId,
    status: VoucherStatus,
    approved_by: Option<UserId>,
    posted_at: Option<OffsetDateTime>,
    reversed_by: Option<VoucherId>,
    occurred_at: OffsetDateTime,
) -> Result<(), PgVoucherError> {
    // `approved_by` is write-once: COALESCE only stamps it when this transition is
    // the 승인 step, and the DB trigger rejects any later change.
    sqlx::query(
        r#"
        UPDATE finance_gl_vouchers
        SET status = $2,
            approved_by = COALESCE($3, approved_by),
            posted_at = COALESCE($4, posted_at),
            reversed_by_voucher_id = COALESCE($5, reversed_by_voucher_id),
            updated_at = $6
        WHERE id = $1
        "#,
    )
    .bind(*voucher_id.as_uuid())
    .bind(status.as_db_str())
    .bind(approved_by.map(|id| *id.as_uuid()))
    .bind(posted_at)
    .bind(reversed_by.map(|id| *id.as_uuid()))
    .bind(occurred_at)
    .execute(tx.as_mut())
    .await
    .map_err(DbError::Sqlx)?;
    Ok(())
}

async fn load_summary(
    tx: &mut Transaction<'_, Postgres>,
    voucher_id: VoucherId,
) -> Result<VoucherSummary, PgVoucherError> {
    let row = sqlx::query(
        r#"
        SELECT id, voucher_no, branch_id, status, memo,
               source_object_type, source_object_id,
               reversal_of_voucher_id, reversed_by_voucher_id,
               created_by, approved_by, posted_at, created_at, updated_at,
               -- Same-org correlated lookups (RLS-scoped to app.current_org,
               -- exactly like the LEFT JOIN they stand in for) — resolve the
               -- branch/user display names so the inspector never has to show
               -- a raw id. NULL only for a deleted row (branches never are,
               -- via FK RESTRICT; a deleted user leaves NULL, same as
               -- support's assignee_name).
               (SELECT b.name FROM branches b
                 WHERE b.id = finance_gl_vouchers.branch_id) AS branch_name,
               (SELECT u.display_name FROM users u
                 WHERE u.id = finance_gl_vouchers.created_by) AS created_by_name,
               (SELECT u.display_name FROM users u
                 WHERE u.id = finance_gl_vouchers.approved_by) AS approved_by_name
        FROM finance_gl_vouchers
        WHERE id = $1
        "#,
    )
    .bind(*voucher_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await
    .map_err(DbError::Sqlx)?
    .ok_or_else(|| KernelError::not_found("voucher was not found"))?;

    let line_rows = sqlx::query(
        r#"
        SELECT id, line_no, account_code, side, amount_won, memo
        FROM finance_gl_voucher_lines
        WHERE voucher_id = $1
        ORDER BY line_no ASC
        "#,
    )
    .bind(*voucher_id.as_uuid())
    .fetch_all(tx.as_mut())
    .await
    .map_err(DbError::Sqlx)?;

    let mut lines = Vec::with_capacity(line_rows.len());
    let mut debit_total_won: i64 = 0;
    let mut credit_total_won: i64 = 0;
    for lr in line_rows {
        let side = DebitCredit::from_db_str(&lr.get::<String, _>("side"))?;
        let amount_won = lr.get::<i64, _>("amount_won");
        match side {
            DebitCredit::Debit => {
                debit_total_won = debit_total_won.saturating_add(amount_won);
            }
            DebitCredit::Credit => {
                credit_total_won = credit_total_won.saturating_add(amount_won);
            }
        }
        lines.push(VoucherLineSummary {
            id: lr.get::<Uuid, _>("id"),
            line_no: lr.get::<i32, _>("line_no"),
            account_code: lr.get::<String, _>("account_code"),
            side,
            amount_won,
            memo: lr.get::<String, _>("memo"),
        });
    }

    Ok(VoucherSummary {
        id: VoucherId::from_uuid(row.get::<Uuid, _>("id")),
        voucher_no: row.get::<String, _>("voucher_no"),
        branch_id: BranchId::from_uuid(row.get::<Uuid, _>("branch_id")),
        branch_name: row.get::<Option<String>, _>("branch_name"),
        status: VoucherStatus::from_db_str(&row.get::<String, _>("status"))?,
        memo: row.get::<String, _>("memo"),
        source_object_type: row.get::<Option<String>, _>("source_object_type"),
        source_object_id: row.get::<Option<String>, _>("source_object_id"),
        reversal_of_voucher_id: row
            .get::<Option<Uuid>, _>("reversal_of_voucher_id")
            .map(VoucherId::from_uuid),
        reversed_by_voucher_id: row
            .get::<Option<Uuid>, _>("reversed_by_voucher_id")
            .map(VoucherId::from_uuid),
        debit_total_won,
        credit_total_won,
        lines,
        created_by: UserId::from_uuid(row.get::<Uuid, _>("created_by")),
        created_by_name: row.get::<Option<String>, _>("created_by_name"),
        approved_by: row
            .get::<Option<Uuid>, _>("approved_by")
            .map(UserId::from_uuid),
        approved_by_name: row.get::<Option<String>, _>("approved_by_name"),
        posted_at: row.get::<Option<OffsetDateTime>, _>("posted_at"),
        created_at: row.get::<OffsetDateTime, _>("created_at"),
        updated_at: row.get::<OffsetDateTime, _>("updated_at"),
    })
}

fn source_ref(object_type: Option<String>, object_id: Option<String>) -> Option<VoucherSourceRef> {
    match (object_type, object_id) {
        (Some(object_type), Some(object_id)) => Some(VoucherSourceRef {
            object_type,
            object_id,
        }),
        _ => None,
    }
}

/// A tenant-unique, human-legible voucher number: `VC-YYYYMMDD-XXXXXXXX`.
fn next_voucher_no(occurred_at: OffsetDateTime) -> String {
    let date = occurred_at.date();
    let short = Uuid::new_v4().simple().to_string()[..8].to_uppercase();
    format!(
        "VC-{:04}{:02}{:02}-{}",
        date.year(),
        u8::from(date.month()),
        date.day(),
        short
    )
}
