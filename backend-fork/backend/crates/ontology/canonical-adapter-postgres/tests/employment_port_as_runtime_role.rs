#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `EmploymentPort` proven against a REAL PostgreSQL as the genuine runtime role
//! `console_rt` — never the BYPASSRLS superuser the `#[sqlx::test]` pool
//! connects as, which would mask a broken `org_isolation` policy.
//! `a_foreign_tenant_is_invisible_and_unwritable_to_the_runtime_role` is what
//! makes that claim observable: it is the only test that crosses a tenant
//! boundary, it asserts through the owner pool that the foreign rows genuinely
//! EXIST first, and it dies when `org_isolation` is loosened to `USING (true)`.
//!
//! WHY `#[sqlx::test]` IS NOT OPTIONAL HERE. Migration 0196 refuses a superuser
//! applier unless `CURRENT_DATABASE()` matches `^_sqlx_test_[A-Za-z0-9_]{52}$`
//! with the `console.sqlx_test_bootstrap` marker set, so the schema itself
//! admits exactly one applier and a hand-rolled `sqlx::migrate!` harness is not
//! a design choice that was available.
//!
//! WHY `execute` IS CALLED FROM `spawn_blocking`. `CanonicalPort::execute` is
//! SYNCHRONOUS — `canonical-domain` declares it so and this lane may not edit
//! that crate — so the adapter bridges to `sqlx` with `Handle::block_on`, which
//! panics inside an async context. A `spawn_blocking` thread is not one.
//!
//! IMMUTABILITY IS THE TRIGGER, AND ONLY THE TRIGGER.
//! `ops/postgres-reconcile-topology.sh` runs `ALTER DEFAULT PRIVILEGES FOR ROLE
//! console_app … GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO console_rt`
//! BEFORE migrations, so in the deployed database `console_rt` DOES hold UPDATE
//! on `employment_revisions` — 0214's own header says so.
//! `#[sqlx::test]` applies migrations as the cluster superuser, for whom no such
//! per-role default privilege exists, so an UPDATE here would otherwise be
//! refused by a `42501` that does not exist where the code ships.
//! `a_revision_is_append_only_and_the_trigger_is_what_refuses_the_update`
//! therefore GRANTs UPDATE first, asserts the grant is really held, and only
//! then asserts the `P0001` the trigger raises.
//!
//! THE LEGACY HEAD IS PART OF THE PORT, NOT NEXT TO IT. `employees` is the
//! fourth table the contract assigns to `Employment`, and
//! `a_promotion_carries_the_new_state_onto_the_legacy_employees_head` is the
//! test that makes the relocation of `backend/app/src/hr.rs`'s three statements
//! meaningful rather than cosmetic: the same
//! `employment::apply_employment_change` the REST lifecycle handler now calls is
//! the statement the port itself issues.

use console_kernel_core::{ErrorKind, OrgId, UserId};
use console_ontology_canonical_adapter_postgres::company::{
    CompanyCommand, CompanyQuery, PgCompanyPort,
};
use console_ontology_canonical_adapter_postgres::employment::{
    EmploymentAttributes, EmploymentChange, EmploymentCommand, EmploymentError, EmploymentHead,
    EmploymentQuery, NewEmployeeRecord, PgEmploymentPort, apply_employment_change,
    insert_employee_record, reassign_org_unit_via_transfers_in_tx,
};
use console_ontology_canonical_adapter_postgres::job_position::{
    JobPositionCommand, JobPositionQuery, PgJobPositionPort,
};
use console_ontology_canonical_adapter_postgres::org_unit::{
    OrgUnitCommand, OrgUnitQuery, PgOrgUnitPort,
};
use console_ontology_canonical_adapter_postgres::person::{
    PersonCommand, PersonQuery, PgPersonPort,
};
use console_ontology_canonical_domain::{
    CanonicalPort, CommandId, CommandReceipt, DispatchTarget, EmploymentPort, ObjectKey,
    ReceiptOwner,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use time::{Date, OffsetDateTime, macros::offset};
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0001);
const FOREIGN_ORG: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0002);
const ORG_UNIT_SALES: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0010);
const ORG_UNIT_TECH: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0011);
const ORG_UNIT_ADMIN: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0012);
const JOB_STAFF: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0020);
const JOB_LEAD: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0021);
const JOB_EXEC: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0022);

/// The port must satisfy the NAMED trait, not merely `CanonicalPort`. The
/// blanket impl in `canonical-domain` makes `EmploymentPort` an alias for
/// `CanonicalPort<Object = Employment>`, so this bound stops holding the moment
/// the adapter is retargeted at a different object.
fn assert_implements_employment_port<P: EmploymentPort>() {}

/// `console_platform_test_support::runtime_role_pool`, inlined: adding that
/// crate as a dev-dependency rewrites this package's entry in
/// `backend/Cargo.lock` beyond the two names this lane structurally needs.
async fn runtime_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_rt").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .expect("connect console_rt-role test pool")
}

/// Seeded as the migration owner, before any role switch.
async fn seed_org_and_super_admin(owner_pool: &PgPool, org: Uuid, tag: &str) -> UserId {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{tag}"))
    .bind(format!("Org {tag}"))
    .execute(owner_pool)
    .await
    .unwrap();
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*user_id.as_uuid())
        .bind(format!("Admin {tag}"))
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(org)
        .execute(owner_pool)
        .await
        .unwrap();
    user_id
}

async fn seed_employee(owner_pool: &PgPool, org: Uuid, source_key: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO employees \
         (org_id, company, name, source_filename, source_sheet, source_row, source_key) \
         VALUES ($1, 'ACME', $2, 'seed.xlsx', 'Sheet1', 1, $2) RETURNING id",
    )
    .bind(org)
    .bind(source_key)
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

/// The tenant, its actor, seeded OrgUnit/JobPosition rows, and the port built
/// on a `console_rt` pool.
async fn fixture(owner_pool: &PgPool) -> (OrgId, UserId, PgEmploymentPort) {
    let actor = seed_org_and_super_admin(owner_pool, ORG, "employment").await;
    seed_org_structure(owner_pool, ORG).await;
    let runtime_pool = runtime_role_pool(owner_pool).await;
    let port = PgEmploymentPort::new(runtime_pool, tokio::runtime::Handle::current());
    (OrgId::from_uuid(ORG), actor, port)
}

async fn seed_org_structure(owner_pool: &PgPool, org: Uuid) {
    for unit in [ORG_UNIT_SALES, ORG_UNIT_TECH, ORG_UNIT_ADMIN] {
        sqlx::query("INSERT INTO org_units (org_id, id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(org)
            .bind(unit)
            .execute(owner_pool)
            .await
            .unwrap();
    }
    for (job, unit) in [
        (JOB_STAFF, ORG_UNIT_SALES),
        (JOB_LEAD, ORG_UNIT_SALES),
        (JOB_EXEC, ORG_UNIT_SALES),
    ] {
        sqlx::query(
            "INSERT INTO job_positions (org_id, id, org_unit_id) VALUES ($1, $2, $3) \
             ON CONFLICT DO NOTHING",
        )
        .bind(org)
        .bind(job)
        .bind(unit)
        .execute(owner_pool)
        .await
        .unwrap();
    }
}

/// Drive the SYNCHRONOUS `execute` off the runtime's worker thread.
async fn execute(
    port: &PgEmploymentPort,
    command: EmploymentCommand,
) -> Result<CommandReceipt, EmploymentError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.execute(&command))
        .await
        .unwrap()
}

async fn get(
    port: &PgEmploymentPort,
    org: OrgId,
    employment_id: Uuid,
) -> Result<Option<EmploymentHead>, EmploymentError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.get(org, employment_id))
        .await
        .unwrap()
}

async fn get_as_of(
    port: &PgEmploymentPort,
    org: OrgId,
    employment_id: Uuid,
    at: OffsetDateTime,
) -> Result<Option<EmploymentHead>, EmploymentError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.get_as_of(org, employment_id, at))
        .await
        .unwrap()
}

async fn list(port: &PgEmploymentPort, org: OrgId) -> Result<Vec<EmploymentHead>, EmploymentError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.list(org))
        .await
        .unwrap()
}

fn attributes(org_unit: Uuid, position: Uuid, status: &str) -> EmploymentAttributes {
    EmploymentAttributes {
        company: "ACME".to_owned(),
        org_unit_id: Some(org_unit),
        job_position_id: Some(position),
        employment_status: status.to_owned(),
    }
}

fn command(org: OrgId, actor: UserId, query: EmploymentQuery) -> EmploymentCommand {
    EmploymentCommand {
        org_id: org,
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        actor_id: actor,
        query,
        action_key: "revise".to_owned(),
        object_type_id: Uuid::nil(),
    }
}

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_736_928_000 + seconds).unwrap()
}

