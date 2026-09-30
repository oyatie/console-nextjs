> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Cedar policy catalog and staging contract

> Status: implementation contract / no live authorization switch.
> Source issue: GitHub #313 (`[backend-gap/B16] Cedar policy catalog/authoring — read/simulate/no-code save`).
> Source design: `.omc/research/oyatie/prototype-anatomy/04-backend-contract.md` lines 102-111 and 224-279.
> Governing baseline: `docs/decisions/ADR-0021-cedar-pbac-authorization-strangler.md`, `docs/specs/cedar-pbac-cutover.md`, and `docs/specs/cedar-pbac-verification-observability.md`.

## 1. Purpose

The Policy screen needs a backend-real Cedar policy surface that is not confused with the existing legacy
role-matrix Policy Studio APIs. This contract defines the first B16 backend seam for:

1. listing Cedar policy catalog rows with natural-language rule text, permit/forbid effect, and status;
2. simulating who can see or do a target action/resource against the live policy set without changing enforcement;
3. saving no-code policy blocks as reviewable draft/staging artifacts, never directly as enforced policy.

This document is intentionally a design/implementation handoff. It does not add endpoints, migrations, clients,
or a Cedar live-enforcement promotion by itself.

## 2. Explicit split from legacy `/api/v1/policy/*`

Existing `/api/v1/policy/*` endpoints are the legacy custom-role / role-matrix bridge:

- `GET /api/v1/policy/features`
- `GET|POST /api/v1/policy/roles`
- `PATCH /api/v1/policy/roles/{id}`
- `PATCH /api/v1/policy/roles/{id}/status`
- `POST /api/v1/policy/roles/{id}/status-preview`
- `GET /api/v1/policy/assignments`
- `PUT /api/v1/policy/users/{id}/assignments`
- `POST /api/v1/policy/users/{id}/assignment-preview`
- `GET /api/v1/policy/audit-events`

Those routes remain useful for the G016 custom-role bridge, but they are not Cedar policy authoring and must not be
used as substitutes for B16. They expose `Feature` permissions, role definitions, assignments, and assignment-impact
previews; they do not expose Cedar principal/action/resource/effect rules, Cedar policy text, Cedar bundle identity,
or draft no-code policy staging.

New B16 endpoints use a separate prefix:

- `GET /api/v1/cedar-policies`
- `POST /api/v1/cedar-policy-simulations`
- `POST /api/v1/cedar-policy-drafts`

The OpenAPI descriptions for both old and new surfaces should say this plainly so generated clients and frontend
work cannot wire the Cedar canvas to the legacy role-matrix bridge by accident.

## 3. Shared authorization and tenant model

All three B16 endpoints are tenant/org scoped by the authenticated principal and server request context.

- The client never sends `org_id`, `tenant_id`, `policy_version`, `bundle_digest`, `schema_version`, `status = enforced`,
  or any other authority-bearing scope field.
- REST handlers derive org scope from `current_org()` / the resolved `Principal` and call storage through
  `with_org_conn` for reads or `with_audit` / `with_audits` for audited writes.
- Postgres tables must carry `org_id`, enable and force RLS, grant only the intended operations to `console_rt`, and use the
  standard `org_id = current_setting('app.current_org')::uuid` policy shape.
- Cross-org subject, role, team, branch, object, resource, or draft references are either invisible under RLS or rejected
  as validation/not-found errors. They are never silently resolved by owner/superuser connections.
- Authorization gates use the existing server-authoritative boundary. The minimum B16 gate is `RoleManage`; future
  narrower Cedar authoring capabilities may be added only through the Cedar promotion/governance ladder.
- UI projections and simulation responses are advisory. Protected endpoints must continue to reauthorize server-side.

## 4. Domain model

Implement the new Cedar authoring surface as a clean-architecture policy domain, not by extending the legacy role DTOs.
The intended crate split is:

```text
backend/crates/policy/domain
backend/crates/policy/application
backend/crates/policy/adapter-postgres
backend/crates/policy/rest
```

