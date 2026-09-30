# Authority tip — Train 2 leaf admission (ann / soe / mbl / cm3 / wnv / 9ze / ls3 / zd7 / l6c / 937 / pdo / ctk)

**Date:** 2026-08-10
**Kind:** authority tip ledger bound on candidate C (product tip below); jurisdiction tip T is register-only (#618 grammar)
**Candidate (authority train):** `732eb5bdc960db55cfac24ab4785f6bd96e6f460` (immutable absolute SHA of the product tip that C parents; rekeyed at C commit time)
**Scope:** twelve critic-APPROVED leaves after #622; bqg hardening needle folded with soe (not a separate ordered leaf); lx6/0lj PARKED.
**Not product authority.** Clears no HOLD. Makes no production, frontend, or projection claim.

## Summary

- **Gates:** parser-based OpenAPI `$ref` totality (ann); platform-contract-drift module-scope fail-closed (wnv); ADR-0030 §8 Rust console route inventory (9ze); image-release canonical-enforce wiring (soe) + production-hardening static needle (bqg fold); audit/RLS app-surface expansion + baseline (937); independent crate-disk census for backlog-audit (pdo); authz `custom_role_runtime_feature_allowed` SSOT for AiAssist (ctk).
- **Product:** leave 근로기준법 §60 reshape + OpenAPI filter enum (cm3); ExternalSealSigner crate (mbl → ae5 for production wiring); collaboration audience membership (l6c); acting-tenant audit org threading (ls3); stale-take confirm graft attestation (zd7).
- **Soft serializations honored:** ann→wnv (`package.json`); zd7→pdo (`lane-fanout.test.mjs`); l6c before 937 (`backend/app/src/`).

## Remaining HOLDs / follow-ups (not closed by this tip)

- console-ae5 — production ExternalSealSigner wiring (mbl crate only here)
- console-we1 / console-kwm — cm3 deferred consult + privilege-census pin
- console-i91 — ann class sweep across scripts/check-*.mjs
- console-cvh — 9ze residual layer-boundary sighting
- console-zyw — lane-fanout.test.mjs source-contains retarget (zd7 file owner)
- console-h3e — EmploymentQuery subject_id totality (post-#622 Batch B)
- console-jth — COVERED header flip / /*-in-quoted-SQL residual
- console-9ry — release-please vs authority-train unsigned bot candidate strategy
- console-bqg — remain annotated if separate-leaf park still open after fold
- console-lx6 / console-0lj — identity siblings PARKED

## Critic receipt binds (lane worktree APPROVE tips)

| leaf | tip | critic |
|------|-----|--------|
| soe | 09ac785f0 | soe-critic APPROVE |
| bqg (fold) | 61175ffa2 | console-bqg-critic APPROVE (separate-leaf DENIED) |
| mbl | 458b88c1f | mbl-critic APPROVE |
| 9ze | 18f6e2f28 | 9ze-critic APPROVE |
| ls3 | b7f2fb228 | ls3-critic APPROVE |
| l6c | 3b71464ff | l6c-critic APPROVE |
| cm3 | 6609ba107 | cm3-critic APPROVE |
| zd7 | b898287e7 | zd7-critic APPROVE |
| ann | 7e8bb2808 | ann-critic APPROVE |
| wnv | 73dcc7d99 | wnv-critic APPROVE |
| 937 | 403cd8f87 | 937-fix-critic APPROVE |
| pdo | ec3d60b87 | pdo-critic APPROVE |
| ctk | ba2be5d9f | ctk-critic APPROVE |

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
    "Cartesian doubt": "Separated product tip P from legal C (ledger+manifest) and T (jurisdiction-only); verified #622 squash tip e86cdd1bb before any cherry-pick; critic APPROVE bound to lane tips not hub stale BLOCK copies.",
    "Essentialism / YAGNI": "Admitted the 12 ordered leaves only; folded bqg needle with soe rather than opening a 13th leaf; left Batch B and #621/9ry outside this train.",
    "Chesterton's Fence": "Kept #618 C/T grammar (ledger+seed/index on C; jurisdiction register on T) after wave-1 proved ledger-on-T fails authenticate allow-list when mixed with manifest.",
    "Red Team": "Serialized soft collisions (package.json, lane-fanout.test.mjs, backend/app/src) so 3-way merges could not silently drop gate pins; refused to admit lx6/0lj from the ctk tip.",
    "Operability / Day-2": "Regenerated documentation seed+index on C for the new ledger path; SSH-signed C and T; check-admit-sync after push; babysit image-release soe edge deferred to post-merge watch.",
    "Blast-radius / cell-based": "Path-disjoint ordered admit on one branch from post-#622 main; no Batch B fold; migration 0216 free after #622 0213-0215.",
    "Zero-trust / defense-in-depth": "Pinned SSH-signed C and T; authenticate-console-authority allow-list; critic receipts required APPROVE before cherry-pick."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "bqg tip is a real hardening delta (+103 lines) atop soe-equivalent parent — fold with soe, do not close as duplicate.",
    "Hub ann-critic.json STALE BLOCK ignored; lane ann-critic APPROVE @ 7e8bb2808 is canonical.",
    "937 forces doc-manifest regen onto C; T stays jurisdiction-only.",
    "cm3 migration 0216 did not collide with #622 migrations 0213-0215.",
    "Admit-window fix: cargo fmt on leave/domain; zyw oracle retargeted to refusing repo-wide same-name fallback (proven red after wnv).",
    "Admit-window: ratcheted executed-tests baseline for cm3/mbl/l6c/ctk/937 attribute gains before push.",
    "Admit-window CI: clippy allow on rls-arming tests; retarget ecosystem-plan carveout citation to allowed_exclusion_set_is_the_bound_carveouts."
  ],
  "decisions_changed_or_rejected": [
    "Rejected separate-leaf admit for bqg; fold needle only.",
    "Rejected folding Batch B (ugg/avb/6n4/nuc/h3e/jth) into train-2.",
    "Rejected merging #621 until console-9ry strategy.",
    "Rejected admitting lx6/0lj from ctk tip."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
