#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `company-conformance` — an isolated generic instance-engine regression.
//!
//! **Current product boundary (2026-08-03):** this suite's five fixtures are
//! instance-backed and omit Person. It is not the Company/HR projection target,
//! does not prove domain owning-port or single-writer boundaries, and dispatches
//! no fan-out work. Historical lane language below explains how the regression
//! was built; it is test archaeology, not current program authority.
//!
//! ONE scenario, TWO drivers. Both bind only to surfaces that exist TODAY, so the
//! target is never rewritten as lanes land:
//!   * [`rest::RestDriver`]  — the generic ontology REST surface over the real
//!     axum router, signed ES256 bearer, `console_rt` pool. This is where RLS
//!     arming, the org-wide authority gate and request-context live.
//!   * [`store::StoreDriver`] — the same `OntologyRestState` action methods called
//!     in-process under an explicit `scope_org`. `PgOntologyStore` has NO action
//!     dispatch; `preflight_action`/`execute_action` are on `OntologyRestState`
//!     (`rest/src/lib.rs:747`, `:774`, documented HTTP-independent at `:592-596`).
//!
//! OWNERSHIP. This file, `company_conformance/{harness,rest,store,fixtures}.rs`
//! are owned outside the lanes and a lane MAY NOT edit them. A lane owns exactly
//! one file under `company_conformance/fixtures/`: that type's `declare`, and the
//! param bags for it. Every one of the five slots already exists, so landing a
//! lane is an edit to that lane's file and no other.
//!
//! `declare` is the seam this suite shipped without. It resolves the five types
//! and classifies the result, but on the first version NOTHING between
//! `Harness::bootstrap` and the first `resolve_type` could create one — bootstrap's
//! sole ontology call is `seed_governed_config_object_types`, a closed
//! `install_builtin_catalog` over a digest-allowlisted manifest, and every
//! `fixtures::*` entry point is reached only from inside `if ids.contains_key(..)`,
//! after the type has already resolved. The suite was therefore unsatisfiable by
//! the lanes it was written for, and a lane would have discovered that only after
//! being told its own file was sufficient.
//!
//! RED FOR THE RIGHT REASON. `scenario` resolves all five lane types first and
//! CLASSIFIES rather than panicking: an absent type must fail with the pinned
//! [`UNKNOWN_TYPE`] signature (`adapter-postgres/src/lib.rs:634` is the sole
//! producer of that message on this path) or the assertion fires in-loop — a
//! different shape is a typo / unseeded tenant / broken harness, never "not built
//! yet". Steps whose inputs exist run and panic IN PLACE on failure, so a
//! regression is never absorbed as a gap. The tail assert names what is missing.
//! The "ledger" is the catalog itself — nothing is checked in, nothing is edited
//! when a lane lands.
//!
//! A SCHEMA-ONLY STUB MUST NOT TURN THIS SUITE GREEN. The first version of this
//! file was rejected as VACUOUS: a 170-line declarative stub (`prop`/`draft`/
//! `publish` for the five types, zero backend code) made all 12 ids pass, because
//! every assertion was something the ENGINE already supplies for any published
//! type — create, attribute round-trip, history, CAS, receipt replay, as-of. The
//! suite asserted the types were DECLARED; it must assert a company can be
//! OPERATED. Four assertion classes carry that weight, each chosen because the
//! declarative substrate provably cannot fake it:
//!
//!   1. RELATIONAL INTEGRITY THROUGH THE ENGINE (CC-02/03/04). Not "the instance
//!      has an `org_unit_id` attribute" — `traverse` must return the edge.
//!      WHEN THIS WAS WRITTEN that could not happen: the auto `create` action
//!      only runs `apply_edits`, which writes ATTRIBUTES
//!      (`application/src/lib.rs:296`), and `PgInstanceStore::create_link` had
//!      zero non-test callers, so a declared reference property yielded an empty
//!      graph. It holds NOW because this assertion forced the mechanism into
//!      existence: `sync_property_links_tx` (`adapter-postgres/src/instances.rs`)
//!      projects a property's declared `config.link` into a real `ont_links`
//!      edge on every revision write. The assertion stays as the regression that
//!      guards it.
//!   2. REFERENTIAL VALIDATION (CC-05). A dangling or wrong-type reference must
//!      be REFUSED. Likewise historical: the engine's only check on a
//!      `reference` property WAS `value.is_string()` (`instances.rs`
//!      `check_field_shape`), so a stub accepted a random UUID or a pointer at
//!      the wrong type. `sync_property_links_tx` now pre-checks both in Rust —
//!      `not_found` for a missing referent, `validation` for a wrong-typed one,
//!      compared on `stable_key` rather than the per-version `object_type_id`.
//!
//!      Both paragraphs above described the engine as it was on 2026-07-28 and
//!      were left describing it that way for one day after the code changed
//!      underneath them. They are corrected here rather than deleted because the
//!      pre-state is why these assertion classes were chosen — but a reader who
//!      trusted the stale text would have concluded the engine performs no
//!      referential validation, which is now the opposite of true. This is the
//!      same failure this program has recorded twice before: a comment that
//!      describes the problem it has already fixed.
//!   3. DERIVATION (CC-10). The pay run's `gross_total` is COMPUTED from the
//!      referenced employments' `base_salary`; the suite never sends it and
//!      asserts the fixture did not either. `apply_edits` resolves an edit to a
//!      constant or a named param and nothing else — there is no arithmetic in
//!      the declarative substrate. The cycle runs TWICE over different
//!      populations, so a hard-coded constant `value` edit cannot satisfy both.
//!   4. EFFECTIVE-DATED BEHAVIOUR THAT CHANGES AN ANSWER (CC-07/11). The transfer
//!      must change what the SAME query returns either side of T1 — in the
//!      attribute bag AND in the live graph, whose superseded edge must close
//!      (`traverse` walks `valid_to IS NULL` only).
//!
//! If a future edit makes all 12 satisfiable by declaration alone, the target has
//! regressed to the rejected version. The stub test in `docs`-free form: publish
//! the five types with the right properties and nothing else; this suite must
//! still be RED on CC-02.
//!
//! WHAT A LANE MUST SHIP for its step to go green (all verified against the
//! substrate, each would otherwise produce a misleading red):
//!   1. Build the type with `create_object_type` + the 4-step publish. Do NOT
//!      touch the built-in catalog: `install_builtin_catalog` sha256s the
//!      canonical manifest against a migration-owned allowlist, so a 28th type
//!      needs a new `BUILTIN_CATALOG_VERSION` plus a digest migration.
//!      Working publish sequence (`rest/tests/publish_auto_create_action_as_runtime_role.rs:105-164`):
//!      `create_object_type` -> `transition_lifecycle(ReviewPending)` ->
//!      `create_approval`/`decide_approval` of kind `ontology.schema.publish`
//!      carrying `payload_summary.key_revision == reviewed.key_write_revision`
//!      and decided by a DIFFERENT user -> `transition_lifecycle(Published)`.
//!      draft -> published direct raises 23514 `ontology_write.review_required`.
//!   2. Author NO action. Publish auto-attaches the generic `create`
//!      (`instance_revision`, authority-gate-only, one param per property,
//!      `required` mirrored from the property). A hand-authored differently-keyed
//!      action yields 404 `action type was not found for that object type`, which
//!      reads almost exactly like CC-01's red.
//!   3. `parent_org_unit_id` on `org_unit` MUST be `required: false` — the root
//!      unit has no parent, and `validate_params` rejects a missing required param.
//!   4. Every param bag is the FULL required set on EVERY revision. `apply_edits`
//!      resolves an absent param to `Value::Null` and inserts it over the base
//!      (`ontology/application/src/lib.rs:288,296`), so a partial bag blanks the
//!      rest and validation rejects. It is also what makes CC-09 work: with a full
//!      bag `new_attrs` is base-independent, so a replayed `payload_digest` matches.
//!   5. `validate_params` rejects any param NOT in the schema, so a create's stored
//!      attributes equal its params exactly — CC-01/03/10 assert that round-trip.
//!
//! FIXITY — WHY THIS SUITE DOES NOT CALL `verify_chain`. The brief specified
//! `verify_chain(&history) == None` for CC-05/CC-07. That assertion CANNOT hold
//! today, for any object whose attribute bag has two or more keys, and the reason
//! is an engine defect rather than anything a lane controls:
//!   * `canonical_revision` hashes the attribute bag by serializing it
//!     (`instances.rs:809-826`). Its header comment claims determinism because
//!     "no `preserve_order` feature in the workspace → BTreeMap"
//!     (`instances.rs:800-802`) — that comment is FALSE.
//!     `cargo tree -p console-ontology-rest -e features` lists
//!     `serde_json feature "preserve_order"`, so `serde_json::Map` is an
//!     insertion-ordered IndexMap.
//!   * The write hashes the bag in PARAM order; the read gets it back in jsonb's
//!     own order. Executed on the built-in `position` type:
//!     `PROBE sent   = {"worksite":…,"job_function":…,"job_title":…,"headcount":2}`
//!     `PROBE stored = {"worksite":…,"headcount":2,"job_title":…,"job_function":…}`
//!     so the recompute diverges and `verify_chain` reports a break on a
//!     perfectly good v1.
//!   * The one existing green caller
//!     (`adapter-postgres/tests/instances_rls_surfaces_as_runtime_role.rs:2103`)
//!     uses a SINGLE-attribute type, where no reordering is possible. That is why
//!     this has never fired.
//!
//! Asserting it anyway would make CTL-2 red today and hand every lane a target it
//! cannot reach. So CC-05/CC-07/CTL-2 assert the part that IS deterministic — the
//! chain LINKAGE (genesis sentinel, then each revision onto its predecessor's
//! `row_hash`) — and the full recompute is left to the fix. TIGHTEN THIS BACK TO
//! `verify_chain` the moment canonicalization is made order-independent.
//!
//! DELIBERATE DEVIATION FROM THE BRIEF, CC-12. The brief said "CC-06's action as
//! Unprivileged". That cannot prove denial: inside `instance_revision_writeback`
//! the receipt lookup and the revision CAS both run BEFORE the gate chain
//! (`rest/src/lib.rs:1124-1156`), so replaying CC-06's `command_id` denies with
//! `forbidden`/"command_id belongs to another principal" and a fresh id with the
//! stale `expected_revision` denies with `ontology_action_revision_precondition_failed`.
//! Either would be a green-looking assertion for the wrong reason. CC-12 therefore
//! uses a FRESH `command_id` and the CURRENT revision, so the ONLY thing that can
//! deny it is the authority gate.

