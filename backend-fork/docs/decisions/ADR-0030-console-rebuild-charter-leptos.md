---
id: ADR-0030
status: accepted
doc_status: review
date: 2026-07-30
owner: jasonlee
decision: console-rebuild-chartered-on-leptos-planning-only
amends: [ADR-0025]
amended_by: [ADR-0041]
related: [ADR-0001, ADR-0009, ADR-0012, ADR-0018, ADR-0021, ADR-0022, ADR-0023, ADR-0025, ADR-0041]
---

# ADR-0030: Console rebuild chartered on Leptos; planning authorized, implementation gated

## Status

**Accepted 2026-07-30.** Source: D4 (theme T12) in
`docs/ideas/adr-adjudication.md`, resolved with owner decisions captured
2026-07-30 in `docs/ideas/d4-frontend-charter.md`. This record amends ADR-0025.
The withdrawals in §3 are in effect and the reciprocal edits in the final section
landed with acceptance. ADR-0025 remains accepted and authoritative for
everything §3 does not name.

> **Current observation (2026-08-03):** The accepted text below is preserved as
> written. Since acceptance, CI removed workflow path filters and the OpenAPI title
> changed to `Console API`; no contracts-crate emitter or schema-to-wire fidelity
> proof exists. These observations do not rewrite the Decision or open its frontend
> implementation gate.

> **Current observation (2026-08-25):** The accepted text below is still preserved
> as written. ADR-0041 accepts `Layer::Ui` so `console-<domain>-ui` members may
> exist — the ADR-0001 amendment §6 named. ADR-0041 does not resume frontend
> implementation or authorize shipping screens. This observation does not rewrite the
> Decision and does **not** retire §8's planning-only CI assertion. Inventory may
> allow `-ui` members and lockfile Leptos. Absence of a mounted shell remains
> green: `scripts/console/route-inventory.test.mjs` still asserts HEAD tracks
> neither React route source, and that absence-as-green assertion is retired only
> by the change that mounts a shell. The first full-depth vertical is payroll
> execution; a machine-readable Leptos route-facts file remains a frontend-lane
> follow-up.

## Context

**The stack has never had a decision record.** `Leptos` returns zero hits
across `docs/decisions/` (verified 2026-07-30). It exists only in
`docs/PIVOT-2026-07-28.md` §3 (`:34-48`), which fixes Leptos 0.9-beta with SSR
and islands (`:38`), `leptos_axum` against the existing axum 0.8.9 (`:41`), and
isolation behind a contracts crate outside default workspace members (`:43`).
`docs/PIVOT-2026-07-28.md` is not in `docs/decisions/`, so under
`docs/decisions/README.md:1-2` and `:4` it binds nothing. A stack the whole
repository is being rebuilt on is currently governed by a file with no
authority. This record is where that becomes authoritative.

**ADR-0025's structural clauses name machinery that is absent from HEAD.**
`web/` does not exist in the working tree. The two console route sources whose
shapes the route inventory parses are
`web/src/console/shell/nav.ts` and `web/src/console/screens/registry.ts`
(`scripts/console/route-inventory.mjs:4-5`) — the same
`web/src/console/**` tree ADR-0025's `## Decision` designates as the target application
root. `scripts/console/route-inventory.test.mjs:36` asserts that HEAD tracks
neither file, and CI runs that assertion at `.github/workflows/ci.yml:137`. So
the repository's own executable gate proves the absence that ADR-0025's
Decision text still presumes.

`docs/decisions/README.md:12` classifies exactly this as a **governance gap,
not silent supersession**, reconcilable only through a new decision. A
reciprocal frontmatter key alone would not close it, because ADR-0025's
Decision sentences would remain standing and false in an authoritative record.

**One correction to the source charter, recorded because the code wins.**
`docs/ideas/d4-frontend-charter.md:24` and `:59` attribute the nine-item
evidence bar to ADR-0025 **§7** at `:125-145`. §7 is *Converge and delete the
legacy visual system* (`:205`). The nine-item bar is **§4, *Ship only complete
vertical slices*** (`:127-152`), with the enumerated items at `:133-141`. The
retained clause is identified here by its correct section and lines.

## Decision

### 1. Leptos is the console stack

