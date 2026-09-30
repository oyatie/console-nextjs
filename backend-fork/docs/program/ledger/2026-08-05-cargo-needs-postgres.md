# Ledger — Cargo disposable-PostgreSQL CI harness (2026-08-05)

## Identity

- Slice: S-CIC / console-vam — Cargo `needs_postgres` + shared rust-cache for PR PG CI
- Base: `7965fbdf7b28e948e471c1ec046441473b1a32b8` (`origin/main` post-#573)
- Beads: `console-vam` (primary); Wave 0 prep packs `console-g1n` / `console-93w` / `console-7vm`
- Worktree branch: `ci/cargo-needs-postgres`

## Why this exists

Required PR CI wall clock is dominated by **`postgres-domain-reachability`** (~40–72 min).
The job was a cold Buck2 build of 183 serialized disposable-PostgreSQL targets with **no portable
compile cache** across runs. Domain-unit already proved cargo + `Swatinem/rust-cache` restores
`backend/target` across jobs; this slice applies the same driver to the PG lane.

## Behavior change (facts)

| Surface | Change |
|---------|--------|
| `.github/workflows/ci.yml` `postgres-domain-reachability` | Replaces `tools/buck/test_needs_postgres.sh` + 183 explicit `//tools/buck:*` wrappers with `tools/ci/cargo_needs_postgres.sh --workflow-only --num-threads=1` |
| rust-cache | Restore-only shared key `backend-cargo` (writer remains `backend` on main) |
| DotSlash on this job | Removed (cargo path does not need Buck2) |
| Display name | **Unchanged** — still `Dispatch, attendance and ontology — disposable PostgreSQL reachability` (branch-protection / aggregator coupling) |
| `tools/ci/postgres-cargo-map.json` | 183 workflow-mapped Buck wrappers → `cargo test` argv; 3 unmapped non-workflow fixtures documented |
| `scripts/check-executed-tests.mjs` | Expands cargo map so dark-set ratchet stays fail-closed after Buck wrappers leave the job body |
| `scripts/check-ci-preflight.mjs` | Digests, action contracts, map coverage, ontology reachability via map |
| Wave 0 prep packs | `docs/program/lane-prep/w0-{ont,pol,apr}/` admission + OWNERSHIP + trial (prep only; no domain product code) |

## Non-goals

- No NativeLink / Buck CAS
- No path filters (console-8x4 / CI-2 still open)
- No company-conformance or backend Buck PG suite swap
- No product domain mutations; no HOLD clears
- No branch-protection API edits

## Verification (orchestrator)

- `npm run check:ci-preflight` — pass
- `node --test scripts/check-ci-preflight.test.mjs` — 53/53
- `node scripts/check-executed-tests.mjs` — dark set still 10 (baseline hold)
- `node tools/ci/check-postgres-cargo-map.mjs` — cargo harness; 183 workflow entries
- `node scripts/check-test-credentials.mjs` — pass
- Local trial: `tools/ci/cargo_needs_postgres.sh --only dispatch-p1-postgres --num-threads 1` (prior session: 13/13 on trial map)

## Rollback

Revert this PR: job returns to Buck serialized wrappers; map and harness remain inert until re-wired.
Display name and Required / CI membership unchanged.

## Wave 0 unlock

Prep packs admit ONT/POL/APR domain-increment lanes under allowlists once this CI wall-clock
reduction lands (or in parallel after admission). First domain code still requires
`domain-increment` workflow + dual review; packs alone do not implement product behavior.

<!-- REASONING-LENS-EVIDENCE:START -->
```json
{
  "lens_contract": "v1",
  "lens_contract_digest": "ac1e7d6b8150808ef73e5e3cd1a1e54d2f37eb43e84aaa1370dbbaaff3c44373",
  "task_class": "implementation",
  "risk_class": "high",
  "risk_domains": [
    "release",
    "other"
  ],
  "selected_lenses": [
    "Cartesian doubt",
    "Essentialism / YAGNI",
    "Chesterton's Fence",
    "Pragmatism",
    "Red Team",
    "Systems Thinking",
    "Operability / Day-2",
    "Blast-radius / cell-based",
    "Telemetry-first",
    "Zero-trust / defense-in-depth"
  ],
  "task_fit": {
    "Cartesian doubt": "Separated cold Buck cache absence (measured) from test correctness; map+executed-tests keep coverage fail-closed.",
    "Essentialism / YAGNI": "Swap one critical-path job driver; defer path filters, sharding, NativeLink, company-conformance cargo.",
    "Chesterton's Fence": "Kept display name, timeout, Required/CI membership, and shared rust-cache writer=backend save-if rules.",
    "Pragmatism": "Cargo+rust-cache is the only portable warm path available before NativeLink CAS.",
    "Red Team": "Fail-closed map coverage and executed-tests expansion prevent silent deletion of PG reachability targets under a shorter wall clock.",
    "Systems Thinking": "executed-tests, preflight digests, ontology wrapper reachability, and credentials scanner updated together.",
    "Operability / Day-2": "Map checker + harness script are the ops surface; rollback is revert PR; display name preserved for protection coupling.",
    "Blast-radius / cell-based": "CI harness + prep docs only; no domain crates, migrations, or HOLD control changes.",
    "Telemetry-first": "Harness logs per-entry pass/fail; map counts.workflow_mapped frozen at 183.",
    "Zero-trust / defense-in-depth": "Preflight digests + map coverage prevent silent target list deletion; restore-only cache avoids poisoning."
  },
  "mandatory_lens_exceptions": {},
  "findings": [
    "183 workflow Buck PG wrappers fully mapped to cargo argv; 3 unmapped are non-workflow fixtures.",
    "Preflight previously forbade rust-cache on postgres-domain-reachability because the job was Buck-only; that ban is lifted with save-if:false.",
    "Wave 0 prep packs land in the same candidate to unlock parallel domain-increment admission without waiting for a second docs PR."
  ],
  "decisions_changed_or_rejected": [
    "Rejected keeping Buck wrappers listed in ci.yml once cargo harness is authoritative.",
    "Rejected save-if:true on the postgres job (cache poisoning under shared-key).",
    "Deferred CI-2 path filters and company-conformance cargo swap."
  ],
  "lens_set_changes": []
}
```
<!-- REASONING-LENS-EVIDENCE:END -->

## Authority tip

T is the signed authority tip for this candidate train. C prebinds this ledger blob.
