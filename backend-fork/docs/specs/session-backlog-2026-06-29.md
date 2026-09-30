> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Session Backlog — Enterprise Operations Platform Hardening

Date: 2026-06-29

This backlog captures the current session scope and acceptance criteria. Items here are not demo/stub work: each must be implemented with tests, browser/mobile verification where applicable, audit/security review, and production rollout evidence before it can be marked done.

## P0 — Authentication, authorization, and lifecycle correctness

- [ ] **Passkey-first auth paths**
  - Desktop native passkey login/register works.
  - Desktop QR -> mobile passkey auth/register -> desktop session handoff works without requiring desktop refresh.
  - Mobile native passkey login/register works.
  - OTP-first login does not grant full access until required passkey setup is complete.
  - Account settings allows multiple passkeys and self OTP issuance where policy allows.
- [ ] **Sensitive actions require passkey confirmation**
  - Any action equivalent to signing/approval/ownership change/role grant requires fresh passkey confirmation.
  - Audit log actor must match the person who performed the action.
- [ ] **Configurable PBAC/ABAC/RBAC**
  - Policies can be configured by group, organization, department/team, position, role, custom role, and lifecycle state.
  - Users default to least privilege and usually see only their own dashboard unless assigned broader scope.

## P0 — Approval/workflow lifecycle defects

- [ ] **Slack Workflow Builder-level workflow authoring**
  - Managers with policy rights can create versioned workflow definitions without developer involvement: trigger, form, source business object, steps/actions, approval/payment line, conditional branches, notifications, messenger/mail/calendar/poll updates, evidence requirements, documents, and audit events.
  - The target is a visual no-code canvas, not a JSON form: searchable node palette, drag/connect typed nodes, typed input/output ports, branch labels, inline validation, selected-node inspector, execution preview, zoom/pan/minimap, keyboard accessibility, and production-grade error states.
  - Backend runtime persistence foundation exists in `0077_create_workflow_runtime_spine.sql`: runs, node attempts, waiting human tasks, outbox side effects, and execution locks are durable tenant-scoped objects guarded by `check:workflow-runtime-spine`.
  - Builder UX supports draft, manual simulation, partial node simulation where safe, publish, pause, rollback, clone, retire, and change-history review before the workflow affects live operations.
  - n8n Community Edition is a valid benchmark for canvas mechanics, node/connection mental model, manual/production executions, queue/worker execution, concurrency, and execution history; it is not a permissive source dependency. Follow ADR-0018 for clean-room Rust reimplementation boundaries.
  - The workflow engine must support all corporate workflow classes, not only maintenance: 기안/품의, 전자결재, 휴가신청, 병가, overtime, 연차 사용 촉진 공지, 구매/정산, HR onboarding/offboarding/transfer, payroll-adjacent review, asset/equipment ownership/operator lifecycle, operational automation, mail/messenger/calendar/poll automations, support-ticket routing, imports, and future recommendations.
  - Workflow actions are permissioned through PBAC/ABAC/RBAC and an allow-listed connector/action catalog; risky connectors/actions require admin approval.
  - Approval steps support required approvers, role/position/department/group scoped approver resolution, custom approve/reject responses, decision comments, attachments/evidence, delegation/escalation, SLA reminders, and passkey confirmation for signing-equivalent decisions.
  - Benchmark references: Slack Workflow Builder for trigger/step/branch/connector UX, Power Automate approvals for approval action semantics and attachments, SAP Build Process Automation/Task Center for enterprise inbox forms and task context.
  - Acceptance: browser E2E proves a manager builds a leave-request workflow on the canvas, simulates it, publishes with passkey step-up, an employee submits the request, the manager approves/rejects with comment/evidence, mail/notification/audit/work-hub states update, and terminal items disappear from action inbox.
- [ ] **Approved items leave approval queues immediately**
  - After manager approval, the item disappears from approval inbox/action inbox without manual refresh.
  - Closed tickets/workflows do not remain in action inbox.
