> **HISTORICAL after the 2026-07-28 pivot.** This document preserves evidence and prior reasoning only. It cannot authorize Buck2, broad product scope, fan-out, or implementation work. Current product authority is [`docs/current/PRODUCT.md`](../current/PRODUCT.md), [`ROADMAP.md`](../current/ROADMAP.md), and [`DELIVERY.md`](../current/DELIVERY.md). Pivot history only: [`docs/PIVOT-2026-07-28.md`](../PIVOT-2026-07-28.md) (non-authority).

# Console hyperscale fan-out epoch contract

**Status:** enforceable planning/admission contract. Buck2 remains the only
build and impact graph authority. This planner owns only immutable source
admission, writable-root collision avoidance, capacity admission, independent
review, and serialization of shared faces.

## Bind before dispatch

A receipt is bound across immutable commits. The CLI reads the
registry and generated-face authority **from that commit**, requires its caller
worktree to be clean, requires `source_revision` to be a resolvable
`<ref>@<40-sha>` immutable provenance commit that is an ancestor of the anchor
(and whose locally resolvable ref is not behind it), and records SHA-256
authority digests. This avoids self-referential candidate-commit metadata while
still binding the actual registry and generated-face blobs to the anchor. Missing, empty, or
schema-incompatible generated-face authority fails closed.

Every declared ownership root is a repository-relative literal or a literal
with one terminal `/**`. Dot aliases, `..`, absolute paths, and wildcard forms
such as `foo*`, `**/file`, or brace/character globs are rejected. A generated
output pattern which cannot be represented by that algebra is conservatively
widened to its literal prefix subtree; this can reduce leaf concurrency but can
never create an unsound intersection result.

Every lane root must be wholly covered by exactly one capability-owned private
root. A lane may not widen ownership, and any intersection with a shared,
generated, or migration root is a hard hold before completed-source shortcuts
are considered. A reviewed leaf is single-parent and may change only paths
covered by those validated private roots; its direct single-parent review commit
may change only the canonical receipt artifact.

The migration authority is exactly
`backend/crates/platform/db/migrations/**`; it must exist in the anchor tree
and is excluded from all leaf writable roots.

## Safe parallelism

A source lane is admissible only with:

- unique owner, exact worktree, exact branch, signature story, evidence path,
  executable leaf gates, and required Buck targets;
- clean declared worktree discovered through `git worktree list --porcelain`,
  whose branch matches the declaration and whose HEAD is the anchor or an
  immutable descendant;
- one explicit integer resource declaration for `writer`, `postgres`,
  `browser`, `ios`, `graph`, and `cas`; and
- disjoint private roots.

The deterministic quality-weighted maximal independent set admits source writers
only on disjoint private roots and the writer limit. Source leaves run cheap
checks and may fan out; expensive `rust_compile`, graph, CAS, Postgres, browser,
and iOS capacity never reduces that safe source admission. Dependencies are merge
holds, not an excuse to block an otherwise safe leaf implementation.

Expensive verification is a separate exact-SHA queue. Compatible reviewed leaves
are grouped by exact SHA/cache affinity and run through one canonical local Buck
daemon with a combined target set; vector capacities schedule those jobs in
deterministic density order. This planner does not claim no-starvation until a
separate immutable cross-epoch enqueue-age authority is introduced. The
local cold-Rust authority is at most two concurrent jobs, each `-j 6`. Only
incompatible state receives an isolation directory: Buck isolated daemons do not
share local cache. This contract makes no remote-cache or remote-execution claim.

Each selected lane receives a deterministic, epoch-scoped Buck isolation path:

```text
.buck2/console-epochs/<anchor-12>/<lane-slug>-<sha256(full-lane-id)>
```

Run Buck through `tools/buck2 --isolation-dir <that path> ...` only for an
incompatible state. Compatible exact-SHA verification belongs on the canonical
local daemon/train. This changes no Buck cells or build graph ownership.

## Review and consolidation

