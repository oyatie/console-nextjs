# Ledger — S3′ JS test reachability ratchet (2026-08-05)

## Identity

- Slice: S3′ / G004 / issue #570
- Base: `7965fbdf7b28e948e471c1ec046441473b1a32b8` (main post-#573)
- Remeasure date: 2026-08-05

## Remeasurement (precondition)

At base `7965fbdf7`:

- `scripts/dev/mjs-test-reachability.mjs` is a **diagnostic** that reads **`origin/main`** (not candidate-bound).
- No `scripts/check-js-test-reachability.mjs`, no `docs/program/js-test-reachability-baseline.json`, no fail-closed CI step.
- Report: 32 suites on main before this slice; 26 exact-wired; 6 nowhere (no npm script).

**Conclusion:** population is **not** already ratcheted. Option E retraction does **not** apply. Implement S3′.

## What shipped

1. `scripts/check-js-test-reachability.mjs` — candidate-bound via `git ls-files`; npm script fixed-point expansion; **exact path** = coverage; basename-only = diagnostic only; unregistered dark fails; baseline decoration/stale deferred fail.
2. `docs/program/js-test-reachability-baseline.json` — six deferred suites without npm scripts.
3. Wiring: `check:js-test-reachability`, `test:js-test-reachability`, preflight job steps, preflight ordered contract, verify PLAN.

## Non-goals

- Wiring the six deferred suites (follow-up tranche).
- Buck deletion.
- Basename matching as green coverage.

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
    "Red Team",
    "Systems Thinking",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Remeasured origin/main diagnostic vs candidate-bound fail-closed gate before choosing implement over retract.",
    "Essentialism / YAGNI": "Promotes one ratchet script and baseline; does not wire all deferred suites in this slice.",
    "Chesterton's Fence": "Keeps deferred list for known dark suites instead of forcing fake npm wiring.",
    "Red Team": "Rejects basename-only hits as coverage and fails closed on baseline growth and decoration.",
    "Systems Thinking": "Aligns with executed-tests ratchet patterns without conflating Rust and JS inventories.",
    "Operability / Day-2": "Exact commands in package.json and preflight; baseline is the hand-maintained debt register.",
    "Blast-radius / cell-based": "Scripts, baseline, CI wiring only; no product domain or HOLD surfaces.",
    "Telemetry-first": "Reports exact-wired, dark, and basename-only counts on every run.",
    "Zero-trust / defense-in-depth": "Candidate git ls-files and gating workflow extraction; does not trust origin/main for admission."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "The pre-existing mjs reachability tool was never a CI ratchet because it was not candidate-bound and not fail-closed.",
    "Six .test.mjs files still lack any npm script and remain deferred debt.",
    "Exact path matching is the only green path; basename matching is diagnostic."
  ],
  "decisions_changed_or_rejected": [
    "Rejected Option E retraction after remeasurement.",
    "Rejected counting basename-only workflow mentions as reachability.",
    "Rejected wiring all six dark suites in this increment."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
