# Benchmark Matrix — Module: COMPLIANCE

## Obligations / regulations / frameworks · controls · findings · audits · four-eyes

Compiled 2026-07-11. Our-console column is code-grounded (file paths cited). Vendor claims are
`[V]` VERIFIED (source URL) or `[I]` INFERRED (reasoned from known product patterns, labeled honestly).

---

## 0. What "compliance" is in OUR system today (evidence-based)

Grounded in a read of the actual tree, not the roadmap's aspirations:

- **Domain graph — BUILT (persistence + domain + application layers).** Migration
  `backend/crates/platform/db/migrations/0148_create_compliance_domain.sql` defines the full spine in source:
  `compliance_regulation_impacts` (RG-####), `compliance_obligations` (CP-####),
  `compliance_frameworks` (FW-####), `compliance_controls`, plus three link tables —
  `compliance_obligation_regulations` (DERIVED_FROM/AMENDED_BY/SUPERSEDED_BY/INTERPRETS/EVIDENCES,
  **append-only**), `compliance_control_obligations` (coverage = PRIMARY/PARTIAL/SUPPORTING/COMPENSATING),
  and `compliance_evidence_bindings` (a control/obligation → `audit_event | evidence_media |
  workflow_run | workflow_task | object_link | governance_finding | external_document`, with
  SHA-256, confidence, valid_from/to, PROPOSED→ACCEPTED FSM).
- **Scoped governance controls are source-backed.** Server-issued **immutable codes** (trigger
  `enforce_compliance_code_immutable`), **FORCE ROW LEVEL SECURITY** org-isolation on every table,
  **no-DELETE grants** to `console_rt` (append-oriented database retention), append-only relation triggers.
  Domain enums fully typed in `backend/crates/compliance/domain/src/lib.rs`; use-cases + pagination in
  `.../application/src/lib.rs`.
- **REST surface — PARTIAL.** `backend/crates/compliance/rest/src/lib.rs` currently wires the
  **location-consent FSM** (grant/suspend/resume/withdraw), location-pings, arrival-events, and
  the covert CEO audit stream. The location-consent FSM is a product/policy control, not a statutory conclusion.
  The [Location Information Act](https://www.law.go.kr/%EB%B2%95%EB%A0%B9/%EC%9C%84%EC%B9%98%EC%A0%95%EB%B3%B4%EC%9D%98%EB%B3%B4%ED%98%B8%EB%B0%8F%EC%9D%B4%EC%9A%A9%EB%93%B1%EC%97%90%EA%B4%80%ED%95%9C%EB%B2%95%EB%A5%A0), [PIPA](https://www.law.go.kr/%EB%B2%95%EB%A0%B9/%EA%B0%9C%EC%9D%B8%EC%A0%95%EB%B3%B4%EB%B3%B4%ED%98%B8%EB%B2%95), and labor-law regimes are distinct; the applicable basis, notice, consent, retention, and employment issues remain scenario-specific for qualified Korean counsel.
  The **obligation/framework/control CRUD is NOT yet exposed over HTTP** — the application layer
  exists, but routes are unwired.
- **Console UI — NOT built.** No React screen for obligations/frameworks/controls. The
  `console-program-ledger.md` lists "compliance UI surface" as **Phase C wave 2 item ⑦** (planned).
  The compliance persona journey (감사→drill→정책 시뮬) is specified but unrendered.
- **Adjacent primitives already strong and reusable by this module:**
  - **Findings:** `governance_findings` (mig 0050) — anti-embezzlement detectors, severity/status,
    subject_user, idempotent per (detector_id, entity). `gov_approval_requests` (mig 0158).
  - **Audit-chain seam:** `audit_events` + audit-chain seals (mig 0100/0101, PR #204) — seal worker,
    verify, gap detection, and corrupt-seal verdict; console `AuditFeed.tsx`. Production sealing defaults
    OFF, the in-memory signer is not a trust root, NULL-org rows are excluded, and real tamper evidence
    requires an external signer plus out-of-band anchor.
  - **Four-eyes / SoD:** a **platform-wide** primitive — `gov_approvals` (approver ≠ requester)
    enforced on ontology schema commits (`ontology/model.ts`), object-card overrides (§20,
    `objectcard/wired.tsx`), leave approvals (SoD no-self-approve, `leave/LeaveConsole.tsx`),
    workflow-studio version approval, and `appr/composeModel.ts` (`sod: "self_approval"` block).
  - **Evidence / chain-of-custody:** EV- objects — copy/fixity metadata, custody events
    (REGISTERED→…→LEGAL_HOLD→DISPOSED), evidence-readiness chips, and nullable RFC-3161/TSA
    metadata (`console/evidence/types.ts`). No non-null exercised TSA token is current proof.

**Net:** we have source-wired fixity/WORM-status and hold seams, a four-eyes primitive, an
ontology-linked obligation→control→evidence domain graph, and FORCE RLS on the compliance tables.
Production object lock, an external trust root and out-of-band anchor, exercised non-null TSA, NULL-org coverage, and operational proof remain open.
Live authorization remains the current server/legacy matrix; the UI feature projection is
non-authoritative, and Cedar is target/authoring/simulation/shadow only until an action is enrolled,
shadow-proven, and promoted under ADR-0021 and `docs/specs/cedar-pbac-coexistence-map.json`.
There is still **no user-facing compliance product yet** (no CRUD API, screens, regulatory content
library, control-testing campaigns, compliance calendar UI, or risk heatmap).

---

## 1. Capability matrix

Columns: **US** = our console · **FDN** = Palantir Foundry · **SLK** = Slack · **TMS** = MS Teams
(Purview) · **ASN** = Asana · **N8N** = n8n · **RPL** = Rippling · **SAP** = SAP (GRC / S/4HANA).

Legend per cell: `[V]`=verified(URL in §3) · `[I]`=inferred · `N/A`=doesn't play here (reason).

### R1 — Information architecture (the compliance object model)

| US | FDN | SLK | TMS | ASN | N8N | RPL | SAP |
|----|-----|-----|-----|-----|-----|-----|-----|
| Explicit obligation→regulation→framework→control→evidence graph as first-class typed objects w/ coverage semantics `[V-code: mig 0148]` | No compliance objects, but the *pattern* is native: model it as ontology object-types + link-types `[I]` | N/A — a messaging store, no obligation model | Purview Compliance Manager = regulation→assessment→control→improvement-action tree `[V]` | Generic work items + custom fields; no built-in obligation model `[I]` | N/A — no domain model, only workflows | HR/employment obligations are code-embedded, not user-modeled objects `[I]` | GRC is THE reference model: regulation → control objective → control → test → issue → risk `[V]` |

### R2 — Regulatory content library (out-of-box templates)

| US | FDN | SLK | TMS | ASN | N8N | RPL | SAP |
|----|-----|-----|-----|-----|-----|-----|-----|
| **None** — tenants author their own CP/RG/FW; no seeded ISO/SOC2/K-법령 packs `[V-code]` | None shipped `[I]` | N/A | **320+ ready templates** (GDPR, ISO 27001, SOC 2, HIPAA…), customizable `[V]` | N/A | N/A | US federal/state employment-law content is maintained *for you* (wage, ACA, leave) `[V]` | Ships regulatory + control-framework content; large partner/rulebook ecosystem `[I]` |

### R3 — Obligation lifecycle, ownership & scoping

| US | FDN | SLK | TMS | ASN | N8N | RPL | SAP |
|----|-----|-----|-----|-----|-----|-----|-----|
| Typed status FSM DRAFT→ACTIVE→WAIVED→SUPERSEDED→ARCHIVED, owner_user, severity, review_cadence, **group/branch/site/team/role scope** `[V-code: mig 0148]` | Would be an object lifecycle + Actions; owner = property `[I]` | N/A | Improvement actions assignable to users w/ status `[V]` | Task owner + status + custom fields; no cadence/scope semantics `[I]` | N/A | Obligations tied to employee/jurisdiction automatically; no user-editable lifecycle `[I]` | Control/issue owners, workflow status, org-unit scoping — mature `[I]` |

### R4 — Control library & control testing / attestation campaigns

| US | FDN | SLK | TMS | ASN | N8N | RPL | SAP |
|----|-----|-----|-----|-----|-----|-----|-----|
| Control objects (type/cadence, evidence_requirements JSONB) + coverage links exist; **no test-execution / attestation run engine yet** `[V-code]` | No native control tests; could build as scheduled Automate condition→effect `[I]` | N/A | Improvement-action "implementation + testing work" tracked, manual `[V]` | Could fake via recurring tasks; no control-test semantics `[I]` | Could *drive* a test (call an API, check output) but stores no result model `[I]` | Automated eligibility/threshold "tests" (ACA hours, min-wage) run continuously `[V]` | **Process Control: continuous automated control monitoring**, scheduled tests, pass/fail feeding risk `[V]` |

### R5 — Findings / issue management & remediation (CAPA)

| US | FDN | SLK | TMS | ASN | N8N | RPL | SAP |
|----|-----|-----|-----|-----|-----|-----|-----|
| `governance_findings` (detector, severity, status, subject, idempotent) BUILT; remediation/CAPA workflow + UI not yet `[V-code: mig 0050]` | Findings = objects + Action-driven remediation state `[I]` | N/A | Communication-compliance alerts → review/resolve queue `[V]` | Strong: task = issue, rules automate remediation routing `[I]` | N/A | Flags violations + "tells you how to resolve them" (guided remediation) `[V]` | Audit Mgmt: findings + **corrective & preventive actions (CAPA)** tracked to closure `[V]` |

### R6 — Evidence collection & chain of custody

| US | FDN | SLK | TMS | ASN | N8N | RPL | SAP |
|----|-----|-----|-----|-----|-----|-----|-----|
| Copy/fixity metadata, custody events, evidence-readiness state, source-wired verify/hold operations, and evidence_bindings are code-evidenced. Object-lock deployment is unproved; RFC-3161/TSA is nullable and wire-pending, with no non-null exercised token proving timestamp custody. `[code: evidence/types.ts, mig 0148; TSA ceiling: nullable contract only]` | The cited lineage/provenance surface provides evidence of data flow; a comparable custody product was not established in this sample `[I]` | Slack Enterprise Grid only: eDiscovery preserves immutable msg/file copies via retention/hold `[V]` | eDiscovery Premium keeps immutable copies even after user delete `[V]` | Attachments + proofing; no fixity/custody `[I]` | N/A | Stores generated filings (1094-C/1095-C) as records; no custody chain `[I]` | Evidence attached to control tests/audits; no cryptographic custody chain `[I]` |

### R7 — Audit trail (tamper-evidence & coverage proof)

| US | FDN | SLK | TMS | ASN | N8N | RPL | SAP |
|----|-----|-----|-----|-----|-----|-----|-----|
| **Partial/DARK:** seal/verify and gap-detection code exists, but production sealing is OFF, the in-memory signer is not a trust root, NULL-org rows are excluded, and trusted tamper evidence awaits external signing plus out-of-band anchoring `[V-code: mig 0100/0101, audit-chain crate, app config]` | Comprehensive who/what/when/where audit logs, categorized `[V]` | Slack Enterprise Grid only: Audit Logs API (real-time, org-wide) `[V]` | Audit (Premium): 1-yr retention, custom retention policies `[V]` | Immutable audit log + API; 90-day retention `[V]` | Enterprise audit logging (workflow.updated etc.), ≥12-mo retention `[V]` | Payroll/HR change history; not a general tamper-evident chain `[I]` | Full audit trail across GRC; change docs on controls/risks `[I]` |

### R8 — Segregation of Duties (SoD) / four-eyes / access-risk analysis

| US | FDN | SLK | TMS | ASN | N8N | RPL | SAP |
|----|-----|-----|-----|-----|-----|-----|-----|
| Four-eyes (approver≠requester) enforced platform-wide (ontology commits, object overrides, leave, workflow versions); no **access-risk ruleset** analyzer yet `[V-code: appr/composeModel.ts]` | Object/property policies + proposal reviewers ≈ maker-checker; no SoD conflict matrix `[V]` | N/A | Purview roles + dual-approval on some flows `[I]` | N/A | SoD by env separation: builder≠approver≠deployer (dev/stg/prod) `[V]` | Approval chains in HR/payroll; no SoD conflict engine `[I]` | **Access Control: automated SoD-conflict detection**, risk ruleset, mitigating controls, periodic access certification `[V]` |

### R9 — Compliance calendar / review cadence / deadline scheduling

| US | FDN | SLK | TMS | ASN | N8N | RPL | SAP |
|----|-----|-----|-----|-----|-----|-----|-----|
| Cadence + next_review_on stored on obligations/controls; **no calendar UI or reminder engine** `[V-code: mig 0148]` | Automate schedule could fire reviews; no calendar surface `[I]` | N/A | Assessment due-dates tracked; no jurisdiction calendar `[I]` | Timeline/calendar + rules for recurring reviews `[I]` | Cron-schedule any check; no compliance-specific calendar `[I]` | **Reference: 2026 US compliance calendar**, auto-add key dates, proactive law-change alerts `[V]` | Scheduled control tests + audit planning calendars `[I]` |

### R10 — Risk register & scoring / heatmap

| US | FDN | SLK | TMS | ASN | N8N | RPL | SAP |
|----|-----|-----|-----|-----|-----|-----|-----|
| Severity/risk_level fields per obligation/regulation; **no aggregated risk register or heatmap** `[V-code]` | Aggregation/charts over risk objects possible in Workshop `[I]` | N/A | Compliance Manager compliance **score** (posture %) `[V]` | Portfolio status roll-ups; not risk-scored `[I]` | N/A | Flags risk of specific violations; no enterprise register `[I]` | **Risk Management: live enterprise heat map**, residual scoring fed by control tests + SoD `[V]` |

### R11 — Automation hooks (continuous control monitoring, evidence auto-capture)

| US | FDN | SLK | TMS | ASN | N8N | RPL | SAP |
|----|-----|-----|-----|-----|-----|-----|-----|
| Evidence can bind `workflow_run`/`audit_event` (auto-capture path exists); no CCM scheduler wired `[V-code]` | Automate: condition→effect, continuous/scheduled `[V]` | Discovery API feeds external DLP/CCM tools `[V]` | Communication-compliance policies scan continuously `[V]` | Rules automate task routing on triggers `[I]` | **This is n8n's core:** any trigger→evidence pull/control test; SoD-safe promotion `[V]` | Continuous eligibility/threshold monitoring on payroll data `[V]` | Process Control automated control monitors on live process data `[V]` |

### R12 — eDiscovery / legal hold / retention

| US | FDN | SLK | TMS | ASN | N8N | RPL | SAP |
|----|-----|-----|-----|-----|-----|-----|-----|
| Legal-hold custody stage on evidence objects; no cross-corpus eDiscovery search/export case tool `[V-code: evidence/types.ts]` | Retention/deletion governance across lifecycle `[V]` | Slack Enterprise Grid only: Discovery API + legal holds (preserve edited/deleted) `[V]` | **Cited case-workspace reference: eDiscovery Premium** with hold, immutable copies, and export `[V]` | eDiscovery via partner (Theta Lake) + audit API; not native `[V]` | N/A | N/A | Records retention on GRC objects; not a general eDiscovery suite `[I]` |

### R13 — Permissions model over compliance data

| US | FDN | SLK | TMS | ASN | N8N | RPL | SAP |
|----|-----|-----|-----|-----|-----|-----|-----|
| Current server/legacy authorization matrix + non-authoritative UI feature projection; compliance tables enforce org isolation with FORCE RLS. Cedar is target/shadow pending per-action enrollment, evidence, and promotion; current coexistence entries are `legacy_only`. `[V-code: mig 0148, authz.ts; ADR-0021 + docs/specs/cedar-pbac-coexistence-map.json]` | Row+cell object/property policies, mandatory markings `[V]` | Enterprise Grid roles (Legal Holds Admin etc.) `[V]` | Purview role groups per solution `[I]` | Enterprise+ tier + Service Accounts for audit/compliance `[V]` | RBAC + SSO on paid plans `[V]` | Role-based HR admin scoping `[I]` | Fine-grained GRC authorizations per org-unit/role `[I]` |

### R14 — Auditor-facing reporting / export

| US | FDN | SLK | TMS | ASN | N8N | RPL | SAP |
|----|-----|-----|-----|-----|-----|-----|-----|
| Audited Excel/KPI export exists platform-wide (PR#223); no compliance-pack (SoA/control-report) generator `[V-code]` | Workshop reports + object exports `[I]` | Compliance exports of all messages `[V]` | Auditor reporting from Compliance Manager `[V]` | Reporting dashboards; not audit-pack shaped `[I]` | N/A | Auditor-ready wage/ACA reports auto-generated `[V]` | Audit-ready evidence + control-report packs are a core deliverable `[V]` |

### R15 — Mobile / field attestation & Korean-B2B fit

| US | FDN | SLK | TMS | ASN | N8N | RPL | SAP |
|----|-----|-----|-----|-----|-----|-----|-----|
| Native Android/iOS field app + **위치추적 동의 FSM** wired as a product/policy control; applicable privacy, location-information, and employment duties require scenario review `[V-code: compliance/rest]` | Mobile viewer; no KR-law content `[I]` | Mobile app; global, no Korean employment-law model `[I]` | Teams mobile; Purview content is US/EU-centric `[I]` | Mobile app; no field-attestation compliance `[I]` | N/A (backend) | US-only employment content — **mismatch for KR** `[V]` | Global; KR-localization via SAP localization, heavy/costly `[I]` |

---

## 2. Per-vendor: "how they'd build OUR compliance module"

**Palantir Foundry `[I]`** — not presented as a GRC product in the cited surface; they'd model obligation/regulation/framework/control as
**ontology object-types + link-types**, mutate through **Actions**, gate with **object/property policies**,
and let **Automate** run condition→effect control monitors. Evidence = automatic **lineage/provenance**.
That target resembles our ontology grammar. Our source-wired fixity/WORM-status and hold seams are a
different custody substrate, but the open deployment and trust gaps prevent a legal or superiority claim.

**Slack `[I]`** — would not build obligation management at all; they'd deliver the **compliance
periphery**: Discovery API + legal holds + audit-logs API + compliance message exports, and let a partner
GRC tool own the model. Lesson: treat messenger/collaboration as an *evidence source* feeding our
evidence_bindings, not as the system of record.

**Microsoft Teams / Purview `[V]`** — a selected comparator for the intended UX. **Compliance Manager**:
pick from 320+ regulatory templates → assessment → controls → improvement actions assigned to owners →
compliance **score** posture. Plus eDiscovery Premium, communication-compliance scanning, Audit Premium.
Their version is template-driven and posture-scored. Our Korean-content direction and per-item technical
custody seams differ, but do not establish comparative superiority or production custody.

**Asana `[I]`** — would build "compliance as work": obligations = tasks in a portfolio, custom fields for
status/owner/cadence, **rules** for recurring reviews and remediation routing, immutable **audit-log API**
for the trail, partner (Theta Lake) for archiving/eDiscovery. Fast, friendly, but no control/coverage
semantics, no risk scoring, no evidence integrity — a lightweight tracker, not a GRC system.

**n8n `[V]`** — owns exactly one row brilliantly (**R11 automation**): any trigger → pull evidence, run a
control test, write the result, and it enforces **SoD by environment** (builder≠approver≠deployer). It
would be the **execution engine** *under* our control-testing/CCM layer, never the model or UI. Adopt the
pattern: our workflow_run→evidence_binding path is the same idea; wire a scheduler on top.

**Rippling `[V]`** — builds compliance as **embedded, automated, US-employment-law** intelligence: a
maintained **compliance calendar**, proactive law-change alerts, and continuous violation flagging
(min-wage, ACA, leave) with guided remediation — zero modeling by the customer. Brilliant for HR
compliance, but content is US-only (a hard mismatch for 근로기준법) and obligations aren't user-editable
objects. Steal the *calendar + proactive-alert UX*, not the closed content model.

**SAP (GRC / S/4HANA) `[V]`** — **the reference.** Access Control (automated SoD-conflict detection,
access certification, mitigating controls), Process Control (continuous automated control monitoring,
scheduled tests → pass/fail), Risk Management (live enterprise **heat map**, residual scoring fed by SoD +
control results), Audit Management (audit planning, findings, **CAPA** to closure). Their version of our
module is the full integrated loop: control test fails → raises the linked risk's residual score → issue →
corrective action → re-test. Heavy, expensive, SAP-centric, KR-localization costly. It defines the
capability ceiling we should reach with our lighter, ontology-native, deny-by-default grammar.

---

## 3. What we'd steal — ranked

Each: capability → selected cited reference → fit with our accepted target ontology-first / Cedar-PBAC / audit-chain grammar → cost.

1. **Continuous control monitoring + control-test result model (test→pass/fail→feeds risk)** → **SAP
   Process Control** `[V]`. Fit: perfect — a control-test is an Action producing an evidence_binding +
   a governance_finding on fail; our workflow_run→evidence path is 80% there. **Cost: L** (needs a
   scheduler, a result table, and the risk-scoring rollup).
2. **Regulatory content library — seeded framework/control templates (ISO 27001, SOC 2, K-법령 packs)**
   → **Purview Compliance Manager (320+ templates)** `[V]`. Fit: templates are just seed FW/control rows
   in our existing tables; ship a "clone template → tenant assessment" Action. Biggest UX unlock for
   least architectural risk. **Cost: M** (content authoring dominates, not code).
3. **Automated SoD-conflict analysis + periodic access certification** → **SAP Access Control** `[V]`.
   Fit: we have the four-eyes primitive, and target Cedar subject attributes can support a conflict ruleset
   after promotion; add evaluation over role assignments + a recurring certification campaign. Aligns
   with the accepted Cedar-PBAC direction without claiming current enforcement. **Cost: L.**
4. **Compliance calendar + proactive deadline/law-change alerts** → **Rippling** `[V]`. Fit: cadence +
   next_review_on already stored; add a calendar surface + reminder/notification job. Localize content to
   Korean statutory dates (전자결재 마감, 4대보험, 근로기준법 reviews). **Cost: M.**
5. **Risk register with residual scoring + heat map** → **SAP Risk Management** `[V]`. Fit: severity
   fields exist; add an aggregation object + Workshop-style heatmap, fed by control-test + finding
   results. **Cost: M.**
6. **Improvement-action / CAPA workflow (finding → assigned action → tested → closed)** → **SAP Audit
   Mgmt + Purview improvement actions** `[V]`. Fit: governance_findings + gov_approval_requests already
   exist; add a remediation FSM + owner assignment UI. **Cost: M.**
7. **Compliance posture score (a single readable % + trend)** → **Purview Compliance Manager** `[V]`.
   Fit: derive from control coverage + test pass-rate; a computed read, no schema change. **Cost: S.**
8. **eDiscovery case workspace (search across evidence/audit/messages → hold → export)** → **Purview
   eDiscovery Premium** `[V]`. Fit: we have source-wired hold/fixity seams; establish durable custody before adding a cross-corpus search+export case
   object. Pairs with our messenger + evidence stores. **Cost: L** (defer; niche until a legal event).
9. **Automation-as-execution-engine under CCM, with SoD-by-environment** → **n8n** `[V]`. Fit: reuse our
   workflow engine as the control-test runner; enforce builder≠approver≠deployer via existing four-eyes.
   **Cost: S** (pattern adoption, engine already exists).

### Korean B2B mismatches to design around

- **Rippling/Purview content is US/EU-centric** — our seeded packs must be **근로기준법 / 산업안전보건법 /
  개인정보보호법 / 4대보험** obligations, not GDPR/ACA. This is a Korea-specific content gap in the sampled global-product surfaces.
- **전자결재 culture** — our four-eyes/SoD primitive should present as a native 결재선 (sequential approval
  chain with delegation/전결), not a single maker-checker toggle. SAP/Purview approvals are flatter.
- **Group-company (법인/branch/site) scoping** — already in `compliance_obligations.scope_type`; keep it
  first-class in every steal above (a 그룹 obligation cascading to 계열사) — none of the 7 vendors model a
  Korean conglomerate hierarchy natively.

### Honest gap flag for the adversarial phase

Our compliance module is **substrate-partial and product-incomplete**: no CRUD REST, no UI, no content,
no test/calendar/risk engines. Every "steal" above assumes we first wire the obligation/framework/control
CRUD API + the Phase-C wave-2 UI surface (ledger item ⑦). Until then our column's strengths (R6 evidence,
R7 audit, R8 four-eyes, R13 permissions, R15 KR-fit) are real; its weaknesses (R2, R4, R9, R10) are total.

---

## 4. Sources (verified claims)

- SAP GRC (AC/PC/RM/Audit, SoD, heat map, CAPA): https://pathlock.com/learn/sap-grc-understanding-10-core-modules/ · https://onapsis.com/articles/sap-grc-ultimate-guide/ · https://gracker.ai/blog/sap-grc-risk-management-guide
- Purview Compliance Manager (320+ templates, assessments, controls, improvement actions, score): https://learn.microsoft.com/en-us/purview/compliance-manager · https://learn.microsoft.com/en-us/purview/compliance-manager-assessments · https://learn.microsoft.com/en-us/purview/compliance-manager-regulations-list
- Purview eDiscovery/hold/audit/comms-compliance: https://learn.microsoft.com/en-us/purview/edisc-hold-create · https://learn.microsoft.com/en-us/office365/servicedescriptions/microsoft-365-service-descriptions/microsoft-365-tenantlevel-services-licensing-guidance/microsoft-purview-service-description · https://learn.microsoft.com/en-us/microsoftteams/security-compliance-overview
- Rippling (compliance calendar, alerts, min-wage/ACA, guided remediation): https://www.rippling.com/resources/compliance-calendar · https://www.rippling.com/payroll-compliance · https://www.rippling.com/aca-compliance
- Palantir Foundry (audit logs, lineage/provenance, governance, certs): https://www.palantir.com/docs/foundry/security/audit-logs-overview · https://www.palantir.com/docs/foundry/security/data-protection-and-governance
- Slack (Discovery API, legal holds, audit logs, compliance exports): https://slack.com/help/articles/360002079527-A-guide-to-Slacks-Discovery-APIs · https://slack.com/help/articles/4401830811795-Create-and-manage-legal-holds
- Asana (immutable audit-log API, Enterprise+ compliance add-on, 90-day retention, Theta Lake): https://developers.asana.com/reference/audit-log-api · https://help.asana.com/s/article/audit-suite-api-and-integration-support
- n8n (SOC2, audit logging ≥12mo, data residency self-host, SoD by env): https://n8n.io/legal/security/ · https://n8nlab.io/blog/enterprise-n8n-security-hardening-guide
- Our console (code): backend/crates/platform/db/migrations/0148_create_compliance_domain.sql · .../0050_create_governance_findings.sql · .../0100_create_audit_chain_seals.sql · backend/crates/compliance/{domain,application,rest}/src/lib.rs · web/src/console/{evidence/types.ts,audit/AuditFeed.tsx,appr/composeModel.ts} · docs/program/console-program-ledger.md

---

## Cross-cutting lens findings (5 independent review lenses)

- **Task-flow:** money task = *run a compliance check / simulate a policy against the regulation*. Today partially wired — simulate via the policy canvas (~3 steps), real typed-policy eval pending. ServiceNow GRC's cross-module propagation means updating a control once reflects across every linked register/policy/audit (0 redundant re-testing); ours would touch each surface. **Steal:** cross-module propagation via the single ontology engine (a control IS an object; linked risks/policies/audits are link-types → one edit propagates for free). Cost **L**.
- **IA / layout:** nav `compliance` gated `INTEGRITY_ROLES + integrity_findings_read` (EXECUTIVE/SUPER_ADMIN; **ADMIN excluded by design** — a deliberate SoD choice); ties to `evidence/` + `audit/`. Integrity findings and partial audit seams are useful inputs, but cryptographic integrity is not production-proved. **GAP:** no **control→test→finding→remediation** master-detail workflow surface. **Steal:** control-library → test → finding → remediation master-detail → AuditBoard/ServiceNow [M]; automated control-test scheduling (reuse `automate` + `scheduled`) [M]; findings inbox routing to `appr` for remediation approval [S].
- **Data-model:** **weaker in surfacing, competitive in domain modeling** — obligation, regulation, and framework are published projected/read types over domain tables with typed FSMs and a regulation validity window. They still have **no as-of read fn, no ontology actions, and no web UI** (coverage-matrix: 0 refs). **Steal:** deepen the published types with control↔risk↔obligation links + attestation cycles [M]; add a domain-owned effective-dated regulation as-of read [S]; wire assessment/evidence-request cycles to EV- objects [M].
- **Governance:** **Behind** — compliance-as-a-product (the Vanta/Drata/OneTrust space) is our largest unbuilt governance module. Reusable fixity/hold, partial audit, Cedar authoring/simulation/shadow, and automation seams exist; durable custody and trusted anchoring remain open. **Steal:** control→test→evidence continuous-monitoring loop (Control–Test → schedule → Evidence; re-run on cron + diff; fail → finding) → Vanta/Drata (highest-value compliance build) [L]; cross-framework control mapping (one evidence ↔ many requirements) [M]; SAP-style SoD ruleset + mitigation-control library [L]; DSR/consent/RoPA workflow (PIPA / 개인정보보호법, legal-hold check before deletion) → OneTrust [L]; access-review / recertification campaigns [M].
- **Automation / extensibility:** partial evidence seams exist; production WORM/trusted anchoring, the continuous-test scheduler, and integration-sourced evidence collection do not. **Steal:** continuous control-test scheduler (schedule trigger + a "test" Action evaluating a predicate over instances → timestamped EV-) [M]; cross-framework mapping via many-many link types [M]; evidence-from-integration collectors (internal-source first) [M–L].

**Adjudication:** this re-audit classifies compliance as substrate-partial and product-incomplete: REST PARTIAL, UI not built, durable custody/trusted anchoring unproved, and R2/R4/R9/R10 total weaknesses. Its `gov_approval_requests = mig 0158` citation is correct (the data-model lens's earlier "0112" was the error, since corrected). R6/R12 Slack eDiscovery/hold and R7 Slack audit are Enterprise-Grid-only.
