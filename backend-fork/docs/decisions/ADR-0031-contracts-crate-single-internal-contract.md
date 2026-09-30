---
id: ADR-0031
status: accepted
doc_status: review
date: 2026-07-30
owner: jasonlee
decision: contracts-crate-single-internal-contract
amends: [ADR-0009]
related: [ADR-0001, ADR-0009, ADR-0012]
---

# ADR-0031 — A Rust contracts crate is the internal contract; `openapi.yaml` is generated from it

## Status

**Accepted 2026-07-30 · doc_status `review`.** Source: owner decision 2026-07-30, reached
independently of the twelve-theme ADR adjudication. This record amends the
*contract mechanism* half of ADR-0009 only. Its dual-native client-generation, dual-build, and
per-slice sequencing clauses are equally divergent from HEAD (evidence below) but are outside this
record's scope and need their own decision. Deliberately split from the console frontend charter:
the contracts crate benefits the backend whether or not the console ever starts, so it stands as
its own decision rather than as a clause of a frontend record.

> **Current observation (2026-08-03):** The accepted record below is preserved as
> written. Since acceptance, the OpenAPI title changed to `Console API`, the
> tautological `check:openapi-app` script was retired, and
> `check:platform-contract-drift` now compares only platform method/path inventory
> with committed text. No contracts-crate emitter or schema-to-wire fidelity proof
> exists, so the accepted target remains unimplemented.

## Context

ADR-0009 is accepted and its Decision (`ADR-0009-dualnative-swiftkotlin-parity-strategy-via-single.md:20`)
read, **before this record amended it**, in part: *"one utoipa-emitted `openapi.yaml` is the single contract; CI generates
ts/swift/kotlin clients and fails on drift (T1.9); both apps build from every release tag (T1.8
dual-build gate)"*. Both halves of that sentence describe machinery that is absent from HEAD.
An ADR Decision line is prose about code, not code, so each claim below was checked against the
tree rather than against the adjudication.

**The emitter does not exist.** `git grep -in utoipa` returns seven hits, every one of them prose:
`ADR-0009:20` itself and six lines under `docs/ideas/`. There is no `utoipa` dependency in any
manifest and no derive on any type.

**`openapi.yaml` is hand-maintained and served verbatim.** `backend/openapi/openapi.yaml` is 35,935
lines. Its `info.title` (`backend/openapi/openapi.yaml:3`) still reads `Maintenance FSM Backend API`
— the pre-rename product name. `backend/app/src/lib.rs:214` embeds the file with
`include_str!("../../openapi/openapi.yaml")`; the route is registered at `backend/app/src/lib.rs:2889`
and the handler returns the embedded bytes unchanged (`backend/app/src/lib.rs:3486`).

**The client-generation and dual-build artifacts are gone.** `clients/`, `ios/`, and `android/` do
not exist at HEAD. `gen:api`, `check:ts`, `check:kotlin`, and `check:swift` are absent from
`package.json`; the only remaining OpenAPI script is `check:openapi-app` (`package.json:10`).
`git grep -iln 'swift\|kotlin' -- .github/ package.json` returns nothing at all, so neither the
T1.9 drift gate nor the T1.8 dual-build gate has a surviving artifact.

**What the existing gates actually guarantee is narrower than "contract".** `backend/app/tests/openapi_drift.rs`
(884 lines) proves *route coverage*: `configured_route_inventory_includes_each_configured_surface`
(`:271`), `configured_route_inventory_covers_router_route_calls` (`:304`), and
`openapi_yaml_covers_configured_route_inventory` (`:351`, which compares OpenAPI **path keys** only).
The rest is hand-written spot-checks on named shapes (`:367`, `:410`, `:426`, `:453`) plus platform
operation keys (`:548`, `:571`). **No test compares a documented request or response schema to the
Rust type the handler actually serializes.** `scripts/check-openapi-app.mjs` proves only that the
served document is byte-identical to the committed one (`:23`, `:67`–`:73`) — identity between two
copies of the same hand-written text, not fidelity to code. The 36k lines of schemas are therefore
unverified prose about the API.

