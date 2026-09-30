> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Benefit-catalog domain contract

Status: implementation-ready design note for Kanban `t_1bfcd1e1` / GitHub issue #310.

Sources:
- Prototype authority: `docs/design/oyatie-console/Oyatie Console.dc.html` `benefitData()` and `benefitAdvance`.
- Backend contract extract: `.omc/research/oyatie/prototype-anatomy/04-backend-contract.md` lines 55-72.
- Generic lifecycle substrate: BE-LC PR #211 on `origin/main`, especially `GET /api/v1/lifecycles/{objectType}/{objectId}`, `POST .../transition`, and `POST .../hold`.

Non-goals:
- No benefit payout, payroll calculation, claim, reimbursement, or payslip behavior.
- No bespoke benefit FSM. Benefit lifecycle is only a new object type/rule set in the generic lifecycle substrate.
- No Excel/import-first workflow. CRUD rows are primary; imports are future bootstrap tooling only.

## 1. Domain boundary

The domain stores benefit catalog rows needed by the console benefit tab:

- category tabs: `legal` / `extra` from the prototype.
- rows: name, coverage label/count, cost label/normalized cost, note/legal basis, optional related domain link.
- tier details: the prototype `tiers: { by, rows: [{ k, v }] }` shape.
- eligibility/condition details: the prototype row expansion currently renders conditions such as org/site applicability, tenure, and tier-basis-specific differences.
- lifecycle affordance: row state chip and advance button, but backed by BE-LC `object_lifecycles`, not by columns on the benefit table.

Canonical lifecycle binding:

- `objectType`: `benefit_catalog_item`
- `objectId`: `benefit_catalog_items.id`
- UI advance call: `POST /api/v1/lifecycles/benefit_catalog_item/{benefitId}/transition`
- UI lifecycle read: `GET /api/v1/lifecycles/benefit_catalog_item/{benefitId}` or the benefit list response's read-only lifecycle summary when joined for list performance.

Prototype lifecycle labels map to generic states as data:

| Prototype key | Korean label | Generic lifecycle state | Next state |
|---|---|---|---|
| `draft` | 초안 | `draft` | `pending` |
| `pending` | 승인 대기 | `pending` | `finalized` |
| `finalized` | 확정 · 시행 예정 | `finalized` | `implemented` |
| `implemented` | 시행 중 | `implemented` | `retiring` |
| `retiring` | 폐지 예정 | `retiring` | `retired` |
| `retired` | 폐지됨 | `retired` | none |

Seed these rules into `lifecycle_transition_rules` for `benefit_catalog_item`:

- `draft -> pending`
- `pending -> finalized`
- `finalized -> implemented`
- `implemented -> retiring`
- `retiring -> retired`

This is not a bespoke FSM; it is the BE-LC rule table's object-type configuration.

## 2. Clean architecture crate layout

Add the domain as a normal clean-architecture module:

```text
backend/crates/benefit/domain/
  Cargo.toml
  src/lib.rs
backend/crates/benefit/application/
  Cargo.toml
  src/lib.rs
backend/crates/benefit/adapter-postgres/
  Cargo.toml
  src/lib.rs
backend/crates/benefit/rest/
  Cargo.toml
  src/lib.rs
```

Workspace wiring:

- `backend/Cargo.toml` already admits `crates/<domain>/*`; if using an explicit member list on the active branch, add `crates/benefit/*`.
- Add kernel ID newtypes in `backend/crates/kernel/core/src/ids.rs`:
  - `BenefitCatalogItemId`
  - `BenefitCatalogTierId`
  - `BenefitCatalogConditionId`
- Domain crate depends only on `console-kernel-core` and `serde`.
- Application crate depends on domain + kernel and owns DTOs/commands/read models/audit builders.
- Adapter crate depends on domain/application + `console-platform-db` + `console-platform-request-context`; it derives org from `current_org()` and never accepts `org_id` from client input.
- REST crate depends on application/adapter + auth/authz; `backend/app` mounts it like other domain REST modules.

