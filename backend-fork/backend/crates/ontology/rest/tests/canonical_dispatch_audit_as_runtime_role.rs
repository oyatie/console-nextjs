#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RUNTIME proof that a canonical `projected_usecase` success path emits an
//! org-scoped `ontology.canonical.execute` audit row (distinct from the
//! InstanceRevision writeback's `ontology.action.execute`). Canonical ports open
//! raw `pool.begin()` and do not write `audit_events` themselves — the engine
//! must.

use std::collections::BTreeSet;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use console_governance_adapter_postgres::PgGovernanceStore;
use console_kernel_core::{AuditAction, AuditEvent, OrgId, TraceContext, UserId};
use console_ontology_adapter_postgres::instances::PgInstanceStore;
use console_ontology_adapter_postgres::{
    ActionTypeInput, CreateObjectTypeDraft, PgOntologyStore, PropertyDefInput,
};
use console_ontology_canonical_adapter_postgres::catalog;
use console_ontology_canonical_adapter_postgres::company::PgCompanyPort;
use console_ontology_canonical_adapter_postgres::employment::{
    NewEmployeeRecord, PgEmploymentPort, insert_employee_record,
};
use console_ontology_canonical_adapter_postgres::job_position::PgJobPositionPort;
use console_ontology_canonical_adapter_postgres::org_unit::PgOrgUnitPort;
use console_ontology_canonical_adapter_postgres::person::PgPersonPort;
use console_ontology_canonical_domain::{
    CanonicalObject, CanonicalPort, CanonicalQuery, CommandId, CommandReceipt as CanonicalReceipt,
    Company, DispatchTarget, Preflight, ReceiptOwner,
};
use console_ontology_domain::{ActionDispatch, BackingKind, InstanceId, ObjectTypeId};
use console_ontology_rest::{ActionCommand, OntologyRestState, ProjectedDispatchRegistry};
use console_payroll_adapter_postgres::pay_run::PgPayRunPort;
use console_platform_authz::{Principal, Role};
use console_platform_db::{DbError, with_audit};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

const AT: OffsetDateTime = datetime!(2026-07-10 12:00 UTC);
const PROMOTE_AT: OffsetDateTime = datetime!(2026-07-11 12:00 UTC);
const TRANSFER_AT: OffsetDateTime = datetime!(2026-07-12 12:00 UTC);
const DISPATCH_TARGET: &str = "company.revise";

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

fn super_admin(user_id: UserId, org: OrgId) -> Principal {
    Principal::new(
        user_id,
        org,
        BTreeSet::from([Role::SuperAdmin]),
        console_kernel_core::BranchScope::All,
    )
}

#[derive(Debug, Deserialize)]
struct EchoQuery {
    target: String,
}

impl CanonicalQuery for EchoQuery {
    fn dispatch_target(&self) -> DispatchTarget {
        self.target
            .parse()
            .expect("dispatcher injects a roster member")
    }

    // company.revise is tenant-scoped (`org_id`), not a payload row id — same as
    // CompanyQuery after h3e deleted the CanonicalQuery::subject_id default.
    fn subject_id(&self) -> Option<Uuid> {
        None
    }
}

/// Records what reached the port and writes nothing — isolates the engine's
/// audit obligation from any adapter-side audit (ports use raw `begin()`).
struct StubPort<O> {
    seen: Arc<Mutex<Vec<String>>>,
    object: PhantomData<O>,
}

impl<O> StubPort<O> {
    fn new(seen: &Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            seen: Arc::clone(seen),
            object: PhantomData,
        }
    }
}

impl<O> CanonicalPort for StubPort<O>
where
    O: CanonicalObject + Send + Sync + 'static,
{
    type Object = O;
    type Query = EchoQuery;
    type Command = (OrgId, CommandId, UserId, DispatchTarget);
    type Error = std::convert::Infallible;

    fn preflight(_query: &Self::Query) -> Preflight {
        Preflight::ok()
    }

    fn command(
        org_id: OrgId,
        command_id: CommandId,
        actor_id: UserId,
        query: Self::Query,
        _action_key: &str,
        _object_type_id: uuid::Uuid,
    ) -> Self::Command {
        (org_id, command_id, actor_id, query.dispatch_target())
    }

    fn execute(&self, command: &Self::Command) -> Result<CanonicalReceipt, Self::Error> {
        let (org_id, command_id, actor_id, target) = command;
        self.seen
            .lock()
            .expect("stub mutex")
            .push(target.as_str().to_owned());
        Ok(CanonicalReceipt::new(
            *org_id,
            *command_id,
            ReceiptOwner::Canonical(<O as CanonicalObject>::KEY),
            *target,
            *actor_id,
            [0_u8; 32],
            json!({ "stub": true }),
            OffsetDateTime::UNIX_EPOCH,
        ))
    }
}

