# Console delivery authority

Status: active delivery authority. Product and roadmap decisions remain in [`PRODUCT.md`](PRODUCT.md) and [`ROADMAP.md`](ROADMAP.md).

## Admission and ownership

Start from an exact immutable base in a clean bounded worktree. A lane is a named command that is currently red on that base; occupancy of a path, a merge-tree-clean slice, or a worktree is not a lane. Start implementers only through `tools/lanes/fanout.py`: missing, green, or non-executable probe is stop, and the same probe must be green before the lane is success. Record the outcome and non-goals, owner, allowed and forbidden paths, source-of-truth writer, shared-resource leases, pre-mortem, detection, rollback and stop conditions, verification baseline, reviewers, evidence, candidate SHA, result, and remaining HOLDs. Serialize migrations, lockfiles, generated contracts, CI, and authority records.

Never import ignored or untracked artifacts, local runtime state, workbooks, secrets, or custody material merely because they exist on a developer machine. Establish custody from the exact candidate Git tree and regular-blob identities; path membership alone is insufficient.

## Candidate, review, and merge

Evidence and reviews bind to an exact candidate SHA. High-risk authorization, migration, contract, approval, HR/payroll, release, production, and compliance-sensitive work requires independent adversarial review appropriate to the risk. CI is evidence, not a substitute for review, legal authority, release authority, or production authority.

`dev` is the canonical integration and source-release branch and the intended repository default. Feature, authority, and release-candidate work enters through its protected pull-request and merge-group path. Ordered environment mirrors after `dev` are `staging`, `canary`, and `production`: their names, tips, or rulesets cannot authorize product work, source release, live promotion, or exposure. Creating or fast-forwarding those names is not production exposure. Changing this branch model again requires a separately reviewed current-authority candidate and exact readback of every affected required check and protection. Until GitHub default-branch readback shows `dev` and `origin/main` is gone, a candidate may still enter through the currently protected default (`main`) merge queue so this record can land; that path is a transition, not a second integration branch.

A required status context must have an executable protected workflow producer for every branch where it is enforced. Configure and verify the producer before enabling the requirement; a context inherited from an unrelated commit or branch is not evidence. Required checks `authenticate-console-authority`, `Required / CI`, and `Required / Security` must have producers on `dev` before they are enforced there. CI, Security, Nightly, CAS seed, bootstrap `assertLivePullRequestSnapshot`, and release-please key on `dev`. GitHub default-branch readback and deletion of `origin/main` remain a later API step. Until separately authorized, environment-branch promotion (live DNS, TLS, or production exposure) remains fail-closed and on HOLD. Argo `targetRevision: main` is unchanged.

Merge only the reviewed candidate through the protected integration path (`dev` once it is the hosted default; `main` only while it remains the hosted default during this switch). After merge, read back the hosted commit and required checks. A local commit, branch, pull request, or green local run is unpublished evidence and is not proof that work is merged or released.

## High-risk program method

Tenancy, authorization, migration, contract, HR, payroll, frontend shipping, and Intelligence-seam work uses this sequence. Authority and design records are orchestrator-owned; they are not `fanout.py` lanes.

1. Four independent design-review rounds, consensus, and a revision-bound explicit approval (design SHA).
2. Tests written first against that design SHA, independently reviewed, and explicitly approved (test SHA). Semantic test changes return to test review.
3. Implementation only from the exact approved test commit. Material design change returns to design review.
4. After implementation: coverage, security hardening, simplification and refactor, with review-fix until approve.
5. A 16-lens audit (the task-selected reasoning lenses in [`AGENTS.md`](../../AGENTS.md)) before the merge verdict. High-risk authz, migration, HR, and payroll include Red Team, Operability / Day-2, Blast-radius / cell-based, and Zero-trust / defense-in-depth.
6. Independent COMMENT on the candidate SHA, then merge only through the protected integration path above. `fanout.py` still admits implementation lanes: a named command that is red on the clean tree.

This method does not authorize live promotion, DNS, TLS, secrets, exposure, payment execution, or cloning Intelligence.

## PR #862 one-time containment

PR #862 merged candidate `c61949c6c57d56755a49722d82f0cec2a471680f` as `ec866c1b0450829bc6b776be570e13f6a18edbd6` with tree `2b5c870eb9494def1d64f024f51c512cf694df0b` at 2026-08-24T08:46:05Z before any independent review; GitHub recorded zero submitted reviews. That merge violated the review-before-merge rule above. This record does not waive, excuse, or retroactively satisfy it.

One bounded prospective containment of that exact tree is admitted: release, promotion, and dependent authority work remains stopped until a candidate based on `ec866c1b0450829bc6b776be570e13f6a18edbd6`, changing only this receipt and the two generated documentation-manifest projections, receives independent adversarial review of the full inherited current-authority tree before merge, enters through protected `main`, and has its hosted tip and required checks read back. Current authority resumes prospectively at that new reviewed tip only. Any material content finding requires a normal reviewed correction or revert. This paragraph is not precedent or a reusable review exception and authorizes no release, environment promotion, production action, or HOLD clearance.

## Verification method

Run the smallest targeted regression first, then the applicable format, lint/type, contract, security, and domain gates. A new clone must install pinned Node tooling and put the pinned DotSlash runtime on `PATH` before the supported repository entrypoint. The installer cannot modify its parent shell (`GITHUB_PATH` exists only in GitHub Actions).

```sh
npm ci
tools/buck/install_dotslash.sh
export PATH="${CONSOLE_DOTSLASH_BIN_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}/console-dotslash}/bin}:$PATH"
npm run verify
```

This is the same sequence as [`README.md`](../../README.md). Buck2-backed steps fail as environment errors, not product regressions, if DotSlash is missing from `PATH`.

For documentation-authority changes, also run the doc-link tests and gate, ADR tests and gate, citation checks, foundation tests and gate, CI-preflight tests and gate, verifier tests, `npm run verify`, and `git diff --check`. Inspect the exact changed-path allowlist and ignored/untracked state before signing a candidate.

Record exact commands, revision, toolchain/environment, discovered and executed counts, failures, artifact hashes where relevant, and validation gaps. A ran-nothing result, stale SHA, superseded candidate, or omitted required surface is not green evidence.

## Issue lifecycle policy

An issue closes only when one of these facts is recorded:

1. the requested outcome is merged, and release is recorded when the issue requires release; or
2. the issue is explicitly identified as a duplicate or is explicitly superseded by another tracked outcome.

Keep the issue open when work is partial, unpublished, on a local branch or unmerged pull request, blocked by a HOLD, awaiting security review or remediation, awaiting recovery/readback evidence, or represented only by an ambiguous roadmap statement. A commit, passing local tests, an implementation claim, or a planned follow-up does not by itself satisfy closure.

## Non-authority

Historical plans, evidence, branches, chats, handoffs, and transient runtime state may explain or support delivery facts. They never replace current authority or exact-candidate proof. Document classes (`current`, `decision`, `executable-contract`, `evidence`, `historical`, `quarry`) describe custody, not permission to ship aspirations.