- [ ] **Approval evidence is visible**
  - Photos uploaded by mechanics are viewable during manager approval.
  - Evidence view is permissioned, audited, and redacts sensitive metadata where needed.
- [ ] **Approval/payment lines are required**
  - Workflows that require an approval/payment line cannot advance without a configured line.
  - Missing line shows a blocking, actionable UI state.
- [ ] **Workflow lifecycle is object-centered**
  - Work order, planned task, approval, evidence, messages, mail, tickets, audit events, and notifications link back to the source business object.
  - Lifecycle states are explicit, searchable, auditable, and not represented as cosmetic text blocks.
  - Queue membership and badges are based on the current pending step/assignee, not only the parent object's coarse status.

## P0 — Planned work / daily-plan source-object workflow

- [ ] **계획업무 starts from an existing received work order**
  - Mechanic/manager selects the existing 접수내용 first, then states what maintenance will be performed for that equipment.
  - API requires `work_order_id` for every planned-work item and validates same tenant/branch scope before saving.
  - Manager approval shows the source reception/work order, equipment, customer/site, proposed action, decision comment, approval line, and audit history together.
  - Approved/rejected/confirmed states update task, approval, notification, and action-inbox counts immediately.
- [ ] **작업지시 상세 is the canonical operations/control surface**
  - Dispatch control is available directly from the work-order detail page, not only from the dispatch board.
  - Managers can edit intake narrative fields from the detail page through an audited, permissioned PATCH path.
  - Board/list cards should drive users into detail for complex decisions; status, assignment, approval line, evidence, messages, and audit history remain visible in one object-centered workflow.
  - Future lifecycle edits must avoid generic free-form mutation for ownership, approval, legal, accounting, or cross-org changes; those stay workflow-governed with required sign-offs.

## P0 — Notifications, alarms, and badges

- [ ] **Urgent dispatch notification delivery**
  - Setting priority/importance to 긴급 sends mechanic-facing web notification, mobile push/local notification path where available, and visible badge counts.
  - Mechanics see urgent updates on their current screen without needing full refresh.
- [ ] **Notification badges**
  - Badges show unread/actionable counts and decrement after read/ack/approval.
  - Counts are scoped by user, org, group role, and object permissions.

## P0 — Equipment/assets ownership and lifecycle

- [ ] **Group admin selected owner organization on equipment create**
  - Group admin selects the legal owner corporation during 장비등록.
  - Create writes under the selected owner org token context, not the current HQ/COSS token by accident.
  - Legal ownership sign-off is prompted before create.
- [ ] **Cross-org equipment movement requires two-party approval**
  - Moving equipment from org A to org B is not a direct field edit.
  - Sending org and receiving org both approve before ownership/intake changes.
  - Legal/accounting sign-off is captured because asset ownership is not merely a system transfer.
- [ ] **Owner vs operator model**
  - Asset may be owned by another affiliate while KNL operates rental/maintenance workflows.
  - UI can switch between group consolidated view and individual org scope.
- [ ] **Equipment search parity**
  - Search supports 호기번호, 관리번호, 차대번호/VIN, model, maker, customer, and site consistently across browse, management, dispatch, intake, maintenance, and map views.
- [ ] **Equipment lifecycle UX**
  - Registration, assignment, rental, maintenance, substitute, sale/disposal, cost ledger, and audit timeline are understandable to actual users.
  - Dispatch board/card layouts do not overflow or hide critical actions.
  - Dispatch board acts as an operational Kanban with counts, urgent/unassigned/review badges, empty states, equipment/customer/site/assignee/SLA context, and one clear next action per card.

## P0 — Live data import and data governance

- [ ] **장비Master List.xlsx production ingestion**
  - All equipment rows are registered faithfully in live production.
  - Owner org is mapped correctly; KNL operational authority is preserved where KNL operates the asset.
  - Verification includes workbook row count -> DB count -> API -> browser evidence.
