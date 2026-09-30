# 2026-08-02 — the crate 152 others depend on ran its tests nowhere

`console-kernel-core` appears nowhere in `ci.yml`. Its **43 lib tests pass and have never executed
in CI**. 152 `Cargo.toml` files depend on the crate.

It was found while wiring a type boundary whose entire proof is 12 `compile_fail` doctests — which
would also have run nowhere, so the boundary would have shipped with no automated guard of any kind.

`check-executed-tests.mjs` cannot see this class. It counts crate roots under `tests/`, and these are
`#[cfg(test)]` inside `src/`. That is the **third** population the gate is blind to, after the 14
`.test.mjs` suites and the Buck wrappers with no workflow path. The gate's own header states the
right rule — *"a rust_test target is not wiring; a crate is not wiring; only a path from a workflow"*
— and applies it to one population out of three.

Both invocations join the existing `Domain crate unit tests` step rather than adding a new step name,
so nothing has to be re-declared in `scripts/verify.mjs`, whose completeness checks fail closed in
both directions. `--doc` is separate and deliberate: `compile_fail` doctests are the only artifact
that can hold a NEGATIVE claim, and `cargo test` does not run doctests without it. Proven
load-bearing — a planted `compile_fail` block that actually compiles exits 101.

Every capability, evidence contract, jurisdiction binding, Korea control, review disposition, and
exposure state remains `HOLD`.
