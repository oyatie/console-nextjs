# Benchmark Matrix — Module: `leave` (연차/휴가 관리)

Fixed-target source observation only; no browser, deployment, activation, or production-runtime validation was performed.

Scope: balances · requests · approvals · **사용촉진 (근로기준법 §61) compliance** · 원장(ledger).
Most-relevant vendors for this module: **Rippling, SAP SuccessFactors, Microsoft Teams (Approvals/Shifts)**. Foundry, Slack, Asana, n8n cover the periphery (workflow/automation substrate) and are scored honestly where they touch, N/A'd where they don't.

Rigor legend: **[V]** = verified against a cited source, **[I]** = inferred from the vendor's known product patterns (honest, unproven).

---

## Our console — evidence base (grepped, not assumed)

Read from `web/src/console/leave/{LeaveConsole.tsx,model.ts}`, `backend/app/src/hr.rs`, `docs/program/console-program-ledger.md`.

- **Implemented UI surface is deep, but not end-to-end shipped**: a drillable stat bar; 내 연차 self-service; 팀 결재함; 사용촉진 회차 states; and 인원별 연차 원장 are co-located. Source-wired self and managed request reads, request creation, exact-charge resolution, decisions, balances, and 촉진 controls call real leave endpoints in source. Closed-loop real-backend persona E2E and production proof are absent.
- **Backend and UI, source-wired**: `GET /api/v2/me/leave`, `GET|POST /api/v2/leave/requests`, `POST /api/v2/leave/requests/{id}/decide`, `POST /api/v2/leave/requests/{id}/charge-resolution`, `GET /api/v1/leave/balances`, and `POST /api/v1/leave/promotions` are mounted in source and called by `LeaveBody.tsx`. The deployed v1 request-list/decision response shape is a frozen compatibility contract; the additive exact-charge and version-fenced shape is isolated under v2.
- **Round-labelled notice/receipt substrate**: the domain validates promotion round labels `1|2` and normalizes refusal to round `2`; the Postgres adapter records idempotent, receipt-gated notices. It does not enforce statutory deadlines or round sequencing, and a refusal does not prove a persisted round-2 notice preceded it.
- **PBAC**: `LEAVE_ACTIONS` + `LEAVE_RUNTIME_GATE` (allow-list stub today; Cedar `authorize()` wire-pending, same pattern as AutomatePage).
- **Honest gaps**: the closed loop lacks real-backend persona E2E proof across creation, charge review/resolution, decision, promotion, audit, retry, and failure behavior. Statutory timing and round-sequence enforcement are absent. No automatic accrual engine (balances are imported/summed, not earned by rule). No 근무일/holiday calendar (calendar-day count only). No calendar/timeline view. No mobile leave surface yet. Leave ObjectCard audit history is unwired (`history: []`).

Net: **rich domain UI with source-observed reads and material source-observed mutations, but still PARTIAL.** Closed-loop real-backend E2E, production evidence, accrual/payroll coupling, and statutory timing/sequence enforcement remain gaps.

---

## Capability matrix (rows = dimensions, cells = HOW, 1-3 lines)

### 1. Information architecture / object model

| Vendor | How |
|---|---|
| **Our console** | The source defines request, ledger, and promotion concepts, but only ledger rows currently open an ObjectCard. Request/promotion drill-through and server-backed audit history remain unwired. [V] code |
| **Foundry** | No leave module. Generic ontology/object-graph platform; you'd *model* a LeaveRequest object type yourself over integrated HR source data. [I] |
| **Slack** | No object model — a PTO request is a Workflow Builder form + a message in a channel. [V] slack.com PTO template |
| **Teams** | Approvals app = a structured "approval" record; Shifts = a time-off request tied to a schedule. Two disjoint object types, no unified ledger. [V] learn.microsoft.com |
| **Asana** | No leave object. Modeled as tasks in an "Out of Office" project or via 3rd-party (Calamari) integration. [V] forum.asana.com |
| **n8n** | No object model; leave = rows in Google Sheets/Airtable that a workflow reads/writes. [V] n8n.io HR templates |
| **Rippling** | First-class Time Off policy + per-employee balance objects inside a unified employee graph (HR/payroll/IT). [V] rippling.com/blog/leave-management |
| **SAP SF** | Rich EC Time Off model: Time Type, Time Account, Accrual, Absence, holiday calendar, work schedule — deeply typed, deeply configurable. [V] help.sap.com |

