> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Cross-Org Work Assignments and Operations Workflow Spec

Status: planning spec for the no-code org/ops editor. This is not an implementation plan and does not authorize code, schema, or production policy changes by itself.

Related contracts: `SPEC.md`, `docs/specs/knl-business-os.md`, `docs/specs/org-hierarchy.md`, `docs/specs/rbac-configurable.md`, `docs/specs/hr-core.md`, `docs/specs/hr-payroll-readiness.md`, `docs/specs/operations-intelligence.md`, `docs/specs/foundation-gates.md`, and `docs/ideas/enterprise-role-workflows.md`.

## 1. Objective

Model operational workflows where a worker whose home employment belongs to one legal entity, department,
team, site, or cell can perform scoped work for another organization, department, team, site, or cell while
preserving:

- home employment ownership and legally required HR/payroll history;
- host-side operational authority for specific work objects only;
- reporting visibility for home manager, host supervisor, HQ/group operators, HR/payroll, and auditors;
- RLS tenant isolation: every business read/write still runs under one real `Org` at a time;
- configurable policy/ruleset evaluation, preview/simulation, approval, audit, and revocation.

The central object is not a role checkbox. It is a governed `WorkAssignment` lifecycle that binds a person,
home employer, host scope, duties, ruleset context, approvals, and access grants for a bounded purpose and
time window.

## 2. Non-goals and boundaries

- Do not treat cross-org assignment as an employment transfer. A permanent legal-employer move remains an
  HR employment transition with payroll/severance/retirement handling. A `WorkAssignment` keeps the home
  employment episode intact unless an approved HR transition explicitly changes it.
- Do not weaken org isolation. `app.current_org` must always be a real legal-entity `Org`; group, site,
  cell, or assignment IDs never arm RLS.
- Do not expose home-org payroll, wage, resident-registration, bank, disability, retirement, or private HR
  facts to the host org. Host supervisors see only operational assignment facts unless explicit HR/payroll
  permission and legal basis exist.
- Do not model cross-group staffing by default. This spec assumes assignments are within one approved
  group/HQ membership graph. Cross-group assignments require a separate vendor/contract/legal workflow.
- Do not make import/upload the primary model. Imports may bootstrap people/sites later, but assignments
  are CRUD-first database objects with preview, approval, audit, and revocation.

## 3. Core concepts and objects

| Object | Purpose | Ownership / sensitivity |
| --- | --- | --- |
| `Person` | Stable human identity across employment episodes. | Identity/person domain; minimized in assignment views. |
| `Employee` | The home-org employment record. | Home org owns HR history and employment status. |
| `EmploymentEpisode` | Legal employer, hire/exit, status, payroll/severance continuity. | Home org / HR-payroll sensitive. |
| `AssignmentEnvelope` | Cross-org governance header: assignment id, home org, host org, state, requested scope, purpose, dates, and participant orgs. Contains no raw wage/PII payload. | Group/HQ governance bridge, visible only to participants through policy resolvers. |
| `AssignmentProjection` | Per-org shadow view of the envelope under each participant org's RLS boundary. | Home projection and host projection each read under their own `app.current_org`. |
| `WorkAssignment` | Activated assignment binding worker, duties, host scope, schedule window, active ruleset/policy versions, and access grant ids. | Host operational scope plus home visibility projection. |
| `AssignmentScope` | Target org, department/team, branch, worksite/site, cell, object set, duties, and allowed action set. | Never a tenant boundary by itself; maps to policy resource hierarchy. |
| `ReportingLineException` | Temporary operational supervisor, escalation path, approval delegation, and visibility rules. | Shared operational governance; no sensitive HR payload. |
| `EligibilityCheckResult` | Deterministic results for employment state, credentials, skills, site prerequisites, legal/policy constraints, and conflicts. | Stored with inputs minimized and policy/ruleset versions attached. |
| `AssignmentApprovalCase` | Approval line, comments, passkey step-up markers, evidence references, and decision history. | Workflow/approval object; append-only events. |
| `AccessGrant` | Generated scoped host-org operational permissions for the assignment window. | Host org; revocable and tied to policy version. |
| `PayrollContextBinding` | Declares payroll owner, cost allocation, premium/allowance ruleset references, and blocked legal gates without exposing wage values to unauthorized host users. | Home/payroll domain; host sees only allowed cost-center/reimbursement labels. |
| `RevocationEvent` | Suspension/revocation reason, actor, effective time, task handoff, session/access invalidation, and closure evidence. | Append-only audit; visible to participants according to policy. |
| `SimulationRun` | Preview output for access diff, approval path, inherited rules, payroll/cost flags, reporting graph, and revocation plan. | Readable only by users allowed to draft/approve the assignment. |

