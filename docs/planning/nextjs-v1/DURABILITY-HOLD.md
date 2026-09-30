# Two-site confirmation: implementation hold

Base: `346db7a44568e4eeb5481232cbdf621db3723e4e` in the independent backend fork. This note is an implementation finding, not a deployment or release receipt.

## Observed failure

`backend/app/tests/two_site_confirmation.rs` commits an actual row on a disposable PostgreSQL 18 primary with `synchronous_commit=remote_apply`, captures a post-commit WAL insert LSN, and verifies that `pg_stat_replication` has no physical standby. It first requires `SHOW synchronous_standby_names` to be empty and sets a five-second transaction statement timeout, so a named-but-missing standby cannot leave the probe waiting indefinitely. `/readyz` still returns HTTP 200 with `status=ready` and `database=ready`. The current readiness owner checks only `SELECT 1` (`backend/app/src/lib.rs:3786-3867`). The exact focused test discovered and executed one case: zero passed, one failed at the expected write-readiness assertion. No application behavior was changed. Captured red output SHA-256: `de0aecd56301f2208e5c0e0edf1cb59974309940cf1e64e73826a98a7cee42e1`.

Run the red probe with a disposable PostgreSQL 18 `DATABASE_URL` and the backend-pinned toolchain:

```sh
RUSTUP_TOOLCHAIN=1.98.1 SQLX_OFFLINE=true DATABASE_URL=<disposable-primary-dsn> \
  cargo test --manifest-path backend/Cargo.toml -p console-app \
  --test two_site_confirmation -- \
  --exact committed_mutation_is_not_ready_without_independent_replica --nocapture
```

The repository lane harness admitted this exact red command. Its first invocation from the worktree root selected the default nightly toolchain; the pinned-toolchain rerun was also admitted. The directly observed failure above came from `cargo test` under the backend toolchain.

## Why this cannot be made green by a narrow readiness patch

The backend has no current-primary identity or authenticated independent-standby connection, no PostgreSQL system-ID/timeline plus WAL confirmation observer, no monotonic writer/egress epoch lease, and no positive old-process/network fencing source. Domain owners each commit through their own PostgreSQL adapter; external sends and protocol acknowledgments can occur outside an HTTP response wrapper. A standby count, `remote_apply` setting, or successful COMMIT cannot establish the plan's post-commit two-site confirmation barrier.

The first safe implementation stage must supply independently authenticated primary and full-standby endpoints, a fenced writer/egress epoch bound to their system ID and timeline, and a common confirmation interface used before every acknowledged write, outbox egress, protocol admission, and live read publication. A read-only one-site recovery state needs its own certified safe-prefix path and explicit readiness mode. Physical site-loss, dropped-COMMIT-response, epoch change, stalled replay, and stale sender tests must precede write-ready activation. Until then, the v1 two-site write-ready mode remains unavailable.

## Risk and rollback

Premortem: a partial readiness fix could report green while domain adapters or direct protocols still emit unconfirmed effects. Blast radius is every acknowledged business write and external action. Detection is the red probe plus end-to-end fault tests. Rollback for this lane is removal of the non-activated test/note only; no applied migration, production route, credential, or external effect changed. Stop on missing independent site evidence, lease authority, fencing, or any confirmed response without the barrier. Independent architecture/security review remains pending.

## Audited-prefix candidate held — 2026-09-27

An isolated candidate `f8c29c5` added exact Company/action/target audit-row checks against a PostgreSQL 18 primary and physical standby. Its fault test passed **1/1**, including paused replay, a canceled synchronous COMMIT wait and a non-BYPASSRLS test observer (`.artifacts/durability-audit-physical-f8c29c5.log`, SHA-256 `d3bd4ff60f7efe32f98552c8d6f5b743cfdbb527b912397720d5f42a7509ecaa`). It was **not integrated** into `backend-fork`.

The candidate changed the audited path's `ConfirmedWalPrefix` from the required post-COMMIT primary INSERT-LSN upper bound to an initial FLUSH bound and a final standby replay LSN. The physical test exposed why: an audit SELECT can emit hint-bit WAL, advancing INSERT-LSN while flush/replay remains idle; retrying with a freshly captured bound can wait indefinitely for unrelated observer WAL. The narrower audit-row custody proof cannot serve as the plan's generic acknowledgment/read barrier. Independent review agreed the shared prefix type made that mismatch easy to misuse. A production observer role that can set `app.current_org` also needs separate cross-Company credential custody; non-BYPASSRLS alone does not confine it. A separately reviewed design must preserve a stable primary INSERT bound across retry and prove liveness without silently lowering the bound. No serving acknowledgment, external egress or deployment is authorized from this result.
