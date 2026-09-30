# Disk-wipe consolidation custody-corrected authority

## Exact identity

- Protected base B: `435e251edfab12750850d5b1d411528b10a3ed8a`
- Revoked predecessor tip: `db7eda3ec2783ca93039b9d03cc2aaade613a927`
- Final signed content candidate C:
  `a1bf36f6a88ed73301a3bf4a21b855fa1ed321d7`
- Final authority tip T: the signed direct child of C that adds only this file

C preserves every reviewed implementation repair from the predecessor and adds
only the ignored-artifact and whole-disk custody correction. The predecessor had
passed source, signature, topology, local-verifier, protected-main simulation,
and hosted authority checks, but a later read-only ignored-file audit disproved
its broad claim that all useful planning context was already self-contained.
That factual contradiction revoked its merge authority. No earlier C/T identity
or check result can authorize this pair.

The external scorecard relayed by the owner remains opinion only. It authorized
no implementation, deletion, product boundary, readiness, deployment, or
compliance claim. This correction follows independently reproduced filesystem,
Git-reference, and tracked-reference evidence only.

## Custody correction

Tracked records named twelve ignored OMX artifacts that would disappear in a
fresh clone. Ten were broad pre-pivot issue, pull-request, route-audit,
frontend, or platform-maturity evidence. Their exact paths, byte lengths, and
SHA-256 digests are now retained in the disk-wipe handoff; their active reference
edges were removed or marked historical, and the bytes are explicitly retired.
They cannot silently regain product or continuation authority if later recovered.

The other two artifacts are 53,955-byte and 220,509-byte profiles derived from a
real eight-sheet HR/payroll workbook. No profile bytes or raw values entered Git.
The handoff binds their hashes and places both profiles with the unreadable
910,174-byte source workbook in a restricted encrypted-custody-or-explicit-
discard decision. A high-confidence pattern scan found no private-key, token,
RRN, phone, or email values in the profiles, but this is not a data-publication
review and does not establish de-identification.

The same bounded audit found no usable or independently read-back off-device
destination. It also found material dirty, untracked, stashed, worktree-only, or
unpublished state in Oyatie, TencentDB-Agent-Memory, and Asterinas. Console merge
authority does not authorize mutation of those repositories. Their exact
read-only counts and recovery boundary are now part of the durable handoff so a
future session cannot equate a clean Console remote with whole-disk safety.

## Verification before T

- C is signed by the pinned ED25519 authority fingerprint
  `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`.
- Independent review recomputed all ten retired-artifact sizes and hashes and
  both restricted-profile sizes and hashes against the untouched original
  worktree; every receipt matched.
- No exact retired or restricted artifact reference remains outside the custody
  handoff. All three changed JSON documents parse, the current execution receipt
  hash matches the handoff, `git diff --check` passes, and the documentation-link
  scan passes across 372 Markdown files.
- Foundation validation passes 134 checks, the reasoning-lens suite passes
  40/40, and ADR governance passes 29/29. These focused results supplement but
  do not replace exact-T full verification or hosted CI.
- No ignored artifact, workbook content, secret value, outside-repository dirty
  file, or external audit prose was copied into the candidate.

These measurements authorize exact-object verification of C/T only. Before
squash merge, the protected-main simulator, complete exact-T local verifier, two
independent exact-object reviews, every hosted required context, one formal
non-author GitHub approval with stale/last-push/conversation controls, and branch-
protection readback must all pass on the final tip. The squash-binding workflow,
any generated release PR, final main/tag/release/branch/PR readback, and zero-open-
PR state remain separate closeout gates.

Disk erase remains blocked even after repository merge. The exact signing key is
still needed for the generated release PR, and no approved encrypted off-device
destination or independent readback exists for the restricted workbook evidence,
identity/infrastructure secrets, Talos recovery tree, business inputs, or the
three outside repositories. This ledger grants no wipe, deployment, or discard
authority.

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
    "Cartesian doubt": "Revoked a signed, locally verified, exact-object-reviewed tip when a later ignored-file audit contradicted its custody claim.",
    "Red Team": "Recomputed every retained receipt, searched profiles for high-confidence sensitive patterns, and proved the referenced files were absent from a fresh worktree.",
    "Systems Thinking": "Joined tracked reference edges, ignored planning state, business-data confidentiality, release signing, outside repositories, and physical backup availability into one wipe boundary.",
    "Operability / Day-2": "Replaced workstation-only dependencies with durable retirement or custody receipts and documented what a fresh session can and cannot reconstruct.",
    "Blast-radius / cell-based": "Changed six continuity/reference records, copied no restricted bytes, and made no mutation to outside repositories or infrastructure.",
    "Telemetry-first": "Bound every retirement and custody decision to path, byte length, digest, exact commit, check count, and required post-operation readback.",
    "Zero-trust / defense-in-depth": "Kept source review, exact C/T authentication, hosted CI, formal approval, merge binding, release closeout, and external custody as independent gates."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "An ignored file can remain a real continuation dependency when a tracked record still names it; age and ignore status do not prove dispensability.",
    "Masked workbook previews are not automatically publishable or de-identified and belong with their source in restricted custody until reviewed.",
    "A remote-safe repository does not make a whole disk safe when unrelated repositories contain dirty, untracked, stashed, worktree-only, or unpublished state."
  ],
  "decisions_changed_or_rejected": [
    "Revoked predecessor tip db7eda3ec2783ca93039b9d03cc2aaade613a927 despite its green source evidence.",
    "Rejected committing workbook-derived profiles merely to make a fresh clone self-contained.",
    "Rejected treating every pre-pivot OMX artifact as runtime churn without auditing tracked reference edges.",
    "Rejected treating Console merge completion as whole-disk wipe authority."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
