//! M2 workflow-runtime payroll outbox drainer (design §B/§F).
//!
//! A single background task (mirroring `mail_sync::spawn`) ticks on a fixed
//! cadence and, PER tenant, (1) runs the crash-recovery reconciler
//! (`m2_strangler::reconcile_completion_tails`) to restage any FINAL_COMPLETED work
//! order whose runtime tail never reached SUCCEEDED — never started (no run) OR
//! started then died mid-tail (a partial run), then (2) drains the JOB
//! payroll outbox into idempotent `payroll_draft_runs` staging rows via the workflow
//! runtime adapter. Reconciling before draining means a tail restaged this tick is
//! drained into a payroll draft in the same tick. Both steps are dark-safe no-ops
//! for un-enrolled tenants.
//!
//! Because the app connects as the non-owner `console_rt` role under RLS, the loop
//! cannot see any tenant's rows without arming `app.current_org`. Each tick:
//!   1. ENUMERATES every real tenant via the `platform_list_organizations()`
//!      SECURITY DEFINER function (id-only) — the one read that legitimately
//!      spans tenants and the only RLS-safe way for `console_rt` to discover orgs;
//!      then
//!   2. for each org, re-enters the tenant scope with `scope_org(org, ..)` (a
//!      bare `tokio::spawn`/loop iteration does NOT inherit `CURRENT_ORG`, which
//!      would leave the GUC unset → RLS zero rows) and calls the adapter drainer,
//!      which arms `app.current_org` to that org for its own `with_audits` txn.
//!
//! Draining an un-enrolled tenant is a cheap no-op: it has emitted no JOB payroll
//! outbox events, so the claim returns nothing. M2 lands dark — nothing enrolls a
//! tenant in a shipped migration/seed — so in production this loop finds no work.
//!
//! ponytail: per-tick full-org scan + one claim txn per org. Fine while M2 is
//! dark and org count is small; swap the enumerate step for a
//! `comms_due_email_accounts`-style SECURITY DEFINER "orgs with pending JOB
//! events" function if org count or event volume ever makes the empty passes hurt.

use std::sync::Arc;
use std::time::Duration;

use console_kernel_core::OrgId;
use console_notifications_adapter_postgres::PgNotificationStore;
use console_payroll_adapter_postgres::pay_run::PgPayRunPort;
use console_platform_realtime::PostgresNotificationNotifier;
use console_platform_request_context::scope_org;
use console_workflow_runtime_adapter_postgres::PgWorkflowRuntimeStore;
use tokio::sync::watch;
use uuid::Uuid;

/// Seconds between drain ticks.
const DEFAULT_TICK_SECS: u64 = 30;

/// Max JOB payroll events claimed per tenant per tick (bounds one txn's size).
const DRAIN_BATCH_LIMIT: i64 = 100;

/// A handle that stops the drainer loop on explicit shutdown.
#[derive(Debug)]
pub struct WorkflowDrainHandle {
    shutdown_tx: watch::Sender<bool>,
}

impl WorkflowDrainHandle {
    /// Signal the drainer loop to stop.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Spawn the workflow payroll outbox drainer on the app pool. The loop runs until
/// the returned handle is shut down.
#[must_use]
pub fn spawn(pool: sqlx::PgPool) -> WorkflowDrainHandle {
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let store = PgWorkflowRuntimeStore::new(pool.clone());
    // The compensation NOTIFICATION outbox drains into real notification rows
    // through this sink; its realtime notifier means a bridged approval-line
    // notification also fans out over the WebSocket, same as a REST-created one.
    let notification_sink = PgNotificationStore::new(pool.clone())
        .with_notifier(Arc::new(PostgresNotificationNotifier::new(pool.clone())));
    // `payroll_draft_runs` belongs to `ObjectKey::PayRun`, whose sole permitted
    // writer crate is `console-payroll-adapter-postgres`. The workflow adapter
    // may not depend on it (adapter → adapter is a layer-boundary violation), so
    // the owner is injected here as `console_workflow_domain::PayrollDraftStaging`
    // — exactly how `notification_sink` above crosses the same boundary.
    let payroll_staging = PgPayRunPort::new(pool.clone(), tokio::runtime::Handle::current());

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(DEFAULT_TICK_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tracing::info!(
            tick_secs = DEFAULT_TICK_SECS,
            "workflow payroll outbox drainer started"
        );

        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        tracing::info!("workflow payroll outbox drainer stopping");
                        break;
                    }
                }
                _ = ticker.tick() => {
                    run_tick(&pool, &store, &notification_sink, &payroll_staging).await;
                }
            }
        }
    });

    WorkflowDrainHandle { shutdown_tx }
}

