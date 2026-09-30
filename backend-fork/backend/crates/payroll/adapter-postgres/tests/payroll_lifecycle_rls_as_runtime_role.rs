#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the four 0186 lifecycle tables
//! (`payroll_line_calculations`, `payroll_run_exceptions`,
//! `payroll_disbursements`, `payroll_payslip_deliveries`), proven as the
//! genuine non-owner `console_rt` role — never the BYPASSRLS superuser pool.
//!
//! What this proves:
//!  * every lifecycle read (exceptions page, disbursement, payslip delivery,
//!    calc rows) under another org's GUC yields nothing — deny-by-omission,
//!    a cross-org run id is indistinguishable from a missing one;
//!  * the org-isolation policies' WITH CHECK rejects writing another org's
//!    row outright;
//!  * a lifecycle mutation (`close_attendance_in_tx`) against another org's
//!    run id is a NotFound, not a cross-org write;
//!  * `load_payslip_issuance_in_tx` on a non-PAID run is invalid_state and
//!    writes no recipient inbox_docs; a cross-org run id is NotFound omit
//!    (RLS), not InvalidState (which would distinguish a foreign run from a
//!    missing one).
//!
//! Draft-run fixtures mint via `payroll.create_run` as `console_rt` (no owner
//! INSERT of draft runs). Lines still INSERT (calculate HOLD).

use console_kernel_core::{OrgId, UserId};
use console_ontology_canonical_domain::{CanonicalPort, CommandId, CommandReceipt, DispatchTarget};
use console_payroll_adapter_postgres::lifecycle::{
    LifecycleError, close_attendance_in_tx, close_preflight_in_tx, get_disbursement_in_tx,
    list_exceptions_in_tx, load_payslip_issuance_in_tx, payslip_delivery_in_tx,
};
use console_payroll_adapter_postgres::pay_run::{PayRunCommand, PayRunQuery, PgPayRunPort};
use console_platform_db::with_org_conn;
use console_platform_test_support::runtime_role_pool;
use sqlx::PgPool;
use time::OffsetDateTime;
use time::macros::date;
use uuid::Uuid;

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

async fn seed_actor(owner_pool: &PgPool, org: Uuid) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("stager-{}", user_id.as_uuid()))
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    user_id
}

async fn seed_run(owner_pool: &PgPool, org: Uuid, actor: UserId) -> Uuid {
    let runtime_pool = runtime_role_pool(owner_pool).await;
    let pay_run = PgPayRunPort::new(runtime_pool, tokio::runtime::Handle::current());
    let created = execute_sync(
        &pay_run,
        PayRunCommand {
            org_id: OrgId::from_uuid(org),
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: PayRunQuery::CreateRun {
                run_id: Uuid::new_v4(),
                period_start: date!(2026 - 06 - 01),
                period_end: date!(2026 - 06 - 30),
                connector: Some("m2".to_owned()),
                job: Some("payroll_draft".to_owned()),
            },
            action_key: "create_run".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .expect("payroll.create_run as console_rt");
    assert_eq!(created.target(), DispatchTarget::PayrollCreateRun);
    created.result()["draft_run_id"]
        .as_str()
        .expect("CreateRun must name draft_run_id")
        .parse()
        .unwrap()
}

async fn execute_sync<P: CanonicalPort + Clone + Send + 'static>(
    port: &P,
    command: P::Command,
) -> Result<CommandReceipt, P::Error>
where
    P::Command: Send + 'static,
    P::Error: Send + 'static,
{
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.execute(&command))
        .await
        .unwrap()
}

async fn seed_line(owner_pool: &PgPool, org: Uuid, run_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO payroll_draft_lines \
         (org_id, run_id, employee_source_key, employee_display_name, employee_company) \
         VALUES ($1, $2, $3, 'Alice', 'KNL') RETURNING id",
    )
    .bind(org)
    .bind(run_id)
    .bind(format!("src-{run_id}"))
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

