# Disk-wipe consolidation final candidate authority

## Exact identity

- Superseded authority C/T: `534f054f4769d08f3d62214c7bee8af28f1ff8c2` / `3cf035d028a4c888a6db8f787744ff36125e280c`
- Final signed content candidate C: `1a7ad2a36d3a1a8e560d89e8a64a1832ae55e6ca`
- Final authority tip T: the signed direct child that adds only this ledger entry
- Protected base B: `435e251edfab12750850d5b1d411528b10a3ed8a`

Protected-main simulation authenticated the superseded pair and passed all 71
dependency-empty candidate checks. Its complete local verifier then passed every
code, contract, security, CI, Buck, Cargo, and domain-unit surface it ran except
one stale citation: a historical draft still described the `pull_request` path
filter that this consolidation intentionally removed. Intermediate signed content
commit `86e4ae0e48f78a76100cdc5a5707f791a1ec5b50` corrected that paragraph; the citation
verifier then reported 678 total citations with zero broken and zero unverifiable.

Independent review also found that the hermetic-authority ledger overstated its
secondary proof. Normal CI unconditionally reruns the exact-byte and focused
structural authority-workflow suite after install, and CI preflight locks that
reachability; no separate generic YAML-parser proof reads the target-only workflow.
Final C corrects the attestation to match the executable evidence. It adds no
product or workflow behavior beyond the already simulated dependency-free fix.

All prior consolidation scope, branch-deletion evidence, preservation/rejection
decisions, non-authoritative external-opinion treatment, ignored/Ultragoal policy,
external secret-custody requirements, protection migration, formal approval,
merge/release closeout, final readback, and safety holds remain in force. Only
simulation, complete verification, and independent reviews bound to the exact T
containing this entry can authorize the PR to proceed.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "approval",
    "release"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Red Team",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Separated an actual exact-byte suite rerun from an inferred YAML-parser proof and retained only what the executable paths establish.",
    "Red Team": "Let full verification and independent review invalidate two documentary claims instead of treating the signed prior tip as presumptively complete.",
    "Operability / Day-2": "Left future maintainers a current path-filter description and an accurate account of the trust-root runtime boundary.",
    "Blast-radius / cell-based": "Confined both corrections to the two inaccurate paragraphs and resealed one final candidate without changing product or workflow behavior.",
    "Telemetry-first": "Recorded every superseded identity, the one failing verifier surface, the 678-citation clean rerun, and the exact limits of secondary workflow evidence.",
    "Zero-trust / defense-in-depth": "Invalidated prior tip authority and requires simulation, reviews, hosted CI, and formal approval to bind independently to final exact T."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "The broad implementation was green, but one stale citation and one overclaimed parser proof made its documentary evidence unfit for final merge authority."
  ],
  "decisions_changed_or_rejected": [
    "Rejected waiving a documentation-only verifier failure during disk-wipe closeout.",
    "Rejected inventing a generic YAML-parser proof when the repository establishes an exact-byte structural suite and its unconditional post-install rerun instead.",
    "Rejected carrying reviews or simulation authority forward from superseded bytes."
  ],
  "lens_set_changes": [
    "Added Cartesian doubt after independent review exposed a distinction between observed suite execution and inferred parser coverage."
  ]
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
