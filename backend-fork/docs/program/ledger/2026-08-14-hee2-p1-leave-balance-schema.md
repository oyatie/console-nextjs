# Authority tip — console-hee2 PR-1: 0218 employee_leave_balances schema-only leave-balance table

**Date:** 2026-08-14
**Kind:** authority tip (T) bound on candidate C; T adds this ledger entry only
**Head SHA (base / fork point, origin/main):** `6a7b27b5ff5174db35d04de02b2a1ce2bce98e3d`
**Candidate C and Tip T:** two distinct commits on top of Base, both signed by the pinned authority (principal `jason19931225@gmail.com`, ED25519 `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`). C is the single schema/wiring commit whose parent is Base and which does NOT contain this file (migration 0218 + its contract test + the generated-face/CI wiring + the documentation-manifest seed entry + regenerated documentation-index + executed-tests baseline); T is the single commit whose parent is C and which adds only this file. Their SHAs are recoverable from the branch as `C = lane-hee2-p1^` and `T = lane-hee2-p1`, and are frozen in the post-merge readback update — the ledger cannot self-embed them because C's documentation-manifest `blob_sha` points at T's content.
**Scope:** two NEW files — `backend/crates/platform/db/migrations/0218_create_employee_leave_balances.sql` and `backend/crates/platform/db/tests/employee_leave_balances_migration_contract.rs` — plus the conductor-granted generated-face/CI wiring required to run that contract test in CI (`tools/buck/gen_first_party.py` TEST_RESOURCE_REQUIREMENTS entry, regenerated `backend/crates/platform/db/BUCK`, a `tools/buck/BUCK` `sh_test` wrapper, a `tools/ci/postgres-cargo-map.json` shard entry, and `docs/program/executed-tests-baseline.json`), plus the documentation-manifest seed entry for this ledger and the regenerated `docs/documentation-index.json`. No census/topology change, no Cargo.toml/Cargo.lock, no OpenAPI or `.github/workflows` change.
**Not product authority.** Clears no HOLD and authorizes no production, credential, compliance, payment, erase, OCI, frontend, projection, or Oyatie action. This is slice 1 of 4 of the console-hee2 leave-writer removal and is behavior-neutral on its own.

## Operational receipt

| Field | Record |
|---|---|
| Pre-mortem | A new leave-domain table could (a) miss RLS FORCE and leak tenant rows through the BYPASSRLS owner; (b) inherit console_app's default `GRANT SELECT, INSERT, UPDATE, DELETE TO console_rt` and silently hand the runtime role a mutation verb; (c) get an FK whose column order does not match `employees` UNIQUE(id, org_id), producing a semantically wrong self-pairing reference; (d) be omitted from personal-data classification and shelter under the frozen unclassified baseline; (e) collide with a migration number if another lane merges first. |
| Blast radius | A single additive table (`employee_leave_balances`) and one contract test. Nothing reads or writes the table in this slice, so there is no runtime blast radius; the only consumers are the three downstream PRs (backfill+cutover 0219, reader 0220-era, and the employees-column drop) that are serialized behind this merge. |
| Detection | `cargo run -q -p console-gate-migration-safety` (contiguity), `cargo test -p console-gate-migration-safety` (15/15), `cargo test -p console-gate-tenant-isolation` (19/19), the personal-data-classification gate (296 tables / 3383 cols / 789 classified), and the `#[sqlx::test]` migration contract (3/3 via pgtest.sh) all fail closed if the table shape, RLS, grants, or pd:personal comments drift. The contract reads the live catalog, so a `COMMENT ON COLUMN` naming a missing column, a missing RLS half, or a leaked grant is caught where it is real. |
| Rollback | Pre-application (Git-only): revert the two commits (T then C) — `employee_leave_balances` is additive and unreferenced, so dropping the two files restores the pre-lane tree with no data or schema to unwind. Post-application (migration already applied to a database): reverting the 0218 commit is NOT valid — it removes 0218 from the embedded `sqlx::migrate!` set while its row stays in `_sqlx_migrations`, so the next PreSync rejects applied version 218 as missing and blocks deployment. A forward `DROP TABLE` migration is likewise NOT available: 0218 marks `employee_leave_balances` as an audited table, and `console-gate-migration-safety` rejects every `DROP TABLE` on an audited table, so the forward compensation must be a concrete approved exception to the audited-table drop gate (approved and recorded first), not a plain `DROP TABLE`. Stop condition: this slice is schema-only and unshipped — 0218 is not applied in any deployed environment, so no post-application rollback is expected to run. |
| Stop conditions | A migration-number conflict with a merged lane (renumber 0218 contiguously BEFORE training); widening beyond the conductor-granted scope (migrations/platform-db + `tools/buck/gen_first_party.py`, first-party BUCK faces, `tools/buck/BUCK`, `tools/ci/postgres-cargo-map.json`, executed-tests baseline, seed); touching the census allowlist or the 20 canonical tables; claiming a DB-suite green without a hosted required-check run. |

