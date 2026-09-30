# Disk-wipe consolidation hermetic authority correction

## Exact identity

- Superseded diagnostic C/T: `bbc0cd6f6de43be5d1202d61a02b556fd242b515` / `f858e26960fe24048d59b37d6d4372d9421e5583`
- Corrected signed content candidate C: `534f054f4769d08f3d62214c7bee8af28f1ff8c2`
- Corrected authority tip T: the signed direct child that adds only this ledger entry
- Protected base B: `435e251edfab12750850d5b1d411528b10a3ed8a`

The first C/T pair passed protected-main graph authentication, but the subsequent
authenticated candidate suite failed in a dependency-empty worktree: the candidate
bootstrap test imported `js-yaml` even though the trust-root workflow deliberately
runs no package installation before candidate authentication. That pair is retained
as an honest diagnostic record and is not the merge authority.

The corrected C removes the runtime package import from the pre-install suite. It
binds the authority workflow's exact SHA-256 bytes and retains focused assertions
for its trigger, permissions, job separation, protected-target checkout, proof
ordering, and external-head fail-closed behavior. Normal CI reruns that exact-byte
and focused structural suite after
its lockfile-governed install boundary; CI preflight locks the suite's unconditional
reachability and execution semantics. No separate generic YAML-parser coverage is
claimed for this target-only workflow. The corrected bootstrap suite passed 17/17
locally without relying on that package.

All scope, preservation/discard decisions, external-opinion limitations, branch
deletion evidence, secret-custody requirements, review requirements, protection
migration requirements, post-merge readbacks, and safety holds in the preceding
[`2026-08-03-disk-wipe-consolidation-authority.md`](2026-08-03-disk-wipe-consolidation-authority.md)
remain in force. Exact-T simulation, independent review, hosted CI, formal approval,
merge, release closeout, and final branch/PR readback remain pending at this record.

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
    "Red Team",
    "Systems Thinking",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Red Team": "Executed the protected trust-root path in a dependency-empty candidate worktree and refused to waive the reproduced failure.",
    "Systems Thinking": "Distinguished the pre-authentication runtime boundary from the normal post-install suite rerun instead of assuming one environment or an unrelated YAML parser covered both.",
    "Operability / Day-2": "Made the trust-root regression suite runnable on the actual hosted job contract without hidden workstation state.",
    "Blast-radius / cell-based": "Changed one test file, preserved the workflow bytes and protection model, and resealed authority instead of broadening bootstrap privileges.",
    "Telemetry-first": "Recorded the failed C/T identities, exact missing dependency, corrected C, workflow digest contract, and 17/17 regression result.",
    "Zero-trust / defense-in-depth": "Kept candidate code behind protected-target signature authentication while retaining an unconditional post-install suite rerun and CI reachability locks."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "A package-backed test inside a deliberately dependency-free authentication boundary would have made the required authority context fail on hosted CI."
  ],
  "decisions_changed_or_rejected": [
    "Rejected treating graph authentication alone as a pass after the authenticated candidate suite reproduced a hermeticity failure.",
    "Rejected adding an npm install to the trust-root job; the regression test was made dependency-free instead."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