`docs/decisions/README.md:6` governs this situation: implementation divergence from an ADR is a
governance gap, not silent supersession, and is reconciled through a new decision. This record is
that reconciliation for the contract mechanism.

**What survives is the principle.** ADR-0009's thesis — parity is enforced structurally, not by
discipline — is correct and load-bearing. Only its named mechanism was never built.

## Decision

*Accepted; everything below is in force.*

1. **A dedicated contracts crate is the single internal API contract.** It holds wire DTOs plus the
   OpenAPI derive metadata, and it is the only artifact both the backend REST layer and any
   first-party Rust frontend depend on for request/response shape.
2. **The DTOs are dedicated wire types, never domain types.** Two independent reasons. ADR-0001
   layers `kernel ← domain ← application ← adapter ← {rest, worker} ← app`
   (`backend/ci/gates/layer-boundary/src/lib.rs:5`–`:9`), so a frontend crate depending on `domain`
   skips layers; and domain types carry invariants and constructors the wire does not need, which
   would turn every internal refactor into a frontend break.
3. **`openapi.yaml` becomes a generated artifact with a diff gate.** The committed file is emitted
   from the contracts crate; CI regenerates it and fails when the committed and emitted documents
   differ. Generation must also correct `info.title`, which must not survive into generated output.
4. **`openapi.yaml`'s role changes from source to deliverable.** It stops being the frontend's
   source of truth and becomes an external contract document. `backend/app/tests/openapi_drift.rs`
   is retained as the instrument proving the emitted document still describes the served surface,
   because a Rust frontend will no longer notice when the YAML drifts.
5. **Drift becomes a compile error for the frontend**, which is the whole point: the frontend
   consumes the crate, not the YAML.
6. **Scope limits.** This record does not authorize console implementation, does not reinstate
   ts/swift/kotlin client generation or the dual-build gate, and does not make any Korea compliance
   claim.

## Decision drivers

- ADR-0009's structural-parity principle is currently unimplementable as written, because its named
  emitter does not exist.
- Schema fidelity is today unenforced by any gate; the only enforcement is review discipline, which
  is exactly what ADR-0009 set out to replace.
- ADR-0012 keeps the contract and its consumers versioned atomically in one repository, so a
  generated-in-tree contract is cheaper here than a published artifact would be. (ADR-0012's own
  "generated clients" premise is equally absent from HEAD; that is noted, not decided here.)
- The backend gains verified schemas regardless of console timing, which is why this is not a
  frontend decision.

## Alternatives considered

**Keep the hand-maintained `openapi.yaml` and add a schema-fidelity test.** Rejected. A test that
compares 36k lines of YAML against handler types is a second, weaker generator with no single source
of truth — the failure mode is a gate that is perpetually amended to match whichever side was edited
last.

**Spec-first: generate Rust DTOs from `openapi.yaml`.** Rejected. It leaves the hand-edited 36k-line
document as the source of truth and gives the backend no compile-time break when a handler and its
documented schema diverge.

**Reuse domain types with serde derives instead of dedicated DTOs.** Rejected on driver 2 above:
layer skip plus invariant leak.

**Publish a versioned contract package consumed from outside the repository.** Rejected against
ADR-0012's contract atomicity; it reintroduces the version skew that decision exists to prevent.

## Why the contracts crate was chosen

It is the only option in which the contract has exactly one source, both consumers fail loudly, and
the failure is a compile error rather than a review comment. It makes ADR-0009's own thesis true for
the first time rather than aspirational, and it retires 35,935 lines of hand-maintained prose that
no gate has ever verified.

## Consequences

+ Request and response schemas gain their first structural enforcement; the generated document
  cannot silently disagree with the types it was emitted from.
+ 35,935 lines of hand-maintained YAML stop being authored and become build output.
+ The pre-rename `info.title` is corrected as a by-product of generation rather than as a
  documentation chore.
− **There are no DTOs to extract.** With no `utoipa` in the tree, this is new authoring across every
  REST surface, not a lift of existing derives. That cost is the honest price of the enforcement
  never having existed.
