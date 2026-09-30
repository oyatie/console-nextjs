# Authority tip — lane harness cannot report a false green

**Date:** 2026-08-08
**Kind:** authority tip (T) for the P3 org-change and lane-harness candidate
**Scope:** `.claude/workflows/**` (the reusable fan-out harness and its offline preflight), the
P3 org-change preflight persistence fix and its PostgreSQL lane wiring, and the CI/Buck plumbing
those two require.
**Not product authority.** Clears no HOLD. Makes no production, frontend, or projection claim, and
does not assert that the writer-ownership gate runs in CI — that lane is still converging and lands
separately.

## Summary

- **Convergence keys on proof, not on a severity label.** A measured run reported `CONVERGED` while
  both reviewers held six separately *proven* fail-opens, each filed "major". Findings now carry
  `provenByExecution` and `ownerLease`; a major the reviewer watched fail blocks the lane, one they
  argued does not, and an owner lease releases at any severity.
- **A dead reviewer is not an absent finding.** `parallel()` resolves a died agent to `null` and
  `.filter(Boolean)` erased it, so a lane could converge on one surviving reviewer in silence.
  Session limits took 4 of 7 agents from one run and 3 of 3 from another. A dead *standing* lens now
  blocks convergence and is named in the log.
- **Telemetry that could not be wrong.** `checkersPerBuild` was `((LENSES.length + 1) * rounds) / rounds`
  — algebraically a constant, structurally unable to observe a dead agent. It now measures dispatched
  against returned.
- **Standing lenses that never ran.** `LENSES = ARGS.lenses || [...]`, and every invocation passed
  `lenses`, so oracle integrity — the most common rejection cause in this programme — had never once
  been reviewed for. Custom lenses now add to the standing set rather than replacing it.
- **Unknown options abort.** In a sibling runner the same defect — an option accepted, ignored, and
  the run looking normal — cost six lanes and roughly 2.3M tokens.
- **Landing is part of the pipeline.** The harness had no terminal step, so converged work sat on a
  detached HEAD in a worktree nobody landed; twelve-plus worktrees accumulated above `main` with zero
  open PRs. Converged lanes now land onto a long-lived integration branch, single-writer, refusing a
  worktree with a dirty status.
- **P3 org-change:** preflight persisted rows it had no business persisting. Submit now recomputes,
  legacy rows are not stranded, and the zero-write proof is wired into the PostgreSQL lane so it is
  executed rather than asserted.

## Review round — what the automated reviewer found that the preflight did not

Eleven P1 findings were filed against `4dd6cd899`. Each was re-measured on the then-current head
before anything was changed; none had gone stale, and none was invented.

- **A dead *verifier* is not a verification.** The previous round fixed the dead *reviewer* and
  stopped one stub short of the generalisation. `verifierOk = !verify || (...)` made a verifier
  killed by a session limit read as agreement, and the lane logged "independently re-verified by
  4/4 reviewers" with `verified=null`. Convergence now requires a live verifier whenever the lane
  claims a green; a dead one emits `CANNOT CONVERGE — VERIFIER DIED` and is classified
  `checker-died`, not `unreproducible-claim`, so telemetry does not blame a brief that was correct.
  The deliberate skip for a non-green build is preserved structurally and is asserted separately, so
  the fix cannot degenerate into "every missing verifier is fatal".
- **Landing named nothing.** The land prompt read `o.lane.key` / `o.lane.wt` / `o.fix` off the
  flattened record, whose `lane` is a string, and rendered `1. undefined — worktree undefined`. The
  test had asserted that landing was *dispatched*, never what it was dispatched *with* — the
  harness's own "execute the control" rule turned inside out. It is now built from the original
  results, and the total form of the assertion is `!/undefined/.test(landPrompt)`.
- **Landing was not bound to the reviewed head.** A clean worktree was accepted as the binding, so a
  commit added after the reviewers finished could be landed unreviewed. `headSha` is now required on
  the verify schema, captured by the verifier in the same parallel block as the review, printed per
  lane, and the lander must refuse on mismatch. A lane with no captured head refuses.
- **Fan-out fabricated `/Users/jasonlee` paths.** `program-tick` sent every selected lane to a
  hard-coded workstation path regardless of `candidateWt`. Worktrees are now resolved against the
  inventory the Collect phase read; an unresolvable lane refuses the whole fan-out rather than
  silently dropping the selected work.
- **The two OpenAPI contracts the code stopped honouring** were republished: the org-change
  preflight no longer stores a receipt or promotes to `PRECHECKED`, and the governance decision
  endpoint now requires an open approval request.
- Buck resource metadata, the shard domain-entry tripwire, the reachability baseline and the
  fan-out preflight's CI wiring were closed in the leaves that precede this tip.

### Second review round — three more, and one reviewer remedy rejected on measurement

- **Two lanes could declare the same worktree.** Nothing checked, and two writers in
  one worktree has already cost this programme a round and produced 28 duplicate beads.
  Duplicate `wt` and duplicate lane `key` now abort the fan-out before dispatch.
