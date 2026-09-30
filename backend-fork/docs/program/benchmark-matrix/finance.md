# Benchmark Matrix — Module: **finance** (GL vouchers / journal entries, document flow, balance verification, period locks)

> **Benchmark evidence metadata**
> - Observation/revalidation date: 2026-07-19.
> - Sampled products/surfaces: Oyatie finance GL/voucher source; SAP FI/Universal Journal; Rippling Spend/GL sync; Palantir Ontology Actions; n8n accounting nodes; Slack, Teams, and Asana approval-edge surfaces.
> - Evidence modality: Fixed-target repository source plus live-checked public official documentation/product pages and explicitly labeled public secondary pages; hands-on product tenants, screenshot capture, deployment, activation, and production validation were not performed.
> - Scope/claim ceiling: Only the named pages, surfaces, and fixed-target source are in scope; no whole-product, current-production, provider-parity, universal-superiority, legal, tax, labor, deployment, activation, or production conclusion.
> - Legend: [V] = bounded external observation with a direct URL or same-document source-list entry; [E]/[code] = fixed-target repository observation; [I] = recommendation or inference. Every steal/adopt item is [I].

**Scope:** How each product handles the general-ledger transaction: creating a voucher/journal entry, tracing it back to its source document, proving debit=credit and account validity, and locking a fiscal period so history can't be re-posted.

**Most-relevant vendors:** SAP (FI/GL — the reference implementation), Rippling (spend→GL sync), Palantir (Ontology writeback/Actions). Slack / Teams / Asana / n8n play only at the automation-and-approval edges of this module (see per-cell N/A reasons). **[I]**

**Rigor:** every vendor claim is [V] VERIFIED (source URL) or [I] INFERRED (reasoned from known product patterns). Our column is code-evidenced from this repository.

---

## OUR CONSOLE — actual state (code-evidenced)

Audited at `origin/main@86a97771a76b7e770dfcf8c6c7d83fd9d70a98bf`: `web/src/console/modules/*`, the finance screen body/adapter, `backend/crates/finance-gl/*`, migration `0160`, and the app router mount.

- **Source-present frontend descriptor plus dedicated finance-GL domain, not seeded generic ontology.** The frontend `typeRegistry` type descriptor records voucher fields, relation/link presentation, and the displayed balance analytic. The finance module surface consumes it for columns, stat-strip, and detail-link presentation. `finance_voucher` is absent from the exact 27 seeded generic ontology keys, so the descriptor is not an engine seed or proof of generic ontology backing. The dedicated finance-GL routes, FSM, database guards, and runtime-role tests are separate Source-present evidence; deployment, production operation, and accounting certification remain unproved. **[code]**
- **Document flow is modeled as object links.** Detail linkChips wire a voucher to its source across domains: `dx_ingest`, `approval`, `payroll_run`, `purchase_request`, `contract`, `gl_account`, `cost_ledger`, plus `lifecycle`, `audit_trail`, `object_graph` (`moduleScreens.ts:454-463`). This is lineage-as-graph, which is the design differentiator.
- **A separate period-lock substrate exists.** `backend/crates/platform/db/src/period_lock.rs` + migration `0107_create_period_locks_versioning_lifecycle.sql` are RLS-tested for their existing ledger paths. The finance-GL post route does not prove an integrated fiscal-close/period-lock gate, so this remains a finance completion item.
- **Authorization chronology is explicit.** The current server/legacy authorization matrix remains the live enforcer; the client `PolicyGate` consumes a non-authoritative UI feature projection and deny-read omits the offered module (`FinanceModuleScreen.test.tsx:95`). Cedar remains target/shadow only until a finance action is enrolled, shadow-proven, and explicitly promoted under ADR-0021 and `docs/specs/cedar-pbac-coexistence-map.json`; current coexistence entries are `legacy_only`.
- **The GL slice is built in source.** Mounted `/api/v1/finance-gl` routes provide list/create/get/submit/approve/post/reverse/account-entry operations. The real console adapter targets those paths. Migration `0160` defines voucher/header and line tables with FORCE RLS, a DB balance gate, posted immutability, append-only lines, and reversal links; the domain also fails closed on empty, nonpositive, overflowed, or unbalanced vouchers.
- **Korean-native by construction.** 재무/전표 labels, `voucher_source → approval` is the 전자결재 spine, org-scoped via RLS `app.current_org` (group-company / 법인 isolation).

