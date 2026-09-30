#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Identity (source==target) and unknown-destination ReassignOrgUnit
//! fail-closed cases. Sibling of `employment_reassign_as_runtime_role`
//! (unbound + mixed-peer) so neither file crosses the 100–300 size bar.

use console_kernel_core::{OrgId, UserId};
use console_ontology_canonical_adapter_postgres::employment::{
    EmploymentError, PgEmploymentPort, reassign_org_unit_via_transfers_in_tx,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0001);
const ORG_UNIT_SALES: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0010);
const ORG_UNIT_TECH: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0011);
const JOB_STAFF: Uuid = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0020);

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

async fn begin_org_armed_tx(owner_pool: &PgPool) -> sqlx::Transaction<'_, sqlx::Postgres> {
    let mut tx = owner_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(ORG.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx
}

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000 + seconds).unwrap()
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

/// source == target → `Blocked` ("source and target must differ").
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn reassign_org_unit_same_source_and_target_is_refused(owner_pool: PgPool) {
    let (org, actor, _port) = fixture(&owner_pool).await;
    let sales = ORG_UNIT_SALES.to_string();
    let refused = drive_reassign(&owner_pool, org, actor, Uuid::new_v4(), &sales, &sales).await;
    let EmploymentError::Blocked(blockers) = &refused else {
        panic!("same source and target must be Blocked, got {refused:?}");
    };
    assert!(
        blockers
            .iter()
            .any(|b| b.contains("source and target must differ")),
        "got {blockers:?}"
    );
}

/// Unknown destination OrgUnit → `UnknownOrgUnit`, not `Ok(0)`.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn reassign_org_unit_unknown_to_org_unit_is_refused(owner_pool: PgPool) {
    let (org, actor, _port) = fixture(&owner_pool).await;
    let unknown_to = Uuid::from_u128(0xe3b0_0000_0000_0000_0000_0000_0000_0099);
    let refused = drive_reassign(
        &owner_pool,
        org,
        actor,
        Uuid::new_v4(),
        &ORG_UNIT_SALES.to_string(),
        &unknown_to.to_string(),
    )
    .await;
    assert!(
        matches!(refused, EmploymentError::UnknownOrgUnit(id) if id == unknown_to),
        "unknown destination must be UnknownOrgUnit, not Ok(0); got {refused:?}"
    );
}
