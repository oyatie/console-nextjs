#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use console_dispatch_adapter_postgres::PgDispatchStore;
use console_dispatch_application::{IncidentLocationInput, StartP1DispatchCommand};
use console_dispatch_domain::{DispatchStatus, DispatchTimerConfig};
use console_dispatch_worker::{AlimtalkEscalationPolicy, DispatchWorker};
use console_kernel_core::TraceContext;
use console_platform_jobs::{DispatchTimerJob, PlatformJob};
use console_platform_push::{
    AlimtalkMessage, BoxFuture, FcmPushMessage, ProviderMessageId, PushError, PushNotifier,
};
use sqlx::{PgPool, Row};
use time::macros::datetime;

#[path = "../../../../test_support/dispatch_worker_fixtures.rs"]
mod dispatch_worker_fixtures;

use dispatch_worker_fixtures::seed_dispatch_context;

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn timer_worker_delivers_alimtalk_and_manager_force_push(pool: PgPool) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_dispatch_context(&pool).await;
        let store = PgDispatchStore::new(pool.clone());
        let now = datetime!(2026-06-12 09:00 UTC);
        let timers = DispatchTimerConfig::default();
        let started = store
            .start_dispatch(
                StartP1DispatchCommand {
                    actor: seeded.receptionist,
                    work_order_id: seeded.work_order_id,
                    incident_location: Some(IncidentLocationInput {
                        latitude: 37.5651,
                        longitude: 126.9895,
                    }),
                    include_region: false,
                    trace: TraceContext::generate(),
                    occurred_at: now,
                },
                timers,
            )
            .await
            .unwrap();
        assert_eq!(started.target_count, 3);

        let notifier = Arc::new(RecordingNotifier::default());
        let worker = DispatchWorker::new(
            store.clone(),
            Some(notifier.clone()),
            AlimtalkEscalationPolicy::enabled(),
        );

        worker
            .handle(PlatformJob::DispatchAlimtalkNoAck(DispatchTimerJob {
                dispatch_id: started.id,
                org_id: console_kernel_core::OrgId::knl(),
                scheduled_for: now + timers.alimtalk_no_ack_after,
            }))
            .await
            .unwrap();

        assert_eq!(notifier.alimtalk_count(), 2);
        assert_eq!(
            alert_count(&pool, started.id, "ALIMTALK_NO_ACK", "SENT").await,
            2
        );

        worker
            .handle(PlatformJob::DispatchAcceptWindowExpired(DispatchTimerJob {
                dispatch_id: started.id,
                org_id: console_kernel_core::OrgId::knl(),
                scheduled_for: started.accept_window_ends_at,
            }))
            .await
            .unwrap();

        let expired = store.dispatch(started.id).await.unwrap();
        assert_eq!(expired.status, DispatchStatus::ManagerForcePending);
        assert_eq!(notifier.fcm_count(), 1);
        assert_eq!(
            alert_count(&pool, started.id, "MANAGER_FORCE_ASSIGN", "SENT").await,
            1
        );
    })
    .await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn escalation_chain_skips_unconfigured_alimtalk_flags_manual_call_and_clears_on_force_assign(
    pool: PgPool,
) {
    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_dispatch_context(&pool).await;
        let store = PgDispatchStore::new(pool.clone());
        let now = datetime!(2026-06-12 09:00 UTC);
        let timers = DispatchTimerConfig {
            accept_window: time::Duration::minutes(5),
            alimtalk_no_ack_after: time::Duration::minutes(2),
            force_assign_alert_after: time::Duration::minutes(10),
            gps_ping_freshness: time::Duration::minutes(15),
        };
        let started = store
            .start_dispatch(
                StartP1DispatchCommand {
                    actor: seeded.receptionist,
                    work_order_id: seeded.work_order_id,
                    incident_location: Some(IncidentLocationInput {
                        latitude: 37.5651,
                        longitude: 126.9895,
                    }),
                    include_region: false,
                    trace: TraceContext::generate(),
                    occurred_at: now,
                },
                timers,
            )
            .await
            .unwrap();

        let worker = DispatchWorker::new(store.clone(), None, AlimtalkEscalationPolicy::disabled());

        worker
            .handle(PlatformJob::DispatchAlimtalkNoAck(DispatchTimerJob {
                dispatch_id: started.id,
                org_id: console_kernel_core::OrgId::knl(),
                scheduled_for: now + timers.alimtalk_no_ack_after,
            }))
            .await
            .unwrap();

        assert_eq!(
            alert_count_without_sent_at(&pool, started.id, "ALIMTALK_NO_ACK", "SKIPPED").await,
            2
        );
        // FIX 7a: the disabled path skips PENDING -> SKIPPED directly and must never
        // transiently enter SENDING (no lease is claimed for non-deliverable alerts).
        assert_eq!(
            alert_count_without_sent_at(&pool, started.id, "ALIMTALK_NO_ACK", "SENDING").await,
            0
        );
        assert_eq!(
            skipped_alert_reason(&pool, started.id, "ALIMTALK_NO_ACK").await,
            "Solapi Alimtalk disabled: approved dispatch template id is not configured"
        );

        worker
            .handle(PlatformJob::DispatchAcceptWindowExpired(DispatchTimerJob {
                dispatch_id: started.id,
                org_id: console_kernel_core::OrgId::knl(),
                scheduled_for: started.accept_window_ends_at,
            }))
            .await
            .unwrap();

        let manager_pending = store.dispatch(started.id).await.unwrap();
        assert_eq!(manager_pending.status, DispatchStatus::ManagerForcePending);

        worker
            .handle(PlatformJob::DispatchManualCallRequired(DispatchTimerJob {
                dispatch_id: started.id,
                org_id: console_kernel_core::OrgId::knl(),
                scheduled_for: now + timers.force_assign_alert_after,
            }))
            .await
            .unwrap();

        let flagged = store.dispatch(started.id).await.unwrap();
        assert!(flagged.manual_call_required);
        assert_eq!(
            flagged.manual_call_required_at,
            Some(now + timers.force_assign_alert_after)
        );
        assert_eq!(flagged.manual_call_cleared_at, None);
        assert_eq!(
            audit_count(
                &pool,
                started.id,
                "dispatch.escalation.manual_call_required"
            )
            .await,
            1
        );

        let forced = store
            .force_assign(console_dispatch_application::ForceAssignP1DispatchCommand {
                actor: seeded.manager,
                dispatch_id: started.id,
                mechanic_id: seeded.near_mechanic,
                trace: TraceContext::generate(),
                occurred_at: now + time::Duration::minutes(11),
            })
            .await
            .unwrap();

        assert!(!forced.manual_call_required);
        assert_eq!(
            forced.manual_call_cleared_at,
            Some(now + time::Duration::minutes(11))
        );
    })
    .await;
}

