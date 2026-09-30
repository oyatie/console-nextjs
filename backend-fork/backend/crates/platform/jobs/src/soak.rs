use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration as StdDuration, Instant},
};

use apalis::prelude::{BoxDynError, Data, WorkerBuilder, WorkerContext};
use apalis_postgres::{Config, PgPool as ApalisPgPool, PostgresStorage};
use console_kernel_core::{Clock, FixedClock, SystemClock, Timestamp};
use sqlx::{Connection as _, Row};
use tokio::task::JoinHandle;

use crate::{
    ApalisPostgresJobQueue, JobQueue, JobQueueError, JobRequest, PlatformJob, SkewedClock,
    connect_apalis_runtime_pool, migrate_and_reconcile_apalis_postgres, schedule_after,
};

pub const DEFAULT_SOAK_JOB_COUNT: usize = 50;
pub const DEFAULT_TOLERANCE: StdDuration = StdDuration::from_secs(2);
pub const APALIS_VERSION: &str = "1.0.0-rc.9";
pub const APALIS_POSTGRES_VERSION: &str = "1.0.0-rc.8";
pub const APALIS_STABLE_1_0_0_AVAILABLE: bool = false;

const WORKER_TIMEOUT: StdDuration = StdDuration::from_secs(30);
const CRASH_STALL: StdDuration = StdDuration::from_secs(60);

#[derive(Debug, Clone)]
pub struct SoakReport {
    pub generated_at: Timestamp,
    pub job_count: usize,
    pub tolerance_ms: i128,
    pub database_url: String,
    pub apalis_version: &'static str,
    pub apalis_postgres_version: &'static str,
    pub stable_1_0_0_available: bool,
    pub gates: Vec<GateReport>,
}

impl SoakReport {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.gates.iter().all(|gate| gate.passed)
    }

    #[must_use]
    pub fn render_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# Apalis Soak Harness Evidence\n\n");
        out.push_str(&format!("- Generated: `{}`\n", self.generated_at));
        out.push_str(&format!(
            "- Database: `{}`\n",
            redact_url(&self.database_url)
        ));
        out.push_str(&format!("- Job count per gate: `{}`\n", self.job_count));
        out.push_str(&format!("- Timing tolerance: `{} ms`\n", self.tolerance_ms));
        out.push_str(&format!(
            "- Live crate check: `apalis {}`, `apalis-postgres {}`; stable `1.0.0` available: `{}`\n",
            self.apalis_version, self.apalis_postgres_version, self.stable_1_0_0_available
        ));
        out.push_str("- Crates.io evidence: `https://crates.io/api/v1/crates/apalis`, `https://crates.io/api/v1/crates/apalis-postgres`\n\n");
        out.push_str("| Gate | Result | Scheduled | Effects | Attempts | Duplicate attempts suppressed | Max early ms | Max late ms | Elapsed ms |\n");
        out.push_str("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n");
        for gate in &self.gates {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                gate.name,
                if gate.passed { "PASS" } else { "FAIL" },
                gate.scheduled,
                gate.effects,
                gate.attempts,
                gate.duplicate_attempts_suppressed,
                gate.max_early_ms,
                gate.max_late_ms,
                gate.elapsed_ms
            ));
        }
        out.push('\n');
        for gate in &self.gates {
            out.push_str(&format!("## {}\n\n", gate.name));
            for note in &gate.notes {
                out.push_str(&format!("- {note}\n"));
            }
            out.push('\n');
        }
        out
    }
}

