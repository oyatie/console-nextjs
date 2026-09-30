> **SUPERSEDED PRE-CONSOLIDATION GOVERNANCE:** This procedure is retained as a
> historical checklist, not current merge authority. It does not implement the
> signed direct-child candidate/authority-tip contract, authenticated synthetic
> merge admission, or the current two-independent-reviewer rule. Use
> [`docs/program/console-development-pipeline.md`](../program/console-development-pipeline.md)
> and
> [`docs/program/agentic-engineering-playbook.md`](../program/agentic-engineering-playbook.md)
> for every current review, CI, and merge decision.

# Review/fix and merge governance gates

Source basis: `docs/specs/autonomous-pr-lifecycle-operating-spec.md`.
Last authored: 2026-07-09T17:00:50Z.

This procedure is the concrete operating checklist for independent review/fix
loops, CI evidence, conflict checks, approval handling, merge eligibility, and
post-merge closeout in this repository. Repository authority is an issue or PR
when one exists, an isolated worktree/branch at its immutable commit SHA, a
review artifact tied to that diff/head, and check evidence tied to that SHA;
external task-system claims and receipts are not authority. It is intentionally stricter
than GitHub branch protection: if GitHub allows an action but this document has
not been satisfied, the worker must not claim the lane is merge-ready or done.

No worker may fabricate review approval, credentials, branch permissions,
production rollout status, release status, or live user-story evidence.

## 1. Stage model and owners

A lane moves through these terminally auditable stages:

1. **Implementation ready for review** — the implementation issue, PR, or
   worktree record has RED or a documented test-after exception, GREEN evidence,
   simplification/test-fill notes, security-hardening notes, changed paths,
   current head SHA, PR number or branch, and required review lenses.
2. **Independent Review/fix active** — a dedicated independent review record,
   tied to the current diff/head, owns the review-required state. Review is never
   hidden as a passive block on the implementation record.
3. **REQUEST_CHANGES fix active** — every blocking review finding has a fix owner
   and rerun plan. Small fixes may remain in the same review record; substantial
   or disjoint-root fixes require a separate repository issue or PR record.
4. **Approval recorded** — approval is backed by an actual PR review state,
   reviewer report, or human/operator statement tied to the current diff/head.
5. **Merge readiness active** — CI/checks, unresolved threads, conflict state,
   branch protection, release gates, dirty-root safety, and human blockers are
   rechecked against the exact head to be merged.
6. **Merged** — the merge commit/SHA and method are recorded, or the merge
   record blocks with the exact unavailable capability.
7. **Post-merge closeout** — target branch containment, post-merge CI/release
   signals, rollout/E2E or N/A, release-governance note, rollback/observability,
   and learning/observation-harvest disposition are recorded.

Roles:

- **Implementer**: owns RED/GREEN/simplify/security evidence and must create or
  link the independent review artifact before completing implementation when review remains.
- **Independent reviewer**: inspects a current diff/head with fresh context and
  records APPROVE, COMMENT, or REQUEST_CHANGES with concrete evidence.
- **Fix owner**: changes only the requested surface, reruns required evidence, and
  returns the repository record to independent review.
- **Merge operator**: checks GitHub/branch-protection state and merges only after
  review, CI, conflict, release, and human-blocker gates are satisfied.
- **Human/operator**: supplies credentials, protected-branch/admin action, legal
  sign-off, production access, live-account proof, or product decisions that no
  agent may infer.

When only one automated reviewer capability is available, independence still requires a
separate review record or session and a review prompt that cites the current diff,
head SHA, evidence, and review lenses. It is weaker than a separate human or
independently provisioned reviewer, so high-risk or protected-branch changes should
block for human review if repo policy or risk demands it.

## 2. Independent review record contract

Every implementation issue, PR, or worktree record that has functional work but
still needs review must create or reuse a repository review record titled
`Review/fix: <source title or PR>`. The review artifact must include these
parser-visible sections before dispatch:

```markdown
Source implementation: issue/PR/worktree `<reference>` at `<sha>`
PR/head: #NNN or branch `name` at `<sha>`
Base/target: `origin/main` at `<sha>` checked at `<timestamp>`
Changed files: `path/a`, `path/b`
Review lenses: correctness, tests, security/privacy, product/UX, accessibility,
architecture, migration/generated-client/release as applicable
Required evidence to inspect: RED proof, GREEN checks, simplification notes,
security notes, CI/checks, user-story/E2E, release gates
Allowed fix scope: exact path prefixes and non-goals
REQUEST_CHANGES routing: fix in this review record if small; create a separate
issue/PR record if scope expands, shared roots move, or a different specialist is needed
Rerun matrix after fixes: focused proof, affected CI/local checks, security lens,
and full release-gate subset listed below
Approval source accepted: PR review id/url, reviewer report artifact, or explicit
human/operator statement tied to the current head
Human-blocked criteria: credentials, protected branch permission, live account,
legal/product decision, production secret, signing key, or unobservable approval
```

