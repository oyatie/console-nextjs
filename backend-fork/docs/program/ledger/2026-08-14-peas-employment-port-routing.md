# Authority tip — Console-pees: Employment port-routing (retire the orgchange `#[path]` seam)

**Date:** 2026-08-14
**Kind:** authority tip (T) bound on candidate C; T adds this ledger entry only
**Head SHA (base / fork point, origin/main):** `f9a88ed192fb7c0588c9c6ba16ea64da84f2887d`
**Candidate C and Tip T:** two distinct commits on top of Base, both signed by the pinned authority (principal `jason19931225@gmail.com`, ED25519 `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`). C is the single code/wiring commit whose parent is Base and which does NOT contain this file; T is the single commit whose parent is C and which adds only this file. Their SHAs are recoverable from the branch as `C = lane-peas^` and `T = lane-peas`, and are frozen in the post-merge readback update — the ledger cannot self-embed them because C's documentation-manifest `blob_sha` points at T's content.
**Review identities:** Owner and signing principal `Jason Lee` / `jason19931225@gmail.com` (ED25519 `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`). Codex connector automated review on PR #781 (login `chatgpt-codex-connector`; three threads): one ledger-binding finding was refuted (it cited a nonexistent commit `745d154f` authored by `Codex <codex@openai.com>`; the actual chain is the signed `C = lane-peas^` / `T = lane-peas` below), and two valid findings — the unwired-port error kind flattened to 409, and the moved test left stale in `postgres-cargo-map.json`/`executed-tests-baseline.json` — were adopted and fixed in this tip.
**Scope:** `backend/crates/orgchange/adapter-postgres/src/lib.rs` (retire the Employment `#[path]` seam; declare the consumer-side `EmploymentTransferPort` trait + `TransferPortFuture`; fail-closed `None` default on `PgOrgChangeStore`), `backend/app/src/lib.rs` (composition root wires `PgEmploymentTransferPort` delegating to the canonical `reassign_org_unit_via_transfers_in_tx`), the Employment port test file moved from `backend/crates/orgchange/adapter-postgres/tests/` to `backend/crates/ontology/canonical-adapter-postgres/tests/` (only the `use` path and one assertion message change), the orgchange BUCK face (test removed) + canonical adapter BUCK face (test added) + `tools/buck/gen_first_party.py` resource-requirement entries, `tools/ci/postgres-cargo-map.json` (employment entry moved to the canonical package) + `docs/program/executed-tests-baseline.json` (the 22-count entry moved to the canonical path) + `tools/ci/postgres-shard.test.mjs` (domain-half inventory tripwire 89 → 88, employment leaving the domain family for ontology), and `backend/ci/gates/writer-ownership/tests/gate_detects_violation.rs` (new RED→GREEN seam-retirement gate test). No migration, no Cargo.toml, no lockfile, no OpenAPI, no `ci.yml`.
**Not product authority.** Clears no HOLD and authorizes no production, credential, compliance, payment, erase, OCI, frontend, projection, or Oyatie action.

## Lane recovery and review metadata

| Field | Record |
|---|---|
| Pre-mortem | The composition root forgets `.with_employment_transfer(..)` and `ReassignOrgUnit` returns a runtime 500 (`KernelError::internal("employment transfer port is not wired")`) instead of failing at compile time — fail-closed, but a latent wiring hole not caught by any dedicated end-to-end test; the delegation flattens `CanonicalPortError` to `KernelError::internal(message)` and the orgchange arm re-wraps it as `KernelError::conflict`, so a drift between `.message` and the source error's `Display` would silently change the 409 text; the moved test's `use` path (`console_orgchange_adapter_postgres::employment` → `console_ontology_canonical_adapter_postgres::employment`) could break if the canonical adapter's re-exports differ from what the seam exposed; a future contributor re-introduces a `#[path]` of the owner's `employment.rs` and the gate test is the only tripwire. |
| Blast radius | `console-orgchange-adapter-postgres` (the `ReassignOrgUnit` apply arm + the fail-closed default), the `console-app` composition-root wiring, the moved test file, and the writer-ownership gate test + BUCK/`gen_first_party.py` test wiring. The Employment DML never leaves the canonical adapter (single writer); non-reassign proposal kinds are untouched; no migration. |
| Detection | `orgchange_no_longer_compiles_the_employment_seam` (RED while the seam was present, GREEN after); `cargo run -p console-gate-writer-ownership` (294 production files, OK — single writer per owned table); `cargo run -p console-gate-layer-boundary` (173 crates, 0 violations — no adapter→adapter edge); the 22-test `employment_port_as_runtime_role` DB suite now runs against the canonical adapter; the orgchange integration suites still effectuate non-reassign proposals through the unwired `new()` constructor; fmt/clippy on the four touched crates. |
| Rollback | Revert this branch's two commits (C then T); base `f9a88ed19` introduces no migration from this lane, so a revert restores the pre-lane tree. |
| Stop conditions | Adding a Cargo edge from `console-orgchange-adapter-postgres` → `console-ontology-canonical-adapter-postgres` (adapter→adapter is illegal); touching migrations, the lockfile, OpenAPI, or `ci.yml`; deleting, weakening, or `.gitignore`-ing the seam-retirement gate test; reintroducing Employment DML compilation into the orgchange crate; claiming a DB-suite green without a hosted required-check run. |

