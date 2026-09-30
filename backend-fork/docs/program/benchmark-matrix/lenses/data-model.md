# DATA-MODEL / OBJECT-SEMANTICS lens — 14 modules

> **Benchmark evidence metadata**
> - Observation/revalidation date: 2026-07-19.
> - Sampled products/surfaces: Oyatie 14-module ontology semantics; Palantir Ontology/Workshop; ServiceNow tables; SAP MDG/S/4HANA; Workday objects; Rippling Employee Graph; Asana custom/reference fields; Slack Lists/Canvases; n8n data pinning; other explicitly named supporting vendors.
> - Evidence modality: Fixed-target repository source plus live-checked public official documentation/product pages and explicitly labeled public secondary pages; hands-on product tenants, screenshot capture, deployment, activation, and production validation were not performed.
> - Scope/claim ceiling: Only the named pages, surfaces, and fixed-target source are in scope; no whole-product, current-production, provider-parity, universal-superiority, legal, tax, labor, deployment, activation, or production conclusion.
> - Legend: [V] = bounded external observation with a direct URL or same-document source-list entry; [E]/[code] = fixed-target repository observation; [I] = recommendation or inference. Every steal/adopt item is [I].

**Question per module:** how does each vendor model the underlying objects —
*typed? linked? versioned? effective-dated?* — vs our ontology engine, and where
what source-backed strengths and gaps the object model has.

**Our engine, evidence-based** (read from `web/src/console/**` +
`docs/program/{console-program-ledger.md,ontology-coverage-matrix.md}`):
- **Typed**: `ont_object_types` → `ont_property_defs` (10 FieldKinds: text/number/money/date/datetime/boolean/choice/user/object_ref/attachment; arch says ~35) with `required`, per-property policy flag. `objectcard/types.ts`, `ontology/types.ts`.
- **Linked**: `ont_link_types` as a **4-tuple `[rel, to, cardinality, reverse-name]`** (one_one/one_many/many_many), instance edges in `ont_links`, graph traversal REST (depth-bounded). `objectcard/types.ts:31`.
- **Versioned**: append-only `ont_instance_revisions`, **content-addressed schema versions** (published = immutable, v+1, superseded/retired FSM), **row_hash fixity chain** (L20 canonicalizer, tamper-detected). `ontology/types.ts:7`, coverage-matrix OB-/OT- rows.
- **Effective-dated**: `valid_from/valid_to` + **`get_as_of` reconstruction** + `history` (proven for `ont_instances`). D2: audit-derived history for projected types, full bi-temporal only for instance-backed.
- **Two backing modes**: *projected* (domain tables surfaced as typed projections — WO/employee/equipment) vs *instance-backed* (engine-owned store).
- **Governance primitives in the model**: draft→active→locked→archived→disposed instance FSM; selected transitions/overrides have audit and four-eyes evidence. Cedar object/property policy plus residual→SQL remains target/shadow, not universal live enforcement.

**Honest reality check (weakness that spans every module):** target-base tenant seeding publishes
**27 published tenant types (9 governed config + 3 C-chain + 15 projected domain)**.
The 15 domain types expose **projected/read-oriented semantics; writes remain domain-owned**.
Their ontology actions are generally absent; only registered projected dispatches execute,
and unregistered targets fail closed as `NotWiredYet`. The FE `ONT_TYPES` mirror remains a
hand-authored wire-pending constant. Registration breadth is therefore real, but product
depth is uneven: generic consumers, links, lifecycle depth, and action dispatch remain
incomplete across modules. Every stronger claim below must stay inside that ceiling. **[I]**

Vendor claims labeled **[V]** (verified, URL) or **[I]** (inferred). N/A = vendor
doesn't play here.

---

## 1. overview

A landing/portal surface, not a data domain of its own — it *projects* other
objects. Data-model relevance = whether the portal is a typed object or hardcoded.

| Vendor | Typed | Linked | Versioned | Eff-dated | Note |
|---|---|---|---|---|---|
| Palantir Workshop home | ✅ objects | ✅ | via ontology | N/A | Home = a Workshop module bound to ontology object sets [I] |
| ServiceNow homepages | ◐ tables | ✅ ref | ◐ | ✗ | portal widgets over tables [I] |
| **Ours** | ✅ (once `console_view` populated) | ✅ | ✅ governed-config-as-instance | ✅ as-of | overview = a `console_view` ont instance, staged draft→approve |

