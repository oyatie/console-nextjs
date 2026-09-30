# Authority tip — Wave 1 canonical hardening (tai.1 / uoh / q06 / 6pl)

**Date:** 2026-08-10
**Kind:** authority tip (T) for candidate `5b919bbe641ccca9328d8536af6e198ed1ac7c81`
**Candidate (authority train):** `5b919bbe641ccca9328d8536af6e198ed1ac7c81` (immutable absolute SHA; not a relative `HEAD^` expression)
**Scope:** writer-ownership sqlparser lexer (console-tai.1) including escape-decode + opaque quoted DATA; ambiguous employment_source_bindings refuse (console-uoh); canonical subject_id bind to gated target_id (console-q06); JobPosition single-writer proof already on tip (console-6pl, receipt-only).
**Not product authority.** Clears no HOLD. Makes no production, frontend, or projection claim.

## Summary

- **Residual 11 closed.** `without_sql_comments` tokenizes with sqlparser so a `--` inside single-quoted SQL data cannot hide a later write in the same literal; escapes are decoded (`with_unescape(true)`) so escaped-newline line comments cannot fail open; quoted string contents stay opaque to `write_targets`.
- **Ambiguous binding refuse.** `bound_employee` uses `fetch_all` and fails closed when `employment_id` maps to more than one employee.
- **Four-eyes subject bind.** `CanonicalQuery::subject_id` plus `canonical_port_handler` refuses payload subject ≠ gated `target_id`. EmploymentQuery totality deferred to console-h3e (q06 held open).
- **JobPosition port.** Re-measured satisfied on origin/main tip; no product leaf required.

## Skipped this wave (path overlap)

`console-avb`, `console-6n4`, `console-nuc` — owned roots collide with q06/uoh/cross-port audit; deferred. `console-ann` / `console-soe` are train 2 (scope freeze).

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "authz",
    "hr_payroll"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Essentialism / YAGNI",
    "Red Team",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Separated tip-shape facts (C vs T allow-list) from product leaf facts; authority auth failed on receipts tip and preflight failed on missing lens_contract — both measured in CI logs before rebuild.",
    "Essentialism / YAGNI": "Took the smallest disjoint Wave-1 set (tai.1/uoh/q06 + 6pl verify) and deferred overlapping avb/6n4/nuc rather than inventing a combined mega-lane.",
    "Red Team": "Ambiguous employment bindings and four-eyes subject mismatch are fail-closed refuses, not silent picks; residual-11 quoted-dash evasion is closed by a real SQL lexer rather than another byte-scan spelling; escaped-newline fail-open and quoted-DATA false-positive were closed with hostile pins before tip rebuild.",
    "Operability / Day-2": "Regenerated first-party and third-party Buck faces with the sqlparser dep so generated-face cheap admission sees the same closure CI will merge; ratcheted executed-tests baseline for the new uoh control; seed+index land on C so Repo gates see the ledger path at T.",
    "Blast-radius / cell-based": "Path-disjoint worktrees; serial admit onto one train; C carries product+faces+baseline+manifest, T is ledger-only so C..T cannot smuggle product bytes.",
    "Zero-trust / defense-in-depth": "Pinned SSH-signed C and T; authenticate-console-authority allow-list enforced; lane receipts validated before train push."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "Initial tip put .cursor/receipts on T and failed authenticate-console-authority allow-list.",
    "sqlparser required third-party/rust BUCK regeneration, not only first-party writer-ownership BUCK.",
    "Governed ledger tips require lens_contract v1 evidence; missing block failed reasoning-lens changed-record admission.",
    "executed-tests baseline needed --update for the +1 uoh RED test attribute before merge.",
    "console-6pl was already satisfied on tip; closed via receipt evidence without a product leaf.",
    "Remote tip 9c7ea19f6 failed fmt and doc-manifest; QA/hostile harness proved sqlparser with_unescape(false) fail-open — folded into candidate C before re-tip."
  ],
  "decisions_changed_or_rejected": [
    "Rejected parallel avb/q06 (rest+domain collision) and uoh/6n4 (employment.rs collision).",
    "Rejected jurisdiction-only T this round in favor of ledger-only T so product C stays free of register churn.",
    "Deferred EmploymentQuery subject_id totality to console-h3e (Batch B); kept q06 open after admit.",
    "Deferred /*-in-quoted-SQL residual to console-jth; closed quoted -- DATA false-positive on C."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
