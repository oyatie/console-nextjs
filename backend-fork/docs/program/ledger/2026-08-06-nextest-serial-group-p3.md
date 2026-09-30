# Ledger: nextest serial-group config (DN-0005 P3)

**Date:** 2026-08-06  
**Kind:** CI substrate evidence  
**Related:** ADR-0039, DN-0005

## Outcome

Land `.config/nextest.toml` with `cluster-global` max-threads=1 for the six
cluster-global mutator suites (leave/key_revision/attendance migration + apalis).
Pin cargo-nextest **0.9.138** in config comments. Fail-closed
`tools/ci/check-nextest-config.mjs` wired into `check:ci-preflight`.

Cargo remains primary CI runner for domain-unit; nextest dual-path expansion
in hosted domain-unit is a follow-up (preflight command locks).

## Verification

- `node tools/ci/check-nextest-config.mjs`
- `node --test tools/ci/check-nextest-config.test.mjs`

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
    "Essentialism / YAGNI": "Config + fail-closed checker only; no full CI runner swap.",
    "Chesterton's Fence": "Keeps cargo domain-unit primary; serial group matches ADR list.",
    "Pragmatism": "Pin 0.9.138; dual hosted nextest deferred past preflight locks.",
    "Red Team": "Checker refuses missing filters or missing pin documentation.",
    "Operability / Day-2": "Wired into check:ci-preflight for every PR.",
    "Blast-radius / cell-based": "No ci.yml command lock churn this PR.",
    "Telemetry-first": "Checker OK line names control.",
    "Zero-trust / defense-in-depth": "Does not demote Required PG or residual Buck gate."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "domain-unit proof digests lock cargo argv; nextest dual-path in that job deferred."
  ],
  "decisions_changed_or_rejected": [],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