**Stronger than them:** the overview/dashboard config is itself a **governed **[I]**
ontology object** (`console_view` is engine-registered TODAY — coverage-matrix),
so the landing page has draft→approve→effective + rollback + as-of. Foundry Home
is configured but not itself a first-class versioned business object with four-eyes.
**Weaker:** Foundry Home ships live object-set widgets out of the box; ours needs **[I]**
the widget→ontQuery binding finished (post-replica backlog).

**What we'd steal (ranked):** **[I]**
1. Object-set-bound home widgets (Palantir) → fits our ontQuery grammar → **M**.
2. Per-persona default home resolved from role attributes (Workday landing) → aligns with the target Cedar principal-attribute model after enrollment and promotion → **S**.

---

## 2. dashboard

| Vendor | Typed | Linked | Versioned | Eff-dated | Note |
|---|---|---|---|---|---|
| Palantir Quiver/Workshop | ✅ ontology-bound | ✅ | ✅ ontology | ◐ | metrics are functions over typed objects [V-inferred from ontology docs] |
| Tableau/Power BI/Looker | ◐ semantic layer (LookML typed) | ◐ joins | ◐ git-versioned (Looker) | ✗ | dashboards over a modeled layer, not effective-dated objects [I] |
| **Ours** | ◐ ontQuery wire-pending | ◐ drill affordances; shell wiring unproven | ✅ config | ✅ | scope×period + honest-scale; universal drill unproved |

**Presentation strength with a wiring ceiling:** source contains stat-strip **drill affordances** and honest-scale presentation. Their absolute React Router targets are registered by `AppRouter` as legacy `ConsoleShell`/`AppShell` routes, so they exit the carbon-console shell and bypass its `state.screen`/ObjectCard flow. Universal working drill-to-ObjectCard behavior is not established, and browser behavior remains unverified. They still must be routed through the console screen/object-explorer model and browser-proven. Mature BI aggregation remains ahead of this bounded evidence. **[I]**

**What we'd steal:** **[I]**
1. Looker's git-versioned semantic model as the pattern for our `console_view` diffs → we already version, borrow the **diff/merge UX** → **M**.
2. Power BI incremental-refresh windows → maps to our effective-dated folds → **M**.

---

## 3. finance

| Vendor | Typed | Linked | Versioned | Eff-dated | Note |
|---|---|---|---|---|---|
| SAP S/4HANA GL | ✅ document types + line items | ✅ (doc header↔lines, ledger) | ✅ **document principle — posted doc immutable until archive** | ✅ posting date/period | every txn = a saved document, min 2 lines D/C, never mutated [I] |
| Workday Financials | ✅ business objects | ✅ | ✅ | ✅ effective-dated | [I] |
| NetSuite | ✅ record types | ✅ | ◐ | ◐ | [I] |
| **Ours** | ✅ `finance_gl_vouchers` + append-only lines, mounted REST, DB/domain Dr=Cr gate | ◐ source-object linkage; broader document flow incomplete | ✅ posted immutability + reversal | ◐ | migration `0160`; period-close and full reporting remain incomplete |

**Still weaker than SAP, but no longer absent:** SAP's document principle is **[I]**
populated and battle-proven for GL. Our exact source now has voucher headers/lines,
mounted lifecycle REST, DB/domain balance gates, posted immutability, reversal, and
FORCE RLS. Remaining gaps include chart-of-accounts governance, period-close
integration, multi-ledger/reporting depth, runtime proof, and promoted Cedar
property-policy enforcement. The current DARK audit chain does not establish a
superiority claim. Korean context: 전표/분개 must **[I]**
map to 부가세/원천세 and 세금계산서 — SAP localizes this; we'd need the 전표 entity to
carry Korean tax typed props.

