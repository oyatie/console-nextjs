# Disk-wipe consolidation custody-truth-corrected authority

## Exact identity

- Protected base B: `435e251edfab12750850d5b1d411528b10a3ed8a`
- Refused local predecessor tip:
  `17f84f210a9dbf32987cdff36d4aeabaff4540ac`
- Final signed content candidate C:
  `ddaca4d85b461ac881740b92bae4ded5fe0bc6a2`
- Final authority tip T: the signed direct child of C that adds only this file

C retains every reviewed implementation and ignored-artifact correction from the
predecessor. It changes only three durable records that independent exact-object
review found could falsely imply that restricted workbook inputs had already
reached external encrypted custody. The refused predecessor was never pushed and
never acquired merge authority. Its source results cannot authorize this pair.

The external scorecard relayed by the owner remains opinion only. It authorized
no implementation, deletion, product boundary, readiness, deployment, or
compliance claim. This correction follows the independently reproduced local
custody state and exact-object reviews only.

## Truth correction

No usable off-device destination or independently verified read-back existed at
C. The two ignored workbook profiles and their source therefore remain local
restricted inputs subject to a still-pending encrypted-preservation-or-explicit-
discard decision. C now states that fact consistently in the machine execution
receipt, the durable disk-wipe handoff, and the draft data-exchange spec. Future
implementation may restore the inputs only if approved encrypted custody is
later established and read back; after an explicit discard it must instead
obtain and review a newly authorized source.

The machine execution receipt is SHA-256
`49333378c2c756107798609be42c3266b7ef1bfbbe526a38618a0d3c11a76ce2`,
and the disk-wipe handoff binds that exact value. Ten stale pre-pivot ignored OMX
artifacts remain explicitly retired by path, size, and digest. The two restricted
profiles remain excluded from Git and bound only by their custody receipts. No
raw workbook value, ignored artifact, secret, or outside-repository dirty byte
entered C.

## Verification before T

- C is signed by the pinned ED25519 authority fingerprint
  `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`.
- Two independent exact-object reviewers refused predecessor T
  `17f84f210a9dbf32987cdff36d4aeabaff4540ac` for the same three statements and
  passed its remaining signature, topology, hash, reference, secret-boundary,
  external-repository, and simulator-assumption checks.
- C applies the complete correction set specified independently by both
  reviewers and explicitly records the refused tip as superseded.
- After correction, JSON parsing and receipt-hash readback pass, `git diff
  --check` passes, the documentation-link scan passes across 373 Markdown files,
  foundation validation passes 134 checks, and the reasoning-lens suite passes
  40/40.

These focused measurements authorize exact-object verification of C/T only.
Before squash merge, the protected-main simulator, complete exact-T local
verifier, two independent exact-object reviews, every hosted required context,
one formal non-author GitHub approval with stale/last-push/conversation controls,
and branch-protection readback must all pass on the final tip. The squash-binding
workflow, any generated release PR, final main/tag/release/branch/PR readback,
and zero-open-PR state remain separate closeout gates.

Disk erase remains blocked even after repository merge. The exact signing key is
still needed for the generated release PR, and no approved encrypted off-device
destination or independent read-back exists for the restricted workbook inputs,
identity/infrastructure secrets, Talos recovery tree, business inputs, or the
three outside repositories. This ledger grants no wipe, deployment, preservation,
or discard authority.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "authz",
    "migration",
    "contracts",
    "approval",
    "hr_payroll",
    "release",
    "production",
    "compliance_sensitive"
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
    "Cartesian doubt": "Refused a signed and fully locally verified tip when three durable statements contradicted the observed absence of any usable custody destination.",
    "Red Team": "Searched every custody and preservation statement across the correction set and required restoration to be conditional on later verified custody.",
    "Systems Thinking": "Kept repository authority, restricted business inputs, signing identity, outside repositories, and physical backup availability as separate wipe-safety boundaries.",
    "Operability / Day-2": "Made the fresh-session path distinguish later verified restoration from owner-approved discard and acquisition of a newly authorized source.",
    "Blast-radius / cell-based": "Corrected three continuity records, copied no restricted bytes, and made no mutation to outside repositories or infrastructure.",
    "Telemetry-first": "Bound the correction to exact refused and replacement commits, the execution-receipt digest, independent review findings, and focused check counts.",
    "Zero-trust / defense-in-depth": "Kept exact-object review, local simulation, hosted CI, formal approval, merge binding, release closeout, and external custody as independent gates."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Classifying an input for encrypted custody is not evidence that encrypted custody or an independent read-back has occurred.",
    "A continuation instruction must account for both approved preservation and explicit discard without assuming either decision has already been made.",
    "A remote-safe Console repository does not make the whole disk safe while restricted local inputs and unrelated repository state remain without verified custody."
  ],
  "decisions_changed_or_rejected": [
    "Refused local predecessor tip 17f84f210a9dbf32987cdff36d4aeabaff4540ac despite its passing full local verifier.",
    "Rejected wording that converted a pending custody classification into a claim of completed encrypted preservation.",
    "Rejected a restoration-only continuation path because explicit discard remains an allowed owner decision.",
    "Rejected treating Console merge completion as whole-disk wipe authority."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
