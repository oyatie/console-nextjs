# Ledger — PostgreSQL reachability shard S1 (2026-08-05)

## Identity

- Slice: S1 facet jobs + load-bearing aggregator
- Depends on: S0 (`tools/ci/postgres-shard.mjs`, `--shard-id`)
- Design: `docs/ideas/pg-shard-mvp.md`

## Decision

1. Four facet jobs: `Test PostgreSQL — {app,platform,ontology,domain adapters}`.
2. Aggregator job id `postgres-domain-reachability` keeps display name
   `Dispatch, attendance and ontology — disposable PostgreSQL reachability`.
3. Preflight contracts updated for facets, digests, protected job set, rust-cache restore-only.
4. Required / CI still needs only the aggregator (same ten proofs).

## Non-goals

- Demoting required PG proof
- Matrix display name on the load-bearing check
- Path-filter skip of required context

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "other"
  ],
  "selected_lenses": [
    "Essentialism / YAGNI",
    "Chesterton's Fence",
    "Pragmatism",
    "Red Team",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Essentialism / YAGNI": "Four package facets plus aggregator; no path-filter product scope.",
    "Chesterton's Fence": "Preserves exact load-bearing display name and Required / CI edge.",
    "Pragmatism": "Parallelizes wall clock of 183 serial targets without demoting proof.",
    "Red Team": "Aggregator fails closed with if always; facets restore-only on rust-cache.",
    "Operability / Day-2": "Facet names group in Checks UI; failure isolates package area.",
    "Blast-radius / cell-based": "ci.yml and preflight contracts co-change; no domain code.",
    "Telemetry-first": "Wall becomes max(facet) measurable with run IDs after land.",
    "Zero-trust / defense-in-depth": "Does not weaken admission; still required on every PR."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "S1 preflight and 53 preflight unit tests pass locally before PR."
  ],
  "decisions_changed_or_rejected": [],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