Domain layer types:

- `BenefitCategory`: `Legal`, `Extra`; DB strings `LEGAL`, `EXTRA`; wire strings `legal`, `extra`.
- `BenefitLifecycleObjectType`: constant `benefit_catalog_item` for REST/client contract only, not a domain FSM.
- `BenefitCatalogItem`: pure row invariant aggregate.
- `BenefitTier`: validates non-empty `tier_key`, `value_label`, display order.
- `BenefitCondition`: validates condition kind/operator/value shape and display label.
- Value objects:
  - `BenefitCode` matching `^BF-[0-9]{4,}$`.
  - `CoverageLabel`, `CostLabel`, `BenefitNote`, `LegalBasis` bounded text.
  - `MoneyWon` non-negative optional normalized cost.
  - `RateBasisPoints` optional, 0..=10000 for employer rates such as 4.5%.

Application layer commands/read models:

- `ListBenefitCatalogItemsQuery { branch_scope, category, branch_id, site_id, lifecycle_state, q, limit, offset }`
- `GetBenefitCatalogItemQuery { branch_scope, item_id }`
- `CreateBenefitCatalogItemCommand { actor, branch_scope, scope, category, name, coverage, cost, note, legal_basis, related_domain, effective_on, retires_on, tiers, conditions, trace, occurred_at }`
- `UpdateBenefitCatalogItemCommand { actor, branch_scope, item_id, fields, trace, occurred_at }`
- `ReplaceBenefitTiersCommand { actor, branch_scope, item_id, tiers, trace, occurred_at }`
- `ReplaceBenefitConditionsCommand { actor, branch_scope, item_id, conditions, trace, occurred_at }`
- `BenefitCatalogItemView` should include all console fields plus `lifecycle_object_type = "benefit_catalog_item"` and `lifecycle_object_id = id`.

## 3. Persistence model

Migration filename: `backend/crates/platform/db/migrations/<next>_create_benefit_catalog.sql`.

Do not hard-code the migration number in advance. Pick the next free number immediately before merge because concurrent sessions frequently reserve migration slots.

### 3.1 `benefit_code_counters`

Purpose: per-tenant immutable code issuance for `BF-0001` style object codes.

Columns:

- `org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT`
- `object_prefix TEXT NOT NULL CHECK (object_prefix = 'BF')`
- `next_value BIGINT NOT NULL DEFAULT 1 CHECK (next_value >= 1)`
- `updated_at TIMESTAMPTZ NOT NULL DEFAULT now()`
- primary key `(org_id, object_prefix)`

### 3.2 `benefit_catalog_items`

Columns:

