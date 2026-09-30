---
id: ADR-0023
status: accepted
doc_status: published
date: 2026-07-04
owner: jasonlee
related_documents: [DESIGN.md]
amended_by: [ADR-0025, ADR-0026]
related: [ADR-0009, ADR-0018, ADR-0021, ADR-0025, ADR-0026, ADR-0036]
---

# ADR-0023: Oyatie console authority — accepted foundation; two-shell surface strategy superseded by ADR-0025

## Status

**Accepted; surface strategy amended by ADR-0025 on 2026-07-13, and the COSS RN
public-site follow-up amended by ADR-0026 on 2026-07-22.** This record was consensus-approved
2026-07-04 (Critic APPROVE after two Planner→Architect→Critic iterations) and
execution-approved by the user the same day. ADR-0025 replaces its
shared-chrome/two-shell composition and non-feature-flag coexistence clauses
with the isolated carbon-copy `/console` application and a staged, measurable
rollout. All other decisions in this ADR remain accepted, including the product
grammar, canonical `/overview`/Work Hub/My Work semantics, workflow-engine
direction, policy/audit requirements, and fully-wired/no-stub delivery rule.

Read the amended clauses through ADR-0025; the unamended clauses below remain
current authority. The repository-native
[Console development pipeline](../program/console-development-pipeline.md)
governs plan-to-deployment execution of those clauses; historical `.omc`
process references below are retained for traceability and are not current
dispatch or completion authority.

## Context

The console (`web/`) needed a single UI/UX authority instead of ad hoc per-screen design. The
Oyatie Console design (`docs/design/oyatie-console/` — prototype `Oyatie Console.dc.html`, charter
`DESIGN.md`, `TODO.md`, `HANDOFF.md`, `AGENTS.md`) supplies that authority: ontology-first typed
objects with reference chips and up/down-stream traversal, a window/pin workspace grammar, and
render-as-policy-decision (PBAC-gated screens/cards/rows/actions).