### 2. Balance & accrual engine

| Vendor | How |
|---|---|
| **Our console** | Balance reads call `/api/v1/leave/balances` in source (grant/used/left); decisions call the backend endpoint in source. **No rule-based accrual engine yet.** [V] LeaveBody.tsx / leave REST |
| **Foundry** | N/A as engine; could compute balances as derived object properties/pipelines, but nothing out-of-box. [I] |
| **Slack** | None — balance is a number a human types into the form or tracks in a sheet. [V] |
| **Teams** | None native; Shifts shows availability, not an accrued balance. [I] |
| **Asana** | None native; Calamari add-on carries the balance. [V] |
| **n8n** | "Check against leave balance" = a Sheet lookup node; you build the arithmetic. [V] |
| **Rippling** | Full accrual engine: accrual schedules, upfront grants, unlimited, carryover, tenure tiers; real-time balances feed scheduling + payroll. [V] rippling.com/blog/pto-accrual |
| **SAP SF** | Accrual calendars run automatically; seniority-based averaging (MY/ID/VN), accrual in weeks/days/hours, recalculation options, prorating. Substantial engine here. [V] help.sap.com accruals |

### 3. Request creation / self-service

| Vendor | How |
|---|---|
| **Our console** | 내 연차 validates the request, submits through `POST /api/v2/leave/requests` with an idempotency key, preserves half-day intent, and updates the UI from the authoritative POST response. Real-backend persona E2E and production proof remain absent. [V] LeaveConsole.tsx / LeaveBody.tsx / leave REST |
| **Foundry** | N/A (no requester UX). [I] |
| **Slack** | Slash-command/form PTO request from within Slack; low-friction, no balance check. [V] slack.com template |
| **Teams** | Employee submits via Shifts "Time off" request or an Approvals card with dates+reason. [V] support.microsoft.com |
| **Asana** | Create a task in the OOO project / set vacation indicator + return date. [V] asana.com/resources |
| **n8n** | `/leave` Slack command triggers the workflow; n8n is the plumbing, Slack is the form. [V] |
| **Rippling** | Self-service portal: view accrual, upcoming leave, submit — with instant manager notify. [V] |
| **SAP SF** | Employee self-service Time Off tile with holiday/work-schedule-aware day counting. [V] |

### 4. Approval workflow + Separation of Duties

| Vendor | How |
|---|---|
| **Our console** | Team decision controls call the mounted backend path in source, which enforces `decider≠requester` against the authoritative requester in-tx. Closed-loop E2E proof, including failure/retry behavior, is still absent. [V] LeaveBody.tsx / leave adapter |
| **Foundry** | N/A native; could route via Actions but no approval primitive for leave. [I] |
| **Slack** | Single approver via Workflow Builder button/Approvals; no SoD guard, no multi-step chain. [V] |
| **Teams** | Approvals app = auditable single/parallel approval; Power Automate "Start and wait for approval" for multi-stage. No native SoD rule. [V] learn.microsoft.com |
| **Asana** | Task assignee/approval task type; not a real leave-approval control. [I] |
| **n8n** | Arbitrary multi-step routing (IF/Router → manager lookup); you code SoD if you want it. [V] n8n.io |
| **Rippling** | Configurable approval workflows per policy; auto-forward to manager; org-chart-aware routing. [V] |
| **SAP SF** | Workflow config (user-triggered vs admin-triggered), multi-approver chains, dynamic groups/escalation. [V] help.sap.com |

### 5. 사용촉진 compliance (근로기준법 §61) — Korean-specific

