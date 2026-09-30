#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS + FSM gate for the finance-GL voucher (전표) domain, exercised as
//! the genuine non-owner runtime role `console_rt` (NOSUPERUSER / NOBYPASSRLS / FORCE
//! RLS) — the only faithful exercise of the tenant policy. The default
//! `#[sqlx::test]` pool connects as a BYPASSRLS superuser that sees every row and
//! would green-light a broken policy, so we SEED as the owner and DO EVERYTHING
//! ELSE as `console_rt`.
//!
//! Proves the lane's binding invariants:
//!   (a) BALANCE GATE — an unbalanced voucher cannot advance past 차대검증, both
//!       via the use-case (`submit`) and via a raw status UPDATE (the DB trigger).
//!   (b) POSTED IMMUTABILITY — a posted voucher rejects further FSM steps and its
//!       lines cannot be inserted/mutated (append-only, draft-only trigger).
//!   (c) REVERSAL — reversing a posted voucher creates a linked contra voucher
//!       (sides swapped) that nets the pair to zero; the original is marked
//!       REVERSED and points at its contra. The posted voucher is never mutated.
//!   (d) CROSS-ORG ISOLATION — under org A's armed GUC, org B's voucher is
//!       not-found, and the account drill never sees B's lines.
//!   (e) FAIL-CLOSED — with no tenant scope armed, every read/write errors.

use console_finance_gl_adapter_postgres::PgVoucherStore;
use console_finance_gl_application::{
    CreateVoucherDraftCommand, CreateVoucherDraftFromSourceCommand, ReverseVoucherCommand,
    VoucherLineInput, VoucherSourceRef, VoucherTransitionCommand,
};
use console_finance_gl_domain::{DebitCredit, VoucherId, VoucherStatus};
use console_kernel_core::{BranchId, OrgId, TraceContext, UserId};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::datetime;
use uuid::Uuid;

const ORG_A: Uuid = Uuid::from_u128(0x0A11_0A11_0A11_0A11_0A11_0A11_0A11_0A11);
const ORG_B: Uuid = Uuid::from_u128(0x0B22_0B22_0B22_0B22_0B22_0B22_0B22_0B22);

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
        .bind(org)
        .bind(format!("org-{}", tag.to_lowercase()))
        .bind(format!("Org {tag}"))
        .execute(owner_pool)
        .await
        .unwrap();
}

async fn seed_branch(owner_pool: &PgPool, org: Uuid) -> BranchId {
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("Region {}", Uuid::new_v4()))
            .bind(org)
            .fetch_one(owner_pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind(format!("Branch {}", Uuid::new_v4()))
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(owner_pool: &PgPool, org: Uuid) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("User {}", Uuid::new_v4()))
        .bind(Vec::from(["ADMIN"]))
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    user_id
}

fn line(account: &str, side: DebitCredit, amount_won: i64) -> VoucherLineInput {
    VoucherLineInput {
        account_code: account.to_owned(),
        side,
        amount_won,
        memo: String::new(),
    }
}

fn draft(
    actor: UserId,
    branch_id: BranchId,
    lines: Vec<VoucherLineInput>,
) -> CreateVoucherDraftCommand {
    CreateVoucherDraftCommand {
        actor,
        branch_id,
        memo: "test voucher".to_owned(),
        // Hand-keyed drafts carry no source (S7); source flows only through the
        // trusted derive path.
        source: None,
        lines,
        trace: TraceContext::generate(),
        occurred_at: datetime!(2026-07-10 09:00 UTC),
    }
}

fn step(actor: UserId, voucher_id: VoucherId) -> VoucherTransitionCommand {
    VoucherTransitionCommand {
        actor,
        voucher_id,
        trace: TraceContext::generate(),
        occurred_at: datetime!(2026-07-10 09:00 UTC),
    }
}

