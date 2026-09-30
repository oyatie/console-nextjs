> **EXECUTABLE-CONTRACT / NON-AUTHORITY.** CI-bound gate contract (machine-checked). Cannot authorize product scope, clear HOLDs, release, or production readiness. Current product authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Foundation Gates — G002 Shared Contracts and Hard Gates

FOUNDATION-GATE-READY: true

Generated: 2026-06-29T09:14Z; refreshed for Ultragoal G002 on 2026-06-30T00:34Z.

This document is the durable `G002-wave-1-shared-contracts-and-hard-gat` contract. Domain goals G003-G009 must not claim completion until this gate passes locally and in CI. The gate exists to prevent implementation lanes from widening scope, bypassing policy/audit/passkey constraints, treating UI/security/release requirements as optional, or using obsolete goal ownership from earlier plans.

## Entry evidence

- Canonical backlog ledger: `docs/specs/backlog-clearance-ledger.md`
- Route/persona ownership register: `docs/benchmarks/enterprise-ui-route-audit.json`
- Automated gates: `npm run check:foundation-gates` and `npm run check:enterprise-ux-parity`
- CI enforcement: `.github/workflows/ci.yml` runs both gates and watches `docs/specs/**` plus `docs/benchmarks/**`.
- Concurrent-delivery authority: `docs/program/console-fanout-epoch-contract.md` and `docs/program/console-buck2-scale-playbook.md` define the repository-owned Buck2-only, immutable exact-SHA, bounded-fan-out, multi-cell, candidate-CI, and release-matrix evidence policy. The G002 gate verifies those durable contracts only; it does not depend on a developer-local profile, home-state metadata, or external orchestration runtime.

## Gate A — policy/audit/passkey contract baseline

Required before domain completion claims in G003-G009:

1. Backend mutation boundaries keep the existing CI gates green:
   - `console-gate-layer-boundary`
   - `console-gate-audit-coverage`
   - `console-gate-migration-safety`
   - `console-gate-tenant-isolation`
   - `console-gate-pii-no-logs`
   - `console-gate-rls-arming`
   - `console-gate-dev-auth-absence`
2. Sensitive object actions, policy assignment, custom-role changes, account lifecycle transitions, ownership transfers, payroll/HR legal decisions, and any signing-equivalent approval must require fresh passkey step-up and produce append-only audit evidence.
3. OTP/cold-start setup is only a bootstrap path. It must not grant mature account status until passkey setup and required Korean privacy/service agreements are complete.
4. Approval feeds and Work Hub feeds must be server-owned, authorization-checked, tenant/RLS-scoped, and group/org/branch aware; browser composition cannot be the security boundary.
5. PII, payroll, location, HR, and legal-ownership data must be masked/minimized by default and never copied into routine audit text or GitHub-facing docs.

## Gate B — workflow/approval/action lifecycle baseline

Required before G005/G006/G007/G008 completion claims:

1. Workflows are source-object centered: action inbox, messenger, mail, files, approvals, audit, notification rules, and notifications attach to an operational object rather than floating demo widgets.
2. Approval/payment-line lifecycle must expose line definition, current actor, required/optional comment, evidence/images, decision state, terminal queue removal, badge changes, and immutable history.
3. Planned work must start from an existing received/source work order when that is the business process; later lanes may not invent detached plan objects to satisfy UI only.
4. Notifications, unread counts, urgent badges, and mobile alerts must be derived from server lifecycle state, not local-only optimistic UI state.

## Gate C — ontology/import/export/object-lineage baseline

Required before G006/G008 completion claims:

1. Every route row has source object, lifecycle states, data class, scope/denial story, and required E2E evidence in `docs/benchmarks/enterprise-ui-route-audit.json`.
2. Import/export must preserve raw-file lineage, typed mapping, Korean encoding, validation results, dry-run/apply audit, and standardized output schema.
3. Entity-type allowlists are mandatory: employee data maps only to people/HR schemas, assets only to equipment/inventory schemas, sites only to location/customer-site schemas.
4. Analytics, MES, AI/ML/RL/LLM scopes remain deterministic/future-gated until source objects, workflows, policy, observability, and evaluation evidence exist.

## Gate D — CI/CD/security/release baseline

Required before production-impacting completion claims:

1. CI must keep Rust fmt, clippy `-D warnings`, tests, OpenAPI contract, generated client drift, TypeScript, Kotlin, web lint/test/build, browser e2e, Android, and iOS gates intact.
2. Security workflow must keep Trivy filesystem/secret/IaC scans, cargo audit, cargo deny, and npm audit high/critical gates intact.
3. Image release must wait for CI, build immutable digests, run blocking Trivy image scan, sign with cosign, attest provenance/SBOM, and auto-bump GitOps overlays as desired state; live deployment claims still require default `scripts/deploy.sh` rollout/pod-digest/endpoint verification, not bump-only output.
4. Release Please retains `GITHUB_TOKEN` for bot-authored PR/tag operations; protected custody convergence requires `RELEASE_PLEASE_TOKEN` only for the release-branch update that schedules required PR workflows.
5. Rust is pinned to 1.96.0 for CI/local parity.
6. OCI/free-tier, backup/restore, KMS/secrets, Argo, and observability hardening remain G009/G008 execution work unless a lane changes those artifacts; this G002 gate only proves the baseline and the follow-on evidence columns.

## Gate E — UI shell/design/i18n/a11y/no-text-wall baseline

Required before user-facing completion claims:

1. User-facing work must use the existing authenticated shell, role-aware navigation, and page-header conventions.
2. Browser e2e for critical user stories must include console-error guards, zero critical/serious axe violations, and no raw i18n key leakage.
3. Web lint must keep the UI string gate. Cross-surface i18n checks must remain available through `scripts/check-i18n.mjs`.
4. Enterprise UX work must cite the parity matrix and benchmark gaps. It must replace text walls with actionable state, scope, priority, evidence, and next-action affordances.
5. Loading, empty, error, disabled, and permission-denied states are required UI states, not optional polish.
6. A route is not mature until its G003 browser-persona evidence replaces pending screenshot/trace placeholders with real screenshots/traces and live DB/API/browser/mobile proof where relevant.

## Downstream lane contract

Every later lane must cite both the backlog ledger row and this foundation gate, then provide its own RED/GREEN evidence and PR/review/fix/merge/rollout proof. If `npm run check:foundation-gates` or `npm run check:enterprise-ux-parity` fails, domain work stops until the shared gate is restored. Domain goals G003-G009 must not claim completion from local unit tests alone; they need the lane-specific verification ladder defined in the backlog ledger and route audit register.
