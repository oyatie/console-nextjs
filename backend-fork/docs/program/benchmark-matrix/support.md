# Benchmark Matrix — Module: **support** (Support / CX tickets: intake, SLO, linked objects, comment threads)

Scope of our module (evidence, grepped from this repository):
- Backend crate `backend/crates/support/{domain,application,adapter-postgres,rest}` — status FSM + priority→SLA in `domain/src/lib.rs`; REST in `rest/src/lib.rs`; schema in `platform/db/migrations/0022_create_support.sql`.
- Frontend `web/src/features/support/*` — `SupportTicketList.tsx`, `SupportTicketDetail.tsx`, `SupportTicketPin.tsx`, `CustomerIntakeForm.tsx`, `CreateTicketForm.tsx`, `SloSettingsCard.tsx`, `slo-settings.ts`, `support-format.ts`.
- Ledger: `docs/program/console-program-ledger.md` (§4-26 SLO≠SLA; support row "58 / revise / §4-11 KPI tiles + chip filters + right-pin detail"; the later Phase 0 Support SLO truth-down supersedes the historical `support_slo_setting` parity claims).

**Rigor:** every vendor cell is `[V]` (verified, source URL) or `[I]` (inferred from known product patterns). Our-console cells are code-cited. Vendors that don't play in this space get **N/A + one line**.

> **Phase 0 Support SLO truth-down (2026-07-25; Support-only source overlay
> `55d00f8aacaf8d1ba4db87b2f5345605af856a27`): PARTIAL.** The six-category
> local defaults currently drive ticket timers, breach counts, and alerts. The
> separately seeded three-bucket `support_slo_setting` ontology type is legacy,
> does not serve those computations, and is taxonomy-incompatible with the six
> Support ticket categories. Its settings-card approval is client-only and
> commits multiple writes without backend aggregate atomicity. Ticket `due_at`
> remains an SLA deadline and is not SLO policy authority. The approved next
> architecture is one Support-owned, immutable six-category elapsed-only policy
> aggregate with backend atomic approval. That architecture is not implemented
> here; no migration, backend route, parity, deployment, or completion is
> claimed.

Columns: **Our Console** · Palantir Foundry · Slack · Microsoft Teams · Asana · n8n · Rippling · SAP (Service Cloud / SuccessFactors).

Module-relevant vendor weighting (per brief): **Asana** (service-desk patterns), **Slack/Teams** (huddle/swarm support), **SAP** (CRM service) are the primary sampled references; Foundry (object grammar), n8n (automation), Rippling (IT/HR-linked desk) are secondary but covered.

---

## Capability matrix

### 1. Information architecture (ticket object model)

| Vendor | How |
|---|---|
| **Our Console** | Dedicated `support_tickets` table, first-class `SUP-` object code. Typed enums: origin `INTERNAL`/`CUSTOMER`, 6 categories, 4 priorities, 5-state status. Branch-scoped (`branch_id` nullable for untriaged customer tickets). Separate FSM from the 16-state work-order flow — deliberately not overloaded (`domain/src/lib.rs:1-8`). |
| Palantir Foundry | [V] Ticket = an Ontology object type (semantic props + links + kinetic Actions); no domain ticket ships out of the box — you model it. https://www.palantir.com/docs/foundry/ontology/overview |
| Slack | [V] No native ticket object; a "case" is a channel + threaded message posted by a workflow, linked back to the real ticketing system. https://slack.com/blog/collaboration/salesforces-support-resolves-cases-faster |
| MS Teams | [I] Same — Teams is the surface; the ticket object lives in ServiceNow/Dynamics, mirrored as an adaptive card. https://kanini.com/resources/servicenow-virtual-agent-teams-integration/ |
| Asana | [V] Ticket = a Task in a dedicated project; fields = custom fields; queues = sections. Object model is generic project/task, not a purpose-built case entity. https://asana.com/resources/asana-request-tracking-ticketing |
| n8n | N/A as a system of record — n8n holds no ticket object; it moves ticket data between other systems (Postgres/Slack/Zendesk). https://blog.n8n.io/human-in-the-loop-automation/ |
| Rippling | [I] No native case object in the platform; IT tickets are deflected by HR/identity automation or handed to Zendesk. https://www.rippling.com/blog/zendesk-rippling-integration |
| SAP | [V] Service Cloud V2 replaces V1 flat "tickets" with a lifecycle **Case** entity (system of record, contact/account/product linked). https://www.spadoom.com/en/blog/sap-service-cloud-v2-whats-different/ |

