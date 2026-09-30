# Wave 0 prep — D-APR (approval / preflight / distinct-human)

Status: **prep only** — no implementation code in this pack. Unlocks domain-increment admission when base SHA, writer identity, security reviewers, and trial receipt are filled at start of work.

## Identity

| Field | Value |
|---|---|
| Lane id | `w0-apr` |
| Bead | `console-7vm` |
| Epic | `console-66n` |
| Domain | D-APR — Governance approval, four-eyes, lifecycle preflight (non-mutating) |
| Risk class | **high (authz / approval)** — distinct-human four-eyes, self-approval ban, preflight non-mutation. Mandatory security review. |
| Base SHA | *fill at admission* |
| Writer | *fill at admission* — single implementer; two independent adversarial reviewers |

## Outcome (first increment)

Land a **bounded** governance hardening that proves or tightens:

1. **Preflight non-mutation** — lifecycle preflight uses the same gate evaluation inputs as execute intent and **writes nothing** (status/outcome only);
2. **Distinct-human four-eyes** — requester and approver are different natural-person principals (`approver_id <> requested_by` at domain, store, and DB CHECK);
3. No frontend approval UI work;
4. Targeted cargo tests green with counts.

Non-goals: multi-stage MSMP-style routing product, SoD toxic-combination engine, frontend ApprovalCompose, ontology schema changes, policy engine promotion, compliance legal conclusions.

## Allowlist (strict writable roots)

**Writable:**

- `backend/crates/governance/**`

**Cargo packages (facts from Cargo.toml):**

| Path | Package |
|---|---|
| `backend/crates/governance/domain` | `console-governance-domain` |
| `backend/crates/governance/application` | `console-governance-application` |
| `backend/crates/governance/adapter-postgres` | `console-governance-adapter-postgres` |
| `backend/crates/governance/rest` | `console-governance-rest` |

**REST paths owned by this surface (fact from rest crate):**

- `POST /api/v1/governance/overrides`
- `POST /api/v1/governance/approvals`
- `POST /api/v1/governance/approvals/decide`
- `POST /api/v1/governance/lifecycle/transitions`
- `POST /api/v1/governance/lifecycle/preflight` — **status only, never commits** (module docs)

**Forbidden:**

- Frontend / Leptos / web approval UIs
- `backend/crates/ontology/**` (w0-ont) — even though ontology execute *consumes* gates
- `backend/crates/policy/**` and platform Cedar engine (w0-pol)
- Consumer modules that call four-eyes (docs, consulting, orgchange, etc.) unless a later admitted expansion — **default out of scope**
- Migrations, lockfiles, OpenAPI, CI, PRODUCT/ROADMAP authority edits

## Single-writer OWNERSHIP

See sibling [`OWNERSHIP.tsv`](./OWNERSHIP.tsv). Approval request, decision, consume, and preflight evaluation each have one writer module. Preflight must not become a write path.

## HOLDs still in force

- **Frontend** — HOLD.
- **Projection fan-out / JobPosition** — HOLD.
- **Production / compliance / Korea conclusions** — HOLD.
- **True preflight + distinct-human approval rules** are ROADMAP architecture foundations work — this lane may **implement backend evidence** toward them; this pack does not claim ROADMAP item completion until candidate SHA + independent review + DELIVERY receipt.
- Lane-protocol product writer fan-out preparation gate — still HOLD for projection pilots.

PRODUCT invariant (normative, not a legal claim): *Requester and approver are distinct natural persons even when their capacities differ.* *Preflight uses the same authorization, policy, state, revision, and input validation as execute and performs no mutation.*

## Acceptance criteria (first increment — small, measurable)

