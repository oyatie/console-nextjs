# My Work UX and capability benchmark

Research date: 2026-07-12 EDT

Scope: the authenticated personal-work landing for a Korean-first, no-code conglomerate-operations console.

Decision type: evidence-backed product and architecture benchmark; not an implementation spec or release claim.

## 1. Executive decision

Build **My Work as a repair-and-reuse composition over the existing governed work substrate**, not as a new module shell and not as an alias of the current Overview page.

The repository already contains most of the expensive pieces:

- a server-owned personal fan-in at `GET /api/v1/me/action-inbox`;
- person-scoped todos and pending dispatch offers;
- a receipt-gated personal legal-document inbox;
- source-specific approval, dispatch, support, and work-order mutations;
- an Overview action queue with filtering, deadline ordering, J/K/Enter navigation, partial-failure handling, pins, and direct mutations;
- shared console list, chip, state, object-card, policy, and window primitives.

The missing product is the composition and contract alignment. Today, `mywork` is still a navigation entry without a mounted body, the existing Overview client bypasses the canonical action-inbox fan-in, receipt-required documents are absent from the action queue, support scope differs between the client fan-out and the personal backend fan-in, and mutation failures collapse denied, stale, and transient outcomes into one generic toast. The canonical backend fan-in is also all-or-nothing: any source read failure aborts the whole response, and its response contract exposes neither per-source health nor a safe diagnostic reference. That contradicts the required partial-failure behavior and must be repaired rather than inherited.

The recommended slice therefore:

1. makes the server-owned action inbox the canonical open-work read model;
2. extends it only where mandatory sources or action metadata are missing;
3. extracts reusable queue/filter/row/due/action/error primitives from the current Overview implementation;
4. composes those primitives into both My Work and the Overview top-N summary;
5. keeps full approval, dispatch, inbox, support, and object pages as source-of-record drill targets;
6. routes every inline mutation through the existing source endpoint with fresh authorization, idempotency, audit, and precise denied/stale handling.

This is an **M-sized repair**. A greenfield page plus another client-side aggregator would be an L-sized duplication with a higher policy-drift risk.

## 2. Evidence and authority method

### 2.1 Authority order used

The temporary program consolidation explicitly ranks live code and exact execution evidence above audits, the console design mirror, and historical ledgers (`/tmp/maintenance-program-consolidation.md:31-49`). It also says the dominant problem is product breadth and daily workflows, not another architecture rewrite (`/tmp/maintenance-program-consolidation.md:9-27`), places My Work first in the daily-surface wave (`/tmp/maintenance-program-consolidation.md:462-481`), and prohibits page-specific component grammar (`/tmp/maintenance-program-consolidation.md:68-92`).

Accordingly, this benchmark uses:

1. current source and OpenAPI in this checkout for the built-state baseline;
2. `docs/program/parity-matrix.md` and `docs/program/ontology-coverage-matrix.md` for audited gaps;
3. `DESIGN.md`, `docs/ideas/enterprise-role-workflows.md`, `docs/benchmarks/issue-55-collaboration-work-hub.md`, `docs/design/oyatie-console/DESIGN.md`, `docs/design/oyatie-console/ROADMAP.md`, and `docs/design/oyatie-console/AGENTS.md` for product requirements and reusable grammar;
4. `docs/program/benchmark-matrix/overview.md` and the benchmark index for prior vendor research;
5. current first-party vendor pages for bounded capability verification.

The root checkout is a stale local `feat/cedar-activation` branch whose upstream is gone. Current-code findings in this brief are therefore workspace observations, not claims about remote `main`, a PR head, or production. The implementation lane must repeat the source audit on a fresh exact-main worktree before turning this benchmark into a spec.

The historical `docs/program/console-program-ledger.md` is used for intent and provenance only, consistent with the consolidation warning that it is a snapshot requiring reconciliation (`/tmp/maintenance-program-consolidation.md:42-49`).

### 2.2 Claim labels

- **Documented requirement** — stated by repository authority.
- **Current implementation evidence** — present in current source or OpenAPI.
- **Verified vendor pattern** — observed on a first-party source on 2026-07-12 EDT.
- **Repository-reviewed vendor pattern** — supported by a prior repository benchmark, but not independently revalidated in this pass.
- **Recommendation** — synthesis constrained by the evidence above.

### 2.3 Retrieval limitation