- `id UUID PRIMARY KEY DEFAULT gen_random_uuid()`
- `org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT`
- `benefit_code TEXT NOT NULL CHECK (benefit_code ~ '^BF-[0-9]{4,}$')`
- `category TEXT NOT NULL CHECK (category IN ('LEGAL','EXTRA'))`
- `name TEXT NOT NULL CHECK (btrim(name) <> '' AND char_length(name) <= 120)`
- `scope_type TEXT NOT NULL DEFAULT 'ORG' CHECK (scope_type IN ('ORG','BRANCH','SITE','TEAM','ROLE','EMPLOYEE_SEGMENT'))`
- `scope_ref UUID NULL`
- `branch_id UUID NULL`
- `site_id UUID NULL`
- `coverage_label TEXT NOT NULL CHECK (btrim(coverage_label) <> '' AND char_length(coverage_label) <= 80)`
- `covered_count INTEGER NULL CHECK (covered_count IS NULL OR covered_count >= 0)`
- `cost_label TEXT NOT NULL CHECK (btrim(cost_label) <> '' AND char_length(cost_label) <= 80)`
- `estimated_annual_cost_won BIGINT NULL CHECK (estimated_annual_cost_won IS NULL OR estimated_annual_cost_won >= 0)`
- `employer_rate_bps INTEGER NULL CHECK (employer_rate_bps IS NULL OR employer_rate_bps BETWEEN 0 AND 10000)`
- `note TEXT NULL CHECK (note IS NULL OR char_length(note) <= 500)`
- `legal_basis TEXT NULL CHECK (legal_basis IS NULL OR char_length(legal_basis) <= 300)`
- `related_domain TEXT NULL CHECK (related_domain IS NULL OR related_domain ~ '^[a-z][a-z0-9_]{1,63}$')`
- `related_object_id UUID NULL`
- `effective_on DATE NULL`
- `retires_on DATE NULL CHECK (retires_on IS NULL OR effective_on IS NULL OR retires_on >= effective_on)`
- `display_order INTEGER NOT NULL DEFAULT 0`
- `metadata JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(metadata) = 'object')`
- `created_by UUID NOT NULL`
- `updated_by UUID NOT NULL`
- `created_at TIMESTAMPTZ NOT NULL DEFAULT now()`
- `updated_at TIMESTAMPTZ NOT NULL DEFAULT now()`
- `UNIQUE (id, org_id)`
- `UNIQUE (org_id, benefit_code)`
- foreign keys:
  - `(branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT`
  - `(site_id, org_id) REFERENCES registry_sites(id, org_id) ON DELETE RESTRICT`
  - `(created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT`
  - `(updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT`

Scope check:

- `ORG`: `scope_ref`, `branch_id`, `site_id` are null.
- `BRANCH`: `branch_id IS NOT NULL`, `scope_ref = branch_id`, `site_id IS NULL`.
- `SITE`: `branch_id IS NOT NULL`, `site_id IS NOT NULL`, `scope_ref = site_id`.
- `TEAM`, `ROLE`, `EMPLOYEE_SEGMENT`: `scope_ref IS NOT NULL`; branch/site may narrow further when applicable.

Indexes:

- `idx_benefit_catalog_items_category_order ON benefit_catalog_items (org_id, category, display_order, name)`
- `idx_benefit_catalog_items_scope ON benefit_catalog_items (org_id, scope_type, scope_ref)`
- `idx_benefit_catalog_items_branch_site ON benefit_catalog_items (org_id, branch_id, site_id) WHERE branch_id IS NOT NULL`
- unique expression index for natural duplicates:
  - `(org_id, category, lower(btrim(name)), scope_type, coalesce(scope_ref, '00000000-0000-0000-0000-000000000000'::uuid))`

### 3.3 `benefit_catalog_tiers`

Purpose: relational representation of prototype tier rows.

Columns:

- `id UUID PRIMARY KEY DEFAULT gen_random_uuid()`
- `org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT`
- `benefit_id UUID NOT NULL`
- `tier_basis TEXT NOT NULL CHECK (btrim(tier_basis) <> '' AND char_length(tier_basis) <= 80)`; examples: `직급`, `현장`, `직책`, `직급·연령`.
- `tier_key TEXT NOT NULL CHECK (btrim(tier_key) <> '' AND char_length(tier_key) <= 120)`; prototype `k`.
- `value_label TEXT NOT NULL CHECK (btrim(value_label) <> '' AND char_length(value_label) <= 300)`; prototype `v`.
- `amount_won BIGINT NULL CHECK (amount_won IS NULL OR amount_won >= 0)`
- `limit_period TEXT NULL CHECK (limit_period IS NULL OR limit_period IN ('MONTH','QUARTER','YEAR','EVENT','TENURE_MILESTONE'))`
- `criteria JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(criteria) = 'object')`
- `display_order INTEGER NOT NULL DEFAULT 0`
- `status TEXT NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE','RETIRED'))`
- `created_by UUID NOT NULL`
- `updated_by UUID NOT NULL`
- `created_at TIMESTAMPTZ NOT NULL DEFAULT now()`
- `updated_at TIMESTAMPTZ NOT NULL DEFAULT now()`
- `UNIQUE (id, org_id)`
- `FOREIGN KEY (benefit_id, org_id) REFERENCES benefit_catalog_items(id, org_id) ON DELETE RESTRICT`
- user FKs for `created_by` / `updated_by`

