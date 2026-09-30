#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `PersonPort` proven against a REAL PostgreSQL as the genuine runtime role
//! `console_rt` — never the BYPASSRLS superuser the `#[sqlx::test]` pool
//! connects as, which would mask a broken `org_isolation` policy.
//! `a_foreign_tenant_is_invisible_and_unwritable_to_the_runtime_role` is what
//! makes that claim observable: it is the only test that crosses a tenant
//! boundary, and it dies when `org_isolation` is loosened to `USING (true)`.
//!
//! WHY `#[sqlx::test]` IS NOT OPTIONAL HERE. Migration 0196 refuses a superuser
//! applier unless `CURRENT_DATABASE()` matches `^_sqlx_test_[A-Za-z0-9_]{52}$`
//! with the `console.sqlx_test_bootstrap` marker set — measured, as
//! `42501 platform_force_role_topology.superuser_test_bootstrap_required`, by
//! running this suite against a hand-migrated database first. So the schema
//! itself admits exactly one applier, and a `sqlx::migrate!` harness of our own
//! is not a design choice that was available.
//!
//! WHY `execute` IS CALLED FROM `spawn_blocking`. `CanonicalPort::execute` is
//! SYNCHRONOUS — `canonical-domain` declares it so and this lane may not edit
//! that crate — so the adapter bridges to `sqlx` with `Handle::block_on`, which
//! panics inside an async context. A `spawn_blocking` thread is not one: the
//! blocking pool never calls `enter_runtime`, so its `EnterRuntime` flag stays
//! `NotEntered`. `#[sqlx::test]` drives a CURRENT-THREAD runtime, on which
//! `Handle::block_on` cannot turn the IO driver itself — which is fine here for
//! the reason tokio's own documentation gives: the test task is parked awaiting
//! the `JoinHandle`, so `Runtime::block_on` on the main thread is still driving.
//!
//! IMMUTABILITY IS THE TRIGGER, AND ONLY THE TRIGGER. There is no second layer:
//! `ops/postgres-reconcile-topology.sh` runs `ALTER DEFAULT PRIVILEGES FOR ROLE
//! console_app IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES
//! TO console_rt` BEFORE migrations, so in the deployed database `console_rt`
//! DOES hold UPDATE on `person_revisions` — migration 0213 says so in its own
//! header ("the runtime role already holds UPDATE on every table this file
//! creates"). `#[sqlx::test]` instead applies migrations as the cluster
//! superuser, for whom no such per-role default privilege exists, so an UPDATE
//! here would otherwise be refused by a `42501` that does not exist where the
//! code ships. `an_update_of_a_revision_row_is_refused_by_the_trigger` therefore
//! GRANTs UPDATE to `console_rt` first, reproducing the production ACL, asserts
//! the grant is really held, and only then asserts the `P0001` the trigger
//! raises.

use console_kernel_core::{OrgId, UserId};
use console_ontology_canonical_adapter_postgres::person::{
    PersonCommand, PersonError, PersonHead, PersonQuery, PgPersonPort,
};
use console_ontology_canonical_domain::{
    CanonicalPort, CommandId, CommandReceipt, DispatchTarget, ObjectKey, PersonPort, ReceiptOwner,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0x9e50_0000_0000_0000_0000_0000_0000_0001);
const FOREIGN_ORG: Uuid = Uuid::from_u128(0x9e50_0000_0000_0000_0000_0000_0000_0002);

/// The port must satisfy the NAMED trait, not merely `CanonicalPort`. The
/// blanket impl in `canonical-domain` makes `PersonPort` an alias for
/// `CanonicalPort<Object = Person>`, so this bound stops holding the moment the
/// adapter is retargeted at a different object.
fn assert_implements_person_port<P: PersonPort>() {}

/// `console_platform_test_support::runtime_role_pool`, inlined: adding that
/// crate as a dev-dependency rewrites this package's entry in
/// `backend/Cargo.lock`, which this lane may not write.
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

/// `console_platform_test_support::seed_org_and_super_admin`, inlined for the
/// same reason. Seeded as the migration owner, before any role switch.
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

