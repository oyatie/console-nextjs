> **HISTORICAL / NON-AUTHORITY.** Planning snapshot only. Current authority: `docs/current/*`.

# Ontology lifecycle coverage matrix

This is a fixed-revision source audit of the accepted target
`86a97771a76b7e770dfcf8c6c7d83fd9d70a98bf` (tree
`fb94f53a5725357bc58b1f6ae6d4f441d5293516`). It is a **historical** planning snapshot (non-authority),
not a deployment or production-readiness record.

The Phase 0 Support-only overlay below was re-audited at exact source
`55d00f8aacaf8d1ba4db87b2f5345605af856a27`. It supersedes earlier
Support-SLO interpretation without changing the fixed-target denominator or
unrelated rows.

## Claim levels used here

- **Declared** — a schema, type, route, or UI descriptor is written in source.
- **Source-present** — the cited accepted-target implementation exists.
- **Tested** — an accepted-target test exercises the stated behavior; this does
  not mean the test was rerun for this documentation repair.
- **Deployed** — requires environment rollout/readback evidence. None was
  collected for this source audit.
- **Production-proven** — requires production telemetry or data readback. None
  was collected for this source audit.

Registration, backing, domain mutation, UI presentation, tests, deployment, and
production proof are separate facts throughout this matrix.

## Fixed-target denominator

Fixed-target seeded tenant-type denominator: **27**. It is derived from
`backend/crates/ontology/adapter-postgres/src/seed.rs:30-65,1076-1163,1193-1223`:

- 9 governed/config types with `BackingKind::Instance`;
- 3 C-chain types with `BackingKind::Instance`; and
- 15 domain-table projections with `BackingKind::Projected`.

`seed_published` creates then publishes each draft through the registry
(`backend/crates/ontology/adapter-postgres/src/seed.rs:1166-1187`). The onboarding
runtime-role test independently names the exact 27-key set
(`backend/crates/platform/platform-rest/tests/onboard_seeds_config_objects.rs:224-270`).
The registry records themselves are not an extra tenant type and generic
`ont_instances` rows are instances of the 12 instance-backed types, not another
seeded type.

<!-- fixed-target-seeded-types:start -->
| Stable key | Backing at the fixed target | Write/read boundary | Test ceiling |
|---|---|---|---|
| `approval` | Projected | `gov_approval_requests`; governance/domain use-case writes, ontology current-list read | All-15 registration/backing tested; this row's read not separately asserted |
| `compliance_framework` | Projected | `compliance_frameworks`; compliance/domain writes, ontology current-list read | All-15 registration/backing tested; this row's read not separately asserted |
| `compliance_obligation` | Projected | `compliance_obligations`; compliance/domain writes, ontology current-list read | All-15 registration/backing tested; this row's read not separately asserted |
| `compliance_regulation` | Projected | `compliance_regulation_impacts`; compliance/domain writes, ontology current-list read | All-15 registration/backing tested; this row's read not separately asserted |
| `console_view` | Instance | Ontology-owned `ont_instances` revisions | Config tests exercise create/revision behavior |
| `contract` | Instance | Ontology-owned `ont_instances` revisions | C-chain publish/link/traversal tests exist |
| `customer` | Projected | `registry_customers`; domain-owned writes, ontology current-list read | All-15 registration/backing tested; this row's read not separately asserted |
| `employee` | Projected | `employees`; HR/domain writes, ontology current-list read | Projected real-row and RLS read tested |
| `equipment` | Projected | `registry_equipment`; registry/domain writes, ontology current-list read | All-15 registration/backing tested; this row's read not separately asserted |
| `evidence` | Projected | `docs_evidence_objects`; docs-owned writes, ontology current-list read | Registration plus docs REST/RLS/custody tests exist |
| `handover_policy` | Instance | Ontology-owned `ont_instances` revisions | Group publish tested; per-type instance flow not separately asserted |
| `labor_refusal` | Instance | Ontology-owned `ont_instances` revisions | Group publish tested; per-type instance flow not separately asserted |
| `leave_request` | Projected | `leave_requests`; HR/domain writes, ontology current-list read | All-15 registration/backing tested; this row's read not separately asserted |
| `mail` | Projected | `email_messages`; comms/domain writes, ontology current-list read | All-15 registration/backing tested; this row's read not separately asserted |
| `messenger_thread` | Projected | `messenger_threads`; comms/domain writes, ontology current-list read | All-15 registration/backing tested; this row's read not separately asserted |
| `position` | Instance | Ontology-owned `ont_instances` revisions | C-chain publish/link/traversal tests exist |
| `posting` | Instance | Ontology-owned `ont_instances` revisions | C-chain publish/link/traversal tests exist |
| `profitability_analytic` | Instance | Ontology-owned `ont_instances` revisions | Group publish tested; per-type instance flow not separately asserted |
| `regulation_param` | Instance | Ontology-owned `ont_instances` revisions | Niche-config create/revision behavior tested |
| `shift_timetable` | Instance | Ontology-owned `ont_instances` revisions | Group publish tested; per-type instance flow not separately asserted |
| `site` | Projected | `registry_sites`; domain-owned writes, ontology current-list read | All-15 registration/backing tested; this row's read not separately asserted |
| `site_coverage` | Instance | Ontology-owned `ont_instances` revisions | Group publish tested; per-type instance flow not separately asserted |
| `sla_setting` | Instance | Ontology-owned `ont_instances` revisions | Group publish tested; per-type instance flow not separately asserted |
| `support_slo_setting` | Instance; **legacy/non-serving for Support timers** | Ontology-owned three-bucket (`incident`/`request`/`change`) revisions; taxonomy-incompatible with the six Support ticket categories and not consumed by ticket timers/alerts | Config tests exercise generic create/revision behavior only; they do not prove serving SLO policy, backend-atomic approval, or Support-category compatibility |
| `support_ticket` | Projected | `support_tickets`; support/domain writes, ontology current-list read; `due_at` is an SLA deadline, not SLO policy authority | All-15 registration/backing tested; this row's read and the serving SLO-policy relationship are not separately asserted |
| `work_order` | Projected | `work_orders`; workorder/domain writes, ontology current-list read | All-15 registration/backing tested; this row's read not separately asserted |
| `workflow_definition` | Projected | `workflow_definitions`; workflow/domain writes, ontology current-list read | Projected real-row and RLS read tested |
<!-- fixed-target-seeded-types:end -->