The current ServiceNow documentation route cited for My To-dos returned an error page during this pass. ServiceNow My To-dos claims below are therefore limited to the repository-reviewed benchmark dated 2026-06-28 (`docs/ideas/enterprise-role-workflows.md:9-31`); this document does not promote the failed live retrieval into new fact. The current Dispatcher Workspace documentation did load and exposed its configuration topics.

## 3. Product boundary: Overview, My Work, and Personal Inbox are different jobs

| Surface | Primary question | Required content | Must not become |
|---|---|---|---|
| **Overview** | “How is my permitted operating scope doing?” | compact KPI/exception summary, top-N urgent work, agenda | a second full task manager |
| **My Work** | “What can or must I act on now?” | open-only personal queue, due/urgency filters, direct governed actions, todos | an executive dashboard or a client-side cross-domain scrape |
| **Personal Inbox** | “Which private or legal documents were delivered to me?” | receipt state, passkey gate, document metadata/payload rules | a generic notification list or unauthenticated preview |
| **Source module/object** | “What is the full context and lifecycle?” | complete record, evidence, conversation, history, all valid actions | duplicated inside every queue row |

This separation follows the root Work Hub decision: the first authenticated surface must answer what needs attention, what is blocked on approval, where conversation/evidence lives, and which source object owns the work (`DESIGN.md:3-23`). It also preserves the queue → object → action pattern and rejects one-off cosmetic screens (`docs/ideas/enterprise-role-workflows.md:44-73`).

## 4. Best-in-class evidence retained

### 4.1 SAP Task Center — canonical enterprise task fan-in

Verified from the official SAP Task Center guide on 2026-07-12:

- one place to access tasks assigned to the user across task providers;
- custom search;
- sort by priority, due date, and status;
- filter by priority, task type, creation date, status, due date, and custom attributes;
- inline task actions;
- task details plus “open in app” for the provider’s full action set;
- due-soon and overdue notifications can be configured.

Product implication: My Work needs a server-owned, open-only task fan-in with source provenance, due/status filtering, one safe inline action, and source drill. It does not need to reproduce every source form.

Source: SAP, *SAP Task Center* public guide, pp. 477-492 and task-list sections, accessed 2026-07-12: <https://help.sap.com/doc/ab1cc29fb9aa41889779ce4f699142cd/Cloud/en-US/TaskCenter_PUBLIC_EN_1.pdf>.

### 4.2 Microsoft Teams Approvals — approval-in-flow status and federation

Verified from Microsoft Support on 2026-07-12:

- sent and received approvals are visible in one hub, including completed approvals;
- approvals can originate across Power Automate, SharePoint, and Dynamics 365;
- approval cards expose real-time status, including who has and has not responded;
- submitters can decide approval order;
- workflow approvals created through Power Automate appear in the same list.

Product implication: a My Work approval row should preserve the approval’s source and current turn/status. Completed approvals belong in explicit history, not the default action queue. Direct decisions must still use the source workflow’s rules.

Source: Microsoft Support, *What is Approvals?*, accessed 2026-07-12: <https://support.microsoft.com/en-US/Teams/free/what-is-approvals>.

### 4.3 ServiceNow Dispatcher Workspace — configuration is part of dispatch UX

Verified from the current ServiceNow Field Service Management documentation tree on 2026-07-12. Dispatcher Workspace exposes configuration for:

- work-order state colors;
- task-card and contextual side-panel fields;
- which tasks appear in the task panel;
- dispatcher filters and sort options;
- map appearance;
- dynamic scheduling and loaded resources;
- advanced resource filters, popovers, hourly calendar, keyword search, and UI Builder customization.

Product implication: the personal dispatch slice may be compact, but its fields, filters, and action affordances should come from the shared task/object contract and governed view configuration. A hardcoded dispatch-only card grammar would regress the platform.

Source: ServiceNow, *Configuring Dispatcher Workspace*, accessed 2026-07-12: <https://www.servicenow.com/docs/r/field-service-management/configuring-dispatcher-workspace.html>.

### 4.4 Palantir Action Types — one governed action logic across surfaces

Verified from Palantir documentation on 2026-07-12:

- an Action is a single transaction changing object properties or links from user-defined logic;
- Action Types can define standardized parameters, rules, validation, authorization, and side effects;
- the same action logic and validation can be exposed across user-facing applications;
- successful actions write back to the Ontology and become visible to all applications.

Product implication: My Work should present existing governed actions, not implement another mutation path. Inline actions are projections of source actions; preflight, authorization, state validation, side effects, audit, and writeback remain authoritative behind the source endpoint.

