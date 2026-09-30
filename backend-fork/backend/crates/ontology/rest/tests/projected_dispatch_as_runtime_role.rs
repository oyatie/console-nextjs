#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proofs for the §18 projected-dispatch path — a `projected_usecase`
//! action routing THROUGH the owning domain crate's use-case (here
//! `registry.update_equipment`), exercised as the genuine non-owner `console_rt` role
//! (NOSUPERUSER, NOBYPASSRLS, FORCE RLS) so RLS org-isolation is really enforced.
//!
//! This is the wiring test for [`ProjectedDispatchRegistry`]: the production
//! handlers live in the App tier (`console-app`), which alone may depend on a domain
//! ADAPTER; the ontology REST tier stays layer-clean (this test's registry-backed
//! handler is a dev-only dependency, which the layer-boundary gate exempts).
//!
//! Proves:
//!   (a) execute of a projected action fires the DOMAIN use-case — the
//!       `registry_equipment` row is mutated by the registry crate AND its own
//!       `equipment.update` audit row lands — while the ontology engine writes
//!       NOTHING of its own (`ont_instances` stays empty: no second source of
//!       truth, arch §9.3);
//!   (b) an unknown `dispatch_target` fails closed (`NotWiredYet`) and mutates
//!       nothing;
//!   (c) a failed §16 gate (four-eyes required, none supplied) denies BEFORE
//!       dispatch — the domain use-case is never called and nothing is written;
//!   (d) a projected action of one tenant is invisible to another (`NotFound`).

use std::collections::BTreeSet;
use std::sync::Arc;

use console_governance_adapter_postgres::PgGovernanceStore;
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, EquipmentId, KernelError, OrgId, TraceContext, UserId,
};
use console_ontology_adapter_postgres::instances::PgInstanceStore;
use console_ontology_adapter_postgres::{ActionTypeInput, CreateObjectTypeDraft, PgOntologyStore};
use console_ontology_domain::{ActionDispatch, BackingKind, InstanceId, ObjectTypeId};
use console_ontology_rest::{
    ActionCommand, ActionError, OntologyRestState, ProjectedDispatch, ProjectedDispatchRegistry,
    ProjectedHandler,
};
use console_platform_authz::{Principal, Role};
use console_platform_db::{DbError, with_audit};
use console_registry_adapter_postgres::{PgRegistryError, PgRegistryStore};
use console_registry_application::{UpdateEquipmentCommand, UpdateEquipmentFields};
use console_registry_domain::EquipmentStatus;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

const ORG_B: Uuid = Uuid::from_u128(0x6666_6666_6666_6666_6666_6666_6666_6666);
const AT: OffsetDateTime = datetime!(2026-07-10 12:00 UTC);
const DISPATCH_TARGET: &str = "registry.update_equipment";

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
        .unwrap()
}

