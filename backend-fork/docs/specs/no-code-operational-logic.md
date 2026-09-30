> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Spec: No-Code Operational Logic, Policy Inheritance, and Payroll Rulesets

> **Status:** Planning spec for `NORTHSTAR-NOCODE-ORG-OPS-EDITOR-20260701` child task `t_388bf246`.
> This is a product/configuration contract, not an implementation design. It intentionally avoids table,
> endpoint, framework, or migration details. Follow-on PR lanes must turn the approved plan into code only
> after the synthesis plan is accepted.
>
> **Parent context:** `SPEC.md`, `docs/specs/knl-business-os.md`, `docs/specs/org-hierarchy.md`,
> `docs/specs/rbac-configurable.md`, `docs/specs/payroll.md`, `docs/specs/hr-payroll-readiness.md`, and
> `docs/ideas/enterprise-role-workflows.md`.

## 1. Objective

Create the planning contract for a no-code operational logic editor that lets non-technical group,
organization, HR/payroll, and site administrators configure how the company actually works:

- inherited policy templates and rulesets;
- defaults, local overrides, prohibited overrides, and time-bounded exceptions;
- eligibility checks for workers, workflows, sites, roles, and cross-org assignments;
- approval rules, escalation paths, segregation-of-duties controls, and passkey step-up requirements;
- operational quirks that differ by department/team or worksite/사업장 cell;
- payroll ruleset inputs and ownership boundaries without pretending regulated payroll is live before
  official rate tables, golden cases, and licensed professional review exist;
- simulation/preview so an admin can inspect the effective rule set for a worker, workflow, CRUD action,
  site, date, or assignment before saving.

The result must support a 점조직-style group/HQ that manages many corporations/orgs and worksites, while
preserving local legal-entity boundaries, site-specific nuance, and auditable least-privilege behavior.

## 2. Principles and non-goals

1. **CRUD-first, database-backed operations.** Rules exist to drive real create/read/update/delete and
   workflow actions on company, org, employee, site, policy, assignment, approval, and payroll-readiness
   objects. Upload/import can seed configuration later, but cannot be the primary management model.
2. **No-code does not mean unsafe.** Admins configure from templates, forms, matrices, previews, and
   governed workflows; they do not write code, SQL, or hidden scripts.
3. **Upper-scope guardrails can supersede lower rules.** Group/HQ and legal-entity/org policies may mark
   some rules mandatory, locked, or non-overridable so that a department, site, position, employee
   exception, or cross-org assignment cannot contradict them.
4. **Local cells may still be nuanced.** A worksite/사업장 cell can override or extend allowed defaults for
   its calendar, evidence checklist, shift rules, allowances, approval routing, and safety requirements
   when the upper scope explicitly permits the override.
5. **Payroll rulesets define governed inputs and eligibility, not unreviewed payroll math.** Any payable
   calculation remains behind the payroll release gate in `docs/specs/payroll.md` and
   `docs/specs/hr-payroll-readiness.md`.
6. **Policy evaluation is previewable and explainable.** The editor must show why a rule applies, which
   scope supplied it, whether it is inherited or local, what version is active, and what approvals/audit
   records would be created.
7. **Tenant/org isolation is not configurable.** A rule may grant scoped work access or cross-org
   assignment inside an approved group relationship, but it cannot weaken the hard org/RLS boundary or
   make payroll/financial data group-visible by default.

Non-goals for this planning child:

- no code, DB schema, endpoint, or UI implementation details;
- no final Cedar/PBAC syntax; the sibling authorization spec owns that mapping;
- no final cross-org assignment lifecycle; the sibling operations-workflow spec owns that lifecycle;
- no production payroll calculation approval.

## 3. No-code rule artifacts

The editor should expose a small vocabulary that business admins can understand and reuse.

