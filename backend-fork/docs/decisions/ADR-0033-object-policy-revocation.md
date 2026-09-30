---
id: ADR-0033
status: accepted
doc_status: review
date: 2026-07-30
owner: jasonlee
decision: object-policy-revocation-asymmetry
related: [ADR-0021, ADR-0023]
---

# ADR-0033: Object-policy revocation, and the one case that is genuinely unrevokable

## Status

**Accepted 2026-07-30.** `related` only. This record amends nothing and supersedes
nothing; it records a measured property of the shipped no-code
object-policy path and specifies what a revocation path would have to satisfy
before anyone builds one.

## Context

The planning premise this record was opened against — *"an over-broad permit
cannot be undone"* — is **false**, and inverted. The code says the opposite, and
where the planning evidence disagrees with the code, the code wins.

**An over-broad permit is correctable today, with no schema change.** `effect` is
a required field on the attach request with enum `[permit, forbid]`
(`backend/openapi/openapi.yaml:12290-12295`), passed through to the authored
block unchanged (`backend/crates/ontology/rest/src/lib.rs:522`). The definer
mints a fresh catalog `stable_key` per call —
`'object_policy.' || <type key> || '.' || <fresh uuid>`
(`backend/crates/platform/db/migrations/0205_ont_policy_api_attach_writer.sql:277-279`)
— and its own comment states that "two policies on one type is a supported
shape, not a conflict", so the catalog's `UNIQUE (org_id, stable_key, status)`
(`0150_create_cedar_policy_staging.sql:40`) is never reached and the
attachment's `UNIQUE (org_id, object_type_id, cedar_policy_id)`
(`0154_create_cedar_object_property_policies.sql:35`) is per catalog row, not
per type. Correcting a too-broad permit is therefore one further authenticated
attach call carrying `effect: forbid`: lowering ORs every permit into a single
parenthesised group and appends each forbid as
`AND NOT COALESCE(<clause>, FALSE)`
(`backend/crates/platform/authz/src/cedar_pbac/residual.rs:210-213`).

**What has no reversal is a mistaken FORBID.** The same three lines that make a
permit correctable make a forbid permanent:

- **No permit can out-permit a forbid.** Later permits only add OR-terms *inside*
  the group that every forbid is then `AND NOT`-ed against (`residual.rs:210-213`).
  There is no counter-effect in the grammar.
- **An unconditional forbid hides the whole type.** An empty condition list
  lowers to `TRUE` (`residual.rs:246-248`), so the filter becomes
  `COALESCE((…), FALSE) AND NOT COALESCE(TRUE, FALSE)` — no row matches, for
  anyone. Pinned by `unconditional_forbid_collapses_whole_filter_to_false`
  (`residual.rs:449-458`).
- **A forbid with an untranslatable term is worse than an unconditional one.**
  Any untranslatable term collapses the *entire* filter to `FALSE`
  (`residual.rs:188-193`), and `contains` is deliberately routed to a subject
  attribute no request carries (`ontology/rest/src/lib.rs:961`, `:965`), so a
  `contains` forbid denies the type outright.
- **It is org-wide, not subject-scoped.** Applicability is
  `block.action == "view" && block.resource_type == stable_key`
  (`ontology/rest/src/lib.rs:945`) — nothing narrows a forbid to a principal.
- **The attachment row cannot be edited or deleted.** `trg_*_no_update` and
  `trg_*_no_delete` call `cedar_policy_attach_append_only()`, which raises
  unconditionally (`0154:59-62`, `:91-97`), because "a policy attachment is an
  immutable link record" (`0154:67`).
- **This is live, not prospective.** The residual is applied on every ontology
  instance read: `list_instances` (`ontology/rest/src/lib.rs:553-559`),
  `visible_head_inner` (`:779-790`), and `visible_traversal` (`:818-856`), each
  through `object_view_policies` → `load_enforced_object_policy_blocks`
  (`:637`; `backend/crates/platform/authz-rest/src/store.rs:538-558`). It is a
  distinct path from ADR-0021's Cedar engine flip, which remains unpromoted.