Indexes:

- `idx_benefit_catalog_tiers_item ON benefit_catalog_tiers (org_id, benefit_id, status, display_order)`
- `UNIQUE (org_id, benefit_id, tier_basis, tier_key) WHERE status = 'ACTIVE'`

### 3.4 `benefit_catalog_conditions`

Purpose: eligibility/applicability rows for console condition chips and future Cedar no-code rule editing.

Columns:

- `id UUID PRIMARY KEY DEFAULT gen_random_uuid()`
- `org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT`
- `benefit_id UUID NOT NULL`
- `condition_kind TEXT NOT NULL CHECK (condition_kind IN ('ORG','BRANCH','SITE','TEAM','ROLE','POSITION','TENURE','AGE','GENDER','EMPLOYMENT_TYPE','CONTRACT','COST_CENTER','CUSTOM'))`
- `operator TEXT NOT NULL CHECK (operator IN ('eq','in','not_in','gte','lte','range','exists','custom_policy'))`
- `condition_key TEXT NOT NULL CHECK (condition_key ~ '^[a-z][a-z0-9_]{1,63}$')`
- `condition_value JSONB NOT NULL CHECK (jsonb_typeof(condition_value) IN ('object','array','string','number','boolean'))`
- `display_label TEXT NOT NULL CHECK (btrim(display_label) <> '' AND char_length(display_label) <= 200)`
- `cedar_policy_ref TEXT NULL CHECK (cedar_policy_ref IS NULL OR char_length(cedar_policy_ref) <= 200)`
- `display_order INTEGER NOT NULL DEFAULT 0`
- `status TEXT NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE','RETIRED'))`
- `created_by UUID NOT NULL`
- `updated_by UUID NOT NULL`
- `created_at TIMESTAMPTZ NOT NULL DEFAULT now()`
- `updated_at TIMESTAMPTZ NOT NULL DEFAULT now()`
- `UNIQUE (id, org_id)`
- `FOREIGN KEY (benefit_id, org_id) REFERENCES benefit_catalog_items(id, org_id) ON DELETE RESTRICT`
- user FKs for `created_by` / `updated_by`

Indexes:

- `idx_benefit_catalog_conditions_item ON benefit_catalog_conditions (org_id, benefit_id, status, display_order)`
- `idx_benefit_catalog_conditions_kind ON benefit_catalog_conditions (org_id, condition_kind, condition_key) WHERE status = 'ACTIVE'`

### 3.5 Lifecycle rule seed

The same migration must add global lifecycle rules:

```sql
INSERT INTO lifecycle_transition_rules (object_type, from_state, to_state) VALUES
    ('benefit_catalog_item', 'draft', 'pending'),
    ('benefit_catalog_item', 'pending', 'finalized'),
    ('benefit_catalog_item', 'finalized', 'implemented'),
    ('benefit_catalog_item', 'implemented', 'retiring'),
    ('benefit_catalog_item', 'retiring', 'retired')
ON CONFLICT DO NOTHING;
```

If seeding the prototype rows in the same slice, create their `object_lifecycles` rows with the desired current state and an explicit bootstrap reason in `object_lifecycle_transitions`. Runtime state changes after bootstrap must use `/api/v1/lifecycles/.../transition`.

Prototype seed coverage:

- Legal rows: 국민연금, 건강보험, 고용보험, 산재보험, 퇴직급여, 연차 유급휴가, 출산전후휴가, 육아휴직, 배우자 출산휴가, 법정 건강검진.
- Extra rows: 경조사비, 명절 선물, 중식 지원, 통신비 보조, 자기계발비, 동호회 지원, 건강검진 상향, 경조 휴가, 리프레시 휴가, 포상 휴가, 장기근속 포상.
- Prototype current states: default `implemented`; `통신비 보조` = `finalized`; `자기계발비` = `pending`; `동호회 지원` = `retiring`. Convert display-only `lifeDate` strings into real `effective_on` / `retires_on` when the year is known by the seed source; otherwise leave dates null and keep the display phrase in migration seed metadata only.

