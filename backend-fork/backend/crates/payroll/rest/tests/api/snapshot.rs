//! Disposable local read-consistency probes. Fixture SQL is not a governed
//! payroll calculation, payment, legal approval or document-service claim.
use super::*;
use sqlx::{Postgres, Transaction, postgres::PgPoolOptions};
use std::time::Duration as StdDuration;
use tokio::{task::JoinHandle, time::timeout};

const READER: &str = "payroll-snapshot-reader";
const LOCK: i64 = 718_239_551;

struct Fixture {
    rt: PgPool,
    keys: Keys,
    org: OrgId,
    actor: UserId,
    run: Uuid,
    employee: Uuid,
    token: String,
}

async fn fixture(owner: &PgPool) -> Fixture {
    let org = OrgId::knl();
    let actor = UserId::new();
    seed_user(owner, actor, *org.as_uuid(), "EXECUTIVE").await;
    let run = seed_run(owner, *org.as_uuid(), actor).await;
    let employee = seed_employee(owner, *org.as_uuid(), "Before").await;
    seed_line(owner, *org.as_uuid(), run, employee, "Before").await;
    let rt = PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(
            owner
                .connect_options()
                .as_ref()
                .clone()
                .application_name(READER),
        )
        .await
        .unwrap();
    let keys = keys();
    let token = bearer(&rt, &keys, actor, org, "EXECUTIVE").await;
    Fixture {
        rt,
        keys,
        org,
        actor,
        run,
        employee,
        token,
    }
}

struct Gate {
    controller: Transaction<'static, Postgres>,
    pid: i32,
    query: String,
    exact: bool,
}

impl Gate {
    async fn install(
        owner: &PgPool,
        table: &'static str,
        query: &'static str,
        exact: bool,
    ) -> Self {
        // All interpolated values are fixed test literals, never client input.
        let comparison = if exact { "=" } else { "LIKE" };
        let pattern = if exact {
            query.to_owned()
        } else {
            format!("{query}%")
        };
        let pattern = pattern.replace("'", "''");
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE FUNCTION payroll_snapshot_gate(row_id uuid) RETURNS boolean \
             LANGUAGE plpgsql VOLATILE PARALLEL UNSAFE AS $gate$ \
             BEGIN IF current_setting('application_name') = '{READER}' \
             AND btrim(current_query(),chr(32)||chr(10)||chr(13)||chr(9)) {comparison} '{pattern}' THEN \
             PERFORM pg_advisory_xact_lock_shared({LOCK}, CASE current_setting('transaction_isolation') WHEN 'repeatable read' THEN 2 ELSE 1 END); END IF; \
             RETURN row_id IS NOT NULL; END $gate$; \
             CREATE POLICY payroll_snapshot_probe ON {table} AS RESTRICTIVE \
             FOR SELECT TO console_rt USING (payroll_snapshot_gate(id));"
        ))).execute(owner).await.unwrap();
        let mut controller = owner.begin().await.unwrap();
        let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(controller.as_mut())
            .await
            .unwrap();
        for mode in [1_i32, 2] {
            sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
                .bind(LOCK as i32)
                .bind(mode)
                .execute(controller.as_mut())
                .await
                .unwrap();
        }
        Self {
            controller,
            pid,
            query: query.to_owned(),
            exact,
        }
    }

    async fn require_observed<T>(self, owner: &PgPool, reader: &mut JoinHandle<T>) -> (Self, i64) {
        let observed = timeout(StdDuration::from_secs(15), async {
            loop {
                let blocked: Option<(i64, String)> = sqlx::query_as(
                    "SELECT l.objid::bigint, a.query FROM pg_stat_activity a JOIN pg_locks l ON l.pid=a.pid \
                     WHERE a.datname=current_database() AND a.application_name=$1 \
                     AND a.wait_event_type='Lock' AND a.wait_event='advisory' \
                     AND $2=ANY(pg_blocking_pids(a.pid)) AND l.locktype='advisory' \
                     AND NOT l.granted AND l.classid=$5::bigint::oid AND l.objsubid=2 \
                     AND CASE WHEN $4 THEN btrim(a.query,chr(32)||chr(10)||chr(13)||chr(9))=$3 ELSE starts_with(btrim(a.query,chr(32)||chr(10)||chr(13)||chr(9)),$3) END"
                ).bind(READER).bind(self.pid).bind(&self.query).bind(self.exact).bind(LOCK)
                    .fetch_optional(owner).await.unwrap();
                if let Some((mode, query)) = blocked {
                    eprintln!("snapshot probe: exact query={:?}; controller={}; isolation_mode={} (1=read committed,2=repeatable read)", query, self.pid, mode);
                    return mode;
                }
                tokio::time::sleep(StdDuration::from_millis(10)).await;
            }
        }).await;
        match observed {
            Ok(mode) => (self, mode),
            Err(_) => {
                self.release().await;
                reader.abort();
                let _ = reader.await;
                panic!("probe setup failed: exact query never blocked; no writer executed");
            }
        }
    }

    async fn release(self) {
        self.controller.commit().await.unwrap();
    }
}