| Artifact | Purpose | Admin-facing examples |
| --- | --- | --- |
| **Policy template** | Reusable bundle of defaults and guardrails for a group, org, department, site type, role, or workflow. | “HQ safety baseline”, “COSS production-line cell”, “KNL logistics dispatch desk”, “Payroll processor”. |
| **Ruleset** | Versioned set of rules for one domain: access, eligibility, approvals, operational quirks, payroll inputs, workflow behavior. | Site-access eligibility, overtime review, purchase approval, leave approval, wage-statement issuance readiness. |
| **Rule** | One condition/effect pair with scope, dates, override mode, and audit reason. | “Night-shift overtime requires site manager approval above 2 hours.” |
| **Default** | Inherited baseline value used when a lower scope does not provide an allowed override. | Org default pay schedule, default approval chain, default evidence checklist. |
| **Override** | Lower-scope replacement or extension of an explicitly overrideable default. | A worksite adds a safety photo requirement to the org-level completion checklist. |
| **Mandatory guardrail** | Upper-scope rule that lower scopes cannot weaken. | Group prohibits self-approval for payroll or purchase workflows. |
| **Exception** | Time-bounded, approved deviation from the effective rules. | Temporary role coverage while the normal approver is on leave. |
| **Eligibility check** | Rule determining whether a worker, role, site, or assignment may perform an action. | Active employment, required training, passkey freshness, site certification, payroll enrollment. |
| **Approval rule** | Required reviewers, thresholds, escalation, comments, evidence, and step-up auth. | Two approvers for high-value procurement; HR + host-site approval for cross-org assignment. |
| **Operational quirk** | Local behavior that changes a workflow without changing code. | Site-specific shift cutoff, customer access window, required equipment checklist, offline evidence rule. |
| **Payroll ruleset** | Legal-employer/payroll ownership, pay schedule, allowance eligibility, time classification, cost allocation, and release-gate status. | Hazard allowance at one site; host-org cost allocation for borrowed workers; wage statement blocked until validated. |
| **Simulation case** | Saved preview scenario used before activation and for regression checks. | “COSS hourly worker assigned to BESTEC night shift on a Sunday.” |

## 4. Attachment scopes

Rules attach to scopes, and the preview engine must show both the attachment point and the resulting
effective path.

| Scope | What can be configured there | What must not happen there |
| --- | --- | --- |
| **Group / HQ** | Group-wide mandatory guardrails, shared templates, cross-subsidiary baseline workflows, executive approval thresholds, group-managed policy versions, prohibited override list. | No blanket read/write of member org data; no payroll/finance visibility unless separate group-finance policy allows it. |
| **Corporation / Org / Legal entity** | Legal-employer defaults, org-specific HR/payroll readiness, local policy templates, org-wide approval chains, local role bundles, data-retention defaults. | Cannot opt out of group mandatory rules or hard legal/privacy/payroll gates. |
| **Department / Team** | Team queue behavior, manager chain, work-calendar defaults, approval routing, responsibility assignments, default work hub surface. | Cannot create cross-org visibility by itself; cannot weaken org/group guardrails. |
| **Worksite / 사업장 cell** | Site calendar, shift boundary, local safety/evidence requirements, customer-site rules, allowed allowances, local supervisors, access windows, equipment/asset quirks. | Cannot waive legal payroll gates, tenant isolation, passkey step-up for signing-equivalent actions, or mandatory safety/privacy rules. |
| **Role / Position** | Job-function bundle, signing authority, approval level, eligibility for tasks, queue ownership, required credentials/training. | Position/title alone does not become login authority without explicit policy assignment and preview. |
| **Employee / Worker** | Individual accommodations, employment-contract exceptions, temporary delegation, training/certification status, approved responsibility overrides. | No hidden privilege grant; every exception is time-bounded, approved, and auditable. |
| **Cross-org assignment** | Host org/site duties, allowed actions, host supervisor, source/home owner, duration, cost allocation, host-site rules, revocation conditions. | Does not transfer legal employer/payroll ownership by default; does not expose home-org sensitive payroll data to the host. |
| **Workflow / Object / Action context** | State-specific requirements, object sensitivity, approval state, evidence requirement, import/export restrictions, signing-equivalent step-up. | UI cannot bypass server policy by hiding or showing buttons only. |

## 5. Inheritance model

The editor computes an **effective ruleset** from a hierarchy plus overlays.

Primary hierarchy:

```text
Group / HQ
  -> Corporation / Org / Legal Entity
    -> Department / Team
      -> Worksite / 사업장 cell
```

