#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_kernel_core::{OrgId, UserId};
use console_ontology_canonical_adapter_postgres::org_unit::{
    OrgUnitCommand, OrgUnitError, OrgUnitQuery, PgOrgUnitPort,
};
use console_ontology_canonical_domain::{CanonicalPort, CommandId, CommandReceipt};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0x0c51_0000_0000_0000_0000_0000_0000_0001);
const FOREIGN_ORG: Uuid = Uuid::from_u128(0x0c51_0000_0000_0000_0000_0000_0000_0002);

const KIND_REQUIRED: &str = "kind is required";
const KIND_EMPTY: &str = "kind must not be empty";
const KIND_CLOSED: &str = "kind must be site, department, or team";
const SITE_NO_PARENT: &str = "site must not have parent_id";
const DEPT_PARENT_REQUIRED: &str = "department parent_id is required";
const TEAM_PARENT_REQUIRED: &str = "team parent_id is required";
const PARENT_UUID: &str = "parent_id must be a uuid";
const KIND_IMMUTABLE: &str = "kind is immutable";
const DEPT_PARENT_SITE: &str = "department parent must be a site";
const TEAM_PARENT_KIND: &str = "team parent must be a department or team";
const PARENT_IN_ORG: &str = "parent_id must refer to an OrgUnit in this organization";
const PARENT_NOT_SELF: &str = "parent_id must not be self";
const PARENT_CYCLE: &str = "parent_id must not form a cycle";

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

async fn fixture(owner_pool: &PgPool) -> (OrgId, UserId, PgOrgUnitPort) {
    let actor = seed_org_and_super_admin(owner_pool, ORG, "org-unit-kinds").await;
    let runtime_pool = runtime_role_pool(owner_pool).await;
    let port = PgOrgUnitPort::new(runtime_pool, tokio::runtime::Handle::current());
    (OrgId::from_uuid(ORG), actor, port)
}

async fn execute(
    port: &PgOrgUnitPort,
    command: OrgUnitCommand,
) -> Result<CommandReceipt, OrgUnitError> {
    let port = port.clone();
    tokio::task::spawn_blocking(move || port.execute(&command))
        .await
        .unwrap()
}

fn create(attributes: Value) -> OrgUnitQuery {
    OrgUnitQuery::Create {
        source: None,
        attributes,
    }
}

