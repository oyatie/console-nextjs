---
id: ADR-0025
status: accepted
doc_status: published
date: 2026-07-13
owner: jasonlee
decision: isolated-carbon-copy-console-with-shared-platform-spine
amends: [ADR-0023]
amended_by: [ADR-0030]
related: [ADR-0009, ADR-0018, ADR-0021, ADR-0022, ADR-0023, ADR-0030]
---

# ADR-0025: Carbon-copy Console with an isolated visual system and shared platform spine

## Status

**Accepted 2026-07-13.** After a direct comparison of the isolated carbon-copy
application and ADR-0023's shared-chrome strangler, the owner selected **Option
A: the carbon-copy application**. This ADR amends ADR-0023 only where its
shared-chrome/two-shell composition and non-feature-flag coexistence conflict
with Option A. ADR-0023's unamended product, workflow, policy, audit, mobile,
and fully-wired delivery decisions remain accepted.

## Context

ADR-0023 established the Oyatie Console design as authenticated-console
authority and chose a strangler inside `web/`: shared tokens and chrome across a
new `ConsoleShell` and the legacy `AppShell`, followed by route-by-route
migration. That decision preserved working routes and reused the existing web
stack, but its shared visual layer conflicts with the later carbon-copy mandate.
The visual system being replaced cannot also constrain the target if literal
prototype fidelity is a product acceptance gate.

The 2026-07-09 carbon-copy directive instead called for a visually isolated
application under `web/src/console/` that owns the `/console` viewport while
reusing the expensive operational spine: backend contracts, typed client,
authentication, authorization, audit, realtime, internationalization,
telemetry, and real-backend E2E infrastructure. Current implementation already
contains that boundary, so leaving ADR-0023's surface clauses unamended would
preserve a known governance contradiction.

Ratifying the boundary does **not** ratify current completeness. At the
2026-07-13 baseline, the carbon-copy navigation declares 36 screen keys but only
11 screen bodies are mounted. Twenty-five chrome-only entries remain incomplete,
and several mounted workflows still have backend, policy, atomicity, or persona
proof gaps. Directional alignment is not readiness evidence.

## Decision

### 1. Isolate the visual application; share the platform spine

**Amended by ADR-0030: the `web/src/console/` root and the `/console/*` mount are
withdrawn.** `web/` is absent from HEAD, and CI asserts that neither console route
source is tracked (`scripts/console/route-inventory.test.mjs:36`, run at
`.github/workflows/ci.yml:137`). The target authenticated console is a Leptos
workspace crate family (ADR-0030 §1, §5, §6), and its implementation is gated by
ADR-0030 §7. What survives here unamended: the console owns the whole application
viewport and its visual behavior, and it remains part of the existing repository,
build, router, deployment, and backend; this is not a separate package, repository,
service, or product.

**The boundary as enumerated below is amended by ADR-0030.** Its rows "Generated
OpenAPI types and the single typed client/cache", "frontend policy-decision
adapters", and "Internationalization corpus and string gates" name machinery that
no longer exists, and the table is retained for history rather than as a live
enumeration. The principle it expressed is retained and binding: the console owns
its surface and reuses the platform rather than forking auth, contracts, policy,
audit, or telemetry.

The boundary as originally enumerated was:

| Shared platform spine | Carbon-copy visual ownership |
|---|---|
| Auth/session, org and scope context | Console shell, navigation, topbar, rail, dock, and viewport |
| Generated OpenAPI types and the single typed client/cache | Scoped tokens, typography, spacing, elevation, and motion |
| Backend policy enforcement and frontend policy-decision adapters | Window, pin, tray, composer, object-card, lifecycle, and module grammar |
| Audit APIs and sensitive-view/action audit helpers | Screen composition and every user-visible state |
| Realtime/event transport | Console-specific accessible interaction implementations |
| Internationalization corpus and string gates | Fidelity baselines and component visual-regression states |
| Rollout flags, route telemetry, RUM, and error reporting | Prototype-to-build screen captures |
| Real-backend persona/E2E harness | Carbon-copy screen implementations |

**Amended by ADR-0030: the inheritance prohibition is withdrawn.** It named
`web/src/components/shell/**`, `web/src/components/ui/**`, the legacy `AppShell`,
shadcn styling, and legacy Tailwind utility composition — every one of which is
deleted from HEAD, so the prohibition constrains nothing. The non-forking rule
survives and is the operative sentence: reusable nonvisual behavior may be imported
or extracted into shell-neutral modules; it must not be copied into a
console-private client, auth system, policy engine, or backend contract.

