> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Roadmap to Production — All Remaining Work to a Finished Enterprise-Grade SaaS

> Planning artifact (agent-skills:planning-and-task-breakdown). Reconciles the ultragoal ledger
> (`.omc/ultragoal/plans/conglomerate-platform/` G001–G015) with everything surfaced since: the
> coverage-audit worklist (129 actions / 109 gaps), the auth incident (#19.25 + OTP-issuance), log
> persistence, and the remaining #19/#20 issue items. **Status: PLAN — not yet in execution.**
> **Detail lives in:** `SPEC.md`, `docs/specs/{knl-business-os,org-hierarchy,rbac-configurable,
> operations-intelligence,payroll,accounting}.md`, the coverage-audit output, and the issue threads
> #19/#20.

## 0. Definition of done (the bar — every track inherits it)

"Enterprise-grade production SaaS" = the conglomerate operations platform of `SPEC.md`, and for **every
shipped action**, all of:
- **Tested trifecta:** unit + integration (real `console_rt`, never BYPASSRLS-masked) + e2e/regression.
- **Apparent breakage:** the full CI suite (incl. Playwright E2E) is **green and BLOCKS the deploy**.
- **Secure + audited:** RLS-armed, authz-gated (capability not role-string), every mutation audited,
  tenant-isolated, no secrets in logs/`/tmp`.
- **Accessible + legible:** AA a11y, Blueprint dense/legible, **visual-verdict ≥90** on every path,
  Korean copy only in `ko.ts`, no raw UUIDs, KST.
- **Operable:** logs persist + are queryable, metrics/alerts, backups verified, runbook current.
- **Regulated modules** (payroll, 회계): effective-dated rates + golden-case tests + 노무사/세무사 sign-off.
- **No premature AI.** AI/ML/RL/LLM are final-stage decision-support augmentations only after the
  deterministic/algorithmic platform, observability, evaluation, privacy, and workflow controls in
  `docs/specs/operations-intelligence.md` are production-grade.
- **No deferral, no MVP, no v1/v2.** The FULL enterprise scope below is built now, meticulously and to
  completion. The "waves" in §4 are **parallel-execution order driven by hard dependencies** (you cannot
  compute payroll before HR employees/attendance exist), **not scope phasing** — every track ships
  complete, no shortcuts, no stubs, no "for now".
- **External integrations = GOVERNMENT-DIRECT ONLY.** Use the government's own APIs: **국세청** (직접
  전자(세금)계산서 발급/조회), **공공데이터포털 data.go.kr** (4대보험 요율·소득세 간이세액표·최저임금 등 고시
  데이터), **도로명주소/국토교통부** (주소검색·좌표변환). **No commercial ASP / relay / SaaS middleman**
  (팝빌·바로빌·Kakao·etc.). Each government API is verified live against its official spec before use
  (see [[verify-latest-dependencies]]). 노무사/세무사 sign-off is a **release-gate process**, not an API
  dependency.

## 1. Where we are

DONE + LIVE: G001 (stabilize live ops) — #19 core flows (intake/plan/purchase/inquiry/PM) + codex P0
authz, deployed + checkpointed. The platform is live on OCI/Talos (KNL, 1 real tenant). IN FLIGHT:
substitution #20.1/#19.20 (committed), tenant force-remove #18 (building), the CI-debt green (push
pending). DESIGNED + security-reviewed, awaiting implementation: org-hierarchy (G002), configurable RBAC.

## 2. The dependency spine (what gates what)

```
Quality & Ops foundations (Track Q, Track O)  ── cross-cutting, mostly parallel, START NOW
        │
Security foundations:  org-hierarchy (G002) ─→ configurable RBAC (B0.5)   ── gate multi-법인 + role config
        │                                            │
        ▼                                            ▼
Foundry UI spine:  Blueprint tokens (G003) ─→ Object-View kit (G004) ─→ object views (G005…) ─→ Triage Home (G007)
        │                                                              ─→ Object-Set/faceted (G008) ─→ timeline/graph (G009)
        │                                                              ─→ action/write-back engine (G010)
        ▼
Back-office:  HR core (G011) ─→ Korean payroll (G012)        ERP (G013, semi-independent)
Comms/field:  webmail enable (G014)            dispatch map (G015)
        │
        ▼
Launch hardening: perf + a11y + full security review + load/scale  ── final gate
```
Bottom-up: foundations first. The Foundry UI is a **sequential spine** (tokens→kit→views); most other
work parallelizes around it.

