---
id: ADR-0036
status: accepted
doc_status: review
date: 2026-07-30
owner: jasonlee
decision: object-dimensioned-economics
related: [ADR-0002, ADR-0003, ADR-0023]
---

# ADR-0036: Object-dimensioned economics over the double-entry voucher

## Status

**Accepted 2026-07-30.** This record neither amends nor supersedes any accepted
decision; it declares `related` only. It draws a boundary around an existing money
store and names what a future finance subsystem — a **peer plan**, not this record —
must not foreclose. A full ledger design (account master, analytic dimensions,
currency model, period close, reporting) is out of scope here by intent.

## Context

The whole of the money store is two tables plus one later additive migration:

- `finance_gl_vouchers` (`backend/crates/platform/db/migrations/0160_create_finance_gl_vouchers.sql:22-50`)
  and `finance_gl_voucher_lines` (`0160:57-68`).
- `0163_finance_gl_voucher_sod.sql:18-28` adds `approved_by` and the SoD CHECK
  (`status NOT IN ('APPROVED','POSTED','REVERSED') OR (approved_by IS NOT NULL AND
  approved_by <> created_by)`), and replaces the rules trigger function
  (`0163:33-76`). The trigger itself is still the one created at `0160:116-118`, so
  the **effective** balance-gate and immutability body to read is `0163:33-76`, not
  `0160:78-114`.

What already works, and works fail-closed at the DB layer:

1. **Balance gate.** No advance into `BALANCE_CHECKED`/`APPROVED`/`POSTED` unless
   Σ debit = Σ credit and the total is positive, recomputed from the lines
   (`0163:58-72`).
2. **Posted immutability.** `POSTED` is terminal except for the single
   `POSTED → REVERSED` transition; `REVERSED` is terminal (`0163:47-54`).
3. **Append-only lines.** Insert only, and only while the parent is `DRAFT`
   (`0160:122-140`); `console_rt` holds `SELECT, INSERT` on lines with `UPDATE` and
   `DELETE` revoked (`0160:163-164`), and `DELETE` on vouchers is revoked
   (`0160:162`).
4. **A deliberately domain-free source reference.** The header carries
   `source_object_type`/`source_object_id` as free TEXT with **no FK**, because the
   source may live in any domain (`0160:31-35`); both-or-neither is a CHECK
   (`0160:46`) and the pair is indexed (`0160:53-54`). Provenance is set only by the
   trusted approval-derived path — hand-keyed vouchers carry none
   (`backend/crates/finance-gl/rest/src/lib.rs:166-168`).

What is missing, and each item belongs to the peer plan:

- **The line carries no object reference at all** (`0160:57-68`). The header is
  dimensioned; the line is not. So a per-object or per-현장 figure is
  header-approximated, and line-level analytic dimensions do not exist.
- **No account master.** `account_code` is free TEXT, blank-rejected but not
  trimmed (`0160:62`), so `'100'` and `' 100'` are two accounts and any
  `GROUP BY account_code` aggregate is not reproducible by construction.
- **No business/accounting date.** Only a nullable `posted_at` (`0160:41`), stamped
  from `now_utc()` (`finance-gl/rest/src/lib.rs:171,246,279`) at the moment of the
  transition (`finance-gl/adapter-postgres/src/lib.rs:477,483`). The period-lock
  guard takes a `Date`
  (`backend/crates/platform/db/src/period_lock.rs:60-66`) and an accounting domain
  exists and is already called elsewhere
  (`backend/crates/financial/adapter-postgres/src/lib.rs:1256-1259`), so
  `period_locks` cannot apply to a voucher as currently shaped.
- **Currency is in the column name.** `amount_won BIGINT` (`0160:64`).

The sharpest single fact in the theme: the voucher surface is authorized by the
period-lock capability —
`const VOUCHER_FEATURE: Feature = Feature::PeriodLockManage;`
(`finance-gl/rest/src/lib.rs:28`) — and never calls the period-lock guard. Those
two mentions (`:4` in the module docs, `:28`) are the crate's only relationship to
the period lock.

Meanwhile three parallel money records already ship with three encodings:
`equipment_cost_ledger` with a business-dated `entry_at` and `amount_won`
(`0015_create_financial.sql:44-60`, date at `:56`),
`equipment_3r_rental_cases.monthly_rate_minor`
(`0182_create_equipment_3r.sql:33`), and
`equipment_3r_dispositions.cost_minor`/`sale_amount_minor` (`0182:96-97`). A fourth
is cheap to write and expensive to reconcile.

