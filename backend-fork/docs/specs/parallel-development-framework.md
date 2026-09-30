> **QUARRY / NON-AUTHORITY.** Planning artifact for multi-agent dispatch only. Cannot dispatch product work, clear HOLDs, or override `docs/current/{PRODUCT,ROADMAP,DELIVERY}.md`. Executable projections: Beads issues + `.grok/harness/work-graph.v1.json` (custodied in-repo from this candidate onward so fresh checkouts can reproduce the graph; live status stays board+beads).

# Parallel development framework — Bun-derived operating contract for the remaining Console program

**Date:** 2026-08-14 · **Base:** `7fc04e3e4167817f6ebc497cb5b329185c2eed5c` (origin/main) · **Status:** plan — waves dispatch only through the existing admit/review/merge machinery.

## 1. Where we are (measured 2026-08-14)

| Phase | Status |
|---|---|
| P0 unblock, P1 substrate, P2 arch foundations, P3 ontology | NOT CLOSED — still required by `docs/current/ROADMAP.md` (substrate convergence + architecture foundations remain open) |
| P4 owning ports (`console-1qw`, 12/12) | Core wave landed on main via #618/#622/#736/#739/#741/#755; `console-1qw.5` (L4-CI-DERIVE-TABLES) landed via #774, `console-1qw.4` (L4-EMPL-RETARGET) via #775 |
| P5 org→HR→payroll (`console-dgo`, 6/6 children) | "Eligible for close"; edge blocked on open P4 epic |
| P6 Leptos | HOLD. ADR-0030 §7: all six substrate conditions are measured MET on main (the contracts/OpenAPI generation and the aggregate read model — a policy-scoped read path, explicitly not a materialized view — closed via #755). The remaining shell restriction is the `console-8nq` delivery HOLD; the planning-only CI assertion stays until the change that opens the gate |
| P∞ always-on | Open: process lane, HOLD-prep custody |

**Stale-stream finding (Wave A).** The checkout at `p4/canonical-ports-writer-ownership` (HEAD `530eed647`) is a stale, partially-superseded stream: origin/main is 46 commits ahead of the shared merge-base, and the two-dot delta of the branch is 75 files (+18868/−245). Its true uncommitted residue is small — 6 tracked files (`backend/Cargo.lock`, the writer-ownership gate `lib.rs`, migrations 0213–0215) plus untracked custody candidates (`backend/crates/payroll/ui/`, the 2026-08-11 shared-CAS handoff, the 2026-08-06 anti-sprawl ledger) — and origin/main already carries the writer-ownership gate, canonical-port hardening, and the per-crate `openapi/` fragment modularization under different commit subjects. Per the P4 ledger's own lesson ("a rebase against superseded history is the wrong tool"), the stream must be reconciled by **two-dot delta measurement**, not rebase. The checkout itself is **preserved untouched as historical evidence** (never force-deleted); reconciliation proceeds in a separate bounded worktree, and only the genuinely-unique 일정 design residue becomes leaves on origin/main (EMPL-RETARGET and CI-DERIVE already landed via #775/#774).

**Open PRs:** #769 (CAS warm canary; green but BEHIND), #760 (release-please 0.3.7) — #768 already merged (it is the declared base). **Beads:** 66 open (P1: `console-kp0`; in-progress: `console-ry4f`; blocked: `console-8x4`, `console-c236`). **CI on `530eed647`:** all green incl. Required / CI and Required / Security. **Worktrees:** 245 registered; 3 stale registrations pruned; `.worktrees/` ≈ 205 GB; disk free ≈ 2.8 TB.

## 2. What ultragoal and ralplan were (historical method)

- **ultragoal(gjc)** — goal-ledger execution: `.omc/ultragoal/plans/conglomerate-platform/` G001–G015 (original conglomerate plan), then `.omx/ultragoal/` G001–G006 (Cedar/PBAC contract baseline; all complete 2026-07-03; archived). Spec retained at `.grok/harness/ultragoal.v1.json`.
- **ralplan(gjc)** — OMX Planner → Architect → Critic approval loop; produced the ADR corpus ("ralplan iteration 3") and `.omx/plans/*` stage artifacts; its mandatory `preflight` stop condition later caused stalls (see `docs/ideas/delegation-economics.md`).
- Both are **context/history only**. The current program is the phase graph P0–P6 + P∞ in `.grok/harness/work-graph.v1.json`, mirrored in Beads.

## 3. Operating contract (roles, cargo budget, git discipline)

**Roles.** Conductor (one per wave; the only aggregate-builder, tip-serial claimant, merge serializer, bead closer). Implementer per lane (RED test first; targeted single-crate checks only). Two diff-only adversarial reviewers (one tasked to refute; findings carry failure scenarios) + one applier. Human owner counter-signs every receipt; no agent self-approval.

**Cargo rules (the Bun anti-hammering core).**
1. **One aggregate error census per wave**, run by the conductor before fan-out and pre-collected into per-crate files. Lanes run **targeted single-crate checks only**, never aggregate cargo, never cargo inside parallel fix loops (Bun phase D, mapped). Targeted regression comes first; the aggregate `npm run verify` runs once on the completed candidate at admit — exactly DELIVERY.md's order.
2. One lane = one worktree = one `CARGO_TARGET_DIR` (precedent: `.tmp/cargo-target-*`), jobserver-capped.
3. Tip-serial paths are claimed through the conductor, one writer at a time — the implemented inventory from `tools/ci/assess-tip-contention.mjs:22-44`: `docs/documentation-manifest.seed.json`, `docs/documentation-index.json`, `docs/program/executed-tests-baseline.json`, `docs/program/js-test-reachability-baseline.json`, `docs/program/gate-input-exceptions.json`, `docs/program/console-program-ledger.md`, `docs/program/console-capability-registry.json`, `docs/program/console-enterprise-roadmap.md`, `docs/program/ledger/`, `docs/program/console-jurisdiction-register.json`, `docs/CI-GATES.md`, `docs/GO-LIVE-CHECKLIST.md`, `docs/benchmarks/enterprise-parity-matrix.md`, `docs/specs/`, `docs/program/console-fanout-epoch-contract.md`, `docs/program/console-buck2-scale-playbook.md`, `docs/current/`, `.github/workflows/`, `.github/actions/`, `.github/trust/console.allowed_signers`, `scripts/`, `.grok/`, `.claude/workflows/`, `backend/Cargo.toml`, `backend/Cargo.lock`, `backend/.sqlx/`, `package.json`, `package-lock.json`, `tools/buck/`, `.buckconfig`, `**/BUCK`, `**/*.bzl`, `tools/ci/`, `backend/ci/gates/`, `registry/`, `backend/**/migrations/`, `backend/**/openapi/`, `**/BUCK`, `third-party/rust/reindeer.toml`, `third-party/rust/reindeer/`, `security/`, `backend/rust-toolchain.toml`, `backend/deny.toml`, `renovate.json5`, `release-please-config.json`, `.release-please-manifest.json`, `ops/`, `deploy/`, `.config/nextest.toml`.
4. Buck2 NativeLink CAS stays read-only canary (PR #769); Cargo stays on the CI rust-cache path (2026-08-11 handoff §3). sccache for lanes only if the disk census allows — measure, then enable.
5. Hosted CI is **evidence, never authority**: review, protected merge, and post-merge readback remain the gate (DELIVERY.md:11-15). Local runs are evidence too.

**Git rules.** Lanes may only branch from the pinned base in their own worktree, add/commit specific files per batch, and push. No stash/reset/checkout-hopping; force-with-lease is reserved for the integration owner’s restack (console-drive.rhai). Merge-conflict avoidance: path-disjoint allowlists (≤3 owned roots), single-parent leaf commits, base-SHA pinning, two-dot delta measurement before any rebase decision, one integration owner, no multi-tip product PRs, no destructive shared-workspace git operations. **Enforcement honesty:** the mechanical admission layer classifies changed paths and its contention check is nonblocking (`scripts/local-admission.mjs:51-103`, `tools/ci/assess-tip-contention.mjs:163-178` rejects only a BEHIND tip writer) — allowlist denial is currently a conductor/process control, and a fail-closed changed-paths-vs-allowlist gate is a recorded L∞-PROC gap, not a claim of mechanical enforcement.

**Worktree budget.** No new worktree without a reclaimed one; prune stale registrations first; disk census before each wave.

**Stage gates.** RED baseline → targeted gates for the touched surface → aggregate verify at admit → 2 adversarial reviews + independent re-runner → merge via protected path → post-merge containment readback → bead close with evidence (DELIVERY.md exit rule).

## 4. Wave plan (maximally parallel within the safety envelope)

| Wave | Content | Parallelism |
|---|---|---|
| **A** — P4 closure | Reconcile the stale p4 stream against origin/main (two-dot delta, **preserve the checkout untouched as evidence**), land genuine residue as leaves; close P4 epic (`console-1qw.4/.5` already landed via #775/#774) | Serial by necessity (one stale stream); review lanes parallel on disjoint commit groups |
| **B** — P5 containment | Close `console-dgo` with readback evidence; then canonical-bug family, partitioned by file ownership: B-PAY (`3yu`, `ai2`, `r25`), B-EMP (`31e`, `rte`, `fi8`, `2kd`, `0hf`), B-ID (`2v1`, `9sb`, `cg6`, `lx6`), B-HR-REST (`tfg2`; `e8vn` openapi via conductor) | Up to 4 lanes concurrent where file-disjoint; serial merges |
| **C** — CI/contract residuals | `8x4` conflict decision first, then `ss7`, `gwa`, `cae0`, `u4p5`, `37c`, `ry4f`/`c236` | Serial (scripts/, ci.yml, openapi are tip-serial) |
| **D** — 공휴일 reference table | `kp0` design → implementation of the payroll-owned public-holiday reference table only. The 일정-as-projection-over-`collaboration.rs`/`workbench.rs` rewrite is **out of scope** — communications are outside the PRODUCT boundary | After A; concurrent with B only if allowlists are disjoint |
| **E** — P6 recon | Track the `console-8nq` delivery HOLD and custody the ADR-0030 §7 six-green evidence; prepare-only; NO shell | Parallel-safe (planning only) |
| **P∞** | program-tick, babysit #768/#769/#760, failure-class promotion (2-repeat rule), hindsight retain, worktree reclaim | Always on |

**Wave D migration risk record (required before its fan-out, AGENTS.md high-risk set).** Red Team: the reference table must not become a second writer of calendar facts — it is read-only reference data behind the payroll reader, RLS-armed and audited like every canonical relation. Operability / Day-2: additive migration with contiguity + rollback evidence, no destructive DDL. Blast-radius / cell-based: one new table, no shared-file serialization beyond the migration lock. Zero-trust / defense-in-depth: the table grants nothing to `console_rt` beyond the read the payroll writer already holds; privilege scope is verified by the writer-ownership gate.

**Parallelism ceiling:** ≤4 implementation lanes at once, bounded by path-disjointness, reviewer capacity, cargo budget, and human sign-off — not by phase labels.

## 5. Scope filter (PRODUCT boundary)

Dispatchable: phase epics, the canonical bug family, `kp0` (public-holiday reference table only), `tfg2`/`e8vn`, CI residuals. **Quarry (not dispatchable):** attendance/comms/ERP/analytics/HA portability beads and legacy `gh#` items outside the PRODUCT boundary — recorded, never silently dispatched, never deleted.

## 6. Failure modes and stop conditions

Scope drift / allowlist violation → stop the lane; the denial is conductor/process control today, with a fail-closed allowlist gate as a recorded L∞-PROC gap (see §3). False-green → ExactActiveRun work (`ry4f`) stays P∞ priority; every lane's evidence records discovered/executed counts. Review impasse → converge-or-escalate after 2 rounds. Human sign-off unavailable → merge stays blocked (fail closed). Disk pressure → suspend new worktrees, reclaim prunables first. Tip-serial path touched without claim → rejected by the conductor's claim registry.

## 7. Acceptance criteria

P4 epic closed 12/12; P5 epic closed with merged/readback evidence; every lane records its packet, RED baseline, exact gate outputs with counts, 2 signed reviews + owner counter-signature, exact SHAs, remaining HOLDs; main green with post-merge containment; P6 un-mounted (negative acceptance); bead graph and work-graph agree (0 inverted edges); worktree count within budget.
