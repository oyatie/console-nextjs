# Authority tip — Wave-2 admit (dgo.1 / ae5 / i91)

**Date:** 2026-08-11
**Kind:** authority tip ledger bound on candidate C (product tip below); jurisdiction tip T is register-only (#618 grammar)
**Candidate (authority train):** `f36dba1658255f4d1b8853aa7781fc198e1807ad` (immutable absolute SHA of the product tip that C parents; rekeyed at C commit time after #741 restack)
**Scope:** three critic-APPROVED leaves after restack onto main `57fddda198ef3a6e0670aa60ad365417c9f22b2d` (#741 payroll LifecycleError CanonicalPort). we1 critic APPROVE exists but is HOLD for this PR (not folded).
**Not product authority.** Clears no HOLD beyond beads closed on merge. Makes no production tamper-evident claim for ae5 custody daemon (console-9gk).

## Summary

- **dgo.1:** distinct-natural-person four-eyes bar for Company/HR/Payroll (governance).
- **ae5:** ExternalSealSigner production wiring + custody transport (audit-chain / app). Custody daemon E2E remains HOLD.
- **i91:** own-property lookups for prototype-chain false-resolve across scripts/check-*.mjs (+ shared `scripts/own-property.mjs`). Residual census → console-g14a.

## Remaining HOLDs / follow-ups (not closed by this tip)

- console-we1 — leave §60⑤ consult; critic APPROVE exists but not admitted on #742 (separate train)
- console-9gk — custody daemon E2E / do not overclaim gh#271 closed-as-tamper-evident
- console-g14a — i91.1 residual census
- console-9sxn — we1 OpenAPI lease
- console-uhlv — Actions secret RELEASE_PLEASE_TOKEN (ops; never commit value)
- console-umgn — converge heal-mode tip e13c00b58 still needs critic+admit (post-#621)

## Critic receipt binds (lane worktree APPROVE tips)

| leaf | product tip | critic |
|------|-------------|--------|
| dgo.1 | 1c0cec8b73fea98cba8de626c0a727a572045a22 | dgo.1-critic APPROVE |
| ae5 | e7e943565284c68a81c95ad1ebedf73a87e238d5 | ae5-critic APPROVE |
| i91 | 5a3f6742c30bb117296aace8291bbabfa4a129af | i91-critic APPROVE |

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
    "Cartesian doubt": "Restacked onto origin/main 57fddda19 (#741) before push; product tip parent of C is f36dba165 (i91); we1 APPROVE exists but is out of this PR.",
    "Essentialism / YAGNI": "Admit only path-disjoint APPROVED trio dgo.1/ae5/i91; refuse folding we1 into #742 despite critic APPROVE.",
    "Chesterton's Fence": "Keep #618 C=ledger+seed/index, T=jurisdiction-only grammar after restack.",
    "Red Team": "Path overlap check dgo.1/ae5/i91 = NONE; we1 leave paths remain absent from this tip.",
    "Operability / Day-2": "Ops bead console-uhlv for RELEASE_PLEASE_TOKEN; we1 remains a separate admit after #742.",
    "Blast-radius / cell-based": "Single admission lane checkout under hub nesting; restack via rebase onto main, never unsigned branch button.",
    "Zero-trust / defense-in-depth": "SSH-signed C and T after restack; critic APPROVE required for each admitted leaf only."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "we1 critic APPROVE observed but not admitted on #742 — separate train.",
    "ae5 custody daemon E2E remains HOLD (console-9gk).",
    "Actions RELEASE_PLEASE_TOKEN tracked as console-uhlv — never commit secret."
  ],
  "decisions_changed_or_rejected": [
    "Rejected folding we1 into #742 after main #741 restack despite critic APPROVE.",
    "Rejected inventing a four-leaf admit; keep three-leaf scope (dgo.1/ae5/i91)."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

**Manifest rebind:** C rebind commit after initial ledger tip adds `docs/documentation-manifest.seed.json` + `docs/documentation-index.json` for the new ledger path (forward-only; no history rewrite).
