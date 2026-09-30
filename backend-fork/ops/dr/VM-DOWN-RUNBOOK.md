# VM-Down Emergency Dispatch Runbook

> **POST-PIVOT UNVERIFIED / HOLD:** This pre-pivot incident procedure is retained
> as historical/operator reference. It is not evidence of a current production
> VM, current personnel/communications authority, or an activated recovery path.
> The repository currently authorizes zero production mutations. Start with the
> [disk-wipe consolidation handoff](../../docs/handoffs/2026-08-03-disk-wipe-consolidation.md).

## Trigger

Use this runbook when the production VM, public API, or Postgres primary is down
and a P1 emergency dispatch is in flight or arrives while the system is
unavailable.

## Roles

- 관리자: incident commander, manual assignment authority, recovery approver.
- 접수자: dispatch coordinator, customer contact, Alimtalk operator.
- 정비사: on-duty technician pool for the affected branch or region.
- 임원: escalation recipient when RTO risk exceeds 1 hour.

No personal phone numbers or private identifiers are stored in this repository.
Operators use the current branch roster and approved Alimtalk console during an
incident.

## In-Flight P1 Manual Dispatch

1. 접수자 records the P1 request on the approved paper or shared emergency sheet:
   branch, customer/site, equipment identifier, symptom, received time, and
   whether anyone was already assigned before outage.
2. 접수자 starts the first-contact timer.
3. 관리자 identifies the eligible on-duty 정비사 pool for the branch. If branch
   coverage is unavailable, 관리자 expands to the region pool.
4. 접수자 calls the first eligible 정비사 by phone. If unanswered after one
   attempt, move to the next eligible 정비사 and send the approved P1 Alimtalk
   escalation message.
5. The first 정비사 who verbally accepts is manually assigned by 관리자.
6. 접수자 notifies the customer/site that manual dispatch is active and records
   the contact time.
7. 관리자 watches for RTO risk. If service is not expected within 1 hour, notify
   임원 and keep dispatch operating manually until recovery.

## Communication Tree

| Event | Primary role | Backup role | Channel |
| --- | --- | --- | --- |
| Incident declaration | 관리자 | 임원 | Phone |
| P1 intake while down | 접수자 | 관리자 | Phone |
| Technician contact | 접수자 | 관리자 | Phone + approved Alimtalk |
| Customer/site update | 접수자 | 관리자 | Phone |
| RTO risk escalation | 관리자 | 임원 | Phone |
| Recovery approval | 관리자 | 임원 | Phone + post-restore audit note |

## Recovery Reconciliation

1. After `/readyz` is healthy, 관리자 freezes new manual dispatch entries.
2. 접수자 enters each paper-sheet P1 into the recovered system.
3. 관리자 records the manual assignment decision and exact phone/Alimtalk times
   in the work-order notes.
4. 관리자 verifies the audit log contains the recreated P1, assignment, and
   reconciliation note.
5. 접수자 marks the emergency sheet reconciled and stores it in the approved
   operational evidence location.

## Rehearsal Evidence

Each rehearsal writes a timestamped log under `docs/evidence/` containing:

- simulated P1 received time;
- first technician contact attempt time;
- manual assignment acceptance time;
- time-to-first-contact in seconds;
- role names used in the drill;
- recovery reconciliation notes.
