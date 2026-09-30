> **Subordinate program record — not authority.** Current product authority is [`docs/current/PRODUCT.md`](../current/PRODUCT.md), [`ROADMAP.md`](../current/ROADMAP.md), and [`DELIVERY.md`](../current/DELIVERY.md). This register **cannot authorize work**, open a HOLD, admit a capability, or amend an ADR. Where it disagrees with current authority or an accepted ADR, authority wins and the conflict is a recorded open item (§5). Decisions here become executable only when a reviewed candidate lands them in authority.

# Entity-Platform Decision Register

> **Purpose.** A `G0 → G6` delivery plan was reviewed against the tree, found not implementation-ready,
> and its blockers were resolved by interview. Every selection was then attacked adversarially, and the
> ontology/contract layer was re-derived from first principles against a "modern SAP-class omni SaaS"
> north star. This register is the durable record of that reasoning. Without it the output exists only
> in a session transcript.
>
> **Why it is a register and not an ADR.** Roughly one selection in five did not survive attack, and
> every reversal was forced by repository evidence rather than argument. Rows are expected to keep
> moving. ADR promotion comes when they stop.
>
> **Review base:** `b68e89ff06a6738c64ec0e360f19acf7ae3d0f83`. Every claim below was re-verified
> against that tree; §6 lists the commands. `origin/main` advances — re-verify before acting.
>
> **No gate may read this file.** It is classed `evidence` in the documentation manifest. If a
> required product gate ever takes it as an input, it becomes exactly the prose-controlled gate
> input this program exists to eliminate, and it must be converted to an executable check instead.

## 1. What changed about the program

The decisions below shift the program from *"ship an HR/payroll slice with an ontology underneath"*
to *"build a governed entity-authoring and maintenance platform, proven by a complete HR/payroll
vertical."* The original `G0 → G6` milestone text describes the former and cannot be resumed as
written. Its reusable parts are the disciplines, not the milestones: strict gate order, one writer
per object, exact-SHA review, deny-by-omission, real PostgreSQL evidence, non-payable scope, and
explicit production/compliance HOLDs.

## 2. Why the original plan was not implementation-ready — 7 blockers

1. **Authority incomplete or contradictory.** Revision hashing is unauthorized by
   `docs/current/ROADMAP.md:18`. The plan's G3 exit claims "branchless capability" with no candidate
   scheduled. Frontend is HOLD at `docs/current/PRODUCT.md:35`, and
   [`ADR-0030`](../decisions/ADR-0030-console-rebuild-charter-leptos.md):228 separately forbids the
   mounted shell — an ADR-0001 amendment alone opens neither.
2. **The completion/readback protocol is impossible.**
   `.github/workflows/console-authority-bootstrap.yml:4` triggers on `pull_request_target` only, so
   `authenticate-console-authority` can never appear on a merged `main` SHA. A candidate also cannot
   embed its own future squash SHA. Branch protection currently requires **zero** human approvals,
   so green checks never prove "independent review." → resolved by **V7**.
3. **Approval binding too weak.** `0158_create_gov_approval_requests.sql` stores a descriptive
   `payload_summary`; `0164_bind_consume_four_eyes.sql` binds consumption to request/kind/target,
   not to the exact mutation. → resolved by **A5**.
4. **G4's atomic transaction is impossible under current credentials.**
   `backend/crates/platform/db/migrations/0221_employee_create_routing_authority.sql:83-86` revokes
   `leave_api.set_employee_home_branch_create` from `console_rt` and grants EXECUTE only to
   `console_leave_cmd`. Borrowing a transaction does not merge two roles. → resolved by **A1**.
5. **No browser→SSR authentication path.** `platform/request-context/src/lib.rs:276-281` requires an
   `Authorization: Bearer` header; the refresh cookie is `Path=/api/v1/auth`
   (`platform/auth-rest/src/lib.rs:3525`); the access token never becomes a cookie. A plain
   navigation to `/overview` cannot authenticate. → resolved by **B1**.
6. **PayRun is not fail-closed.** In `payroll/adapter-postgres/src/lifecycle.rs`: an empty roster
   satisfies attendance completeness (`missing_total == 0`, ~line 336); calculation sets
   `status='CALCULATED'` with blocked lines present (~line 686); submit checks exceptions but not
   roster coverage (~line 920). → resolved by **P4**.
