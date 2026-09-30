---
id: ADR-0007
status: accepted
doc_status: published
date: 2026-06-12
owner: jasonlee
consensus: ralplan iteration 3 (Planner/Architect/Critic APPROVE, 2026-06-12)
related: [ADR-0002]
---

# ADR-0007 — Postgres-persisted messenger with LISTEN/NOTIFY multi-instance fan-out (not E2EE; audit-grade)

## Status
Accepted (consensus-approved plan §2.5).

## Context
The company runs on KakaoTalk today; the replacement must be a full messenger (user decision: WO threads + team channels + DM + groups + read receipts + search) with server-side auditability. True E2EE makes server-side audit impossible by design; business messaging products (Slack/Teams) use transit+at-rest encryption with audited server access.

## Decision
Messages persist to Postgres BEFORE any fan-out (the DB row is the source of truth; tokio broadcast channels are never authoritative). WebSocket delivery uses per-connection mpsc; cross-instance wake-up uses Postgres LISTEN/NOTIFY carrying IDs ONLY (8000-byte hard payload ceiling — subscribers re-read rows), so multi-instance scale-out is correct from day one. Every message emits an audit event (ADR-0002); media rides the evidence pipeline (presigned S3, ADR-0005). Explicitly NOT E2EE — documented to users.

## Consequences
+ Audit-grade history, search, and exact-once client recovery from the DB; horizontal scale path without Redis.
− Server can read messages (accepted and disclosed); NOTIFY adds one indirection hop.

## Alternatives considered
E2EE (rejected: incompatible with audit mandate); embed Mattermost (rejected: separate auth/audit/ops island, weak WO integration); Redis pub/sub (rejected: new stateful dependency duplicating what Postgres provides at this scale).
