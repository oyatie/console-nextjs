> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Cedar/PBAC Authorization for Org and Ops CRUD

Date: 2026-07-01
Status: DESIGN / TARGET STATE — planning spec for the no-code org/ops editor.
Parent: `SPEC.md`, `docs/specs/org-hierarchy.md`, `docs/specs/rbac-configurable.md`, `docs/specs/knl-business-os.md`.

## Objective

Make Cedar/PBAC the authorization model for organization and operations CRUD from the start of the
no-code editor, not a later wrapper around generated screens. Every generated object type, action,
relationship, workflow, approval, and policy template must produce a deterministic authorization contract:

1. a stable principal/resource/action/context vocabulary;
2. a Cedar entity graph and policy bundle version;
3. a server-side decision request for every CRUD/read/list/write/approve/revoke/simulate operation;
4. an audit record that explains the policy version, decision path, actor, target, purpose, and revoke impact.

This spec does not weaken the existing hard tenant boundary. Postgres RLS with `app.current_org`, `console_rt`
NOBYPASSRLS, and FORCE RLS remain the row-isolation floor. Cedar/PBAC decides whether a principal may take an
action on an already-scoped resource. It never becomes a substitute for RLS, and it never arms
`app.current_org` with anything except a real tenant Org id.

## 1. Design principles and invariants

1. **Default-deny everywhere.** Missing principal attributes, unknown resources, unknown actions, stale policy
   bundles, unsupported generated conditions, missing purpose, or unresolvable relationships return deny.
2. **RLS is the tenant isolation floor.** Cedar can allow an action only after the request is associated with a
   concrete target Org. The server still executes reads/writes through `with_org_conn` / `with_audit` under the
   target Org GUC. A Group/HQ id is never a tenant GUC.
3. **Policy is the runtime contract for generated logic.** The no-code editor may generate forms, tables,
   workflows, approval states, and object actions, but each generated action is executable only when it has a
   corresponding Cedar/PBAC action, entity schema, policy template, simulation case, and audit event mapping.
4. **No role-string authorization.** Built-in roles and custom roles are inputs to effective policy. Runtime
   decisions evaluate capabilities, relationships, assignments, object attributes, action purpose, and context,
   not `Role::Admin` / role strings at call sites.
5. **Forbid wins.** Explicit deny/forbid policies for terminated users, suspended credentials, out-of-scope
   target orgs, missing passkey step-up, stale policy version, self-approval violations, or sensitive-purpose
   mismatch override any permit.
6. **Scope precedence is explicit.** System/legal guardrails and locked Group/HQ/Org forbids are evaluated
   before lower department, worksite/cell, employee-exception, or assignment permits. Lower scopes may narrow
   access or add requirements, but they cannot weaken higher mandatory policy. Contradictory generated bundles
   fail simulation/activation instead of relying on UI convention.
7. **Policy versions are first-class.** Every active policy bundle has a monotonically increasing
   `policy_version` and a content digest. Role, template, ruleset, group grant, delegation, assignment, and
   revoke writes synchronously bump the appropriate version before returning.
8. **CRUD uses the audited console API.** Generated database tables and operations are not a backdoor. All
   business mutations flow through server handlers that authorize, mutate, and write audit in the same
   transaction.
9. **Simulation uses the same evaluator.** Preview/dry-run/simulation calls pass hypothetical entities and
   draft policy overlays into the same PDP path used by runtime decisions. They may not use simplified UI-only
   checks.
10. **Cross-organization access is explicit and revocable.** A worker may belong to one Org while receiving a
   bounded grant against another Org/group/cell, but that grant is represented as an entity relationship with
   purpose, scope, expiry, approver, and revoke/audit semantics.
11. **Generated policy remains reviewable.** The editor stores source templates, generated Cedar policies,
    generated entity-schema diffs, sample allow/deny cases, and activation approvals so reviewers can prove
    what changed before activation.

## 2. Cedar/PBAC vocabulary

Use a single Cedar namespace such as `Maintenance` for org/ops policy. The exact crate/module names can land
later, but the vocabulary below is the target interface between generated no-code metadata and runtime authz.

### 2.1 Principals

