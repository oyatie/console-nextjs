---
id: ADR-0038
status: proposed
doc_status: review
date: 2026-07-31
owner: jasonlee
decision: location-erasure-unlogged-then-crypto-shred
related: [ADR-0005, ADR-0014, ADR-0015, ADR-0029, ADR-0037]
---

# ADR-0038: Erasure for 개인위치정보 — minimise, exclude from the WAL, then crypto-shred what is left

## Status

**Proposed 2026-07-31.** ADR-0037 states the erasure-versus-PITR conflict and deliberately decides
nothing. This record decides the *mechanism* for one data class — 개인위치정보 — and only that class.
It asserts no Korean legal conclusion; it states what instruments say and routes interpretation to
counsel, as `docs/program/console-jurisdiction-register.json` `Missing, stale, conflicting, or unqualified authority is HOLD; agents may not invent certainty.`
requires.

It carries no `amends` key: `scripts/check-adrs.mjs` `proposed ADR cannot declare` forbids one on a
proposed record. Where it narrows ADR-0014 in practice, that is noted rather than enacted.

## Context

### Why location data is not the general PII case

ADR-0037 reasons from PIPA, where 법 제21조제1항 and 제36조제2항 both say **지체 없이** — without
delay. Personal location data is governed by a different instrument with a different word.

`위치정보의 보호 및 이용 등에 관한 법률` 제23조제1항 (법률 제21066호, 시행 2025-10-01) requires
개인위치정보 to be destroyed **즉시** once its purpose is achieved. The surrounding provisions close
the routes a bounded window would need:

- **제23조제2항** — destruction must include 복구 또는 재생을 **방지**하기 위한 조치.
- **시행령 제26조의2제1항** (대통령령 제36084호) 준용s 제8조의2: deletion by a
  `재생할 수 없는 기술적 방법`, or physical destruction.
- **시행령 제26조의2제2항** — the only `대통령령으로 정하는 정당한 사유` is
  `개인위치정보주체가 자신의 개인위치정보의 보유에 관하여 별도로 동의한 경우`. Operational necessity is
  not named. Disaster recovery is not named.
- **시행령 제26조의2제3항** — even with that consent, retention is capped at **최대 1년**.
- **제40조의2** — non-destruction is criminal: 2년 이하의 징역 또는 2천만원 이하의 벌금.
- **제23조제3항** — the regulator may have officials inspect the **파기실태**, the destruction
  practice, rather than the policy describing it.

Every text above was retrieved from 국가법령정보센터 on 2026-07-31 via
`scripts/korean-legal/fetch-statutory-source.mjs --article`, not recalled.

### Three facts about this system, verified

**1. The retention routine does not run.** `0005_create_compliance_location_store.sql` defines
`purge_expired_location_data`, and `backend/crates/compliance/adapter-postgres/src/lib.rs:392` wraps
it. Repo-wide there is no worker, no cron, no apalis job and no route that calls the wrapper. The
only other references are one integration test and the ADR-0029 audit-coverage exclusion list.
**Nothing bounds the growth of `location_pings` in the live database.** ADR-0037 presented this
routine as a shipped retention mechanism and has been corrected.

**2. The archive window is finite, and recently so.** `deploy/apps/console/base/database.yaml`
`retentionPolicy: "35d"`. Until 2026-07-31 it was absent and PITR reached back to the first base
backup forever. Analyses calibrated on an unbounded tail are calibrated on a state that ended —
including any design whose mitigation is "wait for the archive to age out", which now terminates.

**3. `location_pings` has no inbound foreign key, and that is not the same as being free to move.**
`grep -rn "REFERENCES location_pings"` returns nothing. But *outbound* it holds
`user_id`/`branch_id` `ON DELETE RESTRICT` (0005) and `org_id` (`0034_enforce_org_id_rollout.sql`),
it is enrolled in FORCE RLS (`0035_enable_rls_rollout.sql`), and **five org-removal procedures each
run `DELETE FROM location_pings WHERE org_id = p_id`** — `0059`, `0081`, `0090`, `0193`, `0196`.
Moving the table to a second database silently converts all five into no-ops with no foreign key
left to restrict on. An earlier reading of this record's own author asserted the inbound fact and
drew the outbound conclusion; that inference is retracted here.

## Decision

Two phases. **Phase 1 is unconditionally correct and independent of Phase 2.**

### Phase 1 — minimise, then exclude from the write-ahead log

1. **Declare `location_pings` `UNLOGGED`.** Unlogged relations are not written to the WAL, are
   excluded from base backups, and are not replicated to standbys — so coordinates never enter the
   archive at all. This is the segregation boundary, placed at `relpersistence` rather than at a
   database cluster, which is what keeps the foreign keys, the RLS enrolment, the five org-removal
   deletes and the single-transaction atomicity intact.
2. **Run `purge_expired_location_data`, at a retention no longer than the longest read window.**
   A routine that has never run is not a control. This is now the mechanism that bounds how much
   개인위치정보 exists at any moment, and it is doing the work that step 1 of an earlier draft of this
   record tried to do by collapsing the table — see the retraction below.
3. **Validate `recorded_at` at the domain boundary** so a future-dated ping cannot outlive the
   purge horizon.

