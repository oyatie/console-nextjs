# Benchmark Matrix — Module: **Automate** (Workflow Studio)

> **Benchmark evidence metadata**
> - Observation/revalidation date: 2026-07-19.
> - Sampled products/surfaces: Oyatie workflow studio source; Palantir Foundry Automate; n8n workflow builder/executions; Asana Rules; Slack Workflow Builder; Microsoft Power Automate/Teams approvals; SAP Build Process Automation; Rippling Workflows.
> - Evidence modality: Fixed-target repository source plus live-checked public official documentation/product pages and explicitly labeled public secondary pages; hands-on product tenants, screenshot capture, deployment, activation, and production validation were not performed.
> - Scope/claim ceiling: Only the named pages, surfaces, and fixed-target source are in scope; no whole-product, current-production, provider-parity, universal-superiority, legal, tax, labor, deployment, activation, or production conclusion.
> - Legend: [V] = bounded external observation with a direct URL or same-document source-list entry; [E]/[code] = fixed-target repository observation; [I] = recommendation or inference. Every steal/adopt item is [I].

Scope: triggers · conditions · actions/effects · runs · schedules · monitors · approvals-on-publish.
Columns: **Ours** + Palantir Foundry Automate · n8n · Asana Rules · Slack Workflow Builder · MS Teams (Power Automate) · SAP Build Process Automation · Rippling Workflow Studio.
Most-relevant here: **n8n** (the reference for a full trigger→node→run engine), **Asana** (trigger/condition/action rule grammar), **Slack** (no-code branching + forms), **Teams/Power Automate** (connector breadth + approvals), **SAP SBPA** (enterprise approval/decision governance). Rippling and Foundry covered as HR-native and ontology-native comparators. **[I]**

Rigor: every vendor cell is `[V]` verified (source URL) or `[I]` inferred (reasoned from known product patterns). Our column is grounded in `backend/app/src/workflow_studio.rs` (~8,657 LOC), `web/src/console/workflows/*`, and `docs/program/console-program-ledger.md`.

---

## Our console — evidence baseline (grep'd, not claimed)

