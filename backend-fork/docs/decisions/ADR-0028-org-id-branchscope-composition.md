---
id: ADR-0028
status: accepted
doc_status: review
date: 2026-07-30
owner: jasonlee
decision: org-id-branchscope-composition
amends: [ADR-0003]
related: [ADR-0003, ADR-0018, ADR-0021]
---

# ADR-0028: `org_id` × `BranchScope` composition, and capability-derived all-branch scope

## Status

**Accepted 2026-07-30 · review.** This record amends one clause of
ADR-0003 and closes a documented composition gap under
`docs/decisions/README.md:12` (authority rule 6). It ratifies behaviour already shipped; it
authorizes no new behaviour, unholds nothing, and asserts no compliance
conclusion.

## Context

**No accepted ADR says how tenancy and branch scope compose.** `BranchScope`
appears in exactly one ADR line — `ADR-0003:20` — and `org_id` in exactly two
others, `ADR-0021:50` and `ADR-0018:204`. Neither `org_id` line mentions
`BranchScope` and the `BranchScope` line does not mention `org_id`. Under
README.md:12 that is a governance gap to reconcile by decision, not a silent
divergence.

`ADR-0003:20` reads, verbatim:

> `Branch`/`Region` are first-class day-1 schema concepts. Principals carry a
> `BranchScope` (kernel type): `All` for SUPER_ADMIN/EXECUTIVE rollups, an
> explicit branch set otherwise. Repositories filter by scope by default
> (default-deny); cross-branch access is an authorization test fixture (T0.6).
> P1 broadcasts, KPI rollups, wall-boards, and team channels are branch-scoped.

Two things that sentence does are load-bearing and must be handled in one
record, because they are one clause.

**1. It keys `BranchScope::All` to role literals, and the shipped code already
has a second derivation.** The role match is real:
`backend/crates/platform/authz/src/lib.rs:1480` returns `BranchScope::All` when
any role is `Role::SuperAdmin | Role::Executive`. But it is not the only
tenant-side path. `backend/crates/platform/request-context/src/lib.rs:353`
early-returns for `TenantAccessContext::GroupAdmin` and never calls
`resolve_branch_scope_in_org` at all; `resolve_group_admin_tenant_context_principal`
requires the claim roles to equal exactly `{Role::Admin}` (`:392`), re-proves
live ACTIVE membership through `group_admin_member_orgs` (`:405-414`), and then
passes `BranchScope::All` directly into the composition helper (`:421`). With
`AccessScopeLevel::Org` matching the armed org,
`backend/crates/kernel/core/src/access_scope.rs:88` returns `All`, so the
intersect at `authz/src/lib.rs:1541` yields `All`. A principal holding no
SUPER_ADMIN and no EXECUTIVE role therefore holds all-branch scope today. That
divergence predates the ecosystem plan.

**2. It is the anchor for a role literal the plan proposes to delete.** If the
`Role` enum goes, the only documented derivation of `BranchScope::All`
disappears with it, leaving an authoritative record that names a type which no
longer exists. The replacement must be named before the deletion is proposed,
not after.

**What does not change: `Feature` survives.** `Feature::as_str()` is the Cedar
action id —
`backend/crates/platform/authz/src/cedar_pbac/engine.rs:430` builds the action
UID as `entity_uid("Action", request.action.feature().as_str())`, with the
fail-closed comment at `:428-429`. Verified by count: 592 `Feature::`
references across `backend/crates` and `backend/app`, 479 of them outside
`tests/`. `Role` and `matrix_row` are much thinner: 380 `Role::` references,
and `matrix_row` has exactly two sites — its definition at
`authz/src/lib.rs:573` and its single consumer at `:1141`. `Feature` is the
vocabulary; `Role` is a label and `matrix_row` a default table.

**The composition helper already exists and is already correct.**
`effective_branch_scope_for_tenant` (`authz/src/lib.rs:1519-1542`) refuses
`AccessScopeLevel::Group` outright (`:1525-1528`), projects `Region`/`Worksite`
through a missing `BranchProjection` so `access_scope.rs:90-97` fails closed to
`BranchScope::none()`, and returns `live_scope.intersect(&projected_scope)`
(`:1541`) so a token scope can only narrow the live database scope. What is
missing is not the mechanism but the decision saying it is the only one.

