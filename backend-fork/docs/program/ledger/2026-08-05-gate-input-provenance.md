# Ledger — S2 gate-input provenance + tranche-1 decoupling (2026-08-05)

## Identity

- Slice: S2 / G003 / issue #568
- Base: `3a5dcc344d0f1f19e6b5c84d1a376c05f7375ee7` (`origin/main` after #572)
- Plan: console-roadmap-20260805 final v2 sha256 `812ab03daa9865ab764b661c80f00dd61867676913fedcb4fd9d879e4c4a14ab`

## What shipped

1. **Instrument** `scripts/check-gate-input-provenance.mjs --json` emits the `gate_inputs` relation: `{gate, script, input_path, class, assertion_count}`. Class is joined from `docs/documentation-index.json` and is never derived from gate consumption (K-1).
2. **K-2 bounded tracing**: `scripts/lib/text-gate.mjs` hooks reads/assertions via `scripts/lib/gate-inputs.mjs`. The five private-reader floor scripts (`check-g004`…`g007`, `verify-doc-citations.mjs`) declare document inputs and emit the same provenance side-channel. Untraceable floor gates fail closed.
3. **Exception register** `docs/program/gate-input-exceptions.json` ships populated (`baseline_count` locked). Red rules: growth beyond baseline, expired `remove_by`, decoration (unobserved pairs).
4. **T1-CONV (one counted Markdown row)**: removed `check:g004-identity-foundation`’s prose assertion that `docs/specs/backlog-clearance-ledger.md` contains the G004 goal id. Replacement is the existing executable `matrix.goalId === goalId` check in the same gate (matrix JSON is machine-owned). Directional red: mutate `docs/benchmarks/g004-identity-foundation-matrix.json` `goalId` → `npm run check:g004-identity-foundation` fails. CI configuration: `npm run check:g004-identity-foundation` in `repo-gates` (locked in preflight). Binary identity confirmed against package script + preflight lock (not demoted by `#[ignore]` / feature flags — pure Node).
5. **T1-HYG (counts zero)**: deleted Rust doc-comment assertions from `check-payroll-release-gate.mjs` (golden case, professional validation, artifact_sha256, NTS, pure-kernel boundary). Executable coverage remains in `console-payroll-domain` unit tests. K-4: NTS prose row dropped rather than strengthened to `.message` equality.
6. **T1-b**: remaining `docs/specs/payroll.md` assertions for absent regulated controls stay registered as residuals — no payroll control implemented.

## Counts (instrument, post-slice)

- Document rows observed: 20
- Counted rows (class ∈ {historical, quarry, evidence} ∧ assertion_count > 0): 19
- Counted assertions: 794
- Exceptions: 19 (= baseline_count)
- Converted: 1 counted row (`check:g004` × `backlog-clearance-ledger.md`)
- Hygiene deletions: 5 Rust-comment assertions (count zero)

## Verification order

See plan S2 verification block. Hosted evidence is the PR checks for this candidate.

## Reasoning lenses (canonical v1)

Cartesian doubt, Essentialism/YAGNI, Chesterton's Fence, Pragmatism, Red Team, Systems Thinking, Operability/Day-2, Blast-radius, Zero-trust/defense-in-depth, Telemetry-first — applied to instrument fail-closed design, HOLD-preserving T1-b, and bounded (not process-wide) tracing.

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
    "Red Team",
    "Systems Thinking",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Treats instrument counts, exception registration, and a converted prose assertion as claims requiring hermetic red proofs and CI-reachable replacement identity rather than as authority by assertion.",
    "Red Team": "Fails closed on untraceable readers, declared-but-unread and read-but-undeclared paths, exception growth, expiry, and decoration, and requires directional red for the T1-CONV replacement.",
    "Systems Thinking": "Keeps document class independent of the gate_inputs relation so wiring a gate cannot erase measured prose debt by reclassifying consumption.",
    "Operability / Day-2": "Ships a deterministic instrument, populated remove_by register, exact admission wiring, and a ledger that records conversion versus hygiene versus residual HOLD prose separately.",
    "Blast-radius / cell-based": "Confines the slice to gate scripts, provenance helpers, CI wiring, and one evidence ledger in a fresh worktree; product code, migrations, deploy, and authority registers stay untouched except the single additive seed row required by the S1 ratchet.",
    "Telemetry-first": "Publishes counted rows, assertion totals, exception baseline, discovered-versus-executed suite counts, C/T identities, and remaining nonblocking caveats.",
    "Zero-trust / defense-in-depth": "Combines bounded opt-in tracing (not process-wide fs hooks), declaration matching, exception fail-closed rules, independent package/CI locks, and the protected C/T authority train without letting any one proof substitute for the rest."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "gate_inputs must remain a separate relation from class or measured prose dependency can erase itself by wiring.",
    "A conversion is only honest when the replacement runs in CI's configuration and fails directionally when the guarded property is violated.",
    "Hygiene deletions of Rust doc-comment assertions count zero toward decoupling and must be labeled as such."
  ],
  "decisions_changed_or_rejected": [
    "Rejected process-wide filesystem tracing in favor of the bounded text-gate hook plus five private-reader admissions.",
    "Rejected converting HOLD payroll prose by implementing absent controls; T1-b keeps the prose registered.",
    "Rejected treating seed-forbidden as blocking the plan-required evidence ledger; one additive evidence row is the minimum ratchet-preserving path."
  ],
  "lens_set_changes": [
    "Added Systems Thinking because class, gate_inputs, exception register, CI reachability, and HOLD residual registers interact but must remain distinct."
  ]
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
