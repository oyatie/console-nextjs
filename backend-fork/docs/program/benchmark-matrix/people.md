# Benchmark Matrix — module: **people** (HR core: employee lifecycle · org · payroll runs · attendance/근태)

Fixed-target source observation only; no browser, deployment, activation, or production-runtime validation was performed.

Columns: **Our console** + 7 vendors (Rippling, SAP [SuccessFactors + S/4HANA HCM], Asana, Palantir Foundry, Slack, Microsoft Teams, n8n).
Most-relevant reference vendors for this module: **Rippling** (the HRIS reference), **SAP SuccessFactors** (enterprise/global-payroll reference), **Asana** (onboarding-workflow reference).
Rigor: every vendor claim is **[V]** verified (source URL) or **[I]** inferred (reasoned from known product patterns, honestly labeled).

---

## 0. Vendor relevance triage (never force-fit)

| Vendor | Plays in HR core? | Verdict |
|---|---|---|
| **Rippling** | Yes — HRIS is the product's spine (records, onboarding, payroll, T&A). | Full column. Reference. |
| **SAP SuccessFactors** | Yes — Employee Central = enterprise HRIS; ECP = global payroll; Time Off/Time Sheet = 근태. | Full column. Reference. |
| **Asana** | Partial — no HRIS/payroll; owns the *onboarding/HR-request workflow* layer via Forms+Rules. | Column, scoped to lifecycle/onboarding + automation rows. |
| **Palantir Foundry** | **N/A** for HR core — a data/ontology platform, not an HR module. Contributes only as an *architectural mirror* (ontology-first, which is our own grammar). Rows where relevant: IA, extensibility, analytics. | Scoped N/A. |
| **Slack** | **N/A** for HR core — collaboration surface. Contributes only as a *self-service/approval front-door* (Workflow Builder, approval apps). Rows: automation, approvals, self-service. | Scoped N/A. |
| **Microsoft Teams** | **N/A** for HR core — same as Slack; contributes via Approvals app + Viva/Shifts adjacency (Shifts touches 근태). Rows: attendance, approvals, self-service. | Scoped N/A. |
| **n8n** | **N/A** for HR core — general workflow-automation engine, no employee record. Contributes only as an *integration/automation fabric* between HR systems. Rows: automation hooks, extensibility. | Scoped N/A. |

For the four scoped-N/A vendors, cells appear only in the rows where they genuinely contribute; elsewhere read as "N/A — not an HR system."

---

## 1. Our console — evidence-based state (grepped, not asserted)

Sources: `backend/app/src/hr.rs` (10,288 LoC), `backend/crates/payroll/domain/src/lib.rs`, `web/src/console/{leave,lifecycle,shell/nav.ts,modules/moduleScreens.ts}`, `docs/program/console-program-ledger.md`.