7. **The test-inventory premise is already false.** `tools/ci/postgres-cargo-map.json` declares
   `mapped: 222, workflow_targets: 207` but holds **224 entries / 209 in-workflow**. No generator
   owns the file. 17 Buck-only binary identities exist, not 2. → resolved by **V1**.

## 3. Decision register

**Verdict key** — **HELD**: survived attack unchanged · **BOUND**: survives only with the stated
condition · **REVERSED**: attack overturned the original selection. **[E]** marks a verdict forced
by repository evidence rather than reasoning; those may not be re-litigated without new evidence.

**A decision without its binding condition is a different decision.** The condition column is
normative, not commentary.

### 3.1 North star and platform shape

| # | Decision | Verdict | Binding condition |
| --- | --- | --- | --- |
| N1 | North star = **composable enterprise operating system**, not SAP feature-cloning | BOUND | Anti-platform fence: shared infrastructure is earned by a **second real domain**. Exception: security and delivery controls, which are inherently cross-cutting |
| N2 | Product goal = **governed entity drafting and maintenance**; HR/payroll is the proof, not the point | HELD | — |
| N3 | Two drafting planes: **definition** (types, properties, links, actions, policies) and **record** (a given Company / Employment / PayRun) | HELD | Never collapsed into one generic draft system |
| N4 | **Thin dual proof** as the first center of gravity: one canonical vertical plus one tenant-authored entity | HELD | Attack noted the doubled critical path; accepted, because a single-sided proof cannot demonstrate the platform claim |
| N5 | **Two-tier authors**: platform-owned core models, tenant-owned extensions | HELD | — |
| N6 | **Disjoint authorities** for definition source-of-truth | BOUND | A core resource is Git-owned *forever*; a tenant resource registry-owned *forever*; both compile through one versioned IR. Ownership changes only by explicit migration ceremony. Never two editable copies — that is the Dataverse layering failure |
| N7 | **Bounded extensions** for tenant power | BOUND | Extension data may not silently join a canonical transaction. The canonical owner must explicitly accept a transaction-borrowing extension capability; otherwise the extension commits separately and the UI must say so |
| N8 | **Dedicated Definition Studio** as the author surface | BOUND | It is a second privileged application surface: its own authorization doors, nondisclosure tests, audit, rate and size quotas, and rollout gate |
| N9 | **Layered preview** before publication | BOUND | Default preview is generated fixtures plus aggregate profiles. Row-level masked tenant data is an explicit, audited, short-lived opt-in with re-identification tests, no egress, and guaranteed cleanup |
| N10 | **Two draft engines** — definition vs. record | BOUND | Two state machines, two owners. They may share only stable envelope primitives (identity, base revision, expiry, diff, conflict, receipt), and only after conformance proves the semantics match |
| N11 | **Safe subset** of automatic published-schema change | BOUND | Forward repair, not rollback. Published records stay bound to their schema version; restoring old semantics is a new release; no schema action may claim to undo data or external side effects |
| N12 | **Branches and proposals** for Studio collaboration | BOUND | Branches may rebase, but publication is one atomic package release. No partial merge; no branch action writes main business state; abandoned branches have quotas and deterministic cleanup |
| N13 | **One package** publishes atomically on proposal approval | BOUND | Atomic strictly within the database boundary. External effects are outbox intents — the package contract must state this rather than implying end-to-end atomicity |
| N14 | **Exact artifact stages** for tenant-package ALM | HELD | Non-production for this delivery; the model is fixed now to avoid a later rewrite |
| N15 | First tenant-authored proof entity = **onboarding checklist** | HELD | Chosen for being structurally incapable of becoming a shadow Employment or payroll writer |
| N16 | First tenant-side author persona | **REVERSED** | A line manager publishing schema, policy, or action changes is a privilege-escalation path. Redefined as **guided proposer**: a small grammar, proposal only; a steward owns review and publication |

### 3.2 Data model and contracts

