#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use apalis_sqlx::Row as _;
use console_platform_jobs::{
    ApalisPostgresJobQueue, BoxFuture, JobQueue, JobQueueError, JobRequest, PlatformJob,
    PlatformJobHandler, connect_apalis_runtime_pool, migrate_and_reconcile_apalis_postgres,
    run_apalis_worker_until_shutdown,
};
use sqlx::{Connection as _, Row};

struct RecordingHandler {
    delivered: tokio::sync::mpsc::UnboundedSender<PlatformJob>,
}

impl PlatformJobHandler for RecordingHandler {
    fn handle<'a>(&'a self, job: PlatformJob) -> BoxFuture<'a, Result<(), JobQueueError>> {
        Box::pin(async move {
            self.delivered
                .send(job)
                .map_err(|_| JobQueueError::Worker("recording receiver closed".to_owned()))
        })
    }
}

#[derive(Clone)]
struct LedgerRow {
    version: i64,
    description: String,
    installed_on: time::OffsetDateTime,
    success: bool,
    checksum: Vec<u8>,
    execution_time: i64,
}

#[tokio::test]
async fn owner_and_runtime_apalis_schema_contract_is_fail_closed() {
    let owner_url = required_url("CONSOLE_APALIS_OWNER_DATABASE_URL");
    let runtime_url = required_url("CONSOLE_APALIS_RUNTIME_DATABASE_URL");
    let admin_url = required_url("CONSOLE_APALIS_ADMIN_DATABASE_URL");
    reassert_serving_role_passwords(&owner_url, &runtime_url).await;
    let mut owner = sqlx::PgConnection::connect(&owner_url)
        .await
        .expect("connect as console_app");

    let database_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&mut owner)
        .await
        .expect("read database name");
    assert!(
        database_name.ends_with("_contract"),
        "destructive Apalis contract test requires an isolated *_contract database"
    );

    sqlx::query("SELECT pg_advisory_lock(901012)")
        .execute(&mut owner)
        .await
        .expect("serialize destructive contract test");

    reset_apalis(&mut owner).await;
    sqlx::raw_sql("CREATE SCHEMA apalis; CREATE TABLE apalis.jobs (partial INTEGER)")
        .execute(&mut owner)
        .await
        .expect("create deliberately partial unledgered schema");
    let partial_error = migrate_and_reconcile_apalis_postgres(&mut owner)
        .await
        .expect_err("unledgered partial schema must be refused");
    assert!(partial_error.to_string().contains("unledgered"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM information_schema.columns WHERE table_schema = 'apalis'",
        )
        .fetch_one(&mut owner)
        .await
        .expect("count partial columns"),
        1,
        "owner refusal must not repair the partial schema"
    );
    assert!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT to_regclass('apalis.platform_jobs_apalis_migrations')::text",
        )
        .fetch_one(&mut owner)
        .await
        .expect("read absent ledger")
        .is_none(),
        "owner refusal must not create a ledger"
    );

    reset_apalis(&mut owner).await;
    sqlx::raw_sql(
        "CREATE FUNCTION public.generate_ulid() RETURNS TEXT LANGUAGE sql IMMUTABLE AS 'SELECT ''partial'''",
    )
    .execute(&mut owner)
    .await
    .expect("create lone adapter-owned public helper");
    let helper_definition: String =
        sqlx::query_scalar("SELECT pg_get_functiondef('public.generate_ulid()'::regprocedure)")
            .fetch_one(&mut owner)
            .await
            .expect("snapshot lone public helper");
    let helper_error = migrate_and_reconcile_apalis_postgres(&mut owner)
        .await
        .expect_err("lone public helper without a ledger must be refused");
    assert!(helper_error.to_string().contains("unledgered"));
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT pg_get_functiondef('public.generate_ulid()'::regprocedure)",
        )
        .fetch_one(&mut owner)
        .await
        .expect("read lone helper after refusal"),
        helper_definition,
        "owner refusal must not replace the lone public helper"
    );
    assert!(
        sqlx::query_scalar::<_, Option<String>>("SELECT to_regnamespace('apalis')::text",)
            .fetch_one(&mut owner)
            .await
            .expect("read absent Apalis schema")
            .is_none(),
        "owner refusal must not create the Apalis schema"
    );
    sqlx::query("DROP FUNCTION public.generate_ulid()")
        .execute(&mut owner)
        .await
        .expect("remove lone helper fixture");

    reset_apalis(&mut owner).await;
    sqlx::raw_sql("REVOKE ALL PRIVILEGES ON SCHEMA public FROM PUBLIC, console_rt")
        .execute(&mut owner)
        .await
        .expect("remove runtime access to the public schema");
    migrate_and_reconcile_apalis_postgres(&mut owner)
        .await
        .expect("fresh owner migration succeeds");
    migrate_and_reconcile_apalis_postgres(&mut owner)
        .await
        .expect("owner migration rerun is idempotent");
    assert_reconciliation_cancellation_releases_lock(&owner_url).await;

    let (mut owner_a, mut owner_b) = (
        sqlx::PgConnection::connect(&owner_url)
            .await
            .expect("connect first concurrent owner"),
        sqlx::PgConnection::connect(&owner_url)
            .await
            .expect("connect second concurrent owner"),
    );
    let (first, second) = tokio::join!(
        migrate_and_reconcile_apalis_postgres(&mut owner_a),
        migrate_and_reconcile_apalis_postgres(&mut owner_b)
    );
    first.expect("first concurrent owner migration succeeds");
    second.expect("second concurrent owner migration succeeds");

    assert_runtime_least_privilege(&runtime_url).await;
    assert_runtime_pool_resets_poisoned_session(&runtime_url).await;
    assert_runtime_can_enqueue(&runtime_url, &mut owner).await;
    assert_runtime_worker_delivers_and_shuts_down(&runtime_url, &mut owner).await;

    let expected = latest_ledger_row(&mut owner).await;
    sqlx::query("DELETE FROM apalis.platform_jobs_apalis_migrations WHERE version = $1")
        .bind(expected.version)
        .execute(&mut owner)
        .await
        .expect("remove known migration row");
    assert_runtime_refuses(&runtime_url, "missing migration").await;
    assert!(
        ledger_row(&mut owner, expected.version).await.is_none(),
        "runtime validation must not restore a missing migration"
    );
    restore_ledger_row(&mut owner, &expected).await;

    sqlx::query(
        "UPDATE apalis.platform_jobs_apalis_migrations SET success = FALSE WHERE version = $1",
    )
    .bind(expected.version)
    .execute(&mut owner)
    .await
    .expect("mark migration unsuccessful");
    let unsuccessful_snapshot = apalis_state_snapshot(&mut owner).await;
    assert_runtime_refuses(&runtime_url, "unsuccessful migration").await;
    assert_eq!(
        apalis_state_snapshot(&mut owner).await,
        unsuccessful_snapshot,
        "runtime refusal must not mutate an unsuccessful ledger"
    );
    restore_ledger_row(&mut owner, &expected).await;

    sqlx::query(
        "UPDATE apalis.platform_jobs_apalis_migrations SET checksum = decode('00', 'hex') WHERE version = $1",
    )
    .bind(expected.version)
    .execute(&mut owner)
    .await
    .expect("corrupt checksum");
    assert_runtime_refuses(&runtime_url, "checksum mismatch").await;
    assert_eq!(
        ledger_row(&mut owner, expected.version)
            .await
            .expect("corrupt row remains")
            .checksum,
        vec![0],
        "runtime validation must not repair a checksum"
    );
    restore_ledger_row(&mut owner, &expected).await;

    sqlx::query(
        "UPDATE apalis.platform_jobs_apalis_migrations SET description = 'tampered' WHERE version = $1",
    )
    .bind(expected.version)
    .execute(&mut owner)
    .await
    .expect("corrupt description");
    let tampered_snapshot = apalis_state_snapshot(&mut owner).await;
    assert_runtime_refuses(&runtime_url, "description mismatch").await;
    assert_eq!(
        ledger_row(&mut owner, expected.version)
            .await
            .expect("tampered row remains")
            .description,
        "tampered",
        "runtime validation must not repair a description"
    );
    let owner_tamper_error = migrate_and_reconcile_apalis_postgres(&mut owner)
        .await
        .expect_err("owner must preflight a tampered description");
    assert!(
        owner_tamper_error
            .to_string()
            .contains("description mismatch")
    );
    assert_eq!(
        apalis_state_snapshot(&mut owner).await,
        tampered_snapshot,
        "failed owner preflight must not mutate schema, ACL, or ledger"
    );
    restore_ledger_row(&mut owner, &expected).await;

    sqlx::query("ALTER TABLE apalis.jobs DISABLE TRIGGER notify_workers")
        .execute(&mut owner)
        .await
        .expect("disable required notify trigger");
    assert_runtime_refuses(&runtime_url, "disabled notify trigger").await;
    let trigger_enabled: String = sqlx::query_scalar(
        "SELECT tgenabled::text FROM pg_trigger WHERE tgrelid = 'apalis.jobs'::regclass AND tgname = 'notify_workers'",
    )
    .fetch_one(&mut owner)
    .await
    .expect("read disabled trigger state");
    assert_eq!(trigger_enabled, "D", "runtime must not repair the trigger");
    sqlx::query("ALTER TABLE apalis.jobs ENABLE TRIGGER notify_workers")
        .execute(&mut owner)
        .await
        .expect("restore required notify trigger");

    sqlx::query("DROP INDEX apalis.idx_jobs_idempotency_key")
        .execute(&mut owner)
        .await
        .expect("remove required idempotency index");
    assert_runtime_refuses(&runtime_url, "missing idempotency index").await;
    assert!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT to_regclass('apalis.idx_jobs_idempotency_key')::text",
        )
        .fetch_one(&mut owner)
        .await
        .expect("read missing index")
        .is_none(),
        "runtime must not recreate a missing index"
    );
    sqlx::query(
        "CREATE UNIQUE INDEX idx_jobs_idempotency_key ON apalis.jobs(job_type, idempotency_key)",
    )
    .execute(&mut owner)
    .await
    .expect("restore idempotency index");

    sqlx::query("ALTER TABLE apalis.jobs DROP CONSTRAINT fk_worker_lock_by")
        .execute(&mut owner)
        .await
        .expect("remove required worker foreign key");
    assert_runtime_refuses(&runtime_url, "missing worker foreign key").await;
    let foreign_key_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_constraint WHERE conrelid = 'apalis.jobs'::regclass AND conname = 'fk_worker_lock_by')",
    )
    .fetch_one(&mut owner)
    .await
    .expect("read missing worker foreign key");
    assert!(
        !foreign_key_exists,
        "runtime must not recreate the foreign key"
    );
    sqlx::query(
        "ALTER TABLE apalis.jobs ADD CONSTRAINT fk_worker_lock_by FOREIGN KEY(lock_by) REFERENCES apalis.workers(id)",
    )
    .execute(&mut owner)
    .await
    .expect("restore worker foreign key");

    let get_jobs_definition: String = sqlx::query_scalar(
        "SELECT pg_get_functiondef('apalis.get_jobs(text,text,integer)'::regprocedure)",
    )
    .fetch_one(&mut owner)
    .await
    .expect("read current get_jobs definition");
    sqlx::raw_sql(
        r#"
        CREATE OR REPLACE FUNCTION apalis.get_jobs(
            worker_id TEXT,
            v_job_type TEXT,
            v_job_count INTEGER DEFAULT 5
        ) RETURNS SETOF apalis.jobs AS $$
        BEGIN
            RETURN;
        END;
        $$ LANGUAGE plpgsql VOLATILE
        "#,
    )
    .execute(&mut owner)
    .await
    .expect("tamper get_jobs body without changing its shape");
    assert_runtime_refuses(&runtime_url, "tampered get_jobs body").await;
    let tampered_body: String = sqlx::query_scalar(
        "SELECT prosrc FROM pg_proc WHERE oid = 'apalis.get_jobs(text,text,integer)'::regprocedure",
    )
    .fetch_one(&mut owner)
    .await
    .expect("read tampered get_jobs body");
    assert!(tampered_body.contains("RETURN;"));
    // The definition is read directly from pg_get_functiondef for this fixed
    // adapter-owned function; no user or configuration input is interpolated.
    sqlx::raw_sql(sqlx::AssertSqlSafe(get_jobs_definition))
        .execute(&mut owner)
        .await
        .expect("restore get_jobs definition");

    let later_version: i64 =
        sqlx::query_scalar("SELECT MAX(version) + 1 FROM apalis.platform_jobs_apalis_migrations")
            .fetch_one(&mut owner)
            .await
            .expect("calculate later version");
    sqlx::query(
        r#"
        INSERT INTO apalis.platform_jobs_apalis_migrations
            (version, description, success, checksum, execution_time)
        VALUES ($1, 'future successful migration', TRUE, decode('01', 'hex'), 0)
        "#,
    )
    .bind(later_version)
    .execute(&mut owner)
    .await
    .expect("insert forward migration row");
    sqlx::query("ALTER TABLE apalis.jobs ADD COLUMN future_additive TEXT")
        .execute(&mut owner)
        .await
        .expect("add forward-compatible column");
    sqlx::raw_sql(
        r#"
        CREATE TABLE apalis.future_jobs_contract (id INTEGER PRIMARY KEY);
        CREATE FUNCTION apalis.future_contract_helper() RETURNS INTEGER
        LANGUAGE sql IMMUTABLE AS 'SELECT 1';
        GRANT SELECT ON TABLE apalis.future_jobs_contract TO console_rt;
        GRANT EXECUTE ON FUNCTION apalis.future_contract_helper() TO console_rt;
        "#,
    )
    .execute(&mut owner)
    .await
    .expect("create and grant future objects");
    migrate_and_reconcile_apalis_postgres(&mut owner)
        .await
        .expect("owner reconciles ACL after additive migration");
    let future_grants_preserved: bool = sqlx::query_scalar(
        r#"
        SELECT has_table_privilege('console_rt', 'apalis.future_jobs_contract', 'SELECT')
           AND has_function_privilege('console_rt', 'apalis.future_contract_helper()', 'EXECUTE')
        "#,
    )
    .fetch_one(&mut owner)
    .await
    .expect("read future object grants");
    assert!(
        future_grants_preserved,
        "an older owner reconciliation must preserve future object grants"
    );
    ApalisPostgresJobQueue::connect(&runtime_url, "contract.forward")
        .await
        .expect("runtime accepts successful later migration and additive column");
    sqlx::query("ALTER TABLE apalis.jobs DROP COLUMN future_additive")
        .execute(&mut owner)
        .await
        .expect("remove additive test column");
    sqlx::raw_sql(
        "DROP FUNCTION apalis.future_contract_helper(); DROP TABLE apalis.future_jobs_contract",
    )
    .execute(&mut owner)
    .await
    .expect("remove future test objects");
    sqlx::query("DELETE FROM apalis.platform_jobs_apalis_migrations WHERE version = $1")
        .bind(later_version)
        .execute(&mut owner)
        .await
        .expect("remove forward test migration");

    let gap_version: i64 = sqlx::query_scalar(
        "SELECT version FROM apalis.platform_jobs_apalis_migrations ORDER BY version DESC OFFSET 1 LIMIT 1",
    )
    .fetch_one(&mut owner)
    .await
    .expect("select known migration to create a gap");
    let gap_row = ledger_row(&mut owner, gap_version)
        .await
        .expect("read known migration selected for gap");
    sqlx::query("DELETE FROM apalis.platform_jobs_apalis_migrations WHERE version = $1")
        .bind(gap_version)
        .execute(&mut owner)
        .await
        .expect("create known migration gap");
    sqlx::query(
        "INSERT INTO apalis.platform_jobs_apalis_migrations (version, description, success, checksum, execution_time) VALUES ($1, 'later drift', TRUE, decode('77', 'hex'), 0)",
    )
    .bind(later_version)
    .execute(&mut owner)
    .await
    .expect("insert later row above known gap");
    let gap_snapshot = apalis_state_snapshot(&mut owner).await;
    migrate_and_reconcile_apalis_postgres(&mut owner)
        .await
        .expect_err("owner must reject a known gap before a later row");
    assert_eq!(
        apalis_state_snapshot(&mut owner).await,
        gap_snapshot,
        "gap refusal must not mutate schema, ACL, or ledger"
    );
    sqlx::query("DELETE FROM apalis.platform_jobs_apalis_migrations WHERE version = $1")
        .bind(later_version)
        .execute(&mut owner)
        .await
        .expect("remove later drift row");
    restore_ledger_row(&mut owner, &gap_row).await;

    sqlx::raw_sql(
        r#"
        CREATE TABLE public.platform_jobs_apalis_migrations
            (LIKE apalis.platform_jobs_apalis_migrations INCLUDING ALL);
        INSERT INTO public.platform_jobs_apalis_migrations
        SELECT * FROM apalis.platform_jobs_apalis_migrations;
        "#,
    )
    .execute(&mut owner)
    .await
    .expect("create matching legacy ledger");
    sqlx::query(
        "INSERT INTO apalis.platform_jobs_apalis_migrations (version, description, success, checksum, execution_time) VALUES ($1, 'canonical future', TRUE, decode('88', 'hex'), 0)",
    )
    .bind(later_version)
    .execute(&mut owner)
    .await
    .expect("insert canonical future row");
    sqlx::query(
        "INSERT INTO public.platform_jobs_apalis_migrations (version, description, success, checksum, execution_time) VALUES ($1, 'conflicting legacy future', TRUE, decode('99', 'hex'), 0)",
    )
    .bind(later_version)
    .execute(&mut owner)
    .await
    .expect("insert conflicting legacy future row");
    migrate_and_reconcile_apalis_postgres(&mut owner)
        .await
        .expect_err("owner must reject conflicting overlapping ledger rows");
    assert_eq!(
        ledger_row(&mut owner, later_version)
            .await
            .expect("canonical future row remains after refusal")
            .description,
        "canonical future",
        "legacy conflict refusal must not overwrite the canonical ledger"
    );
    sqlx::query("DELETE FROM apalis.platform_jobs_apalis_migrations WHERE version = $1")
        .bind(later_version)
        .execute(&mut owner)
        .await
        .expect("remove canonical overlap row");
    sqlx::query("DROP TABLE public.platform_jobs_apalis_migrations")
        .execute(&mut owner)
        .await
        .expect("remove legacy overlap ledger");

    let mut runtime = sqlx::PgConnection::connect(&runtime_url)
        .await
        .expect("connect runtime through workspace SQLx");
    let runtime_owner_error = migrate_and_reconcile_apalis_postgres(&mut runtime)
        .await
        .expect_err("console_rt must not enter owner migration path");
    assert!(
        runtime_owner_error
            .to_string()
            .contains("console_app database owner")
    );

    let mut admin = sqlx::PgConnection::connect(&admin_url)
        .await
        .expect("connect contract-test administrator");
    sqlx::query("ALTER TABLE apalis.workers OWNER TO console_rt")
        .execute(&mut admin)
        .await
        .expect("create foreign-owned object state");
    let foreign_owner_error = migrate_and_reconcile_apalis_postgres(&mut owner)
        .await
        .expect_err("owner migration must refuse foreign-owned objects");
    assert!(
        foreign_owner_error
            .to_string()
            .contains("not owned by console_app")
    );
    sqlx::query("ALTER TABLE apalis.workers OWNER TO console_app")
        .execute(&mut admin)
        .await
        .expect("restore worker table owner");
    migrate_and_reconcile_apalis_postgres(&mut owner)
        .await
        .expect("owner migration succeeds after ownership restoration");

    sqlx::query("SELECT pg_advisory_unlock(901012)")
        .execute(&mut owner)
        .await
        .expect("unlock contract test");
}