// FIX 4: a worker crash after the provider send but before the SENT mark must
// not cause a second logical delivery. The claimed-but-unmarked alert stays
// SENDING under a lease; only after the lease expires is it reclaimed, and the
// stable idempotency key lets the provider dedupe the (at most one) retry.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn crash_after_send_yields_exactly_one_sent_row_and_stable_idempotency_key(pool: PgPool) {
    use console_dispatch_adapter_postgres::ALERT_LEASE_TTL;

    console_platform_request_context::scope_org(console_kernel_core::OrgId::knl(), async move {
        let seeded = seed_dispatch_context(&pool).await;
        let store = PgDispatchStore::new(pool.clone());
        let now = datetime!(2026-06-12 09:00 UTC);
        let timers = DispatchTimerConfig::default();
        let started = store
            .start_dispatch(
                StartP1DispatchCommand {
                    actor: seeded.receptionist,
                    work_order_id: seeded.work_order_id,
                    incident_location: Some(IncidentLocationInput {
                        latitude: 37.5651,
                        longitude: 126.9895,
                    }),
                    include_region: false,
                    trace: TraceContext::generate(),
                    occurred_at: now,
                },
                timers,
            )
            .await
            .unwrap();

        // Materialize the PENDING ALIMTALK_NO_ACK alerts (normally done by the
        // worker's no-ack handler before fanout).
        let no_ack_at = now + timers.alimtalk_no_ack_after;
        store
            .mark_alimtalk_no_ack(console_dispatch_application::ExpireP1DispatchCommand {
                dispatch_id: started.id,
                trace: TraceContext::generate(),
                occurred_at: no_ack_at,
            })
            .await
            .unwrap();

        // --- First worker run: claim + "send", then CRASH before marking SENT. ---
        let notifier = Arc::new(RecordingNotifier::default());
        let claimed = store
            .claim_alimtalk_no_ack_alerts(started.id, no_ack_at)
            .await
            .unwrap();
        assert_eq!(claimed.len(), 2, "two technicians have phones");
        let first_keys: Vec<String> = claimed.iter().map(|a| a.idempotency_key.clone()).collect();
        for alert in &claimed {
            // simulate the provider call succeeding...
            let _ = notifier
                .send_alimtalk(AlimtalkMessage {
                    to: alert.phone.clone(),
                    variables: std::collections::BTreeMap::new(),
                    idempotency_key: alert.idempotency_key.clone(),
                })
                .await;
            // ...then the worker crashes here, before mark_alert_sent.
        }
        // Alerts remain SENDING (leased), none SENT yet.
        assert_eq!(
            alert_count_without_sent_at(&pool, started.id, "ALIMTALK_NO_ACK", "SENDING").await,
            2
        );
        assert_eq!(
            alert_count(&pool, started.id, "ALIMTALK_NO_ACK", "SENT").await,
            0
        );

        // A retry BEFORE the lease expires claims nothing (no double-send window).
        let blocked = store
            .claim_alimtalk_no_ack_alerts(started.id, no_ack_at + time::Duration::seconds(1))
            .await
            .unwrap();
        assert!(blocked.is_empty(), "leased alerts must not be re-claimable");

        // --- Recovery worker run AFTER the lease expires: reclaim + deliver. ---
        let recovery_now = no_ack_at + ALERT_LEASE_TTL + time::Duration::seconds(1);
        let worker = DispatchWorker::new(
            store.clone(),
            Some(notifier.clone()),
            AlimtalkEscalationPolicy::enabled(),
        );
        // The worker reclaims expired leases inside its claim step.
        worker
            .deliver_alimtalk_no_ack_alerts_at(started.id, recovery_now)
            .await
            .unwrap();

        // Exactly one SENT row per recipient (two technicians) — no duplicates.
        assert_eq!(
            alert_count(&pool, started.id, "ALIMTALK_NO_ACK", "SENT").await,
            2
        );
        assert_eq!(
            alert_count_without_sent_at(&pool, started.id, "ALIMTALK_NO_ACK", "SENDING").await,
            0
        );

        // The idempotency key is stable across the crash + retry: the keys observed
        // by the provider on the recovery send equal the keys from the first claim.
        let sent_messages = notifier.alimtalk_messages();
        let recovery_keys: Vec<String> = sent_messages
            .iter()
            .skip(2)
            .map(|m| m.idempotency_key.clone())
            .collect();
        assert_eq!(recovery_keys.len(), 2);
        for key in &recovery_keys {
            assert!(
                first_keys.contains(key),
                "idempotency key must be stable across retries: {key}"
            );
            assert!(key.contains(':'), "key must be dispatch_id:alert_id");
        }
    })
    .await;
}

