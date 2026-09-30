# Console roadmap authority

Status: active roadmap authority. Product scope comes from [`PRODUCT.md`](PRODUCT.md); delivery and issue state come from [`DELIVERY.md`](DELIVERY.md).

## Ordered work

1. **Documentation custody and active authority**
   - Establish README plus PRODUCT, ROADMAP, and DELIVERY as the only active authority concerns.
   - Generate the full first-party tracked-document manifest with class, owner, status, replacement, retention, and blob SHA. Exactly one of 454 records is a signed, validator-read-back archive probe and `archive_tag` remains null for the other 453; `scripts/check-doc-links.mjs` and `scripts/console/generate-documentation-manifest.mjs` fail closed on every non-null reference. GitHub has no tag-target ruleset and its single origin is not independent off-device custody, so this probe does not authorize `coverage: complete`, bulk moves, deletion, or graveyard copies. Vendored trees (`third-party/`, 9 tracked files) remain outside that project-owned universe; former tracked agent-delivery trees such as `.grok/` have been removed and cannot regain authority merely by reappearing locally. `buck-out/`, `node_modules/` and `target/` are also excluded but contain no tracked Markdown.
   - Fail CI on new unclassified documents. The index reached `first-party-manifest` coverage, which is complete over the checker-defined universe rather than over every tracked Markdown file: `check-doc-links` enforces exactly one record per document inside that universe and rejects records outside it. Agent/runtime trees such as `.grok/` remain outside that universe; the exclusion is not permission to restore them or place active authority there. `authority-slice` survives only as a legacy alternative the checker still accepts.
2. **Executable-contract decoupling**
   - Replace machine checks that depend on historical prose or draft ideas with source-derived or machine-readable contracts while retaining behavioral regressions.
3. **Delivery substrate convergence**
   - Done: `Required / CI` (`.github/workflows/ci.yml`) and `Required / Security` (`.github/workflows/security.yml`) are out of shadow mode, report success on recent runs, and are merge-blocking required contexts on the integration branch alongside the independent `authenticate-console-authority` check.
   - The canonical integration and source-release branch is `dev` (not `main`). Ordered mirrors: `dev` → `staging` → `canary` → `production`. Required checks (`authenticate-console-authority`, `Required / CI`, `Required / Security`) must have an executable protected workflow producer on `dev`, and on every branch where they are enforced, before they are required there. CI, Security, Nightly, CAS seed, bootstrap `assertLivePullRequestSnapshot`, and release-please key on `dev`. GitHub default-branch switch and deletion of `origin/main` remain a later API step. Fast-forwarding environment-named mirrors is not live exposure. Environment-branch promotion (live DNS/TLS/exposure) remains HOLD until a later go-live PRODUCT. Argo `targetRevision: main` is unchanged.
   - Partition the exact 214-target PostgreSQL reachability inventory (229 mapped entries, 214 in-workflow targets). These are Cargo invocations, not test cases -- one entry runs one test binary, and `governance_rls_as_runtime_role` alone carries 12 tests. An exact test count must come from discovered/executed output, never from this number across isolated disposable databases while retaining a strict compatibility aggregate and proving no omission or duplication. The partitioning, the five disposable-database facet jobs, and the fail-closed aggregate are already implemented.
   - Prove Cargo test membership, feature-bearing reachability, JavaScript reachability, credential safety, and zero required Buck-only coverage before any Buck deletion.
   - Fix the remaining migration-parser gaps. The migration-safety gaps (`ALTER TABLE ONLY`, schema-qualified audited tables) are fixed. The personal-data-classification gaps remain open — a multi-action `ALTER TABLE` is still judged from its first action, and concatenation-split keywords are still unreachable — and migrations 0212-0221 were admitted since 2026-08-04 under the compensating catalog-based completeness assertion in `backend/crates/platform/db/tests/personal_data_classification.rs`, not under a parser fix.
4. **Architecture foundations**
   - Complete branchless capability and temporal-grant contracts, contracts-crate/OpenAPI composition, true preflight, and distinct-human approval rules.
