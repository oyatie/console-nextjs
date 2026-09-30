# Authority tip — freeze-window gate on the port write path; backdated changes append history, never rewrite the head

**Date:** 2026-08-14
**Kind:** authority tip (T) for the B-EMP-B containment lane (console-rte, console-r25)
**Scope:** the Employment canonical port's write path (`console-ontology-canonical-adapter-postgres`),
the shared `apply_employment_change` statement, its two REST lifecycle callers in `console-app`, the
org-change adapter's `ReassignOrgUnit` dispatch arm, and the employment-port runtime-role tests. No
migrations, no Cargo/OpenAPI/CI changes, no lockfile churn.
**Immutable base:** `f0f8c1d63b04bca9f260c6e7238c2590b3cc1b51` (origin/main at re-ceremony — the #778
executor-hardening merge on top of the #781 peas employment-port-routing merge). #776 (identity
containment) and #777 (payroll provenance/drain gate) merged mid-train on the earlier base
`f9a88ed19`; then #781 (peas) retired the orgchange `#[path]` Employment seam in favour of the
consumer-declared `EmploymentTransferPort` trait, moved the Employment port test suite from
`console-orgchange-adapter-postgres` to `console-ontology-canonical-adapter-postgres`, and bumped the
domain-half tripwire 89 → 88; then #778 (executor-hardening, scripts-only) merged with no
source-file overlap with this lane. This lane is rebased onto that tip; the error-kind preservation
below is re-expressed through the trait seam (`KernelError` kind + `Frozen` marker), not the removed
`#[path]` module.
**Candidate C and Tip T:** two distinct commits on top of Base, both signed by the pinned authority
(principal `jason19931225@gmail.com`, ED25519 `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`).
C is the single code/wiring commit whose parent is Base and which does NOT contain this file; T is the
single commit whose parent is C and which adds only this file. Their SHAs are recoverable from the
branch as `C = lane-b-emp-b^` and `T = lane-b-emp-b`, and are frozen in the post-merge readback update
— the ledger cannot self-embed them because C's documentation-manifest `blob_sha` points at T's
content.
**Not product authority.** Clears no HOLD. Makes no production, frontend, or compliance claim.

## Summary

- **console-rte — every dated mutation is gated, at the KST business date.** The port's
  `write_in_tx` runs `assert_employment_change_window_open` before any insert, so Appoint, Promote,
  and Transfer — backdated or not — are all refused when their effective date falls inside a closed
  Payroll or Accounting period. The date derives from the fixed KST offset (`+09:00`), never the
  caller-supplied RFC3339 offset, so one instant cannot be submitted as two calendar days to bypass a
  lock. The same gate also stays in the shared `apply_employment_change`, covering the two REST
  lifecycle handlers. A closed window is `EmploymentError::Frozen(KernelError)` → 409; an unparseable
  REST `effective_date` is client input → validation (422), not 500.
- **console-r25 — APPEND decision, bounded by the head's lifetime.** The canonical side was already
  correct (migration 0214 is append-only; intervals derive from `valid_from` order), so it is pinned.
  The port computes whether a Promote/Transfer is *backdated* (`valid_from < MAX(valid_from)`) and,
  when it is, skips only the legacy-head rewrite and the EXITED `valid_to` close — the revision still
  lands as canonical history and the receipt still records it. Two guards close the gaps that follow
  from that skip: a revise dated before the head's opening bound (`employment_heads.valid_from`) is
  refused, and the source binding must resolve for every revise, backdated or not, so a deleted or
  ambiguous binding cannot append a revision the legacy head could never show.
- **Error-kind preservation (re-expressed through the peas trait seam).** The org-change
  `ReassignOrgUnit` arm carries the employment error's `KernelError` kind through both wrappers
  instead of collapsing everything to Conflict, so a freeze refusal stays 409, a domain refusal
  (unbound/ambiguous binding, unknown OrgUnit/JobPosition) stays validation 422, and a gate/lookup
  failure stays 500. Because #781 replaced the `#[path]` Employment seam with the
  `EmploymentTransferPort` trait (whose method returns `KernelError`), the composition root re-derives
  the canonical `KernelError` kind (Frozen → carried kernel; Database/UnreadableReceipt → Internal;
  DigestConflict → Conflict; the domain refusals → Validation) instead of flattening every
  non-freeze failure to Internal, and the orgchange arm re-emits `Frozen` for the Conflict kind so
  `record_freeze_refusal` still fires.
