# Benchmark Matrix — Module: `overview` (Operations Overview / Work Hub)

Scope: the landing surface a signed-in operator hits first — stat strip, unified work queue,
agenda, and comms rail. Compared against Palantir Foundry, Slack, Microsoft Teams, Asana, n8n,
Rippling, and SAP (S/4HANA Fiori + SuccessFactors). Most-relevant vendors for this module:
**Slack / Teams** (home & activity triage), **Asana** (My Tasks / Inbox / Home widgets),
**Rippling** (unified home), **Foundry** (Workshop workspace). n8n and SAP are covered but weaker
fits (noted per row).

Rigor: every vendor claim is `[V]` VERIFIED (source URL) or `[I]` INFERRED (reasoned from known
product patterns). Our own column is grounded in the actual code state (grep'd, not aspirational).

---

## 0. Our console — evidence-based baseline (what actually exists today)

Read from `origin/main@86a97771a76b7e770dfcf8c6c7d83fd9d70a98bf` (source-only audit, 2026-07-18):

Fixed-target source observation only; no browser, deployment, activation, or production-runtime validation was performed.

- **`screens/overview/OverviewBody.tsx`** — the mounted-in-source Overview body calls the
  `/api/v1/me/action-inbox`, derives queue stats and filter counts from those same items, and renders
  source-route row actions. The same source-observed action-inbox response supplies a source-derived due-today agenda/timeline with a week
  ribbon. Rows navigate to their source screen; inline or ObjectCard completion is not implemented.
- **`shell/ConsoleShell.tsx` + `shell/navBadges.ts`** — the 3-column shell is
  **sidebar · main · comms-rail**. `overview` is registered and mounted; action-inbox and notification
  summary reads supply real badges for approvals, dispatch, support, personal work, and unread inbox.
- **`shell/CommsRailPanel.tsx`** — the comms rail is default-expanded and interactive. It calls the notification and mail endpoints in source to read
  notifications and mail threads, groups them into messenger/mail/notification/notice sections, shows
  unread counts, and calls the real mark-all-notifications-read mutation. A user may collapse it to the
  glyph strip.
- **Authorization today** — the source-designated server/legacy authorization path remains the current authority in source;
  the UI feature projection only shapes offered navigation and is explicitly non-authoritative. Cedar
  remains the accepted target/shadow path until an action is enrolled, shadow-proven, and promoted under
  ADR-0021 and `docs/specs/cedar-pbac-coexistence-map.json`; every current coexistence-map entry is
  `legacy_only`. Console semantics follow ADR-0023 as amended by ADR-0025.
- Historical program-ledger scores are revision-bound planning evidence, not current runtime proof. The
  current source keeps the deterministic/no-AI UI and Group → 법인 → branch → worksite model. Audit
  seal/verify/gap-detection is partial/DARK: production sealing is OFF, the in-memory signer is not a
  trust root, NULL-org rows are excluded, and an external signer plus out-of-band anchor are required.

**Net:** Overview is a source-wired personal operations hub: action-inbox reads, derived queue stats, nav
badges, source-route row actions, a real due-item agenda, and a default-expanded notification/mail
rail. Inline/ObjectCard completion and richer cross-source triage remain gaps.

---

## 1. Capability matrix

Columns: **Us** = our console. **Fnd** = Foundry. **Slk** = Slack. **Tms** = Teams. **Asa** = Asana.
**n8n**. **Rip** = Rippling. **SAP** = S/4HANA Fiori + SuccessFactors.

### Row 1 — Information architecture (the landing surface)

- **Us:** 3-column shell (sidebar · source-wired work main · comms rail); overview = source-observed action inbox +
  derived stats/counts + source-route row actions + due-item agenda. The default-expanded rail reads
  notifications/mail and supports mark-all-read. Inline/ObjectCard completion remains a gap.
  `[code: ConsoleShell.tsx, OverviewBody.tsx, navBadges.ts]`
- **Fnd:** Workshop module = module-header + pages + sections + widgets; a "homepage" is just another
  Workshop app you build, no fixed home. `[V]` [layouts](https://www.palantir.com/docs/foundry/workshop/concepts-layouts)
- **Slk:** Left rail + Home (channels/DMs) and a dedicated **Activity view** as the triage surface.
  `[V]` [Activity view](https://slack.com/help/articles/19693583638803-Get-your-work-done-from-the-Activity-view)
- **Tms:** App bar with **Activity** as first stop (feed of everything across Teams). `[V]`
  [Activity feed](https://support.microsoft.com/en-us/office/explore-the-activity-feed-in-microsoft-teams-91c635a1-644a-4c60-9c98-233db3e13a56)
- **Asa:** Dedicated **Home** = customizable widget canvas + **My Tasks** + **Inbox** as three distinct
  surfaces. `[V]` [Home](https://asana.com/features/project-management/home)
- **n8n:** **Overview** page = the home base (workflow/project list + global Executions tab); the 5 ops
  metrics live on the separate **Insights** dashboard, not the Overview landing page. `[V]`
  [insights](https://docs.n8n.io/insights/)
- **Rip:** Single unified home with People/Payroll/Benefits/IT/Finance menus + an attention panel.
  `[V]` [platform](https://www.rippling.com/platform)
- **SAP:** **Fiori Launchpad** = spaces/pages of tiles; **My Home** (S/4HANA 2023 FPS01+); SuccessFactors
  **redesigned Home Page** (2H 2025). `[V]` [My Home](https://community.sap.com/t5/technology-blog-posts-by-sap/sap-fiori-for-sap-s-4hana-empowering-your-homepage-enabling-my-home-for-sap/ba-p/13672904)

### Row 2 — Unified work queue / personal task inbox ("my tasks / to-dos")

- **Us:** Overview already renders the source-observed personal action inbox with source-route actions, derived
  counts, filtering, and a mirrored personal-work nav badge. The gap vs vendors is inline terminal
  completion, not an absent queue. `[code: OverviewBody.tsx, navBadges.ts]`
- **Fnd:** No native "my tasks" — you build an object-set widget filtered to `assignee = me` over an
  ontology task type; Automate can route. `[I]` (pattern from Workshop object-set widgets + Automate)
- **Slk:** Not a task manager, but **Later / saved items** + reminders act as a personal follow-up
  queue; threads/mentions filters. `[V]` [reminders](https://slack.com/help/articles/208423427-Set-a-reminder)
- **Tms:** No first-class task queue in Activity; tasks live in the Planner/Tasks app, not the hub.
  `[I]` (Activity is notifications, not assignments)
- **Asa:** **My Tasks** is the flagship — Today/Upcoming/Later sections, list/board/calendar views,
  auto-promotion by due date. Source-cited. `[V]` [My Tasks](https://asana.com/features/project-management/my-tasks)
- **n8n:** N/A as a human task queue — its "queue" is workflow **executions** (Failed/Running/Success/
  Waiting), a machine work log, not assignments. `[V]` [executions](https://docs.n8n.io/workflows/executions/all-executions/)
- **Rip:** **Task inbox** surfaces pending approvals, onboarding tasks, and compliance deadlines to act
  on. `[V]` [platform](https://www.rippling.com/platform)
- **SAP:** **My Inbox 2.0** = unified work-item queue across all workflow providers (All-Items tile or
  scenario-filtered tiles); **Task Center** consolidates cross-system. `[V]` [My Inbox](https://community.sap.com/t5/technology-blog-posts-by-sap/sap-fiori-for-sap-s-4hana-fiori-my-inbox-part-1-activation/ba-p/13326175)

### Row 3 — Approvals surfacing (전자결재 / 결재함)

- **Us:** Approval items returned by the source-observed action-inbox call appear in the Overview queue and contribute
  the real approvals nav badge; the row action source-routes to the approvals screen. Inline approve /
  reject is not implemented. `[code: OverviewBody.tsx, overviewModel.ts, navBadges.ts]`
- **Fnd:** Approvals modeled as ontology Actions + Automate; a Workshop widget can list pending. `[I]`
- **Slk:** Approvals via **Workflow Builder** / approval apps posting to a channel or DM, not a native
  hub queue. `[I]`
- **Tms:** Native **Approvals** app + adaptive-card approvals inline in Activity/chat. `[I]`
- **Asa:** Task **Approvals** (approve/request-changes/reject status) surface in My Tasks & Inbox. `[V]`
  [My Tasks](https://asana.com/features/project-management/my-tasks)
- **n8n:** N/A — no human approval inbox (can send an approval webhook step, but no console). `[I]`
- **Rip:** Pending **approvals** are a headline item on the home attention panel & task inbox. `[V]`
  [platform](https://www.rippling.com/platform)
- **SAP:** **My Inbox** IS the approval engine — multi-step workflow approvals, delegation, mass
  actions; a cited reference for 전자결재-style flows. `[V]` [My Inbox](https://community.sap.com/t5/enterprise-resource-planning-blog-posts-by-sap/sap-fiori-for-sap-s-4hana-replacing-sap-fiori-apps-during-system-conversion/ba-p/14260897)

### Row 4 — Activity / notification feed & triage

- **Us:** The default-expanded comms rail calls notification and mail endpoints in source to read notifications and mail threads, groups messenger /
  mail / notification / notice rows, shows unread counts, and provides a real mark-all-notifications-read
  action. Cross-source filters, snooze, and per-row terminal actions remain gaps.
  `[code: ConsoleShell.tsx, CommsRailPanel.tsx, overviewApi.ts]`
- **Fnd:** Notifications from Action side-effects + Automate effects; no consumer-grade unified feed —
  you compose one. `[I]` / `[V]` effects [Automate](https://www.palantir.com/docs/foundry/automate/effect-actions)
- **Slk:** **Activity view** with tabs Unreads / DMs / Mentions / Threads / Reactions (all but "All
  notifications" and "DMs" now hidden by default behind the Filters control) + custom filters; clear-all
  triage. Source-cited triage UX. `[V]` [Activity](https://slack.com/help/articles/19693583638803)
- **Tms:** **Activity feed** = all @mentions/replies/likes/meeting events, 30-day retention, filter
  pills for @mention & unread, keyword filter (via the Filter control). `[V]` [feed](https://support.microsoft.com/en-us/office/explore-the-activity-feed-in-microsoft-teams-91c635a1-644a-4c60-9c98-233db3e13a56)
- **Asa:** **Inbox** = per-task activity/notifications, archive/snooze, filterable. `[V]`
  [Home](https://asana.com/features/project-management/home)
- **n8n:** Execution log is the "feed" — failures/runs, filterable by status; machine events only. `[V]`
  [executions](https://docs.n8n.io/build/understand-workflows/understand-executions/view-all-executions)
- **Rip:** Attention panel aggregates changes needing action; not a chat-style feed. `[I]`
- **SAP:** **Needs Attention** section (SuccessFactors 2H 2025) groups all attention cards with
  type filters; notification bell in Launchpad. `[V]` [2H 2025 home](https://community.sap.com/t5/human-capital-management-blog-posts-by-sap/sap-successfactors-hcm-updated-home-page-2h-2025/ba-p/14269975)

### Row 5 — Stat strip / at-a-glance KPI tiles

- **Us:** Overview's compact stat buttons are derived from the source-observed action-inbox response, carry urgency tones,
  and filter the queue below. They are work-queue counts, not executive analytical KPI tiles.
  `[code: OverviewBody.tsx, overviewModel.ts]`
- **Fnd:** Metric-card / KPI widgets in Workshop bound to object-set aggregations; fully composable. `[V]`
  [widgets](https://www.palantir.com/docs/foundry/workshop/concepts-widgets)
- **Slk:** N/A — no KPI tiles; not an analytics surface. `[I]`
- **Tms:** N/A natively (Power BI tab can embed, but not the hub). `[I]`
- **Asa:** **Dashboard widgets** (charts, number widgets, completion stats) on Home & project
  dashboards. `[V]` [dashboard widgets](https://help.asana.com/s/article/text-widgets-in-dashboards?language=en_US)
- **n8n:** 5 fixed metrics (Prod. executions, Failed, Failure rate, Time saved, Avg runtime) — on the
  **Insights** dashboard, not the Overview landing page. `[V]` [insights](https://docs.n8n.io/insights/)
- **Rip:** Attention counts (approvals, deadlines, onboarding) but not analytical KPI tiles on home. `[I]`
- **SAP:** **KPI / dynamic-count tiles** on the Launchpad (live count on the tile face) — the original
  drill-from-tile pattern. `[V]` [Launchpad](https://help.sap.com/docs/SAP_S4HANA_ON-PREMISE/22bbe89ef68b4d0e98d05f0d56a7f6c8/753af2c410584bc98f0363ca69a404f1.html)

### Row 6 — Agenda / today / calendar view

- **Us:** A source-derived due-today agenda/timeline is rendered from the source-observed action-inbox items, with a week
  ribbon, due time, completion marker, source-route action, site, and responsible person. Broader
  Today/Upcoming/Later and calendar views remain gaps. `[code: OverviewBody.tsx, overviewModel.ts]`
- **Fnd:** Build a calendar/Gantt-style widget over a date property; no native agenda. `[I]`
- **Slk:** Reminders + calendar-app unfurls; no agenda pane per se. `[I]`
- **Tms:** Calendar app + meeting notifications in Activity (2024+); not on a home pane. `[V]`
  [feed calendar](https://office365itpros.com/2024/05/29/teams-activity-feed-changes/)
- **Asa:** **My Tasks calendar/week view** + Home "upcoming tasks" widget = de-facto agenda. `[V]`
  [My Tasks](https://asana.com/features/project-management/my-tasks)
- **n8n:** N/A (schedule triggers exist, but no human agenda). `[I]`
- **Rip:** Upcoming compliance deadlines / onboarding timelines surfaced; not a calendar. `[I]`
- **SAP:** SuccessFactors home cards can show upcoming (e.g. time-off, reviews); Fiori has a Calendar
  app but not a home agenda. `[I]`

### Row 7 — Comms rail / embedded messaging

- **Us:** Dedicated, default-expanded rail is source-wired for notification and mail reads, grouped messenger /
  mail / notification / notice display, unread counts, and mark-all-notifications-read. It remains a
  triage/read surface rather than inline reply or terminal action UI.
  `[code: ConsoleShell.tsx, CommsRailPanel.tsx, overviewApi.ts]`
- **Fnd:** N/A — no built-in chat; comms happen in the object/notification layer, not a rail. `[I]`
- **Slk:** IS the comms product — DMs/threads/huddles are the whole app. `[V]`
  [threads](https://slack.com/help/articles/115000769927-Use-threads-to-organize-discussions)
- **Tms:** IS the comms product — chat/channels/calls with the app in one shell. `[V]`
  [activity/chat](https://support.microsoft.com/en-us/office/explore-the-activity-feed-in-microsoft-teams-91c635a1-644a-4c60-9c98-233db3e13a56)
- **Asa:** Task comments + project messages; no persistent side chat rail. `[I]`
- **n8n:** N/A (Chat beta = AI assistant, not team comms). `[V]` [insights](https://docs.n8n.io/insights/)
- **Rip:** No native team chat rail. `[I]`
- **SAP:** No native persistent chat rail (SAP Jam retired; relies on Teams integration). `[I]`

### Row 8 — Drill-down navigation (stat → source records)

- **Us:** each source-observed queue row and due-item title calls `openItem`; absent an override, `kindRoute`
  source-routes approval, dispatch, support, and work items to their source screen. There is no inline
  completion or ObjectCard hop today. `[code: OverviewBody.tsx, overviewModel.ts]`
- **Fnd:** Widget events → navigate/filter; object-set drill into Object Explorer search-around. `[V]`
  [events](https://www.palantir.com/docs/foundry/workshop/concepts-events)
- **Slk:** Activity item → jump to message-in-channel. `[V]` [Activity](https://slack.com/help/articles/19693583638803-Get-your-work-done-from-the-Activity-view)
- **Tms:** Activity item → open the source message/meeting. `[V]` [feed](https://support.microsoft.com/en-us/office/explore-the-activity-feed-in-microsoft-teams-91c635a1-644a-4c60-9c98-233db3e13a56)
- **Asa:** Home widget / My-Task row → open the task; number widgets link to filtered lists. `[V]`
  [Home](https://asana.com/features/project-management/home)
- **n8n:** Metric → filtered Executions list → single run detail. `[V]` [executions](https://docs.n8n.io/workflows/executions/all-executions/)
- **Rip:** Task inbox item → the action screen. `[V]` [platform](https://www.rippling.com/platform)
- **SAP:** **Dynamic tile → filtered list → object page** — the canonical drill chain. `[V]`
  [Launchpad](https://help.sap.com/docs/SAP_S4HANA_ON-PREMISE/22bbe89ef68b4d0e98d05f0d56a7f6c8/753af2c410584bc98f0363ca69a404f1.html)

### Row 9 — Personalization / configurable widgets

- **Us:** No end-user home personalization yet; layout is fixed code. Config-as-governed-object is the
  charter (dashboard widget slots) but not on the overview surface. `[code: — fixed layout]`
- **Fnd:** Fully config-as-data (widgets + typed variables + events); but that's *builder-time*, not
  end-user drag. `[V]` [variables](https://www.palantir.com/docs/foundry/workshop/concepts-variables)
- **Slk:** Sidebar sections + Activity filters are the personalization; layout is fixed. `[V]`
  [Activity](https://slack.com/help/articles/46751260742035-Introducing-the-new-Activity-view-in-Slack)
- **Tms:** Pin/reorder apps in app bar; filter Activity; layout fixed. `[V]` [manage notifications](https://support.microsoft.com/en-us/office/manage-notifications-in-microsoft-teams-1cc31834-5fe5-412b-8edb-43fecc78413d)
- **Asa:** **Selected end-user personalization reference** — drag/drop/resize Home widgets, background styles, add/remove widget
  types, private notepad. `[V]` [customize Home](https://help.asana.com/s/article/how-to-customize-your-home-page)
- **n8n:** Home metrics are fixed; no widget personalization. `[I]`
- **Rip:** Role-shaped home; limited end-user layout control. `[I]`
- **SAP:** **Admin + end-user home configuration** — spaces/pages, role-based launchpad params
  (/UI2/FLP_PROF_CONF new in 2025), SF card config & user personalization. `[V]`
  [what's new 2025](https://avotechs.com/blog/sap-fiori-for-s4hana-2025-release/)

### Row 10 — Permissions / scoping (who sees which stats & items)

- **Us:** UI navigation uses deny-by-omission from a non-authoritative feature projection; the current
  source-designated server/legacy authorization path re-authorizes calls and remains the current authority in source. Cedar is target /
  shadow only until per-action enrollment, evidence, and explicit promotion; current coexistence entries
  are `legacy_only`. `[code: nav.ts, policy/authz.ts; ADR-0021 +
  docs/specs/cedar-pbac-coexistence-map.json; console: ADR-0023 amended by ADR-0025]`
- **Fnd:** Object/property policies (row + cell-level), mandatory markings; a stat computed over an
  object-set inherits them. Substantial data-level model. `[V]`
  [object policies](https://www.palantir.com/docs/foundry/object-permissioning/object-and-property-policies)
- **Slk:** Channel membership + workspace/org roles; DLP/enterprise controls. Coarse vs data-cell. `[I]`
- **Tms:** Team/channel roles, sensitivity labels; feed shows only what you can access. `[I]`
- **Asa:** Project/team membership + admin roles; My Tasks is inherently self-scoped. `[I]`
- **n8n:** Project/RBAC (enterprise) scopes visible workflows/executions. `[V]`
  [view-all-executions](https://docs.n8n.io/build/understand-workflows/understand-executions/view-all-executions)
- **Rip:** Granular role/permission graph across HR/IT/Fin; home reflects entitlements. `[I]`
- **SAP:** Business-role → launchpad space/page/tile assignment; workflow-provider authorization on My
  Inbox items. Mature but role-catalog-heavy. `[V]` [My Inbox activation](https://community.sap.com/t5/technology-blog-posts-by-sap/sap-fiori-for-sap-s-4hana-fiori-my-inbox-part-1-activation/ba-p/13326175)

### Row 11 — Automation hooks (surfacing automated conditions on the hub)

- **Us:** Inbox urgency and due tones surface source-observed conditions already returned by the action-inbox API;
  a general Automate→Overview monitor/feed contract remains target work. `[code: overviewModel.ts]`
- **Fnd:** **Automate**: Condition(s)→Effect(s) continuous/scheduled monitors feed notifications/cards.
  Substantial. `[V]` [Automate](https://www.palantir.com/docs/foundry/automate/overview)
- **Slk:** Workflow Builder can post scheduled/triggered items into channels/Activity. `[I]`
- **Tms:** Power Automate cards land in Activity/chat. `[I]`
- **Asa:** **Rules** trigger task moves/assignments that surface in My Tasks/Inbox. `[V]`
  [rules & widgets](https://help.asana.com/s/article/rules-integrations-and-widgets?language=en_US)
- **n8n:** IS the automation engine — Insights metrics ARE automation output. `[V]`
  [insights](https://docs.n8n.io/insights/)
- **Rip:** **If-then Workflow Studio** drives tasks onto the home attention panel. `[V]`
  [workflows](https://www.rippling.com/platform/workflows)
- **SAP:** Workflow engine routes items to My Inbox; SF business rules drive home cards. `[V]`
  [My Inbox](https://community.sap.com/t5/enterprise-resource-planning-blog-posts-by-sap/sap-fiori-for-sap-s-4hana-replacing-sap-fiori-apps-during-system-conversion/ba-p/14260897)

### Row 12 — Mobile

- **Us:** Native field app exists (Android `com.console.app`); console overview is web-responsive;
  no dedicated overview mobile widget appears in the current console tree. `[code: android/app/build.gradle.kts;
  web/src/console/screens/overview]`
- **Fnd:** Workshop **mobile** modules + app launcher + mobile nav-bar widget. `[V]`
  [mobile](https://www.palantir.com/docs/foundry/workshop/mobile-overview)
- **Slk:** Full native apps; Activity/reminders parity. `[V]` [Activity](https://slack.com/help/articles/19693583638803-Get-your-work-done-from-the-Activity-view)
- **Tms:** Full native apps; Activity feed parity. `[I]`
- **Asa:** Native apps + **home-screen widgets** (top-5 today on iOS, task lists on Android). `[V]`
  [widgets](https://asana.com/inside-asana/widgets)
- **n8n:** Web-only, not mobile-optimized. `[I]`
- **Rip:** Native employee app with tasks/self-service. `[I]`
- **SAP:** Fiori is responsive; SAP Mobile Start app aggregates tiles/notifications. `[I]`

### Row 13 — Audit / compliance

- **Us:** Append-oriented audit plus seal/verify and gap-detection code exists, but the seam is
  **partial/DARK**: production sealing defaults OFF, the in-memory signer is not a trust root,
  NULL-org rows are excluded, and current evidence does not prove coverage under every Overview mutation.
  `[code: backend/crates/platform/audit-chain; migrations 0100/0101]`
- **Fnd:** Ontology changelog + Action writeback lineage; full history. `[V]`
  [proposals/changelog](https://www.palantir.com/docs/foundry/ontologies/ontologies-proposals)
- **Slk:** Enterprise audit logs API + eDiscovery/DLP (Enterprise Grid). `[I]`
- **Tms:** Purview audit/compliance, retention labels. `[I]`
- **Asa:** Admin audit-log API (Enterprise+). `[I]`
- **n8n:** Execution history is the audit trail; enterprise log streaming. `[V]`
  [executions](https://docs.n8n.io/workflows/executions/all-executions/)
- **Rip:** HR/payroll compliance + audit trails on changes. `[I]`
- **SAP:** Workflow item history, change docs, GRC; deep regulated-industry audit. `[I]`

### Row 14 — Extensibility (custom cards/widgets on the hub)

- **Us:** StatusChip and the Overview model helpers are shared primitives; adding a new Overview section
  is code today (config-as-data charter pending). `[code: components/, overviewModel.ts]`
- **Fnd:** **Custom (iframe) widgets** read/write Workshop variables via a documented bridge. `[V]`
  [embed apps](https://www.palantir.com/docs/foundry/workshop/widgets-embed-foundry-apps)
- **Slk:** App home tab + Block Kit; apps post to Activity. `[I]`
- **Tms:** Custom apps/tabs, adaptive cards in Activity. `[I]`
- **Asa:** App widgets on dashboards via the developer Widgets API. `[V]`
  [widgets API](https://developers.asana.com/reference/widgets)
- **n8n:** Community nodes extend workflows, not the home UI. `[I]`
- **Rip:** App Shop / custom apps; limited home-surface extensibility. `[I]`
- **SAP:** Custom tiles/cards on Launchpad & SF home (KBA 2641544). `[V]`
  [custom SF tile](https://userapps.support.sap.com/sap/support/knowledge/en/2641544)

### Row 15 — Korean B2B fit (전자결재 culture, 근로기준법, group-company scoping)

- **Us:** **Purpose-built** — Korean-first copy, 진행/확정 period grammar, group→법인→branch→worksite
  scope, 결재/approvals surfacing, and 근로기준법-aware modules (leave/att/pay). Native fit. `[code]`
- **Fnd:** Locale-agnostic; you build 전자결재 as Actions/Automate — flexible but from scratch. `[I]`
- **Slk/Tms:** Localized UI, but 전자결재 is a bolt-on app; no native multi-법인 org scoping in the hub. `[I]`
- **Asa:** Localized; no 전자결재 semantics; flat workspace model mismatches group-company hierarchy. `[I]`
- **n8n:** N/A for HR/approval culture. `[I]`
- **Rip:** US-payroll-centric; Korean 근로기준법/4대보험 payroll not a native strength. `[I]`
- **SAP:** Closest global fit — multi-company-code, delegation, localized payroll; but heavy, and My
  Inbox ≠ Korean 결재선/전결 규정 out of the box. `[I]`

---

## 2. How each vendor would build OUR overview module

**Palantir Foundry.** A Workshop module with a header + one page; sections holding metric-card widgets
bound to object-set aggregations over a `WorkOrder`/`Approval` ontology type, each with an event that
navigates to a filtered Object Explorer view. The "work queue" = an object-table widget filtered to
`assignee = currentUser`; the "alerts" = Automate monitors (Condition→Effect) writing notification
cards. No fixed home — the home *is* config-as-data, versioned like the ontology. Our source-observed inbox and
source-route patterns already echo this; Foundry would push us to make the whole surface a
declarative document, not TSX. `[V]` widgets/events/automate cited above.

**Slack.** Overview = an **Activity view**: Unreads / Mentions / Reactions / Approvals(app) tabs with
filter pills and clear-all triage, plus a Home with pinned channels per worksite. Work items arrive as
Workflow-Builder messages with interactive approve buttons; "later" = saved items. Slack would nail the
*triage ergonomics* (fast filter, keyboard, snooze) but treat KPIs and structured queues as second-class
— they'd live in embedded apps, not native tiles. `[V]` Activity/reminders cited.

**Microsoft Teams.** A pinned **Activity feed** as the hub: filter pills for @mention/unread, calendar
+ approval adaptive-cards inline, 30-day retention. The work queue is delegated to the Tasks/Planner
app; approvals to the Approvals app. Teams would deliver a strong notification-triage home but a
fragmented task story (many apps, one feed). `[V]` feed/filters cited.

**Asana.** A selected personal-work-hub reference: a customizable **Home** (drag/resize widgets: My
Priorities, upcoming, completion stats, notepad) + **My Tasks** (Today/Upcoming/Later, list/board/
calendar) + **Inbox** (per-item activity, snooze/archive). Rules auto-route tasks; number widgets drill
to filtered lists. Asana provides the cited *end-user personalization and agenda* patterns, but no KPI-grade
authorized rollups or 전자결재/audit-chain rigor. `[V]` Home/My Tasks/widgets cited.

**n8n.** An **Overview** page of fixed operational metrics (executions, failure rate, time saved) + a
global execution log filterable by status, drilling to a single run. It would model our "work" as
*automation runs*, not human assignments — excellent for the SLA/monitor half of our ops alerts, a
non-fit for the human work queue / approvals / comms half. `[V]` overview/executions cited.

**Rippling.** A unified **home + task inbox**: an attention panel of pending approvals, compliance
deadlines, and onboarding tasks, driven by an if-then Workflow Studio, with entitlement-shaped
visibility across HR/IT/Finance. Closest to our "one operational hub across domains" thesis; Rippling
would push cross-domain task unification hard, but its home is HR-ops-shaped and US-payroll-centric.
`[V]` platform/workflows/inbox cited.

**SAP (S/4HANA + SuccessFactors).** A **Fiori Launchpad** of role-assigned spaces/pages with dynamic
KPI-count tiles drilling tile→list→object, plus **My Inbox 2.0 / Task Center** as the unified,
delegation-capable approval queue, and a SuccessFactors **"Needs Attention" / "For You Today"** home
with configurable cards. SAP would deliver a substantial approval-workflow + role-catalog machinery and
the original drill-from-tile pattern — at the cost of weight and a role-catalog burden, and My Inbox
still isn't Korean 결재선/전결 규정 natively. `[V]` My Inbox/Launchpad/2H-2025 home cited.

---

## 3. What we'd steal (ranked, actionable)

Fit rated against our accepted target **ontology-first, Cedar-PBAC, deterministic, audited, Korean-B2B** grammar. Cost
S/M/L.

1. **Inline terminal actions on the source-observed personal queue → cited reference: Teams; enterprise proof: SAP My Inbox.**
   Extend the existing cross-module action inbox so eligible rows can approve/reject/acknowledge without
   leaving Overview, while retaining source-route navigation for deeper work. Fit: excellent — the source-observed
   queue and source routes exist; inline policy-gated completion is the missing layer. **Cost: M.**

2. **Activity/triage filters on the source-wired rail → cited reference: Slack; enterprise: SAP "Needs Attention".**
   Extend the existing notification/mail rail beyond reads and mark-all-read with Unreads / Mentions /
   Approvals / Reminders filters, snooze, and source deep-links. Fit: strong — the source-wired rail and unread
   mutation exist. **Cost: M.**

3. **Today/Upcoming/Later agenda depth → cited reference: Asana.**
   Extend the source-observed due-today timeline into grouped upcoming work and calendar/week views without
   fabricating events. Fit: native — due items and the week ribbon already exist. **Cost: S–M.**

4. **End-user home personalization (drag/resize widgets) → cited reference: Asana; governed: SAP.**
   Let operators arrange overview widgets, backed by our config-as-governed-object charter (draft→
   approve→effective, per-persona defaults). Fit: good, but must stay inside the governed-config model,
   not free-form localStorage. **Cost: L.**

5. **Approval delegation + escalation semantics → cited reference: SAP My Inbox.**
   Add delegation (위임) and escalation to the approvals surfaced on the hub — the piece 전자결재 culture
   actually needs (전결/대결/부재중 위임). Fit: strong and locally required; global vendors miss the
   Korean 결재선 semantics, so we build it, informed by SAP's model. **Cost: M.**

6. **Automate-style Condition→Effect monitors feeding the hub → cited reference: Foundry; ops-proof: n8n.**
   Generalize the source-observed inbox urgency/due signals into declarative monitors that any ontology type can raise onto the
   overview + comms feed. Fit: native to the target ontology/Cedar substrate; deterministic (no AI) keeps it in
   bounds. **Cost: M.**

7. **Home-screen mobile widget (top-N today) → cited reference: Asana.**
   A small overview widget for the native field app (top open WOs / pending approvals). Fit: good, low
   priority vs the web hub gaps. **Cost: S.**

**Deliberately NOT stealing:** Teams' one-feed-many-apps fragmentation (we want one authorized hub, not
N bolt-on apps); n8n's runs-as-work model for humans (our queue is assignments, not executions); any
AI-driven "smart" surfacing (deterministic house rule). Asana's flat workspace and Rippling's
US-payroll shape both mismatch our group→법인→branch→worksite + 근로기준법 reality — steal the UX, not
the org model.

---

## 4. Cross-cutting lens findings (5 independent review lenses)

Five reviewers each swept all 14 modules through one lens (evidence read from `web/src/console/**` + the program ledger). Their `overview`-specific findings:

- **Task-flow (money-task step count):** money task = *triage my inbox → act*. Ours today requires one source-route navigation and then the action on the source screen; the Overview row has no inline completion and does not open an ObjectCard. Slack/Teams make the notification itself terminal — **0 navigation, 1 click** **[I]** (approve/reject render inline). **Steal:** policy-gated actions on eligible inbox rows collapse the flow to one click. Cost **M**. This is the highest cross-module ROI item (touches overview/comms/appr/leave/finance).
- **IA / layout:** the landing has a source-observed action-inbox surface, derived stats/counts, nav badges, and source-route actions. Korean 전자결재 culture still favors **결재 대기함 as the hero of home** (다우오피스 makes 상신/수신함 the landing). **Steal:** inline/ObjectCard completion on each source-observed row [M]; Actions\|Views landing grammar → Workday [M]; richer 결재 대기함 hero treatment [S].
- **Data-model / object-semantics:** the landing *projects* other objects; its config (`console_view`) is **engine-registered TODAY** as a governed ontology instance (draft→approve→effective + rollback + as-of) — **stronger than Foundry Home** **[I]**, which is configured but not itself a first-class versioned business object with four-eyes. **Weaker:** Foundry ships live object-set widgets out of the box; ours needs the widget→ontQuery binding finished.
- **Governance:** **Behind on governance-posture summary; no current Cedar-enforcement lead.** The UI uses a non-authoritative deny-by-omission projection while the source-designated server/legacy authorization path remains the current authority in source; Cedar denial posture is target/shadow telemetry until an action is enrolled, shadow-proven, and promoted. **Steal:** a governance-posture strip (pending four-eyes · active legal holds · shadow Cedar denials · overdue lifecycle reviews) → Vanta/Drata — an ontQuery widget over `gov_approvals`/holds/`cedar_decision_log`. Cost **S**, high-visibility.
- **Automation / extensibility:** Overview currently consumes inbox conditions and source-routes rows; it does not execute inline actions. **Steal:** alert-rule trigger (condition crosses threshold → run workflow) → Grafana; source-row-to-inline governed Action → Foundry/Retool; scheduled digest effect.

**Adjudicated current state:** Overview is mounted in source and source-wired: action-inbox reads feed derived queue stats,
nav badges, source-route rows, and the due-item agenda; the default-expanded rail reads notifications /
mail and supports mark-all-read. Inline/ObjectCard completion and richer triage remain target gaps.
