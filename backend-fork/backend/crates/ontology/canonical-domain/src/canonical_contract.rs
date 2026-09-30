//! Locked identity for the six ports, the six projected keys, and the thirteen
//! dispatch targets. Test-only.
//!
//! Every roster here is derived from a single `ALL` constant and sized against
//! `ALL.len()`, so a seventh object key or a fourteenth dispatch target cannot
//! make these loops pass vacuously — the shape of
//! `cedar_pbac/mode_contract.rs:36-46`.

#![allow(clippy::panic)]

use std::collections::BTreeSet;

use super::{
    CanonicalObject, CanonicalPort, CanonicalPortError, CanonicalQuery, CommandId, CommandReceipt,
    Company, CompanyPort, DispatchTarget, Employment, EmploymentPort, JobPosition, JobPositionPort,
    ObjectKey, OrgUnit, OrgUnitPort, PayRun, PayRunPort, Person, PersonPort, Preflight,
    ReceiptOwner,
};
use console_kernel_core::{ErrorKind, KernelError, OrgId, UserId};

/// A port that exists only to witness that the six named traits are reachable
/// and that each pins its object. Never constructed.
pub(crate) struct NoPort<O>(std::marker::PhantomData<O>);

/// The witness port's query. `()` would do for everything except
/// [`CanonicalQuery`], which every port's query must satisfy — that bound is
/// the point, so the witness carries it too.
pub(crate) struct NoQuery;

impl CanonicalQuery for NoQuery {
    fn dispatch_target(&self) -> DispatchTarget {
        DispatchTarget::CompanyRevise
    }

    fn subject_id(&self) -> Option<uuid::Uuid> {
        None
    }
}

impl<O: CanonicalObject> CanonicalPort for NoPort<O> {
    type Object = O;
    type Query = NoQuery;
    type Command = ();
    type Error = std::convert::Infallible;

    fn preflight(_query: &Self::Query) -> Preflight {
        Preflight::ok()
    }

    fn command(
        _org_id: OrgId,
        _command_id: CommandId,
        _actor_id: UserId,
        _query: Self::Query,
        _action_key: &str,
        _object_type_id: uuid::Uuid,
    ) -> Self::Command {
    }

    fn execute(&self, _command: &Self::Command) -> Result<CommandReceipt, Self::Error> {
        // Witness is never constructed; Infallible keeps the Error bound honest.
        match Option::<std::convert::Infallible>::None {
            Some(never) => Err(never),
            None => unreachable!("NoPort is never constructed"),
        }
    }
}

/// Fixture that proves the kind mapping the dispatcher must preserve.
#[derive(Debug)]
enum KindMapFixture {
    Blocked(Vec<String>),
    DigestConflict(uuid::Uuid),
    Database(&'static str),
}

impl std::fmt::Display for KindMapFixture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blocked(blockers) => write!(f, "blocked: {blockers:?}"),
            Self::DigestConflict(id) => write!(f, "digest conflict {id}"),
            Self::Database(msg) => write!(f, "database: {msg}"),
        }
    }
}

impl CanonicalPortError for KindMapFixture {
    fn into_kernel_error(self) -> KernelError {
        let message = self.to_string();
        match self {
            Self::Blocked(_) => KernelError::validation(message),
            Self::DigestConflict(_) => KernelError::conflict(message),
            Self::Database(_) => KernelError::internal(message),
        }
    }
}

#[test]
fn digest_conflict_maps_to_conflict_not_internal() {
    let err = KindMapFixture::DigestConflict(uuid::Uuid::nil());
    let kernel = err.into_kernel_error();
    assert_eq!(kernel.kind, ErrorKind::Conflict);
    assert_ne!(kernel.kind, ErrorKind::Internal);
}

#[test]
fn blocked_maps_to_validation_and_database_to_internal() {
    assert_eq!(
        KindMapFixture::Blocked(vec!["x".into()])
            .into_kernel_error()
            .kind,
        ErrorKind::Validation
    );
    assert_eq!(
        KindMapFixture::Database("fk").into_kernel_error().kind,
        ErrorKind::Internal
    );
}

#[test]
fn six_projected_stable_object_keys_verbatim() {
    let keys: Vec<&str> = ObjectKey::ALL.iter().map(|key| key.as_str()).collect();
    assert_eq!(
        keys,
        vec![
            "company",
            "org_unit",
            "job_position",
            "person",
            "employment",
            "pay_run",
        ]
    );
    assert_eq!(keys.len(), ObjectKey::ALL.len());
}

