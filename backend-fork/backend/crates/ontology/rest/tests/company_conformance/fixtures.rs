//! OWNED index — PRE-RESERVED. All five `pub mod` lines already exist below, so a
//! lane edits ONLY its own `fixtures/<type>.rs` and touches no shared file.
//!
//! Deliberate: "each lane appends one line here" is a claim about agent behaviour, not
//! a merge property. Two lanes appending to one file tail conflict in the same hunk,
//! and git cannot know the lines are independent. Pre-reserving makes the disjointness
//! structural instead of procedural. Adding a SIXTH type is a deliberate act that edits
//! this file once, outside any lane.
//!
//! Everything under `fixtures/` is per-type scenario DATA plus that type's own
//! `declare`: the full required param bag for the auto-attached generic `create`
//! action, and the object type the bag is a bag FOR.
//!
//! `declare` exists because the suite as first landed was UNSATISFIABLE. It
//! resolves the five types and classifies the result, but nothing between
//! `Harness::bootstrap` and the first `resolve_type` could create one:
//! bootstrap's only ontology call is `seed_governed_config_object_types`, whose
//! body is a closed `install_builtin_catalog` over a digest-allowlisted manifest
//! with no extension point, and every `fixtures::*` entry point is called from
//! INSIDE `if ids.contains_key(..)` — i.e. only after the type already resolved.
//! A lane could not have turned its ids green by editing only its own file, so
//! the seam is opened once, here, outside every lane.
//!
//! It is deliberately NOT the built-in catalog: that path is digest-allowlisted
//! per `BUILTIN_CATALOG_VERSION`, which is the one genuine serialised lock in
//! this fan-out, and routing five lanes through it would serialise all five.
//!
//! Two hard rules for a lane's file:
//!   1. The bag is the FULL required param set on EVERY call, including edits.
//!      `apply_edits` resolves an absent param to `Value::Null` and inserts it
//!      over the base (`ontology/application/src/lib.rs:288,296`).
//!   2. The bag contains ONLY declared params. `validate_params` rejects an
//!      undeclared key, and the target asserts every sent param round-trips into
//!      the stored attributes verbatim.
//!
//! The signatures are fixed by the target; the bodies are the lane's.

#[path = "fixtures/company.rs"]
pub mod company;
#[path = "fixtures/employment.rs"]
pub mod employment;
#[path = "fixtures/job_position.rs"]
pub mod job_position;
#[path = "fixtures/org_unit.rs"]
pub mod org_unit;
#[path = "fixtures/pay_run.rs"]
pub mod pay_run;

use crate::harness::Harness;

/// Declare every lane type that has been built, in `LANE_TYPES` order — which is
/// also dependency order, since a unit sits under a company, a position within a
/// unit, an employment fills a position, and a pay run pays employments.
///
/// Called once at the tail of `Harness::bootstrap`, AFTER the built-in catalog:
/// `0204_ontology_catalog_additive_upgrade.sql:119-123` raises 23514
/// `ontology_builtin.empty_org_required` when an org already holds
/// `ont_object_types` rows but has no prior `ont_builtin_catalog_installs` row,
/// so a type created before the install aborts bootstrap itself.
///
/// An unbuilt type's `declare` is a no-op, which is what keeps its scenario ids
/// RED with the pinned `UNKNOWN_TYPE` signature instead of failing some other
/// way. Each is a separate call rather than a loop so that landing a lane is an
/// edit to that lane's file alone.
pub async fn declare_all(h: &Harness) {
    company::declare(h).await;
    org_unit::declare(h).await;
    job_position::declare(h).await;
    employment::declare(h).await;
    pay_run::declare(h).await;
}
