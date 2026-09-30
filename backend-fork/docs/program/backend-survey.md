> **HISTORICAL after the 2026-07-28 pivot.** This document preserves evidence and prior reasoning only. It cannot authorize Buck2, broad product scope, fan-out, or implementation work. Current product authority is [`docs/current/PRODUCT.md`](../current/PRODUCT.md), [`ROADMAP.md`](../current/ROADMAP.md), and [`DELIVERY.md`](../current/DELIVERY.md). Pivot history only: [`docs/PIVOT-2026-07-28.md`](../PIVOT-2026-07-28.md) (non-authority).

# Backend surface survey (2026-07-09) — console wiring coverage vs HANDOFF contract

Source: `backend/openapi/openapi.yaml` (223 REST paths). Console is **almost entirely wired to the real typed client, not fixtures** (~118 paths used; genuine fixtures essentially absent). Local repo `HANDOFF.md` = old 76-line FSM handoff; the real backend contract = HANDOFF.md in design project `9c7c313a`.

## Coverage highlights
- **Solid/wired (contract-adjacent):** work-orders/dispatch, employees(+import), equipment/asset, financial purchase-requests(full workflow), customers/sites, attendance, messenger, mail(custom Rust), map/location, KPI/ops, support/tickets, identity/credential admin(platform.ts/groupAdmin.ts), storefront/sales, workflow-studio(full: catalog/CRUD/run/simulate/publish/pause/resume/rollback/clone/history/run-log/revisions).
- **PARTIAL:** explore(equipment-typed only, no generic object search/graph/type-registry), policy(RBAC-role-shaped, no Cedar doc CRUD/simulate/authorize REST), financial(no GL/journal/voucher), leave(read-only), evidence(presign/status; no attestation/verify), ingest(domain importers, no generic DX pipeline), notifications(WS only), directory.
- **MISSING REST (crate exists, zero REST):** payroll, inventory. Benefit catalog REST now materializes and hydrates the generic lifecycle binding and supports audited catalog/tier/eligibility replacement under PBAC/RLS; the development-only body uses typed catalog creation and item-field edits. Production exposure and independently executed closed-loop runtime evidence remain pending. **Also missing:** leave mutations, recruiting(postings/candidates), notifications REST, board.
- **Backend-ready but UI-unwired (quick wins):** `reporting/work-diary`(+confirm), `exports/daily-status`/`work-diary`, `console/kill-switch`, `console/rollout/org-flag`, `hr/exit-cases/{id}/approval-draft`.

## THE SPINE — HANDOFF engine layer, essentially UNBUILT (critical path)
Design assumes an ontology/governance engine under every screen. Ranked backend sub-tasks (foundational engines first — most sections depend on them):
1. **§18 Ontology engine (backbone)** — ObjectType registry table + REST · generic instance store + graph traversal · typed linkTypes w/ cardinality · actions(writeback) · analytics(derived). *No backend today* (only equipment-scoped `object-actions/{catalog,execute}` + `timeline-graph`). Everything below assumes it.
2. **§20 + §15 lifecycle/CRUD governance** — effective-dated versioned store · as-of query · draft-direct vs override(reason+four-eyes+before-audit) engine · impact preflight · soft-archive gate. (only workflow-def revisions + lifecycle-events log today)
3. **§16 guardrails engine** — per-action preflight control-point (authority/checklist/approval/egress), fail-closed, Checklist object, egress gate (= §13 layer-2).
4. **§10 DX- ingest** — Source connector + IngestJob FSM + deterministic Rust parse/OCR + Template + provenance/lineage.
5. **§11 evidence hardening** — RFC-3161 TSA, custody chain, derivatives, re-verify REST, media/ZIP. *WORM store + staging + hash-chain (L20 #204) already exist — completion, not greenfield.*
6. **Cedar policy authoring + simulate REST** — Cedar engine + `0103_create_cedar_policy_staging` exist (DARK, this branch `feat/cedar-activation`); missing policy-doc CRUD + simulate/can/authorize REST that Policy Studio needs.
7. **§17 enterprise standard** — SSO(SAML/OIDC), SCIM 2.0, KMS-envelope, OpenTelemetry, SIEM/OCSF, FW- objects. (RLS tenant isolation = the one existing pillar.)
8. **§19 configurable dashboards** — DashComponent persistence + shared-layout deploy-approval governance.
9. **§14 mail decision** — adopt mox vs keep working custom Rust IMAP/SMTP (comms/adapter-imap + mail_sync.rs); either way add litigation-hold/journaling/e-discovery/delegation-PBAC/outbound-DLP.
10. **§12 office editor** — DocumentServer(JWT/versions/PBAC); heaviest/latest, AGPL gate.

## Decisions I'm taking (full autonomy)
- **Mail (§14):** keep the working custom Rust mail stack; add compliance features incrementally. Do NOT rip out for mox now (working code > rewrite; MIT/custom both fine). Revisit only if compliance features prove infeasible on custom.
- **Office editor (§12) + full §17 SSO/SCIM:** defer to a late tier (heavy, AGPL/infra) — not on the critical path for the console overhaul.
- **Critical path = backend engine tier (§18 → §20/§15 → §16 → Cedar authoring REST)**, built by spawned subagents (main session can't run cargo; subagents can), grounded in the benchmark brief (Foundry ontology + Cedar). This tier must be ready by the Phase-C wiring pass; start it early, parallel to the frontend foundation + Phase A/B.

Key files: Cedar `backend/crates/platform/authz/src/cedar_pbac/engine.rs` + `0103_create_cedar_policy_staging.sql`; WORM `backend/crates/platform/storage/src/lib.rs` + `0019_harden_worm...`; mail `backend/crates/comms/**` + `backend/app/src/mail_sync.rs`; ontology Actions slice = `object-actions/*` in openapi.yaml.
