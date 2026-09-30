---
id: ADR-0027
status: accepted
doc_status: review
date: 2026-07-30
owner: jasonlee
decision: identity-linkage-is-human-asserted
amends: [ADR-0022]
related: [ADR-0002, ADR-0003, ADR-0004, ADR-0010, ADR-0017, ADR-0021, ADR-0022, ADR-0025]
---

# ADR-0027: Identity linkage is human-asserted; no platform identity row in Slice 0

## Status

**Accepted 2026-07-30 · doc_status `review`.** This record narrows ADR-0022 by adding
one prohibition its integration bullet does not cover, and defers the durable platform
identity handle out of Slice 0. It is a **narrowing**: it grants no new capability,
opens no new read path, and consumes no migration slot, which is what made it safe to
accept while the handle itself stays deferred. It is authoritative over ADR-0022 within
that stated scope (README:7, README:9).

Source: theme T1 / draft D1 of `docs/ideas/adr-adjudication.md`. Where that
adjudication's evidence disagrees with the code, the code is followed and the
divergence is stated.

## Context

### What ADR-0022 actually decides, stated precisely because it was over-read

ADR-0022's `## Context` heading is at `ADR-0022:23` and its `## Decision` block is
`ADR-0022:31-39`. The Decision opens *"Do not ship a speculative external IdP
seam."* (`ADR-0022:33`), confines `console-identity-application` to *"only local
org/account administration commands, read models, and audit builders"*
(`ADR-0022:36`), and forbids HR/roster/attendance/payroll integrations from
authenticating users, asserting sessions, granting roles, or deciding account status
(`ADR-0022:38`).

**That is a decision against an external identity-provider seam. It is narrower than
"no platform identity."** Verified: the string `org-scoped` appears nowhere in
ADR-0022 (grep over the file returns 0), and `ADR-0022:25` — cited in an earlier plan
revision as deciding tenancy — is Context prose, not Decision. A platform-tier
`party` row is still *local* identity, so ADR-0022 obstructs it less than the
withdrawn plan record G1 assumed. The ecosystem plan now records this itself and
withdraws G1 (`docs/ideas/ecosystem-plan-DRAFT.md:2172`); the same finding was reached
independently at `docs/ideas/ecosystem-plan-architect-findings.md:77`. **The scope
correction makes the handle cheaper, not necessary** — the reason for deferral is
below and is not an ADR-0022 prohibition.

ADR-0004 likewise contains no tenancy clause. The `(user_id, org_id)` pinning of
credentials is a migration-level mechanism
(`0034_enforce_org_id_rollout.sql:143-144`), not an accepted decision.

### The account-chooser case the product thesis asks for already ships

One physical passkey already serves a person employed by two tenants:

- `0004_create_auth.sql:7` makes `credential_id` UNIQUE per **credential**, not per person.
- `backend/crates/platform/auth/src/webauthn.rs:338-342` loads `exclude_credentials`
  from that org-scoped user's own passkeys only, and `:349-353` passes the per-org
  `users.id` as the WebAuthn user handle — so one device enrolled against two handles
  yields two credential ids and nothing is rejected.
- Login is usernameless/discoverable with an empty `allowCredentials` list
  (`webauthn.rs:786-796`), resolved by `platform_resolve_credential_org`
  (`0038_platform_auth_and_runtime_role.sql:64-86`) whose `LIMIT 1` sits over a UNIQUE
  column (`0038:79-80`), so the resolution is deterministic rather than arbitrary.

**The passkey choice is the org choice.** What a platform-tier identity row adds
beyond this is group-level *"same person"* resolution across legal entities — which is
a different requirement from account selection, and one with no counted consumer today.

### The measured half: the visibility mechanism costs nothing

Experiment X4 (`docs/ideas/experiment-x4.md`, executed 2026-07-29 on `postgres:18.4`,
re-runnable via `docs/ideas/experiments/x4/run.sh`) tested the plan's central claim
that tenant visibility is mediated by an **edge** rather than by scoping the party row.
**Verdict CONFIRMED, 30 assertions PASS / 0 FAIL / 3 controls observed RED**
(`experiment-x4.md:7-12`, `:246-256`):

- **Zero new GUCs.** Only `app.current_org` was referenced, in both variants
  (`experiment-x4.md:187-193`).
- **Zero changes to the 141 RLS policies.** Independently re-verified here:
  `grep -c 'ENABLE ROW LEVEL SECURITY'` across
  `backend/crates/platform/db/migrations/*.sql` sums to **141**.
