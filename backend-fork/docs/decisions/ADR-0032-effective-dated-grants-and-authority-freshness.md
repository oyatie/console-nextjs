---
id: ADR-0032
status: accepted
doc_status: review
date: 2026-07-30
owner: jasonlee
decision: effective-dated-grants-scoped-replay
related: [ADR-0002, ADR-0003, ADR-0021]
---

# ADR-0032 — Effective-dated grants, and what replay actually guarantees

## Status

Accepted 2026-07-30, `doc_status: review`. Amends nothing and supersedes nothing; `related`
only. It grants no authority, widens no exposure, and weakens no gate. It makes no Korean compliance
claim: the six Korea controls remain `HOLD`.

## Context

A 발령 (appointment) has a date. Today a role grant does not. `user_role_assignments`
(`backend/crates/platform/db/migrations/0065_create_policy_roles.sql:141-152`) carries `created_at`
and nothing else temporal, with `UNIQUE (org_id, user_id, role_id)` at `:148`, so the row records
when a clerk typed it, never when the authority began. Backdating a grant, pre-loading next
quarter's 발령, and 소급 정정 of a mis-entered 발령일 are all inexpressible.

Adding `valid_from`/`valid_to` to that one table is cheap. The trap is what it appears to buy.
"Effective-dated grants" reads as "we can now replay who could do what on any past date," and that
inference is false, because the grant is one of seven inputs the authority fold reads and the other
six are read at head. `resolve_effective_feature_grants_in_org`
(`backend/crates/platform/authz/src/lib.rs:1248-1386`) folds:

| # | Input | Read at | History today |
|---|---|---|---|
| 1 | `users.team` | `authz/src/lib.rs:1262` | none — plain column, `0002_create_users.sql:16` |
| 2 | branch membership → `live_branch_scope` | `authz/src/lib.rs:1495` | none — `user_branches` is PK-only, `0002_create_users.sql:23-27` |
| 3 | `user_role_assignments` | `authz/src/lib.rs:1273` | none — `0065:141-152` |
| 4 | `policy_roles.status = 'ACTIVE'` | `authz/src/lib.rs:1282` | none — head status |
| 5 | `policy_role_permissions` | `authz/src/lib.rs:1272`, joined `:1277-1279` | none |
| 6 | `policy_role_conditions` | `authz/src/lib.rs:1315-1316` | none |
| 7 | `users.roles` (system-role arm, unioned at `authz/src/lib.rs:1192-1196`) | principal | none — `0002_create_users.sql:12-14` |

An as-of fold over an effective-dated row 3 and head-valued rows 1, 2, 4, 5, 6, 7 does not return a
past answer. It returns a fluent present-tense answer wearing a past date, which is strictly worse
than refusing: an incomplete answer invites a second question, a confidently wrong one does not.
The repository already states the principle one level down, for money rather than authority:
`resolve_derived_attributes_tx` reads referents as of `valid_from` and **fails** the write when a
referent has no revision at that instant, because dropping the term "would store a smaller, entirely
plausible total" (`backend/crates/ontology/adapter-postgres/src/instances.rs:1129-1141`, predicate
`:1228-1229`, typed refusal naming the offending instance `:1246-1252`).

ADR-0021 is commonly read as blocking this work, and does not. Verified against code, not prose:
`bundle_digest` is `hex(sha256(schema_src ‖ policy_src))` with no grant row as an input
(`backend/crates/platform/authz/src/cedar_pbac/engine.rs:490-496`); `CompiledBundle` documents itself
as "bundle identity + compiled artifacts, not a decision cache" (`:136-140`); entities are built per
request at `:449`, immediately before `is_authorized` at `:460`. ADR-0021's §4 bundle-cache rule
(`ADR-0021:55-56`) and §5 freshness rule (`:57-60`) are therefore what make effective-dating
**safe**, not what forbids it. A wall-clock authority predicate already ships and already coexists with them:
`clearance_assignments` is read under `status = 'ACTIVE' AND starts_at <= now() AND (expires_at IS
NULL OR expires_at > now())` on every request
(`backend/crates/compliance/adapter-postgres/src/lib.rs:615-623`).

One correction to the record this decision was drafted against. The claim that the
`authz_subject_version` token is not written today is **false**: `bump_subject_version_tx`
(`backend/crates/identity/adapter-postgres/src/lib.rs:2258-2279`) has three live call sites —
system-role/branch replacement `:304`, account activate/archive `:672`, and custom-role assignment
replacement `:1606` — and `:1606` runs inside the `with_audit` transaction opened at `:1540`. A
custom-role grant is written, and a grant change does bump the token. What is genuinely absent is
different and sharper, and clause 6 below states it.

