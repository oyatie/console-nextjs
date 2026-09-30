# Trial receipt — w0-pol

Status: **trial not yet run** in this prep pack.

## Purpose

Prove policy package tests green under fail-closed semantics without frontend or enforcement-promotion work.

## Exact invocations (planned)

```sh
cargo test -p console-policy-domain --lib
cargo test -p console-policy-application --lib
cargo test -p console-policy-adapter-postgres --lib
```

If adapter integration tests are required for the chosen AC:

```sh
cargo test -p console-policy-adapter-postgres --test draft_storage -- --test-threads=1
```

Only if platform packages were admitted into the allowlist for this increment:

```sh
cargo test -p console-platform-authz --lib
cargo test -p console-platform-authz --test cedar_pbac_readiness_cases
```

## Expected green shape

| Command | Expected |
|---|---|
| `console-policy-domain --lib` | unit tests pass (enum/status/validation); 0 failed |
| `console-policy-application --lib` | unit tests pass (query normalize / draft orchestration); 0 failed |
| `console-policy-adapter-postgres` | lib + any executed integration: 0 failed; Postgres absence must be explicit not silent pass |
| platform packages (if any) | 0 failed; no test ignored without receipt |

**Fail-closed signal:** tests that expect `Err` / validation / non-enforced status on bad input must remain present and passing — do not delete or `#[ignore]` them.

## Receipt fields (fill when run)

| Field | Value |
|---|---|
| Base / head SHA | *TBD* |
| Date | *TBD* |
| Operator | *TBD* |
| Discovered counts | *TBD* |
| Executed counts | *TBD* |
| Failures | *TBD* |
| Security reviewer ids | *TBD* (required before merge) |
| Result | **not run** |

## Non-claims

Passing these tests does not prove live-route Cedar enforcement, production authz cutover, or legal compliance.