| # | Decision | Verdict | Binding condition |
| --- | --- | --- | --- |
| D1 | Canonical hash byte format | **REVERSED [E]** | RFC 8785 JCS cannot faithfully cover the existing `i64`/`u64`/exact-decimal/arbitrary-JSON value space without narrowing the public model or string-wrapping numbers — it would make an existing object unmodifiable after v2. Adopt **deterministic CBOR** as a *versioned internal hash preimage over a closed typed AST* only. JSON remains the API and debug form; the row stores `hash_version` plus encoding profile; cross-language golden vectors are required; malformed or non-preferred encodings fail closed |
| D2 | v2 hash rollout = **expand → drain → emit** | BOUND | Proving the *drain* is the hard part. Rolling deploys, lagging workers, and queued writes all produce a v1 write after an operator believes the drain finished. Drain completion requires a durable observed-writer marker plus a quiet period — never a deploy timestamp. A v1 write after the cutover boundary is a **downgrade failure**, not a legacy-success status, and that alert must fail closed and have a named owner |
| D3 | Revision hashing enters via **authorize-before-slice** | HELD | Authority (ROADMAP) precedes the implementation candidate |
| D4 | Canonical entity contracts = **manifest-first**, compiled to Rust and SQL | HELD | Carries a known cost: a third source of truth whose drift detection is itself a deliverable. Destructive schema operations may **not** be auto-generated |
| D5 | Tenant-authored records = **revision ledger plus projected indexes** | HELD | — |
| D6 | Extension ↔ canonical interaction = **explicit compound action** | BOUND | The compound action is the new concentration of power, and partial failure is where the boundary is actually tested. It must declare participants up front; partial failure is a defined, receipted outcome, never a silent partial commit |
| D7 | Tenant/account vs. legal Company = **one account, many Companies** | BOUND | Permitted only where the Companies share one contractual security, residency, encryption, backup/erase, and data-controller boundary. Otherwise each Company is its own tenant under Group federation. `org_id` and `company_id` are **never** inferred equal, even when a migration seeds a default Company with the same UUID |
| D8 | REST contract migration = **complete, face by face** | BOUND | Sequence corrected: shared contract infrastructure → admitted HR/payroll faces → **the real vertical** → remaining legacy faces → UI contract completeness. Converting every face before the vertical is platform-first work that delays the proof the platform claim depends on |
| D9 | Strict `command_id` / `expected_revision` | **MODIFIED** | Not unconditional v2 versioning. Evidence-conditioned on a consumer census: a v1 path that cannot honestly synthesize `command_id` or `expected_revision` may not be preserved and called safe |
| D10 | v1 endpoint retirement = **evidence-based sunset** | HELD | Absence of observed traffic is weak evidence for external consumers; the census must be positive identification, not silence |

### 3.3 Authorization, approval, transactions

| # | Decision | Verdict | Binding condition |
| --- | --- | --- | --- |
| A1 | HR atomicity topology = **capability transaction** | BOUND **[E]** | Topology is proven in-tree: 37 migrations declare `SECURITY DEFINER` functions, 97 `SET search_path` pins exist, and three command roles are already in use (`console_leave_cmd`, `console_ontology_cmd`, `console_platform_force_cmd`). But a `console_hr_cmd` holding EXECUTE across Person + Employment + branch + receipt + audit composes into a **god-role by composition** — whatever it may call, it may call in any order. Therefore: sequencing and completeness invariants live **inside** the definers, never in the caller; `console_hr_cmd` gets no table DML; every definer pins `search_path`; `console_platform_force_cmd` must be unreachable from this path. Resolves blocker #4 |
| A2 | Temporal-grant overlap enforcement | **MODIFIED** | `btree_gist` GiST exclusion as a **prerequisite**, not a runtime choice — no fallback schema selected by environment. Availability must be proven under the *real migration role*, and reconciled against [`ADR-0032`](../decisions/ADR-0032-effective-dated-grants-and-authority-freshness.md)'s promised fallback and [`ADR-0024`](../decisions/ADR-0024-bare-metal-portability-and-ha.md) portability |
| A3 | Approval expiry = **policy TTL, capped** | HELD | — |
| A4 | Approval assurance = **transaction-bound passkey** | HELD | Chosen over "recent step-up": a valid session can authorize a payload the human never intended |
| A5 | **`ApprovalClaim`** immutably binds tenant, requester user and Person, approver Person, action, target, `command_id`, `expected_revision`, and canonical payload digest | HELD | Execute recomputes the digest, replays before consuming, and consumes inside the owner transaction. Preflight may check, never consume. Resolves blocker #3 |

### 3.4 Browser surface

