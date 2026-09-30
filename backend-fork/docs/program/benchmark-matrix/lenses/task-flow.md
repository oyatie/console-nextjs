# TASK-FLOW LENS — money task step/click count, ours vs vendors (14 modules)

> **Benchmark evidence metadata**
> - Observation/revalidation date: 2026-07-19.
> - Sampled products/surfaces: Oyatie 14-module task flows; Slack and Teams; Workday; Palantir; SAP; ServiceNow; Asana; n8n; Rippling; named Korean groupware patterns.
> - Evidence modality: Fixed-target repository source plus live-checked public official documentation/product pages and explicitly labeled public secondary pages; hands-on product tenants, screenshot capture, deployment, activation, and production validation were not performed.
> - Scope/claim ceiling: Only the named pages, surfaces, and fixed-target source are in scope; no whole-product, current-production, provider-parity, universal-superiority, legal, tax, labor, deployment, activation, or production conclusion.
> - Legend: [V] = bounded external observation with a direct URL or same-document source-list entry; [E]/[code] = fixed-target repository observation; [I] = recommendation or inference. Every steal/adopt item is [I].

Method: for each module I name its ONE money task, count the interaction steps in
OUR console (evidence: `web/src/console/**` read directly + `docs/program/console-program-ledger.md`),
then compare vendor step-count under the Rigor-label contract below. Direct URL or same-document source-backed
product observations may carry the verified label; derived step counts and comparisons, known-product-pattern
extrapolations, and all recommendations carry the inference label. A URL alone does not convert an inference into
a verified observation. Korean 전자결재 / 근로기준법 / group-scope fit is called out where the sampled global-vendor
surfaces miss it.

Rigor labels: [V]=bounded direct observation with a URL or same-document source entry; [I]=derived comparison, step count, extrapolation, or recommendation; N/A=vendor doesn't play here.

---

## 1. overview

**Money task:** triage my inbox — see what needs me, act on it.
**OURS (evidence):** overview calls the action-inbox endpoint in source, derives source-observed stats/counts, feeds nav badges, and gives each row a source-route action. The missing step-collapse is inline/ObjectCard completion: a row routes to its source instead of approving/rejecting/acknowledging in place, so completion remains **2–3 steps**.
**Vendors:**
- Slack/Teams: the inbox item IS the action surface — approve/reject buttons render inline on the message/adaptive card, **0 navigation, 1 click** [I] slack.com/blog (Approvals Bot, deals 70% faster); learn.microsoft.com (universal actions for adaptive cards).
- Workday: "My Tasks" inbox, open item → act = **2 steps** [I] (standard Workday inbox pattern).
- Palantir Foundry: home = Workshop module with Object Table; row → Object View → action button = **2–3 steps** [I] palantir.com/docs/foundry/workshop.
**Where vendors collapse what we spread:** Slack/Teams make the notification itself terminal (decision without opening anything). Our source-observed row routes to the source surface before completion.
**What we'd steal:** actionable inbox rows (approve/reject/ack rendered on the row, PolicyGated) → collapses overview money task from 3 → 1. Vendor: Slack/Teams. Fit: our PolicyGated + GovernedObjectCard action layer already exists; render its top action inline on the row. Cost: **M**. **[I]**

## 2. dashboard

**Money task:** spot an anomaly → drill to the offending object.
**OURS:** scope×period presentation, real-API stat strip, honest charts, and authored **drill targets** are Source-present.
The absolute React Router targets are registered by `AppRouter` as legacy `ConsoleShell`/`AppShell` routes. They exit the carbon-console shell and bypass its `state.screen`/ObjectCard flow; universal working drill-to-ObjectCard is not established, browser behavior remains unverified, and the exits are not browser-proven.
**Vendors:** Palantir documents widget/filter/Object View drill patterns [I]; Workday/SAP report drill-through is multi-hop [I].
**What we would steal:** route targets through the console screen/object-explorer model, browser-prove each path, and retain honest-scale/stat-strip strengths. Cost **S–M**. **[I]**

## 3. finance