- **`scopeCreep` was collected and ignored** at both convergence sites, so a lane that
  widened past its owned root could still report CONVERGED.
- **`blockedTargets` was documented and rejected** by the unknown-key guard — the guard
  that exists to stop silent drops was itself the silent drop.
- **The backlog audit reported APPLIED when its single writer died**, and its
  empty-census guard refused a tracker that is genuinely clean. Both fixed in the
  commit preceding this tip.

The reviewer's migration-slot remedy was **rejected on measurement, not on preference**.
Asked to vacate 0212 for a ledger pre-assignment, the gate answers directly: occupied is
`PASSED`/EXIT=0, vacated is `FAILED [NonContiguousMigrationVersion] missing migration
version 0212 before 0213`/EXIT=1. `origin/main` tops out at 0211. A reserved number and a
contiguity gate cannot both be satisfied by whoever merges first, which is what §5 of the
wave-4 ledger already says and what row 10 already did once. The ledger was corrected
rather than the migration.

Not closed, and honestly so: the reviewer asked for three deleted OpenAPI contract suites to be
restored. Each reads `clients/ts/src/schema.d.ts` at module load; `clients/` was removed by
PIVOT-2026-07-28 and nothing generates it, so restoring them re-introduces three guaranteed
`ENOENT` failures. The deletion was an explicit owner override recorded on lane console-99j. The
route/schema drift assertions they carried are genuinely gone and need a client-free replacement;
that is a follow-up, not a revert.

## Verification

`node .claude/workflows/lane-fanout.test.mjs` — 52 assertions, ALL PASS, offline, stub agents. Every
assertion is a defect that actually shipped. Two classes are invisible to reading and both occurred:
a backtick nested inside a prompt template silently truncated `BASE_LOCK`, and a rule can be written,
documented, believed, and reachable by nothing. `node --check` is the wrong validator — the body is an
async function with top-level `await` — so the preflight compiles it through `AsyncFunction`, which is
how the truncation was caught. It earned its place on its first run by failing: the new landing phase
executed and its branch and head SHA were only logged, never returned.

Eleven of the twelve assertions added this round were proved RED first, by running the final test
file against the pre-fix sources extracted with `git show HEAD:...` — including the exact false
green, `a: CONVERGED round 1 (independently re-verified by 4/4 reviewers)` printed with
`verified=null`. Adjudicated independently of the fixing agent by driving the compiled harness with
every reviewer clean and the verifier returning `null`: the lane reports `UNCONVERGED`, and the
control with a live clean verifier still reports `CONVERGED`, so the block is not an over-block.

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
    "approval"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Essentialism / YAGNI",
    "Red Team",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Separated a reviewer's severity opinion from the fact of whether they observed the failure; convergence now rests on the latter.",
    "Essentialism / YAGNI": "Replaced enumerations with total rules rather than extending them, deleting more rule than was added.",
    "Red Team": "Treated the harness as the adversary's target: a dead reviewer, an ignored option and a constant telemetry expression were each a silent path to a false green.",
    "Operability / Day-2": "Landing became part of the pipeline so converged work cannot accumulate unlanded, and the preflight runs offline in under a second before every dispatch.",
    "Blast-radius / cell-based": "Landing is single-writer and refuses a worktree with uncommitted changes, because two writers in one worktree has already cost this programme a round.",
    "Telemetry-first": "Checker dispatch and return counts are now measured rather than computed from intent, so a lost agent is visible.",
    "Zero-trust / defense-in-depth": "The harness is the trust boundary that decides whether work is verified, so it now trusts none of its own inputs by default: not a reviewer's severity label (only an observed failure blocks), not an agent's survival (a missing standing lens blocks rather than passing quietly), not a lane's self-reported green (an independent re-runner must reproduce it), and not an option it was handed (an unrecognised argument aborts instead of being ignored). Each is an independent layer, so no single one failing admits a false green."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "A convergence rule keyed on a reviewer's severity label admitted six proven fail-opens.",
    "Agent death was silently absorbed, so a lane could converge on a fraction of its intended review.",
    "A telemetry expression that reduced to a constant reported intent as though it were measurement.",
    "The dead-checker fix stopped one stub short: the verifier resolved to null from the same parallel() block and read as agreement.",
    "Landing was asserted to have been dispatched but never asserted on its payload, so it rendered 'undefined - worktree undefined' and was bound to no reviewed head."
  ],
  "decisions_changed_or_rejected": [
    "Rejected a consolidation phase as the remedy for unlanded work, because it institutionalises the debt instead of removing what generates it.",
    "Rejected blocking convergence on any dead checker, because forcing a rebuild round over a lost custom lens costs more than it saves; only standing lenses block.",
    "Rejected restoring three deleted OpenAPI contract suites as filed, because each loads clients/ts/src/schema.d.ts, which PIVOT-2026-07-28 removed; restoring them adds three certain ENOENT failures rather than a drift assertion."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
