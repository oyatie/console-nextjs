---
id: DN-0007
kind: design-note
parent_adr: ADR-0039
authority: subordinate
activation: planning
date: 2026-08-18
owner: jasonlee
supersedes_planning: DN-0006 (substrate-readiness claims only)
---

# DN-0007 — Buck2 warm cache, measured (corrects DN-0006's substrate claim)

## Status

**Planning record** under proposed ADR-0039. Corrects one claim in DN-0006. Does
not accept ADR-0039, grant delete authority, or flip merge-required warm cache.

## What DN-0006 got wrong

DN-0006 and the 2026-08-11 handoff recorded the CAS substrate as **present
(cache-only)** on the strength of a lab canary verdict of `GREEN_REAPI`. That
canary proved the **server** was reachable over mTLS/Access TCP. It did not
prove Buck2 would ever **use** it — and Buck2 would not have.

Measured 2026-08-18 against a live NativeLink cache-only server:

1. **The Wave A overlays were inert.** Root `.buckconfig` sets
   `execution_platforms = prelude//platforms:default`, whose executor is
   `Local(LocalExecutorOptions)` — no remote executor exists. Every
   `[buck2_re_client]` key was parsed and then unusable. A gate build reported
   `Commands: 97 (cached: 0, remote: 0, local: 97)`, `up 0B`, and the server
   logged zero client connections.
2. **`--config-file` cannot deliver `[buck2_re_client]`.** It is daemon-startup
   config; `[build]` is per-command. The handoff's own example invocation fails
   with `Error: (No engine address)`. RE addresses must be written to
   `.buckconfig` / `.buckconfig.local`.

## What is now proven

With a remote-cache-capable execution platform (`//platforms:remote-cache`) and
the RE client in a mode-0600 `.buckconfig.local`, against
`//backend/ci/gates/layer-boundary:console-gate-layer-boundary`:

| Condition | Result |
|---|---|
| Cold, empty store | 97 local actions, ~103 MiB uploaded, 3703 `batch_update_blobs` |
| Warm, `buck-out` wiped + daemon killed | **`Cache hits: 100%`, `Commands: 97 (cached: 97, local: 0)`, 0.6 s** |
| Different checkout path, same isolation dir | **100 %** — digests are path-independent |
| Differing isolation dir | **0 %** — the isolation dir is part of output paths |
| Platform target renamed | dropped to 11 %, restored to 100 % after one refill |

Cache-only is sufficient: **no scheduler, no RE workers, no autoscaler.** Uploads
happen from locally-executed actions (`allows_cache_upload=True` on 97 real rule
actions).

## Constraints this establishes

1. **Buck2 does not degrade when the CAS is down — it fails.** It queries RE
   capabilities before any action and treats failure as fatal
   (`Internal error (stage: remote_action_cache) … Connection refused`).
   Every warm-capable invocation must be guarded by a reachability probe;
   `--no-remote-cache` is the verified fallback. See `scripts/cas/cas-preflight.sh`.
   This must land before any warm flip on merge-required jobs.
2. **The cache key namespace includes the execution platform's target label and
   the buck2 isolation dir.** Renaming or moving either invalidates the cache for
   every consumer. Both must be chosen once and held stable across machines.
3. **Path independence holds**, so runner-to-runner warm cache across ephemeral
   checkouts is viable — the exact `cached: 0` failure DN-0005 measured.
   Cross-OS sharing is still expected to miss (platform constraints plus
   non-hermetic `system_cxx_toolchain`) and is **not yet measured**.

## Upstream facts corrected

- NativeLink **v1.6.5** publishes a multiarch OCI index (`linux/amd64` +
  `linux/arm64`) plus `aarch64-unknown-linux-musl`, `x86_64-unknown-linux-musl`
  and `aarch64-apple-darwin` tarballs, each cosign-signed with SPDX SBOM and
  SLSA provenance (verified `Verified OK` against the TraceMachina GitHub
  Actions OIDC identity). Architecture does **not** constrain host placement.
- The upstream docs pin **v1.3.2**, which is genuinely single-arch amd64. Do not
  infer arch support from the documented tag.
- The darwin tarball links against a Nix store path
  (`/nix/store/…-libiconv-113/lib/libiconv.2.dylib`) and will not start on a
  stock Mac. Workaround: `install_name_tool -change <nixpath>
  /usr/lib/libiconv.2.dylib` then `codesign --force -s -`.

## Wave A wiring (replaces the handoff's Wave A section)

- `//platforms:remote-cache` — execution platform with the remote cache enabled
  and execution left local.
- `infra/ci/buckconfig/warm-cache.buckconfig` — carries only
  `[build] execution_platforms`; replaces the four address-bearing overlays,
  whose addresses could never work from `--config-file`.
- `scripts/cas/materialize-buckconfig-local.sh` — writes the full
  `[buck2_re_client]` block (`--profile lab|gha`, `--role writer|reader`,
  `--endpoint`, `--no-tls`).
- `scripts/cas/cas-preflight.sh` — reachability probe emitting `BUCK2_CAS_FLAGS`.

## Unchanged by this note

Fail-closed warm reads, no fork-PR exposure of CAS credentials, reader/writer
split, and the requirement that ADR-0039 acceptance remain a separate act.

## Open

- CAS **placement/region** is undecided and now the only thing gating
  provisioning: cold cost is round-trip dominated (3703 blob RPCs for one small
  target), so the store wants to be near whichever consumer matters most.
- Cross-OS (macOS ↔ Linux) cache sharing unmeasured.

## Related

- Corrects: `DN-0006-buck2-primary-shared-cas.md`, `docs/handoffs/2026-08-11-oyatie-shared-cas.md`
