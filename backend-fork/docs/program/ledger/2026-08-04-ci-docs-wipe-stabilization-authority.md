# CI, documentation, and wipe-readiness stabilization authority

## Exact candidate

- Protected base B:
  `9497bd6169c2efdfef64d228294264ed1b16bf1a`
- Signed content candidate C:
  `7eeeb2145a432ce37abc2b3804d6f2b2c6fb862f`
- Final authority tip T: the signed direct child of C that adds only this file

C consolidates two independently developed slices into one stabilization train.
It adds same-workflow shadow aggregates for the existing ten CI and five
Security leaf proofs, establishes README as the sole entry point to exactly
three current authorities, fails closed on documentation custody from an exact
Git index tree, and updates the disk-wipe evidence and itemized disposition
gate. It changes no product code, migrations, dependencies, generated API,
runtime deployment, production resource, or secret value.

## CI decision

`Required / CI` depends on the exact ten existing CI jobs and
`Required / Security` on the exact five existing Security jobs. Each aggregate
runs after its dependencies and accepts only a `success` result for every leaf;
a failed, cancelled, or skipped dependency therefore fails the aggregate. The
aggregate jobs do not check out candidate content. Their job identities, needs,
conditions, runner envelope, timeout, and executable comparison are locked by
mutation-tested repository validators.

The two aggregate contexts are shadow evidence only in C/T. Current branch
protection still requires sixteen GitHub-Actions-app-bound contexts: the ten CI
leaves, five Security leaves, and the independent
`authenticate-console-authority` protected-target proof. Protection may move
to `Required / CI`, `Required / Security`, and
`authenticate-console-authority` only after both new contexts pass on this
exact pull-request tip and the resulting exact `main` commit. The migration
must preserve strict up-to-date checking, app binding, stale-review dismissal,
admin enforcement, conversation resolution, and the force-push/deletion
prohibitions, then be read back immediately.

The serialized disposable-PostgreSQL job remains useful but not optimal. It
executes the exact 183-test database/RLS/migration/REST/domain inventory and was
observed at 43 minutes on the released base. Only three tests currently need
cluster-global role serialization. Issue #564 remains open for isolated shards,
an exact union/no-duplicate/no-omission proof, and a compatibility aggregate;
this train does not relabel or weaken the current required context.

## Documentation and issue decision

README is the sole onboarding entry and points to PRODUCT, ROADMAP, and DELIVERY
as the exact three current authorities. Historical specifications, pivots,
program files, evidence, and handoffs retain path-stable context but cannot
override or dispatch current work. The machine index deliberately claims only
`authority-slice`; every premature `complete` claim fails until cross-record
semantics and signed archive validation are implemented. The link/index gate
reads immutable blobs from a sanitized exact Git index tree, rejects untracked
or ignored authority, symlinks, submodules, hostile alternate-index
environments, missing tracked targets, unknown schema fields, and drift from the
exact ordered three-authority README declaration. Issue #565 remains open for
the full first-party document classification and any later bulk moves.

Issue hygiene follows outcome evidence rather than age. #439 was closed only
after exact merged proof; #273's stale `main`-is-unprotected guidance was
corrected and read back without closing its remaining work; #354 received an
overlap/evidence comment but stayed open because it retains unique acceptance
criteria. #564 and #565 remain open because this train is partial against their
complete acceptance criteria.

## Wipe and recovery boundary

The itemized ledger is complete enough to make the current verdict explicit:
**NO-GO for disk erase**. It has ten P0 rows: nine remain `PENDING`; the
greenfield Talos/PostgreSQL/registry/CAS/monitor/OpenBao/KMS data row records
`DATA DISCARD ACCEPTED; ROTATION PENDING`. There is no proven writable
off-device destination or recovery read-back. The exact Console candidate,
signing identity, Keychain/account/passkey/2FA recovery, OCI/Talos material,
both ignored secret files, restricted business inputs, personal/TCC-protected
home data, unpublished state in other repositories, and curated global
agent/session evidence remain preservation-or-explicit-disposition work.

The two ignored local secret files were tightened to mode `0600`; no secret
bytes enter Git. The four active OMX continuity artifacts remain tracked and
hash-bound. Retired Console runtime/session material and explicitly rejected
lanes remain discardable and must not regain authority. The grandfathered OCI
Ampere A1 instance (4 OCPU / 24 GB) must never be destroyed, terminated,
resized, or reprovisioned. No source merge authorizes Talos/cloud mutation,
production promotion, credential reset, or disk erase.

## Verification before T

- Exact base read-back: `HEAD == origin/main == B`; the remote exposed only
  `main`, and no pull request was open before this train.
- Documentation link regressions passed 25/25; the exact staged-tree gate passed
  380 first-party Markdown files, including hostile Git-environment, schema,
  README-authority, and symlink probes.