- **Freeze-gate serialization (advisory lock).** `assert_employment_change_window_open` now takes the
  same per-tenant, per-domain advisory key the period-lock CREATE path takes
  (`console_platform_db::lock_period_lock_key`) before its read, on both the port write path and the
  shared REST statement, so a lock committed concurrently cannot slip between the read and the
  write's commit under READ COMMITTED.
- **Internal gate failures are redacted on both surfaces.** `HrError::from(EmploymentError::Frozen(Internal))`
  logs the driver diagnostics and returns a generic message on the HR REST path, and the org-change
  seam redacts `Internal` the same way before the kind reaches `RestError::store`, so DB internals
  never reach either REST client; a closed window (Conflict) and a domain refusal (Validation) still
  pass through verbatim.
- **RED-first, then reviewer-driven.** The two defects were reproduced before the fix, turned green by
  real-DB tests, and then the automated reviewer's findings (freeze bypass for backdated and Appoint,
  offset bypass, head-opening, binding validation, date-validation, kind collapse, and — post-peas —
  the advisory-lock race and the internal-message leak) were each fixed with a pinned test. One
  residual P1 (projected-action preflight never consulting period locks) is gap-noted below, not
  fixed: it is a v1 engine limitation shared by every freeze-gated domain, not a defect this lane
  introduced.

## Verification

### Real-DB greens (disposable Postgres, run here on the post-peas tree)

```sh
tools/lanes/pgtest.sh /Users/jasonlee/Developer/console/.worktrees/lane-b-emp-b \
  env CARGO_TARGET_DIR=/Users/jasonlee/Developer/console/.tmp/cargo-target-b-emp-b SQLX_OFFLINE=true \
  cargo test -p console-ontology-canonical-adapter-postgres --test employment_port_as_runtime_role

tools/lanes/pgtest.sh /Users/jasonlee/Developer/console/.worktrees/lane-b-emp-b \
  env CARGO_TARGET_DIR=/Users/jasonlee/Developer/console/.tmp/cargo-target-b-emp-b SQLX_OFFLINE=true \
  cargo test -p console-orgchange-adapter-postgres
```

43 passed: `employment_port_as_runtime_role` 31 (now in the ontology crate after the peas move; incl.
the original freeze/backdating tests plus backdated-in-locked-window, appoint-in-locked-window,
predating-head-opening, unparseable-date-is-validation, KST-offset-normalization), `org_reference_surface`
4, `preflight_persists_nothing` 5, `apply_refuses_deactivated_region` 1, and the retained
`org_unit_binding` seam unit tests 2. All 31 employment tests pass with the advisory-lock serialization
added before the freeze read, and the org-change integration stays green with the trait-seam
error-kind preservation.

### Unit tests (no DB)

```sh
cd backend && CARGO_TARGET_DIR=/Users/jasonlee/Developer/console/.tmp/cargo-target-b-emp-b SQLX_OFFLINE=true \
  cargo test -p console-ontology-canonical-adapter-postgres --lib
cd backend && CARGO_TARGET_DIR=/Users/jasonlee/Developer/console/.tmp/cargo-target-b-emp-b SQLX_OFFLINE=true \
  cargo test -p console-app --lib frozen_internal_gate_failure_is_redacted_in_the_rest_response
```

8 passed on the ontology lib (incl. `frozen_window_is_conflict_not_internal` and
`frozen_unparseable_date_is_validation_not_internal`), plus the new
`frozen_internal_gate_failure_is_redacted_in_the_rest_response` (console-app) proving the internal
message is logged, not returned.

### Format / lint / gates (run here on the post-peas tree)