| Principal type | Entity id | Required attributes / relationships | Notes |
| --- | --- | --- | --- |
| `User` | `User::<uuid>` | `home_org`, active credential state, employment state, system roles, custom role assignments, department/team, position/level, branch/site/worksite scopes, group grants, current passkey step-up age, delegated responsibilities | Normal employee/admin/operator principal. |
| `ServiceAccount` | `ServiceAccount::<uuid>` | owning Org/platform tier, allowed machine actions, key status, rotation metadata | For background jobs/codegen only; must not inherit broad human roles. |
| `PlatformOperator` | `PlatformOperator::<uuid>` | platform support role, view-as flag, support ticket/purpose, target Org context | Vendor tier stays distinct from tenant-tier group admins. |
| `ExternalApprover` | `ExternalApprover::<uuid>` | limited approval link, purpose, expiry, allowed action/resource ids | Optional future pattern for accountant/labor/legal sign-off; default disabled. |

Required principal relationship entities:

- `Employment(user, employee)` — status: `pending_setup`, `active`, `transferred`, `on_leave`, `suspended`,
  `terminated`, `retired`, `rehired`.
- `OrgMembership(user, org)` — membership status, job function, position level, department/team, branch/site
  reach, effective dates.
- `GroupGrant(user, group)` — `GROUP_VIEWER`, `GROUP_ADMIN`, `GROUP_FINANCE`, or future group roles; does not
  confer tenant capabilities by itself.
- `WorkAssignment(user, resource_or_scope)` — explicit duty for site owner, equipment owner, line supervisor,
  payroll processor, purchase approver, safety reviewer, cross-org support worker, etc.
- `Delegation(user, delegated_by, resource_or_scope)` — bounded temporary authority with expiry, purpose,
  source approval, and revoke pointer.
- `ApprovalRole(user, approval_queue_or_resource)` — generated workflow approver authority with threshold,
  segregation-of-duties, passkey, expiry, and approval-policy lineage.
- `SharedServiceGrant(user, group, target_org_or_scope)` — HQ/shared-services reach for HR, payroll, finance,
  operations, or audit duties; it is always purpose-bound, target-Org-armed, and revocable.

### 2.2 Resources

Every editor-created resource must include `org_id` unless it is deliberately topology-only or platform-tier.
The resource entity is the object being protected, not merely the UI route.

| Resource type | Examples | Required attributes / relationships |
| --- | --- | --- |
| `Group` | HQ/group/conglomerate container | `status`, member Orgs, group policy version, topology only; not an RLS tenant. |
| `Org` | legal corporation/subsidiary | `group`, `status`, legal boundary, policy version, tenant id used for RLS. |
| `Department` / `Team` | HR, payroll, operations, finance, dispatch | parent Org/department, manager, sensitivity, active status. |
| `Employee` / `Person` | worker/personnel record | home Org, employment state, department/team, position, worksite, sensitivity classes, payroll/PII flags. |
| `Role` | built-in or custom job-function role | Org, `system`/`custom`, status, feature/action grants, conditions, assignability. |
| `ReportingLine` | manager/subordinate edge | Org, manager, subordinate, effective period, approval state. |
| `WorksiteCell` | branch/site/cell/사업장 | Org, branch/region, local policy/ruleset pointer, payroll/operation quirks, status. |
| `PolicyTemplate` | reusable policy pattern | owner Org/group/platform, source editor primitive, supported actions, generated Cedar policy ids. |
| `RuleSet` | activated policy/rules version | Org or group, version, status, effective time, compiled bundle digest, rollback pointer. |
| `ApprovalRequest` | approval workflow item | requested action/resource, requester, approvers, current state, threshold, segregation-of-duties markers. |
| `Assignment` | role/scope/work/duty assignment | target user, resource/scope, assigned_by, effective period, purpose, status. |
| `AuditRecord` | append-only decision/mutation evidence | Org, actor, action, resource, policy version, decision, before/after digest, retention class. |
| `SimulationCase` | preview/dry-run/test fixture | draft policy/template, hypothetical principal/resource/context, expected allow/deny, activation gate. |

### 2.3 Actions

Actions are canonical and generated from editor primitives. They should use stable names like
`employee.read`, not route names. CRUD names should be consistent across resource classes so generated UI can
ask for decisions without custom code branches.

