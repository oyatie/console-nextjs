> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Approved Plan: No-Code Org/Ops Editor, Cedar/PBAC, and Cross-Org Work

Date: 2026-07-01  
Status: APPROVED PLAN ARTIFACT / PLANNING ONLY  
Kanban: `t_cac2779c`, synthesized from root `t_7e7a01e7` and child planning specs.  
Scope: maintenance repo only. No Oyatie changes. No code, schema, endpoint, migration, or production policy change is authorized by this document.

## 1. Purpose

This document consolidates the completed no-code org/ops editor planning specs into one coherent plan for later PR-lane approval. It starts from the product basics: a company needs people, a legal/org structure, worksites/cells, roles/positions, policies, rules, assignments, and audited CRUD workflows before import/export or advanced automation can be first-class.

The target product is a database-backed, CRUD-first B2B SaaS editor where non-technical administrators can model a 점조직-style HQ/group with subsidiaries, departments, teams, worksites/사업장 cells, employees, positions, reporting lines, cross-organization work assignments, inherited policies, local rules, payroll-readiness context, and Cedar/PBAC-governed CRUD permissions.

This plan intentionally does not prescribe exact table names, Rust modules, endpoints, or UI component files. Those details belong in future implementation PR lanes after this plan is accepted.

## 2. Source artifacts synthesized

Primary planning specs:

- `docs/specs/org-editor-primitives-ux.md` — no-code editor primitives, object lifecycles, CRUD surfaces, setup flows, rollback, observability, and audit expectations.
- `docs/specs/no-code-operational-logic.md` — inherited defaults, rule/ruleset attachment scopes, site/cell overrides, operational quirks, payroll ruleset boundaries, simulation/preview, audit, and observability.
- `docs/specs/cross-org-work-assignments.md` — cross-org assignment lifecycle, home/host projections, approvals, access grants, reporting exceptions, payroll context, revocation, and operational UI surfaces.
- `docs/specs/cedar-pbac-authorization.md` — Cedar/PBAC principal/resource/action/context vocabulary, generated policy bundle contract, runtime evaluation flow, scope-precedence/conflict metadata, context-switch actions, RLS invariants, audit, revoke, cache safety, and delivery plan.

Governing context:

- `SPEC.md` — KNL conglomerate operations platform root spec; RLS, audit, passkey, quality bar, and no-AI boundary.
- `docs/specs/org-hierarchy.md` — group/HQ hierarchy, per-member armed reads, real-Org-only RLS, group grants, and scope selector constraints.
- `docs/specs/rbac-configurable.md` — configurable roles/policy direction, capability/PBAC posture, role-string authorization ban, passkey-gated policy lifecycle, versioned effective policy, and revoke semantics.
- `.hermes/kanban-artifacts/2026-07-01-maintenance-direction-shift-focused-audit.md` — board audit that identified direction-relevant cards and required guardrails for future lanes.

## 3. Non-negotiable product principles

1. CRUD-first, import-second.
   - Every important primitive must have direct create/read/update/archive/revoke/simulate/audit surfaces.
   - Upload, Excel, and import/export are secondary bootstrap/migration tools and must not become the primary management model.

2. No-code, not code-hidden.
   - Administrators configure structures, policies, rules, approvals, eligibility, exceptions, simulations, and templates through product UI: wizards, tables, tree/canvas editors, inspectors, rule builders, preview panels, and Work Hub queues.
   - Admins do not write SQL, hidden scripts, JSON policy by hand, or direct database updates.

3. Organization remains the RLS hard boundary.
   - `app.current_org` is always a real legal-entity Org id.
   - Group/HQ, worksite/cell, assignment, policy, role, or scope ids never arm RLS.
   - Consolidated HQ/group views aggregate over per-member armed reads; they do not perform blanket BYPASSRLS scans.

4. Cedar/PBAC is the generated-logic authorization contract.
   - Generated forms, tables, workflows, object actions, rules, and policy templates are executable only through deterministic principal/resource/action/context decisions.
   - UI visibility is never the control. Server-side authorization, RLS, and audit decide.

5. Higher-scope guardrails supersede contradictory lower rules.
   - System/legal controls, group/HQ locked rules, and org/legal-entity locked rules can prevent site/cell/department/employee/assignment overrides that weaken safety, privacy, payroll, audit, passkey, or tenant isolation requirements.
   - Lower scopes may add local nuance only where the inherited rule declares the field overrideable, additive, stricter-only, or exception-only.