The console frontend is a Leptos application composed of workspace crates. It
integrates with the existing axum surface through `leptos_axum`. Leptos 0.9 is
beta at the date of this record; §7 below gates implementation, so beta status
is a planning input rather than a live production risk, and the fallback
position is a follow-up rather than an assumption.

### 2. SSR shell with island editors — for authorization, not performance

The shell, navigation, and every read projection render server-side. Client
hydration is confined to islands that need local interaction state: editors,
composers, and direct-manipulation surfaces.

**The reason is the authorization model, not page speed.** DN-0003 invariant 5
(`docs/decisions/notes/DN-0003-adr-0025-operational-object-runtime.md:84-86`)
requires that denied data be omitted **including counts and relationship
existence**, and invariant 2 (`:76-78`) requires that clients render
projections rather than decide eligibility. Progressive disclosure in this
product therefore derives from the fold: what a principal may not see is never
composed. Under client-side rendering the shipped bundle would have to carry
the route table and the capability set for the client to decide what to
reveal, which makes navigation itself a disclosure channel and puts the count
of denied objects in the client's hands. Server rendering keeps the fold on
the server: markup for a denied surface is never generated, so there is
nothing to omit late. Any measured performance benefit is incidental.

### 3. What is withdrawn from ADR-0025

Three structural prescriptions cease to bind. Each is withdrawn because it names
machinery absent from HEAD, not because it has merely aged:

1. **The carbon-copy visual authority** — ADR-0025's "one carbon-copy
   visual system" end state and `:70-76`'s prohibition on inheriting target
   visuals from `web/src/components/shell/**`, `web/src/components/ui/**`, the
   legacy `AppShell`, shadcn styling, or legacy Tailwind composition. A
   prohibition against inheriting from a deleted tree constrains nothing, and
   an end state defined as a copy of a deleted surface is unreachable.
2. **The `web/src/console/**` path and the two-shell composition** —
   ADR-0025's `## Decision` and its boundary table. A Leptos application is a workspace crate, so
   surface ownership must be stated in crate terms. The *intent* — one owner
   per surface, no forking of shared machinery — is retained and restated in
   §5 and §6.
3. **The spine boundary as enumerated** — ADR-0025's shared-versus-console boundary table. Its rows "Generated
   OpenAPI types and the single typed client/cache", "frontend
   policy-decision adapters", and "Internationalization corpus and string
   gates" are stack-specific and describe machinery that no longer exists. The
   *principle* — the console owns its surface and reuses the platform rather
   than forking auth, contracts, policy, audit, or telemetry — is retained.

Nothing else in ADR-0025 is touched. Its §3 product semantics (`/overview`,
Work Hub, My Work), §5 workflow/policy/data authority under ADR-0018 and
ADR-0021, and §6 rollout discipline remain accepted and unamended. DN-0003
remains a subordinate design note parented to ADR-0025.

### 4. What is retained, and why the retention makes the withdrawal safe

**ADR-0025 §4's nine-item evidence bar (`:127-152`, items at `:133-141`)
survives in full** and governs every future console screen: a reachable
mounted body for every exposed navigation state; real backend reads and
mutations through the shared contract; source-object drill-through and
canonical human-safe identifiers; server-side authorization plus
fail-closed/deny-by-omission client behavior; required audit events and
atomicity for sensitive decisions; loading, empty, denied, stale,
partial-failure, and full-failure behavior; persona-based real-backend E2E
coverage; fidelity, accessibility, performance, and console-error gates; and
explicit legacy-parity coverage or an owner-approved deferral. Incomplete
navigation entries stay hidden or classified DARK and are never counted as
product breadth (`:144-147`).

This is why the amendment is narrow. Everything withdrawn in §3
prescribes *how* the console is built. The bar constrains *what counts as
built*, and it is the strongest anti-stub defence in the record. Retaining it
is what makes withdrawing the rest safe.

Item 2's "shared typed contract" and item 8's "fidelity" gate are read against
whatever contract and design authority the accepted records then name; the
obligation does not lapse because its instrument changed.

### 5. Repository structure: no stack split

The repository is organized by domain, then by layer within the domain, so one
vertical slice is one directory rather than two trees kept in step. There is no
`frontend/` or `backend/` top-level division.