fn employment_of(receipt: &CommandReceipt) -> Uuid {
    receipt.result()["employment_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

const COUNT_EMPLOYEES: &str = "SELECT count(*)::bigint FROM employees";
const COUNT_HEADS: &str = "SELECT count(*)::bigint FROM employment_heads";
const COUNT_REVISIONS: &str = "SELECT count(*)::bigint FROM employment_revisions";
const COUNT_BINDINGS: &str = "SELECT count(*)::bigint FROM employment_source_bindings";
const COUNT_RECEIPTS: &str = "SELECT count(*)::bigint FROM ont_action_command_receipts";

/// The four tables the contract assigns to `Employment`, each with the count
/// over it. `sqlx` 0.9 accepts only `&'static str` SQL, so the table name cannot
/// be interpolated and every statement here is a literal.
const OWNED_TABLES: [(&str, &str); 4] = [
    ("employees", COUNT_EMPLOYEES),
    ("employment_heads", COUNT_HEADS),
    ("employment_revisions", COUNT_REVISIONS),
    ("employment_source_bindings", COUNT_BINDINGS),
];

async fn count_rows(owner_pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(owner_pool).await.unwrap()
}

/// The SQLSTATE and the message PostgreSQL actually returned, so a test quotes
/// the real error rather than a paraphrase of it.
fn database_error(error: &EmploymentError) -> (String, String) {
    let EmploymentError::Database(sqlx_error) = error else {
        panic!("expected a database error, got {error:?}");
    };
    let database = sqlx_error
        .as_database_error()
        .unwrap_or_else(|| panic!("expected a database error, got {sqlx_error:?}"));
    (
        database
            .code()
            .map(|code| code.into_owned())
            .unwrap_or_default(),
        database.message().to_owned(),
    )
}

/// The whole revision row as JSONB, so "unchanged" means the whole row and not
/// the two columns a test remembered to name.
async fn revision_snapshot(owner_pool: &PgPool, employment: Uuid, version: i64) -> String {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT to_jsonb(r) FROM employment_revisions r \
         WHERE org_id = $1 AND employment_id = $2 AND version = $3",
    )
    .bind(ORG)
    .bind(employment)
    .bind(version)
    .fetch_one(owner_pool)
    .await
    .unwrap()
    .to_string()
}

/// The legacy head's four canonical columns.
async fn legacy_head(
    owner_pool: &PgPool,
    employee: Uuid,
) -> (String, Option<String>, Option<String>, String) {
    let row = sqlx::query(
        "SELECT company, org_unit, position, employment_status FROM employees \
         WHERE org_id = $1 AND id = $2",
    )
    .bind(ORG)
    .bind(employee)
    .fetch_one(owner_pool)
    .await
    .unwrap();
    (
        row.get("company"),
        row.get("org_unit"),
        row.get("position"),
        row.get("employment_status"),
    )
}

/// One appointed employment: the seeded legacy row, the head it opened.
async fn appointed(
    owner_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    port: &PgEmploymentPort,
    tag: &str,
) -> (Uuid, Uuid) {
    let employee = seed_employee(owner_pool, ORG, tag).await;
    let receipt = execute(
        port,
        command(
            org,
            actor,
            EmploymentQuery::Appoint {
                employee_id: employee,
                valid_from: at(0),
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap();
    (employee, employment_of(&receipt))
}

/// The stated invariant is that `valid_to` is the ONE mutable column on a head.
///
/// Nothing enforced it: `GRANT ... UPDATE, DELETE ON employment_heads TO
/// console_rt` lets an org-armed session rewrite `valid_from` or `created_at`,
/// and such an update preserves `org_id` so the RLS policy passes untouched. A
/// stated invariant the grant does not restrict is a claim about a control, not
/// a control. Both directions are asserted, because a trigger that refuses
/// everything would satisfy the first half and break the column the design
/// exists to allow.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn only_valid_to_may_move_on_an_employment_head(owner_pool: PgPool) {
    let (org, _actor, _port) = fixture(&owner_pool).await;

    let head: Uuid = sqlx::query_scalar(
        "INSERT INTO employment_heads (org_id, valid_from) VALUES ($1, now()) RETURNING id",
    )
    .bind(org.as_uuid())
    .fetch_one(&owner_pool)
    .await
    .expect("seed a head");

    // PERMITTED: the one column the design names.
    sqlx::query("UPDATE employment_heads SET valid_to = now() WHERE id = $1")
        .bind(head)
        .execute(&owner_pool)
        .await
        .expect("valid_to is the legitimate mutation and must still work");

    // REFUSED: anything else, named in the error rather than a bare refusal.
    let refused = sqlx::query("UPDATE employment_heads SET valid_from = now() WHERE id = $1")
        .bind(head)
        .execute(&owner_pool)
        .await
        .expect_err("valid_from must not move on a head");
    let text = refused.to_string();
    assert!(
        text.contains("only valid_to may change") && text.contains("valid_from"),
        "the refusal must name the column that moved: {text}"
    );
}

#[test]
fn the_contract_identity_is_copied_verbatim_and_the_port_is_the_named_one() {
    assert_implements_employment_port::<PgEmploymentPort>();
    assert_eq!(ObjectKey::Employment.as_str(), "employment");
    assert_eq!(
        ObjectKey::Employment.owned_tables(),
        [
            "employees",
            "employment_heads",
            "employment_revisions",
            "employment_source_bindings"
        ]
    );
    assert_eq!(
        ObjectKey::Employment.owner_crate(),
        "console-ontology-canonical-adapter-postgres",
        "the canonical adapter is the owner (retargeted by console-1qw.4); \
         ReassignOrgUnit reaches it through the injected Employment-transfer port"
    );
    for (target, wire) in [
        (DispatchTarget::HrAppoint, "hr.appoint"),
        (DispatchTarget::HrPromote, "hr.promote"),
        (DispatchTarget::HrTransfer, "hr.transfer"),
    ] {
        assert_eq!(target.as_str(), wire);
        assert_eq!(target.object(), ObjectKey::Employment);
    }
}

#[test]
fn preflight_is_pure_and_blocks_what_the_database_would_only_refuse_at_23514() {
    let blank = EmploymentQuery::Appoint {
        employee_id: Uuid::new_v4(),
        valid_from: at(0),
        attributes: EmploymentAttributes {
            company: "   ".to_owned(),
            org_unit_id: None,
            job_position_id: None,
            employment_status: "PROBATION".to_owned(),
        },
    };
    let preflight = <PgEmploymentPort as CanonicalPort>::preflight(&blank);
    assert!(!preflight.is_ok());
    assert_eq!(
        preflight.blockers(),
        [
            "company must not be blank".to_owned(),
            "employment_status must be one of [\"ACTIVE\", \"EXITED\", \"UNKNOWN\"]".to_owned(),
        ]
    );

    let nil_head = <PgEmploymentPort as CanonicalPort>::preflight(&EmploymentQuery::Promote {
        employment_id: Uuid::nil(),
        valid_from: at(0),
        attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
    });
    assert!(!nil_head.is_ok());
    assert_eq!(
        nil_head.blockers(),
        ["employment_id must not be nil".to_owned()]
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_appointment_opens_a_head_binds_the_legacy_row_and_appends_revision_one(
    owner_pool: PgPool,
) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let employee = seed_employee(&owner_pool, ORG, "appoint-1").await;
    let query = EmploymentQuery::Appoint {
        employee_id: employee,
        valid_from: at(0),
        attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
    };
    assert!(<PgEmploymentPort as CanonicalPort>::preflight(&query).is_ok());

    let receipt = execute(&port, command(org, actor, query)).await.unwrap();

    assert_eq!(receipt.org_id(), org);
    assert_eq!(receipt.actor_id(), actor);
    assert_eq!(
        receipt.owner(),
        ReceiptOwner::Canonical(ObjectKey::Employment),
        "the receipt must be owned by the canonical Employment object"
    );
    assert_eq!(receipt.target(), DispatchTarget::HrAppoint);
    assert_eq!(receipt.result()["version"].as_i64(), Some(1));

    let employment_id = employment_of(&receipt);
    let row = sqlx::query(
        "SELECT h.valid_from, h.valid_to, r.version, r.attributes, r.valid_from AS revision_from, \
                r.command_id, r.payload_digest, b.employee_id \
         FROM employment_heads h \
         JOIN employment_revisions r ON r.org_id = h.org_id AND r.employment_id = h.id \
         JOIN employment_source_bindings b ON b.org_id = h.org_id AND b.employment_id = h.id \
         WHERE h.org_id = $1 AND h.id = $2",
    )
    .bind(ORG)
    .bind(employment_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();

    assert_eq!(row.get::<i64, _>("version"), 1);
    assert_eq!(row.get::<OffsetDateTime, _>("valid_from"), at(0));
    assert_eq!(row.get::<Option<OffsetDateTime>, _>("valid_to"), None);
    assert_eq!(row.get::<OffsetDateTime, _>("revision_from"), at(0));
    assert_eq!(row.get::<Uuid, _>("employee_id"), employee);
    assert_eq!(
        row.get::<serde_json::Value, _>("attributes"),
        serde_json::json!({
            "company": "ACME",
            "employment_status": "ACTIVE",
            "job_position_id": JOB_STAFF.to_string(),
            "org_unit_id": ORG_UNIT_SALES.to_string(),
        })
    );
    assert_eq!(
        row.get::<Uuid, _>("command_id"),
        *receipt.command_id().as_uuid()
    );
    assert_eq!(
        row.get::<Vec<u8>, _>("payload_digest"),
        receipt.payload_digest().to_vec(),
        "the stored digest is the 32 bytes the receipt carries"
    );

    let head = get(&port, org, employment_id)
        .await
        .unwrap()
        .expect("created Employment must be queryable");
    assert_eq!(head.id, employment_id);
    assert_eq!(
        head.person_id, None,
        "person_id must come from employee_person_bindings, never invented from employee_id"
    );
    assert_eq!(head.org_unit_id, Some(ORG_UNIT_SALES));
    assert_eq!(head.job_position_id, Some(JOB_STAFF));
    assert_eq!(head.appointed_on, at(0));
    assert_eq!(list(&port, org).await.unwrap(), vec![head]);
    let unknown = get(&port, org, Uuid::new_v4()).await;
    assert!(
        matches!(unknown, Ok(None)),
        "unknown Employment id is Ok(None) on the runtime-role pool, never a distinct error; got {unknown:?}"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_promotion_carries_the_new_state_onto_the_legacy_employees_head(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (employee, employment_id) = appointed(&owner_pool, org, actor, &port, "promote-1").await;

    // The appointment BINDS the legacy row and does not rewrite it, so the
    // assertion below is not vacuous: these are the seeded values.
    assert_eq!(
        legacy_head(&owner_pool, employee).await,
        ("ACME".to_owned(), None, None, "ACTIVE".to_owned())
    );

    let promoted = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id,
                valid_from: at(86_400),
                attributes: attributes(ORG_UNIT_SALES, JOB_LEAD, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(promoted.target(), DispatchTarget::HrPromote);
    assert_eq!(promoted.result()["version"].as_i64(), Some(2));

    assert_eq!(
        legacy_head(&owner_pool, employee).await,
        (
            "ACME".to_owned(),
            Some(ORG_UNIT_SALES.to_string()),
            Some(JOB_LEAD.to_string()),
            "ACTIVE".to_owned()
        ),
        "the promotion must reach `employees`, the legacy compatibility head"
    );

    let head = get(&port, org, employment_id)
        .await
        .unwrap()
        .expect("promoted Employment must be queryable");
    assert_eq!(head.job_position_id, Some(JOB_LEAD));
    assert_eq!(head.org_unit_id, Some(ORG_UNIT_SALES));
    assert_eq!(
        head.appointed_on,
        at(0),
        "appointed_on is the head opening, not the promote revision's valid_from"
    );
}

/// An EXITED promote must close `employment_heads.valid_to` at the exit
/// revision's `valid_from`. Legacy `employees.exit_date` alone is not enough:
/// a future temporal reader over heads would still count the person as open.
/// Mutating the close away (drop the UPDATE) must turn this red.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_exited_promote_closes_the_employment_head_window(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (employee, employment_id) = appointed(&owner_pool, org, actor, &port, "exit-1").await;

    let open: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT valid_to FROM employment_heads WHERE org_id = $1 AND id = $2")
            .bind(ORG)
            .bind(employment_id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(open, None, "a newly appointed head must still be open");
    assert!(
        get(&port, org, employment_id).await.unwrap().is_some(),
        "an open appointed head must be queryable before EXITED, or the omit below is vacuous"
    );

    let exit_at = at(86_400);
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id,
                valid_from: exit_at,
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "EXITED"),
            },
        ),
    )
    .await
    .unwrap();

    let closed: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT valid_to FROM employment_heads WHERE org_id = $1 AND id = $2")
            .bind(ORG)
            .bind(employment_id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(
        closed,
        Some(exit_at),
        "EXITED must set employment_heads.valid_to to the exit revision's valid_from"
    );
    assert!(
        get(&port, org, employment_id).await.unwrap().is_none(),
        "a closed head is not a current head — list/get must not surface EXITED assignments"
    );
    assert!(
        list(&port, org).await.unwrap().is_empty(),
        "list must omit the closed head, not return it with EXITED revision pointers"
    );

    let (status, exit_date): (String, Option<String>) = {
        let row = sqlx::query(
            "SELECT employment_status, exit_date FROM employees WHERE org_id = $1 AND id = $2",
        )
        .bind(ORG)
        .bind(employee)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        (row.get("employment_status"), row.get("exit_date"))
    };
    assert_eq!(status, "EXITED");
    let expected_exit_date = exit_at.date().to_string();
    assert_eq!(
        exit_date.as_deref(),
        Some(expected_exit_date.as_str()),
        "legacy exit_date stays the date form of the same effective instant"
    );

    // Control: a non-EXITED revise must leave the window open. Seed a second
    // employment so the assertion is about status, not about promote-in-general.
    let (_employee2, employment_open) =
        appointed(&owner_pool, org, actor, &port, "exit-still-open").await;
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id: employment_open,
                valid_from: at(86_400),
                attributes: attributes(ORG_UNIT_SALES, JOB_LEAD, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap();
    let still_open: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT valid_to FROM employment_heads WHERE org_id = $1 AND id = $2")
            .bind(ORG)
            .bind(employment_open)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(
        still_open, None,
        "ACTIVE promote must not close employment_heads.valid_to"
    );
    assert!(
        get(&port, org, employment_id).await.unwrap().is_none(),
        "the EXITED head stays omitted after a sibling ACTIVE promote"
    );
    let sibling = get(&port, org, employment_open)
        .await
        .unwrap()
        .expect("sibling ACTIVE employment remains queryable after EXITED omit");
    assert_eq!(
        list(&port, org).await.unwrap(),
        vec![sibling],
        "list is open heads only"
    );
}

/// A trusted uniquely-resolved person bind is the employment head's person_id.
/// Unbound appoint (the other list/get test) leaves person_id None; this is
/// the Some(_) arm, via PersonQuery::Create { employee_id: Some(...) }.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_bound_person_is_the_employment_head_person_id(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let person_port = PgPersonPort::new(
        runtime_role_pool(&owner_pool).await,
        tokio::runtime::Handle::current(),
    );
    let employee = seed_employee(&owner_pool, ORG, "bound-person-1").await;

    let person_command = PersonCommand {
        org_id: org,
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        actor_id: actor,
        query: PersonQuery::Create {
            employee_id: Some(employee),
            attributes: serde_json::json!({ "legal_name": "김바인드" }),
        },
        action_key: "revise".to_owned(),
        object_type_id: Uuid::nil(),
    };
    let person_receipt = {
        let person_port = person_port.clone();
        tokio::task::spawn_blocking(move || person_port.execute(&person_command))
            .await
            .unwrap()
            .unwrap()
    };
    let person_id: Uuid = person_receipt.result()["person_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(person_id, employee);

    let appointed = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Appoint {
                employee_id: employee,
                valid_from: at(0),
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap();
    let employment_id = employment_of(&appointed);
    let head = get(&port, org, employment_id)
        .await
        .unwrap()
        .expect("open bound employment must be queryable");
    assert_eq!(
        head.person_id,
        Some(person_id),
        "person_id is the bound natural person, not an invented employee_id alias"
    );
    assert_eq!(list(&port, org).await.unwrap(), vec![head]);
}

/// Reactivation (EXITED → ACTIVE|UNKNOWN) must clear legacy `exit_date`.
/// Preflight already accepts the status change; preserving the termination date
/// leaves reads/exports disagreeing with status (console-90h / #692). Mutating
/// `apply_employment_change` back to `ELSE exit_date` must turn this red.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn reactivating_an_exited_employee_clears_legacy_exit_date(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (employee, employment_id) = appointed(&owner_pool, org, actor, &port, "reactivate-1").await;

    let exit_at = at(86_400);
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id,
                valid_from: exit_at,
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "EXITED"),
            },
        ),
    )
    .await
    .unwrap();

    let exited_date: Option<String> =
        sqlx::query_scalar("SELECT exit_date FROM employees WHERE org_id = $1 AND id = $2")
            .bind(ORG)
            .bind(employee)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    let expected_exit_date = exit_at.date().to_string();
    assert_eq!(
        exited_date.as_deref(),
        Some(expected_exit_date.as_str()),
        "control: EXITED promote must stamp exit_date before reactivation"
    );

    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id,
                valid_from: at(172_800),
                attributes: attributes(ORG_UNIT_SALES, JOB_LEAD, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap();

    let (status, exit_date): (String, Option<String>) = {
        let row = sqlx::query(
            "SELECT employment_status, exit_date FROM employees WHERE org_id = $1 AND id = $2",
        )
        .bind(ORG)
        .bind(employee)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        (row.get("employment_status"), row.get("exit_date"))
    };
    assert_eq!(status, "ACTIVE");
    assert_eq!(
        exit_date, None,
        "ACTIVE reactivation must clear exit_date so status and exit date agree"
    );

    // Transfer path shares apply_employment_change; UNKNOWN is the other
    // non-EXITED CHECK value. Seed a second exit then transfer to UNKNOWN.
    let (employee2, employment2) =
        appointed(&owner_pool, org, actor, &port, "reactivate-unknown").await;
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id: employment2,
                valid_from: at(86_400),
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "EXITED"),
            },
        ),
    )
    .await
    .unwrap();
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Transfer {
                employment_id: employment2,
                valid_from: at(259_200),
                attributes: attributes(ORG_UNIT_TECH, JOB_STAFF, "UNKNOWN"),
            },
        ),
    )
    .await
    .unwrap();
    let (status2, exit_date2): (String, Option<String>) = {
        let row = sqlx::query(
            "SELECT employment_status, exit_date FROM employees WHERE org_id = $1 AND id = $2",
        )
        .bind(ORG)
        .bind(employee2)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        (row.get("employment_status"), row.get("exit_date"))
    };
    assert_eq!(status2, "UNKNOWN");
    assert_eq!(
        exit_date2, None,
        "UNKNOWN transfer reactivation must clear exit_date"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_transfer_appends_a_third_revision_and_the_intervals_stay_half_open(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (employee, employment_id) = appointed(&owner_pool, org, actor, &port, "transfer-1").await;

    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id,
                valid_from: at(86_400),
                attributes: attributes(ORG_UNIT_SALES, JOB_LEAD, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap();

    let transferred = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Transfer {
                employment_id,
                valid_from: at(172_800),
                attributes: attributes(ORG_UNIT_TECH, JOB_LEAD, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        transferred.target(),
        DispatchTarget::HrTransfer,
        "the target is what distinguishes a transfer from a promotion in the receipt"
    );
    assert_eq!(transferred.result()["version"].as_i64(), Some(3));

    assert_eq!(
        legacy_head(&owner_pool, employee).await.1,
        Some(ORG_UNIT_TECH.to_string())
    );

    // 0214 stores no per-revision `valid_to`: an interval ends where the next
    // one begins. Derive them and assert the window is a partition, not a set
    // that may overlap.
    let windows: Vec<(i64, OffsetDateTime, Option<OffsetDateTime>)> = sqlx::query(
        "SELECT version, valid_from, lead(valid_from) OVER (ORDER BY valid_from) AS derived_to \
         FROM employment_revisions WHERE org_id = $1 AND employment_id = $2 ORDER BY version",
    )
    .bind(ORG)
    .bind(employment_id)
    .fetch_all(&owner_pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| {
        (
            row.get::<i64, _>("version"),
            row.get::<OffsetDateTime, _>("valid_from"),
            row.get::<Option<OffsetDateTime>, _>("derived_to"),
        )
    })
    .collect();

    assert_eq!(
        windows,
        vec![
            (1, at(0), Some(at(86_400))),
            (2, at(86_400), Some(at(172_800))),
            (3, at(172_800), None),
        ]
    );

    // A second revision at an instant already taken is refused structurally.
    let clash = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Transfer {
                employment_id,
                valid_from: at(172_800),
                attributes: attributes(ORG_UNIT_ADMIN, JOB_LEAD, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap_err();
    let (code, message) = database_error(&clash);
    assert_eq!(code, "23505", "got {message}");
    assert!(
        message.contains("employment_revisions_org_id_employment_id_valid_from_key"),
        "non-overlap is structural — UNIQUE (org_id, employment_id, valid_from) — got {message}"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_revision_is_append_only_and_the_trigger_is_what_refuses_the_update(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (_employee, employment_id) = appointed(&owner_pool, org, actor, &port, "immutable-1").await;
    let before = revision_snapshot(&owner_pool, employment_id, 1).await;

    // Reproduce the DEPLOYED ACL before asserting anything about it. See the
    // module doc: production applies migrations as `console_app`, after
    // `ALTER DEFAULT PRIVILEGES … GRANT … UPDATE … TO console_rt`, so the
    // runtime role holds UPDATE there.
    sqlx::query("GRANT UPDATE ON employment_revisions TO console_rt")
        .execute(&owner_pool)
        .await
        .unwrap();
    let runtime_holds_update: bool = sqlx::query_scalar(
        "SELECT has_table_privilege('console_rt', 'employment_revisions', 'UPDATE')",
    )
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert!(
        runtime_holds_update,
        "the production ACL must be in place, or the assertion below observes a privilege error \
         that does not exist where this code ships"
    );

    let runtime_pool = runtime_role_pool(&owner_pool).await;
    let mut tx = runtime_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let as_runtime_role = sqlx::query(
        "UPDATE employment_revisions SET attributes = '{\"company\":\"forged\"}'::jsonb \
         WHERE org_id = $1 AND employment_id = $2",
    )
    .bind(ORG)
    .bind(employment_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    let runtime_error = as_runtime_role.as_database_error().unwrap();
    assert_eq!(
        runtime_error.code().unwrap(),
        "P0001",
        "the trigger is the enforcement, not the grant; got {}",
        runtime_error.message()
    );
    assert_eq!(
        runtime_error.message(),
        "canonical employment table employment_revisions: UPDATE is refused, the row is immutable"
    );
    drop(tx);

    // The owner is refused by the same trigger; ownership buys no exemption.
    // DELETE too — the revisions are the history the derived intervals are read
    // out of, so a removed row silently rewrites its predecessor's window.
    let as_owner =
        sqlx::query("UPDATE employment_revisions SET version = version + 100 WHERE org_id = $1")
            .bind(ORG)
            .execute(&owner_pool)
            .await
            .unwrap_err();
    assert_eq!(
        as_owner.as_database_error().unwrap().message(),
        "canonical employment table employment_revisions: UPDATE is refused, the row is immutable"
    );
    let deleted = sqlx::query("DELETE FROM employment_revisions WHERE org_id = $1")
        .bind(ORG)
        .execute(&owner_pool)
        .await
        .unwrap_err();
    assert_eq!(
        deleted.as_database_error().unwrap().message(),
        "canonical employment table employment_revisions: DELETE is refused, the row is immutable"
    );

    let after = revision_snapshot(&owner_pool, employment_id, 1).await;
    assert_eq!(before, after, "a refused write rewrote the revision row");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_foreign_tenant_is_invisible_and_unwritable_to_the_runtime_role(owner_pool: PgPool) {
    let (org, _actor, port) = fixture(&owner_pool).await;
    let foreign_actor = seed_org_and_super_admin(&owner_pool, FOREIGN_ORG, "foreign").await;
    let foreign_employee = seed_employee(&owner_pool, FOREIGN_ORG, "foreign-1").await;

    // A head, a revision and a binding that genuinely exist — under the OTHER
    // tenant. Seeded through the BYPASSRLS owner pool, which is the only way to
    // put rows on the far side of the boundary being tested.
    let foreign_employment: Uuid = sqlx::query_scalar(
        "INSERT INTO employment_heads (org_id, valid_from) VALUES ($1, now()) RETURNING id",
    )
    .bind(FOREIGN_ORG)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO employment_revisions \
         (org_id, employment_id, version, command_id, actor_id, payload_digest, valid_from, \
          attributes, receipt) \
         VALUES ($1, $2, 1, gen_random_uuid(), $3, $4, now(), '{}'::jsonb, '{}'::jsonb)",
    )
    .bind(FOREIGN_ORG)
    .bind(foreign_employment)
    .bind(*foreign_actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO employment_source_bindings \
         (org_id, employee_id, employment_id, actor_id, payload_digest) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(FOREIGN_ORG)
    .bind(foreign_employee)
    .bind(foreign_employment)
    .bind(*foreign_actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .execute(&owner_pool)
    .await
    .unwrap();

    // The rows are really there — otherwise "sees zero" below would be the
    // vacuous kind of zero, an empty table rather than a policy doing work.
    for (table, count) in OWNED_TABLES {
        assert_eq!(
            count_rows(&owner_pool, count).await,
            1,
            "{table} must hold exactly the foreign tenant's row before the boundary is tested"
        );
    }

    // A `console_rt` session armed for ORG, which owns none of them.
    let runtime_pool = runtime_role_pool(&owner_pool).await;
    let mut tx = runtime_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    for (table, count) in OWNED_TABLES {
        let visible: i64 = sqlx::query_scalar(count).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(
            visible, 0,
            "org_isolation must hide {table} rows belonging to another tenant"
        );
    }

    // And the write half: WITH CHECK refuses a row planted in the other tenant.
    let refused =
        sqlx::query("INSERT INTO employment_heads (org_id, valid_from) VALUES ($1, now())")
            .bind(FOREIGN_ORG)
            .execute(&mut *tx)
            .await
            .unwrap_err();
    let error = refused.as_database_error().unwrap();
    assert_eq!(error.code().unwrap(), "42501", "got {}", error.message());
    assert_eq!(
        error.message(),
        "new row violates row-level security policy for table \"employment_heads\""
    );
    drop(tx);

    assert!(list(&port, org).await.unwrap().is_empty());
    let foreign_head = get(&port, org, foreign_employment).await;
    assert!(
        matches!(foreign_head, Ok(None)),
        "foreign Employment id is Ok(None) on the runtime-role pool, never a wrong-tenant error; got {foreign_head:?}"
    );
    let foreign_as_of = get_as_of(&port, org, foreign_employment, OffsetDateTime::now_utc()).await;
    assert!(
        matches!(foreign_as_of, Ok(None)),
        "as-of read must keep the same tenant boundary; got {foreign_as_of:?}"
    );
    let unknown = get(&port, org, Uuid::new_v4()).await;
    assert!(
        matches!(unknown, Ok(None)),
        "unknown Employment id is indistinguishable from a foreign tenant: Ok(None); got {unknown:?}"
    );
}

/// Operator data-repair can leave one `employment_id` on N bindings (PK is
/// `(org_id, employee_id)`). Promote must refuse that ambiguity rather than
/// let `fetch_one` silently pick an arbitrary legacy row.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_ambiguous_employment_binding_refuses_promote(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (employee_a, employment_id) = appointed(&owner_pool, org, actor, &port, "ambig-a").await;
    let employee_b = seed_employee(&owner_pool, ORG, "ambig-b").await;

    sqlx::query(
        "INSERT INTO employment_source_bindings \
         (org_id, employee_id, employment_id, actor_id, payload_digest) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(ORG)
    .bind(employee_b)
    .bind(employment_id)
    .bind(*actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .execute(&owner_pool)
    .await
    .unwrap();

    let refused = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id,
                valid_from: at(86_400),
                attributes: attributes(ORG_UNIT_SALES, JOB_LEAD, "ACTIVE"),
            },
        ),
    )
    .await
    .expect_err("ambiguous source binding must refuse promote, not pick an arbitrary employee");

    assert!(
        matches!(
            refused,
            EmploymentError::AmbiguousSourceBinding {
                employment_id: id,
                binding_count: 2,
            } if id == employment_id
        ),
        "got {refused:?}"
    );

    // Neither legacy head was rewritten; no second revision landed.
    assert_eq!(
        legacy_head(&owner_pool, employee_a).await,
        ("ACME".to_owned(), None, None, "ACTIVE".to_owned())
    );
    assert_eq!(
        legacy_head(&owner_pool, employee_b).await,
        ("ACME".to_owned(), None, None, "ACTIVE".to_owned())
    );
    assert_eq!(
        count_rows(&owner_pool, COUNT_REVISIONS).await,
        1,
        "a refused promote must append no revision"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_second_binding_for_the_same_employee_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (employee, employment_id) = appointed(&owner_pool, org, actor, &port, "rebind-1").await;

    let refused = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Appoint {
                employee_id: employee,
                valid_from: at(1),
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap_err();
    let (code, message) = database_error(&refused);
    assert_eq!(code, "23505", "got {message}");
    assert!(
        message.contains("employment_source_bindings_pkey"),
        "the primary key (org_id, employee_id) must be what refuses it; got {message}"
    );

    // The refused command rolled back whole: one binding, no orphan head.
    let bound: Vec<Uuid> = sqlx::query_scalar(
        "SELECT employment_id FROM employment_source_bindings \
         WHERE org_id = $1 AND employee_id = $2",
    )
    .bind(ORG)
    .bind(employee)
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(bound, vec![employment_id]);
    assert_eq!(
        count_rows(&owner_pool, COUNT_HEADS).await,
        1,
        "the refused command must persist no head"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_actor_from_another_org_is_refused(owner_pool: PgPool) {
    let (org, _actor, port) = fixture(&owner_pool).await;
    let foreign_actor = seed_org_and_super_admin(&owner_pool, FOREIGN_ORG, "foreign").await;
    let employee = seed_employee(&owner_pool, ORG, "foreign-actor-1").await;

    let refused = execute(
        &port,
        command(
            org,
            foreign_actor,
            EmploymentQuery::Appoint {
                employee_id: employee,
                valid_from: at(0),
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap_err();
    let (code, message) = database_error(&refused);
    assert_eq!(code, "23503", "got {message}");
    assert!(
        message.contains("employment_revisions"),
        "the (actor_id, org_id) foreign key on employment_revisions must refuse it; got {message}"
    );
    assert_eq!(
        count_rows(&owner_pool, COUNT_HEADS).await,
        0,
        "a refused command must persist no head"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_repeat_of_the_same_command_replays_the_stored_receipt(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let employee = seed_employee(&owner_pool, ORG, "replay-1").await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());
    let query = EmploymentQuery::Appoint {
        employee_id: employee,
        valid_from: at(0),
        attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
    };

    let first = execute(
        &port,
        EmploymentCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: query.clone(),
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();

    let replayed = execute(
        &port,
        EmploymentCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query,
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        replayed, first,
        "a repeat of the same command id must replay the stored receipt verbatim"
    );
    assert_eq!(count_rows(&owner_pool, COUNT_HEADS).await, 1);
    assert_eq!(
        count_rows(&owner_pool, COUNT_REVISIONS).await,
        1,
        "a replayed command must append no revision"
    );
    assert_eq!(count_rows(&owner_pool, COUNT_BINDINGS).await, 1);
    assert_eq!(count_rows(&owner_pool, COUNT_RECEIPTS).await, 1);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_repeat_with_a_different_payload_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let employee = seed_employee(&owner_pool, ORG, "conflict-1").await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());

    execute(
        &port,
        EmploymentCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: EmploymentQuery::Appoint {
                employee_id: employee,
                valid_from: at(0),
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
            },
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();

    let refused = execute(
        &port,
        EmploymentCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: EmploymentQuery::Appoint {
                employee_id: employee,
                valid_from: at(0),
                attributes: attributes(ORG_UNIT_SALES, JOB_EXEC, "ACTIVE"),
            },
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap_err();

    assert!(
        matches!(refused, EmploymentError::DigestConflict(id) if id == *command_id.as_uuid()),
        "got {refused:?}"
    );
    assert_eq!(
        count_rows(&owner_pool, COUNT_REVISIONS).await,
        1,
        "a refused repeat must append no revision"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_blocked_preflight_never_reaches_the_database(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;

    let blocked = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id: Uuid::nil(),
                valid_from: at(0),
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap_err();
    let EmploymentError::Blocked(blockers) = &blocked else {
        panic!("expected a blocked preflight, got {blocked:?}");
    };
    assert_eq!(blockers, &["employment_id must not be nil".to_owned()]);
    assert_eq!(count_rows(&owner_pool, COUNT_REVISIONS).await, 0);
    assert_eq!(count_rows(&owner_pool, COUNT_RECEIPTS).await, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_stored_receipt_naming_no_dispatch_target_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let employee = seed_employee(&owner_pool, ORG, "unreadable-1").await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());
    let query = EmploymentQuery::Appoint {
        employee_id: employee,
        valid_from: at(0),
        attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
    };
    let accepted = execute(
        &port,
        EmploymentCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: query.clone(),
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();

    // Put back a row carrying the SAME digest — so the replay gets past the
    // digest comparison — but a receipt naming no dispatch target, which is the
    // shape an `ontology.action` row has. 0177's trigger refuses UPDATE and
    // DELETE per row and TRUNCATE is statement-level, so this is the only way a
    // test can stand a hostile row where a good one was.
    sqlx::query("TRUNCATE ont_action_command_receipts")
        .execute(&owner_pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO ont_action_command_receipts \
         (org_id, command_id, actor_id, payload_digest, receipt, created_at) \
         VALUES ($1, $2, $3, $4, $5, now())",
    )
    .bind(ORG)
    .bind(*command_id.as_uuid())
    .bind(*actor.as_uuid())
    .bind(accepted.payload_digest().as_slice())
    .bind(
        serde_json::json!({ "employment_id": employment_of(&accepted).to_string(), "version": 1 }),
    )
    .execute(&owner_pool)
    .await
    .unwrap();

    let refused = execute(
        &port,
        EmploymentCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query,
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(refused, EmploymentError::UnreadableReceipt(id, _) if id == *command_id.as_uuid()),
        "a receipt the roster cannot read must be refused, never replayed; got {refused:?}"
    );
}

#[test]
fn preflight_blocks_nil_canonical_ids() {
    let query = EmploymentQuery::Appoint {
        employee_id: Uuid::new_v4(),
        valid_from: at(0),
        attributes: EmploymentAttributes {
            company: "ACME".to_owned(),
            org_unit_id: Some(Uuid::nil()),
            job_position_id: Some(Uuid::nil()),
            employment_status: "ACTIVE".to_owned(),
        },
    };
    let preflight = <PgEmploymentPort as CanonicalPort>::preflight(&query);
    assert!(!preflight.is_ok());
    assert!(
        preflight
            .blockers()
            .iter()
            .any(|b| b.contains("org_unit_id must not be nil"))
    );
    assert!(
        preflight
            .blockers()
            .iter()
            .any(|b| b.contains("job_position_id must not be nil"))
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn free_text_org_unit_attrs_are_not_authority_unknown_uuid_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let employee = seed_employee(&owner_pool, ORG, "unknown-unit-1").await;
    let unknown = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0099);
    let refused = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Appoint {
                employee_id: employee,
                valid_from: at(0),
                attributes: EmploymentAttributes {
                    company: "ACME".to_owned(),
                    org_unit_id: Some(unknown),
                    job_position_id: Some(JOB_STAFF),
                    employment_status: "ACTIVE".to_owned(),
                },
            },
        ),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(refused, EmploymentError::UnknownOrgUnit(id) if id == unknown),
        "got {refused:?}"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn reassign_org_unit_emits_hr_transfer_not_a_raw_bulk_rewrite(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (employee, employment_id) = appointed(&owner_pool, org, actor, &port, "reassign-1").await;

    // Appointment binds without rewriting the legacy head. Stamp the UUID
    // org_unit onto employees so ReassignOrgUnit can match.
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id,
                valid_from: at(3_600),
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        legacy_head(&owner_pool, employee).await.1,
        Some(ORG_UNIT_SALES.to_string())
    );

    // RED control class: free-text team labels fail closed.
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let free_text = reassign_org_unit_via_transfers_in_tx(
        &mut tx,
        org,
        actor,
        Uuid::new_v4(),
        "영업본부",
        "기술본부",
        "ACME",
        at(86_400),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(free_text, EmploymentError::OrgUnitRefNotUuid),
        "got {free_text:?}"
    );
    drop(tx);

    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let moved = reassign_org_unit_via_transfers_in_tx(
        &mut tx,
        org,
        actor,
        Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_00aa),
        &ORG_UNIT_SALES.to_string(),
        &ORG_UNIT_TECH.to_string(),
        "ACME",
        at(86_400),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(moved, 1);

    assert_eq!(
        legacy_head(&owner_pool, employee).await.1,
        Some(ORG_UNIT_TECH.to_string())
    );

    let revision = sqlx::query(
        "SELECT version, attributes, receipt FROM employment_revisions \
         WHERE org_id = $1 AND employment_id = $2 ORDER BY version DESC LIMIT 1",
    )
    .bind(ORG)
    .bind(employment_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(revision.get::<i64, _>("version"), 3);
    assert_eq!(
        revision.get::<serde_json::Value, _>("attributes")["org_unit_id"],
        serde_json::json!(ORG_UNIT_TECH.to_string())
    );
    assert_eq!(
        revision.get::<serde_json::Value, _>("receipt")["target"],
        serde_json::json!("hr.transfer")
    );

    let _ = port;
}

/// EXITED employees in the source unit must not be transferred. Preflight
/// `scope_headcount` counts ACTIVE only; reassign must match that set so an
/// org-structure move cannot rewrite a closed `employment_heads.valid_to`.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn reassign_org_unit_skips_exited_employees_and_leaves_valid_to(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (active_employee, active_employment) =
        appointed(&owner_pool, org, actor, &port, "reassign-active").await;
    let (exited_employee, exited_employment) =
        appointed(&owner_pool, org, actor, &port, "reassign-exited").await;

    // Stamp UUID org_unit onto both legacy heads (appointment leaves org_unit null).
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id: active_employment,
                valid_from: at(3_600),
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap();
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id: exited_employment,
                valid_from: at(3_600),
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap();

    let exit_at = at(7_200);
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id: exited_employment,
                valid_from: exit_at,
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "EXITED"),
            },
        ),
    )
    .await
    .unwrap();

    let exited_valid_to_before: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT valid_to FROM employment_heads WHERE org_id = $1 AND id = $2")
            .bind(ORG)
            .bind(exited_employment)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(exited_valid_to_before, Some(exit_at));
    assert_eq!(
        legacy_head(&owner_pool, exited_employee).await.3,
        "EXITED".to_owned()
    );
    assert_eq!(
        legacy_head(&owner_pool, exited_employee).await.1,
        Some(ORG_UNIT_SALES.to_string())
    );

    let reassign_at = at(86_400);
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let moved = reassign_org_unit_via_transfers_in_tx(
        &mut tx,
        org,
        actor,
        Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_00ab),
        &ORG_UNIT_SALES.to_string(),
        &ORG_UNIT_TECH.to_string(),
        "ACME",
        reassign_at,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        moved, 1,
        "only the ACTIVE peer must move; EXITED must be excluded from the SELECT"
    );
    assert_eq!(
        legacy_head(&owner_pool, active_employee).await.1,
        Some(ORG_UNIT_TECH.to_string()),
        "ACTIVE peer must transfer to the destination OrgUnit"
    );
    assert_eq!(
        legacy_head(&owner_pool, exited_employee).await.1,
        Some(ORG_UNIT_SALES.to_string()),
        "EXITED employee must not be moved"
    );
    assert_eq!(
        legacy_head(&owner_pool, exited_employee).await.3,
        "EXITED".to_owned()
    );
    let exited_valid_to_after: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT valid_to FROM employment_heads WHERE org_id = $1 AND id = $2")
            .bind(ORG)
            .bind(exited_employment)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(
        exited_valid_to_after, exited_valid_to_before,
        "EXITED employment_heads.valid_to must stay at the original exit instant, not the reassignment timestamp"
    );

    let _ = port;
}

/// A syntactically valid but nonexistent source OrgUnit must fail closed as
/// `UnknownOrgUnit`, not succeed with `moved=0` (silent apply audit).
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn reassign_org_unit_unknown_from_org_unit_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let unknown_from = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0098);

    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let refused = reassign_org_unit_via_transfers_in_tx(
        &mut tx,
        org,
        actor,
        Uuid::new_v4(),
        &unknown_from.to_string(),
        &ORG_UNIT_TECH.to_string(),
        "ACME",
        at(86_400),
    )
    .await
    .unwrap_err();
    drop(tx);

    assert!(
        matches!(refused, EmploymentError::UnknownOrgUnit(id) if id == unknown_from),
        "unknown source OrgUnit must be UnknownOrgUnit, not Ok(0); got {refused:?}"
    );

    let _ = port;
}

/// Seeded through the BYPASSRLS owner pool: an active (still-locked) payroll or
/// accounting window covering exactly `day`, which `assert_period_open` must
/// refuse for that date.
async fn seed_period_lock(owner_pool: &PgPool, org: Uuid, domain: &str, day: Date) {
    sqlx::query(
        "INSERT INTO period_locks (org_id, domain, period_start, period_end, reason) \
         VALUES ($1, $2, $3, $4, 'test freeze')",
    )
    .bind(org)
    .bind(domain)
    .bind(day)
    .bind(day)
    .execute(owner_pool)
    .await
    .unwrap();
}

/// §3.9.1 (console-rte): the port's promote path must refuse a write whose
/// effective date falls inside a locked payroll period. Before the freeze gate
/// this command was accepted and rewrote `employees` for a sealed period.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_promote_effective_inside_a_locked_payroll_period_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (_employee, employment_id) =
        appointed(&owner_pool, org, actor, &port, "freeze-promote").await;

    let promote_at = at(86_400);
    seed_period_lock(&owner_pool, ORG, "payroll", promote_at.date()).await;

    let refused = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id,
                valid_from: promote_at,
                attributes: attributes(ORG_UNIT_SALES, JOB_LEAD, "ACTIVE"),
            },
        ),
    )
    .await
    .expect_err("a promote whose effective date falls in a locked payroll period must be refused");

    let EmploymentError::Frozen(error) = &refused else {
        panic!("expected a freeze refusal, got {refused:?}");
    };
    assert_eq!(
        error.kind,
        ErrorKind::Conflict,
        "a closed window must surface as a conflict"
    );
    assert!(
        error.message.contains("locked"),
        "the refusal must name the lock: {}",
        error.message
    );

    assert_eq!(
        count_rows(&owner_pool, COUNT_REVISIONS).await,
        1,
        "a refused promote must append no revision"
    );
}

/// The transfer path shares `apply_employment_change`; it must refuse under the
/// OTHER freeze domain (accounting), proving the gate checks both domains.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_transfer_effective_inside_a_locked_accounting_period_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (_employee, employment_id) =
        appointed(&owner_pool, org, actor, &port, "freeze-transfer").await;

    let transfer_at = at(86_400);
    seed_period_lock(&owner_pool, ORG, "accounting", transfer_at.date()).await;

    let refused = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Transfer {
                employment_id,
                valid_from: transfer_at,
                attributes: attributes(ORG_UNIT_TECH, JOB_STAFF, "ACTIVE"),
            },
        ),
    )
    .await
    .expect_err(
        "a transfer whose effective date falls in a locked accounting period must be refused",
    );

    assert!(
        matches!(&refused, EmploymentError::Frozen(error) if error.kind == ErrorKind::Conflict),
        "got {refused:?}"
    );
    assert_eq!(count_rows(&owner_pool, COUNT_REVISIONS).await, 1);
}

/// console-r25: a backdated correction is a legitimate history insert on the
/// CANONICAL side (0214 appends MAX(version)+1 and derives intervals by
/// `valid_from` order), so the backdated revision must land as history while the
/// LEGACY `employees` head — a projection of the LATEST effective state — must
/// NOT move backward to the pre-transfer company.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_backdated_promote_is_history_not_a_head_rewrite(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (employee, employment_id) = appointed(&owner_pool, org, actor, &port, "backdate-1").await;

    // Transfer to company B at the LATER effective instant (t2).
    let transfer_at = at(172_800);
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Transfer {
                employment_id,
                valid_from: transfer_at,
                attributes: EmploymentAttributes {
                    company: "ACME-B".to_owned(),
                    org_unit_id: Some(ORG_UNIT_TECH),
                    job_position_id: Some(JOB_LEAD),
                    employment_status: "ACTIVE".to_owned(),
                },
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        legacy_head(&owner_pool, employee).await,
        (
            "ACME-B".to_owned(),
            Some(ORG_UNIT_TECH.to_string()),
            Some(JOB_LEAD.to_string()),
            "ACTIVE".to_owned()
        )
    );

    // Backdated correction to company A at t1 (between appoint t0 and transfer t2).
    let backdated_at = at(86_400);
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id,
                valid_from: backdated_at,
                attributes: EmploymentAttributes {
                    company: "ACME-A".to_owned(),
                    org_unit_id: Some(ORG_UNIT_SALES),
                    job_position_id: Some(JOB_STAFF),
                    employment_status: "ACTIVE".to_owned(),
                },
            },
        ),
    )
    .await
    .unwrap();

    // CANONICAL pin: the backdated correction was appended as history.
    assert_eq!(
        count_rows(&owner_pool, COUNT_REVISIONS).await,
        3,
        "the backdated correction must still append a canonical revision"
    );
    let revision_windows: Vec<(i64, OffsetDateTime)> = sqlx::query(
        "SELECT version, valid_from FROM employment_revisions \
         WHERE org_id = $1 AND employment_id = $2 ORDER BY valid_from",
    )
    .bind(ORG)
    .bind(employment_id)
    .fetch_all(&owner_pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| {
        (
            row.get::<i64, _>("version"),
            row.get::<OffsetDateTime, _>("valid_from"),
        )
    })
    .collect();
    assert_eq!(
        revision_windows,
        vec![(1, at(0)), (3, at(86_400)), (2, at(172_800))],
        "intervals derived by valid_from order keep the backdated revision between its neighbours"
    );

    // LEGACY fix: the head must not move backward to the pre-transfer company.
    assert_eq!(
        legacy_head(&owner_pool, employee).await,
        (
            "ACME-B".to_owned(),
            Some(ORG_UNIT_TECH.to_string()),
            Some(JOB_LEAD.to_string()),
            "ACTIVE".to_owned()
        ),
        "a backdated promote must not reset the legacy head to the pre-transfer state"
    );
}

/// B01: a later correction changes the answer for its effective interval,
/// while a transfer and an exit retain their own exact half-open boundaries.
/// The read is made through the runtime-role port after all writes commit.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn as_of_read_replays_retroactive_change_and_preserves_pre_exit_truth(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (_employee, employment_id) =
        appointed(&owner_pool, org, actor, &port, "as-of-history").await;
    let transfer_at = at(172_800);
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Transfer {
                employment_id,
                valid_from: transfer_at,
                attributes: attributes(ORG_UNIT_TECH, JOB_LEAD, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap();

    let before_correction = get_as_of(&port, org, employment_id, at(86_400))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before_correction.org_unit_id, Some(ORG_UNIT_SALES));
    assert_eq!(before_correction.job_position_id, Some(JOB_STAFF));

    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id,
                valid_from: at(86_400),
                attributes: attributes(ORG_UNIT_SALES, JOB_LEAD, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap();

    assert!(
        get_as_of(&port, org, employment_id, at(-1))
            .await
            .unwrap()
            .is_none()
    );
    let before_correction_effect = get_as_of(&port, org, employment_id, at(86_399))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before_correction_effect.job_position_id, Some(JOB_STAFF));
    let corrected = get_as_of(&port, org, employment_id, at(86_400))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(corrected.org_unit_id, Some(ORG_UNIT_SALES));
    assert_eq!(corrected.job_position_id, Some(JOB_LEAD));
    let transferred = get_as_of(&port, org, employment_id, transfer_at)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(transferred.org_unit_id, Some(ORG_UNIT_TECH));
    assert_eq!(transferred.job_position_id, Some(JOB_LEAD));

    let exit_at = at(259_200);
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id,
                valid_from: exit_at,
                attributes: attributes(ORG_UNIT_TECH, JOB_LEAD, "EXITED"),
            },
        ),
    )
    .await
    .unwrap();

    assert!(get(&port, org, employment_id).await.unwrap().is_none());
    assert_eq!(
        get_as_of(&port, org, employment_id, at(259_199))
            .await
            .unwrap(),
        Some(transferred),
    );
    assert!(
        get_as_of(&port, org, employment_id, exit_at)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(count_rows(&owner_pool, COUNT_REVISIONS).await, 4);
}