async fn assert_runtime_least_privilege(runtime_url: &str) {
    let mut runtime = sqlx::PgConnection::connect(runtime_url)
        .await
        .expect("connect console_rt");
    let acl: bool = sqlx::query_scalar(
        r#"
        SELECT NOT has_schema_privilege(current_user, 'public', 'USAGE')
           AND has_schema_privilege(current_user, 'apalis', 'USAGE')
           AND NOT has_schema_privilege(current_user, 'apalis', 'CREATE')
           AND NOT has_database_privilege(current_user, current_database(), 'CREATE')
           AND has_table_privilege(current_user, 'apalis.jobs', 'SELECT')
           AND has_table_privilege(current_user, 'apalis.jobs', 'INSERT')
           AND has_table_privilege(current_user, 'apalis.jobs', 'UPDATE')
           AND NOT has_table_privilege(current_user, 'apalis.jobs', 'DELETE')
           AND has_table_privilege(current_user, 'apalis.workers', 'SELECT')
           AND has_table_privilege(current_user, 'apalis.workers', 'INSERT')
           AND has_table_privilege(current_user, 'apalis.workers', 'UPDATE')
           AND has_table_privilege(current_user, 'apalis.workers', 'DELETE')
           AND has_table_privilege(current_user, 'apalis.platform_jobs_apalis_migrations', 'SELECT')
           AND NOT has_table_privilege(current_user, 'apalis.platform_jobs_apalis_migrations', 'INSERT')
           AND has_function_privilege(current_user, 'apalis.get_jobs(text,text,integer)', 'EXECUTE')
           AND NOT has_function_privilege(current_user, 'apalis.notify_new_jobs()', 'EXECUTE')
           AND NOT has_function_privilege(current_user, 'apalis.push_job(text,json,text,timestamp with time zone,integer,integer)', 'EXECUTE')
           AND NOT has_function_privilege(
               current_user,
               (
                   SELECT function.oid
                   FROM pg_catalog.pg_proc AS function
                   JOIN pg_catalog.pg_namespace AS namespace
                     ON namespace.oid = function.pronamespace
                   WHERE namespace.nspname = 'public'
                     AND function.proname = 'generate_ulid'
                     AND function.pronargs = 0
               ),
               'EXECUTE'
           )
        "#,
    )
    .fetch_one(&mut runtime)
    .await
    .expect("read effective runtime ACL");
    assert!(acl, "runtime ACL must be exact least privilege");
    assert!(
        sqlx::query("CREATE TABLE apalis.forbidden (id INTEGER)")
            .execute(&mut runtime)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("CREATE SCHEMA forbidden")
            .execute(&mut runtime)
            .await
            .is_err()
    );
    assert!(sqlx::query(
        "INSERT INTO apalis.platform_jobs_apalis_migrations VALUES (0, 'forbidden', now(), true, decode('00','hex'), 0)",
    )
    .execute(&mut runtime)
    .await
    .is_err());
}