async fn seed_employee(owner_pool: &PgPool, org: Uuid, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO employees \
         (id, org_id, company, name, source_filename, source_sheet, source_row, source_key) \
         VALUES ($1, $2, 'KNL', $3, 'roster.xlsx', 'Sheet1', 1, $4)",
    )
    .bind(id)
    .bind(org)
    .bind(name)
    .bind(format!("emp-{id}"))
    .execute(owner_pool)
    .await
    .unwrap();
    id
}

async fn seed_user_linked_to_employee(owner_pool: &PgPool, org: Uuid, employee: Uuid) -> UserId {
    let user_id = UserId::new();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, org_id, employee_id) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(*user_id.as_uuid())
    .bind(format!("User {}", user_id.as_uuid()))
    .bind(vec!["MEMBER".to_string()])
    .bind(org)
    .bind(employee)
    .execute(owner_pool)
    .await
    .unwrap();
    user_id
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn lifecycle_tables_are_org_isolated_and_write_checked(pool: PgPool) {
    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    seed_org(&pool, org_a, "A").await;
    seed_org(&pool, org_b, "B").await;

    let stager = seed_actor(&pool, org_a).await;
    let run_a = seed_run(&pool, org_a, stager).await;
    let line_a = seed_line(&pool, org_a, run_a).await;
    let emp_a = seed_employee(&pool, org_a, "Alice").await;
    let recipient = seed_user_linked_to_employee(&pool, org_a, emp_a).await;
    sqlx::query("UPDATE payroll_draft_lines SET employee_id = $1 WHERE id = $2")
        .bind(emp_a)
        .bind(line_a)
        .execute(&pool)
        .await
        .unwrap();

    // Seed one row into each lifecycle table for org A (owner pool).
    sqlx::query(
        "INSERT INTO payroll_line_calculations \
         (org_id, run_id, line_id, version, gross_won, deductions, total_deductions_won, \
          net_won, tax_table_version) \
         VALUES ($1, $2, $3, 1, 3000000, '[]'::jsonb, 373302, 2626698, 'v1')",
    )
    .bind(org_a)
    .bind(run_a)
    .bind(line_a)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO payroll_run_exceptions \
         (org_id, run_id, line_id, employee_display_name, kind, severity, summary_ko) \
         VALUES ($1, $2, $3, 'Alice', 'OVERTIME_ALLOWANCE', 'warn', '연장수당 확인')",
    )
    .bind(org_a)
    .bind(run_a)
    .bind(line_a)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO payroll_disbursements (org_id, run_id, scheduled_at) VALUES ($1, $2, $3)",
    )
    .bind(org_a)
    .bind(run_a)
    .bind(OffsetDateTime::now_utc())
    .execute(&pool)
    .await
    .unwrap();

    let rt_pool = runtime_role_pool(&pool).await;

    // Under org B's GUC every lifecycle read of org A's run yields nothing —
    // the run id is indistinguishable from a nonexistent one.
    let b = OrgId::from_uuid(org_b);
    let exceptions = with_org_conn::<_, _, LifecycleError>(&rt_pool, b, move |tx| {
        Box::pin(async move { list_exceptions_in_tx(tx, run_a, None, None).await })
    })
    .await
    .unwrap();
    assert!(
        exceptions.is_none(),
        "org B must not see org A's exceptions"
    );

    let disbursement = with_org_conn::<_, _, LifecycleError>(&rt_pool, b, move |tx| {
        Box::pin(async move { get_disbursement_in_tx(tx, run_a).await })
    })
    .await
    .unwrap();
    assert!(
        disbursement.is_none(),
        "org B must not see org A's disbursement"
    );

    let delivery = with_org_conn::<_, _, LifecycleError>(&rt_pool, b, move |tx| {
        Box::pin(async move { payslip_delivery_in_tx(tx, run_a, None, None).await })
    })
    .await
    .unwrap();
    assert!(delivery.is_none(), "org B must not see org A's deliveries");

    let calc_rows = with_org_conn::<_, _, LifecycleError>(&rt_pool, b, move |tx| {
        Box::pin(async move {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM payroll_line_calculations")
                .fetch_one(tx.as_mut())
                .await?;
            Ok(count)
        })
    })
    .await
    .unwrap();
    assert_eq!(calc_rows, 0, "org B must read zero org-A calc rows");

    // A lifecycle mutation against another org's run id is NotFound.
    let cross_close = with_org_conn::<_, _, LifecycleError>(&rt_pool, b, move |tx| {
        Box::pin(async move {
            close_attendance_in_tx(tx, run_a, Uuid::new_v4(), OffsetDateTime::now_utc()).await
        })
    })
    .await;
    assert!(
        matches!(cross_close, Err(LifecycleError::NotFound)),
        "cross-org close must be NotFound, got {cross_close:?}"
    );

    // If RLS leaked the row, this would be InvalidState (status is not PAID),
    // which is a distinct wrong-tenant error rather than omit.
    let cross_issue = with_org_conn::<_, _, LifecycleError>(&rt_pool, b, move |tx| {
        Box::pin(async move { load_payslip_issuance_in_tx(tx, run_a).await })
    })
    .await;
    assert!(
        matches!(cross_issue, Err(LifecycleError::NotFound)),
        "cross-org issue load must be NotFound omit (RLS), not a distinct wrong-tenant error, got {cross_issue:?}"
    );

    // WITH CHECK: writing an org-A row while armed as org B is rejected.
    let smuggle = with_org_conn::<_, _, LifecycleError>(&rt_pool, b, move |tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO payroll_run_exceptions \
                 (org_id, run_id, line_id, employee_display_name, kind, severity, summary_ko) \
                 VALUES ($1, $2, $3, 'Mallory', 'RETRO_ADJUSTMENT', 'info', '소급')",
            )
            .bind(org_a)
            .bind(run_a)
            .bind(line_a)
            .execute(tx.as_mut())
            .await?;
            Ok(())
        })
    })
    .await;
    assert!(
        smuggle.is_err(),
        "org B must not be able to insert an org-A exception row"
    );

    // Under org A's own GUC the same reads are all visible.
    let a = OrgId::from_uuid(org_a);
    let own = with_org_conn::<_, _, LifecycleError>(&rt_pool, a, move |tx| {
        Box::pin(async move { list_exceptions_in_tx(tx, run_a, None, None).await })
    })
    .await
    .unwrap()
    .expect("org A must see its own run");
    assert_eq!(own.total, 1);
    assert_eq!(own.open, 1);
    assert_eq!(own.items[0].kind, "OVERTIME_ALLOWANCE");

    // Seeded runs default to BLOCKED_LEGAL_GATE, which is not PAID. The
    // adapter load is the PAID gate; this crate does not emit vault docs, so
    // the inbox count is the side-effect floor it can observe.
    let own_issue = with_org_conn::<_, _, LifecycleError>(&rt_pool, a, move |tx| {
        Box::pin(async move { load_payslip_issuance_in_tx(tx, run_a).await })
    })
    .await;
    match own_issue {
        Err(LifecycleError::InvalidState(message)) => assert_eq!(
            message,
            "cannot issue payslips for a run in status BLOCKED_LEGAL_GATE"
        ),
        other => panic!("issue on a non-PAID run must be invalid_state, got {other:?}"),
    }
    let recipient_id = *recipient.as_uuid();
    let leaked_rt = with_org_conn::<_, _, LifecycleError>(&rt_pool, a, move |tx| {
        Box::pin(async move {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM inbox_docs WHERE recipient_user_id = $1 AND kind = 'payslip'",
            )
            .bind(recipient_id)
            .fetch_one(tx.as_mut())
            .await?;
            Ok(count)
        })
    })
    .await
    .unwrap();
    assert_eq!(
        leaked_rt, 0,
        "runtime org A must see no payslip inbox_docs for the recipient after a refused issue"
    );
    let leaked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inbox_docs WHERE recipient_user_id = $1 AND kind = 'payslip'",
    )
    .bind(recipient_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        leaked, 0,
        "a refused non-PAID issue must leave the recipient's payslip inbox empty"
    );
}

