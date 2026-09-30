#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `CompanyPort` proven against a REAL PostgreSQL as the genuine runtime role
//! `console_rt` — never the BYPASSRLS superuser the `#[sqlx::test]` pool
//! connects as, which would mask a broken `org_isolation` policy.
//!
//! WHY THIS SUITE CARRIES AN EXTRA TEST THE `Person` ONE DOES NOT.
//! `Company` owns TWO tables and this port writes only ONE of them. That is not
//! an omission, and
//! `the_runtime_role_may_not_write_the_organizations_head` is here so the claim
//! is EXECUTED rather than argued: migration
//! `0031_runtime_role_and_immutable_org.sql` grants `console_rt` `SELECT` on
//! `organizations` and then `REVOKE INSERT, UPDATE, DELETE`s the write verbs
//! back, because "provisioning a tenant is an owner operation, so the runtime
//! role must never INSERT/UPDATE/DELETE org rows". A `CompanyPort` that issued
//! `UPDATE organizations` would be dead code in every deployed database. So
//! `organizations` stays the provisioning-owned head and `company_revisions`
//! carries the whole canonical company state, exactly as the contract says
//! ("no `companies` table is created").
//!
//! WHY `#[sqlx::test]` IS NOT OPTIONAL HERE, WHY `execute` IS CALLED FROM
//! `spawn_blocking`, and WHY THE IMMUTABILITY TEST GRANTS FIRST: identical to
//! `tests/person_port_as_runtime_role.rs`, whose module doc states each in
//! full. The one difference is the grant: 0215 gives `console_rt` only
//! `SELECT, INSERT` on `company_revisions`, while
//! `ops/postgres-reconcile-topology.sh` runs `ALTER DEFAULT PRIVILEGES ... GRANT
//! SELECT, INSERT, UPDATE, DELETE ON TABLES TO console_rt` BEFORE migrations, so
//! in the deployed database the runtime role DOES hold UPDATE and DELETE on it.
//! The test reproduces that ACL and asserts it is really held, so what refuses
//! the write is observably the TRIGGER and not a missing privilege.

use console_kernel_core::{OrgId, UserId};
use console_ontology_canonical_adapter_postgres::company::{
    CompanyCommand, CompanyError, CompanyHead, CompanyQuery, PgCompanyPort,
};
use console_ontology_canonical_domain::{
    CanonicalPort, CommandId, CommandReceipt, CompanyPort, DispatchTarget, ObjectKey, ReceiptOwner,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0xc000_0000_0000_0000_0000_0000_0000_0001);
const FOREIGN_ORG: Uuid = Uuid::from_u128(0xc000_0000_0000_0000_0000_0000_0000_0002);

/// The port must satisfy the NAMED trait, not merely `CanonicalPort`. The
/// blanket impl in `canonical-domain` makes `CompanyPort` an alias for
/// `CanonicalPort<Object = Company>`, so this bound stops holding the moment the
/// adapter is retargeted at a different object.
fn assert_implements_company_port<P: CompanyPort>() {}

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
/// same reason. Seeded as the migration owner, before any role switch — which
/// is also the only identity that MAY insert an `organizations` row.
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
async fn fixture(owner_pool: &PgPool) -> (OrgId, UserId, PgCompanyPort) {
    let actor = seed_org_and_super_admin(owner_pool, ORG, "company").await;
    let runtime_pool = runtime_role_pool(owner_pool).await;
    let port = PgCompanyPort::new(runtime_pool, tokio::runtime::Handle::current());
    (OrgId::from_uuid(ORG), actor, port)
}

/// Drive the SYNCHRONOUS `execute` off the runtime's worker thread.
async fn execute(
    port: &PgCompanyPort,
    command: CompanyCommand,
) -> Result<CommandReceipt, CompanyError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.execute(&command))
        .await
        .unwrap()
}

async fn get(port: &PgCompanyPort, org: OrgId) -> Result<Option<CompanyHead>, CompanyError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.get(org))
        .await
        .unwrap()
}