**A `BranchScope` predicate is not a tenant predicate.**
`backend/crates/kernel/core/src/branch.rs:26-31` makes `BranchScope::All`
return `true` for every `BranchId`, so a scope filter alone isolates nothing
across tenants. The realtime fan-out is where that matters:
`backend/crates/platform/realtime/src/lib.rs:885` and `:899` filter connection
slots on `slot.principal.branch_scope.allows(branch_id)` and nothing else, and
`dispatch_notification` at `:843` filters on recipient identity alone. The
tenant line there is held only inside `authorized_thread_members`, whose
`with_org_conn(pool, org, …)` membership read at `:1096` is skipped entirely
when a caller supplies `authorized_users` or when `pool` is `None` (`:878`) —
and `PgRealtimeHub::for_tests` is `pub` and sets `pool: None` (`:486-491`), so
the boundary is compiled out in exactly the configuration the unit tests run
in. The fix is cheap because the fact is already present:
`RealtimePrincipal.org_id` exists on every slot (`:402`).

## Decision

1. **`BranchScope::All` is derived at exactly one point in tenant request
   context, from one of two sources only:** a built-in `Feature` capability
   authored in code and not mintable from the console, or a live database
   membership proof resolved at request time. **Never from a role-name
   literal.** The existing role match at `authz/src/lib.rs:1480` is the
   scaffolding this clause replaces; until a built-in all-branch `Feature` is
   seeded and enrolled, it stays in place as the interim implementation of
   source (i) and must not be extended to further role names.

2. **The already-shipped group-admin path is ratified as an instance of source
   (ii).** A principal whose claim roles are exactly `{ADMIN}` receives
   `BranchScope::All` for one subsidiary after `group_admin_member_orgs` proves
   ACTIVE membership (`request-context/src/lib.rs:383-434`). This is recorded as
   pre-existing divergence from `ADR-0003:20` brought under governance per
   README.md:12, not as new authority.

3. **`effective_branch_scope_for_tenant` (`authz/src/lib.rs:1519-1542`) is the
   sole legal composition of a token scope with a live scope.**
   `AccessScopeLevel::Group` is refused on every ordinary tenant route and must
   use a group fan-out resolver (`:1525-1528`). `Region` and `Worksite` project
   to `BranchScope::none()` until a database-backed hierarchy resolver supplies
   a matching `BranchProjection` (`access_scope.rs:90-97`). A claim scope may
   only narrow the live membership scope, never widen it (`:1541`).
   `AccessScopeLevel` stays extensible by migration behind that fail-closed
   default.

4. **A `BranchScope` predicate is never a tenant predicate.** Every fan-out,
   filter, and projection path asserts `org_id` explicitly and may not rely on a
   co-located RLS-armed read for tenant isolation. This binds
   `realtime/src/lib.rs:843`, `:885`, and `:899` by name; the org filter uses
   the `RealtimePrincipal.org_id` already carried at `:402`. `org` remains the
   top fan-out dimension, and the absence of any dimension above it is correct
   under `ADR-0018:231-233`.

5. **`org_id` remains the RLS boundary and gains no second isolation axis.**
   Custom role definitions cannot widen `All` (`docs/specs/rbac-configurable.md:366`,
   unchanged). Cross-tenant facts live only in owner-only tables reached through
   SECURITY DEFINER resolvers, never in a tenant-writable row. Nothing here
   widens `ADR-0021:50`: Cedar may not widen `org_id` or bypass RLS, and this
   record introduces no second GUC.

6. **Competence is a condition attribute on a custom role, not a third relation
   and not a scope-type change.** It takes the shape the `"team"` arm already
   has (`authz/src/lib.rs:1421-1425`): a subject-side predicate that gates
   whether the role applies and leaves `BranchScope` untouched.

7. **The authored condition vocabulary is narrowed at the write path to what
   the runtime resolver evaluates** — attributes `{branch, team}`, operators
   `{equals, in}` (`authz/src/lib.rs:1406`, `:1421`, `:1426`) — with a test
   asserting write-accepted ⊆ resolver-evaluated. The fail-closed whole-role
   void at `authz/src/lib.rs:1351-1361` is correct and must not be relaxed into
   per-condition ignoring; the contrary comment at
   `backend/crates/platform/db/migrations/0065_create_policy_roles.sql:101-103`
   is struck. The database CHECK stays permissive as the additive extension
   point: 17 attribute literals (`0065:110-128`) and three operators
   (`0065:129`).

8. **`feature_catalog` and `Feature::ALL` are one vocabulary in both
   directions**, enforced by a new CI gate. No such gate exists today — the nine
   gates under `backend/ci/gates/` are `audit-coverage`, `dev-auth-absence`,
   `iac-tier`, `layer-boundary`, `migration-safety`, `pii-no-logs`,
   `rls-arming`, `tenant-isolation`, and `vendor-lockin`.
   `orgchange`'s `role_floor` (`backend/crates/orgchange/rest/src/lib.rs:396`,
   called at `:406`, `:414`, `:423`, `:431`) is to be replaced by `authorize(…)`
   so no route re-derives authority from role names.