// A test binary's crate root sits in `tests/`, so its submodules resolve there
// too. `#[path]` keeps the owned/lane-editable split on disk (and the CODEOWNERS
// rules readable) without leaking five stray files into `tests/`.
#[path = "company_conformance/fixtures.rs"]
mod fixtures;
#[path = "company_conformance/harness.rs"]
mod harness;
#[path = "company_conformance/rest.rs"]
mod rest;
#[path = "company_conformance/store.rs"]
mod store;

use std::collections::BTreeMap;

use console_ontology_adapter_postgres::instances::{
    InstanceState, RevisionSummary, TraversalGraph,
};
use console_ontology_domain::{InstanceId, ObjectTypeId};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use harness::{Harness, T0, T1};

/// The five historical fixture types exercised by this generic engine regression.
/// They deliberately do not define the current projected product model.
const LANE_TYPES: [&str; 5] = [
    "company",
    "org_unit",
    "job_position",
    "employment",
    "pay_run",
];

/// The auto-attached generic action every published instance-backed type gets.
const ACTION_KEY: &str = "create";

/// The ONE failure signature that means "this type is not built yet".
const UNKNOWN_TYPE: (&str, &str) = ("not_found", "object type was not found");

/// A stable key that can never exist, for the cross-driver signature pin (CTL-3).
const ABSENT_KEY: &str = "__conformance_absent__";