/// Drive a balanced draft all the way to POSTED and return its id. Separation of
/// duties is honoured: `creator` files 기표 + 제출, a DISTINCT `approver` signs 승인,
/// and 전기 posts it.
async fn seed_posted_voucher(
    store: &PgVoucherStore,
    org: OrgId,
    creator: UserId,
    approver: UserId,
    branch_id: BranchId,
) -> VoucherId {
    assert_ne!(creator, approver, "SoD: preparer and approver must differ");
    console_platform_request_context::scope_org(org, async {
        let created = store
            .create_draft(draft(
                creator,
                branch_id,
                vec![
                    line("1000", DebitCredit::Debit, 10_000),
                    line("4000", DebitCredit::Credit, 10_000),
                ],
            ))
            .await
            .unwrap();
        store.submit(step(creator, created.id)).await.unwrap();
        let approved = store.approve(step(approver, created.id)).await.unwrap();
        assert_eq!(approved.approved_by, Some(approver));
        let posted = store.post(step(approver, created.id)).await.unwrap();
        assert_eq!(posted.status, VoucherStatus::Posted);
        assert!(posted.posted_at.is_some());
        assert_eq!(posted.approved_by, Some(approver));
        created.id
    })
    .await
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn balance_gate_blocks_unbalanced_advance(owner_pool: PgPool) {
    seed_org(&owner_pool, ORG_A, "A").await;
    let branch = seed_branch(&owner_pool, ORG_A).await;
    let actor = seed_user(&owner_pool, ORG_A).await;
    let rt = runtime_role_pool(&owner_pool).await;
    let store = PgVoucherStore::new(rt);
    let org = OrgId::from_uuid(ORG_A);

    let unbalanced_id = console_platform_request_context::scope_org(org, async {
        let created = store
            .create_draft(draft(
                actor,
                branch,
                vec![
                    line("1000", DebitCredit::Debit, 10_000),
                    line("4000", DebitCredit::Credit, 9_000),
                ],
            ))
            .await
            .unwrap();
        // Use-case balance gate: submit (기표 → 차대검증) must fail closed.
        let submitted = store.submit(step(actor, created.id)).await;
        assert!(
            submitted.is_err(),
            "unbalanced voucher must not clear 차대검증"
        );
        // Still DRAFT after the rejected submit.
        let reread = store.get(created.id).await.unwrap();
        assert_eq!(reread.status, VoucherStatus::Draft);
        created.id
    })
    .await;

    // Defense in depth: a raw status UPDATE past 차대검증 is rejected by the DB
    // trigger even if the use-case gate were bypassed. Arm the GUC ourselves so
    // the row is visible to RLS and the trigger — not RLS — is what rejects it.
    let mut tx = store.pool().begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let raw =
        sqlx::query("UPDATE finance_gl_vouchers SET status = 'BALANCE_CHECKED' WHERE id = $1")
            .bind(*unbalanced_id.as_uuid())
            .execute(tx.as_mut())
            .await;
    assert!(raw.is_err(), "DB trigger must reject unbalanced advance");
    let _ = tx.rollback().await;
    let _ = org;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn posted_voucher_is_immutable(owner_pool: PgPool) {
    seed_org(&owner_pool, ORG_A, "A").await;
    let branch = seed_branch(&owner_pool, ORG_A).await;
    let actor = seed_user(&owner_pool, ORG_A).await;
    let approver = seed_user(&owner_pool, ORG_A).await;
    let rt = runtime_role_pool(&owner_pool).await;
    let store = PgVoucherStore::new(rt);
    let org = OrgId::from_uuid(ORG_A);

    let posted_id = seed_posted_voucher(&store, org, actor, approver, branch).await;

    // No further FSM step except reverse (use-case gate).
    console_platform_request_context::scope_org(org, async {
        assert!(
            store.approve(step(actor, posted_id)).await.is_err(),
            "posted voucher must reject 승인"
        );
        assert!(
            store.submit(step(actor, posted_id)).await.is_err(),
            "posted voucher must reject 차대검증"
        );
    })
    .await;

    // Lines are append-only + draft-only: a raw INSERT into a posted voucher is
    // rejected by the trigger (posted lines immutable). Arm the GUC so the
    // trigger — not RLS — is what rejects it.
    let mut tx = store.pool().begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let inject = sqlx::query(
        r#"INSERT INTO finance_gl_voucher_lines
             (id, org_id, voucher_id, line_no, account_code, side, amount_won, memo)
           VALUES (gen_random_uuid(), $1, $2, 99, '9999', 'DEBIT', 1, '')"#,
    )
    .bind(ORG_A)
    .bind(*posted_id.as_uuid())
    .execute(tx.as_mut())
    .await;
    assert!(
        inject.is_err(),
        "posted voucher lines must be immutable (draft-only INSERT)"
    );
    let _ = tx.rollback().await;

    // A raw UPDATE of a posted line is impossible — console_rt holds no UPDATE grant
    // on the lines table (append-only), so it errors regardless of tenant scope.
    let mut tx = store.pool().begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let mutate =
        sqlx::query("UPDATE finance_gl_voucher_lines SET amount_won = 1 WHERE voucher_id = $1")
            .bind(*posted_id.as_uuid())
            .execute(tx.as_mut())
            .await;
    assert!(mutate.is_err(), "console_rt must not UPDATE posted lines");
    let _ = tx.rollback().await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn reversal_links_and_nets_to_zero(owner_pool: PgPool) {
    seed_org(&owner_pool, ORG_A, "A").await;
    let branch = seed_branch(&owner_pool, ORG_A).await;
    let actor = seed_user(&owner_pool, ORG_A).await;
    let approver = seed_user(&owner_pool, ORG_A).await;
    let rt = runtime_role_pool(&owner_pool).await;
    let store = PgVoucherStore::new(rt);
    let org = OrgId::from_uuid(ORG_A);

    let posted_id = seed_posted_voucher(&store, org, actor, approver, branch).await;

    console_platform_request_context::scope_org(org, async {
        let contra = store
            .reverse(ReverseVoucherCommand {
                actor,
                voucher_id: posted_id,
                memo: "역분개 사유".to_owned(),
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-07-10 10:00 UTC),
            })
            .await
            .unwrap();

        // Contra links back to the original and is itself posted.
        assert_eq!(contra.reversal_of_voucher_id, Some(posted_id));
        assert_eq!(contra.status, VoucherStatus::Posted);

        // Original is now REVERSED and points at its contra — never mutated lines.
        let original = store.get(posted_id).await.unwrap();
        assert_eq!(original.status, VoucherStatus::Reversed);
        assert_eq!(original.reversed_by_voucher_id, Some(contra.id));

        // Sides are swapped, so the pair nets to zero.
        assert_eq!(original.debit_total_won, contra.credit_total_won);
        assert_eq!(original.credit_total_won, contra.debit_total_won);
        assert_eq!(
            original.debit_total_won + contra.debit_total_won,
            original.credit_total_won + contra.credit_total_won,
            "reversed pair must net to zero"
        );

        // Reversing again is rejected (REVERSED is terminal).
        assert!(
            store
                .reverse(ReverseVoucherCommand {
                    actor,
                    voucher_id: posted_id,
                    memo: "again".to_owned(),
                    trace: TraceContext::generate(),
                    occurred_at: datetime!(2026-07-10 10:00 UTC),
                })
                .await
                .is_err(),
            "a reversed voucher cannot be reversed twice"
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn cross_org_isolation_and_fail_closed(owner_pool: PgPool) {
    seed_org(&owner_pool, ORG_A, "A").await;
    seed_org(&owner_pool, ORG_B, "B").await;
    let branch_a = seed_branch(&owner_pool, ORG_A).await;
    let branch_b = seed_branch(&owner_pool, ORG_B).await;
    let actor_a = seed_user(&owner_pool, ORG_A).await;
    let actor_b = seed_user(&owner_pool, ORG_B).await;
    let rt = runtime_role_pool(&owner_pool).await;
    let store = PgVoucherStore::new(rt);
    let org_a = OrgId::from_uuid(ORG_A);
    let org_b = OrgId::from_uuid(ORG_B);

    // Each tenant posts one voucher on distinct accounts.
    let a_id = console_platform_request_context::scope_org(org_a, async {
        store
            .create_draft(draft(
                actor_a,
                branch_a,
                vec![
                    line("1000", DebitCredit::Debit, 5_000),
                    line("4000", DebitCredit::Credit, 5_000),
                ],
            ))
            .await
            .unwrap()
            .id
    })
    .await;
    let b_id = console_platform_request_context::scope_org(org_b, async {
        store
            .create_draft(draft(
                actor_b,
                branch_b,
                vec![
                    line("7000", DebitCredit::Debit, 3_000),
                    line("8000", DebitCredit::Credit, 3_000),
                ],
            ))
            .await
            .unwrap()
            .id
    })
    .await;

    // Under org A's armed GUC, org B's voucher is NOT visible.
    console_platform_request_context::scope_org(org_a, async {
        assert!(store.get(a_id).await.is_ok(), "A sees its own voucher");
        assert!(
            store.get(b_id).await.is_err(),
            "A must not see B's voucher under RLS as console_rt"
        );
        // The account drill for B's account returns nothing under A's scope.
        let drill = store.account_drill("7000").await.unwrap();
        assert!(drill.is_empty(), "A must not drill B's account entries");
        // A's list only contains A's rows.
        let listed = store.list(None, None).await.unwrap();
        assert!(listed.iter().all(|v| v.id == a_id));
    })
    .await;

    // FAIL-CLOSED: with no tenant scope armed, every access errors.
    assert!(
        store.get(a_id).await.is_err(),
        "read with unset app.current_org must fail closed"
    );
    assert!(
        store.list(None, None).await.is_err(),
        "list with unset app.current_org must fail closed"
    );
}

/// SEPARATION OF DUTIES (M2): the 기표자 cannot approve their own voucher; a
/// distinct approver clears 승인, is recorded, and the voucher then posts.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn self_approval_rejected_and_distinct_approver_posts(owner_pool: PgPool) {
    seed_org(&owner_pool, ORG_A, "A").await;
    let branch = seed_branch(&owner_pool, ORG_A).await;
    let creator = seed_user(&owner_pool, ORG_A).await;
    let approver = seed_user(&owner_pool, ORG_A).await;
    let rt = runtime_role_pool(&owner_pool).await;
    let store = PgVoucherStore::new(rt);
    let org = OrgId::from_uuid(ORG_A);

    console_platform_request_context::scope_org(org, async {
        let created = store
            .create_draft(draft(
                creator,
                branch,
                vec![
                    line("1000", DebitCredit::Debit, 8_000),
                    line("4000", DebitCredit::Credit, 8_000),
                ],
            ))
            .await
            .unwrap();
        store.submit(step(creator, created.id)).await.unwrap();

        // 승인 by the 기표자 is rejected (fail-closed) before any write.
        assert!(
            store.approve(step(creator, created.id)).await.is_err(),
            "the preparer must not be able to approve their own voucher"
        );
        // Still awaiting approval — no approver recorded.
        let reread = store.get(created.id).await.unwrap();
        assert_eq!(reread.status, VoucherStatus::BalanceChecked);
        assert_eq!(reread.approved_by, None);

        // A DISTINCT approver clears 승인 and is recorded.
        let approved = store.approve(step(approver, created.id)).await.unwrap();
        assert_eq!(approved.status, VoucherStatus::Approved);
        assert_eq!(approved.approved_by, Some(approver));

        // 전기 posts (no SoD on post) and keeps the recorded approver.
        let posted = store.post(step(creator, created.id)).await.unwrap();
        assert_eq!(posted.status, VoucherStatus::Posted);
        assert_eq!(posted.approved_by, Some(approver));
    })
    .await;
}

/// `approved_by` is write-once: a raw UPDATE rewriting it (even to a THIRD, still-
/// distinct user, so the SoD CHECK would pass) is rejected by the DB trigger.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn approved_by_is_immutable(owner_pool: PgPool) {
    seed_org(&owner_pool, ORG_A, "A").await;
    let branch = seed_branch(&owner_pool, ORG_A).await;
    let creator = seed_user(&owner_pool, ORG_A).await;
    let approver = seed_user(&owner_pool, ORG_A).await;
    let other = seed_user(&owner_pool, ORG_A).await;
    let rt = runtime_role_pool(&owner_pool).await;
    let store = PgVoucherStore::new(rt);
    let org = OrgId::from_uuid(ORG_A);

    let posted_id = seed_posted_voucher(&store, org, creator, approver, branch).await;

    let mut tx = store.pool().begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let rewrite = sqlx::query("UPDATE finance_gl_vouchers SET approved_by = $2 WHERE id = $1")
        .bind(*posted_id.as_uuid())
        .bind(*other.as_uuid())
        .execute(tx.as_mut())
        .await;
    assert!(
        rewrite.is_err(),
        "approved_by must be immutable once recorded"
    );
    let _ = tx.rollback().await;
}

/// Defense in depth: a raw status advance to APPROVED with no recorded approver is
/// rejected by the SoD CHECK, independent of the use-case gate.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn advancing_without_approver_rejected_by_db(owner_pool: PgPool) {
    seed_org(&owner_pool, ORG_A, "A").await;
    let branch = seed_branch(&owner_pool, ORG_A).await;
    let creator = seed_user(&owner_pool, ORG_A).await;
    let rt = runtime_role_pool(&owner_pool).await;
    let store = PgVoucherStore::new(rt);
    let org = OrgId::from_uuid(ORG_A);

    let balance_checked_id = console_platform_request_context::scope_org(org, async {
        let created = store
            .create_draft(draft(
                creator,
                branch,
                vec![
                    line("1000", DebitCredit::Debit, 6_000),
                    line("4000", DebitCredit::Credit, 6_000),
                ],
            ))
            .await
            .unwrap();
        store.submit(step(creator, created.id)).await.unwrap();
        created.id
    })
    .await;

    let mut tx = store.pool().begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG_A.to_string())
        .execute(tx.as_mut())
        .await
        .unwrap();
    let raw = sqlx::query("UPDATE finance_gl_vouchers SET status = 'APPROVED' WHERE id = $1")
        .bind(*balance_checked_id.as_uuid())
        .execute(tx.as_mut())
        .await;
    assert!(
        raw.is_err(),
        "DB CHECK must reject APPROVED without a distinct approver"
    );
    let _ = tx.rollback().await;
}

/// S7: a hand-keyed voucher cannot assert an unverified source linkage; the
/// trusted derive path (called by the approval engine) still records one.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn hand_keyed_source_rejected_but_derive_path_allowed(owner_pool: PgPool) {
    seed_org(&owner_pool, ORG_A, "A").await;
    let branch = seed_branch(&owner_pool, ORG_A).await;
    let actor = seed_user(&owner_pool, ORG_A).await;
    let rt = runtime_role_pool(&owner_pool).await;
    let store = PgVoucherStore::new(rt);
    let org = OrgId::from_uuid(ORG_A);

    console_platform_request_context::scope_org(org, async {
        // A hand-keyed create that fabricates a "derived from approved expense"
        // linkage is rejected outright.
        let mut forged = draft(
            actor,
            branch,
            vec![
                line("1000", DebitCredit::Debit, 4_000),
                line("4000", DebitCredit::Credit, 4_000),
            ],
        );
        forged.source = Some(VoucherSourceRef {
            object_type: "expense_approval".to_owned(),
            object_id: "NONEXISTENT".to_owned(),
        });
        assert!(
            store.create_draft(forged).await.is_err(),
            "a hand-keyed voucher must not assert an unverified source linkage"
        );

        // The trusted derive path DOES record the source.
        let derived = store
            .create_draft_from_source(CreateVoucherDraftFromSourceCommand {
                actor,
                branch_id: branch,
                memo: "derived".to_owned(),
                source: VoucherSourceRef {
                    object_type: "expense_approval".to_owned(),
                    object_id: "EXP-1".to_owned(),
                },
                projected_lines: vec![
                    line("1000", DebitCredit::Debit, 4_000),
                    line("4000", DebitCredit::Credit, 4_000),
                ],
                trace: TraceContext::generate(),
                occurred_at: datetime!(2026-07-10 09:00 UTC),
            })
            .await
            .unwrap();
        assert_eq!(
            derived.source_object_type.as_deref(),
            Some("expense_approval")
        );
        assert_eq!(derived.source_object_id.as_deref(), Some("EXP-1"));
    })
    .await;
}

/// The finance inspector must never leak a raw branch/user id: `branch_name`,
/// `created_by_name`, and `approved_by_name` resolve to real display names via
/// the same-org correlated lookup (round-5 leaks-polish fix), and `None` only
/// before a voucher is approved (no `approved_by_name` yet).
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn voucher_summary_resolves_branch_and_user_display_names(owner_pool: PgPool) {
    seed_org(&owner_pool, ORG_A, "A").await;
    let region_id: Uuid =
        sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
            .bind("리전")
            .bind(ORG_A)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    let branch_id: Uuid = sqlx::query_scalar(
        "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(region_id)
    .bind("남해지사")
    .bind(ORG_A)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    let branch = BranchId::from_uuid(branch_id);

    let creator = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*creator.as_uuid())
        .bind("김기표")
        .bind(Vec::from(["ADMIN"]))
        .bind(ORG_A)
        .execute(&owner_pool)
        .await
        .unwrap();
    let approver = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*approver.as_uuid())
        .bind("이승인")
        .bind(Vec::from(["ADMIN"]))
        .bind(ORG_A)
        .execute(&owner_pool)
        .await
        .unwrap();

    let rt = runtime_role_pool(&owner_pool).await;
    let store = PgVoucherStore::new(rt);
    let org = OrgId::from_uuid(ORG_A);

    console_platform_request_context::scope_org(org, async {
        let created = store
            .create_draft(draft(
                creator,
                branch,
                vec![
                    line("1000", DebitCredit::Debit, 5_000),
                    line("4000", DebitCredit::Credit, 5_000),
                ],
            ))
            .await
            .unwrap();
        assert_eq!(created.branch_name.as_deref(), Some("남해지사"));
        assert_eq!(created.created_by_name.as_deref(), Some("김기표"));
        // Not yet approved — no approver name to show.
        assert_eq!(created.approved_by_name, None);

        store.submit(step(creator, created.id)).await.unwrap();
        let approved = store.approve(step(approver, created.id)).await.unwrap();
        assert_eq!(approved.approved_by_name.as_deref(), Some("이승인"));
        // Unchanged across the transition.
        assert_eq!(approved.branch_name.as_deref(), Some("남해지사"));
        assert_eq!(approved.created_by_name.as_deref(), Some("김기표"));
    })
    .await;
}
