# Native attendance capture versus payroll-period freeze

Base: `b370d1a9ea75d074285dc05b608e7379d5407b56`.
Root is the sole source, test, cache and authority writer in the isolated
`payroll-native-attendance-readiness` worktree. No shared Console, live service,
credential, deployment, migration, dependency or prototype writes.

## Bounded outcome and ownership

The real `POST /api/v1/hr/attendance-records/me` resolves and locks the current
Account/employee link, returns exact idempotent replay before any new write,
inserts a raw event, checks its database-generated KST work date against payroll
period locks, then atomically inserts its material reference and two audits.
That check alone does not serialize with the organization-wide attendance
close which creates the freeze: a lock may commit after the open check but
before capture commits. This is source inference until a reviewed real-router
PostgreSQL probe demonstrates the missing ordering.

Extend `create_my_attendance_record` once: after exact replay/conflict handling,
before transition lookup and INSERT, acquire the existing transaction-scoped
`lock_period_lock_key(Payroll, principal.org_id)` used by the freeze creator and
gated staging. Keep the existing date check after the INSERT, its 409 mapping,
and the atomic record/reference/audit transaction. Ordering is Account-link
row lock → Company/payroll advisory lock. The freeze creator takes its existing
month-close lock → Company/payroll key, with implicit Account FK locks below.
The freeze's `locked_by`, close attestor and audit-actor foreign keys implicitly
take KEY SHARE on `users`. A capture holding FOR UPDATE while waiting for the
payroll key can therefore deadlock with its own Account acting as attestor.
Change the private linked-employee selector's sole writer call to
`FOR NO KEY UPDATE OF u`, the existing Account-lock mode in platform auth.
This still conflicts with relinking, deactivation, ordinary updates, deletes
and other captures, while admitting FK KEY SHARE. Keep the selector and all
identity comparisons unchanged. No new mutex, lock material, transaction,
public input or status is introduced.

If capture owns the key first, the freeze waits until the fact, reference and
audits commit/rollback. If freeze owns it first, capture waits, rechecks the
committed lock in READ COMMITTED and rolls back with 409. Exact duplicate replay
remains write-free even while a period lock is held; changed payload conflicts.
Different Companies and accounting-domain keys do not intentionally serialize.
Keep existing database transaction timestamps/work dates; no timestamp or
offline/backdated-correction semantics change belongs in this repair.

## Test-first proof

Extend `console-app --test hr_attendance_self_read`, keeping existing tests
unchanged and using genuine `console_rt` routers and disposable PostgreSQL.
Use max-one pools with distinct application names and backend PIDs. Witness
exact advisory lock identity, granted/waiting state, production query and
`pg_blocking_pids`; deadlines fail proof and sleeps never establish ordering.

1. Freeze-first: hold the shared payroll key, start the real organization-wide
   attendance-close owner for the current KST month and observe its exact wait,
   then start authenticated capture by that same linked attestor Account and
   observe its wait behind that owner. This also proves FK lock compatibility.
   Release the controller so the queued freeze commits first. After
   release require 409 and unchanged raw-record, reference and capture-action
   audit counts; independently require the real freeze's rows and audits.
   Repeating the rejected operation remains refused without partial effects.
2. Capture-first: delay capture after its open check using a test-only
   PostgreSQL gate on its material-reference INSERT; observe capture holding the shared
   key, start the real organization-wide freeze owner, and observe that owner
   waiting for capture. Release the insertion gate; require capture 200, one
   raw fact, matching material reference and exactly two capture audits,
   followed by a durable freeze. New captures then refuse and exact replay
   retains its original identity without new effects. A separate controlled
   relink UPDATE must wait on the captured Account until commit; after relinking,
   a new request resolves the new employee, never the stale captured linkage.
3. Replay and isolation: while the Company payroll key is held, an existing
   identical operation returns 200/duplicate and changed payload returns 409
   without waiting or writing. A different Company payroll key and the same
   Company's accounting key do not block a genuine new capture.

Tests may use disposable triggers/control transactions to stop the real write;
they cannot replace production owner behavior. Assert runtime role is neither
owner nor BYPASSRLS. Bind tests and exact intended RED assertion locations to
independent approval; infrastructure/setup failures are never behavioral RED.
Admit only a complete named clean-tree RED through `tools/lanes/fanout.py admit`.
Implement from the approved test commit, then run the same tests unchanged to
GREEN, relevant HR/attendance close targets, strict Clippy, format and frozen
source custody. The existing CI producer already runs the full HR target.

## Review, risks, rollback and remaining work

Use Red Team, Zero-trust, Operability, Blast-radius and simplicity/ownership.
Four independent design rounds precede exact-test review and execution.
Coverage/security/simplification review and a final 16-lens audit must approve
the exact candidate before its independent COMMENT and protected merge queue.
Record exact commands, test discovery/execution, source hashes and cleanup.

The Company-wide payroll key intentionally serializes new native captures
with freeze/staging and with each other. This reuses the existing lock contract;
measure contention before changing its granularity. Errors roll back; no
success fallback. Rollback is a reviewed source revert before deployment,
requiring no schema/data rewrite, but it reintroduces the freeze race. Stop on
deadlock, broken replay, authority loss, test weakening, false RED or custody
drift. No direct or bypass merge, protection change, or exposure is authorized.

This does not attach native events to payroll rosters, admit a native-derived
payroll population, compute intervals/hours, reconcile attendance, clear legal
or calculation gates, publish payslips or pay wages. Those need their own exact
source/coverage contracts. Next HR screens remain development prototypes;
actionable SSR/detail, error-versus-empty, keyboard/mobile/dialog and real API
browser acceptance remain open. Two-site confirmation, legal-rule proof, full
HR/Org/Payroll/Foundry, public exposure and MVP release remain HOLD.