- The confidentiality assertion held across `COUNT`, `EXISTS`, `DISTINCT`, aggregate,
  `UPDATE`, and a `23505` collision probe, with the hidden row proven physically
  present.

So the *mechanism* is measured even though the handle is deferred. Two limits of that
evidence are load-bearing for the constraints below:

1. **X4 built Variant A and Variant B, never the sentinel-homed variant.** Both X4
   variants gave `party` no `org_id` at all. Variant A (grant, no RLS) needs no definer
   but discloses platform-wide party cardinality to any tenant
   (`experiment-x4.md:207-214`), which collides with `DN-0003:85-86` — *"Denied data is
   omitted, including counts and relationship existence."* Variant B (no grant) makes a
   `SECURITY DEFINER` mandatory (`experiment-x4.md:195-241`). Constraint 4 below picks a
   third shape that needs neither, and that shape is **unmeasured**. Experiment X4b
   (`adr-adjudication.md:1320-1345`, schema-only, no build) is its gate.
2. **X4 CONTROL 3 is a security finding, not a style note.** A UNIQUE key that omits
   `org_id` from the front leaked relationship existence through error `23505` past a
   correctly armed FORCE policy (`experiment-x4.md:99-112`, `:272-274`), because a
   unique index is enforced physically below RLS.

### A shipped prohibition that nothing holds

`0075_employee_identity_resolution.sql:16-17` is, verbatim,
`identity_name_only_merge BOOLEAN NOT NULL DEFAULT FALSE CHECK (identity_name_only_merge = FALSE)`
— a database-enforced ban on name-inferred identity merge. **It is unprotected.**
`employees` is absent from `built_in_audited_tables()`
(`backend/ci/gates/migration-safety/src/lib.rs:163-172`; `users` is present at `:168`),
and no `-- console-gate: audited-table employees` marker exists anywhere in the
migration set (verified: zero matches, against 144 distinct markers that do exist).
The `DropAuditedColumn` violation at `migration-safety/src/lib.rs:300-309` therefore
does not fire for `employees`, so a future migration can drop that CHECK with a green
gate.

## Decision

1. **No platform-tier identity row and no `party` table is created in Slice 0.** The
   durable handle is deferred until it has a consumer that is not itself `HOLD`.
   `users.party_id` and `employees.party_id` are not added yet.
2. **Identity linkage across tenants is human-asserted, and is permitted local
   account administration within `ADR-0022:36`.** It may be established ONLY by a
   user-verified WebAuthn assertion of a credential the person already holds: the
   shipped step-up path is the shape — the credential owner is re-checked against the
   asserted user handle before verification (`webauthn.rs:455-466`) and a mere
   user-presence touch is rejected (`webauthn.rs:480-484`). Any linkage read must be
   resolved through a narrow `SECURITY DEFINER` in the exact shape of
   `platform_resolve_credential_org` (`0038:64-86` — `SET search_path`,
   `row_security` off→on, `REVOKE ALL … FROM PUBLIC`, `EXECUTE` to `console_rt` only)
   returning the linkage handle and nothing else.
3. **No roster, import, attendance, payroll, matching, or confidence-scored path may
   create, match, or merge an identity link.** `0075:16-17` is the database expression
   of this rule and may not be dropped or relaxed. This is the prohibition
   `ADR-0022:38` does not cover: its four forbidden verbs are *authenticate*, *assert
   sessions*, *grant roles*, *decide account status* — none of them is *link two
   accounts to one identity*, which is exactly how imported roster data would later
   confer group authority.
4. **Cross-group deduplication is not guaranteed.** A person imported into `employees`
   at a second employer who never enrolls a credential is never auto-linked. For a
   vendor-operated group-company platform this is a scope decision, not an oversight.

### Non-foreclosure constraints — what the entity model must not make impossible

The deferral is only safe if the handle stays cheap to add. These are the constraints
that keep it so, and they bind while the handle is deferred:

1. **No cross-tenant identifier may be declared a `FOREIGN KEY`, nor appear in any
   `UNIQUE` constraint or index whose key does not lead with `org_id`.** Both are
   enforced physically below RLS; X4 CONTROL 3 measured the `23505` disclosure
   (`experiment-x4.md:99-112`). When the visibility edge lands, the reason must be a
   comment in the migration's own text, or the next person to tidy the key order
   reopens a confidentiality hole with a green suite (`experiment-x4.md:272-274`).