| Vendor | How |
|---|---|
| **Our console** | The repository has a round-labelled notice/receipt substrate: the domain accepts promotion labels `1` and `2`, the adapter records idempotent receipt-gated notices, and the console calls the promotion endpoint in source. It enforces neither statutory timing nor round sequencing, and refusal does not prove a prior round `2`; this is not a native §61 FSM or a shipped end-to-end vendor lead. [V] backend/domain evidence |
| **Foundry** | N/A — no legal knowledge; you'd model §61 rounds yourself. [I] |
| **Slack** | N/A — no compliance concept. [V] |
| **Teams** | N/A native; DIY via Power Automate + reminders, no §61 semantics. [I] |
| **Asana** | N/A. [I] |
| **n8n** | N/A native; buildable as scheduled reminder + receipt-log workflow, no legal model. [I] |
| **Rippling** | Strong US/global compliance (FMLA, statutory leave) but **§61 사용촉진 서면촉구/수령확인 is not a native Korean primitive** — Korea Payroll/EOR coverage is partial. [I] |
| **SAP SF** | Country-specific accrual rules incl. Asia; but §61 촉진 회차/수령확인 documents are typically **custom BRF/extension work, not shipped**. [I] |

### 6. Permissions / RBAC → PBAC

| Vendor | How |
|---|---|
| **Our console** | Persona lenses use advisory client `PolicyGated`/`LEAVE_ACTIONS`; Cedar `authorize()` is wire-pending and ADR-0021 does not switch live authorization. Current row isolation remains the server/RLS boundary. [V] model.ts + ADR-0021 |
| **Foundry** | Source-cited object/row-level security + attribute policies (Markings), but not leave-specific. [I] |
| **Slack** | Channel membership only; no field/action-level authz. [V] |
| **Teams** | Approvals admin + manager role; RBAC coarse, tied to team/schedule ownership. [V] approval-admin |
| **Asana** | Project/team membership; no HR-grade authz. [I] |
| **n8n** | Credential/workflow-level access; no per-record authz. [I] |
| **Rippling** | Granular role-based permissions across HR data; approval scoping by org. [V] |
| **SAP SF** | RBP (Role-Based Permissions) — very granular target-population + field-level. [V] help.sap.com |

### 7. Automation hooks

| Vendor | How |
|---|---|
| **Our console** | Ontology automations/series on object transitions (design); 대근/cover-planner cron in backlog. Not yet firing on leave events. [V] ledger |
| **Foundry** | Actions + functions on object edits; strong but you build it. [I] |
| **Slack** | Workflow Builder triggers (form submit, emoji, schedule). [V] |
| **Teams** | Power Automate: on-approve post to channel, remove from schedule, notify. [V] |
| **Asana** | Rules (Flowsana) shift due dates around PTO/holidays. [V] forum.asana.com |
| **n8n** | **Automation-focused product** — broad cited automation graph; 248 HR automation workflows (community catalog, live count), arbitrary integrations. [V] n8n.io/workflows/categories/hr |
| **Rippling** | "Recipes"/workflow automation (e.g. alert when request exceeds accrued hours); schedule-blocking on approve. [V] rippling.com/recipes |
| **SAP SF** | Business rules + Intelligent Services events on time-off. [I] |

### 8. Calendar / scheduling integration

| Vendor | How |
|---|---|
| **Our console** | No calendar/timeline view yet; no holiday calendar (calendar-day count only). Gap. [V] model.ts ponytail note |
| **Foundry** | N/A. [I] |
| **Slack** | Posts to channel; Google Calendar via workflow, not native. [V] |
| **Teams** | Shifts schedule is the calendar; on approve the employee is marked **unavailable (gray "Off" indicator)** — the assigned shift is **NOT auto-removed** ("This is a designed behavior"); auto-remove / reassign-to-open-shift requires a custom **Power Automate** flow. [V] learn.microsoft.com Q&A |
| **Asana** | Timeline/calendar shows OOO tasks; core competency. [V] |
| **n8n** | Writes to Google Calendar on approve. [V] |
| **Rippling** | Team PTO calendar + schedule-blocking; feeds workforce/shift planning. [V] |
| **SAP SF** | Time-off calendars, team absence calendar, holiday/work-schedule aware. [V] help.sap.com calendars |

### 9. Mobile

| Vendor | How |
|---|---|
| **Our console** | Native field app exists but **no leave surface yet** (mobile leave = backlog). Gap. [V] ledger |
| **Foundry** | Mobile app, not for leave. [I] |
| **Slack** | Full mobile — request/approve in the Slack app. [V] |
| **Teams** | Full mobile — Shifts + Approvals on phone (frontline-worker focus). [V] |
| **Asana** | Mobile app for tasks/OOO. [V] |
| **n8n** | No end-user app (mobile UX = whatever Slack/Teams front-end you wire). [I] |
| **Rippling** | Mobile self-service request/approve/balances. [V] |
| **SAP SF** | Mobile ESS/MSS time-off. [V] |

