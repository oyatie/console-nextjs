#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `OrgUnitPort` proven against a REAL PostgreSQL as the genuine runtime role
//! `console_rt` — never the BYPASSRLS superuser the `#[sqlx::test]` pool
//! connects as, which would mask a broken `org_isolation` policy.
//! `a_foreign_tenant_is_invisible_and_unwritable_to_the_runtime_role` is what
//! makes that claim observable, and it is deliberately NON-VACUOUS: every
//! foreign row is counted through the owner pool FIRST, so "console_rt sees
//! zero" is a policy doing work rather than an empty table.
//!
//! Shaped after `person_port_as_runtime_role.rs`, for the reasons that file
//! records: `#[sqlx::test]` is the only applier migration 0196 admits,
//! `execute` is driven from `spawn_blocking` because `CanonicalPort::execute`
//! is synchronous while `sqlx` is async-only, and immutability is the TRIGGER
//! and only the trigger — so the UPDATE test GRANTs `console_rt` the privilege
//! the deployed ACL already gives it before asserting the `P0001`.
//!
//! THE SEAM. `org_unit_source_bindings` is where a legacy record is bound to a
//! canonical org unit. It REFUSES UPDATE and PERMITS DELETE, so a rebind is an
//! explicit DELETE then INSERT and erasure stays available;
//! `a_source_binding_is_unique_and_rebound_only_by_delete_then_insert` proves
//! both halves. `regions` and `branches` are the branch-scoped AUTHORIZATION
//! spine, are named by neither the contract nor this suite, and a branch bound
//! to a unit is a ROW HERE, never a write there.

use console_kernel_core::{OrgId, UserId};
use console_ontology_canonical_adapter_postgres::company::{
    CompanyCommand, CompanyHead, CompanyQuery, PgCompanyPort,
};
use console_ontology_canonical_adapter_postgres::job_position::{
    JobPositionCommand, JobPositionQuery, JobPositionView, PgJobPositionPort,
};
use console_ontology_canonical_adapter_postgres::org_unit::{
    OrgUnitCommand, OrgUnitError, OrgUnitHead, OrgUnitQuery, PgOrgUnitPort, SourceBinding,
};
use console_ontology_canonical_domain::{
    CanonicalPort, CommandId, CommandReceipt, DispatchTarget, ObjectKey, OrgUnitPort, ReceiptOwner,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0x0c50_0000_0000_0000_0000_0000_0000_0001);
const FOREIGN_ORG: Uuid = Uuid::from_u128(0x0c50_0000_0000_0000_0000_0000_0000_0002);

/// The port must satisfy the NAMED trait, not merely `CanonicalPort`. The
/// blanket impl in `canonical-domain` makes `OrgUnitPort` an alias for
/// `CanonicalPort<Object = OrgUnit>`, so this bound stops holding the moment the
/// adapter is retargeted at a different object.
fn assert_implements_org_unit_port<P: OrgUnitPort>() {}

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
async fn fixture(owner_pool: &PgPool) -> (OrgId, UserId, PgOrgUnitPort) {
    let actor = seed_org_and_super_admin(owner_pool, ORG, "org-unit").await;
    let runtime_pool = runtime_role_pool(owner_pool).await;
    let port = PgOrgUnitPort::new(runtime_pool, tokio::runtime::Handle::current());
    (OrgId::from_uuid(ORG), actor, port)
}

/// Drive the SYNCHRONOUS `execute` off the runtime's worker thread. See the
/// module doc: this is the whole reason the suite is shaped this way.
async fn execute(
    port: &PgOrgUnitPort,
    command: OrgUnitCommand,
) -> Result<CommandReceipt, OrgUnitError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.execute(&command))
        .await
        .unwrap()
}

async fn get(
    port: &PgOrgUnitPort,
    org: OrgId,
    org_unit_id: Uuid,
) -> Result<Option<OrgUnitHead>, OrgUnitError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.get(org, org_unit_id))
        .await
        .unwrap()
}