## Summary

- **0218 creates `employee_leave_balances`** — the leave-owned ledger that relocates the three `employees` balance columns (`leave_accrued`/`leave_used`/`leave_remaining`, added by 0066, widened by 0166) off the canonical `employees` table. Shape: `PRIMARY KEY (org_id, employee_id)`; `FOREIGN KEY (employee_id, org_id) REFERENCES employees (id, org_id) ON DELETE CASCADE`; `org_id REFERENCES organizations(id) ON DELETE CASCADE`; three `NUMERIC(16,6) NOT NULL DEFAULT 0` balances; `updated_at TIMESTAMPTZ NOT NULL DEFAULT now()`; `-- console-gate: audited-table` marker; RLS `ENABLE` + `FORCE` with the canonical `org_isolation` policy; `REVOKE ALL` from `PUBLIC` and `console_rt`, then `GRANT SELECT` to `console_rt` and `SELECT, INSERT, UPDATE` to `console_leave_definer`; six `pd:personal` column comments.
- **The task's reversed FK column order was a semantic bug, corrected.** The brief's `(org_id, employee_id) REFERENCES employees(id, org_id)` pairs each `org_id` against the employee's `id` in the first position — not the intended tenant-global join. The migration writes `(employee_id, org_id) REFERENCES employees (id, org_id)`, matching `employees` UNIQUE(id, org_id) (0166:18).
- **No census/topology change** (verified): the writer-ownership census is scoped to the 20 canonical ObjectKey tables, and `EXPECTED_DEFINERS` is role-based with `console_leave_definer` already listed, so the additive table needs no allowlist entry. The census enforced cleanly during pgtest.
- **RED→GREEN:** 3 contract tests failed with `42P01 relation does not exist` before the migration applied; green after.
- **Generated-face/CI wiring (conductor-scoped):** the new `#[sqlx::test]` contract test is registered in `tools/buck/gen_first_party.py` (`console-platform-db` → `integration` → `'postgres'`), the first-party BUCK faces regenerated (173 files; `backend/crates/platform/db/BUCK` gains the `console-platform-db-itest-employee_leave_balances_migration_contract` `rust_test` target), a `platform-db-employee-leave-balances-migration-contract-pg` `sh_test` wrapper added to `tools/buck/BUCK`, a `postgres-cargo-map.json` shard entry added, and `executed-tests-baseline.json` locked (+3 declared test attrs). The test is `platform` family, so the domain facet tripwire is unchanged at 88 (post-peas: peas moved `employment_port` domain→ontology, 89→88) while the platform facet moves 38 → 39.
- **Residuals recorded:** `NOT NULL DEFAULT 0` diverges from the design sketch's nullable balances (the `employees.leave_*` source is nullable and 0166 reads with `COALESCE`) — PR-2's backfill must `COALESCE` or revisit; sqlx embeds migrations at compile time, so a `.sql` edit after a cached build needs a forced recompile (workflow gotcha for later PRs); the full platform-db suite plus rls_isolation/rollout were not re-run (representative-table proofs, unaffected by an additive table).