> **Retracted before proposal: "collapse to one row per subject" was wrong, and this record's own
> falsification test is what caught it.**
>
> An earlier draft made step 1 *collapse `location_pings` to `UNIQUE (user_id)`*, on the reasoning
> that the product needs last-known-position rather than a movement history. The *Cost and
> falsification* section then said that reverts if any read path needs more than the latest row.
> Running that check: `backend/crates/dispatch/adapter-postgres/src/lib.rs` reads the table in four
> places. Two are `ORDER BY recorded_at DESC LIMIT 1` and would have survived. **Two are not**, at
> `:1501` and `:1570`:
>
> ```sql
> EXISTS (SELECT 1 FROM location_pings lp
>          WHERE lp.user_id = u.id AND lp.branch_id = ub.branch_id
>            AND lp.on_duty AND lp.recorded_at >= $3)
> ```
>
> That is an existence check over a **window**, and it decides dispatch eligibility. Under one row
> per subject, a mechanic who pinged on-duty at branch A an hour ago and off-duty at branch B five
> minutes ago is eligible today and would silently stop being. Data minimisation by collapsing the
> table is therefore unavailable without changing who receives work.
>
> Minimisation survives as **retention**, not as shape: a running purge bounds the history to the
> window the reads actually need. That is a weaker form of the same idea and it is the one this
> system can have.

### Phase 2 — envelope-encrypt per subject, gated

Encrypt the coordinate payload in Rust before the `INSERT`, under a per-subject key held in
OpenBao, so that copies `relpersistence` cannot reach are unreadable. Destruction on withdrawal
becomes a key delete ordered **before** the existing transaction. Gated on the two measurements in
*Cost and falsification*.

## Why both, when one boundary would do

The redundancy objection deserves an answer, not "defence in depth". The test is: **name a copy each
mechanism reaches that the other does not.** If neither can, one is decorative.

| Copy of the coordinate | `UNLOGGED` reaches it | Key destruction reaches it |
| --- | --- | --- |
| WAL segment in the object-storage archive | **yes** — never written | yes |
| Base backup | **yes** — never copied | yes |
| Streaming standby | **yes** — not replicated | yes |
| Superseded heap tuple, before autovacuum | no | **yes** |
| `pg_dump` / `COPY TO` / analytics export | no | **yes** |
| A future `ALTER TABLE … SET LOGGED` | no — retroactively republishes | **yes** — republishes ciphertext |
| Rows written before the cipher shipped | yes, from cutover | **no** |

Two non-empty exclusive columns, so the mechanisms are not redundant. The last row is the one that
must not be talked around: NIST SP 800-88 Rev. 2 §3.2.2 conditions Cryptographic Erase on no
sensitive data having previously been stored in plaintext, because CE can only sanitise keys related
to encrypted data. **Crypto-shredding cannot reach the plaintext already written.** That is why
Phase 1 ships first and stands alone.

*(Rev. 1 was withdrawn on 2025-09-26. Most published crypto-shredding guidance still cites it; an
auditor checking the citation finds a superseded document.)*

**What the redundancy objection does defeat is segregation-by-cluster.** A second PostgreSQL
instance buys nothing the `UNLOGGED` column does not already buy, and costs the three outbound
RESTRICT foreign keys, the FORCE-RLS enrolment, the five org-removal deletes, and the
single-transaction atomicity that consent withdrawal depends on. `relpersistence` delivers the
identical archive exclusion for one keyword and keeps all of them.

## The consent split

`location_consents` and `location_consent_ledger` are **evidence of lawful collection** and stay
logged, archived and retained. `location_pings` is the 개인위치정보 and is destroyed.

Destroying the consent record to satisfy a destruction duty would remove the proof that collection
was lawful. The split is the point: **retain the permission, destroy the observation.**

## Atomicity

Consent withdrawal today deletes collection logs and pings in one transaction
(`backend/crates/compliance/adapter-postgres/src/lib.rs:236,242`). Phase 1 preserves that exactly —
an `UNLOGGED` table participates in ordinary transactions, which is the decisive advantage over a
second cluster.

Phase 2 introduces one cross-store step. The key delete is ordered **before** the transaction, so
the failure mode is a row whose key is already gone: unreadable, and the subsequent delete is then
idempotent cleanup rather than the erasure itself. "Eventually consistent erasure" is not available
when the standard is 즉시.

## Anti-patterns this avoids

- **Backing up the key store, undoing the shred.** Phase 2's key store needs a DR posture whose
  correct failure mode is permanent loss. Named as a gate, not assumed.
- **Encrypting into an archive that already holds the plaintext.** Phase 1 removes the plaintext
  path before Phase 2 introduces a cipher.
- **Audit records containing the erased data.** Destruction evidence records subject id, time and
  method — never coordinates.
- **A control that does not run.** The `purge_expired_location_data` finding is exactly this, and
  the decision forces it to run or go.
- **Segregation that breaks referential integrity.** Avoided by not segregating at the cluster.

## What is not decided here

Questions for counsel, stated as questions:

1. Is this operator a 위치정보사업자 or 위치기반서비스사업자 under 제5조/제9조 — that is, does 위치정보법
   bind this deployment at all?
2. Does 제23조제1항's 즉시 admit an architecture in which the row is deleted immediately but a
   superseded heap tuple persists until autovacuum?
3. Is destroying an encryption key accepted as a `재생할 수 없는 기술적 방법` under Korean guidance?
   No published PIPC or 방송미디어통신위원회 guidance was found that says so explicitly.

## Cost and falsification

**Cost.** Phase 1 is one migration plus an ingest change. `UNLOGGED` means the table is **truncated
on crash recovery and lost on failover** — acceptable for last-known-position telemetry, and it must
be stated to the product owner as an availability trade rather than discovered during an incident.

**What would falsify this.** If `location_pings` history turns out to be a product requirement
rather than an artefact — for example if attendance or dispatch reads more than the latest row —
then Phase 1's collapse is wrong and the design reverts to a retained-but-unlogged history with a
running purge. That is checkable in the read paths and should be checked before the migration is
written.