fn start_read(f: &Fixture, path: String) -> JoinHandle<JsonResponse> {
    let service = app(f.rt.clone(), &f.keys);
    let token = f.token.clone();
    tokio::spawn(async move { get(service, &path, &token).await })
}

async fn finish_task<T>(mut reader: JoinHandle<T>) -> T {
    match timeout(StdDuration::from_secs(15), &mut reader).await {
        Ok(result) => result.unwrap(),
        Err(_) => {
            reader.abort();
            let _ = reader.await;
            panic!("probe setup failed: reader did not complete after gate release");
        }
    }
}

async fn finish_read(reader: JoinHandle<JsonResponse>) -> JsonResponse {
    let response = finish_task(reader).await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "read setup failed: {}",
        response.json
    );
    response
}

async fn audit_count(owner: &PgPool, actor: UserId, action: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE actor = $1 AND action = $2")
        .bind(actor.as_uuid())
        .bind(action)
        .fetch_one(owner)
        .await
        .unwrap()
}

async fn add_next_period_run(owner: &PgPool, f: &Fixture) -> Uuid {
    let port = PgPayRunPort::new(
        runtime_role_pool(owner).await,
        tokio::runtime::Handle::current(),
    );
    let result = execute_sync(
        &port,
        PayRunCommand {
            org_id: f.org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: f.actor,
            query: PayRunQuery::CreateRun {
                run_id: Uuid::new_v4(),
                period_start: date!(2026 - 07 - 01),
                period_end: date!(2026 - 07 - 31),
                connector: Some("m2".to_owned()),
                job: Some("payroll_draft".to_owned()),
            },
            action_key: "create_run".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();
    result.result()["draft_run_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn run_list_count_and_page_share_a_snapshot(owner: PgPool) {
    let f = fixture(&owner).await;
    let gate = Gate::install(
        &owner,
        "payroll_draft_runs",
        "SELECT COUNT(*) FROM payroll_draft_runs",
        true,
    )
    .await;
    let mut pending = start_read(&f, PAYROLL_RUNS_PATH.to_owned());
    let (gate, mode) = gate.require_observed(&owner, &mut pending).await;
    let added = add_next_period_run(&owner, &f).await;
    assert_ne!(added, f.run);
    gate.release().await;
    let original = finish_read(pending).await;
    let fresh = get(app(f.rt.clone(), &f.keys), PAYROLL_RUNS_PATH, &f.token).await;
    assert_eq!(fresh.status, StatusCode::OK);
    assert_eq!(fresh.json["total"], 2);
    assert_eq!(fresh.json["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        audit_count(&owner, f.actor, "payroll_run.list_read").await,
        2
    );
    assert_eq!(
        (
            original.json["total"].as_i64().unwrap(),
            original.json["items"].as_array().unwrap().len()
        ),
        (1, 1)
    );
    assert_eq!(mode, 2, "read must use repeatable-read snapshot");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn run_detail_head_lines_and_counts_share_a_snapshot(owner: PgPool) {
    let f = fixture(&owner).await;
    let other = seed_employee(&owner, *f.org.as_uuid(), "After").await;
    sqlx::query("UPDATE payroll_draft_runs SET source_summary=jsonb_set(source_summary,'{fixture_line_count}','1') WHERE id=$1").bind(f.run).execute(&owner).await.unwrap();
    let gate = Gate::install(
        &owner,
        "payroll_draft_runs",
        "SELECT id, period_start, period_end, source_label, status,",
        false,
    )
    .await;
    let path = format!("{PAYROLL_RUNS_PATH}/{}", f.run);
    let mut pending = start_read(&f, path.clone());
    let (gate, mode) = gate.require_observed(&owner, &mut pending).await;
    let mut changed = owner.begin().await.unwrap();
    sqlx::query("INSERT INTO payroll_draft_lines (org_id,run_id,employee_id,employee_source_key,employee_display_name,employee_company) VALUES ($1,$2,$3,$4,'After','KNL')").bind(f.org.as_uuid()).bind(f.run).bind(other).bind(format!("src-{other}")).execute(changed.as_mut()).await.unwrap();
    sqlx::query("UPDATE payroll_draft_runs SET source_summary=jsonb_set(source_summary,'{fixture_line_count}','2') WHERE id=$1").bind(f.run).execute(changed.as_mut()).await.unwrap();
    changed.commit().await.unwrap();
    gate.release().await;
    let original = finish_read(pending).await;
    let fresh = get(app(f.rt.clone(), &f.keys), &path, &f.token).await;
    assert_eq!(fresh.status, StatusCode::OK);
    assert_eq!(fresh.json["source_summary"]["fixture_line_count"], 2);
    assert_eq!(fresh.json["lines_total"], 2);
    assert_eq!(fresh.json["lines"].as_array().unwrap().len(), 2);
    assert_eq!(audit_count(&owner, f.actor, "payroll_run.read").await, 2);
    assert_eq!(
        (
            original.json["source_summary"]["fixture_line_count"]
                .as_i64()
                .unwrap(),
            original.json["lines_total"].as_i64().unwrap(),
            original.json["lines"].as_array().unwrap().len()
        ),
        (1, 1, 1)
    );
    assert_eq!(mode, 2, "read must use repeatable-read snapshot");
}

async fn add_exception(owner: &PgPool, f: &Fixture, label: &str) {
    sqlx::query("INSERT INTO payroll_run_exceptions (org_id,run_id,employee_display_name,kind,severity,summary_ko) VALUES ($1,$2,$3,'ACCOUNT_VERIFICATION','warn',$3)")
        .bind(f.org.as_uuid()).bind(f.run).bind(label).execute(owner).await.unwrap();
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn exceptions_count_open_and_page_share_a_snapshot(owner: PgPool) {
    let f = fixture(&owner).await;
    add_exception(&owner, &f, "Before").await;
    let gate = Gate::install(
        &owner,
        "payroll_run_exceptions",
        "SELECT COUNT(*) FROM payroll_run_exceptions WHERE run_id = $1",
        true,
    )
    .await;
    let path = format!("{PAYROLL_RUNS_PATH}/{}/exceptions", f.run);
    let mut pending = start_read(&f, path.clone());
    let (gate, mode) = gate.require_observed(&owner, &mut pending).await;
    add_exception(&owner, &f, "After").await;
    gate.release().await;
    let original = finish_read(pending).await;
    let fresh = get(app(f.rt.clone(), &f.keys), &path, &f.token).await;
    assert_eq!(fresh.status, StatusCode::OK);
    assert_eq!(
        (
            fresh.json["total"].as_i64().unwrap(),
            fresh.json["open"].as_i64().unwrap(),
            fresh.json["items"].as_array().unwrap().len()
        ),
        (2, 2, 2)
    );
    assert_eq!(
        audit_count(&owner, f.actor, "payroll_run.exceptions_read").await,
        2
    );
    assert_eq!(
        (
            original.json["total"].as_i64().unwrap(),
            original.json["open"].as_i64().unwrap(),
            original.json["items"].as_array().unwrap().len()
        ),
        (1, 1, 1)
    );
    assert_eq!(mode, 2, "read must use repeatable-read snapshot");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn close_preflight_count_and_blocking_refs_share_a_snapshot(owner: PgPool) {
    let f = fixture(&owner).await;
    let gate = Gate::install(&owner, "payroll_draft_lines", "SELECT COUNT(*) FROM payroll_draft_lines WHERE run_id = $1 AND attendance_source_row_count = 0 AND attendance_event_count = 0", true).await;
    let path = format!("{PAYROLL_RUNS_PATH}/{}/close-preflight", f.run);
    let mut pending = start_read(&f, path.clone());
    let (gate, mode) = gate.require_observed(&owner, &mut pending).await;
    sqlx::query("UPDATE payroll_draft_lines SET attendance_event_count = 1 WHERE run_id = $1")
        .bind(f.run)
        .execute(&owner)
        .await
        .unwrap();
    gate.release().await;
    let original = finish_read(pending).await;
    let fresh = get(app(f.rt.clone(), &f.keys), &path, &f.token).await;
    assert_eq!(fresh.status, StatusCode::OK);
    let check = |response: &Value| {
        response["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["key"] == "attendance_material")
            .unwrap()
            .clone()
    };
    assert_eq!(check(&fresh.json)["ok"], true);
    let before = check(&original.json);
    assert_eq!(before["ok"], false);
    assert_eq!(before["blocking_refs"].as_array().unwrap().len(), 1);
    assert_eq!(mode, 2, "read must use repeatable-read snapshot");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn self_link_and_page_share_a_snapshot(owner: PgPool) {
    let mut f = fixture(&owner).await;
    let other = seed_employee(&owner, *f.org.as_uuid(), "Other").await;
    seed_line(&owner, *f.org.as_uuid(), f.run, other, "Other").await;
    sqlx::query("UPDATE payroll_draft_lines SET work_days = 3 WHERE employee_id = $1")
        .bind(other)
        .execute(&owner)
        .await
        .unwrap();
    let member = UserId::new();
    seed_user_linked_to_employee(&owner, member, *f.org.as_uuid(), f.employee).await;
    f.token = bearer(&f.rt, &f.keys, member, f.org, "MEMBER").await;
    let gate = Gate::install(
        &owner,
        "users",
        "SELECT employee_id FROM users WHERE id = $1",
        true,
    )
    .await;
    let mut pending = start_read(&f, PAYROLL_MY_PAYSLIPS_PATH.to_owned());
    let (gate, mode) = gate.require_observed(&owner, &mut pending).await;
    let mut changed = owner.begin().await.unwrap();
    sqlx::query("UPDATE users SET employee_id = $1 WHERE id = $2")
        .bind(other)
        .bind(member.as_uuid())
        .execute(changed.as_mut())
        .await
        .unwrap();
    sqlx::query("UPDATE payroll_draft_lines SET work_days = 9 WHERE employee_id = $1")
        .bind(f.employee)
        .execute(changed.as_mut())
        .await
        .unwrap();
    changed.commit().await.unwrap();
    gate.release().await;
    let original = finish_read(pending).await;
    let fresh = get(
        app(f.rt.clone(), &f.keys),
        PAYROLL_MY_PAYSLIPS_PATH,
        &f.token,
    )
    .await;
    assert_eq!(fresh.status, StatusCode::OK);
    assert_eq!(fresh.json["total"], 1);
    assert_eq!(fresh.json["items"][0]["work_days"].as_f64(), Some(3.0));
    assert_eq!(original.json["total"], 1);
    assert_eq!(original.json["items"][0]["work_days"], Value::Null);
    assert_eq!(mode, 2, "read must use repeatable-read snapshot");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn delivery_count_and_items_share_a_snapshot(owner: PgPool) {
    let f = fixture(&owner).await;
    let doc = add_delivery(&owner, &f, f.employee).await;
    let other = seed_employee(&owner, *f.org.as_uuid(), "Other").await;
    seed_line(&owner, *f.org.as_uuid(), f.run, other, "Other").await;
    let gate = Gate::install(
        &owner,
        "payroll_payslip_deliveries",
        "SELECT COUNT(*)::BIGINT AS issued,",
        false,
    )
    .await;
    let path = format!("{PAYROLL_RUNS_PATH}/{}/payslip-delivery", f.run);
    let mut pending = start_read(&f, path.clone());
    let (gate, mode) = gate.require_observed(&owner, &mut pending).await;
    let added = add_delivery(&owner, &f, other).await;
    assert_ne!(doc, added);
    gate.release().await;
    let original = finish_read(pending).await;
    let fresh = get(app(f.rt.clone(), &f.keys), &path, &f.token).await;
    assert_eq!(fresh.status, StatusCode::OK);
    assert_eq!(fresh.json["issued"], 2);
    assert_eq!(fresh.json["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        (
            original.json["issued"].as_i64().unwrap(),
            original.json["items"].as_array().unwrap().len()
        ),
        (1, 1)
    );
    assert_eq!(mode, 2, "read must use repeatable-read snapshot");
}

async fn add_delivery(owner: &PgPool, f: &Fixture, employee: Uuid) -> Uuid {
    let doc:Uuid=sqlx::query_scalar("INSERT INTO inbox_docs (org_id,recipient_user_id,kind,title,legal_basis,source_kind,source_id,payload) VALUES ($1,$2,'payslip','Fixture','{}','payroll_run',$3,'{}') RETURNING id").bind(f.org.as_uuid()).bind(f.actor.as_uuid()).bind(f.run.to_string()).fetch_one(owner).await.unwrap();
    sqlx::query("INSERT INTO payroll_payslip_deliveries (org_id,run_id,line_id,employee_id,inbox_doc_id) SELECT org_id,run_id,id,employee_id,$1 FROM payroll_draft_lines WHERE run_id=$2 AND employee_id=$3").bind(doc).bind(f.run).bind(employee).execute(owner).await.unwrap();
    doc
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn standalone_run_list_count_and_page_share_a_snapshot(owner: PgPool) {
    let f = fixture(&owner).await;
    let gate = Gate::install(
        &owner,
        "payroll_draft_runs",
        "SELECT COUNT(*) FROM payroll_draft_runs",
        true,
    )
    .await;
    let store = PgPayrollStore::new(f.rt.clone());
    let org = f.org;
    let mut pending = tokio::spawn(console_platform_request_context::scope_org(
        org,
        async move { store.list_runs(None, None).await.unwrap() },
    ));
    let (gate, mode) = gate.require_observed(&owner, &mut pending).await;
    assert_ne!(add_next_period_run(&owner, &f).await, f.run);
    gate.release().await;
    let original = finish_task(pending).await;
    let fresh = console_platform_request_context::scope_org(
        f.org,
        PgPayrollStore::new(f.rt.clone()).list_runs(None, None),
    )
    .await
    .unwrap();
    assert_eq!((fresh.total, fresh.items.len()), (2, 2));
    assert_eq!((original.total, original.items.len()), (1, 1));
    assert_eq!(mode, 2, "read must use repeatable-read snapshot");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn standalone_run_detail_count_and_page_share_a_snapshot(owner: PgPool) {
    let f = fixture(&owner).await;
    let other = seed_employee(&owner, *f.org.as_uuid(), "Other").await;
    let gate = Gate::install(
        &owner,
        "payroll_draft_lines",
        "SELECT COUNT(*) FROM payroll_draft_lines WHERE run_id = $1",
        true,
    )
    .await;
    let store = PgPayrollStore::new(f.rt.clone());
    let (org, run) = (f.org, f.run);
    let mut pending = tokio::spawn(console_platform_request_context::scope_org(
        org,
        async move { store.get_run(run, None, None).await.unwrap().unwrap() },
    ));
    let (gate, mode) = gate.require_observed(&owner, &mut pending).await;
    seed_line(&owner, *f.org.as_uuid(), f.run, other, "Other").await;
    gate.release().await;
    let original = finish_task(pending).await;
    let fresh = console_platform_request_context::scope_org(
        f.org,
        PgPayrollStore::new(f.rt.clone()).get_run(f.run, None, None),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!((fresh.lines_total, fresh.lines.len()), (2, 2));
    assert_eq!((original.lines_total, original.lines.len()), (1, 1));
    assert_eq!(mode, 2, "read must use repeatable-read snapshot");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn action_inbox_count_and_page_share_a_snapshot(owner: PgPool) {
    let f = fixture(&owner).await;
    let submitter = UserId::new();
    seed_user(&owner, submitter, *f.org.as_uuid(), "EXECUTIVE").await;
    let submitted = OffsetDateTime::now_utc() - Duration::hours(1);
    sqlx::query("UPDATE payroll_draft_runs SET status='SUBMITTED', submitted_by=$1, submitted_at=$2 WHERE id=$3").bind(submitter.as_uuid()).bind(submitted).bind(f.run).execute(&owner).await.unwrap();
    let gate = Gate::install(
        &owner,
        "payroll_draft_runs",
        "SELECT COUNT(*) FROM payroll_draft_runs WHERE status = 'SUBMITTED'",
        false,
    )
    .await;
    let store = PgPayrollStore::new(f.rt.clone());
    let (org, actor) = (f.org, f.actor);
    let as_of = OffsetDateTime::now_utc();
    let mut pending = tokio::spawn(console_platform_request_context::scope_org(
        org,
        async move {
            store
                .list_submitted_action_inbox_page(actor, as_of, None, 100)
                .await
                .unwrap()
        },
    ));
    let (gate, mode) = gate.require_observed(&owner, &mut pending).await;
    sqlx::query("UPDATE payroll_draft_runs SET status='BLOCKED_LEGAL_GATE' WHERE id=$1")
        .bind(f.run)
        .execute(&owner)
        .await
        .unwrap();
    gate.release().await;
    let original = finish_task(pending).await;
    let fresh = console_platform_request_context::scope_org(
        f.org,
        PgPayrollStore::new(f.rt.clone())
            .list_submitted_action_inbox_page(f.actor, as_of, None, 100),
    )
    .await
    .unwrap();
    assert_eq!((fresh.1, fresh.0.len(), fresh.2), (0, 0, false));
    assert_eq!((original.1, original.0.len(), original.2), (1, 1, false));
    assert_eq!(original.0[0].id, f.run);
    assert_eq!(mode, 2, "read must use repeatable-read snapshot");
}

fn read_paths(run: Uuid) -> Vec<String> {
    vec![
        format!("{PAYROLL_RUNS_PATH}/{run}"),
        format!("{PAYROLL_RUNS_PATH}/{run}/exceptions"),
        format!("{PAYROLL_RUNS_PATH}/{run}/close-preflight"),
        format!("{PAYROLL_RUNS_PATH}/{run}/payslip-delivery"),
    ]
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn audited_reads_refuse_data_when_audit_insert_fails(owner: PgPool) {
    let f = fixture(&owner).await;
    sqlx::raw_sql("CREATE FUNCTION reject_payroll_read_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.action LIKE 'payroll_run.%read' THEN RAISE EXCEPTION 'test audit refusal'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_payroll_read_audit BEFORE INSERT ON audit_events FOR EACH ROW EXECUTE FUNCTION reject_payroll_read_audit();").execute(&owner).await.unwrap();
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
        .fetch_one(&owner)
        .await
        .unwrap();
    let mut paths = read_paths(f.run);
    paths.push(PAYROLL_RUNS_PATH.to_owned());
    for path in paths {
        let refused = get(app(f.rt.clone(), &f.keys), &path, &f.token).await;
        assert_eq!(
            refused.status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{path}: {}",
            refused.json
        );
        assert_eq!(
            refused.json.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["error"]
        );
        assert!(!refused.json.to_string().contains("Before"));
    }
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
        .fetch_one(&owner)
        .await
        .unwrap();
    assert_eq!(after, before, "failed reads must not append success audits");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn affected_gets_preserve_absence_roles_and_revoked_family_denials(owner: PgPool) {
    let f = fixture(&owner).await;
    let other_org = Uuid::new_v4();
    seed_org(&owner, other_org, "snapshot-foreign").await;
    let other_actor = UserId::new();
    seed_user(&owner, other_actor, other_org, "EXECUTIVE").await;
    let foreign = seed_run(&owner, other_org, other_actor).await;
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
        .fetch_one(&owner)
        .await
        .unwrap();
    for run in [foreign, Uuid::new_v4()] {
        for path in read_paths(run) {
            let missing = get(app(f.rt.clone(), &f.keys), &path, &f.token).await;
            assert_eq!(
                missing.status,
                StatusCode::NOT_FOUND,
                "{path}: {}",
                missing.json
            );
        }
    }
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
        .fetch_one(&owner)
        .await
        .unwrap();
    assert_eq!(
        after, before,
        "absent and foreign reads must not append audits"
    );
    let member = UserId::new();
    seed_user(&owner, member, *f.org.as_uuid(), "MEMBER").await;
    let member_token = bearer(&f.rt, &f.keys, member, f.org, "MEMBER").await;
    let mut paths = read_paths(f.run);
    paths.push(PAYROLL_RUNS_PATH.to_owned());
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
        .fetch_one(&owner)
        .await
        .unwrap();
    for path in &paths {
        let denied = get(app(f.rt.clone(), &f.keys), path, &member_token).await;
        assert_eq!(
            denied.status,
            StatusCode::FORBIDDEN,
            "{path}: {}",
            denied.json
        );
    }
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
        .fetch_one(&owner)
        .await
        .unwrap();
    assert_eq!(after, before, "role-denied reads must not append audits");
    let admin = UserId::new();
    seed_user(&owner, admin, *f.org.as_uuid(), "ADMIN").await;
    let admin_token = bearer(&f.rt, &f.keys, admin, f.org, "ADMIN").await;
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
        .fetch_one(&owner)
        .await
        .unwrap();
    for path in paths.iter().cloned().chain(read_paths(Uuid::new_v4())) {
        assert_eq!(
            get(app(f.rt.clone(), &f.keys), &path, &admin_token)
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            get_unauthenticated(app(f.rt.clone(), &f.keys), &path).await,
            StatusCode::UNAUTHORIZED
        );
    }
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
        .fetch_one(&owner)
        .await
        .unwrap();
    assert_eq!(
        after, before,
        "admin and unauthenticated reads must not append audits"
    );
    sqlx::query("UPDATE auth_refresh_token_families SET revoked_at=now() WHERE user_id=$1")
        .bind(f.actor.as_uuid())
        .execute(&owner)
        .await
        .unwrap();
    paths.push(PAYROLL_MY_PAYSLIPS_PATH.to_owned());
    for path in paths {
        let denied = get(app(f.rt.clone(), &f.keys), &path, &f.token).await;
        assert_eq!(
            denied.status,
            StatusCode::UNAUTHORIZED,
            "{path}: {}",
            denied.json
        );
    }
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
        .fetch_one(&owner)
        .await
        .unwrap();
    assert_eq!(after, before, "denied reads must not append success audits");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn mutation_audits_keep_default_isolation_and_rollback_on_refusal(owner: PgPool) {
    let f = fixture(&owner).await;
    use console_platform_db::with_audits;
    let employee = f.employee;
    let company = f.org;
    let (isolation, read_only, org): (String, String, String) = with_audits::<_, _, DbError>(&f.rt, f.org, |tx| Box::pin(async move {
        let settings = sqlx::query_as("SELECT current_setting('transaction_isolation'), current_setting('transaction_read_only'), current_setting('app.current_org')").fetch_one(tx.as_mut()).await?;
        sqlx::query("UPDATE employees SET name='Committed' WHERE id=$1").bind(employee).execute(tx.as_mut()).await?;
        Ok((settings, vec![test_audit_event("test.snapshot_mutation", "employee", employee, *company.as_uuid())]))
    })).await.unwrap();
    assert_eq!(
        (isolation.as_str(), read_only.as_str(), org),
        ("read committed", "off", f.org.to_string())
    );
    let refused = with_audits::<_, (), DbError>(&f.rt, f.org, move |tx| {
        Box::pin(async move {
            sqlx::query("UPDATE employees SET name='MustRollback' WHERE id=$1")
                .bind(employee)
                .execute(tx.as_mut())
                .await?;
            Err(DbError::Sqlx(sqlx::Error::RowNotFound))
        })
    })
    .await;
    assert!(refused.is_err());
    let name: String = sqlx::query_scalar("SELECT name FROM employees WHERE id=$1")
        .bind(employee)
        .fetch_one(&owner)
        .await
        .unwrap();
    assert_eq!(name, "Committed");
    sqlx::raw_sql("CREATE FUNCTION reject_second_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.action='test.refuse_snapshot_mutation' THEN RAISE EXCEPTION 'test audit refusal'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_second_audit BEFORE INSERT ON audit_events FOR EACH ROW EXECUTE FUNCTION reject_second_audit();").execute(&owner).await.unwrap();
    let org = f.org;
    let refused = with_audits::<_, (), DbError>(&f.rt, org, move |tx| {
        Box::pin(async move {
            sqlx::query("UPDATE employees SET name='MustRollback' WHERE id=$1")
                .bind(employee)
                .execute(tx.as_mut())
                .await?;
            Ok((
                (),
                vec![
                    test_audit_event("test.before_refusal", "employee", employee, *org.as_uuid()),
                    test_audit_event(
                        "test.refuse_snapshot_mutation",
                        "employee",
                        employee,
                        *org.as_uuid(),
                    ),
                ],
            ))
        })
    })
    .await;
    assert!(refused.is_err());
    let name: String = sqlx::query_scalar("SELECT name FROM employees WHERE id=$1")
        .bind(employee)
        .fetch_one(&owner)
        .await
        .unwrap();
    assert_eq!(name, "Committed");
    let partial: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE action IN ('test.before_refusal','test.refuse_snapshot_mutation')").fetch_one(&owner).await.unwrap();
    assert_eq!(
        partial, 0,
        "prior audit and business write must roll back together"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn standalone_self_page_count_and_items_share_a_snapshot(owner: PgPool) {
    let f = fixture(&owner).await;
    let gate = Gate::install(
        &owner,
        "payroll_draft_lines",
        "SELECT COUNT(*) FROM payroll_draft_lines WHERE employee_id = $1",
        true,
    )
    .await;
    let store = PgPayrollStore::new(f.rt.clone());
    let (org, employee) = (f.org, f.employee);
    let mut pending = tokio::spawn(console_platform_request_context::scope_org(
        org,
        async move { store.list_my_lines(employee, None, None).await.unwrap() },
    ));
    let (gate, mode) = gate.require_observed(&owner, &mut pending).await;
    let added = add_next_period_run(&owner, &f).await;
    seed_line(&owner, *f.org.as_uuid(), added, f.employee, "Before").await;
    gate.release().await;
    let original = finish_task(pending).await;
    let fresh = console_platform_request_context::scope_org(
        f.org,
        PgPayrollStore::new(f.rt.clone()).list_my_lines(f.employee, None, None),
    )
    .await
    .unwrap();
    assert_eq!((fresh.total, fresh.items.len()), (2, 2));
    assert_eq!((original.total, original.items.len()), (1, 1));
    assert_eq!(mode, 2, "read must use repeatable-read snapshot");
}