1. **Path discipline:** diffs only under `backend/crates/governance/**`.
2. **Preflight non-mutation:** lifecycle preflight handler / domain evaluation path does not insert/update/delete approval or lifecycle rows. Measurable by: unit test of pure `evaluate_gate_chain` + existing/adapted integration test that asserts no row change after preflight call (or code-level proof that preflight only SELECTs + pure eval). Do not weaken to “best effort.”
3. **Distinct-human four-eyes:** self-approval rejected at store **and** (where present) DB CHECK; distinct approver can decide. Existing tests:
   - `governance/adapter-postgres/tests/governance_rls_as_runtime_role.rs` (`self_approval_is_rejected`, `distinct_approval_is_appended_and_immutable`)
   - `governance/adapter-postgres/tests/approvals_create_as_runtime_role.rs`
   - `governance/adapter-postgres/tests/four_eyes_bind_consume.rs` (bind + single-use consume)
   These remain green; any new logic preserves three-layer ban (domain + store + DB).
4. **No client-trusted requester spoof:** decide path continues to treat pending request’s `requested_by` as authoritative over client-supplied field (application docs).
5. **Security review** recorded for the candidate SHA.
6. **Tests green** with exact commands and counts.
7. **No PRODUCT HOLD clear; no frontend.**

Suggested first-increment scope (inference — pick one):

- Pure-domain tests that preflight-equivalent `evaluate_gate_chain` is side-effect free and fail-closed on missing evidence; **or**
- Tighten/document+test preflight REST path so unconfigured edges remain deny; **or**
- Extend four-eyes bind/consume coverage for a missing edge case without expanding crate allowlist.

## Verification commands

```sh
# Pure domain (gate chain + lifecycle FSM — no Postgres)
cargo test -p console-governance-domain --lib
cargo test -p console-governance-application --lib

# Adapter integration (Postgres + runtime role) — four-eyes / distinct human
cargo test -p console-governance-adapter-postgres --test four_eyes_bind_consume -- --test-threads=1
cargo test -p console-governance-adapter-postgres --test approvals_create_as_runtime_role -- --test-threads=1
cargo test -p console-governance-adapter-postgres --test governance_rls_as_runtime_role -- --test-threads=1

# REST unit surface if present
cargo test -p console-governance-rest --lib
```

npm / frontend: **not required** for this allowlist.

## Blast radius

- **In blast:** gov approval tables access patterns, four-eyes open/decide/consume, lifecycle transition config, preflight REST.
- **Out of blast:** ontology action execute writeback (consumes gates but owned by ONT), policy catalog, consumer REST modules (docs hold release, consulting engagement approval, org-change preflight) unless later expanded.
- **Red Team cases:** self-approval via spoofed `requested_by`; replay of consumed approval; preflight that mutates then “returns status”; cross-target approval bind bypass.

## Rollback

- Revert to base SHA.
- No migration numbers from this lane by default.
- If approval rows were written only in disposable test DBs, no prod rollback.

## Stop conditions

- Write outside `backend/crates/governance/**`
- Preflight gains any write/commit side effect
- Self-approval ban weakened at any layer
- Consumer-module expansion without new admission
- Migration/lockfile/OpenAPI/CI/authority need
- Test weaken/skip without approved receipt
- Missing dual adversarial security review
- Collision with w0-ont (control_points) or w0-pol (Cedar) requiring shared-root edit

## Pre-mortem

| Failure mode | Detection | Mitigation |
|---|---|---|
| Preflight mutates | dry-run / row-count test | stop; redesign to pure SELECT + eval |
| Self-approval slip | three-layer tests + DB CHECK | keep all three; never rely on UI |
| Cross-target reuse | four_eyes_bind_consume | bind kind+target; single-use consume |
| Scope into ontology execute | path allowlist | hand off to w0-ont |

## Review (mandatory)

High-risk approval work:

- 1 implementer, 2 independent adversarial reviewers, distinct integrator when needed
- Lenses: Red Team, Operability/Day-2, Blast-radius, Zero-trust/defense-in-depth

## Evidence artifacts

- This pack: `admission.md`, `OWNERSHIP.tsv`, `trial.md`
- Security review bound to candidate SHA
- Lane ledger with remaining HOLDs
