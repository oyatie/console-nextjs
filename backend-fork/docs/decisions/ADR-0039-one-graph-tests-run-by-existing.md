---
id: ADR-0039
status: proposed
doc_status: review
date: 2026-08-02
owner: jasonlee
decision: one-graph-tests-run-by-existing
related: [ADR-0037, ADR-0038]
---

# ADR-0039 — One graph: a test runs because it exists

**Status:** Proposed · **Date:** 2026-08-02 · **Supersedes:** nothing · **Relates to:** ADR-0037, ADR-0038

> **Measurement boundary and current disposition (2026-08-03).** Quantities and failure examples in
> this proposal describe the pre-consolidation snapshot measured on 2026-08-02; they are not
> present-tense claims about HEAD. Since that measurement,
> `//tools/buck:equipment-3r-http-postgres` has been restored,
> `check-executed-tests.mjs` discovers JavaScript suites and `#[cfg(test)]` modules throughout crate
> `src/` trees and fails closed when a target root is missing, and `tools/lanes/pgtest.sh` refuses
> credentials in argv. Those repairs close the named immediate holes. The proposed Cargo/nextest
> convergence remains a future simplification and grants no authority while this ADR is proposed.
>
> **Disposition (2026-08-05).** RE/CAS for Buck2 remains absent; cargo portable cache (GHA
> `rust-cache` + local `sccache`) is the only working cross-run/local compile reuse for this
> workspace. Product PostgreSQL has begun cargo-native sharding (S0–S2). **DN-0005** records the
> binding inventory and the phased cutover (equivalence → nextest serial groups → residual switch →
> drop Buck product jobs; faces carved out). This ADR remains **proposed** and grants no Buck
> deletion authority until sequence steps 1–3 have dual-green evidence. Generated faces are out of
> scope for the product-test cutover. Do not keep Buck as the product test driver “until RE lands.”

## The obligation this is anchored to

