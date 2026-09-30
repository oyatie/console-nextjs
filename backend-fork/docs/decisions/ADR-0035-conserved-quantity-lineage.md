---
id: ADR-0035
status: accepted
doc_status: review
date: 2026-07-30
owner: jasonlee
decision: conserved-quantity-lineage-deferred
related: [ADR-0001, ADR-0002, ADR-0018, ADR-0029]
---

# ADR-0035: Conserved quantity lineage — the row CHECK is a backstop, not the mechanism

## Status

**Accepted 2026-07-30.** No accepted decision is amended or superseded by this
record. It defers a schema (quantity-bearing split/merge lineage) and fixes the
mechanism story for the conservation the repository already ships. Nothing here
authorizes a migration slot, a table, or a new gate.

## Context

A quantity-bearing lineage model — lots, splits, merges, a derivation DAG, and
recursive traversal with "children sum to parent" enforced — has been proposed
as platform ground. Two facts decide the timing, and a third decides the
mechanism.

**First: nothing in the repository needs a DAG, and the first vertical needs it
least.** HR + payroll is the named first vertical
(`docs/ideas/authority-and-approval-model.md:49`). It has no lot, no batch, no
BOM, no yield and no scrap. Neither does the migration set: no `CREATE TABLE`
for a lot or batch entity exists anywhere under
`backend/crates/platform/db/migrations/`. The domains that would eventually use
lineage ship linear models today and are satisfied by them — production pins one
entry point per plan (`0173_create_production_execution.sql:59`,
`first_operation_id UUID NOT NULL UNIQUE`) and orders operations one-per-sequence
(`:88`, `UNIQUE (plan_id, sequence)`); logistics' partial draw is a monotone
cascade on a single row (`0179_create_logistics_pilot.sql:48-49`,
`requested_quantity ≥ reserved_quantity ≥ picked_quantity`); inventory
consumption writes one before/consumed/after triple per event
(`0156_create_inventory.sql:103`). Building a DAG now replaces a working model
in a vertical that is not first, at the cost of the vertical that is.

**Second: the row-level CHECK that appears to enforce conservation does not.**
Two CHECKs carry the arithmetic:

- `0156_create_inventory.sql:103` —
  `CHECK (quantity_before_milli - quantity_consumed_milli = quantity_after_milli)`
- `0191_create_inventory_cycle_counts.sql:119` —
  `CHECK (quantity_before_milli + quantity_delta_milli = quantity_after_milli)`

Both are per-row predicates over three columns of the row being written. Neither
can observe another row, another transaction, or the item's live balance. Two
concurrent consumptions of 60 against a 100-unit item can each read 100, each
compute `100 − 60 = 40`, and each write the triple `(100, 60, 40)`. **Both rows
satisfy the CHECK. 120 units were consumed from a 100-unit item and no
constraint objected.** A CHECK is an arithmetic backstop on one row's internal
consistency; it is not a conservation mechanism, and it never was.

