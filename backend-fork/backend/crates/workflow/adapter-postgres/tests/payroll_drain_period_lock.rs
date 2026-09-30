#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-LC period-lock enforcement on the REAL payroll write path
//! (`drain_payroll_job_outbox`), proven as the genuine non-owner `console_rt` role
//! under FORCE RLS:
//!
//!   (a) an active `payroll` period lock overlapping the event's draft period
//!       makes the drain SKIP the event fail-closed: no `payroll_draft_runs`
//!       row is created and the event stays PENDING (retryable, never lost);
//!   (b) after unlock the SAME event drains normally: draft created, event
//!       acked DELIVERED.
//!   (c) a committed draft whose acknowledgment was interrupted reconciles
//!       through the whole drain after locking, without changing its roster;
//!   (d) provenance, unknown-state and database failures never become an ack.

// Reuse the guarded APPLIED-import fixtures rather than inventing another
// importer. Helpers for the standalone roster target are unused in this target.
#[allow(dead_code)]
#[path = "../../../payroll/adapter-postgres/tests/roster_materialisation/seed.rs"]
mod roster_seed;

use console_kernel_core::{ErrorKind, KernelError, OrgId};
// The REAL `ObjectKey::PayRun` owner, as a dev-dependency. Dev-dependencies are
// exempt from the layer-boundary edge check (it scopes to normal deps), which is
// what lets this test drive the actual seam instead of a stub that would only
// prove the drain calls something.
use console_payroll_adapter_postgres::pay_run::PgPayRunPort;
use console_platform_request_context::scope_org;
use console_workflow_domain::{PayrollDraftStaging, PortFuture, StagePayrollDraft};
use console_workflow_runtime_adapter_postgres::PgWorkflowRuntimeStore;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use uuid::Uuid;

async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    for grant in [
        "GRANT SELECT, UPDATE ON workflow_outbox_events TO console_rt",
        "GRANT SELECT, INSERT, UPDATE ON payroll_draft_runs TO console_rt",
        "GRANT SELECT, INSERT ON audit_events TO console_rt",
        "GRANT SELECT ON organizations TO console_rt",
    ] {
        sqlx::query(grant).execute(owner_pool).await.unwrap();
    }
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