Source: Palantir, *Action types — Overview*, accessed 2026-07-12: <https://www.palantir.com/docs/foundry/action-types/overview>.

### 4.5 Prior repository benchmark patterns retained

The repository’s overview benchmark supports:

- Asana-style Today/Upcoming/Later personal task ergonomics;
- Slack-style fast filter-pill triage;
- SAP My Inbox/Task Center federation and source drill;
- one-click decide as the highest cross-module step-collapse opportunity;
- live counts and governed configuration as reusable platform capabilities.

Sources: `docs/program/benchmark-matrix/overview.md:76-125`, `docs/program/benchmark-matrix/overview.md:332-376`, and `docs/program/benchmark-matrix/overview.md:380-390`. These are retained as repository-reviewed patterns; current code evidence supersedes that file’s older “Overview unbuilt” baseline.

## 5. Mandatory parity benchmark

Mandatory means the capability is explicitly required by the cited product sources and needed for the My Work surface to stop being an empty or misleading navigation target. It does not mean every adjacent vendor feature is in scope.

| Capability | Mandatory UX contract | Source evidence | Current implementation evidence | Gap to close |
|---|---|---|---|---|
| **Actionable-item filtering** | Default to non-terminal items assigned to or awaiting the current principal. Filter by urgency/due bucket, source/kind, and scope permitted by policy. Preserve a user/team saved view through the governed console-view model rather than a page constant. | `docs/ideas/enterprise-role-workflows.md:33-53,87-97,223-236`; `docs/benchmarks/issue-55-collaboration-work-hub.md:69-76`; SAP Task Center verified above. | Backend fan-in scopes sources and excludes terminal support/work-order states (`backend/app/src/action_inbox.rs:1-26,223-242,262-283`). Overview has live kind filters and deadline ordering (`web/src/pages/OverviewPage.tsx:274-288,394-416`; `web/src/features/overview/overview-data.ts:207-231`). | My Work body absent; Overview does not consume the canonical fan-in; no saved-view binding; urgency uses a fixed 24-hour heuristic rather than a source commitment setting. |
| **Approval turn** | Show that the item is awaiting this user, the source object/workflow, requester/context available from the source, due/commitment, and one context-valid next action. Decisions requiring memo/evidence/step-up open the shared governed action panel; do not allow blind approval. | `docs/benchmarks/issue-55-collaboration-work-hub.md:48-52,69-76`; Microsoft Approvals verified above; design’s single-context CTA rule (`docs/design/oyatie-console/DESIGN.md:36-45`). | Overview claims/approves workflow tasks through real endpoints (`web/src/pages/OverviewPage.tsx:303-339`). The federated approval API carries ontology/workflow/policy envelopes (`docs/specs/issue-55-approval-command-center.md:15-30`). | Current Overview direct approve has no memo, reject, evidence, step-up, or precise policy reason. The generic action-inbox item has no action descriptor or approval-turn metadata. |
| **Dispatch queue** | Surface pending offers and assigned open work for the caller; show response window or target due time, source work order/site, and only currently valid responses. Accept/decline uses the dispatch source endpoint and refreshes the row. | Personal landing requirement in `docs/program/parity-matrix.md:35-40`; dispatcher persona in `docs/design/oyatie-console/ROADMAP.md:121-130`; ServiceNow Dispatcher verified above. | Action inbox includes person-scoped pending offers and assigned non-terminal work (`backend/app/src/action_inbox.rs:195-220,262-320`). Overview accepts offers through the source endpoint (`web/src/pages/OverviewPage.tsx:340-349`). | Decline is not exposed in Overview; the shared row lacks action metadata and site is omitted for offers; fixed card fields are not governed-view driven. |
| **Receipt acknowledgement** | Include legal documents awaiting receipt as actionable items. Show safe metadata while locked; selecting the action starts the Personal Inbox/passkey flow. Never render the protected payload or call a list/read as acknowledgement. Success must produce the existing receipt evidence. | Root UX rule for sensitive decisions (`DESIGN.md:13-23`); legal inbox design (`docs/design/oyatie-console/DESIGN.md:168-180`); HR/payroll receipt story (`docs/benchmarks/issue-55-collaboration-work-hub.md:59-76`). | OpenAPI already lists receipt-required docs with `filter=action`, withholds locked payloads, and separates read from confirmation (`backend/openapi/openapi.yaml:6188-6267`). | Receipt-required docs are absent from `/me/action-inbox`, Overview, and My Work. The dedicated console inbox body remains a parity gap. |
| **Due/SLA visibility** | Sort by real due/response deadline; expose overdue, due-soon, and no-deadline states without color alone. Label contractual SLA separately from internal SLO. Use source/configured commitment semantics, not one global heuristic presented as truth. | Executive/manager stories (`docs/benchmarks/issue-55-collaboration-work-hub.md:54-57,69-76`); SLO ≠ SLA invariant (`docs/design/oyatie-console/DESIGN.md:222-223`); SAP Task Center verified above. | Action inbox carries `due`, urgency, and due tone, but computes urgency with a fixed 24-hour heuristic (`backend/app/src/action_inbox.rs:124-143`). Overview displays due labels and sorts by due (`web/src/features/overview/overview-data.ts:60-69,207-210`). | No commitment type or setting reference; no explicit overdue text; global 24-hour bucket can mislabel source-specific SLA/SLO/response windows. |
| **Direct governed actions** | Permit the most common valid action from the row when it can be safely completed with the available context. Otherwise open the shared ObjectCard/action panel. Source endpoint reauthorizes and returns an execution/audit receipt; no queue-local business mutation. | Palantir Action Types verified above; reusable Action doctrine in `docs/program/benchmark-matrix/INDEX.md:17-24`; design guardrails (`docs/design/oyatie-console/DESIGN.md:73-86`). | Overview executes real workflow, dispatch, and support mutations, uses idempotency for workflow actions, then reloads (`web/src/pages/OverviewPage.tsx:303-391`). Governed ObjectCard already provides preflight/execute UI. | Action definitions are hardcoded in `overview-data.ts`; support/approval mutations bypass the generic governed preflight presentation; action success is inferred from response presence rather than a normalized receipt. |
| **Denied and stale action handling** | Covert/inaccessible items remain omitted. A known visible item whose action is denied shows a non-sensitive policy/state explanation only when the backend authorizes that explanation. A stale/invalid transition refreshes or removes the row and announces that the work changed; it must not silently retry or claim success. | “Action validity is visible” and terminal exclusion (`docs/ideas/enterprise-role-workflows.md:55-68,87-97`); root role-gated-route and failure-state rules (`DESIGN.md:13-23`); source-specific mutations reauthorize (`docs/specs/issue-55-approval-command-center.md:22-30`). | The approval, dispatch-response, and support-transition contracts expose 403/404/409 outcomes (`backend/openapi/openapi.yaml:1231-1267,5074-5104,9921-9960`); workflow errors preserve `forbidden`, `conflict`, and `invalid_transition` classes (`backend/app/src/workflow_studio.rs:7449-7497`). | Overview catches all mutation errors, drops status/body, emits one generic failure toast, then reloads (`web/src/pages/OverviewPage.tsx:315-389`). This is a mandatory repair. |
| **Partial-failure resilience and source observability** | A failed source must not blank healthy work. Return the healthy items plus bounded per-source health, a non-sensitive error class, and a correlation reference; identify the failed source in the UI and support a bounded retry. Server logs/traces retain the internal cause without exposing it to the caller. | Aggregate surfaces require partial- and full-failure states (`DESIGN.md:13-19`; `/tmp/maintenance-program-consolidation.md:475-481`); the prior Work Hub preserves loaded sources during a partial failure (`docs/benchmarks/issue-55-collaboration-work-hub.md:91-96`). | Overview's client fan-out preserves successful sources and reports failed ones (`web/src/pages/OverviewPage.tsx:443-454`). The canonical fan-in instead uses fail-fast `?`/`map_err` reads for each source and exposes only `items` plus `total` (`backend/app/src/action_inbox.rs:118-122,153-303`). OpenAPI documents neither a partial-source envelope nor the handler's possible 500 (`backend/openapi/openapi.yaml:6504-6538,19784-19795`). | Extend the canonical response and instrumentation so source failure is explicit, non-sensitive, correlated, and non-blank. Define bounded retry semantics and regenerate clients; do not fall back to a permanent client-side aggregator. |
| **Accessibility** | Semantic queue/list, labeled filters and counts, visible focus, keyboard list navigation, screen-reader announcement for refresh/action outcomes, no color-only urgency, and ≥44 px touch targets in narrow/mobile layouts. | Consolidated quality bar (`/tmp/maintenance-program-consolidation.md:68-78`); ROADMAP responsive/accessibility DoD (`docs/design/oyatie-console/ROADMAP.md:6-15`); mobile target rule (`docs/design/oyatie-console/DESIGN.md:168-176`). | Overview has semantic labels, `aria-pressed`, focus rings, and J/K/Enter navigation with tests (`web/src/pages/OverviewPage.tsx:457-516`; `web/src/pages/OverviewPage.test.tsx:408-423`). | Current filter/action controls use 28-36 px minimum heights, below the mobile 44 px contract; generic toasts need verified live-region behavior; urgency tone is not paired with explicit overdue/due-soon text everywhere. |
| **Reusable console primitives** | One queue model and row/filter/action grammar must serve My Work, Overview top-N, notification action rows, and source modules. Reuse shared pins, ObjectCard/preflight, list grammar, partial-failure states, policy gates, and window model. No My Work-specific modal, panel, or alternate action evaluator. | Reusable-first and no page-specific grammar (`/tmp/maintenance-program-consolidation.md:68-92,162-175`); DESIGN reuse and pattern propagation (`docs/design/oyatie-console/DESIGN.md:109-112,139-166`). | Existing reusable pieces include console primitives, `useListNav`, `PageEmpty/PageError/Skeleton`, `PinButton`, ObjectCard/governed actions, policy gates, and window manager. | Overview’s item builders and mutation switch are page-specific; My Work has no composition; action-inbox schema and client model are not aligned with the existing row/action model. |

