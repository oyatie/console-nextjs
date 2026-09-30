---
id: ADR-0015
status: accepted
doc_status: published
date: 2026-06-12
owner: jasonlee
consensus: ralplan iteration 3 (Planner/Architect/Critic APPROVE, 2026-06-12)
amended_by: [ADR-0024]
related: [ADR-0005, ADR-0024]
---

# ADR-0015 — DR posture: WAL archiving + continuous PITR (RPO ≤5min / RTO ≤1h) and VM-down emergency-dispatch fallback

## Status
Accepted (consensus-approved plan §ops), amended by ADR-0024 on 2026-07-13.

## Context
Launch runs on a single OCI VM (single fault domain) carrying criminal-liability compliance records and P1 emergency dispatch. Nightly-only backups (RPO ≈24h) were judged inadequate by consensus review.

## Decision
Postgres continuous WAL archiving with point-in-time recovery remains mandatory; targets RPO ≤5min and RTO ≤1h must be proven by restore to an arbitrary timestamp. Backup targets use the provider-neutral object-storage capability required by ADR-0005/ADR-0024. The current implementation is an S3-specific adapter, not proof of native GCS/Azure Blob portability; every context adapter must preserve the same archive, integrity, retention, and restore behavior.

The `oci-guest` context may remain single-node until provisioned capacity exists; node loss is restore/rebuild, not automatic failover. The accepted `on-prem-ha` target is the first self-host proof and requires multi-node/multi-failure-domain CNPG, replicated storage, an independent backup site, and recorded failover/restore drills before any HA claim. Oyatie Cloud, AWS, OCI, Azure, and GCP may later implement the same recovery/availability contract with context-native database, backup, replication, and failover adapters. Accepting the target does not claim that the DARK substrate is activated.

The VM-down degraded-mode contract remains: manual 유선 dispatch + Alimtalk while the system is unavailable, with each rehearsal recording time-to-first-contact and recovery evidence.

## Consequences
+ Data-loss targets, degraded operations, and restore proof remain invariant across deployment contexts; HA can be added without deleting the working OCI path.
− WAL/archive storage, independent copies, and drill cadence remain ongoing costs; no context may claim HA from manifests alone.

## Alternatives considered
Nightly dumps only (rejected: RPO 24h indefensible for this data); paid/native cloud HA before the self-host reference (deferred: it remains first-class but does not block the first portable proof).