The implementation record must link the review artifact and current head SHA.
If review-record creation or linking fails, block the implementation record with
the concrete capability failure; do not complete it as if review happened.

## 3. REQUEST_CHANGES handling

`REQUEST_CHANGES`, `CHANGES_REQUESTED`, a fail-closed reviewer verdict, or a
security/logic finding is blocking until resolved. The worker must:

1. Freeze current state: record PR number, branch, head SHA, base SHA, review id
   or report path, changed files, and which evidence the reviewer inspected.
2. Classify every finding:
   - `valid_blocker`: must be fixed before approval.
   - `already_fixed`: cite the commit/check proving it.
   - `false_positive`: needs reviewer or human acceptance before it can be
     treated as non-blocking.
   - `deferred`: requires an explicit follow-up issue or PR and reviewer/human agreement
     that deferral is safe for this merge.
3. Create or reuse fix work:
   - Small fixes stay in the review record.
   - Larger fixes, shared-root fixes, security fixes, migration/client-generation
     fixes, or specialist work become separate linked issue or PR records.
   - Fix work must not broaden scope beyond the finding without returning to
     plan/spec review.
4. Patch root causes, not just the named line. Check sibling call paths when the
   flaw is a shared validation/authz/SQL/path/generation pattern.
5. Rerun required evidence after every fix batch:
   - the original RED/GREEN proof for the acceptance criterion;
   - tests or checks for each changed path;
   - any previously failing CI job or local equivalent;
   - security-hardening lenses affected by the fix;
   - generated drift or migration checks when source-of-truth or generated files
     move;
   - browser/device/user-story evidence when UI behavior moves;
   - `git diff --check` or an equivalent whitespace/format hygiene check.
6. Request or run independent re-review on the new head. A stale approval on an
   old head is not enough unless the approval source explicitly covers the new
   diff and GitHub still reports it as valid.
7. Record closure: finding id, disposition, fix commit/SHA or patch artifact,
   rerun commands/results, reviewer response, and remaining blockers.

A fix record is not done merely because the code compiles. It is done when the
reviewer-blocking finding is fixed, the affected evidence is green, and the lane
has been returned to review or approved.

## 4. Approval evidence rules

Accepted approval sources:

- GitHub PR review state `APPROVED` from `gh pr view --json reviews` or the PR UI,
  tied to the current PR/head and not superseded by later changes-requested state.
- A durable independent reviewer report/comment that states `APPROVE` or
  equivalent, names the inspected PR/head/diff, lists lenses applied, and includes
  no blocking security or logic findings.
- A human/operator statement granting approval, recorded in the repository PR,
  issue, or review artifact with the
  approver identity, scope, and any constraints.

Not accepted as approval:

- The implementer's self-review.
- Green CI without an approval source.
- Absence of comments, silence, or a closed review thread without reviewer
  disposition.
- A stale review that inspected a different head, unless the reviewer explicitly
  confirms the new diff is covered or GitHub still considers the approval valid.
- A generated summary that says approval exists without a verifiable review id,
  PR state, report artifact, or human quote.
- Admin permission, repo write access, or branch-protection absence.

Approval records must include: approver/reviewer identity or profile, source URL
or artifact, timestamp checked, PR/head SHA, conclusion, unresolved caveats, and
who recorded it. If approval cannot be observed because of missing credentials or
permissions, block the merge/review record instead of fabricating it.

## 5. CI and local evidence gates

CI evidence must be tied to the exact head intended for merge. A worker should
prefer GitHub checks for PR readiness and use local checks to shorten feedback or
cover gates GitHub does not run for the path.

Minimum evidence by class:

| Change class | Required rerun evidence after implementation/fix |
| --- | --- |
| docs/process only | markdown/readability or custom required-term check, link consistency when touched, governance consistency review; product CI is N/A only with rationale. |
| backend/internal | `cargo fmt --all -- --check`, `SQLX_OFFLINE=true cargo clippy --all-targets -- -D warnings`, relevant `cargo test`, and applicable `console-gate-*` gates from `docs/CI-GATES.md`. |
| DB migration/RLS/authz | migration-safety, tenant-isolation/RLS arming, SQLx/offline query cache checks, rollback/forward-only note, and authz/tenant regression tests. |
| OpenAPI/generated clients | source OpenAPI/generator change, regenerated drift checks, `npm run check:ts`, `npm run check:kotlin`, `npm run check:swift`, openapi-app, contract tests as applicable. |
| web/RN UI | lint/type/test/build plus browser or React Native web user-story evidence for the changed flow; API-only tests are not enough. |
| Android/iOS | platform unit/build/behavior evidence, string/i18n parity, device/simulator evidence when user-visible behavior changes. |
| CI/release/deploy | workflow syntax/action pinning review, secret/ref safety, release dry-run where available, image/security/signing gates, rollback/smoke proof. |
| multi-surface | all applicable rows above plus serialized integrator review and conflict check for shared roots. |

When a fix follows `REQUEST_CHANGES`, rerun at least the focused failing proof,
every check whose input changed, and every release/security gate whose invariant
was implicated. For high-risk security fixes, rerun the independent security
review on the new diff even if the code change is small.

CI statuses to record before merge:

- PR number, head SHA, base branch, and `gh pr checks` or check-run source.
- Each required check name and conclusion.
- Any pending/skipped checks with precise N/A or blocker rationale.
- Link to failed logs if blocked.
- Whether path filters intentionally skipped a workflow. A path-filter skip is
  not proof that unrelated release gates passed.

## 6. Conflict and dirty-root checks

Before merge readiness is claimed, run or record equivalent evidence for:

1. `git fetch origin` succeeded.
2. Local branch/head matches the PR head or the PR's remote head SHA is explicitly
   used for the check.
3. The target base (`origin/main` unless the repository record says otherwise) is current.
4. Merge conflict status is clean via GitHub mergeability, `git merge-tree`, a
   throwaway merge, or another recorded conflict check.
5. No unresolved conflict markers remain in tracked files.
6. The worker did not stage, commit, revert, stash, or claim unrelated dirty-root
   changes. In this repo's shared checkout, prefer clean task-owned worktrees for
   product-code mutation.
7. Shared roots are serialized: migrations, OpenAPI/generated clients, workflow
   files, release files, deployment manifests, dependency manifests, and generated
   artifacts require an explicit owner/integrator issue or PR record.
8. If a conflict exists, create or reuse a conflict-fix repository record and rerun review/CI
   after the conflict resolution. A conflict resolution is a code change and can
   invalidate prior approval.

## 7. Security-hardening lenses

Review and fix evidence must apply these lenses proportionally to the changed
surface. Each lens needs a pass note or an N/A reason specific enough for a
reviewer to challenge.

- **Secrets and credentials**: no tokens, signing keys, passwords, OTPs, private
  URLs, or customer secrets committed, logged, printed in CI, or copied into
  issue, PR, review, or check metadata. Production secrets remain operator-injected.
- **Input validation and output encoding**: hostile/malformed payloads, size
  limits, null/missing fields, and user-provided display text fail closed or are
  safely encoded.
- **Authn/authz and tenant/privacy boundaries**: role, branch, org, group, cell,
  PBAC/Cedar, RLS, step-up/passkey, and purpose-bound access are enforced at the
  backend authority point, not only in UI affordances.
- **Audit/compliance**: state-changing actions write audit evidence in the same
  transaction where required; PII/GPS/legal/payroll/location data is minimized,
  masked, retained, or destroyed according to policy.
- **Path traversal and file access**: file paths are normalized/bounded; archive,
  upload, temp-file, and generated-file paths cannot escape their intended roots.
- **Shell/process use**: no shell interpolation of untrusted input; prefer argv
  arrays; commands have bounded cwd/env, failure handling, and no secret echoing.
- **SQL/data safety**: parameterized queries; migrations are append-only for
  audited data; RLS/session GUC paths and tenant isolation are tested when moved;
  concurrent-index and rollback/forward-only risks are documented.
- **Generated files and supply chain**: generated clients/manifests are reproduced
  from source of truth; new dependencies/actions/images are version-checked live,
  pinned where the repo requires, and covered by audit/security scans.
- **Availability/race/consistency**: retries, idempotency, transactional
  boundaries, locks, outbox/notification ordering, and exactly-once or at-least-
  once semantics are reviewed where applicable.
- **Rollback and observability**: failures produce useful logs/metrics/audit/SLO
  signals without PII; rollback/revert/forward-fix posture is stated.

Any security blocker from an independent review remains blocking until fixed,
accepted as false-positive by a reviewer/human, or deferred through an explicit
risk-acceptance/human decision. Agents cannot self-accept security risk.

## 8. Merge eligibility checklist

A PR or branch is merge-eligible only when every applicable item below is true and
recorded on the merge/review record:

