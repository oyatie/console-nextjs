# Authority tip — cursor pkill/lock race ban (console-pkill-lock-ban)

**Date:** 2026-08-11
**Kind:** candidate ledger evidence for PR #757 (C is the signed product commit; T is the jurisdiction tip child)
**Scope:** deny command-shaped git-targeted `pkill`/`kill` and unlock-by-`rm` of `index.lock` in `git-lock-enforcer` before `CURSOR_ALLOW_GIT_DANGEROUS`; add `scripts/cursor/safe-stale-git-lock.sh`; document `process.git-pkill-lock-race`.
**Not product authority.** Clears no HOLD. Makes no production claim.

## Summary

- Hook deny for cross-agent git process/lock races (measured in agent transcripts).
- Stale locks only via `bash scripts/cursor/safe-stale-git-lock.sh <lock-path>` (dead PID + age gate).
- Operator escape: `CURSOR_ALLOW_GIT_PKILL=1` (does not follow from `CURSOR_ALLOW_GIT_DANGEROUS`).

## Remaining HOLDs / follow-ups

- None closed by this tip; ops note for agents after merge: use safe-stale helper only.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
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
    "Cartesian doubt": "Measured agent transcripts showed pkill/rm-index.lock races, not checked-in cleanup scripts; deny command shape before CURSOR_ALLOW_GIT_DANGEROUS.",
    "Essentialism / YAGNI": "Ship hook deny + safe-stale helper + docs only; no broader git orchestration rewrite.",
    "Chesterton's Fence": "CURSOR_ALLOW_GIT_DANGEROUS remains for intentional dangerous git; pkill escape is a separate CURSOR_ALLOW_GIT_PKILL=1 operator flag.",
    "Red Team": "Hostile agents must not unstick by killing sibling git or deleting locks; pattern matches command tokens not path substrings like lane-console-pkill-*.",
    "Operability / Day-2": "Stale locks: bash scripts/cursor/safe-stale-git-lock.sh only (dead PID + age gate).",
    "Blast-radius / cell-based": "Deny is local to command enforcement; does not cancel in-flight git held by a live PID.",
    "Zero-trust / defense-in-depth": "Deny precedes dangerous-git allow; operator escape is explicit and non-inherited from CURSOR_ALLOW_GIT_DANGEROUS."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Authority train heal required: C carries product+ledger+doc-manifest; T is jurisdiction-only.",
    "Empty retrigger commit failed C..T authority-document requirement."
  ],
  "decisions_changed_or_rejected": [
    "Rejected weakening the new hook deny to land CI.",
    "Rejected unlock-by-rm; safe-stale helper is the only unlock path."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
