# Payroll roster restage integrity

Base: `87212b127503e4f7164a009d6c38e138f075af48` in the independent
`oyatie/console-nextjs` repository. Root owns this isolated worktree and design.
Status: proposed; no product implementation or test approval yet.

The existing `payroll.create_run` canonical action and workflow
`PayrollDraftStaging` both call the payroll owner's staging transaction, which
refreshes `payroll_draft_lines` even for an existing natural-key run. Its
unconditional upsert replaces source IDs, time totals, verified tax status and
review blockers after attendance close, calculation or approval. An exact
command receipt replay avoids this, but a fresh command or workflow restage
does not. This can invalidate the basis of an already reviewed result.

Required bounded outcome:

- At the shared `roster::materialise_roster_in_tx` owner, acquire the same run
  row lock used by attendance close, calculate, submit and decide. Resolve the
  exact Company, run and declared period under current RLS before reading sources.
- Refresh only the existing pre-close states `STAGED`, `BLOCKED_LEGAL_GATE`
  and `READY_FOR_REVIEW`. Preserve pre-close empty-roster repair and refresh;
  reuse the lifecycle's state definition so the two cannot silently diverge.
- Known post-close states (`ATTENDANCE_CLOSED`, `CALCULATING`, `CALCULATED`,
  `SUBMITTED`, `REJECTED`, `APPROVED`, `DISBURSEMENT_SCHEDULED`, `PAID`,
  `ISSUED`, `VOID`) return zero changed lines. Restaging remains an accepted
  no-op; it does not revoke approvals, rewrite calculations, reopen attendance
  or lose the pending workflow acknowledgment.
- Missing/foreign runs, mismatched periods and unknown states refuse without
  roster writes. No tenant information leaks through errors. Keep existing
  provenance mismatch, command receipt and period-lock behavior.
- Remove the workflow drain's redundant phase-1 period-open filter. The
  authoritative staging owner already serializes period locks, refuses locked
  new drafts, and accepts same-provenance existing drafts. Delegate this decision
  to that owner so staging committed before an interrupted acknowledgment can
  be reconciled after a period lock. Locked new drafts and mismatched provenance
  remain pending with no draft/roster write or acknowledgment. Do not duplicate
  owner lookups or provenance rules inside the workflow runtime.
- In the gated staging owner's existing-run branch, a matching run under an
  active period lock is accepted without roster refresh, even while pre-close.
  Make that decision under its existing period advisory lock. Otherwise removing
  the early filter could admit a pre-close source rewrite previously prevented
  by the whole drain. Database failures remain failures; they are never treated
  as a successful no-op. Ordinary pre-close repair remains allowed when this
  applicable gate is open. Canonical staging's existing period semantics are
  unchanged; the shared post-close roster guard covers both callers.
- Close versus refresh has one ordered result: refresh committed before close
  contributes to its basis; refresh after close preserves that basis. Hold the
  run lock through both validation and roster changes in the caller transaction.

Tests first in existing payroll/runtime-role targets: exact before/after roster
content for pre-close refresh and closed/calculated/rejected/approved restage;
real canonical fresh-command and workflow restage (including acknowledgment
after staging commits, acknowledgment is interrupted and the period locks,
with one acknowledgment/audit and no duplicate draft); retain locked-new-draft
and mismatched-provenance negatives; same-command receipt replay; missing/foreign/wrong-period
zero-write refusal; controlled close-versus-refresh concurrency using PostgreSQL
lock observation rather than sleeps, exercising both lock orders. Use the existing genuine `console_rt`
role and owners. Fixtures stay exclusively in tests. No production source edit
starts before independent design/test approval and an admitted named red probe.
Also test interrupted acknowledgment of a pre-close run followed by a period
lock and changed source material: whole-drain acknowledgment must preserve the
exact roster, including verification fields, and emit one audit only.

Blast radius: shared payroll roster refresh and workflow staging's early filter.
The owner remains the only period/provenance decision and payroll writer.
No schema, applied migration, API, hash/codec or new business writer.
Rollback before deployment reverts this
guard; deployed rollback must retain the guard or refuse restage of closed runs
because the old binary would reopen the corruption path. The compatible rollback
build must also retain the owner-directed drain decision and locked-existing-run
no-refresh behavior; restoring the old early filter would strand accepted pending
work. Verify the same one-acknowledgment/audit recovery path through rollback.
Detect through exact
source/roster/calculation snapshots and outbox status. Stop for changed frozen
content, lost acknowledgment, permission leak, lock-order deadlock or failed checks.

This contains one source-mutation path; it does not establish full employee
coverage, effective Employment serving, independent natural-person approval,
exact-payload publication, corrected payroll, payment, legal conclusions,
authenticated Next workflows or two-site durability. It creates no claim that
the existing payroll lifecycle is complete or payable. Raw SQL privileges and
other writers remain separate boundaries requiring review.
