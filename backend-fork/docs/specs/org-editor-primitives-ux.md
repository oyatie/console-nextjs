> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Spec: No-Code Org Editor Primitives and Setup UX Flows

**Status:** Draft planning spec for Kanban `t_ccc11e52`  
**Parent lane:** `NORTHSTAR-NOCODE-ORG-OPS-EDITOR-20260701` (`t_7e7a01e7`)  
**Scope:** Maintenance repo only. Planning/design artifact; no implementation in this card.  
**Parent specs:** `SPEC.md`, `docs/specs/knl-business-os.md`, `docs/specs/org-hierarchy.md`, `docs/specs/hr-core.md`, `docs/specs/rbac-configurable.md`, `docs/specs/data-exchange-import-export.md`, `docs/decisions/ADR-0004-passkeysfirst-auth-with-rotating-refreshtoken-families.md`

## 1. Objective

Define the no-code editor's **core company-structure primitives** and **CRUD-first setup UX** so a non-technical admin can model a real 점조직-style HQ/group: group/HQ, subsidiary corporations/orgs, departments/teams, employees, positions, reporting lines, worksites/사업장 cells, and cross-organization work assignments.

This document deliberately starts with the basics: a company needs people, structure, worksites, positions, reporting lines, and safe account/policy setup before advanced import, analytics, or workflow automation. Import/export is secondary migration/bootstrap tooling; the primary product is database-backed CRUD screens that let admins create, inspect, edit, deactivate, restore, and audit each object directly.

## 2. Current substrate and target gap

Existing repo context already provides important foundations:

- `groups`, `organizations.group_id`, `group_memberships`, and group-role resolvers establish the Group/HQ -> Organization topology without weakening the `app.current_org` RLS boundary.
- `organizations` are the hard legal-entity / tenant data boundary.
- `regions` and `branches` provide an early location hierarchy and already use soft deactivation for safe CRUD.
- `employees` hold tenant-scoped HR directory rows with safe promoted fields, raw-row preservation, identity-review metadata, and an explicit nullable `users.employee_id` link.
- `employee_lifecycle_events` provide append-only onboarding/offboarding/termination/transfer evidence.
- Passkey-first auth is the accepted account/security posture.

The target editor must turn these foundations into a coherent setup product. Today several concepts still appear as strings or partial views (`employees.org_unit`, `employees.position`, `employees.worksite_name`). The editor target is to make them first-class, linkable objects with lifecycle, validation, audit, and no-code policy hooks.

## 3. Product principles

1. **CRUD-first, import-second.** Every primitive must have a direct database-backed screen and action set. Import may seed drafts, but admins must be able to manage the same objects manually without spreadsheets.
2. **Object-centric.** Each primitive is a navigable object with identity, properties, relationships, timeline, audit/provenance, and guarded actions.
3. **Group is not a tenant.** A Group/HQ coordinates member organizations. Writes still name a target Organization unless a future reviewed group-scoped workflow says otherwise. Organization remains the RLS hard boundary.
4. **Non-technical admin UX.** The editor uses wizards, tree/canvas views, tables, inspectors, preview panels, and plain-language validation. Admins should not write SQL, JSON, or code to create the company model.
5. **Draft -> validate -> publish.** Complex setup happens in drafts. Publishing runs referential checks, cycle checks, policy/access simulation, required-field checks, and conflict detection before live data changes.
6. **Lifecycle over destructive delete.** Foundational objects use deactivate/archive/retire with referential guards. Historical payroll, labor, audit, and reporting evidence must not disappear.
7. **Policy/PBAC hooks from day one.** This spec names the objects and UX flows; sibling specs own the full policy inheritance and Cedar/PBAC details. The primitives here must expose the attributes Cedar/PBAC and ruleset simulation will need.

## 4. Core primitives

### 4.1 Group / HQ

**Purpose.** A conglomerate or HQ coordination layer that owns consolidated visibility and shared defaults across member organizations. It is not an RLS tenant and never arms `app.current_org`.

**Key fields.** Stable slug, display name, legal/HQ label, status (`draft`, `active`, `suspended`, `archived`), primary country/timezone, default language, default policy/ruleset bundle references, owner/admin contacts, audit metadata.

**Relationships.** Has many member `Organization` records. Has group-level admin grants. May define inherited defaults for policies, approval templates, payroll calendars, worksite classifications, and org chart conventions.

