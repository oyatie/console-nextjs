# Release-please bot candidate admission class (console-9ry)

**Date:** 2026-08-10
**Kind:** authority tip ledger for the fail-closed release-please bot candidate class
**Bead:** `console-9ry` / failure class `process.release-candidate-unsigned`
**Not product authority.** Clears no HOLD. Makes no production claim.

## Summary

Release Please tips are authored by `github-actions[bot]` and cannot carry the pinned
SSH candidate signature. Rebuilding every release as a hand-signed C/T train (0.3.3
pattern) works but recreates the treadmill. This tip admits a **narrow class** instead:

1. Tip author/committer are exactly github-actions[bot] / GitHub noreply
2. Subject matches `chore(<scope>): release X.Y.Z`
3. Parent..tip changes only `.release-please-manifest.json` and `CHANGELOG.md`
   (regular mode-100644 modifications)
4. On `pull_request_target` bootstrap: event PR author is `github-actions[bot]` and
   head ref matches `release-please--branches--main--components--*`
5. Synthetic merge `M` remains structural (two parents, tree equals tip)

Product / authority PRs still require pinned SSH C/T. Extra paths, human PR authors,
or non-release subjects fall through to the SSH train and fail closed.

## Mechanism

- `scripts/console/release-please-bot-candidate.mjs` — shared classifier + ratchet tests
- `verify-console-authority-train.mjs` — CI truth-ledger / plan-fanout admit the class
- `verify-console-pr-authority-bootstrap.mjs` — required `authenticate-console-authority`
  admits the class (event meta required) and skips detached-C checks for it
- `docs/CI-GATES.md` — documents the class so the gate and the prose cannot drift

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
    "Cartesian doubt": "Separates forgeable commit headers from trusted PR event author/ref.",
    "Chesterton's Fence": "Keeps SSH C/T for product; only replaces endless re-sign for bot+docs-only.",
    "Red Team": "Human PR with forged bot author rejected; path allow-list refuses product smuggling.",
    "Operability / Day-2": "Same class shared by bootstrap, truth-ledger, and plan-fanout.",
    "Blast-radius / cell-based": "Two release metadata files only; no product or trust-policy paths.",
    "Zero-trust / defense-in-depth": "Event meta + identity + path allow-list; examined-zero fails."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "PR #621 bot tip fails SSH C/T because C becomes main (unsigned squash) and T is the bot commit.",
    "0.3.3 re-signed; architect chose narrow verifier class over endless re-sign."
  ],
  "decisions_changed_or_rejected": [
    "Rejected broad unsigned exception and Dependabot-style bypass."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