/// The REST lifecycle handlers (`create_employee_lifecycle_event` and
/// `insert_confirmed_exit_lifecycle_event`) call `apply_employment_change`
/// directly on their own transaction, not through the port. The freeze gate
/// must live in the statement so this third caller is refused too.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn the_rest_lifecycle_statement_refuses_a_locked_effective_date(owner_pool: PgPool) {
    let (_org, _actor, _port) = fixture(&owner_pool).await;
    let employee = seed_employee(&owner_pool, ORG, "freeze-rest").await;

    let effective = at(86_400).date();
    seed_period_lock(&owner_pool, ORG, "payroll", effective).await;

    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let org_unit = ORG_UNIT_SALES.to_string();
    let position = JOB_STAFF.to_string();
    let effective_date = effective.to_string();
    let refused = apply_employment_change(
        &mut tx,
        ORG,
        employee,
        EmploymentChange {
            company: "ACME",
            org_unit: Some(&org_unit),
            position: Some(&position),
            employment_status: "ACTIVE",
            effective_date: &effective_date,
        },
    )
    .await
    .unwrap_err();
    drop(tx);

    assert!(
        matches!(&refused, EmploymentError::Frozen(error) if error.kind == ErrorKind::Conflict),
        "got {refused:?}"
    );
    assert_eq!(
        legacy_head(&owner_pool, employee).await,
        ("ACME".to_owned(), None, None, "ACTIVE".to_owned()),
        "a refused write must leave the legacy head untouched"
    );
}

