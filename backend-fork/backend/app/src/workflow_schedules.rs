//! Workflow cron-schedule substrate (BE-AUTO slice 1, closes adequacy-audit
//! gap 9): cron parsing/validation/next-fire helpers shared by the Workflow
//! Studio schedule REST surface, plus the background poller that starts due
//! runs.
//!
//! ## Poller (mirrors `workflow_drain`)
//! A single background task ticks on a fixed cadence; per tick it enumerates
//! every tenant via the `platform_list_organizations()` SECURITY DEFINER
//! function (the one legitimate cross-tenant read for a `console_rt` loop), then
//! re-enters each tenant scope with `scope_org` and polls that org's due
//! schedules (`enabled AND next_run_at <= now`).
//!
//! ## Exactly-once under concurrent poll
//! The due row's `next_run_at` is the claim token. For fire F the poller
//! starts a run under the DETERMINISTIC idempotency key
//! `schedule:{id}:{F.unix_timestamp()}` — the run spine's
//! `UNIQUE(org_id, idempotency_key)` guarantees at most one run per fire even
//! when two pollers pick up the same due row — then advances the schedule with
//! an UPDATE guarded on `next_run_at = F`, so exactly one advance applies and
//! a slot is never skipped or double-fired. A crash between start and advance
//! re-runs the same fire next tick: the key collides (`SKIPPED`) and the
//! advance still lands.
//!
//! ## Catch-up policy
//! `next_run_at` advances to the next occurrence AFTER `max(now, fire)`: a
//! poller that was down over several slots fires ONCE for the oldest missed
//! slot and then jumps to the future — recurring business workflows (e.g.
//! 일일 점검 상신) must not stampede N make-up runs after an outage.
//!
//! ## Timezone
//! `cron_expr` is evaluated in the schedule's IANA `timezone` (default
//! Asia/Seoul): "0 9 * * *" fires at 09:00 KST. chrono/chrono-tz are confined
//! to this module's cron boundary; everything stored or exposed stays
//! `time::OffsetDateTime` (UTC).

use std::str::FromStr;
use std::time::Duration;

use chrono::TimeZone;
use console_kernel_core::{KernelError, OrgId, TraceContext};
use console_platform_request_context::scope_org;
use console_workflow_domain::TriggerType;
use console_workflow_runtime::{AuditContext, StartRunRequest, TriggeredStart, start_bound_run};
use console_workflow_runtime_adapter_postgres::{DueScheduleRow, PgWorkflowRuntimeStore};
use serde_json::json;
use time::OffsetDateTime;
use tokio::sync::watch;
use uuid::Uuid;

/// Default IANA timezone a schedule's cron pattern is evaluated in.
pub const DEFAULT_TIMEZONE: &str = "Asia/Seoul";

/// Seconds between poll ticks (matches the workflow_drain cadence).
const DEFAULT_TICK_SECS: u64 = 30;

/// Max due schedules handled per tenant per tick (bounds one tick's work).
const POLL_BATCH_LIMIT: i64 = 50;

/// How many upcoming fire times the preview endpoint returns.
pub const PREVIEW_FIRE_COUNT: usize = 3;

/// Parse + validate a cron expression (standard 5-field, optionally extended
/// with seconds — croner's accepted grammar). Garbage is a validation error.
pub fn validate_cron(expr: &str) -> Result<croner::Cron, KernelError> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Err(KernelError::validation("cron_expr must not be empty"));
    }
    croner::Cron::from_str(trimmed)
        .map_err(|err| KernelError::validation(format!("invalid cron expression: {err}")))
}

/// Parse + validate an IANA timezone name (e.g. `Asia/Seoul`).
pub fn validate_timezone(tz: &str) -> Result<chrono_tz::Tz, KernelError> {
    chrono_tz::Tz::from_str(tz.trim())
        .map_err(|_| KernelError::validation(format!("unknown IANA timezone {tz:?}")))
}

/// The next occurrence of `expr` (evaluated in `tz`) STRICTLY AFTER `after`,
/// as a UTC `OffsetDateTime`.
pub fn next_occurrence(
    expr: &str,
    tz: &str,
    after: OffsetDateTime,
) -> Result<OffsetDateTime, KernelError> {
    let cron = validate_cron(expr)?;
    let zone = validate_timezone(tz)?;
    next_after(&cron, zone, after)
}

/// The next `count` occurrences of `expr` (evaluated in `tz`) strictly after
/// `after` — the schedule preview surface.
pub fn next_occurrences(
    expr: &str,
    tz: &str,
    after: OffsetDateTime,
    count: usize,
) -> Result<Vec<OffsetDateTime>, KernelError> {
    let cron = validate_cron(expr)?;
    let zone = validate_timezone(tz)?;
    let mut fires = Vec::with_capacity(count);
    let mut cursor = after;
    for _ in 0..count {
        let next = next_after(&cron, zone, cursor)?;
        fires.push(next);
        cursor = next;
    }
    Ok(fires)
}

