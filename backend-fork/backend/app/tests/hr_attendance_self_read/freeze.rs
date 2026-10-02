//! Real capture/close interleavings. PostgreSQL witnesses order; timers only fail proof.
use super::*;
use sqlx::{Postgres, Transaction};
use tokio::task::JoinHandle;

const CLOSE_PATH: &str = "/api/v1/attendance/closes";
const INSERT_GATE: &str = "test.native-attendance.material-insert";

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn same_account_freeze_first_refuses_capture_without_partial_effects(pool: PgPool) {
    let keys = keys();
    let actor = UserId::new();
    seed_linked_employee(&pool, actor, "SUPER_ADMIN", "worker-attestor").await;
    let token = bearer(&pool, &keys, actor, "SUPER_ADMIN").await;
    let (close, close_pid) = named_router(&pool, &keys, "freeze-first-close").await;
    let (capture, capture_pid) = named_router(&pool, &keys, "freeze-first-capture").await;
    let mut gate = hold_key(&pool, &payroll_key()).await;
    let gate_pid = transaction_pid(&mut gate).await;
    let close_body = close_body(&pool).await;
    let close_task = spawn_post(close, CLOSE_PATH, &token, close_body);
    assert!(
        wait_for_key(
            &pool,
            close_pid,
            gate_pid,
            &close_task,
            "SELECT pg_advisory_xact_lock"
        )
        .await,
        "real freeze owner must wait on the held payroll key"
    );
    let body = json!({"kind":"CLOCK_IN","idempotency_key":"freeze-first"});
    let capture_task = spawn_post(capture.clone(), ME_PATH, &token, body.clone());
    let capture_waited = wait_for_key(
        &pool,
        capture_pid,
        gate_pid,
        &capture_task,
        "SELECT pg_advisory_xact_lock",
    )
    .await;
    gate.commit().await.unwrap();
    let closed = finish(close_task).await;
    let captured = finish(capture_task).await;
    assert_eq!(closed.status, StatusCode::CREATED, "{:?}", closed.json);
    assert_eq!(
        captured.status,
        if capture_waited {
            StatusCode::CONFLICT
        } else {
            StatusCode::OK
        },
        "unexpected owner response is not missing-ordering RED: {:?}",
        captured.json
    );
    assert!(
        capture_waited,
        "capture must serialize with the real freeze owner before checking its period"
    );
    assert_eq!(captured.status, StatusCode::CONFLICT, "{:?}", captured.json);
    assert_eq!(capture_counts(&pool).await, (0, 0, 0));
    assert_freeze(&pool, &closed.json).await;
    let before = attendance_counts(&pool).await;
    let retried = post(capture, ME_PATH, &token, body).await;
    assert_eq!(retried.status, StatusCode::CONFLICT, "{:?}", retried.json);
    assert_eq!(
        attendance_counts(&pool).await,
        before,
        "refusal cannot leave any effects"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn capture_first_freeze_waits_for_fact_reference_and_audits_to_commit(pool: PgPool) {
    let keys = keys();
    let worker = UserId::new();
    let employee = seed_linked_employee(&pool, worker, "MEMBER", "capture-first-worker").await;
    let attestor = UserId::new();
    seed_user(&pool, attestor, "SUPER_ADMIN").await;
    let worker_token = bearer(&pool, &keys, worker, "MEMBER").await;
    let attestor_token = bearer(&pool, &keys, attestor, "SUPER_ADMIN").await;
    let (capture, capture_pid) = named_router(&pool, &keys, "capture-first-capture").await;
    let (close, close_pid) = named_router(&pool, &keys, "capture-first-close").await;
    install_insert_gate(&pool).await;
    let mut gate = hold_key(&pool, INSERT_GATE).await;
    let gate_pid = transaction_pid(&mut gate).await;
    let body = json!({"kind":"CLOCK_IN","idempotency_key":"capture-first"});
    let capture_task = spawn_post(capture.clone(), ME_PATH, &worker_token, body.clone());
    assert!(
        wait_for_key(
            &pool,
            capture_pid,
            gate_pid,
            &capture_task,
            "INSERT INTO payroll_attendance_material_refs"
        )
        .await,
        "capture must reach material INSERT after the actual open check"
    );
    assert_eq!(
        capture_counts(&pool).await,
        (0, 0, 0),
        "uncommitted facts must not escape"
    );
    let close_task = spawn_post(close, CLOSE_PATH, &attestor_token, close_body(&pool).await);
    let freeze_waited = wait_for_key(
        &pool,
        close_pid,
        capture_pid,
        &close_task,
        "SELECT pg_advisory_xact_lock",
    )
    .await;
    gate.commit().await.unwrap();
    let captured = finish(capture_task).await;
    let closed = finish(close_task).await;
    assert_eq!(captured.status, StatusCode::OK, "{:?}", captured.json);
    assert_eq!(closed.status, StatusCode::CREATED, "{:?}", closed.json);
    assert!(
        freeze_waited,
        "freeze must wait for the capture transaction after its open check"
    );
    assert_eq!(capture_counts(&pool).await, (1, 1, 2));
    let reference =
        Uuid::parse_str(captured.json["payroll_material_ref_id"].as_str().unwrap()).unwrap();
    assert_record_shape(&captured.json, employee, Some(reference), false);
    assert_persisted_raw_fields(&pool, &captured.json).await;
    let matching: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM employee_attendance_records r JOIN payroll_attendance_material_refs m ON m.org_id=r.org_id AND m.attendance_record_id=r.id AND m.employee_id=r.employee_id AND m.work_date=r.work_date WHERE m.id=$1)")
        .bind(reference).fetch_one(&pool).await.unwrap();
    assert!(
        matching,
        "acknowledged reference must match the raw fact exactly"
    );
    assert_freeze(&pool, &closed.json).await;
    let before = attendance_counts(&pool).await;
    let replay = post(capture.clone(), ME_PATH, &worker_token, body).await;
    assert_eq!(replay.status, StatusCode::OK, "{:?}", replay.json);
    assert_record_shape(&replay.json, employee, Some(reference), true);
    assert_eq!(replay.json["id"], captured.json["id"]);
    let refused = post(
        capture,
        ME_PATH,
        &worker_token,
        json!({"kind":"CLOCK_OUT","idempotency_key":"after-freeze"}),
    )
    .await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{:?}", refused.json);
    assert_eq!(
        attendance_counts(&pool).await,
        before,
        "replay/refusal must not write"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn replay_and_company_domain_isolation_do_not_wait_for_unrelated_freezes(pool: PgPool) {
    let keys = keys();
    let worker = UserId::new();
    seed_linked_employee(&pool, worker, "MEMBER", "replay-worker").await;
    let token = bearer(&pool, &keys, worker, "MEMBER").await;
    let (service, _) = named_router(&pool, &keys, "replay-isolation").await;
    let body = json!({"kind":"CLOCK_IN","idempotency_key":"replay-open"});
    let created = post(service.clone(), ME_PATH, &token, body.clone()).await;
    assert_eq!(created.status, StatusCode::OK, "{:?}", created.json);
    let before = attendance_counts(&pool).await;
    let gate = hold_key(&pool, &payroll_key()).await;
    let replay = finish(spawn_post(service.clone(), ME_PATH, &token, body)).await;
    assert_eq!(replay.status, StatusCode::OK, "{:?}", replay.json);
    assert_eq!(replay.json["id"], created.json["id"]);
    assert_eq!(replay.json["duplicate"], true);
    let changed = finish(spawn_post(
        service.clone(),
        ME_PATH,
        &token,
        json!({"kind":"CLOCK_OUT","idempotency_key":"replay-open"}),
    ))
    .await;
    assert_eq!(changed.status, StatusCode::CONFLICT, "{:?}", changed.json);
    assert_eq!(attendance_counts(&pool).await, before);
    gate.rollback().await.unwrap();
    let other_company = hold_key(
        &pool,
        &format!("console.period-lock|payroll|{}", Uuid::new_v4()),
    )
    .await;
    let accounting = hold_key(
        &pool,
        &format!("console.period-lock|accounting|{}", OrgId::knl().as_uuid()),
    )
    .await;
    let next = finish(spawn_post(
        service,
        ME_PATH,
        &token,
        json!({"kind":"CLOCK_OUT","idempotency_key":"unrelated-keys"}),
    ))
    .await;
    assert_eq!(next.status, StatusCode::OK, "{:?}", next.json);
    assert_eq!(capture_counts(&pool).await, (2, 2, 4));
    accounting.rollback().await.unwrap();
    other_company.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn capture_lock_preserves_employee_link_until_commit_then_resolves_new_link(pool: PgPool) {
    let keys = keys();
    let worker = UserId::new();
    let original = seed_linked_employee(&pool, worker, "MEMBER", "link-original").await;
    let other = UserId::new();
    let replacement = seed_linked_employee(&pool, other, "MEMBER", "link-replacement").await;
    sqlx::query("UPDATE users SET employee_id=NULL WHERE id=$1")
        .bind(*other.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let token = bearer(&pool, &keys, worker, "MEMBER").await;
    let (capture, capture_pid) = named_router(&pool, &keys, "link-capture").await;
    install_insert_gate(&pool).await;
    let mut gate = hold_key(&pool, INSERT_GATE).await;
    let gate_pid = transaction_pid(&mut gate).await;
    let capture_task = spawn_post(
        capture.clone(),
        ME_PATH,
        &token,
        json!({"kind":"CLOCK_IN","idempotency_key":"link-before"}),
    );
    assert!(
        wait_for_key(
            &pool,
            capture_pid,
            gate_pid,
            &capture_task,
            "INSERT INTO payroll_attendance_material_refs"
        )
        .await
    );
    let update_pool = named_pool(&pool, "link-update").await;
    let update_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&update_pool)
        .await
        .unwrap();
    let update_task = tokio::spawn(async move {
        sqlx::query("UPDATE users SET employee_id=$1 WHERE id=$2 AND org_id=$3")
            .bind(replacement)
            .bind(*worker.as_uuid())
            .bind(*OrgId::knl().as_uuid())
            .execute(&update_pool)
            .await
            .unwrap()
            .rows_affected()
    });
    let relink_waited = wait_for_row(&pool, update_pid, capture_pid, &update_task).await;
    gate.commit().await.unwrap();
    let captured = finish(capture_task).await;
    let updated = tokio::time::timeout(std::time::Duration::from_secs(10), update_task)
        .await
        .unwrap()
        .unwrap();
    assert!(
        relink_waited,
        "Account link UPDATE must serialize with capture"
    );
    assert_eq!(updated, 1);
    assert_eq!(captured.status, StatusCode::OK, "{:?}", captured.json);
    assert_eq!(captured.json["employee_id"], original.to_string());
    let next = post(
        capture,
        ME_PATH,
        &token,
        json!({"kind":"CLOCK_IN","idempotency_key":"link-after"}),
    )
    .await;
    assert_eq!(next.status, StatusCode::OK, "{:?}", next.json);
    assert_eq!(next.json["employee_id"], replacement.to_string());
    assert_eq!(capture_counts(&pool).await, (2, 2, 4));
}

fn payroll_key() -> String {
    format!("console.period-lock|payroll|{}", OrgId::knl().as_uuid())
}

async fn close_body(pool: &PgPool) -> Value {
    let month: String =
        sqlx::query_scalar("SELECT to_char(now() AT TIME ZONE 'Asia/Seoul','YYYY-MM')")
            .fetch_one(pool)
            .await
            .unwrap();
    json!({"month":month,"attest":true})
}

async fn capture_counts(pool: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM employee_attendance_records),(SELECT count(*) FROM payroll_attendance_material_refs),(SELECT count(*) FROM audit_events WHERE action IN ('employee_attendance.record','payroll_attendance.link'))")
        .fetch_one(pool).await.unwrap()
}

async fn assert_freeze(pool: &PgPool, closed: &Value) {
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM period_locks WHERE org_id=$1 AND domain='payroll' AND unlocked_at IS NULL),(SELECT count(*) FROM attendance_month_closes WHERE org_id=$1 AND branch_id IS NULL),(SELECT count(*) FROM audit_events WHERE org_id=$1 AND action IN ('period_lock.create','attendance.close.confirm'))")
        .bind(*OrgId::knl().as_uuid()).fetch_one(pool).await.unwrap();
    assert_eq!(
        counts,
        (1, 1, 2),
        "real organization-wide close must retain its own effects"
    );
    assert!(
        closed["period_lock_id"]
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok())
            .is_some()
    );
}

async fn named_pool(owner: &PgPool, name: &str) -> PgPool {
    let name = name.to_owned();
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .after_connect(move |conn, _| {
            let name = name.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('application_name',$1,false)")
                    .bind(name)
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("SELECT set_config('app.current_org',$1,false)")
                    .bind(OrgId::knl().as_uuid().to_string())
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(owner.connect_options().as_ref().clone())
        .await
        .unwrap();
    let runtime: bool = sqlx::query_scalar("SELECT current_user='console_rt' AND NOT rolsuper AND NOT rolbypassrls AND NOT EXISTS(SELECT 1 FROM pg_class WHERE relname IN ('users','employee_attendance_records','period_locks') AND relowner=(SELECT oid FROM pg_roles WHERE rolname=current_user)) FROM pg_roles WHERE rolname=current_user")
        .fetch_one(&pool).await.unwrap();
    assert!(
        runtime,
        "contender must enforce RLS as a genuine non-owner runtime role"
    );
    pool
}

async fn named_router(owner: &PgPool, keys: &Keys, name: &str) -> (axum::Router, i32) {
    let pool = named_pool(owner, name).await;
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&pool)
        .await
        .unwrap();
    (
        build_router(app_state(pool, keys.public_pem.clone()).unwrap()),
        pid,
    )
}

async fn hold_key<'a>(pool: &'a PgPool, key: &str) -> Transaction<'a, Postgres> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(key)
        .execute(tx.as_mut())
        .await
        .unwrap();
    tx
}

