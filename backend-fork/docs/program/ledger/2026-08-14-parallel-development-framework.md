# Authority tip — parallel development framework lane

**Date:** 2026-08-14
**Kind:** authority tip ledger bound on candidate C; T adds this ledger entry only
**Base:** `7fc04e3e4167817f6ebc497cb5b329185c2eed5c` (origin/main, PR #782)
**Candidate C and Tip T:** both signed by the pinned authority (principal `jason19931225@gmail.com`, ED25519 `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`). Candidate C is the DIRECT single-parent commit of T on `origin/framework/parallel-lanes-20260814` (not the base); the C..T diff contains ONLY this ledger entry (1 path), while the full product/custody diff (workflows, scripts, manifests, graph, spec) lives in C, i.e. base..C. The complete reviewed tree is T's tree — C's product/custody bytes plus this ledger entry — reachable via `git rev-parse <T>^{tree}`. No concrete C/T/tree SHA is frozen in this entry because every re-sign re-creates those identities (a SHA frozen here would dangle after the next train); the exact identities are bound structurally and recorded in the post-merge readback ledger update: T cannot name its own SHA (its bytes are the last to freeze) and C's custody manifest names T's blob, so T/S identities exist only once the merge completes — recorded then, per DELIVERY.md's readback step. The post-merge squash commit S (GitHub's single-parent squash) is unsigned by design; the signed authority is C+T, verified by the authenticate-console-authority check (C..T = only this ledger entry, both signed) and bound post-merge by the squash-binding step (S tree == T tree).
**Review:** 158 `chatgpt-codex-connector` review threads across the review rounds on 2026-08-14 (91 P1 / 67 P2), every one fixed or recorded as an explicit gap, 0 unresolved at freeze. Owner (Jason Lee) signs off via merge.
**Scope:** the framework lane for the remaining Console program: `docs/specs/parallel-development-framework.md` (quarry), its custody registration, the `.grok/harness/work-graph.v1.json` process extension, and the Wave A1 stale-stream measurement.
**Not product authority.** Clears no HOLD and authorizes no production, credential, compliance, payment, erase, OCI, frontend, projection, or Oyatie action.

## Summary

