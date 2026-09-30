# Release-please bot tip: documentation custody pair (console-9ry follow-up)

**Date:** 2026-08-11
**Kind:** authority tip ledger for the release-please bot candidate custody extension
**Bead:** `console-9ry` follow-up / failure class `process.release-candidate-unsigned`
**Not product authority.** Clears no HOLD. Makes no production claim.

## Summary

console-9ry admitted github-actions[bot] tips whose parent..tip diff was exactly
`.release-please-manifest.json` + `CHANGELOG.md`. That unblocked authority
bootstrap on release PR #621, but Required CI stayed red:
`CHANGELOG.md` is first-party manifest evidence, so `check:doc-manifest` /
`check:doc-links` require regenerated
`docs/documentation-manifest.seed.json` + `docs/documentation-index.json`.
Those custody files were outside the bot-candidate allow-list, so the tip could
not self-heal.

This tip extends the **same fail-closed class** (not a broad unsigned exception):

1. Identity / subject / PR event author+ref clauses unchanged from console-9ry
2. Parent..tip MUST still change both release metadata files
3. Parent..tip MAY also change the documentation custody pair (all-or-nothing,
   regular mode-100644 modifications)
4. Any other path still falls through to the SSH C/T train and fails closed
5. `release-please.yml` runs `converge-release-please-doc-custody.mjs` so future
   release tips rewrite as one bot commit with changelog + custody converged

## Mechanism

- `scripts/console/release-please-bot-candidate.mjs` — `RELEASE_PLEASE_CUSTODY_PATHS` + ratchet tests
- `scripts/console/converge-release-please-doc-custody.mjs` — tip rewrite via protected generator + data-only worktree; refuses default GITHUB_TOKEN rewrite
- `.github/workflows/release-please.yml` — post-action converge step (`RELEASE_PLEASE_TOKEN` for push)
- `docs/CI-GATES.md` — prose locked to the same allow-list

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "approval",
    "release",
    "production"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Chesterton's Fence",
    "Red Team",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Separates changelog custody drift (doc gates) from authority bootstrap green.",
    "Chesterton's Fence": "Keeps the 9ry bot class; only adds the custody pair CHANGELOG already implies.",
    "Red Team": "Half custody / product paths still rejected; core release files remain mandatory.",
    "Operability / Day-2": "Workflow converge step removes the manual tip treadmill for each release.",
    "Blast-radius / cell-based": "Allow-list grows by exactly two generated custody blobs; no product paths.",
    "Zero-trust / defense-in-depth": "Event meta + identity + path allow-list; custody all-or-nothing."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "After 9ry, #621 tip 0ea88329 failed doc-manifest: CHANGELOG blob_sha c03fa533… ≠ tip OID adedcf25….",
    "Bot tip could not carry regenerated seed/index under the two-file allow-list."
  ],
  "decisions_changed_or_rejected": [
    "Rejected weakening verifier for arbitrary unsigned paths.",
    "Rejected perpetual signed human custody refresh for every release tip."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
