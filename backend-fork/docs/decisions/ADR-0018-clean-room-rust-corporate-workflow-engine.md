---
id: ADR-0018
status: accepted
doc_status: published
date: 2026-06-30
owner: jasonlee
related: [ADR-0021, ADR-0023, ADR-0028, ADR-0035]
---

# ADR-0018: Clean-room Rust corporate workflow engine benchmarked against n8n-style canvas mechanics

## Status
Accepted

## Date
2026-06-30

## Context
The product is no longer a maintenance-only workflow surface. It must become an enterprise
operations platform where managers can configure corporate workflows without developer
involvement. Required workflows include:

- 기안 / 품의 / 구매 요청 / 전자결재.
- 휴가신청, 병가, 연차 사용 촉진 공지, overtime and payroll-adjacent HR workflows.
- Asset/equipment ownership, rental, dispatch, maintenance, evidence, sale/disposal, and cost
  lifecycle workflows.
- Mail, messenger, notification, calendar, poll, support-ticket, import/export, and audit
  automations attached to source business objects.
- Future operational automation, optimization recommendations, and ERP/MES/CX workflows.

n8n Community Edition is a useful product benchmark for canvas authoring and workflow mechanics:
nodes, triggers, connections, execution modes, partial/manual test runs, queued execution, workers,
credentials/secrets, execution history, and operational observability. However n8n is not a
permissive MIT/Apache dependency. The upstream repository states that n8n is fair-code under the
Sustainable Use License and Enterprise License, so we must not copy implementation code, UI code,
types, assets, or proprietary/enterprise source into this product.

Reference anchors used for this decision:

- n8n repository and license: <https://github.com/n8n-io/n8n>, <https://github.com/n8n-io/n8n/blob/master/LICENSE.md>
- n8n README license statement: <https://github.com/n8n-io/n8n/blob/master/README.md>
- n8n workflow nodes/connections docs: <https://docs.n8n.io/build/understand-workflows/workflow-components/work-with-nodes/>, <https://docs.n8n.io/build/understand-workflows/workflow-components/connect-nodes-together/>
- n8n execution docs: <https://docs.n8n.io/build/understand-workflows/understand-executions/>, <https://docs.n8n.io/build/understand-workflows/understand-executions/types-of-executions/>
- n8n queue/concurrency docs: <https://docs.n8n.io/deploy/host-n8n/configure-n8n/scaling/enable-queue-mode/>, <https://docs.n8n.io/deploy/host-n8n/configure-n8n/scaling/control-concurrency/>

Existing local foundation:

- `workflow_definitions`, `workflow_definition_versions`, and `workflow_definition_events` already
  provide tenant-scoped versioned workflow definitions.
- `/api/v1/workflow-studio/*` already provides catalog, list, create draft, simulate, publish,
  pause, rollback, clone, history, passkey step-up, audit, and allowlist validation.
- `/settings/workflows` exists but is currently JSON/form oriented. It must evolve into a mature
  visual no-code canvas.

## Decision
Build our own clean-room, Rust-native, cloud-native corporate workflow engine. Use n8n only as a
benchmark for UX/mechanics and as public documentation inspiration, never as copied source.

The engine model is:

1. **Versioned workflow definition**
   - Draft, simulate, publish, pause, rollback, clone, retire.
   - Immutable published versions and append-only change history.
   - Every production execution binds to `(definition_id, version)`.

2. **Visual no-code canvas**
   - React canvas UI with accessible keyboard/mouse operations and enterprise-grade interaction
     polish. The primary user is a real manager/HR/payroll/finance/operations owner configuring
     live business behavior, not a developer editing JSON.
   - Node palette grouped by domain: trigger, form, approval, decision, human task, notification,
     mail, messenger, calendar, document, payroll/HR, asset/equipment, finance/procurement,
     integration, audit, and analytics recommendation.
   - Edges connect typed output ports to typed input ports. Invalid edges fail in the editor before
     publish.
   - Canvas should support zoom/pan, mini-map, branch labels, inline validation, node execution
     state, evidence badges, actor/assignee indicators, and object links.
   - The UI must be self-explanatory through layout, labels, affordances, validation, preview, and
     visual prioritization. Avoid explanatory walls of text that do not perform a workflow function.
   - Empty, loading, error, simulation, draft, publish-blocked, live, paused, failed, and rollback
     states must all be intentionally designed and browser-E2E verified.

