# Coherent native payroll readiness reads

Starting source: `5de52cc9acfc6f70a017c951ecedf31e58d16b5b`.
Worktree: `payroll-read-snapshot`; sole repository/cache writer: root.

## Observed path and bounded outcome

The real payroll run list/detail, exception and delivery GET owners append
read audits through `with_audits`. Its default READ COMMITTED transaction gives
each statement a different snapshot. A concurrent commit can therefore make
the run head, count, line page, calculation and delivery summaries contradict
one another. Standalone store readers share this count/page problem. The
self-service HTTP path additionally resolves its linked employee in a separate
transaction. These are local read-consistency defects, not physical durability
or frontend acceptance.

The existing `with_org_snapshot` already provides Company-armed REPEATABLE
READ/READ ONLY transactions. Reuse it for standalone reads. Audited reads must
retain their audit rows in the same transaction; they cannot insert into that
read-only helper.

Proposed owner change, subject to independent review:

1. Extend the existing `with_audits` implementation with a named
   `with_audited_snapshot` sibling. Share transaction/audit/rollback logic;
   only the sibling sets REPEATABLE READ before Company arming. Keep mutation
   isolation unchanged. This writable snapshot permits the existing read
   audit, not a new business writer or a claim of confirmed custody.
2. Use that sibling for the run list, run detail, run exceptions and payslip
   delivery GETs. Close-preflight is pure SELECT and instead reuses
   `with_org_snapshot`, preserving its existing separate payload-free audit.
   Misses still do not create read audits. Preserve permissions,
   RLS, pagination, response fields, absence handling and audit semantics.
   Mutation handlers retain their existing locked/current-state path.
3. Reuse `with_org_snapshot` for standalone store list/detail/readiness and
   submitted-action-inbox pages (each has multiple read statements).
   Compose self-service Account-to-employee resolution and its page in one
   store-owned snapshot. Keep legacy public helpers compatible, and do not
   accept an employee ID or Company authority from browser input.
4. Repair only the stale authentication fixtures in the existing payroll
   `api` target using the established real refresh-family test helper. Do not
   weaken its permission, isolation or readiness assertions.

## Proof and delivery

Independently review this design and exact test bytes. First establish that
the repaired API baseline actually reaches its assertions. Admit deterministic
concurrent-read RED on unchanged product source, then GREEN with unchanged
tests. Test-only SELECT policies/advisory locks in disposable PostgreSQL may
pause an existing production query after its statement snapshot is established;
observe actual blocked backend state before committing the concurrent writer.
Do not add a production hook or treat sleeping as synchronization.
For self-link composition, a users policy must match the exact owner query
`SELECT employee_id FROM users WHERE id = $1`, not earlier authentication
queries. Record the blocked query and transaction isolation from PostgreSQL.

Exercise coherent count/page and detail/summary responses, fresh reads after
the concurrent commit, self-link/page composition, real family-bound role and
Company denials, successful read audits and refusal/rollback. Test the shared
helper's Company context/isolation/audit rollback and preserve normal mutation
isolation. Run the complete affected API/owner and prior payroll lifecycle
targets, strict Clippy, formatting, source custody and evidence tooling.
Extend protected CI without removing its existing producers. Independent
source and evidence review must approve the final revision before protected
queue admission; no bypass.

No schema, migration, dependency, endpoint, canonical codec, arithmetic,
institution, payment, deployment or seeded UI exposure change is proposed.
No original Console or shared-root backend-fork changes are allowed. Stop for
weakened authority, changed legitimate lifecycle behavior, unexplained
serialization failures, false empty/fallback success or failed required proof.

## Remaining prerequisites

This slice does not implement the missing opaque Next session owner, resolve
the recorded native purpose/provenance authentication HOLD, freeze current
authorization at response emission, or supply externally witnessed writer
epochs, positive fencing and two-site confirmation. Snapshot coherence is
not durable acknowledgment. Production business UI remains excluded. Exact
money encoding, effective Person/Employment/Org drill-through, source/correction
links and all full-module/release acceptance remain open.
The separate employee `payslip-draft` GET (name/wage/attendance/citations), native
SSR error-to-empty folding and source security/precision are explicitly outside
this run/readiness slice and remain unresolved. No blanket payroll GET coherence
or operational serving claim is made.
