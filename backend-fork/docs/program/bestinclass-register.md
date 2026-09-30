# Best-in-Class Capability Register — W3 Mining Lane

> **Directive**: DESIGN.md §4-21 (모듈별 3문 벤치마크 리뷰 루프) + §4-25 (폐루프 페이지 리뷰).
> For **every** deployed console module ask: ① what does the best-in-class product
> (Palantir Foundry, Teams/Slack, SAP/Workday/Greenhouse/ServiceNow per domain) do here
> that we don't? ② which of those capabilities fit **our** ontology-first, Cedar-PBAC,
> deterministic no-AI grammar (§4-20) — not bolt-ons? ③ what is the smallest coherent
> increment?
>
> **Inputs**: `docs/program/benchmark-brief.md`, `docs/design/oyatie-console/DESIGN.md §4-21`,
> deployed module list (`web/src/console/shell/nav.ts` NAV_GROUPS + `modules/moduleScreens.ts` MOD_SCREENS).
> **This is a research/mining artifact — no implementation.** The next deep slice is *selected from
> this register*, not invented ad hoc.
>
> Compiled 2026-07-10. Sources are live-cited inline. Where the brief already carries a
> citation, it is referenced as `[brief §N]` to avoid re-fetching.

---

## How to read the scores

Each capability is scored on three axes, 1–5:

- **Impact** — how much it moves enterprise-grade operability / defensibility / daily-driver quality. 5 = load-bearing.
- **Fit** — how native it is to our ontology-first, Cedar-PBAC, no-AI, config-as-data grammar (§4-20/§4-22).
  5 = it *is* the grammar (a single-engine primitive every consumer inherits); 1 = a bolt-on that fights the model.
- **Cost** — build cost incl. backend + governance wiring. 5 = large multi-lane; 1 = a slice.

**Priority = Impact × Fit ÷ Cost** (higher = do sooner). Fit is deliberately weighted equally with impact:
a high-impact bolt-on that violates single-engine (§4-20) is a trap — it looks like progress and becomes
the thing someone rewrites at 3am. Ties broken toward lower Cost (ship the increment, learn, iterate).

**Ranked master table is §0. Per-module derivations (the 3-question loop) follow in §1–§10.**

---

## §0. Ranked master register (priority-ordered)

### Tier A — cross-cutting single-engine primitives (do first: one build, every module inherits)

| # | Capability | Module(s) served | Benchmark | Impact | Fit | Cost | **Priority** |
|---|-----------|------------------|-----------|:---:|:---:|:---:|:---:|
| A1 | **Partial-eval residual → SQL WHERE** for row-level list filtering (one authorizer, every list endpoint; no per-row loop) | all list surfaces (hr, dispatch, appr, finance, audit, mail…) | Cedar `is_authorized_partial` [brief §2f] | 5 | 5 | 3 | **8.3** |
| A2 | **Saved views = named filter/sort/group AST** decoupled from data (Collaborative/Personal/Locked ownership) | every table module | Airtable/Notion views [brief §3d] | 5 | 5 | 3 | **8.3** |
| A3 | **Actions-as-writeback**: every mutation is a declarative named Action → append-only writeback/event row, never a direct UPDATE; human + automation hit the same verb | every mutating module | Foundry Actions / Temporal Commands [brief §0,§1a,§4] | 5 | 5 | 4 | **6.3** |
| A4 | **Effective-dating / time-slice** on every business entity (validity interval, retro-edit + future-date for free) | hr, payroll, policy, positions, contracts | Workday effective-dating [brief §4] | 5 | 5 | 4 | **6.3** |
| A5 | **Automate: Condition(object-set predicate) → Effect(Action/notify/webhook)** monitors defined over ontology object sets, not raw tables | automation (workflow/scheduled), cross-cutting | Foundry Automate [brief §1e] | 5 | 5 | 4 | **6.3** |
| A6 | **Field-rule records = ServiceNow tri-state** (Mandatory/Visible/Read-only ∈ {T,F,leave-alone}, ordered, reverse-if-false), enforced **both** client and server over one condition model | every form (appr, hr, finance, config) | ServiceNow UI Policy + Data Policy [brief §3f] | 4 | 5 | 3 | **6.7** |
| A7 | **Config-as-governed-object**: per-user invisible drafts → review → **immutable content-addressed released version** → env-agnostic artifact + env-bound creds → free rollback; config carries its own effective-principal | configconsole, policy, workflow, views, screens | ToolJet/Windmill/Retool [brief §3e,§3h] | 4 | 5 | 4 | **5.0** |
| A8 | **Field schema = discriminated union** `{id,name,type,config}`, IDed option sub-entities (rename-safe), reader degrades on unknown `type` (new field types w/o migration) | ontology engine, every typed surface | Airtable/Notion field model [brief §3c] | 4 | 5 | 4 | **5.0** |

