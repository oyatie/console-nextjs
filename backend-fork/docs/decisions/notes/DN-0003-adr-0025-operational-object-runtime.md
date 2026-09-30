---
id: DN-0003
kind: design-note
parent_adr: ADR-0025
authority: subordinate
activation: in_progress
date: 2026-07-23
owner: jasonlee
---

> **Post-pivot reconciliation (2026-08-03):** Current product authority is [`docs/current/PRODUCT.md`](../../current/PRODUCT.md) (+ ROADMAP/DELIVERY). Pivot history: [`docs/PIVOT-2026-07-28.md`](../../PIVOT-2026-07-28.md) (non-authority). This document is subordinate and must not authorize work outside Ontology/Foundry/Policy → Company/OrgUnit/Employee → HR/Payroll. Conflicting ERP, finance, communications, compliance-product, ingest/evidence, office-editing, AI-judgment, frontend, or Buck2 execution claims are historical context or **HOLD** until explicitly reconciled by a current candidate.

# DN-0003 — Operational Object Runtime for the console

## Status

IN PROGRESS implementation guidance under ADR-0025. This note does not expose a
screen, authorize a rollout, or claim Palantir parity. It records the
architecture being implemented and the evidence required before any capability
can move from DARK development inventory into production exposure.

The game-engine analogy in this note is architectural, not visual. The console
must behave as a programmable operational substrate rather than a collection of
fixed SaaS CRUD pages. It must not adopt game styling, a generic entity-component
database, runtime JavaScript plugins, or a second source of truth beside the
existing domain and ontology stores.

## Source patterns

The adopted patterns are derived from primary Palantir documentation and adapted
to this repository:

