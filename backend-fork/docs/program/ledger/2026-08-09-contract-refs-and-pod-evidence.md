# Authority tip — a `$ref` that resolves nowhere, and a POD rule published weaker than it is enforced

**Date:** 2026-08-09
**Kind:** authority tip (T) for the contract-refs and POD-evidence candidate
**Scope:** `backend/crates/contracts/`, `backend/crates/logistics/rest/`, and one schema property in
`backend/openapi/openapi.yaml`.
**Not product authority.** Clears no HOLD. No migration, no new dependency, no production promotion.
**Head SHA (authority tip parent / candidate C):** `d37897aa5c56fffffc5c05cabfed173baaa9877e`
**(Prior reviewed tip before finish-lane product commit):** `c6dea4d2075e42b0e4c3006e8a1809c5f646c367`
**Close base:** `97a45cfc7f937154eeed27f457e6ee50851c4229` (`origin/main`, #783)
**Review identities:** Codex connector automated review on PR #620 (7 threads, all resolved);
Cursor-native finish/self-critic lane on worktree `console-integration` (no CLI critic wrapper).
**Authority train note:** Finish-lane product (Buck `RESOURCE_CONFIG`, regenerated BUCK, class-sweep
hardening) lives on candidate `C=d37897aa5`. This tip commit is authority-only (ledger) so `C..T`
stays within the allow-listed train.

## Lane delivery evidence

Recorded separately for each lane because AGENTS.md binds review and recovery to the exact
candidate. Shared CI/Buck wiring that landed with this tip is listed once under Verification.

### `console-qjb` (contracts `$ref` composition)

**Status:** CLOSED already-on-tip (GH #703); product on main via #620 / later Fragment work — no open residual in this ledger.

| Field | Record |
|---|---|
| Pre-mortem | A `$ref` typo that names a non-existent component section composes clean and ships into the published contract; clients and generators follow a pointer that resolves nowhere. |
| Blast radius | `backend/crates/contracts/` only for mechanism; compose callers that embed fragments (todos face drift test keeps `schema_refs` signature). No migration, no OpenAPI property change from this lane. |
| Detection | `cargo test -p console-contracts` — compose suite rejects `#/components/schema/Todo` and the six "points somewhere else" spellings; drift gate still green. |
| Rollback | Revert the contracts commits on this branch; published OpenAPI and logistics crate unchanged by this lane alone. |
| Stop conditions | Widening to reject well-formed refs into `responses`/`parameters` (nine live todos refs); adding a YAML dependency to `console-contracts` without a layer decision (`console-ann`). |

### `console-jf3` (POD evidenceReference publication)

| Field | Record |
|---|---|
| Pre-mortem | Clients read `pattern: '^evidence://'` while the server and migration 0212 reject outside `[A-Za-z0-9._/-]` and length 19..=411 — predictable 400s invisible from the contract. |
| Blast radius | One schema property in `backend/openapi/openapi.yaml`; unit tests in `backend/crates/logistics/rest/`; CI domain-unit wiring + Buck resource metadata so the new test actually builds and runs. |
| Detection | `cargo test -p console-logistics-rest --lib` — `published_schema_and_validator_agree_on_concrete_references` (1 passed); oracle proved RED by reverting to weak `^evidence://`. |
| Rollback | Revert the logistics/OpenAPI/CI/Buck commits; constants 19/411/class remain in validator + migration 0212 until republished together. |
| Stop conditions | Retyping 19/411/class into a fourth site; claiming `check:ci-preflight` inherited while `ci.yml` changed; full Unicode class enumeration or YAML-parser `$ref` totality without the tracked bead (`console-ann`). (`console-5yn` CLOSED via squash-merge #767 / `4417bb377`.) |

## Summary

Two findings from PR #612's review, each dispatched to a lane whose brief demanded a reproduction
before a fix.

- **`console-qjb` — a `$ref` naming a component section that does not exist composed clean.**
  The finding as filed said "component references that are not schemas go unresolved" and attached
  no failing case. Reproduced narrowly: `#/components/schema/Todo`, a one-character typo of
  `schemas`, passed because `schema_refs` split on the literal `#/components/schemas/` — so a ref
  naming any other section was invisible rather than checked. That ref resolves in NO document and
  shipped into the published contract silently. Two further defects of the same shape were found
  while fixing it: the rule was stated over a substring rather than over ref VALUES, and a pointer
  was checked only where it followed `$ref:` rather than wherever it is written.
  Refs into sections OpenAPI defines but `Fragment` does not model (`responses`, `parameters`) are
  still accepted, deliberately: `compose` cannot see the published document, so the section NAME is
  the only thing checkable from a fragment alone, and the todos face ships nine such refs today.
  `schema_refs` keeps its exact signature, so the existing drift test is unaffected.

- **`console-jf3` — the POD evidence rule was published weaker than it is enforced.**
  `validate_evidence_reference` enforces the `evidence://` scheme plus `[A-Za-z0-9._/-]` with total
  length 19..=411, mirroring migration 0212's CHECK, while the contract published only
  `pattern: '^evidence://'`. Clients could not see the rule they would be rejected by. The published
  schema now carries `minLength: 19`, `maxLength: 411` and the real character class, and the test
  sweeps the published evidence CLASS rather than sampling it, so agreement is measured over the set
  rather than over a handful of chosen strings.

## What the lane's own review still holds against this, and why it lands anyway

Three rounds of adversarial review ended **UNCONVERGED**: seven blockers on contracts,
three on logistics, every one `provenByExecution`. That verdict is recorded rather than
summarised away, because this candidate lands on an owner decision and not on a green.

The seven are one finding wearing seven faces: the two-axis scan is total over the
POSITIONS it covers and **not over YAML**. A `$ref` in flow style, a quoted pointer with
an internal space, a `discriminator.mapping` in implicit schema-name form, and a foreign
prefix ending in a character that is a delimiter outside a quoted scalar and legal inside
one — all still compose clean. `console-ann` carries them with the reason they must not be
patched individually: two prior revisions each replaced one enumeration with another, and
this would be the third spelling. The total primitive is a YAML parser, which this
zero-dependency crate cannot adopt without a decision about the layer's dependency surface.

What changed before landing is the **claim**, not the coverage. The module doc asserted
"Every `$ref` points into this document" as an invariant; adversarial review proved that
false in four positions. It now states what the scanner sees, and names the four gaps and
their cause. A partial control is defensible; a partial control describing itself as total
is the exact failure this crate exists to prevent in published contracts, and it had
committed that failure in its own module doc.

Two of the logistics three were tracked as `console-5yn`, including the sharper one: the
agreement corpus derives from the published pattern it audits, so the two sides are not
independent. That bead is CLOSED via squash-merge #767 (`4417bb377`); GH #691 closed with it.
The third — `gen_first_party.py` crashing on the crate's first `#[cfg(test)]` module — is
fixed here.

## Why one PR and not two

Both lanes ran against the same tip with disjoint owned roots, and neither touches the other's
crate. The only shared file is `backend/openapi/openapi.yaml`, where the logistics lane changed
exactly one schema property — recorded here because a one-line change to a file outside a lane's
crate reads as scope creep unless it is stated.

## Verification

**A stale claim corrected, because it was written true and became false.** An earlier revision of
this section read: *"`check:ci-preflight` is inherited from `main`: this branch changes four files,
none of which is `ci.yml`…"*. That was accurate when written and stopped being accurate one commit
later, when wiring the new unit tests into the `domain-unit` job changed `ci.yml`,
`scripts/check-ci-preflight.mjs` and `tools/buck/gen_first_party.py`. The branch changes **twelve**
files, not four. A verification claim nobody re-derives after the next commit is the exact way these
documents rot, so the inheritance argument is deleted rather than repaired: the gate was RUN.

Executed, with results:

| Command | Result |
|---|---|
| `cargo test -p console-contracts` | 30 passed, 0 failed |
| `cargo test -p console-logistics-rest --lib` | 1 passed, 0 failed — discovered 1 lib test (`published_schema_and_validator_agree_on_concrete_references`), executed 1 |
| `python3 tools/buck/gen_first_party.py` then `buck2 build //backend/crates/logistics/rest:console-logistics-rest-unit` | BUCK regenerates with `//backend/openapi:openapi.yaml` external on library + unit; **BUILD SUCCEEDED** (build id `0ffef5a1-81ea-4d6b-b0f0-a949c26fe61a`) |
| `npm run check:ci-preflight` | PASS |
| `node --test scripts/check-ci-preflight.test.mjs` | 53 passed, 0 failed |
| `npm run check:executed-tests` | PASS — the new binary reachable, not exempted |
| `node scripts/check-platform-contract-drift.mjs` | PASS, 17 backend operations |
| `check:doc-links` / `check:doc-manifest` / `check:doc-citations` / `check:js-test-reachability` | PASS |
| `cargo fmt --all -- --check` | clean |
| POSTFLIGHT `git status --porcelain` after every generator | empty |

The logistics test is one `#[test]` that SWEEPS the published evidence class rather than sampling it,
so one result covers the class. Its oracle was proved before it was wired: reverting the published
schema to the old weak `^evidence://` fails it with a concrete counterexample —
`published {pattern: "^evidence://", minLength: None, maxLength: None} accepts=true, the server
accepts=false. Candidate: "evidence://aaaaaaa"` — eighteen characters, one under the enforced floor.

## Ledger-close verification (2026-08-14)

Recorded at the close of `console-5yn` / `console-qjb` open-bead prose, on rebase base
`97a45cfc7f937154eeed27f457e6ee50851c4229` (`origin/main`, #783). The closure adds disposition
only; the candidate-bound evidence recorded above is unchanged.

| Command | Result |
|---|---|
| `git diff --check` | clean |
| `npm run check:doc-manifest` | OK (448 markdown files) |
| `npm run check:doc-links` | OK (448 markdown files) |
| `npm run check:doc-citations` | `docs/ideas/ecosystem-plan-DRAFT.md`: 679 citations, 0 BROKEN / 0 UNVERIFIABLE / 19 MISSING (nonfatal). `docs/program/false-green-gate-holes.md`: 38 citations, 0 BROKEN / 0 UNVERIFIABLE / 0 MISSING |
| `npm run check:adrs` | 39 ADRs, 6 design notes |
| `npm run test:adrs` | 29 passed, 0 failed |
| `npm run check:foundation-gates` | PASS |
| `npm run test:foundation-gates` | 6 passed, 0 failed |
| `npm run check:ci-preflight` | PASS |
| `npm run test:verify` | 13 passed, 0 failed |
| `npm run verify` | PASS (fast tier) |
| `node scripts/console/simulate-main-gate.mjs <lane-worktree> <authority-tip> 97a45cfc7f937154eeed27f457e6ee50851c4229` | MAIN BOOTSTRAP GATE: PASS — C and T both pass the pinned SSH signature, `C..T` is authority-only, synthetic M tree equals T. Authenticated candidate checks ALL PASS (validate-console-truth-ledger STRUCTURALLY_VALID_HOLD_PRESERVED + plan-fanout exact-M admission) on a clean detached C worktree. (`<authority-tip>` is the commit containing this ledger, so its SHA is not inlined; base is pinned to `97a45cfc7f937154eeed27f457e6ee50851c4229`.) |

## HOLDs remaining

Unchanged. No production promotion, no frontend, no payment execution, no invented compliance scope.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "contracts"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Essentialism / YAGNI",
    "Red Team",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "The finding as filed attached no failing case, so the first deliverable was a reproduction rather than a fix; what it reproduced was narrower and more concrete than what was reported.",
    "Essentialism / YAGNI": "Refs into sections OpenAPI defines but Fragment does not model are left accepted, because compose cannot see the published document and the section name is the only thing checkable from a fragment alone.",
    "Red Team": "A one-character typo of `schemas` was the attack: it named a section that exists in no document and passed a check that split on a literal substring.",
    "Operability / Day-2": "The four positions the scanner still misses are named in the module doc with their cause, so the next reader inherits the gap rather than the belief that it is closed.",
    "Blast-radius / cell-based": "The total fix needs a YAML crate in a zero-dependency crate, which pulls in the layer dependency surface and reindeer vendoring; that blast radius is why it is a tracked decision rather than a lane change.",
    "Telemetry-first": "The POD test sweeps the published evidence class rather than sampling it, so agreement between contract and validator is measured over the set instead of over chosen strings.",
    "Zero-trust / defense-in-depth": "The published contract is no longer trusted to describe the enforced rule: a test sweeps the published evidence class against the validator, and its oracle was proved by reverting the schema and observing a named counterexample."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "A $ref naming a component section that does not exist composed clean, because the rule was stated over a substring rather than over ref values.",
    "A pointer was checked only where it followed `$ref:` rather than wherever it is written.",
    "The POD evidence rule was published as `^evidence://` while the validator and migration 0212 enforce a character class and a 19..=411 length.",
    "The ledger's own verification section claimed the branch changed four files and inherited check:ci-preflight from main; the wiring commit made both false, and it went unre-derived."
  ],
  "decisions_changed_or_rejected": [
    "Rejected widening the check to every section OpenAPI defines, because compose cannot see the published document and nine live refs would break.",
    "Rejected retyping 19 and 411 into the schema as a third spelling without saying so; the constraint is recorded in the ledger so the next reader knows the three sites must move together.",
    "Rejected classifying this standard-risk with an empty risk_domains array. That satisfied the validator by dropping the true domain rather than by being accurate: the change edits a published contract and a contract-checking gate, so `contracts` is its domain and the record is high-risk with the four mandatory lenses."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