fn next_after(
    cron: &croner::Cron,
    zone: chrono_tz::Tz,
    after: OffsetDateTime,
) -> Result<OffsetDateTime, KernelError> {
    let after_utc = chrono::DateTime::from_timestamp(after.unix_timestamp(), after.nanosecond())
        .ok_or_else(|| KernelError::validation("timestamp out of range for cron evaluation"))?;
    let zoned = zone.from_utc_datetime(&after_utc.naive_utc());
    let next = cron
        .find_next_occurrence(&zoned, false)
        .map_err(|err| KernelError::validation(format!("no next cron occurrence: {err}")))?;
    OffsetDateTime::from_unix_timestamp(next.timestamp())
        .map_err(|err| KernelError::internal(format!("cron occurrence out of range: {err}")))
}

/// A handle that stops the schedule poller loop on explicit shutdown.
#[derive(Debug)]
pub struct WorkflowSchedulePollerHandle {
    shutdown_tx: watch::Sender<bool>,
}

impl WorkflowSchedulePollerHandle {
    /// Signal the poller loop to stop.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Spawn the workflow schedule poller on the app pool. The loop runs until the
/// returned handle is shut down.
#[must_use]
pub fn spawn(pool: sqlx::PgPool) -> WorkflowSchedulePollerHandle {
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let store = PgWorkflowRuntimeStore::new(pool.clone());

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(DEFAULT_TICK_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tracing::info!(
            tick_secs = DEFAULT_TICK_SECS,
            "workflow schedule poller started"
        );

        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        tracing::info!("workflow schedule poller stopping");
                        break;
                    }
                }
                _ = ticker.tick() => {
                    run_tick(&pool, &store).await;
                }
            }
        }
    });

    WorkflowSchedulePollerHandle { shutdown_tx }
}

/// One poll tick: enumerate tenants, then poll each under its armed org scope.
async fn run_tick(pool: &sqlx::PgPool, store: &PgWorkflowRuntimeStore) {
    let orgs: Vec<Uuid> = match sqlx::query_scalar("SELECT id FROM platform_list_organizations()")
        // rls-arming: ok platform_list_organizations() is the SECURITY DEFINER id-only tenant discovery; every per-org pass below runs under scope_org / with_org_conn
        .fetch_all(pool)
        .await
    {
        Ok(orgs) => orgs,
        Err(err) => {
            tracing::warn!(error = %err, "workflow schedules: enumerate tenants failed");
            return;
        }
    };

    for org_uuid in orgs {
        let org = OrgId::from_uuid(org_uuid);
        match scope_org(org, poll_org(store, org, OffsetDateTime::now_utc())).await {
            Ok(0) => {}
            Ok(started) => tracing::info!(
                org = %org,
                started,
                "workflow schedules: started due runs"
            ),
            Err(err) => tracing::warn!(
                org = %org,
                error = %err.message,
                "workflow schedules: tenant pass failed"
            ),
        }
    }
}

/// Poll one tenant's due schedules as of `now`: start one idempotent run per
/// due fire, then advance the schedule. Returns the number of NEW runs
/// started. Per-schedule failures record `last_status = FAILED` and STILL
/// advance `next_run_at` (a broken definition must not hot-loop every tick);
/// they never abort the rest of the batch.
pub async fn poll_org(
    store: &PgWorkflowRuntimeStore,
    org: OrgId,
    now: OffsetDateTime,
) -> Result<u32, KernelError> {
    let due = store.list_due_schedules(org, now, POLL_BATCH_LIMIT).await?;
    let mut started = 0u32;
    for schedule in due {
        match fire_schedule(store, org, &schedule, now).await {
            Ok(true) => started += 1,
            Ok(false) => {}
            Err(err) => tracing::warn!(
                org = %org,
                schedule_id = %schedule.id,
                error = %err.message,
                "workflow schedules: due schedule failed; continuing tenant batch"
            ),
        }
    }
    Ok(started)
}

/// Handle one due fire: start the run (exactly-once via the deterministic
/// idempotency key), compute the next occurrence, and advance the row guarded
/// on the claimed fire. Returns whether a NEW run was started.
async fn fire_schedule(
    store: &PgWorkflowRuntimeStore,
    org: OrgId,
    schedule: &DueScheduleRow,
    now: OffsetDateTime,
) -> Result<bool, KernelError> {
    let fire = schedule.next_run_at;
    let outcome = start_scheduled_run(store, org, schedule, fire).await;

    let (last_status, started) = match &outcome {
        Ok(TriggeredStart::Started { .. }) => ("STARTED", true),
        Ok(TriggeredStart::AlreadyStarted) => ("SKIPPED", false),
        Err(err) => {
            tracing::warn!(
                org = %org,
                schedule_id = %schedule.id,
                error = %err.message,
                "workflow schedules: run start failed; recording FAILED and advancing"
            );
            ("FAILED", false)
        }
    };

    // Catch-up policy: next occurrence strictly after max(now, fire) — one
    // make-up fire per outage, never a stampede. A next-fire computation
    // failure (cron/timezone corrupted post-authoring) parks the schedule
    // (next_run_at NULL) instead of hot-looping.
    let after = if now > fire { now } else { fire };
    let next = match next_occurrence(&schedule.cron_expr, &schedule.timezone, after) {
        Ok(next) => Some(next),
        Err(err) => {
            tracing::warn!(
                org = %org,
                schedule_id = %schedule.id,
                error = %err.message,
                "workflow schedules: next-fire computation failed; parking schedule"
            );
            None
        }
    };
    let last_status = if next.is_none() {
        "FAILED"
    } else {
        last_status
    };

    match store
        .advance_schedule(org, schedule.id, fire, next, last_status)
        .await
    {
        Ok(true) => {}
        Ok(false) => return Ok(started),
        Err(err) => {
            tracing::warn!(
                org = %org,
                schedule_id = %schedule.id,
                error = %err.message,
                "workflow schedules: schedule advance failed; will retry next tick"
            );
            return Ok(false);
        }
    }
    Ok(started)
}