**Lifecycle.** Draft -> active -> suspended -> archived. Suspending a group blocks new cross-org assignment activation and group-wide policy publishing, but does not suspend member organizations automatically.

**Validation.** Slug uniqueness; at least one active group admin before activation; member orgs must be active and in at most one group; cannot archive while live cross-org assignments or group-level approvals are active without a transfer/revoke plan.

**CRUD surface.** `Group Management`: create HQ/group, edit identity, add/remove member organizations through guarded actions, view consolidated setup progress, manage group admins, inspect member org health, deactivate/archive with impact preview.

### 4.2 Organization / Corporation / Legal Entity

**Purpose.** A legal company/corporation/subsidiary. This is the tenant data boundary for ordinary business records.

**Key fields.** Slug, legal name, display name, business registration metadata, status (`setup_pending`, `active`, `suspended`, `archived`), group membership, locale/timezone, address, primary contact, onboarding stage, default payroll/workweek/policy bundle refs.

**Relationships.** Belongs to zero or one `Group`. Owns departments/teams, worksites/cells, positions, employees, accounts, local policies, and operational data.

**Lifecycle.** Setup pending -> active -> suspended -> archived. Archived orgs remain queryable to authorized platform/HQ views for historical evidence but cannot receive new assignments.

**Validation.** Slug uniqueness; group membership is explicit; at least one active organization admin before activation; status changes require impact preview; destructive delete is platform-only and outside normal org-editor CRUD.

**CRUD surface.** `Organizations`: create subsidiary/org, edit profile, assign to group, view activation checklist, suspend/archive with impact preview, drill into org-specific setup.

### 4.3 Department / Team / OrgUnit

**Purpose.** The internal org chart inside one Organization: departments, divisions, teams, crews, or HQ cells.

**Key fields.** Name, code, type (`division`, `department`, `team`, `crew`, `hq_cell`, `project_cell`), parent org unit, owning organization, status (`draft`, `active`, `inactive`, `archived`), effective dates, cost center, manager position, inherited default refs.

**Relationships.** Belongs to one Organization. May nest under another OrgUnit. Has many Positions, Employees/Assignments, and responsibility scopes. May map to one or more worksites/cells for operational execution.

**Lifecycle.** Draft -> active -> inactive -> archived. Inactive units cannot receive new assignments, but historical employees and reports retain references.

**Validation.** No parent cycles; unique sibling code/name within an org; manager position must belong to the same org unless represented as an approved cross-org assignment; cannot deactivate while active positions or employees remain without reassignment plan.

**CRUD surface.** `Org Structure Editor`: tree/canvas create, rename, move, split/merge plan, deactivate, restore, assign manager position, view linked employees/positions/worksites, run cycle and impact checks.

### 4.4 Worksite / 사업장 Cell

**Purpose.** A physical, operational, or administrative cell where work happens. This covers 사업장, branches, yards, factories, customer sites, project sites, and temporary cells.

**Key fields.** Name, code, site type (`office`, `branch`, `factory`, `customer_site`, `field_site`, `project_cell`, `remote`), address/geocode, owning organization, parent region/branch when applicable, status (`planned`, `active`, `temporarily_closed`, `deactivated`, `archived`), operating calendar, safety/entry requirements, default policy/ruleset/payroll-context refs.

**Relationships.** Belongs to one Organization. May be grouped under Region/Branch. May host employees from the owning org or cross-org assignments from another member organization. May attach to departments/teams, cost centers, assets, work items, and customer/vendor records.

**Lifecycle.** Planned -> active -> temporarily closed -> deactivated -> archived. Temporary closure pauses new work and assignments but preserves historical operations.

**Validation.** Active worksites need address or explicit remote flag; cannot deactivate while active assignments/workflows are open unless a closure plan exists; cell-specific overrides must not contradict locked group/org rules; payroll context must be explicit where local wage/hour rules differ.

**CRUD surface.** `Worksites & Cells`: create site/cell, edit details, classify cell type, attach departments/teams, view active employees/assignments, configure local setup hooks, close/deactivate with referential guard.

### 4.5 Person

**Purpose.** A real human identity independent from employment, login account, and assignment. One person can have employment history, accounts, and assignments over time.