If the workspace naming convention changes before implementation, keep the boundary equivalent: a domain model for Cedar
policy rows/drafts, an application service that owns validation and commands, a Postgres adapter that owns RLS/audit-safe
persistence, and a REST crate that owns DTO/OpenAPI wiring. Reuse `console_platform_authz` types such as `Feature`,
`AuthorizationRequest`, `AuthorizationResource`, `DecisionEffect`, `DecisionReason`, `DualEngineMode`,
`SubjectFreshness`, and `CompiledBundleCacheKey` where they model the same concept; do not leak identity custom-role DTOs
into the Cedar contract.

### 4.1 `CedarPolicyCatalogRow`

A catalog row is the read model used by the policy screen. It may come from an enforced bundle, a shadow-only bundle, or
an org-scoped staging draft, but its status must make that source clear.

Required fields:

```text
CedarPolicyCatalogRow {
  id: Uuid,
  stable_key: String,
  title: String,
  natural_language_rule: String,
  effect: CedarPolicyEffect,              // permit | forbid
  status: CedarPolicyStatus,              // enforced | shadow | draft | review_pending | rejected | retired
  source: CedarPolicySource,              // system_generated | no_code_draft | promoted_policy | imported_fixture
  principal: CedarPrincipalSelector,
  action: CedarActionSelector,
  resource: CedarResourceSelector,
  conditions: Vec<CedarCondition>,
  engine_mode: Option<DualEngineMode>,
  policy_version: Option<i64>,
  schema_version: Option<String>,
  bundle_digest: Option<String>,
  cedar_sdk_version: Option<String>,
  cedar_language_version: Option<String>,
  validation_status: CedarValidationStatus,
  created_by: Option<UserId>,
  updated_by: Option<UserId>,
  created_at: Timestamp,
  updated_at: Timestamp,
}
```

Status semantics:

- `enforced`: current live Cedar enforcement consults this rule after a separate promotion gate. B16 draft save must never
  create this status.
- `shadow`: the rule is compiled/evaluated only for observation or comparison. It cannot change live outcomes.
- `draft`: editable no-code staging data; ignored by live simulation unless an explicit draft-preview mode is later added.
- `review_pending`: submitted staging artifact awaiting four-eyes/security review; still ignored by live enforcement.
- `rejected`: reviewed and rejected; retained for audit/history.
- `retired`: no longer part of the active catalog; retained for history.

Effect semantics:

- `permit`: a matching Cedar permit policy can allow only when all boundary preconditions, schema validation, freshness,
  RLS proof, and engine mode gates also pass.
- `forbid`: an explicit deny that wins over permits.
- No matching permit is a deny by omission. The API should surface this as a reason in simulations, not as a missing row
  error.

### 4.2 Selectors and conditions

Selectors are normalized server-owned structures generated from the no-code block grammar. The client may submit block
choices, but the server re-loads and normalizes every referenced fact.

```text
CedarPrincipalSelector {
  kind: role | job_function | user | team | branch | self | all_visible_users,
  key: Option<String>,
  user_id: Option<UserId>,
  display_label: String,
}

CedarActionSelector {
  action_key: String,      // canonical Feature/coexistence-map/action-registry key
  display_label: String,
}

CedarResourceSelector {
  resource_type: String,   // canonical resource kind, e.g. attendance_record, payroll_detail, work_order
  resource_id: Option<String>,
  scope: org | branch | team | self | object,
  display_label: String,
}

CedarCondition {
  condition_key: String,
  attribute: org | branch | team | employment_status | purpose | location | device_posture | sensitive_action | object_lifecycle | classification,
  operator: equals | not_equals | in | contains | present,
  values: Vec<String>,
  display_label: String,
}
```

The natural-language Korean rule line is generated from these normalized selectors and conditions. Client-submitted free
text can be stored as `author_note`, but it cannot be the authoritative `natural_language_rule` and it cannot become Cedar
policy text.

### 4.3 `CedarPolicyDraft`

Draft save writes a reviewable staging artifact, not runtime policy.

```text
CedarPolicyDraft {
  id: Uuid,
  org_id: OrgId,
  draft_key: String,
  title: String,
  author_note: Option<String>,
  blocks: CedarPolicyBlocks,
  normalized_row: CedarPolicyCatalogRow,
  generated_policy_text: String,
  generated_policy_digest: String,
  validation_status: CedarValidationStatus,
  validation_errors: Vec<CedarValidationError>,
  review_status: draft | review_pending | rejected | approved_for_promotion,
  reviewer_id: Option<UserId>,
  review_note: Option<String>,
  created_by: UserId,
  updated_by: UserId,
  created_at: Timestamp,
  updated_at: Timestamp,
}
```