**Money task:** approve a voucher / vendor bill.
**OURS (source at `origin/main@86a97771…`):** `FinanceModuleScreen` has a source-wired adapter to the mounted finance-GL list/create/get/submit/approve/post/reverse/account-entry contract. The database/domain enforce nonempty positive balanced vouchers, posted immutability, append-only lines, reversals, and FORCE RLS. This proves a source-wired slice, not deployed operation or complete accounting breadth; list → row → detail action is the intended **2–3-step** path.
**Vendors:**
- SAP Concur mobile: open app → **one click** approve or send-back-with-comment; summary + invoice image + line items on one glance [I] concur.com/blog (How Concur Invoice Works).
- NetSuite: Enter Bills > List → Edit → set Approved → Save = **~4 steps**; but **bulk approve** page approves many at once [I] docs.oracle.com/netsuite (Approving Vendor Bills / Bulk Approving).
**Where vendors collapse:** Concur = 1-click from a rich single-glance card + mobile push. NetSuite = bulk approve (N bills, 1 action).
**What we'd steal:** (a) prove the mounted voucher flow in runtime/database/browser tests and surface close/period state; (b) 1-click approve-with-image glance card (Concur); (c) bulk/batch approve. Vendor: Concur (glance+1-click) + NetSuite (bulk). The finance slice already has direct mounted REST; projected ontology action breadth is a separate integration choice. Cost: **M/L**. **[I]**

## 4. people (HR / 인사)

**Money task:** open an employee, take an HR action (change position, start offboarding).
**OURS:** employee is one of the 15 seeded projected read types; Position and Posting exist in the three-type C-chain. Domain crates remain the writers, employee action dispatch is not registered, and employee↔position/effective-date depth remains incomplete. Open-and-view is available in principle; uniform act-from-object is not.
**Vendors:**
- Workday: worker profile → Related Actions (the orange "twinkie") → action = **2–3 steps**, one consistent action menu on every object [I] (Workday Related Actions pattern is canonical).
- SAP SuccessFactors: employee file → action = **2–3 steps** [I].
**Where vendors collapse:** Workday's Related Actions menu is on every worker uniformly. Our ObjectCard aspires to this, but projected action dispatch is currently proven only for `registry.update_equipment`.
**Korean fit:** global HCM weak on 4대보험 filing, 법정 수령확인 문서, 연차촉진 — our default type catalog targets these (a potential local-fit distinction once wired).
**What we'd steal:** Workday Related-Actions uniformity → keep employee's domain crate as sole writer, register explicit employee action targets, and present them consistently in ObjectCard. Vendor: Workday. Cost: **M/L**. **[I]**

## 5. leave (연차)

**Money task:** submit a leave request / decide a team member's request.
**OURS (implemented UI shape, `leave/LeaveConsole.tsx`):** ONE screen holds all four personas. LeaveConsole calls the request/balance and decision/promotion endpoints in source; team decide remains an inline **1-click** control, and 촉진 uses the same source-wired surface. Self-submit is not wired—the validated form ends in a non-submitting approval-template link—and the full path lacks closed-loop E2E proof.
**Vendors:**
- Workday 2025 "Manage Absence": fewer clicks, request/edit in the mobile Absence calendar worklet — but self-service, decide, and 촉진 are SEPARATE worklets [I] hr.osu.edu/news/2025/05 (New Manage Absence), workday.com/absence.
- Korean 근태 (Shifty/flex): request on one screen [I].
**Where vendors collapse:** Workday reduced its own click count but still spreads self/approve/policy across worklets. Our co-location and source-wired decision/promotion call sites are present in source, but the cited Workday surface is more complete end to end until request creation and the full loop are verified here.
**Korean fit:** the repository provides a round-labelled receipt/notice substrate: the domain validates labels `1|2`, and the adapter records idempotent receipt-gated notices. It enforces neither statutory timing nor round sequencing, and refusal does not prove a prior round `2`; this is not a native §61 FSM or a proven end-to-end differentiator.
**What we'd steal:** preserve the co-located-personas pattern, but wire request creation, implement deadline and round-sequence enforcement, and prove submission, decision, promotion, receipt, audit, failure, and retry E2E. Only then propagate it. Cost: **M**. **[I]**

