# Console development pipeline

> **HISTORICAL / NON-AUTHORITY:** Current delivery, verification, merge, and issue-lifecycle policy lives in [`../current/DELIVERY.md`](../current/DELIVERY.md). The material below is retained in place as historical method and cannot override current delivery authority.

Status: historical delivery method retained for path stability; non-authority.

## Authority precedence

1. Pivot.
2. Accepted ADRs consistent with the pivot.
3. Current roadmap and this pipeline.
4. Machine-readable capability/jurisdiction registers.
5. Exact candidate and verification evidence.
6. Historical plans, branches, runtime state, chats, and handoffs as context only.

## Admission and ownership

A lane is admitted only with parser-visible registry metadata and an additive ledger receipt containing outcome/non-goals, exact base SHA, reference contract, owner, allowed and forbidden roots, source-of-truth writer, resource leases, pre-mortem, rollback/stop conditions, immutable test baseline, review identities, evidence, head SHA, result, HOLDs, and post-merge readback.

Use bounded writer worktrees plus reserved integration and fix roots. Private roots may proceed in parallel; migrations, lockfiles, OpenAPI, CI, authority registers, and generated faces are serialized. Increase concurrency only after two collision-free epochs and reduce it after collision, stale-base rebuild, saturation, or reviewer backlog.

## Candidate train

Every candidate uses exact immutable SHAs: product candidate **C**, signed direct-child authority tip **T**, and hosted merge **M** whose content tree equals T. Reviews and evidence bind to the current SHA. Never use GitHub update-branch merge, destructive shared-workspace Git operations, or evidence from a superseded candidate.

## Required gates

- **Build/CI:** exact test-set membership in both directions, no ran-nothing success, feature-bearing and JavaScript reachability, credential safety, deterministic shard manifests, and zero required Buck-only coverage before Buck deletion.
- **Migrations:** parser negative controls, contiguous number assigned at landing, clean/populated apply-reapply, tenant/RLS/audit/PII checks, and one migration-directory writer.
- **Contracts:** route/operation/schema parity, deterministic OpenAPI 3.1 YAML, served-spec drift, and consumer validation.
- **Authz/engine/domain:** before/after verdicts, nondisclosure, temporal bounds, deterministic identity/replay/revisions, no-mutation preflight, atomic mutation/audit/approval/receipt, and domain-specific golden cases.
- **Tests:** no unapproved deletion, skipping, quarantine, or weakening; preserve discovered and executed counts and independently prove the exact invocation.
- **Docs:** local links and authority markers pass executable checks.

## Review and rollout

High-risk authz, migration, contract, approval, HR, release, and compliance-sensitive work requires an implementer, two independent adversarial reviewers, and a distinct fixer/integrator. CI can prove record completeness or implementation evidence; it cannot synthesize legal compliance, production exposure, or release authority.

After every wave or failure, record evidence versus inference, disproven hypotheses, root-cause confidence, corrective code/test, mechanical prevention, workflow version, owner/revisit date, and collision/queue/review/cache/retry/escaped-defect metrics.

No live production, DNS, TLS, secret, release, exposure, payment, or compliance-claim action is authorized by this pipeline.
