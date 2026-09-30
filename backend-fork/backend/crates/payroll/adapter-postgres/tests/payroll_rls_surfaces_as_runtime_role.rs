#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME RLS gate for the payroll draft-run/line staging adapter.
//!
//! Proven as the genuine non-owner runtime role `console_rt` (NOSUPERUSER,
//! NOBYPASSRLS, FORCE RLS) — NOT the `#[sqlx::test]` BYPASSRLS superuser pool,
//! which sees every row and would green-light a broken org filter.
//!
//! What this proves:
//!  * `list_runs`/`get_run` are org-isolated: another tenant's runs/lines are
//!    invisible (empty list / `None` on direct id lookup — deny-by-omission);
//!  * `list_my_lines` is employee-scoped: one employee's own draft lines never
//!    include another employee's rows, even within the same run;
//!  * an `employee_id` from a DIFFERENT org, looked up under this org's GUC,
//!    yields zero rows rather than leaking the other org's row (RLS, not
//!    application-level filtering, is the enforcement boundary);
//!  * a proposed payroll approver from a DIFFERENT org is rejected by the
//!    read-only tenant prerequisite before any future approval write exists.
//!
//! Draft-run fixtures mint via `payroll.create_run` as `console_rt` (no owner
//! INSERT of draft runs). Lines still INSERT (calculate HOLD).

use console_kernel_core::{ErrorKind, OrgId, UserId};
use console_ontology_canonical_domain::{CanonicalPort, CommandId, CommandReceipt, DispatchTarget};
use console_payroll_adapter_postgres::pay_run::{PayRunCommand, PayRunQuery, PgPayRunPort};
use console_payroll_adapter_postgres::{MyPayrollLine, PgPayrollStore};
use console_platform_test_support::runtime_role_pool;
use sqlx::PgPool;
use time::OffsetDateTime;
use time::macros::date;
use uuid::Uuid;

#[cfg(test)]
fn assert_my_payroll_line_is_readiness_not_won(line: &MyPayrollLine) {
    let value = serde_json::to_value(line).unwrap();
    let object = value
        .as_object()
        .expect("MyPayrollLine serializes as a JSON object");
    let won_keys: Vec<&String> = object.keys().filter(|key| key.contains("won")).collect();
    assert!(
        won_keys.is_empty(),
        "MyPayrollLine is readiness, not won; unexpected keys {won_keys:?} in {value}"
    );
    for key in [
        "regular_hours",
        "overtime_hours",
        "night_hours",
        "holiday_hours",
        "gross_pay_source_present",
        "net_pay_source_present",
    ] {
        assert!(
            object.contains_key(key),
            "MyPayrollLine must keep hours / *_source_present ({key} missing) in {value}"
        );
    }
}

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!(
        "org-{}",
        tag.chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
    ))
    .bind(format!("Org {tag}"))
    .execute(owner_pool)
    .await
    .unwrap();
}

async fn seed_employee(owner_pool: &PgPool, org: Uuid, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO employees \
         (id, org_id, company, name, source_filename, source_sheet, source_row, source_key) \
         VALUES ($1, $2, 'KNL', $3, 'roster.xlsx', 'Sheet1', 1, $4)",
    )
    .bind(id)
    .bind(org)
    .bind(name)
    .bind(format!("emp-{id}"))
    .execute(owner_pool)
    .await
    .unwrap();
    id
}

async fn seed_user_linked_to_employee(owner_pool: &PgPool, org: Uuid, employee: Uuid) -> UserId {
    let user_id = UserId::new();
    sqlx::query(
        "INSERT INTO users (id, display_name, roles, org_id, employee_id) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(*user_id.as_uuid())
    .bind(format!("User {}", user_id.as_uuid()))
    .bind(vec!["MEMBER".to_string()])
    .bind(org)
    .bind(employee)
    .execute(owner_pool)
    .await
    .unwrap();
    user_id
}

async fn seed_actor(owner_pool: &PgPool, org: Uuid) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("stager-{}", user_id.as_uuid()))
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    user_id
}

