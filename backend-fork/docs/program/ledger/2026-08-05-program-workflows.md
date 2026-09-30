# Ledger — program workflows (domain-increment + program-tick)

## Identity
- PR chore/program-workflows
- Base: main post-#573

## What shipped
- `.grok/workflows/domain-increment.rhai` — specify→design→build→verify→review→handoff
- `.grok/workflows/program-tick.rhai` — read-only operator heartbeat

## Non-goals
Auto-merge, HOLD clearance, product feature code.


<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "release",
    "other"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Essentialism / YAGNI",
    "Chesterton's Fence",
    "Red Team",
    "Systems Thinking",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Workflows are orchestration only; they do not claim product completion or HOLD clearance.",
    "Essentialism / YAGNI": "Two workflow files only; no product code.",
    "Chesterton's Fence": "Leader merge and HOLD gates remain outside automation.",
    "Red Team": "Rejects silent HOLD clearing and destructive git in agent prompts.",
    "Systems Thinking": "Separates specify/design/build/verify/review/handoff phases with leases.",
    "Operability / Day-2": "program-tick provides operator heartbeat without auto-merge.",
    "Blast-radius / cell-based": "isolation_worktree for build; disjoint crate roots for parallel lanes.",
    "Telemetry-first": "Handoff schema captures candidate notes for merge evidence.",
    "Zero-trust / defense-in-depth": "Reviewers are split-context and read-only; executors cannot mutate ultragoal."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Program workflows encode SDLC phases without replacing DELIVERY admission records.",
    "Auto-merge is intentionally omitted; leader owns protected integration."
  ],
  "decisions_changed_or_rejected": [
    "Rejected auto-merge from program-tick.",
    "Rejected domain fan-out without prep pack."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