## 6. Mandatory parity versus optional future work

### 6.1 Mandatory parity for the first production My Work slice

1. Mount a substantive `mywork` body; no empty route or alias that silently changes Overview semantics.
2. Use a server-owned, principal-scoped, open-only queue as the canonical source.
3. Include at least the already-supported mandatory source families:
   - workflow/approval tasks awaiting the caller;
   - pending dispatch offers and assigned open work;
   - assigned actionable support work;
   - receipt-required personal inbox documents;
   - user todos as the personal planning rail, not as server-assigned work.
4. Preserve source provenance and one-click drill to the source object/module.
5. Show approval turn, response/due time, and SLA/SLO/other deadline semantics without fabricating unavailable context.
6. Expose only source-authorized primary actions; source endpoint must reauthorize at click time.
7. Distinguish success, denied, stale/conflict, disappeared/not-found, validation, and transient/partial failure.
8. Keep completed/closed/archived items out of the default action queue; history is an explicit view.
9. Preserve loaded sources during partial failure and offer source-specific retry.
10. Meet the keyboard, focus, semantic, announcement, contrast, and 44 px narrow-layout requirements.
11. Reuse shared queue, object, action, policy, state, and window primitives.
12. Lock the flows with backend RLS/runtime-role tests, component interaction/a11y tests, and persona E2E.