6. Cross-org work is explicit and revocable.
   - A worker keeps a home organization/legal employer while receiving bounded host-org work access through a governed assignment, projection, policy grant, approval case, audit trail, and revoke path.
   - Host orgs do not gain home payroll/PII visibility by default.

7. Audit, rollback, observability, and revoke are first-class.
   - Every publish, policy activation, assignment activation, CRUD mutation, approval, denial, revoke, and simulation has bounded evidence.
   - Observability must use safe reason/status buckets and avoid raw worker names, wage values, resident identifiers, bank data, private HR notes, or other sensitive facts as labels.

## 4. Editor primitives and object model

The no-code editor should expose the following primitives as navigable objects with identity, owner scope, lifecycle, relationships, validation, audit/provenance, and CRUD actions.

| Primitive | Product role | CRUD/editor expectations |
| --- | --- | --- |
| Group / HQ | Coordinates subsidiaries and group-level defaults/guardrails; not a tenant. | Create/edit HQ, manage member orgs, group admins, consolidated setup progress, locked defaults, suspend/archive with impact preview. |
| Organization / Corporation / Legal Entity | Real tenant/legal entity and RLS boundary. | Create/edit org profile, assign to group, activate/suspend/archive, manage onboarding checklist, audit target-org writes. |
| Department / Team / OrgUnit | Internal org chart nodes inside an Org. | Tree/canvas/table create, rename, move, nest, assign manager position, deactivate/archive/restore, cycle checks. |
| Worksite / 사업장 Cell | Physical/operational/admin cell where work happens. | Create/edit/classify site/cell, attach teams, assign workers, configure local hooks, preview local rules/access/payroll context. |
| Person | Human identity independent from employment and account. | Create/edit safe identity/contact fields, duplicate review, privacy masking, link evidence, retain/archive. |
| Employee / Worker Profile | Home-org HR/worker record. | Create/edit HR-safe fields, lifecycle events, primary assignment, transfer, suspend, terminate/reactivate, inspect history. |
| Platform Account / User | Authentication/session principal. | Invite, link to reviewed employee, passkey setup, suspend/revoke session family, preview access. |
| Position / Role-in-Organization | Job/title/responsibility node, distinct from login authorization. | Create/edit/retire, connect to org unit, define manager position, suggest policy bundles, view incumbents. |
| Policy Role / Permission Bundle Hook | Human-readable access/policy bundle hook. | Preview recommended bundles, request/approve grants, view effective access, revoke with impact preview. |
| Reporting Line | Dated manager/subordinate or dotted-line relationship. | Create/edit/revoke/supersede direct/dotted/temporary/cross-org lines, validate cycles and effective dates. |
| Employment Assignment | Dated link from employee to org/unit/position/site. | Create/edit/approve/activate/end/revoke assignment, preview manager/access/payroll context. |
| Cross-Organization Assignment | Specialized assignment where home org differs from host org/site. | Draft, simulate, approve, activate, extend, suspend, revoke, close; preserve home ownership and host scoped access. |
| Setup Draft / Change Request | Versioned draft envelope for multi-object setup before publishing. | Create/edit/validate/simulate/request review/publish/rollback/discard; publish writes audited rows. |
| Audit / Provenance Record | Append-only mutation/decision/simulation evidence. | Search/view/export authorized evidence; no normal update/delete; corrections are linked annotations. |

Relationship model:

```text
Group / HQ
  -> Organization / Corporation / Legal Entity (RLS boundary)
    -> Department / Team / OrgUnit hierarchy
    -> Worksite / 사업장 Cell hierarchy or branch/site classification
    -> Position graph and ReportingLine graph
    -> Person / Employee / User / EmploymentAssignment records
    -> PolicyTemplate / RuleSet / Approval / Audit records
Group / HQ
  -> Cross-Organization Assignment between member Organizations
```

Important separations:

- Person is not Employee.
- Employee is not User.
- Position is not login authorization.
- Group/HQ is not an Org tenant.
- Worksite/cell is not a weaker tenant boundary.
- Cross-org assignment is not an employment transfer unless a separate HR transition changes legal employer/payroll ownership.

## 5. No-code UX flows

### 5.1 Post-signup foundation flow