## 6. support (SUP-)

**Money task:** resolve a ticket.
**OURS (evidence):** §4-11 stat strip + chip filters + right-pin detail (ledger support-slo, fe-fix-wave1). Resolve = filter chip → row → detail pin → action. SLO (not SLA) modeled as configurable setting object. Resolve = **~3 steps, one ticket at a time, no canned reply/macro**.
**Vendors:**
- Zendesk: **one-click macro** = canned reply + multiple field changes bundled; apply via toolbar button OR type `/` inline (no leaving keyboard); 7 most-used macros float to top [I] support.zendesk.com (Using macros / Creating macros).
**Where vendors collapse:** Zendesk macro = N field updates + reply in 1 click; we change fields one action at a time.
**Korean fit:** neutral (support is domain-generic).
**What we'd steal:** saved macros (bundled field-set + reply as a reusable ontology action) + `/`-inline apply. Vendor: Zendesk. Fit: our ontology Action types ARE bundled writebacks — expose "saved action bundle" as a macro. Cost: **M**. **[I]**

## 7. evidence (EV-)

**Money task:** produce audit evidence for a record (verify fixity, apply/release hold).
**OURS (evidence, `evidence/EvidenceCard.tsx` + `crates/docs/rest`):** EvidenceCard with WORM/fixity/custody/legal-hold status; verify = **1 click** on the source-wired path; hold = four-eyes (distinct approver). Per-record, on-demand. Object-lock deployment and trusted audit anchoring are unproved. Verify one record = **1 click**; assemble an audit package = **N clicks (one per record)**.
**Vendors:**
- ServiceNow GRC / Audit Mgmt: **continuous automated evidence collection** — control testing + attestation auto-collect evidence before the audit, "reducing audit fatigue"; single-pane Audit Workspace [I] servicenow.com/docs (GRC Audits), store.servicenow.com (Audit Management).
**Where vendors collapse:** ServiceNow makes audit-time evidence steps ≈ **0** (evidence pre-collected continuously); ours is verify-on-demand (N steps at audit time).
**What we'd steal:** scheduled/continuous auto-verify + auto-attestation so an audit package assembles itself. Vendor: ServiceNow GRC. Fit: L20 seal worker already runs; extend to scheduled re-verify + package rollup. Cost: **M**. **[I]**

## 8. object-platform (ontology / 온톨로지)