## Verification

Implementer-executed, with results (repository root; `cd backend && …`; `CARGO_TARGET_DIR=/Users/jasonlee/Developer/console/.tmp/cargo-target-hee2 SQLX_OFFLINE=true`):

| Command | Result |
|---|---|
| `cargo run -q -p console-gate-migration-safety` | PASSED (contiguity — 0218 immediately follows 0217) |
| `cargo test -p console-gate-migration-safety` | 15/15 passed |
| `cargo test -p console-gate-tenant-isolation` | 19/19 passed |
| personal-data-classification gate | PASSED (296 tables / 3383 cols / 789 classified) |
| migration contract (`employee_leave_balances_migration_contract.rs`) via pgtest.sh | 3/3 passed (RED baseline: 3 failed with `42P01 relation does not exist`) |
| classification sweep | 27 passed, 1 ignored |
| `cargo fmt … -- --check` / `cargo clippy … -- -D warnings` | clean |

Owner ceremony re-validation (re-trained on `6a7b27b5f`, post-peas/post-#778/post-#774/post-#780; DB-test wiring re-verified against the post-rebase tree):

| Command | Result |
|---|---|
| (a) `git ls-tree -r --name-only 6a7b27b5f -- backend/crates/platform/db/migrations/` | main tip = 0217; 0218 does not collide, contiguous |
| (b) `grep -c employee_leave_balances_migration_contract tools/buck/gen_first_party.py` | 1 (present exactly once, no duplicate) |
| (c) `python3 tools/buck/gen_first_party.py` + `python3 tools/buck/test_gen_first_party.py` | regenerated 173 BUCK faces; 29/29 (green with faces staged) |
| (d) `grep -c '"name": "platform-db-employee-leave-balances-migration-contract-pg"' tools/ci/postgres-cargo-map.json` | 1 (exactly one entry, no duplicate alias) |
| (e) `node tools/ci/check-postgres-cargo-map.mjs` | OK (208 workflow entries; facets app=56 platform=39 ontology=25 domain-a=44 domain-b=44) |
| (f) `node --test tools/ci/postgres-shard.test.mjs` | 5/5 (domain tripwire stays 88 — post-peas; `console-platform-db` is `platform` family, not `domain`) |
| (g) `node scripts/check-executed-tests.mjs --update` | baseline locked (361 sources, 2679 declared attrs; base `6a7b27b5f` 2676 + 3 for the new contract test) |
| `node scripts/console/generate-documentation-manifest.mjs --write` | regenerated index + seed for the C commit |
| `git -c gpg.ssh.allowedSignersFile=/tmp/allowed_signers verify-commit HEAD` and `… HEAD~1` | Good signature for `jason19931225@gmail.com` (ED25519 `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`) on both C and T (verified post-commit) |

Executed vs stated, kept separate: the DB-backed surfaces — the migration contract (3/3), the personal-data-classification gate, and the tenant-isolation integration test `owner_only_acl_is_effectively_private_on_postgres18` — were executed by the implementer against the migrated probe DB (pgtest.sh) and are not re-run by this owner ceremony, which re-runs the static/cargo gates (`--lib`), the manifest/reasoning-lens checks, and the commit-signature verification.

Omitted-suite exception (recorded, not silent): the full platform-db suite and the rls_isolation/rollout matrix are not re-run locally in this ceremony. This omission is an **approved exception** — (a) authorized by the conductor's merge-readiness directive for this lane (hybrid policy: fix real findings, gap-note unproven ones), (b) independently reviewed by the lane critic (PR #783 review threads), and (c) the full DB-backed suite is exercised by the hosted **Required / CI** check on this lane's final head before the tip freezes; representative-table reasoning alone is not the merge basis, the hosted required run IS the omitted-suite run. Freeze (below) stays gated on that run.

Re-ceremony rebinding: the prior tip's base line (`de8fc6432`, #776) predated the #777 merge and did not match the actual fork point; this tip is re-trained on `6a7b27b5f` (origin/main = peas `fb9ae31e6` + the #778 hardening/ry4f merge + the #774 census-derive-tables merge + the #780 employment freeze/backdating merge, each of which touched the serialized documentation index/seed — reconciled, not overwritten) and its C/T SHAs are the signed commits at `lane-hee2-p1^` / `lane-hee2-p1` (frozen in the post-merge readback update — the ledger cannot self-embed them because C's documentation-manifest `blob_sha` points at T's content). The hosted `authenticate-console-authority` required check independently verifies the C/T train (C and T both signed by the pinned principal, C..T = ledger-only, and the synthetic merge tree equals T's tree); it is the durable signature/identity record that the ledger cannot self-embed.

