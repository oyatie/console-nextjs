# Authority tip — B-EMP-A employment containment lane (console-31e/2kd/0hf)

**Date:** 2026-08-14
**Kind:** authority tip (T) bound on candidate C; T adds this ledger entry only
**Base (fork point, origin/main):** `b9c65ebc9b1c31b424011df43a8a2d849d78b734` (post-#768 custody
docs + check-ci-preflight, post-#783 hee2-p1 0218 schema, post-#780 employment freeze/backdating,
post-#774 census derive, post-#778 hardening,
post-#781 peas employment port routing)
**Candidate C and Tip T:** two distinct commits on top of Base, both signed by the pinned authority
(principal `jason19931225@gmail.com`, ED25519 `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`).
C is the single code/wiring commit whose parent is Base and which does NOT contain this file; T is
the single commit whose parent is C and which adds only this file. Their SHAs are recoverable from
the branch as `C = lane-b-emp-a^` and `T = lane-b-emp-a`, and are frozen in the post-merge readback
update — the ledger cannot self-embed them because C's `documentation-manifest` `blob_sha` points at
T's content, so naming a literal SHA here would change C's tree and therefore the very SHA it names
(the same convention as the merged `l4-empl-retarget` and `b-id-identity` ledgers). Reviewers'
references to single unsigned intermediate heads (`3b60b002`, `7ce31016`) were stale heads from
before the round-4 re-ceremony; the merge head is the two signed commits above, each
`git verify-commit`-clean against the pinned authority.
**Scope:** `backend/app/src/hr.rs` (console-31e), `backend/crates/ontology/rest/src/lib.rs`
(console-2kd/0hf plus the round-2 replay authz recheck, the distinct canonical-projected audit
action, the round-3 cross-action replay rejection, and the round-6 object-type binding + legacy-audit
recognition), `backend/crates/ontology/rest/src/projected_dispatch_derivation.rs` (the dispatch
contract's `object_type_id` thread through `ProjectedDispatch`),
`backend/crates/ontology/rest/tests/canonical_replay.rs`
(the replay/repair/authority/cross-action/cross-object-type regressions) and
`backend/crates/ontology/rest/tests/canonical_dispatch_audit_as_runtime_role.rs` (the PostgreSQL
audit integration proof, retaxonomied to `ontology.canonical.execute`), `backend/crates/ontology/canonical-domain`
(the `CanonicalPort` `command()` signature), the six canonical ports (`company`, `employment`,
`job_position`, `org_unit`, `person` in `ontology/canonical-adapter-postgres`; `pay_run` in
`payroll/adapter-postgres`) + their runtime-role test suites,
`backend/crates/platform/db/migrations/0219_canonical_projected_audit_dedup.sql` (the receipt
`action_key` + `object_type_id` columns) + `backend/crates/platform/db/migrations/0220_audit_events_canonical_projected_execute_idx.sql`
(the CONCURRENT partial unique index; **migration lease held — hee2 renumbers**),
`backend/crates/ontology/rest/BUCK` + `tools/buck/BUCK` + `tools/ci/postgres-cargo-map.json` +
`tools/buck/gen_first_party.py` + `tools/buck/test_gen_first_party.py` (test-face wiring),
`docs/documentation-manifest.seed.json` + `docs/documentation-index.json` (this ledger's admission),
`docs/program/executed-tests-baseline.json` (test-attribute lock), and this ledger. No `ci.yml`
workflow change, OpenAPI, or `Cargo.toml` change — CI *wiring* IS changed via
`tools/ci/postgres-cargo-map.json` and the Buck faces (DB-test routing, not a workflow edit).
**Not product authority.** Clears no PRODUCT/ROADMAP HOLD. Makes no production, legal, or
compliance claim. Merge authority is earned separately by the hosted required checks on the PR head.

## Summary

Three contained defects, each fixed RED-first and each leaving a regression test that pins the
refusal rather than merely documenting it.

- **console-31e — reinstatement of an EXITED employee is refused.** `validate_lifecycle_transition`
  now returns `invalid_transition` (HTTP 409) for `ONBOARD` and `TRANSFER` when the employee's
  `current_status` is `EXITED`. RED: the regression test `unwrap_err()`s on a result the pre-fix
  code returned as `Ok`. Green: 1/1.
- **console-2kd — projected dry-run now runs the owning port's PURE preflight.** A new
  `ProjectedDispatchRegistry::preflight` registers each canonical port's `P::preflight` (an
  associated function with no `&self`, no IO, no side effect) and routes `preflight_action` through
  the shared `decode_canonical_query` so `would_execute` reflects the port's own verdict, not just
  the §16 gates and submit criteria. An unresolved roster target fails closed (`NotWiredYet`),
  matching dispatch. Green: 19 non-DB lib tests.
- **console-0hf — a canonical replay returns the stored receipt without re-consuming approval.**
  `execute_action` peeks `ont_action_command_receipts` (RLS-scoped, inside the same armed
  transaction the writers use) *before* the gate chain. When a prior receipt exists for the same
  `command_id` and its `actor_id` matches the principal, it short-circuits to the port's replay,
  returning the stored receipt verbatim and skipping the four-eyes gate and the single-use consume.
  The `ontology.canonical.execute` audit is ensured **idempotently**: a retry whose first attempt
  committed the mutation + receipt but whose audit emission then failed replays to success and
  repairs the missing audit, while a healthy replay mints no second row. RED: a retry was denied
  `GateDenied`. Green: 5/5 via `tools/lanes/pgtest.sh` (replay + repair + authority-recheck +
  cross-action-rejection + cross-object-type-rejection).

### Round-2 review fixes (conductor decision — all four implemented)

- **P1 PKF — replay authz recheck.** The replay short-circuit now re-runs the CURRENT
  `authority_effect(principal)` before returning the stored receipt; a requester who has since lost
  the org-wide capability is refused with a 409 conflict. Pinned by
  `canonical_replay_rechecks_current_authority`.
- **P1 PKA — dedup discriminator.** Canonical projected execute now records its audit under a
  distinct action, `ontology.canonical.execute` (not `ontology.action.execute`, which
  `instance_revision_writeback` owns), so the two paths can never collide on the polymorphic
  `target_id`.
- **P2 GbV — DB-enforced uniqueness.** Migration 0220 adds a partial unique index on
  `audit_events (org_id, action, target_id) WHERE action = 'ontology.canonical.execute'`, closing
  the check-then-insert TOCTOU; 0219 adds the receipt `action_key` column (see round-3).
- **P2 PKH — repair metadata.** The accepted `action_key` is now bound into the immutable receipt
  (`ont_action_command_receipts.action_key`, written by each canonical port via the widened
  `CanonicalPort::command()` signature); a legacy (NULL) receipt falls back to the retry's key on
  repair.

### Round-3 review fixes (conductor decision — both implemented)

- **P1 IXx — reject cross-action replay.** The stored receipt's `action_key` is now COMPARED with
  the retry's; a non-null mismatch (same `command_id`, same canonical target + payload, different
  action) is refused with a 409 conflict rather than handed the stored receipt, because the retry's
  action may carry different checklist/four-eyes/egress controls. Pinned by
  `canonical_replay_rejects_a_different_action` (replaces the round-2
  `canonical_replay_records_the_accepted_action_key`, whose accept-and-relabel scenario is now
  rejected outright).
- **P1 IX5 — build the audit index concurrently.** The partial unique index moved out of 0219 into
  a `-- no-transaction` migration 0220 as `CREATE UNIQUE INDEX CONCURRENTLY`, matching the
  `audit_events` read-path precedent (0101/0103/0104/0124): a plain `CREATE UNIQUE INDEX` would
  block `audit_events` inserts for the heap scan on a populated table. 0219 now carries only the
  transactional `ALTER TABLE … ADD COLUMN action_key, ADD COLUMN object_type_id` + `COMMENT`s.

### Round-6 review fixes (conductor decision — two REAL-CODE, three REAL-DOC)

- **P1 ATN — bind replay receipts to the object type.** `action_key` is unique only per object
  type (the `ActionCommand` doc says so), yet the replay guard compared only the key string. 0219
  now also adds a NULLABLE `object_type_id` column; the `CanonicalPort::command()` signature threads
  `object_type_id` through every port into the receipt, and the replay guard compares it (non-null
  mismatch → 409). Pinned by `canonical_replay_rejects_a_different_object_type`.
- **P1 ATR — recognize legacy canonical audits during replay repair.** Commands accepted before this
  deployment recorded their canonical-projected audit under `ontology.action.execute` with the same
  `after_snap` `"dispatch":"projected_usecase"` shape; the repair existence check now recognises
  BOTH that legacy taxonomy and the new `ontology.canonical.execute`, so a replay of a legacy command
  does not append a second audit row.

**Residuals (documented, not defects):**

- Replay returns empty gates (`GateChainOutcome { gates: Vec::new(), allow: true }`): canonical
  ports store no gate chain, and the actor-match check before the replay preserves authz — a
  `command_id` owned by another principal is refused (`forbidden`), never replayed.
- The 0hf regression is a `#[sqlx::test]` (the implementer's final report placed it inside
  `lib.rs mod tests`, so lib runs needed `DATABASE_URL` or `--skip`); a review P1 moved it to
  `tests/canonical_replay.rs` — a `resource.postgres` integration face wired through
  `TEST_RESOURCE_REQUIREMENTS`, `tools/buck/BUCK`, and `tools/ci/postgres-cargo-map.json` — so the
  `resource.none` unit target no longer carries a database-dependent test.
- 2kd's RED was **structural** (the `preflight` method was absent, so a projected dry run returned
  `Ok`), not a live runtime failure.