/// A built-in, instance-backed, `create`-carrying type. The action-path control.
const BUILTIN_ACTION_TYPE: &str = "position";

/// A built-in projected type. Resolve-only control (projected types carry zero
/// actions — `seed.rs:191`), which is why it cannot anchor CTL-2.
const BUILTIN_RESOLVE_TYPE: &str = "customer";

// ===========================================================================
// Driver API — implemented twice, in `rest.rs` and `store.rs`.
// ===========================================================================

#[derive(Debug, Clone)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

impl Failure {
    fn shape(&self) -> (&str, &str) {
        (self.code.as_str(), self.message.as_str())
    }
}

/// One business command. `params` is ALWAYS the full required set (see the module
/// doc, point 4).
#[derive(Debug, Clone)]
pub struct Command {
    pub type_key: &'static str,
    /// `(target, expected_revision)` for an edit; absent for a create.
    pub instance: Option<(InstanceId, i64)>,
    pub title: Option<String>,
    pub params: Value,
    pub command_id: Uuid,
    pub valid_from: Option<OffsetDateTime>,
}

impl Command {
    fn create(
        type_key: &'static str,
        title: &str,
        params: Value,
        valid_from: OffsetDateTime,
    ) -> Self {
        Self {
            type_key,
            instance: None,
            title: Some(title.to_owned()),
            params,
            command_id: Uuid::new_v4(),
            valid_from: Some(valid_from),
        }
    }

