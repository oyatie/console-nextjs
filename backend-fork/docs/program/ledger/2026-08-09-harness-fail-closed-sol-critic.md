# Authority tip — fail-closed Sol/Codex harness defects on the scout candidate

**Date:** 2026-08-10
**Kind:** authority tip (T) for candidate `5203b7c90244377e961b09649e0dd29ba900c786`
**Candidate (authority train):** `5203b7c90244377e961b09649e0dd29ba900c786` (immutable absolute SHA; not a relative `HEAD^` expression)
**Workflow fix commit:** `5203b7c90244377e961b09649e0dd29ba900c786`
**Authority-train parent tip^:** seed-registration commit that adds the tip ledger blob to the documentation manifest (same harness tree as the candidate above).
**Scope:** `.claude/workflows/{scout,lane-fanout,stale-take-audit}.js`, `.claude/workflows/lane-fanout.test.mjs`, `scripts/check-platform-contract-drift.mjs`, this ledger tip.
**Not product authority.** Clears no HOLD. Touches no backend crate, no migration, no OpenAPI document.

## Summary

Codex P1/P2 fail-closed findings on tip `5a629c66a`, fixed on the candidate above:

1. **lane-fanout.js — nonempty claimed commands on done.** `status=done` with omitted/blank `commands` no longer converges vacuously against a verifier's unrelated `commandsRun`.
2. **stale-take-audit.js — diffs in `args.repo`.** Prompted diffs use `git -C ${REPO}` for both directions so agents audit `args.repo`, not the inherited cwd.
3. **stale-take-audit.js — per-file audit coverage.** Every requested path must appear exactly once in audit results before "full coverage".
4. **scout.js — extensionless owned roots.** `Dockerfile` / `BUCK` / `Makefile` paths are preserved as exact file roots instead of inventing `…/Dockerfile/`.
5. **check-platform-contract-drift.mjs — discover after strip.** Route-source discovery runs `stripRustCommentsAndLiterals` before `.route(` detection so doc-only hits cannot abort the gate.
6. **Ledger tip binding.** Candidate recorded as an immutable absolute SHA (not a relative `HEAD^` expression).

## Verification

- `node .claude/workflows/lane-fanout.test.mjs` — ALL PASS.
- `node scripts/check-platform-contract-drift.mjs` — PASS, 582 backend `/api/` operations across 54 route sources.
- `git diff --check origin/main...HEAD` — clean.

## HOLDs remaining

Unchanged. No production promotion, no frontend, no payment execution, no invented compliance scope.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "contracts",
    "release"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Essentialism / YAGNI",
    "Red Team",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Reproduced empty claimed-commands vacuity and unqualified stale-take git diffs against args.repo.",
    "Essentialism / YAGNI": "Closed the two P1s and same-class P2s without expanding product scope.",
    "Red Team": "Treated omitted commands, wrong-cwd audits, partial audit batches, and doc-only .route( hits as adversarial false-greens.",
    "Operability / Day-2": "Regression probes cover nonempty commands, git -C REPO, missing audit files, extensionless roots, and strip discovery.",
    "Blast-radius / cell-based": "Harness-only change; no backend/OpenAPI/migration edits.",
    "Zero-trust / defense-in-depth": "Done greens require named commands the verifier can re-run; coverage claims require one result per requested file."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Empty claimedCommands made unrun coverage vacuously true.",
    "stale-take-audit instructed unqualified git diff against inherited cwd.",
    "Audit coverage counted dead batches only, not missing per-file results.",
    "scout normaliseRoot invented extensionless file directories.",
    "Route discovery matched raw .route( inside doc comments."
  ],
  "decisions_changed_or_rejected": [
    "Rejected whole-file maskStringLiterals for route discovery after it dropped real multi-line routers.",
    "Rejected leaving cheap same-class P2s open once P1s were under repair."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