**What we'd steal:** **[I]**
1. **SAP document principle as the voucher/posting entity spec** (immutable header+lines, doc-type registry) → maps onto an instance-backed ontology type plus the current audit-event seam; trusted chain anchoring remains separate → **L** (biggest finance win). **[I]**
2. Extension-ledger concept (parallel valuation) → model as a link-typed shadow revision → **M**.
3. Number-range/document-type registry → a `finance_voucher` ont_object_type with typed doc-type choice → **S**.

---

## 4. people (HR)

| Vendor | Typed | Linked | Versioned | Eff-dated | Note |
|---|---|---|---|---|---|
| **Workday HCM** | ✅ business objects | ✅ **Worker→Position→JobProfile→SupOrg nested** | ✅ | ✅ **effective-dating is the core primitive** — correct(overwrite) vs new-dated-change | almost every Core-HCM object is effective-dated; entry-date + effective-date dual stamp [I] |
| SAP SuccessFactors | ✅ | ✅ | ✅ | ✅ MDF effective-dated | [I] |
| BambooHR | ◐ fields | ◐ | ◐ history | ◐ | lighter model [I] |
| **Ours** | ◐ `employee` is a published projected/read type; `position` is a published instance-backed C-chain type, while legacy `employees.position` remains a string | ◐ C-chain/product linkage incomplete | ◐ projected audit history + instance revisions | ◐ mixed | target-base seed + coverage-matrix |

**Weaker — Workday is the selected effective-dating reference in this sample.** Workday's **[I]**
**correct-vs-new-effective-change distinction** (retroactively overwrite history vs
append a future-dated slice) is *precisely* our draft-direct-vs-override semantics
but far more mature and pervasive. We publish `position` as an instance-backed C-chain
type and `employee` as a projected/read type, yet legacy `employees.position` remains
a string and the product link/consumer lifecycle is incomplete. **Potential edge:**
our uniform typed registry and hash-fixity substrate are public, while Cedar field
masking remains target/shadow pending promotion. A typed per-shift contract object is a product/policy control,
not a statutory conclusion; this benchmark does not establish an ontology shape as
a labor-law requirement. The applicable record, notice, and contract-term questions
remain scenario-specific for qualified Korean counsel.

**What we'd steal:** **[I]**
1. **Position/Job-Profile as first-class linked objects** (Workday) → deepen the existing published C-→Position→Posting→Employee chain with JobProfile, employee linkage, and consumer lifecycle depth → **L**.
2. Workday's **correct vs. new-effective-dated-change** UX distinction → map onto our draft-direct/override + as-of, make the two paths explicit in ObjectCard → **M**.
3. Dual entry-date/effective-date stamping (bi-temporal) → we have valid_from/to; add the entry-date axis for instance types → **M**.

---

## 5. leave

| Vendor | Typed | Linked | Versioned | Eff-dated | Note |
|---|---|---|---|---|---|
| Workday Absence | ✅ leave/time-off objects | ✅ →Worker | ✅ | ✅ effective-dated accruals | [I from Workday model] |
| Korean 근태 (Shiftee/flex/Hanbiro) | ◐ | ◐ | ◐ | ◐ | 연차 rules typed to 근로기준법 [I] |
| **Ours** | ◐ `leave_request` is a published projected/read type (domain mig `0122`; promotions `0123`) | ◐ →employee | ✅ domain FSM DRAFT→SUBMITTED→APPROVED/REJECTED, audited, decider≠requester CHECK | ✗ effective-dated balance | coverage-matrix leave rows: shared card + promotion rounds |

**Mixed.** Our leave has a **real audited FSM with SoD (decider≠requester CHECK)**
and 촉진 promotion rounds — a Korean-specific domain seam Workday Absence does not
ship natively. `leave_request` is a published projected/read type, but registration
does not create an effective-dated accrual balance or ontology actions. Workday remains
stronger on effective-dated derived balances. `labor_refusal` is one of the 9 published **[I]**
governed-config types; the backend notice/receipt flow still does not prove statutory
timing or sequence. Product depth, consumer wiring, and scenario-specific legal review
remain separate from registration.

**What we'd steal:** **[I]**
1. Workday **effective-dated accrual/balance object** (not just requests) → new instance-backed `leave_balance` type folding grants−takes as-of → **M**.
2. Carryover/expiry as effective-dated slices → our valid_from/to fits → **S**.

