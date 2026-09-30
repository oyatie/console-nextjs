# Trial receipt — w0-apr

Status: **trial not yet run** in this prep pack.

## Purpose

Prove governance pure-domain tests and the four-eyes / distinct-human adapter suite are green, establishing the verification baseline for preflight + approval work.

## Exact invocations (planned)

```sh
cargo test -p console-governance-domain --lib
cargo test -p console-governance-application --lib
cargo test -p console-governance-rest --lib
```

Postgres-backed four-eyes / distinct-human (preferred trial set — 1–3 files):

```sh
cargo test -p console-governance-adapter-postgres --test four_eyes_bind_consume -- --test-threads=1
cargo test -p console-governance-adapter-postgres --test approvals_create_as_runtime_role -- --test-threads=1
cargo test -p console-governance-adapter-postgres --test governance_rls_as_runtime_role -- --test-threads=1
```

## Expected green shape

| Command | Expected |
|---|---|
| `console-governance-domain --lib` | FSM + gate-chain unit tests pass; 0 failed |
| `console-governance-application --lib` | command/type tests pass if present; 0 failed |
| `console-governance-rest --lib` | compile + any unit tests; 0 failed |
| `four_eyes_bind_consume` | bind, single-use consume, wrong-target reject paths pass |
| `approvals_create_as_runtime_role` | create → distinct decide path passes |
| `governance_rls_as_runtime_role` | self-approval rejected (store + DB CHECK); distinct approval append-only |

**Preflight non-mutation signal:** domain `evaluate_gate_chain` remains pure (no I/O). REST preflight must not call consume/write helpers — verify by code review + tests when AC requires.

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

Green four-eyes tests do not prove frontend four-eyes UX, multi-stage routing product parity, or legal/compliance sufficiency of approval artifacts.