- Added a quarry-marked framework spec codifying the remaining-program wave plan (A: P4 stale-stream reconciliation; B: P5 canonical containment; C: CI residuals; D: 공휴일 reference table; E: P6 §7 tracker; P∞), Bun-derived cargo-budget and git-discipline rules, the PRODUCT-boundary scope filter, failure modes, and acceptance criteria.
- Registered the spec in `docs/documentation-manifest.seed.json`; regenerated `docs/documentation-index.json`. Evidence: `node scripts/console/generate-documentation-manifest.mjs --check` → "documentation manifest OK (452 markdown files)"; `git diff --check` clean.
- Wave A1 measurement (recorded on bead `console-mrqv`): origin/main `7fc04e3e4167817f6ebc497cb5b329185c2eed5c` is 46 commits ahead of the stale stream's merge-base `ede052d3d`; two-dot delta of the branch = 75 files (+18868/−245); the stale ledger doc is already on main under the same path. The checkout's true uncommitted residue is 6 tracked files (`backend/Cargo.lock`, the writer-ownership gate `lib.rs`, migrations 0213–0215) plus untracked custody candidates; per-file residue review is the Wave A owner's task under clean worktree resolution. **Decision direction: preserve-then-reconcile** — the stale checkout's exact bytes stay untouched as historical evidence; reconciliation proceeds in a separate bounded worktree; no destructive step is prescribed. `console-1qw.4`/`console-1qw.5` landed via #775/#774; only the 일정 design residue remains for Wave A.
- Verification recorded (doc-authority + CI-contract candidate, DELIVERY.md:25-27): exact invocations, discovered/executed counts, revision, toolchain, and validation gaps are in **Verification** below. There is no separate lane admission record for this candidate.
- Fixed a CI defect surfaced by this PR: the backend job ran its `Path-class skip proof` step before `Checkout` while inheriting the job's `working-directory: backend` default, so every docs-only PR failed at job launch (bash could not start because `backend/` did not exist yet). The fix makes `Checkout` unconditional and moves it before the proof step, with the matching backend contract amendment in `scripts/check-ci-preflight.mjs` (`actionStep(0, "Checkout", ...)`).
- Process extension: `.grok/harness/work-graph.v1.json` custodied in-repo (43 nodes / 83 edges, validated, all nodes registered in their phase lists, top-level `tip_serial_paths` matches the implemented inventory including the `.github/workflows/` root, `.github/actions/`, `.github/trust/console.allowed_signers`, `backend/Cargo.toml`, `backend/.sqlx/`, migrations and openapi directories, and `memory.framework_v1.base_sha` is pinned to the declared base `7fc04e3e4…`, `transport` rebound to harness subagents + bd with `mm-role` recorded as a workstation-local gap, `L4-CLOSE` explicitly gates `L5-CLOSE` on the P4-epic close, `L5-CLOSE` gates the containment lanes, `L5-KP0` depends on `L1-MIG-PARSE` and is marked `tip_serial` because it introduces an additive migration; the obsolete `L4-STALE-RECONC → containment/KP0/P5-close` edges were removed so the top-level `edges` array exactly matches every node's `depends_on`). The mechanical classifier in `tools/ci/assess-tip-contention.mjs` now glob-matches `backend/**/migrations/` and `backend/**/openapi/` (a real `**` matcher, not literal `startsWith`), serializes the whole `.github/workflows/` root, `.github/actions/`, and `.github/trust/console.allowed_signers` (not just `ci.yml`), and carries `backend/Cargo.toml` and `backend/.sqlx/` in `TIP_SERIAL_PATH_PREFIXES` and additionally serializes `docs/program/console-capability-registry.json`, `docs/program/console-enterprise-roadmap.md`, the registered generated BUCK faces (`backend/app/**/BUCK`, `backend/crates/**/BUCK`, `backend/ci/**/BUCK`, `third-party/rust/BUCK`), and the Reindeer lockfiles (`third-party/rust/reindeer/Cargo.lock`, `third-party/rust/reindeer/upstream.lock`) and the always-full CI inputs (`release-please-config.json`, `renovate.json5`, `backend/deny.toml`, `backend/rust-toolchain.toml`, `security/`) so migration, contract, CI-workflow, composite-action, signing-policy, SQLx-cache, generated-face, reindeer-bootstrap, and workspace-manifest lanes cannot evade the serial queue. The spec's cargo-rule tip-serial inventory (`docs/specs/parallel-development-framework.md`, rule 3) now lists the same full set so the human-facing guide matches the executable controls. The `.cursor/agents/lane-critic.md` definition gains a push ban: reviewers file threads and never push to PR branches, so the pinned-authority train head is owned by the conductor alone. Its `select_helper` is rebound to `bd ready` + `lane-board.live.json`: the `tools/ci/console-graph-ready.mjs` selector is DEFERRED to Wave C with its fail-closed allowlist, blocked-lane, beads-status, missing-dependency, verification-contract, P∞-default-set, and parallelism-ceiling fixes (7 review threads recorded). Beads epics/children: `console-mrqv`, `console-wrq3` (+4 lanes), `console-7thr`, `console-2qf5`, `console-j3x7`, `console-sv28`. `console-mrqv`, `console-wrq3` (+4 lanes), `console-7thr`, `console-2qf5`, `console-j3x7`, `console-sv28`.
- Incident (tracked as `console-8sgr`): the bd post-checkout hook wrote a stray `core.worktree` into the shared `.git/config` and clobbered 7 uncommitted files in the `lane-full-console-g002-s0b` worktree; shared config corrected; g002's committed work (`24ac6715c`, local-only) is intact.

## Verification

Executed on the complete reviewed tree: T's tree — C's product/custody bytes plus this ledger entry — reachable via `git rev-parse <T>^{tree}` (no stale SHA names are recorded here — every round's bot rewrite had left superseded identities); the re-run results in the table below were produced on that complete content. Commit SHAs are recorded in the post-merge readback update.

| Command | Result |
|---|---|
| `node --test scripts/check-doc-links.test.mjs` | 36 discovered, 36 executed, 36 pass, 0 fail |
| `npm run check:doc-links` | PASS — `doc links OK (452 markdown files)` |
| `npm run test:adrs` | 29 discovered, 29 executed, 29 pass, 0 fail |
| `npm run check:adrs` | PASS — `39 ADRs, 6 design notes` |
| `npm run check:doc-citations` | `docs/ideas/ecosystem-plan-DRAFT.md`: 679 citations, MISSING 19 (not fatal), UNVERIFIABLE 0; `docs/program/false-green-gate-holes.md`: 38 citations, MISSING 0, UNVERIFIABLE 0 |
| `npm run test:foundation-gates` | 6 discovered, 6 executed, 6 pass, 0 fail |
| `npm run check:foundation-gates` | PASS — 135 checks |
| `node --test scripts/check-ci-preflight.test.mjs` | 57 discovered, 57 executed, 56 pass, 1 fail: `rejects a dependency missing from Cargo.lock while the clean lock passes` — `cargo generate-lockfile` status 101 because cargo 1.83.0 cannot stabilize edition 2024 |
| `npm run check:ci-preflight` | PASS — gate `CI preflight contract passed`; 9-file batch 40/40; `check-foundation-gates.test.mjs` 6/6; `check-non-oci-mail-imessage-relay.test.mjs` 7/7; `run-verification-queue.test.mjs` 17/17 |
| `npm run test:verify` | 13 discovered, 13 executed, 13 pass, 0 fail |
| `npm run check:doc-manifest` | PASS — `documentation manifest OK (452 markdown files)` |
| `npm run check:reasoning-lens-contract` | PASS — 70 evidence blocks |
| `npm run test:reasoning-lens-contract` | 40 discovered, 40 executed, 40 pass, 0 fail |
| `npm run check:console-truth-ledger` | `STRUCTURALLY_VALID_HOLD_PRESERVED`, `capability_count` 27, `candidate_sha` recorded in the post-merge readback (the ledger cannot self-embed its own C SHA) |
| `npm run verify` | not green in this worktree — first step `Cheap Buck2 generated-face admission` failed (`dotslash` missing); `Cargo.lock consistency` and `check:executed-tests` failed (`backend/Cargo.toml` needs edition 2024 / rustc 1.97.1); CI preflight contract tests 56/57 as above. Not treated as green aggregate evidence. Hosted Required CI on this PR remains the remaining aggregate surface. |
| `git diff --check` | clean |

## Operational receipt (lane-specific)

- **Lane:** framework/parallel-lanes-20260814 · **Worktree:** `.worktrees/framework-parallel-lanes` · **Owner:** Jason Lee.
- **Pre-mortem:** docs-only PRs fail at backend job launch if `Path-class skip proof` runs before `Checkout` while the job default is `working-directory: backend` — bash cannot start because `backend/` does not exist yet (the defect this candidate repairs). The rest of the lane is additive quarry spec + in-repo process graph + beads; other failure modes modeled are review-loop nonconvergence, unsigned branch pushes, and train-structure mistakes.
- **Blast radius:** backend job step order in `.github/workflows/ci.yml` plus the matching `scripts/check-ci-preflight.mjs` pin (`actionStep(0, "Checkout", ...)` with no `if`); quarry spec, in-repo work-graph, documentation manifests, and this ledger. No product code, migrations, or OpenAPI.
- **Detection:** `npm run check:ci-preflight` fails closed if backend `Checkout` is not unconditional step 0 or if skip proof precedes it. Live signal: a docs-only PR whose backend job never starts (`working-directory: backend` missing). `authenticate-console-authority` + `check:doc-manifest` + `check:reasoning-lens-contract` remain fail-closed on train/doc/lens regression; the reviewer re-reviews every push.
- **Rollback:** revert the squash on main, or revert the `ci.yml` + `check-ci-preflight.mjs` pairing together. Do not restore skip-proof-before-checkout. The pre-merge branch is preserved; beads are additive.
- **Stop conditions:** backend `Checkout` becomes conditional or is no longer step 0; skip proof runs before checkout while `defaults.run.working-directory` is `backend`; workflow and preflight contract diverge on that step; any required check red on the merge ref; unresolved review threads; loss of the pinned signing authority.
- **Review identities:** chatgpt-codex-connector (157-thread inventory recorded above); independent local re-verification by the owner; merge sign-off by the owner.
- **Head SHA at freeze:** candidate C is this tip's parent commit on `origin/framework/parallel-lanes-20260814`; the complete reviewed tree is this tip's tree, reachable via `git rev-parse <T>^{tree}`; this tip's own commit SHA is recorded in the post-merge readback update (self-reference).

## Remaining HOLDs / follow-ups

- All current PRODUCT/ROADMAP HOLDs remain unchanged (frontend, projection fan-out, live production, Korea compliance, OCI A1 instance, disk wipe).
- Framework PR requires human review and merge via the protected path; post-merge containment readback pending.
- Wave A is preserve-then-reconcile: the stale checkout's exact bytes stay untouched as historical evidence; reconciliation proceeds in a separate bounded worktree. No destructive discard is prescribed.
- `console-1qw.4`/`console-1qw.5` landed via #775/#774; P4 epic close proceeds; P5 epic `console-dgo` closes after P4.

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
    "Shared-nothing / eventual consistency",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Separated the measured object facts (45-commit gap, two-dot delta of 75 files) from worktree-resolution artifacts caused by the stray shared core.worktree, and re-derived every claim from clean measurements.",
    "Essentialism / YAGNI": "Filtered the 66 open beads through the PRODUCT boundary so legacy attendance/comms/ERP/HA items stay quarry instead of being dispatched.",
    "Chesterton's Fence": "Layered Bun's cargo and git rules onto the existing cell-parallel/tip-serial machinery instead of replacing the lane-packet/admit/review gates that already caught past false-greens.",
    "Red Team": "Modeled the failure classes that recurred here (false-green gates, scope drift, unsigned tips) and the bd post-checkout hook incident as an adversary's path to clobber another lane.",
    "Systems Thinking": "Traced wave dependencies (A before B/D, C serial on the CI root) and the tip-serial mutex across ledger, lockfiles, openapi, and ci.yml.",
    "Operability / Day-2": "Lane receipt records checkout-specific pre-mortem (docs-only job launch under working-directory: backend), detection (check-ci-preflight pins unconditional Checkout as backend step 0), rollback (revert ci.yml+preflight pairing together), stop conditions, and a P∞ babysit bead so the framework degrades to a visible queue rather than silent drift.",
    "Blast-radius / cell-based": "Kept every artifact additive and cell-local: one quarry spec, one in-repo work-graph, additive beads; no product code or migrations touched, and the CI fix is scoped to reordering one checkout step plus its one contract entry.",
    "Shared-nothing / eventual consistency": "Lanes stay path-disjoint with base-SHA pinning and contracts-first integration; counters move only when commits land.",
    "Zero-trust / defense-in-depth": "The candidate modifies the Required CI workflow and its executable preflight contract — the boundary that decides what runs and what counts as proof. The fix is the minimal reordering (unconditional Checkout before the skip proof) with the contract amended to match, and the verifier still pins the trusted signer, the pinned digest, and the fail-closed step semantics; no check is weakened or removed."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "The p4/canonical-ports-writer-ownership checkout is a stale partially-superseded stream: origin/main is 46 commits ahead of the merge-base and already carries the writer-ownership gate, canonical hardening, and the openapi fragment modularization under different subjects.",
    "The bd post-checkout hook wrote core.worktree into the shared .git/config and clobbered 7 uncommitted files in the lane-full-console-g002-s0b worktree; its committed work (24ac6715c) is local-only and intact.",
    "The backend job's docs-only skip-proof step ran before Checkout under the job's working-directory: backend default, failing every docs-only PR at job launch (PR #773 was the first to hit it after #770 landed).",
    "Independent review (chatgpt-codex-connector) found 13 threads: shortened base SHA, stale ADR-0030 §7 status (all six conditions are MET on main via #755; the shell restriction is the console-8nq delivery HOLD), CI mislabeled as authority, Wave D communications scope, lane-startup aggregate cargo, narrower-than-implemented tip-serial inventory, non-reproducible work-graph, over-claimed allowlist enforcement, missing Wave D migration lens record, missing Zero-trust lens on the CI boundary, missing exact identities in this ledger, and discard wording risking evidence loss. All were folded into this candidate."
  ],
  "decisions_changed_or_rejected": [
    "Rejected rebasing the stale stream commit-by-commit; measured the two-dot delta instead and adopted preserve-then-reconcile (console-1qw.4/1qw.5 have since landed via #775/#774, leaving only the 일정 design residue).",
    "Rejected adding sccache for lanes until a disk census justifies it; Cargo stays on the CI rust-cache path.",
    "Rejected dismissing the bot review: every thread was either folded into the spec/ledger or recorded as an explicit L∞-PROC gap."
  ],
  "lens_set_changes": [
    "Zero-trust / defense-in-depth added after review found the CI-contract boundary change contradicted the earlier not-applicable exception."
  ]
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