**One-line honest self-assessment:** a real source-wired GL/voucher slice now exists; period-close integration, source-document auto-materialization, chart-of-accounts validation, full reporting/close, and deployed runtime/database/browser proof remain open.

---

## Capability Matrix

Legend: **[V]** verified w/ source · **[I]** inferred · **N/A** genuinely out of module.

### 1. Information architecture (how a GL transaction is modeled)

| Ours | SAP | Rippling | Palantir | n8n | Slack | Teams | Asana |
|---|---|---|---|---|---|---|---|
| Source-present frontend `finance_voucher` type descriptor plus dedicated finance-GL header, lines, and relation/link presentation; the key is absent from the 27 seeded generic ontology keys. [code] | Universal Journal: header in BKPF, lines in ACDOCA. [I] | Spend transactions map coding fields to the downstream GL. [I] | Generic Ontology objects with typed links; no native accounting schema. [I] | No data model; passes JSON. [I] | Messages/forms; no GL object. **N/A**. | Approval requests; no GL object. **N/A**. | Tasks/custom-fields; no GL object. **N/A**. |

### 2. Voucher / journal-entry entry flow

| Ours | SAP | Rippling | Palantir | n8n | Slack | Teams | Asana |
|---|---|---|---|---|---|---|---|
| Source-wired adapter + mounted REST support draft create → submit/balance-check → approve → post → reverse, with list/detail/account-entry reads. Source-wired; runtime not exercised here. [code] | Park (draft) → Post; posting date auto-derives period & fiscal year; manual + recurring + template entries. [I] | Auto-generates JEs from card spend/bills/payroll; user rarely hand-keys — "coded to GL in real time". [I] | Actions create/edit/link objects with rule sets; a "create JournalEntry" Action would be built, not shipped. [I] | Xero native node covers Contacts/Invoices only; "Manual Journals" needs raw HTTP node. [I] | Form → routed message; no posting. **N/A**. | Approval form; no posting. **N/A**. | Task-form intake; no posting. **N/A**. |

### 3. Document flow / source-to-posting lineage

| Ours | SAP | Rippling | Palantir | n8n | Slack | Teams | Asana |
|---|---|---|---|---|---|---|---|
| Lineage is native: voucher linkChips to dx_ingest / approval / PO / contract / payroll + object_graph view. **Design lead.** [code] | Classic "Document Flow" / relationship browser: PO→GR→invoice→payment chain, drill-down to source. [I] | Card→expense report→reimbursement→GL coded end-to-end; source receipt attached to the synced line. [I] | Ontology links + writeback datasets capture full provenance of each edit back to system-of-record. [I] | Can *move* a doc between systems but keeps no lineage graph itself. [I] | Thread history only. **N/A**. | Comments+timestamps on the approval, not on a GL doc. [I] | Task attachments/subtasks; not GL lineage. **N/A**. |

### 4. Balance verification (double-entry debit=credit + valid GL account)

| Ours | SAP | Rippling | Palantir | n8n | Slack | Teams | Asana |
|---|---|---|---|---|---|---|---|
| Domain and DB gates require at least one positive line and equal checked debit/credit totals before advancement; migration `0160` recomputes the cross-row sum. Account codes are nonblank but a governed chart-of-accounts validity check is not proved. [code] | Hard invariant: a document that isn't balanced cannot post; account/field validations + substitution rules at post time. [I] | Categorization/coding validated against CoA before sync; the double-entry invariant lives in the downstream GL, not Rippling. [I] | No built-in accounting invariant; you'd encode "Dr=Cr" as an Action validation rule. [I] | No validation engine; you'd script the check in a Function/IF node. [I] | **N/A**. | **N/A**. | **N/A**. |

### 5. Period locks / posting-period control