- [ ] Implementation issue/PR/worktree evidence is functionally complete and links its review artifact.
- [ ] Independent review record is approved, or a human-only approval blocker is recorded.
- [ ] Every REQUEST_CHANGES finding has a disposition and rerun evidence.
- [ ] Current PR head SHA matches the reviewed/approved head or approval remains
      valid after the last head change.
- [ ] Required CI/checks are green on the current head, or skipped/N/A with
      reviewer-agreed rationale for docs/process-only changes.
- [ ] Security-hardening lenses are recorded with pass/N/A notes.
- [ ] Conflict check against current target branch is clean.
- [ ] No unresolved review threads or untriaged failing comments remain.
- [ ] Branch protection and repository rules are obeyed. If GitHub reports no
      branch protection, this checklist remains mandatory.
- [ ] Merge method matches repo convention, typically squash merge with a
      conventional-commit title when Release Please/changelog automation applies.
- [ ] Release-governance impact is known: Release Please/changelog/tag/image or
      mobile release implications, or explicit no-release-impact note.
- [ ] Rollback/observability plan is recorded.
- [ ] User-facing changes have browser/device/user-story evidence or an explicit
      non-UI N/A rationale.
- [ ] Human-only blockers are absent or the repository record is blocked with exact owner and
      missing action.

Do not merge when:

- CI is pending/failing and no human emergency authorization exists.
- Approval is missing, stale, or unobservable.
- The PR has unresolved `CHANGES_REQUESTED` or unresolved security/logic findings.
- The branch conflicts with the target.
- The merge requires protected-branch/admin privileges the agent lacks.
- A production credential, signing key, legal approval, live test account, or
  customer/operator decision is missing.
- The only evidence is an opened PR, a local green check on a different head, or a
  claimed but unverifiable reviewer summary.

## 9. Protected branch, credentials, and permission blockers

Block instead of guessing when the next action needs:

- GitHub write/merge permission, bypass/admin action, or protected-branch override
  the agent cannot perform under repo rules.
- A review/approval visible only to a human account or private team permission.
- Production secrets, app signing keys, OAuth/API credentials, App Store/Play/OCI,
  Kakao, DNS/TLS, WORM retention-lock, or customer data access.
- KCC/legal/privacy/labor/location approval, operator sign-off, or risk
  acceptance.
- Authenticated live browser/device verification when no safe test credential is
  available.
- A product/UX decision that changes scope, retention, policy, or customer-visible
  workflow.

Blocked repository records must state: blocker kind, exact missing action/evidence, owner,
why agentic fallback is unsafe, safe work still possible, and what event unblocks
the lane.

## 10. Merge execution and evidence

When merge is authorized:

1. Re-fetch and re-check PR head, base, CI, reviews, conflicts, and dirty-root
   scope immediately before merging.
2. Merge via the authorized GitHub path (`gh pr merge` or UI/API equivalent) using
   the repo's selected method. Do not direct-push to `main` or force-push unless a
   human has explicitly authorized an emergency and the merge record records why.
3. Record merge result:
   - PR number and URL;
   - head SHA reviewed;
   - merge commit/SHA or squash commit SHA;
   - merge method;
   - actor/tool used;
   - timestamp checked;
   - branch deletion or retention disposition;
   - any GitHub response or error.
4. If merge fails, block or create fix work for the exact failure: permissions,
   branch protection, stale checks, conflicts, required review, queue required,
   or GitHub outage.

A successful merge does not complete the lane until post-merge closeout is done
or an explicit post-merge child owns the remaining evidence.

## 11. Post-merge closeout checklist

Closeout records must include:

- [ ] Target branch containment: current `origin/main` or release branch contains
      the merge SHA.
- [ ] Post-merge CI/check-run status for the merge/current main when available;
      PR-head checks alone are not current-main rollout proof.
- [ ] Release Please/changelog/release note status, or no-release-impact reason.
- [ ] Image Release, mobile release, deployment, or operator release gates checked
      when their path filters/surfaces apply.
- [ ] User-story E2E/browser/device/API replay evidence for user-facing changes,
      or precise non-UI N/A.
- [ ] Rollback/observability note: revert command/path, migration rollback or
      forward-only posture, logs/metrics/audit/SLO/smoke signal, and operator
      follow-up if production access is needed.
- [ ] Repository evidence settled: implementation worktree/commit, review,
      merge, rollout/E2E, release, and learning records are linked or blocked
      with typed blockers; no stale issue, PR, or branch claims the same lane.
- [ ] Learning/observation-harvest disposition: create an issue, PR, or durable
      evidence record when the lane exposed a reusable process, skill, test, CI,
      or product-governance gap; otherwise record why N/A.

