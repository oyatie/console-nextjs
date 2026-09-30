# 2026-08-01 — six cross-branch defects, and the guard that was a shape

Six handlers in `registry` and `identity` let a branch-scoped principal read or mutate another
branch's rows. The most reachable needed no privilege at all beyond an ordinary field role: a
MECHANIC at one branch could read any other branch's equipment version history — owner, insurer,
policy holder, rental fee, vehicle value, residual value, acquisition cost.

The guard looked like a guard and checked nothing. `authorize_read_access` takes no resource id,
so it authorized against `branches.iter().next()` — a branch the caller already held — and
`principal.branch_scope.allows(that)` is true by construction. The sibling `get_equipment` passed
the real scope and 404'd cross-branch; the richer history endpoint beside it did not.

Four of the six were writes: edit, rollback and soft-delete of any equipment in the org; ownership
transfers opened and decided on another branch's fleet; substitutions assigned and returned inside
a branch the caller had no relationship to; and a branch ADMIN renaming or re-parenting **any**
branch in the tenant, including HQ.

The fix is at the shared fetch points rather than the handlers, because the handlers drifted
precisely where the rule lived only in prose — `update_site` already carried the correct
`allows()` → `not_found` guard and its siblings did not. A cross-branch miss returns `not_found`,
never `forbidden`, so existence is not revealed. `BranchScope::All` is unaffected: the bind
collapses to `TRUE` and every org-wide path executes the identical SQL.

Twelve boundary crossings were demonstrated before the fix and refused after, including an
intruder setting `asset_owner` on another branch's equipment and renaming a foreign branch while
its rejected rename still wrote an audit row.

Every capability, evidence contract, jurisdiction binding, Korea control, review disposition, and
exposure state remains `HOLD`.
