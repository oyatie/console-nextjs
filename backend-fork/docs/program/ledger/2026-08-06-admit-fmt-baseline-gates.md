# Ledger: Admit phase rustfmt + executed-tests baseline gates

**Date:** 2026-08-06  
**Kind:** process upgrade  

## Outcome

Extend domain-increment Admit to require backend rustfmt check and
check-executed-tests (with --update commit on gain). Add failure classes
ops.rustfmt-drift and ops.executed-tests-baseline plus a setup tip.

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
    "Essentialism / YAGNI": "Admit checklist only; no product scope.",
    "Chesterton's Fence": "Keeps existing admit gates; adds two classes that burned hosted wall.",
    "Pragmatism": "Mirrors rustfmt and executed-tests failures from W0-ONT PR.",
    "Red Team": "No auto-merge; process-only paths.",
    "Operability / Day-2": "Tip file for agents; failure-classes catalog.",
    "Blast-radius / cell-based": ".grok only.",
    "Telemetry-first": "Class ids for retro.",
    "Zero-trust / defense-in-depth": "Fail closed before push."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "W0-ONT failed hosted rustfmt and executed-tests baseline gain after adding tests.",
    "Admit phase listed gates but omitted fmt and baseline update."
  ],
  "decisions_changed_or_rejected": [],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