### Tier B — high-leverage per-module capabilities

| # | Capability | Module | Benchmark | Impact | Fit | Cost | **Priority** |
|---|-----------|--------|-----------|:---:|:---:|:---:|:---:|
| B1 | **Object Explorer search-around** = link traversal as a first-class UI verb; linked-object filtering; charts aggregate on main **or linked** types; shareable deep-links | objectExplorer | Foundry Object Explorer [brief §1c] | 4 | 5 | 3 | **6.7** |
| B2 | **Saved Explorations / saved object sets** revisitable (latest results), importable across surfaces (explorer ↔ analysis ↔ automation input) | objectExplorer, forecast, dashboard | Foundry Object Explorer save-explorations¹ | 4 | 5 | 3 | **6.7** |
| B3 | **Interview Kit + structured Scorecard** (predefined attributes, fixed rubric, overall rec) — Application-scoped, not Candidate-scoped; source-tracking on the Application | recruit, evaluation | Greenhouse structured hiring [brief §4] | 4 | 5 | 3 | **6.7** |
| B4 | **3-way match on a clearing/suspense account** (PO↔GR↔Invoice net to zero within tolerance, else block) + **balanced-document** invariant (Σdr=Σcr) on every financial mutation | finance, purchase | SAP MM/FI [brief §4] | 5 | 4 | 4 | **5.0** |
| B5 | **Control → Test → Evidence** with cross-framework control mapping (one evidence item satisfies overlapping requirements); continuous re-run on cron + diff | compliance | Vanta/Drata [brief §4] | 4 | 4 | 3 | **5.3** |
| B6 | **Fixity-hashed WORM AIP** (SHA-256 per object + PDI/provenance, object-lock retention, certification record) — ISO 15489 integrity ∩ OAIS fixity ∩ FRE 902(14) self-authentication | audit, evidence, docs | ISO 15489/OAIS/FRE 902 [brief §4] | 4 | 4 | 4 | **4.0** |
| B7 | **Dispatcher workbench**: unassigned/assigned split, technician status + skills/parts/proximity/availability match, one-click assign, rebalance-on-change; assignment is an audited Action | dispatch, maintenance | ServiceNow FSM² | 4 | 4 | 4 | **4.0** |
| B8 | **PM work-order status profile** with cost-posting gates `CRTD→REL(bookable)→TECO(no cost)→CLSD`, forward-only | maintenance, dispatch | SAP PM [brief §4] | 4 | 5 | 3 | **6.7** |
| B9 | **Thread model = parent `ts` + nullable `thread_ts` self-FK** (flat one-level); link unfurl = metadata-only event → resolve → post-back (never trust event to carry body) | messenger, board | Slack [brief §4] | 4 | 4 | 3 | **5.3** |
| B10 | **Header-chain threading** (In-Reply-To + References first, normalized-subject only as tiebreak) + **labels = many-to-many tags** (not folders); thread labels = union of members | mail | Gmail [brief §4] | 3 | 4 | 3 | **4.0** |
| B11 | **Business-Process framework**: every HR action = event instance through a configurable BP definition (ordered steps: approval/to-do/checklist/integration/notify, each gated by condition + routing) ending in a **mandatory commit step** | appr, hr, payroll, leave | Workday BP [brief §4] | 5 | 5 | 4 | **6.3** |
| B12 | **Continuous feedback / check-in objects** (real-time feedback request, peer recognition, coaching notes) as first-class event objects feeding the review — not a once-a-year form | evaluation | Workday/Lattice³ | 3 | 4 | 3 | **4.0** |
| B13 | **Calibration session**: cross-group rating distribution view, rubric-anchored levels, self-eval hidden until manager's initial rating submitted | evaluation | perf calibration best-practice³ | 3 | 4 | 3 | **4.0** |
| B14 | **ABC-classed inventory policy**: per-class reorder point + safety stock (lead-time/forecast-error/supplier-reliability driven) + cycle-count cadence (A most frequent); reorder-point breach fires an Automate effect | inventory, purchase | Coupa/ISM⁴ | 3 | 4 | 3 | **4.0** |
| B15 | **Data-lineage graph** source→dataset→object/link type→apps&automations; trace any object property to the exact upstream transform | objectExplorer, dashboard, forecast | Foundry Data Lineage [brief §1f] | 3 | 5 | 4 | **3.8** |
| B16 | **Template-linked ad-hoc grants** (Cedar templates: `?principal`/`?resource` slots; edit template updates all links) for "share this doc/ticket with user Y" without writing N policies | policy, docs, appr, mail | Cedar templates [brief §2e] | 3 | 5 | 2 | **7.5** |
| B17 | **Ontology proposals = PRs**: branch → merge-check (conflict detect) → reviewer approval → changelog; protection = no direct-to-main schema edits | configconsole, ontology, policy | Foundry Ontology Manager [brief §1b] | 4 | 5 | 3 | **6.7** |
| B18 | **Field-level (property) security** = column policy; fail → field returns `null`, object still visible (cell-level authz over the same Cedar set) | hr (salary), payroll, finance | Foundry property policies [brief §1a] / Cedar field action [brief §2f] | 4 | 5 | 3 | **6.7** |