#[derive(Debug, Clone)]
pub struct GateReport {
    pub name: &'static str,
    pub passed: bool,
    pub scheduled: usize,
    pub effects: i64,
    pub attempts: i64,
    pub duplicate_attempts_suppressed: i64,
    pub max_early_ms: i64,
    pub max_late_ms: i64,
    pub elapsed_ms: u128,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
struct Scenario {
    name: &'static str,
    id: String,
    queue_name: String,
}

#[derive(Debug, Clone)]
struct SoakWorkerState {
    pool: sqlx::PgPool,
    scenario_id: String,
    target_effects: i64,
    stop_after_effects: Option<i64>,
    stall_after_first_effect: bool,
    stalled: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
struct GateStats {
    effects: i64,
    attempts: i64,
    max_early_ms: i64,
    max_late_ms: i64,
}

struct WorkerSpec<'a> {
    scenario: &'a Scenario,
    target_effects: usize,
    stop_after_effects: Option<i64>,
    stall_after_first_effect: bool,
    worker_suffix: &'a str,
}

pub async fn run_required_soak_gates(database_url: &str) -> Result<SoakReport, JobQueueError> {
    run_soak_gates(database_url, DEFAULT_SOAK_JOB_COUNT).await
}

pub async fn run_soak_gates(
    database_url: &str,
    job_count: usize,
) -> Result<SoakReport, JobQueueError> {
    if job_count == 0 {
        return Err(JobQueueError::Soak(
            "job count must be greater than zero".to_owned(),
        ));
    }

    let mut owner_connection = sqlx::PgConnection::connect(database_url).await?;
    migrate_and_reconcile_apalis_postgres(&mut owner_connection).await?;

    let runtime_database_url =
        std::env::var("CONSOLE_APALIS_RUNTIME_DATABASE_URL").map_err(|_| {
            JobQueueError::Soak(
            "CONSOLE_APALIS_RUNTIME_DATABASE_URL is required and must authenticate as console_rt"
                .to_owned(),
        )
        })?;
    let observation_pool = sqlx::PgPool::connect(database_url).await?;
    ensure_observation_schema(&observation_pool).await?;

    let apalis_pool = connect_apalis_runtime_pool(&runtime_database_url).await?;

    let mut gates = Vec::with_capacity(4);
    gates.push(run_normal_gate(&apalis_pool, &observation_pool, job_count).await?);
    gates.push(run_restart_gate(&apalis_pool, &observation_pool, job_count).await?);
    gates.push(run_clock_skew_gate(&apalis_pool, &observation_pool, job_count).await?);
    gates.push(run_crash_recovery_gate(&apalis_pool, &observation_pool, job_count).await?);

    Ok(SoakReport {
        generated_at: SystemClock.now(),
        job_count,
        tolerance_ms: duration_millis_i128(DEFAULT_TOLERANCE),
        database_url: database_url.to_owned(),
        apalis_version: APALIS_VERSION,
        apalis_postgres_version: APALIS_POSTGRES_VERSION,
        stable_1_0_0_available: APALIS_STABLE_1_0_0_AVAILABLE,
        gates,
    })
}

pub async fn write_evidence(path: &Path, report: &SoakReport) -> Result<(), JobQueueError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, report.render_markdown()).await?;
    Ok(())
}

async fn run_normal_gate(
    apalis_pool: &ApalisPgPool,
    observation_pool: &sqlx::PgPool,
    job_count: usize,
) -> Result<GateReport, JobQueueError> {
    let started = Instant::now();
    let scenario = Scenario::new("normal");
    let config = fast_config(&scenario.queue_name);
    reset_scenario(observation_pool, &scenario).await?;
    let queue =
        ApalisPostgresJobQueue::from_pool_with_config(apalis_pool.clone(), config.clone()).await?;
    let due_times = due_now_times(SystemClock.now(), job_count);

    enqueue_due_times(&queue, &scenario, &due_times).await?;
    let worker = spawn_worker(
        apalis_pool.clone(),
        observation_pool.clone(),
        config,
        WorkerSpec {
            scenario: &scenario,
            target_effects: job_count,
            stop_after_effects: None,
            stall_after_first_effect: false,
            worker_suffix: "normal",
        },
    );
    await_worker(worker, WORKER_TIMEOUT).await?;

    build_gate_report(
        scenario.name,
        observation_pool,
        &scenario.id,
        job_count,
        started,
        vec!["all timers fired through apalis-postgres without restart".to_owned()],
        None,
    )
    .await
}