| Action family | Required actions | Notes |
| --- | --- | --- |
| CRUD | `create`, `read`, `list`, `update`, `archive` | `delete` is not the default business operation; use `archive`/`retire` unless legal retention allows deletion. |
| Assignment | `assign`, `unassign`, `transfer`, `delegate`, `accept_assignment` | Covers roles, responsibilities, worksites, approvals, and cross-org worker grants. |
| Policy lifecycle | `draft`, `simulate`, `preview`, `approve`, `activate`, `rollback`, `retire` | All policy/template/ruleset changes require preview and audit before activation. |
| Approval workflow | `request_approval`, `approve`, `reject`, `cancel`, `escalate`, `override` | Segregation of duties and self-approval checks are PBAC conditions, not UI-only rules. |
| Revoke | `revoke`, `revoke_sessions`, `revoke_assignments`, `revoke_policy_version` | Revoke is its own action family because it has cache/session/audit side effects. |
| Audit | `audit.read`, `audit.export`, `audit.annotate`, `audit.seal` | Audit records are append-only; modification is not a normal action. |
| Simulation | `simulation.run`, `simulation.compare`, `simulation.export` | Must use the same evaluator path with hypothetical overlays. |
| Context switch | `context.switch`, `org.switch_context`, `group.consolidated_read` | Switching or fanning out across member Orgs is an audited action; it never arms RLS with a Group id. |

Generated concrete actions should combine resource and family, for example:

- `org.create`, `org.read`, `org.update`, `org.archive`;
- `department.create`, `department.update`, `department.archive`;
- `employee.read`, `employee.update`, `employee.transfer`, `employee.archive`;
- `role.create`, `role.assign`, `role.revoke`, `role.retire`;
- `reporting_line.create`, `reporting_line.update`, `reporting_line.archive`;
- `worksite_cell.create`, `worksite_cell.update`, `worksite_cell.assign_policy`, `worksite_cell.archive`;
- `policy_template.preview`, `ruleset.activate`, `approval_request.approve`, `audit_record.read`.

### 2.4 Context

Every runtime request must supply bounded context. Context is not optional metadata; it is part of the policy
decision and the audit trail.

| Context field | Purpose |
| --- | --- |
| `request_id`, `trace_id` | Correlate decision, mutation, audit, logs, and UI preview. |
| `current_org`, `target_org` | Real Org ids. `current_org` is the GUC tenant; `target_org` must match for normal tenant requests. |
| `group_scope` | Optional group id plus resolved member org list for consolidated/HQ flows; never an RLS GUC. |
| `policy_version`, `bundle_digest` | Proves which active policy decided. Stale/missing means deny. |
| `purpose` | HR admin, payroll processing, assignment handoff, emergency support, audit review, simulation, etc. |
| `action_intent` | Human-readable reason or workflow transition requested by the user. |
| `before`, `after`, `diff_summary` | Write decisions must authorize the actual field/resource changes, not just the route. |
| `sensitivity` | Payroll, wage, location, PII, finance, safety, legal, audit-only, public. |
| `branch_scope`, `worksite_scope`, `department_scope` | Intra-org scope projections resolved server-side from live assignments. |
| `scope_precedence_trace` | Ordered source of the effective policy: system/legal guardrails, group, org, department/team, worksite/cell, role/responsibility, assignment, exception, and workflow context. |
| `passkey_step_up_age_seconds` | Sensitive policy/role/revoke/approval actions require fresh step-up. |
| `time`, `device_posture`, `ip_class`, `location_consent_state` | Only for policies that explicitly use them; missing source-of-truth attributes fail closed. |
| `simulation_mode` | `false` for runtime; `true` for preview/dry-run with hypothetical overlays and no mutation. |

## 3. Editor primitive to Cedar/PBAC mapping

