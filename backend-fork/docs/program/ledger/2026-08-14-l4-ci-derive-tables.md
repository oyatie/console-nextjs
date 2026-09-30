# Authority tip — L4-CI-DERIVE-TABLES: census scope derived from the topology script

**Date:** 2026-08-14
**Kind:** authority tip ledger bound on candidate C; T adds this ledger entry only
**Base:** `f0f8c1d63b04bca9f260c6e7238c2590b3cc1b51` (origin/main, post #778 rebase)
**Candidate C and Tip T:** both signed by the pinned authority (principal `jason19931225@gmail.com`, ED25519 `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`). Commit/tree SHAs are recorded in the post-merge readback ledger update (T cannot name its own SHA).
**Scope:** `backend/ci/gates/writer-ownership/tests/census_executes_against_postgres.rs` only. No production code, no migration, no CI wiring change.
**Not product authority.** Clears no HOLD and authorizes no production, credential, compliance, payment, erase, OCI, frontend, projection, or Oyatie action.

## Summary

- The census scope is now DERIVED at runtime from the `required_tables CONSTANT TEXT[] := ARRAY[...]` statement in `ops/postgres-reconcile-topology.sh`, scoped to that canonical SQL block via its BEGIN/END markers instead of a hand-maintained fixed-arity `const REQUIRED_TABLES: [&str; 20]`.
- The 20-name list survives as `EXPECTED_REQUIRED_TABLES` — a verbatim pin inside the census test, not a second production source. The new no-Docker unit test `derived_required_tables_match_the_verbatim_roster` fails the run whenever the shell array and the pin diverge, so a lane landing a new canonical table edits two places (shell first, then the pin) and the census can no longer silently shrink its scope.
- RED baseline recorded: compile error `required_tables_from_topology not found in this scope` (E0425) before the function existed.

## Verification

Every Cargo command below is runnable from the repository root as `cd backend && …`. Environment: `CARGO_TARGET_DIR=target` `SQLX_OFFLINE=true`.

- `cd backend && CARGO_TARGET_DIR=target SQLX_OFFLINE=true cargo fmt -p console-gate-writer-ownership -- --check`: clean.
- `cd backend && CARGO_TARGET_DIR=target SQLX_OFFLINE=true cargo clippy -p console-gate-writer-ownership --all-targets -- -D warnings`: clean (collapsible-if findings fixed with let-chains).
- `cd backend && CARGO_TARGET_DIR=target SQLX_OFFLINE=true cargo test -p console-gate-writer-ownership --test census_executes_against_postgres required_tables`: 20 passed / 3 filtered (the verbatim pin + the nineteen decoy unit tests; the 3 Docker-required tests run in CI). The full 23-test binary runs in CI.

Documentation-authority suite (`docs/current/DELIVERY.md:25-27`), executed at the DAG-reachable candidate tree — the tree of this entry's parent commit C (`git rev-parse <T>^{tree}`) — on node v22.14.0 at the pinned rustc 1.97.1; commit SHAs land in the post-merge readback update.

| Command | Result |
|---|---|
| `node --test scripts/check-doc-links.test.mjs` | 36 passed / 0 failed |
| `npm run check:doc-links` | OK (446 markdown files) |
| `npm run test:adrs` | 29 passed / 0 failed |
| `npm run check:adrs` | 39 ADRs, 6 design notes |
| `npm run check:doc-citations` | ecosystem-plan: 679 citations, RESOLVES 534, FILE-ONLY 126, MISSING 19 (not fatal), UNVERIFIABLE 0, BROKEN 0; false-green-gate-holes: 38 citations, RESOLVES 14, FILE-ONLY 24, MISSING 0, UNVERIFIABLE 0, BROKEN 0 |
| `node --test scripts/check-foundation-gates.test.mjs` | 6 passed / 0 failed |
| `npm run check:foundation-gates` | 135 checks passed |
| `node --test scripts/check-ci-preflight.test.mjs` | 57 passed / 0 failed (the earlier `cargo generate-lockfile` 101 fixture failure was rustc 1.83.0-specific; this runner carries the pinned 1.97.1) |
| `npm run check:ci-preflight` | PASS (`CI preflight contract passed.`), including nested foundation 6/6, mail-relay 7/7, verification-queue 17/17 |
| `npm run test:verify` | 13 passed / 0 failed |
| `npm run check:doc-manifest` | OK (446 markdown files) |
| `npm run check:reasoning-lens-contract` | OK (mode=structural, 65 evidence blocks) |
| `node scripts/check-reasoning-lens-contract.mjs --changed-since origin/main` | OK (mode=changed-since base=`f0f8c1d63b04bca9f260c6e7238c2590b3cc1b51`, 65 evidence blocks) |
| `npm run test:reasoning-lens-contract` | 40 passed / 0 failed |
| `git diff --check origin/main...HEAD` | clean |

Changed-path allowlist vs `origin/main` (`f0f8c1d63b04bca9f260c6e7238c2590b3cc1b51`): `backend/ci/gates/writer-ownership/tests/census_executes_against_postgres.rs`, `docs/program/executed-tests-baseline.json`, `docs/program/ledger/2026-08-14-l4-ci-derive-tables.md`, `docs/documentation-index.json`, `docs/documentation-manifest.seed.json`.

Gaps (not claimed green): the full `npm run verify` fast tier did not complete on this workstation — it ran green through `clippy -D warnings` (foundation gates, reasoning-lens, CI-preflight, authority-train, lane-receipt validator, executed-tests baseline, rustfmt, and clippy all PASS), then the `Layer-boundary gate` `buck2 test //backend/app:console-app-unit` build is slow under multi-lane buck2 contention on this shared machine. The full fast tier is executed authoritatively by the hosted CI runner (pinned toolchain). Docker census execution remains CI-only.

## Operational receipt (lane-specific)

- **Lane:** lane-1qw5-derive-tables · **Worktree:** `.worktrees/lane-1qw5` · **Owner:** Jason Lee.
- **Pre-mortem:** the change re-sources a gate scope from a shell script; modeled failure = a decoy marker or `ARRAY[` declaration earlier in the script hijacking the parse, or the census scope silently shrinking.
- **Detection:** the nineteen decoy unit tests (a decoy `ARRAY[` before the `DO $canonical$` block; a decoy declaration parked in a block comment after the real array; an old/example block parked in a block comment before the live block; a literal moved into a block comment inside the array; an entry inside a nested block comment; a declaration parked inside a dollar-quoted literal; a decoy block parked in a non-psql heredoc; `'name'` text parked inside a dollar-quoted literal in the array; a shadowing declaration in a nested block; a declaration parked inside an `E'...'` escape string; an example block parked inside a dollar-quoted SQL value; a decoy block in a `cat <<'SQL'` heredoc; a `CASE` expression entry rejected fail-closed; an `echo psql <<'SQL'` heredoc; an earlier unused `DO $canonical$` block; a commented `unnest(required_tables)` mention; a `cat <<'DOC'` data heredoc containing psql text; a `backup_required_tables` identifier suffix before the real declaration; and a double-quoted identifier embedding the declaration — each returns only the live block's entry or fails loudly), `derived_required_tables_match_the_verbatim_roster`, and the executed-tests baseline ratchet.
- **Rollback:** revert the squash on main; the parser is test-local with no production runtime surface.
- **Stop conditions:** any required check red on the merge ref; unresolved review threads; loss of the pinned signing authority.
- **Review identities:** 34 `chatgpt-codex-connector` review threads resolved 2026-08-14 (severity mixed P1/P2; 31 prior fixed/evidenced, 1 identifier-boundary finding fixed with a regression test, 2 esoteric escalations — unreachable-enforcement and heredoc-terminator — deferred as ownerLease gaps); owner merge sign-off.
- **Head SHA at freeze:** recorded in the post-merge readback update (self-reference).

## Out-of-scope gaps (deferred decoy-scenario escalations)

Unproven decoy-scenario escalations beyond the parser's proven scope are recorded here (one line per finding id) as minor/ownerLease deferrals, not implemented. A failing test or a red required gate is a proven blocker; a new unproven bot opinion is not.

- `PRRT_kwDOS636Ss6Zbk4E` (P2, unreachable-enforcement-reference): deferred, not implemented. Block selection is already bound to the enforcing `DO $canonical$` block via a LIVE-SQL `unnest(required_tables)` match (two prior fixes: enforcing-block binding and live-SQL code-context matching). The production script carries exactly one canonical block with one enforcement reference (`ops/postgres-reconcile-topology.sh:791-1104`, reference at `:957`). The scenario requires an adversarial LATER helper block whose `unnest(required_tables)` sits only in an unreachable `IF false THEN` branch, which a test-local static ratchet cannot distinguish without a full PL/pgSQL control-flow/reachability analysis — out of scope for this parser. Per the critic anti-treadmill clause (`.cursor/agents/lane-critic.md`): same class appearing again → demand mechanism replacement, not another patch; an unproven bot opinion is filed as minor/ownerLease, not a merge bar.

- `PRRT_kwDOS636Ss6ZcxGG` (P2, heredoc-terminator-shell-semantics): deferred, not implemented. The `.trim()` terminator match is the fifth heredoc-discovery finding in this cycle (after non-psql heredoc, data-heredoc body skip, `echo psql` rejection, and psql COMMAND-word matching) — the same class of piecemeal shell-heredoc patch. The production script carries its `<<'SQL'` terminator at column zero, so the described scenario requires an adversarial data heredoc with an indented delimiter the live script never contains. Per the anti-treadmill clause, the correct next step for this class is a MECHANISM REPLACEMENT (a real shell-heredoc parser, or folding the static ratchet into the Docker census that already executes the script as ground truth), not another terminator patch — filed as ownerLease, not a merge bar.

## Freeze status

**NOT FROZEN YET.** The two surfaces that previously could not complete locally are now resolved on this runner (pinned rustc 1.97.1 with buck2/dotslash on `PATH`): the ci-preflight test file records 57/57 and the fast tier runs green through `clippy -D warnings`; only the slow buck2 app-unit suite and the Docker census complete on the hosted CI runner. This authority tip freezes only in the post-merge readback update after the hosted required checks pass — per this lane's own stop condition.

## Remaining HOLDs / follow-ups

- All current PRODUCT/ROADMAP HOLDs remain unchanged.
- `console-1qw.4` (L4-EMPL-RETARGET) remains open; the P4 epic closes after both 1qw.4 and this lane merge.
- Docker census execution on this workstation is unavailable; CI is the executing evidence.

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
    "Systems Thinking",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Separated what the census scope WAS (two hand-maintained copies of one set) from what it must be (one production source plus one verbatim ratchet), and re-checked the derivation against the actual script bytes.",
    "Essentialism / YAGNI": "Runtime parsing of the existing marker block with std-only string handling — no build.rs, no generated file, no new dependency, no lockfile churn.",
    "Chesterton's Fence": "Kept the verbatim-pin idiom this repository already uses (e.g. thirteen_dispatch_targets_verbatim): the pin is the behavioral regression lock, the shell array is the single production source.",
    "Red Team": "Modeled the failure that motivated the change — a rename silently shrinking the examined scope while a count assertion stayed green — and added the marker-missing and empty-extraction failure modes so the parser fails loudly rather than examining nothing.",
    "Systems Thinking": "Traced the collision the bead found: four port lanes hand-appending the same set to two shared files; the derivation removes the census half of that collision.",
    "Operability / Day-2": "The derivation runs in the same test binary that already locates the topology script; no new runtime surface, and the failure messages name the script path and the missing marker.",
    "Blast-radius / cell-based": "One test file changed; no production or CI surface touched; the Docker-required census behavior is unchanged and still fails closed without Docker.",
    "Zero-trust / defense-in-depth": "The derived scope is compared against the verbatim pin on every run — neither the script nor the pin is trusted alone; divergence fails the census instead of defaulting open."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "The census scope and the topology script's required_tables array were two hand-maintained copies of one set, and a fixed-arity array whose arity had to be bumped on every canonical-table migration.",
    "A rename once shrank the examined scope from eight tables to seven while the run still reported success — the count assertion could not see the shrink."
  ],
  "decisions_changed_or_rejected": [
    "Rejected a build.rs-generated const file: it would add a generated-file serialization point and build-time machinery for a set the test can read directly from its already-located script.",
    "Rejected removing the verbatim pin: the pin is the ratchet that refuses drift, consistent with the repository's existing verbatim idioms."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