Both halves of the asymmetry are deliberate and neither is a defect.
Fail-closed-by-construction is the module's stated contract — deny by omission,
forbid always wins, never silently drop a term (`residual.rs:12-21`) — and the
residual may only narrow beneath the independent RLS org floor
(`residual.rs:8-10`), which is exactly ADR-0021's row-boundary rule
(`ADR-0021:49-51`). Append-only attachment is likewise deliberate. The gap is
that the two together leave the *decision* reversible in principle with no
reversal *write* anywhere in the system.

There is also no reachable status transition, contrary to the planning note that
treated one as available. `status = 'retired'` is CHECK-legal (`0150:15`) and no
trigger blocks the UPDATE — `0150:106-110` installs only
`enforce_org_id_immutable` on the catalog. But **no role holds the privilege**:
`0150:118` revokes `INSERT, UPDATE, DELETE` on `cedar_policy_catalog_entries`
from `console_rt`, and `0205:170` grants the ontology writer `SELECT, INSERT`
only, with `0205:168-169` stating why ("It gets no UPDATE and no DELETE"). A
grep of `backend/crates` finds no code that writes `'retired'` to this table.
`retired` is legal in the CHECK and unreachable in the system.

Finally, who may write policy at all is changing underneath this question.
PR #526 (open, read 2026-07-30) adds
`0206_ont_policy_api_attach_command_role.sql`, moves the attach capability from
the runtime role to an EXECUTE-only audited command credential, and moves the
audit INSERT into the definer. A revocation mechanism specified against today's
credential map would be stale before it merged.

## Decision

1. **Record the asymmetry as a known property of the shipped system, and retire
   the inverted premise.** An over-broad permit is correctable by attaching a
   forbid. A mistaken forbid has no reversal write. Any document, plan, or gate
   rationale asserting that a permit is the unrevokable case is wrong and may
   not be cited as evidence.
2. **Do not build a revocation mechanism on this record.** No mistaken forbid has
   been counted in any environment. Specification now, construction when a real
   one is counted — a mechanism nobody has needed is speculative, and a
   half-shipped one is worse than none (clause 4).
3. **Treat the attach route's forbid arm as a one-way write until a revocation
   path exists.** Operator-facing surfaces and runbooks must say so at the point
   of the write, and an unconditional or `contains`-conditioned forbid must be
   presented as hiding the entire object type for the entire organization, not
   as a narrowing of one permit.
4. **When revocation is built, it ships as one change or not at all.** The change
   must satisfy every clause below; a partial landing is refused.
   1. **Revocation is a catalog status transition, not an attachment edit** —
      `cedar_policy_catalog_entries.status` `'enforced'` → `'retired'`. A
      `detached_at` column on `ont_object_policies` is not an option: the UPDATE
      is blocked by TRIGGER, not privilege (`0154:59-62`, `:91-97`), so that
      design silently requires dropping an append-only trigger.
   2. **It requires a new privilege, not a status write.** Since no role can
      UPDATE the catalog (`0150:118`, `0205:168-170`), revocation is a new
      audited definer routine plus one narrow grant to it — and that routine
      must be reached by the *same* credential class that owns attach at the
      time it ships, so the platform does not grow a second policy-write
      authority alongside PR #526's.
   3. **It is audited in the caller's transaction**, in the shape the attach
      writer already uses, so the audit row commits or rolls back with the
      retirement.
   4. **The same change adds `AND c.status = 'enforced'` to both
      `OBJECT_POLICY_SELECT` (`store.rs:820-826`) and `PROPERTY_POLICY_SELECT`
      (`store.rs:828-834`).** Neither filters status today; both filter only
      `generated_policy_text IS NOT NULL`. A retirement honoured on the
      enforcing read and ignored on the point-decision read is worse than no
      retirement at all.
   5. **It states its limits rather than implying more.** Retirement restores
      visibility going forward. Because the residual is a SQL `WHERE` fragment,
      a row filtered out produces no denial record, so retirement cannot tell an
      operator which reads were denied while the forbid stood. Reconstructing
      that blast radius is a separate problem and must not be claimed as
      delivered.
   6. **It weakens no enforcement invariant.** Deny-by-omission
      (`residual.rs:200-203`) and forbid-always-wins (`:210-213`) stay exactly as
      they are. Retirement removes a policy from the applicable set; it never
      introduces an out-permit.

