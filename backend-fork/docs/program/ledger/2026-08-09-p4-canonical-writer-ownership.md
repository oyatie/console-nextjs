# Authority tip — one production writer per canonical table, proven by a gate

**Date:** 2026-08-10
**Kind:** authority tip (T) for candidate `74f1230ef05af81d12a16a636effbd8d3b001c9ef`
**Candidate (authority train):** `74f1230ef05af81d12a16a636effbd8d3b001c9ef` (immutable absolute SHA; not a relative `HEAD^` expression)
**Scope:** the six canonical ports and their PostgreSQL adapters, migrations 0212–0215, the
`console-gate-writer-ownership` gate and its CI wiring, the projected-dispatch derivation, and the
CI plumbing those require.
**Not product authority.** Clears no HOLD. Makes no production, frontend, or projection claim.
ADR-0030 §8 still forbids a frontend shell: closing §7 row 4 leaves rows 3 and 5 open.

## Summary

- **The ratchet is empty because the violations are empty.** `KNOWN_SECOND_WRITERS` is `&[]`, and the
  gate cannot be talked into that state: one assertion pins `violations.len()` to the ratchet's
  length and `stale_exemptions()` fails any entry that no longer describes a real violation. Delist
  without removing the writer is red; remove without delisting is red the other way.
- **The database half is total.** `has_table_privilege(role, relation, DML)` subsumes direct grants,
  column grants, ownership, recursive role membership and superuser — the five catalogs an earlier
  revision unioned by hand while missing a sixth source every round. Superusers must appear on the
  expected-writer list BY NAME, which is why that hole survived four rounds.
- **Six test binaries were dark.** 52 tests against real PostgreSQL executed in no CI job.
  `check:postgres-cargo-map` passed throughout, because it validates the entries that exist rather
  than coverage of the ones that do not — two gates, two subjects. `--update` would also have turned
  the gate green, by writing the six into the baseline as permanently exempt; they were wired into
  the map instead so they actually run.
- **Salvage on rebase (2026-08-10).** Migrations 0213–0215 now `REVOKE ALL … FROM console_rt`
  before intentional `GRANT`, so omitted DML verbs are actually withheld. Workspace-inherited
  dependency aliases remain unhandled and are tracked as bead `console-ugg` — `cargo metadata` is
  the total primitive; another text-scan patch is the wrong shape (third spelling).

## Two corrections worth recording, because both nearly shipped

**A test that builds its own subject measures the stub, not the deployment.** Six tests proved the
dispatch derivation total over `DispatchTarget::ALL` and all six passed — while the production
composition root registered ZERO of the thirteen canonical targets. Both facts were true at once,
because those tests construct their own registry from stub ports and then measure what they built.
The condition ("actions do not require a hand-written closure per action") is a claim about the
DEPLOYED registry, so it is now also measured by
`the_wired_registry_resolves_every_canonical_dispatch_target`, which drives the real
`projected_dispatch_registry`. RED proof: dropping one `register_port` line yields
`the deployed registry resolves no port for ["hr.appoint", "hr.promote", "hr.transfer"]` — exactly
the targets that port owns, so the control cannot pass vacuously.

**A rebase against superseded history is not a merge conflict, it is the wrong tool.** Replaying 61
commits onto the merged main stopped 13 in, with every conflict an intermediate state arguing with a
newer final one: `origin/main` already carried this branch's authz branch-provenance and
grant-validity types under a different commit subject, and #612 had landed ten rounds of harness
hardening the branch predates. The tip already contained the work. Measuring the two-dot delta
first — 78 files, and only 7 where main was ahead — turned a guess into a decision.

Two things that measurement caught, which the replay would have shipped: an exclusion loop that read
"P4 lacks a file main added" as "P4 deletes it" and staged a deletion of #612's own ledger entry
(the merge-base is the arbiter — only a file present THERE and absent in the branch is a real
deletion); and three `employees` DML sites in `hr.rs` where main is the OLD state and their removal
is this branch's deliverable, not a regression.

