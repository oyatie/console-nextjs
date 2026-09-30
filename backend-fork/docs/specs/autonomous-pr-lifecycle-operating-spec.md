> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Autonomous PR Lifecycle Operating Spec

Source directive: Kanban `t_5db03578` / operating-spec card `t_bd85ccd9`.
Last authored: 2026-07-09T16:44:37Z.

This spec defines how the `maintenance` board turns research, planning, code,
review, merge, and post-merge evidence into a complete autonomous PR lifecycle.
It is a board/process contract, not a product feature spec. Product work remains
bounded by the repo guardrails in `HANDOFF.md`, `DESIGN.md`, `docs/CI-GATES.md`,
and `docs/GO-LIVE-CHECKLIST.md`.

No bypass of review, CI, branch protection, conflict checks, release governance,
or post-merge evidence is allowed.

The concrete step-by-step procedure for review/fix, approval evidence,
REQUEST_CHANGES routing, merge readiness, post-merge closeout, and proportional
security lenses lives in
[`review-fix-merge-governance.md`](review-fix-merge-governance.md). This file
defines the lifecycle contract; the governance runbook defines the operational
checklists workers execute at review and merge time.

## Non-negotiable definition of done

An implementation lane is not done when a branch exists, a test passes once, or a
PR is opened. It is done only when the board records all applicable evidence:

1. Research/plan/spec were completed or explicitly scoped as N/A.
2. The card body contains parser-visible owner, worktree, branch, scope,
   dependency, path/conflict, non-goal, acceptance, review, release, rollback,
   observability, and human-blocker metadata before dispatch.
3. For bug or feature code, a RED test/repro failed for the intended reason
   before production code changed. Test-after exceptions are documented on the
   card before closeout.
4. GREEN implementation evidence proves the intended behavior, not just nearby
   compilation.
5. Simplify/refactor/write-tests ran after GREEN and removed unnecessary code,
   filled edge/error/security tests, or documented why no simplification was
   available.
6. Security hardening ran with proportional lenses for the changed surface.
7. Independent review/fix completed. `REQUEST_CHANGES` creates or reuses fix work
   and reruns the affected evidence until approval. The implementation card must
   create or link its Review/fix child before closeout when review is still the
   only missing gate.
8. PR merge eligibility was rechecked against current head: CI green, conflicts
   absent, required review/approval complete, no fabricated approval, and no
   bypass of branch protection or repo rules.
9. Merge evidence and post-merge closeout are recorded: merged commit, current
   `main` containment when relevant, rollout/E2E or explicit N/A, release note or
   release-governance impact, rollback/observability evidence, and learning or
   observation-harvest follow-up when applicable.
10. Human-only blockers are surfaced as typed blocked cards, not hidden in chat.

If any required gate cannot be completed by an agent, the lane blocks with the
specific missing external dependency. It must not silently downgrade the gate.

## Required card metadata

Every lifecycle-ready implementation, review/fix, merge, release, or closeout
card must include these fields in the body, not only in comments. Comments are
for evolving evidence; the body is the dispatcher/reviewer contract.

### Lifecycle metadata

- `owner`: profile or human/operator accountable for the active stage.
- `worktree`: absolute path or creation rule; mutating work should use a clean,
  task-owned worktree from `origin/main` unless the card explicitly owns the
  shared dirty root.
- `branch`: intended branch name, PR head, or detached verification target.
- `scope`: user story, issue/PR/card refs, affected surface, and intended
  outcome.
- `dependencies`: parent cards, upstream PRs, credentials, data fixtures, CI
  state, or human approvals required before this card can finish.
- `path_conflict_class`: one of `docs-only`, `web-ui`, `backend`,
  `db-migration`, `generated-client`, `mobile-android`, `mobile-ios`,
  `ci-release-deploy`, `multi-surface`, or a stricter project-specific class.
- `non_goals`: explicitly out-of-scope changes and surfaces the worker must not
  touch.
- `acceptance_criteria`: observable criteria, required tests, and completion
  evidence.
- `rollback_observability`: rollback path, telemetry/log/audit/SLO evidence, and
  any production or deployment observability expected.
- `review_lenses`: independent review perspectives required for the change.
- `user_story_e2e`: browser/device/API/story proof required, or precise N/A.
- `bounded_release_gates`: CI, release, deploy, migration, signing,
  release-please, or go-live gates that must be met before completion.
- `human_blocked_criteria`: exact cases that require a human/operator instead of
  agentic action.

### Path/conflict metadata

Include this exact section on every dispatchable lifecycle card so dry-run safety
checks and reviewers can read it without searching comments:

