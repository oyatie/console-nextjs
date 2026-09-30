# Ledger — PostgreSQL reachability shard S0 (2026-08-05)

## Identity

- Slice: S0 harness `--shard-id` + pure partition module (no workflow graph change)
- Design: `docs/ideas/pg-shard-mvp.md`

## Decision

1. Add `tools/ci/postgres-shard.mjs` package to facet rules (`app|platform|ontology|domain`).
2. `cargo_needs_postgres.sh --shard-id` filters the workflow map.
3. Map checker enforces partition completeness and disjointness.
4. Does **not** change `ci.yml` or demote the load-bearing PG check name.

## Non-goals

- S1 facet jobs / aggregator (follow-up)
- num-threads greater than 1

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
    "Essentialism / YAGNI": "S0 only: filter flag plus unit tests; no required-context graph change.",
    "Chesterton's Fence": "Keeps full --workflow-only path and load-bearing job invocation string unchanged.",
    "Pragmatism": "Enables measured parallel facets without branch-protection rename.",
    "Red Team": "Default harness path unchanged; --shard-id is opt-in filter only.",
    "Operability / Day-2": "Operators still run full suite without new flags.",
    "Blast-radius / cell-based": "tools/ci only until S1 lands.",
    "Telemetry-first": "Harness logs shard id and entry count.",
    "Zero-trust / defense-in-depth": "Does not weaken Required CI admission."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Partition of 183 workflow targets: app=51 platform=35 ontology=19 domain=78."
  ],
  "decisions_changed_or_rejected": [],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
