> **HISTORICAL after the 2026-07-28 pivot.** This document preserves evidence and prior reasoning only. It cannot authorize Buck2, broad product scope, fan-out, or implementation work. Current product authority is [`docs/current/PRODUCT.md`](../current/PRODUCT.md), [`ROADMAP.md`](../current/ROADMAP.md), and [`DELIVERY.md`](../current/DELIVERY.md). Pivot history only: [`docs/PIVOT-2026-07-28.md`](../PIVOT-2026-07-28.md) (non-authority).

# THE PARITY MATRIX — intended shape (design markdowns) vs built console

> Master remaining-work checklist for the console program. **Binding directive: polish comes only after visual AND functional parity with the intended shape described in `docs/design/oyatie-console/*`.** This document derives that intended shape exhaustively from the design markdowns and diffs it against the built console.
>
> **Method.** Intended shape = `ROADMAP.md` module matrix + `DESIGN.md` §4 invariants (incl. §4.7 window model, token grammar @/#/!, §4-14 object card, §4-19..§4-26) + `HANDOFF.md` §0–§20 backend contract + `AGENTS.md` change-log through (101). Built state audited **evidence-based, no stack boot**, against the real build tip.
>
> **Source revision audited = `origin/main@86a97771a76b7e770dfcf8c6c7d83fd9d70a98bf`.** Every current-state statement below is revision-bound to that tree. No deployment, database, browser, or production-runtime verification was performed, so source presence is never presented as live or enterprise-readiness proof.
>
> **Phase 0 Support-only supersession (2026-07-25).** The `support` module row,
> the `SLO ≠ SLA` cross-cutting row, and their summary counts were re-audited at
> exact source `55d00f8aacaf8d1ba4db87b2f5345605af856a27`. That bounded overlay
> supersedes the older Support parity claims only; all other rows retain the
> earlier fixed-revision ceiling. Support is **PARTIAL**: local six-category
> defaults drive timers and alerts, approval is client-only and non-atomic, the
> three-bucket ontology schema is legacy/non-serving and taxonomy-incompatible,
> and `due_at` remains SLA rather than SLO authority. The approved next shape is
> one Support-owned immutable six-category elapsed-only policy aggregate with
> backend atomic approval. It is not implemented, and this matrix claims no
> migration, backend route, parity, deployment, or completion for it.
>
> **THE headline finding.** At the audited revision, the new `ConsoleShell` renders the main content area from `web/src/console/screens/registry.ts`. Structural extraction finds **36 literal nav entries, 11 registry bodies, and 25 chrome-only entries**. The 36 denominator deliberately excludes the `screen: string` type declaration that a raw text count would misclassify as navigation.

---

## 1. Summary table (counts by classification)

Classification scheme:
- **PARITY** — source-complete at the audited revision in the target ontology-first console and wired to a mounted real backend. This is source classification only, not deployed/runtime/browser/database or enterprise-readiness proof.
- **PARTIAL** - capability exists but a named source or integration-depth gap remains: backend wired yet no new-console body; a console component built but not mounted in ConsoleShell; console body present but backend dark; superseded legacy pages still host it; or a mounted body and backend still have a material source-wiring or authority gap.
- **MISSING** — no target-console surface at all (and often no backend).
- **N/A** — explicitly deferred by the design docs themselves.

| Layer | PARITY | PARTIAL | MISSING | N/A (deferred) | Total |
|---|---:|---:|---:|---:|---:|
| Module surfaces (ROADMAP section 4 / nav keys) | 4 | 27 | 6 | 2 | 39 |
| Cross-cutting grammar (DESIGN §4.x) | 2 | 11 | 2 | 1 | 16 |

**Bottom line:** with the bounded Support supersession, the matrix yields **4 PARITY / 27 PARTIAL / 6 MISSING / 2 N/A = 39** module rows. Cedar remains an accepted target baseline rather than a universal live authorization switch. Finance GL, notices, payroll-read, analytics-quant, C-chain seeding, 27-type tenant seeding, and one projected dispatch now exist in source, but their presence does not prove deployment or enterprise readiness. The largest deficits remain missing new-console bodies, incomplete closed loops, unverified durability/custody, the Support SLO authority split, and the absent ingest/recruiting/purchase/review/workforce product slices.

---

## 2. Module surfaces — per-module parity (ROADMAP §4)

Each row: intended capability (one line) · verdict · evidence (path/tag). "REGISTRY body" = has an entry in `web/src/console/screens/registry.ts` (renders in ConsoleShell). "backend tag N" = openapi op count under that tag (`backend/openapi/openapi.yaml`, 320 ops total). "mounted" = router merged in `backend/app/src/lib.rs build_router` (~30 routers).

### Overview / personal

| Module | Intended | Verdict | Evidence |
|---|---|---|---|
| overview | Palantir/Workday home: Task·WorkObject·KPI, agenda | **PARITY** | REGISTRY `overview`→`screens/overview/OverviewScreen.tsx` + `overviewApi.ts`; round 14 agenda depth |
| mywork (내 업무) | personal landing: 결재 차례·배차 큐·수령확인·오늘 할일 | **PARTIAL** | REGISTRY `mywork`→`screens/mywork/MyWorkScreen.tsx`; authenticated action-inbox + todo CRUD adapter, closed-world source-object drills, loading/error/empty recovery; runtime and closed-loop E2E evidence remain open |
| inbox (개인수신함) | passkey 수령확인 vault, InboxDoc legal docs | **PARTIAL** | backend `inbox` rest mounted + `passkeys(2)` + webauthn migration `0144`; **no console body** |

### HR / people

| Module | Intended | Verdict | Evidence |
|---|---|---|---|
| hr (인사) | Workday HCM: Person·인사카드 카테고리·PBAC | **PARTIAL** | backend `hr(10)`+`employees(8)` mounted; legacy `pages/EmployeesPage.tsx`; **no REGISTRY body** |
| recruit (채용) | Greenhouse: Posting·Applicant·인재풀 | **MISSING** | no console body; **no recruiting REST** (BE-5 gap); no legacy page |
| org (조직도) | Workday Org: Entity·Site·Team·Position, draft→archive | **PARTIAL** | backend `branches(5)`/`regions(4)`/`sites(2)` + legacy `OrgPage`/`GroupAdminPage`; no REGISTRY body |
| review (인사평가) | Lattice/15Five: Review·KPI·근태연동 | **MISSING** | no console body, **no backend** |
| directory (주소록) | Workday People: Person·조직 | **PARTIAL** | backend `employees(8)`/`users(7)`; no console body |

### Time / pay

| Module | Intended | Verdict | Evidence |
|---|---|---|---|
| att (근태) | Kronos/Deputy: 계획/실적·대근·주52h·월마감 | **PARTIAL** | backend `daily-work-plans(6)`; legacy `AttendancePage`; no REGISTRY body |
| pay (급여) | Workday Payroll: PayrollRun·PayItem·PS-·셀프서비스 | **PARTIAL** | mounted read-only payroll REST exposes draft-run readiness and self-scoped draft lines; it is not a persisted payroll calculation/payslip engine. Legacy `PayrollPage`; no console body |
| leave (연차) | Workday Absence: Leave·촉진·거부권 | **PARTIAL** | REGISTRY body has source-observed request/balance reads and source-wired decision/promotion calls; request creation is unwired and closed-loop E2E is absent. The round-labelled notice/receipt substrate lacks statutory timing and sequence enforcement |
| benefit (복리후생) | Workday Benefits: 제도 수명주기·tier | **PARTIAL** | tenant-scoped catalog REST materializes/hydrates generic lifecycle state and supports audited catalog/tier/eligibility replacement under PBAC/RLS; development-mounted REGISTRY body uses typed catalog creation and item-field edits and the generic governed lifecycle route. Production exposure remains disabled pending independent runtime and closed-loop evidence |

### Governance / docs / policy

| Module | Intended | Verdict | Evidence |
|---|---|---|---|
| appr (전자결재) | 그룹웨어+ServiceNow: Approval AP-·종결 | **PARTIAL** | `console/appr/ApprovalCompose.tsx` built + `governance(4)`/`approval-items(1)`; **not in REGISTRY** (rendered only via legacy `ConsoleModuleRoute` special-case, not new shell) |
| docs (문서·기록물/증거) | Foundry Docs·M-Files: 기록물·IN-·증거 WORM | **PARTIAL** | REGISTRY `docs`→`screens/evidence/EvidenceScreenBody.tsx` with real list/detail/verify/hold calls; backend evidence/integrity/lifecycle routes exist. External signer, out-of-band anchor, object-lock deployment proof, and media/ZIP depth remain incomplete |
| policy (권한·정책) | AWS Cedar/OPA: Policy·AccessGrant, no-code canvas | **PARITY (authoring surface only)** | REGISTRY `policy`→`screens/policy/PolicyBody.tsx`; authoring/simulation APIs exist, but ADR-0021 keeps live authorization unchanged until explicit coexistence-map enrollment and promotion |
| compliance | OneTrust/Purview: 의무 CP-·규제 RG-·FW-·DSR | **PARTIAL** | `crates/compliance` rest mounted (`location-consent(5)`); no console body; multi-jurisdiction PII deferred |
| audit (감사로그) | Splunk/CloudTrail: AuditEvent | **PARTIAL** | `console/audit/AuditFeed.tsx` **built but unrouted (orphan)**; backend `audit(1)`+`integrity(2)`+CEO covert stream mig `0147` |

### Foundry group (ontology / automation / analytics)

| Module | Intended | Verdict | Evidence |
|---|---|---|---|
| explore (객체탐색) | Foundry Object Explorer/graph | **PARITY** | REGISTRY `objectExplorer`→`screens/explore/ExploreBody.tsx`; backend `objects`/`object-links(3)`/traverse |
| ontology-manager (타입·매니저) | Foundry Ontology Manager: type editor·revision staging | **PARITY** (1 residual) | REGISTRY `ontologyManager`→`screens/ontology-manager/OntologyManagerBody.tsx`; backend `Ontology(10)`/`object-types(2)`/`object-actions(2)`. **Residual:** `registry.update_equipment` has a real App-tier projected dispatch, while every unregistered target remains fail-closed as `NotWiredYet` |
| auto/Automate (자동화·예약) | Workato/n8n: Workflow·Schedule, typed builder | **PARTIAL** | REGISTRY workflow+scheduled maps to screens/automate/AutomateBody.tsx and backend workflow-studio/workflow-runs source is present; runtime/browser integration, run-as authority, durable replay, and trigger-library depth remain open |
| dashboard | Foundry Quiver/Tableau: 파생지표 drill | **PARTIAL** | REGISTRY dashboard maps to screens/dashboard/DashboardBody.tsx and reporting/KPI source exists; React Router drill targets are not wired into the state.screen shell, so universal working drill behavior is unproved |
| laborcost (인건비/경영분석) | Foundry Contour/Adaptive: LaborCost·수익성 | **PARTIAL** | backend `financial`/`reporting` partial; legacy `KpiPage`/`OperationsIntelligencePage`; no console body |
| forecast (예측) | Anaplan/Foundry: 시나리오(규칙) | **N/A** (P4 deferred) | legacy `ForecastPage`; **no Monte-Carlo/EVT backend** (HANDOFF §18 quant deferred) |

### ERP

| Module | Intended | Verdict | Evidence |
|---|---|---|---|
| finance (재무) | SAP/NetSuite/더존: Voucher VC-·자동전표 | **PARTIAL** | REGISTRY finance body uses the source-wired adapter and mounted GL REST for list/create/get/submit/approve/post/reverse/account entries; migration `0160` enforces balance, posted immutability, append-only lines, and FORCE RLS. Period locks, full close/reporting, and runtime proof remain gaps |
| purchase (구매) | Coupa/SAP Ariba: PO-·Vendor·3-way match | **MISSING** | `crates/erp` = domain-only; **no purchase REST**; no console body |
| inventory (재고) | SAP MM/Fishbowl: IV-·안전재고 | **MISSING** (backend built-dark) | `crates/inventory` domain-only + table `0156`, **no REST/not mounted**; no console body |
| asset (자산) | ServiceNow ITAM/EAM: FL-·GPU·렌탈 | **PARTIAL** | backend `equipment(15)`/`equipment-substitutions(2)` + legacy `EquipmentPage`; module config had equipment loader (feat branch); no REGISTRY body |

### Field ops

| Module | Intended | Verdict | Evidence |
|---|---|---|---|
| dispatch (배차) | Samsara/Onfleet: WO- 큐×기사×SLA·지도 | **PARTIAL** | backend `dispatch` rest mounted + `p1-dispatches(3)`; legacy `DispatchPage`/`DispatchMapPage`; no console body |
| maintenance (정비) | UpKeep/Fiix/SAP PM: WO- | **PARTIAL** | backend `work-orders(14)`; legacy `MaintenancePage`/`WorkOrderDetailPage`; no console body |
| field (고객·현장) | ServiceNow FSM: CustomerSite·SLA | **PARTIAL** | backend `customers(1)`/`work-orders`; no console body |

### Comms

| Module | Intended | Verdict | Evidence |
|---|---|---|---|
| mail | Gmail/mox: Mail·threading·litigation hold | **PARTIAL** | `console/mail/*` uses typed account/folder/thread/read-state/send/reply/forward/download APIs, governance/egress states, and container-scoped 3→2→1 panes with URL-backed master/detail/compose state; inactive object/ingest/evidence controls removed pending contracts; rail summary via `CommsRailPanel`; **main full-view not in REGISTRY** (§4.8 promotion missing) |
| messenger (msgr) | Slack/Teams parity: Thread·presence·unfurl | **PARTIAL** | `console/messenger/*` built + backend `messenger(13)` + migs `0133-0143`; rail wired; **main full-view not in REGISTRY** |
| notif (알림) | notification pointers | **PARTIAL** | backend `notifications` mounted; no console body |
| board (게시판·공지) | Confluence/Slack: Notice NT-·수령확인 | **PARTIAL** | mounted notices REST provides draft/list/get/publish/ack/progress and recipient snapshots; no new-console board body |

### Contract chain / workforce / support / editor

| Module | Intended | Verdict | Evidence |
|---|---|---|---|
| contract (국가지원·조달·계약) | SAM.gov/나라장터/Icertis: Contract C-·Grant·Bid | **PARTIAL** | tenant seeding publishes the C-chain `contract`→`position`→`posting` ontology types; no contract console body, domain-owned contract workflow, grant, or bid product slice |
| ingest (데이터 인제스트) | Foundry Pipeline/Rossum: IngestJob DX-·Source·Template | **MISSING** | **no ingest crate, no DX- pipeline, no REST** anywhere; whole P1 pillar absent from build |
| workforce (인력풀) | WFM: WorkforcePool·대근 | **MISSING** | not in nav; backend `equipment-substitutions` partial; no console surface |
| support (지원센터) | Zendesk: SUP-·SLO(≠SLA) | **PARTIAL** | Support surface and ticket backend are source-present, but local six-category defaults drive timers/alerts. The separate three-bucket `support_slo_setting` is legacy, non-serving, and incompatible with the ticket taxonomy; settings approval is client-only and emits non-atomic writes. SLA `due_at` is not SLO policy authority. The approved backend-atomic six-category aggregate remains unimplemented. |
| editor (오피스 편집기) | ONLYOFFICE/Euro-Office fork | **N/A** (P3 deferred) | backend `office(4)` = governance shell (`app/src/office.rs`); real iframe editor deferred (HANDOFF §12) |

---

## 3. Cross-cutting grammar (DESIGN §4.x, §4.7)

| Capability | Intended (design ref) | Verdict | Evidence / residual gap |
|---|---|---|---|
| Window model | §4.7-2 pin/split·popout·tray·cross-screen persist | **PARTIAL (HIGH gap)** | full engine `console/window/*` exercised mainly in `window/harness.tsx`; **only 3 of 11 wired screen bodies consume it** (ontology workspace, graph explorer, support — verified `git grep origin/main -- screens/**`); the other 8 (leave, finance, dashboard, policy, evidence, overview…) use a reduced/no window model. **popout has no button, saveLayout/restoreDefault/width-drag API-only** (ledger F1 gaps) |
| objDrag reference tokens | §4-20/§4-23 every object = drag source | **PARTIAL** | `console/window/objDrag.ts` — 9 evidence-backed prefixes + multi-segment fix; **prefix regex hardcoded/triplicated = design's named gap** (not registry-derived) |
| Token grammar @/#/! | §4.7-7 mention/object/code, PBAC candidates | **PARTIAL** | `console/composer/*` (grammar, TokenComposer, PBAC-gated candidates) built; **surface breadth incomplete** (not on every input) |
| Object card 3-layer | §4-14 semantic/kinetic/dynamic + relation-draw + CRUD | **PARTIAL** | primitive is wired in `console/objectcard/wired.tsx`, but console-wide consumption is confined to a few bodies rather than the design-mandated grammar |
| Honest chart scaling | §4-24 axis-truncation chip | **PARITY** | `console/charts/honestScale.ts` + `HonestMarks` + `ProjectionPanel` (CI95/CVaR95) |
| Config console / add-anything | §4-22 add-anything · §19 component model | **PARTIAL** | `console/configconsole/*` (widget palette, add column/stat, `console_view` persisted via engine); **§4-22 full add-anything audit still OPEN (TODO #12)** — not end-to-end for every element type |
| Lifecycle engine (UI) | §3.9/§15 draft→archive, as-of | **PARTIAL** | `console/lifecycle/*` + `governance` backend; **bi-temporal as-of for projected types = D2 partial** |
| Guardrails (UI) | §3.10/§16 authority·checklist·four-eyes·egress | **PARTIAL** | governance gate-chain + mail egress wired; **checklist/four-eyes/SoD surfacing is backend-engine, thin UI** (ledger §85 judgment) |
| Cedar PBAC deny-by-omission | §4.5 screen/card/row/action/aggregate | **PARTIAL** | target/shadow primitives exist in `console/shell/authz.ts`, `policy/PolicyGate`, `api/policyCedar.ts`, and residual lowering, but ADR-0021 explicitly leaves live authorization unchanged until per-action enrollment, shadow evidence, and promotion |
| Audit backbone | §7 tamper-evident hash chain | **PARTIAL (DARK)** | seal/verify/gap-detection plumbing and integrity routes exist, but production sealing defaults OFF, the in-memory signer is not a trust root, NULL-org rows are excluded, and real tamper evidence requires an external signer plus out-of-band anchor |
| No-explanatory-UI / stat-strip | §4-11/§4-12 | **PARITY** | mechanically enforced by `check-ui-strings` gate |
| SLO ≠ SLA | §4-26 configurable setting objects | **PARTIAL** (support) | SLA `due_at` and internal SLO policy are not cleanly separated in serving source. Six local category defaults drive Support timers/alerts; the legacy three-bucket ontology object does not. Client-only, multi-write approval is not backend-atomic governance. The approved Support-owned immutable six-category elapsed-only aggregate is architecture only. |
| Comms rail↔main promotion | §4.8 | **PARTIAL** | `CommsRailPanel` (rail summary) wired; **main full-view promotion for mail/messenger MISSING** |
| DLP client layer-1 | §13.1 copy/ctx/devtools/print/watermark | **MISSING** (in console) | shipped in `.dc.html` prototype (AGENTS 87/89/90); **not ported to React console** (post-replica backlog). Layer-3 = deployment req (N/A) |
| Mobile employee app | §4.8 <768 app + bottom tab bar | **MISSING** | no `console/mobile/` dir; post-replica backlog |
| Closed-loop review protocol | §4-25 8-question loop | **N/A** (process) | mechanism = visual-verdict pipeline + `check-ui-strings`/gates; not a surface |

> **Primitive-consumption cross-check (vs `screens/**` on origin/main).** `HonestMarks`/`honestScale` and PBAC `hasPolicy`/`PolicyGate` are broadly consumed by the wired screen bodies (≥6 each — dashboard, policy, leave, finance, evidence, ontology, explore) — **not dead code**; the projection panel (EWMA/Student-t/CVaR) renders in the dashboard body in source. But `window-model`/`objDrag`, `GovernedObjectCard`, and `TokenComposer` are consumed by **only the ontology-workspace / graph-explorer / support** bodies — the §4.7 window model and the object-card/token grammar are effectively confined to those surfaces, not the console-wide grammar the design mandates. Minor: two parallel gate components (`PolicyGate` vs `PolicyGated`) coexist — a reduced-copy seam worth collapsing.

---

## 4. Backend contract (HANDOFF §0–§20)

Mounted routers in `backend/app/src/lib.rs build_router` at the audited revision include the earlier console/platform surfaces plus **notices, finance-gl, payroll, and analytics-quant**. A mounted source route is implementation evidence; it is not deployment or successful-runtime evidence.

| HANDOFF § | Contract | Verdict | Evidence |
|---|---|---|---|
| §0 | do-not-ship scaffold absent | **PARITY** | final-shape rule; view-as/pkAuth-sim/client-hash-chain absent from console |
| §2/§4.5 | Cedar PBAC authorize/simulate/CRUD, residual→SQL | **BUILT-WIRED (authoring/shadow only)** | endpoints and storage exist in `crates/policy` + `platform/authz`, but ADR-0021 does not switch live authorization; promotion remains per-action and evidence-gated |
| §3 | InboxDoc + passkey WebAuthn | **BUILT-WIRED** | `crates/inbox` rest + `passkeys` + webauthn mig `0144`, inbox_docs `0119` |
| §4 | 연차촉진·노무수령거부 | **BUILT-WIRED (partial console)** | `crates/leave` REST is mounted; the console reads requests/balances and POSTs decisions/promotions. Request creation is unwired; timing/sequence enforcement and closed-loop E2E are absent |
| §5 | finalization (작성자 종결·override) | **PARTIAL** | governance covers approvals; finalization-specific FSM thin |
| §6 | 자동화·예약작업 | **BUILT-WIRED** | `crates/workflow`+`app/src/workflow_*.rs`; migs `0105`/`0106`/`0127` |
| §7 | audit seal/verify chain | **PARTIAL / DARK** | seal/verify/gap-detection plumbing exists, but production sealing defaults OFF; the in-memory signer is dev/test only and not a trust root; NULL-org rows are outside the chain; external signer plus out-of-band anchor are required |
| §10 | 데이터 인제스트 DX- (parse/OCR/Template/lineage) | **MISSING** | **no crate, no pipeline, no REST** |
| §11 | 증거 WORM/hash/TSA/custody | **BUILT-WIRED (partial)** | `crates/docs` rest `evidence(4)` + `docs_evidence_objects` `0151`; **TSA/RFC-3161 nullable/deferred** |
| §12 | 오피스 편집기 | **PARTIAL** (deferred) | `app/src/office.rs` governance shell + `office(4)`; real editor iframe deferred |
| §14 | 메일 | **BUILT-WIRED** | `crates/comms` (imap/smtp/mox adapters) mounted + `mail(12)`; custom Rust stack (not mox rewrite) |
| §15 | 생애주기 엔진 effective-dating/as-of | **BUILT-WIRED (partial)** | `crates/governance`+`app/src/lifecycle.rs`+`lifecycles(3)`; bi-temporal projected = partial |
| §16 | 가드레일 엔진 preflight/four-eyes/egress | **BUILT-WIRED** | `crates/governance` gate-chain, self-approval CHECK; mig `0153`/`0158` |
| §17 | 엔터프라이즈 표준 SSO/SCIM/KMS/OTel/SIEM/TSA | **MISSING (mostly)** | FW- objects are UI-only; SSO/SCIM/KMS-envelope/OTel/SIEM/TSA-anchor deferred |
| §18 | 온톨로지 엔진 registry+instances+actions+analytics+traversal | **BUILT-WIRED (partial)** | ontology REST and objects paths exist; tenant seeding publishes 9 governed config + 3 C-chain + 15 projected domain types = 27. `registry.update_equipment` has a real projected dispatch; unregistered targets still fail closed as `NotWiredYet` |
| §19 | 구성 가능 콘솔 (DashComponent/console_view 영속) | **BUILT-WIRED (partial)** | `console_view` seeded through engine + governance deploy approval |
| §20 | 전면 CRUD 거버넌스 (override four-eyes) | **BUILT-WIRED** | `gov_approval_requests` `0158`; override reason+four-eyes |
| plain | payroll REST | **BUILT-WIRED (read-only partial)** | mounted list/get/self routes expose draft readiness; no persisted calculation-result/payslip amount engine |
| plain | inventory · benefit REST | **PARTIAL** | inventory remains built-dark; benefit catalog REST materializes/hydrates lifecycle state and has an unexposed development REGISTRY typed catalog creation and item-field edit body. Benefit production exposure still requires independent runtime and closed-loop evidence |
| plain | notices / board backend | **BUILT-WIRED (UI partial)** | mounted draft/list/get/publish/ack/progress routes; no board body |
| plain | analytics quant | **BUILT-WIRED (narrow)** | mounted authenticated stateless projection endpoint; no claim of Monte-Carlo/EVT or forecast-product completeness |
| plain | C-chain / recruiting | **PARTIAL / MISSING** | C-chain ontology types are seeded; contract product workflow and UI remain partial, while recruiting REST remains missing |

---

## 5. AGENTS change-log (92)–(101) feature parity

These are the **latest shipped state in the `.dc.html` design prototype** — each must be reflected in the built React console. Most are **NOT yet in the built console** because the built console (rounds 3-15) is a ground-up re-implementation that has reached 11 screen bodies, not the prototype's full surface set.

| AGENTS | Prototype feature | Built-console status |
|---|---|---|
| (91) leave 카드 존 (window-model) | window-zone wrap of leave sections | **PARTIAL** — LeaveBody wired; window-zone retrofit breadth is the §4.7 gap |
| (92)(93) benefit·docs 카드 존 | single card-zone retrofit | **PARTIAL (benefit/docs)** — benefit has a development-mounted typed catalog creation and item-field edit body with server data and governed lifecycle drill; production exposure and independently executed closed-loop runtime evidence remain absent. Evidence list/detail/verify/hold is wired but custody/durability proof is incomplete |
| (94) dashboard 위젯 제네릭 바인딩 | count/trend/dist widgets on ONT query | **PARTIAL** — DashboardBody wired; generic widget-add scope partial |
| (95) 기안 §68 투영 + fail-closed | structured 기안 + projection panel | **PARTIAL** — ApprovalCompose built, not in new shell |
| (96) 대시보드 실데이터 파생 | source-computed stats | **PARITY** — DashboardBody real-API |
| (97)(98) 급여·공고·월간 J/K/Enter | keyboard nav | **MISSING** — pay/recruit/att have no console bodies |
| (99) 미편성 결원 SLO 알림 시드 | SLO notif seed | **PARTIAL** — SupportBody renders alerts from local six-category defaults; no serving governed Support SLO aggregate or cross-surface seed is proved |
| (100) 증거 WORM 뷰어 | media pane + ZIP tree | **PARTIAL** — EvidenceScreenBody wired; media/ZIP viewer depth partial |
| (101) 인제스트 TP- 템플릿·hrSep·체크인·§18.2·커버플래너 | mapping templates, 퇴사/휴직 FSM, geofence check-in, sunset, cover planner | **MISSING** — ingest/att/hr bodies absent; §18.2 sunset partial (backend lifecycle only) |

---

## 6. RANKED gap register (impact × user-visibility)

### (a) Existing source slices that still need bounded completion

This is a revision-bound backlog, not a claim about any live workstream or person. At `origin/main@86a97771…`:
1. **Support SLO authority convergence** — replace the local six-category defaults plus legacy non-serving three-bucket schema with the approved one Support-owned immutable six-category elapsed-only policy aggregate and backend-atomic approval. Keep SLA `due_at` separate. No implementation or migration is claimed. *(HIGH — policy/governance correctness)*
2. **§18 projected-type dispatch breadth** — `registry.update_equipment` is real; every unregistered target must remain fail-closed until separately wired and verified. *(HIGH — engine correctness)*
3. **finance module depth** — source-wired GL exists; period lock, close/reporting, and runtime/database/browser evidence remain. *(HIGH)*
4. **Window-model retrofit breadth + popout/split/persistence** — §4.7 is confined to 3 of 11 wired bodies (ontology/explore/support); the full engine is otherwise harness-only. *(HIGH — dedicated retrofit candidate)*
5. **objDrag registry-derived prefixes** (drop the triplicated hardcoded regex). *(MED)*
6. **Evidence media/ZIP viewer depth**, **dashboard generic widget-add**, **SLO relabel sweep** — polish on wired screens. *(LOW-MED)*

> New bodies may be added only where their backing source contract is real and the complete slice can meet ADR-0025; incomplete navigation stays DARK rather than fixture-fed.

### (b) New charters — gaps nobody has chartered (the next waves)

Ranked by impact × visibility. **Top 10 program gaps overall are in bold** (these dominate the whole matrix, not just this list):

1. **hr (인사) console body** — backend `hr(10)`/`employees(8)` wired, nav key renders empty; core daily HR surface. *(HIGH)*
2. **att (근태) console body** — backend `daily-work-plans(6)` partial; empty canvas; every-manager surface. *(HIGH)*
3. **pay (급여) product completion** — read-only payroll readiness REST exists, but persisted calculation results, issued payslip semantics, and a new-console body do not. *(HIGH)*
4. **appr (전자결재) into new ConsoleShell** — ApprovalCompose built but only in legacy route; every employee. *(HIGH — mount + reconcile key-namespace)*
5. **mail + messenger main full-view promotion (§4.8)** — rail works, main canvas empty; core comms. *(HIGH)*
6. **mywork (내 업무) personal landing** — source-mounted and API-wired; runtime and closed-loop E2E evidence remain open. *(HIGH)*
7. **recruit (채용) surface + recruiting REST** — HR persona core; no backend, no body. *(HIGH — needs backend + UI)*
8. **org (조직도) console body** — backend wired, no body; org-lifecycle reference impl (§3.9.2) not surfaced. *(MED-HIGH)*
9. **dispatch/maintenance/field console bodies** — backend `work-orders(14)`/`dispatch`/`p1-dispatches` wired, 3 empty canvases. *(MED-HIGH)*
10. **inbox (개인수신함 passkey) console body** — passkey backend + webauthn wired, no body; legal 수령확인 evidence path. *(MED-HIGH)*

Further new charters (below the top 10): benefit/inventory REST + bodies (crates dark); purchase surface + REST; review(인사평가) surface + backend; laborcost/notif/directory/compliance/audit bodies; workforce(인력풀) surface; DLP client layer-1 port to React; mobile employee app (§4.8); §17 enterprise standard (SSO/SCIM/KMS/OTel/SIEM/TSA); identity onboarding mount; and nav-key↔ConsoleModuleRoute namespace reconciliation.

### (c) Deferred-epic debt (chartered separately in the ledger — still parity debt)

- **데이터 인제스트 DX- pipeline (§10)** — whole P1 pillar, no backend. *(design P1; deferred in build)*
- **contract C-/Position/Posting (§3 north-star chain)** — the three ontology types are seeded; product workflow, instances, links to employee, and console evidence remain.
- **오피스 편집기 (§12)** — ONLYOFFICE AGPL fork; office shell only.
- **DLP layer-3 (§13)** — enterprise browser/VDI/MDM = deployment requirement.
- **규제 PII / 다중 관할 (Jurisdiction/Consent/DSR objects)** — compliance multi-jurisdiction.
- **forecast product / advanced quant (§18 Monte-Carlo/EVT)** — the narrow deterministic projection endpoint exists; the broader P4 forecast product remains deferred.
- **메일 mox 백엔드 개조** (custom Rust stack kept instead) + **access-grant TTL 토큰 (break-glass)**.
- **Tier-2 rebrand** (repo/crates console-*→console-*), **buck2 adoption**, **bare-metal portability** (infra, not parity).

---

*Re-audited 2026-07-18 against `origin/main@86a97771a76b7e770dfcf8c6c7d83fd9d70a98bf` with source inspection only; no stack, deployment, database, browser, or production-runtime proof.*
