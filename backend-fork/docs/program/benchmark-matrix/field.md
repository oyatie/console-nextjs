# Benchmark Matrix — Module: **field** (field ops: dispatch, work orders, mobile handoff, geofence check-in)

> **Benchmark evidence metadata**
> - Observation/revalidation date: 2026-07-19.
> - Sampled products/surfaces: Oyatie legacy dispatch, backend FSM, and Android field source; SAP Field Service Management, S/4HANA, and SuccessFactors; Rippling Time Clock; Asana mobile/tasks; Palantir Workshop/Ontology; Slack; Microsoft Teams Frontline; n8n.
> - Evidence modality: Fixed-target repository source plus live-checked public official documentation/product pages and explicitly labeled public secondary pages; hands-on product tenants, screenshot capture, deployment, activation, and production validation were not performed.
> - Scope/claim ceiling: Only the named pages, surfaces, and fixed-target source are in scope; no whole-product, current-production, provider-parity, universal-superiority, legal, tax, labor, deployment, activation, or production conclusion.
> - Legend: [V] = bounded external observation with a direct URL or same-document source-list entry; [E]/[code] = fixed-target repository observation; [I] = recommendation or inference. Every steal/adopt item is [I].

Compared: **Our Console** vs SAP (FSM + S/4HANA + SuccessFactors), Rippling, Asana, Palantir Foundry, Slack, Microsoft Teams, n8n.
Rigor: every vendor cell is `[V]` VERIFIED (source URL) or `[I]` INFERRED (reasoned from known product patterns). Our column is code-grounded (paths cited).

**Most-relevant vendors for this module:** SAP FSM (purpose-built field service), Rippling (time/geo clock-in), Asana (mobile task assignment), Palantir (ontology-driven ops apps). Slack/Teams/n8n cover the collaboration + automation edges; none is a field-ops system of record (flagged per row).

---

## Our Console — evidence base (grep'd, not assumed)

> ⚠️ **Layer note (critical for reading this doc):** the DispatchBoard / DispatchMapPage / MechanicDispatchOffers / WorkOrderDispatchControls / SlaBadge surfaces below all live in `web/src/features/dispatch/*` and `web/src/pages/DispatchMapPage.tsx` — the **legacy react-router app** (mounted via `AppRouter`), **not** the new ontology console. `web/src/console/` has **no** dispatch/field/workorder subdirectory. The **NEW ontology console ships NO dispatch board, no schedule board, no map, no drag-drop dispatch** — WO- falls back to the generic kanban lanes (ia-layout §13 + task-flow §13, both code-confirmed). So the SAP-FSM-Gantt and ServiceNow-Dispatcher-Workspace comparisons below are against a **superseded legacy screen slated for Phase-D re-implementation in the ontology console**, not a shipped console surface. The backend `crates/dispatch` 16-state FSM and the Android offline field app are legit and unaffected.

