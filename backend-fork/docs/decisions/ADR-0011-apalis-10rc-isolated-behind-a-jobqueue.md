---
id: ADR-0011
status: accepted
doc_status: published
date: 2026-06-12
owner: jasonlee
consensus: ralplan iteration 3 (Planner/Architect/Critic APPROVE, 2026-06-12)
related: [ADR-0002]
---

# ADR-0011 — apalis 1.0-rc isolated behind a JobQueue trait (RC pin + soak gate + swap path)

## Status
Accepted (consensus-approved plan §2.10).

## Context
Escalation timers (P1 accept-window, Alimtalk fallback), retention purges, WORM replication retries, and report generation need a background job runner. apalis (Postgres backend — no Redis) is the ecosystem leader but sits at 1.0.0-rc; the production-grade-only mandate is in tension with RC dependencies, flagged by the consensus Critic.

## Decision
All job scheduling goes through our own `JobQueue` trait; apalis-postgres is one adapter behind it. The RC is admitted only after passing a timer-reliability soak test (T1.10: N escalation timers fire within tolerance under process restart, clock skew, and crash recovery) which is a hard M2 entry gate. The documented fallback (a `FOR UPDATE SKIP LOCKED` Postgres queue) is implementable behind the same trait without touching domains.

## Consequences
+ Emergency-dispatch timers rest on verified behavior, not version-number optimism; swap path is bounded.
− Trait indirection adds a small abstraction layer; soak suite is real work.

## Alternatives considered
Hand-rolled SKIP LOCKED queue first (kept as fallback); Redis-backed queues (new stateful dep); tokio timers in-process only (lost on restart — unacceptable for escalation).