**Key fields.** Legal/display name, preferred name, contact channels, identity-resolution metadata, privacy/sensitivity flags, status (`draft`, `invited`, `active`, `inactive`, `retained`), source provenance, duplicate-review state.

**Relationships.** May have one or more Employee records over time, one linked User account when approved, and many lifecycle events/assignments. Sensitive identifiers are not shown in general org setup.

**Lifecycle.** Draft/import-review -> invited -> active -> inactive/retained. Name-only merges are forbidden; manual review is required for ambiguous identity.

**Validation.** No automatic merge by name; explicit trusted identifier or manual review for user-account link; contact channel validation before invitation; privacy basis required for sensitive fields.

**CRUD surface.** `People Directory`: create person, edit safe contact/display fields, mark duplicate review, link/unlink to employee/account through guarded review, retain/archive with privacy policy.

### 4.6 Employee / Worker Profile

**Purpose.** The employment/worker record owned by a home Organization. This is HR data, not the same thing as a login role.

**Key fields.** Employee number, home organization, employment type, hire/exit dates, employment status (`pending_setup`, `active`, `on_leave`, `suspended`, `terminated`, `retained`), org unit, primary position, worksite, manager, source keys, identity review flags, lifecycle summary.

**Relationships.** Belongs to one home Organization and one Person. May be linked to one User account. May hold current primary Assignment and many historical EmploymentAssignment / CrossOrgAssignment records.

**Lifecycle.** Pending setup -> active -> on leave/suspended/transferred -> terminated/retained -> rehired/reactivated. Changes are recorded as lifecycle events, not destructive edits.

**Validation.** Employee number unique per org when present; active employee requires home org and at least one current assignment or explicit unassigned state; terminated employees cannot receive new active assignments without rehire/reactivation; payroll-sensitive fields are not edited through general employee setup.

**CRUD surface.** `Employees`: create worker, edit safe HR fields, assign primary position/team/site, trigger lifecycle event, link account, invite passkey setup, deactivate/terminate with handoff checklist, inspect history.

### 4.7 Platform Account / User

**Purpose.** Authentication credential and session principal. A User is not automatically an Employee, Position, or Role.

**Key fields.** Email/login identifier, org, linked employee id, status (`invited`, `credential_pending`, `active`, `suspended`, `revoked`), passkey enrollment state, system roles/bootstrap grants, last credential event.

**Relationships.** Belongs to one Organization for tenant auth. May link to one Employee in the same org after review. May receive role/policy assignments and cross-org access through approved grants.

**Lifecycle.** Invite -> passkey enrollment pending -> active -> suspended/revoked. Passkey ceremonies follow ADR-0004.

**Validation.** Same-org employee FK; no name-only linking; credential setup required before active account; elevated grants require passkey step-up and audit.

**CRUD surface.** `Account Seeds & Access`: create invitation, resend passkey setup, link to employee, suspend/revoke session family, view credential history, preview effective access before granting.

### 4.8 Position / Role-in-Organization

**Purpose.** A job position/title/responsibility node such as 대표, 현장소장, HR manager, dispatcher, mechanic lead, or payroll processor. This is distinct from login RBAC roles, though it may recommend policy bundles.

**Key fields.** Title, code, organization, org unit, level/band, job family/function, status (`draft`, `active`, `frozen`, `retired`), default manager relationship, default policy bundle, required certifications, approval authority hints.

**Relationships.** Belongs to an Organization and optionally an OrgUnit. Has many incumbent Employees over time. May report to another Position. May suggest Role/Policy assignments but cannot grant access by itself.

**Lifecycle.** Draft -> active -> frozen -> retired. Retired positions stay visible for history and payroll/legal evidence.

**Validation.** Unique code per org; no reporting cycles; one primary manager path unless explicit matrix/dotted-line reporting is configured; cannot retire while active incumbents remain without reassignment plan.

**CRUD surface.** `Positions`: create/edit title, connect to department/team, define manager position, set default responsibilities, view incumbents, retire with reassignment checklist.

### 4.9 Policy Role / Permission Bundle Hook

**Purpose.** A human-readable access bundle such as org admin, HR manager, payroll processor, worksite manager, or group viewer. The full Cedar/PBAC model is specified by the sibling authorization card, but the org editor must expose the hook at setup time.