### Tier C — polish / long-tail (worthwhile, lower leverage or higher cost)

| # | Capability | Module | Benchmark | Impact | Fit | Cost | **Priority** |
|---|-----------|--------|-----------|:---:|:---:|:---:|:---:|
| C1 | **Variable-lineage / lazy-compute** binding graph for dashboard widgets (compute only when a visible widget displays it; recompute modes) | dashboard, canvas | Foundry Workshop variables [brief §1d] | 3 | 4 | 4 | **3.0** |
| C2 | **Unified inbox as pointer-stream** over all object types (approvals, mail, mentions, WO) — one triage surface, dedup, snooze; each row a deep-link into the object | inbox, mywork, notif | Foundry Automate notifications / Slack | 3 | 4 | 3 | **4.0** |
| C3 | **Alternate table representations** table↔kanban↔timeline↔matrix over one view AST (§4-25-⑧) — layout is view-config, not a new screen | every list module | Airtable/Notion layouts [brief §3d] | 3 | 4 | 3 | **4.0** |
| C4 | **Time-series analysis surface** (Quiver-style) over object time-series properties (52h trend, labor-cost trend, attendance) with honest-scaling (§4-24) | forecast, laborcost, attendance | Foundry Quiver time-series¹ | 3 | 4 | 4 | **3.0** |
| C5 | **Reverse-if-false two-way binding** on form field rules (condition goes false → auto-revert the action) | appr, hr forms | ServiceNow UI Policy reverse [brief §3f] | 2 | 4 | 2 | **4.0** |
| C6 | **Multi-environment promote** Dev→Staging→Prod with env-bound resource creds, env-agnostic version artifact | configconsole, rollout | ToolJet/Windmill [brief §3e] | 3 | 4 | 4 | **3.0** |

**Selection guidance for the next deep slice**: take from **Tier A first** — each is a single-engine
primitive (§4-20) that every module inherits, so the marginal module cost after the first is near-zero.
A1 (partial-eval row filter), A2 (saved views), and A6 (tri-state field rules) are the three highest-priority
increments that are also individually shippable. Within Tier B, **B16 (template-linked grants, priority 7.5)**
is the cheapest high-fit win. Avoid starting with Tier C bolt-ons before the Tier A grammar exists — they'd
be rebuilt onto the primitives later.

---

## §1. Overview group — `overview`, `mywork`, `inbox`

**① Best-in-class does what we don't** — Foundry's home/inbox and Slack's activity surface treat the inbox
as a **pointer stream over every object type** (an approval, a mention, a work-order assignment, a mail are
all just typed pointers), with dedup, snooze, and one-click deep-link into the source object. Workday's
"My Tasks" is the single commit-queue for every Business-Process step awaiting the user.

