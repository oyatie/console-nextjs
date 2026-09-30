# GOVERNANCE LENS — permissions · approval/SoD · audit · retention/holds

> **Benchmark evidence metadata**
> - Observation/revalidation date: 2026-07-19.
> - Sampled products/surfaces: Oyatie governance, RLS, Cedar, and evidence source; Palantir; AWS Cedar; ServiceNow GRC; SAP GRC; Workday; Vanta and Drata; Slack Enterprise; Microsoft Purview; Collibra and OneTrust; Retool, Windmill, and ToolJet.
> - Evidence modality: Fixed-target repository source plus live-checked public official documentation/product pages and explicitly labeled public secondary pages; hands-on product tenants, screenshot capture, deployment, activation, and production validation were not performed.
> - Scope/claim ceiling: Only the named pages, surfaces, and fixed-target source are in scope; no whole-product, current-production, provider-parity, universal-superiority, legal, tax, labor, deployment, activation, or production conclusion.
> - Legend: [V] = bounded external observation with a direct URL or same-document source-list entry; [E]/[code] = fixed-target repository observation; [I] = recommendation or inference. Every steal/adopt item is [I].

Independent pass. Yardstick = OUR source-evidenced stack (read from fixed-target `web/src/console/**` + `docs/program/console-program-ledger.md`). Every vendor claim labeled **[V]** (verified w/ URL) or **[I]** (inferred from known product patterns).

Rigor note: I did NOT read the draft matrices (independent context). Vendor set chosen per governance relevance, grounded in `docs/program/benchmark-brief.md`: **Palantir Foundry, AWS Cedar, ServiceNow (GRC), SAP (GRC Access Control), Workday, Vanta/Drata, Slack Enterprise Grid, Microsoft Purview, Collibra/OneTrust**. Retool/Windmill/ToolJet used for config-governance rows.

---

## 0. OUR governance stack — the yardstick (evidence-based)

Read from the tree, not memory:

- **Permissions target — Cedar PBAC.** `console/policy/*` and backend Cedar authoring/residual primitives establish the target object/property-policy model, but ADR-0021 explicitly does not switch live authorization. Current enforcement is legacy server-side permission/middleware plus PostgreSQL RLS where evidenced. Cedar remains target/shadow until an action is enrolled, shadow-proven, and promoted through `docs/specs/cedar-pbac-coexistence-map.json`.
- **Tenant isolation — evidenced FORCE-RLS tables**, `app.current_org` GUC, tested as real `console_rt` role (not BYPASSRLS superuser). `console-gate-rls-arming` + `tenant-isolation` CI gates prove those covered tables; they do not establish an every-table claim.
- **Approval / SoD / four-eyes.** Frontend `console/appr/*` = 전자결재 approval line (multi-node sequential, per-node state, comment-required, reason options, evidence-required attachment policy, object-link targets). SoD self-approval detected client-side (`lineHasSelfApproval`). Backend `crates/governance` (`0153_create_governance.sql`): `gov_approvals` `CHECK(approver_id <> requested_by)`, decider≠requester, `four_eyes_approved_conn` re-checkable in-tx (TOCTOU-safe), decide reads authoritative requester in-tx (spoofed-requester self-approval hole closed). The §16 gate-chain has an authority slot currently backed by the live server checks; Cedar is its target evaluator after promotion. Distinct-approver four-eyes applies on evidenced hold paths.
- **Audit surfaces.** `console/audit/AuditFeed`; append-oriented audit plus hash-chain seal/verify and gap-detection code are present. This is **partial/DARK**: production sealing defaults OFF, the in-memory signer is not a trust root, NULL-org rows are excluded, and real tamper evidence requires an external signer plus out-of-band anchor. The Cedar decision-log schema (`0159_create_cedar_decision_log.sql`) records calls through the Cedar evaluation endpoints; it does not prove that every current legacy authorization decision is logged. (Audit-chain seals live at `0100`/`0101`.)
- **Retention / holds / WORM.** `console/evidence/*` EvidenceCard exposes WORM/fixity/custody/legal-hold status. Backend `crates/docs/rest` has SHA-256 fixity metadata per copy, custody events, nullable RFC-3161 TSA (deferred), POST verify, and a fail-closed distinct-approver hold path. Object-lock deployment is not proved. ISO 15489 / OAIS / FRE 902(14) are framing, not readiness evidence.
- **Lifecycle governance.** `crates/governance` overrides: effective-dated versioned store, as-of query, draft-direct vs override(reason + four-eyes + before-audit), impact preflight, soft-archive gate (no hard delete). Config = governed ontology object (draft→approve→effective, rollback, as-of) not code constants.
- **Config-as-governed-object.** `support_slo_setting` + `console_view` seeded *through the engine* (staging v+1 / as-of); `console:configure` and `console:deploy` are target Cedar action names, not proof of live Cedar enforcement.