## 4. RLS, grants, and deny-by-omission

All tenant tables are RLS protected:

```sql
ALTER TABLE <table> ENABLE ROW LEVEL SECURITY;
ALTER TABLE <table> FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON <table>
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
```

Grant runtime role only what the domain needs:

- `benefit_code_counters`: `SELECT, INSERT, UPDATE`; revoke `DELETE`.
- `benefit_catalog_items`: `SELECT, INSERT, UPDATE`; revoke `DELETE`.
- `benefit_catalog_tiers`: `SELECT, INSERT, UPDATE`; revoke `DELETE`.
- `benefit_catalog_conditions`: `SELECT, INSERT, UPDATE`; revoke `DELETE`.
- Child row removal is logical retirement (`status = 'RETIRED'`), never physical delete.

Org and branch scope rules:

- `org_id` is always derived from the authenticated principal / `current_org()`, never request body or query string.
- Read list returns only rows whose `scope_type` is org-wide or intersects the caller's allowed branch/site/team/responsibility scope.
- Direct reads for inaccessible rows return 404, not 403, so cross-scope existence is not leaked.
- Mutations require the manage feature and a scope check: callers may only create/update rows inside scopes they are allowed to manage.
- A platform/operator token without a tenant context must see zero tenant benefit rows unless it explicitly enters an authorized tenant context through the existing platform flow.

Feature seeds:

```sql
INSERT INTO feature_catalog (feature_key) VALUES
    ('benefit_catalog_read'),
    ('benefit_catalog_manage')
ON CONFLICT (feature_key) DO NOTHING;
```

Authz recommendations:

- Read: `BenefitCatalogRead` for HR/payroll/manager personas that can view benefit policy rows.
- Manage: `BenefitCatalogManage` for HR/legal/admin personas; branch managers can be limited to branch/site-scoped rows.
- Lifecycle transition/hold itself is still guarded by `LifecycleManage` until BE-LC grows per-object read/write delegation. The benefit REST layer must not bypass that by writing `object_lifecycles` directly.

## 5. Audit requirements

Every benefit-catalog mutation is audited in the same transaction as the data mutation.

Use action strings:

- `benefit_catalog.item.create`
- `benefit_catalog.item.update`
- `benefit_catalog.tiers.replace`
- `benefit_catalog.conditions.replace`

Audit event requirements:

- `actor`: authenticated user.
- `target_type`: `benefit_catalog_item`.
- `target_id`: item UUID or `benefit_code` as the domain convention chooses; prefer UUID in storage and expose code in snapshots.
- `org_id`: attached with `.with_org(org)`.
- `branch_id`: attached when item scope has a branch.
- `trace`: request trace context.
- snapshots: before/after JSON including category, name, scope, coverage/cost labels, changed tier/condition summaries, and lifecycle object mapping.

Lifecycle mutations use BE-LC audit actions already defined by PR #211:

- `lifecycle.transition`
- `lifecycle.hold_set`

The benefit implementation must not duplicate these events, but it should include the lifecycle object mapping in benefit read models so auditors can traverse from BF row to lifecycle history.

REST tests and fixtures must not seed mutable state with bare unaudited `INSERT`/`UPDATE` when testing behavior that is supposed to be audited; use the repo's audit helpers or create through the adapter/REST surface.

## 6. REST API surface

Benefit-specific endpoints are needed for catalog CRUD. Lifecycle state changes continue to use the generic lifecycle endpoints.

Proposed paths:

