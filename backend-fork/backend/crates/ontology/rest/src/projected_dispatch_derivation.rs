//! Projected dispatch is DERIVED over the contract's roster, not enumerated.
//!
//! ADR-0030 §7 row 4: *"Actions on a projected type do not require a
//! hand-written Rust closure per action."* These tests measure the mechanism,
//! which is the only thing that can satisfy that row — wiring thirteen closures
//! would satisfy its letter and violate it exactly.
//!
//! The measurement each test makes:
//!
//! * `every_dispatch_target_in_the_contract_resolves_to_a_port` — totality over
//!   `DispatchTarget::ALL`. It names no target, so a FOURTEENTH target added to
//!   `dispatch_targets!` is covered the moment it exists; it goes RED only if a
//!   target arrives owned by an object with no port, which is the one way the
//!   derivation can be incomplete.
//! * `an_unknown_target_still_fails_closed` and
//!   `a_roster_target_with_no_port_fails_closed` — the property a generic lookup
//!   is most likely to lose. A derivation that accepts anything would be far
//!   worse than the list it replaces.
//! * `a_payload_that_decodes_as_a_different_target_is_refused` — the seam. The
//!   dispatcher injects the CONTRACT's target string and then asks the decoded
//!   query what it actually is; a port whose variants and serde tags ever
//!   disagree is refused instead of writing under the wrong target.
//! * `digest_conflict_from_port_surfaces_as_conflict_not_internal` — call-site
//!   control for the spawn_blocking `map_err`. A port whose Error maps
//!   DigestConflict → Conflict must still surface Conflict through the
//!   dispatcher; reverting to `KernelError::internal(error.to_string())` goes
//!   red here even when adapter `into_kernel_error` unit pins stay green.
//!
//! `EchoQuery` is deliberately target-agnostic: one stub port type serves all
//! six objects and every target, present or future, which is only possible
//! because nothing in the dispatcher names an action.

use super::{ActionError, ProjectedDispatch, ProjectedDispatchRegistry};
use console_kernel_core::{BranchScope, ErrorKind, KernelError, OrgId, UserId};
use console_ontology_adapter_postgres::PgOntologyError;
use console_ontology_canonical_domain::{
    CanonicalObject, CanonicalPort, CanonicalPortError, CanonicalQuery, CommandId,
    CommandReceipt as CanonicalReceipt, Company, DispatchTarget, Employment, JobPosition,
    ObjectKey, OrgUnit, PayRun, Person, Preflight, ReceiptOwner,
};
use console_platform_authz::Principal;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use time::OffsetDateTime;
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0x0AC1);
const ACTOR: Uuid = Uuid::from_u128(0x0AC2);

/// A query that decodes ONLY the tag the dispatcher injects, so one stub can
/// stand in for every port and every target. Optional subject fields mirror
/// revise/pay-run payloads so the `target_id` binding seam can be exercised.
#[derive(Debug, Deserialize)]
struct EchoQuery {
    target: String,
    #[serde(default)]
    job_position_id: Option<Uuid>,
    #[serde(default)]
    org_unit_id: Option<Uuid>,
    #[serde(default)]
    person_id: Option<Uuid>,
    #[serde(default)]
    run_id: Option<Uuid>,
    #[serde(default)]
    employment_id: Option<Uuid>,
}

impl CanonicalQuery for EchoQuery {
    fn dispatch_target(&self) -> DispatchTarget {
        self.target
            .parse()
            .expect("the dispatcher injects a roster member, never a free string")
    }

    fn subject_id(&self) -> Option<Uuid> {
        self.job_position_id
            .or(self.org_unit_id)
            .or(self.person_id)
            .or(self.run_id)
            .or(self.employment_id)
    }
}

/// A query that always claims to be `company.revise`, whatever it decoded from.
/// Stands in for a port whose serde tags and `target()` have drifted apart.
#[derive(Debug, Deserialize)]
struct LyingQuery {
    #[allow(dead_code)]
    target: String,
}

impl CanonicalQuery for LyingQuery {
    fn dispatch_target(&self) -> DispatchTarget {
        DispatchTarget::CompanyRevise
    }