---

## 6. support

| Vendor | Typed | Linked | Versioned | Eff-dated | Note |
|---|---|---|---|---|---|
| **Zendesk** | ✅ **custom objects** (≤100 fields) + native ticket/user/org | ✅ **lookup relationship fields** (5-10/obj), junction objects for M:N | ◐ (audit events, no object versioning) | ✗ | you extend the data model with typed custom objects + typed lookups [I] |
| ServiceNow ITSM | ✅ tables, extend base | ✅ reference fields | ◐ | ✗ | [I] |
| Jira SM | ◐ issue types + fields | ◐ | ◐ | ✗ | [I] |
| **Ours** | ◐ `support_ticket` is a published projected/read type; `support_slo_setting` is a governed instance | ◐ | ✅ TicketStatus domain FSM audited; SLO setting = governed instance w/ pendingRev staging | ◐ | coverage-matrix support/SLO rows; §4-26 SLO≠SLA |

**Close, different axes.** Zendesk's **custom-objects + typed lookup relationships**
provide broader no-code authoring depth in the cited surface. Our `support_ticket` is published as a
projected/read type, but domain writes remain authoritative and ontology actions are
generally absent. **Source-bounded difference:** `support_slo_setting` is a governed
ontology instance with draft→approve staging + as-of (§4-26), and the ticket domain
FSM is audited. Zendesk's lookup cap (5-10) vs our typed 4-tuple link substrate is a
modeling-ceiling difference, not proof of equal consumer maturity.

**What we'd steal:** **[I]**
1. **Zendesk custom-objects + typed lookup UX** as the reference for no-code ticket-adjacent types (our add-a-type still has 6 manual steps — coverage-matrix) → **M**. **[I]**
2. Junction-object pattern for M:N → we already have many_many links; borrow their **relationship-field authoring UI** → **S**.

---

## 7. evidence

| Vendor | Typed | Linked | Versioned | Eff-dated | Note |
|---|---|---|---|---|---|
| WORM/records (NetApp SnapLock, CTERA, DAM 17a-4) | ◐ file+metadata | ✗ | ✅ **every change = new version, immutable, retention-expiry** | ◐ retention dates | hash/digital-signature fingerprint per record; chain-of-custody audit trail [I] |
| Veritas/OpenText RM | ✅ record classes | ◐ | ✅ | ✅ retention schedules | [I] |
| **Ours** | ◐ `evidence` is a published projected/read type over `docs_evidence_objects`; writes remain domain-owned | ◐ custody events | ◐ **14-stage wire custody FSM plus 15-state frontend presentation union (synthesized ACCESSED), copy/fixity metadata, nullable TSA metadata, holds, exports; object-lock deployment unproved** | ◐ audit-derived | shared card + source-wired `/verify`; no non-null exercised TSA token or durable-WORM proof |

**Richer source-level object semantics than a bare storage volume, but not proved as commodity WORM.** Our evidence has a
**14-stage wire custody FSM + 15-state frontend presentation union (including synthesized ACCESSED) + source-wired fixity re-verify + legal-hold four-eyes**. Object-lock deployment and trusted audit anchoring remain unproved. **Weaker vs Veritas/OpenText:** **[I]**
they ship **effective-dated retention schedules + record-class taxonomies + disposition
workflows**; ours has holds but **no retention-schedule object** and TSA anchoring is
nullable/wire-pending. Custody without a non-null exercised token is not RFC-3161 proof; TSA completion
is a post-replica backlog item. Korean context: 전자문서 &
공인전자문서센터 / 수령확인 문서 — our default catalog lists 법정 수령확인 문서; global RM doesn't.

**What we'd steal:** **[I]**
1. **Effective-dated retention-schedule + disposition object** (Veritas) → instance-backed `retention_schedule` type driving the disposed lifecycle state → **M**.
2. 17a-4-style **immutable-fingerprint attestation** (already have SHA-256; add RFC-3161 TSA) → **M** (backlog).
3. Record-class taxonomy as ont types → our registry fits → **S**.

---

## 8. object-platform (the head-to-head)