5. **Organization and HR**
   - Done: explicit owning ports and single-writer boundaries are mechanically proven for Company, OrgUnit, JobPosition, Person, Employment, and PayRun, releasing projection fan-out only.
   - Tenant is always a Group. Every Company mints a real Group row (standalone = Group of one). Company remains the legal entity and RLS cell; Group never arms `app.current_org`. Cross-entity HR/payroll is policy on member companies, still one `org_id` per write. Two-org `consolidated_read` as `console_rt` is proven (#954). People create binds Person (#966). UUID OrgUnit/JobPosition refs fail closed (#968). Mandatory `organizations.group_id` plus 1:1 mint is done (#971). OrgUnit kinds Site/Department/Team are enforced (#975); parent cycles longer than 1 are refused (#981). Next: `hr.appoint` stays four-eyes, not directory create. `ObjectKey::Group` is not admitted by this record.
6. **Payroll**
   - Project the existing payroll writer without a second write path; preserve deterministic rounding, golden cases, immutable receipts, and payslip drafts.
   - Draft calculate is admitted. `payable`, Korea compliance conclusions, wage-statement legal sign-off, and payment execution remain **HOLD**.
7. **Leptos acceptance surface**
   - `Layer::Ui` is accepted (ADR-0041); first full-depth vertical is payroll execution.
   - GET `/_ui` is mounted (#952). Contracts-backed authorized reads (#959): empty shell without `PayrollRunRead`; `data-run-*` from `PayrollRunSummary` required fields with it. Those runs are a Leptos `#[island]` (#962) hydrated by committed release WASM (#964). Empty shell still omits `/_ui/pkg/`.
   - Shipping screens are composed (#976). Payroll drill-through hrefs and empty-vs-denied SSR landed (#982). Persona real-backend E2E (ADR-0025 §4) on org/HR/payroll GET `/_ui` landed (#978). Production exposure stays **HOLD**. Import/export is not the data-entry base except 자료실; the comms rail is out of this slice.
8. **Palantir AIP / Intelligence**
   - Palantir AIP is the target intelligence layer. It is built in the separate Intelligence repository until that repository names a SHA-bound stable base; cloning it into Console is a later lane.
   - Console-owned fail-closed seams are admitted now (same `Principal`, ontology actions, no second writer, no Palantir client). Owner lock (2026-08-29): **bind-only** Intelligence HTTP on the existing listener, `127.0.0.1` only, fail-closed if unset or non-loopback. Implemented (#972): `GET http://127.0.0.1:<listen-port>/internal/intelligence/bind`. Not published OpenAPI, not `/_ui`, not `/api/v1`. No chat, no inference text in tenant UX, no HR/payroll write, no autonomous merge. Not a tenant-app Intelligence product.

## Explicit HOLDs

- Bulk documentation moves, deletion, or graveyard copies are **HOLD** until custody and recoverability are proven with the full manifest and signed archive references.
- Live or production promotion, DNS, TLS, secrets, exposure, payment, credential-reset, and compliance claims are **HOLD** absent separate authority. A source release follows repository release authority and evidence; it does not authorize live promotion or exposure.
- The grandfathered OCI Ampere A1 instance (4 OCPU / 24 GB) is standing **off-limits**, not a program HOLD: never destroy, terminate, resize, or reprovision it; the reserved capacity cannot be recreated.
- Workstation full-disk erase **already occurred** (post-wipe, owner fact 2026-08-28). Remaining local P0 work is re-issue or off-device confirmation, not a future erase. Do not erase again as a program action.
- Cloning Intelligence into Console is **HOLD** until Intelligence names a SHA-bound stable base. Intelligence is not a tenant application and not autonomous merge authority. Console-owned seams are admitted.
- Ambiguous roadmap prose, historical plans, and unpublished or partial work do not clear a HOLD and do not dispatch implementation.

## Exit rule

A roadmap item advances only when its exact candidate, independent review, acceptance evidence, post-merge containment, and remaining HOLDs are recorded under [`DELIVERY.md`](DELIVERY.md). Partial completion remains open work.

## Proposed work after first-vertical integration

Preserve payroll as first complete vertical. First-vertical completion includes durable inputs, current policy, source entry/repair, partial-visibility human review, approved-draft supersession, monetary NONPAYABLE_REVIEW publication and historical own correction requests, automatic/manual task transfer, selected-Company Group progress and operational recovery. Native communication rail remains outside this first implementation slice while being an explicit later deliverable.

Then complete the detailed HR/attendance/labor/payroll story inventory and owner acceptance per story; native mail/messenger/calendar with independent communication navigation and policy-bound sharing; enterprise concurrent sheets over native objects/properties/actions with explicit formula/structure/reconnect semantics; and the versioned leaf-level Foundry/Ontology/Actions/Fusion/full-suite capability inventory with owners, dependencies, alternatives and acceptance evidence. These commitments do not postpone the first coherent implementation until the entire suite is built, and do not silently discard the larger target.

Sequence before implementation remains approved integrated design SHA after four independent review rounds, independently reviewed and approved executable test SHA, then a real named RED command admitted through fanout.py. Scope-authority reconciliation is a concrete reviewed change; adoption and hosted readback follow current DELIVERY. Production/payment/legal/provider deployment authority remains separate.