**② Fits our grammar** — Yes, natively. DESIGN §2 already declares "알림은 개체를 가리키는 포인터" and §5
declares every state transition an event. The inbox is the read-projection of the event/notification stream
filtered to `assignee == me` — this is exactly A1 (partial-eval residual) applied to the notification object set.
`mywork` = the BP commit-queue (B11). No bolt-on: it reuses the single ontology.

**③ Smallest increment** — **C2**: model the inbox as one pointer-stream object set with a typed `kind` badge
+ deep-link resolver; dedup identical pointers; snooze = a per-user view filter (A2). Defer alternate layouts.

---

## §2. HR group — `hr`, `recruit`, `orgchart`, `evaluation`

**① Best-in-class** — **Workday**: every HR action is an *event instance* through a configurable
Business-Process (ordered gated steps + mandatory commit) and every record is **effective-dated** (time-sliced).
**Greenhouse**: separates **Candidate (person)** from **Application (candidacy for one Req)**, ordered Stage
pipeline, per-stage Interview Kit, mandatory structured Scorecard on a fixed rubric; source-tracking lives on
the Application. **Lattice/Workday performance**: continuous feedback + check-ins + calibration sessions, not a
once-a-year form; self-eval hidden until the manager submits an initial rating; rubric-anchored level definitions.

**② Fits our grammar** — Strongly. Effective-dating (A4) and BP-framework (B11) are already the §3.9 lifecycle
spine; HR is the canonical consumer. Greenhouse's Candidate≠Application split (B3) maps to two object types
linked many-to-one to a Position/Req — DESIGN §2 already has 지원자 as its own object and §2-plan step 4 has
포지션→공고 one-click. Evaluation's continuous-feedback and calibration (B12/B13) are event objects (§5) +
a distribution analysis (honest-scaling §4-24), **no AI** — calibration is a deterministic cross-group
distribution view, not a model.

**③ Smallest increment** — **B3**: give `recruit` the Application object (distinct from person) with an ordered
Stage pipeline + a structured Scorecard whose attributes are typed schema (A8), reused verbatim by `evaluation`.
Field-level security (B18) nulls salary/rating for unauthorized viewers. Effective-dating (A4) lands as its own
Tier-A slice shared with payroll.

---

## §3. Payroll·Attendance group — `payroll`, `attendance`, `leave`, `benefit`

**① Best-in-class** — **Workday**: absence/time/payroll are BP event instances; effective-dated policy slices
(a rate change is a new validity interval, not an overwrite). **SAP**: payroll run = a period-gated pipeline
with a hard close. Overtime/leave requests are approvable transactions that auto-flow into the period.

**② Fits our grammar** — Exactly the §3 관계 체인: 근태 기록 ⇄ 연장근로(AP- 결재) → 근태 마감(게이트) →
급여 회차 → 이체. Effective-dating (A4) is load-bearing here (retro pay corrections, future-dated rate changes).
The month-close gate (§2 근태 marker) is a BP commit step (B11). Field-level security on pay figures (B18).
Leave balance = a folded event log (accrual/consume events), never a mutated counter — matches §0 substrate.

**③ Smallest increment** — **A4** (effective-dating) shared with HR, applied first to policy presets (근무·수당);
then leave-balance as a folded accrual/consume event stream so retro edits and future accrual are free.

---

## §4. ERP group — `finance`, `purchase`, `inventory`, `asset`

**① Best-in-class** — **SAP MM/FI**: 3-way match (PO↔GR↔Invoice) reconciled through a **GR/IR clearing account**
that nets to zero only within qty+price tolerance (else the invoice is blocked); every GL posting is a
**balanced document** (Σdebit = Σcredit). **Coupa**: ABC-classed inventory — per-class reorder point + safety
stock (lead-time/forecast-error/supplier-reliability) + cycle-count cadence (A items most frequent); reorder
breach auto-notifies. **SAP PM**: asset/work-order status profile with cost-posting gates.