| # | Decision | Verdict | Binding condition |
| --- | --- | --- | --- |
| B1 | `/overview` authentication = **same-process opaque BFF session** | HELD | The cookie carries only a random opaque identifier; session state stays server-side and internal. Resolves blocker #5 |
| B2 | Session policy = **short, rotating** | BOUND | Rotation races SSR navigation and multiple tabs, and refresh-family reuse detection can false-positive a legitimate second tab into a session kill. Concurrent-tab semantics must be specified before implementation, not discovered in production. Revocation, CSRF/origin handling, and logout are in scope |
| B3 | Protobuf for the browser transport | **REJECTED** | Would create a second contract authority and a second browser transport with no identified scaling problem. Retained as a possible future service-to-service option behind the application boundary |
| B4 | UI division = **four ownership crates, one aggregate bundle** | HELD | — |
| B5 | `/overview` exclusion = **compile-time and runtime gates** | BOUND **[E]** | A proven pattern exists and must be copied rather than reinvented: `console-gate-dev-auth-absence`, run at `scripts/check-ci-preflight.mjs:941` and enforced at `.github/workflows/image-release.yml:715`. Must also resolve the standing hazard that a compile-gated binary in CI is not the binary under test |
| B6 | Localization = **Korea-first locale kernel** | BOUND | Locale, calendar, money, and jurisdiction must be explicit seams. Hard-coded Korean assumptions presented as "omni" would falsify the north star rather than serve it |

### 3.5 Payroll

| # | Decision | Verdict | Binding condition |
| --- | --- | --- | --- |
| P1 | Truth model = **versioned frozen roster** | HELD | — |
| P2 | Calculation sources = **canonical hybrid** | HELD | Canonical Employment decides roster membership; contract wages decide remuneration; attendance is a completeness gate; an immutable verified NTS artifact supplies withholding inputs. A mismatch **blocks** — nothing is inferred, defaulted, or zero-filled |
| P3 | Correction path = **state-specific correction** | BOUND | Immutable snapshots are acceptable only because operators are given a bounded, explicit correction path for backdated or corrected source facts |
| P4 | Fail-closed invariants | BOUND **[E]** | `0186_payroll_run_lifecycle.sql:59` declares `payable BOOLEAN NOT NULL DEFAULT FALSE` with a comment, and no adapter writer sets it. **A default is not enforcement**, and "no in-scope writer sets it" is a code-review claim — exactly the prose-controlled gate this program exists to remove. Enforcement must be a real database object: column-level `REVOKE UPDATE (payable)` from the runtime role, or a CHECK/trigger. Plus: nonempty frozen roster, exactly one successful latest calculation per roster member, zero blockers, zero unresolved exceptions. UI darkness is not sufficient while issuance and payment routes stay mounted. Resolves blocker #6 |

### 3.6 Workflow / ActionGraph

An audit found that [`ADR-0018`](../decisions/ADR-0018-clean-room-rust-corporate-workflow-engine.md)
describes an engine that **does not exist as claimed**. Hosted `main` carries two incompatible
workflow languages, no compiler between them, parser/runtime drift, no durable cursor, no general
timer/retry/resume engine, and a direct-run stub. Extending that walker would preserve false
guarantees. Reconciling ADR-0018 is an open item (§5.3).

| # | Decision | Verdict | Binding condition |
| --- | --- | --- | --- |
| W1 | Workflow ambition | **REDEFINED** | Not a horizontal automation clone. "Full" means complete durability and operator semantics for a **narrow, ontology-native ActionGraph** |
| W2 | Tenant-executable behavior = **state transitions** | HELD | The finite-state transition model *is* v1 of the workflow DSL: typed guards, bounded assignments, explicit human tasks, transactional outbox intents. No arbitrary loops, scripts, SQL, network calls, dynamic capability names, or hidden clocks. A general orchestrator is admitted only after two independent real workflows demonstrate what the grammar actually needs |
| W3 | Durable execution substrate | **REVERSED [E]** | [`ADR-0024`](../decisions/ADR-0024-bare-metal-portability-and-ha.md):42 clause 5 forbids a provider-managed service becoming "a prerequisite for the portable core or the self-host reference"; clause 1 makes end-to-end self-host the delivery gate. Temporal as *the* substrate adds a required stateful cluster with its own persistence, backup, restore, and HA evidence. Therefore the **reference substrate is Postgres-backed durable execution** — the database is already a hard dependency — and Temporal becomes an *optional context adapter behind the same port*. This preserves the original intent (a replaceable substrate) while satisfying accepted portability authority |
| W4 | The two existing workflow contracts = **census, then contain** | HELD | — |

### 3.7 Delivery, verification, tracker