9. **`Role` and `matrix_row` are a label and a default, not a decision.** They
   become deletable only after the system roles are seeded as `is_system` data
   rows with a golden parity proof against `matrix_row()`
   (`authz/src/lib.rs:573`, `:1141`), and only when every ADR-0021
   coexistence-map entry reads `cedar_only`. Their deletion is not a
   prerequisite for any capability and must not gate any plan. `Feature` is not
   in scope for deletion at all (see Context).

10. **The elevated-capability floor is not relaxed.** Letting a tenant-authored
    role carry `RoleManage` or `ElevatedRoleGrant`
    (`custom_role_runtime_feature_allowed`, `authz/src/lib.rs:1388-1396`)
    remains forbidden until the no-lockout holder floor
    (`docs/specs/rbac-configurable.md:260-262`) is implemented as a hard
    rejection under a per-org advisory lock. No application route can repair a
    locked-out org: the sole writer of `users.roles` is the tenant-scoped path
    at `backend/crates/identity/adapter-postgres/src/lib.rs:291`.

## Why this shape was chosen

The alternative shapes all failed on the same point: they leave a false
sentence standing in an accepted record while shipped code contradicts it.
Deriving all-branch scope from a capability rather than a role name is what
makes the clause survive `Role`'s deletion without a second amendment, and
naming `effective_branch_scope_for_tenant` as the sole composition is what makes
the gap closable by pointing at code that already behaves correctly rather than
by commissioning new mechanism. The three themes that reached this clause
independently are merged here for a mechanical reason, not an editorial one:
`scripts/check-adrs.mjs:399-409` validates only that `amends`/`amended_by` is
reciprocal, so two accepted records both declaring `amends: [ADR-0003]` and
editing line 20 incompatibly would pass CI and leave the authoritative record
self-contradictory.

## Consequences

- The composition gap closes with a decision that points at existing code, so
  no new indirection is introduced at the authorization boundary.
- Strict two-way vocabulary equality means every future capability costs a
  migration row plus an enum variant plus — until `matrix_row` dies — one matrix
  cell. That is deliberate: it forecloses runtime-minted capability names, which
  is already the incumbent decision (`rbac-configurable.md:257-259`; `0065:65`
  grants `console_rt` SELECT on `feature_catalog` only).
- Narrowing the condition write path makes any already-written role carrying one
  of the other 15 attributes, or `not_equals`, un-resaveable without editing the
  condition. Those roles grant nothing today because
  `effective_scope_for_custom_role_conditions` returns `None` and the whole role
  is dropped (`authz/src/lib.rs:1399-1427`, `:1351-1361`), so this is strictly
  better feedback — but it needs a one-time read-only census first (Follow-ups).
- The realtime `org_id` filter is a **narrowing of delivery**: any caller
  relying on a connection being reachable across orgs breaks. None was found in
  this repository, and nothing was compiled to prove it.
- `AccessScopeLevel::Region` and `Worksite` stay fail-closed to `none()`.
  Shipping an enum arm without its resolver returns empty result sets, not
  errors, so anything described as a "worksite-scoped read" is **unavailable**
  rather than merely unfinished.
- It forecloses "delete the matrix to unlock the canvas" as a rationale. Anyone
  proposing `matrix_row` deletion must justify it as cleanup, not capability.
- Deleting `team_policy_values` (`authz/src/lib.rs:1447`, a hardcoded match on
  team names sourced from free-text `users.team`) would break any deployment
  relying on those names resolving through the current path. They must be
  re-expressed as principal attribute data first.
- Any `group_memberships` uniqueness change or `groups.parent_group_id` addition
  is sequenced after this record and routed through **security** review rather
  than schema review, because those rows are the sole input to the
  `{ADMIN}` + `All` derivation at `request-context/src/lib.rs:405-421`.
- Cost: three named remediations (realtime org filter, condition-vocabulary
  narrowing, `role_floor` removal) become owed work the moment this record is
  accepted, and none of them is a one-line change.

## Alternatives considered

### Leave `ADR-0003:20` alone and record the divergence elsewhere

Rejected. README.md:12 makes implementation divergence a governance gap to
reconcile by decision. A note that does not touch the clause leaves an
authoritative record asserting a rule the code does not follow — the failure
mode this repository has already paid for once.

### Amend only the role match, conditional on `Role` deletion

Rejected. That framing understates the trigger, because the divergence exists
today independent of any plan step, and under-scopes the remediation, because
patching the role match at `authz/src/lib.rs:1480` leaves
`request-context/src/lib.rs:421` minting `All` unchanged.

