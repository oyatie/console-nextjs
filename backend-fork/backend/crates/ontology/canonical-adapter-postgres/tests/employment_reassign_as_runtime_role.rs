#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Unbound and mixed-peer ReassignOrgUnit fail-closed cases. Sibling of
//! `employment_port_as_runtime_role`. Identity and unknown destination live
//! in `employment_reassign_identity_as_runtime_role`.

use console_kernel_core::{OrgId, UserId};
use console_ontology_canonical_adapter_postgres::employment::{
    EmploymentAttributes, EmploymentCommand, EmploymentError, EmploymentQuery, PgEmploymentPort,
    reassign_org_unit_via_transfers_in_tx,
};
use console_ontology_canonical_domain::{CanonicalPort, CommandId, CommandReceipt};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0001);
const ORG_UNIT_SALES: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0010);
const ORG_UNIT_TECH: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0011);
const JOB_STAFF: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0020);
const COUNT_REVISIONS: &str = "SELECT count(*)::bigint FROM employment_revisions";
const COUNT_RECEIPTS: &str = "SELECT count(*)::bigint FROM ont_action_command_receipts";

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

async fn seed_employee(owner_pool: &PgPool, source_key: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO employees \
         (org_id, company, name, source_filename, source_sheet, source_row, source_key) \
         VALUES ($1, 'ACME', $2, 'seed.xlsx', 'Sheet1', 1, $2) RETURNING id",
    )
    .bind(ORG)
    .bind(source_key)
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

async fn stamp_legacy_org_unit(owner_pool: &PgPool, employee: Uuid, org_unit: Uuid) {
    sqlx::query("UPDATE employees SET org_unit = $1, employment_status = 'ACTIVE' WHERE id = $2")
        .bind(org_unit.to_string())
        .bind(employee)
        .execute(owner_pool)
        .await
        .unwrap();
}

async fn begin_org_armed_tx(owner_pool: &PgPool) -> sqlx::Transaction<'_, sqlx::Postgres> {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx
}

