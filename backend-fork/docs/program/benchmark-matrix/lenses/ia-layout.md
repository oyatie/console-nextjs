# INFORMATION ARCHITECTURE & LAYOUT LENS — Oyatie Console vs vendors

> **Benchmark evidence metadata**
> - Observation/revalidation date: 2026-07-19.
> - Sampled products/surfaces: Oyatie console IA across 14 module sections; SAP Fiori; Workday; ServiceNow; Palantir; Salesforce; Slack; Microsoft Teams; Hanbiro, DaouOffice, and NAVER WORKS; AuditBoard; Linear and Notion.
> - Evidence modality: Fixed-target repository source plus live-checked public official documentation/product pages and explicitly labeled public secondary pages; hands-on product tenants, screenshot capture, deployment, activation, and production validation were not performed.
> - Scope/claim ceiling: Only the named pages, surfaces, and fixed-target source are in scope; no whole-product, current-production, provider-parity, universal-superiority, legal, tax, labor, deployment, activation, or production conclusion.
> - Legend: [V] = bounded external observation with a direct URL or same-document source-list entry; [E]/[code] = fixed-target repository observation; [I] = recommendation or inference. Every steal/adopt item is [I].

Independent pass. External vendor claims are **[V]** only when a direct HTTPS URL or same-document primary-source key supports the bounded statement; **[I]** marks inference or recommendation. Fixed-target Oyatie observations are **[E]** or **[code]**.

> **Deliverable note:** the `benchmark/draft/` dir is a protected sibling artifact (draft matrices) I was told not to read/clobber. All 14 module sections are consolidated here in `lens/ia-layout.md`.

## IA primary source catalog

Each key below binds a bounded external claim to a sampled primary page. The description is the claim ceiling for that key; strategy and product-preference conclusions remain [I].