### 3.1 Dual-projection storage rule

A cross-org assignment needs both parties to work from their own tenant boundary. The planning model is:

1. A minimal `AssignmentEnvelope` records participant org ids, state, effective dates, and policy/ruleset
   version references. It stores no raw payroll fields, no private HR notes, and no direct work-order data.
2. A home-org `AssignmentProjection` is read under the home org and shows employment, capacity, payroll
   gate, and home-manager approval state.
3. A host-org `AssignmentProjection` is read under the host org and shows operational scope, site/cell
   prerequisites, host supervisor, work queues, and access grants.
4. Any operational work item created or updated because of the assignment is still a host-org row and is
   written under host-org RLS with audit. Any employment/payroll fact is still a home-org row and is
   written under home-org RLS with audit.
5. Group/HQ consolidated screens aggregate projections through the existing per-member armed-read pattern;
   they never perform a blanket BYPASSRLS scan.

## 4. Assignment types

| Type | Typical use | Special controls |
| --- | --- | --- |
| Temporary support | A COSS safety specialist supports a KNL site for three weeks. | End date required; host access expires automatically; home capacity impact shown. |
| Shared-services duty | HQ/payroll/finance supports multiple subsidiaries. | Sensitive data remains conjunctive: group role plus domain permission plus purpose tag. |
| Site/cell placement | A worker is assigned to a worksite cell, production line, dispatch zone, or customer site. | Site/cell rules, safety training, location consent, shift calendars, and local approvals apply. |
| Reporting exception | The worker keeps home manager but reports operationally to host supervisor for a scope/window. | Delegation is explicit; host cannot approve employment/payroll decisions unless policy allows. |
| Emergency assignment | Incident/SLA risk needs immediate host task access. | Short TTL, emergency reason, after-action review, and stricter audit; no silent permanent access. |
| Permanent transfer candidate | Trial or planned movement to another legal entity/team. | Must convert to HR employment transition if legal employer changes; assignment cannot hide settlement gates. |

## 5. State machine

`WorkAssignment` state is append-only. Updates create new events and may create a new simulation/approval
round; they do not overwrite history.

```text
DRAFT
  -> SIMULATED
  -> SUBMITTED
  -> HOME_APPROVED
  -> HOST_APPROVED
  -> POLICY_APPROVED
  -> SCHEDULED
  -> ACTIVE
  -> COMPLETION_PENDING
  -> COMPLETED
  -> CLOSED
```

Alternative and terminal paths:

```text
DRAFT -> CANCELLED
SIMULATED -> DRAFT                       # edit after failed/changed preview
SUBMITTED -> NEEDS_REVISION -> DRAFT
SUBMITTED/HOME_APPROVED/HOST_APPROVED/POLICY_APPROVED -> REJECTED
SCHEDULED/ACTIVE -> SUSPENDED -> ACTIVE  # only after re-simulation if policy changed
SCHEDULED/ACTIVE/SUSPENDED -> REVOKED
ACTIVE -> EXPIRED -> COMPLETION_PENDING
COMPLETION_PENDING -> CLOSED
```

### 5.1 State responsibilities