1. User signs up and chooses `Create new company/group` or `Join invited organization`.
2. Product creates a Setup Draft and collects safe Group/HQ or first Organization identity.
3. User completes passkey enrollment before privileged setup publish.
4. Wizard asks whether the customer is a single organization or HQ/group with subsidiaries.
5. Setup checklist opens: legal entities, departments/teams, worksites/cells, positions, employees, accounts, reporting lines, policy/access preview.
6. Admin saves partial drafts while validation and simulation highlight blockers.
7. Publish requires passkey step-up for sensitive/elevated changes and writes audit/provenance records.

### 5.2 Group/HQ onboarding flow

1. Create Group/HQ object.
2. Add member Organizations one by one with legal/display identity, slug, status, primary contact, and setup state.
3. Use scope selector: `All subsidiaries` for consolidated views or a single Organization for real-org writes.
4. Define group-level defaults, mandatory guardrails, prohibited override list, shared approval templates, and policy/ruleset versions.
5. Mark which settings lower org/site/cell scopes may override.
6. Preview consolidated org chart and member-specific setup health.
7. Publish only when active member orgs have admin coverage, required structure, and validated policy/ruleset baseline.

### 5.3 First organization/editor model flow

1. Open Org Structure Editor for one Organization.
2. Create departments/teams/HQ cells using tree/canvas and table views.
3. Add worksites/사업장 cells and attach them to regions/branches/departments/teams where appropriate.
4. Create positions and reporting lines, then assign employees through EmploymentAssignment records.
5. Run cycle checks, required-field checks, effective-date checks, policy access preview, and payroll/ruleset preview.
6. Publish as an audited Setup Draft.

### 5.4 Employee/account setup flow

1. Create or select Person.
2. Create Employee/Worker Profile in the home Organization.
3. Add home org unit, primary position, primary worksite/cell, manager/reporting-line context, and employment status.
4. If product access is needed, create User invitation and link only after reviewed same-org employee identity.
5. User enrolls passkey.
6. Admin previews effective policy bundles separately from job title before granting access.

### 5.5 Policy/ruleset configuration flow

1. Select template: HQ safety baseline, org HR/payroll readiness, manufacturing worksite, dispatch office, cross-org temporary assignment, etc.
2. Choose attachment scope: group, org, department/team, worksite/cell, role/position, employee, cross-org assignment, workflow/object/action context.
3. Define rules in business terms: condition, effect, override mode, dates, approvals, evidence, reason.
4. Review inheritance tree: locked rules, inherited defaults, local overrides, additive rules, stricter-only rules, exceptions, conflicts.
5. Run simulation cases and before/after diff.
6. Approve and activate version, or reject and revise draft.
7. Monitor activation health, conflict blocks, simulation failures, exception expirations, rollback events, and stale-rule warnings.

### 5.6 Cross-organization worker assignment flow

1. Home org admin, host org admin, or HQ operator opens worker/site/work hub and starts CrossOrgAssignment draft.
2. Select worker, home org, host org, host department/team/site/cell, requested duties, dates, payroll owner, cost allocation label, requested host actions, and purpose.
3. Simulate worker eligibility, home constraints, host site/cell prerequisites, policy/ruleset inheritance, access diff, reporting graph, payroll/cost context, audit/revoke plan.
4. Collect home manager, home HR/payroll, host supervisor/site owner, host org admin, policy/security, and group/HQ approvals as required.
5. Activate host access grants for the bounded assignment window.
6. During active operations, every host work action still evaluates Cedar/PBAC against assignment state, policy/ruleset version, purpose, sensitivity, and passkey freshness.
7. Complete, expire, suspend, extend, or revoke. Revocation disables host access, removes queue membership, handles open work, notifies participants, and writes audit.

## 6. Inheritance, overrides, quirks, and payroll rulesets

The editor computes effective rules from a hierarchy plus overlays.

Primary hierarchy:

```text
System/legal guardrails
  -> Group / HQ guardrails and defaults
    -> Organization / legal entity guardrails and defaults
      -> Department / Team defaults
        -> Worksite / 사업장 Cell local rules
```

Orthogonal overlays:

```text
Role / Position
+ Responsibility assignment
+ Employee-specific exception
+ Cross-org assignment overlay
+ Workflow / object / action context
+ Time / shift / environment context
```

Rule metadata must include:

- scope owner;
- domain: access, eligibility, approval, operational quirk, payroll readiness, audit, privacy, workflow;
- effect: allow, deny/prohibit, require approval, require evidence, set default, set minimum/maximum, add checklist item, add payroll flag, require passkey step-up, require review;
- override mode: locked, overrideable, additive, replaceable, stricter-only, exception-only, prohibited, expires;
- effective dates and version;
- required approvers;
- safe reason text for admins and auditors.