**Key fields.** Display name, scope type (`group`, `org`, `department`, `worksite`, `object`, `self`), assignable status, version, effective policy preview, required approver/passkey step-up, owner.

**Relationships.** May be recommended by Position or Assignment. Applies to a Principal over a Resource scope. May inherit from system defaults and local policy bundles.

**Lifecycle.** Draft -> previewed -> approved -> active -> retired/rolled back.

**Validation.** Cannot silently escalate beyond assigner's authority; unsupported/elevated policy remains inert until reviewed; effective access preview is mandatory before activation.

**CRUD surface.** `Access & Policy Preview`: select a person/position/assignment, preview recommended bundles, request/approve grants, view effective CRUD permissions, revoke with impact preview.

### 4.10 Reporting Line

**Purpose.** A dated manager/subordinate or dotted-line relationship between positions or employees.

**Key fields.** Reporter, manager, relationship type (`direct`, `dotted`, `temporary`, `project`, `cross_org_supervision`), source object type (`position` or `employee_assignment`), effective dates, status (`draft`, `active`, `scheduled`, `expired`, `revoked`), reason.

**Relationships.** Connects Positions and/or Assignments. May cross organizations only through an approved CrossOrgAssignment and explicit visibility/access scope.

**Lifecycle.** Draft -> active/scheduled -> expired/revoked. Historical reporting lines remain visible on timelines.

**Validation.** No cycles in direct management graph; effective date ranges cannot overlap for a single primary manager unless an explicit matrix rule allows it; cross-org reporting requires host and home org approval.

**CRUD surface.** `Reporting Lines`: create manager line by dragging in org chart, edit effective dates/type, preview resulting visibility/approval chain, revoke/supersede with audit.

### 4.11 Employment Assignment

**Purpose.** The dated relationship placing an Employee into an Organization, OrgUnit, Position, and optional Worksite/Cell. This is the primary source for org chart membership.

**Key fields.** Employee, home organization, assignment organization, org unit, position, worksite/cell, assignment type (`primary`, `secondary`, `temporary`, `project`, `cross_org`), start/end dates, allocation percent, status (`draft`, `pending_approval`, `active`, `scheduled`, `expired`, `revoked`, `rejected`), payroll-owner context, policy context.

**Relationships.** Links Employee, Organization, OrgUnit, Position, Worksite/Cell, ReportingLine, and PolicyRole grants.

**Lifecycle.** Draft -> pending approval -> active/scheduled -> expired/revoked. Corrections create superseding records, not silent edits to history.

**Validation.** Active assignments require active employee, active target objects, non-overlapping primary assignment date ranges, payroll-owner clarity, and access preview. Cross-org assignment requires group membership or explicit approved relation.

**CRUD surface.** `Assignments`: create or transfer employee, choose org/team/position/site, preview manager chain and access, request approval, activate, extend, end, revoke.

### 4.12 Cross-Organization Assignment

**Purpose.** A specialization of EmploymentAssignment where the worker's home Organization differs from the host Organization/site. This is first-class because 점조직 HQ/group operations often dispatch or lend workers across subsidiaries/cells.

**Key fields.** Home org, host org, host org unit/site/cell, worker, requested role/position, requested access bundles, payroll owner, cost allocation, effective dates, approval packet, revoke plan, status (`draft`, `pending_home_approval`, `pending_host_approval`, `pending_policy_preview`, `active`, `paused`, `expired`, `revoked`, `rejected`).

**Relationships.** Requires a Group/HQ relationship or explicit inter-org contract. Links to reporting lines, policy grants, payroll/ruleset context, audit events, and operational work queues.

**Lifecycle.** Draft -> approvals/policy preview -> active -> expired/revoked. Host access and reporting visibility terminate on revoke/expiry; home employment history remains.

**Validation.** Home and host org must be authorized members of the same group or an approved relationship; host worksite/cell must be active; payroll owner must be explicit; access grants must be scope-limited and revocable; lower/site rules cannot contradict locked group/org rules.

**CRUD surface.** `Cross-Org Assignments`: request assignment, preview worker effective rules/access, collect approvals, activate, monitor active cross-org workers, revoke/expire with handoff checklist.

### 4.13 Setup Draft / Change Request

**Purpose.** A versioned draft envelope for multi-object setup changes before publishing them live.

