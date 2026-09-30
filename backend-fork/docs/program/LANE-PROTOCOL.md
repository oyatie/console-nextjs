# Lane protocol

Status: active preparation and integration contract. Product scope comes from the pivot; detailed method comes from the agentic engineering playbook.

## Preparation gate

Fan-out is forbidden until all are true:

1. Company, Person, Employment, and PayRun each have an accepted owning port and
   proven single-writer boundary, and Company plus OrgUnit form a hand-reviewed
   product reference.
2. A replacement product conformance target exists for that reference. The
   instance-backed `company_conformance` fixture remains a generic-engine
   regression and must not be frozen or promoted as the product target.
3. `CATALOG.md` has been reconciled against those accepted contracts and promoted
   by a later current authority. Its present preparation status cannot dispatch
   work mechanically.
4. A later candidate explicitly authorizes a small collision pilot after the
   preceding gates pass. JobPosition and projection fan-out remain HOLD now; this
   protocol does not pre-authorize a two-lane pilot.
5. The original test baseline and exact invocation have independent evidence.

Person, Employment, and PayRun are domain-owned projected writers, not generic instance fan-out.

## Worktree topology

While the preparation gate is HOLD, create no product writer worktrees from this
protocol. After a current candidate records every gate above as satisfied, use at
most three concurrent writer worktrees, one reserved integration worktree, and
one reserve/fix worktree. Read-only reviewers inspect frozen diffs and exact SHAs.
Increase writers by one only after two collision-free epochs. Reduce concurrency
immediately after any collision, stale-base rebuild, resource saturation, or
reviewer backlog.

## Required lane receipt

Every admitted lane adds `docs/program/ledger/<lane-id>.md` and parser-visible registry metadata with:

- outcome and non-goals;
- exact base SHA and reference contract;
- owner, allowed writable roots, and forbidden shared roots;
- source-of-truth writer;
- resource demands and disposable leases;
- pre-mortem, blast radius, detection, rollback, and stop conditions;
- immutable test baseline and exact invocation;
- review lenses and approvers;
- evidence artifacts, head SHA, result, remaining HOLDs, and post-merge readback.

## Ownership

No two writers edit the same path. Migrations, lockfiles, OpenAPI, CI, authority records, generated files, and integration manifests have one serialized owner. Migration numbers are assigned immediately before landing, never reserved on stale branches. Generated files are changed through their source generator.

High-risk authz, migration, contract, approval, HR, release, and compliance-sensitive changes require one implementer, two independent adversarial reviewers, and a distinct fixer/integrator.

## Stop conditions

Stop and return the lane to integration on an out-of-scope write, target-baseline change, hidden dependency, migration-number conflict, stale base, failing nondisclosure/rollback evidence, test weakening, or a need to change a serialized face. Runtime state and branch names do not establish completion.
