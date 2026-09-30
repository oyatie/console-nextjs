# BE Ontology + Governance Engine — Architecture (spec for N build lanes)

Status: design, no code. Author lane: BE-1. Branch: `feat/cedar-activation`.
Grounds in `.omc/research/{benchmark-brief,backend-survey}.md` + `console-design-program.md`.
This is the buildable spec the ontology/lifecycle/guardrails/Cedar-authoring lanes implement.

## 0. What already exists (build ON, do not fork)

| Asset | Location | Reuse as |
|---|---|---|
| Cedar compile+eval engine (fail-closed strict-validate, panic-isolated) | `crates/platform/authz/src/cedar_pbac/engine.rs` | THE evaluator; add authoring + partial-eval, don't rewrite |
| Cedar coexistence boundary + `SubjectFreshness` + engine-mode ladder | `crates/platform/authz/src/cedar_pbac.rs` | the enforce-mode state machine (`legacy_only`→`cedar_shadow_legacy_enforce`→`cedar_enforce_legacy_compare`→`cedar_only`) |
| Cedar staging tables (catalog + no-code drafts) | migration `0103_create_cedar_policy_staging.sql` | Policy Studio substrate; extend with object/property-policy + simulate log |
| `with_audit(pool, event, closure)` — mutation + audit row in ONE tx | `crates/platform/db/src/audit_tx.rs` | every writeback wraps in this; atomicity is non-negotiable |
| RLS arming: `set_config('app.current_org',$1,true)` transaction-local GUC | `audit_tx.rs` + `console_platform_request_context::with_request_context` | every ontology table FORCE-RLS `org_isolation` reads this; unset ⇒ fail-closed |
| `audit_events` append-only (actor/action/target/before_snap/after_snap/trace) + immutable triggers + REVOKE UPDATE/DELETE | migration `0003` | the audit trail; ontology writes land here via `with_audit` |
| Tamper-evident hash-chain (L20) + canonicalization + seal/verify | `crates/compliance/integrity/{domain,store,rest}.rs` | REUSE its canonical-hash helper for instance-revision fixity — do NOT re-implement sha256 chaining |
| WORM object-lock storage (retention, `checksum_sha256`, object-lock buckets) | `crates/platform/storage/src/lib.rs` | evidence/attachment fixity for action side-effects |
| Equipment-scoped proto-ontology: `object-actions/{catalog,execute}`, `equipment/{id}/timeline-graph`, step-up gate | `crates/registry/rest/src/lib.rs` | GENERALIZE this into the typed action + traversal surface (it is the seed, already step-up-gated) |
| Hexagonal crate template | `crates/financial/{domain,application,adapter-postgres,rest}`, `crates/compliance/*` | copy the split for every new crate; each `rest` crate exposes `router(state)` self-applying `with_request_context` |
| Workflow lifecycle + revisions + effective-dating precedent | `crates/workflow/{domain,runtime,adapter-postgres}` | the lifecycle-FSM/revision-staging pattern to mirror, not re-invent |

Migrations live in `crates/platform/db/migrations/00NN_*.sql`; highest today is **0104**. Numbers
collide across parallel lanes — **reserve the next free integer right before push, not at author time.**

---

## 1. Substrate layer — append-only, effective-dated, fixity-stamped

The one primitive under everything (benchmark §0): current state is a **fold over immutable records**,
never an in-place mutate. Two storage shapes, chosen by whether the object type projects an existing
domain table:

### 1a. Projected types (WO / employee / equipment / …) — no new store
The domain table + its existing `audit_events` (before_snap/after_snap, written by `with_audit`) ARE the
event log. The ontology layer holds only **metadata** (the projection map) and reads/writes **through the
domain crate's existing use-cases** — which already carry RLS + audit + authz. The ontology never owns
projected storage.
- As-of reconstruction for projected types in v1 = fold `audit_events.before/after_snap` for the target.
  Limited by snapshot coverage. **Decision to flag:** full bi-temporal as-of for projected types is
  deferred; v1 gives current-state + audit-derived history. Adequate for the console; note the ceiling.

### 1b. Generic instance types (user-authored `OT-…`) — owned effective-dated store
For object types with no backing domain table, one append-only revision log; current state is the newest
revision. **JSONB attribute bag validated against the property schema — NOT EAV.**