async fn assert_reconciliation_cancellation_releases_lock(owner_url: &str) {
    let mut blocker = sqlx::PgConnection::connect(owner_url)
        .await
        .expect("connect reconciliation lock blocker");
    let mut blocker_transaction = blocker
        .begin()
        .await
        .expect("begin lock blocker transaction");
    sqlx::query("SELECT pg_advisory_xact_lock(901011)")
        .execute(&mut *blocker_transaction)
        .await
        .expect("hold reconciliation transaction lock");
    let mut cancelled = sqlx::PgConnection::connect(owner_url)
        .await
        .expect("connect cancellation candidate");
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            migrate_and_reconcile_apalis_postgres(&mut cancelled),
        )
        .await
        .is_err(),
        "reconciliation must wait behind the held advisory lock"
    );
    blocker_transaction
        .rollback()
        .await
        .expect("release reconciliation lock blocker");

    let mut probe = cancelled
        .begin()
        .await
        .expect("cancelled reconciliation must leave its connection transaction-clean");
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(901011)")
        .fetch_one(&mut *probe)
        .await
        .expect("probe reconciliation lock after cancellation");
    assert!(acquired, "cancelled reconciliation must release its lock");
    probe.rollback().await.expect("rollback cancellation probe");
}

async fn assert_runtime_worker_delivers_and_shuts_down(
    runtime_url: &str,
    owner: &mut sqlx::PgConnection,
) {
    let queue_name = format!("contract.worker.{}", uuid::Uuid::new_v4());
    let worker_name = format!("contract-worker-{}", uuid::Uuid::new_v4());
    let key = format!("contract-worker-job:{}", uuid::Uuid::new_v4());
    let expected = PlatformJob::EscalationTimer(console_platform_jobs::EscalationTimerJob {
        scenario_id: "contract".to_owned(),
        timer_id: "worker-delivery".to_owned(),
        scheduled_for: time::OffsetDateTime::now_utc(),
    });
    let request = JobRequest {
        job: expected.clone(),
        idempotency_key: console_platform_jobs::IdempotencyKey::new(&key)
            .expect("build unique worker contract idempotency key"),
    };
    let (delivered_tx, mut delivered_rx) = tokio::sync::mpsc::unbounded_channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let worker_runtime_url = runtime_url.to_owned();
    let worker_queue_name = queue_name.clone();
    let worker_id = worker_name.clone();
    let worker = tokio::spawn(async move {
        run_apalis_worker_until_shutdown(
            &worker_runtime_url,
            &worker_queue_name,
            worker_id,
            RecordingHandler {
                delivered: delivered_tx,
            },
            async {
                let _ = shutdown_rx.await;
            },
        )
        .await
    });

    wait_for_worker_registration(owner, &queue_name, &worker_name).await;
    ApalisPostgresJobQueue::connect(runtime_url, &queue_name)
        .await
        .expect("connect worker contract queue")
        .enqueue(request)
        .await
        .expect("enqueue due worker contract job");
    let delivered = tokio::time::timeout(std::time::Duration::from_secs(10), delivered_rx.recv())
        .await
        .expect("worker delivery timed out")
        .expect("recording handler channel closed before delivery");
    assert_eq!(
        delivered, expected,
        "worker must deliver the exact job once"
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(300), delivered_rx.recv())
            .await
            .is_err(),
        "worker must not redeliver the completed job"
    );
    wait_for_job_done(owner, &queue_name, &key).await;

    shutdown_tx.send(()).expect("signal worker shutdown");
    tokio::time::timeout(std::time::Duration::from_secs(5), worker)
        .await
        .expect("worker shutdown timed out")
        .expect("worker task panicked")
        .expect("worker returns Ok after shutdown");
    sqlx::query("DELETE FROM apalis.jobs WHERE job_type = $1")
        .bind(&queue_name)
        .execute(&mut *owner)
        .await
        .expect("clean worker contract jobs");
    sqlx::query("DELETE FROM apalis.workers WHERE worker_type = $1")
        .bind(&queue_name)
        .execute(&mut *owner)
        .await
        .expect("clean worker contract registration");
}