3. **Typed node/connector contract**
   - Triggers: object event, schedule, webhook/internal event, manual test, import completion,
     mail received, ticket event, calendar/poll event.
   - Forms: 기안서, 휴가신청, 병가, 연차 사용 촉진 acknowledgement, equipment transfer, purchase request,
     payroll adjustment, generic custom object form.
   - Human tasks: approval, rejection, comment request, evidence request, delegation, escalation,
     review, sign-off.
   - System actions: create/update object, send notification, send mail, post messenger message,
     create calendar event, create poll, append audit event, generate document, enqueue job.
   - Connectors are server-owned allowlisted capabilities, not arbitrary browser-side code.

4. **Execution runtime**
   - Rust execution service evaluates workflow graphs deterministically from a typed, versioned IR.
   - Durable execution tables store runs, node attempts, input/output metadata, redaction class,
     state, retry/backoff, idempotency key, owner scope, and audit link.
   - Queue-backed worker model supports concurrency limits, retries, dead-letter review, graceful
     shutdown, observability, and horizontal scaling.
   - Manual/partial simulation uses pinned or synthetic test data and can never mutate production
     objects unless explicitly published and triggered.
   - The runtime must be crash-safe and replay-aware: every external side effect goes through an
     outbox/idempotency boundary, waiting human tasks are durable, and retries never duplicate
     approvals, mail, payroll notices, ownership changes, or audit events.
   - Execution history must preserve enough redacted metadata to debug failures and prove compliance
     without leaking HR/payroll/location/personal data into generic logs.

5. **Policy, identity, tenancy, and legal boundaries**
   - Every trigger, node, edge, action, form field, and execution read/write is evaluated under
     RBAC/PBAC/ABAC with group/org/department/team/position/custom-role scope.
   - Cedar/PBAC cutover for workflow guards is governed by
     `docs/decisions/ADR-0021-cedar-pbac-authorization-strangler.md` and
     `docs/specs/cedar-pbac-cutover.md`: Cedar may decide workflow capabilities/actions after explicit
     coexistence-map enrollment, while `console_rt`/RLS remains the row boundary and legacy guards remain live
     until promoted.
   - Sensitive actions require fresh passkey step-up: approval/signature, role/policy change,
     payroll/HR/legal, asset ownership, financial/payment, and cross-org transfer decisions.
   - HR/payroll/location/personal data is purpose-tagged, masked where appropriate, and excluded
     from generic execution logs unless explicitly allowed.
   - Korean labor-law workflows such as annual-leave usage-promotion notices are templates with
     effective dates, acknowledgement evidence, escalation, and payroll consequence tracking, not
     generic notification text.

6. **Object-centered operation**
   - Workflows run against ontology objects: person, employee, employment episode, group, org,
     department/team, asset/equipment, inventory item, site, customer, contract, purchase request,
     work order, ticket, mail thread, calendar event, payroll record, document, recommendation.
   - Workflow history becomes structured operational history for future analytics and optimization,
     but recommendations remain drafts until human/policy approval.

7. **Integrated-platform contract**
   - Workflow Builder is the orchestration layer across the platform, not a separate app. A workflow
     action must be able to read/write through the same domain APIs, policy decisions, audit events,
     notification channels, object timelines, and import/export mappings that normal screens use.
   - Messenger, mail, calendar, polls, work hub, approvals, HR/payroll, asset/equipment, inventory,
     support, procurement, ERP/MES/CX, and analytics must share source-object links and lifecycle
     semantics. No module should keep a disconnected "demo" workflow state.
   - UI navigation should converge on object pages and work queues: user sees the right action,
     context, conversation, documents, approvals, history, and next step in one coherent path.
   - Backend integrations must be via typed ports/connectors and outbox events, not direct
     cross-module table pokes or browser-only orchestration. This keeps modules independently
     testable while making the product feel native and unified.

## Alternatives Considered

### Embed or fork n8n

- Pros: mature workflow editor and execution concepts exist already.
- Cons: license is not permissive for our target product; runtime is Node.js-first and generic
  integration-oriented; data/policy/audit model would not be native to our Rust/Postgres/RLS,
  passkey, Korean HR/payroll, group/org, and ontology requirements.
- Rejected.

### Use a generic BPMN engine

- Pros: standard notation and existing process-engine ecosystem.
- Cons: too abstract for field workers/managers; weaker fit for Slack/n8n-style no-code ergonomics;
  still needs our policy, audit, tenant, HR/payroll, mail/messenger/calendar, and object model.
- Rejected as the primary UX. BPMN import/export can be future optional compatibility.

### Keep current JSON/form Workflow Studio

