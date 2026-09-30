use console_app::{AppConfig, AppRole, AppState, init_tracing, run_migrations, serve};

#[tokio::main]
async fn main() -> Result<(), console_app::AppError> {
    let config = AppConfig::from_env()?;
    let telemetry = init_tracing(&config)?;

    // Migrate mode is a one-shot OWNER DDL run (Argo PreSync Job): it needs only
    // DATABASE_URL and must NOT build the full AppState (no JWT/S3 wiring, no HTTP
    // server). Dispatch it before `from_config` so a migration Job can run with a
    // minimal environment, then exit.
    if config.role == AppRole::Migrate {
        let result = run_migrations(&config).await;
        telemetry.shutdown();
        return result;
    }

    let state = AppState::from_config(config.clone()).await?;
    let result = serve(config, state).await;
    telemetry.shutdown();
    result
}