− **The frontend build becomes coupled to backend crates.** A backend change cannot land while the
  frontend is red. That is the mechanism working as designed, not a defect — but it is a real
  constraint on the landing model and must be reflected in it before the frontend consumes the
  crate.
− The layer-boundary gate does not currently enforce decision 2. `Layer::Adapter | Layer::Platform`
  may depend on `Application`, `Domain`, and `Kernel`
  (`backend/ci/gates/layer-boundary/src/lib.rs:97`–`:102`), and any unmatched crate under `crates/`
  falls back to `Layer::Adapter` (`:185`–`:187`). So a contracts crate placed under
  `crates/platform/` would be *permitted* to depend on `domain`. Until the follow-up below lands,
  "dedicated DTOs, not domain types" is a review rule, not a compiler-enforced one.
− `openapi.yaml` diffs become generated noise in review and need a CI-side regeneration check rather
  than human reading.

## Follow-ups

1. Choose the crate's path and layer classification, then extend
   `backend/ci/gates/layer-boundary/src/lib.rs` with a contracts layer whose `allowed_deps` excludes
   `Domain`, so decision 2 is mechanically enforced instead of reviewed.
2. Wire the emitter and the diff gate into CI as a required check.
3. Before switching authority, emit against the current committed document and measure the
   divergence; a large diff is evidence about the hand-written schemas, not a reason to hand-edit
   the generated output.
4. Retain and re-verify `backend/app/tests/openapi_drift.rs` coverage before any existing OpenAPI
   consumer is deleted, so the deletion does not remove the only thing that noticed drift.
5. Record the landing-model constraint from the coupling consequence wherever branch/landing shape
   is decided.

## Reciprocal record landed on acceptance

`docs/decisions/README.md:9` requires amendment to be explicit in both records and `:26` requires
relationship keys to be reciprocal where applicable. The following edits landed atomically with the
status change:

1. **`ADR-0009` frontmatter gained a key it did not carry.** Its frontmatter was
   `id`, `status`, `doc_status`, `date`, `owner`, `consensus`, `related`
   (`ADR-0009-dualnative-swiftkotlin-parity-strategy-via-single.md:1`–`:9`), with
   `related: [ADR-0012]` at `:8`. There was **no `amended_by` key**, so this created it rather than
   appending: `amended_by: [ADR-0031]`. `scripts/check-adrs.mjs:399`–`:409` makes that reciprocal
   edit mandatory in the same commit as this record declaring `amends: [ADR-0009]`, which
   `:421`–`:425` permits only once this record's status is `accepted`.
2. **`ADR-0009`'s README index row changed.** It previously read
   `| [ADR-0009](…) | accepted | Dual-native Swift/Kotlin employee apps from one OpenAPI contract; `coss-rn` is outside this scope |`.
   Its status cell became `accepted, amended` and its scope cell names this amendment, in the
   style already used for ADR-0005 and ADR-0015.
3. **This record's own index row moved from `proposed` to `accepted`**, and the *Effective
   relationship graph* section gained a bullet stating that the contract-mechanism clause of
   ADR-0009 is amended while its dual-native scope is untouched by this record.
4. **`ADR-0009`'s Decision text was edited in place, because a reciprocal key alone would leave
   a false sentence standing in an authoritative record.** In the sentence at `ADR-0009:20`, the
   clause

   > one utoipa-emitted `openapi.yaml` is the single contract   ← as it read before this amendment

   is false as verified above and became

   > one `openapi.yaml` emitted from the ADR-0031 wire-DTO contracts crate is the single contract,
   > with a CI diff gate failing when the committed document differs from the emitted one

   Only that clause is within this record's scope. The remaining clauses of `:20` — ts/swift/kotlin
   client generation with a drift gate (T1.9), the dual-build gate (T1.8), and web+Android-then-iOS
   sequencing — are also false against HEAD by the evidence in this record's Context, and they need
   a separate accepted decision; they must not be quietly rewritten under this one.
5. **`ADR-0009`'s title is left alone by this record.** "Dual-native (Swift+Kotlin) parity strategy
   via single OpenAPI contract + CI parity gate" remains accurate about the clause amended here and
   stale about the clauses that are not. Whoever reconciles the dual-native half owns that title.