async fn wait_for_worker_registration(
    owner: &mut sqlx::PgConnection,
    queue_name: &str,
    worker_name: &str,
) {
    for _ in 0..100 {
        let registered: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM apalis.workers WHERE worker_type = $1 AND id = $2)",
        )
        .bind(queue_name)
        .bind(worker_name)
        .fetch_one(&mut *owner)
        .await
        .expect("poll worker registration");
        if registered {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("worker did not register within five seconds");
}

async fn wait_for_job_done(owner: &mut sqlx::PgConnection, queue_name: &str, key: &str) {
    for _ in 0..100 {
        let row = sqlx::query(
            "SELECT status, done_at IS NOT NULL AS has_done_at FROM apalis.jobs WHERE job_type = $1 AND idempotency_key = $2",
        )
        .bind(queue_name)
        .bind(key)
        .fetch_one(&mut *owner)
        .await
        .expect("poll completed worker job");
        if row.get::<String, _>("status") == "Done" && row.get::<bool, _>("has_done_at") {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("worker job did not reach Done with done_at within five seconds");
}

async fn apalis_state_snapshot(owner: &mut sqlx::PgConnection) -> serde_json::Value {
    sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
            'database_acl', (
                SELECT datacl FROM pg_catalog.pg_database WHERE datname = current_database()
            ),
            'schema', (
                SELECT jsonb_build_array(namespace.nspowner, namespace.nspacl)
                FROM pg_catalog.pg_namespace AS namespace WHERE namespace.nspname = 'apalis'
            ),
            'ledger', (
                SELECT jsonb_agg(to_jsonb(ledger) ORDER BY version)
                FROM apalis.platform_jobs_apalis_migrations AS ledger
            ),
            'relations', (
                SELECT jsonb_agg(jsonb_build_array(object.relname, object.relkind, object.relowner, object.relacl) ORDER BY object.relname)
                FROM pg_catalog.pg_class AS object
                JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
                WHERE namespace.nspname = 'apalis'
            ),
            'indexes', (
                SELECT jsonb_agg(pg_get_indexdef(job_index.indexrelid) ORDER BY job_index.indexrelid::regclass::text)
                FROM pg_catalog.pg_index AS job_index
                JOIN pg_catalog.pg_class AS object ON object.oid = job_index.indrelid
                JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
                WHERE namespace.nspname = 'apalis'
            ),
            'constraints', (
                SELECT jsonb_agg(jsonb_build_array(object.relname, schema_constraint.conname, schema_constraint.contype, pg_get_constraintdef(schema_constraint.oid)) ORDER BY object.relname, schema_constraint.conname)
                FROM pg_catalog.pg_constraint AS schema_constraint
                JOIN pg_catalog.pg_class AS object ON object.oid = schema_constraint.conrelid
                JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
                WHERE namespace.nspname = 'apalis'
            ),
            'functions', (
                SELECT jsonb_agg(jsonb_build_array(function.oid::regprocedure::text, function.proacl, function.prosrc) ORDER BY function.oid::regprocedure::text)
                FROM pg_catalog.pg_proc AS function
                JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = function.pronamespace
                WHERE namespace.nspname = 'apalis'
                   OR (namespace.nspname = 'public' AND function.proname = 'generate_ulid')
            ),
            'columns', (
                SELECT jsonb_agg(to_jsonb(columns) ORDER BY table_name, ordinal_position)
                FROM information_schema.columns
                WHERE table_schema = 'apalis'
            ),
            'triggers', (
                SELECT jsonb_agg(jsonb_build_array(trigger.tgname, trigger.tgenabled, trigger.tgtype, trigger.tgfoid) ORDER BY trigger.tgname)
                FROM pg_catalog.pg_trigger AS trigger
                WHERE trigger.tgrelid = 'apalis.jobs'::regclass AND NOT trigger.tgisinternal
            )
        )
        "#,
    )
    .fetch_one(owner)
    .await
    .expect("snapshot Apalis schema, ACL, and ledger")
}