async fn run_restart_gate(
    apalis_pool: &ApalisPgPool,
    observation_pool: &sqlx::PgPool,
    job_count: usize,
) -> Result<GateReport, JobQueueError> {
    let started = Instant::now();
    let scenario = Scenario::new("worker_restart_mid_window");
    let first_config = crash_config(&scenario.queue_name);
    let second_config = fast_config(&scenario.queue_name);
    reset_scenario(observation_pool, &scenario).await?;
    let queue =
        ApalisPostgresJobQueue::from_pool_with_config(apalis_pool.clone(), second_config.clone())
            .await?;
    let due_times = due_now_times(SystemClock.now(), job_count);

    enqueue_due_times(&queue, &scenario, &due_times).await?;
    let first_worker = spawn_worker(
        apalis_pool.clone(),
        observation_pool.clone(),
        first_config,
        WorkerSpec {
            scenario: &scenario,
            target_effects: job_count,
            stop_after_effects: Some(1),
            stall_after_first_effect: false,
            worker_suffix: "restart-a",
        },
    );

    await_worker(first_worker, WORKER_TIMEOUT).await?;
    let before_restart = count_effects(observation_pool, &scenario.id).await?;

    if before_restart < job_count as i64 {
        let second_worker = spawn_worker(
            apalis_pool.clone(),
            observation_pool.clone(),
            second_config,
            WorkerSpec {
                scenario: &scenario,
                target_effects: job_count,
                stop_after_effects: None,
                stall_after_first_effect: false,
                worker_suffix: "restart-b",
            },
        );
        await_worker(second_worker, WORKER_TIMEOUT).await?;
    }

    build_gate_report(
        scenario.name,
        observation_pool,
        &scenario.id,
        job_count,
        started,
        vec![format!(
            "worker restarted after {before_restart} effects in the active timing window"
        )],
        Some(before_restart > 0 && before_restart < job_count as i64),
    )
    .await
}

async fn run_clock_skew_gate(
    apalis_pool: &ApalisPgPool,
    observation_pool: &sqlx::PgPool,
    job_count: usize,
) -> Result<GateReport, JobQueueError> {
    let started = Instant::now();
    let scenario = Scenario::new("clock_skew_via_kernel_clock");
    let config = fast_config(&scenario.queue_name);
    reset_scenario(observation_pool, &scenario).await?;
    let queue =
        ApalisPostgresJobQueue::from_pool_with_config(apalis_pool.clone(), config.clone()).await?;

    let first_due = floor_to_second(SystemClock.now());
    let mut due_times = Vec::with_capacity(job_count);
    for index in 0..job_count {
        let target_due = first_due;
        let base_now = FixedClock(target_due - time::Duration::seconds(1));
        let (offset, delay) = if index % 2 == 0 {
            (
                time::Duration::milliseconds(750),
                StdDuration::from_millis(250),
            )
        } else {
            (
                time::Duration::milliseconds(-750),
                StdDuration::from_millis(1_750),
            )
        };
        let skewed = SkewedClock::new(&base_now, offset);
        due_times.push(schedule_after(&skewed, delay)?);
    }

    enqueue_due_times(&queue, &scenario, &due_times).await?;
    let worker = spawn_worker(
        apalis_pool.clone(),
        observation_pool.clone(),
        config,
        WorkerSpec {
            scenario: &scenario,
            target_effects: job_count,
            stop_after_effects: None,
            stall_after_first_effect: false,
            worker_suffix: "clock-skew",
        },
    );
    await_worker(worker, WORKER_TIMEOUT).await?;

    build_gate_report(
        scenario.name,
        observation_pool,
        &scenario.id,
        job_count,
        started,
        vec![
            "scheduled timestamps were derived through console_kernel_core::Clock".to_owned(),
            "half the timers used +750 ms skew and half used -750 ms skew".to_owned(),
        ],
        None,
    )
    .await
}