/// P1 follow-up: the freeze gate runs for EVERY dated mutation, so a backdated
/// Promote (which skips only the legacy-head rewrite) must still be refused when
/// its effective date falls inside a locked period.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_backdated_promote_inside_a_locked_period_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (_employee, employment_id) =
        appointed(&owner_pool, org, actor, &port, "backdate-freeze").await;

    // Transfer to the LATER instant (t2), then try a backdated promote at t1.
    let transfer_at = at(172_800);
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Transfer {
                employment_id,
                valid_from: transfer_at,
                attributes: EmploymentAttributes {
                    company: "ACME-B".to_owned(),
                    org_unit_id: Some(ORG_UNIT_TECH),
                    job_position_id: Some(JOB_LEAD),
                    employment_status: "ACTIVE".to_owned(),
                },
            },
        ),
    )
    .await
    .unwrap();

    let backdated_at = at(86_400);
    seed_period_lock(&owner_pool, ORG, "payroll", backdated_at.date()).await;

    let refused = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id,
                valid_from: backdated_at,
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
            },
        ),
    )
    .await
    .expect_err("a backdated promote inside a locked period must be refused");

    assert!(
        matches!(&refused, EmploymentError::Frozen(error) if error.kind == ErrorKind::Conflict),
        "got {refused:?}"
    );
    assert_eq!(
        count_rows(&owner_pool, COUNT_REVISIONS).await,
        2,
        "no backdated revision may be appended through a closed window"
    );
}

