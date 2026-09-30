> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Spec: Operations Intelligence & Decision Management

> **Status:** DESIGN — product/architecture guardrail. This spec makes business intelligence a
> governed operating-system layer, not a collection of charts. It is linked from
> `docs/specs/knl-business-os.md`, `docs/specs/roadmap-to-production.md`,
> `docs/specs/rbac-configurable.md`, and `docs/specs/korean-legal-boundaries.md`.
>
> **Benchmark anchors:** Palantir Foundry Ontology/Actions/Object Explorer/Workshop, SAP Fiori
> object/list patterns, ServiceNow workflow/task surfaces, IBM Maximo and Dynamics 365 asset/work-order
> models. URLs are tracked in `docs/ideas/enterprise-role-workflows.md`.

## Objective

Build an **overall intelligence business management layer** that converts operational history into
explainable, policy-governed recommendations for future decisions: sell/keep/repair assets, rental
rate floors, target margins, preventive-maintenance windows/cycles, manpower plans, reserve equipment,
inventory and parts policies, procurement/bid strategy, and workflow/approval bottleneck reduction.

The system must answer questions such as:

- Should this equipment be sold, kept, repaired, overhauled, or replaced based on projected maintenance
  cost, downtime risk, market value, utilization, margin, and replacement lead time?
- What staffing, skill mix, spare equipment, inventory, and parts levels are needed to maintain a target
  service level objective (SLO) given historical demand, failure rates, lead times, travel times, absence,
  overtime, and vendor performance?
- What rental rate or bid price protects margin after expected maintenance, depreciation, financing,
  insurance, utilization, seasonality, SLA penalties, and customer/site risk?
- Which procurement/vendor choice is best when price, lead time, quality, failure rate, warranty, cash
  timing, and operational risk are considered together?

## Non-negotiable product rules

1. **Recommendation, not silent automation.** Intelligence outputs are drafts/scenarios until routed
   through workflow, policy, approval, and passkey step-up where needed.
2. **Object-centric evidence.** Every recommendation links to the objects and events that produced it:
   asset, work order, part, vendor, customer/site, employee/skill, contract/SLO, approval, purchase,
   invoice, rental, bid, payroll/work-hour aggregate, and outcome.
3. **Probabilistic, not fake certainty.** Forecasts expose distributions, confidence bands, assumptions,
   comparable cases, and calibration quality. Show P50/P90/P95 where operationally meaningful.
4. **Purpose and sensitivity boundaries.** HR, payroll, location, wage, retirement, and private personnel
   fields may inform only authorized, purpose-bound, minimized analytics. Generic dashboards must not
   leak sensitive facts.
5. **No model without lineage.** Every model/recommendation stores input snapshot, feature set,
   model/version, policy version, generated timestamp, actor/requester, and later outcome/variance.
6. **No optimization against illegal or unsafe objectives.** Labor law, privacy, safety, contractual SLOs,
   maintenance safety limits, segregation of duties, and approval policy are hard constraints, not soft
   costs.
7. **Human override is governed.** Users may override recommendations, but must record reason/evidence;
   overrides become training/outcome events.
8. **Mechanical and algorithmic foundations first.** Deterministic rules, ledgers, lifecycle state,
   forecasting math, scenario simulation, and observability must be reliable before adding AI/ML/RL/LLM.
   Artificial intelligence is a final-stage augmentation layer, never a substitute for missing product
   primitives or missing data discipline.

## Ontology additions

Add these generic object types over the existing operational primitives. They should be configurable by
ontology later, but the first implementation can ship opinionated system types.

| Object type | Purpose | Key links |
| --- | --- | --- |
| `ServiceLevelObjective` | Target response time, uptime, fill rate, maintenance window, SLA penalty, or internal SLO. | Contract, customer/site, asset class, work queue, department/team. |
| `AssetLifecycleDecision` | Sell/keep/repair/replace/overhaul recommendation and decision record. | Asset/equipment, work orders, cost ledger, utilization, market-value snapshot, approval. |
| `MaintenanceForecast` | Probabilistic future maintenance cost/downtime/failure forecast. | Asset, model/spec/manufacturer, maintenance cycle, parts, technician skill, historic outcomes. |
| `CapacityPlan` | Staffing/equipment/reserve plan required to hit a target SLO. | SLO, employees/skills, shifts/calendars, equipment fleet, demand forecast, region/site. |
| `InventoryPolicy` | Min/max/reorder/safety stock policy for parts and other inventory. | Inventory item, vendor, lead-time history, work-order consumption, criticality, cost. |
| `PricingScenario` | Rental/bid/quote price options with margin and risk explanation. | Customer/site, asset class, utilization, costs, SLA, market/rate history, approval. |
| `ProcurementScenario` | Vendor/quote/bid evaluation with risk, lead time, and quality history. | Purchase request, vendor, quote, part/asset/item, budget, receiving/outcome. |
| `ForecastModelVersion` | Versioned model/configuration/assumption bundle. | Feature snapshot, training window, calibration report, owner, approval status. |
| `Recommendation` | Generic recommendation envelope for any domain. | Target object/action, model version, assumptions, confidence, policy path, final decision. |
| `OutcomeVariance` | Actual-vs-expected measurement after execution. | Recommendation, decision, work/order/purchase/rental outcome, metric deltas. |
| `ExperimentRun` | Offline/simulation experiment for algorithms, ML, RL, or LLM-assisted workflows. | Model version, training/eval data snapshot, metrics, approval, rollout state. |

