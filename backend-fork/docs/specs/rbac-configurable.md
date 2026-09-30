# RBAC — Configurable Roles & Policy (design sub-spec)

> **Post-pivot status (2026-08-03):** Retained only as backend policy/authz
> design and shipped-mechanism reference within the current Policy boundary. Its
> former `knl-business-os.md` parent is superseded; this document cannot authorize
> a frontend, production cutover, org-wide widening, or work outside the canonical
> pivot. P3 UI and every stated live/prod transition remain **HOLD** until current
> program authority explicitly promotes them with exact-candidate evidence.

> **Status:** DESIGN / TARGET STATE — adversarial security-review DONE (verdict **NOT-YET — HIGH**:
> 3 CRITICAL + 4 HIGH + 3 MEDIUM); revisions §0.1 below fold every must-fix into the design.
> **Current repository-source increment is the backend subset of §9 plus early P1 guardrails:** tenant role definitions/catalog,
> ABAC/PBAC condition metadata, assignment impact preview, passkey-gated role lifecycle changes,
> delegated branch-scope guardrails, a tenant `policy_version` lineage field, and runtime-effective
> additive custom-role grants for supported ordinary tenant features. Unsupported/elevated/scope-widening
> policy remains inert until re-reviewed before P1/P2 cutover. This is source evidence, not production,
> exposure, or UI readiness. The G002 pattern: kill the HIGHs pre-code.
> **Historical parents:** `SPEC.md`, superseded `docs/specs/knl-business-os.md`, and
> `docs/specs/org-hierarchy.md`. Current scope comes from the canonical pivot and program roadmap.
> **Cedar/PBAC target baseline:** `docs/decisions/ADR-0021-cedar-pbac-authorization-strangler.md`
> and `docs/specs/cedar-pbac-cutover.md` govern the next authorization substrate. This spec remains the
> shipped custom-role bridge until Cedar-enrolled actions are explicitly promoted; UI policy projections remain
> non-authoritative and live routes remain server-authorized.
> **Trigger:** user directive 2026-06-24 — *"should allow new / custom roles with
> configurable permissions / policy"*, generalizing the *"add an org-admin grant"*
> decision into a data-driven RBAC. **Target quality bar:** Palantir-grade, enterprise-production;
> no current production claim is implied.
> Benchmark/gate: [`docs/benchmarks/palantir-foundry-ops-benchmark.md`](../benchmarks/palantir-foundry-ops-benchmark.md) — object/action model, policy preview, lineage, audit, telemetry, and runbook evidence.

## 0.1 Security-review revisions (must-fix, folded into the design)

The pre-code security-review found the model would ship with **silent privilege-escalation and
parity holes**. The dominant discovery: **authorization on a role STRING/variant is pervasive today
(18 sites)** — and the dangerous ones are not the obvious `is_admin_like`, but the **"grant ≤ self"
guard itself, the OTP/credential-reset privileged-target gates, and a money-path approval router** —
each of which silently breaks or is bypassed once roles become renamable/custom. Each must-fix is now
binding:

- **R1 (C1) — Ban role-string authorization.** *No authorization decision may branch on a `Role`
  variant or role string.* Every one of the 18 enumerated sites converts to a capability check. The
  scary ones (must convert in the RBAC slice, **before** any custom role can be assigned in prod):
  `identity/rest:787` (`authorize_user_write` — the live "grant ≤ self" enforcement, blind to a
  custom super-role), `auth-rest:990/993` (`issue_admin_otp`) + `auth-rest:1083/1086`
  (`admin_credential_reset`) privileged-target tests (→ account takeover of a custom-super account),
  `financial:1574-1586` (`purchase_actor_for_user_tx` role-string cascade that drives the purchase
  approval state machine), `financial:1675` (self-approval exemption), `registry/rest:166/1350`
  (`all_branches` substitute search). Plus the **~13 `BranchScope::All`-from-role producers** (see R6).
  A new CI gate (sibling of `console-gate-rls-arming`) **fails on any `matches!(*, Role::…)` / `role ==
  "…"` / `.roles.contains(Role::…)` outside the authz resolver**.