**Net posture:** our governance primitives include a live RLS tenant floor, Cedar target/partial infrastructure, four-eyes DB checks, source-wired fixity/hold operations, and a partial/DARK audit-chain seam. Cedar deny-by-omission is not yet universal live enforcement; durable WORM/object-lock custody is not proved. Remaining gaps include trusted signing/anchoring, SoD rulesets, continuous control monitoring, retention/disposition, DSR/consent, access recertification, and a reusable BP-style approval framework.

---

## Vendor governance one-liners (context for every module)

- **Palantir Foundry** — governance IS the platform: markings (cell-level, propagate down lineage) [V], purpose-based access, object/restricted-view granular policies (row/column by user attribute) [V]. Closest philosophical peer to us. [V] https://www.palantir.com/docs/foundry/security/markings , https://www.palantir.com/docs/foundry/object-permissioning/object-security-policies
- **AWS Cedar** — the PBAC engine we build on; policy templates [V] ([templates](https://docs.cedarpolicy.com/policies/templates.html)), explicit deny overriding allow [V] ([policy effects](https://docs.cedarpolicy.com/policies/syntax-policy.html)), human-readable schema format [V] ([schema format](https://docs.cedarpolicy.com/schema/human-readable-schema.html)), and schema-based policy validation [V] ([policy validation](https://docs.cedarpolicy.com/policies/validation.html)). Partial evaluation remains [I] via benchmark-brief §2f rather than a direct same-document official source.
- **ServiceNow GRC** — approval workflows w/ SoD validation + step-up MFA on high-risk; Audit Management module; domain separation. [V] https://www.servicenow.com/docs/r/governance-risk-compliance/audit-management/c_GRCAudits.html
- **SAP GRC Access Control** — the SoD **ruleset** benchmark: predefined global risk library, mitigation/compensating controls w/ test plans + monitor frequency, cross-system SoD. [V] https://help.sap.com/docs/SUPPORT_CONTENT/grc/3362387180.html , https://pathlock.com/blog/sap-grc/sap-access-control/
- **Workday** — Business-Process framework (definition→steps→condition/action/routing→commit) + configurable security groups + effective-dating; SoD flagged at role-assignment/transaction. [V] https://www.kainos.com/insights/blogs/guide-how-to-win-at-auditing-segregation-of-duties-in-workday
- **Vanta / Drata** — control→test→evidence continuous monitoring (Vanta hourly, Drata daily), auto evidence collection (Vanta ≤90%), Audit Hub (auditor works in-platform). [I] https://drata.com/products/compliance/monitoring-and-tests , https://www.vanta.com/compare/drata
- **Slack Enterprise Grid** — org-wide legal holds override retention; Discovery API (metadata + content, org-level, approved partners); native DLP tombstoning; granular retention per content type. [I] https://viewexport.com/post/legal-holds-slack , https://www.strac.io/blog/slack-discovery-api
- **Microsoft Purview** — retention labels (item/container), **records / regulatory-record** immutability lock, retention-schedule + disposition review, eDiscovery holds (case/custodian scoped) vs Litigation Hold (coarse), Audit (Premium). [V] https://learn.microsoft.com/en-us/purview/records-management , https://learn.microsoft.com/en-us/purview/edisc-hold-types-mailboxes
- **Collibra / OneTrust** — Collibra: BPMN stewardship/approval chains, glossary, retention, DSAR workflow. OneTrust: DSR automation (ID-verify, retrieve/delete, **legal-hold check**, redaction), retention-policy enforcement + audit-ready docs, consent/RoPA. [V] https://www.onetrust.com/products/data-subject-request-dsr-automation/ , https://productresources.collibra.com/docs/collibra/latest/Content/Protect/co_protect-scenarios.htm

---

## Per-module governance matrices

Legend: **Ahead** = stronger for the named capability in this cited sample / **Par** / **Behind** = the named cited product surface is stronger. These are scoped planning assessments, not market-wide rankings. Each module ends with a ranked **STEAL** list (capability → selected cited reference → ontology-fit → cost S/M/L). **[I]**

### 1. overview

Governance surface = "what governance state can a user *see* at a glance for the whole tenant."
- **Foundry** [I]: home surfaces respect markings; no dedicated governance overview. **Vanta/Drata** [I]: dashboard = control-pass %, failing tests, evidence freshness — a genuine *governance posture* landing page. **ServiceNow** [I]: GRC risk/compliance dashboards.
- **OURS:** overview shell (ShellDock, scope×period) — current visibility comes from a non-authoritative UI projection plus legacy server authorization and evidenced RLS. No aggregate "governance posture" tile (pending approvals across tenant, controls failing, holds active, overdue recerts).
- **Verdict: Behind on governance-posture summary; current enforcement is legacy server/RLS, with no Cedar lead claimed.** **[I]**
- **STEAL:** ① governance-posture strip (pending four-eyes count · active legal holds · shadow Cedar denials · overdue lifecycle reviews) → Vanta/Drata → target widget over gov_approvals/holds/decision_log after a real query binding exists → **S**. ② "why can't I see X" affordance (deny-reason on request) → Cedar → **M** (needs safe non-leaking denial copy and promotion evidence). **[I]**

### 2. dashboard

- **Foundry** [I]: dashboards are objects → inherit governance; widget only shows permitted rows (restricted views). **ServiceNow/Workday** [V/I]: report access via security groups.
- **OURS:** the source-observed Dashboard calls server-produced KPI/operations/attendance/payslip rollups under legacy server authorization and evidenced RLS. The generic `ontQuery` widget binding is wire-pending, and no live Cedar residual-filter pipeline is proved. §4-24 honest-scaling is implemented separately.
- **Verdict: Behind Foundry on promoted object/property-filtered aggregates; current scope is server-authorized rollups, not Cedar-residual filtering.** **[I]**
- **STEAL:** ① aggregate-suppression threshold (k-anonymity: hide a count when <k rows so a filtered aggregate can't fingerprint an individual) → Foundry restricted-view spirit → add after the target residual layer is enrolled and promoted → **M**. ② per-widget "governed-by" chip only after a real Cedar decision shapes the number → **S**. **[I]**

### 3. finance

Governance = SoD on the money path + immutable ledger + approval thresholds.
- **SAP** [I]: 3-way match (PO↔GR↔Invoice via GR/IR clearing, tolerance-gated block), balanced-document GL (Σdr=Σcr), forward-only WO status w/ cost-posting gates; GRC SoD ruleset flags "create-vendor + pay-vendor" conflicts. **Workday** [I]: BP approval chains w/ threshold routing on spend.
- **OURS:** the mounted finance-GL slice has voucher lifecycle routes, a database/domain balance gate, posted immutability, append-only lines, reversals, FORCE RLS, and audit seams. It still lacks 3-way-match reconciliation, amount-threshold approval routing, a finance SoD ruleset, proven period-lock integration, deployment evidence, and mature accounting breadth.
- **Verdict: Partial** — the balanced-document invariant exists; the cited SAP/Workday surfaces provide greater finance-specific governance depth and operational evidence.
- **STEAL:** ① amount-threshold approval routing (>₩X → +1 approver level) as a typed policy predicate on the appr line → Workday BP → ontology-fit: reason/routing already in ApprTemplate; add threshold condition → **M**. ② balanced-document invariant (Σdr=Σcr as a publish-gate on voucher instances) → SAP FI → **M**. ③ 3-way-match as a reconciliation object-type (clearing-account nets-to-zero-in-tolerance = lifecycle gate) → SAP MM → **L**. ④ finance SoD ruleset seed (create-vendor vs approve-payment vs post-GL conflict pairs) → SAP GRC → **M** (see cross-module SoD-ruleset finding). **[I]**

### 4. people (HR)

- **Workday** [I]: BP framework governs Hire/Terminate/Comp-Change — who can *initiate/approve/view* each; effective-dated worker records; SoD flags at role assignment. The HR-governance benchmark. **Greenhouse** [I]: structured hiring, source-tracking on Application (defensible record).
- **OURS:** `app/src/hr.rs` + leave FSM; employee directory branch-scoped (`EmployeeDirectoryManage`). Approval via appr line. Effective-dating via governance store. **No BP-framework generality** — approval routing is per-workflow-definition, not a reusable definition→steps→condition/routing→commit model applied uniformly to every HR event.
- **Verdict: Partial/local-fit, Behind on the reusable BP abstraction.** Korean note: annual-leave rights and notice/receipt duties must be modeled from the applicable statute, while approval routing, half/quarter-day slicing, and per-shift handling may be workplace policy or product controls rather than standalone legal mandates. The repository has round-labelled notice/receipt fields and inbox migration `0119`, but request creation, statutory timing/sequence enforcement, and closed-loop E2E remain unproved. **[I]**
- **STEAL:** ① Workday BP-framework generalization: one `BusinessProcessDefinition` object-type (ordered steps: approval/todo/checklist/integration/notification + condition + routing + mandatory commit step) that every HR action instantiates → Workday → ontology-fit: this IS our appr line generalized + governance commit step; huge leverage → **L**. ② initiate/approve/**view** as three separate Cedar actions per BP (Workday splits them) → Cedar → **S**. **[I]**

### 5. leave (연차/휴가)

- **Workday** [I]: absence = BP instance, effective-dated, approval-routed. Global tools stop here.
- **OURS:** `console/leave/*` + `hr.rs` call request/balance and decision/promotion endpoints in source, a no-self-decide check, promotion rounds, and receipt-status fields. Request creation, statutory timing/sequence enforcement, and closed-loop E2E are absent.
- **Verdict: PARTIAL/local-fit.** The round-labelled 연차촉진 and receipt substrate is Korea-specific, but it is not Ahead until the complete request-to-receipt loop is implemented and proved. **[I]**
- **STEAL:** ① complete the 수령확인 notice/receipt path as a reusable governed acknowledgment object-type with audit and closed-loop E2E → OneTrust DSR receipt pattern → **S**. ② add statutory timing/sequence enforcement and promotion-round SLA/escalation → ServiceNow escalation → **M**. **[I]**

### 6. support

- **ServiceNow** [I]: ITSM approval + SoD on high-risk changes; SLA/OLA; audit trail on every ticket. **Zendesk** [I]: role-based macros/triggers, audit log.
- **OURS:** `console/support/*` — §4-26 SLO (not SLA) as configurable setting object w/ pendingRev staging; §4-11 stat strip. Governance = ticket state audited, SLO setting is a governed-config object (draft→approve). Support is a lighter governance surface.
- **Verdict: scoped design difference.** SLO-as-governed-config-object makes a config change four-eyes-able; the cited ServiceNow SLA surface describes admin configuration rather than this draft/approve gate.
- **STEAL:** ① SLO-threshold four-eyes is **already enforced on the client** (`approveSloRevision`, `web/src/features/support/slo-settings.ts:118-122`, blocks self-approval when `pending.stagedById === approverId`, approver≠requester) — the remaining work is **backend enforcement**: route the SLO revision approve through `gov_approvals` so the gate is server-side, not FE-only → our own config-object model → **S**. (Corrects an earlier draft that framed this as not-yet-built; support.md is right.) ② high-risk ticket → step-up (SoD validation before a privileged support action, e.g. impersonation/data-export from a ticket) → ServiceNow → ontology-fit: Cedar action + guardrail gate → **M**. **[I]**

### 7. evidence

- **Purview** [I]: retention labels → record/regulatory-record immutability; retention-schedule + **disposition review** (approver signs off before delete). **OAIS/ISO 15489/FRE 902(14)** = the standards. **Slack/Vanta** [I]: evidence export/retention.
- **OURS:** `crates/docs/rest` + EvidenceCard provide real list/detail/verify/hold source paths, SHA-256 fixity fields, custody events, and distinct-approver hold controls. Object-lock deployment proof is pending; RFC-3161 is nullable; production audit sealing is OFF; the in-memory signer is not a trust root; NULL-org rows are excluded; external signing plus an out-of-band anchor is required.
- **Verdict: Partial/DARK on durable custody; Behind on retention-schedule + disposition-review.** The current source supports useful fixity and hold seams, but it does not justify Purview-class, superiority, universal coverage, or production WORM conclusions. **[I]**
- **STEAL:** ① retention-schedule + disposition-review (label sets retention window → at expiry, a governed four-eyes disposition review before soft-archive/dispose) → Purview records-management → ontology-fit: retention-label = governed setting object; disposition = lifecycle transition + four-eyes; we already have the soft-archive gate → **M**. ② finish RFC-3161 TSA anchoring (real evidentiary timestamp) → OAIS PDI fixity → **M**. ③ regulatory-record lock tier (stricter than record: even admins can't release the hold) → Purview → **S**. **[I]**

### 8. object-platform (ontology)

- **Foundry** [I]: markings propagate cell→downstream via lineage; object-security-policies (granular row/column by attribute); Ontology-Manager Git-style schema governance (branch/proposal/merge-check/changelog). THE benchmark.
- **OURS:** ontology engine (registry + instances + as-of + traverse); schema-lifecycle draft→review_pending→published(immutable/content-addressed v+1)→superseded→retired; Cedar object/property policies and residual lowering remain target/shadow; **as-of reconstruction + append-only revisions + fixity chain** are separate current primitives.
- **Verdict: Strong schema governance but Behind Foundry on live field-policy enforcement and marking propagation.** Residual query filtering is not a universal live capability until separately enrolled and promoted. **[I]**
- **STEAL:** ① sensitivity **markings that propagate down link/derivation lineage** (mark an instance/property → derived analytics/exports inherit the eligibility gate) → Foundry markings → ontology-fit: marking = property + a forbid-policy keyed on it; propagation follows link-types → **L** (highest-value governance feature we lack). ② purpose-based access (bind a grant to a stated purpose, audited) → Foundry → **M**. **[I]**

### 9. policy

- **AWS Cedar** [I]: policy templates (parameterize principal/resource), schema-validate in CI, partial eval. **OPA** [I]: rego + decision logs. **ServiceNow** [I]: UI/Data Policy tri-state field rules as records enforced client+server.
- **OURS:** `console/policycanvas/*` — no-code P→R→A→Effect blocks + typed predicates + simulator, per-policy pendingRev, four-eyes publish, review FSM. Backend `cedar_pbac/authoring.rs` strict-validates and exposes simulate/authorize endpoints. This proves governed authoring/evaluation, not promotion of live application routes.
- **Verdict: scoped design difference.** Policy change is four-eyes-gated, simulated, and versioned; raw Cedar does not itself provide that change-governance workflow. No broader product ranking follows. **[I]**
- **STEAL:** ① **policy templates** (parameterized reusable policy: "manager-approves-own-team" instantiated per org) → Cedar templates → ontology-fit: template = policy doc w/ typed holes bound at attach → **M**. ② ServiceNow tri-state field rules (Mandatory/Visible/Read-Only ∈ {true,false,leave-alone}, `Order`, reverse-if-false) as a governed-config layer distinct from Cedar authz → ServiceNow Data/UI Policy → ontology-fit: field-rule record enforced client + server over same predicate grammar → **M**. ③ CI schema-validation of every policy (fail build on invalid) → Cedar → **S** (we validate at authoring; add a gate). **[I]**

### 10. automate

- **Foundry Automate** [I]: object-monitor → effect (= our model). **Temporal** [I]: durable append-only event history, deterministic replay, effectively-once. **Windmill** [I]: `permissioned_as ≠ created_by` (config runs as a defined principal, not last-editor) — anti-escalation. **n8n** [I]: execution logs.
- **OURS:** `console/workflows/*` + workflow-studio REST — branch canvas, simulator, runLog, four-eyes publish, effect=ontology-action; monitors-as-definitions. Humans and automation share a declarative Action shape, but current execution remains under legacy server guards. Cedar inheritance is target/shadow until each action is promoted.
- **Verdict: Partial.** The shared Action shape is promising, but no universal live Cedar gate or no-shadow-privilege guarantee is proved. Windmill's explicit `permissioned_as` remains a security requirement.
- **STEAL:** ① `permissioned_as` — automation/workflow carries its own effective-principal, Cedar-evaluated at execution (not the author's live grants) → Windmill → ontology-fit: definition object gets a `runs_as` principal attribute → **M** (real security fix, not polish). ② durable event-history/replay for automation runs (audit + effectively-once) → Temporal → **L** (we have runLog; full replay is heavier). **[I]**

### 11. comms (messenger/mail)

- **Slack Enterprise Grid** [I]: org-wide legal hold overrides retention; Discovery API (org-level, metadata+content, approved partners); native DLP tombstone; per-content-type retention. **Purview** [I]: comms-compliance (supervisory review), journaling, eDiscovery on Teams/Exchange. **Gmail/Vault** [I]: retention + hold + export.
- **OURS:** custom Rust mail + messenger (channels/mute/presence/ack/quoted-replies, FORCE-RLS). Auditable in-app chat, **no E2EE by design**, so message content is server-accessible in principle; that fact alone does not prove retention, indexing, export, hold, or eDiscovery. BUT the ledger's own backlog lists **mail compliance (litigation hold · journaling · e-discovery · delegation-PBAC · outbound DLP) as NOT-yet-built.** Messenger has no retention/hold/eDiscovery.
- **Verdict: Behind** — comms governance (hold/retention/eDiscovery/DLP) is the biggest coverage gap vs Slack/Purview. Existing append-oriented, evidence-hold, and partial audit seams may be reused, but durable WORM and trusted anchoring are still open; the comms-specific surfaces do not exist. **[I]**
- **STEAL:** ① litigation hold on messenger/mail (hold overrides retention; hold = governed object, four-eyes release) → Slack → ontology-fit: reuse evidence-hold model (already distinct-approver four-eyes) applied to message copies → **M**. ② retention policy per channel/content-type (governed setting object) → Slack/Purview → **M**. ③ eDiscovery export (scoped by custodian/date, audited, watermarked per §13.1) → Slack Discovery / Purview eDiscovery → **L**. ④ outbound DLP (tombstone on sensitive-pattern match, DLP-admin review) → Slack DLP → **M** (ties to §13 egress gate we already have). **[I]**

### 12. appr (전자결재)

Core governance module. **This is where Korean context matters most.**
- **ServiceNow/Workday/SAP** [V/I]: approval chains w/ SoD validation, delegation, escalation, threshold routing. Global "approval workflow." But **none model 전자결재 natively** — sequential 결재선 (기안→검토→합의→전결→후결), 전결 (delegated final authority), 대결 (acting approval), 후결 (post-hoc ratification), 반려 (return-for-revision, not reject) are Korean-org-specific.
- **OURS:** `console/appr/*` — approval line (multi-node sequential, per-node state incl. `returned`/`skipped`), reason options, evidence-required, object-link targets, SoD self-approval block, idempotency, **post-finalization rejection + compensation** (반려 after finalize). Backend gov_approvals CHECK + in-tx four-eyes. **Source-present 전자결재 shape provides a local-fit distinction that global vendors force-fit.**
- **Verdict: local-fit distinction in this sample; gap on reusable abstraction and delegation depth.**
- **STEAL:** ① **delegation / 전결·대결** (approver delegates authority for a window; acting-approver records who they act for; audited) → Workday delegation + SAP → ontology-fit: delegation = a governed grant object w/ TTL + Cedar principal-attribute → **M** (already in backlog as break-glass TTL tokens — align). ② 합의 (parallel co-sign, not just sequential) node type on the appr line → 전자결재 norm → **S** (line already supports node states; add parallel-group). ③ escalation on stalled node (auto-notify/reassign after SLA) → ServiceNow → **M**. ④ SoD **ruleset** on the approval line (block a line where the same person appears at two conflicting roles, or where approver mitigates an open SoD violation) → SAP GRC → **M**. **[I]**

### 13. field

- **SAP PM** [I]: WO status machine (CRTD→REL→TECO→CLSD, forward-only, cost-posting gates). **ServiceNow FSM** [I]: mobile approvals, audit.
- **OURS:** WO- FSM and field check-in/WO/일지/급여 paths have evidenced legacy server checks, audit seams, and selected guardrails. Cedar action gating remains target/shadow; not every field action is proved checklist-gated or Cedar-enforced.
- **Verdict: Partial/local-fit.** The guardrail design is strong, but current coverage must be evaluated per action and promoted separately.
- **STEAL:** ① offline-approval integrity (field approvals captured offline must carry signed device-context + sync-time audit, tamper-evident) → (no directly matched reference was found in the cited sample; use the local hash-chain as the design baseline) → ontology-fit: extend audit chain to field-captured events → **M**. ② WO cost-posting gate (block cost booking after TECO-equivalent) → SAP PM → **S** (FSM already forward-only; add the gate predicate). **[I]**

### 14. compliance

- **Vanta/Drata** [I]: control→test→evidence continuous monitoring (hourly/daily), auto-evidence, cross-framework mapping (one evidence item ↔ SOC2∩ISO), Audit Hub. **ServiceNow GRC** [I]: policy/risk/control lifecycle, audit engagements. **SAP GRC** [I]: SoD ruleset + mitigation controls w/ test-plan + monitor frequency. **OneTrust** [I]: DSR/consent/RoPA.
- **OURS:** CP-/RG-/FW- compliance object types (some MISSING per coverage-matrix — RG- niche-seedable), typed-policy real-eval pending, PIPA consent object type in default catalog (planned). We have evidence/fixity/hold seams, partial/DARK audit code, and target/shadow Cedar infrastructure, but no production WORM proof, continuous-control-monitoring loop, control→test→evidence model, cross-framework mapping, or DSR/consent workflow.
- **Verdict: Behind** — compliance-as-a-product (the Vanta/Drata/OneTrust space) is our largest unbuilt governance module. Everything to build it exists as primitives. **[I]**
- **STEAL:** ① control→test→evidence continuous-monitoring loop (Control *–* Test → (integration/query/schedule) → Evidence; re-run on cron + diff; fail → finding) → Vanta/Drata → ontology-fit: Control/Test/Evidence = object-types, Test-run = automation firing on a schedule → Evidence instance; durable custody remains a separate prerequisite → **L** (highest-value compliance build). ② cross-framework control mapping (one evidence ↔ many requirements) → Vanta → ontology-fit: link-type Requirement *–* Control → **M**. ③ SAP-style SoD ruleset + mitigation-control library (predefined conflict pairs, compensating control w/ test-plan+frequency) → SAP GRC → **L** (see cross-module finding #1). ④ DSR/consent/RoPA workflow (PIPA — Korea's GDPR-equivalent: 개인정보보호법) w/ legal-hold check before deletion → OneTrust + our hold model → ontology-fit: DSR = governed workflow object, deletion routes through our soft-archive + hold-check gate → **L**. ⑤ access-review / recertification campaigns (periodic "does this person still need this grant" → approve/revoke, audited) → SailPoint/ServiceNow → ontology-fit: campaign = automation over principal-attributes + four-eyes revoke → **M**. **[I]**

---

## TOP-10 CROSS-MODULE GOVERNANCE FINDINGS (ranked) **[I]**

1. **SoD is enforced (self-approval CHECK) but there is no SoD RULESET.** SAP GRC's whole value is a *predefined library of conflicting-permission pairs* (create-vendor vs pay-vendor, initiate vs approve) + mitigation controls w/ test-plans. We block one person approving their own request; we don't detect "this person holds two roles that together enable fraud." **Build a governed SoD-ruleset object-type (conflict pairs over Cedar actions) + mitigation-control library.** Touches appr/finance/people/compliance. [V SAP] — **L, highest governance ROI.** **[I]**

2. **Compliance-as-continuous-monitoring is entirely unbuilt.** Control→Test→Evidence + cron-diff (Vanta hourly/Drata daily) + cross-framework mapping is the one place a specialist vendor decisively leads. Existing evidence, automation, and partial audit seams reduce the modeling work, but production WORM and trusted anchoring remain infrastructure prerequisites. **Model Control/Test/Evidence as object-types; a scheduled automation = a test-run producing an evidence instance.** [V Vanta/Drata] — **L.**

3. **Comms governance (hold/retention/eDiscovery/DLP) is the biggest coverage gap vs Slack/Purview** — and the ledger already flags it as unbuilt. The no-E2EE choice plus append-oriented and evidence-hold seams make messenger/mail a plausible reuse path, but production WORM/object-lock is not proved. Add litigation hold, per-channel retention, and eDiscovery export only with explicit durable-custody evidence. [V Slack/Purview] — **M–L.** **[I]**

4. **Foundry's marking-propagation-down-lineage is the one object-governance feature we lack.** Our residual lowering is target/shadow and not universal live query filtering; Foundry marks a source cell and every derived dataset inherits the eligibility gate. For a conglomerate platform (민감정보 flowing 법인→branch→analytics) this is the difference between "filtered on read" and "sensitivity travels with the data." **Marking = property + forbid-policy; propagate along link-types.** [V Foundry] — **L.**

5. **Retention/disposition and durable-custody proof are both missing.** Source-wired hold/fixity operations exist, but object-lock deployment and trusted anchoring are unproved; Purview's *scheduled retention window → disposition-review (four-eyes) → governed dispose* is also absent. Add a retention-label setting object only after the storage/trust boundary is explicit. [V Purview] — **M.**

6. **Workday's Business-Process framework should generalize our per-workflow approval into one reusable governed abstraction.** Today approval routing is defined per workflow-definition; Workday models *every* mutating event as `definition→steps(approval/checklist/integration/notify)→condition/routing→mandatory commit`. Our appr line + governance commit step is 80% there. Splitting initiate/approve/**view** into three Cedar actions per BP is a free governance win. [V Workday] — **L (leverage), S (the 3-action split).**

7. **Automation may run with author privileges — adopt Windmill's `permissioned_as ≠ created_by`.** If a workflow/automation executes with its author's live grants, a departed/demoted author's automations become a privilege-escalation and stale-authority hole. Give each definition its own effective-principal, Cedar-evaluated at run time. This is a security fix, not polish. [V Windmill] — **M.** **[I]**

8. **No access-review / recertification loop.** Every mature governance stack (SailPoint, ServiceNow, SAP) periodically asks "does this principal still need this grant?" Current grants remain in the legacy authorization model; the target Cedar principal-attribute model also needs expiry and recertification. A campaign = automation over principal attributes → four-eyes revoke. Pairs with the break-glass TTL-token backlog item. [I/V ServiceNow] — **M.**

9. **Governance posture is invisible at the top level.** Vanta/Drata/ServiceNow all lead with a posture dashboard; we never summarize pending four-eyes, active holds, shadow Cedar denials, overdue reviews, or failing controls. Build a real server-backed strip first; `ontQuery` and Cedar decision inputs remain target/shadow until wired and promoted. [V Vanta] — **S.**

10. **Korean-statutory governance is a differentiated target — protect and extend it.** Source-present primitives cover delegate-finalize/대행, 반려, 연차촉진 rounds, 수령확인/노무수령거부, per-shift 근로계약, and PIPA/개인정보보호법 scaffolding. Native 전결/대결/후결, 합의 parallel co-sign, governed receipt confirmation, and PIPA DSR with legal-hold checks remain builds, not source-proved complete semantics. [V/I across appr/leave/compliance] — **M each, strategic.**

---
Sources (verified): Purview records-mgmt/holds (learn.microsoft.com/purview), SAP GRC SoD (help.sap.com/grc, pathlock.com), Foundry markings/object-policies (palantir.com/docs/foundry), ServiceNow GRC (servicenow.com/docs), Workday SoD/BP (kainos.com), Vanta/Drata monitoring (drata.com, vanta.com), Slack Grid holds/discovery (viewexport.com, strac.io), Collibra/OneTrust DSR (onetrust.com, collibra.com). OUR column: `web/src/console/{appr,policy,audit,evidence,lifecycle,objectcard}`, `docs/program/console-program-ledger.md`.
