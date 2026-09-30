# Ledger — image-release registry BuildKit cache + GHA hygiene (2026-08-05)

## Identity

- Slice: B2 registry cache + automated GHA cache hygiene protocol
- Base: origin/main post-#576
- Files:
  - `.github/workflows/image-release.yml` — `cache-from`/`cache-to` `type=registry`
  - `.github/workflows/cache-hygiene.yml` — scheduled + post-release + manual protocol
  - `tools/ci/gha-cache-hygiene.mjs` — force / age / budget policy
  - `tools/ci/gha-cache-hygiene.test.mjs` — pure policy unit tests (no network)

## Decision

1. **Stop writing Docker BuildKit cache into Actions GHA storage.** Use GHCR refs  
   `ghcr.io/<owner>/console-app:buildcache-<arch>` with `mode=max` (safe on registry).
2. **Automated cleanup / hygiene protocol** (ordered, fail-closed on budget):
   1. **Force-delete** keys under `DELETE_PREFIXES` (`buildkit-`, `index-`).
   2. **Age-delete** non-`KEEP` keys older than `MAX_AGE_DAYS` (default 14).
   3. **Budget-LRU** non-`KEEP` until ≤ `MAX_TOTAL_BYTES` (default 8 GiB).
   4. **Fail the job** if usage remains over budget after deletes.
3. **Keep-list is sacred:** `v0-rust-` / `node-cache-` are never age- or budget-deleted.
4. **Triggers (all automated or one-click):**
   - Daily cron `17 6 * * *` UTC
   - `workflow_run` after **Image Release** success (immediate residual catch)
   - `workflow_dispatch` with `dry_run` + optional `max_age_days`
5. **Policy unit tests** run in the hygiene workflow before prune executes.

## Non-goals

- No change to multi-arch digests, Trivy, cosign, or production promotion
- No GHCR blob GC API (registry lifecycle is separate)
- No deletion of rust-cache / npm cache via budget or age

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "release",
    "other"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Essentialism / YAGNI",
    "Chesterton's Fence",
    "Pragmatism",
    "Red Team",
    "Systems Thinking",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "FinOps / unit-cost",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Inventory showed buildkit/index GHA keys not cargo caused the 10GB breach; fix moves Docker cache off Actions storage.",
    "Essentialism / YAGNI": "Registry cache refs plus one scheduled hygiene script with unit-tested policy; no registry GC service.",
    "Chesterton's Fence": "Keeps per-arch cache isolation (buildcache-amd64/arm64) so multi-arch builds do not clobber each other.",
    "Pragmatism": "GHCR already authenticated for image push; same login pushes buildcache tags.",
    "Red Team": "Hygiene never deletes v0-rust- or node-cache- prefixes; dry_run available on workflow_dispatch; policy unit tests gate prune.",
    "Systems Thinking": "Image-release no longer writes GHA blobs; hygiene removes residual type=gha leftovers, ages branch noise, and enforces 8GiB soft cap.",
    "Operability / Day-2": "Daily schedule + post Image Release + manual dispatch; step summary reports before/after usage and by_reason counts.",
    "Blast-radius / cell-based": "Only image-release cache lines and a non-required hygiene workflow; CI required contexts unchanged.",
    "FinOps / unit-cost": "Actions cache is a hard shared 10GB pool; registry storage is the correct place for multi-GB layer graphs.",
    "Telemetry-first": "Hygiene logs usage, victim counts by reason; fails if still over budget after cleanup.",
    "Zero-trust / defense-in-depth": "Trivy/cosign/provenance gates unchanged; cache policy is performance isolation not admission weakening."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "type=gha mode=max per image times arch filled ~10.3GB of the shared Actions cache.",
    "Registry cache keeps mode=max without competing with rust-cache.",
    "Automated hygiene: force buildkit/index, age non-KEEP 14d, budget-LRU to 8GiB; post-release + daily triggers."
  ],
  "decisions_changed_or_rejected": [
    "Rejected mode=min on GHA as the long-term fix; chose B2 registry cache.",
    "Rejected deleting rust-cache in hygiene; keep-list is explicit.",
    "Rejected schedule-only hygiene; added post Image Release workflow_run for residual catch."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