async fn command_role_pool(owner_pool: &PgPool) -> PgPool {
    let options = owner_pool.connect_options().as_ref().clone();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE console_ontology_cmd")
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

// --- seed helpers (org / branch / user / equipment) --------------------------

fn test_audit_event(
    action: &str,
    target_type: &str,
    target_id: impl ToString,
    org: Uuid,
) -> AuditEvent {
    AuditEvent::new(
        None,
        AuditAction::new(action).unwrap(),
        target_type,
        target_id.to_string(),
        TraceContext::generate(),
        OffsetDateTime::now_utc(),
    )
    .with_org(OrgId::from_uuid(org))
}

async fn seed_org(owner_pool: &PgPool, org: Uuid, tag: &str) {
    let event = test_audit_event("test.seed_org", "organization", org, org);
    let tag = tag.to_owned();
    with_audit(owner_pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING",
            )
            .bind(org)
            .bind(format!("org-{}", &org.simple().to_string()[..12]))
            .bind(format!("Org {tag}"))
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
}

async fn seed_branch(owner_pool: &PgPool, org: Uuid) -> BranchId {
    let branch_id = Uuid::new_v4();
    let event = test_audit_event("test.seed_branch", "branch", branch_id, org);
    with_audit(owner_pool, event, |tx| {
        Box::pin(async move {
            let region_id: Uuid = sqlx::query_scalar(
                "INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id",
            )
            .bind(format!("Region {}", Uuid::new_v4()))
            .bind(org)
            .fetch_one(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            sqlx::query(
                "INSERT INTO branches (id, region_id, name, org_id) VALUES ($1, $2, $3, $4)",
            )
            .bind(branch_id)
            .bind(region_id)
            .bind(format!("Branch {}", Uuid::new_v4()))
            .bind(org)
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
    BranchId::from_uuid(branch_id)
}

async fn seed_user(owner_pool: &PgPool, org: Uuid, tag: &str) -> UserId {
    let user_id = UserId::new();
    let event = test_audit_event("test.seed_user", "user", *user_id.as_uuid(), org);
    let tag = tag.to_owned();
    with_audit(owner_pool, event, |tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)",
            )
            .bind(*user_id.as_uuid())
            .bind(format!("Admin {tag}"))
            .bind(["SUPER_ADMIN"].as_slice())
            .bind(org)
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
    user_id
}

/// Seed one equipment row (status 임대) via direct INSERT, mirroring the registry
/// crate's own RLS tests. Returns its id — the projected action's target.
async fn seed_equipment(owner_pool: &PgPool, org: Uuid, branch_id: BranchId) -> EquipmentId {
    let equipment_id = Uuid::new_v4();
    let event = test_audit_event("test.seed_equipment", "equipment", equipment_id, org);
    with_audit(owner_pool, event, |tx| {
        Box::pin(async move {
            let customer_id: Uuid = sqlx::query_scalar(
                "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
            )
            .bind(*branch_id.as_uuid())
            .bind(format!("Customer {}", Uuid::new_v4()))
            .bind(org)
            .fetch_one(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            let site_id: Uuid = sqlx::query_scalar(
                "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) \
                 VALUES ($1, $2, $3, $4) RETURNING id",
            )
            .bind(*branch_id.as_uuid())
            .bind(customer_id)
            .bind(format!("Site {}", Uuid::new_v4()))
            .bind(org)
            .fetch_one(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            let n = Uuid::new_v4().as_u128() % 10_000;
            sqlx::query(
                r#"
                INSERT INTO registry_equipment (
                    id, branch_id, customer_id, site_id, equipment_no, management_no,
                    manufacturer_code, kind_code, power_code, status,
                    specification, ton_text, source_sheet, source_row, org_id
                )
                VALUES ($1, $2, $3, $4, $5, $6, 'A', 'B', 'C', '임대',
                        '좌식', '2.5T', 'projected-dispatch-test', 1, $7)
                "#,
            )
            .bind(equipment_id)
            .bind(*branch_id.as_uuid())
            .bind(customer_id)
            .bind(site_id)
            .bind(format!("ABC12-{n:04}"))
            .bind(format!("MGMT-{}", Uuid::new_v4()))
            .bind(org)
            .execute(tx.as_mut())
            .await
            .map_err(DbError::Sqlx)?;
            Ok::<(), DbError>(())
        })
    })
    .await
    .unwrap();
    EquipmentId::from_uuid(equipment_id)
}

fn super_admin(user_id: UserId, org: OrgId) -> Principal {
    Principal::new(
        user_id,
        org,
        BTreeSet::from([Role::SuperAdmin]),
        console_kernel_core::BranchScope::All,
    )
}

// --- the registry-backed projected handler (mirrors the App-tier wiring) -----

/// Map a registry use-case error onto [`ActionError`] without touching the
/// ontology adapter's error type — the pattern every App-tier handler follows.
fn to_action_error(err: PgRegistryError) -> ActionError {
    match err {
        PgRegistryError::Domain(kernel) => ActionError::domain(kernel),
        PgRegistryError::Db(db) => ActionError::domain(KernelError::internal(db.to_string())),
        PgRegistryError::Workbook(message) => ActionError::domain(KernelError::internal(message)),
    }
}

/// Build a handler that routes `registry.update_equipment` into the registry
/// crate's real `update_equipment` use-case (its own RLS + audit + versioning).
fn update_equipment_handler(store: PgRegistryStore) -> ProjectedHandler {
    Arc::new(move |input: ProjectedDispatch| {
        let store = store.clone();
        Box::pin(async move {
            let equipment_uuid = input.target_id.ok_or_else(|| {
                ActionError::domain(KernelError::validation(
                    "update_equipment requires a target equipment id",
                ))
            })?;
            let status = input
                .params
                .get("status")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ActionError::domain(KernelError::validation("status param is required"))
                })
                .and_then(|s| EquipmentStatus::parse(s).map_err(ActionError::domain))?;

            let audit_event_id = store
                .update_equipment(UpdateEquipmentCommand {
                    actor: input.principal.user_id,
                    branch_scope: input.principal.branch_scope.clone(),
                    equipment_id: EquipmentId::from_uuid(equipment_uuid),
                    fields: UpdateEquipmentFields {
                        status: Some(status),
                        ..UpdateEquipmentFields::default()
                    },
                    trace: TraceContext::generate(),
                    occurred_at: input.occurred_at,
                })
                .await
                .map_err(to_action_error)?;

            Ok(json!({
                "target": input.target,
                "equipment_id": equipment_uuid,
                "audit_event_id": audit_event_id.to_string(),
            }))
        })
    })
}

fn state_with_registry(pool: &PgPool, command_pool: &PgPool) -> OntologyRestState {
    let registry = ProjectedDispatchRegistry::new().register(
        DISPATCH_TARGET,
        update_equipment_handler(PgRegistryStore::new(pool.clone())),
    );
    OntologyRestState::new(
        PgOntologyStore::new(pool.clone()).with_command_pool(command_pool.clone()),
        PgInstanceStore::new(pool.clone()),
        PgGovernanceStore::new(pool.clone()),
        None,
    )
    .with_projected_dispatch(registry)
}

/// Publish a projected object type backed by `registry_equipment` with one
/// `projected_usecase` action (configurable target + control points).
#[allow(clippy::too_many_arguments)]
async fn seed_projected_action(
    owner_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    key: &str,
    action_key: &str,
    dispatch_target: &str,
    control_points: Value,
    submission_criteria: Value,
) -> ObjectTypeId {
    console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(owner_pool.clone())
            .with_command_pool(command_role_pool(owner_pool).await);
        let draft = CreateObjectTypeDraft {
            stable_key: key.to_owned(),
            title: "장비".to_owned(),
            title_property_key: None,
            backing_kind: BackingKind::Projected,
            backing_table: Some("registry_equipment".to_owned()),
            primary_key_property: Some("id".to_owned()),
            properties: Vec::new(),
            links: Vec::new(),
            actions: vec![ActionTypeInput {
                stable_key: action_key.to_owned(),
                title: "장비 상태 변경".to_owned(),
                params_schema: json!({"status": {"required": true}}),
                edits: json!([]),
                submission_criteria,
                side_effects: json!([]),
                dispatch: ActionDispatch::ProjectedUsecase,
                dispatch_target: Some(dispatch_target.to_owned()),
                control_points,
            }],
            analytics: Vec::new(),
        };
        store
            .create_object_type(actor, draft, TraceContext::generate(), AT)
            .await
            .expect("create projected object type")
            .id
    })
    .await
}

async fn equipment_status(owner_pool: &PgPool, equipment_id: EquipmentId) -> String {
    sqlx::query_scalar("SELECT status FROM registry_equipment WHERE id = $1")
        .bind(*equipment_id.as_uuid())
        .fetch_one(owner_pool)
        .await
        .unwrap()
}

async fn count_action_audits(owner_pool: &PgPool, org: OrgId, action: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = $2")
        .bind(*org.as_uuid())
        .bind(action)
        .fetch_one(owner_pool)
        .await
        .unwrap()
}

async fn count_instances(owner_pool: &PgPool, org: OrgId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM ont_instances WHERE org_id = $1")
        .bind(*org.as_uuid())
        .fetch_one(owner_pool)
        .await
        .unwrap()
}

fn dispatch_command(object_type_id: ObjectTypeId, equipment_id: EquipmentId) -> ActionCommand {
    ActionCommand {
        object_type_id,
        instance_id: Some(InstanceId::from_uuid(*equipment_id.as_uuid())),
        title: None,
        params: json!({"status": "예비"}),
        reason: Some("field re-classification".to_owned()),
        valid_from: Some(AT),
        checklist_all_acknowledged: None,
        four_eyes_request_ref: None,
        command_id: None,
        expected_revision: None,
    }
}

// --- tests -------------------------------------------------------------------

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn projected_dispatch_fires_domain_use_case_and_engine_writes_nothing(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid, "a").await;
    let branch = seed_branch(&owner_pool, org_uuid).await;
    let actor = seed_user(&owner_pool, org_uuid, "a").await;
    let equipment_id = seed_equipment(&owner_pool, org_uuid, branch).await;
    let type_id = seed_projected_action(
        &owner_pool,
        org,
        actor,
        "equip.proj",
        "reclassify",
        DISPATCH_TARGET,
        json!(["authority"]),
        json!([]),
    )
    .await;

    let outcome = console_platform_request_context::scope_org(org, async {
        state_with_registry(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "reclassify",
                dispatch_command(type_id, equipment_id),
            )
            .await
    })
    .await
    .expect("projected dispatch must route into the registry use-case");

    // The engine wrote nothing itself; the projected result carries the domain summary.
    assert!(
        outcome.instance.is_none(),
        "no ontology revision for a projected action"
    );
    assert_eq!(
        outcome.projected.as_ref().unwrap()["target"],
        DISPATCH_TARGET
    );

    // The DOMAIN use-case actually mutated its own table + audited it.
    assert_eq!(
        equipment_status(&owner_pool, equipment_id).await,
        "예비",
        "the registry use-case changed the equipment status"
    );
    assert_eq!(
        count_action_audits(&owner_pool, org, "equipment.update").await,
        1,
        "the registry crate's own audit row fired"
    );
    // No second source of truth: the ontology engine created no instance.
    assert_eq!(
        count_instances(&owner_pool, org).await,
        0,
        "a projected dispatch must not write ont_instances"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn unknown_dispatch_target_fails_closed_and_writes_nothing(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid, "a").await;
    let branch = seed_branch(&owner_pool, org_uuid).await;
    let actor = seed_user(&owner_pool, org_uuid, "a").await;
    let equipment_id = seed_equipment(&owner_pool, org_uuid, branch).await;
    // Registered registry handler is "registry.update_equipment"; this action
    // points at an UNMAPPED target.
    let type_id = seed_projected_action(
        &owner_pool,
        org,
        actor,
        "equip.unmapped",
        "reclassify",
        "registry.does_not_exist",
        json!(["authority"]),
        json!([]),
    )
    .await;

    let err = console_platform_request_context::scope_org(org, async {
        state_with_registry(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "reclassify",
                dispatch_command(type_id, equipment_id),
            )
            .await
    })
    .await
    .expect_err("an unmapped dispatch_target must fail closed");
    match err {
        ActionError::NotWiredYet { target } => {
            assert_eq!(target.as_deref(), Some("registry.does_not_exist"));
        }
        other => panic!("expected NotWiredYet, got {other:?}"),
    }
    assert_eq!(equipment_status(&owner_pool, equipment_id).await, "임대");
    assert_eq!(
        count_action_audits(&owner_pool, org, "equipment.update").await,
        0
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn failed_gate_denies_before_dispatch_and_writes_nothing(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid, "a").await;
    let branch = seed_branch(&owner_pool, org_uuid).await;
    let actor = seed_user(&owner_pool, org_uuid, "a").await;
    let equipment_id = seed_equipment(&owner_pool, org_uuid, branch).await;
    // Requires four-eyes; the command supplies no approval ref → deny pre-dispatch.
    let type_id = seed_projected_action(
        &owner_pool,
        org,
        actor,
        "equip.gated",
        "reclassify",
        DISPATCH_TARGET,
        json!(["authority", "four_eyes"]),
        json!([]),
    )
    .await;

    let err = console_platform_request_context::scope_org(org, async {
        state_with_registry(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "reclassify",
                dispatch_command(type_id, equipment_id),
            )
            .await
    })
    .await
    .expect_err("a required-but-unapproved four-eyes gate must deny");
    assert!(matches!(err, ActionError::GateDenied(_)), "got {err:?}");
    // The domain use-case was never called: nothing changed, nothing audited.
    assert_eq!(equipment_status(&owner_pool, equipment_id).await, "임대");
    assert_eq!(
        count_action_audits(&owner_pool, org, "equipment.update").await,
        0
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn projected_action_is_invisible_across_tenants(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org_a = OrgId::knl();
    let org_b = OrgId::from_uuid(ORG_B);
    seed_org(&owner_pool, *org_a.as_uuid(), "a").await;
    seed_org(&owner_pool, *org_b.as_uuid(), "b").await;
    let branch_a = seed_branch(&owner_pool, *org_a.as_uuid()).await;
    let actor_a = seed_user(&owner_pool, *org_a.as_uuid(), "a").await;
    let actor_b = seed_user(&owner_pool, *org_b.as_uuid(), "b").await;
    let equipment_a = seed_equipment(&owner_pool, *org_a.as_uuid(), branch_a).await;
    let type_a = seed_projected_action(
        &owner_pool,
        org_a,
        actor_a,
        "equip.a",
        "reclassify",
        DISPATCH_TARGET,
        json!(["authority"]),
        json!([]),
    )
    .await;

    // Under org-B's GUC, org-A's action type does not resolve → NotFound.
    let err = console_platform_request_context::scope_org(org_b, async {
        state_with_registry(&rt, &cmd)
            .execute_action(
                &super_admin(actor_b, org_b),
                "reclassify",
                dispatch_command(type_a, equipment_a),
            )
            .await
    })
    .await
    .expect_err("org-A's projected action must be invisible to org-B");
    assert!(matches!(err, ActionError::NotFound), "got {err:?}");
    assert_eq!(equipment_status(&owner_pool, equipment_a).await, "임대");
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn projected_submission_criteria_fail_closed_and_write_nothing(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid, "a").await;
    let branch = seed_branch(&owner_pool, org_uuid).await;
    let actor = seed_user(&owner_pool, org_uuid, "a").await;
    let equipment_id = seed_equipment(&owner_pool, org_uuid, branch).await;
    // A projected action that declares a submission criterion referencing the
    // target's (unreadable) domain state. In v1 the engine cannot read a projected
    // row, so it must NOT silently evaluate the criterion against an empty base and
    // dispatch — it must fail closed.
    let type_id = seed_projected_action(
        &owner_pool,
        org,
        actor,
        "equip.crit",
        "reclassify",
        DISPATCH_TARGET,
        json!(["authority"]),
        json!([{"field": "locked", "op": "ne", "value": true}]),
    )
    .await;

    let err = console_platform_request_context::scope_org(org, async {
        state_with_registry(&rt, &cmd)
            .execute_action(
                &super_admin(actor, org),
                "reclassify",
                dispatch_command(type_id, equipment_id),
            )
            .await
    })
    .await
    .expect_err("a projected action with submission criteria must fail closed in v1");
    assert!(matches!(err, ActionError::CriteriaFailed(_)), "got {err:?}");
    // The domain use-case was never called: nothing changed, nothing audited.
    assert_eq!(equipment_status(&owner_pool, equipment_id).await, "임대");
    assert_eq!(
        count_action_audits(&owner_pool, org, "equipment.update").await,
        0
    );
}