Precedence model:

1. System/legal guardrails win first: tenant isolation, privacy, payroll release gates, audit, passkey step-up for signing-equivalent actions, and professional-validation blockers.
2. Locked Group/HQ guardrails win next.
3. Locked Organization/legal-entity rules win within that Org.
4. Overrideable defaults flow downward only where explicitly allowed.
5. Additive rules accumulate when safe.
6. Most-specific wins only for replaceable defaults.
7. Stricter restriction wins for safety, security, payroll, audit, privacy, and passkey controls.
8. Cross-org assignments are conjunctive: home org, host org/site, assignment, worker, workflow, and policy context must all allow.
9. Exceptions are explicit, time-bounded, approved overlays and must expire or route for renewal.

Conflict behavior:

- Lower rule contradicts locked upper guardrail: block activation and show inherited rule, owner, reason, and escalation path.
- Two defaults apply at same specificity: block until admin selects one or narrows conditions.
- Lower rule tries to weaken safety/privacy/payroll/audit/passkey: apply stricter inherited rule and flag attempted weakening as invalid.
- Payroll ownership conflict: keep home/legal-employer payroll owner unless reviewed employment transfer changes ownership.
- Effective-date overlap: block incompatible overlapping rules.
- Cross-org visibility conflict: default deny sensitive data until group-scope authorization and per-org domain permission both pass.

Payroll ruleset stance:

- Payroll rulesets define governed inputs, eligibility, ownership, cost allocation, allowance readiness, pay schedule conventions, classification, approvals, masking, and release-gate state.
- They do not ship unreviewed payroll calculations.
- Payable outputs remain blocked until official rates, golden cases, and professional review gates pass.
- Host org/site can produce payable inputs or cost-allocation facts; home/legal employer remains payroll owner unless a reviewed workflow changes it.

Operational quirk stance:

- Site/cell quirks are configuration, not code forks.
- Examples: local shift day, safety evidence checklist, customer-site access window, reserve-equipment handoff, offline evidence rule, local approval routing.
- Quirks must be visible in preview, Work Hub action rails, audit, and simulation cases.

## 7. Cedar/PBAC evaluation contract

Every generated no-code object/action must produce a deterministic authorization contract.

### 7.1 Runtime invariants

- Default deny on missing attributes, unknown resource/action, stale bundle, unsupported condition, missing purpose, unresolvable relationship, or PDP adapter failure.
- RLS remains the isolation floor. Cedar/PBAC never substitutes for `with_org_conn`, `with_audit`, `app.current_org`, `console_rt` NOBYPASSRLS, or FORCE RLS.
- Runtime decisions evaluate capabilities, relationships, assignments, object attributes, action purpose, policy/ruleset version, and context — not role strings alone.
- Forbid wins for terminated/suspended users, stale policy versions, missing passkey step-up, out-of-scope target org, self-approval violations, and sensitivity-purpose mismatch.
- Simulation uses the same evaluator path as runtime, with hypothetical entities and draft bundle overlays.
- Policy versions, bundle digests, scope-precedence/conflict traces, and revoke/session/cache side effects are auditable.
- Generated bundles and simulation outputs must carry enough scope-precedence metadata to prove lower-scope department/site/cell/employee-exception/assignment rules cannot weaken locked system/legal, Group/HQ, or Org policy.

### 7.2 Decision inputs

Principal inputs:

- User/account credential state, passkey freshness, employment/person status, home org, current org, system/custom role inputs, department/team, position, responsibilities, worksite reach, group grants, delegations, active assignments, policy version.

Resource inputs:

- Object type/id, owner org, group membership if topology-related, department/team, worksite/cell, sensitivity class, lifecycle state, active ruleset/policy versions, assignment/projection/grant relationships, audit/revoke state.

Action inputs:

- Stable generated action keys such as `org.create`, `org.switch_context`, `employee.update`, `worksite_cell.assign_policy`, `ruleset.activate`, `assignment.revoke`, `approval_request.approve`, `simulation.run`, `audit_record.read`.

Context inputs:

- request id, trace id, current org, target org, optional group scope, scope-precedence trace, purpose, action intent, before/after diff summary, sensitivity, passkey age, time/shift/device/location state where policy uses it, simulation mode, policy version, bundle digest.

