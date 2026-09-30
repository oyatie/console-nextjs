# Disk-wipe consolidation sealed replacement authority

## Exact identity

- Protected base B: `435e251edfab12750850d5b1d411528b10a3ed8a`
- Revoked authority tip: `3885ed49b4bf2906f75c1907cfcd7f03f9aac0c3`
- Signed non-authority recovery checkpoint: `171ed4f79bf72bc8871f526a483df0391830c1df`
- Final signed content candidate C: `27f6fb959f707a79c1b7d49349b996dae6f6cf8a`
- Final authority tip T: the signed direct child of C that adds only this file

C is a signed, empty tree-sealing commit over the reviewed recovery checkpoint:
`git diff --exit-code 171ed4f79bf72bc8871f526a483df0391830c1df
27f6fb959f707a79c1b7d49349b996dae6f6cf8a` exits zero. The checkpoint commit
message explicitly required a newly sealed candidate; C supplies that identity
without changing the reviewed bytes. This ledger supplies the only C-to-T tree
change. All earlier disk-wipe authority ledgers and C/T identities are
superseded; none carries merge authority into this pair.

The external scorecard relayed by the owner is opinion only. It authorized no
deletion, product boundary, implementation, readiness, deployment, or compliance
claim. The corrections below were admitted only after repository, database,
Git, or hosted-run evidence reproduced them.

## Corrections after the revoked tip

Exact-object review found that migration 0211 accepted classifications on
non-public schemas, materialized views, and foreign tables in its completeness
reader while its retention-floor reader inspected only `public` ordinary and
partitioned tables. A valid sensitive marker could therefore derive one year
instead of two. Both readers now share the same non-`pg_catalog`, non-temporary
`r/p/m/f` relation universe, and `personal_data_columns()` returns the
schema-qualified identity needed for collision-safe cleanup. Isolated PostgreSQL
probes plant sensitive markers in a non-public table, a materialized view, and a
foreign table; each derives two years while both SECURITY DEFINER functions
remain unavailable to an ungranted role and `console_rt` with SQLSTATE `42501`.

The same review audited every classified JSONB target against migration 0211's
Rule C. Exactly five unbounded/user-controlled columns were incomplete:
`employees.raw_row`, `data_import_rows.raw_row`,
`docs_evidence_custody_events.from_custodian`,
`docs_evidence_custody_events.to_custodian`, and `todos.scopes`. Each now carries
`undeclared` without losing its known personal, RRN, or health classification;
catalog tests mutation-lock all five. It also reconciled two touched API
contracts: site creation documents the implemented branch scope, and exit-case
creation returns the documented `201 Created` rather than 200.

Hosted run `30834869062` then proved that the generated workorder test face did
not contain `mobile_evidence_fixtures.rs`. The replacement exports the mobile
and dispatch shared fixtures, maps all four path-module users, and places the
three previously dark timer/mobile binaries in the serialized PostgreSQL lane.
The exact dark baseline falls from 13 to 10 instead of documenting the missing
fixture as deferred.

Current-authority documents no longer dispatch the generic
`company_conformance` engine fixture as projected Company/HR product work. All
current capability states are `HOLD`; current worktree, branch, and lane
assignments are null; and the exact prior registry state remains separately
hash-bound as history. Production/readiness prose is historical or unverified,
and the retained backup, restore, PITR, and CNPG drill entrypoints invoke one
common authority guard that exits 78 before substantive action. These are
continuity and safety holds, not claims that the product pivot or an operational
recovery path is complete.

## Verification before T

- Signed C and checkpoint signatures verify against the pinned ED25519
  fingerprint `SHA256:5grGNUtX9Zgmy1SWne6wF9DR8W1ElUQaF/Z8SYRz8E8`; their
  trees are byte-identical.
- Disposable PostgreSQL passed personal-data classification 27/27 with one
  intentional generator ignored, app inline PostgreSQL 18/18, and the four
  repaired dispatch/mobile targets 15/15.
- OpenAPI drift passed 11/11. The app compiled with `test-postgres`, and the
  migration/classification test binary compiled independently.
- The first-party generator suite passed 25/25 after the generated BUCK files
  became committed. Truth-ledger/fanout tests passed 54/54.
- CI preflight passed 51/51. Production hardening passed 234 structural checks
  and 58/58 regressions. Executed-test reachability reports 335 defined, 325
  reachable, 10 explicitly named dark binaries, and 2,124 static attributes;
  its focused suite passed 23/23.
- ADR governance passed 29/29, foundation validation passed 134 checks, OpenAPI
  and personal-data gates passed, and the documentation-link suite passed 7/7
  before scanning 371 tracked documentation files successfully. The link walker
  now ignores generated `buck-out` trees so dangling build-artifact symlinks
  cannot make a post-Buck local verification crash.
- Independent repair review returned no blocker after rerunning the database,
  HTTP status/audit, OpenAPI, and documentation cases. Independent continuity
  review matched the machine receipt hash
  `bc567c4b67adc27b000c52332dbfc92765a37223c1468ee879cddb577d053fcc`,
  all three immutable Ultragoal inputs, the registry reset, ignored-state audit,
  and external-custody boundary.

These measurements authorize exact-object verification of C/T only. They do not
authorize merge by themselves. The protected-main simulator, complete exact-T
local verifier, detached exact-object review, every hosted required context, one
formal non-author GitHub approval with stale/last-push/conversation controls,
branch-protection readback, squash binding, any generated release PR, and final
main/branch/PR readback remain independent gates.

Disk erase remains separately blocked until the owner copies and read-back
verifies the pinned signing key, the complete `~/.config/talos-mnt/**` recovery
tree if it is to be retained, required external secrets, and the four named
business inputs—or explicitly approves each discard/reissue decision. Repository
merge cannot reconstruct those bytes.

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
    "production",
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
    "Cartesian doubt": "Revoked a locally green signed tip when exact review and hosted execution contradicted its authority claims.",
    "Red Team": "Planted sensitive markers in every formerly omitted catalog shape, audited all JSONB classifications, and required generated fixture consumers to execute.",
    "Systems Thinking": "Joined Git custody, test reachability, branch protection, API behavior, migration derivation, program state, ignored planning context, and external secrets into one closeout boundary.",
    "Operability / Day-2": "Made production scripts fail closed, reset disposable continuation pointers, preserved exact restart evidence, and kept workstation recovery outside Git explicit.",
    "Blast-radius / cell-based": "Kept all repairs in the sole draft PR, preserved historical state separately, and made no infrastructure, DNS, legal, or rejected-product mutation.",
    "Telemetry-first": "Bound every claim to exact commits, signatures, run identity, test counts, hashes, contexts, and required post-operation readbacks.",
    "Zero-trust / defense-in-depth": "Kept independent exact-T simulation, object review, hosted CI, formal approval, protected-branch controls, squash binding, and external-custody confirmation as separate gates."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "A classification completeness reader and a derived legal floor must consume the same catalog universe or a valid marker can become dangerous under-retention.",
    "A generated test target is not executable evidence until every shared source fixture is mapped and the wrapper is named by a gating CI lane.",
    "Repository continuity and disk-wipe safety are separate: Git can preserve reviewed code and plans but not unescrowed workstation credentials or external business files."
  ],
  "decisions_changed_or_rejected": [
    "Rejected every prior disk-wipe C/T identity after new evidence invalidated it.",
    "Rejected documenting valid non-public and m/f classifications as a deferred retention blind spot.",
    "Rejected preserving provisional lanes or operational prose as current continuation authority.",
    "Rejected treating an external scorecard or a signed recovery checkpoint as merge authority."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
