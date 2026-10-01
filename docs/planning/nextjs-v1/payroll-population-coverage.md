# Staged payroll population coverage and atomic local publication

Base: `08cafb8588ca77b9b690a1442ec860d69e8a0283`.
Owner: root, sole repository writer in the isolated
`payroll-population-coverage` worktree. No Console or shared-fork changes.

## Observed native paths

The existing REST submit/decision handlers and canonical PayRun port delegate
to `payroll/adapter-postgres::lifecycle`. Submission currently checks only
`CALCULATED` and open exceptions. Approval does not recheck coverage. A partial
calculation can therefore advance, even with no usable calculation or roster.

The issuance loader chooses latest calculations per line and inner-joins
recipients. It can omit uncalculated/unlinked members or combine stale versions.
The final recorder accepts any delivery subset and unconditionally sets
`ISSUED`. REST commits the loader, emits documents in separate Inbox transactions,
then records completion. A late failure can leave a disclosed partial batch.
Calculation/line/delivery foreign keys do not alone prove exact same-run binding.

## Required bounded outcome

This repair accounts for the run's existing staged roster, not every legally
eligible worker in the Company. It neither invents missing Employment/population
facts nor certifies source provenance, legal payroll, approval or payment.

1. Extend the existing latest-calculation summary with exact Company/run/line
   binding and truthful coverage. A usable current calculation belongs to an
   actual line of this run at the run-wide maximum version; its line has a
   same-Company resolved employee, `READY_FOR_REVIEW` status and exactly empty
   JSON-array blockers. The current roster owner has one line per person;
   duplicate resolved employee IDs are defective coverage, even when source
   keys differ. This is not a ban on concurrent Employment in the full product.
   Missing/stale/non-ready/unresolved/duplicate-person lines cannot count as
   complete coverage. Derive the actual roster count and coverage in the same
   SQL snapshot, rather than trusting an earlier count. Extraneous current
   calculation rows and duplicate people prevent an overall net total.
   Preserve the version, calculation time, exact arithmetic and draft
   `payable=false`; do not change canonical money encoding or historical rows.
2. Add a private coverage guard at the existing lifecycle owner. Under its
   existing run lock, lock roster rows in deterministic ID order. Require a
   nonempty person-grain roster and complete exact current-version coverage,
   including refusal of duplicate employee representation. Use the existing
   `InvalidState`/409/conflict surface with a fixed value-free refusal. Run this
   after existing state/exception checks at submission and before APPROVE.
   Approval also rechecks open exceptions. SoD remains first; REJECT with a
   reason remains possible for a defective submitted run.
3. Issuance retains the existing PAID and release gates. Lock the run and
   roster, enforce the same coverage, select the single current run version,
   and require a same-Company linked Account for every line before publication.
   Do not require an Account for liability submission, or omit former workers
   because Employment ended. Lock recipient rows in UUID order using
   `FOR NO KEY UPDATE`, then revalidate the complete mapping. This prevents
   concurrent relinking through the established identity owner and is
   compatible with foreign-key KEY SHARE locks. Missing mapping fails closed.
4. Extract transaction-scoped emission within the existing Inbox owner; keep
   the standalone method and InboxDocSink delegating to it. Use the existing
   Company context, RLS, immutable document inputs, unique dedup index and
   `insert_audit_event` helper. No inner commits, new writer or dependency.
   Valid redelivery returns the same document without another emit audit.
   A repeated key with different immutable document content/metadata conflicts
   without returning its content or overwriting the old artifact. Compare all
   `NewInboxDoc` fields, excluding confirmation state/timestamps. The sole other
   production consumer is leave promotion; preserve valid notice redelivery,
   receipt locks and tenant/recipient isolation. Conflicting redelivery is a
   deliberate strengthening of this owner's existing idempotency boundary.
5. Compose loader, all Inbox emissions/audits, delivery recording and payroll
   audit/status in one existing `with_audits` Company transaction. Hold run,
   roster and recipient locks until commit. A failure anywhere rolls back all
   newly created documents, links, emit/payroll audits and completion. This is
   atomic local PostgreSQL publication, not two-site durability or legal service.
6. The final delivery recorder independently rechecks the gates and exact
   roster/current-version/recipient set. Require exactly one supplied tuple per
   expected line, with matching employee and a current-recipient payslip doc
   whose title, source and exact payload describe that line/version/period and
   amounts. Use shared payroll title/payload helpers where both creation and
   validation need them. Lock all referenced documents and existing links in
   deterministic ID order through commit so validation cannot become stale.
   Validate existing dedup delivery links as well; no
   foreign-run, wrong employee/doc, duplicate, extraneous or incomplete link may
   become `ISSUED`. Preserve matching legacy partial documents/links for safe
   completion; do not rewrite or delete them. Existing post-ISSUED replay remains
   its typed state conflict.

