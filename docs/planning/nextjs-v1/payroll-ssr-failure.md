# Payroll SSR authorized failure repair — R1

Base: `9e6ddb687e2eaaf031beac4018bf568c37020090`. Root is the sole source,
test, CI, custody and evidence writer in this isolated worktree. This is a
bounded correction of the existing `/_ui` and `/_ui/payroll` read journey,
under the R4 screen-state requirement and the copied DELIVERY review method.

## Observed mechanism and outcome

`PayrollRestState::visible_run_summaries` calls the native audited list owner
with limit100/offset0, then turns every failure into an empty vector. Its sole
production caller, `compose_ui_screens`, combines that vector with a second
authorization lookup. A failed authorized read therefore displays "표시할 급여
이력이 없습니다". The existing `ScreenSection::Failure` renderer is never fed
that real failure.

The correction displays the existing Korean failure state when the authorized
Payroll listing or its required audit fails. Successful zero-row reads remain
empty; successful populated reads retain their existing identifiers and links.
Unavailable or denied identity/PayrollRunRead authority remains omitted, with
no data, counts, failure detail or unauthorized screen offered.

## Owner path

1. Extend the existing Payroll REST SSR helper with a small public outcome:
   Omitted, Loaded(PayrollRunPage), FailedAfterAuthorization. Keep raw RestError
   private. Resolve the current principal and require org-wide PayrollRunRead
   before producing an offered screen. Any error establishing that authority
   produces Omitted, including unavailable policy/identity resolution.
2. Factor the existing authorized audited-list body into one private helper
   shared by REST GET and SSR. Reuse `with_audited_snapshot`, Company RLS,
   `list_runs_in_tx`, its bounded ordered page/count and `payroll_run.list_read`.
   Audit failure returns no rows and rolls back its success audit. REST status,
   JSON, URL, authorization and parameter semantics remain unchanged.
3. Map this outcome directly to the existing UI ScreenSection. Remove only the
   redundant Payroll authorization-floor field. Organization/HR composition
   stays unchanged. A failed Payroll section contains no run ID, source label,
   empty-state copy or Payroll hydration payload; authorized navigation stays
   available. Reload is the existing retry path and must recover after the
   injected fault is removed.

The internal Rust helper signature changes at its sole production caller.
The current first100 SSR page remains bounded; usable SSR pagination and list
completeness are explicitly still open. This repair makes no complete-screen,
Next business-session, browser/passkey, monetary, legal or launch claim.

## Test-first contract

Correct the existing `health_readiness` test-only bearer helper to use the
existing real `issue_session_token` family owner. Preserve every existing
behavior assertion; never weaken current-request validation. No dev-auth,
production fixture or auth bypass is introduced.

Add real Router/PostgreSQL tests for authorized empty/populated states, exact
read-audit counts, real Payroll audit-insert rejection, sanitized failure,
recovery on reload, denied/missing/invalid/revoked/wrong-tier credentials,
foreign-Company absence and unavailable current authority. Exercise both Home
and Payroll SSR, preserving read-only drill-through. The fault is a disposable
database trigger narrowly rejecting `payroll_run.list_read`; it must not break
unrelated organization/HR reads. Existing Payroll REST snapshot/denial and UI
tests remain required.

Freeze and independently approve test bytes before execution/admission.
The lane command runs the complete `health_readiness` target; only reviewed
behavioral failure pins may count as RED. Missing tools, fixture/setup errors,
unreached assertions or other unexpected failures never admit implementation.
The same probe must become GREEN. Add this complete target to protected CI
without removing existing producers. Record actual discovered/executed,
failed/ignored/filtered counts and all rejected attempts.

## Delivery and boundaries

Allowed product paths: Payroll REST `src/lib.rs` and application `src/lib.rs`.
Allowed proof paths: `app/tests/health_readiness.rs`, `.github/workflows/ci.yml`,
this design/review/evidence record, requirement index and fork-delta ledger.
No migration, dependency, applied-byte, generated API, calculation, UI asset,
other domain, original Console, live service or shared dirty-fork change.

Pre-mortem: exposing a failure before proven authority leaks screen existence;
catching audit failure as empty invents business meaning; duplicating reads
loses count/page/audit coherence; stale familyless fixtures falsely test omission.
Detection is the real fault/denial/recovery matrix with unchanged REST/snapshot
gates. Selected lenses: Red Team, Operability/Day-2, Blast radius, Zero trust,
Systems Thinking and Essentialism. Rollback uses the previous compatible
source against unchanged data; it restores the known false-empty defect and
does not waive that defect for release.

Four independent design rounds, exact test approval, genuine RED/fanout
admission, source review/fix, sixteen-lens audit and exact-head independent
COMMENT precede protected queue admission. Stop on material design change,
weakened authority/tests, altered REST behavior, unsafe custody/rollback or
required failure. Current authority at emission, two-site confirmation/fencing,
identity/recovery, genuine Next HR/Org/Payroll/Foundry journeys, pagination and
the complete MVP/launch remain open.