| State | Required evidence | Allowed next actions |
| --- | --- | --- |
| `DRAFT` | Worker, home org, requested host scope, purpose, dates, duties, requester. | Edit, simulate, cancel. |
| `SIMULATED` | Eligibility results, effective-access diff, inherited rule trace, approval path, revocation plan. | Submit, edit, cancel. |
| `SUBMITTED` | Frozen request version and simulation id. | Approve/reject/request revision. |
| `HOME_APPROVED` | Home manager/HR approval for capacity, legal employer continuity, and payroll ownership. | Host approval or rejection. |
| `HOST_APPROVED` | Host department/site/cell approval for operational need, supervisor, work queues, prerequisites. | Policy/security/payroll approval or rejection. |
| `POLICY_APPROVED` | Required policy/security/payroll approvals and passkey step-up for access-widening or sensitive gates. | Schedule or reject before activation. |
| `SCHEDULED` | Start/end timestamps, access grant plan, supervisor plan, task handoff plan. | Activate, resimulate, suspend, revoke. |
| `ACTIVE` | Host access grants live; worker appears in host work queues; audit shows policy version. | Extend, scope-change draft, suspend, complete, revoke. |
| `SUSPENDED` | Reason, actor, affected grants disabled, open work handoff plan. | Resume after checks, revoke, complete/close if no work remains. |
| `COMPLETION_PENDING` | Host tasks reconciled, evidence captured, home capacity returned, access revoke scheduled. | Close or reopen active if evidence incomplete. |
| `COMPLETED` | Outcome, supervisor signoff, task/evidence links, access revoked. | Close after audit review. |
| `CLOSED` | Immutable terminal record. | Read history/export only. |
| `REVOKED` | Revocation reason, grants/session invalidation, task reassignment, participant notification. | Close after audit review. |
| `REJECTED` | Rejection reason and policy/approval actor. | Clone to new draft only. |

## 6. CRUD-first workflow operations

The no-code editor and operational UI expose these as audited actions, not direct SQL or hidden scripts.

| Resource | Create | Read | Update | Delete / terminal action |
| --- | --- | --- | --- | --- |
| Assignment draft | `create_assignment_draft` from employee, site/cell, work item, or group work hub. | List drafts by home/host/group scope; object-page rail on worker/site/org. | Edit purpose, scope, dates, duties, supervisor, ruleset refs until submission. | Cancel draft; hard delete only before submission and only if audit policy permits. |
| Simulation run | `simulate_assignment` with request version and policy/ruleset versions. | Read trace, access diff, blockers, approval path, revocation plan. | Not edited; rerun creates a new simulation. | Expire old simulations when policies/rulesets change. |
| Approval case | Created on submit from required approver rules. | Work Hub/approval queue; source-object context and current actor visible. | Approve, reject, request revision, add evidence/comment. | Terminal decisions remain immutable. |
| Active assignment | Created by activation after approvals. | Worker roster, host work queue, home capacity view, group oversight. | Extend, reduce/expand scope, change supervisor, suspend/resume through new simulation/approval. | Complete, revoke, expire. |
| Reporting-line exception | Created with active/scheduled assignment. | Visible on worker object, manager work hub, host site/cell roster. | Change operational supervisor/escalation path with approval. | End with assignment or explicit revoke. |
| Access grant | Generated on activation, never hand-authored in SQL. | Effective-access preview and audit view. | Refresh only through policy version/assignment scope changes. | Revoke on completion, suspension, expiry, or emergency revocation. |
| Payroll context binding | Created/updated by HR/payroll approval. | Authorized HR/payroll sees details; host sees only allowed labels/flags. | Update cost center/ruleset/refund allocation through payroll approval. | Close with assignment; correction is a new event. |
| Audit/export | Created automatically for every transition. | Participant-scoped history and compliance export. | Append correction note only; no destructive edits. | Retention policy governs archival, not UI delete. |

## 7. Required eligibility checks