Orthogonal overlays:

```text
Role / Position
+ Responsibility assignment
+ Employee-specific exception
+ Cross-org assignment overlay
+ Workflow/object/action context
+ Date/time/shift/environment context
```

Each rule declares:

- scope owner: where the rule is attached;
- domain: access, eligibility, approval, operational quirk, payroll readiness, audit, privacy, workflow;
- effect type: allow, deny/prohibit, require approval, require evidence, set default, set minimum, set
  maximum, add checklist item, add payroll flag, require passkey step-up, require review;
- override mode: locked, overrideable, additive, replaceable, stricter-only, expires, or exception-only;
- effective dates and version;
- required approvals for changing the rule;
- reason shown to admins and auditors.

The effective-rule builder should use these semantics:

1. **Hard system/legal guardrails win first.** Tenant isolation, privacy, payroll release gate, audit,
   passkey step-up for signing-equivalent actions, and professional-validation blockers cannot be
   weakened by any no-code rule.
2. **Locked group/HQ guardrails win next.** Example: group prohibits payroll self-approval and requires
   policy-preview evidence for all cross-org worker assignments.
3. **Locked org/legal-entity rules win inside that org.** Example: the COSS legal entity requires HR
   approval before any worker is assigned to another subsidiary.
4. **Overrideable defaults flow downward.** A group template may provide a baseline; org, department, or
   site can override only fields explicitly marked overrideable.
5. **Additive rules accumulate when safe.** Evidence checklist items, notification recipients, audit tags,
   and training requirements normally merge rather than replace.
6. **Most-specific wins only for replaceable defaults.** Site-specific shift cutoff can replace org default
   cutoff only if the org template says that field is replaceable at site scope.
7. **Stricter restriction wins for safety/security/payroll.** If two scopes disagree about review,
   masking, approval, or eligibility, the stricter requirement applies unless an approved exception exists.
8. **Cross-org assignments are conjunctive.** The worker must satisfy home-org constraints, host-org/site
   constraints, assignment-specific constraints, and workflow/action constraints. One pass is not enough;
   every relevant scope must allow the assignment/action.
9. **Exceptions are explicit overlays, not silent edits.** Exceptions must be time-bounded, approved,
   reasoned, visible in preview, and automatically expire or route for renewal.

## 6. Override and conflict-resolution semantics

### 6.1 Override classes

| Class | Meaning | Example behavior |
| --- | --- | --- |
| **Locked / non-overridable** | Lower scopes cannot change or weaken the rule. | Group requires passkey step-up for payroll approval; site UI shows the control as locked. |
| **Overrideable default** | Lower scope may replace the inherited value. | Org default shift day is midnight; a 24-hour worksite sets shift day to 06:00. |
| **Additive** | Lower scope may add requirements but not remove inherited ones. | Site adds photo evidence to the org completion checklist. |
| **Stricter-only** | Lower scope may make the rule more restrictive, never looser. | Department can require two approvers where org requires one; cannot reduce to zero. |
| **Exception-only** | Changes require a time-bounded exception workflow. | Temporary approval by backup manager during leave. |
| **Prohibited** | The rule or value cannot be configured at that scope. | Site cannot define an unvalidated statutory payroll formula. |

### 6.2 Conflict types and required behavior

| Conflict | Required behavior |
| --- | --- |
| Lower rule contradicts locked group/org guardrail. | Block activation; show the inherited locked rule, owner, reason, and who can propose an upper-scope change. |
| Two overrideable defaults apply at the same specificity. | Block activation until admin selects one winner or narrows conditions. |
| Lower scope tries to weaken safety, privacy, payroll, audit, or passkey requirement. | Apply stricter inherited rule and flag the attempted weakening as invalid. |
| Numeric threshold conflicts. | Use explicit min/max semantics: for approval thresholds, the stricter/lower approval threshold or higher review requirement wins; for allowances, legal floor/ceiling and org payroll policy define the allowed range. |
| Eligibility conflicts. | Deny until all required eligibility checks pass or an approved exception overlays the failure. |
| Payroll ownership conflict. | Keep legal employer/home-org payroll owner unless a professionally reviewed employment-transfer workflow changes ownership. Host org may receive cost allocation, not payroll authority by default. |
| Effective-date overlap conflict. | Block activation if two incompatible rules cover the same worker/site/action/time interval. |
| Cross-org group visibility conflict. | Default deny sensitive data; require both group-scope authorization and per-org domain permission before any consolidated view. |

