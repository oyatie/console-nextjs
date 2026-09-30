---
id: ADR-0006
status: accepted
doc_status: published
date: 2026-06-12
owner: jasonlee
consensus: ralplan iteration 3 (Planner/Architect/Critic APPROVE, 2026-06-12)
related: [ADR-0014]
---

# ADR-0006 — P1 broadcast-accept dispatch with live-GPS scoring (deliberate departure from dispatcher-mediated norm)

## Status
Accepted (consensus-approved plan §2.4).

## Context
Verified industry baseline (Salesforce/ServiceTitan/ServiceMax) is dispatcher-mediated assignment with a distinct emergency path; broadcast-accept is a gig-economy pattern. At a 3-technicians-per-branch scale, with managers sometimes unavailable, the user chose broadcast + live GPS (informed decision, interview R2).

## Decision
P1 등록 → branch/region-scoped broadcast push to technicians + managers (≤5s server-side); accept/decline with countdown; ≥2 accepts → auto-assign by score (live GPS distance × current-work priority weight); 0 accepts after timeout → manager force-assign alert with Alimtalk escalation and 유선 instruction. The accept-window FSM (BROADCASTING/AUTO_ASSIGNED/MANAGER_FORCE_PENDING) is a separate machine in the `dispatch` domain, orthogonal to the WorkOrder's 16-state FSM. Push is best-effort by design; the in-app ACK loop + timed escalation chain (push → Alimtalk → 유선) is the delivery guarantee. Live GPS use is conditional on the 위치정보법 compliance core (consent ledger, off-switch, destruction — ADR-0014, T0.11); technicians without consent fall back to schedule-based ranking.

## Consequences
+ Fastest response with thin management; honest legal posture.
− Deliberate departure from FSM-product precedent; escalation timers and consent machinery are launch-blocking complexity.

## Alternatives considered
Dispatcher-mediated only (rejected by user: manager bottleneck); broadcast without GPS (offered — user chose GPS with full compliance workstream).