/// One drain tick: enumerate tenants, then drain each under its armed org.
async fn run_tick(
    pool: &sqlx::PgPool,
    store: &PgWorkflowRuntimeStore,
    notification_sink: &PgNotificationStore,
    payroll_staging: &PgPayRunPort,
) {
    let orgs: Vec<Uuid> = match sqlx::query_scalar("SELECT id FROM platform_list_organizations()")
        // rls-arming: ok platform_list_organizations() is the SECURITY DEFINER id-only tenant discovery; every per-org drain below runs under scope_org / with_org_conn
        .fetch_all(pool)
        .await
    {
        Ok(orgs) => orgs,
        Err(err) => {
            tracing::warn!(error = %err, "workflow drain: enumerate tenants failed");
            return;
        }
    };

    for org_uuid in orgs {
        let org = OrgId::from_uuid(org_uuid);

        // Recovery reconciler (crash-safety): the legacy path commits FINAL_COMPLETED
        // and only then runs the runtime tail across separate txns — a crash in
        // between leaves a completed work order whose tail never reached SUCCEEDED
        // (no run at all, OR a partial run whose outbox event was never written), so
        // no outbox event for the drainer to claim. Re-drive those tails idempotently
        // BEFORE draining, so a tail restaged this tick is drained into a payroll
        // draft in the same tick. Dark-safe: a no-op unless the tenant is flag-on with
        // a published completion definition.
        match scope_org(
            org,
            console_workorder_rest::m2_strangler::reconcile_completion_tails(store, org),
        )
        .await
        {
            Ok(0) => {}
            Ok(restaged) => tracing::info!(
                org = %org,
                restaged,
                "workflow drain: reconciler restaged crash-orphaned completion tails"
            ),
            Err(err) => tracing::warn!(
                org = %org,
                error = %err,
                "workflow drain: reconciler pass failed"
            ),
        }

        // Re-enter the tenant scope before the adapter call. The adapter arms
        // app.current_org from the org argument for its own txn; scope_org keeps
        // CURRENT_ORG consistent for any task-local reader on this path.
        let result = scope_org(
            org,
            store.drain_payroll_job_outbox(org, DRAIN_BATCH_LIMIT, payroll_staging),
        )
        .await;
        match result {
            Ok(0) => {}
            Ok(created) => tracing::info!(
                org = %org,
                drafts_created = created,
                "workflow drain: staged payroll drafts"
            ),
            Err(err) => tracing::warn!(
                org = %org,
                error = %err,
                "workflow drain: tenant pass failed"
            ),
        }

        // Compensation bridge: NOTIFICATION outbox -> real notification rows for
        // the approval line. Idempotent per (outbox event, recipient), so a
        // re-drain never doubles a notification.
        match scope_org(
            org,
            store.drain_notification_outbox(org, DRAIN_BATCH_LIMIT, notification_sink),
        )
        .await
        {
            Ok(0) => {}
            Ok(emitted) => tracing::info!(
                org = %org,
                notifications_emitted = emitted,
                "workflow drain: bridged approval-line notifications"
            ),
            Err(err) => tracing::warn!(
                org = %org,
                error = %err,
                "workflow drain: notification bridge pass failed"
            ),
        }
    }
}