async fn assert_runtime_pool_resets_poisoned_session(runtime_url: &str) {
    let pool = connect_apalis_runtime_pool(runtime_url)
        .await
        .expect("connect hardened runtime pool");
    {
        let mut connection = pool.acquire().await.expect("acquire runtime connection");
        apalis_sqlx::query("SET ROLE console_rt")
            .execute(&mut *connection)
            .await
            .expect("poison runtime role state");
        apalis_sqlx::query("SET app.current_org = 'poison'")
            .execute(&mut *connection)
            .await
            .expect("poison runtime tenant GUC");
        apalis_sqlx::query("SET statement_timeout = '1ms'")
            .execute(&mut *connection)
            .await
            .expect("poison runtime timeout");
    }
    let mut connection = pool
        .acquire()
        .await
        .expect("reacquire reset runtime connection");
    let row = apalis_sqlx::query(
        r#"
        SELECT session_user::text AS session_user,
               current_user::text AS current_user,
               current_setting('statement_timeout')::interval = interval '30 seconds' AS statement_ok,
               current_setting('idle_in_transaction_session_timeout')::interval = interval '30 seconds' AS idle_ok,
               current_setting('transaction_timeout')::interval = interval '45 seconds' AS transaction_ok,
               current_setting('app.current_org', true) IS NULL AS tenant_guc_cleared
        "#,
    )
    .fetch_one(&mut *connection)
    .await
    .expect("read reset runtime session");
    assert_eq!(row.get::<String, _>("session_user"), "console_rt");
    assert_eq!(row.get::<String, _>("current_user"), "console_rt");
    assert!(row.get::<bool, _>("statement_ok"));
    assert!(row.get::<bool, _>("idle_ok"));
    assert!(row.get::<bool, _>("transaction_ok"));
    assert!(row.get::<bool, _>("tenant_guc_cleared"));
}

