#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `JobPositionPort` proven against a REAL PostgreSQL as the genuine runtime
//! role `console_rt` — never the BYPASSRLS superuser the `#[sqlx::test]` pool
//! connects as, which would mask a broken `org_isolation` policy.
//! `a_foreign_tenant_is_invisible_and_unwritable_to_the_runtime_role` is what
//! makes that claim NON-VACUOUS: it seeds the foreign tenant's rows through the
//! owner pool, ASSERTS through that pool that they really exist, and only then
//! shows `console_rt` counting zero. Without the first assertion "sees nothing"
//! would be an empty table rather than a policy doing work.
//!
//! The harness shape — `#[sqlx::test]`, the inlined `runtime_role_pool` and
//! `seed_org_and_super_admin`, and `spawn_blocking` around the SYNCHRONOUS
//! `execute` — is `tests/person_port_as_runtime_role.rs`'s, for the reasons it
//! records: migration 0196 admits exactly one migration applier,
//! `console-platform-test-support` cannot be added without rewriting this
//! package's `backend/Cargo.lock` entry, and `Handle::block_on` panics on a
//! runtime worker thread but not on a `spawn_blocking` one.
//!
//! WHAT IS DIFFERENT HERE, AND IT IS THE POINT. `job_positions` is a MUTABLE
//! head: 0215 gives it `org_unit_id`, no immutability trigger, and an explicit
//! `GRANT ... UPDATE ... TO console_rt`. `job_position_revisions` is
//! append-only. So this suite proves BOTH halves of that asymmetry against the
//! same database — `a_reorganisation_moves_the_head_and_the_history_survives_it`
//! and `an_update_of_a_revision_row_is_refused_by_the_trigger` — and a port that
//! confused them would fail one of the two.
//!
//! `org_units` is NOT written by this port. It belongs to `OrgUnitPort` in the
//! sibling module, so every org unit here is seeded through the owner pool, the
//! way a caller's already-existing unit arrives.

use console_kernel_core::{OrgId, UserId};
use console_ontology_canonical_adapter_postgres::job_position::{
    JobPositionCommand, JobPositionError, JobPositionQuery, JobPositionView, PgJobPositionPort,
};
use console_ontology_canonical_domain::{
    CanonicalPort, CommandId, CommandReceipt, DispatchTarget, JobPositionPort, ObjectKey,
    ReceiptOwner,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0x6f10_0000_0000_0000_0000_0000_0000_0001);
const FOREIGN_ORG: Uuid = Uuid::from_u128(0x6f10_0000_0000_0000_0000_0000_0000_0002);

/// The port must satisfy the NAMED trait, not merely `CanonicalPort`. The
/// blanket impl in `canonical-domain` makes `JobPositionPort` an alias for
/// `CanonicalPort<Object = JobPosition>`, so this bound stops holding the moment
/// the adapter is retargeted at a different object.
fn assert_implements_job_position_port<P: JobPositionPort>() {}

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

/// An org unit under `org`, seeded through the owner pool. `org_units` belongs
/// to `OrgUnitPort`; this port only REFERENCES it.
async fn seed_org_unit(owner_pool: &PgPool, org: Uuid) -> Uuid {
    sqlx::query_scalar("INSERT INTO org_units (org_id) VALUES ($1) RETURNING id")
        .bind(org)
        .fetch_one(owner_pool)
        .await
        .unwrap()
}

/// The tenant, its actor, one org unit, and the port built on a `console_rt`
/// pool.
async fn fixture(owner_pool: &PgPool) -> (OrgId, UserId, Uuid, PgJobPositionPort) {
    let actor = seed_org_and_super_admin(owner_pool, ORG, "jobposition").await;
    let unit = seed_org_unit(owner_pool, ORG).await;
    let runtime_pool = runtime_role_pool(owner_pool).await;
    let port = PgJobPositionPort::new(runtime_pool, tokio::runtime::Handle::current());
    (OrgId::from_uuid(ORG), actor, unit, port)
}

/// Drive the SYNCHRONOUS `execute` off the runtime's worker thread. See the
/// module doc: this is the whole reason the suite is shaped this way.
async fn execute(
    port: &PgJobPositionPort,
    command: JobPositionCommand,
) -> Result<CommandReceipt, JobPositionError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.execute(&command))
        .await
        .unwrap()
}