A simulation must evaluate all applicable checks before submission. A blocker is either hard-deny, requires
approval, or requires a draft edit.

### 7.1 Worker and employment checks

- Person is uniquely resolved; no unresolved import placeholder or name-only merge risk.
- Employee is active or otherwise eligible for assignment; pending setup, suspended, leave, terminated, or
  retired states deny by default unless a specific legal workflow permits a limited action.
- Passkey/setup and required privacy/service agreements are complete.
- Home employment episode remains intact; permanent legal-employer changes route to HR transition instead.
- Open responsibilities, leave, absence, shift, overtime, and rest-period constraints are checked.
- Required skills, certifications, medical/safety training, and customer/site credentials are current.

### 7.2 Home-org checks

- Home manager capacity approval is required when the assignment affects staffing, schedule, or reporting.
- HR approval is required when the assignment changes employment status, reporting line, workplace, labor
  basis, or legally sensitive personnel handling.
- Payroll approval is required when cost allocation, premium/allowance, worksite-specific payroll rules,
  overtime, dispatch allowance, or severance/retirement continuity could be affected.
- Home policy may set hard-deny rules such as "employees in payroll close week cannot be assigned away" or
  "union/safety-certified role cannot be loaned without HR signoff".

### 7.3 Host-org/site/cell checks

- Host org is an active member of the same approved group/HQ graph and reachable by policy.
- Host site/cell exists, is active, and accepts external/borrowed workers for the requested duty.
- Host supervisor is eligible to manage this duty and cannot approve conflicting self-owned work.
- Site/cell prerequisites are satisfied: safety induction, customer NDA, equipment license, geofence or
  location-consent gate, shift calendar, PPE/evidence requirements, and local operational quirks.
- Host work queues and object actions are compatible with the assignment scope; disabled actions must
  explain missing policy/state/evidence.

### 7.4 Policy and segregation checks

- Cedar/PBAC effective decision is allow for the requested assignment actions, scopes, and context.
- Group/HQ and org-level hard-deny or mandatory-minimum rules supersede lower site/cell attempts to relax
  them. A local site/cell may be stricter or add required evidence, but cannot override a higher hard deny.
- Separation of duties prevents the same actor from requesting, approving, supervising, and closing
  sensitive assignments when policy forbids it.
- Sensitive HR/payroll/finance/location fields remain purpose-bound and masked unless the actor has the
  domain permission, purpose tag, and passkey freshness required for that view/action.
- Policy version, ruleset version, and simulation id are frozen when the request is submitted. If a relevant
  policy changes before activation, the request returns to `SIMULATED` or `NEEDS_REVISION`.

## 8. Approval model

The approval path is generated from policy/rulesets and shown before submission.

Minimum approval roles for a normal temporary cross-org assignment:

1. requester confirmation: purpose, scope, dates, and work duties are accurate;
2. home manager: capacity, schedule, and reporting impact;
3. home HR: employment-state/legal-employer continuity when workplace or reporting line changes;
4. home payroll or finance: payroll owner, cost allocation, site/cell payroll rules, overtime/allowance risk;
5. host supervisor or site/cell owner: operational need, safety/site prerequisites, task queue fit;
6. host org admin or department manager: host resource and policy acceptance;
7. policy/security approver: access widening, sensitive data, elevated permissions, or exception handling;
8. group/HQ approver: required when the assignment crosses subsidiary boundaries, high-risk sites, or
   group-wide rules.

Passkey step-up is required for approvals that are signing-equivalent: access widening, HR/payroll legal
signoff, sensitive policy exceptions, emergency override, revocation of another manager's assignment, or
activation of a high-risk site/cell placement.

## 9. Reporting-line and visibility rules

A cross-org assignment creates a reporting exception; it does not erase the home reporting line.

