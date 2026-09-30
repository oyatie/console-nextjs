# Authority tip — CI fail-slow one-sweep (PR-0, D4)

**Date:** 2026-08-15
**Kind:** authority tip ledger bound on candidate C; T adds this ledger entry only
**Base:** `e21c04af2` (origin/main, post #773 parallel-development-framework; rebased over #768, #779, #780, #782, #783)
**Candidate C and Tip T:** both signed by the pinned authority (principal `jason19931225@gmail.com`, ED25519 `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`). Commit/tree SHAs are recorded in the post-merge readback update (T cannot name its own SHA).
**Scope:** `.github/workflows/ci.yml` (step-level fail-slow guards + per-job `collect-failures` steps + the `domain-unit` keep-going block), `tools/ci/cargo_needs_postgres.sh` + `tools/ci/cargo-test-runner.sh` (new; the extracted keep-going loop) + `tools/ci/cargo-test-runner.test.mjs` (new unit test), `scripts/lib/ci-workflow-executables.mjs` (keep-going attribution for `check-executed-tests`), `scripts/ci-collect-failures.mjs` (new), `scripts/verify.mjs` (classify the new `collect-failures` step in the local mirror), `scripts/check-ci-preflight.mjs` + `scripts/check-ci-preflight.test.mjs` (the contract is updated to lock the new fail-slow semantics), and `package.json` (wire the new test into `check:ci-preflight`). No production code, no migration, no Cargo.toml/lockfile, no OpenAPI, no Buck target.
**Not product authority.** Clears no HOLD and authorizes no production, credential, compliance, payment, erase, OCI, frontend, projection, or Oyatie action.

## Summary

- **One sweep, one commit, one push.** Every CI run now surfaces ALL of fmt, clippy, every gate, and every test binary in a single run, so a lane fixes everything at once instead of polling micro-commits.
- **Step-level fail-slow in `preflight` and `backend`.** Independent steps carry `if: ${{ !cancelled() }}`; steps that depend on a setup root carry `if: ${{ !cancelled() && steps.<id>.outcome == 'success' }}`. The dependency roots are `checkout` / `setup-node` / `npm-ci` / `dotslash` / `rust-toolchain` in preflight, and `topology` (the reconcile step) in backend. When `npm ci` fails the ~20 npm gates skip instead of cascading 20 reds; when topology reconcile fails the DB-dependent backend steps skip.
- **`collect-failures` closes each of the two jobs.** It reads `${{ toJSON(steps) }}` via `scripts/ci-collect-failures.mjs`, prints the `outcome == "failure"` step ids, and exits 1, so the job-level red that feeds `Required / CI` is preserved and the root failure is named.
- **`cargo_needs_postgres.sh` gains an explicit keep-going mode.** Default `--keep-going` (workflow), opt-out `--fail-fast` (local). The per-binary loop moved to `tools/ci/cargo-test-runner.sh` so it is unit-testable without Docker (fake map + stubbed cargo), and now prints a per-binary PASS/FAIL summary table before exiting 1 on any failure.
- **`domain-unit`'s inline block no longer aborts at the first failing binary.** It keeps `set -e` out of the loop, captures each cargo invocation's status, prints a summary, and exits 1 if any failed — while keeping every `cargo test` invocation text byte-for-byte so `check-executed-tests` still attributes every binary (the `ci-keep-going:` contract in `scripts/lib/ci-workflow-executables.mjs` makes `set +e` non-disqualifying because the summary re-raises).
- **The CI-preflight contract is updated, not weakened.** `scripts/check-ci-preflight.mjs` re-locks the new step ids, conditions, and the two `collect-failures` steps (including their `env` and `working-directory`), the new `domain-unit` run digest, and the backend's two fail-slow condition families; the exhaustive bypass-mutation matrix is re-armed at the new step counts.

## Verification

Executed on the rebased worktree at base `e21c04af2` (node v24.16.0, cargo/rustc 1.97.1). Rows marked **(ceremony)** are the exact commands recorded for the conductor.

| Command | Result |
|---|---|
| `node --test tools/ci/cargo-test-runner.test.mjs` | 3 passed / 0 failed — keep-going runs every invocation, `--fail-fast` aborts at the first failure, exit 0 only when all pass |
| `node --test scripts/check-ci-preflight.test.mjs` | 57 passed / 0 failed |
| `node scripts/check-ci-preflight.mjs` | `CI preflight contract passed.` |
| `node --test scripts/verify.test.mjs` | 13 passed / 0 failed (local mirror covers the new `collect-failures` step) |
| `npm run check:ci-preflight` | exit 0 (check-ci-preflight + tools/ci tests + dark-suite `--strict` + foundation + mail-relay + verification-queue) |
| `node tools/ci/check-mjs-dark-suites.mjs --strict` | `dark_count: 0` |
| `npm run test:js-test-reachability` | 2 passed / 0 failed |
| `node scripts/check-executed-tests.mjs` | green — 364 defined, 364 reachable, 1 dark (`seaweedfs_worm.rs`, baseline-pinned) |
| `node scripts/check-js-test-reachability.mjs` | green (baseline + live candidate) |
| `node -e "… yaml.load(ci.yml)"` | YAML parses (js-yaml) |
| `actionlint .github/workflows/ci.yml` | clean |
| `npm run verify` (fast tier) | all mirrored Rust/gate/test suites pass; exits 1 only on two expected uncommitted/unsigned artifacts — `Cheap Buck2 generated-face admission` (package.json digest vs `git archive HEAD`, because the change is staged not committed) and `Console truth-ledger exact-M admission` (no signed C commit yet). Both resolve after the conductor commits + signs the C/T train. |
| `git diff --check` | clean |

## Operational receipt (lane-specific)

- **Lane:** ci/fail-slow-one-sweep-pr0 · **Worktree:** `/Users/jasonlee/Developer/console-pr0-fail-slow` · **Owner:** Jason Lee.
- **Pre-mortem:** a step-guard typo silently skips a gate (red becomes grey with no job red); the keep-going `set +e` makes `check-executed-tests` stop attributing a binary and the ratchet goes dark; the `collect-failures` `toJSON(steps)` leaks a step output or misreads `skipped` as a failure.
- **Blast radius:** `.github/workflows/*`, `tools/ci/*`, `scripts/check-ci-preflight.mjs` + test, `scripts/lib/ci-workflow-executables.mjs`, `scripts/ci-collect-failures.mjs`, `package.json`. Revert = re-merge the previous train; no ruleset edit, no `required-ci` `needs:` change.
- **Detection:** `check-ci-preflight.mjs` (exact step id/condition/digest/order lock + the exhaustive mutation matrix), `check-executed-tests.mjs` (the dark-set ratchet still sees every binary), `check-mjs-dark-suites.mjs --strict`, and `cargo-test-runner.test.mjs`.
- **Rollback:** revert the C then T commits; base `e21c04af2` introduces no migration from this lane.
- **Stop conditions:** any required check red on the merge ref; a `check-executed-tests` dark-set drift; an un-resolved review thread; loss of the pinned signing authority.
- **Review identities:** lane owner + signing principal `Jason Lee` / `jason19931225@gmail.com` (ED25519 `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`); Codex connector automated review (PR #784, login `chatgpt-codex-connector[bot]`); conductor ratification at merge (recorded in the post-merge readback).
- **Head SHA at freeze:** recorded in the post-merge readback update (self-reference).

## Freeze status

**NOT FROZEN YET.** This tip freezes in the post-merge readback update after the hosted required checks (`Required / CI`, `Required / Security`, `authenticate-console-authority`) report green on the merge ref.

## Remaining HOLDs / follow-ups

- All current PRODUCT/ROADMAP HOLDs remain unchanged.
- D1 path-phased lanes, D2 warm-cache program, and D3 duplicate/buck2 retirement remain queued per `.tmp/lane-packets/ci-overhaul.md`; this PR is D4 only.
- Rebasing onto #773 reconciled the backend `Checkout` step (kept #773's unconditional index-0 checkout, added `id: checkout`, dropped the `!cancelled() && run_heavy` guard); #768's skip-proof working-directory note is preserved.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "standard",
  "risk_domains": [],
  "selected_lenses": [
    "Cartesian doubt",
    "Essentialism / YAGNI",
    "Chesterton's Fence",
    "Red Team",
    "Systems Thinking",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first"
  ],
  "task_fit": {
    "Cartesian doubt": "Re-checked the load-bearing claim that check-executed-tests must keep attributing every binary after the keep-going restructure, by running it (362 defined / 362 reachable) rather than assuming the `set +e` restructure was parser-transparent.",
    "Essentialism / YAGNI": "Scoped to D4 only: step guards, two collect-failures steps, the keep-going harness, and the contract update — no path-phased lanes, no warm-cache program, no buck2 retirement, no ruleset edit.",
    "Chesterton's Fence": "The fail-fast lock in check-ci-preflight.mjs existed because a skipped/failed step is a false-green surface; the change preserves that intent by re-locking the new fail-slow semantics rather than deleting the ratchet.",
    "Red Team": "Modeled the `set +e` false-green (a failing cargo test swallowed with no re-raise) and closed it by requiring the summary `exit 1` and by making the keep-going contract explicit in the parser; modeled the collect-failures skipped-vs-failed misread and keyed it on `outcome === 'failure'`.",
    "Systems Thinking": "Traced the two dependency DAGs (preflight: checkout → npm-ci → gates; backend: topology → DB steps) so one root failure skips its dependents instead of cascading reds, and the summary step still preserves the job-level red.",
    "Operability / Day-2": "The collect-failures step and the summary tables make the sweep observable in one look; the keep-going loop is extracted into a unit-testable script instead of staying inline-only.",
    "Blast-radius / cell-based": "Contained failures per job: a failing fmt/clippy/gate no longer masks the rest, and a failing topology reconcile skips only its DB dependents; the change is revertible as one train.",
    "Telemetry-first": "collect-failures prints the exact failing step ids, cargo-test-runner prints a per-binary PASS/FAIL table, and both feed the existing required-check red rather than a separate signal."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "preflight and backend were fail-fast at the step level: one red step skipped every later step, so a lane needed one commit and one push per failure class.",
    "cargo_needs_postgres.sh and the domain-unit block aborted at the first failing binary, hiding later binaries in the same shard.",
    "The check-ci-preflight contract had locked the exact fail-fast conditions, so the fail-slow sweep had to re-lock the contract (ids, conditions, digests, and the two new collect-failures steps) in the same change.",
    "A literal `set +e` in a step would make check-executed-tests stop attributing its cargo runs; the keep-going contract in the parser re-establishes attribution because the summary re-raises failures."
  ],
  "decisions_changed_or_rejected": [
    "Rejected adding workflow-level `paths:` filters and renaming job display names: both would un-require required contexts.",
    "Rejected a comment-only keep-going marker with no enforcement: check-ci-preflight now requires the `ci-keep-going:` contract AND the summary `exit 1`.",
    "Rejected moving the domain-unit cargo list into a separate map/script: the spec requires the inline block with verbatim invocation text so check-executed-tests keeps attributing every binary.",
    "Rejected a per-dependent-step marker echo: the skip is the GHA skipped state and collect-failures names the root failure, avoiding 20 marker steps."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
