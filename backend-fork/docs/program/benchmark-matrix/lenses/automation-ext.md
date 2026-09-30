# Lens Pass — AUTOMATION / EXTENSIBILITY (independent)

Scope: triggers · rules · APIs · webhooks · connectors · app-platforms · code escape-hatches — at each
vendor vs **our workflow studio (Automate) + ontology actions**, across all 14 modules. Independent of the
draft matrices (not read). Rigor: every vendor-capability claim is `[V]` (source URL) or `[I]` (reasoned,
honest). Cost tags S/M/L = rough build cost against our ontology-first, Cedar-PBAC, deterministic-no-AI grammar.

Sources marked `[brief]` are pre-verified in `docs/program/benchmark-brief.md` (URLs inline there).

---

## 0. Our real automation/extensibility state (evidence-based)

Read from `web/src/console/workflows/*`, `console/canvas/*`, `console/policycanvas/*`, and
`docs/program/console-program-ledger.md`.

**What exists (wired, Phase C wave 1 — ledger L160):**
- **Automate hub** = Workflow / Schedule / Monitor tabs, real workflow-studio REST (run / simulate /
  toggle / four-eyes publish stage→approve→withdraw), `RunLogTimeline` over a real runLog
  (`WorkflowRunEvent`: status, actor, error, `retryable`, `retryCount`, `generatedObjects`). Evidence:
  `WorkflowAutoScreen.tsx`, `workflows/types.ts`.
- **Block grammar** = `WorkflowBlockKind = "trigger" | "condition" | "branch" | "action"`
  (`workflows/types.ts:21`). `BlockCanvas` with typed nodes, 2px connectors, branch ≥2 outputs,
  field·op·value `PredicateEditor`, real-eval `SimulationPanel` (ledger L147).
- **Triggers today**: (a) **cron schedule** (`ScheduleSummary.cron` + `cronLabel` + `nextRun`), (b)
  **object-data monitors** ("monitors-as-definitions", ledger L160). That is the whole trigger taxonomy.
