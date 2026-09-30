# Wave 0 prep — D-ONT (ontology / Foundry backend)

Status: **prep only** — no implementation code in this pack. Unlocks domain-increment admission when base SHA, writer identity, and trial receipt are filled at start of work.

## Identity

| Field | Value |
|---|---|
| Lane id | `w0-ont` |
| Bead | `console-g1n` |
| Epic | `console-ssf` |
| Domain | D-ONT — Ontology / Foundry-style object engine (backend only) |
| Risk class | **standard–high** (product substrate; not pure authz, but shared schema/lifecycle surface). Escalate to high-risk review if the increment touches gate chains, migrations, or projected writers. |
| Base SHA | *fill at admission* — immutable commit this lane branches from |
| Writer | *fill at admission* — single human/agent identity |

## Outcome (first increment)

Land a **bounded backend-only** ontology hardening or pure-layer fix inside `backend/crates/ontology/**` that:

1. does not open UI / frontend work;
2. does not clear any PRODUCT HOLD;
3. does not add alternate write paths for projected objects;
4. leaves the targeted cargo suite green.

Non-goals for this Wave 0 increment: Leptos/UI, Company/Person/Employment/PayRun projection fan-out, JobPosition fan-out, migrations ownership (serialized elsewhere), OpenAPI/CI/lockfile edits, production claims.

## Allowlist (strict writable roots)

**Writable:**

- `backend/crates/ontology/**` only

**Cargo packages (facts from Cargo.toml):**

| Path | Package |
|---|---|
| `backend/crates/ontology/domain` | `console-ontology-domain` |
| `backend/crates/ontology/application` | `console-ontology-application` |
| `backend/crates/ontology/adapter-postgres` | `console-ontology-adapter-postgres` |
| `backend/crates/ontology/rest` | `console-ontology-rest` |

**Forbidden (this lane):**

- Any path outside `backend/crates/ontology/**`
- Frontend / `web/**` / Leptos
- `docs/current/PRODUCT.md`, `ROADMAP.md`, `DELIVERY.md` (authority expansion)
- Migrations under `backend/crates/platform/db/migrations/**` (serialized owner)
- Lockfiles, OpenAPI generated faces, CI workflows
- Governance / policy / platform-authz crates (other Wave 0 lanes or shared writers)
- Projected domain writers (HR, payroll, registry, etc.)

Read-only coordination with other crates is allowed for compile/test linkage; **no writes**.

## Single-writer OWNERSHIP

See sibling [`OWNERSHIP.tsv`](./OWNERSHIP.tsv). One writer module per fact object; no dual write paths for instances, schema registry rows, or action execute.

## HOLDs still in force

From current product/roadmap authority (not cleared by this pack):

- **Frontend / Leptos** — HOLD until PRODUCT frontend conditions and ADR-0030 gates are satisfied.
- **Company / Person / Employment / PayRun projection fan-out** — HOLD until each has an explicit owning port and proven single-writer boundary.
- **JobPosition and projection fan-out** — HOLD (ROADMAP).
- **Live production, DNS, TLS, secrets, exposure, payment, credential-reset, compliance claims** — HOLD without separate authority.
- **Korea compliance conclusions** — HOLD pending qualified authority.
- **Bulk documentation moves / custody erase / OCI Ampere destroy** — permanent or gated HOLDs as in PRODUCT/ROADMAP.
- **Lane-protocol preparation gate** (owning ports + product reference + catalog promotion) — still HOLD for product writer fan-out; this Wave 0 pack is backend-ontology substrate prep only and does **not** authorize JobPosition/projection pilots.

Historical plans and this prep pack do not clear HOLDs.

## Acceptance criteria (first increment — small, measurable)