| Vendor | Typed | Linked | Versioned | Eff-dated | Note |
|---|---|---|---|---|---|
| **Palantir Foundry Ontology** | ✅ object types + properties (time-series, geospatial base types) | ✅ **link types 1:1/1:many/many:many** | ✅ **Global Branching — ontology proposal = PR; changelog; status active/experimental/deprecated** | ◐ time-series props, not object-level bitemporal | THE comparator; action types = declarative edit-sets w/ side-effects [I] |
| Microsoft Dataverse | ✅ typed tables/columns | ✅ relationships | ◐ | ✗ | [I] |
| Salesforce | ✅ custom objects + typed fields | ✅ lookup/master-detail | ◐ **field history (20 fields, 18 months)** | ✗ | history is field-scoped + time-capped, not object-versioning [I] |
| **Ours** | ✅ ont_object_types + 10 FieldKinds + property-policy | ✅ **4-tuple link (adds reverse-name)** | ✅ content-addressed immutable versions + **hash-fixity chain** | ✅ **as-of reconstruction + bi-temporal (instance types)** | the engine |

**Source-bounded difference from the cited Foundry surface:**
1. **Fixity/tamper-evidence in the object model.** Foundry versions ontology
   resources (proposals, changelog) but does **not** hash-chain object *instance*
   revisions for tamper-evidence; ours does (L20 canonicalizer, verify_chain).
   [V for Foundry branching; our fixity = coverage-matrix OB-].
2. **Governance primitives are native to selected instance writes.** Our instance FSM + override(reason+
   four-eyes) is evidenced on the object-model path. Cedar property-policy field-masking and
   **partial-eval→SQL residual deny-by-omission** remain target/shadow until per-action enrollment,
   evidence, and promotion; they are not universal current write enforcement. Foundry Actions have
   side-effects + Cedar-like permissions [V our authoring/residual substrate; Foundry = separate governance layer].
3. **True object-level as-of / bitemporal** for instance types. Foundry's point-in-time story centers on
   time-**series properties** + edit-history writeback transactions; a *uniform object-level as-of/bitemporal
   reconstruction* is our engine's native primitive [I] (avoid asserting Foundry lacks it as verified fact —
   it exposes edit history via the Ontology changelog).
4. **Effective-dated no-code config-as-object** (support_slo/console_view governed
   instances) — Foundry config isn't itself a versioned business object with four-eyes.

**Where we're WEAKER than Foundry (honest):** **[I]**
1. **Branching/proposal workflow.** Foundry has **Global Branching — an ontology
   proposal is a PR with reviewers, changelog, isolated test-before-merge** [I]. Our
   schema lifecycle is draft→review→publish (linear, single-track); we have no
   branch/merge/isolated-preview of schema changes. This is their clearest edge.
2. **Breadth + maturity of populated types + Functions/Workshop consumer stack.**
   Foundry ships a rich compute layer (Functions on objects) over a fully populated
   ontology; ours seeds 27 published tenant types (9 governed config + 3 C-chain +
   15 projected domains). Depth remains incomplete: projected writes stay
   domain-owned, `registry.update_equipment` is the one registered projected
   dispatch, and unregistered targets fail closed as `NotWiredYet`.
3. **Base-type richness** — Foundry has geospatial + time-series base property types;
   we have 10 FieldKinds (arch says 35 planned).

**What we'd steal (highest-value module):** **[I]**
1. **Foundry ontology branching / proposal-as-PR** → extend our draft→publish to a branch/merge model w/ isolated preview → **L** (our biggest object-platform gap). **[I]**
2. **Functions-on-objects** (typed compute bound to object types) → our `ont_analytics` is the seed; grow to invokable typed functions → **L**.
3. **Geospatial + time-series base property types** (Foundry) → add FieldKinds → **M** (Korea terrain layer already wanted for dashboard).
4. Salesforce master-detail cascade semantics for owned children → link cardinality already there; add cascade-lifecycle → **S**.

---

## 9. policy