/// The tenant, its actor, and the port built on a `console_rt` pool.
async fn fixture(owner_pool: &PgPool) -> (OrgId, UserId, PgPersonPort) {
    let actor = seed_org_and_super_admin(owner_pool, ORG, "person").await;
    let runtime_pool = runtime_role_pool(owner_pool).await;
    let port = PgPersonPort::new(runtime_pool, tokio::runtime::Handle::current());
    (OrgId::from_uuid(ORG), actor, port)
}

/// Drive the SYNCHRONOUS `execute` off the runtime's worker thread. See the
/// module doc: this is the whole reason the suite is shaped this way.
async fn execute(
    port: &PgPersonPort,
    command: PersonCommand,
) -> Result<CommandReceipt, PersonError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.execute(&command))
        .await
        .unwrap()
}

async fn get(
    port: &PgPersonPort,
    org: OrgId,
    person_id: Uuid,
) -> Result<Option<PersonHead>, PersonError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.get(org, person_id))
        .await
        .unwrap()
}

async fn list(port: &PgPersonPort, org: OrgId) -> Result<Vec<PersonHead>, PersonError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.list(org))
        .await
        .unwrap()
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

const COUNT_PERSONS: &str = "SELECT count(*)::bigint FROM persons";
const COUNT_REVISIONS: &str = "SELECT count(*)::bigint FROM person_revisions";
const COUNT_BINDINGS: &str = "SELECT count(*)::bigint FROM employee_person_bindings";
const COUNT_RECEIPTS: &str = "SELECT count(*)::bigint FROM ont_action_command_receipts";

/// The three tables the contract assigns to `Person`, each with the count over
/// it. `sqlx` 0.9 accepts only `&'static str` SQL, so the table name cannot be
/// interpolated and every statement here is a literal.
const OWNED_TABLES: [(&str, &str); 3] = [
    ("persons", COUNT_PERSONS),
    ("person_revisions", COUNT_REVISIONS),
    ("employee_person_bindings", COUNT_BINDINGS),
];

/// Rows counted through the BYPASSRLS owner pool, so a test can see what a
/// tenant-armed session must not.
async fn count_rows(owner_pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(owner_pool).await.unwrap()
}