### 10. Audit / compliance trail

| Vendor | How |
|---|---|
| **Our console** | Decision and promotion POSTs traverse audited backend paths, but no closed-loop E2E proves creation through receipt, failure, and retry; Leave ObjectCard audit history is currently unwired (`history: []`). [V] leave adapter, model.ts |
| **Foundry** | Full lineage/audit on object edits. [I] |
| **Slack** | Message history only; weak audit. [V] |
| **Teams** | Approvals app touts "auditing, compliance, accountability" — approval record retained. [V] approval-admin |
| **Asana** | Task activity log. [I] |
| **n8n** | Execution logs per run. [V] |
| **Rippling** | HR-grade audit on approvals/changes. [I] |
| **SAP SF** | Audit trail + recalculation history; enterprise-grade. [I] |

### 11. Analytics / reporting

| Vendor | How |
|---|---|
| **Our console** | Drillable stat bar (소진율/촉진대상) = inline analytics; dashboard module for deeper. [V] LeaveConsole.tsx |
| **Foundry** | Source-cited analytics/quiver over the object graph. [I] |
| **Slack** | None. [V] |
| **Teams** | Minimal (request counts). [I] |
| **Asana** | Dashboards/reporting on OOO tasks. [I] |
| **n8n** | None native (pipe to a BI tool). [I] |
| **Rippling** | Real-time PTO analytics + liability reporting. [V] rippling.com/blog/leave-management |
| **SAP SF** | Time reporting, accrual liability, Stories/People Analytics. [I] |

### 12. Extensibility / no-code type creation

| Vendor | How |
|---|---|
| **Our console** | Target: a new object type should wire itself end-to-end (instances/module/policy/automation/graph/i18n) without code; current end-to-end wiring is incomplete and remains a ledger backlog item. [I] ledger 2026-07-10 |
| **Foundry** | A selected reference for no-code object-type modeling, but ships **zero** domain types. [I] |
| **Slack** | Workflow Builder = low-code forms only. [V] |
| **Teams** | Power Apps/Power Automate for custom leave apps. [V] teamswork.app |
| **Asana** | Custom fields/templates; app integrations. [I] |
| **n8n** | Infinitely extensible plumbing; no data-model layer. [V] |
| **Rippling** | Configurable policies, custom fields; not open object modeling. [I] |
| **SAP SF** | Deeply configurable (MDF objects, business rules) but consultant-heavy, not no-code. [I] |

### 13. Korean 전자결재 / 법인 scoping fit

| Vendor | How |
|---|---|
| **Our console** | Targeted for it: Korean-first UI, 전자결재-style queue, organization→branch scoping, and round-labelled notice/receipt primitives. Request creation is source-wired; statutory timing/sequence enforcement, closed-loop E2E, and Cedar promotion remain gaps. [V/I] |
| **Foundry** | Neutral platform; no Korean HR/legal content. [I] |
| **Slack/Teams/Asana/n8n** | Global-generic; **미스매치** — no 전자결재 승인선/전결규정, no 근로기준법, no 법인 scoping; Korean HR teams bolt on 시프티/플렉스 instead. [I] |
| **Rippling** | US-centric; Korea via EOR/partners, weak 전자결재 + §61. [I] |
| **SAP SF** | Localizable to Korea but §61 촉진/전결규정 = custom implementation project. [I] |

---

## Per-vendor: "how they'd build OUR leave module"

**Foundry** — Would model 연차원장/신청/촉진회차 as first-class object types with row-level Markings for 법인 scoping, and drive every transition through Actions + Functions, with lineage-perfect audit. It would *nail* the ontology and security spine — which is exactly the spine we already copied. What it would never ship is the §61 legal content, the accrual math, or a Korean 결재함; Foundry hands you the loom, not the cloth. Our module is essentially "Foundry-for-Korean-HR with the domain types pre-woven."

**Slack** — A PTO Workflow Builder form + an Approvals message, balance tracked in a linked sheet. Fast, delightful, zero compliance. Their version optimizes the 30 seconds of request+approve and ignores accrual, ledger, and §61 entirely. Steal the *friction* (request-in-the-conversation), not the model.