```
ont_instances            (id, org_id, object_type_id, title, current_revision_id,
                          lifecycle_state, created_at)          -- one row per object, pointer to head
ont_instance_revisions   (id, org_id, instance_id, version BIGINT,   -- v+1, append-only
                          attributes JSONB,                     -- validated vs property schema
                          valid_from TIMESTAMPTZ, valid_to TIMESTAMPTZ NULL,  -- effective-dating
                          action_type_id, actor, reason,
                          prev_hash CHAR(64), row_hash CHAR(64), -- fixity chain per (org,instance)
                          created_at)
ont_links                (id, org_id, link_type_id, from_instance_id, to_instance_id,
                          valid_from, valid_to NULL, created_at) -- effective-dated edges
```
- **Current state** = revision where `valid_to IS NULL`. **As-of(t)** = revision where
  `valid_from <= t < coalesce(valid_to, ∞)`. Future-dating allowed (valid_from > now).
- **Fixity:** `row_hash = canonical_hash(prev_hash ‖ canonical(revision))` using the L20 integrity
  crate's canonicalizer. Per-(org,instance) chain → tamper-evident instance history for free.
- GIN index on `attributes` for property filters; `(org_id,object_type_id,lifecycle_state)` btree for lists.

`ponytail:` JSONB attribute bag over EAV — one row per revision, schema validates shape. Add a
column-per-property projection view only if a hot query measurably needs it.

---

## 2. §18 Ontology registry (backbone)

Foundry model: object type = schema; property = column; link type = relationship; action type = the
verb that mutates; analytic = derived property. The registry is **config-as-governed-data** (its own
schema lifecycle, §3). All tables FORCE-RLS org-isolated.

```
ont_object_types   (id, org_id, stable_key, title, title_property_key,
                    backing_kind TEXT CHECK IN ('projected','instance'),
                    backing_table TEXT NULL,           -- projected: domain table name (allowlisted)
                    primary_key_property TEXT NULL,    -- projected: PK column
                    schema_version BIGINT, lifecycle_state, created_by, ...)
ont_property_defs  (id, org_id, object_type_id, key, title,
                    type TEXT,                          -- discriminated union tag (§3c benchmark)
                    config JSONB,                       -- type-specific (choices as {id,name,color})
                    backing_column TEXT NULL,           -- projected: source column
                    required BOOL, in_property_policy BOOL)  -- ≤1 property policy per prop (Foundry)
ont_link_types     (id, org_id, stable_key, title, from_object_type_id, to_object_type_id,
                    cardinality TEXT CHECK IN ('one_one','one_many','many_many'), traversable BOOL)
ont_action_types   (id, org_id, object_type_id, stable_key, title,
                    params_schema JSONB,                -- typed inputs (defaults, object-set dropdowns)
                    edits JSONB,                        -- declarative property writes
                    submission_criteria JSONB,          -- validation gating submit
                    side_effects JSONB,                 -- notify/webhook/attachment (WORM)
                    dispatch TEXT CHECK IN ('projected_usecase','instance_revision'),
                    dispatch_target TEXT NULL,          -- projected: which domain use-case
                    control_points JSONB)               -- §16 gate config (see §4)
ont_analytics      (id, org_id, object_type_id, key, title,
                    formula JSONB, result_type JSONB)   -- derived property; references props by id
```

- **Field schema = discriminated union** (`{id,name,type,config}`, ~35 types, choices IDed sub-entities):
  new field types ship with zero migration; reader degrades on unknown `type`, never crashes (benchmark §3c).
- **Property-value mirroring:** an instance's `attributes` JSONB mirrors the property schema shape.
- **Graph traversal** (Object Explorer search-around): `GET /ontology/instances/{id}/traverse?link_type=&depth=`
  walks `ont_links` (instance types) or resolves FK columns (projected types) into a node/edge payload —
  generalizes the existing equipment `timeline-graph`.

### REST — Ontology Manager surface
```
GET    /api/v1/ontology/object-types                 list (Cedar-filtered)
POST   /api/v1/ontology/object-types                 create draft schema
GET    /api/v1/ontology/object-types/{key}           def + property/link/action/analytic children
PUT    /api/v1/ontology/object-types/{key}           stage revision (v+1)  → §3 governance
GET    /api/v1/ontology/link-types | action-types | analytics   (CRUD, draft-staged)
GET    /api/v1/ontology/instances?type=&filter=&as_of=          list (RLS ∧ Cedar residual, §5)
POST   /api/v1/ontology/instances                    create (via an action type)
GET    /api/v1/ontology/instances/{id}?as_of=        object card (field policy nulls forbidden props)
GET    /api/v1/ontology/instances/{id}/traverse      search-around graph
GET    /api/v1/ontology/instances/{id}/history       revision timeline (hash-verified)
POST   /api/v1/ontology/actions/{action_key}/preflight   §16 gates status (which pending)
POST   /api/v1/ontology/actions/{action_key}/execute     writeback (humans + automation, same path)
```

