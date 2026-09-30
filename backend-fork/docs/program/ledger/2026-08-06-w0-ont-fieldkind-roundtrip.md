# Ledger: W0-ONT FieldKind roundtrip + draft discard tests

**Date:** 2026-08-06  
**Bead:** console-g1n  
**Allowlist:** backend/crates/ontology/**

## Outcome

Domain unit tests: all known `FieldKind` tags parse/echo/`is_known`; draft may
archive without activation. No HOLD clear; no migrations; pure domain only.

## Verification

Also rewrote `docs/program/executed-tests-baseline.json` test_attribute_baseline for the +2 domain tests.


```
cargo test -p console-ontology-domain --lib
# 10 passed
cargo test -p console-ontology-application --lib
# 6 passed
```

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
    "Essentialism / YAGNI": "Two unit tests only; no schema or API change.",
    "Chesterton's Fence": "Unknown tag degrade path retained; known tags fully covered.",
    "Pragmatism": "Measurable AC: 10 domain unit tests green.",
    "Red Team": "No alternate write path; no HOLD clear.",
    "Operability / Day-2": "Tests document FSM discard and FieldKind tags.",
    "Blast-radius / cell-based": "ontology/domain only.",
    "Telemetry-first": "cargo test counts in ledger.",
    "Zero-trust / defense-in-depth": "Fail-closed instance transitions still reject illegal jumps."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "FieldKind known tags lacked explicit roundtrip tests.",
    "Draft to archived discard path was not unit-tested."
  ],
  "decisions_changed_or_rejected": [],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