- `GET /api/v1/benefit-catalog/items`
- `POST /api/v1/benefit-catalog/items`
- `GET /api/v1/benefit-catalog/items/{benefitId}`
- `PATCH /api/v1/benefit-catalog/items/{benefitId}`
- `PUT /api/v1/benefit-catalog/items/{benefitId}/tiers`
- `PUT /api/v1/benefit-catalog/items/{benefitId}/conditions`

List query parameters:

- `category=legal|extra`
- `branchId`, `siteId` when the caller wants a narrower scope; server still intersects with principal scope.
- `lifecycleState` optional; implement with a left join to `object_lifecycles` for list performance, but do not mutate lifecycle in this endpoint.
- `q`, `limit`, `offset`.

Response shape for a row:

```json
{
  "id": "uuid",
  "benefitCode": "BF-0001",
  "category": "legal",
  "name": "국민연금",
  "scope": { "type": "ORG", "ref": null, "branchId": null, "siteId": null },
  "coverageLabel": "1,284",
  "coveredCount": 1284,
  "costLabel": "₩3.42억",
  "estimatedAnnualCostWon": 342000000,
  "employerRateBps": 450,
  "note": "사업자 4.5%",
  "legalBasis": null,
  "relatedDomain": null,
  "relatedObjectId": null,
  "effectiveOn": null,
  "retiresOn": null,
  "tiers": [
    { "id": "uuid", "basis": "직급", "key": "사원·주임", "valueLabel": "결혼 50만 · 조사 30만", "criteria": {} }
  ],
  "conditions": [
    { "id": "uuid", "kind": "SITE", "operator": "eq", "key": "site_id", "value": "uuid-or-json", "displayLabel": "㈜코스 · 안산공장 적용" }
  ],
  "lifecycle": {
    "objectType": "benefit_catalog_item",
    "objectId": "same uuid",
    "currentState": "implemented",
    "legalHold": false,
    "retentionUntil": null
  },
  "createdAt": "RFC3339",
  "updatedAt": "RFC3339"
}
```

If the list endpoint cannot cheaply join lifecycle rows in the first implementation, it may return only `lifecycle.objectType` / `lifecycle.objectId` and the console can fan out to `GET /api/v1/lifecycles/...`. Do not copy lifecycle state into `benefit_catalog_items` as a cached mutable column.

OpenAPI/client obligations:

- Any new REST path requires `backend/openapi/openapi.yaml` updates.
- Regenerate all supported clients, including Swift: `npm run gen:api` or the repo's current split commands if the branch uses them.
- Update route drift tests in `backend/app/tests/openapi_drift.rs` or equivalent.

## 7. Console contract mapping

Prototype `benefitData()` fields map as follows:

| Prototype field | Backend field |
|---|---|
| tab `legal` / `extra` | `category` |
| `name` | `name` |
| `cover` | `coverage_label`; parse numeric when safe into `covered_count` |
| `cost` | `cost_label`; parse normalized won amount when safe into `estimated_annual_cost_won` |
| `note` | `note`; legal citations may additionally populate `legal_basis` |
| `link: "leave"` | `related_domain = "leave"` until a concrete leave object exists |
| `tiers.by` | `benefit_catalog_tiers.tier_basis` |
| `tiers.rows[].k` | `benefit_catalog_tiers.tier_key` |
| `tiers.rows[].v` | `benefit_catalog_tiers.value_label` |
| rendered `conds[]` | `benefit_catalog_conditions.display_label` plus structured kind/operator/value |
| `life` | generic `object_lifecycles.current_state` |
| `lifeDate` | `effective_on` for `finalized`, `retires_on` for `retiring`, or seed metadata if date is display-only |

UI behavior:

- Category tab uses `GET /api/v1/benefit-catalog/items?category=legal|extra`.
- Row expansion renders `tiers` and `conditions` from the response.
- Condition edit opens Cedar/policy editing only if the caller has manage capability; otherwise the action is omitted.
- Advance button is shown only when lifecycle read says a next rule exists and the user has lifecycle transition authority.
- Advance POST body uses BE-LC request schema: `{ "toState": "pending|finalized|implemented|retiring|retired", "reason": "..." }`.

