# Release 0.3.3 candidate authority

## Exact identity

- Protected pre-release base B: current `origin/main` at seal time
- Release Please generated delta G: `.release-please-manifest.json` `0.3.2`→`0.3.3` and CHANGELOG section for 0.3.3
- Signed release candidate C: this commit's parent chain includes G bytes
- Final authority tip T: signed direct child of C that adds only this file's Authority tip footer

G changes exactly `.release-please-manifest.json` and `CHANGELOG.md`. It advances
the manifest from `0.3.2` to `0.3.3` and records #579 request_no fixture uniqueness
as the 0.3.3 bug fix. No generated release byte was rewritten beyond restack onto
current main after tip-serial drain of #591/#592/#593 (restack onto post-#593 main).

## Release boundary

This record binds the generated 0.3.3 metadata to signed C. The release remains
candidate-only until exact-tip verification, Required CI/Security, independent
review, squash merge, post-merge squash binding, tag and GitHub Release creation,
and final remote readback all pass.

Capability, jurisdiction, Korea-control, production-exposure, legal, deployment,
and product-readiness conclusions remain unchanged and `HOLD`. The changelog is
release history, not evidence that every named capability is complete or live.
Image publication follows successful exact-main CI independently; production
promotion remains a separate manual, false-by-default operation and is not
authorized here.

## Verification before T

- B is current `origin/main` after tip-serial merges.
- B..C includes exactly the release manifest and changelog plus this ledger and
  documentation-manifest prebind.
- Manifest reads `0.3.3`; changelog section names only the #579 bug fix.
- Before merge, T must remain C's signed single-parent direct child with a
  one-file regular ledger-only diff.

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
    "Cartesian doubt": "Treats bot-generated version and changelog as candidate bytes requiring signed C/T.",
    "Red Team": "No production promotion; HOLD preserved.",
    "Systems Thinking": "Release source separate from image publish and prod promote.",
    "Operability / Day-2": "Post-merge tag/release readbacks required.",
    "Blast-radius / cell-based": "Two release files + one ledger only.",
    "Telemetry-first": "Exact SHAs and manifest 0.3.3.",
    "Zero-trust / defense-in-depth": "Pinned SSH C/T train required."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Release-please PR #584 was BEHIND and unsigned after tip-serial drain.",
    "Authority bootstrap rejects unsigned bot head; convert G into signed C/T train."
  ],
  "decisions_changed_or_rejected": [],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