## Required event history

Operational history must be structured enough that future intelligence does not scrape free-text notes.
For every workflow in 기안, 구매, 승인, 입찰, pricing, planning, maintenance, rental, HR/payroll, and asset
lifecycle, capture:

- intent/object context: why the request exists, target object(s), department/team, customer/site,
  contract/SLO, sensitivity class;
- alternatives considered: vendor quotes, price options, repair/replace choices, staffing/reserve
  options, maintenance windows;
- decision path: approvers, policy version, passkey step-up, memo/evidence, delegation, rejections,
  exceptions, self-approval prevention;
- operational inputs: cost, expected downtime, utilization, market value, inventory, lead time, labor
  capacity, skills, calendar/shift, weather/route when legally collected, customer/site constraints;
- outcome and variance: final cost, realized downtime, SLA hit/miss, margin, vendor quality, asset
  reliability, customer response, overtime, stockout/fill rate, delay reasons;
- privacy/legal controls: purpose tag, retention, masking, viewer permission, legal basis/consent where
  relevant.

## Decision domains and nuances

### Asset sell/keep/repair/replace

Decision inputs must include at least:

- trailing maintenance cost and downtime by asset/model/spec/manufacturer/cohort;
- projected maintenance cost distribution over the decision horizon;
- probability of failure, expected downtime, safety-critical defect history, and maintenance-window fit;
- utilization, revenue/margin contribution, customer/site criticality, and substitution availability;
- current market/resale value, expected depreciation, replacement cost, financing/lease cost, lead time;
- parts availability, vendor support, warranty, technician skill availability;
- accounting/tax/legal review markers where disposal/acquisition materially affects books.

Recommendation outputs:

- expected total cost of ownership, downtime risk, margin impact, and cash impact by scenario;
- P50/P90/P95 risk bands for maintenance cost and downtime;
- suggested action: keep, preventive overhaul, repair, replace, sell, retire, or hold pending data;
- required workflow path and approvers based on value, risk, policy, and segregation of duties;
- comparable historical cases and explanation of why they are/are not comparable.

### Rental rate, bid price, and margin

Pricing must treat the rate as a risk-adjusted business decision, not a flat table lookup.

Inputs:

- asset class, model, age, utilization, depreciation, financing/insurance/storage costs;
- expected maintenance and downtime distribution; reserve/substitution cost;
- customer/site segment, geography, seasonality, contract duration, SLA/penalty terms;
- historical win/loss, competitor/market snapshots if legitimately sourced, collection risk, payment
  timing, customer quality, and delivery/operation constraints;
- target gross margin/contribution margin and group/org policy thresholds.

Outputs:

- price floor, target price, and risk-premium range;
- expected margin distribution and break-even utilization;
- SLA/capacity impact: whether accepting the contract threatens fleet/manpower/parts SLOs;
- workflow requirement for discounts, margin exceptions, unusual payment terms, or high-risk customers.

### Manpower, equipment reserve, inventory, and parts SLO planning

SLO planning is a stochastic capacity problem. It must consider demand volatility, failure probability,
lead-time uncertainty, skills, calendars, and legal labor constraints.

Inputs:

- target SLOs: response time, completion time, uptime, stockout/fill rate, maintenance window, backlog
  age, customer priority;
- historical demand by site/customer/asset class/season/weekday/shift;
- workforce availability, skills/certifications, shift calendars, absence, overtime, legal constraints,
  travel time, productivity variance;
- fleet/equipment availability, preventive maintenance schedule, reserve/substitution pool;
- parts inventory, consumption, criticality, lead time distribution, vendor reliability, shelf life,
  holding cost, stockout cost;
- dependencies between parts, equipment, vendors, technicians, sites, and approvals.

Outputs:

- required headcount/skill mix by period/site/department to satisfy the SLO with target confidence;
- reserve equipment count and placement recommendations;
- reorder point, safety stock, max stock, and substitute-item policy;
- maintenance windows/cycles that minimize SLO risk and cost while respecting safety/manufacturer rules;
- tradeoff frontier: cost vs SLO probability vs overtime vs inventory holding cost;
- exception tasks when current plan cannot meet target SLO.

### Purchasing, procurement, and bids

Purchasing must preserve more than the approved PO. It should learn from quotes, bids, approvals,
vendor performance, receiving quality, payment timing, and later operational outcomes.

Inputs:

- past quotes by item/vendor/spec, lead time, defect/return/warranty rate, delivery reliability;
- budget availability, cash timing, approval thresholds, preferred/blocked vendor status;
- required evidence, segregation of duties, self-approval restrictions, and conflict-of-interest flags;
- downstream effect on SLO, inventory, asset lifecycle, margin, and customer commitments.

Outputs:

- recommended vendor/quote with rationale, alternatives, and risk;
- expected total landed/holding/stockout cost, not just unit price;
- approval workflow path, required evidence, and policy exceptions;
- post-receipt outcome variance to improve future procurement decisions.

### HR, payroll, and labor operations

HR/payroll intelligence must be conservative and legally bounded.

- Payroll calculations are deterministic/legal-rule outputs with effective-dated rates and golden tests,
  not opaque optimization.
- Workforce planning may use aggregated availability, skills, shift, overtime, and absence data, but
  sensitive payroll/wage/health/retirement facts require domain permission and masking.
- Recommendations cannot pressure illegal overtime, evade leave/retirement obligations, or discriminate.
- Employment transitions, intra-group moves, retirement settlement, and payroll receipt issuance remain
  governed workflows with audit and passkey step-up.

## API and data-contract principles

- Use append-only event/ledger tables for facts that train or evaluate recommendations; corrections are
  new events, not destructive rewrites.
- Recommendation APIs return structured envelopes: `recommendationId`, `targetObject`, `proposedAction`,
  `modelVersion`, `inputSnapshotId`, `assumptions`, `confidence`, `sensitivityClass`, `policyPath`,
  `comparableCases`, `tradeoffs`, `createdAt`, `expiresAt`.
- Scenario write-back creates a workflow draft/request; it must not mutate the target object directly.
- List APIs are paginated, filterable by object type/status/sensitivity/scope/modelVersion, and default
  to current actionable recommendations, with history available explicitly.
- Import/export mapping must preserve source columns and classify whether each field is allowed for
  analytics, operational use, payroll/HR use, or masked-only use.

## UI/UX requirements

1. **Executive/manager command center:** exceptions, SLO risk, margin risk, capacity gaps, major pending
   approvals, and scenario tasks before vanity charts.
2. **Object-page intelligence rail:** every asset/customer/vendor/employee/site/work-order object shows
   relevant forecasts, recommendations, comparable cases, and outcome history with policy-gated actions.
3. **Scenario workbench:** compare sell/keep/repair/replace, staffing levels, reserve parts, pricing,
   procurement options, or maintenance windows side by side.
4. **Explainability panel:** show source data freshness, assumptions, confidence bands, comparable cases,
   constraints, and what data is missing.
5. **Workflow conversion:** one button creates 기안/구매/승인/입찰/계획 draft with all recommendation evidence
   attached. The draft then follows normal policy/workflow.
6. **Data-quality warnings:** recommendations degrade visibly when workbook imports, asset history, costs,
   parts consumption, or outcome data are missing or stale.
7. **Sensitive-data masking:** payroll/HR/location-derived insights are aggregated/masked unless the user
   has explicit domain permission and purpose.
8. **Maturity before feature explosion:** the core UI must be stable enough that new domains reuse proven
   patterns instead of inventing new page shapes. The minimum gate is Work Hub queue, Object Explorer
   list/detail, approval/workflow action rail, effective-policy explanation, scenario workbench, and
   accessible dense tables/cards that pass tests and visual review for the primary roles.

### First shipped deterministic UI foundation (G012)

The first production UI slice is `/intelligence`, an executive/manager scenario workbench. It does **not**
claim autonomous optimization. It gives leaders one governed entry point for:

- rental pricing and bid-margin review;
- asset sell/keep/repair/replace review;
- reserve equipment, inventory, and parts policy review;
- workforce, skill, and SLO capacity review;
- procurement/vendor/bid scenario review;
- maintenance-cycle and preventive-window review;
- data-quality and governance triage;
- MES readiness and AI/ML/RL/LLM readiness.

Each domain card links to source-object evidence and to the workflow/policy surface that would turn a
scenario into a draft approval/request. This keeps G012 honest: deterministic source-object navigation,
policy guardrails, and workflow conversion ship now; probabilistic models, MES execution, and AI-assisted
recommendations remain gated by the foundations below.

