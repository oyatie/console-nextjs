# Architecture decision records

This directory is the local decision authority for Console within the product boundary fixed by
[`docs/PIVOT-2026-07-28.md`](../PIVOT-2026-07-28.md). The index is reviewed against `origin/main` and
must be updated atomically with every ADR status, identity, amendment, or supersession change.

## Authority rules

1. An **accepted** local ADR is authoritative within its stated scope. It applies only when consistent with the canonical pivot.
2. Within the ADR set, only another **accepted** local ADR may amend or supersede it.
3. A later number does not win automatically. Amendment or supersession must be explicit in both records.
4. `proposed`, `draft`, `design-note`, plan, prototype, and DARK material cannot supersede an accepted ADR.
5. Sibling-project records must be namespaced (for example, `oyatie ADR-0240`). They are references until a local accepted ADR adopts a specific rule.
6. Current implementation/live evidence may show that code diverged from an ADR; that is a governance gap, not silent supersession. Reconcile it through a new decision.
7. `ADR-0013` was a plan-only APNs placeholder and was never issued. Do not reuse or backfill the number.

Required ADR frontmatter:

```yaml
id: ADR-0000
status: proposed | accepted | superseded | rejected | withdrawn
doc_status: review | published | archived
date: YYYY-MM-DD
owner: name
related: []
```

`related` is always required and uses an inline list, including `related: []`. Relationship keys (`amends`, `amended_by`, `supersedes`, `superseded_by`, `related`) use local ADR IDs and must be reciprocal where applicable. A proposed record may use `proposes_amendments_to`; it cannot declare active `amends` or `supersedes` authority. Design notes live under `notes/` and declare `kind`, `parent_adr`, `authority: subordinate`, and activation state.

## Current index