| Ours | SAP | Rippling | Palantir | n8n | Slack | Teams | Asana |
|---|---|---|---|---|---|---|---|
| Generic period-lock source and runtime-role tests exist for existing ledger paths, but integration with finance-GL posting and a complete fiscal-close workflow is not established. [code] | OB52 / Manage-Posting-Periods app: closed periods block new postings; authorization-group override; special periods for adjustments. [I] | "Close the books faster" — but the period close/lock lives in the connected GL (QBO/NetSuite), Rippling feeds it. [I] | No native fiscal-period lock; modelled as object state + Action guard. [I] | No period concept. **N/A**. | **N/A**. | **N/A**. | Can gate a "close" task but no ledger period lock. **N/A**. |

### 6. Approval / four-eyes / SoD (전자결재)

| Ours | SAP | Rippling | Palantir | n8n | Slack | Teams | Asana |
|---|---|---|---|---|---|---|---|
| REST exposes distinct submit/approve/post transitions and the domain rejects illegal state edges. A full finance SoD ruleset and amount/attribute routing are not proved; current authorization is the legacy server matrix plus FORCE RLS. [code] | Park→release two-step, workflow for JE approval, authorization-object SoD; strong but config-heavy. [I] | Multi-level spend/bill approval policies before GL sync. [I] | Actions granularly permissioned (analyst opens, only managers close) — approval-as-permission. [I] | Orchestrates approvals across apps but has no opinion on SoD. [I] | Workflow Builder: forms + conditional branching (≤15+5 conditions), dynamic approvers, logged decisions. [I] | Approvals app: create/approve/reject/reassign, comments+timestamps, Purview audit. [I] | Native Approvals action inside Rules; move task on "Approved". [I] |

### 7. Permissions & scoping (group-company / 법인)

| Ours | SAP | Rippling | Palantir | n8n | Slack | Teams | Asana |
|---|---|---|---|---|---|---|---|
| Current server/legacy authorization matrix + non-authoritative UI projection; `finance_gl_vouchers`/lines have FORCE RLS org isolation, and deny-read omits the offered module. Cedar is target/shadow pending explicit promotion. [code: authz.ts, mig 0160] | Company-code / authorization-object granularity; the enterprise reference but heavyweight to admin. [I] | Role-based; entity/subsidiary scoping for multi-entity spend. [I] | Per-Action writeback permissions by user group/condition. [I] | RBAC + SOC2; app-level, not GL-object-level. [I] | Workspace/channel ACL; not row-level financial scope. [I] | Team/tenant ACL; Purview governs audit. [I] | Project/team membership; not entity-scoped GL. **N/A** for GL scope. |

### 8. Automation hooks (auto-posting, rules, integrations)

| Ours | SAP | Rippling | Palantir | n8n | Slack | Teams | Asana |
|---|---|---|---|---|---|---|---|
| Manual draft creation and lifecycle REST exist. Automatic materialization from payroll/purchase/ingest source documents is not established. [code/design] | BAPIs / substitution / recurring-entry / intercompany auto-posting; deep but ABAP-gated. [I] | AI-powered auto-categorization + policy rules auto-code every card txn to GL. [I] | Actions ↔ process models drive state transitions/writeback automation. [I] | **The specialist**: trigger/action nodes across QBO/Xero/Stripe; charges per workflow-run not per task. [I] | Conditional-branch workflows, no-code. [I] | Power Automate flows behind Approvals. [I] | Rules engine + Bundles (reusable template/rule packages). [I] |

### 9. Audit / compliance trail

| Ours | SAP | Rippling | Palantir | n8n | Slack | Teams | Asana |
|---|---|---|---|---|---|---|---|
| Voucher persistence and transitions emit audited source seams, but production audit sealing is OFF and the in-memory signer is not a trust root. No universal or runtime-complete voucher custody claim is made. [code] | Full document change logs, audit trail, GoBD/SOX-grade; the compliance benchmark. [I] | Immutable spend audit trail feeding SOX close checklists. [I] | Writeback datasets = every edit versioned & attributable. [I] | Execution logs per run; not a financial audit trail. [I] | Logged decisions in threads; not tamper-evident. [I] | Purview audit log of every approval event w/ timestamps. [I] | Task activity log; not financial-grade. [I] |