```sh
cd backend && CARGO_TARGET_DIR=/Users/jasonlee/Developer/console/.tmp/cargo-target-b-emp-b SQLX_OFFLINE=true cargo fmt --check    # clean
cd backend && CARGO_TARGET_DIR=/Users/jasonlee/Developer/console/.tmp/cargo-target-b-emp-b SQLX_OFFLINE=true \
  cargo clippy --all-targets -p console-ontology-canonical-adapter-postgres \
    -p console-orgchange-adapter-postgres -p console-app -- -D warnings                                  # clean
git diff --check                                                                                          # clean
node tools/ci/check-postgres-cargo-map.mjs                                                                # OK (207 entries; domain-a=44, domain-b=44)
node scripts/check-executed-tests.mjs --update                                                            # baseline rewritten (2656 attrs; employment 22→31, app lib 184→185)
node --test tools/ci/postgres-shard.test.mjs                                                              # 5 passed — domain-half tripwire stays 88 (peas 89→88, unchanged here)
npm run check:doc-manifest                                                                                # OK
node scripts/check-reasoning-lens-contract.mjs --changed-since origin/main                                # OK
git -c gpg.ssh.allowedSignersFile=/tmp/allowed_signers verify-commit HEAD                                  # Good
git -c gpg.ssh.allowedSignersFile=/tmp/allowed_signers verify-commit HEAD~1                                # Good
```

## Freeze status: NOT-FROZEN

This ledger is registered in the seed manifest as `class=evidence`, `status=active`,
`retention=retain`. It is **NOT frozen**: it stays active until the hosted required checks pass and
the PR reaches MERGE_READY, after which the conductor may flip `status` to `frozen` and re-sign. No
production, promotion, or compliance claim rides on an unfrozen ledger; corrections remain permitted
until freeze.

## Operational receipt

- **Pre-mortem (what can still go wrong).** (1) A tenant with a legitimately-open window could be
  refused 409 if a period lock is mis-seeded — but the gate runs RLS-scoped inside the caller's own
  transaction and names the domain and window in its message. (2) `MAX(valid_from)` orders by effective
  instant, not by append order; a same-instant second revision is not "backdated" and still moves the
  head (a 23505 from `UNIQUE (org_id, employment_id, valid_from)` is the canonical-side guard). (3)
  The KST offset is a fixed convention; a future tenant with a different business timezone would need
  that decision re-opened, not silently inherited.
- **Blast radius.** The Employment port write path, the two REST lifecycle handlers that share
  `apply_employment_change`, and the org-change `ReassignOrgUnit` dispatch arm; all tenants with an
  active Payroll or Accounting period lock. No migration, schema, or wire-contract change.
- **Detection.** A false positive shows as a 409 naming the locked domain/window; a regression shows
  as the legacy head moving backward, a backdated/Appoint change landing inside a locked period, or a
  500-class gate failure surfacing as 409. All pinned by the real-DB and unit tests above.
- **Rollback.** Revert the C commit (code + custody registration) and the T commit (this ledger); the
  gate and the backdated skip are code-only, so `git revert` restores the prior behavior without a
  migration.
- **Stop conditions.** STOP and report if any finding demands a migration, `ci.yml`, OpenAPI, or
  `Cargo.toml`/`Cargo.lock` change; STOP if a hosted check fails and cannot be fixed in-tree. Out of
  scope here (separate bead `console-zt3x`): the REST lifecycle path still rewrites the legacy head
  for a backdated `effective_date`.
- **Review identities.** Implementer: Jason Lee `<jason19931225@gmail.com>` (RED-first evidence).
  Conductor: Jason Lee `<jason19931225@gmail.com>` (independent re-run + reviewer-finding fixes +
  signed C+T train). Hosted review is the GitHub PR on `jason931225/console`.

## HOLDs remaining

- The REST handlers are not HTTP-tested (the shared statement is pinned directly); a full HTTP
  lifecycle freeze test remains unproven.
- The pre-existing REST-path backdating defect (backdated `effective_date` still rewrites the legacy
  head) is a separate follow-up bead (`console-zt3x`), not addressed here.
- **Gap-noted, not fixed (critic anti-treadmill clause).** The projected-action preflight
  (`backend/crates/ontology/application/src/prepared.rs`) is a pure, non-consuming decision that in
  v1 cannot read a projected domain row; it already fails closed on submission criteria it cannot
  evaluate, and the period-lock freeze gate is inherently a DB read. Wiring the freeze decision into
  preflight would be a cross-cutting engine change affecting every freeze-gated domain (org-change,
  payroll, attendance, financial), not a defect this lane introduced, so it is filed as a sweep bead
  rather than a one-file patch that would recreate the treadmill.
