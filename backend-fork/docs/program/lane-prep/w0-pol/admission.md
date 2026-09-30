# Wave 0 prep — D-POL (policy / Cedar fail-closed)

Status: **prep only** — no implementation code in this pack. Unlocks domain-increment admission when base SHA, writer identity, security reviewers, and trial receipt are filled at start of work.

## Identity

| Field | Value |
|---|---|
| Lane id | `w0-pol` |
| Bead | `console-93w` |
| Epic | `console-a80` |
| Domain | D-POL — Cedar policy catalog/draft staging + fail-closed authoring substrate |
| Risk class | **high (authz)** — authorization, deny-by-omission, fail-closed evaluation. Mandatory security review. |
| Base SHA | *fill at admission* |
| Writer | *fill at admission* — single implementer; two independent adversarial reviewers per DELIVERY high-risk rule |

## Outcome (first increment)

Land a **bounded fail-closed** improvement inside policy domain crates (and only as needed into related Cedar substrate) that:

1. preserves deny-by-omission / forbid-wins semantics;
2. never promotes draft policies to live enforcement without separate governance promotion authority;
3. does not touch frontend Policy Studio UI;
4. leaves targeted cargo tests green with recorded counts.

Non-goals: UI canvas, ADR-0021 full Cedar-only production cutover, SoD ruleset engine, recertification campaigns, OpenAPI/CI/lockfile, compliance legal conclusions.

## Allowlist (strict writable roots)

**Primary writable (default for first increment):**

- `backend/crates/policy/**`

**Cargo packages (facts from Cargo.toml):**

| Path | Package |
|---|---|
| `backend/crates/policy/domain` | `console-policy-domain` |
| `backend/crates/policy/application` | `console-policy-application` |
| `backend/crates/policy/adapter-postgres` | `console-policy-adapter-postgres` |

**Related Cedar/authz — only if the admitted AC cannot be met in `policy/**` alone; each path requires explicit admission amendment + security review:**

| Path | Package | Role |
|---|---|---|
| `backend/crates/platform/authz/**` | `console-platform-authz` | Cedar PBAC engine, residual lowering, coexistence map |
| `backend/crates/platform/authz-rest/**` | `console-platform-authz-rest` | `/policy/*` simulate/authorize/catalog REST |

Default Wave 0 first increment should stay in `policy/**` (catalog/draft models, validation, staging adapter). Treat platform authz as **shared high-risk** — do not edit without re-declaring ownership and reviewers.

**Forbidden:**

- Frontend (`web/**`, PolicyCanvas, Leptos)
- `backend/crates/ontology/**` (w0-ont)
- `backend/crates/governance/**` (w0-apr)
- Migrations (serialized owner)
- Lockfiles, generated OpenAPI, CI workflows
- Authority docs PRODUCT/ROADMAP expansion
- Live-route Cedar enforcement promotion claims without ADR-0021 evidence path

## Single-writer OWNERSHIP

See sibling [`OWNERSHIP.tsv`](./OWNERSHIP.tsv). Draft staging and catalog row lifecycle have one writer each; engine evaluation lives in platform-authz and is **not** dual-owned by this lane unless explicitly amended.

## HOLDs still in force

- **Frontend / Leptos** — HOLD.
- **Cedar live enforcement promotion** — existing authorization + RLS remain live boundaries until per-action enrollment/shadow/evidence/promotion (ADR-0021 coexistence; policy domain crate docs state promotion is a later governance lane). This pack does not clear that.
- **Projection fan-out / JobPosition** — HOLD.
- **Production exposure / compliance claims / Korea conclusions** — HOLD.
- **Custody / OCI Ampere / bulk doc moves** — as in PRODUCT/ROADMAP.

## Acceptance criteria (first increment — small, measurable)

1. **Path discipline:** diffs only under admitted allowlist (`policy/**` default).
2. **Fail-closed:** unknown enum/status/filter or invalid draft material **rejects** (no silent permit / no silent skip of security-relevant validation). Measurable via unit tests in `console-policy-domain` and/or `console-policy-application`.
3. **No runtime promotion:** draft save path must not create `enforced`/`shadow` rows from B16-style staging (domain already encodes `is_runtime_enforced`; preserve and test).
4. **Deny-by-omission preserved:** absence of permit remains deny; forbid still wins where modeled — no weaken to allow-on-error.
5. **Security review:** independent adversarial review recorded before merge (high-risk authz).
6. **Tests green** with exact commands and counts (see Verification).
7. **No UI / no PRODUCT HOLD clear.**

Suggested first-increment scope (inference — choose one at admission):

- Strengthen pure-domain validation for catalog query filters / draft status transitions with unit tests; **or**
- Adapter draft-storage regression that proves fail-closed reject paths (Postgres) without touching platform engine.

## Verification commands

```sh
# Pure layers
cargo test -p console-policy-domain --lib
cargo test -p console-policy-application --lib

# Adapter (includes tests/draft_storage.rs — may need Postgres)
cargo test -p console-policy-adapter-postgres

# Only if platform authz was explicitly admitted into allowlist for this increment:
# cargo test -p console-platform-authz --lib
# cargo test -p console-platform-authz --test policy
# cargo test -p console-platform-authz-rest --test cedar_authoring_rls_as_runtime_role -- --test-threads=1
```

npm / frontend gates: **not required** for backend-only allowlist.

## Blast radius

- **In blast:** Cedar catalog/draft types, draft save orchestration, policy Postgres staging, (if amended) PBAC engine/REST.
- **Out of blast:** ontology instance writers, governance four-eyes consumption, frontend authz mirrors (advisory_ui_only), live route enrollment map without explicit work.
- **Misuse model (Red Team):** crafted draft that auto-enforces; invalid filter accepted as empty-allow; residual collapse-to-allow instead of deny; self-review of policy publish without four-eyes (publish/four-eyes is APR/governance — do not reimplement here).

## Rollback

- Revert lane commits to base SHA.
- No migration ownership ⇒ no migration reverse from this lane.
- If platform authz was touched: coordinated rollback with security reviewer sign-off.

## Stop conditions

- Write outside admitted allowlist
- Any path that promotes draft → enforced/shadow without separate promotion authority
- Frontend touch
- Migration/lockfile/OpenAPI/CI/authority-doc need
- Test skip/weaken without approved receipt
- Engine fail-open change (error → Allow)
- Overlap with w0-ont or w0-apr without serialization
- Missing security review on authz-sensitive diff

## Pre-mortem

| Failure mode | Detection | Mitigation |
|---|---|---|
| Silent allow on invalid policy | unit tests + review | fail-closed asserts |
| Accidental enforcement promotion | status enum tests + review | domain `is_runtime_enforced` guards |
| Shared root collision with authz engine | ownership TSV | amend admission or stop |
| False green | executed counts in trial | refuse admission without counts |

## Review (mandatory)

High-risk authz per DELIVERY / Agents.md:

- 1 implementer
- 2 independent adversarial reviewers
- Distinct fixer/integrator when remediation is needed
- Lenses required: Red Team, Operability/Day-2, Blast-radius, Zero-trust/defense-in-depth

## Evidence artifacts

- This pack: `admission.md`, `OWNERSHIP.tsv`, `trial.md`
- Security review notes bound to candidate SHA (post-work)
- Lane ledger with remaining HOLDs