`approved_for_promotion` is still not enforcement. Promotion to `shadow` or `enforced` belongs to a later promotion
endpoint and review gate, not to `POST /api/v1/cedar-policy-drafts`.

## 5. Persistence requirements

Use the next-free migration number only immediately before the implementation PR is ready to merge. Do not reserve a
number in this design note.

Minimum tables:

1. `cedar_policy_drafts`
   - `id uuid primary key default gen_random_uuid()`
   - `org_id uuid not null references organizations(id) on delete cascade`
   - `draft_key text not null`
   - `title text not null`
   - `author_note text null`
   - `blocks jsonb not null`
   - `normalized_row jsonb not null`
   - `generated_policy_text text not null`
   - `generated_policy_digest text not null`
   - `validation_status text not null check (...)`
   - `validation_errors jsonb not null default '[]'::jsonb`
   - `review_status text not null check (review_status in ('draft','review_pending','rejected','approved_for_promotion'))`
   - `reviewer_id uuid null references users(id)`
   - `review_note text null`
   - `created_by uuid not null references users(id)`
   - `updated_by uuid not null references users(id)`
   - `created_at timestamptz not null default now()`
   - `updated_at timestamptz not null default now()`
   - `unique (org_id, draft_key)`

2. `cedar_policy_catalog_entries`
   - materialized rows for promoted/shadow/enforced policy entries when the promotion/storage slice needs durable catalog
     rows beyond draft data;
   - same org/RLS/audit constraints;
   - `status` must include `shadow` and `enforced` separately;
   - writes to `enforced` must be blocked outside the separate promotion lane.

3. Optional `cedar_policy_simulation_audit` is not required if simulation audit rows are written to the shared
   `audit_events` table. Prefer shared audit rows unless simulation volume forces a later read-optimized table.

Required DB invariants:

- Every Cedar policy table has `FORCE ROW LEVEL SECURITY`.
- Every child reference that can prove same-org ownership should use composite FK shape where available, e.g.
  `(org_id, user_id) -> users(org_id, id)` rather than `user_id` alone.
- `org_id` is immutable via the standard trigger.
- `created_by`, `updated_by`, and `reviewer_id` must point to users visible in the same org or be validated in the write
  transaction under RLS.
- Draft saves do not bump the runtime `policy_versions` row used by the legacy/custom-role resolver. If a separate
  `cedar_policy_draft_versions` counter is needed for UI invalidation, name it separately so it cannot be mistaken for
  live authorization freshness.
- Only the future promotion path may bump the runtime Cedar bundle `policy_version` used in `CompiledBundleCacheKey`.

## 6. Endpoint contracts

### 6.1 List catalog rows

`GET /api/v1/cedar-policies`

Query parameters:

```text
status?: enforced | shadow | draft | review_pending | rejected | retired | all
source?: system_generated | no_code_draft | promoted_policy | imported_fixture
resource_type?: string
action_key?: string
effect?: permit | forbid
limit?: integer (1..100, default 50)
cursor?: string
```

Server behavior:

- Authenticate and resolve `Principal`.
- Require `RoleManage` or later explicit Cedar policy-read capability.
- Derive org from request context; ignore/reject any client org field.
- Read catalog/draft rows through `with_org_conn` under `console_rt`.
- Return rows the caller is allowed to see; branch-scoped policy managers see only rows whose selectors/resources are
  inside delegated scope. Omitted rows are not disclosed.
- Include `policy_version`/bundle identity only for rows backed by a current shadow/enforced bundle.

Response shape:

```json
{
  "policy_version": { "version": 12, "updated_at": "2026-07-09T00:00:00Z" },
  "cedar_bundle": {
    "schema_version": "cedar-policy-catalog.v1",
    "bundle_digest": "sha256:...",
    "cedar_sdk_version": "4.11.2",
    "cedar_language_version": "4.5",
    "engine_mode": "cedar_shadow_legacy_enforce"
  },
  "items": [
    {
      "id": "00000000-0000-0000-0000-000000000000",
      "stable_key": "attendance.team_lead.read_team_attendance",
      "title": "팀장 소속팀 근태 열람",
      "natural_language_rule": "팀장은 소속 팀원의 근태를 열람할 수 있다.",
      "effect": "permit",
      "status": "shadow",
      "source": "system_generated",
      "principal": { "kind": "job_function", "key": "team_lead", "display_label": "직책 · 팀장" },
      "action": { "action_key": "attendance_read", "display_label": "근태 열람" },
      "resource": { "resource_type": "attendance_record", "resource_id": null, "scope": "team", "display_label": "소속 팀원 근태" },
      "conditions": [
        { "condition_key": "same_team", "attribute": "team", "operator": "equals", "values": ["subject.team"], "display_label": "소속 팀" }
      ],
      "validation_status": "valid",
      "created_at": "2026-07-09T00:00:00Z",
      "updated_at": "2026-07-09T00:00:00Z"
    }
  ],
  "next_cursor": null
}
```

### 6.2 Simulate current live policy

`POST /api/v1/cedar-policy-simulations`

The simulation endpoint answers “who can see/do X?” against the current live policy set. It is read-only for policy state:
it may write an audit event for the simulation itself, but it must not write or promote policy data, change role
assignments, or change runtime enforcement.

Request shape:

```json
{
  "action_key": "attendance_read",
  "resource": {
    "resource_type": "attendance_record",
    "resource_id": "AT-2026-07-09-CHO",
    "scope_hint": "team"
  },
  "subject_selector": {
    "mode": "users",
    "user_ids": ["00000000-0000-0000-0000-000000000000"]
  },
  "context": {
    "purpose": "manager_review",
    "channel": "console"
  },
  "limit": 50
}
```

Allowed `subject_selector.mode` values:

- `me`: simulate only the caller.
- `users`: simulate named users visible to the caller under current org/branch scope.
- `role`: simulate visible users with a role/job-function selector.
- `all_visible`: sample or page through visible users only; never cross org.

Server behavior:

- Authenticate and require policy simulation authority (`RoleManage` initially).
- Load every subject/resource/action attribute server-side under armed RLS. Client fields identify targets only; they do
  not supply authorization facts.
- Evaluate the live policy set only: rows with `status in ('enforced','shadow')` depending on engine mode. Draft and
  review-pending rows are ignored unless a future explicit `draft_preview_id` contract is added.
- Preserve ADR-0021 dual-engine semantics. In `legacy_only`, the live outcome is legacy and Cedar output is explanatory
  only. In shadow modes, Cedar cannot grant or revoke live access. In Cedar-enforced modes, fail-closed reasons deny.
- Deny by omission: if no permit matches, return `deny / no_permit_matched` rather than “not found”. Explicit forbids win.
- Audit the simulation as `policy.cedar.simulate` with actor, action, resource type/id, subject selector summary, engine
  mode, policy version, decision counts, trace id, and span id. Do not place high-cardinality IDs in metric labels.

Response shape:

```json
{
  "simulation_id": "00000000-0000-0000-0000-000000000000",
  "policy_version": { "version": 12, "updated_at": "2026-07-09T00:00:00Z" },
  "engine_mode": "cedar_shadow_legacy_enforce",
  "resource": {
    "resource_type": "attendance_record",
    "resource_id": "AT-2026-07-09-CHO",
    "display_label": "조이슨 2026-07-09 근태"
  },
  "action": { "action_key": "attendance_read", "display_label": "근태 열람" },
  "results": [
    {
      "subject": {
        "user_id": "00000000-0000-0000-0000-000000000000",
        "display_label": "김팀장",
        "principal_labels": ["직책 · 팀장"]
      },
      "decision": "allow",
      "effect_source": "permit",
      "reason": "permit_matched",
      "matched_policy_ids": ["00000000-0000-0000-0000-000000000000"],
      "omitted_policy_count": 0,
      "rls_scope_proof": "runtime_role_guc",
      "audit_trace_id": "0123456789abcdef0123456789abcdef"
    }
  ],
  "summary": { "allow": 1, "deny": 0, "omitted": 0 }
}
```