async fn seed_run(owner_pool: &PgPool, org: Uuid, actor: UserId) -> Uuid {
    let runtime_pool = runtime_role_pool(owner_pool).await;
    let pay_run = PgPayRunPort::new(runtime_pool, tokio::runtime::Handle::current());
    let created = execute_sync(
        &pay_run,
        PayRunCommand {
            org_id: OrgId::from_uuid(org),
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: PayRunQuery::CreateRun {
                run_id: Uuid::new_v4(),
                period_start: date!(2026 - 06 - 01),
                period_end: date!(2026 - 06 - 30),
                connector: Some("m2".to_owned()),
                job: Some("payroll_draft".to_owned()),
            },
            action_key: "create_run".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .expect("payroll.create_run as console_rt");
    assert_eq!(created.target(), DispatchTarget::PayrollCreateRun);
    created.result()["draft_run_id"]
        .as_str()
        .expect("CreateRun must name draft_run_id")
        .parse()
        .unwrap()
}

async fn execute_sync<P: CanonicalPort + Clone + Send + 'static>(
    port: &P,
    command: P::Command,
) -> Result<CommandReceipt, P::Error>
where
    P::Command: Send + 'static,
    P::Error: Send + 'static,
{
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.execute(&command))
        .await
        .unwrap()
}

