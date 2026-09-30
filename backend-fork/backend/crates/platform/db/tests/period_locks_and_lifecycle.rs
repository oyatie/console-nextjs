#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-LC slice 1 runtime proofs, executed as the genuine non-owner runtime
//! role `console_rt` (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) — the only faithful
//! exercise of the tenant policy:
//!
//! 1. `assert_period_open` fails closed inside an active lock, is scoped to
//!    (domain, tenant), and unlock restores the write window.
//! 2. `period_locks` rows only permit the one-shot unlock UPDATE and never a
//!    DELETE (immutable close history).
//! 3. The lifecycle engine walks the seeded `document` chain, refuses illegal
//!    transitions, and the dispose gate fails closed under legal hold or a
//!    future retention deadline.
//! 4. `object_lifecycle_transitions` is append-only, and lifecycles are
//!    invisible across tenants.

use console_kernel_core::{ErrorKind, OrgId};
use console_platform_db::{
    PeriodLockDomain, assert_period_open, lifecycle, lock_period_lock_key, with_org_conn,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::macros::date;
use uuid::Uuid;

const ORG_B: Uuid = Uuid::from_u128(0x4444_4444_4444_4444_4444_4444_4444_4444);

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
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", tag.to_lowercase()))
    .bind(format!("Org {tag}"))
    .execute(owner_pool)
    .await
    .unwrap();
}

async fn seed_user(owner_pool: &PgPool, org: Uuid, display_name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (org_id, display_name, roles) VALUES ($1, $2, ARRAY['ADMIN']) \
         RETURNING id",
    )
    .bind(org)
    .bind(display_name)
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

async fn seed_active_lock(owner_pool: &PgPool, org: Uuid, domain: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO period_locks (org_id, domain, period_start, period_end, reason) \
         VALUES ($1, $2, DATE '2026-06-01', DATE '2026-06-30', '6월 마감') RETURNING id",
    )
    .bind(org)
    .bind(domain)
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

// ===========================================================================
// 5. Lock creation serializes against gated writes through the shared key.
// ===========================================================================
#[sqlx::test(migrations = "./migrations")]
async fn period_lock_key_serializes_lock_creation_against_gated_writes(owner_pool: PgPool) {
    // The drain's gated staging write takes this per-org advisory lock BEFORE
    // its freeze-window-checking INSERT. Holding it on one session must exclude
    // a concurrent lock creator (close_month), so a lock cannot commit between
    // the staging INSERT's snapshot and its commit.
    let mut holder = owner_pool.begin().await.unwrap();
    lock_period_lock_key(&mut holder, PeriodLockDomain::Payroll, ORG_B)
        .await
        .unwrap();

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let acquired_elsewhere: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("console.period-lock|payroll|{ORG_B}"))
            .fetch_one(&rt_pool)
            .await
            .unwrap();
    assert!(
        !acquired_elsewhere,
        "a concurrent creator must be excluded while the staging side holds the key"
    );

    holder.rollback().await.unwrap();

    // Once the holder releases (commit/rollback), the key is free again.
    let mut free = owner_pool.begin().await.unwrap();
    let reacquired: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("console.period-lock|payroll|{ORG_B}"))
            .fetch_one(free.as_mut())
            .await
            .unwrap();
    assert!(
        reacquired,
        "the key must be free once the holder releases it"
    );
    free.rollback().await.unwrap();
}