## 3. Tracks (parallel-capable units) — each is a batch family

Collision rule: agents parallelize only on **disjoint file trees**; `ko.ts` + `openapi.yaml` are shared
collision points; run isolated via **git worktrees**, merge with discipline. ⟂ = parallel-safe now.

### Track Q — Quality & Test Mandate (cross-cutting; ~80% parallel-safe) ⟂
The 109-gap worklist. Backend `console_rt`/unit tests live in disjoint per-crate `tests/` dirs → fan out.
- **Q0 (Tier-0, no openapi):** execute-purchase console_rt+e2e, register_device, admin-OTP IDOR, update-user
  escalation negative, storefront-media leak, delete-listing, arrival-events read, integrity list/triage,
  OrgWideQueueTriage negative, **force-remove tests committed + e2e**.
- **Q1 (Tier-1):** convert BYPASSRLS-masked writes to real `console_rt` (registry/messenger/financial/
  reporting); auth-rest handler `tests/`; purchase approve/reject SoD under console_rt.
- **Q2 (Tier-2):** parsers, thin reads, transition e2e (per the worklist).
- **Q3 (gate):** make red E2E/test **block the deploy** (ci/image-release `needs:`); green + de-flake the
  Playwright suite; the org-binding rls-arming lint (#43); the real #31 PlatformConsole fix.
- **Standing rule:** every NEW action in any track ships with its trifecta — enforced by Q3's gate.

### Track O — Ops & Infra (disjoint `deploy/**`, `.github/**`) ⟂
- **O1 Log persistence (#50):** Loki + Grafana Alloy → OCI Object Storage, retention, query. *(infra
  deploy to live — confirm approach first.)*
- **O2 Observability:** app metrics + alerts (latency/error-rate/auth-failure/queue-depth), so incidents
  are caught proactively, not reported by users.
- **O3 Secrets:** `CONSOLE_MAIL_MASTER_KEY` → OCI Vault (#28); audit all secrets are Vault-sourced.
- **O4 Resilience:** verify CNPG/Barman restore drill; single-node → document/plan the scale path.

### Track A — Auth hardening (gated on the acme commit; touches `provisioning`+`ko.ts`)
- **A1 #19.25 enroll fix (#49):** desktop poll→advance + rate-limit headroom + `auth-09` e2e + backend
  integration. **High product priority** (real users locked out).
- **A2:** passkey/OTP handler authz tests; device-registration path; recovery flows.

### Track S — Security foundations (sequential within; gates B-track multi-법인)
- **S1 org-hierarchy (G002):** Group→법인 scoped RBAC; consolidated reads = armed per-member fan-out.
  Design + security-review done (8 fixes folded) → **implement** (migration, resolver, fan-out helper,
  view-as, tests). *Re-confirm the §0 revisions before code.*
- **S2 configurable RBAC (B0.5, #46):** data-driven roles + per-role policy. Design + security-review done
  (NOT-YET → 9 must-fixes folded) → **re-review the revised spec**, then implement P0→P4 (de-string the 18
  authz sites + CI gate; 3 tables + policy_version + feature_catalog; RoleManage + escalation closure;
  console UX; org-admin as first custom role).

### Track F — FSM completion (remaining #19/#20; per-domain, ko.ts/openapi serialize)
- **F1** #19.9 Excel import of work progress (#35) · **F2** #19.11 org-chart tab + 직급/rank (#36) [feeds
  HR] · **F3** #19.12 corp-to-corp linking (#37) [feeds org-hierarchy] · **F4** #19.19 sales listing
  autofill + unlimited photos + field set (#41) · **F5** #19.18 거래명세표 upload/scan (#42) · **F6**
  #19.23/24 ticket delete + unify inquiries (#42) · **F7** #23 role-workflow blockers (WO detail, approval
  queue, MEMBER dead-role) · **F8** analytics drill-down/trend/target · **F9** #19 calendar views ·
  **F10** #25 mail/messenger maturity (notify-on-message, send reliability). **Blocked:** #19.13–16 FLMS
  (external Windows MSI — needs the vendor data export, not buildable here).

### Track P — Foundry/Platform UI (G003–G010; SEQUENTIAL spine)
- **P0 Blueprint tokens (G003)** → **P1 Object-View kit (G004)** → **P2 Equipment view (G005)** → **P3 ⌘K/
  nav/DataTable (G006)** → **P4 Triage Home (G007)** → **P5 Object-Set query API + faceted (G008)** → **P6
  timeline + graph lenses (G009)** → **P7 action/write-back engine (G010, CAP-5)**. The kit (P1) unblocks
  every object view; build it once, then views fan out.

### Track H — HR + Korean payroll (G011–G012; H2 depends on H1)
- **H1 HR core (G011):** employees + 직급/조직도 (reuses F2) + attendance + leave. Object-View over the kit.
- **H2 Korean payroll (G012):** 4대보험/소득세/주휴/연차/퇴직금/최저임금; **effective-dated rate tables sourced
  from the government** (국세청 근로소득 간이세액표; 국민연금·건강보험·고용·산재 공단 고시 요율 via 공공데이터포털;
  고용노동부 최저임금 고시); 급여명세서; **golden-case tests vs 노무사-validated worked examples**;
  RBAC-restricted + audited. Release-gated: no payroll calc to prod without versioned government rates +
  golden test + **노무사 sign-off**.

### Track E — ERP (G013; semi-independent, large)
- **E1** 견적→수주→세금계산서→미수금 (AR) · **E2** PO→입고→거래명세표→미지급금 (AP) · **E3** 재고 (WO-consumed
  parts decrement + cost ledger) · **E4** 회계 double-entry + 부가세 period reconcile · **E5** 전자(세금)계산서
  발급/조회 via the **국세청 direct API** (홈택스/e세로 전자세금계산서 — government source; **no 팝빌/바로빌 ASP**).
  Release-gated: 회계 needs 세무사 sign-off + golden tests.

### Track C — Comms & field (G014–G015)
- **C1 webmail enable (G014):** built (#26) → needs O3 master key + B-mail-3 hardening (#39) → enable +
  e2e. · **C2 dispatch map (G015, #29):** consent-gated live location during clocked-in hours + road
  routing/ETA + arrival/return markers.

### Track L — Launch hardening (final gate, after the above)
Web perf/CWV, full a11y AA sweep + visual-verdict ≥90 on every path, **independent full security review**,
load/scale test, runbook refresh, on-call/alerting. Then "production-grade" is true.

### Track I — Operations Intelligence (after foundations; AI last)
- **I1 Mechanical decision ledgers:** every 기안/구매/승인/입찰/pricing/planning/maintenance/rental/HR/payroll
  workflow captures structured inputs, alternatives, decisions, policy version, and outcome variance.
- **I2 Deterministic + probabilistic calculators:** asset sell/keep/repair/replace, rental rate/margin,
  maintenance windows/cycles, reserve equipment, manpower/SLO, inventory/parts reorder, and procurement
  scenarios. These are transparent, testable algorithms before AI.
- **I3 Observability + evaluation:** freshness/completeness, forecast calibration, recommendation
  acceptance/rejection/override, workflow conversion, SLO/margin/downtime/stockout variance, and drift.
- **I4 Scenario workbench + workflow conversion:** recommendations create governed drafts only; no direct
  business write-back.
- **I5 Final-stage AI/ML/RL/LLM:** ML forecasting, offline RL simulation, LLM evidence summaries/draft
  memos/import assistance, and executive decision briefs only after I1-I4, security/privacy review, and
  model rollback gates are live.

## 4. Parallelization plan (waves)

Mechanism: **worktree-isolated agents** per batch; serialize `ko.ts`/openapi merges; the Foundry spine
(Track P) is one sequential lane.

- **Wave 1 (NOW, ⟂ — concurrent with the acme build):** Q0+Q1 (backend coverage fan-out, per-crate),
  O1+O3 (log persistence + secrets), Q3 (deploy-blocking gate + #43 + #31). Disjoint trees, clean merges.
- **Wave 2 (after acme commits):** A1 (#19.25), then S1 (org-hierarchy) — both gate later work; in
  parallel start P0+P1 (Blueprint tokens + Object-View kit — disjoint web foundation) and the F-track
  items that don't touch the same domain (e.g. F1 Excel-import backend vs F4 sales — disjoint crates).
- **Wave 3:** S2 (configurable RBAC, after its re-review) + P2–P4 (object views + Triage Home) + F-track
  remainder + H1 (HR core) — fan out by domain.
- **Wave 4:** H2 (payroll), E (ERP), P5–P7 (lenses + action engine), C (webmail + dispatch).
- **Wave 5:** Track L launch hardening → GA. Track I starts with mechanical ledgers/calculators only
  after the UI foundation is mature enough that expansion work reuses Work Hub/Object Explorer/Workflow
  and Policy patterns instead of creating one-off screens; AI/ML/RL/LLM waits until the final I5 gate.

Max safe concurrency per wave ≈ number of disjoint domains (≈5–8); the Foundry spine is the wall-clock
critical path, so it should start as early as Wave 2 and run continuously.

## 5. Checkpoints (gate between waves; no advance until green)

- **After Wave 1:** coverage materially up (Tier-0 closed); CI green + **deploy now blocks on red**; logs
  persist. *This is the "breakage is apparent" milestone — earns trust in every later wave.*
- **After Wave 2:** #19.25 fixed + regression-tested (real users unblocked); org-hierarchy live; the
  Object-View kit exists (every later view is cheap).
- **After Wave 3:** configurable RBAC live (multi-법인 ready); FSM issue backlog closed; HR core usable.
- **After Wave 4:** payroll golden-validated + 노무사-signed; ERP period reconciles + 세무사-signed; webmail +
  dispatch live.
- **After Wave 5:** perf/a11y/security/load all pass → **production-grade GA**.

## 6. Risks & dependencies (engineering-managed; government-direct only)

| Risk / dependency | Track | Resolution |
|---|---|---|
| 전자(세금)계산서 issuance | E | **국세청 direct API** (홈택스/e세로) with the 사업자 공동인증서 — government source, no ASP. Verify the official spec live; build the issue/조회 client + a sandbox-test harness. |
| Payroll rate data (4대보험·간이세액표·최저임금) | H2 | **공공데이터포털 (data.go.kr) + 국세청** government datasets, ingested as versioned effective-dated rate tables; refreshed on official 고시. No commercial rate feed. |
| 회계 / 급여 correctness | E/H2 | golden-case tests vs worked examples + **노무사/세무사 sign-off** as a release gate (process, not an API). |
| Address / geocode | F/C | **도로명주소 OpenAPI + 국토교통부 좌표 API** (government), replacing any Kakao usage. |
| FLMS master data (#19.13–16) | F | KNL's own legacy data behind a Windows MSI — needs the vendor's data export (not a provider choice); engineer the importer once the export arrives. |
| Single-node cluster | O/L | scale-out plan (multi-node Talos + CNPG HA) built + load-tested in Track L before real multi-tenant load. |
| 109→0 coverage closure is large | Q | the parallel worktree fan-out is the mitigation; the deploy-blocking gate keeps it from regressing. |
| Premature AI/LLM/RL/ML requests | I | Reject until trusted data, deterministic algorithms, observability, evaluation, privacy, and governed workflow write-back exist. AI is a last-stage assistant, not a foundation. |

## 7. Resolved direction (per the 2026-06-24 directive)

- **Scope:** FULL enterprise production, built now — **no v1/MVP/deferral**, no stubs/shortcuts. The full
  no-code configurable ontology + action/write-back engine (Track P incl. G010) **is in scope and built**,
  not deferred to a v2. Every regulated module (payroll, 회계) is built to completion against government
  rate sources with golden tests + professional sign-off.
- **External integrations:** **government-direct only** (국세청, 공공데이터포털, 도로명주소/국토교통부). No
  commercial ASP/relay/SaaS middleman. (Supersedes SPEC.md's earlier "ask-first: 팝빌/바로빌/Kakao" note —
  that boundary is now "government-direct only".)
- **Sequencing:** dependency-ordered for correctness, not value-cut. Foundations (Q/O + A1 + S) are
  non-negotiable and start first because everything else stands on them; the Foundry UI spine (P) runs
  continuously from the moment its kit exists; F/H/E/C fan out by domain in parallel. **All of it ships** —
  the order is only about safe concurrency, never about leaving anything out.
- **Build posture:** meticulous, comprehensive, systematic — each action gets its trifecta + security
  review + visual-verdict ≥90 before its checkpoint; no checkpoint advances on red.
- **Expansion gate:** after the current UI/UX foundation passes tests, visual review, and role-story
  audits, new feature work must reuse the shared Work Hub, Object Explorer, Approval/Workflow, Policy,
  Scenario, and dense-table patterns. If a proposed screen needs a new pattern, document why before
  implementation.
