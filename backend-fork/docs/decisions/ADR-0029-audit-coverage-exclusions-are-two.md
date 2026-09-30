---
id: ADR-0029
status: accepted
doc_status: review
date: 2026-07-30
owner: jasonlee
decision: audit-coverage-exclusion-cardinality-and-binding
amends: [ADR-0002, ADR-0014]
amended_by: [ADR-0040]
related: [ADR-0002, ADR-0014, ADR-0035, ADR-0040]
---

# ADR-0029 — Audit-coverage exclusions are two, bound to a (file, function) pair

## Status

**Accepted 2026-07-30.** A retroactive reconciliation under README:6: the code is
right and the record was false. This record amends ADR-0002's audit-coverage
exclusion sentence and authorises no new carve-out. ADR-0014's destructible-store
decision is unaffected, but its closing Decision sentence repeated the same false
count, so at acceptance the owner took the first of the two remedies this record's
final section names and widened its scope to `amends: [ADR-0002, ADR-0014]` — both
false sentences are corrected here rather than one, and the ADR-0014 reciprocity is
carried in the final section alongside ADR-0002's.

**Amended 2026-08-10 by ADR-0040**, which authorises a third bound carve-out for
console route-telemetry ingestion after `backend/app/src` entered the handler-surface
set. The two LocationPing bindings below remain in force; only the closed-at-two
cardinality is lifted.

## Context

`ADR-0002:20` states, verbatim: "its exclusion set contains exactly one entry —
the LocationPing ingestion path (ADR-0014) — and a test asserts that is the only
exclusion."

The gate returns two. Read from the executable code, not from prose about it:

- `backend/ci/gates/audit-coverage/src/lib.rs:90-107` — `allowed_audit_exclusions()`
  returns two entries: `location_ping_ingestion`, bound to
  `crates/compliance/adapter-postgres/src/lib.rs` and `record_location_ping`
  (`:92-96`), and `location_data_retention_purge`, bound to the same file and
  `purge_expired_location_data` (`:101-105`).
- `backend/ci/gates/audit-coverage/tests/gate_detects_violation.rs:26-46` — the
  test named in ADR-0002 is `allowed_exclusion_set_is_the_two_location_carveouts`.
  It asserts `exclusions.len() == 2` (`:28`) and then pins each entry by reason,
  file, and function (`:32-45`). It asserts the opposite of what ADR-0002 says it
  asserts.
- Both exemption comments exist in the tree at
  `backend/crates/compliance/adapter-postgres/src/lib.rs:273` and `:386`, on the
  writers they are bound to (`:274`, `:392`).
- The cardinality assertion executes. `.github/workflows/ci.yml:462-470` runs the
  gate's mutation suite under Buck2; the comment at `:448-461` records why that
  step exists — `cargo run -p console-gate-audit-coverage` (`:428`) proves only
  that the gate exits 0 against this tree, which a gate scanning nothing also
  does.

Under README:6 a divergence of this shape is a governance gap, not silent
supersession, so it is reconciled by a new decision rather than absorbed.

