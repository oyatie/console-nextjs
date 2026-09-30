# Benchmark Matrix — Module `appr` (전자결재 / Electronic Approval Documents)

Scope: compose (기안) → approval line (결재선) → decision (승인/반려/거부) → delegation (대행/전결/대결) → finalize/종결 → receipts (수령확인) → template governance. Compared against Palantir Foundry, Slack, Microsoft Teams, Asana, n8n, Rippling, SAP.

Rigor: every vendor claim is `[V]` VERIFIED (source URL) or `[I]` INFERRED (labeled, reasoned from known product patterns). Our column is code-grounded (file:line evidence).

---

## Our console — evidence base (grep'd, not assumed)

- `web/src/console/appr/ApprovalCompose.tsx` (705 L), `composeModel.ts` (466 L), `composeApi.ts` (290 L), + tests.
- IA: **기안/compose is Source-present through the legacy special-case** (`ApprovalCompose.tsx`, template gallery, 결재선 preview). **결재함/상신함/수신함/반려함/완료함 boxes are planned and unwired** for the new shell; labels do not prove rendered tabs.
- Compose: template gallery (양식 선택) → title + reason **enum** + body with `@`mention / `#`channel / typed **object-code** autocomplete (`composeModel.ts:310-349`), typed object links (WO-/AP-/AT-/CS-… `OBJECT_CODE_RE` `:209`), evidence attachments linked to source-wired fixity/WORM-status and hold seams (deployment proof remains open), approval-line preview.
- Line states: `pending|current|approved|returned|rejected|skipped` — **sequential Vec**, no parallel/quorum (`composeModel.ts:1-7`, `mapLineState :395`).
- Decision: approve / reject / return + comment (`composeApi.ts:172`), `commentRequired` per node.
- Finalize 종결 + **delegate-finalize 대행** (`finalizeTask(mode: "author"|"delegate")` `:190`) + **post-finalization rejection 사후 반려** → compensation doc + notifications (`:203`, `deriveCompletionResult :359`).
- SoD: self-approval blocked in FE (`lineHasSelfApproval :409`) AND BE `gov_approvals CHECK(approver_id<>requested_by)` + §16 four-eyes gate-chain under current server authorization (`crates/governance/*`, ledger 2026-07-10). Cedar is the target authority stage, not verified live enforcement.
- Built on the workflow engine: `POST /api/v1/workflow-runs` → `/decide` → `/finalize`, idempotency keys, correlation id, object-links (`link_type: approval_target`), records-archive gating (`blocked_records_archive` `:353`).
- Templates: **source-wired** catalog = 4 workflow-studio submittable-definitions (equipment access / maintenance completion / purchase+payment / asset transfer — `workflow_studio.rs:167-196`, `required_payment_line` for purchase). A richer 8-form `APPROVAL_TEMPLATES` (ot/leave/expense/sub/purchase/benefit/reimburse/general, each w/ reason-enum + linked-object + default line + `receipt_required`) exists but is **`#[allow(dead_code)]`** (`workflow_studio.rs:198-285`) — **not wired to the gallery yet**.
- Current actions run through the legacy server authorization/middleware matrix and evidenced RLS/governance checks; Cedar per-action enrollment is target/shadow work under ADR-0021. Audit-chain and ontology-link evidence are separate from live Cedar enforcement.

**Honest gaps:** no parallel/quorum/any-of line; no native 전결/대결/후결/합의/협조 semantics (only delegate-finalize ≈ 대행); no out-of-office auto-delegation rule; no amount/attribute-driven dynamic routing (line is template-fixed); 8-form gallery dead-coded; no e-signature; no mobile approval surface (L-Mobile pending); receipts only partial (hr-leave `receipt_status` + promotion rounds).

---

## Capability matrix (rows = dimensions; each vendor line labeled [V]/[I])

