> **HISTORICAL after the 2026-07-28 pivot.** This document preserves evidence and prior reasoning only. It cannot authorize Buck2, broad product scope, fan-out, or implementation work. Current product authority is [`docs/current/PRODUCT.md`](../current/PRODUCT.md), [`ROADMAP.md`](../current/ROADMAP.md), and [`DELIVERY.md`](../current/DELIVERY.md). Pivot history only: [`docs/PIVOT-2026-07-28.md`](../PIVOT-2026-07-28.md) (non-authority).

# Console Buck2 hyperscale graph-and-data logistics playbook

**Status:** implementation policy for the console train. The optimization target
is a millions-of-lines monorepo with very large target graphs, multi-cell
ownership, generated faces, and bounded parallel delivery. **Graph and data
movement are optimized first; developer-facing CI latency is a consequence, not
the sole metric.** Rust completion evidence is Buck2-only. Cargo remains
manifest/dependency metadata and is never a substitute for product verification.

## Immutable toolchain and graph snapshots

- `./tools/buck2` is the official DotSlash manifest pinned to released
  [Buck2 2026-07-15](https://github.com/facebook/buck2/releases/tag/2026-07-15).
  Every platform entry uses its exact BLAKE3 digest. CI installs an exact
  SHA-256-verified DotSlash 0.5.7 runtime before invoking the manifest; scripts
  never select a developer-global Buck binary.
- The **released pin** is production authority. New upstream capabilities remain
  a separately recorded **canary** until exact-digest toolchain, target-graph,
  action-result, and artifact-provenance parity are proven. Do not point CI at
  upstream `main` or silently call a canary “latest”.
- Candidate selection starts from immutable base and diff snapshots: `(base SHA,
  candidate SHA, cell map, package ownership, target graph digest, generated-face
  registry digest)`. Merge-train rebases/reorders invalidate only the affected
  graph closure; they never reuse a result whose base snapshot differs.
- Build reports retain the input snapshot, action keys, execution platform,
  outputs, cache hits/misses, and provenance. This permits precise invalidation
  rather than runner-time heuristics.

## Cells, ownership, and generated faces

Cells are trust, toolchain, and configuration boundaries—not ordinary module
directories. The root, toolchain, bundled-prelude, and empty alias cells are
intentional. **No per-module cells:** module boundaries are packages and targets
inside the root cell. Add a cell only for a proven trust, configuration, or
execution-platform boundary; audit it with `buck2 audit cell`.

Every generated Rust test receives exactly one `test.unit` or
`test.integration` label and exactly one `resource.none` or `resource.postgres`
label. Postgres tests temporarily retain `needs-postgres` for runner
compatibility. `owner.*` and `domain.*` labels derive from stable package paths;
there is no central exceptions table to decay as the graph grows.

`tools/buck/generated_face_registry.json` is authority metadata, not a second
dependency graph. It is a structured allowlist, not shell text: each face has
one executable artifact plus a resolvable Buck writer target, existing declared
source roots, exact generated output paths or constrained `/**`/`/**/BUCK`
patterns, and a `writer-snapshot` drift gate. The validator rejects missing
artifacts, unresolved targets, raw commands, and overlapping writable faces.
The no-write cheap admission command, `tools/buck/preflight.sh`, snapshots the
candidate and executes only registered `cheap` faces through the allowlisted
writer dispatcher. It prints a `DEFERRED` receipt for every registered
`expensive` face, so a face is never silently omitted. The separately callable
full closure, `tools/buck/preflight.sh --full-generated-faces`, executes
**every** registered face and is the generated-face status required by policy.
It is not merge-enforced until the live GitHub ruleset admits that exact status;
until then the job proves closure but not admission. `cheap` and `expensive` are
scheduling metadata, never permission to skip authority checks. Archive snapshots
exclude mutable `node_modules`; before any OpenAPI gate runs, the preflight
requires byte-identical `package.json` and
`package-lock.json`, validates every lockfile package/link against the caller's
installed tree, then creates a snapshot-local symlink to that verified tree.
Missing or inconsistent provenance fails closed—there is no ancestor-directory
Node resolution and no copy of dependencies or generated output into Git.

This applies to:

1. generated first-party BUCK files;
2. the Reindeer third-party Rust graph; and
3. OpenAPI TypeScript, Kotlin, and Swift outputs.

A generated face changes only through its registered writer; the registry is
validated in the cheap no-write preflight. Consolidation performs one shared
regeneration after source lanes land, preventing generated-face merge storms.

## Data locality, remote execution, and cache policy

Execution platforms describe hermetic macOS/Linux tools and exact-SHA images.
Remote execution and CAS are adopted only behind a measured canary with
compatible RE API behavior, authorization, artifact provenance, deterministic
outputs, and rollback proof.

- Keep CAS keys content-addressed and platform/configuration-aware. Upload once,
  schedule near existing inputs, and avoid copying generated clients, source
  trees, or database fixtures between shards when an action digest already
  identifies them.
- Prefer remote cache hits that match the immutable base/diff snapshot. Treat
  cross-branch reuse as an optimization only after action-key equivalence proves
  it safe.
- Separate local developer isolation from shared remote cache namespaces; failed
  or untrusted actions never publish reusable outputs.

## Merge-train and bounded fan-out

Cheap gates run first: pinned-manifest integrity, generated-face registry,
`audit cell`, target enumeration, lock/generator drift, and diff-to-target
planning. A merge-train planner then computes the minimal safe closure from the
base/diff snapshots and labels. Candidate CI and release authorization remain
separate: an immutable image may publish only after the candidate’s required
closure and image provenance gates succeed.

Fan-out is bounded by graph structure and resources, not by arbitrary job
counts:

- shard pure `test.unit` targets by stable target digest and estimated action
  cost;
- schedule `test.integration` only with isolated external resources;
- serialize `resource.postgres` per disposable database lease while parallelizing
  independent leases and mobile/browser device pools;
- group actions to retain compiler/CAS locality, then cap each pool by remote
  queue pressure, cache-miss amplification, and artifact bandwidth;
- invalidate only affected shards on merge-train changes; retain immutable
  outputs for unaffected closures.

## Rollout roadmap

1. **H0 (now):** exact test/resource/ownership taxonomy; generated-face authority
   registry; no-write preflight; base/diff snapshot contract.
2. **H1:** BXL plus Buck2 change detector compute affected targets, owners, and
   stable shard manifests from immutable snapshots.
3. **H2:** hermetic local execution platforms, build reports, action/cache
   telemetry, and cost-aware bounded shard planning.
4. **H3:** remote cache canary, then remote execution canary with CAS locality,
   auth/provenance, and rollback gates.
5. **H4:** merge-train graph invalidation and image-release triggering from
   successful candidate CI—not polling—while full release matrices remain a
   mandatory backstop.

Primary references: [cells and key concepts](https://buck2.build/docs/concepts/key_concepts/),
[configurations](https://buck2.build/docs/concepts/configurations/),
[modifiers](https://buck2.build/docs/concepts/modifiers/),
[BXL](https://buck2.build/docs/users/commands/bxl/),
[remote execution](https://buck2.build/docs/users/remote_execution/),
[test labels](https://buck2.build/docs/users/commands/test/), and
[build reports](https://buck2.build/docs/users/build_observability/build_report/).