---

## 3. §20 / §15 lifecycle + CRUD governance

Two distinct lifecycles; both append-only, no hard delete, before-value audited.

### 3a. Schema lifecycle (object/link/action/analytic defs) — Foundry Ontology-Manager proposals
`draft → review_pending → published(immutable, schema_version+1) → superseded → retired`.
- Direct-to-published forbidden once **protection** is on: edits go through draft → proposal →
  merge-check (conflict detect) → reviewer-approval (four-eyes) → publish. Changelog = per-user/timestamp.
- Published schema versions are **immutable, content-addressed** (digest per version) → free rollback +
  as-of schema. Mirrors the Cedar catalog `status` ladder already in `0103`.

### 3b. Instance lifecycle + CRUD governance (§15)
`draft → active → (locked?) → archived → disposed`. Per-object-type configurable FSM (reuse workflow-crate
transition validation).
- **draft-direct vs post-draft override:** editing a non-draft (active/locked) requires
  `{reason, four-eyes approval, before-value snapshot}` — enforced in the execute path, recorded to
  `audit_events` (before_snap already native) + a governance override row.
- **Revision staging:** a change stages a v+1 revision with `valid_from` (effective-dating), does not
  mutate the live row until effective.
- **Impact preflight:** before archive/schema-change, `POST …/preflight` returns dependent links,
  instances, referencing policies/workflows. Fail if `on_delete=restrict` dependents exist.
- **Soft-archive/dispose gates:** archive is reversible; dispose is terminal, four-eyes + retention-check
  gated, and (for evidence-bearing objects) writes a WORM tombstone. **No hard DELETE anywhere.**

```
gov_lifecycle_transitions (id, org_id, object_type_id, from_state, to_state,
                           requires_reason BOOL, requires_four_eyes BOOL, requires_checklist BOOL)
gov_overrides             (id, org_id, target_type, target_id, actor, reason,
                           before_snapshot JSONB, approved_by, approved_at, created_at)
gov_approvals             (id, org_id, request_ref, kind, requested_by, approver_id,   -- four-eyes
                           decision TEXT CHECK IN ('pending','approved','rejected'),
                           CHECK (approver_id <> requested_by), decided_at)             -- self-approval blocked
```

### REST
```
POST /api/v1/governance/overrides            open an override (reason + before-snap)
POST /api/v1/governance/approvals/{id}/decide  four-eyes decision (approver ≠ requester)
POST /api/v1/ontology/instances/{id}/lifecycle  transition (validated vs gov_lifecycle_transitions)
POST /api/v1/ontology/instances/{id}/impact     dependency preflight
```

---

## 4. §16 guardrails engine — per-action preflight control-point, fail-closed

Every action execution passes an ordered gate chain BEFORE any writeback commits; any gate not
satisfied ⇒ **deny, nothing written** (fail-closed). Gate config lives on `ont_action_types.control_points`.

Fixed gate order (each optional per action):
1. **Authority** — Cedar `authorize(subject, Action::"<action_key>", resource)` (§5). Deny ⇒ stop.
2. **Self-checklist** — required acknowledgements (`Checklist` object) all checked.
3. **Four-eyes approval** — a `gov_approvals` row in `approved` by a distinct principal.
4. **Egress/DLP** — outbound side-effects (webhook/attachment/export) pass the egress classifier
   (= §13 layer-2). Deny ⇒ stop.

- `preflight` returns each gate's status so the UI shows what's pending, **without** committing.
- `execute` **re-evaluates every gate inside the writeback transaction** (TOCTOU-safe) then dispatches:
  projected ⇒ domain use-case; instance ⇒ append revision. All within `with_audit` (mutation + audit atomic).
- The same execute path serves human clicks and Automate rules — one mutation surface (Foundry principle).

---

## 5. Cedar authoring + evaluation REST (Policy Studio spine)

Build on `engine.rs` (compile/validate/evaluate already done, fail-closed, panic-isolated). Add:

### 5a. Authoring / catalog CRUD (extends `0103` tables)
```
GET    /api/v1/policy/catalog                list catalog entries (enforced/shadow/draft/…)
POST   /api/v1/policy/drafts                 no-code draft (blocks → normalized_row → generated Cedar text)
PUT    /api/v1/policy/drafts/{id}            edit (per-user invisible draft; benchmark §3e)
POST   /api/v1/policy/drafts/{id}/validate   strict-validate vs schema (returns errors, no activation)
POST   /api/v1/policy/drafts/{id}/submit     → review_pending (validation must be 'valid')
POST   /api/v1/policy/drafts/{id}/review     four-eyes approve/reject → promotion-eligible
```
- Draft saves **cannot** create shadow/enforced rows (already enforced by `0103` CHECKs) — promotion is a
  separate, gated lane. Roles stay principal attributes (membership = data, loaded server-side into the
  Cedar Subject); adding a user to a role is a data write, never a policy edit.

### 5b. Object-policy (row) + property-policy (field)
Add two policy scopes so one policy set does row + field authz (Foundry object/property policies):
```
ont_object_policies   (id, org_id, object_type_id, cedar_policy_id, effect)  -- row: deny ⇒ instance hidden
ont_property_policies  (id, org_id, property_def_id, cedar_policy_id)        -- field: deny ⇒ value null
```
`forbid` used for tenant-isolation + legal-hold guardrails (forbid always wins, can't be out-permitted).

### 5c. Simulate / can / authorize
```
POST /api/v1/policy/simulate     {subject, action, resource, context} → Allow/Deny + matched policies + diagnostics
POST /api/v1/policy/authorize    live decision (same evaluator the guardrail gate calls)
```
Simulate reuses `evaluate()`; deny-by-omission is the default (never a catch-all permit).

### 5d. Partial-eval residual → SQL WHERE (the row-filter killer feature)
For list endpoints: call Cedar `is_authorized_partial` with **`resource` unknown** → residual =
"the filter for every resource this principal may see." Translate residual → SQL `WHERE`, push to DB —
no per-row loop.
- Projected type: residual attrs → real columns. Instance type: residual attrs → `attributes->>'key'`.
- **Composition with RLS:** final list query is `WHERE <RLS org_isolation> AND <Cedar residual>`. RLS is
  the hard tenant floor (can't be widened); the residual is the discretionary deny-by-omission filter.
- **Restricted residual grammar** (v1): `attr <op> literal`, `AND`/`OR`, membership `in`. Anything the
  translator can't lower (function calls, unknown-principal terms) ⇒ fall back to **deny** (fail-closed),
  never to a silent allow. `ponytail:` narrow grammar first; widen only when a real policy needs it.

**RISK — Cedar partial-eval is an experimental feature** in `cedar-policy` (`is_authorized_partial`,
`PartialResponse`, `unknown()`), gated behind a crate feature flag. **Human decision needed:** enable the
experimental feature on the pinned `=4.11.2`, or (fallback) evaluate the residual ourselves by lowering
our own catalog `conditions` JSONB (we author the no-code grammar, so we control it) to SQL directly —
which sidesteps the experimental API entirely and may be the lazier, more stable path. Recommend
prototyping both on RoleManage before committing.

---

## 6. How RLS + Cedar + audit-chain compose (one request)

`list instances` (representative):
1. Middleware arms `app.current_org` (`with_request_context`) — RLS now scoped; unset ⇒ zero rows.
2. Cedar partial-eval(resource=unknown) → residual → SQL WHERE.
3. Query = `SELECT … WHERE <RLS auto> AND <residual> AND <lifecycle active>`. Property policies null
   forbidden fields in projection.
4. No mutation ⇒ no audit row (reads aren't audited unless a covert-stream policy says so).

`execute action` (mutation):
1. Arm org. 2. §16 gate chain (gate 1 = Cedar authorize). 3. `with_audit` tx: re-check gates →
   dispatch (projected use-case | append instance revision with hash chain) → INSERT `audit_events`
   (before/after) → COMMIT. 4. Side-effects (WORM attachment, notify) after commit, idempotent.

Invariant: **tenant isolation (RLS) and authority (Cedar) are independent layers that both must pass** —
neither can widen the other. Audit + fixity are written in the same tx as the mutation or not at all.

---

## 7. Crate layout

New crates (hexagonal `{domain,application,adapter-postgres,rest}`, copy `crates/financial/*`):

| Crate | BE lane | Owns |
|---|---|---|
| `crates/ontology/*` | BE-1 | registry (object/link/action/analytic), instance store + revisions + fixity, traversal, action execute/preflight dispatch |
| `crates/governance/*` | BE-2 + BE-3 | lifecycle FSM, override/four-eyes/impact (§15), guardrail control-point chain (§16) |
| `crates/platform/authz` (extend) + `crates/platform/authz-rest` (new) | BE-4 | Cedar authoring CRUD, simulate/authorize, object/property-policy, partial-eval residual→SQL. New modules `cedar_pbac/authoring.rs`, `cedar_pbac/residual.rs`; thin rest crate |

Each `rest` crate exposes `router(state)` self-applying `with_request_context`, merged in `app/src/lib.rs`
`build_router` alongside the existing domain routers. `ontology` depends on `governance` (gate chain) +
`platform/authz` (authorize) + domain crates (projected dispatch, via their application traits — not their
tables). `governance` depends on `platform/authz` only. Keep the dep DAG acyclic; the layer-boundary CI
gate (`ci/gates/layer-boundary`) enforces it.

---

## 8. Build sub-lanes (disjoint, parallel)

Reserve migration numbers at push time. Each lane owns disjoint crates + migrations.

1. **L-ONT-registry** (`crates/ontology/{domain,adapter-postgres}`, mig: object/link/action/analytic +
   property_defs) — registry CRUD + schema lifecycle draft-staging. No instance store yet.
2. **L-ONT-instances** (`ontology/adapter-postgres`, mig: `ont_instances/_revisions/_links`) — effective-
   dated JSONB store + fixity chain + as-of + traversal. Depends on registry types.
3. **L-ONT-actions** (`ontology/application` + `rest`) — action execute/preflight, dispatch to projected
   use-case vs instance revision; generalizes registry `object-actions`. Depends on 1,2 + governance gate.
4. **L-GOV** (`crates/governance/*`, mig: `gov_lifecycle_transitions/_overrides/_approvals`) — FSM,
   four-eyes, override, impact preflight, §16 gate chain. Independent; L-ONT-actions consumes it.
5. **L-CEDAR-authoring** (`platform/authz` + `authz-rest`, mig: extend `0103` + object/property policy) —
   catalog/draft CRUD, validate, simulate/authorize REST. Independent of ontology.
6. **L-CEDAR-residual** (`platform/authz/residual.rs`) — partial-eval (or JSONB-condition lowering)
   → SQL WHERE; consumed by L-ONT-instances list. Prototype-gated (§5d risk).
7. **L-WIRE** (serial, last) — merge routers in `build_router`, openapi.yaml + regenerate clients
   (ts/kotlin/swift — every op needs per-domain `tags:`, or Kotlin client OOMs), full CI gate.

Parallelizable now: {1,4} → {2,5} → {3,6} → 7. L-GOV(4) runs anytime.

---

## 9. Top risks / decisions to flag

1. **Cedar partial-eval is experimental** (§5d) — enable the crate feature on `=4.11.2` vs. lower our own
   catalog-condition JSONB to SQL. **Recommend the JSONB-lowering path** (we own the no-code grammar →
   stable, no experimental API), prototype both on RoleManage. *Human decision.*
2. **Projected vs instance as-of asymmetry** (§1a) — projected types get audit-derived history, not full
   bi-temporal, in v1. Confirm that's acceptable for the console, or promote projected types to shadow
   revisions later. *Human decision.*
3. **Projected write path** must route through the domain crate's existing use-case (which owns its
   RLS+audit+FSM), NOT a second writeback into its table — else two sources of truth. Ontology holds
   metadata + dispatch only.
4. **Residual grammar fail-closed** — any untranslatable residual term ⇒ deny, never silent allow. Test as
   `console_rt` (superuser BYPASSRLS masks a broken filter — the known trap).
5. **JSONB attributes over EAV** (§1b) — accepted; revisit column-projection views only under measured
   hot-path pressure.
6. **Migration-number collisions** across the 6 lanes — reserve the next free `00NN` immediately before
   push; never at author time.
7. **`forbid` for guardrails** (tenant isolation, legal hold) — always-wins semantics make them
   un-out-permittable; model holds as `forbid`, never omission.
8. **No hard delete** anywhere in the engine — dispose is a terminal soft state + WORM tombstone, four-eyes
   gated. Enforce with REVOKE DELETE + trigger like `audit_events`.
