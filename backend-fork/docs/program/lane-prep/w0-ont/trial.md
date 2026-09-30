# Trial receipt — w0-ont

Status: **trial not yet run** in this prep pack. Commands and expected green shape are fixed below for admission-time execution.

## Purpose

Prove 1–3 targeted Cargo packages under the ontology allowlist are green on the immutable base (or post-increment head) without UI or foreign crates.

## Exact invocations (planned)

Run from repository root with the workspace Cargo.toml:

```sh
cargo test -p console-ontology-domain --lib
cargo test -p console-ontology-application --lib
cargo test -p console-ontology-adapter-postgres --lib
```

Optional third surface if adapter lib alone is insufficient for the chosen AC:

```sh
cargo test -p console-ontology-rest --lib
```

Postgres-backed integration (only if environment has disposable DB + role fixtures):

```sh
cargo test -p console-ontology-adapter-postgres --test key_write_cas_as_runtime_role -- --test-threads=1
```

## Expected green shape

| Command | Expected (shape, not frozen counts) |
|---|---|
| `console-ontology-domain --lib` | all unit tests pass; 0 failed |
| `console-ontology-application --lib` | all unit tests pass; 0 failed |
| `console-ontology-adapter-postgres --lib` | unit tests pass; integration tests may be skipped if no DB — **record skip vs execute explicitly** |
| focused `*_as_runtime_role` | if run: 0 failed; `--test-threads=1` preferred for RLS |

**Not green:** zero tests discovered, compile-only success without test binary run, or ignored suite without receipt.

## Environment notes

- Pure domain/application: no Postgres required (fact from crate layout: pure types + application helpers).
- Adapter/rest integration: many files under `adapter-postgres/tests/` and `rest/tests/` use runtime-role Postgres harnesses.
- Do not treat Buck as the Cargo membership authority for this trial; Cargo is the target entrypoint.

## Receipt fields (fill when run)

| Field | Value |
|---|---|
| Base / head SHA | *TBD* |
| Date | *TBD* |
| Operator | *TBD* |
| Discovered counts | *TBD* |
| Executed counts | *TBD* |
| Failures | *TBD* |
| Result | **not run** |

## Non-claims

This trial does not prove product conformance for Company/HR, frontend readiness, or HOLD clearance.