For docs/process-only changes, acceptable closeout may be: target branch
containment N/A until PR merge, product rollout N/A, release impact none, and
learning N/A if this procedure itself is the learning artifact. The N/A reasons
must still be recorded.

## 12. Evidence comment templates

### Review/fix verdict

```markdown
Review/fix verdict for <issue/PR/worktree> at <head_sha>:
- Inspected: <diff/PR/reports/evidence>
- Lenses: <correctness/tests/security/privacy/product/architecture/...>
- Findings:
  - <id>: <APPROVE|COMMENT|REQUEST_CHANGES> — <summary>
- Approval source: <PR review URL/id or reviewer report path>
- Blocking findings remaining: <none/list>
- Required reruns after fixes: <commands/checks/user-story proof>
```

### REQUEST_CHANGES closure

```markdown
REQUEST_CHANGES closure for <finding id>:
- Disposition: <fixed|already_fixed|false_positive accepted by ...|deferred to t_xxx>
- Fix head/SHA: <sha>
- Changed paths: <paths>
- Rerun evidence: <commands/results/CI URLs>
- Security lenses rerun: <lenses>
- Re-review source: <review id/report/human statement>
```

### Merge readiness

```markdown
Merge readiness for PR #<n> at <head_sha> -> <base_sha>:
- Reviews: <APPROVED source; unresolved threads count; no fabricated approval>
- CI/checks: <green/pending/failing/N/A with URLs>
- Conflicts: <clean evidence>
- Branch protection/repo rules: <observed status; no bypass>
- Release-governance: <Release Please/changelog/image/mobile/no-impact>
- Rollback/observability: <note>
- Human blockers: <none/list>
- Decision: <merge-ready|blocked with reason>
```

### Post-merge closeout

```markdown
Post-merge closeout for PR #<n>:
- Merge SHA: <sha>; target branch contains it: <evidence>
- Current-main/post-merge checks: <status/URLs/N/A>
- Rollout/E2E: <browser/device/API/user-story proof or N/A>
- Release-governance: <Release Please/changelog/tag/image/mobile/no-impact>
- Rollback/observability: <revert/forward-fix and telemetry/audit/smoke>
- Repository evidence settled: <issues/PRs/worktrees/reviews/checks linked or blocked>
- Learning/observation harvest: <issue/PR/evidence reference or N/A>
```

## 13. Completion metadata keys

Use namespaced metadata so automation does not mistake read-only audits for code
edits:

```json
{
  "repo_changes_by_this_task": ["path/changed/by/this/task"],
  "observed_pr_files": ["path/seen/in/pr"],
  "review_fix_child": "t_xxx",
  "fix_children": ["t_xxx"],
  "review_lenses": ["correctness", "security/privacy"],
  "approval_sources": [{"type": "pr_review", "url": "...", "head_sha": "..."}],
  "ci_evidence": [{"check": "CI", "head_sha": "...", "conclusion": "success"}],
  "conflict_check": "clean against origin/main at ...",
  "merge_sha": "...",
  "post_merge_evidence": "current-main checks/rollout/E2E or N/A",
  "release_governance": "Release Please/changelog/image/mobile/no-impact",
  "rollback_observability": "revert/forward-fix + logs/metrics/audit note",
  "human_blockers": [],
  "learning_records": []
}
```

Never place secrets, tokens, raw PII, customer payloads, or unverifiable approval
claims in completion metadata.

## 14. Fast fail-closed decision table

| Situation | Required action |
| --- | --- |
| Reviewer says `REQUEST_CHANGES` | Keep Review/fix active; create/reuse fix work; rerun affected evidence; re-review. |
| CI fails on current head | Diagnose logs; fix or block with exact failing check; do not merge. |
| CI green on old head only | Rerun or wait for checks on current head; old-head green is not merge evidence. |
| Approval on old head after new fix | Treat stale until GitHub/reviewer/human confirms validity for new head. |
| Conflict with `origin/main` | Create/reuse conflict-fix work; rerun review/CI after resolution. |
| Missing merge permission/protected branch | Block with capability/needs_input; do not bypass with direct push. |
| Missing production/live credential | Block with owner/action; use local/E2E only if it is an honest N/A substitute. |
| User-facing UI change has API-only proof | Keep rollout/E2E incomplete; create browser/device evidence issue/PR record or block. |
| Docs/process-only change | Product CI, rollout, and release may be N/A only with explicit rationale and governance review. |
| Security finding accepted as risk | Requires human/operator or reviewer acceptance; agent self-acceptance is invalid. |