## Decision drivers

- **The measured asymmetry, not the argued one.** The premise was checked against
  `residual.rs` and inverted; a record that repeated it would propagate a false
  fact into the authority chain.
- **The irreversible write is the one carrying less protection than the
  reversible one.** Attaching a forbid is permanent and org-wide; the cost of
  getting it wrong falls entirely on the write's own ceremony. Whether that
  ceremony is materially thinner than a role-assignment write's is unmeasured
  here and is left as a follow-up rather than asserted.
- **Fail-closed is correct and stays.** Every alternative that gives forbid a
  counter-effect trades a live safety property for an authoring convenience.
- **Rung one applies.** No counted incident exists. Specifying a mechanism costs
  a document; shipping an unneeded one costs a privilege, a definer, a migration
  slot, and two read-path edits.
- **The write-authority boundary is in motion.** PR #526 is open and unmerged, so
  naming a credential today would pin the mechanism to a map that is being
  replaced.

## Alternatives considered

### Give `forbid` a counter-effect in the residual grammar

Rejected. It inverts `residual.rs:12-21`'s stated contract. A permit that can
out-permit a forbid means the enforcing read no longer fails closed, and the
property lost is far larger than the authoring inconvenience gained.

### `detached_at` on `ont_object_policies` as the revocation mechanism

Rejected as mis-specified. The blocking control is
`cedar_policy_attach_append_only()`, which raises unconditionally on UPDATE and
DELETE (`0154:59-62`, `:91-97`). No grant makes that UPDATE succeed, so the
design's real cost is dropping an append-only trigger on a policy table —
unstated in the proposal and unacceptable.

### Hard-delete the catalog row

Rejected. `ont_object_policies.cedar_policy_id` is
`REFERENCES cedar_policy_catalog_entries(id, org_id) ON DELETE CASCADE`
(`0154:36`), so deleting the catalog row would cascade the attachment away — and
the attachment table's `trg_*_no_delete` exists precisely to keep that link
record permanent. It would also destroy the evidence of what was enforced and
when, which is the point of retaining a retired status value.

### Ship the status transition now, add the read-path filters later

Rejected, and it is the failure clause 4 exists to prevent. A retirement written
to the catalog while `OBJECT_POLICY_SELECT` and `PROPERTY_POLICY_SELECT` ignore
status yields an operator who believes a policy is revoked and a read path that
still honours it. Two inconsistent answers about a live authorization decision
is a worse state than one consistent no.

### Do nothing and leave the property undocumented

Rejected. The property is live on three read paths today, the premise in
circulation about it is inverted, and the write that triggers it is presented to
operators as an ordinary attach.

## Consequences

- **Positive — the authority chain stops carrying an inverted fact.** The
  permit-is-unrevokable claim has a named refutation with executable citations,
  so a later reviewer can check it in one read instead of re-deriving it.
- **Positive — the enforcement invariants are untouched.** Nothing here weakens
  deny-by-omission, forbid-always-wins, the RLS floor, or append-only
  attachment. No gate is relaxed and no production exposure widens.
- **Positive — no migration slot is spent.** The mechanism is specified without
  claiming one, so it cannot collide with the migration ordering PR #526 is
  already moving.