The editor must never silently choose a surprising winner. Every resolved or blocked conflict needs a
human-readable reason and a path to fix it.

## 7. Rule domains

### 7.1 Eligibility checks

Eligibility rules answer: “May this subject perform this workflow/action for this object at this time?”

Required eligibility dimensions:

- employment/person status: pending setup, active, on leave, suspended, terminated, retired;
- account/credential status: passkey enrolled, session fresh, required agreements complete;
- job function, role, position, and responsibility assignment;
- department/team membership and reporting chain;
- scope: group, org, department, team, worksite/cell, object, self;
- site requirements: training, certification, PPE, safety briefing, customer access window;
- legal/privacy purpose tag for HR, payroll, location, wage, retirement, and sensitive fields;
- cross-org assignment status, home owner, host owner, duration, and revocation state;
- workflow/object state: draft, pending approval, active, blocked, approved, issued, void, closed.

Eligibility is fail-closed. Missing source facts should produce a preview warning and block activation or
execution when the missing fact is safety-, payroll-, privacy-, or authorization-critical.

### 7.2 Approval rules

Approval rules define who must approve, in what order, with what evidence, and when escalation happens.
They must support:

- serial, parallel, and quorum approval chains;
- threshold-based approval by money, payroll sensitivity, employee count, site risk, overtime hours,
  asset criticality, or workflow type;
- segregation of duties and self-approval prevention;
- substitute approver rules during leave or vacancy;
- passkey step-up for signing-equivalent approvals;
- mandatory comment/evidence requirements;
- escalation when SLA or deadline is missed;
- revocation/reopen behavior when a material upstream fact changes.

### 7.3 Operational quirks

Operational quirks are local configuration, not forks in code. Examples:

- a worksite requires entry before 07:30 and marks late arrival differently from the org default;
- a customer site requires a photo of a safety board before work starts;
- a manufacturing cell treats the shift day as 06:00-to-05:59 for attendance grouping;
- a logistics site requires a reserve-equipment handoff checklist before closing a work order;
- a department requires manager review for any assignment outside its normal site;
- a field team permits offline evidence capture but requires sync before approval.

Quirks must be discoverable in preview and reflected in the Work Hub/Object Action rail so users know why
an action is available, disabled, or awaiting evidence.

### 7.4 Payroll rulesets

Payroll rulesets define how payroll-relevant facts are owned, classified, and reviewed. They should not
claim production calculation authority until the payroll release gate is satisfied.

A payroll ruleset can configure:

- legal employer / payroll owner for a worker or assignment;
- pay schedule and pay period conventions;
- attendance source priority and cutoff dates;
- time classification: regular, overtime, night, holiday, leave, unpaid, training, travel, standby;
- allowance eligibility: site allowance, hazard allowance, meal/transport allowance, shift differential;
- cost allocation: which org/site/project bears cost when a worker serves another org;
- approval path for payroll-affecting changes;
- professional-validation status and official source/golden-case blockers;
- wage-statement readiness, issuance workflow, masking, retention, and purpose tags.

Payroll ruleset conflict stance:

- statutory/legal gates, official rates, NTS withholding rows, golden cases, and professional review are
  locked upper guardrails;
- a site may add an allowance eligibility rule only if the org payroll owner permits that allowance type;
- host-site rules can produce payable inputs or cost-allocation facts, but home/legal-employer payroll
  ownership remains unless an approved employment-transfer workflow changes it;
- any payable output remains blocked when source tables, golden cases, or professional review are missing.

### 7.5 Exceptions and revocation

Exception rules are allowed, but must be safe:

- named owner and approver;
- reason, start/end, impacted workers/sites/actions, and sensitivity class;
- before/after preview;
- audit record on approval, activation, use, expiration, revocation, and renewal;
- automatic expiry or review queue;
- revocation path that recalculates effective rules and removes future access/eligibility without deleting
  historical audit or payroll records.