### 7.3 Evaluation flow for CRUD

1. Resolve principal from token plus live RLS-armed DB attributes.
2. Resolve target resource under the concrete target Org. For create, build proposed resource entity from validated request and parent scope.
3. Build authorization request: principal, action, resource, context, entity graph.
4. Evaluate Cedar/PBAC through in-process or controlled PDP seam.
5. If denied, return safe reason and write required decision audit for sensitive/high-risk actions.
6. If allowed, mutate through audited console API under `with_audit` / `with_org_conn` and the real target Org.
7. Write audit event with decision result, policy version, reason, actor, target, before/after digest, and revoke/cache/session side effects.

### 7.4 Evaluation flow for list/search

- First constrain by RLS tenant and explicit branch/worksite/department filters derived from the principal.
- Compile safe repository predicates only from reviewed finite relationship/scope attributes.
- Unsupported policy filter means deny/unsupported, not over-broad list.
- Sensitive fields can return object shells while requiring separate field/sensitivity read decisions.
- Audit aggregate metadata with safe result-count buckets and denied-sensitive-field counts.

### 7.5 Cross-org and HQ authorization

Group/HQ:

- GroupGrant gives group-level reach such as consolidated read, member management, context switch, or group finance read.
- It never directly grants tenant CRUD capability.
- Consolidated reads are N per-member real-Org RLS-armed reads plus Cedar/PBAC group/member checks.
- Cross-entity writes name exactly one target Org and run under that Org.

Cross-org worker:

- Worker in Org A acts in Org B only through explicit Assignment/Delegation/SharedServiceGrant/ApprovalRole entities.
- Each grant carries target Org/resource/scope, action families, purpose, sensitivity ceiling, effective/expiry time, approver, revoke id, policy version, and optional passkey/device/location constraints.
- Runtime arms the target Org, not the home Org and never the Group id.
- Audit records both home and target context, while the business mutation audit row belongs to the target Org.

## 8. End-to-end story required for future implementation evidence

Future implementation lanes must prove this story through browser/user-story evidence or a precise non-UI N/A rationale for slices that are not user-facing.

Scenario: COSS Group has COSS Manufacturing and KNL Logistics. An HQ/group admin configures the org model and temporarily assigns a COSS safety specialist to a KNL site/cell.

1. Signup.
   - New admin signs up and starts a company/group setup draft.
   - Expected evidence: account created, setup draft visible, no privileged publish before passkey.

2. Organization onboarding.
   - Admin creates Group/HQ, then creates/joins COSS Manufacturing and KNL Logistics as member Organizations.
   - Expected evidence: group management screen, member org rows, scope selector, real-org context switch.

3. Passkey setup.
   - Admin enrolls passkey and returns to the setup draft.
   - Expected evidence: credential active, step-up required for sensitive publish/approval actions.

4. Build org structure/editor model.
   - Admin creates departments/teams, positions, reporting lines, worksites/사업장 cells, employees, accounts, and assignments through CRUD screens.
   - Expected evidence: create/read/update/archive flows, validation states, object timelines, no raw UUID-first display, no import requirement.

5. Configure policy/rulesets.
   - Admin applies group-level locked guardrails, org defaults, site/cell local quirks, payroll-readiness context, approval rules, exception rules, and saved simulation cases.
   - Expected evidence: inheritance tree, before/after diff, conflict panel, locked upper-scope rule preventing contradictory lower rule, simulation outputs, audit preview.

6. Assign cross-org worker.
   - Admin starts CrossOrgAssignment draft for a COSS safety specialist to support a KNL worksite/cell.
   - Expected evidence: home/host projections, eligibility checks, approval route, host access diff, payroll owner/cost allocation labels, reporting exception, policy/ruleset trace.

7. Perform CRUD workflow.
   - Worker acts on assigned KNL work object within host scope: read assigned work item, update evidence/checklist, request/approve allowed operational transition as policy permits.
   - Expected evidence: allowed host actions succeed under KNL target Org; unrelated KNL customer/export/payroll actions deny; COSS home HR/payroll data remains hidden from KNL host users.

8. Audit/revoke/simulate.
   - Admin reviews audit trail, runs simulation for changed policy/ruleset, revokes or expires assignment, and confirms access/session/cache invalidation.
   - Expected evidence: decision audit, revoke audit, disabled host grants, removed Work Hub queue membership, post-revoke denied action, saved simulation showing reason path.

