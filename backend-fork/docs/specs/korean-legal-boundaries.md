> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Korean legal/compliance boundaries for enterprise operations

Status: engineering guardrails for product planning and implementation. This is not legal advice and does not replace Korean counsel/management sign-off before launch.

## Source anchors checked on 2026-06-28

- Personal Information Protection Act / 개인정보 보호법: <https://law.go.kr/lsInfoP.do?lsId=011357>
- PIPC current privacy-policy guide list and 2026 guide: <https://www.pipc.go.kr/np/cop/bbs/selectBoardList.do?bbsId=BS217&mCode=D010030000>, <https://www.privacy.go.kr/front/bbs/bbsView.do?bbsNo=BBSMSTR_000000000049&bbscttNo=20885>
- Labor Standards Act / 근로기준법: <https://www.law.go.kr/LSW/lsInfoP.do?ancYnChk=0&lsId=001872>
- Labor Standards Act Enforcement Decree examples for worker/payroll identifying records: <https://www.law.go.kr/LSW/lsInfoP.do?efYd=20211014&lsiSeq=236083>
- MOEL wage-statement guidance/calculator: <https://www.moel.go.kr/wageCal.do>
- Location Information Act / 위치정보의 보호 및 이용 등에 관한 법률: <https://www.law.go.kr/LSW/lsInfoP.do?lsiSeq=277359>
- EasyLaw personal-location explainer: <https://www.easylaw.go.kr/CSP/CnpClsMain.laf?ccfNo=2&cciNo=1&cnpClsNo=1&csmSeq=1702>
- Employee Retirement Benefit Security Act / 근로자퇴직급여 보장법: <https://www.law.go.kr/LSW//lsSideInfoP.do?docCls=jo&joBrNo=00&joNo=0009&lsiSeq=279829&urlMode=lsScJoRltInfoR>

## Product guardrails

1. **Purpose and minimization first.** Every personal/employee/customer/contact/location/payroll field needs a declared purpose, lawful basis/consent marker where needed, retention policy, and domain owner. Generic imports must stage unknown/sensitive fields; they must not silently write them to canonical tables.
2. **Initial login and consent.** Console/mobile onboarding must present privacy policy, personal-data collection/use terms, cookie notice where applicable, and location/work telemetry terms before collecting or using non-essential personal/location data.
3. **Sensitive domains are separate.** HR, payroll, bank/account, tax/social-insurance, resident-registration, disability/protected-status, retirement/severance, and location telemetry require domain-specific permissions, masking, audit, and retention controls.
4. **Korean labor records are first-class.** Worker roster, employment dates, worksite/position/assignment, wage ledger, wage statement, work hours, overtime/night/holiday work, leave, retirement/severance, and intermediate settlement dates must preserve calculation inputs and effective dates.
5. **Payroll is not “just employee data.”** Payroll imports/exports and wage/retirement calculations require payroll permission, legal/versioned formula sources, golden-case tests, and explicit legal-review gates before production use.
6. **Location is consent/purpose scoped.** GPS/arrival/geofence data must be tied to an explicit work purpose, consent state, collection interval, retention period, and viewer/action permission. Avoid continuous tracking by default.
7. **Signing-equivalent actions require signer attribution.** Approvals that affect pay, employment, legal commitments, asset sale/acquisition, pricing, write-off, sensitive location disclosure, or policy changes require passkey step-up before claiming the audit actor personally performed the action.
8. **Audit without overexposure.** Audit logs should prove actor, time, object, action, policy decision, step-up state, and memo/evidence references; they should not dump raw payroll, bank, resident-registration, or private location payloads into generic logs.
9. **Cross-border/cloud storage needs disclosure.** OCI object storage, logs, backups, and processors must be disclosed in the privacy policy/processor records where applicable, with deletion/export procedures.
10. **Recommendations are not decisions.** Optimization outputs for rental pricing, reserve policy, asset lifecycle, payroll, staffing, or SLA risk are recommendations until a governed workflow approves them; retain assumptions, input snapshots, and decision lineage.
11. **Workflow history can be analytics input, but only with purpose and masking.** Past 기안, 구매, 승인, 입찰, pricing, planning, dispatch, maintenance, HR, and payroll events may feed future recommendations only when the data class, purpose, retention, masking, and viewer permissions are respected. Sensitive HR/payroll/location fields must not leak through generic analytics.
12. **AI/ML/RL/LLM are final-stage assistants only.** Do not use AI to make payroll, employment, discipline, termination, retirement/severance, location-surveillance, pricing, purchase, asset-disposal, or customer-contract decisions automatically. Any future AI-assisted output must be an explainable draft/recommendation with source lineage, human review, policy approval, privacy/labor-purpose limits, and audit.