| Viewer | Must see | Must not see by default |
| --- | --- | --- |
| Worker | Assigned host duties, supervisor, site/cell rules, schedule, required evidence, own access window. | Hidden policy internals, other workers' private HR/payroll facts. |
| Home manager | Worker capacity impact, assignment state, host supervisor, schedule, completion/outcome, revocation alerts. | Host confidential customer/site data beyond assignment need. |
| Host supervisor | Worker name/contact as allowed, qualifications, assigned duties, task/evidence status, safety prerequisites. | Wage, bank, resident-registration, severance, private HR notes, unrelated home-org history. |
| HR/payroll | Employment continuity, payroll/cost allocation flags, legal signoffs, sensitive gate blockers. | Host operational details outside HR/payroll purpose unless separately permitted. |
| Group/HQ admin | Participant orgs, state, major blockers, policy/ruleset trace, audit health, cross-org capacity rollup. | Raw per-org sensitive data unless group role plus domain permission allows it. |
| Auditor | Append-only transition history, approvers, policy/ruleset versions, access diffs, revocation evidence. | Payloads excluded by privacy/minimization policy. |

Delegated approvals are explicit. A host supervisor may approve daily operational task completion but cannot
approve home employment transfer, payroll change, severance/retirement decision, or home-policy exception
unless the generated policy grants that authority and the action is audited.

## 10. Ruleset and authorization interaction

### 10.1 Ruleset precedence

Rules can attach to group/HQ, org, department/team, role/position, site/cell, employee, assignment type,
object type, and workflow action. Evaluation uses a traceable precedence model:

1. legal/regulatory/system hard denies and mandatory controls;
2. group/HQ hard denies, mandatory minimums, and cross-subsidiary guardrails;
3. home-org employment/HR/payroll rules;
4. host-org operational/security rules;
5. department/team rules;
6. site/cell local rules and quirks;
7. role/position/responsibility bundles;
8. employee-specific exceptions;
9. assignment-request overrides.

Lower rules may narrow access, add approvals/evidence, set local defaults, or request an exception. They may
not relax a higher hard deny, skip a mandatory approval, remove audit/passkey requirements, or expose a
higher-sensitivity data class. If two applicable rules conflict and neither has explicit precedence, the
simulation blocks activation and routes to policy review.

### 10.2 Cedar/PBAC evaluation shape

The no-code editor should generate deterministic policy inputs. A policy decision uses:

```text
principal = person + account + home org + current org + job function + position + departments/teams
          + responsibilities + group roles + active assignment grants + passkey/setup state

resource = assignment/request/projection/access_grant/work_item/site/cell/employee/reporting_exception
         + owning org + host org + home org + group + sensitivity class + lifecycle state

action = draft/read/update/simulate/submit/approve/reject/activate/assign_task/read_task
       + suspend/resume/revoke/complete/export_audit/read_payroll_context

context = purpose + effective time + request version + policy version + ruleset version + site/cell
        + assignment type + emergency flag + device/location/shift/passkey freshness + actor relationship
```

The authorization result must include `allow/deny`, policy ids, ruleset ids, reason codes, missing approvals,
required evidence, data masking decision, and whether a user-facing action should be enabled, disabled with
reason, or hidden.

### 10.3 Runtime access behavior

- Before activation, simulation/approval reads use request/projection policy only; no host operational access
  exists yet.
- On activation, generated `AccessGrant` rows add only the host actions and object scopes named by the
  approved assignment. They do not mutate the worker's home employment or system role.
- Every runtime work action still evaluates the current policy version and assignment state. If the
  assignment is suspended, revoked, expired, or the policy version invalidates the grant, access fails closed.
- Revocation bumps the relevant policy/access version or invalidates grant ids so caches and sessions drop
  access on the next request; high-risk revocation may also force session invalidation.

## 11. Preview and simulation requirements

The editor must show a complete, non-technical preview before saving/submitting:

- assignment summary: worker, home org, host org, target department/team/site/cell, dates, duties;
- inherited-vs-local rules trace, including higher-level rules that supersede lower site/cell choices;
- effective access diff: capabilities gained/lost, object scopes, data classes, work hub queues, mobile
  routes, disabled actions, and expiry behavior;
