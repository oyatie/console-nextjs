# Post-pivot Console roadmap

> **HISTORICAL / NON-AUTHORITY:** Current ordered work and HOLDs live in [`../current/ROADMAP.md`](../current/ROADMAP.md). The roadmap below is retained in place as history and cannot dispatch work or close an issue.

Status: historical roadmap retained for path stability; non-authority.

## Wave 0 — truth and delivery substrate

- Preserve dirty/stale work; work from a clean exact `origin/main` base.
- Reconcile root/program authority and retire the purchase slice and broad ERP/comms/mobile scope.
- Prove Cargo test membership, feature-bearing reachability, JavaScript reachability, credential safety, and zero required Buck-only coverage; delete Buck paths only in a dedicated later train.
- Fix migration parser handling for `ALTER TABLE ONLY` and schema-qualified audited tables before adding migrations.
- Extract only reviewed pivot-aligned branch artifacts. Historical experiments remain HOLD.

## Wave 1 — architecture foundations

Land disjoint lanes for ResourceBranch/branchless capability APIs and ADR-0032 temporal grants; a contracts crate and OpenAPI 3.1 composition; and delegation rules that always require distinct requester and approver humans. Migration numbering is assigned only at integration.

## Wave 2 — engine and organization reference

Use one validation path for true preflight and atomic execute. Retain `company_conformance` only as
an isolated generic-engine regression: its instance-backed fixtures omit Person and are not the
Company/HR product target. A replacement conformance target, Company projection, and any
JobPosition fan-out remain HOLD until Company, Person, Employment, and PayRun each have an explicit
owning port and a proven single-writer boundary. Complete the contracts/OpenAPI cutover without
inventing those projection contracts.

## Wave 3 — operational HR

After the projected ownership contracts above are accepted, create the canonical HR assignment
writer for appointment, promotion, and transfer. Close/open assignment and grant intervals at the
same effective instant; preserve deterministic identity, replay, conflict, revision, audit,
receipt, nondisclosure, and no-mutation preflight behavior. `orgchange` invokes HR ports and never
writes assignment truth.

## Wave 4 — payroll

After the PayRun owning-port contract is accepted, project the existing payroll writer without a
second write path. Consume versioned employment/attendance inputs and preserve deterministic
rounding, golden cases, immutable receipts, and payslip drafts. Payment execution and compliance
claims remain excluded.

## Wave 5 — Leptos

Only after every ADR-0030 gate is freshly green: accept `Layer::Ui`, land a contracts-only SSR shell, then build organization, HR, and payroll surfaces using real data and deny-by-omission. No placeholders, comms rail, or client business logic.

## Exit rule

A wave completes only when its exact candidate, independent reviews, acceptance evidence, post-merge containment, and remaining HOLDs are recorded. No live production, DNS, TLS, secret, release, exposure, or compliance-claim action is part of this roadmap.