## 8. No-code configuration UX

The editor should feel like a guided admin product, not a database editor.

1. **Template catalog.** Admin chooses a starting template such as “HQ safety baseline”, “Org HR/payroll
   readiness”, “Manufacturing worksite”, “Dispatch office”, or “Cross-org temporary assignment”.
2. **Scope selector.** Admin chooses where the template/rule applies: group, org, department/team,
   worksite/cell, role/position, employee, cross-org assignment, workflow/action.
3. **Rule builder.** Forms and matrices let the admin define conditions, effects, evidence, approvers,
   effective dates, override mode, and exceptions in business terms.
4. **Inheritance tree.** UI shows inherited group/org/team/site rules, locked rules, local overrides,
   and conflicts in one tree.
5. **Before/after diff.** For every draft, show affected workers, roles, sites, workflows, approvals,
   payroll-readiness flags, CRUD actions, and revoked grants before activation.
6. **Simulation cases.** Admin can run sample workers/workflows/sites/dates through the draft before
   saving. Required scenarios can be saved with the policy version.
7. **Conflict panel.** Critical conflicts block activation; warnings require acknowledgement and audit
   comment; informational differences stay visible.
8. **Approval workflow.** Sensitive policy/payroll/scope changes route to required approvers with passkey
   step-up where applicable.
9. **Version activation.** Draft -> preview -> approve -> activate -> monitor -> rollback/retire. Every
   runtime decision logs policy/ruleset version and reason.
10. **Rollback and sunset.** Admin can retire or roll back a version without deleting history. Expiring
    rules produce action items before they fail operational workflows.
11. **Audit and observability.** The product should expose policy/ruleset activation health, conflict
    blocks, simulation failures, exception expirations, rollback events, and stale-rule warnings as
    operations-facing counters and timelines, while keeping worker names, wage values, resident identifiers,
    bank details, and other sensitive facts out of metric labels or routine audit summaries.

## 9. Simulation and preview requirements

Simulation is mandatory for this editor. Before saving or activating a rule version, admins must be able
to answer: “What will happen to this worker, workflow, site, and CRUD action if I activate this?”

### 9.1 Scenario inputs

A preview must allow selection or construction of:

- actor/worker: employee, role/position, department/team, home org, employment state, credential state;
- target resource: org, department, site/cell, employee, policy template, ruleset, approval, payroll run
  input, work item, object/action;
- action: create, read, update, delete, assign, approve, revoke, import, export, issue, close;
- context: date/time, shift, site, device/passkey freshness, purpose tag, cross-org assignment, workflow
  state, sensitivity class;
- draft version versus currently active version.

### 9.2 Preview outputs

The preview must show:

- effective decision: allowed, denied, requires approval, requires evidence, blocked by payroll/legal gate,
  or requires exception;
- inherited rule path by scope: group -> org -> department/team -> site/cell plus role/employee/assignment
  overlays;
- which rules are locked, overrideable, additive, stricter-only, or exception-only;
- conflicts and blockers with owner/resolution path;
- approvals required, approver candidates, step-up requirements, evidence/comment requirements;
- payroll-readiness effects: payroll owner, allowance eligibility, time classification, cost allocation,
  source/golden-case blockers, masking requirements;
- CRUD impact: which create/read/update/delete actions would change for the selected actor/resource;
- affected population summary: counts of workers/sites/workflows impacted, with sensitive values masked;
- audit event preview: what would be recorded on activation and later use;
- rollback impact: what changes if the draft is rolled back.

### 9.3 Saved regression scenarios

Each active ruleset version should carry representative simulation cases, for example:

- normal employee at home org/site;
- worksite-local manager;
- HR/payroll specialist viewing sensitive facts with a purpose tag;
- cross-org temporary worker at host site;
- site-specific night shift;
- revoked/expired assignment;
- pending setup user without passkey;
- attempted self-approval;
- payroll run with missing official source/golden case.

These cases become the acceptance examples for future implementation and review lanes.

## 10. Worked examples

### 10.1 Site-specific operational quirk: manufacturing night cell