**② Fits our grammar** — Strongly, and it forces the append-only substrate honestly: a balanced document is
literally header + line-items with a Σdr=Σcr invariant (B4), a natural Action-writeback (A3). 3-way match is a
deterministic predicate ("does the clearing line net to zero within tolerance") — **no AI**, exactly §4-20's
"조건 = field·operator·value 술어가 실제 평가". ABC policy (B14) is a typed config object (§4-22 no-free-text),
and the reorder-breach → PO-draft is an Automate Condition→Effect (A5). Asset ties to `maintenance` via the PM
status profile (B8).

**③ Smallest increment** — **B4**: model finance mutations as balanced header+lines with the Σ invariant enforced
server-side; wire `purchase` PO ↔ GR ↔ Invoice through a clearing-account net-to-zero-within-tolerance predicate
that **blocks** on mismatch. Inventory ABC policy + reorder Automate is a follow-on slice (B14 + A5).

---

## §5. Field Ops group — `dispatch`, `maintenance`, `field`

**① Best-in-class** — **ServiceNow FSM Dispatcher Workspace**: single view of technicians (status, skills,
parts, location, availability) + unassigned/assigned split; assign by skills+parts+proximity+availability
(one-click or rule-driven); **rebalance in real time** when a tech falls behind or an urgent job arrives.
**SAP PM**: work-order status profile `CRTD→REL→TECO→CLSD` with cost-posting gates, forward-only.

**② Fits our grammar** — Yes. A work order is already `WO-` (§2). The dispatcher board is a saved object-set
view (A2) over unassigned WOs; **assignment is an audited Action** (A3), not a table edit. The skills/proximity
match is a deterministic scoring predicate (§4-20, no-AI — replace "AI dispatch" with rule/predicate scoring).
Status profile (B8) is the §3.9 lifecycle specialized with cost gates. Rebalance = an Automate Condition
(tech-behind / new-urgent) → Effect (re-assign Action) (A5).

**③ Smallest increment** — **B8** (status profile with cost gates on `WO-`) first — it's the §3.9 lifecycle plus
two gates. Then **B7** dispatcher board as an A2 saved view whose assign-CTA is an audited Action; deterministic
skills/proximity scoring as a follow-on.

---

## §6. Governance group — `appr`, `docs`, `policy`, `compliance`, `audit`

**① Best-in-class** — **Workday BP** (approval chains as configurable gated steps ending in a mandatory commit;
routing modifiers). **Foundry Ontology Manager** (schema/policy changes via branch → proposal/PR → merge-check →
reviewer → changelog; protection = no direct-to-main). **Cedar** (deny-by-default, `forbid` guardrails always
win, templates for ad-hoc grants, property policies for field authz). **Vanta/Drata** (Control→Test→Evidence,
cross-framework mapping, continuous re-run). **ISO 15489/OAIS/FRE 902(14)** (fixity-hashed WORM AIP,
self-authentication by matching hash).

**② Fits our grammar** — This group *is* the grammar. `appr` = BP framework (B11) over the §3.9 lifecycle with a
finalization step (DESIGN §2 종결 규칙: 최종승인 ≠ 종결, author confirms, Cedar override with audited reason).
`policy` = Cedar itself — templates (B16) for ad-hoc doc/ticket sharing, property policies (B18) for field authz,
proposals-as-PRs (B17) for policy changes. `compliance` = Control→Test→Evidence (B5), deterministic tests
(field·operator·value predicates re-run on cron — no AI). `audit` = the L20 fixity-hashed WORM chain (B6),
already in flight per project memory (L20 PR-1 merged). All map to §0 substrate.

**③ Smallest increment** — **B16** (Cedar template-linked ad-hoc grants, priority 7.5, Cost 2) is the cheapest
high-fit win in the whole register — "share doc X with user Y" = link a template, not a new policy. Then **B11**
finalization step on `appr` (author-confirms + audited Cedar override), then **B5** Control→Test→Evidence for
`compliance`. B6 (audit WORM) continues the existing L20 lane.

---

## §7. Analytics group — `dashboard`, `laborcost`, `objectExplorer`, `forecast`

**① Best-in-class** — **Foundry Object Explorer**: one search bar over the whole ontology, **search-around**
(link traversal as a UI verb), linked-object filtering, charts aggregate on main **or linked** types,
**saved Explorations** (revisitable, latest results), shareable deep-links; **Quiver** for object-set analysis
+ time-series; **Data Lineage** traces any property to its upstream transform. **Foundry Workshop**: dashboard
widgets bound to typed variables via a lazy-compute lineage graph.