    fn edit(
        type_key: &'static str,
        target: InstanceId,
        expected_revision: i64,
        params: Value,
        valid_from: OffsetDateTime,
    ) -> Self {
        Self {
            type_key,
            instance: Some((target, expected_revision)),
            title: None,
            params,
            command_id: Uuid::new_v4(),
            valid_from: Some(valid_from),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Actor {
    /// SUPER_ADMIN — the only role `Feature::RoleManage` admits
    /// (`authz/src/lib.rs:605`, row `[D,D,D,D,D,A]`).
    Privileged,
    /// EXECUTIVE — `BranchScope::All` WITHOUT a DB read
    /// (`authz/src/lib.rs:1478-1483`), which isolates the feature check from the
    /// scope check. No `users` row is needed: roles come from the verified token
    /// claims, not the DB.
    Unprivileged,
}

#[allow(async_fn_in_trait)]
pub trait Driver {
    const NAME: &'static str;
    /// REST denies org-wide pre-handler (403 `forbidden`, zero DB contact); the
    /// store has no routing layer, so the same principal reaches the gate chain
    /// and is denied there (`gate_denied`). The asymmetry is pinned, not papered
    /// over.
    const DENIAL_CODE: &'static str;

    async fn resolve_type(&self, key: &str) -> Result<ObjectTypeId, Failure>;
    async fn execute(&self, cmd: &Command, actor: Actor) -> Result<InstanceState, Failure>;
    async fn read(
        &self,
        id: InstanceId,
        as_of: Option<OffsetDateTime>,
    ) -> Result<InstanceState, Failure>;
    async fn history(&self, id: InstanceId) -> Result<Vec<RevisionSummary>, Failure>;
    /// The engine's own §2 search-around over LIVE (`valid_to IS NULL`) edges.
    /// This is the surface that separates a declared reference property from a
    /// real relationship: nothing on the action path writes `ont_links` today
    /// (`create_link` has zero non-test callers), so a schema-only type returns
    /// an empty edge set here no matter how many `*_id` properties it declares.
    async fn traverse(&self, root: InstanceId, depth: u32) -> Result<TraversalGraph, Failure>;
}

// ===========================================================================
// Small assertion helpers (shared by both drivers).
// ===========================================================================

fn ok<T>(name: &str, id: &str, result: Result<T, Failure>) -> T {
    match result {
        Ok(value) => value,
        Err(failure) => panic!(
            "[{name}] {id}: expected success, got code={} message={}",
            failure.code, failure.message
        ),
    }
}

fn err<T: std::fmt::Debug>(name: &str, id: &str, result: Result<T, Failure>) -> Failure {
    match result {
        Err(failure) => failure,
        Ok(value) => panic!("[{name}] {id}: expected a failure, got Ok({value:?})"),
    }
}

/// Every param a create sent must come back verbatim in the stored attributes.
fn assert_round_trip(name: &str, id: &str, params: &Value, state: &InstanceState) {
    let sent = params.as_object().expect("params must be an object");
    for (key, value) in sent {
        assert_eq!(
            state.revision.attributes.get(key),
            Some(value),
            "[{name}] {id}: attribute '{key}' did not round-trip; stored={:?}",
            state.revision.attributes
        );
    }
}

fn attr<'a>(state: &'a InstanceState, key: &str) -> &'a Value {
    state.revision.attributes.get(key).unwrap_or_else(|| {
        panic!(
            "attribute '{key}' is absent from {:?}",
            state.revision.attributes
        )
    })
}

/// `instances.rs:37` — the per-instance chain's genesis sentinel.
const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// The order-independent half of the fixity proof (see the module doc's FIXITY
/// note): v1 chains onto the genesis sentinel and every later revision chains
/// onto its predecessor's `row_hash`. An out-of-band INSERT, a replaced head, or
/// a dropped revision all break this.
fn assert_chain_linkage(name: &str, id: &str, history: &[RevisionSummary]) {
    assert!(
        !history.is_empty(),
        "[{name}] {id}: an instance must have a history"
    );
    assert_eq!(
        history[0].prev_hash, GENESIS_HASH,
        "[{name}] {id}: v1 must chain onto the genesis sentinel"
    );
    for pair in history.windows(2) {
        assert_eq!(
            pair[1].prev_hash, pair[0].row_hash,
            "[{name}] {id}: v{} must chain onto v{}'s row_hash",
            pair[1].version, pair[0].version
        );
    }
    for revision in history {
        assert_eq!(
            revision.row_hash.len(),
            64,
            "[{name}] {id}: every row_hash must be a sha256 hex digest"
        );
        assert_ne!(
            revision.row_hash, revision.prev_hash,
            "[{name}] {id}: a revision must not chain onto itself"
        );
    }
}

fn instance_ref(id: InstanceId) -> Value {
    json!(id.to_string())
}

fn as_instance_id(value: &Value) -> InstanceId {
    InstanceId::from_uuid(
        value
            .as_str()
            .and_then(|s| s.parse::<Uuid>().ok())
            .unwrap_or_else(|| panic!("expected an instance id string, got {value:?}")),
    )
}

/// Everything the engine's traversal surface reaches ONE live hop out of `from`.
/// A schema-only type returns an empty vector here however many `*_id`
/// properties it declares — see the module doc, class 1.
async fn hop<D: Driver>(d: &D, id: &str, from: InstanceId) -> Vec<InstanceId> {
    let graph = ok(D::NAME, id, d.traverse(from, 1).await);
    graph
        .edges
        .iter()
        .filter(|edge| edge.from_instance_id == from)
        .map(|edge| edge.to_instance_id)
        .collect()
}

/// A write the business must refuse. The lane picks the code — `validation` and
/// `not_found` are the two both drivers map identically — but it must be a
/// DELIBERATE refusal, never a crash and never an authorization accident (this
/// actor is privileged, so a denial code here would mean the test is proving the
/// wrong thing).
fn assert_refusal(name: &str, id: &str, what: &str, failure: &Failure) {
    assert!(
        matches!(failure.code.as_str(), "validation" | "not_found"),
        "[{name}] {id}: {what} must be refused as invalid data — expected code `validation` or \
         `not_found`, got code={} message={}. `internal` is a crash, and a denial code means the \
         gate refused it rather than the model. NOTE a schema-only type ACCEPTS this write: the \
         engine only checks that a reference is a string.",
        failure.code,
        failure.message
    );
}

/// The relationship must be REACHABLE, not merely named. `what` describes the
/// edge in the business's own words so a red reads as a missing relationship
/// rather than a missing property.
fn assert_reaches(name: &str, id: &str, what: &str, hops: &[InstanceId], target: InstanceId) {
    assert!(
        hops.contains(&target),
        "[{name}] {id}: {what} must be reachable through the engine's traversal surface \
         (a live `ont_links` edge), not merely named in an attribute. Declaring the reference \
         property satisfies the attribute round-trip and FAILS this — that is the point. \
         one hop reached {hops:?}, expected to include {target}"
    );
}

// ===========================================================================
// THE SCENARIO — written once, driven twice. 12 ids.
// ===========================================================================

#[allow(clippy::too_many_lines)]
async fn scenario<D: Driver>(d: &D) {
    let name = D::NAME;

    // Resolve every lane type first and CLASSIFY. An absent type must carry the
    // pinned unknown-type signature; anything else is a harness bug and is loud.
    let mut ids: BTreeMap<&str, ObjectTypeId> = BTreeMap::new();
    let mut absent: Vec<&str> = Vec::new();
    for key in LANE_TYPES {
        match d.resolve_type(key).await {
            Ok(id) => {
                ids.insert(key, id);
            }
            Err(failure) => {
                assert_eq!(
                    failure.shape(),
                    UNKNOWN_TYPE,
                    "[{name}] {key}: an absent lane type must fail with the pinned unknown-type \
                     error (CTL-3 pins it byte-identically across both drivers). \
                     got code={} message={}. A different shape means a typo, an unseeded tenant, \
                     or a broken harness — NOT an unbuilt type.",
                    failure.code,
                    failure.message
                );
                absent.push(key);
            }
        }
    }

    let mut ran = 0_u32;

    // --- CC-01 found a company ------------------------------------------------
    let company = if ids.contains_key("company") {
        let params = fixtures::company::found("KNL Logistics");
        let legal_name = params
            .get("legal_name")
            .and_then(Value::as_str)
            .expect("the company fixture must carry legal_name")
            .to_owned();
        let cmd = Command::create("company", &legal_name, params.clone(), T0);
        let state = ok(name, "CC-01", d.execute(&cmd, Actor::Privileged).await);
        assert_eq!(
            state.revision.version, 1,
            "[{name}] CC-01: create must be v1"
        );
        assert_eq!(
            state.instance.title, legal_name,
            "[{name}] CC-01: head title must be the legal name"
        );
        assert_round_trip(name, "CC-01", &params, &state);
        ran += 1;
        Some(state)
    } else {
        None
    };

    // --- CC-02 create org units (root / division / team) ----------------------
    // The hierarchy must be WALKABLE, not merely annotated: a declared
    // `parent_org_unit_id` property round-trips for free, so the edge is what
    // separates a real org tree from a schema.
    let units: Vec<InstanceState> = if ids.contains_key("org_unit") {
        let mut built: Vec<InstanceState> = Vec::new();
        for (label, has_parent) in [("Root", false), ("Division", true), ("Team", true)] {
            let parent = if has_parent {
                Some(built.last().expect("a parent unit").instance.id)
            } else {
                None
            };
            let params = fixtures::org_unit::unit(label, parent);
            let cmd = Command::create("org_unit", label, params.clone(), T0);
            let state = ok(name, "CC-02", d.execute(&cmd, Actor::Privileged).await);
            assert_eq!(
                state.revision.version, 1,
                "[{name}] CC-02: {label} must be v1"
            );
            assert_round_trip(name, "CC-02", &params, &state);
            if let Some(parent) = parent {
                assert_eq!(
                    attr(&state, "parent_org_unit_id"),
                    &instance_ref(parent),
                    "[{name}] CC-02: {label} must point at its parent unit"
                );
                let hops = hop(d, "CC-02", state.instance.id).await;
                assert_reaches(
                    name,
                    "CC-02",
                    &format!("{label}'s parent org unit"),
                    &hops,
                    parent,
                );
            }
            built.push(state);
        }
        ran += 1;
        built
    } else {
        Vec::new()
    };

    // --- CC-03 define positions ----------------------------------------------
    let positions: Vec<InstanceState> = if ids.contains_key("job_position") && units.len() == 3 {
        let mut built: Vec<InstanceState> = Vec::new();
        for (label, unit_index, headcount) in [("현장반장", 1_usize, 2_i64), ("정비기사", 2, 5)]
        {
            let unit = units[unit_index].instance.id;
            let params = fixtures::job_position::position(label, unit, headcount);
            let cmd = Command::create("job_position", label, params.clone(), T0);
            let state = ok(name, "CC-03", d.execute(&cmd, Actor::Privileged).await);
            assert_eq!(
                state.revision.version, 1,
                "[{name}] CC-03: {label} must be v1"
            );
            assert_round_trip(name, "CC-03", &params, &state);
            assert_eq!(
                attr(&state, "org_unit_id"),
                &instance_ref(unit),
                "[{name}] CC-03: {label} must sit in a CC-02 org unit"
            );
            assert_eq!(
                attr(&state, "headcount").as_i64(),
                Some(headcount),
                "[{name}] CC-03: headcount must round-trip as an integer, got {:?}",
                attr(&state, "headcount")
            );
            let hops = hop(d, "CC-03", state.instance.id).await;
            assert_reaches(name, "CC-03", &format!("{label}'s org unit"), &hops, unit);
            built.push(state);
        }
        ran += 1;
        built
    } else {
        Vec::new()
    };

    // --- CC-04 hire people ----------------------------------------------------
    let employments: Vec<InstanceState> = if ids.contains_key("employment") && positions.len() == 2
    {
        let mut built: Vec<InstanceState> = Vec::new();
        for (person, index) in [("김정비", 0_usize), ("박현장", 1)] {
            let position = &positions[index];
            let unit = InstanceId::from_uuid(
                attr(position, "org_unit_id")
                    .as_str()
                    .and_then(|s| s.parse::<Uuid>().ok())
                    .expect("a position's org_unit_id must be an instance id"),
            );
            let params = fixtures::employment::hire(person, position.instance.id, unit);
            let cmd = Command::create("employment", person, params.clone(), T0);
            let state = ok(name, "CC-04", d.execute(&cmd, Actor::Privileged).await);
            assert_eq!(
                state.revision.version, 1,
                "[{name}] CC-04: {person} must be v1"
            );
            assert_round_trip(name, "CC-04", &params, &state);
            assert_eq!(
                attr(&state, "job_position_id"),
                &instance_ref(position.instance.id),
                "[{name}] CC-04: {person} must hold a CC-03 position"
            );
            assert_eq!(
                attr(&state, "org_unit_id"),
                &instance_ref(unit),
                "[{name}] CC-04: {person} must sit in that position's org unit"
            );
            assert_eq!(
                state.revision.valid_from, T0,
                "[{name}] CC-04: the hire must be effective-dated at T0"
            );
            let history = ok(name, "CC-04", d.history(state.instance.id).await);
            assert_eq!(
                history.len(),
                1,
                "[{name}] CC-04: a fresh hire has exactly one revision"
            );
            assert_chain_linkage(name, "CC-04", &history);
            let hops = hop(d, "CC-04", state.instance.id).await;
            assert_reaches(
                name,
                "CC-04",
                &format!("{person}'s job position"),
                &hops,
                position.instance.id,
            );
            built.push(state);
        }
        ran += 1;
        built
    } else {
        Vec::new()
    };

    // --- CC-05 referential integrity ------------------------------------------
    // The engine checks a `reference` property with `value.is_string()` and
    // NOTHING else (`instances.rs` `check_field_shape`): it never confirms the
    // referent exists, nor that it is of the right type. A schema-only stub
    // therefore ACCEPTS both of these writes. Refusing them needs real code.
    if ids.contains_key("job_position")
        && !units.is_empty()
        && let Some(company_state) = company.as_ref()
    {
        let dangling = InstanceId::new();
        let cmd = Command::create(
            "job_position",
            "dangling",
            fixtures::job_position::position("dangling", dangling, 1),
            T0,
        );
        let failure = err(name, "CC-05", d.execute(&cmd, Actor::Privileged).await);
        assert_refusal(
            name,
            "CC-05",
            "a job position under a NONEXISTENT org unit",
            &failure,
        );

        // Right shape, wrong type: a real instance id that is not an org unit.
        let company_id = company_state.instance.id;
        let cmd = Command::create(
            "job_position",
            "mistyped",
            fixtures::job_position::position("mistyped", company_id, 1),
            T0,
        );
        let failure = err(name, "CC-05", d.execute(&cmd, Actor::Privileged).await);
        assert_refusal(
            name,
            "CC-05",
            "a job position whose org unit is actually the COMPANY",
            &failure,
        );
        ran += 1;
    }

    // --- CC-06 transfer one ---------------------------------------------------
    // The transfer is an ATTRIBUTE revision, never an edge move: `ont_links` has
    // exactly one INSERT and no UPDATE (`instances.rs:319`, `:554`), so nothing
    // can close a link's `valid_to`.
    let transferred = if employments.len() == 2 && units.len() == 3 {
        let subject = &employments[0];
        let old_unit = attr(subject, "org_unit_id").clone();
        let new_unit = units[2].instance.id;
        assert_ne!(
            old_unit,
            instance_ref(new_unit),
            "[{name}] CC-06: the transfer target must differ from the current unit"
        );
        let params = fixtures::employment::hire("김정비", positions[0].instance.id, new_unit);
        let cmd = Command::edit("employment", subject.instance.id, 1, params, T1);
        let state = ok(name, "CC-06", d.execute(&cmd, Actor::Privileged).await);
        assert_eq!(
            state.revision.version, 2,
            "[{name}] CC-06: the transfer must be v2"
        );
        assert_eq!(
            attr(&state, "org_unit_id"),
            &instance_ref(new_unit),
            "[{name}] CC-06: the transfer must land the new org unit"
        );
        ran += 1;
        Some((subject.instance.id, old_unit, new_unit, cmd, state))
    } else {
        None
    };

    if let Some((subject, ref old_unit, new_unit, ref cmd, ref v2)) = transferred {
        // --- CC-07 revision, not overwrite ------------------------------------
        let history = ok(name, "CC-07", d.history(subject).await);
        assert_eq!(
            history.len(),
            2,
            "[{name}] CC-07: the edit must APPEND, not overwrite"
        );
        assert_eq!(history[0].version, 1);
        assert_eq!(history[1].version, 2);
        assert_eq!(
            history[0].valid_to,
            Some(T1),
            "[{name}] CC-07: v1's interval must close exactly at the transfer instant"
        );
        assert_eq!(
            history[0].attributes.get("org_unit_id"),
            Some(old_unit),
            "[{name}] CC-07: v1 must still carry the OLD org unit"
        );
        assert_chain_linkage(name, "CC-07", &history);
        ran += 1;

        // --- CC-08 stale write refused ----------------------------------------
        let mut stale = cmd.clone();
        stale.command_id = Uuid::new_v4();
        let failure = err(name, "CC-08", d.execute(&stale, Actor::Privileged).await);
        assert_eq!(
            failure.code, "ontology_action_revision_precondition_failed",
            "[{name}] CC-08: a stale expected_revision must be refused by the CAS, got {failure:?}"
        );
        assert_eq!(
            ok(name, "CC-08", d.history(subject).await).len(),
            2,
            "[{name}] CC-08: a refused write must leave zero rows"
        );
        ran += 1;

        // --- CC-09 idempotent command -----------------------------------------
        let replay = ok(name, "CC-09", d.execute(cmd, Actor::Privileged).await);
        assert_eq!(
            replay.revision.id, v2.revision.id,
            "[{name}] CC-09: replaying the same command_id must return the STORED receipt, \
             not a new revision"
        );
        assert_eq!(
            ok(name, "CC-09", d.history(subject).await).len(),
            2,
            "[{name}] CC-09: a replay must not append"
        );
        ran += 1;

        // --- CC-10 run a pay cycle: the total is DERIVED, never sent ----------
        // `apply_edits` resolves an edit to a constant or a named param and does
        // no arithmetic, so a declarative type cannot produce this number. The
        // cycle runs TWICE over different populations: one hard-coded constant
        // cannot be right both times.
        if ids.contains_key("pay_run") {
            let salary = |state: &InstanceState| -> i64 {
                attr(state, "base_salary").as_i64().unwrap_or_else(|| {
                    panic!(
                        "[{name}] CC-10: employment.base_salary must be an integer, got {:?}",
                        attr(state, "base_salary")
                    )
                })
            };

            let everyone: Vec<InstanceId> = employments.iter().map(|e| e.instance.id).collect();
            let total_everyone: i64 = employments.iter().map(salary).sum();
            let just_one = vec![employments[0].instance.id];
            let total_one = salary(&employments[0]);
            assert_ne!(
                total_everyone, total_one,
                "[{name}] CC-10: the two people must carry DIFFERENT non-zero salaries, or one \
                 constant would satisfy both pay runs and prove nothing"
            );

            // Two populations, two different correct answers.
            for (members, expected) in [(everyone, total_everyone), (just_one, total_one)] {
                let params = fixtures::pay_run::cycle(&members, T0, T1);
                assert!(
                    params.get("gross_total").is_none(),
                    "[{name}] CC-10: the pay-run fixture must NOT send `gross_total` — it is the \
                     DERIVED value under test. Sending it turns this back into a round-trip \
                     assertion, which is the vacuity this suite exists to prevent."
                );
                let pay_cmd = Command::create("pay_run", "2026-07", params.clone(), T1);
                let run = ok(name, "CC-10", d.execute(&pay_cmd, Actor::Privileged).await);
                assert_eq!(
                    run.revision.version, 1,
                    "[{name}] CC-10: the pay run must be v1"
                );
                assert_round_trip(name, "CC-10", &params, &run);
                let carried = attr(&run, "employment_ids");
                for member in &members {
                    assert!(
                        carried
                            .as_array()
                            .is_some_and(|ids| ids.contains(&instance_ref(*member))),
                        "[{name}] CC-10: the pay run must carry every employment id, \
                         got {carried:?}"
                    );
                }
                assert_eq!(
                    attr(&run, "gross_total").as_i64(),
                    Some(expected),
                    "[{name}] CC-10: `gross_total` must be COMPUTED from the {} referenced \
                     employments' base_salary (expected {expected}); the suite never sent it. \
                     A null/absent value means the type merely DECLARES the property and nothing \
                     derives it — `apply_edits` does no arithmetic. got {:?}",
                    members.len(),
                    attr(&run, "gross_total")
                );
            }
            ran += 1;
        }

        // --- CC-11 reconstruct as-of ------------------------------------------
        let before = ok(
            name,
            "CC-11",
            d.read(subject, Some(T1 - time::Duration::hours(1))).await,
        );
        assert_eq!(
            before.revision.version, 1,
            "[{name}] CC-11: pre-transfer must read v1"
        );
        assert_eq!(
            attr(&before, "org_unit_id"),
            old_unit,
            "[{name}] CC-11: pre-transfer must read the OLD org unit"
        );
        let after = ok(
            name,
            "CC-11",
            d.read(subject, Some(T1 + time::Duration::hours(1))).await,
        );
        assert_eq!(
            after.revision.version, 2,
            "[{name}] CC-11: post-transfer must read v2"
        );
        assert_eq!(
            attr(&after, "org_unit_id"),
            &instance_ref(new_unit),
            "[{name}] CC-11: post-transfer must read the NEW org unit"
        );
        let current = ok(name, "CC-11", d.read(subject, None).await);
        assert_eq!(
            current.revision.id, after.revision.id,
            "[{name}] CC-11: the current read must equal the post-transfer as-of read"
        );

        // The LIVE GRAPH must agree with the attribute bag. `traverse` walks only
        // `valid_to IS NULL` edges, so a transfer that opens the new edge without
        // closing the old one leaves the org chart showing the person in two
        // units at once — the exact lie an attribute-only model cannot detect.
        let hops = hop(d, "CC-11", subject).await;
        let old_unit_id = as_instance_id(old_unit);
        assert_reaches(name, "CC-11", "the post-transfer org unit", &hops, new_unit);
        assert!(
            !hops.contains(&old_unit_id),
            "[{name}] CC-11: the transfer must CLOSE the superseded edge to {old_unit_id}; \
             traverse walks live links only, so a still-open old edge means the person reads as \
             a member of BOTH units. one hop reached {hops:?}"
        );
        ran += 1;

        // --- CC-12 non-privileged refused -------------------------------------
        // Fresh command_id + the CURRENT revision, so the authority gate is the
        // ONLY thing that can deny (see the module doc's deviation note).
        let mut denied = cmd.clone();
        denied.command_id = Uuid::new_v4();
        denied.instance = Some((subject, 2));
        let failure = err(name, "CC-12", d.execute(&denied, Actor::Unprivileged).await);
        assert_eq!(
            failure.code,
            D::DENIAL_CODE,
            "[{name}] CC-12: a non-privileged principal must be refused with the driver's \
             pinned denial code, got {failure:?}"
        );
        assert_eq!(
            ok(name, "CC-12", d.history(subject).await).len(),
            2,
            "[{name}] CC-12: a refused write must leave zero rows"
        );
        ran += 1;
    }

    assert!(
        absent.is_empty(),
        "[{name}] {ran}/12 scenario ids green; blocked on unbuilt types: {absent:?}"
    );
    // Self-enforcing: with every type present, all 12 ids must actually have RUN.
    // Without this a future guard that quietly skips a step would read as green.
    assert_eq!(
        ran, 12,
        "[{name}] every lane type resolved, so all 12 scenario ids must have run; only {ran} did. \
         A step was skipped by its own input guard — that is a hole in the target, not a pass."
    );
}

// ===========================================================================
// CONTROLS — green today and after fan-out. Never expected-red.
// ===========================================================================

async fn controls<D: Driver>(d: &D) {
    let name = D::NAME;

    // CTL-1 — the 27-type built-in catalog really installed.
    ok(name, "CTL-1", d.resolve_type(BUILTIN_RESOLVE_TYPE).await);

    // CTL-2 — params, the §16 authority gate, CAS, audit and receipt all work
    // TODAY on an instance-backed built-in. `customer` cannot anchor this: every
    // projected type ships zero actions (`seed.rs:191`).
    let params = json!({
        "worksite": "창원 성산 현장",
        "job_function": "정비",
        "job_title": "현장반장",
        "headcount": 2,
    });
    let cmd = Command::create(BUILTIN_ACTION_TYPE, "현장반장", params.clone(), T0);
    let state = ok(name, "CTL-2", d.execute(&cmd, Actor::Privileged).await);
    assert_eq!(
        state.revision.version, 1,
        "[{name}] CTL-2: built-in create must be v1"
    );
    assert_round_trip(name, "CTL-2", &params, &state);
    let history = ok(name, "CTL-2", d.history(state.instance.id).await);
    assert_eq!(history.len(), 1);
    assert_chain_linkage(name, "CTL-2", &history);

    // CTL-3 — the pinned unknown-type signature, byte-identical across BOTH
    // drivers. The whole red proof rests on this one shape.
    let failure = err(name, "CTL-3", d.resolve_type(ABSENT_KEY).await);
    assert_eq!(
        failure.shape(),
        UNKNOWN_TYPE,
        "[{name}] CTL-3: got code={} message={}",
        failure.code,
        failure.message
    );

    // CTL-5 — the same admitted action, refused for a non-privileged principal.
    let mut denied = cmd.clone();
    denied.command_id = Uuid::new_v4();
    let failure = err(name, "CTL-5", d.execute(&denied, Actor::Unprivileged).await);
    assert_eq!(
        failure.code,
        D::DENIAL_CODE,
        "[{name}] CTL-5: got {failure:?}"
    );

    // CTL-6 — every remaining MECHANISM the scenario leans on, executed today on
    // a built-in type: the as-of read, the v+1 revision, the interval close, the
    // stale-revision CAS refusal and the command-receipt replay. Without this the
    // codes CC-08/CC-09 assert would be strings nobody has ever seen come out of
    // these drivers, and a lane's red could just as easily be a broken driver.
    let target = state.instance.id;
    let at_v1 = ok(name, "CTL-6", d.read(target, Some(T0)).await);
    assert_eq!(
        at_v1.revision.version, 1,
        "[{name}] CTL-6: an as-of read at T0 is v1"
    );
    assert_eq!(
        ok(name, "CTL-6", d.read(target, None).await).revision.id,
        at_v1.revision.id,
        "[{name}] CTL-6: the current read must equal the only revision"
    );

    let mut edited = params.clone();
    edited["headcount"] = json!(7);
    let edit = Command::edit(BUILTIN_ACTION_TYPE, target, 1, edited, T1);
    let v2 = ok(name, "CTL-6", d.execute(&edit, Actor::Privileged).await);
    assert_eq!(
        v2.revision.version, 2,
        "[{name}] CTL-6: the edit must append v2"
    );

    let history = ok(name, "CTL-6", d.history(target).await);
    assert_eq!(
        history.len(),
        2,
        "[{name}] CTL-6: the edit must APPEND, not overwrite"
    );
    assert_eq!(
        history[0].valid_to,
        Some(T1),
        "[{name}] CTL-6: v1's interval must close at the edit instant"
    );
    assert_chain_linkage(name, "CTL-6", &history);
    assert_eq!(
        ok(
            name,
            "CTL-6",
            d.read(target, Some(T1 - time::Duration::hours(1))).await
        )
        .revision
        .id,
        history[0].id,
        "[{name}] CTL-6: an as-of read before the edit must return v1"
    );

    let mut stale = edit.clone();
    stale.command_id = Uuid::new_v4();
    let failure = err(name, "CTL-6", d.execute(&stale, Actor::Privileged).await);
    assert_eq!(
        failure.code, "ontology_action_revision_precondition_failed",
        "[{name}] CTL-6: a stale expected_revision must be refused, got {failure:?}"
    );

    let replay = ok(name, "CTL-6", d.execute(&edit, Actor::Privileged).await);
    assert_eq!(
        replay.revision.id, v2.revision.id,
        "[{name}] CTL-6: replaying a command_id must return the stored receipt"
    );
    assert_eq!(
        ok(name, "CTL-6", d.history(target).await).len(),
        2,
        "[{name}] CTL-6: neither the refusal nor the replay may write a row"
    );

    // CTL-7 — the TRAVERSAL surface answers today. CC-02/03/04/11 read a red out
    // of an empty edge set, so this pins that an empty graph means "no edge was
    // ever written" and not "traverse is broken / unreachable / 403". A built-in
    // instance has no links because nothing on the action path writes `ont_links`.
    let graph = ok(name, "CTL-7", d.traverse(target, 1).await);
    assert_eq!(
        graph.root, target,
        "[{name}] CTL-7: the graph is rooted here"
    );
    assert!(
        graph.nodes.iter().any(|n| n.instance_id == target),
        "[{name}] CTL-7: the root instance must be hydrated as a node, got {:?}",
        graph.nodes
    );
    assert!(
        graph.edges.is_empty(),
        "[{name}] CTL-7: a built-in instance has no links, got {:?}",
        graph.edges
    );
}

// ===========================================================================
// Entry points — one binary, four tests.
// ===========================================================================

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn control_surfaces_rest(owner_pool: PgPool) {
    let h = Harness::bootstrap(owner_pool).await;
    let d = rest::RestDriver::new(&h);
    controls(&d).await;

    // CTL-4 (rest only) — a mistyped PATH is axum's fallback 404 with an EMPTY
    // body, which is what proves CTL-3's 404 is a DOMAIN 404 and not a routing
    // accident.
    let (status, body) = d
        .raw_get("/api/v1/ontology/object-typo/customer", &h.admin_token)
        .await;
    assert_eq!(
        status.as_u16(),
        404,
        "[rest] CTL-4: a mistyped path must 404"
    );
    assert!(
        body.is_empty(),
        "[rest] CTL-4: the fallback 404 must have an EMPTY body, got {:?}",
        String::from_utf8_lossy(&body)
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn control_surfaces_store(owner_pool: PgPool) {
    let h = Harness::bootstrap(owner_pool).await;
    let d = store::StoreDriver::new(&h);
    controls(&d).await;

    // CTL-5 (store extension) — the same call with NO tenant context bound.
    // Unreachable through REST: the router arms `CURRENT_ORG` from the verified
    // token before any handler, so only the store driver can prove fail-closed.
    let type_id = ok("store", "CTL-5", d.resolve_type(BUILTIN_ACTION_TYPE).await);
    let failure = d.execute_unarmed(type_id).await;
    assert_eq!(
        (failure.code.as_str(), failure.message.as_str()),
        (
            "internal",
            "no tenant context is bound to the current request"
        ),
        "[store] CTL-5: an unarmed app.current_org must fail closed, got {failure:?}"
    );
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn company_scenario_rest(owner_pool: PgPool) {
    let h = Harness::bootstrap(owner_pool).await;
    scenario(&rest::RestDriver::new(&h)).await;
}

#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn company_scenario_store(owner_pool: PgPool) {
    let h = Harness::bootstrap(owner_pool).await;
    scenario(&store::StoreDriver::new(&h)).await;
}
