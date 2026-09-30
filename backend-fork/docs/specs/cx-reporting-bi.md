> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# CX, reporting, BI, and executive dashboard contract

Date: 2026-06-29
Ultragoal story: `G026-cx-sales-support-reporting-bi-and-ex`

## Production UI contract

1. **CX/service desk is an operational queue, not a notes page.** `/support` must show SLA posture, urgency, assignment, closed/resolved history, list search, and direct links from a ticket to the work object, messenger, mail, and reporting paths.
2. **Reporting exports are standardized outputs.** `/reporting` must keep the workbook export path as the primary action and show session download history only after a real successful export; it must not show backend-missing history panels or demo copy.
3. **BI dashboards must drill back to execution.** `/kpi` and `/wallboard` must expose actionable links back to operations, reporting, support, and the relevant source scope rather than vanity metrics only.
4. **Scope is honest.** Current KPI rollups are company/region/branch/technician scopes from the API. UI labels may describe consolidated versus scoped views, but must not claim group-wide consolidation unless the backend supplies group/org scope.
5. **No dead panels.** CX/reporting/BI screens must not expose “coming soon”, backend-unavailable, or demo/stub capability. Missing future analytics belongs in the backlog, not production UI.

## Verification surface

- `scripts/check-cx-reporting-maturity.mjs` statically checks the production UI contract and CI wiring.
- Focused tests cover support object links, reporting command/history behavior, KPI drilldowns, and wallboard navigation.
