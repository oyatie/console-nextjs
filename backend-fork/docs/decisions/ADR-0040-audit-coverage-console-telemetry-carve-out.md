---
id: ADR-0040
status: accepted
doc_status: review
date: 2026-08-10
owner: jasonlee
decision: audit-coverage-console-route-telemetry-carve-out
amends: [ADR-0029, ADR-0002]
related: [ADR-0002, ADR-0014, ADR-0029]
---

# ADR-0040 — Audit-coverage gains a bound console route-telemetry carve-out

## Status

**Accepted 2026-08-10.** Amends ADR-0029's closed-at-two cardinality so the
audit-coverage gate may carry a third bound exclusion when `backend/app/src` is
scanned as a handler surface (console-937 / gh#396). Authorises exactly one new
writer: `app/src/console_telemetry.rs::insert_route_telemetry`.

## Context

ADR-0029 closed the exclusion set at two LocationPing-related writers and said
no third exclusion may be added without a further accepted decision. Expanding
audit-coverage's handler-surface heuristic to the path component `app` (so
composition-root modules under `backend/app/src` are unmarked-scanned) surfaces
`insert_route_telemetry`: it `INSERT`s into `console_route_telemetry` under
`with_org_conn` without `with_audit`.

That write is cardinality-safe RUM / route-selection instrumentation. Putting
every event into append-only `audit_events` would flood the business audit trail
and reverse the point of a separate telemetry table. Leaving the writer
unscanned would re-open the one-file escape hatch ADR-0029 rejected.

## Decision

1. **`allowed_audit_exclusions()` may contain three bound entries.** The third is
   `console_route_telemetry_ingestion`, bound to
   `app/src/console_telemetry.rs` + `insert_route_telemetry`. The gate test pins
   length and every binding.
2. **Binding remains the control.** Cardinality is still only a weak proxy; a
   fourth writer still requires another accepted decision and a literal set edit.
3. **ADR-0029's LocationPing bindings are unchanged.** This record does not
   weaken path-binding, duplicate detection, or unknown-reason rejection.

## Consequences

- ADR-0002's exclusion-set sentence must say three bound entries (two location,
  one console telemetry).
- ADR-0029 remains accepted history for the two-entry reconciliation; it is
  amended here, not superseded.
- `docs/CI-GATES.md` and the `console-kernel-core` audit module doc must follow
  the gate, not the older count.