- required approvals with current approver, delegation, passkey, and evidence requirements;
- eligibility blockers and warnings, grouped by worker, home org, host org, site/cell, payroll, policy, and
  legal/privacy domain;
- reporting graph: home manager, host supervisor, escalation path, approval delegation, and visibility;
- payroll/cost context: payroll owner, cost center/allocation labels, overtime/allowance flags, payroll
  release blockers, and masked sensitive fields;
- audit and revocation plan: what events will be written, which access grants will be created, when they
  expire, and how emergency revoke works;
- representative test cases: "Can this worker read host work order X?", "Can host supervisor approve
  overtime?", "Can home manager see completion?", "What happens after end date?".

A simulation result is stale when worker state, home/host membership, site/cell status, policy version,
ruleset version, credential state, or requested scope changes. Stale simulations cannot be submitted.

## 12. Complete lifecycle example

Scenario: COSS Group owns COSS Manufacturing and KNL Logistics. A COSS safety specialist is temporarily
assigned to KNL's Changwon customer-site cell for three weeks to supervise a forklift repair backlog.

1. **Draft.** A group operations manager opens the KNL site object and selects "Request cross-org worker".
   The form selects the COSS worker, host org KNL, target department "field maintenance", site/cell
   "Changwon hospital forklift cell", duties "safety supervision and evidence review", dates, host
   supervisor, and purpose "temporary backlog recovery".
2. **Simulation.** The system checks that COSS and KNL are active members of the same group, the worker is
   active with completed passkey/privacy setup, the worker has safety certification, the host cell allows
   borrowed workers, KNL's site rule requires daily evidence review, and COSS payroll rules require payroll
   review for cross-org overtime. The preview shows a host access diff: read assigned KNL work orders,
   comment/review evidence, approve safety checklist, no payroll read, no unrelated KNL customer export.
3. **Approval.** Required approvals are generated: COSS home manager for capacity, COSS HR/payroll for
   workplace/payroll context, KNL site owner for operational fit, KNL department manager for queue access,
   and group policy approver because the assignment crosses subsidiaries. Payroll approval records home
   payroll owner and cost allocation label only; KNL does not see wage values.
4. **Scheduling.** After approvals, the assignment is scheduled for the start date. Access grants are still
   inactive but visible in preview. The worker's home manager sees capacity reserved; KNL's host supervisor
   sees an upcoming borrowed-worker roster row.
5. **Activation.** At start time, the host access grants become active. The worker's Work Hub shows only the
   assigned KNL site tasks and required safety/evidence actions. Every KNL task write is audited under KNL's
   org. COSS HR/payroll history remains under COSS.
6. **Active operations.** If KNL tries to add unrelated customer export permission, simulation blocks it
   because the approved scope only allows assigned work-order evidence review. If the site cell adds a
   stricter PPE evidence rule, the worker sees a new required evidence item; if the rule conflicts with a
   group hard deny, the assignment returns to policy review.
7. **Extension.** KNL requests one more week. The extension creates a new draft version, re-runs simulation,
   requires home capacity and payroll review again, and preserves the original approval/audit history.
8. **Completion.** Host supervisor marks backlog support complete. The system verifies no open assigned
   tasks remain, revokes host grants, removes host queue membership, records outcome evidence, releases home
   capacity, and moves the assignment to `COMPLETED` then `CLOSED`.
9. **Revocation path.** If the worker's certification expires mid-assignment, eligibility monitoring moves
   the assignment to `SUSPENDED`, disables host grants, notifies home and host managers, and offers either
   resume after re-certification or `REVOKED` with task reassignment and audit reason.

## 13. Revocation and expiry requirements

Revocation is a first-class workflow, not a manual role cleanup.

- Automatic revocation occurs at assignment end time, employee suspension/termination, home/host org
  membership removal, host site/cell closure, certification expiry, policy version hard deny, or group
  membership revocation.