#[derive(Default)]
struct RecordingNotifier {
    fcm: Mutex<Vec<FcmPushMessage>>,
    alimtalk: Mutex<Vec<AlimtalkMessage>>,
}

impl RecordingNotifier {
    fn fcm_count(&self) -> usize {
        self.fcm.lock().unwrap().len()
    }

    fn alimtalk_count(&self) -> usize {
        self.alimtalk.lock().unwrap().len()
    }

    fn alimtalk_messages(&self) -> Vec<AlimtalkMessage> {
        self.alimtalk.lock().unwrap().clone()
    }
}

impl PushNotifier for RecordingNotifier {
    fn send_fcm<'a>(
        &'a self,
        message: FcmPushMessage,
    ) -> BoxFuture<'a, Result<ProviderMessageId, PushError>> {
        Box::pin(async move {
            let mut sent = self.fcm.lock().unwrap();
            sent.push(message);
            Ok(ProviderMessageId(format!("fcm-{}", sent.len())))
        })
    }

    fn send_alimtalk<'a>(
        &'a self,
        message: AlimtalkMessage,
    ) -> BoxFuture<'a, Result<ProviderMessageId, PushError>> {
        Box::pin(async move {
            let mut sent = self.alimtalk.lock().unwrap();
            sent.push(message);
            Ok(ProviderMessageId(format!("alimtalk-{}", sent.len())))
        })
    }
}

async fn alert_count(
    pool: &PgPool,
    dispatch_id: console_kernel_core::P1DispatchId,
    alert_type: &str,
    status: &str,
) -> i64 {
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) AS alert_count
        FROM p1_dispatch_alerts
        WHERE dispatch_id = $1
          AND alert_type = $2
          AND status = $3
          AND sent_at IS NOT NULL
          AND provider_message_id IS NOT NULL
        "#,
    )
    .bind(*dispatch_id.as_uuid())
    .bind(alert_type)
    .bind(status)
    .fetch_one(pool)
    .await
    .unwrap();
    row.try_get("alert_count").unwrap()
}

async fn alert_count_without_sent_at(
    pool: &PgPool,
    dispatch_id: console_kernel_core::P1DispatchId,
    alert_type: &str,
    status: &str,
) -> i64 {
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) AS alert_count
        FROM p1_dispatch_alerts
        WHERE dispatch_id = $1
          AND alert_type = $2
          AND status = $3
        "#,
    )
    .bind(*dispatch_id.as_uuid())
    .bind(alert_type)
    .bind(status)
    .fetch_one(pool)
    .await
    .unwrap();
    row.try_get("alert_count").unwrap()
}

async fn skipped_alert_reason(
    pool: &PgPool,
    dispatch_id: console_kernel_core::P1DispatchId,
    alert_type: &str,
) -> String {
    sqlx::query_scalar(
        r#"
        SELECT DISTINCT failure_reason
        FROM p1_dispatch_alerts
        WHERE dispatch_id = $1
          AND alert_type = $2
          AND status = 'SKIPPED'
        "#,
    )
    .bind(*dispatch_id.as_uuid())
    .bind(alert_type)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn audit_count(
    pool: &PgPool,
    dispatch_id: console_kernel_core::P1DispatchId,
    action: &str,
) -> i64 {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM audit_events
        WHERE target_type = 'p1_dispatch'
          AND target_id = $1
          AND action = $2
        "#,
    )
    .bind(dispatch_id.to_string())
    .bind(action)
    .fetch_one(pool)
    .await
    .unwrap()
}