**Key fields.** Draft name, target group/org, author, status (`draft`, `validating`, `needs_changes`, `ready_to_publish`, `published`, `discarded`), included changes, validation results, simulation snapshot, approvers, published version.

**Relationships.** References all primitive changes in a setup batch. Produces audit events and, when approved, real object mutations.

**Lifecycle.** Draft -> validate -> approve -> publish -> audit/version. Failed validation returns actionable errors by object and field.

**Validation.** Required object completeness, relationship consistency, no cycles, policy/ruleset conflicts, publish permissions, passkey step-up for sensitive/elevated changes.

**CRUD surface.** `Setup Drafts`: create draft, edit batch, run validation/simulation, request review, publish, rollback to prior version where safe.

### 4.14 Audit / Provenance Record

**Purpose.** Every setup mutation and simulation outcome must be traceable.

**Key fields.** Actor, target object, target org, action, before/after diff summary, reason, source (`manual`, `import`, `workflow`, `system`), policy version, passkey step-up marker, timestamp.

**Relationships.** Attached to every primitive's timeline and to setup drafts/change requests.

**Lifecycle.** Append-only. Corrections are new audit events.

**Validation.** No setup publish without audit. Sensitive diffs are masked according to domain policy.

**CRUD surface.** Read/search/export only for authorized users; no update/delete through normal admin UI.

## 5. Relationship model

The editor should expose the structure as a graph while enforcing clear ownership rules:

```text
Group/HQ
  -> Organization/Corporation (RLS boundary)
    -> OrgUnit hierarchy (department/team/HQ cell)
    -> Worksite/사업장 cells
    -> Position graph
    -> Employee/Worker profiles
      -> Employment assignments
      -> Reporting lines
      -> Account links/passkey invitations
Group/HQ
  -> Cross-organization assignments between member Organizations
```

Rules:

- A Group may coordinate and view member setup, but live writes target a specific Organization unless a reviewed group-level primitive explicitly owns the write.
- A Person is not automatically an Employee; an Employee is not automatically a User; a Position is not automatically an access Role.
- EmploymentAssignment is the canonical link from employee to org unit/position/worksite. Avoid copying department/team/site strings into unrelated tables once first-class objects exist.
- CrossOrgAssignment preserves a worker's home organization and payroll owner while granting narrowly scoped host organization visibility/action rights.
- Reporting lines should prefer Position-to-Position definitions for stable org charts, with Employee/Assignment overrides for temporary or exceptional cases.
- Worksite/cell membership may overlap with departments/teams, but a cell remains first-class because site-specific policy, payroll, safety, access, and operational quirks can differ from the org chart.

## 6. Lifecycle and validation standards

All primitives use the same setup grammar:

1. **Create draft.** Admin starts from a wizard, table action, tree canvas, or import preview.
2. **Edit relationships.** The editor shows inline required fields, relationship pickers, and object previews.
3. **Validate.** Server-side validation checks required fields, uniqueness, effective dates, status compatibility, cycles, references, and privilege.
4. **Simulate.** The preview shows the resulting org chart, reporting chain, effective access, rule inheritance, and payroll/operational context flags.
5. **Approve/publish.** Sensitive changes require approval/passkey step-up. Publishing writes audited DB rows.
6. **Operate and revise.** Admins can edit, transfer, deactivate, revoke, archive, or supersede without losing history.

Minimum validation set:

- Required identity fields: every Group/Organization/OrgUnit/Worksite/Position has name and status; slugs/codes are unique in their scope when present.
- No hierarchy cycles: OrgUnit parent tree, Position/reporting tree, and cross-org supervision chains must be acyclic unless explicitly marked dotted-line/non-primary.
- Effective-date correctness: no overlapping primary assignments, no activation against inactive target objects, no end date before start date.
- Referential guards: deactivating groups/orgs/units/sites/positions requires reassignment, closeout, or archival justification for active children.
- Access guard: publishing setup changes that affect accounts, roles, cross-org assignments, reporting visibility, payroll context, or policy context requires effective-access preview and audit.
- Group/org/site precedence guard: lower cell/site settings can be local only where group/org rules allow override; contradictions are blocked or routed to an explicit exception approval.
- Privacy guard: sensitive identity/payroll/contact fields require correct domain permissions and masking.