| Key | Publisher | Direct primary source | Bounded support |
|---|---|---|---|
| `auditboard_customer_story` | AuditBoard/Optro | [source](https://go.auditboard.com/rs/961-ZQV-184/images/United-Bankshares-AuditBoard-Customer-Success-Story-WP.pdf) | First-party-published customer story supports a bounded control/test/workpaper/evidence workflow; not universal product behavior. |
| `daouoffice_submission` | DaouOffice | [source](https://manual.daouoffice.co.kr/hc/ko/articles/24397742073753-%EA%B2%B0%EC%9E%AC-%EC%83%81%EC%8B%A0) | Approval submission states on the named manual page. |
| `hanbiro_approval` | Hanbiro | [source](https://www.hanbiro.com/software/groupware-workflow-approval.html) | Workflow approval features on the named page. |
| `linear_command_menu` | Linear | [source](https://linear.app/docs/conceptual-model) | Linear command-menu conceptual model only. |
| `naver_works_approval` | NAVER WORKS | [source](https://naver.worksmobile.com/products/works-support/approval/) | Approval product-page features only. |
| `notion_keyboard_shortcuts` | Notion | [source](https://www.notion.com/help/keyboard-shortcuts) | Notion keyboard shortcuts/command access only. |
| `palantir_carbon_overview` | Palantir | [source](https://www.palantir.com/docs/foundry/carbon/overview) | Carbon overview. |
| `palantir_carbon_workspaces` | Palantir | [source](https://www.palantir.com/docs/foundry/carbon/workspaces-overview) | Carbon workspace organization/menu behavior. |
| `palantir_explore_charts` | Palantir | [source](https://www.palantir.com/docs/foundry/object-explorer/explore-charts) | Charts in Object Explorer; retain only documented chart/filter behavior. |
| `palantir_object_explorer` | Palantir | [source](https://www.palantir.com/docs/foundry/object-explorer/getting-started) | Object Explorer navigation and exploration surface. |
| `palantir_object_views` | Palantir | [source](https://www.palantir.com/docs/foundry/object-views/config-object-views) | Configurable Object Views/tabs. |
| `salesforce_console` | Salesforce | [source](https://help.salesforce.com/s/articleView?id=service.console2_planning_questions.htm&language=en_US&type=5) | Console workspace tabs/subtabs. |
| `salesforce_lightning_workspace` | Salesforce | [source](https://trailhead.salesforce.com/en/content/learn/modules/lightning-experience-for-salesforce-classic-users/work-with-your-data) | Lightning workspace tabs, split view, and utility bar on the sampled surface. |
| `sap_fiori_launchpad` | SAP | [source](https://help.sap.com/doc/289ec1eb1a9b4efab8cb1bf60f6f8e03/202210.002/en-US/bde12a271f0647e799b338574cda0808.pdf) | Fiori launchpad navigation and role-oriented app entry; do not infer universal tile behavior. |
| `sap_fiori_list_report` | SAP | [source](https://experience.sap.com/fiori-design-web/v1-46/list-report-floorplan-sap-fiori-element/) | List Report floorplan only. |
| `sap_fiori_object_page` | SAP | [source](https://experience.sap.com/fiori-design-web/object-page/) | Object Page header and anchored/tabbed sections; access may redirect/restrict but publisher is primary. |
| `sap_fiori_overview` | SAP | [source](https://experience.sap.com/fiori-design-web/v1-48/overview-page/) | Overview Page cards and filter behavior on this documented design-system surface. |
| `servicenow_audit_workspace` | ServiceNow | [source](https://www.servicenow.com/docs/r/store-release-notes/store-grc-rn-audit-mgmt-workspace.html) | Audit Management Workspace named features only. |
| `servicenow_configurable_workspace` | ServiceNow | [source](https://www.servicenow.com/docs/r/platform-user-interface/c_set-up-configurable-workspace.html) | Configurable Workspace shell/record-workspace behavior; not a universal home claim. |
| `servicenow_dispatcher_workspace` | ServiceNow | [source](https://www.servicenow.com/docs/r/field-service-management/field-service-scheduling/using-dispatcher-workspace.html) | Dispatcher Workspace documented panes and scheduling surface. |
| `servicenow_dynamic_scheduling` | ServiceNow | [source](https://www.servicenow.com/docs/r/field-service-management/dispatcher-ws-dy-scheduling.html) | Dynamic scheduling/drag-and-drop behavior where documented. |
| `servicenow_irm` | ServiceNow | [source](https://www.servicenow.com/products/integrated-risk-management.html) | IRM product-page statements; no provider-parity conclusion. |
| `slack_custom_sections` | Slack | [source](https://slack.com/help/articles/360043207674-Organize-your-sidebar-with-custom-sections) | Sidebar custom sections; does not prove redesign-density superiority. |
| `slack_shared_sections` | Slack | [source](https://slack.com/help/articles/29873996048019-Share-sidebar-sections-in-Slack) | Shared sidebar sections. |
| `teams_main_views` | Microsoft | [source](https://support.microsoft.com/en-US/accessibility/teams/use-a-screen-reader-to-explore-and-navigate-microsoft-teams) | Named Teams views/navigation only. |
| `teams_schedule_meeting` | Microsoft | [source](https://support.microsoft.com/en-us/teams/meetings/schedule-a-meeting-in-microsoft-teams) | Meeting scheduling surface; meeting-native product thesis remains [I]. |
| `workday_custom_tasks` | Workday | [source](https://developer.workday.com/documentation/yna1522349470046/CreateCustomTasks) | Custom tasks/worklets on the sampled developer surface; exact two-column/search/dashboard layout unsupported. |
| `workday_related_actions` | Workday | [source](https://developer.workday.com/documentation/mvv1530164381144) | Related Actions on the sampled developer surface; not proof of every HCM/finance screen. |

---


## 0. Our console's IA — ground truth (code evidence)

- **Left sidebar**, 9 groups, Korean labels carbon-copied from `Oyatie Console.dc.html`: 개요 · 인사 · 급여·근태 · ERP · 현장운영 · 거버넌스 · 분석 · 자동화 · 커뮤니케이션 (`shell/nav.ts:84-228`). **Deny-by-omission** gating (item hidden unless role/feature grant intersects); empty groups dropped. Responsive auto-collapse <1280px (`ConsoleShell.tsx:51-68`).
- **Topbar**: group-company **scope selector** (UNION_SCOPE — 법인/branch scoping), **⌘K command palette** — but **results surface is empty/unwired** (`ConsoleShell.tsx:326` "full palette … is a later slice"), theme cycle, user chip.
- **Right comms rail [E]:** Interactive collapsed/open CommsRail exists and is tested. The fixed target covers collapsed/open behavior, section switching, unread/read-all behavior, navigation, and sending. Remaining comms gaps are richer triage and runtime/production proof.
- **Navigation is `state.screen`-driven, NOT react-router** (`ConsoleApp.tsx:24`, `ConsoleShell.tsx:70`). No breadcrumbs; flat 2-level (group → item).
- **Generic master-detail engine** (`module/ModuleScreen.tsx`): header (title + search + policy-gated primary action) → **statbar** (exception-only chips, `0`→em-dash, §4.7-1) → optional **prog bar** → body = **list-table** (resizable cols snapping to 8px ticks, **J/K/Enter** keyboard grammar) OR **kanban lanes** → **single 22rem right `DetailPanel`** (KV grid + object **link chips** that route to object nav + policy-gated action footer).
- **TWO parallel module engines coexist** — a divergence: legacy `module/ModuleScreen.tsx` (workOrder/support configs) AND newer ontology-driven `modules/moduleScreens.ts` (finance/asset; columns derive from `ONT_TYPES.propSchema`, `dataAdapter` pattern, richer link-chip graph). Two grammars for "a module."
- **Ontology-first grammar**: link chips carry object-kind tone and route to object nav; object surfaces exist as `explore/ObjectExplorer`, `ontology/OntologyManager`, `objectcard/ObjectCard`, `lifecycle/LifecycleCard`, `policycanvas`, `workflows` canvas.
- **Nav offers more than exists**: `inbox, recruit, orgchart, evaluation, purchase, inventory, dispatch, forecast, board, directory` lack built screens. `mywork` is source-mounted through `SCREEN_REGISTRY` with authenticated action-inbox and todo reads, but remains DARK and lacks runtime/closed-loop E2E evidence. `scheduled` is excluded: it is Source-present through shared `AutomateBody`, with schedule list/detail, cron, run, edit/save, toggle, and delete behavior; runtime/browser/production proof remains open.

---

## 1. Overview (개요 / mywork / inbox)

| Vendor | Landing IA | Disclosure | Src |
|---|---|---|---|
| SAP Fiori | The sampled launchpad documents navigation and role-oriented app entry; no universal live-count tile behavior is inferred. | role/app entry | [V] `sap_fiori_launchpad` |
| Workday | The sampled developer page documents custom tasks/worklets; the exact Actions/Views split and search-first layout remain an inference. | custom task/worklet | [V] `workday_custom_tasks`; broader layout [I] |
| ServiceNow | The sampled Configurable Workspace page documents workspace shell and record-workspace behavior, not a universal persona home. | workspace→record | [V] `servicenow_configurable_workspace` |
| Palantir Carbon | The sampled Carbon page documents workspace organization and configurable menu behavior. | menu→workspace | [V] `palantir_carbon_workspaces` |

**Ours:** `overview` calls the action-inbox endpoint in source and renders its source-observed response with derived stats/counts, nav badges, and source-route row actions. `mywork` is source-mounted and API-wired to the authenticated action-inbox and personal todo CRUD surfaces, with closed-world object drills and recovery states; it remains DARK and lacks runtime/closed-loop E2E evidence. `inbox` remains incomplete. The gap is inline/ObjectCard completion on the landing row. **Korean context:** 전자결재 culture wants **결재 대기함** as the hero of home (다우오피스 makes 상신/수신함 the landing).
**Steal:** (1) inline/ObjectCard completion on the source-observed overview row → Slack/Teams [**M**]; (2) Actions\|Views landing grammar → Workday [**M**]; (3) 결재 대기함 hero card [**S**]. **[I]**

## 2. Dashboard (분석 / KPI)

| Vendor | IA | Disclosure | Src |
|---|---|---|---|
| Workday | Custom tasks/worklets are documented; the claimed dashboard ordering, sizing, and security layout is not established. | custom task/worklet | [V] `workday_custom_tasks`; layout [I] |
| SAP Fiori | The sampled Overview Page documents cards and filter behavior on that design-system surface. | card→filtered view | [V] `sap_fiori_overview` |
| Palantir | The sampled Object Explorer page documents chart and filter behavior; it does not establish that every metric universally maps to an ontology object. | chart→filtered set | [V] `palantir_explore_charts` |

**Ours (`DashboardScreen.tsx`):** candidate presentation strengths in this sampled comparison — advisory policy-relative scope segments (§4.5), **typed month-period segments** (§4-19), **one-row authored drill-affordance stat strip** (§4-11), **honest-scale charts** (§4-24), **omits unbacked sections** not placeholders (§4-12). The policy projection is not a live Cedar boundary. **DIVERGENCE/bug [E]:** the absolute React Router `Link` targets (`/dispatch`, `/approvals`, `/ops`, and peers) are registered by `AppRouter` as legacy `ConsoleShell`/`AppShell` routes. The resulting carbon-console shell exit and `state.screen`/ObjectCard bypass are an inferred consequence [I]; browser behavior remains unverified.
**Steal:** (1) fix drills to route into `objectExplorer`/screen model, not router paths → Palantir + correctness fix [**S**]; (2) smart-filter bar unifying scope+period → SAP [**M**]; (3) user-configurable dashboard [**L**]. **[I]**

## 3. Finance (ERP / 전표)

| Vendor | IA | Disclosure | Src |
|---|---|---|---|
| SAP Fiori S/4HANA | The sampled Object Page documents header plus anchored/tabbed sections; the sampled List Report documents that floorplan. | list→object sections | [V] `sap_fiori_object_page`, `sap_fiori_list_report` |
| Salesforce | N/A — CRM, no native GL/finance-of-record. | — | [I] |
| Workday Fin | Related-Actions on every amount/account. | related actions | [I] parity w/ HCM |

**Ours (`financeModuleScreen` [E]):** Finance is source-wired for list/detail/create/post/reverse with status-gated row actions and an unblocked primary action. Remaining finance gaps are anchored multi-section object pages, richer reporting, and runtime proof.
**Steal:** (1) SAP anchored in-panel sections (header→JE lines→GL→audit) so drill doesn't lose context [**M**]; (2) smart-filter list report [**M**]; (3) compact/comfort density toggle [**S**]. **[I]**

## 4. People (인사 / 조직도 / 평가)

| Vendor | IA | Disclosure | Src |
|---|---|---|---|
| Workday HCM | Related Actions are documented on the sampled developer surface; the exact anchored profile sections, search ordering, and org drill remain design inference. | related actions | [V] `workday_related_actions`; broader layout [I] |
| SuccessFactors | People-Profile block wall; photo-card org chart. | block→detail | [I] |

**Ours:** 4 nav items, only `identity/` has real components; `recruit/orgchart/evaluation` **unbuilt**; no worker object page (would fall to generic list+KV panel). **Korean:** 직급 vs 직책, 호봉, 발령 history, 법인→본부→팀 org — Workday's flat "position" mismatches 직급 tables; our ontology could model 직급/호봉 as object props natively (potential local fit if built).
**Steal:** (1) Related-Actions menu → our object-action catalog already exists in asset module [**M**]; (2) anchored worker profile (Job/발령/평가/근태) [**M-L**]; (3) org-chart drill reusing topbar scope tree [**M**]. **[I]**

## 5. Leave (급여·근태 / 휴가)

| Vendor | IA | Disclosure | Src |
|---|---|---|---|
| Workday Absence | Time-Off worklet; request → approval routing; balance card. | request→approval | [I] from worklet model |
| Korean HR (시프티/flex) | Calendar-grid team leave board + 잔여 연차 counter + 결재 line. | calendar→request | [I] Korean HR SaaS norm |

**Ours:** `leave/LeaveConsole.tsx` exists and is list-based; the measured screen uses co-located self-service and decision queues rather than a calendar-grid IA. Request and balance lists plus decision/촉진 controls call target endpoints in source; request creation is unwired and closed-loop E2E remains absent, so the full 전자결재 handoff is still partial. **Korean context is decisive:** 근로기준법 accrual rules, 대체공휴일, and 반차/반반차 require deeper modeling than a flat global-vendor PTO request.
**Steal:** (1) team **calendar-grid** leave board (who's out) → Korean HR SaaS [**M**]; (2) 잔여 연차 balance counter tied to 근로기준법 accrual [**M**]; (3) leave-request → 결재선 handoff (reuse `appr`) [**S**]. **[I]**

## 6. Support (현장운영 / 티켓)

| Vendor | IA | Disclosure | Src |
|---|---|---|---|
| ServiceNow | The sampled page documents Configurable Workspace shell and record-workspace behavior; no universal replacement claim is made. | workspace→record | [V] `servicenow_configurable_workspace` |
| Salesforce Console | The sampled sources document console tabs/subtabs and a Lightning workspace with split view and utility bar. | tabs/subtabs; split view | [V] `salesforce_console`, `salesforce_lightning_workspace` |
| Zendesk | Ticket list + composer; macros. | list→ticket | [I] |

**Ours (`supportTicketModuleConfig`):** generic ModuleScreen, list + 22rem panel + resolve action (real mutation). Exception-only chips. **GAP:** no multi-record **tabs/subtabs** — you can hold only ONE open detail; no **split view** persistence; no **utility bar**. An agent juggling 5 tickets can't tab between them.
**Steal:** (1) **workspace tabs + subtabs** for multi-record work → Salesforce (biggest agent-productivity gap) [**L**]; (2) utility bar (notes/recent) as docked footer → Salesforce [**M**]; (3) progressive-disclosure tabbed record → ServiceNow [**M**]. **[I]**

## 7. Evidence (거버넌스 / 증빙)

| Vendor | IA | Disclosure | Src |
|---|---|---|---|
| ServiceNow GRC | The sampled release note documents named Audit Management Workspace features only. | audit workspace | [V] `servicenow_audit_workspace` |
| AuditBoard (Optro) | A first-party-published customer story describes a bounded control/test/workpaper/evidence workflow; it does not establish universal product behavior. | control→workpaper | [V] `auditboard_customer_story` |

**Ours:** `evidence/EvidenceCard`, `EvidenceRecords`, and `audit/` provide the card/list surfaces; `backend/crates/platform/audit-chain` plus migrations `0100`/`0101` provide seal/verify and gap-detection code. **GAP:** no single-pane **audit-timeline workspace** tying request→control→evidence→observation.
**Steal:** (1) single-pane audit-timeline workspace → ServiceNow GRC [**M**]; (2) evidence-request task loop (recurring auto-request) → ServiceNow [**M**]; (3) surface the seal/verify verdict inline as an evidence-card badge (our local design emphasis) [**S**]. **[I]**

## 8. Object-platform (오브젝트 / 온톨로지 — our differentiator)

| Vendor | IA | Disclosure | Src |
|---|---|---|---|
| Palantir Foundry | The sampled primary pages document Object Explorer navigation, chart/filter behavior, configurable Object Views, and Carbon workspace organization. | group→set→object→view | [V] `palantir_object_explorer`, `palantir_explore_charts`, `palantir_object_views`, `palantir_carbon_workspaces` |

**Ours:** `explore/ObjectExplorerScreen`, `ontology/OntologyManagerScreen`, `objectcard/ObjectCard`, `explore/RelationAuthoringPanel`, `policycanvas`. This is our closest-to-Palantir surface and the strategic core. **Assessment vs Palantir:** we have the object card + relation authoring + ontology manager, but likely lack (a) **configurable object groups** in a side-nav, (b) **exploration-view charts as the filter mechanism** (chart-click = filter), (c) **saveable/shareable Layouts**, (d) **multi-object tabs**.
**Steal:** (1) **chart-as-filter exploration view** (each chart = property aggregation, click to filter set) → Palantir — the single highest-fidelity gap for the differentiator [**L**]; (2) **saveable shareable Layouts** [**M**]; (3) configurable object-group side-nav [**M**]; (4) **Object View tabs** (multi-object) [**L**]. **[I]**

## 9. Policy (거버넌스 / PBAC — Cedar)

| Vendor | IA | Disclosure | Src |
|---|---|---|---|
| Palantir | Restricted views / markings on object properties; no visual policy IDE exposed. | inline marking | [I] |
| AWS/Cedar, OPA | Policy-as-code editors: policy list + test/playground + decision trace. | policy→test→trace | [I] Cedar/OPA tooling norm |

**Ours:** `policy/` (PolicyGate/PolicyGated/usePolicyGate) + `policycanvas/` provide advisory UI gating and authoring/simulation surfaces. Coverage is not universal, and live authorization remains legacy server-side plus evidenced RLS until Cedar promotion. **GAP (IA):** no first-class **policy authoring workspace** (policy list → editor → **test/simulation** → **decision trace**). Korean SoD and 법인 scoping remain target requirements under ADR-0021.
**Steal:** (1) policy **test/simulation playground + decision-trace** panel → Cedar/OPA norm [**M**]; (2) policy-list master-detail (policy → affected principals/actions) [**M**]; (3) "why blocked" inline trace from a gated action [**S**]. **[I]**

## 10. Automate (자동화 / 워크플로우)

| Vendor | IA | Disclosure | Src |
|---|---|---|---|
| ServiceNow | Flow Designer: trigger→action canvas; run history. | flow→run log | [I] from platform |
| Palantir | Pipeline/Workshop: DAG canvas over ontology actions. | node→run | [I] |

**Ours:** most listed IA nodes are stubs or shell fallbacks. The Source-present shared `AutomateBody` exposes schedule list/detail, cron, run, edit/save, toggle, and delete behavior; runtime integration and browser/production proof remain open.
**Steal:** finish the genuinely absent list/detail and role-specific IA nodes; enrich Automate triggers/actions without relabeling the Source-present schedule surface as absent. Browser-prove schedule behavior rather than proposing to build an already-present recurring view. **[I]**

## 11. Comms (커뮤니케이션 / 메신저 / 메일)

| Vendor | IA | Disclosure | Src |
|---|---|---|---|
| Slack | Slack Help documents custom and shared sidebar sections. The density/redesign preference remains inference. | sidebar sections | [V] `slack_custom_sections`, `slack_shared_sections`; density thesis [I] |
| Teams | Microsoft support pages document named Teams views/navigation and meeting scheduling. The meeting-native product thesis remains inference. | views; meeting scheduling | [V] `teams_main_views`, `teams_schedule_meeting`; product thesis [I] |

**Ours [E]:** `messenger/MessengerConsoleScreen` and `mail/` are present, and the fixed-target shell has the 54px rail. Interactive collapsed/open CommsRail exists and is tested. Remaining comms gaps are richer triage and runtime/production proof.
**Recommendation [I]:** extend the existing CommsRail with richer triage while preserving its tested collapsed/open, navigation, read-state, and sending behavior; add channel sections for org/법인 grouping only after bounded UX/runtime validation. Do not rebuild the implemented rail. Cost **M** [I].

## 12. Appr (거버넌스 / 전자결재) — Korean-critical

| Vendor | IA | Disclosure | Src |
|---|---|---|---|
| 한비로/다우오피스/네이버웍스 | The sampled primary pages document workflow-approval features, approval-submission states, and approval product features; no unsupported form-count total is retained. | approval→submission states | [V] `hanbiro_approval`, `daouoffice_submission`, `naver_works_approval` |
| Workday | Business-process approval routing (Related Actions). | inbox→approve | [I] |

**Ours:** `appr/ApprovalCompose.tsx` + `composeModel` (real 전자결재 compose). Nav `appr` (checkSq). **The sampled SAP/Workday surfaces do not show the same Korean approval combination** — their cited approval surfaces use a flatter inbox, while Korean 전자결재 needs **결재선 (sequential + 병렬 + 전결/대결)**, 상신/수신/반려 함 separation, and drafter-vs-approver **state dualism**. Our `ApprovalCompose` is the right foundation.
**Steal:** (1) **결재선 builder** (순차/병렬/전결/대결) as the compose core → Korean groupware — this exact combination was not observed in the sampled global-product sources [**M**]; (2) **함 IA**: 상신함/수신함/반려함/완료함 as master-detail tabs [**M**]; (3) drafter/approver **dual-state chips** (예정/대기/완료 vs 반려/협의) [**S**]. **[I]**

## 13. Field (현장운영 / 배차 / 정비)

| Vendor | IA | Disclosure | Src |
|---|---|---|---|
| ServiceNow FSM | The sampled pages document Dispatcher Workspace panes/scheduling and dynamic scheduling/drag-and-drop where stated. | queue+schedule surface | [V] `servicenow_dispatcher_workspace`, `servicenow_dynamic_scheduling` |
| Salesforce Field Service | Gantt scheduler + map + service appointments. | gantt→appointment | [I] |

**Ours:** nav `dispatch/maintenance/field` (fieldOps group) — but **screens unbuilt**; work orders flow through the generic **kanban lanes** (unassigned/active/review) in `workOrderModuleConfig`. **GAP:** no **schedule board** (time-grid), no **map**, no **drag-drop dispatch** — the three defining FSM IA elements. Kanban lanes are a weaker substitute for a dispatcher. **[I]**
**Steal:** (1) **dispatcher single-pane** = unassigned-queue + schedule board + map → ServiceNow FSM (defining gap) [**L**]; (2) **drag-drop assignment** onto technician/timeslot [**M**]; (3) local-time-aligned schedule grid [**M**]. (Mobile field-exec is the native Android app, not console — correct split.) **[I]**

## 14. Compliance (거버넌스 / 무결성)

| Vendor | IA | Disclosure | Src |
|---|---|---|---|
| ServiceNow IRM/GRC | The sampled IRM product page supports only its named risk/control/audit product statements. | IRM product surface | [V] `servicenow_irm` |
| AuditBoard | The first-party-published customer story supports a bounded control/test/workpaper/evidence workflow; a universal finding/remediation chain is not inferred. | bounded customer workflow | [V] `auditboard_customer_story` |

**Ours:** nav `compliance` (fileCheck) gated `INTEGRITY_ROLES + integrity_findings_read` (EXECUTIVE/SUPER_ADMIN; ADMIN **excluded by design** — a deliberate SoD choice). Ties to `evidence/` + `audit/` chain. **Edge:** integrity findings + sealed audit chain = automated control evidence with cryptographic integrity. **GAP:** no **control→test→finding→remediation** master-detail workflow surface.
**Steal:** (1) control-library → test → **finding → remediation** master-detail → AuditBoard/ServiceNow [**M**]; (2) automated control-test scheduling (reuse `automate` + `scheduled`) [**M**]; (3) findings inbox routing to `appr` for remediation approval [**S**]. **[I]**

---

## CROSS-MODULE TOP-10 FINDINGS (ranked) **[I]**

1. **No multi-record workspace (tabs/subtabs/split-view persistence) [E].** The fixed-target console holds one open detail panel. The sampled Salesforce and ServiceNow pages document tabs/subtabs, split-view, utility-bar, and configurable workspace behavior [V] `salesforce_console`, `salesforce_lightning_workspace`, `servicenow_configurable_workspace`. Treating this as the highest-leverage structural gap and Cost **L** is [I].

2. **The 22rem detail panel loses context on every drill [E].** Related objects open as link chips that navigate away. The sampled SAP Object Page and Salesforce console sources document anchored/tabbed sections and subtabs [V] `sap_fiori_object_page`, `salesforce_console`. Adopting in-panel anchored sections and Cost **M** are [I].

3. **Overview rows do not complete actions inline [E].** Source-observed inbox counts, nav badges, and source-route actions exist, but terminal completion still leaves the row. The sampled SAP, ServiceNow, and Korean approval pages document their bounded navigation/workspace/approval surfaces [V] `sap_fiori_launchpad`, `servicenow_configurable_workspace`, `hanbiro_approval`, `daouoffice_submission`, `naver_works_approval`. The inline-completion recommendation and Cost **M** are [I].

4. **Command palette results are empty in the fixed target [E].** The sampled Linear and Notion pages document command access and keyboard shortcuts [V] `linear_command_menu`, `notion_keyboard_shortcuts`. Treating command access as primary navigation or the largest keyboard-productivity gap and Cost **M** are [I].

5. **Two divergent module engines** (`module/ModuleScreen` vs `modules/moduleScreens`). One ontology-driven with dataAdapter + propSchema columns, one hand-config. Every module should be the newer ontology-driven grammar; the fork is tech-debt that will make cross-module IA inconsistent. Consolidate. Cost **M**. **[I]**

6. **Dashboard drills use AppRouter-registered legacy routes rather than the carbon-console state.screen model [E].** They exit the carbon-console shell and bypass its `state.screen`/ObjectCard flow; browser behavior remains unverified. The sampled Palantir page documents chart/filter exploration [V] `palantir_explore_charts`. The proposed repair to route into `objectExplorer`/screen model and Cost **S** are [I].

7. **The fixed target lacks chart-as-filter exploration and saveable layouts on the object platform [E].** The sampled Palantir pages document Object Explorer, chart/filter behavior, and configurable Object Views [V] `palantir_object_explorer`, `palantir_explore_charts`, `palantir_object_views`. Strategic priority and Cost **L** are [I].

8. **The fixed target has no field/dispatch schedule board or map [E].** The sampled ServiceNow pages document Dispatcher Workspace and dynamic scheduling [V] `servicenow_dispatcher_workspace`, `servicenow_dynamic_scheduling`. Treating this as the reference architecture and Cost **L** are [I].

9. **전자결재 (appr) is a Korea-specific comparison lane.** An exact 결재선 (순차/병렬/전결/대결), 상신/수신/반려/완료 함 IA, and drafter-vs-approver dual-state combination was not observed in the sampled SAP/Workday surfaces; the cited Korean-groupware pattern is the design reference. Extending `ApprovalCompose` with that IA is a local-fit recommendation, not a verified superiority claim. Cost **M**. **[I]**

10. **Some nav entries still have no built screen [E].** Gate offers on a built-capability signal or use honest blocked surfaces; do not cite the source-wired finance slice as an unbuilt-action example. The gating recommendation and Cost **S** are [I].

### Cross-cutting synthesis

Our IA has **two promising edges**: (a) an advisory policy/scope projection woven into parts of the shell (scope selector + deny-by-omission affordances), while current live authorization remains legacy server-side plus evidenced RLS and coverage is not universal; (b) an **ontology-first link grammar** on evidenced detail surfaces. The recurring **weakness is progressive disclosure at the record level**: a single non-persistent detail panel that navigates away, no tabs, no anchored sections, no inline completion on overview rows, and an empty palette. The sampled ServiceNow, Salesforce, and SAP Object Page surfaces show **single-pane configurable workspaces with tabs + anchored sections + docked utilities**; we stopped at list+one-panel. The Korean-specific moats (전자결재 결재선, 근로기준법 leave, 법인 scoping) are correctly identified in nav but mostly unbuilt.