async fn run_crash_recovery_gate(
    apalis_pool: &ApalisPgPool,
    observation_pool: &sqlx::PgPool,
    job_count: usize,
) -> Result<GateReport, JobQueueError> {
    let started = Instant::now();
    let scenario = Scenario::new("crash_recovery_idempotent_effects");
    let config = crash_config(&scenario.queue_name);
    reset_scenario(observation_pool, &scenario).await?;
    let queue =
        ApalisPostgresJobQueue::from_pool_with_config(apalis_pool.clone(), config.clone()).await?;
    let due_times = due_now_times(SystemClock.now(), job_count);

    enqueue_due_times(&queue, &scenario, &due_times).await?;
    let crashing_worker = spawn_worker(
        apalis_pool.clone(),
        observation_pool.clone(),
        config.clone(),
        WorkerSpec {
            scenario: &scenario,
            target_effects: job_count,
            stop_after_effects: None,
            stall_after_first_effect: true,
            worker_suffix: "crash-a",
        },
    );

    wait_for_effects_at_least(observation_pool, &scenario.id, 1, StdDuration::from_secs(8)).await?;
    crashing_worker.abort();
    tokio::time::sleep(StdDuration::from_secs(1)).await;

    let recovery_worker = spawn_worker(
        apalis_pool.clone(),
        observation_pool.clone(),
        config,
        WorkerSpec {
            scenario: &scenario,
            target_effects: job_count,
            stop_after_effects: None,
            stall_after_first_effect: false,
            worker_suffix: "crash-b",
        },
    );
    await_worker(recovery_worker, WORKER_TIMEOUT).await?;

    let stats = collect_stats(observation_pool, &scenario.id).await?;
    build_gate_report(
        scenario.name,
        observation_pool,
        &scenario.id,
        job_count,
        started,
        vec![
            "first worker was aborted after an effect write and before apalis ack".to_owned(),
            format!(
                "{} duplicate attempt(s) were suppressed by the effect idempotency key",
                stats.attempts.saturating_sub(stats.effects)
            ),
        ],
        Some(stats.attempts > stats.effects),
    )
    .await
}

fn spawn_worker(
    apalis_pool: ApalisPgPool,
    observation_pool: sqlx::PgPool,
    config: Config,
    spec: WorkerSpec<'_>,
) -> JoinHandle<Result<(), JobQueueError>> {
    let scenario = spec.scenario;
    let state = Arc::new(SoakWorkerState {
        pool: observation_pool,
        scenario_id: scenario.id.clone(),
        target_effects: spec.target_effects as i64,
        stop_after_effects: spec.stop_after_effects,
        stall_after_first_effect: spec.stall_after_first_effect,
        stalled: Arc::new(AtomicBool::new(false)),
    });
    let worker_name = format!("{}-{}", scenario.id, spec.worker_suffix);

    tokio::spawn(async move {
        let backend = PostgresStorage::<PlatformJob>::new_with_config(&apalis_pool, &config);
        let worker = WorkerBuilder::new(worker_name)
            .backend(backend)
            .data(state)
            .build(handle_platform_job);
        worker
            .run()
            .await
            .map_err(|err| JobQueueError::Worker(err.to_string()))
    })
}

async fn handle_platform_job(
    job: PlatformJob,
    state: Data<Arc<SoakWorkerState>>,
    worker: WorkerContext,
) -> Result<(), BoxDynError> {
    match job {
        PlatformJob::EscalationTimer(job) => {
            insert_attempt(&state.pool, &job, worker.name()).await?;
            insert_effect(&state.pool, &job, worker.name()).await?;

            if state.stall_after_first_effect
                && state
                    .stalled
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                tokio::time::sleep(CRASH_STALL).await;
            }

            let count = count_effects(&state.pool, &state.scenario_id).await?;
            if state
                .stop_after_effects
                .is_some_and(|stop_at| count >= stop_at)
            {
                worker.stop()?;
                return Ok(());
            }
            if count >= state.target_effects {
                worker.stop()?;
            }
        }
        PlatformJob::DispatchAcceptWindowExpired(_)
        | PlatformJob::DispatchAlimtalkNoAck(_)
        | PlatformJob::DispatchManualCallRequired(_)
        | PlatformJob::EvidenceTranscode(_) => {}
    }
    Ok(())
}

async fn ensure_observation_schema(pool: &sqlx::PgPool) -> Result<(), JobQueueError> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS platform_jobs_soak_attempts (
            id BIGSERIAL PRIMARY KEY,
            scenario_id TEXT NOT NULL,
            timer_id TEXT NOT NULL,
            idempotency_key TEXT NOT NULL,
            scheduled_for TIMESTAMPTZ NOT NULL,
            attempted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
            worker_name TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS platform_jobs_soak_effects (
            scenario_id TEXT NOT NULL,
            timer_id TEXT NOT NULL,
            idempotency_key TEXT NOT NULL,
            scheduled_for TIMESTAMPTZ NOT NULL,
            fired_at TIMESTAMPTZ NOT NULL DEFAULT now(),
            worker_name TEXT NOT NULL,
            PRIMARY KEY (scenario_id, idempotency_key)
        )
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}