### 1. Information architecture (inbox / outbox / draft / doc catalog)
- **Ours:** **compose (기안)** surface + template gallery + 결재선 preview are built; docs are workflow-runs coded AP-. The **함 IA (결재함/상신함/반려함/완료함 tabs) is a gap** — ko.ts strings only, no rendering component (see evidence base). Evidence-based (`ApprovalCompose.tsx`; `ko.ts:1930-1931`).
- **SAP:** approvers work the **My Inbox** Fiori app; drafts/documents live in the source object (PR/PO); no separate "approval doc" — the business object *is* the doc. [V] https://community.sap.com/t5/enterprise-resource-planning-blog-posts-by-members/flexible-workflow-for-sourcing-and-procurement-in-sap-s-4hana/ba-p/13554169
- **Teams:** dedicated **Approvals app** hub (Received / Sent / grouped), pinned in Teams rail. [V] https://learn.microsoft.com/en-us/microsoftteams/approval-admin
- **Asana:** no standalone approval inbox — approvals are tasks with an approval **task type** living inside projects/My Tasks. [V] https://help.asana.com/s/article/approvals
- **n8n:** N/A as an IA — no inbox; each pending approval is a *paused execution*; humans act in the notify channel (Slack/email), not an n8n doc list. [V] https://docs.n8n.io/advanced-ai/human-in-the-loop-tools/
- **Rippling:** unified approvals inbox across HR/IT/Finance/Spend; requests grouped by policy. [V] https://www.rippling.com/permissions
- **Palantir:** no approval inbox primitive; pending edits surface as Action submissions inside Workshop apps / Object Explorer, gated by submission criteria. [V] https://www.palantir.com/docs/foundry/workshop/actions-use
- **Slack:** requests are messages in channels/DMs; the "list" is your Slack history + the workflow's stored record; no dedicated doc archive UI. [V] https://slack.com/help/articles/360035692513-Guide-to-Slack-Workflow-Builder

### 2. Compose / 기안 authoring (templates, structured fields, rich body)
- **Ours:** template-driven; title + reason-enum + free body + typed-object autocomplete; validation (title/reason/targets/evidence required). Source-wired catalog only 4 forms; 8-form set dead-coded.
- **SAP:** no free "compose" — the requisition/PO/invoice form (transaction or Fiori app) is the document; fields come from the ERP data model, not a form builder. [I] (standard SAP MM/FI doc-entry pattern).
- **Teams:** admins/owners build **approval templates** with custom form fields + workflow settings; **requires a Microsoft Forms license**. [V] https://techcommunity.microsoft.com/blog/microsoftteamsblog/streamline-requests-with-new-approval-features-in-microsoft-teams/2259871
- **Asana:** intake via **Forms** → creates a task; task template + custom fields standardize the "doc". Source-cited form→work mapping. [V] https://forum.asana.com/t/use-workflow-and-rules-to-customize-tasks-from-forms-based-on-form-data/163461
- **n8n:** compose = a Form Trigger / send-and-wait payload; fully custom fields but developer-built, not end-user template. [V] https://docs.n8n.io/advanced-ai/human-in-the-loop-tools/
- **Rippling:** request forms are pre-modeled per domain (expense, PTO, access); fields bound to employee/transaction data, limited free authoring. [V] https://www.rippling.com/expense-management
- **Palantir:** the "form" is an **Action type** with typed parameters over the ontology; parameter values validated live before submit. Strong typing, no rich-text doc. [V] https://www.palantir.com/docs/foundry/action-types/overview
- **Slack:** **"Collect info from a form"** step gathers request details; message body built from form variables. Simple, no rich doc body. [V] https://slack.com/help/articles/360035692513-Guide-to-Slack-Workflow-Builder

