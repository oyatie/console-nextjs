# Ledger — CI latency baseline & packaging inventory (2026-08-05)

## Identity

- Slice: CI-0 / Phase 0 of CI latency reduction plan (no runtime behavior change)
- Base: `3a5dcc344d0f1f19e6b5c84d1a376c05f7375ee7` (`origin/main` post-#572)
- Machine inventory: `docs/program/ci-latency-inventory.json`

## Why this exists

Every pull request currently pays **~40–45 minutes** of required CI wall clock. Measurement shows the critical path is almost entirely **`postgres-domain-reachability`** (~2470–2590s), not documentation or script gates. This ledger freezes baseline numbers and a **non-authoritative tier split** of the 183 disposable-PostgreSQL Buck targets so later phases can re-package proofs without gutting them.

## Measured baselines (green or partial)

| Run | Label | Event | Approx wall clock | Critical path |
|-----|-------|-------|-------------------|---------------|
| 30991382251 | PR #572 S1 docs manifest | pull_request | ~45 min (2700s) | postgres 2588s |
| 30882384141 | main after #566 | push | ~43 min (2577s) | postgres 2472s |
| 30996682737 | PR #573 S2 | pull_request | in progress at write | backend complete 1213s; postgres still running |

### Backend step highlights (run 30996682737)

| Step | Seconds |
|------|---------|
| Free runner disk | 93 |
| clippy -D warnings | 187 |
| PR 473 migration operational gate | **454** |
| Boot smoke | 77 |
| Buck2 console-app unit suite | 154 |
| Buck2 console-app inline PostgreSQL | 67 |

Postgres job also spends ~80s on free-disk before the serialized matrix.

## Inventory counts (heuristic name tiers)

From `docs/program/ci-latency-inventory.json` against main's workflow:

| Tier | Count | Intended later use (not active yet) |
|------|------:|--------------------------------------|
| pg-core | 7 | Pre-merge when backend/db paths change |
| pg-domain | 33 | Pre-merge when domain paths change |
| pg-app | 50 | Pre-merge when app paths change |
| pg-full residual | 93 | Nightly / main full matrix |
| **total unique** | **183** | Today's single serialized job |

**Caveat:** tiers are by target **name** only. Phase 3a must re-validate any `pg-core` subset with planted reds before it becomes a merge gate. Misclassification is expected; the JSON is a planning artifact, not a proof.

## Required context contract (observed)

Branch protection required contexts (API readback, 2026-08-05):

1. `authenticate-console-authority`
2. `Required / CI`
3. `Required / Security`

`Required / CI` is an aggregator whose `needs` list currently includes ten jobs (preflight, domain-unit, postgres-domain-reachability, company-conformance, generated-face-authority, backend, dev-up-smoke, repo-gates, api-contract, kubernetes-manifests).

### Display-name coupling

Workflow comments state that the postgres job **display name** is load-bearing for historical protection matching. Phase 1 strategy (plan Option A): **do not rename** `Required / CI` / `Required / Security`; change **membership** of the aggregator later; avoid matrix display-name traps until aggregator-only protection is verified again at execution time.

## Packaging diagnosis (not a behavior change)

| Finding | Implication |
|---------|-------------|
| Critical path ≈ one serialized PG job | Sharding or moving full matrix off PR path is the only large lever |
| No path filters | Docs/scripts PRs pay full monorepo tax |
| PG-like work also in backend + dev-up + conformance | Overlapping proof classes; not always redundant, but cost stacks |
| Free-disk 80–93s × N jobs | Runner image tax, not product signal |
| PR 473 migration gate ~7.6 min on every backend job | Likely incident fossil; path-condition candidate |

## Non-goals of this ledger

- No workflow `if:` changes
- No target deletions
- No branch-protection edits
- No claim that pg-core is merge-safe yet

## Next admitted slices (plan)

1. CI-1 — required-context contract prose + protection verification receipt  
2. CI-2 — path-conditional heavy jobs (docs-only ≤5 min target)  
3. CI-3 — pg-core pre-merge + full matrix post-merge/nightly  
4. CI-4 — backend step diet (PR473 path filter, free-disk)

## Measurement method (repeatable)

```bash
gh run view <id> --json jobs --jq '.jobs[] | {name, conclusion,
  seconds: ((.completedAt|fromdateiso8601)-(.startedAt|fromdateiso8601))}'
```

Or the inventory's embedded baseline tables after refresh.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "release",
    "other"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Essentialism / YAGNI",
    "Chesterton's Fence",
    "Red Team",
    "Systems Thinking",
    "Operability / Day-2",
    "Opportunity Cost",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Treats wall-clock anecdotes as claims; freezes run IDs, per-job seconds, and critical-path identity before proposing cuts.",
    "Essentialism / YAGNI": "Phase 0 only inventories and records; no path filters or target moves until numbers and tiers exist.",
    "Chesterton's Fence": "Preserves the reason postgres is required and display-name coupling before any rename or shard.",
    "Red Team": "Records how missing path filters and cold Buck builds manufacture false urgency and queue saturation as attack surface on delivery.",
    "Systems Thinking": "Separates proof packaging, build tool cache portability, and path-conditional scope so one change cannot silently redefine another.",
    "Operability / Day-2": "Documents how to re-measure and which later phases own which packaging change.",
    "Opportunity Cost": "Identifies that optimizing clippy is noise next to a 43-minute postgres critical path.",
    "Blast-radius / cell-based": "Docs and inventory only; no CI runtime change; separate from in-flight domain work.",
    "Telemetry-first": "Machine-readable inventory JSON plus baseline table for p50/p90 comparisons later.",
    "Zero-trust / defense-in-depth": "Does not weaken required checks; inventory enables later fail-closed path filters rather than skip-on-error."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Merge latency is dominated by one serialized disposable-PostgreSQL job, not by documentation gates.",
    "183 unique Buck PG targets sit behind a single required aggregator membership.",
    "Display-name coupling and lack of path filters force full-suite cost onto every PR class."
  ],
  "decisions_changed_or_rejected": [
    "Rejected mixing CI redesign into S2 PR #573.",
    "Rejected deleting PG targets in Phase 0.",
    "Rejected treating name-heuristic tiers as merge-authoritative without planted reds."
  ],
  "lens_set_changes": [
    "Added Opportunity Cost because minutes spent on secondary optimizations do not move wall clock while postgres owns the critical path."
  ]
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
