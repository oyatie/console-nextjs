---
id: ADR-0037
status: proposed
doc_status: review
date: 2026-07-31
owner: jasonlee
decision: erasure-versus-pitr-conflict
related: [ADR-0002, ADR-0005, ADR-0014, ADR-0015, ADR-0024, ADR-0029]
---

# ADR-0037: Erasure versus point-in-time recovery — the conflict, the options, and why this record does not decide it

## Status

**Proposed 2026-07-31.** This record decides nothing. It states a conflict between two
things the repository already ships, prices four ways out of it, and routes the choice
to an authority that has not yet been engaged. It amends and supersedes no record, and
it carries no `amends` key — `scripts/check-adrs.mjs` `proposed ADR cannot declare`
forbids one on a proposed record, and the prohibition is correct here: a record that
poses a question has no authority to narrow an accepted one.

It asserts no Korean legal conclusion. It does not state what any statute requires, what
a retention period is, or whether any option below is sufficient for anything. Those are
findings for qualified counsel, and
`docs/program/console-jurisdiction-register.json` `Missing, stale, conflicting, or unqualified authority is HOLD; agents may not invent certainty.`
binds this record as it binds every other.

## Context

`docs/ideas/korean-legal-sources.md` `Name the erasure-versus-PITR conflict in a record`
recorded this as an open follow-up. That document states the conflict; this one prices
it. Neither restates the other.

### The two shipped facts that collide

**ADR-0015 requires a restore to an arbitrary past timestamp.** Its Decision reads
`ADR-0015` `Postgres continuous WAL archiving with point-in-time recovery remains mandatory`,
and the proof obligation is
`ADR-0015` `targets RPO ≤5min and RTO ≤1h must be proven by restore to an arbitrary timestamp`.
The word doing the work is *arbitrary*. A recovery target that may be any past instant is
a recovery target that can land immediately before any deletion.

**A destruction path already ships, and it is already the reason a table exists.**
ADR-0014 carved a destructible store out of the append-only audit store precisely so that
data could be physically removed:
`ADR-0014` `Destruction-on-withdrawal is physically realizable (drop partitions)`.
The mechanism is real, not planned. Consent withdrawal runs three deletions inside one
transaction, gated on `backend/crates/compliance/adapter-postgres/src/lib.rs:destroy_location_data`
— among them
`backend/crates/compliance/adapter-postgres/src/lib.rs` `DELETE FROM location_pings WHERE user_id = $1`
— and a separate retention routine,
`0005_create_compliance_location_store.sql` `purge_expired_location_data`, which drops whole
day partitions and deletes collection logs on a cutoff.

> **Retracted 2026-07-31: `purge_expired_location_data` is not a running retention mechanism.**
> This record presented it as one, alongside the consent-withdrawal path, and that is wrong in a way
> that understates the exposure rather than overstating it. Repo-wide, the only references are the
> migration that defines it, the adapter wrapper at
> `backend/crates/compliance/adapter-postgres/src/lib.rs:392,416`, one integration test, and the
> ADR-0029 audit-coverage exclusion list. **There is no worker, no cron, no scheduled job and no
> route that calls it.** Nothing bounds the accumulation of `location_pings` in the live database,
> which is a separate and more immediate exposure than the archive question this record is about.
> Recorded here rather than fixed here: scheduling a routine that deletes personal data is a change
> with its own review.

**So the conflict is not prospective.** It is not waiting on personal-data infrastructure
that does not exist yet. Every `DELETE` and every `DROP TABLE` those two paths execute
today is executed against a cluster whose write-ahead log is being archived continuously,
and a restore to a timestamp before the deletion returns the deleted rows. Deleting a row
does not make it unreconstructable while the archive covering its lifetime is retained.

### The window is unbounded, and that was a decision

The sharpest fact in this record is a comment in a live manifest.
`deploy/apps/console/base/database.yaml` `never prunes base backups / WALs` — the
`ObjectStore` named `console-backups` declares no retention policy at all, and the comment
that records why is dated and attributed to the owner. The same manifest offers the
remedy it did not take:
`deploy/apps/console/base/database.yaml` `re-add e.g. retentionPolicy: "90d"`.
`deploy/OPS-RUNBOOK.md` documents the same posture in prose. Nothing else in `deploy/`
sets a retention policy, and no overlay patches one in.

The reconstruction window is therefore not long. It is **open-ended by design**, reaching
back to the first base backup and never closing. Any framing of this conflict as "data
survives for N days" is wrong about this deployment.