```markdown
## Path/conflict metadata

- allowed_path_prefixes: `path/a`, `path/b`
- forbidden_shared_roots: `backend/crates/platform/db/migrations/` unless this
  card owns the migration integrator lane; `backend/openapi/openapi.yaml` and
  `clients/**` unless this card owns generated-client settlement; `.github/**`
  unless this card owns CI/release changes.
- conflict_class: docs-only | web-ui | backend | db-migration |
  generated-client | mobile-android | mobile-ios | ci-release-deploy |
  multi-surface
- generated_file_policy: regenerate from the source of truth; no hand edits to
  generated clients or generated manifests except throwaway diagnosis that is
  reverted before closeout.
- dirty_root_policy: do not mutate the shared dirty checkout; use a clean
  worktree from `origin/main`, or document why this card is the serialized owner
  of the shared root.
- pr_review_routing: implementation must create/link Review/fix before closeout;
  open PRs stay guarded until CI/review/merge/post-merge evidence exists.
- dependency_edge_model: Kanban links mean parent -> child; downstream stages wait
  on all parents; review/fix and merge gates are explicit children, not prose.
- path_safety_evidence: command/output or reasoning proving the allowed prefixes
  are narrow enough and no sibling worker owns the same shared root.
```

## Lifecycle stages and exit criteria

### 1. Research

Goal: understand current repo, product, board, PR, issue, and external state
without mutating product code.

Required evidence:
- Relevant repo docs/code read with paths and line references where useful.
- Live PR/issue/CI/board state checked when the lane depends on current state.
- Existing implementation, tests, generated artifacts, and workflows traced to
  the actual source of truth.
- Unknowns, human-only blockers, and stale-board assumptions called out.

Exit criteria:
- Research note or parent handoff summarizes evidence, active PRs/issues/cards,
  branch/worktree constraints, likely path/conflict classes, review/CI/merge
  constraints, and human-blocked items.
- No product-code mutation occurred unless the research card explicitly allowed a
  throwaway spike and documented cleanup.

### 2. Plan

Goal: convert research into a bounded executable lane.

Required evidence:
- Owner/worktree/branch selected.
- Dependencies expressed through Kanban parent links where possible, not only
  prose.
- Scope and non-goals are narrow enough for a single owner.
- Path/conflict metadata is in the card body.
- Release gates and human-blocked criteria are listed before dispatch.

Exit criteria:
- The card can be dispatched without guessing files, branch, dependencies, tests,
  or review route.
- Shared roots such as migrations, generated clients, workflow files, release
  files, and deployment manifests have a serialized integrator owner.
- Mass-dispatch is not allowed until dry-run warnings are understood and bounded.

### 3. Spec

Goal: define what success means before code changes.

Required evidence:
- Acceptance criteria tied to user-visible or operational outcomes.
- User-story/E2E proof for UI or workflow changes. API endpoint tests alone are
  insufficient for UI feature claims.
- Rollback and observability expectations.
- Required review lenses and security lenses.
- Test plan with the RED-capable proof named.

Exit criteria:
- The spec makes clear how a reviewer distinguishes the intended fix from a
  nearby green check.
- Test-after exceptions, if any, are documented before implementation closeout.

### 4. TDD / RED

Goal: prove the bug or missing behavior is observable before production code is
changed.

Required evidence:
- A failing automated test, failing fixture, failing CLI/API transcript, browser
  repro, or other smallest meaningful red proof.
- Failure reason is recorded and tied to the acceptance criterion.
- For migrations/generated-client changes, RED can be a drift/gate/check failure
  that the GREEN path must resolve.

Test-after exception protocol:
- Allowed for docs-only/process-only cards, pure dependency bumps where no prior
  behavior can fail, or emergency fixes where a safe RED harness cannot be built
  before containment.
- The card must record: exception reason, why RED is unsafe/impossible, smallest
  compensating proof, reviewer who must scrutinize it, and follow-up test debt if
  any.
- A test-after exception never waives review, CI, merge, or post-merge evidence.

Exit criteria:
- RED proof exists or the exception protocol is complete.

### 5. Implement / GREEN

Goal: make the smallest production change that satisfies the spec.

Required evidence:
- Changed files stay inside allowed path prefixes.
- Production code passes the RED proof and the focused relevant checks.
- Generated outputs are regenerated from the source of truth when needed.
- No unrelated dirty-root changes are staged, committed, or claimed.

Exit criteria:
- GREEN output is recorded with command names and result summaries.
- The implementation did not broaden scope, silently alter release gates, or hide
  new blockers.

### 6. Simplify / refactor / write-tests

Goal: improve the GREEN state without changing the accepted behavior.

Required evidence:
- Remove duplication, dead abstractions, brittle special cases, or unnecessary
  dependencies found during implementation.
- Add edge/error/regression tests that were not necessary for initial GREEN but
  are needed for durable quality.
- Re-run the focused checks touched by simplification.

Exit criteria:
- Either simplification/test-fill work is recorded, or the card explains why the
  minimal implementation already had no safe simplification opportunity.

### 7. Security hardening

Goal: inspect changed trust boundaries before review/merge.

Minimum lenses, proportional to the diff:
- Secrets and credentials: no secret material printed, committed, or exposed in
  logs; production secrets remain operator-injected.
- Input validation and output encoding: hostile payloads, size limits, and
  malformed data handled fail-closed.
- Authn/authz and tenant/privacy boundaries: branch/org/cell policy enforced;
  cross-tenant or role-bypass paths tested.
- Audit and compliance: state changes write audit evidence in the same
  transaction; PII/GPS/legal constraints respected.
- Path traversal and file/system access: no unbounded paths, shell injection, or
  unsafe temp-file assumptions.
- SQL/data safety: migrations append-only for audited data; RLS/session GUC paths
  verified where relevant.
- Generated files and supply chain: generated artifacts reproduce; dependency
  additions are justified and version-checked live.
- Rollback/observability: failures are observable and rollback is documented.

Exit criteria:
- Security notes are recorded on the card or review/fix child, including N/A
  rationale for lenses that do not apply.

### 8. Independent review/fix loop

Goal: force an independent check before merge and make fix work visible.

Operational checklist: follow
[`review-fix-merge-governance.md`](review-fix-merge-governance.md) for Review/fix
child card fields, `REQUEST_CHANGES` fix routing, approval evidence rules, and
rerun requirements after fixes.

Required evidence:
- Implementation cards create or link a Review/fix child before completion when
  review remains. Do not park implementation as passive `blocked/review_required`
  when a Review/fix child can own that gate.
- Required review lenses are explicit: correctness, tests, security/privacy,
  product/UX, architecture, migration/generated-client/release as applicable.
- Reviewers cite current diff/head/PR and evidence they inspected.
- `REQUEST_CHANGES` creates or reuses fix work, reruns the affected RED/GREEN,
  security, and CI evidence, and routes back to review.
- Approval is recorded only from real reviewer output or PR review state; never
  fabricated from an agent's preference.

Exit criteria:
- Review/fix is approved or explicitly blocked on a human-only dependency.
- All requested changes are resolved with rerun evidence.

### 9. PR, CI, conflict, and merge

Goal: land the reviewed change without bypassing repo rules.

Operational checklist: follow
[`review-fix-merge-governance.md`](review-fix-merge-governance.md) for current-head
CI evidence, conflict and dirty-root checks, protected-branch blockers,
release-governance notes, merge eligibility, and post-merge closeout records.

Required evidence before merge:
- PR head is current and corresponds to the reviewed diff.
- Required CI/checks are green from a fresh run or documented as N/A for a
  docs/process-only change with reviewer agreement.
- Merge conflict check against current `main` is clean.
- Required reviews are approved; unresolved comments or changes-requested states
  have fix evidence.
- Branch protection, if present, is obeyed. If GitHub reports no protection,
  board/process gates still apply; absence of protection is not permission to
  bypass review or CI.
- No forced push, history rewrite, direct push to `main`, or admin bypass unless a
  human explicitly authorizes an emergency action and the card records why.

Exit criteria:
- Merge commit/SHA is recorded, or the card blocks with the exact reason merge is
  unavailable.
- Opening or updating a PR is not a terminal state.

### 10. Post-merge closeout

Goal: prove the merged change is integrated and operationally understood.

Required evidence:
- Merged commit is contained in the target branch or release branch.
- Post-merge CI/release signals checked when available.
- User-facing changes have browser/user-story/device evidence walking the actual
  flow, or a precise non-UI N/A rationale.
- Release-governance impact is recorded: Release Please PR/tag, changelog note,
  operator release note, or explicit no-release-impact decision.
- Rollback and observability notes are filed: revert path, migration rollback or
  forward-only posture, logs/metrics/audit/SLO checks, deploy smoke result where
  applicable.
- Learning/observation-harvest follow-up exists when the lane revealed a process,
  product, test, or skill gap.

Exit criteria:
- Downstream Kanban cards are done, explicitly blocked with typed blockers, or
  linked to a successor owner. No stale `running`/`ready` ambiguity is left for
  the same lane.

## Review lenses by change class

Use the narrowest set that covers the diff. Multi-surface cards inherit every
applicable lens.

| Change class | Required lenses |
| --- | --- |
| docs-only/process | correctness, governance consistency, no stale factual claims |
| web-ui | product/UX, accessibility, browser/E2E, authz/tenant, i18n, error states |
| backend | correctness, DB/RLS, audit, authz, observability, regression tests |
| db-migration | append-only safety, migration numbering, SQLx cache, rollback/forward-only note |
| generated-client | OpenAPI/source-of-truth, regenerated drift, TS/Kotlin/Swift compile as applicable |
| mobile-android | parity, offline/session, auth/passkey, strings, unit/device evidence |
| mobile-ios | parity, offline/session, auth/passkey, strings, Swift build/behavior evidence |
| ci-release-deploy | supply chain, secrets, rollback, branch/ref safety, operator gates |
| multi-surface | serialized integrator review plus all touched surface lenses |

## Bounded release gates

Release work is bounded by the surface changed. A card must state which gates
apply before dispatch.

- Docs/process only: markdown/readability check plus governance consistency;
  product CI is N/A unless docs under CI-triggered paths intentionally need PR CI.
- Backend/internal: fmt, clippy, tests, relevant `console-gate-*`, DB/migration checks.
- OpenAPI/client: OpenAPI app coverage, generated-client drift, TS/Kotlin/Swift
  compile, contract test where runtime shapes move.
- Web/UI: lint/test/build plus browser/user-story evidence for the changed flow.
- Android/iOS: platform unit/build/behavior/device evidence proportional to the
  changed surface.
- Deploy/release: CI success on target SHA, release-please/changelog or explicit
  no-release-impact note, image/security/signing gates where relevant.
- Go-live/operator: `docs/GO-LIVE-CHECKLIST.md` evidence for OCI, TLS, secrets,
  KCC/legal, Kakao templates, mobile signing, pilot roster, dashboards/alerts.

A release gate may be marked N/A only with a reason specific enough for a reviewer
or successor card to challenge.

## Human-blocked criteria

Block instead of guessing when the lane needs:

- Production credentials, signing keys, App Store/Play/OCI/Kakao access, or live
  secrets.
- Legal/privacy/location approval, KCC filing evidence, or customer/operator sign
  off.
- Protected-branch/admin action the agent cannot perform under repo rules.
- Authenticated live visual verification where no safe credential/test account is
  available.
- A product/UX decision that changes scope, data retention, legal posture, or
  customer-visible workflow.
- A dependency card or PR that is not complete and cannot be safely bypassed.

Blocked cards must record blocker kind, owner, exact missing evidence, safe
agentic work still possible, and what will unblock the lane.

## Active PR and stale-board reconciliation

Before dispatching a card guarded by an active PR or stale status, re-check live
state.

- Open PR with pending CI/review stays guarded and routes to its Review/fix,
  CI-fix, conflict, or merge-closeout lane.
- Open PR with `CHANGES_REQUESTED` creates/reuses fix work and reruns evidence.
- Merged PR with green/current evidence can close stale ready/running cards by
  recording merge and post-merge evidence; do not duplicate implementation work.
- A merged PR without rollout/release/user-story evidence needs a post-merge
  closeout card, not a new implementation card.
- A closed/unmerged PR needs explicit disposition: superseded, abandoned,
  reopened, or replaced by a new lifecycle lane.

## Completion metadata contract

Kanban completion summaries should be short, but metadata must be machine-usable.
Use keys like:

```json
{
  "changed_files": ["docs/specs/example.md"],
  "worktree": "/abs/path",
  "branch": "feat/example",
  "pr": 123,
  "head_sha": "...",
  "merge_sha": "...",
  "tests_run": ["command -> result"],
  "red_evidence": "test/repro or documented exception",
  "green_evidence": "focused pass evidence",
  "review_lenses": ["correctness", "security", "product/UX"],
  "review_fix_child": "t_xxx",
  "release_gates": ["CI green", "release note updated"],
  "rollback_observability": "rollback + telemetry/audit note",
  "human_blockers": [],
  "post_merge_evidence": "merge + rollout/E2E or N/A"
}
```

Do not include secrets, tokens, raw PII, or stale promises. If evidence is too
long for metadata, put the concise result in metadata and the durable details in
a Kanban comment or repo artifact.

## Forbidden shortcuts

- Marking a card done at `opened PR`.
- Skipping RED without the test-after exception protocol.
- Treating API tests as complete proof for a UI/user-facing workflow.
- Bypassing independent review, CI, conflict checks, branch protection, or release
  governance because automation is absent.
- Dispatching broad cards without parser-visible path/conflict metadata.
- Mutating generated clients by hand instead of changing the source of truth and
  regenerating.
- Using the shared dirty checkout for unrelated implementation work.
- Completing hidden work in chat without mirroring the durable state to Kanban.
- Fabricating approvals, production credentials, live-verification evidence, or
  post-merge rollout status.