ADR-0001:20 already mandates a domain-first crate family,
`console-<domain>-{domain,application,adapter-postgres,rest,worker}`, and says
nothing about a path prefix; `backend/crates/ontology/{domain,application,
adapter-postgres,rest}` is that family on disk today. Crate names are
therefore unchanged by this convention — only the path prefix would be
dropped.

**Moving the existing tree is outside this record's scope.** Dropping the
`backend/` prefix touches the workspace member globs, the generated `BUCK`
files (`tools/buck/gen_first_party.py` discovers members by filesystem walk,
so a stale regeneration is a silent-divergence hazard rather than a build
error), the `.github/workflows/ci.yml` path filters, the `sqlx` migration
path, and every `file:line` citation in every document in the repository. This
record charters the **convention**, so that new crates land correctly. The
move of the existing tree requires its own decision, its own verification, and
its own owner approval.

### 6. The `ui` crate layer — a flagged dependency, not decided here

The console's per-domain surface crate is `console-<domain>-ui`, and its legal
dependency edges are:

> A `console-<domain>-ui` crate may depend on the shared platform contracts
> crate and on other `ui` crates. It may not depend on that domain's `domain`,
> `application`, `adapter-postgres`, or `rest` crates.

That rule turns the sequencing constraint in §7 into a compiler error rather
than a policy someone must remember: a `ui` crate cannot compile against a
domain whose contracts do not yet exist.

**This edge rule extends the crate family that ADR-0001:20 enumerates, so it
depends on a separate accepted amendment to ADR-0001.** ADR-0001 enforces
dependency direction twice — by crate visibility and by the CI layer-boundary
gate run at `.github/workflows/ci.yml:425` (`cargo run -p
console-gate-layer-boundary`) — and a new layer needs its legal edges declared
in both. ADR-0001 is a separate amendment target with its own record; this
record does not amend two ADRs. Until that amendment is accepted, §6 is a
declared dependency and no `ui` crate may be added.

The shared contracts crate itself, and the change from a hand-maintained to a
generated `backend/openapi/openapi.yaml`, are the ADR-0009 record's material
and are referenced here as dependencies rather than decided.

### 7. The gate: frontend implementation begins when the ontology engine
substrate is proven

Implementation is gated on the **ontology engine substrate**, not on the
HR/payroll/org verticals, and "proven" means the following conditions measured
green — never that a plan was approved:

| Condition | Status (measured 2026-07-30 unless dated otherwise) | Evidence |
|---|---|---|
| An authored type's rows are readable through an audited policy-attachment path | **MET** | `backend/crates/platform/db/migrations/0205_ont_policy_api_attach_writer.sql` lands the audited attach path in a `SECURITY DEFINER` routine owned by a `NOLOGIN`/`NOBYPASSRLS` role, leaving `console_rt` without `INSERT` on the policy catalog |
| An authored relationship produces edges | **MET, with a rule** | X1 measured that a link type alone writes zero edges; no reachable path writes one without a property `config.link`. The rule is a planning input, not a defect |
| The shared contracts crate exists and `backend/openapi/openapi.yaml` is generated from it | **MET (2026-08-11)** | `console-contracts` composes the published document from 35 Fragments (1 shared + 34 REST faces) via `console-openapi-gen` (`backend/crates/contracts/src/bin/console_openapi_gen.rs`); `EXPECTED_FRAGMENTS = 35` refuses an empty/partial registry (examined-zero fails). Faces keep path/schema YAML under `rest/openapi/` and contribute through `include_str!` modules. CI Backend runs `cargo run … --bin console-openapi-gen` then `git diff --exit-code -- backend/openapi/openapi.yaml` (`.github/workflows/ci.yml`). App still serves the generated bytes via `include_str!("../../openapi/openapi.yaml")` at `backend/app/src/lib.rs:221`. Measured: `cargo test -p console-contracts --test compose` 35/35; `cargo test -p console-todos-rest --test openapi_fragment` 8/8; generator idempotent on the wave-4 admit tip |
| Actions on a projected type do not require a hand-written Rust closure per action | **MET (2026-08-09)** | **The 2026-07-30 citation was stale in three ways, and is corrected here rather than quietly replaced.** (a) Every anchor moved: the type is now `backend/crates/ontology/rest/src/lib.rs:209-212`, the REST state's empty construction `:129`, the fail-closed `NotWiredYet` `:263-277`. (b) "Constructed empty" stopped being true: the App tier overrode it with `.with_projected_dispatch(...)` and registered exactly ONE target, `registry.update_equipment` — which is not a member of `DispatchTarget::ALL`, so CANONICAL coverage was 0 of 13 and the defect was the mechanism, not emptiness. (c) The roster is THIRTEEN targets, not fourteen (`thirteen_dispatch_targets_verbatim`, `backend/crates/ontology/canonical-domain/src/canonical_contract.rs`); a design sized for fourteen would have been sized against a number that does not exist. **What closes it:** dispatch is DERIVED from the contract instead of listed. `ProjectedDispatchRegistry::dispatch` (`:263`) parses the wire string with `DispatchTarget::from_str` and keys on `DispatchTarget::object()`, so the registry's key is the `ObjectKey` — locked at six by `six_projected_stable_object_keys_verbatim` — and `register_port` (`:238`) is generic over `CanonicalPort`, taking a port and no closure. One generic `canonical_port_handler` (`:295`) serves every target a port owns: it injects the CONTRACT's target string into the payload, decodes the port's `Query` (internally tagged on that string), re-checks the decoded value with `CanonicalQuery::dispatch_target` (`canonical-domain/src/lib.rs:536`), runs the port's PURE `preflight`, and hands over through `spawn_blocking`. The composition root (`backend/app/src/lib.rs:2584-2601`) wires all thirteen targets in SIX lines, one per object; a fourteenth target added to `dispatch_targets!` resolves with no edit to the registry, the handler, or the composition root. Fail-closed is asserted, not assumed: a non-roster string and a roster member whose object has no port BOTH still return `ActionError::NotWiredYet`. The engine still writes no domain table — the port owns its transaction, RLS arming and audit (§9.3). Measured by the six tests in `backend/crates/ontology/rest/src/projected_dispatch_derivation.rs` (totality over `DispatchTarget::ALL`, end-to-end routing, both fail-closed properties, the decoded-target mismatch refusal, the idempotency-key requirement) and by `projected_dispatch_coverage::the_wired_registry_resolves_every_canonical_dispatch_target` (`backend/app/src/lib.rs:2610`), which asserts the DEPLOYED registry over the roster. RED baseline, with the six `register_port` lines removed: `the derivation is not total: ["company.revise", …, "payroll.decide_run"] resolve to no port` (13 of 13 unresolved) and `hr.transfer resolves through the Employment port: NotWiredYet { target: Some("hr.transfer") }`. `console-gate-writer-ownership` still exits 0, over 254 production source files against 253 at `05a06aa9c` — the moved count is what proves it observed the change |
| A read model exists for aggregate queries | **MET (2026-08-11)** | **Not a MATERIALIZED VIEW.** Per-subject counts cannot share one MV body with `residual::lower`. `PgInstanceStore::aggregate_instances` (`backend/crates/ontology/adapter-postgres/src/instances.rs:582`) reuses the list path's residual lowering (`LoweringTarget::Instance`) and emits `SELECT <allowlisted group key>, COUNT(*) … GROUP BY 1` under `with_org_conn`. Group keys are allowlisted (`lifecycle_state`, `object_type_id`, or `attributes->>` with `is_safe_ident` + bound key). REST `GET /api/v1/ontology/instances/aggregate` (`INSTANCES_AGGREGATE_PATH` at `backend/crates/ontology/rest/src/lib.rs:439`) is classified Gated. Measured by `aggregate_instances_subject_counts_match_list_and_deny_all_as_runtime_role` in `instances_residual_filter_as_runtime_role` (subject sum equals filtered list length; empty policies → deny-all → zero counts) |
| A real dry run exists | **MET (2026-08-09)** | The 2026-07-30 citation was already stale when written: `apply_edits` moved onto the shared `PreparedCommand::prepare` both entry points call, so preflight had resolved the edits since `57ef1710b`. The false green was one validator further on — the edits' RESULT is judged by `validate_attributes` (`backend/crates/ontology/adapter-postgres/src/instances.rs`), which ran only inside the writeback, so a numeric value for a `choice` property still preflighted as `would_execute: true` and then 422'd on execute. `preflight_action` now runs the SAME `stage_revision_in_tx` / `create_instance_in_tx` the execute path runs, inside `with_org_rollback` — a transaction it always rolls back (`backend/crates/platform/db/src/audit_tx.rs`). Measured by `preflight_refuses_an_edit_set_the_writeback_refuses` (RED before the change, reporting `would_execute: true`) plus `preflight_writes_zero_rows_and_never_spends_the_approval`, which compare a per-table content digest before and after a preflight over a rejected AND an accepted edit set, with an executed command as the positive control |