Three further prose sites still say "one". All are documentation, not governance:
the gate's own module doc (`backend/ci/gates/audit-coverage/src/lib.rs:9-11`,
"The only allowed carve-out is LocationPing ingestion"),
`backend/crates/kernel/core/src/audit.rs:2-4` ("the only carve-out is
`LocationPing` ingestion"), and `ADR-0014:20`. The ADR prose was cited downstream
in place of the gate, which is how one false premise propagated into planning
evidence.

What the code enforces is not the invariant ADR-0002 described, and it is
stronger. Each exclusion is keyed on a `(file, function)` pair, not on a reason
string: `exclusion_matches_site` (`src/lib.rs:267-272`) requires exact
function-name equality plus a repo-relative path suffix match, and a reason used
anywhere else yields `MisboundAuditExclusion` (`:40`, `:208-218`) — proven for a
different crate (`tests/gate_detects_violation.rs:269-308`) and for a different
function inside the bound file (`:310-344`). A reason outside the literal set is
`UnknownAuditExclusion` (`src/lib.rs:219-228`, test `:346-373`), and reusing a
bound reason twice is `DuplicateAuditExclusion` (`src/lib.rs:192-201`).

## Decision drivers

- An authoritative record asserting a falsifiable count that CI contradicts is
  worse than no assertion: it is cited as evidence, and it was.
- Cardinality is a weak proxy for the property ADR-0014 actually needs — that raw
  coordinates never reach `audit_events` and that the carve-out cannot migrate to
  another writer. The `(file, function)` binding enforces that property directly.
- A count in prose can be satisfied by renaming; a bound pair cannot. The control
  should live where the diff is visible.

## Decision

1. **The allowed-exclusion set is closed at exactly two named entries**, each
   bound to a `(file, function)` pair, with one test asserting the set in full —
   length and every binding (`backend/ci/gates/audit-coverage/src/lib.rs:90-107`;
   `tests/gate_detects_violation.rs:26-46`).
2. **An exclusion is bound to a (file, function) pair, not to a count.** Adding,
   moving, or renaming one is an edit to that literal set and to the test that
   pins it, so it appears in review as a named writer gaining an exemption rather
   than as a number going up. A cardinality claim on its own is not a control and
   is not to be restated as one.
3. **This reconciliation is retroactive and authorises nothing new.** The second
   carve-out is already in `main` and already bound and tested; what was missing
   was the record. No third exclusion is approved here, and none may be added
   without a further accepted decision.
4. **ADR-0014's carve-out survives unchanged in substance.** Its "exactly one
   path" phrasing was a proxy for the binding; the binding now carries it. The
   retention purge erases expired location-derived data to honour the retention
   window ADR-0014 requires, which is why it is data-lifecycle maintenance rather
   than an auditable business write (`src/lib.rs:97-100`).
5. **All four stale prose sites must be corrected**, as documentation. The two ADR
   sentences named in the final section are corrected in the acceptance change itself.
   The two source comments — `backend/ci/gates/audit-coverage/src/lib.rs:9-11` and
   `backend/crates/kernel/core/src/audit.rs:2-4` — were **not** corrected in the
   acceptance change, which touched `docs/decisions/` only; they remain owed at
   Follow-up 2 and no further decision is needed to make them.

## Alternatives considered

### Reduce the gate to one exclusion so ADR-0002:20 becomes true

Rejected. `purge_expired_location_data` is a real writer that must not emit audit
events, and deleting its exemption would either fail the gate or push the purge
outside the scanned surface. Editing code to make a record true inverts README:6.

### Leave ADR-0002:20 in place and record the second carve-out only here

Rejected. A reciprocal key without a text edit leaves a false sentence standing in
an authoritative document, where it will be read and cited before any relationship
field is followed. That is precisely the failure this record exists to close.

### Keep the invariant as a count and drop the (file, function) binding

Rejected, and it would weaken the gate. Under a count-only rule the
`location_ping_ingestion` reason could be applied to any other handler and still
satisfy "one exclusion"; `tests/gate_detects_violation.rs:269-344` shows the bound
form rejecting exactly that.

### Assert non-empty `audit_events` at runtime for every wrapped write

Rejected here as out of scope and unsafe as stated: `with_audits`
(`backend/crates/platform/db/src/audit_tx.rs:111`) accepts an empty
`Vec<AuditEvent>`, and an empty vector is legitimate on idempotent receipt replay.
A blanket runtime assertion would abort correct transactions. Any narrower rule
belongs to a decision about the write path, not to the exclusion set.

## Consequences

+ The exclusion invariant stated in the decision record and the invariant CI
  enforces become the same sentence, and it is the stronger of the two.
+ Adding an exclusion becomes a reviewable change to a named writer, which is
  visible in a diff without reading a count.
+ The three "only carve-out" prose sites stop functioning as citable evidence for
  a cardinality the gate does not enforce.
− A governance gap is being closed after the fact. The second carve-out ran in
  `main` without a record, and this record cannot retroactively supply the review
  it never received.
− The exclusion set stays hard-coded in Rust, so a lawful future carve-out costs a
  code change and a release. That cost is the intended friction.
− Nothing here widens gate coverage. `is_handler_surface` (`src/lib.rs:450-455`)
  still matches only path components `application`, `rest`, and `worker`, plus the
  compliance adapter (`:461-463`). `backend/app/src/` is scanned but never
  classified as a handler surface, and zero `console-gate: state-changing-handler`
  markers exist under it — verified by grep. Those writers are audited by
  discipline, not by this gate.

## Follow-ups

1. **Measure before extending the handler surface.** Extending
   `is_handler_surface` to match component `app` is the obvious next control, but
   its blast radius is unmeasured. Run the gate over the workspace with the
   predicate extended and report the violation count, with a planted unwrapped
   writer under `backend/app/src/` as the known-bad control. Treat the count as
   unknown, not zero, until that runs; the change is not proposed by this record.
2. **Correct the two source comments** at
   `backend/ci/gates/audit-coverage/src/lib.rs:9-11` and
   `backend/crates/kernel/core/src/audit.rs:2-4`. **Outstanding:** the acceptance
   change was documentation-only and did not reach either file, so both still read
   "the only carve-out" against a gate that returns two.
3. **Sweep planning evidence under `docs/program` and `docs/ideas`** for the
   one-exclusion count and for citations of `ADR-0002:20` as authority on it.

## Reciprocal records landed on acceptance

README:9 requires amendment to be explicit in both records, and README:26 requires
relationship keys to be reciprocal where applicable. All of the below landed in the
same change as the status flip; `check-adrs.mjs:414-419` rejects an `amended_by`
pointing at a non-accepted ADR, so the status flip and the keys are one commit.

**Scope at acceptance.** Item 5 below offered the owner two remedies for ADR-0014's
twin false sentence. The first was taken: this record's `amends` names **both**
ADR-0002 and ADR-0014, and items 1-3 carry ADR-0014's reciprocity alongside
ADR-0002's. No separate record was issued.

**1. ADR-0002 and ADR-0014 frontmatter.**
`ADR-0002-auditfirst-transactional-discipline-audit-event-in.md`
gained `amended_by: [ADR-0029]`. Its frontmatter (`ADR-0002:1-9`) carried
`id, status, doc_status, date, owner, consensus, related` and no relationship key
other than `related: [ADR-0014]` (`:8`) — so this **created** the key rather than
appending to it. `related` became `[ADR-0014, ADR-0029]`, and gained `ADR-0032`,
`ADR-0035`, and `ADR-0036` from their own acceptances in the same pass.
`ADR-0014-locationping-destructible-store-carved-out-of.md` likewise gained
`amended_by: [ADR-0029]` — also a created key — and `ADR-0029` in its `related`.
This record's `proposes_amendments_to` was replaced by `amends: [ADR-0002, ADR-0014]`
at the same moment; `check-adrs.mjs:405` fails the build if either half is missing.

**2. ADR-0002's Decision text, edited in place.** `ADR-0002:20` previously read:

> A CI `audit-coverage` gate (T0.4) fails the build if a state-changing handler
> emits no audit event; its exclusion set contains exactly one entry — the
> LocationPing ingestion path (ADR-0014) — and a test asserts that is the only
> exclusion.

That clause was replaced with:

> A CI `audit-coverage` gate (T0.4) fails the build if a state-changing handler
> emits no audit event; its exclusion set contains exactly two entries — the
> LocationPing ingestion path and the expired-location-data retention purge
> (ADR-0014, ADR-0029) — each bound to a specific (file, function) pair, and a
> test asserts that set in full.

**3. README index rows.** The ADR-0002 row changed status `accepted` →
`accepted, amended` and gained the scope note, following the ADR-0005
and ADR-0023 precedent:

> | [ADR-0002](ADR-0002-auditfirst-transactional-discipline-audit-event-in.md) | accepted, amended | Audit event in the same transaction; append-only audit store; exclusion-set cardinality and binding amended by ADR-0029 |

The ADR-0014 row changed the same way, keeping its own authored scope sentence and
gaining the amendment clause. ADR-0029's own row moved from `proposed` to
`accepted`, and the effective relationship graph gained: "ADR-0029 narrowly amends
the audit-coverage exclusion sentence in ADR-0002 and in ADR-0014, records a
pre-existing governance gap under authority rule 6, and does not amend ADR-0002's
same-transaction or append-only decisions or ADR-0014's destructible-store
decision."

**4. ADR-0014's Decision text, edited in place.**
`ADR-0014:20` previously ended: "The audit-coverage gate's exclusion set contains
exactly this one path, and a test asserts it is the only exclusion." That sentence
was false for the same reason and against the same evidence. ADR-0014's decision — a
separate destructible store, coordinates never entering `audit_events`, consent
lifecycle audited — is untouched; only the cardinality clause was wrong, and it now
reads:

> The audit-coverage gate's exclusion set contains this path bound to its exact
> writer, alongside the expired-location-data retention purge recorded in
> ADR-0029, and a test asserts that set in full.

**5. Why this record amends two ADRs rather than one.** As drafted, this record's
`proposes_amendments_to` named ADR-0002 only, which left the twin sentence in
ADR-0014 standing — fixing one false sentence and leaving its identical partner in
place. The draft required the owner, before acceptance, either to extend the record
to both targets and carry the ADR-0014 reciprocity in items 1-3, or to issue a
separate record. **The first was chosen at acceptance**, because both sentences are
false against the same two lines of the same gate and a second record would have
split one evidentiary finding across two decisions.