> **Superseded 2026-07-31 by the change this record argued for.** The manifest now carries
> `deploy/apps/console/base/database.yaml` `retentionPolicy: "35d"`. The window is finite, and the
> paragraph above — written when it was not — is left in place because the reasoning that follows
> it depends on the distinction, but it no longer describes the deployment.
>
> This does not resolve the conflict, and the change that set it said so: 35 days of
> reconstruction is still 35 days. What it does change is that designs which *wait for the archive
> to age out* now terminate, where under indefinite retention they never would. Any analysis
> calibrated on "unbounded tail" is calibrated on a state that ended.

### What does not exist, verified rather than assumed

There is no personal-data, retention-policy, erasure, or data-subject-request table. The
search was over every migration:

```
$ ls backend/crates/platform/db/migrations/ | grep -Ei 'pii|personal_data|retention_polic|erasure|dsr|subject_request|data_subject|anonymization|redaction|purge'
(no output)

$ grep -rhoiE 'CREATE TABLE (IF NOT EXISTS )?[a-z0-9_."]+' backend/crates/platform/db/migrations/ \
    | sed -E 's/CREATE TABLE (IF NOT EXISTS )?//I' | tr -d '"' | sort -u \
    | grep -Ei 'pii|personal_data|retention_polic|erasure|dsr|subject_request|data_subject|anonymization|redaction|purge'
(no output)
```

282 distinct table names across 206 migrations, highest `0206_ont_policy_api_attach_command_role.sql`;
none matches. The only consent tables are `location_consents` and `location_consent_ledger`,
both created by `0005_create_compliance_location_store.sql`, and both are location-tracking
consent. The `purge` term does appear in the tree — as the routine named above, not as a
table — so the absence is of a *schema* for erasure, not of a destruction *mechanism*.

**The absence is what makes this cheap now.** An encryption boundary chosen before rows
exist costs a column type. Chosen after, it cannot reach backwards: rows already written
as plaintext are already plaintext in the archive, and re-encrypting them in place writes
a new version while leaving every prior version recoverable. Whatever this conflict is
resolved with, the resolution is strictly cheaper before personal data lands than after.

### The review already planned does not cover this

Payroll is blocked behind a professional sign-off:
`docs/specs/payroll.md` `Production payroll remains blocked until a licensed 노무사/세무사 validates the worked examples and signs the release gate`.
A 노무사 is a **labour** professional and a 세무사 is a **tax** professional. That review is
scoped to wage computation, statutory deductions and filing — and it is the only external
professional review this program has planned.

**It is not privacy counsel and cannot be read as covering this question.** Booking it does
not advance this record one step. This is easy to miss precisely because the payroll gate
is the most visible "we are waiting on a professional" hold in the repository, and it is
tempting to treat one pending expert as coverage for another domain. It is not.

## Decision

Nothing here is decided. The record fixes four things so that the open question stays open
honestly rather than being closed by accident.

1. **The conflict is recorded as a live architectural condition, not a future risk.** Two
   accepted records — ADR-0014's destructible store and ADR-0015's arbitrary-timestamp
   restore — are simultaneously in force and pull in opposite directions on the same rows.
   Under README authority rule 6 that is a governance gap to reconcile through a decision,
   not a defect in either record.

2. **No option below is adopted, and none may be treated as chosen** because it appears
   here. The costs are stated so counsel and the owner can choose against a real price.

3. **A `DELETE` may not be described as destruction without naming the archive.** Any
   design note, ADR, test name, code comment or product statement that claims data is
   destroyed must say whether it means the row is gone from the live cluster or gone from
   everything that can reconstruct the live cluster. Those are different claims and only
   the first is currently true of anything in this repository. This constraint is
   architectural, not legal, and it binds prose from the moment this record is read —
   there is no gate that enforces it.

4. **The question is routed to privacy counsel** as its own engagement, distinct from the
   labour/tax sign-off described above. Until such an authority speaks with a
   candidate-bound receipt, no option here is sufficient for anything and this record must
   not be cited as though one were.

## Decision drivers

- **Timing dominates cost.** The encryption boundary is nearly free before rows exist and
  unreachable-backwards after, and no personal-data table exists today. The window in
  which this is a cheap decision is open now and closes on the first migration that stores
  personal data.
- **The reconstruction window does not expire on its own.** With no retention policy the
  conflict has no natural end date, so "wait and it ages out" is not an available answer.
- **Two of the four options change what ADR-0015 proves**, and one does not. That
  difference is the main axis of the choice, so each option below states it explicitly.