One conclusion accounting reached independently. A retroactive 반려 after posting
cannot be an undo: the DB forecloses it (`0163:47-54` plus the revoked grants at
`0160:162-164`), so the only expressible correction is a new voucher. `reverse`
builds exactly that — a contra whose lines swap 차↔대 (`line.side.reversed()`,
`finance-gl/adapter-postgres/src/lib.rs:204`), posted and linked in the same audited
transaction, with the original stamped `REVERSED` and pointed at its contra
(`:150-231`) through `reversal_of_voucher_id`/`reversed_by_voucher_id` (`0160:38-39`,
FKs `:48-49`). That is the same shape ADR-0023:84 chose for approval lines —
사후 반려 as a compensating document, never a reopened terminal.

## Decision

1. **The voucher pair is the accounting record of money, and cost is a query over
   it.** Economics is answered by reading the money record dimensioned by object
   reference. No second denormalized figure and no fourth parallel money table is
   introduced to answer a reporting question.
2. **This record does not design the finance subsystem.** Account master, analytic
   dimension model, currency model, period close, and any reporting surface are
   decided by a peer plan. This record binds only the boundary and the
   non-foreclosure constraints below.
3. **A voucher-derived figure may not be presented as period-stable until two
   things land together:** an `accounting_date DATE NOT NULL` on
   `finance_gl_vouchers` distinct from `posted_at`, and an
   `assert_period_open(tx, PeriodLockDomain::Accounting, accounting_date)` caller
   inside the voucher's own transaction. Until then a voucher answers "when was
   this keyed", never "which period does this belong to". `posted_at` is not to be
   read as a business date.
4. **The line-level object dimension is additive, nullable, and domain-free.** When
   it lands it follows the header's deliberate shape (`0160:31-33`): a logical
   object reference with no FK into any single domain. It must not weaken the
   append-only insert rule, the `DRAFT`-only insert trigger, or the balance gate,
   and it must not require a rewrite of any posted line.
5. **`amount_won` is not reinterpreted.** A multi-currency or minor-unit shape
   arrives as new columns with an explicit currency, never by silently redefining a
   column on posted, immutable, undeletable rows.
6. **Correction of a posted voucher is a compensating contra voucher, never an
   undo.** This clause is in force now, not a follow-up — the undo is already
   DB-foreclosed and the contra already exists.
7. **Named non-decisions.** This record decides no chart of accounts, no statutory
   or Korea-specific accounting compliance posture, and no reporting surface.
   Nothing here is evidence that any jurisdictional control is satisfied.

## Drivers

1. **The same question can get two answers today.** `equipment_cost_ledger` is
   business-dated (`0015:56`) and its writes are refused inside a locked accounting
   period (`financial/adapter-postgres/src/lib.rs:1256-1259`); the voucher path is
   neither. Asked before and after a backdated correction, one store's answer is
   stable and the other's is not. **This is a prediction, not a measurement** —
   probe X-T9a in `docs/ideas/adr-adjudication.md:1391-1400` is unrun, and if the
   voucher answer turns out stable, clause 3's premise is wrong.
2. **The dimension the header already has proves the pattern and its gap.** The
   free-text, FK-free source reference (`0160:31-35`) is how this codebase already
   attaches money to an object across domains. The line simply never got it.
3. **The capability/guard mismatch is real and cheap to state now.**
   `Feature::PeriodLockManage` gates the surface (`finance-gl/rest/src/lib.rs:28`)
   while nothing in the crate consults the lock.
4. **"Manageable without developers" argues for dimensions, not columns.** A new
   analytic question should be a new grouping over an object reference, not a new
   column and a new migration each time.

## Alternatives considered

### Decide the whole finance subsystem in this record

Rejected on scope and on evidence. The account-master question is an owner decision
(`docs/ideas/adr-adjudication.md:1291-1297`), and two of the probes that would settle
the money-shape questions (X-T9a, X-T9b) are unrun. A record that decided the master
now would be deciding it from argument.

### Answer economics from a denormalized per-question cost column or a new money table

Rejected. Three parallel money records with three encodings already exist
(`0015:44-60`, `0182:33`, `0182:96-97`); a fourth multiplies the reconciliation
surface without adding an answer the voucher cannot give once its line is
dimensioned.

### Treat `posted_at` as the accounting date