| Editor primitive | Cedar/PBAC resource | CRUD/actions | Policy inputs | Audit/revoke implications |
| --- | --- | --- | --- | --- |
| Org / subsidiary | `Org` | `org.create/read/update/archive`, `org.switch_context` | platform/operator authority, group membership, target Org status, legal boundary, current support purpose | Org creation/archival is platform/group-governed, audited with target Org. Archival revokes active grants and blocks new writes. |
| Department/team | `Department` / `Team` | `department.create/read/update/archive`, `department.assign_manager` | actor has Org/department admin reach, parent department active, no cycle in hierarchy, manager relationship | Manager changes recalculate descendant scopes and bump org policy/relationship version. |
| Employee/person | `Employee` | `employee.create/read/update/archive`, `employee.transfer`, `employee.suspend`, `employee.rehire` | employment state, target department/worksite, sensitive fields, HR/payroll purpose, branch/site reach, passkey for sensitive changes | No hard delete for retained labor/payroll history. Termination/suspension revokes sessions, assignments, delegations, and active duties. |
| Role/custom role | `Role` | `role.create/read/update/retire/assign/unassign/revoke` | `RoleManage`, `ElevatedRoleGrant`, grant≤self over effective set, no-lockout floor, policy version, passkey freshness | Every role write bumps `policy_version`; revoke invalidates cache and records affected users/resources. |
| Reporting line/org chart edge | `ReportingLine` | `reporting_line.create/update/archive`, `employee.set_manager` | manager/subordinate active, same Org or explicit cross-org delegation, no cycles, department reach, effective date | Changes affect manager-derived visibility and approval routing; write audit includes old/new manager and downstream impact preview. |
| Worksite/cell/사업장 | `WorksiteCell` | `worksite_cell.create/read/update/archive`, `worksite_cell.assign_policy`, `worksite_cell.assign_worker` | branch/site authority, cell status, local payroll/operation quirks, safety sensitivity, assigned owner | Cell policy/ruleset changes bump policy version for the Org/cell; archives require open work/assignment handoff. |
| Policy template | `PolicyTemplate` | `policy_template.create/read/update/preview/approve/retire` | template owner, supported resource/actions, generated Cedar policy static checks, reviewer approval | Templates are versioned source artifacts; activation produces bundle digest and audit links to source template. |
| Ruleset/policy version | `RuleSet` | `ruleset.draft/simulate/activate/rollback/retire` | draft diff, simulation pass/fail, approvers, effective time, rollback target, conflict checks | Activation/rollback bumps policy version, signs bundle, writes activation audit, and invalidates PDP cache. |
| Approval workflow | `ApprovalRequest` | `approval_request.create/read/approve/reject/escalate/cancel/override` | requester, approver set, threshold, segregation-of-duties, self-approval rule, passkey, object sensitivity | Decision audit records approver, policy reason, evidence, and next state. Rejected/cancelled approvals cannot be reused. |
| Assignment/delegation | `Assignment` / `Delegation` | `assignment.create/read/update/revoke/transfer`, `delegation.accept/revoke` | assignee active, assigner has grant≤self, scope/resource exists, purpose, expiry, target Org membership | Revoke is immediate for future decisions; affected sessions are refreshed or killed based on sensitivity. |
| Audit record | `AuditRecord` | `audit_record.read/export/annotate/seal` | `AuditLogRead`, same Org or explicit group-finance/audit role, purpose, retention class, sensitivity | Append-only. Annotations are new records linked to the original; no update/delete except lawful retention job. |
| Revoke operation | `RevokeRequest` | `revoke.execute/preview/rollback_if_safe` | actor has authority over target assignment/session/policy, impact preview completed, passkey for sensitive revokes | Must bump version or session epoch before returning. Audit includes affected users/resources and cache/session invalidation outcome. |
| Simulation/preview | `SimulationCase` | `simulation.run/compare/export`, `policy.preview`, `assignment.preview` | draft policy overlay, hypothetical principal/resource/context, expected allow/deny cases, no mutation | Simulation results are audit/governance artifacts but do not grant authority; activation requires passing cases. |

## 4. Generated no-code policy model

The no-code org/ops editor should treat authorization as a generated contract with four artifacts per object
or workflow primitive:

1. **Entity schema extension.** New object/resource types declare attributes used by policy: owner Org,
   parent scope, sensitivity, lifecycle status, assignment relationships, and effective dates.
2. **Action registry row.** Each generated CRUD or workflow action has a stable key, display label, required
   resource type, context requirements, risk class, and audit event name.
