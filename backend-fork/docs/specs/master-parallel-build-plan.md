> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Master Parallel Build Plan — maintenance → enterprise multi-tenant SaaS (oyatie-borrowed)

> Output of the oyatie-borrow-scan workflow + planning-and-task-breakdown. Companion to
> `roadmap-to-production.md` (the WHAT) and `build-strategy.md` (the WHY). This is the HOW: the
> parallelization architecture, the borrow manifest, the substrate-first sequence, the 20 immediate
> disjoint lanes, and a future MES lane the fleet must not fan out prematurely. **Grounding:** maintenance already has 14 bounded-context crates in the
> `{domain,application,adapter-postgres,rest}` shape, glob workspace members (kills the #1 merge
> conflict), 6 CI gates, `deny.toml`, `crates/kernel/core`, openapi-first, ultragoal — so the substrate
> is ~60% in place; S0–S5 closes the gap, then the lanes fan out.

## 1. Parallelization architecture (four invariants; 3 already in place)

- **(a) Kernel = the agent-sized unit.** Each domain (WO-FSM, payroll brackets, leave-accrual, invoice
  totals) gets a **pure no-I/O core** (no sqlx/axum/tokio/`Utc::now` — time+ids passed in), in
  `crates/<ctx>/domain` or `crates/kernel/core` if cross-context. One agent owns one kernel, unit-tests it
  with zero DB. *We already do this.*
- **(b) Layer direction = collision firewall.** `domain ← application ← adapter-postgres ← rest`, inward
  only, enforced by the `layer-boundary` gate. **Add one edge-ban:** a domain crate may never depend on
  another context's domain crate — cross-context talk goes through contracts only.
- **(c) Contracts-first seam = how disjoint lanes integrate.** Lanes never import each other's crates; they
  share a **per-context OpenAPI fragment** (sync seam) + a **tiny event-contract YAML** (async, e.g.
  attendance→payroll). The integration handshake is the contract row, not the code.
- **(d) Agent-operating-contract = merge-clean protocol.** One lane = one `git worktree` on `agent/<lane>` =
  one PR over one crate family. Serialize points = root `Cargo.toml`/`Cargo.lock` + `openapi.yaml` +
  generated clients (claim across these one-lane-at-a-time; glob members already eliminates the members
  conflict). "Done" = pasted gate output + RLS-as-`console_rt` test; never self-approval.

Concrete integration: payroll (L14) needs attendance owned by HR (L13). It does NOT depend on `console-hr-domain`;
HR publishes `openapi/hr/attendance-v1.yaml` + `contracts/events/attendance-recorded-v1.yaml`; payroll
consumes the generated type. A **route-parity gate** + a **bindings.tsv** keep handler↔contract↔wire-type
1:1; a **cohesion gate** ensures exactly one owner per contract.

## 2. Borrow manifest (ranked: lift-wholesale / adapt / reference / skip)