### 6.2 Optional, evidence-supported future work

These are valid but not required to close the My Work parity gap:

- SAP-style bulk/multi-task decisions after source workflows can prove uniform safe semantics.
- Approval delegation, substitution, escalation, 전결/대결/부재중 위임, and richer ordered 결재선.
- Slack-style snooze/clear-all and a separate activity/notification triage feed.
- Asana-style calendar/board views and richer end-user widget personalization.
- ServiceNow-style dispatcher schedule/map workspace inside the full dispatch module.
- Mobile home-screen top-N widget, offline action packs, and push action cards.
- Cross-system external task-provider federation beyond repository-owned sources.
- Predictive prioritization or AI recommendations. Repository authority explicitly defers AI/LLM product features (`/tmp/maintenance-program-consolidation.md:68-79`).

Optional items must not be represented by inactive controls or placeholder cards in the mandatory slice.

## 7. Reusable target architecture

### 7.1 Canonical flow

`principal-scoped server queue -> shared WorkQueue view -> source object / shared governed action -> source mutation -> audit/action receipt -> queue refresh`

The queue is a read model, not a new system of record. Domain sources own state and mutations.

### 7.2 Existing assets to reuse or repair

| Asset | Reuse decision |
|---|---|
| `backend/app/src/action_inbox.rs` | Keep as the canonical bounded fan-in; extend source coverage/metadata and replace its all-or-nothing response with a typed partial-source envelope rather than build a second aggregator. |
| `/api/v1/me/todos` | Keep as personal planning data; render beside the action queue and never conflate todo completion with source workflow completion. |
| `/api/v1/me/inbox-docs?filter=action` | Add as receipt-required queue input or server-side fan-in source; preserve locked-payload and passkey semantics. |
| `web/src/pages/OverviewPage.tsx` | Extract its queue mechanics; keep Overview as KPI/top-N composition, not the owner of personal-work business logic. |
| `web/src/features/overview/overview-data.ts` | Replace page-specific hardcoded builders with source adapters to one shared row model. |
| `useListNav`, `Chip`, `MonoRef`, `PageEmpty/PageError/Skeleton` | Reuse directly. |
| `PinButton`, ObjectCard, governed preflight/execute UI | Use for context and actions that cannot safely complete inline. |
| PolicyGate/authz projection | Reuse for display gating only; backend remains authoritative. |
| Window/pin/tray model | Use for source context; do not create a My Work modal/panel stack. |