- **Nothing here needs a legal conclusion to be worth writing.** That a restore returns
  deleted rows is a property of write-ahead logs, checkable without any statute.

## Options and what each costs

### A. Crypto-shredding — encrypt per subject, destroy the key

Personal data is written encrypted under a per-subject key held outside the database;
erasure destroys the key rather than the rows.

**Effect on ADR-0015's proof: none.** This is the only option here that leaves
`restore to an arbitrary timestamp` intact, because it never touches the archive. A
restore to any past instant still succeeds and still meets RPO/RTO; what changes is that
the restored cluster yields ciphertext for shredded subjects. The DR contract and the
erasure path stop competing for the same mechanism.

**Costs, and they are real.**
- The key store becomes the thing that must *not* be recoverable, which inverts every
  instinct ADR-0015 encodes. If the key store is itself backed up with a PITR window, the
  keys come back and the shred is undone — so it needs its own DR posture whose failure
  mode is permanent, unrecoverable data loss by design. That posture has to be designed,
  not assumed.
- Key loss is indistinguishable from key destruction after the fact. An operational
  mistake and a lawful erasure look identical, so the record of *why* a key is gone
  becomes the evidence, and that record has to live somewhere it cannot itself be erased.
- Every read of personal data acquires a key fetch on the path.
- It is retrofit-hostile, per the timing argument above. Applied after plaintext rows
  exist, it protects new writes only and leaves history in the archive.

### B. Shorten the PITR window for personal-data tables

**This option is not expressible in the shipped posture, and that is a finding rather
than an opinion.** WAL is a physical log of the entire cluster; Barman retention is
configured per `ObjectStore`, and there is exactly one — `console-backups` — with one
archiver, `deploy/apps/console/base/database.yaml` `isWALArchiver: true`. There is no
mechanism to retain WAL covering one table and not another. The only reachable version of
this option is *shorten the window for everything*.

**Effect on ADR-0015's proof: it narrows it.** "Restore to an arbitrary timestamp" becomes
"restore to an arbitrary timestamp within N days". That is a smaller claim than the one
ADR-0015 makes, and shrinking it is an amendment to an accepted record, not a
configuration tweak.

> **Retracted 2026-07-31 — the paragraph above overstates what ADR-0015 says.** ADR-0015
> states no window length anywhere. Its sentence is
> `ADR-0015` `targets RPO ≤5min and RTO ≤1h must be proven by restore to an arbitrary timestamp`,
> and *arbitrary* there governs **which** instant inside the recoverable range may be
> demanded of a restore drill — it is the proof method for RPO/RTO, not an assertion that
> the range is unbounded. An unbounded archive is what the absence of a `retentionPolicy`
> produced; it is not something ADR-0015 requires.
>
> This matters because the two readings were both in circulation and could not both be
> right: the pull request that first proposed a finite window recorded that
> "ADR-0015 constrains recovery *speed* … and says nothing about window *length*", which is
> the opposite of the paragraph above. Reading it as an amendment would have made a
> one-line manifest default require a new accepted ADR — and `scripts/check-adrs.mjs`
> forbids a `proposed` record from declaring `amends`, so this record could not have
> carried it. A control that expensive gets routed around rather than obeyed.
>
> **Setting a finite window is therefore not an amendment and not the adoption of this
> option.** Option B is proposed here as an *answer to the erasure conflict*, and a finite
> window is not one: 35 days of reconstruction is still 35 days, and whether a bounded
> window means anything is exactly what this record still refuses to decide. Every option
> above, including A and C, wants a finite window underneath it for storage reasons alone.
> What changed in the manifest is a default that was never chosen; what stays open is the
> question.

**Costs.**
- **For personal location data this option is not merely weak, it is unavailable.**
  위치정보법 제23조제1항 requires 즉시 파기, and 시행령 제26조의2제2항 admits only the data
  subject's separate consent as an exception — so no value of N satisfies it. See
  *위치정보법 imposes 즉시* below. This is the one cost here that no number can buy off.
- It buys a bounded window, not erasure: data still reconstructs for N days after
  deletion. Whether a bounded window means anything is exactly the question this record
  refuses to answer.
- It trades against the reason the window was left open. The owner chose indefinite
  retention on a recorded date; reversing that is a decision with its own consequences for
  recovery from a slow-discovered corruption.
- Storage cost falls, which is the one unambiguous benefit and the reason the manifest
  comment already contemplates it.

### C. Segregate personal data into a separately-backed store

A second database or cluster holds personal data with its own, shorter or absent, archive.