/// Seed org + minimal ACTIVE definition + RUNNING run + one PENDING JOB
/// payroll_draft outbox event for June 2026.
async fn seed_payroll_event(owner_pool: &PgPool, org: Uuid) -> (Uuid, Uuid) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, 'plock', 'Lock Org') \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .execute(owner_pool)
    .await
    .unwrap();
    let definition_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_definitions \
             (org_id, workflow_key, display_name, object_type, status, latest_version, active_version) \
         VALUES ($1, 'payroll.period_lock', 'Payroll Lock', 'payroll_period', 'ACTIVE', 1, 1) \
         RETURNING id",
    )
    .bind(org)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_definition_versions \
             (org_id, definition_id, version, status, definition, required_approval_line, required_payment_line) \
         VALUES ($1, $2, 1, 'PUBLISHED', '{}'::jsonb, TRUE, TRUE)",
    )
    .bind(org)
    .bind(definition_id)
    .execute(owner_pool)
    .await
    .unwrap();
    let run_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_runs \
             (id, org_id, definition_id, definition_version, status, trigger_type, \
              idempotency_key, correlation_id, input_payload) \
         VALUES ($1, $2, $3, 1, 'RUNNING', 'OBJECT_EVENT', $4, $5, '{}'::jsonb)",
    )
    .bind(run_id)
    .bind(org)
    .bind(definition_id)
    .bind(format!("plock-trigger-{run_id}"))
    .bind(format!("plock-corr-{run_id}"))
    .execute(owner_pool)
    .await
    .unwrap();
    let event_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow_outbox_events \
             (org_id, run_id, channel, destination_ref, idempotency_key, status, payload) \
         VALUES ($1, $2, 'JOB', 'payroll', $3, 'PENDING', $4) RETURNING id",
    )
    .bind(org)
    .bind(run_id)
    .bind(format!("plock-payroll-{run_id}"))
    .bind(serde_json::json!({
        "job": "payroll_draft",
        "connector": "payroll",
        "period_start": "2026-06-01",
        "period_end": "2026-06-30",
    }))
    .fetch_one(owner_pool)
    .await
    .unwrap();
    (run_id, event_id)
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn payroll_period_lock_blocks_draft_creation_and_unlock_restores(owner_pool: PgPool) {
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    let (_run_id, event_id) = seed_payroll_event(&owner_pool, org_uuid).await;

    // Active payroll lock overlapping the event's June period.
    let lock_id: Uuid = sqlx::query_scalar(
        "INSERT INTO period_locks (org_id, domain, period_start, period_end, reason) \
         VALUES ($1, 'payroll', DATE '2026-06-01', DATE '2026-06-30', '6월 급여 마감') \
         RETURNING id",
    )
    .bind(org_uuid)
    .fetch_one(&owner_pool)
    .await
    .unwrap();

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let store = PgWorkflowRuntimeStore::new(rt_pool.clone());
    let payroll_staging = PgPayRunPort::new(rt_pool.clone(), tokio::runtime::Handle::current());

    // (a) Locked → drain creates nothing, event stays PENDING (retryable).
    let created = scope_org(
        org,
        store.drain_payroll_job_outbox(org, 10, &payroll_staging),
    )
    .await
    .expect("drain itself must not fail on a locked period");
    assert_eq!(created, 0, "no payroll draft may be created inside a lock");

    let (status, drafts): (String, i64) = {
        let status: String =
            sqlx::query_scalar("SELECT status FROM workflow_outbox_events WHERE id = $1")
                .bind(event_id)
                .fetch_one(&owner_pool)
                .await
                .unwrap();
        let drafts: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM payroll_draft_runs WHERE org_id = $1")
                .bind(org_uuid)
                .fetch_one(&owner_pool)
                .await
                .unwrap();
        (status, drafts)
    };
    assert_eq!(status, "PENDING", "the blocked event must stay retryable");
    assert_eq!(
        drafts, 0,
        "no draft row may land while the period is locked"
    );

    // (b) Unlock → the SAME event drains: draft created, event DELIVERED.
    sqlx::query(
        "UPDATE period_locks SET unlocked_at = now(), unlock_reason = '재개' WHERE id = $1",
    )
    .bind(lock_id)
    .execute(&owner_pool)
    .await
    .unwrap();

    let created = scope_org(
        org,
        store.drain_payroll_job_outbox(org, 10, &payroll_staging),
    )
    .await
    .expect("drain must succeed after unlock");
    assert_eq!(created, 1, "the retried event must now create the draft");

    let status: String =
        sqlx::query_scalar("SELECT status FROM workflow_outbox_events WHERE id = $1")
            .bind(event_id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(status, "DELIVERED");
    let drafts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM payroll_draft_runs \
         WHERE org_id = $1 AND period_start = DATE '2026-06-01' AND period_end = DATE '2026-06-30'",
    )
    .bind(org_uuid)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(drafts, 1, "exactly one June draft after unlock");
}

/// A staging seam that acquires the payroll period lock AFTER the drain's
/// phase-1 gate read but BEFORE the real staging write — the exact interleaving
/// the two-transaction split allows — then delegates to the real owner. This
/// proves the WRITE re-runs the gate rather than relying on the earlier read.
struct LockAfterGate {
    lock_pool: PgPool,
    org: Uuid,
    inner: PgPayRunPort,
}

impl PayrollDraftStaging for LockAfterGate {
    fn stage<'a>(&'a self, draft: StagePayrollDraft) -> PortFuture<'a, bool> {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO period_locks (org_id, domain, period_start, period_end, reason) \
                 VALUES ($1, 'payroll', DATE '2026-06-01', DATE '2026-06-30', 'mid-drain lock')",
            )
            .bind(self.org)
            .execute(&self.lock_pool)
            .await
            .map_err(|e| KernelError::internal(e.to_string()))?;
            self.inner.stage(draft).await
        })
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_lock_acquired_after_the_read_gate_still_blocks_the_staging_write(owner_pool: PgPool) {
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    let (_run_id, event_id) = seed_payroll_event(&owner_pool, org_uuid).await;

    let rt_pool = runtime_role_pool(&owner_pool).await;
    let store = PgWorkflowRuntimeStore::new(rt_pool.clone());
    let staging = LockAfterGate {
        lock_pool: owner_pool.clone(),
        org: org_uuid,
        inner: PgPayRunPort::new(rt_pool, tokio::runtime::Handle::current()),
    };

    let created = scope_org(org, store.drain_payroll_job_outbox(org, 10, &staging))
        .await
        .expect("the drain itself must not fail; the blocked draft must stay pending");
    assert_eq!(
        created, 0,
        "no draft may land when the gate closes before staging"
    );

    let status: String =
        sqlx::query_scalar("SELECT status FROM workflow_outbox_events WHERE id = $1")
            .bind(event_id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(status, "PENDING", "the blocked event must stay retryable");

    let drafts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM payroll_draft_runs WHERE org_id = $1")
            .bind(org_uuid)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(
        drafts, 0,
        "the lock that closed between the phases must block the draft"
    );
}

/// Commit through the actual owner, then interrupt the path before phase 3.
/// This injects the stage/ack gap; it does not fabricate an owner success or ack.
struct InterruptAfterStage(PgPayRunPort);

impl PayrollDraftStaging for InterruptAfterStage {
    fn stage<'a>(&'a self, draft: StagePayrollDraft) -> PortFuture<'a, bool> {
        Box::pin(async move {
            self.0.stage(draft).await?;
            Err(KernelError::internal(
                "test interruption after owner commit",
            ))
        })
    }
}