### 2. Intake channels

| Vendor | How |
|---|---|
| **Our Console** | Two channels, both real: (a) authenticated internal create (`CreateTicketForm.tsx`), (b) **unauthenticated customer intake** endpoint (`SUPPORT_INTAKE_PATH`, `CustomerIntakeForm.tsx`) — DB-backed fixed-window rate limit reusing the auth `auth_rate_limit` scheme + `X-Forwarded-For` trusted-proxy-count derivation; ack response is deliberately minimal (no internal IDs leaked). No email-to-ticket, no chat intake yet. |
| Palantir Foundry | [I] Intake = any pipeline writing objects (Workshop form Action, data connector, API); no turnkey email/portal intake. https://www.palantir.com/docs/foundry/action-types/overview |
| Slack | [V] Intake = a Slack **Workflow form** (case #, urgency, subject) that posts into the swarm channel with a deep link to the source ticket. https://slack.com/blog/collaboration/salesforces-support-resolves-cases-faster |
| MS Teams | [V] Intake via Virtual Agent chat / message-extension adaptive cards; ticket created in ServiceNow from the conversation. https://kanini.com/resources/servicenow-virtual-agent-teams-integration/ |
| Asana | [V] **Forms** (each submission → a task with fields pre-populated) + email + a dedicated intake section; strong multi-form intake. https://asana.com/resources/asana-request-tracking-ticketing |
| n8n | [V] Webhook node = intake for anything (form, email parser, chat); DIY, you wire it. https://docs.n8n.io/flow-logic/error-handling/ |
| Rippling | [I] "Intake" is largely eliminated — lifecycle events (hire/promote/team change) auto-provision access, so ~80% of IT tickets never get filed. https://www.rippling.com/blog/it-service-management-software |
| SAP | [V] Channel-agnostic inbox: email, phone, chat, social, web form all land as cases in one queue. https://www.spadoom.com/en/blog/discover-the-power-of-sap-service-cloud/ |

### 3. Triage, assignment & routing

| Vendor | How |
|---|---|
| **Our Console** | Manual triage: self-assign ("claim") + explicit assignee set, gated by `AssigneeManage` (admin-only) feature. Untriaged customer queue is a partial index (`branch_id IS NULL`). No skills/load-based auto-routing. (`rest/src/lib.rs` `assign_ticket`; `SupportTicketDetail.tsx` claim button.) |
| Palantir Foundry | [I] Routing = an Action + automation on the object (assignee property write); you model the rule, no built-in skills engine. https://www.palantir.com/docs/foundry/use-case-patterns/alerting-workflow |
| Slack | [V] "Swarm" model: case posted to product/region channel, a **swarm lead** manages the thread; humans self-select, not algorithmic routing. https://slack.com/blog/transformation/discover-salesforces-recipe-for-customer-success |
| MS Teams | [I] Routing handed to ServiceNow assignment groups; Teams surfaces the assignment as a card. https://aelumconsulting.com/blogs/servicenow-microsoft-teams-integration/ |
| Asana | [V] Rules assign by request type/urgency; AI auto-names and tags; sections move tickets across triage stages. https://asana.com/resources/asana-request-tracking-ticketing |
| n8n | [V] Intelligent routing via conditional branches + AI classify node; fully DIY. https://www.wednesday.is/writing-articles/building-customer-support-automation-with-n8n |
| Rippling | [I] Access requests auto-approved/routed off employee graph + policy; ticket routing largely bypassed. https://www.rippling.com/blog/it-service-management-software |
| SAP | [V] Real routing engine: **skills matching** (language/product/cert), **load balancing** by agent workload, priority queues. https://www.spadoom.com/en/blog/discover-the-power-of-sap-service-cloud/ |

### 4. SLA / SLO tracking & escalation

| Vendor | How |
|---|---|
| **Our Console** | **PARTIAL.** Priority→SLA `due_at` is derived on create (URGENT 4h / HIGH 1d / MED 3d / LOW 7d, `SlaPolicy::due_at`) and remains an SLA deadline, not SLO policy authority. The internal SLO timer chips, breach counts, and alert targets instead use `defaultSloSettings()` — six local category rules over elapsed time. The timer helper currently lets `due_at` override that local deadline, which is a semantic incompatibility rather than evidence of a served SLO policy. The separately persisted three-bucket ontology settings are not consumed by these timers or alerts. (`slo-settings.ts`, `SupportPage.tsx`, `SupportBody.tsx`.) |
| Palantir Foundry | [I] No SLA primitive; a due-date property + scheduled automation that fires when breached — you build it. https://www.palantir.com/docs/foundry/use-case-patterns/alerting-workflow |
| Slack | [I] No native SLA; urgency captured in the swarm form, breach handling lives in the connected Service Cloud. https://slack.com/blog/collaboration/salesforces-support-resolves-cases-faster |
| MS Teams | [I] SLA enforced in ServiceNow; Teams only relays breach notifications. https://aelumconsulting.com/blogs/servicenow-microsoft-teams-integration/ |
| Asana | [V] SLA timers preconfigured by request type/urgency; **timers start only after acknowledgement and auto-pause on "on hold"/"awaiting user info."** https://asana.com/resources/asana-request-tracking-ticketing |
| n8n | [V] SLA enforcement = a scheduled/wait workflow: tag critical → notify on-call via SMS/chat, update dashboards. https://www.wednesday.is/writing-articles/building-customer-support-automation-with-n8n |
| Rippling | N/A — no SLA/case-timer concept; IT posture is deflection, not queue-SLA. https://www.rippling.com/blog/it-service-management-software |
| SAP | [V] SLA baked into the case entity: response + resolution targets fire by priority **and customer tier**; agent countdown timers, manager compliance dashboards, escalation alerts to management/experts. https://www.spadoom.com/en/blog/discover-the-power-of-sap-service-cloud/ · https://help.sap.com/docs/CX_NG_SVC/56436b4e8fa84dc8b4408c7795a012c4/3292dfa8eaf64e219a2261860411acb4.html |

### 5. Status lifecycle (FSM)

| Vendor | How |
|---|---|
| **Our Console** | Enforced FSM `OPEN→IN_PROGRESS→ON_HOLD⇄IN_PROGRESS→RESOLVED→CLOSED`, RESOLVED can reopen to IN_PROGRESS, CLOSED terminal; every edge validated + audited (`domain/src/lib.rs:64-93`; exhaustive transition-matrix unit tests). |
| Palantir Foundry | [I] Lifecycle = Actions gated by object state; you define the allowed transitions (kinetic layer). https://www.palantir.com/docs/foundry/ontology/overview |
| Slack | N/A — no ticket state machine; status lives in the source system, thread is just conversation. |
| MS Teams | N/A — state owned by ServiceNow; Teams shows current status on the card. |
| Asana | [V] Status = task section / custom status field; no enforced transition rules (any move allowed). https://asana.com/resources/asana-request-tracking-ticketing |
| n8n | N/A — no state store; workflow branches encode transitions ad hoc. |
| Rippling | N/A — no case lifecycle. |
| SAP | [V] V2 "lifecycle case engine" — structured case stages replacing V1's flat status. https://www.spadoom.com/en/blog/sap-service-cloud-v2-whats-different/ |

### 6. Comment threads / collaboration

| Vendor | How |
|---|---|
| **Our Console** | `support_ticket_comments` thread with **internal-note vs customer-visible** flag (`is_internal_note`); internal notes never returned on the customer path; system/customer author = NULL user id. Comment triggers notification fan-out. (`0022_create_support.sql`; `SupportTicketDetail.tsx` `CommentThread`/`AddCommentForm`.) |
| Palantir Foundry | [I] Comments = a linked "note" object type or object-level comments in Workshop; not a purpose-built agent/customer split. https://www.palantir.com/docs/foundry/ontology/applications |
| Slack | [V] **This is Slack's crown jewel** — threaded swarm in a channel; engineers + senior agents collaborate in one thread; 26% faster close, 64% backlog cut. https://slack.com/blog/transformation/discover-salesforces-recipe-for-customer-success |
| MS Teams | [I] Teams thread / channel per case; adaptive cards for structured replies; strong huddle/call escalation. https://www.usepylon.com/blog/microsoft-teams-helpdesk-2025-guide |
| Asana | [V] Task comments + @mentions + proofing; internal by default, customer replies via connected email. https://asana.com/resources/asana-request-tracking-ticketing |
| n8n | N/A — no thread UI; it can post/echo comments between systems. |
| Rippling | N/A — no comment thread surface. |
| SAP | [V] Case interaction timeline (emails, notes, internal vs external) plus Joule-drafted replies. https://www.spadoom.com/en/blog/discover-the-power-of-sap-service-cloud/ |

### 7. Linked objects / ontology

| Vendor | How |
|---|---|
| **Our Console** | `SUP-` code is a **§4-20 drag source** (`objDrag`); ticket detail has an **object rail** linking to work order (`/dispatch?source=support`), messenger, mail, reporting. Ticket → work-order handoff is a first-class path. (`SupportTicketDetail.tsx` `TicketObjectRail`.) Semantic-layer registration still projected/partial (ledger §193). |
| Palantir Foundry | [V] **Source-cited** — the ticket is a node in one ontology graph; links to any object (asset, person, order) are the whole point; Actions traverse links. https://www.palantir.com/explore/platforms/foundry/ontology/ |
| Slack | [I] "Linking" = a URL back to the source ticket + Salesforce record unfurl; no object graph. https://www.salesforce.com/slack/crm/ |
| MS Teams | [I] Adaptive card deep-links to the ServiceNow record; no native graph. https://kanini.com/resources/servicenow-virtual-agent-teams-integration/ |
| Asana | [V] Task can link to other tasks/projects, custom-field references; portfolio rollups; no formal ontology. https://asana.com/resources/asana-request-tracking-ticketing |
| n8n | N/A — no object model to link. |
| Rippling | [I] Ticket context enriched from employee graph (role/dept/location) via integration. https://www.rippling.com/blog/zendesk-rippling-integration |
| SAP | [V] Case linked to account/contact/product/installed-base in the CX data model. https://help.sap.com/docs/sap-cloud-for-customer/solution-guide-for-sap-service-cloud/2b7d1a1a1cc44691a8fd585495fc1712.html |

### 8. Permissions / RBAC / SoD

| Vendor | How |
|---|---|
| **Our Console** | Deny-by-default feature-map: `AssigneeManage` (admin-only) gates triage/assign/transition; `WorkOrderStart` gates commenting (MECHANIC/ADMIN/SUPER_ADMIN); receptionist (Limited) reads but composer is hidden, not 403-on-click. RLS FORCE + branch scoping applies to the cited ticket reads. SLO settings approval is only a client-side actor inequality check; the three ontology writes are issued together from the browser but are not one backend-atomic approval. |
| Palantir Foundry | [V] Granular ontology security incl. dynamic/row-level security on objects and Actions. https://www.palantir.com/docs/foundry/ontology/overview |
| Slack | [I] Channel membership = visibility; private channels for sensitive cases; no field-level ticket ACL. https://slack.com/blog/collaboration/salesforces-support-resolves-cases-faster |
| MS Teams | [I] Team/channel roles + ServiceNow ACLs behind; approvals via adaptive cards. https://aelumconsulting.com/blogs/servicenow-microsoft-teams-integration/ |
| Asana | [V] Project-level roles/permissions + form-response privacy; no case-field SoD. https://asana.com/apps/service-desk-for-asana |
| n8n | [I] Workflow-level credentials/roles; no per-ticket authz. https://docs.n8n.io/flow-logic/error-handling/ |
| Rippling | [V] **Source-cited identity/RBAC** — policy-driven access off the employee graph is the core product. https://www.rippling.com/products/it |
| SAP | [I] Role-based case access + org/territory scoping in CX. https://www.spadoom.com/en/blog/discover-the-power-of-sap-service-cloud/ |

### 9. Automation hooks

| Vendor | How |
|---|---|
| **Our Console** | Notification fan-out on assign/status/comment — `deliver_notifications` resolves staff push tokens and fans out **FCM** (degrades gracefully if no notifier). Automation is hard-wired to these events today, not a user-authored rule builder. (`rest/src/lib.rs:460+`.) |
| Palantir Foundry | [V] Automations (scheduled/event) fire Actions/Functions on object changes — a real rules engine over the ontology. https://www.palantir.com/docs/foundry/use-case-patterns/alerting-workflow |
| Slack | [V] Workflow Builder + custom workflows drive the swarm intake/notify loop. https://slack.com/blog/collaboration/salesforces-support-resolves-cases-faster |
| MS Teams | [V] Power Automate + ServiceNow flows trigger from Teams messages/approvals. https://www.servicenow.com/community/app-engine-forum/integration-servicenow-with-microsoft-teams-to-send-approvals/m-p/3218671 |
| Asana | [V] Rules engine: triggers (form submit, field change) → actions (assign, set SLA, move section, notify). https://asana.com/resources/asana-request-tracking-ticketing |
| n8n | [V] **This is the entire product** — node-graph automation, Wait/webhook human-in-loop, Error Workflow, dedicated error handling. https://docs.n8n.io/flow-logic/error-handling/ |
| Rippling | [V] Workflow automation off HR/identity events (provision/deprovision) — powerful but IT-lifecycle, not ticket-rules. https://www.rippling.com/blog/it-service-management-software |
| SAP | [V] Automatic case classification (70-90%), sentiment, Joule suggestions, skills routing. https://www.spadoom.com/en/blog/discover-the-power-of-sap-service-cloud/ |

### 10. Mobile

| Vendor | How |
|---|---|
| **Our Console** | Native app exists (`com.console.app`) but is work-order/field focused; **support-ticket surface on mobile is a gap** (no `support` screens found under `android/`). Web console is responsive. |
| Palantir Foundry | [I] Foundry Mobile renders Workshop/ontology apps incl. object views. https://www.palantir.com/docs/foundry/ontology/applications |
| Slack | [V] Full-fidelity mobile app — swarm threads work natively on phone. https://slack.com/ |
| MS Teams | [V] Teams mobile + adaptive-card approvals/tickets on phone. https://www.usepylon.com/blog/microsoft-teams-helpdesk-2025-guide |
| Asana | [V] Native iOS/Android with forms, tasks, comments. https://asana.com/ |
| n8n | N/A — backend automation, no ticket-agent mobile UX. |
| Rippling | [V] Mobile app for employee self-service/requests. https://www.rippling.com/ |
| SAP | [V] SAP Sales/Service Cloud mobile app for agents. https://www.spadoom.com/en/blog/discover-the-power-of-sap-service-cloud/ |

### 11. Audit / compliance / PII

| Vendor | How |
|---|---|
| **Our Console** | `support_tickets` + `support_ticket_comments` are `console-gate: audited-table`; evidenced transition/assign/comment paths write audit events. Customer PII (`requester_contact`) is excluded from logging macros and audit snapshots by the cited gates. The platform audit-chain seam is partial/DARK, not proved universal or production-tamper-evident. (`0022_create_support.sql` header + migration comments.) |
| Palantir Foundry | [V] Full lineage + object edit history + dynamic security; audit is a platform property. https://www.palantir.com/docs/foundry/object-backend/overview |
| Slack | [I] Enterprise Grid audit logs + DLP + eDiscovery, but PII sits in conversation text. https://slack.com/ |
| MS Teams | [I] Purview audit/retention/DLP; ticket audit lives in ServiceNow. https://www.usepylon.com/blog/microsoft-teams-helpdesk-2025-guide |
| Asana | [I] Admin audit-log API (Enterprise); no field-level PII redaction guarantee. https://asana.com/ |
| n8n | [V] Execution logs + error workflows; you own audit trail + any PII handling. https://speedrun.ventures/blog/2026-01-30-complete-guide-n8n-workflow-monitoring-error-handling/ |
| Rippling | [I] SOC2/compliance posture; employee-data handling is core. https://www.rippling.com/products/it |
| SAP | [I] Enterprise audit + data-privacy tooling in CX/BTP. https://www.spadoom.com/en/blog/discover-the-power-of-sap-service-cloud/ |

### 12. Extensibility / no-code config

| Vendor | How |
|---|---|
| **Our Console** | **PARTIAL.** A legacy three-bucket ontology object can be listed and revised from the settings card, but it is not the serving SLO policy: six local category defaults drive timers and alerts. The taxonomies are incompatible, staging exists only in client state, and approval fans out non-atomically. Ticket categories/priorities/fields remain code enums. The approved Support-owned immutable six-category aggregate is next architecture, not current implementation. |
| Palantir Foundry | [V] **Source-cited** — define object types/props/links/Actions with no code; app built in Workshop. https://www.palantir.com/docs/foundry/action-types/overview |
| Slack | [I] Workflow Builder (no-code) + apps; ticket schema comes from the connected system. https://slack.com/ |
| MS Teams | [I] Power Platform low-code; schema from ServiceNow/Dataverse. https://aelumconsulting.com/blogs/servicenow-microsoft-teams-integration/ |
| Asana | [V] Custom fields, forms, rules, templates — all no-code; strong config. https://asana.com/apps/service-desk-for-asana |
| n8n | [V] Fully extensible via node library + custom nodes/webhooks (code-optional). https://toolshelf.tech/blog/n8n-developer-workflow-automation-2025/ |
| Rippling | [I] Configurable policies/workflows; ticketing schema not the extensibility target. https://www.rippling.com/blog/it-service-management-software |
| SAP | [V] Key-user extensibility (custom fields, rules) on the case entity; BTP for deep custom. https://www.spadoom.com/en/blog/discover-the-power-of-sap-service-cloud/ |

### 13. AI assist (triage / draft / deflection)

| Vendor | How |
|---|---|
| **Our Console** | **Deliberate N/A for this benchmark scope** — `docs/program/benchmark-brief.md` defines the console comparison as deterministic/no-AI. No auto-classify or draft-reply implementation is claimed. |
| Palantir Foundry | [V] AIP adds LLM-in-loop over the ontology, but the base object grammar is deterministic. https://build.palantir.com/ |
| Slack | [V] Agentforce Service in Slack: AI answers + case-swarm assist. https://trailhead.salesforce.com/content/learn/modules/slack-for-agentforce-it-service/maximize-your-it-team-with-ai-and-swarming |
| MS Teams | [V] ServiceNow **EmployeeWorks** (Moveworks AI) in Teams — up to 30% ticket deflection in pilots. https://windowsnews.ai/article/servicenow-unveils-employeeworks-ai-driven-help-desk-tightly-integrated-with-microsoft-teams.431820 |
| Asana | [V] AI triage: auto-name, ask for missing info, draft responses, tag. https://asana.com/resources/asana-request-tracking-ticketing |
| n8n | [V] AI classify/draft nodes with human-in-loop gates. https://blog.n8n.io/human-in-the-loop-automation/ |
| Rippling | [I] AI in ITSM roadmap; deflection primarily via automation not chat AI. https://www.rippling.com/blog/it-service-management-software |
| SAP | [V] Joule + Digital Service Agent (available since Q3 2025, GA'd in the SAP H2-2025 cycle) resolves routine cases end-to-end. https://www.spadoom.com/en/blog/discover-the-power-of-sap-service-cloud/ |

### 14. Korean B2B fit (전자결재 · 근로기준법 · group-company scoping)

| Vendor | How |
|---|---|
| **Our Console** | **PARTIAL.** Korean labels and escalation-target vocabulary (팀장/전담자/관리자) are present, and ticket reads retain branch/group scoping. 적용 승인/철회 is currently client-local UI behavior, not backend-atomic 전자결재 enforcement; it must not be treated as a completed four-eyes control. (`ko.support.*`, `supportslo-strings.ts`, `SloSettingsCard.tsx`.) |
| Palantir Foundry | [I] Localizable but no built-in Korean approval/labor semantics; you model 전자결재 yourself. |
| Slack | [I] Korean UI available; 전자결재/근로기준법 workflows are custom-built. |
| MS Teams | [I] Korean UI; approvals via Power Automate, no 근로기준법 primitives. |
| Asana | [I] Korean UI; approval chains ≈ tasks; no native 전자결재 four-eyes gate. |
| n8n | [I] Language-agnostic; build any approval chain, no local semantics. |
| Rippling | [V] Global HR/payroll but **weak Korea coverage** — 근로기준법/4대보험 not first-class vs a domestic build. https://www.rippling.com/ |
| SAP | [V] SuccessFactors + localization packs cover Korea, but heavy/expensive and generic 전자결재 vs a purpose-built 법인 model. https://www.sap.com/ |

---

## Per-vendor: "how they'd build OUR support module"

**Palantir Foundry** — Ticket becomes one Ontology object type with links to Work Order, Equipment, Employee; status transitions and SLO escalation are Actions with dynamic security; the whole desk is a Workshop app with no bespoke code. SLO breach = a scheduled automation firing an Action. The cited surface provides substantial object-graph + lineage coverage; limited on turnkey intake channels and out-of-box case ergonomics — you build everything.

**Slack** — There is no ticket screen; a case is a message in a product/region channel, swarmed in a thread, with a Workflow-Builder intake form and a swarm lead. Internal notes = the thread; customer reply = the connected Service Cloud. They'd nail collaboration + speed (26% faster close, 64% backlog cut) and mobile, but SLO/FSM/audit/PII-redaction would all be delegated to the system of record.

**Microsoft Teams** — The desk is adaptive cards inside Teams: Virtual Agent intake, inline approvals, task modules for triage, with ServiceNow (EmployeeWorks/Moveworks AI) as the engine behind. Strong for in-flow approvals and 30%-deflection AI; the case lifecycle, SLA, and audit are ServiceNow's, not Teams'.

**Asana** — A dedicated intake project: Forms → tasks in an intake section, Rules to assign + start SLA timers (that pause on hold), custom fields for priority/category, AI to auto-name/triage. Selected sampled comparator for this module's shape and a direct no-code intake + rules reference. Weaker on enforced FSM, field-level PII redaction, and audited four-eyes config.

**n8n** — Not a desk; the *connective tissue* between one. Webhook intake → classify → route → Wait-node human approval → SLA cron → escalate on breach, all as a node graph with a dedicated Error Workflow. They'd build our automation layer beautifully and leave the object store/UI to us.

**Rippling** — They'd try to make the ticket *not exist*: bind support to the employee/identity graph so access-request and onboarding tickets auto-resolve via policy (their claim: 80% IT ticket reduction). Source-cited RBAC/identity, but no case lifecycle/SLA/thread — for genuine CX complaints they'd hand off to Zendesk.

**SAP (Service Cloud V2)** — The heavyweight: a lifecycle Case engine with channel-agnostic inbox, skills-based + load-balanced routing, SLA by priority **and customer tier** with manager compliance dashboards, Joule AI + Digital Service Agent. Everything our SLO row wants and more, but enterprise-heavy, expensive, and its 전자결재/법인 model is generic vs our purpose-built one.

---

## What we'd steal (ranked)

1. **SLA timer that starts on acknowledgement and auto-pauses on "on hold"/"awaiting info"** → **Asana** is the cited reference for this capability. Our SLA `due_at` is a static create-time stamp; it must remain separate from the approved elapsed-only SLO-policy aggregate. Any pause/resume design is a later decision, not part of the approved Phase 0 architecture. **Cost: M.**
2. **SLA by customer tier, not just priority** → **SAP**. Customer-tier SLA policy is a separate contractual axis; it must not be folded into or used to claim completion of the internal Support SLO aggregate. **Cost: S–M.**
3. **Skills-based + load-balanced auto-routing** → **SAP**. We only self-assign/manual-assign. A routing Action over branch + skill tags + open-ticket count would cut triage latency. Fits our authz/branch model. **Cost: M.**
4. **Swarm thread with a designated swarm lead** → **Slack**. Our comment thread is flat agent/customer; a "pull in an expert / escalate to a swarm" affordance on high-priority tickets (reusing messenger) mirrors the 26%-faster-close pattern. **Cost: M.**
5. **Multi-form typed intake (one form per request type, pre-mapped fields)** → **Asana**. We have exactly two intake forms; per-category forms that pre-set category/priority reduce triage. Fits our typed-enum grammar. **Cost: S.**
6. **In-flow approval cards for escalation** → **Teams**. Our escalation is an audited internal note; an adaptive-card-style inline approve/reassign (already have four-eyes governance) tightens the loop without leaving the ticket. **Cost: S.**
7. **Error/retry workflow around notification fan-out** → **n8n**. Our `deliver_notifications` logs-and-drops on failure; a durable retry/dead-letter for FCM delivery is a small hardening win. **Cost: S.**
8. **Object-graph ticket links as first-class typed relations (not URL rails)** → **Foundry**. Our object rail is hard-coded hrefs; promoting SUP-↔WO-↔equipment to registered ontology link-types (already the Phase-C north star) makes the rail data-driven. **Cost: L** (rides the ontology backfill already planned).

**Do NOT steal:** AI auto-triage/draft/deflection (SAP/Teams/Asana/Slack all lean here) — violates the platform's explicit NO-AI mandate; keep it a deliberate N/A, not a gap.

---

## Cross-cutting lens findings (5 independent review lenses)

- **Task-flow:** money task = *resolve a ticket* — **~3 steps, one ticket at a time, no canned reply/macro**. Zendesk's **one-click macro** bundles a canned reply + N field changes and applies via toolbar or `/`-inline. **Steal:** saved macros (a bundled field-set + reply exposed as a reusable ontology Action) + `/`-inline apply — our ontology Action types *are* bundled writebacks. Cost **M**.
- **IA / layout:** generic ModuleScreen — list + single 22rem panel + resolve action. **GAP:** no multi-record **tabs/subtabs**, no **split-view** persistence, no **utility bar** — an agent juggling 5 tickets can't tab between them. **Steal:** Salesforce **workspace tabs + subtabs** (the biggest agent-productivity gap) [L]; utility bar (notes/recent) as a docked footer [M]; ServiceNow progressive-disclosure tabbed record [M].
- **Data-model:** Zendesk's custom-objects + typed lookups are a mature no-code extension model (more mature than our currently populated state). The existing `support_slo_setting` is a legacy three-bucket ontology schema, not the serving policy and not compatible with the six Support categories. The approved replacement shape is one immutable six-category elapsed-only Support aggregate; implementation and migration remain unclaimed. **Steal:** Zendesk custom-objects + typed lookup UX [M]; junction-object / relationship-field authoring UI [S].
- **Governance:** scoped design target — a Support-owned policy revision should be four-eyes-able as one aggregate. Current source only hides approval from the staging actor in the client and then issues three independent writes; it supplies neither a backend approval decision nor aggregate atomicity. High-risk ticket → step-up SoD before a privileged support action (impersonation/data-export) remains a ServiceNow-derived option [M].
- **Automation / extensibility:** **Steal:** on-ticket-event → workflow as a first-class lifecycle-transition trigger (not just a periodic monitor) → Zendesk [S–M]; reusable action groups (subflows/spokes) → ServiceNow [M]. An SLO-breach trigger must consume the future Support-owned policy aggregate rather than SLA `due_at` or the legacy ontology schema [M].

**Adjudication:** current self-approval prevention is client-only and current multi-rule commit is non-atomic. The approved backend-atomic six-category aggregate remains architecture only; this dossier names no migration or backend route and makes no implementation, parity, deployment, or completion claim.
