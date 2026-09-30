---
id: ADR-0003
status: accepted
doc_status: published
date: 2026-06-12
owner: jasonlee
consensus: ralplan iteration 3 (Planner/Architect/Critic APPROVE, 2026-06-12)
amended_by: [ADR-0028]
related: [ADR-0028, ADR-0032, ADR-0036]
---

# ADR-0003 — Branch-scoped authorization model (non-null branch scope day 1; default-deny)

## Status
Accepted (consensus-approved plan §2.3).

## Context
The organization is 300+ people across multiple 지점/지역 with a 수도권→충청→영남→호남 rollout. The prior project deferred branch scoping ("nullable, then mandatory") and its own docs flagged that as a must-fix-day-1.

## Decision
`Branch`/`Region` are first-class day-1 schema concepts. Principals carry a `BranchScope` (kernel type): `All` only where a built-in `Feature` capability or a live database membership proof derives it — the target being that it never derives from a role name — and an explicit branch set otherwise. **Interim, and lawful until the capability is seeded:** the role match at `Role::SuperAdmin | Role::Executive` in `authz/src/lib.rs` survives as ADR-0028 Decision 1's implementation of the capability source, and must not be extended to further role names. A reader must not treat that match as a live violation; ADR-0028 sanctions it explicitly and governs its removal. Composition with `org_id`, the single legal composition point, and the migration path for existing SUPER_ADMIN/EXECUTIVE principals are governed by ADR-0028. Repositories filter by scope by default (default-deny); cross-branch access is an authorization test fixture (T0.6). P1 broadcasts, KPI rollups, wall-boards, and team channels are branch-scoped.

## Consequences
+ No retrofit migration; rollout waves map to branch seeding.
− Every query carries scope; small constant complexity cost.

## Alternatives considered
Nullable-then-mandatory branch_id (rejected: known prior-project regret); separate DB per branch (rejected: cross-branch rollups + ops cost).