**Microsoft Teams** — Shifts (frontline schedule) + Approvals app + Power Automate glue: submit time-off, **mark the employee unavailable (gray "Off") on approve** (auto-remove / reassign-to-open-shift needs a Power Automate flow), post to channel, retain an auditable approval record. Strong for shift workers and audit-of-approval, but leave "balance/accrual" and §61 live outside it in a custom Power App. Their strength is **schedule-coupling** (approve → unavailable) which our 대근/cover-planner backlog should copy — though the *fully* coupled "approve auto-clears the roster slot" reference is **Rippling**, not Teams.

**Asana** — An "Out of Office" project of tasks on a timeline, vacation indicators, Flowsana rules shifting due dates around PTO. It treats leave as *capacity planning*, not HR compliance — genuinely good at "who's out and how does that move the work," which our workforce/dispatch surface could borrow, but it is not a system of record for balances or §61.

**n8n** — Wouldn't build a UI at all: a graph — Slack `/leave` → balance lookup → manager approval button → update calendar/payroll — with the org-chart→manager mapping in Airtable so routing survives reorgs. Their lesson for us is the **externalized routing table** (approvers as data, not code) — directly relevant to our 승인선/전결규정 automation.

**Rippling** — A selected peer comparator: policy-driven accrual engine, real-time balances, self-service portal, org-aware approval routing, schedule-blocking on approve, and PTO analytics/liability — all fused with payroll so an approved day flows to pay automatically. Their version is everything our runtime is *not yet*: fully wired accrual→approve→payroll. What they lack is §61 촉진 and 전자결재 승인선. If we wire our FSM to payroll the way Rippling does, plus keep §61, the combined design is a Korea-specific recommendation, not a verified superiority claim.

**SAP SuccessFactors** — Feature-rich and implementation-intensive in the cited surface: Time Types, Time Accounts, automatic accrual calendars, seniority tiers, holiday/work-schedule-aware counting, RBP field-level security, multi-step workflows, recalculation. Their version handles a broad set of documented accrual cases — but §61 촉진 회차/수령확인 문서 is a consulting deliverable, and the whole thing needs an implementation team. We copy the **accrual-calendar + work-schedule-aware day counting** concepts without the SI cost.

---

## What we'd steal — ranked (capability → source-cited → fit with our ontology-first grammar → cost)

1. **Rule-based accrual engine** (accrual schedules, upfront grant, tenure tiers, carryover) → **Rippling / SAP SF** → fits as a `dynamic`-layer derivation on the 연차 원장 object + a nightly accrual series; replaces today's imported-balance stub. This is the single biggest runtime gap. **Cost: L**
2. **Close the already source-wired request loop** (prove request→charge review/resolution→decision→promotion→receipt, audit, failure, retry, and idempotency with real-backend persona E2E and production evidence) → **Rippling** (their approve→balance→payroll flow is the reference). The create, exact-charge, decision, and promotion call sites are present in source. **Cost: M**
3. **Work-schedule / holiday-aware day counting** (skip 휴일·비근무일 in the span) → **SAP SF** → a 근무표/holiday-calendar object feeding `requestDays()`; removes the `ponytail:` calendar-day approximation. **Cost: M**
4. **Schedule-coupling on approve** (approved leave → unavailable in dispatch/대근 planning) → **Rippling** (fully coupled — approve clears the roster slot; Teams only marks unavailable, auto-remove needs Power Automate) → an automation on the APPROVED transition writing to the workforce/cover-planner surface; leverages the 대근 cron already in backlog. **Cost: M**
5. **Externalized approval-routing table** (승인선/전결규정 as data, org-chart-aware, survives reorgs) → **n8n / Rippling** → model 전결규정 as an ontology instance type the `appr` engine reads; no hard-coded approver. **Cost: M**
6. **Request-in-the-conversation** (submit/approve from the CommsRail/messenger, not just the module) → **Slack / Teams** → an Approvals-style card in our messenger bound to the same FSM action; low friction, high adoption. **Cost: S**
7. **PTO liability / burn analytics + team absence calendar** → **Rippling / SAP SF** → extends the existing drillable stat bar with a liability rollup + a timeline view (the missing calendar surface). **Cost: M**
8. **Mobile self-service leave** (request/approve/balance on the native field app) → **Rippling / Teams** → add a leave tab to the pending mobile app against the same REST. **Cost: M**

