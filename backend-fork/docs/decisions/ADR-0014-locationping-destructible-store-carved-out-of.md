---
id: ADR-0014
status: accepted
doc_status: published
date: 2026-06-12
owner: jasonlee
consensus: ralplan iteration 3 (Planner/Architect/Critic APPROVE, 2026-06-12)
amended_by: [ADR-0029]
related: [ADR-0002, ADR-0006, ADR-0029]
---

# ADR-0014 — LocationPing destructible store carved out of the append-only audit store (위치정보법 destruction compatibility)

## Status
Accepted (consensus-approved plan §2.2).

## Context
위치정보법 Art. 15(1) requires individual consent for location collection (criminal penalties); Art. 24 grants non-refusable suspension and requires destruction of location data and collection records "without delay" on withdrawal. An append-only audit store is by design indestructible — auditing GPS coordinates would make lawful destruction impossible (latent compliance defect found by the consensus Critic). High-frequency pings would also bloat the audit table.

## Decision
`LocationPing` rows live in a separate, destructible, day-partitioned `location_pings` store with an enforced TTL/retention-purge job; withdrawal destroys all of a user's pings and collection logs (tested, T0.11). Coordinates NEVER enter `audit_events` (CI/lint assertion). Consent lifecycle events (grant/withdraw/suspend/resume) ARE audited — the regulator-relevant facts are when consent changed, not where the person was. The audit-coverage gate's exclusion set contains this path bound to its exact writer, alongside the expired-location-data retention purge recorded in ADR-0029, and a test asserts that set in full.

## Consequences
+ Destruction-on-withdrawal is physically realizable (drop partitions); audit invariant stays intact everywhere else.
− One deliberate, documented exception to "audit everything" — bounded and tested.

## Alternatives considered
Auditing pings with later redaction (rejected: append-only store cannot redact; legal risk); not collecting GPS (rejected by user decision ADR-0006 — GPS scoring chosen with full compliance workstream).