// ===========================================================================
// 1. Period-lock guard semantics under console_rt.
// ===========================================================================
#[sqlx::test(migrations = "./migrations")]
async fn period_lock_guard_blocks_scoped_by_domain_and_org_and_unlock_restores(owner_pool: PgPool) {
    let org_a = *OrgId::knl().as_uuid();
    seed_org(&owner_pool, org_a, "A").await;
    seed_org(&owner_pool, ORG_B, "B").await;
    let lock_id = seed_active_lock(&owner_pool, org_a, "payroll").await;

    let rt_pool = runtime_role_pool(&owner_pool).await;

    // (a) Inside the locked window + domain + tenant → conflict, fail closed.
    let blocked =
        with_org_conn::<_, _, console_platform_db::DbError>(&rt_pool, OrgId::knl(), move |tx| {
            Box::pin(async move {
                Ok(assert_period_open(tx, PeriodLockDomain::Payroll, date!(2026 - 06 - 15)).await)
            })
        })
        .await
        .unwrap();
    let err = blocked.expect_err("date inside an active payroll lock must be refused");
    assert_eq!(err.kind, ErrorKind::Conflict);
    assert!(
        err.message.contains("locked"),
        "error must name the lock: {}",
        err.message
    );

    // (b) Same tenant, same window, OTHER domain → open.
    // (c) Same tenant, date outside the window → open.
    // (d) OTHER tenant, same domain + window → open (RLS isolation).
    let checks =
        with_org_conn::<_, _, console_platform_db::DbError>(&rt_pool, OrgId::knl(), move |tx| {
            Box::pin(async move {
                let other_domain =
                    assert_period_open(tx, PeriodLockDomain::Accounting, date!(2026 - 06 - 15))
                        .await;
                let outside =
                    assert_period_open(tx, PeriodLockDomain::Payroll, date!(2026 - 07 - 01)).await;
                Ok((other_domain, outside))
            })
        })
        .await
        .unwrap();
    checks
        .0
        .expect("accounting domain must not be frozen by a payroll lock");
    checks.1.expect("a date outside the window must stay open");

    let cross_org = with_org_conn::<_, _, console_platform_db::DbError>(
        &rt_pool,
        OrgId::from_uuid(ORG_B),
        move |tx| {
            Box::pin(async move {
                Ok(assert_period_open(tx, PeriodLockDomain::Payroll, date!(2026 - 06 - 15)).await)
            })
        },
    )
    .await
    .unwrap();
    cross_org.expect("org A's lock must be invisible to org B (RLS as console_rt)");

    // (e) Unlock restores the window.
    sqlx::query(
        "UPDATE period_locks SET unlocked_at = now(), unlock_reason = '정정 재개' WHERE id = $1",
    )
    .bind(lock_id)
    .execute(&owner_pool)
    .await
    .unwrap();
    let reopened =
        with_org_conn::<_, _, console_platform_db::DbError>(&rt_pool, OrgId::knl(), move |tx| {
            Box::pin(async move {
                Ok(assert_period_open(tx, PeriodLockDomain::Payroll, date!(2026 - 06 - 15)).await)
            })
        })
        .await
        .unwrap();
    reopened.expect("unlocking must restore the write window");
}

// ===========================================================================
// 2. Lock rows are immutable history: only the one-shot unlock UPDATE.
// ===========================================================================
#[sqlx::test(migrations = "./migrations")]
async fn period_lock_rows_refuse_rewrites_and_deletes(owner_pool: PgPool) {
    let org_a = *OrgId::knl().as_uuid();
    seed_org(&owner_pool, org_a, "A").await;
    let lock_id = seed_active_lock(&owner_pool, org_a, "accounting").await;

    let rewrite = sqlx::query("UPDATE period_locks SET reason = 'tampered' WHERE id = $1")
        .bind(lock_id)
        .execute(&owner_pool)
        .await;
    assert!(rewrite.is_err(), "lock content rewrite must be rejected");

    let shrink = sqlx::query(
        "UPDATE period_locks SET period_end = DATE '2026-06-02', unlocked_at = now(), \
         unlock_reason = 'x' WHERE id = $1",
    )
    .bind(lock_id)
    .execute(&owner_pool)
    .await;
    assert!(
        shrink.is_err(),
        "unlock must not be able to smuggle a window rewrite"
    );

    let delete = sqlx::query("DELETE FROM period_locks WHERE id = $1")
        .bind(lock_id)
        .execute(&owner_pool)
        .await;
    assert!(delete.is_err(), "lock rows must never be deleted");

    // The one-shot unlock itself works…
    sqlx::query(
        "UPDATE period_locks SET unlocked_at = now(), unlocked_by = NULL, \
         unlock_reason = '재개' WHERE id = $1",
    )
    .bind(lock_id)
    .execute(&owner_pool)
    .await
    .unwrap();
    // …and is itself immutable afterwards.
    let reunlock = sqlx::query("UPDATE period_locks SET unlock_reason = 'rewritten' WHERE id = $1")
        .bind(lock_id)
        .execute(&owner_pool)
        .await;
    assert!(reunlock.is_err(), "an unlocked row must be fully immutable");
}