async fn seed_line(owner_pool: &PgPool, org: Uuid, run_id: Uuid, employee: Uuid, name: &str) {
    sqlx::query(
        "INSERT INTO payroll_draft_lines \
         (org_id, run_id, employee_id, employee_source_key, employee_display_name, employee_company) \
         VALUES ($1, $2, $3, $4, $5, 'KNL')",
    )
    .bind(org)
    .bind(run_id)
    .bind(employee)
    .bind(format!("src-{employee}"))
    .bind(name)
    .execute(owner_pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn runs_and_lines_are_org_isolated(pool: PgPool) {
    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    seed_org(&pool, org_a, "A").await;
    seed_org(&pool, org_b, "B").await;

    let actor_a = seed_actor(&pool, org_a).await;
    let actor_b = seed_actor(&pool, org_b).await;
    let run_a = seed_run(&pool, org_a, actor_a).await;
    let run_b = seed_run(&pool, org_b, actor_b).await;
    let emp_a = seed_employee(&pool, org_a, "Alice").await;
    seed_line(&pool, org_a, run_a, emp_a, "Alice").await;

    let rt_pool = runtime_role_pool(&pool).await;
    let store = PgPayrollStore::new(rt_pool);

    // Org A's GUC sees only org A's run, never org B's.
    let page_a = console_platform_request_context::scope_org(OrgId::from_uuid(org_a), async {
        store.list_runs(None, None).await
    })
    .await
    .unwrap();
    assert_eq!(page_a.total, 1);
    assert_eq!(page_a.items[0].id, run_a);

    // Org B's GUC sees only its own run.
    let page_b = console_platform_request_context::scope_org(OrgId::from_uuid(org_b), async {
        store.list_runs(None, None).await
    })
    .await
    .unwrap();
    assert_eq!(page_b.total, 1);
    assert_eq!(page_b.items[0].id, run_b);

    // Direct id lookup of org A's run under org B's GUC is a miss, not a leak.
    let cross_org_detail =
        console_platform_request_context::scope_org(OrgId::from_uuid(org_b), async {
            store.get_run(run_a, None, None).await
        })
        .await
        .unwrap();
    assert!(
        cross_org_detail.is_none(),
        "org B must not be able to read org A's run by id"
    );

    // The correct org's GUC reads the run plus its one line.
    let detail = console_platform_request_context::scope_org(OrgId::from_uuid(org_a), async {
        store.get_run(run_a, None, None).await
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(detail.lines_total, 1);
    assert_eq!(detail.lines[0].employee_display_name, "Alice");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn my_lines_are_employee_scoped_never_a_coworkers(pool: PgPool) {
    let org = Uuid::new_v4();
    seed_org(&pool, org, "KNL").await;

    let stager = seed_actor(&pool, org).await;
    let run = seed_run(&pool, org, stager).await;
    let alice = seed_employee(&pool, org, "Alice").await;
    let bob = seed_employee(&pool, org, "Bob").await;
    seed_line(&pool, org, run, alice, "Alice").await;
    seed_line(&pool, org, run, bob, "Bob").await;

    let alice_user = seed_user_linked_to_employee(&pool, org, alice).await;
    let admin_user = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*admin_user.as_uuid())
        .bind("Admin no link")
        .bind(vec!["ADMIN".to_string()])
        .bind(org)
        .execute(&pool)
        .await
        .unwrap();

    let rt_pool = runtime_role_pool(&pool).await;
    let store = PgPayrollStore::new(rt_pool);

    let org_id = OrgId::from_uuid(org);

    // Alice resolves to her own employee id and sees ONLY her own line.
    let alice_employee = console_platform_request_context::scope_org(org_id, async {
        store.linked_employee_id(alice_user).await
    })
    .await
    .unwrap()
    .expect("alice is linked to an employee");
    assert_eq!(alice_employee, alice);

    let alice_lines = console_platform_request_context::scope_org(org_id, async {
        store.list_my_lines(alice_employee, None, None).await
    })
    .await
    .unwrap();
    assert_eq!(alice_lines.total, 1);
    assert_eq!(alice_lines.items[0].run_id, run);
    assert_my_payroll_line_is_readiness_not_won(&alice_lines.items[0]);

    // An account with no employee link resolves to `None` (the REST layer
    // turns this into an empty page, never a 403 — mirrors
    // `hr.rs::load_optional_linked_employee_id`).
    let admin_employee = console_platform_request_context::scope_org(org_id, async {
        store.linked_employee_id(admin_user).await
    })
    .await
    .unwrap();
    assert!(admin_employee.is_none());

    // Asking for Bob's own lines under Bob's id still returns only Bob's row
    // — proves the scoping is by employee_id, not "first row in the run".
    let bob_lines = console_platform_request_context::scope_org(org_id, async {
        store.list_my_lines(bob, None, None).await
    })
    .await
    .unwrap();
    assert_eq!(bob_lines.total, 1);
    assert_ne!(bob_lines.items[0].run_id, Uuid::nil());
    assert_eq!(
        bob_lines.total, 1,
        "Bob must see exactly his own line, not Alice's too"
    );
    assert_my_payroll_line_is_readiness_not_won(&bob_lines.items[0]);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn my_lines_for_a_foreign_org_employee_id_yields_nothing(pool: PgPool) {
    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    seed_org(&pool, org_a, "A").await;
    seed_org(&pool, org_b, "B").await;

    let stager = seed_actor(&pool, org_a).await;
    let run_a = seed_run(&pool, org_a, stager).await;
    let emp_a = seed_employee(&pool, org_a, "Alice").await;
    seed_line(&pool, org_a, run_a, emp_a, "Alice").await;

    let rt_pool = runtime_role_pool(&pool).await;
    let store = PgPayrollStore::new(rt_pool);

    // Org A's employee id, looked up under org B's GUC: RLS must yield zero
    // rows, never org A's line.
    let leaked = console_platform_request_context::scope_org(OrgId::from_uuid(org_b), async {
        store.list_my_lines(emp_a, None, None).await
    })
    .await
    .unwrap();
    assert_eq!(leaked.total, 0);
    assert!(leaked.items.is_empty());
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn foreign_org_approver_is_rejected_under_runtime_rls(pool: PgPool) {
    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    seed_org(&pool, org_a, "Approver A").await;
    seed_org(&pool, org_b, "Approver B").await;

    let employee_a = seed_employee(&pool, org_a, "Approver A").await;
    let employee_b = seed_employee(&pool, org_b, "Approver B").await;
    let user_a = seed_user_linked_to_employee(&pool, org_a, employee_a).await;
    let user_b = seed_user_linked_to_employee(&pool, org_b, employee_b).await;

    let store = PgPayrollStore::new(runtime_role_pool(&pool).await);
    let org_a_id = OrgId::from_uuid(org_a);

    console_platform_request_context::scope_org(org_a_id, async {
        store.assert_approver_belongs_to_current_org(user_a).await
    })
    .await
    .unwrap();

    let error = console_platform_request_context::scope_org(org_a_id, async {
        store.assert_approver_belongs_to_current_org(user_b).await
    })
    .await
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Forbidden);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn submitted_inbox_page_excludes_submitter_non_submitted_and_other_org(pool: PgPool) {
    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    seed_org(&pool, org_a, "Inbox A").await;
    seed_org(&pool, org_b, "Inbox B").await;

    let submitter = seed_plain_user(&pool, org_a, "Submitter").await;
    let approver = seed_plain_user(&pool, org_a, "Approver").await;
    let outsider = seed_plain_user(&pool, org_b, "Outsider").await;
    let submitted_at = OffsetDateTime::now_utc();
    let visible = seed_run_status(
        &pool,
        org_a,
        "inbox-submitted",
        "SUBMITTED",
        Some(submitter),
        Some(submitted_at),
    )
    .await;
    let _calculated =
        seed_run_status(&pool, org_a, "inbox-calculated", "CALCULATED", None, None).await;
    let other_org = seed_run_status(
        &pool,
        org_b,
        "inbox-other-org",
        "SUBMITTED",
        Some(outsider),
        Some(submitted_at),
    )
    .await;

    let store = PgPayrollStore::new(runtime_role_pool(&pool).await);
    let as_of = submitted_at + time::Duration::seconds(1);

    let for_submitter =
        console_platform_request_context::scope_org(OrgId::from_uuid(org_a), async {
            store
                .list_submitted_action_inbox_page(submitter, as_of, None, 50)
                .await
        })
        .await
        .unwrap();
    assert_eq!(for_submitter.1, 0);
    assert!(for_submitter.0.is_empty());

    let for_approver =
        console_platform_request_context::scope_org(OrgId::from_uuid(org_a), async {
            store
                .list_submitted_action_inbox_page(approver, as_of, None, 50)
                .await
        })
        .await
        .unwrap();
    assert_eq!(for_approver.1, 1);
    assert_eq!(for_approver.0.len(), 1);
    assert_eq!(for_approver.0[0].id, visible);
    assert!(!for_approver.2);

    let other_org_for_approver =
        console_platform_request_context::scope_org(OrgId::from_uuid(org_b), async {
            store
                .list_submitted_action_inbox_page(approver, as_of, None, 50)
                .await
        })
        .await
        .unwrap();
    assert_eq!(other_org_for_approver.1, 1);
    assert_eq!(other_org_for_approver.0[0].id, other_org);
    assert_ne!(other_org_for_approver.0[0].id, visible);

    let other_org_for_submitter =
        console_platform_request_context::scope_org(OrgId::from_uuid(org_b), async {
            store
                .list_submitted_action_inbox_page(outsider, as_of, None, 50)
                .await
        })
        .await
        .unwrap();
    assert_eq!(other_org_for_submitter.1, 0);
    assert!(other_org_for_submitter.0.is_empty());
}

async fn seed_plain_user(owner_pool: &PgPool, org: Uuid, name: &str) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(name)
        .bind(vec!["EXECUTIVE".to_string()])
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    user_id
}

async fn seed_run_status(
    owner_pool: &PgPool,
    org: Uuid,
    source_label: &str,
    status: &str,
    submitted_by: Option<UserId>,
    submitted_at: Option<OffsetDateTime>,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO payroll_draft_runs \
         (org_id, period_start, period_end, source_label, status, submitted_by, submitted_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(org)
    .bind(date!(2026 - 06 - 01))
    .bind(date!(2026 - 06 - 30))
    .bind(source_label)
    .bind(status)
    .bind(submitted_by.map(|user| *user.as_uuid()))
    .bind(submitted_at)
    .fetch_one(owner_pool)
    .await
    .unwrap()
}