## 8. Implementation sequence

1. Rebase/branch from a head that contains BE-LC PR #211 or otherwise includes `object_lifecycles`, `lifecycle_transition_rules`, and lifecycle REST routes.
2. Add kernel ID newtypes and benefit crates.
3. Add migration with benefit tables, feature seeds, RLS/grants, lifecycle rules, and optional prototype seed rows.
4. Implement domain validators and application commands/views/audit builders.
5. Implement Postgres adapter:
   - reads via `with_org_conn`;
   - mutations via `with_audit`/`with_audits`;
   - no client-supplied org;
   - branch/scope predicates applied before every item fetch/update.
6. Implement REST module and app router wiring.
7. Update OpenAPI, generated clients, route drift lists, and feature enum/matrix.
8. Add tests and gates listed below.

## 9. Test obligations

Domain/unit:

- category parse/serialize round trips.
- code/rate/money/text validators reject invalid values.
- tier and condition validators reject blank labels, unsupported operators/kinds, and invalid JSON shapes.
- scope invariant tests for `ORG`, `BRANCH`, `SITE`, `TEAM`, `ROLE`, and invalid combinations.

Adapter DB tests, all against runtime role `console_rt` and FORCE RLS:

- create/list/get benefit rows in one tenant.
- cross-tenant rows are invisible under RLS.
- branch/site scope deny-by-omission: list omits unauthorized rows; direct get/update returns not found.
- mutation emits exactly one audit event in the same transaction, with before/after snapshots.
- audit rollback: if a tier/condition replacement fails, neither data rows nor audit rows persist.
- DELETE is unavailable for runtime role on all benefit tables.
- tier/condition replacement logically retires old child rows instead of deleting them.
- lifecycle rule seed allows `benefit_catalog_item` transitions through the expected chain and rejects skipped transitions.
- lifecycle hold/retention blocks `retired`/terminal disposal behavior if future BE-LC maps `retired` to a dispose-equivalent control; until then, hold/retention still must round-trip through `/hold`.

REST tests:

- 401 unauthenticated, 403 without read/manage feature, 404 for inaccessible id.
- list returns legal/extra rows in display order.
- create/update/replace tiers/replace conditions audit and return console-ready shape.
- benefit REST does not accept or trust `orgId` in the request body.
- lifecycle advance uses `/api/v1/lifecycles/benefit_catalog_item/{id}/transition`; no benefit-specific transition route exists.

Gate commands:

- `SQLX_OFFLINE=true cargo fmt --check` from `backend/`.
- `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings` from `backend/` when scope permits.
- Focused DB tests for benefit adapter/rest as `console_rt` against dev Postgres on `127.0.0.1:55432`.
- `cargo run -p console-gate-tenant-isolation`.
- `cargo run -p console-gate-rls-arming`.
- `cargo run -p console-gate-audit-coverage`.
- `cargo run -p console-gate-migration-safety`.
- `cargo run -p console-gate-layer-boundary`.
- OpenAPI/client drift checks after client regeneration.

## 10. Acceptance checklist for the implementation card

- Benefit rows are stored in tenant-scoped tables, not frontend fixtures.
- Legal and extra categories from `benefitData()` are represented.
- Tier rows and eligibility/condition rows are relational, queryable, and auditable.
- Benefit lifecycle uses `objectType=benefit_catalog_item` + `objectId=benefit_catalog_items.id` through BE-LC endpoints.
- No benefit-specific FSM columns or transition endpoints exist.
- Reads and writes deny by omission across org/branch/site scope.
- Every mutation is audited with snapshots.
- Migration includes constraints, indexes, FORCE RLS, runtime grants, no runtime DELETE, feature seeds, and lifecycle rule seeds.
- REST/OpenAPI/client surfaces are updated only for catalog CRUD; lifecycle remains generic.
- Tests prove RLS, audit, lifecycle binding, and console response shape.
