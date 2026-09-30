---
id: ADR-0021
status: accepted
doc_status: published
date: 2026-07-03
owner: jasonlee
acceptance_scope: target baseline only; live Cedar enforcement requires explicit promotion evidence
related: [ADR-0003, ADR-0004, ADR-0022, ADR-0023, ADR-0028, ADR-0032]
---

# ADR-0021: Cedar PBAC authorization strangler

## Status

Accepted as the authorization target baseline. This ADR does **not** switch live authorization.

## Context

The current shipped increment combines immutable system roles, custom role policy rows, and
per-request resolved feature grants. That bridge is useful, but the target product needs richer PBAC
(policy-based access control) over subject attributes, resource attributes, action, purpose, step-up
freshness, and legal/sensitivity context without letting UI projections or stale custom-role grants become
the source of truth.

`docs/specs/master-parallel-build-plan.md` already names a Cedar PDP adapter as the authz upgrade path:
Cedar decides capabilities, while Postgres RLS decides rows. The July 2026 access issue around stale or
mis-projected `RoleManage` affordances reinforced that the next cutover must be fail-closed, explicit, and
observable before any live switch.

Official source check on 2026-07-03:

- Cedar upstream release page marks `v4.11.2` as latest, released 2026-06-22, with a schema transitive-closure
  bug fix.
- crates.io API for `cedar-policy` reports `max_version = 4.11.2` and `updated_at = 2026-06-22T22:02:16Z`.
- Cedar language docs say the language version is distinct from the Rust SDK / `cedar-policy` crate version,
  and recommend using the latest SDK that supports the desired language version.
- Cedar schema and validation docs require schema-backed validation so incorrect entity/action/attribute
  references are caught before runtime authorization.

## Decision

Adopt a Cedar-backed PBAC strangler behind an internal `AuthzEngine` boundary. The first implementation
slices must keep live authorization unchanged until a specific action is enrolled in the coexistence map,
shadowed, audited, and promoted through explicit gates.

1. **PBAC via Cedar, not role-string RBAC.** Cedar becomes the target policy evaluator for capability
decisions. Built-in roles and tenant custom roles are subject inputs/policy bundle generators, not
   authoritative allow decisions by themselves.
2. **RLS remains the row boundary.** Cedar may allow an action on a resource type or object id, but every
   database read/write still runs under `console_rt` / armed RLS. Cedar cannot widen `org_id`, bypass RLS, or
   replace per-org consolidated-read rules.
3. **Server decisions are authoritative.** Web/mobile/desktop clients may receive policy projections for UX
   only. Navigation, button visibility, and Policy Studio previews are non-authoritative; every endpoint and
   mutation reauthorizes server-side.
4. **Compiled bundle cache only in v1.** The runtime may cache parsed/validated Cedar policy/schema/entity
   bundle material by immutable version/digest keys. It must not cache cross-request allow/deny decisions.
5. **Mutable subject freshness is load-bearing.** Authorization inputs include a subject freshness token such
   as `authz_subject_version` plus session/passkey/credential freshness. Role, assignment, responsibility,
   employment state, branch/team, or credential changes synchronously bump subject/policy versions so stale
   subject material cannot keep granting access.
6. **Dual-engine semantics fail closed.** During migration the legacy engine and Cedar engine coexist only
   through an explicit map. Enrolled actions have a mode (`legacy_only`, `cedar_shadow_legacy_enforce`,
   `cedar_enforce_legacy_compare`, or `cedar_only`). Missing map entries for enrolled Cedar domains,
   inconsistent versions, evaluation errors, stale bundles, stale subjects, or RLS arming failures deny.
7. **Source-pinned dependency decision.** When code first adds Cedar, use the upstream Rust SDK crate
   `cedar-policy` with a source-pinned exact version selected from official upstream evidence. As of
   2026-07-03 the candidate is `=4.11.2`. The implementation PR must re-check upstream release/crate metadata
   before editing Cargo manifests and must record the resolved crate version, Cedar language version, schema
   version, and bundle format version in audit/metrics.
8. **No live switch in this ADR/G001.** This baseline records the target contract only. Live routes remain on
   the current authorization behavior until later goals add contracts, tests, observability, shadow evidence,
   and an explicit promotion decision.

## Coexistence map

The canonical map for the first Cedar/PBAC cutover is `docs/specs/cedar-pbac-coexistence-map.json`. A route or
action is not Cedar-enrolled until it appears in that map with owner, mode, inputs, stale-deny requirements,
RLS proof requirements, and promotion evidence. The map is intentionally more conservative than ordinary
feature flags: absent or malformed entries deny for enrolled domains instead of falling back to allow.

## Consequences

- Cedar adoption is slower than directly replacing `authorize()` calls, but it avoids a high-risk partial
  migration where stale UI grants or unmodeled attributes can grant access.
- The existing custom-role bridge remains useful as bootstrap subject/policy data, while the target evaluator
  moves from role strings and feature cells to typed PBAC requests.
- Policy bundle lifecycle, schema validation, source pinning, audit events, metrics, and RLS tests become part
  of authorization correctness, not operational nice-to-haves.
- Any future remote Cedar agent/distribution fabric is a separate scaling decision. The initial target is an
  in-process SDK boundary with immutable bundle cache and no cross-request allow-decision cache.

## Verification baseline

Later implementation goals must prove these invariants before live promotion:

- stale policy bundle denies;
- stale subject freshness denies;
- Cedar allow cannot bypass `console_rt` / RLS row filtering;
- UI projection cannot expose or grant `RoleManage`-tier authority;
- dual-engine map absence/malformed mode/error disagreement fails closed;
- dependency/source pin is current at implementation time and recorded in audit/metrics;
- schema validation catches invalid action, entity, attribute, optional-attribute, and context references before
  a bundle is active.
