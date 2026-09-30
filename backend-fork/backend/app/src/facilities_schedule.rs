//! Scheduled HVAC occurrence poller. The facilities crate owns the locked,
//! tenant-armed materialization transaction; app composition owns its lifetime.

use std::time::Duration;

use tokio::sync::watch;

pub struct FacilitiesScheduleHandle {
    shutdown_tx: watch::Sender<bool>,
}

impl FacilitiesScheduleHandle {
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

#[must_use]
pub fn spawn(pool: sqlx::PgPool) -> FacilitiesScheduleHandle {
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() { break; }
                }
                _ = ticker.tick() => match console_facilities_rest::poll_scheduled_hvac(&pool).await {
                    Ok(0) => {},
                    Ok(created) => tracing::info!(created, "facilities scheduled HVAC occurrences materialized"),
                    Err(error) => tracing::warn!(%error, "facilities scheduled HVAC occurrence poll failed"),
                }
            }
        }
    });
    FacilitiesScheduleHandle { shutdown_tx }
}