**Amended by ADR-0030: the two-shell naming-ambiguity clause is withdrawn.** It
disambiguated `web/src/console/shell/ConsoleShell.tsx` from
`web/src/components/shell/ConsoleShell.tsx` and `AppShell`; none of those three
files exists at HEAD, and the Leptos crate family has no two-shell composition to
disambiguate.

### 2. Make the design mirror a fidelity acceptance gate

`docs/design/oyatie-console/` is the visual and interaction authority for the
carbon-copy surface. The reusable grammar is implemented once—tokens, shell,
window/pin/tray engine, token composer, generic module template, lifecycle
surface, object card, list/table behavior, and shared action/error states—and
screens compose those primitives rather than redrawing them.

Every material divergence from the design authority must be classified as one
of:

1. an accessibility or standards correction;
2. compliance with a stronger rule already stated by the design charter; or
3. a named positive polish improvement with an observable user benefit.

Convenience drift and unclassified visual differences fail the fidelity gate.
Where the mirrored prototype contains a target state, compare reference and
build captures at the same viewport and state. Where it does not, use the
current design grammar/change log plus committed build-side state captures; the
absence of a prototype frame never waives build-side visual proof.

### 3. Preserve the unamended product semantics from ADR-0023

The target console opens on the **Overview** screen. `/overview` remains the
canonical authenticated landing URL and stable compatibility contract.
`/console/*` is the isolated carbon-copy application and rollout boundary; its
default internal screen is `overview`. During coexistence, `/overview` may keep
legacy cohorts on the working legacy surface and route eligible cohorts into the
carbon-copy Overview only through the rollout decision. At final cutover,
`/overview` must resolve to the carbon-copy Overview rather than becoming an
abandoned competing home route.

**Work Hub** remains the behavioral contract for the role-aware action inbox:
what needs attention, what is blocked, which source object owns the work, and
where its conversation, mail, evidence, approval, task action, and audit trail
live. **My Work** is the complete personal queue inside that model, not a
competing home surface.

The ontology-first object grammar, window/pin workspace, Korean-first product
copy, deny-by-omission rendering, and audit-everywhere direction remain in
force. Mobile employee-app parity remains outside this web-console ADR and is
governed by ADR-0009.

### 4. Ship only complete vertical slices

**Survives the ADR-0030 amendment in full**, and governs every future console
screen on the Leptos stack. Item 2's "shared typed contract" and item 8's
"fidelity" gate are read against whatever contract and design authority the
accepted records then name; the obligation does not lapse because its instrument
changed.

A navigation label, component gallery, screenshot, stub, fixture-only screen,
or mounted empty shell is not a completed capability. A console screen may be
counted as shipped only when all applicable evidence exists:

1. a reachable, mounted body for every exposed navigation state;
2. real backend reads and mutations through the shared typed contract;
3. source-object drill-through and canonical human-safe identifiers;
4. server-side authorization, plus fail-closed/deny-by-omission client behavior;
5. required audit events and atomicity for sensitive decisions;
6. loading, empty, denied, stale, partial-failure, and full-failure behavior;
7. persona-based real-backend E2E coverage;
8. fidelity, accessibility, performance, and console-error gates; and
9. explicit legacy-parity coverage or an owner-approved deferral.

Incomplete navigation entries must be hidden or clearly classified as DARK;
they must not be presented or counted as working product breadth. When the
backend cannot realize the design, the slice includes the backend work or waits
behind a named backend charter. Fabricated operational, legal, policy, audit,
custody, or workflow state is forbidden.

Stub-first work is permitted only inside tests or development harnesses that
are unreachable in production. It is not an acceptable merged,
product-reachable phase, and wiring-last historical plans are subordinate to
this full-stack slice rule.

### 5. Keep one workflow, policy, and data authority

Approvals continue to use the corporate workflow engine from ADR-0018. The
generalized runtime/task REST, arbitrary approval-line roles, and pre-terminal
finalization/receipt model carried by ADR-0023 remain adopted: terminal runs are
not reopened, and post-approval reversal is represented by a compensating
document/event.

Workspace state may be local to the carbon-copy window/panel engine; server data
continues through the shared typed client/cache. Client-side composition is
never the authorization boundary. ADR-0021 continues to govern Cedar/PBAC
promotion: current legacy enforcement and Cedar shadow evidence remain until a
separate accepted activation decision passes its gates.

### 6. Use a staged, measurable, reversible rollout

The legacy application remains functional while the carbon-copy console is
completed. Cutover extends the existing limited rollout substrate; current
organization eligibility, per-user opt-in, and kill-switch controls do not by
themselves prove cohort or percentage ramping. The required target is:

1. server-controlled organization eligibility;
2. per-user opt-in with immediate return to legacy;
3. an operator kill switch;
4. added server-owned cohort and percentage ramping;
5. added assignment-bound route adoption, workflow success, error, latency, and Core Web Vitals
   telemetry; and
6. a tested rollback path before each expansion.

The server-owned effective rollout decision—including organization eligibility,
user choice, and kill-switch state—must be the routing authority. A hostname or
client redirect that sends traffic directly to `/console` without consuming
that decision does not satisfy staged rollout and must be repaired before any
cohort-ramp claim. The kill switch fails closed to the working legacy surface.

Do not make the carbon-copy surface the unconditional destination merely because
its shell renders. Expand a cohort only when its required workflows pass the
vertical-slice gates and its error/performance budgets. Legacy routes receive
correctness, security, and continuity fixes during coexistence, but new product
investment belongs in the carbon-copy surface unless an explicit exception is
required to keep production safe.

Internal `state.screen` navigation may reproduce the prototype workspace model,
but it must not eliminate browser history, canonical `/overview` behavior, or
stable source-object deep links. Current `main` resolves that boundary to the
frontend `web/src/lib/objectRegistry.ts` as the sole browser destination
authority; the backend owns canonical kind/id/code and authorization and must
not return `url_path`. Console-private kind/prefix tables must converge into the
same shell-neutral contract; duplicated destination registries are not an
accepted end state.

### 7. Converge and delete the legacy visual system

The two visual worlds are a migration state, not a permanent architecture.
Legacy deletion requires:

- every required legacy workflow replaced or explicitly owner-descoped;
- all hard-blocking items in the legacy-parity register resolved;
- policy, audit, passkey/step-up, period-lock, and source-object E2E proof where
  applicable;
- successful rollback rehearsal and production error/performance budgets;
- a complete workflow/persona/cadence inventory with every descope explicitly
  accepted by the owner, predeclared eligible-request and active-use denominators,
  and recurrence-aware evidence: at least two distinct production revisions and
  fourteen complete days are the minimum technical-stability floor, while daily,
  weekly, monthly, period-close, quarterly, annual, and event-driven workflows
  must each be observed through a representative recurrence or pass a predeclared
  production-like rehearsal when waiting for the natural event would be unsafe;
- zero required legacy-route traffic at at least 99.9% reconciled classification
  coverage for the deletion-eligible cohort, with lost/unknown events counted as
  legacy rather than silently dropped;
- a two-stage decommission: first disable legacy routing while retaining a signed,
  restorable legacy image/config/data-compatibility packet; then remove code only
  after a timed restoration drill and the recurrence-aware observation gate pass;
  retain the last verified restoration packet for at least 90 days after removal;
  and
- removal of redundant shell, visual primitive, route, and compatibility code
  in the same convergence program.

**Amended by ADR-0030: the carbon-copy half of this end state is withdrawn** — an
end state defined as a copy of a deleted surface is unreachable. The surviving half
stands alone: the target end state is one shared platform spine, not two maintained
frontend products.

## Alternatives considered

### ADR-0023 shared-chrome two-shell composition

This option maximizes reuse and is the lower-cost path to one evolving design
system. Its surface composition was rejected because shared chrome and shared
visual primitives make the legacy visual system a constraint on the surface
that must be a literal carbon copy. ADR-0023's unamended clauses remain current.

### Separate console package or repository

This would enforce visual isolation but would fracture the single typed-client
regeneration discipline, auth/session ownership, build/deploy path, and E2E
harness. It was rejected because the required visual boundary can be enforced
inside the existing application.

### Token-only refresh

This would reduce implementation cost but omit the window, object, lifecycle,
composer, and interaction grammar that defines the design. It was rejected as
insufficient to satisfy the product mandate.

## Consequences

- The repository temporarily carries two visual worlds and a broader test
  matrix; rollout and deletion evidence are mandatory controls for that cost.
- Carbon-copy primitives may reimplement accessible interactions that the
  legacy UI already has. Their accessibility, responsive behavior, and
  performance must be proven rather than assumed.
- Shared auth, contracts, policy, audit, telemetry, and E2E infrastructure must
  not fork merely to simplify visual isolation.
- Prototype fidelity and product completeness are independent gates. A faithful
  empty shell fails completeness; a fully wired legacy-looking screen fails the
  target fidelity gate.
- The immediate planning problem is to complete, hide, or deliberately defer
  the current chrome-only entries and close backend/security gaps—not to create
  another shell or continue unconstrained polish.
- The carbon-copy execution details live in
  `.omc/plans/carbon-copy-charter.md`, subordinate to this ADR and to fresh
  implementation/security evidence.