**Third: what actually conserves is a row-level pessimistic lock, and it already
ships.** The consumption path in
`backend/crates/inventory/adapter-postgres/src/lib.rs` is a faithful instance of
ADR-0002:20's decided ordering (`SELECT FOR UPDATE → validate transition →
UPDATE → INSERT audit_events → COMMIT`, via `with_audit`), composed of three
controls:

1. `:376` — `lock_consumption_idempotency_key_tx`, a transaction-scoped
   `pg_advisory_xact_lock` over a length-delimited `(org, normalized key)`
   composite (`:1150-1170`), so one replay pair serializes and the follower reads
   the committed event instead of racing the `UNIQUE (org_id, idempotency_key)`
   constraint at `0156:102`.
2. `:394` — `fetch_item_for_update_tx`, which resolves through `:1019` and
   `:1044` to a `SELECT … FOR UPDATE OF i` (`:1053-1054`). This is the lock that
   makes the balance read authoritative for the rest of the transaction. Its
   non-locking sibling `fetch_item_tx` (`:1012-1017`) reads the same row through
   the same builder with `for_update = false`; substituting it is exactly the
   over-allocation above.
3. `:406` — `state.consume(quantity)`, a pure domain transition
   (`backend/crates/inventory/domain/src/lib.rs:439`) whose `checked_sub` at
   `:451` refuses a negative result with a typed conflict at `:453`. The
   conservation predicate lives in a domain crate, not in SQL, which is what
   ADR-0001:20's dependency rule requires of it.

Only then does the event INSERT run (`:409-441`) and the cached balance update
(`:443-450`). The CHECK at `0156:103` witnesses the arithmetic of a row the lock
already made safe. Ordering matters: the lock is mechanism, the CHECK is
evidence.

**A correction to the premise this record was drafted from.** The supporting
analysis claims every shipped write site takes exactly one lock, and concludes
that an N-into-1 merge would introduce a deadlock class with no precedent here.
The code says otherwise. The cycle-count approval path already locks a set of
rows in a deterministic order: the count row under `FOR UPDATE OF c` (`:1485`),
then its variance lines under `ORDER BY item_id … FOR UPDATE` (`:776`), then each
item row through `fetch_item_for_update_tx` in that same key order (`:783`).
Multi-row ordered locking is shipped, not novel. What remains true is the part
that matters: a merge must *name* its serialization point and lock order, and it
may not inherit one by accident.

**`object_links` cannot carry this, quantity aside.** The generic edge table
(`0102_create_object_types_and_links.sql:53-69`) has no quantity column, and its
`UNIQUE (org_id, src_kind, src_id, dst_kind, dst_id, link_type)` at `:68` permits
at most one edge of a given type between an ordered pair per tenant. A second
partial draw between the same source and destination is therefore
unrepresentable, and a split/merge DAG with repeated edges is not expressible
there even if quantity were ignored. Amending an edge in place is also
unavailable: `:86` grants `SELECT, INSERT, DELETE` to `console_rt` and
deliberately no UPDATE, so a corrected edge is a delete plus an insert and the
audit line records the destruction.

## Decision

Quantity-bearing lineage is **deferred**. No `lot`, `lot_split`,
`lot_derivation`, or recursive-traversal table is created by this record, and no
migration slot is claimed. In exchange, four constraints bind any later design
so the deferral does not become a trap.

1. **Conservation requires row-level pessimistic locking of the row whose
   balance is being changed.** The authoritative read and the write must occur
   inside one transaction that holds `FOR UPDATE` on that row, exactly as
   `inventory/adapter-postgres/src/lib.rs:394` does today. A design that claims a
   row-level CHECK alone is sufficient for conservation is wrong, and this record
   exists so that claim is not re-litigated.
2. **A row-level CHECK is a per-row backstop and may be described as nothing
   more.** `0156:103` and `0191:119` are legitimate and should stay. Neither may
   be cited as the reason a balance cannot be over-allocated, in an ADR, a
   design note, a test name, or a code comment.
3. **The conservation predicate stays a pure domain function.** It belongs where
   `InventoryItemView::consume` (`inventory/domain/src/lib.rs:439`) already puts
   it — a total, side-effect-free transition returning a typed refusal — not in a
   trigger, a deferred constraint, or a denormalized parent total. This is
   ADR-0001:20's dependency rule applied to an invariant, not a new rule.
4. **A merge must name its serialization point and lock order before it is
   accepted.** The N-into-1 case acquires more than one lock, so it must declare
   which rows it locks, in which total order, and what it does on lock timeout.
   `:776`/`:783` is the available precedent for a deterministic key order; a
   design that acquires the same set of locks in a different order in a different
   code path is rejected.
5. **Quantity-bearing and lineage edges may never live in `object_links`.** The
   uniqueness constraint at `0102:68` and the absent UPDATE grant at `:86` make
   the table structurally unable to hold a repeated or revisable weighted edge.
   Lineage, when it arrives, gets its own table.
6. **The deferral reopens on a counted requirement, not on an argument.** One
   named caller with a real lot, batch, or genealogy need reopens this record. A
   pattern catalogue, a competitor comparison, or an anticipated ERP module does
   not.

If a `TRANSFER` movement kind is proposed before lineage exists, it carries its
from/to location pair on **one row**. Widening the `kind` CHECK at
`0191_create_inventory_cycle_counts.sql:97` while leaving the single
`stock_location_id` at `:96` would produce two rows on an append-only table whose
pairing is recorded nowhere.

## Alternatives considered

### Build the lineage DAG now, with "children sum to parent" enforced

Rejected on cost and sequencing. In PostgreSQL that invariant is not a CHECK: it
needs either a deferred constraint trigger firing on every insert or a
denormalized parent total, on the hottest write path of a domain that is not the
first vertical. The prerequisites are absent too — `inventory_movements`
(`0191:91-127`) has no parent-movement reference and no from/to pair — so this is
a replacement of the shipped model, not an increment on it.

### Keep the deferral but say nothing about the mechanism

Rejected. This is the option that costs later. The stale story — that the row
CHECK enforces conservation — is the one already written down, and left standing
it becomes the premise of the next design. The deferral is cheap to reverse; a
lineage table built on a per-row CHECK is not.

### Model lineage as `object_links` edges with a quantity attribute

Rejected as unavailable, not merely undesirable. The table has no quantity
column, `:68` permits one edge per `(org, src, dst, link_type)`, and `:86` grants
no UPDATE. Adding a quantity column to the generic edge table would put a
conserved value on a row no lock protects and no domain function validates,
reproducing the exact defect this record names.

### Declare conservation solved because the shipped inventory path is correct

Rejected. The shipped path *is* correct, and that is the problem: its correctness
comes from `:376`, `:394` and `:406`, which a future table would not inherit by
writing a similar CHECK. Correct code is not a transferable guarantee; a named
constraint is.

## Consequences

- **No migration slot is consumed and the first vertical is not taxed.** HR +
  payroll delivery is unaffected by a lineage model it does not use.
- **The mechanism is recorded where a later lane will read it.** The next design
  that touches a conserved quantity inherits "lock the row, keep the predicate
  pure, treat the CHECK as evidence" as a constraint rather than rediscovering it
  from an over-allocation in production.
- **A falsifiable claim, not an assertion.** Constraint 1 is testable without new
  code: two concurrent 60-unit consumptions against a 100-unit item must yield
  one success, one conflict, and a final balance of 40; swapping
  `fetch_item_for_update_tx` for the existing `fetch_item_tx` (`:1012-1017`) must
  admit both writes and consume 120 from 100. If the non-locking control does not
  over-allocate, constraint 1 is wrong and this record must be revised.
- **Inventory and production carry a real capability gap for as long as the
  deferral holds.** No lot traceability, no recall by batch, no yield or scrap
  accounting. That is the accepted price, and it must be quoted honestly to any
  caller who asks for those features rather than described as "planned".
- **A future merge pays a design cost this record makes explicit.** Naming a lock
  order and a timeout policy is work that a schema-first design would have
  skipped and then paid for as an intermittent deadlock.
- **Constraint 2 creates a documentation obligation with no automated
  enforcement.** No gate detects a comment or design note that misattributes
  conservation to a CHECK; only review does. This is a weaker mechanism than the
  invariant it protects, and it is recorded as such rather than overstated.

## Reciprocal record edits landed on acceptance

This record declares `related` only. No amendment or supersession is asserted,
and no accepted record acquired an `amended_by` key from it
(`docs/decisions/README.md:9`, `:26`). The following edits landed in
the same atomic commit as the status change:

1. **`related` additions.** `ADR-0001`, `ADR-0002`, `ADR-0018`, and `ADR-0029`
   each added `ADR-0035` to their own `related` list — for ADR-0029, this was
   possible because it was itself accepted first, in the preceding commit of the
   same pass.
   `related` reciprocity is not
   machine-enforced — `scripts/check-adrs.mjs:23-27` lists only the
   `amends`/`amended_by` and `supersedes`/`superseded_by` pairs in
   `RECIPROCAL_RELATIONSHIPS`, and `:248-249` validates `related` as an inline
   array only — so this is a README:9 obligation kept by discipline, not by the
   gate.
2. **Index row.** The `ADR-0035` row in `docs/decisions/README.md` changed its
   status cell from `proposed` to `accepted`. `scripts/check-adrs.mjs:461-464`
   fails the build if the index status and the frontmatter status disagree, so
   this edit was not optional.
3. **No target sentence is edited by this record.** Its constraints add to
   ADR-0001:20, ADR-0002:20 and ADR-0018:94; they do not make any sentence in
   those records false. One stale sentence did sit nearby — ADR-0002:20's
   exclusion count — and it belonged to ADR-0029, not here; ADR-0029 corrected it
   on its own acceptance. A `related` key cannot license editing another record's
   Decision text.

## Follow-ups

1. **Run the two-transaction probe** described under Consequences before any
   lineage design is proposed, and attach its output. It requires a scratch
   database and no new code.
2. **The stale clause in ADR-0002 was another record's business, and is now
   resolved.** ADR-0002:20 used to state that the audit-coverage gate's "exclusion
   set contains exactly one entry — the LocationPing ingestion path". The
   executable gate returns **two**: `allowed_audit_exclusions()` at
   `backend/ci/gates/audit-coverage/src/lib.rs:90` yields `record_location_ping`
   (`:95`) and `purge_expired_location_data` (`:104`). ADR-0029 owned that
   reconciliation and corrected the sentence on its own acceptance; this record
   never duplicated it. What matters here is the
   scope of the reliance: this record leans only on ADR-0002:20's transaction
   ordering, which the inventory path honors at
   `inventory/adapter-postgres/src/lib.rs:360`. The one-entry clause must not be
   copied forward into any follow-up text.
3. **Audit the existing prose for the misattribution constraint 2 forbids**
   before it propagates further. The claim that a row-level CHECK enforces
   conservation appears in planning material and is the reason this record was
   written.
4. **Do not schedule a lineage lane.** Reopening is triggered by a counted
   caller requirement, per constraint 6. A scheduled lane would convert a
   deferral into a commitment this record deliberately withheld.