// ===========================================================================
// 3. Lifecycle engine: seeded chain, illegal transitions, dispose gates.
// ===========================================================================
#[sqlx::test(migrations = "./migrations")]
async fn lifecycle_walks_document_chain_and_gates_dispose(owner_pool: PgPool) {
    let org_a = *OrgId::knl().as_uuid();
    seed_org(&owner_pool, org_a, "A").await;
    seed_org(&owner_pool, ORG_B, "B").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let object_id = Uuid::new_v4();
    let today = date!(2026 - 07 - 09);
    // Two actors: submitted → approved is a maker–checker transition, so the
    // 기안자 cannot also be the 승인자 (see lifecycle_maker_checker.rs).
    let maker = seed_user(&owner_pool, org_a, "기안자").await;
    let checker = seed_user(&owner_pool, org_a, "승인자").await;

    // Walk the full legal chain (first transition implicitly registers at draft).
    for (to_state, actor) in [
        ("submitted", maker),
        ("approved", checker),
        ("active", checker),
        ("revised", maker),
        ("archived", maker),
    ] {
        let record = with_org_conn::<_, _, console_platform_db::DbError>(
            &rt_pool,
            OrgId::knl(),
            move |tx| {
                Box::pin(async move {
                    Ok(lifecycle::transition_lifecycle(
                        tx,
                        org_a,
                        "document",
                        object_id,
                        to_state,
                        Some(actor),
                        "정상 전이",
                        today,
                    )
                    .await)
                })
            },
        )
        .await
        .unwrap()
        .unwrap_or_else(|e| panic!("transition to {to_state} must succeed: {e:?}"));
        assert_eq!(record.current_state, to_state);
    }

    // Illegal transition refused (archived → approved has no rule).
    let illegal =
        with_org_conn::<_, _, console_platform_db::DbError>(&rt_pool, OrgId::knl(), move |tx| {
            Box::pin(async move {
                Ok(lifecycle::transition_lifecycle(
                    tx,
                    org_a,
                    "document",
                    object_id,
                    "approved",
                    None,
                    "역행 시도",
                    today,
                )
                .await)
            })
        })
        .await
        .unwrap();
    let err = illegal.expect_err("archived -> approved must be refused");
    assert_eq!(err.kind, ErrorKind::InvalidTransition);

    // Unknown object type fails closed.
    let unknown =
        with_org_conn::<_, _, console_platform_db::DbError>(&rt_pool, OrgId::knl(), move |tx| {
            Box::pin(async move {
                Ok(lifecycle::transition_lifecycle(
                    tx,
                    org_a,
                    "mystery_kind",
                    object_id,
                    "submitted",
                    None,
                    "미지 타입",
                    today,
                )
                .await)
            })
        })
        .await
        .unwrap();
    assert_eq!(
        unknown
            .expect_err("unknown object type must fail closed")
            .kind,
        ErrorKind::Validation
    );

    // Dispose gate 1: legal hold blocks dispose.
    let held =
        with_org_conn::<_, _, console_platform_db::DbError>(&rt_pool, OrgId::knl(), move |tx| {
            Box::pin(async move {
                lifecycle::set_lifecycle_hold(tx, org_a, "document", object_id, true, None)
                    .await
                    .unwrap();
                Ok(lifecycle::transition_lifecycle(
                    tx,
                    org_a,
                    "document",
                    object_id,
                    "disposed",
                    None,
                    "폐기 시도",
                    today,
                )
                .await)
            })
        })
        .await
        .unwrap();
    assert_eq!(
        held.expect_err("dispose under legal hold must be refused")
            .kind,
        ErrorKind::Conflict
    );

    // Dispose gate 2: future retention blocks dispose even without a hold.
    let retained =
        with_org_conn::<_, _, console_platform_db::DbError>(&rt_pool, OrgId::knl(), move |tx| {
            Box::pin(async move {
                lifecycle::set_lifecycle_hold(
                    tx,
                    org_a,
                    "document",
                    object_id,
                    false,
                    Some(date!(2027 - 01 - 01)),
                )
                .await
                .unwrap();
                Ok(lifecycle::transition_lifecycle(
                    tx,
                    org_a,
                    "document",
                    object_id,
                    "disposed",
                    None,
                    "폐기 시도",
                    today,
                )
                .await)
            })
        })
        .await
        .unwrap();
    assert_eq!(
        retained
            .expect_err("dispose before the retention deadline must be refused")
            .kind,
        ErrorKind::Conflict
    );

    // Retention elapsed → dispose succeeds; the log holds the full history.
    let disposed =
        with_org_conn::<_, _, console_platform_db::DbError>(&rt_pool, OrgId::knl(), move |tx| {
            Box::pin(async move {
                lifecycle::set_lifecycle_hold(
                    tx,
                    org_a,
                    "document",
                    object_id,
                    false,
                    Some(date!(2026 - 01 - 01)),
                )
                .await
                .unwrap();
                let record = lifecycle::transition_lifecycle(
                    tx,
                    org_a,
                    "document",
                    object_id,
                    "disposed",
                    None,
                    "보존기간 경과 폐기",
                    today,
                )
                .await
                .unwrap();
                let log = lifecycle::list_transitions(tx, record.id).await?;
                Ok((record, log))
            })
        })
        .await
        .unwrap();
    assert_eq!(disposed.0.current_state, "disposed");
    assert_eq!(
        disposed.1.len(),
        6,
        "every applied transition must be logged (5 chain steps + dispose)"
    );
    assert_eq!(disposed.1[0].to_state, "disposed");

    // Cross-org isolation: org B cannot see org A's lifecycle.
    let foreign = with_org_conn::<_, _, console_platform_db::DbError>(
        &rt_pool,
        OrgId::from_uuid(ORG_B),
        move |tx| {
            Box::pin(async move { lifecycle::get_lifecycle(tx, "document", object_id).await })
        },
    )
    .await
    .unwrap();
    assert!(foreign.is_none(), "lifecycles must be tenant-isolated");
}