## Verification

`check:doc-links`, `check:doc-manifest`, `check:doc-citations`, `check:postgres-cargo-map`,
`check:executed-tests`: PASS. `postgres-shard.test.mjs`: 5/5. Migration contiguity 0212–0215:
PASSED, and measured both ways — vacating 0212 yields
`[NonContiguousMigrationVersion] missing migration version 0212 before 0213`. Dark binaries 6 → 1.

The shard tripwire moves 82 → 84, not 82 → 88: only `employment` and `pay_run` land in the domain
halves, the four ontology-family suites are counted by the `ontology` shard. Stated in the comment,
because a reader expecting +6 would "correct" it to 88 and turn it red for the wrong reason.

## HOLDs remaining

Unchanged. No production promotion, no frontend, no payment execution, no invented compliance scope.

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
    "hr_payroll"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Essentialism / YAGNI",
    "Red Team",
    "Systems Thinking",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Separated what a test constructs from what production deploys: six green tests measured a registry they built from stubs while the composition root wired none of it, and both facts were true at once.",
    "Essentialism / YAGNI": "register_port is called once per canonical OBJECT, so six lines cover thirteen targets and a fourteenth needs no edit; the alternative satisfies the condition's letter and violates it exactly.",
    "Red Team": "Treated the gate as the adversary's target. has_table_privilege subsumes direct grants, column grants, ownership, recursive membership and superuser, so a privilege path cannot be routed around the five catalogs an earlier revision unioned by hand.",
    "Systems Thinking": "Six test binaries passed locally and executed in no CI job, because check:postgres-cargo-map validates the entries that exist rather than coverage of the ones that do not — two gates whose subjects only appear to overlap.",
    "Operability / Day-2": "The shard tripwire moves 82 to 84 rather than 88 and says why in the comment, so the next reader does not 'correct' it to the intuitive number and turn it red for the wrong reason.",
    "Blast-radius / cell-based": "Rebuilt onto merged main rather than replaying 61 commits against superseded history; the two-dot delta bounded the change to 78 files with only 7 where main was ahead.",
    "Telemetry-first": "The writer-ownership gate reports the production file count it scanned (254 against 253 at the prior tip), so a run that observed nothing is distinguishable from a run that observed and agreed.",
    "Zero-trust / defense-in-depth": "No claim is trusted from the layer that makes it. The static crate-boundary half does not trust itself and says so; the database half re-asks PostgreSQL directly; the deployed dispatch registry is measured by a test that drives the real constructor rather than a fixture; and the ratchet's emptiness is pinned from both directions, so delisting without removing the writer and removing without delisting are each red."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "A test that constructs its own subject proved a derivation total while production registered zero of thirteen targets.",
    "Six test binaries carrying 52 real-PostgreSQL tests executed in no CI job, and the map gate passed throughout because coverage is not its subject.",
    "check-executed-tests.mjs --update would have turned the dark-test gate green by declaring the six permanently exempt.",
    "A rebase against superseded history produced conflicts that were intermediate states arguing with newer final ones.",
    "An exclusion loop read 'absent from this branch' as 'deleted by this branch' and staged the removal of a file main had added.",
    "Default GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO console_rt left omitted verbs live until explicit REVOKE.",
    "Workspace-inherited dependency aliases are not resolved by manifest text scan; tracked as console-ugg rather than a fourth enumeration patch."
  ],
  "decisions_changed_or_rejected": [
    "Rejected replaying the branch commit-by-commit, because main already carried its authz types under a different subject and its harness ten rounds newer.",
    "Rejected --update on the executed-tests baseline, because exempting a dark test is the false green the gate exists to prevent.",
    "Rejected renumbering migration 0212, measured: vacating it yields NonContiguousMigrationVersion and origin/main tops out at 0211."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
