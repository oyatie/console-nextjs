# Authority tip — REG-P4: payroll calculation rows made append-only in the database

**Date:** 2026-08-17
**Kind:** lane record for candidate PR #793, written to satisfy [AGENTS.md](../../../AGENTS.md) L8.
**Base:** `bc70e1587480cf149ba8b944b7201917f866d140` (origin/main) — the immutable diff base, taken from `git merge-base HEAD origin/main` on the reviewed head rather than transcribed. It moved once when #794 merged and this branch was brought up to date; a candidate's base is only immutable relative to a fixed head, which is why it is derived here and not copied forward.
**Candidate head at review:** `d8ee9ae8` was the head the fourth-round review bound to; this record is amended on the head that answers it. A candidate cannot contain its own final SHA, so the merge SHA is recorded by the documentation-only closeout rather than predicted here — that recursion is register row V7.
**Review:** automated adversarial review by `chatgpt-codex-connector[bot]` on head `0e3bf6be89`, verdict **COMMENTED**, three P1 findings. All three accepted; two changed the migration, one produced this file. No human reviewer identity is recorded because branch protection requires zero approvals — that gap is register row V6 and is not resolved here.
**Scope:** `backend/crates/platform/db/migrations/0222_payroll_payable_runtime_write_revoked.sql` (new), `backend/crates/payroll/adapter-postgres/tests/payroll_lifecycle_rls_as_runtime_role.rs` (one added test), `docs/program/executed-tests-baseline.json` (ratchet), `docs/documentation-manifest.seed.json` and `docs/documentation-index.json` (both regenerated for this ledger's blob), and this ledger. No Cargo.toml/Cargo.lock, no OpenAPI, no `ci.yml`.
**Not product authority.** Clears no HOLD. Authorizes no production, payment, issuance, or compliance action.

## Summary

Migration 0186 declares `payroll_line_calculations` rows *"append-only ... only the `payable` flip is a legal update"*. Neither half was enforced. `console_rt` — the role the application runs as — held table-wide INSERT and UPDATE. Measured against a PostgreSQL 18.4 replica of the CI role topology with all 221 migrations applied: as `console_rt`, `UPDATE payroll_line_calculations SET payable = TRUE` **succeeded**.

0222 revokes INSERT and UPDATE at table scope and re-grants **INSERT only**, on every column except `payable`. The runtime role can therefore append a calculation and nothing else.

## Pre-mortem

The way this change kills payroll is by revoking too much: if `console_rt` loses a privilege the calculation writer needs, `calculate` fails closed for every tenant and no draft can be produced. The second failure mode is revoking too little and believing otherwise — the shape the first revision actually took.

## What the review changed

- **Re-granting UPDATE was wrong.** The first revision re-granted UPDATE on every non-`payable` column to avoid disturbing the writer. Nothing in the tree updates this table: the only writer is a plain INSERT with an explicit column list, and there is no `ON CONFLICT DO UPDATE` against it. The re-grant preserved an unused privilege that left `gross_won`, `deductions`, `net_won` and `tax_table_version` rewritable after calculation and review, and those columns are read straight into payslip issuance. Removed.
- **The stated rationale was false.** The candidate claimed `payable` gates payslip issuance. It does not: `load_payslip_issuance_in_tx` validates `legal_basis` and never selects or checks `payable`. The migration is still correct — append-only is the declared contract — but it is justified by that contract, not by an issuance gate that does not exist. **`payable` gates nothing on the production path today; that gap is unresolved and out of scope here.**

## Blast radius

**Three tables, not one.** `payroll_line_calculations` loses `console_rt`'s
INSERT and UPDATE (re-granted per column, excluding `payable`) and DELETE.
`payroll_draft_runs` and `payroll_draft_lines` each lose `console_rt`'s DELETE
and keep INSERT, SELECT, UPDATE — they are revoked because both foreign keys
from the child cascade, so the child revoke alone was bypassable through either
parent. Those two are used throughout the payroll lifecycle, so an operator
inspecting or restoring this change must look at all three ACLs, not just the
child's.

`console_app` (migration owner) is untouched everywhere, so the release path
`payable` exists for remains buildable by a later, separately reviewed change.
No data is read, written, or migrated; this is a privilege change only.

## Detection

The migration proves its own effect and refuses to apply otherwise: it raises `payroll_payable.runtime_update_not_revoked` if any UPDATE survives, `payroll_payable.runtime_insert_not_revoked` if `payable` remains insertable, and `payroll_payable.runtime_lost_legitimate_insert` if the writer's own columns were dropped. It also raises `payroll_payable.cascade_parent_delete_not_revoked` if either
parent keeps DELETE. In service, the symptom of over-revocation is a payroll
write returning insufficient-privilege (SQLSTATE 42501) rather than a wrong
number — on the child that is `calculate`; on either parent it would be any
lifecycle write, which is why the parents keep INSERT/SELECT/UPDATE and lose
only DELETE.

## Rollback

Additive and reversible by a corrective migration that re-grants what 0222
removed — on all three tables: column-level INSERT and UPDATE plus DELETE on
`payroll_line_calculations`, and DELETE on `payroll_draft_runs` and
`payroll_draft_lines`. Applied migrations are never edited. There is no data
change to undo.

## Stop conditions

Halt and reverse if any payroll calculation write fails with SQLSTATE 42501 in any environment, if the migration's own assertions raise on a real database, or if a writer of this table is discovered that this record claims does not exist.

## Verification

- Migration applied cleanly as `console_app` on a fresh database carrying all 222 migrations, after `ops/postgres-reconcile-topology.sh` reconciled the seven-role topology.
- Fail-closed matrix, as `console_rt`: `UPDATE ... SET payable = TRUE` **denied**; `INSERT ... (payable)` **denied**; `UPDATE ... SET net_won` / `gross_won` / `tax_table_version` **denied**; `INSERT` omitting `payable` **allowed**; `SELECT payable` **allowed**.
- `cargo test --locked --manifest-path backend/Cargo.toml -p console-payroll-adapter-postgres --test payroll_lifecycle_rls_as_runtime_role` — 2 passed (the new test plus the pre-existing RLS test). The repository root has no `Cargo.toml`; the workspace manifest is `backend/Cargo.toml`, so every Cargo command here carries `--manifest-path` or is run from `backend/`.
- Mutation proof: **RED** with 0222 removed, **GREEN** restored. This required forcing a rebuild — `#[sqlx::test(migrations = ...)]` embeds the migration set at **compile time**, so moving the file out and back leaves a stale binary that silently tests the old schema. Two earlier attempts at this proof were invalid for exactly that reason and were discarded.
- `cargo fmt --all -- --check` and `cargo clippy --all-targets -- -D warnings`, both run from `backend/` — green.
- Cascade closure, same database: `DELETE` on `payroll_line_calculations`,
  `payroll_draft_lines` and `payroll_draft_runs` all denied (42501), while the
  parents keep `INSERT,SELECT,UPDATE` and the child keeps `SELECT` plus
  column-level `INSERT`. Both FKs are `ON DELETE CASCADE` (0186), so revoking
  only the child left the rows erasable through either parent.

**Documentation-authority gates** (required by `docs/current/DELIVERY.md`
because this candidate adds a ledger record and modifies both manifests):

- `npm run check:doc-links` — `doc links OK (457 markdown files)`
- `npm run check:doc-manifest` — `documentation manifest OK (457 markdown files)`
- `npm run check:doc-citations` — passed, 0 missing
- `npm run check:adrs` — `ADR governance gate passed: 39 ADRs, 6 design notes`
- `npm run check:foundation-gates` — passed
- `node scripts/check-ci-preflight.mjs` — `CI preflight contract passed.`
- `node scripts/check-executed-tests.mjs` — ratchet green at 2690 declared test
  attributes across 362 reachable sources (static; not an executed count)
- `node scripts/check-reasoning-lens-contract.mjs --changed-since <base>` — passed
- `git diff --check` — clean

**Required test invocations** (`DELIVERY.md` names the tests, not only the
gates; discovered/executed counts recorded):

- `node --test scripts/check-doc-links.test.mjs` — 36 passed, 0 failed
- `node --test scripts/check-adrs.test.mjs` — 29 passed, 0 failed
- `node --test scripts/check-foundation-gates.test.mjs` — 6 passed, 0 failed
- `node --test scripts/check-ci-preflight.test.mjs` — 63 passed, 0 failed
- `node --test scripts/verify.test.mjs` — 15 passed, 0 failed
- `npm run verify` — `verify(fast) passed.` (exit 0)

`npm run verify` required the pinned DotSlash runtime on PATH. It is obtainable
only with the export precedence corrected in #794 —
`${CONSOLE_DOTSLASH_BIN_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}/console-dotslash}/bin}`
— which is why the earlier revision of this record could not produce it.

 Note `cargo fmt` without `--all` does not format `tests/` and exits "Failed to find targets" against this workspace; an earlier run of it reported success while formatting nothing.

## Harness limitation found while fixing this

`#[sqlx::test]` does not reproduce the production privilege topology. Migration
0031 grants the runtime role DML on future tables via
`ALTER DEFAULT PRIVILEGES **FOR ROLE console_app**`; the sqlx harness applies
migrations as `console_buck_admin`, so tables it creates never carry that
default. Measured: on a database built the production way `console_rt` holds
`INSERT,SELECT,UPDATE,DELETE` on `payroll_line_calculations` before 0222 and
`SELECT` after it.

**The gap is narrower than first recorded, and the earlier wording was wrong.**
0186 grants `SELECT, INSERT, UPDATE` explicitly, so those arrive in a sqlx
database regardless of which role creates the table, and the tests asserting
them are sound. Only the *default-privilege-derived* grants are absent — DELETE
here, and DELETE on the two cascade parents. So the limitation is specific:
a `*_as_runtime_role` test cannot prove a privilege that arrives through
`ALTER DEFAULT PRIVILEGES FOR ROLE console_app`, because the harness applies
migrations as `console_buck_admin`. It proves explicit grants and RLS normally.
Recording it as "runtime-role grant tests are generally ineffective" would have
invited discarding valid regressions.

What covers the default-derived cases is the migration's own
`runtime_delete_not_revoked` and `cascade_parent_delete_not_revoked`
assertions, evaluated wherever migrations run.

## Remaining HOLDs

- **Forgery by append is not closed.** `console_rt` keeps column-level INSERT
  and issuance reads the highest `version` per line without consulting
  `payable`, so a compromised session can append a higher-version row with
  arbitrary money fields after approval and before issuance. Revoking UPDATE
  and DELETE closes mutation and erasure only. Closing this needs INSERT routed
  through a status-checked writer that refuses a new version once the run
  leaves the calculating state — a change to the calculation path, not a grant
  change.
- `payable` still gates no production path. Issuance validates `legal_basis` only.
- No human review identity. Branch protection requires zero approvals (register V6).
- The register's REG-P4 row should be amended to drop the issuance-gate claim.

## Reasoning lens contract

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "authz",
    "migration",
    "hr_payroll"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Chesterton's Fence",
    "Red Team",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "The register asserted `payable` was unenforced; that was re-derived by execution rather than reading, on a PostgreSQL 18.4 replica of the CI role topology with all 221 migrations applied, where `UPDATE payroll_line_calculations SET payable = TRUE` succeeded as `console_rt`. The same doubt applied to the first fix: a column-level REVOKE against a table-level grant was measured to be a silent no-op instead of assumed effective, and two mutation proofs were discarded once `#[sqlx::test(migrations)]` was found to embed the migration set at compile time.",
    "Chesterton's Fence": "Migration 0186 declares these rows append-only with only the `payable` flip as a legal update. The first revision re-granted UPDATE on all non-`payable` columns to avoid disturbing the writer, which honored the fence's shape while removing its reason; the grant now matches what 0186 actually says. `console_app` keeps its privileges so the release path the column exists for stays buildable.",
    "Red Team": "Modeled a compromised tenant-scoped `console_rt` session: it could set `payable` (the release-gate bit) and, under the first revision, rewrite `gross_won`, `deductions`, `net_won` and `tax_table_version` after calculation and review — columns `load_payslip_issuance_in_tx` reads verbatim into issued payslips. Both paths are now denied at plan time. The reviewer found the second; it was not caught by a passing test because the test asserted the mechanism, not the exposure.",
    "Operability / Day-2": "The failure mode of over-revocation is `calculate` returning SQLSTATE 42501 for every tenant, so the migration asserts in both directions and refuses to apply if the writer's own columns were dropped. Rollback is an additive corrective migration; applied migrations are never edited, and there is no data change to undo.",
    "Blast-radius / cell-based": "Three tables, one role, two privilege classes. `payroll_line_calculations` loses INSERT/UPDATE at table scope (INSERT re-granted per column, excluding `payable`) and DELETE; `payroll_draft_runs` and `payroll_draft_lines` each lose DELETE only, because both foreign keys from the child cascade and the child revoke alone was bypassable through either parent. No data read, written, or migrated; `console_app` untouched; no other grantee affected. A failure is visible as a refused write rather than a wrong number.",
    "Zero-trust / defense-in-depth": "Enforcement is a database object rather than the absence of code that writes the column. `payable` remains DEFAULT FALSE, but the runtime role can no longer set it or amend a committed calculation, so the invariant survives a caller that tries."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "`console_rt` held table-wide INSERT and UPDATE on `payroll_line_calculations`, so `payable` — documented as 'true only after release-gate pass' — was enforced by nothing but the absence of code that set it.",
    "A column-level `REVOKE INSERT (payable), UPDATE (payable)` against a table-level grant is a silent no-op; the privilege must be removed at table scope and re-granted per column.",
    "Nothing in the tree updates this table: the only writer is a plain INSERT with an explicit column list and there is no `ON CONFLICT DO UPDATE` against it, so the first revision's re-grant of UPDATE preserved an unused privilege over money columns.",
    "`payable` gates nothing on the production path: `load_payslip_issuance_in_tx` validates `legal_basis` and never selects or checks it. The candidate's original justification was false; the migration stands on the append-only contract instead.",
    "The first revision's self-assertion checked that `console_rt` RETAINED UPDATE on `net_won` — a self-proving migration proving the wrong invariant.",
    "`#[sqlx::test(migrations = ...)]` embeds the migration set at compile time, so removing a migration file and re-running produces a stale binary that silently tests the old schema; two mutation proofs were invalid before this was found.",
    "`cargo fmt` without `--all` does not format `tests/` and exits 'Failed to find targets' against this workspace, reporting success while formatting nothing."
  ],
  "decisions_changed_or_rejected": [
    "Rejected re-granting UPDATE on non-`payable` columns (the first revision): nothing uses it and it left the money columns rewritable after review.",
    "Rejected a CHECK constraint pinning `payable = FALSE`: it would block the legitimate future release path and require a migration to lift.",
    "Rejected adding an issuance-time `payable` gate in this lane: it is a behavior change to the payslip path, not a privilege fix, and is recorded as an open HOLD instead.",
    "Rejected picking a migration number by inspecting the directory: the wave-4 slot ledger assigns numbers at merge, and 0222 is a placeholder."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