### 10. Mobile

| Ours | SAP | Rippling | Palantir | n8n | Slack | Teams | Asana |
|---|---|---|---|---|---|---|---|
| Native field app exists (com.console.app) but finance voucher UI not on it yet. [code] | Fiori mobile + S/4 mobile approve; JE entry is desk-work. [I] | Strong mobile: snap receipt, approve on phone. [I] | Mobile/Workshop apps; JE approval possible. [I] | No first-party mobile authoring. [I] | Full mobile approvals. [I] | Full mobile approvals. [I] | Full mobile app. [I] |

### 11. Extensibility (chart of accounts, dimensions, custom fields)

| Ours | SAP | Rippling | Palantir | n8n | Slack | Teams | Asana |
|---|---|---|---|---|---|---|---|
| Frontend descriptor centralizes displayed voucher fields/links; generic ontology auto-propagation from a new property/relation is not established. [code] | Coding blocks/custom fields. [I] | Fixed spend schema + GL mapping. [I] | Open Ontology. [I] | Generic HTTP. [I] | Forms only. [I] | Approval fields. [I] | Custom fields + Bundles. [I] |

### 12. Reporting / trial balance / close

| Ours | SAP | Rippling | Palantir | n8n | Slack | Teams | Asana |
|---|---|---|---|---|---|---|---|
| Source-rendered page-derived stat strip exists; account-entry reads exist. Trial balance, P&L/BS, reconciliation, and full close remain unbuilt. [code] | Real-time TB/P&L/BS off ACDOCA, no subledger reconciliation needed at close. [I] | Month-end-close checklist + faster close via real-time GL sync. [I] | BI/analytics off Ontology objects; you build the close app. [I] | Reports = sync data elsewhere. [I] | **N/A**. | **N/A**. | Close *checklist* as a project, not financials. [I] |

---

## Per-vendor: "how they'd build OUR finance module"

**SAP (FI/GL — the reference).** [V/I] They'd make the voucher a Universal-Journal line: one ACDOCA-style wide table, park→post with a non-negotiable Dr=Cr gate, OB52 period control with authorization-group overrides, and substitution/validation rules at post. Everything reconciles by construction — no subledger drift. The trade-off is exactly what our ontology avoids: configuration weight (posting-period variants, auth objects, ABAP substitutions) that needs a consultant. Our steal from SAP is the *invariant discipline*, not the admin surface. **[I]**

