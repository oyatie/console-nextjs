> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# MES future scope — manufacturing execution without becoming a manufacturing-only product

Date: 2026-06-29
Status: Future scope / backlog seed, not an implementation commitment.
Matrix row: `EP-017` in [`docs/benchmarks/enterprise-parity-matrix.md`](../benchmarks/enterprise-parity-matrix.md).

## Product stance

MES capability belongs in the long-term enterprise operations platform, but it is **not** the next
implementation lane. Build it only after the platform has durable foundations for:

- group / tenant / org / plant hierarchy and scope switching;
- people, positions, teams, departments, shifts, and labor capture;
- assets, inventory, materials, sites, and locations;
- ERP/procurement/finance source objects;
- configurable workflow, approvals, PBAC/RBAC/ABAC, audit, and passkey step-up;
- import/export with typed mapping, validation, dry run, lineage, and standardized output;
- analytics source-object lineage and observability.

The platform must stay domain-neutral: manufacturing/MES is one enterprise domain, just like HR, payroll,
ERP, logistics, maintenance, CX, and support. Non-manufacturing tenants should not see MES vocabulary unless
they enable it.

## Benchmark targets

Official benchmark sources to use during design and implementation:

- SAP Digital Manufacturing (<https://www.sap.com/products/scm/digital-manufacturing.html>) and SAP
  Digital Manufacturing help (<https://help.sap.com/docs/sap-digital-manufacturing>): production execution, resource
  orchestration, insights, and enterprise integration.
- Siemens Opcenter Execution (<https://www.siemens.com/en-us/products/opcenter/execution/>): MOM/MES
  execution patterns for work centers, quality, production tracking, and traceability.
- Rockwell Plex MES (<https://plex.rockwellautomation.com/en-us/products/manufacturing-execution-system.html>):
  cloud MES, production management, quality, inventory, and plant-floor visibility.
- Tulip Frontline Operations Platform (<https://tulip.co/platform/>): composable frontline apps, work
  instructions, data capture, and continuous improvement loops.
- Microsoft Dynamics 365 Supply Chain production floor execution
  (<https://learn.microsoft.com/en-us/dynamics365/supply-chain/supply-chain-dev/production-floor-execution-styles>):
  production-floor UI patterns, operator execution, jobs, and device/floor styling constraints.

Use official documentation/product pages first; third-party roundups are discovery material only.

## Capability matrix

| Capability area | Future capability | Best-in-class UX benchmark pattern | Required integration |
| --- | --- | --- | --- |
| Plant model | Plant, line, area, work center, station, resource, calendar, shift | Visual plant/work-center hierarchy, status chips, scope switcher | Org hierarchy, assets, HR shifts, policy scopes |
| Production orders | Release, dispatch, start, pause, complete, cancel, hold | Dispatch list prioritized by due date, constraint, material readiness, and exception state | ERP orders, inventory, workflow, approvals |
| Routings and operations | Routing, operation, step, standard time, setup/run/teardown | Operator sees only the current safe next step; supervisor sees bottlenecks and exceptions | Ontology schema, work instructions, audit |
| Work instructions | Versioned instructions, attachments, media, checklists, safety gates | Touch/scan-friendly guided execution with required evidence capture | Documents, files, training, policy, mobile |
| Material consumption | Issue/consume/backflush materials, substitutions, lot/serial capture | Barcode/QR-first material confirmation with mismatch blocking | Inventory, procurement, ERP, traceability |
| WIP and genealogy | Track WIP movement, lot/serial genealogy, rework loops | Drill from finished good to raw material lots and operators/actions | Ontology graph, quality, audit, recall workflow |
| Quality execution | In-process checks, inspections, holds, release, NC/CAPA | Quality hold/release queue with source step, defect, evidence, and decision | Quality module, approvals, mail/messenger, audit |
| Downtime and OEE | Availability/performance/quality events, reason codes, OEE | Real-time line board with downtime reason capture and supervisor escalation | Assets/EAM, work orders, analytics |
| Labor capture | Operator assignment, clock-in/out, activity time, skill qualification | Minimal operator input; exceptions and approvals handled by supervisors | HR, payroll, shifts, mobile passkey |
| Maintenance link | Equipment state, PM/CM work, line stop cause | Downtime event can create/attach maintenance work without context loss | EAM/assets, work orders, spare parts |
| Analytics | Yield, throughput, cycle time, bottlenecks, scrap/rework, OEE | Drillable dashboards that explain source data and assumptions | Semantic layer, Power BI/SAP Analytics-style views |
| Compliance/audit | E-signature, correction history, retention, release records | Signature-equivalent actions require passkey step-up and reason capture | Audit chain, policy, legal retention |

## UX requirements

- **Operator-first**: one-glance current job, current step, required material, required quality check, and
  next action. No wall-of-text explanations.
- **Supervisor exception handling**: line/area board must surface blocked, late, quality-held, material-short,
  downtime, and labor-short states before normal work.
- **Scan/touch-friendly**: barcode/QR-first material, lot/serial, equipment, and operator confirmations.
- **Object-linked collaboration**: messages, mail, files, support comments, approvals, and audit events attach
  to the production order, operation, quality hold, downtime event, or material lot.
- **Role/scope awareness**: operator, line lead, supervisor, planner, quality, maintenance, warehouse,
  HR/payroll, finance, plant manager, group executive, and platform admin each see a different default view.
- **Offline/degraded-mode plan**: production-floor network failures must have explicit safe behavior; do not
  silently accept writes that cannot be audited or reconciled.

## Data model backlog

Future entities should be typed ontology/workflow objects, not ad-hoc MES-only tables:

- `Plant`, `Area`, `Line`, `WorkCenter`, `Station`, `Resource`
- `ProductionOrder`, `ProductionLot`, `Routing`, `Operation`, `OperationStep`
- `WorkInstruction`, `InstructionVersion`, `SafetyGate`, `EvidenceRequirement`
- `Material`, `MaterialLot`, `Serial`, `InventoryMovement`, `ConsumptionEvent`
- `WipMove`, `OperationStart`, `OperationPause`, `OperationComplete`
- `QualityInspection`, `InspectionResult`, `Nonconformance`, `Disposition`, `CapaAction`
- `DowntimeEvent`, `DowntimeReason`, `ScrapReworkEvent`, `OeeFact`
- `LaborActivity`, `OperatorQualification`, `ShiftAssignment`

Every entity needs org/group scope, data classification, retention class, import/export mapping, audit
events, and policy hooks from day one.

## Workflow backlog

1. Planner releases production order from ERP demand.
2. Supervisor sequences work by material readiness, workforce, equipment state, and due date.
3. Operator starts operation with required materials, instructions, and safety/quality gates.
4. Operator records consumption, completion, defect, downtime, or rework.
5. Quality handles hold/release/nonconformance/CAPA with passkey step-up where signature-equivalent.
6. Maintenance work is created from downtime or abnormal equipment state without losing production context.
7. Inventory/ERP is updated through auditable integration events, not silent side effects.
8. Executives drill from OEE/yield/cost dashboards to source orders, lots, downtime, labor, and decisions.

## Acceptance gates before implementation

- A matrix row and user-story spec exist for each target persona.
- The ontology and workflow engines can express the MES objects and state transitions without custom one-off
  UI shortcuts.
- ERP, inventory, asset, HR/labor, quality, and audit contracts are versioned and stable.
- Sensitive decisions require passkey step-up and immutable audit evidence.
- Import/export mapper can ingest legacy production, material, quality, and equipment spreadsheets safely.
- E2E tests cover at least: operator start/complete, material mismatch block, quality hold/release,
  downtime-to-maintenance handoff, and executive OEE drilldown.

## Explicit non-goals for the next near-term wave

- Do not implement MES screens before group/org, people, assets/inventory, ERP, workflow, policy, and import
  foundations are production-grade.
- Do not add manufacturing vocabulary to the global Work Hub unless the tenant/group enables MES.
- Do not build an isolated demo MES module that lacks ERP/inventory/quality/audit integration.
- Do not use AI/ML/RL for scheduling, yield, downtime, or optimization until the mechanical source-object
  model, observability, and approval/write-back controls are trusted.
