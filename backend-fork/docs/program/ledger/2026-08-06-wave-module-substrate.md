# Ledger: multi-module pure domain substrate wave (single PR)

**Date:** 2026-08-06  

## Outcome

Single-PR multi-module pure fail-closed domain tests + process local-admission. HOLDs not cleared.

## Root cause fixed

**ops.dark-mjs-not-wired:** process suites exact-wired via `check:ci-preflight`; admit runs `check:js-test-reachability` when scripts/tools.ci change.

**ops.npm-audit-js-yaml:** pin `js-yaml@4.3.1` (GHSA-5p4m-2wfm-xmqj quadratic `!!omap` on 4.0.0–4.3.0).

**ops.lens-order:** ledger `selected_lenses` unique + canonical v1 order.

## Supersedes

Tip PRs #601-#609 (`ops.multi-tip-wave`).

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "standard",
  "risk_domains": [],
  "selected_lenses": [
    "Cartesian doubt",
    "Essentialism / YAGNI",
    "Chesterton's Fence",
    "Pragmatism",
    "Systems Thinking",
    "Blast-radius / cell-based"
  ],
  "task_fit": {
    "Cartesian doubt": "Cargo green is not evidence of hosted preflight green.",
    "Essentialism / YAGNI": "One integrate PR; wire tests into existing CI surface only.",
    "Chesterton's Fence": "js-test-reachability exact-path ratchet is load-bearing; tip-serial authority JSON roots stay tip writers.",
    "Pragmatism": "Admit must execute the same ratchet CI executes; patch js-yaml rather than exception the finding.",
    "Systems Thinking": "npm scripts only count if CI executable graph reaches them; preflight fail cascades PG reachability soft reds.",
    "Blast-radius / cell-based": "Domain libs path-disjoint; shared writers once."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Process .test.mjs suites were orphaned from CI exact-path surface (ops.dark-mjs-not-wired).",
    "Local admit omitted check:js-test-reachability so hosted preflight failed while cargo admit looked green.",
    "selected_lenses listed Systems Thinking before Pragmatism (canonical order violation).",
    "js-yaml@4.3.0 unmatched high GHSA-5p4m-2wfm-xmqj failed Required/Security."
  ],
  "decisions_changed_or_rejected": [
    "Rejected multi-tip #601-#609 land path (ops.multi-tip-wave).",
    "Rejected orphan npm test scripts as substitute for CI-wired suites.",
    "Rejected node-audit exception for js-yaml when 4.3.1 patch is available."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Process must_fix closed (owner review)

1. Tip-serial roots include exact `docs/program/executed-tests-baseline.json` + `docs/documentation-manifest.seed.json` (unit-asserted).
2. `check:tip-contention` maps to `--check` (BEHIND tip writers fail; multi open tip writers non-fatal by design).
3. Admit plans `test:local-admission` + `test:ci-tools` when scripts/tools.ci change.

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