## Summary

- **Seam retired** — the orgchange adapter no longer compiles `#[path = "../../../ontology/canonical-adapter-postgres/src/employment.rs"]`, so it compiles no Employment DML at all. The owner (retargeted by console-1qw.4) stays the canonical adapter.
- **Consumer-declared port** — `EmploymentTransferPort` (and its `TransferPortFuture<'a>` alias) is declared BY the orgchange adapter, because the layer-boundary gate forbids one adapter depending on another. The trait borrows the caller's apply transaction for its whole duration and returns `Result<u64, KernelError>` (number of employees moved).
- **Fail-closed default** — `PgOrgChangeStore::new(pool)` sets `employment_transfer: None`; the `ReassignOrgUnit` arm `ok_or_else`s to `KernelError::internal("employment transfer port is not wired; ReassignOrgUnit cannot apply")` rather than silently skipping the employee move. `apply_ops` checks for a missing port against any `ReassignOrgUnit` op BEFORE its per-op conflict mapping, so the wiring defect surfaces as a 500 (`ErrorKind::Internal`) instead of being flattened to a client-correctable 409. Non-reassign proposal kinds never touch the port, so a store built without it still effectuates those proposals (proved by the orgchange integration suites).
- **Composition-root wiring** — `console-app` defines `PgEmploymentTransferPort` implementing the trait by delegating to the canonical `reassign_org_unit_via_transfers_in_tx`, and wires it in `build_router` via `.with_employment_transfer(Arc::new(PgEmploymentTransferPort))`. The composition root is the only place the two adapters may meet.
- **Test file moved** — `employment_port_as_runtime_role.rs` moved from the orgchange adapter's tests to the canonical adapter's tests (22 DB tests), so the runtime-role DB suite now exercises the owner's port directly rather than through the seam. Only the `use` path and one assertion message changed; the DML it verifies is unchanged.
- **Error-kind flattening preserved byte-for-byte** — the port returns `KernelError` whose `.message` is the canonical error's `to_string()`, so the orgchange arm's `KernelError::conflict("REASSIGN_ORG_UNIT via hr.transfer failed: {}")` produces the identical 409 text as before.

## Verification

Executed (repository root unless noted; `cd backend && ...`; `CARGO_TARGET_DIR=/Users/jasonlee/Developer/console/.tmp/cargo-target-peas SQLX_OFFLINE=true`). Rows marked **(ceremony)** were re-executed first-hand during this lane's commit ceremony; the DB-backed rows were executed by the implementer against the migrated probe DB and are re-verified by the hosted CI shards.