- **Migration-number collision resolved:** this lane initially used 0218, but hee2-p1 (PR #783) holds
  `0218_create_employee_leave_balances.sql`; this lane now holds **0219 + 0220** (the round-3
  concurrent-index split — 0219 carries the transactional `action_key` column, 0220 the
  `-- no-transaction` `CREATE UNIQUE INDEX CONCURRENTLY`). hee2-p1 merged first (`97a45cfc7`), so the
  contiguity gate now passes contiguous **0218 → 0219 → 0220** on the rebased head (verified against
  `git ls-files 'backend/crates/platform/db/migrations/*.sql'`).
- **Conductor pre-ruling:** migrations 0219 + 0220 are ADMITTED under the same rationale as hee2-p1 —
  the `ALTER TABLE` in 0219 is a bare `ADD COLUMN` (no parser-gap class), and 0220 is a single
  `-- no-transaction` `CREATE UNIQUE INDEX CONCURRENTLY` statement, so neither falls in the
  `ALTER TABLE ONLY` parser-gap class.
- `ont_action_command_receipts.action_key` AND `object_type_id` are both NULLABLE so legacy receipts
  written before 0219 replay with a fallback to the retry's key/type (a non-null accepted key or type
  that differs from the retry's is now REJECTED — see rounds 3 and 6); the six canonical ports always
  write both going forward. This NULL-legacy fallback is the documented migration limitation, not a
  defect: 0219 added the columns, so a pre-0219 receipt carries neither by construction and the
  cross-action/cross-object-type mismatch cannot be detected for it. Both columns are classified
  `pd:none` (ontology identity keys, not personal data).

## Freeze status: NOT-FROZEN

This ledger is registered in `docs/documentation-manifest.seed.json` as `class: evidence`,
`status: active`, `retention: retain`. It is deliberately **NOT-FROZEN** and must remain
`active`/unfrozen until the hosted required checks — `Required / CI`, `Required / Security`, and
`authenticate-console-authority` — pass on the PR head. It is frozen only after the train reaches
MERGE_READY; a review finding may still change the code, the verification counts, or the residuals.

## Verification

### RED-first evidence (implementer's final report)

- **31e — live RED:** `lifecycle_transition_refuses_onboard_and_transfer_when_already_exited`
  did `unwrap_err()` on `Ok(())` — pre-fix, `ONBOARD` of an `EXITED` employee returned success;
  post-fix it returns `invalid_transition` (HTTP 409).
- **2kd — structural RED:** `ProjectedDispatchRegistry::preflight` did not exist, so a projected
  dry-run returned `Ok` for every payload and the port's own `P::preflight` refusal was invisible
  until execute.
- **0hf — live RED:** via `bash tools/lanes/pgtest.sh <repo_root> cargo test -p
  console-ontology-rest --lib canonical_replay_returns_stored_receipt_without_respending_four_eyes`,
  the retry failed with `a same-command replay must return the stored receipt, not GateDenied:
  GateDenied("an action gate is not satisfied")`.

### Green verification (exact invocations)

Run from the lane worktree with
`CARGO_TARGET_DIR=/Users/jasonlee/Developer/console/.tmp/cargo-target-b-emp` and
`SQLX_OFFLINE=true`:

```bash
# 31e — EXITED reinstatement refusal (console-app lib, default features)
cargo test -p console-app --lib lifecycle_transition_refuses_onboard_and_transfer_when_already_exited
# => 1 passed; 0 failed; 0 ignored; 162 filtered out

# 2kd — non-DB lib suite (the DB replay test excluded by name)
cargo test -p console-ontology-rest --lib -- --skip canonical_replay_returns_stored_receipt_without_respending_four_eyes
# => 19 passed; 0 failed

# 0hf — DB replay test on disposable PostgreSQL 18.4 (run twice; container removed cleanly)
bash tools/lanes/pgtest.sh <repo_root> cargo test -p console-ontology-rest --lib \
  canonical_replay_returns_stored_receipt_without_respending_four_eyes
# => 1 passed; 0 failed (x2)

# format + lint (both changed crates)
cargo fmt -p console-app -p console-ontology-rest            # applied, then:
cargo fmt -p console-app -p console-ontology-rest -- --check # => clean
cargo clippy -p console-app -p console-ontology-rest --lib --tests  # => clean
```

### Lane re-verification (round-2, after the review moved the DB regressions to an integration target)

```bash
cargo test -p console-ontology-rest --lib                     # => 19 passed; 0 failed; 0 filtered out
tools/lanes/pgtest.sh . cargo test -p console-ontology-rest --test canonical_replay
# => 5 passed; 0 failed (replay + repair + authority-recheck + cross-action-rejection + cross-object-type-rejection)
cargo clippy -p console-ontology-rest -p console-ontology-canonical-adapter-postgres \
  -p console-payroll-adapter-postgres -p console-orgchange-adapter-postgres --all-targets -- -D warnings
# => clean
```

### Round-2 gates (conductor-required)

```bash
cargo run -p console-gate-migration-safety                 # => PASSED (contiguous 0218→0219→0220)
cargo run -p console-gate-tenant-isolation                 # => PASSED
cargo run -p console-gate-personal-data-classification     # => PASSED (action_key = pd:none)
```

### Round-3 verification (round-6 re-ceremony, on top of post-#768 `b9c65ebc9`)

```bash
# 19 non-DB ontology-rest lib tests (post-rebase, SQLX_OFFLINE)
cargo test -p console-ontology-rest --lib                     # => 19 passed; 0 failed

# clippy -D warnings across the four changed crates (collapsible-if collapsed in the cross-action guard)
cargo clippy -p console-ontology-rest -p console-ontology-canonical-adapter-postgres \
  -p console-payroll-adapter-postgres -p console-orgchange-adapter-postgres --all-targets -- -D warnings
# => clean

# DB-backed replay suite (replay + repair + authority-recheck + cross-action + cross-object-type)
bash tools/lanes/pgtest.sh . cargo test -p console-ontology-rest --test canonical_replay
# => 5 passed; 0 failed

# Gates
cargo run -p console-gate-tenant-isolation                 # => PASSED
cargo run -p console-gate-personal-data-classification     # => PASSED (action_key + object_type_id = pd:none)
cargo run -p console-gate-migration-safety                 # => PASSED (contiguous 0218→0219→0220, post-#783)

# Documentation-authority surfaces
git diff --check                                            # => clean
node scripts/console/generate-documentation-manifest.mjs --check   # => OK (ledger blob_sha pinned)
# Buck generator: corrected the console-app inline ordinary-test pin 163→164 (post-#780 added one)
# and re-ran the exact command below:
python3 tools/buck/test_gen_first_party.py                  # => Ran 29 tests: 29 OK, 0 FAILED
node scripts/check-executed-tests.mjs --update              # => tripwire recomputed on post-#783 tree (362 sources, 2685 attrs)
node tools/ci/check-postgres-cargo-map.mjs                  # => OK (209 workflow entries)
npm run verify                                              # => see the final tip re-run in the push gate (migration-safety now passes)
```

Discovered/executed: 19 non-DB ontology-rest lib tests + 5 DB-backed
replay/repair/authority/cross-action/cross-object-type integration tests + 1 hr.rs lifecycle test
= 25 executed, all green; tenant-isolation, personal-data-classification, and migration-safety
(contiguous 0218→0219→0220) gates all PASS.

## Operational receipt

- **Pre-mortem** (what could go wrong and how it is contained): (1) A canonical replay short-circuit
  could replay *another principal's* receipt if the actor-match check regresses — it is a hard
  `forbidden` refusal before dispatch, and the DB test asserts the approval is spent exactly once.
  (2) Empty replay gates could be misread as "no gate ran" instead of "already authorized" — the
  ledger documents the empty-gates residual explicitly. (3) The EXITED refusal could over-block a
  legitimate future reinstate flow — that flow must route through a different transition, not this
  guard. (4) The "PURE preflight" claim breaks if a port's `P::preflight` ever gains IO or a side
  effect — `canonical_port_preflight` calls only the associated function and the blocking-port test
  asserts the execute path is unreachable.
- **Detection:** RED-first regression tests (`unwrap_err` on `Ok` for 31e; `GateDenied` for 0hf; a
  blocking port for 2kd), 19/19 non-DB, 5/5 DB-backed via `pgtest.sh`, `clippy -D warnings` clean,
  and the DB-backed suite re-run in CI. The P1 audit-gap finding was caught by hosted review and
  pinned by the new `canonical_replay_repairs_a_missing_execute_audit` regression.
- **Rollback:** revert the two commits (code `fix(hr)` then `docs(ledger)`); the previous behavior
  is restored exactly (pre-fix `validate_lifecycle_transition` accepted the transitions; pre-fix
  `execute_action` evaluated the gate chain before any receipt peek).
- **Stop conditions:** any review finding that demands a `ci.yml` workflow, OpenAPI, or
  `Cargo.toml` change is **out of lane scope** — STOP and report to the delegating conductor; do not
  widen the diff. Migration changes are in-scope under the conductor's lease (0219 + 0220).
- **Review identities:** automated adversarial review by `chatgpt-codex-connector` on PR #779
  (seventeen threads across four passes — audit-gap, DB-placement, candidate-SHA, scope, then
  authority-recheck / dedup-discriminator / DB-uniqueness / repair-metadata, then the round-3 pass
  (concurrent-index / cross-action-replay / candidate-SHA restatement / scope restatement /
  doc-authority-verification), then the round-4 re-ceremony — every one resolved in-tree); lane owner
  `Jason Lee <jason931225@gmail.com>` adopts, fixes, re-signs, and resolves on worktree `lane-b-emp`.

## HOLDs remaining

Unchanged. No production promotion, no frontend, no payment execution, no `ci.yml` workflow change
or OpenAPI change, and no invented compliance or legal scope. Migrations 0219 + 0220 are held by this
lane under the conductor's lease (hee2's later slices renumber). **Conductor pre-ruling (round-5):**
0219 + 0220 are ADMITTED under the same rationale as hee2-p1 — 0219's `ALTER TABLE` is a bare
`ADD COLUMN` and 0220 is a single `-- no-transaction` `CREATE UNIQUE INDEX CONCURRENTLY`, so neither
falls in the `ALTER TABLE ONLY` parser-gap class. The three required hosted checks must pass on the
PR head before this train is MERGE_READY; this ledger is not itself merge authority.

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
    "Red Team",
    "Systems Thinking",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Verified each bug RED-first rather than trusting the implementer's green claims; re-ran exact invocations and recorded discovered/executed counts.",
    "Essentialism / YAGNI": "Smallest sufficient change per defect; the round-2 migration is one additive partial unique index plus one nullable receipt column, with no ci.yml/OpenAPI/Cargo change.",
    "Red Team": "Modeled the replay short-circuit as the attack surface (another principal's command_id, double-spend of the four-eyes approval, a committed mutation left without its execute audit, a lost-capability requester replayed, and an audit dedup key that could collide with instance_revision) and pinned each with an assertion.",
    "Systems Thinking": "Traced the gate chain, the RLS-scoped receipt store, and the canonical port preflight as one dispatch seam so decode/target/subject binding cannot diverge between dry-run and execute.",
    "Operability / Day-2": "The DB-backed regression runs in CI and locally via pgtest.sh with disposable PostgreSQL; rollback is a two-commit revert.",
    "Blast-radius / cell-based": "Round-2 widens to the six canonical ports (a command() signature) plus one additive migration; the receipt peek stays inside the armed tenant transaction and no production-role grant or shared-writer boundary changed.",
    "Telemetry-first": "Recorded exact SHAs, invocations, and pass/fail counts; the replay path returns the stored receipt verbatim so a retry is observable rather than re-minted.",
    "Zero-trust / defense-in-depth": "Actor-match before replay, RLS-scoped peek in the armed transaction, fail-closed NotWiredYet for unresolved preflight targets, and single-use approval consumed exactly once."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "console-31e: an EXITED employee could be re-onboarded or transferred; the transition validator now refuses both with 409 invalid_transition.",
    "console-2kd: a projected dry-run returned Ok for every payload because it never ran the owning port's P::preflight; preflight_action now routes through the pure preflight seam.",
    "console-0hf: a same-command retry was denied GateDenied because the gate chain peeked an already-spent approval before the port could replay; the receipt peek now short-circuits before the gate chain.",
    "Review (P1): the replay branch returned success while skipping the execute audit, so a first attempt whose port committed but whose audit emission failed left the HR/payroll mutation unaudited; the replay now idempotently ensures the audit exists.",
    "Review (P1): the #[sqlx::test] regressions sat in lib.rs mod tests under the resource.none unit target; they moved to tests/canonical_replay.rs (resource.postgres) so the unit suite runs clean without a database.",
    "Review (P1 PKF): the replay returned the stored receipt to a requester who had since lost the org-wide capability; it now re-runs the current authority_effect and refuses with 409.",
    "Review (P1 PKA): the dedup key (action + target_id) collided with instance_revision_writeback's ontology.action.execute rows; canonical projected execute now records ontology.canonical.execute.",
    "Review (P2 GbV): the check-then-insert dedup was TOCTOU-racy with no DB uniqueness; migration 0220 adds a partial unique index over (org_id, action, target_id) for the canonical action.",
    "Review (P2 PKH): the repair recorded the retry's action_key; the accepted action_key is now bound into the receipt and read back on repair (a non-null mismatch is now rejected, see IXx).",
    "Review (P1): canonical_dispatch_audit_as_runtime_role still counted the old ontology.action.execute, so the new action taxonomy would zero it out; the test and its contract text now use ontology.canonical.execute.",
    "Review (P2): two concurrent repairs could both pass the SELECT EXISTS and the losing insert would surface 23505 from the unique index; emit_canonical_projected_audit now treats 23505 as idempotent success.",
    "Review (P1 IXx): the replay read prior_action_key but never compared it, so a cross-action replay (same command_id through a different action) was accepted; it is now rejected with 409 and pinned by canonical_replay_rejects_a_different_action.",
    "Review (P1 IX5): the unique index was a plain CREATE UNIQUE INDEX, which blocks audit_events inserts during the heap scan on a populated table; it moved to a -- no-transaction CREATE UNIQUE INDEX CONCURRENTLY migration (0220), matching the 0101/0103/0104/0124 precedent.",
    "Review (P1): the lane scope omitted tools/buck/test_gen_first_party.py and misstated the CI change; the scope now enumerates every changed path and states the CI-wiring (postgres-cargo-map.json + Buck faces) explicitly.",
    "Review (P1 ATN): action_key is unique only per object type but the replay guard compared only the key; the receipt now binds object_type_id (threaded through CanonicalPort::command) and the guard rejects a cross-object-type replay with 409, pinned by canonical_replay_rejects_a_different_object_type.",
    "Review (P1 ATR): the replay repair existence check searched only the new ontology.canonical.execute taxonomy, so a legacy ontology.action.execute canonical-projected audit was invisible and the replay appended a second row; the check now recognises both taxonomies via the after_snap dispatch discriminator.",
    "Review (P1 doc): the scope omitted src/projected_dispatch_derivation.rs and tests/canonical_dispatch_audit_as_runtime_role.rs; the DB-backed count contradicted the invocation (2/2 vs 5/5); the Buck-generator 29/29 claim was stale after #780 (163 vs 164). All three corrected in-tree."
  ],
  "decisions_changed_or_rejected": [
    "Rejected a bare-pool receipt peek in favor of the RLS-scoped with_org_conn read inside the armed transaction, so a FORCE-RLS tenant-scoped table cannot be read out of scope.",
    "Rejected synthesizing a gate chain on replay; the stored receipt is returned with empty gates and the actor-match check is the authz boundary.",
    "Rejected blindly skipping the audit on replay; emit_canonical_projected_audit is now check-then-insert so a replay repairs a missing audit without double-minting.",
    "Rejected leaving the DB regressions in lib.rs mod tests; they moved to a resource.postgres integration target (matching the sibling canonical_dispatch_audit itest) instead of a --skip workaround.",
    "Rejected widening scope to ci.yml/OpenAPI/Cargo.toml changes; migrations 0219+0220 are now in scope under the conductor's lease (previously a STOP-and-report condition, lifted by the conductor decision).",
    "Rejected accepting a cross-action replay and relabeling the audit with the accepted key; the replay now REJECTS a non-null action-key mismatch instead of recording it.",
    "Rejected a plain CREATE UNIQUE INDEX on the append-heavy audit_events table; the index is now built CONCURRENTLY in a no-transaction migration."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob; the ledger
stays `active`/NOT-FROZEN until the hosted required checks pass on the PR head.
