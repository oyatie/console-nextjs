# Benchmark Matrix — Module: **dashboard** (analytics: KPIs, quant projection, insights, profitability)

> **Benchmark evidence metadata**
> - Observation/revalidation date: 2026-07-19.
> - Sampled products/surfaces: Oyatie dashboard/projection/editor source; Palantir Quiver, Workshop, and Object Explorer; SAP Analytics Cloud and Fiori; Asana reporting dashboards; Rippling analytics; Slack analytics; Microsoft Teams/Power BI; n8n execution views.
> - Evidence modality: Fixed-target repository source plus live-checked public official documentation/product pages and explicitly labeled public secondary pages; hands-on product tenants, screenshot capture, deployment, activation, and production validation were not performed.
> - Scope/claim ceiling: Only the named pages, surfaces, and fixed-target source are in scope; no whole-product, current-production, provider-parity, universal-superiority, legal, tax, labor, deployment, activation, or production conclusion.
> - Legend: [V] = bounded external observation with a direct URL or same-document source-list entry; [E]/[code] = fixed-target repository observation; [I] = recommendation or inference. Every steal/adopt item is [I].

Compares **our console** against Palantir Foundry, Slack, Microsoft Teams, Asana, n8n, Rippling, SAP.
Rigor: every vendor claim is **[V]** verified (source URL) or **[I]** inferred (reasoned from known product patterns, labeled honestly). Our column is code-evidenced from `web/src/console`.

---

## 0. Our console — fixed-target source-evidenced state

Read from source, not spec:

- **`console/dashboard/DashboardScreen.tsx`** — the executive dashboard. Server-authorized **scope segments** (only the KPI rollups the real-API contract returns for the caller under the current server/legacy authorization matrix, §4.5), typed **month-period segments** (§4-19, current=진행 / closed=확정, 6 months), a one-row **stat strip** where every available stat is authored as a drill `<Link>` to its source screen (`/dispatch?status=COMPLETED`, `/inspection`, `/approvals`, `/support`…). These absolute React Router targets are registered by `AppRouter` as legacy `ConsoleShell`/`AppShell` routes; they exit the carbon-console shell and bypass its `state.screen`/ObjectCard flow, while browser behavior remains unverified (see Row 6). **Honest-scale charts** use `HonestBar`. Metrics: completed count, avg response, completion duration + due compliance, revisit rate, delay-rate + reason distribution, inspection-plan completion, P1 acceptance. Ops overlay: SLA breached/at-risk, pending approvals, open support. **Unavailable metrics render a warn chip with a reason — sections with no backing API are omitted, never placeholdered** (§4-12).
- **`console/charts/projection.ts` + `ProjectionPanel.tsx`** — deterministic **정량 투영**: EWMA point estimate + EWMA σ, **CI95 band** and **CVaR95** fat-tail downside under a pinned Student-t(ν=4). Pure client math, no AI, no randomness. Every number drills. `wire-pending: Phase C` backend Monte-Carlo/EVT behind the same `Projection` shape. **[I]**
- **`console/charts/honestScale.ts`** — axis truncation is **governed**: baseline stays at 0 unless relative variance < 1/3, and any truncation forces the mandatory warn chip `축 절단 — 기준 ₩x (0 아님)` (§4-24). Anti-deceptive-chart as a design law.
- **`console/configconsole/DashboardEditor.tsx` + `widgets.tsx`** — **no-code dashboard builder**: 4-slot grid over a widget palette (liveCount / statBar / chart), the whole layout is **ONE serializable config doc**. Save = personal view (audited); **팀 배포 = 결재** (shared-layout deploy gated by AP- approval). Widgets recompute from `(config, rows)` off the ontology instance store. `wire-pending` stub→fetch swap to `GET /ontology/instances?type=`.
- **Governance baked in**: a non-authoritative UI feature projection shapes the `KPI_READ` / MANAGEMENT_ROLES nav offer (`console/shell/nav.ts`); the current server/legacy authorization matrix re-authorizes every call, and applicable data tables retain their RLS boundaries. Cedar remains target/shadow until per-action enrollment, shadow evidence, and explicit promotion under ADR-0021 and `docs/specs/cedar-pbac-coexistence-map.json`; current coexistence entries are `legacy_only`. Console semantics follow ADR-0023 as amended by ADR-0025.