| Command | Result |
|---|---|
| `cargo fmt --check` | clean **(ceremony)** |
| `cargo clippy -p console-app -p console-ontology-canonical-adapter-postgres -p console-orgchange-adapter-postgres -p console-gate-writer-ownership --all-targets -- -D warnings` | clean **(ceremony)** |
| `cargo test -p console-gate-writer-ownership --lib` | 10 passed **(ceremony)** |
| `cargo test -p console-gate-writer-ownership --test gate_detects_violation` | 48 passed (47 pre-existing + the new seam gate) **(ceremony)** |
| `cargo run -p console-gate-writer-ownership` | OK — 294 production source files, no new second writer, no stale ratchet entry **(ceremony)** |
| `cargo test -p console-gate-layer-boundary --test gate_detects_violation` | 13 passed **(ceremony)** |
| `cargo run -p console-gate-layer-boundary` | PASSED — 173 workspace crates, 0 violations **(ceremony)** |
| `node tools/ci/check-postgres-cargo-map.mjs` | OK — the moved employment entry resolves on the canonical package/facet **(ceremony)** |
| `node scripts/check-executed-tests.mjs` | green — the 22-count employment entry moved with the file; no new dark binary **(ceremony)** |
| `node --test tools/ci/postgres-shard.test.mjs` | 5 passed — domain-half inventory tripwire locked 89 → 88 (employment moved to the ontology shard) **(ceremony)** |
| `cargo test -p console-ontology-canonical-adapter-postgres --lib` | 6 passed **(ceremony)** |
| `cargo test -p console-orgchange-adapter-postgres --lib` | 2 passed (the retained `org_unit_binding` seam unit tests) **(ceremony)** |
| `cargo test -p console-gate-writer-ownership --test census_executes_against_postgres` | 3 passed, incl. `census_binds_to_an_executed_database` (20 tables examined) — DB-backed |
| `cargo test -p console-ontology-canonical-adapter-postgres --test employment_port_as_runtime_role` | 22 passed — DB-backed |
| orgchange integration: `apply_refuses_deactivated_region` (1), `org_reference_surface` (4), `preflight_persists_nothing` (5) | 10 passed — DB-backed; store-without-port still effectuates non-reassign proposals |
| `cargo test -p console-ontology-canonical-domain --lib` | 11 passed |
| `cargo test -p console-app --lib` | 162 passed |
| `python3 tools/buck/test_gen_first_party.py` | idempotent (the clean-faces test passes post-commit) |

Executed vs stated baselines, kept separate:

- **RED→GREEN gate test executed by the implementer:** while the `#[path]` seam was still present, `orgchange_no_longer_compiles_the_employment_seam` failed 0/1 (the source still named `canonical-adapter-postgres/src/employment.rs`); after the seam retires, the same test passes 1/1 and the full `gate_detects_violation` suite is 48/48.
- **The DB-backed suites (census 3/3, employment_port 22/22, orgchange 1+4+5) were executed by the implementer against the migrated probe DB**, not re-run during the ceremony; their GREEN result is re-established by the hosted `Test PostgreSQL` shards, and this tip freezes only after those hosted checks pass.

## Freeze status

**NOT FROZEN YET.** The DB-backed suites and the aggregate verify run on the CI runner; this tip freezes in the post-merge readback update after hosted required checks pass.

## Remaining HOLDs / follow-ups

