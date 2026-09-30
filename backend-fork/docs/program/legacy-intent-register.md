> **HISTORICAL after the 2026-07-28 pivot.** This document preserves evidence and prior reasoning only. It cannot authorize Buck2, broad product scope, fan-out, or implementation work. Current product authority is [`docs/current/PRODUCT.md`](../current/PRODUCT.md), [`ROADMAP.md`](../current/ROADMAP.md), and [`DELIVERY.md`](../current/DELIVERY.md). Pivot history only: [`docs/PIVOT-2026-07-28.md`](../PIVOT-2026-07-28.md) (non-authority).

# Legacy Intent Register — distilled intentions for Phase D reimagination

> **Purpose.** Phase D of the console program does **not** port the legacy frontend. It distills the
> *intentions* behind every legacy surface — the real user problem each one served — and reimagines them at
> the new console's quality bar. This register is that distillation: the input for the Phase D spec wave.
>
> **What the legacy work got wrong (the reason we distill, not port).** The legacy `pages/*` + `features/*`
> shell was a placeholder generation. Its recurring failures against the bar: explanatory subtitles under
> every page title (the `PageHeader.description` disease), fixture-farm panels that render without a
> backend, non-functional "dark" affordances (condition editors that never evaluate), raw JSON/UUID
> inspectors, form-farm settings pages, and export farms. None of that survives. The **intent** does.
>
> **The bar (binding).** Functional components only; zero filler, zero stubs; **no explanatory
> subtitles/captions** — the UI is self-explanatory, status is a chip, copy only drives an action (DESIGN
> §4-12); compact 1-row stat bars, never big-number KPI cards (§4-11); full reuse of shared primitives —
> "draw the same shape twice = violation" (§4-18); benchmark each surface against Palantir / Slack / Teams /
> Workday / Gmail / n8n (§4-21). Every reimagined charter runs the 10-stage pipeline
> (research → spec → API contract → TDD red/green → incremental impl → code+security review → simplify →
> root-cause debug → CI gates → ship w/ visual-verdict) and is **verified by a committed Playwright
> user-story spec** (W3 persona-lock pattern), not by API-only evidence.
>
> **Method.** Every AppShell/platform/auth route in `web/src/AppRouter.tsx` walked route → component → API;
> intent read from the *actual fields collected and endpoints called*, not the markdowns. Coverage checked
> against the console module matrix (design ROADMAP §4), the ontology lifecycle matrix
> (`ontology-coverage-matrix.md`, file:line-grounded), and the deletion-readiness audit
> (`docs/design/oyatie-console/LEGACY-PARITY-BACKLOG.md`, cross-referenced as `[parity #N]`). Legacy
> markdowns distilled: `SPEC.md`, root `HANDOFF.md`, `docs/specs/**`, `android/E2E-MANUAL-SMOKE.md`.

> **Evidence and retirement ceiling (binding).** This register is a revision-bound mapping of intentions to
> fixed-target source; it does not validate browser behavior, backend evaluation, deployment, activation, or
> production operation. Those layers remain unverified here. `COVERED` and `retire` are planning labels
> only, not evidence that a capability is shipped or that a route may be disabled or deleted. Under accepted
> ADR-0025, shipped/readiness requires every applicable complete-slice gate: a reachable body, typed
> real-backend reads/mutations, source-object drill, server authorization/fail-closed client behavior,
> audit/atomicity, failure states, persona real-backend E2E, quality gates, and explicit legacy parity or an
> owner-approved deferral. Retirement additionally requires the complete workflow/persona/cadence inventory,
> parity/descope and policy/audit evidence, rollback rehearsal and budgets, recurrence-aware proof (including
> the two-revision/fourteen-day floor and representative recurrence), at least 99.9% reconciled classification
> with lost/unknown events counted as legacy, and the two-stage decommission: disable while retaining a signed,
> restorable packet, then remove only after restoration-drill and observation gates, retaining the last verified
> packet for at least 90 days.

## Legend