- **Dispatch board** (legacy `features/dispatch`): `web/src/features/dispatch/DispatchBoard.tsx` — kanban grouped by WO status into 6 lanes (received/assigned/active/review/blocked/done), Korean labels via `ko.dispatch`.
- **WO lifecycle FSM**: `backend/crates/dispatch/*` + `DispatchBoard` status set = `RECEIVED, UNASSIGNED, ASSIGNED, IN_PROGRESS, TEMPORARY_ACTION, REPORT_SUBMITTED, ADMIN_REVIEW, ON_HOLD, DELAYED, PART_WAITING, EQUIPMENT_IN_USE, REVISIT_REQUIRED, FINAL_COMPLETED, REJECTED, ARCHIVED, CANCELLED` — a maintenance-native 16-state model, every transition an audit event (ledger §77 kinetic layer).
- **Dispatch controls**: `WorkOrderDispatchControls.tsx` — set priority (P1/P2/P3/OUTSOURCE), request schedule (target-due) change w/ reason, multi-mechanic assign (PRIMARY/SECONDARY), P1 force-assign, create outsource work; manager-only, backend re-checks authz per call.
- **P1 emergency dispatch**: `MechanicDispatchOffers.tsx` + `backend/crates/dispatch/{worker,domain}` — broadcast-accept FSM, GPS proximity scoring (`console_kernel_core::haversine_meters`), escalation timers, FCM push carrying dispatch id; mechanic accept/decline via `/responses` (HANDOFF M2 T2.4/T2.5, E2E-proven).
- **Geofence/arrival**: `0041_add_site_geofence_radius.sql` (per-site radius, default 300m), `DispatchMapPage.tsx` (Leaflet map, Korean-terrain center, ArrivalEvent markers, directions link), `LocationPing` w/ consent (`compliance/domain`, coord-range validated), Android `LocationConsentStateMachine`.
- **SLA**: `features/dispatch/sla.ts` + `SlaBadge.tsx` — on-track/at-risk(≤30m)/breached from `target_due_at`; org rollup via `OpsSummary.sla_at_risk/breached`.
- **Evidence**: `WorkOrderEvidenceList.tsx`, `EvidenceUpload.tsx`; Android `CameraCaptureScreen.kt` + evidence module (RustFS-backed).
- **Mobile app** (`android/app/.../field`): tabs Today / WorkHub / Messenger / Operations; **offline mutation queue** (`data/offline/*`: `MutationQueueStore`, `OfflineQueueRepository`, `ConnectivityReplayScheduler`, idempotent `RequestIdFactory`), passkey step-up for sensitive actions, WO approve from mobile.
- **Permissions**: current field access uses legacy server/role checks plus evidenced RLS; `ADMIN/EXECUTIVE/SUPER_ADMIN` gates appear on dispatch/equipment surfaces. Cedar PBAC and residual→SQL lowering remain target/shadow until per-action enrollment and promotion under ADR-0021.
- **Audit/gov**: append-only effective-dated event log, org-scoped FORCE-RLS, three-layer ontology object tracking (semantic/kinetic/dynamic) for WO- objects.

---

## Capability Matrix (rows = dimensions, cells = how each does it)

### 1. Information architecture (the dispatch surface)