- Manual revocation can be initiated by authorized home manager, host supervisor, HR/payroll, group policy
  approver, security/admin, or emergency incident commander according to policy.
- Emergency revocation may disable grants immediately, then require after-action approval/comment within a
  defined SLA. It must not silently become a permanent hidden state.
- Revocation must: disable host access grants, remove Work Hub queue membership, reassign or pause open host
  tasks, notify worker/home/host stakeholders, preserve home HR/payroll history, record reason/evidence,
  and create an audit event with policy/ruleset versions.
- Extensions, reactivations, or scope changes after revocation create a new request/simulation/approval path;
  they do not mutate the revoked record.

## 14. Audit, observability, and history

Every state transition writes an append-only audit event in the same transaction as the state change where
possible. Audit payloads must include:

- assignment id, request version, simulation id, previous/new state;
- actor, acting org, target/home/host org ids, target site/cell ids where applicable;
- policy version, ruleset version, approval case id, passkey step-up marker when required;
- access diff summary and generated/revoked grant ids;
- eligibility blocker/reason codes, not raw sensitive values;
- purpose tag, legal/privacy/sensitivity class, evidence references, and trace id.

Metrics should count assignments by state, type, blocker class, approval latency, revocation reason,
expired-grant cleanup, and simulation deny reasons. Metrics must not include worker names, raw IDs, wages,
resident-registration numbers, bank accounts, or private HR notes as labels.

## 15. User-facing surfaces

The same workflow must be reachable from multiple object-centric entry points:

- Employee object: "Assign to another org/site" action, current assignments, reporting exceptions, access
  history, and home capacity impact.
- Site/cell object: borrowed-worker roster, request worker action, prerequisites, active assignment list,
  and local rule quirks.
- Group Work Hub: cross-org assignment requests, blockers, approvals, emergency revocations, and expiring
  grants.
- Policy Studio: reusable templates for assignment types, approval lines, eligibility checks, data masking,
  and ruleset precedence.
- Workflow Studio: versioned state machine, forms, notifications, passkey step-up, SLA timers, and audit
  routing for the assignment lifecycle.
- Simulation view: before/after access, inherited/local rule trace, approval path, reporting graph, payroll
  context, and revocation plan.

The UI should be dense and actionable, not a text wall: open requests, blockers, next approver, effective
scope, and expiry are visible in tables/cards; closed assignments move to history unless explicitly viewed.

## 16. Acceptance gates for future implementation lanes

Before a code lane starts, the synthesized plan should create separate PR lanes for:

1. governance objects and dual-projection contract;
2. simulation/eligibility engine over existing policy/ruleset inputs;
3. approval/workflow template and Work Hub integration;
4. host access grant activation/revocation contract;
5. employee/site/cell object UI actions and previews;
6. audit/observability/revocation cleanup gates;
7. browser E2E story from signup/org onboarding/passkey through assignment, CRUD work action, audit,
   revoke, and simulation.

Each lane must keep `app.current_org` real-org-only, use capability/PBAC decisions rather than role-string
shortcuts, and provide real `console_rt` tests plus browser/user-story evidence when user-facing.

## 17. Acceptance-criteria mapping for this spec

- Complete cross-org lifecycle: §5 and §12 define draft -> simulation -> approval -> activation -> active
  operations -> extension/completion/revocation.
- Required approvals: §8 lists home, host, payroll/HR, policy/security, and group/HQ approvals plus passkey
  gates.
- Revocation paths: §5, §12, and §13 define suspension, emergency revoke, automatic expiry, task handoff,
  access invalidation, and closeout.
- Policy/ruleset/authorization interaction: §7, §10, and §11 define eligibility, precedence, Cedar/PBAC
  evaluation shape, effective-access diff, and inherited-vs-local simulation.
- CRUD/state operations: §6 defines database-backed CRUD and terminal actions for drafts, simulations,
  approvals, active assignments, reporting exceptions, access grants, payroll context, and audit.