**Rippling (spend).** [I] They'd never ask a human to key a voucher — the JE is a *byproduct*. Card swipe / bill / payroll run → AI auto-categorizes to the CoA → real-time GL-coded sync → month-end close checklist. They own the source-to-code automation and mobile receipt capture; they *don't* own the ledger, period lock, or double-entry invariant (that's the connected GL). Their philosophy maps cleanly onto our `voucher_source → payroll_run/purchase_request` links: the voucher should mostly *auto-materialize* from the linked source, not be typed.

**Palantir (writeback).** [V/I] Closest to our own grammar. JournalEntry becomes an Ontology object; "post voucher" is an Action with a validation rule (Dr=Cr) and granular writeback permissions (analyst drafts, controller posts) — approval-as-permission, matching our planned create/post authorization split. Period lock = object-state + Action guard. Provenance is free via writeback datasets. What they lack that we already have: a *real, RLS-enforced* period lock in the DB rather than a modelled guard.

**n8n (automation).** [I] They wouldn't build the module — they'd wire it. Trigger on an approved 전자결재, HTTP-node the voucher into the GL, sync to QBO/Xero. Native accounting nodes are shallow (Xero = Contacts/Invoices; Manual Journals need raw HTTP), so it's glue, not ledger. Relevant only as the *integration egress* pattern for our voucher→external-GL sync.

**Slack.** [I] N/A for the ledger; they'd own the *approval front-door*: Workflow-Builder form with conditional branching and dynamic approvers, logged decisions, then hand off to a real GL. Useful reference for how our 전자결재 request should *feel* (form + SLA + logged thread), not where it should live.

**Microsoft Teams.** [I] Same N/A on ledger; prominent at *audited approval*: Approvals app + Purview logs every create/approve/reject/reassign with timestamps. Reference for the audit-event granularity our audit_trail chip should expose.

**Asana.** [I] N/A for accounting; they'd model the *close as a project* — Rules + Approvals + Bundles to run a repeatable month-end checklist with handoffs. Reference for our close-orchestration UX, never for the numbers.

---

## What we'd steal (ranked, ontology-fit + cost) **[I]**

1. **Complete account validity and fiscal-period enforcement at post time.** → SAP is the cited reference for this capability. Fit: Dr=Cr already fails closed in domain and DB; add governed chart-of-accounts validity and prove the finance-GL post route observes the period lock. **Cost: M.** **[I]**
2. **Rippling's auto-materialized voucher from the source doc.** → selected reference: Rippling. Fit: the mounted voucher REST and source-link fields now exist; have the workflow engine emit a draft voucher on source-doc finalization. **Cost: M.** **[I]**
3. **Palantir's approval-as-permission + validation-in-Action.** → selected reference: Palantir. Fit: our planned create/post authorization split is this pattern — after explicit enrollment, shadow evidence, and promotion under ADR-0021 and `docs/specs/cedar-pbac-coexistence-map.json`, formalize post as a Cedar-gated Action carrying the balance validation, so SoD and the invariant are one gate. **Cost: S** (extends the existing advisory PolicyGate and server authorization boundary). **[I]**
4. **SAP's document-flow drill-down as the default voucher view.** → selected references: SAP, with Palantir as a secondary comparator. Fit: we already have `object_graph` + linkChips — make the lineage graph the *primary* posted-voucher screen, not a secondary chip. **Cost: S** (UI wiring; graph exists). **[I]**
5. **Teams/Purview audit-event granularity on the approval.** → selected reference: Teams. Fit: reuse the current audit-event vocabulary (created/approved/reassigned + timestamp+actor) on the `audit_trail` chip; the partial/DARK hash-chain seam is not a production trust claim. **Cost: S.** **[I]**
6. **Slack Workflow-Builder form ergonomics for the 전자결재 request.** → selected reference: Slack. Fit: form + dynamic approver + SLA is the UX our approval-linked voucher intake should copy. **Cost: S–M.** **[I]**
7. **n8n's per-run egress pattern for external-GL sync.** → selected reference: n8n. Fit: only if a customer keeps QBO/Xero/NetSuite as book-of-record — a voucher→external-GL sync job. **Cost: M**, and **YAGNI until a customer needs it** (we are the GL). **[I]**

---

## Korean B2B fit notes (where global vendors mismatch)

- **전자결재 is a first-class edge in this design.** The cited SAP surface models JE approval as workflow configuration; the sampled Slack/Teams/Asana surfaces place approval in an app/workflow layer. Our `voucher_source → approval` link makes the 전자결재 chain a first-class voucher edge; this is a local design choice, not a superiority claim. Steal the *ergonomics* (Slack forms, Teams audit) but keep our native link. **[I]**
- **Group-company (법인) scoping:** SAP company-code + Rippling multi-entity both handle it; the collaboration tools (Slack/Teams/Asana) have no row-level financial entity scope. Our RLS `app.current_org` already wins here — don't regress to app-level ACL.
- **근로기준법 / payroll→GL:** Rippling's payroll-to-GL auto-sync assumes US payroll semantics; our `voucher_source → payroll_run` must carry Korean payroll (통상임금, 4대보험) coding — steal the *auto-materialize* mechanism, not the CoA mapping. **[I]**
- **Period close culture:** Korean 결산 expects tamper-evident, auditor-facing history. The repository has finance invariants, a separate period-lock substrate, and partial/DARK audit-chain code, but their finance-GL integration and production trust posture are not proved.

---

## Sources

- SAP Universal Journal / ACDOCA: https://blog.sap-press.com/what-is-saps-universal-journal · https://help.sap.com/docs/SAP_S4HANA_ON-PREMISE/651d8af3ea974ad1a4d74449122c620e/523b8a55559ad007e10000000a44538d.html
- SAP OB52 / posting-period control: https://learning.sap.com/courses/customizing-core-settings-in-financial-accounting-in-sap-s4hana/managing-posting-periods · https://sapsharks.com/ob52-open-and-close-fi-posting-periods/
- Rippling spend / bill pay / GL sync / close: https://www.rippling.com/blog/introducing-rippling-spend-management · https://www.rippling.com/blog/introducing-rippling-bill-pay · https://www.rippling.com/blog/month-end-close-checklist
- Palantir Ontology / Actions / writeback / permissions: https://www.palantir.com/docs/foundry/action-types/overview · https://www.palantir.com/docs/foundry/slate/applications-writeback · https://www.palantir.com/docs/foundry/ontology/overview
- n8n accounting nodes: https://n8n.io/integrations/xero/ · https://n8n.io/integrations/categories/finance-and-accounting/
- Slack Workflow Builder / approvals: https://slack.com/blog/news/conditional-branching-workflow-builder
- Teams Approvals + Purview audit: https://learn.microsoft.com/en-us/microsoftteams/approval-admin
- Asana Rules / Approvals / Bundles: https://asana.com/features/workflow-automation/rules · https://help.asana.com/s/article/approvals

**Our-console evidence:** `moduleScreens.ts` mounts list/detail/create/post/reverse endpoints and exposes an unblocked primary action; `financeModel.ts` status-gates post/reverse actions; `backend/app/src/lib.rs` mounts the finance router. `typeRegistry.ts` is a Source-present frontend `finance_voucher` type descriptor with a displayed balance analytic, not seeded generic-ontology registration. Finance tests, period-lock source/tests, and migrations `0107`/`0160` provide bounded UI, substrate, FORCE-RLS, and database evidence.

---

## Cross-cutting lens findings (5 independent review lenses)

- **Task-flow:** source now contains a usable draft→submit→approve→post→reverse path through the real adapter and mounted REST. Runtime/browser proof is absent, and bulk approval, receipt glance, exception handling, and period-close integration remain. **Steal:** 1-click approve-with-image glance card (Concur) and bulk/batch approve (NetSuite). Cost **M–L**. **[I]**
- **IA / layout:** descriptor-driven presentation from frontend `finance_voucher.propSchema`, with a rich link-chip graph, and source-wired for list/detail/create/post/reverse, with status-gated post/reverse actions and an unblocked primary action. This is not proof of seeded generic ontology backing or automatic generic-ontology projection. **GAPS:** anchored multi-section object pages, richer reporting, period-close integration, and runtime/browser/production proof. **[I]**
- **Data-model:** the source now implements voucher headers/lines, lifecycle, balance gate, posted immutability, append-only lines, reversal linkage, and FORCE RLS. SAP remains ahead on chart-of-accounts depth, document types, multi-ledger valuation, close, and battle-tested operation. Audit-chain source does not justify a current superiority claim. **Steal:** extension-ledger parallel valuation [M] and number-range/document-type registry [S]. **[I]**
- **Governance:** **Partial** — Dr=Cr is enforced, but finance-specific SoD rulesets, tolerance blocks, amount-threshold routing, integrated period close, and 3-way match remain incomplete. **Steal:** amount-threshold routing [M], finance SoD seed [M], and 3-way-match reconciliation [L]. **[I]**
- **Automation / extensibility:** finance automation should be **document-triggered** (on-post → downstream), never free-form. **Steal:** document-posting trigger with tolerance gate (SAP 3-way match: GR/IR clearing nets within tolerance → auto-release/block) [L]; balanced-document invariant as a submission-criterion (predicate engine can express Σdr=Σcr) [M]; 부품부족→PO reorder monitor [M]. **[I]**

**Adjudication:** the earlier "GL unbuilt" baseline is superseded by the exact target-base source. The finance-GL slice is source-wired and enforces its core voucher invariants. The generic period-lock tests remain valid evidence for that substrate, but finance-GL integration and deployed operation are still unverified.
