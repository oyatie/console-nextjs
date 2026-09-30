# Ledger: W0-POL policy domain wire roundtrip tests

**Date:** 2026-08-06  
**Epic:** console-a80  
**Allowlist:** backend/crates/policy/domain/**

## Outcome

Unit tests: CedarPolicyStatus/Effect/ValidationStatus wire roundtrips; runtime
enforcement flags; validate_key rejects empty/uppercase/overlong. Pure domain.

## Verification

cargo test -p console-policy-domain --lib  # 5 passed
Locked executed-tests-baseline policy domain attrs.

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
    "Essentialism / YAGNI": "Domain unit tests only.",
    "Chesterton's Fence": "Draft never runtime-enforced retained.",
    "Pragmatism": "5 unit tests green.",
    "Red Team": "No client authority fields; keys fail closed.",
    "Operability / Day-2": "Baseline locked.",
    "Blast-radius / cell-based": "policy/domain only.",
    "Telemetry-first": "cargo counts.",
    "Zero-trust / defense-in-depth": "Unknown wire values rejected."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Policy status/effect wire enums lacked roundtrip coverage beyond draft/enforced pair.",
    "validate_key had no unit tests for empty uppercase overlong cases."
  ],
  "decisions_changed_or_rejected": [],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