**Backend (real REST + domain):**
- Employees: list, CSV **import pipeline** (preview → dry-run → apply, with provenance), CSV export, per-employee lifecycle-events (`/api/v1/employees/import/{run_id}/{dry-run,apply}`, `/employees/{id}/lifecycle-events`).
- Org: `/api/v1/hr/org-chart`, readiness-summary.
- 근태/attendance: import pipeline (preview/dry-run/apply/summary) + `/attendance-records/me` self-service check-in create + org attendance-records list + attendance-summary.
- Leave: request-decision FSM with **decider≠requester** SoD; source-observed request/balance reads and source-wired decision/promotion calls; plus a round-labelled notice/receipt substrate that accepts `1|2` and records idempotent receipt-gated notices. It does not enforce statutory timing or round sequencing, and refusal does not prove prior round `2`.
- Offboarding: absence-exit-dashboard, exit-cases (report/confirm/**approval-draft** → 전자결재 AP- object).
- **Payroll kernel** (`console_payroll_domain`): statutory **4대보험** contribution rates (연금/건강/고용/산재) as ppm with effective-period versioning, **national-pension base limits**, **minimum-wage** table, **NTS withholding** rows, payroll-draft builder, **severance-pay** draft enforcing the max-of-average-vs-ordinary-wage floor — *mandatory field, compile-error on omission* — and a **release gate** requiring golden-case + professional validation. ⚠️ **Citation fix:** the governing provision is **근로기준법 제2조제2항** (the Act: "평균임금이 통상임금보다 적으면 통상임금액을 평균임금으로 한다"); 근로기준법 **시행령** 제2조 instead governs *periods excluded from average-wage calculation* (수습·휴업·출산휴가 등). The code comment (`payroll/domain/src/lib.rs:149,553`) currently mis-cites this as 시행령 §2② and should be corrected to 제2조제2항.
- Authz: branch-scoped `EmployeeDirectoryManage` and deny-by-default advisory feature projection; live enforcement remains legacy server-side plus evidenced RLS, with Cedar/PBAC target/shadow only. Hash-chain audit infrastructure exists, but universal people-event coverage is not claimed.

**Frontend:**
- Nav groups are mounted in source: **hr** (directory/org/review) + **payroll** group (payroll/attendance/leave/benefit).
- `leave/LeaveConsole.tsx` has deep implemented UI: drillable stat bar, 내 연차 inputs, 팀 결재함, round-labelled 촉진 controls, and 인원별 연차 원장. LeaveConsole calls the request/balance and decision/promotion endpoints in source; only ledger rows are **objDrag sources → 3-layer ObjectCard** pins. Request creation remains a non-submitting link, and closed-loop E2E is absent.
- `lifecycle/LifecycleCard.tsx` = generic 3-layer ObjectCard (semantic/kinetic/dynamic; FSM transitions, holds, as-of history) wired to real BE-LC REST.

**Honest gaps (from ledger, dated 2026-07-10/11):**
- payroll & attendance **UI screens are stubs** ("wire-pending"); LeaveConsole has source-observed reads and source-wired decision/promotion calls, but request creation, statutory timing/sequence enforcement, and closed-loop E2E remain gaps.
- **Semantic ontology depth remains thin** — employee is one of 15 seeded projected read types, but projected domains keep domain-owned writes and employee has no registered projected action dispatch; link/effective-date depth remains incomplete.
- **No ATS/recruiting REST** (recruit nav exists; recruiting in BE gap list). **No benefits admin depth** (BenefitCatalog rows only). **No mobile HR app** yet. Korea-only payroll by design (no multi-country).

---

## 2. The matrix (rows = capability dimensions; cells = HOW, labeled)

### Row 1 — Information architecture (how the employee record is modeled)

- **Ours:** Employee is seeded as a projected read type and can participate in the shared object surface, while its domain crate remains the writer. The 3-layer target (kinetic history plus dynamic policies/automations) and typed links to payroll/position/voucher are incomplete; no employee projected action dispatch is registered. [code-evidenced]
- **Rippling:** Single **"employee graph"** — one source of truth where any employee attribute propagates to payroll/IT/benefits automatically; changes cascade across connected modules. [V] (rippling.com/platform/workflows)
- **SAP SF:** **Employee Central** master-data foundation — effective-dated records, position management, 100+ country data models. Heavy, config-driven MDF (Metadata Framework). [V] (help.sap.com Employee Central)
- **Asana:** No employee record — the "unit" is a **task/project**; an onboarding is a project instantiated from a template. HR data lives elsewhere. [V] (asana.com/templates/employee-onboarding)
- **Foundry:** N/A as HR — but its **Ontology** (objects+links+actions over source data) is exactly our grammar; if Foundry "did HR," employee would be an ontology object with typed link-types. Architectural mirror only. [I]

### Row 2 — Employee lifecycle (hire → onboard → transfer → offboard)

- **Ours:** import→lifecycle-events per employee; **exit-cases** FSM (report/confirm/approval-draft) tuned to the 부재→퇴직 (absence→exit) population, wired to severance. Onboarding flow itself is thin (no offer-letter/e-sign). [I, code]
- **Rippling:** **Full lifecycle** incl. offer letter, E-Verify, benefits election, device/app provisioning at hire, auto-cascade on promote/transfer/relocate, offboarding revokes access. Reference-grade. [V] (businessnewsdaily.com/16121; rippling.com)
- **SAP SF:** Full lifecycle via EC events + Onboarding module (offer, doc-signing, cross-boarding); enterprise-heavy, implementation-project scale. [V] (help.sap.com)
- **Asana:** **Onboarding-as-project** — 30-60-90 templates, Forms+Rules branch tasks by role/location, relative due dates, approval tasks for policy sign-off. No system-of-record actions (no access grant, no pay setup). [V] (asana.com; ido-clarity.com)
- **Slack/Teams:** N/A core — used as the *notification/nudge* surface for onboarding checklists. [I]

### Row 3 — Org structure / positions / org chart

- **Ours:** `/hr/org-chart` endpoint + org nav screen; **position is a string column** (per ontology audit), not a first-class object — chain C-→Position→Posting→Employee is *broken 3/4*. [I, ledger-evidenced]
- **Rippling:** Org chart auto-derived from the employee graph; reporting lines drive permission/approval routing dynamically. [V] (rippling.com/platform/permissions)
- **SAP SF:** **Organizational Management** = first-class positions, jobs, org units, cost centers; a cited enterprise reference for position management. [V] (hicron.com; scdsoft.de)
- **Asana:** N/A — no org model (only project membership/teams). [I]

### Row 4 — Payroll runs (calc engine, statutory correctness, cycle)

- **Ours:** **Korea-statutory-native kernel** — 4대보험 ppm rates with effective periods, NTS withholding, min-wage, severance = max(average, ordinary) enforced at *compile time*, release gate with golden-case + professional validation. Deep correctness; **no run-orchestration UI yet** (payroll screen stub). [V-internal, code]
- **Rippling:** **Native full-service payroll** — automates ~95% of admin, tax filing, direct deposit, garnishments, year-end forms; "pay run in 90 seconds." Native **global payroll only 10 countries**; Korea via **EOR/partner**, not deep native localization. [V] (rippling.com/blog/rippling-payroll-review; rippling.com/global; rippling.com/country-hiring/south-korea-employees)
- **SAP SF:** **Employee Central Payroll (ECP)** — multi-country/-currency/-lingual, has an **official SAP Korea localization** (연금/건강/고용/산재, 근로소득세). The cited surface covers multi-country payroll and Korea localization, with implementation cost/complexity noted by the secondary source. [V] (sap.com/korea ECP page; suretysystems.com)
- **Asana / others:** N/A — no payroll. [I]

### Row 5 — Attendance / 근태 / time

- **Ours:** attendance import pipeline + **self-service check-in** (`/attendance-records/me`) + org roll-up summary; screen is a stub. No shift/roster engine yet (교대/대근 in backlog). [I, code]
- **Rippling:** Time & Attendance native module (clock-in, geofencing, overtime rules, break policies, PTO accrual) feeding payroll directly. [V] (rippling.com; localization "working time rules, break policies")
- **SAP SF:** **Time Sheet** (attendances) + **Time Off** (absences); integrates to ECP; supports complex 근무제 rules. [V] (gavdi.com; zalaris.com)
- **Microsoft Teams:** **Shifts** app covers frontline rostering/clock-in inside Teams — the one place Teams genuinely touches 근태. [I] (known Teams Shifts feature)
- **Asana:** N/A (time-off *request tracking* via Forms only, not attendance). [V]

### Row 6 — Leave / 연차 management (Korea-specific)

- **Ours:** request-decision FSM with SoD, source-observed request/balance reads and source-wired decision/promotion calls, 인원별 원장, and a round-labelled receipt/notice substrate. Request creation is unwired; statutory timing/sequence enforcement and closed-loop E2E are absent. [V-internal, code]
- **Rippling:** PTO policies with accrual/carryover, approval routing, calendar; generic global PTO — **not modeled to 근로기준법 §60/§61 사용촉진 notice procedure**. [V] (rippling.com localization: "vacation and sick leave") [I on §61 gap]
- **SAP SF:** Time Off with accrual rules configurable to Korean 연차; Korea localization exists but 사용촉진 is a **config exercise, not a shipped legal FSM**. [I] (config-framework known)
- **Asana:** Time-off *request* workflow (Forms+Rules+Approval task) — a request tracker, no balance/accrual/statute. [V] (asana.com/teams/hr)

### Row 7 — Permissions / scoping (RBAC → PBAC)

- **Ours:** current people access uses the non-authoritative feature projection plus legacy server authorization and evidenced RLS. `EmployeeDirectoryManage` and Group→법인→branch→worksite inputs inform the accepted Cedar PBAC target, where roles become principal attributes only after enrollment and promotion. [code: web/src/console/policy/authz.ts; ADR-0021]
- **Rippling:** **Attribute-based permissions** — permission profiles attached to employee attributes (level/dept) that **auto-update as people move**. Closest external analog to our PBAC; the pattern to steal. [V] (rippling.com/platform/permissions)
- **SAP SF:** **RBP** (Role-Based Permissions) — granular but role-centric, permission groups by attribute; powerful yet notoriously complex to administer. [V] (help.sap.com) [I on complexity]
- **Asana:** Project/team membership + admin roles; no fine-grained data-field permissions. [I]

### Row 8 — Automation hooks / workflow triggers

- **Ours:** audit and Cedar-evaluation decision-log seams plus series/automations are surfaced on the ObjectCard dynamic layer; current proof does not establish universal audit or live Cedar enforcement. Automation authoring exists, but the HR-specific recipe library is thin. [I, code]
- **Rippling:** **Workflow Automator** — *any* field/attribute in Rippling or connected apps can be a trigger paired to any action; no-code, "go beyond basic automation." Source-cited HR automation. [V] (rippling.com/platform/workflows; blog/go-beyond-basic-automation)
- **SAP SF:** Business rules engine + Intelligent Services (event-driven cross-module); enterprise-grade, config-heavy. [V] (help.sap.com) [I on ergonomics]
- **Asana:** **Rules** (trigger→action) + Forms; strong for onboarding branching, scoped to Asana objects. [V] (gend.co)
- **n8n:** N/A core — but the **integration fabric** if you want to wire HR events across arbitrary SaaS with code-level nodes; self-hostable. [I]
- **Slack/Teams:** Workflow Builder / Power Automate as the *front-door trigger* (form in chat → HR action). [I]

### Row 9 — Approvals / 전자결재

- **Ours:** SoD-enforced leave approval, exit **approval-draft → AP- governance object**, governance approvals REST (append-only, requester-authoritative to close self-approval hole), 결재함 UI. Maps to Korean 전자결재 line-approval culture. [V-internal, code + ledger]
- **Rippling:** Approvals app + Workflow-Studio approval logic; permission-scoped routing. Linear/parallel approvals, but **no native 결재선/전결 규정** concept. [V] (rippling.com permissions/workflows) [I on 전자결재 gap]
- **SAP SF:** Workflow approvals with delegate/escalation; can model multi-step 결재선 via config. [I]
- **Asana:** **Approval task** type (Approve/Request-changes/Reject) — lightweight, good for onboarding sign-offs, not a governance ledger. [V] (asana.com)
- **Slack/Teams:** Approvals app native to the chat surface — good UX for the *last-mile tap-to-approve*, no audit-grade ledger. [I]

### Row 10 — Audit / compliance

- **Ours:** hash-chain seal/verify with gap detection, runtime-role RLS tests, and evidence/custody surfaces are repository-backed. This proves the implemented audit substrate, not that every people FSM transition is sealed. [code: backend/crates/platform/audit-chain; migrations 0100/0101]
- **Rippling:** Audit logs, SOC2, compliance automation (tax filing, ACA, EEO); compliance-*as-service*, not a cryptographic chain. [V] (rippling.com) [I on chain]
- **SAP SF:** Deep audit trails, GRC integration, data-retention/GDPR tooling; enterprise compliance benchmark. [V] (help.sap.com) [I]
- **Asana:** Admin audit log (Enterprise tier); no HR-compliance semantics. [I]

### Row 11 — Mobile / employee self-service

- **Ours:** web-console leave and attendance paths exist; the repository's Android app is the field app (`com.console.app`), not a general employee self-service app. A native employee app remains backlog. [code: android/app/build.gradle.kts; web/src/console]
- **Rippling:** Full mobile app — pay stubs, PTO, docs, onboarding tasks. [V] (rippling.com) [I on parity]
- **SAP SF:** Mobile app for EC/Time; functional but not loved for UX. [I]
- **Asana:** Strong mobile app for task/onboarding, not HR data. [I]
- **Slack/Teams:** The de-facto mobile self-service *shell* many firms bolt HR bots onto. [I]

### Row 12 — Extensibility (custom fields / objects / no-code types)

- **Ours:** **ontology grammar** — goal is add-a-type no-code (register type → instances/drag/module/policy/automation free). *Today: 6 manual steps, not yet no-code* (per ontology audit). North-star, partially built. [V-internal, ledger]
- **Rippling:** Custom fields/objects on the employee graph; strong but within Rippling's model. [V] (implied by attribute-based everything) [I]
- **SAP SF:** **MDF** (Metadata Framework) — arbitrary custom objects/fields; extremely flexible, extremely complex. [V] (help.sap.com) [I on complexity]
- **Foundry:** N/A HR — but its Ontology + Actions is the reference implementation of "define an object type, get CRUD/lineage/permissions free." Our north-star mirror. [I]
- **Asana:** Custom fields on tasks/projects; not a data-modeling platform. [I]

### Row 13 — Korean B2B fit (전자결재 · 근로기준법 · group-company scoping)

- **Ours:** **native substrate** — 4대보험/근로소득세 kernel, round-labelled §61 notice/receipt records, severance §2② rule, 전자결재 AP- line-approval, Group→법인→branch scoping, Korean-first UI. The leave substrate does not yet enforce statutory timing or sequence. [V-internal, code]
- **Rippling:** Korea via **EOR/partner** — handles it, but as a localized *service overlay*, not a 근로기준법-native product; no 전자결재 결재선, no 사용촉진 FSM. [V] (rippling.com/country-hiring/south-korea) [I on depth]
- **SAP SF:** Official SAP **Korea localization** exists (ECP) — the only sampled vendor whose cited source establishes a Korea payroll localization; this does not establish a market-wide ranking. It remains enterprise-cost/complexity and generic on 전자결재. [V] (sap.com/korea) [I]
- **Asana / Slack / Teams / n8n / Foundry:** N/A — no Korean HR statute awareness; global tools with zero 근로기준법 modeling. [I]

---

## 3. Per-vendor "how they'd build OUR people module"

**Rippling.** Would collapse our HR/payroll/attendance/leave into a single **employee graph** and make *everything* a cascade off attribute changes — hire an employee and payroll, access, device, benefits self-configure. Permissions would be attribute-bound (level/dept) and auto-update on transfer. Every HR process would be a **Workflow Automator** recipe with any-field triggers. The cited surface covers onboarding, US-style payroll, and automation ergonomics, but they'd ship Korea as an EOR overlay, missing our 근로기준법-native kernel and 전자결재 결재선. Verdict: steal the graph + attribute-permissions + automator; keep our statutory depth.

**SAP SuccessFactors.** Would model Employee Central as the effective-dated master-data core with first-class **positions/org units/cost centers**, ECP for multi-country payroll (Korea localization included), Time Off + Time Sheet for 근태, MDF for extensibility, RBP for permissions. The cited surface covers more org-management and global-payroll capabilities than the current repository, with a heavier implementation model. No reciprocal UX, audit, or time-to-value superiority is claimed. Verdict: steal position/org-management rigor and effective-dating discipline; reject the weight.

**Asana.** Would treat the module as a **workflow layer, not a system of record** — onboarding/offboarding/HR-requests as templated projects with Forms→Rules branching by role/location, relative due dates, and Approval tasks. Useful as a cited process-choreography reference around HR events, but not a data/payroll system of record. Their version of "our module" is the **onboarding project template + request intake** sitting on top of a real HRIS (ours). Verdict: steal the onboarding-template + Forms/Rules intake pattern for our thin onboarding flow.

**Palantir Foundry.** Wouldn't build an "HR module" — it'd register `Employee`, `Position`, `PayrollRun`, `LeaveRequest` as **ontology object types with typed link-types and Actions**, and every screen would be a generated view over that ontology with lineage and branch-scoped permissions for free. This is *literally our stated north-star*; Foundry is the cited reference for this ontology/action grammar. Verdict: it validates our ontology-first bet — the "steal" is finishing the no-code type registration we've only half-built.

**Slack.** Would build nothing in HR core; it'd be the **front-door** — a Workflow-Builder form for 휴가 신청, an Approvals message for the manager, a bot that posts payslip-ready nudges. The employee never leaves chat. Verdict: steal the tap-to-approve-in-chat last-mile for our leave/exit approvals (as an integration, not core).

**Microsoft Teams.** Same as Slack, plus **Shifts** for frontline rostering/clock-in (the one genuine 근태 adjacency) and Power Automate + Approvals for HR request routing. For a Microsoft-shop client, Teams becomes the self-service shell over our backend. Verdict: steal the Shifts-style roster/clock UX pattern for our 교대/근태 screen; integrate Approvals.

**n8n.** Would be the **integration fabric** — no HR record of its own, but code-level nodes wiring our HR events to arbitrary SaaS (Slack, NTS 홈택스, 4대보험 EDI, accounting) on a self-hostable engine. Verdict: relevant only as the automation glue; our own Automate/series layer is the in-platform equivalent.

---

## 4. What we'd steal (ranked, actionable, with ontology-fit + cost)

1. **Attribute-bound permissions that auto-update on transfer** → **Rippling** is the cited reference for this capability. Fit: native to the accepted PBAC/Cedar target, where roles become principal attributes after enrollment; wire employee attrs (level/dept/법인), shadow-prove the decisions, and promote explicitly. **Cost: M.** Highest leverage — turns our static branch-scope into dynamic.
2. **The employee graph / attribute cascade** → **Rippling**. Fit: employee is already a seeded projected read type; deepen its typed links to payroll_run/position/leave/attendance and add domain-owned action dispatch target by target. **Cost: M**.
3. **First-class positions & org management (effective-dated)** → **SAP SF**. Fit: Position exists in the seeded C-chain, but employee linkage/effective-dating remains incomplete and legacy `employees.position` is still a string. Deepen the C-→Position→Posting→Employee chain without creating a second writer. **Cost: M/L**.
4. **No-code custom object/field extensibility** → **Foundry** (grammar) / **SAP MDF** (feature scope). Fit: *is* our north-star; the steal is finishing add-a-type no-code (kill the 6 manual steps — generic create-action auto-attach, registry-derived code prefixes, data-driven MOD_SCREENS). **Cost: L** but already chartered.
5. **Onboarding template + Forms/Rules intake** → **Asana**. Fit: our onboarding flow is thin; add a templated onboarding *project* (relative due dates, role/location branching, approval sign-offs) as a lifecycle FSM instance. **Cost: S/M.**
6. **Workflow Automator with any-field triggers** → **Rippling**. Fit: extend our Automate layer so any employee/leave/attendance field change is a no-code trigger; we have the audit/series substrate. **Cost: M.**
7. **90-second pay-run orchestration UX** → **Rippling**. Fit: our payroll *kernel* is deep but the run UI is a stub — steal the "one-screen, mostly-automated, review-and-release" run flow on top of our correctness engine + release gate. **Cost: M.**
8. **Shifts-style roster/clock 근태 screen** → **Teams Shifts / Rippling T&A**. Fit: our attendance import + check-in REST exist; needs the roster/교대/대근 UI (backlog item). **Cost: M/L.**
9. **Tap-to-approve-in-chat last mile** → **Slack/Teams Approvals**. Fit: integration off our messenger + governance approvals REST; keep the audit ledger authoritative. **Cost: S** (integration).

**Where global vendors mismatch and we keep locally specific semantics (do NOT steal):** round-labelled §61 notices/receipts, severance §2② max(average, ordinary), 4대보험/NTS statutory kernel, 전자결재 결재선/전결, and Group→법인→branch scoping. The §61 substrate still needs deadline and round-sequence enforcement before it is a statutory FSM; trusted audit anchoring also remains open. Rippling/SF treat Korea as a localization overlay; deepen these local primitives without overstating them.

---

### Sources

- Rippling: [platform/workflows](https://www.rippling.com/platform/workflows), [platform/permissions](https://www.rippling.com/platform/permissions), [payroll review](https://www.rippling.com/blog/rippling-payroll-review), [global](https://www.rippling.com/global), [South Korea hiring](https://www.rippling.com/country-hiring/south-korea-employees), [Workflow Automator](https://www.rippling.com/blog/go-beyond-basic-automation-with-workflow-automator), [BusinessNewsDaily review](https://www.businessnewsdaily.com/16121-rippling.html)
- SAP SuccessFactors: [Employee Central help](https://help.sap.com/docs/SAP_SUCCESSFACTORS_EMPLOYEE_CENTRAL), [ECP Korea](https://www.sap.com/korea/products/hcm/employee-central-payroll.html), [Surety Systems ECP](https://www.suretysystems.com/insights/what-is-successfactors-payroll-key-capabilities-benefits/), [Hicron EC](https://hicron.com/blog/successfactors-employee-central/), [scdsoft EC/Time](https://www.scdsoft.de/en/blog/evaluation-of-sap-successfactors-employee-central-ec-time-tracking-employee-central-payroll/), [Gavdi Time](https://gavdi.com/sap-successfactors-time-management-time-tracking/)
- Asana: [onboarding template](https://asana.com/templates/employee-onboarding), [HR templates](https://asana.com/templates/team/hr), [HR teams](https://asana.com/teams/hr), [customise onboarding](https://www.gend.co/blog/customise-onboarding-in-asana), [ido-clarity](https://ido-clarity.com/blog/asana-for-hr-onboarding-operations/)
- Korea statute context: [Ian Labor Law payroll guide](https://www.ianhr.com/en/guidetopayrollinkorea/), [Forvis Mazars payroll KR](https://www.forvismazars.com/kr/en/insights/korean-insights/payroll-in-korea)
- Internal (grepped): `backend/app/src/hr.rs`, `backend/crates/payroll/domain/src/lib.rs`, `web/src/console/{leave,lifecycle,shell/nav.ts}`, `docs/program/console-program-ledger.md`

---

## 5. Cross-cutting lens findings (5 independent review lenses)

- **Task-flow:** money task = *open an employee, take an HR action*. Employee is a seeded projected read type, but no employee action dispatch is registered, so uniform Related Actions remain incomplete. Workday's menu is on every worker uniformly. **Steal:** keep the domain crate as sole writer and add explicit employee action dispatch plus a consistent ObjectCard action row. Cost **M/L**.
- **IA / layout:** only `identity/` has real components; `recruit/orgchart/evaluation` **unbuilt**; no worker object page. Korean fit: 직급 vs 직책, 호봉, 발령 history, 법인→본부→팀 org — Workday's flat "position" mismatches 직급 tables; our ontology could model 직급/호봉 as object props natively (potential local fit if built). **Steal:** Related-Actions menu (exists in the asset module) [M]; anchored worker profile (Job/발령/평가/근태) [M-L]; org-chart drill reusing the topbar scope tree [M].
- **Data-model:** **Workday is the selected effective-dating reference in this sample** — its correct-vs-new-effective-change distinction is precisely our draft-direct-vs-override, but far more mature, and it makes **Position a first-class object** where we store `employees.position` as a **string**. Our current strengths are a uniform typed registry, hash-fixity substrate, and explicit-reason/four-eyes override path; Cedar field masking remains target/shadow. **Steal:** Position/Job-Profile as first-class linked objects (the C-→Position→Posting→Employee north-star chain) [L]; the correct-vs-new-effective-dated-change UX [M]; dual entry-date/effective-date bi-temporal stamping [M].
- **Governance:** **Partial/local-fit, Behind on the reusable BP abstraction** — current enforcement is legacy server-side plus evidenced RLS, and approval routing is per-workflow-definition rather than a reusable `definition→steps→condition/routing→commit` model applied to every HR event. The repository has a 노무수령거부/수령확인 notice-and-receipt substrate (inbox `0119`, leave R-), but it does not enforce statutory timing or prove round sequence. **Steal:** Workday BP-framework generalization (one `BusinessProcessDefinition` object-type every HR action instantiates) [L]; initiate/approve/**view** split into three target Cedar actions per BP [S].
- **Automation / extensibility:** we have the effective-dating substrate Workday relies on; missing HR-event **triggers** and outbound HR webhooks. **Steal:** HR-event lifecycle triggers (on-approve-leave → auto balance decrement / 연차촉진 round) → Workday BP [S–M]; 연차촉진 round scheduler (근로기준법-specific; an equivalent was not observed in the sampled product sources) [M]; routing modifiers by org scope (Group→법인→branch→worksite) [M].

**Adjudicated citation:** the severance floor's statute is **근로기준법 제2조제2항** (the Act), not 시행령 §2② — the code comment (`payroll/domain/src/lib.rs:149,553`) mislabels it and should be corrected (see §1 above).
