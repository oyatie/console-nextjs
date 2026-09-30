# Authority tip — L4-EMPL-RETARGET: Employment owner retargeted to the canonical adapter

**Date:** 2026-08-14
**Kind:** authority tip ledger bound on candidate C; T adds this ledger entry only
**Base:** `dea1f91bf6c336319fc718f9d9f3eb2c2047f63c` (origin/main)
**Candidate C and Tip T:** both signed by the pinned authority (principal `jason19931225@gmail.com`, ED25519 `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`). Commit/tree SHAs are recorded in the post-merge readback ledger update.
**Scope:** the Employment DML relocation (`backend/crates/orgchange/adapter-postgres/src/employment.rs` → `backend/crates/ontology/canonical-adapter-postgres/src/employment.rs`), the contract owner string, the writer-ownership gate catalog, and app import sites. No migration, no new dependency, no Cargo.toml/Cargo.lock, no OpenAPI or CI change.
**Not product authority.** Clears no HOLD and authorizes no production, credential, compliance, payment, erase, OCI, frontend, projection, or Oyatie action.

## Summary

- Fulfilled the `canonical-domain` doc-comment promise: `ObjectKey::Employment`'s owner is now `console-ontology-canonical-adapter-postgres` (was the interim `console-orgchange-adapter-postgres`).
- The DML moved **byte-identical** (diff of the moved file against origin/main's orgchange version is empty); org-change reassignment still reaches the same code through a `#[path]` seam in the orgchange adapter, mirroring the existing `org_unit_binding` seam — no second writer is created.
- The writer-ownership gate catalog and its tests were retargeted; a new RED→GREEN test `employment_is_owned_by_the_canonical_adapter` pins the owner string AND fails if the orgchange adapter regains production DML against an employment table.
- `ObjectKey` remains locked at six keys (`six_projected_stable_object_keys_verbatim` unaffected); the table roster is unchanged.
- Buck faces: the relocation left the orgchange adapter's `test.unit` resource metadata stale (generated-faces gate red). Fixed by updating `TEST_RESOURCE_REQUIREMENTS` in `tools/buck/gen_first_party.py` (orgchange unit pins removed; canonical-adapter comment updated) and regenerating (173 first-party BUCK files, 2 changed: `backend/crates/orgchange/adapter-postgres/BUCK` + the generator). Also refreshed two stale pins in `tools/buck/test_gen_first_party.py` (console-app inline tests 153→162, sqlx 18→22) and feature-gated 9 ungated inline tests (`audit_chain_signer.rs` ×8, `lib.rs` ×1) so the variant-gate test passes. Evidence: `python3 tools/buck/test_gen_first_party.py` 29/29 at the committed tree; `cargo test -p console-app --lib audit_chain` 8/8.

## Verification

- RED baseline: `left: "console-orgchange-adapter-postgres" / right: "console-ontology-canonical-adapter-postgres"` on the new gate test before implementation.
- `cargo fmt -p console-gate-writer-ownership -p console-orgchange-adapter-postgres -p console-ontology-canonical-adapter-postgres -p console-ontology-canonical-domain -p console-app -- --check`: clean.
- `cargo clippy -p console-gate-writer-ownership -p console-orgchange-adapter-postgres -p console-ontology-canonical-adapter-postgres -p console-ontology-canonical-domain --all-targets -- -D warnings`: clean.
- `cargo test -p console-gate-writer-ownership --test gate_detects_violation employment_is_owned_by_the_canonical_adapter`: 1/1 (independent re-run after the lane's own run).
- Lane runs (SQLX_OFFLINE): gate 10 lib + 47 integration + 3 census (Docker) pass; canonical-domain 11; canonical-adapter --lib 6; orgchange --lib 4; app --lib 162; layer-boundary 19 + 13.
- DB-backed `#[sqlx::test]` employment tests require a DATABASE_URL and were not re-run locally; the census Docker suite ran 3/3 on the lane host. CI executes the full matrix.

## Safety controls

**Pre-mortem** — what could break and how it would manifest:

- **SQL drift:** if the moved `employment.rs` were not byte-identical to origin/main's orgchange version, Employment writes could silently change behavior. Contained because the byte-compare against origin/main is empty (recorded above), and the gate/census pin the single-writer boundary.
- **Second writer:** the interim `#[path]` seam still compiles the module into both the canonical and orgchange crates, which could weaken the crate-level writer boundary. Contained by `employment_is_owned_by_the_canonical_adapter`, which fails RED if the orgchange adapter regains production DML against an employment table; the seam retires under follow-up `console-pees` (port-routing).
- **Wrong owner string:** if the registry retarget were mistyped, the gate would attribute Employment writes to the wrong crate and silently drop enforcement. Contained by the gate test pinning the exact owner string.

**Detection signals:**

- CI `Backend — fmt / clippy / test / gates` runs the writer-ownership gate; `employment_is_owned_by_the_canonical_adapter` (1/1) fails if the owner string reverts or a second production writer appears.
- The writer-ownership census (3/3, Docker) enumerates writers per table and flags any duplicate writer for an employment table.
- Compile-time: all import sites (app and org-change reassignment) are type-checked, so a stale import path fails the build.
- Post-deploy: org-change reassignment against employment tables must behave exactly as before (byte-identical SQL, same transaction ownership); any reassignment regression is an immediate behavioral signal.

**Rollback procedure:**

- Code-only change: no migration, no Cargo.toml/Cargo.lock, no OpenAPI/CI change, and byte-identical SQL, so there is no schema or data to unwind.
- Revert is a pure `git revert` of C and T (restoring the prior orgchange `employment.rs` path, owner string, gate catalog, and app import sites) followed by redeploy of the prior commit. No data rollback is required.

**Stop conditions** (do not merge if any of these fire):

- `employment_is_owned_by_the_canonical_adapter` fails.
- The byte-compare of the moved file against origin/main's orgchange version is non-empty (SQL drift).
- The writer-ownership census reports two writers for any employment table.
- Any Cargo.toml/Cargo.lock/OpenAPI/CI drift appears (out of scope for this lane).
- App import sites or org-change reassignment fail to compile.

## Remaining HOLDs / follow-ups

- All current PRODUCT/ROADMAP HOLDs remain unchanged.
- The canonical-adapter `Cargo.toml` `description` still says four objects (cosmetic; not edited because this lane makes no Cargo.toml changes).
- After both this lane and `console-1qw.5` (PR #774) merge, the P4 epic closes; the P5 epic `console-dgo` then closes with readback evidence.

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
    "Cartesian doubt": "Verified the relocation is byte-identical against origin/main's orgchange source rather than trusting the lane's own claim, and re-ran the gate test independently.",
    "Essentialism / YAGNI": "The seam reuses the established org_unit_binding #[path] pattern instead of a new abstraction, and the moved SQL is untouched — relocation, not refactor.",
    "Chesterton's Fence": "Kept the interim orgchange owner string's gate function (reject additional writers) intact while fulfilling the contract's documented retarget promise.",
    "Red Team": "Modeled the regression the retarget could hide: the new gate test fails both when the roster is retyped and when orgchange regains production employment DML, so a second writer cannot re-enter silently.",
    "Systems Thinking": "Traced every consumer of the employment DML — app imports, org-change reassignment, the gate catalog, and the census — before moving the owner.",
    "Operability / Day-2": "DB-backed behavior is unchanged (byte-identical SQL, same transaction ownership); CI executes the full matrix, and the residual cosmetic Cargo.toml description is recorded, not hidden.",
    "Blast-radius / cell-based": "The change is confined to the two adapter crates, the contract registry, the gate, and import sites; no migration, lockfile, or workflow surface.",
    "Shared-nothing / eventual consistency": "Exactly one production writer per employment table after the move; the gate proves it, and the seam is a compile-time inclusion, not a runtime dispatch.",
    "Zero-trust / defense-in-depth": "The owner registry is the trust anchor for writer enforcement; the retarget keeps the gate's deny-by-default posture and the new test pins the trust boundary itself."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "The Employment owner string had remained at the interim console-orgchange-adapter-postgres owner after the EmploymentPort lane landed, leaving the contract doc-comment promise unfulfilled.",
    "Byte-compare confirmed the moved file is identical to origin/main's orgchange version, so the relocation cannot have smuggled SQL or audit changes."
  ],
  "decisions_changed_or_rejected": [
    "Rejected copying the DML (duplicate code) in favor of the #[path] seam already established for org_unit_binding.",
    "Rejected editing the canonical-adapter Cargo.toml description in this lane because Cargo.toml changes are out of scope; recorded as a follow-up instead."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
