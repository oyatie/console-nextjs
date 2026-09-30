//! Dispatch timer worker handlers.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::BTreeMap;
use std::sync::Arc;

use console_dispatch_adapter_postgres::{
    PendingAlimtalkAlert, PendingFcmPush, PgDispatchError, PgDispatchStore,
};
use console_dispatch_application::ExpireP1DispatchCommand;
use console_kernel_core::{OrgId, P1DispatchAlertId, TraceContext};
use console_platform_jobs::{
    BoxFuture, DispatchTimerJob, JobQueueError, PlatformJob, PlatformJobHandler,
};
use console_platform_push::{AlimtalkMessage, FcmPushMessage, PushError, PushNotifier};

#[derive(Debug, thiserror::Error)]
pub enum DispatchWorkerError {
    #[error(transparent)]
    Dispatch(#[from] PgDispatchError),

    #[error("unsupported job for dispatch worker")]
    UnsupportedJob,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlimtalkEscalationPolicy {
    enabled: bool,
}

impl AlimtalkEscalationPolicy {
    #[must_use]
    pub const fn enabled() -> Self {
        Self { enabled: true }
    }

    #[must_use]
    pub const fn disabled() -> Self {
        Self { enabled: false }
    }

    #[must_use]
    pub const fn is_enabled(self) -> bool {
        self.enabled
    }
}

const ALIMTALK_DISABLED_REASON: &str =
    "Solapi Alimtalk disabled: approved dispatch template id is not configured";

#[derive(Clone)]
pub struct DispatchWorker {
    store: PgDispatchStore,
    push_notifier: Option<Arc<dyn PushNotifier>>,
    alimtalk_policy: AlimtalkEscalationPolicy,
}

impl DispatchWorker {
    #[must_use]
    pub fn new(
        store: PgDispatchStore,
        push_notifier: Option<Arc<dyn PushNotifier>>,
        alimtalk_policy: AlimtalkEscalationPolicy,
    ) -> Self {
        Self {
            store,
            push_notifier,
            alimtalk_policy,
        }
    }

    pub async fn handle(&self, job: PlatformJob) -> Result<(), DispatchWorkerError> {
        // The timer worker is a background (non-request) processor, so it has no
        // request task-local org. Its adapter reads/writes still need
        // `app.current_org` armed under the non-owner RLS role, so we enter the
        // tenant scope here — using the org carried ON THE JOB PAYLOAD (the
        // dispatch's tenant), so background processing is tenant-correct for ANY
        // tenant, not just the bootstrap one. A job with no org (legacy payload)
        // defaults to KNL via serde, preserving single-tenant behavior.
        let org = job_org(&job);
        console_platform_request_context::scope_org(org, self.handle_inner(job)).await
    }

    async fn handle_inner(&self, job: PlatformJob) -> Result<(), DispatchWorkerError> {
        match job {
            PlatformJob::DispatchAcceptWindowExpired(job) => {
                self.handle_accept_window(job).await?;
                Ok(())
            }
            PlatformJob::DispatchAlimtalkNoAck(job) => {
                self.handle_alimtalk_no_ack(job).await?;
                Ok(())
            }
            PlatformJob::DispatchManualCallRequired(job) => {
                self.handle_manual_call_required(job).await?;
                Ok(())
            }
            PlatformJob::EscalationTimer(_) | PlatformJob::EvidenceTranscode(_) => {
                Err(DispatchWorkerError::UnsupportedJob)
            }
        }
    }

    async fn handle_accept_window(&self, job: DispatchTimerJob) -> Result<(), DispatchWorkerError> {
        self.store
            .expire_accept_window(ExpireP1DispatchCommand {
                dispatch_id: job.dispatch_id,
                trace: TraceContext::generate(),
                occurred_at: job.scheduled_for,
            })
            .await?;
        self.deliver_manager_force_alerts(job.dispatch_id, job.scheduled_for)
            .await?;
        Ok(())
    }

    async fn handle_alimtalk_no_ack(
        &self,
        job: DispatchTimerJob,
    ) -> Result<(), DispatchWorkerError> {
        self.store
            .mark_alimtalk_no_ack(ExpireP1DispatchCommand {
                dispatch_id: job.dispatch_id,
                trace: TraceContext::generate(),
                occurred_at: job.scheduled_for,
            })
            .await?;
        self.deliver_alimtalk_no_ack_alerts_at(job.dispatch_id, job.scheduled_for)
            .await?;
        Ok(())
    }

    async fn handle_manual_call_required(
        &self,
        job: DispatchTimerJob,
    ) -> Result<(), DispatchWorkerError> {
        self.store
            .mark_manual_call_required(ExpireP1DispatchCommand {
                dispatch_id: job.dispatch_id,
                trace: TraceContext::generate(),
                occurred_at: job.scheduled_for,
            })
            .await?;
        Ok(())
    }

    /// Deliver pending ALIMTALK_NO_ACK alerts, claiming a lease per alert at the
    /// supplied logical `now` (also used to reclaim crashed leases). Exposed for
    /// deterministic crash-recovery testing.
    pub async fn deliver_alimtalk_no_ack_alerts_at(
        &self,
        dispatch_id: console_kernel_core::P1DispatchId,
        now: time::OffsetDateTime,
    ) -> Result<(), DispatchWorkerError> {
        // When delivery is disabled we must not claim leases for delivery — skip
        // the still-PENDING alerts directly (PENDING -> SKIPPED) so they never
        // transiently enter SENDING.
        let notifier = match (
            self.alimtalk_policy.is_enabled(),
            self.push_notifier.as_ref(),
        ) {
            (true, Some(notifier)) => notifier,
            _ => {
                self.store
                    .skip_pending_alimtalk_no_ack_alerts(
                        dispatch_id,
                        ALIMTALK_DISABLED_REASON,
                        TraceContext::generate(),
                        now,
                    )
                    .await?;
                return Ok(());
            }
        };
        let alerts = self
            .store
            .claim_alimtalk_no_ack_alerts(dispatch_id, now)
            .await?;
        for alert in alerts {
            self.send_alimtalk(notifier.as_ref(), alert, "P1 emergency dispatch")
                .await?;
        }
        Ok(())
    }

    async fn deliver_manager_force_alerts(
        &self,
        dispatch_id: console_kernel_core::P1DispatchId,
        now: time::OffsetDateTime,
    ) -> Result<(), DispatchWorkerError> {
        let Some(notifier) = self.push_notifier.as_ref() else {
            return Ok(());
        };
        let pushes = self
            .store
            .claim_fcm_pushes(dispatch_id, "MANAGER_FORCE_ASSIGN", now)
            .await?;
        for push in pushes {
            self.send_manager_push(notifier.as_ref(), push).await?;
        }
        let fallback_alerts = self
            .store
            .claim_manager_force_alimtalks(dispatch_id, now)
            .await?;
        for alert in fallback_alerts {
            if self.alimtalk_policy.is_enabled() {
                self.send_alimtalk(
                    notifier.as_ref(),
                    alert,
                    "P1 dispatch needs manager assignment",
                )
                .await?;
            } else {
                let alert_id = alert.alert_id;
                let lease_held = self
                    .store
                    .mark_alert_skipped(
                        alert_id,
                        Some(alert.lease_token),
                        ALIMTALK_DISABLED_REASON.to_owned(),
                        TraceContext::generate(),
                        time::OffsetDateTime::now_utc(),
                    )
                    .await?;
                warn_if_lease_lost(lease_held, alert_id);
            }
        }
        Ok(())
    }

    async fn send_manager_push(
        &self,
        notifier: &dyn PushNotifier,
        push: PendingFcmPush,
    ) -> Result<(), DispatchWorkerError> {
        let message = FcmPushMessage {
            token: push.push_token,
            title: "P1 dispatch requires assignment".to_owned(),
            body: "No technician accepted before the deadline".to_owned(),
            data: BTreeMap::from([
                ("type".to_owned(), "p1_dispatch_manager_force".to_owned()),
                ("dispatch_id".to_owned(), push.dispatch_id.to_string()),
                ("work_order_id".to_owned(), push.work_order_id.to_string()),
            ]),
            idempotency_key: push.idempotency_key,
        };
        let alert_id = push.alert_id;
        match notifier.send_fcm(message).await {
            Ok(provider_id) => {
                let lease_held = self
                    .store
                    .mark_alert_sent(
                        alert_id,
                        push.lease_token,
                        non_empty_provider_id(provider_id.0),
                        TraceContext::generate(),
                        time::OffsetDateTime::now_utc(),
                    )
                    .await?;
                warn_if_lease_lost(lease_held, alert_id);
            }
            Err(err) => {
                let lease_held = self
                    .store
                    .mark_alert_failed(
                        alert_id,
                        push.lease_token,
                        provider_failure_reason(err),
                        TraceContext::generate(),
                        time::OffsetDateTime::now_utc(),
                    )
                    .await?;
                warn_if_lease_lost(lease_held, alert_id);
            }
        }
        Ok(())
    }

    async fn send_alimtalk(
        &self,
        notifier: &dyn PushNotifier,
        alert: PendingAlimtalkAlert,
        title: &'static str,
    ) -> Result<(), DispatchWorkerError> {
        let message = AlimtalkMessage {
            to: alert.phone,
            variables: BTreeMap::from([
                ("title".to_owned(), title.to_owned()),
                ("dispatch_id".to_owned(), alert.dispatch_id.to_string()),
                ("work_order_id".to_owned(), alert.work_order_id.to_string()),
            ]),
            idempotency_key: alert.idempotency_key,
        };
        let alert_id = alert.alert_id;
        match notifier.send_alimtalk(message).await {
            Ok(provider_id) => {
                let lease_held = self
                    .store
                    .mark_alert_sent(
                        alert_id,
                        alert.lease_token,
                        non_empty_provider_id(provider_id.0),
                        TraceContext::generate(),
                        time::OffsetDateTime::now_utc(),
                    )
                    .await?;
                warn_if_lease_lost(lease_held, alert_id);
            }
            Err(PushError::Config(message)) => {
                let lease_held = self
                    .store
                    .mark_alert_skipped(
                        alert_id,
                        Some(alert.lease_token),
                        format!("Solapi Alimtalk disabled: {message}"),
                        TraceContext::generate(),
                        time::OffsetDateTime::now_utc(),
                    )
                    .await?;
                warn_if_lease_lost(lease_held, alert_id);
            }
            Err(err) => {
                let lease_held = self
                    .store
                    .mark_alert_failed(
                        alert_id,
                        alert.lease_token,
                        provider_failure_reason(err),
                        TraceContext::generate(),
                        time::OffsetDateTime::now_utc(),
                    )
                    .await?;
                warn_if_lease_lost(lease_held, alert_id);
            }
        }
        Ok(())
    }
}

impl PlatformJobHandler for DispatchWorker {
    fn handle<'a>(&'a self, job: PlatformJob) -> BoxFuture<'a, Result<(), JobQueueError>> {
        Box::pin(async move {
            DispatchWorker::handle(self, job)
                .await
                .map_err(|err| JobQueueError::Worker(err.to_string()))
        })
    }
}

