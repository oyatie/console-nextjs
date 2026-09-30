//! `console-cedar-parity-report` — Cedar/PBAC enrollment parity evidence aggregator.
//!
//! Reads the append-only `authz.cedar_pbac_parity` audit rows recorded by the
//! shadow lanes (workflow decide/claim/finalize + object resolve) and prints, per
//! site (org), agree/disagree totals plus the concrete divergent cases. This is
//! the promotion evidence artifact: Cedar may be promoted to enforce for a site
//! only after a sustained zero-divergence window, which this report measures.
//!
//! Read-only. No REST surface, no OpenAPI, no client regen. It connects with
//! `DATABASE_URL` and must run as a cross-tenant read role (the ops/owner role)
//! to see every site; scope to one tenant by exporting `CEDAR_PARITY_ORG=<uuid>`
//! (arms `app.current_org` so a tenant-scoped role sees only its own rows).
//!
//! Exit code: `0` normally; `2` when `CEDAR_PARITY_FAIL_ON_DIVERGENCE=1` and any
//! divergence or malformed audit row exists (so it can double as a promotion CI
//! gate); `1` on error.

use std::process::ExitCode;

use console_app::cedar_parity::{CEDAR_PBAC_PARITY_AUDIT_ACTION, ParityObservation, aggregate};
use sqlx::Row;
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(exit) => exit,
        Err(err) => {
            eprintln!("console-cedar-parity-report: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL must be set (read-only cross-tenant connection)")?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;

    let mut conn = pool.acquire().await?;
    if let Ok(org) = std::env::var("CEDAR_PARITY_ORG") {
        // Tenant-scoped run: arm the GUC so a tenant read role sees its own rows.
        sqlx::query("SELECT set_config('app.current_org', $1, false)")
            .bind(org)
            .execute(conn.as_mut())
            .await?;
    }

    let limit = std::env::var("CEDAR_PARITY_LIMIT")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(10_000);

    let rows = sqlx::query(
        "SELECT org_id, after_snap FROM audit_events \
         WHERE action = $1 AND after_snap IS NOT NULL AND org_id IS NOT NULL \
         ORDER BY created_at DESC LIMIT $2",
    )
    .bind(CEDAR_PBAC_PARITY_AUDIT_ACTION)
    .bind(limit)
    .fetch_all(conn.as_mut())
    .await?;

    let total_rows = rows.len();
    let mut skipped_rows = 0usize;
    let observations = rows.into_iter().filter_map(|row| {
        let parsed = (|| {
            let org_id: uuid::Uuid = row.try_get("org_id").ok()?;
            let after: serde_json::Value = row.try_get("after_snap").ok()?;
            let observation: ParityObservation = serde_json::from_value(after).ok()?;
            Some((org_id.to_string(), observation))
        })();
        if parsed.is_none() {
            skipped_rows += 1;
        }
        parsed
    });

    let report = aggregate(observations);
    if skipped_rows > 0 {
        eprintln!(
            "console-cedar-parity-report: skipped {skipped_rows} unparsable row(s) out of {total_rows}"
        );
    }
    if total_rows == limit as usize {
        eprintln!(
            "console-cedar-parity-report: scanned the newest {limit} row(s); set CEDAR_PARITY_LIMIT to widen the window"
        );
    }
    println!("{}", serde_json::to_string_pretty(&report)?);

    let fail_on_divergence = std::env::var("CEDAR_PARITY_FAIL_ON_DIVERGENCE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if fail_on_divergence && (report.disagree > 0 || skipped_rows > 0) {
        eprintln!(
            "console-cedar-parity-report: {} divergence(s), {} malformed row(s) across {} site(s) — promotion blocked",
            report.disagree,
            skipped_rows,
            report.per_site.len()
        );
        return Ok(ExitCode::from(2));
    }
    Ok(ExitCode::SUCCESS)
}