**All six §7 conditions are MET as of 2026-08-11 (wave-4 admit: console-b4z
generated OpenAPI + console-09c policy-scoped aggregate path; projected
dispatch and dry-run were already MET).** The Critic's B6 read-model defect
from `docs/ideas/ecosystem-plan-review.md` is closed by the live
`aggregate_instances` residual path, not by a MATERIALIZED VIEW. The
programme may still defer mounting a console shell (`console-8nq`) as a
delivery HOLD; that is not a §7 substrate gap. Gating on measured engine
conditions rather than vertical completeness remains the rule.

### 8. What is authorized now, and what is forbidden

**Authorized:** the rebuild's existence; Leptos as the stack; design and
planning work of any depth, including the north-star console shape,
information architecture, and the surface inventory.

**§7 substrate bar:** measured green (2026-08-11). Backend ontology/OpenAPI
substrate work that this charter gated is no longer blocked by §7.

**Programme HOLD on shell mount:** `console-8nq` remains deferred — no mounted
shell, route, component, or navigation entry until that bead is deliberately
opened. ADR-0025 §4's anti-stub rule (`:146-147`) still applies to any future
slice.

**The CI assertion that no console route source exists therefore stays until
8nq.** `scripts/console/route-inventory.test.mjs:36`, run at
`.github/workflows/ci.yml:137`, remains the planning-only enforcement; it may
be retired only in the change that opens the shell, with a justified commit.

## Decision drivers

- **A stack decision with no accepted-ADR home is unenforceable.** Zero
  `Leptos` hits in `docs/decisions/` means no gate, review, or plan can cite
  authority for the choice the entire rebuild depends on.
- **An authoritative record that presumes deleted machinery propagates false
  facts.** ADR-0025's `web/src/console/**` clauses are contradicted by CI
  (`scripts/console/route-inventory.test.mjs:36`). Leaving them standing
  invites agents and reviewers to derive plans from them.
- **The anti-stub bar must not be lost in the pivot.** ADR-0025 §4 is the only
  accepted clause that defines what "shipped console screen" means. Its
  survival is the precondition for withdrawing anything else.
- **Deny-by-omission is a rendering-topology constraint.** DN-0003 invariant 5
  (`:84-86`) forbids leaking counts and relationship existence, which
  constrains where composition happens, not merely what an endpoint returns.
- **Sequencing enforced by the compiler beats sequencing enforced by
  memory.** ADR-0001's twice-enforced dependency direction is the existing
  mechanism; a `ui` layer with declared edges reuses it.
- **README:12 requires a decision, not a patch.** Code that has diverged from
  an accepted ADR is a governance gap reconcilable only by a new record.

## Alternatives considered

### Keep ADR-0025 whole and rebuild inside its enumerated boundary

Rejected. It is not achievable: the boundary it enumerates is a deleted React
tree, and CI asserts its absence. Retaining the clauses would either block the
rebuild or be quietly ignored, and quiet non-compliance with an accepted ADR
is the failure mode README:12 exists to prevent.

### Supersede ADR-0025 in full

Rejected. Supersession would retire ADR-0025 §4's nine-item evidence bar,
ADR-0025 §3's `/overview` and Work Hub semantics, and ADR-0025 §6's rollout
discipline along with the three stack-bound prescriptions. That would trade a
narrow structural correction for the loss of the strongest anti-stub control
in the decision set, and would orphan DN-0003's parent. Amendment keeps the
retained clauses in their original home, where existing citations still
resolve.

### Leave Leptos in `docs/PIVOT-2026-07-28.md` and cite the pivot document