| Vendor | Typed | Linked | Versioned | Eff-dated | Note |
|---|---|---|---|---|---|
| **AWS Cedar / Verified Permissions** | ✅ **schema: entityTypes + attributes + actions + commonTypes** | ✅ entity refs (photographer→User) | ◐ policy store versioning | ✗ | principals/resources are typed entities w/ attrs; schema validates policies [I] |
| OPA/Rego | ◐ untyped JSON data | ◐ | ◐ bundle versions | ✗ | schemaless data documents [V-inferred] |
| **Ours** | ✅ Cedar catalog + **blocks→normalized_row→Cedar text**, object/property policies typed to ont types | ✅ policies reference ont object/link types | ✅ draft/publish staging FSM + pendingRev per-policy | ◐ | authoring/shadow substrate; residual→SQL is not universal live enforcement |

**Stronger authoring target than raw Cedar/OPA.** The Cedar substrate uses the typed-entity model and adds: **[I]**
(a) **no-code P→R→A→Effect canvas → normalized_row → generated Cedar text** with a server-backed
simulator (deny-by-omission), (b) **policies are governed ontology objects** with
draft→approve staging + audit, (c) target/shadow **partial-eval → residual → SQL WHERE** lowering for
list filtering. ADR-0021 keeps current routes on legacy server authorization plus evidenced RLS until
separate promotion. Raw Cedar is a decision library; OPA is *schemaless* (weaker typing **[I]**
than both). **Weaker:** Cedar's **schema-validation of policies against entity types** **[I]**
is a mature safety net; our residual **fail-closes to DENY on any untranslatable term**
(safe but coarser than full validation). AVP also has managed policy-store versioning
we approximate with staging.

**What we'd steal:** **[I]**
1. **Cedar schema-based policy validation** (catch type errors at author time, not runtime-deny) → bind our block editor to the ont-type schema for pre-submit validation → **M**.
2. `commonTypes` reuse (Cedar) → shared predicate fragments in our canvas → **S**.

---

## 10. automate

| Vendor | Typed | Linked | Versioned | Eff-dated | Note |
|---|---|---|---|---|---|
| **n8n** | ◐ **items = array of `{json, binary}` — largely untyped/schema-inferred** | ✗ (data flows node→node, no object links) | ◐ workflow JSON versioned; node-type versions | ✗ | data is JSON items, schema inferred per-run, not a typed object model [I] |
| Temporal | ◐ typed activities/signals | ✗ | ✅ workflow versioning/replay | ✅ event-sourced history | durable event-sourced execution [I] |
| Zapier/Workato | ◐ mapped fields | ✗ | ◐ | ✗ | [I] |
| **Ours** | ✅ **effect = ontology action** (typed dispatch: projected_usecase / instance_revision) | ✅ acts on typed ont objects | ✅ workflow def publish FSM + four-eyes, run/node FSM audited | ◐ | coverage-matrix workflow row; Automate = ontology action |

**Stronger than n8n/Zapier — clearest data-model win in automation.** n8n's data is **[I]**
**untyped JSON items with per-run inferred schema**. n8n does provide workflow-owner/RBAC controls and
credential scoping for workflows and resources, but it lacks native ontology identity, typed object links,
and equivalent object-level governance. Our automation **effect IS a typed ontology action** dispatched through the
same writeback shape humans use (projected_usecase/instance_revision). Where routed, automation
and human edits can share audit/fixity seams; current execution remains under legacy server guards,
and the same Cedar gate is target/shadow pending promotion. n8n can't reference "the WO object" as a typed linked entity; it passes a
JSON blob. **Weaker vs Temporal:** Temporal's **event-sourced durable execution with **[I]**
replay + workflow versioning** is more mature than our run/node FSM for long-running,
retryable orchestration; and n8n's **connector breadth** dwarfs ours.

**What we'd steal:** **[I]**
1. **Temporal-style event-sourced durable execution + replay** → our run-log is close; add deterministic replay over the append log we already have → **L**.
2. n8n **connector/integration breadth** (typed triggers on external systems) → **M** (ongoing).
3. n8n Schema-view auto-inference for mapping external JSON onto typed ont props → aids DX- ingest mapping → **M**.

---

## 11. comms

