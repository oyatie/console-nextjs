# Payroll source validation

Base: `f35a1716886894e86014318d201f61169762c7a2`.
Root is the sole repository writer in the isolated worktree. This is a bounded
calculation-owner repair, not approval of serving HR/Payroll/Foundry or release.

## Observed path and required outcome

`POST /api/v1/payroll/runs/{run_id}/calculate` calls the existing
`lifecycle::calculate_run_in_tx` owner. After its run lock, state gate and
source-readiness checks, `select_source_amounts` reads linked canonical rows.
Its `filter_map(extract_source_amounts)` silently omits malformed declared
payroll payloads if another row is valid. The optional pension basis uses
`and_then(Value::as_i64)`, so a supplied malformed basis becomes absent and
the kernel may instead use gross remuneration.

Extend this existing selector, without another parser or business writer:

- A canonical object without a `payroll` key remains an ignorable non-payroll
  source, including attendance-only rows.
- A non-object canonical row, or a present `payroll` value that cannot satisfy
  the current source schema, blocks the entire line with the fixed code
  `SOURCE_AMOUNTS_INVALID`. Do not place source values in the blocker or logs.
- Required gross, national income tax and local income tax remain JSON i64
  integers. The tax table version remains a required string. Existing kernel
  validation of signs, nonblank versions, rates and arithmetic is preserved.
- Optional pension basis may be absent or JSON null. A supplied non-null basis
  must be an i64 integer; strings, booleans, fractional numbers, objects, arrays
  and out-of-range numbers must not be converted to missing or coerced.
- Identical valid decoded figure sets remain acceptable. Different valid sets
  retain `SOURCE_AMOUNTS_CONFLICTING`. No payroll source retains
  `SOURCE_AMOUNTS_NOT_MATERIALIZED`. Invalid input takes precedence over
  conflicting valid rows independent of input order.
- The calculation owner records the invalid line as blocked, writes no
  calculation for it, and continues accounting for other valid lines. Existing
  run status, draft-only/payable=false behavior, exceptions, audit owner and
  downstream submission gate remain unchanged.

## Proof and ownership

Write tests first in the existing lifecycle unit-test module and
`pay_run_port_as_runtime_role` target. Independently approve exact test bytes,
then admit their clean-tree failing command through `tools/lanes/fanout.py`.
Use unchanged tests for GREEN.

Cover valid-plus-malformed payloads in both orders, invalid-before-conflict
precedence, all invalid optional pension types, legitimate absent/null pension,
attendance-only rows, duplicates and conflicting valid values. Through real
PostgreSQL and genuine `console_rt`, exercise one run with valid and malformed
source lines: exact per-line blockers, no invalid calculation, unchanged source
rows, valid line still calculated with unchanged arithmetic and payable=false,
complete outcome counts. Preserve
the existing Company-isolation and restage/close concurrency tests.

Allowed product path: payroll adapter `src/lifecycle.rs`. Tests remain in its
existing unit module and runtime-role target. Planning, source-custody ledger
and the protected CI producer are root-owned serialized changes. No schema,
applied migration, dependency, public route or monetary encoding change.

Pre-mortem: classifying absent metadata as invalid would block legitimate
attendance; ignoring bad pension values would silently change deductions;
early conflict return could make blockers row-order dependent. Detect through
exact permutations, real stored source/calculation snapshots and counts.
Blast radius: shared payroll source selector and its calculation consumer.
Rollback before deployment reverts the repair; a deployed rollback must retain
the stricter selector or refuse affected calculations, because the old build
silently omits invalid evidence. Stop for changed valid arithmetic, source
writes, lost population accounting, scope widening or failed required checks.

The existing submission owner checks state and exceptions, not full calculation
coverage; this repair does not change or prove that gate. Full source/roster
coverage across submission and issuance needs its own coherent reviewed change.
Source-to-person/period/provenance validation, immutable review binding,
effective Employment coverage, natural-person independence, server-session
SSR, legal rates, payment, two-site durability and persisted cross-build
rollback remain separate unresolved boundaries. This repair makes no claim
that a calculated draft is approved, issuable, paid or legally complete.

Selected lenses: Red Team, Operability / Day-2, Blast-radius / cell-based,
Zero-trust / defense-in-depth, Essentialism and Systems Thinking. Independent
design, test and final reviews must bind exact source hashes/commits.