| # | Artifact (oyatie) | Verdict | Accelerates | Adapt notes |
|---|---|---|---|---|
| 1 | Slice-plan template (`tasks/*-plan.md`) | **LIFT** | every lane (per-agent brief) | add maintenance constraints: arm `app.current_org`; adapter tests as real `console_rt`; new table→RLS policy+gate; new endpoint→openapi fragment+regen |
| 2 | billing-kernel `aggregate_invoice` (int-micros, round-half-up, overflow-guarded) | **LIFT** | ERP invoice + 거래명세표 + 부가세 10% | into `kernel/core`; won as minor unit; `tax_basis_points=1000` |
| 3 | OpenAPI↔REST route-parity gate | **LIFT** | stops openapi drift across lanes; fixes red #47 | new `ci/gates/openapi-route-parity` (scan `*_PATH` consts vs openapi `paths:`) |
| 4 | workload-identity `evaluate_decision` (forbid-wins precedence + audit `DecisionReason`) | **ADAPT** | configurable RBAC (#46) | fold into authz; keep 42×6 matrix as default; reason→audit |
| 5 | RLS overlay (default-row `org_id IS NULL` + per-tenant + `app.role` bypass) | **LIFT** | ontology defaults + custom roles | overlay `USING` only on ontology/role tables; business tables stay strict-equality; arm `app.role` |
| 6 | Cedar PDP adapter (signed bundles, default-deny, version cache) | **ADAPT** | configurable RBAC at scale | `console-authz-cedar`; default bundle from 42×6; Cedar decides capability, RLS decides rows; single in-process PDP (skip the distribution fabric until a real sub-60s-revocation SLO exists) |
| 7 | Binding manifests (operationId→crate→handler→type→test) | **ADAPT** | reviewer verifies a slice is fully wired | `openapi/bindings.tsv` + gate asserting each operationId has a row + symbol/test on disk |
| 8 | Per-domain OpenAPI split + `.meta.yaml` sidecar | **ADAPT** | modularizes the openapi monolith | `openapi/<domain>/<name>-v1.yaml` + owner/version; `$ref`-bundle build for client gen |
| 9 | tenant-quota kernel (check/consume/release) | **ADAPT** | SaaS plan tiering | durable storage+seats axes; reuse mail rate-limiter (#30) as rate axis |
| 10 | signed audit digest-chain | **ADAPT** | tamper-EVIDENT audit | `console-audit-chain`: batch-seal a hash-link over new `audit_events`, sign with OCI Vault |
| 11 | i18n completeness check + ko.ts split | **ADAPT** | UI lanes don't collide on ko.ts | `web/src/i18n/<domain>/ko.ts` merged at an index; vitest key-parity |
| 12 | cohesion / one-owner gate | **ADAPT** | merge-clean (one owner per contract) | gate over the bounded-context registry |
| 13 | cursor-pagination kernel | **ADAPT** | every list endpoint | into `kernel/core` |
| 14 | codegen-determinism harness | **ADAPT** | api-drift stays green | gate: regen == committed (fixes the #47 api-clients lane root cause) |
| 18 | bounded-context registry + gate | **ADAPT** | the lane-assignment SSOT | `backend/registry/bounded-contexts.json` |

## 3. Substrate-first sequence (S0–S5 — single-lane, before the fleet)

These touch shared artifacts, so they run **sequentially first** (~one short wave), then the 20 immediate
lanes fan out. The future MES lane stays gated until its prerequisites are accepted.

- **S0 — Agent-operating-contract + slice-plan template + PR template.** The rulebook every lane obeys
  (pre-flight Why+Test, done-definition, reviewer-routing, sanctioned-primitives, **mandatory: arm
  `app.current_org` + test as `console_rt`**). `.github/pull_request_template.md` ("no Code Review section = no
  merge"). Folds into the ultragoal brief/loop.
- **S1 — OpenAPI modularization + bundle + route-parity + bindings + codegen-determinism gates.** Splits
  the serialize point so every REST lane owns its fragment; fixes the red #47 api lane.
- **S2 — ko.ts modularization + completeness vitest.** Splits the web serialize point.
- **S3 — Authz upgrade:** `evaluate_decision` fold + RLS overlay + `app.role` GUC; keep 42×6 as default.
  Gates configurable RBAC + ontology.
- **S4 — Shared kernels into `kernel/core`:** cursor-pagination + `aggregate_invoice`. Financial/HR/ERP/list
  lanes all consume them.
- **S5 — Bounded-context registry + cohesion gate:** `backend/registry/bounded-contexts.json` (the 14
  existing + planned hr/payroll/erp/govadapter/ontology, each with `crate_family`+`openapi_contract`+
  `owner_lane`). The lane-assignment SSOT; route-parity+bindings validate reality against it.

## 4. Disjoint lane map (concurrent worktrees; shared deps = contracts only)

Existing contexts → **maturity** lanes (mature + close coverage/#19/#20); new → **greenfield** lanes.

| Lane | Owns | Consumes (contracts) | Notes |
|---|---|---|---|
| L1 identity | `identity/*` | — (owns OrgId/principal) | region/branch deactivation in flight |
| L2 workorder | `workorder/*` | identity, registry | WO-FSM kernel |
| L3 registry | `registry/*` | identity | 장비; ontology-adjacent |
| L4 dispatch 🆕crate | `dispatch/*` | workorder, identity | dispatch map #29 |
| L5 sales | `sales/*` | registry, financial | storefront + inquiries (#41) |
| L6 comms | `comms/*` | identity | mail rate→quota axis |
| L7 messenger | `messenger/*` | identity | thread-lifecycle FSM + reaction-tally |
| L8 inspection | `inspection/*` | registry, workorder | PM schedule |
| L9 support | `support/*` | identity | ticket delete/unify (#42) |
| L10 financial | `financial/*` | **S4 aggregate_invoice** | invoice/명세표 (#19.18) |
| L11 compliance | `compliance/*` | identity, audit-chain | integrity |
| L12 reporting | `reporting/*` | all (read-only) | SLA/fleet rollup kernel; analytics |
| **L13 HR** 🆕 | `hr/*` | identity | attendance + leave-accrual kernel + leave-request governance workflow; **publishes** attendance event + org-chart/직급 (#36) |
| **L14 payroll** 🆕 | `payroll/*` | HR attendance event, S4 | KR 4대보험+소득세 brackets (gov data, data-driven) |
| **L15 ERP** 🆕 | `erp/*` | financial, registry, S4 | GL, AR/AP, 재고, 부가세, purchase-request governance workflow |
| **L16 gov-adapters** 🆕 | `govadapter/*` | financial, payroll | 국세청 e-세금계산서, 공공데이터포털 rates, 도로명주소 |
| **L17 ontology** 🆕 | `ontology/*` | S3 authz overlay, audit-chain | no-code typed entities; jsonb + per-property `data_class`; RLS overlay |
| L18 authz/RBAC | `platform/authz` + `console-authz-cedar` 🆕 | — (S3 owns) | console-managed custom roles + approval graph policy |
| L19 quota/billing-plan 🆕 | `platform/quota` | comms, storage | SaaS tiering |
| L20 audit-chain 🆕 | `console-audit-chain` | — | tamper-evident sealing worker |
| **L21 MES** 🆕 future | `mes/*` | identity, ontology, erp, registry, hr, financial, audit-chain | Future manufacturing execution: production orders, routings, work centers, quality/NC, material traceability, OEE, labor capture; **do not fan out until org/people/assets/inventory/ERP/workflow/policy/import foundations are accepted** |

**Plus cross-cutting parallel lanes:** L-Q coverage backfill (per-domain console_rt/unit, ~80% of the 109
gaps), L-O ops/infra (log persistence #50, metrics, secrets #28), L-Auth (#19.25 enroll fix #49).

First parallel wave = the pure-domain/kernel lanes (L2 FSM, L7 messenger, L10/L14 money math, L13
leave-accrual) — zero shared infra, integrate via contracts only. **L13→L14** is the one ordered dep (HR
publishes the attendance event-contract in its first subtask so payroll starts against it immediately).

## 5. Stack-compat verdict

- **Backend:** same stack family (Rust 2024 + axum 0.8 + sqlx 0.8.6 + tokio). **Liftable = the ~7 pure
  std-only kernels** (`aggregate_invoice`, `evaluate_decision`, cursor-pagination, route-parity, quota
  reserve-reconcile, sla rollup, pressure-band) — drop into our `domain`/`kernel` with an identifier rename
  + replacing oyatie's `Classified<T>`/data-boundary dep with our `OrgId`. **Everything else
  reference-only** (oyatie is gRPC/tonic-primary; its domain crates are pure invariant models — there is NO
  runnable payroll or ontology engine to copy; **KR payroll + the ontology engine are built fresh**). GUC
  rename `oyatie.tenant_id`→`app.current_org`, role→`console_rt`.
- **Frontend: SKIP** (oyatie is SolidJS/Leptos; we're React 19 + Vite + Tailwind + shadcn + openapi-fetch).
  Only the codegen-determinism harness ports.
- **buck2: do NOT migrate for this build.** We already have the cargo equivalent (`ci.yml` + 6 gates). The
  `../maintenance-buck2` worktree stays an independent, non-critical track. Borrowable CI idea: the
  fan-out→single fan-in required-context + a `gate_registration` meta-test (a gate can't be silently
  dropped).

## 6. Execution model (the fleet)

1. **S0–S5 substrate** (single-lane, sequential) — land first. *(~1 wave; mostly already-have + the two
   collision-eliminating splits + the kernels.)*
2. **Fan out the 20 immediate lanes + the 3 cross-cutting lanes** as worktree-isolated agents, each taking a row from
   the bounded-context registry, building its crate family + openapi fragment + ko namespace + the trifecta,
   merging against frozen contracts. Concurrency ≈ disjoint domains (8–16 at a time under the agent cap).
3. **Integration is continuous** — per-domain fragments merge trivially; the route-parity/cohesion/bindings
   gates + the deploy-blocking trifecta catch cross-lane breakage; a nightly full-suite is the backstop.
4. **No lane checkpoints on red** — the deploy-blocking gate (S1) is the enforcement of "anything that
   breaks is apparent."

## 7. Risks (carried from review)

- Massive parallel ≠ sloppy — the deploy-blocking trifecta + per-lane security-review + visual-verdict ≥90
  hold the bar; the bounded-context registry + cohesion gate prevent two lanes touching one contract.
- The hardest fresh-builds (KR payroll brackets, 회계 double-entry, the ontology engine, 국세청 e-invoice) are
  net-new + regulated — they get golden-case tests + professional sign-off as release gates.
- Contract churn: if a shared contract (S1–S5) must change after fan-out, it re-serializes the affected
  lanes — so freeze the v1 contracts deliberately before fanning out.

## 8. Pre-fan-out substrate HARDENING (adversarial review: NOT-YET → GO-on-sequencing)

The review verdict: **architecture GO, sequencing NOT-YET** — 14 blockers + 16 highs, all fixable with
reservation schemes (no redesign), but they must EXIST before the immediate lane fleet starts writing. These become
binding substrate steps; **no lane spawns until all are green.**

### ⛔ S‑1 — The precondition: make red actually BLOCK deploy, then get green (THE biggest risk)
Confirmed live-system fact: **the deploy never gates on CI.** `image-release.yml` triggers on `push:`,
deploy job `needs: images` with **zero `ci.yml` dependency**; `ci.yml` has no `concurrency:`; **40/40
recent CI runs red while Image Release ships green on the same SHA.** So "no checkpoints on red because the
deploy gate makes breakage apparent" is fiction today — and our own G001 deploys went out with red CI.
Fan out 16 lanes onto this and we ship unreviewed red to a live tenant 16× faster.
- Gate `image-release` on CI success (`workflow_run: {workflows:[CI], types:[completed]}` + `conclusion==
  'success'`, or branch-protection required-checks + a merge queue).
- Add `concurrency: {group: ci-${ref}, cancel-in-progress: true}` to `ci.yml` (the bump-supersede churn).
- Drive CI green: pay down #47 (chronic red) + #31 with the REAL fix (render-once per describe, not the 6GB
  heap raise) — keep green N consecutive runs.
- **Fan-out targets a non-auto-deploying integration branch**, never `feat/multi-tenant-phase1`.
- **Acceptance gate: a deliberately-red PR is observed to block a deploy.** Until this passes, nothing spawns.

Progress checkpoint (2026-06-25): local workflow changes now add CI concurrency and an Image Release
`ci-gate` preflight that waits for the same SHA's `CI` run to complete successfully before images build.
The #31 web OOM workaround was removed after replacing `PlatformConsole.test.tsx`'s heavy full-`AppRouter`
mounts with a small route-guard harness plus direct page renders; the full web suite now passes without
`NODE_OPTIONS`. S‑1 is still not accepted until this is pushed through a non-auto-deploy path and a
deliberately-red change is observed to block deployment.

### Additional substrate (S6–S12)
- **S6 Migration numbering** — global `NNNN` seq (at 0059) → immediate lanes collide on `0060` (sqlx dup-version =
  silent DDL loss). Reserve disjoint bands per lane in the registry, frozen at fan-out, + a
  `DuplicateVersion`/`NonContiguous` migration-safety variant; better, switch new migrations to
  timestamp/ULID lexical prefixes so "next number" is no longer a shared write.
  > **Corrected 2026-07-28:** the `DuplicateVersion` variant proposed here **already exists and is
  > wired** — `ViolationKind::DuplicateMigrationVersion` in
  > `backend/ci/gates/migration-safety/src/lib.rs`, run as the "Migration-safety gate" step of the
  > `backend` job. So the loss is not "silent". The residual hazard is merge-ordering: the gate only
  > fires when both files are in one tree, and `main` is unprotected (no required checks, no
  > up-to-date requirement, no merge queue), so a stale-green lane can still merge and turn `main` red
  > afterwards. Band reservation remains correct for that reason, not for the stated one.
  Progress checkpoint (2026-06-25): `console-gate-migration-safety` now rejects duplicate `NNNN` prefixes and
  missing numeric gaps before the normal migration scan; the real repo gate passes through `0059`. The
  registry band reservation is still required before any multi-lane fan-out.
- **S7 Transactional outbox** — the attendance→payroll event seam is vaporware (no bus/outbox exists).
  Build `domain_events` (written in the SAME tx as the source mutation) + a relay (apalis) +
  `event_consumer_offsets`. **L13→L14 cannot start contract-only until this exists.**
- **S8 Cross-context coupling gates** — the edge-ban is a no-op; the real coupling is **SQL-level**
  (workorder reads `registry_equipment`; financial UPDATEs it in one tx; reporting JOINs 17 tables) and
  invisible to a Cargo-dep gate. Add a **SQL-ownership gate** (each adapter may only touch its owned-table
  set or a declared read-model/tx-port allowlist); declare sanctioned **shared-tx ports** (co-transactional
  writes) + **versioned read-model views** (hot in-tx reads, reporting) in the registry.
- **S9 RLS overlay safety (security: UNSOUND as drafted)** — `app.role` must be a **closed platform-only
  enum** with **allowlisted call sites** (compile-time-constant value, never derived from `users.roles`/
  Cedar); overlay `WITH CHECK` stays **strict tenant-equality** even when `USING` relaxes reads (gate
  asserts the asymmetry; `console_rt` test: tenant INSERT of `org_id=NULL` rejected); teach tenant-isolation +
  rls-arming gates about `app.role`; tighten the dynamic-RLS detector; **ship #43 (org-binding lint) as
  substrate**; invert rls-arming from denylist→allowlist + forbid pool aliasing.
- **S10 Contract freeze + regulated specs** — pre-populate `backend/registry/bounded-contexts.json` with
  **all** rows (14 + 7 greenfield: hr, payroll, erp, govadapter, ontology, authz, quota) carrying frozen
  `openapi_contract` + migration-band + `owner_lane` (JSONL, line-wise merges; lanes READ only). A
  contract-freeze gate: every fragment/event has a `version:`; a v1 change fails CI unless the version bumps
  AND consumers' pins update. Promote `docs/specs/payroll.md` + `accounting.md` with a **노무사/세무사 DESIGN
  review** (effective-dated rate schema, per-pay-item 통상임금 flags, 중도입퇴사 일할, 4대보험 정산) — day-1
  modeling, frozen before L14/L15 spawn.
- **S11 Pull L16 + L17 OUT of the fan-out wave.**
  - **L16 (국세청 e-invoice):** run a feasibility spike → one hard artifact: a named NTS endpoint + sandbox
    cred that issues a test 세금계산서 from a server with a 공동인증서, OR written confirmation it doesn't
    exist. **Likely outcome: no pure direct API; the government-sanctioned programmatic channel is a
    국세청-CERTIFIED ASP gateway** (홈택스 is manual). → **decision needed: amend "no ASP" to "a 국세청-certified
    gateway IS the government channel, not a forbidden middleman."** Do not fan out on an unverified API.
  - **L17 (ontology):** re-classify as a **post-spine extraction** task gated on a checkable boolean
    (≥6 production ObjectViewConfigs + the action executor + ≥2 lenses shipped green) — it cannot run
    concurrently with the spine it extracts FROM. (Correction to §5: oyatie DOES ship a runnable ontology
    crate + leave-accrual ledger — borrow the engine architecture; write only the KR rules fresh.)
- **S12 Lane-sandbox throughput** — invert the 230-line `build_router` to `mount()` registration
  (linkme/inventory) so the composition root never changes; pin shared deps in `[workspace.dependencies]`
  (Cargo.lock churn); per-worktree `CARGO_TARGET_DIR` + per-lane ephemeral Postgres (the 57 existing
  worktrees share one `console_ci` + one target dir — "8–16 concurrent" is wishful until isolated); shard
  Playwright (`--shard`, per-shard DB + distinct loopback IP for the per-IP auth rate-limit); per-crate
  `cargo sqlx prepare -p <crate>` + a `--workspace --check` determinism gate.

### Actual collisions the plan called disjoint (+ the reservation scheme)
| Point | Why it collides | Scheme |
|---|---|---|
| Migrations (`0059` flat) | 5 lanes pick `0060` → silent DDL loss | registry bands + `DuplicateVersion` gate, or ULID prefixes |
| `kernel/core` (65 consumers) | a signature change re-serializes everyone | freeze public surface at S4; new kernels → owning context's `domain` crate |
| CI gates (`ci/gates/*`) | concurrent rule-adds edit same crates + ci.yml | land all new gates in S1/S5 single-lane |
| Registry JSON | 6+ writers; cohesion-gate's own input | pre-populate ALL rows pre-fan-out; READ-only; JSONL |
| `app.role` GUC | invisible to both security gates; capability→row-scope leak | closed enum + allowlist + gate parses overlay |
| Generated clients (3) | regen wholesale; source split ≠ artifact split | one-lane claim (like Cargo.lock) OR per-domain client modules |
| `build_router` (230 lines) | 9 greenfield lanes each `.merge` → conflict cascade | invert to `mount()` registration; root never changes |
| `Cargo.lock` (6675 lines) | every dep bump rewrites it | `[workspace.dependencies]` pinning; lanes ref `workspace=true` |
| `.sqlx/` (39 files) | `--workspace` regen churns siblings | per-crate `prepare -p`; `--workspace --check` gate |
| Cross-context SQL | not a crate dep — Cargo edge-ban can't see it | SQL-ownership gate + declared read-model/tx-port seams |

### Should-fix (medium)
CODEOWNERS generated from the registry (mechanical cross-lane review, vs the advisory PR template);
orchestrator hands each agent exactly one registry row at spawn (don't self-assign); **vendor the lifted
oyatie kernels into this repo with a recorded source SHA** (don't leave regulated provenance depending on a
`/tmp` sibling — this project already lost a cluster to `/tmp`); replace "visual-verdict ≥90 as the merge
gate" with a deterministic Playwright `toHaveScreenshot` golden gate (keep the AI verdict as an authoring
aid); ontology shared (`org_id IS NULL`) rows carry schema metadata only, never instance data.

## 9. GitHub issue/comment intake (binding live-feedback ledger)

**Last polled:** 2026-06-26T04:20:00Z (2026-06-26 00:20 EDT). Source of truth:
open GitHub issues + issue comments in `jason931225/console`. Current live intake found PR #37 open for the platform/group/import-foundation slice; re-run this intake before every wave gate and before marking an issue-backed lane done.

**Intake rules.**
- A GitHub issue/comment becomes plan scope when it is actionable, maps to a business workflow, security
  constraint, acceptance criterion, or release gate, and does not require posting/retaining sensitive data.
- Comments that are operator clarifications become acceptance criteria on the owning lane, not a new lane.
- GitHub issues/comments are inputs, not orders. Reject or defer requests that would weaken product maturity,
  tenant isolation, privacy/compliance posture, auditability, data quality, operational clarity, or the core
  B2B SaaS domain model; record the rejection rationale instead of implementing the request.
- Do not adopt user-provided labels when they blur product boundaries. Translate them into the correct domain
  concept first (for example, the #20 "groupware" wording maps to corporate governance/approval workflows,
  not a legacy groupware clone).
- Sensitive GitHub content is never copied into docs or test fixtures; keep only the requirement shape and
  remove/redact secrets in GitHub. This incorporates the maintainer warning on #19 ("stop posting sensitive
  data here").
- Attachments are evidence only; the lane must turn them into tests/specs before implementation.
- FLMS history/master data work depends on a vendor data export/script, not installing a Windows MSI. This
  incorporates the maintainer comment on #19 ("I can't install msi").

| GitHub source | Valid intake | Owning lane / gate |
|---|---|---|
| [#18](https://github.com/jason931225/console/issues/18) + latest comment | OTP issuance bug; region edit; customer/site creation; intake priority admin-setting; equipment search/list visibility after import. | L1 identity + L3 registry + L2 workorder; Q0/Q1 console_rt regressions. |
| [#19](https://github.com/jason931225/console/issues/19) comments 9-25 | Excel progress import; equipment detail popup; org chart + rank; corporation grouping; intake/approval visibility; FLMS import/export dependency; purchase-request + 거래명세표 upload/scan; sales listing autofill/photos/fields; spare/substitution recommendation; external inquiries → support; PM schedule cycle; inactive-user deletion/archive + PC/QR passkey flow. | L1/L2/L3/L5/L9/L10/L13/L18; Q0 first for broken live loops, then F-track maturity. |
| [#20](https://github.com/jason931225/console/issues/20) + comments + 2026-06-25 operator field report | Site-scoped substitution dropdown; bulk OTP/passkey defects, including a live lost-device admin credential-reset failure that shows `패스키 재설정에 실패했습니다. 다시 시도하세요.` instead of issuing a replacement code; duplicate-name/deactivated-user archival and role-revocation behavior; intake equipment validation bug; daily-plan equipment selection, plan detail view, auto-review submission, admin visibility; worksite-level org tree/personnel view; customer/site/manufacturer/model/specification fields should be dropdown-first with add-new typing, and fields derivable from a known model should auto-fill where safe; corporate governance workflow/approval graph needs (기안서, 구매요청서, 휴가신청서) without adopting the legacy "groupware" label. | L1 identity/auth Q0 recovery gate, L2 workorder/daily plan, L3 registry, L4 dispatch, L13 HR/org tree + leave workflow, L15 ERP purchase workflow, L18 RBAC/approval graph. |
| [#24](https://github.com/jason931225/console/issues/24) | Production rollout is not done until `knllogistic.com` and `console.knllogistic.com` serve the current `main` web assets. GitHub state is clean after `v0.1.11` (`main` only, no open PRs, CI/Security/Release Please/Image Release green), but read-only production still shows Argo `root`/`maintenance` pinned to `feat/multi-tenant-phase1`, live rollout digests differ from the current overlay, and public hosts still serve the older asset hash. | Track L launch gate; requires live Argo/kubectl evidence or live asset match before closing. |
| [#17](https://github.com/jason931225/console/issues/17) comments | Daily backup preferred; weekly acceptable if storage-constrained; server-down/offline-resync concern; document leak prevention/alerting; VPN has access-friction and must not become a blanket requirement without UX/security tradeoff. | Track O resilience/security + Track L launch hardening; backup restore drill and document-access audit/alerting are release gates. |
| [#6](https://github.com/jason931225/console/issues/6), [#10](https://github.com/jason931225/console/issues/10) | Public landing page + marketing view of sale/rental assets; inquiry/contact/FAQ; payment/subscription interest. | L5 sales + L19 quota/billing-plan; keep payments behind explicit provider/security review. |
| [#7](https://github.com/jason931225/console/issues/7), [#9](https://github.com/jason931225/console/issues/9) | Daily diary/progress exports for internal reporting/corporate-governance approval; import existing Excel progress so operations continue from current sheets. | L12 reporting + F1 import/export + L18 approval graph; generated XLSX must be tested against sample attachments. |
| [#8](https://github.com/jason931225/console/issues/8), [#15](https://github.com/jason931225/console/issues/15) comments | Equipment master/detail system; residual-value math from depreciation + repair costs; repair/failure history by vehicle; warranty/contract history; mobile-readable equipment tab; simple generic 3D/failure hotspot view if cheap. | L3 registry + L10 financial + L12 reporting + P object view. 3D is optional only if it does not delay core equipment history. |
| [#11](https://github.com/jason931225/console/issues/11) | User permissions page, create/remove users, lower org groups, edit user details, mobile app linkage, deactivation on leavers. | S3 authz/RBAC + L18; ties to #20 inactive-user/archive comments. |
| [#12](https://github.com/jason931225/console/issues/12), [#13](https://github.com/jason931225/console/issues/13) | Visual rental dispatch map by province/city/customer site; move/displace assets; customer/site registration with contact/address; arrival/departure location events. | L4 dispatch + L3 registry + C dispatch map; consent-gated arrival/return markers, not 24h live tracking. |
| [#14](https://github.com/jason931225/console/issues/14) | Intake receipt form with required fields, equipment model pulled from master list, request date, symptom, contact. | L2 workorder intake + OpenAPI/UI tests. |
| [#16](https://github.com/jason931225/console/issues/16) | AI assistant request for maintenance recommendations and report generation. | Valid only as final-stage operations intelligence. First build deterministic/probabilistic maintenance recommendations, structured workflow history, observability, evaluation, tenant isolation, privacy review, and no-direct-write workflow drafts per `docs/specs/operations-intelligence.md`; reject any request for premature autonomous AI action. |

**Latest #20 clarification:** do not adopt "groupware" as the product/domain label. The clarified need is
corporate governance capabilities: draft requests, purchase requests, leave requests, and other workflow +
approval-graph flows. L13/L15/L18 should model the actual request/approval capabilities and policies;
L13/L18 should still classify people by product access, site/department responsibility, and operational
capabilities rather than a legacy groupware-user taxonomy.

**2026-06-25 field report tied to #20 auth recovery:** the admin lost-device credential-reset dialog can
fail with `패스키 재설정에 실패했습니다. 다시 시도하세요.` instead of revoking passkeys and returning a
replacement one-time code. Treat this as a Q0/L1 recovery gate: identify the server/API failure class, add
a regression for the affected authorization/branch/role path, and keep personally identifying details out
of docs and fixtures.

**2026-06-25 #20.6 intake resolution:** the "호기 선택 후 접수등록해도 호기수를 제대로 입력하라"
report was a multi-branch mismatch: the lookup could resolve equipment in a secondary assigned branch,
while submit used the first active JWT branch. The web intake submit now carries the resolved
equipment `branch_id`; the regression asserts a selected cross-branch equipment submits with that branch.
Code is merged in `4ebcfe2`; release `v0.1.7` / prod-overlay bump `a3df78e` carry the fix.

**2026-06-25 #20/G002 org-hierarchy security foundation:** `6c7d121` adds the backend security base for
worksite-level org tree/personnel and group-level governance without widening tenant RLS: group identity,
`organizations.group_id`, owner-only memberships/grants, SECURITY DEFINER resolvers, tenant-isolation gate
coverage, and runtime-role tests. `5a95983` adds the pure kernel `AccessScope`/`ScopeNodeId` + BranchScope
projection bridge without any `ScopeNodeId`→`OrgId` conversion. Follow-up releases carried P2 JWT/principal
scope (`v0.1.10`) and P3 consolidated-read helper (`v0.1.11`). Remaining scope still includes the tenant-tier
group-admin REST/authz/grant surfaces, UI org tree, site-responsibility surface, and approval/workflow graph
under G002/L13/L18.

**2026-06-26 local #20/G002 platform-operator bridge:** the current worktree adds an audited platform-only
single-tenant management context (`/api/platform/tenant-context`) and projects group identity into platform
tenant/ops reads so the platform console can switch scope between 전체 / 그룹 / 개별 조직 and then manage one
org. This is a vendor support bridge for "see all together / isolate to each org"; it does **not** close
tenant-tier group-admin/org-chart/personnel/site-responsibility/approval-graph scope, which remains in
G002/L13/L18.

**2026-06-26 #20 comment refresh:** latest #20 comment through the fresh poll is the owner G002 P3
consolidated-read status update; no newer non-owner requirement was found after it. Keep P4+ REST/authz/UI
follow-up open and continue treating future GitHub issue comments as intake candidates, not automatic scope.

**2026-06-25 #24 rollout state after v0.1.11:** GitHub delivery is clean (`v0.1.11` at `378f00f`,
prod-overlay bump `7ed8194`, open PRs 0, remote branches `main` only, CI/Security/Release
Please/Image Release green). Read-only prod verification still shows Argo `root` and `maintenance`
Applications pinned to `feat/multi-tenant-phase1`, running older `console-app@8bbf...` and `console-web@eb78...`
images instead of the current overlay (`console-app@5095...`, `console-web@7646...`); both public hosts still
serve `/assets/utils-DRMbRFdX.js`. Keep #24 open until a production Argo patch/sync to the intended current
revision is performed and live assets match the current overlay.

**2026-06-25 issue-backed #19 cleanup:** code/UI already use a 300 m default geofence; API/client docs and
comments must not drift back to the obsolete 150 m wording. The GPS consent settings page must keep the
current consent status usable when the audit ledger endpoint is unavailable or unauthorized; ledger loading
is optional for mechanics, status loading is not.

**2026-06-25 legal/privacy release gate:** initial console login now needs an engineering control that
records separate required acceptance of 개인정보 수집·이용 and service terms before first passkey
enrollment. Public storefront work also needs cookie/privacy notice, footer copyright, semver display, and
family-site links (COSS + Bestec). This control is not legal sign-off: the go-live checklist still blocks
production until counsel/management approve and publish the formal privacy policy, location terms, and
cookie notice.

**2026-06-25 mechanic-facing copy refinement:** the exact "담당 정비사" label is mechanic-facing copy only.
Admin/planner/inspection assignment surfaces should use "정비사" while preserving the assignment selector and
validation behavior, so non-mechanic screens do not present the field as if the current operator is the
assigned mechanic.

**2026-06-25 reference-data UI clarification:** customer name, site/worksite name, manufacturer, model,
and specification should not be raw memory-only text fields. Prefer searchable dropdown/typeahead controls
that still allow a new value to be saved when it is missing from the list. A known model may derive safe
defaults such as manufacturer, specification, and tonnage, but derivation must not overwrite a value the
operator has already typed.

**Bottom line: S‑1 first (prove a red PR blocks deploy), then S6–S12 + freeze the registry/contracts +
pull L16/L17 out — then the 20 immediate lanes fan out safely. L21 MES is future scope only until
org/people/assets/inventory/ERP/workflow/policy/import foundations are accepted.** The architecture stands;
only the sequence changes.