**What is differentiated and worth defending:** the repository's round-labelled notice/receipt substrate, backend SoD invariant, exact-charge command path, object/audit target shape, and organization→branch scoping. This is not yet a shipped differentiator: statutory timing/sequence enforcement, Cedar promotion, closed-loop real-backend E2E, and production evidence remain required.

---

Sources: [Rippling leave management](https://www.rippling.com/blog/leave-management), [Rippling PTO accrual](https://www.rippling.com/blog/pto-accrual), [Rippling PTO-exceeds-accrued recipe](https://www.rippling.com/recipes/pto-request-exceeds-accrued-hours-alert), [SAP SF Time Off config guide](https://help.sap.com/docs/successfactors-employee-central/time-off-and-leave-of-absence-configuration-guide-getting-started/accruals-based-on-recorded-times), [SAP handling leave absences](https://learning.sap.com/courses/sap-successfactors-time-management-academy/handling-leave-absences), [Teams Approvals admin](https://learn.microsoft.com/en-us/microsoftteams/approval-admin), [Teams manage time off in Shifts](https://support.microsoft.com/en-us/office/manage-shift-requests-and-time-off-in-shifts-231fc82f-db7f-4f06-9215-8b36b599d69c), [Asana OOO/time-off](https://forum.asana.com/t/holiday-leave-block-off-time-either-as-pto-holiday-or-all-day-task/20302), [Asana + Calamari](https://asana.com/apps/calamari), [n8n HR workflows](https://n8n.io/workflows/categories/hr/), [Slack PTO request template](https://slack.com/templates/time-off-request-process). Our-console claims: `web/src/console/leave/{LeaveConsole.tsx,model.ts}`, `backend/app/src/hr.rs`, `docs/program/console-program-ledger.md`.

---

## Cross-cutting lens findings (5 independent review lenses)

- **Task-flow:** the leave console co-locates source-wired request creation, team-decision calls, 촉진 calls, and ledger views. The closed loop still lacks real-backend E2E proof; prove audit, failure, retry, and idempotency before using this as the propagation reference. Cost **M**.
- **IA / layout:** `LeaveConsole` is real; Korean statutory annual-paid-leave accrual and use rules are distinct from half-day or quarter-day slicing. 반차/반반차 is a workplace-agreement or policy design choice, not a standalone statutory mandate. Routing leave through 전자결재 is likewise a product/workplace-control choice. **Steal:** team calendar-grid leave board [M], statutory accrual-aware balance [M], and optional request→결재선 handoff [S].
- **Data-model:** the backend enforces the request-decision FSM and `decider≠requester`; the separate 촉진 substrate labels rounds and records receipt-gated notices without enforcing timing or sequence. **Weaker:** Workday ships effective-dated accrual balances; ours lacks that object and closed-loop E2E proof. **Steal:** Workday effective-dated accrual/balance object [M]; carryover/expiry slices [S].
- **Governance:** the round-labelled notice/receipt substrate is differentiated groundwork, not statutory timing/sequence enforcement or a proven production lead. **Steal:** make 수령확인 a reusable governed acknowledgment object type, retain the existing inbox/leave flow, and add deadline plus round-sequence enforcement after the base path is E2E-proven [M].
- **Automation / extensibility:** we have effective-dating + actions; missing HR-event triggers. **Steal:** HR-event lifecycle triggers (on-approve-leave → balance decrement / 연차촉진 round) [S–M]; 연차촉진 round scheduler + 촉진 통보 notification effect [M]; org-scope routing modifiers [M].

**Adjudicated contradictions:** (1) **Teams Shifts does NOT auto-remove from schedule on approve** — its own cited MS source says the employee is marked unavailable (gray "Off"); auto-remove needs Power Automate. The fully-coupled "approve clears the roster slot" reference is **Rippling**, not Teams (Row 8, per-vendor, and Steal #4 corrected above). (2) **노무수령거부** has a built backend notice/receipt path (`crates/leave/domain`, migrations `0123`/`0119`) and `labor_refusal` is one of the 9 published governed-config types. Neither registration nor the path proves statutory timing/sequence, and the flow is **not** in the FE `model.ts` the draft cited (Row 5 corrected).