| ID | Status | Decision and scope |
|---|---|---|
| [ADR-0001](ADR-0001-modularmonolith-cargo-workspace-with-compilerenforced-cleanarchitecture.md) | accepted, amended | Modular-monolith Rust workspace and compiler-enforced layering; crate family gains `ui` and `Layer::Ui` edges by ADR-0041 |
| [ADR-0002](ADR-0002-auditfirst-transactional-discipline-audit-event-in.md) | accepted, amended | Audit event in the same transaction; append-only audit store; exclusion-set cardinality and binding amended by ADR-0029 and ADR-0040 |
| [ADR-0003](ADR-0003-branchscoped-authorization-model-nonnull-branch-scope.md) | accepted, amended | Non-null branch scope and default-deny authorization; `BranchScope::All` derivation and `org_id` composition amended by ADR-0028 |
| [ADR-0004](ADR-0004-passkeysfirst-auth-with-rotating-refreshtoken-families.md) | accepted | Passkey-first local auth and rotating refresh-token families |
| [ADR-0005](ADR-0005-seaweedfs-primary-oci-object-storage-worm.md) | accepted, amended | SeaweedFS primary and a context-appropriate independent WORM replica; amended by ADR-0024 self-host-first portable seams |
| [ADR-0006](ADR-0006-p1-broadcastaccept-dispatch-with-livegps-scoring.md) | accepted | P1 broadcast-accept dispatch and live-GPS scoring |
| [ADR-0007](ADR-0007-postgrespersisted-messenger-with-listennotify-multiinstance-fanout.md) | accepted | Postgres messenger and LISTEN/NOTIFY fan-out |
| [ADR-0008](ADR-0008-excel-export-engine.md) | accepted | Excel export engine |
| [ADR-0009](ADR-0009-dualnative-swiftkotlin-parity-strategy-via-single.md) | accepted, amended; historical mobile scope outside pivot | Historical dual-native client decision; ADR-0031 amends its contract mechanism, while the canonical pivot removes all client/mobile surfaces from current scope and forbids their resurrection without later authority |
| [ADR-0010](ADR-0010-integration-seams-as-ports-only-oyatie.md) | accepted, amended | Oyatie AI port; speculative identity-provider portion amended by ADR-0022 |
| [ADR-0011](ADR-0011-apalis-10rc-isolated-behind-a-jobqueue.md) | accepted | Apalis isolated behind `JobQueue` |
| [ADR-0012](ADR-0012-monorepo-layout-for-four-deliverables-contract.md) | accepted; historical deliverable set outside pivot | Historical four-deliverable monorepo decision; the current pivot retains one repository but removes React/native clients and required-context path filters |
| ADR-0013 | never issued | Plan-only APNs placeholder; reserved historical gap |
| [ADR-0014](ADR-0014-locationping-destructible-store-carved-out-of.md) | accepted, amended | Destructible location store outside the append-only audit store; audit-coverage exclusion cardinality amended by ADR-0029 |
| [ADR-0015](ADR-0015-dr-posture-wal-archiving-continuous-pitr.md) | accepted, amended | Continuous PITR/degraded-mode invariants plus context-specific multi-node/multi-site target; amended by ADR-0024 |
| [ADR-0016](ADR-0016-oyatie-ai-assistant-port-contract.md) | accepted | Oyatie AI assistant port contract |
| [ADR-0017](ADR-0017-superseded-identity-provider-port-contract.md) | superseded | Superseded in full by ADR-0022 |
| [ADR-0018](ADR-0018-clean-room-rust-corporate-workflow-engine.md) | accepted | Clean-room Rust corporate workflow engine |
| [ADR-0019](ADR-0019-standalone-mailbox-server-build-vs-adopt.md) | accepted, amended, reconciliation required | Clean-room Rust mailbox default; ADR-0024 makes self-host the first deployment envelope. Mox design/DARK implementation still needs a newer accepted decision before activation |
| [ADR-0020](ADR-0020-korean-institutional-connectivity-coverage-factory.md) | accepted, fixture-only | Institutional connector coverage factory; no live institution access |
| [ADR-0021](ADR-0021-cedar-pbac-authorization-strangler.md) | accepted target only | Cedar/PBAC strangler baseline; no live enforcement switch |
| [ADR-0022](ADR-0022-local-identity-no-external-idp.md) | accepted, amended | Local passkey identity; no speculative external IdP seam; cross-tenant identity linkage narrowed to a human assertion by ADR-0027 |
| [ADR-0023](ADR-0023-oyatie-console-authority.md) | accepted, amended | Console product/workflow authority; shared-chrome composition and coexistence clauses amended by ADR-0025; historical COSS RN follow-up amended by ADR-0026 |
| [ADR-0024](ADR-0024-bare-metal-portability-and-ha.md) | accepted | Self-host first; cloud-agnostic core through ports/adapters; Oyatie Cloud, AWS, OCI, Azure, and GCP remain first-class and may use native capabilities behind replaceable context boundaries |
| [ADR-0025](ADR-0025-carbon-copy-console-shared-platform-spine.md) | accepted, amended | Amends ADR-0023 with an isolated carbon-copy `/console` visual system, one shared platform spine, staged rollout, full-stack slice gates, and measured legacy deletion; structural prescriptions — carbon-copy visual authority, the `web/src/console/**` path and two-shell composition, and the spine boundary as enumerated — amended by ADR-0030; its §4 nine-item slice bar and §3 product semantics remain in force |
| [ADR-0026](ADR-0026-retire-coss-rn-public-site-surface.md) | accepted | Retire the standalone COSS RN public-site surface; remove it from npm workspaces and do not cite its historical evidence for Console parity or releases |
| [ADR-0027](ADR-0027-identity-linkage-human-asserted.md) | accepted | Identity linkage across tenants is human-asserted by a user-verified WebAuthn assertion; no platform `party` row, no `users.party_id` in Slice 0, with numbered non-foreclosure constraints; amends ADR-0022 |
| [ADR-0028](ADR-0028-org-id-branchscope-composition.md) | accepted | `org_id` × `BranchScope` composition, capability-or-membership-derived `BranchScope::All`, and an explicit tenant predicate on realtime fan-out; amends ADR-0003 |
| [ADR-0029](ADR-0029-audit-coverage-exclusions-are-two.md) | accepted, amended | Audit-coverage exclusions were reconciled to two bound LocationPing writers; cardinality lifted to three by ADR-0040 (telemetry); amends ADR-0002 and ADR-0014 |
| [ADR-0030](ADR-0030-console-rebuild-charter-leptos.md) | accepted, amended | Charters the console rebuild on Leptos with an SSR shell and island editors for authorization reasons, a domain-first repository convention with no stack split, and frontend implementation gated on the ontology engine substrate; withdraws ADR-0025's carbon-copy visual authority, `web/src/console/**` path and two-shell composition, and enumerated spine boundary, and retains its nine-item slice bar; `Layer::Ui` members allowed by ADR-0041; §8 absence-as-green (no mounted shell) is not retired (additive observation; Decision text retained) |
| [ADR-0031](ADR-0031-contracts-crate-single-internal-contract.md) | accepted | A Rust wire-DTO contracts crate is the single internal API contract, with `openapi.yaml` emitted from it and diff-gated; amends ADR-0009's contract mechanism |
| [ADR-0032](ADR-0032-effective-dated-grants-and-authority-freshness.md) | accepted | Effective-dates the role grant only, keeps the authority fold per-request and uncached citing ADR-0021 §4/§5 as enabling, and refuses an as-of authority read while six of the fold's seven inputs are head-valued |
| [ADR-0033](ADR-0033-object-policy-revocation.md) | accepted | An over-broad object-policy permit is correctable by attaching a forbid; a mistaken forbid has no reversal write. Records the asymmetry and specifies revocation as a one-landing catalog status transition, unbuilt until an incident is counted |
| [ADR-0034](ADR-0034-delegation-of-authority-routing.md) | accepted | 전결규정 routing as a delta on ADR-0023's approval-line model: routing is a lookup that may resolve above, laterally or below the raising unit; competence is a third relation beside control and structure; a signature records the capacity it was made under |
| [ADR-0035](ADR-0035-conserved-quantity-lineage.md) | accepted | Quantity-bearing split/merge lineage deferred; conservation requires row-level `FOR UPDATE` locking and a pure domain predicate, and the row CHECK is a per-row backstop only |
| [ADR-0036](ADR-0036-object-dimensioned-economics.md) | accepted | Cost is a query over the double-entry voucher dimensioned by object reference; the finance subsystem is a peer plan, and the missing line dimension, `accounting_date` + period-lock caller, account master, and currency shape must stay additive |
| [ADR-0037](ADR-0037-erasure-versus-pitr-conflict.md) | proposed, question only | Names the conflict between ADR-0014's shipped destruction paths and ADR-0015's restore capability over the live 35-day restorable window, prices crypto-shredding, a shorter window, a segregated store and an accepted conflict against that proof, and routes the choice to privacy counsel — distinct from the 노무사/세무사 payroll sign-off; decides nothing and asserts no Korean legal conclusion |
| [ADR-0038](ADR-0038-location-erasure-unlogged-then-crypto-shred.md) | proposed, mechanism for one data class | Decides HOW 개인위치정보 is erased, where ADR-0037 only names the conflict: declare `location_pings` UNLOGGED so coordinates never enter the WAL, base backups or standbys; run the never-called `purge_expired_location_data`; then envelope-encrypt per subject in a gated phase 2. Places the segregation boundary at `relpersistence` rather than a second cluster, which keeps the outbound RESTRICT foreign keys, FORCE-RLS enrolment, five org-removal deletes and single-transaction consent withdrawal intact. Retracts its own draft's collapse-to-one-row-per-subject after its falsification test found dispatch eligibility reads a time window. Asserts no Korean legal conclusion |
| [ADR-0039](ADR-0039-one-graph-tests-run-by-existing.md) | proposed | Proposes one-graph test execution (originally framed as Cargo/nextest under RE/CAS absence). **DN-0005** recorded the 2026-08-05 cargo-primary cutover under CAS absence. **DN-0006** (2026-08-11) supersedes that planning path: shared NativeLink CAS is up and founder decision is Buck2-everywhere; still proposed / grants no Buck delete authority alone. **DN-0007** (2026-08-18) corrects DN-0006's substrate claim: the Wave A overlays were inert and `GREEN_REAPI` proved server reachability only; warm cache is now measured working (100% hits) with a remote-cache execution platform, and Buck2 hard-fails when the CAS is down. **DN-0008** (2026-08-19) withdraws DN-0006's Buck2-everywhere sequence: its premise was false, Required CI is 36 cargo vs 5 buck2 invocations, and the Cargo-vs-Buck2 comparison was never run — Cargo stays the CI driver, Buck2 becomes a nightly hermeticity check |
| [ADR-0040](ADR-0040-audit-coverage-console-telemetry-carve-out.md) | accepted | Third bound audit-coverage carve-out for `app/src/console_telemetry.rs::insert_route_telemetry`; amends ADR-0029 and ADR-0002 after app-surface expansion (console-937) |
| [ADR-0041](ADR-0041-layer-ui-accepted.md) | accepted | Accepts `Layer::Ui` (`allowed_deps` `[Contracts, Ui]`; App gains Ui; Ui forbids `sqlx`), classifies `-ui` before `crates/platform/`, and allows `-ui` members; does not resume frontend implementation or authorize shipping screens; does not retire ADR-0030 §8 absence-as-green; amends ADR-0001 and ADR-0030 |

