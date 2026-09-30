use std::{env, path::PathBuf};

use console_platform_jobs::{
    JobQueueError,
    soak::{DEFAULT_SOAK_JOB_COUNT, run_soak_gates, write_evidence},
};

#[tokio::main]
async fn main() -> Result<(), JobQueueError> {
    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://jasonlee@localhost/console_dev".to_owned());
    let job_count = env::var("CONSOLE_APALIS_SOAK_N")
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|err| JobQueueError::Soak(format!("invalid CONSOLE_APALIS_SOAK_N: {err}")))
        })
        .transpose()?
        .unwrap_or(DEFAULT_SOAK_JOB_COUNT);

    let report = run_soak_gates(&database_url, job_count).await?;
    if !report.passed() {
        return Err(JobQueueError::Soak(
            "one or more apalis soak gates failed".to_owned(),
        ));
    }

    let evidence_path = env::var("CONSOLE_APALIS_SOAK_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_evidence_path(report.generated_at.unix_timestamp()));
    write_evidence(&evidence_path, &report).await?;

    println!(
        "apalis soak passed: {} gates, evidence {}",
        report.gates.len(),
        evidence_path.display()
    );
    Ok(())
}

fn default_evidence_path(timestamp: i64) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../docs/evidence")
        .join(format!("apalis-soak-{timestamp}.md"))
}