    fn subject_id(&self) -> Option<Uuid> {
        None
    }
}

/// Port Error that preserves DigestConflict as Conflict — the shape a real
/// adapter returns, used only to pin the dispatcher's `map_err` call site.
#[derive(Debug)]
enum StubPortError {
    DigestConflict(Uuid),
}

impl std::fmt::Display for StubPortError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DigestConflict(id) => write!(f, "digest conflict {id}"),
        }
    }
}

impl CanonicalPortError for StubPortError {
    fn into_kernel_error(self) -> KernelError {
        match self {
            Self::DigestConflict(id) => KernelError::conflict(format!("digest conflict {id}")),
        }
    }
}

/// Always fails with DigestConflict so the dispatcher call-site mapping is the
/// only thing under test (no DB, no adapter match arms).
struct DigestConflictPort;

impl CanonicalPort for DigestConflictPort {
    type Object = Company;
    type Query = EchoQuery;
    type Command = (OrgId, CommandId, UserId, DispatchTarget);
    type Error = StubPortError;

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

    fn execute(&self, _command: &Self::Command) -> Result<CanonicalReceipt, Self::Error> {
        Err(StubPortError::DigestConflict(Uuid::nil()))
    }
}

/// Records what reached the port, and writes nothing.
struct StubPort<O, Q> {
    seen: Arc<Mutex<Vec<String>>>,
    object: PhantomData<O>,
    query: PhantomData<Q>,
}

impl<O, Q> StubPort<O, Q> {
    fn new(seen: &Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            seen: Arc::clone(seen),
            object: PhantomData,
            query: PhantomData,
        }
    }
}

impl<O, Q> CanonicalPort for StubPort<O, Q>
where
    O: CanonicalObject + Send + Sync + 'static,
    Q: CanonicalQuery + Send + Sync + 'static,
{
    type Object = O;
    type Query = Q;
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

/// A registry wired the way the composition root wires it: once per OBJECT.
/// Six calls, no target named anywhere.
fn all_six_ports(seen: &Arc<Mutex<Vec<String>>>) -> ProjectedDispatchRegistry {
    ProjectedDispatchRegistry::new()
        .register_port(StubPort::<Company, EchoQuery>::new(seen))
        .register_port(StubPort::<OrgUnit, EchoQuery>::new(seen))
        .register_port(StubPort::<JobPosition, EchoQuery>::new(seen))
        .register_port(StubPort::<Person, EchoQuery>::new(seen))
        .register_port(StubPort::<Employment, EchoQuery>::new(seen))
        .register_port(StubPort::<PayRun, EchoQuery>::new(seen))
}

fn principal() -> Principal {
    Principal::new(
        UserId::from_uuid(ACTOR),
        OrgId::from_uuid(ORG),
        BTreeSet::new(),
        BranchScope::All,
    )
}

fn input(target: &str, params: Value, command_id: Option<Uuid>) -> ProjectedDispatch {
    ProjectedDispatch {
        principal: principal(),
        target: target.to_owned(),
        target_id: None,
        command_id,
        action_key: "revise".to_owned(),
        object_type_id: Uuid::nil(),
        params,
        reason: None,
        occurred_at: OffsetDateTime::UNIX_EPOCH,
    }
}

/// TOTALITY. Sized against `DispatchTarget::ALL` and naming no target, so it
/// cannot pass vacuously and cannot go stale: a target added to the contract
/// tomorrow is asserted here today.
#[test]
fn every_dispatch_target_in_the_contract_resolves_to_a_port() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let registry = all_six_ports(&seen);

    // The claim above is that this test cannot pass vacuously. Nothing enforced
    // it: `unresolved.is_empty()` is trivially true over an empty roster, so
    // emptying `dispatch_targets!` would turn this from a totality proof into a
    // green that examined nothing. A control that cannot see its subject exits 0
    // exactly like one that checked and agreed.
    assert!(
        !DispatchTarget::ALL.is_empty(),
        "the roster is empty, so every assertion below is vacuous"
    );

    let unresolved: Vec<&str> = DispatchTarget::ALL
        .iter()
        .filter(|target| !registry.resolves(**target))
        .map(|target| target.as_str())
        .collect();
    assert!(
        unresolved.is_empty(),
        "the derivation is not total: {unresolved:?} resolve to no port"
    );

    // Six registrations covered every target, whatever the roster's length is.
    // If that length and `ObjectKey::ALL` ever come apart, the loop above is
    // what fails, not this line.
    assert_eq!(ObjectKey::ALL.len(), 6);

    // And this is the ADR-0030 §7 row-4 property itself, rather than a proxy for
    // it: strictly more targets than ports means the mapping is many-to-one, so
    // registrations CANNOT be per-action. Thirteen against six today. Wiring one
    // closure per action would satisfy "every target resolves" and fail here,
    // which is the whole distinction the row is drawing.
    assert!(
        DispatchTarget::ALL.len() > ObjectKey::ALL.len(),
        "{} targets against {} ports — at parity the registrations may be per-action, \
         which is what §7 row 4 forbids",
        DispatchTarget::ALL.len(),
        ObjectKey::ALL.len()
    );
}