async fn list(port: &PgCompanyPort, org: OrgId) -> Result<Vec<CompanyHead>, CompanyError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.list(org))
        .await
        .unwrap()
}

const COUNT_REVISIONS: &str = "SELECT count(*)::bigint FROM company_revisions";
const COUNT_RECEIPTS: &str = "SELECT count(*)::bigint FROM ont_action_command_receipts";
/// Scoped to one id on purpose: migrations 0028 and 0036 seed `organizations`
/// rows of their own (`knl`, `platform`), so a global count over that table
/// asserts the seed data rather than the tenant boundary.
const COUNT_ONE_ORGANIZATION: &str = "SELECT count(*)::bigint FROM organizations WHERE id = $1";

/// Rows counted through the BYPASSRLS owner pool, so a test can see what a
/// tenant-armed session must not.
async fn count_rows(owner_pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(owner_pool).await.unwrap()
}

/// The SQLSTATE and the message PostgreSQL actually returned, so a test quotes
/// the real error rather than a paraphrase of it.
fn database_error(error: &CompanyError) -> (String, String) {
    let CompanyError::Database(sqlx_error) = error else {
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

fn revise(name: &str) -> CompanyQuery {
    CompanyQuery {
        attributes: json!({ "legal_name": name }),
    }
}

fn command(org: OrgId, actor: UserId, query: CompanyQuery) -> CompanyCommand {
    CompanyCommand {
        org_id: org,
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        actor_id: actor,
        query,
        action_key: "revise".to_owned(),
        object_type_id: Uuid::nil(),
    }
}

/// The whole revision row as JSONB, so "unchanged" means the whole row and not
/// the two columns a test remembered to name.
async fn revision_snapshot(owner_pool: &PgPool, version: i64) -> String {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT to_jsonb(r) FROM company_revisions r WHERE org_id = $1 AND version = $2",
    )
    .bind(ORG)
    .bind(version)
    .fetch_one(owner_pool)
    .await
    .unwrap()
    .to_string()
}

#[test]
fn the_contract_identity_is_copied_verbatim_and_the_port_is_the_named_one() {
    assert_implements_company_port::<PgCompanyPort>();
    assert_eq!(ObjectKey::Company.as_str(), "company");
    assert_eq!(
        ObjectKey::Company.owned_tables(),
        ["organizations", "company_revisions"]
    );
    assert_eq!(
        ObjectKey::Company.owner_crate(),
        "console-ontology-canonical-adapter-postgres"
    );
    assert_eq!(DispatchTarget::CompanyRevise.as_str(), "company.revise");
    assert_eq!(DispatchTarget::CompanyRevise.object(), ObjectKey::Company);
    // `Company` has exactly ONE dispatch target. If the roster ever grows a
    // second, this port's single-struct query is no longer the right shape.
    let company_targets: Vec<&str> = DispatchTarget::ALL
        .iter()
        .filter(|target| target.object() == ObjectKey::Company)
        .map(|target| target.as_str())
        .collect();
    assert_eq!(company_targets, ["company.revise"]);
}

#[test]
fn preflight_is_pure_and_blocks_a_non_object_attribute_payload() {
    let query = CompanyQuery {
        attributes: json!("not an object"),
    };
    let preflight = <PgCompanyPort as CanonicalPort>::preflight(&query);
    assert!(!preflight.is_ok());
    assert_eq!(
        preflight.blockers(),
        ["attributes must be a JSON object".to_owned()]
    );

    assert!(<PgCompanyPort as CanonicalPort>::preflight(&revise("주식회사 아크메")).is_ok());

    let missing = CompanyQuery {
        attributes: json!({}),
    };
    let missing_preflight = <PgCompanyPort as CanonicalPort>::preflight(&missing);
    assert!(!missing_preflight.is_ok());
    assert_eq!(
        missing_preflight.blockers(),
        ["legal_name is required".to_owned()]
    );

    let blank = CompanyQuery {
        attributes: json!({ "legal_name": "   " }),
    };
    let blank_preflight = <PgCompanyPort as CanonicalPort>::preflight(&blank);
    assert!(!blank_preflight.is_ok());
    assert_eq!(
        blank_preflight.blockers(),
        ["legal_name must not be empty".to_owned()]
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_company_revision_is_created_and_read_back(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;

    assert!(
        get(&port, org).await.unwrap().is_none(),
        "no revision is not a fabricated head from organizations.name"
    );
    assert!(list(&port, org).await.unwrap().is_empty());

    let receipt = execute(&port, command(org, actor, revise("주식회사 아크메")))
        .await
        .unwrap();

    assert_eq!(receipt.org_id(), org);
    assert_eq!(receipt.actor_id(), actor);
    assert_eq!(
        receipt.owner(),
        ReceiptOwner::Canonical(ObjectKey::Company),
        "the receipt must be owned by the canonical Company object"
    );
    assert_eq!(receipt.target(), DispatchTarget::CompanyRevise);
    assert_eq!(receipt.result()["version"].as_i64(), Some(1));

    let row = sqlx::query(
        "SELECT r.version, r.attributes, r.command_id, r.payload_digest, o.name \
         FROM company_revisions r JOIN organizations o ON o.id = r.org_id \
         WHERE r.org_id = $1",
    )
    .bind(ORG)
    .fetch_one(&owner_pool)
    .await
    .unwrap();

    assert_eq!(row.get::<i64, _>("version"), 1);
    assert_eq!(
        row.get::<serde_json::Value, _>("attributes"),
        json!({ "legal_name": "주식회사 아크메" })
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
    assert_eq!(
        row.get::<String, _>("name"),
        "Org company",
        "the port must leave the provisioning-owned `organizations` head alone"
    );

    let head = get(&port, org)
        .await
        .unwrap()
        .expect("created company revision must be queryable");
    assert_eq!(head.org_id, ORG);
    assert_eq!(head.legal_name.as_deref(), Some("주식회사 아크메"));
    assert_eq!(
        head.reg_no, None,
        "a field absent from stored attributes must be omitted, not invented"
    );
    assert_eq!(head.version, 1);
    assert_eq!(list(&port, org).await.unwrap(), vec![head]);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_revision_is_appended_and_the_prior_revision_is_unchanged(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    execute(&port, command(org, actor, revise("주식회사 아크메")))
        .await
        .unwrap();
    let before = revision_snapshot(&owner_pool, 1).await;

    let revised = execute(&port, command(org, actor, revise("아크메 주식회사")))
        .await
        .unwrap();
    assert_eq!(revised.target(), DispatchTarget::CompanyRevise);
    assert_eq!(revised.result()["version"].as_i64(), Some(2));

    let head = get(&port, org).await.unwrap().expect("latest head");
    assert_eq!(head.version, 2);
    assert_eq!(head.legal_name.as_deref(), Some("아크메 주식회사"));

    let after = revision_snapshot(&owner_pool, 1).await;
    assert_eq!(before, after, "appending a revision rewrote revision 1");

    let versions: Vec<i64> = sqlx::query_scalar(
        "SELECT version FROM company_revisions WHERE org_id = $1 ORDER BY version",
    )
    .bind(ORG)
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(versions, vec![1, 2]);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_update_or_delete_of_a_revision_row_is_refused_by_the_trigger(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    execute(&port, command(org, actor, revise("주식회사 아크메")))
        .await
        .unwrap();
    let before = revision_snapshot(&owner_pool, 1).await;

    // Reproduce the DEPLOYED ACL before asserting anything about it. See the
    // module doc: production applies migrations as `console_app`, after
    // `ALTER DEFAULT PRIVILEGES ... GRANT ... UPDATE, DELETE ... TO console_rt`.
    sqlx::query("GRANT UPDATE, DELETE ON company_revisions TO console_rt")
        .execute(&owner_pool)
        .await
        .unwrap();
    for verb in ["UPDATE", "DELETE"] {
        let held: bool =
            sqlx::query_scalar("SELECT has_table_privilege('console_rt', 'company_revisions', $1)")
                .bind(verb)
                .fetch_one(&owner_pool)
                .await
                .unwrap();
        assert!(
            held,
            "the production ACL must be in place, or the assertion below observes a privilege \
             error that does not exist where this code ships ({verb})"
        );
    }

    let runtime_pool = runtime_role_pool(&owner_pool).await;
    let mut tx = runtime_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();

    let updated = sqlx::query(
        "UPDATE company_revisions SET attributes = '{\"legal_name\":\"forged\"}'::jsonb \
         WHERE org_id = $1",
    )
    .bind(ORG)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    let update_error = updated.as_database_error().unwrap();
    assert_eq!(
        update_error.code().unwrap(),
        "P0001",
        "the trigger is the enforcement, not the grant; got {}",
        update_error.message()
    );
    assert_eq!(
        update_error.message(),
        "canonical org-structure table company_revisions: UPDATE is refused, the row is immutable"
    );
    drop(tx);

    // DELETE too: this table is the legal history of the tenant's own company
    // record, so erasure is not available where it is for a source binding.
    let mut tx = runtime_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let deleted = sqlx::query("DELETE FROM company_revisions WHERE org_id = $1")
        .bind(ORG)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    let delete_error = deleted.as_database_error().unwrap();
    assert_eq!(delete_error.code().unwrap(), "P0001");
    assert_eq!(
        delete_error.message(),
        "canonical org-structure table company_revisions: DELETE is refused, the row is immutable"
    );
    drop(tx);

    // The owner is refused by the same trigger; ownership buys no exemption.
    let as_owner = sqlx::query("UPDATE company_revisions SET version = version + 100")
        .execute(&owner_pool)
        .await
        .unwrap_err();
    assert_eq!(
        as_owner.as_database_error().unwrap().code().unwrap(),
        "P0001"
    );

    let after = revision_snapshot(&owner_pool, 1).await;
    assert_eq!(before, after, "a refused write rewrote the revision row");
}

/// The reason `CompanyPort` holds DML against exactly one of its two owned
/// tables. Not an argument — the privilege and the refusal, both measured.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn the_runtime_role_may_not_write_the_organizations_head(owner_pool: PgPool) {
    let (_org, _actor, _port) = fixture(&owner_pool).await;

    for verb in ["INSERT", "UPDATE", "DELETE"] {
        let held: bool =
            sqlx::query_scalar("SELECT has_table_privilege('console_rt', 'organizations', $1)")
                .bind(verb)
                .fetch_one(&owner_pool)
                .await
                .unwrap();
        assert!(
            !held,
            "0031 revokes {verb} on organizations from console_rt — provisioning a tenant is an \
             owner operation. A CompanyPort that wrote the head would be dead code in production."
        );
    }
    let held_select: bool =
        sqlx::query_scalar("SELECT has_table_privilege('console_rt', 'organizations', 'SELECT')")
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert!(held_select, "the head stays readable to the runtime role");

    let runtime_pool = runtime_role_pool(&owner_pool).await;
    let mut tx = runtime_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let refused = sqlx::query("UPDATE organizations SET name = 'forged' WHERE id = $1")
        .bind(ORG)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    let error = refused.as_database_error().unwrap();
    assert_eq!(error.code().unwrap(), "42501", "got {}", error.message());
    assert_eq!(
        error.message(),
        "permission denied for table organizations",
        "the refusal is the grant layer, beneath RLS"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_foreign_tenant_is_invisible_and_unwritable_to_the_runtime_role(owner_pool: PgPool) {
    let (org, _actor, port) = fixture(&owner_pool).await;
    let foreign_actor = seed_org_and_super_admin(&owner_pool, FOREIGN_ORG, "foreign").await;

    // A revision that genuinely exists — under the OTHER tenant. Seeded through
    // the BYPASSRLS owner pool, the only way to put a row on the far side of the
    // boundary being tested.
    sqlx::query(
        "INSERT INTO company_revisions \
         (org_id, version, command_id, actor_id, payload_digest, attributes, receipt) \
         VALUES ($1, 1, gen_random_uuid(), $2, $3, '{}'::jsonb, '{}'::jsonb)",
    )
    .bind(FOREIGN_ORG)
    .bind(*foreign_actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .execute(&owner_pool)
    .await
    .unwrap();

    // The rows are really there — otherwise "sees zero" below would be the
    // vacuous kind of zero, an empty table rather than a policy doing work.
    assert_eq!(
        count_rows(&owner_pool, COUNT_REVISIONS).await,
        1,
        "company_revisions must hold exactly the foreign tenant's row before the boundary is tested"
    );
    for (tag, org) in [("this", ORG), ("the foreign", FOREIGN_ORG)] {
        let head: i64 = sqlx::query_scalar(COUNT_ONE_ORGANIZATION)
            .bind(org)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
        assert_eq!(
            head, 1,
            "{tag} tenant's head must exist before the boundary is tested"
        );
    }

    // A `console_rt` session armed for ORG, which owns neither row.
    let runtime_pool = runtime_role_pool(&owner_pool).await;
    let mut tx = runtime_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let visible_revisions: i64 = sqlx::query_scalar(COUNT_REVISIONS)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        visible_revisions, 0,
        "org_isolation must hide company_revisions rows belonging to another tenant"
    );
    let visible_foreign_head: i64 = sqlx::query_scalar(COUNT_ONE_ORGANIZATION)
        .bind(FOREIGN_ORG)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        visible_foreign_head, 0,
        "org_isolation on the head must hide the other tenant's row"
    );
    let visible_own_head: i64 = sqlx::query_scalar(COUNT_ONE_ORGANIZATION)
        .bind(ORG)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(visible_own_head, 1, "this tenant's own head stays readable");

    // And the write half: WITH CHECK refuses a row planted in the other tenant.
    let refused = sqlx::query(
        "INSERT INTO company_revisions \
         (org_id, version, command_id, actor_id, payload_digest, attributes, receipt) \
         VALUES ($1, 9, gen_random_uuid(), $2, $3, '{}'::jsonb, '{}'::jsonb)",
    )
    .bind(FOREIGN_ORG)
    .bind(*foreign_actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .execute(&mut *tx)
    .await
    .unwrap_err();
    let error = refused.as_database_error().unwrap();
    assert_eq!(error.code().unwrap(), "42501", "got {}", error.message());
    assert_eq!(
        error.message(),
        "new row violates row-level security policy for table \"company_revisions\""
    );
    drop(tx);

    // Armed for ORG: the foreign revision is not a fabricated local head.
    // Deny-by-omission: a missing or other-tenant company is Ok(None) on the
    // runtime-role pool, never a distinct "wrong tenant" error.
    let foreign_as_local = get(&port, org).await;
    assert!(
        matches!(foreign_as_local, Ok(None)),
        "foreign Company is Ok(None) when armed for this org; got {foreign_as_local:?}"
    );
    assert!(list(&port, org).await.unwrap().is_empty());
    let unknown = get(&port, OrgId::from_uuid(Uuid::new_v4())).await;
    assert!(
        matches!(unknown, Ok(None)),
        "unknown org id is Ok(None) on the runtime-role pool, never a distinct error; got {unknown:?}"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_actor_from_another_org_is_refused(owner_pool: PgPool) {
    let (org, _actor, port) = fixture(&owner_pool).await;
    let foreign_actor = seed_org_and_super_admin(&owner_pool, FOREIGN_ORG, "foreign").await;

    let refused = execute(&port, command(org, foreign_actor, revise("타사 행위자")))
        .await
        .unwrap_err();
    let (code, message) = database_error(&refused);
    assert_eq!(code, "23503", "got {message}");
    assert!(
        message.contains("company_revisions"),
        "the (actor_id, org_id) foreign key on company_revisions must refuse it; got {message}"
    );

    assert_eq!(
        count_rows(&owner_pool, COUNT_REVISIONS).await,
        0,
        "a refused command must persist no revision"
    );
    assert_eq!(
        count_rows(&owner_pool, COUNT_RECEIPTS).await,
        0,
        "a refused command must persist no receipt"
    );
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
    let first_attributes = json!({ "legal_name": "주식회사 아크메", "reg_no": "110111" });
    let retry_attributes = json!({ "reg_no": "110111", "legal_name": "주식회사 아크메" });
    assert_eq!(
        first_attributes, retry_attributes,
        "the two payloads must be the same command for this test to mean anything"
    );

    let first = execute(
        &port,
        CompanyCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: CompanyQuery {
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
        CompanyCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: CompanyQuery {
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
        CompanyCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: revise("주식회사 아크메"),
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();

    let refused = execute(
        &port,
        CompanyCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: revise("위조"),
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap_err();

    assert!(
        matches!(refused, CompanyError::DigestConflict(id) if id == *command_id.as_uuid()),
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
        CompanyCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: revise("중복 키"),
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap_err();

    assert!(
        matches!(refused, CompanyError::DigestConflict(id) if id == *command_id.as_uuid()),
        "got {refused:?}"
    );
    assert_eq!(
        count_rows(&owner_pool, COUNT_REVISIONS).await,
        0,
        "a command id already spent under another owner must persist no revision"
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
            CompanyQuery {
                attributes: json!([1, 2, 3]),
            },
        ),
    )
    .await
    .unwrap_err();
    let CompanyError::Blocked(blockers) = &blocked else {
        panic!("expected a blocked preflight, got {blocked:?}");
    };
    assert_eq!(blockers, &["attributes must be a JSON object".to_owned()]);
    assert_eq!(count_rows(&owner_pool, COUNT_REVISIONS).await, 0);
    assert_eq!(count_rows(&owner_pool, COUNT_RECEIPTS).await, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_stored_receipt_naming_no_dispatch_target_is_refused(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());
    let query = revise("판독 불가");
    let accepted = execute(
        &port,
        CompanyCommand {
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
    .bind(json!({ "version": 1 }))
    .execute(&owner_pool)
    .await
    .unwrap();

    let refused = execute(
        &port,
        CompanyCommand {
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
        matches!(refused, CompanyError::UnreadableReceipt(id, _) if id == *command_id.as_uuid()),
        "a receipt the roster cannot read must be refused, never replayed; got {refused:?}"
    );
}

/// The port records WHOSE receipt this is, not the instance-action default.
///
/// `ont_action_command_receipts.owner` DEFAULTs to `'ontology.action'`, so until
/// the writers passed it explicitly every canonical receipt claimed to belong to
/// the pre-existing instance-action path. A wrong attribution recorded as fact is
/// worse than an absent column, because it reads as an answer.
///
/// The value is derived from the command's own `query.dispatch_target()`, never
/// from `action_key` -- this suite's commands carry `action_key: "revise"`, which
/// is unique only per object type and names no target on its own.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_company_receipt_is_attributed_to_company(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    execute(&port, command(org, actor, revise("주식회사 아크메")))
        .await
        .expect("the revise must land");

    let (owner, target): (String, Option<String>) =
        sqlx::query_as("SELECT owner, target FROM ont_action_command_receipts WHERE org_id = $1")
            .bind(*org.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(
        owner, "company",
        "the receipt must name the object that owns it"
    );
    assert_eq!(
        target.as_deref(),
        Some("company.revise"),
        "and the dispatch target it records"
    );
}
