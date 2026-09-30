# Ledger: Grok dual-track process harness (Bun fix-the-process)

**Date:** 2026-08-06  
**Kind:** process / delivery harness evidence  
**Related:** DN-0005, BUN-PARALLEL-DISCIPLINE

## Outcome

Land reusable Grok workflows and harness under `.grok/` so agents cannot fall into
endless waiting-for-CI: **ci-fleet-tick** + **product-process-tick** (orchestrated by
**program-tick**). Process upgrades edit workflows/tools, not product scope.

Also exclude `.grok/` from first-party documentation-manifest universe (agent harness,
not product custody).

## Workflows

- program-tick, ci-fleet-tick, product-process-tick
- domain-increment (Admit phase), process-upgrade

## Non-goals

No PRODUCT HOLD clear. Leader-only merge. No one-shot hooks.

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
    "Essentialism / YAGNI": "Dual-track workflows only; no product feature scope.",
    "Chesterton's Fence": "Doc-manifest excludes agent harness, not product docs.",
    "Pragmatism": "Reusable workflows replace passive CI wait.",
    "Red Team": "Leader-only merge; no auto-merge in workflows.",
    "Operability / Day-2": "Catalog + process-upgrade for class recurrence.",
    "Blast-radius / cell-based": ".grok + manifest exclude + ledger only.",
    "Telemetry-first": "Fleet classify + failure-classes detectors.",
    "Zero-trust / defense-in-depth": "Admit phase + fail-closed dual-track."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Endless waiting-for-CI is ops.passive-wait; program-tick pairs fleet with product/process."
  ],
  "decisions_changed_or_rejected": [],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
