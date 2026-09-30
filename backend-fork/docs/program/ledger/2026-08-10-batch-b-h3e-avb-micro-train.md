# Authority tip — Batch B h3e→avb micro-train

**Date:** 2026-08-10
**Kind:** authority tip ledger bound on candidate C (product tip below); jurisdiction tip T is register-only (#618 grammar)
**Candidate (authority train):** `f10937d6ca91599afbf7e3516f554dd1589e99b4` (immutable absolute SHA of the product tip that C parents; rekeyed at C commit time)
**Scope:** Micro-train after split Batch B (#736) — critic-APPROVED h3e then avb (avb stacks on h3e; hard exact-file collision on canonical-domain). Does **not** re-admit ugg/6n4/nuc/pnb1 (already on main via #736).
**Not product authority.** Clears no HOLD beyond the beads closed on merge. Makes no production, frontend, or projection claim.

## Summary

- **h3e:** require explicit CanonicalQuery::subject_id (delete trait default None) — mechanism replacement for q06 silent-skip. Tip restack of `82eebb31c` onto post-#736 main as cherry-pick `cc2c9db75`.
- **avb:** map CanonicalPort::Error kinds via CanonicalPortError::into_kernel_error instead of flattening to internal; projected-dispatcher call-site mutate→red pin for DigestConflict→Conflict. Restack of lane tip `9c9c7f8f3` (commits `742f93c33`+`9c9c7f8f3`) as `9f19e5164`+`faffbf0a8`. Auto-merge preserved 6n4 valid_to close and nuc canonical projected audit emit.

## Remaining HOLDs / follow-ups (not closed by this tip)

- console-avb critic minor ownerLease — PayRun Lifecycle→internal wholesale refinement (payroll)
- console-avb critic minor unproven — OrgUnit/JobPosition/Person port_error_kind unit-pin parity with Company
- Class bead (parent): process.call-site-pin-missing / ontology port-error flatten — confirm or open sweep

## Critic receipt binds (lane worktree APPROVE tips)

| leaf | tip | critic |
|------|-----|--------|
| h3e | 82eebb31c | h3e-critic APPROVE |
| avb | 9c9c7f8f3 | avb-critic APPROVE (agent a55d704e) |

## Beads on merge

Close: console-h3e, console-avb, console-q06. Do not re-open/re-admit ugg/6n4/nuc/pnb1.

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
    "Cartesian doubt": "Verified #736 merge tip e391abb91 on origin/main before cherry-pick; confirmed 6n4 valid_to and nuc audit emit survived avb auto-merge.",
    "Essentialism / YAGNI": "Admit only h3e→avb micro-train; refuse dual-admit of already-merged ugg/6n4/nuc/pnb1.",
    "Chesterton's Fence": "Kept #618 C/T grammar (ledger+seed/index on C; jurisdiction register on T) proven on split #736.",
    "Red Team": "Stacked avb on h3e for hard collision on canonical-domain/src/lib.rs; verified post-merge employment/rest surfaces did not regress split landings.",
    "Operability / Day-2": "SSH-signed product+C+T; documentation seed+index on C; check-admit-sync after push; local cargo pins before PR.",
    "Blast-radius / cell-based": "In-hub admission worktree from post-#736 main; path-scoped micro-train; no update-branch button.",
    "Zero-trust / defense-in-depth": "Critic APPROVE a55d704e at 9c9c7f8 required; dispatcher call-site pin mutate→red proven; authenticate-console-authority allow-list."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Split #736 MERGED e391abb91 before micro-train open.",
    "h3e cherry-pick clean; avb auto-merge preserved 6n4/nuc product surfaces.",
    "projected_dispatch 9/9 including digest_conflict call-site pin; canonical-domain 11/11; company port_error_kind 2/2.",
    "q06 closes only with h3e on landed admit tip.",
    "Admit-window CI: declare unit test resources for avb port_error_kind pins (canonical-adapter + orgchange BUCK).",
    "Admit-window CI: wire console-ontology-canonical-adapter-postgres + console-orgchange-adapter-postgres --lib into domain-unit; ratchet executed-tests attribute baseline; rehash domain-unit proofDigest.",
    "Admit-window CI: EchoQuery in canonical_dispatch_audit_as_runtime_role implements subject_id after h3e deleted the trait default."
  ],
  "decisions_changed_or_rejected": [
    "Rejected re-admitting ugg/6n4/nuc/pnb1 after #736.",
    "Rejected admitting h3e alone ahead of avb (hard collision cell).",
    "Rejected using lane tips without restack onto post-#736 main."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