### 6.3 Save no-code policy draft/staging artifact

`POST /api/v1/cedar-policy-drafts`

Request shape:

```json
{
  "title": "팀장 소속팀 근태 열람",
  "author_note": "Prototype policy canvas draft from Policy screen.",
  "principal": { "kind": "job_function", "key": "team_lead" },
  "resource": { "resource_type": "attendance_record", "scope": "team" },
  "action": { "action_key": "attendance_read" },
  "effect": "permit",
  "conditions": [
    { "attribute": "team", "operator": "equals", "values": ["subject.team"] }
  ],
  "save_mode": "draft"
}
```

`save_mode` values:

- `draft`: store editable staging data. Incomplete drafts may be stored with `validation_status = invalid`, but they cannot
  move to review or promotion.
- `review_pending`: store and submit the draft for review. This requires strict validation, non-empty generated policy
  text, a fresh passkey step-up when the implementation adds the reviewer flow, and no self-approval.

Server behavior:

- Authenticate and require `RoleManage` initially.
- Generate `draft_key`, normalized selectors, natural-language rule text, Cedar policy text, and digest server-side.
- Reject client-submitted Cedar policy source as authoritative input. A future advanced/debug view may display generated
  source only.
- Run schema-backed validation against the current Cedar schema when possible. Strict validation failures may be saved as
  editable `draft` rows, but `review_pending` must return `422` until fixed.
- Write the draft through `with_audit` / `with_audits` and append `policy.cedar_draft.create` or
  `policy.cedar_draft.update` audit evidence with before/after snapshots, validation status, generated digest, trace id,
  and span id.
- Do not bump runtime `policy_versions` and do not write any row with `status = enforced` or `status = shadow`.
- Return `201` for a new draft and `200` for an update if the implementation later supports idempotent draft keys.

Response shape:

```json
{
  "draft": {
    "id": "00000000-0000-0000-0000-000000000000",
    "draft_key": "attendance.team_lead.read_team_attendance.draft",
    "title": "팀장 소속팀 근태 열람",
    "natural_language_rule": "팀장은 소속 팀원의 근태를 열람할 수 있다.",
    "effect": "permit",
    "status": "draft",
    "review_status": "draft",
    "validation_status": "valid",
    "validation_errors": [],
    "generated_policy_digest": "sha256:...",
    "created_at": "2026-07-09T00:00:00Z",
    "updated_at": "2026-07-09T00:00:00Z"
  },
  "enforcement_effect": "none",
  "audit_trace_id": "0123456789abcdef0123456789abcdef",
  "next_actions": ["simulate", "submit_for_review"]
}
```

## 7. Validation rules

- `effect` is exactly `permit` or `forbid`.
- `title` is 1..120 characters after normalization.
- `author_note` is optional and capped at 512 characters.
- `principal.kind`, `resource.resource_type`, `action.action_key`, condition attributes, and condition operators come from
  server allowlists. Unknown strings return `422`.
- `action.action_key` must resolve to a canonical action in the server action registry. For v1 this should be the existing
  `Feature` / coexistence-map vocabulary unless a domain explicitly owns a richer action registry.
- The server reloads referenced users, roles, teams, branches, object rows, classifications, and lifecycle attributes under
  `console_rt` RLS. Not visible means not eligible; do not disclose whether the row exists in another org.
- Client-supplied `resource_id` identifies a candidate resource only. The server reloads the resource's org, branch/team,
  classification, lifecycle state, and other policy attributes.
- The server generates natural-language rule text and Cedar policy text from normalized blocks. Raw Cedar authoring is not
  accepted in B16.
- Missing permit means deny. Explicit forbid wins over permit. Engine errors, stale bundle, stale subject, missing map,
  malformed map, missing RLS proof, or audit write failure fail closed per ADR-0021.
- A draft may be saved while invalid only as `review_status = draft`; `review_pending`, `approved_for_promotion`, `shadow`,
  and `enforced` require strict validation plus a separate review/promotion path.

## 8. Audit, metrics, and observability

Required audit actions:

- `policy.cedar.simulate` for simulation runs;
- `policy.cedar_draft.create` for first draft save;
- `policy.cedar_draft.update` for draft edits;
- `policy.cedar_draft.submit` for moving a draft to `review_pending` when that transition is implemented;
- `policy.cedar_draft.reject` for review rejection when that transition is implemented.

Required audit payload fields:

- actor user id, org id, trace id, span id, occurred_at;
- target type (`cedar_policy_draft`, `cedar_policy_simulation`, or later `cedar_policy_catalog_entry`);
- target id/draft key;
- before and after snapshots for mutations;
- generated policy digest, schema version, Cedar SDK version, Cedar language version when generated/validated;
- validation status and validation errors for draft writes;
- action key, resource type/id, selector summaries, engine mode, and decision counts for simulation.

Metric labels must remain low-cardinality: `operation`, `outcome`, `effect`, `engine_mode`, `reason`, and `domain` are
acceptable. Do not label metrics with user ids, org ids, resource ids, draft ids, trace ids, or bundle digests.

## 9. OpenAPI and generated-client impacts

Implementation must update `backend/openapi/openapi.yaml` with these operation ids and schemas:

- `listCedarPolicies`
- `simulateCedarPolicyDecision`
- `saveCedarPolicyDraft`
- `CedarPolicyCatalogResponse`
- `CedarPolicyCatalogRow`
- `CedarPolicyEffect`
- `CedarPolicyStatus`
- `CedarPolicySource`
- `CedarPrincipalSelector`
- `CedarActionSelector`
- `CedarResourceSelector`
- `CedarCondition`
- `CedarPolicySimulationRequest`
- `CedarPolicySimulationResponse`
- `CedarPolicyDraftSaveRequest`
- `CedarPolicyDraftSaveResponse`
- `CedarValidationStatus`
- `CedarValidationError`

The existing `/api/v1/policy/*` descriptions should be amended to call them “legacy custom-role / role-matrix bridge”
where needed. Any `openapi.yaml` edit requires the repository's full client regeneration path, including Swift, Kotlin,
and TypeScript clients, and the OpenAPI drift tests.

## 10. Implementation and verification plan

Suggested sequencing for the existing B16 Kanban children:

1. Storage/domain child (`t_78327b21`): add domain/application models, migrations, RLS, audit builders, and Postgres
   adapter methods for catalog/draft persistence. Keep draft saves from changing runtime policy versions.
2. REST/OpenAPI child (`t_5916c5d7`): add REST routes/DTOs, OpenAPI schemas, generated clients, and request validation.
3. Verification child (`t_8634bda1`): prove tenant isolation, `console_rt` RLS, audit coverage, deny-by-omission simulation,
   draft-save non-enforcement, and OpenAPI/client drift.
4. Frontend consumer (#343 / policy screen) remains blocked from using legacy `/api/v1/policy/*` as a Cedar substitute
   until the B16 REST endpoints exist.

Minimum backend verification commands for implementation PRs:

```text
cd backend
SQLX_OFFLINE=true cargo test -p console_policy_domain -p console_policy_application -p console_policy_adapter_postgres -p console_policy_rest
SQLX_OFFLINE=true cargo test -p console_platform_authz cedar
cargo run -p console-gate-tenant-isolation
cargo run -p console-gate-rls-arming
cargo run -p console-gate-audit-coverage
cargo run -p console-gate-migration-safety
cargo run -p console-gate-layer-boundary
```

Adjust package names to the actual crate names if the implementation chooses a different `policy` crate prefix.
OpenAPI/client verification must include the repo's canonical `npm run gen:api` and drift checks after the REST child
edits `backend/openapi/openapi.yaml`.

## 11. Non-goals

- No live Cedar authorization switch.
- No endpoint in this contract can promote a draft to enforced policy.
- No client-submitted `org_id` or policy authority fields.
- No raw Cedar text authoring in the default no-code save path.
- No AI/LLM policy decisions or AI-generated authorization facts.
- No cross-org or owner-pool simulation.
- No replacement or deprecation of the legacy custom-role bridge in this slice.
- No import/upload/Excel-first workflow; the policy screen is a CRUD/no-code authoring workflow.
- No remote Cedar agent/distribution fabric.