## Phase 0 Support SLO authority split

Status is **PARTIAL**. At the Support-only overlay revision, source has two
incompatible policy shapes:

- ticket timers, breach counts, and alert targets consume six local
  `SupportTicketCategory` defaults from `web/src/features/support/slo-settings.ts`;
- the ontology type `support_slo_setting` stores three independent
  `incident`/`request`/`change` instances and is displayed by the settings card,
  but does not feed those serving computations;
- the browser locally stages an edit, hides approval from the staging actor, and
  commits the three ontology instances as independent writes. This client-only
  flow is neither a backend approval decision nor aggregate atomicity; and
- `support_tickets.due_at` is derived by the Support SLA policy and must not be
  reclassified as internal SLO authority.

The approved next architecture is one Support-owned immutable aggregate with six
category rules, elapsed-only evaluation, and backend-atomic approval. This is a
target decision only. No implementation, migration identifier, backend route,
parity, deployment, or completion is recorded by this matrix.

The projected helper sets `BackingKind::Projected`, a backing table, and no
ontology actions (`backend/crates/ontology/adapter-postgres/src/seed.rs:166-187`).
The seeding function explicitly keeps domain use-cases as the sole writers
(`backend/crates/ontology/adapter-postgres/src/seed.rs:1091-1097`). The runtime
proof checks all 15 registrations/backing kinds, reads real `employee` and
`workflow_definition` rows, and checks tenant isolation
(`backend/crates/ontology/adapter-postgres/tests/projected_instances_read_as_runtime_role.rs:108-252`).
That test does not prove every projected table's row mapping or UI.

## Dynamic and lifecycle boundary

For instance-backed types, source-present generic revisions, lifecycle, hash
chain, `get_as_of`, and `history` use `ont_instances` and
`ont_instance_revisions`
(`backend/crates/ontology/adapter-postgres/src/instances.rs:1-14,341-415,633-699`).
For projected types, `list_instances` dispatches to a **projected current-list read** over the real domain table
(`backend/crates/ontology/adapter-postgres/src/instances.rs:417-455`). The cited
fixed-target `get_as_of` and `history` queries read `ont_instances` /
`ont_instance_revisions`; this audit therefore does not claim generic projected
as-of/history, generic projected writes, or generic projected analytics. A
domain may have its own lifecycle or history, but that must be cited separately.

## Evidence: registration, custody, UI, and tests

Evidence is a fixed-target **published projected/read** ontology type, not an
unregistered table:

- `EVIDENCE_KEY` and `evidence_draft` bind the projection to
  `docs_evidence_objects`
  (`backend/crates/ontology/adapter-postgres/src/seed.rs:51,405-440`;
  `evidence_draft` begins at `seed.rs:407`).
