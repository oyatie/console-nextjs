---
id: ADR-0005
status: accepted
doc_status: published
date: 2026-06-12
owner: jasonlee
consensus: ralplan iteration 3 (Planner/Architect/Critic APPROVE, 2026-06-12)
amended_by: [ADR-0024]
related: [ADR-0010, ADR-0024]
---

# ADR-0005 — SeaweedFS primary + independent WORM replica (RustFS rejected at launch; re-evaluate at GA)

## Status

Accepted (consensus-approved plan §2.6), amended by ADR-0024 on 2026-07-13.

## Context

Maintenance photo/video evidence is legally meaningful and must be tamper-proof (WORM) and durable beyond a single machine. The user initially decided RustFS; adversarial verification (2026-06-11) found: pre-GA beta, disk-full metadata corruption (rustfs discussion #2737, unanswered), SSE-plaintext and Object-Lock misreporting bugs in recent betas, CVE-2025-68926 (CVSS 9.8 hardcoded credential), and the only rigorous third-party evaluation concluding "not production-ready". MinIO community is archived (no CVE fixes); Garage lacks Object Lock; Ceph needs 3+ nodes.

## Decision

SeaweedFS remains the self-hosted primary behind the current S3-specific storage implementation (`console-platform-storage`), hardened: Filer UI/Admin GUI not exposed, releases pinned behind head, and our own WORM retention test suite in CI (put-retention COMPLIANCE → version-delete attempt must fail).

ADR-0024 generalizes the replica rule: every evidence object must reach a retention-locked copy in an independent failure domain through a provider-neutral object-storage capability contract. The current S3 adapter serves SeaweedFS, OCI Object Storage, and other proven S3-compatible endpoints; it is not itself the provider-neutral contract. Native GCS, Azure Blob, and other non-S3 services require separate context adapters that preserve the same object, integrity, retention, and recovery behavior without leaking provider types into the core. The first `on-prem` self-host reference must provide the SeaweedFS implementation and an independent physical site or equivalent failure domain. No managed object store is mandatory across contexts.

The completion interlock remains unchanged: a WorkOrder cannot reach FINAL_COMPLETED with unverified AFTER/REPORT evidence. Any storage-engine replacement must pass the same integrity, retention, recovery, and security-policy gates before activation.

## Consequences

+ Evidence integrity survives a primary-node/site loss; context-native object stores remain available behind one capability seam.
− Every production context still operates and proves an independent WORM/retention copy; replication failures require the alert/queue path.

## Alternatives considered

RustFS now (rejected on evidence above, user approved fallback); MinIO (archived); Garage (no Object Lock); Ceph/RGW (ops-disproportionate).