2. **The authorization path may not read `employees`.** True today: the sole
   occurrence of the word in `backend/crates/platform/authz/src/` is a doc comment at
   `authz/src/lib.rs:239`, not a query (verified). This must be frozen by a CI
   assertion rather than left as an accident.
3. **No speculative handle column may be added to a built-in audited table.** `users`
   is audited (`migration-safety/src/lib.rs:168`) and `ALTER TABLE users DROP COLUMN`
   is a gate violation (`:300-309`), so a column added today is permanent. Adding it
   later is purely additive and blocked by nothing: `0076_user_employee_link.sql:12-13`
   already added a nullable link column to `users` after the fact, and
   `0060_create_groups.sql:43` already proves a cross-tenant reference to `users(id)`
   needs no key surgery.
4. **When the handle lands it must be an ordinary tenant-scoped row homed at the
   existing platform sentinel organization** `00000000-0000-0000-0000-00000000face`
   (`0036_platform_onboarding.sql:222-227`; the rationale that it is not a tenant is at
   `:217-221`; `0196_platform_force_command_and_fk_closure.sql:171-173` refuses it as a
   removal target), carrying `org_id NOT NULL` plus `ENABLE`/`FORCE ROW LEVEL SECURITY`
   plus the standard `org_isolation` policy on `app.current_org`. **Not** a Tier O
   carve-out: no `global_table_allowlist` or `owner_only_table_allowlist` entry, no new
   GUC, no change to the 141 policies, no new `SECURITY DEFINER` for the read.
5. **Every party-level mutation must be stamped with a real `org_id`,** written through
   `with_audits(OrgId::platform())` (`backend/crates/platform/db/src/audit_tx.rs:111`,
   which arms the GUC at `:121`) and never with the `org_id = NULL` form. The audit
   policy's `WITH CHECK` admits a NULL org (`0035_enable_rls_rollout.sql:109-112`) but
   its `USING` clause cannot return that row (`0035:108`), so a NULL-org receipt is
   written and then unreadable.
6. **No mechanism that guesses may be introduced in the interim.** Constraint 1 of this
   list plus Decision 3 together mean the interim state degrades to "no link", never to
   "a probable link".

### The open question acceptance must answer