async fn transaction_pid(tx: &mut Transaction<'_, Postgres>) -> i32 {
    sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(tx.as_mut())
        .await
        .unwrap()
}

async fn install_insert_gate(pool: &PgPool) {
    // Disposable test database only; production owns the actual request/transaction.
    sqlx::raw_sql("CREATE FUNCTION test_capture_gate() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(hashtextextended('test.native-attendance.material-insert',0)); RETURN NEW; END $$; CREATE TRIGGER test_capture_gate BEFORE INSERT ON payroll_attendance_material_refs FOR EACH ROW EXECUTE FUNCTION test_capture_gate();")
        .execute(pool).await.unwrap();
}

fn spawn_post(
    service: axum::Router,
    path: &str,
    token: &str,
    body: Value,
) -> JoinHandle<JsonResponse> {
    let path = path.to_owned();
    let token = token.to_owned();
    tokio::spawn(async move { post(service, &path, &token, body).await })
}

async fn finish(task: JoinHandle<JsonResponse>) -> JsonResponse {
    tokio::time::timeout(std::time::Duration::from_secs(10), task)
        .await
        .expect("real request must complete after lock release")
        .unwrap()
}

async fn wait_for_key<T>(
    pool: &PgPool,
    waiter: i32,
    holder: i32,
    task: &JoinHandle<T>,
    query: &str,
) -> bool {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let witnessed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity a JOIN pg_locks w ON w.pid=a.pid AND w.locktype='advisory' AND NOT w.granted JOIN pg_locks h ON h.pid=$2 AND h.locktype='advisory' AND h.granted AND h.database IS NOT DISTINCT FROM w.database AND h.classid=w.classid AND h.objid=w.objid AND h.objsubid=w.objsubid WHERE a.pid=$1 AND a.query LIKE '%' || $3 || '%' AND a.wait_event_type='Lock' AND a.wait_event='advisory' AND $2=ANY(pg_blocking_pids(a.pid)))")
            .bind(waiter).bind(holder).bind(query).fetch_one(pool).await.unwrap();
        if witnessed {
            return true;
        }
        if task.is_finished() {
            return false;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "proof timeout observing advisory wait; not behavioral RED"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

async fn wait_for_row<T>(pool: &PgPool, waiter: i32, holder: i32, task: &JoinHandle<T>) -> bool {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let witnessed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND query LIKE 'UPDATE users SET employee_id=%' AND wait_event_type='Lock' AND wait_event IN ('transactionid','tuple') AND $2=ANY(pg_blocking_pids(pid)))")
            .bind(waiter).bind(holder).fetch_one(pool).await.unwrap();
        if witnessed {
            return true;
        }
        if task.is_finished() {
            return false;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "proof timeout observing Account row wait; not behavioral RED"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
