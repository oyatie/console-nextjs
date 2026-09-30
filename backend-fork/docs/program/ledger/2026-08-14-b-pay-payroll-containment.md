# Authority tip — B-PAY: PayRun conflict refusal + drain period-gate recheck (3yu, ai2)

**Date:** 2026-08-14
**Kind:** authority tip ledger bound on candidate C; T adds this ledger entry only
**Base:** `de8fc6432bd0e24f4033a6ad8e403a644dc87a7e` (origin/main; rebased onto #775 then #776, which merged after round 1)
**Reviewed head (round 1):** `c0978df43b971549544fd6c371f03a7d2af60a4e` — candidate C `f6fee47c9bb1c0a11767da5ea06d5fe056f3ae20` (code tree `595ec19bdee04f4109274c0106047394ce54138d`), the exact artifact under first review (then based on `dea1f91bf`).
**Review:** 16 `chatgpt-codex-connector` (Cursor Agent) review threads across eight rounds on 2026-08-14 — round 1 (head `c0978df43`): 6 (4 ledger P1: bind head + reviewer, correct test-file/wiring evidence, rollback + stop conditions, exact invocations; 1 code P1: serialize period check with lock creation; 1 code P2: duplicate postgres-cargo-map alias); round 2 (head `0938babf6`): 1 code P1 (serialize lock creation with the gated insert — the `NOT EXISTS` alone leaves a snapshot-to-commit gap); round 3 (head `a1a8fc03e`): 1 ledger P1 (stale resolution claim while the lock race was still open); round 4 (head `e53ad5973`): 1 code P1 (serialize the public `POST /api/v1/period-locks` creator too); round 5 (head `c7cef032c`): 3 ledger P1 (reconcile review identities, bind the receipt to the final fixed candidate, run the documentation-authority verification); round 6 (head `e6f8e7c3d`): 1 code P2 (align the `PayrollDraftStaging` port contract and drain docs with provenance refusal); round 7 (head `b35759f8e`): 2 code (1 P1: allow an already-staged draft to be acknowledged after the period is locked; 1 P2: refuse noncanonical stored provenance values); round 8 (head `e9031a00e`): 1 ledger P1 (record the re-run 15 + 1 + 4 payroll integration result). Every thread is fixed in this tip (ledger) or folded into candidate C (code); owner (Jason Lee) signs off via merge.
**Candidate C and Tip T:** both signed by the pinned authority (principal `jason19931225@gmail.com`, ED25519 `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`). Candidate C is re-signed with the review fixes folded in, so its re-signed commit SHA and T's own SHA are recorded in the post-merge readback update (T cannot name its own SHA — its bytes are the last to freeze — and C's custody manifest names T's blob).
**Scope:** `backend/crates/payroll/adapter-postgres/**` (PayRun provenance refusal + the atomic, advisory-lock-serialized staging freeze-window gate), `backend/crates/workflow/adapter-postgres/**` (drain seam + `payroll_drain_period_lock` test), `backend/crates/workflow/domain/**` (`PayrollDraftStaging` contract wording), `backend/crates/platform/db/**` (`lock_period_lock_key` + serialization test), `backend/crates/attendance/adapter-postgres/**` (close_month serializes lock creation), `backend/app/src/lifecycle.rs` (the public `POST /api/v1/period-locks` creator serializes lock creation), `tools/ci/postgres-cargo-map.json` regeneration, `docs/program/executed-tests-baseline.json`. No migration, no grant change, no second writer.
**Not product authority.** Clears no HOLD and authorizes no production, credential, compliance, payment, erase, OCI, frontend, projection, or Oyatie action.

## Summary

- **console-3yu (fail-closed decision):** the PayRun `ON CONFLICT` arm now REFUSES a changed provenance on the same run_id+period (`StageDraftError::ProvenanceMismatch` → `PayRunError::ProvenanceConflict`) instead of returning `created:false` + the stored `draft_run_id` while the requested provenance is never stored. Provenance comparison is on field presence, JSON type, AND value (a missing or non-string stored `connector`/`job` is refused, not normalized to absence), and an already-staged draft with identical provenance is acknowledged (`Ok(false)`) even after the period is locked — the freeze gate applies only to a genuinely new write. The refresh alternative was evaluated and rejected: rewriting provenance on a possibly-progressed row corrupts the audit trail.
- **console-ai2 (atomic + serialized gate):** the drain staging seam re-checks the freeze-window gate ATOMICALLY, folded into the staging INSERT's `WHERE NOT EXISTS (… period_locks …)` so the check and the write share one snapshot, AND serializes that write against period-lock creation through a shared per-org advisory lock (`console.period-lock|payroll|<org>`, [`lock_period_lock_key`]) taken by EVERY production lock-creation path (attendance close_month AND the public `POST /api/v1/period-locks` creator in `backend/app`). The `WHERE NOT EXISTS` closes the phase-1-read→write gap; the advisory lock closes the narrower snapshot→commit gap where a lock commits after the INSERT's READ COMMITTED snapshot but before its commit. A lock committing in either window is refused as `StageDraftError::PeriodLocked` (the event stays PENDING). Holding the lock across both phases was rejected (reintroduces the cross-crate deadlock the split exists to avoid).
- New tests: 2 in `pay_run_port_as_runtime_role.rs` (changed provenance refused; seam refuses a locked period) + 1 atomic-gate test in the same file (a lock committed after the phase-1 read but before the write is refused) + 2 more in the same file (noncanonical stored provenance refused; an existing draft acknowledged after the period is locked) + 1 second test in the pre-existing `payroll_drain_period_lock.rs` (a lock acquired after the read gate still blocks the staging write) + 1 serialization test in `period_locks_and_lifecycle.rs` (the shared advisory lock excludes a concurrent lock creator). The `payroll_drain_period_lock.rs` file, its generated Buck face, and its postgres-cargo-map entry already existed in the parent commit — this change adds tests to existing files and requires no new map entry; the duplicate map alias introduced mid-change is removed per review (see the code thread), so the map regenerates from the existing wrapper.

## Verification

Executed on the reviewed head and re-run on the fixed candidate. Commands run from `backend/` with `CARGO_TARGET_DIR=/Users/jasonlee/Developer/console/.tmp/cargo-target-b-pay SQLX_OFFLINE=true`; DB suites via the repo Docker harness `tools/lanes/pgtest.sh /Users/jasonlee/Developer/console/.worktrees/lane-b-pay <cargo test argv>` (pinned `postgres:18.4@sha256:65f70a152846cf504dff86e807007e9aeac98c3aeb7b62541b2c55ab9d264e56` + reconcile + migrations, CI-equivalent):

- RED baselines executed before the fix: 3yu `unwrap_err() on Ok(CommandReceipt{created:false,...})`; ai2 seam `stage() returned Ok(true)` despite the lock; ai2 drain `left: 1, right: 0` for a draft landing after the lock closed; ai2 atomic `stage_draft_run_in_tx` returned `Ok` for a lock committed between the phase-1 read and the write; ai2 snapshot `NOT EXISTS` still passes a lock that commits after the INSERT's snapshot but before its commit.
- `cargo fmt --all -- --check` — clean.
- `SQLX_OFFLINE=true cargo clippy -p console-payroll-adapter-postgres -p console-workflow-runtime-adapter-postgres -p console-platform-db -p console-attendance-adapter-postgres -p console-app --all-targets -- -D warnings` — clean.
- `cargo test --locked -p console-payroll-adapter-postgres --lib` — 15 passed.
- `tools/lanes/pgtest.sh /Users/jasonlee/Developer/console/.worktrees/lane-b-pay cargo test --locked -p console-payroll-adapter-postgres --test pay_run_port_as_runtime_role --test payroll_lifecycle_rls_as_runtime_role --test payroll_rls_surfaces_as_runtime_role` — 15 + 1 + 4 = 20 passed.
- `tools/lanes/pgtest.sh /Users/jasonlee/Developer/console/.worktrees/lane-b-pay cargo test --locked -p console-workflow-runtime-adapter-postgres --lib --test notification_bridge --test payroll_drain_period_lock` — 1 + 2 + 2 = 5 passed.
- `tools/lanes/pgtest.sh /Users/jasonlee/Developer/console/.worktrees/lane-b-pay cargo test --locked -p console-platform-db --test period_locks_and_lifecycle` — 5 passed (incl. the new serialization test).
- `tools/lanes/pgtest.sh /Users/jasonlee/Developer/console/.worktrees/lane-b-pay cargo test --locked -p console-attendance-adapter-postgres --test concurrency` — 3 passed.
- Payroll total 35 passed (lib 15 + pay_run_port 15 + lifecycle_rls 1 + rls_surfaces 4); workflow total 5 passed (lib 1 + notification_bridge 2 + payroll_drain 2); platform-db `period_locks_and_lifecycle` 5 passed; attendance `concurrency` 3 passed.
- `node tools/ci/check-postgres-cargo-map.mjs` — OK. `node scripts/check-executed-tests.mjs` — green; baseline locked 2626 → 2643 via `--update` (2626 + 7 this lane + 10 from the #775/#776 rebase).
- Documentation-authority checks (DELIVERY.md:23-27): `git diff --check` clean; `npm run check:doc-links` → "doc links OK (442 markdown files)"; `npm run test:adrs` pass + `npm run check:adrs` → "39 ADRs, 6 design notes"; `npm run check:doc-citations` → BROKEN 0 / MISSING 0 / FILE-ONLY 24; `npm run test:foundation-gates` pass + `npm run check:foundation-gates` pass; `npm run check:ci-preflight` pass; `npm run test:verify` pass; `npm run check:doc-manifest` → "documentation manifest OK (442 markdown files)"; `npm run check:reasoning-lens-contract` → 61 evidence blocks; `npm run check:console-truth-ledger` → requires `CONSOLE_CANDIDATE_SHA` (recorded in the post-merge readback, self-reference). `npm run verify` aggregate + writer-ownership/layer-boundary gates (forbidden paths) + `console-app` `m2_real_engine_drive` are the hosted Required CI surface and run there.

## Operational receipt (lane-specific)

- **Lane:** lane-b-pay · **Worktree:** `.worktrees/lane-b-pay` · **Owner:** Jason Lee.
- **Pre-mortem:** (1) a period lock committing between the drain's phase-1 gate read and the phase-2 staging write lands a draft inside a locked payroll window — the SELECT-then-INSERT gap under READ COMMITTED; (2) a changed provenance on the PayRun natural-key conflict is silently absorbed, so a caller can act on a stored `draft_run_id` whose row differs from the request.
- **Blast radius:** payroll/workflow adapter-postgres staging + PayRun conflict arm + the pre-existing `payroll_drain_period_lock` test file + postgres-cargo-map regeneration + executed-tests baseline. No migration, no grant, no gate, no frontend/OCI/projection/Oyatie.
- **Detection:** the regression tests fail closed (changed provenance refused; seam refuses a locked period; lock-after-read-gate refused; lock-after-phase-1-read refused; the shared advisory lock excludes a concurrent lock creator); `cargo clippy -- -D warnings`; `node scripts/check-executed-tests.mjs`; `node tools/ci/check-postgres-cargo-map.mjs`; hosted Required CI/Security + `authenticate-console-authority`.
- **Rollback:** revert the squash on main (revert C + T together). Do not restore the SELECT-then-INSERT split or drop the advisory-lock serialization (both reintroduce the race), and do not restore the duplicate map alias. The pre-merge branch and ledger are preserved.
- **Stop conditions:** the freeze-window gate is moved back out of the staging INSERT or the shared advisory lock is dropped (reintroducing the race); a locked period fails the write OPEN (event acked/dropped instead of staying PENDING); any required check red on the merge ref; unresolved review threads; loss of the pinned signing authority.
- **Review identities:** `chatgpt-codex-connector` 16-thread review across eight rounds (recorded above); independent local re-verification by the owner; merge sign-off by the owner.
- **Head SHA at freeze:** review heads `c0978df43` → `0938babf6` → `a1a8fc03e` → `e53ad5973` → `c7cef032c` → `e6f8e7c3d` → `b35759f8e` → `e9031a00e`; the final candidate C and tip T commit/tree SHAs are recorded in the post-merge readback update (self-reference — C's custody manifest names T's blob, so C/T identities freeze only at merge).

## Freeze status

**NOT FROZEN YET.** The aggregate verify and the gate suites run on the CI runner; this tip freezes in the post-merge readback update after hosted required checks pass.

## Remaining HOLDs / follow-ups

- All current PRODUCT/ROADMAP HOLDs remain unchanged.
- Fail-closed drain leaves a genuinely-changed-provenance outbox event PENDING indefinitely (payroll drain has no dead-letter); recorded ops consideration, intended "never fail forgotten".
- Provenance comparison excludes `outbox_event_id` (correlation id), so a same-connector/job fresh command still absorbs as `created:false` — existing idempotency preserved.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "hr_payroll"
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
    "Cartesian doubt": "Separated the two half-findings of the adversarial review: the refuted atomicity blocker from the standing two-phase gate gap, then re-derived the gap as the SELECT-then-INSERT race under READ COMMITTED and fixed it by folding the gate into the INSERT.",
    "Essentialism / YAGNI": "Folding the gate predicate into the staging INSERT (`WHERE NOT EXISTS`) is the smallest correct fix; holding the lock across both phases was rejected for re-introducing the deadlock the split avoids.",
    "Chesterton's Fence": "The phase split exists to avoid a cross-crate lock hold; the fix keeps the split but makes the write re-check the gate in the same statement instead of undoing the split.",
    "Red Team": "Modeled the caller acting on a draft_run_id whose provenance differs from the request, the lock closing between read and write, the concurrent lock INSERT committing after the gate read but before the staging write, and the lock committing after the INSERT's snapshot but before its commit; all now fail closed with regression tests.",
    "Systems Thinking": "Traced the drain's two-transaction shape and the period-lock primitive across crates before choosing the atomic-gate fix.",
    "Operability / Day-2": "The PENDING-forever residual and the exact rollback/stop conditions are recorded, not hidden; the new tests are wired so they can fail in CI.",
    "Blast-radius / cell-based": "Payroll/workflow adapter family only; no migration, no gate, no CI-authority change.",
    "Zero-trust / defense-in-depth": "Provenance is verified at the storage boundary against the stored row, and the freeze-window gate is verified in the same statement as the write AND serialized against lock creation by a shared per-org advisory lock, refusing divergence and lock races instead of trusting the identifier, the earlier read, or the statement snapshot."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "The ON CONFLICT arm returned created:false plus the stored draft_run_id while a changed provenance was never stored — a caller could act on an identifier whose row differs from the request.",
    "A period lock closing between the drain's phase-1 read gate and phase-2 staging write was not seen by the write under READ COMMITTED, landing drafts into a locked period.",
    "A period lock committing after the gated INSERT's READ COMMITTED snapshot but before its commit is still invisible to the NOT EXISTS (the two inserts touch unrelated rows), so both can commit — the remaining gap the atomic gate alone did not close.",
    "Review found the ledger deferred the candidate/tree SHA and reviewer identity, misstated the test-file/wiring evidence (the file, Buck face, and map entry pre-existed), omitted rollback/stop conditions, and abbreviated the verification invocations."
  ],
  "decisions_changed_or_rejected": [
    "Rejected refreshing source_summary on conflict: rewriting provenance of a possibly-progressed row corrupts the audit trail; refusal is consistent with DigestConflict.",
    "Rejected holding the period lock across both drain phases: reintroduces the cross-crate deadlock the transaction split exists to avoid.",
    "Rejected the separate SELECT-then-INSERT gate recheck: folds the gate into the INSERT so check and write share one snapshot.",
    "Adopted a shared per-org advisory lock on both the gated write and the close-month lock creator to close the snapshot-to-commit gap the NOT EXISTS alone cannot."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
