# 2026-08-02 — sccache never saw a single compilation

`scripts/console/lane-env.sh` is the **only** place `RUSTC_WRAPPER=sccache` is exported, and it never
set `CARGO_INCREMENTAL`. Cargo defaults `incremental` on for the `dev` and `test` profiles;
`backend/Cargo.toml:359-363` sets only `debug = "line-tables-only"` on both and does not override it.
rustc invoked with `-C incremental=…` is not a cacheable compilation.

So the failure was not a low hit rate. Measured on one crate, `cargo clean -p` and
`sccache --zero-stats` before each run:

| | Rust hits | Rust misses | non-cacheable |
|---|---|---|---|
| incremental on (what lanes had) | 0 | 0 | 0 |
| `CARGO_INCREMENTAL=0` | — | 1 | 0 |

Zero across every counter is not a miss. **sccache never saw the compilation.**

This is the second time this wrapper has been wired and measured to be doing nothing. The first was
that nothing sourced `lane-env.sh` at all, so `Compile requests` read 0 across the whole program
while every lane recompiled cold. Both failures share a shape: the configuration was correct and
the thing it configured was never on the path that mattered. A cache is not observable from its
config; only its counters say whether it ran.

`ci.yml` already sets `CARGO_INCREMENTAL=0` in three places, so this makes the local lane
environment agree with the one place a shared cache exists between lanes. Written as
`${CARGO_INCREMENTAL:-0}` so an explicit caller override still wins; both paths verified.

Every capability, evidence contract, jurisdiction binding, Korea control, review disposition, and
exposure state remains `HOLD`.