3. **Policy template instantiation.** Editor choices such as "department manager may edit subordinate worksite
   assignments" or "HQ HR can read employees across subsidiaries" instantiate reviewed templates into Cedar
   policies, not hand-written route checks.
4. **Simulation cases.** Every generated rule ships with allow/deny examples. Activation is blocked when the
   examples do not evaluate as expected or when unsupported attributes would make runtime fail closed.

Generated bundles must include:

- Cedar schema for principal/resource/action/entity relationships;
- Cedar policies grouped by source template and editor version;
- a `policy_version`, bundle digest, and signer;
- migration compatibility notes for existing `Feature`/`policy_roles` grants;
- scope-precedence and conflict metadata proving lower-scope rules do not weaken locked group/org/system
  guardrails;
- a rollback pointer to the last active bundle;
- a decision-reason catalog safe for audit/UI display.

### Relationship to the current `Feature` / custom-role substrate

The existing system-role matrix and `feature_catalog` remain the bootstrap floor. The Cedar/PBAC layer should
adapt them, not fork them:

- `Feature` keys become coarse action/capability families in Cedar, especially for existing operational
  endpoints such as `employee_directory_read`, `employee_directory_manage`, `role_manage`, and
  `org_wide_queue_triage`.
- Current `policy_roles`, `policy_role_permissions`, `policy_role_conditions`, `user_role_assignments`, and
  `policy_versions` become input entities for the Cedar resolver until a richer ontology-action catalog lands.
- Unsupported ABAC/PBAC condition rows continue to fail closed until their source-of-truth attributes exist.
- New no-code actions are introduced through a reviewed action registry and policy template, not by allowing
  arbitrary tenant-defined capabilities in the hot path.

## 5. Runtime evaluation flow

### 5.1 Single-object read/write

1. Resolve the authenticated principal from token + live DB attributes under the relevant Org: system roles,
   custom role assignments, group grants, active employment/membership state, branch/worksite/department reach,
   delegations, and policy version.
2. Load the target resource entity from an RLS-armed read when it already exists. For creates, build a proposed
   resource entity from the validated request body and parent scope.
3. Build `AuthorizeRequest { principal, action, resource, context, entities }` with current Org, target Org,
   purpose, policy version, before/after diff, sensitivity, passkey state, and request id.
4. Evaluate Cedar/PBAC in-process or through a tightly controlled PDP adapter. Any adapter failure is deny.
5. If allow, perform the mutation through `with_audit` / `with_org_conn` in the same tenant transaction.
6. Write `audit_events` with the decision result, policy version, reason codes, actor, target resource, before
   and after digest, and revoke/cache side effects.

### 5.2 List/search

List endpoints must not become global scans with per-row UI hiding.

- First constrain by RLS tenant and existing branch/worksite/department filters derived from the principal.
- For generated object types with additional policy filters, compile a safe repository predicate only from
  reviewed, finite relationship/scope attributes. If a rule cannot compile to a safe predicate, return an
  explicit unsupported/denied result rather than listing too much.
- For high-sensitivity fields, list may return object shells while requiring per-object `read_sensitive` or
  field-level actions before returning payroll, wage, location, resident-id, finance, or legal details.
- Every list decision still records aggregate audit metadata: action, filters, result count bucket, purpose,
  policy version, and denied-sensitive-field count. Do not log raw PII in metric labels or audit summaries.

### 5.3 Simulation and preview

Simulation is an authorization operation in its own right:

1. The editor builds a draft bundle overlay plus hypothetical principal/resource/context cases.
2. The PDP evaluates the overlay with `simulation_mode=true`; no runtime grant/session/cache state is changed.
3. Results show allow/deny, missing attributes, forbids, stale references, affected users/resources, and sample
   before/after decisions, including any lower-scope override that contradicts locked group/org/system policy.
4. Activation requires passing simulation cases, fresh passkey step-up for sensitive policy changes, approval
   where configured, and an activation audit event.

## 6. Cross-organization workers and HQ/group management

The target customer model has HQ/group administrators, multiple legal corporations, site/worksite cells, and
workers who may do cross-organization work. The representation must keep each layer explicit.

### 6.1 Group/HQ management

- `Group` is topology and authorization metadata, not a tenant. It may group member Orgs, but it does not own
  tenant business rows.