async fn list(port: &PgOrgUnitPort, org: OrgId) -> Result<Vec<OrgUnitHead>, OrgUnitError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.list(org))
        .await
        .unwrap()
}

const COUNT_UNITS: &str = "SELECT count(*)::bigint FROM org_units";
const COUNT_REVISIONS: &str = "SELECT count(*)::bigint FROM org_unit_revisions";
const COUNT_BINDINGS: &str = "SELECT count(*)::bigint FROM org_unit_source_bindings";
const COUNT_RECEIPTS: &str = "SELECT count(*)::bigint FROM ont_action_command_receipts";

/// The three tables the contract assigns to `OrgUnit`, each with the count over
/// it. `sqlx` 0.9 accepts only `&'static str` SQL, so the table name cannot be
/// interpolated and every statement here is a literal.
const OWNED_TABLES: [(&str, &str); 3] = [
    ("org_units", COUNT_UNITS),
    ("org_unit_revisions", COUNT_REVISIONS),
    ("org_unit_source_bindings", COUNT_BINDINGS),
];

/// Rows counted through the BYPASSRLS owner pool, so a test can see what a
/// tenant-armed session must not.
async fn count_rows(owner_pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(owner_pool).await.unwrap()
}

/// The SQLSTATE and the message PostgreSQL actually returned, so a test quotes
/// the real error rather than a paraphrase of it.
fn database_error(error: &OrgUnitError) -> (String, String) {
    let OrgUnitError::Database(sqlx_error) = error else {
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

fn source(kind: &str, id: &str) -> SourceBinding {
    SourceBinding {
        kind: kind.to_owned(),
        id: id.to_owned(),
    }
}

fn create(source: Option<SourceBinding>, name: &str) -> OrgUnitQuery {
    OrgUnitQuery::Create {
        source,
        attributes: json!({ "name": name, "kind": "site" }),
    }
}

fn revise(org_unit_id: Uuid, source: Option<SourceBinding>, name: &str) -> OrgUnitQuery {
    OrgUnitQuery::Revise {
        org_unit_id,
        source,
        attributes: json!({ "name": name, "kind": "site" }),
    }
}

fn command(org: OrgId, actor: UserId, query: OrgUnitQuery) -> OrgUnitCommand {
    OrgUnitCommand {
        org_id: org,
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        actor_id: actor,
        query,
        action_key: "revise".to_owned(),
        object_type_id: Uuid::nil(),
    }
}

fn unit_of(receipt: &CommandReceipt) -> Uuid {
    receipt.result()["org_unit_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

/// The whole revision row as JSONB, so "unchanged" means the whole row and not
/// the two columns a test remembered to name.
async fn revision_snapshot(owner_pool: &PgPool, unit: Uuid, version: i64) -> String {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT to_jsonb(r) FROM org_unit_revisions r \
         WHERE org_id = $1 AND org_unit_id = $2 AND version = $3",
    )
    .bind(ORG)
    .bind(unit)
    .bind(version)
    .fetch_one(owner_pool)
    .await
    .unwrap()
    .to_string()
}

#[test]
fn the_contract_identity_is_copied_verbatim_and_the_port_is_the_named_one() {
    assert_implements_org_unit_port::<PgOrgUnitPort>();
    assert_eq!(ObjectKey::OrgUnit.as_str(), "org_unit");
    assert_eq!(
        ObjectKey::OrgUnit.owned_tables(),
        [
            "org_units",
            "org_unit_revisions",
            "org_unit_source_bindings"
        ]
    );
    assert_eq!(
        ObjectKey::OrgUnit.owner_crate(),
        "console-ontology-canonical-adapter-postgres"
    );
    assert_eq!(
        DispatchTarget::OrganizationCreateOrgUnit.as_str(),
        "organization.create_org_unit"
    );
    assert_eq!(
        DispatchTarget::OrganizationReviseOrgUnit.as_str(),
        "organization.revise_org_unit"
    );
    assert_eq!(
        DispatchTarget::OrganizationCreateOrgUnit.object(),
        ObjectKey::OrgUnit
    );
    assert_eq!(
        DispatchTarget::OrganizationReviseOrgUnit.object(),
        ObjectKey::OrgUnit
    );
}

#[test]
fn preflight_is_pure_and_blocks_what_the_database_would_only_catch_later() {
    let not_an_object = OrgUnitQuery::Create {
        source: None,
        attributes: json!("not an object"),
    };
    let preflight = <PgOrgUnitPort as CanonicalPort>::preflight(&not_an_object);
    assert!(!preflight.is_ok());
    assert_eq!(
        preflight.blockers(),
        ["attributes must be a JSON object".to_owned()]
    );

    let nil_target =
        <PgOrgUnitPort as CanonicalPort>::preflight(&revise(Uuid::nil(), None, "무명"));
    assert!(!nil_target.is_ok());
    assert_eq!(
        nil_target.blockers(),
        ["org_unit_id must not be nil".to_owned()]
    );

    // The CHECKs `source_kind <> ''` and `source_id <> ''` restated purely, so a
    // caller learns both at once instead of one round trip at a time.
    let missing_name = OrgUnitQuery::Create {
        source: None,
        attributes: json!({}),
    };
    let missing = <PgOrgUnitPort as CanonicalPort>::preflight(&missing_name);
    assert!(!missing.is_ok());
    assert_eq!(
        missing.blockers(),
        ["name is required".to_owned(), "kind is required".to_owned()]
    );

    let blank_name = OrgUnitQuery::Create {
        source: None,
        attributes: json!({ "name": "  ", "kind": "site" }),
    };
    let blank = <PgOrgUnitPort as CanonicalPort>::preflight(&blank_name);
    assert!(!blank.is_ok());
    assert_eq!(blank.blockers(), ["name must not be empty".to_owned()]);

    let empty_source =
        <PgOrgUnitPort as CanonicalPort>::preflight(&create(Some(source("", "")), "영업본부"));
    assert!(!empty_source.is_ok());
    assert_eq!(
        empty_source.blockers(),
        [
            "source_kind must not be empty".to_owned(),
            "source_id must not be empty".to_owned()
        ]
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_org_unit_is_created_and_read_back(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let query = create(Some(source("legacy_branch", "BR-001")), "영업본부");
    assert!(<PgOrgUnitPort as CanonicalPort>::preflight(&query).is_ok());

    let receipt = execute(&port, command(org, actor, query)).await.unwrap();

    assert_eq!(receipt.org_id(), org);
    assert_eq!(receipt.actor_id(), actor);
    assert_eq!(
        receipt.owner(),
        ReceiptOwner::Canonical(ObjectKey::OrgUnit),
        "the receipt must be owned by the canonical OrgUnit object"
    );
    assert_eq!(receipt.target(), DispatchTarget::OrganizationCreateOrgUnit);
    assert_eq!(receipt.result()["version"].as_i64(), Some(1));

    let unit = unit_of(&receipt);
    let row = sqlx::query(
        "SELECT r.version, r.attributes, r.command_id, r.payload_digest \
         FROM org_units u JOIN org_unit_revisions r \
           ON r.org_id = u.org_id AND r.org_unit_id = u.id \
         WHERE u.org_id = $1 AND u.id = $2",
    )
    .bind(ORG)
    .bind(unit)
    .fetch_one(&owner_pool)
    .await
    .unwrap();

    assert_eq!(row.get::<i64, _>("version"), 1);
    assert_eq!(
        row.get::<serde_json::Value, _>("attributes"),
        json!({ "name": "영업본부", "kind": "site" })
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

    // The seam: a legacy record is bound HERE, never by writing `branches`.
    let bound: Uuid = sqlx::query_scalar(
        "SELECT org_unit_id FROM org_unit_source_bindings \
         WHERE org_id = $1 AND source_kind = $2 AND source_id = $3",
    )
    .bind(ORG)
    .bind("legacy_branch")
    .bind("BR-001")
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(bound, unit);

    let head = get(&port, org, unit)
        .await
        .unwrap()
        .expect("created OrgUnit must be queryable");
    assert_eq!(head.id, unit);
    assert_eq!(head.name.as_deref(), Some("영업본부"));
    assert_eq!(
        head.parent_id, None,
        "parent_id is not a column and was not stored in attributes"
    );
    assert_eq!(head.version, 1);
    assert_eq!(list(&port, org).await.unwrap(), vec![head]);
    let unknown = get(&port, org, Uuid::new_v4()).await;
    assert!(
        matches!(unknown, Ok(None)),
        "an unknown id must be Ok(None) on the runtime-role pool, never a distinct error; got {unknown:?}"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_revision_is_appended_and_the_prior_revision_is_unchanged(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let created = execute(&port, command(org, actor, create(None, "영업본부")))
        .await
        .unwrap();
    let unit = unit_of(&created);

    let before = revision_snapshot(&owner_pool, unit, 1).await;

    let revised = execute(&port, command(org, actor, revise(unit, None, "영업1본부")))
        .await
        .unwrap();
    assert_eq!(revised.target(), DispatchTarget::OrganizationReviseOrgUnit);
    assert_eq!(revised.result()["version"].as_i64(), Some(2));

    let head = get(&port, org, unit).await.unwrap().expect("latest head");
    assert_eq!(head.version, 2);
    assert_eq!(head.name.as_deref(), Some("영업1본부"));

    let after = revision_snapshot(&owner_pool, unit, 1).await;
    assert_eq!(before, after, "appending a revision rewrote revision 1");

    let versions: Vec<i64> = sqlx::query_scalar(
        "SELECT version FROM org_unit_revisions \
         WHERE org_id = $1 AND org_unit_id = $2 ORDER BY version",
    )
    .bind(ORG)
    .bind(unit)
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(versions, vec![1, 2]);
    assert_eq!(
        count_rows(&owner_pool, COUNT_UNITS).await,
        1,
        "a revision must not mint a second identity"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_update_of_a_revision_row_is_refused_by_the_trigger(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let created = execute(&port, command(org, actor, create(None, "생산본부")))
        .await
        .unwrap();
    let unit = unit_of(&created);
    let before = revision_snapshot(&owner_pool, unit, 1).await;

    // Reproduce the DEPLOYED ACL before asserting anything about it: production
    // applies migrations as `console_app`, after `ALTER DEFAULT PRIVILEGES ...
    // GRANT ... UPDATE ... TO console_rt`, so the runtime role holds UPDATE
    // there. `#[sqlx::test]` applies them as the superuser, for whom that
    // default privilege does not exist, and 0215 itself grants only
    // SELECT, INSERT on this table.
    sqlx::query("GRANT UPDATE ON org_unit_revisions TO console_rt")
        .execute(&owner_pool)
        .await
        .unwrap();
    let runtime_holds_update: bool = sqlx::query_scalar(
        "SELECT has_table_privilege('console_rt', 'org_unit_revisions', 'UPDATE')",
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
        "UPDATE org_unit_revisions SET attributes = '{\"name\":\"forged\"}'::jsonb \
         WHERE org_id = $1 AND org_unit_id = $2",
    )
    .bind(ORG)
    .bind(unit)
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
        "canonical org-structure table org_unit_revisions: UPDATE is refused, the row is immutable"
    );
    drop(tx);

    // The owner is refused by the same trigger; ownership buys no exemption.
    let as_owner =
        sqlx::query("UPDATE org_unit_revisions SET version = version + 100 WHERE org_id = $1")
            .bind(ORG)
            .execute(&owner_pool)
            .await
            .unwrap_err();
    let owner_error = as_owner.as_database_error().unwrap();
    assert_eq!(owner_error.code().unwrap(), "P0001");
    assert_eq!(
        owner_error.message(),
        "canonical org-structure table org_unit_revisions: UPDATE is refused, the row is immutable"
    );

    // DELETE is refused too, so history cannot be quietly shortened.
    let deleted = sqlx::query("DELETE FROM org_unit_revisions WHERE org_id = $1")
        .bind(ORG)
        .execute(&owner_pool)
        .await
        .unwrap_err();
    let delete_error = deleted.as_database_error().unwrap();
    assert_eq!(delete_error.code().unwrap(), "P0001");
    assert_eq!(
        delete_error.message(),
        "canonical org-structure table org_unit_revisions: DELETE is refused, the row is immutable"
    );

    let after = revision_snapshot(&owner_pool, unit, 1).await;
    assert_eq!(before, after, "a refused UPDATE rewrote the revision row");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_foreign_tenant_is_invisible_and_unwritable_to_the_runtime_role(owner_pool: PgPool) {
    let (org, _actor, port) = fixture(&owner_pool).await;
    let foreign_actor = seed_org_and_super_admin(&owner_pool, FOREIGN_ORG, "foreign").await;

    // A unit, a revision and a binding that genuinely exist — under the OTHER
    // tenant. Seeded through the BYPASSRLS owner pool, which is the only way to
    // put rows on the far side of the boundary being tested.
    let foreign_unit: Uuid =
        sqlx::query_scalar("INSERT INTO org_units (org_id) VALUES ($1) RETURNING id")
            .bind(FOREIGN_ORG)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO org_unit_revisions \
         (org_id, org_unit_id, version, command_id, actor_id, payload_digest, attributes, receipt) \
         VALUES ($1, $2, 1, gen_random_uuid(), $3, $4, '{}'::jsonb, '{}'::jsonb)",
    )
    .bind(FOREIGN_ORG)
    .bind(foreign_unit)
    .bind(*foreign_actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .execute(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO org_unit_source_bindings \
         (org_id, source_kind, source_id, org_unit_id, actor_id, payload_digest) \
         VALUES ($1, 'legacy_branch', 'FOREIGN-1', $2, $3, $4)",
    )
    .bind(FOREIGN_ORG)
    .bind(foreign_unit)
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
    let refused = sqlx::query("INSERT INTO org_units (org_id) VALUES ($1)")
        .bind(FOREIGN_ORG)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    let error = refused.as_database_error().unwrap();
    assert_eq!(error.code().unwrap(), "42501", "got {}", error.message());
    assert_eq!(
        error.message(),
        "new row violates row-level security policy for table \"org_units\""
    );
    drop(tx);

    assert!(list(&port, org).await.unwrap().is_empty());
    let foreign_head = get(&port, org, foreign_unit).await;
    assert!(
        matches!(foreign_head, Ok(None)),
        "foreign OrgUnit id is Ok(None) on the runtime-role pool, never a wrong-tenant error; got {foreign_head:?}"
    );
    let unknown = get(&port, org, Uuid::new_v4()).await;
    assert!(
        matches!(unknown, Ok(None)),
        "unknown OrgUnit id is indistinguishable from a foreign tenant: Ok(None); got {unknown:?}"
    );
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
        message.contains("org_unit_revisions"),
        "the (actor_id, org_id) foreign key on org_unit_revisions must refuse it; got {message}"
    );

    assert_eq!(
        count_rows(&owner_pool, COUNT_UNITS).await,
        0,
        "a refused command must persist no org unit"
    );
    assert_eq!(count_rows(&owner_pool, COUNT_RECEIPTS).await, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_source_binding_is_unique_and_rebound_only_by_delete_then_insert(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let first = execute(
        &port,
        command(
            org,
            actor,
            create(Some(source("legacy_branch", "BR-7")), "구미지점"),
        ),
    )
    .await
    .unwrap();
    let first_unit = unit_of(&first);

    // ONE legacy record resolves to at most ONE canonical unit, made
    // unrepresentable by PRIMARY KEY (org_id, source_kind, source_id).
    let refused = execute(
        &port,
        command(
            org,
            actor,
            create(Some(source("legacy_branch", "BR-7")), "중복지점"),
        ),
    )
    .await
    .unwrap_err();
    let (code, message) = database_error(&refused);
    assert_eq!(code, "23505", "got {message}");
    assert!(
        message.contains("org_unit_source_bindings_pkey"),
        "the primary key must be what refuses it; got {message}"
    );
    assert_eq!(
        count_rows(&owner_pool, COUNT_UNITS).await,
        1,
        "the refused command must persist no second org unit"
    );

    // The reverse direction is deliberately NOT unique: one canonical unit
    // legitimately absorbs several legacy records.
    execute(
        &port,
        command(
            org,
            actor,
            revise(first_unit, Some(source("legacy_dept", "D-9")), "구미지점"),
        ),
    )
    .await
    .unwrap();
    assert_eq!(count_rows(&owner_pool, COUNT_BINDINGS).await, 2);

    // Re-pointing a binding by editing a column is what an audit must not
    // tolerate, so UPDATE is refused — by the TRIGGER, not by the grant.
    // 0215 grants `console_rt` only SELECT, INSERT, DELETE here, while the
    // deployed database also gives it UPDATE through
    // `ALTER DEFAULT PRIVILEGES FOR ROLE console_app ... GRANT ... UPDATE`, run
    // before migrations. Reproduce that ACL, or the assertion below observes a
    // 42501 that does not exist where this code ships.
    sqlx::query("GRANT UPDATE ON org_unit_source_bindings TO console_rt")
        .execute(&owner_pool)
        .await
        .unwrap();
    let runtime_holds_update: bool = sqlx::query_scalar(
        "SELECT has_table_privilege('console_rt', 'org_unit_source_bindings', 'UPDATE')",
    )
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert!(runtime_holds_update, "the production ACL must be in place");

    let runtime_pool = runtime_role_pool(&owner_pool).await;
    let mut tx = runtime_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let repointed = sqlx::query(
        "UPDATE org_unit_source_bindings SET org_unit_id = $1 \
         WHERE org_id = $2 AND source_kind = 'legacy_branch' AND source_id = 'BR-7'",
    )
    .bind(Uuid::new_v4())
    .bind(ORG)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    let repoint_error = repointed.as_database_error().unwrap();
    assert_eq!(
        repoint_error.code().unwrap(),
        "P0001",
        "got {}",
        repoint_error.message()
    );
    assert_eq!(
        repoint_error.message(),
        "canonical org-structure table org_unit_source_bindings: UPDATE is refused, the row is \
         immutable"
    );
    // That error ABORTED this transaction, so the DELETE below needs a fresh
    // one — a second statement on an aborted transaction fails with 25P02 and
    // would prove nothing about the grant.
    drop(tx);

    // … while DELETE stays available, which is what keeps a rebind explicit and
    // erasure possible. `console_rt` holds DELETE on this table by 0215.
    let mut erasing = runtime_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *erasing)
        .await
        .unwrap();
    let erased = sqlx::query(
        "DELETE FROM org_unit_source_bindings \
         WHERE org_id = $1 AND source_kind = 'legacy_branch' AND source_id = 'BR-7'",
    )
    .bind(ORG)
    .execute(&mut *erasing)
    .await
    .unwrap();
    assert_eq!(erased.rows_affected(), 1);
    erasing.commit().await.unwrap();
    assert_eq!(count_rows(&owner_pool, COUNT_BINDINGS).await, 1);
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
    let first_attributes = json!({ "name": "영업본부", "code": "SALES", "kind": "site" });
    let retry_attributes = json!({ "code": "SALES", "kind": "site", "name": "영업본부" });
    assert_eq!(
        first_attributes, retry_attributes,
        "the two payloads must be the same command for this test to mean anything"
    );

    let first = execute(
        &port,
        OrgUnitCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: OrgUnitQuery::Create {
                source: Some(source("legacy_branch", "BR-2")),
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
        OrgUnitCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: OrgUnitQuery::Create {
                source: Some(source("legacy_branch", "BR-2")),
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
    assert_eq!(count_rows(&owner_pool, COUNT_UNITS).await, 1);
    assert_eq!(
        count_rows(&owner_pool, COUNT_REVISIONS).await,
        1,
        "a replayed command must append no revision"
    );
    assert_eq!(
        count_rows(&owner_pool, COUNT_BINDINGS).await,
        1,
        "a replayed command must append no binding"
    );
    assert_eq!(count_rows(&owner_pool, COUNT_RECEIPTS).await, 1);
}

/// A re-split of the SAME source bytes must NOT collide.
///
/// `hasher.update(kind); hasher.update(id)` is ambiguous: ("hris", "emp-1") and
/// ("hrise", "mp-1") concatenate to identical bytes, so the digest matched and the
/// port replayed the FIRST command's receipt — reporting success for a source
/// binding it never wrote, which a reconciler then records as synced. RED before
/// the fields were length-prefixed. `employment.rs` already hashed this way; this
/// was the one canonical port that did not.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_resplit_source_binding_is_not_the_same_digest(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());

    execute(
        &port,
        OrgUnitCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: create(Some(source("hris", "emp-1")), "영업본부"),
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .expect("first create");

    // Same command_id, same concatenated bytes, DIFFERENT split.
    let refused = execute(
        &port,
        OrgUnitCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: create(Some(source("hrise", "mp-1")), "영업본부"),
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .expect_err("a different source split must not replay the first receipt");

    assert!(
        matches!(refused, OrgUnitError::DigestConflict(id) if id == *command_id.as_uuid()),
        "got {refused:?}"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_repeat_with_a_different_payload_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());

    execute(
        &port,
        OrgUnitCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: create(None, "영업본부"),
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();

    let refused = execute(
        &port,
        OrgUnitCommand {
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
        matches!(refused, OrgUnitError::DigestConflict(id) if id == *command_id.as_uuid()),
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
async fn a_blocked_preflight_never_reaches_the_database(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;

    let blocked = execute(
        &port,
        command(org, actor, revise(Uuid::nil(), None, "무명")),
    )
    .await
    .unwrap_err();
    let OrgUnitError::Blocked(blockers) = &blocked else {
        panic!("expected a blocked preflight, got {blocked:?}");
    };
    assert_eq!(blockers, &["org_unit_id must not be nil".to_owned()]);
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
        OrgUnitCommand {
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
    .bind(json!({ "org_unit_id": "0c500000-0000-0000-0000-0000000000ff", "version": 1 }))
    .execute(&owner_pool)
    .await
    .unwrap();

    let refused = execute(
        &port,
        OrgUnitCommand {
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
        matches!(refused, OrgUnitError::UnreadableReceipt(id, _) if id == *command_id.as_uuid()),
        "a receipt the roster cannot read must be refused, never replayed; got {refused:?}"
    );
}

/// The Company/OrgUnit reference ROADMAP item 5 names: from a provisioned
/// empty tenant, as `console_rt`, produce the OrgUnit and JobPosition UUIDs
/// `hr.appoint` already consumes — without a developer INSERT into `org_units`.
///
/// Not a tree: 0215 refuses `parent_id` on the identity anchor. Not Group /
/// 다법인: PRODUCT is one Company per tenant. Frontend stays HOLD.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn empty_tenant_company_org_unit_and_job_position_round_trip(owner_pool: PgPool) {
    let actor = seed_org_and_super_admin(&owner_pool, ORG, "org-unit").await;
    let runtime_pool = runtime_role_pool(&owner_pool).await;
    let handle = tokio::runtime::Handle::current();
    let company = PgCompanyPort::new(runtime_pool.clone(), handle.clone());
    let units = PgOrgUnitPort::new(runtime_pool.clone(), handle.clone());
    let positions = PgJobPositionPort::new(runtime_pool, handle);
    let org = OrgId::from_uuid(ORG);

    let no_company = spawn_get_company(&company, org).await.unwrap();
    assert!(
        no_company.is_none(),
        "provisioning an organizations row must not fabricate a company head; got {no_company:?}"
    );
    assert!(
        list(&units, org).await.unwrap().is_empty(),
        "a provisioned tenant has no OrgUnits until organization.create_org_unit"
    );

    execute_company(
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
    let head = spawn_get_company(&company, org)
        .await
        .unwrap()
        .expect("company.revise must produce a queryable head");
    assert_eq!(head.legal_name.as_deref(), Some("주식회사 아크메"));
    assert_eq!(head.version, 1);

    let created = execute(
        &units,
        OrgUnitCommand {
            org_id: org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: create(None, "영업본부"),
            action_key: "create_org_unit".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();
    let unit = unit_of(&created);
    let unit_head = get(&units, org, unit)
        .await
        .unwrap()
        .expect("organization.create_org_unit must produce a queryable head");
    assert_eq!(unit_head.name.as_deref(), Some("영업본부"));
    assert_eq!(
        unit_head.parent_id, None,
        "0215: hierarchy is not a column; this command stored none in attributes"
    );
    assert_eq!(list(&units, org).await.unwrap(), vec![unit_head]);

    let position_receipt = execute_position(
        &positions,
        JobPositionCommand {
            org_id: org,
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            actor_id: actor,
            query: JobPositionQuery::Create {
                org_unit_id: unit,
                attributes: json!({ "title": "백엔드 엔지니어" }),
            },
            action_key: "organization.create_job_position".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();
    let position_id = position_receipt.result()["job_position_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let viewed = spawn_get_position(&positions, org, position_id)
        .await
        .unwrap()
        .expect("organization.create_job_position must produce a queryable head");
    assert_eq!(viewed.org_unit_id, unit);
    assert_eq!(viewed.attributes, json!({ "title": "백엔드 엔지니어" }));
    let listed = spawn_list_positions(&positions, org, unit).await.unwrap();
    assert_eq!(listed, vec![viewed]);
}

async fn execute_company(
    port: &PgCompanyPort,
    command: CompanyCommand,
) -> Result<CommandReceipt, console_ontology_canonical_adapter_postgres::company::CompanyError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.execute(&command))
        .await
        .unwrap()
}

async fn spawn_get_company(
    port: &PgCompanyPort,
    org: OrgId,
) -> Result<Option<CompanyHead>, console_ontology_canonical_adapter_postgres::company::CompanyError>
{
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.get(org))
        .await
        .unwrap()
}

async fn execute_position(
    port: &PgJobPositionPort,
    command: JobPositionCommand,
) -> Result<
    CommandReceipt,
    console_ontology_canonical_adapter_postgres::job_position::JobPositionError,
> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.execute(&command))
        .await
        .unwrap()
}

async fn spawn_get_position(
    port: &PgJobPositionPort,
    org: OrgId,
    job_position_id: Uuid,
) -> Result<
    Option<JobPositionView>,
    console_ontology_canonical_adapter_postgres::job_position::JobPositionError,
> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.get(org, job_position_id))
        .await
        .unwrap()
}

async fn spawn_list_positions(
    port: &PgJobPositionPort,
    org: OrgId,
    org_unit_id: Uuid,
) -> Result<
    Vec<JobPositionView>,
    console_ontology_canonical_adapter_postgres::job_position::JobPositionError,
> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.list_for_org_unit(org, org_unit_id))
        .await
        .unwrap()
}