**Maturity**: KPI dashboard = source-wired to the real-API contract (rebuilt fe-fix-wave1, ledger line 189); browser, deployment, activation, and production operation remain unverified. Projection + config-console dashboard builder = source-implemented, **stub-fed, wire-pending Phase C** (ledger line 153). The fixed-target source is strong on *grammar and governance*, thin on *breadth of data sources and chart types*.

---

## 1. Capability matrix

Legend per cell: 1-3 lines, **[V]**/**[I]**. Vendors that don't play a module get **N/A + reason**.

### Row 1 — Information architecture (how a dashboard is composed)

- **Ours**: 4-slot grid, widget palette, whole layout = one governed config doc; exec dashboard is a fixed scope×period×stat-strip grammar. Ontology object types are the data source. [code]
- **Palantir Foundry (Quiver)**: blank dashboard, drag-and-drop cells from an analysis; object-analytics cards + charts; parameter/metric cells for KPIs. **[I]** (palantir.com/docs/foundry/quiver/dashboards-overview, dashboards-create)
- **SAP (SAC/embedded)**: "Stories" composed of pages, tiles, charts, tables, geo maps; embeddable in S/4HANA Fiori launchpad. **[I]** (sap.com/products/data-cloud/cloud-analytics.html)
- **Asana**: project/portfolio/**universal** dashboards; add chart widgets to a dashboard view; ready-made chart templates. **[I]** (asana.com/features/goals-reporting/reporting-dashboards)
- **Rippling**: drag-and-drop dashboards; custom reports as the compositional unit, pinned into dashboards. **[I]** (rippling.com/platform/analytics)
- **n8n**: no free-form dashboard designer; a fixed **Insights** dashboard (summary banner + per-workflow table). **[I]** (docs.n8n.io/insights/)
- **Slack**: fixed analytics dashboard, top-level sections **Overview / Channels / Members** (Enterprise adds more); not composable. **[I]** (slack.com/help/articles/360057638533)
- **MS Teams (Viva Insights + Power BI)**: no native composable dashboard in Teams; composition happens in **Power BI** via pre-built templates. **[I]** (learn.microsoft.com/viva/insights/tutorials/power-bi-teams)

### Row 2 — Data model behind the dashboard **[I]**

- **Ours**: **ontology object types** (typed props + link types + actions) — dashboards are typed projections over the same registry explore/policy/workflow consume, never a separate store. [code: configconsole/types.ts, ledger §84]
- **Foundry**: the **Ontology** (objects, links, actions) — Quiver is object-driven analysis over it. **[I]** (palantir.com/docs/foundry/quiver/overview)
- **SAP**: CDS views / S/4HANA embedded analytics + SAC live models; no ETL replication for embedded. **[I]** (metricasoftware.com/sap-s4hana-reporting-options…)
- **Asana**: tasks/projects/portfolios/goals as the queryable objects. **[I]** (help.asana.com universal-reporting)
- **Rippling**: unified employee graph across HR+IT+finance + 3rd-party apps; **SQL-like joins + report formulas**. **[I]** (rippling.com/blog/unify-and-level-up…custom-reports)
- **n8n**: workflow-execution telemetry only (production executions, failures, time-saved). **[I]** (docs.n8n.io/insights/)
- **Slack**: workspace usage events (messages, active days, channel membership). **[I]** (slack.com/help/articles/360057638533)
- **Teams/Viva**: M365 collaboration signals (mail/meeting/chat/calendar metadata). **[I]** (learn.microsoft.com/viva/insights/copilot-analytics-introduction)

### Row 3 — Core KPI / metric primitives

- **Ours**: domain KPI rollups (completed count, response/completion seconds, due-compliance bps, revisit/delay bps, inspection-plan %, P1 acceptance) computed backend; ops SLA counters. [code: DashboardScreen kpiStats/opsStats]
- **Foundry**: parameter/metric cells + object aggregations (count, set math, linked-set); metric = highlighted KPI cell. **[I]** (palantir.com/docs/foundry/quiver/dashboards-create)
- **SAP**: calculated measures, restricted/calculated KPIs, thresholds, variances vs plan version. **[I]** (sap.com data-cloud/cloud-analytics)
- **Asana**: number/column/line/burn-up/donut/lollipop charts; KPI = "number" chart tied to a goal target. **[I]** (asana.com reporting-dashboards)
- **Rippling**: headcount/hiring/turnover/comp/payroll-cost/IT-spend metrics from recipes. **[I]** (rippling.com/hr-metrics-reporting)
- **n8n**: total/failed executions, failure rate, time-saved (fixed or path-derived). **[I]** (docs.n8n.io/administer/observe-and-log/track-usage-with-insights)
- **Slack**: messages posted, days active, DAU/WAU-style activity. **[I]** (slack.com/help/articles/360057638533)
- **Teams/Viva**: collaboration hours, focus/after-hours, meeting load, Copilot usage. **[I]** (learn.microsoft.com/viva/insights/org-team-insights/copilot-analytics-reports)

### Row 4 — Quantitative projection / forecasting

- **Ours**: **deterministic EWMA point + CI95 + CVaR95 fat-tail (Student-t ν=4), no AI**; auditable client math, backend Monte-Carlo/EVT wire-pending. Distinctive: source-implements *risk* (tail loss), not just a trend line. [code: projection.ts]
- **SAP**: **Smart Predict** time-series forecast + classification/regression; predictive forecast in charts. **[I]** (sap.com cloud-analytics; savictech.com)
- **Foundry**: point-and-click ML + time-series analysis in Quiver; models feed dashboard cells. **[I]** (palantir.com/docs/foundry/quiver/overview)
- **Asana**: trend lines / burn-up projection of completion; the cited chart surface is descriptive, and statistical forecasting remains unverified in this sample. **[I]**
- **Rippling**: descriptive analytics; predictive is not a core surface. **[I]**
- **n8n**: "time saved" is a static or path multiplier, not a forecast. **[I]** (docs.n8n.io insights time-saved)
- **Slack**: **N/A** — usage analytics only, no forecasting. **[I]** (peoplelogic/slack limitations note)
- **Teams/Viva**: trend comparisons, no user-facing statistical forecast in-app (analyst templates aside). **[I]**

### Row 5 — Profitability / financial analytics

- **Ours**: 계약 수익성 / 인건비 추이 are **designed but omitted until backed** (no fabricated section) — 수익성 analytic is a named default ontology type not yet seeded. [ledger §194; DashboardScreen doc comment]
- **SAP**: **native strength** — margin/cost-center/profit-center analytics, plan-vs-actual, write-back to CO/FI. **[I]** (sap.com cloud-analytics; blog.sap-press.com)
- **Foundry**: profitability modelable as ontology objects + functions; not out-of-box. **[I]**
- **Rippling**: cost analytics on payroll/IT **spend**, not P&L margin. **[I]** (rippling.com/platform/analytics)
- **Asana**: budget/spend-over-time tracking, not accounting profitability. **[I]** (asana.com reporting — "spending over time")
- **n8n / Slack / Teams**: **N/A** — no financial data domain. **[I]/[I]** (product scope)

### Row 6 — Drill-down / interactivity

- **Ours**: every available stat is **authored as a drill** — stat-strip Links target filtered source routes, while chart callbacks select a scope or navigate to a route. ⚠️ **Known wiring gap** (ia-layout lens, code-confirmed): the absolute React Router targets (`/dispatch`, `/approvals`, `/ops`, and peers in `DashboardScreen.tsx`) are registered by `AppRouter` as legacy `ConsoleShell`/`AppShell` routes, but they exit the carbon-console shell and bypass its `state.screen`/ObjectCard flow. This proves authored registered route exits, not universal drill-to-governed-object behavior; browser behavior remains unverified. Route the exits through the carbon-console `state.screen`/ObjectCard model and browser-prove them before asserting a "no dead numbers" law. [code: AppRouter, DashboardScreen, ConsoleApp.tsx state.screen nav]
- **Foundry**: click a mark → filter/linked object set → open object view; deeply object-native drill. **[I]** (palantir.com/docs/foundry/quiver/dashboards-overview)
- **Asana**: each chart interactive — click a data point → the exact tasks/projects/goals. **[I]** (asana.com reporting-dashboards)
- **SAP**: drill-through hierarchies, linked analysis across widgets, jump-to Fiori app. **[I]** (learning.sap-press.com/sap-analytics-cloud)
- **Rippling**: click into report rows / underlying records. **[I]** (report-builder pattern)
- **n8n**: click a workflow row → its executions. **[I]** (docs.n8n.io/insights)
- **Slack**: minimal — export CSV, limited in-dashboard drill. **[I]** (slack.com/help/articles/360057638533)
- **Teams/Viva**: drill in Power BI, not in Teams surface. **[I]**

### Row 7 — Permissions / row-level governance of the data shown

- **Ours**: dashboard renders scope rollups returned by APIs under the current server/legacy authorization matrix; a non-authoritative UI feature projection shapes nav affordances, and RLS remains the row boundary where implemented. It does not use a live `ontQuery`/Cedar residual-filter pipeline. Cedar object/property policy and residual filtering are target/shadow work only until an action is enrolled, shadow-proven, and promoted; current coexistence entries are `legacy_only`. [code: nav.ts, policy/authz.ts; ADR-0021 + docs/specs/cedar-pbac-coexistence-map.json; console: ADR-0023 amended by ADR-0025]
- **Foundry**: Ontology security markings + object/property ACLs propagate into Quiver. **[I]** (Foundry security model; not stated in Quiver dashboard doc directly)
- **SAP**: analytic authorizations / data-access controls on CDS + SAC roles. **[I]** (sap-press/embedded analytics roles)
- **Rippling**: role-based report permissions or unrestricted-viewer sharing. **[I]** (rippling custom-reports blog)
- **Asana**: dashboard visibility follows project/portfolio membership; no field-level row policy. **[I]** (Asana sharing model)
- **n8n**: Insights gated by plan + instance RBAC (owner/admin). **[I]** (docs.n8n.io insights plan-gating)
- **Slack**: org policy can restrict analytics to admins; private-channel data Enterprise-only. **[I]** (slack.com/help/articles/360057613913)
- **Teams/Viva**: strict privacy aggregation (min-group-size), M365 role scoping. **[I]** (learn.microsoft.com viva copilot-analytics-introduction)

### Row 8 — Config-as-governed-data / approval to publish a dashboard

- **Ours**: **standout** — a dashboard layout is a governed object; personal save is audited, **team deploy requires 결재 (AP- approval)**, draft→approve→effective + rollback (전자결재-native). [code: DashboardEditor comment; ledger §21] **[I]**
- **SAP**: SAC content transport / lifecycle across dev→prod; approval via transport, not per-dashboard four-eyes. **[I]** (SAP transport model)
- **Foundry**: dashboards versioned; publish via Marketplace product packaging; branching in Foundry. **[I]** (palantir.com/docs/foundry/quiver/dashboards-marketplace)
- **Asana / Rippling / n8n / Slack / Teams**: **N/A** — publishing a dashboard is not an approval-gated governance event; edit-and-share model. **[I]** (no four-eyes publish surface in product docs)

### Row 9 — Honest / anti-deceptive visualization

- **Ours**: axis truncation is code-governed (0 baseline unless variance<1/3) and forces a mandatory warn chip. The sampled citations do not show an equivalent mandatory disclosure chip. **[I]** [code: honestScale.ts]
- **The seven sampled product surfaces**: **N/A / not a feature** — truncated axes are user-selectable with no mandatory disclosure; none surface an "axis truncated" governance chip. **[I]** (standard BI behavior; absence is the finding)

### Row 10 — Automation / alert hooks from a metric

- **Ours**: dashboards feed the same ontology-action/Automate surface (effect = ontology action); SLO breach thresholds drive alerts (support-slo). [ledger line 130/153; code supportslo strings]
- **n8n**: **native** — metrics + external alerting (Grafana/SigNoz), thresholds → workflows. **[I]** (grafana.com/grafana/dashboards/24475; signoz.io n8n)
- **SAP**: SAC threshold alerts, data-driven notifications, planning triggers. **[I]** (sap cloud-analytics)
- **Foundry**: **Automate** — object/metric conditions → actions/notifications. **[I]** (palantir Automate; ledger §19 benchmark)
- **Asana**: rules on tasks, goal-status auto-updates; dashboard-metric→action is indirect. **[I]**
- **Rippling**: workflow automations on HR data thresholds. **[I]**
- **Slack**: **N/A** — analytics is read-only, no metric-triggered automation. **[I]** (dashboard is reporting-only)
- **Teams/Viva**: Viva goals/nudges; not dashboard-metric automation. **[I]**

### Row 11 — Mobile

- **Ours**: console is responsive (min 44px targets, overflow-scroll strips), native field app is separate (push, not dashboards). **[I]/[code]** (responsive tokens; no dedicated exec mobile dashboard app)
- **SAP**: SAP Analytics Cloud mobile app (iOS). **[I]** (sap.com cloud-analytics)
- **Foundry**: mobile / Carbon delivery to operational users. **[I]** (palantir quiver dashboards — "delivered in Carbon")
- **Asana**: mobile app renders dashboards (limited). **[I]**
- **Rippling / Slack / Teams**: full mobile apps; analytics viewing on mobile. **[I]/[I]** (native apps)
- **n8n**: web only, no mobile dashboard. **[I]**

### Row 12 — Extensibility / custom compute

- **Ours**: widgets recompute from (config, rows); custom metrics via ontology action-types/derived analytics; new object type auto-wires a surface (with gaps). [ledger §78/§84]
- **Foundry**: **Functions** (TypeScript/Python) power custom metric cells; prominent here. **[I]** (palantir functions; quiver ML)
- **SAP**: custom CDS + scripted calculations + R visual. **[I]** (sap-press SAC)
- **Rippling**: report formulas + SQL-like joins. **[I]** (rippling custom-reports)
- **n8n**: the whole product is extensibility — any node feeds a metric. **[I]** (docs.n8n.io)
- **Asana**: fixed chart types, no custom compute. **[I]** (asana chart list is fixed)
- **Slack**: `admin.analytics.getFile` API for raw export → BYO compute. **[I]** (docs.slack.dev admin.analytics.getFile)
- **Teams**: Power BI = unlimited external compute. **[I]** (learn.microsoft power-bi-teams)

### Row 13 — Audit / compliance of analytics access

- **Ours**: drill targets are audited screens; every config change is an audit event; append-only effective-dated event log underneath. [ledger §77/§83]
- **SAP**: SAC audit + S/4 read logging via GRC. **[I]**
- **Foundry**: full lineage + access audit on ontology. **[I]** (Foundry platform audit)
- **Rippling / Slack / Teams / n8n / Asana**: admin audit logs exist at platform level; **not analytics-cell-level lineage**. **[I]**

### Row 14 — Korean B2B fit (전자결재, 근로기준법, group-company scoping)

- **Ours**: **native** — 결재-gated dashboard publish, Group→법인→branch→worksite scoped rollups, Korean labor metrics (연차/근태/급여) as first-class ontology types, ko-first UI. [ledger conglomerate pivot; DashboardScreen ko strings]
- **SAP**: localizes KR payroll/statutory but 전자결재 is bolt-on; heavy. **[I]**
- **All others (Foundry/Asana/Rippling/Slack/Teams/n8n)**: **global-generic** — no 전자결재 approval semantics, no 법인-tier scoping grammar, no 근로기준법 metric catalog; would need heavy config. **[I]** (none document Korean 전자결재/group-scoping natively)

---

## 2. Per-vendor: "how they'd build OUR dashboard module"

**Palantir Foundry** — Model everything as ontology objects (WorkOrder, Contract, Employee, Shift) and let Quiver do object-driven analysis: KPI cells as metric parameters, drill = filter-to-linked-object-set opening an Object View, Automate for SLA breach → action. Closest philosophical twin to us (object-first, action-writeback). They'd out-build us on ML metric cells + Functions; they'd *under*-build the 전자결재 publish gate and honest-axis law (not their concern). Our whole grammar is basically "Foundry for a Korean conglomerate with governance baked in."

**SAP (SAC + S/4HANA embedded)** — A "Story" over live CDS models: executive page with variance-vs-plan KPIs, Smart Predict forecast on 정비비/수익성, geo map of worksites, write-back planning for budgets. They'd nail profitability + forecasting + planning (their crown jewels) and mobile. They'd deliver it as heavy, consultant-configured, Fiori-embedded content — the opposite of our lightweight self-serve token grammar, and 전자결재 would be a transport-approval bolt-on, not per-dashboard four-eyes.

**Asana** — A universal reporting dashboard: pull WorkOrders-as-tasks across portfolios, number charts for KPI-to-target, column/burn-up for throughput, click-through to tasks. Fast, template-driven, genuinely no-code — their strength is time-to-first-chart. They'd miss risk/CVaR, profitability, field-level row policy, and any governance-of-publish. It'd look great and be governance-shallow.

**Rippling** — A drag-and-drop workforce dashboard: headcount/turnover/labor-cost KPIs joined (SQL-like) across HR+IT+spend, role-based sharing. Strong on unified people+cost data and formula metrics. But maintenance/work-order operational KPIs and profitability-by-contract sit outside its employee-graph model, and it has no projection/risk or approval-to-publish. **[I]**

**n8n** — Wouldn't build an exec analytics dashboard; it'd build the **pipeline that feeds one** plus a fixed Insights panel (executions, failure rate, time-saved) and push thresholds to Grafana/SigNoz. Great as our *automation + alerting substrate*, N/A as the analytics UI itself.

**Slack** — Only builds a usage-analytics dashboard (messages, active members, channels) and exposes `admin.analytics.getFile` for BYO-BI. It would never be our operational/financial dashboard; it's the collaboration-adoption lens only.

**Microsoft Teams (Viva Insights)** — Delivers manager/leadership *collaboration* insights inside Teams and punts real dashboarding to Power BI templates. Our KPIs would live in Power BI, embedded back as a Teams tab. Strong privacy-aggregation and unlimited Power BI compute; weak as an in-app, object-native, governed dashboard.

---

## 3. What we'd steal (ranked, actionable) **[I]**

1. **Object-driven drill + linked-object-set filtering → open Object View** — *Palantir Foundry* is the cited reference for this capability. **Fit: native** — we already drill to screens; upgrade drills to open the 3-layer ObjectCard filtered to the exact linked set, not just a route. **Cost: M.** **[I]**
2. **Statistical forecasting on the metric (Smart Predict-style) behind our projection shape** — *SAP SAC*. **Fit: high** — our `Projection` interface already reserves the slot; swap client EWMA for backend Monte-Carlo/EVT (Phase C) and add a forecast band to trend charts. Keep determinism/auditability (no black-box AI). **Cost: L** (backend). **[I]**
3. **Ready-made chart templates / one-click dashboard scaffolds** — *Asana*. **Fit: high** — layer template presets over our config-doc builder so a manager gets a useful dashboard in 3 clicks without hand-placing widgets. **Cost: S.** **[I]**
4. **SQL-like joins + report formulas for custom metrics** — *Rippling*. **Fit: medium** — expose a formula/derived-metric field on ontology property defs so users compute KPIs no-code across linked object types. **Cost: M.** **[I]**
5. **Metric-threshold → automation/alert wiring surfaced on the dashboard** — *n8n / Foundry Automate*. **Fit: native** — connect our SLO-breach + stat thresholds directly to the Automate ontology-action surface with an "alert when" affordance on any stat. **Cost: M.** **[I]**
6. **Profitability / plan-vs-actual variance KPIs with write-back** — *SAP*. **Fit: medium** — seed the 수익성 ontology type + a variance-vs-target metric primitive; write-back stays governed through action-types. **Cost: L.** **[I]**
7. **Raw analytics export API for BYO-BI** — *Slack `admin.analytics.getFile` / Teams→Power BI*. **Fit: medium** — an audited, server-authorized and RLS-bounded `/analytics/export` so enterprises can pull governed metrics into their own BI; Cedar scoping applies only after explicit promotion. **Cost: S/M.** **[I]**
8. **Mobile executive dashboard delivery** — *SAP/Foundry Carbon*. **Fit: medium** — reuse responsive stat-strip in the native app for a read-only exec view (push already exists). **Cost: M.** **[I]**

**Candidate design differentiators in this sampled comparison (defend, don't steal):** honest-axis governance law (Row 9, the sampled citations do not show an equivalent), 전자결재-gated dashboard publish (Row 8), server-authorized scope rollups with RLS as the row boundary (Row 7), and CVaR/tail-risk in a mainstream ops dashboard (Row 4). These are constraints to preserve; every "steal" above must preserve them; Cedar enforcement is future promotion work, not a current advantage. **[I]**

---

### Sources

Palantir: palantir.com/docs/foundry/quiver/{overview,dashboards-overview,dashboards-create,dashboards-marketplace}, /docs/foundry/analytics/overview · SAP: sap.com/products/data-cloud/cloud-analytics.html, metricasoftware.com/sap-s4hana-reporting-options-embedded-analytics-bw-and-sac, blog.sap-press.com/building-an-sap-analytics-cloud-dashboard-with-sap-s4hana-cloud-data · Asana: asana.com/features/goals-reporting/reporting-dashboards, help.asana.com universal-reporting · Rippling: rippling.com/platform/analytics, /hr-metrics-reporting, /blog/unify-and-level-up-your-workforce-analytics-with-ripplings-new-custom-reports · n8n: docs.n8n.io/insights/, /administer/observe-and-log/track-usage-with-insights, grafana.com/grafana/dashboards/24475, signoz.io/blog/n8n-monitoring-with-opentelemetry · Slack: slack.com/help/articles/{360057638533,360057613913}, docs.slack.dev/reference/methods/admin.analytics.getFile · Teams/Viva: learn.microsoft.com/viva/insights/tutorials/power-bi-teams, /copilot-analytics-introduction, /org-team-insights/copilot-analytics-reports · Ours: `web/src/console/{dashboard,charts,configconsole,shell}`, `docs/program/console-program-ledger.md`

---

## 4. Cross-cutting lens findings (5 independent review lenses)

- **Task-flow:** money task = *spot an anomaly → drill to the offending object*. Ours = **2 authored steps** (stat → object), though the current route wiring has the adjudicated gap below. The sampled Workday/SAP surfaces are report-first (3–4 hops). **Steal:** little — hold the line; guard against regressing to big-number KPI-card dashboards (the §4-11 rule already forbids them). Cost **S** (hold the line). **[I]**
- **IA / layout:** candidate strengths in this sampled comparison — server-authorized scope segments, typed month-period segments, one-row authored drill-affordance stat strip, honest-scale charts, omits-unbacked-sections-not-placeholders. **Steal:** fix the drills to route into the `objectExplorer`/screen model (see adjudication) [S]; a smart-filter bar unifying scope+period → SAP Overview Page [M]; user-configurable dashboard [L]. **[I]**
- **Data-model:** tiles carry authored drill affordances, but their React Router targets are not wired into the state.screen shell; universal working drill-to-governed-3-layer-ObjectCard behavior remains unproved. Once wired, metric provenance and object governance can share one model. Looker's LookML is typed+git-versioned but has no lifecycle/as-of on the underlying facts. **Weaker:** BI vendors have mature aggregation/semantic-join engines; our widget chart-binding is still partly stub. **Steal:** Looker's git-versioned semantic-model diff/merge UX for console_view [M]; Power BI incremental-refresh windows [M]. **[I]**
- **Governance:** current server authorization and RLS boundaries are evidenced; Cedar enforcement is not. Aggregates are returned under the server/legacy authorization matrix; Cedar residual filtering and per-widget decision provenance remain target/shadow capabilities until explicit promotion. **Steal:** k-anonymity aggregate-suppression threshold (hide a count when <k rows so a filtered aggregate can't fingerprint an individual) → Foundry restricted-view spirit [M]; after promotion, a per-widget "governed-by" chip (which policy shaped this number) → Cedar decision log [S]. **[I]**
- **Automation / extensibility:** **Steal:** alert-rule trigger (metric threshold → workflow) → Grafana [M]; drill-to-action from a widget (click a stat → run the governed ontology Action on the underlying set) → Foundry/Retool [S–M]; scheduled report/digest effect [M]. **[I]**

**Adjudicated contradiction:** Row 6's "every rendered number is a drill — a design law (no dead numbers)" is **overstated**. Code-confirmed (ia-layout #6): the absolute React Router targets are registered by `AppRouter` as legacy `ConsoleShell`/`AppShell` routes. They exit the carbon-console shell and bypass its `state.screen`/ObjectCard flow, so the source proves registered route exits rather than universal governed-object drill semantics; browser behavior remains unverified. Route the exits through the carbon-console `state.screen`/ObjectCard model before asserting the law. §0/Row 6 above carry the corrected framing.