1. **Path discipline:** `git diff --name-only` against base shows only paths under `backend/crates/ontology/**` (plus this lane-prep receipt if co-landed under docs policy).
2. **No HOLD clear:** No edit claims or implies frontend, projection fan-out, or compliance/production clearance.
3. **Single-writer preserved:** No new alternate write path for projected objects; ontology remains registry + generic instance substrate (domain invariant per PRODUCT).
4. **Tests green:** Targeted package tests pass (see Verification). Record discovered/executed counts.
5. **Pure-layer preference:** Prefer domain/application unit coverage first; adapter/rest integration only if the AC requires runtime evidence.
6. **Receipt:** Lane ledger (when work starts) records base SHA, head SHA, verification commands, reviewers, remaining HOLDs.

Suggested first-increment scope (inference — pick one at admission, keep tiny):

- Harden a pure-domain lifecycle/schema invariant with unit tests in `console-ontology-domain` / `console-ontology-application`; **or**
- Close one measured residual in existing ontology adapter/rest tests without expanding allowlist.

## Verification commands

From repo root (Cargo workspace), after implementation:

```sh
# Pure layers (no Postgres required for unit tests)
cargo test -p console-ontology-domain --lib
cargo test -p console-ontology-application --lib

# Adapter + REST (many `*_as_runtime_role` tests need disposable Postgres)
cargo test -p console-ontology-adapter-postgres --lib
cargo test -p console-ontology-rest --lib

# Optional focused integration (Postgres + runtime role) — only if AC needs it
# cargo test -p console-ontology-adapter-postgres --test key_write_cas_as_runtime_role -- --test-threads=1
# cargo test -p console-ontology-rest --test object_type_lifecycle_over_http -- --test-threads=1
```

npm / frontend gates: **not required** for this allowlist (no UI). Do not run or claim full `npm run verify` as a gate for ontology-only backend unless docs/authority paths also change.

Record: exact command lines, revision, toolchain, pass/fail counts. A ran-nothing result is not green.

## Blast radius

- **In blast:** ontology registry types, instance revision paths, action preflight/execute REST, ontology Postgres adapter, ontology-only tests.
- **Out of blast (must not regress via this lane):** platform migrations, Cedar live enforcement promotion, governance four-eyes, HR/payroll writers, frontend authz.
- **Second-order risk:** rest layer already composes governance gate config (`parse_control_points`); changes that loosen fail-closed control-point parsing require high-risk review and may need APR/POL coordination — **stop** and re-scope if so.

## Rollback

- Revert the lane branch / PR to base SHA.
- No migration numbers assigned by this lane ⇒ no migration rollback.
- If a shared integration tree temporarily included the change: restore ontology paths only; do not reverse unrelated lanes.

## Stop conditions

Stop and return to integration/owner when any of:

- Write outside `backend/crates/ontology/**`
- Need for migration, lockfile, OpenAPI, CI, or authority-doc change
- Need to clear or weaken a PRODUCT/ROADMAP HOLD
- Projected-object second write path appears
- Test weakening, skip, or quarantine without approved receipt
- Overlap with w0-pol / w0-apr ownership facts
- Stale base vs integration head
- Fail-closed security gate change without independent security review

## Pre-mortem (concise)

| Failure mode | Detection | Mitigation |
|---|---|---|
| Scope creep into UI or projections | path allowlist check on PR | reject; re-open under correct HOLD path |
| Dual writer for instances | ownership TSV + review | single module only in OWNERSHIP.tsv |
| False green (no tests run) | require executed counts | trial.md + ledger receipt |
| Collision with POL/APR | ownership map | serialize; no shared root edits |

## Review

- Standard review for pure domain/application.
- If control-points, authz residual, or lifecycle gate semantics change: treat as high-risk (implementer + independent adversarial review per DELIVERY).
- This pack does not name legal/compliance conclusions.

## Evidence artifacts (at admission time)

- This directory: `admission.md`, `OWNERSHIP.tsv`, `trial.md`
- Post-work: `docs/program/ledger/<lane-id>.md` with head SHA and remaining HOLDs