### 6.1 Rollback, observability, and operational evidence

Setup changes need an operational closeout path, not just a happy-path publish:

- **Rollback/supersede plan.** Every SetupDraft publish stores the prior version, exact object/action diff, policy/ruleset version, actor, target org, and a safe rollback or supersede path. Normal rollback is another audited draft/change request that restores or supersedes configuration where referentially safe; it is not a destructive database restore or audit deletion.
- **Revoke-aware rollback.** Account grants, policy roles, cross-org assignments, reporting lines, and worksite/cell rules declare what must be revoked, expired, re-simulated, or re-approved when a draft is rolled back. Sensitive revokes require passkey step-up and produce session/access invalidation evidence where applicable.
- **Blocked rollback handling.** If active employees, assignments, payroll contexts, work items, or policy grants make a rollback unsafe, the UI shows the blocker, affected objects, owner, and required handoff rather than silently applying a partial revert.
- **Bounded observability.** The product records counters and traces for setup validation failures, publish success/failure, policy simulation denies, referential-conflict blockers, approval latency, revoke/rollback events, stale drafts, and import-vs-manual create sources. Metrics must use object type/status/reason buckets only; worker names, raw IDs, wage values, resident identifiers, bank data, and private HR notes must not be metric labels.
- **Audit and support traceability.** Every create/update/deactivate/archive/revoke/publish/rollback action carries request id, trace id, actor, target org, object type/id, before/after diff summary, policy/ruleset version, decision result, and reason codes safe for admin/support display. Object timelines and the setup checklist surface these events so support can explain what changed and how to recover.

## 7. UX surfaces and flows

### 7.1 Post-signup foundation flow

Goal: move a new customer from empty SaaS account to an active company model without code or imports.

1. User signs up and chooses `Create a new company/group` or `Join an invited organization`.
2. The product creates a setup draft and asks for the first Organization or Group/HQ identity.
3. User completes passkey enrollment before any privileged setup publish.
4. The wizard asks whether the company is a single organization or group/HQ with subsidiaries.
5. The setup checklist appears with progress cards: Legal entities, Departments/teams, Worksites/cells, Positions, Employees, Account invitations, Reporting lines, Policy/access preview.
6. The first admin can save partial drafts; nothing becomes active until validation passes and the publish action is audited.

Required screens/actions:

- `Signup / Start setup`: create draft, choose setup model, collect safe org identity.
- `Passkey enrollment`: create credential, confirm active session, return to setup draft.
- `Setup checklist`: resume draft, show blockers, navigate to each primitive editor.

### 7.2 Group/HQ onboarding flow

Goal: model 점조직-style HQ/group management where HQ coordinates multiple legal entities and site/cell operations.

1. Create Group/HQ object.
2. Add member Organizations one by one, each with legal name, display name, slug, status, and primary contact.
3. Use the scope selector to view `All subsidiaries` or one Organization.
4. For each Organization, define departments/teams, worksites/cells, positions, and employees.
5. Configure group-level setup defaults and mark which fields lower organizations/sites may override.
6. Preview consolidated org chart and member-specific drilldown.
7. Publish only when every active Organization has an admin, required org tree, and validated setup baseline.

Required screens/actions:

- `Group Management`: create/edit HQ, member table, consolidated setup progress, group admins, member health.
- `Scope selector`: All subsidiaries -> single org -> region/branch/worksite where authorized.
- `Member Organization wizard`: create/edit/suspend/archive member org, assign to group, inspect activation blockers.

### 7.3 First organization tree flow

This is the first organization tree setup experience for a new company after signup, passkey enrollment, and basic org onboarding.

Goal: let a non-technical admin create the first usable org structure.

1. Open `Org Structure Editor` for one Organization.
2. Add top-level departments/teams/HQ cells through `+ Department/Team`.
3. Drag units to nest or move them; the editor runs cycle and permission checks before saving.
4. Click a node to edit details, status, manager position, cost center, and default setup refs.
5. Create a setup draft and preview resulting org chart before publish.
6. Publish writes/updates OrgUnit records and audit events.

Required screens/actions:

- Tree/canvas view, searchable table view, detail inspector, add/edit/move/deactivate/restore actions, validation panel, audit timeline.

### 7.4 Employee and account setup flow

Goal: create employees, then optionally invite accounts/passkeys, without conflating HR identity and login access.

