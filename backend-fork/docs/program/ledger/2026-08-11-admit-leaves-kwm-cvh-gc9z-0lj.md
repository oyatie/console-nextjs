# Authority tip — residual admit (kwm / cvh / gc9z / 0lj)

**Date:** 2026-08-11
**Kind:** authority tip ledger bound on candidate C; jurisdiction tip T is register-only (#618 grammar)
**Candidate (authority train):** `6cfec81dd892da87480dbf272ebe62ca929f1c06` (product tip parent of C; rekeyed at C commit time)
**Scope:** critic-APPROVED path-disjoint leaves on main base `eca122ad0` (#745). Clippy heal on cvh `ui_surface` included.
**Not product authority.**

## Summary

- **kwm:** §60 kr_labor_guardrails privilege-census named pin
- **cvh:** layer-boundary smuggled UI sight (+ clippy heal)
- **gc9z:** LifecycleError kinds into PgPayrollError
- **0lj:** identity REST branch-removal authz

## Excluded

- g14a (census baseline stale vs current main)
- jdbz / o498 / ah7 (o498 dark tests; ah7 no critic APPROVE; jdbz deferred)

## Critic receipt binds

| leaf | product tip | critic |
|------|-------------|--------|
| kwm | fe10d8a4d / restack | kwm-critic APPROVE |
| cvh | edd4e6344 + clippy heal | cvh-critic APPROVE |
| gc9z | tip with critic APPROVE | gc9z-critic APPROVE |
| 0lj | 9120273fe | 0lj-critic APPROVE |

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "authz",
    "hr_payroll",
    "release"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Essentialism / YAGNI",
    "Chesterton's Fence",
    "Red Team",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Local clippy -D warnings reproduced Backend CI failure on ui_surface.rs; reasoning-lens revalidated after ledger evidence heal.",
    "Essentialism / YAGNI": "Admit only kwm/cvh/gc9z/0lj; park g14a/jdbz/o498/ah7.",
    "Chesterton's Fence": "Keep #618 C=ledger+seed/index+executed-tests, T=jurisdiction-only grammar.",
    "Red Team": "Path overlap among included leaves = NONE; o498 excluded for dark unit tests.",
    "Operability / Day-2": "Force-with-lease restack of admission tip; never GitHub Update-branch.",
    "Blast-radius / cell-based": "Clippy heal scoped to cvh ui_surface; gc9z one-writer fence closed.",
    "Zero-trust / defense-in-depth": "SSH-signed C and T; critic APPROVE required per admitted leaf."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Backend CI failed clippy collapsible_if/manual_contains on cvh tip - healed.",
    "g14a census red on current main (stale residuals) - parked.",
    "o498 adds dark #[cfg(test)] in identity adapter-postgres - parked."
  ],
  "decisions_changed_or_rejected": [
    "Rejected folding g14a while prototype-chain census is red on current main.",
    "Rejected folding o498 while executed-tests ratchet reports a new dark unit-test source.",
    "Rejected inventing ah7 critic APPROVE; implementer-only tip stays out."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

**Manifest rebind:** C includes `docs/documentation-manifest.seed.json` + `docs/documentation-index.json` for the new ledger path.