- **R2 (C2) — Escalation closure.** "Grant ≤ self" is checked over the **post-union effective
  capability set** at **assignment time**, against the **assigner's live resolved set** (never the
  role author's). `RoleManage` is **self-bounded**: it confers only the ability to grant the subset
  the assigner currently holds. Assigning ANY role conferring a capability whose system-default cell
  is `[D,D,D,D,D,A]` (a named, `matrix_row()`-derived elevated allow-list incl. `ElevatedRoleGrant`,
  `RoleManage`, `UserManage`) requires the actor to hold `ElevatedRoleGrant`. Role **creation** is not
  a grant — the assignment guard is the sole choke point and checks every conferred capability.
- **R3 (C3) — Token / `users.roles` / `Role::from_str` collision.** `users.roles` is a `text[]` with a
  hard 6-value CHECK (`0046_…`), the token claim is literally `user.roles.clone()`, and every extractor
  maps via `Role::from_str` (401 on unknown) — so custom slugs **cannot** ride `users.roles` or the
  token through `Role`. Resolution (binding): **`user_role_assignments` is the source of truth**
  (RLS-armed); `users.roles` + its CHECK stay **system-only**; the resolver unions system-role policy
  (from the token's built-in roles) with custom-role assignments **read from DB per request**; custom
  keys **never** flow through the `Role` enum; **key→policy is parse-or-deny** (any key/feature/level
  not resolving to a live row under armed RLS contributes **zero** capabilities — never most-permissive).
- **R4 (H1) — Revoke is globally effective.** Cache key = `(org_id, policy_version)`; every
  `RoleManage` write **synchronously bumps a per-org `policy_version`** (RLS-armed row) before
  returning; resolution reads the version (one cheap armed read, deny on failure) and treats a mismatch
  as a miss. No reliance on per-process TTL/in-node invalidation for a revoke. (Moved from "later" into
  **P1**.)
- **R5 (H2/H4) — No-lockout, hardened + parity-proofed.** The floor is "≥1 **active** user whose
  **effective** capability set (union over assignments) includes **both** `RoleManage` **and**
  `ElevatedRoleGrant`"; the holder-count check runs under a per-org advisory lock / serializable tx
  (kills the concurrent-revoke TOCTOU). Seed parity: a **golden test over the full `Role::ALL ×
  Feature::ALL` grid keyed by enum** (not column index); catalog growth **upserts** new `matrix_row()`
  cells into every org's `kind='system'` rows (idempotent) and re-asserts `SUPER_ADMIN`'s full `Allow`
  set.
- **R6 (H3) — One `BranchScope::All` decision point.** Exactly one shared capability-driven function
  derives `BranchScope::All` from holding the relevant `*OrgWide` capability; **every** producer routes
  through it — the REST queue scope (`work_order_list_scope`), the **evidence** widen
  (`workorder/rest:1706`), the realtime widen (`realtime:1035`, a raw-string second copy **outside** the
  org middleware), `resolve_branch_scope_in_org` and all ~13 per-crate copies, and the
  `support/rest:272/603` consumers. Collapse the duplicates.
- **R7 (M1) — Catalog is referential + parse-or-deny.** `role_permissions.feature`/`level` reference a
  `feature_catalog` table seeded from `Feature::ALL` (FK), single-sourcing the catalog; the resolver
  drops any unparseable `feature`/`level` to `Deny`.
- **R8 (M2) — Prove the cache-warm read is armed.** The per-org policy warm-load must be armed in-tx and
  tested as **real `console_rt`**; a bare-pool warm must yield deny-all (proving arming is load-bearing —
  the `resolve_branch_scope_in_org` footgun). Add the 3 tables + `policy_version`/`feature_catalog` to
  `console-gate-rls-arming`.
- **R9 (M3) — `view_as` stays system-roles-only.** Operator impersonation
  (`platform/platform-rest/view_as.rs`) must never mint arbitrary custom-role tokens without re-deriving
  effective capabilities under the target org's RLS. Stated as an explicit guard.

**Sound (keep):** tier separation (no `PlatformFeature→Feature` bridge; `PlatformPrincipal` carries no
role/scope), default-deny composition, `repository_filter` empty-scope = `FALSE`. The
`role_permissions.feature` CHECK references only `Feature`, **never** `PlatformFeature`.

## 0. Today's model (what we are changing)

RBAC is no longer 100% compile-time for ordinary tenant features:

- `Role` remains a fixed 6-variant **system-role** enum: `SUPER_ADMIN, ADMIN, MECHANIC, RECEPTIONIST, EXECUTIVE, MEMBER`
  (`crates/platform/authz/src/lib.rs`).
- Permissions still have a `const fn matrix_row()` — a `[PermissionLevel; 6]` per `Feature` (~40 features),
  columns `[MEMBER, RECEPTIONIST, MECHANIC, ADMIN, EXECUTIVE, SUPER_ADMIN]`. `permission_for(role,
  feature)` indexes the immutable system-role floor.
- A user's system roles ride in the **verified JWT** (`AccessClaims.roles` → `Role::from_str`); branch scope
  is resolved per-request from `user_branches` (`resolve_branch_scope_in_org`: only `SUPER_ADMIN` /
  `EXECUTIVE` → `BranchScope::All`).
- Tenant-owned custom role rows now resolve into additive `EffectiveFeatureGrant`s on the request principal
  for supported ordinary features. Backend authorization ignores JWT feature hints and re-resolves the live
  custom policy from the DB on every request.

The system-role floor is correct, auditable, and fast, but it is no longer sufficient alone. A conglomerate
of legal entities in different industries (물류 / 제조·OEM / 파견·용역 / …) needs per-tenant roles
("현장소장", "배차데스크", "노무담당", "구매과장") with tenant-specific policy. This sub-spec introduces
a **data-driven role + policy layer** while preserving every isolation and escalation guarantee the
compile-time model gives.

## 1. End-state objective & non-goals

**Objective.** A per-tenant administrator (with the new `RoleManage` capability) can, **through the
audited console** (never SQL): create/edit/retire **custom job-function roles**, set each role's
**policy** (which capabilities, at which `PermissionLevel`, plus scope reach), and **assign** roles
to users alongside their department/team, position, responsibility assignments, and object scopes —
with default-deny, least-privilege, and full audit. The built-in 6 remain as **immutable bootstrap
system roles**; custom policy layers on top. The authz engine resolves a principal's effective
permissions from the **effective policy** (system defaults ∪ tenant custom roles ∪ responsibility /
attribute rules) instead of the static matrix.

### 1.1 Human role model correction and configurable policy

The six built-in tenant roles are not adequate for enterprise production. They are bootstrap columns
for migration parity, not the target operating model. The target policy input is:

- **Job function / function role:** 정비, 배차, 생산, HR, Payroll, Finance, Purchasing, Platform Ops.
- **Department/team:** where the person works and which work queues/calendar/mail/polls they share.
- **Position / level:** title or authority band used for approval thresholds and escalation.
- **Responsibility assignment:** explicit object/scope duties such as site owner, equipment owner,
  line supervisor, payroll processor, purchase approver, group admin, or safety reviewer.
- **Scope:** platform, group, subsidiary/org, department, branch/site, object, self.
- **Context attributes:** employment state, shift/time, passkey step-up freshness, location consent,
  object sensitivity, object lifecycle state, and emergency/exception flags.

Therefore the policy editor must not present a single role checkbox list as the final model. It must
show effective access as the result of `subject attributes + object attributes + action +
environment`, with RBAC job functions and PBAC bundles as shortcuts over that policy. Existing
`users.roles` remains a compatibility source until the resolver moves to the richer subject model.

Policy is **configurable and versioned**, not fixed/static. The server ships default policy bundles
for safe bootstrap, but tenant/group admins with `RoleManage`/policy authority can draft, preview,
simulate, approve, activate, roll back, and retire policy versions through audited UI. Every runtime
authorization decision must be traceable to a policy version and reason.

Employment transitions are policy inputs. The resolver must distinguish person/employee lifecycle
state, account credential state, and policy assignment state:

- pending setup: person/user exists, no finished passkey/signup; do not show as active;
- active: required agreements, credential setup, and minimum assignment complete;
- transferred: scope/team/manager/responsibilities change with effective-access diff and history;
- on leave/suspended: login/actions reduced or disabled while records remain;
- terminated/retired: credentials/sessions revoked, open responsibilities handed off, legally
  required HR/payroll/retirement records retained under domain policy;
- rehired/reactivated: same person history, new credential/policy activation as required.

No employment transition may be modeled as destructive row deletion when labor, wage, retirement,
audit, or privacy-retention obligations require historical preservation.

**Non-goals (end-state design).**
- **No user-defined capabilities.** Custom roles **compose the existing `Feature` catalog** (the ~40
  capabilities already in code). Inventing *new* capability primitives belongs to the later
  **ontology-actions** layer (action/write-back engine, G010), not here. The capability set is the
  fixed, code-reviewed vocabulary; only their **grant** is configurable.
- **No change to the tenant/org boundary.** RLS org-isolation is immutable and **not policy-driven**.
- **No platform-tier configurability.** `PlatformFeature` / `PlatformPrincipal` stay separate and fixed.
- **No AI policy decisions in this slice.** Future AI/ML/RL/LLM support may summarize or recommend only
  after `docs/specs/operations-intelligence.md` prerequisites are met; authorization remains a
  deterministic, auditable policy decision.

## 2. Target data model (per-tenant, RLS-armed)

Target model for the effective-policy resolver. G016-P0 implements the production substrate called out
in §9 (`feature_catalog`, `policy_roles`, `policy_role_permissions`, reserved
`user_role_assignments`, `policy_versions`) but **does not use it for live authorization yet**. All
tenant tables are tenant-scoped, `FORCE ROW LEVEL SECURITY`, `org_id` column, owner-applied, `GRANT`ed
to `console_rt`, RLS policy `org_id = current_setting('app.current_org')::uuid`.

```
roles
  id uuid                                     -- pk
  org_id uuid not null                        -- tenant boundary (RLS)
  key text not null                           -- stable slug, unique per org; system keys reserved
  display_name text not null                  -- shown in console (Korean copy lives in ko.ts, not here)
  kind text not null                          -- 'system' | 'custom'  (system rows immutable)
  is_assignable boolean not null default true
  created_at / updated_at / created_by
  primary key (id)
  unique (org_id, id)                         -- COMPOSITE target so children FK on (org_id,*) — proves same-org
  unique (org_id, key)

feature_catalog                               -- R7: single-sourced from Feature::ALL (seed migration)
  key text                                    -- pk; one row per Feature variant (snake_case)
  primary key (key)

role_permissions
  org_id uuid not null
  role_id uuid not null
  feature text not null
  level text not null                         -- 'deny'|'limited'|'request_only'|'allow' (PermissionLevel)
  primary key (org_id, role_id, feature)
  foreign key (org_id, role_id) references roles(org_id, id) on delete cascade   -- R4-fix #4: composite FK proves same-org
  foreign key (feature) references feature_catalog(key)                          -- R7: no free-text catalog injection

user_role_assignments
  org_id uuid not null
  user_id uuid not null
  role_id uuid not null
  granted_by uuid / granted_at
  primary key (org_id, user_id, role_id)
  foreign key (org_id, role_id) references roles(org_id, id) on delete cascade   -- composite FK: assignment's role is same-org
  foreign key (org_id, user_id) references users(org_id, id)                     -- and same-org user (users must carry the composite key)
```
**Cross-tenant reference is unrepresentable by construction** (review-gate finding #4): the children
FK on `(org_id, role_id) → roles(org_id, id)` (and `(org_id, user_id) → users(org_id, id)`), so a row
whose `org_id` differs from its role's/user's `org_id` is rejected at write — RLS confines the row, the
composite FK confines the *reference*. An `console_rt` test must prove a mismatched-org assignment INSERT
fails.

- **System roles** are seeded (migration) as `kind='system'` rows per org with `role_permissions`
  exactly mirroring today's `matrix_row()` — so day-0 behavior is byte-identical. System rows are
  **immutable** (DB rule / app guard: no UPDATE/DELETE of `kind='system'`; their permissions are the
  baseline floor and the upgrade target if the catalog grows).
- A user's **effective policy** = union over assigned roles of `max(level)` per feature (most-permissive
  wins, same as today's "any role satisfies"). Scope reach (branch vs org) is a property of the role
  (see §5 `OrgWideQueueTriage`) resolved into `BranchScope`, **never** a way to cross `org_id`.

## 3. Authz engine change

- Shipped P1a behavior: `authorize()` keeps its signature and unions the immutable system-role matrix with
  per-request `EffectiveFeatureGrant`s resolved from active assigned custom roles. `permission_for(role, feature)`
  remains the **system-role floor** and migration seed.
- Future P1/P2 target: move additional ABAC/PBAC attributes and no-lockout/escalation closure into the same
  resolver without changing call-site authorization semantics. The Cedar/PBAC target baseline is a typed
  `AuthzEngine` boundary: built-in/custom roles become subject inputs or generated policy-bundle material,
  Cedar evaluates capabilities/actions, and Postgres `console_rt`/RLS remains the hard row boundary.
- Current bridge resolution is **per-request, RLS-armed, and cached** with the cache keyed by **`(org_id,
  policy_version)`** (**R4** — *not* TTL/`org_id`-only): every `RoleManage` write bumps the per-org
  `policy_version` (an RLS-armed row) **synchronously before the write returns**; resolution reads the
  version (one cheap armed read; deny on read failure) and treats a version mismatch as a cache miss, so
  a revoke is globally effective on the next request across all replicas with no inter-node messaging.
  The hot path stays O(1) and never does an unarmed read. The Principal carries resolved built-in roles plus
  additive custom-role feature grants; the effective capability set is the union over built-in + assigned custom
  roles, most-permissive per feature. Cedar v1 may cache compiled bundle material only, keyed by immutable
  policy/schema/bundle/source versions; it must not cache cross-request allow decisions.
- **Fail-closed:** unknown feature string, missing policy row, unarmed read, or cache miss under load →
  **deny**, never allow. Default for any (role,feature) not present = `Deny`.

## 4. Hard invariants (NON-NEGOTIABLE — a security-reviewer must verify each)

1. **Tenant isolation is immutable.** No role, policy, or assignment can read/write across `org_id`.
   All three new tables are FORCE RLS + armed + have `console_rt` tests proving cross-tenant invisibility.
   Policy configures *capabilities within a tenant*, never the tenant boundary.
2. **No privilege escalation via configuration.** Defining/assigning a role can **never grant a
   capability the actor does not themselves hold** ("grant ≤ self"). Enforced server-side on every
   `RoleManage` write, audited, and tested (T: admin cannot mint a role with `ElevatedRoleGrant` or
   `SUPER_ADMIN`-only capabilities and assign it to self/another).
3. **System roles immutable; capability catalog fixed.** Only the **assignment** of the existing
   `Feature` set is editable. No SQL/console path creates a new `Feature`. `SUPER_ADMIN` retains the
   full set; `kind='system'` rows cannot be edited/deleted.
4. **No lockout.** The system guarantees at least one assignable role in each org holds `RoleManage`
   **and** `ElevatedRoleGrant` (the system `SUPER_ADMIN`); a `RoleManage` write that would remove the
   last such grant is rejected.
5. **`ElevatedRoleGrant` still gates elevation.** Assigning a role that confers elevated capabilities is
   itself an elevated action (matrix: `ElevatedRoleGrant` = `SUPER_ADMIN` only). Self-approval rule
   unchanged: only 대표/CEO + `SUPER_ADMIN` may self-approve (see [[operations-through-console-only]]).
6. **Platform tier untouched.** No bridge from `PlatformFeature` to `Feature`; platform principals carry
   no role/policy.
7. **UI projections are non-authoritative.** Policy Studio/navigation may display effective access previews,
   but stale/elevated projection data cannot unlock `RoleManage`-tier routes or grant API access; protected
   routes and endpoints reauthorize server-side.
8. **Everything audited.** Every role create/edit/retire/assign/unassign emits an `AuditEvent`
   `.with_org(org)` (arms the GUC) — through the console API, never direct SQL.

## 5. New capabilities (compose the existing catalog)

Two new `Feature` variants (matrix cells default to **least-privilege**, then become tenant-configurable):

- **`RoleManage`** — define/edit/retire custom roles + assign roles to users. Default cells
  `[D,D,D,D,D,A]` (**SUPER_ADMIN only**). The console role-editor + assignment UI gate on this.
- **`OrgWideQueueTriage`** — read the **org-wide** work-order + daily-plan queues regardless of branch
  membership (the legitimate "central triage desk" reach). Default cells `[D,D,D,D,A,A]`
  (**EXECUTIVE + SUPER_ADMIN**, matching `resolve_branch_scope_in_org`'s org-wide set). This is the
  **bridge fix for the codex HIGH-1 finding**: `work_order_list_scope()` returns `BranchScope::All`
  **iff** the principal holds `OrgWideQueueTriage` (replacing the hardcoded `is_admin_like → All`
  widen). Day-0 effect: branch `ADMIN`s become correctly branch-scoped; org-wide triage flows to
  EXEC/SUPER_ADMIN. Once configurable RBAC ships, **"org-admin" is simply a custom role with
  `OrgWideQueueTriage = Allow`** — the user's chosen "org-admin grant", now data-driven.

> **Historical live-migration concern (HIGH-1):** do not act on the former KNL
> role assumptions. Before any later cutover, an authorized operator must first
> re-establish the live tenant, affected principals, and change authority, then
> prove that scope is not silently reduced.

## 6. Historical/target console UX — HOLD until the Leptos gate

- **Former React Policy Studio (deleted by the pivot):** its list/editor/preview UI
  is historical evidence only. The backend feature catalog, role definitions,
  assignment preview, `policy_version`, passkey gate, and fail-closed resolver
  remain source-implemented mechanisms; they do not establish a current UI or
  production exposure.
- **Future Roles page** (gated on `RoleManage`): list system + custom roles; create/edit a custom role via a
  capability matrix editor (feature × level, grouped by domain) with a **diff-from-baseline** view and
  a **"grant ≤ self" preview** that greys out capabilities the actor lacks. Retire (not hard-delete)
  custom roles that still have assignments only after reassignment.
- **Future User detail**: assign/unassign roles (multi-role), each assignment audited; show effective
  capabilities (read-only rollup) so an admin sees the *net* of multiple roles.
- **Policy lifecycle UI**: draft, diff, simulate, approve, activate, rollback, and retire policy
  versions. Show affected users, employment states, departments/teams, responsibilities, and example
  decisions before activation.
- **Employment-transition UI**: onboarding, transfer, leave/suspension, termination/retirement, and
  rehire flows must update credential status, policy assignments, responsibilities, queue scope,
  and retention state together, with audit and handoff tasks.
- Any future Leptos surface must establish its own localization and safe-label
  conventions; the deleted `ko.ts`/`safeLabel`/`ObjectLink` machinery is not a
  current implementation path.

## 7. Migration & compatibility

- Seed `kind='system'` roles + `role_permissions` from `matrix_row()` for every existing org (and on
  tenant-create), under a golden full-grid parity test keyed by enum (**R5**). Existing users' role
  strings map 1:1 to the seeded system roles → **zero behavior change at cutover** (byte-identical
  policy), then custom roles are additive.
- **Per R3:** the token continues to carry only the **built-in role identifiers** (`users.roles` stays
  system-only, CHECK unchanged); **custom-role assignments live in `user_role_assignments`** and the
  resolver unions system-role policy (from the token) with those per-request armed reads. Custom keys
  never flow through `Role::from_str`; key/feature/level resolution is **parse-or-deny**. Cache is keyed
  by `(org_id, policy_version)` (**R4**).

## 8. Threat model — what the security-review must adversarially probe

- Cross-tenant: can org A's admin create/read/assign a role in org B? (RLS + armed-read tests.)
- Escalation: can a non-SUPER_ADMIN mint/assign a role exceeding their own grant? self-grant
  `ElevatedRoleGrant` / `RoleManage`? assign a system-equivalent super role?
- Lockout/DoS: can a sequence of edits leave an org with no `RoleManage` holder?
- Confused-deputy: does any existing endpoint authorize on a **role string** rather than a **capability**
  (so a renamed/custom role bypasses it)? (Audit every `matches!(role, …)` call-site; `is_admin_like`
  is one — convert to capability checks.)
- Cache poisoning / staleness: can a revoked capability remain effective past invalidation? fail-open on
  cache miss? (Must fail-closed.)
- Catalog injection: can a crafted `feature` string enable an unintended capability or bypass the
  `Feature` CHECK? (Parse-or-deny.)

## 9. G016 backend source status (former UI/client slice deleted)

This slice exists because tenants need to create many named roles (for example department managers, group operators, payroll clerks, site-only supervisors) without adding more hard-coded enum roles. It deliberately starts with a safe, auditable role catalog and policy preview instead of weakening the authorization boundary.

**Present in repository source now**

- Add the tenant feature `RoleManage` to the canonical feature catalog. Only `SUPER_ADMIN` holds it initially (`[D,D,D,D,D,A]`).
- Add tenant-scoped custom role tables with RLS, immutable `org_id`, console_rt grants, feature FK validation, and per-org `policy_version` bumps.
- Add `/api/v1/policy/features` and `/api/v1/policy/roles` so a RoleManage holder can see the capability catalog and create custom role definitions with explicit permission cells.
- Surface the per-tenant `policy_version` in the role catalog response. Version 0
  means no policy write has happened; every role/custom-assignment write bumps the monotonic version
  under RLS for the future `(org_id, policy_version)` resolver cache.
- Custom role definitions may grant ordinary operational features only. Admin escalation features (`RoleManage`, `ElevatedRoleGrant`) and the scope-widening `OrgWideQueueTriage` stay system-role-only until no-lockout/self-bounded grant proofs and richer scope publication land together.
- The former React Policy Studio page was deleted by the pivot. No current UI is
  implemented or authorized by this spec.
- Identity-console S2 account/person mutations use the same impact-preview receipt contract as Policy Studio:
  `POST /api/v1/policy/users/{id}/assignment-preview` accepts `role_ids` plus optional `system_roles` and
  `branch_ids`, returns current/requested system-role and branch-scope sets with a short-lived
  `preview_receipt_id`, and `PATCH /api/v1/users/{id}` must include `preview_acknowledged: true` plus that
  receipt whenever replacing `roles` or `branch_ids`. Account lifecycle uses `POST /api/v1/users/{id}/deactivate`
  to archive/offboard and revoke credentials/sessions, and `POST /api/v1/users/{id}/activate` to reactivate an
  archived user without recreating credentials; the backend OpenAPI document exposes
  `AccountStatus = ACTIVE | PENDING_SETUP | ARCHIVED`. Generated clients are deleted.

**Out of scope for this slice**

- Custom role assignments do **not** replace `users.roles`; built-in roles remain system-role-only token claims.
- Custom role definitions do **not** widen `BranchScope::All`, group scope, or platform scope. Those scopes remain resolved by the existing membership/token systems.
- Runtime authorization overlays are intentionally additive and fail closed: ACTIVE custom roles grant ordinary features inside the caller's live branch scope, while DRAFT/RETIRED roles, unsupported conditions, and elevated/scope-widening features stay inert.

**Backend verification condition**

- Focused REST tests prove an authorized principal can inspect the feature catalog,
  create a custom role such as `maintenance_manager`, and reload it through the
  tenant-scoped API; this is not a current UI or live-user-story claim.
- Attempts to define elevated policy features are rejected by the REST boundary.
- ACTIVE assigned custom roles are resolved into ordinary feature grants on the next request principal, without widening branch/group/platform scope. Runtime-effective conditions are intentionally limited to data-backed branch narrowing and team matching (`equals`/`in`); other metadata-only ABAC/PBAC condition rows remain visible for preview/audit and fail closed until their source-of-truth attributes exist.
- Backend OpenAPI drift, focused Rust tests, fmt, clippy, and the applicable
  authorization/RLS gates stay green on the exact candidate.

**Why this is not a stub**

The persisted role catalog, feature FK, audit row, RLS policy, policy-version bump, OpenAPI text, and central effective-policy resolver are non-stub repository mechanisms. Their presence is not production or exposure evidence. The remaining withheld behavior is the dangerous part: elevated self-granting, org-wide scope widening, group/platform policy fan-out, and richer ABAC/PBAC evaluation (department, position, employment status, assignment, location, purpose/action/resource) before the required source-of-truth attributes, preview/no-lockout/self-bounded checks, and review evidence exist.

## 10. Phased delivery (new ultragoal goal — slots after G002 org-hierarchy)

0. **P0 — capability hygiene + HIGH-1 bridge (ships in G001/now, code-only, no new tables):** add
   `OrgWideQueueTriage`, route the workorder queue/daily-plan `BranchScope::All` widen through it (the
   `is_admin_like` today-holes), fix codex HIGH-2 (gate `list_daily_plans` on
   `DailyPlanRequest`∪`DailyPlanReview`) — all with `console_rt` tests. Closes the live finding
   forward-compatibly. *(Scope here is only the workorder today-holes; the full R1/R6 conversion is P1.)*
1. **P1 — de-string authorization + the `BranchScope::All` chokepoint (R1, R6) + data model + engine:**
   convert **all** 18 role-string sites to capability checks — especially the escalation/IDOR guards
   (`identity/rest:787`, `auth-rest` OTP/credential gates, `financial` approval router) — and collapse
   the ~13 `BranchScope::All`-from-role producers into one capability-driven decision fn; add the CI gate
   forbidding role-string authz. Then: migration (3 tables + `feature_catalog` (R7) + per-org
   `policy_version` (R4), RLS, GRANTs, seed-from-`matrix_row()`), resolver + `(org_id,policy_version)`
   cache, fail-closed parse-or-deny (central additive resolver landed in G016-P1m; branch+team runtime
   condition parity landed in G016-P1r; cache and richer data-backed ABAC/PBAC remain follow-up). `console_rt` RLS tests incl. the **armed cache-warm** proof (R8) and the
   golden full-grid parity test (R5). Add the new tables to `console-gate-rls-arming`.
2. **P2 — RoleManage API + escalation closure (R2) + no-lockout (R5):** create/edit/retire/assign
   endpoints (openapi-first, regen clients); grant-≤-self over the post-union effective set at assignment
   time; `RoleManage` self-bounded; named `[D,…,A]` allow-list gates assignment; holder-floor under
   advisory-lock/serializable. Full audit. Escalation + concurrent-revoke + lockout tests. **Gate:
   custom-role assignment must NOT be enabled in prod until every R1 escalation-guard conversion (P1) has
   landed + been re-reviewed** — else a custom super-role bypasses the OTP/credential/grant guards.
3. **P3 — console UX:** Roles page + capability editor (grant-≤-self preview) + user role-assignment
   (Blueprint, AA, ≥90).
4. **P4 — "org-admin" as first custom role** (`OrgWideQueueTriage=Allow`; proves the loop) + docs.

**Test strategy (per slice):** real `console_rt` RLS round-trip + cross-tenant invisibility for each new
table; golden parity test that seeded system policy == `matrix_row()`; adversarial escalation/lockout
tests; gates + fmt + clippy + `check:platform-contract-drift` green; security-review as a separate pass.

## 11. Open decisions (recommended defaults in **bold** — confirm or override)

1. **Capability granularity:** **compose existing `Feature` catalog only** (new capabilities deferred to
   ontology-actions / G010). _Alt: allow defining new capability primitives now (much larger; couples to
   the action engine)._
2. **Role locus:** **per-tenant custom roles first**; group-level shared roles (define once, apply across
   subsidiaries) as a follow-on tied to `org-hierarchy.md` group grants. _Alt: build group-level now._
3. **Built-ins:** **immutable system roles + custom on top** (no lockout, safe upgrades). _Alt: fully
   editable built-ins (rejected — lockout/upgrade risk)._
4. **Multi-role users:** **yes, union-of-roles, most-permissive-per-feature** (matches today's "any role
   satisfies"). _Alt: single role per user (simpler, less flexible)._
5. **Scope reach as policy:** **model org-wide reach as the `OrgWideQueueTriage` capability** (and future
   `*OrgWide` capabilities), not a free-form scope DSL — keeps RLS the hard floor. _Alt: per-role scope
   expression (more power, larger attack surface)._