**Effect on ADR-0015's proof: it splits it.** Two stores means two DR postures, two RPO/RTO
targets, and two restore drills — and a restore that lands the two stores at different
timestamps, which is a consistency problem ADR-0015 currently does not have.

**Costs, one of which is a direct collision with an accepted record.**
- **ADR-0002 requires the audit event in the same transaction** as the mutation. Its
  Decision states `ADR-0002` `is append-only: UPDATE/DELETE revoked and additionally blocked by trigger`
  for `audit_events`, written through `with_audit` in the mutating transaction. A personal-data
  write in a different database cannot share that transaction. This option therefore cannot
  be adopted without deciding what replaces same-transaction auditing at that seam.
- The append-only audit store is itself unerasable by construction, before PITR is even
  considered. ADR-0014 already handled this for coordinates by keeping them out of
  `audit_events`; segregation does not remove that obligation for anything else, and any
  personal data that reaches the audit store is beyond every option in this record.
- It multiplies the portability surface ADR-0024 governs: a second store needs a
  context-native adapter in every deployment context, not just the live one.
- Operational cost is the highest of the four.

### D. Accept the conflict with a documented compensating control

The window stays open; a procedure re-applies erasures after any restore.

**Effect on ADR-0015's proof: none.** The DR contract is untouched, which is why this is
the cheapest option to adopt and the most expensive to rely on.

**Costs.**
- The control is procedural and fires at the worst possible moment — during a disaster
  recovery, under time pressure, executed by a human. It has no technical enforcement and
  no gate could check it, which puts it in the weakest class of control this repository
  recognises.
- It requires a durable list of every erasure ever performed, so that the list can be
  replayed after a restore. That list is itself a record of who asked to be erased, which
  is a new store of exactly the kind of data this record is about.
- **Accepting a conflict is a decision only counsel can make.** Engineering can say the
  conflict is survivable technically; it cannot say the acceptance is available. Choosing
  D without that authority is the failure the `uncertainty_rule` names.

### The dimension this record first omitted: data we are obliged to KEEP

The first draft framed a two-force problem — a destruction duty against a recovery
capability. It missed a third force, and for an HR and payroll product that third force is
dominant: much of this data is data someone is **obliged to retain**. Raised by the owner;
recorded here rather than corrected silently.

Quoted verbatim from the official legislation portal, 개인정보 보호법 법률 제20897호, 시행
2025-10-02, retrieved 2026-07-31 from `https://www.law.go.kr/법령/개인정보보호법`:

> **제21조(개인정보의 파기)**
> ① 개인정보처리자는 보유기간의 경과, 개인정보의 처리 목적 달성, 가명정보의 처리 기간 경과 등 그
> 개인정보가 불필요하게 되었을 때에는 지체 없이 그 개인정보를 파기하여야 한다. **다만, 다른 법령에
> 따라 보존하여야 하는 경우에는 그러하지 아니하다.**
> ② 개인정보처리자가 제1항에 따라 개인정보를 파기할 때에는 **복구 또는 재생되지 아니하도록** 조치하여야 한다.
> ③ 개인정보처리자가 제1항 단서에 따라 개인정보를 파기하지 아니하고 보존하여야 하는 경우에는 해당
> 개인정보 또는 개인정보파일을 **다른 개인정보와 분리하여서 저장ㆍ관리**하여야 한다.
> ④ 개인정보의 파기방법 및 절차 등에 필요한 사항은 대통령령으로 정한다.

**This record states no conclusion about what that text requires of us.** It quotes the
instrument and observes that its vocabulary lands on architectural questions this record
already had open. Whether and how it applies is for qualified counsel, and every Korea
control remains `HOLD`.

Three observations, all architectural rather than legal:

**A blanket erase-on-request design would be wrong for this product.** The 제1항 단서 carves
out data retained under other statutes, and payroll is dense with such duties — the register
in `docs/ideas/payroll-statutory-sources.md` already tracks nine statutory items for
contribution and withholding purposes alone, without yet touching record-retention periods.
So the operative primitive is not "delete on request" but **retention-class-aware disposal**:
a data class, a duty that pins it, and a disposal action that becomes available only when the
pin lifts. This system has neither the classes nor the pins — no retention table exists in
any migration.

**The statute's disposal wording and our PITR posture use the same vocabulary.** 제2항 speaks
of 복구 또는 재생 — restoration or reproduction. That is what point-in-time recovery is for.
This record's central observation was already that a `DELETE` is not destruction while the
archive can reconstruct it; the instrument's own language describes that same distinction.
Noted as a convergence, not as a finding of compliance or its absence.

