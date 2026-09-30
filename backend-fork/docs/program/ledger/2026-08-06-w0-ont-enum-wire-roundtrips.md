# Ledger: W0-ONT enum wire/db roundtrips

**Date:** 2026-08-06  
**Bead:** console-ssf / console-g1n follow-on  
**Allowlist:** backend/crates/ontology/**

## Outcome

Domain unit tests for fail-closed `from_db_str` roundtrips on
`SchemaLifecycleState`, `BackingKind`, `LinkCardinality`, and `ActionDispatch`.
Unknown tags reject (no silent default). HOLDs not cleared; no migrations; pure
domain only.

## Verification

`docs/program/executed-tests-baseline.json` test_attribute_baseline for domain
lib: 10 → 14.

```
cargo test -p console-ontology-domain --lib
# 14 passed
cargo test -p console-ontology-application --lib
# 6 passed
```

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "standard",
  "risk_domains": [],
  "selected_lenses": [
    "Essentialism / YAGNI",
    "Chesterton's Fence",
    "Pragmatism",
    "Red Team",
    "Blast-radius / cell-based",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Essentialism / YAGNI": "Four pure unit tests only; no schema or API change.",
    "Chesterton's Fence": "Existing as_db_str/from_db_str tags retained; aliases rejected.",
    "Pragmatism": "Measurable AC: 14 domain unit tests green.",
    "Red Team": "Unknown tags fail closed; no alternate write path; no HOLD clear.",
    "Blast-radius / cell-based": "ontology/domain only (+ ledger/baseline).",
    "Zero-trust / defense-in-depth": "Fail-closed parse on wire/db tags."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "LinkCardinality/ActionDispatch/BackingKind/SchemaLifecycle lacked explicit roundtrip + unknown rejection tests after FieldKind coverage in #589."
  ],
  "decisions_changed_or_rejected": [],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