| Vendor | Typed | Linked | Versioned | Eff-dated | Note |
|---|---|---|---|---|---|
| Slack | ◐ messages/channels/users objects (Web API), typed events | ◐ thread_ts links | ✗ (edit history thin) | ✗ | conversation objects, not business-object links [I] |
| MS Teams / Graph | ◐ chatMessage/channel typed via Graph | ◐ | ◐ | ✗ | [I] |
| Gmail | ◐ message/thread/label | ◐ threading | ✗ | ✗ | [I] |
| **Ours** | ◐ `messenger_thread` and `mail` are published projected/read types; messenger has no thread FSM or ontology actions | ◐ **object-card unfurl / #code drag-drop into messages** | ◐ acks/presence, audited | ✗ generic as-of | coverage-matrix messenger/mail rows |

**Source-backed object-linkage distinction; messaging-maturity gap.** The current design can make
messages **carry typed object references** (#WO-2643 drag-drop, object-card unfurl,
policy-projected drop target) — a message links to a business object, which Slack/
Teams/Gmail do only via unfurled URLs rather than first-class ontology edges.
`messenger_thread` and `mail` are published projected/read types, but threads still
lack lifecycle actions, generic as-of depth, and mature real-time/search/retention.
Korean context: our audit-in-app-chat (no E2EE, auditable — per project decision)
is a local auditability choice; the cited Slack surface has a different messaging/security model.

**What we'd steal:** **[I]**
1. Slack **typed event/message object model + retention-as-config** → deepen the published `messenger_thread` type with lifecycle actions and as-of semantics → **M**.
2. Message↔object link as a **first-class typed link** (not just unfurl) → we're 80% there via objDrag; make it an `ont_link` edge → **S**.

---

## 12. appr (전자결재 / approval)

| Vendor | Typed | Linked | Versioned | Eff-dated | Note |
|---|---|---|---|---|---|
| Korean 전자결재 (더존/Naver Works/Hanbiro/Flow) | ✅ **문서양식(typed forms) + 결재선(fixed/dynamic approval line)** | ✅ form↔결재선↔drafter | ◐ document history | ✗ | admin sets whether drafter picks 결재선 or fixed; typed forms + line mgmt [I] |
| SAP Workflow | ✅ | ✅ | ◐ | ✗ | [I] |
| **Ours** | ◐ `approval` is a published projected/read type backed by `gov_approval_requests` (mig `0158`); ontology actions generally absent | ✅ →governed domain object | ✅ **governance config-driven domain FSM + gov_lifecycle_transitions, audited, approver≠requester CHECK** | ◐ audit-derived | coverage-matrix approval row; bespoke ApprovalCompose |

**Mixed vs Korean incumbents — the local-fit test.** Korean 전자결재 ships **mature
typed 문서양식 + 결재선 (fixed vs drafter-selected, delegation, 전결/대결)** that our
approval does not fully model yet. We have **AP- with four-eyes SoD (approver≠requester
CHECK) + config-driven domain lifecycle + audit**, and `approval` is a published
projected/read type. That registration supplies schema/read projection, not an
instance-backed as-of/fixity guarantee, a second writer, or complete 결재선 semantics.
Ordered multi-step lines, 전결 rules, 대결/위임, and 병렬/순차 approval remain the
genuine local product-depth gap; Cedar enrollment/promotion remains a separate gate.

**What we'd steal (high local priority):** **[I]**
1. **Typed 결재선 model — ordered multi-step line, 전결/대결/위임, 병렬 vs 순차** (더존/Naver Works) → model 결재선 as a typed ordered link-set on the approval object → **L** (KR must-have).
2. **문서양식 typed forms** (approval templates) → ont_object_type per form (ledger already has "console-change AP- template UI") → **M**.
3. 부재중 위임(delegation) as effective-dated authority grant → our Cedar + valid_from/to → **M**.

---

## 13. field (field service)

| Vendor | Typed | Linked | Versioned | Eff-dated | Note |
|---|---|---|---|---|---|
| ServiceNow FSM | ✅ work-order/task tables + CMDB CI links | ✅ reference fields to CI/asset | ◐ | ✗ | WO references asset via reference field to cmdb_ci [I] |
| Salesforce Field Service | ✅ WorkOrder/ServiceAppointment/Asset objects | ✅ lookups | ◐ field history | ✗ | [I] |
| SAP FSM | ✅ | ✅ | ◐ | ◐ | [I] |
| **Ours** | ◐ `work_order` and `equipment` are published projected/read types; WO retains its **16-state bespoke domain FSM** | ◐ typed projected links plus legacy display link-chips | ✅ domain FSM audited + RLS; no engine-owned revision writes | ✗ generic as-of | coverage-matrix WO- row; legacy WorkOrderDetailPage, no ObjectCard yet |