Rejected. It is nullable (`0160:41`) and stamped from `now_utc()`
(`finance-gl/rest/src/lib.rs:171,246,279`), so it moves with the keystroke, not with
the period; the lock guard takes a `Date` (`period_lock.rs:60-66`).

### Make the line's object reference a foreign key

Rejected. The header deliberately refuses an FK so the source may live in any domain
(`0160:31-33`). A line-level FK would pick a winning domain and make cross-domain
attribution unexpressible.

### Reinterpret `amount_won` as minor units

Rejected. That is a silent redefinition of already-posted rows that cannot be
corrected by UPDATE or DELETE (`0160:162-164`).

### Add a `CHECK` requiring `reversed_by_voucher_id` whenever status is `REVERSED`

Deferred, not rejected. It is a genuine gap — the DB forecloses the undo but does not
compel the contra — and it is additive, so it belongs to the peer plan's migration
rather than to this record.

## Consequences

- Positive: the boundary is recorded before the peer plan starts, so the four
  missing pieces are named work rather than discoveries mid-slice.
- Positive: clause 6 records at decision level what the DB already enforces, so a
  future slice cannot re-litigate reversal as an in-place edit.
- Positive: clauses 3-5 are all additive, so the peer plan cannot be forced into a
  rewrite of posted rows.
- Negative: until clause 3's two changes land, the voucher store answers no
  period-scoped question, and any console surface that wants one must either wait or
  read `equipment_cost_ledger` — which is exactly the divergence this record wants
  to end.
- Negative: object-dimensioned cost stays a query. Query cost is unmeasured here,
  and if it proves unacceptable the caching/materialization question reopens as its
  own decision.
- Negative: leaving `account_code` free text keeps aggregates non-reproducible for
  as long as the master question stays open (`0160:62`).
- Negative: the four peer-plan items consume one slot in a strictly serial migration
  version space — contiguity is enforced at
  `backend/ci/gates/migration-safety/src/lib.rs:132-142` and `0205` is the highest
  version currently present — so this competes with every other lane for ordering.

## Follow-ups (named out of scope for this record)

1. Run X-T9a and X-T9b (`docs/ideas/adr-adjudication.md:1391-1407`) before the peer
   plan fixes the date and account shapes. Each has a stated known-bad control; a
   probe with no demonstrated failure mode is not evidence.
2. The peer plan states, before it designs anything, whether it optimizes for a
   tenant-authored account vocabulary or a reproducible account master.
3. `accounting_date` + the `assert_period_open` caller + the line object dimension
   as one additive migration, with its version slot allocated rather than assumed.
4. A dedicated `VoucherManage` capability, already named as a clean follow-up in the
   crate's own module docs (`finance-gl/rest/src/lib.rs:1-8`).
5. The `REVERSED` ⇒ `reversed_by_voucher_id IS NOT NULL` CHECK.

## Reciprocal record edits landed on acceptance

This record carries no `amends` or `supersedes` key, so no
target ADR gained `amended_by`. `related` is not a machine-
reciprocal key (`scripts/check-adrs.mjs:23-27` pairs only
`amends`/`amended_by` and `supersedes`/`superseded_by`), but README:9's
"explicit in both records" applies in spirit. On acceptance, and in one atomic
commit:

1. `docs/decisions/ADR-0002-auditfirst-transactional-discipline-audit-event-in.md`
   frontmatter gained `ADR-0036` in its `related` list.
2. `docs/decisions/ADR-0003-branchscoped-authorization-model-nonnull-branch-scope.md`
   frontmatter gained `ADR-0036` in its `related` list.
3. `docs/decisions/ADR-0023-oyatie-console-authority.md` frontmatter gained
   `ADR-0036` in its `related` list.
4. `docs/decisions/README.md`: the ADR-0036 index row's status cell changed from
   `proposed` to `accepted`.

No sentence in ADR-0002, ADR-0003, or ADR-0023 became false on acceptance, so no
in-place Decision text edit was owed in any of them. ADR-0002's same-transaction
audit rule and ADR-0003's branch scope are what the voucher store already obeys
(`with_audits`, `backend/crates/platform/db/src/audit_tx.rs:111`; branch-scoped
`authorize` at `finance-gl/rest/src/lib.rs:189,239,270`), and ADR-0023:84's
compensating-document conclusion is reinforced, not narrowed. If review finds any
such sentence, the sentence is edited in place in the same commit — a `related` key
alone would leave a false statement standing in an authoritative record.
