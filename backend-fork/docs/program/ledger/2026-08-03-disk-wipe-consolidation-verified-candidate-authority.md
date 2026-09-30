# Disk-wipe consolidation verified candidate authority

## Exact candidate

- Protected base B: `435e251edfab12750850d5b1d411528b10a3ed8a`
- Superseded remote checkpoint T: `900c1749a94f945deaa85cc5097a51287620dd95`
- Final signed content candidate C: `b87d09596cbbd1775fbfd0e82371a8a0a08e39a2`
- Final authority tip T: the signed direct child of C that adds only this file

The external scorecard supplied during consolidation remained an opinion. It
authorized no scope, deletion, product claim, or implementation. The final C
contains only outcomes independently reproduced against current repository,
official-source, Git, database, or hosted-run evidence.

## Corrections admitted after the checkpoint

Hosted execution invalidated two checkpoint assumptions. The protected authority
bootstrap could not resolve historical provenance objects that existed only in a
workstation object database, and the directly installed `cargo-audit` executable
was invoked without the `audit` argument that Cargo normally supplies. Signed
archive tags now retain the two out-of-ancestry provenance objects; both machine
registers name their exact refs, the validator checks both registers and binds
each ref to its commit, and a candidate-only clone regression fails until both
advertised refs are fetched. The security workflow now invokes the pinned
`cargo-audit 0.22.2` binary with its required subcommand, and a mutation test
rejects deleting it.

The fabricated-branch gate reported three production files as scanned while
skipping their contents through whole-file handoff exemptions. C deletes those
exemptions and proves all three former suffixes reject the known fabrication.
It then removes the underlying authorization defects rather than suppressing the
gate:

- HR exit reporting locks and derives branch evidence from the exact employee,
  optional exact-employee absence alert, and attendance history. A request branch
  is only a consistency assertion; missing/mismatched evidence fails closed and
  a branchless write requires org-wide authority.
- Registry site creation carries the principal's complete scope to the adapter,
  returns same-tenant foreign-branch customers as not found, and leaves no row or
  audit. The tenant-wide HQ master import requires org-wide authority.
- Reporting authorizes a requested branch as a concrete resource, checks scoped
  repository operations against the complete principal scope, and reserves the
  unfiltered tenant rollup for org-wide authority.

The official 2026-07-01 Korean administrative rule expresses the classified
access-log floor as one or two **years**, not fixed 365/730-day periods. Migration
0211 now preserves that source unit. Its schema introspection remains owner-only,
and the newly introduced compliance REST/adapter surface was removed because
compliance is outside the current product pivot; no compliance conclusion or
operational-retention claim is admitted.

Current documentation now distinguishes accepted targets, current divergence,
and historical measurements. In particular, the instance-backed
`company_conformance` fixtures remain a useful generic-engine regression but are
not authority to dispatch projected Company/HR work. Replacement conformance and
Company/Person/Employment/PayRun projections stay HOLD until explicit owning-port
and single-writer contracts exist.

## Local verification bound to C

- `cargo fmt --all -- --check` and all-target clippy with warnings denied passed.
- Unit execution passed: app 153, platform authz 55, compliance REST 4,
  reporting REST 3, registry REST 2, plus their zero-test adapter libraries.
- Disposable PostgreSQL execution passed: HR durable-exit evidence 1/1,
  registry adapter 6/6, registry REST 19/19, and personal-data classification
  23/23 with its one intentional generator ignored.
- OpenAPI drift passed 11/11; the fabricated-branch gate passed 15 library and
  7 integration regressions while scanning 471 Rust files.
- CI preflight passed 51/51; reasoning-lens enforcement passed 40/40; ADR
  governance passed 29/29; all 395 Markdown files passed link validation.
- Test reachability reports 322 of 335 binaries reachable, with the unchanged 13
  explicitly named legacy-dark roots; workflow hardening locks all five security
  contexts; foundation validation passed 134 checks.
- Direct `cargo-audit 0.22.2 audit --no-fetch --ignore RUSTSEC-2023-0071`
  exited zero with only the separately governed advisory warning.

These local results do not authorize merge. Protected-main simulation, complete
clean-tree verification, independent exact-object reviews, every hosted required
context, an approving non-author GitHub review, branch-protection readback, and
post-merge/release readback must each bind to the final T independently.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "authz",
    "migration",
    "contracts",
    "approval",
    "hr_payroll",
    "release",
    "compliance_sensitive"
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
    "Cartesian doubt": "Treated the external audit, a signed checkpoint, and green local gates as claims to re-establish rather than facts that inherited authority.",
    "Red Team": "Exercised formerly excluded paths, foreign-branch and forged-evidence requests, ungranted database roles, missing provenance objects, and deleted workflow arguments.",
    "Systems Thinking": "Joined Git object custody, branch protection, CI reachability, authorization scope, database evidence, product boundaries, ignored planning state, and fresh-session recovery into one closeout boundary.",
    "Operability / Day-2": "Made out-of-ancestry provenance fetchable, kept secrets external, and updated the tracked restart receipt with measured results and explicit pending hosted obligations.",
    "Blast-radius / cell-based": "Kept corrections in the one existing PR, removed an out-of-pivot API instead of expanding it, and left live infrastructure, DNS, legal claims, and rejected feature lanes untouched.",
    "Telemetry-first": "Bound the candidate to exact commits, signatures, test counts, tagged refs, named dark tests, workflow contexts, and required post-operation readbacks.",
    "Zero-trust / defense-in-depth": "Required durable resource evidence at each tenant boundary and preserves separate protected-target authentication, mutation tests, hosted CI, formal external approval, and merge readback gates."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "The interim checkpoint was recoverable but not mergeable: hosted execution exposed two real false-green assumptions, and three security-sensitive files were still hidden behind gate exemptions.",
    "Useful continuity is the reviewed final net plus reproducible authority and planning evidence, not preservation of every branch, ignored runtime artifact, or external recommendation."
  ],
  "decisions_changed_or_rejected": [
    "Rejected caller-supplied or principal-fabricated branch identifiers as resource authority.",
    "Rejected converting a source rule expressed in years into fixed day counts.",
    "Rejected exposing owner-only schema classification as an out-of-pivot compliance product API.",
    "Rejected dispatching projected Company/HR work from an instance-engine regression fixture.",
    "Rejected carrying simulation, review, or CI authority forward from superseded candidate bytes."
  ],
  "lens_set_changes": [
    "Added Systems Thinking because disk-wipe safety depends on the interaction of Git custody, merge controls, test reachability, authorization, planning continuity, and external secret custody rather than any one code diff."
  ]
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