## Effective relationship graph

- ADR-0022 amends the identity-provider portion of ADR-0010 and supersedes ADR-0017.
- ADR-0005, ADR-0015, and ADR-0019 remain accepted and are amended—not erased—by ADR-0024. For ADR-0019, only the OCI-first deployment-resource envelope changes; the mailbox build-vs.-adopt decision remains. The fully working self-host reference is now the first portability delivery gate; Oyatie Cloud and provider-native cloud adapters follow without losing first-class status.
- ADR-0024's context-native identity seam means workload/infrastructure identity only. It does not amend ADR-0022's local product-user identity or authorize a speculative external IdP/federation seam.
- ADR-0025 amends ADR-0023's shared-chrome composition and non-feature-flag coexistence clauses. ADR-0023 remains accepted for `/overview`, Work Hub/My Work semantics, workflow-engine direction, policy/audit rules, and the fully-wired/no-stub delivery contract.
- ADR-0019 remains the mail-server authority. Mox is DARK and unresolved, not silently accepted.
- ADR-0026 narrowly amends ADR-0023's historical COSS RN follow-up, records a product-surface retirement outside ADR-0009's Console parity scope and ADR-0012's four deliverables, and does not amend either of those decisions.
- ADR-0027 amends ADR-0022 by adding one prohibition its integration clause did not contain — linking two accounts to one identity — and narrows rather than widens ADR-0022. It also records that ADR-0022 never decided the platform-identity question it has been cited for; ADR-0022's decision against a speculative external IdP seam and its `console-identity-application` scope clause remain accepted in full.
- ADR-0028 amends one clause of ADR-0003: the `BranchScope::All` derivation, formerly keyed to SUPER_ADMIN/EXECUTIVE role literals and now derived only from a built-in `Feature` capability or a live database membership proof. ADR-0003 remains accepted for its `Branch`/`Region` day-1 schema, default-deny repository filtering, and branch-scoped broadcast/rollup rules. ADR-0018 and ADR-0021 are `related` only and are not amended.
- ADR-0029 narrowly amends the audit-coverage exclusion sentence in ADR-0002 and in ADR-0014, whose stated cardinality of one disagreed with the gate's two, and records a pre-existing governance gap under authority rule 6. It amends neither ADR-0002's same-transaction or append-only decisions nor ADR-0014's destructible-store decision. At acceptance the owner widened it from ADR-0002 alone to both targets so the twin false sentence in ADR-0014 was corrected in the same change rather than left standing.
- ADR-0040 amends ADR-0029's closed-at-two cardinality (and ADR-0002's matching sentence) to add one bound console route-telemetry carve-out after `backend/app/src` entered the audit-coverage handler-surface set (console-937 / gh#396). LocationPing bindings from ADR-0029 remain in force.
- ADR-0030 amends ADR-0025's stack-bound structural prescriptions only — the carbon-copy visual authority, the `web/src/console/**` path and two-shell composition, and the spine boundary as enumerated. ADR-0025 remains accepted for its §4 nine-item slice bar, `/overview` and Work Hub semantics, workflow/policy authority, and rollout discipline. ADR-0023 is `related` only, and ADR-0001, ADR-0009, and ADR-0012 gain nothing from it. ADR-0041 later accepts `Layer::Ui` (the ADR-0001 amendment ADR-0030 §6 deferred) without retiring §8 absence-as-green; that is an amendment of ADR-0030, not of ADR-0025.
- ADR-0041 amends ADR-0001 by adding `ui` to the crate family and accepting `Layer::Ui` with legal edges `Ui → {Contracts, Ui}` and `App → Ui`. It amends ADR-0030 by allowing `-ui` members without rewriting ADR-0030's historical Decision text and without retiring §8's no-mounted-shell CI assertion. Classification of the `-ui` suffix precedes `crates/platform/`. Import/export is not the data-entry base (자료실 later); the comms rail is out of this slice.
- ADR-0031 amends the contract-mechanism half of ADR-0009's Decision only. The canonical pivot independently places ADR-0009's ts/swift/kotlin client-generation, dual-build, and web+Android-then-iOS sequencing outside current scope; they cannot dispatch work or resurrect deleted surfaces.

### Accepted without amendment — `related` only

Nothing in this subsection is an amendment. Each of these records reached acceptance with `related` links only: no target ADR carries an `amended_by` key for any of them, and no target's Decision text became false on their acceptance. Each states that conclusion as a checked one rather than an omission.

- ADR-0032 reads ADR-0021 §4/§5 as enabling effective-dated grants and adds a scoped-replay invariant without changing ADR-0021's scope. ADR-0002, ADR-0003, and ADR-0021 gained it in `related`.
- ADR-0033 records a measured property of the shipped object-policy path and specifies what a revocation path must satisfy. Neither ADR-0021 nor ADR-0023 needed an edit; object-policy revocation is outside ADR-0023's stated scope, which under rule 1 is silence rather than prohibition.
- ADR-0034 adds the 전결규정 routing delta on top of ADR-0023's approval-line model and ADR-0018's engine, and changes nothing ADR-0023 decided about finality. It named no `related` additions in its targets and none were made; its `related` list stays one-sided by authorship, which `related` permits since the gate reciprocates only `amends`/`amended_by` and `supersedes`/`superseded_by`.
- ADR-0035 defers quantity-bearing split/merge lineage and fixes the mechanism story for the conservation already shipped. ADR-0001, ADR-0002, ADR-0018, and ADR-0029 gained it in `related`.
- ADR-0036 draws a boundary around the existing money store and names what a future finance subsystem must not foreclose. ADR-0002, ADR-0003, and ADR-0023 gained it in `related`.

**These ten were drafted together and were all accepted on 2026-07-30.** ADR-0027 through ADR-0036 were drafted as one pass and accepted the same day. Rule 7 reserves ADR-0013 alone; no other number in this range is skipped. They were not a package deal: each stood or fell on its own evidence, and each was read against the code before its clauses were made binding. Their historical acceptance process included reciprocal target edits. The later canonical pivot now requires ADR bodies to remain historical and current divergence to be recorded through additive status or supersession notes; do not repeat in-place Decision rewrites.

At acceptance, four target sentences were edited in place against executable evidence: the audit-coverage exclusion counts in ADR-0002 and ADR-0014, ADR-0003's `BranchScope::All` derivation, and ADR-0009's contract mechanism. That history is retained, not a precedent. Current readers use explicit amendment relationships and additive observations, and the canonical pivot—not the deleted-client clauses—controls product scope.

## Subordinate design notes

| Note | Parent | Activation | Scope |
|---|---|---|---|
| [DN-0001](notes/DN-0001-adr-0024-ha-workload-scheduling.md) | accepted ADR-0024 | DARK | First self-host HA workload scheduling expectations; not activation evidence |
| [DN-0002](notes/DN-0002-adr-0024-on-prem-vip-ingress.md) | accepted ADR-0024 | DARK | First self-host on-prem VIP/ingress approach; not activation evidence |
| [DN-0003](notes/DN-0003-adr-0025-operational-object-runtime.md) | accepted ADR-0025 | IN PROGRESS | Palantir-derived operational object runtime, deterministic Actions, object-focused tooling, and governed scenario direction; not release evidence |
| [DN-0004](notes/DN-0004-adr-0028-branchless-capability-authorization.md) | accepted ADR-0028 | SHIPPED | `authorize_capability` for resources with no branch, and the single custom-grant verdict the migration moves (partial grant on a multi-branch principal, previously decided by branch-id sort order); no exposure change |

## Planning entry

Current facts, unresolved decisions, and planning stop gates are revision-bound evidence maintained separately under `docs/program`. Do not begin from an ADR number alone; read any higher-numbered accepted ADRs and follow explicit relationship fields.