**No mechanism has been specified by which two orgs arrive at the same `party` row.**
This is the unresolved half of the design and it is not resolved by this record. It
was reached independently at `ecosystem-plan-architect-findings.md:42` and `:77`, and
it is load-bearing: `party` is defined as *"`(id, org_id, party_kind, status,
created_at)` and nothing else"* with *"No identifying attribute"*
(`ecosystem-plan-DRAFT.md:1666-1668`) and *"never put personal attributes on `party`"*
as a permanent recommendation (`:1718`), which closes every attribute-based route by
construction. Without a resolution mechanism, org B onboarding the same human mints a
second `party`, reproducing `users`/`employees` one tier up, and the confidentiality
property becomes vacuous because there is nothing left to be confidential about.

The candidate resolutions, and what each still needs:

- **(a) Mint per passkey credential; the human self-links at second-org onboarding.**
  Admissible, and permitted by ADR-0022 as read above, because it is local identity
  rather than federation. Not yet chosen because it needs an endpoint no record
  designs, and it would put an org-selection step on the login path *before* the GUC is
  armed — surgery on the one path that must never break.
- **(b) Resolution is a platform-principal operation with an audit record, never a
  tenant capability.** Picked on the record by the plan
  (`ecosystem-plan-DRAFT.md:806-813`). Under constraint 4 it is enforced by DDL rather
  than by a handler's authorization check: the `org_isolation` `WITH CHECK` plus the
  sentinel-pinning column CHECK make a tenant-armed `INSERT` into `party` impossible, so
  "platform-principal only" holds even if a later handler forgets to assert it. It still
  needs the operation, its audit shape, and its authority named.
- **(c) A tenant-side matching or confidence-scored service.** Rejected, on shipped
  evidence rather than argument: `employees` already carries
  `identity_resolution_strategy` and `identity_resolution_confidence` (`0075:6`,
  `0075:13`) — a confidence model is what you build when matching is a guess — and the
  `0076` backfill promotes a row only where `HAVING count(*) = 1` holds (`0076:44`),
  behind a *partial* unique index (`0076:22-24`), leaving every duplicate unlinked. It
  would also let one tenant probe another's roster.

**The acceptance decision took the second of the two admissible branches: it is recorded
explicitly here that the duplication `party` exists to remove is not removed.** Neither
(a) nor (b) is chosen, and no endpoint is named, because Decision 1 creates no `party`
row in Slice 0 and there is therefore nothing yet for a resolution mechanism to resolve.
Cross-group deduplication is not guaranteed (Decision 4), and no guarantee of one durable
identity per person is asserted anywhere in this record's scope — which is what keeps an
unachievable guarantee out of an authoritative scope (README:7). Follow-up 4 keeps the
mechanism a precondition on the migration that creates `party`, not on this record.

### No compliance conclusion is asserted

Whether an orphan pseudonymous identifier of a natural person is itself regulated data
is a question this record does not answer and cannot answer. Six Korea controls in
`docs/program/console-jurisdiction-register.json` carry `release_disposition: HOLD`,
and `:1186` is verbatim *"Missing, stale, conflicting, or unqualified authority is
HOLD; agents may not invent certainty."* Deferring the durable row is the disposition
compatible with every one of those six staying `HOLD`; nothing here proposes unholding
any of them.

## Alternatives considered

### Create the `party` table and `users.party_id` in Slice 0

Rejected on irreversibility, not on merit. `users` is audited, so the column is
permanent from the day it lands (`migration-safety/src/lib.rs:168`, `:300-309`), while
adding it later is additive and blocked by nothing. Its only distinctive Slice-0
consumer is a definer-mediated grant read whose grant rows carry no `org_id` predicate,
CONFIRMED by the architect against `0155_create_ontology_instances.sql:18,39`
(`ecosystem-plan-architect-findings.md:28-31`, `:70-72`); building the handle first ships
the column that makes that leak reachable.

### Widen ADR-0022 to authorize a platform identity tier

Rejected. ADR-0022 needs narrowing, not widening. Its scope correction is a finding
about what ADR-0022 never decided, and the right response to "the wall is not there" is
to state the scope accurately and add the missing prohibition — not to grant a
capability nothing consumes yet.

### Draft the plan's §5.11 G1 as a scoped record

Rejected; the plan itself has since withdrawn G1 (`ecosystem-plan-DRAFT.md:2172`). Its
premise was false, so it had no clause to amend, and its claim of *one durable identity
per natural or legal person, across every tenant and vertical* is undeliverable without
the matching the plan itself rejects.

### A second tenancy dimension (`app.current_group`)

Rejected, and invalidated by the requirement it exists to serve: a person can work for
companies in **different** groups, so group-scoping relocates the duplication rather
than removing it and cannot represent a person before they are grouped
(`ecosystem-plan-DRAFT.md:474-491`). It would additionally cost a second GUC bridged
into 141 RLS policies and a gate classification that does not exist — maximum cost for
a design that does not meet the requirement.

### `party` as a Tier G global-read table

Rejected. Tier G means `console_rt` may `SELECT` with no filter, so any tenant
enumerates every party on the platform, contradicting the confidentiality requirement
directly. Every existing Tier G rationale is literally *"no tenant data"*
(`backend/ci/gates/tenant-isolation/src/lib.rs:48-70`).

## Why this shape was chosen

Because it is the only option on the list that is **reversible in the direction that
matters**. Deferring costs a later additive migration, which two shipped precedents
show is cheap (`0076:12-13`, `0060:43`). Shipping the handle early costs a permanent
column on an audited table plus a reachable group-scope read, and the resolution
mechanism that would justify it does not exist yet. The measured evidence points the
same way: X4 confirmed the expensive part — the visibility mechanism — is free, so
nothing is learned by building the handle now that cannot be learned by X4b without a
build.

## Consequences

**In favour**

- Zero migration slots consumed, so the `0207+` serial version space stays free for
  the lanes that need it (`migration-safety/src/lib.rs:131-141` enforces gap-free
  contiguity, which serializes parallel lanes).
- Zero new GUCs, zero changes to the 141 RLS policies, zero new gate classifications,
  no `SECURITY DEFINER` added. ADR-0002, ADR-0003, ADR-0004, ADR-0021 and DN-0003 take
  no delta.
- Matching-based deduplication is foreclosed permanently rather than temporarily; it
  stays an operator action with a receipt.
- The scope of ADR-0022 stops being cited for a prohibition it never contained, which
  removes the false premise from every downstream record that inherited it.

**Against**

- Cross-legal-entity identity continuity stays unrepresentable.
  `docs/specs/korean-legal-boundaries.md:40-43` remains specified-and-unbuilt, so
  전적/전출 between 계열사 and concurrent 겸직 cannot be recorded as one person, and
  group-level headcount or authority rollups spanning subsidiaries are not expressible.
- `org_id` on an existing row is immutable
  (`0031_runtime_role_and_immutable_org.sql:94`), so the sequential-transfer case has no
  `UPDATE` path at all; the only sanctioned re-home is the DELETE+re-INSERT the platform
  bootstrap performs at `0036:204-210`.
- The party is foreclosed as an authentication subject: credentials stay pinned to
  `(user_id, org_id)` (`0034:143-144`), so switching companies remains a credential
  choice at the OS prompt rather than an in-app org switcher after one sign-in.
- **Residual risk accepted: the sentinel-home shape of constraint 4 is unmeasured.** X4
  built Variant A and Variant B and never this one. Confidence is therefore *medium*,
  and rises to high only on a passing X4b.
- The open question above is carried, not closed. If acceptance cannot answer it, the
  handle should stay deferred rather than land without a resolution rule.

## Follow-ups

1. **Run experiment X4b** (`adr-adjudication.md:1320-1345`) — schema-only, extends
   `docs/ideas/experiments/x4/probe.sql`, no build, no migration slot. It gates
   constraint 4. Its two known-bad controls must be observed RED first.
2. **Freeze constraint 2 with a CI assertion** that the authorization crates contain no
   `employees` query, so `authz/src/lib.rs:239` staying a doc comment is enforced rather
   than observed.
3. **Protect `0075:16-17`.** Either add `-- console-gate: audited-table employees` to a
   migration so `DropAuditedColumn` covers it, or record explicitly that the
   name-only-merge ban is droppable. Today it is unprotected by omission, which is the
   worst of the three states.
4. **Name the resolution mechanism** — candidate (a) or (b) — with its endpoint,
   authority, and audit shape, before any migration creates `party`.
5. **Carry constraint 1 into the migration text** that creates the visibility edge, as
   a comment stating why `org_id` leads the unique key.

## Reciprocal record landed on acceptance

`docs/decisions/README.md:9` requires amendment to be explicit in **both** records, and
`README.md:26` requires relationship keys to be reciprocal where applicable. All three
edits below landed in the same commit that flipped this record's status to `accepted`:

1. **Frontmatter key on ADR-0022.** `docs/decisions/ADR-0022-local-identity-no-external-idp.md`
   gained `amended_by: [ADR-0027]`, and gained `ADR-0027` in its `related` list.
   **ADR-0022 carried no `amended_by` key before this change** — its relationship keys
   were `amends: [ADR-0010]` (`:8`), `supersedes: [ADR-0017]` (`:9`), and
   `related: [ADR-0004, ADR-0010, ADR-0017]` (`:10`). **This created the key; it did
   not append to an existing one.** Without it, the reciprocity check at
   `scripts/check-adrs.mjs:399-409` fails the build, and the `amended_by` target must be
   `accepted` by `:411-420`.
2. **README index row.** The ADR-0022 row changes from `accepted` to
   `accepted, amended`, and its scope cell gains the amendment clause — matching the
   house pattern already used for ADR-0010 and ADR-0023:

   > `| [ADR-0022](ADR-0022-local-identity-no-external-idp.md) | accepted, amended | Local passkey identity; no speculative external IdP seam; cross-tenant identity linkage narrowed to a human assertion by ADR-0027 |`

   The ADR-0027 row's own status cell changed from `proposed` to `accepted` in the same
   edit; `scripts/check-adrs.mjs:461-464` fails the build if the index status and the
   frontmatter status disagree.
3. **One sentence edited in place in ADR-0022's Decision.** A reciprocal key alone would
   leave a false sentence standing in an authoritative record. `ADR-0022:38` previously
   enumerated what an HR/roster/attendance/payroll integration must not do —
   *"They must not authenticate users, assert sessions, grant roles, or decide account
   status unless a separate identity-federation ADR names the real IdP/protocol/claims
   and passes security review."* That enumeration was **incomplete as of this record's
   acceptance**, because linking two accounts to one identity was not on the list and is
   the route by which imported roster data would confer group authority. The edit
   inserted the missing verb into that sentence:

   > They must not authenticate users, assert sessions, grant roles, **link two
   > accounts to one identity,** or decide account status unless a separate
   > identity-federation ADR names the real IdP/protocol/claims and passes security
   > review.

   No other sentence in ADR-0022 became false. `ADR-0022:33` and `:36` remain exactly
   correct — the scope finding in this record's Context is about text ADR-0022 never
   contained, not about text it got wrong, so `:36` gained a clarifying cross-reference
   to ADR-0027 rather than a correction.
