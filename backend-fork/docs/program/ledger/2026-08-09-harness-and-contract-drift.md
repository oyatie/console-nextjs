# Authority tip — the harness charges for what it costs to carry, and contract drift is checked without a client

**Date:** 2026-08-09
**Kind:** authority tip (T) for the lane-harness and contract-drift candidate
**Scope:** `.claude/workflows/**` and `scripts/check-platform-contract-drift.mjs`.
**Not product authority.** Clears no HOLD. Touches no backend crate, no migration, no OpenAPI document.

## Summary

- **Five review findings against the fan-out harness**, all from PR #612: overlapping owned roots
  refused at dispatch; the verify schema now requires the lane risk record, the commands the
  verifier actually ran, and an explicit oracle-integrity verdict; `program-tick` refuses lanes
  sharing a candidate worktree and enforces `maxLanes` before fan-out; the backlog audit may not
  close an issue on evidence reachable only from an unmerged branch.
- **A `scout` sidecar** that censuses the backlog, re-derives every dependency edge from the work
  rather than trusting the stored direction, computes the critical path in-script, and emits lanes
  proved disjoint. Separate from `lane-fanout` because it must run when there is no candidate tip
  and no worktree.
- **A maintainability lens, standing**, whose default verdict is DELETE: doc sprawl, crate hoarding,
  comment blobbing, private dialect where the repo has an idiom, a manual step a command could
  decide, and the inverse — a measured constant living only in someone's head.
- **A client-free route/contract drift check.** The three deleted OpenAPI suites stay deleted: each
  loaded `clients/ts/src/schema.d.ts`, removed by PIVOT-2026-07-28 with nothing regenerating it, so
  restoring them adds three certain `ENOENT` failures rather than an assertion. Their drift
  assertions were genuinely lost; this replaces them, reading the served routes and the published
  document directly. Measured: 582 backend `/api/` operations across 54 route sources.

## Three harness defects this branch found in itself

- **A rule kept in two of three sibling files is a coincidence.** The unknown-option guard existed in
  `lane-fanout` and `backlog-audit` and not in `program-tick`; the preflight now asserts it across
  every dispatcher rather than whichever file someone remembered.
- **A list of generated faces is a record of past burns.** The peripherals clause named exactly one
  generator — the documentation manifest, the one that had just failed a lane — and the next lane was
  failed by a different one, the first-party BUCK faces. The lock states two paragraphs below that
  clause that the third spelling means the mechanism is wrong; it was right about the clause. The
  total form is now REGENERATE, THEN ASK GIT: `git status --porcelain` producing any output means the
  commit is incomplete, because git cannot be fooled by a face nobody thought of.
- **Fan-out width must not scale with its input.** `scout` dispatched one agent per edge and one per
  ready bead — 131 agents against a 92-bead tracker, 117 of which died at an account limit, so a full
  quota bought a plan with zero lanes. Batched at ten, the same run is fifteen agents at unchanged
  depth. Width was never what costs wall-clock here; depth is.

## What the preflight caught before dispatch, and what it did not

The offline preflight is 159 assertions and refused to dispatch three times during this work: once on
a backtick that silently truncated `BASE_LOCK` — the exact defect that assertion exists for, in text
I was adding *about* not enumerating — and once on an assertion that pinned the standing-lens count to
a literal `5`, so ADDING a standing lens read as a regression. That one is worth stating plainly: an
assertion a legitimate improvement breaks is one that eventually gets "fixed" by deleting the
improvement. It now sizes against the set.

It did not catch `scout` emitting unusable owned roots, because `scout` had no preflight of its own.
That is recorded rather than repaired here: agent-authored paths went into the emitted plan
unvalidated, and a measured run returned 64 absolute `/Users/...` paths, the literal prose
`"<the remaining files from git grep ..."` split into roots named `"<the"` and `"remaining"`, and
`backend/` as a root — which under union-find transitively swallowed all 35 startable beads into one
lane owning the whole tree. The partition was correct; the input was never checked. Roots are now
normalised and validated in-script, verified against those exact strings.

## Verification

`node .claude/workflows/lane-fanout.test.mjs` — 159 assertions, ALL PASS, offline, stub agents.
`node scripts/check-platform-contract-drift.mjs` — PASS over 582 operations, 54 route sources.
`check:doc-links`, `check:doc-manifest`, `check:doc-citations`, `check:js-test-reachability`,
`check:ci-preflight`: PASS.

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
    "Constant-work / anti-fragility",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "A rule present in two of three sibling dispatchers was treated as a coincidence rather than a rule, and asserted across all of them.",
    "Essentialism / YAGNI": "A maintainability lens whose default verdict is DELETE now charges for doc sprawl, crate hoarding, comment blobbing and manual steps a command could decide.",
    "Red Team": "The harness is the thing that decides whether work is verified, so its own failure paths were attacked: the reporter died on its first detail-less failure, losing every later assertion and the failure count.",
    "Operability / Day-2": "Generated peripherals are now checked by regenerating and asking git, which cannot be fooled by a face nobody thought of, rather than by a list of the generators that have burned us.",
    "Blast-radius / cell-based": "Lane territory is partitioned by union-find before any lane exists, so two lanes cannot share a root by construction rather than by a guard that fires after the work is done.",
    "Constant-work / anti-fragility": "Fan-out width no longer scales with backlog size: one agent per bead became batches, after a run spent a full account quota to return a plan with zero lanes.",
    "Telemetry-first": "A dead batch is named and its subjects reported UNAUDITED rather than absorbed into a clean-looking result.",
    "Zero-trust / defense-in-depth": "Nothing is trusted from the layer that produces it: not a reviewer's severity label, not an agent's survival, not a lane's self-reported green, not an option the harness was handed, and now not an agent-authored file path -- which arrived absolute, as prose, and as the repository root itself."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "The unknown-option guard existed in two of three dispatchers, which makes it a coincidence rather than a rule.",
    "The peripherals clause enumerated one generated face and the next lane was failed by a different one.",
    "Fan-out width scaled with backlog size and exhausted an account limit mid-run.",
    "The preflight's own reporter threw on its first detail-less failure, discarding every later assertion.",
    "An assertion pinning the standing-lens count to a literal blocked the intended way of strengthening review.",
    "Agent-authored paths entered the emitted plan unvalidated: absolute, prose, and repository-wide roots all appeared in one measured run."
  ],
  "decisions_changed_or_rejected": [
    "Rejected restoring three deleted OpenAPI suites, because each loads a file PIVOT-2026-07-28 removed; built a client-free replacement instead.",
    "Rejected adding a second generated-face entry to the list, because the lock itself says the third spelling means the mechanism is wrong.",
    "Rejected separate qa/review/implement workflows, because lane-fanout already is that pipeline and three more files would be drift surface without new capability."
  ],
  "lens_set_changes": [
    "Added MAINTAINABILITY / COST OF CARRY as a standing lens: every other lens asks whether a change is correct, none charged for what it costs to keep."
  ]
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