## Intra-group inter-org employment transitions

This product must not treat an intra-group move as a simple tenant/user edit. If an employee legally
leaves one group company and starts employment at another group company, model it as a **separation
episode plus a new hire episode** under the same person identity, unless counsel/HR classify the move
as legal continuity/succession.

Engineering rules:

1. **Person identity persists; employment episodes are separate.** Keep one `Person`/identity record
   for audit and login continuity, but create separate `EmploymentEpisode` records per legal employer
   with own employer/org, start date, end date, position, department/team, job function,
   responsibilities, wage basis, retirement/severance basis, and policy version.
2. **Voluntary resignation + new hire workflow.** Capture resignation request/acceptance, last working
   day, final wage/payroll calculation, unused leave/settlement fields, retirement/severance
   calculation, and new-hire effective date for the receiving company. Do not silently overwrite the
   prior employer fields.
3. **Retirement/severance settlement is a domain workflow.** If retirement benefit settlement is due,
   record the calculation inputs, payment/transfer route, due date, paid date, payer legal employer,
   and supporting documents. If HR claims continuous service or no settlement, require a legally
   reviewed reason and document reference.
4. **Interim settlement is not a generic button.** 퇴직금 중간정산 is only for legally allowed cases and
   must record the statutory reason/evidence and reset/continuity implications; ordinary group transfer
   should not be mislabeled as 중간정산 unless counsel confirms it.
5. **Access changes atomically.** On separation from the sending org, revoke that org's active
   responsibilities, scopes, queues, mail groups, calendars, and sessions as policy requires. On the
   receiving org start date, activate only the new org scopes/responsibilities. Shared group visibility
   must come from group policy, not stale old-org membership.
6. **Data preservation with purpose boundaries.** Preserve labor/payroll/retirement/audit history for
   the sending legal employer according to retention rules, but restrict visibility after transfer to
   authorized HR/payroll/legal/admin scopes. Receiving-company managers should not automatically see
   prior-company sensitive payroll/retirement details unless policy/legal basis allows it.
7. **No duplicate active users.** The same person can have historical employment episodes and at most
   policy-approved active account contexts. UI must show status such as `active in receiving org`,
   `terminated in sending org`, and `group person retained` rather than two unrelated active users.

Relevant source anchors:

- Labor Standards Act worker roster and record preservation duties: <https://www.law.go.kr/LSW/lsLawLinkInfo.do?chrClsCd=010202&lsId=001872&lsJoLnkSeq=900551968&print=print>
- Labor Standards Act wage ledger/wage statement duties: <https://law.go.kr/lsLinkCommonInfo.do?lsJoLnkSeq=1012793113>
- Labor Standards Act Enforcement Decree record-retention start points: <https://law.go.kr/LSW//lsLinkCommonInfo.do?chrClsCd=010202&lspttninfSeq=70870>
- Employee Retirement Benefit Security Act severance payment rule: <https://www.law.go.kr/LSW//lsSideInfoP.do?docCls=jo&joBrNo=00&joNo=0009&lsiSeq=279829&urlMode=lsScJoRltInfoR>
- MOEL retirement/wage payment timing FAQ: <https://www.moel.go.kr/faq/faqView.do?seqRepeat=118>
- MOEL wage-statement calculator/guidance: <https://www.moel.go.kr/wageCal.do>

## Implementation backlog links

- Initial login/privacy/cookie consent: keep wired to auth/onboarding and public storefront work.
- Data exchange/import: preserve every source cell in staging, classify sensitive fields, and require explicit target-domain mapping.
- Approval/workflow: federated approval items must expose ontology/workflow/policy context and re-authorize source-specific writes.
- Workflow analytics: 기안/구매/승인/입찰/pricing/planning histories should be durable ontology events with inputs, alternatives, decisions, outcomes, variance, sensitivity class, and policy version.
- Operations intelligence: follow `docs/specs/operations-intelligence.md`; deterministic/probabilistic calculators and observability come before AI/ML/RL/LLM, and sensitive labor/payroll/location inputs must remain purpose-bound and masked.
- Org/RBAC/PBAC/ABAC: policy should depend on tenant/group/org/department/team/position/custom role, action, object type, data class, and legal/sensitivity domain.
- HR/payroll: block production payroll calculations until legal formulas, effective-dated rate tables, golden tests, and counsel review are complete.
- Intra-group transfer: implement as person identity + separate employment episodes + separation/new-hire workflow; preserve sending-org records while moving active policy scope to the receiving org.
