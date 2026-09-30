---
id: ADR-0016
status: accepted
doc_status: published
date: 2026-06-12
owner: codex/t61
consensus: implements ADR-0010 under ADR-0001 layering
related: [ADR-0001, ADR-0010]
---

# ADR-0016 - oyatie AI assistant port contract

## Status
Accepted.

## Context
ADR-0010 requires an oyatie cloud intelligence integration seam now, but no
adapter, mock, stub, route, or UI affordance until the real oyatie service is
ready. The seam covers two work-order use cases: symptom plus equipment model
to procedure checklist, and work-order context to report draft.

ADR-0001 says ports live in application-layer crates. The current workspace has
`console-workorder-application`, but no inspection application crate yet.

## Decision
Define `AiAssistantPort` in `console-workorder-application`.

The port exposes:
- `diagnose(symptom, equipment_model) -> ProcedureChecklist`
- `draft_report(work_order_context) -> ReportDraft`

The contract types carry only work-order/equipment context and reviewable text.
They do not perform state transitions, do not call oyatie, and do not create a
feature surface. The trait is dyn-compatible so a future real adapter can be
wired at the composition root without changing the use-case contract.

## Alternatives Considered

### Platform intelligence crate
Rejected for now. A `crates/platform/intelligence` crate would be classified as
platform/adapter layer by the current layer gate, which would make it the wrong
dependency direction for an application-layer port.

### Kernel contract
Rejected. The assistant contract is not a universal primitive like IDs, audit
types, or trace context. Putting it in kernel would leak a work-order use case
into the innermost layer.

### Defer the port entirely
Rejected by ADR-0010. The contract should be compiler-checked before the real
adapter arrives.

## Consequences
+ The seam follows existing application-port precedent and keeps layer-gate
  direction intact.
+ No oyatie feature can appear accidentally because no adapter or route exists.
+ Ponytail cleanup issue #121 keeps `AiAssistantPort` in code as the explicit
  ADR-0010 exception: the compile-time contract is intentional, but adapter
  stubs, mock implementations, routes, and UI affordances remain forbidden until
  production oyatie adapter work is scheduled.
- If a dedicated `inspection` application crate lands later, this contract may
  need to move or be re-exported with a migration ADR.