### 3. Approval-line modeling (sequential / parallel / quorum / conditional)
- **Ours:** **sequential only** — line is an ordered node Vec; `commentRequired` per node; no parallel/quorum/any-of. Clear gap.
- **SAP:** **prominent** — flexible workflow supports single- or multi-step; **parallel approval per step**; agent determination by role/rule or custom **BAdI**; per-step decline handling. [V] https://community.sap.com/t5/enterprise-resource-planning-blog-posts-by-members/flexible-workflow-for-sourcing-and-procurement-in-sap-s-4hana/ba-p/13554169 · BAdI: https://int4.com/discover-approval-level-in-mm-flexible-workflow-agent-determination-badi/
- **Teams:** sequential + parallel + custom chains via **Power Automate**; routing order set per step. [V] https://techcommunity.microsoft.com/blog/microsoftteamsblog/streamline-requests-with-new-approval-features-in-microsoft-teams/2259871
- **Asana:** multi-approver via multiple approval tasks or rules; parallel = several assignees; no first-class quorum. [I] (from approvals + rules docs) https://help.asana.com/s/article/approvals
- **n8n:** any topology you can wire (branch/merge nodes) incl. parallel waits + "multiple approval options"; but hand-built per workflow. [V] https://community.n8n.io/t/human-in-the-loop-send-and-wait-for-response-with-multiple-approval-options/84085
- **Rippling:** **multi-level chains** with dynamic routing; single-or-multi approver logic; re-routes if approver on vacation. [V] https://www.rippling.com/permissions
- **Palantir:** no "line" concept — governance is submission criteria (who may submit/approve); sequencing modeled as chained Action types / status transitions. [I] https://www.palantir.com/docs/foundry/action-types/submission-criteria
- **Slack:** sequential multi-level via **conditional branching** (first-level → procurement → notify); parallel less native. [V] https://slack.com/blog/news/conditional-branching-workflow-builder