async fn assert_runtime_can_enqueue(runtime_url: &str, owner: &mut sqlx::PgConnection) {
    let queue_name = format!("contract.enqueue.{}", uuid::Uuid::new_v4());
    let key = format!("contract-enqueue:{}", uuid::Uuid::new_v4());
    let queue = ApalisPostgresJobQueue::connect(runtime_url, &queue_name)
        .await
        .expect("connect runtime queue");
    let request =
        JobRequest::escalation_timer("contract", "enqueue", time::OffsetDateTime::now_utc(), &key)
            .expect("build contract enqueue request");
    queue.enqueue(request).await.expect("enqueue as console_rt");
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM apalis.jobs WHERE job_type = $1 AND idempotency_key = $2",
    )
    .bind(&queue_name)
    .bind(&key)
    .fetch_one(&mut *owner)
    .await
    .expect("read enqueued job as owner");
    assert_eq!(count, 1);
    sqlx::query("DELETE FROM apalis.jobs WHERE job_type = $1")
        .bind(&queue_name)
        .execute(&mut *owner)
        .await
        .expect("clean contract enqueue job");
}

async fn assert_runtime_refuses(runtime_url: &str, state: &str) {
    assert!(
        ApalisPostgresJobQueue::connect(runtime_url, "contract.failure")
            .await
            .is_err(),
        "runtime must refuse {state}"
    );
}