#[test]
fn thirteen_dispatch_targets_verbatim() {
    let targets: Vec<&str> = DispatchTarget::ALL
        .iter()
        .map(|target| target.as_str())
        .collect();
    assert_eq!(
        targets,
        vec![
            "company.revise",
            "organization.create_org_unit",
            "organization.revise_org_unit",
            "organization.create_job_position",
            "organization.revise_job_position",
            "people.create_person",
            "people.revise_person",
            "hr.appoint",
            "hr.promote",
            "hr.transfer",
            "payroll.create_run",
            "payroll.submit_run",
            "payroll.decide_run",
        ]
    );
    assert_eq!(targets.len(), DispatchTarget::ALL.len());
}

/// Every dispatch target belongs to an object, and every object is dispatchable.
/// The roster is sized against `ObjectKey::ALL`, so a seventh key must arrive
/// with at least one target or this fails.
#[test]
fn every_object_key_owns_at_least_one_dispatch_target() {
    let covered: BTreeSet<ObjectKey> = DispatchTarget::ALL
        .iter()
        .map(|target| target.object())
        .collect();
    assert_eq!(
        covered.len(),
        ObjectKey::ALL.len(),
        "uncovered keys: {:?}",
        ObjectKey::ALL
            .iter()
            .filter(|key| !covered.contains(key))
            .collect::<Vec<_>>()
    );
}

/// Naming each of the six traits by name proves it exists; asking for its
/// `Object::KEY` proves the trait cannot be implemented for the wrong object.
#[test]
fn six_named_ports_exist_and_pin_their_object() {
    fn key_of<P: CanonicalPort>() -> ObjectKey {
        <P::Object as CanonicalObject>::KEY
    }
    fn as_company_port<P: CompanyPort>() -> ObjectKey {
        key_of::<P>()
    }
    fn as_org_unit_port<P: OrgUnitPort>() -> ObjectKey {
        key_of::<P>()
    }
    fn as_job_position_port<P: JobPositionPort>() -> ObjectKey {
        key_of::<P>()
    }
    fn as_person_port<P: PersonPort>() -> ObjectKey {
        key_of::<P>()
    }
    fn as_employment_port<P: EmploymentPort>() -> ObjectKey {
        key_of::<P>()
    }
    fn as_pay_run_port<P: PayRunPort>() -> ObjectKey {
        key_of::<P>()
    }

    let pinned = vec![
        as_company_port::<NoPort<Company>>(),
        as_org_unit_port::<NoPort<OrgUnit>>(),
        as_job_position_port::<NoPort<JobPosition>>(),
        as_person_port::<NoPort<Person>>(),
        as_employment_port::<NoPort<Employment>>(),
        as_pay_run_port::<NoPort<PayRun>>(),
    ];
    assert_eq!(pinned, ObjectKey::ALL.to_vec());
}

/// Every object key names an owner crate and at least one owned table, and no
/// table is claimed twice. Sized against `ALL`, so a seventh key cannot slip
/// through without ownership.
#[test]
fn writer_ownership_registry_is_total_over_object_keys() {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut table_count = 0usize;
    for key in ObjectKey::ALL {
        assert!(
            key.owner_crate().starts_with("console-"),
            "{key:?} owner must be a workspace crate, got {}",
            key.owner_crate()
        );
        assert!(
            !key.owned_tables().is_empty(),
            "{key:?} owns no table, so no writer rule would ever apply to it"
        );
        for table in key.owned_tables() {
            table_count += 1;
            assert!(seen.insert(table), "{table} is claimed by two object keys");
        }
    }
    assert_eq!(seen.len(), table_count);
    assert_eq!(ObjectKey::ALL.len(), 6, "the six keys of handoff line 96");
}

/// The receipt-store owner roster is sized against `ObjectKey::ALL`, so a
/// seventh key cannot reach the store without the CHECK roster growing too.
#[test]
fn receipt_owner_roster_is_sized_against_object_key_all() {
    assert_eq!(ReceiptOwner::ALL.len(), ObjectKey::ALL.len() + 1);
    let values: Vec<&str> = ReceiptOwner::ALL
        .iter()
        .map(|owner| owner.as_str())
        .collect();
    assert_eq!(values[0], "ontology.action");
    let sql = ReceiptOwner::owner_check_constraint_sql();
    for key in ObjectKey::ALL {
        assert!(
            sql.contains(&format!("'{}'", key.as_str())),
            "{key:?} missing from the widening CHECK: {sql}"
        );
    }
    assert!(sql.contains("'ontology.action'"), "{sql}");

    // `CommandReceipt::target` is not optional, so the widening needs a `target`
    // column or a receipt cannot survive a store round-trip. It is NULL exactly
    // for the pre-existing `ontology.action` rows, which have no dispatch
    // target and must not be given a fabricated one.
    let target_sql = ReceiptOwner::target_check_constraint_sql();
    for target in DispatchTarget::ALL {
        assert!(
            target_sql.contains(&format!("'{}'", target.as_str())),
            "{target:?} missing from the widening target CHECK: {target_sql}"
        );
    }
    assert!(
        target_sql.contains("(owner = 'ontology.action') = (target IS NULL)"),
        "the target column must be present exactly when the owner is canonical: {target_sql}"
    );
}

