# Authority tip — W1.0 pilot admit (lane-receipt validator)

**Date:** 2026-08-12
**Kind:** authority tip ledger bound on candidate C (product tip below); T adds this ledger entry only
**Candidate (authority train):** `21fbc60bc3ca5b1e4f21f0d9566bf2c5c0f520a6` (immutable absolute SHA of the product tip that C parents; base `b2acd80c4d7f340199b9147f6df9318d74af5f8d`, behind 0 at admission)
**Scope:** Wave 1 pilot lane `v-lane-receipt-validator-20260812` — tracked lane-receipt schema + validator + 22-test suite wired into the CI preflight contract and js-test-reachability ratchet. Cross-family review (executor Grok, reviewer architect/Opus): R1 BLOCK at `55331bc27`, one consolidated fix round, R2 APPROVE at `ac6f2a4de`; a post-push Codex review probe proved the R2-filed scanDir major, closed in `21fbc60bc` (predicate fix + planted red + exact parity pins) with an R3 delta re-critique APPROVE bound to that final product tip.
**Not product authority.** Makes no production exposure claim. Opens no HOLD. Validator tooling only; ordinary R2 review path, no GAAC.

## Summary

- `scripts/console/lane-receipt.schema.json` — receipt contract; laneReceipt.required is the UNION of the CI-pinned lane-fanout BUILD_SCHEMA fields and the agent-ritual fields.
- `scripts/console/validate-lane-receipt.mjs` — dependency-free schema-driven CLI carrying the incumbent convergence rules (APPROVE-over-blocking-finding conflict, status=done commands, n/a prefix escape, blank-string guard); `--dir` scan fails on zero kind-bearing receipts and FAILS a present-but-unrecognised `kind` instead of skipping it.
- `scripts/console/validate-lane-receipt.test.mjs` — 22 tests: planted-red per rule, exact BUILD_SCHEMA/REVIEW_SCHEMA parity pins (same-size substitution reds), dual-validation of the lane receipt under both validators, real-directory scan, executed floor 22.
- CI wiring: preflight step `Console lane-receipt validator regression`, check-ci-preflight proofRun + pins (29/122/366), CI-GATES.md root-scripts inventory, doc manifests regenerated.
- Receipts: `.cursor/receipts/v-lane-receipt-validator-20260812.json` (lane) + `-critic.json` (R2 verdict) + `-critic-r3.json` (R3 delta verdict), schema-valid under both validators.

## Remaining HOLDs / follow-ups (not closed by this tip)

- Consolidation lane (three receipt authorities collapse to the schema; commands vocabulary; legacy receipt migration — strictly atomic per receipt given the lane|critic vs build|critic token divergence; agent-card/hook drift; scanDir tracked-blob enumeration via git ls-files folding Codex thread-3 with R3-5; R3-2 planted-red for the nonEmptyString items tightening) — recorded in lane receipt followUps. The scanDir kind predicate itself is CLOSED in `21fbc60bc`.
- docs/CI-GATES.md pre-existing broken citation (~902) — W1.1 T1 custody lane
- Frontend HOLD (P-H1.b), ADR-0037 successor, and all programme HOLDs unchanged

## Critic receipt binds (lane worktree APPROVE tips)

| leaf | product tip | critic |
|------|-------------|--------|
| v-lane-receipt-validator-20260812 | 21fbc60bc3ca5b1e4f21f0d9566bf2c5c0f520a6 | R2 architect/Opus APPROVE at `ac6f2a4de`; post-push Codex probe proved the filed scanDir major; fix `21fbc60bc` re-critiqued by R3 architect/Opus delta pass APPROVE bound to `21fbc60bc` covering `ac6f2a4de..21fbc60bc`; C and T are mechanical train commits inside the R3-restated binding condition (receipts+manifests in C, single ledger add in T) |

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "release"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Chesterton's Fence",
    "Red Team",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Handoff premise 'scripts/cursor validator is untracked' was disproven by git ls-files/log (tracked since #738); the R1 verdict and the union-schema redesign followed from that evidence, not the brief.",
    "Chesterton's Fence": "lane-fanout.test.mjs pins redBaseline as permanently required for a recorded reason; the schema now carries the union instead of walking around the fence; critic tip-binding deliberately NOT added because all three incumbent authorities require verdict+findings only.",
    "Red Team": "Critic ran hostile probes: APPROVE-over-blocker receipt, whitespace-only commandsRun, kind-less legacy receipts, parity-regex broken four ways; post-push a Codex review probe proved the filed scanDir kind-skip major (leader-reproduced, exit 0 pre-fix), closed by the 21fbc60bc predicate fix with planted red; every probe now has a planted-red test or a filed followUp.",
    "Operability / Day-2": "Examined-zero fails at three layers (zero-arg exit 1, --dir zero kind-bearing receipts exit 1, suite floor 22); consolidation lane recorded in followUps owns the three-authority collapse and legacy receipt migration.",
    "Blast-radius / cell-based": "Additive only: no incumbent validator, gate, or workflow modified; kind field is the migration boundary so 30 legacy receipts stay under incumbent validators; single revert restores prior state.",
    "Zero-trust / defense-in-depth": "Cross-family review (Grok executor, Opus critic, two rounds); SSH-signed C and T; parity pinned by execution against lane-fanout source; receipt dual-validated under both validators in CI."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "R1 BLOCK proven blocker: third tracked spelling of the receipt field list dropping CI-pinned redBaseline et al.; closed by required-field union + executed subset parity pins.",
    "R2 APPROVE filed 1 unproven major (scanDir skips present-but-unrecognised kind); a post-push Codex probe proved it; closed in 21fbc60bc (Object.hasOwn skip predicate + planted red + exact parity pins) under an R3 delta re-critique APPROVE bound to the final product tip - filed-not-hidden then fixed-when-proven, per convergence doctrine.",
    "Foundation-gates docs-drift gate fired on the undocumented CI step during development and was satisfied by the CI-GATES.md inventory entry - the designed gate worked.",
    "R3 filed: schema nonEmptyString tightening lacks a planted-red control (major, unproven); scanDir non-object/extension skip residue and kind token divergence folded into the consolidation lane; exact parity pins add a co-edit requirement across the leased .claude/** boundary."
  ],
  "decisions_changed_or_rejected": [
    "Rejected landing the schema as a third independent field-list authority; redesigned to a strict superset of BUILD_SCHEMA with executed parity pins.",
    "Rejected adding headSha to critic receipts this wave (would fork a fourth contract); deferred to the consolidation lane with rationale.",
    "Rejected a third fix round for unproven R2 minors per convergence doctrine; filed with exact fixes in the critic receipt."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