// ===========================================================================
// 4. Transition log is append-only.
// ===========================================================================
#[sqlx::test(migrations = "./migrations")]
async fn lifecycle_transition_log_is_append_only(owner_pool: PgPool) {
    let org_a = *OrgId::knl().as_uuid();
    seed_org(&owner_pool, org_a, "A").await;
    let rt_pool = runtime_role_pool(&owner_pool).await;
    let object_id = Uuid::new_v4();

    with_org_conn::<_, _, console_platform_db::DbError>(&rt_pool, OrgId::knl(), move |tx| {
        Box::pin(async move {
            lifecycle::transition_lifecycle(
                tx,
                org_a,
                "document",
                object_id,
                "submitted",
                None,
                "상신",
                date!(2026 - 07 - 09),
            )
            .await
            .unwrap();
            Ok(())
        })
    })
    .await
    .unwrap();

    let rewrite = sqlx::query("UPDATE object_lifecycle_transitions SET reason = 'tampered'")
        .execute(&owner_pool)
        .await;
    assert!(rewrite.is_err(), "transition log UPDATE must be rejected");
    let delete = sqlx::query("DELETE FROM object_lifecycle_transitions")
        .execute(&owner_pool)
        .await;
    assert!(delete.is_err(), "transition log DELETE must be rejected");
}
