# Ledger — PostgreSQL reachability S2 domain facet split (2026-08-05)

## Identity

- Slice: S2 split `domain` → `domain-a` / `domain-b` (balanced package bags)
- Depends on: S0 shard module, S1 facets + aggregator
- Design: `docs/ideas/pg-shard-mvp.md` v1.1

## Decision

1. Replace single `Test PostgreSQL — domain adapters` with A/B halves.
2. Greedy package assignment by workflow entry count (39/39 on current map).
3. Aggregator display name unchanged; needs all five facets.
4. Preflight digests + `verify.mjs` co-change.

## Non-goals

- Demoting required PG proof
- Matrix display name on load-bearing check
- Backend job split (second wave after domain max drops)

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
    "Essentialism / YAGNI": "Split only the long domain bag into two facets; no product scope.",
    "Chesterton's Fence": "Keeps exact load-bearing aggregator display name and Required / CI edge.",
    "Pragmatism": "Greedy balance by package entry count (39/39) to cut max(facet) wall.",
    "Red Team": "Aggregator still if always and needs all five facets; no path skip.",
    "Operability / Day-2": "Facet names A/B group in Checks UI; failure isolates half of domain.",
    "Blast-radius / cell-based": "ci.yml + shard module + preflight digests; no domain runtime code.",
    "Telemetry-first": "Wall becomes max(domain-a, domain-b, other facets) measurable with run IDs.",
    "Zero-trust / defense-in-depth": "Does not demote required PG; still every PR admission."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "S1 wall rid 31027986820 max facet domain adapters 28.4m; domain bag was 78 of 183 entries."
  ],
  "decisions_changed_or_rejected": [],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
