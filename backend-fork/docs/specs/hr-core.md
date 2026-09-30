> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# G011 HR Core Slice

## Objective
Provide the first production HR setup surface for imported people data: employee roster, 직급/직책 and org-chart grouping, imported annual-leave balances, and clock/attendance visibility. This is an incremental HR core, not payroll calculation and not a replacement for any future HR/payroll system of record.

## Scope
- Preserve source workbook rows exactly in `employees.raw_row` / `source_metadata`.
- Promote safe HR fields to first-class columns: employee number, org unit/department, job, position, worksite, hire/exit dates, employment status, annual leave accrued/used/remaining.
- Keep payroll, bank/account, resident registration number, insurance, disability/protected-status, and retirement-settlement fields in raw/staging only until dedicated high-sensitivity payroll schemas and permissions exist.
- Derive an org-chart view from canonical employee rows: company -> org unit/worksite -> position -> employees.
- Expose leave balances from imported HR columns without payroll math.
- Expose attendance summary from existing durable `site_attendance_events` business facts; do not store raw coordinates in HR.

## API
- `GET /api/v1/employees`: returns canonical HR fields plus preserved raw row.
- `POST /api/v1/employees/import`: extracts canonical HR fields during import while preserving all columns.
- `GET /api/v1/hr/org-chart`: tenant-scoped org-chart grouping.
- `GET /api/v1/hr/leave-balances`: tenant-scoped imported leave balances and totals.
- `GET /api/v1/hr/attendance-summary`: branch-scoped attendance summary from durable arrival/departure events.

## Security and privacy
- All endpoints require `EmployeeDirectoryRead` or `EmployeeDirectoryManage` through the existing authz matrix.
- Tenant isolation remains `app.current_org` + FORCE RLS; attendance summary also applies branch scope.
- Sensitive payroll/PII columns are not promoted or shown in summary cards.
- Telemetry uses bounded counters only; no names, IDs, or raw workbook values as metric labels.

## Verification
- Frontend EmployeesPage tests cover roster, org chart, leave, attendance, admin-only import.
- OpenAPI app compile verifies Rust route/schema integration.
- Generated TS/Kotlin/Swift clients include the new HR endpoints.
- Browser e2e proves the HR setup page renders employees, org chart, leave, and attendance panels.
