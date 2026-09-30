> **EXECUTABLE-CONTRACT / NON-AUTHORITY.** CI-bound gate contract (machine-checked). Cannot authorize product scope, clear HOLDs, release, or production readiness. Current product authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Spec: Data Exchange Import/Export Workspace

**Status:** Draft for implementation planning
**Triggered by:** 2026-06-26 Excel/HR import request
**Workbook evidence:** Restricted external input. The source workbook and its two
derived profiles are not stored in Git; their exact custody identities are in the
[disk-wipe handoff](../handoffs/2026-08-03-disk-wipe-consolidation.md#restricted-workbook-custody).
If preserved, implementation must re-establish and review those inputs only from
approved encrypted custody. If discarded, obtain and review a newly authorized
source. Never assume a workstation path exists.
**Parent specs:** `docs/specs/knl-business-os.md`, `docs/specs/org-hierarchy.md`, `docs/specs/rbac-configurable.md`

## 1. Objective

Build a safe **data exchange workspace** for importing non-standard Excel/CSV data from multiple sources, mapping the source table into the correct canonical database entities, validating the mapping before writes, and exporting data back out in a standardized format.

This is **not** a simple upload button. Required flow:

1. Upload source file.
2. Display a preview table with sheet/header/data-row detection.
3. Classify the dataset domain: employee/HR, payroll, organization, RBAC, site/location, machinery/equipment, customer/vendor, etc.
4. Let an authorized user map sheets, rows, columns, fixed values, and relationships into the database schema.
5. Enforce type/domain safety: employee fields cannot be mapped into machinery/site fields; payroll fields cannot be imported through a general employee surface; RBAC grants cannot be bulk-loaded without elevated permission.
6. Run a dry-run validation that reports row-level errors/warnings without writing business records.
7. Apply only after review; write an immutable audit/import ledger.
8. Export canonical templates and standardized data packages regardless of source format.

Success means a messy source workbook can be turned into standard Oyatie/KNL SaaS data, and the system can export a clean standard template for future repeat imports.

**Current apply priority:** import readiness first supports organization setup, people, positions/roles,
clients, locations, and assets. Payroll, optimization analytics, and other advanced domains remain
staged/backlog until the foundation data model and permissions are reliable.

**Implemented readiness guardrail:** `web/src/features/data-exchange/domainMapping.ts` now provides the
front-end entity registry/classifier that previews source columns against canonical domains before an apply
API exists. It classifies Korean HR/payroll/org/site/equipment headers, requires dry-run review for restricted
columns, and blocks unsafe mappings such as employee identifiers into machinery fields or worksite addresses
into personal home-address fields. `web/src/features/data-exchange/sourceText.ts` also standardizes CSV export
with CRLF rows, quoting, and spreadsheet-formula neutralization. These are client-side readiness helpers only;
the future backend apply endpoint must enforce the same registry server-side before writing records.

## 2. Evidence from provided workbook

The provided workbook has 8 company-like sheets:

- `(주)디에스엘`
- `(주)코스`
- `(주)엘소`
- `(주)케이앤엘`
- `(주)청운로지스`
- `(주)씨앤엘`
- `(주)청운HR`
- `제이와이테크`

Each sheet is 1000 rows × 52 columns, with the first row acting as the header row. Most sheets hide column `N`. The populated headers are HR/payroll-oriented:

`NO.`, `소속`, `사번`, `성명`, `근무지(주소)`, `업무`, `장애유무`, `산재관리번호`, `직책`, `기본시급`, `통상시급`, `수당(통상포함)`, `수당(통상 미포함)`, `국민연금`, `건강보험`, `소득세`, `발생연차`, `사용연차`, `잔여연차`, `주민번호`, `입사일`, `보험가입일`, `퇴사일`, `보험상실일`, `근무지`, `거주주소`, `휴대폰`, `퇴직금 중간정산`, `지급일`, `급여산정일`, `은행`, `계좌/계좌번호`.

Detected browser employee rows by sheet use nonblank `성명`/name cells. The workbook also contains blank-name template/staging rows with company/source metadata; those rows must be preserved in raw import evidence but must not appear as people in the browser employee directory.

| Sheet | Browser employee rows | Source template/staging rows | Domain classification |
| --- | ---: | ---: | --- |
| `(주)디에스엘` | 13 | 95 | HR + payroll + organization/site references |
| `(주)코스` | 96 | 96 | HR + payroll + organization/site references |
| `(주)엘소` | 139 | 139 | HR + payroll + organization/site references |
| `(주)케이앤엘` | 4 | 68 | HR + payroll + organization/site references |
| `(주)청운로지스` | 1 | 68 | HR + payroll + organization/site references |
| `(주)씨앤엘` | 43 | 68 | HR + payroll + organization/site references |
| `(주)청운HR` | 22 | 62 | HR + payroll + organization/site references |
| `제이와이테크` | 11 | 11 | HR + payroll + organization/site references |

Sensitive/high-risk fields are present or reserved: name, resident registration number, phone, address, disability status, wage/payroll, insurance/tax, bank/account. Empty columns are meaningful: they may mean “fill later”, “not applicable”, “sensitive omitted”, or “clear existing value” depending on source/mapping policy. The import UI must make this explicit.

**Preservation rule:** every source column and cell must be preserved in the import ledger/staging layer before canonical mapping decides what can be written to first-class tables, including empty cells. This includes fields that are not yet supported by production schema, including `퇴직금 중간정산`, `지급일`, `급여산정일`, payroll/insurance fields, bank/account fields, formulas, comments, dates, hidden columns, and unmapped columns. If a canonical table does not exist yet, the value stays in typed staging/raw-row storage with its source sheet, row, column, header, detected type, original text, normalized value, and sensitivity classification. No field may be silently dropped merely because it is sensitive, empty, fill-later, or not yet modeled.

## 3. Industry-source-driven requirements

The design follows these public enterprise patterns:

- **Review mapping before import.** Microsoft model-driven app import explicitly includes reviewing column-to-field mapping, required/primary fields, optional ignored fields, alternate keys, lookup references, duplicate handling, progress logs, and exporting failed rows for correction. Source: <https://learn.microsoft.com/en-us/power-apps/user/import-data>, <https://learn.microsoft.com/en-us/dynamics365/customer-insights/journeys/import-data>.
- **Source keys for multi-source HR data.** Oracle HCM Data Loader recommends source keys because user/business keys can change and multiple source systems need stable identity. Source: <https://docs.oracle.com/en/cloud/saas/tutorial-hdl-load-files/>.
- **Metadata-driven employee imports.** SAP SuccessFactors uses distinct tools/templates for person/employment, foundation, and generic objects; metadata and field mappings drive CSV/file-based transfer, and test runs are part of safe migration. Source: <https://help.sap.com/doc/8ed3dfb2677a48c5b6ab758a737e8719/2605/en-US/SF_S4_EC_EE_Data_HCI_en-US.pdf>.
- **CSV interoperability.** RFC 4180 defines the common CSV format and `text/csv` MIME type; exports must follow consistent headers, quoting, CRLF, charset/header metadata, and same-field-count rows. Source: <https://www.rfc-editor.org/rfc/rfc4180>.
- **Korean CSV encoding.** Import preview must detect UTF-8/UTF-8-BOM/UTF-16 and CP949/Windows-949-style Korean CSV exports (decoded through the standard `euc-kr` label) before parsing rows. Hangul headers such as `이름`, `회사명`, `부서명` must render correctly in the mapping table; rows containing replacement characters (`�`) are validation errors, not importable records.
- **Secure file handling.** OWASP recommends allowlisted extensions, server-side file type/signature checks, generated filenames, size limits, authorized uploads, out-of-webroot storage, and defense-in-depth. Source: <https://cheatsheetseries.owasp.org/cheatsheets/File_Upload_Cheat_Sheet.html>.
- **Spreadsheet export safety.** OWASP/CWE document CSV/formula injection risks when exported fields start with formula metacharacters; exports must neutralize cells intended for spreadsheet viewing. Sources: <https://owasp.org/www-community/attacks/CSV_Injection>, <https://cwe.mitre.org/data/definitions/1236.html>.
- **Korean personal-data posture.** PIPC guidance emphasizes PIPA applicability to businesses processing Korean data subjects, privacy policy disclosure, breach notification/reporting, data subject rights, and cross-border-transfer clarity. Source: <https://www.pipc.go.kr/eng/user/ltn/new/noticeDetail.do?bbsId=BBSMSTR_000000000001&nttId=2488>.

## 4. Canonical data domains

Mapping is driven by a target **entity registry**, not by arbitrary table/column names. Each target field belongs to exactly one domain, with optional relationship edges to other domains.

### 4.0 Administrative boundary model

The import/export workspace must keep the administrative graph separate from operational verticals:

- **Tenant/Tenancy** = the subscription, isolation, billing, and lifecycle boundary. A tenant is what the platform can create, suspend, archive, or wipe. It is not automatically a conglomerate group.
- **Group** = a conglomerate/family holding view that links multiple subsidiary organizations for consolidated administration and reporting. A group may view member organizations together or drill into exactly one organization, but writes still target an explicit organization unless a later group-scoped workflow declares otherwise.
- **Organization** = a legal entity or operating company inside a tenant/group. It owns users, org units, worksites, roles, payroll context, and operating data.
- **OrgUnit/Position/Assignment** = the internal hierarchy and job graph. These are HR/org-management records, not authentication roles by default.

Imports must classify source data against this graph before allowing a write. A sheet named after a company can seed an `Organization`; a department column can seed an `OrgUnit`; a job-title column can seed a `Position`; but none of those values should silently become login roles, tenant slugs, or equipment/site records. Group-level imports are allowed only through an explicit group/organization mapping profile.

### 4.1 Employee/HR domain

Canonical entities:

- `Person`: legal/display name, contact channels, address refs, identity metadata markers.
- `Employee`: employee number, worker status, employment type, hire/termination dates, source keys. Employee rows stay in this domain; they cannot populate tenant/login identity fields except through an explicit account-creation mapping.
- `EmploymentAssignment`: company/legal entity, department/team, job/role, position, worksite, manager, effective dates.
- `LeaveBalance`: annual leave accrued/used/remaining, effective period.
- `DisabilityOrProtectedStatus`: separate sensitive field group; import only with explicit permission and legal basis.

HR owns person/employment facts. It does **not** own payroll amounts, authentication accounts, or logistics/maintenance job permissions. Those related records are created through explicit relationship mappings.

Workbook examples:

- `사번` → `Employee.employee_number` or source key fallback.
- `성명` → `Person.display_name` / legal name.
- `업무`, `직책` → `Position` / `JobClassification` / assignment title.
- `입사일`, `퇴사일` → `EmploymentAssignment.start_on/end_on` and employee lifecycle.
- `발생연차`, `사용연차`, `잔여연차` → `LeaveBalance`.

### 4.2 Payroll/compensation domain

Canonical entities:

- `CompensationProfile`: pay basis, hourly/monthly wage, allowances, pay schedule.
- `PayrollEnrollment`: pension/health/tax/social insurance flags and effective dates.
- `BankPaymentMethod`: bank/account holder/account number; encrypted/tokenized and separately permissioned.
- `PayrollRunInput`: effective-dated values feeding payroll calculations; no calculation shipped without golden-case validation.

Payroll is a separate high-sensitivity domain from HR. HR may show employment status and assignment; payroll imports and exports require payroll-specific permission, masking, audit, and dry-run review. A general employee import cannot write bank/account/tax/wage fields unless the mapping profile declares a payroll section.

Workbook examples:

- `기본시급`, `통상시급`, `수당`, `지급일`, `급여산정일` → compensation/payroll setup.
- `국민연금`, `건강보험`, `소득세`, `보험가입일`, `보험상실일` → payroll/insurance enrollment.
- `은행`, `계좌/계좌번호` → payment method; never preview raw to unauthorized users.

### 4.3 Organization/hierarchy domain

Canonical entities:

- `Group`: conglomerate/family group.
- `Organization`: legal entity/tenant.
- `OrgUnit`: department/division/team hierarchy within an organization.
- `TenantAccountSeed`: optional account bootstrap record for org-chart imports that create missing tenant organizations/users from approved org data.
- `Position`: position node tied to org unit, job classification, manager chain.
- `CostCenter`: optional accounting/payroll allocation unit.

Organization management owns the group/org/org-unit/position hierarchy. It can reference employees and account seeds, but it is not the tenant lifecycle console and is not a logistics/maintenance workflow.

Workbook examples:

- Sheet names and `소속` values → candidate legal entities or subsidiaries.
- Legacy CSV identifiers/emails such as `coss`/`cossok` are source provenance only; they must not become canonical organization, employee, or login identity.
- `사번` and email may be blank or assigned later; org import must support fill-later identity placeholders without blocking org/unit creation.
- `근무지` → worksite/department/customer site depending mapping.
- `업무`, `직책` → position/job classification when not payroll-specific.

### 4.4 Site/location domain

Canonical entities:

- `Site`: customer or internal worksite.
- `Address`: normalized postal address.
- `GeoPoint`: coordinates, geofence radius.
- `SiteAssignment`: employee assigned to site/work location.

Workbook examples:

- `근무지(주소)` → `Address` for a worksite/location, not a person home address unless mapped as `거주주소`.
- `근무지` → `Site.name` or `OrgUnit.name` depending selected profile.
- `거주주소` → personal address; employee/PII domain, never site domain by default.

### 4.5 Machinery/equipment domain

Already has registry tables and an equipment master-list import. This new workspace must keep it separate:

- Employee/HR columns cannot map to equipment fields.
- Equipment fields can only target registry entities (`registry_customers`, `registry_sites`, `registry_equipment`) through an equipment mapping profile.
- The existing `/api/v1/equipment/import` remains a specialized compatibility path; new work should be a generic Data Exchange profile that can eventually replace it.

### 4.6 RBAC/security domain

Canonical entities:

- `UserAccount`: authentication principal and profile.
- `RoleAssignment`: system/custom role assignment.
- `BranchMembership` / `AccessScope`: scope assignment.
- `GroupRoleGrant`: cross-entity group grants.

Positions and business roles are not automatically security roles. A `Position` such as `관리소장`, `정비사`, `노무담당`, or `구매과장` can be mapped to a recommended role assignment, but the actual `RoleAssignment` write is a separate elevated security action with preview and audit.

Bulk RBAC import is elevated and hazardous:

- Requires `RoleManage`/`ElevatedRoleGrant`-equivalent capability.
- Always dry-runs with explicit escalation report.
- Cannot create a last-admin lockout.
- Cannot import unknown feature/role names as allow; parse-or-deny.

## 5. Type-aware mapping rules

### 5.1 Domain classification comes first

Every source sheet/table gets a `classified_domain` before mapping:

- `employee_hr`
- `payroll`
- `organization`
- `rbac`
- `site_location`
- `machinery_equipment`
- `customer_vendor`
- `mixed`
- `unknown`

A `mixed` sheet must be split into sub-mappings by columns/row sections. For the provided workbook, the sheet should default to `mixed(employee_hr + payroll + organization/site references)`, with HR/payroll selected as the primary domain.

### 5.2 Field compatibility matrix

A source column can map only to target fields in compatible domains:

| Source classification | Allowed target examples | Blocked target examples |
| --- | --- | --- |
| Employee identity | Person, Employee | Equipment, Site geofence, customer billing |
| Payroll/wage | Compensation, PayrollEnrollment | Equipment cost ledger unless explicit finance import profile |
| Organization | Group, Organization, OrgUnit, Position | Person sensitive fields, machinery specs |
| Site/location | Site, Address, GeoPoint | Person home address unless source column is personal-address-classified |
| Machinery | Equipment, model, maker, spec, site assignment | Employee role/payroll fields |
| RBAC | User/role/scope assignments | Payroll, equipment specs |

The UI may suggest mappings, but the backend enforces compatibility. Manual override is allowed only where a target field explicitly declares a relationship edge, such as `employee.worksite_id -> site.id`.

### 5.3 Row-to-record mapping

A row may create/update multiple records through a graph plan, but each column is typed:

Example for a row in this workbook:

```text
Sheet=(주)코스
소속=코스
성명=<employee>
근무지=에스엠지연세병원
근무지(주소)=<site address>
직책=관리소장
기본시급=<wage>
입사일=23.07.01
```

Possible canonical graph:

```text
Organization(코스)
  └─ Site(에스엠지연세병원, address=근무지(주소))
      └─ EmploymentAssignment(employee=<성명/사번/source-key>, position=<직책>, worksite=site)
Employee(source_key, display_name=<성명>)
CompensationProfile(employee, hourly_wage=<기본시급>, effective_from=<입사일>)
```

The mapper should visualize this graph before apply.

### 5.4 Empty-cell semantics

For every mapped column, the user must choose how empty cells behave:

- `ignore`: do not change target value.
- `set_null`: clear existing value; high-risk and disabled by default.
- `unknown`: persist an explicit unknown/fill-later state.
- `sensitive_omitted`: source intentionally does not provide sensitive value.
- `not_applicable`: value does not apply for that row/domain.
- `formula_only`: cell contains a spreadsheet formula; import cached value only if workbook provides one, otherwise recompute with supported transform or reject.

For HR/payroll, default to `ignore` or `sensitive_omitted`, never `set_null`.

### 5.5 Source keys and upserts

Each profile must define an identity strategy:

1. Preferred: `(source_system_owner, source_system_id)` stable external key kept as provenance, not canonical identity.
2. Next: explicit natural key such as `사번 + 소속` only when `사번` is present and trusted for that source.
3. For fill-later employee numbers/emails, create a source-row placeholder and require later reconciliation.
4. Last-resort: review queue for fuzzy matches, never silent merge on name alone.

Legacy CSV IDs, usernames, or emails (`coss`, `cossok`, etc.) may identify the source row/system during import, but must not be promoted into canonical employee number, organization slug, or authentication email unless an authorized mapping explicitly says that value is the new canonical value.

The import engine must support:

- create
- update/upsert
- no-op unchanged
- conflict/review-required
- rejected row

## 6. Data Exchange UI

### 6.1 Upload and profiling

Page: `Data Exchange` or `Import/Export` under admin/data management.

Upload panel requirements:

- Accept `.xlsx` and `.csv` initially; future `.xml`, `.json`, `.zip` only through explicit allowlist.
- Show sheets/tables, row/column counts, hidden sheets/columns, detected header rows, merged cells, formula count, data validation/dropdowns if present.
- Store file under generated object key, not user filename.
- Do not expose sensitive values in preview unless the user has the required permission.

### 6.2 Preview grid

The preview grid must show:

- Sheet selector.
- Header row selector.
- Row range selector.
- Column list with inferred type, domain, sample values redacted as needed.
- Formula indicators.
- Hidden column indicators.
- PII/sensitive badges.
- Unmapped/required/invalid counts.

### 6.3 Mapping builder

Mapping builder requirements:

- Select target domain first.
- Then select target entity/relationship.
- Then map source column → target field, with transforms.
- Show required fields and alternate/source keys.
- Support lookup/reference mapping: e.g. `근무지` resolves or creates `Site`; `소속` resolves or creates `Organization`; `직책` resolves or creates `Position`.
- Support org-chart imports that create missing tenant accounts/organizations from approved organization mappings, while keeping employee roster fields in the employee domain.
- Support option/picklist mapping: source values → canonical enum values.
- Support “create missing dropdown value” only where the target field allows managed vocabulary creation.
- Save named reusable profiles by tenant/org/group.

### 6.4 Dry-run result UI

Dry-run must show:

- Counts: rows parsed, creates, updates, unchanged, warnings, errors, conflicts.
- Entity graph diff by row.
- Row-level errors with sheet/row/column and suggested fix.
- “Export failed rows” as XLSX/CSV with error columns.
- Audit preview: what action category will be logged.

### 6.5 Apply/import UI

Apply requires:

- Dry-run result from the same file checksum and mapping profile version.
- Permission re-check at apply time.
- Explicit confirmation for sensitive fields, payroll fields, RBAC grants, and destructive/nulling changes.
- Idempotency key to prevent double apply.
- Progress and import log.

## 7. Backend architecture

### 7.1 New bounded context

Add a new bounded context rather than expanding equipment import:

```text
backend/crates/data-exchange/domain
backend/crates/data-exchange/application
backend/crates/data-exchange/adapter-postgres
backend/crates/data-exchange/rest
web/src/features/data-exchange
web/src/pages/DataExchangePage.tsx
```

This keeps generic import/export separate from registry-specific code.

### 7.2 Application primitives

Core types:

```rust
DataDomain = EmployeeHr | Payroll | Organization | Rbac | SiteLocation | MachineryEquipment | CustomerVendor | Mixed | Unknown
ImportFileProfile { sheets, header_candidates, columns, inferred_types, sensitivity }
EntityCatalog { domains, entities, fields, relationships, validators, permissions }
MappingProfile { id, org_id, name, source_kind, target_domain, sheet_mappings, column_mappings, transforms, empty_semantics, source_key_policy, version }
ImportJob { id, file_id, mapping_profile_id, mode: DryRun|Apply, status, checksum, totals, created_by, timestamps }
ImportRowResult { job_id, sheet, row_number, planned_changes, warnings, errors }
ExportProfile { id, target_domain, format, fields, redaction_policy, standard_template_version }
```

### 7.3 Persistence sketch

Initial schema tables:

```sql
data_exchange_files
  id, org_id, original_filename, storage_key, content_type, size_bytes, sha256,
  uploaded_by, uploaded_at, profile_json, retention_until

data_exchange_entity_catalog_versions
  id, version, catalog_json, created_at

data_exchange_mapping_profiles
  id, org_id, name, source_kind, target_domain, profile_json, version,
  created_by, created_at, updated_at, retired_at

data_exchange_jobs
  id, org_id, file_id, mapping_profile_id, mapping_profile_version,
  mode, status, idempotency_key, totals_json, started_by, started_at, finished_at

data_exchange_row_results
  job_id, sheet_name, source_row, source_row_hash, status,
  planned_changes_json, warnings_json, errors_json

data_exchange_raw_cells
  job_id, sheet_name, source_row, source_column, source_header,
  original_text, normalized_value, is_empty, sensitivity, provenance_json

data_exchange_audit_links
  job_id, audit_event_id

external_source_keys
  org_id, domain, entity_kind, entity_id, source_system_owner, source_system_id,
  source_record_hash, first_seen_at, last_seen_at,
  unique(org_id, domain, source_system_owner, source_system_id)
```

All tenant-scoped tables must be `FORCE ROW LEVEL SECURITY`, audited for state-changing writes, and compatible with the existing `console-gate` static checks.

### 7.4 Import execution model

1. `POST /api/v1/data-exchange/files` uploads and profiles only.
2. `GET /api/v1/data-exchange/files/{id}/profile` returns redacted profile.
3. `GET /api/v1/data-exchange/catalog` returns target domain/entity/field catalog for the caller's permissions.
4. `POST /api/v1/data-exchange/mappings` saves a mapping profile.
5. `POST /api/v1/data-exchange/jobs:dry-run` parses + validates + stages row results without target writes.
6. `POST /api/v1/data-exchange/jobs/{id}:apply` applies a verified dry-run using idempotency and transactions.
7. `GET /api/v1/data-exchange/jobs/{id}` returns status/results.
8. `GET /api/v1/data-exchange/jobs/{id}/failed-rows` exports repair workbook.
9. `GET /api/v1/data-exchange/exports/templates/{domain}` exports standard templates.
10. `POST /api/v1/data-exchange/exports` exports standardized data.

## 8. Standardized exports

Every target domain has a canonical export profile. Imports may start from XLSX/CSV, but exports use standardized Oyatie formats, not source-specific headers.

- Stable machine headers and Korean display labels.
- Required/optional markers.
- Data dictionary sheet.
- Source key columns included by default.
- Reference lookup columns include both display name and stable ID/key.
- Redaction policies by role.
- CSV output follows RFC 4180; XLSX output uses the same canonical field order.
- Spreadsheet-facing output neutralizes formula injection risk for text cells.
- Organization exports support both group-wide views and individual-organization views.

For HR/payroll, produce at least:

- `standard_employee_roster.xlsx/csv`
- `standard_employment_assignments.xlsx/csv`
- `standard_payroll_setup.xlsx/csv` (sensitive export, finance/payroll permission only)
- `standard_org_hierarchy.xlsx/csv` with group-wide and per-organization scopes
- `standard_rbac_assignments.xlsx/csv` (elevated only)

## 9. Security, privacy, and compliance boundaries

- Uploads: allowlist extension + content sniffing/signature + size limit + generated object key + no public retrieval + retention/deletion policy.
- Parsing: sandboxed/worker execution path for large files; reject encrypted/password-protected files initially unless a safe decrypt flow is specified.
- Preview: redact PII/sensitive/payroll/bank fields by default.
- Sensitive fields: require separate target domain permissions and explicit mapping confirmation.
- Resident registration numbers: do not import by default; if legally required, store only through a dedicated sensitive identifier module with encryption, access logging, retention, and legal basis metadata.
- Payroll/bank/account values: encrypted/tokenized at rest, never logged, never shown in row errors.
- Audit: all file upload/profile, mapping create/update, dry-run, apply, export, failed-row export, and sensitive-field access events are audited.
- Korean privacy posture: implement data minimization, purpose tagging, privacy-policy/consent tracking where required, breach-support logs, and data subject correction/deletion/suspension workflows consistent with the broader compliance work.
- Cross-border/object storage: disclose processors/storage locations if applicable; avoid sending employee source files to third-party scanners unless approved and documented.

## 10. Post-wipe initial seed from the provided workbook

The requested production sequence is:

1. Take final read-only safety exports for any reusable geodata/location evidence.
2. Wipe/reset the production Postgres database and OCI object-storage objects so the system starts from a fresh slate.
3. Create one conglomerate/family group named `그룹`.
4. Create the 8 worksheet organizations and make each a member of `그룹`.
5. Import every employee row from the matching worksheet into the appropriate organization.
6. Preserve every workbook value in the import ledger/staging layer, then write only currently supported canonical records into live first-class tables.

Initial organization seed set:

| Organization name | Proposed slug | Parent group | Browser employee rows |
| --- | --- | --- | ---: |
| `(주)디에스엘` | `dsl` | `그룹` | 13 |
| `(주)코스` | `kos` | `그룹` | 96 |
| `(주)엘소` | `lso` | `그룹` | 139 |
| `(주)케이앤엘` | `knl` | `그룹` | 4 |
| `(주)청운로지스` | `cheongun-logis` | `그룹` | 1 |
| `(주)씨앤엘` | `cnl` | `그룹` | 43 |
| `(주)청운HR` | `cheongun-hr` | `그룹` | 22 |
| `제이와이테크` | `jy-tech` | `그룹` | 11 |

The first post-wipe seed should create default `본사` region/branch records per organization only as an access anchor. Detailed worksites/geodata are not trusted until re-imported or manually reconciled through the mapping workspace; however, a pre-wipe read-only geodata export should be kept as a reference candidate set.

Current-schema seed behavior:

- Create `groups`, `organizations`, and `group_memberships` for the `그룹` family structure.
- Create per-organization default access scope records (`regions`, `branches`) needed by existing RBAC/user membership tables.
- Create tenant organization/account records from the org chart when approved; create employee login users only when a canonical email/identity mapping is approved.
- Preserve `사번`, `성명`, phone, assignment, hire/termination dates, leave, payroll, insurance, bank/account, disability/protected-status, `퇴직금 중간정산`, `지급일`, `급여산정일`, and all other workbook fields in import staging/ledger data.
- Do **not** coerce payroll, bank/account, resident registration number, disability status, or retirement-settlement fields into general `users` columns. Those stay in sensitive typed staging until HR/payroll schemas and permissions are implemented.
- Do **not** silently merge employees by name alone. Use `(organization, 사번)` only when trusted and present; otherwise a deterministic source row key plus review workflow.

Fresh-slate safety gates:

- Wipe/reset is destructive and must happen only after a final operator confirmation naming the target environment and accepting that all existing production database/object data will be deleted.
- Before the wipe, collect a read-only geodata/location candidate export and a schema/object inventory.
- After the wipe, run migrations on an empty database, verify app health, seed `그룹` + the 8 organizations + employee roster, then run a dry-run import report showing preserved-row/column counts and rejected/unmapped fields.

## 11. Implementation plan

### Phase 0 — Spec and fixture lock

- [ ] Commit this spec.
- [ ] Add a redacted workbook profile fixture derived from the provided workbook (headers/types/counts only, no sensitive cell values).
- [ ] Add tests for source profiling and domain classification using that fixture.

### Phase 1 — Catalog and profiling API

- [ ] Add data-exchange domain/application crates with entity catalog and compatibility matrix.
- [ ] Add upload profiling service for XLSX/CSV: sheets, headers, columns, inferred types, sensitivity, formula/hidden-column metadata.
- [ ] Add REST endpoints for file upload/profile/catalog.
- [ ] Add frontend preview grid with redaction.

### Phase 2 — Mapping profiles and dry-run

- [ ] Add mapping profile persistence and validation.
- [ ] Implement compatibility enforcement: employee cannot map to machinery/site fields except declared relationship edges.
- [ ] Implement empty-cell semantics.
- [ ] Implement source key/upsert planning.
- [ ] Add dry-run job and row-results UI.

### Phase 3 — HR/org import apply

- [ ] Add minimal HR/org schema needed for this workbook: person, employee, employment assignment, org unit/position/site assignment, source keys.
- [ ] Implement apply for HR/org only, with audit and idempotency.
- [ ] Keep payroll values in staging/dry-run until payroll schema and legal/compliance posture are approved.

### Phase 4 — Standard exports

- [ ] Export canonical templates for employee roster, employment assignments, org hierarchy, and site assignment.
- [ ] Export failed-row repair workbooks.
- [ ] Add CSV/XLSX formula-injection protection and RFC 4180-compatible CSV output.

### Phase 5 — Payroll/RBAC/equipment expansion

- [ ] Payroll import after payroll golden cases and sensitive-data storage design.
- [ ] RBAC import after configurable RBAC implementation is complete.
- [ ] Migrate current equipment master-list import into generic data exchange profile.

## 12. Acceptance criteria

- A user can upload the provided workbook and see all 8 sheets with redacted previews and detected headers.
- The system classifies the workbook/CSV as HR/payroll/org-location-reference data, not machinery.
- The UI blocks mapping `성명` to equipment model/maker/spec and blocks mapping `기본시급` to site/machinery fields.
- The UI allows mapping `근무지` as a relationship from employee assignment to `Site` only when the target relationship is selected.
- Empty sensitive, employee-number, and email columns can be marked as `sensitive_omitted` or `fill later` without clearing existing data.
- Dry-run shows planned entity graph changes per row and exports failed rows.
- Apply cannot run if mapping profile, file checksum, permissions, or catalog version changed after dry-run.
- Exported employee/org data follows canonical standardized templates regardless of input source format, with group-wide and individual-organization scopes.
- Every upload, mapping, dry-run, apply, and export has an audit trail.
- Unauthorized users cannot preview or export payroll/bank/resident-registration fields.
- Raw import provenance retains every source column, including empty, sensitive, legacy ID/email, and fill-later values.

## 13. Open implementation questions

1. Should the first apply-capable slice import only non-payroll HR/org fields, leaving payroll/bank/resident-registration fields as staged/redacted until the payroll/compliance sub-spec is approved? Recommended: yes.
2. Should source files be retained after successful import, or purged after a short retention period with only row hashes/profiles retained? Recommended: short retention/purge for HR/payroll files.
3. Which canonical source-system owner should be used for this workbook family: one owner per sheet/company, or one owner for the Excel source system with sheet/company as a namespace? Recommended: `excel:<workbook-family>:<sheet-name>` until upstream systems are known.
