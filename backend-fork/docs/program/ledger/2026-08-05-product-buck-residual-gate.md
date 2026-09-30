# Ledger: product Buck residual fail-closed gate (ADR-0039 step 1 partial)

**Date:** 2026-08-05  
**Kind:** CI / equivalence evidence  
**Related:** ADR-0039 (proposed), DN-0005

## Outcome

After S2 cargo PG facets, seven residual `//tools/buck:` product wrappers remain
in `ci.yml` (company-conformance + backend disposable PG). New gate
`tools/ci/check-product-buck-residual.mjs`:

- PG facet block must list **zero** Buck wrappers
- Every residual wrapper must appear in `postgres-cargo-map.json` (mapped or documented unmapped)
- Residual count ceiling **7** (measured post-#583); may only shrink

Wired into `npm run check:ci-preflight`.

## Residual inventory (ceiling)

mapped: app-dev-auth-persona-guard-postgres, auth-rest-dev-auth-group-admin-postgres,
auth-rest-dev-auth-session-postgres, company-conformance-postgres,
provisioning-dev-principal-upsert-race-postgres  
unmapped (documented darkness): app-inline-postgres, auth-rest-dev-auth-inline-postgres

## Next

P3 nextest serial group · P4 switch residual to cargo · P5 drop Buck product jobs

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "other"
  ],
  "selected_lenses": [
    "Essentialism / YAGNI",
    "Chesterton's Fence",
    "Pragmatism",
    "Red Team",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Essentialism / YAGNI": "Gate only; no residual cutover yet.",
    "Chesterton's Fence": "Keeps residual Buck until map-covered switch.",
    "Pragmatism": "Ceiling + map classification from measured post-S2 inventory.",
    "Red Team": "Unknown Buck wrappers in CI fail closed.",
    "Operability / Day-2": "npm script + preflight wire; residual list is printed.",
    "Blast-radius / cell-based": "tools/ci + package.json + ledger; no ci.yml job graph.",
    "Telemetry-first": "OK line enumerates residual names and ceiling.",
    "Zero-trust / defense-in-depth": "Unmapped requires documented reason; no silent ghosts."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Post-S2 residual Buck product surface is exactly 7 wrappers."
  ],
  "decisions_changed_or_rejected": [],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
