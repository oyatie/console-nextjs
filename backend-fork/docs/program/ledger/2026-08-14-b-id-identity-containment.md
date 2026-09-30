# Authority tip — B-ID: identity/leave containment (9sb, cg6, lx6; 2v1 split)

**Date:** 2026-08-14
**Kind:** authority tip (T) bound on candidate C; T adds this ledger entry only
**Head SHA (base / fork point, origin/main):** `86c35b19035cbda3aba12f43df2c17b9d77a0892`
**Candidate C and Tip T:** two distinct commits on top of Base, both signed by the pinned authority (principal `jason19931225@gmail.com`, ED25519 `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`). C is the single code/wiring commit whose parent is Base and which does NOT contain this file; T is the single commit whose parent is C and which adds only this file. Their SHAs are recoverable from the branch as `C = lane-b-id^` and `T = lane-b-id`, and are frozen in the post-merge readback update — the ledger cannot self-embed them because C's documentation-manifest `blob_sha` points at T's content.
**Review identities:** Codex connector automated review on PR #776 (login `chatgpt-codex-connector`; ten threads across three review passes). Owner lane adopts, fixes, re-signs, and resolves on worktree `lane-b-id`. All ten threads — three ledger/evidence, three code (revoke-before-conflict race, OpenAPI 409 publication, Registry default-HQ guard), and two follow-up findings (credential-issuance serialization, create-branch 404 publication) — are addressed and resolved in this tip.
**Scope:** `backend/crates/identity/rest/src/lib.rs`, `backend/crates/identity/adapter-postgres/src/lib.rs`, `backend/crates/registry/adapter-postgres/src/lib.rs`, `backend/crates/platform/auth/src/webauthn.rs`, `backend/crates/platform/auth-rest/src/lib.rs`, four new adapter test files (two identity + one registry + one auth), the four OpenAPI path fragments that publish the new 404/409 responses, `backend/openapi/openapi.yaml` regeneration, Buck faces + `tools/ci/postgres-cargo-map.json` wiring for those tests, and `tools/buck/gen_first_party.py` resource requirements. No migration, no production-role grant change.
**Not product authority.** Clears no HOLD and authorizes no production, credential, compliance, payment, erase, OCI, frontend, projection, or Oyatie action.

## Lane recovery and review metadata

| Field | Record |
|---|---|
| Pre-mortem | A no-op role reorder churns audit rows; a no-op deactivate/activate replay writes a second transition audit row; a branch created or re-parented under a deactivated region attaches live rows to a hidden region and vanishes from the org tree; a deactivation racing a passkey/token issuance leaves a credential that the no-op replay path would no longer sweep; credential issuance concurrent with deactivation mints a usable passkey on an inactive account; the Registry default-HQ resolver attaches live records under a deactivated HQ region. |
| Blast radius | `backend/crates/identity/rest`, `backend/crates/identity/adapter-postgres`, `backend/crates/registry/adapter-postgres`, `backend/crates/platform/auth` + `backend/crates/platform/auth-rest`, the four OpenAPI path fragments + `backend/openapi/openapi.yaml`, and Buck/cargo-map/`gen_first_party.py`/shard test wiring. No migration, no production-role grant change; the census allowlist and unrelated crates are untouched. |
| Detection | `cargo test -p console-identity-rest --lib` (17 passed), `cargo test -p console-identity-adapter-postgres --lib` (4 non-DB passed), `cargo test -p console-registry-adapter-postgres --lib` (0 tests, compiles), `cargo test -p console-platform-auth --lib` (2 passed), `cargo test -p console-platform-auth-rest --lib` (4 passed; 2 pre-existing DB tests harness-fail without DATABASE_URL) run locally; the seven new `#[sqlx::test]` suites assert `Conflict`/audit-count == 1, a racing credential is revoked on replay, issuance on a deactivated account is refused with zero credentials, and the deactivated-HQ import is refused, in CI shards. |
| Rollback | Revert this branch's two commits (C then T); base `86c35b190` introduces no migration from this lane, so a revert restores the pre-lane tree. |
| Stop conditions | Widening beyond the identity + registry + platform-auth crates or the OpenAPI fragment set; adding a migration; touching the census allowlist or the shard tripwire beyond 89; claiming a DB-suite green without a hosted required-check run. |

## Summary

