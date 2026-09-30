> **SUPERSEDED PRE-PIVOT SPEC:** This frozen record describes the former
> conglomerate/FSM/ERP/comms/frontend program and its deleted toolchain. It must
> not dispatch work, define current scope, or be used as a runnable command guide.
> Current scope is only Ontology/Foundry/Policy → Company/OrgUnit/Employee → HR →
> Payroll under
> [`docs/PIVOT-2026-07-28.md`](../PIVOT-2026-07-28.md).

# Spec: Conglomerate Operations Platform — the entire business as software, in one place
(anchor entity: KNL; first delivered vertical: forklift FSM)

> Status: Phase 1 CONVERGED — deep-interview ~19% ambiguity (≤20% gate) + research-grounded.
> Executing via `/ultragoal` (phased extraction). MVP acceptance criteria = the carbon-copy-Foundry
> CAP-1..N (Ontology catalog · Object Views · Object-Set query API · scoped RBAC+RLS · action/write-back
> engine) + Korean-payroll computation list + vertical object models in
> `.omc/research/foundry-domain-research.md`.
> Owner: Jason Lee (KNL). Author: Claude. Branch: `main`.
> Design north star: `.omc/research/palantir-blueprint.md`. Research + fix plans:
> `.omc/research/{foundry-domain-research,broken-flows-fix-plan,role-workflow-backlog,
> analytics-intelligence-roadmap,webmail-build-plan,dispatchmap-build-plan}.md`.

## Objective

One object-centric operational platform that runs **an entire conglomerate's business** — not an FSM
tool with add-ons. KNL (Korea Next Logistics, 창원) is the **logistics** arm (forklift maintenance ·
rental · used-sales) of a larger **group** whose other legal entities run very different industries —
**COSS (제조/manufacturing)**, **BESTEC (OEM 생산)**, and a **인력 파견·용역 agency** (cleaning · security ·
labor staffing). Each entity has its own operational vertical, they share one back office (HR · payroll
· accounting · procurement), and the group needs **consolidated visibility + central administration**
across all of them. Today this lives across a desktop FLMS app, Excel files, and manual process. The
target is a single platform where
**every real-world thing is a first-class Object** (장비, 작업지시, 정비사, 고객, 현장, 직원, 급여,
거래처, 전표, 재고품목, 매물…) with a 360° view, clickable links, and role-gated, audited, closed-loop
actions — benchmarked **heavily** on Palantir's pre-AI operational design (Foundry ontology + object
views, Gotham map/timeline/graph/faceted lenses, the Blueprint visual system, decision-first triage,
provenance). **No AI/LLM integration in the immediate scope; AI/ML/RL/LLM is a last-stage augmentation
after the mechanical, algorithmic, observable operating system is proven.**

Success = a KNL employee runs their whole job here — receive work, plan, dispatch, execute, approve,
bill, get paid, manage people, close the books — without leaving the platform, with nothing silently
failing and every number traceable to its source.

## Strategy (crystallized via deep-interview)

- **North star**: a faithful **carbon-copy of Palantir Foundry** for the operating substrate (no
  immediate AI) — a generic, object-centric
  operational platform that can **capture any business's operations, employees, assets, and workflows**
  as first-class objects, ultimately with a **full no-code configurable ontology** (users define object
  types/links/actions/workflows). Diverge into KNL/conglomerate-specific capabilities *after* the
  generic core is faithful.
