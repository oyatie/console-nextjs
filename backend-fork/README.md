# Console

Console is a Rust platform for a governed company object engine: **Ontology / Foundry / Policy → Company / OrgUnit / Employee → HR → Payroll**. This is the repository's sole onboarding entry point.

## Current authority

1. [`docs/current/PRODUCT.md`](docs/current/PRODUCT.md) — current product scope, architecture, invariants, and holds.
2. [`docs/current/ROADMAP.md`](docs/current/ROADMAP.md) — ordered current work and explicit HOLDs.
3. [`docs/current/DELIVERY.md`](docs/current/DELIVERY.md) — review, CI, merge, verification, and issue-lifecycle policy.

## Records and history

Accepted records in [`docs/decisions/`](docs/decisions/) preserve decision history. Machine-readable registers and path-stable ledgers remain under [`docs/program/`](docs/program/). Evidence and historical documents may support a claim, but they do not override the three active authorities recorded separately from this sole entry point in [`docs/documentation-index.json`](docs/documentation-index.json).

The full first-party Markdown manifest is the generated [`docs/documentation-index.json`](docs/documentation-index.json); its reviewed semantic source is [`docs/documentation-manifest.seed.json`](docs/documentation-manifest.seed.json).

## Repository map

- `backend/` — Rust workspace, application, domain/platform crates, migrations, and backend gates.
- `docs/current/` — the three current product/delivery authorities.
- `docs/decisions/` — governed ADR and design-note history.
- `docs/program/` — machine-readable registers, authority ledgers, and historical program material.
- `docs/evidence/` — immutable evidence excluded from normal onboarding.
- `scripts/` — executable CI, preflight, and verification gates.

## Supported verification entrypoint

A new clone needs Node tooling **and** the pinned DotSlash runtime on `PATH` before `npm run verify`. The installer cannot modify its parent shell (`GITHUB_PATH` exists only in GitHub Actions).

```sh
npm ci
tools/buck/install_dotslash.sh
export PATH="${CONSOLE_DOTSLASH_BIN_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}/console-dotslash}/bin}:$PATH"
npm run verify
```

`npm ci` installs the pinned repository tooling. `npm run verify` is the supported local repository verification entrypoint (`fast` tier). Buck2-backed steps fail as environment errors, not product regressions, if DotSlash is missing from `PATH`. The same sequence is recorded in [`docs/current/DELIVERY.md`](docs/current/DELIVERY.md). Run narrower tests while developing and any additional checks required by the touched surface, then record the exact candidate SHA, commands, environment, discovered/executed counts, failures, and remaining HOLDs there.

<!-- SHARED:REASONING-LENSES:START -->
## Reasoning lens manifest

Canonical definitions and routing rules live in [AGENTS.md](AGENTS.md#task-selected-reasoning-lenses). This identifier-only projection is drift-checked and does not duplicate policy.

1. Cartesian doubt
2. Essentialism / YAGNI
3. Chesterton's Fence
4. Contrarian / outside-the-box
5. Socratic
6. Pragmatism
7. Red Team
8. Systems Thinking
9. Operability / Day-2
10. Opportunity Cost
11. Blast-radius / cell-based
12. Constant-work / anti-fragility
13. Shared-nothing / eventual consistency
14. FinOps / unit-cost
15. Telemetry-first
16. Zero-trust / defense-in-depth
<!-- SHARED:REASONING-LENSES:END -->
