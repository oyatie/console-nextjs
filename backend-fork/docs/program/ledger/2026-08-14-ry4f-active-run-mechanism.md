# Authority tip — console-ry4f active-run mechanism replacement

**Date:** 2026-08-14

**Kind:** authority tip ledger (T) for the console-ry4f ExactActiveRun mechanism-replacement lane. C is the signed hardening + manifest-registration commit; T is the signed direct child that adds only this ledger entry.

**Base:** `fb9ae31e6045b6a9beaa8b6dde893890e693d6ee` (origin/main, `feat(peas): employment port routing (#781)`; restacked onto main after #781 merged — #773 remains the merge-order gate).

**C+T signing note:** each train commit is SSH-signed (ssh-ed25519) by `jason19931225@gmail.com`, verified against the pinned allowed signer key `AAAAC3NzaC1lZDI1NTE5AAAAIAgMAp8vHS9V/9UQQVTa5FtmS9Q9fdB8I520DsZMMDTR`.

**Scope:** `scripts/check-production-hardening.mjs` (fail-closed wrapper-argv0 gate, reusing the `scripts/lib/ci-workflow-executables.mjs` helpers), `scripts/check-production-hardening.test.mjs` (+29 tests), `scripts/lib/ci-workflow-executables.mjs` (block-scalar indicators + folded-scalar reassembly + gating/shell metadata), `scripts/lib/executed-tests-baseline.test.mjs` (+3 tests), this ledger entry, and the `docs/documentation-manifest.seed.json` / `docs/documentation-index.json` registration.

**Not product authority.** Clears no HOLD and authorizes no production, credential, compliance, payment, erase, OCI, frontend, projection, or Oyatie action.

## Summary

- The prior ExactActiveRun strip was a denylist of narrative argv0s applied as strip-and-continue: each residual (ta90 → pwys → 8rr1 → c236) was one more argv0 spelling. A strip keeps matching whatever survives the denylist, so it can never be closed.
- This gate inverts the question into an allowlist mechanism. The committed inventory `docs/program/executed-tests-baseline.json` names the sources whose tests must execute (`test_attribute_baseline`); the real commands that execute them are real binaries (`cargo`, `npm`, `node`, `tools/buck2`, `cargo-audit`, …). An invocation whose argv0 is `source`, `.`, or `timeout` wraps a test/check executor and therefore cannot stand in for the real binary/test command.
- New `workflowWrapperInvocations` scans the three gating workflows (`ci.yml`, `security.yml`, `image-release.yml`) and fails closed on any `source`/`.`/`timeout` argv0 that wraps an executor from the `TEST_CHECK_EXECUTORS` allowlist. The classifier reassembles YAML folded scalars before tokenizing, re-runs transparent-prefix stripping to a fixed point, and refuses protected executors in non-gating steps, under a non-default shell, or on an unparseable command surface.
- The gate validates the executed-tests baseline is an object mapping source paths to non-negative integer counts, and fails closed when it is missing or malformed (the positive anchor), mirroring `check-executed-tests.mjs`.
- Seven review rounds surfaced thirty-eight findings — re-spelled block-scalar indicators, folded-scalar splitting, control-flow prefixes, the `source --` terminator, the unscanned `image-release.yml`, an unschematized baseline, `env`, `builtin`, `command -p`, and `env -S` behind control flow, non-gating protected steps, shell overrides, malformed surfaces, YAML comments, indentation indicators, and escape sequences on `run:` and `steps:`, `defaults.run.shell`, masked exit status, indirect targets, nested shells, combined shell flags, conditional `set`, node audit-policy status, multiline conditionals, shell functions, conditional exits, here-documents and their trailing operators, quoted inline run scalars and shell scalars, and a miscounted test total. Thirty-four are closed with RED decoy tests; two (claimed missing C/T trains at stale SHAs) were refuted; two are recorded as out-of-scope deferred gaps. The thirty-one wrapper-gate tests cover every decoy and the intact workflows.

## Verification

- `node --test scripts/check-production-hardening.test.mjs` — 91/91 pass, 0 fail (31 wrapper-gate tests).
- `node --test scripts/lib/executed-tests-baseline.test.mjs` — 25/25 pass, 0 fail.
- `npm run check:production-hardening` — exit 0; Production hardening check passed (239 checks across 4 groups).
- `npm run check:executed-tests` — exit 0 (362 reachable from a CI step, 1 dark).
- `npm run check:js-test-reachability` — exit 0 (47 suites exact-wired, 0 dark).
- `npm run check:ci-preflight` — exit 0; no ci-preflight contract change (pinned command string unchanged).
- `node scripts/console/generate-documentation-manifest.mjs --check` — documentation manifest OK.
- `node scripts/check-reasoning-lens-contract.mjs --changed-since origin/main` — reasoning lens contract OK.
- `git verify-commit` for each train commit against the pinned allowed signer — Good signature.

## Freeze status

NOT FROZEN. The seed record is `active` (unfrozen) and stays that way until the hosted checks (Required / CI, Required / Security, authenticate-console-authority) report success on the open PR.

## Operational receipt

- **Pre-mortem:** the new gate could false-positive on a legitimate wrapper argv0 in a gating workflow and turn every production-hardening check red; or it could fail to fire on a residual spelling and leave the c236 class open.
- **Detection:** the thirty-one wrapper-gate tests assert both refusal of every decoy (block-scalar, folded-scalar, control-flow, `source --`, image-release, non-gating, shell-override, malformed, masked-status, indentation, indirect-target, commented-jobs, nested-shell, node-audit, multiline-conditional, function, conditional-exit, here-document, quoted-shell, yaml-escape, command-p) and non-flagging of the intact workflows; `check:production-hardening` runs the gate against the real `ci.yml`/`security.yml`/`image-release.yml` on every invocation.
- **Rollback:** revert the train commits in reverse (ledger then code); the seed record returns to `active` pending re-admission; no other lane depends on this gate.
- **Stop conditions:** stop and return to integration on an out-of-scope write, a required `ci.yml` or contract change (owned by PR #773 until it merges), a baseline change, or a test weakening without an approved receipt.
- **Review identities:** signer `Jason Lee <jason19931225@gmail.com>`; independent adversarial review surfaced thirty-eight findings, thirty-four closed, two refuted as stale-SHA false positives, and two recorded as out-of-scope deferred gaps; merge order conductor-managed (after #773); further review through the PR review threads and hosted Required checks before merge.

## Remaining HOLDs / follow-ups

- Merge order is conductor-managed: this PR lands after #773.
- Main advanced with #775 (Employment owner retarget), #776 (identity role-set compare), #777 (payroll provenance), and #781 (peas employment port routing) after this lane opened; the train is restacked onto `fb9ae31e6` and the documentation manifests were regenerated to carry #775's, #776's, #777's, #781's, and this lane's ledger records.
- Freeze/unfreeze depends on the hosted check results (see Freeze status).
- All current PRODUCT/ROADMAP HOLDs remain unchanged.

## Out-of-scope gaps (deferred, recorded not silently accepted)

- `PRRT_kwDOS636Ss6ZbGmB` — command-substitution `$()` descent (a protected executor inside `$(...)` with masked status). Beyond the wrapper-argv0 class; deferred as minor/ownerLease.
- `PRRT_kwDOS636Ss6ZbGmD` — per-operator masked-status precision (the surface-wide `controlFlow` flag over-rejects a terminal `;` after a protected executor). Known imprecision; deferred as minor/ownerLease.

## Authority tip

T is the signed direct child of C that adds only this ledger entry. C prebinds this exact ledger blob in the generated documentation manifests.

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
    "Essentialism / YAGNI",
    "Chesterton's Fence",
    "Red Team",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Separated the denylist-strip narrative from the executor-allowlist mechanism and re-derived the c236 residual class as a wrapper argv0, not one more spelling.",
    "Essentialism / YAGNI": "Replaced only the residual-class mechanism with an allowlist gate plus focused tests; no product, migration, OpenAPI, or CI contract surface changed.",
    "Chesterton's Fence": "Kept the executed-tests baseline as the positive anchor and the missing-baseline-must-fail posture of check-executed-tests.mjs instead of inventing a new inventory.",
    "Red Team": "Each source/./timeout decoy around cargo-audit and each review-surfaced bypass reproduce the c236 false-green and are refused; the gate fails closed on a missing or malformed baseline, a non-gating step, a shell override, a masked status, a condition or function context, an env -S, command -p, or builtin prefix, a YAML-escaped scalar, or an unparseable surface. Two deep shell constructs (command substitution and per-operator masking precision) are recorded as out-of-scope gaps, not silently accepted.",
    "Operability / Day-2": "Records exact invocations and counts (91/91 hardening tests, 239 checks), the SSH-signed C/T train, and the unfrozen-until-hosted-checks posture.",
    "Blast-radius / cell-based": "Changes one hardening gate plus the shared workflow-executables helper, their test files, the ledger, and the manifest seed/index; CI, security, and image-release workflow bytes, and every product domain, stay outside the cell.",
    "Zero-trust / defense-in-depth": "Wrapper argv0 is a finding, not something to look through; the allowlist refuses delegation to an unscanned file or timed wrapper, and commits are SSH-signed against a pinned allowed signer."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "The ExactActiveRun strip denylist (ta90 -> pwys -> 8rr1 -> c236) can never be closed by another argv0 spelling, so the mechanism was replaced with an allowlist + fail-closed wrapper gate.",
    "source / . / timeout argv0 wrappers around a real test/check executor (e.g. cargo-audit) each reproduce the c236 residual false-green and are refused by the new gate.",
    "The intact ci.yml, security.yml, and image-release.yml workflows carry no wrapper-argv0 findings; the legitimate source third-party/rust/reindeer/upstream.lock setup is not falsely flagged.",
    "Seven review rounds surfaced thirty-eight findings (block-scalar indicators, folded-scalar splitting, control-flow prefixes, the source -- terminator, the unscanned image-release.yml, an unschematized baseline, env, builtin, command -p, and env -S behind control flow, non-gating protected steps, shell overrides, malformed surfaces, YAML comments, indentation indicators, and escape sequences on run and steps, defaults.run.shell, masked exit status, indirect targets, nested shells, combined shell flags, conditional set, node audit-policy status, multiline conditionals, shell functions, conditional exits, here-documents and their trailing operators, quoted inline run scalars and shell scalars, and a miscounted test total); thirty-four were closed with RED decoy tests.",
    "Two review claims of a missing SSH-signed C/T train at stale SHAs were refuted: the actual C and T commits are separate, SSH-signed, and structurally correct.",
    "Two round-7 findings (command-substitution $() descent, and per-operator masked-status precision) are out of scope for the wrapper-argv0 class and recorded as deferred gaps, not silently accepted."
  ],
  "decisions_changed_or_rejected": [
    "Rejected adding one more argv0 spelling to the ExactActiveRun strip denylist.",
    "Rejected a strip-and-continue posture that keeps matching whatever survives the denylist.",
    "Rejected weakening the fail-closed baseline-missing posture; the gate mirrors check-executed-tests.mjs and fails when the positive anchor is absent.",
    "Rejected limiting the wrapper scan to ci.yml/security.yml; image-release.yml cosign and Trivy steps are now scanned too.",
    "Rejected treating a block-scalar re-spelling, a control-flow prefix, a source -- terminator, a folded-scalar split, a non-gating step, a shell override, or a malformed surface as not-a-wrapper."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
