# Release 0.3.2 candidate authority

## Exact identity

- Protected pre-release base B:
  `4f58ecf138fdc1e829fbe520a45d2ede2de7d837`
- Release Please generated commit G:
  `28c949306e1272b668df3086cf04597679c3256c`
- Signed release candidate C:
  `0075bba955410f7e9b8b30efd4efc5159a6643f2`
- Final authority tip T: the signed direct child of C that adds only this file

G changes exactly `.release-please-manifest.json` and `CHANGELOG.md` relative to
B. It advances the manifest from `0.3.1` to `0.3.2` and records the merged
post-pivot consolidation as the sole 0.3.2 bug fix. C is a signed direct child
of G that changes only the canonical disk-wipe handoff to record the owner's
solo-repository review-policy correction. No generated release byte was
rewritten.

## Release boundary

This record binds the generated 0.3.2 metadata to signed C. The release remains
candidate-only until exact-tip verification, strict hosted checks, independent
exact-object review, squash merge, post-merge squash binding, tag and GitHub
Release creation, and final remote readback all pass.

Capability, jurisdiction, Korea-control, production-exposure, legal, deployment,
and product-readiness conclusions remain unchanged and `HOLD`. The changelog is
release history, not evidence that every named capability is complete or live.
Image publication follows successful exact-main CI independently; production
promotion remains a separate manual, false-by-default operation and is not
authorized here.

The owner corrected the repository's merge policy after #562 was sealed: this
is a solo-development repository, so historical instructions requiring one
non-author GitHub approval and approval by someone other than the last pusher
are superseded. Current `main` protection requires zero GitHub approvals and
sets `require_last_push_approval` to false. This changes only who may satisfy a
GitHub review rule; it does not waive review or verification. Two independent
exact-object reviews, stale-review dismissal, conversation resolution, and all
16 strict app-bound required contexts remain candidate admission requirements.
The live protection and merge settings must be read back immediately before
merge.

The external repository scorecard remains opinion only. This release does not
alter the product boundary or authorize disk erase. Restricted workbook inputs,
signing identity, infrastructure recovery material, business inputs, and local-
only state outside Console still require verified external custody or explicit
owner disposition under the disk-wipe handoff.

## Verification before T

- B is current `origin/main`, and G is the current head of release PR #563.
- B..G changes exactly the release manifest and changelog; the manifest reads
  `0.3.2`, and the new changelog section contains only the #562 consolidation
  bug-fix entry.
- C is a direct child of G, changes only
  `docs/handoffs/2026-08-03-disk-wipe-consolidation.md`, and verifies against the
  repository-pinned ED25519 signer fingerprint
  `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`.
- PR #562 squash commit B passed the post-merge authority binder before this
  release candidate was sealed.

Before merge, T must remain C's signed single-parent direct child with a one-file
regular ledger-only diff. Protected-main simulation, the complete exact-T local
verifier, independent exact-object review, and all 16 strict app-bound GitHub
contexts must pass on T. After merge, the squash binder, Release Please run,
lightweight `v0.3.2` tag, GitHub Release, tagged label, release-branch deletion,
single-main remote-head state, and zero-open-PR state must all be read back.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "approval",
    "release",
    "production"
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
    "Cartesian doubt": "Treats generated version and changelog bytes as a candidate rather than assuming bot authorship or a green predecessor proves release authority.",
    "Red Team": "Requires the generated diff, signed handoff correction, ledger-only tip, squash tree, tag target, and GitHub Release target to agree exactly.",
    "Systems Thinking": "Separates source release, image publication, production promotion, repository closeout, and workstation custody into independent state transitions.",
    "Operability / Day-2": "Defines the exact post-merge tag, release, label, branch, head, and open-PR readbacks required before declaring repository closeout.",
    "Blast-radius / cell-based": "Keeps the bot-generated two-file release change intact and adds only the canonical handoff correction in signed C plus one authority ledger in T.",
    "Telemetry-first": "Binds every release stage to exact commit identities, tree equality, file inventory, workflow contexts, and final remote observations.",
    "Zero-trust / defense-in-depth": "Keeps local verification, independent review, hosted CI, squash binding, Release Please, and final remote readback as separate gates."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "A generated release commit still needs the repository's signed candidate and ledger-only authority topology.",
    "A version tag is not repository closeout unless its commit, GitHub Release, PR label, branch deletion, and open-PR state agree.",
    "A source release does not authorize production promotion or workstation erasure."
  ],
  "decisions_changed_or_rejected": [
    "Rejected merging the unsigned bot head directly even though its two generated files are correct.",
    "Rejected rewriting generated release bytes merely to add a signature; kept them intact while signed C corrects the canonical handoff.",
    "Rejected treating release metadata as capability, deployment, production, or wipe authority."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
