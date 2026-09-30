# Agentic engineering playbook

> **HISTORICAL METHOD / NON-AUTHORITY:** Current delivery authority lives in [`../current/DELIVERY.md`](../current/DELIVERY.md). The detailed practices below remain reusable reference where consistent with current authority, but they cannot dispatch work, clear a HOLD, or close an issue.

This historical playbook describes reusable delivery practices only where they remain consistent with the current documents.

## Prepare before parallelism

The current product-specific preparation state is HOLD. The instance-backed
`company_conformance` fixture is generic-engine regression evidence, not the
Company/HR target, and `CATALOG.md` is a provisional preparation artifact rather
than a dispatch queue.

1. Accept explicit owning ports and single-writer boundaries for Company, Person,
   Employment, and PayRun.
2. Establish a known, hand-reviewed Company and OrgUnit product reference through
   those contracts.
3. Build and independently approve a replacement product conformance target; do
   not freeze or repurpose the existing generic fixture.
4. Reconcile `CATALOG.md` to the accepted contracts before any later authority
   promotes it as a mechanical expansion guide.
5. Only then may a later candidate authorize a small collision pilot. This
   playbook does not dispatch JobPosition or projection fan-out while the HOLD is
   active.
6. Preserve and independently verify the test baseline before widening.

## Lane contract
Every admitted lane has parser-visible registry metadata and an additive `docs/program/ledger/<lane-id>.md` receipt: outcome/non-goals, exact base SHA, owner and writable roots, forbidden shared roots, source-of-truth writer, leases, pre-mortem, rollback/stop conditions, immutable test invocation, reviewers, evidence, head SHA, result, HOLDs, and post-merge readback.

After current authority records the preparation gate as satisfied, use at most
three writer worktrees, one integration worktree, and one reserve/fix worktree.
Until then, this topology is descriptive only and creates no product lanes.
High-risk authz, migration, contract, HR, release, and compliance-sensitive
changes require one implementer, two independent adversarial reviewers, and a
distinct integrator. Increase concurrency only after two collision-free epochs;
reduce it after collisions, stale-base rebuilds, saturation, or review backlog.

## Evidence and enforcement
Preflight rejects dirty integration roots, stale bases, overlapping ownership, unowned migrations, generated-face writes, and weakened tests. Receipts retain command, revision, lockfile, runtime, counts, failures, and artifact hashes. Dry-run checks prove non-mutation. Jurisdiction records retain source locator, retrieval/effective dates, snapshot hash, applicability, reviewer, evidence links, due date, and claim type; CI never synthesizes a compliance conclusion.

## Task-selected reasoning lifecycle
The canonical vocabulary and routing policy live in [`AGENTS.md`](../../AGENTS.md#task-selected-reasoning-lenses). Apply that policy to every substantive planning, investigation, implementation, review, and verification task, not only to code review.

1. Before acting, classify the task and its risk, then select the smallest useful set of at least two lenses.
2. For high-risk authz, migration, contracts, approval, HR/payroll, release, production, or compliance-sensitive work, include Red Team, Operability / Day-2, Blast-radius / cell-based, and Zero-trust / defense-in-depth, or record a lens-specific not-applicable rationale.
3. Use the selected lenses to guide the work. Re-evaluate the set when evidence, scope, or risk changes.
4. When the task produces a durable governed artifact, persist concise findings, decisions, tradeoffs, exceptions, and lens-set changes in its canonical `lens_contract: v1` evidence block. Record outcomes, not private chain-of-thought.

The policy applies normatively to all substantive tasks. CI can prove only the structure of designated durable records and root manifests; it cannot prove universal task compliance, authentic reasoning quality, or access private reasoning. Historical unmarked ledgers and retrospectives are grandfathered, while new or materially revised governed records use the v1 evidence block.

Continue to review object-first, deep-not-wide, fail-closed authorization, mock-independent behavior, human dignity, and closed-loop rollback. Promote a lesson into `AGENTS.md` only after one severe incident or repeated recurrence; otherwise keep it in retrospectives and this playbook.