## Observability requirements for intelligence

AI, ML, RL, and advanced analytics are only useful if the platform already measures the business
mechanically. Each intelligence feature must define the operational questions it answers and emit
structured telemetry for:

- data freshness, completeness, and source quality by object type and tenant/org scope;
- recommendation generation count, latency, failure rate, expiry, and accepted/rejected/overridden rate;
- forecast calibration: predicted vs actual cost, downtime, SLO hit/miss, margin, lead time, stockout,
  utilization, and workforce capacity;
- workflow conversion: recommendation -> draft -> approval -> execution -> outcome;
- policy/security events: sensitivity class, masking mode, RLS scope, policy version, passkey step-up,
  and denied/blocked write-back attempts;
- model drift and stale-data alerts with bounded-cardinality labels, never raw PII or unbounded object IDs
  as metric labels.

On-call questions the telemetry must answer:

1. Are recommendations currently being generated from fresh, complete, legally usable data?
2. Which model/version or deterministic rule produced a bad recommendation, and with what input snapshot?
3. Are users accepting, rejecting, or overriding recommendations, and why?
4. Did a recommendation improve or harm SLO, margin, downtime, inventory, staffing, or procurement
   outcomes after execution?
5. Is any sensitive HR/payroll/location data flowing into a surface where the actor lacks purpose and
   permission?

## Final-stage AI/ML/RL/LLM augmentation

This is **not immediate scope**. It is recorded so the architecture does not block it later, and so
tickets asking for premature AI can be rejected until prerequisites are met.

Permitted final-stage uses after mechanical foundations are production-grade:

- **Machine learning / probabilistic forecasting:** failure probability, maintenance cost distribution,
  demand forecast, lead-time distribution, stockout risk, workforce capacity risk, customer/site risk,
  and pricing/margin variance.
- **Reinforcement learning / optimization:** offline simulation for dispatch, maintenance-window
  scheduling, reserve equipment placement, inventory reorder policy, and pricing strategy. RL may
  recommend scenarios only after backtesting and simulation; it must not directly control production
  operations or payroll/HR decisions.
- **LLM assistance:** summarize evidence, draft 기안/procurement/approval memos, explain policy,
  translate/simplify documents, prepare executive decision briefs, and help map imports to ontology
  fields. LLM output is untrusted draft text and must be cited to source objects, reviewed by a human,
  and routed through workflow.
- **Executive decision support:** scenario narratives, sensitivity analysis, risk explanations,
  comparable cases, and board/management reports over governed facts.

Prerequisites before any AI/ML/RL/LLM production rollout:

- trusted master data, event ledgers, workflow outcomes, and financial/asset/inventory/HR domain
  boundaries are already implemented;
- deterministic algorithms and transparent scenario calculators exist and are observable;
- model registry, evaluation datasets, backtests, calibration reports, drift monitoring, rollback, and
  human-review workflow exist;
- privacy/labor/security review confirms purpose, minimization, masking, retention, and cross-tenant
  isolation;
- every AI-assisted write creates a governed workflow draft; no autonomous write-back to business
  records, payroll, employment status, prices, purchases, or asset disposal.

## Verification and governance gates

Before any recommendation can be production-actionable:

- [ ] Backtest against historical data and record calibration/error by model version.
- [ ] Unit/integration tests prove policy and RLS prevent cross-org/group leakage.
- [ ] E2E proves the UI creates a workflow draft, not direct write-back.
- [ ] Audit log contains actor/requester, model version, recommendation, policy version, passkey state,
      approval path, final decision, and override reason if applicable.
- [ ] Sensitive data review confirms masking/purpose/retention for HR/payroll/location inputs.
- [ ] Finance/HR/legal/business owner signs off any domain that affects payroll, employment, accounting,
      asset disposal, or customer contractual commitments.

## Implementation sequencing

1. **Now:** capture the structured workflow/event fields in every new workflow so future intelligence has
   usable data; add UI copy that treats recommendations as governed drafts.
2. **After ontology/action foundation:** add `Recommendation`, `ForecastModelVersion`,
   `OutcomeVariance`, and domain scenario objects as first-class object types.
3. **After trusted master data + ledgers:** ship backtested recommendations for asset lifecycle, rental
   rate/margin, reserve parts, and manpower/SLO planning.
4. **After governance hardening:** allow recommendations to create workflow drafts automatically, still
   requiring policy/approval/passkey for any business write.
5. **Last:** add AI/ML/RL/LLM augmentation only after the deterministic, algorithmic, policy,
   observability, evaluation, and rollback foundations above are already working in production.