- **console-9sb** — `update_user` now compares role SETS order-insensitively (`system_roles_changed`); the same set in a different declaration order is no longer a change. RED baseline executed locally: `unchanged_role_set_in_a_different_order_is_not_a_change` failed under the sequence comparison, green after.
- **console-cg6** — no-op `deactivate_user`/`activate_user` replays return `KernelError::conflict` so no second transition audit row is produced (matches the `deactivate_region`/`deactivate_branch` convention). The deactivate replay path was rewritten from `with_audit` to `with_audits` + an idempotent `sweep_user_credentials_tx` so the already-inactive no-op arm still RUNS AND COMMITS the credential/session sweep before surfacing Conflict — closing the revoke-before-conflict race. The issuance half is now serialized too: `finish_registration_in_tx` SELECTs `is_active FROM users WHERE id = $1 FOR UPDATE` and rechecks after claiming the ceremony, so a passkey can never be minted on an account the sweep has already run against (inactive → 409, missing → 404). A replayed deactivate now records the two sweep audit rows with count 0 (no racing credential) rather than skipping cleanup.
- **console-lx6** — branch create/re-parent under a DEACTIVATED region returns 409 (`ensure_region_active_tx`, `FOR UPDATE` region lock for correct serialization against `deactivate_region`). The same guard now also protects `ensure_default_hq_branch` (master-list import) and `ensure_hq_branch_in_tx` (org-wide customer/site create) in `console-registry-adapter-postgres`, so the second production writer cannot attach live records under a hidden HQ.
- **OpenAPI 404/409 publication** — `409` → `#/components/responses/Conflict` is declared on all four affected operations (`api__v1__branches.post.yaml`, `api__v1__branches__id.patch.yaml`, `api__v1__users__id__deactivate.post.yaml`, `api__v1__users__id__activate.post.yaml`), and `404` → `#/components/responses/NotFound` is added to `api__v1__branches.post.yaml` for the new unknown/RLS-hidden-region guard; `backend/openapi/openapi.yaml` regenerated.
- **console-2v1 — NOT changed here; split.** The leave-writer delist requires, in order: moving the leave SECURITY DEFINER functions onto the Employment port (post-#775), a migration REVOKE of the 0166 grant, THEN the census-allowlist delist and the guard-test update. Delisting alone is provably red. Recorded as its own lane (see HOLDs).
- Test wiring: 4 test files registered in `TEST_RESOURCE_REQUIREMENTS`, Buck faces regenerated (173 files), 4 entries added to `tools/ci/postgres-cargo-map.json` (checker OK: 207 workflow entries), executed-tests baseline locked (base `86c35b190` sums to 2627 → 2636), shard tripwire 88 → 89 (the platform-family auth entry does not move the domain halves).

## Verification

Executed, with results (repository root; `cd backend && ...`; `CARGO_TARGET_DIR=/Users/jasonlee/Developer/console/.tmp/cargo-target-b-id SQLX_OFFLINE=true`):

| Command | Result |
|---|---|
| `cargo fmt -p console-identity-rest -p console-identity-adapter-postgres -p console-registry-adapter-postgres -p console-platform-auth -p console-platform-auth-rest -- --check` | clean |
| `cargo clippy -p console-identity-rest -p console-identity-adapter-postgres -p console-registry-adapter-postgres -p console-platform-auth -p console-platform-auth-rest --all-targets -- -D warnings` | clean |
| `cargo test -p console-identity-rest --lib` | 17 passed, 0 failed (15 pre-existing + 2 new) |
| `cargo test -p console-identity-adapter-postgres --lib` | 4 passed, 0 failed (non-DB) |
| `cargo test -p console-registry-adapter-postgres --lib` | 0 tests, compiles (no lib unit tests) |
| `cargo test -p console-platform-auth --lib` | 2 passed, 0 failed |
| `cargo test -p console-platform-auth-rest --lib` | 4 passed, 0 failed (+2 pre-existing DB tests harness-fail without DATABASE_URL) |
| `node tools/ci/check-postgres-cargo-map.mjs` | OK (207 workflow entries; facets app=56 platform=38 ontology=24 domain-a=45 domain-b=44) |
| `node --test tools/ci/postgres-shard.test.mjs` | 5 passed, 0 failed (tripwire 89) |
| `node scripts/check-openapi-refs.mjs` | 4987 refs, 27 mapping entries, 0 findings |
| `CARGO_TARGET_DIR=/Users/jasonlee/Developer/console/.tmp/cargo-target-b-id node scripts/check-executed-tests.mjs` | green — baseline locked (base `86c35b190` sums to 2627 → 2636) |
| `python3 tools/buck/test_gen_first_party.py` | 29/29 — the `console-app` gating pin is fixed by the now-merged PR #775 (base `86c35b190`) |

Executed vs stated baselines, kept separate:

- **9sb RED→GREEN executed locally:** the pre-fix sequence comparison failed `unchanged_role_set_in_a_different_order_is_not_a_change` (RED), then the same test passed after the set comparison (GREEN).
- **The 22 `#[sqlx::test]` suites (15 pre-existing + 7 new) were NOT executed locally** — they require the migrated probe DB and run only in CI via the workflow shards. Their RED baselines are STATED from the pre-fix behavior (a replayed deactivate leaves `credential_count == 1` pre-fix; a master-list import under a deactivated HQ succeeds pre-fix; issuance on a deactivated account inserts a credential pre-fix), and their GREEN result is established by the hosted `Test PostgreSQL` shard runs, not by this ledger.

## Freeze status

**NOT FROZEN YET.** The DB-backed suites and the aggregate verify run on the CI runner; this tip freezes in the post-merge readback update after hosted required checks pass.

## Remaining HOLDs / follow-ups

- All current PRODUCT/ROADMAP HOLDs remain unchanged.
- **console-2v1 split lane** (new bead): leave-definer move → 0166 REVOKE migration → census delist → guard-test update; dispatch after #775 merges.
- 9sb is coupled with console-0lj (same `rest/src/lib.rs` region) — merge order note for the conductor.
- cg6 returns 409 (not idempotent-200) on no-op replay: intentional, matches the deactivate_region convention; recorded deviation from the bead's "stored receipt" phrasing.
- PR #775 (Employment → canonical-adapter retarget + generator-suite gating fixes) is now merged at base `86c35b190`; this lane is rebased onto it.

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
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Separated the runnable evidence (9sb RED→GREEN executed locally) from the stated-only DB REDs, and recorded the generator-suite failures as pre-existing at base rather than claiming this tree green.",
    "Essentialism / YAGNI": "The 2v1 delist was NOT done half-way: delisting without removing the writer is provably red, so no partial change was admitted.",
    "Chesterton's Fence": "cg6 returns Conflict because with_audit writes the transition row on Ok; the conflict-rollback path is the established convention the bead itself names, so the no-op arm was reworked to run the sweep inside the same armed transaction instead of skipping it.",
    "Red Team": "Modeled the replay that burns a second audit trail, the deactivated-region hole that makes branches vanish, and the deactivation/credential-issuance race in both directions (a residual passkey after the sweep, and a passkey minted on an inactive account) — all now fail closed with regression tests.",
    "Systems Thinking": "Traced 2v1 across four ordered surfaces (leave functions, migration grant, census allowlist, guard test) and split the lane instead of violating the order; traced the default-HQ guard to a second production writer (Registry) rather than protecting only PgOrgStore.",
    "Operability / Day-2": "The new tests are wired into the workflow shards and the cargo-map so they can fail; the tip stays unfrozen until hosted checks pass.",
    "Blast-radius / cell-based": "Identity + registry + platform-auth crates, the OpenAPI fragment set, and shared test wiring; the shared-tools edits (gen_first_party requirements, cargo-map, shard tripwire) are scoped to the added entries.",
    "Zero-trust / defense-in-depth": "A FOR UPDATE region lock serializes branch create/update against region deactivation in both the identity and registry writers, closing the TOCTOU that let branches vanish and the default-HQ guard gap; a FOR UPDATE user-row lock plus an is_active recheck serializes credential issuance against deactivation, closing both halves of the race."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "update_user treated the same role set as a change whenever declaration order differed, churning audit rows and permissions state for a no-op.",
    "deactivate/activate replay wrote a full transition audit trail on a no-op, corrupting the audit chain's semantic density.",
    "Branches created or re-parented under a deactivated region disappeared from the org tree without a 409.",
    "The census allowlist still names console_leave_definer as an employees writer; the fix spans four ordered surfaces and cannot be completed in this lane.",
    "A deactivation racing a passkey registration or token issuance could leave a credential that the no-op replay path no longer swept — the early Conflict return skipped residual credential revocation.",
    "The new branch create/re-parent 409s and the user lifecycle replay 409s were not declared in the OpenAPI path fragments, so generated clients could not model them.",
    "The Registry default-HQ resolver (master-list import and org-wide customer/site create) upserted the HQ branch without the region-active guard, so a deactivated HQ still accepted live records.",
    "Credential issuance concurrent with deactivation could mint a usable passkey on an inactive account — the issuance path did not lock the user row or recheck is_active.",
    "The new unknown/RLS-hidden-region guard on POST /api/v1/branches returns 404, but the fragment declared only 409."
  ],
  "decisions_changed_or_rejected": [
    "Rejected delisting the leave writer without first moving the leave definer functions and revoking the 0166 grant — the census would raise and the guard test would fail.",
    "Rejected an idempotent-200 replay for cg6: with_audit writes the transition row on Ok, so Conflict-rollback is the only mechanism that actually suppresses it.",
    "Rejected suppressing the duplicate transition audit by bypassing residual credential cleanup: the no-op replay now runs and commits the idempotent sweep before surfacing Conflict, at the cost of two sweep audit rows with count 0."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