**Money task:** create a new object type (no-code) that wires itself end-to-end.
**OURS (evidence, ontology-coverage-matrix + `console/ontology`):** OntologyManagerScreen (type list + 6-subtab editor + revision staging). BUT add-a-type is **NOT no-code end-to-end — 6 manual steps** documented: generic create-action not auto-attached (user types can't create instances at all), code-prefix regex triplicated/hardcoded (silent drag/parse fail), MOD_SCREENS hardcoded 2-entry map, ko.ts labels, FE ONT_TYPES mirror, free-text policy/automation candidates.
**Vendors:**
- Palantir: create Action Type in Ontology Manager → auto-available in Workshop/AIP/Automate; edit objects via Action-backed Button Group; writeback dataset required once [I] palantir.com/docs/foundry/workshop/actions-use, /ontology/overview.
**Where vendors collapse:** Palantir: define the type/action once in Ontology Manager, it propagates to every downstream app automatically. We require 6 hand-edits per new type — the exact opposite of collapse.
**What we'd steal:** auto-propagation on publish (create-action auto-attach + registry-derived code prefixes replacing the triplicated regex + data-driven MOD_SCREENS + ONT_TYPES from GET /object-types). Vendor: Palantir. Fit: this is literally Phase C wave 2's stated acceptance test. Cost: **L**. **[I]**

## 9. policy (Cedar PBAC)

**Money task:** author a policy that denies/permits an action, verify it before ship.
**OURS (evidence, `console/policycanvas` + Cedar authoring REST):** P→R→A→Effect block canvas + typed predicates + server simulation + review flow. Author = drop blocks → set predicates → simulate → submit → review, about **5+ interactions** for a simple rule. This measures the authoring surface only; ADR-0021 explicitly does not switch live authorization, which remains on the current engine/RLS unless an action is enrolled, shadowed, and promoted.
**Vendors:**
- AWS Cedar / OPA: policy-as-text (`permit(...) when {...}`) — fast for authors who write, opaque for non-devs [I] (Cedar is our own backend spine).
- Palantir governance: policy on object/property via UI [I].
**Sampled task-flow tradeoff:** for a simple rule, the cited text/one-line path uses fewer interactions than a canvas. Our canvas supports branching but is overkill for "deny X to role Y".
**Korean fit:** group-company (법인) scoping + clearance is modeled in the Cedar target contract. Live enforcement remains the current engine plus RLS except for any separately evidenced coexistence-map promotion.
**What we'd steal:** a "quick rule" linear path (single P→R→A→Effect row) alongside the canvas for simple policies; keep canvas for branching. Vendor: Cedar text ergonomics. Fit: the block model already serializes; render a 1-row form variant. Cost: **S–M**. **[I]**

## 10. automate (자동화 / Automate)

**Money task:** build a trigger→action automation.
**OURS (evidence, `console/workflows` + Automate hub):** BlockCanvas typed nodes + 2px connectors + branch ≥2 outputs + source-present sim + runLog; effect = ontology action; monitors-as-definitions. Build = drag nodes → connect → configure → publish (four-eyes). Simple 1-trigger-1-action = **canvas-heavy, ~5+ steps**.
**Vendors:**
- Zapier: **linear** trigger→action, plain language, 1-click app connect — fastest for simple, weak at branching [I] n8n.io/vs/zapier.
- n8n: node canvas (like ours), more config, and clearer branching visibility in the cited surface [I] n8n.io/vs/zapier.
- Palantir Automate: effect = Ontology Action (same model as ours) [I] palantir.com/docs/foundry/workshop/actions-use.
**Where vendors collapse:** Zapier collapses the simple case to a linear 2-step form; we (rightly) match n8n's canvas but pay n8n's simple-case tax.
**What we'd steal:** Zapier-style linear quick-automation for the trigger→action 80%; our effect=ontology-action already matches Palantir (keep). Vendor: Zapier (simple path). Cost: **M**. **[I]**

## 11. comms (messenger)

**Money task:** turn a conversation into a decision/action on an object.
**OURS (evidence, `console/messenger`):** 3-tier rail, reply-in-thread, presence, object-card unfurl, objDrag reference-token drop into composer (PBAC-gated). You can DRAG an object into chat and unfurl its card — but the DECISION still happens on the module surface, not in the rail. Chat-to-action = **drag/unfurl in rail, then leave to act = 2+ context switches**.
**Vendors:**
- Slack/Teams: the message IS the transaction — approve/reject inline in-thread, 1 click, no leaving chat [I] slack.com/blog, learn.microsoft.com (adaptive card universal actions).
**Where vendors collapse:** Slack/Teams close the loop inside chat; we unfurl the object but bounce the user out to decide.
**What we'd steal:** in-rail action buttons on the unfurled object card (approve/ack/decide without leaving the rail) — we already unfurl + PolicyGate; add the terminal action. Vendor: Slack/Teams. Fit: CommsRail↔main promotion + GovernedObjectCard action layer exist; wire the action into the unfurl. Cost: **M**. **[I]**

## 12. appr (전자결재)

**Money task (draft side):** compose an approval; **(decide side):** approve/reject.
**OURS (evidence, `appr/ApprovalCompose.tsx`):**
- Compose: select template (1 click) → fill title/reason(enum)/body → **type-and-submit object search form** → click each result to link target → review 결재선 preview → SoD check → submit. = **~6–8 interactions**, the object-linking is a manual typed search even when you already hold the object.
- Decide (`ApprovalDecisionPanel`): type comment → approve/reject/return = **1–2 clicks**; self-block is modeled; the supported delegation primitive is `delegate-finalize`/대행, alongside post-finalization reject + compensation.
**Vendors:**
- Korean groupware (Hiworks/Douzone/Daou): 20+ mobile templates, draft-and-approve on mobile in real-time, 결재선 set by document type all at once, SMS/messenger notify [I] hiworks.com/market/approval, biz-solution.hiworks.com/product/function/groupware/approval, daouoffice.com/features_approval.jsp.
- SAP Concur / Slack / Teams: 1-click approve from push/inline [I] concur.com/blog, slack.com/blog.
**Where vendors collapse:** (a) decide from the notification (Concur/Slack/Teams) — 1 click, 0 nav; (b) Korean groupware pre-binds 결재선 by document type so the drafter never hand-picks approvers; (c) rich template library (20+) vs our thin catalog.
**Korean fit:** source-modelled approval primitives include 결재선 preview, `delegate-finalize`/대행, post-finalization reject, SoD/four-eyes, reason enum, and compensation. Native 전결/대결/후결 semantics remain explicit gaps; do not collapse them into delegation or claim them as shipped.
**What we'd steal:** (1) decide inline from inbox/push (Concur/Slack) — biggest collapse; (2) auto-bind 결재선 by template so the drafter skips approver picking (Korean groupware); (3) launch-approval-FROM-object-card with target pre-linked (kills the manual object search); (4) richer template catalog. Vendors: Concur/Slack (1-click decide) + Hiworks/Douzone (결재선 + templates). Cost: **M** (decide-inline) / **M** (pre-link from card). **[I]**

## 13. field (현장 / work order)

**Money task:** dispatch a work order to a technician (dispatcher) / complete it on-site (tech).
**OURS (evidence):** the new ontology console has no dedicated field body. Work order is a seeded projected read type, but no work-order action target is registered; legacy field-service pages and a native Android field app exist outside the new shell. The new-shell dispatch path therefore lacks a proved closed loop.
**Vendors:**
- ServiceNow FSM: intelligent dispatch auto-matches skill/location/availability (dispatcher assigns in ~1 action or fully auto); tech mobile app shows task + asset + parts in one place, offline-capable [I] servicenow.com/products/field-service-management, servicenow.com/docs (mobile agent).
- Salesforce Field Service: similar auto-dispatch + mobile [I].
**Where vendors collapse:** ServiceNow auto-assigns (dispatcher does 0 manual matching) and gives the tech a single all-in-one mobile card (WO + asset + parts).
**What we'd steal:** (a) auto-dispatch matching (skill/location) as an ontology automation; (b) reconcile the existing Android field app with the new-shell work-order contract; (c) a single all-in-one WO card. Vendor: ServiceNow FSM. Fit: add an explicit domain-owned work-order action dispatch and prove the closed loop. Cost: **L**. **[I]**

## 14. compliance (규제 CP-/RG-/FW-)

**Money task:** run a compliance check / simulate a policy against the regulation.
**OURS (evidence):** compliance UI surface = Phase C wave 2 (ledger); typed policy REAL evaluation for compliance is a listed BACKEND residual ("typed policy real evaluation for compliance"). Today compliance leans on the policy simulator + evidence surface. Run-a-check = **partially wired, simulate via policy canvas (~3 steps), real typed-policy eval pending**.
**Vendors:**
- ServiceNow GRC: control → continuous test → auto-evidence → issue remediation, cross-module (control change instantly updates linked risks/policies/audits) [I] servicenow.com/docs (GRC), inmorphis.com (cross-module).
- AuditBoard: control-test workflow [I].
**Where vendors collapse:** ServiceNow's cross-module propagation — update a control once, every linked register/policy/audit reflects it (0 redundant re-testing). Ours would require touching each surface.
**Korean fit:** multi-jurisdiction 규제 / PIPA consent / DSR objects are in our default catalog roadmap (epic) — global GRC is weak on Korean statutory specifics.
**What we'd steal:** cross-module propagation via the single ontology engine (a control IS an object; linked risks/policies/audits are link-types → one edit propagates for free). Vendor: ServiceNow GRC. Fit: this is exactly what single-engine ontology buys us once compliance objects are registered. Cost: **L**. **[I]**

---

# TOP-10 CROSS-MODULE FINDINGS (ranked by user-value) **[I]**

1. **One-click decide from the notification/inbox is the universal step-collapse overview lacks.** Slack/Teams/Concur make the message or push terminal — approve/reject in 1 click, 0 navigation [I] slack.com/blog, concur.com/blog, learn.microsoft.com. Leave already has a source-wired inline decision call, but overview rows only source-route and do not complete inline/ObjectCard actions. Fix = server-authorized inline completion on the source-observed inbox row. Touches modules 1,3,5,11,12. **Highest cross-module ROI.**

2. **Launch actions FROM the object with context pre-linked, don't re-search.** ApprovalCompose forces a manual typed object-search to link targets even when the user already holds the object (we have objDrag!). Palantir/Concur pre-attach the source object [I] palantir.com/docs/foundry/workshop/actions-use. Collapse: "기안" button on the ObjectCard → approval opens with target pre-linked. Touches 4,8,12,13.

3. **Finance voucher source wiring now exists, but runtime proof and workflow depth remain.** The mounted REST/domain/database slice supports approval/posting transitions; peers still lead on glance-card and bulk UX. Verify the closed loop, period-close integration, and deployment before claiming operational parity.

4. **Projected action breadth remains narrow.** `registry.update_equipment` is the one real App-tier projected dispatch; unregistered employee/work-order targets fail closed. Finance has a separate mounted REST slice and should not be described as backend-absent. Expand each projected target through its domain use-case without creating a second write source.

5. **No macro/bulk collapse anywhere.** Zendesk 1-click macro (canned reply + N field changes) and NetSuite bulk-approve resolve N-updates-in-1 [I] support.zendesk.com, docs.oracle.com/netsuite. Our support + appr + finance are strictly one-item-at-a-time. Our ontology Action types ARE bundled writebacks — expose them as saved "macro" bundles + batch-select. Touches 3,6,12.

6. **Our leave surface is a promising interaction pattern that must be closed-loop-proven before propagation.** It co-locates request inputs, source-wired team-decision calls, and source-wired 촉진 calls, and ledger on one screen, but request creation is unwired and the full path is unproven E2E. Wire creation and verify the complete audit/failure/retry path first; then use the shape as the reference grammar for people/support/compliance. **[I]**

7. **Add-a-type is 6 manual hand-edits vs Palantir's define-once-propagate-everywhere.** Palantir: create Action Type in Ontology Manager → auto-available in every app [I] palantir.com/docs/foundry/workshop/actions-use. We hand-edit regex/MOD_SCREENS/ko.ts/ONT_TYPES per type. Auto-propagation on publish is Phase C wave 2's acceptance test — it directly determines whether "no-code configurability" is real.

8. **Compliance/evidence audit-time steps should be ≈0 via continuous auto-collection.** ServiceNow GRC pre-collects control evidence continuously so audit day is push-button [I] servicenow.com/docs. Ours is verify-on-demand (N clicks, one per record). Extend the L20 seal worker to scheduled re-verify + package rollup. Touches 7,14.

9. **Canvas authoring is right for branching but overkill for the 80% simple rule (policy + automate).** Zapier collapses simple trigger→action to a linear 2-step form [I] n8n.io/vs/zapier; our block canvas (correctly n8n-class for branching) taxes the simple case ~5 steps. Add a linear "quick rule / quick automation" path beside the canvas. Touches 9,10.

10. **Korean 전자결재 depth is the target differentiator; protect exact semantics and steal the ergonomics.** Current primitives cover 결재선 preview, `delegate-finalize`/대행, post-finalization reject, SoD/four-eyes, round-labelled leave notices/receipts, and 법인 scope. Native 전결/대결/후결, statutory leave timing/sequence enforcement, request creation, and closed-loop leave E2E remain gaps. Korean groupware (Hiworks/Douzone) also leads on auto-bound 결재선, template breadth, and mobile one-click flows [I] hiworks.com/market/approval, daouoffice.com. Close the semantic and runtime gaps without relabeling 대행. Touches 5,12.
