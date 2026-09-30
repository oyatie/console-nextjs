> **Post-pivot reconciliation (2026-08-03):** Current product authority is [`docs/current/PRODUCT.md`](../current/PRODUCT.md) (+ ROADMAP/DELIVERY). Pivot history: [`docs/PIVOT-2026-07-28.md`](../PIVOT-2026-07-28.md) (non-authority). This document is subordinate and must not authorize work outside Ontology/Foundry/Policy → Company/OrgUnit/Employee → HR/Payroll. Conflicting ERP, finance, communications, compliance-product, ingest/evidence, office-editing, AI-judgment, frontend, or Buck2 execution claims are historical context or **HOLD** until explicitly reconciled by a current candidate.

# CATALOG.md — the transliteration rules for adding a domain type

> **This is the `PORTING.md` analogue.** Bun's rewrite scaled to 64 agents because a Zig→Rust port is
> *transliteration*: a known-correct reference, a fixed target, and a mechanical rule set meant an agent
> could not design the wrong thing, because it was not designing. This file is the rule set. Its job is
> to make "add a domain type" mechanical rather than a design exercise.
>
> Status: **prep artifact.** Not authority. The reference implementation (Company + OrgUnit) does not exist yet;
> until it does, every rule here is provisional and the reference wins where they disagree.
>
> **Current HOLD (2026-08-03):** the existing `company_conformance` suite is an isolated
> generic-engine regression over five instance-backed fixtures. It omits Person and does not prove
> the projected single-writer boundaries this catalog requires for Company, Person, Employment, or
> PayRun. It must not be frozen or used as a product/fan-out target. Replacement conformance and
> projection work wait for explicit owning-port contracts.

## The one rule that matters

**Catalog types are `Instance`-backed engine types, not bespoke domain crates.**

Verified 2026-07-28. `backend/crates/ontology/domain/src/lib.rs` defines:

| Enum | Variants | Meaning |
|---|---|---|
| `BackingKind` | `Projected` | projects an existing domain table (WO / employee / equipment) |
| | `Instance` | user-authored type with an **owned effective-dated instance store** |
| `ActionDispatch` | `ProjectedUsecase` | routes writeback through a domain crate's existing use-case |
| | `InstanceRevision` | appends a revision to the owned instance store |
| `LinkCardinality` | `OneOne` / `OneMany` / `ManyMany` | typed link arity |

Choose **`Instance` + `InstanceRevision`** for the catalog. Reasons, in order of weight:

1. **The substrate is already correct there.** `ontology/adapter-postgres/src/instances.rs` stores state
   as a fold over immutable, effective-dated, fixity-chained revisions — `attributes` is never
   `UPDATE`d, as-of reads use `valid_from <= t AND (valid_to IS NULL OR t < valid_to)`, and every
   revision is bound into a per-`(org, instance)` SHA-256 chain. `Projected` types inherit the *legacy*
   model instead (`platform/db/versioning.rs`: mutable rows plus a JSON snapshot sidecar), which has no
   as-of and no fixity.
2. **Zero build cost.** An engine type is data in the registry: 0 new crates, 0 `BUCK` files, 0 reindeer
   runs. A bespoke crate costs a `Cargo.toml` member entry (an unmatched glob breaks the build *for
   every lane*) plus a per-crate `BUCK` file.
3. **It is what makes the work mechanical.** Each type becomes the same shape of declaration, so lanes
   transliterate rather than architect.

Use `Projected` **only** when a domain crate already owns the table and its business rules, and the
writeback must go through that crate's use-case. Never create a second writeback path to a table a
domain crate owns.

## Per-type declaration checklist

For each catalog type, state exactly these and nothing else:

- [ ] **`type_id`** and label
- [ ] **`backing`** — `Instance` (default per the rule above) or `Projected` **with the owning crate named**
- [ ] **typed props** — key, `dataType`, unit, options for enums. Free text is a smell: §4-19 says a
      property that is not typed cannot participate in policy, automation, or analytics
- [ ] **link types** — `{rel, fromType, toType, cardinality}`. A free-string `rel` is a defect
- [ ] **actions** — key, label, `ActionDispatch`. Every action is policy-evaluated and audited
- [ ] **derived analytics** — expression, and the single sentence of arithmetic behind it. No
      black-box values (§4-30/§4-38: deterministic, reproducible, explainable)
- [ ] **lifecycle** — the states this type actually uses from `InstanceLifecycleState`
- [ ] **the conformance scenario step it satisfies** — if none, the type is not needed yet

## The catalog

Scope boundary is load-bearing: **org, employee, HR, payroll. That's it.**

| # | Type | Backing | Notes |
|---|---|---|---|
| 1 | **Company** | `Projected` | planned projection of existing organization truth; owning port and conformance are not yet designed — **HOLD** |
| 2 | **OrgUnit** | `Instance` | proposed reference type linked to Company — **HOLD** until Company projection is defined |
| 3 | **JobPosition** | `Instance` | proposed link to OrgUnit (`OneMany`); the seat, distinct from its occupant — **HOLD** |
| 4 | Person | `Projected` | projects existing employee truth; no platform-party auto-linking |
| 5 | Employment | `Projected` | canonical HR application writer; Person × JobPosition over time |
| 6 | PayRun | `Projected` | existing payroll writer; period-scoped, derived from Employment + attendance |

Person, Employment, and PayRun remain domain-owned projected writers and are not generic fan-out
work. All expansion is HOLD until replacement product conformance plus explicit owning ports prove
zero overlapping writes; the existing `company_conformance` suite cannot satisfy that condition.

## Anti-patterns (each has already cost this repo)

- **A bespoke crate for a catalog type.** Costs the Cargo member entry and a `BUCK` file, and forfeits
  the effective-dated store.
- **A second writeback path.** Projected writes route through the domain use-case. Always.
- **Free-text where an enum belongs.** It removes the object from the dynamic layer entirely.
- **A derived value without its formula.** §4-36-⑤: if a number has no formula, constituent items, and
  applied rule version, it does not belong on screen.
- **Inventing catalog semantics.** If the correct declaration is not derivable from the reference or
  from verified engine behaviour, stop and ask. This session produced 38 fabricated documentation
  claims and 21 misclassifications of live infrastructure precisely by not doing that.