## Decision

1. **The interval goes on the grant, and only on the grant.** `user_role_assignments` gains
   `valid_from TIMESTAMPTZ NOT NULL` and `valid_to TIMESTAMPTZ NULL`; the fold's join
   (`authz/src/lib.rs:1273-1284`) gains the same half-open predicate `PgInstanceStore::get_as_of`
   already uses (`instances.rs:374-375`); `UNIQUE (org_id, user_id, role_id)` (`0065:148`) becomes a
   per-`(org_id, user_id, role_id)` non-overlap constraint.
2. **Grants stay in `user_role_assignments` under `with_audit`** and are not migrated into
   `ont_instance_revisions`. That store's append-only trigger permits only `valid_to` to be set and
   refuses any change to `valid_from`
   (`backend/crates/platform/db/migrations/0155_create_ontology_instances.sql:141`, raised at
   `:149-151`), which would make 소급 정정 of a mis-entered 발령일 permanently inexpressible.
3. **The fold is computed per request, never materialized, never cached across requests** — citing
   `ADR-0021:55-56` as the enabling reason, so no later slice re-opens materialization as an
   obstacle or as an optimization.
4. **No `effective(person, scope, as_of)` read is offered.** An as-of authority read is admissible
   only over inputs that are all as-of-capable. Six of seven are not. This record also declines the
   narrower `delegation_rule as_of D` read that has been proposed elsewhere, for the plainer reason
   that no `delegation_rule` relation exists anywhere in the migration set today.
5. **When an as-of authority read is eventually offered, a missing input history is a typed refusal
   that names the unavailable input** — following `instances.rs:1246-1252` and `get_as_of`'s
   `:382-383`. Silently folding over a head value, or dropping a term, is forbidden.
6. **A freshness counter cannot police an interval, so the interval predicate is load-bearing on
   every request.** `SubjectFreshness::satisfies`
   (`backend/crates/platform/authz/src/cedar_pbac.rs:68-78`) compares monotone counters, and
   `bump_subject_version_tx` fires only inside a write transaction. Crossing `valid_from` or
   `valid_to` is not a write, so no bump exists to record it and no counter comparison can detect
   it. A grant's interval must therefore never be cited as a reason to skip a per-request
   evaluation, and expiry must be enforced by the read predicate rather than by a status-flipping
   job. This is the shipped `clearance_assignments` shape (`compliance/adapter-postgres:615-623`),
   which is also why that table needs no bump: it has no non-test writer at all today, its only
   `INSERT` being `backend/app/tests/compliance_api.rs:557`.
7. **Negative invariant, binding on every consumer.** Any answer folded over head-valued inputs 1,
   2, 4, 5, 6 or 7 is not a historical answer and must not be presented as one — not in an audit
   export, not in a Policy Studio preview, not in a 감사 response, not in a UI date picker.

Acceptance conditions, not follow-ups: the non-overlap constraint must be verified to be
constructible before the migration is written. `EXCLUDE USING gist` needs `btree_gist`, and no
migration in this repository has ever created it — `CREATE EXTENSION` appears only for `pgcrypto`
(`0021_auth_otp_hardening_and_coldstart.sql:27`, `0023_coldstart_revoke_fixed_seed.sql:29`) and
`pg_trgm` (`0020_equipment_autocomplete_trgm.sql:14`, `0118_search_trgm_indexes.sql:7`). If the
extension is unavailable, the constraint is a trigger-enforced non-overlap check in the same
migration, never a dropped constraint.

## Alternatives considered

### Temporalize all seven inputs and build true historical replay

Rejected as unbuildable now. Inputs 1, 2 and 7 are columns on `users` and rows in `user_branches`
with no revision store; giving them one is a schema programme, not a clause. Deferring the promise
costs a refusal; making it costs a wrong answer in a 감사 response.

### Leave grants instantaneous and express 발령일 by scheduling the write

Rejected. It moves the effective date into an operator's calendar, where no constraint reaches it,
and leaves the audit trail recording the clerk's keystroke date as the date authority began.

### Move grants into `ont_instance_revisions` to inherit the temporal machinery

Rejected on clause 2's trigger. Inheriting an append-only `valid_from` costs the one correction a
발령 record most needs.

### Materialize the fold into a cached effective-grants table keyed by version/digest

Rejected twice over: it is the cross-request allow/deny cache `ADR-0021:56` forbids, and a
materialized row cannot notice that an interval boundary passed while it sat there.