async fn start_scheduled_run(
    store: &PgWorkflowRuntimeStore,
    org: OrgId,
    schedule: &DueScheduleRow,
    fire: OffsetDateTime,
) -> Result<TriggeredStart, KernelError> {
    let Some((version, definition)) = store
        .resolve_active_exec_definition(org, schedule.definition_id)
        .await?
    else {
        return Err(KernelError::conflict(
            "schedule definition is not an ACTIVE wf.exec.v1 definition",
        ));
    };

    let audit = AuditContext {
        actor: None, // system fire — the authoring act carried the authority
        trace: TraceContext::generate(),
        occurred_at: OffsetDateTime::now_utc(),
    };
    let run_id = Uuid::new_v4();
    start_bound_run(
        store,
        StartRunRequest {
            run_id,
            org_id: org,
            definition_id: schedule.definition_id,
            definition_version: version,
            trigger_type: TriggerType::Schedule,
            object_type: None,
            object_id: None,
            // Deterministic per (schedule, fire): a concurrent double-poll or a
            // crash-replay of the same fire starts exactly one run.
            idempotency_key: format!("schedule:{}:{}", schedule.id, fire.unix_timestamp()),
            correlation_id: format!("schedule:{}:{}", schedule.id, fire.unix_timestamp()),
            trace_id: None,
            input_payload: json!({
                "schedule_id": schedule.id,
                "schedule_label": schedule.label,
                "fired_at": fire.unix_timestamp(),
            }),
            context_payload: json!({}),
            initiated_by: None,
            schedule_id: Some(schedule.id),
        },
        &definition,
        &audit,
    )
    .await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn garbage_cron_is_rejected() {
        for garbage in [
            "",
            "   ",
            "not a cron",
            "61 * * * *",
            "* * * * * * * *",
            "0 25 * * *",
            "0 9 32 * *",
            "0 9 * 13 *",
            "; DROP TABLE workflow_schedules;",
        ] {
            assert!(
                validate_cron(garbage).is_err(),
                "cron {garbage:?} must be rejected"
            );
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn valid_cron_parses() {
        for ok in ["0 9 * * *", "*/5 * * * *", "0 0 1 * *", "0 9 * * MON-FRI"] {
            assert!(validate_cron(ok).is_ok(), "cron {ok:?} must parse");
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn unknown_timezone_is_rejected() {
        assert!(validate_timezone("Asia/Seoul").is_ok());
        assert!(validate_timezone("Mars/OlympusMons").is_err());
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn next_occurrences_respects_kst() {
        // 09:00 KST daily = 00:00 UTC. From 2026-07-08T01:00:00Z (= 10:00 KST,
        // past today's fire) the next three fires are July 9/10/11 00:00 UTC.
        let from = datetime!(2026-07-08 01:00:00 UTC);
        let fires = next_occurrences("0 9 * * *", "Asia/Seoul", from, 3).unwrap();
        assert_eq!(
            fires,
            vec![
                datetime!(2026-07-09 00:00:00 UTC),
                datetime!(2026-07-10 00:00:00 UTC),
                datetime!(2026-07-11 00:00:00 UTC),
            ]
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn next_occurrence_is_strictly_after() {
        // From exactly a fire instant, the NEXT fire is returned (exclusive).
        let at_fire = datetime!(2026-07-09 00:00:00 UTC);
        let next = next_occurrence("0 9 * * *", "Asia/Seoul", at_fire).unwrap();
        assert_eq!(next, datetime!(2026-07-10 00:00:00 UTC));
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn weekly_pattern_lands_on_monday_kst() {
        // Monday 09:00 KST. 2026-07-08 is a Wednesday; next Monday is July 13.
        let from = datetime!(2026-07-08 01:00:00 UTC);
        let fires = next_occurrences("0 9 * * MON", "Asia/Seoul", from, 2).unwrap();
        assert_eq!(
            fires,
            vec![
                datetime!(2026-07-13 00:00:00 UTC),
                datetime!(2026-07-20 00:00:00 UTC),
            ]
        );
    }
}