- **Coverage** — a planning/source-mapping classification at the fixed target. `COVERED` means the
  target-source mapping carries the distilled intent; it is not shipped, readiness, or complete-slice
  evidence and never waives the binding ceiling above. `PARTIAL`: the target source maps a domain surface but misses named depth.
  `UNCOVERED`: no target-source surface maps it. `DESCOPED`: belongs to a separate program (public
  storefront / sales #6). `DO-NOT-PORT`: the legacy shape itself violates the bar and is deleted, not
  re-expressed.
- **Verdict** — a Phase D planning disposition. `retire` is only a candidate label and does not authorize
  cutover, disablement, or deletion; it may take effect only after accepted ADR-0025's complete-slice and
  decommission gates pass. Other labels are `depth charter`, `new charter`, `fold`, `delete`, and
  `separate program`.

## Personas (design ROADMAP §8 — the workflow matrix that gates every reimagined surface)

| Persona | Daily spine |
|---|---|
| **HR 담당 (김성아)** | recruit pipeline → hire → work-contract (inbox passkey) → onboarding check → 인사카드; exceptions: absence justification, 연차 촉진 |
| **배차 담당 (dispatcher)** | WO- queue (SLA chips) → match available driver → assign approval → track → settlement |
| **지게차 기사 / 현장직 (김성호, mobile)** | check-in → WO- receive → work log (JL-) → overtime AP- → own payslip / inbox |
| **공장 반장 (foreman)** | shift roster → absence detect → substitute assign (인력풀) → approve → log |
| **급여 담당 (payroll)** | attendance-close gate → run create → exception review (deductions, substitute pay) → transfer approve → payslip distribute (inbox) |
| **사무직 (all-staff)** | personal inbox · 기안 · mail · messenger · own attendance/payroll/leave self-service |
| **지원자 (external applicant)** | "my applications" status → offer inbox passkey receipt → (employee conversion on hire); everything else deny-by-omission |
| **임원 / 경영진 (executive)** | dashboard → contract-profitability drill → labor cost → 전결 approval → audit stream |
| **CX / 영업 (sales)** | external mail (quote CS-) → contract draft (guardrails) → posting/roster chain |
| **컴플라이언스 / 감사 (compliance)** | audit feed → anomaly chip → object drill → policy sim → override gate |

---

## Inventory

Counts: **59 legacy surfaces** total (51 tenant-console + 8 public storefront). Of the 51 tenant-console
surfaces: **8 COVERED**, **29 PARTIAL**, **11 UNCOVERED**, **2 DO-NOT-PORT/fold**
(`/reporting`, `/equipment/legacy`), **1 DESCOPED** to the sales program (`/catalog`). The 8 storefront
pages are a separate public program.

### Field operations

| Feature (route) | Distilled intent | Persona | Coverage → module | Verdict |
|---|---|---|---|---|
| Dispatch (`/dispatch`) | WO- queue × available-mechanic matching × SLA; the assignment surface | dispatcher | PARTIAL → `dispatch` | depth charter |
| Work-order detail (`/work-orders/:id`) | the WO- object: status FSM, assignment, evidence-gated close; write gated to assigned mechanic | dispatcher / field-tech | PARTIAL → `maintenance` | depth charter (3-layer ObjectCard) |
| Dispatch map (`/dispatch-map`) | live site markers + mechanic units + queue panels for geographic dispatch | dispatcher | PARTIAL → `map` | depth charter |
| Intake (`/intake`) | fast field-ticket creation: resolve customer/site from an equipment code | dispatcher / CX | PARTIAL → `field`/`maintenance` | depth charter (+ DO-NOT-PORT the tag-packing UI) |
| Daily plan (`/daily-plan`) | branch daily work plan: DRAFT→REQUESTED→APPROVED→confirmed review | foreman / dispatcher | **UNCOVERED** | new charter |
| Inspection (`/inspection`) | preventive-maintenance schedules: cycle presets, overdue detection, checklist rounds | field-tech / foreman | **UNCOVERED** | new charter (→ series SR-) |
| Wallboard (`/wallboard`) | chrome-less kiosk showing whole-queue SLA/exception counts | all (branch floor) | PARTIAL → `dashboard` | depth charter (kiosk preset; keep whole-queue count) |

### Analytics / Foundry / governance

| Feature (route) | Distilled intent | Persona | Coverage → module | Verdict |
|---|---|---|---|---|
| KPI (`/kpi`) | the 7 operational KPIs with source drill | executive | PARTIAL → `dashboard` | fold into dashboard |
| Operations Intelligence (`/intelligence`) | governed BI launchpad — insights as objects, not charts | executive | PARTIAL → `dashboard` insights (AN-) | fold [parity #23] |
| Ontology (`/ontology`) | object explorer / object-set query workspace | executive / compliance | PARTIAL → `explore` (ontology engine) | retire (+ DO-NOT-PORT JSON-dump inspector) |
| Automate (`/automate`) | no-code automation blocks + run log | compliance / admin | COVERED → `auto` | retire; small depth (clone, connector catalog) [parity #13] |
| Forecast (`/forecast`) | scenario/what-if forecasting | executive | **UNCOVERED** (wire-pending stub) | new charter (P4) |
| Config console (`/config-console`) | tenant operational configuration surface | admin | PARTIAL → `config-console` | depth charter |
| Reporting (`/reporting`) | data export | executive / compliance | **DO-NOT-PORT** (export farm) | fold → per-module egress-gated actions [parity #19] |
| Integrity (`/integrity`) | governance findings: detector triage REVIEWED/DISMISSED/ESCALATED + mandatory memo | compliance / executive | PARTIAL → `audit`/`compliance` | depth charter [parity #11] |
| Ops dashboard (`/ops`) | operations command dashboard | executive / admin | PARTIAL → `dashboard` | fold [parity #20] |

### Communication

| Feature (route) | Distilled intent | Persona | Coverage → module | Verdict |
|---|---|---|---|---|
| Messenger (`/messenger`) | internal chat with object links, presence, threads | all-staff | COVERED → `msgr` | retire |
| Mail (`/mail`) | corporate mail: Gmail-grade threading | all-staff / CX | COVERED → `mail` | retire |
| Collaboration (`/collaboration`) | unified cross-object 7-day calendar + scoped polls/voting | all-staff | PARTIAL → `comms`/`board` | depth charter [parity #16] |

### HR / Finance / Payroll

| Feature (route) | Distilled intent | Persona | Coverage → module | Verdict |
|---|---|---|---|---|
| Financial (`/financial`) | rental-quote objects + finance postings linked to the C- chain | executive / sales | PARTIAL → `finance` | depth charter (+ ERP spine, see UNCOVERED) [parity #18] |
| Payroll (`/payroll`) | payroll run: attendance-close gate → run → exception review → 명세서 | payroll | PARTIAL → `pay` | depth charter (+ offboarding settlement) [parity #4] |
| Leave management (`/hr/leave-management`) | 연차 request/approve + 촉진 + 거부권 | HR / all-staff | PARTIAL → `leave` | depth charter — retain intent until request creation + committed closed-loop Playwright E2E land |
| Insurance assist (`/hr/insurance`) | 4대보험 acquisition/loss classification + EDI readiness | HR / payroll | **UNCOVERED** | new charter (Korean legal) [parity #4] |
| Employees (`/settings/employees`) | employee master + attendance import + identity-resolution + Korean 4-item sign-off | HR | PARTIAL → `hr`/`recruit` | depth charter [parity #15] |

### Admin / Settings / Governance

| Feature (route) | Distilled intent | Persona | Coverage → module | Verdict |
|---|---|---|---|---|
| Users (`/settings/users`) | identity admin: user CRUD, multi-role + multi-branch scope, user↔employee link, sign-in OTP issuance, credential reset | admin / HR | **UNCOVERED** | new charter (Identity Console) [parity #1] |
| Org (`/settings/org`) | 법인/조직도 hierarchy CRUD with lifecycle | admin / executive | PARTIAL → `org` | depth charter |
| Sites (`/settings/sites`) | worksite/branch CRUD | admin | PARTIAL → `org` | depth charter |
| Admin settings / security (`/settings/security`) | tenant security posture + credential recovery | admin | **UNCOVERED** | new charter (rides Identity Console) [parity #1] |
| Group admin (`/settings/group`) | enter-subsidiary MANAGE context, per-group health, DoA-gated context switch | executive / group-admin | PARTIAL → view-as | depth charter [parity #21] |
| Policy Studio (`/settings/policy`) | no-code Cedar P→R→A→Effect + assignment planner (impact-preview receipt, custom-role CRUD, 16-attr editor) | compliance / admin | COVERED → `policy` | retire; planner depth (+ DO-NOT-PORT dark condition editor) [parity #12] |
| Workflow Studio (`/settings/workflows`) | typed field·op·value editor and server-simulation paths observed in fixed-target source; browser behavior, backend evaluation, deployment, activation, and production operation unverified | compliance / admin | COVERED → `workflows` (source mapping only) | retire candidate blocked on ADR-0025 complete-slice and decommission gates; depth (clone, DAG validate, connector catalog) [parity #13] |
| Location settings (`/settings/location`) | per-branch GPS / PIPA consent objects | admin | PARTIAL → `map`/consent | depth charter (rides PII program) [parity #7] |
| Profile (`/settings/profile`) | self card: passkey list/register/revoke (last-credential guard) + self name/phone edit | all-staff | PARTIAL → self-card | depth charter (Identity Console) [parity #3] |
| Approvals (`/approvals`) | federated Approval Command Center queue | executive / all-staff | PARTIAL → `appr` | keep legacy `ApprovalCompose` special-case until new-shell mount; federated queue/boxes and new-shell integration remain open |
| Catalog admin (`/catalog`) | sales-listing catalog admin | sales | DESCOPED → sales #6 | separate program [parity #22] |

### Equipment / Asset

| Feature (route) | Distilled intent | Persona | Coverage → module | Verdict |
|---|---|---|---|---|
| Equipment browse (`/equipment`) | asset list/search | field-tech / dispatcher | PARTIAL → `asset` | depth charter |
| Equipment detail (`/equipment/:id`) | asset object: attributes, lifecycle, history | field-tech | PARTIAL → `asset` | depth charter (3-layer ObjectCard) |
| Equipment manage (`/equipment/manage`) | equipment master CRUD, bulk XLSX import + dry-run, management-no resolve, substitution assign/return, owner-org cross-context | admin / dispatcher | PARTIAL → `asset` | depth charter [parity #14] |
| Equipment (legacy) (`/equipment/legacy`) | superseded old equipment page | — | **DO-NOT-PORT** | delete |

### Support

| Feature (route) | Distilled intent | Persona | Coverage → module | Verdict |
|---|---|---|---|---|
| Support (`/support`) | support-ticket console: CS- threaded replies, internal notes, self-assign/claim, live SLA clocks | CX / all-staff | PARTIAL → `support` | depth charter [parity #10] |
| Customer intake (`/support/new`, public) | unauthenticated customer support ticket creation | applicant/customer | PARTIAL → `support` intake | depth charter |

### Auth / onboarding

| Feature (route) | Distilled intent | Persona | Coverage → module | Verdict |
|---|---|---|---|---|
| Login (`/login`) | OTP + passkey sign-in ceremony | all | COVERED → `identity` S0 | retire (cutover-gated) |
| Onboarding (`/onboarding`) | first-login: versioned PIPA consent → passkey enroll → phone-QR → desktop device-login approval | all | COVERED → `identity` FirstLoginOnboarding | retire (cutover-gated); **keep the invariants** [parity #2] |
| Pending (`/pending`) | no-role-grant landing (MEMBER with no surface) | applicant/new-user | PARTIAL → deny-by-omission | fold |

### Platform (vendor operator console)

| Feature (route) | Distilled intent | Persona | Coverage → module | Verdict |
|---|---|---|---|---|
| Platform tenants (`/platform/tenants`) | tenant provision/activate/suspend/archive + guarded force-erase | operator | **UNCOVERED** | new charter (operator console) [parity #5] |
| Platform groups (`/platform/groups`) | group CRUD + org assignment | operator | **UNCOVERED** | new charter |
| Platform ops (`/platform/ops`) | cross-tenant ops health | operator | **UNCOVERED** | new charter |
| Platform onboard (`/platform/onboard`) | new-tenant onboarding + bootstrap/group-account OTP issuance | operator | **UNCOVERED** | new charter |
| Platform account (`/platform/account`) | operator account settings | operator | **UNCOVERED** | new charter |

### Public storefront (separate program — judged as a public marketing site, not the console bar)

`/` `/rental` `/used` `/maintenance` `/about` `/contact` `/privacy` and `/platform-fsm` — the KNL storefront
(#6) + the public FSM marketing showcase. **DESCOPED** from the tenant-console register; migrate/retire
through the sales/storefront program. Marketing hero copy is acceptable *here* (it is a public site);
it is **DO-NOT-PORT** into any authenticated console surface.

### Already the new console

| Feature (route) | Distilled intent | Persona | Coverage → module | Verdict |
|---|---|---|---|---|
| Overview (`/overview`) | role-aware landing and action inbox | all-staff | COVERED → `overview` | new-console entry point |
| Attendance (`/attendance`) | attendance plan/actuals and self-service | all-staff / payroll | PARTIAL → `att` | `ConsoleShell` chrome only; absent from `SCREEN_REGISTRY` |

---

## Per-feature distillations (implicit intent from code + what to reimagine)

Only the surfaces where the code reveals intent the markdowns missed, or where reimagination is non-obvious.
`COVERED` surfaces are omitted (their intent is already carried; the table verdict is `retire`).

### Dispatch (`DispatchPage.tsx`) — PARTIAL → `dispatch`

- **Problem/when:** the dispatcher's core loop — a WO- lands, they match it to an available mechanic by
  location/skill and push the assignment. Fires continuously through the dispatcher's day.
- **Missing depth:** P1 auto-dispatch broadcast + mechanic offer accept/decline; policy-gated **force-assign**
  of an escalated P1 (override + reason + approval); multi-mechanic roles; review-gated target-due change;
  outsource-work record. [parity #6]
- **Reimagine:** extend the console `dispatch` screen; offers ride the notification center as actionable
  rows; force-assign = §3.10 override object (reason + approval + audit). Ontology: WO-, mechanic (employee),
  assignment. Shape: shared-track list + process panel (§4-18, reuse `dpRows`).

### Intake (`IntakePage.tsx`) — PARTIAL → `field`

- **Implicit intent (code):** the real need is **equipment-code → customer + site resolution** with
  auto-attached branch chips, so a receptionist creates a correct ticket from one code. [parity #17]
- **DO-NOT-PORT:** the legacy tag-packing form (multiple structured fields flattened into freeform
  tag/label strings) — violates typed-object grammar.
- **Reimagine:** equipment-code lookup resolving the CustomerSite object; typed object references as chips,
  never packed tags.

### Daily plan (`DailyPlanPage.tsx`) — UNCOVERED

- **Implicit intent (code):** a **branch daily-plan object** with a DRAFT→REQUESTED→APPROVED→confirmed
  review chain, a branch queue, and an absence/exit-warning panel — the foreman's morning planning ritual.
  The console's My-Work aggregates tasks but has **no plan lifecycle object**. [parity #9]
- **Reimagine:** instance-backed lifecycle object (`ont_instances`), single lifecycle modal (§3.9); branch
  queue = module surface; warning panel = attendance/exit dynamic reads.

### Inspection (`InspectionPage.tsx`) — UNCOVERED

- **Implicit intent (code):** PM **schedule CRUD** with cycle presets → interval, overdue detection, and
  checklist-round completion. This is literally design §4-15 series SR- (정기 검진) + §3.10 checklist objects,
  riding the BE-AUTO cron substrate — none of it built. [parity #8]
- **Reimagine:** series SR- schedule objects; overdue = derived analytic; checklist round = object with
  completion transition.

### Wallboard (`WallBoardPage.tsx`) — PARTIAL → `dashboard`

- **Implicit intent (code):** `WallBoardPage.tsx:19-24` deliberately fetches **every page** of work-orders
  (not the first 100) so the exception/SLA counts reflect the **whole queue, not a paginated slice** — the
  real intent is a *truthful org-wide SLA counter*, an invariant the markdowns never stated. Keep it.
- **Missing:** a chrome-less kiosk / auto-rotate preset. [parity #20]

### Financial (`FinancialPage.tsx`) + the ERP spine — PARTIAL/UNCOVERED

- **Implicit intent (code + `SPEC.md` §5, `erp.md`):** rental-quote objects with negative-잔존가 flooring
  linked to the C- chain, and the full ERP accounting flow — 견적→수주→세금계산서→미수금 and PO→입고→거래명세표→미지급금,
  VAT-period reconciliation, WO-consumed parts decrementing inventory + posting to the cost ledger.
- **Coverage reality at `origin/main@86a97771…`:** finance has voucher/header and line tables, a
  mounted draft→submit→approve→post→reverse REST/FSM, DB/domain balance gates, posted immutability,
  append-only lines, and FORCE RLS. Tenant seeding also publishes the C-chain ontology types.
  Remaining intent is period-close integration, full reporting/reconciliation, source-document
  auto-materialization, contract product workflows, and runtime/browser proof. [parity #18]
- **Reimagine:** Accounting/GL as balanced-posting objects; Sales/AR + Procurement/AP lifecycle chains;
  E-tax relay is external-integration-gated (ask-first).

### Payroll (`PayrollPage.tsx`) + offboarding — PARTIAL/UNCOVERED

- **Implicit intent (code):** payroll run gated on attendance close → run generation → exception review
  (deductions, substitute pay) → transfer approval → payslip (PS-) distribution to the inbox. The
  **offboarding half** (4대보험 loss, exit-case chain 현장 report→HR confirm→HQ confirm, 퇴직금) is a Korean legal
  obligation with no console carrier. [parity #4]
- **Reimagine:** exit-case = lifecycle object chain with settlement gates (§3.9.1); 퇴직금/severance = AP-
  template with structured fields.

### Insurance assist (`InsuranceAssistPage.tsx`) — UNCOVERED

- **Implicit intent (code + `korean-institutional-connectivity*.md`):** 4대보험 (national pension, health,
  employment, industrial-accident) acquisition/loss classification and NHIS/COMWEL EDI **loss-report**
  readiness — the compliance backbone HR runs on every hire/exit. Foundation-only today. [parity #4]
- **Reimagine:** compliance CP- rows whose evidence is the working exit objects; EDI is external-gated.

### Users / Admin-settings (`UsersPage.tsx`, `AdminSettingsPage.tsx`) — UNCOVERED (Identity Console)

- **Implicit intent (code):** the mandated *"operations through console only"* provisioning + recovery path —
  user CRUD/deactivate, multi-role assignment, multi-branch scope, user↔employee linking, **sign-in OTP
  issuance**, and **credential reset** (passkey wipe + re-OTP). No console surface carries it; blocks legacy
  route deletion. [parity #1]
- **Reimagine:** Account/Person as first-class objects; role/scope changes = policy-evaluated mutations
  with audit chips (roles = principal attributes, converging with Policy Studio); OTP issuance + credential
  reset = **audited action objects** with passkey step-up + reason, surfaced as single CTAs on the person
  card — never a settings-form farm.

### Support (`SupportPage.tsx`) — PARTIAL → `support`

- **Implicit intent (code):** a support-ticket console — CS-threaded replies, internal notes, self-assign/
  claim, and a live SLA command-center clock. Console has the SUP- ticket type + SLA lanes but not the
  threaded-desk depth. [parity #10]

### Equipment manage (`EquipmentManagePage.tsx`) — PARTIAL → `asset`

- **Implicit intent (code):** management-no lookup/resolve, **bulk XLSX master import with dry-run
  diagnostics**, owner-org cross-context, and equipment **substitution** assign/return (예비차량). The
  bulk-import-with-dry-run is the real operational need (445-unit master loads), not single-row CRUD.
  [parity #14]

### Integrity (`IntegrityPage.tsx`) — PARTIAL → `audit`

- **Implicit intent (code):** detector **finding objects** with a REVIEWED/DISMISSED/ESCALATED triage
  lifecycle and a **mandatory memo** on every decision. The console has the audit stream + anomaly chips but
  not the finding-object triage FSM. [parity #11]

### Platform operator console (`Platform*Page.tsx`) — UNCOVERED

- **Implicit intent (code):** tenant provision/activate/suspend/archive + guarded force-erase, group CRUD +
  org assignment, bootstrap/group-account OTP issuance, cross-tenant ops health, read-only impersonation
  (view-as tenant). The fixed-target source reviewed here does not establish a replacement vendor-operator surface; [parity #5] remains UNCOVERED.
- **Open decision:** separate operator program vs. operator-scoped module. Until decided + built, the
  `/platform/*` routes must outlive tenant-console replacement.

---

## Ranked UNCOVERED intents — the Phase D spec charters

Ranked by user value + how hard they block "operations through console only" and Korean legal obligations.

1. **Identity & credential administration** (`/settings/users`, `/settings/security`, self-half of
   `/settings/profile`) — user CRUD, multi-role/multi-branch scope, user↔employee link, sign-in OTP
   issuance, passkey credential reset. Blocks the console-only mandate. [parity #1/#3]
2. **Vendor multi-tenant operator console** (`/platform/*`) — tenant lifecycle (provision/suspend/erase),
   group CRUD, bootstrap OTP, cross-tenant health, audited view-as. Decide: separate program vs. module.
   [parity #5]
3. **4대보험 & offboarding settlement** (`/hr/insurance` + payroll exit) — social-insurance acquisition/loss
   + EDI readiness, exit-case chain, 퇴직금 calculation. Korean legal payroll obligation. [parity #4]
4. **Contract (C-) lifecycle depth** — target-base tenant seeding publishes **27 published tenant types (9 governed config + 3 C-chain + 15 projected domain)**. The C-chain types are published, but product lifecycle depth remains incomplete: legacy `employees.position` is still a string, and the contract→position→posting→employee consumer/link flow is unfinished.
5. **ERP accounting spine** (`/financial` depth) — GL postings/vouchers, tax-invoice draft, VAT
   reconciliation, AR/AP aging; gated on a 세무사 golden-case sign-off. [parity #18]
6. **Daily-plan lifecycle** (`/daily-plan`) — plan object DRAFT→REQUESTED→APPROVED→confirmed + branch queue
   + absence/exit-warning panel. [parity #9]
7. **Preventive inspection / PM schedules** (`/inspection`) — series SR- schedule CRUD, overdue detection,
   checklist rounds. [parity #8]
8. **Support-ticket console depth** (`/support`) — CS- threaded replies, internal notes, self-assign, live
   SLA command-center. [parity #10]
9. **Dispatch operational depth** (`/dispatch`) — P1 broadcast/offer accept-decline, force-assign override +
   reason, multi-mechanic roles, review-gated target-due, outsource record. [parity #6]
10. **Equipment master admin** (`/equipment/manage`) — bulk XLSX import + dry-run diagnostics, management-no
    resolve, owner-org cross-context, substitution assign/return. [parity #14]

Runners-up (charter after the top 10): employee/attendance bulk import + identity-resolution + Korean
4-item sign-off [parity #15]; location event-feed + per-branch PIPA GPS consent [parity #7]; group
MANAGE-context switch + per-group health [parity #21]; collaboration cross-object calendar + polls
[parity #16]; forecast/quant module (P4).

---

## DO-NOT-PORT list — legacy shapes that violate the bar (delete, don't re-express)

| Shape | Where | Why it violates the bar |
|---|---|---|
| **`PageHeader.description` subtitle** | `web/src/components/shell/PageHeader.tsx:26` (`<p class="… text-steel">{description}</p>`), used by ~44 pages | Explanatory subtitle under every page title — the systemic §4-12 violation. Console UI is self-explanatory; status is a chip, not a caption. |
| **ApprovalDocumentDesk fixture farm** | web/src/features/approvals/ApprovalDocumentDesk.tsx | Renders a document desk from fixtures and fails mock-independence (section 4-25-6). Retire the fixture-fed implementation, but preserve its document-desk intent as open appr work: only legacy ApprovalCompose is Source-present; federated queue/boxes and new-shell integration remain open. |
| **Dark condition editors** | `WorkflowStudioPage.tsx`, `PolicyStudioPage.tsx` condition builders | Condition UI rendered but non-evaluating ("dark") — a non-functional affordance. Replace it only with the fixed-target typed field·op·value editor and server-simulation path after closed-loop proof; current evidence is source-only, with browser behavior, backend evaluation, deployment, activation, and production operation unverified. |
| **JSON/UUID-dump object inspector** | `OntologyPage.tsx` / `features/object-view` | Dumps raw JSON / bare UUIDs — violates the `ObjectLink`/`safeLabel` + 3-layer ObjectCard grammar (never render a raw UUID). |
| **Standalone export farm** | `ReportingPage.tsx` (`/reporting`) | A page of export buttons. Exports belong as per-module **egress-gated actions** (§13 / [parity #19]), not a farm. |
| **Explainer hero blocks in authenticated surfaces** | storefront/`PlatformFsmPage` marketing heroes | Explanatory hero copy is fine on the public site; inside the console it is filler. No hero explainers in authenticated surfaces. |
| **Interactive wallboard chrome** | `WallBoardPage.tsx` shell/nav | Keep the whole-queue-count intent; drop shell/nav — the kiosk is chrome-less auto-rotate. |
| **Legacy equipment page** | `EquipmentPage.tsx` (`/equipment/legacy`) | Explicitly the superseded old page kept only during transition. Delete on cutover. |
| **Settings-form farms** | `UsersPage.tsx`, `AdminSettingsPage.tsx` | Multi-field settings forms for provisioning/recovery. Reimagine as audited action-object CTAs on the person card, not a form page. |

## KEEP-INTACT primitives — invariants the reimagination must preserve (re-express, never drop)

| Primitive | Where | The invariant |
|---|---|---|
| **Consent-before-enrollment** | `OnboardingPage.tsx:86-93` (`consentAccepted` gates the passkey ceremony) | The consent-before-passkey ordering is a product/policy control, not a PIPA conclusion. Whether the [Personal Information Protection Act](https://www.law.go.kr/%EB%B2%95%EB%A0%B9/%EA%B0%9C%EC%9D%B8%EC%A0%95%EB%B3%B4%EB%B3%B4%ED%98%B8%EB%B2%95) applies, and what notice, basis, or ordering is required for the actual data flow, remains scenario-specific for qualified Korean counsel. |
| **Desktop device-login QR approval** | `OnboardingPage.tsx` (`EnrollHandoffQr`, `approveDeviceLoginSession`) | Cross-device login handoff — a signed-in phone approves a desktop session. |
| **Last-passkey guard** | `features/auth/SecurityPanel.tsx:25,91,114,164` (backend 409 + `isLastPasskey` disables revoke) | Account-lockout prevention: a user can never delete their only credential. |
| **Whole-queue SLA count** | `WallBoardPage.tsx:19-24` (fetch-all, not paginated) | The counter must reflect the entire queue; a paginated count silently undercounts. |
| **Comment/memo-required decisions** | approval/integrity decision paths | Accountability: overrides, dismissals, and force-assigns require a reason/memo, written to audit. |
| **Object-Set faceted query (Lens)** | `SPEC.md` success criterion 2 (CAP-3/CAP-5), `features/object-view` | The Foundry object-set query capability — the console `explore` engine must carry faceted set queries + inter-object links, never a flat list. |
| **Audit-in-same-transaction** | `HANDOFF.md` guardrail 2 | Every state transition/approval/assignment/message writes `audit_events` in the *same* transaction; the sole carve-out is LocationPing coordinates (위치정보법, ADR-0014). |

---

## Legacy markdowns — intent that never reached code

- **`SPEC.md`** — the north-star intent (carbon-copy Palantir Foundry, no AI) is now the console's own
  charter. Its still-unmet *success criteria* are the deep UNCOVERED intents above: the FSM close-loop
  (intake→plan→dispatch→execute(evidence)→approve→close), the ERP flow (§5), group-admin one-screen 법인
  management, and 급여명세서 matching a 노무사 golden case.
- **`HANDOFF.md`** — deferred **port seams with no adapter**: `AiAssistantPort` (T6.1 — AI deferred, no
  mock) and `IdentityProviderPort` (T6.2). Business-action-gated externals: KCC LBS 신고, Kakao Alimtalk
  templates (code skips un-templated sends gracefully). The LocationPing 위치정보법 carve-out is a binding
  invariant (see KEEP-INTACT).
- **`docs/specs/**`** — deep domains that are foundation-only: `erp.md`/`accounting.md` (GL/AR/AP/e-tax,
  세무사-gated), `mes.md` (future scope, correctly deferred — no crate), `korean-institutional-connectivity*.md`
  (banking/MyData/NTS/NHIS-COMWEL EDI + certificate agent — external-integration-gated),
  `standalone-corporate-mailbox-server.md` (self-hosted MX/IMAP/JMAP — deferred; custom Rust mail kept),
  `operations-intelligence.md` (governed BI layer — folds into dashboard/explore; AI deferred),
  `cross-org-work-assignments.md` + `no-code-operational-logic.md` (no-code org/ops editor — partially met
  by the policy/workflow canvas + 대근/인력풀 + config objects).
- **`android/E2E-MANUAL-SMOKE.md`** — the mobile passkey CREATE/ASSERT ceremony + negative paths; the intent
  (mobile passkey enrollment/assertion, phone-QR handoff) is carried by the console identity flow and the
  KEEP-INTACT primitives above.
