> **EXECUTABLE-CONTRACT / NON-AUTHORITY.** CI-bound gate contract (machine-checked). Cannot authorize product scope, clear HOLDs, release, or production readiness. Current product authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# HR / Payroll Readiness and Annual Leave Workflow Contract (G008)

Status: regulated readiness slice. This is an engineering control contract, not legal/tax advice. Payroll calculation remains blocked until the release gate is professionally validated by a licensed 노무사/세무사 and every payroll run references immutable source/version evidence.

## Target result

The COSS Group import can be used as governed production data for:

1. staging payroll draft runs from imported 급여/근태 rows and durable clock-in facts;
2. surfacing employee annual-leave balances and annual-leave usage-promotion workflow obligations;
3. preserving sensitive wage, bank, resident-registration, insurance, disability, and severance data in restricted import ledgers instead of generic HR screens;
4. later routing PTO-use reminders, wage-statement delivery, and approval requests through messenger/mail/workflow objects without silently calculating or sending regulated payroll outputs.

## Official-source guardrails checked on 2026-06-30

- 근로기준법 제60조 / 제61조: annual paid leave and usage-promotion handling are labor-law workflows, not generic notification copy. Source: <https://www.law.go.kr/법령/근로기준법/제60조>, <https://www.law.go.kr/법령/근로기준법/제61조>
- 근로기준법 제48조 and MOEL wage-statement guidance: wage-statement output requires the required wage-statement facts and audit evidence. Source: <https://www.law.go.kr/법령/근로기준법/제48조>, <https://www.moel.go.kr/policy/policydata/view.do?bbs_seq=20211101053>
- NTS wage-income withholding table: employee income tax must use an official NTS withholding row/artifact; the system must not invent tax brackets. Source: <https://www.nts.go.kr/nts/cm/cntnts/cntntsView.do?cntntsId=7862&mi=6583>
- 국민연금/NHIS official rates: statutory insurance rate tables must be effective-dated and source-linked. Source: <https://www.nps.or.kr/pnsinfo/ntpsklg/getOHAF0038M0.do>, <https://edi.nhis.or.kr/portal/images/popup/20251204_pop01longdesc.html>
- 개인정보 보호법 제15조/제30조 and 시행령 제17조: collection/use purpose, legal basis or consent method, retention, processor disclosure, and privacy-policy publication are release-blocking controls for initial login and employee-data import. Source: <https://www.law.go.kr/법령/개인정보보호법/제15조>, <https://www.law.go.kr/법령/개인정보보호법/제30조>, <https://www.law.go.kr/법령/개인정보보호법시행령/제17조>
- 개인정보 보호법 제23조/제24조의2: payroll, resident-registration number, bank-account, health/disability, contact, and address data are sensitive or uniquely identifying personal data and must stay purpose-bound, masked in previews, and least-privilege. Source: <https://www.law.go.kr/법령/개인정보보호법/제23조>, <https://www.law.go.kr/법령/개인정보보호법/제24조의2>
- 근로자퇴직급여 보장법 제8조: severance/interim-settlement facts can be preserved as source evidence but cannot be treated as a generic editable HR note or automated settlement output. Source: <https://www.law.go.kr/법령/근로자퇴직급여보장법/제8조>

## Data boundaries

- The raw COSS Group rows remain in `seed_import.workbook_rows` and `data_import_rows` with exact source metadata.
- Generic employee/HR screens expose safe directory fields plus aggregated leave/attendance status only.
- Payroll staging stores source-row links and review blockers; it does not store full resident-registration numbers, bank accounts, or raw payroll formulas in generalized run rows.
- All sensitive payroll actions require payroll-specific permission, purpose tag, passkey step-up for signing-equivalent actions, audit log, and source artifact references.
- Import mapping is schema/catalog-driven, not workbook-specific: flat sheets, shuffled columns, title rows, merged/pivoted sheets, and per-person payroll/attendance tabs must be detected, mapped, or rejected into exception reports with reviewer signoff. The entity type gate prevents employee data from being mapped into assets/sites and vice versa.

## Draft run lifecycle

1. **STAGED:** a payroll processor creates a draft from imported rows and/or durable attendance facts.
2. **BLOCKED_LEGAL_GATE:** default state. Payroll cannot be calculated or issued while NTS tax rows, official rate version, golden cases, or professional validation are missing.
3. **READY_FOR_REVIEW:** allowed only after source artifacts and professional validation exist for the pay type.
4. **APPROVED:** requires passkey step-up, actor attribution, approval comment, and immutable audit evidence.
5. **ISSUED:** wage-statement mail/receipt objects can be created only from an approved run.
6. **VOID:** corrections are new runs/events; raw source history remains preserved.

## Annual leave workflow

The annual-leave usage-promotion workflow is first modeled as `annual_leave_obligations` because reminder timing and payout consequences are regulated. Rows are purpose-bound by employee/year and track:

- accrued, used, and remaining leave imported from source workbooks;
- status: `NEEDS_HR_REVIEW`, `USAGE_PROMOTION_DRAFT_REQUIRED`, `PROMOTION_SENT`, `PAYOUT_REVIEW_REQUIRED`, or `CLOSED`;
- statutory-basis links and reviewer notes;
- notification/workflow plans that later become messenger/mail/workflow notification is a workflow object, not an ad-hoc message.

The system must never silently mark statutory usage-promotion complete because a message exists. HR must verify the legal notice path, target worker, delivery evidence, and deadline handling.

## COSS Group May 2026 live staging contract

The production staging SQL derives from the governed import ledger for `COSS Group 2026-05 live import` and produces:

- one payroll draft run per organization with imported rows;
- payroll draft lines grouped by canonical imported employee key;
- attendance/payroll/leave source-row counts and numeric facts only when values match strict numeric patterns;
- fail-closed blockers for missing NTS withholding rows, professional validation, and HR review;
- annual-leave obligations for employees with remaining leave;
- a sanitized aggregate audit event (`data_import.payroll_readiness_stage`) with no raw PII.

## Explicit non-goals for this slice

- No automatic income-tax calculation from memory.
- No autonomous PTO notification sending.
- No wage-statement issuance.
- No severance or interim-settlement calculation.
- No claim that production payroll is legally complete before professional review.
