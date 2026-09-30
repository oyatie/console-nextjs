# Benchmark Research Brief — Ontology-First, Cedar-PBAC B2B Console

Reference brief for N build lanes. Grounds a Palantir-benchmarked, ontology-first, Cedar-PBAC,
deterministic (no-AI), enterprise-production console. Every claim is live-sourced (URLs inline).
Dense by design — read the area you're building, steal the pattern, follow the URL to verify.

Compiled 2026-07-09. Areas 1-3 (Foundry, Cedar, config-console) are the deepest.

---

## 0. The one substrate under all of it

Foundry (§1), Cedar (§2), config-consoles (§3), and half the modules (§4: Workday, Temporal,
SAP GL, OAIS) all converge on the **same primitive: an append-only, effective-dated / fixity-stamped
event log, with current state DERIVED by folding immutable records — never mutated in place.**
- Writes go through **declarative, named mutation verbs** (Foundry Actions, SAP balanced documents,
  Temporal Commands), landing in a writeback/event store, not direct table UPDATEs.
- Authorization is a **policy over the object/row/field** evaluated identically for UI, automation,
  and API (Foundry object/property policies; Cedar policy set) — deny-by-default.
- Config (schema, screens, rules, views) is itself **data — a governed, versioned, immutable-per-deploy object**.

Design everything below as a consumer of that substrate.

---

## 1. Palantir Foundry — the ontology benchmark

**Principle to internalize: "single ontology, many consumers."** One ontology is the shared
**semantic + kinetic** layer; every consumer reads and writes *through* it, never around it.
Object Explorer, Workshop apps, Automate rules, and API clients are all just consumers of one model —
a human clicking an Action and an automation firing the same Action hit identical logic, validation,
writeback, and security. Contrast the anti-pattern: N apps each with their own schema copy, their own
writeback path, their own bolt-on permissions.

