# Credential-reset runtime evidence

This archive contains 404 allowlisted regular files for source candidate `930a04d71053314d6172331ca14a939e1df8b459`: the reviewed RED/GREEN runs, initial failed diagnostics, corrected full diagnostics, exact runner inputs, strict Clippy, formatting and source checks. Original reports and raw command-log bytes are preserved.

Archive SHA-256: `d7ed20fe725a630cc1473bf1ebf1d60d1542081690905a0371c7d6b7100c53f5`. Manifest SHA-256: `f7711f00666996942f8b0b832a3f7b7f79079e3c27b7a1ac1f39aadb7b3da16b`.

`manifest.json` maps every archive member to its original local path, byte count and SHA-256. Historical absolute paths inside the reports identify their original custody; the manifest supplies the corresponding portable archive locations. No cache, environment file, database, business document or source archive is included.

The reset lane discovered39, selected9 and filtered30: RED7 reviewed behavioral failures/2 controls; unchanged GREEN9/9. Full final diagnostics execute87 without skips or filters: provisioning48/48 passes; authentication remains FAILED37/39 at the two inherited OTP-purpose assertions. The initial diagnostic's8 fixture-environment failures and1 schema-expansion expectation failure remain preserved under `initial-diagnostic/`; their repairs and fresh outcomes are documented in the parent evidence record.

Inspect the member list with `tar -tzf runtime-evidence.tar.gz`; extract into a new empty directory. Check regular members against the plain manifest before running any included tooling. Runners describe the original isolated disposable environment and require their pinned source commits, images and task cache; they are not installation or production operations.

This proves the bounded credential-reset repair. It does not clear the full authentication, attended identity/recovery authority, Next browser/UI, two-site durability or launch acceptance gates.
