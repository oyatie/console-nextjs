# First-party documentation manifest ratchet candidate

## Scope and authority

- Issue: [#567](https://github.com/jason931225/console/issues/567), cross-referencing [#565](https://github.com/jason931225/console/issues/565).
- Protected base: `684f89371c4bfbd65bcf4a96a9edae49e4e032b6`.
- Slice worktree: `slice/s1-doc-manifest`, created from that exact base.
- This record describes candidate evidence. It is not current product authority and does not authorize S2 until S1 is merged and read back.
- The slice changes documentation custody and CI admission only. It changes no product implementation, database, migration, deployment, authority register, archive tag, or HOLD-governed content.

## Classification contract

The candidate classifies every tracked first-party Markdown regular blob outside `buck-out/`, `node_modules/`, `target/`, and `third-party/`. A document class records authority and lifecycle, not whether a gate reads the file. The closed classes are `current`, `decision`, `executable-contract`, `evidence`, `historical`, and `quarry`.

`docs/documentation-manifest.seed.json` is the reviewed semantic source. A human owns `class`, `owner`, `status`, `replacement`, and `retention`; `archive_tag` remains `null`. `scripts/console/generate-documentation-manifest.mjs` owns `path`, exact Git index-tree `blob_sha`, and the generated `documents` projection in `docs/documentation-index.json`. `--write` never supplies a missing semantic field.

The generated index uses schema version 2 and coverage `first-party-manifest`. It preserves the pre-existing README entry, three current authorities, and three transitions byte-for-value and requires their document projections to remain equal.

## Pilot-first evidence

The reviewed pilot contained these 12 paths:

1. `README.md`
2. `backend/crates/platform/db/README.md`
3. `deploy/README.md`
4. `docs/CI-GATES.md`
5. `docs/benchmarks/enterprise-parity-matrix.md`
6. `docs/current/PRODUCT.md`
7. `docs/decisions/README.md`
8. `docs/ideas/lane-assembly-line.md`
9. `docs/program/ledger/2026-08-01-candidate-sha-leaves-the-registers.md`
10. `docs/retros/2026-07-03-m1-pr152-intent.md`
11. `docs/runbooks/non-oci-talos-mail-imessage-relay.md`
12. `docs/specs/rbac-configurable.md`

The pilot check reported `documentation manifest OK (12 markdown files)`. Two writes produced identical SHA-256 `901e67f7158bbce2967081c0884f74069b3eab9bdd3faf37bc270b63cfa36172`. Removing `class` remained missing after `--write` and made `--check` fail. A manifest path escaping the worktree failed closed.

The first split-context review returned `BLOCK / REQUEST CHANGES`. Its objections and dispositions were:

- `docs/CI-GATES.md` was changed from `historical` to `executable-contract` because its path-stable inventory is data by construction, not merely because gates consume it.
- Invented owner labels were replaced with the established accountable value `repository maintainers`; no unapproved owner-registry path was added.
- `status` and `retention` vocabularies and the exact eight-field seed shape were closed.
- Pilot and custom-file regeneration diagnostics now preserve their scope.
- The generator now uses sanitized `write-tree` plus `ls-tree` custody and fails closed on unsupported index entries.
- The runbook and RBAC reference remain `historical` but are `active` maintained references.
- The seven existing records use an exact-equality projection model.
- Prefix rules remain proposal inputs; nature-based per-file exceptions govern the full seed.

The repaired pilot review returned `WATCH / APPROVE`; its sole watch was the full-seed nature review that followed.

## Full-seed review

Before this ledger file was added, the candidate contained 381 classified Markdown records:

| Class | Count |
|---|---:|
| `current` | 4 |
| `decision` | 43 |
| `executable-contract` | 2 |
| `evidence` | 163 |
| `historical` | 145 |
| `quarry` | 24 |

Statuses were 147 `active`, 232 `frozen`, and 2 `redirect`.

An independent split-context reviewer inspected the frozen staged tree `e8feda4992f6757b9068d127eebe122a8023c1db` and cached-diff hash `dccd3c00bd4df5a62bcf6933cc7dec89c9d8eaf0`. The verdict was `WATCH / APPROVE` with zero blockers. The review signed off all 49 `current`, `decision`, and `executable-contract` rows, all 24 quarry rows, all seven projections, every contested prefix/lifecycle cluster, and the P1-P7 fixture substance.

Three low, accepted caveats remain recorded rather than hidden:

1. `CHANGELOG.md` is `evidence/frozen` even though release automation appends it; each append therefore deliberately requires regeneration and review.
2. `check-doc-links` still understands legacy `authority-slice`, while the separate manifest gate and generated-byte comparison prevent a schema-v2 downgrade from passing the complete CI sequence.
3. Live agent instruction files fit `historical/active` because the closed authority vocabulary has no operational-tooling class; they are migration candidates only if a later approved vocabulary change exists.

Adding this ledger record increases the final candidate universe to 382 Markdown files and invalidates that earlier staged-tree hash. A fresh exact-candidate review is required after regeneration.

## Planted mutation contract

The hermetic temporary-repository suite watches these failures red and restores green:

- P1: add and stage `docs/tmp-unclassified.md`; failure names that path.
- P2: modify and stage `docs/CI-GATES.md` without regeneration; failure names `blob_sha` drift.
- P3: delete one `documents` record; failure names the missing path.
- P4: set coverage to `complete`; the existing fail-closed coverage error remains.
- P5: set an `archive_tag` non-null; failure records that signed-archive validation is absent.
- P6: remove `documents` from `rootIndexFields` while the index retains it; failure reports `unexpected field: documents`.
- P7: remove a seed row's `class`, run `--write`, then `--check`; the class remains missing and check exits non-zero.

No fixture skips, deletes, or replaces an existing test.

## Verification evidence and limits

Baseline measurements at the slice base were:

- Documentation-link tests: 25 discovered, 25 executed, 25 passed, zero skipped/todo.
- CI-preflight tests: 52 discovered, 52 executed, 52 passed, zero skipped/todo.
- Local-verifier contract tests: 13 discovered, 13 executed, 13 passed, zero skipped/todo.

Candidate measurements before this ledger file were:

- Documentation-link/manifest tests: 36 discovered, 36 executed, 36 passed, zero skipped/todo; `documentation manifest OK (381 markdown files)`; `doc links OK (381 markdown files)`.
- ADR tests: 29 discovered, 29 executed, 29 passed, zero skipped/todo; the live ADR gate passed 38 ADRs and four design notes.
- CI-preflight tests: 53 discovered, 53 executed, 53 passed, zero skipped/todo; the live preflight gate passed.
- Local-verifier contract tests: 13 discovered, 13 executed, 13 passed, zero skipped/todo.
- Reasoning-lens tests: 40 discovered, 40 executed, 40 passed, zero skipped/todo; the structural gate passed.
- Foundation gate: 134 checks passed.
- Documentation citations and staged diff checks passed.

`npm run verify` exercised its broad local mirror but retained two unsuppressed pre-commit failures: the immutable generated-face snapshot saw staged `package.json` bytes differing from the HEAD snapshot, and the existing Console C/T/M authority train failed its direct-child relation. Neither failure is waived. The first must be re-evaluated from a committed candidate. The authority registers are outside this slice; a remaining authority-train failure must follow the established DELIVERY candidate procedure or be escalated as pre-existing evidence, never “fixed” by expanding S1.

All commands must be rerun after this file is classified and the final staged tree is frozen. Hosted CI and merged-main read-back remain outstanding.

## Pre-mortem disposition

- **Wrong broad prefix rule:** pilot-first and independent nature review found and corrected it before full generation.
- **Stubbed green command:** real temporary Git repositories, directional mutations, and exact diagnostics prove red behavior.
- **Semantic fields silently invented:** generated writes are restricted to `path` and `blob_sha`; missing semantics remain red.
- **Gate consumption erases measured debt:** class is independent of the future `gate_inputs` relation.
- **Generated output hand-edited:** byte equality and regeneration checks make the generator the only index writer.
- **Reviewer fatigue over hundreds of rows:** current/decision/executable rows receive explicit signoff and contested clusters receive adversarial review.
- **External artifact leaks private context:** issue and ledger wording is repository-grounded and public-safe; third-party lessons remain heuristic, not authority or compliance conclusions.

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
    "Cartesian doubt": "Treats a green documentation scan, broad prefix rule, generated index, and review count as claims requiring exact-tree and mutation proof rather than as authority by assertion.",
    "Red Team": "Plants unclassified files, stale blob hashes, missing records, false coverage, archive tags, schema drift, and missing semantic fields in disposable Git repositories.",
    "Systems Thinking": "Separates document authority, lifecycle, generated custody, CI consumption, issue state, candidate review, and later gate-input provenance so one relation cannot silently redefine another.",
    "Operability / Day-2": "Provides deterministic regeneration, exact diagnostics, documented ownership, additive CI wiring, and a bounded recovery path for every ordinary documentation edit.",
    "Blast-radius / cell-based": "Confines the slice to documentation and repository-gate paths in a fresh worktree while leaving product code, databases, deployments, authority registers, and HOLD-governed domains unchanged.",
    "Telemetry-first": "Publishes exact base and staged-tree identities, per-class counts, discovered-versus-executed test counts, review verdicts, planted mutations, and explicit remaining failures.",
    "Zero-trust / defense-in-depth": "Combines sanitized exact-index custody, regular-blob refusal, closed schemas, generator byte equality, independent review, local gates, hosted CI, and merged-main read-back without allowing any one proof to substitute for the rest."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "A document manifest is trustworthy only when semantic review is independent of generated path and blob custody.",
    "A gate-input relation must remain separate from class or the measured prose dependency can erase itself by wiring.",
    "A full local verifier can remain red for candidate-construction reasons without licensing suppressed checks or forbidden authority-register edits."
  ],
  "decisions_changed_or_rejected": [
    "Rejected one-pass full-corpus prefix classification in favor of pilot-first derivation and split-context review.",
    "Rejected invented owner labels, semantic defaults, hand-edited generated output, and archive-tag admission.",
    "Rejected treating a dated ledger, third-party heuristic, or green local subset as current authority or merge proof."
  ],
  "lens_set_changes": [
    "Added Systems Thinking because document class, lifecycle, generated custody, CI consumption, release automation, and future provenance measurement interact but must remain distinct."
  ]
}
```
<!-- REASONING-LENS-EVIDENCE:END -->