### Offer `effective(person, scope, as_of)` and document the historyless inputs as a caveat

Rejected. A caveat in a document does not travel with the row it qualifies. Clause 7 is the same
warning placed where it is enforceable.

## Why this one

It costs one predicate discipline and one error path, zero new tables, and turns the untemporalized
backbone from a blocker into an **enforced** coverage boundary rather than a documented one. It also
buys the cheap direction: each later input that gains history widens the as-of surface by itself,
with no re-litigation, because clause 4 already states the admission rule.

## Consequences

- 발령 backdating, pre-dated grants, and 소급 정정 become expressible on the one table where the
  authority actually lives, under the existing `with_audit` discipline.
- The fold gains a time predicate and an interval-overlap constraint; both are per-request costs on
  a path that already runs per request, so no cache is introduced and none is removed.
- Effective-dating narrows authority as often as it widens it: a grant whose `valid_to` has passed
  stops folding immediately, with no revocation write and no session invalidation.
- − Consumers who read "effective-dated grants" as "historical replay" will be refused, and the
  refusal names the missing input rather than degrading to a plausible number. That is the intended
  cost.
- − Two authority-relevant inputs (`users.team`, branch membership) remain head-valued after this
  lands, so the coverage boundary is real and visible rather than closed.
- − `ADR-0021:58`'s subject-input list names "responsibility", and no `responsibility` table or
  column exists in the migration set, so that term names nothing enforceable. This record does not
  repair the list; it records the gap.

## Reciprocity landed on acceptance

This record amends nothing, so no target ADR gained `amended_by`, and no existing Decision
sentence became false by its acceptance. The reciprocal edits landed in the same atomic commit as
acceptance:

- `docs/decisions/ADR-0002-auditfirst-transactional-discipline-audit-event-in.md` frontmatter:
  `related` gained `ADR-0032`.
- `docs/decisions/ADR-0003-branchscoped-authorization-model-nonnull-branch-scope.md` frontmatter:
  `related` gained `ADR-0032` (its `related` was `[]` before ADR-0028's acceptance).
- `docs/decisions/ADR-0021-cedar-pbac-authorization-strangler.md` frontmatter: `related` gained
  `ADR-0032`.
- `docs/decisions/README.md`: the ADR-0032 index row's status cell changed from `proposed` to
  `accepted`, and the "Effective relationship graph" section gained one line recording that ADR-0032
  reads ADR-0021 §4/§5 as enabling effective-dated grants and adds a scoped-replay invariant without
  changing ADR-0021's scope.

One clarification, not an amendment: this record relies on ADR-0002's `with_audit`
same-transaction rule, which is executable. It does not rely on that ADR's audit-coverage
exclusion-set count, which as drafted did not match the gate
(`backend/ci/gates/audit-coverage/src/lib.rs:90-106` returns two exclusions and
`backend/ci/gates/audit-coverage/tests/gate_detects_violation.rs:26-28` asserts two). That
sentence was reconciled by a separate record, ADR-0029, accepted the same day.

## Verification baseline

Cheap, re-runnable, no build required. Each check fails loudly if the premise this record rests on
stops holding:

- `rg -n 'valid_from|valid_to' backend/crates/platform/db/migrations/0065_create_policy_roles.sql`
  — empty today; non-empty once clause 1 lands.
- `rg -n 'CREATE EXTENSION' backend/crates/platform/db/migrations/` — must show `btree_gist` before
  any `EXCLUDE USING gist` appears.
- `rg -n 'SELECT team FROM users|FROM user_branches' backend/crates/platform/authz/src/lib.rs` —
  while these are head reads, clause 7's negative invariant stays binding.
- `rg -n 'not a decision cache' backend/crates/platform/authz/src/cedar_pbac/engine.rs` — the
  no-decision-cache property clause 3 depends on.
- `rg -c 'bump_subject_version_tx' backend/crates/identity/adapter-postgres/src/lib.rs` — a drop in
  call sites means a grant path stopped sourcing freshness.

## Non-goals and open questions

- Temporalizing `users.team`, `user_branches`, `users.roles`, or the role definition tables. Each is
  its own decision; clause 4 admits each to the as-of surface automatically when it arrives.
- The 발령 approval workflow that would author an interval. This record decides the storage and
  read shape only.
- Whether `valid_from` is date- or instant-granular in the UI. The column is `TIMESTAMPTZ`; the
  authoring surface may constrain it further.
- Reconciling `ADR-0021:57-60`'s subject-input list with the inputs that exist.