## Freeze status

**NOT FROZEN YET.** The seed-manifest record for this ledger is `status: active`. This tip freezes in the post-merge readback update after the hosted required checks (Required / CI, Required / Security, and authenticate-console-authority) pass.

## Remaining HOLDs / follow-ups

- All current PRODUCT/ROADMAP HOLDs remain unchanged.
- **PR-2 (0219, backfill + cutover)** is serialized behind this merge; its backfill must `COALESCE` the nullable `employees.leave_*` source columns into the `NOT NULL DEFAULT 0` targets (or revisit the nullability decision).
- **PR-3 and PR-4 of console-hee2** are serialized behind PR-1/PR-2: the reader re-pointing and the drop of the `employees` balance columns plus the `console_leave_definer` INSERT/UPDATE grant on `employees` (0220).
- **Open decision:** the `NOT NULL DEFAULT 0` balance divergence from the design sketch's nullable sketch is recorded as a residual, not silently resolved — PR-2 owns the `COALESCE` obligation.
- **Workflow gotcha for later PRs:** sqlx embeds migrations at compile time; a `.sql` change after a cached build requires a forced recompile to be picked up.
- **Conductor merge-order note:** this PR touches the serialized generated-face + postgres-cargo-map + executed-tests-baseline roots. Conductor directive: merge AFTER #777 (serialized single-writer root). Measurement note: the new contract test is `platform` family, so the domain facet tripwire is 88 (post-peas: peas moved `employment_port` domain→ontology, 89→88) and the platform facet moves 38 → 39 — the conductor's brief predicted a domain-family +1 to 90, corrected by the measured facet split.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "migration",
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
    "Cartesian doubt": "Rejected the task brief's literal FK column order and verified it against the live `employees` UNIQUE(id, org_id) constraint — the brief's `(org_id, employee_id) REFERENCES employees(id, org_id)` is a self-pairing semantic bug, not a stylistic choice.",
    "Essentialism / YAGNI": "PR-1 is schema-only: create the table, arm tenancy, grant least privilege — nothing reads or writes it yet, so the change is provably behavior-neutral and the smallest sufficient slice.",
    "Chesterton's Fence": "Kept the 0166/0213 idioms (REVOKE-then-GRANT is the whole truth because console_app's default privileges grant all verbs to console_rt) and the `org_isolation` FORCE pattern rather than inventing a new grant or RLS shape.",
    "Red Team": "Modeled the four ways an additive table fails silently: a missing RLS ENABLE+FORCE leaves the NOBYPASSRLS serving roles (console_rt / console_leave_definer) unscoped, a default grant hands console_rt a mutation verb, a mistyped FK silently pairs the wrong columns, and an unclassified table shelters under the frozen baseline — each is pinned by the live-catalog contract test or a gate. (console_app is BYPASSRLS and bypasses RLS with or without FORCE; that exemption is migration-only and not the tenant boundary.)",
    "Systems Thinking": "Traced the leave-writer removal across four ordered slices (schema → backfill+cutover → reader → column/grant drop) and confirmed this table sits outside the canonical census without breaking role-based EXPECTED_DEFINERS.",
    "Operability / Day-2": "The contract test reads the live catalog so drift in shape, RLS, grants, or pd:personal comments fails where it is real; the tip stays unfrozen until hosted required checks pass.",
    "Blast-radius / cell-based": "An additive, unreferenced table with one contract test — no runtime consumer in this slice, so the blast radius is the schema surface alone and rollback is a two-commit revert pre-application; post-application the audited-table drop gate rejects a plain `DROP TABLE`, so the (not-expected) forward compensation requires a pre-approved exception to that gate.",
    "Zero-trust / defense-in-depth": "RLS FORCE + REVOKE-from-PUBLIC/console_rt + GRANT SELECT only to the runtime role and SELECT/INSERT/UPDATE only to console_leave_definer, with no DELETE (erasure is the employees cascade), and least-privilege is asserted verb-by-verb in the contract."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "The task brief's reversed FK column order (org_id, employee_id) REFERENCES employees(id, org_id) was a semantic bug; corrected to (employee_id, org_id) to match employees UNIQUE(id, org_id).",
    "NOT NULL DEFAULT 0 diverges from the design sketch's nullable balances (employees.leave_* is nullable and 0166 reads with COALESCE); recorded as a residual so PR-2's backfill must COALESCE or revisit.",
    "sqlx embeds migrations at compile time, so a .sql edit after a cached build is not picked up without a forced recompile — a workflow gotcha for later PRs.",
    "The full platform-db suite and rls_isolation/rollout are not re-run locally (representative-table proofs, unaffected by an additive table); this is an approved exception — conductor-authorized, independently critic-reviewed, and the full DB-backed suite runs under the hosted Required/CI check before freeze.",
    "The implementer's evidence omitted the generated-face/CI wiring for the new contract test; the hosted CI preflight generated-face gate failed until the test was registered in TEST_RESOURCE_REQUIREMENTS, the BUCK faces regenerated, a tools/buck/BUCK wrapper added, and a postgres-cargo-map shard entry plus executed-tests baseline locked.",
    "Re-trained on origin/main (post-peas, #778, #774, #780) and resolved the critic threads: rebinding the receipt to the actual signed C/T (base corrected from de8fc6432 to the current main), recording the post-application rollback (commit-revert invalid; the audited-table drop gate requires a pre-approved exception, not a plain DROP TABLE), correcting the FORCE/BYPASSRLS wording in the migration + contract test, gap-noting the recorded NOT NULL residual (PR-2 owns COALESCE), and recording the conductor-authorized omitted-suite exception."
  ],
  "decisions_changed_or_rejected": [
    "Corrected the FK column order rather than keeping the brief's literal order, which would have created a semantically wrong self-pairing reference.",
    "Kept NOT NULL DEFAULT 0 (an explicit zero for 'no balance yet') rather than matching the nullable source columns, and recorded the backfill COALESCE obligation for PR-2 instead of silently resolving it.",
    "Added no census/topology entry (verified out of scope: the census is scoped to the 20 canonical tables and EXPECTED_DEFINERS is role-based with console_leave_definer already listed).",
    "Did not re-run the full platform-db suite / rls_isolation / rollout locally, recording an approved exception (conductor directive + critic review + hosted Required/CI run) rather than silently accepting representative-table proofs.",
    "Adopted the conductor's scope grant to edit the serialized generated-face/CI roots (gen_first_party.py, first-party BUCK faces, tools/buck/BUCK, postgres-cargo-map.json, executed-tests baseline) after the original migrations/platform-db/seed stop condition fired on the generated-face gate."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