/// The SQLSTATE and the message PostgreSQL actually returned, so a test quotes
/// the real error rather than a paraphrase of it.
fn database_error(error: &PersonError) -> (String, String) {
    let PersonError::Database(sqlx_error) = error else {
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

fn create(employee_id: Option<Uuid>, name: &str) -> PersonQuery {
    PersonQuery::Create {
        employee_id,
        attributes: json!({ "legal_name": name }),
    }
}

fn revise(person_id: Uuid, employee_id: Option<Uuid>, name: &str) -> PersonQuery {
    PersonQuery::Revise {
        person_id,
        employee_id,
        attributes: json!({ "legal_name": name }),
    }
}

fn command(org: OrgId, actor: UserId, query: PersonQuery) -> PersonCommand {
    PersonCommand {
        org_id: org,
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        actor_id: actor,
        query,
        action_key: "revise".to_owned(),
        object_type_id: Uuid::nil(),
    }
}

fn person_of(receipt: &CommandReceipt) -> Uuid {
    receipt.result()["person_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

/// The whole revision row as JSONB, so "unchanged" means the whole row and not
/// the two columns a test remembered to name.
async fn revision_snapshot(owner_pool: &PgPool, person: Uuid, version: i64) -> String {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT to_jsonb(r) FROM person_revisions r \
         WHERE org_id = $1 AND person_id = $2 AND version = $3",
    )
    .bind(ORG)
    .bind(person)
    .bind(version)
    .fetch_one(owner_pool)
    .await
    .unwrap()
    .to_string()
}

#[test]
fn the_contract_identity_is_copied_verbatim_and_the_port_is_the_named_one() {
    assert_implements_person_port::<PgPersonPort>();
    assert_eq!(ObjectKey::Person.as_str(), "person");
    assert_eq!(
        ObjectKey::Person.owned_tables(),
        ["persons", "person_revisions", "employee_person_bindings"]
    );
    assert_eq!(
        ObjectKey::Person.owner_crate(),
        "console-ontology-canonical-adapter-postgres"
    );
    assert_eq!(
        DispatchTarget::PeopleCreatePerson.as_str(),
        "people.create_person"
    );
    assert_eq!(
        DispatchTarget::PeopleRevisePerson.as_str(),
        "people.revise_person"
    );
    assert_eq!(
        DispatchTarget::PeopleCreatePerson.object(),
        ObjectKey::Person
    );
    assert_eq!(
        DispatchTarget::PeopleRevisePerson.object(),
        ObjectKey::Person
    );
}

#[test]
fn preflight_is_pure_and_blocks_a_non_object_attribute_payload() {
    let query = PersonQuery::Create {
        employee_id: None,
        attributes: json!("not an object"),
    };
    let preflight = <PgPersonPort as CanonicalPort>::preflight(&query);
    assert!(!preflight.is_ok());
    assert_eq!(
        preflight.blockers(),
        ["attributes must be a JSON object".to_owned()]
    );

    let nil_target = <PgPersonPort as CanonicalPort>::preflight(&revise(Uuid::nil(), None, "무명"));
    assert!(!nil_target.is_ok());
    assert_eq!(
        nil_target.blockers(),
        ["person_id must not be nil".to_owned()]
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_person_is_created_and_read_back(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let query = create(None, "김철수");
    assert!(<PgPersonPort as CanonicalPort>::preflight(&query).is_ok());

    let receipt = execute(&port, command(org, actor, query)).await.unwrap();

    assert_eq!(receipt.org_id(), org);
    assert_eq!(receipt.actor_id(), actor);
    assert_eq!(
        receipt.owner(),
        ReceiptOwner::Canonical(ObjectKey::Person),
        "the receipt must be owned by the canonical Person object"
    );
    assert_eq!(receipt.target(), DispatchTarget::PeopleCreatePerson);
    assert_eq!(receipt.result()["version"].as_i64(), Some(1));

    let person_id = person_of(&receipt);
    let row = sqlx::query(
        "SELECT r.version, r.attributes, r.command_id, r.payload_digest \
         FROM persons p JOIN person_revisions r ON r.org_id = p.org_id AND r.person_id = p.id \
         WHERE p.org_id = $1 AND p.id = $2",
    )
    .bind(ORG)
    .bind(person_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();

    assert_eq!(row.get::<i64, _>("version"), 1);
    assert_eq!(
        row.get::<serde_json::Value, _>("attributes"),
        json!({ "legal_name": "김철수" })
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

    let head = get(&port, org, person_id)
        .await
        .unwrap()
        .expect("created Person must be queryable");
    assert_eq!(head.id, person_id);
    assert_eq!(head.legal_name.as_deref(), Some("김철수"));
    assert_eq!(
        head.display_name, None,
        "display_name was not stored and must not be invented from legal_name"
    );
    assert_eq!(head.version, 1);
    assert_eq!(list(&port, org).await.unwrap(), vec![head]);
    let unknown = get(&port, org, Uuid::new_v4()).await;
    assert!(
        matches!(unknown, Ok(None)),
        "unknown Person id is Ok(None) on the runtime-role pool, never a distinct error; got {unknown:?}"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_revision_is_appended_and_the_prior_revision_is_unchanged(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let created = execute(&port, command(org, actor, create(None, "이영희")))
        .await
        .unwrap();
    let person_id = person_of(&created);

    let before = revision_snapshot(&owner_pool, person_id, 1).await;

    let revised = execute(
        &port,
        command(org, actor, revise(person_id, None, "이영희(개명)")),
    )
    .await
    .unwrap();
    assert_eq!(revised.target(), DispatchTarget::PeopleRevisePerson);
    assert_eq!(revised.result()["version"].as_i64(), Some(2));

    let head = get(&port, org, person_id)
        .await
        .unwrap()
        .expect("latest head");
    assert_eq!(head.version, 2);
    assert_eq!(head.legal_name.as_deref(), Some("이영희(개명)"));

    let after = revision_snapshot(&owner_pool, person_id, 1).await;
    assert_eq!(before, after, "appending a revision rewrote revision 1");

    let versions: Vec<i64> = sqlx::query_scalar(
        "SELECT version FROM person_revisions WHERE org_id = $1 AND person_id = $2 ORDER BY version",
    )
    .bind(ORG)
    .bind(person_id)
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(versions, vec![1, 2]);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_update_of_a_revision_row_is_refused_by_the_trigger(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let created = execute(&port, command(org, actor, create(None, "박민수")))
        .await
        .unwrap();
    let person_id = person_of(&created);
    let before = revision_snapshot(&owner_pool, person_id, 1).await;

    // Reproduce the DEPLOYED ACL before asserting anything about it. See the
    // module doc: production applies migrations as `console_app`, after
    // `ALTER DEFAULT PRIVILEGES ... GRANT ... UPDATE ... TO console_rt`, so the
    // runtime role holds UPDATE there. `#[sqlx::test]` applies them as the
    // superuser, for whom that default privilege does not exist.
    sqlx::query("GRANT UPDATE ON person_revisions TO console_rt")
        .execute(&owner_pool)
        .await
        .unwrap();
    let runtime_holds_update: bool = sqlx::query_scalar(
        "SELECT has_table_privilege('console_rt', 'person_revisions', 'UPDATE')",
    )
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert!(
        runtime_holds_update,
        "the production ACL must be in place, or the assertion below observes a privilege error \
         that does not exist where this code ships"
    );

    // The runtime role, holding UPDATE exactly as it does in production.
    let runtime_pool = runtime_role_pool(&owner_pool).await;
    let mut tx = runtime_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let as_runtime_role = sqlx::query(
        "UPDATE person_revisions SET attributes = '{\"legal_name\":\"forged\"}'::jsonb \
         WHERE org_id = $1 AND person_id = $2",
    )
    .bind(ORG)
    .bind(person_id)
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
        "canonical person table person_revisions: UPDATE is refused, the row is immutable"
    );
    drop(tx);

    // The owner is refused by the same trigger; ownership buys no exemption.
    let as_owner =
        sqlx::query("UPDATE person_revisions SET version = version + 100 WHERE org_id = $1")
            .bind(ORG)
            .execute(&owner_pool)
            .await
            .unwrap_err();
    let owner_error = as_owner.as_database_error().unwrap();
    assert_eq!(owner_error.code().unwrap(), "P0001");
    assert_eq!(
        owner_error.message(),
        "canonical person table person_revisions: UPDATE is refused, the row is immutable"
    );

    let after = revision_snapshot(&owner_pool, person_id, 1).await;
    assert_eq!(before, after, "a refused UPDATE rewrote the revision row");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_foreign_tenant_is_invisible_and_unwritable_to_the_runtime_role(owner_pool: PgPool) {
    let (org, _actor, port) = fixture(&owner_pool).await;
    let foreign_actor = seed_org_and_super_admin(&owner_pool, FOREIGN_ORG, "foreign").await;
    let foreign_employee = seed_employee(&owner_pool, FOREIGN_ORG, "foreign-1").await;

    // A person, a revision and a binding that genuinely exist — under the OTHER
    // tenant. Seeded through the BYPASSRLS owner pool, which is the only way to
    // put rows on the far side of the boundary being tested.
    let foreign_person: Uuid =
        sqlx::query_scalar("INSERT INTO persons (org_id) VALUES ($1) RETURNING id")
            .bind(FOREIGN_ORG)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO person_revisions \
         (org_id, person_id, version, command_id, actor_id, payload_digest, attributes, receipt) \
         VALUES ($1, $2, 1, gen_random_uuid(), $3, $4, '{}'::jsonb, '{}'::jsonb)",
    )
    .bind(FOREIGN_ORG)
    .bind(foreign_person)
    .bind(*foreign_actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO employee_person_bindings \
         (org_id, employee_id, person_id, actor_id, payload_digest) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(FOREIGN_ORG)
    .bind(foreign_employee)
    .bind(foreign_person)
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
    let refused = sqlx::query("INSERT INTO persons (org_id) VALUES ($1)")
        .bind(FOREIGN_ORG)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    let error = refused.as_database_error().unwrap();
    assert_eq!(error.code().unwrap(), "42501", "got {}", error.message());
    assert_eq!(
        error.message(),
        "new row violates row-level security policy for table \"persons\""
    );
    drop(tx);

    assert!(list(&port, org).await.unwrap().is_empty());
    let foreign_head = get(&port, org, foreign_person).await;
    assert!(
        matches!(foreign_head, Ok(None)),
        "foreign Person id is Ok(None) on the runtime-role pool, never a wrong-tenant error; got {foreign_head:?}"
    );
    let unknown = get(&port, org, Uuid::new_v4()).await;
    assert!(
        matches!(unknown, Ok(None)),
        "unknown Person id is indistinguishable from a foreign tenant: Ok(None); got {unknown:?}"
    );
}

/// P5 handoff: a trusted uniquely-resolved employee binds with
/// `person_id = employee_id`. A random UUID here would break the deterministic
/// identity map console-dgo.1 reads.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn trusted_uniquely_resolved_employee_binds_with_person_id_equal_employee_id(
    owner_pool: PgPool,
) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let employee = seed_employee(&owner_pool, ORG, "trusted-1").await;

    let receipt = execute(
        &port,
        command(org, actor, create(Some(employee), "신뢰직원")),
    )
    .await
    .unwrap();
    let person_id = person_of(&receipt);

    assert_eq!(
        person_id, employee,
        "trusted uniquely-resolved create must use person_id = employee_id; got person={person_id} employee={employee}"
    );

    let bound: Uuid = sqlx::query_scalar(
        "SELECT person_id FROM employee_person_bindings \
         WHERE org_id = $1 AND employee_id = $2",
    )
    .bind(ORG)
    .bind(employee)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(bound, employee);
}

/// Duplicate / review-required imports stay unbound: omitting `employee_id`
/// must create the person with ZERO binding rows, even when attributes carry
/// name/phone/org text that a fuzzy matcher could abuse.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn create_without_employee_id_stays_unbound_despite_name_phone_org_attributes(
    owner_pool: PgPool,
) {
    let (org, actor, port) = fixture(&owner_pool).await;
    // A peer employee whose name/phone match the attributes below — tempting
    // bait for an inference path. The port must ignore it.
    let _peer = seed_employee(&owner_pool, ORG, "동명이인-peer").await;

    let query = PersonQuery::Create {
        employee_id: None,
        attributes: json!({
            "legal_name": "동명이인",
            "phone": "+82-10-1234-5678",
            "salary": 50_000_000,
            "bank_account": "123-456-789012",
            "rrn": "900101-1234567",
            "org_text": "ACME / 서울지사",
            "source_key": "동명이인-peer",
        }),
    };
    let receipt = execute(&port, command(org, actor, query)).await.unwrap();
    let person_id = person_of(&receipt);

    let bindings: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM employee_person_bindings WHERE org_id = $1",
    )
    .bind(ORG)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        bindings, 0,
        "review-required / unbound create must never invent a binding from name/phone/org text"
    );

    let person_bindings: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM employee_person_bindings \
         WHERE org_id = $1 AND person_id = $2",
    )
    .bind(ORG)
    .bind(person_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(person_bindings, 0);

    let stored: serde_json::Value = sqlx::query_scalar(
        "SELECT attributes FROM person_revisions \
         WHERE org_id = $1 AND person_id = $2 AND version = 1",
    )
    .bind(ORG)
    .bind(person_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        stored.get("phone").and_then(serde_json::Value::as_str),
        Some("+82-10-1234-5678"),
        "create may persist phone on the revision; get/list must still omit it"
    );
    for key in ["salary", "bank_account", "rrn"] {
        assert!(
            stored.get(key).is_some(),
            "create stored {key} on the revision, so a leaking head would be observable"
        );
    }

    let head = get(&port, org, person_id)
        .await
        .unwrap()
        .expect("unbound create is still a queryable Person head");
    assert_eq!(head.id, person_id);
    assert_eq!(head.legal_name.as_deref(), Some("동명이인"));
    assert_eq!(head.display_name, None);
    let listed = list(&port, org).await.unwrap();
    assert_eq!(listed, vec![head.clone()]);
    for (label, wire) in [
        ("get", serde_json::to_value(&head).unwrap()),
        ("list", serde_json::to_value(&listed).unwrap()),
    ] {
        let blob = wire.to_string();
        for key in ["phone", "salary", "bank_account", "rrn"] {
            assert!(
                !blob.contains(key),
                "{label} PersonHead must not surface {key}; got {blob}"
            );
        }
        assert!(
            !blob.contains("+82-10-1234-5678"),
            "{label} PersonHead must not surface the stored phone value; got {blob}"
        );
        let object = if label == "get" {
            wire.as_object().expect("get PersonHead is a JSON object")
        } else {
            wire.as_array()
                .and_then(|rows| rows.first())
                .and_then(serde_json::Value::as_object)
                .expect("list PersonHead is a JSON object")
        };
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["display_name", "id", "legal_name", "version"],
            "{label} PersonHead field set must stay the closed four-field projection; got {blob}"
        );
    }
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_second_binding_for_the_same_employee_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let employee = seed_employee(&owner_pool, ORG, "dup-1").await;

    let first = execute(&port, command(org, actor, create(Some(employee), "최지우")))
        .await
        .unwrap();
    let person_id = person_of(&first);
    assert_eq!(person_id, employee);

    // A second Create with the same employee_id fails closed at persons_pkey
    // under person_id = employee_id. Prove the BINDING uniqueness independently:
    // an unbound person revised to claim the already-bound employee is refused
    // by employee_person_bindings_pkey.
    let other = execute(&port, command(org, actor, create(None, "동명이인")))
        .await
        .unwrap();
    let other_person = person_of(&other);

    let refused = execute(
        &port,
        command(org, actor, revise(other_person, Some(employee), "동명이인")),
    )
    .await
    .unwrap_err();
    let (code, message) = database_error(&refused);
    assert_eq!(code, "23505", "got {message}");
    assert!(
        message.contains("employee_person_bindings_pkey"),
        "the primary key (org_id, employee_id) must be what refuses it; got {message}"
    );

    // The refused command rolled back whole: one binding, no orphan rebind.
    let bound: Vec<Uuid> = sqlx::query_scalar(
        "SELECT person_id FROM employee_person_bindings \
         WHERE org_id = $1 AND employee_id = $2",
    )
    .bind(ORG)
    .bind(employee)
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(bound, vec![person_id]);

    let persons: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM persons WHERE org_id = $1")
        .bind(ORG)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert_eq!(
        persons, 2,
        "the refused revise must leave both persons; the second create already committed"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn two_employees_may_bind_to_the_same_person(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let first_employee = seed_employee(&owner_pool, ORG, "two-1").await;
    let second_employee = seed_employee(&owner_pool, ORG, "two-2").await;

    let created = execute(
        &port,
        command(org, actor, create(Some(first_employee), "한지민")),
    )
    .await
    .unwrap();
    let person_id = person_of(&created);

    // ACCEPTED on purpose: one natural person holding two employment records is
    // exactly the case the distinct-natural-person four-eyes bar must detect.
    let bound = execute(
        &port,
        command(
            org,
            actor,
            revise(person_id, Some(second_employee), "한지민"),
        ),
    )
    .await
    .unwrap();
    assert_eq!(bound.result()["version"].as_i64(), Some(2));

    let employees: Vec<Uuid> = sqlx::query_scalar(
        "SELECT employee_id FROM employee_person_bindings \
         WHERE org_id = $1 AND person_id = $2 ORDER BY employee_id",
    )
    .bind(ORG)
    .bind(person_id)
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    let mut expected = vec![first_employee, second_employee];
    expected.sort();
    assert_eq!(employees, expected);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_actor_from_another_org_is_refused(owner_pool: PgPool) {
    let (org, _actor, port) = fixture(&owner_pool).await;
    let foreign_actor = seed_org_and_super_admin(&owner_pool, FOREIGN_ORG, "foreign").await;

    let refused = execute(
        &port,
        command(org, foreign_actor, create(None, "타사 행위자")),
    )
    .await
    .unwrap_err();
    let (code, message) = database_error(&refused);
    assert_eq!(code, "23503", "got {message}");
    assert!(
        message.contains("person_revisions"),
        "the (actor_id, org_id) foreign key on person_revisions must refuse it; got {message}"
    );

    let persons: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM persons WHERE org_id = $1")
        .bind(ORG)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert_eq!(persons, 0, "a refused command must persist no person");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_repeat_of_the_same_command_replays_the_stored_receipt(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());

    // The same payload, built in the two key orders a client and a re-encoding
    // proxy may each produce. `serde_json` resolves with `preserve_order` in
    // this workspace, so these two objects compare EQUAL while serialising to
    // different bytes — the digest must not be able to tell them apart, or the
    // retry after a timeout is refused instead of replayed.
    let first_attributes = json!({ "legal_name": "김철수", "dept": "eng" });
    let retry_attributes = json!({ "dept": "eng", "legal_name": "김철수" });
    assert_eq!(
        first_attributes, retry_attributes,
        "the two payloads must be the same command for this test to mean anything"
    );

    let first = execute(
        &port,
        PersonCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: PersonQuery::Create {
                employee_id: None,
                attributes: first_attributes,
            },
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();

    let replayed = execute(
        &port,
        PersonCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: PersonQuery::Create {
                employee_id: None,
                attributes: retry_attributes,
            },
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
    assert_eq!(count_rows(&owner_pool, COUNT_PERSONS).await, 1);
    assert_eq!(
        count_rows(&owner_pool, COUNT_REVISIONS).await,
        1,
        "a replayed command must append no revision"
    );
    assert_eq!(count_rows(&owner_pool, COUNT_RECEIPTS).await, 1);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_repeat_with_a_different_payload_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());

    execute(
        &port,
        PersonCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: create(None, "이영희"),
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();

    let refused = execute(
        &port,
        PersonCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: create(None, "위조"),
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap_err();

    assert!(
        matches!(refused, PersonError::DigestConflict(id) if id == *command_id.as_uuid()),
        "got {refused:?}"
    );
    assert_eq!(
        refused.to_string(),
        format!(
            "command {} was already applied with a different payload",
            command_id.as_uuid()
        )
    );
    assert_eq!(
        count_rows(&owner_pool, COUNT_REVISIONS).await,
        1,
        "a refused repeat must append no revision"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_command_id_already_held_by_another_receipt_owner_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());

    // An `ontology.action` receipt under the same tenant-global command id.
    // `CommandId`'s contract is that an id "cannot be replayed under a second
    // owner", so this port must refuse rather than accept a second command.
    sqlx::query(
        "INSERT INTO ont_action_command_receipts \
         (org_id, command_id, actor_id, payload_digest, receipt, created_at) \
         VALUES ($1, $2, $3, $4, $5, now())",
    )
    .bind(ORG)
    .bind(*command_id.as_uuid())
    .bind(*actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .bind(json!({ "kind": "ontology.action" }))
    .execute(&owner_pool)
    .await
    .unwrap();

    let refused = execute(
        &port,
        PersonCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: create(None, "중복 키"),
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap_err();

    assert!(
        matches!(refused, PersonError::DigestConflict(id) if id == *command_id.as_uuid()),
        "got {refused:?}"
    );
    assert_eq!(
        count_rows(&owner_pool, COUNT_PERSONS).await,
        0,
        "a command id already spent under another owner must persist no person"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_blocked_preflight_never_reaches_the_database(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;

    let blocked = execute(
        &port,
        command(org, actor, revise(Uuid::nil(), None, "무명")),
    )
    .await
    .unwrap_err();
    let PersonError::Blocked(blockers) = &blocked else {
        panic!("expected a blocked preflight, got {blocked:?}");
    };
    assert_eq!(blockers, &["person_id must not be nil".to_owned()]);
    assert_eq!(count_rows(&owner_pool, COUNT_REVISIONS).await, 0);
    assert_eq!(count_rows(&owner_pool, COUNT_RECEIPTS).await, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_stored_receipt_naming_no_dispatch_target_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());
    let query = create(None, "판독 불가");
    let accepted = execute(
        &port,
        PersonCommand {
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
    .bind(json!({ "person_id": "9e500000-0000-0000-0000-0000000000ff", "version": 1 }))
    .execute(&owner_pool)
    .await
    .unwrap();

    let refused = execute(
        &port,
        PersonCommand {
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
        matches!(refused, PersonError::UnreadableReceipt(id, _) if id == *command_id.as_uuid()),
        "a receipt the roster cannot read must be refused, never replayed; got {refused:?}"
    );
}