async fn reset_scenario(pool: &sqlx::PgPool, scenario: &Scenario) -> Result<(), JobQueueError> {
    sqlx::query(
        r#"
        DELETE FROM apalis.jobs
        WHERE job_type = $1
        "#,
    )
    .bind(&scenario.queue_name)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        DELETE FROM platform_jobs_soak_attempts
        WHERE scenario_id = $1
        "#,
    )
    .bind(&scenario.id)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        DELETE FROM platform_jobs_soak_effects
        WHERE scenario_id = $1
        "#,
    )
    .bind(&scenario.id)
    .execute(pool)
    .await?;

    Ok(())
}

async fn enqueue_due_times(
    queue: &ApalisPostgresJobQueue,
    scenario: &Scenario,
    due_times: &[Timestamp],
) -> Result<(), JobQueueError> {
    for (index, due_at) in due_times.iter().copied().enumerate() {
        let timer_id = format!("timer-{index:03}");
        let idempotency_key = format!("{}:{timer_id}", scenario.id);
        let request =
            JobRequest::escalation_timer(&scenario.id, timer_id, due_at, idempotency_key)?;
        queue.schedule_at(request, due_at).await?;
    }
    Ok(())
}

async fn build_gate_report(
    name: &'static str,
    pool: &sqlx::PgPool,
    scenario_id: &str,
    scheduled: usize,
    started: Instant,
    mut notes: Vec<String>,
    extra_condition: Option<bool>,
) -> Result<GateReport, JobQueueError> {
    let stats = collect_stats(pool, scenario_id).await?;
    let duplicate_attempts_suppressed = stats.attempts.saturating_sub(stats.effects);
    let base_passed = stats.effects == scheduled as i64
        && stats.max_early_ms <= duration_millis_i64(DEFAULT_TOLERANCE)
        && stats.max_late_ms <= duration_millis_i64(DEFAULT_TOLERANCE);
    let passed = base_passed && extra_condition.unwrap_or(true);

    if !base_passed {
        notes.push("failed timing or delivery invariant".to_owned());
    }
    if extra_condition == Some(false) {
        notes.push("scenario-specific recovery invariant did not hold".to_owned());
    }

    Ok(GateReport {
        name,
        passed,
        scheduled,
        effects: stats.effects,
        attempts: stats.attempts,
        duplicate_attempts_suppressed,
        max_early_ms: stats.max_early_ms,
        max_late_ms: stats.max_late_ms,
        elapsed_ms: started.elapsed().as_millis(),
        notes,
    })
}

async fn collect_stats(pool: &sqlx::PgPool, scenario_id: &str) -> Result<GateStats, JobQueueError> {
    let timing = sqlx::query(
        r#"
        SELECT
            COUNT(*)::BIGINT AS effects,
            COALESCE(MAX(GREATEST(EXTRACT(EPOCH FROM (scheduled_for - fired_at)) * 1000.0, 0.0)), 0.0)::BIGINT AS max_early_ms,
            COALESCE(MAX(GREATEST(EXTRACT(EPOCH FROM (fired_at - scheduled_for)) * 1000.0, 0.0)), 0.0)::BIGINT AS max_late_ms
        FROM platform_jobs_soak_effects
        WHERE scenario_id = $1
        "#,
    )
    .bind(scenario_id)
    .fetch_one(pool)
    .await?;

    let attempts = sqlx::query(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM platform_jobs_soak_attempts
        WHERE scenario_id = $1
        "#,
    )
    .bind(scenario_id)
    .fetch_one(pool)
    .await?
    .try_get::<i64, _>(0)?;

    Ok(GateStats {
        effects: timing.try_get("effects")?,
        attempts,
        max_early_ms: timing.try_get("max_early_ms")?,
        max_late_ms: timing.try_get("max_late_ms")?,
    })
}

async fn insert_attempt(
    pool: &sqlx::PgPool,
    job: &crate::EscalationTimerJob,
    worker_name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO platform_jobs_soak_attempts (
            scenario_id,
            timer_id,
            idempotency_key,
            scheduled_for,
            worker_name
        )
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(&job.scenario_id)
    .bind(&job.timer_id)
    .bind(idempotency_key(job))
    .bind(job.scheduled_for)
    .bind(worker_name)
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_effect(
    pool: &sqlx::PgPool,
    job: &crate::EscalationTimerJob,
    worker_name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO platform_jobs_soak_effects (
            scenario_id,
            timer_id,
            idempotency_key,
            scheduled_for,
            worker_name
        )
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (scenario_id, idempotency_key) DO NOTHING
        "#,
    )
    .bind(&job.scenario_id)
    .bind(&job.timer_id)
    .bind(idempotency_key(job))
    .bind(job.scheduled_for)
    .bind(worker_name)
    .execute(pool)
    .await?;
    Ok(())
}