/// REG-P4: `payroll_line_calculations` must be append-only in the database,
/// not in review.
///
/// Migration 0186 declares these rows append-only with only the `payable` flip
/// as a legal update. Before 0222 that was enforced by nothing: `console_rt`
/// held table-wide INSERT and UPDATE, so it could both set `payable` and
/// rewrite `gross_won`/`net_won` after calculation and review.
///
/// Every direction is asserted. Proving only the denials would let an
/// over-broad revoke pass while breaking the calculation writer, and the
/// SQLSTATE is checked rather than "an error occurred" -- a misspelled column
/// also errors, and would otherwise read as a denial.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn runtime_role_cannot_write_calculations_after_insert(pool: PgPool) {
    let rt_pool = runtime_role_pool(&pool).await;

    // Column privileges are resolved at plan time, so `WHERE FALSE` proves the
    // denial without seeding a row the runtime role could not write anyway.
    for statement in [
        // the `payable` flip -- the release-gate bit
        "UPDATE payroll_line_calculations SET payable = TRUE WHERE FALSE",
        "INSERT INTO payroll_line_calculations (org_id, version, payable) \
         SELECT NULL, 1, TRUE WHERE FALSE",
        // and the money columns, which feed payslip issuance verbatim
        "UPDATE payroll_line_calculations SET net_won = 1 WHERE FALSE",
        "UPDATE payroll_line_calculations SET gross_won = 1 WHERE FALSE",
        "UPDATE payroll_line_calculations SET tax_table_version = 'x' WHERE FALSE",
        // DELETE defeats append-only exactly as UPDATE does, and it is never
        // granted explicitly: migration 0031 hands it to `console_rt` through
        // ALTER DEFAULT PRIVILEGES on every future table.
        //
        // NOTE: this assertion cannot fail in this harness, and that is a
        // property of the harness, not of the grant. 0031 scopes the default
        // to `FOR ROLE console_app`, while `#[sqlx::test]` applies migrations
        // as `console_buck_admin`, so `console_rt` never receives DELETE here
        // and the case is unreachable. Measured on a database built the
        // production way -- migrations applied as `console_app` -- `console_rt`
        // holds INSERT,SELECT,UPDATE,DELETE before 0222 and SELECT after it.
        // The enforcement that covers this is the migration's own
        // `payroll_payable.runtime_delete_not_revoked` assertion, which runs
        // wherever migrations run.
        "DELETE FROM payroll_line_calculations WHERE FALSE",
        // Both FKs cascade, so deleting a parent erases calculations without
        // ever needing DELETE on the child. Revoking the child alone is not
        // append-only.
        "DELETE FROM payroll_draft_lines WHERE FALSE",
        "DELETE FROM payroll_draft_runs WHERE FALSE",
    ] {
        let error = sqlx::query(statement)
            .execute(&rt_pool)
            .await
            .expect_err("console_rt must not be able to rewrite a calculation");
        let code = error
            .as_database_error()
            .and_then(|e| e.code())
            .map(|c| c.into_owned())
            .unwrap_or_default();
        assert_eq!(
            code, "42501",
            "must be insufficient_privilege, not an unrelated failure: {error}"
        );
    }

    // The calculation writer inserts an explicit column list excluding
    // `payable` (see lifecycle.rs). That write must survive untouched.
    sqlx::query(
        "INSERT INTO payroll_line_calculations (org_id, version) SELECT NULL, 1 WHERE FALSE",
    )
    .execute(&rt_pool)
    .await
    .expect("console_rt must retain INSERT omitting payable");
}