- **Build path = PHASED EXTRACTION** ("ontology by extraction, not speculation" — the executive call,
  vs platform-first which is ~12–18mo-to-value + over-abstraction + worst-case security):
  1. **Stabilize** the live business (issue-#19 broken flows) — value now.
  2. **Generic object-centric core** (object-views · triage · lenses · scoped RBAC · write-back actions),
     built generic from day one, with **KNL FSM as the first configured instance** that proves it —
     continuous ROI each increment.
  3. **Extract the ontology engine** from the proven patterns (custom fields → configurable lifecycles/
     forms → links/actions).
  4. **Open configurability**: low-code (adapt existing types) → **no-code** (define new types).
  Same destination the user chose; de-risked, financed by shipped value, secured on a proven isolation
  model. The MVP capability scope of the carbon-copy core is being grounded by Foundry research.
- **Assets are universal**: real estate/factory, machinery, vehicles, equipment are all **Asset** types
  with the same lifecycle/cost/depreciation/maintenance/observability; the forklift is a specialized
  config. Same for **Work Item / Party / Money / Inventory**.

## Scope (domains)

1. **FSM** (exists, being hardened): equipment registry · work-order lifecycle · dispatch · inspection/
   preventive-maintenance · evidence media · substitution(대차) · rental · used-sales/storefront.
2. **HR** (new): Employee (extends User: 직급/rank, 부서, hire date, employment type) · 조직도(org chart) ·
   Attendance(출퇴근 — the clock-in/out from the dispatch-map track) · Leave(연차).
3. **Payroll** (new, REGULATED — full Korean engine): 4대보험(국민연금/건강/고용/산재) · 근로소득세 간이세액 +
   지방소득세 · 주휴수당 · 연차수당 · 퇴직금 · 최저임금 검증 · 급여명세서.
4. **ERP — Accounting/회계** (new, REGULATED): double-entry 분개/원장 · 부가세(VAT) · 세금계산서(전자세금계산서) ·
   재무제표.
5. **ERP — Procurement+AP/구매·매입** (extends 구매요청): 거래처(Vendor) master · PO → 입고 → 거래명세표(#18) →
   미지급금(AP).
6. **ERP — Inventory/재고·부품** (new): 부품 master · 재고 수량 · 입출고 · 작업지시→부품 소모→원가.
7. **ERP — Sales+AR/영업·매출** (extends sales/rental/inquiry): 견적 → 수주 → 세금계산서 → 미수금(AR).
8. **Platform substrate** (exists): multi-tenant RLS · auth/passkey/OTP · authz matrix · audit ·
   storage(RustFS) · realtime · webmail(comms) · platform console(view-as/onboard/remove).

Domains 2–8 are the **shared back office** (every entity uses them; books/payroll per-법인, consolidated
at the group).

### Domain separation rails

Use four explicit management surfaces instead of one overloaded "admin" area:

- **Platform console** — vendor/SaaS operator controls tenancy lifecycle, support access, release/ops,
  and group identity/membership. It is not where day-to-day HR/payroll/maintenance work happens.
- **Group management** — conglomerate operators manage group membership, consolidated dashboards, and
  cross-subsidiary policies. Group views can combine subsidiaries or isolate one organization, but every
  write names the target organization and is audited against that organization.
- **Organization management** — each legal entity manages org units, positions, worksites, users,
  custom roles, and local settings inside its RLS boundary.
- **Business-domain consoles** — HR, payroll, executive management, and logistics/maintenance are
  separate capability groups. HR owns employment facts; payroll owns compensation/tax/bank data;
  executive management owns KPI/governance/operations rollups; logistics/maintenance owns dispatch,
  work orders, equipment, and field evidence. A company or position may have none of the
  logistics/maintenance capabilities.

Positions, business titles, and departments are **not** login roles by default. They can suggest role
assignments during onboarding/import, but security grants remain a separate RBAC action with preview,
least-privilege checks, and audit.

### Near-term setup priority

Do not start with advanced analytics. The current setup sequence is:

1. **Organization foundation:** group, tenant/legal entity, organization structure, departments/units,
   positions, people, account seeds, roles, and scoped permissions.
2. **Operating master data:** clients/customers, locations/worksites, addresses/geodata, assets,
   equipment, models/specs/manufacturers, reserve/spare-parts catalogs, and assignment relationships.
3. **Import readiness:** safe Excel/CSV ingestion, preview, column mapping, dry-run validation, audit
   ledger, and standardized export templates for the above entities.

Only after those foundations are reliable should the backlog move into cross-business analytics and
optimization.

### Operations intelligence / analytics backlog (not current setup slice)

The platform should eventually support business-wide optimization across every domain: rental pricing,
cost/profitability analytics, asset lifecycle sell/keep recommendations based on operating cost and
expected market value, reserve equipment/spare-parts sizing from SLA/probabilistic demand models,
workforce utilization and scheduling optimization, and executive scenario planning. These capabilities
must be built as governed analytics over trusted master data with explainable assumptions and audited
write-back recommendations, not as ad-hoc dashboards inside the maintenance vertical.

Detailed guardrails live in `docs/specs/operations-intelligence.md`. In short: nail the mechanical
operating system first — trusted master data, ledgers, workflow outcomes, deterministic calculators,
probabilistic forecasts, policy, audit, and observability. Only after that foundation works should
AI/ML/RL/LLM assist executive decision making, operational intelligence, analytics, scenario narratives,
or workflow drafting. AI output must remain an untrusted draft/recommendation routed through the same
policy/workflow/passkey controls as human proposals.

**CORE PRINCIPLE — capability-driven & industry-agnostic. Do NOT hardcode industries.** The platform is
generic operational primitives + a configurable ontology; an industry is a *configuration*, not code
(the Palantir Foundry model — one platform, many operations). Build the CAPABILITY to encompass anything
a business needs in operational intelligence, analytics, software, and tools:

9.  **Generic operational primitives** (named by role, never by industry): **Work Item** (work orders,
    tasks, production orders, placements — all specializations) · **Asset** (lifecycle/maintenance/cost/
    location) · **Party** (person/employee/customer/vendor/worker + organization) · **Place** (site/geo/
    hierarchy) · **Schedule** (plans/calendars/recurrence) · **Approval/Governance** (configurable chains,
    self-approval rules, anomaly) · **Document/Evidence** (media/files) · **Money** (ledger/payroll/
    billing/AP/AR) · **Inventory Item** (stock/movements) · **Message**.
10. **Configurable ontology**: object types — properties, identity, lifecycle/FSM, links, actions,
    permissions — defined as configuration over the primitives, not a bespoke crate per industry.
11. **Generic intelligence + tools over whatever is configured**: faceted analytics · configurable KPIs
    · anomaly detection · the map/timeline/graph lenses · object-views · triage-home · ⌘K · scoped RBAC.

**KNL's forklift FSM = the first concrete CONFIGURATION** (the reference instance that proves the
primitives), not a hardcoded vertical. Other entities (manufacturing, OEM, 파견·용역 staffing, anything
future) are configured from the same primitives + a JIT discovery sub-spec for their real process — they
require configuration/extension, not a new bespoke app. Engineering guardrail: build generic **where a
real KNL need proves the primitive**, design every capability so a new business is a configuration not a
rewrite, evolve toward runtime-configurable ontology — but never over-abstract into a speculative
meta-platform that ships nothing. Generic by design, grounded by a real first instance.

## Tenancy & Org Model (hierarchical, conglomerate-capable)

Not a flat tenant list. A multi-level org hierarchy:

```
Platform (vendor/SaaS operator — administers all tenants for support; the existing platform tier)
└─ Group / 그룹·conglomerate            ← a holding entity; an Org belongs to 0..1 Group
   │                                      (the KNL group is the first/anchor instance)
   └─ Legal Entity / 법인  (= Org)       ← the RLS HARD-ISOLATION boundary (app.current_org), UNCHANGED
      │  e.g. KNL(물류) · COSS(제조) · BESTEC(OEM) · 인력 agency(파견·용역) — each its own vertical
      └─ Region → Branch → Worksite/Site
         └─ Customer (external) → their Sites/worksites
```
(Architecture supports MANY groups — the platform stays multi-tenant SaaS-capable; the KNL conglomerate
is simply the first group, its four 법인 the first members.)

- **Within a tenant**: multiple locations (Region/Branch), clients (Customer), worksites (Site) —
  already modeled; deepen so every one is a navigable Object.
- **Above a tenant — Group**: owns multiple 법인 and needs **combined visibility + administration**
  and the ability to **separate/combine** entities (conglomerate central management).

**Security model (the keystone — gets a dedicated sub-spec `docs/specs/org-hierarchy.md` +
architect + security-review BEFORE any code):**
- The RLS hard boundary **stays at the Legal Entity (Org)**. The Group layer is a *controlled
  cross-entity scope*, never a hole in isolation.
- **Consolidated group view = aggregation over per-member ARMED reads** (iterate member 법인, each
  read RLS-correct under its own `app.current_org`). **Never a BYPASSRLS blanket read.**
- A **cross-entity admin action** arms the *specific* target 법인 and is audited.
- A group-admin reaches **only their group's member entities**; unrelated groups/orgs stay invisible.
  Distinct from the vendor platform tier (view-as any tenant for support — already built, #9).
- **Access = SCOPED RBAC + least privilege** (the operational-ergonomics core). Every principal has an
  **access scope** = a node in the hierarchy + its subtree (Group · 법인 · Region · Branch · Worksite/
  Team) × their **role** (the existing authz matrix decides *what features*). Effective access = the
  role's permissions, applied **only within the scope subtree**:
  - **Group-admin** (privileged): scope = all member 법인 → **manage every subsidiary in ONE screen**
    (consolidated), with a **scope selector** to switch to a single-법인 (or single-branch) view.
  - **법인-admin**: scope locked to their one legal entity.
  - **Branch / Worksite / Team-local**: scope = their subtree only, **least privilege** — they see and
    do *only what they need* (e.g. a worksite manager → their worksite's work/people; a team lead →
    their team), never the rest of the entity or group.
  - Builds directly on the existing `BranchScope { All | Branches[] }` — generalize it to a hierarchy
    `AccessScope { level, node_id }` resolved at login from the principal's assignment; every list/read
    intersects the scope subtree, every action checks scope-membership + role. The scope selector is one
    shared shell control (group-admin sees all options; a scoped user sees only theirs, no toggle).
- Data model sketch (to be designed/validated): `organizations.group_id` (nullable) + a `groups`
  table + group-membership/group-role grants in the identity/authz layer; an RLS-aware group-read
  helper that fans out over member orgs. Highly-sensitive cross-entity data (payroll, financials)
  stays per-法인 RBAC even within a group unless a group-finance role is explicitly granted.

## Architecture — the Palantir object-centric model (see `palantir-blueprint.md`)

Five layers, one substrate. **Stop building pages; build objects.**
1. **Ontology** — object types on the tenancy spine `Group → Org(법인) → Region → Branch → User`
   (see Tenancy & Org Model), joined by real FK links, each with role-gated Actions. New domains =
   new objects + links + actions, same substrate.
2. **Object Views** — ONE reusable `ObjectViewScaffold` + per-type config (identity → properties →
   linked-object rail → timeline → audit/provenance → ActionBar). Not N hand-rolled detail pages.
3. **Triage Home** (`/home`) — per-role "what needs me now" queues; each row = object + resolving action.
4. **Four lenses** over the same objects — Map(#29) · Timeline · Graph · Faceted analytics.
5. **Blueprint skin** — `.console`-scoped dense operational-industrial visual system (color=meaning,
   mono/tabular for ids/₩/dates, keyboard-first, ⌘K omnibar, dark-ready) extending the C1–C13 polish.
Through-line: **closed-loop + provenance** — every screen ends in a named, validated, audited,
console-only Action; every object shows source + freshness + its AuditEvent stream.

## Tech Stack

- Web: React 19 + Vite + Tailwind v4 (`@theme`) + react-router 7 + Pretendard. i18n in
  `web/src/i18n/ko.ts` (no inline Hangul; `check-ui-strings` enforces). Typed client `@maintenance/
  api-client-ts` from openapi.
- Backend: Rust (axum), workspace of bounded-context crates (`domain/application/adapter-*/rest`),
  sqlx, multi-tenant Postgres 18 with RLS (runtime role `console_rt` NOBYPASSRLS + FORCE RLS; owner
  `console_app` runs migrations out-of-band). OpenAPI served via `include_str` (static).
- Infra: OCI/Talos k8s, CNPG Postgres, Argo Rollouts, image-release auto-build+deploy on push;
  secrets in OCI Vault.
- New external integrations (ask-first): Korean address/geocode (Kakao), 전자세금계산서 relay
  (팝빌/바로빌 or NTS), payroll 요율/세액표 data source.

## Commands

- Web: `cd web && npm run lint` (eslint + check-ui-strings) · `npm run build` (tsc -b + vite) ·
  `npm test` (vitest; `--exclude '**/PlatformConsole.test.tsx'` until #31 fixes its OOM) ·
  `npm run gen:api:ts`.
- Backend: `cd backend && SQLX_OFFLINE=true cargo fmt --all --check` · `cargo clippy --all-targets -- -D
  warnings` · `DATABASE_URL=postgres://<user>@localhost/console_ci cargo test -p <crate>` (username
  REQUIRED) · gates `cargo run -p console-gate-{rls-arming,tenant-isolation,layer-boundary,audit-coverage,
  migration-safety}` · `npm run check:openapi-app` · `npm run gen:api:swift`.

## Project Structure

```
backend/crates/<context>/{domain,application,adapter-postgres,adapter-*,rest}   # bounded contexts
backend/crates/platform/{auth,authz,storage,realtime,request-context,db/migrations,email,comms}
backend/ci/gates/<gate>                  # static-analysis gates (rls-arming, tenant-isolation, …)
backend/app/src/lib.rs                   # composition root (routers + workers)
backend/openapi/openapi.yaml             # single source; clients regenerated from it
web/src/{pages,features/<domain>,components/{ui,shell,states},i18n,lib,context,api}
docs/specs/                              # THIS spec + JIT domain sub-specs (payroll, accounting, …)
.omc/research/                           # blueprints, audits, fix plans (durable)
```
New domains add a bounded-context crate + a `web/src/features/<domain>` + object-view config(s).

## Code Style

Match the surrounding code. Every tenant read/write arms `app.current_org`; tests run as real `console_rt`.
```rust
// Read: armed, org-scoped, fail-closed under FORCE RLS.
pub async fn list_payslips(&self, employee_id: EmployeeId) -> Result<Vec<Payslip>, RepoError> {
    let org = current_org().map_err(KernelError::from)?;          // fail-closed; never a default tenant
    with_org_conn(&self.pool, org, move |tx| Box::pin(async move {
        sqlx::query_as!(/* … WHERE employee_id = $1 … */).fetch_all(tx.as_mut()).await
    })).await
}
// Write: AuditEvent carries .with_org(org) so with_audit arms the GUC (the #27/#43 lesson) —
// org MUST be a dynamic current_org()-derived value, never a hardcoded OrgId literal.
```
Web: shared `ui` primitives only (Dialog/Combobox/DataTable/SkeletonTable/FeedbackBanner); object
links via `ObjectLink` (never render a raw UUID — `safeLabel`); copy in `ko.ts`; KST via
`formatKoreanDateTime`; ₩/ids in `Mono`.

## Testing Strategy

- **TDD**: write the failing test first (the chained `test-driven-development` skill).
- **Backend**: every tenant flow has a **real `console_rt` RLS test** (seed via the ARMED path, not the
  BYPASSRLS owner pool) proving create→read round-trips, org-scoping, cross-tenant invisibility, and
  fail-closed-unarmed. The `rls-verify-as-runtime-role` rule is mandatory.
- **Regulated domains**: payroll + accounting get **golden-case tests** against worked examples
  validated by a 노무사/세무사 (e.g. a known 급여명세서, a 부가세 신고 case), plus rate/세액표 table tests
  keyed by effective-date.
- **Web**: vitest unit/integration per page + the shared kit; **visual-verdict ≥90** on every path
  before a UI phase is "done" (the chained `visual-verdict` skill).
- **Gates** green (rls-arming + the new org-binding lint #43, tenant-isolation, layer-boundary,
  audit-coverage, migration-safety) + fmt/clippy + check:openapi-app before any commit.

## Boundaries

**Always**: arm `app.current_org` + a real `console_rt` test for every tenant read/write · openapi-first
for new endpoints + regen clients · incremental (≤~5 files/task) · gates+fmt+clippy+tests green +
evidence before claiming done · authoring and review are separate passes (code-reviewer / security-
reviewer; never self-approve) · operational/business mutations go through the audited console API
(never direct SQL).

**Ask first**: DB schema changes · new dependencies · new external integrations or API keys (Kakao
geocode, 전자세금계산서 relay, payroll data) · anything that changes the authz matrix or RLS posture ·
enabling a regulated module in production.

**Never (current foundation stage)**: AI/LLM/generative/RL production write-back or decision automation
before the deterministic/algorithmic product, observability, evaluation, rollback, privacy, and workflow
controls in `docs/specs/operations-intelligence.md` are live · ship a **payroll or 세금계산서**
calculation to production without (a) versioned effective-dated rate/세액표 config, (b) a worked
golden-case test, and (c) a 노무사/세무사 sign-off — the system computes + issues, a licensed
professional validates; we do not provide tax/labor advice · secrets to /tmp · self-approving a review ·
weakening tenant isolation.

## Success Criteria

1. **FSM loop closes** end-to-end as real `console_rt`: intake → plan → dispatch → execute(evidence) →
   approve → close, with nothing "created but invisible" (issue #19 #13/#17/#18/#21/#22 fixed + tested).
2. **Object-centric**: ≥6 core objects have an Object View reachable via ⌘K + inter-object links; the
   triage home is the post-login landing for operator roles; raw UUIDs never shown.
3. **HR/Payroll**: employees with 직급/조직도 + attendance + leave; a payroll run produces a 급여명세서
   matching a 노무사-validated golden case for a sample employee; salaries are RBAC-restricted + audited.
4. **ERP**: a transaction flows 견적→수주→세금계산서→미수금 and PO→입고→거래명세표→미지급금; a 부가세 period
   reconciles; parts consumed by a WO decrement inventory + post to the cost ledger.
5. **Provenance**: every KPI/number drills to its source objects; every mutation is audited.
6. **Quality**: all gates green; visual-verdict ≥90 on every path; no premature AI/ML/RL/LLM, and any
   future intelligence remains recommendation-only until governed workflow approval.

## Phased Roadmap (incremental; folds in existing tasks)

- **Track A — STABILIZE (now, no full spec — diagnosis is the spec):** issue-#19 broken-loop bugs (#40)
  + #19 P0 fixes (#33/#5/#7). Operator works today; landed on the object-view/queue pattern.
- **Track B0 — ORG HIERARCHY (foundational, security-critical; sub-spec `org-hierarchy.md` + architect
  + security-review FIRST):** the Group→법인 model, the RLS-preserving consolidated-read helper, group
  membership/roles, the consolidate↔single scope selector. Precedes group-consolidated reporting and
  any cross-entity admin; the per-法인 RLS boundary itself is unchanged so it does not block Track A/B.
- **Track B — PLATFORM ARCHITECTURE (Palantir phases):** P0 Blueprint tokens → P1 Object-View kit →
  P2 Equipment Object View → P3 ⌘K nav fabric + DataTable → P4 Triage Home → P5 Faceted analytics
  → P6 Timeline + Graph lenses → unified inquiry pipeline(#42) + import pipeline(#35/#38).
- **Track C — HR/PAYROLL/ERP (new domains, each gets a JIT sub-spec in `docs/specs/` before build):**
  C1 HR core (Employee/직급/조직도 #19.11, Attendance/clock-in #29, Leave) → C2 Payroll (regulated;
  effective-dated rates; golden cases; 노무사 sign-off) → C3 Accounting/회계 (double-entry + 부가세 +
  전자세금계산서 relay) → C4 Procurement+AP (#18) → C5 Inventory (WO↔parts) → C6 Sales+AR.
- **Track D — COMMS/GEO (in flight):** webmail (B-mail-* done backend; UI + #39 hardening + #28 key
  before enable) · dispatch map (#29, clock-in-gated + arrival/return markers).

Each regulated/complex domain (payroll, accounting, the inquiry/import pipelines) gets its **own
detailed sub-spec** approved before implementation — this master spec is the umbrella + the invariants.

## Open Questions

1. **Org hierarchy / conglomerate** (see Tenancy & Org Model — gets sub-spec `org-hierarchy.md`):
   confirm group-admin gets BOTH consolidated visibility AND cross-entity administration (assumed),
   the consolidate↔single scope toggle, and whether payroll/financial data is group-visible or stays
   per-法인 unless a group-finance role is granted. (Note: #19.12's customer-법인 grouping — one
   customer's many 현장 — is the *separate, intra-tenant* Customer-hierarchy question; both exist.)
2. **전자세금계산서** — which relay (팝빌/바로빌/NTS direct)? Needs a business cert + an account.
3. **Payroll inputs** — pay cycle (월급/시급?), 통상임금 components, 노무사 contact for golden-case validation.
4. **FLMS migration** — vendor data format/script (issue #19.15) for the equipment masterlist + history import.
5. **Sequencing of Track C** vs finishing Track A/B — confirm priority order once this spec is approved.