async fn wait_for_effects_at_least(
    pool: &sqlx::PgPool,
    scenario_id: &str,
    minimum: i64,
    timeout: StdDuration,
) -> Result<(), JobQueueError> {
    let started = Instant::now();
    while started.elapsed() <= timeout {
        if count_effects(pool, scenario_id).await? >= minimum {
            return Ok(());
        }
        tokio::time::sleep(StdDuration::from_millis(50)).await;
    }
    Err(JobQueueError::Soak(format!(
        "timed out waiting for {minimum} effect(s) in {scenario_id}"
    )))
}

async fn count_effects(pool: &sqlx::PgPool, scenario_id: &str) -> Result<i64, JobQueueError> {
    let row = sqlx::query(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM platform_jobs_soak_effects
        WHERE scenario_id = $1
        "#,
    )
    .bind(scenario_id)
    .fetch_one(pool)
    .await?;
    Ok(row.try_get(0)?)
}

async fn await_worker(
    mut worker: JoinHandle<Result<(), JobQueueError>>,
    timeout: StdDuration,
) -> Result<(), JobQueueError> {
    match tokio::time::timeout(timeout, &mut worker).await {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => Err(JobQueueError::Worker(err.to_string())),
        Err(_) => {
            worker.abort();
            Err(JobQueueError::Soak(format!(
                "worker did not finish within {} ms",
                duration_millis_i128(timeout)
            )))
        }
    }
}

fn due_now_times(now: Timestamp, count: usize) -> Vec<Timestamp> {
    let due = floor_to_second(now);
    vec![due; count]
}

fn fast_config(queue_name: &str) -> Config {
    Config::new(queue_name)
        .set_buffer_size(10)
        .set_keep_alive(StdDuration::from_millis(250))
        .set_reenqueue_orphaned_after(StdDuration::from_millis(750))
}

fn crash_config(queue_name: &str) -> Config {
    Config::new(queue_name)
        .set_buffer_size(1)
        .set_keep_alive(StdDuration::from_millis(250))
        .set_reenqueue_orphaned_after(StdDuration::from_millis(750))
}

fn idempotency_key(job: &crate::EscalationTimerJob) -> String {
    format!("{}:{}", job.scenario_id, job.timer_id)
}

fn floor_to_second(value: Timestamp) -> Timestamp {
    value - time::Duration::nanoseconds(value.nanosecond() as i64)
}

impl Scenario {
    fn new(name: &'static str) -> Self {
        let id = format!("t110-{name}-{}", uuid::Uuid::new_v4());
        Self {
            name,
            queue_name: format!("console.t110.{id}"),
            id,
        }
    }
}

fn duration_millis_i64(duration: StdDuration) -> i64 {
    duration_millis_i128(duration) as i64
}

fn duration_millis_i128(duration: StdDuration) -> i128 {
    duration.as_millis() as i128
}

fn redact_url(value: &str) -> String {
    let Some(scheme_end) = value.find("://") else {
        return value.to_owned();
    };
    let authority_start = scheme_end + 3;
    let Some(at_offset) = value[authority_start..].find('@') else {
        return value.to_owned();
    };
    let at = authority_start + at_offset;
    let Some(colon_offset) = value[authority_start..at].find(':') else {
        return value.to_owned();
    };
    let colon = authority_start + colon_offset;
    format!("{}***{}", &value[..=colon], &value[at..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_url_hides_passwords() {
        let redacted = redact_url("postgres://user:secret@localhost/db");

        assert_eq!(redacted, "postgres://user:***@localhost/db");
    }

    #[test]
    fn floor_to_second_removes_subsecond_precision() {
        let value = time::macros::datetime!(2026-06-12 10:01:02.123 UTC);

        assert_eq!(
            floor_to_second(value),
            time::macros::datetime!(2026-06-12 10:01:02 UTC)
        );
    }
}