**Segregation appears in the text, which bears on option C.** 제3항 addresses data kept under
the 단서 and uses 분리하여서 저장ㆍ관리. Option C below was priced as one of four neutral
alternatives. It should be re-read knowing that separation is the shape the instrument
describes for retained data — which may make C serve two purposes at once rather than one,
and changes its cost/benefit relative to A and B. **This record does not adopt it on that
basis**; it flags that the option was priced without this input and should be re-priced with
it.

What this adds to the counsel engagement below: the question is not only *how do we erase
against an unbounded archive*, but *which data may we erase at all, when does each duty lift,
and what does separating retained data mean for a system whose backups are one undifferentiated
stream*. The last of those is squarely architectural, and it is unanswered here.

### 복구 또는 재생: the standard appears twice, and on the request path too

The record first cited only 법 제21조제2항 (파기). The owner pointed at the deletion-request
path; verified 2026-07-31 against the official portal, the same standard appears there as
well, in the parent Act rather than in 시행령 제43조:

> **법 제36조(개인정보의 정정ㆍ삭제)**
> ① … 정정 또는 삭제를 요구할 수 있다. **다만, 다른 법령에서 그 개인정보가 수집 대상으로
> 명시되어 있는 경우에는 그 삭제를 요구할 수 없다.**
> ③ 개인정보처리자가 제2항에 따라 개인정보를 삭제할 때에는 **복구 또는 재생되지 아니하도록**
> 조치하여야 한다.

시행령 제43조, which implements 제36조, is procedural only — request method, notification to a
providing processor, and a 10-day result notice. It carries no 복구 또는 재생 wording.

Two architectural consequences, stated as observations rather than legal conclusions:

**Correction, same day: an earlier draft of this section overstated.** It argued that because
the standard is phrased as 복구 또는 재생, the European package — deferral to a scheduled
overwrite — "maps poorly" to Korea, and that crypto-shredding was therefore the better-fitting
mechanism. The owner disputed it, and the statute supports the owner.

**The trigger standard is 지체 없이, not 즉시.** 법 제21조제1항 and 법 제36조제2항 both require
action 지체 없이 — without undue delay. That is the same standard as GDPR Art. 17(1), and it is
the standard the European position is built on. 시행령 제43조제3항's ten-day result-notice window
points the same way: deletion is treated as a process with an operational horizon, not an
instant. Nothing found requires immediate unrecoverability in every copy.

So **both readings are available and neither is established:**

- *A scheduled overwrite satisfies it.* 지체 없이 admits a reasonable operational window; 복구
  또는 재생되지 아니하도록 describes the measures taken when deleting, not a requirement that
  every replica become unrecoverable in the same instant. On this reading the European package
  — erase live, backup expires on a finite written cycle, erasure log kept outside it and
  re-applied on restore — is a coherent answer, and the finite window is the load-bearing part.
- *Recoverability is the test.* The standard is written about the state of the data rather than
  the schedule, and the PIPC 안내서 treats backup media as a place where destruction is
  performed, with no grace period stated.

The earlier draft asserted the second and dismissed the first. That was the mirror image of the
error this record's research warned against: the Korean material's silence on backups is **an
absence, not a permission** — and equally, it is not a prohibition. Converting silence into
either is the mistake.

**What this changes about the options: nothing is reordered.** Option B (a finite window) is
not subordinate to option A. It is the element the only articulated international position
depends on, and it remains the cheapest thing that moves this system from unbounded to bounded.
A and B are complementary rather than competing — B bounds the exposure, A shortens it further
where the archive already exists. Which combination is adequate is for counsel.

**제36조제1항 단서 confirms the retention dimension** recorded above from 제21조제1항 단서, and
sharpens it: where another statute designates the data as subject to collection, the subject
*cannot require* deletion. So the disposal primitive must resolve, per data class, whether a
pin exists before it can act — which is a lookup this system cannot currently perform.

### What others do — European practice and the hyperscalers

Researched 2026-07-31 across regulator text, hyperscaler documentation and engineering
write-ups, then adversarially fact-checked. Reported as what sources say.