- [Ontology overview](https://www.palantir.com/docs/foundry/ontology/overview):
  semantic objects, properties, and links combined with kinetic Actions,
  Functions, and dynamic security.
- [Action types](https://www.palantir.com/docs/foundry/action-types/overview):
  governed transactional mutations, validation, side effects, and writeback.
- [Action log](https://www.palantir.com/docs/foundry/action-types/action-log):
  inspectable decision and mutation provenance.
- [Functions](https://www.palantir.com/docs/foundry/functions/overview):
  server-side traversal and decision/derivation logic.
- [Automate](https://www.palantir.com/docs/foundry/automate):
  event-driven functions, notifications, and Action submission.
- [Scenarios](https://www.palantir.com/docs/foundry/workshop/scenarios-concepts):
  immutable what-if forks with explicit limits.
- [Data Lineage](https://www.palantir.com/docs/foundry/data-lineage/overview)
  and [Data Health](https://www.palantir.com/docs/foundry/observability/data-health):
  operational provenance and health as product surfaces.

These sources are references, not repository authority. ADR-0025, the local
ontology/workflow/security contracts, and executable evidence remain
authoritative.

## Operational runtime model

| Engine concept | Console runtime contract |
|---|---|
| World | One tenant-scoped operational graph |
| Entity | Stable `EntityRef` containing tenant, object type, and object ID |
| Components | Versioned capability descriptors such as inspectable, temporal, actionable, stateful, evidence-bearing, automatable, and simulatable |
| Systems | Existing domain use cases and workflow nodes |
| Command buffer | Authorized ontology or domain Action |
| Frame output | Immutable command receipt, audit event, and outbox reference |
| Scene/workspace | Saved query, projection, variables, and tool layout |
| Simulation | Immutable base snapshot plus proposed commands; never a live mutation |

Capabilities are interfaces attached to governed object types. They are not rows
in a universal component table. Projected business objects keep their
domain-owned writer and invariants; generic ontology instances keep their
revision-owned writer. The runtime composes those authorities rather than
replacing them.

## Core invariants

1. **Every consequential mutation is an Action.** Direct object-property edits
   are not the normal operational write path.
2. **Decision logic runs server-side.** Clients may collect intent and render
   projections; they do not decide eligibility, authority, or business state.
3. **Commands are deterministic.** Instance commands carry a command ID,
   canonical payload digest, and expected revision. Identical replay returns the
   original receipt; digest mismatch returns `409`; stale revision returns
   `412` before approval consumption.
4. **The write transaction is complete.** Mutation, version/history,
   authorization recheck, approval consumption, audit, and receipt either commit
   together or do not commit.
5. **Security exists at every boundary.** RLS/PBAC protects query, aggregate,
   object, property, relationship, and Action surfaces. Denied data is omitted,
   including counts and relationship existence.
6. **The operating loop is visible.** A user can move from linked object and
   evidence, through server-derived decision support and preflight, to an
   authorized Action, receipt, resulting state, lineage, and health.
7. **Workspaces store projections, not business copies.** Saved state contains
   query, filters, columns, projection, focus, depth, and layout. Authorization
   is re-evaluated on restore.
8. **Object focus is stable across tools.** Graph, list, table, timeline, object
   card, search, workflow, and communications resolve the same entity identity.
9. **Scenarios are immutable.** Promotion is a separate live Action that reruns
   authorization, preflight, expected-revision checks, and health gates.
10. **Extensibility is bounded.** Tenant definitions are declarative; trusted
    first-party tools are compile-time allowlisted; external connectors are
    server-side, typed, scoped, audited, idempotent, and outbox-backed.

## Interaction contract

- The left navigation represents domain lenses and saved views over one
  operational graph, not unrelated CRUD silos.
- The center changes projection—graph, list, table, timeline, or specialized
  allowlisted tool—without losing focused-object identity.
- Selecting an object opens exactly one contextual right rail:
  - **요약**: semantic fields and direct typed relations;
  - **흐름**: lifecycle, revisions, evidence, and provenance;
  - **작용**: policies, automation, analytics, health, and derived state;
  - **작업**: currently permitted Actions.
- Preflight precedes execute. Denied commands are omitted or rendered as a
  truthful non-executable decision state; they never call execute.
- Relationship graphs have an equivalent keyboard-readable list. Focus,
  history, browser deep links, and assistive-technology semantics remain stable.
- The communication rail yields to the object contextual rail when an
  object-focused tool is active. Two competing right rails are not allowed.

## Initial implementation slices

### Slice 1 — Inspect, preflight, act, verify

The API-backed object explorer preserves the real object-type UUID, renders the
governed object card, runs server preflight, executes only allowed Actions, and
refreshes descriptor, traversal, acting state, lifecycle, and history from
server readback. Projected objects remain inspect-only until their real domain
handler is registered. Legacy client-local node creation is not reachable from
the mounted screen.

Acceptance evidence:

- real object/type/relationship/history responses populate the rail;
- denied preflight produces no execute request;
- allowed execute uses exact object-type and instance IDs;
- readback shows the committed server revision and receipt;
- an authority/API switch synchronously fences stale state;
- exactly one complementary rail is present; and
- keyboard, responsive, and accessibility stories pass.

### Slice 2 — Deterministic instance command

Ontology instance Actions gain command identity, payload digest, expected
revision, and an immutable typed receipt. The first real consumer is the saved
`console_view` Action so two editors cannot silently overwrite from the same
revision.

Acceptance evidence:

- editor A saves revision 1 to revision 2;
- editor B, also based on revision 1, receives `412`;
- no revision 3, second audit, or approval consumption occurs for B;
- replaying A with the same command and digest returns the same receipt;
- reusing A's command ID with a different digest returns `409`;
- a foreign tenant learns neither object nor command existence; and
- the client retains its unsaved draft after a conflict.

### Slice 3 — Runtime manifest and server-owned projections

Object-type detail becomes a capability-oriented runtime manifest. Semantic
property kind is separated from renderer/tool choice. Bounded `QuerySpec` and
`ProjectionSpec` contracts move filtering, pagination, snapshot/watermark,
lineage, traversal limits, and PBAC/RLS compilation server-side.

This slice remains DARK until Slices 1 and 2 are integrated and executable.

### Slice 4 — Operational events and governed scenarios

Committed commands emit ordered entity invalidation/reference events. Immutable
Decision, Intervention, and Scenario objects may then model what-if work and
governed promotion through the same command kernel.

This slice must not relabel transient preflight as a persisted simulation.

## Domain-module rule

People, Production, Logistics, IFM, Equipment 3R, Consulting, and Owned Plants
remain bounded contexts with real domain writers. Each module contributes typed
objects, relationships, Actions, events, policies, and specialized tools to the
shared runtime. A module screen is a projection and workflow tool over those
objects, not a separate application or data island.

New module work must therefore deliver:

1. real tenant/RLS-scoped persistence;
2. typed domain lifecycle and legal transitions;
3. deterministic Action/replay/conflict behavior;
4. PBAC and assignment/segregation-of-duties enforcement;
5. immutable provenance, audit, and evidence links;
6. OpenAPI and generated-client parity;
7. a mounted DARK frontend body using the shared object/action grammar; and
8. an executable end-to-end user story before production exposure.

## Rejected approaches

- A generic ECS/component store that becomes a second writer.
- Event-sourcing every existing domain before incremental delivery.
- Runtime JavaScript or server-provided executable navigation.
- A generic meta-UI that removes specialized operator workflows.
- Direct ungoverned object edits as the standard mutation path.
- Mutable shared scenarios or scenario promotion that bypasses live checks.
- Client-side business rules, fabricated lineage, fake health, or mock
  operational records in a product-reachable build.
- Treating a mounted screen, screenshot, or design prototype checklist as
  completion evidence.

## Activation gates

No Operational Object Runtime claim is release-ready until:

1. Buck2 builds and executable database tests pass for the exact integrated SHA;
2. generated clients match OpenAPI for TypeScript, Kotlin, and Swift;
3. real dev-auth browser stories prove inspect, deny, execute, replay, conflict,
   readback, audit, and tenant isolation;
4. visual and accessibility comparison passes at required viewports;
5. no product-reachable stub, placeholder, fake record, dead control, or
   client-local mutation path remains;
6. fresh review finds no unresolved material issue; and
7. production exposure remains behind ADR-0025 rollout evidence and readback.