Not a statute — a measured defect. Commit `2340fa99c` (#550) spent **1,847 lines of registration** to
make **167 already-written tests execute**:

| file | lines added | of which literal target names |
|---|---:|---:|
| `tools/buck/BUCK` | 1,354 | 205 wrappers |
| `scripts/check-ci-preflight.mjs` | 321 | 159 |
| `.github/workflows/ci.yml` | 172 | 159 |
| **total registration** | **1,847** | |
| actual new test code | 169 | |

`tools/buck/BUCK` went 218 → 317 → 387 → **1,753** lines in five days, 28 → 205 wrappers, across nine
commits with **zero deletions, ever**. The driver was not Buck2 adoption. It was this repository doing
the right thing about dark tests. **Every test rescued from darkness costs a wrapper.**

A cost that scales with the thing you most want to do is not a cost, it is a brake.

## The defect is not Buck2. It is hand-maintained cross-file agreement.

At the proposal snapshot, three lists had to agree, and **they did not**:

    .github/workflows/ci.yml         184 `//tools/buck:` names
    scripts/check-ci-preflight.mjs   185 `//tools/buck:` names
                                     ─── the extra one is `equipment-3r-http-postgres`,
                                         whose target does not exist

`scripts/check-executed-tests.mjs` exists solely to detect the failure this arrangement permits. It is
a gate written to catch a problem the design allows.

**So the removal of Buck2 is necessary and not sufficient.** Replacing 205 wrappers with 205
hand-written `-p crate --test name` lines moves the tax from six registration points to two or three.
The tax is the *lists*, not the tool.

## Decision

**A test runs because the file exists.** One graph — the filesystem plus `Cargo.toml` — and no list
anywhere that a human must edit when a test is added.

    cargo nextest run --workspace

That is the whole registration surface. A new test file is executed by having been written.

## The schema is the spec

Serialization is the only real constraint, and it is per-test, not per-job. Six files mutate
cluster-global state (`ALTER ROLE`), so they cannot share a cluster:

```toml
# .config/nextest.toml
[test-groups.cluster-global]
max-threads = 1

[[profile.default.overrides]]
filter = '''
  test(/leave_migration_expand_contract/) +
  test(/key_revision_migration_upgrade/) +
  test(/attendance_console_migration_contract/) +
  test(/apalis_adapter/) +
  test(/apalis_schema_contract/)
'''
test-group = 'cluster-global'
```

```yaml
# .github/workflows/ci.yml — the entire PostgreSQL test surface
- name: Workspace tests
  run: tools/lanes/pgtest.sh "$PWD" cargo nextest run --workspace
```

At the proposal snapshot the same property was bought with `--num-threads=1` at four sites in CI,
which serialises **every** test in the job to protect **six files**. That is why one job takes 41
minutes. A `test-group` serialises the six and leaves the rest parallel.

Per-test database isolation needs nothing new: **197 files already use `#[sqlx::test]`**, which
provisions and drops a database per test. That was never Buck2's contribution.

## What each pattern is, stated as a pattern

| need | pattern | not |
|---|---|---|
| test discovery | the filesystem is the declaration | a list of targets |
| per-test data isolation | a database per test, provisioned by the harness | a wrapper per test |
| cluster-global serialisation | a declared test group with `max-threads = 1` | a single-threaded job |
| credentials | written to a 0600 file, never to argv | trust that nobody inlines a URL |
| unreachability | **impossible by construction** | a gate that detects it |

## The consequence that justifies the change

`scripts/check-executed-tests.mjs` is **deleted, not extended.**

At the proposal snapshot it measured one of three populations: it tracked `tests/*.rs` crate roots,
was blind to `.test.mjs` suites and to `#[cfg(test)]` inside `src/`, and failed open when a target
root was absent. Those implementation defects are now fixed as recorded in the reconciliation
above; they remain historical evidence for why duplicated inventories are risky, not current bugs.

Under `--workspace` there is no set of executed tests distinct from the set of tests. The question the
gate answers stops being askable.

**This is the same move made three times in this repository in one week**, and it is the pattern
worth naming:

| problem | six rounds of | what actually ended it |
|---|---|---|
| personal data in a migration | hardening a SQL parser | pinning column *names* so the swap trick moved the set |
| three docs conflicting every merge | resolving conflicts | deleting the denormalised field so there was nothing to conflict on |
| tests that execute nowhere | a gate to detect them | running everything |

In each case the fix was not a better detector. It was removing the condition the failure required.
**A gate you can delete is worth more than a gate you can fix.**

## What this costs, honestly

- **`cargo-nextest` becomes a required tool.** It is installed locally today and is a single binary in
  CI. Its `test-groups` and `--partition` are stable; its `setup-scripts` are experimental and are
  **not** used here.
- **Compile time is unchanged.** This ADR claims no wall-clock win from the build system. The win is
  the 41-minute job becoming parallel except for six files, and 1,847 lines of registration not being
  written next time.
- **One control had to be re-homed before anything could be deleted.**
  `tools/buck/test_needs_postgres.sh:26` exits 2 when handed a raw `//backend/...` target, forcing
  credentials through a 0600 env file instead of argv. `tools/lanes/pgtest.sh` now sources
  `no-credential-in-argv.sh` before creating its disposable container, so this step-1 safety
  prerequisite is met. **No Buck2 file is deleted merely because that prerequisite landed.**
- **Dependency-direction enforcement does not change.** `backend/ci/gates/layer-boundary/` (839 src
  lines) keeps doing it. Buck2's `within_view` would have been a better mechanism — a load-time
  refusal rather than an after-the-fact judgement — but it has **zero occurrences** in this repository
  and first-party `visibility` is 677 declarations all set to `["PUBLIC"]`, so nothing is lost that
  was ever held.

## Sequence

Each step is useful on its own; none depends on the next being taken.

1. **The safety net.** Generate the equivalence map and assert the Cargo-executed crate-root set is a
   *superset* of the Buck2-executed set. Re-home the credential refusal. **Deletes nothing.**
2. **Adopt nextest with the test group.** Both paths run; both green. Prove the six serialised files
   are the only ones serialised.
3. **Switch CI to the Cargo path**, Buck2 jobs still present. The superset assertion is what makes
   this safe.
4. **Remove the Buck2 CI jobs**, files retained. Reversible by one revert.
5. **Delete the files** — 278 tracked, 61,123 lines — and the `macos-latest` `generated-face-authority`
   job, which exists at GitHub's 10× multiplier solely to prove the Buck2 files match the Cargo files.
6. **Delete `check-executed-tests.mjs`** and the 197 Buck-coupled lines of `check-ci-preflight.mjs`.

Step 5 is the only irreversible one and it comes after two green steps.

## What was considered and rejected

| option | why not |
|---|---|
| Keep Buck2, enable a remote cache | REAPI gRPC, so a plain bucket does not work; needs Buildbarn/NativeLink/BuildBuddy. And `toolchains/BUCK:17` pins `system_cxx_toolchain(compiler = "/usr/bin/clang")`, which is not hermetic, so RE needs a toolchain rewrite first. Caches the 695 s compile, not the 1,585 s of serialised tests. |
| Keep Buck2, adopt `within_view` | Buys a load-time refusal that an existing 839-line gate already provides, at the cost of the entire second graph. |
| Keep Buck2 for affected-target selection | `uquery "rdeps(//backend/..., …)"` exits 3 — `Unknown target cxx_no_default_deps from package toolchains//` — and always has. The required gate only runs `kind('rust_test', '//backend/...')`, which does not traverse deps, so nobody noticed. |
| Bazel + `rules_rust` | Trades a 1,470-line generator and 168 generated BUCK files for `crate_universe` and generated BUILD files — the same synchronisation cost that is the complaint. |
| Nx / Turborepo / moon / Pants / Please / Earthly | Pants and Please cannot compile Rust; Earthly is unmaintained; Turborepo and Nx need ~166 fabricated manifests; moon documents that it must not cache Rust artifacts. |

## Open question for the owner

Steps 1–4 are reversible and step 5 is not. The question is whether to stop after step 4 — Buck2
present but unused — for one release, or to complete the removal in the same cycle. Keeping dead
build files is its own cost, so the argument for pausing is only that a revert stays cheap.

Every capability, evidence contract, jurisdiction binding, Korea control, review disposition, and
exposure state remains `HOLD`.