Rejected. README:1-2 and `:4` make non-`docs/decisions/` material
non-binding, and `:10` denies plan and prototype material any power over an
accepted ADR. Citing the pivot for stack authority would manufacture the
appearance of a decision that does not exist.

### Client-side rendering with a JSON API

Rejected on authorization grounds, as §2 states. It would require the client
to hold the route table and the capability set, making navigation a disclosure
channel against DN-0003 invariant 5's prohibition on leaking counts and
relationship existence.

### Open the frontend gate on the verticals, or on plan approval

Rejected. Gating on approval is precisely the failure the Critic's ITERATE
verdict warns about; gating on the verticals would let the console be built
against an engine whose projected-action dispatch is an empty registry
(`backend/crates/ontology/rest/src/lib.rs:169-173`) and whose preflight cannot
detect an invalid edit (`:1238-1257`).

### Authorize a thin shell now to de-risk the stack choice

Rejected. A mounted empty shell is exactly what ADR-0025 §4 refuses to count
as a capability (`:129-131`), and it would require retiring the CI assertion
at `scripts/console/route-inventory.test.mjs:36` outside the change that opens
the gate.

## Why the chosen option

Leptos with SSR and islands is the only option in front of the owner that
satisfies deny-by-omission structurally rather than by discipline, and the
narrow amendment is the only one that removes the false structural clauses
without also removing the bar that keeps the rebuild honest. Charter-plus-gate
is chosen over charter-alone because a stack decision with no stated
precondition would authorize implementation against a substrate with four
measured gaps.

## Consequences

- The stack acquires enforceable authority: a plan, gate, or review can cite
  a local accepted decision for Leptos instead of a non-binding pivot note.
- ADR-0025 stops asserting a `web/src/console/**` boundary that CI proves
  absent, closing a governance gap of the class README:12 names.
- The nine-item evidence bar governs the rebuild from its first screen, so
  the pivot does not reset the anti-stub floor to zero.
- Deny-by-omission gains a topological guarantee: a denied surface is not
  composed, so there is no client-side omission step to get wrong.
- The gate makes "backend first" auditable. Four named conditions with
  file-level evidence replace a judgement call about readiness.
- − The console's replacement shape is now undefined rather than prescribed.
  ADR-0025 supplied a target; this record supplies a bar and a stack and
  defers the shape to planning. Until that planning lands there is no visual
  or structural authority for the console surface at all.
- − Two dependencies are now on the critical path before any `ui` crate can
  exist: an accepted ADR-0001 amendment for the `ui` layer edges, and the
  contracts crate owned by the ADR-0009 record. Neither is decided here, and
  §6 is inert until the first lands.
