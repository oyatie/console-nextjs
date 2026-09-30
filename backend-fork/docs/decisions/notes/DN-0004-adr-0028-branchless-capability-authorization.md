---
id: DN-0004
kind: design-note
parent_adr: ADR-0028
authority: subordinate
activation: shipped
date: 2026-08-01
owner: jasonlee
---

# DN-0004 — Branch-less capability authorization, and the one verdict it moves

## Status

SHIPPED implementation record under ADR-0028. This note authorizes no new
exposure and unholds nothing. It exists for one reason: the
`authorize_capability` migration is behaviour-preserving at every site except
one, and an undocumented behavioural change is how the next reviewer loses an
afternoon.

## What changed

Eighteen production helpers derived an authorization branch from the CALLER:

```rust
let resource_branch = match &principal.branch_scope {
    BranchScope::All => BranchId::new(),
    BranchScope::Branches(branches) => branches.iter().next().copied().ok_or(..)?,
};
authorize(principal, action, resource_branch)
```

`authorize` checks `principal.branch_scope.allows(resource_branch)` first. Fed a
principal-derived branch that check is a tautology on both arms — `All` allows a
freshly minted id, and `branches.iter().next()` is a member of `branches` by
definition. The branch dimension did not fail loudly; it disappeared.

Sites whose resource genuinely carries no branch now call
`console_platform_authz::authorize_capability(principal, action)`, which drops the
branch dimension visibly. Sites whose resource does carry one thread the real
column. `console-gate-fabricated-branch` fails CI on the fabricating shapes.

`authorize_org_wide` was deliberately NOT used for these sites: it restricts
built-in authority to `SUPER_ADMIN`/`EXECUTIVE`, and `CompletionReview` is
`[D,D,D,A,D,A]` gating the approval inbox at fifteen call sites. Routing there
would have locked out every ADMIN who uses it today.

## The one verdict that moves

`authorize_capability`'s built-in-role arm is byte-identical to `authorize`'s,
which never consults a branch — so every principal whose access comes from a
built-in role is unchanged by construction. The custom-grant arm differs: a grant
must COVER the principal's scope (`grant ⊇ principal`) rather than merely allow
one branch of it.

**Who loses access.** Exactly one class: a principal holding two or more
branches, with no built-in role permission for the feature, whose custom (PBAC)
grant covers some but not all of those branches. A `{seoul, busan}` principal
with a Seoul-only grant on a branch-less feature is denied here.

**Why that is correct.** What it replaced was not a policy, it was a sort order.
The fabricated check authorized against `branches.iter().next()` — the
lowest-sorting branch UUID of a `BTreeSet` — so the identical partial grant
admitted or denied depending on which random id happened to sort first. No
operator could predict it and nothing in the product means it. A deterministic
deny replaces a coin flip, on the conservative side.

It is also right on the merits. These are the sites where nothing downstream
narrows to a branch: a branch-less row has no branch to confine by, and a list
gate's confinement *is* the caller's whole `branch_scope`. A grant covering part
of that scope cannot authorize an action that reaches all of it. The remedy for
an affected tenant is to widen the grant to the principal's scope, which is what
the grant already meant to say.

**Blast radius.** The class requires a multi-branch principal, no built-in
permission, and a partial custom grant simultaneously. Single-branch principals
are unaffected (`grant ⊇ {b}` *is* `grant.allows(b)`) and `All`-scoped principals
are unaffected (the rule collapses to `authorize_org_wide`'s
`grant.branch_scope == BranchScope::All`).

## Evidence

Differential over all 96 features × 3 permission levels, `before` reproducing the
fabricated-branch call verbatim
(`console-platform-authz::tests::capability_is_stricter_or_equal_for_every_principal_shape`):

| Principal shape | before allows | after allows |
|---|---|---|
| group-admin ADMIN (`{Admin}` + `All`) | 205 | 205 |
| branch-scoped ADMIN | 205 | 205 |
| MECHANIC | 44 | 44 |
| RECEPTIONIST | 44 | 44 |
| Executive | 101 | 101 |
| SuperAdmin | 228 | 228 |
| empty-scope principal | 0 | 0 |
| grant-only, grant covers scope | 6 | 6 |
| grant-only, partial grant, 2 branches | 6 | **3** |

The test asserts one-directionally that no cell moved from deny to allow. The
single moved row is pinned by name in
`capability_replaces_an_order_dependent_grant_verdict_with_a_deny`, which also
executes the pre-migration verdict and shows it flipping on branch-id sort order.

## Handoff to the workflow lane

`backend/crates/workflow/**` is owned by a lane that is mid-flight, so this change
does not touch it. `git diff origin/main -- backend/crates/workflow` is empty by
construction. Two public signatures there still take a non-optional `BranchId`,
and both are on the branch-less spine:

| Item | Today | Wanted |
|---|---|---|
| `authz_guard::build_guard_request` | `branch_id: BranchId` | `branch_id: Option<BranchId>` |
| `completion::FinalizePolicyRequest::branch` | `BranchId` | `Option<BranchId>` |

`Some(b)` keeps today's behaviour exactly (`AuthorizationResource::branch`);
`None` builds `AuthorizationResource::branchless` and routes to
`authorize_capability`.

**Why.** `workflow_runs`, `workflow_waiting_tasks` and `workflow_definitions` have
no `branch_id` column, so no caller can supply an honest branch. The only branch
available to `app/src/workflow_studio.rs` was the CALLER's — `guard_branch()`,
`All => BranchId::new()` / `Branches(b) => b.iter().next()` — which made
`authorize`'s `branch_scope.allows(resource_branch)` check vacuous at every
workflow-studio guard: task list visibility, group-inbox role membership, claim,
decide, start, post-finalization rejection, delegated finalize. That helper is
deleted in this change.

**What stands in for it meanwhile.** Two private functions in
`backend/app/src/workflow_studio.rs`:

* `spine_guard_request` — `build_guard_request` with `Option<BranchId>`; `Some`
  delegates to the upstream helper unchanged.
* `enforce_spine_finalize_policy` — `enforce_finalize_policy` with the branch-less
  guard resource. Rule-for-rule identical, including resolving the policy with
  `Feature::from_str` on the raw `required_policy` rather than through
  `guard_policy`.

**What breaks if this is not done.** Nothing compiles differently and no route
changes behaviour — the cost is duplication, not breakage. `enforce_spine_finalize_policy`
is a second copy of the author/delegate finalize rules living in a different crate
from the original. A change to delegated-finalize policy made in
`console-workflow-runtime` will NOT reach `POST /workflow-studio/tasks/{id}/finalize`,
and nothing fails to warn about it. That is the whole reason it is temporary.

**Landing it.** Widen both signatures, then delete both stand-ins and call the
upstream helpers directly; `workflow_studio.rs` already passes
`WORKFLOW_SPINE_HAS_NO_BRANCH` (`Option<BranchId> = None`) at every spine site, so
the call sites need no edit beyond the name. `m2_strangler.rs` passes a real
`work_orders.branch_id` and becomes `Some(branch_id)`.

## What this note does not say

It does not say the migrated tables lack branch identity. `branches` obviously
has one; `employees` has `home_branch_id` (migration 0166). It says only that
these call sites never checked one, because the branch they passed came from the
caller. Threading a real branch column where one exists is a tightening those
lanes still own — `identity/adapter-postgres` for branch mutations, `registry`
for `registry_equipment`/`customers`/`sites`, `reporting` for the non-`Branch`
KPI scopes, and HR for `app/src/hr.rs`.