## 9. Proposed PR lanes for later approval

These lanes are proposals only. They should become implementation cards/PRs only after this plan is accepted and conflict/path ownership is assigned. Each lane must preserve real-Org RLS, audited console writes, no code-only policy switches, no import-first workflow, and browser/user-story proof when user-facing.

### Lane 0 — Plan approval and board guardrail settlement

Goal: treat this document as the plan gate before implementation.  
Scope: docs/Kanban only.  
Acceptance:

- Link this plan from root card `t_7e7a01e7`.
- Convert direction-shift audit findings into concrete downstream cards only after approval.
- Keep Workflow Studio, authz, HR/company, policy/payroll/reporting, and import/export cards gated behind this plan where they touch the no-code editor direction.

### Lane 1 — Org editor object vocabulary and action registry

Goal: define the stable object/action vocabulary for Group, Org, OrgUnit, WorksiteCell, Person, Employee, User, Position, ReportingLine, Assignment, SetupDraft, PolicyTemplate, RuleSet, Approval, Audit, Revoke, and Simulation.  
Expected proof: registry tests for unknown action/resource default-deny; docs trace from primitives to action keys.  
Non-goal: full UI or DB implementation of every object in one PR.

### Lane 2 — Setup Draft and CRUD-first shell

Goal: create the first product shell where admins can create, edit, validate, simulate, publish, rollback, and audit setup drafts over the core primitives.  
Expected proof: console_rt create/read/update/archive/simulate/publish tests for the selected narrow object subset; browser setup checklist story.  
Non-goal: import-first setup.

### Lane 3 — Group/HQ management and real-Org scope selector

Goal: extend group/HQ onboarding and scope selection around per-member armed reads and one-target-Org writes.  
Expected proof: group consolidated read fans out through real Orgs; group id cannot arm `app.current_org`; context switch is audited; group finance/sensitive reads require conjunctive permission.  
Non-goal: blanket multi-org BYPASSRLS view.

### Lane 4 — Cedar/PBAC PDP seam and generated policy bundle contract

Goal: introduce the authorizer seam, principal/resource/action/context shapes, context-switch action family, scope-precedence trace, policy bundle version/digest, and simulation overlay path behind existing behavior.  
Expected proof: default-deny tests for unknown/missing/stale contexts; audit for allow/deny decisions; generated bundles and simulations expose scope-precedence/conflict metadata; no behavior widening before activation.  
Non-goal: arbitrary tenant-defined runtime actions.

### Lane 5 — Policy/ruleset inheritance and simulation engine

Goal: implement inherited defaults, locked guardrails, override classes, conflict detection, saved simulation cases, and preview outputs for a narrow rule domain.  
Expected proof: upper-scope locked rule blocks contradictory lower site/cell override; additive/stricter-only behavior; simulation before activation equals runtime after activation for the supported case.  
Non-goal: production payroll calculation.

### Lane 6 — Worksite/cell local quirks and payroll-readiness context

Goal: model local operational quirks and payroll-readiness inputs on worksites/cells without code forks or unreviewed payroll math.  
Expected proof: site/cell local rules show in preview and action rails; payroll owner/cost allocation/allowance readiness is visible but payable output remains gated.  
Non-goal: enabling live payroll outputs without golden cases and professional review.

### Lane 7 — Cross-org assignment lifecycle and dual projections

Goal: implement the governed WorkAssignment lifecycle from draft/simulation through approvals, activation, operations, extension, completion, expiry, and revocation.  
Expected proof: home/host projections under their own real Orgs; host actions granted only by active assignment; home payroll/PII hidden from host; revoke invalidates access on next request/session refresh.  
Non-goal: employment transfer workflow.

### Lane 8 — Approval, Work Hub, and reporting exception integration

Goal: route setup/policy/assignment approvals and reporting-line exceptions into Work Hub queues with passkey step-up, segregation-of-duties checks, delegation, SLA/escalation, and audit.  
Expected proof: generated approval route, self-approval denial where policy forbids, passkey requirement for signing-equivalent approvals, reporting graph preview.  
Non-goal: generic workflow product detached from org/ops editor substrate.

### Lane 9 — Audit, revoke, rollback, observability, and cache/session hardening

Goal: make audit/revoke/version/cache/session behavior uniform for policy, role, assignment, setup draft, and access grants.  
Expected proof: policy_version/bundle_digest bump, deny stale bundle, revoke closes grants/sessions/cache, safe audit payloads, PII-safe metrics, rollback/supersede path.  
Non-goal: destructive audit deletion.