- All current PRODUCT/ROADMAP HOLDs remain unchanged.
- **console-ptmi** (new follow-up bead): retire the `org_unit_binding` `#[path]` seam — the OrgUnit owner DML is still compiled into the orgchange adapter via `#[path = "../../../ontology/canonical-adapter-postgres/src/org_unit_binding.rs"]`. This lane retires only the Employment seam.
- **No end-to-end proposal-through-trait wiring test** — the reassign write itself is fully covered (the 22-test DB suite against the canonical port), and the wiring is compile-verified plus fail-closed, but no integration test drives a full `ReassignOrgUnit` proposal through `build_router` → `.with_employment_transfer` → the trait impl. Recorded as a residual gap, not a HOLD.
- **`console-ontology-canonical-domain` remains a dependency of the orgchange adapter** — the Employment `CanonicalPort` usage is gone, but the crate still resolves `console_ontology_canonical_domain::DispatchTarget` through the retained `org_unit_binding` seam, so the dep is not removable in this lane (no Cargo.toml edits are permitted, and its retirement belongs to console-ptmi).
- Merge order note for the conductor: this lane is rebased onto base `f9a88ed19` (which already contains PR #775 Employment-retarget, PR #776 identity-containment, and PR #777 payroll-containment); the shared domain-half inventory tripwire is ordered `#777 (89) → this lane (88) → console-hee2-p1 (89)`. No other open lane edits the orgchange apply arm or the composition-root router region touched here.

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
    "Cartesian doubt": "Separated the runnable evidence (fmt/clippy, the four gate suites, canonical-adapter lib 6/6, orgchange lib 2/2) from the DB-backed suites (census 3/3, employment_port 22/22, orgchange 1+4+5) and verified the `canonical-domain` dependency claim against the code — it is still reachable through the retained `org_unit_binding` seam, so 'unused dep' was corrected to 'Employment usage gone; DispatchTarget still binds it'.",
    "Essentialism / YAGNI": "Only the Employment seam is retired; the `org_unit_binding` seam is left in place (its retirement is the separate P2 bead console-ptmi), and no speculative shared-port-crate abstraction was introduced beyond the single consumer-declared trait.",
    "Chesterton's Fence": "The `#[path]` seam existed because adapter→adapter Cargo edges are illegal under the layer-boundary gate; port-routing preserves that invariant by declaring the trait in the consumer and wiring it in the composition root, honoring the fence's reason instead of deleting it.",
    "Red Team": "Modeled the unwired-port case (ReassignOrgUnit must fail closed, not silently skip the move), the error-kind drift across the delegation boundary (the 409 conflict text must stay byte-for-byte), and the accidental-recompile regression (the gate test re-detects a `#[path]` of the owner's employment.rs); each is fail-closed or asserted.",
    "Systems Thinking": "Traced the reassign write across three layers — canonical adapter (owner DML) → composition root (trait impl) → orgchange adapter (consumer trait + apply transaction) — and confirmed the layer-boundary gate still passes (173 crates) and writer-ownership still attributes the SQL to the single owner (294 files).",
    "Operability / Day-2": "The fail-closed default turns a forgotten wiring into a loud 500 at apply time rather than silent data loss; the moved test file and the new gate test are wired into Buck/cargo so they keep running; the tip stays unfrozen until hosted checks pass.",
    "Blast-radius / cell-based": "The Employment DML never leaves the canonical adapter (single writer); the orgchange adapter only loses the seam and gains a consumer-declared trait; non-reassign proposals still effectuate through the unwired `new()` path, and the test-file move is in place (no new DML).",
    "Zero-trust / defense-in-depth": "Four independent guards on one seam: the composition root injects the only impl, the trait default is `None` (fail-closed), the layer gate forbids adapter→adapter edges, and writer-ownership still requires a single production DML owner."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "The orgchange adapter compiled the canonical adapter's `employment.rs` via `#[path]`, giving it a second copy of the Employment DML the layer-boundary gate could not see as an adapter→adapter edge.",
    "Retiring the seam without an injected port would force either an illegal adapter→adapter Cargo edge or a duplication of the owner DML into the orgchange crate.",
    "An unwired store risked silently skipping the employee move; the fail-closed `None` default makes ReassignOrgUnit refuse with an internal error instead.",
    "The error-kind flattening across the delegation boundary (`CanonicalPortError` → `KernelError::internal(message)` → `KernelError::conflict`) could drift the 409 text; it is preserved byte-for-byte but only guarded by the existing 409 tests, not a dedicated byte-equality test.",
    "`console-ontology-canonical-domain` remains a dependency of the orgchange adapter through the retained `org_unit_binding` seam (`DispatchTarget`), so the Employment-side usage is gone but the dep is not yet removable.",
    "The `org_unit_binding` `#[path]` seam (OrgUnit owner DML compiled into orgchange) remains and is tracked as follow-up bead console-ptmi.",
    "The unwired-port `KernelError::internal` was flattened to a 409 by `apply_ops`'s blanket per-op conflict mapping, hiding a composition defect as a client conflict; a pre-loop fail-closed check now preserves the 500.",
    "The moved employment test was left stale in `postgres-cargo-map.json` and `executed-tests-baseline.json`, so the canonical suite would execute nowhere; both entries were moved with the file."
  ],
  "decisions_changed_or_rejected": [
    "Rejected adding a Cargo edge from orgchange → canonical adapter (adapter→adapter is illegal under the layer-boundary gate); the consumer-declared trait + composition-root wiring was chosen instead.",
    "Rejected moving or deleting the Employment DML; the owner stays the canonical adapter and the test file moved with it so the DB suite exercises the owner directly.",
    "Rejected removing `console-ontology-canonical-domain` from the orgchange adapter in this lane: it still resolves `DispatchTarget` for `org_unit_binding`, and a Cargo.toml edit is out of lane scope.",
    "Rejected the review's ledger-binding claim on PR #781: it cited a nonexistent commit `745d154f` authored by `Codex <codex@openai.com>`; the actual chain is the signed `C`/`T` pair verified with `git verify-commit`."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