/// The tenant a job belongs to, read off its payload. Every dispatch-timer job
/// carries it; the escalation-timer job has no tenant context and falls back to
/// the bootstrap tenant (it is unsupported by this worker anyway).
fn job_org(job: &PlatformJob) -> OrgId {
    match job {
        PlatformJob::DispatchAcceptWindowExpired(j)
        | PlatformJob::DispatchAlimtalkNoAck(j)
        | PlatformJob::DispatchManualCallRequired(j) => j.org_id,
        PlatformJob::EvidenceTranscode(j) => j.org_id,
        PlatformJob::EscalationTimer(_) => OrgId::knl(),
    }
}

fn non_empty_provider_id(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

/// Consume the lost-lease signal from a `mark_alert_*` transition. `false` means
/// this worker no longer held the lease (it was reclaimed after a crash, e.g.
/// past the lease TTL) so the transition was a no-op handled elsewhere; surface
/// it so an operator can see the designed double-handling guard firing.
fn warn_if_lease_lost(lease_held: bool, alert_id: P1DispatchAlertId) {
    if !lease_held {
        tracing::warn!(
            %alert_id,
            "alert lease lost before status mark; transition was a no-op (reclaimed elsewhere)"
        );
    }
}

fn provider_failure_reason(err: PushError) -> String {
    let message = err.to_string();
    if message.len() > 512 {
        message.chars().take(512).collect()
    } else {
        message
    }
}