**② Fits our grammar** — Perfect fit — this group is the ontology's read/analysis face (§4-20 그래프는 노드
이동을 넘어 검사·조작). Search-around (B1) and saved object sets (B2) are pure single-engine consumers.
Charts must honor honest-scaling (§4-24). Forecast, being no-AI, is deterministic projection (moving average /
run-rate / seed-predicate simulation §4-20), not an ML model. Data-lineage (B15) makes derived numbers
(충원율, 인건비) drill to source per §5. Dashboard widgets = Workshop variable lineage (C1).

**③ Smallest increment** — **B1** (search-around + linked-object filtering + deep-links on `objectExplorer`) —
it upgrades the graph from click-to-navigate (§4-20 미완) to inspect+filter+aggregate. Then **B2** saved
Explorations feed `forecast`/`dashboard` as reusable object-set inputs.

---

## §8. Automation group — `workflow`, `scheduled`

**① Best-in-class** — **Foundry Automate**: model = Condition(s) → Effect(s), checked continuously or on
schedule. Conditions: time-based, object-data (over object *sets*, not raw tables — "new Alert priority=high",
"objects added to set", "objects modified", periodic sweep). Effects: **Action execution** (the same Action
humans use), function invocation, notification, webhook; triggering objects flow as effect inputs; chainable.
**Temporal**: durable append-only event history, deterministic replay, effectively-once side effects.

**② Fits our grammar** — This is a first-class §4-20 requirement ("로직·통합·임계·라우팅 = 정책/설정 개체로
추출, 코드 상수 하드코딩 금지") and §4-22 (configurable = typed predicate, simulation runs the predicate on a
seed sample — no decorative toasts). A5 (Condition→Effect over object sets) is the whole module. Determinism is
already our charter (no-AI); Temporal's replay-safety maps to our folded-event-log substrate (§0). `scheduled`
= the time-based Condition variant.

**③ Smallest increment** — **A5**: a rule = `Condition(object-set predicate) → Effect(named Action / notify)`
where the predicate is typed field·operator·value and **simulation actually runs it on a seed sample** (§4-22).
Effect verbs are the same Actions (A3) humans invoke — one mutation surface. This is the single highest-value
Tier-A build for this group and reused by every module (reorder breach, dispatch rebalance, SLA escalation).

---

## §9. Comms group — `messenger`, `mail`, `notif`, `board`, `directory`

**① Best-in-class** — **Slack**: message keyed by immutable `ts`; thread = `thread_ts` self-FK (flat, one level);
link unfurl is event-driven metadata-only (`link_shared` → `chat.unfurl`), never trusts the event to carry body.
**Gmail**: header-chain threading (In-Reply-To + References first, normalized-subject only as tiebreak);
**labels = many-to-many tags**, not folders; a thread's labels = union of members'. **Teams/Slack**: mentions,
presence, and every message is addressable/deep-linkable.

**② Fits our grammar** — Yes — DESIGN §2 already makes 메시지 스레드·메일·공지 first-class objects and §4-23
makes every object a drag-reference candidate (`[코드 제목]` tokens via `objDrag`), §4.7 the `@#!` token grammar.
So the message/thread PK model (B9) and label junction (B10) are the correct persistence shapes for objects we
already have. Unfurl-as-metadata-event (B9) is our own reference-token resolver — an object dragged into a
composer resolves to a live chip, exactly the metadata-only→resolve→render pattern. `directory` is a people
object-set view (A2) gated by §4-19 `personVisible`/`peopleAllowed`. `notif` is the §1 pointer-stream (C2).

**③ Smallest increment** — **B9** (thread PK `(channel, ts)` + nullable `thread_ts` self-FK, one-level) on
`messenger`/`board`, and reuse the reference-token resolver as the unfurl path (a dragged object → metadata
pointer → live chip). **B10** (header-chain threading + label junction) lands on `mail`.

---

## §10. Config / cross-cutting engine — `configconsole`, `ontology`, `policy`, `window`, `canvas`