| | How |
|---|---|
| **Our Console** | Status-kanban board (6 collapsed lanes over 16 states) + separate Leaflet dispatch map; WO opens as 3-layer ObjectCard. Board + map are distinct surfaces. `DispatchBoard.tsx`. |
| **SAP FSM** | [V] Graphical **Dispatcher/Planning Board** — Gantt-style timeline of technicians × assignments, drag-to-schedule, manual/semi-auto/auto modes. ([help.sap.com](https://help.sap.com/docs/SAP_FIELD_SERVICE_MANAGEMENT/fsm_plan_dispatch/plan-dispatch-overview.html)) |
| **Rippling** | [I] No dispatch board — IA is time-entry/approval lists + org-graph; field surface is the clock-in screen, not a work queue. ([rippling.com](https://www.rippling.com/mobile-time-clock)) |
| **Asana** | [V] Board/List/Timeline/Calendar views of tasks; a "project" is the closest thing to a dispatch queue, columns = custom statuses. ([asana.com](https://asana.com/features/workflow-automation/rules)) |
| **Palantir** | [V] Workshop apps compose Object Explorer + Object Views + widgets over the Ontology; the "board" is whatever the builder assembles — no canned dispatch board. ([palantir.com](https://www.palantir.com/docs/foundry/ontology/applications)) |
| **Slack** | [I] No board; a dispatch "queue" is a channel + messages/threads. Ephemeral, not stateful. |
| **MS Teams** | [V] Frontline hub pins Shifts + Tasks (Planner) + Approvals + Walkie Talkie; Tasks/Planner is the assignment surface, not a routing board. ([learn.microsoft.com](https://learn.microsoft.com/en-us/microsoft-365/frontline/flw-team-collaboration)) |
| **n8n** | N/A — automation engine, no operator UI for field ops (canvas is for building flows, not dispatching work). |

### 2. Work-order lifecycle / state model

| | How |
|---|---|
| **Our Console** | Domain-owned 16-state FSM incl. maintenance-native states (TEMPORARY_ACTION 응급조치, PART_WAITING 부품대기, REVISIT_REQUIRED 재방문, EQUIPMENT_IN_USE); every transition audited, version history + as-of. `dispatch/domain`. |
| **SAP FSM** | [I] Full service-order lifecycle: create→plan→dispatch→execute→confirm→invoice, asset- and customer-service variants in one model; status-driven. ([sap.com/features](https://www.sap.com/products/scm/field-service-management/features.html)) |
| **Rippling** | N/A — no work-order object; unit of work is a time entry / job code, not a serviceable order. ([rippling.com glossary](https://www.rippling.com/glossary/time-and-attendance)) |
| **Asana** | [V] Task status = custom fields / sections; no domain lifecycle — you model states as columns/fields yourself. Rules fire on status change. ([asana.com](https://asana.com/features/workflow-automation/rules)) |
| **Palantir** | [V] Lifecycle = Ontology Action Types (create/modify/link/delete) gated by rule sets; you author the states as object properties + allowed actions. No built-in WO type. ([palantir.com](https://www.palantir.com/docs/foundry/workshop/actions-use)) |
| **Slack** | [I] State lives in message/workflow variables; no persistent object lifecycle. |
| **MS Teams** | [I] Planner task states (Not started/In progress/Completed) only — coarse, no domain states. |
| **n8n** | [I] Stateless per-execution; "state" = whatever you persist to an external store between webhook calls. |

### 3. Dispatch & assignment (manual / auto / broadcast)

| | How |
|---|---|
| **Our Console** | Manual multi-mechanic assign (PRIMARY/SECONDARY) + **P1 broadcast-accept** w/ GPS proximity scoring + escalation timers + manager force-assign + outsource route. `WorkOrderDispatchControls` + `dispatch/worker`. |
| **SAP FSM** | [I] Manual, semi-automatic, and **fully automatic** AI scheduling on the dispatch board, matched on skills + availability; plus **Crowd Service** to broadcast to external/partner technicians. ([sap.com](https://www.sap.com/products/scm/field-service-management.html)) |
| **Rippling** | [I] Job/shift assignment via scheduling, not skill-matched dispatch; clock-in prompts worker to pick job/customer. ([rippling.com](https://www.rippling.com/mobile-time-clock)) |
| **Asana** | [V] Rules auto-assign tasks by field/form data; single assignee per task (+ subtask assignees). No broadcast/claim model. ([asana.com forms](https://asana.com/features/workflow-automation/forms)) |
| **Palantir** | [V] Dispatch = an Action exposed via Button Group widget; "dynamic resource allocation tools optimize scheduling, routing, task prioritization for large-scale field ops" — but you build the optimizer. ([palantir.com](https://www.palantir.com/docs/foundry/workshop/actions-use)) |
| **Slack** | [I] Workflow Builder can route a request to a channel/person + escalate if no response — a claim/ack pattern, but not location-scored. ([slack.com](https://slack.com/help/articles/360035692513-Guide-to-Slack-Workflow-Builder)) |
| **MS Teams** | [I] Shifts open-shift request/swap + Planner assign; no auto/skill dispatch. ([learn.microsoft.com Shifts](https://learn.microsoft.com/en-us/microsoftteams/expand-teams-across-your-org/shifts/manage-the-shifts-app-for-your-organization-in-teams)) |
| **n8n** | [I] Can implement broadcast/round-robin in a flow (webhook→branch→notify), but no native assignee model or UI. ([n8n.io](https://n8n.io/integrations/webhook/)) |

### 4. Scheduling & SLA

| | How |
|---|---|
| **Our Console** | `target_due_at`-driven SLA (on-track/at-risk≤30m/breached) + board rollup counts; schedule-change requests carry reason (audited). No optimizer yet. `sla.ts`. |
| **SAP FSM** | [V] AI-based scheduling optimizer (skills/availability/route); SLA/response-time is a first-class planning constraint. ([help.sap.com plan-dispatch](https://help.sap.com/docs/SAP_FIELD_SERVICE_MANAGEMENT/fsm_plan_dispatch/plan-dispatch-overview.html)) |
| **Rippling** | [V] Enforces overtime/meal-break/labor-law rules by work location; schedules tied to payroll, not service SLAs. ([rippling.com](https://www.rippling.com/glossary/time-and-attendance)) |
| **Asana** | [V] Due dates + rules ("due date approaching" trigger); no SLA object, no routing optimizer. ([asana.com](https://asana.com/features/workflow-automation/rules)) |
| **Palantir** | [I] SLA = a computed property + Action guard you model; optimization via Foundry solvers/Quiver — powerful but bespoke. |
| **Slack** | [I] "Clear notification rules improve SLA performance" via reminders/digests/escalation — SLA as nudge, not tracked datum. ([slack.com blog](https://slack.com/blog/productivity/automate-tasks-in-slack-with-workflow-builder)) |
| **MS Teams** | N/A — no service-SLA concept; Shifts is schedule-adherence, not order-SLA. |
| **n8n** | [I] Cron/interval + wait nodes can implement SLA timers/escalation, but you wire the whole thing. |

### 5. Geofence / GPS check-in & location

| | How |
|---|---|
| **Our Console** | Per-site geofence radius (`0041`, default 300m, admin PATCH-able), haversine eval in kernel, consented LocationPing (coord-validated), arrival/departure events on the dispatch map. Consent state machine on Android. |
| **Rippling** | [V] **Source-cited**: set a GPS radius workers must be within to clock in; phone GPS logged at every punch; geofence verifies field clock-ins; labor-law enforcement by location. ([rippling.com](https://www.rippling.com/mobile-time-clock)) |
| **MS Teams** | [V] Shifts location detection: "on location" if clock in/out within **200m** of set location; export shows only true/false, not coordinates; stricter geofence needs Power Automate / WFM partner. ([support.microsoft.com](https://support.microsoft.com/en-us/teams/free/clock-in-and-out-with-shifts)) |
| **SAP FSM** | [I] Mobile app captures technician GPS for tracking/routing; geofenced check-in is typically via the mobile execution flow, not a headline feature. ([sap.com features](https://www.sap.com/products/scm/field-service-management/features.html)) |
| **Asana** | N/A — no location/geo capability. |
| **Palantir** | [I] Can model geofence as an Ontology geo-property + Action guard; GPS ingest via mobile/pipeline — bespoke, not turnkey. |
| **Slack** | N/A — no native geofence/clock-in (would need a custom app). |
| **n8n** | [I] Could compute haversine in a Function node on ping payloads; no capture surface. |

### 6. Mobile field execution (offline, evidence capture)

| | How |
|---|---|
| **Our Console** | Native Android app: offline mutation queue w/ connectivity replay + idempotent request ids, camera evidence capture, WO detail + approve, passkey step-up for sensitive ops. `data/offline/*`, `CameraCaptureScreen.kt`. |
| **SAP FSM** | [I] Mobile-first execution: guided workflows, **offline capabilities**, real-time collab, digital task completion for technicians. ([sap.com](https://www.sap.com/products/scm/field-service-management.html)) |
| **Rippling** | [V] Mobile app: web/mobile/kiosk clock-in, break tracking, job assignment at punch. Time-focused, not task-execution. ([rippling.com](https://www.rippling.com/mobile-time-clock)) |
| **Asana** | [I] Mobile app for task view/update/comment/attach; no offline-first queue or field evidence workflow. ([asana.com](https://asana.com/features/workflow-automation)) |
| **Palantir** | [I] Ontology-aware mobile via Workshop/mobile SDK; offline is limited/bespoke. Strong at data context, weak as a turnkey field app. |
| **Slack** | [V] Mobile approvals: approve/deny from notification, thread auto-updates; mobile request workflows. Comms, not execution. ([docs.slack.dev](https://docs.slack.dev/tools/deno-slack-sdk/tutorials/mobile-request/)) |
| **MS Teams** | [V] Mobile-first Shifts (clock in/out, breaks), Tasks, Walkie Talkie PTT over wifi/cell, Approvals — a real frontline mobile suite. ([learn.microsoft.com](https://learn.microsoft.com/en-us/microsoft-365/frontline/flw-team-collaboration)) |
| **n8n** | N/A — no mobile end-user app. |

### 7. Mobile handoff / shift handover

| | How |
|---|---|
| **Our Console** | WO reassign (PRIMARY/SECONDARY swap) + REVISIT_REQUIRED / TEMPORARY_ACTION states carry work across visits; HO- handover policy is a default catalog type (ledger §78). Handover as a governed object, evidence-backed. |
| **MS Teams** | [V] Shifts open-shift/swap/handoff requests, shift notes, Walkie Talkie for live handover; prominent turnkey shift-handover. ([learn.microsoft.com Shifts](https://learn.microsoft.com/en-us/microsoftteams/expand-teams-across-your-org/shifts/manage-the-shifts-app-for-your-organization-in-teams)) |
| **SAP FSM** | [I] Assignment reassignment on the dispatch board + mobile checklists carry state; shift-handover per se is more an S/4 EAM / shift-log concern. |
| **Rippling** | [I] Shift swap/schedule handoff exists on the scheduling side; no work-content handover. |
| **Asana** | [I] Reassign task + comment history = lightweight handoff; no shift concept. |
| **Palantir** | [I] Model a Handover object + Action; audit trail is native, but you build the UX. |
| **Slack** | [I] Handoff = channel thread + workflow; escalate-to-backup-if-no-response is a **third-party app (Wrangle)** feature, not Slack-native (native Workflow Builder offers only basic routing). ([wrangle.io](https://www.wrangle.io/post/managing-approval-workflows-in-slack)) |
| **n8n** | N/A. |

### 8. Permissions / scoping (RBAC → PBAC)

| | How |
|---|---|
| **Our Console** | Current field access is legacy server/role authorization plus evidenced FORCE-RLS org isolation. Cedar PBAC and deny-by-omission residual→SQL list filtering are target/shadow capabilities, not universal live field enforcement. |
| **SAP FSM** | [I] SAP authorization objects / BTP roles; mature but role-centric, coarser than attribute/row-level policy without custom work. |
| **Rippling** | [I] Role + org-graph-based permissions (HR/IdP heritage); good org scoping, not resource-attribute policy. |
| **Asana** | [V] Project/team membership + admin roles + guest access; no row/attribute policy engine. ([asana.com](https://asana.com/features/workflow-automation)) |
| **Palantir** | [V] Ontology security: object/property-level permissions, action rule sets, mandatory + role controls — selected comparator to our PBAC ambition. ([palantir.com](https://www.palantir.com/docs/foundry/architecture-center/ontology-system)) |
| **Slack** | [V] Workflow Builder connector-access admin controls; channel membership scoping. Not resource-policy. ([slack.com](https://slack.com/help/articles/16749280664595-Manage-access-to-Slack-Workflow-Builder-connectors)) |
| **MS Teams** | [I] Entra ID roles + team/channel membership; org-level, not per-object field policy. |
| **n8n** | [I] Instance/project RBAC + credential scoping (self-host); no domain resource policy. |

### 9. Automation hooks

| | How |
|---|---|
| **Our Console** | Automate hub (workflows/schedules/monitors) + BlockCanvas typed nodes, effect = ontology-action; declarative Action types → writeback (humans + automation fire the same action). Governed, in-platform. `console/canvas/*`. |
| **n8n** | [V] **Source-cited raw automation**: webhook triggers (unique URL per flow), HTTP-request node to any REST API, code nodes, self-hosted (no per-task fee, credentials stay on prem). ([n8n.io](https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.webhook)) |
| **Asana** | [V] Rules: rich trigger/condition/action library (up to many conditions), form→task automation, cross-tool integrations. Strong no-code. ([asana.com rules](https://asana.com/features/workflow-automation/rules)) |
| **Slack** | [V] Workflow Builder: triggers→steps, conditional branching (up to **15 branches, ≤10 rules each**, Business+/Grid plans only), connectors to external systems, mobile-friendly. ([slack.com](https://slack.com/features/workflow-automation)) |
| **SAP FSM** | [I] Business rules / workflow config + BTP integration suite; powerful but developer-heavy. |
| **Palantir** | [V] Actions + Automate (scheduled/triggered) + Functions; automation and humans invoke the same Action types. ([palantir.com](https://www.palantir.com/docs/foundry/workshop/actions-use)) |
| **Rippling** | [I] Workflow automations across HR/IT/Finance triggers; not field-ops-shaped. |
| **MS Teams** | [I] Power Automate flows (incl. geofence enforcement pattern) — external engine, not native. ([techcommunity](https://techcommunity.microsoft.com/discussions/microsoftteams/geofencing-shifts-or-time-clock/763625)) |

### 10. Audit / compliance

| | How |
|---|---|
| **Our Console** | Append-oriented, effective-dated event seams and as-of reconstruction exist on evidenced paths. Audit seal/verify code is partial/DARK: production sealing is OFF, the in-memory signer is not a trust root, NULL-org rows are excluded, and universal FSM coverage is not proved. |
| **SAP FSM** | [I] Enterprise audit via S/4 + change docs; mature but configuration-dependent. |
| **Palantir** | [V] Ontology edits go through Actions (recorded), lineage/provenance native; strong auditability. ([palantir.com](https://www.palantir.com/docs/foundry/ontology/overview)) |
| **Rippling** | [I] Payroll/time audit trails for labor compliance; scoped to HR domain. |
| **Asana** | [I] Task activity log + admin audit log (enterprise); not tamper-evident. |
| **Slack** | [I] Audit Logs API (enterprise grid); message-level, not domain-object lifecycle. |
| **MS Teams** | [V] Time-report export (true/false location, hours) for payroll; Purview audit for messages. ([support.microsoft.com](https://support.microsoft.com/en-us/office/clock-in-and-out-with-shifts-ae7b676c-7666-46c7-9f68-85ff54acec8b)) |
| **n8n** | [I] Execution logs per run; no domain audit. |

### 11. Extensibility / customization

| | How |
|---|---|
| **Our Console** | No-code add-anything (row/column/stat/type/action) through governed draft→approve→effective; new ontology type wires itself end-to-end (surface/policy/automation/graph). `console/ontology/*`, ledger §78. |
| **Palantir** | [V] **Source-cited**: Workshop no/low/pro-code widgets over a fully customizable Ontology; build any operational app. ([palantir.com applications](https://www.palantir.com/docs/foundry/ontology/applications)) |
| **Asana** | [V] Custom fields, forms, task templates, rules, app integrations per project. No-code, but bounded to task model. ([asana.com](https://asana.com/features/workflow-automation/forms)) |
| **SAP FSM** | [I] Extensible via BTP extensions, custom fields, smartforms/checklists; developer effort high. |
| **n8n** | [V] Custom nodes + code nodes + any HTTP API; infinitely extensible for logic, nothing for UI. ([n8n.io features](https://n8n.io/features/)) |
| **Slack** | [I] Custom apps (Deno SDK), Block Kit, connectors; extend comms, not a data model. |
| **MS Teams** | [I] Custom apps/tabs, Power Platform; frontline templates fixed. |
| **Rippling** | [I] Custom fields/policies within HR modules; not an app builder. |

### 12. Korean B2B operations fit (전자결재 / 근로기준법 / group scoping)

| | How |
|---|---|
| **Our Console** | Native: 전자결재-style approval (AP-) + four-eyes/SoD guards, 근로기준법-aware attendance/leave, Group→법인→branch→worksite scoping, Alimtalk/KCC hooks (operator-templated), Korean-terrain dispatch map, full ko i18n. Purpose-built for the locale. |
| **SAP FSM** | [I] Global localization incl. Korea (payroll via SuccessFactors/ECP), but 전자결재 culture + 근로기준법 nuance need heavy config; not out-of-box. |
| **Rippling** | [I] US/global labor-law engine; Korea coverage limited — 근로기준법 + 4대보험 + 전자결재 not native. Mismatch for KR B2B. |
| **Asana** | [I] ko UI available; no approval-line (결재선) or labor-law semantics. Generic PM. |
| **Palantir** | [I] Locale-agnostic engine — you can build KR semantics, but nothing localized ships. |
| **Slack / MS Teams** | [I] ko UI + KR data-residency options; no 전자결재/근로기준법 domain logic (Teams Approvals is generic, not 결재선). |
| **n8n** | N/A — locale-agnostic plumbing. |

---

## Per-vendor: "how they'd build OUR field module"

- **SAP FSM** — A selected design comparator. They'd ship a Gantt dispatch board with an AI scheduler matching mechanics to WOs on skill+availability+route, a full service-order lifecycle, offline mobile execution with guided checklists, and **Crowd Service** to broadcast overflow to external forklift techs. Heavy, enterprise, config-first; Korea via SuccessFactors payroll. Our P1-broadcast + geofence would be sub-features of their scheduler. Weakness vs us: 전자결재-culture approval and per-object PBAC need bespoke config.

- **Rippling** — Would treat the field module as a **time-and-location problem**: every mechanic clock-in is geofenced (GPS radius), auto-tagged to a job/customer, and fed straight into payroll with labor-law enforcement. The "work order" would be a job code, not a serviceable object. Excellent for 근태/geo compliance and payroll truth; no dispatch board, no WO lifecycle, no evidence chain. Their version answers "who worked where, paid correctly" — not "is the forklift fixed."

- **Asana** — A dispatch **project** with WOs as tasks, custom-field statuses, intake **forms** creating tasks, and **rules** auto-assigning by field data + nudging on due dates. Clean, fast, no-code, great mobile task UX. But no domain lifecycle, no geofence, no offline-first field capture, no audit chain. Good for a light dispatch board; collapses under maintenance-grade governance/compliance.

- **Palantir Foundry** — Would model a WorkOrder ObjectType in the Ontology, expose dispatch/assign/close as **Action Types** with rule-set guards, and assemble the operator UI in **Workshop** (Object Explorer + Button Groups + map widget). Object/property-level security ≈ our PBAC; automation and humans fire the same Actions; provenance native. The most philosophically aligned (we are explicitly ontology-first). Difference: they ship a **generic engine with no domain types** — you build everything; we ship a rich maintenance catalog + native mobile app + KR localization out of the box.

- **Slack** — Dispatch as a channel + **Workflow Builder**: a form posts a WO to `#dispatch`, a workflow routes it to an on-call mechanic, escalates to a backup if unacknowledged, and mobile approve/deny updates the thread. Superb for the human coordination + approval loop and mobile notifications. Zero system-of-record: no lifecycle, no geofence, no audit object. A collaboration layer *around* a field system, not the system.

- **Microsoft Teams** — A prominent **frontline-comms** take: Shifts for schedule + geofenced (200m) clock-in + swap/handover, Tasks/Planner for assignment, Walkie Talkie for live coordination, Approvals for sign-off, all mobile-first, Power Automate for geofence enforcement. Great shift-handover and clock-in; but assignment is coarse (Planner), no WO lifecycle/SLA/evidence chain, geofence export is true/false only. A frontline hub, not a dispatch engine.

- **n8n** — Not an operator product. They'd build the **connective tissue**: webhook receives a WO event, branches to round-robin a mechanic, calls the maintenance REST API, fires Alimtalk, runs SLA-timer escalation on a cron. Self-hosted, credentials on-prem (fits our bare-metal mandate). It would *drive* automation behind our module, never *be* the module. **[I]**

---

## What we'd steal — ranked (capability → selected cited reference → fit with our ontology-first grammar → cost) **[I]**

1. **AI/optimizer-assisted auto-scheduling on the dispatch board** → **SAP FSM** [I] → fits: an optimizer that reads skills/availability/geo and proposes assignments = a new Automate effect emitting our existing assign Action; the ontology already has mechanic/equipment/site + geo. Big value over today's manual+P1-only dispatch. **Cost: L**.
2. **Turnkey geofenced clock-in with labor-law binding** → **Rippling** [I] → fits: we already have per-site geofence + LocationPing + 근로기준법 attendance; steal the *tight punch↔geofence↔payroll* loop and the "assign hours to job/customer at clock-in" prompt to bind attendance to WOs. **Cost: M**.
3. **Shift handover UX (open-shift/swap/notes + live PTT)** → **MS Teams** [I] → fits: our HO- handover type + REVISIT/TEMPORARY states are the data; steal the mobile swap/handoff request flow and shift-note affordance for the Android app. Walkie-Talkie PTT is a stretch (defer). **Cost: M**.
4. **No-code intake form → auto-created & auto-assigned WO** → **Asana** [I] → fits directly: our add-anything + Automate rules can bind a public/internal form to a WO create Action with rule-based assignment. Cheap, high daily value for dispatchers. **Cost: S**.
5. **Escalation-if-unacknowledged + mobile approve-from-notification** → **Slack** [I] → fits: our P1 dispatch already escalates on timer; generalize the ack-or-escalate-to-backup pattern to all assignments, and enrich the FCM push with inline accept/decline. **Cost: S**.
6. **Crowd/partner-technician broadcast for overflow** → **SAP FSM** (Crowd Service) [I] → fits: our OUTSOURCE priority + P1 broadcast are the seed; extend broadcast to an external/partner mechanic pool with a vendor object. **Cost: M**.
7. **Self-hosted webhook/HTTP automation escape hatch** → **n8n** [I] → fits our bare-metal mandate: expose WO lifecycle events as webhooks so ops can wire external systems without code — complements (doesn't replace) the governed Automate hub. **Cost: S** (event webhooks) / M (bidirectional).

---

## Cross-cutting lens findings (5 independent review lenses)

- **Task-flow:** the **new ontology console has no dedicated field body**. Work order is a seeded projected read type, but no work-order projected action dispatch is registered; legacy dispatch/map/work-order pages and a native Android field app exist outside the new shell. ServiceNow FSM still leads on auto-assignment and one all-in-one technician card. **Steal:** auto-dispatch matching, an explicit domain-owned work-order action dispatch, and a unified mobile work card. Cost **L**. **[I]**
- **IA / layout:** nav `dispatch/maintenance/field` **screens are unbuilt** in the console; work orders fall back to the generic **kanban lanes**. **GAP:** no **schedule board** (time-grid), no **map**, no **drag-drop dispatch** — the three defining FSM IA elements. **Steal:** dispatcher single-pane = unassigned-queue + schedule board + map → ServiceNow FSM (the defining gap) [L]; drag-drop assignment onto technician/timeslot [M]; local-time-aligned schedule grid [M]. (Mobile field-exec is correctly the native Android app, not the console.) **[I]**
- **Data-model:** WO- has a deep domain lifecycle and is seeded as a projected read type. **Weaker:** writes remain domain-owned, projected action/as-of depth is incomplete, and ServiceNow/Salesforce model WO↔Asset↔CI as richer first-class references. **Steal:** deepen WO↔Asset↔CI typed links [M]; ServiceAppointment as a distinct typed object [M]; open WO in the 3-layer ObjectCard without adding a second writer [S]. **[I]**
- **Governance:** **Partial** — selected field-action guardrails and audit seams exist, but universal checklist/audit coverage and trusted offline integrity are not proved. Korean note: 현장 coverage / 대근 (substitution) semantics are represented. **Steal:** offline-approval integrity (signed device-context + sync-time trusted audit) [M]; WO cost-posting gate (block cost booking after TECO-equivalent) → SAP PM [S]. **[I]**
- **Automation / extensibility:** **Steal:** WO lifecycle-transition trigger (CRTD→REL→TECO→CLSD → fires Automate; "every transition is an audit event" fits) [M]; 사전 대근 / cover-planner cron (schedule trigger + substitution Action) [S]; escalation timer (SLA breach → reassign) [M]. **[I]**

**Adjudicated contradiction (layer mismatch):** field.md was the only module doc benchmarking the **legacy** react-router app (`features/dispatch/*` + `pages/*`) while appr/compliance ground on the ontology console. The evidence base was relabeled (legacy, pending Phase-D re-implementation) and an honest gap added: the new ontology console ships NO dispatch board / map / drag-drop — so SAP-FSM-Gantt and ServiceNow-Dispatcher comparisons are against a superseded screen, per the ia-layout §13 + task-flow §13 lenses (both code-confirmed). Also: the Slack "escalate-to-backup handoff" was downgraded [I]→[I] (it's the third-party Wrangle app, not Slack-native), and Slack branching corrected to "15 branches, ≤10 rules each, Business+/Grid only".