- `GroupGrant` gives a principal group-level actions such as consolidated read, member management, context
  switch, or group finance read. It never directly grants tenant `Feature`/object actions.
- Consolidated reads fan out over authorized member Orgs and run N ordinary RLS-armed reads. Cedar evaluates
  group reach and each member Org action; Postgres RLS still confines every member read.
- Cross-entity writes target one concrete Org at a time. The handler proves the target Org is an active member
  reachable by the principal, arms that Org, authorizes the concrete action/resource, mutates, and audits with
  real actor + group grant + target Org.

### 6.2 Cross-organization worker access

A worker with home Org A can act in Org B only through explicit entities such as:

- `WorkAssignment(user, worksite_or_resource_in_org_b)` for a bounded operational duty;
- `Delegation(user, delegated_by, target_scope_in_org_b)` for temporary authority;
- `ApprovalRole(user, approval_queue_in_org_b)` for a defined approval path;
- `SharedServiceGrant(user, group, org_b, action_family)` for HQ/shared services such as HR/payroll/finance.

Each grant carries:

- target Org/resource/scope;
- allowed action families;
- purpose and sensitivity ceiling;
- effective/expiry timestamps;
- assigning actor and approval request;
- revoke id / policy version lineage;
- optional passkey or location/device constraints.

Runtime behavior:

1. The worker chooses or is routed to a target Org/resource.
2. The server resolves the cross-org grant live and checks it has not expired/revoked.
3. The request arms the target Org's RLS GUC, not the home Org and never the Group id.
4. Cedar evaluates the action against the target resource, the worker's grant, purpose, sensitivity, and
   current employment/credential state.
5. Audit records both home Org and target Org context, but the business mutation audit row belongs to the
   target Org so tenant evidence is complete.

## 7. Policy examples

Illustrative only; final syntax should be generated from reviewed templates.

```cedar
// Department manager can read active employees in their managed department,
// but not sensitive payroll fields unless a separate payroll-purpose policy permits it.
permit(
  principal is Maintenance::User,
  action == Maintenance::Action::"employee.read",
  resource is Maintenance::Employee
)
when {
  principal.employment_status == "active" &&
  resource.org == context.current_org &&
  resource.department in principal.managed_departments &&
  !("payroll" in resource.sensitivity)
};
```

```cedar
// A group HR shared-service worker may update an employee in a member Org only through
// an active cross-org assignment and only while the request arms that target Org.
permit(
  principal is Maintenance::User,
  action == Maintenance::Action::"employee.update",
  resource is Maintenance::Employee
)
when {
  context.current_org == resource.org &&
  context.purpose == "hr_shared_service" &&
  resource.org in principal.group_member_orgs &&
  resource.org in principal.cross_org_hr_assignment_orgs &&
  principal.passkey_step_up_age_seconds <= 300
};
```

```cedar
// Forbid wins: terminated/suspended users and stale bundles cannot mutate anything.
forbid(principal, action, resource)
when {
  principal.employment_status in ["terminated", "retired", "suspended"] ||
  context.policy_version != principal.resolved_policy_version
};
```

## 8. Audit, revoke, and cache implications

### 8.1 Decision audit

Every authorizer call should be auditable even when the business operation is denied. The durable audit shape
should include:

- `request_id`, actor user/service id, acting Org/home Org, target Org, group id if any;
- action key, resource type/id, resource sensitivity, purpose;
- decision: allow/deny/forbid/error;
- reason codes safe for UI/support display;
- active `policy_version`, Cedar bundle digest, template/ruleset ids;
- before/after digest for writes and policy changes;
- passkey step-up evidence id for sensitive changes;
- cache/version/revoke side effect result;
- source UI/API route and object editor primitive id.

Denied high-volume list checks may be sampled or bucketed for observability, but sensitive mutation, policy,
role, revoke, approval, HR/payroll, finance, location, and audit-record reads should write explicit decision
evidence.

### 8.2 Revoke semantics

Revoke must be modeled as a first-class workflow, not a row delete.

- Role assignment revoke sets assignment status/effective end, bumps Org `policy_version`, invalidates PDP
  cache, and forces token/session refresh when the revoked grant was present in claims or feature grants.
