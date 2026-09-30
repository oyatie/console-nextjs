# Ledger — work_order request_no seed local CHECK gate (2026-08-05)

## Identity

- Slice: pure local admission for `work_orders.request_no` fixture seeds
- Design: `docs/ideas/ci-agent-throughput-process.md` (shift-left fixtures)

## Decision

1. Add `tools/ci/work-order-request-no-seed.mjs` encoding CHECK
   `^[0-9]{8}-[0-9]{3}$` from migration `0008_create_work_orders.sql`.
2. Unit tests reject `EVD-{uuid}` and bare suffixes; require high uniqueness
   from UUID-derived 8+3 seeds.
3. Wire into `npm run check:ci-preflight` (same path as postgres-shard unit tests).
4. Does **not** demote Required PG; prevents multi-hour hosted discovery of seed bugs.

## Non-goals

- Changing production `request_no` generation
- Replacing disposable-PostgreSQL required proof

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
    "Essentialism / YAGNI": "Pure node CHECK gate only; no Docker and no ci.yml change.",
    "Chesterton's Fence": "Keeps Required PG display name and full hosted proof; local gate is additive.",
    "Pragmatism": "Catches 23514/23505-class fixture bugs before hosted multi-hour PG wall.",
    "Red Team": "Does not weaken admission; rejects known-bad seed patterns agents have reintroduced.",
    "Operability / Day-2": "Wired into npm run check:ci-preflight agents already run before push.",
    "Blast-radius / cell-based": "tools/ci and package.json only; no domain runtime or migrations.",
    "Telemetry-first": "Failures surface as local unit test names with migration regex pin.",
    "Zero-trust / defense-in-depth": "Hosted disposable PG remains required; local gate is extra fail-closed."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Hosted #579 hit 23514 on EVD-{uuid} after 23505 unique collision; local gate encodes both lessons."
  ],
  "decisions_changed_or_rejected": [],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