- The projection is published at `seed.rs:1122` inside the projected seeding
  path. The `seed.rs:1091-1097` contract keeps **docs-owned writes** in the docs
  use-case instead of adding a second ontology writer.
- The docs domain declares the bespoke 14-stage wire custody FSM as `CustodyStage`
  (`backend/crates/docs/domain/src/lib.rs:359-420`). The adapter creates
  `docs_evidence_objects` plus its initial custody event and appends chained
  custody events (`backend/crates/docs/adapter-postgres/src/lib.rs:140-197,1020-1093`).
- Source-present UI reads the real evidence REST surface and renders an
  `ObjectCard` (`web/src/console/evidence/EvidenceRecords.tsx:4-5,297`;
  `web/src/console/evidence/EvidenceCard.tsx:689`). Frontend presentation may
  synthesize `ACCESSED`; the resulting 15-state frontend presentation union is
  not a fifteenth wire custody state.
- Runtime-role tests cover tenant-scoped list/detail, hash-mismatch detection,
  and hold/disposal separation
  (`backend/crates/docs/rest/tests/evidence_rest_rls_surfaces_as_runtime_role.rs:214-360`).

These are Source-present and Tested claims at the accepted revision. They do not
establish Deployed or Production-proven evidence custody.

## Voucher and GL: domain implementation without seeded ontology registration

`finance_voucher` is absent from the exact 27 seeded ontology keys, so the
frontend registry label must not be treated as an engine seed. That does not
mean the backend is absent. The fixed target contains a domain-owned finance-GL
implementation:

- migration tables `finance_gl_vouchers` and `finance_gl_voucher_lines`, with
  RLS and database guards
  (`backend/crates/platform/db/migrations/0160_create_finance_gl_vouchers.sql:22-67,116-162`);
- `VoucherStatus` and `validate_voucher_transition` for
  Draft → BalanceChecked → Approved → Posted → Reversed
  (`backend/crates/finance-gl/domain/src/lib.rs:122-203`);
- audited adapter operations for balance checking, approval, posting, reversal,
  immutable posted lines, and separation of duties
  (`backend/crates/finance-gl/adapter-postgres/src/lib.rs:112-177,434-490,590-700`);
- runtime-role coverage in
  `backend/crates/finance-gl/adapter-postgres/tests/voucher_rls_and_fsm_as_runtime_role.rs`,
  including `self_approval_rejected_and_distinct_approver_posts` at lines
  416-463, plus balance, immutability, reversal, and RLS cases; and
- a Source-present frontend type descriptor/module surface
  (`web/src/console/modules/typeRegistry.ts:138-170`;
  `web/src/console/modules/moduleScreens.ts:349-474`).

The source and tests establish a real voucher/GL domain and FSM at the fixed
target. They do not establish generic ontology backing, deployment, accounting
certification, or production operation.

## UI and test audit ceiling for the remaining inventory

The inventory table deliberately distinguishes group-level publication tests
from row-specific behavior tests. For types other than the Evidence and Voucher
focus rows above, UI presence and domain-specific lifecycle behavior are
**unknown in this fixed-target pass** unless the table names an explicit test.
Published registration is not used as a proxy for a list screen, ObjectCard,
action wiring, end-to-end coverage, or production readiness.

## Revision-bound W1/W2 history

The previous matrix described W1/W2 as merged, CI-green, and operational. Those
statements were historical status notes and are not reproducible from the fixed
Git tree alone. This revision retains the source outcomes that can be checked:
27 published seed drafts are declared, projected reads and selected instance
flows have accepted-target tests, Evidence has docs-owned custody, and finance
has a voucher/GL implementation. CI result, rollout, activation, and production
behavior remain unverified here.

## Current planning implications

1. Keep `support_slo_setting` classified as legacy/non-serving until the approved
   Support-owned immutable six-category aggregate exists and the six local
   defaults no longer drive timers/alerts. Preserve SLA `due_at` as a separate
   contract and require backend-atomic approval evidence before promotion.
2. Preserve the one-writer rule for projected types; add write dispatch only
   through explicit domain use-case adapters and tests.
3. Do not promise generic projected as-of/history from registry publication;
   the cited generic temporal queries are instance-store queries.
4. Close row-specific read/action/UI tests where the all-15 publication proof is
   the present ceiling.
5. Keep finance voucher ontology registration as a separate design decision;
   the domain implementation already exists and must not be rebuilt as a second
   source of truth.
6. Require environment evidence before changing any Source-present or Tested
   entry to Deployed or Production-proven.