- **Gap-noted, not fixed (edge case, follow-up bead).** A backdated `EXITED` correction whose
  `valid_from` predates the current head's `valid_to` appends the earlier exit to canonical history
  but, by the APPEND rule, leaves the compatibility head's `valid_to` and `employees.exit_date` at the
  later exit. This is a terminal-backdate reprojection decision (reject it, or reproject the head)
  rather than a mechanical patch, and it was not introduced by this lane's error-kind reconciliation,
  so it is deferred to a follow-up bead.
- No migration, `ci.yml`, OpenAPI, or `Cargo.toml`/`Cargo.lock` change is included; any finding that
  demands one is a STOP, not a fix-in-place.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "hr_payroll"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Essentialism / YAGNI",
    "Chesterton's Fence",
    "Red Team",
    "Systems Thinking",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Separated the two heads the r25 bead conflated: the canonical side was already append-only and correct, so it is pinned unchanged, while only the legacy employees-head rewrite was defective.",
    "Essentialism / YAGNI": "The freeze gate is one assert_period_open loop, called from the port's write path and the shared statement, so every dated mutation is covered without a per-caller copy; backdated handling stays a boolean skip, not a date-ordering engine.",
    "Chesterton's Fence": "The legacy employees head is a projection of the LATEST effective state, not the most recently appended one; the skip preserves that contract, and the head's opening bound is now the fence a revise cannot predate.",
    "Red Team": "Modeled the freeze-bypass modes the reviewer found — backdated and Appoint paths that skipped the gate, an offset that could re-date the same instant, and a kind-collapse that turned 500s into retry-never 409s — and pinned each with a test.",
    "Systems Thinking": "Traced the write path across the port, both REST lifecycle handlers, and the org-change ReassignOrgUnit dispatch arm, so the gate and the error-kind mapping land at the points all four doors route through.",
    "Operability / Day-2": "A closed window surfaces as a 409 naming the domain and window, an unparseable date as a 422, and a gate failure as a 500 — each recoverable by the right caller — with rollback a plain git revert.",
    "Blast-radius / cell-based": "No migration, schema, or wire change; the surface is bounded to the port write path, its shared statement, and one org-change dispatch arm, each independently recoverable.",
    "Zero-trust / defense-in-depth": "The gate runs RLS-scoped inside the caller's own transaction, derives its date from the fixed KST offset rather than trusting the caller's offset, and the port's conflict/validation mapping is unit-tested so a wrong status cannot pass silently."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Both console-rte and console-r25 were triaged FOLLOW_UP in PR #618 as pre-existing defects (introducedByThisPr=False); this train is the decision to fix them in the port's own shared statement.",
    "The first gate placement let backdated Promote/Transfer and Appoint bypass the freeze check; it is now run before any insert in write_in_tx, and the shared statement still gates the REST callers.",
    "Deriving the lock date from the caller-supplied RFC3339 offset let one instant be re-dated to skip a closed window; the date now comes from the fixed KST offset.",
    "The canonical side needed no change: 0214 is append-only and derives intervals by valid_from order, so the backdated revision lands as history while only the legacy head rewrite had to be suppressed.",
    "The automated reviewer raised seven findings (three P1); all seven are fixed and each is pinned by a real-DB or unit test.",
    "After the #781 peas rebase, the reviewer raised three more: the advisory-lock race (fixed) and the internal-message leak (fixed), each pinned; the projected-action preflight gap (not fixed) is a v1 engine limitation shared by every freeze-gated domain and is gap-noted.",
    "The error-kind preservation was re-expressed through the peas EmploymentTransferPort trait seam: the composition root maps EmploymentError::Frozen to its KernelError (preserving Conflict/Internal) and the orgchange arm re-emits Frozen for Conflict.",
    "The REST lifecycle path is not HTTP-tested and its backdated-effective_date defect remains a separate follow-up (bead console-zt3x)."
  ],
  "decisions_changed_or_rejected": [
    "Adopted the APPEND decision for backdated Promote/Transfer: skip the legacy-head rewrite and EXITED close, keep the canonical history insert and receipt.",
    "Rejected the initial backdated-skip of the freeze check: the period-lock contract gates every dated mutation, so the gate now runs before any insert, backdated or not.",
    "Rejected mapping an unparseable REST effective_date to internal (500): client input maps to validation (422).",
    "Rejected collapsing every ReassignOrgUnit employment error to Conflict: the underlying KernelError kind is now carried through.",
    "Gap-noted the projected-action preflight finding rather than one-file-patching a v1 engine limitation that spans every freeze-gated domain."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
