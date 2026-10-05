//! Bounded proof staging cleanup under the existing native worker role.
//! Retained identity is not deleted, and expiry never certifies family logout.
use console_kernel_core::OrgId;
use console_platform_db::{DbError, with_org_conn};
use sqlx::PgPool;
use tokio::sync::watch;
use uuid::Uuid;

const TICK_SECONDS: u64 = 30;
const COMPANY_BATCH: i32 = 100;
const PROOF_BATCH: i32 = 1000;

pub struct CleanupHandle(watch::Sender<bool>);

impl CleanupHandle {
    pub fn shutdown(&self) {
        let _ = self.0.send(true);
    }
}

pub fn spawn(pool: PgPool) -> CleanupHandle {
    let (shutdown, mut receiver) = watch::channel(false);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECONDS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut cursor = None;
        loop {
            tokio::select! {
                changed = receiver.changed() => {
                    if changed.is_err() || *receiver.borrow() { break; }
                }
                _ = tick.tick() => {
                    match run_tick(&pool, cursor).await {
                        Ok(next) => cursor = next,
                        Err(error) => {
                            metrics::counter!("browser_session_cleanup_failures_total").increment(1);
                            tracing::warn!(error = %error, "browser proof cleanup tick failed");
                        }
                    }
                }
            }
        }
    });
    CleanupHandle(shutdown)
}

async fn run_tick(pool: &PgPool, cursor: Option<Uuid>) -> Result<Option<Uuid>, DbError> {
    // rls-arming: ok fixed SECURITY DEFINER id-only discovery includes retained,
    // removed Companies; every mutation below independently arms exact scope.
    let companies: Vec<Uuid> = sqlx::query_scalar(
        "SELECT company_id FROM public.platform_browser_session_cleanup_companies($1,$2)",
    )
    .bind(cursor)
    .bind(COMPANY_BATCH)
    .fetch_all(pool)
    .await?;
    let next = if companies.len() == COMPANY_BATCH as usize {
        companies.last().copied()
    } else {
        None
    };
    for company in companies {
        let result = with_org_conn::<_, _, DbError>(pool, OrgId::from_uuid(company), move |tx| {
            Box::pin(async move {
                Ok(sqlx::query_scalar::<_, i64>(
                    "SELECT public.platform_browser_session_cleanup($1,$2)",
                )
                .bind(company)
                .bind(PROOF_BATCH)
                .fetch_one(tx.as_mut())
                .await?)
            })
        })
        .await;
        let cleared = match result {
            Ok(cleared) => cleared,
            Err(error) => {
                metrics::counter!("browser_session_cleanup_failures_total").increment(1);
                tracing::warn!(company = %company, error = %error, "browser proof cleanup Company failed");
                continue;
            }
        };
        metrics::counter!("browser_session_proofs_cleared_total").increment(cleared as u64);
        // A full batch signals remaining work without listing custody metadata.
        metrics::gauge!("browser_session_cleanup_batch_saturated", "company" => company.to_string())
            .set(if cleared == i64::from(PROOF_BATCH) { 1.0 } else { 0.0 });
    }
    metrics::counter!("browser_session_cleanup_ticks_total").increment(1);
    Ok(next)
}