- **Group/HQ locked rule:** signing-equivalent approvals require passkey step-up; safety-critical work
  cannot be completed without required evidence.
- **Org default:** COSS production sites use a standard shift calendar and require supervisor review for
  night-shift exceptions.
- **Worksite/cell override:** BESTEC Line A defines shift day as 06:00-to-05:59 and adds a machine lockout
  checklist for night maintenance.
- **Result:** preview shows the group passkey rule and org review rule as inherited; the site-specific
  shift cutoff and checklist apply because the org template marked those fields overrideable/additive.
- **Conflict case:** the site tries to remove passkey step-up for night-shift exception approval. The editor
  blocks activation because the group rule is locked.

### 10.2 Payroll ruleset: borrowed worker with site allowance

- **Home org:** KNL remains the worker's legal employer and payroll owner.
- **Host org/site:** COSS assigns the worker to a manufacturing cell for two weeks and marks the site
  allowance as eligible only when the worker completes safety briefing and records attendance at the host
  site.
- **Ruleset outcome:** host site can create cost-allocation and allowance-eligibility facts; KNL payroll
  owner reviews and approves whether those facts enter a payroll draft.
- **Preview:** shows home payroll owner, host supervisor, allowance eligibility, cost allocation, required
  approvals, and payroll-release blockers.
- **Conflict case:** host org tries to issue wage statement directly. Denied unless an approved payroll
  ownership/processor policy grants that authority and payroll release gates pass.

### 10.3 Department approval threshold with upper guardrail

- **Group rule:** purchases above a group-defined risk threshold require executive approval and cannot be
  self-approved.
- **Org rule:** BESTEC requires finance review above a lower local threshold.
- **Department rule:** production office wants to auto-approve consumables below a smaller amount.
- **Result:** the department auto-approval is allowed only below both the org finance threshold and group
  executive threshold; self-approval remains prohibited at every amount.
- **Preview:** shows which threshold fired, who approves, and whether the actor is disqualified by
  segregation-of-duties.

### 10.4 Employee exception: temporary backup approver

- **Org default:** HR manager approves leave usage-promotion notices.
- **Employee exception:** backup HR lead may approve for one week while manager is on leave.
- **Required controls:** time-bounded exception, reason, approver, passkey step-up, affected workflow list,
  and automatic expiry.
- **Preview:** normal approval path and backup approval path are both shown; after expiry, the backup loses
  authority without deleting historical approvals.

### 10.5 Cross-org assignment: staffing agency worker at KNL site

- **Home org:** staffing/legal-employer org owns employment and payroll record.
- **Host org/site:** KNL site owner requests access for a specific worksite/cell and task type.
- **Approvals:** home HR confirms employment/contract eligibility; host site owner confirms site fit;
  group/HQ policy may require additional review for sensitive sites.
- **Rules applied:** home payroll ownership, host site safety checklist, host work queue access, limited
  object visibility, expiry date, revocation on contract end.
- **Preview:** worker can read/update only assigned host-site work items, cannot view host payroll/finance,
  and cannot retain access after revocation or end date.

## 11. Acceptance checklist for this child spec

- **Inheritance model:** Defined in §§4-6 with scope hierarchy, overlays, rule metadata, precedence, and
  conflict behavior.
- **Override semantics:** Defined in §6, including locked/non-overridable, overrideable, additive,
  stricter-only, exception-only, and prohibited rules.
- **Site-specific quirks and payroll examples:** Covered in §§7.3, 7.4, and all worked examples in §10.
- **Rule attachment scopes:** Group/HQ, corporation/org, department/team, worksite/사업장 cell,
  role/position, employee, cross-org assignment, and workflow/action scopes are all named in §4.
- **Simulation/preview:** Detailed in §9 with inputs, outputs, saved regression scenarios, conflict panels,
  audit preview, and before/after diffs.
- **No-code configuration approach:** Described in §8 through template catalog, scope selector, rule
  builder, inheritance tree, diff, simulation, conflict panel, approval workflow, activation, and rollback.
- **No implementation dependency:** The spec is intentionally product-level. It gives enough behavior for
  the synthesis/planning lane and later PR decomposition without committing to code structure.
