> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Sub-Spec: ORG-HIERARCHY — Conglomerate Group → 법인 → Region → Branch → Worksite

**Story:** G002 (Track B0) · **Status:** P0 schema/resolvers IMPLEMENTED (`6c7d121`); P1 AccessScope
kernel bridge IMPLEMENTED; P2 JWT claims + principal legacy-default + tenant-route enforcement IMPLEMENTED; P3
consolidated-read helper IMPLEMENTED; P4a platform-operator bridge LOCAL/UNRELEASED; P4b+ tenant-tier
group authz/API/UI/security-review scope remains open · **RLS posture:** the per-법인 `app.current_org`
boundary is **UNCHANGED**. This spec adds a *controlled cross-entity scope above* that boundary; it never
punches a hole in it.

## 0. Security-review revisions (applied — review verdict was MUST-REVISE-DESIGN-FIRST)
The keystone architecture (N armed per-member reads, no BYPASSRLS on the data path) PASSED and is kept.
These hardening fixes are now binding on implementation:
- **FIX-1 (was HIGH 8a — cross-tenant disclosure of the auth table):** `group_memberships` and
  `group_role_grants` are **NOT** on the tenant-isolation GLOBAL allowlist and `console_rt` has **NO** raw
  SELECT on them. They are owner-only; the runtime role reads them ONLY through identity-only SECURITY
  DEFINER resolvers that return the caller's OWN grants / their group's member ids (mirroring
  `platform_resolve_token_org`'s "nothing but the tenant id" blast radius). `groups` may carry a minimal
  `console_rt` SELECT (name/slug/status only — no authorization data) or also go through a resolver.
- **FIX-2 (was HIGH 8b — gate blind spot):** the consolidated-read helper lives in a **gate-SCANNED**
  crate (`backend/crates/platform/group/` — new) and *calls* `with_org_conn` from the db crate for member
  reads; it is NOT placed under `/platform/db/` (which the rls-arming gate skips). So a bare-pool read in
  the helper is caught by CI.
- **FIX-3 (was MEDIUM — C1 unenforced):** `scope_node` is a distinct newtype `ScopeNodeId` with **no**
  `Into<OrgId>` and no path to `CURRENT_ORG.scope`/`set_config('app.current_org')`; add a static-gate
  assertion that `app.current_org` is only ever armed from the verified `org` claim or a resolver-returned
  member `OrgId` — never `scope_node`. (Acceptance criterion of P2/P3.)
- **FIX-4 (was MEDIUM — blocking):** migration renumbered **0052 → 0060** (head is 0059); all §References
  re-verified against the current tree before P0.
- **FIX-5 (was MEDIUM — TOCTOU):** the member set is re-resolved LIVE per request for BOTH consolidated
  reads AND cross-entity writes (never cached in token/principal); `switch-context` re-validates the target
  is ACTIVE + still a member at mint time.
- **FIX-6 (was MEDIUM — resolver fragility):** the DEFINER resolver snapshots into a local array under
  `row_security=off`, restores `on`, THEN `RETURN QUERY` from the array (mirrors 0036 byte-for-byte), or
  wraps the body in an `EXCEPTION … restore row_security; RAISE` block.
- **FIX-7 (was LOW — tier):** a `view_as=true` token may **never** carry `group_roles` (an impersonation
  token can't be widened into a cross-entity writer) — asserted in P2 JWT issuance/verification + a test.
- **6 NEW console_rt TESTS** specified in §10: T13 cross-tenant read of group_role_grants returns own/zero rows;
  T14 group helper file is gate-scanned (bare-pool read flagged); T15 C1 negative — a group id can never
  arm app.current_org; T16 mid-session membership revocation drops the member on the next read AND write;
  T17 view_as+group_roles mutual exclusion; T18 resolver restores row_security on the inner-error path.

> Keystone invariant: *the consolidated group view is an aggregation over per-member ARMED reads, never a
> `BYPASSRLS` blanket read* — realized in §4, with the remaining `console_rt` coverage specified in §10. The static gates
> (`rls-arming`, `tenant-isolation`) stay green (§11).

## 1. Objective
Support a `Group → 법인(Org=RLS boundary) → Region → Branch → Worksite/Site` hierarchy on top of existing
single-tenant RLS so that: a **group-admin** manages/views all member 법인 in one consolidated screen,
switches to a single-법인 view, and performs audited cross-entity admin — reaching ONLY their own group's
members; a **법인-admin** stays locked to one entity (today's behavior); a **branch/worksite-local** user
is subtree-scoped with least privilege; sensitive cross-entity data (payroll/financials) stays per-법인
RBAC unless an explicit group role grants it. The **Org remains the single RLS hard boundary**
(`app.current_org`, the `org_isolation` policies in 0030, `console_rt` NOBYPASSRLS/FORCE-RLS in 0031, the
org_id-immutability triggers) — unmodified.
Non-goals: intercompany/elimination accounting (Track C); column/cell masking; no-code group ontology.
The vendor Platform tier (`PlatformPrincipal`, view-as) is a distinct higher tier, unchanged (§6 contrasts).

### 1.1 Platform-operator bridge (local P4a support slice, not tenant-tier group admin)
Implemented in the current local worktree, but not yet merged/released: platform-only
`/api/platform/tenant-context` start/exit endpoints let a vendor platform operator enter an audited,
short-lived writable `SUPER_ADMIN` tenant context pinned to one active 법인. Platform tenant/ops reads also
project identity-only group metadata (`group_id`, `group_slug`, `group_name`) so the platform console can
switch between 전체 / 그룹 / 개별 조직 views before isolating into one org.

This is the support bridge requested for a superadmin that can "see it all together" and also isolate into
each tenant org. It deliberately does **not** complete the tenant-tier group-admin surface in §5-§6: there is
still no tenant-tier group-role grant endpoint, no consolidated group write API, and no full org-tree /
personnel / site-responsibility / approval-graph UI.

## 2. Data Model + Migration
A Group is NOT a tenant and carries NO tenant data — pure grouping + grant target. Only `groups`
is a GLOBAL allowlisted table, limited to identity columns. `group_memberships` and
`group_role_grants` are cross-tenant authorization tables but remain owner-only (not RLS-scoped,
not `console_rt`-readable): group-admin fan-out must enumerate sibling members BEFORE any member GUC
is armed (same chicken-and-egg `platform_resolve_token_org` solves in 0036), so topology/grant
resolution is a narrow SECURITY DEFINER read, not a raw table grant.

```sql
-- 0060_create_groups_and_membership.sql  (house style: 0026/0036/0049; migration head is 0059) [FIX-4]
-- console-gate: global-table groups (rationale: holding topology — name/slug/status only, NO auth data)
-- group_memberships + group_role_grants are OWNER-ONLY (NOT global-allowlisted, NO console_rt grant): they
-- are the cross-tenant AUTHORIZATION tables. Runtime reads go ONLY through identity-only SECURITY DEFINER
-- resolvers (own-grants / own-group-members), mirroring platform_resolve_token_org's blast radius. [FIX-1]
CREATE TABLE groups (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  slug TEXT NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$'),
  name TEXT NOT NULL CHECK (name <> ''),
  status TEXT NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE','SUSPENDED','ARCHIVED')),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
ALTER TABLE organizations ADD COLUMN group_id UUID NULL REFERENCES groups(id) ON DELETE RESTRICT;
CREATE INDEX idx_organizations_group ON organizations (group_id) WHERE group_id IS NOT NULL;
CREATE TABLE group_memberships (
  group_id UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
  org_id   UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (group_id, org_id), UNIQUE (org_id));   -- an Org is in at most ONE group
CREATE TABLE group_role_grants (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  group_id UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
  user_id  UUID NOT NULL REFERENCES users(id)  ON DELETE CASCADE,
  group_role TEXT NOT NULL CHECK (group_role IN ('GROUP_ADMIN','GROUP_VIEWER','GROUP_FINANCE')),
  granted_by UUID NULL REFERENCES users(id), created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (group_id, user_id, group_role));
CREATE INDEX idx_group_role_grants_user ON group_role_grants (user_id);
REVOKE ALL ON groups, group_memberships, group_role_grants FROM console_rt;        -- owner-only by default
GRANT SELECT (id, slug, name, status) ON groups TO console_rt;                     -- topology identity only
-- group_memberships + group_role_grants: NO console_rt grant; exposed ONLY via the DEFINER resolvers. [FIX-1]
-- resolver body must snapshot under row_security=off then restore on (or wrap in EXCEPTION … restore). [FIX-6]

-- identity-only own-group resolver (SECURITY DEFINER, mirrors platform_list_organizations 0036).
CREATE OR REPLACE FUNCTION group_member_org_ids(p_group UUID, p_actor UUID)
RETURNS TABLE (org_id UUID, slug TEXT, name TEXT, status TEXT)
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp AS $$
DECLARE
  result_rows organizations[];
BEGIN
  SET LOCAL row_security = off;
  SELECT array_agg(o ORDER BY o.created_at ASC, o.id ASC)
  INTO result_rows
  FROM organizations o
    JOIN group_memberships m ON m.org_id=o.id
    WHERE m.group_id=p_group AND o.status='ACTIVE'
      AND o.id <> '00000000-0000-0000-0000-00000000face'::uuid   -- never the platform sentinel
      AND EXISTS (
        SELECT 1 FROM group_role_grants g
        JOIN users u ON u.id=g.user_id
        WHERE g.group_id=p_group AND g.user_id=p_actor AND u.is_active
      );
  SET LOCAL row_security = on;                                    -- restore before returning

  RETURN QUERY SELECT r.id,r.slug,r.name,r.status
  FROM unnest(COALESCE(result_rows, ARRAY[]::organizations[])) AS r;
EXCEPTION WHEN OTHERS THEN
  SET LOCAL row_security = on;
  RAISE;
END; $$;
REVOKE ALL ON FUNCTION group_member_org_ids(UUID, UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION group_member_org_ids(UUID, UUID) TO console_rt;

CREATE OR REPLACE FUNCTION group_role_grants_for_user(p_user UUID)
RETURNS TABLE (group_id UUID, group_role TEXT)
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp AS $$
DECLARE
  result_rows group_role_grants[];
BEGIN
  SET LOCAL row_security = off;
  SELECT array_agg(g ORDER BY g.group_id, g.group_role)
  INTO result_rows
  FROM group_role_grants g
    JOIN users u ON u.id=g.user_id
    WHERE g.user_id=p_user AND u.is_active;
  SET LOCAL row_security = on;

  RETURN QUERY SELECT r.group_id,r.group_role
  FROM unnest(COALESCE(result_rows, ARRAY[]::group_role_grants[])) AS r;
EXCEPTION WHEN OTHERS THEN
  SET LOCAL row_security = on;
  RAISE;
END; $$;
REVOKE ALL ON FUNCTION group_role_grants_for_user(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION group_role_grants_for_user(UUID) TO console_rt;
```
Key decisions: `group_id` NULLABLE → an ungrouped 법인 is unchanged (backward-compat free). Group roles
live in the owner-only `group_role_grants`, NOT `users.roles` — a tenant role array must never silently confer
cross-entity reach (same separation as `PlatformPrincipal` is not a `Role`). `console_rt` SELECT-only; grants
are written via an audited DEFINER path (mirrors `platform_create_organization`). Operational rows carry
NO `group_id` — a second weaker isolation axis is explicitly rejected; the subtree stays isolated by the
existing `org_isolation` policies.

## 3. AccessScope — generalizing BranchScope
`BranchScope { All | Branches[] }` (kernel/core/src/branch.rs) is the **intra-org** projection; add an
**inter-org** anchor `AccessScope { level ∈ {Group,Org,Region,Branch,Worksite}, node_id }` (new
kernel/core/src/access_scope.rs) resolved at login from the principal's assignment + carried in the JWT.
Bridge `AccessScope::branch_scope_for_org(org) -> BranchScope` (fail-closed `none()` for an org the scope
doesn't cover): Group→`All` per member (only while iterating that member's armed read), Org→`All` for that
one org / `none()` elsewhere (= today's 법인-admin), Region→`Branches([region's branches])`,
Branch→single, Worksite→branch + a worksite-id predicate. The existing `authorize()`/`repository_filter()`
consume the projected `BranchScope` unchanged.
JWT: add `#[serde(default)]` claims `scope_level`, `scope_node`, `group_roles` (legacy pattern of
`platform`/`view_as`). **Backward-compat (mandatory):** no scope claims ⇒ `AccessScope{Org, org_claim}` =
today's behavior. **Security-critical invariant (C1): `app.current_org` is ALWAYS a real Org id, NEVER a
Group id** — the `org` claim (validated as a real-tenant UUID at verify) stays the sole RLS arming input;
a Group-scoped token still carries `org`=the member currently being viewed (the scope selector, §5), so
the tenant middleware arms exactly one real org per request.

Implementation hardening now applied: every tenant REST/WS principal extractor calls the shared
`console_platform_request_context` resolver (or its bearer-token variant for WebSocket subprotocol auth). The
resolver re-reads live `user_branches`, intersects that live membership with the JWT `AccessScope`, and
rejects `AccessScope::Group` on ordinary tenant routes; group scope must use the §4 fan-out helper instead.
`group_roles` are canonical uppercase codes only (`GROUP_ADMIN`, `GROUP_VIEWER`, `GROUP_FINANCE`) and are
validated on issuance and verification.

## 4. THE KEYSTONE — RLS-preserving consolidated reads
> A group-admin "one screen" view is an AGGREGATION over per-member ARMED reads. The helper iterates the
> group's members; for EACH it opens a tenant-scoped tx, arms `app.current_org` to THAT member, runs the
> EXISTING RLS-correct read, and concatenates. NEVER a `BYPASSRLS`/`row_security off` blanket scan. The
> per-법인 boundary is exercised once per member, unchanged — N invocations of the existing mechanism.

`backend/crates/platform/group/src/lib.rs` (new gate-scanned crate, not under `platform/db`): `group_member_orgs(pool, group, actor)` (the identity-only DEFINER read →
ACTIVE members only when `actor` has a live group grant) + `consolidated_read(pool, group, members, read)`
that, for each authorized member, awaits `read(org)` — which IS `with_org_conn(pool, org, |tx| existing
SQL)` — and tags results by source Org. **No new SQL/policy/DEFINER for the row data**; only the member-id
LIST comes from the DEFINER resolver (exactly like view-as resolves the target-org name before flowing
through the normal armed path).
Fail-closed: empty resolver ⇒ `Ok(vec![])` (never a global scan); each `read(org)` arms its own GUC (FORCE
RLS returns 0 rows if unarmed); the member set passed in is `group_members ∩ principal.AccessScope reach`
(a foreign org isn't in `group_member_org_ids(G, actor)` and the principal has no role there anyway); the only
`row_security off` is the identity-only resolver, restored before return; `console_rt` is NOBYPASSRLS.

## 5. Scope Selector
One shared shell control. Privileged (group-admin): full ladder `[All subsidiaries] · [KNL]·[COSS]·
[BESTEC]·… · ▸region ▸branch`; "All" → consolidated endpoints (§4); a single 법인 → `/api/group/switch-
context` re-mints a tenant token whose `org`=that member after re-checking the grant (a deliberate, audited
`group.context.switch`). Scoped user: sees ONLY their subtree, NO toggle above their anchor — enforced
server-side (switch-context re-validates against `group_role_grants`+`AccessScope`, 403s anything outside
reach; UI hiding is convenience, the server gate is the control). Re-mint (not an omni-token) keeps
`app.current_org`=exactly one real Org per request; the consolidated screen is the only cross-member view
and it spans members via fan-out, not a multi-org GUC.

## 6. Cross-Entity Admin
A group-admin mutation on a specific member: (1) handler resolves the target Org + verifies it is in the
principal's group via `group_member_org_ids(group, actor)` (403 otherwise); (2) the mutation arms THAT target 법인 and
runs the EXISTING audited write (`with_audit` + `AuditEvent.with_org(target_org)`); the org_id-immutability
trigger still applies. (3) audit records the REAL group-admin actor + target org + `group.cross_entity.*`.
Reach: ONLY members of the principal's group; unrelated groups/orgs are absent from the resolver +
unreachable. Cross-group is architecturally impossible (no resolver maps one group → another's members).
Distinct from Platform view-as: a group-admin is a TENANT-tier principal (`platform=false`) who can WRITE
but only within their group's members; view-as is vendor-tier + read-only + can target any tenant. The two
never overlap (the platform extractor rejects a tenant token on `/api/platform/*`).

## 7. Markings / Least Privilege (conjunctive)
Payroll/financial consolidated reads are NOT group-visible by default — require BOTH `scope_level=group`
AND the `GROUP_FINANCE` group role (conjunctive, default-deny); a `GROUP_ADMIN` without `GROUP_FINANCE`
sees consolidated operations but 403 on consolidated payroll. Within a member, payroll stays per-법인 RBAC
(the group layer ADDS a marking, never REMOVES a per-법인 restriction; a group-finance read of COSS still
runs under COSS's GUC + passes COSS's per-법인 feature matrix). Local users (worksite/team) are subtree-
limited (Worksite scope → one branch + worksite predicate). Coarse role-gated marking (finance-vs-not),
not column/cell masking (deferred).

## 8. Authz Matrix Changes
The per-법인 matrix is UNCHANGED. A separate parallel capability set (same separation as
`PlatformFeature`↔`Feature`, no bridge): `GroupRole { GroupAdmin, GroupViewer, GroupFinance }`;
`GroupFeature { GroupConsolidatedRead, GroupMemberManage, GroupContextSwitch, GroupRoleGrant,
GroupFinanceRead }`. Viewer→{ConsolidatedRead,ContextSwitch}; Admin→Viewer+{MemberManage,RoleGrant};
Finance→{FinanceRead} (orthogonal). A GroupFeature NEVER confers a tenant Feature; a tenant Role NEVER
confers a GroupFeature. Effective authority = (group gate if a group endpoint) AND (per-법인 RLS, one
member) AND (per-법인 `authorize()` role×branch) — all default-deny, all must pass; a group role is
additive reach across members, never escalation within one. `Principal` gains `access_scope` +
`group_roles` (alongside `branch_scope`); existing fields untouched so `authorize()` compiles unchanged.

## 9. Security Checklist (each → a §10 test)
C1 app.current_org always a real Org id, never a Group id · C2 consolidated = N armed reads, zero BYPASSRLS
on the data path · C3 only `row_security off` is the identity-only resolver, restored before return · C4
group-admin sees ONLY their group's members · C5 cross-group impossible · C6 single-법인 user = today's
behavior · C7 worksite-local confined to subtree · C8 payroll consolidated requires conjunctive
GROUP_FINANCE · C9 every cross-entity action audited with real actor+target · C10 group-role grants only
via audited DEFINER; console_rt cannot self-grant · C11 consolidated helper unarmed/empty fails closed · C12
legacy tokens unchanged · C13 rls-arming + tenant-isolation gates green · C14 group-admin token
platform=false, rejected on `/api/platform/*`.

## 10. console_rt Test Plan (genuine NOBYPASSRLS runtime_role_pool; seed as owner row_security off)
T1 group_admin_consolidated_sees_all_members · T2 consolidated_excludes_non_member_org · T3
single_entity_scope_isolates (consolidated→403) · T4 cross_group_invisible · T5
worksite_local_least_privilege · T6 no_bypassrls_on_data_path (grep + resolver restores row_security) · T7
cross_entity_admin_arms_target_and_audits · T8 consolidated_helper_fails_closed_unarmed (never a global
read) · T9 group_finance_marking_is_conjunctive · T10 console_rt_cannot_self_grant_group_role · T11
legacy_token_without_scope_claims_is_org_scoped · T12 group_id_immutability_and_member_isolation · T13
cross_tenant_group_role_grants_raw_read_returns_own_or_zero_rows · T14 group_helper_file_is_gate_scanned
(bare-pool read flagged) · T15 group_id_cannot_arm_app_current_org · T16 mid_session_membership_revocation
drops_member_on_next_read_and_write · T17 view_as_and_group_roles_are_mutually_exclusive · T18
resolver_restores_row_security_on_inner_error_path.

## 11. Static-gate satisfaction
rls-arming: the helper lives in the new gate-scanned `backend/crates/platform/group/` crate, and every
member read calls `with_org_conn` from `console-platform-db` (executor `tx.as_mut()`, not a
bare pool). The helper must not live under `platform/db`, because that crate owns the arming primitives and
receives narrower gate treatment. The DEFINER call carries `// rls-arming: ok identity-only DEFINER resolver`.
tenant-isolation: only `groups` is added to the GLOBAL allowlist with a rationale comment;
`group_memberships` and `group_role_grants` stay owner-only and are exposed only through resolvers.
`organizations.group_id` is a nullable add to an already-RLS table (no new policy, like 0049).

## 12. Phased Implementation (each ≤~5 files, console_rt tests, separate review pass)
P0 schema + identity resolver (0060, done) → P1 AccessScope kernel type + BranchScope bridge (pure-logic,
done in `console-kernel-core`) → P2 claims + login resolution + live branch-scope intersection + legacy default
(done in `console-platform-auth` / shared principal resolver / tenant REST+WS adapters) → P3 consolidated-read helper (`platform/group`, done) → P4a platform-operator bridge
(local/unreleased: audited single-tenant management context + platform group/org scope filters) → P4b
tenant-tier group authz + group-role principal extension + conjunctive marking → P5 REST
consolidated/switch-context/cross-entity + audited grant endpoint → P6 tenant group scope selector and org
tree/personnel/site-responsibility UI (web, visual-verdict ≥90) → P7 security-review + checklist sign-off.

## 13. Open Risks / Decisions for the User
1. **Is payroll/financial group-visible at all?** Assumed default-NO, unlocked only by `GROUP_FINANCE`. If
   it must NEVER be group-visible, drop `GROUP_FINANCE` and keep payroll strictly per-법인.
2. **Group-admin = consolidated visibility AND cross-entity write?** Assumed yes; could split write behind
   a separate role + confirmation.
3. **How are groups provisioned?** Assumed platform-tier (vendor onboards a group + assigns members) — a
   법인 must NOT self-join a group. Confirm.
4. **Intercompany/elimination** (one entity billing another) is OUT of scope here (Track C). Confirm.
5. **Context-switch re-mint vs one multi-org session** — chose re-mint to keep one real Org per request.
   Confirm the brief context-switch UX is acceptable.
6. **Can a region/worksite scope span members?** Assumed NO (a sub-Org scope is always within one 법인).

## References (anchors verified)
authz/src/lib.rs:204-261 (matrix), :359-431 (PlatformFeature no-bridge precedent), :444/:559/:477;
kernel/core/src/branch.rs:14-47; request-context/src/lib.rs:93-97,141-142,200-202; db/src/audit_tx.rs:56-94,
219-241 (with_audit/with_org_conn); migrations 0030:13-19,31-84 / 0031:37,69-70,94-129 / 0036:65-130
(platform_resolve_token_org + list_organizations DEFINER + row_security off→on) / 0037 / 0049;
platform-rest/src/view_as.rs:219-235,298-340,380-411; auth/src/jwt.rs:57-95,211-213; auth-rest/src/lib.rs:
1368-1389; region_branch_..._runtime_role.rs:46-59,265-297,458-483; backend/ci/gates/{rls-arming,tenant-isolation}; docs/specs/knl-business-os.md §Tenancy; .omc/research/foundry-domain-research.md CAP-4.
