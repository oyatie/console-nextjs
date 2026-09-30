---
id: DN-0005
kind: design-note
parent_adr: ADR-0039
authority: subordinate
activation: planning
date: 2026-08-05
owner: jasonlee
---

# DN-0005 — ADR-0039 under RE/CAS absence: cargo cache path and phased cutover

## Status

**Planning record** under proposed ADR-0039. This note does **not** accept
ADR-0039, delete Buck2, clear HOLDs, or authorize generated-face removal. It
records the 2026-08-05 inventory that binds the next implementation sequence and
updates rejected alternatives with measured cache reality.

## Binding inventory (2026-08-05)

| Asset | State |
|-------|--------|
| Buck2 remote execution (RE) | **Absent** — no REAPI / Buildbarn / NativeLink / BuildBuddy |
| Buck2 content-addressed storage (CAS) | **Absent** — action cache is daemon-local; CI jobs cold |
| Cargo GHA portable cache | **Present** — `Swatinem/rust-cache` `shared-key: backend-cargo`; main `backend` sole writer; PG/domain facets restore-only |
| Cargo local cross-worktree cache | **Present, underused** — `sccache` + `CARGO_INCREMENTAL=0` via `scripts/console/lane-env.sh` (opt-in; default shells often show 0 hits) |
| Product PG path | **Hybrid** — S0–S2 cargo harness + facets (`tools/ci/cargo_needs_postgres.sh`, shard map); residual Buck lists (e.g. company-conformance) still present until cutover |
| nextest | Installed locally (`cargo-nextest`); **no** repo-pinned `.config/nextest.toml` yet |
| Equivalence superset gate | ADR step 1 **not** complete as a fail-closed CI assertion |
| Generated faces | Still Buck/preflight registry; **out of scope** for product-test cutover |

## Decision implications (subordinate to ADR-0039)

1. **Do not keep Buck2 as the product PostgreSQL/unit test driver “until RE lands.”**
   RE/CAS is not scheduled infrastructure. Without it, Buck’s primary advantages
   (cross-run action cache, RE fan-out, affected-target scheduling near CAS) are
   not inventory. Cold rebuild + multi-list registration remain the measured cost.

2. **Do use the cargo workspace as the product test graph**, with modern defaults:
   - **Discovery:** `cargo nextest` over the workspace (or explicit package sets
     derived from discovery — not hand-maintained wrapper lists).
   - **Resource isolation:** `#[sqlx::test]` per test (already dominant); nextest
     `test-groups` only for the small cluster-global serial set.
   - **CI parallel wall:** multi-job **partitions** (hash/count or stable
     package shards) + fail-closed aggregator — not one megajob, not
     `--num-threads=1` on every test.
   - **Hosted compile reuse:** keep shared `backend-cargo` rust-cache; restore-only
     on non-writer jobs; save only from main `backend`.
   - **Local compile reuse:** default `lane-env.sh` (or direnv) so sccache is on
     for worktrees; never put `rustc-wrapper = sccache` in repo config that CI
     would inherit without the binary.

3. **Do not delete Buck2 files or face jobs in the same step as product CI
   switch.** ADR-0039 sequence still applies: equivalence superset → dual green →
   switch product CI → remove Buck product jobs → later file delete. Faces stay
   until a separate face-writer path is proven.

4. **Rejected (updated):** “Keep Buck, enable remote cache” remains rejected for
   *near-term product tests* because REAPI + hermetic toolchain rewrite is a
   multi-quarter substrate project, while cargo rust-cache + sccache already
   deliver portable compile reuse for this workspace. Reopen only with a
   measured RE canary that beats rust-cache on wall **and** eliminates registration
   tax.

## Modern practice checklist (implementation must satisfy)

| Practice | Requirement |
|----------|-------------|
| Pin runners | Pin `cargo-nextest` version (and Node `24.16.0` where Node runs) in CI |
| Fail-closed discovery | No new product test requires a third hand list (workflow + preflight + BUCK) |
| Serial minority | Only documented cluster-global tests use `max-threads = 1` group |
| Partition, don’t thrash | Prefer nextest `--partition` or package shards sized by measured wall |
| Cache hygiene | One shared rust-cache key; single writer; no PR save; `CARGO_INCREMENTAL=0` in CI |
| Credential refusal | Keep `tools/lanes/pgtest.sh` / no-credential-in-argv (already re-homed) |
| Superset before switch | Cargo-executed set ⊇ Buck-executed product set before dropping Buck jobs |
| Faces carved out | Generated-face registry and macos full-face job are a separate track |

## Implementation sequence

| Step | Work | Exit criteria |
|------|------|----------------|
| **0** | Land PG S2 cargo facets | Required CI green; five facets + aggregator on main |
| **1** | Equivalence map + fail-closed assertion (ADR step 1) | Cargo-discovered product tests ⊇ Buck product wrappers still in CI |
| **2** | Adopt pinned nextest + `.config/nextest.toml` serial group (ADR step 2) | Dual path green on trial partition; only named serial tests single-threaded |
| **3** | Switch residual Buck product jobs to cargo/nextest + partitions (ADR step 3) | Buck product jobs unused; aggregator load-bearing name stable |
| **4** | Remove Buck product CI jobs; retain files (ADR step 4) | Reversible; faces/preflight Buck faces still present if needed |
| **5** | Local sccache default for lanes | `lane-env` documented + optional direnv; stats non-zero after one build |
| **6** | (Later, separate admission) Buck file delete + face path (ADR steps 5–6) | Only after dual green window + face alternative |

## Non-goals

- Accepting ADR-0039 by this note alone
- NativeLink/Buildbarn deployment
- Frontend / projection fan-out / production exposure HOLDs
- Weakening Required CI or demoting load-bearing PG aggregator display names

## Related evidence

- ADR-0039 proposal sequence and list-tax defect
- `ci.yml` comments: Buck cold `cached: 0` vs cargo + rust-cache
- `scripts/console/lane-env.sh` sccache contract
- S0–S2 cargo PG harness and shard map under `tools/ci/`
- Program plan 2026-08-05: cargo-first under RE/CAS absence
