---
id: DN-0008
kind: design-note
parent_adr: ADR-0039
authority: subordinate
activation: planning
date: 2026-08-19
owner: jasonlee
supersedes_planning: DN-0006 (Buck2-as-CI-driver sequence)
---

# DN-0008 — Cargo stays the CI driver; Buck2 becomes a nightly hermeticity check

## Status

**Planning record** under proposed ADR-0039. Reverses DN-0006's *Buck2-everywhere*
sequence. Does not accept ADR-0039, delete Buck targets, or revert the Buck2 jobs
already in Required CI.

## Why DN-0006's sequence is withdrawn

DN-0006 reversed DN-0005's cargo-primary path on one stated ground: the shared
NativeLink CAS was **present** and therefore Buck2's cross-run cache — the thing
Cargo could not offer — was finally available.

Measured 2026-08-18 (DN-0007): the client side never reached that CAS. Root
`.buckconfig` selected `prelude//platforms:default`, whose executor is Local with
no remote executor, so every `[buck2_re_client]` key was inert. A gate build ran
`Commands: 97 (cached: 0, remote: 0, local: 97)`, uploaded `0B`, and the server
logged zero client connections. The `GREEN_REAPI` canary proved the **server** was
reachable, never that Buck2 would use it.

So DN-0006's premise was false when written. DN-0005's reasoning — Cargo works,
the Buck cache does not — was correct at the time, and the substrate that was
supposed to invalidate it did not exist.

## What the migration would actually cost

| Measure | Value |
|---|---|
| `cargo` invocations in Required CI | **36** |
| `tools/buck2` invocations in Required CI | **5** |
| Migration completed after Waves A + C | **5 / 41** |

Finishing Waves D–G means roughly thirty-six further Required-CI changes, each
requiring a sole-writer lease, to replace a path that works today and is already
warm via `v0-rust-` rust-cache entries.

## The comparison that would justify migrating was never run

DN-0007 measured Buck2 **cold vs warm** (0% → 100% hits, ~3 min → 0.6 s). It did
**not** measure Buck2+CAS against Cargo+rust-cache on the same CI work. Migrating
on the former would repeat DN-0006's error: acting on evidence that proves
something adjacent to the claim.

## Failure modes Buck2 adds on the critical path (all measured)

1. Buck2 **hard-fails** when a configured CAS is unreachable — it queries RE
   capabilities before any action and treats refusal as fatal. Every warm-capable
   invocation needs a probe and `--no-remote-cache` fallback.
2. The action digest folds in the **buck2 isolation dir** and the **execution
   platform's target label**. Renaming either invalidates the cache fleet-wide
   (measured: 100% → 0%, and 100% → 11%).
3. **macOS and Linux can never share entries** — the OS changes the configuration
   hash (`#9cddb596` vs `#e39f0472`, same target, same architecture).

## Decision

1. **Cargo remains the CI driver.** rust-cache remains the CI warm path.
2. **The five existing Buck2 Required jobs stay.** They are green and they are the
   hermetic ones; reverting carries its own risk. Stop adding.
3. **Buck2 runs on a daily schedule as a hermeticity check**
   (`.github/workflows/buck2-hermeticity.yml`, 06:53 UTC). It is a separate
   workflow rather than a job in `nightly.yml`, because that workflow's contract
   test asserts the locked `dev-up-smoke` step list covers every step the file
   declares — adding a job there would weaken a guard this change does not own.
   Buck2's differentiator here is that it forces declared
   inputs where Cargo tolerates undeclared ones. That value is about catching
   drift, not speed, and does not belong on every PR.
4. **The warm cache keeps two honest roles**: a developer cache on the workstation
   (3 min → 0.7 s, measured) and an accelerator for the nightly job. It is not a
   CI-critical dependency and touches no Required context.

## Consequences

+ Required CI keeps a working, understood driver; no further sole-writer leases
  are spent on build-system replacement.
+ The Buck2 graph stays exercised daily, so it cannot rot unnoticed.
− Two build systems remain. The nightly job is the mechanism that keeps that cost
  visible instead of discovered during an incident.

## Reopening condition

Measure Buck2+CAS against Cargo+rust-cache on the same Required job and publish
both wall clocks. If Buck2 wins materially and repeatably, DN-0006's sequence may
be reopened on that evidence — not on substrate availability alone.

## Related

- Supersedes the CI-driver sequence in `DN-0006-buck2-primary-shared-cas.md`
- Measurements: `DN-0007-buck2-warm-cache-measured.md`
- Restores the operative conclusion of `DN-0005-adr-0039-re-cas-absence-cargo-cache-path.md`
