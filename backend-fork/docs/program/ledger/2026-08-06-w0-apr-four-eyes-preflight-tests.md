# Ledger: W0-APR four-eyes pending + impact preflight tests

**Date:** 2026-08-06  
**Epic:** console-66n  
**Allowlist:** backend/crates/governance/domain/**

## Outcome

Unit tests: four-eyes `Some(false)` is Pending not Allow; impact preflight
allows Detach-only dependents; GateKind ORDER fixed.

## Verification

cargo test -p console-governance-domain --lib  # 10 passed
executed-tests-baseline locked.

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
    "Essentialism / YAGNI": "Three pure governance domain tests.",
    "Chesterton's Fence": "Four-eyes pending vs fail-closed preserved.",
    "Pragmatism": "10 governance unit tests green.",
    "Red Team": "Distinct-human gate pending is not allow.",
    "Operability / Day-2": "Impact preflight detach path covered.",
    "Blast-radius / cell-based": "governance/domain only.",
    "Telemetry-first": "cargo counts.",
    "Zero-trust / defense-in-depth": "Missing gate still fail-closed."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Four-eyes Some(false) pending path lacked unit coverage (only None fail-closed).",
    "Impact preflight allow-on-detach path untested beside restrict-blocks."
  ],
  "decisions_changed_or_rejected": [],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