async fn get(
    port: &PgJobPositionPort,
    org: OrgId,
    job_position_id: Uuid,
) -> Result<Option<JobPositionView>, JobPositionError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.get(org, job_position_id))
        .await
        .unwrap()
}

async fn list_for_org_unit(
    port: &PgJobPositionPort,
    org: OrgId,
    org_unit_id: Uuid,
) -> Result<Vec<JobPositionView>, JobPositionError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.list_for_org_unit(org, org_unit_id))
        .await
        .unwrap()
}

async fn list(
    port: &PgJobPositionPort,
    org: OrgId,
) -> Result<Vec<JobPositionView>, JobPositionError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.list(org))
        .await
        .unwrap()
}

const COUNT_POSITIONS: &str = "SELECT count(*)::bigint FROM job_positions";
const COUNT_REVISIONS: &str = "SELECT count(*)::bigint FROM job_position_revisions";
const COUNT_RECEIPTS: &str = "SELECT count(*)::bigint FROM ont_action_command_receipts";

/// The two tables the contract assigns to `JobPosition`, each with the count
/// over it. `sqlx` 0.9 accepts only `&'static str` SQL, so the table name cannot
/// be interpolated and every statement here is a literal.
const OWNED_TABLES: [(&str, &str); 2] = [
    ("job_positions", COUNT_POSITIONS),
    ("job_position_revisions", COUNT_REVISIONS),
];

/// Rows counted through the BYPASSRLS owner pool, so a test can see what a
/// tenant-armed session must not.
async fn count_rows(owner_pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(owner_pool).await.unwrap()
}