- [ ] **COSS Group folder ingestion readiness**
  - Raw import ledger exists before ingesting mixed HR/payroll/attendance/billing files.
  - PII/payroll/tax/bank/sensitive fields are not exposed in normal APIs/logs.
  - Variable header rows, merged cells, attendance grids, payroll sheets, and billing sheets are classified before normalized import.
- [ ] **Standardized import/export mapping UI**
  - Import is not a simple upload button: user sees tables, maps columns to allowed entity types, validates types, and receives safe diff/preview.
  - Employee data cannot be mapped into equipment/site/machinery schemas.
  - Output uses standardized schemas.

## P0 — User/employee/group/org lifecycle

- [ ] **Group / organization / tenancy separation**
  - Platform admin manages tenants and groups separately.
  - Group admins manage all subsidiaries in their group and switch consolidated vs individual org views.
  - Users can be assigned group-scope accounts while also belonging to a specific org/tenant.
- [ ] **Employee onboarding/offboarding**
  - New employee lifecycle covers platform account, group membership, org membership, department/team, position, custom roles, device/passkey setup, onboarding tasks, and policy provisioning.
  - Termination/offboarding revokes access, preserves lawful records, handles payroll/severance data, and records audit trail.
  - Intra-group transfers preserve lawful employee history while handling resignation/new employment and severance/retirement settlement where required.
- [ ] **Korean labor/legal boundaries**
  - Personal data collection, privacy consent, cookies, labor, payroll, severance, and HR records comply with Korean legal requirements.
  - Legal requirements are documented as implementation constraints and surfaced at collection/signing points.

## P1 — Enterprise module maturity

- [ ] **Messenger/mail/calendar/poll maturity**
  - Messenger behaves like mature chat: Enter/send semantics, latest-message autofocus, unread counts, mentions, search, threads, attachments, and object links.
  - Mail supports company mail UX, payroll receipts, work mail, permissions, search, folders/labels, attachments, and audit where needed.
  - Calendar supports personal, team, department, org, and group calendars with workflow links.
  - Poll system integrates with workflow/approvals.
- [ ] **Work hub / action inbox**
  - First landing screen is a role-aware daily/weekly work dashboard, not maintenance-specific.
  - Tasks, approvals, tickets, messages, mail, and calendar actions are prioritized by role, deadline, SLA, and permission.
- [ ] **Module parity audits**
  - UI/UX/feature matrices benchmark against SAP, Palantir, Slack, Gmail/Proton, Workday, BambooHR, Gusto, Salesforce, HubSpot, Zendesk, Jira, Confluence, Power BI, NetSuite, Monday, ClickUp, ServiceNow, and relevant best-in-class tools.
  - Gaps become backlog items; invalid/weaken-product GitHub issue requests are rejected with rationale.

## P1 — Mobile apps

- [ ] **Swift and Kotlin apps**
  - Employee clock-in, passkey auth/signing, notifications/alerts, approvals, overtime requests, messenger, mail, calendar, and polls are supported.
  - Mobile and web flows share API contracts and audit semantics.

## P2 — Operations intelligence and future analytics

- [ ] **Mechanical/algorithmic foundations first**
  - Capture high-quality operational data and observability before AI/RL/ML/LLM features.
- [ ] **Optimization backlog**
  - Rental pricing, margin, maintenance windows/cycles, asset sell/keep decisions, spare equipment/parts reserve, workforce utilization, SLA probabilistic models, purchasing/bidding analytics, MES capability, and executive decision intelligence.

## Delivery discipline

Every completed item needs:
- PR -> review -> fix -> merge evidence.
- CI/Trivy/GitHub Actions/Argo rollout evidence where production code changes.
- Unit/integration tests and browser/mobile E2E simulation for each critical user story.
- Audit/security/privacy checks for PII, approvals, auth, ownership, and policy flows.