/// P1 follow-up: `hr.appoint` stamps a business date too, so it must be refused
/// when its effective date falls inside a locked period — the gate cannot live
/// only on the Promote/Transfer legacy-head path.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_appoint_inside_a_locked_period_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let employee = seed_employee(&owner_pool, ORG, "appoint-freeze").await;

    let appoint_at = at(86_400);
    seed_period_lock(&owner_pool, ORG, "accounting", appoint_at.date()).await;

    let refused = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Appoint {
                employee_id: employee,
                valid_from: appoint_at,
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
            },
        ),
    )
    .await
    .expect_err("an appoint inside a locked period must be refused");

    assert!(
        matches!(&refused, EmploymentError::Frozen(error) if error.kind == ErrorKind::Conflict),
        "got {refused:?}"
    );
    assert_eq!(count_rows(&owner_pool, COUNT_HEADS).await, 0);
    assert_eq!(count_rows(&owner_pool, COUNT_REVISIONS).await, 0);
}

/// P2 follow-up: a revise predating the head's opening bound is outside the
/// employment's half-open lifetime and must be refused, not appended as history.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_revision_predating_the_head_opening_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (_employee, employment_id) = appointed(&owner_pool, org, actor, &port, "predate").await;

    // The head opened at at(0); a revise dated before it is outside the lifetime.
    let refused = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id,
                valid_from: at(-86_400),
                attributes: attributes(ORG_UNIT_SALES, JOB_LEAD, "ACTIVE"),
            },
        ),
    )
    .await
    .expect_err("a revise predating the head opening must be refused");

    assert!(
        matches!(&refused, EmploymentError::Blocked(_)),
        "got {refused:?}"
    );
    assert_eq!(
        count_rows(&owner_pool, COUNT_REVISIONS).await,
        1,
        "no predating revision may be appended"
    );
}