### 4. Delegation & proxy (전결 / 대결 / 후결 / out-of-office)
- **Ours:** **delegate-finalize (대행)** on the finalize step + post-finalization rejection; NO 전결(delegated authority by rule), NO 대결 out-of-office auto-swap, NO 후결. Partial.
- **SAP:** substitution rules (planned/unplanned) reassign work items when approver is out; release-code delegation encodes 전결-like authority-by-level. [I] (standard SAP Business Workflow substitution).
- **Teams:** reassign/forward an approval; no formal delegation-authority engine. [I] https://learn.microsoft.com/en-us/microsoftteams/approval-admin
- **Asana:** reassign task; no proxy-authority model. [I] https://help.asana.com/s/article/approvals
- **n8n:** re-route to another channel/person is just wiring; no authority concept. [V] https://docs.n8n.io/advanced-ai/human-in-the-loop-tools/
- **Rippling:** **auto re-route when primary approver on vacation** — closest to 대결; routing keyed to employee attributes so authority follows org changes. [V] https://www.rippling.com/permissions
- **Palantir:** "Current User" checks group membership/org → delegation modeled as group-based permission; no per-doc proxy UI. [V] https://www.palantir.com/docs/foundry/action-types/permissions
- **Slack:** no delegation primitive; workaround = branch to alternate approver. [I]
- **KR context:** 전결, 대결, and 후결 are configurable organizational authority semantics used in Korean approval products, not blanket legal equivalents to CEO signature. Their effect requires an entity/matter-specific authority matrix, corporate-organ validation, and qualified Korean counsel. The [Commercial Act](https://www.law.go.kr/lsLinkCommonInfo.do?lsJoLnkSeq=1016242805) and [Supreme Court decision 89DaKa3677](https://www.law.go.kr/LSW/precInfoP.do?evtNo=89%EB%8B%A4%EC%B9%B43677&mode=0) are official authorities illustrating scope-sensitive governance; neither supplies a universal product rule.

### 5. Decision actions (approve / reject / return / comment / re-route)
- **Ours:** approve / **reject** / **return(반려)** distinct + comment (`commentRequired`); return≠reject is Korea-correct.
- **SAP:** approve / reject with reason; decline routing configurable per step. [V] cloudbook.co.in/blog/sap-s4-hana-mm-release-strategy-flexible-workflow/
- **Teams:** Approve / Reject buttons + optional comment inline; e-sign path shows "who signs next". [V] https://www.docusign.com/blog/esignature-microsoft-teams-approvals
- **Asana:** approval outcomes **Approved / Changes requested / Rejected** built into the task type. [V] https://help.asana.com/s/article/approvals
- **n8n:** approve / disapprove + custom form response as outcome. [V] https://docs.n8n.io/advanced-ai/human-in-the-loop-tools/
- **Rippling:** approve / reject / request-info within the inbox. [I] https://www.rippling.com/permissions
- **Palantir:** submit succeeds/fails against criteria; per-condition **failure messages** shown to user (rich reject reasons). [V] https://www.palantir.com/docs/foundry/action-types/submission-criteria
- **Slack:** **Approve / Deny** buttons + optional comment directly in the message. [V] https://slack.com/help/articles/360035692513-Guide-to-Slack-Workflow-Builder

### 6. Object / data linkage & writeback (attach records, act on data)
- **Ours:** **strong** — targets are typed ontology objects; `link_type: approval_target` written on submit; primary target seeds `object_type/object_id`; finalize can gate records-archive. Ontology-first.
- **SAP:** the approval *is* the ERP transaction — approving releases the PR/PO and posts downstream; tightest data coupling of all. [V] cloudbook.co.in/blog/sap-s4-hana-mm-release-strategy-flexible-workflow/
- **Teams:** links a file/Loop/e-sign doc; no structured business-object writeback. [I] https://learn.microsoft.com/en-us/microsoftteams/approval-admin
- **Asana:** approval task links to project work; fields update via rules on outcome. [V] https://asana.com/features/workflow-automation/rules
- **n8n:** on approve, downstream nodes write to any connected system (DB/API) — unlimited but hand-built. [V] https://docs.n8n.io/advanced-ai/human-in-the-loop-tools/
- **Rippling:** approval **synced to payroll/ERP** on completion (expense→reimbursement→payroll in one flow). [V] https://www.rippling.com/blog/expense-management-software-benefits
- **Palantir:** **source-cited** — approval = an Action that **writes back to the ontology** (writeback dataset); the decision *is* a governed data edit. [V] https://www.palantir.com/docs/foundry/action-types/overview
- **Slack:** stores a record + can trigger downstream steps; no native structured object model. [I] https://slack.com/features/workflow-automation

### 7. Attachments / evidence / e-signature
- **Ours:** evidence attachments can reference source-wired fixity/WORM-status and hold seams plus an `evidence_required` policy; durable object-lock deployment and operational WORM proof remain open. NO e-signature integration.
- **SAP:** attach to the doc via ArchiveLink/DMS; e-sign via SAP Signature Mgmt / partners. [I]
- **Teams:** **native e-signature** (DocuSign / Adobe Sign) inside the Approvals app; admin controls which providers; template pre-tagging. [V] https://www.docusign.com/blog/esignature-microsoft-teams-approvals
- **Asana:** file attachments on tasks; e-sign only via integrations. [I]
- **n8n:** attachments/files pass as binary through nodes; e-sign via DocuSign node. [I]
- **Rippling:** receipts/docs attached to expense; policy checks against them. [V] https://www.rippling.com/expense-management
- **Palantir:** attachments as object properties/media; validation on attachment presence via submission criteria. [I]
- **Slack:** attach files to the request message; e-sign via app integration. [I]

### 8. Automation hooks / conditional routing (amount / attribute-driven)
- **Ours:** engine-backed (schedules/monitors exist) but **line is template-fixed** — no amount-threshold or attribute-based auto-routing on the approval doc itself.
- **SAP:** **release strategy = classic amount/plant/cost-center driven routing**; conditions pick the workflow + agents automatically. Selected cited routing reference. [V] cloudbook.co.in/blog/sap-s4-hana-mm-release-strategy-flexible-workflow/
- **Teams:** Power Automate rules route by any field/amount; triggers from many connectors. [V] https://techcommunity.microsoft.com/blog/microsoftteamsblog/streamline-requests-with-new-approval-features-in-microsoft-teams/2259871
- **Asana:** **Rules** engine (triggers: form submit, field change → actions: request approval, reassign). Bundles push rules to many projects. [V] https://asana.com/features/workflow-automation/rules
- **n8n:** the entire product *is* the automation graph — arbitrary conditional routing pre/post approval. [V] https://docs.n8n.io/advanced-ai/human-in-the-loop-tools/
- **Rippling:** **granular dynamic policies** — route by role/dept/level + amount/vendor/category; big-ticket flagged, small sails through. A cited attribute-driven routing example for HR/Finance. [V] https://www.rippling.com/spend-management-2-2
- **Palantir:** submission criteria = logical conditions over object/user/param; Foundry Rules deploy for broader flows. [V] https://www.palantir.com/docs/foundry/action-types/submission-criteria
- **Slack:** **conditional branching** (no-code) routes multi-branch by form values. [V] https://slack.com/blog/news/conditional-branching-workflow-builder

### 9. Permissions / SoD / four-eyes
- **Ours:** **strong on evidenced SoD, not live Cedar** — FE self-approval block + BE `CHECK(approver≠requester)` + §16 four-eyes gate-chain fail-closed, spoofed-requester hole closed (ledger 2026-07-10). Cedar per-action enforcement remains target/shadow until enrollment and promotion.
- **SAP:** authorization objects + release codes enforce who can release each level; SoD via GRC. [I]
- **Teams:** approver set per template; admin governance of app/providers; SoD not first-class. [V] https://learn.microsoft.com/en-us/microsoftteams/approval-admin
- **Asana:** project/task permissions; no dedicated SoD/four-eyes on approvals. [I]
- **n8n:** whoever holds the resume webhook approves; SoD is your design problem. [I]
- **Rippling:** **role-based permissions** deeply tied to HRIS; approver determined by org attributes. [V] https://www.rippling.com/permissions
- **Palantir:** **source-cited governance** — Current-User criteria (id/group/org), per-condition failure messages, ontology-level editing governance. [V] https://www.palantir.com/docs/foundry/action-types/permissions
- **Slack:** approver chosen in workflow; enterprise governance minimal for SoD. [I]

### 10. Audit / compliance / immutability / retention
- **Ours:** source evidence covers append-oriented governance tables, records-archive gating, correlation/idempotency, and audit seal/verify/gap-detection plumbing. Production sealing is OFF; the in-memory signer is not a trust root; NULL-org rows are excluded; external signing plus an out-of-band anchor is required. Cedar evaluation logs do not establish live-route Cedar enforcement.
- **SAP:** full workflow log + change docs + GRC audit; document retention via ILM. [I]
- **Teams:** approval stored w/ audit in M365 compliance; e-sign gives legal record. [V] https://www.docusign.com/blog/esignature-microsoft-teams-approvals
- **Asana:** activity log per task; no immutable/retention guarantees for compliance. [I]
- **n8n:** execution history logs each run; retention is your DB config; not compliance-grade OOTB. [I]
- **Rippling:** audit trail across HR/Finance; SOX-oriented spend controls. [I] https://www.rippling.com/spend-management-2-2
- **Palantir:** ontology writeback is fully versioned/audited; edits attributable + reversible. [V] https://www.palantir.com/docs/foundry/action-types/overview
- **Slack:** "permanent record" of the decision in Slack; not a compliance archive. [V] https://www.suptask.com/blog/slack-approval-workflow

### 11. Mobile
- **Ours:** **gap** — no mobile approval surface yet (L-Mobile <768 employee-app pending in ledger).
- **SAP:** My Inbox Fiori is responsive + SAP Mobile Start app. [I]
- **Teams:** full approve/reject in Teams mobile incl. push. [V] https://learn.microsoft.com/en-us/microsoftteams/approval-admin
- **Asana:** approvals in Asana mobile app. [I] https://help.asana.com/s/article/approvals
- **n8n:** approve via mobile email/Slack link (channel-native). [I]
- **Rippling:** mobile app for expense/PTO approvals + receipt capture. [I] https://www.rippling.com/expense-management
- **Palantir:** Workshop apps responsive; mobile approvals limited. [I]
- **Slack:** approve/deny from Slack mobile — frictionless. [V] https://slack.com/help/articles/360035692513-Guide-to-Slack-Workflow-Builder

### 12. Receipts / acknowledgement (수령확인)
- **Ours:** `receipt_required` flag exists; hr-leave has `receipt_status` + promotion rounds — **partial**, not surfaced on generic approval docs.
- **SAP:** goods-receipt / acknowledgment is a distinct posting step in the doc flow. [I]
- **Teams / Slack / n8n:** N/A — no receipt-confirmation concept; completion notification only. [I]
- **Asana:** N/A — task completion ≠ recipient acknowledgment. [I]
- **Rippling:** payment/reimbursement confirmation synced to payroll = de-facto receipt. [V] https://www.rippling.com/blog/expense-management-software-benefits
- **Palantir:** could model a "acknowledged" boolean writeback via a follow-up Action; not native. [I]
- **KR context:** 수령확인/후열(post-review acknowledgment) is a real 전자결재 step; global tools lack it. [V] https://sb.pe.kr/274

### 13. Extensibility / template governance (draft → approve → effective)
- **Ours:** **strong direction** — templates as governed ontology config (draft→approve→effective, rollback, as-of); definitions are versioned (`active_version`), four-eyes publish. But 8-form gallery still dead-coded.
- **SAP:** workflows configured in customizing (transportable across DEV→QA→PRD); BAdI for custom logic. Governed but IT-heavy. [V] https://int4.com/discover-approval-level-in-mm-flexible-workflow-agent-determination-badi/
- **Teams:** template management by admins/owners; changes take effect immediately, no staged approval of the template itself. [V] https://techcommunity.microsoft.com/blog/microsoftteamsblog/streamline-requests-with-new-approval-features-in-microsoft-teams/2259871
- **Asana:** **Bundles** = reusable rules+fields+templates pushed to many projects centrally; excellent scale-out, no draft→approve gate on the bundle. [V] https://help.asana.com/s/article/bundles-faq
- **n8n:** workflows are versioned JSON, git-exportable; no business-user template governance. [I]
- **Rippling:** policies edited centrally, adapt to HRIS changes automatically; limited staging. [V] https://www.rippling.com/permissions
- **Palantir:** Action types are governed platform artifacts, versioned & permissioned; ontology-native. [V] https://www.palantir.com/docs/foundry/action-types/overview
- **Slack:** workflows editable in Workflow Builder; publish is immediate, no staged approval. [I] https://slack.com/help/articles/360035692513-Guide-to-Slack-Workflow-Builder

### 14. Korean 전자결재 fit (전결규정 / 근로기준법 / group-company scoping)
- **Ours:** the Source-present legacy special-case implements 기안/compose, return≠reject, reason enums, current server-authorized scoping, and Korean-first strings. 결재함/상신함 and other queue boxes are target IA, not implemented current state; Cedar authorization and 전결/대결/후결/합의/협조 remain targets.
- **SAP:** localizable but 전결규정 must be modeled as release-code hierarchy; heavy consulting; 합의/협조 line types not native. [I]
- **Teams / Slack / Asana / n8n:** **mismatch** — no 결재선/전결/대결/합의/협조 concepts, no return-vs-reject distinction, no 근로기준법-aware leave interplay; Korean orgs bolt these on. [I] (absence in the sampled cited product docs above).
- **Rippling:** US-centric HR/payroll; no Korean 전자결재 or 근로기준법 modeling. [I]
- **Palantir:** neutral engine — could *model* 전결규정 in the ontology but ships nothing pre-built. [I]
- **KR reference for the gap:** 전결/대결/후결/합의/협조 + 위임전결규정 are baseline Korean groupware. [V] https://namu.wiki/w/결재 · https://www.hanbiro.com/software/groupware-workflow-approval.html

---

## Per-vendor: "how they'd build OUR appr module"

**SAP (S/4HANA)** — Would collapse the standalone "approval doc" into the business object: no compose screen, the PR/PO/leave-request *is* the document, and approval = a **release strategy** whose conditions (amount, cost-center, plant) auto-select a multi-step, possibly-parallel line with agent-determination BAdIs. 전결 = release-code hierarchy; 대결 = substitution rules. Rock-solid routing + data coupling + audit; brutal to configure, IT-owned, weak on ad-hoc free-form 기안. Our ontology-object linkage is already SAP-shaped — we'd steal their **condition-driven auto-routing**.

**Microsoft Teams** — A template-per-request-type hub: admins define approval templates with Forms fields, drop in **native e-signature** (DocuSign/Adobe), and route via Power Automate (sequential/parallel/branch). Approve/Deny inline on web + mobile + push. Lightweight, great UX, enterprise-governed at the M365 level, but shallow on structured business-object writeback and zero Korean 전결 semantics. We'd steal **inline e-sign + first-class mobile approve**.

**Asana** — Approval as a **task type** with Approved/Changes-requested/Rejected outcomes, fed by **Forms**, automated by **Rules**, and scaled with **Bundles** (push the same rules+fields+template to every team). Beautiful intake→work mapping and central reuse; but no SoD/four-eyes, no immutable audit, no delegation-authority, no Korean line model. We'd steal **Bundles** (governed template reuse across 법인/branch) and **Forms→structured doc**.

**n8n** — Pure graph: an approval is a **send-and-wait / human-in-the-loop** node that pauses the run and resumes on a webhook, with arbitrary pre/post routing and multi-option responses. Infinitely flexible, developer-built, no inbox, no compliance archive, no Korean semantics. It mirrors what our workflow engine already does under the hood — validation that our engine-backed approach is sound; we'd steal the **pause/resume + multi-option decision** ergonomics for long-running lines.

**Rippling** — HRIS-driven: approval chains built from **employee + transaction attributes** (role, dept, level, amount, vendor), auto-re-routing when an approver is on vacation, and completion synced straight to payroll/ERP. Source-cited dynamic routing + org-aware delegation (closest thing to 대결). US-centric, no 전자결재 culture, limited free-form docs. We'd steal **attribute-driven dynamic routing** and **out-of-office auto-reroute**.

**Palantir Foundry** — There is no "approval doc"; there's an **Action type** with typed parameters and **submission criteria** (logical conditions over object/user/org with per-condition failure messages) that writes back to the **ontology** on approve. The decision *is* a governed, versioned, audited data edit — an example of the ontology-first pattern with typed criteria and writeback. No inbox, no Korean line model, no free 기안 body. We'd steal **submission-criteria-as-typed-predicates** and **per-condition failure messages**.

**Slack** — Approval as a **Workflow Builder** flow: a form step collects the request, a message step posts it, **Approve/Deny buttons** decide inline, **conditional branching** does multi-level routing, and a permanent record is stored. Frictionless, mobile-native, no-code; but not a compliance archive, no structured objects, no 전결/대결. We'd steal **inline button decisions in the comms rail** (we already have a messenger — unfurl an approval card there).

---

## What we'd steal — ranked (capability → selected cited reference → ontology-grammar fit → cost)

1. **Condition-driven auto-routing of the approval line (amount / attribute / object-type thresholds)** → **SAP release strategy** (+ Rippling for attribute logic) → *excellent fit*: our line is already typed-object-anchored; route via typed predicates in the same Cedar/ontology grammar we use for policy. **Cost: M** (line model already exists; add a condition-eval step, reuse policycanvas predicate blocks).
2. **Parallel / quorum / any-of approval nodes** → **SAP** (parallel per step) / **n8n** (fan-out) → *good fit*: extend the sequential Vec to a node-graph with `all_of`/`any_of`/`n_of`. **Cost: M** (model + engine + UI on the line preview).
3. **전결 / 대결 / 후결 delegation semantics + out-of-office auto-reroute** → **Rippling** (org-aware reroute) + **KR groupware** (authority model) → *strong Korean-enterprise fit, not a statutory mandate*: model as delegated-authority objects in the ontology, effective-dated, audited. **Cost: M/L** (governance-grade; touches authz + audit).
4. **Submission-criteria-as-typed-predicates with per-condition failure messages** → **Palantir Foundry** → *native fit*: our validation is ad-hoc booleans today; upgrade to typed predicate blocks reusing policycanvas, surfacing precise "why blocked" reasons. **Cost: M**.
5. **Bundles — governed template reuse across 법인/branch** → **Asana** → *strong fit*: our templates are already ontology config (draft→approve→effective); package reason-enums+line+linked-objects+fields and deploy per org unit with four-eyes. **Cost: S/M** (wire the dead-coded 8-form set as the first bundle).
6. **Inline decision in the comms rail (approve/deny card unfurl) + first-class mobile approve** → **Slack** + **Teams** → *good fit*: we already have a messenger + object-card unfurl (F1/F2); render an approval card with Approve/Reject/Return buttons; extend to L-Mobile. **Cost: M**.
7. **Pluggable e-signature over evidence-required docs** → **Teams** (DocuSign/Adobe as one integration shape) → *moderate fit*: a signer abstraction over evidence records. Provider selection for Korean deployments must be based on customer adoption, assurance, integration, and authoritative legal requirements; the Electronic Signature Act does not support a blanket exclusion of foreign providers. **Cost: L** (provider abstraction + DLP/egress gate).
8. **Wire the dead-coded 8-form 기안 gallery** (ot/leave/expense/sub/purchase/benefit/reimburse/general) → **ours, blocked on ourselves** → *trivially our grammar* → **Cost: S** — highest ROI/lowest cost; unblocks #5.

Sources: SAP flexible-workflow/BAdI (community.sap.com, cloudbook.co.in, int4.com); Teams Approvals + DocuSign (learn.microsoft.com, techcommunity.microsoft.com, docusign.com); Asana approvals/rules/bundles (help.asana.com, asana.com, forum.asana.com); n8n human-in-the-loop (docs.n8n.io, community.n8n.io); Rippling permissions/spend (rippling.com); Palantir Foundry action-types (palantir.com/docs); Slack Workflow Builder (slack.com/help, slack.com/blog, suptask.com); KR 전자결재 (namu.wiki, sb.pe.kr, hanbiro.com, prix.im).

---

## Cross-cutting lens findings (5 independent review lenses)

- **Task-flow:** compose = **~6–8 interactions** (the object-linking is a manual typed search even when you already hold the object via objDrag); decide = **1–2 clicks** (SoD self-block enforced). Vendors collapse three ways: (a) decide-from-the-notification (Concur/Slack/Teams, 1 click, 0 nav); (b) Korean groupware pre-binds 결재선 by document type; (c) 20+ mobile templates vs our thin catalog. **The gap is ERGONOMICS, not semantics.** **Steal:** decide inline from inbox/push [M]; auto-bind 결재선 by template [M]; launch-approval-FROM-the-object-card with target pre-linked (kills the manual search) [M]; richer template catalog.
- **IA / layout:** `ApprovalCompose` is the right foundation — **the sampled SAP/Workday surfaces do not show the same Korean approval combination**: 결재선 (순차/병렬/전결/대결), 상신/수신/반려/완료 함 separation, and drafter-vs-approver **state dualism** (the sampled SAP/Workday surfaces use a flatter approval inbox). **Steal:** 결재선 builder (순차/병렬/전결/대결) as the compose core [M]; **함 IA** (상신함/수신함/반려함/완료함 as master-detail tabs) [M]; drafter/approver dual-state chips [S].
- **Data-model:** ours is a **governed ontology object** with evidenced as-of/fixity seams + four-eyes SoD (approver≠requester CHECK); Cedar is the accepted target authority, not a current live-enforcement advantage. 더존/Flow store approvals in RDBMS rows without the same evidenced tamper controls. **But 결재선 semantics** (ordered multi-step line, 전결 rules, 대결/위임, 병렬 vs 순차) are a Korean-culture requirement the incumbents model natively and we only partially do. **Steal:** typed 결재선 model — ordered multi-step line, 전결/대결/위임, 병렬 vs 순차 [L, KR must-have]; 문서양식 typed forms (ont_object_type per form) [M]; 부재중 위임 as an effective-dated authority grant [M].
- **Governance:** **Local-fit distinction in this sample** (per-node state including `returned`/`skipped`, post-finalization rejection + compensation); **gap on reusable abstraction and delegation depth.** **Steal:** delegation 전결·대결 (a governed grant object w/ TTL + Cedar principal-attribute) [M]; 합의 (parallel co-sign) node type [S]; escalation on stalled node [M]; SoD ruleset on the approval line (block a line where the same person appears at two conflicting roles) [M].
- **Automation / extensibility:** this design contains more of the Korean 전자결재 grammar than the sampled global-product surfaces, but lacks **결재선-as-config**. **Steal:** routing-modifier rules as governed config (전결규정 → 결재선, amount/type-driven, evaluated by the same predicate engine as policies) → Workday BP [M]; parallel + 합의(concur) + 대결(deputy) step types [M]; a mandatory commit step (결재 완료 as an explicit terminal transition that triggers downstream automation) [S].

**Reconciled current state:** only compose/기안 is Source-present through the legacy special-case. 결재함/상신함/수신함/반려함/완료함 are planned and unwired new-shell boxes, not shipped inbox/outbox/archive IA. E-signature remains target work.
