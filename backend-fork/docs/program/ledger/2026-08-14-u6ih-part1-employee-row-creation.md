# Authority tip — U6IH part-1: employee ROW creation routed off leave_api onto the Employment port

**Date:** 2026-08-14
**Kind:** authority tip ledger bound on candidate C; T adds this ledger entry only
**Base:** `0da6c2fdcc6eadc104527338a4772248f843d43d` (origin/main — post-#779, the immutable candidate diff base). Review-bound base (ADV-782-02): `b9c65ebc9b1c31b424011df43a8a2d849d78b734`. Lane-inception base (pre-rebase): `f9a88ed192fb7c0588c9c6ba16ea64da84f2887d`.
**Candidate C and Tip T:** both signed by the pinned authority (principal `jason19931225@gmail.com`, ED25519 `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`). Commit/tree SHAs are recorded in the post-merge readback ledger update.
**Review:** first independent adversarial review `ADV-782-01` (subagent `9d2d98a4`), bound to head `683145c4f` / base `f9a88ed19`, verdict **REQUEST-CHANGES** — four class-C findings (HIGH-1 orphan row behind 403, HIGH-2 half-closed TOCTOU, MEDIUM-3 test strength, LOW-4 role constants/error variants/telemetry); all four remediated in candidate C + migration `0221`. **Second review `ADV-782-02`** (subagent `9d2d98a4`), bound to code head `13dd2d892` / base `b9c65ebc9`, verdict **APPROVE-WITH-NOTES** — HIGH-1/HIGH-2 verified fixed; MEDIUM-1 (message-map gap) FIXED in the post-notes head; LOW-2 (migration gap 0219/0220) gap-noted; LOW-3 (non-locking assert) gap-noted.
**Scope:** `backend/app/src/hr.rs` (the People & Workforce `create_employee` handler plus the `require_home_branch_command_store`, `assign_home_branch_if_unset`, and `create_employee_core` helpers it shares with the recruiting hire handshake), `backend/app/src/recruiting_hire.rs` (passes the `replayed` flag into the shared post-commit routing boundary), `backend/app/tests/hr_people_create_api.rs`, migration `backend/crates/platform/db/migrations/0221_employee_create_routing_authority.sql`, and the generated `docs/program/executed-tests-baseline.json` (test-attribute ratchet bumped 2→4 for the two new `#[sqlx::test]` fns — `deactivated_super_admin_cannot_create_employee` and `revoked_grant_executive_cannot_create_employee`). No Cargo.toml/Cargo.lock, no OpenAPI, no `ci.yml` change.
**Not product authority.** Clears no HOLD and authorizes no production, credential, compliance, payment, erase, OCI, frontend, projection, or Oyatie action. The first-assignment widening (SUPER_ADMIN-only → org-wide directory-manage) restores the pre-reroute EXECUTIVE+custom-grant base behavior; it grants no capability `leave_api.create_employee` did not already authorize.

## Summary

- The People & Workforce `create_employee` REST handler now writes the employee ROW through `ObjectKey::Employment`'s owner (`create_employee_core` → `employment::insert_employee_record`, executing inside the caller's transaction) instead of `leave_api.create_employee` (the `console_leave_definer` INSERT). The leave command capability is no longer a second writer of `employees` on the create flow.
- Handler order is now: `authorize_hr_org_wide` → `require_home_branch_command_store` (fail-closed, before any write) → `with_audits { create_employee_core + employee_create_audit }` → post-commit `assign_home_branch_if_unset` (first home-branch assignment via the leave command channel).
- Removed the dead `CreateEmployeeCommand` import from the `console_leave_adapter_postgres` use list.
- Test changes: a RED-first assertion that a create yields **exactly one** `employee.home_branch_set` audit (the command channel's own audit). The custom-grant EXECUTIVE scenarios (org-wide and team-matching) assert **201** and that `home_branch_id` is established — the base behavior this lane must not narrow — while a plain EXECUTIVE (no grant), a mismatched or branch-narrowed grant, MEMBER, and ADMIN assert **403** with zero writes. `deactivated_super_admin_cannot_create_employee` and `revoked_grant_executive_cannot_create_employee` pin the live is_active / role / grant recheck.
- RED baseline: pre-1a the audit assertion failed 0 vs 1; intermediate runs captured the 201→403 delta on the custom-grant scenarios.
- Security fix (independent reviewer HIGH-1/HIGH-2, conductor-decided in-lane): `create_employee_core` now restores the full migration-0183 `leave_api.assert_employee_directory_manager` predicate at the top of the transaction, BEFORE any write — an active org member who is SUPER_ADMIN, or SUPER_ADMIN/EXECUTIVE with a live, active, non-branch-narrowed `employee_directory_manage` allow grant — reading `users` and `user_role_assignments`/`policy_roles`/`policy_role_permissions`/`policy_role_conditions` under `FOR UPDATE` (the condition rows are additionally locked), so a deactivated, role-revoked, or grant-revoked actor with an unexpired token cannot write. Migration `0221` adds a DEDICATED create-path function `leave_api.set_employee_home_branch_create` (the general `leave_api.set_employee_home_branch` route function is UNTOUCHED and keeps `assert_org_admin` for arbitrary first assignments). The create function is create-scoped by SERVER-SIDE creation evidence — an `employee.create` audit by the actor for THIS employee with a matching `requested_home_branch_id` — and authorizes with the SAME `assert_employee_directory_manager` predicate the create recheck ran, so an EXECUTIVE with an org-wide directory-manage grant — whom the create flow legitimately admits (201) — is no longer 403ed after commit into an orphan, while a `console_leave_cmd` caller cannot invoke the widened authority for an arbitrary imported/historical/residual branchless employee.
- Routing-race fix (reviewer follow-up): `assign_home_branch_if_unset` now takes the `replayed` flag. A replayed create preserves any already-established branch untouched; a FRESH creation that finds a DIFFERENT branch already assigned (another org-wide directory manager won the race) returns **409 conflict** instead of a silent 201 for the wrong branch. The `hr_employee_create_write_then_deny_total` canary also moved into the shared `assign_home_branch_if_unset` boundary so it instruments BOTH the People endpoint and the `recruiting_hire` handshake.

## Verification

- RED baseline: the `employee.home_branch_set` audit count asserted 0 vs 1 before the reroute; the custom-grant EXECUTIVE create asserted 201 before the reroute, was incorrectly 403 after the reroute, and is back to 201 after the HIGH-1 fix.
- `cargo check -p console-app`: green.
- `cargo clippy --all-targets`: green (no warnings).
- `CARGO_TARGET_DIR=/Users/jasonlee/Developer/console/.tmp/cargo-target-u6ih SQLX_OFFLINE=true bash tools/lanes/pgtest.sh /Users/jasonlee/Developer/console/.worktrees/lane-u6ih cargo test -p console-app --test hr_people_create_api`: **4 passed, 0 failed** (`readiness_counts_only_inspectable_active_payroll_close_statuses`, `employee_create_is_idempotent_unique_and_tenant_scoped`, `deactivated_super_admin_cannot_create_employee`, `revoked_grant_executive_cannot_create_employee`).
- Security RED-first: `deactivated_super_admin_cannot_create_employee` mints a `SUPER_ADMIN` token, then (1) deactivates the user and (2) re-activates but revokes the live `SUPER_ADMIN` role, asserting both creates return 403 with NO employee row, profile, lifecycle event, idempotency reservation, or `employee.create` audit. `revoked_grant_executive_cannot_create_employee` mints an `EXECUTIVE` token, then deletes the org-wide `employee_directory_manage` grant and asserts 403 with zero writes.
- `cargo clippy -p console-app --all-targets -- -D warnings` (re-run after the HIGH-1/HIGH-2 fix): green.
- `CARGO_TARGET_DIR=/Users/jasonlee/Developer/console/.tmp/cargo-target-u6ih SQLX_OFFLINE=true bash tools/lanes/pgtest.sh /Users/jasonlee/Developer/console/.worktrees/lane-u6ih cargo test -p console-app --lib --features test-postgres create_employee_core_refuses_a_revoked_custom_grant`: **1 passed, 0 failed** (the transactional grant-recheck boundary test — a lib `#[sqlx::test]` gated by `test-postgres`).
- `CARGO_TARGET_DIR=/Users/jasonlee/Developer/console/.tmp/cargo-target-u6ih SQLX_OFFLINE=true bash tools/lanes/pgtest.sh /Users/jasonlee/Developer/console/.worktrees/lane-u6ih cargo test -p console-app --test recruiting_pipeline_api`: **2 passed, 0 failed** (the shared `create_employee_core` + migration `0221` path, exercised through the recruiting hire handshake — verified because the same code + predicate changes reach both surfaces).
- `node scripts/check-executed-tests.mjs --update`: rewrote the test-attribute ratchet (`hr_people_create_api.rs` 2→4 for the four `#[sqlx::test]` fns); the gate is green after the lock-in (re-run `node scripts/check-executed-tests.mjs` exit 0).
- Migration-safety: PASSED (one migration, `0221`, a `CREATE FUNCTION` `leave_api.set_employee_home_branch_create` only — the general route function is untouched; no table/schema change beyond the new SECURITY DEFINER function's owner + REVOKE/GRANT).
- Writer-ownership `gate_detects_violation`: 47/47 (and `console-gate-writer-ownership` local run: scanned 294 production source files, OK — no new second writer).
- Documentation-authority verification suite (finding 2), exact outputs:
  - `npm run check:adrs` → `ADR governance gate passed: 39 ADRs, 6 design notes.` (exit 0)
  - `npm run check:doc-links` → `doc links OK (444 markdown files)` (exit 0)
  - `npm run check:doc-manifest` → `documentation manifest OK (444 markdown files)` (exit 0)
  - `npm run check:doc-citations` → `RESOLVES 14 / UNVERIFIABLE 0 / BROKEN 0 / FILE-ONLY 24 / MISSING 0` (exit 0)
  - `npm run check:foundation-gates` → `Foundation gate check passed (135 checks).` (exit 0)
  - `npm run verify` (`node scripts/verify.mjs fast`): every executed fast-tier check passed — foundation gate (135), reasoning-lens contract (40/40) + changed-record admission, CI-preflight contract (57/57), console authority-train / truth-ledger-validator / fanout-planner / PR-authority-bootstrap regressions, layer-boundary (173 crates, 0 violations), audit-coverage, migration-safety, tenant-isolation, pii-no-logs, rls-arming, dev-auth-absence, iac-tier, fabricated-branch, personal-data-classification, and writer-ownership. The Executed-tests ratchet step correctly RED'd on the un-locked 2→4 test gain and is green after `--update`; the console fanout-planner admission is skipped locally (dirty worktree by design) and runs in CI. CI `Repo gates — ADR / foundation / domain maturity` (run `31846051423`, job `94913091902`) executes the same doc-link/ADR/citation/foundation gates on this tree.
- `git diff --check`: clean (exit 0).

## Freeze status

NOT-FROZEN. This ledger and its seed-manifest record remain `active` (unfrozen) until the hosted checks for the `u6ih part-1` PR report success (Required / CI, Required / Security, `authenticate-console-authority`). Candidate C and Tip T SHAs are captured at push and finalized in the post-merge readback update.

## Review

Independent adversarial reviewer subagent `9d2d98a4` (report `ADV-782-01`), bound to head `683145c4f` / base `f9a88ed19`. Verdict **REQUEST-CHANGES** — four findings, all class C (real); conductor decided to implement all four in-lane. This is the FIRST (superseded-head) review.

**Second review `ADV-782-02`** (reviewer subagent `9d2d98a4`), bound to code head `13dd2d892` / base `b9c65ebc9`. Verdict **APPROVE-WITH-NOTES** — both HIGHs verified fixed. Notes disposition: MEDIUM-1 (message-map gap) FIXED — `employee_create.home_branch_create_required` (42501) → FORBIDDEN and `leave_home_branch.already_assigned` (40001) → CONFLICT added to `map_leave_command_sqlx` (with test assertion). LOW-2 (migration gap) GAP-NOTE — `0219`/`0220` reserved for B-EMP-A (#779); push-time contiguity check enforces the sequence; gap is intentional, not available for reuse. LOW-3 (non-locking assert) GAP-NOTE — `assert_employee_directory_manager` in `set_employee_home_branch_create` is a non-locking read, but the actor already created the employee under the locked `FOR UPDATE` recheck in `create_employee_core`; the assert is defense-in-depth, not the authority boundary.

ADV-782-02 bound to `13dd2d892` (base `b9c65ebc9`). Final head `C = lane-u6ih^` / `T = lane-u6ih` (base `0da6c2fdc`; SHAs frozen in the post-merge readback — the ledger cannot self-embed them because C's `documentation-manifest` `blob_sha` points at T's content) differs from the reviewed tree ONLY by (1) generated custody files and (2) `hr.rs` lines introduced by B-EMP-A #779 (its own signed/reviewed train, merged first). This lane's reviewed code delta is byte-identical to `13dd2d892`.

| Finding | Severity | Disposition |
| --- | --- | --- |
| HIGH-1 — orphan row behind 403: `create_employee` commits the row/profile/lifecycle/reservation/audit before `assign_home_branch_if_unset`, whose first-assignment authority is SUPER_ADMIN-only, so an admitted EXECUTIVE+custom-grant 403s into an orphan `home_branch_id IS NULL` row with a consumed idempotency key | P1 | Fixed — migration `0221` adds a dedicated create-path function `set_employee_home_branch_create` (general route untouched) so the create flow's first assignment uses `assert_employee_directory_manager`; EXECUTIVE+custom-grant → 201 restored; `hr_employee_create_write_then_deny_total` canary |
| HIGH-2 — half-closed TOCTOU: the recheck validated `is_active` + role strings but not LIVE custom grants | P1 | Fixed — `create_employee_core` re-reads `user_role_assignments`/`policy_roles`/`policy_role_permissions`/`policy_role_conditions` under `FOR UPDATE` before any write |
| MEDIUM-3 — test strength: 403 paths must capture write counts BEFORE the attempt and assert no row for the attempted key | P2 | Fixed — every 403 captures `people_write_counts` before, asserts no employee row/idempotency reservation for the key (mutation-test-style ordering guard); added `revoked_grant_executive_cannot_create_employee` |
| LOW-4 — Role type constants; distinct error variants; telemetry on write-then-deny | P3 | Fixed — `Role::SuperAdmin`/`Role::Executive` constants; distinct not-found / deactivated / role-revoked / grant-revoked messages; write-then-deny counter |

## Operational receipt

**Head SHA (base):** `0da6c2fdcc6eadc104527338a4772248f843d43d` (origin/main). Candidate C and Tip T SHAs are recorded at push.
**Review identities:** independent adversarial reviewer subagent `9d2d98a4` (report `ADV-782-01`), verdict **REQUEST-CHANGES**; the pinned signing authority is principal `jason19931225@gmail.com` (ED25519 `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`).

**Pre-mortem** — what could break and how it would manifest:

- **Second writer regression:** if `create_employee_core` (or any in-app path) regressed to a direct `employees` INSERT or back to `leave_api.create_employee`, the writer-ownership gate would attribute a second writer to an employment table. Contained by `gate_detects_violation` (47/47) and the Employment-owner gate catalog.
- **Routing-authority gap:** the create flow is still two-phase across two credential pools (row write on `console_rt`, first home-branch assignment on the leave command), so a NON-authority failure after the row commit (branch deactivated, optimistic-concurrency) can still leave a row with `home_branch_id IS NULL`. Contained by the fail-closed `require_home_branch_command_store` check before any write, the idempotent/race-convergent `assign_home_branch_if_unset`, and the rollback readback+repair below. AUTHORITY denial can no longer orphan: the routing predicate is now the SAME `assert_employee_directory_manager` that passed in-transaction.
- **Predicate drift (HIGH-1b regression):** if `leave_api.set_employee_home_branch_create` ever loses the server-side creation-evidence gate (or `create_employee_core` diverges from it), an EXECUTIVE+custom-grant could again 403 after commit. Contained by the 201 assertions and the `hr_employee_create_write_then_deny_total` telemetry canary, which counts ONLY FORBIDDEN routing failures and is an INVESTIGATE signal, not a hard stop: a non-zero count means either (a) the two predicates drifted apart or (b) a genuine mid-window revocation/deactivation orphaned a row — both warrant triage.
- **Deactivated / role-revoked / grant-revoked administrator:** an actor with an unexpired token whose `is_active`, built-in role, or custom grant changed after mint could pass the pre-flight and write. Contained by the live `FOR UPDATE` recheck of `users` + `user_role_assignments`/`policy_roles`/`policy_role_permissions`/`policy_role_conditions` at the top of `create_employee_core`, which fails closed before any write with distinct not-found / deactivated / role-revoked / grant-revoked errors.

**Blast radius:** ten paths. The code candidate C touches `backend/app/src/hr.rs`, `backend/app/src/recruiting_hire.rs`, `backend/app/tests/hr_people_create_api.rs`, `backend/crates/leave/adapter-postgres/src/lib.rs` (the new `set_employee_home_branch_create` command method), `backend/crates/leave/adapter-postgres/tests/leave_rls_surfaces_as_runtime_role.rs` (privilege census + entrypoint list), migration `backend/crates/platform/db/migrations/0221_employee_create_routing_authority.sql` (a `CREATE` of one SECURITY DEFINER function; no table/schema change), and the generated `docs/program/executed-tests-baseline.json` (no lockfile, OpenAPI, or CI surface); the authority tip T adds `docs/program/ledger/2026-08-14-u6ih-part1-employee-row-creation.md` and updates `docs/documentation-manifest.seed.json` and `docs/documentation-index.json` (evidence/authority record surface only).

**Detection signals:**

- CI runs the writer-ownership gate; a second `employees` writer or owner-string regression fails `gate_detects_violation`.
- The new `employee.home_branch_set` audit-count assertion (exactly one) fails if the command channel's audit is lost or duplicated.
- The custom-grant EXECUTIVE **201** assertions fail if the routing predicate narrows again (boundary regression); the plain-EXECUTIVE / mismatched-team / branch / MEMBER / ADMIN **403** assertions fail if the boundary loosens.
- The `deactivated_super_admin_cannot_create_employee` / `revoked_grant_executive_cannot_create_employee` tests fail if the live `is_active`/role/grant recheck is removed and the write boundary regresses to token-only authorization. Every 403 also asserts no employee row or idempotency reservation for the attempted key (mutation-test-style ordering guard).

**Rollback procedure (two-phase readback + repair):** the create flow commits the employee row, profile, lifecycle event, idempotency reservation, and `employee.create` audit, then establishes `home_branch_id` post-commit; an employee whose first assignment was refused (or whose grant was revoked mid-window) is left with `home_branch_id IS NULL`, and a retry through the reverted `leave_api.create_employee` returns immediately because the reservation already carries an `employee_id`. Phase 1 — revert: `git revert` of C and T (restores `leave_api.create_employee`) and redeploy the prior commit, THEN drop the added create-path function — a code revert does NOT undo an applied SQL migration, so `leave_api.set_employee_home_branch_create` remains executable until the operator applies:
```sql
DROP FUNCTION IF EXISTS leave_api.set_employee_home_branch_create(UUID, UUID, UUID, TIMESTAMPTZ, UUID, TEXT, TEXT);
```
  Phase 2 — readback + repair: identify THIS candidate's residual rows (created through the Employment-port create flow — an `employee.create` audit whose snapshot records `requested_home_branch_id` — yet still unassigned) with
    `SELECT e.org_id, e.id AS employee_id, e.employee_number, e.name, e.created_at, i.idempotency_key, (a.after_snap->>'requested_home_branch_id')::uuid AS requested_home_branch_id FROM employees e JOIN employee_create_idempotency i ON i.org_id = e.org_id AND i.employee_id = e.id JOIN audit_events a ON a.org_id = e.org_id AND a.action = 'employee.create' AND a.target_type = 'employee' AND a.target_id = e.id::text WHERE e.home_branch_id IS NULL;`
  Scope note: `create_employee_core` + `employee_create_audit` (with `requested_home_branch_id`) are SHARED with the pre-existing `recruiting_hire` handshake, so this readback also matches any historical recruiting-hire residual that is still unassigned — not only rows produced by this rollout. The operator MUST additionally filter `e.created_at` to the deployment window of this candidate (and, when the two-phase residual predates the rollout, exclude or separately review those pre-existing rows) before repairing, so unrelated historical records are never mutated. Rollback is complete only when the readback returns zero rows within the scoped window.
  then, for each, a SUPER_ADMIN establishes routing authority at the READ BACK `requested_home_branch_id` through the command channel (`PUT` home branch → `leave_api.set_employee_home_branch`), which re-validates the active branch and writes the `employee.home_branch_set` audit.

**Stop conditions** (do not merge if any fire):

- `gate_detects_violation` fails, or the writer-ownership census reports a second `employees` writer.
- The `employee.home_branch_set` audit-count assertion fails.
- The custom-grant EXECUTIVE scenarios do NOT return 201 (base behavior narrowed), or the plain-EXECUTIVE / mismatched-team / branch / MEMBER / ADMIN scenarios do NOT return 403.
- The `deactivated_super_admin_cannot_create_employee` or `revoked_grant_executive_cannot_create_employee` test fails (a deactivated / role-revoked / grant-revoked actor writes HR records).
- Any Cargo.toml / Cargo.lock / OpenAPI / `ci.yml` drift appears, or the migration `0221` alters the general `set_employee_home_branch` route function (which must stay untouched).

## Remaining HOLDs / follow-ups

- All current PRODUCT/ROADMAP HOLDs remain unchanged.
- `console-hee2` dependency: relocating `leave_api.set_employee_home_branch` off the leave command channel is OUT of scope here; it remains the only `console_leave_definer` function on the create flow (first home-branch assignment). This lane retargeted only the ROW write.
- `PgLeaveStore::create_employee` / `CreateEmployeeCommand` are now production-dead on this flow but retained (the leave crate is out of scope for this lane).
- The create flow is two-phase: a post-commit home-branch failure leaves the row without routing authority until the PUT route assigns (the `recruiting_hire` pattern) — recorded, not hidden.
- Census / writer-ownership Docker tests are CI-only here; not re-run locally in this lane's environment.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "authz",
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
    "Shared-nothing / eventual consistency",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Re-verified the write path in code rather than trusting the handoff: create_employee_core reaches employment::insert_employee_record (the Employment owner) and no console_leave_definer INSERT remains on the create flow; the RED audit assertion (0 vs 1) independently pins the new home-branch channel.",
    "Essentialism / YAGNI": "Kept the change to one handler plus two helpers and two test deltas; did not relocate leave_api.set_employee_home_branch or delete the dead PgLeaveStore::create_employee / CreateEmployeeCommand, which are console-hee2 and out-of-crate scope respectively.",
    "Chesterton's Fence": "Understood why leave_api.create_employee existed (home-branch routing authority is command-only since 0166) before rerouting: the ROW write moved to the Employment owner, but the first home-branch assignment stays on the leave command channel so that authority is not silently recreated in-app.",
    "Red Team": "Modeled the HIGH-1/HIGH-2 classes being closed: a custom-grant EXECUTIVE was 403ed AFTER the row committed because the first home-branch assignment used SUPER_ADMIN-only assert_org_admin, and the recheck ignored live custom grants. Both fixed by restoring assert_employee_directory_manager for the create recheck AND the first assignment (migration 0221), with deactivated / role-revoked / grant-revoked actors refused 403 before any write and EXECUTIVE+custom-grant admitted 201.",
    "Systems Thinking": "Traced the two-phase split (transactional row write + post-commit home-branch assignment) against the recruiting_hire handshake so the employee row, employment profile, lifecycle event, and idempotency reservation still commit atomically while routing authority is established separately.",
    "Operability / Day-2": "Recorded the two-phase residual explicitly: a home-branch failure after commit leaves the row without routing authority until the PUT route assigns (the recruiting_hire pattern). The audit split (employee.create + employee.home_branch_set) keeps each phase independently observable.",
    "Blast-radius / cell-based": "Change is confined to backend/app/src/hr.rs (one handler + two helpers), backend/app/src/recruiting_hire.rs (the replayed-flag call site), one test file, and one function-only migration (0221); no Cargo.toml/Cargo.lock, OpenAPI, or ci.yml surface.",
    "Shared-nothing / eventual consistency": "Single writer for employees is preserved (Employment owner only); home-branch routing authority remains command-only via the leave channel and converges idempotently under the updated_at CAS, with replay returning an already-assigned employee untouched.",
    "Zero-trust / defense-in-depth": "Fail-closed require_home_branch_command_store runs before any write; the live FOR UPDATE recheck of users + user_role_assignments/policy_roles/policy_role_permissions/policy_role_conditions at the top of create_employee_core re-validates the actor's is_active, role, AND live custom grant against the DB (not just the token) before the insert; row DML is gated to the Employment owner by the writer-ownership gate (47/47); and the first home-branch assignment re-validates the branch and applies the SAME directory-manager predicate at the command layer."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Before this lane, create_employee wrote the employees row through leave_api.create_employee (console_leave_definer INSERT), making the leave command capability a second writer of employees and letting a custom-grant EXECUTIVE obtain a home-branch assignment that skipped the org-admin check.",
    "The rerouted create flow is now two-phase: the Employment-owner row write commits first, and the first home-branch routing authority is established post-commit via the leave command channel (assign_home_branch_if_unset), so a home-branch failure leaves the row without routing authority until the PUT route assigns.",
    "Review found two defense-in-depth regressions: routing the row write through the Employment port dropped the live users.is_active + live-role + live-custom-grant recheck the old leave_api.assert_employee_directory_manager performed (a deactivated/role-revoked/grant-revoked actor could still write), and the first home-branch assignment kept SUPER_ADMIN-only assert_org_admin so an admitted EXECUTIVE+custom-grant 403ed AFTER committing an orphan. Fixed by restoring the full assert_employee_directory_manager predicate under FOR UPDATE at the top of create_employee_core and aligning the first-assignment predicate (migration 0221), with RED-first tests asserting 403 + no writes for deactivated/role-revoked/grant-revoked actors and 201 for EXECUTIVE+custom-grant."
  ],
  "decisions_changed_or_rejected": [
    "Rejected moving leave_api.set_employee_home_branch off the leave channel in this lane: that relocation is console-hee2 scope; only the ROW write was retargeted to the Employment port.",
    "Rejected deleting the now production-dead PgLeaveStore::create_employee / CreateEmployeeCommand: the leave crate is out of scope for this lane; recorded as a residual instead.",
    "Decided to restore the FULL assert_employee_directory_manager predicate (live is_active + role + custom grant) inside create_employee_core AND to align the first home-branch assignment predicate to it (migration 0221), rather than defer, because both regressions were a direct consequence of this lane's reroute."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