/// P2 follow-up: an unparseable REST `effective_date` is client input, so it
/// must surface as a validation refusal (422), not an internal failure (500).
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_rest_lifecycle_unparseable_date_is_validation_not_internal(owner_pool: PgPool) {
    let (_org, _actor, _port) = fixture(&owner_pool).await;
    let employee = seed_employee(&owner_pool, ORG, "unparseable").await;

    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let org_unit = ORG_UNIT_SALES.to_string();
    let position = JOB_STAFF.to_string();
    let refused = apply_employment_change(
        &mut tx,
        ORG,
        employee,
        EmploymentChange {
            company: "ACME",
            org_unit: Some(&org_unit),
            position: Some(&position),
            employment_status: "ACTIVE",
            effective_date: "not-a-date",
        },
    )
    .await
    .unwrap_err();
    drop(tx);

    assert!(
        matches!(&refused, EmploymentError::Frozen(error) if error.kind == ErrorKind::Validation),
        "an unparseable date must be a validation refusal, not 500: got {refused:?}"
    );
}

/// P1 follow-up: the lock date derives from the fixed KST offset, not the
/// caller-supplied RFC3339 offset. A UTC instant late in one day — already the
/// next day in KST — must be refused by a lock on the KST date.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_lock_on_the_kst_business_date_refuses_a_utc_instant(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (_employee, employment_id) = appointed(&owner_pool, org, actor, &port, "kst").await;

    // at(0) is 08:00Z; +12h lands at 20:00Z, whose KST (UTC+9) date is the next
    // calendar day.
    let utc_instant = at(43_200);
    let kst_date = utc_instant.to_offset(offset!(+9)).date();
    assert_ne!(
        utc_instant.date(),
        kst_date,
        "fixture must cross a KST day boundary"
    );
    seed_period_lock(&owner_pool, ORG, "payroll", kst_date).await;

    let refused = execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id,
                valid_from: utc_instant,
                attributes: attributes(ORG_UNIT_SALES, JOB_LEAD, "ACTIVE"),
            },
        ),
    )
    .await
    .expect_err("a lock on the KST business date must refuse the UTC instant");

    assert!(
        matches!(&refused, EmploymentError::Frozen(error) if error.kind == ErrorKind::Conflict),
        "got {refused:?}"
    );
}