**The European consensus is narrower than it is usually quoted as.** It is not "deferral". It
is: erase from live systems; do not surgically edit backups; hold the backup so that it is
used for nothing but restore; let it expire on a **finite, written, scheduled cycle**; keep a
data-minimised **erasure log outside the backup**; and **re-apply that log after any restore**.
The UK ICO frames it as data held "beyond use" and "replaced in line with an established
schedule". Germany's DSK Standard-Datenschutzmodell Module 60 permits deferral to a
*planmäßiges Überschreiben* while **explicitly rejecting** policy-based non-use — a staff
prohibition or an undertaking not to use the data does not discharge the obligation.

**There is no EDPB guideline on Art. 17 and backups.** The EDPB's CEF 2025 report (adopted
2026-02-10) is deliberately non-committal and lists further guidance as work not yet done;
Sweden, Portugal and Hungary formally asked for it. A record citing such a guideline is citing
a document that does not exist.

**The hyperscalers' mechanism is key destruction, and the published window is a bit-lifetime
rather than a recoverability window.** Google's data-deletion whitepaper says it outright:
cryptographic erasure "might occur before the backup that contains customer data has expired…
the customer data is unrecoverable even during its remaining lifespan on Google's backup
systems." Microsoft deletes per-chunk encryption keys on hard delete. Google is the only
vendor found that also bounds **key material itself** (≤45 days) — the loop that a
crypto-shredding design must close, since keys backed up indefinitely defeat the shred. AWS
publishes no deletion SLA at all and assigns backups to the customer.

**Korea is not the European position, and is not established either.** The
「개인정보의 안전성 확보조치 기준 안내서」(2025.11) treats backup media as a place where
destruction is *performed*, offering deletion plus supervision against restoration and
exclusion from subsequent backups — with no grace period and no alternative where a backup
set cannot be selectively edited. A search for a Korean equivalent of "beyond use" across
four PIPC instruments found none; recorded as **not established**, which is not the same
as absent.

### Correction: 백업 is in the 고시, and the earlier search did not reach it

This record previously stated that 백업 appears **zero times** in PIPA and its 시행령. That
is true of those two instruments and false as a claim about Korean law, because the
binding security standard is neither of them — it is a 고시, which is 행정규칙 and a
different search target. Searching `target=law` for it returns nothing, and that null
result was read as absence. The same mistake had already been made once in this repository
with four payroll 고시, and it is written into
`scripts/korean-legal/fetch-statutory-source.mjs` as the reason `--admrul` exists. The
search was run against the wrong target anyway.