async fn seed_canonical_projected_action(
    owner_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    key: &str,
    action_key: &str,
) -> ObjectTypeId {
    console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(owner_pool.clone())
            .with_command_pool(command_role_pool(owner_pool).await);
        let draft = CreateObjectTypeDraft {
            stable_key: key.to_owned(),
            title: "회사".to_owned(),
            title_property_key: None,
            backing_kind: BackingKind::Projected,
            backing_table: Some("company_revisions".to_owned()),
            primary_key_property: Some("org_id".to_owned()),
            properties: Vec::new(),
            links: Vec::new(),
            actions: vec![ActionTypeInput {
                stable_key: action_key.to_owned(),
                title: "회사 개정".to_owned(),
                params_schema: json!({}),
                edits: json!([]),
                submission_criteria: json!([]),
                side_effects: json!([]),
                dispatch: ActionDispatch::ProjectedUsecase,
                dispatch_target: Some(DISPATCH_TARGET.to_owned()),
                control_points: json!(["authority"]),
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

async fn count_execute_audits(owner_pool: &PgPool, org: OrgId) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE org_id = $1 AND action = 'ontology.canonical.execute'",
    )
    .bind(*org.as_uuid())
    .fetch_one(owner_pool)
    .await
    .unwrap()
}

fn state_with_company_stub(
    pool: &PgPool,
    command_pool: &PgPool,
    seen: &Arc<Mutex<Vec<String>>>,
) -> OntologyRestState {
    let registry = ProjectedDispatchRegistry::new().register_port(StubPort::<Company>::new(seen));
    OntologyRestState::new(
        PgOntologyStore::new(pool.clone()).with_command_pool(command_pool.clone()),
        PgInstanceStore::new(pool.clone()),
        PgGovernanceStore::new(pool.clone()),
        None,
    )
    .with_projected_dispatch(registry)
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn canonical_projected_success_emits_ontology_canonical_execute_audit(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid, "a").await;
    let actor = seed_user(&owner_pool, org_uuid, "a").await;
    let type_id =
        seed_canonical_projected_action(&owner_pool, org, actor, "company.proj", "revise").await;

    let seen = Arc::new(Mutex::new(Vec::new()));
    let command_id = Uuid::new_v4();
    let outcome = console_platform_request_context::scope_org(org, async {
        state_with_company_stub(&rt, &cmd, &seen)
            .execute_action(
                &super_admin(actor, org),
                "revise",
                ActionCommand {
                    object_type_id: type_id,
                    instance_id: None,
                    title: None,
                    params: json!({}),
                    reason: Some("canonical audit probe".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(command_id),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("canonical projected dispatch must succeed");

    assert!(outcome.instance.is_none());
    assert_eq!(
        outcome.projected.as_ref().and_then(|v| v.get("target")),
        Some(&Value::String(DISPATCH_TARGET.to_owned()))
    );
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[DISPATCH_TARGET.to_owned()],
        "stub port must have executed"
    );

    let audits = count_execute_audits(&owner_pool, org).await;
    assert_eq!(
        audits, 1,
        "canonical projected success must write one ontology.canonical.execute audit_events row; got {audits}"
    );

    let target_id: String = sqlx::query_scalar(
        "SELECT target_id FROM audit_events \
         WHERE org_id = $1 AND action = 'ontology.canonical.execute' \
         ORDER BY created_at, id LIMIT 1",
    )
    .bind(org_uuid)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        target_id,
        command_id.to_string(),
        "audit target_id must be the tenant-global command_id"
    );
}

/// Foundry action types for the canonical objects: catalog properties on the
/// published type, `projected_usecase` dispatch to the owning port, and a real
/// `console_rt` write. Stable keys are prefixed so they do not collide with
/// the instance-backed `company_conformance` fixtures. Person is registered
/// with the other five ports (the composition root already does). OrgUnit
/// seeds `create_org_unit` and `revise_org_unit`; JobPosition and Person seed
/// create and revise. JobPosition revise covers a title-only append and a
/// reorganisation move of the mutable head. Person revise covers a name
/// append and a further employee binding to the same natural person.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn seeded_org_actions_write_canonical_heads_through_owning_ports(owner_pool: PgPool) {
    let rt = runtime_role_pool(&owner_pool).await;
    let cmd = command_role_pool(&owner_pool).await;
    let org = OrgId::knl();
    let org_uuid = *org.as_uuid();
    seed_org(&owner_pool, org_uuid, "foundry-org").await;
    let actor = seed_user(&owner_pool, org_uuid, "foundry-org").await;
    let decider = seed_user(&owner_pool, org_uuid, "foundry-payroll-decider").await;

    let company_type = seed_canonical_org_action(
        &owner_pool,
        org,
        actor,
        "canonical.company",
        "회사",
        "company_revisions",
        "org_id",
        catalog::COMPANY_LEGAL_NAME,
        "법인명",
        "revise",
        DispatchTarget::CompanyRevise.as_str(),
    )
    .await;
    let unit_type = seed_canonical_org_actions(
        &owner_pool,
        org,
        actor,
        "canonical.org_unit",
        "조직",
        "org_units",
        "id",
        catalog::ORG_UNIT_NAME,
        "조직명",
        &[
            (
                "create_org_unit",
                DispatchTarget::OrganizationCreateOrgUnit.as_str(),
            ),
            (
                "revise_org_unit",
                DispatchTarget::OrganizationReviseOrgUnit.as_str(),
            ),
        ],
    )
    .await;
    let position_type = seed_canonical_org_actions(
        &owner_pool,
        org,
        actor,
        "canonical.job_position",
        "직위",
        "job_positions",
        "id",
        catalog::JOB_POSITION_TITLE,
        "직위명",
        &[
            (
                "create_job_position",
                DispatchTarget::OrganizationCreateJobPosition.as_str(),
            ),
            (
                "revise_job_position",
                DispatchTarget::OrganizationReviseJobPosition.as_str(),
            ),
        ],
    )
    .await;
    let person_type = seed_canonical_org_actions(
        &owner_pool,
        org,
        actor,
        "canonical.person",
        "사람",
        "persons",
        "id",
        "legal_name",
        "성명",
        &[
            ("create_person", DispatchTarget::PeopleCreatePerson.as_str()),
            ("revise_person", DispatchTarget::PeopleRevisePerson.as_str()),
        ],
    )
    .await;
    let employment_type = seed_canonical_org_actions(
        &owner_pool,
        org,
        actor,
        "canonical.employment",
        "고용",
        "employment_heads",
        "id",
        "company",
        "회사",
        &[
            ("appoint", DispatchTarget::HrAppoint.as_str()),
            ("promote", DispatchTarget::HrPromote.as_str()),
            ("transfer", DispatchTarget::HrTransfer.as_str()),
        ],
    )
    .await;
    let pay_run_type = seed_canonical_org_actions(
        &owner_pool,
        org,
        actor,
        "canonical.pay_run",
        "급여",
        "payroll_draft_runs",
        "id",
        "label",
        "라벨",
        &[
            ("create_run", DispatchTarget::PayrollCreateRun.as_str()),
            ("submit_run", DispatchTarget::PayrollSubmitRun.as_str()),
            ("decide_run", DispatchTarget::PayrollDecideRun.as_str()),
        ],
    )
    .await;

    let handle = tokio::runtime::Handle::current();
    let employment_port = PgEmploymentPort::new(rt.clone(), handle.clone());
    let registry = ProjectedDispatchRegistry::new()
        .register_port(PgCompanyPort::new(rt.clone(), handle.clone()))
        .register_port(PgOrgUnitPort::new(rt.clone(), handle.clone()))
        .register_port(PgJobPositionPort::new(rt.clone(), handle.clone()))
        .register_port(PgPersonPort::new(rt.clone(), handle.clone()))
        .register_port(employment_port.clone())
        .register_port(PgPayRunPort::new(rt.clone(), handle.clone()));
    let state = OntologyRestState::new(
        PgOntologyStore::new(rt.clone()).with_command_pool(cmd),
        PgInstanceStore::new(rt.clone()),
        PgGovernanceStore::new(rt.clone()),
        None,
    )
    .with_projected_dispatch(registry);
    let principal = super_admin(actor, org);

    let company_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "revise",
                ActionCommand {
                    object_type_id: company_type,
                    instance_id: None,
                    title: None,
                    params: json!({ "attributes": { "legal_name": "주식회사 아크메" } }),
                    reason: Some("foundry org setup".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("company.revise through the seeded action must succeed");
    assert_eq!(
        company_outcome
            .projected
            .as_ref()
            .and_then(|value| value.get("target")),
        Some(&Value::String(
            DispatchTarget::CompanyRevise.as_str().to_owned()
        ))
    );

    let unit_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "create_org_unit",
                ActionCommand {
                    object_type_id: unit_type,
                    instance_id: None,
                    title: None,
                    params: json!({ "attributes": { "name": "영업본부", "kind": "site" } }),
                    reason: Some("foundry org setup".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("organization.create_org_unit through the seeded action must succeed");
    let org_unit_id = unit_outcome.projected.as_ref().and_then(|value| {
        value
            .get("result")
            .and_then(|result| result.get("org_unit_id"))
            .and_then(Value::as_str)
            .and_then(|raw| Uuid::parse_str(raw).ok())
    });
    let org_unit_id = org_unit_id.expect("create_org_unit receipt must name org_unit_id");

    let version_one = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT attributes FROM org_unit_revisions \
         WHERE org_id = $1 AND org_unit_id = $2 AND version = 1",
    )
    .bind(org_uuid)
    .bind(org_unit_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(version_one, json!({ "name": "영업본부", "kind": "site" }));

    let revise_unit = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "revise_org_unit",
                ActionCommand {
                    object_type_id: unit_type,
                    instance_id: Some(InstanceId::from_uuid(org_unit_id)),
                    title: None,
                    params: json!({
                        "org_unit_id": org_unit_id,
                        "attributes": { "name": "영업1본부", "kind": "site" }
                    }),
                    reason: Some("foundry org unit revise".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("organization.revise_org_unit through the seeded action must succeed");
    assert_eq!(
        revise_unit
            .projected
            .as_ref()
            .and_then(|value| value.get("target")),
        Some(&Value::String(
            DispatchTarget::OrganizationReviseOrgUnit
                .as_str()
                .to_owned()
        ))
    );
    assert_eq!(
        revise_unit.projected.as_ref().and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("org_unit_id"))
        }),
        Some(&Value::String(org_unit_id.to_string()))
    );
    assert_eq!(
        revise_unit.projected.as_ref().and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("version"))
                .and_then(Value::as_i64)
        }),
        Some(2)
    );
    let version_one_after = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT attributes FROM org_unit_revisions \
         WHERE org_id = $1 AND org_unit_id = $2 AND version = 1",
    )
    .bind(org_uuid)
    .bind(org_unit_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        version_one_after, version_one,
        "appending a revision must not rewrite revision 1"
    );
    let version_two = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT attributes FROM org_unit_revisions \
         WHERE org_id = $1 AND org_unit_id = $2 AND version = 2",
    )
    .bind(org_uuid)
    .bind(org_unit_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(version_two, json!({ "name": "영업1본부", "kind": "site" }));

    let position_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "create_job_position",
                ActionCommand {
                    object_type_id: position_type,
                    instance_id: None,
                    title: None,
                    params: json!({
                        "org_unit_id": org_unit_id,
                        "attributes": { "title": "백엔드 엔지니어" }
                    }),
                    reason: Some("foundry org setup".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("organization.create_job_position through the seeded action must succeed");
    assert_eq!(
        position_outcome.projected.as_ref().and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("org_unit_id"))
        }),
        Some(&Value::String(org_unit_id.to_string()))
    );
    let job_position_id = position_outcome
        .projected
        .as_ref()
        .and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("job_position_id"))
                .and_then(Value::as_str)
                .and_then(|raw| Uuid::parse_str(raw).ok())
        })
        .expect("create_job_position receipt must name job_position_id");

    let position_v1 = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT attributes FROM job_position_revisions \
         WHERE org_id = $1 AND job_position_id = $2 AND version = 1",
    )
    .bind(org_uuid)
    .bind(job_position_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(position_v1, json!({ "title": "백엔드 엔지니어" }));

    let revise_position = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "revise_job_position",
                ActionCommand {
                    object_type_id: position_type,
                    instance_id: Some(InstanceId::from_uuid(job_position_id)),
                    title: None,
                    params: json!({
                        "job_position_id": job_position_id,
                        "attributes": { "title": "백엔드 엔지니어(개정)" }
                    }),
                    reason: Some("foundry job position revise".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("organization.revise_job_position through the seeded action must succeed");
    assert_eq!(
        revise_position
            .projected
            .as_ref()
            .and_then(|value| value.get("target")),
        Some(&Value::String(
            DispatchTarget::OrganizationReviseJobPosition
                .as_str()
                .to_owned()
        ))
    );
    assert_eq!(
        revise_position.projected.as_ref().and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("job_position_id"))
        }),
        Some(&Value::String(job_position_id.to_string()))
    );
    assert_eq!(
        revise_position.projected.as_ref().and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("org_unit_id"))
        }),
        Some(&Value::String(org_unit_id.to_string())),
        "a title-only revise must leave the position on its unit"
    );
    assert_eq!(
        revise_position.projected.as_ref().and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("version"))
                .and_then(Value::as_i64)
        }),
        Some(2)
    );
    let position_v1_after = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT attributes FROM job_position_revisions \
         WHERE org_id = $1 AND job_position_id = $2 AND version = 1",
    )
    .bind(org_uuid)
    .bind(job_position_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        position_v1_after, position_v1,
        "appending a revision must not rewrite revision 1"
    );
    let position_v2 = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT attributes FROM job_position_revisions \
         WHERE org_id = $1 AND job_position_id = $2 AND version = 2",
    )
    .bind(org_uuid)
    .bind(job_position_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(position_v2, json!({ "title": "백엔드 엔지니어(개정)" }));
    let head_unit: Uuid =
        sqlx::query_scalar("SELECT org_unit_id FROM job_positions WHERE org_id = $1 AND id = $2")
            .bind(org_uuid)
            .bind(job_position_id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(head_unit, org_unit_id);

    let legal_name: serde_json::Value = sqlx::query_scalar(
        "SELECT attributes FROM company_revisions \
         WHERE org_id = $1 AND version = (SELECT MAX(version) FROM company_revisions WHERE org_id = $1)",
    )
    .bind(org_uuid)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(legal_name, json!({ "legal_name": "주식회사 아크메" }));

    let unit_head: serde_json::Value = sqlx::query_scalar(
        "SELECT attributes FROM org_unit_revisions \
         WHERE org_id = $1 AND org_unit_id = $2 \
           AND version = (SELECT MAX(version) FROM org_unit_revisions \
                          WHERE org_id = $1 AND org_unit_id = $2)",
    )
    .bind(org_uuid)
    .bind(org_unit_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(unit_head, json!({ "name": "영업1본부", "kind": "site" }));

    let instances: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ont_instances WHERE org_id = $1")
        .bind(org_uuid)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
    assert_eq!(
        instances, 0,
        "canonical org writes must not create ont_instances rows (arch §9.3)"
    );

    let employee_id = Uuid::new_v4();
    let unit_text = org_unit_id.to_string();
    let position_text = job_position_id.to_string();
    let mut tx = rt.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org_uuid.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    insert_employee_record(
        &mut tx,
        org_uuid,
        NewEmployeeRecord {
            employee_id,
            company: "ACME",
            name: "김직원",
            employee_number: "E-FOUNDARY",
            org_unit: &unit_text,
            position: &position_text,
            worksite_name: "서울",
        },
    )
    .await
    .expect("console_rt must insert the Employment-owned employee row");
    tx.commit().await.unwrap();

    let person_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "create_person",
                ActionCommand {
                    object_type_id: person_type,
                    instance_id: None,
                    title: None,
                    params: json!({
                        "employee_id": employee_id,
                        "attributes": { "legal_name": "김직원" }
                    }),
                    reason: Some("foundry person bind".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("people.create_person through the seeded action must succeed");
    assert_eq!(
        person_outcome
            .projected
            .as_ref()
            .and_then(|value| value.get("target")),
        Some(&Value::String(
            DispatchTarget::PeopleCreatePerson.as_str().to_owned()
        ))
    );
    let person_id = person_outcome
        .projected
        .as_ref()
        .and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("person_id"))
                .and_then(Value::as_str)
                .and_then(|raw| Uuid::parse_str(raw).ok())
        })
        .expect("create_person receipt must name person_id");
    assert_eq!(
        person_id, employee_id,
        "a uniquely-resolved person is bound with person_id = employee_id"
    );

    let version_one = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT attributes FROM person_revisions \
         WHERE org_id = $1 AND person_id = $2 AND version = 1",
    )
    .bind(org_uuid)
    .bind(person_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(version_one, json!({ "legal_name": "김직원" }));

    let revise_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "revise_person",
                ActionCommand {
                    object_type_id: person_type,
                    instance_id: Some(InstanceId::from_uuid(person_id)),
                    title: None,
                    params: json!({
                        "person_id": person_id,
                        "attributes": { "legal_name": "김직원(개명)" }
                    }),
                    reason: Some("foundry person revise".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("people.revise_person through the seeded action must succeed");
    assert_eq!(
        revise_outcome
            .projected
            .as_ref()
            .and_then(|value| value.get("target")),
        Some(&Value::String(
            DispatchTarget::PeopleRevisePerson.as_str().to_owned()
        ))
    );
    assert_eq!(
        revise_outcome.projected.as_ref().and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("person_id"))
        }),
        Some(&Value::String(person_id.to_string()))
    );
    assert_eq!(
        revise_outcome.projected.as_ref().and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("version"))
                .and_then(Value::as_i64)
        }),
        Some(2)
    );

    let version_one_after = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT attributes FROM person_revisions \
         WHERE org_id = $1 AND person_id = $2 AND version = 1",
    )
    .bind(org_uuid)
    .bind(person_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        version_one_after, version_one,
        "appending a revision must not rewrite revision 1"
    );
    let version_two = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT attributes FROM person_revisions \
         WHERE org_id = $1 AND person_id = $2 AND version = 2",
    )
    .bind(org_uuid)
    .bind(person_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(version_two, json!({ "legal_name": "김직원(개명)" }));

    let second_employee_id = Uuid::new_v4();
    let mut second_tx = rt.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.current_org', $1, true)")
        .bind(org_uuid.to_string())
        .execute(&mut *second_tx)
        .await
        .unwrap();
    insert_employee_record(
        &mut second_tx,
        org_uuid,
        NewEmployeeRecord {
            employee_id: second_employee_id,
            company: "ACME",
            name: "김직원",
            employee_number: "E-FOUNDARY-2",
            org_unit: &unit_text,
            position: &position_text,
            worksite_name: "서울",
        },
    )
    .await
    .expect("console_rt must insert the further Employment-owned employee row");
    second_tx.commit().await.unwrap();

    let bind_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "revise_person",
                ActionCommand {
                    object_type_id: person_type,
                    instance_id: Some(InstanceId::from_uuid(person_id)),
                    title: None,
                    params: json!({
                        "person_id": person_id,
                        "employee_id": second_employee_id,
                        "attributes": { "legal_name": "김직원(개명)" }
                    }),
                    reason: Some("foundry person second bind".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("people.revise_person further bind through the seeded action must succeed");
    assert_eq!(
        bind_outcome
            .projected
            .as_ref()
            .and_then(|value| value.get("target")),
        Some(&Value::String(
            DispatchTarget::PeopleRevisePerson.as_str().to_owned()
        ))
    );
    assert_eq!(
        bind_outcome.projected.as_ref().and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("person_id"))
        }),
        Some(&Value::String(person_id.to_string()))
    );
    assert_eq!(
        bind_outcome.projected.as_ref().and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("version"))
                .and_then(Value::as_i64)
        }),
        Some(3)
    );
    assert_ne!(
        second_employee_id, person_id,
        "a further employment need not equal person_id"
    );
    let version_two_after = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT attributes FROM person_revisions \
         WHERE org_id = $1 AND person_id = $2 AND version = 2",
    )
    .bind(org_uuid)
    .bind(person_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        version_two_after, version_two,
        "binding a further employee must not rewrite revision 2"
    );
    let bound_employees: Vec<Uuid> = sqlx::query_scalar(
        "SELECT employee_id FROM employee_person_bindings \
         WHERE org_id = $1 AND person_id = $2 ORDER BY employee_id",
    )
    .bind(org_uuid)
    .bind(person_id)
    .fetch_all(&owner_pool)
    .await
    .unwrap();
    let mut expected_employees = vec![employee_id, second_employee_id];
    expected_employees.sort();
    assert_eq!(bound_employees, expected_employees);

    let appoint_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "appoint",
                ActionCommand {
                    object_type_id: employment_type,
                    instance_id: None,
                    title: None,
                    params: json!({
                        "employee_id": employee_id,
                        "valid_from": "2026-07-10T12:00:00Z",
                        "attributes": {
                            "company": "ACME",
                            "org_unit_id": org_unit_id,
                            "job_position_id": job_position_id,
                            "employment_status": "ACTIVE"
                        }
                    }),
                    reason: Some("foundry hr assignment".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("hr.appoint through the seeded action must succeed");
    assert_eq!(
        appoint_outcome
            .projected
            .as_ref()
            .and_then(|value| value.get("target")),
        Some(&Value::String(
            DispatchTarget::HrAppoint.as_str().to_owned()
        ))
    );
    let employment_id = appoint_outcome
        .projected
        .as_ref()
        .and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("employment_id"))
                .and_then(Value::as_str)
                .and_then(|raw| Uuid::parse_str(raw).ok())
        })
        .expect("hr.appoint receipt must name employment_id");
    let head = {
        let port = employment_port.clone();
        tokio::task::spawn_blocking(move || port.get(org, employment_id))
            .await
            .unwrap()
            .expect("employment get")
            .expect("hr.appoint must produce an open queryable head")
    };
    assert_eq!(head.org_unit_id, Some(org_unit_id));
    assert_eq!(head.job_position_id, Some(job_position_id));
    assert_eq!(
        head.person_id,
        Some(employee_id),
        "hr.appoint on a bound person must expose person_id = employee_id"
    );

    let tech_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "create_org_unit",
                ActionCommand {
                    object_type_id: unit_type,
                    instance_id: None,
                    title: None,
                    params: json!({ "attributes": { "name": "기술본부", "kind": "site" } }),
                    reason: Some("foundry hr promote-transfer".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("second create_org_unit through the seeded action must succeed");
    let tech_id = tech_outcome
        .projected
        .as_ref()
        .and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("org_unit_id"))
                .and_then(Value::as_str)
                .and_then(|raw| Uuid::parse_str(raw).ok())
        })
        .expect("create_org_unit receipt must name org_unit_id");

    let staff_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "create_job_position",
                ActionCommand {
                    object_type_id: position_type,
                    instance_id: None,
                    title: None,
                    params: json!({
                        "org_unit_id": org_unit_id,
                        "attributes": { "title": "인사팀장" }
                    }),
                    reason: Some("foundry job position move".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("staff create_job_position through the seeded action must succeed");
    let staff_id = staff_outcome
        .projected
        .as_ref()
        .and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("job_position_id"))
                .and_then(Value::as_str)
                .and_then(|raw| Uuid::parse_str(raw).ok())
        })
        .expect("create_job_position receipt must name job_position_id");
    let staff_v1 = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT attributes FROM job_position_revisions \
         WHERE org_id = $1 AND job_position_id = $2 AND version = 1",
    )
    .bind(org_uuid)
    .bind(staff_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(staff_v1, json!({ "title": "인사팀장" }));
    let staff_unit_before: Uuid =
        sqlx::query_scalar("SELECT org_unit_id FROM job_positions WHERE org_id = $1 AND id = $2")
            .bind(org_uuid)
            .bind(staff_id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(staff_unit_before, org_unit_id);

    let move_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "revise_job_position",
                ActionCommand {
                    object_type_id: position_type,
                    instance_id: Some(InstanceId::from_uuid(staff_id)),
                    title: None,
                    params: json!({
                        "job_position_id": staff_id,
                        "org_unit_id": tech_id,
                        "attributes": { "title": "인사팀장" }
                    }),
                    reason: Some("foundry job position move".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("organization.revise_job_position move through the seeded action must succeed");
    assert_eq!(
        move_outcome
            .projected
            .as_ref()
            .and_then(|value| value.get("target")),
        Some(&Value::String(
            DispatchTarget::OrganizationReviseJobPosition
                .as_str()
                .to_owned()
        ))
    );
    assert_eq!(
        move_outcome.projected.as_ref().and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("job_position_id"))
        }),
        Some(&Value::String(staff_id.to_string()))
    );
    assert_eq!(
        move_outcome.projected.as_ref().and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("org_unit_id"))
        }),
        Some(&Value::String(tech_id.to_string())),
        "a reorganisation must name the destination unit on the receipt"
    );
    assert_eq!(
        move_outcome.projected.as_ref().and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("version"))
                .and_then(Value::as_i64)
        }),
        Some(2)
    );
    let staff_v1_after = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT attributes FROM job_position_revisions \
         WHERE org_id = $1 AND job_position_id = $2 AND version = 1",
    )
    .bind(org_uuid)
    .bind(staff_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(
        staff_v1_after, staff_v1,
        "moving the head must not rewrite revision 1"
    );
    let staff_unit_after: Uuid =
        sqlx::query_scalar("SELECT org_unit_id FROM job_positions WHERE org_id = $1 AND id = $2")
            .bind(org_uuid)
            .bind(staff_id)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(
        staff_unit_after, tech_id,
        "a reorganisation must move the head"
    );

    let senior_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "create_job_position",
                ActionCommand {
                    object_type_id: position_type,
                    instance_id: None,
                    title: None,
                    params: json!({
                        "org_unit_id": org_unit_id,
                        "attributes": { "title": "시니어 엔지니어" }
                    }),
                    reason: Some("foundry hr promote-transfer".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("senior create_job_position through the seeded action must succeed");
    let senior_id = senior_outcome
        .projected
        .as_ref()
        .and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("job_position_id"))
                .and_then(Value::as_str)
                .and_then(|raw| Uuid::parse_str(raw).ok())
        })
        .expect("create_job_position receipt must name job_position_id");

    let lead_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "create_job_position",
                ActionCommand {
                    object_type_id: position_type,
                    instance_id: None,
                    title: None,
                    params: json!({
                        "org_unit_id": tech_id,
                        "attributes": { "title": "테크 리드" }
                    }),
                    reason: Some("foundry hr promote-transfer".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("lead create_job_position through the seeded action must succeed");
    let lead_id = lead_outcome
        .projected
        .as_ref()
        .and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("job_position_id"))
                .and_then(Value::as_str)
                .and_then(|raw| Uuid::parse_str(raw).ok())
        })
        .expect("create_job_position receipt must name job_position_id");

    let promote_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "promote",
                ActionCommand {
                    object_type_id: employment_type,
                    instance_id: Some(InstanceId::from_uuid(employment_id)),
                    title: None,
                    params: json!({
                        "employment_id": employment_id,
                        "valid_from": "2026-07-11T12:00:00Z",
                        "attributes": {
                            "company": "ACME",
                            "org_unit_id": org_unit_id,
                            "job_position_id": senior_id,
                            "employment_status": "ACTIVE"
                        }
                    }),
                    reason: Some("foundry hr promote".to_owned()),
                    valid_from: Some(PROMOTE_AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("hr.promote through the seeded action must succeed");
    assert_eq!(
        promote_outcome
            .projected
            .as_ref()
            .and_then(|value| value.get("target")),
        Some(&Value::String(
            DispatchTarget::HrPromote.as_str().to_owned()
        ))
    );
    let after_promote = {
        let port = employment_port.clone();
        tokio::task::spawn_blocking(move || port.get(org, employment_id))
            .await
            .unwrap()
            .expect("employment get")
            .expect("hr.promote must produce an open queryable head")
    };
    assert_eq!(after_promote.org_unit_id, Some(org_unit_id));
    assert_eq!(after_promote.job_position_id, Some(senior_id));

    let transfer_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "transfer",
                ActionCommand {
                    object_type_id: employment_type,
                    instance_id: Some(InstanceId::from_uuid(employment_id)),
                    title: None,
                    params: json!({
                        "employment_id": employment_id,
                        "valid_from": "2026-07-12T12:00:00Z",
                        "attributes": {
                            "company": "ACME",
                            "org_unit_id": tech_id,
                            "job_position_id": lead_id,
                            "employment_status": "ACTIVE"
                        }
                    }),
                    reason: Some("foundry hr transfer".to_owned()),
                    valid_from: Some(TRANSFER_AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("hr.transfer through the seeded action must succeed");
    assert_eq!(
        transfer_outcome
            .projected
            .as_ref()
            .and_then(|value| value.get("target")),
        Some(&Value::String(
            DispatchTarget::HrTransfer.as_str().to_owned()
        ))
    );
    let after_transfer = {
        let port = employment_port.clone();
        tokio::task::spawn_blocking(move || port.get(org, employment_id))
            .await
            .unwrap()
            .expect("employment get")
            .expect("hr.transfer must produce an open queryable head")
    };
    assert_eq!(after_transfer.org_unit_id, Some(tech_id));
    assert_eq!(after_transfer.job_position_id, Some(lead_id));

    let run_id = Uuid::new_v4();
    let payroll_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "create_run",
                ActionCommand {
                    object_type_id: pay_run_type,
                    instance_id: Some(InstanceId::from_uuid(run_id)),
                    title: None,
                    params: json!({
                        "run_id": run_id,
                        // `time::Date`'s serde is (year, ordinal), not ISO-8601.
                        "period_start": [2026, 152],
                        "period_end": [2026, 181],
                        "connector": "m2",
                        "job": "payroll_draft"
                    }),
                    reason: Some("foundry payroll create_run".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("payroll.create_run through the seeded action must succeed");
    assert_eq!(
        payroll_outcome
            .projected
            .as_ref()
            .and_then(|value| value.get("target")),
        Some(&Value::String(
            DispatchTarget::PayrollCreateRun.as_str().to_owned()
        ))
    );
    let draft_run_id = payroll_outcome
        .projected
        .as_ref()
        .and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("draft_run_id"))
                .and_then(Value::as_str)
                .and_then(|raw| Uuid::parse_str(raw).ok())
        })
        .expect("payroll.create_run receipt must name draft_run_id");
    let (status, calculation_enabled): (String, bool) = {
        let row = sqlx::query(
            "SELECT status, calculation_enabled FROM payroll_draft_runs \
             WHERE org_id = $1 AND id = $2",
        )
        .bind(org_uuid)
        .bind(draft_run_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        (row.get("status"), row.get("calculation_enabled"))
    };
    assert_eq!(status, "BLOCKED_LEGAL_GATE");
    assert!(
        !calculation_enabled,
        "a staged run must not be calculation-enabled — this path must not compute won"
    );

    // The port has no calculate dispatch target. Submitting requires CALCULATED,
    // so the table owner flips status only — no line math, calculation_enabled
    // stays false.
    sqlx::query("UPDATE payroll_draft_runs SET status = 'CALCULATED' WHERE id = $1")
        .bind(draft_run_id)
        .execute(&owner_pool)
        .await
        .unwrap();

    let submit_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &principal,
                "submit_run",
                ActionCommand {
                    object_type_id: pay_run_type,
                    instance_id: Some(InstanceId::from_uuid(draft_run_id)),
                    title: None,
                    params: json!({ "run_id": draft_run_id }),
                    reason: Some("foundry payroll submit_run".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("payroll.submit_run through the seeded action must succeed");
    assert_eq!(
        submit_outcome
            .projected
            .as_ref()
            .and_then(|value| value.get("target")),
        Some(&Value::String(
            DispatchTarget::PayrollSubmitRun.as_str().to_owned()
        ))
    );
    let submitted = sqlx::query(
        "SELECT status, submitted_by, calculation_enabled FROM payroll_draft_runs \
         WHERE org_id = $1 AND id = $2",
    )
    .bind(org_uuid)
    .bind(draft_run_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(submitted.get::<String, _>("status"), "SUBMITTED");
    assert_eq!(
        submitted.get::<Option<Uuid>, _>("submitted_by"),
        Some(*actor.as_uuid())
    );
    assert!(
        !submitted.get::<bool, _>("calculation_enabled"),
        "submit must not enable calculation"
    );

    let decide_outcome = console_platform_request_context::scope_org(org, async {
        state
            .execute_action(
                &super_admin(decider, org),
                "decide_run",
                ActionCommand {
                    object_type_id: pay_run_type,
                    instance_id: Some(InstanceId::from_uuid(draft_run_id)),
                    title: None,
                    params: json!({
                        "run_id": draft_run_id,
                        "decision": "APPROVE",
                        "reason": "6월 급여 승인"
                    }),
                    reason: Some("foundry payroll decide_run".to_owned()),
                    valid_from: Some(AT),
                    checklist_all_acknowledged: None,
                    four_eyes_request_ref: None,
                    command_id: Some(Uuid::new_v4()),
                    expected_revision: None,
                },
            )
            .await
    })
    .await
    .expect("payroll.decide_run through the seeded action must succeed");
    assert_eq!(
        decide_outcome
            .projected
            .as_ref()
            .and_then(|value| value.get("target")),
        Some(&Value::String(
            DispatchTarget::PayrollDecideRun.as_str().to_owned()
        ))
    );
    let decided = sqlx::query(
        "SELECT status, decided_by, decision_reason, approved_by, calculation_enabled \
         FROM payroll_draft_runs WHERE org_id = $1 AND id = $2",
    )
    .bind(org_uuid)
    .bind(draft_run_id)
    .fetch_one(&owner_pool)
    .await
    .unwrap();
    assert_eq!(decided.get::<String, _>("status"), "APPROVED");
    assert_eq!(
        decided.get::<Option<Uuid>, _>("decided_by"),
        Some(*decider.as_uuid())
    );
    assert_eq!(
        decided
            .get::<Option<String>, _>("decision_reason")
            .as_deref(),
        Some("6월 급여 승인")
    );
    assert_eq!(
        decided.get::<Option<Uuid>, _>("approved_by"),
        Some(*decider.as_uuid())
    );
    assert!(
        !decided.get::<bool, _>("calculation_enabled"),
        "decide must not enable calculation or write won"
    );

    let after_assign: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ont_instances WHERE org_id = $1")
            .bind(org_uuid)
            .fetch_one(&owner_pool)
            .await
            .unwrap();
    assert_eq!(
        after_assign, 0,
        "canonical org/hr/payroll writes must not write ont_instances"
    );
}

#[allow(clippy::too_many_arguments)]
async fn seed_canonical_org_action(
    owner_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    type_key: &str,
    type_title: &str,
    backing_table: &str,
    primary_key_property: &str,
    property_key: &str,
    property_title: &str,
    action_key: &str,
    dispatch_target: &str,
) -> ObjectTypeId {
    seed_canonical_org_actions(
        owner_pool,
        org,
        actor,
        type_key,
        type_title,
        backing_table,
        primary_key_property,
        property_key,
        property_title,
        &[(action_key, dispatch_target)],
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn seed_canonical_org_actions(
    owner_pool: &PgPool,
    org: OrgId,
    actor: UserId,
    type_key: &str,
    type_title: &str,
    backing_table: &str,
    primary_key_property: &str,
    property_key: &str,
    property_title: &str,
    actions: &[(&str, &str)],
) -> ObjectTypeId {
    let type_key = type_key.to_owned();
    let type_title = type_title.to_owned();
    let backing_table = backing_table.to_owned();
    let primary_key_property = primary_key_property.to_owned();
    let property_key = property_key.to_owned();
    let property_title = property_title.to_owned();
    let actions: Vec<(String, String)> = actions
        .iter()
        .map(|(key, target)| ((*key).to_owned(), (*target).to_owned()))
        .collect();
    console_platform_request_context::scope_org(org, async {
        let store = PgOntologyStore::new(owner_pool.clone())
            .with_command_pool(command_role_pool(owner_pool).await);
        let draft = CreateObjectTypeDraft {
            stable_key: type_key,
            title: type_title,
            title_property_key: Some(property_key.clone()),
            backing_kind: BackingKind::Projected,
            backing_table: Some(backing_table),
            primary_key_property: Some(primary_key_property),
            properties: vec![PropertyDefInput {
                key: property_key.clone(),
                title: property_title,
                field_type: "text".to_owned(),
                config: json!({}),
                backing_column: None,
                required: true,
                in_property_policy: false,
            }],
            links: Vec::new(),
            actions: actions
                .into_iter()
                .map(|(action_key, dispatch_target)| ActionTypeInput {
                    stable_key: action_key,
                    title: "저장".to_owned(),
                    // Port query shape (`attributes: { legal_name | name | title }`),
                    // not flattened instance edits. Required-ness is the port catalog.
                    params_schema: json!({}),
                    edits: json!([]),
                    submission_criteria: json!([]),
                    side_effects: json!([]),
                    dispatch: ActionDispatch::ProjectedUsecase,
                    dispatch_target: Some(dispatch_target),
                    control_points: json!(["authority"]),
                })
                .collect(),
            analytics: Vec::new(),
        };
        store
            .create_object_type(actor, draft, TraceContext::generate(), AT)
            .await
            .expect("canonical org object type must be created")
            .id
    })
    .await
}