### 7.3 Shared primitives to extract, not duplicate

Names are conceptual and do not prescribe exact filenames:

- **WorkQueue** — semantic list, keyboard selection, loading/empty/partial/full-failure states.
- **WorkQueueFilters** — source, urgency/due, scope, and saved-view binding.
- **WorkItemRow** — source chip, object reference/title, due/commitment text, provenance, one primary action, source drill, pin.
- **CommitmentChip** — overdue/due-soon/no-deadline plus explicit `SLA`, `SLO`, response window, or ordinary due type.
- **GovernedInlineAction** — pending state, idempotency, step-up/preflight handoff, normalized outcome classification, audit receipt.
- **WorkQueueErrorState** — denied/stale/disappeared/validation/transient handling without information leakage.

The same primitives should serve:

- full My Work;
- Overview’s urgent top-N projection;
- future actionable notification rows;
- approval/dispatch/support source summaries.

### 7.4 Minimum read-model contract

This is a capability contract, not a proposed OpenAPI patch. A shared row must be able to represent:

- stable source-namespaced item identity;
- source kind and source object identity/title;
- current principal’s relationship to the item (assignee, approver turn, recipient);
- lifecycle/actionable state;
- due timestamp plus commitment type and configured threshold reference when available;
- source drill and bounded linked-object references;
- display-safe requester/site/context fields when the source authorizes them;
- a source-owned primary action descriptor or a clear “open context” action;
- whether preflight, memo/evidence, four-eyes, or passkey step-up is required;
- no embedded protected document payload, no copied policy evaluator, and no fabricated AP-/CS- codes.

If a source cannot honestly supply a field, omit it and keep the source drill. The current action-inbox implementation already follows this non-fabrication rule (`backend/app/src/action_inbox.rs:18-26`).

### 7.5 Minimum aggregate response and telemetry contract

The response around the shared rows must also represent:

- total items and bounded source coverage;
- per-source `ok` or failed health without disclosing protected object existence;
- a stable, non-sensitive error class suitable for UI copy and retry policy;
- a correlation/request reference that operators can join to server-side logs or traces;
- whether returned items are fresh for that source, so stale retained data is never presented as a successful refresh;
- deterministic full-failure semantics when no source can be read.

Each source query remains principal-scoped and bounded. Internal database/provider details stay server-side. The client may identify and retry a failed source through the canonical contract, but it must not recreate source authorization, visibility predicates, or a permanent browser-owned fan-in.

## 8. Denied, stale, and partial-failure behavior

| Runtime outcome | Required behavior |
|---|---|
| **Success with receipt** | Announce success, retain receipt/audit reference where the source returns one, refresh, and remove the row only when it is no longer actionable. |
| **403 denied** | Do not apply optimistic state. For covert data, omit the row. For a still-visible non-sensitive object, show only a backend-authorized policy/state explanation and a source drill if permitted. |
| **409 conflict / invalid transition** | Treat as stale work, not a generic error. Announce that the item changed, refresh the source, and remove or update the row. Never auto-replay the mutation. |
| **404 absent/inaccessible** | Refresh and remove the row without distinguishing “deleted” from “not permitted” unless the backend explicitly authorizes that distinction. |
| **401/session freshness** | Fail closed, use the normal session refresh/reauth path, and do not retain an enabled mutation control. |
| **422 validation / missing memo/evidence** | Keep the row, expose the safe actionable requirement, and open the shared governed action panel rather than adding a page-specific form. |
| **Network/5xx source failure** | Preserve successful sources, identify the failed source, provide source-specific retry, and avoid a blank all-or-nothing page. |
| **Passkey required** | Navigate to or open the existing receipt/action step-up flow; do not label the work complete until the source returns confirmation evidence. |

This behavior turns the design’s deny-by-omission and fail-closed rules into a usable action surface rather than a silent failure.

## 9. Complexity budget

### 9.1 Budget definition

The recommended first slice is bounded to **M**:

- **one canonical server fan-in**, extended rather than replaced;
- **one shared web work-queue model and primitive family**;
- **two compositions**: full My Work and Overview top-N;
- **existing source mutations only** for approval, dispatch, support/work, receipt, and todo flows;
- **zero new domain stores**;
- **zero new policy evaluators**;
- **zero new modal/window systems**;
- **zero placeholder source adapters**;
- one bounded OpenAPI/client regeneration only if the canonical row/action contract changes.

A source requiring new legal semantics, new transaction ownership, or a new domain FSM is outside this slice and must become a separate full-stack charter. The queue may link to it only after it exists.

### 9.2 Required proof within the budget

- backend runtime-role tests for self/scope filtering, terminal exclusion, cross-org/branch denial, receipt metadata non-leakage, and bounded fan-in;
- source mutation tests proving fresh authorization and 403/404/409 outcome mapping;
- web tests for filters, saved view, J/K/Enter, touch targets, direct action, preflight handoff, passkey handoff, partial failure, and denied/stale announcements;
- persona E2E for at least:
  - operator: pending dispatch -> response -> source work order;
  - manager: approval turn -> governed decision/context -> row leaves queue;
  - employee: locked legal notice -> passkey receipt confirmation -> evidence state.

The E2E personas are already explicit in the repository’s user stories and ROADMAP; they are not new product requirements.

## 10. Alternatives evaluated

| Alternative | Cost | Evidence fit | Main risk | Verdict |
|---|---:|---|---|---|
| **Reuse current Overview unchanged / alias `mywork` to it** | S | Reuses real actions and tests | Conflates scope-health Overview with personal work; keeps client fan-out; omits receipts; support scope can exceed “assigned to me”; stale/denied remain generic | **Reject** |
| **Repair + reuse canonical fan-in and shared queue primitives** | M | Matches consolidation, parity, reusable-first, SAP task fan-in, Palantir action reuse, and current assets | Requires careful contract migration and normalized outcome handling | **Recommend** |
| **Do nothing / leave `mywork` empty** | 0 | Contradicts daily-surface priority and empty-route ban | Daily users remain without the required landing; nav promises a capability that is absent | **Reject** |
| **Greenfield My Work page + new client aggregator/action switch** | L | Can mimic vendors quickly | Duplicates Overview and backend fan-in, forks policy/action logic, creates a one-off shell, increases stale-state and scope-leak risk | **Reject** |
| **New dedicated work-orchestration service/store** | XL | Could support external provider federation later | Premature architecture rewrite; duplicates domain ownership and violates “consume the engine, do not fork it” | **Reject for parity; reconsider only if falsification test fails** |

## 11. Falsification test

The repair-and-reuse recommendation is invalid if the existing canonical fan-in and source actions cannot support the mandatory work loop without duplicating source policy or transaction logic.

Run this read-model/action spike before committing the implementation plan:

1. Seed, in one RLS-armed tenant, exactly one of each:
   - open approval awaiting the caller;
   - pending dispatch offer and assigned open work order;
   - assigned open support item;
   - locked receipt-required legal document;
   - overdue SLA item and due-soon internal SLO/ordinary-due item where those source types exist;
   - completed/closed item;
   - same-org item assigned to another user;
   - cross-org item;
   - item made stale after queue load.
2. Read the canonical queue as operator, manager, and employee principals.
3. Assert exact inclusion/omission, source provenance, action turn, due/commitment label, and safe metadata.
4. Execute each mandatory action through the existing source endpoint; assert same-transaction audit/action receipt and that refresh removes or updates the row.
5. Execute the pre-staled action; assert 409/invalid-transition handling, no write, no auto-retry, and an accessible stale-state announcement.
6. Inject one source-read failure; assert healthy-source rows remain visible, the failed source and safe error class are represented, a correlation reference reaches server telemetry, protected details are absent, and retry cannot duplicate or widen work.
7. Fail every source; assert the response and UI enter a deterministic full-failure state rather than showing a false empty queue.
8. Verify that no row requires a second source-specific client read merely to decide whether it is visible or which primary action is valid.

**Recommendation is falsified if any mandatory source can be represented only by:**

- widening visibility beyond its source predicate;
- reimplementing authorization or lifecycle rules in the queue;
- writing through a queue-owned mutation path;
- exposing protected receipt payload before step-up;
- adding a source-specific modal/panel/action evaluator;
- performing per-row/N+1 reads to discover basic actionability;
- or blanking healthy work or losing source diagnostics whenever one fan-in source fails.