`web/` already has React 19 + Tailwind v4 + shadcn + a generated typed OpenAPI client + ~80 tests +
CI gates — discarding it to match the prototype 1:1 would burn months and violate the
increments-of-a-complete-system standard (see enterprise-production-standard). Backend readiness is
uneven: attendance/payroll/leave/HR/org/audit/policy/automation map to live APIs (some partial), but
generalized approvals need engine work first — the M2 workflow engine (PR #179) was, at the time of
this decision, single-template, authoring-only REST, with a terminal FSM that forbids edges out of
terminal nodes (verified `workflow_studio.rs:16-31`, `domain/src/lib.rs:230,462`). Inbox,
notifications, recruiting, review, benefits, and docs-archive have no server domain yet. The design
itself is a prototype with fictional "Acme" data; every screen must bind to real KNL-group org/data
and real Korean-law flows (§61 promotion, 주52h, 4대보험).

The plan is recorded at `.omc/plans/oyatie-console-plan.md`. This ADR transcribes its §8 decision
record.

## Decision

> **ADR-0025 amendment:** the shared-chrome/two-shell composition in the
> historical paragraph below is replaced by an isolated visual application at
> `web/src/console/**` and `/console/*`, with one shared nonvisual platform spine
> and an evidence-gated staged rollout. The `/overview`, Work Hub/My Work,
> product grammar, and fully-wired/no-stub clauses remain current.

Adopt the Oyatie Console design as the UI/UX authority for the authenticated console. The canonical
authenticated landing route is `/overview`; it supersedes the former `/work-hub` route while retaining
**Work Hub** as the behavioral contract for the role-aware action inbox. `mywork` is the personal queue
within that model, not a competing landing surface. Rebuild the shell and views to the Oyatie grammar
(ontology-first objects, window/pin workspace, Cedar-gated rendering, audit-everywhere) through the
ADR-0025 isolated visual application at `web/src/console/**` and `/console/*`, sharing one nonvisual
platform spine rather than chrome or shell ownership. Net-new domains (notifications, inbox, todos,
benefits, recruit, review, docs-archive) are built as full-stack vertical slices. No stubs or
placeholders — every shipped screen is fully wired to real backend APIs with tests; when the backend
can't realize the design intent, the slice builds the backend.

> **Historical 2026-07-04 composition — superseded by ADR-0025; not current
> authority:** the original decision used a strangler rebuild inside `web/` with
> two-shell coexistence, shared chrome, and a `ConsoleShell` beside the legacy
> `AppShell`. ADR-0025 replaced that composition and its rollout model.

Approvals standardize on the M2 workflow engine, but only after an explicit **Engine-Gen**
milestone generalizes it: runtime/instance REST (start-run, list-waiting-tasks, list-my-runs, claim,
decide, finalize), a generalized definition builder for arbitrary approval-line DAGs (dynamic 결재선,
검토/승인/합의/참조 roles, enum reasons, object-link targets), and a **pre-terminal finalization
model** — 최종승인 and 수령확인 are pre-terminal `WAITING` nodes (never a reopened terminal run) with
사후 반려 modeled as a compensating document/event. Engine-Gen opens with a 1–2 day spike validating
this FSM shape; if structurally infeasible, execution stops and returns to consensus with the
bespoke-thin-approvals alternative on the table.

### Drivers

1. `web/`'s existing React 19 + Tailwind v4 + shadcn + typed client + ~80 tests + CI gates are proven
   assets; discarding them would burn months and violate increments-of-a-complete-system.
2. Backend readiness is uneven — generalized approvals need engine work first (M2 engine was
   single-template, authoring-only REST, terminal FSM); other domains map to live APIs already,
   several net-new domains (inbox, notifications, recruit, review, benefit, docs-archive) do not
   exist at all.
3. The design is a prototype with fictional data; every screen must bind to real KNL-group org/data
   and real Korean-law flows.

### Alternatives Considered

> **ADR-0025 amendment:** the isolated in-repository carbon-copy application is
> now the chosen surface composition. The big-bang dark-period rewrite and
> separate-package approach remain rejected; Option B below records the original
> 2026-07-04 choice only.

**A. Big-bang rewrite** — a new app matching the prototype 1:1. Fastest visual fidelity, but discards
60 wired screens and tests, takes the console dark for months, and violates the
increments-of-a-complete-system rule. Rejected.

**B. Historical strangler inside `web/` with two-shell coexistence — chosen on
2026-07-04, then superseded by ADR-0025; not current authority.** It was judged
always shippable and reusable, with centrally shared chrome primitives and a
transient two-shell period. ADR-0025 replaced those shared visual primitives and
the two-shell composition with the isolated carbon-copy application.

**C. Token-only refresh** — restyle existing pages, skip the window/ontology grammar. Cheap, but the
grammar IS the design; skipping it fails the authority mandate. Rejected.

**D. Bespoke thin approvals model** instead of generalizing M2. De-risks four UI milestones if engine
generalization stalls, but creates a second approval store to later migrate and violates the
one-workflow-engine direction. Rejected in favor of Engine-Gen as an explicit prerequisite milestone
— retained as the documented fallback if the Engine-Gen spike finds the FSM change structurally
infeasible.

## Consequences

> **ADR-0025 amendment:** coexistence is now controlled by server-owned rollout
> eligibility, per-user opt-in/revert, a kill switch, cohort ramps, telemetry,
> and measured legacy deletion. It is not the non-feature-flag route migration
> described in the historical bullet below.

- Zustand enters the stack, scoped to the window/panel/workspace engine only; data fetching stays on
  the existing `openapi-fetch` wrapper + read cache.
- **Historical consequence, superseded by ADR-0025; not current authority:**
  `ConsoleShell` and `AppShell` coexist during migration as a two-shell period
  rather than a feature flag. Current rollout authority is the server-owned,
  evidence-gated model named above and defined by ADR-0025.
- New domains (notifications, inbox, todos, benefits, recruit, review, docs-archive) join as crates,
  each with its own migrations, RLS, and audit wiring.
- The workflow engine gains instance/task REST and a finalization/receipt semantics layer that other
  consumers (not just the console) can build on.
- Cedar/PBAC stays in shadow mode for every UI milestone in this program — the enforce flip is a
  separate charter; no UI milestone blocks on it. All ACs phrase authorization as "policy-gated
  (legacy enforce, Cedar shadow)".
- Engine-Gen is a hard dependency for UI-M3/M4/M6/M8/M9 (anything consuming instance REST); it must
  merge before those milestones start.

## Follow-ups (named out of scope for this program)

- Cedar enforce-flip charter, including covert clearance (CEO-designated 비밀인가, clearance role as
  covert resource, CEO-only audit stream — DESIGN §4.5 / HANDOFF §2 / design-TODO #13).
- Audit SSE stream (this program ships polling only).
- No-code policy/workflow visual canvas (DESIGN §4.6 makes it the baseline; this program ships
  read-only NL rows + simulation and defers the canvas).
- Contract C- → Position(인원편성) → PolicyPreset chain editor (DESIGN §3 head of the standard flow —
  design backlog, not in the prototype; enters as its own charter).
- Multi-jurisdiction PII program.
- Object graph explorer.
- Mobile-app parity for 메신저·메일·알림·전자결재 (DESIGN §4.8 — outside this `web/` program; the
  dual-native Swift/Kotlin contract in ADR-0009 governs employee apps). At this
  ADR's acceptance, `coss-rn` was the separate public COSS site and not an
  employee-app replacement; ADR-0026 retires that surface.
- This program's own open questions (UI-M12 internal ordering, comms-rail read/reply scope, 평가
  design depth, no-code canvas sign-off) are tracked in `.omc/plans/oyatie-console-plan.md` §7 and do
  not block UI-M0–M2b.
