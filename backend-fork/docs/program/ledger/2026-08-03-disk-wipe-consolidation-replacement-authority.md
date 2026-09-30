# Disk-wipe consolidation replacement candidate authority

## Exact identity

- Protected base B: `435e251edfab12750850d5b1d411528b10a3ed8a`
- Superseded signed C/T: `b87d09596cbbd1775fbfd0e82371a8a0a08e39a2` / `c61edcd14ef9929c10fce763d972f7ea9f639647`
- Signed repair checkpoint: `baadf03cf4692754fd0b964834e324d14e48f20e`
- Final signed content candidate C: `e1e63fd9e43d83d90edeba2cdcae28e9c78145be`
- Final authority tip T: the signed direct child of C that adds only this ledger entry

The external scorecard relayed during consolidation remains an opinion only. It
authorized no deletion, scope change, product claim, operational claim, or code
change. Every correction below came from executable repository evidence observed
independently of that opinion.

## Why the preceding pair was superseded

A clean fast-verifier run invalidated the preceding signed pair. The first-party
Buck generator had discovered new inline unit targets in `console-registry-rest`
and `console-reporting-rest`, but its reviewed resource map and generated BUCK
files did not admit them. The same behavioral lock still expected 152 ordinary
and 17 SQLx app tests after the candidate had raised those exact cardinalities to
153 and 18. The generator correctly failed instead of silently omitting tests.

The production-hardening regression also timed out while executing fresh shell
command doubles on macOS. Process inspection reproduced the stall before
`deploy.sh` reached product logic: provenance policy held a newly created
executable at process startup, while `/bin/bash <same-file>` completed in
milliseconds. Increasing the timeout would only hide that harness defect.

Final C admits both unit targets as reviewed `resource.none` tests, regenerates
their two BUCK faces, and updates the exact cardinality locks. The deploy test
harness now keeps command doubles non-executable and outside `PATH`, loads
functions through `BASH_ENV`, invokes each double explicitly with `/bin/bash`,
and distinguishes a spawn error from the expected nonzero deploy result. No
production deploy behavior changed.

## Verification before T

- The first-party generator regression passed 24/24. Cheap preflight passed with
  169 generated first-party BUCK files and 335 enumerated Rust test targets.
- After installing the exact `nightly-2026-02-28` toolchain already pinned and
  installed by hosted CI, the complete first-party plus Reindeer generated-face
  closure passed without drift.
- The focused production-hardening file passed 56/56; all four deploy fail-closed
  scenarios completed in under one second instead of reaching the ten-second
  harness timeout. An independent adversarial review found the final diff scoped
  only to the test harness and returned PASS.
- A clean `npm run verify` at signed checkpoint
  `baadf03cf4692754fd0b964834e324d14e48f20e` passed every non-authority stage.
  Its only failures were the two exact-M admissions, which correctly rejected
  that checkpoint because its parent-to-head diff changed test code rather than
  being a ledger-only direct-child authority tip. This T supplies that required
  structure; the complete verifier must now pass again against exact T.
- The durable Ultragoal execution receipt and disk-wipe handoff record both
  repairs, the expected pre-authority rejection, external credential custody,
  and the rule that only final `main` is a fresh-session continuation source.

All earlier preservation/rejection decisions, branch-deletion inventory,
product-pivot boundary, archive tags, external secret-custody requirements,
formal non-author review requirement, hosted required-context requirement,
release closeout, post-merge readbacks, and infrastructure/legal safety holds
remain in force. This entry authorizes exact-object review and verification of C/T
only. It is not itself merge, release, deployment, or disk-wipe authority.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "contracts",
    "approval",
    "release"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Red Team",
    "Systems Thinking",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Allowed a clean verifier to revoke a signed pair and treated both failures as defects until their exact causes were reproduced.",
    "Red Team": "Required the generator to account one-for-one for newly discovered tests and made the shell-wrapper path mechanically load-bearing rather than accepting a longer timeout.",
    "Systems Thinking": "Connected Cargo test additions, generated Buck metadata, local macOS execution policy, hosted runner setup, C/T topology, and restart continuity as one release boundary.",
    "Operability / Day-2": "Pushed signed repair custody while the PR stayed draft, installed the repository-pinned Reindeer toolchain, and recorded the exact recovery path for a fresh machine.",
    "Blast-radius / cell-based": "Changed two generated test faces, their generator metadata/cardinality locks, one test harness, and the durable receipt without changing deploy product behavior or widening pivot scope.",
    "Telemetry-first": "Bound the replacement to exact commits, signatures, 169 generated faces, 335 targets, focused test counts, full-face results, and the expected pre-authority exact-M rejection.",
    "Zero-trust / defense-in-depth": "Preserves separate exact-T simulation, two independent object reviews, hosted checks, formal external approval, branch protection, and post-merge readback after local repair evidence."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "A clean generated-face gate found two real unit targets that the reviewed Buck metadata had not admitted.",
    "Fresh executable test doubles are not a portable macOS harness boundary; explicit Bash interpretation avoids provenance startup stalls without changing production code."
  ],
  "decisions_changed_or_rejected": [
    "Rejected merging or carrying review authority forward from the preceding signed pair after clean verification failed.",
    "Rejected increasing subprocess timeouts to conceal an execution-policy stall.",
    "Rejected treating a content repair checkpoint as an authority tip merely because it was signed and recoverable remotely."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