- Cross-org grant revoke bumps the target relationship/policy version and prevents the next request even if
  another app node has a warm cache.
- Policy/ruleset rollback activates a previous signed bundle, increments version, records affected actions and
  users, and keeps the failed bundle as retained audit evidence.
- Employment termination/suspension revokes sessions, assignments, delegations, approval queue membership, and
  active work duties, while retaining required HR/payroll/audit records.
- Audit records are not revoked; access to them can be revoked, and retention/legal hold jobs can seal/archive
  them according to policy.

### 8.3 Cache safety

- Cache key: `(org_id, policy_version, bundle_digest)` for tenant policy; include group policy version when a
  group/HQ decision participates.
- A cache miss under load is deny or re-load under an RLS-armed/request-safe path; never most-permissive.
- All policy/role/assignment/revoke writes synchronously bump the relevant version before returning.
- Deployment may ship a new PDP adapter/bundle format, but runtime should reject incompatible bundle versions
  rather than falling back to legacy role checks.

## 9. Delivery plan

1. **P0 — Vocabulary and registry.** Create the action/resource/context registry for existing org, HR,
   policy, assignment, audit, and group objects. Map current `Feature` rows to coarse Cedar actions.
2. **P1 — PDP adapter behind current authz.** Add a Cedar/PBAC evaluator seam that can evaluate the bootstrap
   bundle generated from the current system-role matrix and custom-role data without changing behavior.
3. **P2 — Policy bundle generation.** Generate Cedar schema/policy/simulation artifacts from Policy Studio and
   no-code editor templates; require static validation and simulation before activation.
4. **P3 — CRUD enforcement for org/ops editor.** Route generated org/department/employee/reporting-line/
   worksite/policy/approval/assignment endpoints through Cedar/PBAC authorizer plus existing RLS/audit.
5. **P4 — Cross-org/HQ flows.** Add group/HQ and cross-org worker grants as explicit entities; preserve N
   RLS-armed member reads and one-target-Org writes.
6. **P5 — Revoke/session/cache hardening.** Prove sub-minute or next-request revoke semantics, policy-version
   invalidation, denied stale bundle behavior, and session refresh/kill paths.
7. **P6 — Browser/user-story evidence.** Demonstrate sign-up, org onboarding, passkey setup, org/ops CRUD,
   policy preview/simulation, cross-org worker grant/revoke, audit review, and denied unauthorized edits in the
   console.

## 10. Verification requirements

For implementation slices derived from this spec:

- Unit tests: Cedar action/resource registry rejects unknown actions/resources; policy bundle static checks
  fail on missing context or unsupported attributes.
- `console_rt` integration tests: create/read/update/archive round-trips for org-scoped resources; cross-tenant
  invisibility; fail-closed unarmed reads; revoke takes effect on next request.
- Authz tests: allow/deny matrix for each editor primitive in §3, including manager/subordinate, worksite cell,
  group admin, cross-org worker, terminated/suspended user, missing passkey, stale policy version, and
  self-approval violations.
- Audit tests: allowed and denied sensitive operations write bounded decision evidence with policy version and
  no raw PII in labels/summaries.
- Simulation tests: draft policy overlay produces the same decisions as runtime after activation; unsupported
  conditions fail closed before activation.
- Browser E2E: Policy Studio/no-code editor preview shows decision path and revoke impact; actual CRUD actions
  match the preview; unauthorized users see a clear denial and no data leak.

## 11. Open decisions

1. **PDP location:** recommended default is in-process Cedar evaluator in `platform/authz` or a sibling
   `console-authz-cedar` crate. A remote PDP is deferred until there is a real operational need and an outage
   strategy.
2. **Field-level policy:** recommended default is resource/action plus sensitivity families first; field-level
   masking for payroll/PII/location can follow once source-of-truth classifications are complete.
3. **Tenant-defined action primitives:** recommended default is reviewed/generated action registry entries, not
   arbitrary tenant-created actions in runtime policy.
4. **Group policy versioning:** recommended default is separate group topology/grant version plus member Org
   policy versions in the cache key for group decisions.
5. **External approvers:** recommended default is disabled until legal/accountant/labor workflows have signed
   retention, identity, and passkey/e-signature requirements.