### Amend ADR-0021 as well

Rejected. Its decision that roles are subject inputs rather than authoritative
allow decisions stays true if the inputs are removed; it does not require
built-in roles to exist. Its hold on switching live routes is a hold on
promotion, not a prohibition on removing scaffolding. ADR-0021 is `related`
only.

### Three separate records, one per theme

Rejected on the CI ground stated under "Why this shape was chosen": the
governance checker cannot see two records editing the same clause
incompatibly.

### Make competence a third scope relation

Rejected. The `"team"` arm at `authz/src/lib.rs:1421-1425` is already a
non-branch attribute implemented as a subject-side predicate that gates the
role and never touches the scope. A scope-type change is unnecessary to get the
same behaviour.

## Follow-ups

1. **Inert-condition census (read-only, blocks clause 7).** Count existing
   `policy_role_conditions` rows whose attribute is outside `{branch, team}` or
   whose operator is `not_equals`, per org, before narrowing the write path.
2. **Confirm the whole-role drop is intended, not incidental** (blocks clause
   7). `authz/src/lib.rs:1351-1361` voids the entire role on one unsupported
   condition while `0065:101-103` describes unsupported conditions as inert
   metadata. One of the two is wrong; the code is authoritative and the comment
   is the thing to change.
3. **Seed the built-in all-branch `Feature`** and enroll it in the Cedar schema
   before removing the role match at `authz/src/lib.rs:1480`.
4. **Add the `feature_catalog` ↔ `Feature::ALL` two-way gate** under
   `backend/ci/gates/`, then replace `orgchange`'s `role_floor` with
   `authorize(…)`.
5. **Add the realtime `org_id` filter** at `realtime/src/lib.rs:885` and `:899`,
   give `dispatch_notification` an org parameter, and add a test that a
   `pool: None` hub built through `for_tests` still refuses cross-org delivery.

## Reciprocal record landed on acceptance

Reciprocity landed with acceptance (`docs/decisions/README.md:26`, "reciprocal
where applicable"). All three edits below landed atomically in the one commit
that flipped this record's status (README:3).

**1. Frontmatter key on `ADR-0003-branchscoped-authorization-model-nonnull-branch-scope.md`.**
Before this change that file's frontmatter was `id`, `status`, `doc_status`, `date`,
`owner`, `consensus`, `related: []` — it carried **no `amended_by` key**,
so this **created** the key rather than appending to it:

```yaml
amended_by: [ADR-0028]
related: [ADR-0028]
```

`related` also changed from `[]` to include `ADR-0028` (ADR-0032 and ADR-0036 added
their own `related` entries to the same list in the same acceptance pass).
`scripts/check-adrs.mjs:399-409`
then requires this record to declare `amends: [ADR-0003]`, which replaced
`proposes_amendments_to` at the same moment; `:422-424` permits `amends` only on
an accepted record, and `:414-418` requires the `amended_by` target to be
accepted, so the two status changes and the two key changes were one commit or
none.

**2. Index row in `docs/decisions/README.md`.** The ADR-0003 row changed from
`accepted` to:

```
| [ADR-0003](ADR-0003-branchscoped-authorization-model-nonnull-branch-scope.md) | accepted, amended | Non-null branch scope and default-deny authorization; `BranchScope::All` derivation and `org_id` composition amended by ADR-0028 |
```

The ADR-0028 row changed from `proposed` to `accepted`. A bullet was added to the
"Effective relationship graph" section recording that ADR-0003 remains accepted
for its `Branch`/`Region` day-1 schema, default-deny repository filtering, and
branch-scoped broadcast/rollup rules, with only the `BranchScope::All`
derivation clause changed.

**3. `ADR-0003`'s Decision text at line 20, edited in place.** A reciprocal key
alone would leave a false sentence standing in an authoritative record, which is
the specific failure this section exists to prevent. The sentence

> Principals carry a `BranchScope` (kernel type): `All` for SUPER_ADMIN/EXECUTIVE
> rollups, an explicit branch set otherwise.

was replaced by

> Principals carry a `BranchScope` (kernel type): `All` only where a built-in
> `Feature` capability or a live database membership proof derives it — never
> from a role name — and an explicit branch set otherwise. Composition with
> `org_id`, the single legal composition point, and the migration path for
> existing SUPER_ADMIN/EXECUTIVE principals are governed by ADR-0028.

The remaining three sentences of `ADR-0003:20` are unchanged. `ADR-0018` and
`ADR-0021` each gained `ADR-0028` in their own `related` list and needed no other
edit; neither is amended by this record.