**Source-backed lifecycle depth; consumer/action-depth gap.** Our WO- has a **16-state lifecycle
FSM in this repository (audited and RLS-tested)**. The sampled ServiceNow surface is a product comparator, not evidence for a universal lifecycle ranking. WO/equipment/customer/site are published projected/read types, **[I]**
but the legacy WO page does not consume the generic ObjectCard and ontology actions are
generally absent. ServiceNow/Salesforce still provide a more mature **WO↔Asset↔CI**
reference graph and user-facing consumer stack. Korean context: 현장 coverage/대근/교대
domain semantics are relevant local depth, but registration alone does not prove them.

**What we'd steal:** **[I]**
1. **WO↔Asset↔CI first-class typed reference depth** (ServiceNow reference fields) → deepen the published WO/equipment projections and link types, then wire ObjectCard consumers → **M**.
2. Salesforce ServiceAppointment as a distinct typed object (schedule≠work) → new instance type → **M**.
3. Open WO in the 3-layer ObjectCard instead of the legacy page → **S**.

---

## 14. compliance

| Vendor | Typed | Linked | Versioned | Eff-dated | Note |
|---|---|---|---|---|---|
| ServiceNow GRC / SAP GRC / OneTrust | ✅ control/risk/obligation record classes | ✅ control↔risk↔policy links | ◐ | ◐ assessment periods | typed GRC objects + control-test workflows [I] |
| **Ours** | ◐ `compliance_obligation`/`compliance_regulation`/`compliance_framework` are published projected/read types; each domain table has a bespoke audited FSM | ◐ | ◐ **regulation validity window valid_from/valid_to (but no as-of read fn)** | ◐ | coverage-matrix CP-/RG-/FW- rows; **no web UI (0 refs)** |

**Weaker in surfacing, competitive in domain modeling.** We already have **typed status FSMs for **[I]**
obligation/regulation/framework + a regulation validity window (valid_from/valid_to)**, and the
three types are published as projected/read-oriented ontology types. They still have **no as-of
read fn, no ontology actions, and no web UI** (coverage-matrix: 0 refs). GRC vendors ship
**mature control↔risk↔obligation link graphs + assessment/attestation cycles + evidence
attachment**. **Potential once deepened:** an effective-dated regulation read could reconstruct
"which reg text applied on date T" without creating a second writer. Korean context: RG-/규제
PII/multi-jurisdiction + PIPA-oriented policy objects target KR/multi-jurisdiction scenarios,
but applicable legal duties remain scenario-specific.

**What we'd steal:** **[I]**
1. **Control↔Risk↔Obligation typed link graph + attestation cycle** (ServiceNow/SAP GRC) → deepen the 3 published projected types with links and build the missing UI → **M** (domain model half exists).
2. Effective-dated regulation as-of read (finish the fn on the existing validity window) → our engine's as-of → **S** (cheap, distinctive).
3. Assessment/evidence-request cycle wired to our EV- evidence objects → **M**.

---

## Cross-module synthesis

- **Our architecture and registration breadth are real:** tenant seeding publishes 27
  types, while product depth remains incomplete. The 15 projected domain types are
  read-oriented views with domain-owned writes and generally absent ontology actions;
  C-chain and governed-config depth also varies by consumer. The steal-list is therefore **[I]**
  dominated by deeper links, actions, consumers, and lifecycle semantics, not registration.
- **Two genuine architectural gaps vs leaders:** (1) **Foundry ontology branching /
  proposal-as-PR** for schema changes; (2) **Temporal-style event-sourced durable
  execution/replay** for automate.
- **Two Korean local-fit must-haves** the global vendors won't give us: **전자결재 결재선
  semantics** (appr) and **finance 전표/분개 with KR tax typed props** (SAP does GL but
  localizes 세금계산서 heavily) — both are typed-object modeling tasks on our engine.