struct PendingDraft {
    org: OrgId,
    workflow_run: Uuid,
    run: Uuid,
    event: Uuid,
    runtime_pool: PgPool,
}

async fn add_attendance_source(owner_pool: &PgPool, org: OrgId, hours: &str) {
    roster_seed::seed_import(
        owner_pool,
        *org.as_uuid(),
        "APPLIED",
        "CANDIDATE",
        (roster_seed::PERIOD_START, roster_seed::PERIOD_END),
        "worker-1",
        serde_json::json!({ "출근": "09:00", "근무시간": hours, "근무일수": "1" }),
    )
    .await;
}

/// A real committed roster plus a still-PENDING workflow event, with review
/// sentinels that the old unconditional materialisation would overwrite.
async fn interrupted_ack_fixture(owner_pool: &PgPool) -> PendingDraft {
    let org = OrgId::knl();
    let (workflow_run, event) = seed_payroll_event(owner_pool, *org.as_uuid()).await;
    roster_seed::seed_employee(owner_pool, *org.as_uuid(), "worker-1", "근태 대상자").await;
    add_attendance_source(owner_pool, org, "8").await;
    let runtime_pool = runtime_role_pool(owner_pool).await;
    let is_runtime: bool = sqlx::query_scalar(
        "SELECT current_user = 'console_rt' AND NOT rolsuper AND NOT rolbypassrls \
         AND oid <> (SELECT relowner FROM pg_class WHERE oid = 'payroll_draft_runs'::regclass) \
         FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(&runtime_pool)
    .await
    .unwrap();
    assert!(
        is_runtime,
        "the serving pool must be a genuine non-owner runtime role"
    );
    let store = PgWorkflowRuntimeStore::new(runtime_pool.clone());
    let staging = InterruptAfterStage(PgPayRunPort::new(
        runtime_pool.clone(),
        tokio::runtime::Handle::current(),
    ));
    assert_eq!(
        scope_org(org, store.drain_payroll_job_outbox(org, 10, &staging))
            .await
            .unwrap(),
        0,
        "the interrupted owner result must not enter the acknowledgment phase"
    );
    let run: Uuid = sqlx::query_scalar(
        "SELECT id FROM payroll_draft_runs WHERE org_id = $1 AND source_label = $2",
    )
    .bind(*org.as_uuid())
    .bind(format!("workflow_runtime_m2:run:{workflow_run}"))
    .fetch_one(owner_pool)
    .await
    .unwrap();
    let initial: (i64, String, i32) = sqlx::query_as(
        "SELECT count(*), min(regular_hours)::text, min(cardinality(source_data_import_row_ids)) \
         FROM payroll_draft_lines WHERE run_id = $1",
    )
    .bind(run)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    assert_eq!(
        initial,
        (1, "8.00".to_owned(), 1),
        "the owner must actually stage source material"
    );
    // Test-only evidence markers: no claim of legal or tax validation is made.
    sqlx::query(
        "UPDATE payroll_draft_lines SET nts_tax_row_status = 'VERIFIED_SOURCE_ROW', \
         calculation_status = 'READY_FOR_REVIEW', blockers = '[\"review evidence retained\"]'::jsonb \
         WHERE run_id = $1",
    )
    .bind(run)
    .execute(owner_pool)
    .await
    .unwrap();
    assert_eq!(
        event_evidence(owner_pool, event).await,
        ("PENDING".to_owned(), 0, false, 0)
    );
    PendingDraft {
        org,
        workflow_run,
        run,
        event,
        runtime_pool,
    }
}

async fn lock_period(owner_pool: &PgPool, org: OrgId) {
    let mut tx = owner_pool.begin().await.unwrap();
    console_platform_db::lock_period_lock_key(
        &mut tx,
        console_platform_db::PeriodLockDomain::Payroll,
        *org.as_uuid(),
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO period_locks (org_id, domain, period_start, period_end, reason) \
         VALUES ($1, 'payroll', DATE '2026-06-01', DATE '2026-06-30', 'interrupted-ack recovery lock')",
    )
    .bind(*org.as_uuid())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

async fn draft_snapshot(owner_pool: &PgPool, run: Uuid) -> serde_json::Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object('run', to_jsonb(r), \
         'lines', (SELECT coalesce(jsonb_agg(to_jsonb(l) ORDER BY l.id), '[]'::jsonb) \
                   FROM payroll_draft_lines l WHERE l.run_id = r.id), \
         'calculations', (SELECT coalesce(jsonb_agg(to_jsonb(c) ORDER BY c.id), '[]'::jsonb) \
                          FROM payroll_line_calculations c WHERE c.run_id = r.id)) \
         FROM payroll_draft_runs r WHERE r.id = $1",
    )
    .bind(run)
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

async fn event_evidence(owner_pool: &PgPool, event: Uuid) -> (String, i32, bool, i64) {
    sqlx::query_as(
        "SELECT status, attempt_count, delivered_at IS NOT NULL, \
         (SELECT count(*) FROM audit_events \
          WHERE action = 'workflow_runtime.outbox_drain' \
            AND target_type = 'workflow_outbox_event' AND target_id = e.id::text) \
         FROM workflow_outbox_events e WHERE e.id = $1",
    )
    .bind(event)
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

async fn assert_recovered_once(owner_pool: &PgPool, f: &PendingDraft, before: &serde_json::Value) {
    let store = PgWorkflowRuntimeStore::new(f.runtime_pool.clone());
    let staging = PgPayRunPort::new(f.runtime_pool.clone(), tokio::runtime::Handle::current());
    for _ in 0..2 {
        assert_eq!(
            scope_org(f.org, store.drain_payroll_job_outbox(f.org, 10, &staging))
                .await
                .unwrap(),
            0,
            "reconciliation must not create another draft"
        );
        assert_eq!(
            draft_snapshot(owner_pool, f.run).await,
            *before,
            "no run, roster or calculation rewrite"
        );
        assert_eq!(
            event_evidence(owner_pool, f.event).await,
            ("DELIVERED".to_owned(), 1, true, 1)
        );
        let drafts: i64 =
            sqlx::query_scalar("SELECT count(*) FROM payroll_draft_runs WHERE org_id = $1")
                .bind(*f.org.as_uuid())
                .fetch_one(owner_pool)
                .await
                .unwrap();
        assert_eq!(drafts, 1, "retries must preserve one natural-key draft");
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_closed_run_recovers_its_interrupted_ack_after_locking(owner_pool: PgPool) {
    let f = interrupted_ack_fixture(&owner_pool).await;
    lock_period(&owner_pool, f.org).await;
    let actor: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE org_id = $1 ORDER BY id LIMIT 1")
            .bind(*f.org.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    let mut tx = f.runtime_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(f.org.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    console_payroll_adapter_postgres::lifecycle::close_attendance_in_tx(
        &mut tx,
        f.run,
        actor,
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let before = draft_snapshot(&owner_pool, f.run).await;
    assert_eq!(before["run"]["status"], "ATTENDANCE_CLOSED");
    add_attendance_source(&owner_pool, f.org, "11").await;
    assert_recovered_once(&owner_pool, &f, &before).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_locked_preclose_run_recovers_without_refreshing_changed_sources(owner_pool: PgPool) {
    let f = interrupted_ack_fixture(&owner_pool).await;
    let before = draft_snapshot(&owner_pool, f.run).await;
    assert_eq!(before["run"]["status"], "BLOCKED_LEGAL_GATE");
    lock_period(&owner_pool, f.org).await;
    add_attendance_source(&owner_pool, f.org, "11").await;
    assert_recovered_once(&owner_pool, &f, &before).await;
}

async fn assert_still_pending(owner_pool: &PgPool, f: &PendingDraft, before: &serde_json::Value) {
    let store = PgWorkflowRuntimeStore::new(f.runtime_pool.clone());
    let staging = PgPayRunPort::new(f.runtime_pool.clone(), tokio::runtime::Handle::current());
    for _ in 0..2 {
        // Some failures propagate out of phase 1; owner failures are recorded
        // by leaving the event pending. Neither outcome permits an acknowledgment.
        let outcome = scope_org(f.org, store.drain_payroll_job_outbox(f.org, 10, &staging)).await;
        if let Ok(created) = outcome {
            assert_eq!(created, 0);
        }
        assert_eq!(draft_snapshot(owner_pool, f.run).await, *before);
        assert_eq!(
            event_evidence(owner_pool, f.event).await,
            ("PENDING".to_owned(), 0, false, 0)
        );
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_locked_existing_run_with_changed_provenance_stays_pending(owner_pool: PgPool) {
    let f = interrupted_ack_fixture(&owner_pool).await;
    lock_period(&owner_pool, f.org).await;
    sqlx::query(
        "UPDATE workflow_outbox_events SET payload = jsonb_set(payload, '{connector}', '\"different-owner\"') WHERE id = $1",
    )
    .bind(f.event)
    .execute(&owner_pool)
    .await
    .unwrap();
    let before = draft_snapshot(&owner_pool, f.run).await;
    assert_still_pending(&owner_pool, &f, &before).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_locked_existing_run_with_unknown_state_stays_pending(owner_pool: PgPool) {
    let f = interrupted_ack_fixture(&owner_pool).await;
    // Model a newer database state seen by an older binary. Only this disposable
    // test database's CHECK is extended; no production migration is modified.
    sqlx::query("ALTER TABLE payroll_draft_runs DROP CONSTRAINT payroll_draft_runs_status_check")
        .execute(&owner_pool)
        .await
        .unwrap();
    sqlx::query(
        "ALTER TABLE payroll_draft_runs ADD CONSTRAINT payroll_draft_runs_status_check \
         CHECK (status IN ('STAGED','BLOCKED_LEGAL_GATE','READY_FOR_REVIEW','ATTENDANCE_CLOSED', \
         'CALCULATING','CALCULATED','SUBMITTED','REJECTED','APPROVED','DISBURSEMENT_SCHEDULED', \
         'PAID','ISSUED','VOID','FUTURE_REVIEW_PHASE'))",
    )
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query("UPDATE payroll_draft_runs SET status = 'FUTURE_REVIEW_PHASE' WHERE id = $1")
        .bind(f.run)
        .execute(&owner_pool)
        .await
        .unwrap();
    lock_period(&owner_pool, f.org).await;
    let before = draft_snapshot(&owner_pool, f.run).await;
    assert_still_pending(&owner_pool, &f, &before).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn period_lock_select_failure_does_not_ack_an_existing_run(owner_pool: PgPool) {
    let f = interrupted_ack_fixture(&owner_pool).await;
    lock_period(&owner_pool, f.org).await;
    sqlx::query("REVOKE SELECT ON period_locks FROM console_rt")
        .execute(&owner_pool)
        .await
        .unwrap();
    let mut probe = f.runtime_pool.begin().await.unwrap();
    let denied = sqlx::query("SELECT 1 FROM period_locks LIMIT 1")
        .fetch_optional(&mut *probe)
        .await
        .unwrap_err();
    assert_eq!(
        denied.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("42501")
    );
    probe.rollback().await.unwrap();
    let staging = PgPayRunPort::new(f.runtime_pool.clone(), tokio::runtime::Handle::current());
    let failed = staging
        .stage(StagePayrollDraft {
            org: f.org,
            outbox_event_id: f.event,
            run_id: f.workflow_run,
            period_start: Some(roster_seed::PERIOD_START),
            period_end: Some(roster_seed::PERIOD_END),
            connector: Some("payroll".to_owned()),
            job: Some("payroll_draft".to_owned()),
        })
        .await
        .unwrap_err();
    assert_eq!(
        failed.kind,
        ErrorKind::Internal,
        "database failure must remain a failure, not locked-success"
    );
    let before = draft_snapshot(&owner_pool, f.run).await;
    assert_still_pending(&owner_pool, &f, &before).await;
}
