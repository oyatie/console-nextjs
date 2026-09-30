---
id: ADR-0034
status: accepted
doc_status: review
date: 2026-07-30
owner: jasonlee
decision: delegation-of-authority-routing
related: [ADR-0002, ADR-0003, ADR-0018, ADR-0023, ADR-0025, ADR-0028]
---

# ADR-0034 — 전결규정 routing as a delta on the approval-line model

## Context

ADR-0023 already decides the approval-line model: a generalized definition builder for arbitrary
approval-line DAGs with dynamic 결재선 and 검토/승인/합의/참조 roles, and a **pre-terminal finalization
model** in which 최종승인 and 수령확인 are pre-terminal `WAITING` nodes — never a reopened terminal run —
with 사후 반려 modelled as a compensating document or event. ADR-0018 decides the engine that runs it.

This record decides only the delta. It does not restate or reopen either.

Two concepts are absent from the accepted record — verified by search across `docs/decisions/`, including
`notes/`: **전결** and the **capacity** a signature is made under. They are the delta. **Competence is not
one of them**: ADR-0028 Decision 6 decides its shape, and this record defers to it.

## Decision

**Routing is a lookup, not an escalation.** A 전결규정 rule maps *(document category × amount band ×
scope)* to the unit competent to decide, and the resolved unit may sit **above, laterally, or below** the
raising unit. A 전담 unit holds terminal authority for its category and does not escalate. Modelling this
as "escalation" would make the common case — a matter closing where it arose — look like an exception.

**Competence is authored, and it takes the shape ADR-0028 decided — not a third relation.** Which unit
may *decide* is not derivable from who owns whom (control) or who reports to whom (structure), so it must
be authored. But it does **not** need a new relation to be authored: ADR-0028 Decision 6 makes competence
a **condition attribute on a custom role**, taking the shape the `"team"` arm already has
(`authz/src/lib.rs:1421-1425`) — a subject-side predicate that gates whether the role applies and leaves
`BranchScope` untouched. ADR-0028 rejects the third-relation alternative on the merits, and it is right to:
the shipped `"team"` arm already demonstrates the behaviour without a scope-type change.

**This clause was corrected at acceptance.** The draft asserted competence *was* a third relation, written
without reading ADR-0028's Decision 6, and both records were accepted in the same pass — so for one commit
two accepted records decided the same question in opposite directions. Recorded rather than quietly
rewritten, because a contradiction between accepted records is the one class of governance defect
`check-adrs.mjs` cannot see: it reciprocates `amends`/`amended_by`, so a `related`-only record can
contradict its own related target silently.

**A signature records the capacity it was made under**, not only the signer. When one person holds several
grants that could each authorise the same act, which one applied is what decides whether 전결규정 was
satisfied. A signer identity alone cannot answer that question after the fact.

## Decision drivers

- 전결규정 exists to make matters **terminal at lower levels**. A model that only escalates inverts its
  purpose.
- The pre-terminal finalization model ADR-0023 already decides is the right substrate; what it lacks is a
  way to *choose* the line, not a way to run it.
- Korea's controls are `HOLD` and this record asserts nothing about Korean law. 전결 is treated here as a
  configurable organisational authority semantic, which is what it is in Korean approval products.

## Alternatives considered

**Escalation-only routing, where a line always climbs.** Rejected: it cannot express a 전담 연락사무소 that
closes a matter its 본사 never sees, and that case is ordinary rather than exceptional.

**Deriving competence from the structural hierarchy.** Rejected: an HR officer whose 소속 is a subsidiary
may hold group-wide competence. Deriving competence from structure makes that case an override, and
overrides are where audit trails degrade.

**A third approval engine.** Rejected outright. `orgchange` already added one beside `work_orders` and
`gov_approvals`; a fourth would compound the problem this record exists to avoid.

## Two corrections to the evidence this delta was argued from

Both were verified against code for this record, and both had been asserted the other way in planning
documents.

**`gov_approvals` is one signature per NODE, not per request.** Its `UNIQUE (org_id, request_ref)`
constraint reads as one-decision-per-request only if `request_ref` is a request. It is not: the
`org_change_step` insert in `crates/orgchange/adapter-postgres` binds `request_ref` to `step_id` and
`requested_by` to `request.drafted_by`. **An N-node approval line therefore already ships in production.**
Any design premised on "a 결재 line cannot exist in the governance spine" is premised on a false reading,
including the proposal to introduce a separate signature entity for that reason.

**The self-approval invariant is enforced in the database**, by `CHECK (approver_id <> requested_by)` in
`0153_create_governance.sql`. It is the only DB-enforced four-eyes invariant in the repository.

## The tension this record does not resolve

Capacity-bearing signatures and that `CHECK` are in genuine conflict. If a signature's legitimacy depends
on the grant it was made under, then **the same person signing twice under two different capacities becomes
expressible** — and the constraint that currently forbids it cannot distinguish the legitimate case
(delegated final authority) from the illegitimate one (self-approval wearing a second hat).

This record does not decide it, because the decision needs a rule about which capacities may co-occur on
one line, and that rule belongs to the 전결규정 vocabulary this delta only sketches. It is named here so
that whoever implements capacity does not discover the conflict by weakening the constraint.

## Consequences

- Competence is authored as a role condition attribute per ADR-0028, so the no-code authoring surface
  extends the existing role vocabulary rather than gaining a third one — cheaper than the draft assumed.
- Routing resolution becomes a lookup on the raise path, which places it on the latency budget of every
  document raise.
- Recording capacity requires the authorising grant to be identifiable at signature time, which couples
  this delta to the grant model rather than to the approval engine.
- Nothing here changes what ADR-0023 decided about finality. A matter still finalises at a pre-terminal
  node, and 사후 반려 remains a compensating document.

## Follow-ups

- Decide the co-occurrence rule for capacities on one line, and whether the DB-enforced self-approval
  constraint is amended, replaced, or left as the floor with capacity checked above it. **This blocks
  capacity-bearing signatures, not routing.**
- Determine whether a 전결규정 rule set is a grid or a graph. The authoring surface assumes one canvas
  substrate for all authored vocabularies, and a rule table may not fit it.
- Reconcile with `work_order_approval_steps`, which caps at three steps with a fixed role vocabulary, and
  with `orgchange`'s own step ceiling. Three approval mechanisms exist; this record adds a routing layer
  above them rather than a fourth.

Every capability, evidence contract, jurisdiction binding, Korea control, review disposition, and exposure
state remains `HOLD`; this record records a decision and makes no completion, deployment, or
production-exposure claim.

## Reciprocal record edits on acceptance

This record declares `related` only — `[ADR-0002, ADR-0003, ADR-0018, ADR-0023, ADR-0025, ADR-0028]` —
and names no amendment or supersession, so no target ADR gained `amended_by` and no target's Decision
text became false on its acceptance. It named no `related` additions owed in its targets, and none
were made; the only reciprocal edit was the `docs/decisions/README.md` index row's status cell,
`proposed` → `accepted`, plus one line in the effective relationship graph.

`related` reciprocity is not machine-enforced (`scripts/check-adrs.mjs:23-27` pairs only
`amends`/`amended_by` and `supersedes`/`superseded_by`), so the one-sided `related` list above is a
README:9 obligation a later editor may choose to complete in the six targets. It was left as authored
rather than inferred.