/// The SQLSTATE and the message PostgreSQL actually returned, so a test quotes
/// the real error rather than a paraphrase of it.
fn database_error(error: &JobPositionError) -> (String, String) {
    let JobPositionError::Database(sqlx_error) = error else {
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

fn create(org_unit_id: Uuid, title: &str) -> JobPositionQuery {
    JobPositionQuery::Create {
        org_unit_id,
        attributes: json!({ "title": title }),
    }
}

fn revise(job_position_id: Uuid, org_unit_id: Option<Uuid>, title: &str) -> JobPositionQuery {
    JobPositionQuery::Revise {
        job_position_id,
        org_unit_id,
        attributes: json!({ "title": title }),
    }
}

fn command(org: OrgId, actor: UserId, query: JobPositionQuery) -> JobPositionCommand {
    JobPositionCommand {
        org_id: org,
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        actor_id: actor,
        query,
        action_key: "revise".to_owned(),
        object_type_id: Uuid::nil(),
    }
}

fn position_of(receipt: &CommandReceipt) -> Uuid {
    receipt.result()["job_position_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

/// The whole revision row as JSONB, so "unchanged" means the whole row and not
/// the two columns a test remembered to name.
async fn revision_snapshot(owner_pool: &PgPool, position: Uuid, version: i64) -> String {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT to_jsonb(r) FROM job_position_revisions r \
         WHERE org_id = $1 AND job_position_id = $2 AND version = $3",
    )
    .bind(ORG)
    .bind(position)
    .bind(version)
    .fetch_one(owner_pool)
    .await
    .unwrap()
    .to_string()
}

async fn head_unit(owner_pool: &PgPool, position: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT org_unit_id FROM job_positions WHERE org_id = $1 AND id = $2")
        .bind(ORG)
        .bind(position)
        .fetch_one(owner_pool)
        .await
        .unwrap()
}

#[test]
fn the_contract_identity_is_copied_verbatim_and_the_port_is_the_named_one() {
    assert_implements_job_position_port::<PgJobPositionPort>();
    assert_eq!(ObjectKey::JobPosition.as_str(), "job_position");
    assert_eq!(
        ObjectKey::JobPosition.owned_tables(),
        ["job_positions", "job_position_revisions"]
    );
    assert_eq!(
        ObjectKey::JobPosition.owner_crate(),
        "console-ontology-canonical-adapter-postgres"
    );
    assert_eq!(
        DispatchTarget::OrganizationCreateJobPosition.as_str(),
        "organization.create_job_position"
    );
    assert_eq!(
        DispatchTarget::OrganizationReviseJobPosition.as_str(),
        "organization.revise_job_position"
    );
    assert_eq!(
        DispatchTarget::OrganizationCreateJobPosition.object(),
        ObjectKey::JobPosition
    );
    assert_eq!(
        DispatchTarget::OrganizationReviseJobPosition.object(),
        ObjectKey::JobPosition
    );
}

#[test]
fn preflight_is_pure_and_blocks_a_non_object_payload_a_nil_position_and_a_nil_unit() {
    let not_an_object = JobPositionQuery::Create {
        org_unit_id: Uuid::new_v4(),
        attributes: json!("not an object"),
    };
    let blocked = <PgJobPositionPort as CanonicalPort>::preflight(&not_an_object);
    assert!(!blocked.is_ok());
    assert_eq!(
        blocked.blockers(),
        ["attributes must be a JSON object".to_owned()]
    );

    let nil_position =
        <PgJobPositionPort as CanonicalPort>::preflight(&revise(Uuid::nil(), None, "팀장"));
    assert!(!nil_position.is_ok());
    assert_eq!(
        nil_position.blockers(),
        ["job_position_id must not be nil".to_owned()]
    );

    let nil_unit = <PgJobPositionPort as CanonicalPort>::preflight(&create(Uuid::nil(), "팀장"));
    assert!(!nil_unit.is_ok());
    assert_eq!(
        nil_unit.blockers(),
        ["org_unit_id must not be nil".to_owned()]
    );

    let missing_title = JobPositionQuery::Create {
        org_unit_id: Uuid::new_v4(),
        attributes: json!({}),
    };
    let missing = <PgJobPositionPort as CanonicalPort>::preflight(&missing_title);
    assert!(!missing.is_ok());
    assert_eq!(missing.blockers(), ["title is required".to_owned()]);

    let blank_title = JobPositionQuery::Create {
        org_unit_id: Uuid::new_v4(),
        attributes: json!({ "title": " " }),
    };
    let blank = <PgJobPositionPort as CanonicalPort>::preflight(&blank_title);
    assert!(!blank.is_ok());
    assert_eq!(blank.blockers(), ["title must not be empty".to_owned()]);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_job_position_is_created_and_read_back(owner_pool: PgPool) {
    let (org, actor, unit, port) = fixture(&owner_pool).await;
    let query = create(unit, "백엔드 엔지니어");
    assert!(<PgJobPositionPort as CanonicalPort>::preflight(&query).is_ok());

    let receipt = execute(&port, command(org, actor, query)).await.unwrap();

    assert_eq!(receipt.org_id(), org);
    assert_eq!(receipt.actor_id(), actor);
    assert_eq!(
        receipt.owner(),
        ReceiptOwner::Canonical(ObjectKey::JobPosition),
        "the receipt must be owned by the canonical JobPosition object"
    );
    assert_eq!(
        receipt.target(),
        DispatchTarget::OrganizationCreateJobPosition
    );
    assert_eq!(receipt.result()["version"].as_i64(), Some(1));
    assert_eq!(
        receipt.result()["org_unit_id"].as_str().unwrap(),
        unit.to_string(),
        "receipt must carry the head org_unit_id for REST/action readback"
    );

    let position_id = position_of(&receipt);
    let row = sqlx::query(
        "SELECT p.org_unit_id, r.version, r.attributes, r.command_id, r.payload_digest \
         FROM job_positions p \
         JOIN job_position_revisions r ON r.org_id = p.org_id AND r.job_position_id = p.id \
         WHERE p.org_id = $1 AND p.id = $2",
    )
    .bind(ORG)
    .bind(position_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();

    assert_eq!(
        row.get::<Uuid, _>("org_unit_id"),
        unit,
        "the contract's \"referencing OrgUnit\" is the head column, not prose"
    );
    assert_eq!(row.get::<i64, _>("version"), 1);
    assert_eq!(
        row.get::<serde_json::Value, _>("attributes"),
        json!({ "title": "백엔드 엔지니어" })
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

    // First-class query surface: create → get round-trips the same IDs.
    let viewed = get(&port, org, position_id)
        .await
        .unwrap()
        .expect("created JobPosition must be queryable");
    assert_eq!(viewed.job_position_id, position_id);
    assert_eq!(viewed.org_unit_id, unit);
    assert_eq!(viewed.version, 1);
    assert_eq!(viewed.attributes, json!({ "title": "백엔드 엔지니어" }));
    let listed_unit = list_for_org_unit(&port, org, unit).await.unwrap();
    assert_eq!(listed_unit, vec![viewed.clone()]);
    let listed = list(&port, org).await.unwrap();
    assert_eq!(listed, vec![viewed]);
    let unknown = get(&port, org, Uuid::new_v4()).await;
    assert!(
        matches!(unknown, Ok(None)),
        "unknown JobPosition id is Ok(None) on the runtime-role pool, never a distinct error; got {unknown:?}"
    );
}

/// Free-text `employees.position` and `recruit_postings` are excluded by the
/// contract. Seeding both must NOT create or imply a JobPosition; only the port
/// create path does — and its attributes come from the command, not job text.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn free_text_and_recruiting_are_never_inferred_as_job_positions(owner_pool: PgPool) {
    let (org, actor, unit, port) = fixture(&owner_pool).await;

    // Legacy free-text seat label on employees (migration 0066).
    sqlx::query(
        "INSERT INTO employees \
         (org_id, company, name, source_filename, source_sheet, source_row, source_key, \
          position, org_unit) \
         VALUES ($1, 'Acme', '이직원', 'seed.xlsx', 'Sheet1', 1, 'emp-free-text', \
                 '백엔드 엔지니어', '팀')",
    )
    .bind(ORG)
    .execute(&owner_pool)
    .await
    .unwrap();

    // Recruiting posting (migration 0187) — not a canonical position. `position_ref`
    // may hold free text; it must never become a job_positions row by itself.
    sqlx::query(
        "INSERT INTO recruit_postings \
         (org_id, posting_no, role_title, company, worksite, employment_type, scope, \
          headcount, position_ref, status, created_by) \
         VALUES ($1, 'JP-1001', '백엔드 엔지니어', 'Acme', '서울', 'REGULAR', 'EXTERNAL', \
                 1, '백엔드 엔지니어', 'DRAFT', $2)",
    )
    .bind(ORG)
    .bind(*actor.as_uuid())
    .execute(&owner_pool)
    .await
    .unwrap();

    assert_eq!(
        count_rows(&owner_pool, COUNT_POSITIONS).await,
        0,
        "legacy free-text / recruiting must not materialize job_positions"
    );
    let unknown = get(&port, org, Uuid::new_v4()).await;
    assert!(
        matches!(unknown, Ok(None)),
        "querying an unknown id must be Ok(None) on the runtime-role pool, never a distinct error; got {unknown:?}"
    );

    let created = execute(&port, command(org, actor, create(unit, "정규 직위")))
        .await
        .unwrap();
    let position_id = position_of(&created);
    let viewed = get(&port, org, position_id)
        .await
        .unwrap()
        .expect("port-created JobPosition is the authority surface");
    assert_eq!(viewed.attributes, json!({ "title": "정규 직위" }));
    assert_ne!(
        viewed.attributes.get("title").and_then(|v| v.as_str()),
        Some("백엔드 엔지니어"),
        "port must not copy employees.position / recruit title into attributes"
    );
    assert_eq!(count_rows(&owner_pool, COUNT_POSITIONS).await, 1);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_revision_is_appended_and_the_prior_revision_is_unchanged(owner_pool: PgPool) {
    let (org, actor, unit, port) = fixture(&owner_pool).await;
    let created = execute(&port, command(org, actor, create(unit, "주니어 엔지니어")))
        .await
        .unwrap();
    let position_id = position_of(&created);

    let before = revision_snapshot(&owner_pool, position_id, 1).await;

    let revised = execute(
        &port,
        command(org, actor, revise(position_id, None, "시니어 엔지니어")),
    )
    .await
    .unwrap();
    assert_eq!(
        revised.target(),
        DispatchTarget::OrganizationReviseJobPosition
    );
    assert_eq!(revised.result()["version"].as_i64(), Some(2));

    let after = revision_snapshot(&owner_pool, position_id, 1).await;
    assert_eq!(before, after, "appending a revision rewrote revision 1");

    let versions: Vec<i64> = sqlx::query_scalar(
        "SELECT version FROM job_position_revisions \
         WHERE org_id = $1 AND job_position_id = $2 ORDER BY version",
    )
    .bind(ORG)
    .bind(position_id)
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(versions, vec![1, 2]);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_reorganisation_moves_the_head_and_the_history_survives_it(owner_pool: PgPool) {
    let (org, actor, unit, port) = fixture(&owner_pool).await;
    let other_unit = seed_org_unit(&owner_pool, ORG).await;

    let created = execute(&port, command(org, actor, create(unit, "인사팀장")))
        .await
        .unwrap();
    let position_id = position_of(&created);
    assert_eq!(head_unit(&owner_pool, position_id).await, unit);
    let before = revision_snapshot(&owner_pool, position_id, 1).await;

    // The move: `job_positions` carries no immutability trigger, by design, and
    // `console_rt` holds UPDATE on it by an explicit GRANT in 0215.
    execute(
        &port,
        command(
            org,
            actor,
            revise(position_id, Some(other_unit), "인사팀장"),
        ),
    )
    .await
    .unwrap();

    assert_eq!(
        head_unit(&owner_pool, position_id).await,
        other_unit,
        "a reorganisation must move the head"
    );
    assert_eq!(
        revision_snapshot(&owner_pool, position_id, 1).await,
        before,
        "moving the head rewrote the history it was supposed to leave alone"
    );
    let versions: Vec<i64> = sqlx::query_scalar(
        "SELECT version FROM job_position_revisions \
         WHERE org_id = $1 AND job_position_id = $2 ORDER BY version",
    )
    .bind(ORG)
    .bind(position_id)
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    assert_eq!(versions, vec![1, 2]);

    let viewed = get(&port, org, position_id)
        .await
        .unwrap()
        .expect("moved JobPosition must remain queryable");
    assert_eq!(viewed.org_unit_id, other_unit);
    assert_eq!(viewed.version, 2);
    assert!(
        list_for_org_unit(&port, org, unit)
            .await
            .unwrap()
            .is_empty(),
        "the old unit must no longer list the moved position"
    );
    assert_eq!(
        list_for_org_unit(&port, org, other_unit).await.unwrap(),
        vec![viewed.clone()]
    );
    assert_eq!(list(&port, org).await.unwrap(), vec![viewed]);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_update_of_a_revision_row_is_refused_by_the_trigger(owner_pool: PgPool) {
    let (org, actor, unit, port) = fixture(&owner_pool).await;
    let created = execute(&port, command(org, actor, create(unit, "회계 담당")))
        .await
        .unwrap();
    let position_id = position_of(&created);
    let before = revision_snapshot(&owner_pool, position_id, 1).await;

    // 0215 GRANTs `console_rt` only SELECT, INSERT on `job_position_revisions`,
    // and production additionally holds UPDATE through
    // `ALTER DEFAULT PRIVILEGES ... FOR ROLE console_app` (0215's own header).
    // `#[sqlx::test]` applies migrations as the superuser, for whom that default
    // privilege does not exist, so the deployed ACL is reproduced first — or the
    // assertion below observes a 42501 that does not exist where this ships.
    sqlx::query("GRANT UPDATE ON job_position_revisions TO console_rt")
        .execute(&owner_pool)
        .await
        .unwrap();
    let runtime_holds_update: bool = sqlx::query_scalar(
        "SELECT has_table_privilege('console_rt', 'job_position_revisions', 'UPDATE')",
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
        "UPDATE job_position_revisions SET attributes = '{\"title\":\"forged\"}'::jsonb \
         WHERE org_id = $1 AND job_position_id = $2",
    )
    .bind(ORG)
    .bind(position_id)
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
        "canonical org-structure table job_position_revisions: UPDATE is refused, the row is \
         immutable"
    );
    drop(tx);

    // The owner is refused by the same trigger; ownership buys no exemption.
    let as_owner =
        sqlx::query("UPDATE job_position_revisions SET version = version + 100 WHERE org_id = $1")
            .bind(ORG)
            .execute(&owner_pool)
            .await
            .unwrap_err();
    let owner_error = as_owner.as_database_error().unwrap();
    assert_eq!(owner_error.code().unwrap(), "P0001");
    assert_eq!(
        owner_error.message(),
        "canonical org-structure table job_position_revisions: UPDATE is refused, the row is \
         immutable"
    );

    let after = revision_snapshot(&owner_pool, position_id, 1).await;
    assert_eq!(before, after, "a refused UPDATE rewrote the revision row");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_foreign_tenant_is_invisible_and_unwritable_to_the_runtime_role(owner_pool: PgPool) {
    let (org, _actor, _unit, port) = fixture(&owner_pool).await;
    let foreign_actor = seed_org_and_super_admin(&owner_pool, FOREIGN_ORG, "foreign").await;
    let foreign_unit = seed_org_unit(&owner_pool, FOREIGN_ORG).await;

    // A position and a revision that genuinely exist — under the OTHER tenant.
    // Seeded through the BYPASSRLS owner pool, which is the only way to put rows
    // on the far side of the boundary being tested.
    let foreign_position: Uuid = sqlx::query_scalar(
        "INSERT INTO job_positions (org_id, org_unit_id) VALUES ($1, $2) RETURNING id",
    )
    .bind(FOREIGN_ORG)
    .bind(foreign_unit)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO job_position_revisions \
         (org_id, job_position_id, version, command_id, actor_id, payload_digest, attributes, \
          receipt) \
         VALUES ($1, $2, 1, gen_random_uuid(), $3, $4, '{}'::jsonb, '{}'::jsonb)",
    )
    .bind(FOREIGN_ORG)
    .bind(foreign_position)
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

    // A `console_rt` session armed for ORG, which owns neither of them.
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
    let refused = sqlx::query("INSERT INTO job_positions (org_id, org_unit_id) VALUES ($1, $2)")
        .bind(FOREIGN_ORG)
        .bind(foreign_unit)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    let error = refused.as_database_error().unwrap();
    assert_eq!(error.code().unwrap(), "42501", "got {}", error.message());
    assert_eq!(
        error.message(),
        "new row violates row-level security policy for table \"job_positions\""
    );
    drop(tx);

    assert!(list(&port, org).await.unwrap().is_empty());
    let foreign_head = get(&port, org, foreign_position).await;
    assert!(
        matches!(foreign_head, Ok(None)),
        "foreign JobPosition id is Ok(None) on the runtime-role pool, never a wrong-tenant error; got {foreign_head:?}"
    );
    let unknown = get(&port, org, Uuid::new_v4()).await;
    assert!(
        matches!(unknown, Ok(None)),
        "unknown JobPosition id is indistinguishable from a foreign tenant: Ok(None); got {unknown:?}"
    );
    assert!(
        list_for_org_unit(&port, org, foreign_unit)
            .await
            .unwrap()
            .is_empty(),
        "list_for_org_unit of a foreign unit is empty, not a wrong-tenant error"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_actor_from_another_org_is_refused(owner_pool: PgPool) {
    let (org, _actor, unit, port) = fixture(&owner_pool).await;
    let foreign_actor = seed_org_and_super_admin(&owner_pool, FOREIGN_ORG, "foreign").await;

    let refused = execute(
        &port,
        command(org, foreign_actor, create(unit, "타사 행위자")),
    )
    .await
    .unwrap_err();
    let (code, message) = database_error(&refused);
    assert_eq!(code, "23503", "got {message}");
    assert!(
        message.contains("job_position_revisions"),
        "the (actor_id, org_id) foreign key on job_position_revisions must refuse it; got {message}"
    );

    assert_eq!(
        count_rows(&owner_pool, COUNT_POSITIONS).await,
        0,
        "a refused command must persist no job position"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn an_org_unit_from_another_tenant_is_refused(owner_pool: PgPool) {
    let (org, actor, _unit, port) = fixture(&owner_pool).await;
    seed_org_and_super_admin(&owner_pool, FOREIGN_ORG, "foreign").await;
    let foreign_unit = seed_org_unit(&owner_pool, FOREIGN_ORG).await;

    // The unit exists, but under the other tenant. `(org_id, org_unit_id)` is a
    // COMPOSITE key, so it is refused as a foreign key rather than accepted as a
    // cross-tenant edge.
    let refused = execute(
        &port,
        command(org, actor, create(foreign_unit, "타사 조직 소속")),
    )
    .await
    .unwrap_err();
    let (code, message) = database_error(&refused);
    assert_eq!(code, "23503", "got {message}");
    assert!(
        message.contains("job_positions"),
        "the (org_id, org_unit_id) foreign key on job_positions must refuse it; got {message}"
    );
    assert_eq!(count_rows(&owner_pool, COUNT_POSITIONS).await, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_repeat_of_the_same_command_replays_the_stored_receipt(owner_pool: PgPool) {
    let (org, actor, unit, port) = fixture(&owner_pool).await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());

    // The same payload, built in the two key orders a client and a re-encoding
    // proxy may each produce. `serde_json` resolves with `preserve_order` in
    // this workspace, so these two objects compare EQUAL while serialising to
    // different bytes — the digest must not be able to tell them apart, or the
    // retry after a timeout is refused instead of replayed.
    let first_attributes = json!({ "title": "팀장", "grade": "G5" });
    let retry_attributes = json!({ "grade": "G5", "title": "팀장" });
    assert_eq!(
        first_attributes, retry_attributes,
        "the two payloads must be the same command for this test to mean anything"
    );

    let first = execute(
        &port,
        JobPositionCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: JobPositionQuery::Create {
                org_unit_id: unit,
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
        JobPositionCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: JobPositionQuery::Create {
                org_unit_id: unit,
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
    assert_eq!(count_rows(&owner_pool, COUNT_POSITIONS).await, 1);
    assert_eq!(
        count_rows(&owner_pool, COUNT_REVISIONS).await,
        1,
        "a replayed command must append no revision"
    );
    assert_eq!(count_rows(&owner_pool, COUNT_RECEIPTS).await, 1);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_repeat_with_a_different_payload_is_refused(owner_pool: PgPool) {
    let (org, actor, unit, port) = fixture(&owner_pool).await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());

    execute(
        &port,
        JobPositionCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: create(unit, "팀장"),
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap();

    let refused = execute(
        &port,
        JobPositionCommand {
            org_id: org,
            command_id,
            actor_id: actor,
            query: create(unit, "위조"),
            action_key: "revise".to_owned(),
            object_type_id: Uuid::nil(),
        },
    )
    .await
    .unwrap_err();

    assert!(
        matches!(refused, JobPositionError::DigestConflict(id) if id == *command_id.as_uuid()),
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
    let (org, actor, _unit, port) = fixture(&owner_pool).await;

    let blocked = execute(&port, command(org, actor, create(Uuid::nil(), "무명")))
        .await
        .unwrap_err();
    let JobPositionError::Blocked(blockers) = &blocked else {
        panic!("expected a blocked preflight, got {blocked:?}");
    };
    assert_eq!(blockers, &["org_unit_id must not be nil".to_owned()]);
    assert_eq!(count_rows(&owner_pool, COUNT_POSITIONS).await, 0);
    assert_eq!(count_rows(&owner_pool, COUNT_RECEIPTS).await, 0);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_stored_receipt_naming_no_dispatch_target_is_refused(owner_pool: PgPool) {
    let (org, actor, unit, port) = fixture(&owner_pool).await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());
    let query = create(unit, "판독 불가");
    let accepted = execute(
        &port,
        JobPositionCommand {
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
    .bind(json!({ "job_position_id": "6f100000-0000-0000-0000-0000000000ff", "version": 1 }))
    .execute(&owner_pool)
    .await
    .unwrap();

    let refused = execute(
        &port,
        JobPositionCommand {
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
        matches!(refused, JobPositionError::UnreadableReceipt(id, _) if id == *command_id.as_uuid()),
        "a receipt the roster cannot read must be refused, never replayed; got {refused:?}"
    );
}
