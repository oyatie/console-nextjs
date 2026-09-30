#!/usr/bin/env bash
# Local build acceleration for parallel lane worktrees. Source it, do not run it:
#
#   source scripts/console/lane-env.sh
#
# Why this is a script and not `.cargo/config.toml`:
# putting `rustc-wrapper = "sccache"` in repo config would apply in CI too, and
# every Rust job would fail on runners without sccache installed. Lanes are a
# local concern, so the wrapper is opted into locally.
#
# Measured problem this solves (lane rehearsal, 2026-07-28): each git worktree
# gets its own `backend/target`, so lanes never contend on the cargo build lock —
# but they also share nothing. A build of one target reported `Cache hits: 0%`
# across 4,084 commands. Every lane recompiles the entire dependency graph.
# sccache caches at the rustc-invocation level, so lanes keep separate target
# directories AND reuse each other's compiled artifacts.

set -u

if ! command -v sccache >/dev/null 2>&1; then
  printf 'lane-env: sccache not found on PATH; install it (brew install sccache) for cross-lane cache reuse\n' >&2
  return 1 2>/dev/null || exit 1
fi

export RUSTC_WRAPPER=sccache
# Without this the wrapper above buys NOTHING. Cargo defaults `incremental` on for
# the dev and test profiles, `backend/Cargo.toml` does not override it, and rustc
# invoked with `-C incremental=...` is not a cacheable compilation — sccache does
# not miss it, it never sees it.
#
# Measured on one crate, `cargo clean -p` then `sccache --zero-stats` before each:
#   incremental on   -> 0 Rust hits, 0 Rust misses, 0 non-cacheable  (invisible)
#   CARGO_INCREMENTAL=0 -> 1 Rust miss                               (entered the cache)
#
# `ci.yml` already sets this in three places; this line makes the local lane
# environment agree with the one place the cache is actually shared between lanes.
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
# Generous ceiling: this workspace is large and disk is not the constraint
# (measured 4.0 TiB free, 1% used).
export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-50G}"

sccache --start-server >/dev/null 2>&1 || true

printf 'lane-env: RUSTC_WRAPPER=sccache, cache ceiling %s\n' "$SCCACHE_CACHE_SIZE"
sccache --show-stats 2>/dev/null | awk '/Compile requests|Cache hits|Cache misses/ {print "  " $0}'
printf 'lane-env: run `sccache --show-stats` after a build to confirm hits are climbing\n'