- CI preflight passed 52/52 plus its live gate; Security hardening passed 36/36
  plus its live gate; local verifier classification passed 13/13.
- Foundation regressions passed 6/6 and its live gate passed 134 checks; ADR
  governance passed 29/29 with 38 ADRs and four design notes.
- Protected-authority bootstrap regressions passed 17/17; reasoning-lens
  regressions passed 40/40 plus the live contract.
- Executed-test reachability reported 335 defined binaries, 325 reachable, and
  the unchanged ten explicitly named dark binaries.
- `actionlint`, Node syntax checks, citation gates, `git diff --check`, staged
  regular-mode inspection, and credential-shaped-addition scanning passed.
- The content candidate changed exactly 28 tracked paths. No `.env`, workbook,
  node_modules, OMX runtime, build output, credential, private-key, or secret
  path entered C. `AGENTS.md` is a tracked candidate blob even though a
  non-authoritative local `.git/info/exclude` rule also names it.
- Pre-seal CI-slice and documentation/custody reviews ended with zero Critical
  or Important findings. The first exact-T review of the superseded tip
  `d7fd24875d248fb91300d0a8cee3cae0c004f58a` then found two Important
  documentation false greens; the re-review of superseded tip
  `06910a9d17c74b44e872a0557ea5e852607b67fe` found one remaining prose-form
  README-authority bypass. All three mutations were reproduced red, fixed, and
  locked by the 25-test suite before C. A separate verifier found the live #273
  contradiction; the issue body was corrected and read back before C. None of
  these corrections substitutes for the fresh exact-T reviews still required
  below.

These focused results authorize construction and exact-object review of T, not
merge. T must remain C's signed direct one-file child. Protected-main simulation,
the complete clean-tree verifier, two independent exact-T reviews, every hosted
required context, protection read-back, squash tree binding, exact-main CI, and
final zero-open-PR/single-main read-back remain mandatory.

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
    "production",
    "other"
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
    "Cartesian doubt": "Treats the external scorecard, broad worktree counts, greenfield statement, and green local gates as claims to reproduce and classify rather than automatic preservation, deletion, or merge authority.",
    "Red Team": "Tests skipped dependencies, hostile Git environment overrides, symlink custody, false complete-manifest claims, secret-path admission, stale issue closure, and a wipe performed before recovery proof.",
    "Systems Thinking": "Connects Git authority, CI context topology, documentation precedence, issue lifecycle, workstation custody, cloud recovery, and fresh-session continuity without making one boundary imply another.",
    "Operability / Day-2": "Leaves a single entry point, exact verification command, tracked execution handoff, itemized preservation/disposition ledger, branch-protection migration sequence, and post-merge readbacks.",
    "Blast-radius / cell-based": "Keeps the train to CI aggregation, documentation authority, and custody evidence; it makes no product-code, migration, dependency, production, or cloud mutation.",
    "Telemetry-first": "Binds decisions to exact B/C/T identities, job/result sets, test counts, worktree and custody inventories, issue URLs, branch-protection fields, and required fresh-clone evidence.",
    "Zero-trust / defense-in-depth": "Separates signed candidate custody, independent review, hosted checks, protected-target authentication, squash binding, secret custody, credential rotation, and disk-erasure authority."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Stable same-workflow aggregates improve branch-protection maintainability, but they are safe replacements only after exact pull-request and exact-main shadow proof.",
    "The disposable PostgreSQL job is valuable coverage but its handwritten 183-test serialized inventory is a follow-up optimization, not a reason to weaken it during stabilization.",
    "Documentation consolidation is trustworthy only when authority bytes come from an exact sanitized Git tree and incomplete full-manifest semantics fail closed.",
    "A greenfield data-discard decision does not preserve signing identity, account recovery, unpublished repositories, personal data, or credential rotation, so the workstation remains NO-GO."
  ],
  "decisions_changed_or_rejected": [
    "Rejected a cross-workflow global aggregate because native needs edges do not cross workflow boundaries.",
    "Rejected migrating branch protection before both shadow aggregates pass on the exact pull request and exact main commit.",
    "Rejected asserting complete documentation coverage before cross-record and signed-archive semantics exist.",
    "Rejected bulk issue closure, age-based closure, and closing partial #564/#565 scope.",
    "Rejected preserving every stale worktree or global agent session as useful authority; preserved only classified outcomes and left unresolved repositories and recovery stores itemized.",
    "Rejected treating greenfield cluster data or a merged Console PR as permission to erase the workstation or mutate the grandfathered OCI instance."
  ],
  "lens_set_changes": [
    "Added Systems Thinking because the wipe decision depends on interactions among repository convergence, CI admission, documentation authority, issue state, credential recovery, and off-device custody."
  ]
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