- **Backend** `workflow_studio.rs`: definitions CRUD + `/publish`; **revision staging** with `/revisions/{rev}/approve|withdraw` (four-eyes, `pending_version`, `staged_by` barred from self-approve); **trigger-bindings** with `/enable`+`/disable`; **schedules** (cron) with `/preview-next-runs` + `/runs`; **simulate** run with sample context that exercises condition/branch nodes and reports the branch taken; run log with **retryable/retryCount**.
- **Connectors are internal-only** — `ALLOWED_CONNECTORS`: `internal.approvals`, `internal.notifications`, `internal.mail`, `internal.audit`, `internal.jobs` (transactional outbox). No third-party/HTTP connector surface. Publish-validation rejects graphs that reference disallowed connectors.
- **Effect = ontology Action** (ledger B1/Phase-C): an automation effect dispatches a governed ontology Action → writeback, i.e. humans and automations fire the *same* verb (Foundry's model).
- **Approval-line native**: templates carry `required_approval_line: true` (maintenance_completion_approval, purchase_payment_approval, approval.ot/leave/expense/…) — a first-class Korean 전자결재 line, not a bolt-on.
- **Frontend:** shared `AutomateBody` mounts `WorkflowAutoScreen.tsx` with workflow/schedule tabs, BlockCanvas, RunLogTimeline, and policy-projected actions. The Source-present schedule surface provides schedule list/detail, cron, run, edit/save, toggle, and delete behavior. Source presence does not prove runtime integration, browser behavior, production operation, or Cedar enrollment.
- **Governance substrate**: current server authorization plus evidenced RLS/governance checks; partial/DARK audit-chain code (production sealing OFF, no trusted external anchor yet); SoD (decider≠requester enforced in-tx). Cedar deny-by-omission on each action is the required promotion shape, not verified universal live enforcement.

---

## Capability matrix

Legend per cell: `[V]`=verified w/ source · `[I]`=inferred · `N/A`=vendor doesn't play here (reason given).

### 1. Builder / information architecture

| Ours | Foundry | n8n | Asana | Slack | Teams (PA) | SAP SBPA | Rippling |
|---|---|---|---|---|---|---|---|
| Block canvas: trigger·condition·branch·action; effect=ontology Action; ontology-first grammar. | Condition+Effect rule model on top of Ontology; not a free node graph — pick condition, pick effect. `[V]` [overview](https://www.palantir.com/docs/foundry/automate/overview) | Free-form node/edge DAG; any node can be any step; broad canvas flexibility in the cited surface. `[I]` [anatomy](https://medium.com/@Quaxel/the-anatomy-of-an-n8n-workflow-3ade4a335266) | Card stack: When / Check if / Do this — linear, task-object scoped. `[V]` [create-rule](https://help.asana.com/s/article/how-to-create-a-rule) | No-code step builder in-channel; steps + branches + forms. `[V]` [WB docs](https://docs.slack.dev/workflows/workflow-builder/) | Cloud-flow designer: trigger + sequential actions + controls; low-code. `[V]` [PA docs](https://learn.microsoft.com/en-us/power-automate/) | BPMN-style low-code flow: forms, decisions, branches, subflows. `[V]` [SBPA](https://blog.fink-its.de/en/sap-btp/automate-sap-workflow-with-sap-build-process-automation/) | Trigger + actions + conditional logic builder, HR-object scoped. `[V]` [workflows](https://www.rippling.com/platform/workflows) |

### 2. Triggers (event / schedule / data-change)

| Ours | Foundry | n8n | Asana | Slack | Teams (PA) | SAP SBPA | Rippling |
|---|---|---|---|---|---|---|---|
| trigger-bindings (enable/disable) + cron schedules; ontology object-change + time. `[code]` | Time-based, object-data (added/removed/modified/threshold), or combined. `[V]` [conditions](https://www.palantir.com/docs/foundry/automate/condition-objects) | Webhook, Cron, Email-receive, app-event, sub-workflow trigger. `[V]` [trigger docs](https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.workflowtrigger/) | ~task events (added, field change, due, moved) + scheduled. `[V]` [rule-triggers](https://help.asana.com/s/article/rule-triggers) | Emoji/message/form/schedule/webhook + time-based is-before/after/within. `[V]` [branching blog](https://slack.com/blog/news/conditional-branching-workflow-builder) | Channel msg, @mention, keyword, membership change, adaptive-card response, schedule, HTTP. `[V]` [PA for Teams](https://citizendevelopmentacademy.com/power-automate-for-teams/) | Business Event from S/4HANA, form-start, API, schedule. `[I]` [event triggers](https://community.sap.com/t5/technology-blog-posts-by-sap/business-event-triggers-in-sap-build-process-automation-for-sap-s-4hana/ba-p/13573138) | Any employee-data change (hire, dept, promo, term) + connected-app events + schedule. `[V]` [workflows](https://www.rippling.com/platform/workflows) |

### 3. Conditions & branching

| Ours | Foundry | n8n | Asana | Slack | Teams (PA) | SAP SBPA | Rippling |
|---|---|---|---|---|---|---|---|
| condition + branch blocks (≥2 outputs); simulate reports branch taken. `[code]` | Object-set + threshold conditions; function-returns-true. `[V]` [conditions](https://www.palantir.com/docs/foundry/automate/condition-objects) | If / Switch / Filter nodes on any field. `[V]` [lowcode](https://www.lowcode.agency/blog/n8n-workflows) | Check-if cards, AND/OR, nested branching. `[V]` [branching](https://help.asana.com/s/article/conditions-and-branching-in-rules) | Up to 15 conditions, nested branches, ≤10 rules/branch + fallback. `[V]` [dev blog](https://slack.dev/introducing-conditional-branching-in-workflow-builder/) | Condition/Switch controls + expressions. `[V]` [PA docs](https://learn.microsoft.com/en-us/power-automate/) | Decision tables + condition branches; auto-approve threshold rules. `[V]` [SBPA](https://blog.fink-its.de/en/sap-btp/automate-sap-workflow-with-sap-build-process-automation/) | Multi-step conditional logic on any attribute. `[V]` [Jan-25](https://www.rippling.com/blog/january-2025-product-updates) |

### 4. Actions / effects

| Ours | Foundry | n8n | Asana | Slack | Teams (PA) | SAP SBPA | Rippling |
|---|---|---|---|---|---|---|---|
| Effect = governed ontology Action (same verb as humans) → writeback + audit; internal connectors. `[code]` | Action-effect runs Action-types; batch strategies (once-for-all / per-batch). `[V]` [effect-actions](https://www.palantir.com/docs/foundry/automate/effect-actions) | 400+ integration nodes; any API call, transform, code node. `[V]` [nodes](https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.workflowtrigger/) | Multiple actions per rule (assign, set field, comment, create task, cross-app). `[V]` [features](https://asana.com/features/workflow-automation/rules) | Post msg, form, create list item, canvas, 70+ connector steps. `[V]` [WB](https://docs.slack.dev/workflows/workflow-builder/) | 1000s of connector actions; approvals, adaptive cards, HTTP. `[V]` [PA docs](https://learn.microsoft.com/en-us/power-automate/) | Actions, mail, decisions, RPA bots, S/4 API calls, subflows. `[V]` [SBPA](https://blog.fink-its.de/en/sap-btp/automate-sap-workflow-with-sap-build-process-automation/) | Cross-module: email, assign training, change app perms, update payroll, webhook. `[V]` [workflows](https://www.rippling.com/platform/workflows) |

### 5. Connectors / extensibility (external integrations)

| Ours | Foundry | n8n | Asana | Slack | Teams (PA) | SAP SBPA | Rippling |
|---|---|---|---|---|---|---|---|
| **Internal-only** (approvals/notif/mail/audit/jobs); no 3rd-party connector surface — deliberate closed governance. `[code]` | Foundry-internal (Actions/Functions/webhooks); external via Function/webhook effect. `[I]` from effect model | **The reference**: 400+ nodes + custom/community nodes + HTTP + code. `[V]` [nodes](https://cyberincomeinnovators.com/mastering-n8n-nodes-triggers-your-definitive-guide-to-powerful-workflow-automation-2025) | ~100s app integrations + developer app-components-on-rules. `[V]` [app-components](https://developers.asana.com/docs/app-components-on-rules) | 70+ connectors incl. Salesforce. `[V]` [branching](https://slack.com/blog/news/conditional-branching-workflow-builder) | 1000+ connectors + custom connectors. `[V]` [approvals-custom-connector](https://learn.microsoft.com/en-us/power-automate/teams/approvals-custom-connector) | Rich S/4 + BTP destination connectors; custom-business-object integration. `[I]` [integrate](https://community.sap.com/t5/technology-blog-posts-by-sap/integrate-sap-build-process-automation-with-custom-business-object-from-s/ba-p/13918965) | Connected 3rd-party apps as trigger/action source (SSO-governed). `[V]` [workflows](https://www.rippling.com/platform/workflows) |

### 6. Runs / execution history / observability

| Ours | Foundry | n8n | Asana | Slack | Teams (PA) | SAP SBPA | Rippling |
|---|---|---|---|---|---|---|---|
| Run log timeline (status, actor, generatedObjects, error) + schedule `/runs`; audit-chained. `[code]` | Activity log: triggered/recovered events; retained 6 months then purged. `[V]` [activity](https://www.palantir.com/docs/foundry/automate/concept-activity) | Executions log: time, status, mode, running-time, per-node data. `[V]` [error-handling](https://docs.n8n.io/flow-logic/error-handling/) | Rule activity log at project/portfolio + task activity feed. `[V]` [features](https://asana.com/features/workflow-automation/rules) | Workflow activity/analytics per workflow (built-in execution data). `[I]` from WB event model | Flow run history w/ per-action inputs/outputs; auditable. `[V]` [approvals](https://alphavima.com/blog/power-automate-teams-adaptive-card-approval/) | Process visibility / monitoring dashboards + workflow instance history. `[V]` [SBPA](https://blog.fink-its.de/en/sap-btp/automate-sap-workflow-with-sap-build-process-automation/) | Workflow run tracking in Workflow Studio. `[I]` from studio model |

### 7. Error handling & retries

| Ours | Foundry | n8n | Asana | Slack | Teams (PA) | SAP SBPA | Rippling |
|---|---|---|---|---|---|---|---|
| retryable/retryCount on run events; publish-validation fail-closed on bad graph. `[code]` | Threshold recover events; execution settings. `[V]` [execution-settings](https://www.palantir.com/docs/foundry/automate/execution-settings) | Retry-on-fail, continue-on-error, dedicated Error Trigger workflow. `[V]` [error-handling](https://docs.n8n.io/flow-logic/error-handling/) | No retry engine; rules just don't fire / log failure. `[I]` — rule model has no retry primitive | Fallback branch if no rule matches; limited failure semantics. `[V]` [dev blog](https://slack.dev/introducing-conditional-branching-in-workflow-builder/) | Configurable retry policy per action + run-error surfacing. `[I]` PA action retry settings | BPMN error boundary events + escalation/deadline handlers. `[I]` from BPMN engine | Re-routing on approver-unavailable; limited technical-retry docs. `[V]` [permissions](https://www.rippling.com/platform/permissions) |

### 8. Approvals / human-in-the-loop

| Ours | Foundry | n8n | Asana | Slack | Teams (PA) | SAP SBPA | Rippling |
|---|---|---|---|---|---|---|---|
| First-class approval-line (전자결재) templates + `internal.approvals` connector; SoD decider≠requester. `[code]` | Actions can require review, but no native multi-stage approval app. `[I]` | Human-in-loop via wait/webhook/form nodes; no native approval object. `[I]` | Approval task type + rule can request approval. `[V]` [features](https://asana.com/features/workflow-automation/rules) | Approve/Reject/Escalate buttons → branches; no formal approval ledger. `[V]` [branching blog](https://slack.com/blog/news/conditional-branching-workflow-builder) | Native Approvals app in Teams (adaptive cards, multi-approver, audit). `[V]` [native-approvals](https://learn.microsoft.com/en-us/power-automate/teams/native-approvals-in-teams) | Decisions determine approvers + approval forms; auto-approve thresholds. `[I]` [q&a](https://community.sap.com/t5/technology-q-a/sap-build-process-automation-process-triggers-a-step-by-step-demo-on/qaq-p/14171790) | Approvals app + multi-approver + vacation re-routing; some logic in Studio. `[V]` [permissions](https://www.rippling.com/platform/permissions) |

### 9. Publish governance (versioning / four-eyes / staging)

| Ours | Foundry | n8n | Asana | Slack | Teams (PA) | SAP SBPA | Rippling |
|---|---|---|---|---|---|---|---|
| **Source-backed**: revision staging + four-eyes approve/withdraw; active version serves until approved; staged_by barred. `[code]` | Ontology/pipeline versioning; automations edit-in-place (no doc'd four-eyes on the rule). `[I]` | Git-based version control is an external best-practice, not built-in. `[I]` [tips](https://medium.com/@dejanmarkovic_53716/game-changing-n8n-workflows-tips-and-tricks-for-2025-02ebf08a607c) | Rules are live-edited; no version/approval on the rule itself. `[I]` | Workflows publish live; no staged-approval of the workflow. `[I]` | Flow version history + rollback/restore; no native co-approval gate. `[V]` [version-history](https://alphavima.com/blog/power-automate-flow-version-history/) | Project versions + release/deploy lifecycle across dev→test→prod. `[I]` [subflows](https://blogs.sap.com/2023/04/05/introducing-subprocesses-as-referenced-subflows-in-sap-build-process-automation) | Live-edit; change tracked in audit, no doc'd four-eyes on the workflow def. `[I]` |

### 10. Permissions / PBAC on the automation

| Ours | Foundry | n8n | Asana | Slack | Teams (PA) | SAP SBPA | Rippling |
|---|---|---|---|---|---|---|---|
| Current legacy server authorization and evidenced RLS/governance checks protect workflow routes; Cedar deny-by-omission per `console.workflows.*` action and an effect-specific run-as policy are target requirements pending enrollment, shadow proof, and promotion. `[code+ADR-0021]` | Actions/automations run under object+property security; security-aware params. `[V]` [overview](https://www.palantir.com/docs/foundry/automate/overview) | Owner/RBAC on workflow; credentials scoped; no per-object policy. `[I]` | Project/team roles gate who can edit rules. `[I]` | Workspace/plan-gated (Business+/Enterprise+ for branching); admin controls. `[V]` [branching blog](https://slack.com/blog/news/conditional-branching-workflow-builder) | DLP + environment security roles; connector data-loss policies. `[I]` from Power Platform model | BTP role collections + S/4 authorizations on triggered actions. `[I]` | Role-based permissions is a core Rippling primitive. `[V]` [permissions](https://www.rippling.com/platform/permissions) |

### 11. Simulation / testing

| Ours | Foundry | n8n | Asana | Slack | Teams (PA) | SAP SBPA | Rippling |
|---|---|---|---|---|---|---|---|
| **Standout**: simulate run vs sample context; reports the branch actually taken (no side effects). `[code]` | Preview object-set condition matches before enabling. `[I]` | Manual "Execute workflow" test run w/ per-node data inspection. `[V]` [error-handling](https://docs.n8n.io/flow-logic/error-handling/) | No dry-run; test by triggering a real task. `[I]` | Test-run a workflow before publish. `[I]` | Flow test/checker + run-and-inspect. `[I]` from PA designer | Test/debug run in Build; process test tooling. `[I]` | No documented dry-run simulator. `[I]` |

### 12. Scheduling (cron)

| Ours | Foundry | n8n | Asana | Slack | Teams (PA) | SAP SBPA | Rippling |
|---|---|---|---|---|---|---|---|
| Cron schedules + `/preview-next-runs` (shows upcoming fire times) + per-schedule runs. `[code]` | Time-based conditions "every Monday 9AM". `[V]` [conditions](https://www.palantir.com/docs/foundry/automate/condition-objects) | Cron node w/ full expression. `[I]` [anatomy](https://medium.com/@Quaxel/the-anatomy-of-an-n8n-workflow-3ade4a335266) | Scheduled rule triggers. `[V]` [rule-triggers](https://help.asana.com/s/article/rule-triggers) | Scheduled + time-window triggers. `[V]` [branching blog](https://slack.com/blog/news/conditional-branching-workflow-builder) | Recurrence trigger (cron-like). `[V]` [PA docs](https://learn.microsoft.com/en-us/power-automate/) | Timer/deadline events + scheduled start. `[I]` BPMN timers | Precise scheduling for reminders/reports. `[V]` [Jan-25](https://www.rippling.com/blog/january-2025-product-updates) |

### 13. Mobile

| Ours | Foundry | n8n | Asana | Slack | Teams (PA) | SAP SBPA | Rippling |
|---|---|---|---|---|---|---|---|
| Approvals/notifications reach the Android field app (native push); authoring is console/desktop. `[I]` from app model | Foundry mobile consumes objects; automation authoring is web. `[I]` | Web-only authoring; no first-party mobile builder. `[I]` | Full mobile app runs rules; author on mobile limited. `[I]` | Slack mobile runs workflows + form/button steps. `[V]` [WB](https://docs.slack.dev/workflows/workflow-builder/) | Power Automate + Teams mobile approvals. `[V]` [native-approvals](https://learn.microsoft.com/en-us/power-automate/teams/native-approvals-in-teams) | SAP Mobile Start / approvals on mobile. `[I]` | Rippling mobile app for approvals/tasks. `[I]` |

### 14. Audit / compliance

| Ours | Foundry | n8n | Asana | Slack | Teams (PA) | SAP SBPA | Rippling |
|---|---|---|---|---|---|---|---|
| **Evidenced source seams**: partial/DARK seal/verify code, in-transaction SoD, and append-oriented writeback primitives; production trusted anchoring and universal run/effect coverage are not claimed. `[code]` | Full activity log (6-mo retention) + object-edit lineage. `[V]` [activity](https://www.palantir.com/docs/foundry/automate/concept-activity) | Execution log; audit is DIY / enterprise add-on. `[I]` | Activity feed + rule log; SOC2 platform. `[I]` | Adaptive-card responses auditable in run history. `[V]` [approvals](https://alphavima.com/blog/power-automate-teams-adaptive-card-approval/) | Flow run history + Purview integration. `[I]` | Process visibility + enterprise audit (SAP-grade). `[I]` | HR-grade audit on approvals/changes. `[I]` |

### 15. Korean B2B fit (전자결재 · 근로기준법 · group-company scoping)

| Ours | Foundry | n8n | Asana | Slack | Teams (PA) | SAP SBPA | Rippling |
|---|---|---|---|---|---|---|---|
| **Built for it**: approval-line templates = 전자결재; SoD; group→법인→branch scoped RBAC; leave/OT/expense approval workflows native. `[code]` + ledger | Generic engine; no 전자결재/근로기준법 semantics — you build them. `[I]` | Generic; no Korean approval/labor primitives. `[I]` | Generic PM rules; no 전자결재 line. `[I]` | Generic chat workflows; no 전자결재. `[I]` | Approvals app ≈ sequential 결재 but no 근로기준법 model. `[I]` | Strong multi-step approval + decisions; localizable but Korea-labor logic is custom. `[I]` | US-payroll-centric; no 근로기준법/4대보험 approval-line out of the box. `[I]` |

---

## Per-vendor: "how they'd build OUR Automate module"

- **n8n** — would ship our module as an open node-graph: every ontology Action, connector, and approval as a draggable node, an `If`/`Switch` for conditions, an Error-Trigger workflow for failures, and a full execution log with per-node payload inspection. Strength: unbounded composability + source-cited run debugging. It would *lose* our evidenced four-eyes publish and 전자결재 line; PBAC-per-effect is our target shape, not a current advantage. n8n has no equivalent built-in governance layer, and versioning is left to Git. Their version is feature-rich in the cited run-debugging surface and less governed in the cited workflow-definition surface.

- **Asana** — would model it as **Rules + Bundles**: When/Check-if/Do-this cards scoped to ontology objects, reusable rule bundles pushed to many "projects" (our sites/법인) for consistency, and an activity log at portfolio level. Clean, non-technical authoring; a manager builds it unaided. Weak on multi-stage approval governance, simulation, and error retries. Their version optimizes for adoption-by-non-engineers over auditability.

- **Slack** — would make it conversational: triggers on messages/forms/emoji, no-code nested branching (≤15 conditions), Approve/Reject/Escalate buttons routing to branches, all inside the comms rail. Great fit with our messenger surface. But workflows publish live (no staged approval), governance is plan-gated, and there's no ontology/writeback — effects are messages, not governed mutations. Their version is our comms-rail automation, not our system-of-record automation.

- **Teams / Power Automate** — would lean on the **native Approvals app** + 1000+ connectors + flow version-history with rollback. Selected sampled comparator combining broad connectors with a first-class approvals surface. Gaps vs us: no four-eyes co-approval on the *flow definition* itself and no ontology-Action writeback. Its DLP-based security differs from our target Cedar object-policy shape; that target is not yet universal live enforcement. Their version is enterprise-broad but governance-shallow on the automation artifact.

- **SAP SBPA** — would treat it as a governed **business process**: BPMN flow, Decision tables to pick approvers, approval forms, auto-approve thresholds (<500 EUR), Business-Event triggers from the ERP, and a dev→test→prod release lifecycle. Philosophically closest to our "config-is-a-governed-object" substrate and multi-stage approval. Heavier/slower to author, BTP-locked, and Korea-labor logic is still custom content. In this sample, that surface is enterprise-oriented and heavier to author.

- **Palantir Foundry Automate** — would build exactly our shape: Conditions over the Ontology (object added/removed/modified/threshold) + time, Effects = Action-types (humans and automations fire the same verb), batch execution strategies, 6-month activity retention. This is our north star. Our current additions include four-eyes publish and Korean workflow defaults; trusted tamper evidence remains a target pending external signing and anchoring.

- **Rippling** — would build it HR-object-native: triggers on any employee-data change, cross-module actions (payroll/perms/training), approvals with vacation re-routing, role-based permissions. Source-cited for HR-event automation and "any attribute is a trigger". Gaps: no ontology graph beyond HR, no four-eyes on the workflow def, US-payroll-centric. The cited surface covers the HR lane but not the conglomerate-wide ontology described here.

---

## What we'd steal — ranked **[I]**

1. **n8n Error-Trigger + retry/continue-on-error semantics** → cited reference: n8n. Fit: our run log already has `retryable/retryCount`; add a per-definition "error workflow" binding + explicit retry/continue policy on action nodes. Grammar fit: an error-effect is just another ontology-Action effect. **Cost: M.** [error-handling](https://docs.n8n.io/flow-logic/error-handling/) **[I]**

2. **Foundry batch execution strategies (once-for-all-objects / per-batch)** → cited reference: Foundry. Fit: our effect=ontology-Action; add a batch-strategy field so an object-set trigger doesn't fire N times. Directly matches our writeback model. **Cost: M.** [effect-actions](https://www.palantir.com/docs/foundry/automate/effect-actions) **[I]**

3. **Foundry object-set condition types (added / removed / modified / threshold-crossed)** → cited reference: Foundry. Fit: promotes our trigger-bindings from event hooks to declarative ontology-set deltas — a direct fit with our ontology-first grammar. **Cost: M.** [conditions](https://www.palantir.com/docs/foundry/automate/condition-objects) **[I]**

4. **Slack nested branching w/ fallback + labeled continuation buttons** → cited reference: Slack. Fit: our branch block supports ≥2 outputs; add a guaranteed fallback branch + button-labeled outputs for approval routing, surfaced in the comms rail. **Cost: S.** [branching](https://slack.dev/introducing-conditional-branching-in-workflow-builder/) **[I]**

5. **Asana Rule Bundles (reusable automation pushed to many scopes)** → cited reference: Asana. Fit: publish a definition once, bind it across 법인/sites as a governed bundle — bind it to our four-eyes + versioning requirements without asserting comparative safety. **Cost: M.** [features](https://asana.com/features/workflow-automation/rules) **[I]**

6. **SAP Decision tables for approver determination + auto-approve thresholds** → cited reference: SAP. Fit: replaces hardcoded approval-line templates with a data-driven decision (e.g. amount<₩500k → auto; else 팀장→본부장). Strong 전자결재/근로기준법 fit. **Cost: M/L.** [q&a](https://community.sap.com/t5/technology-q-a/sap-build-process-automation-process-triggers-a-step-by-step-demo-on/qaq-p/14171790) **[I]**

7. **Power Automate flow version-history rollback UX** → cited reference: Teams/PA. Fit: we already stage revisions; expose a full version list with diff + restore, not just the single pending revision. **Cost: S/M.** [version-history](https://alphavima.com/blog/power-automate-flow-version-history/) **[I]**

8. **Rippling "any attribute is a trigger" + connected-app event triggers** → cited reference: Rippling. Fit: generalize trigger-bindings so any ontology property change (not just curated events) can start a workflow. Watch the closed-connector stance — external triggers widen attack surface. **Cost: L.** [workflows](https://www.rippling.com/platform/workflows) **[I]**

9. **`permissioned_as ≠ created_by`** — each workflow/definition carries its own Cedar-evaluated **run-as principal** (not the author's live grants), so a departed/demoted author's automations don't become a privilege-escalation / stale-authority hole → cited reference: **Windmill**. Fit: the definition object gets a `runs_as` principal attribute, evaluated at execution. Surfaced by the governance **and** automation-ext lenses as "a real security fix, not polish" — a governance gap identified by two lenses for a document whose thesis is governed automation. **Cost: M (security fix).** [Windmill](https://www.windmill.dev/docs/core_concepts/roles_and_permissions) **[I]**

10. **Zapier-style linear "quick-automation" path** (a trigger→action form beside the block canvas for the simple case; canvas retained for branching) → cited reference: **Zapier**. Fit: task-flow + ia-layout lenses both measure our canvas at ~5+ steps for a 1-trigger-1-action rule; a linear form collapses the 80% case. **Cost: M.** [n8n vs Zapier](https://n8n.io/vs/zapier/) **[I]**

---

*Note — our deliberate non-goal:* connector breadth (n8n 400+, PA 1000+). Our `internal.*`-only connector set is a governance choice, not a gap. If external integration is ever needed, add it behind Cedar policy + audit as a governed connector type — never an open HTTP node. **[I]**

---

## Cross-cutting lens findings (5 independent review lenses)

- **Task-flow:** money task = *build a trigger→action automation*. The block canvas is correctly n8n-class for branching but pays n8n's **~5+ step** simple-case tax; Zapier collapses the simple case to a linear 2-step form. **Steal:** Zapier-style linear "quick-automation" path beside the canvas for the trigger→action 80% (added as Steal #10). Cost **M**. **[I]**
- **IA / layout:** real **canvas + run-log timeline**, plus a Source-present `AutomateBody` schedule list/detail surface with cron, run, edit/save, toggle, and delete behavior. **GAPS:** runtime integration and browser/production proof remain open; no clear trigger-library master-detail. **Steal:** trigger/action library master-detail [M] and richer run-history trace [S], without proposing to build an already-present recurring view. **[I]**
- **Data-model:** **clearest data-model win in automation** — our effect **IS a typed ontology Action** dispatched through the shared writeback shape used by human flows, with evidenced audit/fixity seams. A common live Cedar gate is still target/shadow work, so automation and human edits must not be claimed authority-identical yet. n8n passes an untyped JSON blob; it can't reference "the WO object" as a typed linked entity. **Weaker vs Temporal:** event-sourced durable execution with replay + workflow versioning; n8n's connector breadth dwarfs ours. **Steal:** Temporal-style event-sourced durable execution + replay [L]; n8n connector/integration breadth [M]; n8n Schema-view auto-inference for mapping external JSON onto typed ont props [M]. **[I]**
- **Governance:** **Par/Ahead on evidenced revision staging and four-eyes publish; unproven on universal authority parity.** Automation and human actions remain under current server guards; Cedar enrollment plus an explicit run-as principal are required before claiming no shadow-privilege bypass. **Behind on Windmill's `permissioned_as`** — if an automation runs with its author's live grants, a departed/demoted author's automations become a privilege-escalation hole. **Steal:** **`permissioned_as` — each definition carries its own Cedar-evaluated run-as principal** (added as Steal #9; a real security fix, two lenses converge) [M]; durable event-history/replay [L]. **[I]**
- **Automation / extensibility:** within the sampled surfaces, this is a **governed, internal, model-extensible** design while the cited n8n/Zapier/ServiceNow surfaces emphasize connector-rich external extension. That contrast is a bounded planning inference, not a claim about the whole market. **Steal:** inbound webhook trigger (URL mints a Cedar-scoped ingress principal → maps into an Action's params) → n8n/Zapier [M]; effect taxonomy beyond ontology-action (notification / webhook-out) → Foundry [M]; durable event-history + replay → Temporal [L]; reusable subflow / named action group → ServiceNow [S–M]. **[I]**

**Adjudicated additions & fix:** two lens-surfaced steal items were added — **Windmill `permissioned_as`** (security fix; governance + automation-ext converge) and the **Zapier quick-automation path** (task-flow + ia-layout). Evidence baseline corrected: `workflow_studio.rs` is **~8,657 LOC** (was "8,669"). **[I]**