async fn reset_apalis(owner: &mut sqlx::PgConnection) {
    sqlx::raw_sql(
        "DROP SCHEMA IF EXISTS apalis CASCADE; DROP TABLE IF EXISTS public.platform_jobs_apalis_migrations; DROP FUNCTION IF EXISTS public.generate_ulid()",
    )
    .execute(owner)
    .await
    .expect("reset isolated Apalis contract database");
}

async fn latest_ledger_row(owner: &mut sqlx::PgConnection) -> LedgerRow {
    let row = sqlx::query(
        r#"
        SELECT version, description, installed_on, success, checksum, execution_time
        FROM apalis.platform_jobs_apalis_migrations
        ORDER BY version DESC LIMIT 1
        "#,
    )
    .fetch_one(owner)
    .await
    .expect("read latest migration row");
    LedgerRow {
        version: row.get("version"),
        description: row.get("description"),
        installed_on: row.get("installed_on"),
        success: row.get("success"),
        checksum: row.get("checksum"),
        execution_time: row.get("execution_time"),
    }
}

async fn ledger_row(owner: &mut sqlx::PgConnection, version: i64) -> Option<LedgerRow> {
    let row = sqlx::query(
        r#"
        SELECT version, description, installed_on, success, checksum, execution_time
        FROM apalis.platform_jobs_apalis_migrations WHERE version = $1
        "#,
    )
    .bind(version)
    .fetch_optional(owner)
    .await
    .expect("read migration row");
    row.map(|row| LedgerRow {
        version: row.get("version"),
        description: row.get("description"),
        installed_on: row.get("installed_on"),
        success: row.get("success"),
        checksum: row.get("checksum"),
        execution_time: row.get("execution_time"),
    })
}