### 1a. Ontology — objects, links, actions, functions
Sources: [overview](https://www.palantir.com/docs/foundry/ontology/overview),
[core-concepts](https://www.palantir.com/docs/foundry/ontology/core-concepts),
[create-object-type](https://www.palantir.com/docs/foundry/object-link-types/create-object-type),
[action-types/overview](https://www.palantir.com/docs/foundry/action-types/overview),
[rules](https://www.palantir.com/docs/foundry/action-types/rules).

- **Object type = schema for an entity/event; object = instance; object set = collection.** Docs' own
  analogy: object type ≈ dataset, object ≈ row, **properties = columns, property values = cells.**
- The model splits into **semantic** (objects, properties, links) + **kinetic** (actions, functions,
  dynamic security). This split is load-bearing: the ontology is a data model *plus the verbs that mutate it*.
- **Backing datasource**: an object type is a **typed projection over an existing dataset**, not a new
  store. Model = pick backing dataset → declare **primary key** (unique per row, required) → declare
  **title key** (human display name) → map columns to typed properties.
- **Link types**: schema of a relationship; supports 1-1 / 1-many / many-many (many-many needs a join
  dataset). Traversable in UI (search-around) and code (ObjectSet search-around).
- **Actions (writeback / kinetic mutation)** — the ONLY sanctioned way to mutate:
  - Action type = schema for a set of edits made atomically, + side effects.
  - Edits land in the object type's **writeback dataset** (most-current data incl. user edits),
    propagated to every consumer immediately. This is how a read-only projection becomes editable.
  - **Parameters** (user inputs; defaults, object-set-filtered dropdowns, security-aware dropdowns).
  - **Rules** = automated behavior on submit; **submission criteria** = validation gating submit.
  - **Side effects**: notifications, webhooks, build/schedule triggers, attachment uploads.
  - **Function-backed actions**: run custom function code, with **batched execution** over many objects.
- **Functions** = TypeScript/Python logic on objects (computed values, action backing).
- **Interfaces** = polymorphism (object types sharing a shape addressed abstractly). **Shared
  properties** = a property definition reused across object types (centralized metadata).

**Object security model** (spans 1a–1c) —
[object-and-property-policies](https://www.palantir.com/docs/foundry/object-permissioning/object-and-property-policies),
[restricted-views](https://www.palantir.com/docs/foundry/security/restricted-views):
- **Two axes**: **discretionary** (granular policies you author) + **mandatory controls** (markings /
  orgs / classifications inherited from the datasource — cannot be discretionarily loosened).
- **Object security policy** = row-level (fail → whole instance hidden). **Property security policy** =
  column-level (fail → that field returns `null`, object still visible). Together = **cell-level.**
- Configured in Ontology Manager Security tab, **independent of backing-dataset permissions** (a user
  needs only Viewer on the object-type definition). Constraints: PK can't be in a property policy; each
  non-PK property in ≤1 property policy.
- **Maps to us:** this is exactly the Cedar (§2) surface — object policy = row authz, property policy =
  field authz. Build both over the same policy set; deny by omission.

### 1b. Ontology Manager — schema authoring, Git-style governance
Sources: [ontologies-proposals](https://www.palantir.com/docs/foundry/ontologies/ontologies-proposals),
[review-ontology-proposals](https://www.palantir.com/docs/foundry/ontologies/review-ontology-proposals),
[foundry-branching/core-concepts](https://www.palantir.com/docs/foundry/foundry-branching/core-concepts).
- Object/link/action types authored in a UI (display name, types, PK, title key, security).
- **Branching**: changes on a branch off main; **rebase** to stay current.
- **Proposals = pull requests for the ontology**: create proposal → **merge checks** (conflict
  detection) → **reviewers** (approver must be editor/owner of the resource) → merge.
- **Protection**: once enabled, edits *must* go through branch + proposal (no direct-to-main).
- **Changelog tab** = full per-user, per-timestamp history.
- **Maps to us:** schema changes are code-reviewed and versioned like source. Governance =
  branch + proposal + merge-check + reviewer-approval + changelog. This IS the "config as governed
  object" pattern (§3E) applied to the ontology itself.

### 1c. Object Explorer — search + graph traversal
Sources: [overview](https://www.palantir.com/docs/foundry/object-explorer/overview),
[search-syntax](https://www.palantir.com/docs/foundry/object-explorer/search-syntax).
- Single search bar over the whole ontology; **graph view** shows link types between object-type groups.
- **Search-around** = link traversal as a first-class UI verb (hover a result → explore across a link).
- **Linked-object filtering**: filter the main object set on properties of *linked* types; charts
  aggregate a property on main or linked types. Actions runnable from the object view. Shareable deep-links.

### 1d. Workshop — configurable app builder (config-as-data)
Sources: [concepts-widgets](https://www.palantir.com/docs/foundry/workshop/concepts-widgets),
[concepts-variables](https://www.palantir.com/docs/foundry/workshop/concepts-variables),
[concepts-events](https://www.palantir.com/docs/foundry/workshop/concepts-events).
- **Widgets** = UI building blocks placed in a layout, configured with input/output variables + Actions.
- **Variables** = typed data flow (object set, string, numeric, boolean, date, timestamp, array, struct,
  geopoint, time-series). Init types: static, function, object property, object-set aggregation,
  object-set definition (filter/traverse), variable transformation.
  - **Lazy computation**: a variable computes only when a visible widget displays it. Recompute modes:
    automatic / event-triggered / load+event. **Variable lineage graph** shows widget↔variable deps.
- **Events** = trigger behavior on user action (button, row-select, dropdown, tabs).
- **Two-way binding**: widget params bind to variables (Workshop→widget); widget events bind to Workshop
  events and can update those params (widget→Workshop).
- **Custom widgets** (iframe): read/write Workshop variables + fire events via a documented bridge.
- **Maps to us:** an app is a **declarative document** — widgets + typed variables + event bindings +
  ontology Actions. No imperative app code. This is the target "per-app config as data" (see §3 for how
  the low-code world stores/versions this).

### 1e. Automate — object monitors → effects
Sources: [automate/overview](https://www.palantir.com/docs/foundry/automate/overview),
[condition-objects](https://www.palantir.com/docs/foundry/automate/condition-objects),
[effect-actions](https://www.palantir.com/docs/foundry/automate/effect-actions).
- **Model = Condition(s) → Effect(s)**, checked continuously or on schedule.
- **Condition types**: (1) **time-based** ("every Mon 9AM"); (2) **object-data** on the ontology
  ("new Alert with priority=high"); (3) combined. Object-set variants: *objects added to set*,
  *objects modified in set*, *run on all objects* (periodic sweep).
- **Effects** (4): **Action execution** (submit a Foundry Action on matching objects),
  **Function invocation**, **Notifications** (platform + email + attachments), **Webhook/external API**.
  Run sequentially or in parallel.
- **Effect inputs**: the exact triggering objects flow automatically as inputs to the effect.
- **Latency modes**: live / scheduled / automation-dependent (chain automations).
- **Maps to us:** monitors are defined **over object sets (ontology queries), not raw tables**, and the
  same Action types humans use are the effect verbs — human + automated paths share one mutation surface.

### 1f. Pipeline Builder / Data Connection + Data Lineage
Sources: [data-integration/overview](https://www.palantir.com/docs/foundry/data-integration/overview),
[data-lineage/overview](https://www.palantir.com/docs/foundry/data-lineage/overview),
[building-pipelines/overview](https://www.palantir.com/docs/foundry/building-pipelines/overview).
- **Flow**: Data Connection (syncs sources, credentials, governance; versioned via dataset transactions)
  → Pipeline Builder (visual pipelines, DQ, loading) → cleaned datasets → **mapped to object/link types**.
  Pipeline Builder can write **directly to the ontology** (output IS the object type's backing dataset).
- **Data Lineage** = interactive graph spanning source → dataset → **object + link types** → apps &
  automations, across project boundaries. Trace any object property back to the exact upstream transform.

**Actionable re-implementation cues:** (1) object types = typed views over existing tables with PK +
title key, not a new store; (2) all writes go through declarative Action types → a writeback table,
never direct UPDATEs; (3) security is a policy on the object/property, evaluated identically for UI,
automation, API; (4) apps and automations are declarative documents (widgets+vars+events /
conditions+effects) referencing ontology objects and Actions; (5) schema changes are branch+proposal+
merge-check+changelog.

---

## 2. AWS Cedar — the PBAC benchmark

Sources: [terminology](https://docs.cedarpolicy.com/overview/terminology.html),
[syntax-policy](https://docs.cedarpolicy.com/policies/syntax-policy.html),
Context7 `/cedar-policy/cedar-docs` (templates, schema, patterns, best-practices),
[partial-evaluation guide (Cedarland)](https://cedarland.blog/usage/partial-evaluation/content.html),
[Cedar OOPSLA paper](https://dl.acm.org/doi/full/10.1145/3649835),
[AWS prescriptive guidance](https://docs.aws.amazon.com/prescriptive-guidance/latest/saas-multitenant-api-access-authorization/cedar.html).

### 2a. Policy structure
A policy = **effect + scope + optional conditions + optional annotations**, ends with `;`.
- **Effect**: `permit` or `forbid`.
- **Scope** (mandatory): `(principal, action, resource)` — each can be unconstrained, `== Entity`,
  or `in Group`. `action` can be `in [Action::"a", Action::"b"]`.
- **Conditions**: `when { ... }` / `unless { ... }` over attributes of principal/resource/context.
```cedar
permit ( principal, action == Action::"editPhoto", resource )
when   { resource.owner == principal };

forbid ( principal, action, resource )
when   { resource.private }
unless { principal == resource.owner };
```

### 2b. The decision algorithm — deny by default, forbid wins
- **Allow iff at least one `permit` matches AND no `forbid` matches.** Otherwise **Deny**.
- **Deny by default / deny-by-omission**: no matching permit → Deny. **`forbid` always overrides
  `permit`** — a single matching forbid denies regardless of any permits. This is exactly the
  enterprise "deny-by-default, explicit forbid as guardrail" posture. Model tenant-isolation,
  legal holds, and hard blocks as `forbid` policies (they can never be accidentally out-permitted).

### 2c. Schema, entities, entity hierarchy
Sources: [human-readable-schema](https://docs.cedarpolicy.com/schema/human-readable-schema.html),
Context7 patterns/best-practices.
- **Schema** declares entity types (with typed attributes + `tags`), action types, and applies-to
  (which principal/resource types each action accepts). The **validator uses the schema to catch policy
  errors before evaluation** — treat schema as a compile-time contract; validate all policies in CI.
```cedarschema
entity User in [Group] { personalGroup: Group, delegate?: User, blocked: Set<User> } tags String;
entity Group enum ["G1","G2","G3"];
```
- **Entity hierarchy** = the `in` / `memberOfTypes` parent chain. A group is an entity with children;
  `principal in Group::"x"` tests membership transitively. Works for principals (users→groups),
  resources (files→folders), AND actions (viewPhoto ∈ readOnly).

### 2d. RBAC-via-attributes (roles = principal attributes, NOT policy structure)
This is the key modeling move for our PBAC direction (roles = principal attributes only):
- **Model a role as a parent entity in the hierarchy**, then scope the permit to it:
```cedar
permit ( principal in Role::"ContractManager",
         action in [Action::"reviewContract", Action::"executeContract"],
         resource );
```
- Membership (who is a ContractManager) is **managed as entity data, independent of the policy**. Adding
  a user to a role is a data write, not a policy edit. The engine **indexes policies by the group** and
  skips them for principals not in that group (fast slicing). Best-practice doc: put the role/group in
  the **scope**, not in a `when` clause, so it's indexable.
- ABAC = add `when { principal.department == resource.department }`. RBAC + ABAC compose in one policy set.

### 2e. Policy templates
Sources: [templates](https://docs.cedarpolicy.com/policies/templates.html), Context7 json-format.
- **Template** = policy with named slots `?principal` / `?resource` (only these two; slots allowed only
  in scope, not conditions). Like a SQL prepared statement.
- A **template-linked policy** binds concrete entities to the slots at runtime. **Editing the template
  updates all linked policies.** Use for ad-hoc sharing / discretionary grants ("share ticket X with
  user Y") — link a template instead of writing N near-identical policies.
```cedar
permit ( principal == ?principal, action in Action::"Shared_TicketAccess", resource == ?resource )
when { resource.status == "OPEN" };
```

### 2f. Partial evaluation — the killer feature for screens/rows/filtering
Sources: [Cedarland partial-eval guide](https://cedarland.blog/usage/partial-evaluation/content.html),
[cedar_policy Rust docs](https://docs.rs/cedar-policy).
- `is_authorized_partial` (vs `is_authorized`) can return Allow, Deny, **or a residual**: the set of
  policies it couldn't fully evaluate because some input was left **`unknown()`**.
- **Residuals = constraints describing the principal's viewable set.** Leave `resource` unknown → the
  residual is "the filter for every resource this principal may access."
- **You can translate a residual into a SQL WHERE clause** and push it to the DB — the engine has already
  resolved everything the DB can't answer (e.g. "was the request MFA-authenticated"). This is how you do
  **row-level filtering / list endpoints** without evaluating per-row in a loop.
- **Maps to our granularities:**
  - **Screen/nav**: `is_authorized(user, Action::"viewScreen::Foo", App::"console")` → deny-by-omission
    hides the nav entry.
  - **Row**: partial-eval with unknown resource → residual → SQL filter on the list query.
  - **Field**: one action per sensitive field (`Action::"viewField::salary"`) or property-policy style;
    fail → null the field (mirror Foundry §1a).
  - **Aggregation**: authorize the aggregation action AND ensure the underlying row filter (residual) is
    applied before aggregating, so counts/sums never leak forbidden rows.

**Cedar best practices to enforce in CI:** validate every policy against the schema (catches typos/type
errors pre-deploy); put roles/groups in scope (indexable) not conditions; use `forbid` for guardrails
(tenant isolation, holds) since forbid always wins; keep entity IDs stable/unique; deny-by-omission is
the default — never write a catch-all permit.

---

## 3. Configurable-console architecture — config-as-governed-data

Every low-code platform independently arrived at the same shape. Steal the shape, not any one product.

### 3a. Component model + reactive data-binding
Sources: [Retool components](https://docs.retool.com/apps/web/guides/components) +
[transformers](https://docs.retool.com/queries/guides/transformers),
[Appsmith writing-code](https://docs.appsmith.com/core-concepts/writing-code),
[ToolJet control-components](https://docs.tooljet.com/docs/app-builder/custom-code/control-components/),
[Budibase data-provider](https://docs.budibase.com/docs/data-provider),
[Windmill app_editor](https://www.windmill.dev/docs/apps/app_editor).
- **Universal pattern**: every component/query/variable is a **globally-named object in a flat
  namespace**; UI props bind via a `{{ }}` expression **evaluated as JS**. The set of `{{ }}` references
  forms a **dependency graph that auto-reruns on input change** (Retool, Appsmith, ToolJet, Budibase).
- **Keep the graph acyclic**: reads are pure reactive bindings (Retool transformers can only `return`,
  never mutate); **writes are quarantined to explicit imperative actions** (`setValue`/`setIn`,
  ToolJet CSAs). Adopt this read/write split.
- **Two data-flow shapes**: standalone query objects components pull from by reference (Retool/Appsmith/
  ToolJet — composable) vs a Data-Provider pushing context down the tree (Budibase — simple but couples
  fetch to position). For a governed console, prefer decoupled query-objects + named exposed variables.
- **Fix a small set of binding namespaces** — Windmill's Context / State / Component-outputs /
  Background is the cleanest articulation. Define these explicitly, not a free-for-all global.

### 3b. Per-screen config stored AS DATA (JSON documents, not code)
Sources: [Retool import-export](https://docs.retool.com/apps/guides/app-management/import-export),
[Appsmith JSON DSL](https://www.appsmith.com/blog/introducing-json-forms-in-appsmith),
[Windmill app_editor](https://www.windmill.dev/docs/apps/app_editor).
- **Every platform stores the app as a serializable config document built client-side** (Retool
  JSON/Toolscript ZIP, Appsmith JSON DSL, Windmill declarative tree, Budibase CouchDB docs).
- **Split config per screen/page to survive concurrent editing** — Retool deliberately stores component
  positions in per-page `.positions.json` **to prevent merge conflicts**. One document per screen, not
  one monolith. (Directly relevant given our unprotected-main + parallel-lane reality.)

### 3c. Field/property schema — the discriminated-union model
Sources: [Airtable field model](https://airtable.com/developers/web/api/field-model),
[Notion property object](https://developers.notion.com/reference/property-object).
Airtable and Notion converged on a near-identical model — this is THE field-schema pattern:
- Every field = `{ id, name, type, config }` where **`type` is a string tag and `config` is
  type-specific**. ~35 types. **New field types ship without migrations.**
- **Options/choices are IDed sub-entities** `{id, name, color}`, referenced by id not label → rename-safe.
- **Computed fields reference other fields by id and nest a result-type** (Airtable `result`, Notion
  rollup) → computed types compose recursively.
- **Forward-compat is a hard requirement**: Airtable explicitly states clients must gracefully handle
  unknown `type`. Build the reader to skip/degrade, never crash.
- **Schema-value mirroring**: a record/page's stored values mirror the schema shape (value type derived
  from property type). One source of truth.

### 3d. Views = saved, named, per-view query config
Sources: [Airtable view config](https://support.airtable.com/docs/view-configuration-options),
[Notion working-with-views](https://developers.notion.com/guides/data-apis/working-with-views).
- A view = serializable bundle of `{visible fields + order, filters, sorts, grouping, layout extras}`
  bound to one table, **fully decoupled from data**. Hiding a field is view-scoped, never destructive.
- **Reuse one filter/sort AST at both view-level and ad-hoc query-level** (Notion: a view IS a saved
  query). Define the filter/sort schema once.
- **Per-view ownership/lock**: Collaborative / Personal (per-user) / Locked. Plan this into the view
  object from day one.

### 3e. Governance / versioning of config (draft → approve → released, rollback, environments)
Sources: [ToolJet version-control](https://docs.tooljet.com/docs/development-lifecycle/release/version-control/) +
[multi-environment](https://docs.tooljet.com/docs/release-management/multi-environment/),
[Windmill draft_and_deploy](https://www.windmill.dev/docs/core_concepts/draft_and_deploy) +
[roles/permissions](https://www.windmill.dev/docs/core_concepts/roles_and_permissions),
[Retool source-control](https://docs.retool.com/build/apps/guides/source-control),
[ServiceNow Data Policy](https://www.servicenow.com/community/servicenow-ai-platform-articles/difference-between-data-policy-and-ui-policy/ta-p/2313599).
Two dominant patterns for turning config into a governed, releasable artifact:
1. **Immutable-version + promote (ToolJet, Windmill)** — recommended for us. Draft (editable) →
   **Saved/Deployed = immutable checkpoint** (Windmill content-addresses by hash); only immutable
   versions promote; sequential Dev→Staging→Prod; **version artifact is env-agnostic, connection config
   is env-bound.** Cleanest for audit + rollback (restore any past immutable version).
2. **Git-PR + separate release gate (Retool, Appsmith)** — config history in git via PRs, but
   **Releases are deliberately separate from git** (git = history, Release Management = deploy gate).
- **Drafts are per-user and invisible to the workspace** (Windmill, ToolJet) — multiple editors draft
  the same item conflict-free; only deploy makes it authoritative. Replicate to avoid clobbering.
- **Environment separation** (unanimous): keep the app/version definition env-agnostic; a **single
  resource/datasource object carries per-environment credentials**. Never fork the app per env.
- **Content-address deploys**: immutable hash per deploy + a mutable path pointer to latest (Windmill).
  Free rollback + tamper-evidence. Ties directly to our audit-chain work.
- **Decouple author from runtime identity**: `permissioned_as` ≠ `created_by` (Windmill) — config runs
  as a defined principal, not whoever last edited it. **Directly relevant to Cedar/PBAC**: the config
  object carries its own effective-principal, evaluated at execution → no privilege escalation via shared config.
- **Publish ≠ data migration**: Budibase couples app-publish with internal-data replication (can
  overwrite prod data). **Keep config-promotion and data-migration as separate pipelines.**

### 3f. Per-form field rules as data (ServiceNow — the declarative-rule benchmark)
Sources: [ServiceNow UI Policy](https://www.servicenow.com/docs/bundle/zurich-platform-administration/page/administer/form-administration/task/t_CreateAUIPolicy.html),
[race UI-policy vs client-script](https://www.servicenow.com/community/developer-blog/a-race-between-ui-policy-and-client-script/ba-p/2292668).
- **UI Policy = a record, not code**: `sys_ui_policy` (conditions as data + table + Order + on-load /
  reverse-if-false flags). Field rules live in child records (`sys_ui_policy_action`), one per field,
  each tri-state **Mandatory / Visible / Read-Only ∈ {True, False, Leave-alone}.**
- **Tri-state "leave alone" is what lets multiple rule-sets compose non-destructively.** Integer `Order`
  sequences policies (ascending, last-writer-wins on shared fields). **Reverse-if-false** auto-reverts
  actions when the condition goes false (declarative two-way binding).
- **Server-side twin** (`sys_data_policy2`, Data Policy): enforce the expressible subset (mandatory /
  read-only) **on every write path** (form/import/API), sharing the same condition-record shape.
  **Never trust client-only config enforcement — mirror it server-side over the same data model.**

### 3g. "Add-anything" UX without a deploy
- **Inline schema editing** (Airtable/Notion): adding a field is a data insert, not a code change.
- **Autogenerated screens from schema** (Budibase): read a table schema → generate CRUD UI → new table
  yields a working screen with zero build.
- Because layout, fields, views, and rules are all **data documents**, "add a screen/field/view/rule" is
  a governed write to the config store, flowing through the same draft→release pipeline — never a redeploy.

### 3h. Making config a GOVERNED OBJECT (summary checklist)
Immutable hash per deploy + mutable "latest" pointer · per-user invisible drafts · draft→review→
release gate · env-agnostic artifact + env-bound credentials · content-addressed rollback · config
carries its own effective-principal (Cedar) · hierarchical RBAC with a small fixed verb set
(Own/Edit/Use/None) attached to the config object's path/scope · **server-side enforcement of every
client-declared rule** · interpolation as an injection boundary (Windmill's locked `$var:`/`$res:`
grammar — no arbitrary eval at the secret boundary).

---

## 4. Per-module best-in-class (dense)

Each: (a) core model that makes it best-in-class, (b) one reusable takeaway.

- **Workday — Business Process framework + effective-dating.** (a) Every HR action (hire, absence, time,
  staffing) is an *event instance* run through a configurable BP definition: ordered steps
  (approval/to-do/checklist/integration/notification), each gated by condition rules + routing modifiers,
  ending in a mandatory **completion/commit step**; all records **effective-dated** (time-sliced versions
  keyed by effective date). (b) Separate transaction data from the process that commits it: BP =
  `definition → steps[] → (condition, action, routing)` + one commit step; store every entity as
  append-only effective-dated rows (validity interval), not in-place updates → audit + retro edits +
  future-dating for free.
  [datasheet](https://www.workday.com/content/dam/web/en-us/documents/datasheets/workday-business-process-framework.pdf)

- **Recruiting — Greenhouse (structured hiring).** (a) Separates **Candidate (person)** from
  **Application (candidacy for one Job/Req)**; each Application flows an ordered **Stage** pipeline; each
  stage has an **Interview Kit**; each interviewer files a **Scorecard** rating predefined **attributes**
  on a fixed rubric + overall rec. Mandatory structured scorecards make the hiring record defensible.
  (b) Schema: `Candidate 1─* Application *─1 Job`; `Job 1─* Stage(ordered) 1─1 InterviewKit`;
  `Application *─* Interviewer via Scorecard`. **Source-tracking lives on the Application, not the Candidate.**
  [scorecard overview](https://support.greenhouse.io/hc/en-us/articles/4414777492891-Scorecard-overview)

- **Automation execution — Temporal (durable event history).** (a) A workflow's whole lifecycle is a
  **durable append-only Event History**; worker code emits **Commands** (intent) → service maps each to
  an **Event** (durable outcome). Recovery = **deterministic replay** feeding recorded events back;
  already-recorded Activities are **not re-executed** (result replayed) → effectively-once side effects.
  (b) Log **events, not snapshots**; event = `{eventId monotonic, eventType, attributes}`; rebuild state
  by folding the log; make code deterministic (replay/history mismatch = bug); expose timeline/full views
  over one log. [event history](https://docs.temporal.io/encyclopedia/event-history)

- **Messaging — Slack (threads + Events API unfurl).** (a) Message keyed by immutable **`ts`** within a
  channel; a **thread** = any message with **`thread_ts`** = parent's `ts` (flat, one-level). Unfurl is
  event-driven: `link_shared` delivers **metadata only** (ts, channel, matched links, thread_ts) — never
  body; app replies via `chat.unfurl` with a per-URL block map (`links:write` scope). (b) Model message
  PK `(channel_id, ts)` + nullable `thread_ts` self-FK; rich previews = register-domains → metadata-only
  event → resolve → post-back (never trust event to carry content); cap registered domains to bound fan-out.
  [link_shared](https://docs.slack.dev/reference/events/link_shared/) ·
  [chat.unfurl](https://docs.slack.dev/reference/methods/chat.unfurl/)

- **Gmail — conversation threading + labels.** (a) A message threads only when all three hold:
  **`In-Reply-To`** (direct parent) + **`References`** (full ancestry) headers link prior messages,
  **`Subject`** matches (ignoring `Re:`-type prefixes), and same `threadId`. Identical subject+sender
  alone will NOT thread without an explicit `References`/`In-Reply-To`. **Labels ≠ folders**: labels are
  many-to-many tags on a message. (b) Thread by header-chain first, normalized-subject + participants
  only as tiebreak; store `Message *─* Label` junction, derive a thread's labels as the union of members'.
  [Gmail threads](https://developers.google.com/workspace/gmail/api/guides/threads)

- **SAP — 3-way match, GL, PM work orders.** (a) **MM 3-way match**: PO ↔ GR ↔ Invoice reconciled via a
  **GR/IR clearing account** — GR posts `dr inventory / cr GR-IR`, invoice posts `dr GR-IR / cr AP`,
  netting to zero only when qty+price+product agree within **tolerance** (else invoice blocked). **FI GL**:
  every posting is a **balanced document** of line items where Σdebit = Σcredit. **PM work order**: status
  machine `CRTD → REL (can book cost) → TECO (no further cost) → CLSD`, forward-only. (b) Use a
  **clearing/suspense account** as the reconciliation join (3-way match = "does the clearing line net to
  zero within tolerance"); model every financial mutation as balanced header + line-items (Σdr=Σcr
  invariant); drive work orders through an explicit status profile with cost-posting gates.
  [3-way match](https://ramp.com/blog/sap-3-way-match)

- **Compliance — Vanta / Drata (control → evidence → continuous test).** (a) `Control → Test → Evidence`:
  each control backed by automated **tests** pulling **evidence** from connected systems on a schedule
  (hourly/daily); a **cross-framework control mapping** lets one evidence item satisfy overlapping
  requirements (SOC2 ∩ ISO 27001). (b) Model `Control *─* Test`, `Test → (integration, query, schedule)
  → Evidence(timestamped, retained)`, `Requirement *─* Control`; continuous = re-run on cron + diff.
  [Vanta evidence](https://www.vanta.com/resources/automated-evidence-collection-for-compliance-all-you-need-to-know) ·
  [Drata monitoring](https://drata.com/products/compliance/monitoring-and-tests)

- **Evidence/records standards — ISO 15489 / OAIS (ISO 14721) / FRE 902(13-14) / WORM.** (a)
  **ISO 15489**: records must be **authentic, reliable, integral (unaltered), usable**, with metadata +
  retention/disposition schedules + audit trails. **OAIS**: six functions (Ingest, Archival Storage, Data
  Mgmt, Administration, Preservation Planning, Access); package as **SIP→AIP→DIP** where the **AIP** bundles
  content + **PDI** (provenance, context, reference, **fixity**). **FRE 902(13)/(14)**: electronic records
  are **self-authenticating** via **certification by a qualified person** — 902(14) endorses proving a copy
  identical via **matching hash values**. **WORM**: write-once immutable retention storage under all of it.
  (b) On ingest: compute + persist a **SHA-256 fixity hash** per object, wrap with provenance/PDI into an
  immutable AIP-style package, write to **WORM/object-lock** for the retention window, keep a certification
  record — one design satisfies ISO 15489 integrity + OAIS fixity + FRE 902(14) hash self-authentication +
  WORM. (Directly informs our L20 audit-chain.)
  [FRE 902](https://www.law.cornell.edu/rules/fre/rule_902) ·
  [ISO 14721](https://www.iso.org/standard/57284.html)

---

## 5. How it all maps to our console (the synthesis)

1. **One ontology, projected over Postgres tables** (object type = typed view + PK + title key), not a
   second store. Every consumer (screens, automations, API) reads/writes through it. (§1)
2. **All mutations are declarative named Actions → writeback/event rows**, never direct UPDATEs; state is
   folded from an append-only, effective-dated log (Workday/Temporal/SAP/OAIS all agree). (§0, §1a, §4)
3. **Cedar is the single authz surface** for screen / row / field / aggregation: deny-by-default,
   `forbid` for tenant-isolation guardrails (always wins), roles = principal attributes in scope (indexable),
   partial-eval residuals → SQL WHERE for row-level list filtering. Validate every policy against schema in
   CI. (§2) — aligns with existing Cedar-activation + tenant-isolation gate work.
4. **Screens/apps/automations/views/field-rules are governed config DATA** — one JSON doc per screen,
   discriminated-union field schema (forward-compat on unknown types), views = saved filter/sort ASTs,
   field rules = ServiceNow-style tri-state records enforced BOTH client and server. (§3)
5. **Config is an immutable-versioned governed object**: per-user drafts → review → released checkpoint
   (content-addressed hash) → env-agnostic artifact + env-bound creds → content-addressed rollback →
   config carries its own effective-principal for Cedar. Schema changes go branch+proposal+merge-check+
   changelog like Foundry Ontology Manager. (§1b, §3e/h)
6. **Audit/evidence = fixity-hashed, WORM, certifiable** (ISO 15489 / OAIS / FRE 902(14)) — the L20 chain.

**Doc gaps flagged during research** (verify against a live instance if you depend on them): Appsmith's
exact git repo layout + DB JSON schema; Retool's internal stored app schema (only export formats
documented); Windmill git_sync page 404'd (mechanics via CLI `sync`); some ServiceNow tri-state precedence
details lean on community articles; several ToolJet `.ai`/`tooljet-concepts` URLs 404 (canonical paths under
`/docs/development-lifecycle/` + `/docs/release-management/`).