1. Create or select a Person.
2. Create Employee/Worker Profile in the home Organization with employee number, employment status, hire date, primary org unit, primary position, and primary worksite/cell.
3. Add lifecycle event (`ONBOARD`, `TRANSFER`, `TERMINATE`, etc.) as needed.
4. If the worker needs product access, create an Account invitation linked to the reviewed Employee row.
5. The worker enrolls a passkey and becomes an active User.
6. Admin previews and approves access bundles separately from job title.

Required screens/actions:

- `People Directory`: create/edit person, duplicate review, link evidence.
- `Employees`: create/edit employee, lifecycle event, assignment, terminate/reactivate.
- `Account Seeds & Access`: invite, resend, link employee, suspend/revoke, passkey status, effective access preview.

### 7.5 Position and reporting-line flow

Goal: model positions and reporting relationships independently from individual people.

1. Create Position records under departments/teams.
2. Define manager position and optional dotted-line reporting relationships.
3. Assign employees to positions through EmploymentAssignment records.
4. Preview the org chart, manager chain, approval chain, and effective visibility.
5. Publish with cycle checks and audit.

Required screens/actions:

- `Positions`: add/edit/retire, set manager position, view incumbents, suggested policy bundles.
- `Reporting Lines`: add direct/dotted/temporary line, edit effective dates, revoke/supersede, cycle/conflict preview.

### 7.6 Worksite / 사업장 cell setup flow

Goal: treat operational cells as first-class objects, not free-text employee fields.

1. Create Worksite/Cell with site type, address/remote flag, owning org, operating calendar, and status.
2. Attach site to branch/region and optional departments/teams.
3. Assign employees or teams to the cell.
4. Mark local setup hooks: safety requirements, work calendar, payroll context, operational quirks, and whether each is inherited or local.
5. Preview which employees/workflows will be affected before activation or deactivation.

Required screens/actions:

- `Worksites & Cells`: create/edit/deactivate/restore, attach teams, assign employees, view active work and cross-org workers, preview local rules/access.

### 7.7 Cross-organization worker assignment flow

Goal: enable HQ/group operations where a worker from one subsidiary helps another subsidiary/site while preserving ownership, payroll context, and revocation.

1. Home org admin or HQ admin starts a CrossOrgAssignment draft.
2. Select worker, home org, host org, host team/site/cell, requested position/responsibility, effective dates, and payroll owner.
3. Preview host access, reporting visibility, rule inheritance, and payroll/quirk context.
4. Collect required home/host/HQ approvals.
5. Activate the assignment; host org sees only the scoped worker and permitted actions.
6. Expire or revoke assignment; access grants and reporting visibility are removed while home employee history remains.

Required screens/actions:

- `Cross-Org Assignments`: request, edit draft, approval status, preview access/rules, activate, extend, pause, revoke, audit.

## 8. Database-backed CRUD screens/actions

| Surface | Primary objects | Required actions |
| --- | --- | --- |
| Setup checklist | SetupDraft, Organization, Group | create draft, resume, validate, simulate, publish, discard |
| Group Management | Group, Organization membership | create/edit group, add/remove member org, suspend/archive group, manage group admins, inspect setup progress |
| Organizations | Organization | create/edit org, assign to group, activate, suspend, archive, restore where allowed, view audit |
| Org Structure Editor | OrgUnit | create/edit/move, set parent/manager, deactivate/archive/restore, validate tree, publish draft |
| Worksites & Cells | Worksite/Cell | create/edit/classify, attach teams, assign workers, temporarily close, deactivate/archive, preview impact |
| People Directory | Person | create/edit, duplicate review, privacy masking, link evidence, retain/archive |
| Employees | Employee, EmployeeLifecycleEvent | create/edit safe HR fields, onboard/transfer/terminate/reactivate, assign primary position/site, view history |
| Account Seeds & Access | User, User-Employee link, passkey state | invite, link/unlink employee, resend passkey setup, suspend/revoke, preview effective access |
| Positions | Position | create/edit, set job family/level, connect to org unit, retire, view incumbents |
| Reporting Lines | ReportingLine | create/edit direct or dotted line, schedule, revoke, validate cycles |
| Assignments | EmploymentAssignment | create/edit/approve/activate/end/revoke, preview manager/access/payroll context |
| Cross-Org Assignments | CrossOrgAssignment | request, approve, simulate, activate, extend, expire, revoke |
| Audit & History | Audit/Provenance | search, view object timeline, export authorized evidence, compare versions |