If falsified, the replacement is not a one-off My Work page. It is a separately specified **shared work-read-model service/contract** that all work surfaces consume, with source adapters that retain domain ownership.

## 12. Acceptance benchmark for the eventual implementation

The My Work parity claim is valid only when all of the following are demonstrated on an exact head:

1. `mywork` mounts a substantive shared-primitive composition.
2. Default queue contains only currently actionable principal-scoped items.
3. Approval, dispatch, receipt, due/SLA, todo, and source-drill stories work on real APIs.
4. Denied, stale, disappeared, validation, partial, and full failures are distinguishable and fail closed; a one-source read failure preserves healthy work and emits a safe correlation reference.
5. Completed/closed items appear only in explicit history.
6. Direct actions reauthorize and yield audited receipts; no queue-local business mutation exists.
7. Receipt payload remains locked until the existing passkey flow confirms receipt.
8. Narrow layout uses ≥44 px targets; keyboard and screen-reader flows pass.
9. Overview and My Work share the same queue primitives/model rather than parallel implementations.
10. Backend runtime-role tests, web interaction/a11y tests, and the three persona E2E stories pass.
11. No placeholder, demo, fixture-only, wire-pending, or unsupported control ships.
12. A fresh source audit shows no requirement in this document exceeded its cited evidence.

## 13. Source register

### Repository sources

- `DESIGN.md:3-23` — Work Hub first, interaction model, role-gated links, failure states, source benchmark pointer.
- `docs/benchmarks/issue-55-collaboration-work-hub.md:40-76` — role stories and capability requirements.
- `docs/ideas/enterprise-role-workflows.md:33-73,87-97,200-238,271-295` — queue-first direction, action validity, role workflows, Work Hub acceptance bar, anti-goals.
- `docs/program/parity-matrix.md:35-40,183-209` — My Work parity gap and ranking.
- `docs/program/console-program-ledger.md:17-24,68-79,109-118` — reusable/governed architecture and snapshot scope.
- `docs/program/ontology-coverage-matrix.md:43-83,86-126` — current object/UI coverage constraints.
- `docs/program/benchmark-matrix/overview.md` — prior overview/vendor benchmark and cross-lens findings.
- `docs/program/benchmark-matrix/INDEX.md:11-38,42-69` — ranked cross-module reuse/action findings and caveats.
- `docs/design/oyatie-console/DESIGN.md:36-86,88-112,139-180,197-223` — single CTA, guardrails, reusable grammar, all-employee scope, personal receipt inbox, review protocol, SLO/SLA distinction.
- `docs/design/oyatie-console/ROADMAP.md:6-18,33-43,121-133` — common DoD, shared systems, persona flows.
- `docs/design/oyatie-console/AGENTS.md:13-31,76-84` — reusable patterns and prototype My Work/dispatch intent.
- `/tmp/maintenance-program-consolidation.md:1-49,53-92,193-218,239-296,462-481` — temporary consolidation, authority hygiene, product/quality bar, benchmark synthesis, current gaps, daily-surface order.
- `backend/app/src/action_inbox.rs` and `backend/app/tests/action_inbox_api.rs` — current canonical server fan-in and RLS evidence.
- `backend/openapi/openapi.yaml:1231-1267,5074-5104,6188-6267,6504-6538,9921-9960,19711-19795` — source mutation outcomes, receipt inbox, and action-inbox contracts.
- `web/src/pages/OverviewPage.tsx`, `web/src/features/overview/overview-data.ts`, `web/src/features/overview/TodayPanel.tsx`, and `web/src/pages/OverviewPage.test.tsx` — current reusable behavior and gaps.

### First-party external sources accessed 2026-07-12 EDT

- SAP Task Center public guide: <https://help.sap.com/doc/ab1cc29fb9aa41889779ce4f699142cd/Cloud/en-US/TaskCenter_PUBLIC_EN_1.pdf>
- Microsoft Support, *What is Approvals?*: <https://support.microsoft.com/en-US/Teams/free/what-is-approvals>
- ServiceNow, *Configuring Dispatcher Workspace*: <https://www.servicenow.com/docs/r/field-service-management/configuring-dispatcher-workspace.html>
- Palantir, *Action types — Overview*: <https://www.palantir.com/docs/foundry/action-types/overview>

---

The benchmark intentionally recommends no one-off My Work shell, no new work store, no AI prioritization, and no fake source integrations. The product move is to make the existing governed system visible, actionable, reusable, and honest at the user’s first daily landing.