- − The i18n corpus and string gates were enumerated in the withdrawn spine
  boundary (ADR-0025's boundary-table row `Generated OpenAPI types and the single typed client/cache`) and are not replaced. Korean is the product's primary
  language and `scripts/check-i18n.mjs` is absent from HEAD, so the repository
  has no string gate meanwhile. This is a known, recorded gap.
- − Charter-without-implementation means the stack choice stays unvalidated by
  running code for as long as the gate is closed, and Leptos 0.9 remains beta
  in the interim.
- − The domain-first convention will coexist with the current `backend/`-
  prefixed tree until a separate decision moves it, so the repository carries
  two path conventions during that period.

## Follow-ups

1. Propose the ADR-0001 amendment adding `ui` to the enumerated crate family
   and declaring its legal edges, including the layer-boundary gate rule at
   `.github/workflows/ci.yml:425`. §6 stays inert until it is accepted.
2. Close the four open §7 conditions, each with re-runnable evidence: the
   contracts crate and generated `openapi.yaml`; projected-action dispatch
   without a per-action Rust closure; a read model for aggregate queries; and
   a preflight that exercises `apply_edits`.
3. Record an explicit fallback position for Leptos 0.9 not reaching stable
   before the gate opens.
4. Record the absent string gate as a tracked gap and design its replacement
   in the planning work; Korean-first product copy remains an ADR-0023
   obligation.
5. Do the north-star console shape and information-architecture design that
   this charter authorizes, under the ADR-0025 §4 bar.
6. Charter the move of the existing `backend/` tree as its own decision,
   designing its verification around `tools/buck/gen_first_party.py`
   discovering members by filesystem walk.
7. Resolve, in the planning work, the instruments ADR-0025 §4 items 7 and 8
   require for a Leptos surface — persona-based real-backend E2E and the
   console-error gate — since the React instruments are gone.

## Reciprocal record landed on acceptance

`docs/decisions/README.md:9` requires amendment to be explicit in **both**
records, and `:26` requires relationship keys to be reciprocal where
applicable. All three of the following landed atomically with acceptance:

1. **Frontmatter key on the amended record.**
   `docs/decisions/ADR-0025-carbon-copy-console-shared-platform-spine.md`
   gained `amended_by: [ADR-0030]`. Before this change ADR-0025's
   frontmatter (`:1-10`) carried `amends: [ADR-0023]` and
   `related: [ADR-0009, ADR-0018, ADR-0021, ADR-0022, ADR-0023]` and **no
   `amended_by` key**, so this **created** the key rather than appending to
   it. `ADR-0030` was also added to ADR-0025's `related`. Note the gate's
   ordering constraint: `scripts/check-adrs.mjs:411-419` requires an
   `amended_by` target to be `accepted`, so ADR-0025 could not gain the key
   before this record's status changed in the same commit.

2. **Index row changes in `docs/decisions/README.md`.** ADR-0025's row
   changed status cell from `accepted` to `accepted, amended` and its
   scope cell gains: "structural prescriptions — carbon-copy visual
   authority, the `web/src/console/**` path and two-shell composition, and the
   spine boundary as enumerated — amended by ADR-0030; its §4 nine-item slice
   bar and §3 product semantics remain in force". This record's own row
   changed status from `proposed` to `accepted`. A bullet was added to the
   *Effective relationship graph* section stating that the ADR-0025 clauses
   amended by ADR-0030 are its stack-bound structural prescriptions only —
   the carbon-copy visual authority, the `web/src/console/**` path and
   two-shell composition, and the spine boundary as enumerated — and that
   ADR-0025 remains
   accepted for the nine-item slice bar, `/overview` and Work Hub semantics,
   workflow/policy authority, and rollout discipline.

3. **Sentence edits in ADR-0025's Decision text, because a reciprocal key
   alone would leave false sentences standing.** Each edit below was made in
   place, marking the clause as amended rather than deleting the history:

   - `:51-53` — "The target authenticated console is the in-repository
     application rooted at `web/src/console/` and mounted at `/console/*`."
     This names a path CI asserts is absent
     (`scripts/console/route-inventory.test.mjs:36`). It was edited to
     record that the path and mount are amended by ADR-0030 and that the
     console is a Leptos workspace crate family.
   - `:70-76` — the prohibition on inheriting visuals from
     `web/src/components/shell/**`, `web/src/components/ui/**`, `AppShell`,
     and shadcn styling. Every named source is deleted from HEAD. It must be
     edited to record the withdrawal, retaining only the non-forking rule
     ("must not be copied into a console-private client, auth system, policy
     engine, or backend contract").
   - `:77-82` — the `ConsoleShell`/`AppShell` naming-ambiguity clause, whose
     three named files no longer exist. It was edited to record that the
     two-shell composition is amended by ADR-0030.
   - `:59-68` — the spine-boundary table. It was edited to record that
     the enumeration is amended by ADR-0030 and that the underlying principle
     (the console owns its surface and reuses the platform rather than forking
     it) is retained.
   - `:233-234` — "The target end state is one carbon-copy visual system on
     one shared platform spine, not two maintained frontend products." The
     carbon-copy half is unreachable; the one-spine half survives. It must be
     edited so the surviving half stands alone.

   ADR-0025 §4 (`:127-152`) is edited only to note that it survives the
   amendment. §3, §5, and §6 are not edited.

**Not owed.** `ADR-0001` gains nothing from this record; the `ui` layer edge
rule in §6 is a declared dependency on a separate ADR-0001 amendment, and this
record does not amend two ADRs. `ADR-0009` and `ADR-0012` gain nothing here;
the contracts-crate and generated-`openapi.yaml` reciprocity belongs to the
ADR-0009 record. `DN-0003` needs no edit: its `parent_adr: ADR-0025` remains
valid because ADR-0025 remains accepted, and the clauses withdrawn in §3 are
not the clauses DN-0003 elaborates.