- Pros: already implemented and testable.
- Cons: not usable enough for real managers; cannot express complex corporate workflows safely at
  scale; will become a hidden developer tool rather than a business no-code builder.
- Rejected as the end state. It remains a bootstrap admin surface until canvas authoring is built.

## Consequences

- Workflow Studio becomes a platform foundation, not a module page.
- We need a clean workflow IR/schema that both canvas and Rust runtime understand.
- Existing workflow definition tables are a useful start; execution/runtime persistence is now
  anchored by migrations `0077_create_workflow_runtime_spine.sql` and
  `0078_harden_workflow_runtime_integrity.sql`, guarded by
  `npm run check:workflow-runtime-spine`. Remaining backend gaps are typed node schemas,
  policy-aware connector catalogs, runtime workers, and observability/replay controls.
- UI work must be measured against n8n/Slack Workflow Builder-level ergonomics: searchable node
  palette, typed edges, simulation, publish review, execution history, inline errors, and fast
  iteration.
- Backend work must be measured against production workflow-engine expectations: typed validation,
  transactionality, idempotency, durable queues, retry/dead-letter behavior, human-task waits,
  outbox side effects, policy evaluation, passkey step-up, audit trails, traces, metrics, and
  legally safe redaction.
- Code review must reject copied n8n source or assets. Only public behavior, documentation-level
  concepts, and clean-room product patterns are allowed.

## Initial Implementation Milestones

1. **Workflow IR contract**
   - Define `WorkflowGraph`, `WorkflowNode`, `WorkflowEdge`, `NodePort`, `NodeSchema`,
     `WorkflowTrigger`, `WorkflowAction`, `WorkflowForm`, `HumanTask`, `PolicyRequirement`,
     `DataSensitivity`, and `ExecutionMode`.
   - Add server validation that rejects cycles where forbidden, missing trigger, invalid edge,
     unknown connector/action, missing approval/payment line, unsafe data-class exposure, and
     missing step-up requirement.

2. **Execution persistence** — implemented foundation
   - `0077_create_workflow_runtime_spine.sql` adds `workflow_runs`, `workflow_node_runs`,
     `workflow_waiting_tasks`, `workflow_outbox_events`, and `workflow_execution_locks`.
   - Each table is tenant-scoped with RLS/FORCE, same-org foreign keys, idempotency keys,
     no-delete durability guards, and object/task/outbox fields needed to integrate approvals,
     mail, messenger, calendar, audit, HR/payroll, asset/equipment, and future ERP/MES/CX flows.
   - `0078_harden_workflow_runtime_integrity.sql` closes the run/node traceability invariant:
     waiting tasks and outbox rows may only reference node runs that belong to the same
     `(org_id, run_id)` parent run, and terminal outbox states must carry timestamp/error evidence.
   - Full sensitive payloads still belong in domain-owned tables or sealed storage when legally
     allowed; runtime snapshots must remain redacted/minimized.

3. **Canvas MVP**
   - Replace raw JSON-first authoring with a two-pane canvas: node palette + graph canvas + selected
     node inspector.
   - Keep JSON import/export as advanced mode, but make visual mapping the default.
   - E2E: manager creates a 휴가신청 approval workflow, simulates it, publishes with passkey step-up,
     employee submits a request, manager approves with comment, mail/notification/audit events land,
     and terminal items leave action inbox.

4. **Corporate workflow templates**
   - 기안/품의.
   - 전자결재 with approval line, delegation, rejection, resubmission, and document export.
   - 휴가신청 / 병가 / overtime.
   - 연차 사용 촉진 공지 with legally timed notices and acknowledgement evidence.
   - 구매 요청 / payment-line approval.
   - Asset/equipment transfer and owner/operator workflows.
   - Operational automation: ticket-to-task, mail-to-work-object, calendar/poll-linked decision.

5. **Runtime workers and observability**
   - Queue-backed Rust worker with concurrency limits, retry/dead-letter, idempotency, traces,
     metrics, execution history, and admin replay/recover controls.

## Boundary Notes for Future Expansion

- The current runtime spine is deliberately **org-local**. Group-wide workflows should be modeled as
  a parent orchestration envelope that spawns auditable child runs inside each participating org,
  rather than bypassing tenant/RLS boundaries with shared mutable rows.
- The current `trigger_type` and outbox `channel` values are a closed allowlist. Before adding broad
  external connector families, introduce a server-owned connector/action registry with typed ports,
  versioned schemas, policy requirements, secret scopes, and redaction rules. Do not turn workflow
  nodes into arbitrary browser-defined code or direct table access.