/// Every roster is bidirectional over ALL. Without the inverse, dispatch and
/// receipt read-back must re-spell all thirteen literals by hand — the exact
/// drift this crate exists to prevent — and a typo there is a runtime 404
/// rather than a compile error.
#[test]
fn every_roster_round_trips_over_all() {
    for key in ObjectKey::ALL {
        assert_eq!(key.as_str().parse::<ObjectKey>(), Ok(*key));
    }
    for target in DispatchTarget::ALL {
        assert_eq!(target.as_str().parse::<DispatchTarget>(), Ok(*target));
    }
    for owner in ReceiptOwner::ALL {
        assert_eq!(owner.as_str().parse::<ReceiptOwner>(), Ok(*owner));
    }

    // And nothing outside the roster parses: a stored string that names no
    // member is an error, never a silent default.
    assert!("company".parse::<DispatchTarget>().is_err());
    assert!("hr.appoint".parse::<ObjectKey>().is_err());
    assert!("hr.resign".parse::<DispatchTarget>().is_err());
    assert!("ontology.actions".parse::<ReceiptOwner>().is_err());
}

/// The receipt carries all six properties of migration 0177 in its type.
#[test]
fn receipt_preserves_the_six_migration_0177_properties() {
    use console_kernel_core::{OrgId, UserId};

    let org = OrgId::knl();
    let actor = UserId::new();
    let receipt = CommandReceipt::new(
        org,
        CommandId::from_uuid(uuid::Uuid::from_u128(7)),
        ReceiptOwner::Canonical(ObjectKey::Company),
        DispatchTarget::CompanyRevise,
        actor,
        [9u8; 32],
        serde_json::json!({"revision": 1}),
        time::OffsetDateTime::UNIX_EPOCH,
    );

    // 1 + 6: tenant-global key, and the RLS key is mandatory.
    assert_eq!(receipt.org_id(), org);
    assert_eq!(*receipt.command_id().as_uuid(), uuid::Uuid::from_u128(7));
    // 2: actor bound to the same tenant.
    assert_eq!(receipt.actor_id(), actor);
    // 3: digest is exactly 32 bytes, by type.
    assert_eq!(receipt.payload_digest().len(), 32);
    // 5: stored result replays verbatim.
    assert_eq!(receipt.result(), &serde_json::json!({"revision": 1}));
    assert_eq!(receipt.owner(), ReceiptOwner::Canonical(ObjectKey::Company));
    assert_eq!(receipt.target(), DispatchTarget::CompanyRevise);
    assert_eq!(receipt.created_at(), time::OffsetDateTime::UNIX_EPOCH);

    // 4: immutability. NOT PROVEN HERE, and the earlier clone/eq assertion that
    // claimed to prove it did not: it stays green if someone adds a `&mut`
    // setter tomorrow. Immutability is enforced by 0177's
    // `BEFORE UPDATE OR DELETE` trigger, in the database; the Rust type merely
    // does not currently expose a mutator. Proving THAT would need a
    // compile-fail harness (`trybuild`), which is a new workspace dependency
    // this lane cannot add. The clone below pins only `PartialEq`, which is what
    // the assertions above rely on.
    let same = receipt.clone();
    assert_eq!(same, receipt);
}

/// Preflight is pure: it is an associated function with no `&self`, so it cannot
/// hold a connection, and the domain layer forbids sqlx/axum/tokio.
#[test]
fn preflight_is_pure_and_reports_blockers() {
    assert!(<NoPort<Company> as CanonicalPort>::preflight(&NoQuery).is_ok());
    let blocked = Preflight::blocked(vec!["ambiguous org text".to_owned()]);
    assert!(!blocked.is_ok());
    assert_eq!(blocked.blockers(), ["ambiguous org text".to_owned()]);
}