/// One target, routed end to end, decoded from a payload that carries no target
/// of its own. RED before the derivation existed: `register_port` did not exist
/// and `hr.transfer` reached no handler.
#[tokio::test]
async fn a_canonical_target_routes_through_the_derivation() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let registry = all_six_ports(&seen);

    let employment_id = Uuid::from_u128(7);
    let mut dispatch_input = input(
        "hr.transfer",
        json!({ "employment_id": employment_id }),
        Some(Uuid::from_u128(9)),
    );
    dispatch_input.target_id = Some(employment_id);

    let outcome = registry
        .dispatch(dispatch_input)
        .await
        .expect("hr.transfer resolves through the Employment port");

    assert_eq!(outcome["target"], json!("hr.transfer"));
    assert_eq!(outcome["owner"], json!("employment"));
    assert_eq!(*seen.lock().expect("stub mutex"), vec!["hr.transfer"]);
}

/// FAIL-CLOSED, part 1: a string that is not a roster member reaches no port
/// and no handler.
#[tokio::test]
async fn an_unknown_target_still_fails_closed() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let registry = all_six_ports(&seen);

    let error = registry
        .dispatch(input(
            "hr.definitely_not_a_target",
            json!({}),
            Some(Uuid::nil()),
        ))
        .await
        .expect_err("an unknown target must not resolve");

    match error {
        ActionError::NotWiredYet { target } => {
            assert_eq!(target.as_deref(), Some("hr.definitely_not_a_target"));
        }
        other => panic!("expected NotWiredYet, got {other:?}"),
    }
    assert!(seen.lock().expect("stub mutex").is_empty());
}

/// FAIL-CLOSED, part 2: a genuine roster member whose object has no port is
/// refused with the same typed error. Deriving the lookup did not turn an
/// unwired object into a silent accept.
#[tokio::test]
async fn a_roster_target_with_no_port_fails_closed() {
    let registry = ProjectedDispatchRegistry::new();

    let error = registry
        .dispatch(input(
            "company.revise",
            json!({ "attributes": {} }),
            Some(Uuid::nil()),
        ))
        .await
        .expect_err("a roster member with no port must not resolve");

    assert!(
        matches!(error, ActionError::NotWiredYet { .. }),
        "{error:?}"
    );
}

