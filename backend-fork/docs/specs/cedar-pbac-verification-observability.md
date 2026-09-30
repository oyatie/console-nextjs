> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Cedar/PBAC verification and observability readiness

> **Status:** DESIGN / TEST CONTRACT. No live authorization switch is made here.
> **Decision source:** `docs/decisions/ADR-0021-cedar-pbac-authorization-strangler.md`.
> **Cutover contract:** `docs/specs/cedar-pbac-cutover.md`.
> **Fixture:** `backend/crates/platform/authz/tests/fixtures/cedar_pbac_readiness_cases.json`.

## Purpose

Before the first Cedar pilot can move beyond `legacy_only`, the boundary must prove that uncertainty never
becomes an allow. This document records the minimum failure fixtures, metric labels, and audit fields required
for that proof. It complements the executable contract in `backend/crates/platform/authz/src/cedar_pbac.rs`.

## Required fail-closed fixtures

| Fixture id | Required fault | Expected decision |
| --- | --- | --- |
| `stale_policy_denies` | Cedar evaluation returns a bundle key that does not match the coexistence-map bundle key. | `deny / boundary_preflight / stale_policy_bundle` |
| `stale_subject_denies` | Subject freshness is older than the request's required policy, subject, session, or step-up generation. | `deny / boundary_preflight / stale_subject` |
| `rls_separation_denies` | The verified subject org differs from the server-loaded resource org before any Cedar allow can apply. | `deny / boundary_preflight / rls_boundary_mismatch` |
| `dual_engine_map_missing_denies` | An enrolled domain/action reaches the boundary without a coexistence-map entry. | `deny / boundary_preflight / missing_coexistence_map` |
| `dual_engine_disagreement_denies` | `cedar_enforce_legacy_compare` sees Cedar allow while legacy denies. | `deny / dual_engine / engine_disagreement` |
| `cedar_error_denies` | Cedar evaluation, schema validation, or adapter execution returns an error. | `deny / cedar / cedar_error` |

The fixture file is deliberately data-only. The Rust tests bind these fixture names to executable regressions so
a future implementation cannot silently rename or remove the first-pilot proof cases.

## Metric contract

Emit one authorization-decision counter per boundary decision after enforcement has been computed. Metric labels
must stay low-cardinality:

- `effect`: `allow` or `deny`;
- `engine`: `boundary_preflight`, `legacy`, `cedar`, or `dual_engine`;
- `reason`: the machine-readable decision reason;
- `mode`: the coexistence-map mode, or absent for missing-map denials;
- `domain`: coexistence-map domain, or absent for missing-map denials.

Do **not** put `request_id`, `resource_id`, `bundle_digest`, tenant ids, or user ids in metric labels. Those
belong in the audit event.

## Audit contract

Every Cedar/PBAC boundary decision must have an audit payload with:

- decision effect, engine, reason, and mode;
- coexistence entry id and domain when present;
- action, required permission, resource type, resource id, principal org id, resource org id, and branch id;
- request id, purpose, and request channel when present;
- subject freshness and required freshness;
- expected compiled bundle cache key when a map entry has one: org id, policy version, schema version, bundle
  digest, Cedar SDK version, and Cedar language version;
- evaluated compiled bundle cache key when Cedar returns bundle-bearing evaluation material, so stale-policy
  investigations can compare expected vs evaluated policy versions and digests;
- raw Cedar adapter deny/error reason detail when Cedar returns a human-readable diagnostic, kept in audit
  payloads only and never promoted to a metric label.

Audit write failure is itself a cutover blocker and must deny for live Cedar-enforced paths, as already recorded
in `docs/specs/cedar-pbac-coexistence-map.json`.

## First-pilot promotion gate

A pilot action may not leave `legacy_only` until all of the following evidence is present in the PR/ledger:

1. Fixture JSON validates and names all required fail-closed scenarios.
2. Rust boundary tests prove each expected denial reason.
3. Observation tests prove metric labels and audit payload include mode, reason, freshness, expected bundle
   versions, evaluated bundle identity for stale-policy cases, and raw Cedar deny/error details for diagnostics.
4. UI tests prove non-authoritative Cedar/JWT projections cannot expose RoleManage-tier pages.
5. RLS proof shows Cedar allow cannot bypass `console_rt` / tenant row boundaries.
