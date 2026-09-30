---
id: ADR-0041
status: accepted
doc_status: review
date: 2026-08-25
owner: jasonlee
decision: layer-ui-accepted
amends: [ADR-0001, ADR-0030]
related: [ADR-0001, ADR-0025, ADR-0030, ADR-0031]
---

# ADR-0041 — Accept `Layer::Ui`

## Status

**Accepted 2026-08-25.** Amends ADR-0001 by adding `ui` to the enumerated crate
family and declaring its legal edges. Amends ADR-0030 by accepting `Layer::Ui`
so `-ui` members may exist. ADR-0030 §8's absence-as-green (no mounted shell /
no React route source) is **not** retired here. ADR-0030's Decision text is
retained as accepted history; this record is the additive layer legalization,
not a silent rewrite.

## Context

ADR-0030 §6 chartered `console-<domain>-ui` and named its legal edges, but left
those edges inert until a separate accepted amendment to ADR-0001. ADR-0030 §8
and the layer-boundary / route-inventory gates forbade any such member while
that amendment was missing. ADR-0030 §7's ontology-substrate conditions are
already measured green. The remaining block was the unaccepted layer, not the
engine.

The layer-boundary classifier currently treats `crates/platform/*` as
`Layer::Platform` before it reads the `-ui` suffix, so a future
`console-platform-ui` would silently inherit Platform privileges. Ui members
are skipped alongside Gate in layer-edge checks, so even a declared
`allowed_deps` would not be enforced. `PlanningOnlyUiCrate` fails closed on
existence. Those three facts are why a crate cannot land yet.

## Decision

1. **`Layer::Ui` is accepted.** A workspace member whose package name ends in
   `-ui` is `Layer::Ui`. `allowed_deps` is `[Contracts, Ui]`. `Layer::App`
   `allowed_deps` gains `Ui`. Ui may not depend on that domain's `domain`,
   `application`, `adapter-*`, `rest`, `worker`, `platform`, or `kernel`
   crates. Ui may not depend on `sqlx` (no database authority). `tokio` and
   `axum` remain allowed because SSR islands run on the server runtime; leaving
   `forbidden_external_deps` empty would fail-open `sqlx`. The compiler still
   refuses absent edges; the layer-boundary gate refuses illegal ones.

2. **Classify the `-ui` suffix before the `crates/platform/` path.** A future
   `console-platform-ui` is Ui, not Platform. Gate, App, and Kernel path
   classification stay ahead of the suffix (a gate crate is not a UI surface).

3. **Enforce Ui edges.** Do not skip `Layer::Ui` in layer-edge checks. Gate
   remains exempt. `PlanningOnlyUiCrate` is removed as a failure: a `-ui`
   member may exist. `SmuggledUiSurface` remains for non-ui crates. Skip the
   UI-needle scan for `Layer::Ui` members (their views are the chartered
   surface).

4. **Leptos pin.** Implementation uses Leptos **0.9.0-beta** from crates.io.
   **0.8.x** is the rollback if beta blocks production. This record does not
   add `leptos` as a workspace dependency; the frontend lane owns
   `backend/Cargo.lock` and the workspace pin.

5. **Rendering topology.** SSR plus selective WASM hydration (islands). The
   client has no business authority. Deny-by-omission is server composition:
   markup for a denied surface is never generated.

6. **Route inventory.** React tombstone paths stay absent. `-ui` members are
   allowed. Lockfile `leptos*` is allowed. **Absence of a mounted shell remains
   green** until the change that mounts one: the HEAD assertion that neither
   React route source is tracked is not retired here. Until the frontend lane
   lands a machine-readable route-facts file, inventory may report zero Leptos
   packages and must not fail HEAD for that absence. A **non-ui** crate that
   declares a Leptos-family dependency still fails. HEAD classification of
   members must not require `cargo` — docs-only preflight has no rustup.

7. **Buck.** First-party Buck generation skips members whose package name ends
   in `-ui` because Leptos is not vendored in `third-party/rust`. A skipped
   `-ui` dependency must be omitted from generated BUCK, not rewritten as
   `//third-party/rust:<name>`. This record does not invent `App → Ui` edges
   in generated faces.

8. **Vertical and exclusions.** Crate existence is legal. Frontend
   implementation is not resumed. Screens are not authorized until PRODUCT
   HOLD conditions — a mounted contracts-backed SSR shell and persona-based
   real-backend E2E evidence — are actually satisfied. The first full-depth
   vertical, when those conditions clear, is **payroll execution**.
   Import/export is not the data-entry base; 자료실 is a later exception and
   is not in this record. The comms rail is not in this slice. ADR-0025 §4's
   nine-item evidence bar still governs every screen.

## Consequences

- ADR-0001's crate family includes `ui`; ADR-0030 §6's edge rule is in force.
- A `console-<domain>-ui` crate can be added without `PLANNING_ONLY_UI_CRATE`.
- Smuggling HTML / `view!` / `leptos::` into `-rest` (or any non-ui crate)
  still fails.
- Lockfile Leptos is no longer a planning-only violation; introducing it
  remains a frontend-lane lockfile change.
- − No Ui crate, Leptos pin, SSR shell, or E2E evidence lands here. Frontend
  implementation remains HOLD until PRODUCT conditions are actually satisfied.
- − 0.9.0-beta remains beta; 0.8.x is the recorded rollback, not a second
  supported production pin.

## Alternatives considered

### Keep planning-only until a shell PR lands

Rejected. ADR-0030 sequenced the ADR-0001 amendment as the gate, not the first
mounted route. Leaving existence forbidden would force the shell PR to amend
two ADRs, the layer gate, and the inventory in one lockfile-touching change.

### Let Rest depend on Ui, or Ui depend on Platform

Rejected. ADR-0030 §6's compiler-enforced sequencing is the point: a ui crate
cannot compile against a domain whose contracts do not yet exist, and it must
not reach adapter/platform internals.

### Classify `-ui` after `crates/platform/`

Rejected. That is the silent Platform fallback this record exists to close.

## Why the chosen option

The smallest change that makes a Leptos crate *legal* without making one
*exist*. Edges, classification, and inventory fail-closed on the residual
classes (non-ui Leptos, smuggled markup) and fail-open only on the chartered
name.