- **Effects today**: **"Automate effect = ontology action"** (ledger L130 #65) — an effect submits a
  governed ontology Action through `actions/execute`, the SAME verb a human invokes. No other effect type.
- **Governance of the automation itself**: draft→stage→**four-eyes approve** (self-approval blocked,
  `sameActor` guard + backend `gov_approvals` CHECK, ledger L135) →effective, version history, as-of,
  content-hash rollback. `PolicyGated` shapes advisory UI affordances; live authorization remains the
  legacy server boundary until each Cedar action is enrolled and promoted.
- **Ontology extensibility**: no-code **add-a-type** with the intent that a new type wires itself
  end-to-end (instances CRUD, module surface, policy resource, automation triggers, graph, i18n, route) —
  ledger L78. This is our extensibility story: extend the **model**, not plug in third-party code.

**What does NOT exist (grep-confirmed absent from console/workflows + ledger residuals):**
- **No inbound webhook trigger.** No "catch hook → run workflow". No `webhook` block kind.
- **No external-API / outbound-webhook effect.** Effects are ontology actions only; Foundry Automate's
  webhook/external-API/notification effects have no analog here.
- **No connector / spoke / integration catalog.** Zero pre-built third-party connectors.
- **No external-facing public API, API keys, or app platform / manifest / marketplace.** `openapi.yaml`
  is the internal console contract, not a partner-extensibility surface (ledger L160 ⑤).
- **No code / function escape-hatch** for automation logic (no Code node, no user-authored functions).
  Deliberate under the deterministic-**no-AI** mandate, but it is a real ceiling for power users.
- **Notification-as-effect** not confirmed wired (notif backend exists #198; effects = ontology action).
- **§16 checklist / four-eyes / SoD API-layer enforcement for automation** still a backend residual
  (ledger L116, "85 판정").

Bottom line: we are a **governed, internal, model-extensible** automation engine. The sampled automation-vendor
surfaces are **open, connector-rich, and externally extensible**. The gap and the design distinction are the
same fact.

---

## 1. AUTOMATE (n8n / Zapier / Power Automate / Foundry Automate / Temporal)

| Capability | n8n | Zapier | Power Automate | Foundry Automate | **Ours** |
|---|---|---|---|---|---|
| Inbound webhook trigger | Webhook node, unique URL, GET/POST `[V]` | Catch Hook / Catch Raw Hook `[V]` | webhook triggers (push) `[V]` | webhook effect (out), object monitors (in) `[brief]` | **none** |
| Schedule/cron trigger | Schedule/Cron trigger `[V]` | Schedule `[I]` | scheduled flow `[V]` | time-based condition `[brief]` | **cron schedule ✓** |
| Data/record-change trigger | polling triggers `[I]` | polling `[I]` | polling triggers `[V]` | object-added/modified/all monitor `[brief]` | **object monitor ✓** |
| Conditional branching | IF/Switch nodes `[I]` | Paths + Filters `[V]` | conditions/switch `[I]` | multiple conditions `[brief]` | **branch/condition blocks ✓** |
| Pre-built connectors | 400+ built-in + community nodes `[V]` | 8,000+ apps `[V]` | 1,000+ (std/premium) `[V]` | via effects/pipelines `[brief]` | **0 (ontology actions only)** |
| Code escape-hatch | Code/Function node (JS) `[V]` | Code by Zapier `[I]` | custom connector `[V]` | Function-backed actions `[brief]` | **none (no-AI/determinism)** |
| Durable replay / history | execution log `[I]` | task history `[I]` | run history `[I]` | — | shallow retry (`retryCount`) |
| Governed publish (approval, rollback) | flat versions `[I]` | flat `[I]` | solutions/env `[I]` | branch+proposal `[brief]` | **four-eyes + as-of + hash ✓✓** |
| Effect = same verb humans use | no (service account) `[I]` | no `[I]` | no `[I]` | **yes** `[brief]` | **yes ✓✓** |

Durable-execution benchmark = **Temporal**: append-only Event History, deterministic replay, effectively-once
side effects `[brief]`. Our retries are `retryable`/`retryCount` flags on runLog events — not a replayable log.

**What we'd steal (ranked):**
1. **Inbound webhook trigger** → selected references: n8n/Zapier → new `WorkflowBlockKind:"webhook"`; the URL mints a
   Cedar-scoped ingress principal, payload maps into an ontology action's parameters. Fits "humans+automation
   share one mutation surface." **M** (needs a public ingress route + HMAC verify + replay guard).
2. **Effect taxonomy beyond ontology-action** → Foundry Automate (action / function / **notification** /
   **webhook-out**) → add `effect.kind` discriminated union; notification + outbound-webhook first. **M**.
3. **Durable event-history + replay** → Temporal → fold runLog into an append-only, effectively-once event
   store (we already have L20 fixity + append-only instances to reuse). **L**.
4. **Reusable subflow / named action group** → ServiceNow subflows `[V]` → workflow-as-callable. **S–M**.

---

## 2. COMMS (Slack platform / Teams / Zendesk-notify)

| Capability | Slack | Teams | **Ours** |
|---|---|---|---|
| Slash commands (keystroke → app) | `/cmd` installable per-workspace or via Marketplace `[V]` | commands `[I]` | **none** (messenger has `#code` objDrag markers only) |
| Events API (subscribe HTTP) | pick events → HTTP delivery `[V]` | Graph subscriptions `[I]` | **none** |
| Interactive message components | Block Kit stackable blocks + interactivity `[V]` | Adaptive Cards `[I]` | messenger cards (internal) |
| No-code workflow builder in chat | Workflow Builder: triggers/steps/vars `[V]` | Power Automate in Teams `[I]` | **Automate hub (separate surface)** |
| App manifest / portability | YAML/JSON manifest, portable config `[V]` | app manifest `[I]` | **none** |
| Marketplace distribution | Slack Marketplace `[V]` | Teams store `[I]` | **none (single-tenant internal)** |
| Link unfurl (event → metadata → post-back) | `link_shared` → `chat.unfurl` `[brief]` | — | objDrag `#code` markers (internal analog) |

Our repository contains an in-app messenger and audit surfaces (`backend/crates/messenger`,
`web/src/console/messenger`); the cited paths do not prove E2EE or native-push posture.
A power user coming from Slack loses **slash commands** and **chat-native workflow triggers** most.

**What we'd steal (ranked):**
1. **Chat-native workflow trigger** → Slack Workflow Builder `[V]` → a messenger message/marker fires an
   Automate workflow (objDrag markers already exist as a drop target). Fits ontology grammar cleanly. **M**.
2. **Slash-command → ontology action** → Slack `[V]` → `/wo close WO-123` runs the governed action, Cedar-gated,
   audited. Big power-user win, on-brand with token grammar. **M**.
3. **Interactive approval blocks in messenger** → Block Kit `[V]` → inline four-eyes approve/deny in the
   comms rail. **S** (approvals + messenger both exist).
4. App manifest / marketplace → N/A for a single-conglomerate internal platform (YAGNI, say so).

---

## 3. APPR — 전자결재 / approvals (sampled product references are weakly matched)

Global automation vendors do NOT model Korean 전자결재 as first-class: **결재선**(approval line), **전결규정**
(delegation-of-authority matrix), **대결/전결/합의/병렬**(deputy/final/concur/parallel), and 근로기준법-bound
routing are absent from Slack/Zapier/ServiceNow flows — they offer generic sequential/parallel approval steps
only `[I]` (reasoned from their published step models). Closest global benchmark = **Workday Business Process
framework**: ordered steps (approval/to-do/checklist/integration/notification) gated by condition rules +
routing modifiers, ending in a mandatory commit step `[brief]`.

**Ours**: governance four-eyes approve/withdraw (self-approval banned, `gov_approvals` CHECK), AP- approval
objects, console-change AP- template gating (ledger L137 #73). This design contains **more of the Korean 전자결재 grammar than the sampled global-product
surfaces** but lack 결재선-as-config.

**What we'd steal (ranked):**
1. **Routing-modifier rules as governed config** → Workday BP `[brief]` → 전결규정 becomes a governed ontology
   object (amount/type → 결재선), evaluated by the same predicate engine as policies. Local-fit recommendation. **M**.
2. **Parallel + 합의(concur) + 대결(deputy) step types** → Workday step taxonomy → extend AP- lifecycle FSM.
   **M**.
3. **Mandatory commit step** → Workday `[brief]` → make "결재 완료" an explicit terminal transition that
   triggers downstream automation (approval → auto-run ontology action). **S**.

---

## 4. SUPPORT (Zendesk / ServiceNow ITSM)

| Capability | Zendesk | ServiceNow | **Ours** |
|---|---|---|---|
| Event triggers (ticket create/update) | Triggers (event-based) `[V]` | Flow Designer record triggers `[V]` | object monitor `[brief]` (partial) |
| Time-based automations | Automations (time-elapsed) `[V]` | scheduled flows `[V]` | cron schedule ✓ |
| Outbound webhook on event | webhook → HTTP on event `[V]` | IntegrationHub REST `[V]` | **none** |
| Custom objects + object triggers | custom_objects_v2 + object_triggers `[V]` | tables + flows `[I]` | **ontology types + monitors ✓** |
| App framework | ZAF (`client.on/get`) + Marketplace `[V]` | scoped apps + Spokes `[V]` | **none (no-code type authoring instead)** |
| Reusable action steps | — | Custom Actions + Subflows + Spokes `[V]` | ontology actions ✓ (not composable-as-subflow) |

We have SUP- tickets and object monitors (`backend/crates/support`, `web/src/console/screens/support`).
Zendesk/ServiceNow win on **webhook-out on ticket event**
and **spoke/app extensibility**; we win on **typed custom objects with governance** (their custom objects are
schema-thin vs our ontology).

**What we'd steal (ranked):**
1. **On-ticket-event → workflow** as a first-class trigger (not just a periodic monitor) → Zendesk triggers
   `[V]` → lifecycle-transition trigger kind (SUP- state change fires Automate). **S–M** (monitors exist).
2. **Reusable action groups (subflows/spokes)** → ServiceNow `[V]` → package ontology-action sequences as a
   named, versioned, Cedar-scoped unit. **M**.
3. **SLA-breach timer trigger** → Zendesk time automations `[V]` → we have SLO setting objects (BE2); wire an
   SLA-clock trigger. **M**.

---

## 5. FIELD (ServiceNow FSM / Salesforce Field Service)

Both drive field automation through their platform engines: ServiceNow **Flow Designer** (record/schedule/API
triggers, custom actions, spokes) `[V]`; Salesforce Field Service via **Flow + Apex** triggers `[I]`. Dispatch,
SLA timers, and mobile-work-order state changes are the automation surface.

**Ours**: WO- work orders have a domain FSM (`backend/crates/workorder/domain` and its FSM tests); projecting that FSM into the ontology remains a target. The "cover-planner /
사전 대근 cron" is an explicit TODO (ledger L116). A field power user expects **on-status-change** and
**geo/SLA** triggers.

**What we'd steal (ranked):**
1. **WO lifecycle-transition trigger** (CRTD→REL→TECO→CLSD, SAP PM analog `[brief]`) → firing Automate on the
   projected FSM transition. Fits "kinetic layer = every transition is an audit event." **M**.
2. **사전 대근 / cover-planner cron** → already scoped `[V-internal]` → schedule trigger + ontology action
   (substitution). **S** (schedule engine exists).
3. **Escalation timer (SLA breach → reassign)** → ServiceNow/Salesforce `[I]` → SLO object + timer trigger. **M**.

---

## 6. PEOPLE (Workday) & 7. LEAVE

**Workday Business Process framework** is the benchmark for both: every HR action (hire, absence, staffing) is
an event instance through a configurable BP — ordered steps, condition rules, routing modifiers, mandatory
commit; all records effective-dated `[brief]`. BambooHR-class tools add **webhooks + REST API** on HR events
`[I]`.

**Ours**: HR/payroll crates exist and leave has `hr_leave_workflow` (migration 0111, ledger L188);
effective-dated ontology instances already exist (append-only, as-of). We have the **substrate** Workday relies
on. Missing: HR-event **triggers** (on-hire → provision, on-leave-approve → adjust balance) as automation, and
outbound HR webhooks.

**What we'd steal (ranked):**
1. **HR-event lifecycle triggers** → Workday BP `[brief]` → on-approve-leave → auto ontology action
   (balance decrement, 연차촉진 round). We have effective-dating + actions; just need the trigger kind. **S–M**.
2. **연차촉진 round scheduler** → 근로기준법-specific; an equivalent was not observed in the sampled product sources `[I]` → schedule trigger + notification
   effect (촉진 통보). Local-fit. **M** (needs notification effect first).
3. **Routing modifiers by org scope** (Group→법인→branch→worksite) → Workday `[brief]` → our scoped RBAC already
   models the hierarchy; bind routing to it. **M**.

Korean mismatch: Workday/BambooHR do not encode 근로기준법 leave-accrual or 연차촉진 obligations — our niche
catalog (ledger L78) is the differentiator; automation must carry it.

---

## 8. FINANCE (SAP / NetSuite)

**SAP** automates via workflow + BAPIs/IDocs on balanced-document postings; **NetSuite** via SuiteFlow
(record-triggered) + SuiteScript `[I]`. The automation invariant: financial mutations are **balanced documents
(Σdr=Σcr)** and work orders move through a status profile with cost-posting gates `[brief]`.

**Ours**: FinanceModuleScreen exists; ERP depth (ledger-integrity, MM 3-way match, PM orders) is a build
target (ledger L130 #64). Finance automation should be **document-triggered** (on-post → downstream action),
never free-form.

**What we'd steal (ranked):**
1. **Document-posting trigger with tolerance gate** → SAP 3-way match `[brief]` → GR/IR clearing nets within
   tolerance → auto-release or block-invoice action. Deterministic, on-brand. **L** (needs ERP depth first).
2. **Balanced-document invariant as a submission-criterion** → SAP GL `[brief]` → workflow refuses to submit an
   unbalanced posting (our predicate engine can express Σdr=Σcr). **M**.
3. **부품부족 → PO** reorder automation → SAP PM `[brief]` → monitor(stock<min) → create-PO action. **M**.

---

## 9. OBJECT-PLATFORM (Palantir Foundry)

Foundry IS our north star and its extensibility model is the one we're consciously copying: **one ontology,
many consumers**; Actions = the only mutation verb; Automate monitors over object sets → effects (action /
function / notification / webhook); Functions (TS/Python) as the code escape-hatch; **OSDK / API** for external
callers `[brief]`. Governance = branch + proposal + merge-check + changelog `[brief]`.

**Ours**: single `ONT_TYPES` registry consumed by explore/policy/workflow/modules (ledger L19, L84); actions →
writeback; Cedar object/property policies; revision staging. We match the **shape**. We lag on: (a) **Functions**
(no user code-backed actions), (b) **external API / OSDK** for third parties, (c) webhook/notification effects.

**What we'd steal (ranked):**
1. **Effect parity with Foundry Automate** (action/function/notification/webhook) → directly `[brief]` → the
   single highest-leverage automation gap. **M–L**.
2. **Deterministic "function-backed action"** (a governed, sandboxed, no-AI transform) → Foundry Functions
   `[brief]` → escape-hatch without violating no-AI. **L**.
3. **OSDK-style typed external API** → Foundry `[brief]` → if/when we open to partner integration; likely YAGNI
   for a single conglomerate today — flag, don't build. **L**.

---

## 10. POLICY (Cedar / OPA)

Extensibility here = **policy-as-code** authored no-code. Cedar: schema-validated policies, templates
(`?principal`/`?resource` slots), partial-eval residuals → SQL WHERE `[brief]`. OPA: rego + bundles + decision
logs `[I]`. Both are extended by **writing policies as governed data**, not plugins.

**Ours**: no-code P→R→A→Effect canvas + typed predicates + server-backed simulator (ledger L47, L152); Cedar
authoring (`cedar_pbac/authoring.rs`) has a four-eyes review FSM, deny-by-omission, and forbid-wins semantics.
This proves authoring/evaluation substrate, not live-route Cedar enforcement.

**What we'd steal (ranked):**
1. **Policy templates for discretionary grants** → Cedar templates `[brief]` → "share object X with user Y"
   without writing N policies. **S–M** (authoring engine exists).
2. **Decision-log stream** → OPA decision logs `[I]` → every authz decision as an audit/automation event
   (could itself be a trigger: on-deny → notify). **M**.
3. Policy-as-automation-trigger → Cedar `[I]` → on policy-change, re-simulate affected surfaces. **M**.

N/A: the sampled product sources do not show an applicable "app marketplace" concept to policy — extensibility here is intentionally
data-authoring, which we've nailed.

---

## 11. EVIDENCE & 14. COMPLIANCE (Vanta / Drata)

**Vanta/Drata** automation = `Control → Test → Evidence`: automated tests pull evidence from **connected
systems** on a schedule (hourly/daily), cross-framework mapping lets one evidence item satisfy overlapping
controls `[brief]`. Extensibility = a large **integrations catalog** + API to feed evidence `[I]`.

**Ours**: audit-chain seal/verify code (`backend/crates/platform/audit-chain`, migrations `0100`/`0101`),
CP-/RG- compliance objects, and evidence records (EV-). These are partial source seams: production sealing
defaults OFF, trusted external signing/anchoring and object-lock deployment are unproved. We also lack the
**continuous-test scheduler** and **integration-sourced evidence collection**.

**What we'd steal (ranked):**
1. **Continuous control-test scheduler** → Vanta/Drata `[brief]` → schedule trigger (exists) + an ontology
   "test" action that evaluates a predicate over instances → writes timestamped EV-. Deterministic, on-brand.
   **M**.
2. **Cross-framework control mapping** → Vanta `[brief]` → one EV- satisfies many RG- via link types
   (many-many). Pure ontology modeling. **M**.
3. **Evidence-from-integration collectors** → Vanta catalog `[I]` → needs the connector surface we lack;
   internal-source collectors (our own systems) first, external later. **M–L**.

---

## 12. OVERVIEW & 13. DASHBOARD (Grafana / Retool)

**Grafana**: alert rules → contact points (webhook/Slack/email), on-threshold automation `[I]`. **Retool**:
Workflows (cron/webhook-triggered) + custom components + query-triggered actions `[I]`. Dashboard extensibility
= **alert-as-trigger** and **drill-to-action**.

**Ours**: config-console (widget palette, stub-fed count widgets, honest charts, 팀 배포 결재 — ledger L153);
Dashboard source-observed data comes from current API calls, not `ontQuery`. Generic widget→`ontQuery` binding and Cedar
residual filtering are wire-pending target work. ProjectionPanel (CI95/CVaR95) is deterministic, and a
**threshold-alert → automation** loop is not wired.

**What we'd steal (ranked):**
1. **Alert-rule trigger** (metric crosses threshold → run workflow) → Grafana `[I]` → a monitor over an
   aggregation (we have object-set aggregation + monitors; combine them). **M**.
2. **Drill-to-action from a widget** → Foundry Workshop / Retool `[brief]/[I]` → click a stat → run the
   governed ontology action on the underlying set. Fits objDrag + ObjectCard. **S–M**.
3. **Scheduled report/digest effect** → Retool `[I]` → schedule trigger + notification effect (needs
   notification effect). **M**.

---

## Cross-module synthesis

Bounded inference from the sampled product surfaces (observed 2026-07-18): **the cited n8n/Slack-platform
surfaces emphasize outward extension through connectors, webhooks, apps, or code; this design emphasizes
inward ontology-model extension.** Inbound webhook trigger, connector catalog, code node, and app platform
remain deliberate gaps rather than proof of market-wide differentiation. The target gain is one governed ontology Action shape for humans and automation. Cedar evaluation and
an explicit `runs_as` principal still require per-action enrollment, shadow evidence, and promotion. The
highest-ROI investments close the *governed* versions of the
gaps (webhook-in as a Cedar-scoped ingress; notification/webhook-out effects; lifecycle-transition triggers)
without importing the ungoverned service-account model.