async fn restore_ledger_row(owner: &mut sqlx::PgConnection, row: &LedgerRow) {
    sqlx::query(
        r#"
        INSERT INTO apalis.platform_jobs_apalis_migrations
            (version, description, installed_on, success, checksum, execution_time)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (version) DO UPDATE SET
            description = EXCLUDED.description,
            installed_on = EXCLUDED.installed_on,
            success = EXCLUDED.success,
            checksum = EXCLUDED.checksum,
            execution_time = EXCLUDED.execution_time
        "#,
    )
    .bind(row.version)
    .bind(&row.description)
    .bind(row.installed_on)
    .bind(row.success)
    .bind(&row.checksum)
    .bind(row.execution_time)
    .execute(owner)
    .await
    .expect("restore migration row");
}

fn required_url(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required for Apalis contract test"))
}

/// Re-assert the two serving-role passwords the harness minted, before connecting.
///
/// `ALTER ROLE ... PASSWORD` is CLUSTER-GLOBAL: it outlives the per-test database that
/// `#[sqlx::test]` drops. Four sibling test files in this same serialized single-container
/// run rewrite `console_app`'s password to a hardcoded literal so they can log in as it —
/// `platform/db/tests/attendance_console_migration_contract.rs:981,1178`,
/// `ontology/adapter-postgres/tests/key_revision_migration_upgrade.rs:37`,
/// `leave/adapter-postgres/tests/leave_migration_expand_contract.rs:158` — and none restores
/// it. Whichever of them runs first, every later target authenticating as `console_app` or
/// `console_rt` from the harness URL gets `28P01 password authentication failed`.
///
/// Restoring in those siblings would be order-dependent and a panic would skip it. Re-asserting
/// here is neither: this test states the credential it needs, from the admin URL the harness
/// also supplies, and is correct regardless of what any sibling did or how it exited.
async fn reassert_serving_role_passwords(owner_url: &str, runtime_url: &str) {
    fn password_of(url: &str) -> String {
        let after_scheme = url.split("://").nth(1).expect("database URL has a scheme");
        let credentials = after_scheme
            .split('@')
            .next()
            .expect("database URL has credentials");
        credentials
            .split_once(':')
            .expect("database URL carries a password")
            .1
            .to_owned()
    }
    let admin_url = std::env::var("CONSOLE_APALIS_ADMIN_DATABASE_URL")
        .expect("CONSOLE_APALIS_ADMIN_DATABASE_URL is required to re-assert role passwords");
    let mut admin = sqlx::PgConnection::connect(&admin_url)
        .await
        .expect("connect as harness admin");
    for (role, password) in [
        ("console_app", password_of(owner_url)),
        ("console_rt", password_of(runtime_url)),
    ] {
        // format(%I,%L) builds the statement server-side: ALTER ROLE takes no bind
        // parameters, and the password is a harness-minted literal.
        let statement: String =
            sqlx::query_scalar("SELECT format('ALTER ROLE %I PASSWORD %L', $1::text, $2::text)")
                .bind(role)
                .bind(&password)
                .fetch_one(&mut admin)
                .await
                .expect("build ALTER ROLE statement");
        // Sound: `statement` is built by PostgreSQL's own format(%I,%L) from a role name
        // this function fixes and a harness-minted hex password — never request-derived.
        sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
            .execute(&mut admin)
            .await
            .expect("re-assert serving role password");
    }
}