| # | Decision | Verdict | Binding condition |
| --- | --- | --- | --- |
| V1 | Gate and test inventory authority = **generated graph** | HELD | Keyed by `(package, binary, feature set)`; compares test *identities*, not command counts or lexical reachability. Known hazard: the generator becomes unverified authority on day one unless checked against what it replaces during a dual-run window. Resolves blocker #7 |
| V2 | Runtime execution evidence | **REVERSED [E]** | `cargo-nextest` is **already adopted and already pinned** — `.config/nextest.toml` exists under a "DN-0005 P3" control and `tools/ci/check-nextest-config.mjs:38` pins **0.9.138**. "Pin nextest" was never an open decision. Restated: adopt the *existing* pin as execution-evidence authority and reconcile with DN-0005 rather than re-pinning. Nextest may **not** be the sole authority, because it cannot run doctests — and doctests here carry `compile_fail` claims, the only artifact able to hold a NEGATIVE type-boundary assertion ("this must not compile"). **Corrected 2026-08-17 — see the amendment note below; the original wording of this row was wrong on the specifics.** |
| V3 | Playwright prerequisite setup = **layered real backend** | BOUND **[E]** | Login is a passkey/WebAuthn ceremony (`platform/auth-rest/src/lib.rs:66`), so "log in through the real flow" requires a CDP virtual authenticator. The tempting shortcut is the `dev-auth` feature — which `console-gate-dev-auth-absence` exists to keep out of every shipped build. Enabling `dev-auth` to make login work would prove a path that ships nowhere: an explicit stop condition |
| V4 | `.beads` dirty-file custody | **RESOLVED [E]** | Inspection closed this. The delta is a one-line `issue-prefix: "console"` config setting plus 6 append-only `interactions.jsonl` records (all `actor: jasonlee`) and 4 untracked `issues.jsonl` records — all recording the *closure* of issues whose work is **already merged into `origin/main`**, the last one citing `b68e89ff` itself. This is the trailing local audit trail of completed work, not another lane's live state. Verdict: **preserve**; it is a record of merged history. The prior framing as a program-level custody blocker was disproportionate to a 7-line delta |
| V5 | Fully autonomous agent review as the merge gate | **REVERSED** | Candidate-controlled workflows, prompts, hooks, and shared credentials make N agents **one correlated trust domain**. A three-agent quorum inside that domain is redundancy, not independence |
| V6 | Replacement merge gate = **external trust plane** | HELD | The reviewer runs outside the candidate's trust domain; the authoring agent may never self-approve; autonomous merge occurs only through branch protection after independent exact-head review contexts succeed. The *design* of that plane is still open (§5.2) |
| V7 | Corrected readback protocol | HELD | Require all three contexts on the **frozen PR head** → named independent review of that same head → squash merge → prove reviewed-head tree equality → require only `Required / CI` and `Required / Security` on the merged `main` SHA → record the implementation merge SHA in a **documentation-only closeout PR**, whose own readback lives in Beads/GitHub rather than recursively inside itself. Resolves blocker #2 |
| V8 | Tracker granularity | **REVERSED** | "One child issue per gate" is unworkable — gate issues **plus bounded candidate/lane issues**. The larger gates are far too big for one ownership and one review receipt each |

### 3.8 Amendments

Rows are corrected here rather than silently rewritten, so a reader can see what
the register got wrong and on what evidence it changed.

**A-1 · 2026-08-17 · row V2 — the doctest claim was wrong on the specifics, and
the real defect was worse.**

The original V2 wording asserted that ten files carried dark doctests, naming
`ontology/canonical-domain/src/lib.rs` and `ontology/adapter-postgres/src/instances.rs`
among "precisely the crates carrying the A2, D1, and D4 contracts". Verified
against `b68e89ff` with rustdoc rather than by grepping for fences:

- Those two files carry ` ```sql ` and ` ```json ` fences. Rustdoc never compiles
  or runs them. **Both packages contain zero doctests.** The claim was an
  artifact of counting fences instead of asking the tool.
- The genuine dark set is `console-platform-authz` (17 doctests, 10 of them
  `compile_fail`) and `console-workorder-application` (1).
- The larger defect the row missed entirely: the doctest gate was **executing
  zero tests**. `--doc -p console-kernel-core` was the whole gate from
  `4a9c7579 (#559)`, and that crate has no doc fence anywhere in its source, so
  the step reported green on 0 tests for months. It is not recorded in
  `docs/program/false-green-gate-holes.md`.
- The ten dark `compile_fail` claims guarded `ResourceBranch`, `authorize`,
  `authorize_scoped`, and `GrantValidity` — the branch-scope and effective-dated
  grant contracts, i.e. A2's subject matter.

