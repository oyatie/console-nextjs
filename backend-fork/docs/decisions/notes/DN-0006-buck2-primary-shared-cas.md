---
id: DN-0006
kind: design-note
parent_adr: ADR-0039
authority: subordinate
activation: planning
date: 2026-08-11
owner: jasonlee
supersedes_planning: DN-0005
---

# DN-0006 — Buck2 primary under shared NativeLink CAS (reopens DN-0005)

## Status

**Planning record** under proposed ADR-0039. Does **not** by itself accept ADR-0039,
delete Cargo.toml/Cargo.lock, or flip merge-required warm cache. It records the
2026-08-11 founder decision and the substrate that invalidates DN-0005’s
cargo-primary path.

## Founder decision (2026-08-11)

> Switch cargo **everywhere** to Buck2 — build/test/CI/docs/scripts/workflows.
> Shared laptop NativeLink CAS (`~/oyatie-cas`, `instance_name=main`) is the
> remote cache substrate. Prefer Buck2 remote/CAS integration; do not keep cargo
> as the primary driver.

## Inventory that changed vs DN-0005

| Asset (DN-0005 claim) | 2026-08-11 state |
|-----------------------|------------------|
| Buck2 CAS / REAPI | **Present (cache-only)** — NativeLink on founder Mac; local mTLS `:50051`/`:50052`; Access TCP canary **GREEN_REAPI** |
| Cross-run Buck cache | **Unblocked in principle** — was the measured reason CI chose cargo (daemon-local AC; `cached: 0` across runners) |
| Cargo rust-cache | Still present; **demote** as Buck2+CAS lands on merge-required jobs |
| Product PG map | `tools/ci/postgres-cargo-map.json` already carries `buck_inner` / `buck_wrapper` for 215 entries — reverse harness is mapped |
| Gate binaries | Buck targets already exist under `backend/ci/gates/*/BUCK` while CI still `cargo run -p console-gate-*` |

## Decision implications (replace DN-0005 §Decision implications)

1. **Buck2 is the primary product build/test driver.** Cargo remains a
   **generator input** (reindeer / Cargo.lock / `cargo metadata` lock proof) until
   a later admission removes those needs — not the CI execution engine.
2. **Reopen** DN-0005 rejected alternative “Keep Buck, enable remote cache” —
   substrate now exists; hermeticity (`system_cxx_toolchain`) still limits
   cross-machine hit rate (opportunistic hits ≠ fleet license).
3. **Fail-closed warm reads** until console cites GREEN_REAPI + reviewed license
   (mirror oyatie `warm_reads_licensed: false`).
4. **Do not expose** writer CAS secrets to fork `pull_request` workflows.
5. **ADR-0039** remains proposed; this note reverses only the *cargo-as-driver*
   planning path, not face/delete authority.

## Migration waves (concrete PRs)

| Wave | Scope | Exit |
|------|--------|------|
| **A** (this lane) | Opt-in `infra/ci/buckconfig/*` + `scripts/cas/*` + this DN + handoff inventory | Overlays never in root `.buckconfig`; cold Buck2 still green |
| **B** | Founder: `~/oyatie-cas/gha/bootstrap-cas-secrets.sh console`; trusted Access TCP sidecar composite | `gh secret list` shows CF_ACCESS_* + OYA_CAS_TLS_* on console; fork jobs have none |
| **C** | Flip `backend` job gates from `cargo run -p console-gate-*` → `tools/buck2 run //backend/ci/gates/...` | Preflight contracts updated; mutation suites already Buck |
| **D** | Flip `domain-unit` cargo tests → Buck unit targets + CAS overlay on trusted pushes | Preflight “must pass --lib on cargo” retired; wall ≤ cargo+rust-cache or accepted |
| **E** | Flip `cargo_needs_postgres.sh` → Buck `buck_inner` driver (map already dual-keyed) | Five PG facets + aggregator green; rename map away from cargo when idle |
| **F** | Retire rust-cache writer / cargo fmt+clippy steps or replace with Buck equivalents | Only lock/metadata cargo left (or reindeer-only) |
| **G** | Docs/scripts/hooks (`cargo-scope-enforcer`, lane pgtest, bacon) → Buck primary | No developer docs prescribe cargo test as default |

## Non-goals

- Silent warm-cache on Required CI before canary citation
- Deleting reindeer / third-party Cargo graph in the same wave as CI flip
- Expecting Cargo itself to speak REAPI

## Related

- Handoff: `docs/handoffs/2026-08-11-oyatie-shared-cas.md`
- Lab: `~/oyatie-cas/README.md`, `canary/reapi-access-canary.sh`
- Supersedes planning authority of DN-0005 cargo-primary sequence (steps 1–4 toward nextest-as-driver)