## Test-first proof

Four revision-bound independent design rounds precede tests. Independently
approve exact test bytes, then admit a genuine clean-tree RED through
`tools/lanes/fanout.py`. Use unchanged tests for GREEN. Missing setup or compile
failures are not behavioral RED. Run complete selected targets, with exact
discovery/execution/ignored/filter counts and raw-log hashes.

- Existing genuine-runtime PayRun tests retain receipt ID continuity, delegation,
  SoD and nondisclosure assertions. Their positive prerequisites must include a
  real staged/owner-calculated roster instead of merely marking an empty run
  `CALCULATED`. Renaming misleading readiness wording requires test review;
  do not weaken assertions. Preserve prior source, close/restage and Company
  isolation regressions.
- Add owner tests for empty/partial populations, blocked/non-array blockers,
  unresolved or duplicated employee, mixed latest versions and same-count wrong-run calculation
  rows; submission/APPROVE refusal must preserve state, receipts and sources.
  REJECT remains available. Summary must withhold misleading totals.
- Exercise the production payroll Router through its real ES256/PBAC/current
  Company/runtime-role SQLx target (`run_lifecycle_api`), including complete
  multi-person issuance and exact stored documents/delivery counts/amounts.
  Missing recipients, stale conflicting dedup artifacts, invalid final tuples
  and a controlled second-document database failure create no new documents,
  links, audits or completion. Reuse exact matching legacy partial artifacts.
- Prove both calculation/submission and recipient-relink/publication lock orders
  with database-observed blockers; no sleeps alone count as synchronization.
  An intervening linkage change is either serialized afterward or observed and
  refused before publication. Include a former-worker case without employment
  deletion or account reset.
- Preserve complete Inbox runtime-role receipt/dedup/isolation tests. Add
  conflicting same-key immutable-input refusals and transactional rollback/audit
  proof; exercise valid leave-promotion redelivery through its existing target.
- Formatting, strict owner/test Clippy, source custody and candidate diff checks
  must pass. Add complete selected payroll REST and Inbox targets to protected
  PR/merge-group CI without removing existing producers. Independently review
  exact implementation and execution evidence, including the final 16-lens
  audit. Submit revision-bound COMMENT and enter the protected main queue only.

## Ownership, risk and limits

Allowed product paths: payroll adapter `src/lifecycle.rs`, payroll REST
`src/lifecycle.rs`, Inbox adapter `src/lib.rs`. Tests stay in existing runtime
PayRun, payroll lifecycle, payroll REST, Inbox and leave runtime targets. Root
serializes CI, planning and fork-delta evidence. No schema, applied migration, lockfile,
dependency, endpoint, institution or live banking change.

Pre-mortem: raw counts can conceal wrong-line coverage; per-line latest can reuse
stale money; final refusal cannot undo earlier disclosure; dedup can replay an
old artifact; account locks can deadlock when ordered inconsistently. Detect
using exact identities/versions, stored artifact equality, whole-database
snapshots on refusals, observed lock waits and full target execution.
Blast radius: shared submit/approve/summary owners and Inbox emission, plus the
existing local payslip route. No universal administrator or second writer.

Rollback before deployment may revert source. A deployed rollback must retain
coverage/atomicity safeguards or refuse affected operations; old code restores
partial admission/disclosure. Persisted cross-build rollback is unverified.
Stop and return to review for changed valid arithmetic, weakened authority or
SoD, migration/API/dependency changes, test weakening, unresolved lock failures,
partial publication, or failed required acceptance.

This does not freeze/bind an exact approval artifact, prohibit all compromised
runtime SQL appends, prove complete legal Employment coverage or source
person/period/applied-import provenance, fix payslip timing relative to payment,
implement natural-person proofing or authenticated Next HR/Org/Payroll/Foundry
UI, certify legal packs, activate payment/institutions, deploy, or prove two-site
durability/restore/migration. Required full-module and release acceptance remains
open. Router tests are not browser or passkey-ceremony proof.

Selected reasoning lenses: Red Team, Operability / Day-2, Blast-radius /
cell-based, Zero-trust / defense-in-depth, Systems Thinking and Essentialism.