Remedy landed as `--doc --workspace`: 23 doctests discovered, 18 execute and
pass, 5 are `ignore` fences that never execute by design. Nothing fails, so it
arms existing coverage rather than changing behaviour. Workspace-wide rather
than a longer `-p` list because a hand-maintained list is exactly what failed,
and nothing compared it against the tree.

**The transferable lesson**, which generalises past this row: the wrong claim
came from inspecting source text, and the correction came from running the
tool. Fence-counting is static reachability; `cargo test --doc` is execution.
The register already says static reachability is not runtime execution (V1, V2)
— it then violated its own rule. Treat any row justified by grep alone as
provisional until something executes.

## 4. Adversarial method and result

Every row was attacked on security, operability, compatibility/portability, opportunity cost, and
whether it serves entity-authoring or merely resembles enterprise breadth. A selection that merely
sounded strongest was not treated as decided.

**Result across both passes:** 6 reversed or redefined (D1, N16, W3, V2, V5, V8), 3 materially
modified (D9, A2, W1), 1 rejected outright (B3), 1 resolved by inspection (V4), the remainder held —
17 of them with a binding condition the original selection did not carry.

**The signal worth keeping:** every evidence-forced verdict **[E]** overturned or bound its
decision, and none confirmed one. Decisions made in the abstract about this repository have been
wrong at a high rate. Attack the tree, not the argument.

## 5. Open items

1. **The `G0 → G6` plan has not been rewritten.** Its scope, sequence, and exit criteria all change
   under §3. Resuming the old text would implement a superseded program.
2. **The V6 external-trust-plane design is unspecified.** V7's corrected readback protocol depends
   on it. This is the single largest undesigned dependency.
3. **ADR reconciliation is unscoped.** Implicated: ADR-0018 (engine claims vs. reality), ADR-0030
   (UI HOLD and repository structure), ADR-0031 (contract authority), ADR-0032 (grant fallback vs.
   A2), ADR-0024 (portability vs. A2's `btree_gist` prerequisite), ADR-0001 (`Layer::Ui`). Which are
   amendments, which supersessions, and which new ADRs is undecided.
4. **The D9/D10 consumer census has not been run**, and D10 requires positive identification rather
   than observed silence.
5. **`docs/current/{PRODUCT,ROADMAP,DELIVERY}.md` still describe the pre-decision program**,
   including the stale statements the original G0 was meant to correct — documentation-index
   coverage, and a PostgreSQL inventory count fixed at 183 that should be generated.
6. **§3 is not authority.** Nothing here is admitted until a reviewed candidate lands it.

## 6. Verification

Re-checked against `b68e89ff06a6738c64ec0e360f19acf7ae3d0f83`:

```sh
# §2 blockers
head -12 .github/workflows/console-authority-bootstrap.yml                    # #2 trigger
grep -n 'REVOKE\|GRANT EXECUTE' \
  backend/crates/platform/db/migrations/0221_employee_create_routing_authority.sql   # #4
grep -n 'Bearer' backend/crates/platform/request-context/src/lib.rs           # #5
grep -n 'Path=/api/v1/auth' backend/crates/platform/auth-rest/src/lib.rs      # #5
grep -n 'payable\|CALCULATED' backend/crates/payroll/adapter-postgres/src/lifecycle.rs  # #6
node -e "const p=require('./tools/ci/postgres-cargo-map.json');\
 console.log(p.counts, p.entries.length, p.entries.filter(e=>e.in_workflow_postgres_job).length)"  # #7

# §3 evidence-forced verdicts
# V2 doctests — ask rustdoc, do NOT grep for fences (see amendment A-1: fence
# counting produced a wrong row; ```sql/```json fences are not doctests).
SQLX_OFFLINE=true cargo test --locked --manifest-path backend/Cargo.toml \
  --doc --workspace                                                           # V2 doctests
grep -n 'nextest' tools/ci/check-nextest-config.mjs                           # V2 existing pin
sed -n '42p' docs/decisions/ADR-0024-bare-metal-portability-and-ha.md         # W3 clause 5
sed -n '59p' backend/crates/platform/db/migrations/0186_payroll_run_lifecycle.sql       # P4
grep -rc 'SECURITY DEFINER' backend/crates/platform/db/migrations             # A1 topology
grep -n 'PASSKEY_LOGIN_START_PATH' backend/crates/platform/auth-rest/src/lib.rs         # V3
```

`origin/main` advances. Re-run before acting on any row.