### Lane 10 — Browser E2E no-code org/ops story

Goal: prove the required end-to-end story: signup -> org onboarding -> passkey -> build org/editor model -> configure policy/rulesets -> assign cross-org worker -> perform CRUD workflow -> audit/revoke/simulate.  
Expected proof: Playwright/browser evidence, screenshots or traces where applicable, denied unauthorized flows, audit/revoke verification.  
Non-goal: API-only proof for user-facing behavior.

### Lane 11 — Secondary import/export bootstrap

Goal: provide import/export as a secondary migration/bootstrap lane after CRUD surfaces exist.  
Expected proof: mapping, validation, preview/dry-run, duplicate detection, idempotency, audit, rollback/error reporting, and parity with manual CRUD objects.  
Non-goal: spreadsheet as the primary management surface.

## 10. Board audit implications after plan approval

The focused Kanban audit identified 48 direction-relevant cards. After this plan is accepted, downstream board updates should use this mapping:

1. Workflow Studio cards become implementation substrate for the no-code org/operations editor, not a standalone generic automation product.
2. Authz/access-boundary cards add Cedar/PBAC acceptance: default-deny, generated action/resource/context registry, group/org/site policy precedence, cross-org grants, conflict detection, audit/revoke, and CRUD decisions.
3. HR/company/group CRUD cards add primitives: group/HQ, org, department/team, employee, role/position, reporting-line, worksite/cell, and cross-org assignment semantics.
4. Policy/payroll/reporting/export cards add optional site/cell policies, operational quirks, payroll ruleset ownership, inherited-vs-local override semantics, simulation, and PII-safe observability.
5. Import/export/Excel cards remain secondary bootstrap tooling behind database-backed CRUD/editor workflows.

No active implementation card should be reframed silently. Each affected card needs an explicit comment or child task with scope, non-goals, acceptance criteria, and verification path.

## 11. Acceptance mapping

| Original acceptance criterion | Plan coverage |
| --- | --- |
| 1. Define no-code editor primitives and UX flows for company/org/employee/site setup. | Sections 4 and 5 define primitives, relationships, setup drafts, CRUD/editor expectations, signup, onboarding, passkey, org structure, employee/account setup, policy/ruleset, and cross-org flows. |
| 2. Capture 점조직-style HQ/group management and cross-org worker access/work assignment. | Sections 3, 4, 5.2, 5.6, 7.5, 8, and Lane 7 define Group/HQ as topology, per-member armed reads, target-org writes, group guardrails, home/host projections, approvals, active host grants, and revocation. |
| 3. Model inherited defaults plus optional site/cell overrides for policy, quirks, and payroll rulesets. | Section 6 defines hierarchy, overlays, rule metadata, precedence, conflict behavior, payroll ruleset stance, and operational quirk stance; Lanes 5 and 6 translate this into proposed implementation lanes. |
| 4. Define how Cedar/PBAC evaluates generated org/ops logic for CRUD permissions. | Section 7 defines runtime invariants, decision inputs, CRUD/list/search/simulation flows, group/HQ authorization, cross-org grants, audit, default-deny, forbid-wins, and real-Org RLS coupling; Lane 4 proposes the PDP seam. |
| 5. Define E2E story from signup to audit/revoke/simulate. | Section 8 gives the full story with expected evidence for signup, org onboarding, passkey, org model creation, ruleset configuration, cross-org assignment, CRUD workflow, audit, revoke, and simulation; Lane 10 proposes the browser E2E PR lane. |

## 12. Approval and implementation guardrails

- This document is the coherent synthesized plan artifact. It is not a merge-ready implementation design and not authorization to begin all PR lanes at once.
- Future PR lanes should be small, reviewed independently, and sequenced so shared roots such as authz, RLS, audit, migrations, generated clients, Work Hub, and browser E2E do not race.
- Any user-facing lane must include browser/user-story evidence or a precise non-UI N/A rationale.
- Any tenant read/write lane must include real `console_rt` proof with `app.current_org` armed to a real Org and cross-tenant invisibility/fail-closed behavior.
- Any policy/ruleset/assignment/authz lane must include audit, revoke, rollback/supersede, observability, and cache/session invalidation expectations where applicable.
- No lane may propose Oyatie changes from this maintenance-plan work.
