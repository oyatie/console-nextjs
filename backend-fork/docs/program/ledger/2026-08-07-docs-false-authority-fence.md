# Authority tip — docs false-authority fence

**Date:** 2026-08-07  
**Kind:** authority tip (T) for docs fence candidate  
**Scope:** In-place false-authority fences and quarry stamps under `docs/**` excluding `docs/current/*` product body.  
**Not product authority.** Does not clear HOLDs. Bulk move remains HOLD.

## Summary

- Residual PIVOT SSOT neutralized; program dual-authority retargeted to docs/current/*
- GO-LIVE / ENTERPRISE / PLATFORM / DESIGN-DOCTRINE / parity / overhaul fenced
- Unfenced specs/ideas quarry-stamped
- Parallel-lanes work graph idea (quarry) + Hindsight packet fields

## HOLDs remaining

PRODUCT/ROADMAP HOLDs unchanged. No production, frontend, or projection fan-out claims.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "approval"
  ],
  "selected_lenses": [
    "Essentialism / YAGNI",
    "Chesterton's Fence",
    "Red Team",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Essentialism / YAGNI": "Fence existing historical docs without moving or rewriting the corpus.",
    "Chesterton's Fence": "Preserve historical evidence while removing its implied current authority.",
    "Red Team": "Explicitly deny product authority and HOLD clearance.",
    "Operability / Day-2": "Keep docs/current/* as the single maintained authority path.",
    "Blast-radius / cell-based": "Limit changes to documentation outside docs/current/*.",
    "Telemetry-first": "The documentation and reasoning-lens gates detect missing fences or evidence.",
    "Zero-trust / defense-in-depth": "Treat every historical planning document as non-authoritative unless current authority grants it."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Historical planning and readiness documents could be mistaken for current authority without an explicit fence."
  ],
  "decisions_changed_or_rejected": [
    "Rejected bulk moves or deletion because they would disturb historical evidence."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