async fn fixture(owner_pool: &PgPool) -> (OrgId, UserId, PgEmploymentPort) {
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
    )
    .bind(ORG).bind("org-employment").bind("Org employment").execute(owner_pool).await.unwrap();
    let actor = UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
        .bind(*actor.as_uuid())
        .bind("Admin employment")
        .bind(["SUPER_ADMIN"].as_slice())
        .bind(ORG)
        .execute(owner_pool)
        .await
        .unwrap();
    for unit in [ORG_UNIT_SALES, ORG_UNIT_TECH] {
        sqlx::query("INSERT INTO org_units (org_id, id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(ORG)
            .bind(unit)
            .execute(owner_pool)
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO job_positions (org_id, id, org_unit_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(ORG).bind(JOB_STAFF).bind(ORG_UNIT_SALES).execute(owner_pool).await.unwrap();
    let port = PgEmploymentPort::new(
        runtime_role_pool(owner_pool).await,
        tokio::runtime::Handle::current(),
    );
    (OrgId::from_uuid(ORG), actor, port)
}

fn attributes() -> EmploymentAttributes {
    EmploymentAttributes {
        company: "ACME".to_owned(),
        org_unit_id: Some(ORG_UNIT_SALES),
        job_position_id: Some(JOB_STAFF),
        employment_status: "ACTIVE".to_owned(),
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
    OffsetDateTime::from_unix_timestamp(1_800_000_000 + seconds).unwrap()
}

async fn execute(
    port: &PgEmploymentPort,
    command: EmploymentCommand,
) -> Result<CommandReceipt, EmploymentError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.execute(&command))
        .await
        .unwrap()
}

async fn count_rows(owner_pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(owner_pool).await.unwrap()
}

async fn legacy_head(owner_pool: &PgPool, employee: Uuid) -> Option<String> {
    sqlx::query("SELECT org_unit FROM employees WHERE org_id = $1 AND id = $2")
        .bind(ORG)
        .bind(employee)
        .fetch_one(owner_pool)
        .await
        .unwrap()
        .get("org_unit")
}

async fn appointed(
    owner_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    port: &PgEmploymentPort,
    tag: &str,
) -> (Uuid, Uuid) {
    let employee = seed_employee(owner_pool, tag).await;
    let receipt = execute(
        port,
        command(
            org,
            actor,
            EmploymentQuery::Appoint {
                employee_id: employee,
                valid_from: at(0),
                attributes: attributes(),
            },
        ),
    )
    .await
    .unwrap();
    (
        employee,
        receipt.result()["employment_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
    )
}

async fn drive_reassign(
    owner_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    command_id: Uuid,
    from: &str,
    to: &str,
) -> EmploymentError {
    let mut tx = begin_org_armed_tx(owner_pool).await;
    let refused = reassign_org_unit_via_transfers_in_tx(
        &mut tx,
        org,
        actor,
        command_id,
        from,
        to,
        "ACME",
        at(86_400),
    )
    .await
    .expect_err("reassign must fail closed");
    drop(tx);
    refused
}

/// Unbound ACTIVE legacy head → `UnboundEmployeeForTransfer`; no revision, no receipt, head stays.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn reassign_org_unit_refuses_unbound_employee_and_writes_nothing(owner_pool: PgPool) {
    let (org, actor, _port) = fixture(&owner_pool).await;
    let unbound = seed_employee(&owner_pool, "unbound-head").await;
    stamp_legacy_org_unit(&owner_pool, unbound, ORG_UNIT_SALES).await;
    let bindings: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM employment_source_bindings WHERE employee_id = $1",
    )
    .bind(unbound)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(bindings, 0, "fixture must be an unbound legacy head");
    let revisions = count_rows(&owner_pool, COUNT_REVISIONS).await;
    let receipts = count_rows(&owner_pool, COUNT_RECEIPTS).await;
    let refused = drive_reassign(
        &owner_pool,
        org,
        actor,
        Uuid::new_v4(),
        &ORG_UNIT_SALES.to_string(),
        &ORG_UNIT_TECH.to_string(),
    )
    .await;
    assert!(
        matches!(
            refused,
            EmploymentError::UnboundEmployeeForTransfer { employee_id } if employee_id == unbound
        ),
        "got {refused:?}"
    );
    assert_eq!(
        legacy_head(&owner_pool, unbound).await,
        Some(ORG_UNIT_SALES.to_string())
    );
    assert_eq!(count_rows(&owner_pool, COUNT_REVISIONS).await, revisions);
    assert_eq!(count_rows(&owner_pool, COUNT_RECEIPTS).await, receipts);
}

/// Mixed bound+unbound unit → whole tx refuses; bound peer does not move.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn reassign_org_unit_refuses_when_a_peer_is_unbound(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let (bound_employee, bound_employment) =
        appointed(&owner_pool, org, actor, &port, "reassign-bound-peer").await;
    execute(
        &port,
        command(
            org,
            actor,
            EmploymentQuery::Promote {
                employment_id: bound_employment,
                valid_from: at(3_600),
                attributes: attributes(),
            },
        ),
    )
    .await
    .unwrap();
    let unbound = seed_employee(&owner_pool, "reassign-unbound-peer").await;
    stamp_legacy_org_unit(&owner_pool, unbound, ORG_UNIT_SALES).await;
    let revisions = count_rows(&owner_pool, COUNT_REVISIONS).await;
    let receipts = count_rows(&owner_pool, COUNT_RECEIPTS).await;
    let refused = drive_reassign(
        &owner_pool,
        org,
        actor,
        Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_00ac),
        &ORG_UNIT_SALES.to_string(),
        &ORG_UNIT_TECH.to_string(),
    )
    .await;
    assert!(
        matches!(
            refused,
            EmploymentError::UnboundEmployeeForTransfer { employee_id } if employee_id == unbound
        ),
        "got {refused:?}"
    );
    assert_eq!(
        legacy_head(&owner_pool, bound_employee).await,
        Some(ORG_UNIT_SALES.to_string())
    );
    assert_eq!(
        legacy_head(&owner_pool, unbound).await,
        Some(ORG_UNIT_SALES.to_string())
    );
    assert_eq!(count_rows(&owner_pool, COUNT_REVISIONS).await, revisions);
    assert_eq!(count_rows(&owner_pool, COUNT_RECEIPTS).await, receipts);
}