/// ROADMAP item 5's assignment writer on the org tree: a provisioned empty
/// tenant, as `console_rt`, creates Company / OrgUnit / JobPosition through
/// their owning ports, inserts the legacy employee row the Employment port
/// owns, binds a Person, then `hr.appoint`s onto those UUIDs. Fixture
/// `INSERT INTO org_units` is not this path.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn empty_tenant_hr_appoint_sits_on_canonical_org_tree(owner_pool: PgPool) {
    let tree = provision_empty_tenant_assignment(&owner_pool).await;
    assert_eq!(tree.appointed.target(), DispatchTarget::HrAppoint);
    let head = get(&tree.employment, tree.org, tree.employment_id)
        .await
        .unwrap()
        .expect("hr.appoint must produce an open queryable head");
    assert_eq!(head.org_unit_id, Some(tree.sales));
    assert_eq!(head.job_position_id, Some(tree.engineer));
    assert_eq!(
        head.person_id,
        Some(tree.employee_id),
        "a uniquely-resolved person is bound with person_id = employee_id"
    );
}

/// The same empty-tenant tree then `hr.promote`s (new JobPosition, same
/// OrgUnit) and `hr.transfer`s (new OrgUnit + JobPosition) through the
/// Employment port. Both verbs store their own action keys, not the shared
/// helper's `revise`.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn empty_tenant_hr_promote_and_transfer_sit_on_canonical_org_tree(owner_pool: PgPool) {
    let tree = provision_empty_tenant_assignment(&owner_pool).await;

    let tech_receipt = execute_sync(
        &tree.units,
        OrgUnitCommand {
            org_id: tree.org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: tree.actor,
            query: OrgUnitQuery::Create {
                source: None,
                attributes: json!({ "name": "기술본부", "kind": "site" }),
            },
            action_key: "create_org_unit".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();
    let tech: Uuid = tech_receipt.result()["org_unit_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let senior_receipt = execute_sync(
        &tree.positions,
        JobPositionCommand {
            org_id: tree.org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: tree.actor,
            query: JobPositionQuery::Create {
                org_unit_id: tree.sales,
                attributes: json!({ "title": "시니어 엔지니어" }),
            },
            action_key: "create_job_position".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();
    let senior: Uuid = senior_receipt.result()["job_position_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let lead_receipt = execute_sync(
        &tree.positions,
        JobPositionCommand {
            org_id: tree.org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: tree.actor,
            query: JobPositionQuery::Create {
                org_unit_id: tech,
                attributes: json!({ "title": "테크 리드" }),
            },
            action_key: "create_job_position".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();
    let lead: Uuid = lead_receipt.result()["job_position_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let promoted = execute(
        &tree.employment,
        EmploymentCommand {
            org_id: tree.org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: tree.actor,
            query: EmploymentQuery::Promote {
                employment_id: tree.employment_id,
                valid_from: at(86_400),
                attributes: EmploymentAttributes {
                    company: "ACME".to_owned(),
                    org_unit_id: Some(tree.sales),
                    job_position_id: Some(senior),
                    employment_status: "ACTIVE".to_owned(),
                },
            },
            action_key: "promote".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();
    assert_eq!(promoted.target(), DispatchTarget::HrPromote);
    assert_eq!(promoted.result()["version"].as_i64(), Some(2));
    let after_promote = get(&tree.employment, tree.org, tree.employment_id)
        .await
        .unwrap()
        .expect("hr.promote must leave an open queryable head");
    assert_eq!(after_promote.org_unit_id, Some(tree.sales));
    assert_eq!(after_promote.job_position_id, Some(senior));
    assert_eq!(after_promote.person_id, Some(tree.employee_id));

    let transferred = execute(
        &tree.employment,
        EmploymentCommand {
            org_id: tree.org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: tree.actor,
            query: EmploymentQuery::Transfer {
                employment_id: tree.employment_id,
                valid_from: at(172_800),
                attributes: EmploymentAttributes {
                    company: "ACME".to_owned(),
                    org_unit_id: Some(tech),
                    job_position_id: Some(lead),
                    employment_status: "ACTIVE".to_owned(),
                },
            },
            action_key: "transfer".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();
    assert_eq!(transferred.target(), DispatchTarget::HrTransfer);
    assert_eq!(transferred.result()["version"].as_i64(), Some(3));
    let after_transfer = get(&tree.employment, tree.org, tree.employment_id)
        .await
        .unwrap()
        .expect("hr.transfer must leave an open queryable head");
    assert_eq!(after_transfer.org_unit_id, Some(tech));
    assert_eq!(after_transfer.job_position_id, Some(lead));
    assert_eq!(after_transfer.person_id, Some(tree.employee_id));
}

struct EmptyTenantAssignment {
    org: OrgId,
    actor: UserId,
    units: PgOrgUnitPort,
    positions: PgJobPositionPort,
    employment: PgEmploymentPort,
    sales: Uuid,
    engineer: Uuid,
    employee_id: Uuid,
    employment_id: Uuid,
    appointed: CommandReceipt,
}

async fn provision_empty_tenant_assignment(owner_pool: &PgPool) -> EmptyTenantAssignment {
    let actor = seed_org_and_super_admin(owner_pool, ORG, "employment").await;
    let runtime_pool = runtime_role_pool(owner_pool).await;
    let handle = tokio::runtime::Handle::current();
    let company = PgCompanyPort::new(runtime_pool.clone(), handle.clone());
    let units = PgOrgUnitPort::new(runtime_pool.clone(), handle.clone());
    let positions = PgJobPositionPort::new(runtime_pool.clone(), handle.clone());
    let persons = PgPersonPort::new(runtime_pool.clone(), handle.clone());
    let employment = PgEmploymentPort::new(runtime_pool.clone(), handle);
    let org = OrgId::from_uuid(ORG);

    execute_sync(
        &company,
        CompanyCommand {
            org_id: org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: CompanyQuery {
                attributes: json!({ "legal_name": "주식회사 아크메" }),
            },
            action_key: "company.revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();

    let unit_receipt = execute_sync(
        &units,
        OrgUnitCommand {
            org_id: org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: OrgUnitQuery::Create {
                source: None,
                attributes: json!({ "name": "영업본부", "kind": "site" }),
            },
            action_key: "create_org_unit".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();
    let sales: Uuid = unit_receipt.result()["org_unit_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let position_receipt = execute_sync(
        &positions,
        JobPositionCommand {
            org_id: org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: JobPositionQuery::Create {
                org_unit_id: sales,
                attributes: json!({ "title": "백엔드 엔지니어" }),
            },
            action_key: "create_job_position".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();
    let engineer: Uuid = position_receipt.result()["job_position_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let employee_id = Uuid::new_v4();
    let sales_text = sales.to_string();
    let engineer_text = engineer.to_string();
    let mut tx = runtime_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    insert_employee_record(
        &mut tx,
        ORG,
        NewEmployeeRecord {
            employee_id,
            company: "ACME",
            name: "김직원",
            employee_number: "E-1001",
            org_unit: &sales_text,
            position: &engineer_text,
            worksite_name: "서울",
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    execute_sync(
        &persons,
        PersonCommand {
            org_id: org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: PersonQuery::Create {
                employee_id: Some(employee_id),
                attributes: json!({ "legal_name": "김직원" }),
            },
            action_key: "create_person".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();

    let appointed = execute(
        &employment,
        EmploymentCommand {
            org_id: org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: EmploymentQuery::Appoint {
                employee_id,
                valid_from: at(0),
                attributes: EmploymentAttributes {
                    company: "ACME".to_owned(),
                    org_unit_id: Some(sales),
                    job_position_id: Some(engineer),
                    employment_status: "ACTIVE".to_owned(),
                },
            },
            action_key: "appoint".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();
    let employment_id = employment_of(&appointed);
    EmptyTenantAssignment {
        org,
        actor,
        units,
        positions,
        employment,
        sales,
        engineer,
        employee_id,
        employment_id,
        appointed,
    }
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

/// B01 read-only slice: current canonical reads must derive the state effective
/// at the database clock. This intentionally makes no assertion about the
/// legacy `employees` projection or serving attendance/leave routes.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn canonical_employment_current_reads_follow_effective_time_without_a_worker(
    owner_pool: PgPool,
) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let db_now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    let past = db_now - time::Duration::days(1);
    let boundary = db_now + time::Duration::seconds(12);

    let future_employee = seed_employee(&owner_pool, ORG, "effective-read-appoint").await;
    let future_employment = employment_of(
        &execute(
            &port,
            command(
                org,
                actor,
                EmploymentQuery::Appoint {
                    employee_id: future_employee,
                    valid_from: boundary,
                    attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
                },
            ),
        )
        .await
        .unwrap(),
    );

    let transfer_employee = seed_employee(&owner_pool, ORG, "effective-read-transfer").await;
    let transfer_employment = employment_of(
        &execute(
            &port,
            command(
                org,
                actor,
                EmploymentQuery::Appoint {
                    employee_id: transfer_employee,
                    valid_from: past,
                    attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
                },
            ),
        )
        .await
        .unwrap(),
    );
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Transfer {
                employment_id: transfer_employment,
                valid_from: boundary,
                attributes: attributes(ORG_UNIT_TECH, JOB_LEAD, "ACTIVE"),
            },
        ),
    )
    .await
    .unwrap();

    let exit_employee = seed_employee(&owner_pool, ORG, "effective-read-exit").await;
    let exit_employment = employment_of(
        &execute(
            &port,
            command(
                org,
                actor,
                EmploymentQuery::Appoint {
                    employee_id: exit_employee,
                    valid_from: past,
                    attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
                },
            ),
        )
        .await
        .unwrap(),
    );
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id: exit_employment,
                valid_from: boundary,
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "EXITED"),
            },
        ),
    )
    .await
    .unwrap();

    let before: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert!(before < boundary, "fixture missed the future boundary");
    assert!(
        get_as_of(&port, org, future_employment, before)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        get_as_of(&port, org, future_employment, boundary)
            .await
            .unwrap()
            .is_some()
    );
    let transfer_before = get_as_of(&port, org, transfer_employment, before)
        .await
        .unwrap()
        .expect("transfer has an effective prior state");
    assert_eq!(transfer_before.org_unit_id, Some(ORG_UNIT_SALES));
    assert_eq!(
        get_as_of(&port, org, transfer_employment, boundary)
            .await
            .unwrap()
            .unwrap()
            .org_unit_id,
        Some(ORG_UNIT_TECH)
    );
    let exit_before = get_as_of(&port, org, exit_employment, before)
        .await
        .unwrap()
        .expect("exit has an effective prior state");
    assert!(
        get_as_of(&port, org, exit_employment, boundary)
            .await
            .unwrap()
            .is_none()
    );

    let mut premature = Vec::new();
    if get(&port, org, future_employment).await.unwrap().is_some() {
        premature.push("future appointment appears in current get");
    }
    if get(&port, org, transfer_employment).await.unwrap() != Some(transfer_before.clone()) {
        premature.push("future transfer replaces current get");
    }
    if get(&port, org, exit_employment).await.unwrap() != Some(exit_before.clone()) {
        premature.push("future exit removes current get");
    }
    let current_ids: Vec<Uuid> = list(&port, org)
        .await
        .unwrap()
        .iter()
        .map(|head| head.id)
        .collect();
    if current_ids.contains(&future_employment) {
        premature.push("future appointment appears in current list");
    }
    if !current_ids.contains(&transfer_employment) || !current_ids.contains(&exit_employment) {
        premature.push("future transfer or exit is missing from current list");
    }
    let observed_before: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert!(
        observed_before < boundary,
        "fixture crossed the boundary before current reads finished"
    );

    while !sqlx::query_scalar::<_, bool>("SELECT clock_timestamp() >= $1")
        .bind(boundary)
        .fetch_one(&owner_pool)
        .await
        .unwrap()
    {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    let after_ids: Vec<Uuid> = list(&port, org)
        .await
        .unwrap()
        .iter()
        .map(|head| head.id)
        .collect();
    assert!(after_ids.contains(&future_employment));
    assert!(after_ids.contains(&transfer_employment));
    assert!(!after_ids.contains(&exit_employment));
    assert!(get(&port, org, future_employment).await.unwrap().is_some());
    assert_eq!(
        get(&port, org, transfer_employment)
            .await
            .unwrap()
            .unwrap()
            .org_unit_id,
        Some(ORG_UNIT_TECH)
    );
    assert!(get(&port, org, exit_employment).await.unwrap().is_none());
    assert_eq!(
        get_as_of(&port, org, transfer_employment, past)
            .await
            .unwrap()
            .unwrap()
            .org_unit_id,
        Some(ORG_UNIT_SALES),
        "the past transfer interval remains queryable after the boundary"
    );
    assert_eq!(
        get_as_of(&port, org, exit_employment, past).await.unwrap(),
        Some(exit_before),
        "pre-exit history remains queryable after the boundary"
    );
    assert!(premature.is_empty(), "{}", premature.join("\n"));
}

/// UNKNOWN preserves the relationship record but is not an active work
/// assignment. The canonical active-head reads must not treat it as ACTIVE.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn unknown_effective_status_is_not_a_current_assignment(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let db_now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    let employee = seed_employee(&owner_pool, ORG, "unknown-current-read").await;
    let employment_id = employment_of(
        &execute(
            &port,
            command(
                org,
                actor,
                EmploymentQuery::Appoint {
                    employee_id: employee,
                    valid_from: db_now - time::Duration::days(1),
                    attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
                },
            ),
        )
        .await
        .unwrap(),
    );
    assert!(get(&port, org, employment_id).await.unwrap().is_some());
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id,
                valid_from: db_now - time::Duration::hours(1),
                attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "UNKNOWN"),
            },
        ),
    )
    .await
    .unwrap();

    assert!(get(&port, org, employment_id).await.unwrap().is_none());
    assert!(
        list(&port, org)
            .await
            .unwrap()
            .iter()
            .all(|head| head.id != employment_id)
    );
    assert!(
        get_as_of(&port, org, employment_id, db_now)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        get_as_of(&port, org, employment_id, db_now - time::Duration::hours(2))
            .await
            .unwrap()
            .is_some()
    );
}

/// A canonical detail or list must report an N:1 source-binding conflict,
/// never silently omit an ACTIVE worker or select one source arbitrarily.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn current_employment_reads_refuse_ambiguous_source_bindings(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let db_now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    let first = seed_employee(&owner_pool, ORG, "current-ambiguous-first").await;
    let employment_id = employment_of(
        &execute(
            &port,
            command(
                org,
                actor,
                EmploymentQuery::Appoint {
                    employee_id: first,
                    valid_from: db_now - time::Duration::days(1),
                    attributes: attributes(ORG_UNIT_SALES, JOB_STAFF, "ACTIVE"),
                },
            ),
        )
        .await
        .unwrap(),
    );
    assert!(get(&port, org, employment_id).await.unwrap().is_some());

    let second = seed_employee(&owner_pool, ORG, "current-ambiguous-second").await;
    sqlx::query(
        "INSERT INTO employment_source_bindings \
         (org_id, employee_id, employment_id, actor_id, payload_digest) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(ORG)
    .bind(second)
    .bind(employment_id)
    .bind(*actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .execute(&owner_pool)
    .await
    .unwrap();
    let bound: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM employment_source_bindings \
         WHERE org_id = $1 AND employment_id = $2",
    )
    .bind(ORG)
    .bind(employment_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(bound, 2, "fixture must represent a real reverse conflict");

    let detail = get(&port, org, employment_id).await;
    let listed = list(&port, org).await;
    assert!(
        matches!(
            detail,
            Err(EmploymentError::AmbiguousSourceBinding {
                employment_id: id,
                binding_count: 2
            }) if id == employment_id
        ),
        "detail must report the conflict, not return a head or None"
    );
    assert!(
        matches!(
            listed,
            Err(EmploymentError::AmbiguousSourceBinding {
                employment_id: id,
                binding_count: 2
            }) if id == employment_id
        ),
        "list must report the conflict, not silently omit this worker"
    );
}