The immutable `epoch_base_sha` contains lane definitions and trusted reviewer
authority. A later immutable `admission_sha` contains only the canonical
`docs/evidence/console/fanout-admission.json` manifest, which references each
review commit and its canonical receipt path
`docs/evidence/console/fanout-receipts/<sha256(lane-id)>.json`. The chain is
strictly `epoch base -> leaf -> review -> admission`. The review commit must be
the direct child of the leaf and may change only its receipt artifact. Its
receipt's result digest is recomputed from the fixed Git binary diff
`git diff --no-ext-diff --no-renames --full-index --binary <base> <leaf>`.
Reviewer identity is accepted only when the review commit author/committer and
verified signing fingerprint match trusted epoch-base authority. Reviewer IDs
and canonical uppercase signing fingerprints are each unique in that authority;
each authority row declares exact non-empty author and committer names/emails.
The authority carries a format-discriminated signing declaration. For `gpg`, the
planner invokes `git verify-commit --raw` and accepts exactly one machine-readable
`[GNUPG:] VALIDSIG <fingerprint>` record. For `ssh`, it accepts exactly one raw
Git verification record with the declared principal and full `SHA256:` key
fingerprint. In either format it rejects unsigned commits, key/principal/
fingerprint mismatches, malformed or duplicate status records, and unavailable
verification tooling; it never treats author text or a fingerprint substring as
signature proof. SSH verification materializes the canonical
`.github/trust/console.allowed_signers` policy from the immutable product
candidate Git tree into a mode-0600 temporary file and invokes Git with explicit
`gpg.format=ssh`, `gpg.ssh.program=ssh-keygen`, and `gpg.ssh.allowedSignersFile` overrides. HOME and global Git
configuration are not trusted; Git verifies the `git` signature namespace and the
one policy entry must match the declared principal and full fingerprint. Later authority commits therefore
cannot change candidate or review trust. If signer infrastructure is unavailable, receipts
fail closed: source planning can continue, but completion and consolidation
remain held.

The admission commit is a single-parent commit which changes only
`docs/evidence/console/fanout-admission.json`. Every manifest reference has a
unique lane, review commit, and canonical receipt path, and every referenced
review commit must be an ancestor of that admission commit. Merge admissions,
unrelated admission changes, duplicate references, and disconnected review
commits are rejected. Receipt content equality is compared through canonical
JSON bytes/digests, rather than JavaScript object identity, so separately parsed
identical immutable receipt content remains valid.

All immutable JSON (registry, generated authority, receipts, admission manifest,
and immutable evidence) is parsed only after rejecting duplicate object keys;
the planner retains raw and canonical SHA-256 digests for that parsed content.
The complete closure is a serialized rebase/cherry-pick admission train:
`epoch → authorized leaf → direct receipt-only review → … → manifest-only
admission`. No merge, unreviewed intermediate, unrelated commit, or divergent
leaf branch is admitted.

`fanout_epoch.normalized_lane_ids` is optional epoch-base authority. When it is
present, every lane absent from that explicit list is a legacy hold; it is not
silently treated as ready. The initial live bootstrap contains the local SSH
reviewer authority but deliberately normalizes no lane, because no current lane
has all required live worktree, ownership, resource, and leaf-gate evidence.

Shared OpenAPI, generated clients, migrations, route/nav, tokens, and generated
Buck faces are never leaf writes. A consolidation entry remains false until an
exact-anchor independent-review receipt is approved for every leaf and the
consolidation owner, worktree, branch, and resource declaration are valid. The
planner discovers the consolidation worktree through `git worktree list
--porcelain`, verifies its exact branch, cleanliness, anchor-descendant HEAD,
and that its resource demand fits the epoch capacity before it can be ready.
Each leaf receipt names its exact lane and implementer, a distinct reviewer, the
reviewed leaf commit, and the SHA-256 leaf-result digest. The admission manifest
binds that immutable receipt artifact to its review commit, avoiding an
impossible self-referential review-commit hash inside the signed receipt blob.
Review fans out per leaf;
rejections return only to the leaf owner. The consolidated exact tip then runs
cheap admission, affected Buck targets, resource-class shards, browser stories,
and the full backstop.

The receipt is planning evidence, not a completion claim. A capability is done
only after its roadmap stories, real backend wiring, independent review,
shared-face consolidation, and exact-tip verification all pass.

The exported in-memory planner never accepts caller-provided receipt objects as
completion authority. Only the CLI runtime may mint its private attestation
after complete immutable Git receipt, path, ancestry, identity, and signature
validation succeeds. Selected source lanes create no expensive verification jobs.
Verification jobs arise only from runtime-validated review attestations and use
the actual reviewed `leaf_commit` SHA for grouping/cache affinity (or a
separately verified consolidated tip), never the epoch anchor SHA.