Each screen needs loading, empty, partial failure, full failure, validation, and referential-conflict states. Disabled actions must explain the missing permission or blocked lifecycle condition.

## 9. No-code editor behavior for non-technical admins

The editor should avoid exposing raw implementation details. Admin-facing behavior:

- **Tree/canvas mode** for org units, positions, and reporting lines.
- **Table mode** for large rosters, worksites, assignments, and member organizations.
- **Inspector panel** for selected object fields, lifecycle status, relationships, and audit.
- **Relationship pickers** with scoped search and safe labels, never raw UUIDs as primary display.
- **Setup templates** for common structures: single company, group/HQ with subsidiaries, project cell, branch/site operations, HR/payroll back office.
- **Validation drawer** grouped by severity: blockers, warnings, cleanup suggestions.
- **Simulation drawer** answering: who can see/do what, who reports to whom, which rules apply, which payroll/worksite context applies, and what audit events will be written.
- **Publish summary** listing exact objects to create/update/deactivate and requiring passkey step-up for sensitive changes.

## 10. 점조직-style HQ/group handling

The editor must explicitly support a HQ/group that coordinates many legal entities and cells:

- HQ can create and monitor member Organizations from one consolidated setup surface.
- HQ users see `All subsidiaries` only when granted group scope; otherwise users remain locked to their org/subtree.
- HQ can define group-wide defaults and locked guardrails, then let subsidiaries or worksites override only allowed fields.
- A worker's **home Organization** remains the owner for employment history and payroll unless a specific approved assignment says otherwise.
- A host Organization/site can receive a cross-org worker with scoped operational access, temporary reporting lines, and revocable permissions.
- Consolidated views aggregate member-org data through per-member scoped reads; the UX should not imply that Group is a tenant or that group id can be used as an org id.
- Every cross-entity write names the target Organization and records the real actor, home org, host org, reason, and approval chain.

## 11. Handoff boundaries to sibling specs

This card defines primitives and setup UX. It intentionally leaves the full details of these topics to sibling tasks while preserving integration hooks:

- Policy inheritance, site/cell overrides, quirks, payroll rulesets, conflict resolution, and inherited-vs-local simulation: `t_388bf246`.
- Cedar/PBAC entity/action/context mapping and generated policy evaluation: `t_2807559b`.
- Full cross-org assignment workflow approvals, revocation, eligibility, and operations interactions: `t_75025850`.
- Final approved plan and PR lanes: `t_cac2779c` after all planning specs complete.

This spec's primitives must remain compatible with those outputs: every object has stable identity, lifecycle, relationships, status, owner org, audit hooks, and enough attributes to feed policy/ruleset evaluation.

## 12. Acceptance coverage

| Acceptance requirement | Coverage |
| --- | --- |
| Clearly names all primitives | Section 4 defines Group/HQ, Organization, OrgUnit, Worksite/Cell, Person, Employee, User, Position, PolicyRole hook, ReportingLine, EmploymentAssignment, CrossOrgAssignment, SetupDraft, Audit. |
| Shows how non-technical admin creates and edits them | Sections 7-9 describe wizard, tree/canvas, table, inspector, validation, simulation, publish, and per-object screens/actions. |
| Identifies database-backed CRUD screens/actions | Section 8 maps every surface to primary objects and create/edit/deactivate/archive/revoke/preview/publish actions. |
| Handles group/HQ and 점조직 management | Sections 4.1, 7.2, and 10 define HQ/group member management, consolidated setup, scope selector, cross-org worker handling, and target-org writes. |
| Covers requested setup story pieces | Sections 7.1-7.7 cover signup, org onboarding, passkey enrollment, first org tree, adding employees, assigning positions/reporting lines, worksites/cells, and cross-org assignments. |
| Preserves CRUD-first north star | Sections 1, 3, and 8 state import-second posture and require manual database-backed CRUD surfaces before import dependence. |
| Records rollback and observability expectations | Section 6.1 defines rollback/supersede behavior, revoke-aware rollback, blocked rollback handling, bounded observability, and support/audit traceability. |