Retrieved 2026-07-31 from the 국가법령정보센터 API,
**개인정보의 안전성 확보조치 기준** (고시, 개인정보보호위원회 제2026-9호, 시행 2026-07-01,
<https://www.law.go.kr/행정규칙/개인정보의 안전성 확보조치 기준>). 백업 appears **once**:

> 제11조(재해ㆍ재난 대비 안전조치) 10만명 이상의 정보주체에 관하여 개인정보를 처리하는
> 대기업ㆍ중견기업ㆍ공공기관 또는 100만명 이상의 정보주체에 관하여 개인정보를 처리하는
> 중소기업ㆍ단체에 해당하는 개인정보처리자는 […] 2. **개인정보처리시스템 백업 및 복구를
> 위한 계획을 마련**

**It requires a plan, and states no period.** That is the whole of Korean privacy law on
backups. The finding therefore strengthens rather than weakens what this record already
said: there is no Korean retention *duration* for a backup archive to satisfy — and the
obligation that does exist is a document, above a subject-count threshold this deployment
is nowhere near.

### The retention floors that do exist attach to other stores

Every figure below was retrieved from the instrument itself on 2026-07-31 via
`scripts/korean-legal/fetch-statutory-source.mjs --article`, not recalled. This states what
the instruments say; it does not state that any of them applies to this system, which is a
finding for counsel.

| Store | Instrument | Period |
| --- | --- | --- |
| 근로자 명부, 근로계약 서류 | 근로기준법 제42조 (시행 2025-10-23) + 시행령 제22조 | **3년**, from the nine start dates 시행령 제22조제2항 enumerates |
| 장부 및 증거서류 | 국세기본법 제85조의3제2항 (시행 2026-07-01) | **5년**, 7년 for 역외거래, from the day after the 법정신고기한 |
| 접속기록 | 안전성 확보조치 기준 제8조제1항 | **1년**; **2년** where the system handles ≥50,000 subjects **or 고유식별정보 or 민감정보** |
| Backup / PITR archive | 안전성 확보조치 기준 제11조 | **a plan; no period** |

Two consequences follow, and they point in opposite directions.

**The backup window has no floor.** A backup taken today already contains all three and
five years of those records — window length is how far *back* one may rewind, not how old
the data inside is. Importing 5년 from 국세기본법 into `retentionPolicy` would buy no
compliance and would triple the erasure horizon. It would also fail 법 제21조제3항, which
requires data preserved under the 제21조제1항 단서 to be stored **분리하여** — and a PITR
archive is an undifferentiated copy of the entire cluster, the opposite of separate
storage. The archive cannot be the statutory preservation medium.

**접속기록 has a floor this system will hit.** Korean payroll processes 주민등록번호, and
`개인정보 보호법 시행령` 제19조제1호 (시행 2026-05-19) names 「주민등록법」 제7조의2제1항에
따른 주민등록번호 as 고유식별정보. 제8조제1항 sets 1년 이상 and then 2년 이상
`다음 각 호의 어느 하나에 해당하는 경우` — **any one** of the listed cases — of which 제2호 is
`고유식별정보 또는 민감정보를 처리하는 개인정보처리시스템`. So the floor is **2년**
irrespective of headcount: the 50,000-subject test in 제1호 is an independent trigger, not a
qualifier on 제2호.

That is a floor on a real table, and it is the first Korean retention obligation found in
this work that binds regardless of scale. It is out of scope for this record, which is
about the archive, and is written down here so it is not lost.

### 위치정보법 imposes 즉시, and this record's reasoning was PIPA-shaped

**This is the sharpest finding in this record, and it narrows a claim made above.**

Everything preceding this section reasons from PIPA. PIPA 제21조제1항 and 제36조제2항 both say
**지체 없이**, and this repository has already recorded — correctly — that 지체 없이 is not 즉시,
that it tolerates a reasonable operational window, and that waiting for a backup cycle to turn
over is therefore arguable. That reasoning is sound for the data PIPA governs. **It does not
extend to personal location data, because a different statute governs that with a different
word.**

`위치정보의 보호 및 이용 등에 관한 법률` 제23조제1항 (법률 제21066호, 시행 2025-10-01):

> 위치정보사업자등은 개인위치정보의 수집, 이용 또는 제공목적을 달성한 때에는 […] 개인위치정보는
> **즉시** 파기하여야 한다. 다만, 다른 법률에 따라 보유하여야 하거나 대통령령으로 정하는 정당한
> 사유가 있는 경우 개인위치정보를 보유할 수 있다.

The surrounding provisions close the routes a bounded window would need:

- **제23조제2항** — destruction must include `복구 또는 재생을 방지하기 위한 조치`.
- **시행령 제26조의2제1항** (대통령령 제36084호, 시행 2026-02-10) 준용s 제8조의2, which requires
  deletion by a `재생할 수 없는 기술적 방법` or physical destruction.
- **시행령 제26조의2제2항** — the 단서's `대통령령으로 정하는 정당한 사유` means
  `개인위치정보주체가 자신의 개인위치정보의 보유에 관하여 별도로 동의한 경우` — **the data
  subject's separate consent, and nothing else.** Operational necessity is not named. Disaster
  recovery is not named.
- **시행령 제26조의2제3항** — even with that consent, retention is capped at **최대 1년**.
- **제40조의2** — `제23조제1항을 위반하여 개인위치정보를 파기하지 아니한 자는 2년 이하의 징역 또는
  2천만원 이하의 벌금에 처한다`. Criminal, not administrative.
- **제23조제3항** — the regulator may have officials inspect the **파기실태**, the destruction
  practice itself, rather than only the policy describing it.

**This system holds the data the article is about.**
`0005_create_compliance_location_store.sql:47-70` creates `location_pings` with `latitude`,
`longitude`, `accuracy_m` and `user_id` — coordinates bound to an identified person. ADR-0014
exists precisely to make that store destructible, and its Decision reads
`ADR-0014` `Destruction-on-withdrawal is physically realizable (drop partitions)`.

**The live-database side is sound; the archive side is not.**
`purge_expired_location_data` drops whole day partitions, and `destroy_location_data` deletes a
subject's pings inside one transaction. Both are real. But a dropped partition remains
reconstructable from the WAL archive for the length of the retention window, and a
reconstructable copy is neither 즉시 파기 nor 재생 불가.

**Consequence for the four options, and it is a hard one.** Option B — shorten the PITR window —
**cannot satisfy 제23조 at any value of N**, because the article's standard is not "short" but
"immediate", and 시행령 제26조의2제2항 does not admit a backup as a 정당한 사유. That is now a
structural limitation of option B rather than a preference against it. For personal location data
only **option A** (the archive holds ciphertext; destroying the key makes it 재생 불가) and
**option C** (segregate the store so it is not in the archived cluster at all) remain reachable.

**Two things this record still does not assert.** Whether 위치정보법 binds this deployment at all
turns on whether the operator is a 위치정보사업자 or 위치기반서비스사업자 under 제5조/제9조 — a
registration-status question, and a legal determination this record has no authority to make. And
whether any option is *sufficient* remains, as everywhere else here, a finding for counsel. What
is established is narrower and enough to change the engineering: **the instrument exists, its
standard is 즉시 rather than 지체 없이, its only exception is subject consent, and the data it
governs is already in this database.**

## What this record does not know

Stated rather than papered over, because two of the four options cannot be fully costed
without it.

- **Whether the backup bucket is retention-locked is undetermined.** ADR-0005's WORM rule
  binds evidence objects — `ADR-0005` `every evidence object must reach a retention-locked copy in an independent failure domain`,
  proven by `ADR-0005` `put-retention COMPLIANCE → version-delete attempt must fail`. No
  manifest in the tree applies an equivalent lock to `s3://mnt-db-backups/`, and
  `deploy/apps/object-store/README.md` `WORM/object-lock validation` lists that validation
  as work not yet run. If the backup bucket turns out to carry a compliance-mode lock, then
  option B is unavailable too — a retention policy cannot prune objects the store refuses
  to delete — and option A becomes the only one that does not require changing the bucket.
  **This is checkable against the live tenancy and should be checked before any option is
  weighed.**
- **No legal question here has an answer in this repository**, including whether any
  bounded window, any encryption scheme, or any procedure is adequate. This record's whole
  contribution is that the architectural question is now written down with prices attached.

## Consequences

- **Positive: the conflict stops being invisible.** It currently exists between two
  accepted records with nothing pointing from either to the other, which is how a
  contradiction survives review.
- **Positive: the cheap moment is named while it is still open.** The record states
  explicitly that the resolution costs less before personal data lands, so a later
  migration that stores it is a decision to pay more, taken knowingly.
- **Positive: constraint 3 stops the misdescription from spreading** into the prose that a
  later design would be built from — the same failure mode ADR-0035 records for
  conservation and the row CHECK.
- **Negative: the location paths that ship today keep this property for as long as the
  record stays proposed.** `destroy_location_data` runs against the archive now, and nothing in
  this record changes that. `purge_expired_location_data` does not run at all — see the retraction
  in Context — so the live store is unbounded as well as the archive.
- **Negative: constraint 3 has no automated enforcement.** No gate distinguishes "deleted"
  from "unreconstructable" in prose. Only review does, which is weaker than the property it
  protects, and is recorded as such rather than overstated.
- **Negative: this record consumes an ADR number for a question rather than an answer**,
  and will need a successor. That is the intended trade: an open question in the decision
  log beats a closed one decided by whoever writes the next migration.

## Follow-ups

1. **Determine the object-lock posture of `s3://mnt-db-backups/` against the live
   tenancy.** It is a read-only check, it gates the availability of option B, and no option
   should be weighed before it is known.
2. **Engage privacy counsel as an engagement distinct from the 노무사/세무사 payroll
   sign-off.** Name the four options and ask which are available, not which is best — the
   architecture question is answered here, the availability question is not.
   **Put 위치정보법 first on that agenda, ahead of the retention number.** Two questions, in
   order: is this operator a 위치정보사업자 or 위치기반서비스사업자 under 제5조/제9조; and if so,
   does 제23조제1항's 즉시 admit any architecture in which a dropped `location_pings` partition
   remains restorable from a WAL archive. A negative answer to the second forecloses option B
   for that data regardless of what the window is set to, and 제40조의2 makes the exposure
   criminal rather than administrative.
3. **Do not write a personal-data migration before this record has a successor.** The
   timing argument is the whole reason this was worth writing now; the first such migration
   spends the option it names.
4. **If option A is chosen, design the key store's DR posture as its own decision.** A
   store whose correct failure mode is permanent loss cannot inherit ADR-0015's contract,
   and treating it as ordinary infrastructure would silently undo every shred.
5. **Re-check the reciprocal obligations before acceptance.** This record declares
   `related` only and asks for no key in any target, so no target ADR is edited by its
   acceptance; that claim should be re-verified against the six targets at the time,
   because `related` reciprocity is not machine-enforced.

Every capability, evidence contract, jurisdiction binding, Korea control, review
disposition, and exposure state remains `HOLD`; this record poses a question, records no
decision, and makes no completion, deployment, or production-exposure claim.