**① Best-in-class** — **Retool/Appsmith/ToolJet/Windmill/Budibase** all converged: every component/query/variable
is a globally-named object in a flat namespace; UI props bind via `{{ }}` expressions forming an auto-rerunning
**dependency graph**; reads are pure reactive bindings, **writes quarantined to explicit imperative actions**;
apps stored as **serializable config documents, one per screen** (Retool `.positions.json` per page to avoid
merge conflicts); config is a **governed object** — per-user invisible drafts → review → immutable
content-addressed released version → env-agnostic artifact + env-bound creds → free rollback; config carries its
own **effective-principal** (`permissioned_as` ≠ `created_by`). **Foundry**: fields = discriminated-union schema
(A8); interpolation is a locked injection boundary (Windmill `$var:`/`$res:` — no arbitrary eval at the secret edge).

**② Fits our grammar** — This is the substrate under §4-20/§4-22/§3.9.0 directly. A7 (config-as-governed-object),
A8 (discriminated-union field schema), A6 (tri-state field rules enforced client **and** server), and A2
(views as saved AST) are all here. Critical for our unprotected-main + parallel-lane reality: **one config
document per screen** (not a monolith) to survive concurrent editing — directly echoes the project's own
draft-PR-while-iterating memory. The config's own effective-principal (A7) prevents privilege escalation via
shared config — a Cedar-native property.

**③ Smallest increment** — **A8** (discriminated-union field schema with rename-safe IDed options + degrade-on-
unknown-type) first — it unblocks A6, A2, and §4-22 add-anything (new field = a governed data write, no
migration). Then **A7** governance wrapper (immutable version + promote) on top of the existing §3.9.0
revision-staging. **A6** tri-state field rules with mandatory server-side twin (never trust client-only).

---

## Anti-patterns this register guards against (from §4-20/§4-22/§4-6)

- **Bolt-on capability that copies best-in-class UI but forks the model** — any capability scoring Fit ≤ 2 is a
  trap: it defines the ontology twice (§4-20 "같은 온톨로지를 두 번 정의하면 위반"). None promoted above Tier C.
- **AI-shaped features** — every capability here is restated as a **deterministic predicate / rule / template /
  simulation** (§4-20 no-AI charter). Forecast = run-rate projection, dispatch match = scoring predicate,
  calibration = distribution view, compliance test = field·operator·value re-run. No model dependency.
- **Free-text "config"** — every configurable surface is a typed config object (enum/number/operator/field
  select), not a decorative label (§4-22).
- **Client-only rule enforcement** — A6/A7/B18 all mandate the server-side twin over the same data model
  (§3f "never trust client-only config enforcement").

---

## Sources (beyond `[brief §N]` inline citations)

1. Palantir Foundry — Object Explorer save-explorations & Quiver: <https://www.palantir.com/docs/foundry/object-explorer/save-explorations>, <https://www.palantir.com/docs/foundry/quiver/overview>, <https://www.palantir.com/docs/foundry/quiver/timeseries-overview>
2. ServiceNow FSM — Dispatcher Workspace / dynamic scheduling: <https://www.servicenow.com/products/field-service-management.html>, <https://www.servicenow.com/products/dynamic-scheduling.html>, <https://www.servicenow.com/docs/r/field-service-management/optimizing-scheduling-and-dispatching-operations.html>
3. Performance review calibration & continuous feedback: <https://www.small-improvements.com/blog/performance-review-calibration/>, <https://www.workday.com/en-us/perspectives/hr/types-of-performance-management-system.html>, <https://www.betterworks.com/magazine/8-top-performance-management-tools-for-workday-users>
4. Inventory ABC / cycle-count / reorder-point best practice: <https://www.coupa.com/blog/what-is-inventory-management/>, <https://www.ism.ws/logistics/inventory-management-analysis/>, <https://www.fishbowlinventory.com/blog/inventory-cycle-count-key-steps-and-best-practices>

All `[brief §N]` references resolve to `docs/program/benchmark-brief.md` (compiled 2026-07-09), which carries the
primary live-sourced URLs for Foundry, Cedar, config-consoles, Workday, Greenhouse, Temporal, Slack, Gmail, SAP,
Vanta/Drata, and ISO 15489/OAIS/FRE 902.