- **Positive — the eventual change is bounded before it starts.** Six clauses,
  one landing, and a refusal condition, rather than a mid-build discovery that
  the privilege does not exist.
- **Negative — a mistaken forbid remains unrecoverable for as long as clause 2
  holds.** The only remedies are the ones that exist today: recreate the object
  type under a new key and re-attach correct policies, or restore from backup.
  Both are expensive and neither is a revocation.
- **Negative — the interim control is procedural.** Clause 3 is a documentation
  and UI obligation, not a mechanical one; nothing in the code prevents an
  operator from attaching an org-wide forbid by accident.
- **Negative — the blast radius of a past forbid stays unmeasurable.** No
  read-side denial record exists, so even after revocation ships, nobody can
  enumerate what was hidden.
- **Neutral — this defers rather than settles the ceremony question.** Whether
  the forbid write should require step-up authentication or an impact preview is
  named as a follow-up and is unmeasured here.

## Reciprocal records landed on acceptance

This record declares `related` only, so **no target ADR gained a relationship key
and no existing ADR file changed.** That is a checked conclusion, not an
omission:

- **ADR-0021** — no Decision sentence becomes false. Its row-boundary rule
  (`ADR-0021:49-51`) is what the residual already honours (`residual.rs:8-10`),
  and its bundle-cache clause (`:55-56`) permits caching only by immutable
  version/digest keys while `load_enforced_object_policy_blocks` re-reads the
  catalog per request (`store.rs:538-558`) — so a future retirement needs no
  cache-invalidation clause added to ADR-0021.
- **ADR-0023** — object-policy revocation is not in its charter. The nearest
  material is its "Follow-ups (named out of scope for this program)" list
  (`ADR-0023:148-156`), and an accepted ADR is authoritative "within its stated
  scope" (`docs/decisions/README.md:7`) — out-of-scope is silence, not
  prohibition. Nothing to amend, nothing to withdraw.
- **`docs/decisions/README.md`** — one index row was added for this record at status
  `proposed`, per the gate's index requirement. On acceptance the only reciprocal
  edits were that row's status cell, `proposed` → `accepted`, plus one line in the
  effective relationship graph stating that this record is `related`-only and
  amends nothing.

## Follow-ups

1. **Count mistaken forbids before building anything.** Clause 2's trigger is a
   real incident in a real environment. Until one is counted, the specification
   stands unbuilt.
2. **Run the retirement probe on a scratch database when clause 4 is triggered.**
   Attach an unconditional forbid through the shipped route, confirm reads return
   zero rows, flip `status` to `'retired'` by superuser, and confirm
   `load_enforced_object_policy_blocks` returns no blocks and the rows reappear.
   The known-bad control is the same flip on a type carrying only a permit: it
   must **not** make rows visible, because permits-empty hits deny-by-omission
   (`residual.rs:200-203`). If the control also flips to visible, the probe is
   observing an unfiltered read and proves nothing.
3. **Measure the ceremony asymmetry rather than asserting it.** Compare the
   protections on the forbid attach write against those on a role-assignment
   write. If the irreversible write is demonstrably thinner, that measurement —
   not this record — is the basis for requiring step-up or an impact preview.
4. **Correct the point-decision read scope in the planning evidence.** The
   adjudication's claim that revocation would be "silently ignored on the
   point-decision read" is narrower than stated: policies authored through the
   attach route store `generated_policy_text` as NULL by construction
   (`0205:22-36`, `:273`) and both selects filter
   `generated_policy_text IS NOT NULL`, so they never reach either one. The gap
   is real only for catalog rows that do carry text. Both filters are still
   required by clause 4.4; the justification is consistency across reads, not a
   leak in the attach route.
5. **Re-verify every citation in clause 4 against the merged state of PR #526.**
   It rewrites `0205`, adds `0206`, and edits `store.rs`; the line anchors here
   were read on 2026-07-30 against `main`.
