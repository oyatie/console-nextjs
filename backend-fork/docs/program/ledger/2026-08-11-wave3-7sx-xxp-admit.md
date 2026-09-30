# Authority tip — Wave-3 admit (7sx / xxp)

**Date:** 2026-08-11
**Kind:** authority tip ledger bound on candidate C (product tip below); jurisdiction tip T is register-only (#618 grammar)
**Candidate (authority train):** `f3d080280d2c26c502e18073fb85438ea442d939` (immutable absolute SHA of the product tip that C parents; rekeyed at C commit time after restack onto main 59c9c8861)
**Scope:** two critic-APPROVED leaves after restack onto main `59c9c8861` (#751 on top of #753/#744). Leaves: console-7sx (L5-ORG) + console-xxp (L5-JOB). No Wave4 / #754.
**Not product authority.** Clears no HOLD beyond beads closed on merge. Does not unpark console-y0n until this train lands.

## Summary

- **7sx (L5-ORG):** Company/OrgUnit reference surface on org-entities; FORCE-RLS reads armed via `with_org_conn` + console_rt oracle pin.
- **xxp (L5-JOB):** JobPosition first-class query (`get` / `list_for_org_unit`) + action receipt ID readback; free-text/recruiting never inferred as job_positions.
- **Layer boundary:** orgchange adapter no longer depends on ontology adapter (OrgUnit binding moved to ontology canonical-adapter).

## Remaining HOLDs / follow-ups (not closed by this tip)

- console-y0n — PARKED; unpark after 7sx+xxp land (EmploymentAttributes UUID + ReassignOrgUnit→hr.transfer)
- console-7sx minor — CreateRegion/CreateBranch binding emit lacks dedicated console_rt apply-path pin (readback-only coverage)
- console-xxp optional — revise/move receipt `org_unit_id` + post-move get/list asserts

## Critic receipt binds (lane worktree APPROVE tips)

| leaf | product tip | critic |
|------|-------------|--------|
| 7sx | c8fdd7d21dbb9650b6d648485bb4d8a20bf3d444 | 7sx-critic APPROVE (PR tip receipt-binds 14e5de608) |
| xxp | 445e986998ead19d22d16c2de762831d37414cba | xxp-critic APPROVE (major+proven tip-serial baseline = admit-owned; folded here) |

## Beads on merge

Close: console-7sx, console-xxp (+ linked GH #752 / #750 as superseded by this train). Unpark: console-y0n.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "authz",
    "hr_payroll",
    "release"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Essentialism / YAGNI",
    "Chesterton's Fence",
    "Red Team",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Restacked product tip onto origin/main 59c9c8861 (#751) before C/T; verified critic APPROVE receipts and empty 7sx/xxp path intersection.",
    "Essentialism / YAGNI": "Two-leaf serial admit only; refuse Wave4/#754 and y0n product scope.",
    "Chesterton's Fence": "Keep #618 C=ledger+seed/index, T=jurisdiction-only grammar after restack.",
    "Red Team": "Path overlap 7sx/xxp = NONE; FORCE-RLS org reads remain with_org_conn; JobPosition never widens OrgEntitySummary; orgchange no longer depends on ontology adapter.",
    "Operability / Day-2": "SSH-signed C/T; check-admit-sync after push; Required CI before squash; unpark y0n only after merge.",
    "Blast-radius / cell-based": "Admission WT under hub .worktrees/admission-wave3-20260811; sole-writer remint after DIRTY vs main.",
    "Zero-trust / defense-in-depth": "Critic APPROVE required; layer-boundary heal keeps orgchange→ontology edge out of adapter graph."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "PR was DIRTY vs main 59c9c8861 (#751); rebuilt product tip via cherry-pick then reminted C/T.",
    "first-party BUCK regen + domain shard tripwire 85 included in product tip.",
    "Concurrent tip rewrites / API rate-limit cancelled earlier Required runs."
  ],
  "decisions_changed_or_rejected": [
    "Rejected merging while mergeStateStatus=DIRTY; restacked onto main first.",
    "Rejected folding Wave4/#754 into this train."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