async fn seed_payroll_period_lock(owner_pool: &PgPool, org: Uuid) {
    sqlx::query(
        "INSERT INTO period_locks (org_id, domain, period_start, period_end, reason) \
         VALUES ($1, 'payroll', $2, $3, 'test lock')",
    )
    .bind(org)
    .bind(date!(2026 - 06 - 01))
    .bind(date!(2026 - 06 - 30))
    .execute(owner_pool)
    .await
    .unwrap();
}

/// A run with NO roster lines must not pass the attendance gate.
///
/// `missing_total` counts lines that LACK attendance material, so over an empty
/// roster it is 0, and `ok: missing_total == 0` was `0 == 0` — true. Since
/// `can_close` is the AND of the hard checks, an empty run closed on the period
/// lock alone and took a 근태 원천 확보 attestation over zero evidence, then
/// carried on down the lifecycle toward payment.
///
/// This is the same rule the repository already applies elsewhere — the
/// canonical enforcement floor refuses to claim enforcement over zero tables,
/// and the preflight sweep refuses a manifest declaring zero gates. A guard that
/// examines no subject must fail, not pass. On payroll it is the difference
/// between a gate and a rubber stamp.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_empty_roster_cannot_satisfy_the_attendance_gate(pool: PgPool) {
    let org = Uuid::new_v4();
    seed_org(&pool, org, "empty-roster").await;
    let stager = seed_actor(&pool, org).await;
    let run = seed_run(&pool, org, stager).await;
    // The OTHER hard check passes, so the attendance check alone decides.
    seed_payroll_period_lock(&pool, org).await;

    // Plain owner transaction: this proves the GATE's arithmetic, not RLS —
    // the isolation properties are covered by the suite above.
    let mut tx = pool.begin().await.unwrap();
    let preflight = close_preflight_in_tx(&mut tx, run)
        .await
        .unwrap()
        .expect("the run exists, so a preflight must be produced");
    let attendance = preflight
        .checks
        .iter()
        .find(|check| check.key == "attendance_material")
        .expect("the attendance check must be present");
    assert!(
        !attendance.ok,
        "an empty roster must not satisfy 근태 원천 확보; it did, so the gate is vacuous"
    );
    assert!(
        !preflight.can_close,
        "an empty run must not be closeable on the period lock alone"
    );
    assert_eq!(
        attendance.note.as_deref(),
        Some("명세 대상 없음(로스터 0명)"),
        "the note must say the roster is empty, not that 0 people lack material"
    );
    drop(tx);

    // And the close itself must refuse, not merely report.
    let mut tx = pool.begin().await.unwrap();
    let closed =
        close_attendance_in_tx(&mut tx, run, Uuid::new_v4(), OffsetDateTime::now_utc()).await;
    assert!(
        matches!(closed, Err(LifecycleError::PreflightBlocked(_))),
        "closing an empty run must be blocked, got {closed:?}"
    );
}

/// The positive control: a roster line WITH attendance material still closes.
///
/// Without this, the guard above could be satisfied by a check that always
/// fails, which would block every real payroll run — a worse outcome than the
/// bug it fixes.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_roster_line_with_attendance_material_still_closes(pool: PgPool) {
    let org = Uuid::new_v4();
    seed_org(&pool, org, "material-present").await;
    let stager = seed_actor(&pool, org).await;
    let run = seed_run(&pool, org, stager).await;
    let line = seed_line(&pool, org, run).await;
    seed_payroll_period_lock(&pool, org).await;
    sqlx::query("UPDATE payroll_draft_lines SET attendance_source_row_count = 3 WHERE id = $1")
        .bind(line)
        .execute(&pool)
        .await
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    let preflight = close_preflight_in_tx(&mut tx, run)
        .await
        .unwrap()
        .expect("the run exists, so a preflight must be produced");
    let attendance = preflight
        .checks
        .iter()
        .find(|check| check.key == "attendance_material")
        .expect("the attendance check must be present");
    assert!(
        attendance.ok,
        "a roster of one line carrying attendance material must satisfy the gate"
    );
    assert!(
        preflight.can_close,
        "with both hard checks green the run must close"
    );
}
