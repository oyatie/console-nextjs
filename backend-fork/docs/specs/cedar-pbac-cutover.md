> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Cedar/PBAC cutover contract

> **Status:** DESIGN / GOVERNANCE BASELINE. No live authorization switch is made by this document.
> **Decision source:** `docs/decisions/ADR-0021-cedar-pbac-authorization-strangler.md`.
> **Coexistence map:** `docs/specs/cedar-pbac-coexistence-map.json`.
> **Verification/observability contract:** `docs/specs/cedar-pbac-verification-observability.md`.
> **Related bridge spec:** `docs/specs/rbac-configurable.md`.

## 1. Target outcome

Move from role-string / feature-matrix authorization toward Cedar-backed PBAC while preserving the current
production safety envelope:

- existing routes keep current behavior until explicitly enrolled;
- Cedar evaluates capability/action decisions through a typed server boundary;
- Postgres RLS remains the immutable row boundary;
- UI policy projections improve UX only and never grant authority;
- stale policy, stale subject, malformed map, or evaluation uncertainty denies.

## 2. Non-negotiable invariants

1. **Server-authoritative authorization.** Every protected API endpoint/mutation calls the server
   authorization boundary. Web/mobile/desktop projections, feature hints, local storage, JWT feature hints,
   and navigation state are advisory only.
2. **RLS row-boundary separation.** Cedar can decide whether an action is allowed; RLS still decides which
   rows exist for the runtime principal. No Cedar policy may set `app.current_org`, use owner/BYPASSRLS pools,
   or perform cross-org blanket reads.
3. **Compiled bundle cache only.** v1 may cache parsed/validated Cedar policies, schema, entity templates,
   and bundle metadata by immutable keys. v1 must not cache allow/deny decisions across requests.
4. **Mutable subject freshness.** A request subject carries freshness inputs covering policy version,
   role/assignment version, employment/person lifecycle version, branch/team/responsibility version, session
   generation, and passkey step-up freshness when required. If any required freshness input is missing, stale,
   or unreadable under RLS, the decision is `Deny`.
5. **Fail-closed dual-engine map.** Cedar and legacy authorization coexist only through an explicit map. Missing
   entries for enrolled domains, unsupported modes, stale bundles, stale subjects, Cedar diagnostics, legacy
   comparison failures, RLS arming failures, or audit write failures deny.
6. **Source-pinned Cedar.** The first dependency PR must pin the exact `cedar-policy` crate version selected
   from upstream release/crate evidence and record both SDK and Cedar language versions. As of 2026-07-03 the
   candidate exact pin is `cedar-policy = "=4.11.2"`, but implementation must re-verify before changing Cargo.
7. **No hidden live cutover.** This baseline and G001 do not enable Cedar on live routes. Promotion requires
   story-specific tests, shadow metrics, review, and map mode change.

## 3. Cedar request contract

Every Cedar evaluation request must be constructed from server-side data only:

```text
AuthorizationRequest {
  subject: SubjectRef + server_loaded_subject_attributes + freshness_versions,
  action: stable_domain_action,
  resource: ResourceRef + server_loaded_resource_attributes,
  context: purpose + environment + request_channel + step_up_state + policy_versions,
  rls_scope: org_id + branch/team/object scope that has already been armed for DB access,
  map_entry: coexistence_map_entry_id + mode + bundle_digest,
}
```

Client-submitted fields may identify the target action/resource, but the server reloads any attribute used for
policy evaluation. If an attribute is unavailable or optional, policies must use explicit presence checks and the
bundle must validate against schema before activation.

## 4. Bundle lifecycle and cache key

A bundle contains Cedar schema, policies/templates, entity-template strategy, generated default policy from the
system-role matrix when applicable, source evidence, and validation output.

The only allowed hot-path cache in v1 is compiled bundle material keyed at least by:

```text
(org_id, policy_version, schema_version, bundle_digest, cedar_sdk_version, cedar_language_version)
```

No key may omit `policy_version`, `bundle_digest`, SDK version, or language version. A cache miss re-validates or
loads compiled bundle material; a validation/load failure denies. A revoke or policy write bumps the relevant
version before returning so a later request cannot use stale compiled material as an allow.

## 5. Subject freshness contract

The server principal for Cedar carries `authz_subject_version` (name may change in code, semantics may not). It
must change when any authorization-relevant subject fact changes:

- system roles or custom role assignments;
- employment/person lifecycle state, including leave/suspension/termination/reactivation;
- department/team/branch/responsibility ownership;
- group/org membership and delegated admin scope;
- credential/session generation, passkey registration/removal, OTP recovery state, or required step-up state.

If the request carries a version older than the server-read version, or if the server cannot read the version
under the correct RLS context, the Cedar boundary denies and emits a stale-subject metric/audit event.

## 6. Dual-engine modes

| Mode | Enforcement | Required behavior |
| --- | --- | --- |
| `legacy_only` | Legacy engine only | Cedar is not on the request path. Use for not-yet-enrolled routes. |
| `cedar_shadow_legacy_enforce` | Legacy enforces, Cedar compares | Cedar denial/disagreement is audited/metriced but cannot grant. Cedar errors fail the Cedar side and trigger shadow-deny evidence. |
| `cedar_enforce_legacy_compare` | Cedar enforces, legacy compares | Cedar must allow for access; legacy disagreement is audited and may trip rollback/watch gates. |
| `cedar_only` | Cedar enforces | Requires prior shadow and compare evidence plus review. Legacy fallback cannot allow. |

A missing/malformed mode is `Deny` for any enrolled domain. `legacy_only` is the only valid default for legacy
routes not yet enrolled in the map.

## 7. UI projection contract

Policy Studio, navigation, and generated clients may show an `effective_policy_projection` for usability. That
projection:

- must be labeled non-authoritative in code/API docs;
- must carry the policy/subject version used to compute it;
- must never unlock RoleManage-tier route guards without the existing server-authoritative role/capability gate;
- must be invalidated/refetched when policy or subject freshness changes;
- must be tested so stale/elevated projections cannot expose or grant protected access.

## 8. Required verification before first live Cedar promotion

- The readiness fixture `backend/crates/platform/authz/tests/fixtures/cedar_pbac_readiness_cases.json`
  names the required fail-closed cases and is covered by executable boundary tests.
- Real `console_rt` tests prove Cedar allow cannot bypass org/branch/object RLS.
- Stale policy bundle fixture denies.
- Stale subject freshness fixture denies.
- Missing/malformed coexistence map entry denies for enrolled domains.
- Cedar schema validation fails invalid action, entity, attribute, optional-attribute, and context references.
- UI tests prove stale/elevated projection cannot expose `RoleManage`-tier surfaces.
- Audit/metrics include engine mode, policy version, schema version, bundle digest, Cedar SDK version, Cedar
  language version, subject freshness version, and deny reason.

## 9. Explicit non-goals for this baseline

- No Cargo dependency is added in G001.
- No production route switches to Cedar in G001.
- No remote Cedar agent/distribution fabric is chosen in G001.
- No allow-decision cache is introduced.
- No UI projection is treated as authorization evidence.