fn revise(org_unit_id: Uuid, attributes: Value) -> OrgUnitQuery {
    OrgUnitQuery::Revise {
        org_unit_id,
        source: None,
        attributes,
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

fn preflight_blockers(query: &OrgUnitQuery) -> Vec<String> {
    let preflight = <PgOrgUnitPort as CanonicalPort>::preflight(query);
    assert!(
        !preflight.is_ok(),
        "expected blocked preflight, got ok for {query:?}"
    );
    preflight.blockers().to_vec()
}

fn site(name: &str) -> Value {
    json!({ "name": name, "kind": "site" })
}

fn department(name: &str, parent_id: Uuid) -> Value {
    json!({ "name": name, "kind": "department", "parent_id": parent_id })
}

fn team(name: &str, parent_id: Uuid) -> Value {
    json!({ "name": name, "kind": "team", "parent_id": parent_id })
}

async fn create_unit(port: &PgOrgUnitPort, org: OrgId, actor: UserId, attributes: Value) -> Uuid {
    let receipt = execute(port, command(org, actor, create(attributes)))
        .await
        .unwrap();
    unit_of(&receipt)
}

fn assert_blocked(err: OrgUnitError, expected: &[&str]) {
    match err {
        OrgUnitError::Blocked(blockers) => {
            assert_eq!(
                blockers,
                expected.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
            );
        }
        other => panic!("expected OrgUnitError::Blocked, got {other:?}"),
    }
}

#[test]
fn t1_create_without_kind_is_blocked() {
    assert_eq!(
        preflight_blockers(&create(json!({ "name": "본사" }))),
        [KIND_REQUIRED.to_owned()]
    );
}

#[test]
fn t2_blank_kind_is_blocked() {
    assert_eq!(
        preflight_blockers(&create(json!({ "name": "본사", "kind": "  " }))),
        [KIND_EMPTY.to_owned()]
    );
}

#[test]
fn t3_unknown_kind_is_blocked() {
    assert_eq!(
        preflight_blockers(&create(json!({ "name": "본사", "kind": "division" }))),
        [KIND_CLOSED.to_owned()]
    );
}

#[test]
fn t4_uppercase_site_is_blocked() {
    assert_eq!(
        preflight_blockers(&create(json!({ "name": "본사", "kind": "SITE" }))),
        [KIND_CLOSED.to_owned()]
    );
}

#[test]
fn t5_site_with_parent_id_is_blocked() {
    assert_eq!(
        preflight_blockers(&create(json!({
            "name": "본사",
            "kind": "site",
            "parent_id": "0c510000-0000-0000-0000-0000000000aa"
        }))),
        [SITE_NO_PARENT.to_owned()]
    );
}

#[test]
fn t6_department_without_parent_id_is_blocked() {
    assert_eq!(
        preflight_blockers(&create(json!({ "name": "영업", "kind": "department" }))),
        [DEPT_PARENT_REQUIRED.to_owned()]
    );
}

#[test]
fn t7_team_without_parent_id_is_blocked() {
    assert_eq!(
        preflight_blockers(&create(json!({ "name": "백엔드", "kind": "team" }))),
        [TEAM_PARENT_REQUIRED.to_owned()]
    );
}

#[test]
fn t8_parent_id_must_be_a_uuid() {
    assert_eq!(
        preflight_blockers(&create(json!({
            "name": "영업",
            "kind": "department",
            "parent_id": "not-a-uuid"
        }))),
        [PARENT_UUID.to_owned()]
    );
}

#[test]
fn t9_missing_name_and_kind_report_both() {
    assert_eq!(
        preflight_blockers(&create(json!({}))),
        ["name is required".to_owned(), KIND_REQUIRED.to_owned()]
    );
}

#[test]
fn t10_revise_without_kind_is_blocked() {
    assert_eq!(
        preflight_blockers(&revise(
            Uuid::from_u128(0x0c51_0000_0000_0000_0000_0000_0000_00aa),
            json!({ "name": "본사" })
        )),
        [KIND_REQUIRED.to_owned()]
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn t11_kind_is_immutable_on_revise(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let site_id = create_unit(&port, org, actor, site("본사")).await;
    let other = create_unit(&port, org, actor, site("공장")).await;
    let err = execute(
        &port,
        command(org, actor, revise(site_id, department("영업", other))),
    )
    .await
    .unwrap_err();
    assert_blocked(err, &[KIND_IMMUTABLE]);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn t12_department_parent_must_be_a_site(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let site_id = create_unit(&port, org, actor, site("본사")).await;
    let dept_id = create_unit(&port, org, actor, department("영업", site_id)).await;
    let team_id = create_unit(&port, org, actor, team("백엔드", dept_id)).await;
    let err = execute(
        &port,
        command(org, actor, create(department("기획", team_id))),
    )
    .await
    .unwrap_err();
    assert_blocked(err, &[DEPT_PARENT_SITE]);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn t13_team_parent_must_not_be_a_site(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let site_id = create_unit(&port, org, actor, site("본사")).await;
    let err = execute(&port, command(org, actor, create(team("백엔드", site_id))))
        .await
        .unwrap_err();
    assert_blocked(err, &[TEAM_PARENT_KIND]);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn t14_unknown_parent_is_blocked(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let missing = Uuid::from_u128(0x0c51_ffff_0000_0000_0000_0000_0000_0001);
    let err = execute(
        &port,
        command(org, actor, create(department("영업", missing))),
    )
    .await
    .unwrap_err();
    assert_blocked(err, &[PARENT_IN_ORG]);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn t15_foreign_parent_is_blocked(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let foreign_actor = seed_org_and_super_admin(&owner_pool, FOREIGN_ORG, "foreign-kinds").await;
    let foreign_unit: Uuid =
        sqlx::query_scalar("INSERT INTO org_units (org_id) VALUES ($1) RETURNING id")
            .bind(FOREIGN_ORG)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO org_unit_revisions \
         (org_id, org_unit_id, version, command_id, actor_id, payload_digest, attributes, receipt) \
         VALUES ($1, $2, 1, gen_random_uuid(), $3, $4, $5::jsonb, '{}'::jsonb)",
    )
    .bind(FOREIGN_ORG)
    .bind(foreign_unit)
    .bind(*foreign_actor.as_uuid())
    .bind([0_u8; 32].as_slice())
    .bind(site("외국본사"))
    .execute(&owner_pool)
    .await
    .unwrap();

    let err = execute(
        &port,
        command(org, actor, create(department("영업", foreign_unit))),
    )
    .await
    .unwrap_err();
    assert_blocked(err, &[PARENT_IN_ORG]);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn t16_self_parent_on_revise_is_blocked(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let site_id = create_unit(&port, org, actor, site("본사")).await;
    let dept_id = create_unit(&port, org, actor, department("영업", site_id)).await;
    let err = execute(
        &port,
        command(org, actor, revise(dept_id, department("영업", dept_id))),
    )
    .await
    .unwrap_err();
    assert_blocked(err, &[PARENT_NOT_SELF]);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn t17_department_parent_must_not_be_a_department(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let site_id = create_unit(&port, org, actor, site("본사")).await;
    let dept_id = create_unit(&port, org, actor, department("영업", site_id)).await;
    let err = execute(
        &port,
        command(org, actor, create(department("기획", dept_id))),
    )
    .await
    .unwrap_err();
    assert_blocked(err, &[DEPT_PARENT_SITE]);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn t18_length_two_team_parent_cycle_on_revise_is_blocked(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let site_id = create_unit(&port, org, actor, site("본사")).await;
    let dept_id = create_unit(&port, org, actor, department("영업", site_id)).await;
    let team_a = create_unit(&port, org, actor, team("팀A", dept_id)).await;
    let team_b = create_unit(&port, org, actor, team("팀B", team_a)).await;
    let err = execute(
        &port,
        command(org, actor, revise(team_a, team("팀A", team_b))),
    )
    .await
    .unwrap_err();
    assert_blocked(err, &[PARENT_CYCLE]);
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn t19_length_three_team_parent_cycle_on_revise_is_blocked(owner_pool: PgPool) {
    let (org, actor, port) = fixture(&owner_pool).await;
    let site_id = create_unit(&port, org, actor, site("본사")).await;
    let dept_id = create_unit(&port, org, actor, department("영업", site_id)).await;
    let team_a = create_unit(&port, org, actor, team("팀A", dept_id)).await;
    let team_b = create_unit(&port, org, actor, team("팀B", team_a)).await;
    let team_c = create_unit(&port, org, actor, team("팀C", team_b)).await;
    let err = execute(
        &port,
        command(org, actor, revise(team_a, team("팀A", team_c))),
    )
    .await
    .unwrap_err();
    assert_blocked(err, &[PARENT_CYCLE]);
}