/// The seam. The dispatcher injects the contract's target and then asks the
/// DECODED query what it is; a port that answers something else is refused
/// before a transaction opens.
#[tokio::test]
async fn a_payload_that_decodes_as_a_different_target_is_refused() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let registry =
        ProjectedDispatchRegistry::new().register_port(StubPort::<OrgUnit, LyingQuery>::new(&seen));

    let error = registry
        .dispatch(input(
            "organization.create_org_unit",
            json!({ "attributes": {} }),
            Some(Uuid::nil()),
        ))
        .await
        .expect_err("a query that names another target must be refused");

    match error {
        ActionError::Validation(message) => {
            assert!(message.contains("company.revise"), "{message}");
            assert!(
                message.contains("organization.create_org_unit"),
                "{message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
    assert!(seen.lock().expect("stub mutex").is_empty());
}

/// Four-eyes / gate binding seam: approval and instance scope travel as
/// `target_id`, but the canonical port writes the subject named in params.
/// When both are present and disagree, refuse before execute — otherwise an
/// approval bound to I is spent while the port revises J.
#[tokio::test]
async fn a_payload_whose_subject_differs_from_target_id_is_refused() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let registry = ProjectedDispatchRegistry::new()
        .register_port(StubPort::<JobPosition, EchoQuery>::new(&seen));

    let bound = Uuid::from_u128(0x0B01);
    let other = Uuid::from_u128(0x0B02);
    let mut dispatch_input = input(
        "organization.revise_job_position",
        json!({ "job_position_id": other, "attributes": {} }),
        Some(Uuid::nil()),
    );
    dispatch_input.target_id = Some(bound);

    let error = registry
        .dispatch(dispatch_input)
        .await
        .expect_err("mismatched payload subject must be refused");

    match error {
        ActionError::Validation(message) => {
            assert!(
                message.contains(&bound.to_string()),
                "refusal must name the bound target_id: {message}"
            );
            assert!(
                message.contains(&other.to_string()),
                "refusal must name the payload subject: {message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
    assert!(seen.lock().expect("stub mutex").is_empty());
}

/// Employment promote/transfer (and any subject-bearing query) must not skip the
/// bind by omitting `target_id` — that was the live fail-open for EmploymentQuery
/// while `subject_id()` was omitted / when the guard only matched Some/Some.
#[tokio::test]
async fn a_subject_bearing_payload_without_target_id_is_refused() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let registry = ProjectedDispatchRegistry::new()
        .register_port(StubPort::<Employment, EchoQuery>::new(&seen));

    let subject = Uuid::from_u128(0x0E01);
    let error = registry
        .dispatch(input(
            "hr.promote",
            json!({ "employment_id": subject }),
            Some(Uuid::from_u128(0x0E99)),
        ))
        .await
        .expect_err("subject-bearing promote without target_id must be refused");

    match error {
        ActionError::Validation(message) => {
            assert!(
                message.contains(&subject.to_string()),
                "refusal must name the payload subject: {message}"
            );
            assert!(
                message.contains("target_id"),
                "refusal must demand target_id: {message}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
    assert!(seen.lock().expect("stub mutex").is_empty());
}

/// A canonical port REPLAYS a repeat of the same command id. Minting one on the
/// caller's behalf would turn a retried network call into a second write, so a
/// command without one is refused rather than guessed.
#[tokio::test]
async fn a_canonical_target_requires_the_idempotency_key() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let registry = all_six_ports(&seen);

    let error = registry
        .dispatch(input("payroll.submit_run", json!({}), None))
        .await
        .expect_err("no command_id must be refused");

    match error {
        ActionError::Validation(message) => assert!(message.contains("command_id"), "{message}"),
        other => panic!("expected Validation, got {other:?}"),
    }
    assert!(seen.lock().expect("stub mutex").is_empty());
}

/// CALL-SITE pin: DigestConflict through `canonical_port_handler`'s map_err must
/// remain Conflict. Adapter `into_kernel_error` unit tests alone do not cover
/// this — reverting the dispatcher to `KernelError::internal(error.to_string())`
/// leaves those green and fails here.
#[tokio::test]
async fn digest_conflict_from_port_surfaces_as_conflict_not_internal() {
    let registry = ProjectedDispatchRegistry::new().register_port(DigestConflictPort);

    let error = registry
        .dispatch(input(
            "company.revise",
            json!({ "attributes": {} }),
            Some(Uuid::nil()),
        ))
        .await
        .expect_err("digest conflict must surface from the port");

    match error {
        ActionError::Store(PgOntologyError::Domain(kernel)) => {
            assert_eq!(
                kernel.kind,
                ErrorKind::Conflict,
                "dispatcher must preserve Conflict via into_kernel_error; \
                 flattening with KernelError::internal(Display) yields Internal"
            );
            assert_ne!(kernel.kind, ErrorKind::Internal);
        }
        other => panic!("expected Store(Domain(Conflict)), got {other:?}"),
    }
}
